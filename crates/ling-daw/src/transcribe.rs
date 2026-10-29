//! An acapella (a sung vocal on its own) read for a song: File > New AI Song's ACAPELLA row.
//!
//! 1. **Notes** ([`detect`]): the crate's YIN pitch tracker (`vocal::track_pitch`, 5 ms frames) cut into notes where the pitch is stable
//!    (`vocal::detect_notes`), split again at the vocal's onsets (`chops::transients`: a new syllable on the same pitch), each onset
//!    pulled back to the transient that starts it; the recording's tuning (the median sub-semitone offset) is taken out before the
//!    pitches are rounded; velocity from the note's level.
//! 2. **Tempo and grid** ([`estimate_bpm`], [`grid_origin`]): the onsets' autocorrelation over beat, half-beat and bar lags (with a mild
//!    preference for 80..140 bpm), refined by the eighth-grid coherence; the beat phase from that coherence, the on-beat half chosen by
//!    where the long notes sit, the downbeat where phrases start. Or the tempo given (the plan's / the reference's).
//! 3. **Quantised notes** on the sixteenth grid, the key from their pitch classes.
//! 4. **Harmony** ([`fit_chords`]): a chord per bar (or half bar) by a Viterbi pass over the key's triads (and the major V of a minor
//!    key): notes that are chord tones score (strong-beat and long ones most, the root a little more), notes in the key but outside the
//!    chord cost a little, notes outside the key more; root motion by a fourth / fifth is preferred, needless changes cost; the voicing
//!    is voice-led ([`voiced`]).
//! 5. **Structure and story**: the melody's own repetition and intensity (songanalysis.rs) - the most repeated loud material is the
//!    chorus, rests of the singer are the band's sections (a drop after a chorus, a break) - with an intro before the first line and an
//!    outro after the last.
use crate::compose::Kind;
use crate::songanalysis::{self, PartIn, Shape};
use crate::songgen::Section;
use crate::theory::{self, Key, N};

/// a sung note in seconds of the recording
#[derive(Clone, Debug)]
pub struct Sung {
    pub on: f64,
    pub off: f64,
    pub midi: f64,
    pub vel: f32,
}

/// a chord over a span of beats
#[derive(Clone, Debug, PartialEq)]
pub struct ChordSeg {
    pub start: f64,
    pub len: f64,
    pub root: u8,
    pub minor: bool,
    /// degree of the key 0..6, 7 = the major V of a minor key
    pub degree: usize,
    pub name: String,
    pub pcs: [u8; 3],
}

#[derive(Clone, Debug)]
pub struct Acapella {
    pub seconds: f64,
    pub bpm: f64,
    /// true when the tempo was found in the recording (false: given)
    pub bpm_found: bool,
    /// seconds of the recording where grid beat 0 is (a downbeat; can be negative when the singing starts at once)
    pub origin_s: f64,
    pub tuning_cents: f64,
    pub key: Key,
    pub key_conf: f64,
    /// the melody on the acapella's own grid (beat 0 = `origin_s`), in the key it is sung in
    pub notes: Vec<N>,
    pub chords: Vec<ChordSeg>,
    /// Roman numerals of the verse's and the chorus's loops ("" = none found)
    pub prog: String,
    pub chorus_prog: String,
    /// bars of intro the song plays before grid bar 0
    pub lead_in: u32,
    /// the song's sections (the lead-in and an outro included); every bar of the acapella lands where it is
    pub sections: Vec<Section>,
    pub labels: Vec<String>,
    pub shape: Shape,
    /// seconds of the first sung note
    pub first_s: f64,
}

/// Sung notes of a recording (interleaved stereo) and its tuning offset in cents.
pub fn detect(stereo: &[f32], sr: u32) -> (Vec<Sung>, f64) {
    let mono = crate::vocal::mono_of(stereo);
    let p = crate::vocal::track_pitch(&mono, sr as f32);
    let dn = crate::vocal::detect_notes(&p);
    let tr: Vec<f64> = crate::chops::transients(stereo, sr).into_iter().map(|(i, _)| i as f64 / sr as f64).collect();
    let midi_at = |j: usize| -> f64 { if p.f0.get(j).copied().unwrap_or(0.0) > 0.0 { 69.0 + 12.0 * (p.f0[j] as f64 / 440.0).log2() } else { f64::NAN } };
    let median_between = |a: f64, b: f64| -> Option<f64> {
        let (ja, jb) = ((a / p.hop) as usize, ((b / p.hop) as usize).min(p.f0.len()));
        // the middle of the span: scoops and falls at the edges do not count
        let span = jb.saturating_sub(ja);
        let (ja, jb) = (ja + span / 5, jb - span / 5);
        let mut v: Vec<f64> = (ja..jb).map(midi_at).filter(|m| m.is_finite()).collect();
        if v.len() < 3 {
            return None;
        }
        v.sort_by(f64::total_cmp);
        Some(v[v.len() / 2])
    };
    // the level every 5 ms: a syllable boundary has a dip before its onset
    let lv = crate::vocal::level_track(&mono, sr as f32, 0.005);
    let lv_at = |t: f64| lv.get((t / 0.005) as usize).copied().unwrap_or(0.0) as f64;
    let dips = |t: f64, on: f64, off: f64| -> bool {
        let mut body: Vec<f64> = (0..((off - on) / 0.005) as usize).map(|i| lv_at(on + i as f64 * 0.005)).collect();
        if body.is_empty() {
            return false;
        }
        body.sort_by(f64::total_cmp);
        let med = body[body.len() / 2].max(1e-9);
        let low = (0..12).map(|i| lv_at(t - 0.06 + i as f64 * 0.005)).fold(f64::MAX, f64::min);
        low < 0.55 * med
    };
    let mut out: Vec<Sung> = vec![];
    for d in dn {
        // a new syllable on the same pitch: split at the onsets inside the note
        // (only where the level dips first: a consonant, not a vibrato or a swell inside one vowel)
        let mut cuts: Vec<f64> = tr.iter().copied().filter(|t| *t > d.on + 0.09 && *t < d.off - 0.09 && dips(*t, d.on, d.off)).collect();
        cuts.dedup_by(|a, b| (*a - *b).abs() < 0.08);
        let mut edges = vec![d.on];
        edges.extend(cuts);
        edges.push(d.off);
        for w in edges.windows(2) {
            let (a, b) = (w[0], w[1]);
            if b - a < 0.06 {
                continue;
            }
            let m = median_between(a, b).unwrap_or(d.midi);
            out.push(Sung { on: a, off: b, midi: m, vel: 0.8 });
        }
    }
    // the tracker hears the voice a little late: pull each onset back to the transient that starts it
    for s in out.iter_mut() {
        if let Some(t) = tr.iter().copied().filter(|t| *t <= s.on + 0.02 && *t >= s.on - 0.09).max_by(f64::total_cmp) {
            s.on = t;
        }
    }
    // velocity from the level
    let lvl: Vec<f64> = out.iter().map(|s| {
        let (a, b) = (((s.on * sr as f64) as usize).min(mono.len()), ((s.off * sr as f64) as usize).min(mono.len()));
        if b <= a { 0.0 } else { ((a..b).map(|i| (mono[i] as f64).powi(2)).sum::<f64>() / (b - a) as f64).sqrt() }
    }).collect();
    let top = lvl.iter().cloned().fold(1e-9, f64::max);
    for (s, l) in out.iter_mut().zip(&lvl) {
        s.vel = (0.45 + 0.5 * (l / top)) as f32;
    }
    // the recording's tuning: the duration-weighted median of the offsets from the nearest semitone
    let mut offs: Vec<(f64, f64)> = out.iter().map(|s| (s.midi - s.midi.round(), s.off - s.on)).collect();
    offs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total: f64 = offs.iter().map(|o| o.1).sum();
    let mut acc = 0.0;
    let mut tuning = 0.0;
    for (o, w) in &offs {
        acc += w;
        if acc >= total * 0.5 {
            tuning = *o;
            break;
        }
    }
    for s in out.iter_mut() {
        s.midi -= tuning;
    }
    (out, tuning * 100.0)
}

/// onset weights for the tempo search: longer and louder notes count more
fn weights(sung: &[Sung]) -> Vec<(f64, f64)> {
    sung.iter().map(|s| (s.on, (0.5 + (s.off - s.on).min(1.0)) * s.vel as f64)).collect()
}

/// coherence of onsets with a grid of period `p` seconds: (magnitude 0..1, phase in seconds 0..p)
fn coherence(on: &[(f64, f64)], p: f64) -> (f64, f64) {
    let (mut re, mut im, mut w) = (0.0, 0.0, 0.0);
    for (t, wt) in on {
        let a = std::f64::consts::TAU * t / p;
        re += wt * a.cos();
        im += wt * a.sin();
        w += wt;
    }
    if w <= 0.0 {
        return (0.0, 0.0);
    }
    let ph = im.atan2(re).rem_euclid(std::f64::consts::TAU);
    ((re * re + im * im).sqrt() / w, ph / std::f64::consts::TAU * p)
}

/// The tempo of sung onsets (70..180 bpm): autocorrelation of an onset train (10 ms steps, smoothed) over the beat, half-beat and
/// bar lags with a mild preference for 80..140, then refined by the eighth-grid coherence within 3 %.
pub fn estimate_bpm(sung: &[Sung]) -> f64 {
    let on = weights(sung);
    if on.len() < 6 {
        return 120.0;
    }
    let end = on.iter().map(|o| o.0).fold(0.0, f64::max) + 1.0;
    let n = (end * 100.0) as usize + 1;
    let mut x = vec![0.0f64; n];
    for (t, w) in &on {
        let c = (t * 100.0).round() as i64;
        for d in -3i64..=3 {
            let i = c + d;
            if i >= 0 && (i as usize) < n {
                x[i as usize] += w * (-(d * d) as f64 / 4.0).exp();
            }
        }
    }
    let ac = |lag: f64| -> f64 {
        let l = lag.round() as usize;
        if l == 0 || l >= n {
            return 0.0;
        }
        (0..n - l).map(|i| x[i] * x[i + l]).sum::<f64>() / (n - l) as f64
    };
    let mut best = (120.0, f64::MIN);
    let mut b = 70.0;
    while b <= 180.0 {
        let beat = 6000.0 / b; // in 10 ms steps
        let s = ac(beat) + 0.5 * ac(beat * 2.0) + 0.35 * ac(beat * 4.0) + 0.25 * ac(beat * 0.5);
        let prior = (-((b / 110.0f64).log2().powi(2)) / (2.0 * 0.45f64.powi(2))).exp();
        let s = s * (0.6 + 0.4 * prior);
        if s > best.1 {
            best = (b, s);
        }
        b += 0.5;
    }
    // refine on the eighth grid
    let mut fine = (best.0, f64::MIN);
    let mut b = best.0 * 0.97;
    while b <= best.0 * 1.03 {
        let (m, _) = coherence(&on, 30.0 / b);
        if m > fine.1 {
            fine = (b, m);
        }
        b += 0.05;
    }
    (fine.0 * 10.0).round() / 10.0
}

/// Seconds of a downbeat for `bpm`: the eighth-grid phase, the half that puts the long notes on the beats, the beat of the bar where
/// phrases start; returned as the last downbeat at or before the first note.
pub fn grid_origin(sung: &[Sung], bpm: f64) -> f64 {
    let on = weights(sung);
    if on.is_empty() {
        return 0.0;
    }
    let t = 60.0 / bpm;
    let (_, ph8) = coherence(&on, t * 0.5);
    let beat_score = |o: f64| -> f64 {
        sung.iter().map(|s| {
            let f = ((s.on - o) / t).rem_euclid(1.0);
            if !(0.15..=0.85).contains(&f) { (s.off - s.on).min(2.0) } else { 0.0 }
        }).sum()
    };
    let o1 = if beat_score(ph8) >= beat_score(ph8 + t * 0.5) { ph8 } else { ph8 + t * 0.5 };
    // phrase starts: a note after a rest of a beat or more (or the first one)
    let starts: Vec<f64> = sung.iter().enumerate().filter(|(i, s)| *i == 0 || s.on - sung[i - 1].off >= 0.9 * t).map(|(_, s)| s.on).collect();
    let mut best = (0usize, f64::MIN);
    for k in 0..4 {
        let o = o1 + k as f64 * t;
        let mut sc = 0.0;
        for p in &starts {
            let pos = ((p - o) / t).rem_euclid(4.0);
            // on the downbeat, or a pickup in the last beat before it
            if pos < 0.25 || pos > 3.85 {
                sc += 3.0;
            } else if pos >= 3.0 {
                sc += 1.5;
            } else if (pos - 2.0).abs() < 0.25 {
                sc += 0.5;
            }
        }
        for s in sung {
            let pos = ((s.on - o) / t).rem_euclid(4.0);
            if pos < 0.15 || pos > 3.9 {
                sc += 0.5 * (s.off - s.on).min(2.0);
            }
        }
        if sc > best.1 {
            best = (k, sc);
        }
    }
    let o = o1 + best.0 as f64 * t;
    let first = sung.iter().map(|s| s.on).fold(f64::INFINITY, f64::min);
    // the last downbeat at or before the first note (a pickup up to a beat early belongs to the bar after it ... no: to its own bar)
    o + ((first + 0.06 - o) / (4.0 * t)).floor() * 4.0 * t
}

/// Notes on the grid (sixteenths), overlaps trimmed
pub fn quantize(sung: &[Sung], bpm: f64, origin: f64) -> Vec<N> {
    let t = 60.0 / bpm;
    let q = |s: f64| (((s - origin) / t) * 4.0).round() / 4.0;
    let mut v: Vec<N> = sung.iter().map(|s| {
        let a = q(s.on).max(0.0);
        let b = q(s.off).max(a + 0.25);
        N { start: a, len: b - a, key: s.midi.round().clamp(0.0, 127.0) as u8, vel: s.vel }
    }).collect();
    v.sort_by(|a, b| a.start.total_cmp(&b.start).then(b.len.total_cmp(&a.len)));
    v.dedup_by(|b, a| (a.start - b.start).abs() < 1e-6);
    for i in 0..v.len().saturating_sub(1) {
        let nx = v[i + 1].start;
        if v[i].start + v[i].len > nx {
            v[i].len = (nx - v[i].start).max(0.25);
        }
    }
    v
}

fn chord_pcs(key: Key, degree: usize) -> [u8; 3] {
    let s = key.scale();
    if degree == 7 {
        let r = (key.tonic + 7) % 12;
        return [r, (r + 4) % 12, (r + 7) % 12];
    }
    [s[degree], s[(degree + 2) % 7], s[(degree + 4) % 7]]
}

fn chord_name(key: Key, degree: usize) -> String {
    if degree == 7 {
        return "V".into();
    }
    crate::songgen::roman(degree, key.minor).to_string()
}

/// Chords for a melody: one per `seg` beats (4 = a bar) by a Viterbi pass (see the module notes). `total` = beats to cover.
pub fn fit_chords(notes: &[N], key: Key, total: f64, seg: f64) -> Vec<ChordSeg> {
    let nseg = ((total / seg).ceil() as usize).max(1);
    let cands: Vec<usize> = if key.minor { (0..8).collect() } else { (0..7).collect() };
    let pcs: Vec<[u8; 3]> = cands.iter().map(|d| chord_pcs(key, *d)).collect();
    let is_minor = |d: usize| d != 7 && {
        let p = chord_pcs(key, d);
        (p[1] + 12 - p[0]) % 12 == 3
    };
    let dim = |d: usize| d != 7 && {
        let p = chord_pcs(key, d);
        (p[2] + 12 - p[0]) % 12 == 6
    };
    let prior = |d: usize| -> f64 {
        match d {
            0 => 0.15,
            3 | 4 => 0.1,
            5 => 0.08,
            7 => 0.02,
            _ if dim(d) => -0.2,
            _ => 0.0,
        }
    };
    // emission per segment and candidate
    let mut em = vec![vec![0.0f64; cands.len()]; nseg];
    let mut empty = vec![true; nseg];
    for (si, row) in em.iter_mut().enumerate() {
        let (a, z) = (si as f64 * seg, si as f64 * seg + seg);
        for n in notes.iter().filter(|n| n.start < z && n.start + n.len > a) {
            empty[si] = false;
            let ov = (n.start + n.len).min(z) - n.start.max(a);
            let beat = n.start.rem_euclid(4.0);
            let strong = if n.start >= a - 1e-6 && ((beat - 0.0).abs() < 0.1 || (beat - 2.0).abs() < 0.1) {
                1.6
            } else if n.start >= a - 1e-6 && (beat.fract() < 0.1) {
                1.15
            } else {
                1.0
            };
            let w = ov.min(2.0) * strong * (0.5 + 0.5 * n.vel as f64);
            let pc = n.key % 12;
            for (ci, p) in pcs.iter().enumerate() {
                row[ci] += if p.contains(&pc) {
                    w * if p[0] == pc { 1.25 } else { 1.0 }
                } else if key.accepts(pc) {
                    -0.35 * w
                } else {
                    -0.7 * w
                };
            }
        }
        for (ci, d) in cands.iter().enumerate() {
            row[ci] += prior(*d);
        }
    }
    let trans = |a: usize, b: usize| -> f64 {
        if a == b {
            return 0.0;
        }
        let (ra, rb) = (pcs[a][0], pcs[b][0]);
        let iv = (rb + 12 - ra) % 12;
        -0.06 + match iv {
            5 => 0.12,
            7 => 0.06,
            2 | 10 => 0.03,
            6 => -0.2,
            _ => 0.0,
        }
    };
    // Viterbi
    let k = cands.len();
    let mut score = vec![vec![f64::MIN; k]; nseg];
    let mut back = vec![vec![0usize; k]; nseg];
    for c in 0..k {
        score[0][c] = em[0][c] + if cands[c] == 0 { 0.2 } else { 0.0 };
    }
    for s in 1..nseg {
        for c in 0..k {
            let (bp, bv) = (0..k).map(|p| (p, score[s - 1][p] + trans(p, c) + if empty[s] && p == c { 0.05 } else { 0.0 })).max_by(|x, y| x.1.total_cmp(&y.1)).unwrap_or((0, 0.0));
            // a cadence home at the end of a line (the next segment is a rest or the end)
            let cad = if (s + 1 == nseg || empty[s + 1]) && cands[c] == 0 && matches!(cands[bp], 4 | 7) { 0.2 } else { 0.0 };
            score[s][c] = bv + em[s][c] + cad;
            back[s][c] = bp;
        }
    }
    let mut c = (0..k).max_by(|a, b| score[nseg - 1][*a].total_cmp(&score[nseg - 1][*b])).unwrap_or(0);
    let mut path = vec![0usize; nseg];
    for s in (0..nseg).rev() {
        path[s] = c;
        c = back[s][c];
    }
    path.iter().enumerate().map(|(s, ci)| {
        let d = cands[*ci];
        let p = pcs[*ci];
        ChordSeg { start: s as f64 * seg, len: seg, root: p[0], minor: is_minor(d), degree: d, name: chord_name(key, d), pcs: p }
    }).collect()
}

/// voice-led triads around C4 (55..72) and a bass root (36..47) per chord
pub fn voiced(chords: &[ChordSeg], vel: f32) -> (Vec<N>, Vec<N>) {
    let mut out = vec![];
    let mut bass = vec![];
    let mut last: Vec<i32> = vec![57, 60, 64];
    for c in chords {
        let mut best: (Vec<i32>, i32) = (vec![], i32::MAX);
        for inv in 0..3 {
            let mut v: Vec<i32> = (0..3).map(|i| c.pcs[(i + inv) % 3] as i32).collect();
            for i in 1..3 {
                while v[i] <= v[i - 1] {
                    v[i] += 12;
                }
            }
            for oct in [36, 48, 60, 72] {
                let w: Vec<i32> = v.iter().map(|x| x + oct).collect();
                if w[0] < 53 || w[2] > 76 {
                    continue;
                }
                let cost: i32 = w.iter().zip(&last).map(|(a, b)| (a - b).abs()).sum();
                if cost < best.1 {
                    best = (w, cost);
                }
            }
        }
        if best.0.is_empty() {
            continue;
        }
        for k in &best.0 {
            out.push(N { start: c.start, len: c.len, key: *k as u8, vel });
        }
        last = best.0;
        bass.push(N { start: c.start, len: c.len, key: 36 + ((c.root as i32 + 12 - 0) % 12) as u8, vel: vel.max(0.7) });
    }
    (out, bass)
}

/// the loop of a stretch of chords as Roman numerals: one per bar, repeats collapsed ("i VI III VII")
fn loop_of(chords: &[ChordSeg], from_bar: usize, bars: usize, seg: f64) -> String {
    let per_bar = (4.0 / seg).round().max(1.0) as usize;
    let mut v: Vec<&str> = vec![];
    for b in from_bar..from_bar + bars.min(8) {
        if let Some(c) = chords.get(b * per_bar) {
            v.push(&c.name);
        }
    }
    v.dedup();
    if let Some(p) = (2..v.len()).find(|p| (0..v.len()).all(|i| v[i] == v[i % p])) {
        v.truncate(p);
    }
    if v.len() >= 2 { v.join(" ") } else { String::new() }
}

/// Read an acapella (interleaved stereo). `bpm` = a tempo to use (0 = find it). None when nothing is sung.
pub fn analyze(stereo: &[f32], sr: u32, bpm: f64) -> Option<Acapella> {
    let (sung, tuning) = detect(stereo, sr);
    if sung.len() < 4 {
        return None;
    }
    let found = bpm <= 0.0;
    let bpm = if found { estimate_bpm(&sung) } else { bpm };
    let origin = grid_origin(&sung, bpm);
    Some(build(stereo.len() as f64 / 2.0 / sr as f64, &sung, tuning, bpm, found, origin))
}

/// the rest of [`analyze`], from the sung notes and the grid
pub fn build(seconds: f64, sung: &[Sung], tuning: f64, bpm: f64, found: bool, origin: f64) -> Acapella {
    let notes = quantize(sung, bpm, origin);
    let mut hist = [0.0f64; 12];
    for n in &notes {
        hist[(n.key % 12) as usize] += n.len * n.vel as f64;
    }
    let (key, key_conf, _) = theory::detect_key_hist(&hist);
    let end = notes.iter().map(|n| n.start + n.len).fold(0.0, f64::max);
    let bars = ((end / 4.0).ceil() as usize).max(1);
    let seg = 4.0;
    let chords = fit_chords(&notes, key, bars as f64 * 4.0, seg);
    // structure from the melody's own repetition and intensity
    let part = [PartIn { name: "Acapella", role: "voice", kind: Kind::Melody, notes: &notes }];
    let shape = songanalysis::analyze_notes(&part, bars, key);
    let (mut sections, mut labels) = if shape.sections.is_empty() {
        (vec![Section { tag: "verse".into(), bars: bars as u32, energy: 0.6 }], vec!["verse".to_string()])
    } else {
        (shape.sections.clone(), shape.labels.clone())
    };
    // the band's own sections where the singer rests
    let mut at = 0usize;
    for (s, l) in sections.iter_mut().zip(labels.iter()) {
        let silent = !notes.iter().any(|n| n.start >= at as f64 * 4.0 && n.start < (at + s.bars as usize) as f64 * 4.0);
        if silent {
            s.energy = match l.as_str() {
                "intro" => 0.35,
                "outro" => 0.3,
                "drop" => 0.95,
                _ => 0.4,
            };
        }
        at += s.bars as usize;
    }
    let lead_in = if labels.first().map(|l| l == "intro").unwrap_or(false) && notes.first().map(|n| n.start >= 8.0).unwrap_or(false) { 0 } else { 4 };
    // the verse's and the chorus's loops, read before the intro is put in front
    let start_of = |want: &[&str]| -> Option<(usize, usize)> {
        let mut at = 0usize;
        for (s, l) in sections.iter().zip(&labels) {
            if want.contains(&l.as_str()) {
                return Some((at, s.bars as usize));
            }
            at += s.bars as usize;
        }
        None
    };
    let prog = start_of(&["verse"]).or(Some((0, bars))).map(|(a, n)| loop_of(&chords, a, n, seg)).unwrap_or_default();
    let mut chorus_prog = start_of(&["chorus", "drop"]).map(|(a, n)| loop_of(&chords, a, n, seg)).unwrap_or_default();
    if chorus_prog == prog {
        chorus_prog.clear();
    }
    if lead_in > 0 {
        sections.insert(0, Section { tag: "intro".into(), bars: lead_in, energy: 0.35 });
        labels.insert(0, "intro".into());
    }
    if labels.last().map(|l| l != "outro").unwrap_or(true) {
        sections.push(Section { tag: "outro".into(), bars: 4, energy: 0.3 });
        labels.push("outro".into());
    }
    let first_s = sung.iter().map(|s| s.on).fold(f64::INFINITY, f64::min);
    Acapella { seconds, bpm, bpm_found: found, origin_s: origin, tuning_cents: tuning, key, key_conf, notes, chords, prog, chorus_prog, lead_in, sections, labels, shape, first_s }
}

impl Acapella {
    pub fn structure_text(&self) -> String {
        self.labels.iter().zip(&self.sections).map(|(l, s)| format!("{l} {}", s.bars)).collect::<Vec<_>>().join(", ")
    }

    /// one sentence for the dialog
    pub fn describe(&self) -> String {
        format!(
            "{:.0} bpm{}, {} ({:.0}% sure), {} notes{}, chords {}{}; {}",
            self.bpm,
            if self.bpm_found { " (heard)" } else { "" },
            self.key.name(),
            self.key_conf * 100.0,
            self.notes.len(),
            if self.tuning_cents.abs() >= 15.0 { format!(", sung {:+.0} cents off A440", self.tuning_cents) } else { String::new() },
            if self.prog.is_empty() { "free".to_string() } else { self.prog.clone() },
            if self.chorus_prog.is_empty() { String::new() } else { format!(" (chorus {})", self.chorus_prog) },
            self.structure_text()
        )
    }

    /// the melody, the fitted chords and a bass in song beats (the lead-in in front)
    pub fn song_parts(&self) -> (Vec<N>, Vec<N>, Vec<N>) {
        let off = self.lead_in as f64 * 4.0;
        let sh = |v: Vec<N>| v.into_iter().map(|n| N { start: n.start + off, ..n }).collect::<Vec<_>>();
        let (ch, bass) = voiced(&self.chords, 0.6);
        (sh(self.notes.clone()), sh(ch), sh(bass))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// a melody (beats, len, key): two 4-bar lines in A minor, the second repeated as a chorus higher up
    pub(crate) fn melody() -> Vec<(f64, f64, u8)> {
        let verse: [(f64, f64, u8); 12] = [
            (0.0, 1.0, 69), (1.0, 0.5, 72), (1.5, 0.5, 71), (2.0, 2.0, 69),
            (4.0, 1.0, 67), (5.0, 1.0, 69), (6.0, 2.0, 64),
            (8.0, 0.5, 69), (8.5, 0.5, 72), (9.0, 1.0, 76), (10.0, 2.0, 74), (12.0, 3.0, 69),
        ];
        let chorus: [(f64, f64, u8); 10] = [
            (0.0, 1.0, 76), (1.0, 1.0, 77), (2.0, 2.0, 76),
            (4.0, 0.5, 74), (4.5, 0.5, 72), (5.0, 1.0, 74), (6.0, 2.0, 72),
            (8.0, 2.0, 76), (10.0, 2.0, 79), (12.0, 3.0, 76),
        ];
        let mut v = vec![];
        for (base, part) in [(0.0, &verse[..]), (16.0, &chorus[..]), (32.0, &verse[..]), (48.0, &chorus[..])] {
            for (s, l, k) in part {
                v.push((base + s, *l, *k));
            }
        }
        v
    }

    fn synth_sung(mel: &[(f64, f64, u8)], bpm: f64, origin: f64, cents: f64) -> Vec<Sung> {
        let t = 60.0 / bpm;
        // the choruses (beats 16..32, 48..64) are sung louder
        mel.iter().map(|(s, l, k)| Sung { on: origin + s * t + 0.012, off: origin + (s + l * 0.9) * t, midi: *k as f64 + cents / 100.0, vel: if (*s / 16.0) as i32 % 2 == 1 { 0.95 } else { 0.7 } }).collect()
    }

    #[test]
    fn tempo_and_grid_from_sung_onsets() {
        let mel = melody();
        for bpm in [96.0, 120.0, 128.0] {
            let sung = synth_sung(&mel, bpm, 0.37, 0.0);
            let b = estimate_bpm(&sung);
            assert!((b / bpm - 1.0).abs() < 0.02, "{bpm}: found {b}");
            let o = grid_origin(&sung, bpm);
            let beat = 60.0 / bpm;
            assert!(((o - 0.37) / beat).abs() < 0.13, "{bpm}: origin {o}");
            let q = quantize(&sung, bpm, o);
            assert_eq!(q.len(), mel.len());
            for (n, m) in q.iter().zip(&mel) {
                assert!((n.start - m.0).abs() < 1e-6 && n.key == m.2, "{n:?} vs {m:?}");
            }
        }
    }

    #[test]
    fn chords_fit_the_melody_in_key() {
        let mel = melody();
        let notes: Vec<N> = mel.iter().map(|(s, l, k)| N { start: *s, len: *l, key: *k, vel: 0.8 }).collect();
        let key = Key::new(9, true);
        let ch = fit_chords(&notes, key, 64.0, 4.0);
        assert_eq!(ch.len(), 16);
        let mut strong = 0;
        let mut inside = 0;
        for n in &notes {
            let b = n.start.rem_euclid(4.0);
            if b.abs() < 1e-6 || (b - 2.0).abs() < 1e-6 {
                strong += 1;
                let c = &ch[(n.start / 4.0) as usize];
                if c.pcs.contains(&(n.key % 12)) {
                    inside += 1;
                }
            }
        }
        assert!(inside as f64 >= 0.75 * strong as f64, "{inside} of {strong}: {:?}", ch.iter().map(|c| c.name.clone()).collect::<Vec<_>>());
        for c in &ch {
            assert!(c.pcs.iter().all(|p| key.accepts(*p)), "{c:?}");
        }
        // the voicing moves little
        let (v, bass) = voiced(&ch, 0.6);
        assert_eq!(bass.len(), 16);
        let tops: Vec<i32> = v.chunks(3).map(|c| c.iter().map(|n| n.key as i32).sum::<i32>()).collect();
        for w in tops.windows(2) {
            assert!((w[1] - w[0]).abs() <= 3 * 5, "{tops:?}");
        }
    }

    #[test]
    fn an_acapella_gets_a_structure_and_loops() {
        let mel = melody();
        let sung = synth_sung(&mel, 120.0, 0.0, 18.0);
        let a = build(40.0, &sung, 18.0, 120.0, false, 0.0);
        assert_eq!(a.lead_in, 4);
        assert_eq!(a.labels.first().map(|s| s.as_str()), Some("intro"));
        assert_eq!(a.labels.last().map(|s| s.as_str()), Some("outro"));
        let total: u32 = a.sections.iter().map(|s| s.bars).sum();
        assert_eq!(total, 4 + 16 + 4, "{}", a.structure_text());
        assert!(a.labels.iter().any(|l| l == "chorus"), "{}", a.structure_text());
        assert!(!a.prog.is_empty(), "{}", a.describe());
        let (m, c, b) = a.song_parts();
        assert!((m[0].start - 16.0).abs() < 1e-6);
        assert!(!c.is_empty() && !b.is_empty());
        assert_eq!(a.structure_text(), "intro 4, verse 4, chorus 4, verse 4, chorus 4, outro 4");
        // the singer rests for 8 bars after a chorus: the band's drop
        let gap: Vec<(f64, f64, u8)> = mel.iter().map(|(st, l, k)| (if *st >= 32.0 { st + 32.0 } else { *st }, *l, *k)).collect();
        let a = build(60.0, &synth_sung(&gap, 120.0, 0.0, 0.0), 0.0, 120.0, false, 0.0);
        assert_eq!(a.structure_text(), "intro 4, verse 4, chorus 4, drop 8, verse 4, chorus 4, outro 4", "{:?} {:?}", a.shape.letters, a.shape.energy);
        assert!(a.sections[3].energy > 0.9);
    }
}
