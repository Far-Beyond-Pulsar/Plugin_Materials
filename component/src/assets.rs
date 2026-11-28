//! Material assets (SceneDB#84): a scalar surface document
//! ([`SurfaceMaterialDocument`] JSON) or a shader-graph folder
//! ([`SHADER_GRAPH_FILE`], with the textures it samples).
//!
//! The plugin owns the kind ([`helio_mats::asset::MATERIAL_ASSET_KIND`]):
//! its loader ([`load_material_asset`], registered by the plugin's root
//! crate) runs on a SceneDB worker thread and reads the document or compiles
//! the graph. A graph's textures are assets it needs
//! ([`material_dependencies`]): SceneDB loads them, coarse first, shared with
//! whatever else shows them, and [`compose_material`] makes the
//! [`MaterialPayload`] from them, again as each sharpens or changes. Whoever
//! draws with materials binds that payload and asks it for rows
//! ([`MaterialAsset::resolve`]), without linking this crate. A graph takes
//! slots in a scene's texture store when a draw in that scene resolves it.
//!
//! A material's source is its path as authored, normalized
//! (`helio_component::asset_source`): the loader resolves it against the
//! project, the working directory and the engine's assets.
//! [`material_sources_for_file`] maps a changed file to the materials it is
//! (a surface document, a graph save, or a texture a loaded graph samples),
//! for the engine's asset reloads.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use helio_component::asset_component::asset_source;
use helio_component::cooked_texture::TextureUse;
use helio_component::texture_assets::LoadedTexture;
use helio_mats::asset::{
    MaterialAsset, MaterialPayload, ResolvedMaterial, SurfaceMaterialDocument, SurfaceValues,
    SHADER_GRAPH_FILE,
};
use pulsar_scenedb::{AssetDependency, AssetId, AssetPayload};

/// A loaded material.
pub enum LoadedMaterial {
    /// A scalar surface document.
    Surface(SurfaceValues),
    /// A shader graph, compiled per scene (see [`GraphMaterial`]).
    Graph(GraphMaterial),
}

/// A texture a graph samples, as the material was composed with it.
#[derive(Clone)]
struct GraphTexture {
    /// The asset path the graph names it with.
    asset: String,
    id: AssetId,
    /// The texture's content version: its GPU copy follows it, so one
    /// texture sharpening or changing leaves the others' uploads alone.
    version: u64,
    image: Arc<LoadedTexture>,
}

/// A shader-graph material: the graph and the textures it samples.
#[derive(Clone)]
pub struct GraphMaterial {
    graph: psgc::GraphDescription,
    textures: Vec<GraphTexture>,
    /// Compiled while the asset loader parses this material, which runs on a
    /// SceneDB worker. Renderer initialization must only register this source;
    /// compiling here would make the first GPU mirror attach wait on PSGC.
    compiled_snippet: String,
    /// Whether the compiled surface reads the frame clock (a `time` node).
    pub reads_time: bool,
    /// What drives the graph's cut-out coverage (`crate::graph::opacity_mask`).
    mask: crate::graph::GraphMask,
    /// The graph's authored constant opacity (`crate::graph::constant_opacity`).
    opacity: Option<f32>,
}

/// Slot-independent compiled source plus the scene-local residency slots it
/// uses. The source is shared by every scene; only this row projection varies.
struct CompiledGraph {
    hash: u64,
    snippet: String,
}

impl MaterialAsset for LoadedMaterial {
    fn resolve(
        &self,
        id: AssetId,
        version: u64,
        mirror: Option<&pulsar_scenedb::gpu::GpuMirrorHandle>,
        fallback: &SurfaceValues,
    ) -> Result<ResolvedMaterial, String> {
        match self {
            Self::Surface(surface) => Ok(ResolvedMaterial::surface(surface)),
            Self::Graph(graph) => {
                let mirror = mirror.ok_or("a material graph draws only in a scene")?;
                let (hash, texture_slots) = compiled_graph(id, version, graph, mirror)?;
                let mut row = fallback.row(texture_slots);
                let mut texture_data = None;
                // A texture-masked graph cuts out in every pass at the same
                // cutoff: its mask texture is ordinal 0, the row's base colour.
                if let crate::graph::GraphMask::TextureAlpha { transform, .. } = &graph.mask {
                    row.flags |= helio_mats::FLAG_ALPHA_TEST;
                    // Sampled through a UV transform: the passes that cut out
                    // without running the graph (`material_cut_out`) read the
                    // base colour slot through the row's texture data.
                    if *transform != helio_mats::TextureTransform::IDENTITY {
                        let mut data = helio_mats::MaterialTextureData::derived(&row);
                        data.base_color =
                            helio_mats::MaterialTextureSlot::new(row.tex_base_color, 0, transform);
                        row.flags |= helio_mats::FLAG_TEXTURE_DATA;
                        texture_data = Some(data);
                    }
                }
                // A graph that authors an opacity below 1 is glass: blended,
                // transparent-only, at that alpha (the transparent drawer
                // reads the row's alpha, it does not run the graph).
                if let Some(opacity) = graph.opacity.filter(|opacity| *opacity < 1.0) {
                    row.base_color[3] *= opacity;
                    row.flags |= helio_mats::FLAG_ALPHA_BLEND | helio_mats::FLAG_TRANSPARENT_ONLY;
                }
                Ok(ResolvedMaterial {
                    row,
                    material_class: helio_mats::MATERIAL_CLASS_CUSTOM,
                    graph_hash: hash,
                    texture_data,
                })
            }
        }
    }

    fn reads_time(&self) -> bool {
        matches!(self, Self::Graph(graph) if graph.reads_time)
    }

    fn held_bytes(&self) -> usize {
        match self {
            Self::Graph(graph) => graph.textures.len() * 64,
            Self::Surface(_) => 0,
        }
    }

    fn warm(&self) {
        // A graph's textures the residency cache dropped start coming back.
        if let Self::Graph(graph) = self {
            for texture in &graph.textures {
                texture.image.warm();
            }
        }
    }
}

/// Bytes a loaded material holds outside SceneDB's residency cache: a
/// graph's texture handles (the images are budgeted by the cache, see
/// `LoadedTexture`). The size of the plugin's asset loader (the root crate
/// registers it).
pub fn payload_size(payload: &AssetPayload) -> usize {
    payload
        .downcast_ref::<MaterialPayload>()
        .map_or(0, |material| material.0.held_bytes())
}

/// The file the material at `source` is read from: a graph folder's save,
/// or the document itself.
fn resolve_material_file(source: &str) -> PathBuf {
    let path = match engine_state::get_project_path() {
        Some(root) => helio_component::subsystems::resolve_asset_path(Path::new(&root), source),
        None => PathBuf::from(source),
    };
    if engine_fs::virtual_fs::is_dir(&path) {
        path.join(SHADER_GRAPH_FILE)
    } else {
        path
    }
}

/// What the material loader makes: the material before the textures it needs
/// are in.
enum MaterialSource {
    Surface(SurfaceValues),
    Graph {
        /// Without textures yet.
        material: GraphMaterial,
        /// Each texture it samples: the path the graph names it with, and its
        /// asset.
        needs: Vec<(String, AssetId)>,
    },
}

/// The material kind's loader: runs on a SceneDB worker thread (the root
/// crate registers it for [`helio_mats::asset::MATERIAL_ASSET_KIND`]).
pub fn load_material_asset(source: &str) -> Result<AssetPayload, String> {
    Ok(Arc::new(read_material(source)?))
}

/// The textures a loaded material needs (the root crate registers it with
/// [`compose_material`]).
pub fn material_dependencies(loaded: &AssetPayload) -> Vec<AssetId> {
    match loaded.downcast_ref::<MaterialSource>() {
        Some(MaterialSource::Graph { needs, .. }) => needs.iter().map(|(_, id)| *id).collect(),
        _ => Vec::new(),
    }
}

/// The material, from what the loader made and the textures it needs, in
/// [`material_dependencies`]' order. Runs on a SceneDB worker thread, where
/// it prepares each texture's GPU form for the use the graph makes of it, so
/// the draw that binds it finds it ready.
pub fn compose_material(
    loaded: &AssetPayload,
    needs: &[AssetDependency],
) -> Result<AssetPayload, String> {
    let source = loaded
        .downcast_ref::<MaterialSource>()
        .ok_or("not a material")?;
    let material = match source {
        MaterialSource::Surface(surface) => LoadedMaterial::Surface(surface.clone()),
        MaterialSource::Graph {
            material,
            needs: named,
        } => {
            let uses = crate::graph::texture_uses(&material.graph);
            let mut material = material.clone();
            for ((asset, id), need) in named.iter().zip(needs) {
                let image = need
                    .payload::<LoadedTexture>()
                    .ok_or_else(|| match &need.state {
                        pulsar_scenedb::AssetState::Failed(error) => {
                            format!("texture '{asset}': {error}")
                        }
                        other => format!("texture '{asset}' is unavailable ({other:?})"),
                    })?;
                image
                    .prepare(texture_use(&uses, asset))
                    .map_err(|error| format!("texture '{asset}': {error}"))?;
                material.textures.push(GraphTexture {
                    asset: asset.clone(),
                    id: *id,
                    version: need.version,
                    image,
                });
            }
            LoadedMaterial::Graph(material)
        }
    };
    Ok(Arc::new(MaterialPayload(Arc::new(material))))
}

/// What `asset` is sampled as: a colour is stored as sRGB, a roughness or
/// normal map is not.
fn texture_use(uses: &HashMap<String, TextureUse>, asset: &str) -> TextureUse {
    uses.get(asset).copied().unwrap_or(TextureUse::Color)
}

/// Load the material at `source` and everything it needs now (for tools that
/// show a material outside a world).
pub fn load_material(source: &str) -> Result<Arc<dyn MaterialAsset>, String> {
    pulsar_scenedb::register_composer(
        helio_mats::asset::MATERIAL_ASSET_KIND,
        material_dependencies,
        compose_material,
    );
    let composed = pulsar_scenedb::compose_asset_now(
        helio_mats::asset::MATERIAL_ASSET_KIND,
        load_material_asset(source)?,
    )?;
    composed
        .downcast_ref::<MaterialPayload>()
        .map(|payload| payload.0.clone())
        .ok_or_else(|| "the material was not composed".to_string())
}

/// Read the material at `source`, without what it needs.
fn read_material(source: &str) -> Result<MaterialSource, String> {
    let file = resolve_material_file(source);
    let bytes = engine_fs::virtual_fs::read_file(&file)
        .map_err(|error| format!("could not read '{}': {error}", file.display()))?;
    if file
        .file_name()
        .is_some_and(|name| name == SHADER_GRAPH_FILE)
    {
        return load_graph(source, &bytes);
    }
    let material: SurfaceMaterialDocument = serde_json::from_slice(&bytes)
        .map_err(|error| format!("'{}' is not a surface material: {error}", file.display()))?;
    if material.version != SurfaceMaterialDocument::VERSION {
        return Err(format!(
            "'{}' has unsupported version {}",
            file.display(),
            material.version
        ));
    }
    Ok(MaterialSource::Surface(material.values()))
}

fn load_graph(source: &str, bytes: &[u8]) -> Result<MaterialSource, String> {
    let text = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
    // Shader graph saves may carry a line comment before their JSON body.
    let json_start = text
        .find('{')
        .ok_or_else(|| "shader graph JSON object is missing".to_string())?;
    let document: serde_json::Value = serde_json::from_str(&text[json_start..])
        .map_err(|error| format!("invalid shader graph JSON: {error}"))?;
    let graph_value = document
        .get("main_graph")
        .ok_or_else(|| "shader graph asset has no main_graph".to_string())?;
    let graph: psgc::GraphDescription = serde_json::from_value(graph_value.clone())
        .map_err(|error| format!("invalid main_graph: {error}"))?;
    let root = engine_state::get_project_path().map(PathBuf::from);
    let graph_textures = crate::graph::texture_assets(&graph)?;
    // The textures are assets of their own: named here with the use the graph
    // makes of each, loaded and shared by SceneDB.
    let uses = crate::graph::texture_uses(&graph);
    let needs: Vec<(String, AssetId)> = graph_textures
        .into_iter()
        .map(|asset| {
            let path = crate::graph::texture_path(&asset, root.as_deref());
            let texture_source = helio_component::texture_assets::texture_source_for_file(&path);
            let id = helio_component::texture_assets::needed_texture(
                &texture_source,
                texture_use(&uses, &asset),
            );
            (asset, id)
        })
        .collect();
    // Compile against placeholder slots while the SceneDB asset loader is on
    // a worker. Graph-local ordinals produce the same slot-independent source
    // later used by the renderer, so mirror attachment never runs PSGC.
    let placeholders = needs
        .iter()
        .enumerate()
        .map(|(slot, (asset, _))| (asset.clone(), slot as u32))
        .collect();
    let snippet = crate::graph::compile_material_graph(&graph, &placeholders)?;
    let mask = crate::graph::opacity_mask(&graph);
    if mask == crate::graph::GraphMask::Other {
        tracing::warn!(
            "material '{source}': its opacity mask is not a texture's alpha at the mesh UV, so it is not cut out (depth and shadow passes cannot evaluate the graph)"
        );
    }
    Ok(MaterialSource::Graph {
        material: GraphMaterial {
            mask,
            opacity: crate::graph::constant_opacity(&graph),
            reads_time: snippet.contains("radiant_graph_time()"),
            graph,
            textures: Vec::new(),
            compiled_snippet: snippet,
        },
        needs,
    })
}

/// The materials the file at `path` (absolute) is: a surface document, or a
/// graph save, which is its folder's material. A texture a material samples
/// is an asset of its own: its change reaches the material through SceneDB.
pub fn material_sources_for_file(path: &Path) -> Vec<String> {
    let mut sources = Vec::new();
    let file = path.to_string_lossy();
    if path
        .file_name()
        .is_some_and(|name| name == SHADER_GRAPH_FILE)
    {
        if let Some(folder) = path.parent().and_then(|folder| folder.to_str()) {
            sources.extend(asset_source(folder));
        }
    }
    sources.extend(asset_source(&file));
    sources.sort();
    sources.dedup();
    sources
}

/// Compiled graph sources (hash and WGSL), by material and content version.
/// Texture residency belongs to each material row, not the shader identity.
type CompiledGraphs = HashMap<(AssetId, u64), Result<CompiledGraph, String>>;
const MAX_COMPILED_GRAPHS: usize = 1024;

fn compiled_graphs() -> &'static Mutex<CompiledGraphs> {
    static COMPILED: OnceLock<Mutex<CompiledGraphs>> = OnceLock::new();
    COMPILED.get_or_init(Default::default)
}

/// Register `graph`'s textures in `mirror`'s store, register its worker-compiled
/// slot-independent source, and return the scene-local slots that the material
/// row binds. Register the source on every use because the renderer's graph
/// registry is bounded.
fn compiled_graph(
    id: AssetId,
    version: u64,
    graph: &GraphMaterial,
    mirror: &pulsar_scenedb::gpu::GpuMirrorHandle,
) -> Result<(u64, [u32; 5]), String> {
    profiling::profile_scope!("Material::bind_graph (textures to slots)");
    let assets = crate::graph::texture_assets(&graph.graph)?;
    if assets.len() > crate::graph::GRAPH_TEXTURE_FIELDS.len() {
        return Err(format!(
            "material graphs support at most {} sampled textures",
            crate::graph::GRAPH_TEXTURE_FIELDS.len()
        ));
    }
    let bindings: HashMap<_, _> = assets
        .iter()
        .enumerate()
        .map(|(ordinal, asset)| (asset.clone(), ordinal as u32))
        .collect();
    let mut slots = [helio_mats::GpuMaterial::NO_TEXTURE; 5];
    let uses = crate::graph::texture_uses(&graph.graph);
    // A material with several images the store does not hold yet cooks them
    // side by side on the loader threads, not one after another here.
    helio_component::texture_assets::prefetch_missing(
        graph.textures.iter().map(|texture| {
            (
                texture.id,
                texture.version,
                texture_use(&uses, &texture.asset),
                &*texture.image,
            )
        }),
        mirror,
    );
    for texture in &graph.textures {
        let asset = &texture.asset;
        let slot = helio_component::texture_assets::resident_slot_staged(
            texture.id,
            texture.version,
            &texture.image,
            texture_use(&uses, asset),
            mirror,
        )
        .map_err(|error| format!("texture '{asset}': {error}"))?;
        let ordinal = *bindings
            .get(asset)
            .ok_or_else(|| format!("texture '{asset}' is not sampled by the graph"))?
            as usize;
        slots[ordinal] = slot;
    }
    let mut compiled = compiled_graphs()
        .lock()
        .map_err(|_| "compiled graph cache poisoned".to_string())?;
    compiled
        .retain(|(cached_id, cached_version), _| *cached_id != id || *cached_version == version);
    if compiled.len() >= MAX_COMPILED_GRAPHS && !compiled.contains_key(&(id, version)) {
        if let Some(victim) = compiled.keys().next().copied() {
            compiled.remove(&victim);
        }
    }
    let result = compiled.entry((id, version)).or_insert_with(|| {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        graph.compiled_snippet.hash(&mut hasher);
        Ok(CompiledGraph {
            hash: hasher.finish().max(1),
            snippet: graph.compiled_snippet.clone(),
        })
    });
    let compiled = result.as_ref().map_err(Clone::clone)?;
    helio_mats::register_graph_source(compiled.hash, compiled.snippet.clone());
    Ok((compiled.hash, slots))
}

#[cfg(test)]
mod full_texture_table_tests {
    use super::*;
    use helio_mats::asset::MaterialAsset;
    use pulsar_scenedb::gpu::{
        EngineGpuContext, GpuMirrorHandle, SceneGpuConfig, SceneGpuStore, TextureStore,
    };
    use std::sync::RwLock;

    /// A graph material folder in `dir` whose base colour samples `texture`.
    fn write_graph(dir: &Path, texture: &Path) -> PathBuf {
        use psgc::*;
        fn node(id: &str, kind: &str) -> NodeInstance {
            let meta = get_shader_nodes()
                .into_iter()
                .find(|m| m.name == kind)
                .unwrap_or_else(|| panic!("no shader node {kind}"));
            let mut node = NodeInstance::new(id, kind, Position { x: 0.0, y: 0.0 });
            for p in meta.params {
                node.inputs.push(PinInstance::new(
                    &p.name,
                    Pin::new(
                        &p.name,
                        &p.name,
                        DataType::Data(TypeInfo::new(&p.param_type)),
                        PinType::Input,
                    ),
                ));
            }
            node
        }
        let mut graph = GraphDescription::new("full texture table");
        graph.add_node(node("output", "fragment_output"));
        graph.add_node(node("uv", "frag_uv"));
        let mut sample = node("texture", "sample_texture");
        sample.properties.insert(
            "texture".into(),
            serde_json::json!(texture.to_string_lossy()),
        );
        graph.add_node(sample);
        for (from, to, pin) in [("uv", "texture", "uv"), ("texture", "output", "base_color")] {
            graph.add_connection(Connection::new(
                from,
                "result",
                to,
                pin,
                ConnectionType::Data,
            ));
        }
        let folder = dir.join("wood.material");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join(SHADER_GRAPH_FILE),
            serde_json::json!({ "main_graph": graph }).to_string(),
        )
        .unwrap();
        folder
    }

    fn context() -> Option<EngineGpuContext> {
        let instance = wgpu::Instance::new(helio::wgpu_instance_descriptor());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).ok()?;
        Some(EngineGpuContext::new(Arc::new(device), Arc::new(queue)))
    }

    /// A scene whose texture table is full (it holds no textures at all).
    fn full_scene(ctx: &EngineGpuContext) -> GpuMirrorHandle {
        let store = Arc::new(SceneGpuStore::new(
            ctx,
            SceneGpuConfig {
                classes: vec![],
                tombstone_headroom: 0,
                max_cells_metadata: 0,
            },
        ));
        GpuMirrorHandle::new(store, Arc::clone(ctx.queue()))
            .with_texture_store(Arc::new(RwLock::new(TextureStore::new(0))))
            .expect("a texture store on the mirror's device")
    }

    #[test]
    fn a_graph_whose_texture_gets_no_slot_names_the_texture_and_the_full_table() {
        let Some(ctx) = context() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let dir = std::env::temp_dir().join(format!(
            "materials-full-texture-table-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let texture = dir.join("wood_albedo.png");
        image::save_buffer(
            &texture,
            &[200, 120, 40, 255],
            1,
            1,
            image::ColorType::Rgba8,
        )
        .unwrap();
        let folder = write_graph(&dir, &texture);
        let material = load_material(&folder.to_string_lossy()).expect("the graph loads");

        let scene = full_scene(&ctx);
        let id = AssetId::of(
            helio_mats::asset::MATERIAL_ASSET_KIND,
            "full-texture-table-test",
        );
        let error = material
            .resolve(id, 1, Some(&scene), &SurfaceValues::default())
            .expect_err("a full table cannot bind the graph's texture");
        assert!(error.contains("wood_albedo.png"), "{error}");
        assert!(
            error.contains("the scene's texture table is full"),
            "{error}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
