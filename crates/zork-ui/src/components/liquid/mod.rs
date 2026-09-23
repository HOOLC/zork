//! Shared liquid material, contour and rendering contracts for native views.
//! Geometry and physics contain no GPUI or platform types.
pub mod composer;
pub mod controls;
pub mod departure;
pub mod motion;
pub mod navigation;
pub mod overlay;
pub mod panel;
mod presentation;
pub mod press;
pub mod primitives;
mod render;
pub(crate) mod tessellation;

pub use render::{skin, ContentClip, ContentClipBinding, Surface, SurfaceColors};
pub use zork_liquid::{
    inset_radius, rounded_rectangle, row_inset, trace, trace_dense, Constraints, Contour,
    ContourError, Cubic, Material, Options, Particle, Point, Pose, Simulation, Spring, FIXED_DT,
};

#[cfg(test)]
mod tests;
