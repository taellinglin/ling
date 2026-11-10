//! k-diffusion samplers on the SD (CompVis, eps-prediction) noise schedule — the same maths the
//! WebUI's "Euler a", "DPM++ 2M Karras" etc. run, so settings carry over.

use candle_core::{Result, Tensor};

pub const SAMPLERS: &[&str] = &["Euler a", "Euler", "Heun", "DPM++ 2M", "DPM++ 2M SDE", "DDIM"];
pub const SCHEDULES: &[&str] = &["Automatic", "Uniform", "Karras", "Exponential"];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sampler {
    EulerA,
    Euler,
    Heun,
    Dpmpp2m,
    Dpmpp2mSde,
    Ddim,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Schedule {
    Uniform,
    Karras,
    Exponential,
    Ddim,
}

/// "DPM++ 2M Karras" (the pre-1.9 WebUI spelling) or "DPM++ 2M" + schedule "Karras".
pub fn parse(sampler: &str, schedule: &str) -> (Sampler, Schedule, String, String) {
    let mut s = sampler.trim().to_string();
    let mut sch = schedule.trim().to_string();
    for suffix in ["Karras", "Exponential"] {
        if let Some(x) = s.strip_suffix(&format!(" {suffix}")) {
            s = x.to_string();
            sch = suffix.to_string();
        }
    }
    let sampler = match s.to_ascii_lowercase().as_str() {
        "euler" => Sampler::Euler,
        "heun" => Sampler::Heun,
        "dpm++ 2m" | "dpmpp_2m" => Sampler::Dpmpp2m,
        "dpm++ 2m sde" | "dpmpp_2m_sde" => Sampler::Dpmpp2mSde,
        "ddim" => Sampler::Ddim,
        _ => Sampler::EulerA,
    };
    let schedule = match sch.to_ascii_lowercase().as_str() {
        "karras" => Schedule::Karras,
        "exponential" => Schedule::Exponential,
        "uniform" | "normal" => Schedule::Uniform,
        _ => match sampler {
            Sampler::Dpmpp2m | Sampler::Dpmpp2mSde => Schedule::Karras,
            Sampler::Ddim => Schedule::Ddim,
            _ => Schedule::Uniform,
        },
    };
    let name = SAMPLERS[match sampler {
        Sampler::EulerA => 0,
        Sampler::Euler => 1,
        Sampler::Heun => 2,
        Sampler::Dpmpp2m => 3,
        Sampler::Dpmpp2mSde => 4,
        Sampler::Ddim => 5,
    }]
    .to_string();
    let sname = match schedule {
        Schedule::Uniform => "Uniform",
        Schedule::Karras => "Karras",
        Schedule::Exponential => "Exponential",
        Schedule::Ddim => "DDIM",
    }
    .to_string();
    (sampler, schedule, name, sname)
}

/// The discrete noise levels of the 1000-step training schedule (scaled-linear betas).
pub struct Table {
    log_sigmas: Vec<f64>,
}

impl Table {
    pub fn sd() -> Self {
        let (b0, b1) = (0.00085f64.sqrt(), 0.012f64.sqrt());
        let mut ac = 1.0f64;
        let mut log_sigmas = Vec::with_capacity(1000);
        for i in 0..1000 {
            let b = b0 + (b1 - b0) * i as f64 / 999.0;
            ac *= 1.0 - b * b;
            log_sigmas.push((((1.0 - ac) / ac).sqrt()).ln());
        }
        Self { log_sigmas }
    }
    pub fn sigma_min(&self) -> f64 {
        self.log_sigmas[0].exp()
    }
    pub fn sigma_max(&self) -> f64 {
        self.log_sigmas[999].exp()
    }
    pub fn t_to_sigma(&self, t: f64) -> f64 {
        let t = t.clamp(0.0, 999.0);
        let lo = t.floor() as usize;
        let hi = (t.ceil() as usize).min(999);
        let w = t - lo as f64;
        ((1.0 - w) * self.log_sigmas[lo] + w * self.log_sigmas[hi]).exp()
    }
    pub fn sigma_to_t(&self, sigma: f64) -> f64 {
        let ls = sigma.max(1e-10).ln();
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
        let w = (ls - a) / (b - a);
        lo as f64 + w
    }

    /// `n` steps -> n+1 sigmas, high to low, ending in 0.
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
                let (mi, ma) = (smin.powf(1.0 / rho), smax.powf(1.0 / rho));
                (0..n)
                    .map(|i| {
                        let r = if n == 1 { 0.0 } else { i as f64 / (n - 1) as f64 };
                        (ma + r * (mi - ma)).powf(rho)
                    })
                    .collect()
            }
            Schedule::Exponential => (0..n)
                .map(|i| {
                    let r = if n == 1 { 0.0 } else { i as f64 / (n - 1) as f64 };
                    (smax.ln() + r * (smin.ln() - smax.ln())).exp()
                })
                .collect(),
            Schedule::Ddim => {
                let c = (1000 / n).max(1);
                let mut ts: Vec<usize> = (0..n).map(|i| (i * c + 1).min(999)).collect();
                ts.reverse();
                ts.into_iter().map(|t| self.log_sigmas[t].exp()).collect()
            }
        };
        out.push(0.0);
        out
    }
}

/// Everything a sampler needs from the outside: the denoiser, fresh noise, and a per-step hook
/// (which gets the step index and the current x0 estimate; returning false stops early).
pub trait Host {
    fn denoise(&mut self, x: &Tensor, sigma: f64) -> Result<Tensor>;
    fn noise_like(&mut self, x: &Tensor) -> Result<Tensor>;
    fn step(&mut self, i: usize, denoised: &Tensor) -> bool;
}

fn ancestral(s: f64, s_next: f64) -> (f64, f64) {
    if s_next == 0.0 {
        return (0.0, 0.0);
    }
    let up = (s_next * s_next * (s * s - s_next * s_next) / (s * s)).max(0.0).sqrt().min(s_next);
    let down = (s_next * s_next - up * up).max(0.0).sqrt();
    (down, up)
}

pub fn run(sampler: Sampler, sigmas: &[f64], x: Tensor, h: &mut dyn Host) -> Result<Tensor> {
    let mut x = x;
    let n = sigmas.len() - 1;
    let mut old_denoised: Option<Tensor> = None;
    let mut h_last: Option<f64> = None;
    for i in 0..n {
        let (s, sn) = (sigmas[i], sigmas[i + 1]);
        let den = h.denoise(&x, s)?;
        match sampler {
            Sampler::Euler | Sampler::Ddim => {
                let d = ((&x - &den)? / s)?;
                x = (&x + (d * (sn - s))?)?;
            }
            Sampler::EulerA => {
                let (sd, su) = ancestral(s, sn);
                let d = ((&x - &den)? / s)?;
                x = (&x + (d * (sd - s))?)?;
                if sn > 0.0 {
                    let z = h.noise_like(&x)?;
                    x = (x + (z * su)?)?;
                }
            }
            Sampler::Heun => {
                let d = ((&x - &den)? / s)?;
                let dt = sn - s;
                if sn == 0.0 {
                    x = (&x + (d * dt)?)?;
                } else {
                    let x2 = (&x + (&d * dt)?)?;
                    let den2 = h.denoise(&x2, sn)?;
                    let d2 = ((&x2 - &den2)? / sn)?;
                    x = (&x + (((d + d2)? * 0.5)? * dt)?)?;
                }
            }
            Sampler::Dpmpp2m => {
                let t = -s.ln();
                if sn == 0.0 {
                    x = den.clone();
                } else {
                    let tn = -sn.ln();
                    let hh = tn - t;
                    let k = -(-hh).exp_m1(); // 1 - e^-h
                    let d = match (&old_denoised, h_last) {
                        (Some(od), Some(hl)) => {
                            let r = hl / hh;
                            ((&den * (1.0 + 1.0 / (2.0 * r)))? - (od * (1.0 / (2.0 * r)))?)?
                        }
                        _ => den.clone(),
                    };
                    x = ((&x * (sn / s))? + (d * k)?)?;
                    h_last = Some(hh);
                }
                old_denoised = Some(den.clone());
            }
            Sampler::Dpmpp2mSde => {
                if sn == 0.0 {
                    x = den.clone();
                } else {
                    let eta = 1.0;
                    let hh = (-sn.ln()) - (-s.ln());
                    let eta_h = eta * hh;
                    let k = -(-hh - eta_h).exp_m1();
                    x = ((&x * ((sn / s) * (-eta_h).exp()))? + (&den * k)?)?;
                    if let (Some(od), Some(hl)) = (&old_denoised, h_last) {
                        let r = hl / hh;
                        x = (x + ((&den - od)? * (0.5 * k / r))?)?;
                    }
                    let z = h.noise_like(&x)?;
                    x = (x + (z * (sn * (-(-2.0 * eta_h).exp_m1()).max(0.0).sqrt()))?)?;
                    h_last = Some(hh);
                }
                old_denoised = Some(den.clone());
            }
        }
        if !h.step(i, &den) {
            break;
        }
    }
    Ok(x)
}
