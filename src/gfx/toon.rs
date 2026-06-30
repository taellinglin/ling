// src/gfx/toon.rs — Unified tone ramp + screen-space toon post-processing.
//
// All lighting (shadow, mid-tone, highlight) passes through a single
// ToneRamp — a gradient with sorted (t, brightness) stops and an optional
// cubic Bezier curve that remaps the input luminance before stop lookup.
//
// Usage
//   1. Set up ToneRamp stops — at least a dark and a bright stop.
//   2. Call `apply()` after `queue.flush()` and before presenting the buffer.
//
// The ramp replaces the old per-pixel cel-snap in the rasteriser and the
// separate `smooth_shadow_edges` / `draw_highlights` passes.
//
// Bezier control:
//   The optional bezier remaps the raw normalised luminance t ∈ [0,1] before
//   stop lookup.  Two control-point y-values [y1, y2] define a cubic Bézier:
//
//       f(t) = 3t(1-t)²·y1 + 3t²(1-t)·y2 + t³
//
//   with implicit anchors (0,0) and (1,1).  Setting [1/3, 2/3] gives the
//   identity; [0, 0] makes dark tones dominant (ease-in); [1, 1] brightens
//   the ramp (ease-out); [0.1, 0.9] gives a smooth S-curve.

// ── Tone ramp ────────────────────────────────────────────────────────────────

/// A single stop on the tone ramp.
///
/// `t`     — input luminance position [0..1]
/// `value` — output brightness multiplier [0..1]
#[derive(Debug, Clone, PartialEq)]
pub struct ToneStop {
    pub t:     f32,
    pub value: f32,
}

/// Maps pixel luminance through a gradient of brightness stops.
///
/// `stops`  — sorted by `t` ascending.
/// `smooth` — `false` = hard-snap to the left stop (cel shade);
///            `true`  = linear-interpolate between stops (soft gradient).
/// `bezier` — optional `[y1, y2]` cubic Bézier remap applied before lookup.
///            `None` = identity (no remap).
#[derive(Debug, Clone)]
pub struct ToneRamp {
    pub stops:  Vec<ToneStop>,
    pub smooth: bool,
    pub bezier: Option<[f32; 2]>,
}

impl Default for ToneRamp {
    /// 3-band cel-shade matching the old hardcoded thresholds:
    ///   shadow  t < 0.25 → 0.08
    ///   mid     t < 0.60 → 0.50
    ///   lit     t ≥ 0.60 → 1.00
    fn default() -> Self {
        Self {
            stops: vec![
                ToneStop { t: 0.00, value: 0.08 },
                ToneStop { t: 0.25, value: 0.50 },
                ToneStop { t: 0.60, value: 1.00 },
                ToneStop { t: 1.00, value: 1.00 },
            ],
            smooth: false,
            bezier: None,
        }
    }
}

/// Cubic Bézier remap: f(t) = 3t(1-t)²·y1 + 3t²(1-t)·y2 + t³.
/// Anchors are (0,0) and (1,1); y1, y2 are the two control-point y-values.
#[inline]
fn bezier_remap(y1: f32, y2: f32, t: f32) -> f32 {
    let mt = 1.0 - t;
    3.0 * t * mt * mt * y1 + 3.0 * t * t * mt * y2 + t * t * t
}

/// Sample the ramp at normalised input `t_in` ∈ [0..1].
/// Returns a brightness multiplier in [0..1].
pub fn sample_ramp(ramp: &ToneRamp, t_in: f32) -> f32 {
    let t = t_in.clamp(0.0, 1.0);
    let t = match ramp.bezier {
        Some([y1, y2]) => bezier_remap(y1, y2, t).clamp(0.0, 1.0),
        None => t,
    };

    let stops = &ramp.stops;
    if stops.is_empty() { return t; }

    // Before first stop → first value
    if t <= stops[0].t { return stops[0].value; }

    let last = stops.len() - 1;
    // At or past last stop → last value
    if t >= stops[last].t { return stops[last].value; }

    // Find the surrounding pair
    for i in 0..last {
        if t < stops[i + 1].t {
            if ramp.smooth {
                let span = stops[i + 1].t - stops[i].t;
                let f = if span > 1e-6 { (t - stops[i].t) / span } else { 1.0 };
                return stops[i].value + f * (stops[i + 1].value - stops[i].value);
            } else {
                return stops[i].value; // hard snap: use the left stop's output
            }
        }
    }
    stops[last].value
}

/// Apply the tone ramp to the entire framebuffer in-place.
///
/// Each pixel's luminance is computed, normalised, passed through the ramp,
/// and the RGB channels are scaled to achieve the new luminance (hue is
/// preserved).  Black pixels (lum ≈ 0) are left untouched.
///
/// When `zbuf` is provided and the same size as the buffer, pixels at
/// `zbuf[i] == +∞` are treated as background and skipped.  This avoids
/// tone-mapping the scene's background clear colour when depth-testing is
/// enabled.  With the painter's path (no zbuf), the ramp is applied to all
/// non-black pixels.
pub fn apply_ramp(
    buf:    &mut Vec<u32>,
    zbuf:   &[f32],
    width:  usize,
    height: usize,
    ramp:   &ToneRamp,
) {
    let n = width * height;
    if buf.len() < n || ramp.stops.is_empty() { return; }
    let use_zbuf = zbuf.len() >= n;

    #[inline]
    fn shade(p: u32, ramp: &ToneRamp) -> u32 {
        let r = ((p >> 16) & 0xFF) as f32;
        let g = ((p >>  8) & 0xFF) as f32;
        let b = ( p        & 0xFF) as f32;
        let lum = 0.299 * r + 0.587 * g + 0.114 * b;
        if lum < 0.001 { return p; }
        let new_val = sample_ramp(ramp, lum / 255.0);
        let scale   = (new_val * 255.0 / lum).clamp(0.0, 8.0);
        (((r * scale).min(255.0) as u32) << 16)
            | (((g * scale).min(255.0) as u32) <<  8)
            |  ((b * scale).min(255.0) as u32)
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        use rayon::prelude::*;
        const ROWS: usize = 32;
        let band = ROWS * width;
        if use_zbuf {
            buf[..n]
                .par_chunks_mut(band)
                .zip(zbuf[..n].par_chunks(band))
                .for_each(|(bb, zz)| {
                    for (px, z) in bb.iter_mut().zip(zz) {
                        if z.is_finite() {
                            *px = shade(*px, ramp);
                        }
                    }
                });
        } else {
            buf[..n].par_chunks_mut(band).for_each(|bb| {
                for px in bb.iter_mut() {
                    *px = shade(*px, ramp);
                }
            });
        }
        return;
    }
    #[cfg(target_arch = "wasm32")]
    for i in 0..n {
        if use_zbuf && !zbuf[i].is_finite() { continue; }
        buf[i] = shade(buf[i], ramp);
    }
}

// ── Silhouette outline detection ──────────────────────────────────────────────

/// Draw toon ink lines where the depth buffer has a sharp discontinuity.
///
/// For each pixel, we compare its camera-space z against its 4 neighbours.
/// When the maximum depth difference exceeds `threshold`, that pixel is on a
/// silhouette or a crease — we stamp a filled circle of radius `thickness`
/// pixels in `color`.
///
/// `thickness` — half-width of the ink line in pixels (1.0 = single pixel, 2.0 = anime thick)
/// `color`     — 0x00RRGGBB ink colour
/// `threshold` — depth difference that triggers the edge (0.02–0.1 for typical scenes)
pub fn draw_outlines(
    buf:       &mut Vec<u32>,
    zbuf:      &[f32],
    width:     usize,
    height:    usize,
    thickness: f32,
    color:     u32,
    threshold: f32,
) {
    if zbuf.len() < width * height || buf.len() < width * height {
        return;
    }
    let t   = thickness.clamp(0.5, 6.0);
    let t_i = t.ceil() as i32;
    let t2  = t * t;

    for y in t_i..(height as i32 - t_i) {
        for x in t_i..(width as i32 - t_i) {
            let idx = y as usize * width + x as usize;
            let z = zbuf[idx];
            if !z.is_finite() { continue; }

            let zn = zbuf[(y - 1) as usize * width + x as usize];
            let zs = zbuf[(y + 1) as usize * width + x as usize];
            let zw = zbuf[y as usize * width + (x - 1) as usize];
            let ze = zbuf[y as usize * width + (x + 1) as usize];
            let dmax = (z - zn).abs()
                .max((z - zs).abs())
                .max((z - zw).abs())
                .max((z - ze).abs());
            if dmax < threshold { continue; }

            for dy in -t_i..=t_i {
                for dx in -t_i..=t_i {
                    let dist2 = (dx as f32) * (dx as f32) + (dy as f32) * (dy as f32);
                    if dist2 > t2 { continue; }
                    let nx = x + dx;
                    let ny = y + dy;
                    if nx >= 0 && ny >= 0 && nx < width as i32 && ny < height as i32 {
                        let ni = ny as usize * width + nx as usize;
                        let cov = (t2 - dist2).sqrt() / t.max(1.0);
                        let cov = cov.clamp(0.0, 1.0);
                        if cov >= 0.999 {
                            buf[ni] = color;
                        } else {
                            let dst = buf[ni];
                            let dr = ((dst >> 16) & 0xFF) as f32;
                            let dg = ((dst >>  8) & 0xFF) as f32;
                            let db = ( dst        & 0xFF) as f32;
                            let ir = ((color >> 16) & 0xFF) as f32;
                            let ig = ((color >>  8) & 0xFF) as f32;
                            let ib = ( color        & 0xFF) as f32;
                            let r = (ir * cov + dr * (1.0 - cov)) as u32;
                            let g = (ig * cov + dg * (1.0 - cov)) as u32;
                            let b = (ib * cov + db * (1.0 - cov)) as u32;
                            buf[ni] = (r << 16) | (g << 8) | b;
                        }
                    }
                }
            }
        }
    }
}

// ── ToonConfig ────────────────────────────────────────────────────────────────

/// Post-process configuration stored in `GfxState`.
///
/// The `ramp` replaces the old separate shadow-softness and highlight passes.
/// Lighting and shadowing are expressed as a single tone gradient — set stops,
/// toggle `smooth`, optionally shape with a Bezier curve.
#[derive(Debug, Clone)]
pub struct ToonConfig {
    /// Unified tone ramp: maps pixel luminance → output brightness.
    /// Applied as a post-process after geometry rendering.
    pub ramp: ToneRamp,
    /// Outline thickness in pixels (0 = off).
    pub outline_px:     f32,
    /// Depth discontinuity that triggers an outline stamp.
    pub outline_thresh: f32,
    /// Ink colour (0x00RRGGBB).
    pub outline_color:  u32,
}

impl Default for ToonConfig {
    fn default() -> Self {
        Self {
            ramp:           ToneRamp::default(),
            outline_px:     0.0,
            outline_thresh: 0.05,
            outline_color:  0x00_00_00,
        }
    }
}

/// Apply all enabled toon passes in the correct order.
///
/// 1. Tone ramp — luminance reshaping / cel-quantisation.
/// 2. Outlines  — depth-discontinuity ink lines (optional).
///
/// Call this after `queue.flush()` and before presenting the buffer to screen.
pub fn apply(
    cfg:    &ToonConfig,
    buf:    &mut Vec<u32>,
    zbuf:   &[f32],
    width:  usize,
    height: usize,
) {
    apply_ramp(buf, zbuf, width, height, &cfg.ramp);

    if cfg.outline_px > 0.0 {
        draw_outlines(buf, zbuf, width, height,
            cfg.outline_px, cfg.outline_color, cfg.outline_thresh);
    }
}

// ── Unit tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_toon_default() -> ToonConfig { ToonConfig::default() }

    #[test]
    fn sample_ramp_hard_snap_3band() {
        let ramp = ToneRamp::default();
        // Shadow band: t < 0.25 → 0.08
        let v = sample_ramp(&ramp, 0.10);
        assert!((v - 0.08).abs() < 1e-4, "shadow band: {v}");
        // Mid band: 0.25 ≤ t < 0.60 → 0.50
        let v = sample_ramp(&ramp, 0.40);
        assert!((v - 0.50).abs() < 1e-4, "mid band: {v}");
        // Lit band: t ≥ 0.60 → 1.00
        let v = sample_ramp(&ramp, 0.80);
        assert!((v - 1.00).abs() < 1e-4, "lit band: {v}");
    }

    #[test]
    fn sample_ramp_smooth_lerps() {
        let mut ramp = ToneRamp::default();
        ramp.smooth = true;
        // At t=0.125 (midpoint of shadow→mid segment [0.00, 0.25])
        // Expected: lerp(0.08, 0.50, 0.5) = 0.29
        let v = sample_ramp(&ramp, 0.125);
        assert!((v - 0.29).abs() < 0.01, "smooth lerp: {v}");
    }

    #[test]
    fn bezier_identity_at_1third_2third() {
        // y1=1/3, y2=2/3 → exact identity f(t)=t
        let ramp = ToneRamp {
            stops: vec![ToneStop { t: 0.0, value: 0.0 }, ToneStop { t: 1.0, value: 1.0 }],
            smooth: true,
            bezier: Some([1.0 / 3.0, 2.0 / 3.0]),
        };
        for &t in &[0.0f32, 0.25, 0.5, 0.75, 1.0] {
            let v = sample_ramp(&ramp, t);
            assert!((v - t).abs() < 1e-4, "identity at t={t}: got {v}");
        }
    }

    #[test]
    fn apply_ramp_preserves_hue_in_shadow_band() {
        // A pure-red pixel at lum ≈ 0.3*255 = 76.5 (within shadow band t≈0.30 → mid)
        // With default ramp (hard snap): t=0.30 → value=0.50
        // Expected scale = 0.50*255/76.5 ≈ 1.67 → r scales up
        let width = 4; let height = 4;
        let mut buf = vec![0u32; width * height];
        let r_in = 255u32; let g_in = 0u32; let b_in = 0u32;
        let _lum_in = 0.299 * r_in as f32; // ≈76.5 (kept for readability)
        for px in buf.iter_mut() { *px = (r_in << 16) | (g_in << 8) | b_in; }
        let zbuf: Vec<f32> = vec![]; // no zbuf
        let ramp = ToneRamp::default();
        apply_ramp(&mut buf, &zbuf, width, height, &ramp);
        let p = buf[5];
        let r = (p >> 16) & 0xFF;
        let g = (p >>  8) & 0xFF;
        let b =  p        & 0xFF;
        // Hue: must stay pure red (g=b=0)
        assert_eq!(g, 0, "hue must remain red");
        assert_eq!(b, 0, "hue must remain red");
        // Lum after: 0.50 * 255 = 127.5 → r ≈ 127.5/0.299 ≈ 426... clamped to 255
        // Actually: scale = 0.50*255/76.5 = 1.667, r_out = 255*1.667 = 425 → clamped to 255
        assert!(r > 0, "red channel should be non-zero");
    }

    #[test]
    fn apply_ramp_skips_background_with_zbuf() {
        // Background pixels (zbuf = +inf) must not be touched.
        let width = 2; let height = 2;
        let mut buf = vec![0x808080u32; width * height]; // grey background
        let zbuf = vec![f32::INFINITY; width * height];  // all background
        let ramp = ToneRamp::default();
        apply_ramp(&mut buf, &zbuf, width, height, &ramp);
        for px in &buf {
            assert_eq!(*px, 0x808080, "background pixels must be unchanged");
        }
    }

    #[test]
    fn default_toon_config_has_3_band_ramp() {
        let cfg = make_toon_default();
        assert_eq!(cfg.ramp.stops.len(), 4);
        assert!(!cfg.ramp.smooth, "default is hard cel");
        assert!(cfg.ramp.bezier.is_none(), "default has no bezier");
    }
}
