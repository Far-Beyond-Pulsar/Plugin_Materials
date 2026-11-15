// Transparent counterpart of the standard material surface. Texture and
// scalar opacity are evaluated through the shared material contract; the
// drawer adds direct lighting and accumulates this in its OIT targets.
fn radiant_eval_transparent(material: GpuMaterial, material_tex: MaterialTextureData, input: MaterialInput, ambient: vec3<f32>) -> vec4<f32> {
    let pbr = default_pbr_surface(material, material_tex, input);
    return vec4<f32>(pbr.albedo.rgb * ambient + pbr.emissive, 1.0);
}
