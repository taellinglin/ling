//! Audio to acoustic-source parameters.
//!
//! Heuristic inverse of the forward model, good for a rough body rather than an
//! exact one: spectral peaks become sources (height from log-frequency, left/right
//! offset from stereo balance), the envelope gives onset and damping, spectral
//! flatness decides whether a broadband impulse is present, and brightness and
//! loudness set the membrane tension and coupling.

use crate::fft::Spectrum;
use crate::params::{Params, Source, Waveform};

#[derive(Clone, Debug)]
pub struct Peak {
    pub freq: f32,
    pub power: f32,
    pub bin: usize,
    pub width_ratio: f32,
}

pub struct Analysis {
    pub params: Params,
    pub peaks: Vec<Peak>,
    pub notes: Vec<String>,
}

const MAX_SOURCES: usize = 6;
const MIN_HZ: f32 = 40.0;

fn welch(sp: &Spectrum, x: &[f32]) -> Vec<f32> {
    let n = sp.n;
    let hop = n / 2;
    let mut acc = vec![0.0f32; n / 2 + 1];
    let mut frames = 0.0f32;
    let mut at = 0;
    while at + n <= x.len() || (frames == 0.0 && at == 0) {
        for (a, b) in acc.iter_mut().zip(sp.bins(&x[at.min(x.len())..])) {
            *a += b.norm_sqr();
        }
        frames += 1.0;
        at += hop;
    }
    acc.iter_mut().for_each(|a| *a /= frames);
    acc
}

pub(crate) fn spectral_peaks(power: &[f32], sp: &Spectrum, rate: f32) -> Vec<Peak> {
    let max = power.iter().cloned().fold(0.0f32, f32::max);
    if max <= 0.0 {
        return Vec::new();
    }
    let lo = ((MIN_HZ * sp.n as f32 / rate).ceil() as usize).max(2);
    let hi = ((0.45 * sp.n as f32) as usize).min(power.len() - 2);
    let mut cand: Vec<Peak> = Vec::new();
    for k in lo..hi {
        if power[k] > power[k - 1] && power[k] >= power[k + 1] && power[k] > max * 1e-3 {
            let (a, b, c) = (power[k - 1].max(1e-30).ln(), power[k].ln(), power[k + 1].max(1e-30).ln());
            let denom = a - 2.0 * b + c;
            let off = if denom.abs() > 1e-12 { (0.5 * (a - c) / denom).clamp(-0.5, 0.5) } else { 0.0 };
            let half = power[k] * 0.5;
            let (mut l, mut r) = (k, k);
            while l > lo && power[l] > half {
                l -= 1;
            }
            while r < hi && power[r] > half {
                r += 1;
            }
            let freq = sp.bin_freq(k as f32 + off, rate);
            cand.push(Peak { freq, power: power[k], bin: k, width_ratio: sp.bin_freq((r - l) as f32, rate) / freq });
        }
    }
    cand.sort_by(|a, b| b.power.total_cmp(&a.power));
    let mut out: Vec<Peak> = Vec::new();
    for c in cand {
        if out.len() == MAX_SOURCES {
            break;
        }
        if out.iter().all(|p| (p.freq - c.freq).abs() > (0.06 * p.freq).max(MIN_HZ)) {
            out.push(c);
        }
    }
    out
}

fn flatness(power: &[f32], lo: usize, hi: usize) -> f32 {
    let slice = &power[lo..hi.max(lo + 1).min(power.len())];
    let mean = slice.iter().sum::<f32>() / slice.len() as f32;
    if mean <= 0.0 {
        return 0.0;
    }
    let log = slice.iter().map(|p| p.max(1e-30).ln()).sum::<f32>() / slice.len() as f32;
    log.exp() / mean
}

/// Frame RMS at a fixed hop.
fn envelope(x: &[f32], hop: usize) -> Vec<f32> {
    x.chunks(hop).map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt()).collect()
}

/// Exponential decay rate in 1/s from the falling edge between 80 % and 10 % of the peak.
fn decay_rate(env: &[f32], hop_s: f32) -> Option<f32> {
    let (pk, &max) = env.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))?;
    let pts: Vec<(f32, f32)> = env[pk..]
        .iter()
        .enumerate()
        .take_while(|(_, &v)| v > max * 0.1)
        .filter(|(_, &v)| v < max * 0.8)
        .map(|(i, &v)| (i as f32 * hop_s, v.ln()))
        .collect();
    if pts.len() < 3 {
        return None;
    }
    let n = pts.len() as f32;
    let (sx, sy) = pts.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
    let (sxx, sxy) = pts.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0 * p.0, a.1 + p.0 * p.1));
    let slope = (n * sxy - sx * sy) / (n * sxx - sx * sx).max(1e-12);
    (slope < 0.0).then_some(-slope)
}

/// First STFT frame where the bin's magnitude reaches a quarter of its maximum, with the phase there.
fn band_onset(sp: &Spectrum, x: &[f32], bin: usize, rate: f32) -> (f32, f32) {
    let hop = (sp.n / 4).max(1);
    let mut frames: Vec<(f32, f32)> = Vec::new();
    let mut at = 0;
    while at < x.len() {
        let b = sp.bins(&x[at..]);
        let c = b[bin.min(b.len() - 1)];
        frames.push((c.norm(), c.arg()));
        at += hop;
    }
    let max = frames.iter().fold(0.0f32, |m, f| m.max(f.0));
    let first = frames.iter().position(|f| f.0 >= 0.25 * max).unwrap_or(0);
    ((first * hop) as f32 / rate, frames[first].1)
}

pub fn analyze(left: &[f32], right: &[f32], rate: u32, base: &Params) -> Result<Analysis, String> {
    let len = left.len().min(right.len());
    if len < 64 {
        return Err("audio is too short to analyse".into());
    }
    let rate_f = rate as f32;
    let mono: Vec<f32> = (0..len).map(|i| 0.5 * (left[i] + right[i])).collect();
    let n = (len.next_power_of_two() / if len.is_power_of_two() { 1 } else { 2 }).clamp(256, 2048);
    let sp = Spectrum::new(n);
    let power = welch(&sp, &mono);
    let pl = welch(&sp, &left[..len]);
    let pr = welch(&sp, &right[..len]);
    let peaks = spectral_peaks(&power, &sp, rate_f);

    let mut params = base.clone();
    params.sources.clear();
    let mut notes = Vec::new();
    let height = params.getf("height");
    let half_span = 0.5 * 1.02 * height * params.getf("arm_span");
    let max_power = peaks.first().map_or(1.0, |p| p.power);

    let env_hop = 256usize;
    let env = envelope(&mono, env_hop);
    let env_peak = env.iter().cloned().fold(0.0f32, f32::max);
    let onset = env.iter().position(|&v| v >= 0.2 * env_peak).unwrap_or(0) as f32 * env_hop as f32 / rate_f;
    let alpha = decay_rate(&env, env_hop as f32 / rate_f);
    let decay_time = alpha.map_or(0.1, |a| (2.3 / a).clamp(0.01, 0.5));

    let sr_sim = rate_f.clamp(8000.0, 96000.0);
    for (i, pk) in peaks.iter().enumerate() {
        let (start, phase) = band_onset(&sp, &mono, pk.bin, rate_f);
        let lr = (pl[pk.bin] - pr[pk.bin]) / (pl[pk.bin] + pr[pk.bin]).max(1e-30);
        let t = ((pk.freq.max(MIN_HZ) / MIN_HZ).ln() / (4000.0f32 / MIN_HZ).ln()).clamp(0.0, 1.0);
        let y = height * (0.06 + 0.89 * t);
        let wave = if pk.width_ratio < 0.02 { Waveform::SineBurst } else { Waveform::Ricker };
        let mut s = Source::new(&format!("peak{i}"), [lr * half_span * 0.9, y, 0.0], wave, pk.freq.min(0.4 * sr_sim));
        s.amp = (pk.power / max_power).sqrt();
        s.phase = phase;
        s.start = start;
        s.dur = decay_time.max(4.0 / pk.freq);
        notes.push(format!("peak {:.1} Hz  level {:.2}  {}", pk.freq, s.amp, wave.name()));
        params.sources.push(s);
    }

    let lo = (MIN_HZ * sp.n as f32 / rate_f) as usize;
    let hi = (0.4 * sp.n as f32) as usize;
    let flat = flatness(&power, lo.max(1), hi);
    if flat > 0.25 || peaks.is_empty() {
        let mut s = Source::new("impulse", [0.0, 0.72 * height, 0.0], Waveform::Impulse, 1000.0);
        s.start = onset;
        notes.push(format!("broadband energy (flatness {flat:.2}) → impulse at chest"));
        params.sources.push(s);
    }

    let total: f32 = power.iter().sum::<f32>().max(1e-30);
    let centroid = power.iter().enumerate().map(|(k, p)| sp.bin_freq(k as f32, rate_f) * p).sum::<f32>() / total;
    let peak_abs = mono.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    params.set("sr", sr_sim as f64);
    params.set("tension", (0.2 + 2.5 * (centroid / (0.1 * rate_f)).clamp(0.0, 1.0)) as f64);
    params.set("coupling", (0.5 + 2.0 * peak_abs.clamp(0.0, 1.0)) as f64);
    if let Some(a) = alpha {
        params.set("damping", (2.0 * a / sr_sim) as f64);
        notes.push(format!("decay {a:.1} /s → damping {:.4}", params.get("damping")));
    }
    let steps = ((len as f32).min(rate_f * 0.25)).max(200.0);
    params.set("steps", steps as f64);
    notes.push(format!("centroid {centroid:.0} Hz → tension {:.2}", params.get("tension")));
    Ok(Analysis { params, peaks, notes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovers_two_tones_and_stereo_side() {
        let rate = 22050u32;
        let n = 16384;
        let tone = |f: f32, i: usize| (2.0 * std::f32::consts::PI * f * i as f32 / rate as f32).sin();
        let left: Vec<f32> = (0..n).map(|i| 0.8 * tone(300.0, i) + 0.2 * tone(2000.0, i)).collect();
        let right: Vec<f32> = (0..n).map(|i| 0.2 * tone(300.0, i) + 0.8 * tone(2000.0, i)).collect();
        let a = analyze(&left, &right, rate, &Params::default()).unwrap();
        let src = &a.params.sources;
        let low = src.iter().find(|s| (s.freq - 300.0).abs() < 15.0).expect("300 Hz source");
        let high = src.iter().find(|s| (s.freq - 2000.0).abs() < 40.0).expect("2 kHz source");
        assert!(low.pos[0] > 0.0 && high.pos[0] < 0.0, "left/right placement");
        assert!(low.pos[1] < high.pos[1], "low frequency sits lower on the body");
    }

    #[test]
    fn impulse_is_detected_as_broadband() {
        let mut x = vec![0.0f32; 8192];
        x[100] = 1.0;
        let a = analyze(&x, &x, 22050, &Params::default()).unwrap();
        assert!(a.params.sources.iter().any(|s| s.wave == Waveform::Impulse));
    }
}
