//! ling-physics — simulation, world generation, and hyperbolic geometry.
//!
//! Modules:
//! - [`rigid`]     — rigid-body dynamics (position-based + impulse)
//! - [`soft`]      — spring-mass soft bodies (deformable ball bounce, cloth)
//! - [`terrain`]   — chunk-based terrain with fractal noise and LOD
//! - [`foliage`]   — procedural trees, grass, wind deformation
//! - [`weather`]   — day/night cycle, atmosphere, wind, precipitation
//! - [`hyperbolic`]— Poincaré ball model, hyperbolic sphere worlds
//! - [`world`]     — scene descriptor, chunk manager, entity placement
//! - [`gltf`]      — glTF 2.0 model loading with skeletal animation

pub mod rigid;
pub mod soft;
pub mod terrain;
pub mod foliage;
pub mod weather;
pub mod hyperbolic;
pub mod world;
pub mod gltf;

pub use glam::{Vec2, Vec3, Vec4, Mat4, Quat};
