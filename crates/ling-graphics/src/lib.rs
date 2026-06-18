//! ling-graphics — 3D/4D rendering, geometry, animation, and font tools.

pub mod animation;
pub mod camera;
pub mod color;
pub mod font;
pub mod geometry;
pub mod material;
pub mod math;
pub mod renderer;
pub mod scene;
pub mod shading;
pub mod vfont;
pub mod viewport;
pub mod window;

pub use animation::{EaseFunction, Keyframe, Lerp, Timeline, Track};
pub use camera::{Camera3D, Camera4D, HyperModel, Projection};
pub use color::{BlendMode, Color, ColorGradient};
pub use font::{FontAtlas, GlyphFont, GlyphInfo};
pub use geometry::{Mesh, MeshBuilder, Vertex};
pub use material::{AlphaMode, Material, TextureData};
pub use math::{Aabb, Frustum, Mat3, Mat4, Mat5, Plane, Quat, Ray3, Vec2, Vec3, Vec4, Vec4H};
pub use renderer::{FrameBuffer, Renderer, SoftwareRenderer};
pub use scene::{NodeId, Scene, SceneNode, Transform};
pub use shading::{lit_vertex, posterize, LightS, ShadeParams};
pub use vfont::{GlyphOutline, VectorFont};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
