//! Materials (`pulsar.materials`): the material assets, the material graph
//! compiler and the materials' Helio feature set.
//!
//! - [`assets`]: the material asset kind (a scalar surface document or a
//!   shader-graph folder): its SceneDB loader and its reloads. Whoever draws
//!   with materials reads them through the materials' api
//!   (`helio_mats::asset`).
//! - [`graph`]: the one material graph compiler; [`graph_preview`]
//!   compiles a graph for a preview without a scene.
//! - The materials' Helio feature (`helio-pass-materials`: the material
//!   texture table every kind's drawers bind, [`render_features`]) and its
//!   sampler, from the project's texture streaming settings
//!   ([`configure_renderer`], [`sampler`]).
//!
//! This crate registers nothing itself: the plugin's root crate
//! (`materials_plugin`) submits the render feature, the asset loader and the
//! reloads through the engine's generic seams.

pub mod assets;
pub mod graph;
pub mod graph_preview;
pub mod preview_program;
pub mod sampler;

/// The materials' features: the texture table.
pub fn render_features() -> helio_feature_api::FeatureSet {
    helio_feature_api::FeatureSet::new().with(helio_pass_materials::MaterialsFeature)
}

/// The material sampler from the project's `streaming.texture_*` settings:
/// scene materials read through trilinear, anisotropic, repeating filtering,
/// not wgpu's nearest/clamp default.
pub fn configure_renderer(config: helio::RendererConfig) -> helio::RendererConfig {
    config.with_feature(helio_mats::MaterialSampler(
        sampler::SamplerSettings::from_project().descriptor(),
    ))
}
