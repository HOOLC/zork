//! Shared liquid material, contour and rendering contracts for native and WASM.
//! Geometry and physics contain no GPUI or platform types.
pub mod composer;
pub mod controls;
pub mod departure;
pub mod navigation;
pub mod overlay;
mod motion;
mod presentation;
pub mod panel;
pub mod press;
pub mod primitives;
mod render;
pub(crate) mod tessellation;

pub use zork_liquid::{
    trace, trace_dense, Contour, ContourError, inset_radius, rounded_rectangle,
    row_inset, Cubic, Point, Pose, Constraints, Material, Options, Particle,
    Simulation, Spring, FIXED_DT,
};
pub use render::{skin, ContentClip, ContentClipBinding, Surface, SurfaceColors};

#[cfg(test)]
mod tests;
