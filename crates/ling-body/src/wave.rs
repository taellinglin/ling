//! Three-dimensional acoustic FDTD on a staggered (Yee) grid: pressure at cell
//! centres, particle velocity on cell faces, leapfrog in time. Density and bulk
//! modulus vary per cell, so the skin is a genuine impedance interface and the
//! scheme is stable up to the usual CFL limit for any density contrast. Rigid walls
//! close the domain and a quadratic sponge layer absorbs outgoing waves.

use crate::grid::Grid;
use crate::params::{Source, Waveform};
use rayon::prelude::*;

const SPONGE_MAX: f32 = 0.25;
pub const CFL: f32 = 0.9;

/// Smallest lattice spacing that keeps the scheme stable for sound speed `c_max` at time step `dt`.
pub fn spacing_for(c_max: f32, dt: f32) -> f32 {
    c_max * dt * 3.0f32.sqrt() / CFL
}

struct Emitter {
    cells: [usize; 8],
    weights: [f32; 8],
    wave: Waveform,
    freq: f32,
    amp: f32,
    phase: f32,
    start: f32,
    dur: f32,
}

impl Emitter {
    fn value(&self, t: f32, dt: f32) -> f32 {
        use std::f32::consts::PI;
        match self.wave {
            Waveform::Ricker => {
                let tau = t - (self.start + 1.5 / self.freq);
                if tau.abs() > 3.0 / self.freq {
                    return 0.0;
                }
                let a = (PI * self.freq * tau).powi(2);
                self.amp * (1.0 - 2.0 * a) * (-a).exp()
            },
            Waveform::SineBurst => {
                let dur = self.dur.max(2.0 / self.freq);
                let local = t - self.start;
                if !(0.0..=dur).contains(&local) {
                    return 0.0;
                }
                let env = (PI * local / dur).sin().powi(2);
                self.amp * env * (2.0 * PI * self.freq * local + self.phase).sin()
            },
            Waveform::Impulse => {
                let sigma = 1.5 * dt;
                let tau = t - (self.start + 4.0 * sigma);
                if tau.abs() > 4.0 * sigma {
                    return 0.0;
                }
                self.amp * (-0.5 * (tau / sigma).powi(2)).exp()
            },
        }
    }

    fn last_active(&self, dt: f32) -> f32 {
        match self.wave {
            Waveform::Ricker => self.start + 3.0 / self.freq + 1.5 / self.freq,
            Waveform::SineBurst => self.start + self.dur.max(2.0 / self.freq),
            Waveform::Impulse => self.start + 12.0 * dt,
        }
    }
}

pub struct WaveSettings {
    pub c_air: f32,
    pub c_body_ratio: f32,
    pub rho_body: f32,
    pub damping: f32,
    pub sponge: usize,
    pub dt: f32,
    pub drive_tau: f32,
}

pub struct WaveField {
    pub grid: Grid,
    pub dt: f32,
    pub p: Vec<f32>,
    vx: Vec<f32>,
    vy: Vec<f32>,
    vz: Vec<f32>,
    /// Exponential moving average of p².
    pub energy: Vec<f32>,
    /// dt / dx
    h: f32,
    /// Bulk modulus ρc² per cell.
    bulk: Vec<f32>,
    rho: Vec<f32>,
    keep: Vec<f32>,
    emitters: Vec<Emitter>,
    mics: Vec<([usize; 8], [f32; 8])>,
    pub mic_signal: Vec<Vec<f32>>,
    pub steps_done: usize,
    ema: f32,
}

fn corner_weights(grid: &Grid, pos: [f32; 3]) -> ([usize; 8], [f32; 8]) {
    let l = grid.lattice(pos);
    let cl = |v: f32, n: usize| v.clamp(0.0, (n - 1) as f32 - 1e-3);
    let (fx, fy, fz) = (cl(l[0], grid.nx), cl(l[1], grid.ny), cl(l[2], grid.nz));
    let (x0, y0, z0) = (fx as usize, fy as usize, fz as usize);
    let (tx, ty, tz) = (fx - x0 as f32, fy - y0 as f32, fz - z0 as f32);
    let mut cells = [0usize; 8];
    let mut w = [0.0f32; 8];
    for k in 0..8 {
        let (dx, dy, dz) = (k & 1, (k >> 1) & 1, (k >> 2) & 1);
        cells[k] = grid.idx((x0 + dx).min(grid.nx - 1), (y0 + dy).min(grid.ny - 1), (z0 + dz).min(grid.nz - 1));
        w[k] = (if dx == 1 { tx } else { 1.0 - tx })
            * (if dy == 1 { ty } else { 1.0 - ty })
            * (if dz == 1 { tz } else { 1.0 - tz });
    }
    (cells, w)
}

impl WaveField {
    pub fn new(grid: Grid, inside: &[bool], s: &WaveSettings, sources: &[Source], mics: &[[f32; 3]]) -> WaveField {
        let n = grid.len();
        let c_body = s.c_air * s.c_body_ratio;
        let mut bulk = vec![0.0f32; n];
        let mut rho = vec![1.0f32; n];
        let mut keep = vec![1.0f32; n];
        let sponge = s.sponge.max(1) as f32;
        for i in 0..n {
            let c = if inside[i] { c_body } else { s.c_air };
            if inside[i] {
                rho[i] = s.rho_body;
            }
            bulk[i] = rho[i] * c * c;
            let (x, y, z) = grid.coords(i);
            let edge = x.min(y).min(z).min(grid.nx - 1 - x).min(grid.ny - 1 - y).min(grid.nz - 1 - z) as f32;
            let mut gamma = s.damping;
            if edge < sponge {
                gamma += SPONGE_MAX * ((sponge - edge) / sponge).powi(2);
            }
            keep[i] = 1.0 - 0.5 * gamma.min(1.0);
        }
        let emitters = sources
            .iter()
            .map(|src| {
                let (cells, weights) = corner_weights(&grid, src.pos);
                Emitter {
                    cells,
                    weights,
                    wave: src.wave,
                    freq: src.freq.max(1.0),
                    amp: src.amp,
                    phase: src.phase,
                    start: src.start,
                    dur: src.dur,
                }
            })
            .collect();
        let mic_cells: Vec<_> = mics.iter().map(|m| corner_weights(&grid, *m)).collect();
        let ema = if s.drive_tau > 0.0 { 1.0 - (-s.dt / s.drive_tau).exp() } else { 1.0 };
        WaveField {
            grid,
            dt: s.dt,
            p: vec![0.0; n],
            vx: vec![0.0; n],
            vy: vec![0.0; n],
            vz: vec![0.0; n],
            energy: vec![0.0; n],
            h: s.dt / grid.dx,
            bulk,
            rho,
            keep,
            emitters,
            mic_signal: vec![Vec::new(); mic_cells.len()],
            mics: mic_cells,
            steps_done: 0,
            ema,
        }
    }

    /// Time in seconds after which no source emits any more.
    pub fn quiet_after(&self) -> f32 {
        self.emitters.iter().map(|e| e.last_active(self.dt)).fold(0.0, f32::max)
    }

    pub fn step(&mut self, count: usize) {
        for _ in 0..count {
            self.step_once();
        }
    }

    fn step_once(&mut self) {
        let g = self.grid;
        let (nx, ny, nz) = (g.nx, g.ny, g.nz);
        let sxy = nx * ny;
        let h = self.h;
        let (rho, keep, bulk) = (&self.rho, &self.keep, &self.bulk);

        let p = &self.p;
        let (vx, vy, vz) = (&mut self.vx, &mut self.vy, &mut self.vz);
        vx.par_chunks_mut(sxy)
            .zip(vy.par_chunks_mut(sxy))
            .zip(vz.par_chunks_mut(sxy))
            .enumerate()
            .for_each(|(z, ((sx, sy), sz))| {
                for y in 0..ny {
                    for x in 0..nx {
                        let i = x + nx * y + sxy * z;
                        let l = x + nx * y;
                        let face = |j: usize| 2.0 / (rho[i] + rho[j]);
                        let k = keep[i];
                        sx[l] = if x + 1 < nx { (sx[l] - h * face(i + 1) * (p[i + 1] - p[i])) * k } else { 0.0 };
                        sy[l] = if y + 1 < ny { (sy[l] - h * face(i + nx) * (p[i + nx] - p[i])) * k } else { 0.0 };
                        sz[l] = if z + 1 < nz { (sz[l] - h * face(i + sxy) * (p[i + sxy] - p[i])) * k } else { 0.0 };
                    }
                }
            });

        let (vx, vy, vz) = (&self.vx, &self.vy, &self.vz);
        self.p.par_chunks_mut(sxy).enumerate().for_each(|(z, slab)| {
            for y in 0..ny {
                for x in 0..nx {
                    let i = x + nx * y + sxy * z;
                    let mut div = vx[i] + vy[i] + vz[i];
                    if x > 0 {
                        div -= vx[i - 1];
                    }
                    if y > 0 {
                        div -= vy[i - nx];
                    }
                    if z > 0 {
                        div -= vz[i - sxy];
                    }
                    let l = x + nx * y;
                    slab[l] = (slab[l] - h * bulk[i] * div) * keep[i];
                }
            }
        });

        let t = (self.steps_done + 1) as f32 * self.dt;
        for e in &self.emitters {
            let s = e.value(t, self.dt);
            if s != 0.0 {
                for k in 0..8 {
                    self.p[e.cells[k]] += s * e.weights[k];
                }
            }
        }

        self.steps_done += 1;

        let a = self.ema;
        self.energy.par_iter_mut().zip(self.p.par_iter()).for_each(|(e, &v)| *e += a * (v * v - *e));

        for (sig, (cells, w)) in self.mic_signal.iter_mut().zip(&self.mics) {
            let mut v = 0.0;
            for k in 0..8 {
                v += self.p[cells[k]] * w[k];
            }
            sig.push(v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(dt: f32) -> WaveSettings {
        WaveSettings { c_air: 343.0, c_body_ratio: 1.0, rho_body: 1.0, damping: 0.0, sponge: 4, dt, drive_tau: 0.01 }
    }

    #[test]
    fn pulse_travels_at_sound_speed() {
        let dt = 1.0 / 22050.0;
        let dx = spacing_for(343.0, dt);
        let grid = Grid { nx: 48, ny: 48, nz: 48, origin: [0.0; 3], dx };
        let inside = vec![false; grid.len()];
        let centre = [24.0 * dx; 3];
        let src = Source { amp: 1.0, ..Source::new("s", centre, Waveform::Impulse, 2000.0) };
        let r = 10.0 * dx;
        let mic = [centre[0] + r, centre[1], centre[2]];
        let mut f = WaveField::new(grid, &inside, &settings(dt), &[src], &[mic]);
        f.step(120);
        let sig = &f.mic_signal[0];
        let peak = sig.iter().enumerate().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).unwrap().0;
        let emission_delay = 6.0;
        let expected = emission_delay + (r / 343.0) / dt;
        assert!((peak as f32 - expected).abs() < 5.0, "peak {} expected {}", peak, expected);
    }

    #[test]
    fn stays_bounded() {
        let dt = 1.0 / 22050.0;
        let dx = spacing_for(343.0, dt);
        let grid = Grid { nx: 24, ny: 24, nz: 24, origin: [0.0; 3], dx };
        let inside: Vec<bool> = (0..grid.len()).map(|i| grid.coords(i).0 > 12).collect();
        let mut s = settings(dt);
        s.rho_body = 10.0;
        s.c_body_ratio = 3.0;
        s.damping = 0.0;
        let src = Source::new("s", [8.0 * dx; 3], Waveform::Ricker, 1500.0);
        let mut f = WaveField::new(grid, &inside, &s, &[src], &[]);
        f.step(1500);
        let early = f.p.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        f.step(2500);
        let late = f.p.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(late.is_finite() && late <= early * 1.2, "field grew: {early} -> {late}");
    }
}
