//! ling-game — ECS, physics, and game loop for Ling.

pub mod audio;
pub mod dialog;
pub mod engine;
pub mod entity;
pub mod mesh;
pub mod physics;
pub mod texture;

pub use audio::{AudioMixer, SoundHandle};
pub use dialog::{Dialog, Role as DialogRole};
pub use engine::{GameApp, GameSystem};
pub use entity::{ComponentStore, Entity, EntityId};
pub use mesh::ProceduralMesh;
pub use physics::{Aabb, RigidBody, Vec2};
pub use texture::Palette;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
