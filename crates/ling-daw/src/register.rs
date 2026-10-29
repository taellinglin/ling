//! Register: where a part sits on the keyboard, whether that range sounds good on its sound, and how layered parts share the octaves.
//!
//! * [`analyze`] - a part's lowest / highest / median pitch, its duration-weighted centre, how much of it sits in each octave, and how
//!   busy it is (notes per beat, average length).
//! * [`limits`] / [`guard`] - the range a voice sounds good in. A bass whose median sits too low for its patch is a fart (no
//!   fundamental a speaker can play, just the buzz of its harmonics): it goes up whole octaves, its shape kept. A sub sine can sit
//!   lower than a growl or a saw bass (all harmonics, they turn to mud first); a busy line (eighths and faster) needs to sit higher
//!   than a slow one, because fast low notes smear into each other. The top end the same way: a lead or arp too high for its patch
//!   (shrill, above about C7, lower for a bright saw) comes down.
//! * [`allocate`] - layered parts get octaves of their own: every part may move by whole octaves only (never a key change), the moves
//!   with the least total movement win, the bass stays lowest, no two parts share an octave as their centre (a pad may sit wide behind
//!   another part; with more parts than room the least important part is thinned out). Drums are not given to it.
//! * [`clash`] - the share of two parts' overlap on harsh intervals (minor seconds, tritones, major sevenths), by pitch class.
use crate::model::Note;

/// what a part does, for its register
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub enum Voice {
    Bass,
    Pad,
    Chords,
    Lead,
    Arp,
}
impl Voice {
    pub fn name(self) -> &'static str {
        match self {
            Voice::Bass => "bass",
            Voice::Pad => "pad",
            Voice::Chords => "chords",
            Voice::Lead => "lead",
            Voice::Arp => "arp",
        }
    }
    /// where its centre sits most comfortably (MIDI), low .. high
    pub fn home(self) -> (f64, f64) {
        match self {
            Voice::Bass => (33.0, 48.0),
            Voice::Pad => (50.0, 70.0),
            Voice::Chords => (52.0, 72.0),
            Voice::Lead => (60.0, 82.0),
            Voice::Arp => (64.0, 90.0),
        }
    }
    /// the order from the bottom of the mix (bass 0 .. arp 3)
    pub fn rank(self) -> i32 {
        match self {
            Voice::Bass => 0,
            Voice::Pad | Voice::Chords => 1,
            Voice::Lead => 2,
            Voice::Arp => 3,
        }
    }
}

/// a part's place on the keyboard
#[derive(Clone, Debug, PartialEq)]
pub struct Reg {
    pub lo: u8,
    pub hi: u8,
    pub median: f64,
    /// the duration-weighted mean pitch
    pub centre: f64,
    /// the share of note duration in each octave (MIDI 0-11, 12-23 ... 120-127)
    pub octaves: [f32; 11],
    /// notes per beat
    pub rate: f64,
    /// average note length in beats
    pub avg_len: f64,
    pub n: usize,
}

impl Reg {
    /// the same part moved by `oct` octaves
    pub fn moved(&self, oct: i32) -> Reg {
        let d = oct * 12;
        let mut o = [0f32; 11];
        for (i, v) in self.octaves.iter().enumerate() {
            let j = i as i32 + oct;
            if (0..11).contains(&j) {
                o[j as usize] += v;
            }
        }
        Reg {
            lo: (self.lo as i32 + d).clamp(0, 127) as u8,
            hi: (self.hi as i32 + d).clamp(0, 127) as u8,
            median: self.median + d as f64,
            centre: self.centre + d as f64,
            octaves: o,
            ..self.clone()
        }
    }
    /// 0 (long notes, slow) .. 1 (sixteenths): fast low notes smear into each other
    pub fn busy(&self) -> f64 {
        let r = ((self.rate - 1.5) / 2.5).clamp(0.0, 1.0);
        let l = ((0.75 - self.avg_len) / 0.5).clamp(0.0, 1.0);
        (r * 0.7 + l * 0.3).clamp(0.0, 1.0)
    }
}

/// a part's register over `beats` (None without notes)
pub fn analyze(notes: &[Note], beats: f64) -> Option<Reg> {
    if notes.is_empty() {
        return None;
    }
    let mut keys: Vec<u8> = notes.iter().map(|n| n.key).collect();
    keys.sort();
    let median = if keys.len() % 2 == 1 { keys[keys.len() / 2] as f64 } else { (keys[keys.len() / 2 - 1] as f64 + keys[keys.len() / 2] as f64) / 2.0 };
    let mut o = [0f32; 11];
    let (mut wsum, mut w) = (0.0, 0.0);
    for n in notes {
        let d = n.len.max(0.05);
        wsum += n.key as f64 * d;
        w += d;
        o[(n.key / 12).min(10) as usize] += d as f32;
    }
    for v in o.iter_mut() {
        *v /= w.max(1e-9) as f32;
    }
    // notes per beat counts onsets (a chord is one event)
    let mut onsets: Vec<i64> = notes.iter().map(|n| (n.start * 48.0).round() as i64).collect();
    onsets.sort();
    onsets.dedup();
    Some(Reg {
        lo: keys[0],
        hi: *keys.last().unwrap(),
        median,
        centre: wsum / w.max(1e-9),
        octaves: o,
        rate: onsets.len() as f64 / beats.max(1.0),
        avg_len: w / notes.len() as f64,
        n: notes.len(),
    })
}

/// How bright a sound is from its role or patch name (0 a sine .. 1 a saw / growl): bright sounds turn to mud higher up in the bass
/// and get shrill lower down at the top.
pub fn brightness(name: &str) -> f32 {
    let r = name.to_lowercase();
    let has = |w: &[&str]| w.iter().any(|x| r.contains(x));
    if has(&["sub", "sine", "808"]) {
        0.1
    } else if has(&["growl", "wobble", "reese", "saw", "screech", "laser", "tearout", "riddim", "fm ", "neuro", "distort"]) {
        0.9
    } else if has(&["square", "chip", "bell", "brass", "organ", "stab"]) {
        0.7
    } else if has(&["rolling", "pluck", "keys", "piano", "lead", "arp"]) {
        0.5
    } else if has(&["pad", "string", "warm", "ambient", "choir", "flute"]) {
        0.3
    } else {
        0.5
    }
}

/// the range a part sounds good in: its median between `floor` and `ceil`, no note under `lo_min` or over `hi_max`
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    pub floor: f64,
    pub ceil: f64,
    pub lo_min: f64,
    pub hi_max: f64,
}

/// The limits of a voice on a sound of `bright`ness (0..1) playing `busy` (0..1, see [`Reg::busy`]).
/// Bass: the median at least D1 (26) on a slow sub sine, about G1 / A1 (31..33) on a growl or saw, up to 5 semitones more when it is
/// busy (16ths on a growl must sit around C#2 .. D2), no note more than a fifth below that; at most G3 (55) or it stops being a bass.
/// Pads and chords: the median at least A2 (45), at most C6 (84) less 6 for a bright sound. Leads: at least D3 (50), at most C7 (96)
/// less 10 for a bright saw (D6, 86), no note over A7 (105) less 8. Arps: like leads, 4 higher.
pub fn limits(v: Voice, bright: f32, busy: f64) -> Limits {
    let b = bright.clamp(0.0, 1.0) as f64;
    let busy = busy.clamp(0.0, 1.0);
    match v {
        Voice::Bass => {
            let floor = 26.0 + 7.0 * b + 5.0 * busy;
            Limits { floor, ceil: 55.0, lo_min: floor - 7.0, hi_max: 74.0 }
        }
        Voice::Pad | Voice::Chords => Limits { floor: 45.0 + 2.0 * busy, ceil: 84.0 - 6.0 * b, lo_min: 33.0, hi_max: 100.0 - 6.0 * b },
        Voice::Lead => Limits { floor: 50.0, ceil: 96.0 - 10.0 * b, lo_min: 38.0, hi_max: 105.0 - 8.0 * b },
        Voice::Arp => Limits { floor: 52.0, ceil: 100.0 - 10.0 * b, lo_min: 40.0, hi_max: 108.0 - 8.0 * b },
    }
}

/// how far (in semitones) a register is outside its limits; the median counts twice as much as the extremes
pub fn violation(r: &Reg, l: &Limits) -> f64 {
    let m = (l.floor - r.median).max(0.0) + (r.median - l.ceil).max(0.0);
    let e = (l.lo_min - r.lo as f64).max(0.0) + (r.hi as f64 - l.hi_max).max(0.0);
    2.0 * m + e
}

/// why a part moved
pub fn reason(v: Voice, oct: i32) -> &'static str {
    match (v, oct.signum()) {
        (Voice::Bass, 1) => "was muddy",
        (Voice::Bass, -1) => "too high for a bass",
        (_, -1) => "was shrill",
        (Voice::Pad | Voice::Chords, 1) => "was boomy",
        (_, 1) => "sat too low",
        _ => "",
    }
}

/// The smallest whole-octave move (-4..4) that puts a part inside its limits (within 3 semitones of the best any move can do: a line a
/// semitone over a soft limit is not moved a whole octave for it). 0 when it fits.
pub fn guard(r: &Reg, l: &Limits) -> i32 {
    let order = [0, 1, -1, 2, -2, 3, -3, 4, -4];
    let viol: Vec<Option<f64>> = order
        .iter()
        .map(|k| {
            let m = r.moved(*k);
            // off the keyboard
            if r.lo as i32 + 12 * k < 0 || r.hi as i32 + 12 * k > 127 {
                return None;
            }
            Some(violation(&m, l))
        })
        .collect();
    let min = viol.iter().flatten().cloned().fold(f64::MAX, f64::min);
    order.iter().zip(&viol).find(|(_, v)| v.map(|v| v <= min + 3.0).unwrap_or(false)).map(|(k, _)| *k).unwrap_or(0)
}

/// one part to place in a layered section
#[derive(Clone, Debug)]
pub struct Placing {
    pub reg: Reg,
    pub voice: Voice,
    pub limits: Limits,
    /// the cost of moving it an octave (the hook more than a layer)
    pub weight: f64,
    /// never thinned out
    pub keep: bool,
}

/// what [`allocate`] decided for each part
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub octave: i32,
    /// shares an octave with another part, kept quieter and wide behind it (a pad)
    pub behind: bool,
    /// left out: more parts than octaves
    pub thinned: bool,
}

fn overlap(a: f64, b: f64) -> bool {
    // the same octave band, or centres closer than a major sixth
    (a / 12.0).floor() == (b / 12.0).floor() || (a - b).abs() < 9.0
}

/// Give every part an octave of its own (see the module notes). Every part tries the octaves within two of its guarded position that
/// do not sound worse than that; the assignment with the least cost wins: 1 per octave moved (x weight), 6 per pair of parts sharing an
/// octave (2.5 when one is a pad, which then sits behind), 8 when the bass is not clearly the lowest, 1 per pair out of the usual
/// order (pads / chords under the lead under the arp), 0.1 per semitone a centre is outside its voice's home.
pub fn allocate(parts: &[Placing]) -> Vec<Placed> {
    let n = parts.len();
    if n == 0 {
        return vec![];
    }
    let base: Vec<i32> = parts.iter().map(|p| guard(&p.reg, &p.limits)).collect();
    let cands: Vec<Vec<i32>> = parts
        .iter()
        .zip(&base)
        .map(|(p, b)| {
            let v0 = violation(&p.reg.moved(*b), &p.limits);
            let mut c: Vec<i32> = (-2..=2).map(|d| b + d).filter(|k| violation(&p.reg.moved(*k), &p.limits) <= v0 + 1e-9).filter(|k| (p.reg.lo as i32 + 12 * k) >= 0 && (p.reg.hi as i32 + 12 * k) <= 127).collect();
            if c.is_empty() {
                c.push(*b);
            }
            c.sort_by_key(|k| (k - b).abs());
            c
        })
        .collect();
    let cost_of = |ks: &[i32]| -> f64 {
        let mut c = 0.0;
        let cen: Vec<f64> = (0..ks.len()).map(|i| parts[i].reg.centre + 12.0 * ks[i] as f64).collect();
        for i in 0..ks.len() {
            let p = &parts[i];
            c += ks[i].abs() as f64 * p.weight;
            let (h0, h1) = p.voice.home();
            c += 0.1 * ((h0 - cen[i]).max(0.0) + (cen[i] - h1).max(0.0));
            for j in 0..i {
                let q = &parts[j];
                if overlap(cen[i], cen[j]) {
                    c += if p.voice == Voice::Pad || q.voice == Voice::Pad { 2.5 } else { 6.0 };
                }
                let (lo, hi) = if p.voice.rank() < q.voice.rank() { (i, j) } else { (j, i) };
                if parts[lo].voice.rank() != parts[hi].voice.rank() {
                    if parts[lo].voice == Voice::Bass && cen[hi] < cen[lo] + 7.0 {
                        c += 8.0;
                    } else if cen[hi] < cen[lo] {
                        c += 1.0;
                    }
                }
            }
        }
        c
    };
    // exhaustive over the candidates (at most 5 each); a greedy fallback for many parts
    let mut best_k = base.clone();
    let mut best_c = cost_of(&base);
    let combos: usize = cands.iter().map(|c| c.len()).product();
    if combos <= 200_000 {
        let mut idx = vec![0usize; n];
        loop {
            let ks: Vec<i32> = (0..n).map(|i| cands[i][idx[i]]).collect();
            let c = cost_of(&ks);
            if c + 1e-9 < best_c {
                best_c = c;
                best_k = ks;
            }
            let mut d = 0;
            while d < n {
                idx[d] += 1;
                if idx[d] < cands[d].len() {
                    break;
                }
                idx[d] = 0;
                d += 1;
            }
            if d == n {
                break;
            }
        }
    } else {
        for _ in 0..3 {
            for i in 0..n {
                for k in &cands[i] {
                    let mut t = best_k.clone();
                    t[i] = *k;
                    let c = cost_of(&t);
                    if c + 1e-9 < best_c {
                        best_c = c;
                        best_k = t;
                    }
                }
            }
        }
    }
    let cen: Vec<f64> = (0..n).map(|i| parts[i].reg.centre + 12.0 * best_k[i] as f64).collect();
    let mut out: Vec<Placed> = best_k.iter().map(|k| Placed { octave: *k, behind: false, thinned: false }).collect();
    for i in 0..n {
        for j in 0..i {
            if out[i].thinned || out[j].thinned || !overlap(cen[i], cen[j]) {
                continue;
            }
            // a pad gives way by sitting behind; else the less important part (not kept, later in the list) is thinned out
            if parts[i].voice == Voice::Pad {
                out[i].behind = true;
            } else if parts[j].voice == Voice::Pad {
                out[j].behind = true;
            } else if !parts[i].keep {
                out[i].thinned = true;
            } else if !parts[j].keep {
                out[j].thinned = true;
            }
        }
    }
    out
}

/// harsh intervals by pitch class: a minor second, a tritone, a major seventh
pub fn harsh(a: u8, b: u8) -> bool {
    matches!((a as i32 - b as i32).rem_euclid(12), 1 | 6 | 11)
}

/// the share of the overlap of `notes` with `against` that is harsh (0 when they never overlap)
pub fn clash(notes: &[Note], against: &[Note]) -> f64 {
    let (mut bad, mut over) = (0.0, 0.0);
    for n in notes {
        for a in against {
            let o = (n.start + n.len).min(a.start + a.len) - n.start.max(a.start);
            if o > 0.0 {
                over += o;
                if harsh(n.key, a.key) {
                    bad += o;
                }
            }
        }
    }
    if over > 0.0 {
        bad / over
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(keys: &[u8], step: f64) -> Vec<Note> {
        keys.iter().enumerate().map(|(i, k)| Note::new(i as f64 * step, step * 0.9, *k, 0.8)).collect()
    }

    #[test]
    fn a_muddy_bass_goes_up_a_fine_sub_stays_a_shrill_lead_comes_down() {
        // a busy growl line around E1 (28): mud - up an octave, the shape kept
        let growl = line(&[28, 28, 31, 28, 33, 28, 26, 28, 28, 31, 28, 35, 28, 26, 28, 28], 0.25);
        let r = analyze(&growl, 4.0).unwrap();
        let k = guard(&r, &limits(Voice::Bass, brightness("growl bass"), r.busy()));
        assert_eq!(k, 1, "{r:?}");
        assert_eq!(reason(Voice::Bass, k), "was muddy");
        // a slow sub sine on F1 / G1 (29 / 31) is fine where it is
        let sub = line(&[29, 29, 31, 29], 4.0);
        let r = analyze(&sub, 16.0).unwrap();
        assert_eq!(guard(&r, &limits(Voice::Bass, brightness("sub bass"), r.busy())), 0);
        // the same slow line on a growl sits too low
        assert_eq!(guard(&r, &limits(Voice::Bass, brightness("growl bass"), r.busy())), 1);
        // a bass two octaves down (C0 .. ) comes up two
        let deep = line(&[12, 14, 15, 12, 19, 12], 1.0);
        let r = analyze(&deep, 6.0).unwrap();
        assert_eq!(guard(&r, &limits(Voice::Bass, 0.5, r.busy())), 2);
        // a supersaw lead around D7 comes down an octave, a soft lead at A6 stays
        let shrill = line(&[98, 100, 102, 98, 103, 100], 0.5);
        let r = analyze(&shrill, 3.0).unwrap();
        let k = guard(&r, &limits(Voice::Lead, brightness("supersaw lead"), r.busy()));
        assert_eq!(k, -1);
        assert_eq!(reason(Voice::Lead, k), "was shrill");
        let soft = line(&[88, 91, 93, 91], 1.0);
        let r = analyze(&soft, 4.0).unwrap();
        assert_eq!(guard(&r, &limits(Voice::Lead, brightness("flute"), r.busy())), 0);
        // the move keeps the shape: every interval the same
        let m = r.moved(-1);
        assert_eq!((m.hi - m.lo, m.median), (r.hi - r.lo, r.median - 12.0));
    }

    #[test]
    fn layered_parts_get_octaves_of_their_own_with_the_least_movement() {
        // bass, pad, lead and arp all piled up around middle C
        let mk = |keys: &[u8], step: f64, v: Voice, bright: f32, keep: bool| {
            let notes = line(keys, step);
            let reg = analyze(&notes, 8.0).unwrap();
            let busy = reg.busy();
            Placing { reg, voice: v, limits: limits(v, bright, busy), weight: if keep { 1.5 } else { 1.0 }, keep }
        };
        let parts = vec![
            mk(&[62, 65, 69, 67, 64, 62], 1.0, Voice::Lead, 0.5, true),
            mk(&[48, 48, 43, 45], 2.0, Voice::Bass, 0.5, false),
            mk(&[60, 64, 67, 60], 2.0, Voice::Pad, 0.3, false),
            mk(&[60, 64, 67, 72, 67, 64, 60, 64, 67, 72, 67, 64, 60, 64, 67, 72], 0.5, Voice::Arp, 0.6, false),
        ];
        let got = allocate(&parts);
        let cen: Vec<f64> = parts.iter().zip(&got).map(|(p, g)| p.reg.centre + 12.0 * g.octave as f64).collect();
        assert!(got.iter().all(|g| !g.thinned), "{got:?}");
        for i in 0..cen.len() {
            for j in 0..i {
                let shared = (cen[i] / 12.0).floor() == (cen[j] / 12.0).floor();
                assert!(!shared || got[i].behind || got[j].behind, "{i} and {j} share an octave: {cen:?} {got:?}");
            }
        }
        assert!((0..4).filter(|i| *i != 1).all(|i| cen[i] >= cen[1] + 7.0), "the bass is clearly lowest: {cen:?}");
        assert!(cen[3] > cen[0], "the arp sparkles over the lead: {cen:?}");
        let moved: i32 = got.iter().map(|g| g.octave.abs()).sum();
        assert!(moved <= 3, "as little movement as it takes: {got:?}");
        // the hook (kept, weighted) stays where it is
        assert_eq!(got[0].octave, 0);
        // already spread out: nothing moves
        let spread = vec![
            mk(&[40, 43, 45, 40], 1.0, Voice::Bass, 0.5, false),
            mk(&[55, 59, 62, 55], 2.0, Voice::Pad, 0.3, false),
            mk(&[72, 74, 76, 79], 1.0, Voice::Lead, 0.5, true),
        ];
        assert!(allocate(&spread).iter().all(|g| g.octave == 0 && !g.thinned && !g.behind));
        // two leads in the same place with no room: the one not kept is thinned or moved, never both on one octave
        let crowd = vec![
            mk(&[70, 72, 74, 72], 1.0, Voice::Lead, 0.5, true),
            mk(&[71, 72, 74, 76], 1.0, Voice::Lead, 0.5, false),
        ];
        let g = allocate(&crowd);
        let c0 = crowd[0].reg.centre + 12.0 * g[0].octave as f64;
        let c1 = crowd[1].reg.centre + 12.0 * g[1].octave as f64;
        assert!(g[1].thinned || (c0 - c1).abs() >= 9.0, "{g:?}");
    }

    #[test]
    fn harsh_intervals_are_counted() {
        let c = vec![Note::new(0.0, 4.0, 60, 0.7)];
        assert_eq!(clash(&line(&[61, 66, 71, 73], 1.0), &c), 1.0);
        assert_eq!(clash(&line(&[64, 67, 72, 76], 1.0), &c), 0.0);
        assert_eq!(clash(&line(&[64], 1.0), &[]), 0.0);
    }
}
