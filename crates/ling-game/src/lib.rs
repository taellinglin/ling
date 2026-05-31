//! ling-game — ECS, physics, and game loop for Ling.

pub mod engine;
pub mod entity;
pub mod physics;
pub mod audio;
pub mod texture;
pub mod mesh;

pub use engine::{GameApp, GameSystem};
pub use entity::{Entity, EntityId, ComponentStore};
pub use physics::{RigidBody, Vec2, Aabb};
pub use audio::{AudioMixer, SoundHandle};
pub use texture::Palette;
pub use mesh::ProceduralMesh;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
