/// Key of a drawer's variant of a material program: (template_id, graph_hash, feature_flag_mask)
#[derive(Hash, Eq, PartialEq, Clone, Copy, Debug)]
pub struct RadiantShaderKey {
    pub template_id: u32,
    pub graph_hash: u64,
    pub feature_flags: u32,
}

/// The WGSL of the drawer program `program` for a material variant:
/// `template` composed into it with `graph_wgsl` (empty: the template as it
/// is) and the program's own `snippets` (`RadiantTemplate::compose`), lowered
/// to baseline-WebGPU bindings where the device has no binding arrays.
///
/// The same variant always composes to the same text. A drawer compiles it
/// off the frame (`helio_core::compile::Variants`) and creates the shader
/// module there: runtime-generated WGSL has no source file to watch, so it
/// cannot go through `helio_core::shader::module`'s registered path.
pub fn compose_program(
    template: &super::template::RadiantTemplate,
    program: &str,
    graph_wgsl: &str,
    snippets: &[helio_core::shader::ShaderSnippet],
    material_binding: crate::material::MaterialBindingConfig,
) -> String {
    let source = template.compose(program, graph_wgsl, material_binding.max_textures, snippets);
    if material_binding.uses_binding_arrays() {
        source
    } else {
        super::template::RadiantTemplate::apply_webgpu_fixups(&source, material_binding.max_textures)
    }
}
