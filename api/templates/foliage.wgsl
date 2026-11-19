// Two-sided foliage (`MATERIAL_CLASS_FOLIAGE`): thin, translucent leaves.
// Light from behind the surface passes through it: its transmission share
// (`class_params.x`, `material_transmission`) of its colour
// (`material_transmitted`), held as the frame's two-sided transmission
// (`surface_with_transmission`: `SURFACE_FLAG_TWO_SIDED`, the G-buffer's
// subsurface target, a hit's `subsurface`). What traces the light through
// the surface (shadows, GI) reads the same share. A voxel terrain's leaves
// and a mesh's leaf cards shade the same.
fn radiant_eval_surface(material: GpuMaterial,
                        material_tex: MaterialTextureData,
                        input: MaterialInput) -> SurfaceData {
    var s = default_pbr_surface(material, material_tex, input);

    s = surface_with_transmission(s, material_transmitted(material, s.albedo.rgb));

    // RADIANT_OVERRIDE_SURFACE
    // RADIANT_OVERRIDE_END

    return s;
}
