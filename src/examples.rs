//! The plugin's example materials: a read-only `examples/` folder in the
//! plugin's virtual folder (`plugins/<plugin>/examples/`), served from files
//! embedded in the binary (`examples_assets/`).
//!
//! - `.material` folders: shader-graph materials (`shader_graph_save.json`),
//!   as the material graph editor writes them, each with a `//` header and
//!   comment boxes in the graph explaining the nodes.
//! - `.mat` files: scalar surface documents (the material asset that is not a
//!   graph), as mesh slots use them.

use std::sync::Arc;

use engine_fs::{EmbeddedFile, FileProvider, FolderSource, InGame, MemoryFolder, PluginFolder};

macro_rules! example {
    ($name:literal) => {
        EmbeddedFile {
            path: concat!("examples/", $name),
            bytes: include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/examples_assets/",
                $name
            )),
        }
    };
}

/// Every embedded file, by its path inside the plugin's folder.
pub(crate) static EXAMPLES: &[EmbeddedFile] = &[
    example!("README.md"),
    example!("Checker.material/shader_graph_save.json"),
    example!("Marble.material/shader_graph_save.json"),
    example!("Lava.material/shader_graph_save.json"),
    example!("TexturedTint.material/shader_graph_save.json"),
    example!("TintedGlass.material/shader_graph_save.json"),
    example!("Gold.mat"),
    example!("Rubber.mat"),
    example!("NeonSign.mat"),
    example!("FrostedGlass.mat"),
];

fn open(folder: PluginFolder) -> Arc<dyn FolderSource> {
    Arc::new(MemoryFolder::with_files(folder, EXAMPLES))
}

/// Registers the plugin's example files.
pub(crate) fn build(p: &mut pulsar_plugins::PluginBuilder) {
    p.add(FileProvider {
        origin: p.origin(),
        open,
        in_game: InGame::Snapshot,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use helio_mats::asset::SurfaceMaterialDocument;
    use std::collections::HashMap;

    fn text(file: &EmbeddedFile) -> &'static str {
        std::str::from_utf8(file.bytes).expect("utf-8")
    }

    fn graphs() -> Vec<&'static EmbeddedFile> {
        EXAMPLES
            .iter()
            .filter(|f| f.path.ends_with("shader_graph_save.json"))
            .collect()
    }

    #[test]
    fn every_example_is_listed_in_the_readme() {
        let readme = EXAMPLES
            .iter()
            .find(|f| f.path == "examples/README.md")
            .map(text)
            .expect("README");
        assert!(graphs().len() >= 4, "several varied graph materials");
        for file in EXAMPLES.iter().filter(|f| !f.path.ends_with("README.md")) {
            let name = file
                .path
                .trim_start_matches("examples/")
                .trim_end_matches("/shader_graph_save.json");
            assert!(readme.contains(name), "README lists {name}");
        }
    }

    /// Reads a graph file as the material loader does (a `//` header before the
    /// JSON object) and compiles it with the material graph compiler.
    #[test]
    fn every_graph_material_loads_and_compiles() {
        for file in graphs() {
            let source = text(file);
            // The loader starts at the first `{`, so the header must not hold one.
            let header: &str = &source[..source.find('{').unwrap()];
            assert!(header.lines().all(|l| l.trim().is_empty() || l.starts_with("//")));
            let document: serde_json::Value = serde_json::from_str(&source[source.find('{').unwrap()..])
                .unwrap_or_else(|e| panic!("{}: {e}", file.path));
            assert_eq!(document["format_version"], 1, "{}", file.path);
            let graph: psgc::GraphDescription =
                serde_json::from_value(document["main_graph"].clone())
                    .unwrap_or_else(|e| panic!("{}: {e}", file.path));
            assert!(!graph.metadata.description.is_empty(), "{} is described", file.path);
            assert!(graph.nodes.contains_key("output"), "{}", file.path);
            let bindings: HashMap<String, u32> = materials_component::graph::texture_assets(&graph)
                .unwrap()
                .into_iter()
                .enumerate()
                .map(|(slot, asset)| (asset, slot as u32))
                .collect();
            materials_component::graph::compile_material_graph(&graph, &bindings)
                .unwrap_or_else(|e| panic!("{}: {e}", file.path));
            assert!(document["shader_metadata"]["description"].is_string());
        }
    }

    #[test]
    fn every_surface_material_is_a_valid_document() {
        let mats: Vec<_> = EXAMPLES.iter().filter(|f| f.path.ends_with(".mat")).collect();
        assert!(mats.len() >= 3, "several surface materials");
        for file in mats {
            let doc: SurfaceMaterialDocument = serde_json::from_slice(file.bytes)
                .unwrap_or_else(|e| panic!("{}: {e}", file.path));
            assert_eq!(doc.version, SurfaceMaterialDocument::VERSION, "{}", file.path);
            let values = doc.values();
            for channel in [values.roughness, values.metallic, values.alpha] {
                assert!((0.0..=1.0).contains(&channel), "{}", file.path);
            }
        }
    }
}
