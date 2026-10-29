//! User wavetables for Ling Growl and Ling Prism, and "resample to wavetable": any audio (a bounced clip, a sample file, a
//! rendered note of a track) cut into single-cycle frames.
//!
//! A table is a list of single cycles of `N` = 2048 samples (the Serum convention, so a Serum / Vital `.wav` table loads too). It is
//! stored as a mono 32-bit float WAV, frame after frame. On load every frame is band-limited into `LEVELS` (the richest has 256
//! harmonics, the poorest 1) with an FFT, so a high note never aliases; the instruments choose the level from the note's frequency
//! the same way their built-in tables do.
//!
//! Resampling (`analyse`): the pitch is found (YIN, the expected key as a hint when there is one), `frames` positions are spread over
//! the audible part of the sound, and at each one exactly one period (the local period, re-measured there) is read with cubic
//! interpolation into 2048 points. The seam is closed (the difference between the cycle's start and one period later is taken out
//! as a ramp), the DC removed, every frame is shifted to line up with the one before it (circular cross-correlation, so the frames
//! morph without phase jumps) and normalised to a peak of 1.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// samples per frame
pub const N: usize = 2048;
/// harmonics per band-limited level, richest first
pub const LEVELS: [usize; 9] = [256, 128, 64, 32, 16, 8, 4, 2, 1];
/// more frames than this are thinned out evenly on load
pub const MAX_FRAMES: usize = 64;

pub struct WaveTable {
    /// the file it was loaded from ("" = made in memory)
    pub path: String,
    /// the single cycles as stored (N samples each, normalised)
    pub frames: Vec<Vec<f32>>,
    /// [frame * LEVELS.len() + level] -> one band-limited cycle of N samples
    mips: Vec<Vec<f32>>,
}

impl WaveTable {
    /// A table from single cycles of any length (each is resampled to N); the DC goes and the whole table is normalised to a peak of 1.
    pub fn from_frames(frames: &[Vec<f32>], path: &str) -> Result<WaveTable, String> {
        let frames: Vec<&Vec<f32>> = frames.iter().filter(|f| f.len() >= 4).collect();
        if frames.is_empty() {
            return Err("no frames".into());
        }
        // thin out very long tables evenly
        let pick: Vec<&Vec<f32>> = if frames.len() > MAX_FRAMES { (0..MAX_FRAMES).map(|k| frames[k * (frames.len() - 1) / (MAX_FRAMES - 1)]).collect() } else { frames };
        let mut cycles: Vec<Vec<f32>> = pick.iter().map(|f| resample_cycle(f, N)).collect();
        let mut peak = 1e-9f32;
        for c in cycles.iter_mut() {
            let mean = c.iter().sum::<f32>() / N as f32;
            c.iter_mut().for_each(|x| *x -= mean);
            peak = c.iter().fold(peak, |m, x| m.max(x.abs()));
        }
        if peak < 1e-6 {
            return Err("the table is silent".into());
        }
        for c in cycles.iter_mut() {
            c.iter_mut().for_each(|x| *x /= peak);
        }
        let mut mips = Vec::with_capacity(cycles.len() * LEVELS.len());
        for c in &cycles {
            let (mut re, mut im) = (c.clone(), vec![0.0f32; N]);
            fft(&mut re, &mut im, false);
            for &h in LEVELS.iter() {
                // keep harmonics 1..=h (and their mirror images), then back to the time domain
                let (mut r2, mut i2) = (vec![0.0f32; N], vec![0.0f32; N]);
                for k in 1..=h.min(N / 2 - 1) {
                    r2[k] = re[k];
                    i2[k] = im[k];
                    r2[N - k] = re[N - k];
                    i2[N - k] = im[N - k];
                }
                fft(&mut r2, &mut i2, true);
                mips.push(r2);
            }
        }
        Ok(WaveTable { path: path.to_string(), frames: cycles, mips })
    }

    pub fn nframes(&self) -> usize {
        self.frames.len()
    }

    /// the richest level whose top harmonic stays under 0.45 sr for a partial at `f` Hz
    #[inline]
    pub fn level(f: f32, sr: f32) -> usize {
        let allowed = (0.45 * sr / f.max(10.0)).floor() as usize;
        LEVELS.iter().position(|&h| h <= allowed).unwrap_or(LEVELS.len() - 1)
    }

    #[inline]
    pub fn cycle(&self, frame: usize, level: usize) -> &[f32] {
        &self.mips[frame.min(self.frames.len() - 1) * LEVELS.len() + level.min(LEVELS.len() - 1)]
    }

    /// the two frames around position `pos` (0..1 over the table) and the crossfade between them
    #[inline]
    pub fn frames_at(&self, pos: f32) -> (usize, usize, f32) {
        let n = self.frames.len();
        if n <= 1 {
            return (0, 0, 0.0);
        }
        let p = pos.clamp(0.0, 1.0) * (n - 1) as f32;
        let i = (p as usize).min(n - 2);
        (i, i + 1, p - i as f32)
    }

    /// one sample: phase `ph` (cycles), position `pos`, band-limited level `level`
    #[inline]
    pub fn read(&self, pos: f32, level: usize, ph: f32) -> f32 {
        let (a, b, f) = self.frames_at(pos);
        let x = rd(self.cycle(a, level), ph);
        if f <= 0.0 {
            x
        } else {
            x + (rd(self.cycle(b, level), ph) - x) * f
        }
    }

    /// `n` points of the cycle at `pos` (the panel's picture)
    pub fn plot(&self, pos: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| self.read(pos, 0, i as f32 / n as f32)).collect()
    }

    /// Read a wavetable file: a WAV of frames of 2048 (or 256 / 512 / 1024 / 4096) samples. Loaded tables are shared by path.
    pub fn load(path: &str) -> Result<Arc<WaveTable>, String> {
        static CACHE: OnceLock<Mutex<HashMap<String, (std::time::SystemTime, Arc<WaveTable>)>>> = OnceLock::new();
        let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let stamp = std::fs::metadata(path).and_then(|m| m.modified()).map_err(|e| format!("{path}: {e}"))?;
        if let Some((t, w)) = cache.lock().unwrap_or_else(|p| p.into_inner()).get(path) {
            if *t == stamp {
                return Ok(w.clone());
            }
        }
        let x = read_mono_wav(path)?;
        let fs = [2048usize, 1024, 512, 256, 4096].into_iter().find(|&f| x.len() >= f && x.len() % f == 0 && x.len() / f <= 1024).ok_or_else(|| {
            format!("{}: not a wavetable (its length is not a whole number of 2048-sample frames) - use Resample to make one from audio", path)
        })?;
        let frames: Vec<Vec<f32>> = x.chunks_exact(fs).map(|c| c.to_vec()).collect();
        let w = Arc::new(WaveTable::from_frames(&frames, path)?);
        cache.lock().unwrap_or_else(|p| p.into_inner()).insert(path.to_string(), (stamp, w.clone()));
        Ok(w)
    }
}

/// linear read of one cycle (N samples) at phase `ph` (wraps)
#[inline]
pub fn rd(tab: &[f32], ph: f32) -> f32 {
    let x = ph * N as f32;
    let fl = x.floor();
    let i = fl as isize as usize & (N - 1);
    let f = x - fl;
    let (a, b) = (tab[i], tab[(i + 1) & (N - 1)]);
    a + (b - a) * f
}

/// resample one cycle (periodic) to `n` points with cubic (Catmull-Rom) interpolation
fn resample_cycle(c: &[f32], n: usize) -> Vec<f32> {
    if c.len() == n {
        return c.to_vec();
    }
    let m = c.len();
    let at = |i: isize| c[i.rem_euclid(m as isize) as usize];
    (0..n)
        .map(|j| {
            let p = j as f32 * m as f32 / n as f32;
            let i = p.floor() as isize;
            hermite(at(i - 1), at(i), at(i + 1), at(i + 2), p - i as f32)
        })
        .collect()
}

#[inline]
fn hermite(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * t + c2) * t + c1) * t + y1
}

/// cubic read of a signal at fractional index `p` (clamped at the ends)
#[inline]
fn at_frac(x: &[f32], p: f64) -> f32 {
    let i = p.floor() as isize;
    let g = |k: isize| x[k.clamp(0, x.len() as isize - 1) as usize];
    hermite(g(i - 1), g(i), g(i + 1), g(i + 2), (p - i as f64) as f32)
}

/// in-place iterative radix-2 complex FFT (`re.len()` a power of two); the inverse is scaled by 1/n
pub fn fft(re: &mut [f32], im: &mut [f32], inverse: bool) {
    let n = re.len();
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = std::f64::consts::TAU / len as f64 * if inverse { 1.0 } else { -1.0 };
        for s in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (wr, wi) = ((ang * k as f64).cos() as f32, (ang * k as f64).sin() as f32);
                let (a, b) = (s + k, s + k + len / 2);
                let (xr, xi) = (re[b] * wr - im[b] * wi, re[b] * wi + im[b] * wr);
                re[b] = re[a] - xr;
                im[b] = im[a] - xi;
                re[a] += xr;
                im[a] += xi;
            }
        }
        len <<= 1;
    }
    if inverse {
        let s = 1.0 / n as f32;
        re.iter_mut().for_each(|x| *x *= s);
        im.iter_mut().for_each(|x| *x *= s);
    }
}

/// a mono WAV (any channel count: the channels are averaged)
pub fn read_mono_wav(path: &str) -> Result<Vec<f32>, String> {
    let r = hound::WavReader::open(path).map_err(|e| format!("{path}: {e}"))?;
    let spec = r.spec();
    let ch = spec.channels.max(1) as usize;
    let data: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.into_samples::<f32>().filter_map(|s| s.ok()).collect(),
        hound::SampleFormat::Int => {
            let s = 1.0 / (1u64 << (spec.bits_per_sample.max(1) - 1)) as f32;
            r.into_samples::<i32>().filter_map(|x| x.ok()).map(|x| x as f32 * s).collect()
        }
    };
    Ok(data.chunks(ch).map(|c| c.iter().sum::<f32>() / c.len() as f32).collect())
}

/// Write a table as a mono 32-bit float WAV, frame after frame (each resampled to N).
pub fn save(frames: &[Vec<f32>], path: &str) -> Result<(), String> {
    if let Some(d) = std::path::Path::new(path).parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let spec = hound::WavSpec { channels: 1, sample_rate: 48000, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| format!("{path}: {e}"))?;
    for f in frames {
        for x in resample_cycle(f, N) {
            w.write_sample(x).map_err(|e| e.to_string())?;
        }
    }
    w.finalize().map_err(|e| e.to_string())
}

/// YIN's cumulative mean normalised difference for lags `lo..=hi` over the window `x[at..at + w]`; returns (lag, value) of the
/// best dip (the first one under `thresh`, else the lowest), refined by a parabola
fn yin(x: &[f32], at: usize, w: usize, lo: usize, hi: usize, thresh: f32) -> Option<(f64, f32)> {
    if lo < 2 || hi <= lo || at + w + hi + 1 >= x.len() {
        return None;
    }
    let mut d = vec![0.0f32; hi + 2];
    for tau in 1..=hi + 1 {
        let mut s = 0.0f32;
        for i in at..at + w {
            let e = x[i] - x[i + tau];
            s += e * e;
        }
        d[tau] = s;
    }
    let mut cm = vec![1.0f32; hi + 2];
    let mut run = 0.0f32;
    for tau in 1..=hi + 1 {
        run += d[tau];
        cm[tau] = if run > 0.0 { d[tau] * tau as f32 / run } else { 1.0 };
    }
    let mut best = None;
    let mut tau = lo;
    while tau <= hi {
        if cm[tau] < thresh {
            // walk down to the bottom of this dip
            while tau < hi && cm[tau + 1] < cm[tau] {
                tau += 1;
            }
            best = Some(tau);
            break;
        }
        tau += 1;
    }
    let tau = best.unwrap_or_else(|| (lo..=hi).min_by(|&a, &b| cm[a].total_cmp(&cm[b])).unwrap_or(lo));
    let (a, b, c) = (cm[tau - 1], cm[tau], cm[tau + 1]);
    let den = a - 2.0 * b + c;
    let off = if den.abs() > 1e-9 { (0.5 * (a - c) / den).clamp(-0.5, 0.5) } else { 0.0 };
    Some((tau as f64 + off as f64, b))
}

/// The pitch of `x` (mono) in Hz around its loudest part; `hint` (Hz, e.g. the rendered key) narrows the search to +-4 semitones.
pub fn detect_f0(x: &[f32], sr: f32, hint: Option<f32>) -> Option<f32> {
    let (s, e) = audible(x);
    if e <= s + 256 {
        return None;
    }
    let (lo, hi) = match hint {
        Some(h) if h > 10.0 => (((sr / h) * 0.79) as usize, ((sr / h) * 1.26).ceil() as usize),
        _ => ((sr / 2000.0) as usize, (sr / 25.0) as usize),
    };
    let lo = lo.max(2);
    let w = (hi * 2).max(1024).min(e - s);
    // a few windows over the first half of the sound; the median of the clear ones
    let mut found: Vec<f64> = vec![];
    for k in 0..5 {
        let at = s + (e - s) * k / 10;
        if at + w + hi + 2 >= x.len() {
            break;
        }
        // with a hint the search is narrow, so a weaker dip still counts (a sub an octave down keeps the dip at the key shallow)
        let accept = if hint.is_some() { 0.7 } else { 0.35 };
        if let Some((lag, v)) = yin(x, at, w, lo, hi, 0.15) {
            if v < accept {
                found.push(lag);
            }
        }
    }
    if found.is_empty() {
        return None;
    }
    found.sort_by(|a, b| a.total_cmp(b));
    Some(sr / found[found.len() / 2] as f32)
}

/// the part of the sound above -40 dB of its peak (an 8 ms envelope)
fn audible(x: &[f32]) -> (usize, usize) {
    let pk = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    if pk <= 0.0 {
        return (0, 0);
    }
    let th = pk * 0.01;
    let s = x.iter().position(|v| v.abs() > th).unwrap_or(0);
    let e = x.iter().rposition(|v| v.abs() > th).map(|i| i + 1).unwrap_or(x.len());
    (s, e)
}

/// Cut audio into `nframes` single-cycle frames (see the module notes). Returns the frames (N samples each) and the pitch found.
pub fn analyse(x: &[f32], sr: f32, nframes: usize, hint: Option<f32>) -> Result<(Vec<Vec<f32>>, f32), String> {
    let nframes = nframes.clamp(1, MAX_FRAMES);
    let f0 = detect_f0(x, sr, hint).or(hint).ok_or("no clear pitch in this audio (a pitched, sustained sound works best)")?;
    let period = (sr / f0) as f64;
    let (s, e) = audible(x);
    let s = s as f64 + period * 0.5;
    let e = e as f64 - period * 2.5;
    if e <= s {
        return Err("the sound is shorter than three cycles".into());
    }
    let mut frames: Vec<Vec<f32>> = vec![];
    let mut prev_spec: Option<(Vec<f32>, Vec<f32>)> = None;
    for k in 0..nframes {
        let pos = if nframes == 1 { s + (e - s) * 0.3 } else { s + (e - s) * k as f64 / (nframes - 1) as f64 };
        // the local period (the pitch may glide or wobble), searched near the global one
        let at = pos as usize;
        let lo = (period * 0.9).floor().max(2.0) as usize;
        let hi = (period * 1.1).ceil() as usize + 1;
        let w = ((period * 2.0) as usize).max(64);
        let p = match yin(x, at, w.min(x.len().saturating_sub(at + hi + 2)), lo, hi, 0.2) {
            Some((lag, v)) if v < 0.5 && at + w + hi + 2 < x.len() => lag,
            _ => period,
        };
        // one period into N points; the seam closed by a ramp, the DC out
        let mut c: Vec<f32> = (0..N).map(|j| at_frac(x, pos + p * j as f64 / N as f64)).collect();
        let d = at_frac(x, pos + p) - c[0];
        for (j, v) in c.iter_mut().enumerate() {
            *v -= d * j as f32 / N as f32;
        }
        let mean = c.iter().sum::<f32>() / N as f32;
        c.iter_mut().for_each(|v| *v -= mean);
        let pk = c.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        if pk < 1e-6 {
            if let Some(last) = frames.last().cloned() {
                frames.push(last);
            }
            continue;
        }
        c.iter_mut().for_each(|v| *v /= pk);
        // line the frame up with the one before: the circular shift with the highest cross-correlation (FFT); the first frame
        // starts where its fundamental rises through zero
        let (mut re, mut im) = (c.clone(), vec![0.0f32; N]);
        fft(&mut re, &mut im, false);
        let shift = match &prev_spec {
            Some((pr, pi)) => {
                let (mut cr, mut ci) = (vec![0.0f32; N], vec![0.0f32; N]);
                for h in 0..N {
                    // X * conj(P)
                    cr[h] = re[h] * pr[h] + im[h] * pi[h];
                    ci[h] = im[h] * pr[h] - re[h] * pi[h];
                }
                fft(&mut cr, &mut ci, true);
                let best = (0..N).max_by(|&a, &b| cr[a].total_cmp(&cr[b])).unwrap_or(0);
                best as f64
            }
            None => {
                // phase of the fundamental: sin start = phase -pi/2 in the cosine convention
                let ph = (im[1] as f64).atan2(re[1] as f64);
                let tau = (ph + std::f64::consts::FRAC_PI_2) / std::f64::consts::TAU;
                (tau.rem_euclid(1.0) * N as f64).round()
            }
        };
        let sh = shift as usize % N;
        if sh != 0 {
            c.rotate_left(sh);
            re = c.clone();
            im = vec![0.0f32; N];
            fft(&mut re, &mut im, false);
        }
        prev_spec = Some((re, im));
        frames.push(c);
    }
    if frames.is_empty() {
        return Err("nothing to cut".into());
    }
    Ok((frames, f0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corr(a: &[f32], b: &[f32]) -> f32 {
        let (ma, mb) = (a.iter().sum::<f32>() / a.len() as f32, b.iter().sum::<f32>() / b.len() as f32);
        let (mut s, mut sa, mut sb) = (0.0f32, 0.0f32, 0.0f32);
        for (x, y) in a.iter().zip(b) {
            s += (x - ma) * (y - mb);
            sa += (x - ma) * (x - ma);
            sb += (y - mb) * (y - mb);
        }
        s / (sa * sb).sqrt().max(1e-12)
    }

    /// a band-limited cycle with a given harmonic recipe, at phase `ph`
    fn wave(h: &[(usize, f32, f32)], ph: f64) -> f32 {
        h.iter().map(|&(n, a, p)| a * ((std::f64::consts::TAU * n as f64 * ph) as f32 + p).sin()).sum()
    }

    #[test]
    fn fft_round_trips() {
        let mut re: Vec<f32> = (0..64).map(|i| ((i * 7) % 13) as f32 - 6.0).collect();
        let orig = re.clone();
        let mut im = vec![0.0; 64];
        fft(&mut re, &mut im, false);
        fft(&mut re, &mut im, true);
        for (a, b) in re.iter().zip(&orig) {
            assert!((a - b).abs() < 1e-4);
        }
    }

    #[test]
    fn resampling_a_known_wave_gives_its_cycle_back() {
        let sr = 48000.0;
        let f = 110.3f32;
        // a lopsided wave (odd phases) so a shifted or flipped copy would not match
        let recipe = [(1usize, 1.0f32, 0.3f32), (2, 0.5, 1.1), (3, 0.33, -0.4), (5, 0.2, 2.0), (8, 0.1, 0.7)];
        let x: Vec<f32> = (0..(sr * 1.0) as usize).map(|i| wave(&recipe, i as f64 * f as f64 / sr as f64) * 0.6).collect();
        let (frames, f0) = analyse(&x, sr, 16, None).unwrap();
        assert!((f0 / f - 1.0).abs() < 0.003, "pitch {f0}");
        assert_eq!(frames.len(), 16);
        // every frame is one cycle of the wave: the best circular alignment correlates almost perfectly
        let reference: Vec<f32> = (0..N).map(|j| wave(&recipe, j as f64 / N as f64)).collect();
        for (k, fr) in frames.iter().enumerate() {
            let best = (0..N).step_by(4).map(|s| {
                let mut r = reference.clone();
                r.rotate_left(s);
                corr(fr, &r)
            }).fold(f32::MIN, f32::max);
            assert!(best > 0.995, "frame {k}: {best}");
            // the seam is closed: the jump from the last sample back to the first is like any other step
            let jump = (fr[N - 1] - fr[0]).abs();
            let step = fr.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
            assert!(jump <= step * 1.5 + 1e-3, "frame {k} seam {jump} vs {step}");
            assert!(fr.iter().sum::<f32>().abs() / (N as f32) < 1e-3, "DC");
            assert!((fr.iter().fold(0.0f32, |m, v| m.max(v.abs())) - 1.0).abs() < 1e-4, "normalised");
        }
        // frames line up with each other (no phase jumps between neighbours)
        for w in frames.windows(2) {
            assert!(corr(&w[0], &w[1]) > 0.99);
        }
        // and as a table, the richest level of frame 0 is that cycle (it has only 8 harmonics)
        let t = WaveTable::from_frames(&frames, "").unwrap();
        assert!(corr(t.cycle(0, 0), &frames[0]) > 0.999);
        // the one-harmonic level is a sine
        let s: Vec<f32> = (0..N).map(|j| (std::f32::consts::TAU * j as f32 / N as f32).sin()).collect();
        let best = (0..N).step_by(8).map(|k| {
            let mut r = s.clone();
            r.rotate_left(k);
            corr(t.cycle(0, LEVELS.len() - 1), &r)
        }).fold(f32::MIN, f32::max);
        assert!(best > 0.999, "{best}");
    }

    #[test]
    fn a_morphing_sound_gives_a_morphing_table_and_round_trips_through_a_file() {
        let sr = 44100.0;
        let f = 82.4f64;
        let n = (sr * 1.5) as usize;
        // a sine that grows into a bright saw-ish wave, with the pitch drifting by a few cents
        let mut ph = 0.0f64;
        let x: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f64 / n as f64;
                ph += f * (1.0 + 0.004 * (t * 6.0).sin()) / sr as f64;
                let b = t as f32;
                (1..=12).map(|h| if h == 1 { 1.0 } else { b / h as f32 } * ((std::f64::consts::TAU * h as f64 * ph) as f32).sin()).sum::<f32>() * 0.5
            })
            .collect();
        let (frames, _) = analyse(&x, sr, 8, Some(82.4)).unwrap();
        // brightness: the energy above the fundamental against the fundamental's
        let bright = |c: &[f32]| {
            let (mut re, mut im) = (c.to_vec(), vec![0.0f32; N]);
            fft(&mut re, &mut im, false);
            let e = |h: usize| re[h] * re[h] + im[h] * im[h];
            (2..20).map(e).sum::<f32>() / e(1).max(1e-9)
        };
        assert!(bright(&frames[7]) > bright(&frames[0]) * 2.0, "{} vs {}", bright(&frames[7]), bright(&frames[0]));
        let dir = std::env::temp_dir().join(format!("lsx_wt_test_{}", std::process::id()));
        let p = dir.join("t.wav").to_string_lossy().into_owned();
        save(&frames, &p).unwrap();
        let t = WaveTable::load(&p).unwrap();
        assert_eq!(t.nframes(), 8);
        assert!(corr(&t.frames[3], &frames[3]) > 0.9999);
        let _ = std::fs::remove_dir_all(&dir);
        // something that is not a table says so
        assert!(analyse(&vec![0.0; 1000], sr, 8, None).is_err());
    }
}
