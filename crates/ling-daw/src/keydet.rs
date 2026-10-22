//! Key detection for audio: a chroma profile (how much of each of the 12 pitch classes a recording holds) from short FFTs, and the
//! tuning offset of the recording against A = 440 Hz. The profile goes to the same Krumhansl-Schmuckler matcher the notes use
//! (`theory::detect_key_hist`), so a project's MIDI and audio can vote together.

pub struct Chroma {
    /// pitch-class weights, summing to 1 (all zero for silence)
    pub hist: [f64; 12],
    /// how far the recording sits from A = 440 Hz tuning, in cents (-50..50)
    pub tuning_cents: f64,
    /// RMS level, to weigh recordings against each other
    pub level: f64,
    /// how many independent pitches sound at once on average (overtones of a lower pitch do not count): ~1 for a solo line, 3+ for chords
    pub poly: f64,
}

/// in-place radix-2 complex FFT (`re`, `im` of a power-of-two length)
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -std::f32::consts::TAU / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        let mut i = 0;
        while i < n {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (i + k, i + k + len / 2);
                let (tr, ti) = (re[b] * cr - im[b] * ci, re[b] * ci + im[b] * cr);
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let nr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = nr;
            }
            i += len;
        }
        len <<= 1;
    }
}

/// Chroma of an interleaved stereo recording: the spectral peaks between 60 Hz and 2.5 kHz vote for their pitch class, bass notes
/// count extra (they name the tonic), and the sub-semitone remainders give the recording's tuning.
pub fn chroma(stereo: &[f32], sr: f32) -> Chroma {
    const N: usize = 4096;
    let mut out = Chroma { hist: [0.0; 12], tuning_cents: 0.0, level: 0.0, poly: 1.0 };
    if sr < 8000.0 || stereo.len() < 2 * N {
        return out;
    }
    let dec = ((sr / 11025.0).round() as usize).max(1);
    let fs = sr as f64 / dec as f64;
    let mono: Vec<f32> = stereo.chunks_exact(2 * dec).map(|c| c.iter().sum::<f32>() / c.len() as f32).collect();
    if mono.len() < N {
        return out;
    }
    out.level = (mono.iter().map(|x| x * x).sum::<f32>() / mono.len() as f32).sqrt() as f64;
    // at most ~300 frames however long the recording is
    let frames = ((mono.len() - N) / (N / 2) + 1).max(1);
    let step = ((frames as f64 / 300.0).ceil() as usize).max(1) * (N / 2);
    let win: Vec<f32> = (0..N).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / N as f32).cos()).collect();
    let (mut re, mut im) = (vec![0.0f32; N], vec![0.0f32; N]);
    let (mut sin_sum, mut cos_sum) = (0.0f64, 0.0f64);
    let k_lo = (60.0 * N as f64 / fs).ceil() as usize;
    let k_hi = ((2500.0 * N as f64 / fs) as usize).min(N / 2 - 2);
    let (mut poly_sum, mut poly_n) = (0.0f64, 0usize);
    let loud = mono.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    let mut pos = 0;
    while pos + N <= mono.len() {
        for i in 0..N {
            re[i] = mono[pos + i] * win[i];
            im[i] = 0.0;
        }
        fft(&mut re, &mut im);
        let mag: Vec<f32> = (0..N / 2).map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt()).collect();
        let peak = mag[k_lo..=k_hi].iter().cloned().fold(0.0f32, f32::max);
        // the strong peaks, minus the ones that are overtones of a lower one
        if peak > 1e-4 && mono[pos..pos + N].iter().fold(0.0f32, |m, x| m.max(x.abs())) > loud * 0.15 {
            let mut pk: Vec<(f64, f32)> = (k_lo.max(1)..=k_hi).filter(|k| mag[*k] > peak * 0.2 && mag[*k] > mag[*k - 1] && mag[*k] >= mag[*k + 1]).map(|k| (k as f64 * fs / N as f64, mag[k])).collect();
            pk.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            let mut roots: Vec<f64> = vec![];
            for (f, _) in pk {
                let harmonic = roots.iter().any(|r| {
                    let q = f / r;
                    q > 1.5 && q < 9.0 && (q - q.round()).abs() < 0.04 * q.round()
                });
                if !harmonic {
                    roots.push(f);
                }
            }
            poly_sum += roots.len().max(1) as f64;
            poly_n += 1;
        }
        if peak > 1e-4 {
            for k in k_lo.max(1)..=k_hi {
                if mag[k] < peak * 0.04 || mag[k] <= mag[k - 1] || mag[k] < mag[k + 1] {
                    continue;
                }
                // parabolic refinement of the peak position
                let (a, b, c) = (mag[k - 1].max(1e-9).ln() as f64, mag[k].max(1e-9).ln() as f64, mag[k + 1].max(1e-9).ln() as f64);
                let d = 0.5 * (a - c) / (a - 2.0 * b + c).min(-1e-9);
                let f = (k as f64 + d.clamp(-0.5, 0.5)) * fs / N as f64;
                let midi = 69.0 + 12.0 * (f / 440.0).log2();
                let near = midi.round();
                let w = (mag[k] as f64 / peak as f64).powf(0.7) * if midi < 52.0 { 1.6 } else { 1.0 };
                out.hist[(near as i64).rem_euclid(12) as usize] += w;
                let frac = (midi - near) * std::f64::consts::TAU;
                sin_sum += w * frac.sin();
                cos_sum += w * frac.cos();
            }
        }
        pos += step;
    }
    if poly_n > 0 {
        out.poly = poly_sum / poly_n as f64;
    }
    let total: f64 = out.hist.iter().sum();
    if total > 0.0 {
        for h in out.hist.iter_mut() {
            *h /= total;
        }
        // the mean of the remainders as angles (one turn = one semitone), back to cents
        out.tuning_cents = (sin_sum.atan2(cos_sum) / std::f64::consts::TAU * 100.0).clamp(-50.0, 50.0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theory::{detect_key_hist, Key};

    const SR: f32 = 44100.0;

    /// chords (each a list of MIDI notes, 0.8 s long) as soft harmonic tones, tuned `cents` off
    fn progression(chords: &[&[f64]], cents: f64) -> Vec<f32> {
        let per = (SR * 0.8) as usize;
        let mut out = vec![0.0f32; per * chords.len() * 2];
        for (ci, ch) in chords.iter().enumerate() {
            for i in 0..per {
                let t = i as f32 / SR;
                let env = (t / 0.03).min(1.0).min((0.8 - t) / 0.05).max(0.0);
                let mut v = 0.0f32;
                for k in ch.iter() {
                    let f = 440.0 * 2f32.powf((*k as f32 - 69.0 + cents as f32 / 100.0) / 12.0);
                    for h in 1..=4 {
                        v += (std::f32::consts::TAU * f * h as f32 * t).sin() / (h * h) as f32;
                    }
                }
                let s = v * 0.08 * env;
                out[(ci * per + i) * 2] = s;
                out[(ci * per + i) * 2 + 1] = s;
            }
        }
        out
    }

    #[test]
    fn the_fft_finds_a_tone() {
        let n = 1024;
        let mut re: Vec<f32> = (0..n).map(|i| (std::f32::consts::TAU * 64.0 * i as f32 / n as f32).sin()).collect();
        let mut im = vec![0.0f32; n];
        fft(&mut re, &mut im);
        let mag: Vec<f32> = (0..n / 2).map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt()).collect();
        let best = (0..n / 2).max_by(|a, b| mag[*a].partial_cmp(&mag[*b]).unwrap()).unwrap();
        assert_eq!(best, 64);
        assert!(mag[64] > 400.0);
    }

    #[test]
    fn a_c_major_progression_is_c_major_and_a_minor_one_is_a_minor() {
        // C F G C   (with a bass note under each chord)
        let c = progression(&[&[48.0, 60.0, 64.0, 67.0], &[41.0, 60.0, 65.0, 69.0], &[43.0, 59.0, 62.0, 67.0], &[48.0, 60.0, 64.0, 67.0]], 0.0);
        let ch = chroma(&c, SR);
        assert!((ch.hist.iter().sum::<f64>() - 1.0).abs() < 1e-9);
        let (k, corr, _) = detect_key_hist(&ch.hist);
        assert_eq!(k, Key::new(0, false), "got {}", k.name());
        assert!(corr > 0.6, "{corr}");
        // Am Dm E Am
        let a = progression(&[&[45.0, 57.0, 60.0, 64.0], &[38.0, 57.0, 62.0, 65.0], &[40.0, 56.0, 59.0, 64.0], &[45.0, 57.0, 60.0, 64.0]], 0.0);
        let (k, _, _) = detect_key_hist(&chroma(&a, SR).hist);
        assert_eq!(k, Key::new(9, true), "got {}", k.name());
    }

    #[test]
    fn the_tuning_offset_of_a_recording_is_measured() {
        let on = chroma(&progression(&[&[48.0, 60.0, 64.0, 67.0], &[43.0, 59.0, 62.0, 67.0]], 0.0), SR).tuning_cents;
        let sharp = chroma(&progression(&[&[48.0, 60.0, 64.0, 67.0], &[43.0, 59.0, 62.0, 67.0]], 30.0), SR).tuning_cents;
        assert!(on.abs() < 8.0, "in tune: {on}");
        assert!((sharp - 30.0).abs() < 10.0, "30 cents sharp: {sharp}");
    }

    #[test]
    fn a_solo_line_has_one_independent_pitch_and_chords_have_several() {
        let solo = progression(&[&[60.0], &[62.0], &[64.0], &[60.0]], 0.0);
        let chords = progression(&[&[48.0, 60.0, 64.0, 67.0], &[41.0, 60.0, 65.0, 69.0], &[43.0, 59.0, 62.0, 67.0]], 0.0);
        let (ps, pc) = (chroma(&solo, SR).poly, chroma(&chords, SR).poly);
        assert!(ps < 1.6, "solo {ps}");
        assert!(pc > 1.8 && pc > ps + 0.7, "chords {pc} vs solo {ps}");
    }

    #[test]
    fn silence_and_short_audio_give_an_empty_profile() {
        assert_eq!(chroma(&vec![0.0; 100000], SR).hist, [0.0; 12]);
        assert_eq!(chroma(&[0.1; 10], SR).hist, [0.0; 12]);
        assert_eq!(chroma(&[], SR).level, 0.0);
    }
}
