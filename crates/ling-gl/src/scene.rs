//! Backend-neutral packed scene: what the engine's depth queue turns into once
//! every vertex is already projected to screen space.

use bytemuck::{Pod, Zeroable};

/// Far plane used to normalise camera depth into the GPU depth buffer.
pub const ZFAR: f32 = 4096.0;

/// One projected vertex. `iw`, `uw`, `vw` are 1/w, u/w and v/w, so the fragment
/// stage recovers perspective-correct UVs and depth exactly like the CPU path.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct Vertex {
    pub x: f32,
    pub y: f32,
    pub iw: f32,
    pub uw: f32,
    pub vw: f32,
    pub tint: [f32; 4],
    /// World position times 1/w, for per-pixel point lights.
    pub w: [f32; 3],
}

/// Per-pixel point lighting: `amb` plus up to [`MAX_LIGHTS`] lights
/// `[x, y, z, r, g, b, intensity, radius]` with a squared falloff.
#[derive(Clone, Debug, Default)]
pub struct Lighting {
    pub lit: bool,
    pub amb: f32,
    pub lights: Vec<[f32; 8]>,
}

pub const MAX_LIGHTS: usize = 8;

/// Blend mode of a batch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Blend {
    #[default]
    Opaque,
    Alpha,
    Additive,
}

/// Distance fog applied per fragment (`end <= start` disables it).
#[derive(Clone, Copy, Debug)]
pub struct Fog {
    pub start: f32,
    pub end: f32,
    pub color: [f32; 3],
}

impl Default for Fog {
    fn default() -> Self {
        Fog { start: 0.0, end: 0.0, color: [0.0; 3] }
    }
}

/// A texture, 0xAARRGGBB texels. `id` is a caller-chosen key: textures are
/// uploaded once and reused while the id stays the same.
pub struct Texture {
    pub id: u64,
    pub w: u32,
    pub h: u32,
    pub px: Vec<u32>,
}

/// A run of triangles sharing texture, blend mode and fog. Batches draw in
/// order; the depth buffer resolves visibility among opaque ones.
#[derive(Default)]
pub struct Batch {
    /// `None` draws untextured (the tint alone).
    pub texture: Option<u64>,
    pub blend: Blend,
    pub fog: Fog,
    pub lighting: Lighting,
    /// Triangle list, 3 vertices per triangle.
    pub verts: Vec<Vertex>,
}

#[derive(Default)]
pub struct Scene {
    pub textures: Vec<Texture>,
    pub batches: Vec<Batch>,
    /// Background colour (linear RGB 0..1).
    pub clear: [f32; 3],
    /// 0 leaves uncovered pixels transparent so the result can be composited over an existing frame.
    pub clear_alpha: f32,
    /// Camera distance offset: the CPU rasteriser's depth is `1/iw - zdist`.
    pub zdist: f32,
}

impl Scene {
    pub fn new(clear: [f32; 3]) -> Self {
        Scene { textures: Vec::new(), batches: Vec::new(), clear, clear_alpha: 1.0, zdist: 0.0 }
    }

    pub fn tri_count(&self) -> usize {
        self.batches.iter().map(|b| b.verts.len() / 3).sum()
    }
}

/// Pack 0xAARRGGBB into straight-alpha RGBA bytes.
pub fn argb_to_rgba(px: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(px.len() * 4);
    for &p in px {
        out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8, (p >> 24) as u8]);
    }
    out
}
