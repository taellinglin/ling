//! shading — holographic cel lighting model for the Ling renderer.
//!
//! The look the engine targets is *anime / holographic cel*: smooth shading
//! over the surface (no faceted triangle edges) but with **crisp posterised
//! bands** rather than a muddy Gouraud gradient. We get this by:
//!
//!   1. lighting each **vertex** with smooth (averaged) normals — continuous
//!      across the mesh,
//!   2. interpolating the lit colour across the triangle (done by the
//!      rasteriser), then
//!   3. **posterising per pixel** (luminance banded, chroma preserved) so the
//!      band boundaries are smooth curves over the surface — the anime line.
//!
//! On top of diffuse we add:
//!   • **coloured lights** — each light contributes its own RGB,
//!   • **coloured shadows** — unlit regions are tinted toward `shadow`
//!     (a complementary colour) instead of going flat black,
//!   • a **Fresnel rim** — a view-dependent edge glow for the holographic feel,
//!   • an optional **normal-gradient sheen** (`holo`) that shifts hue with the
//!     surface normal, like an iridescent film.
//!
//! All colours here are linear `[f32;3]` in `0..1`. The renderer converts to
//! `0x00RRGGBB` at the end.

/// A coloured point light in world space (mirror of the engine's `Light`).
#[derive(Clone, Copy, Debug)]
pub struct LightS {
    pub pos: [f32; 3],
    pub color: [f32; 3],
    pub intensity: f32,
    pub radius: f32, // 0 = no attenuation
}

/// Tunable parameters for the cel/holo model.
#[derive(Clone, Copy, Debug)]
pub struct ShadeParams {
    /// Number of posterisation bands (>=2). Lower = chunkier cel look.
    pub bands: u32,
    /// Ambient fill 0..1 applied to the base colour.
    pub ambient: f32,
    /// Coloured-shadow tint added in unlit regions (linear rgb 0..1).
    pub shadow: [f32; 3],
    /// Fresnel rim strength (0 = off).
    pub rim: f32,
    /// Rim glow colour.
    pub rim_color: [f32; 3],
    /// Enable the normal-gradient holographic sheen.
    pub holo: bool,
}

impl Default for ShadeParams {
    fn default() -> Self {
        Self {
            bands: 4,
            ambient: 0.22,
            shadow: [0.10, 0.13, 0.30], // cool indigo shadow
            rim: 0.6,
            rim_color: [0.45, 0.85, 1.0], // cyan holo edge
            holo: true,
        }
    }
}

#[inline] fn norm3(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0]*v[0] + v[1]*v[1] + v[2]*v[2]).sqrt();
    if l < 1e-8 { [0.0, 0.0, 0.0] } else { [v[0]/l, v[1]/l, v[2]/l] }
}
#[inline] fn dot3(a: [f32;3], b: [f32;3]) -> f32 { a[0]*b[0]+a[1]*b[1]+a[2]*b[2] }

/// Soft cel ramp for a single diffuse term: keeps smooth shading but gives the
/// lit/shadow transition a gentle "step" so it reads as toon shading even
/// before per-pixel posterisation. Smoothstep around two thresholds.
#[inline]
pub fn cel_ramp(d: f32) -> f32 {
    // d in 0..1 (already clamped)
    let lo = 0.30; let hi = 0.55;
    if d < lo { 0.20 }
    else if d > hi { 1.0 }
    else {
        // smoothstep lo..hi mapped to 0.20..1.0
        let t = (d - lo) / (hi - lo);
        let s = t * t * (3.0 - 2.0 * t);
        0.20 + s * 0.80
    }
}

/// Light one vertex. `base`, result in linear rgb 0..1.
/// `n` = smooth world normal, `pos` = world position, `eye` = camera position.
pub fn lit_vertex(
    base: [f32; 3],
    n: [f32; 3],
    pos: [f32; 3],
    eye: [f32; 3],
    lights: &[LightS],
    p: &ShadeParams,
) -> [f32; 3] {
    let n = norm3(n);
    // coloured-shadow baseline: ambient base + shadow tint where unlit
    let mut acc = [
        base[0] * p.ambient + p.shadow[0] * (1.0 - p.ambient),
        base[1] * p.ambient + p.shadow[1] * (1.0 - p.ambient),
        base[2] * p.ambient + p.shadow[2] * (1.0 - p.ambient),
    ];

    for l in lights {
        let d = [l.pos[0]-pos[0], l.pos[1]-pos[1], l.pos[2]-pos[2]];
        let dist = (d[0]*d[0]+d[1]*d[1]+d[2]*d[2]).sqrt().max(1e-6);
        let atten = if l.radius > 0.0 { (1.0 - dist/l.radius).max(0.0) } else { 1.0 };
        if atten <= 0.0 { continue; }
        let ldir = [d[0]/dist, d[1]/dist, d[2]/dist];
        let diff = dot3(n, ldir).max(0.0);      // front-lit only → real shadow side
        let shaded = cel_ramp(diff) * l.intensity * atten;
        acc[0] += base[0] * shaded * l.color[0];
        acc[1] += base[1] * shaded * l.color[1];
        acc[2] += base[2] * shaded * l.color[2];
    }

    // Fresnel rim — bright at grazing angles (view perpendicular to normal)
    if p.rim > 0.0 {
        let vd = norm3([eye[0]-pos[0], eye[1]-pos[1], eye[2]-pos[2]]);
        let f = (1.0 - dot3(n, vd).max(0.0)).clamp(0.0, 1.0);
        let rim = f * f * f * p.rim;           // tighten to the silhouette
        acc[0] += p.rim_color[0] * rim;
        acc[1] += p.rim_color[1] * rim;
        acc[2] += p.rim_color[2] * rim;
    }

    // Holographic normal-gradient sheen: iridescent hue tied to normal dir.
    // Modulated by the base colour so it tints rather than washing to white.
    if p.holo {
        let s = 0.07;
        acc[0] += (0.5 + 0.5 * n[0]) * s * (0.4 + 0.6*base[0]);
        acc[1] += (0.5 + 0.5 * n[1]) * s * (0.4 + 0.6*base[1]);
        acc[2] += (0.5 + 0.5 * n[2]) * s * (0.4 + 0.6*base[2]);
    }

    [acc[0].min(1.0), acc[1].min(1.0), acc[2].min(1.0)]
}

/// Posterise a colour into `bands` luminance levels while preserving chroma.
/// This is what turns the smooth interpolated colour into crisp cel bands.
#[inline]
pub fn posterize(c: [f32; 3], bands: u32) -> [f32; 3] {
    let bands = bands.max(2) as f32;
    let lum = 0.299*c[0] + 0.587*c[1] + 0.114*c[2];
    if lum < 1e-5 { return c; }
    // quantise luminance to the nearest band, then rescale chroma to it
    let q = ((lum * bands).floor() + 0.5) / bands;
    let k = (q / lum).clamp(0.0, 4.0);
    [(c[0]*k).min(1.0), (c[1]*k).min(1.0), (c[2]*k).min(1.0)]
}

/// Pack linear 0..1 rgb into 0x00RRGGBB.
#[inline]
pub fn pack(c: [f32; 3]) -> u32 {
    let r = (c[0].clamp(0.0,1.0) * 255.0) as u32;
    let g = (c[1].clamp(0.0,1.0) * 255.0) as u32;
    let b = (c[2].clamp(0.0,1.0) * 255.0) as u32;
    (r << 16) | (g << 8) | b
}

/// Unpack 0x00RRGGBB into linear 0..1 rgb.
#[inline]
pub fn unpack(rgb: u32) -> [f32; 3] {
    [((rgb>>16)&0xFF) as f32/255.0, ((rgb>>8)&0xFF) as f32/255.0, (rgb&0xFF) as f32/255.0]
}
