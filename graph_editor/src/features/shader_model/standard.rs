//! Standard Unlit and PBR shader models
//!
//! Defines the built-in shader models that ship with the editor.

use super::{register, ShaderModel};

pub fn register_standard_models() {
    register(ShaderModel {
        id: "Standard_unlit".to_string(),
        name: "Standard Unlit".to_string(),
    });

    register(ShaderModel {
        id: "Standard_pbr".to_string(),
        name: "Standard PBR".to_string(),
    });
}
