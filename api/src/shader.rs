//! Shared PBR/BRDF evaluation — `fresnel_schlick`, the distribution and geometry terms.
//!
//! Lives alongside [`crate::material`] and [`crate::radiant`], which already own the
//! CPU side of the material model and consume this text directly (manual
//! concatenation, not `helio-core`'s generic shader-snippet mechanism — see
//! `helio-core/src/shader/mod.rs`'s doc for why that mechanism only owns the
//! generic prelude and pass-declared snippets, never a specific domain's text).
//!
//! Registering a module here is the step that is easy to miss and gives no warning when
//! missed: the `//!use` marker is an ordinary WGSL comment, so an unregistered module
//! leaves the marker inert, the source passes through untouched, and the shader fails at
//! `create_shader_module` with "no definition in scope" for a function that plainly
//! exists. That is exactly how `deferred_lighting.wgsl` and `forward_lit.wgsl` came to be
//! uncompilable — which on this path takes out lighting entirely.
pub const PBR_EVAL: &str = include_str!("../shaders/pbr_eval.wgsl");

/// Marker opting a shader into [`PBR_EVAL`]. Must appear in the source.
pub const PBR_MARKER: &str = "//!use pbr_eval";

/// [`helio_core::shader::ShaderSnippet`] for [`PBR_EVAL`]. Real consumers
/// (gbuffer/forward-lit/deferred-light templates) currently splice
/// [`PBR_EVAL`] in by hand rather than through `helio_core::shader`'s
/// marker mechanism, but this is the snippet form for anything that wants
/// to go through `resolve_with`/`module_with` instead.
pub const PBR_EVAL_SNIPPET: helio_core::shader::ShaderSnippet =
    helio_core::wgsl_snippet!(PBR_MARKER, "../shaders/pbr_eval.wgsl");

/// Specular anti-aliasing (`normal_map_roughness`, `geometric_roughness`,
/// `specular_aa_roughness`): roughness widened by the normals a pixel
/// covers, read from normal maps whose mips keep their spread
/// ([`helio_core::texture_mips`]).
pub const SPECULAR_AA: &str = include_str!("../shaders/specular_aa.wgsl");

/// Marker opting a shader into [`SPECULAR_AA`]. Must appear in the source.
pub const SPECULAR_AA_MARKER: &str = "//!use specular_aa";

/// [`helio_core::shader::ShaderSnippet`] for [`SPECULAR_AA`]: every material
/// shader built from a Radiant template gets it
/// ([`crate::RadiantTemplate::build_shader_source`]).
pub const SPECULAR_AA_SNIPPET: helio_core::shader::ShaderSnippet =
    helio_core::wgsl_snippet!(SPECULAR_AA_MARKER, "../shaders/specular_aa.wgsl");

/// The material row in WGSL (`//!use material_row`): `GpuMaterial`, its
/// word layout for raw readers (`MATERIAL_WORDS`, `MATERIAL_FLAGS_WORD`,
/// `MATERIAL_CLASS_WORD`), `MATERIAL_NO_TEXTURE`, the `MATERIAL_FLAG_*`,
/// `MATERIAL_CLASS_*` and `MATERIAL_WORKFLOW_*` constants and
/// `material_stand_in()`, the grey material a missing row draws as. Every
/// kind reading the material table reads it through this.
pub const MATERIAL_ROW_SNIPPET: helio_core::shader::ShaderSnippet =
    helio_core::wgsl_snippet!("//!use material_row", "../shaders/material_row.wgsl");

/// Material evaluation in WGSL (`//!use material_surface`, with
/// `material_row`, `specular_aa` and the frame's `frame_surface`): the
/// texture metadata (`MaterialTextureSlot`/`MaterialTextureData`),
/// `MaterialInput` (the point a drawer hands a material); what it gets back
/// is the frame's `SurfaceData` (`helio_frame_api::gbuffer::SURFACE_SNIPPET`:
/// the surface a G-buffer texel and a ray's hit both hold);
/// `material_at` (a row, or the stand-in), `material_texture_data`,
/// `sample_texture` and `default_pbr_surface`. The reader declares the
/// table's bindings as `materials`, `material_textures`, `scene_textures`
/// and `scene_sampler` (see the file's header).
pub const MATERIAL_SURFACE_SNIPPET: helio_core::shader::ShaderSnippet = helio_core::wgsl_snippet!(
    "//!use material_surface",
    "../shaders/material_surface.wgsl"
);

/// The gradients a material reads its textures with in a stage without
/// implicit derivatives (`//!use material_gradients`, with
/// `material_surface`): `material_duv_dx` / `_dy`, `material_dn_dx` / `_dy`
/// and `material_slot_gradient`. A compute program that evaluates materials
/// opts in, sets them for each point and is lowered with
/// [`explicit_gradients`].
pub const MATERIAL_GRADIENTS_SNIPPET: helio_core::shader::ShaderSnippet = helio_core::wgsl_snippet!(
    "//!use material_gradients",
    "../shaders/material_gradients.wgsl"
);

/// Every material snippet, in the order they are prepended: what a
/// drawer's material program resolves (`RadiantTemplate::compose` adds
/// them itself). The frame's surface (`//!use frame_surface`) is the
/// struct a material returns; a program opts into it with the others.
pub const MATERIAL_SNIPPETS: [helio_core::shader::ShaderSnippet; 6] = [
    helio_frame_api::gbuffer::SURFACE_SNIPPET,
    MATERIAL_ROW_SNIPPET,
    SPECULAR_AA_SNIPPET,
    PBR_EVAL_SNIPPET,
    MATERIAL_SURFACE_SNIPPET,
    MATERIAL_GRADIENTS_SNIPPET,
];

/// A composed material program (`RadiantTemplate::compose`, and for a
/// device without binding arrays `apply_webgpu_material_bindings` first)
/// lowered for a stage without implicit derivatives: every texture read of
/// the evaluator (`sample_texture`) at the gradients the stage set
/// (`//!use material_gradients`, through each slot's UV transform), a
/// material graph's own reads at `material_duv_dx` / `_dy`, specular
/// anti-aliasing's normal derivatives as `material_dn_dx` / `_dy`, and any
/// other derivative a graph takes (a screen-space DDX node) as zero: a
/// compute stage has no neighbouring invocations of the same surface.
/// `max_textures` and `bindless` are the program's material binding
/// ([`crate::MaterialBindingConfig`]).
///
/// The one lowering every compute evaluator of materials uses (virtual
/// geometry's visibility-buffer shade: a pixel's analytic derivatives; the
/// meshes' hit surfaces: a ray cone's footprint), so a material reads its
/// textures alike on the raster, in a visibility buffer and at a hit.
pub fn explicit_gradients(src: &str, max_textures: usize, bindless: bool) -> String {
    const GRAD: &str = "uv, material_slot_gradient(slot, material_duv_dx), material_slot_gradient(slot, material_duv_dy)";
    let mut out = src
        .replace("dpdx(n)", "material_dn_dx")
        .replace("dpdy(n)", "material_dn_dy")
        // The expanded binding's graph helper (`pulsar_graph_sample`).
        .replace("let dx = dpdx(uv);", "let dx = material_duv_dx;")
        .replace("let dy = dpdy(uv);", "let dy = material_duv_dy;");
    if bindless {
        out = out.replace(
            "textureSample(scene_textures[slot.texture_index], scene_sampler, uv)",
            &format!(
                "textureSampleGrad(scene_textures[slot.texture_index], scene_sampler, {GRAD})"
            ),
        );
    } else {
        for index in 0..max_textures {
            out = out.replace(
                &format!("textureSampleLevel(scene_texture_{index}, scene_sampler, uv, 0.0)"),
                &format!("textureSampleGrad(scene_texture_{index}, scene_sampler, {GRAD})"),
            );
        }
    }
    // What is left reads with implicit derivatives: a graph's own samples.
    out = rewrite_calls(&out, "textureSample(", |args| {
        (args.len() == 3).then(|| {
            format!(
                "textureSampleGrad({}, {}, {}, material_duv_dx, material_duv_dy)",
                args[0], args[1], args[2]
            )
        })
    });
    for derivative in [
        "dpdx(",
        "dpdy(",
        "fwidth(",
        "dpdxCoarse(",
        "dpdyCoarse(",
        "dpdxFine(",
        "dpdyFine(",
        "fwidthCoarse(",
        "fwidthFine(",
    ] {
        out = rewrite_calls(&out, derivative, |args| {
            (args.len() == 1).then(|| format!("(({0}) - ({0}))", args[0]))
        });
    }
    out
}

/// `source` with every call `name(args)` (`name` ends in `(`, and is not
/// the tail of a longer identifier) replaced by what `rewrite` makes of its
/// arguments (kept as it is where `rewrite` returns `None`).
fn rewrite_calls(source: &str, name: &str, rewrite: impl Fn(&[&str]) -> Option<String>) -> String {
    let mut out = String::with_capacity(source.len());
    let mut cursor = 0;
    while let Some(offset) = source[cursor..].find(name) {
        let start = cursor + offset;
        let joined = source[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        let Some((end, args)) = (!joined)
            .then(|| call_arguments(source, start + name.len() - 1))
            .flatten()
        else {
            out.push_str(&source[cursor..start + name.len()]);
            cursor = start + name.len();
            continue;
        };
        let parts = split_wgsl_arguments(args);
        out.push_str(&source[cursor..start]);
        match rewrite(&parts) {
            Some(call) => out.push_str(&call),
            None => out.push_str(&source[start..end]),
        }
        cursor = end;
    }
    out.push_str(&source[cursor..]);
    out
}

/// Replace the texture table's binding array with one binding per texture.
///
/// Finds the `scene_textures` binding array's declaration and puts
/// `scene_texture_{i}` at its group and binding `+ i` in its place
/// ([`crate::MaterialBindingConfig::append_layout_entries`]' expanded
/// layout), and drops the native-only extension directive. The one
/// `scene_sampler` binding is the same in both layouts, so it stays.
fn expand_texture_table(src: &str, max_textures: usize) -> String {
    let mut source = String::with_capacity(src.len() + 64 * max_textures);
    for line in src.lines() {
        if line.trim() == "enable wgpu_binding_array;" {
            // wgpu-only extension; baseline WebGPU rejects the directive.
        } else if line.contains("scene_textures:") && line.contains("binding_array<texture_2d") {
            let attribute = |name: &str| -> u32 {
                let open = format!("@{name}(");
                line.find(&open)
                    .and_then(|start| {
                        let rest = &line[start + open.len()..];
                        rest[..rest.find(')')?].trim().parse().ok()
                    })
                    .unwrap_or_else(|| panic!("scene_textures declaration without @{name}: {line}"))
            };
            let (group, first) = (attribute("group"), attribute("binding"));
            for index in 0..max_textures {
                source.push_str(&format!(
                    "@group({group}) @binding({}) var scene_texture_{index}: texture_2d<f32>;\n",
                    first + index as u32,
                ));
            }
        } else {
            source.push_str(line);
            source.push('\n');
        }
    }
    source
}

/// A `switch` over the expanded table returning
/// `textureSampleLevel(scene_texture_{i}, scene_sampler, uv, 0.0)` for
/// `selector`, and `fallback` past it.
fn expanded_sample_switch(selector: &str, max_textures: usize, fallback: &str) -> String {
    let mut switch = format!("switch {selector} {{\n");
    for index in 0..max_textures {
        switch.push_str(&format!(
            "        case {index}u: {{ return textureSampleLevel(scene_texture_{index}, scene_sampler, uv, 0.0); }}\n",
        ));
    }
    switch.push_str(&format!(
        "        default: {{ return {fallback}; }}\n    }}"
    ));
    switch
}

/// Replace native material binding arrays with baseline-WebGPU bindings.
///
/// Browser WebGPU does not expose wgpu's `binding_array` WGSL extension. The
/// fixed bindings and switch preserve every material texture slot without
/// requiring a native-only feature. The native-only extension directive is
/// removed together with the binding-array declaration.
pub fn apply_webgpu_material_bindings(src: &str, max_textures: usize) -> String {
    let mut source = expand_texture_table(src, max_textures).replace(
        "return textureSample(scene_textures[slot.texture_index], scene_sampler, uv);",
        &expanded_sample_switch("slot.texture_index", max_textures, "fallback"),
    );
    // Blueprint graphs bind TextureSrc assets to fixed SceneDB texture slots
    // at compile time. Lower those constant indices to the same individual
    // bindings used by the WebGPU fallback path.
    for index in 0..max_textures {
        source = source.replace(
            &format!("scene_textures[{index}u]"),
            &format!("scene_texture_{index}"),
        );
    }
    lower_graph_texture_samples(&source, max_textures)
}

/// Graph WGSL indexes the scene texture table through `GpuMaterial` fields.
/// Baseline WebGPU replaces that table with fixed bindings, so lower each
/// graph sampling expression to a switch over those bindings.
fn lower_graph_texture_samples(source: &str, max_textures: usize) -> String {
    const GRAPH_FIELDS: [&str; 5] = [
        "tex_base_color",
        "tex_normal",
        "tex_roughness",
        "tex_emissive",
        "tex_occlusion",
    ];

    let mut rewritten = String::with_capacity(source.len());
    let mut helpers = std::collections::BTreeSet::new();
    let mut cursor = 0;
    while cursor < source.len() {
        let next = [
            "textureSample(",
            "textureSampleLevel(",
            "textureSampleGrad(",
        ]
        .into_iter()
        .filter_map(|name| {
            source[cursor..]
                .find(name)
                .map(|offset| (cursor + offset, name))
        })
        .min_by_key(|(offset, _)| *offset);
        let Some((start, name)) = next else {
            rewritten.push_str(&source[cursor..]);
            break;
        };
        let Some((end, args)) = call_arguments(source, start + name.len() - 1) else {
            rewritten.push_str(&source[cursor..]);
            break;
        };
        let parts = split_wgsl_arguments(args);
        let texture_field = parts
            .first()
            .map(|texture| strip_outer_parentheses(texture))
            .and_then(|texture| texture.strip_prefix("scene_textures[material."))
            .and_then(|field| field.strip_suffix(']'));
        let reads_scene_sampler = parts
            .get(1)
            .is_some_and(|sampler| strip_outer_parentheses(sampler) == "scene_sampler");
        let variant = match name {
            "textureSample(" if parts.len() == 3 => Some(("pulsar_graph_sample", 1)),
            "textureSampleLevel(" if parts.len() == 4 => Some(("pulsar_graph_sample_level", 2)),
            "textureSampleGrad(" if parts.len() == 5 => Some(("pulsar_graph_sample_grad", 3)),
            _ => None,
        };
        if let (Some(texture_field), true, Some((helper, helper_index))) =
            (texture_field, reads_scene_sampler, variant)
        {
            if GRAPH_FIELDS.contains(&texture_field) && max_textures > 0 {
                rewritten.push_str(&source[cursor..start]);
                let mut call = format!("{helper}(material.{texture_field}, {}", parts[2]);
                for arg in parts.iter().skip(3) {
                    call.push_str(", ");
                    call.push_str(arg);
                }
                call.push(')');
                rewritten.push_str(&call);
                helpers.insert(helper_index);
            } else {
                rewritten.push_str(&source[cursor..end]);
            }
        } else {
            rewritten.push_str(&source[cursor..end]);
        }
        cursor = end;
    }

    if helpers.is_empty() {
        return rewritten;
    }
    let mut generated = String::new();
    for helper in helpers {
        generated.push_str(&graph_sample_helper(helper, max_textures));
    }
    if let Some(position) = rewritten.find("fn radiant_eval_surface") {
        rewritten.insert_str(position, &generated);
    } else {
        rewritten.push_str(&generated);
    }
    rewritten
}

fn call_arguments(source: &str, open: usize) -> Option<(usize, &str)> {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    for (offset, byte) in bytes.iter().enumerate().skip(open) {
        match *byte {
            b'(' => depth += 1,
            b')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some((offset + 1, &source[open + 1..offset]));
                }
            }
            _ => {}
        }
    }
    None
}

fn split_wgsl_arguments(args: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (index, byte) in args.bytes().enumerate() {
        match byte {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                parts.push(args[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(args[start..].trim());
    parts
}

fn strip_outer_parentheses(mut expression: &str) -> &str {
    expression = expression.trim();
    while expression.starts_with('(') && expression.ends_with(')') {
        let mut depth = 0usize;
        let closes_at_end = expression.bytes().enumerate().all(|(index, byte)| {
            match byte {
                b'(' => depth += 1,
                b')' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 && index + 1 < expression.len() {
                        return false;
                    }
                }
                _ => {}
            }
            true
        });
        if !closes_at_end {
            break;
        }
        expression = expression[1..expression.len() - 1].trim();
    }
    expression
}

fn graph_sample_helper(kind: usize, max_textures: usize) -> String {
    let (name, params, sample) = match kind {
        1 => (
            "pulsar_graph_sample",
            "slot: u32, uv: vec2<f32>",
            "textureSampleGrad(TEXTURE, SAMPLER, uv, dx, dy)",
        ),
        2 => (
            "pulsar_graph_sample_level",
            "slot: u32, uv: vec2<f32>, level: f32",
            "textureSampleLevel(TEXTURE, SAMPLER, uv, level)",
        ),
        _ => (
            "pulsar_graph_sample_grad",
            "slot: u32, uv: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>",
            "textureSampleGrad(TEXTURE, SAMPLER, uv, dx, dy)",
        ),
    };
    let mut helper = format!("fn {name}({params}) -> vec4<f32> {{\n");
    if kind == 1 {
        helper.push_str("    let dx = dpdx(uv);\n    let dy = dpdy(uv);\n");
    }
    helper.push_str("    switch slot {\n");
    for index in 0..max_textures {
        helper.push_str(&format!(
            "        case {index}u: {{ return {}; }}\n",
            sample
                .replace("TEXTURE", &format!("scene_texture_{index}"))
                .replace("SAMPLER", "scene_sampler")
        ));
    }
    helper.push_str("        default: { return vec4<f32>(1.0); }\n    }\n}\n");
    helper
}

/// Replace a kind's scene texture table with baseline-WebGPU bindings, for
/// a reader that samples it by a plain `u32` index.
///
/// Same idea as [`apply_webgpu_material_bindings`], for readers (the decal
/// collect compute shader, sprites) that index the table with a plain
/// `texture_index` rather than a material slot and sample with
/// `textureSampleLevel` (compute has no implicit derivatives).
pub fn apply_webgpu_decal_bindings(src: &str, max_textures: usize) -> String {
    expand_texture_table(src, max_textures).replace(
        "return textureSampleLevel(scene_textures[texture_index], scene_sampler, uv, 0.0);",
        &expanded_sample_switch("texture_index", max_textures, "vec4<f32>(1.0)"),
    )
}

#[cfg(test)]
mod tests {
    use super::{apply_webgpu_decal_bindings, apply_webgpu_material_bindings};

    #[test]
    fn expands_material_binding_arrays() {
        let source = r#"
enable wgpu_binding_array;
@group(1) @binding(2) var scene_sampler: sampler;
@group(1) @binding(3) var scene_textures: binding_array<texture_2d<f32>, 2>;
fn sample_texture(slot: MaterialTextureSlot, uv: vec2<f32>, fallback: vec4<f32>) -> vec4<f32> {
    return textureSample(scene_textures[slot.texture_index], scene_sampler, uv);
}
"#;
        let fixed = apply_webgpu_material_bindings(source, 2);

        assert!(!fixed.contains("binding_array"));
        assert!(!fixed.contains("enable wgpu_binding_array"));
        // The one sampler stays where it is; the textures follow it.
        assert!(fixed.contains("@group(1) @binding(2) var scene_sampler: sampler;"));
        assert!(fixed.contains("@group(1) @binding(3) var scene_texture_0: texture_2d<f32>;"));
        assert!(fixed.contains("@group(1) @binding(4) var scene_texture_1: texture_2d<f32>;"));
        assert!(fixed.contains(
            "case 1u: { return textureSampleLevel(scene_texture_1, scene_sampler, uv, 0.0); }"
        ));
        assert!(fixed.contains("default: { return fallback; }"));
    }

    #[test]
    fn lowers_graph_material_indices_for_expanded_bindings() {
        let source = r#"
@group(1) @binding(2) var scene_sampler: sampler;
@group(1) @binding(3) var scene_textures: binding_array<texture_2d<f32>, 2>;
fn graph(material: GpuMaterial, uv: vec2<f32>) -> vec4<f32> {
    let a = textureSample((scene_textures[material.tex_base_color]), scene_sampler, uv);
    let b = textureSampleLevel(scene_textures[material.tex_normal], scene_sampler, uv, 2.0);
    let c = textureSampleGrad(scene_textures[material.tex_roughness], scene_sampler, uv, vec2<f32>(1.0), vec2<f32>(1.0));
    return a + b + c;
}
"#;
        let fixed = apply_webgpu_material_bindings(source, 2);

        assert!(!fixed.contains("binding_array"));
        assert!(!fixed.contains("scene_textures[material."));
        assert!(fixed.contains("pulsar_graph_sample(material.tex_base_color, uv)"));
        assert!(fixed.contains("let dx = dpdx(uv);"));
        assert!(fixed.contains("textureSampleGrad(scene_texture_0, scene_sampler, uv, dx, dy)"));
        assert!(fixed.contains("pulsar_graph_sample_level(material.tex_normal, uv, 2.0)"));
        assert!(fixed.contains(
            "pulsar_graph_sample_grad(material.tex_roughness, uv, vec2<f32>(1.0), vec2<f32>(1.0))"
        ));
    }

    /// A compute stage's lowering: the evaluator reads at the stage's
    /// gradients through each slot's transform, a graph's own samples at
    /// the UV gradients, specular anti-aliasing's normal derivatives are the
    /// stage's, and any other derivative is zero; no implicit derivative is
    /// left, in either binding mode.
    #[test]
    fn explicit_gradients_leave_no_implicit_derivative() {
        use super::explicit_gradients;
        let source = r#"
fn sample_texture(slot: MaterialTextureSlot, uv: vec2<f32>, fallback: vec4<f32>) -> vec4<f32> {
    return textureSample(scene_textures[slot.texture_index], scene_sampler, uv);
}
fn aa(n: vec3<f32>) -> f32 { return length(dpdx(n)) + length(dpdy(n)); }
fn graph(material: GpuMaterial, uv: vec2<f32>) -> f32 {
    let a = textureSample(scene_textures[material.tex_base_color], scene_sampler, uv * 2.0);
    return a.x + fwidth(uv.x) + dpdxFine(uv.y);
}
"#;
        let bindless = explicit_gradients(source, 4, true);
        assert!(bindless.contains("textureSampleGrad(scene_textures[slot.texture_index], scene_sampler, uv, material_slot_gradient(slot, material_duv_dx), material_slot_gradient(slot, material_duv_dy))"));
        assert!(bindless.contains("length(material_dn_dx) + length(material_dn_dy)"));
        assert!(bindless.contains("textureSampleGrad(scene_textures[material.tex_base_color], scene_sampler, uv * 2.0, material_duv_dx, material_duv_dy)"));
        assert!(bindless.contains("((uv.x) - (uv.x))"));
        assert!(bindless.contains("((uv.y) - (uv.y))"));
        let expanded = explicit_gradients(&apply_webgpu_material_bindings(source, 2), 2, false);
        for text in [&bindless, &expanded] {
            for implicit in ["textureSample(", "dpdx", "dpdy", "fwidth"] {
                assert!(
                    !text.contains(implicit),
                    "{implicit} left:
{text}"
                );
            }
        }
        assert!(expanded.contains("textureSampleGrad(scene_texture_1, scene_sampler, uv, material_slot_gradient(slot, material_duv_dx), material_slot_gradient(slot, material_duv_dy))"));
    }

    #[test]
    fn expands_decal_binding_arrays() {
        let source = r#"
enable wgpu_binding_array;
@group(1) @binding(0) var scene_sampler: sampler;
@group(1) @binding(1) var scene_textures: binding_array<texture_2d<f32>, 256>;
fn sample_decal_texture(texture_index: u32, uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(scene_textures[texture_index], scene_sampler, uv, 0.0);
}
"#;
        let fixed = apply_webgpu_decal_bindings(source, 2);

        // The extension directive and the binding array must be gone —
        // baseline WebGPU rejects both.
        assert!(!fixed.contains("binding_array"));
        assert!(!fixed.contains("enable wgpu_binding_array"));
        // The one sampler stays; the textures follow it.
        assert!(fixed.contains("@group(1) @binding(0) var scene_sampler: sampler;"));
        assert!(fixed.contains("@group(1) @binding(1) var scene_texture_0"));
        assert!(fixed.contains("@group(1) @binding(2) var scene_texture_1"));
        assert!(!fixed.contains("scene_sampler_"));
        assert!(fixed.contains(
            "case 1u: { return textureSampleLevel(scene_texture_1, scene_sampler, uv, 0.0); }"
        ));
        assert!(fixed.contains("default: { return vec4<f32>(1.0); }"));
    }
}
