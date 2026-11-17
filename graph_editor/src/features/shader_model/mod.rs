//! Shader model system - registerable shader output models
//!
//! The models a material graph can pick from the toolbar: an id, stored in
//! the graph's metadata, and a display name.

pub mod standard;

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;

/// A registered shader model definition
#[derive(Clone, Debug)]
pub struct ShaderModel {
    /// Unique identifier (e.g. "Standard_unlit", "Standard_pbr")
    pub id: String,
    /// Display name for the UI
    pub name: String,
}

static REGISTRY: OnceLock<Mutex<HashMap<String, ShaderModel>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<String, ShaderModel>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn register(model: ShaderModel) {
    registry().lock().unwrap().insert(model.id.clone(), model);
}

pub fn get_all_models() -> Vec<ShaderModel> {
    registry().lock().unwrap().values().cloned().collect()
}
