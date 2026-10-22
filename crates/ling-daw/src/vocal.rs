//! Tuning a vocal recording to the notes of the score.
//!
//! 1. **Pitch**: a YIN tracker finds the voice's pitch every 5 ms (on a 12 kHz copy, so a minute of singing takes about a second).
//! 2. **Transients**: where a note starts in the score, the nearest start of a sung (voiced) sound within a window is found and the
//!    audio is time-warped so that it lands on the beat.
//! 3. **Tuning**: each moment is pulled toward the pitch of the note the score has there (octave-folded, with a strength and a
//!    retune speed), optionally with vibrato added to the long notes.
//! 4. **Re-synthesis**: pitch-synchronous overlap-add (PSOLA), which moves pitch and time while keeping the formants, so the voice
//!    keeps its character.
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct VNote {
    /// seconds in the recording's own time line (where the score wants the note)
    pub on: f64,
    pub off: f64,
    pub hz: f64,
}

#[derive(Clone, Debug)]
pub struct TuneOpts {
    /// 0..1: how far toward the note
    pub strength: f64,
    /// time constant of the pitch correction in ms (0 = hard tune)
    pub retune_ms: f64,
    pub align: bool,
    pub max_shift: f64,
    pub vibrato: bool,
    pub vib_depth_cents: f64,
    pub vib_rate: f64,
    /// notes shorter than this get no vibrato (seconds)
    pub vib_min_len: f64,
    /// tremolo: depth 0..1 of the loudness swing inside every note, and its rate in Hz
    pub trem_depth: f64,
    pub trem_rate: f64,
    /// fade in / out of every note in seconds (0 = none)
    pub fade_in: f64,
    pub fade_out: f64,
}
impl Default for TuneOpts {
    fn default() -> Self {
        TuneOpts { strength: 1.0, retune_ms: 25.0, align: true, max_shift: 0.12, vibrato: false, vib_depth_cents: 25.0, vib_rate: 5.5, vib_min_len: 0.6, trem_depth: 0.0, trem_rate: 5.0, fade_in: 0.0, fade_out: 0.0 }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub notes: usize,
    pub anchors: usize,
    pub voiced: f64,
    pub median_hz: f64,
    pub cents_before: f64,
    pub cents_after: f64,
}

pub struct Pitch {
    /// seconds per f0 entry
    pub hop: f64,
    /// Hz, 0 = not voiced; entry j describes the time j * hop
    pub f0: Vec<f32>,
}
impl Pitch {
    pub fn at(&self, t: f64) -> f32 {
        let j = (t / self.hop).round();
        if j < 0.0 {
            return 0.0;
        }
        self.f0.get(j as usize).copied().unwrap_or(0.0)
    }
}

pub fn cents(a: f64, b: f64) -> f64 {
    1200.0 * (a / b).log2()
}

pub fn midi_hz(k: f64) -> f64 {
    440.0 * 2f64.powf((k - 69.0) / 12.0)
}

/// the octave of `target_hz` that is closest (in pitch) to `f`
fn fold_octave(target: f64, f: f64) -> f64 {
    let k = (f / target).log2().round();
    target * 2f64.powf(k)
}

pub fn mono_of(stereo: &[f32]) -> Vec<f32> {
    stereo.chunks_exact(2).map(|f| (f[0] + f[1]) * 0.5).collect()
}

// ── pitch tracking ──────────────────────────────────────────────────────────
pub fn track_pitch(mono: &[f32], sr: f32) -> Pitch {
    let dec = ((sr / 12000.0).round() as usize).max(1);
    let sd = sr as f64 / dec as f64;
    let d: Vec<f32> = mono.chunks(dec).map(|c| c.iter().sum::<f32>() / c.len() as f32).collect();
    let hop_s = 0.005;
    let hop = ((hop_s * sd) as usize).max(1);
    let w = (0.036 * sd) as usize;
    let tau_min = (sd / 800.0).max(2.0) as usize;
    let tau_max = ((sd / 70.0) as usize).min(w);
    let nfr = d.len() / hop + 1;
    let global_rms = (d.iter().map(|x| x * x).sum::<f32>() / d.len().max(1) as f32).sqrt();
    let mut f0 = vec![0.0f32; nfr];
    let mut diff = vec![0.0f32; tau_max + 2];
    let mut cm = vec![0.0f32; tau_max + 2];
    for j in 0..nfr {
        let centre = j * hop;
        let start = centre as isize - (w / 2) as isize;
        let at = |i: usize| -> f32 {
            let k = start + i as isize;
            if k < 0 || k as usize >= d.len() {
                0.0
            } else {
                d[k as usize]
            }
        };
        let rms = ((0..w).step_by(2).map(|i| at(i) * at(i)).sum::<f32>() / (w / 2).max(1) as f32).sqrt();
        if rms < global_rms * 0.06 || rms < 1e-4 {
            continue;
        }
        for tau in 1..=tau_max {
            let mut s = 0.0f32;
            for i in 0..w {
                let a = at(i) - at(i + tau);
                s += a * a;
            }
            diff[tau] = s;
        }
        let mut run = 0.0f32;
        cm[0] = 1.0;
        for tau in 1..=tau_max {
            run += diff[tau];
            cm[tau] = if run > 0.0 { diff[tau] * tau as f32 / run } else { 1.0 };
        }
        let mut pick = 0usize;
        let mut tau = tau_min;
        while tau < tau_max {
            if cm[tau] < 0.16 {
                while tau + 1 < tau_max && cm[tau + 1] < cm[tau] {
                    tau += 1;
                }
                pick = tau;
                break;
            }
            tau += 1;
        }
        if pick == 0 {
            // no clear dip: take the deepest one if it is reasonably periodic
            let (bt, bv) = (tau_min..tau_max).map(|t| (t, cm[t])).fold((0, 1.0), |a, b| if b.1 < a.1 { b } else { a });
            if bv < 0.28 {
                pick = bt;
            }
        }
        if pick > 0 && pick + 1 <= tau_max {
            let (a, b, c) = (cm[pick - 1], cm[pick], cm[pick + 1]);
            let den = a - 2.0 * b + c;
            let shift = if den.abs() > 1e-9 { 0.5 * (a - c) / den } else { 0.0 };
            f0[j] = (sd / (pick as f64 + shift as f64)) as f32;
        }
    }
    // a 5-point median over voiced runs removes octave glitches
    let src = f0.clone();
    for j in 2..nfr.saturating_sub(2) {
        if src[j] > 0.0 {
            let mut v: Vec<f32> = (j - 2..=j + 2).map(|k| src[k]).filter(|x| *x > 0.0).collect();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            f0[j] = v[v.len() / 2];
        }
    }
    // a lone voiced frame is noise
    let src = f0.clone();
    for j in 1..nfr.saturating_sub(1) {
        if src[j] > 0.0 && src[j - 1] == 0.0 && src[j + 1] == 0.0 {
            f0[j] = 0.0;
        }
    }
    Pitch { hop: hop as f64 / sd, f0 }
}

/// short-time level at 1 ms resolution
fn envelope(mono: &[f32], sr: f32) -> (Vec<f32>, f64) {
    let step = ((0.001 * sr) as usize).max(1);
    let win = step * 4;
    let env: Vec<f32> = (0..mono.len() / step).map(|i| {
        let a = (i * step).saturating_sub(win / 2);
        let b = (a + win).min(mono.len());
        ((a..b).map(|k| mono[k] * mono[k]).sum::<f32>() / (b - a).max(1) as f32).sqrt()
    }).collect();
    (env, step as f64 / sr as f64)
}

/// start times (seconds) of sung sounds: where the voice becomes pitched after being silent or unpitched, refined with the level
pub fn voiced_onsets(p: &Pitch, mono: &[f32], sr: f32) -> Vec<f64> {
    let (env, es) = envelope(mono, sr);
    let mut out = vec![];
    let mut j = 1;
    while j < p.f0.len() {
        if p.f0[j] > 0.0 && p.f0[j - 1] == 0.0 {
            let tc = j as f64 * p.hop;
            // the pitch tracker sees the voicing a little late: go back to where the level starts to rise
            let lo = ((tc - 0.045) / es).max(0.0) as usize;
            let hi = (((tc + 0.06) / es) as usize).min(env.len().saturating_sub(1));
            if hi > lo + 3 {
                let peak = env[lo..=hi].iter().cloned().fold(0.0, f32::max);
                let k = (lo..=hi).find(|k| env[*k] >= peak * 0.22).unwrap_or(lo);
                out.push(k as f64 * es);
            } else {
                out.push(tc);
            }
        }
        j += 1;
    }
    out
}

/// energy onsets for sounds that have no pitch (a consonant-led word, a whisper): strong rises of the level
fn energy_onsets(mono: &[f32], sr: f32) -> Vec<f64> {
    let (env, es) = envelope(mono, sr);
    let mut out = vec![];
    let gap = (0.06 / es) as usize;
    let mut last = 0usize;
    for k in gap..env.len().saturating_sub(5) {
        let before = env[k - gap..k].iter().cloned().fold(0.0, f32::max) + 1e-4;
        if env[k] > before * 3.0 && env[k] > 0.01 && (out.is_empty() || k - last > gap) {
            out.push(k as f64 * es);
            last = k;
        }
    }
    out
}

// ── the time map ────────────────────────────────────────────────────────────
struct TimeMap {
    pts: Vec<(f64, f64)>, // (output time, input time)
}
impl TimeMap {
    fn map(&self, t: f64) -> f64 {
        if self.pts.is_empty() {
            return t;
        }
        if t <= self.pts[0].0 {
            return self.pts[0].1 + (t - self.pts[0].0);
        }
        for w in self.pts.windows(2) {
            if t <= w[1].0 {
                let f = (t - w[0].0) / (w[1].0 - w[0].0).max(1e-9);
                return w[0].1 + f * (w[1].1 - w[0].1);
            }
        }
        let l = self.pts.last().unwrap();
        l.1 + (t - l.0)
    }
}

fn build_time_map(notes: &[VNote], p: &Pitch, mono: &[f32], sr: f32, o: &TuneOpts) -> TimeMap {
    let mut pts: Vec<(f64, f64)> = vec![];
    if !o.align {
        return TimeMap { pts };
    }
    let voiced = voiced_onsets(p, mono, sr);
    let energy = energy_onsets(mono, sr);
    let mut sorted: Vec<&VNote> = notes.iter().collect();
    sorted.sort_by(|a, b| a.on.partial_cmp(&b.on).unwrap());
    for n in sorted {
        let near = |v: &Vec<f64>| v.iter().cloned().filter(|t| (t - n.on).abs() <= o.max_shift).min_by(|a, b| (a - n.on).abs().partial_cmp(&(b - n.on).abs()).unwrap());
        let cand = near(&voiced).or_else(|| near(&energy));
        if let Some(t_in) = cand {
            if (t_in - n.on).abs() < 0.004 {
                continue; // already there
            }
            if let Some(last) = pts.last() {
                let slope = (t_in - last.1) / (n.on - last.0).max(1e-6);
                if t_in <= last.1 + 0.02 || !(0.6..=1.7).contains(&slope) {
                    continue;
                }
            }
            pts.push((n.on, t_in));
        }
    }
    TimeMap { pts }
}

// ── the correction contour ──────────────────────────────────────────────────
/// pitch ratio per 5 ms of output time (1.0 = leave alone), plus the mean cents error before correction
fn ratio_grid(notes: &[VNote], p: &Pitch, tm: &TimeMap, n_out: usize, sr: f32, o: &TuneOpts) -> (Vec<f32>, f64, usize) {
    let hop = p.hop;
    let nj = (n_out as f64 / sr as f64 / hop) as usize + 2;
    let mut corr = vec![0.0f64; nj];
    let mut desired = vec![0.0f64; nj];
    let mut active = vec![false; nj];
    let mut err_sum = 0.0;
    let mut err_n = 0usize;
    let mut sorted: Vec<&VNote> = notes.iter().collect();
    sorted.sort_by(|a, b| a.on.partial_cmp(&b.on).unwrap());
    for j in 0..nj {
        let t = j as f64 * hop;
        let f = p.at(tm.map(t)) as f64;
        if f <= 0.0 {
            continue;
        }
        // the note the score has at this moment (a little slack at the ends for sloppy timing)
        let note = sorted.iter().rev().find(|n| t >= n.on - 0.03 && t <= n.off + 0.06);
        if let Some(n) = note {
            let target = fold_octave(n.hz, f);
            let c = cents(target, f);
            if c.abs() <= 700.0 {
                desired[j] = c * o.strength;
                active[j] = true;
                err_sum += c.abs();
                err_n += 1;
            }
        }
    }
    let tau = (o.retune_ms / 1000.0).max(0.001);
    let alpha = 1.0 - (-hop / tau).exp();
    let mut prev_active = false;
    for j in 0..nj {
        if active[j] {
            corr[j] = if prev_active { corr[j - 1] + alpha * (desired[j] - corr[j - 1]) } else { desired[j] };
        } else if prev_active {
            // unvoiced or between notes: let the correction relax instead of snapping back
            corr[j] = corr[j - 1] * 0.8;
        }
        prev_active = active[j] || (prev_active && corr[j].abs() > 0.5 && j > 0 && p.at(tm.map(j as f64 * hop)) > 0.0);
        if !prev_active {
            corr[j] = if j > 0 { corr[j - 1] * 0.5 } else { 0.0 };
        }
    }
    if o.vibrato {
        for n in &notes_sorted(notes) {
            if n.off - n.on < o.vib_min_len {
                continue;
            }
            let delay = 0.18;
            let j0 = ((n.on + delay) / hop) as usize;
            let j1 = ((n.off - 0.04) / hop) as usize;
            for j in j0..j1.min(nj) {
                let t = j as f64 * hop;
                if p.at(tm.map(t)) <= 0.0 {
                    continue;
                }
                let fade = ((t - n.on - delay) / 0.3).clamp(0.0, 1.0);
                // the vibrato dies away at the very end of a note as a singer's does
                let tail = ((n.off - t) / 0.12).clamp(0.0, 1.0);
                corr[j] += o.vib_depth_cents * fade * tail * (std::f64::consts::TAU * o.vib_rate * (t - n.on - delay)).sin();
            }
        }
    }
    let ratio: Vec<f32> = corr.iter().map(|c| 2f64.powf(c / 1200.0) as f32).collect();
    (ratio, if err_n > 0 { err_sum / err_n as f64 } else { 0.0 }, err_n)
}

fn notes_sorted(notes: &[VNote]) -> Vec<VNote> {
    let mut v = notes.to_vec();
    v.sort_by(|a, b| a.on.partial_cmp(&b.on).unwrap());
    v
}

// ── PSOLA ───────────────────────────────────────────────────────────────────
/// analysis marks (sample positions): one per period inside the voiced stretches
fn pitch_marks(mono: &[f32], p: &Pitch, sr: f32) -> Vec<usize> {
    let mut marks: Vec<usize> = vec![];
    let n = mono.len();
    let mut pos = 0usize;
    let mut j = 0usize;
    while j < p.f0.len() {
        if p.f0[j] <= 0.0 {
            j += 1;
            continue;
        }
        // a voiced run
        let mut e = j;
        while e < p.f0.len() && p.f0[e] > 0.0 {
            e += 1;
        }
        let t0 = (j as f64 * p.hop * sr as f64) as usize;
        let t1 = ((e as f64 * p.hop * sr as f64) as usize).min(n);
        let per0 = (sr / p.f0[j]) as usize;
        let mut cur = t0.max(pos);
        // one polarity for the whole run: marks sit on the strongest peak of the side that is larger, so they land on the same
        // feature of every period instead of alternating between two similar peaks
        let probe_hi = (cur + per0 * 4).min(t1.max(cur + 1)).min(n);
        let (mx, mn) = mono[cur.min(n.saturating_sub(1))..probe_hi.max(cur.min(n.saturating_sub(1)))].iter().fold((0.0f32, 0.0f32), |m, x| (m.0.max(*x), m.1.min(*x)));
        let pol = if mx >= -mn { 1.0f32 } else { -1.0 };
        let hi = (cur + per0).min(n);
        if hi > cur {
            cur = (cur..hi).max_by(|a, b| (mono[*a] * pol).partial_cmp(&(mono[*b] * pol)).unwrap()).unwrap_or(cur);
        }
        while cur < t1 {
            marks.push(cur);
            let f = p.at(cur as f64 / sr as f64).max(60.0) as f64;
            let per = (sr as f64 / f) as usize;
            let centre = cur + per.max(8);
            let w = (per / 5).max(2);
            let lo = centre.saturating_sub(w);
            let hi = (centre + w).min(n.saturating_sub(1));
            if hi <= lo {
                break;
            }
            cur = (lo..=hi).max_by(|a, b| (mono[*a] * pol).partial_cmp(&(mono[*b] * pol)).unwrap()).unwrap_or(centre);
        }
        pos = t1;
        j = e;
    }
    marks.dedup();
    marks
}

fn nearest_mark(marks: &[usize], t: usize) -> usize {
    let i = marks.partition_point(|m| *m < t);
    if i == 0 {
        return marks[0];
    }
    if i >= marks.len() {
        return marks[marks.len() - 1];
    }
    if t - marks[i - 1] <= marks[i] - t {
        marks[i - 1]
    } else {
        marks[i]
    }
}

struct Windows(HashMap<usize, Vec<f32>>);
impl Windows {
    fn get(&mut self, len: usize) -> &Vec<f32> {
        self.0.entry(len).or_insert_with(|| (0..len).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * (i as f32 + 0.5) / len as f32).cos()).collect())
    }
}

/// Re-synthesise `stereo` with pitch-synchronous overlap-add: pitch moves by `ratio` (one value per `p.hop` of output time), time follows `tm`.
fn resynth(stereo: &[f32], mono: &[f32], sr: f32, p: &Pitch, tm: &TimeMap, ratio: &[f32]) -> Vec<f32> {
    resynth_n(stereo, mono, sr, p, tm, ratio, mono.len())
}

/// [`resynth`] into `n_out` frames of output (longer or shorter than the input when the time map stretches the recording)
fn resynth_n(stereo: &[f32], mono: &[f32], sr: f32, p: &Pitch, tm: &TimeMap, ratio: &[f32], n_out: usize) -> Vec<f32> {
    let n = mono.len();
    if n == 0 || n_out == 0 {
        return vec![0.0; n_out * 2];
    }
    let marks = pitch_marks(mono, p, sr);
    let mut out = vec![0.0f32; n_out * 2];
    let mut wsum = vec![0.0f32; n_out];
    let mut wins = Windows(HashMap::new());
    let grid_hop = p.hop * sr as f64;
    let mut cur = 0.0f64;
    let un_len = ((0.012 * sr) as usize) | 1;
    let un_hop = 0.006 * sr as f64;
    let mut guard = 0usize;
    while cur < n_out as f64 && guard < n_out * 4 {
        guard += 1;
        let t_out = cur / sr as f64;
        let t_in = tm.map(t_out);
        let in_pos = (t_in * sr as f64).clamp(0.0, (n - 1) as f64);
        let f = p.at(t_in) as f64;
        let (centre, len, hop_out) = if f > 0.0 && !marks.is_empty() {
            let per_in = (sr as f64 / f).max(8.0);
            let r = ratio.get((cur / grid_hop) as usize).copied().unwrap_or(1.0) as f64;
            let m = nearest_mark(&marks, in_pos as usize);
            (m, ((2.0 * per_in) as usize) | 1, per_in / r.clamp(0.5, 2.0))
        } else {
            (in_pos as usize, un_len, un_hop)
        };
        let half = len / 2;
        let win = wins.get(len).clone();
        let oc = cur as isize;
        for (k, w) in win.iter().enumerate() {
            let src = centre as isize + k as isize - half as isize;
            let dst = oc + k as isize - half as isize;
            if src < 0 || src as usize >= n || dst < 0 || dst as usize >= n_out {
                continue;
            }
            let (s, d) = (src as usize, dst as usize);
            out[d * 2] += stereo[s * 2] * w;
            out[d * 2 + 1] += stereo[s * 2 + 1] * w;
            wsum[d] += w;
        }
        cur += hop_out.max(2.0);
    }
    for i in 0..n_out {
        let g = 1.0 / wsum[i].max(0.25);
        out[i * 2] *= g;
        out[i * 2 + 1] *= g;
        if !out[i * 2].is_finite() {
            out[i * 2] = 0.0;
        }
        if !out[i * 2 + 1].is_finite() {
            out[i * 2 + 1] = 0.0;
        }
    }
    out
}

/// Tune and align `stereo` (interleaved) to the score's `notes`. Returns the new audio (same length) and a report.
pub fn tune(stereo: &[f32], sr: f32, notes: &[VNote], o: &TuneOpts) -> (Vec<f32>, Report) {
    let mono = mono_of(stereo);
    let n = mono.len();
    let mut rep = Report { notes: notes.len(), ..Report::default() };
    if n < (sr * 0.2) as usize || notes.is_empty() {
        return (stereo.to_vec(), rep);
    }
    let p = track_pitch(&mono, sr);
    let voiced: Vec<f32> = p.f0.iter().cloned().filter(|f| *f > 0.0).collect();
    rep.voiced = voiced.len() as f64 / p.f0.len().max(1) as f64;
    if !voiced.is_empty() {
        let mut v = voiced.clone();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        rep.median_hz = v[v.len() / 2] as f64;
    }
    let tm = build_time_map(notes, &p, &mono, sr, o);
    rep.anchors = tm.pts.len();
    let (ratio, before, _) = ratio_grid(notes, &p, &tm, n, sr, o);
    rep.cents_before = before;
    let out = resynth(stereo, &mono, sr, &p, &tm, &ratio);
    // how well did it land? track the result and compare it to the notes
    let after = track_pitch(&mono_of(&out), sr);
    let mut sum = 0.0;
    let mut cnt = 0;
    let sorted = notes_sorted(notes);
    for (j, f) in after.f0.iter().enumerate() {
        if *f > 0.0 {
            let t = j as f64 * after.hop;
            if let Some(nt) = sorted.iter().rev().find(|nt| t >= nt.on && t <= nt.off) {
                sum += cents(*f as f64, fold_octave(nt.hz, *f as f64)).abs();
                cnt += 1;
            }
        }
    }
    rep.cents_after = if cnt > 0 { sum / cnt as f64 } else { 0.0 };
    let mut out = out;
    apply_note_dynamics(&mut out, sr, &sorted, o);
    (out, rep)
}

// ── the inline vocal editor's render ────────────────────────────────────────

/// One sung note of an edited vocal: where it is in the recording (`src_*`, seconds), where the edit puts it (`dst_*`, seconds on the
/// same time line), its detected pitch (fractional MIDI), how far it is moved (semitones), how much of its pitch drift is flattened
/// toward its centre (0..1) and its loudness (linear gain).
#[derive(Clone, Debug)]
pub struct Seg {
    pub src_on: f64,
    pub src_off: f64,
    pub dst_on: f64,
    pub dst_off: f64,
    pub midi: f64,
    pub shift: f64,
    pub drift: f64,
    pub gain: f64,
}

#[derive(Clone, Debug)]
pub struct EditOpts {
    /// vibrato added to notes of 0.3 s and longer (0 = none)
    pub vib_depth_cents: f64,
    pub vib_rate: f64,
    /// how fast the pitch moves from one note's correction to the next (ms; the glide between notes)
    pub retune_ms: f64,
}
impl Default for EditOpts {
    fn default() -> Self {
        EditOpts { vib_depth_cents: 0.0, vib_rate: 5.5, retune_ms: 12.0 }
    }
}

/// The time map an edit makes: both ends of every note, in order (a point that would run backwards is skipped). The sound between two
/// notes (breaths, consonants) is stretched to fill the gap; before the first note and after the last the recording runs as it was.
fn seg_time_map(segs: &[Seg]) -> TimeMap {
    let mut pts: Vec<(f64, f64)> = vec![];
    for g in segs {
        for (o, i) in [(g.dst_on, g.src_on), (g.dst_off, g.src_off)] {
            if let Some(l) = pts.last() {
                if o <= l.0 + 1e-4 || i <= l.1 + 1e-4 {
                    continue;
                }
            }
            pts.push((o, i));
        }
    }
    TimeMap { pts }
}

/// where the recording time `t_in` lands after the edit (the inverse of the time map)
pub fn segs_out_time(segs: &[Seg], t_in: f64) -> f64 {
    let tm = seg_time_map(segs);
    if tm.pts.is_empty() {
        return t_in;
    }
    if t_in <= tm.pts[0].1 {
        return tm.pts[0].0 + (t_in - tm.pts[0].1);
    }
    for w in tm.pts.windows(2) {
        if t_in <= w[1].1 {
            let f = (t_in - w[0].1) / (w[1].1 - w[0].1).max(1e-9);
            return w[0].0 + f * (w[1].0 - w[0].0);
        }
    }
    let l = tm.pts.last().unwrap();
    l.0 + (t_in - l.1)
}

/// Render an edited vocal: every note gets its own pitch move, drift correction, timing / length (time-slice stretch) and gain, with
/// pitch-synchronous resynthesis (the formants stay). `p` is the pitch track of `stereo`. The output may be longer or shorter than the input.
pub fn render_segments(stereo: &[f32], sr: f32, p: &Pitch, segs: &[Seg], o: &EditOpts) -> Vec<f32> {
    let mono = mono_of(stereo);
    let n = mono.len();
    let mut s: Vec<Seg> = segs.iter().filter(|g| g.dst_off > g.dst_on && g.src_off > g.src_on).cloned().collect();
    s.sort_by(|a, b| a.dst_on.total_cmp(&b.dst_on));
    let tm = seg_time_map(&s);
    let end_out = segs_out_time(&s, n as f64 / sr as f64).max(0.05);
    let n_out = (end_out * sr as f64) as usize;
    let hop = p.hop;
    let nj = (n_out as f64 / sr as f64 / hop) as usize + 2;
    let mut desired = vec![0.0f64; nj];
    let mut active = vec![false; nj];
    let mut k = 0usize;
    for j in 0..nj {
        let t = j as f64 * hop;
        while k < s.len() && s[k].dst_off + 0.01 < t {
            k += 1;
        }
        let Some(g) = s.get(k).filter(|g| t >= g.dst_on - 0.01) else { continue };
        let f = p.at(tm.map(t)) as f64;
        if f <= 0.0 {
            continue;
        }
        let mut c = g.shift * 100.0 + g.drift.clamp(0.0, 1.0) * cents(midi_hz(g.midi), f);
        let len = g.dst_off - g.dst_on;
        if o.vib_depth_cents > 0.0 && len >= 0.3 {
            let delay = 0.15_f64.min(len * 0.3);
            let fade = ((t - g.dst_on - delay) / 0.25).clamp(0.0, 1.0);
            let tail = ((g.dst_off - t) / 0.1).clamp(0.0, 1.0);
            c += o.vib_depth_cents * fade * tail * (std::f64::consts::TAU * o.vib_rate * (t - g.dst_on - delay).max(0.0)).sin();
        }
        desired[j] = c;
        active[j] = true;
    }
    // glide between notes; an unpitched stretch keeps the last correction (its grains do not use it, and the next note starts from there)
    let alpha = 1.0 - (-hop / (o.retune_ms / 1000.0).max(0.001)).exp();
    let mut corr = vec![0.0f64; nj];
    let mut prev = 0.0f64;
    let mut was = false;
    for j in 0..nj {
        if active[j] {
            prev = if was { prev + alpha * (desired[j] - prev) } else { desired[j] };
            was = true;
        } else {
            was = false;
        }
        corr[j] = prev;
    }
    let ratio: Vec<f32> = corr.iter().map(|c| 2f64.powf(c / 1200.0) as f32).collect();
    let mut out = resynth_n(stereo, &mono, sr, p, &tm, &ratio, n_out);
    // loudness of every note, with 10 ms ramps so a gain change never clicks
    let ramp = 0.01;
    for g in &s {
        if (g.gain - 1.0).abs() < 1e-3 {
            continue;
        }
        let i0 = (((g.dst_on - ramp) * sr as f64).max(0.0) as usize).min(n_out);
        let i1 = (((g.dst_off + ramp) * sr as f64) as usize).min(n_out);
        for i in i0..i1 {
            let t = i as f64 / sr as f64;
            let w = ((t - (g.dst_on - ramp)) / ramp).clamp(0.0, 1.0).min(((g.dst_off + ramp - t) / ramp).clamp(0.0, 1.0));
            let gain = (1.0 + (g.gain.max(0.0) - 1.0) * w) as f32;
            out[i * 2] *= gain;
            out[i * 2 + 1] *= gain;
        }
    }
    out
}

/// loudness (RMS) of `mono` per `hop` seconds
pub fn level_track(mono: &[f32], sr: f32, hop: f64) -> Vec<f32> {
    let step = ((hop * sr as f64) as usize).max(1);
    let win = step * 2;
    (0..mono.len() / step + 1)
        .map(|i| {
            let a = (i * step).saturating_sub(step / 2).min(mono.len());
            let b = (a + win).min(mono.len());
            if b <= a { 0.0 } else { ((a..b).map(|k| mono[k] * mono[k]).sum::<f32>() / (b - a) as f32).sqrt() }
        })
        .collect()
}

/// tremolo and fades inside every note (the sound between notes is left alone)
fn apply_note_dynamics(out: &mut [f32], sr: f32, notes: &[VNote], o: &TuneOpts) {
    if o.trem_depth <= 0.0 && o.fade_in <= 0.0 && o.fade_out <= 0.0 {
        return;
    }
    let n = out.len() / 2;
    for nt in notes {
        let (i0, i1) = (((nt.on * sr as f64) as usize).min(n), ((nt.off * sr as f64) as usize).min(n));
        for i in i0..i1 {
            let t = i as f64 / sr as f64;
            let mut g = 1.0;
            if o.trem_depth > 0.0 {
                // 1 at the top of the swing, 1 - depth at the bottom
                g *= 1.0 - o.trem_depth.clamp(0.0, 1.0) * 0.5 * (1.0 - (std::f64::consts::TAU * o.trem_rate * (t - nt.on)).cos());
            }
            if o.fade_in > 0.0 {
                g *= ((t - nt.on) / o.fade_in).clamp(0.0, 1.0);
            }
            if o.fade_out > 0.0 {
                g *= ((nt.off - t) / o.fade_out).clamp(0.0, 1.0);
            }
            out[i * 2] *= g as f32;
            out[i * 2 + 1] *= g as f32;
        }
    }
}

/// a note found in a recording
#[derive(Clone, Debug)]
pub struct DNote {
    /// seconds in the recording
    pub on: f64,
    pub off: f64,
    /// the note's pitch (median over its length, so vibrato does not count) in fractional MIDI
    pub midi: f64,
    /// 0..1 loudness is filled in by the caller when it has the audio
    pub vel: f32,
}

/// Cut a pitch track into sung notes: a note ends when the pitch leaves it for a few frames or the voice stops for 60 ms; notes under
/// 70 ms are dropped; neighbours on the same semitone with a hair of a gap are joined.
pub fn detect_notes(p: &Pitch) -> Vec<DNote> {
    let nj = p.f0.len();
    let m: Vec<f64> = p.f0.iter().map(|f| if *f > 0.0 { 69.0 + 12.0 * (*f as f64 / 440.0).log2() } else { f64::NAN }).collect();
    let median = |v: &mut Vec<f64>| -> f64 {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[v.len() / 2]
    };
    let gap_frames = ((0.06 / p.hop) as usize).max(2);
    let mut out: Vec<DNote> = vec![];
    let mut j = 0;
    while j < nj {
        if m[j].is_nan() {
            j += 1;
            continue;
        }
        let j0 = j;
        let mut vals: Vec<f64> = vec![m[j]];
        let mut last = j;
        let mut k = j + 1;
        while k < nj {
            if m[k].is_nan() {
                if k - last > gap_frames {
                    break;
                }
            } else {
                let mut r: Vec<f64> = vals.iter().rev().take(8).cloned().collect();
                let med = median(&mut r);
                if (m[k] - med).abs() > 0.7 {
                    let stay = (k..(k + 4).min(nj)).filter(|q| !m[*q].is_nan() && (m[*q] - med).abs() > 0.7).count();
                    if stay >= 3 {
                        break;
                    }
                } else {
                    vals.push(m[k]);
                    last = k;
                }
            }
            k += 1;
        }
        let j1 = last + 1;
        let dur = (j1 - j0) as f64 * p.hop;
        if dur >= 0.07 && vals.len() >= 4 {
            let mut v = vals.clone();
            let c = median(&mut v);
            out.push(DNote { on: j0 as f64 * p.hop, off: j1 as f64 * p.hop, midi: c, vel: 0.8 });
        }
        j = k.max(j1);
    }
    // join neighbours that are one note with a hiccup between
    let mut joined: Vec<DNote> = vec![];
    for n in out {
        if let Some(l) = joined.last_mut() {
            if n.on - l.off < 0.05 && (n.midi.round() - l.midi.round()).abs() < 0.5 {
                l.off = n.off;
                continue;
            }
        }
        joined.push(n);
    }
    joined
}

/// Deviations in cents from equal temperament of the twelve notes counted from the key's tonic, for the temperaments the vocal tuner offers
/// (0 equal, 1 just intonation, 2 Pythagorean, 3 quarter-comma meantone, 4 Werckmeister III).
pub const TEMPERAMENT_NAMES: [&str; 5] = ["Equal", "Just", "Pythagorean", "Meantone", "Werckmeister"];
pub const TEMPERAMENTS: [[f64; 12]; 5] = [
    [0.0; 12],
    [0.0, 11.73, 3.91, 15.64, -13.69, -1.96, -9.78, 1.96, 13.69, -15.64, 17.60, -11.73],
    [0.0, -9.78, 3.91, -5.87, 7.82, -1.96, 11.73, 1.96, -7.82, 5.87, -3.91, 9.78],
    [0.0, -23.95, -6.84, 10.26, -13.69, 3.42, -20.53, -3.42, -27.37, -10.26, 6.84, -17.11],
    [0.0, -9.78, -7.82, -5.87, -9.78, -1.96, -11.73, -3.91, -7.82, -11.73, -3.91, -7.82],
];

/// frequency of a MIDI key in a temperament whose tonic is pitch class `root`
pub fn key_hz(key: f64, temperament: usize, root: i32) -> f64 {
    let t = TEMPERAMENTS[temperament.min(TEMPERAMENTS.len() - 1)];
    let pc = ((key.round() as i32) - root).rem_euclid(12) as usize;
    midi_hz(key + t[pc] / 100.0)
}


// ── tuning to a scale instead of to a score ─────────────────────────────────

/// cubic Bezier easing from 0 to 1 with control heights `y1`, `y2` (0 and 1 give the smooth S-curve; 0.9 and 1 a quick snap; 0 and 0.4 a lazy one)
pub fn bezier_ease(u: f64, y1: f64, y2: f64) -> f64 {
    let u = u.clamp(0.0, 1.0);
    let v = 1.0 - u;
    (3.0 * v * v * u * y1 + 3.0 * v * u * u * y2 + u * u * u).clamp(-0.05, 1.05)
}

/// The nearest allowed pitch (a MIDI note number) to the fractional note `m`; `prefer` keeps the last note when it is nearly as close
/// (so a sung slide does not flip between two neighbours).
pub fn nearest_allowed(m: f64, allowed: &[bool; 12], prefer: Option<i32>) -> i32 {
    let base = m.round() as i32;
    let mut best = base;
    let mut bd = f64::MAX;
    for k in base - 3..=base + 3 {
        if allowed[k.rem_euclid(12) as usize] {
            let d = (k as f64 - m).abs();
            if d < bd {
                bd = d;
                best = k;
            }
        }
    }
    if let Some(p) = prefer {
        if allowed[p.rem_euclid(12) as usize] && (p as f64 - m).abs() <= bd + 0.18 {
            return p;
        }
    }
    best
}

/// Pull a monophonic melodic recording (a voice, a lead) onto the notes of a scale.
///
/// The pitch track is cut into sung notes (a note ends when the pitch leaves it for a few frames or the voice stops). Each note's
/// centre (its median, so vibrato does not count) is moved to the nearest note the scale allows, and the whole note is shifted by
/// that amount so the singer's own vibrato and inflection stay. Where one note leads straight into the next, the two corrections
/// are joined by a cubic Bezier ease of `glide_ms` instead of stepping, so the tuning slides like a hand turning a knob.
pub fn tune_to_scale(stereo: &[f32], sr: f32, allowed: &[bool; 12], strength: f64, glide_ms: f64, ease: (f64, f64)) -> (Vec<f32>, Report) {
    let mono = mono_of(stereo);
    let n = mono.len();
    let mut rep = Report::default();
    if n < (sr * 0.2) as usize {
        return (stereo.to_vec(), rep);
    }
    let p = track_pitch(&mono, sr);
    let nj = p.f0.len();
    let m: Vec<f64> = p.f0.iter().map(|f| if *f > 0.0 { 69.0 + 12.0 * (*f as f64 / 440.0).log2() } else { f64::NAN }).collect();
    rep.voiced = p.f0.iter().filter(|f| **f > 0.0).count() as f64 / nj.max(1) as f64;
    // cut into notes
    struct Seg {
        j0: usize,
        j1: usize,
        shift: f64,
        centre: f64,
    }
    let mut segs: Vec<Seg> = vec![];
    let median = |v: &mut Vec<f64>| -> f64 {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[v.len() / 2]
    };
    let mut j = 0;
    while j < nj {
        if m[j].is_nan() {
            j += 1;
            continue;
        }
        let j0 = j;
        let mut vals: Vec<f64> = vec![m[j]];
        let mut last = j;
        let mut k = j + 1;
        while k < nj {
            if m[k].is_nan() {
                if k - last > 3 {
                    break;
                }
            } else {
                let recent: Vec<f64> = vals.iter().rev().take(8).cloned().collect();
                let mut r = recent.clone();
                let med = median(&mut r);
                if (m[k] - med).abs() > 0.75 {
                    // a candidate for a new note: it needs to stay away from the old one for 3 frames
                    let stay = (k..(k + 3).min(nj)).filter(|q| !m[*q].is_nan() && (m[*q] - med).abs() > 0.75).count();
                    if stay >= 2 {
                        break;
                    }
                } else {
                    vals.push(m[k]);
                    last = k;
                }
            }
            k += 1;
        }
        let j1 = last + 1;
        if vals.len() >= 6 {
            let mut v = vals.clone();
            let centre = median(&mut v);
            segs.push(Seg { j0, j1, shift: 0.0, centre });
        }
        j = k.max(j1);
    }
    // each note's target and shift
    let mut prev: Option<i32> = None;
    let mut before = 0.0;
    let mut after = 0.0;
    for sg in segs.iter_mut() {
        let t = nearest_allowed(sg.centre, allowed, prev);
        prev = Some(t);
        let off = (t as f64 - sg.centre) * 100.0;
        before += off.abs();
        let shift = off * strength.clamp(0.0, 1.0);
        sg.shift = if shift.abs() < 4.0 { 0.0 } else { shift };
        after += (off - sg.shift).abs();
    }
    rep.notes = segs.len();
    if segs.is_empty() {
        return (stereo.to_vec(), rep);
    }
    rep.cents_before = before / segs.len() as f64;
    rep.cents_after = after / segs.len() as f64;
    // the correction contour: constant per note, Bezier-eased where notes join
    let mut corr = vec![0.0f64; nj + 2];
    for sg in &segs {
        for q in sg.j0..sg.j1.min(nj) {
            corr[q] = sg.shift;
        }
    }
    let glide = (glide_ms / 1000.0).max(0.004);
    let gj = ((glide / p.hop) as usize).max(1);
    for w in segs.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        let gap = b.j0.saturating_sub(a.j1);
        if gap as f64 * p.hop > 0.06 || (a.shift - b.shift).abs() < 1e-6 {
            continue;
        }
        let centre = (a.j1 + b.j0) / 2;
        let lo = centre.saturating_sub(gj / 2);
        let hi = (centre + gj / 2).min(nj);
        for q in lo..hi {
            let u = (q - lo) as f64 / (hi - lo).max(1) as f64;
            let e = bezier_ease(u, ease.0, ease.1);
            corr[q] = a.shift + (b.shift - a.shift) * e;
        }
        // frames before / after the window keep their own note's shift
        for q in a.j0..lo.min(a.j1) {
            corr[q] = a.shift;
        }
        for q in hi.max(b.j0)..b.j1.min(nj) {
            corr[q] = b.shift;
        }
    }
    let ratio: Vec<f32> = corr.iter().map(|c| 2f64.powf(c / 1200.0) as f32).collect();
    let tm = TimeMap { pts: vec![] };
    let out = resynth(stereo, &mono, sr, &p, &tm, &ratio);
    // how well did it land?
    let res = track_pitch(&mono_of(&out), sr);
    let (mut sum, mut cnt) = (0.0, 0usize);
    for f in res.f0.iter().filter(|f| **f > 0.0) {
        let mm = 69.0 + 12.0 * (*f as f64 / 440.0).log2();
        let t = nearest_allowed(mm, allowed, None);
        sum += ((t as f64 - mm) * 100.0).abs();
        cnt += 1;
    }
    if cnt > 0 {
        rep.cents_after = sum / cnt as f64;
    }
    (out, rep)
}

/// how steadily a recording holds one pitch at a time: the share of voiced frame pairs whose pitch moves less than a semitone.
/// A solo voice scores high; a mix of chords and drums jumps around and scores low. Returns (voiced share, steadiness).
pub fn monophonic_score(stereo: &[f32], sr: f32) -> (f64, f64) {
    let mono = mono_of(stereo);
    if mono.len() < (sr * 0.3) as usize {
        return (0.0, 0.0);
    }
    let p = track_pitch(&mono, sr);
    let voiced = p.f0.iter().filter(|f| **f > 0.0).count() as f64 / p.f0.len().max(1) as f64;
    let (mut pairs, mut calm) = (0usize, 0usize);
    for w in p.f0.windows(2) {
        if w[0] > 0.0 && w[1] > 0.0 {
            pairs += 1;
            if (12.0 * (w[1] as f64 / w[0] as f64).log2()).abs() < 1.0 {
                calm += 1;
            }
        }
    }
    (voiced, if pairs > 0 { calm as f64 / pairs as f64 } else { 0.0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 44100.0;

    /// a crude voice: a harmonic-rich tone whose pitch can wander, with a soft attack
    fn voice(segments: &[(f64, f64, f64)], secs: f64, vib_cents: f64) -> Vec<f32> {
        let n = (secs * SR as f64) as usize;
        let mut out = vec![0.0f32; n * 2];
        let mut ph = 0.0f64;
        for i in 0..n {
            let t = i as f64 / SR as f64;
            if let Some((on, off, hz)) = segments.iter().find(|(a, b, _)| t >= *a && t < *b) {
                let f = hz * 2f64.powf(vib_cents / 1200.0 * (t * std::f64::consts::TAU * 5.5).sin());
                ph += f / SR as f64;
                let env = ((t - on) / 0.02).min(1.0).min((off - t) / 0.03).max(0.0);
                let mut v = 0.0;
                for h in 1..=8 {
                    v += (ph * h as f64 * std::f64::consts::TAU).sin() / h as f64;
                }
                out[i * 2] = (v * 0.25 * env) as f32;
                out[i * 2 + 1] = out[i * 2];
            }
        }
        out
    }

    fn median_pitch(x: &[f32], from: f64, to: f64) -> f64 {
        let p = track_pitch(&mono_of(x), SR);
        let mut v: Vec<f32> = p.f0.iter().enumerate().filter(|(j, f)| **f > 0.0 && (*j as f64 * p.hop) >= from && (*j as f64 * p.hop) <= to).map(|(_, f)| *f).collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        if v.is_empty() {
            0.0
        } else {
            v[v.len() / 2] as f64
        }
    }

    #[test]
    fn the_tracker_finds_the_pitch_of_a_voice_like_tone() {
        let x = voice(&[(0.1, 1.0, 220.0)], 1.2, 0.0);
        let f = median_pitch(&x, 0.3, 0.9);
        assert!((cents(f, 220.0)).abs() < 12.0, "{f}");
    }

    #[test]
    fn a_flat_note_is_pulled_onto_the_score_and_keeps_its_length_and_level() {
        // sung 45 cents flat of A3 (220 Hz), the score says A3
        let flat = 220.0 * 2f64.powf(-45.0 / 1200.0);
        let x = voice(&[(0.1, 1.4, flat)], 1.6, 0.0);
        let notes = vec![VNote { on: 0.1, off: 1.4, hz: 220.0 }];
        let before = median_pitch(&x, 0.3, 1.2);
        let (y, rep) = tune(&x, SR, &notes, &TuneOpts::default());
        let after = median_pitch(&y, 0.3, 1.2);
        assert_eq!(y.len(), x.len());
        assert!(cents(before, 220.0).abs() > 30.0, "{before}");
        assert!(cents(after, 220.0).abs() < 12.0, "{after} {rep:?}");
        assert!(rep.cents_before > 30.0 && rep.cents_after < rep.cents_before * 0.5, "{rep:?}");
        let rms = |v: &[f32]| (v.iter().map(|a| a * a).sum::<f32>() / v.len() as f32).sqrt();
        let (rx, ry) = (rms(&x), rms(&y));
        assert!(ry > rx * 0.7 && ry < rx * 1.4, "level kept: {rx} {ry}");
        assert!(y.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn strength_zero_leaves_the_pitch_and_a_sharp_note_is_pulled_down_to_the_nearest_octave() {
        let sharp = 440.0 * 2f64.powf(40.0 / 1200.0);
        let x = voice(&[(0.1, 1.2, sharp)], 1.4, 0.0);
        // the score has A3 (220) but the singer is in the octave above: the tuner folds the octave
        let notes = vec![VNote { on: 0.1, off: 1.2, hz: 220.0 }];
        let (y, _) = tune(&x, SR, &notes, &TuneOpts::default());
        assert!(cents(median_pitch(&y, 0.3, 1.0), 440.0).abs() < 15.0);
        let (z, _) = tune(&x, SR, &notes, &TuneOpts { strength: 0.0, ..TuneOpts::default() });
        assert!(cents(median_pitch(&z, 0.3, 1.0), sharp).abs() < 12.0, "untouched");
    }

    #[test]
    fn late_starts_are_moved_onto_the_beat() {
        // the singer comes in 70 ms after the score's note
        let x = voice(&[(0.57, 1.5, 220.0)], 1.7, 0.0);
        let notes = vec![VNote { on: 0.5, off: 1.5, hz: 220.0 }];
        let onset_of = |v: &[f32]| {
            let m = mono_of(v);
            let peak = m.iter().cloned().fold(0.0f32, |a, b| a.max(b.abs()));
            m.iter().position(|s| s.abs() > peak * 0.1).unwrap() as f64 / SR as f64
        };
        let before = onset_of(&x);
        let (y, rep) = tune(&x, SR, &notes, &TuneOpts { strength: 0.0, ..TuneOpts::default() });
        let after = onset_of(&y);
        assert!(rep.anchors >= 1, "{rep:?}");
        assert!((before - 0.57).abs() < 0.03, "{before}");
        assert!((after - 0.5).abs() < 0.03, "moved onto the beat: {after}");
        // alignment off keeps the timing
        let (z, _) = tune(&x, SR, &notes, &TuneOpts { strength: 0.0, align: false, ..TuneOpts::default() });
        assert!((onset_of(&z) - before).abs() < 0.01);
    }

    #[test]
    fn vibrato_is_added_to_long_notes_only() {
        let x = voice(&[(0.1, 2.0, 220.0), (2.1, 2.4, 247.0)], 2.6, 0.0);
        let notes = vec![VNote { on: 0.1, off: 2.0, hz: 220.0 }, VNote { on: 2.1, off: 2.4, hz: 247.0 }];
        let spread = |y: &[f32], a: f64, b: f64| {
            let p = track_pitch(&mono_of(y), SR);
            let v: Vec<f64> = p.f0.iter().enumerate().filter(|(j, f)| **f > 0.0 && (*j as f64 * p.hop) >= a && (*j as f64 * p.hop) <= b).map(|(_, f)| *f as f64).collect();
            let (lo, hi) = v.iter().fold((f64::MAX, 0.0f64), |m, x| (m.0.min(*x), m.1.max(*x)));
            cents(hi, lo)
        };
        let (plain, _) = tune(&x, SR, &notes, &TuneOpts::default());
        let (vib, _) = tune(&x, SR, &notes, &TuneOpts { vibrato: true, vib_depth_cents: 40.0, ..TuneOpts::default() });
        assert!(spread(&vib, 0.9, 1.8) > spread(&plain, 0.9, 1.8) + 30.0, "{} {}", spread(&vib, 0.9, 1.8), spread(&plain, 0.9, 1.8));
        assert!(spread(&vib, 2.15, 2.35) < 30.0, "a short note has no vibrato");
    }

    #[test]
    fn silence_and_nonsense_pass_through_safely() {
        let silent = vec![0.0f32; (SR as usize) * 2];
        let notes = vec![VNote { on: 0.1, off: 0.5, hz: 220.0 }];
        let (y, _) = tune(&silent, SR, &notes, &TuneOpts::default());
        assert!(y.iter().all(|v| *v == 0.0));
        let (z, _) = tune(&silent, SR, &[], &TuneOpts::default());
        assert_eq!(z.len(), silent.len());
        let short = vec![0.1f32; 100];
        let (s, _) = tune(&short, SR, &notes, &TuneOpts::default());
        assert_eq!(s.len(), 100);
    }

    #[test]
    fn a_flat_sharp_voice_is_pulled_onto_the_scale_with_eased_joins() {
        // C major: D, E, F#?, G with every note 35 cents sharp and one chromatic F# that should land on F or G
        let mut allowed = [false; 12];
        for pc in [0, 2, 4, 5, 7, 9, 11] {
            allowed[pc] = true;
        }
        let sharp = |k: f64| k + 0.35;
        let segs = [(0.1, 0.6, midi_hz(sharp(62.0))), (0.6, 1.1, midi_hz(sharp(64.0))), (1.1, 1.6, midi_hz(sharp(66.0))), (1.6, 2.2, midi_hz(sharp(67.0)))];
        let st = voice(&segs, 2.4, 0.0);
        let (out, rep) = tune_to_scale(&st, SR, &allowed, 1.0, 80.0, (0.0, 1.0));
        assert_eq!(out.len(), st.len());
        assert!(out.iter().all(|x| x.is_finite()));
        assert!(rep.notes >= 3, "the voice was cut into notes: {}", rep.notes);
        assert!(rep.cents_before > 20.0 && rep.cents_after < 15.0, "before {} after {}", rep.cents_before, rep.cents_after);
        // the result sits on scale notes: track it and look at every sustained frame
        let after = track_pitch(&mono_of(&out), SR);
        let mut off = vec![];
        for f in after.f0.iter().filter(|f| **f > 0.0) {
            let mm = 69.0 + 12.0 * (*f as f64 / 440.0).log2();
            let t = nearest_allowed(mm, &allowed, None);
            off.push(((t as f64 - mm) * 100.0).abs());
        }
        off.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!(off[off.len() / 2] < 12.0, "median error {} cents", off[off.len() / 2]);
        // strength 0 changes nothing audible
        let (same, _) = tune_to_scale(&st, SR, &allowed, 0.0, 80.0, (0.0, 1.0));
        let med = |x: &[f32]| {
            let mut v: Vec<f32> = track_pitch(&mono_of(x), SR).f0.into_iter().filter(|f| *f > 0.0).collect();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            v[v.len() / 2] as f64
        };
        assert!(cents(med(&same), med(&st)).abs() < 8.0, "strength 0 should leave the pitch alone");
    }

    #[test]
    fn the_bezier_ease_runs_from_zero_to_one_and_the_shapes_differ() {
        assert!((bezier_ease(0.0, 0.0, 1.0)).abs() < 1e-9 && (bezier_ease(1.0, 0.0, 1.0) - 1.0).abs() < 1e-9);
        assert!((bezier_ease(0.5, 0.0, 1.0) - 0.5).abs() < 1e-9, "the smooth curve is centred");
        assert!(bezier_ease(0.2, 0.9, 1.0) > bezier_ease(0.2, 0.0, 1.0), "a snappy curve is ahead early");
        assert!(bezier_ease(0.5, 0.0, 0.4) < 0.5, "a lazy curve is behind");
        let mut last = -1.0;
        for i in 0..=20 {
            let e = bezier_ease(i as f64 / 20.0, 0.0, 1.0);
            assert!(e >= last - 1e-12);
            last = e;
        }
    }

    #[test]
    fn nearest_allowed_prefers_the_last_note_in_a_slide() {
        let mut allowed = [false; 12];
        for pc in [0, 2, 4, 5, 7, 9, 11] {
            allowed[pc] = true;
        }
        assert_eq!(nearest_allowed(66.2, &allowed, None), 67);
        assert_eq!(nearest_allowed(65.9, &allowed, None), 65);
        // 66.0 is equidistant from F and G: coming from F it stays
        assert_eq!(nearest_allowed(66.05, &allowed, Some(65)), 65);
        assert_eq!(nearest_allowed(60.0, &allowed, Some(61)), 60);
    }

    #[test]
    fn a_solo_voice_is_steady_and_noise_is_not() {
        let st = voice(&[(0.1, 1.0, 220.0)], 1.2, 0.0);
        let (voiced, steady) = monophonic_score(&st, SR);
        assert!(voiced > 0.5 && steady > 0.8, "{voiced} {steady}");
        let mut r = 12345u32;
        let noise: Vec<f32> = (0..(SR as usize)).flat_map(|_| { r = r.wrapping_mul(1664525).wrapping_add(1013904223); let x = (r >> 8) as f32 / 8388608.0 - 1.0; [x * 0.3, x * 0.3] }).collect();
        let (_, steady_n) = monophonic_score(&noise, SR);
        assert!(steady_n < 0.6 || monophonic_score(&noise, SR).0 < 0.3, "noise looks like a voice: {steady_n}");
    }
}
