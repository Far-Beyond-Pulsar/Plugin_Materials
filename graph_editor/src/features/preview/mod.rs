//! 3D material preview viewport
//!
//! Renders a 3D mesh with the compiled WGSL shader for material preview.

pub mod camera;
pub mod mesh;
pub mod panel;
pub mod renderer;
pub mod thumbnail;

pub use panel::MaterialPreviewPanel;
