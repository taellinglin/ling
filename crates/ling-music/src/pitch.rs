//! Monophonic pitch detection (for karaoke sung-pitch scoring).
//!
//! Uses a normalized autocorrelation (NSDF-style, à la McLeod) with parabolic
//! interpolation of the chosen lag. Robust enough for a single voice / lead.

/// Detect the fundamental of `samples` (mono) at `rate` Hz. Searches `fmin..fmax`.
/// Returns `None` when the signal is too quiet or no clear period is found.
pub fn detect_range(samples: &[f32], rate: u32, fmin: f32, fmax: f32) -> Option<f32> {
    let n = samples.len();
    if n < 256 {
        return None;
    }

    // Reject near-silence.
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / n as f32).sqrt();
    if rms < 1e-3 {
        return None;
    }

    let rate = rate as f32;
    let min_lag = (rate / fmax).floor().max(2.0) as usize;
    let max_lag = (rate / fmin).ceil().min((n / 2) as f32) as usize;
    if max_lag <= min_lag {
        return None;
    }

    // Normalized square-difference / autocorrelation (McLeod NSDF).
    let mut nsdf = vec![0.0f32; max_lag + 1];
    for lag in min_lag..=max_lag {
        let mut ac = 0.0f32; // autocorrelation
        let mut m = 0.0f32; // sum of squares (both windows)
        for i in 0..(n - lag) {
            let a = samples[i];
            let b = samples[i + lag];
            ac += a * b;
            m += a * a + b * b;
        }
        nsdf[lag] = if m > 0.0 { 2.0 * ac / m } else { 0.0 };
    }

    // Pick the first strong peak above a threshold of the global max.
    let global_max = nsdf[min_lag..=max_lag]
        .iter()
        .cloned()
        .fold(0.0f32, f32::max);
    if global_max < 0.3 {
        return None;
    }
    let thresh = global_max * 0.85;

    let mut best_lag = 0usize;
    let mut lag = min_lag + 1;
    while lag < max_lag {
        if nsdf[lag] > thresh && nsdf[lag] >= nsdf[lag - 1] && nsdf[lag] >= nsdf[lag + 1] {
            best_lag = lag;
            break;
        }
        lag += 1;
    }
    if best_lag == 0 {
        return None;
    }

    // Parabolic interpolation around the peak for sub-sample accuracy.
    let y0 = nsdf[best_lag - 1];
    let y1 = nsdf[best_lag];
    let y2 = nsdf[best_lag + 1];
    let denom = y0 - 2.0 * y1 + y2;
    let shift = if denom.abs() > 1e-9 {
        0.5 * (y0 - y2) / denom
    } else {
        0.0
    };
    let refined = best_lag as f32 + shift;
    if refined <= 0.0 {
        return None;
    }
    Some(rate / refined)
}

/// Convenience: detect over the typical sung/instrument range (~55–1000 Hz).
pub fn detect(samples: &[f32], rate: u32) -> Option<f32> {
    detect_range(samples, rate, 55.0, 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects_a440_sine() {
        let rate = 44100u32;
        let f = 440.0f32;
        let buf: Vec<f32> = (0..4096)
            .map(|i| (2.0 * std::f32::consts::PI * f * i as f32 / rate as f32).sin())
            .collect();
        let hz = detect(&buf, rate).expect("should detect pitch");
        assert!((hz - 440.0).abs() < 4.0, "got {hz}");
    }
    #[test]
    fn detects_low_note() {
        let rate = 44100u32;
        let f = 110.0f32; // A2
        let buf: Vec<f32> = (0..8192)
            .map(|i| (2.0 * std::f32::consts::PI * f * i as f32 / rate as f32).sin() * 0.5)
            .collect();
        let hz = detect(&buf, rate).unwrap();
        assert!((hz - 110.0).abs() < 3.0, "got {hz}");
    }
    #[test]
    fn silence_is_none() {
        assert!(detect(&vec![0.0; 4096], 44100).is_none());
    }
}
