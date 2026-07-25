//! Offline music analysis: tempo (BPM), beat grid, musical key, and onsets.
//!
//! All functions take a **mono** `f32` buffer + sample rate. They share a small
//! STFT (Hann-windowed magnitude spectrogram) built on `rustfft`.

use crate::note::pitch_class_name;
use rustfft::{num_complex::Complex, FftPlanner};

const FRAME: usize = 1024;
const HOP: usize = 512;

/// RMS loudness of a buffer.
pub fn rms(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
}

/// Duration in seconds for `n` mono samples at `rate`.
pub fn duration(n: usize, rate: u32) -> f32 {
    n as f32 / rate.max(1) as f32
}

/// Magnitude spectrogram: `Vec<frame>` where each frame is `FRAME/2` magnitudes.
fn spectrogram(mono: &[f32]) -> Vec<Vec<f32>> {
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FRAME);
    let bins = FRAME / 2;
    let window: Vec<f32> = (0..FRAME)
        .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (FRAME - 1) as f32).cos()))
        .collect();
    let mut frames = Vec::new();
    if mono.len() < FRAME {
        return frames;
    }
    let mut pos = 0;
    let mut scratch = vec![Complex { re: 0.0, im: 0.0 }; FRAME];
    while pos + FRAME <= mono.len() {
        for i in 0..FRAME {
            scratch[i] = Complex { re: mono[pos + i] * window[i], im: 0.0 };
        }
        fft.process(&mut scratch);
        frames.push(
            scratch[..bins]
                .iter()
                .map(|c| (c.re * c.re + c.im * c.im).sqrt())
                .collect(),
        );
        pos += HOP;
    }
    frames
}

/// Spectral-flux onset envelope (one value per STFT hop): sum of positive
/// magnitude increases across consecutive frames, then normalized.
pub fn onset_envelope(mono: &[f32]) -> Vec<f32> {
    let spec = spectrogram(mono);
    if spec.len() < 2 {
        return Vec::new();
    }
    let mut env = vec![0.0f32; spec.len()];
    for t in 1..spec.len() {
        let mut flux = 0.0f32;
        for (cur, prev) in spec[t].iter().zip(spec[t - 1].iter()) {
            let d = cur - prev;
            if d > 0.0 {
                flux += d;
            }
        }
        env[t] = flux;
    }
    let max = env.iter().cloned().fold(0.0f32, f32::max);
    if max > 0.0 {
        for e in &mut env {
            *e /= max;
        }
    }
    env
}

/// Onset times (seconds) — adaptive-threshold peak-picking on the flux envelope.
pub fn onsets(mono: &[f32], rate: u32) -> Vec<f32> {
    let env = onset_envelope(mono);
    if env.is_empty() {
        return Vec::new();
    }
    let sec_per = HOP as f32 / rate as f32;
    let w = 8; // local window for the moving average
    let mut out = Vec::new();
    for t in 1..env.len().saturating_sub(1) {
        let lo = t.saturating_sub(w);
        let hi = (t + w).min(env.len() - 1);
        let mean: f32 = env[lo..=hi].iter().sum::<f32>() / (hi - lo + 1) as f32;
        if env[t] > env[t - 1] && env[t] >= env[t + 1] && env[t] > mean + 0.08 {
            out.push(t as f32 * sec_per);
        }
    }
    out
}

/// Estimate tempo in BPM from the onset envelope's autocorrelation.
pub fn bpm(mono: &[f32], rate: u32) -> f32 {
    let env = onset_envelope(mono);
    if env.len() < 16 {
        return 0.0;
    }
    let env_rate = rate as f32 / HOP as f32; // envelope samples per second

    // Search BPM 60..200 → lag in envelope frames.
    let min_bpm = 60.0f32;
    let max_bpm = 200.0f32;
    let min_lag = (env_rate * 60.0 / max_bpm).floor().max(1.0) as usize;
    let max_lag = (env_rate * 60.0 / min_bpm)
        .ceil()
        .min((env.len() / 2) as f32) as usize;
    if max_lag <= min_lag {
        return 0.0;
    }

    let mean: f32 = env.iter().sum::<f32>() / env.len() as f32;
    let centered: Vec<f32> = env.iter().map(|e| e - mean).collect();

    let mut best_lag = min_lag;
    let mut best = f32::MIN;
    for lag in min_lag..=max_lag {
        let mut ac = 0.0f32;
        for i in 0..(centered.len() - lag) {
            ac += centered[i] * centered[i + lag];
        }
        // mild preference for mid-tempo (avoids picking very fast subdivisions)
        let weight = 1.0
            - 0.15
                * ((lag as f32 - (min_lag + max_lag) as f32 * 0.5).abs()
                    / (max_lag - min_lag) as f32);
        let score = ac * weight;
        if score > best {
            best = score;
            best_lag = lag;
        }
    }
    let mut bpm = 60.0 * env_rate / best_lag as f32;
    // Octave-correct into a musical 70..170 range.
    while bpm < 70.0 {
        bpm *= 2.0;
    }
    while bpm > 170.0 {
        bpm /= 2.0;
    }
    (bpm * 10.0).round() / 10.0
}

/// Beat timestamps (seconds): phase-align a BPM grid to the strongest onsets.
pub fn beat_grid(mono: &[f32], rate: u32, bpm_val: f32) -> Vec<f32> {
    if bpm_val <= 0.0 {
        return Vec::new();
    }
    let period = 60.0 / bpm_val;
    let total = duration(mono.len(), rate);
    let ons = onsets(mono, rate);
    // Best phase offset = onset that maximizes alignment (simple: first onset).
    let phase = ons.first().copied().unwrap_or(0.0) % period;
    let mut t = phase;
    let mut beats = Vec::new();
    while t < total {
        beats.push((t * 1000.0).round() / 1000.0);
        t += period;
    }
    beats
}

// ── Key detection (chroma + Krumhansl–Schmuckler) ────────────────────────────

// Krumhansl–Kessler key profiles (major / minor), starting on the tonic.
const MAJOR_PROFILE: [f32; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
const MINOR_PROFILE: [f32; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];

/// Average 12-bin chroma vector across the track.
pub fn chroma(mono: &[f32], rate: u32) -> [f32; 12] {
    let spec = spectrogram(mono);
    let mut c = [0.0f32; 12];
    let bin_hz = rate as f32 / FRAME as f32;
    for frame in &spec {
        for (b, &mag) in frame.iter().enumerate() {
            if b == 0 {
                continue;
            }
            let hz = b as f32 * bin_hz;
            if !(55.0..=2000.0).contains(&hz) {
                continue;
            }
            let midi = 69.0 + 12.0 * (hz / 440.0).log2();
            let pc = (midi.round() as i32).rem_euclid(12) as usize;
            c[pc] += mag * mag; // power weighting sharpens peaks vs. spectral leakage
        }
    }
    let sum: f32 = c.iter().sum();
    if sum > 0.0 {
        for v in &mut c {
            *v /= sum;
        }
    }
    c
}

/// Detect musical key → (pitch-class root 0..11, is_major).
pub fn key(mono: &[f32], rate: u32) -> (usize, bool) {
    let c = chroma(mono, rate);
    let corr = |profile: &[f32; 12], rot: usize| -> f32 {
        let mut s = 0.0;
        for i in 0..12 {
            s += c[i] * profile[(i + 12 - rot) % 12];
        }
        s
    };
    let mut best = (0usize, true, f32::MIN);
    for root in 0..12 {
        let maj = corr(&MAJOR_PROFILE, root);
        let min = corr(&MINOR_PROFILE, root);
        if maj > best.2 {
            best = (root, true, maj);
        }
        if min > best.2 {
            best = (root, false, min);
        }
    }
    (best.0, best.1)
}

/// Human-readable key string, e.g. `"A minor"`.
pub fn key_name(mono: &[f32], rate: u32) -> String {
    let (root, major) = key(mono, rate);
    format!(
        "{} {}",
        pitch_class_name(root, false),
        if major { "major" } else { "minor" }
    )
}

/// FFT magnitude bands at a specific playback position.
///
/// Computes a single Hann-windowed 1024-point FFT frame from `mono` starting
/// at `pos_secs`, then bins the magnitudes into `nbands` log-spaced frequency
/// bands (20 Hz – Nyquist). Values are normalised to [0..1] relative to the
/// strongest band in this frame.
pub fn fft_bands_at_pos(mono: &[f32], rate: u32, pos_secs: f32, nbands: usize) -> Vec<f32> {
    if mono.is_empty() || nbands == 0 || rate == 0 {
        return vec![0.0; nbands];
    }

    let sample_start = (pos_secs * rate as f32) as usize;
    let sample_start = sample_start.min(mono.len().saturating_sub(FRAME));
    let slice = &mono[sample_start..(sample_start + FRAME).min(mono.len())];

    let n = FRAME;
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n);

    let window: Vec<f32> = (0..n)
        .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (n - 1) as f32).cos()))
        .collect();

    let mut buf: Vec<Complex<f32>> = (0..n)
        .map(|i| Complex {
            re: slice.get(i).copied().unwrap_or(0.0) * window[i],
            im: 0.0,
        })
        .collect();

    let mut scratch = vec![Complex { re: 0.0, im: 0.0 }; fft.get_inplace_scratch_len()];
    fft.process_with_scratch(&mut buf, &mut scratch);

    let bins = n / 2;
    let nyquist = rate as f32 * 0.5;
    let min_hz: f32 = 20.0;
    let max_hz = nyquist.min(20000.0);

    let mut bands = vec![0.0f32; nbands];
    for (b, band) in bands.iter_mut().enumerate().take(nbands) {
        let t0 = b as f32 / nbands as f32;
        let t1 = (b + 1) as f32 / nbands as f32;
        let hz0 = min_hz * (max_hz / min_hz).powf(t0);
        let hz1 = min_hz * (max_hz / min_hz).powf(t1);
        let k0 = ((hz0 * n as f32 / rate as f32).round() as usize)
            .max(1)
            .min(bins - 1);
        let k1 = ((hz1 * n as f32 / rate as f32).round() as usize)
            .max(k0 + 1)
            .min(bins);
        let count = (k1 - k0) as f32;
        let energy: f32 = buf[k0..k1]
            .iter()
            .map(|c| (c.re * c.re + c.im * c.im).sqrt())
            .sum::<f32>()
            / count;
        *band = energy;
    }

    let peak = bands.iter().cloned().fold(0.0f32, f32::max);
    if peak > 0.0 {
        for v in &mut bands {
            *v /= peak;
        }
    }
    bands
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    /// Synthesize a kick-drum click train at `bpm`.
    fn click_train(bpm: f32, rate: u32, secs: f32) -> Vec<f32> {
        let n = (rate as f32 * secs) as usize;
        let mut x = vec![0.0f32; n];
        let period = (rate as f32 * 60.0 / bpm) as usize;
        let mut i = 0;
        while i < n {
            // short decaying 120 Hz thump
            for k in 0..(rate as usize / 12) {
                if i + k >= n {
                    break;
                }
                let env = (-(k as f32) / (rate as f32 * 0.03)).exp();
                x[i + k] += (2.0 * PI * 120.0 * k as f32 / rate as f32).sin() * env;
            }
            i += period;
        }
        x
    }

    #[test]
    fn bpm_on_click_train() {
        let rate = 22050;
        let x = click_train(120.0, rate, 8.0);
        let est = bpm(&x, rate);
        assert!((est - 120.0).abs() < 4.0, "expected ~120, got {est}");
    }

    #[test]
    fn key_of_c_major_chord() {
        // C major triad (C-E-G) sustained → should detect C major.
        let rate = 22050;
        let freqs = [261.63, 329.63, 392.0];
        let n = rate as usize * 3;
        let x: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / rate as f32;
                freqs.iter().map(|f| (2.0 * PI * f * t).sin()).sum::<f32>() / 3.0
            })
            .collect();
        let (root, major) = key(&x, rate);
        assert_eq!(root, 0, "root should be C");
        assert!(major, "should be major");
    }
}
