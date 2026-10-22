//! Thin elastic skin as a level set. The zero level set of `phi` (negative inside)
//! moves along its normal with speed
//! `F = relief·coupling·(drive − threshold) − stiffness·δ + tension·relief·∇²δ`
//! where `δ = phi0 − phi` is the outward displacement from the rest shape: the
//! acoustic drive pushes the skin out, stiffness pulls it back, and tension spreads
//! the displacement sideways like an elastic sheet, so loads are shared with
//! neighbouring skin instead of forming spikes.

use crate::grid::Grid;
use rayon::prelude::*;

#[derive(Clone, Copy, Debug)]
pub struct MembraneCfg {
    pub tension: f32,
    pub coupling: f32,
    pub threshold: f32,
    pub stiffness: f32,
    pub relief: f32,
    pub band_cells: f32,
}

pub struct Membrane {
    pub grid: Grid,
    pub phi: Vec<f32>,
    phi0: Vec<f32>,
    scratch: Vec<f32>,
}

fn interior(g: &Grid, x: usize, y: usize, z: usize) -> bool {
    x > 1 && y > 1 && z > 1 && x + 2 < g.nx && y + 2 < g.ny && z + 2 < g.nz
}

fn grad_mag(phi: &[f32], g: &Grid, i: usize) -> f32 {
    let (sy, sz) = (g.nx, g.nx * g.ny);
    let gx = phi[i + 1] - phi[i - 1];
    let gy = phi[i + sy] - phi[i - sy];
    let gz = phi[i + sz] - phi[i - sz];
    (gx * gx + gy * gy + gz * gz).sqrt() / (2.0 * g.dx)
}

impl Membrane {
    /// The rest field is redistanced first so rest and evolving fields share one metric.
    pub fn new(grid: Grid, phi: Vec<f32>) -> Membrane {
        let mut m = Membrane { grid, scratch: phi.clone(), phi0: Vec::new(), phi };
        m.reinitialize(12, 8.0);
        m.phi0 = m.phi.clone();
        m
    }

    pub fn rest(&self) -> &[f32] {
        &self.phi0
    }

    /// Advances the skin by pseudo-time `tau`. `drive(world)` supplies the normalised
    /// acoustic drive at a lattice point. The interval is split into sub-steps, and when
    /// the sub-step cap is reached tension and stiffness are limited to what the explicit
    /// scheme can integrate stably.
    pub fn step<D>(&mut self, cfg: &MembraneCfg, tau: f32, drive: D)
    where
        D: Fn([f32; 3]) -> f32 + Sync,
    {
        let dx = self.grid.dx;
        let diffusion = (cfg.tension * cfg.relief).max(1e-9);
        let limit = (0.12 * dx * dx / diffusion).min(0.5 / cfg.stiffness.max(1e-6));
        let n = ((tau / limit).ceil() as usize).clamp(1, 96);
        let dt = tau / n as f32;
        let diffusion = diffusion.min(0.12 * dx * dx / dt);
        let stiffness = cfg.stiffness.min(0.9 / dt);
        for _ in 0..n {
            self.substep(cfg, dt, diffusion, stiffness, &drive);
        }
    }

    fn substep<D>(&mut self, cfg: &MembraneCfg, dt: f32, diffusion: f32, stiffness: f32, drive: &D)
    where
        D: Fn([f32; 3]) -> f32 + Sync,
    {
        let g = self.grid;
        let band = cfg.band_cells * g.dx;
        let (sy, sz) = (g.nx, g.nx * g.ny);
        let inv_h2 = 1.0 / (g.dx * g.dx);
        let (phi, phi0) = (&self.phi, &self.phi0);
        self.scratch.par_chunks_mut(g.nx * g.ny).enumerate().for_each(|(z, slab)| {
            for y in 0..g.ny {
                for x in 0..g.nx {
                    let i = g.idx(x, y, z);
                    let c = phi[i];
                    let o = &mut slab[x + g.nx * y];
                    if c.abs() > band || !interior(&g, x, y, z) {
                        *o = c;
                        continue;
                    }
                    let d = |j: usize| phi0[j] - phi[j];
                    let centre = d(i);
                    let lap = (d(i + 1) + d(i - 1) + d(i + sy) + d(i - sy) + d(i + sz) + d(i - sz) - 6.0 * centre) * inv_h2;
                    let speed = cfg.relief * cfg.coupling * (drive(g.world(x, y, z)) - cfg.threshold)
                        - stiffness * centre
                        + diffusion * lap;
                    *o = c - dt * speed * grad_mag(phi, &g, i);
                }
            }
        });
        std::mem::swap(&mut self.phi, &mut self.scratch);
    }

    /// Restores `|∇phi| = 1` near the surface with the Sussman redistancing flow.
    pub fn reinitialize(&mut self, iterations: usize, band_cells: f32) {
        let g = self.grid;
        let reach = band_cells * g.dx;
        let src = self.phi.clone();
        let dtau = 0.3 * g.dx;
        for _ in 0..iterations {
            let phi = &self.phi;
            self.scratch.par_chunks_mut(g.nx * g.ny).enumerate().for_each(|(z, slab)| {
                for y in 0..g.ny {
                    for x in 0..g.nx {
                        let i = g.idx(x, y, z);
                        let c = phi[i];
                        let o = &mut slab[x + g.nx * y];
                        if c.abs() > reach || !interior(&g, x, y, z) {
                            *o = c;
                            continue;
                        }
                        let s0 = src[i];
                        let s = s0 / (s0 * s0 + g.dx * g.dx).sqrt();
                        let d = |a: isize| (phi[(i as isize + a) as usize] - c) / g.dx;
                        let (sx, sy, sz) = (1isize, g.nx as isize, (g.nx * g.ny) as isize);
                        let up = |m: f32, p: f32| {
                            if s > 0.0 {
                                (-m).max(0.0).powi(2).max((-p).max(0.0).powi(2))
                            } else {
                                m.max(0.0).powi(2).max(p.max(0.0).powi(2))
                            }
                        };
                        let mag = (up(d(-sx), d(sx)) + up(d(-sy), d(sy)) + up(d(-sz), d(sz))).sqrt();
                        *o = c - dtau * s * (mag - 1.0);
                    }
                }
            });
            std::mem::swap(&mut self.phi, &mut self.scratch);
        }
    }

    /// Largest inward and outward displacement of the zero level set from its rest shape, in metres.
    pub fn displacement_range(&self) -> (f32, f32) {
        let band = 2.0 * self.grid.dx;
        let (mut lo, mut hi) = (0.0f32, 0.0f32);
        for (p, p0) in self.phi.iter().zip(&self.phi0) {
            if p0.abs() < band {
                let d = p0 - p;
                lo = lo.min(d);
                hi = hi.max(d);
            }
        }
        (lo, hi)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sphere(grid: &Grid, r: f32) -> Vec<f32> {
        let c = [grid.nx as f32 / 2.0 * grid.dx; 3];
        (0..grid.len())
            .map(|i| {
                let (x, y, z) = grid.coords(i);
                let p = grid.world(x, y, z);
                ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) + (p[2] - c[2]).powi(2)).sqrt() - r
            })
            .collect()
    }

    fn cfg(tension: f32, relief: f32) -> MembraneCfg {
        MembraneCfg { tension, coupling: 1.0, threshold: 0.0, stiffness: 1.0, relief, band_cells: 6.0 }
    }

    fn grid() -> Grid {
        Grid { nx: 32, ny: 32, nz: 32, origin: [0.0; 3], dx: 0.02 }
    }

    #[test]
    fn rest_shape_is_stationary() {
        let g = grid();
        let mut m = Membrane::new(g, sphere(&g, 0.2));
        let rest = m.rest().to_vec();
        m.step(&cfg(0.5, 0.01), 1.0, |_| 0.0);
        let worst = m.phi.iter().zip(&rest).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(worst < 1e-5, "drift {worst}");
    }

    #[test]
    fn uniform_drive_settles_at_the_elastic_equilibrium() {
        let g = grid();
        let mut m = Membrane::new(g, sphere(&g, 0.2));
        m.step(&cfg(0.2, 0.02), 8.0, |_| 1.0);
        let (inward, outward) = m.displacement_range();
        assert!((outward - 0.02).abs() < 0.004, "outward {outward}, expected relief/stiffness = 0.02");
        assert!(inward > -0.002, "inward {inward}");
    }

    #[test]
    fn tension_spreads_a_local_push() {
        let g = grid();
        let pole = [0.52f32, 0.32, 0.32];
        let spot = move |w: [f32; 3]| {
            let d2: f32 = (0..3).map(|k| (w[k] - pole[k]).powi(2)).sum();
            if d2.sqrt() < 0.06 { 1.0 } else { 0.0 }
        };
        let reach = |tension: f32| {
            let mut m = Membrane::new(g, sphere(&g, 0.2));
            m.step(&cfg(tension, 0.02), 1.0, spot);
            let probe = g.idx(24, 22, 16);
            m.rest()[probe] - m.phi[probe]
        };
        assert!(reach(0.5) > reach(0.0) + 2e-5, "tension should carry displacement to neighbouring skin");
    }

    #[test]
    fn reinit_restores_unit_gradient() {
        let g = grid();
        let mut phi = sphere(&g, 0.2);
        phi.iter_mut().for_each(|p| *p *= 1.6);
        let mut m = Membrane::new(g, phi);
        m.reinitialize(40, 6.0);
        let gm = grad_mag(&m.phi, &g, g.idx(16, 16, 24));
        assert!((gm - 1.0).abs() < 0.15, "gradient {gm}");
    }
}
