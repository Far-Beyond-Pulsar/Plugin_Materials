//! What the materials plugin registers for the editor (the `editor`
//! feature): the material graph editor.

use materials_editor::MaterialGraphEditorProvider;

// ── Asset editors ───────────────────────────────────────────────────────────

// `.material` assets open in the material graph editor, which also offers the
// graph's AI tools and the import companion.
inventory::submit! {
    plugin_manager::PluginEditorProvider {
        origin: pulsar_plugins::origin!(),
        id: MaterialGraphEditorProvider::ID,
        create: MaterialGraphEditorProvider::create,
    }
}
