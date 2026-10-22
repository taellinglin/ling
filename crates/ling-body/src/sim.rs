//! Simulation orchestration: builds the rest body on the fine membrane lattice,
//! couples the coarse wave field to the membrane and advances both in lock-step.

use crate::grid::Grid;
use crate::humanoid::Humanoid;
use crate::membrane::{Membrane, MembraneCfg};
use crate::mesh::Tris;
use crate::params::Params;
use crate::sdf;
use crate::wave::{spacing_for, WaveField, WaveSettings};
use std::time::Instant;

const MAX_FINE_CELLS: usize = 80_000_000;

pub struct Rest {
    pub grid: Grid,
    pub phi0: Vec<f32>,
    pub rig: Humanoid,
    pub up: usize,
    pub dt: f32,
}

fn lattice_up(n: usize, up: usize) -> usize {
    (n - 1).div_ceil(up) * up + 1
}

/// Voxelises the body (scaffold or imported triangles) on the membrane lattice.
/// Imported triangles must already be normalised to the body height.
pub fn build_rest(params: &Params, body: Option<&Tris>) -> Result<Rest, String> {
    let sr = params.getf("sr");
    let dt = 1.0 / sr;
    let c_max = params.getf("c_air") * params.getf("c_body").max(1.0);
    let up = params.get("up").round().max(1.0) as usize;
    let dx_wave = spacing_for(c_max, dt);
    let dx = dx_wave / up as f32;

    let (rig, lo, hi) = match body {
        Some(t) => {
            let (lo, hi) = t.bounds().ok_or("body has no triangles")?;
            (Humanoid::fit(lo, hi), lo, hi)
        },
        None => {
            let h = Humanoid::new(
                params.getf("height"),
                params.getf("build"),
                params.getf("arm_span"),
                params.getf("head_scale"),
            );
            let (lo, hi) = h.bounds();
            (h, lo, hi)
        },
    };
    let pad = params.get("margin").round() as usize * up;
    let mut grid = Grid::fit(lo, hi, dx, pad);
    grid.nx = lattice_up(grid.nx, up);
    grid.ny = lattice_up(grid.ny, up);
    grid.nz = lattice_up(grid.nz, up);
    if grid.len() > MAX_FINE_CELLS {
        return Err(format!(
            "grid of {}x{}x{} cells is too large; lower sr or margin",
            grid.nx, grid.ny, grid.nz
        ));
    }
    let phi0 = match body {
        Some(t) => sdf::from_tris(t, &grid, params.get("seal").round().max(1.0) as usize),
        None => rig.fill(&grid),
    };
    Ok(Rest { grid, phi0, rig, up, dt })
}

pub struct Sim {
    pub wave: WaveField,
    pub membrane: Membrane,
    coarse: Grid,
    near_surface: Vec<u32>,
    drive: Vec<f32>,
    pub total: usize,
    every: usize,
    tau: f32,
    cfg: MembraneCfg,
    energy_mode: bool,
    reinit_every: usize,
    membrane_calls: usize,
    band_cells: f32,
}

impl Sim {
    pub fn begin(params: &Params, rest: &Rest) -> Result<Sim, String> {
        if params.sources.is_empty() {
            return Err("add at least one acoustic source".into());
        }
        let up = rest.up;
        let fine = rest.grid;
        let coarse = fine.coarsen(up);
        let mut inside = Vec::with_capacity(coarse.len());
        let mut near = Vec::new();
        for i in 0..coarse.len() {
            let (x, y, z) = coarse.coords(i);
            let v = rest.phi0[fine.idx(x * up, y * up, z * up)];
            inside.push(v < 0.0);
            if v.abs() < 3.0 * coarse.dx {
                near.push(i as u32);
            }
        }
        if near.is_empty() {
            return Err("body surface not found on the wave grid".into());
        }

        let head = rest.rig.joints[rest.rig.joint_index("head").unwrap_or(0)].pos;
        let sep = params.getf("mic_sep") * 0.5;
        let guard = (params.get("sponge") + 1.0) as f32 * coarse.dx;
        let place = |side: f32| {
            let want = [head[0] + side * sep, head[1], head[2] + params.getf("mic_dist")];
            let lo = [coarse.origin[0] + guard, coarse.origin[1] + guard, coarse.origin[2] + guard];
            let hi = [
                coarse.origin[0] + (coarse.nx - 1) as f32 * coarse.dx - guard,
                coarse.origin[1] + (coarse.ny - 1) as f32 * coarse.dx - guard,
                coarse.origin[2] + (coarse.nz - 1) as f32 * coarse.dx - guard,
            ];
            [0, 1, 2].map(|k| want[k].clamp(lo[k], hi[k].max(lo[k])))
        };

        let settings = WaveSettings {
            c_air: params.getf("c_air"),
            c_body_ratio: params.getf("c_body"),
            rho_body: params.getf("rho_body"),
            damping: params.getf("damping"),
            sponge: params.get("sponge").round() as usize,
            dt: rest.dt,
            drive_tau: params.getf("drive_tau"),
        };
        let wave = WaveField::new(coarse, &inside, &settings, &params.sources, &[place(1.0), place(-1.0)]);
        let total = params.get("steps").round().max(1.0) as usize;
        let every = params.get("mem_every").round().max(1.0) as usize;
        let calls = total.div_ceil(every) as f32;
        Ok(Sim {
            wave,
            membrane: Membrane::new(fine, rest.phi0.clone()),
            coarse,
            near_surface: near,
            drive: vec![0.0; coarse.len()],
            total,
            every,
            tau: params.getf("mem_time") / calls,
            cfg: MembraneCfg {
                tension: params.getf("tension"),
                coupling: params.getf("coupling"),
                threshold: params.getf("threshold"),
                stiffness: params.getf("stiffness"),
                relief: params.getf("relief"),
                band_cells: params.getf("band"),
            },
            energy_mode: params.get("drive_mode") >= 0.5,
            reinit_every: params.get("reinit_every").round().max(1.0) as usize,
            membrane_calls: 0,
            band_cells: params.getf("band"),
        })
    }

    pub fn done(&self) -> bool {
        self.wave.steps_done >= self.total
    }

    pub fn steps_done(&self) -> usize {
        self.wave.steps_done
    }

    /// Normalised acoustic drive on the coarse lattice, in [0, 2) for the energy mode
    /// (excess of the field energy over its mean at the skin, so quiet skin stays put)
    /// and (-2, 2) for the instantaneous-pressure mode (pressure over its rms).
    fn refresh_drive(&mut self) {
        let n = self.near_surface.len() as f32;
        if self.energy_mode {
            let src = &self.wave.energy;
            let mean: f32 = self.near_surface.iter().map(|&i| src[i as usize]).sum::<f32>() / n;
            if mean > 1e-30 {
                let inv = 1.0 / mean;
                for (d, e) in self.drive.iter_mut().zip(src) {
                    *d = 2.0 * (0.5 * (e * inv - 1.0).max(0.0)).tanh();
                }
            } else {
                self.drive.fill(0.0);
            }
        } else {
            let src = &self.wave.p;
            let rms = (self.near_surface.iter().map(|&i| src[i as usize].powi(2)).sum::<f32>() / n).sqrt();
            let inv = if rms > 1e-30 { 1.0 / rms } else { 0.0 };
            for (d, p) in self.drive.iter_mut().zip(src) {
                *d = 2.0 * (0.5 * p * inv).tanh();
            }
        }
    }

    fn membrane_update(&mut self) {
        self.refresh_drive();
        let (coarse, drive) = (self.coarse, &self.drive);
        self.membrane.step(&self.cfg, self.tau, |w| coarse.sample(drive, coarse.lattice(w)));
        self.membrane_calls += 1;
        if self.membrane_calls.is_multiple_of(self.reinit_every) {
            self.membrane.reinitialize(3, self.band_cells);
        }
    }

    /// Advances the simulation until `budget_ms` has elapsed or it finishes.
    /// Always performs at least one wave/membrane cycle.
    pub fn advance(&mut self, budget_ms: f32) {
        let start = Instant::now();
        loop {
            let left = self.total - self.wave.steps_done;
            let n = left.min(self.every);
            if n == 0 {
                break;
            }
            self.wave.step(n);
            self.membrane_update();
            if self.done() {
                self.membrane.reinitialize(6, self.band_cells);
                break;
            }
            if start.elapsed().as_secs_f32() * 1000.0 >= budget_ms {
                break;
            }
        }
    }

    /// Pressure on an axis-aligned plane of the wave grid, row-major as `(width, height, values)`.
    /// axis 0 = x (plane spans z by y), 1 = y (x by z), 2 = z (x by y).
    pub fn slice(&self, axis: usize, frac: f32) -> (usize, usize, Vec<f32>) {
        let g = &self.coarse;
        let at = |n: usize| ((frac.clamp(0.0, 1.0) * (n - 1) as f32).round() as usize).min(n - 1);
        let p = &self.wave.p;
        match axis {
            0 => {
                let x = at(g.nx);
                let v = (0..g.ny).flat_map(|y| (0..g.nz).map(move |z| (y, z))).map(|(y, z)| p[g.idx(x, y, z)]).collect();
                (g.nz, g.ny, v)
            },
            1 => {
                let y = at(g.ny);
                let v = (0..g.nz).flat_map(|z| (0..g.nx).map(move |x| (x, z))).map(|(x, z)| p[g.idx(x, y, z)]).collect();
                (g.nx, g.nz, v)
            },
            _ => {
                let z = at(g.nz);
                let v = (0..g.ny).flat_map(|y| (0..g.nx).map(move |x| (x, y))).map(|(x, y)| p[g.idx(x, y, z)]).collect();
                (g.nx, g.ny, v)
            },
        }
    }

    /// The acoustic field sampled at world points: energy over its mean at the skin
    /// (`energy`), or pressure over its rms at the skin.
    pub fn surface_field(&self, pts: &[[f32; 3]], energy: bool) -> Vec<f32> {
        let n = self.near_surface.len().max(1) as f32;
        let src = if energy { &self.wave.energy } else { &self.wave.p };
        let at = |i: &u32| src[*i as usize];
        let scale = if energy {
            self.near_surface.iter().map(at).sum::<f32>() / n
        } else {
            (self.near_surface.iter().map(|i| at(i).powi(2)).sum::<f32>() / n).sqrt()
        };
        let inv = if scale > 1e-30 { 1.0 / scale } else { 0.0 };
        pts.iter().map(|p| self.coarse.sample(src, self.coarse.lattice(*p)) * inv).collect()
    }

    pub fn coarse_grid(&self) -> Grid {
        self.coarse
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{Source, Waveform};

    #[test]
    fn small_body_simulates_and_meshes() {
        let mut p = Params::default();
        p.set("sr", 11025.0);
        p.set("steps", 150.0);
        p.set("margin", 10.0);
        p.set("up", 2.0);
        p.set("sponge", 6.0);
        p.sources.push(Source::new("chest", [0.0, 1.25, 0.0], Waveform::Ricker, 400.0));
        let rest = build_rest(&p, None).unwrap();
        let mut sim = Sim::begin(&p, &rest).unwrap();
        while !sim.done() {
            sim.advance(1000.0);
        }
        assert!(sim.wave.mic_signal[0].len() == 150);
        assert!(sim.wave.mic_signal[0].iter().any(|v| v.abs() > 0.0));
        let g = sim.membrane.grid;
        let m = crate::mesher::build(&sim.membrane.phi, &g, 0.0, 2, 0.0);
        assert!(m.is_watertight(), "{} non-manifold edges", m.non_manifold_edges());
        assert!(m.tri_count() > 1000);
    }
}
