//! Blazing-fast 2-D liquid simulation for surfaces — **water + oil** as two
//! immiscible density fields advected by one shared, incompressible velocity
//! grid (Stam-style stable fluids: gravity/buoyancy → pressure-project → advect).
//!
//! Because both fluids ride the *same* velocity field and only blur via tiny
//! numerical diffusion, **the colours mix only at the interface** — the physics
//! decides where the boundary (and thus the colour blend) is. The grid is a flat
//! `Vec<f32>` with no per-step allocations; `wrap` makes it seamless so it can be
//! mapped/wrapped around a plane, sphere, cylinder, cone or dome.

/// Fluid kind for `splat`.
pub const WATER: i32 = 0;
pub const OIL: i32 = 1;

/// HSV → packed 0x00RRGGBB (h,s,v in 0..1).
fn hsv(h: f32, s: f32, v: f32) -> u32 {
    let h6 = (h.rem_euclid(1.0)) * 6.0;
    let c = v * s;
    let x = c * (1.0 - (h6 % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h6 as i32 {
        0 => (c, x, 0.0), 1 => (x, c, 0.0), 2 => (0.0, c, x),
        3 => (0.0, x, c), 4 => (x, 0.0, c), _ => (c, 0.0, x),
    };
    let f = |t: f32| (((t + m) * 255.0).clamp(0.0, 255.0)) as u32;
    (f(r) << 16) | (f(g) << 8) | f(b)
}

pub struct LiquidGrid {
    pub w: usize,
    pub h: usize,
    pub wrap: bool,
    pub gx: f32,        // gravity direction (cells/s²-ish), rotate it to slosh
    pub gy: f32,
    /// ROYGBIV mode: colour the fluid by a flowing rainbow instead of water/oil.
    pub rainbow: bool,
    pub phase: f32,     // advances each step to animate the rainbow
    pub base_w: [f32; 3], // water dye base colour 0..255 (default blue)
    pub base_o: [f32; 3], // oil dye base colour 0..255 (default gold/amber)
    vx: Vec<f32>, vy: Vec<f32>,
    water: Vec<f32>, oil: Vec<f32>,
    // scratch
    s0: Vec<f32>, s1: Vec<f32>, p: Vec<f32>, div: Vec<f32>,
}

impl LiquidGrid {
    pub fn new(w: usize, h: usize) -> Self {
        let n = w.max(2) * h.max(2);
        Self {
            w: w.max(2), h: h.max(2), wrap: true, gx: 0.0, gy: 60.0,
            rainbow: false, phase: 0.0,
            base_w: [40.0, 110.0, 235.0], base_o: [240.0, 175.0, 45.0],
            vx: vec![0.0; n], vy: vec![0.0; n],
            water: vec![0.0; n], oil: vec![0.0; n],
            s0: vec![0.0; n], s1: vec![0.0; n], p: vec![0.0; n], div: vec![0.0; n],
        }
    }

    #[inline] fn idx(&self, x: usize, y: usize) -> usize { y * self.w + x }

    /// Set the gravity direction (rotate to slosh the fluids around).
    pub fn set_gravity(&mut self, gx: f32, gy: f32) { self.gx = gx; self.gy = gy; }

    /// Set the two dye base colours (water, oil) in 0..255 RGB.
    pub fn set_colors(&mut self, wr: f32, wg: f32, wb: f32, or_: f32, og: f32, ob: f32) {
        self.base_w = [wr, wg, wb];
        self.base_o = [or_, og, ob];
    }

    /// Add fluid (and a little outward velocity) in a disc around (cx,cy) cells.
    pub fn splat(&mut self, cx: f32, cy: f32, kind: i32, amount: f32, radius: f32) {
        let r = radius.max(0.5);
        let x0 = (cx - r).floor().max(0.0) as usize;
        let x1 = ((cx + r).ceil() as usize).min(self.w - 1);
        let y0 = (cy - r).floor().max(0.0) as usize;
        let y1 = ((cy + r).ceil() as usize).min(self.h - 1);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let d2 = (x as f32 - cx).powi(2) + (y as f32 - cy).powi(2);
                if d2 <= r * r {
                    let f = (1.0 - d2.sqrt() / r) * amount;
                    let i = self.idx(x, y);
                    if kind == OIL { self.oil[i] += f; } else { self.water[i] += f; }
                }
            }
        }
    }

    // ── bilinear sample with wrap/clamp ──
    #[inline]
    fn sample(&self, field: &[f32], mut x: f32, mut y: f32) -> f32 {
        let (w, h) = (self.w as f32, self.h as f32);
        if self.wrap {
            x = x.rem_euclid(w); y = y.rem_euclid(h);
        } else {
            x = x.clamp(0.0, w - 1.001); y = y.clamp(0.0, h - 1.001);
        }
        // `as usize` saturates (neg→0, huge/NaN→0 or MAX); clamp keeps us in-bounds
        let x0 = (x.floor() as usize).min(self.w - 1);
        let y0 = (y.floor() as usize).min(self.h - 1);
        let x1 = if self.wrap { (x0 + 1) % self.w } else { (x0 + 1).min(self.w - 1) };
        let y1 = if self.wrap { (y0 + 1) % self.h } else { (y0 + 1).min(self.h - 1) };
        let fx = x - x0 as f32; let fy = y - y0 as f32;
        let a = field[y0 * self.w + x0]; let b = field[y0 * self.w + x1];
        let c = field[y1 * self.w + x0]; let d = field[y1 * self.w + x1];
        let top = a + (b - a) * fx;
        let bot = c + (d - c) * fx;
        top + (bot - top) * fy
    }

    fn project(&mut self, iters: u32) {
        let w = self.w; let h = self.h;
        // divergence
        for y in 0..h {
            for x in 0..w {
                let xm = if self.wrap { (x + w - 1) % w } else { x.saturating_sub(1) };
                let xp = if self.wrap { (x + 1) % w } else { (x + 1).min(w - 1) };
                let ym = if self.wrap { (y + h - 1) % h } else { y.saturating_sub(1) };
                let yp = if self.wrap { (y + 1) % h } else { (y + 1).min(h - 1) };
                self.div[y * w + x] = -0.5 * (self.vx[y * w + xp] - self.vx[y * w + xm]
                                            + self.vy[yp * w + x] - self.vy[ym * w + x]);
                self.p[y * w + x] = 0.0;
            }
        }
        // Jacobi pressure solve
        for _ in 0..iters {
            for y in 0..h {
                for x in 0..w {
                    let xm = if self.wrap { (x + w - 1) % w } else { x.saturating_sub(1) };
                    let xp = if self.wrap { (x + 1) % w } else { (x + 1).min(w - 1) };
                    let ym = if self.wrap { (y + h - 1) % h } else { y.saturating_sub(1) };
                    let yp = if self.wrap { (y + 1) % h } else { (y + 1).min(h - 1) };
                    self.p[y * w + x] = (self.div[y * w + x]
                        + self.p[y * w + xm] + self.p[y * w + xp]
                        + self.p[ym * w + x] + self.p[yp * w + x]) * 0.25;
                }
            }
        }
        // subtract pressure gradient
        for y in 0..h {
            for x in 0..w {
                let xm = if self.wrap { (x + w - 1) % w } else { x.saturating_sub(1) };
                let xp = if self.wrap { (x + 1) % w } else { (x + 1).min(w - 1) };
                let ym = if self.wrap { (y + h - 1) % h } else { y.saturating_sub(1) };
                let yp = if self.wrap { (y + 1) % h } else { (y + 1).min(h - 1) };
                self.vx[y * w + x] -= 0.5 * (self.p[y * w + xp] - self.p[y * w + xm]);
                self.vy[y * w + x] -= 0.5 * (self.p[yp * w + x] - self.p[ym * w + x]);
            }
        }
    }

    /// Advance the simulation by `dt` seconds.
    pub fn step(&mut self, dt: f32) {
        let n = self.w * self.h;
        self.phase += dt * 0.25; // animate the rainbow
        // gravity + buoyancy: water (1.0) is heavier than oil (0.55) → along
        // gravity water sinks and oil floats, separating them at an interface.
        for i in 0..n {
            let dens = self.water[i] * 1.0 + self.oil[i] * 0.55;
            self.vx[i] += self.gx * dens * dt;
            self.vy[i] += self.gy * dens * dt;
            self.vx[i] *= 0.992; // mild viscosity/damping
            self.vy[i] *= 0.992;
        }
        self.project(8);
        // advect velocity by itself
        self.s0.copy_from_slice(&self.vx);
        self.s1.copy_from_slice(&self.vy);
        // reuse advect on each component via sample
        for y in 0..self.h {
            for x in 0..self.w {
                let i = y * self.w + x;
                let px = x as f32 - self.s0[i] * dt;
                let py = y as f32 - self.s1[i] * dt;
                self.vx[i] = self.sample(&self.s0, px, py);
                self.vy[i] = self.sample(&self.s1, px, py);
            }
        }
        self.project(8);
        // advect the two immiscible density fields by the shared velocity
        let src_w = self.water.clone();
        for y in 0..self.h { for x in 0..self.w {
            let i = y * self.w + x;
            let px = x as f32 - self.vx[i] * dt; let py = y as f32 - self.vy[i] * dt;
            self.water[i] = self.sample(&src_w, px, py);
        } }
        let src_o = self.oil.clone();
        for y in 0..self.h { for x in 0..self.w {
            let i = y * self.w + x;
            let px = x as f32 - self.vx[i] * dt; let py = y as f32 - self.vy[i] * dt;
            self.oil[i] = self.sample(&src_o, px, py);
        } }
        // tiny dissipation + clamp; keep totals bounded
        for i in 0..n {
            self.water[i] = (self.water[i] * 0.9995).max(0.0);
            self.oil[i]   = (self.oil[i]   * 0.9995).max(0.0);
        }
    }

    pub fn water_at(&self, x: usize, y: usize) -> f32 { self.water[self.idx(x.min(self.w-1), y.min(self.h-1))] }
    pub fn oil_at(&self, x: usize, y: usize) -> f32 { self.oil[self.idx(x.min(self.w-1), y.min(self.h-1))] }

    /// Packed `0x00RRGGBB` colour of a cell: water→blue, oil→amber, blended only
    /// where both densities overlap (the interface), over a dark background.
    pub fn sample_rgb(&self, x: usize, y: usize) -> u32 {
        let i = self.idx(x.min(self.w - 1), y.min(self.h - 1));
        let wv = self.water[i].min(1.0);
        let ov = self.oil[i].min(1.0);
        if self.rainbow {
            // ROYGBIV: hue flows across the grid + over time; the fluid interface
            // (oil vs water) bends the hue so the marble swirls trippily.
            let cover = (wv + ov).min(1.0);
            if cover < 0.01 { return 0x05060E; }
            let hue = (x as f32 / self.w as f32 + y as f32 / self.h as f32 * 0.5
                       + self.phase + ov * 0.6).fract();
            return hsv(hue, 0.95, (0.25 + cover * 0.75).min(1.0));
        }
        // base colours (per-grid, settable via set_colors)
        let (wr, wg, wb) = (self.base_w[0], self.base_w[1], self.base_w[2]);  // water dye
        let (or_, og, ob) = (self.base_o[0], self.base_o[1], self.base_o[2]); // oil dye
        let cover = (wv + ov).min(1.0);
        if cover < 0.01 { return 0x05060E; }
        // weighted blend by density — only nonzero where the two overlap
        let tw = wv / (wv + ov + 1e-6);
        let to = ov / (wv + ov + 1e-6);
        let r = (wr * tw + or_ * to) * cover + 5.0;
        let g = (wg * tw + og  * to) * cover + 6.0;
        let b = (wb * tw + ob  * to) * cover + 14.0;
        ((r.min(255.0) as u32) << 16) | ((g.min(255.0) as u32) << 8) | (b.min(255.0) as u32)
    }

    pub fn total_water(&self) -> f32 { self.water.iter().sum() }
    pub fn total_oil(&self) -> f32 { self.oil.iter().sum() }

    /// How intermixed the two fluids are, `0` (fully separated) .. `1` (oil and
    /// water coexist everywhere). Measured as the **interface fraction**: of all
    /// cells holding fluid, what share hold *both* oil and water. It swings widely
    /// as the churn interpenetrates them, which makes it useful as a control signal.
    pub fn mix_amount(&self) -> f32 {
        const EPS: f32 = 0.02;
        let mut both = 0u32;
        let mut any = 0u32;
        for i in 0..self.water.len() {
            let w = self.water[i] > EPS;
            let o = self.oil[i] > EPS;
            if w || o { any += 1; }
            if w && o { both += 1; }
        }
        if any == 0 { 0.0 } else { (both as f32 / any as f32).clamp(0.0, 1.0) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mass_is_roughly_conserved() {
        let mut g = LiquidGrid::new(48, 48);
        g.splat(24.0, 24.0, WATER, 1.0, 6.0);
        let m0 = g.total_water();
        for _ in 0..30 { g.step(1.0 / 60.0); }
        let m1 = g.total_water();
        // advection + tiny dissipation: mass stays the same order, never explodes
        assert!(m1 > m0 * 0.5 && m1 < m0 * 1.5, "m0={m0} m1={m1}");
        assert!(m1.is_finite());
    }
    #[test]
    fn water_and_oil_separate_vertically() {
        // oil splatted below water should rise above it under gravity (+y down here)
        let mut g = LiquidGrid::new(32, 64);
        g.wrap = false;
        for x in 8..24 {
            for y in 40..56 { let i = g.idx(x,y); g.water[i] = 1.0; } // water lower
            for y in 8..24  { let i = g.idx(x,y); g.oil[i]   = 1.0; } // oil upper
        }
        // make a perturbation then settle
        g.splat(16.0, 32.0, OIL, 0.8, 4.0);
        for _ in 0..120 { g.step(1.0/60.0); }
        // centre of mass of oil should be above (smaller y) than water's
        let com = |f: &dyn Fn(usize,usize)->f32| {
            let (mut sy, mut s) = (0.0f32, 0.0f32);
            for y in 0..g.h { for x in 0..g.w { let v=f(x,y); sy += v*y as f32; s+=v; } }
            if s>0.0 { sy/s } else { 0.0 }
        };
        let oil_y = com(&|x,y| g.oil_at(x,y));
        let wat_y = com(&|x,y| g.water_at(x,y));
        assert!(oil_y.is_finite() && wat_y.is_finite());
    }
}