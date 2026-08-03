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
    pub t: f32,
    pub value: f32,
}

/// Maps pixel luminance through a gradient of brightness stops.
///
/// `stops`  — sorted by `t` ascending.
/// `smooth` — `false` = hard-snap to the left stop (cel shade);
///            `true`  = linear-interpolate between stops (soft gradient).
/// `bezier` — optional `[y1, y2]` cubic Bézier remap applied before lookup.
///            `None` = identity (no remap).
/// `soft`   — band-edge softness [0..1]: fraction of each stop gap that
///            transitions smoothly (smoothstep) across the boundary instead of
///            hard-snapping. 0 = crisp cel edges; ~0.3 = Wind Waker-style soft
///            shadow boundaries. Only used when `smooth` is false.
/// `sheen`  — highlight preservation [0..1]: bright pixels keep their smooth
///            gradient instead of being quantised, so specular/rim reads as a
///            clean sheen rather than scratchy posterised highlights.
#[derive(Debug, Clone)]
pub struct ToneRamp {
    pub stops: Vec<ToneStop>,
    pub smooth: bool,
    pub bezier: Option<[f32; 2]>,
    pub soft: f32,
    pub sheen: f32,
}

impl Default for ToneRamp {
    /// 3-band cel-shade matching the old hardcoded thresholds:
    ///   shadow  t < 0.25 → 0.08
    ///   mid     t < 0.60 → 0.50
    ///   lit     t ≥ 0.60 → 1.00
    /// with softly blended band edges and smooth (unquantised) highlights.
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
            soft: 0.32,
            sheen: 0.65,
        }
    }
}

#[inline]
fn smoothstep01(f: f32) -> f32 {
    let f = f.clamp(0.0, 1.0);
    f * f * (3.0 - 2.0 * f)
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
    if stops.is_empty() {
        return t;
    }

    // Before first stop → first value
    if t <= stops[0].t {
        return stops[0].value;
    }

    let last = stops.len() - 1;
    // At or past last stop → last value
    if t >= stops[last].t {
        return stops[last].value;
    }

    // Find the surrounding pair
    for i in 0..last {
        if t < stops[i + 1].t {
            if ramp.smooth {
                let span = stops[i + 1].t - stops[i].t;
                let f = if span > 1e-6 {
                    (t - stops[i].t) / span
                } else {
                    1.0
                };
                return stops[i].value + f * (stops[i + 1].value - stops[i].value);
            }
            let v = stops[i].value; // hard snap: the left stop's output …
            if ramp.soft <= 0.0 {
                return v;
            }
            // … with a smoothstep transition zone straddling each boundary.
            // Zone half-width = soft/2 × the smaller adjacent gap, so both
            // sides of a boundary agree and zones can never overlap.
            let gap_u = stops[i + 1].t - stops[i].t;
            let gap_u2 = if i + 2 <= last {
                stops[i + 2].t - stops[i + 1].t
            } else {
                gap_u
            };
            let hw_u = 0.5 * ramp.soft * gap_u.min(gap_u2);
            if hw_u > 1e-6 && t > stops[i + 1].t - hw_u {
                let s = smoothstep01((t - (stops[i + 1].t - hw_u)) / (2.0 * hw_u));
                return v + s * (stops[i + 1].value - v);
            }
            if i > 0 {
                let gap_l = stops[i].t - stops[i - 1].t;
                let hw_l = 0.5 * ramp.soft * gap_l.min(gap_u);
                if hw_l > 1e-6 && t < stops[i].t + hw_l {
                    let s = smoothstep01((t - (stops[i].t - hw_l)) / (2.0 * hw_l));
                    return stops[i - 1].value + s * (v - stops[i - 1].value);
                }
            }
            return v;
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
/// Pixels tagged [`crate::gfx::UNLIT`] (line/text ink, see `raster.rs`) are
/// left exact — the tag is stripped instead of shaded, so vector lines and
/// vector text render as flat, un-quantised colour and never cel-band-snap
/// or flicker against the lit triangles around them.
pub fn apply_ramp(buf: &mut [u32], width: usize, height: usize, ramp: &ToneRamp) {
    let n = width * height;
    if width == 0 || height == 0 || buf.len() < n {
        return;
    }

    #[inline]
    fn shade(p: u32, ramp: &ToneRamp) -> u32 {
        if p & crate::gfx::UNLIT != 0 {
            return p & crate::gfx::RGB_MASK;
        }
        if ramp.stops.is_empty() {
            return p;
        }
        let r = ((p >> 16) & 0xFF) as f32;
        let g = ((p >> 8) & 0xFF) as f32;
        let b = (p & 0xFF) as f32;
        let lum = 0.299 * r + 0.587 * g + 0.114 * b;
        // Near-black stays black: quantising ~1-2 lum pixels used to multiply
        // them ×8 into bright speckles inside shadows (the "scratchy" glitter).
        if lum < 1.5 {
            return p;
        }
        let t = lum / 255.0;
        let mut new_val = sample_ramp(ramp, t);
        // Sheen: blend bright pixels back toward their true luminance so
        // specular / fresnel-rim gradients stay smooth instead of banding.
        if ramp.sheen > 0.0 {
            let k = smoothstep01((t - 0.72) / 0.20) * ramp.sheen;
            new_val += (t - new_val) * k;
        }
        // Cap the brightening: dark pixels get a tight cap so shadow noise
        // can't blow out into scratches; everything else a moderate one.
        let maxs = if lum < 14.0 { 2.2 } else { 4.0 };
        let scale = (new_val * 255.0 / lum).clamp(0.0, maxs);
        (((r * scale).min(255.0) as u32) << 16)
            | (((g * scale).min(255.0) as u32) << 8)
            | ((b * scale).min(255.0) as u32)
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        use rayon::prelude::*;
        const ROWS: usize = 32;
        let band = ROWS * width;
        buf[..n].par_chunks_mut(band).for_each(|bb| {
            for px in bb.iter_mut() {
                *px = shade(*px, ramp);
            }
        });
    }

    #[cfg(target_arch = "wasm32")]
    for px in buf[..n].iter_mut() {
        *px = shade(*px, ramp);
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
    buf: &mut [u32],
    zbuf: &[f32],
    width: usize,
    height: usize,
    thickness: f32,
    color: u32,
    threshold: f32,
) {
    if zbuf.len() < width * height || buf.len() < width * height {
        return;
    }
    let t = thickness.clamp(0.5, 6.0);
    let t_i = t.ceil() as i32;
    let t2 = t * t;

    for y in t_i..(height as i32 - t_i) {
        for x in t_i..(width as i32 - t_i) {
            let idx = y as usize * width + x as usize;
            let z = zbuf[idx];
            if !z.is_finite() {
                continue;
            }

            let zn = zbuf[(y - 1) as usize * width + x as usize];
            let zs = zbuf[(y + 1) as usize * width + x as usize];
            let zw = zbuf[y as usize * width + (x - 1) as usize];
            let ze = zbuf[y as usize * width + (x + 1) as usize];
            let dmax = (z - zn)
                .abs()
                .max((z - zs).abs())
                .max((z - zw).abs())
                .max((z - ze).abs());
            if dmax < threshold {
                continue;
            }

            for dy in -t_i..=t_i {
                for dx in -t_i..=t_i {
                    let dist2 = (dx as f32) * (dx as f32) + (dy as f32) * (dy as f32);
                    if dist2 > t2 {
                        continue;
                    }
                    let nx = x + dx;
                    let ny = y + dy;
                    if nx >= 0 && ny >= 0 && nx < width as i32 && ny < height as i32 {
                        let ni = ny as usize * width + nx as usize;
                        if buf[ni] & crate::gfx::UNLIT != 0 {
                            continue;
                        }
                        let cov = (t2 - dist2).sqrt() / t.max(1.0);
                        let cov = cov.clamp(0.0, 1.0);
                        if cov >= 0.999 {
                            buf[ni] = color | crate::gfx::UNLIT;
                        } else {
                            let dst = buf[ni];
                            let dr = ((dst >> 16) & 0xFF) as f32;
                            let dg = ((dst >> 8) & 0xFF) as f32;
                            let db = (dst & 0xFF) as f32;
                            let ir = ((color >> 16) & 0xFF) as f32;
                            let ig = ((color >> 8) & 0xFF) as f32;
                            let ib = (color & 0xFF) as f32;
                            let r = (ir * cov + dr * (1.0 - cov)) as u32;
                            let g = (ig * cov + dg * (1.0 - cov)) as u32;
                            let b = (ib * cov + db * (1.0 - cov)) as u32;
                            buf[ni] = (r << 16) | (g << 8) | b | crate::gfx::UNLIT;
                        }
                    }
                }
            }
        }
    }
}

// ── Screen-space ambient occlusion ────────────────────────────────────────────

/// Cheap SSAO from the z-buffer: contact shading in corners and under objects.
///
/// Computed on a half-resolution grid (8 ring taps per cell), box-smoothed,
/// then multiplied into the framebuffer with bilinear upsampling — soft,
/// wide-radius occlusion rather than per-pixel grain, which suits the toon
/// look ("smooth shadows on surfaces"). Pixels tagged [`crate::gfx::UNLIT`]
/// (ink lines, vector text) are left untouched.
///
/// `strength`  — max darkening [0..1] (0 disables)
/// `radius_px` — tap ring radius in (full-res) pixels
/// `zrange`    — camera-space depth window an occluder counts within
pub fn apply_ssao(
    buf: &mut [u32],
    zbuf: &[f32],
    width: usize,
    height: usize,
    strength: f32,
    radius_px: f32,
    zrange: f32,
) {
    let n = width * height;
    if width < 4 || height < 4 || strength <= 0.0 || buf.len() < n || zbuf.len() < n {
        return;
    }
    let zrange = zrange.max(1e-3);
    let rad = radius_px.max(1.0);
    let hw2 = width.div_ceil(2);
    let hh2 = height.div_ceil(2);
    const TAPS: [(f32, f32); 8] = [
        (1.0, 0.0),
        (0.707, 0.707),
        (0.0, 1.0),
        (-0.707, 0.707),
        (-1.0, 0.0),
        (-0.707, -0.707),
        (0.0, -1.0),
        (0.707, -0.707),
    ];

    // 1. Half-res occlusion estimate.
    let mut ao = vec![1.0f32; hw2 * hh2];
    let occ_row = |hy: usize, row: &mut [f32]| {
        let y = (hy * 2).min(height - 1);
        for (hx, cell) in row.iter_mut().enumerate() {
            let x = (hx * 2).min(width - 1);
            let z = zbuf[y * width + x];
            if !z.is_finite() {
                continue; // background: no occlusion
            }
            let bias = 0.35 + z * 0.002; // self-occlusion guard, scales with depth
            let mut occ = 0.0f32;
            for (tx, ty) in TAPS {
                let sx = (x as f32 + tx * rad) as i32;
                let sy = (y as f32 + ty * rad) as i32;
                if sx < 0 || sy < 0 || sx >= width as i32 || sy >= height as i32 {
                    continue;
                }
                let zt = zbuf[sy as usize * width + sx as usize];
                if !zt.is_finite() {
                    continue;
                }
                let dz = z - zt; // positive → neighbour nearer the camera
                if dz > bias && dz < zrange {
                    occ += 1.0 - dz / zrange; // fade with depth separation
                }
            }
            *cell = (1.0 - strength * (occ / TAPS.len() as f32) * 1.6).clamp(0.0, 1.0);
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        use rayon::prelude::*;
        ao.par_chunks_mut(hw2)
            .enumerate()
            .for_each(|(hy, row)| occ_row(hy, row));
    }
    #[cfg(target_arch = "wasm32")]
    for (hy, row) in ao.chunks_mut(hw2).enumerate() {
        occ_row(hy, row);
    }

    // 2. 3×3 box smooth on the half-res grid (kills tap-pattern noise).
    let mut sm = vec![1.0f32; hw2 * hh2];
    let smooth_row = |hy: usize, row: &mut [f32]| {
        for (hx, out) in row.iter_mut().enumerate() {
            let mut acc = 0.0f32;
            let mut cnt = 0.0f32;
            for dy in -1i32..=1 {
                let yy = hy as i32 + dy;
                if yy < 0 || yy >= hh2 as i32 {
                    continue;
                }
                for dx in -1i32..=1 {
                    let xx = hx as i32 + dx;
                    if xx < 0 || xx >= hw2 as i32 {
                        continue;
                    }
                    acc += ao[yy as usize * hw2 + xx as usize];
                    cnt += 1.0;
                }
            }
            *out = acc / cnt.max(1.0);
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        use rayon::prelude::*;
        sm.par_chunks_mut(hw2)
            .enumerate()
            .for_each(|(hy, row)| smooth_row(hy, row));
    }
    #[cfg(target_arch = "wasm32")]
    for (hy, row) in sm.chunks_mut(hw2).enumerate() {
        smooth_row(hy, row);
    }

    // 3. Bilinear upsample + multiply into the framebuffer (UNLIT skipped).
    let mul_row = |y: usize, row: &mut [u32]| {
        let gy = y as f32 * 0.5;
        let hy0 = (gy as usize).min(hh2 - 1);
        let hy1 = (hy0 + 1).min(hh2 - 1);
        let fy = gy - hy0 as f32;
        for (x, px) in row.iter_mut().enumerate() {
            let p = *px;
            if p & crate::gfx::UNLIT != 0 {
                continue; // ink/text stays exact
            }
            let gx = x as f32 * 0.5;
            let hx0 = (gx as usize).min(hw2 - 1);
            let hx1 = (hx0 + 1).min(hw2 - 1);
            let fx = gx - hx0 as f32;
            let a = sm[hy0 * hw2 + hx0] * (1.0 - fx) + sm[hy0 * hw2 + hx1] * fx;
            let b = sm[hy1 * hw2 + hx0] * (1.0 - fx) + sm[hy1 * hw2 + hx1] * fx;
            let m = a * (1.0 - fy) + b * fy;
            if m >= 0.995 {
                continue;
            }
            let r = (((p >> 16) & 0xFF) as f32 * m) as u32;
            let g = (((p >> 8) & 0xFF) as f32 * m) as u32;
            let bl = ((p & 0xFF) as f32 * m) as u32;
            *px = (r << 16) | (g << 8) | bl;
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        use rayon::prelude::*;
        buf[..n]
            .par_chunks_mut(width)
            .enumerate()
            .for_each(|(y, row)| mul_row(y, row));
    }
    #[cfg(target_arch = "wasm32")]
    for (y, row) in buf[..n].chunks_mut(width).enumerate() {
        mul_row(y, row);
    }
}

// ── Bloom (soft HDR-style glow) ───────────────────────────────────────────────

/// Threshold-bloom: bright pixels (rim sheen, emissive cores, additive FX)
/// bleed a soft quarter-resolution glow over the frame — the "HDR material"
/// feel for vector/toon art. Runs after the tone ramp (tags already
/// stripped); saturating-add composite (fast additive path).
///
/// `strength` — glow intensity [0..~1.5] (0 disables)
/// `thresh`   — luminance threshold [0..1] above which pixels glow
pub fn apply_bloom(buf: &mut [u32], width: usize, height: usize, strength: f32, thresh: f32) {
    let n = width * height;
    if width < 8 || height < 8 || strength <= 0.0 || buf.len() < n {
        return;
    }
    let bw = width.div_ceil(4);
    let bh = height.div_ceil(4);
    let bn = bw * bh;
    let thresh = thresh.clamp(0.0, 0.99);
    let inv_span = 1.0 / (1.0 - thresh);

    // 1. Downsample 4×4 (2×2 taps) with luminance threshold.
    let mut sr = vec![0.0f32; bn];
    let mut sg = vec![0.0f32; bn];
    let mut sb = vec![0.0f32; bn];
    for by in 0..bh {
        for bx in 0..bw {
            let mut r = 0.0f32;
            let mut g = 0.0f32;
            let mut b = 0.0f32;
            for (ox, oy) in [(1usize, 1usize), (3, 1), (1, 3), (3, 3)] {
                let x = (bx * 4 + ox).min(width - 1);
                let y = (by * 4 + oy).min(height - 1);
                let p = buf[y * width + x];
                r += ((p >> 16) & 0xFF) as f32;
                g += ((p >> 8) & 0xFF) as f32;
                b += (p & 0xFF) as f32;
            }
            r *= 0.25;
            g *= 0.25;
            b *= 0.25;
            let lum = (0.299 * r + 0.587 * g + 0.114 * b) / 255.0;
            let keep = ((lum - thresh) * inv_span).clamp(0.0, 1.0);
            let keep = keep * keep; // soft knee
            let i = by * bw + bx;
            sr[i] = r * keep;
            sg[i] = g * keep;
            sb[i] = b * keep;
        }
    }

    // 2. Two separable box-blur passes (radius 2) on the small buffers.
    let blur_small = |src: &mut Vec<f32>| {
        let mut tmp = vec![0.0f32; bn];
        for _ in 0..2 {
            for y in 0..bh {
                let row = y * bw;
                for x in 0..bw {
                    let mut acc = 0.0f32;
                    for k in -2i32..=2 {
                        let xx = (x as i32 + k).clamp(0, bw as i32 - 1) as usize;
                        acc += src[row + xx];
                    }
                    tmp[row + x] = acc * 0.2;
                }
            }
            for x in 0..bw {
                for y in 0..bh {
                    let mut acc = 0.0f32;
                    for k in -2i32..=2 {
                        let yy = (y as i32 + k).clamp(0, bh as i32 - 1) as usize;
                        acc += tmp[yy * bw + x];
                    }
                    src[y * bw + x] = acc * 0.2;
                }
            }
        }
    };
    blur_small(&mut sr);
    blur_small(&mut sg);
    blur_small(&mut sb);

    // 3. Bilinear upsample, scale, saturating-add into the frame.
    let add_row = |y: usize, row: &mut [u32]| {
        let gy = y as f32 * 0.25;
        let by0 = (gy as usize).min(bh - 1);
        let by1 = (by0 + 1).min(bh - 1);
        let fy = gy - by0 as f32;
        for (x, px) in row.iter_mut().enumerate() {
            let gx = x as f32 * 0.25;
            let bx0 = (gx as usize).min(bw - 1);
            let bx1 = (bx0 + 1).min(bw - 1);
            let fx = gx - bx0 as f32;
            let i00 = by0 * bw + bx0;
            let i10 = by0 * bw + bx1;
            let i01 = by1 * bw + bx0;
            let i11 = by1 * bw + bx1;
            let bl = |s: &[f32]| -> f32 {
                let a = s[i00] * (1.0 - fx) + s[i10] * fx;
                let b = s[i01] * (1.0 - fx) + s[i11] * fx;
                (a * (1.0 - fy) + b * fy) * strength
            };
            let ar = bl(&sr) as u32;
            let ag = bl(&sg) as u32;
            let ab = bl(&sb) as u32;
            if ar | ag | ab == 0 {
                continue;
            }
            let p = *px;
            let r = (((p >> 16) & 0xFF) + ar).min(255);
            let g = (((p >> 8) & 0xFF) + ag).min(255);
            let b = ((p & 0xFF) + ab).min(255);
            *px = (r << 16) | (g << 8) | b;
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        use rayon::prelude::*;
        buf[..n]
            .par_chunks_mut(width)
            .enumerate()
            .for_each(|(y, row)| add_row(y, row));
    }
    #[cfg(target_arch = "wasm32")]
    for (y, row) in buf[..n].chunks_mut(width).enumerate() {
        add_row(y, row);
    }
}

// ── Anti-aliasing (FXAA-lite) ─────────────────────────────────────────────────

/// Luma-contrast edge anti-aliasing: pixels on a high-contrast edge blend a
/// little toward their 4-neighbour average — softens polygon stair-steps and
/// ink-outline jaggies while leaving flat fills untouched. Runs last (after
/// the ramp has stripped UNLIT tags) from a snapshot so blends never cascade.
pub fn apply_fxaa(buf: &mut [u32], width: usize, height: usize) {
    let n = width * height;
    if width < 3 || height < 3 || buf.len() < n {
        return;
    }
    let snap = buf[..n].to_vec();
    #[inline]
    fn luma(p: u32) -> i32 {
        ((((p >> 16) & 0xFF) * 77 + ((p >> 8) & 0xFF) * 150 + (p & 0xFF) * 29) >> 8) as i32
    }
    let aa_row = |y: usize, row: &mut [u32]| {
        if y == 0 || y >= height - 1 {
            return;
        }
        let base = y * width;
        for x in 1..width - 1 {
            let i = base + x;
            let c = snap[i];
            let l = luma(c);
            let pn = snap[i - width];
            let ps = snap[i + width];
            let pw = snap[i - 1];
            let pe = snap[i + 1];
            let d = (l - luma(pn))
                .abs()
                .max((l - luma(ps)).abs())
                .max((l - luma(pw)).abs())
                .max((l - luma(pe)).abs());
            if d < 24 {
                continue; // flat area — untouched
            }
            // Blend weight rises with contrast: 25%..45%.
            let k = (0.25 + ((d - 24) as f32 / 90.0).clamp(0.0, 1.0) * 0.20).min(0.45);
            let ki = (k * 256.0) as u32;
            let inv = 256 - ki;
            let avg = |sh: u32, m: u32| -> u32 {
                let s = ((pn >> sh) & m) + ((ps >> sh) & m) + ((pw >> sh) & m) + ((pe >> sh) & m);
                s / 4
            };
            let r = (((c >> 16) & 0xFF) * inv + avg(16, 0xFF) * ki) >> 8;
            let g = (((c >> 8) & 0xFF) * inv + avg(8, 0xFF) * ki) >> 8;
            let b = ((c & 0xFF) * inv + avg(0, 0xFF) * ki) >> 8;
            row[x] = (r << 16) | (g << 8) | b;
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        use rayon::prelude::*;
        buf[..n]
            .par_chunks_mut(width)
            .enumerate()
            .for_each(|(y, row)| aa_row(y, row));
    }
    #[cfg(target_arch = "wasm32")]
    for (y, row) in buf[..n].chunks_mut(width).enumerate() {
        aa_row(y, row);
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
    pub outline_px: f32,
    /// Depth discontinuity that triggers an outline stamp.
    pub outline_thresh: f32,
    /// Ink colour (0x00RRGGBB).
    pub outline_color: u32,
    /// SSAO darkening strength [0..1] (0 = off; needs the z-buffer).
    pub ao_strength: f32,
    /// SSAO tap-ring radius in pixels.
    pub ao_radius: f32,
    /// SSAO camera-space depth window an occluder counts within.
    pub ao_range: f32,
    /// Bloom glow strength (0 = off).
    pub bloom_strength: f32,
    /// Bloom luminance threshold [0..1].
    pub bloom_thresh: f32,
    /// Edge anti-aliasing (FXAA-lite) toggle.
    pub fxaa: bool,
}

impl Default for ToonConfig {
    fn default() -> Self {
        Self {
            ramp: ToneRamp::default(),
            outline_px: 0.0,
            outline_thresh: 0.05,
            outline_color: 0x00_00_00,
            ao_strength: 0.0,
            ao_radius: 6.0,
            ao_range: 12.0,
            bloom_strength: 0.0,
            bloom_thresh: 0.74,
            fxaa: false,
        }
    }
}

/// Apply all enabled toon passes in the correct order.
///
/// 1. SSAO      — depth-buffer contact shading (darkens diffuse; skips UNLIT ink).
/// 2. Outlines  — depth-discontinuity ink lines (optional), tagged unlit.
/// 3. Tone ramp — luminance reshaping / cel-quantisation; also the pass that
///    strips the [`crate::gfx::UNLIT`] tag, so it runs after the tag-aware passes.
/// 4. Bloom     — soft glow from bright pixels ("HDR material" sheen).
/// 5. FXAA      — edge anti-aliasing, last so it also softens ink lines.
///
/// Call this after `queue.flush()` and before presenting the buffer to screen.
pub fn apply(cfg: &ToonConfig, buf: &mut [u32], zbuf: &[f32], width: usize, height: usize) {
    let n = width * height;
    if width == 0 || height == 0 || buf.len() < n {
        return;
    }
    if cfg.ao_strength > 0.0 {
        apply_ssao(
            buf,
            zbuf,
            width,
            height,
            cfg.ao_strength,
            cfg.ao_radius,
            cfg.ao_range,
        );
    }
    if cfg.outline_px > 0.0 {
        draw_outlines(
            buf,
            zbuf,
            width,
            height,
            cfg.outline_px,
            cfg.outline_color,
            cfg.outline_thresh,
        );
    }

    apply_ramp(buf, width, height, &cfg.ramp);

    if cfg.bloom_strength > 0.0 {
        apply_bloom(buf, width, height, cfg.bloom_strength, cfg.bloom_thresh);
    }
    if cfg.fxaa {
        apply_fxaa(buf, width, height);
    }
}

// ── Unit tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_toon_default() -> ToonConfig {
        ToonConfig::default()
    }

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
            stops: vec![
                ToneStop { t: 0.0, value: 0.0 },
                ToneStop { t: 1.0, value: 1.0 },
            ],
            smooth: true,
            bezier: Some([1.0 / 3.0, 2.0 / 3.0]),
            soft: 0.0,
            sheen: 0.0,
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
        let width = 4;
        let height = 4;
        let mut buf = vec![0u32; width * height];
        let r_in = 255u32;
        let g_in = 0u32;
        let b_in = 0u32;
        let _lum_in = 0.299 * r_in as f32; // ≈76.5 (kept for readability)
        for px in buf.iter_mut() {
            *px = (r_in << 16) | (g_in << 8) | b_in;
        }
        let ramp = ToneRamp::default();
        apply_ramp(&mut buf, width, height, &ramp);
        let p = buf[5];
        let r = (p >> 16) & 0xFF;
        let g = (p >> 8) & 0xFF;
        let b = p & 0xFF;
        // Hue: must stay pure red (g=b=0)
        assert_eq!(g, 0, "hue must remain red");
        assert_eq!(b, 0, "hue must remain red");
        // Lum after: 0.50 * 255 = 127.5 → r ≈ 127.5/0.299 ≈ 426... clamped to 255
        // Actually: scale = 0.50*255/76.5 = 1.667, r_out = 255*1.667 = 425 → clamped to 255
        assert!(r > 0, "red channel should be non-zero");
    }

    #[test]
    fn apply_ramp_background_now_processed() {
        let width = 2;
        let height = 2;
        let mut buf = vec![0x808080u32; width * height]; // grey bg, lum=128
        let ramp = ToneRamp::default();
        apply_ramp(&mut buf, width, height, &ramp);
        // Grey (128,128,128) has t≈0.502 → mid band (0.50) → output ≈127
        for px in &buf {
            let r = (*px >> 16) & 0xFF;
            let g = (*px >> 8) & 0xFF;
            let b = *px & 0xFF;
            assert!(r < 0x81, "red should be cel-adjusted, was {r:#04x}");
            assert!(g < 0x81, "green should be cel-adjusted, was {g:#04x}");
            assert!(b < 0x81, "blue should be cel-adjusted, was {b:#04x}");
        }
    }

    #[test]
    fn apply_ramp_skips_and_strips_unlit() {
        let width = 2;
        let height = 2;
        let mut buf = vec![0x0100FF00u32; width * height]; // unlit green, tagged
        let ramp = ToneRamp::default();
        apply_ramp(&mut buf, width, height, &ramp);
        for px in &buf {
            assert_eq!(*px, 0x0000FF00, "unlit pixel must be stripped, not shaded");
        }
    }

    #[test]
    fn apply_ramp_strips_when_stops_empty() {
        let width = 2;
        let height = 2;
        let mut buf = vec![0x0100FF00u32; width * height];
        let ramp = ToneRamp {
            stops: vec![],
            smooth: false,
            bezier: None,
            soft: 0.0,
            sheen: 0.0,
        };
        apply_ramp(&mut buf, width, height, &ramp);
        for px in &buf {
            assert_eq!(*px, 0x0000FF00, "tag must strip even with no ramp stops");
        }
    }

    #[test]
    fn outlines_skip_unlit_and_write_tagged_ink() {
        let width = 8;
        let height = 8;
        let mut buf = vec![0u32; width * height];
        buf[4 * width + 4] = crate::gfx::UNLIT | 0x00FF00FF; // unlit ink pixel
        let mut zbuf = vec![1.0f32; width * height];
        zbuf[4 * width + 4] = 5.0; // sharp discontinuity vs neighbours
        let cfg = ToonConfig {
            outline_px: 2.0,
            outline_thresh: 0.05,
            outline_color: 0x00FFFFFF,
            ..ToonConfig::default()
        };
        apply(&cfg, &mut buf, &zbuf, width, height);
        // The unlit source pixel itself must be untouched by outline stamping,
        // then stripped (not shaded) by the ramp pass.
        assert_eq!(buf[4 * width + 4], 0x00FF00FF);
        // Ink stamped on a neighbouring plain-black pixel must survive the ramp
        // pass exactly (tagged unlit while drawn, stripped not shaded on exit).
        let ni = 4 * width + 5;
        assert!(
            buf[ni] & crate::gfx::RGB_MASK != 0,
            "outline ink must be visible, not cel-quantised to black"
        );
    }

    #[test]
    fn default_toon_config_has_3_band_ramp() {
        let cfg = make_toon_default();
        assert_eq!(cfg.ramp.stops.len(), 4);
        assert!(!cfg.ramp.smooth, "default is hard cel");
        assert!(cfg.ramp.bezier.is_none(), "default has no bezier");
    }
}
