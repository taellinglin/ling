// src/gfx/toon.rs — Screen-space toon post-processing passes.
//
// Three passes, applied after `queue.flush()` in `present()`:
//
//   1. smooth_shadow_edges()  — soften the staircase at cel-shade band
//                               boundaries by blending a narrow penumbra zone
//   2. draw_outlines()        — detect silhouette edges via depth discontinuity
//                               and stamp vector-smooth circular ink dots
//   3. draw_highlights()      — tint the lit band toward a highlight colour
//                               (optional; useful for the anime "shine" look)
//
// All passes are O(w·h) and operate purely on the CPU framebuffer so they
// work identically on native and WASM (which renders to the same `Vec<u32>`
// before blitting to canvas).

// ── 1. Shadow edge smoothing ─────────────────────────────────────────────────

/// Soften the staircase at toon shadow boundaries.
///
/// Scans the framebuffer for pixels that sit in the shadow band
/// (luminance < `dark_thresh`) and are adjacent to a lit pixel.  Those boundary
/// pixels are blended toward the mid-band colour, smoothing the hard step into
/// a narrow anti-aliased gradient.
///
/// `softness` — blend strength [0..1]:
///   0.0  → no effect
///   0.15 → subtle anti-aliased boundary (recommended)
///   0.40 → painterly soft shadow
///
/// Only the 4-connected neighbours are sampled — fast, no allocation.
pub fn smooth_shadow_edges(
    buf:      &mut Vec<u32>,
    width:    usize,
    height:   usize,
    softness: f32,
) {
    if softness <= 0.0 || buf.len() < width * height {
        return;
    }
    let s = softness.clamp(0.0, 1.0);
    let dark_thresh: f32 = 60.0;   // luminance below this = shadow band
    let bright_thresh: f32 = 110.0; // neighbour above this = at the boundary

    // We need a read copy so we don't feed our own writes back.
    // A single-element ring won't do for 2-D; use a row-delayed approach:
    // read from the original slice (buf before changes on this row) and write
    // to a separate output.  Allocate once per frame (much cheaper than the
    // full render).
    let n = width * height;
    let src = buf[..n].to_vec();

    for y in 1..(height as i32 - 1) {
        for x in 1..(width as i32 - 1) {
            let idx = y as usize * width + x as usize;
            let p   = src[idx];
            let r = ((p >> 16) & 0xFF) as f32;
            let g = ((p >> 8)  & 0xFF) as f32;
            let b = (p & 0xFF) as f32;
            let lum = 0.299 * r + 0.587 * g + 0.114 * b;
            if lum >= dark_thresh {
                continue; // not in shadow band
            }
            // Check 4 neighbours for brightness
            let mut max_nlum = 0.0_f32;
            for (dx, dy) in [(-1i32,0),(1,0),(0,-1i32),(0,1)] {
                let ni = (y + dy) as usize * width + (x + dx) as usize;
                let np = src[ni];
                let nr = ((np >> 16) & 0xFF) as f32;
                let ng = ((np >> 8)  & 0xFF) as f32;
                let nb = (np & 0xFF) as f32;
                max_nlum = max_nlum.max(0.299 * nr + 0.587 * ng + 0.114 * nb);
            }
            if max_nlum < bright_thresh {
                continue; // not at a boundary
            }
            // Blend toward mid-grey proportional to softness + contrast
            let contrast = ((max_nlum - lum) / 200.0).clamp(0.0, 1.0);
            let blend = s * contrast;
            // Target: mid band (≈ 0.5 * bright neighbour, to preserve hue)
            let target_lum = (lum + max_nlum) * 0.5;
            let scale = if lum > 0.5 { target_lum / lum } else { 1.0 };
            let nr = (r * (1.0 - blend) + r * scale * blend).clamp(0.0, 255.0) as u32;
            let ng = (g * (1.0 - blend) + g * scale * blend).clamp(0.0, 255.0) as u32;
            let nb = (b * (1.0 - blend) + b * scale * blend).clamp(0.0, 255.0) as u32;
            buf[idx] = (nr << 16) | (ng << 8) | nb;
        }
    }
}

// ── 2. Silhouette outline detection ──────────────────────────────────────────

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
///
/// Uses no heap allocation: the circle stamp is a tight inner loop with
/// integer arithmetic.  Edge pixels are skipped to avoid out-of-bounds access.
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
            if !z.is_finite() { continue; } // background

            // Cross-pattern depth delta — fast and effective for flat/curved surfaces
            let zn = zbuf[(y - 1) as usize * width + x as usize];
            let zs = zbuf[(y + 1) as usize * width + x as usize];
            let zw = zbuf[y as usize * width + (x - 1) as usize];
            let ze = zbuf[y as usize * width + (x + 1) as usize];
            let dmax = (z - zn).abs()
                .max((z - zs).abs())
                .max((z - zw).abs())
                .max((z - ze).abs());
            if dmax < threshold { continue; }

            // Stamp a filled circle — the "vector-smooth" part: a circle has no
            // staircase, so the ink line appears smooth regardless of edge angle.
            for dy in -t_i..=t_i {
                for dx in -t_i..=t_i {
                    let dist2 = (dx as f32) * (dx as f32) + (dy as f32) * (dy as f32);
                    if dist2 > t2 { continue; }
                    let nx = x + dx;
                    let ny = y + dy;
                    if nx >= 0 && ny >= 0 && nx < width as i32 && ny < height as i32 {
                        let ni = ny as usize * width + nx as usize;
                        // Anti-alias: blend ink by coverage at the circle boundary
                        let cov = (t2 - dist2).sqrt() / t.max(1.0);
                        let cov = cov.clamp(0.0, 1.0);
                        if cov >= 0.999 {
                            buf[ni] = color;
                        } else {
                            // Soft edge: blend ink over existing pixel
                            let dst = buf[ni];
                            let dr = ((dst >> 16) & 0xFF) as f32;
                            let dg = ((dst >> 8)  & 0xFF) as f32;
                            let db = (dst & 0xFF) as f32;
                            let ir = ((color >> 16) & 0xFF) as f32;
                            let ig = ((color >> 8)  & 0xFF) as f32;
                            let ib = (color & 0xFF) as f32;
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

// ── 3. Highlight pass ─────────────────────────────────────────────────────────

/// Tint the bright (lit) band pixels toward `highlight_color`.
///
/// This gives the classic anime "shine" — a slightly warm or cool tinted
/// region at the top of each lit face.  Pixels with luminance ≥ `thresh`
/// are blended toward the highlight colour by `strength`.
///
/// Typical values: strength=0.25, thresh=0.78 (lit band ≥ 200/255).
pub fn draw_highlights(
    buf:             &mut Vec<u32>,
    width:           usize,
    height:          usize,
    highlight_color: u32,
    strength:        f32,
    thresh:          f32,
) {
    let n = width * height;
    if buf.len() < n || strength <= 0.0 { return; }
    let hr = ((highlight_color >> 16) & 0xFF) as f32;
    let hg = ((highlight_color >> 8)  & 0xFF) as f32;
    let hb = (highlight_color & 0xFF) as f32;
    let s  = strength.clamp(0.0, 1.0);
    let t  = thresh.clamp(0.0, 1.0) * 255.0;

    for px in buf[..n].iter_mut() {
        let r = ((*px >> 16) & 0xFF) as f32;
        let g = ((*px >> 8)  & 0xFF) as f32;
        let b = (*px & 0xFF) as f32;
        let lum = 0.299 * r + 0.587 * g + 0.114 * b;
        if lum < t { continue; }
        // Blend amount scales with how far into the lit band this pixel is
        let blend = s * ((lum - t) / (255.0 - t + 1.0)).clamp(0.0, 1.0);
        let nr = (r + (hr - r) * blend).clamp(0.0, 255.0) as u32;
        let ng = (g + (hg - g) * blend).clamp(0.0, 255.0) as u32;
        let nb = (b + (hb - b) * blend).clamp(0.0, 255.0) as u32;
        *px = (nr << 16) | (ng << 8) | nb;
    }
}

// ── Convenience: run all configured post-process passes ───────────────────────

/// Post-process configuration stored in `GfxState`.
#[derive(Debug, Clone)]
pub struct ToonConfig {
    /// Shadow boundary softness: 0 = hard, 0.15 = anime-clean, 0.4 = painterly.
    pub shadow_softness: f32,
    /// Outline thickness in pixels (0 = off).
    pub outline_px:      f32,
    /// Depth discontinuity that triggers an outline stamp.
    pub outline_thresh:  f32,
    /// Ink colour (0x00RRGGBB).
    pub outline_color:   u32,
    /// Highlight blend strength (0 = off).
    pub highlight_strength: f32,
    /// Highlight colour.
    pub highlight_color: u32,
    /// Minimum luminance to apply the highlight (normalised 0..1).
    pub highlight_thresh:f32,
}

impl Default for ToonConfig {
    fn default() -> Self {
        Self {
            shadow_softness:    0.0,
            outline_px:         0.0,
            outline_thresh:     0.05,
            outline_color:      0x00_00_00,
            highlight_strength: 0.0,
            highlight_color:    0x00FF_FFFF,
            highlight_thresh:   0.78,
        }
    }
}

/// Apply all enabled toon passes in the correct order.
///
/// Call this after `queue.flush()` and before presenting the buffer to screen.
pub fn apply(
    cfg:    &ToonConfig,
    buf:    &mut Vec<u32>,
    zbuf:   &[f32],
    width:  usize,
    height: usize,
) {
    if cfg.shadow_softness > 0.0 {
        smooth_shadow_edges(buf, width, height, cfg.shadow_softness);
    }
    if cfg.outline_px > 0.0 {
        draw_outlines(buf, zbuf, width, height,
            cfg.outline_px, cfg.outline_color, cfg.outline_thresh);
    }
    if cfg.highlight_strength > 0.0 {
        draw_highlights(buf, width, height,
            cfg.highlight_color, cfg.highlight_strength, cfg.highlight_thresh);
    }
}
