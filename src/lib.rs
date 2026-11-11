//! Materials (`pulsar.materials`): the plugin's root crate, the one place
//! the plugin registers everything it contributes.
//!
//! The plugin's other crates hold the code: `helio-mats` (the contracts
//! every kind draws with: the material row and table, the texture table,
//! material instances), `helio-pass-materials` (the Helio feature that
//! builds the material texture table), `materials_component` (the material
//! assets and the material graph compiler), `materials_editor` (the material
//! graph editor's provider) and `materials_graph_editor` (the graph editor
//! itself). None of them registers a contribution; this crate imports them
//! and registers them
//! through its [`build`], so this file and the two modules below
//! are the whole list of what the plugin brings:
//!
//! - [`runtime`]: what a game links. The Helio feature and its sampler, the
//!   material asset loader and reloads, and the material instances' SceneDB
//!   attachments (handle ledger, GPU rows).
//! - `editor` (with the `editor` feature): what the editor adds. The
//!   material graph editor for `.material` assets.
//!
//! Every registration names this crate as its origin
//! (`p.origin()`), so the plugin gate skips all of them while
//! a project disables the plugin, and the plugin catalog lists them under
//! `pulsar.materials`. Other plugins never depend on this crate (only on
//! `helio-mats`); hosts reach it through the installed set or a generated
//! game, a game without the `editor` feature.

// The plugin's descriptor (`pulsar-plugin.toml`): linking this crate installs
// the plugin; a project enables it or not (`pulsar_plugins`).
pulsar_plugins::installed!("pulsar-plugin.toml", build);

/// Everything the plugin registers (runtime always, editor with the `editor`
/// feature).
pub fn build(p: &mut pulsar_plugins::PluginBuilder) {
    runtime::build(p);
    #[cfg(feature = "editor")]
    editor::build(p);
}

pub use materials_component;
#[cfg(feature = "editor")]
pub use materials_editor;

pub mod runtime;

#[cfg(feature = "editor")]
pub mod editor;
