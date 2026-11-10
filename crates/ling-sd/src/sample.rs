//! k-diffusion samplers on SD's eps-prediction noise schedule (scaled-linear betas, 1000 steps) -- the same maths as the host
//! engine's `sampling.rs` (and the WebUI's), on flat f32 latents.
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sampler {
    EulerA,
    Euler,
    Dpmpp2m,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Schedule {
    Uniform,
    Karras,
}

pub const SAMPLERS: &[&str] = &["Euler a", "Euler", "DPM++ 2M Karras", "DPM++ 2M"];

/// "Euler a", "DPM++ 2M Karras", "DPM++ 2M" (+ "Karras" / "Uniform" suffixes) -> sampler, schedule, the WebUI's name.
pub fn parse(name: &str) -> (Sampler, Schedule, &'static str) {
    let n = name.trim().to_ascii_lowercase();
    let karras = n.ends_with("karras");
    let base = n.trim_end_matches("karras").trim_end_matches("uniform").trim();
    match base {
        "euler" => (Sampler::Euler, if karras { Schedule::Karras } else { Schedule::Uniform }, if karras { "Euler Karras" } else { "Euler" }),
        "dpm++ 2m" | "dpmpp_2m" => {
            if n.ends_with("uniform") {
                (Sampler::Dpmpp2m, Schedule::Uniform, "DPM++ 2M")
            } else {
                // the WebUI's DPM++ 2M defaults to the Karras schedule (as the host engine's parse does)
                (Sampler::Dpmpp2m, Schedule::Karras, if karras { "DPM++ 2M Karras" } else { "DPM++ 2M" })
            }
        },
        _ => (Sampler::EulerA, if karras { Schedule::Karras } else { Schedule::Uniform }, "Euler a"),
    }
}

pub struct Table {
    log_sigmas: Vec<f64>,
}

impl Default for Table {
    fn default() -> Self {
        Self::sd()
    }
}

impl Table {
    pub fn sd() -> Self {
        let (b0, b1) = (libm::sqrt(0.00085f64), libm::sqrt(0.012f64));
        let mut ac = 1.0f64;
        let mut log_sigmas = Vec::with_capacity(1000);
        for i in 0..1000 {
            let b = b0 + (b1 - b0) * i as f64 / 999.0;
            ac *= 1.0 - b * b;
            log_sigmas.push(libm::log(libm::sqrt((1.0 - ac) / ac)));
        }
        Table { log_sigmas }
    }
    pub fn sigma_min(&self) -> f64 {
        libm::exp(self.log_sigmas[0])
    }
    pub fn sigma_max(&self) -> f64 {
        libm::exp(self.log_sigmas[999])
    }
    pub fn t_to_sigma(&self, t: f64) -> f64 {
        let t = t.clamp(0.0, 999.0);
        let lo = libm::floor(t) as usize;
        let hi = (libm::ceil(t) as usize).min(999);
        let w = t - lo as f64;
        libm::exp((1.0 - w) * self.log_sigmas[lo] + w * self.log_sigmas[hi])
    }
    pub fn sigma_to_t(&self, sigma: f64) -> f64 {
        let ls = libm::log(sigma.max(1e-10));
        if ls <= self.log_sigmas[0] {
            return 0.0;
        }
        if ls >= self.log_sigmas[999] {
            return 999.0;
        }
        let mut lo = 0;
        while lo < 998 && self.log_sigmas[lo + 1] <= ls {
            lo += 1;
        }
        let (a, b) = (self.log_sigmas[lo], self.log_sigmas[lo + 1]);
        lo as f64 + (ls - a) / (b - a)
    }

    /// `n` steps -> n + 1 sigmas, high to low, ending in 0.
    pub fn sigmas(&self, n: usize, schedule: Schedule) -> Vec<f64> {
        let n = n.max(1);
        let (smin, smax) = (self.sigma_min(), self.sigma_max());
        let mut out: Vec<f64> = match schedule {
            Schedule::Uniform => (0..n)
                .map(|i| {
                    let t = if n == 1 { 999.0 } else { 999.0 * (1.0 - i as f64 / (n - 1) as f64) };
                    self.t_to_sigma(t)
                })
                .collect(),
            Schedule::Karras => {
                let rho = 7.0;
                let (mi, ma) = (libm::pow(smin, 1.0 / rho), libm::pow(smax, 1.0 / rho));
                (0..n)
                    .map(|i| {
                        let r = if n == 1 { 0.0 } else { i as f64 / (n - 1) as f64 };
                        libm::pow(ma + r * (mi - ma), rho)
                    })
                    .collect()
            },
        };
        out.push(0.0);
        out
    }
}

/// The denoiser and the noise source the samplers need; `step` is told each finished step (false stops early).
pub trait Host {
    fn denoise(&mut self, x: &[f32], sigma: f64) -> Vec<f32>;
    fn noise(&mut self, n: usize) -> Vec<f32>;
    fn step(&mut self, i: usize, denoised: &[f32]) -> bool;
}

fn ancestral(s: f64, s_next: f64) -> (f64, f64) {
    if s_next == 0.0 {
        return (0.0, 0.0);
    }
    let up = libm::sqrt((s_next * s_next * (s * s - s_next * s_next) / (s * s)).max(0.0)).min(s_next);
    let down = libm::sqrt((s_next * s_next - up * up).max(0.0));
    (down, up)
}

/// Run the sampler from `x` (already scaled by sigmas[0]); None when stopped.
pub fn run(sampler: Sampler, sigmas: &[f64], mut x: Vec<f32>, h: &mut dyn Host) -> Option<Vec<f32>> {
    let n = sigmas.len() - 1;
    let mut old_den: Option<Vec<f32>> = None;
    let mut h_last: Option<f64> = None;
    for i in 0..n {
        let (s, sn) = (sigmas[i], sigmas[i + 1]);
        let den = h.denoise(&x, s);
        match sampler {
            Sampler::Euler => {
                for (xv, dv) in x.iter_mut().zip(den.iter()) {
                    let d = (*xv - *dv) as f64 / s;
                    *xv = (*xv as f64 + d * (sn - s)) as f32;
                }
            },
            Sampler::EulerA => {
                let (sd, su) = ancestral(s, sn);
                for (xv, dv) in x.iter_mut().zip(den.iter()) {
                    let d = (*xv - *dv) as f64 / s;
                    *xv = (*xv as f64 + d * (sd - s)) as f32;
                }
                if sn > 0.0 {
                    let z = h.noise(x.len());
                    for (xv, zv) in x.iter_mut().zip(z.iter()) {
                        *xv += (*zv as f64 * su) as f32;
                    }
                }
            },
            Sampler::Dpmpp2m => {
                if sn == 0.0 {
                    x.copy_from_slice(&den);
                } else {
                    let t = -libm::log(s);
                    let tn = -libm::log(sn);
                    let hh = tn - t;
                    let k = -libm::expm1(-hh);
                    let ratio = sn / s;
                    match (&old_den, h_last) {
                        (Some(od), Some(hl)) => {
                            let r = hl / hh;
                            let (a, b) = (1.0 + 1.0 / (2.0 * r), 1.0 / (2.0 * r));
                            for j in 0..x.len() {
                                let d = den[j] as f64 * a - od[j] as f64 * b;
                                x[j] = (x[j] as f64 * ratio + d * k) as f32;
                            }
                        },
                        _ => {
                            for j in 0..x.len() {
                                x[j] = (x[j] as f64 * ratio + den[j] as f64 * k) as f32;
                            }
                        },
                    }
                    h_last = Some(hh);
                }
                old_den = Some(den.clone());
            },
        }
        if !h.step(i, &den) {
            return None;
        }
    }
    Some(x)
}
