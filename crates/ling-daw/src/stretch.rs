//! Time-stretch (WSOLA) and pitch-shift for audio clips.
//!
//! `time_stretch` changes duration without changing pitch; `pitch_shift` changes pitch
//! without changing duration (stretch, then resample back). Both work on interleaved
//! stereo `f32`.

const FRAME: usize = 2048; // analysis / synthesis window (frames)
const HOP: usize = FRAME / 4; // synthesis hop (75 % overlap)
const DECIM: usize = 4; // decimation used for the similarity search
const SEARCH: usize = 384; // +/- frames searched around the ideal position

fn hann(n: usize) -> Vec<f32> {
    (0..n).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()).collect()
}

/// Waveform-similarity overlap-add. `ratio` > 1 lengthens (slower), < 1 shortens.
pub fn time_stretch(input: &[f32], ratio: f64) -> Vec<f32> {
    let n_in = input.len() / 2;
    if (ratio - 1.0).abs() < 1e-4 || n_in < FRAME * 2 {
        return input.to_vec();
    }
    let n_out = (n_in as f64 * ratio) as usize;
    let mut out = vec![0.0f32; n_out * 2 + FRAME * 2];
    let mut norm = vec![0.0f32; n_out + FRAME];
    let win = hann(FRAME);
    // mono, decimated copy for correlation
    let mono: Vec<f32> = (0..n_in / DECIM).map(|i| {
        let f = i * DECIM;
        (input[f * 2] + input[f * 2 + 1]) * 0.5
    }).collect();
    let ha = HOP as f64 / ratio; // analysis hop
    let mut prev: usize = 0; // input position of the previous chosen frame
    let mut k = 0usize;
    loop {
        let opos = k * HOP;
        if opos >= n_out {
            break;
        }
        let ideal = (k as f64 * ha) as usize;
        let mut pos = ideal.min(n_in.saturating_sub(FRAME));
        if k > 0 {
            // reference = what naturally follows the previous frame
            let refpos = (prev + HOP).min(n_in.saturating_sub(FRAME));
            let lo = ideal.saturating_sub(SEARCH);
            let hi = (ideal + SEARCH).min(n_in.saturating_sub(FRAME));
            let (rl, dl) = (FRAME / DECIM, 4usize.max(SEARCH / DECIM / 6));
            let mut best = f32::MIN;
            let mut best_pos = pos;
            let mut cand = lo;
            let rd = refpos / DECIM;
            while cand <= hi {
                let cd = cand / DECIM;
                if cd + rl <= mono.len() && rd + rl <= mono.len() {
                    // correlate every `dl`th decimated sample for speed
                    let mut c = 0.0f32;
                    let mut j = 0;
                    while j < rl {
                        c += mono[rd + j] * mono[cd + j];
                        j += dl.min(2);
                    }
                    if c > best {
                        best = c;
                        best_pos = cand;
                    }
                }
                cand += DECIM;
            }
            pos = best_pos;
        }
        prev = pos;
        for i in 0..FRAME {
            let oi = opos + i;
            if oi >= n_out + FRAME {
                break;
            }
            let ii = pos + i;
            if ii >= n_in {
                break;
            }
            let w = win[i];
            out[oi * 2] += input[ii * 2] * w;
            out[oi * 2 + 1] += input[ii * 2 + 1] * w;
            norm[oi] += w;
        }
        k += 1;
    }
    let mut res = Vec::with_capacity(n_out * 2);
    for i in 0..n_out {
        let g = if norm[i] > 1e-3 { 1.0 / norm[i] } else { 0.0 };
        res.push(out[i * 2] * g);
        res.push(out[i * 2 + 1] * g);
    }
    res
}

/// Linear-interpolating resample: output sample i reads input at i*speed.
pub fn resample_speed(input: &[f32], speed: f64) -> Vec<f32> {
    let n_in = input.len() / 2;
    if n_in < 2 {
        return input.to_vec();
    }
    let n_out = ((n_in as f64 - 1.0) / speed).max(0.0) as usize;
    let mut out = Vec::with_capacity(n_out * 2);
    for i in 0..n_out {
        let p = i as f64 * speed;
        let a = p as usize;
        let f = (p - a as f64) as f32;
        let b = (a + 1).min(n_in - 1);
        for c in 0..2 {
            let x = input[a * 2 + c];
            let y = input[b * 2 + c];
            out.push(x + (y - x) * f);
        }
    }
    out
}

/// Shift pitch by `semitones` keeping the duration.
pub fn pitch_shift(input: &[f32], semitones: f64) -> Vec<f32> {
    if semitones.abs() < 1e-3 {
        return input.to_vec();
    }
    let f = 2f64.powf(semitones / 12.0);
    let stretched = time_stretch(input, f); // f times longer/shorter
    resample_speed(&stretched, f) // read faster/slower => pitch moves, length returns
}

/// Combined: change tempo by `stretch` (duration multiplier) and pitch by `semitones`.
pub fn process(input: &[f32], stretch: f64, semitones: f64) -> Vec<f32> {
    let shifted = pitch_shift(input, semitones);
    time_stretch(&shifted, stretch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, secs: f32, sr: f32) -> Vec<f32> {
        (0..(secs * sr) as usize).flat_map(|i| {
            let s = (i as f32 * freq * std::f32::consts::TAU / sr).sin() * 0.5;
            [s, s]
        }).collect()
    }
    fn freq_of(x: &[f32], sr: f32) -> f32 {
        // zero-crossing rate on the left channel, ignoring the edges
        let n = x.len() / 2;
        let (a, b) = (n / 8, n - n / 8);
        let mut z = 0;
        for i in a + 1..b {
            if (x[(i - 1) * 2] < 0.0) != (x[i * 2] < 0.0) {
                z += 1;
            }
        }
        z as f32 / 2.0 / ((b - a) as f32 / sr)
    }

    #[test]
    fn stretch_keeps_pitch_and_scales_length() {
        let sr = 44100.0;
        let x = sine(440.0, 2.0, sr);
        for ratio in [0.75f64, 1.5] {
            let y = time_stretch(&x, ratio);
            let len_ratio = y.len() as f64 / x.len() as f64;
            assert!((len_ratio - ratio).abs() < 0.01, "length {len_ratio} vs {ratio}");
            let f = freq_of(&y, sr);
            assert!((f - 440.0).abs() < 25.0, "pitch drifted to {f} at ratio {ratio}");
        }
    }

    #[test]
    fn pitch_shift_moves_pitch_keeps_length() {
        let sr = 44100.0;
        let x = sine(440.0, 2.0, sr);
        let y = pitch_shift(&x, 12.0);
        assert!((y.len() as f64 / x.len() as f64 - 1.0).abs() < 0.01);
        let f = freq_of(&y, sr);
        assert!((f - 880.0).abs() < 40.0, "expected ~880 Hz, got {f}");
    }
}
