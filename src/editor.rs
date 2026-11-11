//! What the materials plugin registers for the editor (the `editor`
//! feature): the material graph editor.

use materials_editor::MaterialGraphEditorProvider;

/// Everything the plugin registers for the editor.
pub(crate) fn build(p: &mut pulsar_plugins::PluginBuilder) {
    // ── Asset editors ───────────────────────────────────────────────────────

    // `.material` assets open in the material graph editor, which also offers the
    // graph's AI tools and the import companion.
    p.add(plugin_manager::PluginEditorProvider {
        origin: p.origin(),
        id: MaterialGraphEditorProvider::ID,
        create: MaterialGraphEditorProvider::create,
    });
}
