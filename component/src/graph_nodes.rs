//! The material graph nodes the materials bring themselves: nodes whose WGSL
//! is the materials' own (`//!use material_row`), so they exist only where a
//! material program composes the graph (every compile of a material graph
//! does: [`crate::graph::compile_material_graph`]).
//!
//! They join PSGC's node catalogue (`wgsl_std::SHADER_REGISTRY`), which the
//! compiler, the graph editor's palette and the import companion all read.

use graphy::core::{NodeMetadata, NodeTypes, ParamInfo};
use helio_mats::TextureTransform;
use linkme::distributed_slice;

/// The texture transform node: a texture slot's UV transform (glTF's
/// `KHR_texture_transform`, [`TextureTransform`]) applied to a UV, for a
/// sample to read at. Its WGSL is the materials' `material_uv_transform`,
/// the function every material row's texture slot is sampled through, so a
/// graph and a row read a transformed texture alike. Its gradients follow
/// it into stages without implicit derivatives
/// (`helio_mats::shader::uv_transform_gradients`).
pub const TEXTURE_TRANSFORM: &str = "texture_transform";

#[distributed_slice(wgsl_std::SHADER_REGISTRY)]
fn texture_transform() -> NodeMetadata {
    NodeMetadata::new(TEXTURE_TRANSFORM, NodeTypes::pure, "Texture")
        .with_params(vec![
            ParamInfo::new("uv", "vec2<f32>"),
            ParamInfo::new("offset", "vec2<f32>"),
            ParamInfo::new("scale", "vec2<f32>"),
            // Radians.
            ParamInfo::new("rotation", "f32"),
        ])
        .with_return_type("vec2<f32>")
        .with_source(
            "material_uv_transform(vec4<f32>(offset, scale), vec4<f32>(sin(rotation), cos(rotation), 0.0, 0.0), uv)",
        )
}

/// The value an unwired input of a node of `kind` takes when the graph
/// stores none, where the type's zero would be wrong (a texture transform's
/// scale is 1): the node's identity. The graph editor places nodes with
/// these, and every compile fills them in ([`crate::graph::prepare`]).
pub fn input_defaults(kind: &str) -> &'static [(&'static str, &'static [f64])] {
    match kind {
        TEXTURE_TRANSFORM => &[
            ("offset", &[0.0, 0.0]),
            ("scale", &[1.0, 1.0]),
            ("rotation", &[0.0]),
        ],
        _ => &[],
    }
}

/// `value` as a JSON graph input default: a number for one component, an
/// array of numbers for a vector (what PSGC lowers to `vec2<f32>(...)`).
pub fn default_value(value: &[f64]) -> serde_json::Value {
    match value {
        [single] => serde_json::json!(single),
        many => serde_json::json!(many),
    }
}

/// A texture transform node's inputs for `transform`, as the graph stores
/// them (by input pin).
pub fn texture_transform_properties(
    transform: &TextureTransform,
) -> [(&'static str, serde_json::Value); 3] {
    let float = |value: f32| f64::from(value);
    [
        (
            "offset",
            serde_json::json!([float(transform.offset[0]), float(transform.offset[1])]),
        ),
        (
            "scale",
            serde_json::json!([float(transform.scale[0]), float(transform.scale[1])]),
        ),
        (
            "rotation",
            serde_json::json!(float(transform.rotation_radians)),
        ),
    ]
}

/// The transform a texture transform node's stored inputs describe, its
/// defaults where it stores none; `None` when an input is not a constant
/// of its type (the depth passes cannot evaluate a wired one).
pub fn texture_transform_of(node: &psgc::NodeInstance) -> Option<TextureTransform> {
    let input = |pin: &str| -> Option<Vec<f32>> {
        let default = input_defaults(TEXTURE_TRANSFORM)
            .iter()
            .find(|(name, _)| *name == pin)
            .map(|(_, value)| value.iter().map(|v| *v as f32).collect());
        match node.properties.get(pin) {
            None | Some(serde_json::Value::Null) => default,
            Some(value) => numbers(value),
        }
    };
    let pair = |pin: &str| -> Option<[f32; 2]> {
        match input(pin)?.as_slice() {
            [x, y] => Some([*x, *y]),
            _ => None,
        }
    };
    let rotation = match input("rotation")?.as_slice() {
        [r] => *r,
        _ => return None,
    };
    Some(TextureTransform {
        offset: pair("offset")?,
        scale: pair("scale")?,
        rotation_radians: rotation,
    })
}

/// The numbers a stored input holds: a number, an array of numbers, or
/// either as text (what the graph editor's fields and older files store).
pub fn numbers(value: &serde_json::Value) -> Option<Vec<f32>> {
    match value {
        serde_json::Value::Number(number) => Some(vec![number.as_f64()? as f32]),
        serde_json::Value::Array(items) => items
            .iter()
            .map(|item| item.as_f64().map(|v| v as f32))
            .collect(),
        serde_json::Value::String(text) => {
            let text = text.trim();
            if text.starts_with('[') {
                numbers(&serde_json::from_str(text).ok()?)
            } else {
                Some(vec![text.parse().ok()?])
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_texture_transform_node_joins_the_catalogue() {
        let node = psgc::get_shader_nodes()
            .into_iter()
            .find(|node| node.name == TEXTURE_TRANSFORM)
            .expect("registered");
        let params: Vec<_> = node.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(params, ["uv", "offset", "scale", "rotation"]);
        assert!(node.function_source.contains("material_uv_transform("));
    }

    /// An input the graph leaves unset takes the node's identity default
    /// when the graph compiles, and stored values of any form read back.
    #[test]
    fn unset_inputs_take_the_identity_and_stored_ones_read_back() {
        let mut graph = psgc::GraphDescription::new("t");
        let mut node =
            psgc::NodeInstance::new("t", TEXTURE_TRANSFORM, psgc::Position { x: 0.0, y: 0.0 });
        node.properties
            .insert("offset".into(), serde_json::json!("[0.5, -2.4]"));
        graph.add_node(node.clone());
        let prepared = crate::graph::prepare(&graph);
        let prepared = &prepared.nodes["t"];
        assert_eq!(prepared.properties["scale"], serde_json::json!([1.0, 1.0]));
        assert_eq!(prepared.properties["rotation"], serde_json::json!(0.0));
        assert_eq!(
            prepared.properties["offset"],
            serde_json::json!([0.5, -2.4])
        );
        assert_eq!(
            texture_transform_of(&node),
            Some(TextureTransform {
                offset: [0.5, -2.4],
                ..TextureTransform::IDENTITY
            })
        );
        node.properties
            .insert("scale".into(), serde_json::json!(3.0));
        assert_eq!(texture_transform_of(&node), None, "a scale is two numbers");
    }
}
