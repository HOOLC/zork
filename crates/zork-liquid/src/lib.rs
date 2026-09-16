//! Platform-independent liquid geometry, physics and presentation state.
//! Logical units are desktop logical pixels / Android dp. Hosts own clocks,
//! visibility, drawing, native input and business intents.
pub mod border;
mod contour;
mod field;
mod geometry;
pub mod motion;
mod physics;
pub mod ownership;
pub mod recipes;
pub mod scene;
mod surface;
mod travel;
pub mod tokens;

pub use contour::{trace, trace_dense, Contour, ContourError};
pub use geometry::{inset_radius, rounded_rectangle, row_inset, Cubic, Point, Pose};
pub use physics::{Constraints, Material, Options, Particle, Simulation, Spring, FIXED_DT};
pub use surface::Surface;

#[cfg(test)]
mod tests;
