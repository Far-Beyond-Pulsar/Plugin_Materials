//! What the materials plugin registers for a game (and the editor): every
//! runtime contribution, grouped by the engine seam it plugs into.

use engine_backend::subsystems::render::RenderFeatureRegistration;
use helio_component::asset_component::{AssetLoaderRegistration, AssetReloadRegistration};
use helio_mats::asset::MATERIAL_ASSET_KIND;
use materials_component::assets;
use plugin_editor_api::AssetKind;

/// Everything the plugin registers for a game (and the editor).
pub(crate) fn build(p: &mut pulsar_plugins::PluginBuilder) {
    // ── Rendering ───────────────────────────────────────────────────────

    // The material texture table every kind's drawers bind, read with the
    // project's material sampler.
    p.add(RenderFeatureRegistration {
        origin: p.origin(),
        features: materials_component::render_features,
        configure: materials_component::configure_renderer,
    });

    // ── Assets ──────────────────────────────────────────────────────────

    // Material assets (surface documents and shader-graph folders), loaded on a
    // SceneDB worker thread.
    p.add(AssetLoaderRegistration {
        origin: p.origin(),
        kind: MATERIAL_ASSET_KIND,
        load: assets::load_material_asset,
        size: Some(assets::payload_size),
    });

    // The project watcher publishes an update for a changed image or material.
    p.add(engine_fs::AssetChangeKind {
        origin: p.origin(),
        id: "materials.files",
        kind_of: material_change,
    });

    // A changed material file (a document or a graph save) reloads its material.
    p.add(AssetReloadRegistration {
        origin: p.origin(),
        name: "materials.material_files",
        kind: AssetKind::Material,
        sources_for_file: assets::material_sources_for_file,
    });

    // A changed texture reloads every loaded graph that samples it.
    p.add(AssetReloadRegistration {
        origin: p.origin(),
        name: "materials.graph_textures",
        kind: AssetKind::Texture,
        sources_for_file: assets::graph_sources_sampling,
    });
}

/// An image or material file changed on disk is that asset's update (every
/// world showing it reloads it, SceneDB#84). Surface materials are saved as
/// `.mat` (mesh slots' extracted materials), which the file-type table does
/// not list, and a shader graph folder's save is its material's update.
pub fn material_change(path: &std::path::Path) -> Option<ui_types_common::AssetKind> {
    use ui_types_common::AssetKind;
    let extension = path.extension().and_then(|ext| ext.to_str());
    if path
        .file_name()
        .is_some_and(|name| name == "shader_graph_save.json")
        || extension.is_some_and(|ext| ext.eq_ignore_ascii_case("mat"))
    {
        return Some(AssetKind::Material);
    }
    match extension.map(AssetKind::from_extension) {
        Some(kind @ (AssetKind::Texture | AssetKind::Material)) => Some(kind),
        _ => None,
    }
}

#[cfg(test)]
mod change_tests {
    use super::material_change;
    use std::path::Path;
    use ui_types_common::AssetKind;

    #[test]
    fn images_and_material_files_are_updates_and_others_are_not() {
        assert_eq!(
            material_change(Path::new("a/rock.png")),
            Some(AssetKind::Texture)
        );
        assert_eq!(
            material_change(Path::new("a/skin.mat")),
            Some(AssetKind::Material)
        );
        assert_eq!(
            material_change(Path::new("a/shader_graph_save.json")),
            Some(AssetKind::Material)
        );
        assert_eq!(material_change(Path::new("a/notes.txt")), None);
    }
}

#[cfg(test)]
mod tests {
    use engine_backend::subsystems::render::helio_renderer::render_features;

    #[test]
    fn the_engine_runs_the_material_texture_table() {
        let features = render_features::plugin_features();
        assert!(
            features.ids().contains(&"pulsar.materials"),
            "the materials' feature is registered"
        );
    }

    /// Every registration of the plugin names this root, so it resolves to
    /// the plugin's id.
    #[test]
    fn every_registration_resolves_to_the_plugin() {
        let ours: Vec<_> = pulsar_plugins::contributions()
            .into_iter()
            .filter(|summary| summary.origin == pulsar_plugins::origin!())
            .collect();
        assert!(
            ours.iter()
                .any(|summary| summary.category == "asset.loader")
        );
        for summary in &ours {
            assert_eq!(
                pulsar_plugins::plugin_of(summary.origin).map(|plugin| plugin.id.as_str()),
                Some("pulsar.materials"),
                "{summary:?}"
            );
        }
    }
}
