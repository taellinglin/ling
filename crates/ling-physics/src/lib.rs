//! ling-physics — simulation, world generation, and hyperbolic geometry.
//!
//! Modules:
//! - [`vector`]    — vector operations, linear algebra (dot, cross, distance)
//! - [`forces`]    — force calculations (gravity, drag, spring, buoyancy, wind)
//! - [`rigid`]     — rigid-body dynamics (position-based + impulse)
//! - [`soft`]      — spring-mass soft bodies (deformable ball bounce, cloth)
//! - [`liquid`]    — fast 2-D immiscible water/oil grid fluid (surface-mappable)
//! - [`terrain`]   — chunk-based terrain with fractal noise and LOD
//! - [`foliage`]   — procedural trees, grass, wind deformation
//! - [`weather`]   — day/night cycle, atmosphere, wind, precipitation
//! - [`hyperbolic`]— Poincaré ball model, hyperbolic sphere worlds
//! - [`world`]     — scene descriptor, chunk manager, entity placement
//! - [`gltf`]      — glTF 2.0 model loading with skeletal animation

pub mod foliage;
pub mod forces;
pub mod gltf;
pub mod hyperbolic;
pub mod liquid;
pub mod rigid;
pub mod soft;
pub mod terrain;
pub mod vector;
pub mod weather;
pub mod world;

pub use glam::{Mat4, Quat, Vec2, Vec3, Vec4};
