//! The materials' frame node is the host input that declares and supplies
//! the material texture table (their only other pass, the shading-cost
//! view, joins only while its debug view is shown). The table's node is
//! registered by the plugin's root through its render feature, a data-only
//! node (no flow pins, never on the execution chain) every drawer's
//! `MATERIAL_TEXTURES` input wires to, and absent, with the reason, while
//! the plugin is disabled.

// The plugin as a game links it: its root, with every registration.
use materials_plugin as _;

use std::sync::Mutex;

use engine_backend::subsystems::render::helio_renderer::{pipelines, render_features};
use helio_graph_asset::{validate, GraphAsset, NodeInstance};

const NODE: &str = "host.material_textures";

/// The plugin gate is process-wide state: every test takes this first.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn the_texture_table_is_a_registered_host_node() {
    let _serial = serial();
    pulsar_plugins::deactivate();
    assert_eq!(NODE, format!("host.{}", helio_mats::MATERIAL_TEXTURES));
    let registry = helio_presets::graphs::registry_with(&render_features::plugin_features());
    let node = registry.get(NODE).expect("the texture table node is registered");
    assert_eq!(registry.feature_of(NODE), Some("pulsar.materials"));
    assert!(node.data_only, "it only supplies data: no flow pins");
    assert!(node.generated);
    assert!(node.placement.is_some(), "the preset places it with the host inputs");
}

#[test]
fn a_disabled_plugin_has_no_node_and_a_graph_naming_it_says_so() {
    let _serial = serial();
    pulsar_plugins::deactivate();
    let mut asset = GraphAsset::default();
    asset.nodes.push(NodeInstance {
        id: "table".into(),
        kind: NODE.into(),
        params: Default::default(),
        position: [0.0; 2],
    });

    pulsar_plugins::activate(&pulsar_plugins::resolve(
        pulsar_plugins::installed(),
        &pulsar_plugins::ProjectPlugins::default().with(&["pulsar.materials"], false),
    ));
    let registry = pipelines::graph_node_registry();
    pulsar_plugins::deactivate();

    assert!(registry.get(NODE).is_none(), "its node is unregistered");
    assert_eq!(
        registry.absence(NODE),
        Some("plugin pulsar.materials disabled")
    );
    let validation = validate(&asset, &registry);
    let messages: Vec<_> = validation.errors().map(|d| d.message.clone()).collect();
    assert!(
        messages
            .iter()
            .any(|m| m == "missing node `host.material_textures` (plugin pulsar.materials disabled)"),
        "{messages:?}"
    );
    assert!(pipelines::graph_node_registry().get(NODE).is_some());
}
