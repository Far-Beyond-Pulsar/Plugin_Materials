// The materials' surface for the frame's shading-cost probe: the default
// PBR surface (`surface_data`) lit through the frame's BSDF (`bsdf_eval`,
// `//!use frame_bsdf`), the reflected radiance per unit light.
fn surface_probe(
    n: vec3<f32>,
    v: vec3<f32>,
    l: vec3<f32>,
    base_color: vec3<f32>,
    roughness: f32,
    metallic: f32,
) -> vec3<f32> {
    return bsdf_eval(surface_data(base_color, n, roughness, metallic, vec3<f32>(0.0)), v, l);
}
