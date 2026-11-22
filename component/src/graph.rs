//! Lower compiled PSGC material graphs into Helio surface evaluation.
//!
//! The one material graph compiler: material assets compile their graphs
//! with it ([`crate::assets`]), as do the editors' previews
//! ([`crate::graph_preview`]).

/// Only the output's dependency chain owns runtime texture dependencies.
pub fn texture_assets(graph: &psgc::GraphDescription) -> Result<Vec<String>, String> {
    let mut reachable = std::collections::HashSet::new();
    let mut pending: Vec<_> = graph
        .nodes
        .values()
        .filter(|n| n.node_type == "fragment_output")
        .map(|n| n.id.clone())
        .collect();
    while let Some(id) = pending.pop() {
        if reachable.insert(id.clone()) {
            pending.extend(
                graph
                    .connections
                    .iter()
                    .filter(|c| c.target_node == id)
                    .map(|c| c.source_node.clone()),
            );
        }
    }
    let mut assets = std::collections::BTreeSet::new();
    for node in graph
        .nodes
        .values()
        .filter(|n| reachable.contains(&n.id) && is_texture_sample(&n.node_type))
    {
        let asset = node
            .properties
            .get("texture")
            .and_then(serde_json::Value::as_str)
            .filter(|a| !a.trim().is_empty())
            .ok_or_else(|| format!("Texture sample '{}' has no texture asset selected", node.id))?;
        assets.insert(asset.to_string());
    }
    let mut assets: Vec<String> = assets.into_iter().collect();
    // A texture-alpha mask takes ordinal 0, the row's base colour slot: the
    // depth and shadow passes cut out with it (`material_cut_out`).
    if let GraphMask::TextureAlpha { texture: mask, .. } = opacity_mask(graph) {
        if let Some(at) = assets.iter().position(|asset| *asset == mask) {
            let mask = assets.remove(at);
            assets.insert(0, mask);
        }
    }
    Ok(assets)
}

/// What drives a graph's `opacity_mask` (its cut-out coverage), as the passes
/// that cut out without running the graph see it.
#[derive(Clone, Debug, PartialEq)]
pub enum GraphMask {
    /// Not connected: the material is not masked.
    None,
    /// The alpha of `texture` sampled at the mesh UV through `transform`
    /// (identity unless a constant texture transform node feeds the
    /// sample): the material row cuts out with it in every pass
    /// (`FLAG_ALPHA_TEST`, ordinal 0, the slot's transform in the row's
    /// texture data, which `material_cut_out` samples through).
    TextureAlpha {
        texture: String,
        transform: helio_mats::TextureTransform,
    },
    /// Any other expression: only a pass running the graph could evaluate it.
    Other,
}

/// Whether `node` is the mesh UV (UV set 0): a UV input node, or a texture
/// coordinate node reading set 0.
fn is_mesh_uv(node: &psgc::NodeInstance) -> bool {
    match node.node_type.as_str() {
        "frag_uv" | "vertex_uv" => true,
        "texture_coordinate" => node
            .properties
            .get("index")
            .and_then(crate::graph_nodes::numbers)
            .is_none_or(|index| index.first().is_none_or(|index| *index == 0.0)),
        _ => false,
    }
}

/// How `graph`'s output drives `opacity_mask`.
pub fn opacity_mask(graph: &psgc::GraphDescription) -> GraphMask {
    let source_of = |node: &str, pin: &str| {
        graph
            .connections
            .iter()
            .find(|c| c.target_node == node && c.target_pin == pin)
            .and_then(|c| graph.nodes.get(&c.source_node))
    };
    let Some(output) = graph.nodes.values().find(|n| n.node_type == "fragment_output") else {
        return GraphMask::None;
    };
    let Some(mask) = source_of(&output.id, "opacity_mask") else {
        return GraphMask::None;
    };
    let Some(sample) = (mask.node_type == "component_a")
        .then(|| source_of(&mask.id, "vec"))
        .flatten()
        .filter(|sample| sample.node_type == "sample_texture")
    else {
        return GraphMask::Other;
    };
    // Sampled at the mesh UV (the pin unconnected or fed by the UV), or at
    // the mesh UV through one texture transform of constant inputs.
    let transform = match source_of(&sample.id, "uv") {
        None => Some(helio_mats::TextureTransform::IDENTITY),
        Some(uv) if is_mesh_uv(uv) => Some(helio_mats::TextureTransform::IDENTITY),
        Some(node) if node.node_type == crate::graph_nodes::TEXTURE_TRANSFORM => {
            let wired = |pin: &str| source_of(&node.id, pin).is_some();
            let from_mesh_uv = source_of(&node.id, "uv").is_some_and(is_mesh_uv);
            (from_mesh_uv && !wired("offset") && !wired("scale") && !wired("rotation"))
                .then(|| crate::graph_nodes::texture_transform_of(node))
                .flatten()
        }
        Some(_) => None,
    };
    let texture = sample
        .properties
        .get("texture")
        .and_then(serde_json::Value::as_str)
        .filter(|asset| !asset.trim().is_empty());
    match (texture, transform) {
        (Some(texture), Some(transform)) => GraphMask::TextureAlpha {
            texture: texture.to_string(),
            transform,
        },
        _ => GraphMask::Other,
    }
}

/// The constant `opacity` a graph authors on its output (a default on the
/// unconnected pin, number or text), `None` when the pin is connected (the
/// value is per pixel, which the transparent drawer cannot evaluate) or
/// unset. Below 1 the material is translucent: its row takes the blended,
/// transparent-only path with this alpha ([`crate::assets`]).
pub fn constant_opacity(graph: &psgc::GraphDescription) -> Option<f32> {
    let output = graph.nodes.values().find(|n| n.node_type == "fragment_output")?;
    if graph
        .connections
        .iter()
        .any(|c| c.target_node == output.id && c.target_pin == "opacity")
    {
        return None;
    }
    let value = output.properties.get("opacity")?;
    let opacity = value
        .as_f64()
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))?;
    Some((opacity as f32).clamp(0.0, 1.0))
}

/// The image file a graph's texture asset names: a texture reference
/// (`TextureRef`, the asset's id) through the project's file index, else a
/// path, relative to `project_root`. The one place a graph's textures
/// resolve, for the engine's materials and the editors' previews alike.
pub fn texture_path(asset: &str, project_root: Option<&std::path::Path>) -> std::path::PathBuf {
    let Some(root) = project_root else {
        return std::path::PathBuf::from(asset);
    };
    if let Some(reference) = pulsar_reflection::TextureRef::parse(asset) {
        if let Some(path) = engine_fs::open_project_file_index(root)
            .ok()
            .flatten()
            .and_then(|index| {
                index
                    .path_for_asset_id_kind("texture", reference.as_str())
                    .ok()
                    .flatten()
            })
        {
            return if path.is_relative() {
                root.join(path)
            } else {
                path
            };
        }
    }
    let path = std::path::Path::new(asset);
    if path.is_relative() {
        root.join(path)
    } else {
        path.to_path_buf()
    }
}

fn is_texture_sample(kind: &str) -> bool {
    matches!(
        kind,
        "sample_texture" | "sample_texture_level" | "sample_texture_grad"
    )
}

/// How each texture the graph samples is used, by the asset path it names:
/// what the output pin it ends up feeding expects.
///
/// A texture feeding `base_color` or `emissive_color` is a colour; one feeding
/// `normal` is a normal map; one feeding `roughness`, `metallic`,
/// `ambient_occlusion`, `opacity` or `opacity_mask` is data, numbers that must
/// not be gamma-decoded. A texture that reaches several pins takes the
/// strongest use ([`helio_component::cooked_texture::TextureUse::strongest`]: colour
/// wins, which is how every texture was treated before uses existed). Only
/// samples the output depends on are listed, as in [`texture_assets`].
pub fn texture_uses(
    graph: &psgc::GraphDescription,
) -> std::collections::HashMap<String, helio_component::cooked_texture::TextureUse> {
    use helio_component::cooked_texture::TextureUse;

    fn pin_use(pin: &str) -> TextureUse {
        match pin {
            "normal" => TextureUse::Normal,
            "metallic" | "roughness" | "ambient_occlusion" | "opacity" | "opacity_mask" => {
                TextureUse::Data
            }
            // base_color, emissive_color, and anything unknown.
            _ => TextureUse::Color,
        }
    }

    let mut uses: std::collections::HashMap<String, TextureUse> = Default::default();
    for output in graph
        .nodes
        .values()
        .filter(|n| n.node_type == "fragment_output")
    {
        for feed in graph
            .connections
            .iter()
            .filter(|c| c.target_node == output.id)
        {
            let usage = pin_use(&feed.target_pin);
            let mut seen = std::collections::HashSet::new();
            let mut pending = vec![feed.source_node.clone()];
            while let Some(id) = pending.pop() {
                if !seen.insert(id.clone()) {
                    continue;
                }
                if let Some(node) = graph.nodes.get(&id) {
                    if is_texture_sample(&node.node_type) {
                        if let Some(asset) = node
                            .properties
                            .get("texture")
                            .and_then(serde_json::Value::as_str)
                            .filter(|a| !a.trim().is_empty())
                        {
                            uses.entry(asset.to_string())
                                .and_modify(|existing| *existing = existing.strongest(usage))
                                .or_insert(usage);
                        }
                    }
                }
                pending.extend(
                    graph
                        .connections
                        .iter()
                        .filter(|c| c.target_node == id)
                        .map(|c| c.source_node.clone()),
                );
            }
        }
    }
    uses
}

/// A copy of `graph` with its input defaults in the shape PSGC reads: numbers
/// and booleans typed as text (what the graph editor's fields and older files
/// store) become numbers and booleans, and a blank default of an input pin is
/// dropped so the pin falls back to the shader type's default (serialized as
/// `""` it would reach the WGSL as a string literal), and an input a node
/// declares a default for ([`crate::graph_nodes::input_defaults`], a texture
/// transform's unit scale) takes it where the graph stores none. The one
/// normalization every compile of a graph goes through.
pub fn prepare(graph: &psgc::GraphDescription) -> psgc::GraphDescription {
    let mut prepared = graph.clone();
    for node in prepared.nodes.values_mut() {
        for (pin, value) in crate::graph_nodes::input_defaults(&node.node_type) {
            let stored = node
                .properties
                .get(*pin)
                .is_some_and(|stored| match stored {
                    serde_json::Value::Null => false,
                    serde_json::Value::String(text) => !text.trim().is_empty(),
                    _ => true,
                });
            if !stored {
                node.properties
                    .insert((*pin).to_string(), crate::graph_nodes::default_value(value));
            }
        }
        let inputs: Vec<String> = node.inputs.iter().map(|pin| pin.id.clone()).collect();
        let keys: Vec<String> = node.properties.keys().cloned().collect();
        for key in keys {
            let Some(serde_json::Value::String(text)) = node.properties.get(&key) else {
                continue;
            };
            match property_to_psgc_value(text, inputs.contains(&key)) {
                Some(value) => {
                    node.properties.insert(key, value);
                }
                None => {
                    node.properties.remove(&key);
                }
            }
        }
    }
    prepared
}

fn property_to_psgc_value(value: &str, is_input_property: bool) -> Option<serde_json::Value> {
    let value = value.trim();
    if is_input_property && value.is_empty() {
        return None;
    }
    Some(if let Ok(number) = value.parse::<f64>() {
        serde_json::json!(number)
    } else if let Some(vector) = number_array(value) {
        vector
    } else if let Ok(boolean) = value.parse::<bool>() {
        serde_json::json!(boolean)
    } else {
        serde_json::json!(value)
    })
}

/// A vector default typed as text, `[x, y]`: the array of numbers PSGC
/// lowers to a WGSL vector.
fn number_array(value: &str) -> Option<serde_json::Value> {
    if !value.starts_with('[') {
        return None;
    }
    let parsed: serde_json::Value = serde_json::from_str(value).ok()?;
    parsed
        .as_array()?
        .iter()
        .all(serde_json::Value::is_number)
        .then_some(parsed)
}

/// Bind graph textures through `GpuMaterial` fields before code generation.
/// The map values are graph-local texture ordinals, not scene texture slots,
/// so the compiled source and its pipeline key stay independent of residency.
pub fn compile_material_graph(
    graph: &psgc::GraphDescription,
    texture_bindings: &std::collections::HashMap<String, u32>,
) -> Result<String, String> {
    let assets = texture_assets(graph)?;
    if assets.len() > GRAPH_TEXTURE_FIELDS.len() {
        return Err(format!(
            "material graphs support at most {} sampled textures",
            GRAPH_TEXTURE_FIELDS.len()
        ));
    }
    for asset in &assets {
        if !texture_bindings.contains_key(asset) {
            return Err(format!(
                "Texture asset '{asset}' has no material texture binding"
            ));
        }
    }
    let mut graph = prepare(graph);
    for node in graph.nodes.values_mut() {
        if !is_texture_sample(&node.node_type) {
            continue;
        }
        if let Some(asset) = node
            .properties
            .get("texture")
            .and_then(serde_json::Value::as_str)
        {
            if let Some(ordinal) = texture_bindings.get(asset) {
                let Some(field) = GRAPH_TEXTURE_FIELDS.get(*ordinal as usize) else {
                    return Err(format!(
                        "texture asset '{asset}' has unsupported graph-local texture ordinal {ordinal}"
                    ));
                };
                node.properties.insert(
                    "texture".into(),
                    serde_json::Value::String(format!("scene_textures[material.{field}]")),
                );
            }
        }
    }
    let generated = psgc::compile_shader(&graph)
        .map_err(|error| format!("shader graph compile failed: {error}"))?;
    adapt_graph_wgsl(&generated)
}

/// The material row's existing texture fields provide scene-local indirection
/// for a graph's sampled textures.
pub const GRAPH_TEXTURE_FIELDS: [&str; 5] = [
    "tex_base_color",
    "tex_normal",
    "tex_roughness",
    "tex_emissive",
    "tex_occlusion",
];

/// UV0's gradients in the graph function, which a texture transform chain's
/// gradients start from ([`helio_mats::shader::uv_transform_gradients`]).
const GRAPH_DUV: [&str; 2] = ["material_graph_duv_dx", "material_graph_duv_dy"];

fn adapt_graph_wgsl(generated: &str) -> Result<String, String> {
    // A sample through texture transforms reads at the transforms' chained
    // gradients, so a stage without derivatives reads it at its own mip.
    let mut source = helio_mats::shader::uv_transform_gradients(generated, "uv", GRAPH_DUV);
    let chains_gradients = source.contains(GRAPH_DUV[0]);
    bind_graph_texture_samplers(&mut source);
    if let Some(start) = source.find("struct Uniforms {") {
        let end = source[start..]
            .find("};")
            .map(|offset| start + offset + 2)
            .ok_or_else(|| "malformed PSGC Uniforms declaration".to_string())?;
        source.replace_range(start..end, "");
    }
    source = source.replace("@group(0) @binding(0) var<uniform> uniforms: Uniforms;", "");
    // Time comes from the host: Radiant defines `radiant_graph_time()` per template.
    source = source.replace("uniforms.time", "radiant_graph_time()");
    source = source.replace("FragmentOutput", "PulsarGraphOutput");
    for location in 0..8 {
        source = source.replace(&format!("@location({location}) "), "");
    }

    let entry = source
        .find("@fragment\nfn fragment_main(")
        .or_else(|| source.find("@fragment\r\nfn fragment_main("))
        .ok_or_else(|| "PSGC output is not a fragment shader".to_string())?;
    let open = source[entry..]
        .find('{')
        .map(|offset| entry + offset)
        .ok_or_else(|| "malformed PSGC fragment entry point".to_string())?;
    let mut replacement = String::from(
        "fn pulsar_material_graph(material: GpuMaterial, input: MaterialInput) -> PulsarGraphOutput {\n    let frag_coord = input.clip_position;\n    let uv = input.tex_coords;\n    let normal = input.world_normal;\n    let world_pos = input.world_position;",
    );
    if chains_gradients {
        // `explicit_gradients` reads these as `material_duv_dx` / `_dy`.
        replacement.push_str(&format!(
            "\n    let {} = dpdx(input.tex_coords);\n    let {} = dpdy(input.tex_coords);",
            GRAPH_DUV[0], GRAPH_DUV[1]
        ));
    }
    source = source.replace(
        "pulsar_material_graph(input)",
        "pulsar_material_graph(material, input)",
    );
    source.replace_range(entry..=open, &replacement);

    let body = r#"let graph_surface = pulsar_material_graph(material, input);
albedo = graph_surface.base_color;
// Preserve the connected RGBA color's alpha. The separate opacity input
// modulates coverage and defaults to 1 when it is not connected.
alpha = clamp(graph_surface.base_color.a, 0.0, 1.0) * clamp(graph_surface.opacity, 0.0, 1.0);
// A masked graph (`opacity_mask` connected, the row's FLAG_ALPHA_TEST) covers
// by its mask, the value the depth and shadow passes cut out with.
if (material.flags & MATERIAL_FLAG_ALPHA_TEST) != 0u { alpha = clamp(graph_surface.opacity_mask, 0.0, 1.0); }
albedo.a = alpha;
roughness = clamp(graph_surface.roughness, 0.045, 1.0);
metallic = clamp(graph_surface.metallic, 0.0, 1.0);
ao = clamp(graph_surface.ambient_occlusion, 0.0, 1.0);
emissive = graph_surface.emissive_color.rgb * graph_surface.emissive_color.a;
let graph_normal_length = length(graph_surface.normal);
if graph_normal_length > 0.0001 { N = graph_surface.normal / graph_normal_length; }
specular_f0 = clamp(mix(vec3<f32>(0.04), albedo.rgb, metallic), vec3<f32>(0.0), vec3<f32>(0.999));"#;
    Ok(format!(
        "/*RADIANT_GRAPH_DECLARATIONS*/\n{source}\n/*RADIANT_GRAPH_BODY*/\n{body}"
    ))
}

/// Reads every graph texture sample of the scene texture table through the
/// table's one sampler (`scene_sampler`), in place of the sampler PSGC names.
fn bind_graph_texture_samplers(source: &mut String) {
    for function in [
        "textureSample(",
        "textureSampleLevel(",
        "textureSampleGrad(",
    ] {
        let mut search_from = 0;
        while let Some(relative) = source[search_from..].find(function) {
            let call_start = search_from + relative;
            let args_start = call_start + function.len();
            let Some(first_comma_rel) = source[args_start..].find(',') else {
                break;
            };
            let first_comma = args_start + first_comma_rel;
            let first_arg = source[args_start..first_comma]
                .trim()
                .trim_matches(['(', ')'])
                .trim();
            if !first_arg.starts_with("scene_textures[") {
                search_from = first_comma + 1;
                continue;
            }
            let Some(second_comma_rel) = source[first_comma + 1..].find(',') else {
                break;
            };
            let second_comma = first_comma + 1 + second_comma_rel;
            source.replace_range(first_comma + 1..second_comma, " scene_sampler");
            search_from = second_comma + 1;
        }
    }
}

#[cfg(test)]
mod property_conversion_tests {
    use super::property_to_psgc_value;

    #[test]
    fn blank_input_property_uses_shader_default() {
        assert_eq!(property_to_psgc_value("", true), None);
        assert_eq!(property_to_psgc_value("   ", true), None);
    }

    #[test]
    fn blank_non_input_property_is_preserved() {
        assert_eq!(
            property_to_psgc_value("", false),
            Some(serde_json::Value::String(String::new()))
        );
    }

    #[test]
    fn scalar_input_properties_keep_their_types() {
        assert_eq!(
            property_to_psgc_value("0.5", true),
            Some(serde_json::json!(0.5))
        );
        assert_eq!(
            property_to_psgc_value("true", true),
            Some(serde_json::json!(true))
        );
    }
}

#[cfg(test)]
mod usage_tests {
    use super::*;
    use helio_component::cooked_texture::TextureUse;
    use psgc::{Connection, ConnectionType, GraphDescription, NodeInstance, Position};

    fn node(graph: &mut GraphDescription, id: &str, kind: &str, texture: Option<&str>) {
        let mut node = NodeInstance::new(id, kind, Position { x: 0.0, y: 0.0 });
        if let Some(texture) = texture {
            node.properties
                .insert("texture".into(), serde_json::json!(texture));
        }
        graph.add_node(node);
    }

    fn connect(graph: &mut GraphDescription, from: &str, to: &str, pin: &str) {
        graph.add_connection(Connection::new(
            from,
            "result",
            to,
            pin,
            ConnectionType::Data,
        ));
    }

    #[test]
    fn each_texture_takes_the_use_of_the_pin_it_feeds_through_any_chain() {
        let mut graph = GraphDescription::new("uses");
        node(&mut graph, "out", "fragment_output", None);
        // base colour straight from a sample.
        node(&mut graph, "albedo", "sample_texture", Some("albedo.png"));
        connect(&mut graph, "albedo", "out", "base_color");
        // roughness through a channel pick.
        node(&mut graph, "orm", "sample_texture", Some("orm.png"));
        node(&mut graph, "green", "component_g", None);
        connect(&mut graph, "orm", "green", "vec");
        connect(&mut graph, "green", "out", "roughness");
        // normal.
        node(&mut graph, "nrm", "sample_texture", Some("normal.png"));
        connect(&mut graph, "nrm", "out", "normal");
        // a sample the output does not depend on is not listed.
        node(&mut graph, "unused", "sample_texture", Some("unused.png"));

        let uses = texture_uses(&graph);
        assert_eq!(uses["albedo.png"], TextureUse::Color);
        assert_eq!(uses["orm.png"], TextureUse::Data);
        assert_eq!(uses["normal.png"], TextureUse::Normal);
        assert!(!uses.contains_key("unused.png"));
    }

    #[test]
    fn a_texture_feeding_colour_and_data_stays_colour() {
        let mut graph = GraphDescription::new("shared");
        node(&mut graph, "out", "fragment_output", None);
        node(&mut graph, "tex", "sample_texture", Some("both.png"));
        node(&mut graph, "r", "component_r", None);
        connect(&mut graph, "tex", "out", "base_color");
        connect(&mut graph, "tex", "r", "vec");
        connect(&mut graph, "r", "out", "ambient_occlusion");
        assert_eq!(texture_uses(&graph)["both.png"], TextureUse::Color);
    }

    #[test]
    fn a_graph_with_no_samples_has_no_uses() {
        let mut graph = GraphDescription::new("empty");
        node(&mut graph, "out", "fragment_output", None);
        assert!(texture_uses(&graph).is_empty());
    }

    /// A foliage graph: base colour and a normal map, cut out by the base
    /// colour's alpha at the mesh UV. `z_leaf.png` sorts after the normal
    /// map, so only the mask rule can put it first.
    fn foliage(uv_through: Option<&str>) -> GraphDescription {
        let mut graph = GraphDescription::new("foliage");
        node(&mut graph, "out", "fragment_output", None);
        node(&mut graph, "uv", "frag_uv", None);
        node(&mut graph, "leaf", "sample_texture", Some("z_leaf.png"));
        node(&mut graph, "nrm", "sample_texture", Some("a_normal.png"));
        node(&mut graph, "alpha", "component_a", None);
        match uv_through {
            Some(kind) => {
                node(&mut graph, "warp", kind, None);
                let pin = if kind == crate::graph_nodes::TEXTURE_TRANSFORM { "uv" } else { "a" };
                connect(&mut graph, "uv", "warp", pin);
                connect(&mut graph, "warp", "leaf", "uv");
            }
            None => connect(&mut graph, "uv", "leaf", "uv"),
        }
        connect(&mut graph, "leaf", "out", "base_color");
        connect(&mut graph, "nrm", "out", "normal");
        connect(&mut graph, "leaf", "alpha", "vec");
        connect(&mut graph, "alpha", "out", "opacity_mask");
        graph
    }

    #[test]
    fn a_texture_alpha_mask_is_ordinal_zero() {
        let graph = foliage(None);
        assert_eq!(
            opacity_mask(&graph),
            GraphMask::TextureAlpha {
                texture: "z_leaf.png".into(),
                transform: helio_mats::TextureTransform::IDENTITY,
            }
        );
        assert_eq!(
            texture_assets(&graph).unwrap(),
            vec!["z_leaf.png".to_string(), "a_normal.png".to_string()],
            "the mask texture is the row's base colour slot, which the depth passes cut out with"
        );
    }

    /// A mask read through a texture transform of constant inputs is still
    /// the texture's alpha, with the transform the cut-out passes sample
    /// through; an input left unset takes the node's identity default.
    #[test]
    fn a_mask_through_a_constant_texture_transform_carries_the_transform() {
        let mut graph = foliage(Some(crate::graph_nodes::TEXTURE_TRANSFORM));
        let warp = graph.nodes.get_mut("warp").unwrap();
        warp.properties
            .insert("offset".into(), serde_json::json!([0.0, -2.4]));
        warp.properties
            .insert("scale".into(), serde_json::json!("[15, 3.4]"));
        assert_eq!(
            opacity_mask(&graph),
            GraphMask::TextureAlpha {
                texture: "z_leaf.png".into(),
                transform: helio_mats::TextureTransform {
                    offset: [0.0, -2.4],
                    scale: [15.0, 3.4],
                    rotation_radians: 0.0,
                },
            }
        );
        assert_eq!(texture_assets(&graph).unwrap()[0], "z_leaf.png");

        // A wired input is per pixel: the cut-out passes cannot evaluate it.
        node(&mut graph, "spin", "time", None);
        connect(&mut graph, "spin", "warp", "rotation");
        assert_eq!(opacity_mask(&graph), GraphMask::Other);
    }

    #[test]
    fn a_mask_sampled_off_the_mesh_uv_is_not_a_texture_alpha() {
        let graph = foliage(Some("multiply"));
        assert_eq!(opacity_mask(&graph), GraphMask::Other);
        assert_eq!(
            texture_assets(&graph).unwrap(),
            vec!["a_normal.png".to_string(), "z_leaf.png".to_string()]
        );
    }

    #[test]
    fn a_graph_without_a_mask_is_not_masked() {
        let mut graph = GraphDescription::new("opaque");
        node(&mut graph, "out", "fragment_output", None);
        node(&mut graph, "tex", "sample_texture", Some("albedo.png"));
        connect(&mut graph, "tex", "out", "base_color");
        assert_eq!(opacity_mask(&graph), GraphMask::None);
    }
}

#[cfg(test)]
mod opacity_tests {
    use super::*;
    use psgc::{Connection, ConnectionType, GraphDescription, NodeInstance, Position};

    fn graph_with_output(opacity: Option<serde_json::Value>) -> GraphDescription {
        let mut graph = GraphDescription::new("opacity");
        let mut output = NodeInstance::new("out", "fragment_output", Position { x: 0.0, y: 0.0 });
        if let Some(value) = opacity {
            output.properties.insert("opacity".into(), value);
        }
        graph.add_node(output);
        graph
    }

    #[test]
    fn a_constant_opacity_is_read_as_a_number_or_text_and_clamped() {
        assert_eq!(constant_opacity(&graph_with_output(None)), None);
        assert_eq!(
            constant_opacity(&graph_with_output(Some(serde_json::json!(0.25)))),
            Some(0.25)
        );
        assert_eq!(
            constant_opacity(&graph_with_output(Some(serde_json::json!(" 0.5 ")))),
            Some(0.5)
        );
        assert_eq!(
            constant_opacity(&graph_with_output(Some(serde_json::json!(3.0)))),
            Some(1.0)
        );
    }

    #[test]
    fn a_connected_opacity_has_no_constant() {
        let mut graph = graph_with_output(Some(serde_json::json!(0.25)));
        graph.add_node(NodeInstance::new("src", "frag_uv", Position { x: 0.0, y: 0.0 }));
        graph.add_connection(Connection::new("src", "result", "out", "opacity", ConnectionType::Data));
        assert_eq!(constant_opacity(&graph), None);
    }
}
