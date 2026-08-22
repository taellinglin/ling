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
    pub x: f32,
    pub y: f32,
    pub z: f32,
    /// Linear RGB colour components in [0..1].
    pub r: f32,
    pub g: f32,
    pub b: f32,
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
    if v < 0.25 {
        0.08
    } else if v < 0.60 {
        0.50
    } else {
        1.00
    }
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
    base: u32,
    normal: [f32; 3],
    centroid: [f32; 3],
    lights: &[Light],
    ambient: f32,
) -> u32 {
    let br = ((base >> 16) & 0xFF) as f32;
    let bg = ((base >> 8) & 0xFF) as f32;
    let bb = (base & 0xFF) as f32;

    // Accumulate light contributions starting from ambient fill
    let mut acc_r = br * ambient;
    let mut acc_g = bg * ambient;
    let mut acc_b = bb * ambient;

    // Ambient-only scene (no point lights): skip the normal-normalise sqrt and
    // the light loop. Hot for flat ambient geometry (floor tiles, HUD, etc.).
    if lights.is_empty() {
        return pack(acc_r.min(255.0), acc_g.min(255.0), acc_b.min(255.0));
    }

    // Normalise the face normal once
    let [nx, ny, nz] = normal;
    let nlen = (nx * nx + ny * ny + nz * nz).sqrt();
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
        let dist = (dx * dx + dy * dy + dz * dz).sqrt().max(1e-6);

        // Linear attenuation within radius
        let atten = if l.radius > 0.0 {
            (1.0 - dist / l.radius).max(0.0)
        } else {
            1.0
        };
        if atten <= 0.0 {
            continue;
        }

        let lx = dx / dist;
        let ly = dy / dist;
        let lz = dz / dist;

        // |dot|: illuminate both sides (back-lit face looks mid-tone, not black)
        let raw = (nx * lx + ny * ly + nz * lz).abs();
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

/// Linearly interpolate between two 0x00RRGGBB colours by factor `t ∈ [0,1]`.
#[inline]
pub fn lerp_color(a: u32, b: u32, t: f32) -> u32 {
    let ar = ((a >> 16) & 0xFF) as f32;
    let ag = ((a >> 8) & 0xFF) as f32;
    let ab = (a & 0xFF) as f32;
    let br = ((b >> 16) & 0xFF) as f32;
    let bg = ((b >> 8) & 0xFF) as f32;
    let bb = (b & 0xFF) as f32;
    pack(ar + (br - ar) * t, ag + (bg - ag) * t, ab + (bb - ab) * t)
}

/// Like `compute_lit_color` but uses the raw (non-quantised) diffuse dot product.
///
/// Use this for per-vertex colours going into a Gouraud shader — the shader's
/// per-pixel cel-quantise step (`fill_triangle_gouraud*` with `bands >= 2`)
/// will snap the smooth gradient to discrete bands after interpolation, giving a
/// continuous gradient-of-shading-bands instead of a flat cel quantise at each
/// vertex (which creates colour seams at vertex boundaries).
pub fn compute_lit_color_linear(
    base: u32,
    normal: [f32; 3],
    centroid: [f32; 3],
    lights: &[Light],
    ambient: f32,
) -> u32 {
    let br = ((base >> 16) & 0xFF) as f32;
    let bg = ((base >> 8) & 0xFF) as f32;
    let bb = (base & 0xFF) as f32;

    let mut acc_r = br * ambient;
    let mut acc_g = bg * ambient;
    let mut acc_b = bb * ambient;

    if lights.is_empty() {
        return pack(acc_r.min(255.0), acc_g.min(255.0), acc_b.min(255.0));
    }

    let [nx, ny, nz] = normal;
    let nlen = (nx * nx + ny * ny + nz * nz).sqrt();
    if nlen < 1e-6 {
        return pack(acc_r.min(255.0), acc_g.min(255.0), acc_b.min(255.0));
    }
    let (nx, ny, nz) = (nx / nlen, ny / nlen, nz / nlen);

    for l in lights {
        let dx = l.x - centroid[0];
        let dy = l.y - centroid[1];
        let dz = l.z - centroid[2];
        let dist = (dx * dx + dy * dy + dz * dz).sqrt().max(1e-6);

        let atten = if l.radius > 0.0 {
            (1.0 - dist / l.radius).max(0.0)
        } else {
            1.0
        };
        if atten <= 0.0 {
            continue;
        }

        let lx = dx / dist;
        let ly = dy / dist;
        let lz = dz / dist;

        // Raw (linear) dot product — no cel-quantise here.
        let raw = (nx * lx + ny * ly + nz * lz).abs().clamp(0.0, 1.0);
        let shaded = raw * l.intensity * atten;

        acc_r += br * shaded * l.r;
        acc_g += bg * shaded * l.g;
        acc_b += bb * shaded * l.b;
    }

    pack(acc_r.min(255.0), acc_g.min(255.0), acc_b.min(255.0))
}

/// Compute lit colours at each of the three triangle vertices independently,
/// producing a smooth gradient across the face under directional/point lights.
///
/// Uses linear (non-cel-quantised) vertex colours so the Gouraud shader can
/// apply per-pixel cel-banding on the smoothly interpolated result — matching
/// the toon shading of flat triangles but without colour seams at vertices.
pub fn compute_lit_color_vertices(
    base: u32,
    normal: [f32; 3],
    va: [f32; 3],
    vb: [f32; 3],
    vc: [f32; 3],
    lights: &[Light],
    ambient: f32,
) -> (u32, u32, u32) {
    compute_lit_color_vertices3(base, normal, normal, normal, va, vb, vc, lights, ambient)
}

/// Like `compute_lit_color_vertices` but takes an independent normal per
/// vertex, so a curved surface built from flat triangles (e.g. terrain) can
/// shade smoothly instead of faceting at triangle edges.
#[allow(clippy::too_many_arguments)]
pub fn compute_lit_color_vertices3(
    base: u32,
    na: [f32; 3],
    nb: [f32; 3],
    nc: [f32; 3],
    va: [f32; 3],
    vb: [f32; 3],
    vc: [f32; 3],
    lights: &[Light],
    ambient: f32,
) -> (u32, u32, u32) {
    (
        compute_lit_color_linear(base, na, va, lights, ambient),
        compute_lit_color_linear(base, nb, vb, lights, ambient),
        compute_lit_color_linear(base, nc, vc, lights, ambient),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn white() -> u32 {
        0x00FF_FFFF
    }
    fn red() -> u32 {
        0x00FF_0000
    }

    #[test]
    fn lerp_color_endpoints() {
        // t=0 → a, t=1 → b
        assert_eq!(lerp_color(red(), white(), 0.0), red());
        assert_eq!(lerp_color(red(), white(), 1.0), white());
    }

    #[test]
    fn lerp_color_midpoint() {
        // 0xFF0000 and 0x00FF00 midpoint should be ~0x7F7F00
        let a = 0x00FF_0000u32;
        let b = 0x0000_FF00u32;
        let mid = lerp_color(a, b, 0.5);
        let r = (mid >> 16) & 0xFF;
        let g = (mid >> 8) & 0xFF;
        let bl = mid & 0xFF;
        assert!((r as i32 - 0x7F).abs() <= 1, "r={r:#04x}");
        assert!((g as i32 - 0x7F).abs() <= 1, "g={g:#04x}");
        assert_eq!(bl, 0);
    }

    #[test]
    fn ambient_only_no_lights() {
        // With no lights, ambient=1.0 should return the base colour unchanged.
        let base = 0x0040_8060u32;
        let result = compute_lit_color(base, [0.0, 1.0, 0.0], [0.0, 0.0, 0.0], &[], 1.0);
        assert_eq!(result, base);
    }

    #[test]
    fn single_point_light_gradient() {
        // A light positioned near vertex A should make A brighter than C.
        let base = 0x00FF_FFFF;
        let normal = [0.0, 0.0, 1.0];
        let light = Light {
            x: 0.0,
            y: 0.0,
            z: 10.0,
            r: 1.0,
            g: 1.0,
            b: 1.0,
            intensity: 1.0,
            radius: 0.0,
        };
        let va = [0.0f32, 0.0, 0.0];
        let vb = [100.0, 0.0, 0.0];
        let vc = [200.0, 0.0, 0.0];
        let (ca, _cb, cc) = compute_lit_color_vertices(base, normal, va, vb, vc, &[light], 0.1);
        // vertex A is closer to the light, so its red channel should be >= C's
        let ra = (ca >> 16) & 0xFF;
        let rc = (cc >> 16) & 0xFF;
        assert!(
            ra >= rc,
            "expected A ({ra}) brighter than C ({rc}) under close light"
        );
    }

    #[test]
    fn flat_shade_returns_same_color() {
        // compute_lit_color_vertices with ambient=1.0 and no lights returns
        // the base colour at every vertex (no gradient possible).
        let base = 0x00AA_BBCC;
        let normal = [0.0, 1.0, 0.0];
        let (a, b, c) = compute_lit_color_vertices(
            base,
            normal,
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            &[],
            1.0,
        );
        assert_eq!(a, base);
        assert_eq!(b, base);
        assert_eq!(c, base);
    }

    #[test]
    fn vertices3_with_a_shared_normal_matches_the_single_normal_variant() {
        let base = 0x00FF_FFFF;
        let normal = [0.0, 1.0, 0.0];
        let light = Light {
            x: 0.0,
            y: 5.0,
            z: 0.0,
            r: 1.0,
            g: 1.0,
            b: 1.0,
            intensity: 1.0,
            radius: 10.0,
        };
        let va = [0.0, 0.0, 0.0];
        let vb = [1.0, 0.0, 0.0];
        let vc = [0.0, 0.0, 1.0];
        let one =
            compute_lit_color_vertices(base, normal, va, vb, vc, std::slice::from_ref(&light), 0.1);
        let three =
            compute_lit_color_vertices3(base, normal, normal, normal, va, vb, vc, &[light], 0.1);
        assert_eq!(one, three);
    }

    #[test]
    fn vertices3_with_independent_normals_shade_differently() {
        // A terrain-style seam: same position, different slopes, must not
        // collapse to identical colours the way a single shared face normal would.
        let base = 0x00FF_FFFF;
        let pos = [0.0, 0.0, 0.0];
        let light = Light {
            x: 8.0,
            y: 2.0,
            z: 0.0,
            r: 1.0,
            g: 1.0,
            b: 1.0,
            intensity: 1.0,
            radius: 20.0,
        };
        let flat = compute_lit_color_linear(
            base,
            [0.0, 1.0, 0.0],
            pos,
            std::slice::from_ref(&light),
            0.1,
        );
        let tilted = compute_lit_color_linear(base, [1.0, 0.0, 0.0], pos, &[light], 0.1);
        assert_ne!(
            flat, tilted,
            "different normals at the same point must shade differently"
        );
    }
}
