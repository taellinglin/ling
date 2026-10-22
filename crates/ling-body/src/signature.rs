//! Acoustic signature of a mesh: the body is voxelised as a dense scatterer, an
//! impulse is fired in front of the mouth, and two microphones just outside the head
//! record the response. The impulse response is then convolved with a knock or a
//! voice-like tone so the shape can be heard, and summarised as a spectrum and a list
//! of resonances.

use crate::analysis;
use crate::fft::Spectrum;
use crate::grid::Grid;
use crate::mesh::{Mesh, Tris};
use crate::params::{Params, Source, Waveform};
use crate::sdf;
use crate::wave::{spacing_for, WaveField, WaveSettings};
use crate::wavio;
use rustfft::num_complex::Complex32;
use rustfft::FftPlanner;
use std::time::Instant;

pub struct Signature {
    wave: WaveField,
    total: usize,
    rate: u32,
    excite: u8,
}

pub struct Rendered {
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    pub rate: u32,
    /// Log-spaced band levels of the impulse response, 0..1.
    pub spectrum: Vec<f32>,
    pub notes: Vec<String>,
}

/// Where the mouth and the ears are, found from the mesh itself: the head is the
/// topmost tenth of the body and the figure faces +Z.
fn head_points(m: &Mesh) -> ([f32; 3], f32, [f32; 3]) {
    let (lo, hi) = m.bounds().unwrap_or(([0.0; 3], [0.0, 1.7, 0.0]));
    let height = hi[1] - lo[1];
    let band = |y0: f32, y1: f32| m.pos.as_chunks::<3>().0.iter().filter(move |p| p[1] >= y0 && p[1] <= y1 && p[0].abs() < 0.12 * height);
    let head: Vec<&[f32; 3]> = band(hi[1] - 0.12 * height, hi[1]).collect();
    let (cx, cz) = if head.is_empty() {
        (0.0, (lo[2] + hi[2]) * 0.5)
    } else {
        let n = head.len() as f32;
        (head.iter().map(|p| p[0]).sum::<f32>() / n, head.iter().map(|p| p[2]).sum::<f32>() / n)
    };
    let half = head.iter().map(|p| (p[0] - cx).abs()).fold(0.05 * height, f32::max);
    let front = head.iter().map(|p| p[2]).fold(cz + 0.05 * height, f32::max);
    let mouth_y = hi[1] - 0.075 * height;
    ([cx, mouth_y, front + 0.04], half, [cx, hi[1] - 0.06 * height, cz])
}

impl Signature {
    pub fn begin(mesh: &Mesh, p: &Params) -> Result<Signature, String> {
        if mesh.idx.is_empty() {
            return Err("no mesh to listen to; preview a body or simulate first".into());
        }
        let sr = p.getf("sig_sr");
        let dt = 1.0 / sr;
        let dx = spacing_for(p.getf("c_air"), dt);
        let (lo, hi) = mesh.bounds().ok_or("mesh has no vertices")?;
        let grid = Grid::fit(lo, hi, dx, 14);
        if grid.len() > 20_000_000 {
            return Err("signature grid is too large; lower sig_sr".into());
        }
        let mut tris = Tris::default();
        for t in mesh.idx.as_chunks::<3>().0 {
            let v = |i: u32| {
                let o = i as usize * 3;
                [mesh.pos[o], mesh.pos[o + 1], mesh.pos[o + 2]]
            };
            tris.push(v(t[0]), v(t[1]), v(t[2]));
        }
        let phi = sdf::from_tris(&tris, &grid, 1);
        let inside: Vec<bool> = phi.iter().map(|&v| v < 0.0).collect();

        let (mouth, half, centre) = head_points(mesh);
        let ear = half + 0.03;
        let mics = [[centre[0] + ear, centre[1], centre[2]], [centre[0] - ear, centre[1], centre[2]]];
        let mut src = Source::new("mouth", mouth, Waveform::Impulse, 1000.0);
        src.amp = 1.0;
        let settings = WaveSettings {
            c_air: p.getf("c_air"),
            c_body_ratio: 1.0,
            rho_body: p.getf("sig_rho"),
            damping: 0.0003,
            sponge: 8,
            dt,
            drive_tau: 0.01,
        };
        let wave = WaveField::new(grid, &inside, &settings, &[src], &mics);
        let total = (p.get("sig_dur") * sr as f64).round().max(64.0) as usize;
        Ok(Signature { wave, total, rate: sr.round() as u32, excite: p.get("sig_excite").round() as u8 })
    }

    pub fn total(&self) -> usize {
        self.total
    }

    pub fn done(&self) -> bool {
        self.wave.steps_done >= self.total
    }

    pub fn steps_done(&self) -> usize {
        self.wave.steps_done
    }

    /// Runs for about `budget_ms`; always makes progress.
    pub fn advance(&mut self, budget_ms: f32) {
        let start = Instant::now();
        while !self.done() {
            let n = (self.total - self.wave.steps_done).min(32);
            self.wave.step(n);
            if start.elapsed().as_secs_f32() * 1000.0 >= budget_ms {
                break;
            }
        }
    }

    /// Convolves the recorded response with the chosen excitation and writes a 16-bit WAV.
    pub fn render(&self, path: &str) -> Result<Rendered, String> {
        let ir = &self.wave.mic_signal;
        if ir.len() != 2 || ir[0].is_empty() {
            return Err("signature has not run".into());
        }
        let rate = self.rate;
        let source = excitation(self.excite, rate);
        let (mut left, mut right) = (convolve(&source, &ir[0]), convolve(&source, &ir[1]));
        let peak = left.iter().chain(&right).fold(1e-12f32, |m, v| m.max(v.abs()));
        left.iter_mut().chain(right.iter_mut()).for_each(|v| *v *= 0.9 / peak);
        let bytes = wavio::encode(rate, &[left.clone(), right.clone()], 16, &[]);
        crate::export::write(path, &bytes)?;

        let mono: Vec<f32> = ir[0].iter().zip(&ir[1]).map(|(a, b)| 0.5 * (a + b)).collect();
        let (spectrum, notes) = summarise(&mono, rate);
        Ok(Rendered { left, right, rate, spectrum, notes })
    }
}

/// The sound sent through the body: 0 impulse, 1 noise knock, 2 voice-like tone.
fn excitation(kind: u8, rate: u32) -> Vec<f32> {
    let r = rate as f32;
    match kind {
        1 => {
            let n = (0.04 * r) as usize;
            let mut state = 0x1234_5678u32;
            (0..n)
                .map(|i| {
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    let noise = (state >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0;
                    noise * (-(i as f32) / (0.008 * r)).exp()
                })
                .collect()
        },
        2 => {
            let n = (0.6 * r) as usize;
            let mut phase = 0.0f32;
            (0..n)
                .map(|i| {
                    let t = i as f32 / r;
                    let f0 = 140.0 * (1.0 + 0.012 * (2.0 * std::f32::consts::PI * 5.5 * t).sin()) * (1.0 + 0.15 * (-t * 6.0).exp());
                    phase = (phase + f0 / r).fract();
                    let env = (t / 0.03).min(1.0) * (1.0 - t / 0.6).max(0.0);
                    let saw: f32 = (1..=12).map(|h| (2.0 * std::f32::consts::PI * h as f32 * phase).sin() / h as f32).sum();
                    saw * env * 0.5
                })
                .collect()
        },
        _ => vec![1.0],
    }
}

fn convolve(a: &[f32], b: &[f32]) -> Vec<f32> {
    let n = (a.len() + b.len()).next_power_of_two();
    let mut planner = FftPlanner::<f32>::new();
    let (fwd, inv) = (planner.plan_fft_forward(n), planner.plan_fft_inverse(n));
    let prep = |x: &[f32]| {
        let mut v: Vec<Complex32> = x.iter().map(|&s| Complex32::new(s, 0.0)).collect();
        v.resize(n, Complex32::new(0.0, 0.0));
        fwd.process(&mut v);
        v
    };
    let (fa, fb) = (prep(a), prep(b));
    let mut prod: Vec<Complex32> = fa.iter().zip(&fb).map(|(x, y)| x * y).collect();
    inv.process(&mut prod);
    prod.iter().take(a.len() + b.len() - 1).map(|c| c.re / n as f32).collect()
}

const BANDS: usize = 48;

/// Log-spaced band levels and the strongest resonances of an impulse response.
fn summarise(ir: &[f32], rate: u32) -> (Vec<f32>, Vec<String>) {
    let n = ir.len().next_power_of_two().clamp(256, 8192);
    let sp = Spectrum::new(n);
    let bins = sp.bins(ir);
    let power: Vec<f32> = bins.iter().map(|c| c.norm_sqr()).collect();
    let nyq = rate as f32 * 0.5;
    let (lo, hi) = (80.0f32, nyq * 0.92);
    let mut bands = vec![0.0f32; BANDS];
    for (k, p) in power.iter().enumerate() {
        let f = sp.bin_freq(k as f32, rate as f32);
        if f >= lo && f < hi {
            let b = (((f / lo).ln() / (hi / lo).ln()) * BANDS as f32) as usize;
            bands[b.min(BANDS - 1)] = bands[b.min(BANDS - 1)].max(p.sqrt());
        }
    }
    let top = bands.iter().cloned().fold(1e-12, f32::max);
    bands.iter_mut().for_each(|b| *b /= top);
    let peaks = analysis::spectral_peaks(&power, &sp, rate as f32);
    let mut notes = vec![format!("signature: {} resonances", peaks.len())];
    notes.extend(peaks.iter().map(|p| format!("resonance {:.0} Hz  level {:.2}", p.freq, (p.power / peaks[0].power).sqrt())));
    (bands, notes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesher;

    fn sphere_mesh(r: f32) -> Mesh {
        let g = Grid { nx: 60, ny: 80, nz: 60, origin: [-0.6, 0.0, -0.6], dx: 0.025 };
        let phi: Vec<f32> = (0..g.len())
            .map(|i| {
                let (x, y, z) = g.coords(i);
                let p = g.world(x, y, z);
                (p[0] * p[0] + (p[1] - 1.0).powi(2) + p[2] * p[2]).sqrt() - r
            })
            .collect();
        mesher::build(&phi, &g, 0.0, 1, 0.0)
    }

    #[test]
    fn convolving_with_an_impulse_returns_the_response() {
        let ir = [0.5, 0.25, -0.125, 0.0625];
        let out = convolve(&[1.0], &ir);
        for (a, b) in out.iter().zip(&ir) {
            assert!((a - b).abs() < 1e-5);
        }
    }

    #[test]
    fn a_body_scatters_sound_so_the_ears_hear_something_that_depends_on_its_size() {
        let mut p = Params::default();
        p.set("sig_dur", 0.05);
        p.set("sig_sr", 12000.0);
        let run = |r: f32| {
            let mut s = Signature::begin(&sphere_mesh(r), &p).unwrap();
            while !s.done() {
                s.advance(1000.0);
            }
            let path = std::env::temp_dir().join(format!("ling_body_sig_{r}.wav"));
            let out = s.render(path.to_str().unwrap()).unwrap();
            let _ = std::fs::remove_file(path);
            out
        };
        let small = run(0.12);
        let large = run(0.30);
        assert_eq!(small.left.len(), large.left.len());
        assert!(small.left.iter().any(|v| v.abs() > 0.1), "microphones heard nothing");
        let diff: f32 = small.left.iter().zip(&large.left).map(|(a, b)| (a - b).abs()).sum();
        assert!(diff > 1.0, "signature should change with the shape");
        assert_eq!(small.spectrum.len(), BANDS);
    }
}
