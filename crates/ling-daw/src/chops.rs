//! Vocal chops for the mashup (Make Medley): a vocal cut at its transients, tuned and played as a composed part - and, in the
//! breakcore style, chopped breakbeats under them.
//!
//! * Source: a vocal file (any audio the decoder reads) or, with none, syllables sung by Ling Voice and rendered to a WAV.
//! * Cut ([`transients`], [`pick_chops`]): every onset of the vocal (5 ms RMS rising sharply over the gate, 60 ms apart) can start
//!   a chop. `count` of them are taken - at random (seeded: every Make draws others) or the strongest - and each one's length is the
//!   chosen length varied up to +-1.5 octaves of time by `vary`, its end moved to the quietest point near there so it does not cut
//!   a sound in half. Each chop's pitch is the median of its voiced frames (vocal::track_pitch).
//! * Play: one Sampler zone holds the whole vocal; a note's lyric "@start:end:cents" (sampler::parse_slice) makes it play that
//!   slice, the cents tuning the chop so the note's key is the pitch heard. Slices fade in (3 ms) and out (the last 25 ms) and the
//!   release is 60 ms: a chop never clicks off. Any number of chops, one track.
//! * Compose ([`compose`]): per section, two chops of the pool (others in every section) in a one-bar rhythm cell chosen to sit in
//!   the gaps - away from the kicks, where the lead rests (call and response) - and on the off-beats; its pitches are chord tones
//!   over the bass of the moment in the section's key, moving by a contour; the cell repeats with the harmony and answers itself
//!   every second bar; stutters (1/32, retriggered) close the phrases; an unpitched chop doubles the snares when it is busy. As the
//!   energy rises the cell gets denser and a second track layers it a third or an octave above. Quiet sections get a few long chops.
//! * Breaks ([`breaks`]): an amen-like bar, its four quarter-slices re-ordered every bar, ghost snares, 1/32 snare rolls into the
//!   section's end; written in double time when the tempo is under 110 bpm (breakcore runs at twice the music).
//! * Tune ([`tune`]): every pitched chop's pitch is measured (YIN, the steady voiced middle, with a confidence; unvoiced chops get
//!   none and play as recorded) and each chop note is a static transposition onto a chord tone of what sounds where it plays (else
//!   the key), by the smallest shift, never more than [`Tune::max_shift`] semitones (an intended note further away is played an
//!   octave nearer or on another chord tone).
use crate::compose::Rng;
use crate::dsp::sampler::{Config, Zone};
use crate::model::Note;
use crate::theory::Key;

mod tune;
pub use tune::{chop_f0, chord_at, Tune, TuneMode, TuneStats, F0};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Style {
    Off,
    Chops,
    Breakcore,
}
impl Style {
    pub fn parse(s: &str) -> Style {
        match s.trim().to_lowercase().as_str() {
            "chops" | "on" | "vocal chops" => Style::Chops,
            "breakcore" | "breakbeat" | "breaks" => Style::Breakcore,
            _ => Style::Off,
        }
    }
}

/// a cut of the vocal: frames in the source, its pitch (MIDI, fractional) when voiced, its loudness
#[derive(Clone, Debug, PartialEq)]
pub struct Chop {
    pub start: u32,
    pub end: u32,
    pub midi: Option<f32>,
    pub level: f32,
}

/// how the chops are taken
#[derive(Clone, Debug)]
pub struct Opts {
    /// how many chops
    pub count: usize,
    /// their length in seconds (the middle of the range)
    pub length: f64,
    /// 0 = all that long .. 1 = from a third to three times as long
    pub vary: f64,
    /// random transients (seeded) or the strongest
    pub random: bool,
}
impl Default for Opts {
    fn default() -> Opts {
        Opts { count: 8, length: 0.25, vary: 0.5, random: true }
    }
}

fn mono_env(stereo: &[f32], hop: usize) -> (Vec<f32>, Vec<f32>) {
    let mono: Vec<f32> = stereo.chunks_exact(2).map(|f| (f[0] + f[1]) * 0.5).collect();
    let env: Vec<f32> = mono.chunks(hop).map(|c| (c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).sqrt()).collect();
    (mono, env)
}

/// Every onset of the vocal: (frame, strength), in time order. An onset is where the 5 ms RMS jumps (over two hops) by more than a
/// quarter of its level and 3 % of the peak, at a local maximum of the jump, with sound after it; onsets are 60 ms apart at least.
pub fn transients(stereo: &[f32], rate: u32) -> Vec<(u32, f32)> {
    let hop = (rate as usize / 200).max(1);
    let (_, env) = mono_env(stereo, hop);
    let peak = env.iter().copied().fold(0.0f32, f32::max);
    if peak <= 1e-5 || env.len() < 8 {
        return vec![];
    }
    let gate = peak * 0.08;
    let flux: Vec<f32> = (0..env.len()).map(|i| if i < 2 { env[i] } else { (env[i] - env[i - 2]).max(0.0) }).collect();
    let mut out: Vec<(u32, f32)> = vec![];
    let mut last: Option<usize> = None;
    for i in 1..env.len() - 2 {
        let f = flux[i];
        if f < (0.03 * peak).max(0.25 * env[i]) || f < flux[i - 1] || f < flux[i + 1] || env[i + 2] < gate {
            continue;
        }
        if last.map(|l| i - l < 12).unwrap_or(false) {
            continue;
        }
        let strength = env[i..(i + 10).min(env.len())].iter().copied().fold(0.0f32, f32::max) / peak;
        out.push(((i.saturating_sub(1) * hop) as u32, strength));
        last = Some(i);
    }
    out
}

/// `o.count` chops starting at transients of the vocal, their lengths varied, their ends at quiet points; in time order
pub fn pick_chops(stereo: &[f32], rate: u32, o: &Opts, seed: u64) -> Vec<Chop> {
    let hop = (rate as usize / 200).max(1);
    let (mono, env) = mono_env(stereo, hop);
    let mut cands = transients(stereo, rate);
    if cands.is_empty() {
        return vec![];
    }
    let mut rng = Rng::new(seed ^ 0xC40B_5EED);
    if o.random {
        for i in (1..cands.len()).rev() {
            let j = (rng.next() % (i as u64 + 1)) as usize;
            cands.swap(i, j);
        }
    } else {
        cands.sort_by(|a, b| b.1.total_cmp(&a.1));
    }
    let frames = mono.len();
    let mut out: Vec<Chop> = vec![];
    for (start, _) in cands {
        if out.len() >= o.count.max(1) {
            break;
        }
        let u = (rng.next() % 2001) as f64 / 1000.0 - 1.0;
        let want = (o.length * 2f64.powf(o.vary.clamp(0.0, 1.0) * 1.5 * u)).clamp(0.05, 1.5);
        let s = start as usize;
        let (lo, hi) = (s + (want * 0.8 * rate as f64) as usize, s + (want * 1.25 * rate as f64) as usize);
        if lo >= frames {
            continue;
        }
        // the quietest 5 ms in the window is the end
        let (ha, hb) = (lo / hop, (hi.min(frames) / hop).min(env.len().saturating_sub(1)));
        let end_hop = (ha..=hb.max(ha)).filter(|h| *h < env.len()).min_by(|a, b| env[*a].total_cmp(&env[*b])).unwrap_or(ha);
        let e = ((end_hop + 1) * hop).min(frames);
        if e <= s + hop * 4 {
            continue;
        }
        let level = env[s / hop..(e / hop).min(env.len())].iter().sum::<f32>() / ((e - s) / hop).max(1) as f32;
        let midi = chop_f0(&mono, s, e, rate).map(|f| f.midi);
        let c = Chop { start: s as u32, end: e as u32, midi, level };
        if !out.iter().any(|x| x.start == c.start) {
            out.push(c);
        }
    }
    out.sort_by_key(|c| c.start);
    out
}

/// the Sampler config that plays the chops: the whole vocal on one zone at root 60 (the notes' slices pick the chops), a short
/// attack and a 60 ms release so nothing clicks
pub fn config(file: &str) -> Config {
    let mut cfg = Config::default();
    cfg.name = "Vocal Chops".into();
    cfg.attack = 0.003;
    cfg.decay = 0.3;
    cfg.sustain = 1.0;
    cfg.release = 0.06;
    let mut z = Zone::new("vocal", file);
    z.root = 60;
    cfg.zones.push(z);
    cfg
}

/// the lyric that makes a sampler note play chop `c` with its key as the pitch heard (an unpitched chop plays as it is at key 60)
pub fn slice_lyric(c: &Chop) -> String {
    let cents = c.midi.map(|m| (60.0 - m) * 100.0).unwrap_or(0.0);
    format!("@{}:{}:{:.0}", c.start, c.end, cents)
}

/// Ling Voice singing a handful of syllables, rendered: interleaved stereo at `sr`
pub fn sung_syllables(sr: f32, seed: u64) -> Vec<f32> {
    let sets: [&[(&str, u8)]; 3] = [
        &[("a", 64), ("ei", 67), ("o", 62), ("ja", 69), ("hej", 64), ("na", 67), ("u", 60), ("wo", 71)],
        &[("je", 67), ("a", 62), ("ou", 64), ("ni", 69), ("da", 60), ("hei", 67), ("o", 72), ("la", 64)],
        &[("o", 62), ("ja", 66), ("ei", 69), ("mu", 64), ("a", 71), ("wei", 67), ("i", 74), ("na", 62)],
    ];
    let set = sets[(seed % 3) as usize];
    let Some(mut v) = crate::dsp::make_instrument("voice", sr) else { return vec![] };
    let mut out = vec![];
    let mut buf = vec![0f32; 512];
    let note = (sr * 0.42) as usize;
    let gap = (sr * 0.22) as usize;
    for (lyric, key) in set {
        v.note_on_lyric(*key, 0.9, 0.0, 0.5, 0.5, lyric, note as f32);
        let mut n = 0;
        while n < note {
            buf.fill(0.0);
            v.render(&mut buf);
            out.extend_from_slice(&buf);
            n += buf.len() / 2;
        }
        v.note_off(*key);
        let mut n = 0;
        while n < gap {
            buf.fill(0.0);
            v.render(&mut buf);
            out.extend_from_slice(&buf);
            n += buf.len() / 2;
        }
    }
    let peak = out.iter().fold(0.0f32, |m, x| m.max(x.abs())).max(1e-6);
    for x in out.iter_mut() {
        *x *= 0.8 / peak;
    }
    out
}

/// what a section offers the chops: its length, key, energy and the notes of its other layers (relative to the section)
pub struct Ctx<'a> {
    pub beats: f64,
    pub key: (u8, bool),
    pub energy: f64,
    pub drums: &'a [Note],
    pub harmony: &'a [Note],
    pub lead: &'a [Note],
    pub style: Style,
    pub seed: u64,
    pub chops: &'a [Chop],
}

const CELLS: [&[u8]; 8] = [
    &[2, 6, 10, 14],
    &[0, 3, 6, 10, 12],
    &[8, 10, 11, 14],
    &[3, 6, 7, 11, 14],
    &[0, 2, 3, 5, 6, 8, 10, 11, 13, 14],
    &[1, 4, 6, 9, 12, 14],
    &[0, 6, 12],
    &[2, 3, 6, 10, 11, 14, 15],
];

/// a note playing chop `c` at pitch `p` (an unpitched chop at key 60, as it is)
fn chop_note(c: &Chop, start: f64, len: f64, p: i32, vel: f32) -> Note {
    tuned_note(c, start, len, p, vel, &Tune::default(), 0).0
}

/// A note playing chop `c`: a pitched chop sounds at `target` as `t` places it (tune.rs: the key is the target, the slice's cents
/// move the sung pitch there, the singer's drift kept by `1 - fine`) and `bend` semitones higher (the riser); an unpitched one plays
/// at key 60 as recorded. Also the shift in cents from the sung pitch (None for an unpitched chop, the bend not counted).
pub fn tuned_note(c: &Chop, start: f64, len: f64, target: i32, vel: f32, t: &Tune, bend: i32) -> (Note, Option<f32>) {
    let (key, cents, shift) = match c.midi {
        Some(f0) => {
            let (k, cents) = t.place(f0, target);
            ((k as i32 + bend).clamp(0, 127) as u8, cents, Some((k as f32 - 60.0) * 100.0 + cents))
        }
        None => (60, 0.0, None),
    };
    let mut n = Note::new(start, len.max(0.03), key, vel.clamp(0.05, 1.0));
    n.lyric = format!("@{}:{}:{:.0}", c.start, c.end, cents);
    (n, shift)
}

/// the lowest pitch class sounding at `t` in the harmony layers (the bass of the moment)
fn bass_at(h: &[Note], t: f64) -> Option<u8> {
    h.iter().filter(|n| n.start <= t + 1e-6 && n.start + n.len > t + 1e-6).map(|n| n.key).min().map(|k| k % 12)
}

/// the chord tones (pitch classes, root first) of the diatonic triad over `bass` in `key` (the tonic's when the bass is chromatic)
fn triad(key: Key, bass: Option<u8>) -> [u8; 3] {
    let sc = key.scale();
    let d = bass.and_then(|b| sc.iter().position(|p| *p == b)).unwrap_or(0);
    [sc[d], sc[(d + 2) % 7], sc[(d + 4) % 7]]
}

/// the nearest pitch to `near` with one of the pitch classes
fn nearest(pcs: &[u8], near: i32) -> i32 {
    (near - 6..=near + 6).filter(|p| pcs.contains(&(p.rem_euclid(12) as u8))).min_by_key(|p| (p - near).abs()).unwrap_or(near)
}

/// The chops of one section: (main track, layer track), sampler notes with their slices in the lyric.
pub fn compose(c: &Ctx) -> (Vec<Note>, Vec<Note>) {
    let mut main = vec![];
    let mut layer = vec![];
    if c.chops.is_empty() || c.beats < 1.0 {
        return (main, layer);
    }
    let pitched: Vec<usize> = (0..c.chops.len()).filter(|i| c.chops[*i].midi.is_some()).collect();
    let noisy: Vec<usize> = (0..c.chops.len()).filter(|i| c.chops[*i].midi.is_none()).collect();
    let pool = if pitched.is_empty() { &noisy } else { &pitched };
    let mut rng = Rng::new(c.seed ^ 0xC40_95);
    let key = Key::new(c.key.0, c.key.1);
    let step = 0.25;
    let bars = (c.beats / 4.0).ceil() as usize;
    let e = c.energy;
    let kicks: Vec<u8> = c.drums.iter().filter(|n| matches!(n.key, 35 | 36)).map(|n| ((n.start % 4.0) / step).round() as u8 % 16).collect();
    let snares: Vec<f64> = c.drums.iter().filter(|n| matches!(n.key, 38 | 40)).map(|n| n.start).collect();
    // the cell that sits best: off the kicks, where the lead rests, busier as the energy rises
    let busy = if e < 0.35 { 3.0 } else if e < 0.6 { 5.0 } else if e < 0.8 { 7.0 } else { 10.0 };
    let lead_at = |t: f64| c.lead.iter().any(|n| n.start <= t + 1e-6 && n.start + n.len > t + 1e-6);
    let mut best = (f64::MAX, 0usize);
    for (ci, cell) in CELLS.iter().enumerate() {
        let mut cost = (cell.len() as f64 - busy).abs();
        for s in cell.iter() {
            if kicks.contains(s) {
                cost += 0.8;
            }
            let hits = (0..bars).filter(|b| lead_at(*b as f64 * 4.0 + *s as f64 * step)).count();
            cost += hits as f64 / bars.max(1) as f64;
        }
        cost += (rng.next() % 1000) as f64 / 1000.0 * 1.5;
        if cost < best.0 {
            best = (cost, ci);
        }
    }
    let cell = CELLS[best.1];
    // two chops of the pool for this section, and a contour (steps along the chord tones) for the cell
    let a = pool[(rng.next() as usize) % pool.len()];
    let b = if pool.len() > 1 { pool[(pool.iter().position(|x| *x == a).unwrap_or(0) + 1 + (rng.next() as usize) % (pool.len() - 1)) % pool.len()] } else { a };
    let contour: Vec<i32> = cell.iter().map(|_| [0, 1, 1, -1, 2, 0, -1][(rng.next() % 7) as usize]).collect();
    let quiet = e < 0.35;
    let gate = if quiet { 1.0 } else if e < 0.7 { 0.9 } else { 0.75 };
    let layered = e >= 0.6 && rng.chance(0.4 + 0.5 * e);
    let up = if rng.chance(0.6) { 3 } else { 12 };
    let mut prev = 60 + ((c.key.0 as i32 + 4) % 12);
    // the medley tunes its chops to the chords (tune.rs defaults): the cell's notes are the intended melody
    let tune = Tune::default();
    for bar in 0..bars {
        if quiet && bar % 2 == 1 {
            continue;
        }
        let t0 = bar as f64 * 4.0;
        let answer = bar % 2 == 1;
        for (k, s) in cell.iter().enumerate() {
            if quiet && k % 2 == 1 {
                continue;
            }
            let t = t0 + *s as f64 * step;
            if t >= c.beats - 1e-6 {
                continue;
            }
            let next = cell.get(k + 1).map(|n| t0 + *n as f64 * step).unwrap_or(t0 + 4.0);
            let len = ((next - t) * gate).min(if quiet { 2.0 } else { 1.0 }).min(c.beats - t);
            let pcs = triad(key, bass_at(c.harmony, t));
            // the answer bar turns the contour around
            let dir = if answer { -contour[k] } else { contour[k] };
            let p = nearest(&pcs, prev + dir * 3);
            let p = if (p - 64).abs() > 9 { nearest(&pcs, 64) } else { p };
            prev = p;
            let (mine, other) = if (k + bar) % 3 == 2 { (b, a) } else { (a, b) };
            let mut which = &c.chops[mine];
            // a chop that would have to move too far for the note: the other chop of the section when it is nearer
            if let (Some(f), Some(g)) = (which.midi, c.chops[other].midi) {
                if (p as f32 - f).abs() > tune.max_shift && (p as f32 - g).abs() < (p as f32 - f).abs() {
                    which = &c.chops[other];
                }
            }
            let tones = tune.tones(key, c.harmony, t);
            let tp = which.midi.map(|f| tune.fit(f, p, &tones).0).unwrap_or(p);
            let vel = 0.62 + if *s % 4 == 2 { 0.18 } else { 0.0 } + 0.15 * e as f32;
            main.push(tuned_note(which, t, len, tp, vel as f32, &tune, 0).0);
            if layered {
                let hp = if up == 12 {
                    tp + 12
                } else {
                    // a diatonic third above
                    let sc = key.scale();
                    (tp + 3..=tp + 4).find(|x| sc.contains(&(x.rem_euclid(12) as u8))).unwrap_or(tp + 4)
                };
                match which.midi {
                    Some(f) => {
                        if let Some(lp) = tune.layer(f, tp, hp, &tones) {
                            layer.push(tuned_note(which, t, len, lp, vel as f32 * 0.8, &tune, 0).0);
                        }
                    }
                    None => layer.push(chop_note(which, t, len, hp, vel as f32 * 0.8)),
                }
            }
        }
        // a stutter closes every second bar (every bar in breakcore): the chop retriggered in 1/32s, rising in velocity
        let stutter = !quiet && (c.style == Style::Breakcore || bar % 2 == 1) && e >= 0.45;
        if stutter {
            let n = if c.style == Style::Breakcore && e >= 0.7 { 8 } else { 4 };
            let ts = t0 + 4.0 - n as f64 * 0.125;
            main.retain(|x| x.start < ts - 1e-6 || x.start >= t0 + 4.0);
            for j in 0..n {
                let t = ts + j as f64 * 0.125;
                if t < c.beats - 1e-6 {
                    let ca = &c.chops[a];
                    let tp = ca.midi.map(|f| tune.fit(f, prev, &tune.tones(key, c.harmony, t)).0).unwrap_or(prev);
                    main.push(tuned_note(ca, t, 0.11, tp, 0.5 + 0.4 * j as f32 / n as f32, &tune, 0).0);
                }
            }
        }
    }
    // an unpitched chop on the snares when it is busy
    if !noisy.is_empty() && (e >= 0.7 || c.style == Style::Breakcore) {
        let nz = &c.chops[noisy[(rng.next() as usize) % noisy.len()]];
        for t in snares.iter().filter(|t| **t < c.beats) {
            main.push(chop_note(nz, *t, 0.2, 60, 0.55));
        }
    }
    main.sort_by(|x, y| x.start.total_cmp(&y.start));
    layer.sort_by(|x, y| x.start.total_cmp(&y.start));
    (main, layer)
}

fn note(start: f64, len: f64, key: u8, vel: f32) -> Note {
    Note::new(start, len.max(0.03), key, vel.clamp(0.05, 1.0))
}

/// An amen-like break for a section (GM drum keys): the bar's four quarter-slices re-ordered bar by bar, ghost snares, an open
/// hat, a crash at the top, 1/32 snare rolls into the section's end; double time under 110 bpm. Empty when it is quiet.
pub fn breaks(beats: f64, energy: f64, bpm: f64, seed: u64) -> Vec<Note> {
    let mut out = vec![];
    if energy < 0.4 || beats < 2.0 {
        return out;
    }
    // (step of 16, key, velocity): the amen's first bar
    const AMEN: [(u8, u8, f32); 14] = [
        (0, 36, 0.95), (2, 36, 0.8), (4, 38, 0.9), (7, 38, 0.45), (9, 38, 0.5), (10, 36, 0.85), (11, 36, 0.7), (12, 38, 0.9), (15, 38, 0.4),
        (0, 42, 0.6), (4, 42, 0.5), (8, 42, 0.6), (12, 42, 0.5), (14, 46, 0.55),
    ];
    let fast = bpm < 110.0;
    let bar_len = if fast { 2.0 } else { 4.0 };
    let step = bar_len / 16.0;
    let mut rng = Rng::new(seed ^ 0xB4EA_C0DE);
    let bars = (beats / bar_len).ceil() as usize;
    for bar in 0..bars {
        let t0 = bar as f64 * bar_len;
        // the slice order: the plain break, then re-ordered slices (the breakcore edit)
        let order: [u8; 4] = if bar % 4 == 0 {
            [0, 1, 2, 3]
        } else {
            let mut o = [0u8, 1, 2, 3];
            for i in (1..4).rev() {
                let j = (rng.next() % (i as u64 + 1)) as usize;
                o.swap(i, j);
            }
            o
        };
        for (slot, src) in order.iter().enumerate() {
            for (s, k, v) in AMEN.iter().filter(|x| x.0 / 4 == *src) {
                let t = t0 + (slot as u8 * 4 + s % 4) as f64 * step;
                out.push(note(t, step * 0.9, *k, v * (0.75 + 0.25 * energy as f32)));
            }
        }
        if rng.chance(0.5 * energy) {
            out.push(note(t0 + 13.0 * step, step * 0.5, 38, 0.3));
        }
        if bar == 0 {
            out.push(note(t0, 1.0, 49, 0.7));
        }
    }
    // the roll into the end: the last beat in 1/32 snares, crescendo
    let n = if fast { 16 } else { 8 };
    let ts = beats - n as f64 * (step / 2.0);
    out.retain(|x| x.start < ts - 1e-6);
    for j in 0..n {
        out.push(note(ts + j as f64 * step / 2.0, step / 2.0 * 0.9, 38, 0.35 + 0.6 * j as f32 / n as f32));
    }
    out.retain(|x| x.start < beats - 1e-6);
    out.sort_by(|x, y| x.start.total_cmp(&y.start));
    out
}

// ════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════
// Lyric-aware chops and the EDM chop arrangement (Vocal Chops window, `daw_chopx_*`)
// ════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════
//
// * Cut at the syllables ([`Vocal::from_notes`]): the notes of the sung part (start, length, key) and what they sing (the syllables
//   written by lyricfit::fit_text: " fal" "ling", "~" holds) say where every syllable and word starts in the recording. Each
//   syllable is a chop whose edges are the note edges, moved at most 15 ms to the quietest point of the recording there; nothing is
//   cut through the middle of a sound. Without notes the chops are the transients of the audio ([`Vocal::from_audio`]).
// * Know the words ([`Vocal::index`]): words, lines, stressed syllables (first syllable of a word, content words, long notes) and
//   the hook - the line with the repeated content words (more when they sit in the chorus), and its word with the best score.
// * Arrange ([`compose_edm`]) per section by what the section is ([`role_of`]):
//     drop / chorus  a dry hit of the hook word on the downbeat, the hook as call and response (words at the chord tones, each syllable
//                    the nearest chord tone to the pitch it was sung at), a gated rhythm of the hook's syllables in order on 1/16s
//                    that skips the kicks and leans on the snares and the off-beats, stutter edits ("fa-fa-fa-falling") on 1/16 or
//                    1/32 that land on the word, a stutter closing every fourth bar, the layer a third or an octave up when it is loud;
//     build          the last phrase of the vocal repeated like a DJ beat-repeat - 1 bar, 1/2, 1/4, 1/8, 1/16, 1/32 ([`beat_repeat`]) -
//                    the velocity rising and a high-pass lane opening with it, then a short silence and the drop (the high-pass snaps open);
//     before a drop  a reverse chop of the hook word that swells into the downbeat;
//     verse / break  a few soft chops of the hook word on the off-beat, or one long one.
//   How busy it is follows the section's energy and the density option; what lands where follows the project's bpm and the drums.
use crate::medley::{beat_repeat, Stage};
use std::collections::HashMap;

/// a sung note with what it sings and where it sits in the vocal recording
#[derive(Clone, Debug)]
pub struct VNote {
    /// beats (project time)
    pub start: f64,
    pub len: f64,
    pub key: u8,
    /// the written syllable(s) of the note: " fal", "ling", " ly-ric", "~" for a hold
    pub text: String,
    /// seconds into the recording
    pub s0: f64,
    pub s1: f64,
}
impl VNote {
    /// a note of a constant-tempo score: the recording starts at beat `origin`, `offset` seconds before it
    pub fn at(start: f64, len: f64, key: u8, text: &str, bpm: f64, origin: f64, offset: f64) -> VNote {
        let k = 60.0 / bpm;
        VNote { start, len, key, text: text.into(), s0: (start - origin) * k + offset, s1: (start + len - origin) * k + offset }
    }
}

/// how the syllables are cut
#[derive(Clone, Copy, Debug)]
pub struct CutOpts {
    /// start every chop this much early (a consonant's attack), never into the chop before
    pub pre_ms: f64,
    /// move the edges to the quietest point within 15 ms
    pub snap: bool,
}
impl Default for CutOpts {
    fn default() -> CutOpts {
        CutOpts { pre_ms: 3.0, snap: true }
    }
}

struct Unit {
    start: f64,
    len: f64,
    key: u8,
    text: String,
    s0: f64,
    s1: f64,
    new_word: bool,
}

/// the syllables of the notes: "~" lengthens the one before, "ly-ric" in one note is two syllables in proportion to their letters,
/// a word starts at a leading space (or at every note when none is spaced: IPA / unwritten lyrics)
fn units(notes: &[VNote]) -> Vec<Unit> {
    let mut ns: Vec<&VNote> = notes.iter().collect();
    ns.sort_by(|a, b| a.start.total_cmp(&b.start));
    let spaced = ns.iter().any(|n| n.text.trim_start_matches('\n').starts_with(' '));
    let mut out: Vec<Unit> = vec![];
    for n in ns {
        let raw = n.text.trim_start_matches('\n');
        let t = raw.trim();
        if t == "~" {
            if let Some(p) = out.last_mut() {
                if n.start - (p.start + p.len) <= 0.3 {
                    p.len = n.start + n.len - p.start;
                    p.s1 = p.s1.max(n.s1);
                }
            }
            continue;
        }
        let parts: Vec<&str> = t.split('-').filter(|p| !p.is_empty()).collect();
        if parts.is_empty() {
            continue;
        }
        let total: f64 = parts.iter().map(|p| p.chars().count().max(1) as f64).sum();
        let first = out.is_empty();
        let mut acc = 0.0;
        for (i, p) in parts.iter().enumerate() {
            let share = p.chars().count().max(1) as f64 / total;
            out.push(Unit {
                start: n.start + n.len * acc,
                len: n.len * share,
                key: n.key,
                text: p.to_string(),
                s0: n.s0 + (n.s1 - n.s0) * acc,
                s1: n.s0 + (n.s1 - n.s0) * (acc + share),
                new_word: i == 0 && (first || !spaced || raw.starts_with(' ')),
            });
            acc += share;
        }
    }
    out
}

fn norm_word(t: &str) -> String {
    t.chars().filter(|c| c.is_alphabetic() || *c == '\'').flat_map(|c| c.to_lowercase()).collect()
}

const STOP: [&str; 36] = [
    "a", "an", "the", "and", "or", "but", "of", "to", "in", "on", "at", "is", "it", "its", "i", "im", "me", "my", "we", "you", "your", "so", "do", "be", "am", "are", "was", "for", "with", "that", "this", "just", "like", "up", "if", "as",
];

/// a cut of the vocal with what it sings
#[derive(Clone, Debug)]
pub struct Piece {
    pub chop: Chop,
    /// the written syllable (lower case letters), "" when it is not known
    pub text: String,
    pub word: usize,
    /// which syllable of its word, and how many the word has
    pub nth: usize,
    pub of: usize,
    /// beats (project time)
    pub start: f64,
    pub len: f64,
    pub key: u8,
    /// 0 .. 1.5: how much the syllable carries
    pub stress: f32,
}

/// a phrase: pieces `first..end`, between two rests of a beat or more
#[derive(Clone, Debug)]
pub struct Line {
    pub first: usize,
    pub end: usize,
    pub start: f64,
    pub stop: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Vocal {
    pub pieces: Vec<Piece>,
    pub lines: Vec<Line>,
    /// piece ranges of the words, and what they say
    pub words: Vec<(usize, usize)>,
    pub word_text: Vec<String>,
    pub hook_line: Option<usize>,
    pub hook_word: Option<usize>,
    /// reversed copies of the hook word's pieces in the bank file ([`Vocal::add_reverse`]): (piece, chop of the copy)
    pub rev: Vec<(usize, Chop)>,
    pub rate: u32,
    pub bpm: f64,
}

/// the hop whose level is within 2 % of the peak of the lowest in `[frame - back, frame + fwd]`, the one nearest `frame`
fn snap_edge(env: &[f32], hop: usize, frame: i64, back: i64, fwd: i64, peak: f32) -> i64 {
    if env.is_empty() {
        return frame;
    }
    let hi_max = env.len() as i64 - 1;
    let lo = ((frame - back).max(0) / hop as i64).min(hi_max);
    let hi = ((frame + fwd).max(0) / hop as i64).clamp(lo, hi_max);
    let mn = (lo..=hi).map(|h| env[h as usize]).fold(f32::MAX, f32::min);
    let eps = mn + 0.02 * peak;
    (lo..=hi).filter(|h| env[*h as usize] <= eps).min_by_key(|h| (h * hop as i64 - frame).abs()).map(|h| h * hop as i64).unwrap_or(frame)
}

/// the pitch of a chop (tune.rs: the steady voiced middle), None when it is unvoiced or unsure
fn chop_pitch(mono: &[f32], s: usize, e: usize, rate: u32) -> Option<f32> {
    if e > mono.len() {
        return None;
    }
    chop_f0(mono, s, e, rate).map(|f| f.midi)
}

impl Vocal {
    /// Chops at the syllables of `notes` in the recording `stereo` (interleaved). `hot` are the project's chorus spans in beats,
    /// which weigh the hook words.
    pub fn from_notes(stereo: &[f32], rate: u32, notes: &[VNote], bpm: f64, o: &CutOpts, hot: &[(f64, f64)]) -> Vocal {
        let us = units(notes);
        let hop = (rate as usize / 200).max(1);
        let (mono, env) = mono_env(stereo, hop);
        let frames = mono.len() as i64;
        let peak = env.iter().copied().fold(0.0f32, f32::max);
        let ms = |x: f64| (x * rate as f64 / 1000.0) as i64;
        let msi = |x: i64| ms(x as f64);
        let mut v = Vocal { rate, bpm, ..Vocal::default() };
        let mut wid = 0usize;
        let mut prev_end = 0i64;
        for (i, u) in us.iter().enumerate() {
            let next_s0 = us.get(i + 1).map(|n| n.s0);
            let s1 = match next_s0 {
                Some(n) => u.s1.min(n.max(u.s0 + 0.02)),
                None => u.s1,
            };
            let mut s = (u.s0 * rate as f64).round() as i64;
            let mut e = (s1 * rate as f64).round() as i64;
            if s >= frames || e <= 0 {
                continue;
            }
            s = s.max(0);
            e = e.min(frames);
            if o.snap && peak > 0.0 {
                let lim = next_s0.map(|n| (n * rate as f64).round() as i64 - e).unwrap_or(msi(10)).clamp(0, msi(10));
                s = snap_edge(&env, hop, s, msi(15), msi(5), peak);
                e = snap_edge(&env, hop, e, msi(12), lim, peak);
            }
            s = (s - ms(o.pre_ms)).max(prev_end).max(0);
            if e < s + 3 * hop as i64 {
                e = (s + 3 * hop as i64).min(frames);
            }
            if e <= s {
                continue;
            }
            prev_end = e;
            let (su, eu) = (s as usize, e as usize);
            let level = env[(su / hop).min(env.len().saturating_sub(1))..(eu / hop).clamp(1, env.len())].iter().sum::<f32>() / ((eu - su) / hop).max(1) as f32;
            let midi = chop_pitch(&mono, su, eu, rate);
            if u.new_word && !v.pieces.is_empty() {
                wid += 1;
            }
            v.pieces.push(Piece { chop: Chop { start: s as u32, end: e as u32, midi, level }, text: norm_word(&u.text), word: wid, nth: 0, of: 1, start: u.start, len: u.len, key: u.key, stress: 0.0 });
        }
        v.index(hot);
        v
    }

    /// Chops at the transients of a recording nobody wrote down: `origin` is the beat the recording starts at.
    pub fn from_audio(stereo: &[f32], rate: u32, bpm: f64, origin: f64, hot: &[(f64, f64)]) -> Vocal {
        let hop = (rate as usize / 200).max(1);
        let (mono, env) = mono_env(stereo, hop);
        let frames = mono.len();
        let tr = transients(stereo, rate);
        let mut v = Vocal { rate, bpm, ..Vocal::default() };
        let top = env.iter().copied().fold(1e-9f32, f32::max);
        for (i, (s, _)) in tr.iter().enumerate() {
            let s = *s as usize;
            let limit = tr.get(i + 1).map(|x| x.0 as usize).unwrap_or(frames).min(s + (0.6 * rate as f64) as usize).min(frames);
            let lo = s + (0.08 * rate as f64) as usize;
            if lo + hop >= limit {
                continue;
            }
            let (ha, hb) = (lo / hop, (limit / hop).min(env.len()) - 1);
            let mn = (ha..=hb).map(|h| env[h]).fold(f32::MAX, f32::min);
            let h = (ha..=hb).filter(|h| env[*h] <= mn + 0.02 * top).max().unwrap_or(hb);
            let e = ((h + 1) * hop).min(frames);
            if e <= s + 4 * hop {
                continue;
            }
            let level = env[s / hop..(e / hop).min(env.len())].iter().sum::<f32>() / ((e - s) / hop).max(1) as f32;
            let midi = chop_pitch(&mono, s, e, rate);
            let beat = origin + s as f64 / rate as f64 * bpm / 60.0;
            let len = (e - s) as f64 / rate as f64 * bpm / 60.0;
            let wid = v.pieces.len();
            let key = midi.map(|m| m.round().clamp(24.0, 108.0) as u8).unwrap_or(60);
            v.pieces.push(Piece { chop: Chop { start: s as u32, end: e as u32, midi, level }, text: String::new(), word: wid, nth: 0, of: 1, start: beat, len, key, stress: 0.0 });
        }
        v.index(hot);
        v
    }

    /// words, lines, stress and the hook from the pieces
    fn index(&mut self, hot: &[(f64, f64)]) {
        self.words.clear();
        self.word_text.clear();
        self.lines.clear();
        let n = self.pieces.len();
        let mut i = 0;
        while i < n {
            let mut j = i + 1;
            while j < n && self.pieces[j].word == self.pieces[i].word {
                j += 1;
            }
            let of = j - i;
            for k in i..j {
                self.pieces[k].nth = k - i;
                self.pieces[k].of = of;
            }
            self.words.push((i, j));
            self.word_text.push(self.pieces[i..j].iter().map(|p| p.text.as_str()).collect::<String>());
            i = j;
        }
        // word ids are dense from here on
        for (wi, (a, b)) in self.words.iter().enumerate() {
            for p in &mut self.pieces[*a..*b] {
                p.word = wi;
            }
        }
        let known = self.pieces.iter().any(|p| !p.text.is_empty());
        let maxlevel = self.pieces.iter().map(|p| p.chop.level).fold(1e-9f32, f32::max);
        for p in &mut self.pieces {
            let mut s = if !known {
                0.7 * p.chop.level / maxlevel
            } else if p.of == 1 {
                if STOP.contains(&p.text.as_str()) || p.text.chars().count() < 2 {
                    0.3
                } else {
                    0.9
                }
            } else if p.nth == 0 {
                1.0
            } else {
                0.45
            };
            if p.len >= 1.0 {
                s += 0.3;
            } else if p.len >= 0.5 {
                s += 0.12;
            }
            if (p.start % 1.0).abs() < 0.05 {
                s += 0.1;
            }
            p.stress = s.clamp(0.0, 1.5);
        }
        // lines: a rest of a beat ends one
        let mut first = 0;
        for k in 1..=n {
            if k == n || self.pieces[k].start - (self.pieces[k - 1].start + self.pieces[k - 1].len) >= 1.0 {
                if k > first {
                    self.lines.push(Line { first, end: k, start: self.pieces[first].start, stop: self.pieces[k - 1].start + self.pieces[k - 1].len });
                }
                first = k;
            }
        }
        self.pick_hook(hot, known);
    }

    fn word_score(&self, wi: usize, counts: &HashMap<String, (f64, f64)>) -> f64 {
        let t = &self.word_text[wi];
        if t.chars().count() < 2 || STOP.contains(&t.as_str()) {
            return 0.0;
        }
        let (all, hot) = counts.get(t).copied().unwrap_or((1.0, 0.0));
        let (a, b) = self.words[wi];
        let stress = self.pieces[a..b].iter().map(|p| p.stress as f64).sum::<f64>() / (b - a) as f64;
        all + 1.5 * hot + 0.15 * (b - a) as f64 + 0.5 * stress
    }

    fn pick_hook(&mut self, hot: &[(f64, f64)], known: bool) {
        self.hook_line = None;
        self.hook_word = None;
        if self.lines.is_empty() {
            return;
        }
        let mut counts: HashMap<String, (f64, f64)> = HashMap::new();
        if known {
            for (wi, (a, _)) in self.words.iter().enumerate() {
                let at = self.pieces[*a].start;
                let c = counts.entry(self.word_text[wi].clone()).or_insert((0.0, 0.0));
                c.0 += 1.0;
                if hot.iter().any(|(s, e)| at >= *s && at < *e) {
                    c.1 += 1.0;
                }
            }
        }
        let sig = |l: &Line| -> String { (self.pieces[l.first].word..=self.pieces[l.end - 1].word).map(|w| self.word_text[w].as_str()).collect::<Vec<_>>().join(" ") };
        let sigs: Vec<String> = self.lines.iter().map(sig).collect();
        // with no words known, a line sung again (the same pitches, about as long) is the hook's sign (medley/acapella.rs)
        let reps: Vec<usize> = if known { vec![] } else { crate::medley::acapella::line_repeats(self) };
        let mut best = (f64::MIN, 0usize);
        for (li, l) in self.lines.iter().enumerate() {
            let (w0, w1) = (self.pieces[l.first].word, self.pieces[l.end - 1].word);
            let nw = (w1 - w0 + 1) as f64;
            let score = if known {
                let sum: f64 = (w0..=w1).map(|w| self.word_score(w, &counts)).sum();
                let rep = sigs.iter().filter(|s| **s == sigs[li]).count() as f64;
                sum / nw.sqrt() * (1.0 + 0.5 * (rep - 1.0)) * if (2.0..=8.0).contains(&nw) { 1.0 } else { 0.6 }
            } else {
                self.pieces[l.first..l.end].iter().map(|p| p.stress as f64 * p.len).sum::<f64>() * (1.0 + 0.5 * reps.get(li).copied().unwrap_or(0).min(4) as f64)
            };
            if score > best.0 {
                best = (score, li);
            }
        }
        self.hook_line = Some(best.1);
        let l = &self.lines[best.1];
        let (w0, w1) = (self.pieces[l.first].word, self.pieces[l.end - 1].word);
        self.hook_word = (w0..=w1).max_by(|a, b| {
            let sa = if known { self.word_score(*a, &counts) } else { 0.0 } + self.pieces[self.words[*a].0..self.words[*a].1].iter().map(|p| p.stress as f64 * p.len).sum::<f64>() * 0.1;
            let sb = if known { self.word_score(*b, &counts) } else { 0.0 } + self.pieces[self.words[*b].0..self.words[*b].1].iter().map(|p| p.stress as f64 * p.len).sum::<f64>() * 0.1;
            sa.total_cmp(&sb)
        });
    }

    /// the words of the hook line, as sung
    pub fn hook_text(&self) -> String {
        let Some(l) = self.hook_line.and_then(|i| self.lines.get(i)) else { return String::new() };
        (self.pieces[l.first].word..=self.pieces[l.end - 1].word).map(|w| self.word_text[w].clone()).collect::<Vec<_>>().join(" ")
    }

    /// the pieces of word `w`
    pub fn word_pieces(&self, w: usize) -> std::ops::Range<usize> {
        self.words.get(w).map(|(a, b)| *a..*b).unwrap_or(0..0)
    }

    /// the line (index) that ends last at or before `beat` (+ a beat of grace), else the hook's
    pub fn line_before(&self, beat: f64) -> Option<usize> {
        self.lines.iter().rposition(|l| l.stop <= beat + 1.0).or(self.hook_line)
    }

    /// the file for the sampler: the recording, then reversed copies of the pieces of the hook word (for the swell into a drop)
    pub fn add_reverse(&mut self, stereo: &[f32]) -> Vec<f32> {
        let mut bank = stereo.to_vec();
        self.rev.clear();
        let Some(w) = self.hook_word else { return bank };
        let pad = (self.rate as f64 * 0.05) as usize;
        bank.extend(vec![0.0; pad * 2]);
        for pi in self.word_pieces(w) {
            let c = self.pieces[pi].chop.clone();
            let (a, b) = (c.start as usize, (c.end as usize).min(stereo.len() / 2));
            if b <= a {
                continue;
            }
            let start = (bank.len() / 2) as u32;
            for f in (a..b).rev() {
                bank.push(stereo[f * 2]);
                bank.push(stereo[f * 2 + 1]);
            }
            let end = (bank.len() / 2) as u32;
            bank.extend(vec![0.0; pad * 2]);
            self.rev.push((pi, Chop { start, end, midi: c.midi, level: c.level }));
        }
        bank
    }

    /// the natural length of a piece in beats
    fn dur_beats(&self, pi: usize) -> f64 {
        let c = &self.pieces[pi].chop;
        (c.end - c.start) as f64 / self.rate.max(1) as f64 * self.bpm / 60.0
    }
}

// ── roles of sections ──

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Role {
    /// chorus / drop: the full chops
    Hot,
    /// a build-up: the DJ beat-repeat
    Build,
    /// a breakdown or bridge: a few long chops
    Break,
    /// verses, intros, outros: soft accents
    Calm,
}

pub fn role_of(tag: &str) -> Role {
    let t = tag.to_lowercase();
    if t.contains("build") || t.contains("riser") || t.contains("lift") || t.starts_with("pre") || t.contains("pre-") || t.contains("pre ") {
        Role::Build
    } else if t.contains("chorus") || t.contains("drop") || t.contains("hook") || t.contains("final") {
        Role::Hot
    } else if t.contains("break") || t.contains("bridge") {
        Role::Break
    } else {
        Role::Calm
    }
}

/// how loud a section of this tag usually is, 0 .. 1
pub fn tag_energy(tag: &str) -> f64 {
    let t = tag.to_lowercase();
    if t.contains("drop") || t.contains("final") {
        0.95
    } else if t.contains("chorus") || t.contains("hook") {
        0.85
    } else if t.contains("build") || t.contains("riser") || t.contains("lift") {
        0.7
    } else if t.starts_with("pre") || t.contains("pre-") {
        0.6
    } else if t.contains("verse") {
        0.5
    } else if t.contains("break") || t.contains("bridge") {
        0.35
    } else {
        0.25
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Build {
    Off,
    /// a DJ beat-repeat of the last phrase, shortening
    Dj,
    /// stutters of one syllable, speeding up
    Stutter,
}
impl Build {
    pub fn parse(s: &str) -> Build {
        match s.trim().to_lowercase().as_str() {
            "off" | "none" | "no" | "0" => Build::Off,
            "stutter" | "stutters" => Build::Stutter,
            _ => Build::Dj,
        }
    }
}

/// what the user asks of the arrangement
#[derive(Clone, Debug)]
pub struct Edm {
    /// 0 .. 1: how many gated chops
    pub density: f64,
    /// 0 .. 1: how often and how fast the stutters are
    pub stutter: f64,
    pub build: Build,
    pub layer: bool,
    pub reverse: bool,
    pub dry: bool,
    /// the last stages of a build bend up to an octave
    pub riser: bool,
    /// how the chops are tuned to the song (tune.rs)
    pub tune: Tune,
}
impl Default for Edm {
    fn default() -> Edm {
        Edm { density: 0.6, stutter: 0.5, build: Build::Dj, layer: true, reverse: true, dry: true, riser: false, tune: Tune::default() }
    }
}

/// what a section offers the arrangement (notes relative to the section start)
pub struct EdmCtx<'a> {
    pub beats: f64,
    pub key: (u8, bool),
    pub energy: f64,
    pub tag: &'a str,
    pub drums: &'a [Note],
    pub harmony: &'a [Note],
    pub vocal: &'a Vocal,
    pub bpm: f64,
    pub seed: u64,
    pub o: &'a Edm,
    /// the last beats of the section that are a build-up (0 = none; a build section is all of it)
    pub build_beats: f64,
    /// a drop or a build follows: a reverse swell into its downbeat
    pub next_hot: bool,
    /// the line the build repeats (None = the hook line)
    pub loop_line: Option<usize>,
    /// only the reverse swell into what follows (a section that gets no chops of its own)
    pub only_swell: bool,
}

#[derive(Default, Debug)]
pub struct EdmOut {
    pub main: Vec<Note>,
    pub layer: Vec<Note>,
    /// the high-pass lane of the build: (beat in the section, 0..1)
    pub hp: Vec<(f64, f32)>,
    /// the loop stages of the build, in section beats
    pub stages: Vec<Stage>,
    /// starts of the gated rhythm notes
    pub fills: Vec<f64>,
    pub stutters: usize,
    pub swells: usize,
    pub dry: bool,
    /// the silence before the drop, beats
    pub silence: f64,
    /// what the tuning did (main track notes)
    pub tune: TuneStats,
}

struct Mk<'a> {
    v: &'a Vocal,
    key: Key,
    harmony: &'a [Note],
    beats: f64,
    layered: bool,
    octave: bool,
    main: Vec<Note>,
    layer: Vec<Note>,
    tune: Tune,
    stats: TuneStats,
}
impl Mk<'_> {
    fn natural(&self, pi: usize) -> i32 {
        self.v.pieces[pi].chop.midi.map(|m| m.round() as i32).unwrap_or(60)
    }
    /// where piece `pi` sounds at `t`: the chord tone (else the key's tone) nearest to the pitch it was sung at; the sung semitone
    /// when tuning is off
    fn pitch(&self, pi: usize, t: f64) -> i32 {
        match self.v.pieces[pi].chop.midi {
            Some(f) => self.tune.nearest(f, &self.tune.tones(self.key, self.harmony, t)),
            None => 60,
        }
    }
    /// a note the arrangement means for piece `pi` at `t` (the written note of a dry hit), within the shift cap
    fn intended(&mut self, pi: usize, t: f64, want: i32) -> i32 {
        match self.v.pieces[pi].chop.midi {
            Some(f) if self.tune.on() => {
                let (p, capped) = self.tune.fit(f, want, &self.tune.tones(self.key, self.harmony, t));
                self.stats.capped += capped as usize;
                p
            }
            _ => self.natural(pi),
        }
    }
    /// a note playing piece `pi` (shortened to `trim` beats when asked, and to its note when that is shorter than the sound) at
    /// `pitch`, `bend` semitones higher (the riser)
    #[allow(clippy::too_many_arguments)]
    fn put(&mut self, pi: usize, at: f64, len: f64, vel: f32, pitch: i32, bend: i32, trim: Option<f64>, lay: bool) -> bool {
        if at >= self.beats - 1e-6 || at < -1e-6 {
            return false;
        }
        let len = len.min(self.beats - at).max(0.03);
        let mut c = self.v.pieces[pi].chop.clone();
        let rate = self.v.rate.max(1) as f64;
        let k = 60.0 / self.v.bpm;
        let dur = (c.end - c.start) as f64 / rate;
        let mut want = trim.map(|b| b * k).unwrap_or(dur).min(dur);
        if len * k < want * 0.8 {
            want = len * k;
        }
        c.end = (c.start + (want.max(0.02) * rate) as u32).min(c.end).max(c.start + 1);
        let (n, shift) = tuned_note(&c, at, len, pitch, vel, &self.tune, bend);
        self.main.push(n);
        self.stats.note(shift, false);
        if let (true, true, Some(f)) = (lay, self.layered, c.midi) {
            // an octave, or the next chord tone a third / fourth above (a diatonic third when tuning to the key or not at all)
            let tones = self.tune.tones(self.key, self.harmony, at);
            let hp = if self.octave { pitch + 12 } else { (pitch + 3..=pitch + 5).find(|x| tones.contains(&(x.rem_euclid(12) as u8))).unwrap_or(pitch + 4) };
            // an octave is too far for a voice under the shift cap: the next chord tone above that is not
            if let Some(lp) = self.tune.layer(f, pitch, hp, &tones) {
                self.layer.push(tuned_note(&c, at, len, lp, vel * 0.8, &self.tune, 0).0);
            }
        }
        true
    }
    /// a word at `at`, its syllables where they were sung; returns its length in beats
    fn word(&mut self, w: usize, at: f64, vel: f32, dry: bool, lay: bool, max_span: f64) -> f64 {
        let r = self.v.word_pieces(w);
        if r.is_empty() {
            return 0.0;
        }
        let t0 = self.v.pieces[r.start].start;
        let mut span = 0.0f64;
        for pi in r.clone() {
            let p = &self.v.pieces[pi];
            let off = p.start - t0;
            if off > max_span {
                break;
            }
            let next = if pi + 1 < r.end { self.v.pieces[pi + 1].start - t0 } else { f64::MAX };
            let len = self.v.dur_beats(pi).min(next - off).max(0.1);
            // a dry hit sounds the note the singer meant (its written note, octave and cap permitting; as sung when tuning is off)
            let pitch = if dry { self.intended(pi, at + off, p.key as i32) } else { self.pitch(pi, at + off) };
            self.put(pi, at + off, len, vel, pitch, 0, None, lay);
            span = span.max(off + len);
        }
        span
    }
}

/// the 16th steps (0..15) of the drum hits with these keys in the bar starting at `t0`
fn steps_in(drums: &[Note], keys: &[u8], t0: f64) -> Vec<u8> {
    drums.iter().filter(|n| keys.contains(&n.key) && n.start >= t0 - 1e-6 && n.start < t0 + 4.0 - 1e-6).map(|n| (((n.start - t0) / 0.25).round() as i64).clamp(0, 15) as u8).collect()
}

/// A gated rhythm over `avail` beats of the bar at `t0`: `hits` notes on the 16th grid, away from the kicks (and the places in
/// `taken`), leaning on the off-beats and the snares, sounding the `pool` syllables in order.
#[allow(clippy::too_many_arguments)]
fn gated(m: &mut Mk, rng: &mut Rng, t0: f64, avail: f64, hits: usize, taken: &[(f64, f64)], kicks: &[u8], snares: &[u8], pool: &[usize], cursor: &mut usize, base: f32, fills: &mut Vec<f64>) {
    if pool.is_empty() || hits == 0 {
        return;
    }
    let mut cand: Vec<(f64, u8)> = vec![];
    for s in 0..16u8 {
        let t = t0 + s as f64 * 0.25;
        if t >= t0 + avail - 1e-6 || taken.iter().any(|(a, b)| t >= *a - 1e-6 && t < *b - 1e-6) || kicks.contains(&s) {
            continue;
        }
        let mut w = match s % 4 {
            2 => 1.0,
            1 | 3 => 0.55,
            _ => 0.25,
        };
        if snares.contains(&s) {
            w += 0.7;
        }
        cand.push((w + rng.f() * 0.35, s));
    }
    cand.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut chosen: Vec<u8> = cand.iter().take(hits).map(|c| c.1).collect();
    chosen.sort_unstable();
    for (k, s) in chosen.iter().enumerate() {
        let t = t0 + *s as f64 * 0.25;
        let gap = chosen.get(k + 1).map(|n| (*n - *s) as f64 * 0.25).unwrap_or(t0 + avail - t);
        let len = (gap.min(0.5) * 0.75).max(0.15);
        let pi = pool[*cursor % pool.len()];
        *cursor += 1;
        let vel = base + if snares.contains(s) { 0.2 } else { 0.0 } + if *s % 4 == 2 { 0.1 } else { 0.0 };
        let pitch = m.pitch(pi, t);
        if m.put(pi, t, len, vel.min(1.0), pitch, 0, None, true) {
            fills.push(t);
        }
    }
}

/// The pieces of `line` that a loop of `l` beats, ending where the line ends, repeats: (offset in the loop, piece, beats, a part of
/// one syllable). A loop shorter than the last syllable is its first `l` beats - the stutter "fa-fa-fa".
fn loop_events(v: &Vocal, line: &Line, l: f64) -> Vec<(f64, usize, f64, bool)> {
    let ws = line.stop - l;
    let mut ev: Vec<(f64, usize, f64, bool)> = vec![];
    for pi in line.first..line.end {
        let p = &v.pieces[pi];
        if p.start >= ws - 1e-6 {
            ev.push((p.start - ws, pi, v.dur_beats(pi), false));
        }
    }
    if ev.is_empty() {
        // the stressed syllable of the last word
        let w = v.pieces[line.end - 1].word;
        let r = v.word_pieces(w);
        let pi = r.clone().max_by(|a, b| v.pieces[*a].stress.total_cmp(&v.pieces[*b].stress)).unwrap_or(line.end - 1);
        ev.push((0.0, pi, l, true));
    }
    for k in 0..ev.len() {
        let cap = ev.get(k + 1).map(|n| n.0).unwrap_or(l) - ev[k].0;
        ev[k].2 = ev[k].2.min(cap).max(0.03);
    }
    ev
}

/// The chops of one section: an arrangement of the vocal's syllables around the drums, in the key, by the section's role.
pub fn compose_edm(c: &EdmCtx) -> EdmOut {
    let mut out = EdmOut::default();
    let v = c.vocal;
    if v.pieces.is_empty() || c.beats < 1.0 || v.bpm <= 0.0 {
        return out;
    }
    let role = role_of(c.tag);
    let e = c.energy.clamp(0.0, 1.0);
    let o = c.o;
    let mut rng = Rng::new(c.seed ^ 0xED_C405);
    let key = Key::new(c.key.0, c.key.1);
    let hook_line = c.vocal.hook_line.unwrap_or(0).min(v.lines.len().saturating_sub(1));
    let hook_word = v.hook_word.unwrap_or(0);
    let line = &v.lines[hook_line];
    // the second word of the hook line, for the answer
    let (w0, w1) = (v.pieces[line.first].word, v.pieces[line.end - 1].word);
    let second = (w0..=w1).filter(|w| *w != hook_word).max_by(|a, b| {
        let s = |w: &usize| v.pieces[v.words[*w].0..v.words[*w].1].iter().map(|p| p.stress as f64 * p.len).sum::<f64>();
        s(a).total_cmp(&s(b))
    });
    let second = second.unwrap_or(hook_word);
    let build_beats = if c.build_beats > 0.0 {
        c.build_beats.min(c.beats)
    } else if role == Role::Build && o.build != Build::Off {
        c.beats
    } else {
        0.0
    };
    let body = (c.beats - build_beats).max(0.0);
    let layered = o.layer && e >= 0.6 && rng.chance(0.4 + 0.5 * e);
    let mut m = Mk { v, key, harmony: c.harmony, beats: c.beats, layered, octave: rng.chance(0.4), main: vec![], layer: vec![], tune: o.tune, stats: TuneStats::default() };
    // the syllables the gated rhythm sounds, in the order they were sung
    let mut pool: Vec<usize> = (line.first..line.end).filter(|p| v.pieces[*p].stress >= 0.6).collect();
    if pool.len() < 2 {
        pool = (line.first..line.end).collect();
    }
    let mut cursor = 0usize;
    let grid = if o.stutter >= 0.7 && e >= 0.65 { 0.125 } else { 0.25 };
    let bars = (body / 4.0).ceil() as usize;
    match role {
        _ if body < 0.5 || c.only_swell => {}
        Role::Hot => {
            let hits = ((2.0 + 7.0 * o.density.clamp(0.0, 1.0)) * (0.45 + 0.55 * e)).round().clamp(2.0, 11.0) as usize;
            for bar in 0..bars {
                let t0 = bar as f64 * 4.0;
                let avail = (body - t0).min(4.0);
                if avail < 0.5 {
                    break;
                }
                let kicks = steps_in(c.drums, &[35, 36], t0);
                let snares = steps_in(c.drums, &[38, 40], t0);
                let mut taken: Vec<(f64, f64)> = vec![];
                match bar % 4 {
                    0 => {
                        // the call (a dry hit of the word on the very first downbeat) and the answer on beat 3
                        let dry = bar == 0 && o.dry;
                        let span = m.word(hook_word, t0, if dry { 1.0 } else { 0.85 }, dry, !dry, 2.0);
                        out.dry |= dry;
                        taken.push((t0, t0 + span.max(0.5)));
                        if avail >= 3.0 {
                            let span = m.word(second, t0 + 2.0, 0.8, false, true, 1.75);
                            taken.push((t0 + 2.0, t0 + 2.0 + span.max(0.5)));
                        }
                    }
                    2 => {
                        m.word(hook_word, t0, 0.8, false, true, 1.0);
                        taken.push((t0, t0 + 1.0));
                        if o.stutter >= 0.15 && avail >= 2.5 {
                            // fa-fa-fa-falling: the first syllable repeated into the word on beat 3
                            let k = if o.stutter >= 0.6 { 4 } else { 3 };
                            let landing = t0 + 2.0;
                            let fp = v.words[hook_word].0;
                            for j in 0..k {
                                let at = landing - (k - j) as f64 * 0.25;
                                let vel = 0.55 + 0.3 * j as f32 / k as f32;
                                let pitch = m.pitch(fp, at);
                                m.put(fp, at, 0.22, vel, pitch, 0, Some(0.23), false);
                            }
                            out.stutters += 1;
                            let span = m.word(hook_word, landing, 0.95, false, true, 1.5);
                            taken.push((landing - k as f64 * 0.25, landing + span.max(0.5)));
                        }
                    }
                    _ => {}
                }
                // the stutter that closes the fourth bar: the last syllable of the hook in the last beat
                if bar % 4 == 3 && o.stutter >= 0.25 && e >= 0.4 && avail >= 4.0 {
                    let lp = line.end - 1;
                    let n = (1.0 / grid) as usize;
                    for j in 0..n {
                        let at = t0 + 3.0 + j as f64 * grid;
                        let vel = 0.5 + 0.45 * j as f32 / n as f32;
                        let pitch = m.pitch(lp, at);
                        m.put(lp, at, grid * 0.9, vel, pitch, 0, Some(grid * 0.92), false);
                    }
                    out.stutters += 1;
                    taken.push((t0 + 3.0, t0 + 4.0));
                }
                let h = if bar % 4 == 1 { hits } else { (hits * 2 / 3).max(2) };
                gated(&mut m, &mut rng, t0, avail, h, &taken, &kicks, &snares, &pool, &mut cursor, 0.55 + 0.15 * e as f32, &mut out.fills);
            }
        }
        Role::Break => {
            for bar in (0..bars).step_by(2) {
                let t0 = bar as f64 * 4.0;
                let w = if (bar / 2) % 2 == 0 { hook_word } else { second };
                m.word(w, t0, 0.65, false, false, 3.0);
            }
        }
        _ => {
            // verses, intros: a soft accent of the hook word on the off-beat of every other bar (every fourth when it is thin)
            let every = if o.density < 0.3 { 4 } else { 2 };
            for bar in (0..bars).step_by(every) {
                m.word(if (bar / every) % 2 == 0 { hook_word } else { second }, bar as f64 * 4.0 + 2.0, 0.5, false, false, 1.5);
            }
            if e >= 0.5 && o.density >= 0.7 {
                let kicks = steps_in(c.drums, &[35, 36], 0.0);
                let snares = steps_in(c.drums, &[38, 40], 0.0);
                for bar in (1..bars).step_by(2) {
                    gated(&mut m, &mut rng, bar as f64 * 4.0, (body - bar as f64 * 4.0).min(4.0), 3, &[], &kicks, &snares, &pool, &mut cursor, 0.45, &mut out.fills);
                }
            }
        }
    }
    // the build: the last phrase repeated, the loop halving, a high-pass opening, a breath of silence
    if build_beats >= 1.0 && !c.only_swell && o.build != Build::Off {
        let b0 = body;
        let bl = build_beats;
        let silence = if bl >= 8.0 { 0.5 } else if bl >= 4.0 { 0.25 } else { 0.0 };
        out.silence = silence;
        let total = bl - silence;
        let lline = &v.lines[c.loop_line.unwrap_or(hook_line).min(v.lines.len() - 1)];
        let mut stages = beat_repeat(total, if bl >= 8.0 && e >= 0.5 { 0.125 } else { 0.25 });
        if o.build == Build::Stutter {
            let st = beat_repeat(total, 0.125);
            let keep: Vec<Stage> = st.iter().copied().filter(|s| s.len <= 0.5 + 1e-9).collect();
            if !keep.is_empty() {
                let sum: f64 = keep.iter().map(|s| s.len * s.reps as f64).sum();
                let mut at = (total - sum).max(0.0);
                stages = keep
                    .iter()
                    .map(|s| {
                        let r = Stage { at, len: s.len, reps: s.reps };
                        at += s.len * s.reps as f64;
                        r
                    })
                    .collect();
            }
        }
        // before the first stage: the hook as it was, a soft gated rhythm
        let lead = stages.first().map(|s| s.at).unwrap_or(total);
        if lead >= 1.0 {
            m.word(hook_word, b0, 0.7, false, false, 2.0);
            let kicks = steps_in(c.drums, &[35, 36], 0.0);
            let snares = steps_in(c.drums, &[38, 40], 0.0);
            let mut t = b0;
            while t < b0 + lead - 0.5 {
                let avail = (b0 + lead - t).min(4.0);
                let taken = [(t, t + 1.0)];
                let tk: &[(f64, f64)] = if t == b0 { &taken[..] } else { &[] };
                gated(&mut m, &mut rng, t, avail, 3, tk, &kicks, &snares, &pool, &mut cursor, 0.5, &mut out.fills);
                t += 4.0;
            }
        }
        let last_two = stages.len().saturating_sub(2);
        let riser_reps: usize = stages.iter().skip(last_two).filter(|s| s.len <= 0.25 + 1e-9).map(|s| s.reps).sum();
        let mut riser_i = 0usize;
        for (si, st) in stages.iter().enumerate() {
            let ev = loop_events(v, lline, st.len);
            for r in 0..st.reps {
                let base = b0 + st.at + r as f64 * st.len;
                let prog = ((base - b0) / bl).clamp(0.0, 1.0) as f32;
                for (k, (off, pi, len, part)) in ev.iter().enumerate() {
                    let at = base + off;
                    let vel = (0.5 + 0.45 * prog + if k == 0 { 0.05 } else { 0.0 }).min(1.0);
                    let pitch = m.pitch(*pi, at);
                    // the riser bends on top of the tuning (an effect, not counted against the shift cap)
                    let mut bend = 0;
                    if o.riser && si >= last_two && st.len <= 0.25 + 1e-9 && m.v.pieces[*pi].chop.midi.is_some() {
                        bend = (12.0 * riser_i as f64 / riser_reps.max(1) as f64).round() as i32;
                    }
                    let trim = if *part { Some(*len * 0.95) } else { None };
                    m.put(*pi, at, *len * 0.95, vel, pitch, bend, trim, false);
                }
                if st.len <= 0.25 + 1e-9 {
                    riser_i += 1;
                }
            }
            out.stutters += (st.len <= 0.5 + 1e-9) as usize;
            out.stages.push(Stage { at: b0 + st.at, len: st.len, reps: st.reps });
        }
        // the high-pass: closed until the loops start, then climbing with them; open again on the drop
        out.hp.push((b0, 0.0));
        let first_at = stages.first().map(|s| s.at).unwrap_or(0.0);
        out.hp.push((b0 + first_at, 0.15));
        for st in stages.iter().skip(1) {
            let f = ((st.at - first_at) / (total - first_at).max(1e-9)).clamp(0.0, 1.0) as f32;
            out.hp.push((b0 + st.at, 0.15 + 0.7 * f));
        }
        out.hp.push((c.beats - 0.02, 0.9));
        out.hp.push((c.beats, 0.0));
    } else if c.next_hot && o.reverse && c.beats >= 2.0 {
        // a reverse swell of the hook word, ending on the downbeat of the drop
        if let Some((_, rc)) = v.rev.iter().max_by_key(|(_, rc)| rc.end - rc.start) {
            let dur = (rc.end - rc.start) as f64 / v.rate.max(1) as f64 * v.bpm / 60.0;
            let swell = dur.min(2.0).min(c.beats);
            let skip = ((dur - swell) * 60.0 / v.bpm * v.rate as f64) as u32;
            let mut r = rc.clone();
            r.start = (r.start + skip).min(r.end.saturating_sub(1));
            let at = c.beats - swell;
            // the swell is tuned to what sounds where it starts, like any chop
            let pitch = match r.midi {
                Some(f) => m.tune.nearest(f, &m.tune.tones(m.key, m.harmony, at)),
                None => 60,
            };
            let (n, shift) = tuned_note(&r, at, swell, pitch, 0.9, &m.tune, 0);
            m.main.push(n);
            m.stats.note(shift, false);
            out.swells += 1;
        }
    }
    m.main.sort_by(|a, b| a.start.total_cmp(&b.start));
    m.layer.sort_by(|a, b| a.start.total_cmp(&b.start));
    out.main = m.main;
    out.layer = m.layer;
    out.tune = m.stats;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::Instrument;

    fn tone(f: f32, secs: f32, rate: u32) -> Vec<f32> {
        let n = (secs * rate as f32) as usize;
        (0..n)
            .flat_map(|i| {
                let x = (i as f32 / rate as f32 * f * std::f32::consts::TAU).sin() * 0.5 * (1.0 - (i as f32 / n as f32 - 0.5).abs() * 0.6);
                [x, x]
            })
            .collect()
    }

    fn vocal(rate: u32) -> Vec<f32> {
        // six sung "syllables" with gaps and a burst of noise
        let mut v = vec![];
        for (f, s) in [(440.0, 0.3), (528.0, 0.25), (329.6, 0.35), (392.0, 0.2), (493.9, 0.3), (293.7, 0.4)] {
            v.extend(tone(f, s, rate));
            v.extend(vec![0.0; (rate as f32 * 0.15) as usize * 2]);
        }
        let mut r = Rng::new(3);
        v.extend((0..(rate as usize / 5)).flat_map(|_| {
            let x = ((r.next() % 2000) as f32 / 1000.0 - 1.0) * 0.4;
            [x, x]
        }));
        v
    }

    #[test]
    fn transients_are_found_and_any_number_of_chops_is_taken_at_random() {
        let rate = 44100;
        let v = vocal(rate);
        let tr = transients(&v, rate);
        assert!(tr.len() >= 7, "{tr:?}");
        // the strongest: as many as asked, pitched where sung, unpitched for the noise
        let best = pick_chops(&v, rate, &Opts { count: 7, length: 0.2, vary: 0.0, random: false }, 1);
        assert_eq!(best.len(), 7);
        assert!(best.iter().filter(|c| c.midi.is_some()).count() >= 5 && best.iter().any(|c| c.midi.is_none()), "{best:?}");
        let a4 = best.iter().find(|c| c.start < rate / 10).unwrap();
        assert!((a4.midi.unwrap() - 69.0).abs() < 0.3);
        // at random: another seed, other chops; lengths vary when asked; ends fall in quiet places
        let r1 = pick_chops(&v, rate, &Opts { count: 3, length: 0.15, vary: 1.0, random: true }, 1);
        let r2 = pick_chops(&v, rate, &Opts { count: 3, length: 0.15, vary: 1.0, random: true }, 2);
        assert_eq!(r1.len(), 3);
        assert_ne!(r1, r2);
        let lens: Vec<u32> = (1..9).flat_map(|s| pick_chops(&v, rate, &Opts { count: 3, length: 0.15, vary: 1.0, random: true }, s)).map(|c| c.end - c.start).collect();
        let (mn, mx) = (lens.iter().min().unwrap(), lens.iter().max().unwrap());
        assert!(*mx as f32 > *mn as f32 * 1.8, "{mn}..{mx}");
        // a slice lyric plays the chop at the note's pitch
        for c in best.iter().filter(|c| c.midi.is_some()) {
            let (_, _, cents) = crate::dsp::sampler::parse_slice(&slice_lyric(c)).unwrap();
            for p in [55, 64, 72] {
                let heard = c.midi.unwrap() + (p as f32 - 60.0) + cents / 100.0;
                assert!((heard - p as f32).abs() < 0.01);
            }
        }
    }

    #[test]
    fn a_slice_fades_in_and_out_and_never_clicks() {
        let rate = 44100;
        let v = tone(220.0, 1.0, rate);
        let mut s = crate::dsp::sampler::Sampler::new(rate as f32);
        let mut z = Zone::new("v", "");
        z.root = 60;
        s.add_loaded(z, v, rate);
        let mut out = vec![0f32; 2 * 22050];
        s.note_on_lyric(60, 1.0, 0.0, 0.5, 0.5, "@4410:13230:0", 1.0);
        s.render(&mut out);
        let l: Vec<f32> = out.chunks(2).map(|f| f[0].abs()).collect();
        // 200 ms of slice: sound in the middle, nothing after it, and the last 2 ms before the end already faded
        assert!(l[2000..6000].iter().copied().fold(0.0, f32::max) > 0.2);
        assert!(l[9000..].iter().all(|x| *x < 1e-6));
        let tail = l[8800..8820].iter().copied().fold(0.0, f32::max);
        assert!(tail < 0.05, "{tail}");
        assert!(l[0] < 0.02, "faded in");
        // an ordinary note still plays the whole zone
        let mut out2 = vec![0f32; 2 * 22050];
        s.all_off();
        s.note_on_lyric(60, 1.0, 0.0, 0.5, 0.5, "", 1.0);
        s.render(&mut out2);
        assert!(out2[2 * 15000..2 * 15100].iter().any(|x| x.abs() > 0.1));
    }

    #[test]
    fn chops_are_composed_around_the_drums_in_the_key() {
        let rate = 44100;
        let v = vocal(rate);
        let chops = pick_chops(&v, rate, &Opts { count: 6, length: 0.2, vary: 0.5, random: false }, 4);
        let drums: Vec<Note> = (0..8).flat_map(|b| vec![Note::new(b as f64 * 2.0, 0.2, 36, 0.9), Note::new(b as f64 * 2.0 + 1.0, 0.2, 38, 0.8)]).collect();
        let harmony: Vec<Note> = vec![Note::new(0.0, 8.0, 45, 0.7), Note::new(8.0, 8.0, 41, 0.7)];
        let ctx = Ctx { beats: 16.0, key: (9, true), energy: 0.85, drums: &drums, harmony: &harmony, lead: &[], style: Style::Chops, seed: 5, chops: &chops };
        let (main, _layer) = compose(&ctx);
        assert!(main.len() >= 12, "{}", main.len());
        let key = Key::new(9, true);
        for n in &main {
            let sl = crate::dsp::sampler::parse_slice(&n.lyric).expect("every chop note carries its slice");
            let c = chops.iter().find(|c| c.start == sl.0).unwrap();
            if c.midi.is_some() {
                assert!(key.in_scale(n.key % 12), "chop at {} is out of A minor", n.key);
            }
            assert!(n.start < 16.0);
        }
        assert!(main.windows(2).any(|w| (w[1].start - w[0].start - 0.125).abs() < 1e-6), "a stutter in 1/32s");
        let quiet = compose(&Ctx { energy: 0.2, ..ctx });
        assert!(quiet.0.len() < main.len() / 2 && quiet.1.is_empty());
        // other seeds use other chops of the pool
        let used: std::collections::BTreeSet<String> = (0..12).flat_map(|s| compose(&Ctx { seed: s, energy: 0.85, ..ctx }).0.into_iter().map(|n| n.lyric)).collect();
        assert!(used.len() >= 4, "{}", used.len());
        // breaks: double time at 85 bpm, a roll at the end, nothing when it is quiet
        let br = breaks(16.0, 0.9, 85.0, 1);
        assert!(br.iter().filter(|n| n.key == 38 && n.start >= 15.0).count() >= 8);
        assert!(br.iter().all(|n| n.start < 16.0));
        assert!(breaks(16.0, 0.2, 85.0, 1).is_empty());
    }

    #[test]
    fn ling_voice_sings_syllables_to_chop() {
        let v = sung_syllables(44100.0, 0);
        assert!(v.len() > 44100 * 2 * 4);
        assert!(pick_chops(&v, 44100, &Opts::default(), 1).len() >= 6);
    }
}

#[cfg(test)]
mod edm_tests {
    use super::*;

    const BPM: f64 = 120.0;
    const RATE: u32 = 44100;

    /// the score of a short vocal: (beat, beats, key, written syllable)
    fn score() -> Vec<VNote> {
        let s: [(f64, f64, u8, &str); 10] = [
            (0.0, 0.5, 64, " I"),
            (0.5, 0.5, 67, " need"),
            (1.0, 0.5, 64, " your"),
            (1.5, 1.5, 69, " love"),
            (4.0, 0.75, 67, " fal"),
            (4.75, 0.75, 64, "ling"),
            (5.5, 0.5, 62, " in"),
            (6.0, 1.5, 69, " love"),
            (7.5, 0.5, 69, "~"),
            (10.0, 1.0, 72, " hey"),
        ];
        s.iter().map(|(a, l, k, t)| VNote::at(*a, *l, *k, t, BPM, 0.0, 0.0)).collect()
    }

    /// a tone per syllable that is silent for the last 40 ms of its note
    fn sung(notes: &[VNote]) -> Vec<f32> {
        let end = notes.iter().map(|n| n.s1).fold(0.0, f64::max) + 0.5;
        let frames = (end * RATE as f64) as usize;
        let mut out = vec![0f32; frames * 2];
        for u in &units(notes) {
            let (a, b) = ((u.s0 * RATE as f64) as usize, ((u.s1 - 0.04) * RATE as f64) as usize);
            let hz = 440.0 * 2f64.powf((u.key as f64 - 69.0) / 12.0);
            for f in a..b.min(frames) {
                let t = (f - a) as f64 / RATE as f64;
                let tail = (b - f) as f64 / RATE as f64;
                let env = (t / 0.01).min(1.0).min(tail / 0.02);
                let x = ((t * hz * std::f64::consts::TAU).sin() * 0.5 * env) as f32;
                out[f * 2] = x;
                out[f * 2 + 1] = x;
            }
        }
        out
    }

    fn vocal_of(opts: &CutOpts) -> (Vocal, Vec<f32>, Vec<VNote>) {
        let n = score();
        let a = sung(&n);
        let v = Vocal::from_notes(&a, RATE, &n, BPM, opts, &[]);
        (v, a, n)
    }

    fn parse(l: &str) -> u32 {
        crate::dsp::sampler::parse_slice(l).map(|x| x.0).unwrap_or(0)
    }

    #[test]
    fn syllables_become_chops_that_land_on_the_note_edges() {
        let (v, audio, notes) = vocal_of(&CutOpts { pre_ms: 0.0, snap: true });
        assert_eq!(v.pieces.len(), 9, "{:?}", v.pieces.iter().map(|p| p.text.clone()).collect::<Vec<_>>());
        assert_eq!(v.word_text, vec!["i", "need", "your", "love", "falling", "in", "love", "hey"]);
        let us = units(&notes);
        for (p, u) in v.pieces.iter().zip(&us) {
            let (s, e) = (p.chop.start as f64 / RATE as f64, p.chop.end as f64 / RATE as f64);
            assert!((s - u.s0).abs() < 0.012, "{} starts {s} for {}", p.text, u.s0);
            assert!((e - u.s1).abs() < 0.012, "{} ends {e} for {}", p.text, u.s1);
            // never through the middle of a sound: both edges are quiet
            for f in [p.chop.start as usize, p.chop.end as usize - 1] {
                assert!(audio[f * 2].abs() < 0.06, "{} cut at {f} in a sound ({})", p.text, audio[f * 2]);
            }
        }
        assert!(v.pieces.iter().filter(|p| p.chop.midi.is_some()).count() >= 7);
        for w in v.pieces.windows(2) {
            assert!(w[0].chop.end <= w[1].chop.start, "chops do not overlap");
        }
        // the held note lengthens the "love" before it
        assert!(v.pieces[7].len > 1.9);
        // a bit of pre-roll is still on the edge
        let (v2, _, _) = vocal_of(&CutOpts::default());
        for (p, u) in v2.pieces.iter().zip(&us) {
            assert!((p.chop.start as f64 / RATE as f64 - u.s0).abs() < 0.02);
        }
    }

    #[test]
    fn a_note_with_two_syllables_is_cut_in_two_and_words_follow_the_spaces() {
        let n = vec![VNote::at(0.0, 2.0, 60, " ly-ric", BPM, 0.0, 0.0), VNote::at(2.0, 1.0, 62, "~", BPM, 0.0, 0.0), VNote::at(3.0, 1.0, 64, " ok", BPM, 0.0, 0.0)];
        let u = units(&n);
        assert_eq!(u.len(), 3);
        assert_eq!((u[0].text.as_str(), u[0].new_word), ("ly", true));
        assert_eq!((u[1].text.as_str(), u[1].new_word), ("ric", false));
        assert!((u[0].start + u[0].len - u[1].start).abs() < 1e-9 && (u[1].start + u[1].len - 3.0).abs() < 1e-9, "the hold lengthens the last syllable");
        assert!(u[2].new_word);
        // unwritten (IPA) lyrics: every note is a word
        let ipa = vec![VNote::at(0.0, 1.0, 60, "ha", BPM, 0.0, 0.0), VNote::at(1.0, 1.0, 60, "lo", BPM, 0.0, 0.0)];
        assert!(units(&ipa).iter().all(|u| u.new_word));
    }

    #[test]
    fn stress_and_the_hook_come_from_the_words() {
        let (v, _, _) = vocal_of(&CutOpts::default());
        assert!(v.pieces[4].stress > v.pieces[5].stress, "first syllable of falling is stressed");
        assert!(v.pieces[3].stress > v.pieces[0].stress, "love beats I");
        assert_eq!(v.word_text[v.hook_word.unwrap()], "love");
        assert!(v.hook_text().contains("love"));
        assert_eq!(v.lines.len(), 3);
        // the chorus weighs a word more
        let hot = Vocal::from_notes(&sung(&score()), RATE, &score(), BPM, &CutOpts::default(), &[(9.0, 12.0), (9.5, 12.0), (9.9, 12.0)]);
        assert!(hot.hook_word.is_some());
    }

    #[test]
    fn transients_stand_in_when_nothing_is_written() {
        let a = sung(&score());
        let v = Vocal::from_audio(&a, RATE, BPM, 0.0, &[]);
        assert!(v.pieces.len() >= 7, "{}", v.pieces.len());
        assert!(v.hook_line.is_some() && v.hook_word.is_some());
        for w in v.pieces.windows(2) {
            assert!(w[0].chop.end <= w[1].chop.start + 8, "no overlap");
        }
    }

    fn drums_floor(beats: f64) -> Vec<Note> {
        let mut d = vec![];
        let mut t = 0.0;
        while t < beats - 1e-6 {
            d.push(Note::new(t, 0.2, 36, 0.9));
            if (t % 2.0 - 1.0).abs() < 1e-6 {
                d.push(Note::new(t, 0.2, 38, 0.9));
            }
            t += 1.0;
        }
        d
    }

    fn slice_in_a_piece(v: &Vocal, n: &Note) -> bool {
        let Some((a, b, _)) = crate::dsp::sampler::parse_slice(&n.lyric) else { return false };
        v.pieces.iter().any(|p| a == p.chop.start && b <= p.chop.end) || v.rev.iter().any(|(_, c)| a >= c.start && b <= c.end)
    }

    #[test]
    fn a_drop_has_a_dry_hit_a_gated_rhythm_off_the_kicks_and_a_stutter_into_the_word() {
        let (mut v, audio, _) = vocal_of(&CutOpts::default());
        v.add_reverse(&audio);
        let drums = drums_floor(32.0);
        let harmony = vec![Note::new(0.0, 16.0, 45, 0.7), Note::new(16.0, 16.0, 41, 0.7)];
        let o = Edm { stutter: 0.8, density: 0.8, ..Edm::default() };
        let ctx = EdmCtx { beats: 32.0, key: (9, true), energy: 0.9, tag: "drop", drums: &drums, harmony: &harmony, vocal: &v, bpm: BPM, seed: 3, o: &o, build_beats: 0.0, next_hot: false, loop_line: None, only_swell: false };
        let out = compose_edm(&ctx);
        assert!(out.main.len() >= 30, "{}", out.main.len());
        // the dry hit on the downbeat
        assert!(out.dry && out.main[0].start == 0.0 && out.main[0].vel >= 0.99);
        // every note plays one syllable, or part of one: never across a boundary
        for n in out.main.iter().chain(out.layer.iter()) {
            assert!(slice_in_a_piece(&v, n), "{n:?}");
            assert!(n.start < 32.0 && n.start + n.len <= 32.0 + 1e-6);
        }
        // the gated rhythm keeps off the kicks (four on the floor) and is plentiful
        assert!(out.fills.len() >= 12, "{}", out.fills.len());
        for t in &out.fills {
            let step = ((t % 4.0) / 0.25).round() as i64 % 16;
            assert!(![0, 4, 8, 12].contains(&step), "a chop on a kick at {t}");
        }
        // pitched chops sit in A minor (the dry hit is the sung pitch)
        let key = Key::new(9, true);
        for n in out.main.iter().filter(|n| n.vel < 0.99) {
            let (a, _, _) = crate::dsp::sampler::parse_slice(&n.lyric).unwrap();
            if v.pieces.iter().find(|p| p.chop.start == a).map(|p| p.chop.midi.is_some()).unwrap_or(false) {
                assert!(key.in_scale(n.key % 12), "{} is out of A minor", n.key);
            }
        }
        // a stutter: three retriggers of one syllable on the 1/16 grid, then the word starting on its beat
        let m = &out.main;
        let found = (0..m.len().saturating_sub(4)).any(|i| {
            let s0 = parse(&m[i].lyric);
            (0..3).all(|j| parse(&m[i + j].lyric) == s0 && (m[i + j + 1].start - m[i + j].start - 0.25).abs() < 1e-6) && parse(&m[i + 3].lyric) == s0 && (m[i + 3].start % 1.0).abs() < 1e-6
        });
        assert!(found, "a fa-fa-fa-falling stutter");
        assert!(out.stutters >= 2);
        // and a 1/32 stutter closes a four-bar phrase
        assert!(m.windows(2).any(|w| (w[1].start - w[0].start - 0.125).abs() < 1e-6));
        // a quiet section is far thinner
        let calm = compose_edm(&EdmCtx { energy: 0.3, tag: "verse", ..ctx });
        assert!(calm.main.len() * 4 < out.main.len(), "{} vs {}", calm.main.len(), out.main.len());
    }

    #[test]
    fn a_build_repeats_the_last_phrase_shorter_and_shorter_and_goes_quiet_before_the_drop() {
        let (mut v, audio, _) = vocal_of(&CutOpts::default());
        v.add_reverse(&audio);
        let last = v.lines.len() - 1;
        let drums = drums_floor(32.0);
        let o = Edm::default();
        let ctx = EdmCtx { beats: 32.0, key: (9, true), energy: 0.8, tag: "build", drums: &drums, harmony: &[], vocal: &v, bpm: BPM, seed: 1, o: &o, build_beats: 0.0, next_hot: true, loop_line: Some(last), only_swell: false };
        let out = compose_edm(&ctx);
        let lens: Vec<f64> = out.stages.iter().map(|s| s.len).collect();
        assert_eq!(lens, vec![4.0, 2.0, 1.0, 0.5, 0.25, 0.125]);
        // the silence before the drop
        assert!((out.silence - 0.5).abs() < 1e-9);
        assert!(out.main.iter().all(|n| n.start < 32.0 - out.silence + 1e-6), "nothing in the silence");
        assert!(out.main.iter().any(|n| n.start > 32.0 - out.silence - 2.0), "the last stage plays");
        // each stage is on its grid, and a rep repeats the one before it
        for st in &out.stages {
            let inside: Vec<&Note> = out.main.iter().filter(|n| n.start >= st.at - 1e-6 && n.start < st.at + st.len * st.reps as f64 - 1e-6).collect();
            assert!(!inside.is_empty());
            for n in &inside {
                let r = ((n.start - st.at) / 0.125).round() * 0.125;
                assert!((n.start - st.at - r).abs() < 1e-6, "off the 1/32 grid at {}", n.start);
                assert!(n.len <= st.len + 1e-6, "a note longer than its loop at {}", n.start);
            }
            if st.len <= 0.5 {
                for n in &inside {
                    let rel = (n.start - st.at) % st.len;
                    assert!(rel < 1e-6 || (st.len - rel) < 1e-6, "{} off the loop grid of {}", n.start, st.len);
                }
            }
            if st.reps >= 2 && st.len >= 1.0 {
                let a: Vec<u32> = inside.iter().filter(|n| n.start < st.at + st.len).map(|n| parse(&n.lyric)).collect();
                let b: Vec<u32> = inside.iter().filter(|n| n.start >= st.at + st.len && n.start < st.at + 2.0 * st.len).map(|n| parse(&n.lyric)).collect();
                assert_eq!(a, b, "the loop repeats");
            }
        }
        // it is the last phrase that repeats
        let ends: Vec<u32> = (v.lines[last].first..v.lines[last].end).map(|i| v.pieces[i].chop.start).collect();
        for n in &out.main {
            if n.start >= out.stages[0].at {
                assert!(ends.contains(&parse(&n.lyric)));
            }
        }
        // it gets louder, and the high-pass opens until it snaps back on the drop
        let q = 32.0 / 4.0;
        let avg = |a: f64, b: f64| {
            let v: Vec<f32> = out.main.iter().filter(|n| n.start >= a && n.start < b).map(|n| n.vel).collect();
            v.iter().sum::<f32>() / v.len().max(1) as f32
        };
        assert!(avg(32.0 - q, 32.0) > avg(out.stages[0].at, out.stages[0].at + q) + 0.05);
        let hp = &out.hp;
        assert!(hp.windows(2).take(hp.len() - 2).all(|w| w[1].1 >= w[0].1 - 1e-6), "{hp:?}");
        assert_eq!(*hp.last().unwrap(), (32.0, 0.0));
        assert!(hp[hp.len() - 2].1 > 0.8);
        // a build is silent before the drop (no swell); a calm section ahead of a drop swells into it
        assert_eq!(out.swells, 0);
        let brk = compose_edm(&EdmCtx { beats: 8.0, tag: "break", energy: 0.3, ..ctx });
        assert_eq!(brk.swells, 1, "a break swells into the drop");
        let calm = compose_edm(&EdmCtx { beats: 8.0, tag: "verse", energy: 0.3, build_beats: 0.0, o: &Edm { build: Build::Off, ..Edm::default() }, ..ctx });
        assert_eq!(calm.swells, 1);
        let sw = calm.main.last().unwrap();
        assert!((sw.start + sw.len - 8.0).abs() < 1e-6 && slice_in_a_piece(&v, sw), "the reverse chop ends on the downbeat: {sw:?}");
        // a short build and an optional riser still end on the beat
        let short = compose_edm(&EdmCtx { beats: 8.0, build_beats: 0.0, tag: "build", o: &Edm { riser: true, ..Edm::default() }, ..ctx });
        assert!(short.main.iter().all(|n| n.start < 8.0 - short.silence + 1e-6));
        let tail: Vec<i32> = short.main.iter().filter(|n| n.start >= 6.0).map(|n| n.key as i32).collect();
        assert!(tail.last().unwrap() > tail.first().unwrap(), "the riser bends up: {tail:?}");
    }

    /// a voice-like take of the score: a few harmonics per syllable, each syllable `detune[i]` semitones off its note, an "s" (noise)
    /// before the last word
    fn sung_off(notes: &[VNote], detune: &[f32]) -> Vec<f32> {
        let end = notes.iter().map(|n| n.s1).fold(0.0, f64::max) + 0.5;
        let frames = (end * RATE as f64) as usize;
        let mut out = vec![0f32; frames * 2];
        let us = units(notes);
        for (i, u) in us.iter().enumerate() {
            let (a, b) = ((u.s0 * RATE as f64) as usize, ((u.s1 - 0.04) * RATE as f64) as usize);
            let hz = 440.0 * 2f64.powf((u.key as f64 + detune[i % detune.len()] as f64 - 69.0) / 12.0);
            let last = i + 1 == us.len();
            for f in a..b.min(frames) {
                let t = (f - a) as f64 / RATE as f64;
                let tail = (b - f) as f64 / RATE as f64;
                let env = (t / 0.01).min(1.0).min(tail / 0.02);
                let ph = t * hz * std::f64::consts::TAU;
                let x = if last && t < 0.12 {
                    // "s": noise
                    (((f as u64).wrapping_mul(2862933555777941757).wrapping_add(3037000493) >> 33) as f64 / (1u64 << 31) as f64 - 1.0) * 0.3 * env
                } else {
                    (ph.sin() * 0.5 + (2.0 * ph).sin() * 0.22 + (3.0 * ph).sin() * 0.1) * env * 0.8
                };
                out[f * 2] = x as f32;
                out[f * 2 + 1] = x as f32;
            }
        }
        out
    }

    /// the pitch heard when a chop note is played by the chop sampler (root 60) over `bank`
    fn heard(s: &mut crate::dsp::sampler::Sampler, n: &Note) -> Option<f32> {
        use crate::dsp::Instrument;
        let (a, b, cents) = crate::dsp::sampler::parse_slice(&n.lyric)?;
        let ratio = 2f64.powf((n.key as f64 - 60.0 + cents as f64 / 100.0) / 12.0);
        let frames = ((b - a) as f64 / ratio) as usize;
        s.all_off();
        let mut sink = vec![0f32; 1024];
        for _ in 0..8 {
            s.render(&mut sink);
        }
        s.note_on_lyric(n.key, 0.9, 0.0, 0.5, 0.5, &n.lyric, 1.0);
        let mut out: Vec<f32> = vec![];
        let mut buf = vec![0f32; 512];
        while out.len() < frames {
            buf.fill(0.0);
            s.render(&mut buf);
            out.extend(buf.chunks(2).map(|f| (f[0] + f[1]) * 0.5));
        }
        out.truncate(frames);
        chop_f0(&out, 0, out.len(), RATE).map(|f| f.midi)
    }

    fn chop_sampler(bank: &[f32]) -> crate::dsp::sampler::Sampler {
        let mut s = crate::dsp::sampler::Sampler::new(RATE as f32);
        let mut z = Zone::new("v", "");
        z.root = 60;
        s.add_loaded(z, bank.to_vec(), RATE);
        s
    }

    #[test]
    fn tuned_chops_sound_on_the_chord_tones_and_unvoiced_ones_as_recorded() {
        let notes = score();
        // the singer is off: sharp, flat, a semitone out of the key
        let detune = [0.32f32, -0.41, 0.18, -0.27, 0.45, 1.0, -0.36, 0.22, 0.0];
        let audio = sung_off(&notes, &detune);
        let mut v = Vocal::from_notes(&audio, RATE, &notes, BPM, &CutOpts::default(), &[]);
        let bank = v.add_reverse(&audio);
        // the measured pitch of each syllable is what was sung (the "s" before "hey" is part of its chop: the vowel counts)
        for (i, p) in v.pieces.iter().enumerate() {
            let m = p.chop.midi.unwrap_or_else(|| panic!("{} has a pitch", p.text));
            assert!((m - (p.key as f32 + detune[i])).abs() < 0.08, "{}: {m} for {}", p.text, p.key as f32 + detune[i]);
        }
        // a drop over Am | F (bass + pad)
        let drums = drums_floor(32.0);
        let mut harmony = vec![];
        for (t0, root) in [(0.0, 45u8), (16.0, 41u8)] {
            harmony.push(Note::new(t0, 16.0, root, 0.7));
            for k in if root == 45 { [0, 3, 7] } else { [0, 4, 7] } {
                harmony.push(Note::new(t0, 16.0, root + 12 + k, 0.5));
            }
        }
        let o = Edm { stutter: 0.8, density: 0.8, layer: true, ..Edm::default() };
        let ctx = EdmCtx { beats: 32.0, key: (9, true), energy: 0.95, tag: "drop", drums: &drums, harmony: &harmony, vocal: &v, bpm: BPM, seed: 7, o: &o, build_beats: 0.0, next_hot: false, loop_line: None, only_swell: false };
        let out = compose_edm(&ctx);
        assert!(out.tune.tuned >= out.main.len() / 2, "{:?}", out.tune);
        let mut s = chop_sampler(&bank);
        let mut errs: Vec<f32> = vec![];
        for n in out.main.iter().chain(out.layer.iter()) {
            let (a, _, cents) = crate::dsp::sampler::parse_slice(&n.lyric).unwrap();
            let piece = v.pieces.iter().find(|p| p.chop.start == a).unwrap();
            let f0 = piece.chop.midi.unwrap();
            // a static shift, never more than 5 semitones
            let moved = n.key as f32 - 60.0 + cents / 100.0;
            assert!((f0 + moved - n.key as f32).abs() < 0.011, "fine: the note's key is the pitch heard ({moved})");
            assert!((n.key as f32 - f0).abs() <= 5.0 + 1e-3, "a shift over the cap: {} from {f0}", n.key);
            // on a chord tone (the dry hit: the note that was written, which is one here too)
            let chord = chord_at(Key::new(9, true), &harmony, n.start).unwrap();
            assert!(chord.contains(&(n.key % 12)), "{} at {} is not in {chord:?}", n.key, n.start);
            // and that is what the sampler plays
            if let Some(h) = heard(&mut s, n) {
                errs.push((h - n.key as f32).abs() * 100.0);
            }
        }
        errs.sort_by(f32::total_cmp);
        assert!(errs.len() >= 20, "{}", errs.len());
        let worst = *errs.last().unwrap();
        println!("drop: {} rendered chop notes, error median {:.1} cents, worst {worst:.1}; tuning {:?}", errs.len(), errs[errs.len() / 2], out.tune);
        assert!(worst < 15.0, "rendered chops within 15 cents of their notes: median {} worst {worst}", errs[errs.len() / 2]);
        // without tuning every chop plays as sung
        let off = Edm { tune: Tune::off(), ..o.clone() };
        let raw = compose_edm(&EdmCtx { o: &off, ..ctx });
        assert_eq!(raw.tune.tuned, 0);
        for n in &raw.main {
            let (a, _, cents) = crate::dsp::sampler::parse_slice(&n.lyric).unwrap();
            let f0 = v.pieces.iter().find(|p| p.chop.start == a).map(|p| p.chop.midi.unwrap()).unwrap();
            assert!((n.key as f32 - 60.0 + cents / 100.0).abs() < 0.006, "{f0}: as sung");
        }
    }

    #[test]
    fn an_unvoiced_chop_is_never_tuned() {
        let mut nz = vec![0f32; 2 * RATE as usize / 2];
        let mut r = Rng::new(4);
        for x in nz.iter_mut() {
            *x = (r.f() as f32 * 2.0 - 1.0) * 0.3;
        }
        let mono: Vec<f32> = nz.chunks(2).map(|f| f[0]).collect();
        assert!(chop_f0(&mono, 0, mono.len(), RATE).is_none());
        let c = Chop { start: 0, end: 10000, midi: None, level: 0.3 };
        for t in [Tune::default(), Tune::parse("key", None, None), Tune::off()] {
            let (n, shift) = tuned_note(&c, 0.0, 0.5, 67, 0.8, &t, 0);
            assert_eq!((n.key, n.lyric.as_str(), shift), (60, "@0:10000:0", None));
        }
    }

    #[test]
    fn medley_chops_on_their_call_and_response_notes_hit_them() {
        // two sung chops a little off pitch, composed (the medley's cell melody on chord tones) and rendered
        let notes = vec![VNote::at(0.0, 1.0, 64, " ah", BPM, 0.0, 0.0), VNote::at(2.0, 1.0, 69, " oh", BPM, 0.0, 0.0), VNote::at(4.0, 1.0, 76, " ey", BPM, 0.0, 0.0)];
        let audio = sung_off(&notes, &[0.37, -0.3, 0.12]);
        let v = Vocal::from_notes(&audio, RATE, &notes, BPM, &CutOpts::default(), &[]);
        let chops: Vec<Chop> = v.pieces.iter().map(|p| p.chop.clone()).collect();
        assert!(chops.iter().all(|c| c.midi.is_some()));
        let drums = drums_floor(16.0);
        let harmony = vec![Note::new(0.0, 8.0, 45, 0.7), Note::new(8.0, 8.0, 41, 0.7)];
        let mut s = chop_sampler(&audio);
        let mut errs = vec![];
        for seed in 0..4 {
            let (main, layer) = compose(&Ctx { beats: 16.0, key: (9, true), energy: 0.85, drums: &drums, harmony: &harmony, lead: &[], style: Style::Chops, seed, chops: &chops });
            for n in main.iter().chain(layer.iter()) {
                let (a, _, _) = crate::dsp::sampler::parse_slice(&n.lyric).unwrap();
                let f0 = chops.iter().find(|c| c.start == a).unwrap().midi.unwrap();
                assert!((n.key as f32 - f0).abs() <= 5.0 + 1e-3, "{} from {f0}", n.key);
                assert!(Key::new(9, true).in_scale(n.key % 12));
                if n.len >= 0.2 {
                    if let Some(h) = heard(&mut s, n) {
                        errs.push((h - n.key as f32).abs() * 100.0);
                    }
                }
            }
        }
        errs.sort_by(f32::total_cmp);
        println!("medley: {} rendered chop notes, error median {:.1} cents, worst {:.1}", errs.len(), errs[errs.len() / 2], errs.last().unwrap());
        assert!(errs.len() >= 20 && *errs.last().unwrap() < 15.0, "{errs:?}");
    }

    #[test]
    fn a_build_at_the_end_of_a_verse_keeps_the_verse_before_it() {
        let (v, _, _) = vocal_of(&CutOpts::default());
        let drums = drums_floor(32.0);
        let o = Edm::default();
        let ctx = EdmCtx { beats: 32.0, key: (9, true), energy: 0.5, tag: "verse", drums: &drums, harmony: &[], vocal: &v, bpm: BPM, seed: 1, o: &o, build_beats: 8.0, next_hot: true, loop_line: None, only_swell: false };
        let out = compose_edm(&ctx);
        assert!(out.stages.iter().all(|s| s.at >= 24.0 - 1e-9));
        assert!(out.main.iter().any(|n| n.start < 24.0), "the verse part");
        assert_eq!(out.hp.first().unwrap().0, 24.0);
    }
}
