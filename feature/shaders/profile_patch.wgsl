//! The shading-cost probe's test patch: the materials' surface
//! (`surface_probe`, the default PBR through `pbr_eval`) lit by `num_lights`
//! synthetic point lights, drawn once per profiled configuration and timed
//! by the pass (`cost.rs`).
//!use materials_surface_probe
//!use pbr_eval

struct PatchParams {
    roughness: f32,
    metallic: f32,
    num_lights: u32,
    _pad: u32,
}

@group(0) @binding(0) var<uniform> params: PatchParams;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

/// Fullscreen triangle.
@vertex
fn vs_main(@builtin(vertex_index) vertex_idx: u32) -> VertexOut {
    var out: VertexOut;
    let x = f32((vertex_idx << 1u) & 2u);
    let y = f32(vertex_idx & 2u);
    out.position = vec4<f32>(x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(x, 1.0 - y);
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let base_color = vec3<f32>(0.8, 0.8, 0.8);
    let normal = vec3<f32>(0.0, 0.0, 1.0);
    let view_dir = vec3<f32>(0.0, 0.0, 1.0);
    let world_pos = vec3<f32>(in.uv.x, in.uv.y, 0.0);

    var total_light = vec3<f32>(0.0);
    for (var i = 0u; i < params.num_lights; i++) {
        // A synthetic light: spread over a grid above the patch.
        let position = vec3<f32>(f32(i % 4u) - 1.5, f32((i / 4u) % 4u) - 1.5, 2.0);
        let to_light = position - world_pos;
        let dist = length(to_light);
        let attenuation = 1.0 / (dist * dist + 1.0);
        let brdf = surface_probe(
            normal,
            view_dir,
            to_light / dist,
            base_color,
            params.roughness,
            params.metallic,
        );
        total_light += brdf * 10.0 * attenuation;
    }
    return vec4<f32>(total_light, 1.0);
}
