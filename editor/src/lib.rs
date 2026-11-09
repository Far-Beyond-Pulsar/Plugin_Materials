//! Materials (`pulsar.materials`): the editor crate -- the material graph
//! editor for `.material` assets (`materials_graph_editor`) as a linked
//! editor provider ([`MaterialGraphEditorProvider`]). It registers nothing
//! itself: the plugin's root crate (`materials_plugin`, with its `editor`
//! feature) submits the provider to the editor.

use plugin_editor_api::*;

/// The material graph editor as a linked editor provider: opens `.material`
/// assets, offers the graph's AI tools, and does the graph editor's startup
/// work (node definitions, thumbnails, the import companion) when the host
/// registers it.
pub struct MaterialGraphEditorProvider;

impl MaterialGraphEditorProvider {
    /// The provider's id (its [`plugin_manager::BuiltinEditorProvider::provider_id`]):
    /// the plugin's.
    pub const ID: &'static str = "pulsar.materials";

    /// The provider, after the graph editor's one-time startup work: what the
    /// root's `plugin_manager::PluginEditorProvider` creates.
    pub fn create() -> std::sync::Arc<dyn plugin_manager::BuiltinEditorProvider> {
        static STARTED: std::sync::Once = std::sync::Once::new();
        STARTED.call_once(|| {
            materials_graph_editor::initialize();
            // Plugins tell the import pipeline what extra work they can do
            // for an import; the pipeline knows nothing about materials.
            asset_import::register_companion(materials_graph_editor::material_import_companion());
        });
        std::sync::Arc::new(MaterialGraphEditorProvider)
    }
}

impl plugin_manager::BuiltinEditorProvider for MaterialGraphEditorProvider {
    fn provider_id(&self) -> &str {
        Self::ID
    }

    fn file_types(&self) -> Vec<FileTypeDefinition> {
        materials_graph_editor::file_types()
    }

    fn editors(&self) -> Vec<EditorMetadata> {
        materials_graph_editor::editors()
    }

    fn can_handle(&self, editor_id: &EditorId) -> bool {
        editor_id.as_str() == materials_graph_editor::EDITOR_ID
    }

    fn ai_tools(&self) -> Vec<AiToolDefinition> {
        materials_graph_editor::ai_tools()
    }

    fn capabilities_for_file(&self, file_path: &std::path::Path) -> Vec<String> {
        materials_graph_editor::capabilities_for_file(file_path)
    }

    fn execute_ai_tool(
        &self,
        file_path: &std::path::Path,
        tool_name: &str,
        tool_args: serde_json::Value,
    ) -> Result<serde_json::Value, PluginError> {
        materials_graph_editor::execute_compiled_tool(file_path, tool_name, tool_args)
    }

    fn create_editor(
        &self,
        file_path: std::path::PathBuf,
        editor_context: &plugin_manager::EditorContext,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Result<std::sync::Arc<dyn ui::dock::PanelView>, PluginError> {
        use gpui::AppContext as _;
        use materials_graph_editor::ShaderEditorPanel;
        let panel = cx.new(|cx| {
            ShaderEditorPanel::new_with_path(file_path.clone(), window, cx).unwrap_or_else(|e| {
                tracing::error!("Failed to load material {:?}: {}", file_path, e);
                ShaderEditorPanel::new(window, cx)
            })
        });
        let undo = editor_context.services.undo.clone();
        panel.update(cx, |panel, cx| panel.set_undo_service(undo, cx));
        // Keep the AI tools aligned with the opened material's graph.
        let graph = panel.read(cx).graph_snapshot(cx);
        materials_graph_editor::upsert_ai_session(file_path, graph);
        Ok(std::sync::Arc::new(panel))
    }
}
