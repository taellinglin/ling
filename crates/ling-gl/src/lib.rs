//! `ling-gl` — GPU rasteriser for Ling's already-projected scenes.
//!
//! The engine projects, clips and sorts on the CPU; this crate only fills
//! perspective-correct textured triangles with a hardware depth buffer, tint,
//! fog and blending. It runs on wgpu, which means Vulkan / Metal / DX12 on
//! desktop and WebGL2 in the browser, from one shader and one scene format.
//!
//! * [`Scene`] / [`Batch`] / [`Vertex`] — the backend-neutral packed scene.
//! * [`Renderer`] — encodes a scene into any colour + depth view you own
//!   (a window surface, a canvas, or an offscreen texture).
//! * [`Headless`] (native) — owns a device and reads the frame back into the
//!   engine's `0x00RRGGBB` framebuffer, with `None` when no GPU is present so
//!   callers fall back to the CPU rasteriser.

#[cfg(feature = "gpu")]
mod renderer;
mod scene;

#[cfg(feature = "gpu")]
pub use renderer::Renderer;
#[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
pub use renderer::Headless;
pub use scene::*;
