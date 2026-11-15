// Glass transparent template. OIT handles layer compositing; this surface
// supplies a Fresnel reflection tint over the material's transmitted colour.
// Screen-space refraction is a separate optical effect and is not implied by
// alpha blending.
fn radiant_eval_transparent(material: GpuMaterial, material_tex: MaterialTextureData, input: MaterialInput, ambient: vec3<f32>) -> vec4<f32> {
    let pbr = default_pbr_surface(material, material_tex, input);
    let N = normalize(input.world_normal);
    let V = normalize(input.view_position - input.world_position);
    let fresnel = pow(1.0 - clamp(abs(dot(N, V)), 0.0, 1.0), 5.0);
    let transmitted = pbr.albedo.rgb * ambient + pbr.emissive;
    let reflected_tint = vec3<f32>(0.82, 0.91, 1.0) * (0.04 + 0.22 * fresnel);
    return vec4<f32>(mix(transmitted, reflected_tint, fresnel), 0.12 + 0.38 * fresnel);
}
