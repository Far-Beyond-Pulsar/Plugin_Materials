// The gradients a material reads its textures with where a stage has no
// implicit derivatives (helio-mats `MATERIAL_GRADIENTS_SNIPPET`,
// `//!use material_gradients`, with `//!use material_surface`): a compute
// stage shading a visibility buffer's pixel or a ray's hit. Its program is
// lowered with `helio_mats::shader::explicit_gradients`, which turns every
// texture read of the evaluator and of a material graph into a read at
// these gradients and specular anti-aliasing's normal derivatives into
// `material_dn_dx` / `_dy`. The stage sets all four before it evaluates the
// material:
//   material_duv_dx, material_duv_dy  how far UV0 moves across the point's
//       footprint, along two axes (a pixel's neighbours; a ray cone's
//       ellipse on the surface)
//   material_dn_dx, material_dn_dy    how far the geometric normal turns
//       across the same footprint
// A texture's own UV transform is applied to them per slot
// (`material_slot_gradient`), so each texture is read at its own level.

var<private> material_duv_dx: vec2<f32>;
var<private> material_duv_dy: vec2<f32>;
var<private> material_dn_dx: vec3<f32>;
var<private> material_dn_dy: vec3<f32>;

// `d` (a UV0 gradient) through slot `slot`'s UV transform: its scale and
// rotation (`material_uv_transform_linear`; the offset drops out).
fn material_slot_gradient(slot: MaterialTextureSlot, d: vec2<f32>) -> vec2<f32> {
    return material_uv_transform_linear(slot.offset_scale, slot.rotation, d);
}
