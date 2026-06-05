//! ling-graphics — 3D/4D rendering, geometry, animation, and font tools.

pub mod math;
pub mod color;
pub mod geometry;
pub mod material;
pub mod camera;
pub mod scene;
pub mod animation;
pub mod font;
pub mod vfont;
pub mod renderer;
pub mod viewport;
pub mod window;
pub mod shading;


pub use math::{Vec2, Vec3, Vec4, Mat3, Mat4, Quat, Vec4H, Mat5, Aabb, Ray3, Plane, Frustum};
pub use color::{Color, BlendMode, ColorGradient};
pub use geometry::{Vertex, Mesh, MeshBuilder};
pub use material::{Material, TextureData, AlphaMode};
pub use camera::{Camera3D, Camera4D, Projection, HyperModel};
pub use scene::{Transform, SceneNode, Scene, NodeId};
pub use animation::{Timeline, Track, Keyframe, EaseFunction, Lerp};
pub use font::{FontAtlas, GlyphInfo, GlyphFont};
pub use vfont::{VectorFont, GlyphOutline};
pub use shading::{LightS, ShadeParams, lit_vertex, posterize};
pub use renderer::{Renderer, FrameBuffer, SoftwareRenderer};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
