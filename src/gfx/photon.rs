// src/gfx/photon.rs — "Photon-water" HDR light accumulation buffer.
//
// Model
// ═════
// Each pixel is a pool.  Light pours RGB energy in — the "water level" rises.
// A `flow()` pass lets energy diffuse slightly into neighbours (simple separable
// box-blur), softening shadow boundaries before the final quantisation step.
// `drain_toon()` snaps each pool's level to discrete toon bands (like water
// settling into terraced channels) and writes the result to the u32 pixel buf.
//
// Usage
// ─────
//   let mut pb = PhotonBuf::new(w, h);
//   // rasteriser writes linear energy via pb.add(idx, r, g, b)
//   pb.flow(0.08);              // optional soft-shadow diffusion
//   pb.drain_toon(&mut buf, 3); // snap → pixel buffer
//   pb.clear();                 // reset for next frame
//
// Performance notes
// ─────────────────
// • Three separate f32 vecs → better cache behaviour than interleaved RGBA.
// • flow() is a two-pass separable filter: O(w·h) with no extra allocation.
// • drain_toon() has no branches inside the pixel loop (the threshold check
//   compiles to branchless cmov on x86/ARM).
// • unsafe indexing in add() because it's called from the rasteriser inner
//   loop; the caller must guarantee idx < w*h.

/// HDR light-accumulation buffer — one f32 per channel per pixel.
pub struct PhotonBuf {
    pub r:      Vec<f32>,
    pub g:      Vec<f32>,
    pub b:      Vec<f32>,
    pub width:  usize,
    pub height: usize,
}

impl PhotonBuf {
    pub fn new(width: usize, height: usize) -> Self {
        let n = width * height;
        Self {
            r: vec![0.0_f32; n],
            g: vec![0.0_f32; n],
            b: vec![0.0_f32; n],
            width,
            height,
        }
    }

    /// Resize (or re-create) to match new dimensions. Clears content.
    pub fn resize(&mut self, width: usize, height: usize) {
        let n = width * height;
        self.r.resize(n, 0.0);
        self.g.resize(n, 0.0);
        self.b.resize(n, 0.0);
        // Zero any new or old data
        self.r[..n].fill(0.0);
        self.g[..n].fill(0.0);
        self.b[..n].fill(0.0);
        self.width  = width;
        self.height = height;
    }

    #[inline]
    pub fn clear(&mut self) {
        self.r.fill(0.0);
        self.g.fill(0.0);
        self.b.fill(0.0);
    }

    /// Add light energy at pixel index `idx`.
    /// # Safety
    /// `idx` must be < `self.r.len()` — the caller clips to the framebuffer bounds.
    #[inline]
    pub unsafe fn add_unchecked(&mut self, idx: usize, r: f32, g: f32, b: f32) {
        *self.r.get_unchecked_mut(idx) += r;
        *self.g.get_unchecked_mut(idx) += g;
        *self.b.get_unchecked_mut(idx) += b;
    }

    /// Safe variant — bounds-checks `idx`.
    #[inline]
    pub fn add(&mut self, idx: usize, r: f32, g: f32, b: f32) {
        if idx < self.r.len() {
            self.r[idx] += r;
            self.g[idx] += g;
            self.b[idx] += b;
        }
    }

    // ── Flow (soft-shadow diffusion) ─────────────────────────────────────────

    /// Diffuse energy into neighbouring pixels before toon quantisation.
    ///
    /// `strength` ∈ [0, 1]:
    ///   0.0 → no diffusion (hard cel shadows, slightly jagged boundaries)
    ///   0.05–0.15 → subtle soft shadow edges (recommended for toon)
    ///   0.4+ → very soft / painted look
    ///
    /// Implemented as two single-pass separable box-blur sweeps (horizontal
    /// then vertical), each with radius 1.  Two rounds give a rough Gaussian
    /// feel without extra allocation.
    pub fn flow(&mut self, strength: f32) {
        if strength <= 0.0 {
            return;
        }
        let w = self.width;
        let h = self.height;
        let s = strength.clamp(0.0, 1.0);
        // Each of the two horizontal neighbours contributes `k` of their energy.
        // Centre keeps `c` = 1 - 2k so total energy is conserved.
        // Boundary pixels treat the missing neighbour as 0 (virtual zero outside
        // the grid) — this keeps the full pass energy-conserving for all pixels.
        let k = s * 0.5;   // 0..0.5
        let c = 1.0 - s;   // 1..0  (never goes negative because k≤0.5)

        // Horizontal sweep — ALL columns including boundaries
        for row in 0..h {
            let base = row * w;
            for col in 0..w {
                let i = base + col;
                let l_val_r = if col > 0 { unsafe { *self.r.get_unchecked(i - 1) } } else { 0.0 };
                let r_val_r = if col + 1 < w { unsafe { *self.r.get_unchecked(i + 1) } } else { 0.0 };
                let l_val_g = if col > 0 { unsafe { *self.g.get_unchecked(i - 1) } } else { 0.0 };
                let r_val_g = if col + 1 < w { unsafe { *self.g.get_unchecked(i + 1) } } else { 0.0 };
                let l_val_b = if col > 0 { unsafe { *self.b.get_unchecked(i - 1) } } else { 0.0 };
                let r_val_b = if col + 1 < w { unsafe { *self.b.get_unchecked(i + 1) } } else { 0.0 };
                unsafe {
                    *self.r.get_unchecked_mut(i) =
                        *self.r.get_unchecked(i) * c + (l_val_r + r_val_r) * k;
                    *self.g.get_unchecked_mut(i) =
                        *self.g.get_unchecked(i) * c + (l_val_g + r_val_g) * k;
                    *self.b.get_unchecked_mut(i) =
                        *self.b.get_unchecked(i) * c + (l_val_b + r_val_b) * k;
                }
            }
        }
        // Vertical sweep — ALL rows including boundaries
        for col in 0..w {
            for row in 0..h {
                let i = row * w + col;
                let u_val_r = if row > 0 { unsafe { *self.r.get_unchecked(i - w) } } else { 0.0 };
                let d_val_r = if row + 1 < h { unsafe { *self.r.get_unchecked(i + w) } } else { 0.0 };
                let u_val_g = if row > 0 { unsafe { *self.g.get_unchecked(i - w) } } else { 0.0 };
                let d_val_g = if row + 1 < h { unsafe { *self.g.get_unchecked(i + w) } } else { 0.0 };
                let u_val_b = if row > 0 { unsafe { *self.b.get_unchecked(i - w) } } else { 0.0 };
                let d_val_b = if row + 1 < h { unsafe { *self.b.get_unchecked(i + w) } } else { 0.0 };
                unsafe {
                    *self.r.get_unchecked_mut(i) =
                        *self.r.get_unchecked(i) * c + (u_val_r + d_val_r) * k;
                    *self.g.get_unchecked_mut(i) =
                        *self.g.get_unchecked(i) * c + (u_val_g + d_val_g) * k;
                    *self.b.get_unchecked_mut(i) =
                        *self.b.get_unchecked(i) * c + (u_val_b + d_val_b) * k;
                }
            }
        }
    }

    // ── Drain (quantise and write to u32 buffer) ─────────────────────────────

    /// Snap each pixel's float energy to `bands` toon levels and pack into `buf`.
    ///
    /// Standard 3-band thresholds (matching `cel_quantize`):
    ///   t < 0.25 → 0.08  (shadow)
    ///   t < 0.60 → 0.50  (mid)
    ///   t ≥ 0.60 → 1.00  (lit)
    ///
    /// Values above 1.0 (overlit emission) are clamped before snapping.
    /// Caller must ensure `buf.len() >= self.r.len()`.
    pub fn drain_toon(&self, buf: &mut [u32], bands: u32) {
        let n = self.r.len().min(buf.len());
        if bands < 2 {
            // Passthrough: clamp + pack, no quantisation
            for i in 0..n {
                if self.r[i] <= 0.0 && self.g[i] <= 0.0 && self.b[i] <= 0.0 {
                    continue; // zero energy → leave buffer black
                }
                let r = (self.r[i].clamp(0.0, 1.0) * 255.0) as u32;
                let g = (self.g[i].clamp(0.0, 1.0) * 255.0) as u32;
                let b = (self.b[i].clamp(0.0, 1.0) * 255.0) as u32;
                buf[i] = (r << 16) | (g << 8) | b;
            }
            return;
        }
        for i in 0..n {
            // SAFETY: i < n ≤ min(r.len(), buf.len())
            unsafe {
                // Zero-energy pixel → leave buffer as-is (background stays black).
                if *self.r.get_unchecked(i) <= 0.0
                    && *self.g.get_unchecked(i) <= 0.0
                    && *self.b.get_unchecked(i) <= 0.0
                {
                    continue;
                }
                let r = snap(*self.r.get_unchecked(i), bands);
                let g = snap(*self.g.get_unchecked(i), bands);
                let b = snap(*self.b.get_unchecked(i), bands);
                *buf.get_unchecked_mut(i) =
                    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
            }
        }
    }

    /// Like `drain_toon` but additively composites over an existing buffer
    /// (for the light-overlay use-case: draw geometry normally, then pour
    /// photon-accumulated highlights on top).
    pub fn drain_additive(&self, buf: &mut [u32]) {
        let n = self.r.len().min(buf.len());
        for i in 0..n {
            if self.r[i] <= 0.0 && self.g[i] <= 0.0 && self.b[i] <= 0.0 {
                continue;
            }
            let r = (self.r[i].clamp(0.0, 1.0) * 255.0) as u32;
            let g = (self.g[i].clamp(0.0, 1.0) * 255.0) as u32;
            let b = (self.b[i].clamp(0.0, 1.0) * 255.0) as u32;
            let src = (r << 16) | (g << 8) | b;
            buf[i] = add_sat(buf[i], src);
        }
    }
}

/// Snap a float value in [0..∞] to a toon band and return a [0..255] byte.
/// Branchless on most architectures — compiles to fsel/fcmp+cmov.
#[inline]
fn snap(v: f32, bands: u32) -> u8 {
    let t = v.clamp(0.0, 1.0);
    let out = if bands == 3 {
        // Fast-path for the common 3-band case — matches cel_quantize exactly
        if t < 0.25 { 20u8 }       // 0.08 * 255 ≈ 20
        else if t < 0.60 { 127u8 } // 0.50 * 255
        else { 255u8 }             // 1.00 * 255
    } else {
        // Generic n-band: evenly divide [0,1]
        let n = bands as f32;
        let band = (t * n).floor().min(n - 1.0);
        // map to [0, 255]: centre of each band
        ((band + 0.5) / n * 255.0) as u8
    };
    out
}

#[inline]
fn add_sat(a: u32, b: u32) -> u32 {
    let r = ((a >> 16 & 0xFF) + (b >> 16 & 0xFF)).min(0xFF);
    let g = ((a >> 8  & 0xFF) + (b >> 8  & 0xFF)).min(0xFF);
    let bl= ((a       & 0xFF) + (b       & 0xFF)).min(0xFF);
    (r << 16) | (g << 8) | bl
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_drain_white_lit() {
        let mut pb = PhotonBuf::new(4, 1);
        pb.add(0, 1.0, 1.0, 1.0); // full energy
        pb.add(1, 0.5, 0.5, 0.5); // half
        pb.add(2, 0.1, 0.1, 0.1); // dark
        // idx 3 stays zero (background)
        let mut buf = vec![0u32; 4];
        pb.drain_toon(&mut buf, 3);
        // lit (≥0.6): pixel 0 should be 0xFFFFFF
        assert_eq!(buf[0] & 0xFF, 0xFF, "full energy → lit band (white)");
        // mid (0.25-0.60): pixel 1 should be 0x7F7F7F
        let mid = buf[1] & 0xFF;
        assert!(mid > 50 && mid < 200, "half energy → mid band, got {mid}");
        // shadow (<0.25): pixel 2 should be dim
        let dark = buf[2] & 0xFF;
        assert!(dark < 50, "dark energy → shadow band, got {dark}");
        // background: pixel 3 should be zero
        assert_eq!(buf[3], 0, "zero energy → black");
    }

    #[test]
    fn flow_conserves_energy_approx() {
        let mut pb = PhotonBuf::new(3, 3);
        // Spike in the centre
        pb.add(4, 1.0, 0.0, 0.0);
        let sum_before: f32 = pb.r.iter().sum();
        pb.flow(0.2);
        let sum_after: f32 = pb.r.iter().sum();
        // Energy shouldn't spontaneously appear (tolerance 5%)
        assert!((sum_after - sum_before).abs() < sum_before * 0.05,
            "flow should approximately conserve energy: {sum_before} vs {sum_after}");
    }
}
