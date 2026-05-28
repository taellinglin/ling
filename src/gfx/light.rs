// src/gfx/light.rs — Point lights with cel-shading quantisation.
//
// Design:
//   • Each Light lives in 3-D world space (same space as geometry before
//     camera rotation).  For 4-D scenes, the Ling program projects lights
//     from 4-D → 3-D the same way it projects geometry, then registers them
//     with `add_light`.
//
//   • Shading is cel / toon: the diffuse dot-product is quantised into
//     discrete bands so every face is either shadowed, mid-tone, or fully lit.
//     This gives the crisp, vector-graphic look of flat-shaded cel animation.
//
//   • Distance attenuation is linear within `radius`; beyond `radius` the
//     contribution is zero.  `radius == 0` means infinite (no attenuation).
//
//   • Both sides of every face are illuminated (|dot|) — avoids harsh black
//     patches from back-facing geometry.

/// A coloured point light in 3-D world space.
#[derive(Debug, Clone)]
pub struct Light {
    /// World-space position.
    pub x: f32, pub y: f32, pub z: f32,
    /// Linear RGB colour components in [0..1].
    pub r: f32, pub g: f32, pub b: f32,
    /// Peak intensity multiplier (1.0 = full pen colour at contact).
    pub intensity: f32,
    /// Influence radius.  0 = no distance attenuation.
    pub radius: f32,
}

/// Cel / toon quantisation — maps a continuous diffuse value → 3 discrete bands.
///
/// ```text
///  raw dot   band    visual
///  ───────────────────────────
///  0.00–0.25  shadow  (dim)
///  0.25–0.60  mid     (flat)
///  0.60–1.00  lit     (bright)
/// ```
#[inline]
pub fn cel_quantize(v: f32) -> f32 {
    if      v < 0.25 { 0.08 }
    else if v < 0.60 { 0.50 }
    else             { 1.00 }
}

/// Compute the final lit colour for one triangle face.
///
/// Parameters
/// - `base`      : 0x00RRGGBB base colour set by `สีดินสอ`
/// - `normal`    : un-normalised world-space face normal (B−A) × (C−A)
/// - `centroid`  : average of the three vertices in world space
/// - `lights`    : active lights for this frame
/// - `ambient`   : ambient fill in [0..1]
///
/// Returns 0x00RRGGBB with lighting applied.
pub fn compute_lit_color(
    base:     u32,
    normal:   [f32; 3],
    centroid: [f32; 3],
    lights:   &[Light],
    ambient:  f32,
) -> u32 {
    let br = ((base >> 16) & 0xFF) as f32;
    let bg = ((base >>  8) & 0xFF) as f32;
    let bb = ( base        & 0xFF) as f32;

    // Accumulate light contributions starting from ambient fill
    let mut acc_r = br * ambient;
    let mut acc_g = bg * ambient;
    let mut acc_b = bb * ambient;

    // Normalise the face normal once
    let [nx, ny, nz] = normal;
    let nlen = (nx*nx + ny*ny + nz*nz).sqrt();
    if nlen < 1e-6 {
        // Degenerate triangle — return ambient only
        return pack(acc_r.min(255.0), acc_g.min(255.0), acc_b.min(255.0));
    }
    let nx = nx / nlen;
    let ny = ny / nlen;
    let nz = nz / nlen;

    for l in lights {
        // Direction from face centroid → light
        let dx = l.x - centroid[0];
        let dy = l.y - centroid[1];
        let dz = l.z - centroid[2];
        let dist = (dx*dx + dy*dy + dz*dz).sqrt().max(1e-6);

        // Linear attenuation within radius
        let atten = if l.radius > 0.0 {
            (1.0 - dist / l.radius).max(0.0)
        } else {
            1.0
        };
        if atten <= 0.0 { continue; }

        let lx = dx / dist;
        let ly = dy / dist;
        let lz = dz / dist;

        // |dot|: illuminate both sides (back-lit face looks mid-tone, not black)
        let raw = (nx*lx + ny*ly + nz*lz).abs();
        let shaded = cel_quantize(raw) * l.intensity * atten;

        acc_r += br * shaded * l.r;
        acc_g += bg * shaded * l.g;
        acc_b += bb * shaded * l.b;
    }

    pack(acc_r.min(255.0), acc_g.min(255.0), acc_b.min(255.0))
}

#[inline]
fn pack(r: f32, g: f32, b: f32) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}
