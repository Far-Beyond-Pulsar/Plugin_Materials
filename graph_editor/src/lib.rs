#![recursion_limit = "512"]

//! # Material graph editor (`pulsar.materials`)
//!
//! Visual shader graph editor for creating materials through node-based programming.
//! The graph is drawn, laid out, hit-tested and edited by the shared Graphy canvas
//! (`graphy-gpui`); this crate supplies what is specific to shaders.
//!
//! ## Architecture
//!
//! - **editor**: the editor panel (canvas, history, dock workspace, toolbar)
//! - **profile** / **types**: the Graphy profile: node catalogue, WGSL connection
//!   rules, node and pin look, preview rows
//! - **previews**: live texture previews filling those rows
//! - **definitions**: shader node definitions from PSGC
//! - **features**: compilation (PSGC), material preview, shader models, import
//! - **ui_components**: the Details panel
//! - **io**: the `.material` asset and its save/load

use gpui::*;
use plugin_editor_api::*;
use serde_json::json;
use std::path::PathBuf;

// Module declarations
mod actions;
mod ai_tools;
mod definitions;
mod editor;
mod features;
mod io;
mod previews;
mod profile;
mod theme;
mod types;
mod ui_components;

// Re-export main types for plugin API compatibility
pub use actions::*;
pub use definitions::*;
pub use editor::panel::ShaderEditorPanel;

pub fn upsert_ai_session(file_path: PathBuf, graph: graphy::GraphDescription) {
    ai_tools::upsert_session(file_path, graph);
}

pub fn execute_compiled_tool(
    file_path: &std::path::Path,
    tool_name: &str,
    tool_args: serde_json::Value,
) -> Result<serde_json::Value, PluginError> {
    ai_tools::execute_compiled_tool(file_path, tool_name, tool_args)
}

/// Register the renderer of the `.material` assets' thumbnails (part of
/// [`initialize`]).
pub fn register_material_thumbnail_renderer() {
    ui_common::thumbnails::register_thumbnail_renderer(
        "material",
        crate::features::preview::thumbnail::render_material_thumbnail,
    );
}

/// The import companion that offers to build materials from the images found
/// with an imported model. The materials plugin registers it with the import
/// pipeline at startup (the pipeline itself knows nothing about materials).
pub use features::import_companion::material_import_companion;

/// The material graph editor's id.
pub const EDITOR_ID: &str = "shader-editor";

/// The `.material` asset (a folder holding the graph).
pub fn file_types() -> Vec<FileTypeDefinition> {
    vec![FileTypeDefinition {
        id: FileTypeId::new("material"),
        extension: "material".to_string(),
        display_name: "Material".to_string(),
        icon: ui::IconName::Component,
        color: gpui::rgb(0x4CAF50).into(),
        structure: FileStructure::FolderBased {
            marker_file: "shader_graph_save.json".to_string(),
            template_structure: vec![],
        },
        default_content: json!({
            "format_version": 1,
            "main_graph": {
                "nodes": {},
                "connections": [],
                "metadata": {
                    "name": "MainMaterial",
                    "description": "",
                    "version": "1.0.0",
                    "created_at": "2024-01-01T00:00:00+00:00",
                    "modified_at": "2024-01-01T00:00:00+00:00"
                },
                "comments": []
            },
            "shader_metadata": {
                "shader_model": "Standard_unlit",
                "stage": "Fragment",
                "description": "",
                "category": "Uncategorized"
            }
        }),
        creation_directory: None,
        cook: material_graph_cook,

        categories: vec!["Shaders".to_string()],
    }]
}

/// Shader graph materials reference their sampled images by path or stable
/// texture id. Resolve ids through the project's asset index before returning
/// edges to the package dependency walker.
fn material_graph_dependencies(
    context: &plugin_editor_api::CookContext<'_>,
) -> Result<plugin_editor_api::file_types::CookOutput, String> {
    let cooked = plugin_editor_api::file_types::copy_folder_for_cook(context)?;
    let graph_path = context.source_path.join("shader_graph_save.json");
    let bytes = engine_fs::virtual_fs::read_file(&graph_path)
        .map_err(|error| format!("{}: {error}", graph_path.display()))?;
    let text = std::str::from_utf8(&bytes).map_err(|error| error.to_string())?;
    let start = text
        .find('{')
        .ok_or_else(|| format!("{} has no graph JSON object", graph_path.display()))?;
    let document: serde_json::Value = serde_json::from_str(&text[start..])
        .map_err(|error| format!("{}: {error}", graph_path.display()))?;
    let graph_value = document
        .get("main_graph")
        .ok_or_else(|| format!("{} has no main_graph", graph_path.display()))?;
    let graph: psgc::GraphDescription = serde_json::from_value(graph_value.clone())
        .map_err(|error| format!("{}: {error}", graph_path.display()))?;
    let dependencies = materials_component::graph::texture_assets(&graph).map(|textures| {
        textures
            .iter()
            .map(|texture| {
                materials_component::graph::texture_path(texture, Some(context.project_root))
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    })?;
    Ok(plugin_editor_api::file_types::CookOutput::new(
        cooked.files,
        dependencies,
    ))
}

fn material_graph_cook(
    context: &plugin_editor_api::CookContext<'_>,
) -> Result<plugin_editor_api::file_types::CookOutput, String> {
    material_graph_dependencies(context)
}

/// The material graph editor, for `.material` assets.
pub fn editors() -> Vec<EditorMetadata> {
    vec![EditorMetadata {
        id: EditorId::new(EDITOR_ID),
        display_name: "Material Graph Editor".into(),
        supported_file_types: vec![FileTypeId::new("material")],
    }]
}

/// The editor's AI tools.
pub fn ai_tools() -> Vec<AiToolDefinition> {
    ai_tools::ai_tools()
}

/// The AI capabilities a material file offers.
pub fn capabilities_for_file(file_path: &std::path::Path) -> Vec<String> {
    ai_tools::capabilities_for_file(file_path)
}

/// Startup work the editor does once (the host has no plugin `on_load` for
/// a linked editor): the node definitions, the material thumbnail renderer.
pub fn initialize() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        crate::features::initialize_features();
        register_material_thumbnail_renderer();
    });
}
