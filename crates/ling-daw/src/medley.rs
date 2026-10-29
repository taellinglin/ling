//! Make Medley: a folder of MIDI files chopped into phrases and put back together as one piece.
//!
//! * Chop: every file's notes are cut at phrase boundaries (`articulate::phrase_map`: rests, repeats, the bar grid; or a fixed
//!   number of bars). Each source track is a part - lead, chords, bass or drums (channel 10 / percussive, register, polyphony).
//! * Fit: every phrase is moved into one key (the most common tonic and mode; a phrase in the other mode goes to its relative) and
//!   plays at one tempo; drums are not transposed.
//! * Order:
//!   - smart: phrases chain where the melody leads well into the next (the jump from one phrase's last lead note to the next's first),
//!     the density follows an arc (quiet start, peak about two thirds in, settle), files alternate, nothing repeats until all played;
//!   - random: a seeded shuffle;
//!   - cipher: a permutation keyed by a passphrase through HMAC-SHA-256 - the same folder and passphrase always give the same medley,
//!     another passphrase another one. With a message, the message itself is written in the order: its bytes become base-N digits
//!     (N = the number of phrases), each digit a phrase through the keyed permutation; `decode` reads it back from the phrases' ids
//!     with the folder and the passphrase.
//! * Mashup ([`mashup`]): a story or a feel becomes an energy and mood curve over the slots and a path on the wheel of fifths (moves a DJ
//!   mixes harmonically: a fifth up or down, the relative minor / major, a whole-step boost into a peak, home at the end). Every part is
//!   categorized (drums, bass, pad, stab, lead, arp); each slot keeps its spine phrase's hook and layers parts of other phrases by the
//!   energy (a pad alone when it is quiet, everything at the peak). Loop rolls join the slots: 4 > 2 > 1 > 1/2 bar loops into a lift,
//!   1/2 > 1 > 2 > 4 back out of a drop, with a filter contour that steps with the loop.
use crate::articulate::phrase_map;
use crate::midi_import::ImportedMidi;
use crate::model::Note;
use hmac::{Hmac, Mac};
use sha2::Sha256;

pub mod acapella;

#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub enum Part {
    Lead,
    Chords,
    Bass,
    Drums,
}
pub const PARTS: [Part; 4] = [Part::Lead, Part::Chords, Part::Bass, Part::Drums];
impl Part {
    pub fn name(self) -> &'static str {
        match self {
            Part::Lead => "Lead",
            Part::Chords => "Chords",
            Part::Bass => "Bass",
            Part::Drums => "Drums",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Phrase {
    pub file: String,
    pub index: usize,
    pub beats: f64,
    /// (part, GM program, notes relative to the phrase start)
    pub parts: Vec<(Part, u8, Vec<Note>)>,
    pub tonic: u8,
    pub minor: bool,
    pub bpm: f64,
    /// notes per beat (all parts)
    pub density: f32,
    /// the first and last lead note (else the highest part's)
    pub first: Option<u8>,
    pub last: Option<u8>,
    /// a stable fingerprint of the notes: the phrase's id in clip names (for decoding)
    pub id: u64,
}

fn classify(t: &crate::midi_import::ImportedTrack) -> Part {
    if t.channel == 9 || t.percussive {
        return Part::Drums;
    }
    if t.notes.is_empty() {
        return Part::Lead;
    }
    let mean = t.notes.iter().map(|n| n.key as f64).sum::<f64>() / t.notes.len() as f64;
    if mean < 50.0 {
        return Part::Bass;
    }
    // how many notes sound at once on average (sampled at each onset)
    let poly = t.notes.iter().map(|a| t.notes.iter().filter(|b| b.start <= a.start + 1e-6 && b.start + b.len > a.start + 1e-6).count()).sum::<usize>() as f64 / t.notes.len() as f64;
    if poly > 1.6 {
        Part::Chords
    } else {
        Part::Lead
    }
}

fn fnv(bytes: &[u8], mut h: u64) -> u64 {
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// Cut one file into phrases (`bars` 0 = found from the music, else that many bars each).
pub fn chop(m: &ImportedMidi, file: &str, bars: f64) -> Vec<Phrase> {
    let tracks: Vec<(Part, u8, &Vec<Note>)> = m.tracks.iter().filter(|t| !t.notes.is_empty()).map(|t| (classify(t), t.program.unwrap_or(0), &t.notes)).collect();
    if tracks.is_empty() {
        return vec![];
    }
    let end = tracks.iter().flat_map(|t| t.2.iter().map(|n| n.start + n.len)).fold(m.length_beats, f64::max);
    let end = (end / 4.0).ceil() * 4.0;
    // phrase boundaries from the pitched parts (the drums would make every bar alike)
    let mut onsets: Vec<(f64, f64)> = tracks.iter().filter(|t| t.0 != Part::Drums).flat_map(|t| t.2.iter().map(|n| (n.start, n.len))).collect();
    if onsets.is_empty() {
        onsets = tracks.iter().flat_map(|t| t.2.iter().map(|n| (n.start, n.len))).collect();
    }
    onsets.sort_by(|a, b| a.0.total_cmp(&b.0));
    let phrases = phrase_map(0.0, end.max(4.0), &onsets, bars);
    // the file's key
    let mut h = [0f64; 12];
    for t in tracks.iter().filter(|t| t.0 != Part::Drums) {
        for n in t.2.iter() {
            h[(n.key % 12) as usize] += n.len.max(0.1);
        }
    }
    let key = crate::theory::detect_key_hist(&h).0;
    let bpm = m.bpm.unwrap_or(120.0);
    let mut out = vec![];
    for (i, ph) in phrases.iter().enumerate() {
        let mut parts: Vec<(Part, u8, Vec<Note>)> = vec![];
        for (part, prog, notes) in &tracks {
            let sel: Vec<Note> = notes
                .iter()
                .filter(|n| n.start >= ph.b0 - 1e-6 && n.start < ph.b1 - 1e-6)
                .map(|n| {
                    let mut c = n.clone();
                    c.start = n.start - ph.b0;
                    c.len = n.len.min(ph.b1 - n.start).max(0.05);
                    c
                })
                .collect();
            if sel.is_empty() {
                continue;
            }
            // tracks of the same part merge
            match parts.iter_mut().find(|p| p.0 == *part) {
                Some(p) => p.2.extend(sel),
                None => parts.push((*part, *prog, sel)),
            }
        }
        let total: usize = parts.iter().map(|p| p.2.len()).sum();
        if total < 4 {
            continue;
        }
        for p in parts.iter_mut() {
            p.2.sort_by(|a, b| a.start.total_cmp(&b.start));
        }
        parts.sort_by_key(|p| p.0);
        let melodic = parts.iter().find(|p| p.0 == Part::Lead).or_else(|| parts.iter().find(|p| p.0 != Part::Drums));
        let first = melodic.and_then(|p| p.2.first().map(|n| n.key));
        let last = melodic.and_then(|p| p.2.last().map(|n| n.key));
        // the file, the phrase's place in it and its notes: a repeated loop is still a phrase of its own
        let mut id = fnv(file.as_bytes(), 0xcbf2_9ce4_8422_2325);
        id = fnv(&(i as u32).to_le_bytes(), id);
        for p in &parts {
            for n in &p.2 {
                id = fnv(&[p.0 as u8, n.key, ((n.start * 16.0).round() as i64 & 0xff) as u8], id);
            }
        }
        let beats = ph.b1 - ph.b0;
        out.push(Phrase { file: file.to_string(), index: i, beats, parts, tonic: key.tonic, minor: key.minor, bpm, density: total as f32 / beats.max(1.0) as f32, first, last, id });
    }
    out
}

/// the most common key of the phrases: (tonic, minor)
pub fn common_key(ph: &[Phrase]) -> (u8, bool) {
    let mut votes = [[0usize; 2]; 12];
    for p in ph {
        votes[p.tonic as usize % 12][p.minor as usize] += 1;
    }
    let mut best = (0u8, false, 0usize);
    for t in 0..12 {
        for m in 0..2 {
            if votes[t][m] > best.2 {
                best = (t as u8, m == 1, votes[t][m]);
            }
        }
    }
    (best.0, best.1)
}

/// semitones that move a phrase into the key (a minor phrase into a major key goes to its relative major and back), -6..5
pub fn shift_to(p: &Phrase, tonic: u8, minor: bool) -> i32 {
    let own = if p.minor == minor { p.tonic as i32 } else if p.minor { (p.tonic as i32 + 3) % 12 } else { (p.tonic as i32 + 9) % 12 };
    let d = (tonic as i32 - own).rem_euclid(12);
    if d > 5 {
        d - 12
    } else {
        d
    }
}

// ── the keyed stream ───────────────────────────────────────────────────────────────

/// HMAC-SHA-256(passphrase, "ling-medley" | counter) as an endless stream of u64s
struct KeyStream {
    key: Vec<u8>,
    ctr: u64,
    buf: Vec<u64>,
}
impl KeyStream {
    fn new(pass: &str) -> KeyStream {
        KeyStream { key: pass.as_bytes().to_vec(), ctr: 0, buf: vec![] }
    }
    fn next(&mut self) -> u64 {
        if self.buf.is_empty() {
            let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&self.key).expect("any key length");
            mac.update(b"ling-medley");
            mac.update(&self.ctr.to_le_bytes());
            self.ctr += 1;
            let out = mac.finalize().into_bytes();
            self.buf = out.chunks(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).rev().collect();
        }
        self.buf.pop().unwrap()
    }
    /// uniform in 0..n (rejection sampling: no bias)
    fn below(&mut self, n: u64) -> u64 {
        let lim = u64::MAX - u64::MAX % n;
        loop {
            let x = self.next();
            if x < lim {
                return x % n;
            }
        }
    }
}

/// the passphrase's permutation of 0..n (Fisher-Yates over the keyed stream)
pub fn keyed_permutation(n: usize, pass: &str) -> Vec<usize> {
    let mut ks = KeyStream::new(pass);
    let mut v: Vec<usize> = (0..n).collect();
    for i in (1..n).rev() {
        let j = ks.below(i as u64 + 1) as usize;
        v.swap(i, j);
    }
    v
}

/// a message as phrase indices: [length byte, bytes...] in base n, each digit through the passphrase's permutation
pub fn encode(message: &str, n: usize, pass: &str) -> Vec<usize> {
    if n < 2 {
        return vec![];
    }
    let mut bytes: Vec<u8> = message.as_bytes().iter().take(255).copied().collect();
    bytes.insert(0, bytes.len() as u8);
    // big number (base 256, most significant first) -> base n digits by repeated division
    let ndig = ((bytes.len() * 8) as f64 / (n as f64).log2()).ceil() as usize;
    let mut num = bytes;
    let mut digits = vec![0usize; ndig];
    for d in (0..ndig).rev() {
        let mut rem = 0u32;
        for b in num.iter_mut() {
            let cur = rem * 256 + *b as u32;
            *b = (cur / n as u32) as u8;
            rem = cur % n as u32;
        }
        digits[d] = rem as usize;
    }
    let perm = keyed_permutation(n, pass);
    digits.into_iter().map(|d| perm[d]).collect()
}

/// phrase indices (as `encode` wrote them) -> the message, with the same n and passphrase
pub fn decode(indices: &[usize], n: usize, pass: &str) -> Option<String> {
    if n < 2 || indices.is_empty() {
        return None;
    }
    let perm = keyed_permutation(n, pass);
    let mut inv = vec![0usize; n];
    for (d, p) in perm.iter().enumerate() {
        inv[*p] = d;
    }
    // base n digits -> base 256
    let mut num: Vec<u32> = vec![0];
    for i in indices {
        let d = *inv.get(*i)? as u32;
        let mut carry = d;
        for b in num.iter_mut().rev() {
            let cur = *b * n as u32 + carry;
            *b = cur % 256;
            carry = cur / 256;
        }
        while carry > 0 {
            num.insert(0, carry % 256);
            carry /= 256;
        }
    }
    let bytes: Vec<u8> = num.into_iter().map(|b| b as u8).collect();
    // the length byte is the first non-padding byte: find the split that is self-consistent
    for s in 0..bytes.len() {
        let len = bytes[s] as usize;
        if s + 1 + len == bytes.len() {
            return String::from_utf8(bytes[s + 1..].to_vec()).ok();
        }
    }
    None
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    Smart,
    Random,
    Cipher,
}

/// The order of `count` phrases (repeating the pool when it asks for more than there are).
pub fn order(ph: &[Phrase], count: usize, mode: Mode, seed: u64, pass: &str, key: (u8, bool)) -> Vec<usize> {
    order_to(ph, count, mode, seed, pass, key, None)
}

/// [`order`] with the energy the smart order follows slot by slot (0..1; None = a quiet start, a peak two thirds in, a settle)
pub fn order_to(ph: &[Phrase], count: usize, mode: Mode, seed: u64, pass: &str, key: (u8, bool), curve: Option<&[f64]>) -> Vec<usize> {
    let n = ph.len();
    if n == 0 || count == 0 {
        return vec![];
    }
    match mode {
        Mode::Cipher => {
            let perm = keyed_permutation(n, if pass.is_empty() { "ling" } else { pass });
            (0..count).map(|i| perm[i % n]).collect()
        }
        Mode::Random => {
            let perm = keyed_permutation(n, &format!("seed-{seed}"));
            (0..count).map(|i| perm[i % n]).collect()
        }
        Mode::Smart => {
            let dmax = ph.iter().map(|p| p.density).fold(0.0f32, f32::max).max(0.01);
            let mut ks = KeyStream::new(&format!("smart-{seed}"));
            let mut used = vec![0usize; n];
            let mut out: Vec<usize> = vec![];
            for k in 0..count {
                // the density the arc wants here
                let pos = k as f32 / (count.max(2) - 1) as f32;
                let want = match curve {
                    Some(c) => c.get(k).copied().unwrap_or(0.5) as f32,
                    None => 0.3 + 0.7 * (std::f32::consts::PI * pos.powf(1.9)).sin(),
                };
                let prev = out.last().map(|i| &ph[*i]);
                let mut best = (f32::MAX, 0usize);
                for (i, p) in ph.iter().enumerate() {
                    let mut cost = (used[i] as f32) * 10.0 + (p.density / dmax - want).abs() * 3.0;
                    if let Some(q) = prev {
                        // the melody's step from the end of one into the start of the next (both in the common key)
                        if let (Some(l), Some(f)) = (q.last, p.first) {
                            let a = l as i32 + shift_to(q, key.0, key.1);
                            let b = f as i32 + shift_to(p, key.0, key.1);
                            cost += ((a - b).abs() as f32 / 2.0).min(6.0);
                        }
                        // harmonic mixing: phrases whose own keys sit close on the Camelot wheel follow each other more easily
                        cost += 0.25 * camelot_steps((q.tonic, q.minor), (p.tonic, p.minor)).min(6) as f32;
                        // the same file twice in a row only when nothing else fits
                        if q.file == p.file {
                            cost += 1.5;
                        }
                        if q.id == p.id {
                            cost += 20.0;
                        }
                    }
                    cost += (ks.next() % 1000) as f32 / 1000.0 * 0.8;
                    if cost < best.0 {
                        best = (cost, i);
                    }
                }
                used[best.1] += 1;
                out.push(best.1);
            }
            out
        }
    }
}

// ── the mashup: a story, a key path, layered parts, loop rolls ─────────────────────────

/// what a part does in a mashup
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub enum Cat {
    Drums,
    Bass,
    Pad,
    Stab,
    Lead,
    Arp,
}
pub const CATS: [Cat; 6] = [Cat::Drums, Cat::Bass, Cat::Pad, Cat::Stab, Cat::Lead, Cat::Arp];
impl Cat {
    pub fn name(self) -> &'static str {
        match self {
            Cat::Drums => "Drums",
            Cat::Bass => "Bass",
            Cat::Pad => "Pad",
            Cat::Stab => "Stab",
            Cat::Lead => "Lead",
            Cat::Arp => "Arp",
        }
    }
}

/// a part's category from its notes: chords held long are a pad, short ones a stab; a line of many short notes on a few pitches is an arp
pub fn category(part: Part, notes: &[Note], beats: f64) -> Cat {
    let avg = notes.iter().map(|n| n.len).sum::<f64>() / notes.len().max(1) as f64;
    match part {
        Part::Drums => Cat::Drums,
        Part::Bass => Cat::Bass,
        Part::Chords => {
            if avg >= 1.5 {
                Cat::Pad
            } else {
                Cat::Stab
            }
        }
        Part::Lead => {
            let mut keys: Vec<u8> = notes.iter().map(|n| n.key).collect();
            keys.sort();
            keys.dedup();
            if notes.len() as f64 / beats.max(1.0) >= 2.5 && avg <= 0.3 && keys.len() <= 8 {
                Cat::Arp
            } else {
                Cat::Lead
            }
        }
    }
}

/// the story arcs (songgen::story_arc) next to the emotional ones (emotion::ARCS)
pub const STORY_ARCS: [&str; 6] = ["rise", "fall", "journey", "dream", "battle", "hero"];

/// (energy 0..1, mood -1 sad .. 1 happy) of an arc at `x` (0..1 through the piece)
pub fn arc_point(arc: &str, x: f64) -> (f64, f64) {
    use std::f64::consts::PI;
    match arc {
        "release" => (0.3 + 0.7 * (PI * x.powf(1.4)).sin().max(0.0), -0.2 + 0.8 * x),
        "slow burn" | "rise" => (0.2 + 0.8 * x.powf(1.3), 0.3 * x),
        "bittersweet" => (0.45 + 0.3 * (PI * x).sin(), if ((x * 3.99) as i32) % 2 == 0 { 0.35 } else { -0.35 }),
        "triumph" => (0.3 + 0.7 * x.powf(0.8), -0.2 + x),
        "melancholy" => (0.5 - 0.25 * x + 0.15 * (PI * x).sin(), -0.6 - 0.2 * (PI * x).sin()),
        "storm" | "battle" => (0.65 + 0.35 * (PI * 3.0 * x).sin().abs(), -0.5 + 0.2 * (PI * 2.0 * x).sin()),
        "lullaby" | "dream" => (0.4 - 0.2 * x + 0.1 * (2.0 * PI * x).sin(), 0.3),
        "fall" => (0.9 - 0.7 * x, 0.4 - x),
        "journey" => (0.4 + 0.3 * (2.0 * PI * x).sin().abs() + 0.2 * x, 0.45 * (2.0 * PI * x).sin()),
        "hero" => (
            if x < 0.4 { 0.5 - 0.5 * x } else { 0.3 + 0.7 * ((x - 0.4) / 0.6) },
            if x < 0.45 { -0.4 } else { -0.4 + 1.2 * (x - 0.45) / 0.55 },
        ),
        _ => (0.3 + 0.7 * (PI * x.powf(1.9)).sin(), 0.0),
    }
}

/// The arc a feel or a story names: an arc's own name, the arc a story's words tell (songgen::story_arc), else the mood of its
/// words (emotion::lyric_lean), else the dice.
pub fn resolve_arc(feel: &str, story: &str, seed: u64) -> String {
    let f = feel.trim().to_lowercase();
    if crate::emotion::ARCS.contains(&f.as_str()) || STORY_ARCS.contains(&f.as_str()) {
        return f;
    }
    let s = crate::songgen::story_arc(story);
    if !s.is_empty() {
        return s;
    }
    if !story.trim().is_empty() {
        let (v, a) = crate::emotion::lyric_lean(story);
        return if v < -0.15 {
            if a > 0.15 { "storm" } else { "melancholy" }
        } else if v > 0.15 {
            if a > 0.15 { "triumph" } else { "release" }
        } else {
            "bittersweet"
        }
        .into();
    }
    crate::emotion::ARCS[(seed % crate::emotion::ARCS.len() as u64) as usize].to_string()
}

/// (energy, mood) for `n` slots along an arc; `lean` shifts the mood (a story's words)
pub fn movement(arc: &str, n: usize, lean: f64) -> Vec<(f64, f64)> {
    (0..n)
        .map(|i| {
            let x = if n < 2 { 0.5 } else { i as f64 / (n - 1) as f64 };
            let (e, v) = arc_point(arc, x);
            (e.clamp(0.05, 1.0), (v + lean).clamp(-1.0, 1.0))
        })
        .collect()
}

/// The key of every slot, moving on the wheel of fifths the way a DJ mixes harmonically (the Camelot wheel): stay; one step to
/// the dominant side (a fifth up, brighter) or the subdominant side (a fifth down, darker) as the energy moves; to the relative
/// minor when the mood falls and the relative major when it lifts; at most two energy boosts (a whole step up) into the peaks in
/// the second half; and home at the end when no boost carried it away. Never more than two steps from where it started.
/// (tonic, minor, the move)
pub fn key_path(mv: &[(f64, f64)], home: (u8, bool), seed: u64) -> Vec<(u8, bool, &'static str)> {
    let n = mv.len();
    let (mut t, mut m) = home;
    let v0 = mv.first().map(|x| x.1).unwrap_or(0.0);
    if v0 < -0.3 && !m {
        t = (t + 9) % 12;
        m = true;
    } else if v0 > 0.3 && m {
        t = (t + 3) % 12;
        m = false;
    }
    let start = (t, m);
    let mean_e = mv.iter().map(|x| x.0).sum::<f64>() / n.max(1) as f64;
    let mut peak = mv.first().map(|x| x.0).unwrap_or(0.0);
    let mut fifths = 0i32;
    let mut boosts = 0;
    let mut ks = KeyStream::new(&format!("keys-{seed}"));
    let mut out = vec![];
    for i in 0..n {
        let mut label = if i == 0 { "home" } else { "stay" };
        if i > 0 {
            let (e, v) = mv[i];
            // where the story wants to be: the minor side when the mood is low (the major when it is high, in between it stays),
            // brighter fifths as the energy rises above its average and the mood lifts, darker below
            let want_minor = if v < -0.2 { true } else if v > 0.2 { false } else { m };
            let want_f = ((e - mean_e) * 5.0 + v * 1.5).round().clamp(-2.0, 2.0) as i32;
            let roll = (ks.next() % 1000) as f64 / 1000.0;
            if e >= 0.85 && e > peak + 0.02 && boosts < 2 && i * 2 >= n {
                t = (t + 2) % 12;
                boosts += 1;
                label = "boost";
            } else if want_minor != m {
                if m {
                    t = (t + 3) % 12;
                    label = "relative major";
                } else {
                    t = (t + 9) % 12;
                    label = "relative minor";
                }
                m = want_minor;
            } else if want_f != fifths && roll < 0.75 {
                if want_f > fifths {
                    t = (t + 7) % 12;
                    fifths += 1;
                    label = "a fifth up";
                } else {
                    t = (t + 5) % 12;
                    fifths -= 1;
                    label = "a fifth down";
                }
            }
            peak = peak.max(e);
            if i + 1 == n && n > 2 && boosts == 0 && (t, m) != start {
                (t, m) = start;
                label = "home";
            }
        }
        out.push((t, m, label));
    }
    out
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Roll {
    None,
    /// 4 > 2 > 1 > 1/2 > 1/2 bars: the loop knob turned down a step at a time, into a lift
    Shrink,
    /// 1/2 > 1/2 > 1 > 2 > 4 bars: turned back up, out of a drop into the next slot
    Grow,
}
/// a loop roll is 8 bars
pub const ROLL_BEATS: f64 = 32.0;

pub fn roll_steps(r: Roll) -> Vec<f64> {
    match r {
        Roll::Shrink => vec![4.0, 2.0, 1.0, 0.5, 0.5],
        Roll::Grow => vec![0.5, 0.5, 1.0, 2.0, 4.0],
        Roll::None => vec![],
    }
}

/// One step of a DJ beat-repeat: `reps` loops of `len` beats, the first starting `at` beats into the build.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stage {
    pub at: f64,
    pub len: f64,
    pub reps: usize,
}

/// the loop lengths of a beat-repeat knob in beats: 1 bar > 1/2 > 1/4 > 1/8 > 1/16 > 1/32 bar
pub const REPEAT_LENS: [f64; 6] = [4.0, 2.0, 1.0, 0.5, 0.25, 0.125];
/// how many times each length loops before the knob goes to the next one
const REPEAT_REPS: [usize; 6] = [2, 2, 4, 4, 8, 16];

/// The loop stages of a DJ build of `total` beats (everything before the silence of the drop) with loops down to `shortest` beats:
/// every length halves the one before, the stages run back to back and end exactly at `total`; the longest lengths that do not fit
/// are left out, spare whole loops go to the first stage, and what is left over (under one loop) is the lead-in the first stage
/// starts after (`stages[0].at`), a time for the plain phrases.
pub fn beat_repeat(total: f64, shortest: f64) -> Vec<Stage> {
    let lens: Vec<usize> = (0..REPEAT_LENS.len()).filter(|i| REPEAT_LENS[*i] >= shortest - 1e-9).collect();
    if lens.is_empty() || total < 0.125 {
        return vec![];
    }
    let dur = |i: usize| REPEAT_LENS[i] * REPEAT_REPS[i] as f64;
    // the longest start that fits whole
    let from = (0..lens.len()).find(|f| lens[*f..].iter().map(|i| dur(*i)).sum::<f64>() <= total + 1e-9).unwrap_or(lens.len() - 1);
    let use_l: Vec<usize> = lens[from..].to_vec();
    let mut reps: Vec<usize> = use_l.iter().map(|i| REPEAT_REPS[*i]).collect();
    let l0 = REPEAT_LENS[use_l[0]];
    let sum = |reps: &[usize]| -> f64 { use_l.iter().zip(reps).map(|(i, r)| REPEAT_LENS[*i] * *r as f64).sum() };
    if sum(&reps) > total + 1e-9 {
        // even the last stage alone is too long (a build shorter than its loops): as many whole loops as fit
        reps[0] = ((total / l0) + 1e-9).floor().max(1.0) as usize;
    } else {
        let spare = total - sum(&reps);
        reps[0] += ((spare / l0) + 1e-9).floor() as usize;
    }
    let mut at = (total - sum(&reps)).max(0.0);
    let mut out = vec![];
    for (i, r) in use_l.iter().zip(&reps) {
        out.push(Stage { at, len: REPEAT_LENS[*i], reps: *r });
        at += REPEAT_LENS[*i] * *r as f64;
    }
    out
}

/// one layer of a slot: a part of a phrase, its category, the semitones into the slot's key; the spine is the slot's own phrase
#[derive(Clone, Debug)]
pub struct Layer {
    pub phrase: usize,
    pub part: usize,
    pub cat: Cat,
    pub shift: i32,
    pub spine: bool,
    /// whole octaves on top of `shift`: the register guard and the octave allocation (register.rs)
    pub octave: i32,
    /// shares its octave with another part: played softer, behind it (a pad)
    pub behind: bool,
    /// the windows (beats in the slot) it plays in; empty = all of it (a second lead answering the hook plays in its gaps)
    pub mask: Vec<(f64, f64)>,
    /// the instrument role (ai_import's names) and which of the role's patches (the category's running count)
    pub role: &'static str,
    pub pick: usize,
}

impl Layer {
    pub fn new(phrase: usize, part: usize, cat: Cat, shift: i32, spine: bool) -> Layer {
        Layer { phrase, part, cat, shift, spine, octave: 0, behind: false, mask: vec![], role: "", pick: 0 }
    }
}

/// what register handling a mashup does: none (a bass still kept on the keyboard, whole), the guard only (a muddy bass up, a shrill
/// lead down), or the guard and an octave of its own for every layered part
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Registers {
    Off,
    Guard,
    Layer,
}
impl Registers {
    pub fn parse(s: &str) -> Registers {
        match s.trim() {
            "off" | "0" | "none" => Registers::Off,
            "guard" => Registers::Guard,
            _ => Registers::Layer,
        }
    }
}

/// the arrangement smarts of a mashup (all on by default)
#[derive(Clone, Debug)]
pub struct Smarts {
    pub registers: Registers,
    /// the Sounds genre: the roles (and so the patches' brightness) of the layers
    pub genre: String,
    /// sections on 1 / 2 / 4 / 8 / 16-bar phrases (a 3-bar phrase loops to 4)
    pub align: bool,
    /// a breakdown (pad and hook) before the last drop
    pub breakdown: bool,
    /// a second lead answers the hook in its gaps instead of talking over it
    pub answer: bool,
    /// layers whose notes clash with what sounds with them are swapped for another part of their kind (or left out)
    pub clash: bool,
}
impl Default for Smarts {
    fn default() -> Smarts {
        Smarts { registers: Registers::Layer, genre: "pop".into(), align: true, breakdown: true, answer: true, clash: true }
    }
}

/// a category's voice for the register (None = drums)
pub fn voice(c: Cat) -> Option<crate::register::Voice> {
    use crate::register::Voice;
    match c {
        Cat::Drums => None,
        Cat::Bass => Some(Voice::Bass),
        Cat::Pad => Some(Voice::Pad),
        Cat::Stab => Some(Voice::Chords),
        Cat::Lead => Some(Voice::Lead),
        Cat::Arp => Some(Voice::Arp),
    }
}

/// a section's length on the phrase grid: the nearest of 1, 2, 4, 8, 16 bars (ties up)
pub fn aligned_beats(beats: f64) -> f64 {
    let bars = (beats / 4.0).max(0.25);
    let mut best = (f64::MAX, 4.0);
    for c in [1.0f64, 2.0, 4.0, 8.0, 16.0] {
        let d = (bars / c).log2().abs();
        if d < best.0 - 1e-9 || ((d - best.0).abs() < 1e-9 && c > best.1) {
            best = (d, c);
        }
    }
    best.1 * 4.0
}

/// steps on the Camelot wheel between two keys (a fifth = 1, the relative = 1, the parallel = 4 ...)
pub fn camelot_steps(a: (u8, bool), b: (u8, bool)) -> i32 {
    // every key by its relative major's place on the circle of fifths
    let pos = |k: (u8, bool)| ((if k.1 { (k.0 as i32 + 3) % 12 } else { k.0 as i32 % 12 }) * 7) % 12;
    let d = (pos(a) - pos(b)).rem_euclid(12);
    d.min(12 - d) + (a.1 != b.1) as i32
}

/// the register decisions of a mashup, counted: "bass up 1 octave (was muddy) x3, ..."
pub fn summarize(log: &[String]) -> String {
    let mut seen: Vec<(String, usize)> = vec![];
    for l in log {
        match seen.iter_mut().find(|s| s.0 == *l) {
            Some(s) => s.1 += 1,
            None => seen.push((l.clone(), 1)),
        }
    }
    seen.iter().map(|(t, n)| if *n > 1 { format!("{t} x{n}") } else { t.clone() }).collect::<Vec<_>>().join(", ")
}

#[derive(Clone, Debug)]
pub struct Slot {
    pub beats: f64,
    pub key: (u8, bool),
    pub energy: f64,
    pub mood: f64,
    pub spine: usize,
    pub layers: Vec<Layer>,
    pub roll_after: Roll,
    pub move_label: &'static str,
}

pub struct Mashup {
    pub arc: String,
    pub slots: Vec<Slot>,
    /// register and arrangement decisions, one line each ("bass up 1 octave (was muddy)"); see [`summarize`]
    pub log: Vec<String>,
    /// the slot made a breakdown before the last drop
    pub breakdown: Option<usize>,
}

/// notes repeated (or cut) to fill `beats`
pub fn tile(notes: &[Note], src_beats: f64, beats: f64) -> Vec<Note> {
    let src = src_beats.max(0.25);
    let mut out = vec![];
    let mut off = 0.0;
    while off < beats - 1e-6 {
        for n in notes.iter().filter(|n| n.start < src - 1e-6) {
            let s = n.start + off;
            if s >= beats - 1e-6 {
                continue;
            }
            let mut m = n.clone();
            m.start = s;
            m.len = m.len.min(beats - s).max(0.03);
            out.push(m);
        }
        off += src;
    }
    out
}

/// a layer's notes for a slot of `beats`: looped to fit, moved into the slot's key and its octave (the whole line, its shape kept),
/// only in its windows, softer and wider when it sits behind
pub fn layer_notes(ph: &[Phrase], l: &Layer, beats: f64) -> Vec<Note> {
    let p = &ph[l.phrase];
    let Some((_, _, notes)) = p.parts.get(l.part) else { return vec![] };
    let mut v = tile(notes, p.beats, beats);
    if !l.mask.is_empty() {
        v.retain(|n| l.mask.iter().any(|(a, b)| n.start >= a - 1e-6 && n.start < b - 1e-6));
        for n in v.iter_mut() {
            if let Some((_, b)) = l.mask.iter().find(|(a, b)| n.start >= a - 1e-6 && n.start < b - 1e-6) {
                n.len = n.len.min(b - n.start).max(0.03);
            }
        }
    }
    if l.cat != Cat::Drums {
        for n in v.iter_mut() {
            n.key = (n.key as i32 + l.shift + 12 * l.octave).clamp(0, 127) as u8;
        }
    }
    if l.behind {
        for n in v.iter_mut() {
            n.vel *= 0.72;
            n.pan = if n.key % 2 == 0 { -0.4 } else { 0.4 };
        }
    }
    v
}

/// the octave that keeps a bass on the keyboard, the whole line moved (no register smarts: its median between 28 and 64)
fn bass_fold(notes: &[Note], shift: i32) -> i32 {
    let Some(r) = crate::register::analyze(notes, 1.0) else { return 0 };
    let m = r.median + shift as f64;
    let mut k = 0;
    while m + 12.0 * (k as f64) < 28.0 && k < 6 {
        k += 1;
    }
    while m + 12.0 * (k as f64) > 64.0 && k > -6 {
        k -= 1;
    }
    k
}

/// A riser for a build of `beats` in `key`: a ladder up the key's scale, two octaves from the tonic around C4, a step every two
/// beats, then a beat, a half, a quarter (it speeds up like the loop roll), louder as it goes; it stops half a beat before the drop.
pub fn riser_notes(key: (u8, bool), beats: f64) -> Vec<Note> {
    let sc = crate::theory::Key::new(key.0, key.1).scale();
    let base = if key.0 > 6 { 48 + key.0 as i32 } else { 60 + key.0 as i32 };
    let end = (beats - 0.5).max(1.0);
    let mut out = vec![];
    let mut t = 0.0;
    while t < end - 1e-6 {
        let frac = t / end;
        let step: f64 = if frac < 0.45 { 2.0 } else if frac < 0.72 { 1.0 } else if frac < 0.9 { 0.5 } else { 0.25 };
        let deg = (frac * 14.0).round() as usize;
        let k = base + (sc[deg % 7] as i32 - key.0 as i32).rem_euclid(12) + 12 * (deg / 7) as i32;
        out.push(Note::new(t, (step * 1.05).min(end - t), k.clamp(0, 127) as u8, (0.3 + 0.6 * frac) as f32));
        t += step;
    }
    out
}

/// A layer as a loop roll (8 bars): each step loops the first bars of its material (4, 2, 1, 1/2 ...), cut on the bar grid so the
/// loops stay in time and in the key, the velocity rising through it.
pub fn roll_notes(notes: &[Note], src_beats: f64, r: Roll) -> Vec<Note> {
    let src = tile(notes, src_beats, 16.0);
    let mut out = vec![];
    let mut at = 0.0;
    for b in roll_steps(r) {
        let len = b * 4.0;
        for n in src.iter().filter(|n| n.start < len - 1e-6) {
            let mut m = n.clone();
            m.start = n.start + at;
            m.len = n.len.min(len - n.start).max(0.03);
            m.vel = (n.vel * (0.72 + 0.4 * (m.start / ROLL_BEATS) as f32)).min(1.0);
            out.push(m);
        }
        at += len;
    }
    out
}

/// The filter contour of the rolls, for a Vox Filter on every track: (high-pass lane, low-pass lane) of (beat, 0..1). A shrinking
/// roll raises the high-pass a stair with each loop step (the knob and the loop move together) and drops it open on the downbeat
/// after; a growing roll opens the low-pass a stair at a time.
pub fn roll_contour(spans: &[(f64, Roll)]) -> (Vec<(f64, f32)>, Vec<(f64, f32)>) {
    let (mut hp, mut lp) = (vec![(0.0, 0.0f32)], vec![(0.0, 1.0f32)]);
    for (start, r) in spans {
        let steps = roll_steps(*r);
        let k = steps.len().max(2);
        match r {
            Roll::Shrink => hp.push(((start - 0.01).max(0.0), 0.0)),
            Roll::Grow => lp.push(((start - 0.01).max(0.0), 1.0)),
            Roll::None => continue,
        }
        let mut at = *start;
        for (i, b) in steps.iter().enumerate() {
            let len = b * 4.0;
            match r {
                Roll::Shrink => {
                    let v = 0.15 + 0.8 * (i + 1) as f32 / k as f32;
                    hp.push((at, v));
                    hp.push((at + len - 0.01, v));
                }
                _ => {
                    let v = i as f32 / (k - 1) as f32;
                    lp.push((at, v));
                    lp.push((at + len - 0.01, v));
                }
            }
            at += len;
        }
        hp.push((at, 0.0));
        lp.push((at, 1.0));
    }
    hp.sort_by(|a, b| a.0.total_cmp(&b.0));
    lp.sort_by(|a, b| a.0.total_cmp(&b.0));
    (hp, lp)
}

/// the role (ai_import's names) a category plays for a slot's energy and mood; `k` rolls among the fitting ones
pub fn role_for(cat: Cat, energy: f64, minor: bool, genre: &str, k: usize) -> &'static str {
    let pick = |v: &[&'static str]| v[k % v.len()];
    let bassm = matches!(genre, "dubstep" | "drum and bass");
    match cat {
        Cat::Drums => "drums",
        Cat::Bass if bassm && energy > 0.6 => pick(&["growl bass", "wobble bass"]),
        Cat::Bass if energy > 0.6 => pick(&["rolling bass", "pluck bass", "sub bass"]),
        Cat::Bass => pick(&["sub bass", "pluck bass"]),
        Cat::Pad if minor => pick(&["dark pad", "warm pad", "ambient pad"]),
        Cat::Pad => pick(&["warm pad", "ambient pad", "strings"]),
        Cat::Stab => pick(&["stab chord", "keys", "piano", "organ"]),
        Cat::Lead if energy > 0.7 && bassm => pick(&["supersaw lead", "laser lead", "synth lead"]),
        Cat::Lead if energy > 0.7 => pick(&["supersaw lead", "synth lead", "pluck"]),
        Cat::Lead => pick(&["synth lead", "pluck", "keys"]),
        Cat::Arp => pick(&["arp", "pluck"]),
    }
}

/// How well notes sit in a key and against what sounds with them: the share of their duration on the key's scale, and the share
/// of their overlap with `against` that is consonant (unison, thirds, fourth, fifth, sixths, octave).
fn fit_score(notes: &[Note], shift: i32, key: crate::theory::Key, against: &[Note]) -> f64 {
    let (mut inside, mut total) = (0.0, 0.0);
    let (mut cons, mut over) = (0.0, 0.0);
    for n in notes {
        let k = n.key as i32 + shift;
        let d = n.len.max(0.05);
        total += d;
        if key.in_scale(k.rem_euclid(12) as u8) {
            inside += d;
        }
        for a in against {
            let o = (n.start + n.len).min(a.start + a.len) - n.start.max(a.start);
            if o > 0.0 {
                over += o;
                if matches!((k - a.key as i32).rem_euclid(12), 0 | 3 | 4 | 5 | 7 | 8 | 9) {
                    cons += o;
                }
            }
        }
    }
    if total <= 0.0 {
        return 0.0;
    }
    inside / total + 0.5 * if over > 0.0 { cons / over } else { 1.0 }
}

/// The transposition that fits a part into a slot: every shift from -6 to +5 tried on its notes (in the key's scale, consonant
/// with the hook), a small cost per semitone so it moves as little as it can; the key-based shift `base` wins ties. A phrase whose
/// detected key was wrong (a modal tune, a borrowed chord) or that sits better a step away goes where it actually fits.
pub fn fit_shift(notes: &[Note], base: i32, key: (u8, bool), against: &[Note]) -> i32 {
    fit_shift_within(notes, base, key, against, -6, 5)
}

/// [`fit_shift`] over the shifts `lo ..= hi` only (a recording moves only a few semitones before its formants give it away)
pub fn fit_shift_within(notes: &[Note], base: i32, key: (u8, bool), against: &[Note], lo: i32, hi: i32) -> i32 {
    let k = crate::theory::Key::new(key.0, key.1);
    let base = base.clamp(lo, hi);
    let mut best = (f64::MIN, base);
    for s in lo..=hi {
        let sc = fit_score(notes, s, k, against) - 0.012 * s.abs() as f64 + if s == base { 0.02 } else { 0.0 };
        if sc > best.0 + 1e-9 {
            best = (sc, s);
        }
    }
    best.1
}

/// a section tag for a slot (for the arranger's ruler and the sound design)
pub fn slot_tag(i: usize, n: usize, energy: f64) -> &'static str {
    if i == 0 && energy < 0.6 {
        "intro"
    } else if i + 1 == n && energy < 0.6 {
        "outro"
    } else if energy >= 0.75 {
        "drop"
    } else if energy >= 0.5 {
        "verse"
    } else {
        "break"
    }
}

/// The mashup plan for the spine order: the arc's energy and mood per slot, the key path, each slot's layers and the rolls.
/// `layering` 0 = every slot plays its own phrase whole (a medley); higher = parts of other files layered on the spine's hook,
/// as many as the energy asks for. `rolls` 0 = none; higher = rolls at smaller energy changes, and one before the peak from 0.5.
/// All the arrangement smarts on ([`Smarts::default`]); see [`mashup_with`].
pub fn mashup(ph: &[Phrase], spine: &[usize], arc: &str, lean: f64, layering: f64, rolls: f64, seed: u64, home: (u8, bool)) -> Mashup {
    mashup_with(ph, spine, arc, lean, layering, rolls, seed, home, &Smarts::default())
}

/// [`mashup`] with the smarts chosen:
/// * phrase alignment - a layered section is 1, 2, 4, 8 or 16 bars (the nearest), its parts looped or cut to it;
/// * a breakdown - the section before the last drop (when it would play at full energy too) drops to a pad and the hook, so the last
///   drop lands (and the roll into it builds);
/// * harmonic mixing - a layer from a file in a key close to the section's on the Camelot wheel is preferred (it needs the least
///   transposition and keeps its character);
/// * chord-aware fitting - every pitched layer is fitted by ear against everything already sounding in the section (not only the
///   hook); one whose notes still clash (over 30 % of its overlap on minor seconds, tritones, major sevenths) is swapped for the
///   least clashing of up to six other parts of its kind, and left out when even that clashes over 45 %;
/// * call and response - a second lead plays in the bars where the hook rests (else every other two bars), answering it;
/// * registers ([`Registers`]) - the guard moves a muddy bass up and a shrill lead or arp down by whole octaves (register.rs, the
///   thresholds from the role's patch brightness and how busy the line is); `Layer` also gives every layered part an octave of its
///   own with the least movement (a pad may sit softer and wide behind another part; with no octave free a layer is left out).
///   In a whole-phrase medley (layering 0) only the guard runs.
pub fn mashup_with(ph: &[Phrase], spine: &[usize], arc: &str, lean: f64, layering: f64, rolls: f64, seed: u64, home: (u8, bool), sm: &Smarts) -> Mashup {
    use crate::register::{allocate, analyze, brightness, clash, guard, limits, reason, Placing};
    let n = spine.len();
    let mut mv = movement(arc, n, lean);
    let keys = key_path(&mv, home, seed);
    let layered = layering >= 0.05;
    let mut log: Vec<String> = vec![];
    // the breakdown before the last drop
    let mut breakdown = None;
    if layered && sm.breakdown && n >= 6 {
        if let Some(l) = (n / 2..n).rev().find(|i| mv[*i].0 >= 0.75) {
            if l >= 2 && mv[l - 1].0 >= 0.6 {
                mv[l - 1].0 = 0.3;
                breakdown = Some(l - 1);
                log.push("a breakdown before the last drop".into());
            }
        }
    }
    let regs = if layered { sm.registers } else if sm.registers == Registers::Layer { Registers::Guard } else { sm.registers };
    // every part of every phrase, categorized, with its density
    let mut cells: Vec<(usize, usize, Cat, f64)> = vec![];
    for (i, p) in ph.iter().enumerate() {
        for (j, (part, _, notes)) in p.parts.iter().enumerate() {
            cells.push((i, j, category(*part, notes, p.beats), notes.len() as f64 / p.beats.max(1.0)));
        }
    }
    let mut dmax = [0.01f64; 6];
    for c in &cells {
        dmax[c.2 as usize] = dmax[c.2 as usize].max(c.3);
    }
    let mut used = vec![0usize; cells.len()];
    let mut uses = [0usize; 6];
    let mut ks = KeyStream::new(&format!("mashup-{seed}"));
    let mut slots = vec![];
    let shifted = |ns: &[Note], s: i32| -> Vec<Note> { ns.iter().map(|x| Note { key: (x.key as i32 + s).clamp(0, 127) as u8, ..x.clone() }).collect() };
    for (si, &pi) in spine.iter().enumerate() {
        let p = &ph[pi];
        let sb = if layered && sm.align { aligned_beats(p.beats) } else { p.beats };
        let (e, v) = mv[si];
        let (t, m, label) = keys[si];
        let shift_of = |q: usize, cat: Cat| if cat == Cat::Drums { 0 } else { shift_to(&ph[q], t, m) };
        let own: Vec<(usize, Cat)> = cells.iter().filter(|c| c.0 == pi).map(|c| (c.1, c.2)).collect();
        // the spine: the phrase's hook (its lead, else chords, arp, bass, drums)
        let pref = [Cat::Lead, Cat::Stab, Cat::Pad, Cat::Arp, Cat::Bass, Cat::Drums];
        let sp = pref.iter().find_map(|c| own.iter().find(|o| o.1 == *c)).copied().unwrap_or((0, Cat::Lead));
        let mut layers = vec![Layer::new(pi, sp.0, sp.1, shift_of(pi, sp.1), true)];
        if !layered {
            for o in own.iter().filter(|o| o.0 != sp.0) {
                layers.push(Layer::new(pi, o.0, o.1, shift_of(pi, o.1), false));
            }
        } else {
            // what the energy asks for: a chord bed, then bass, drums, and an arp (or a second line) at the peak
            let chord = if e >= 0.55 { vec![Cat::Stab, Cat::Pad] } else { vec![Cat::Pad, Cat::Stab] };
            let mut want: Vec<Vec<Cat>> = vec![chord];
            if e >= 0.35 {
                want.push(vec![Cat::Bass]);
            }
            if e >= 0.55 {
                want.push(vec![Cat::Drums]);
            }
            if e >= 0.75 {
                want.push(vec![Cat::Arp, Cat::Lead]);
            }
            for alts in want {
                let alts: Vec<Cat> = alts.into_iter().filter(|c| !layers.iter().any(|l| l.cat == *c)).collect();
                if alts.is_empty() {
                    continue;
                }
                let r = (ks.next() % 1000) as f64 / 1000.0;
                // the spine phrase's own part, when it has one and the dice keep it
                if let Some(o) = own.iter().find(|o| alts.contains(&o.1) && o.0 != sp.0) {
                    if r >= layering {
                        layers.push(Layer::new(pi, o.0, o.1, shift_of(pi, o.1), false));
                        continue;
                    }
                }
                // else the part of the collection that fits best: its density to the energy, the mode of the slot's key and a key
                // near it on the Camelot wheel, another file (the more layering, the more), a length that loops evenly in the slot,
                // not used much yet
                let mut best: Option<(f64, usize)> = None;
                for (ci, c) in cells.iter().enumerate() {
                    if !alts.contains(&c.2) {
                        continue;
                    }
                    let q = &ph[c.0];
                    let mut cost = (c.3 / dmax[c.2 as usize] - e).abs() * 2.0 + used[ci] as f64 * 1.5;
                    cost += alts.iter().position(|a| *a == c.2).unwrap_or(0) as f64 * 0.4;
                    if c.2 != Cat::Drums {
                        if q.minor != m {
                            cost += 0.8;
                        }
                        cost += 0.12 * camelot_steps((q.tonic, q.minor), (t, m)).min(5) as f64;
                    }
                    if q.file != p.file {
                        cost -= 0.8 * layering;
                    }
                    let ratio = sb / q.beats.max(0.25);
                    if ratio < 0.99 || (ratio - ratio.round()).abs() > 0.01 {
                        cost += 0.5;
                    }
                    cost += (ks.next() % 1000) as f64 / 1000.0 * 0.6;
                    if best.map(|b| cost < b.0).unwrap_or(true) {
                        best = Some((cost, ci));
                    }
                }
                if let Some((_, ci)) = best {
                    used[ci] += 1;
                    let c = cells[ci];
                    layers.push(Layer::new(c.0, c.1, c.2, shift_of(c.0, c.2), false));
                }
            }
        }
        // fit by ear, chord-aware: the hook into the key, then every pitched layer into the key and against all that already sounds
        let tiles = |l: &Layer| -> Vec<Note> {
            let q = &ph[l.phrase];
            q.parts.get(l.part).map(|x| tile(&x.2, q.beats, sb)).unwrap_or_default()
        };
        let mut against: Vec<Note> = vec![];
        let mut fitted: Vec<Option<Vec<Note>>> = vec![None; layers.len()];
        let mut li = 0;
        while li < layers.len() {
            if layers[li].cat == Cat::Drums {
                li += 1;
                continue;
            }
            let raw = tiles(&layers[li]);
            let s = fit_shift(&raw, layers[li].shift, (t, m), &against);
            layers[li].shift = s;
            let mut ns = shifted(&raw, s);
            let mut drop_it = false;
            if sm.clash && layered && !layers[li].spine && !against.is_empty() {
                let cl = clash(&ns, &against);
                if cl > 0.3 {
                    // up to six other parts of its kind, the least used first
                    let mut cand: Vec<(usize, u64, usize)> =
                        (0..cells.len()).filter(|ci| cells[*ci].2 == layers[li].cat && !(cells[*ci].0 == layers[li].phrase && cells[*ci].1 == layers[li].part)).map(|ci| (used[ci], ks.next() % 1000, ci)).collect();
                    cand.sort();
                    let mut best: Option<(f64, usize, i32, Vec<Note>)> = None;
                    for (_, _, ci) in cand.into_iter().take(6) {
                        let c = cells[ci];
                        let q = &ph[c.0];
                        let raw2 = tile(&q.parts[c.1].2, q.beats, sb);
                        let s2 = fit_shift(&raw2, shift_to(q, t, m), (t, m), &against);
                        let ns2 = shifted(&raw2, s2);
                        let cl2 = clash(&ns2, &against);
                        if best.as_ref().map(|b| cl2 < b.0).unwrap_or(true) {
                            best = Some((cl2, ci, s2, ns2));
                        }
                    }
                    let name = layers[li].cat.name().to_lowercase();
                    match best {
                        Some((cl2, ci, s2, ns2)) if cl2 < cl - 0.1 => {
                            used[ci] += 1;
                            layers[li].phrase = cells[ci].0;
                            layers[li].part = cells[ci].1;
                            layers[li].shift = s2;
                            ns = ns2;
                            if cl2 > 0.45 {
                                drop_it = true;
                            } else {
                                log.push(format!("{name} swapped for one that does not clash"));
                            }
                        }
                        _ => drop_it = cl > 0.45,
                    }
                    if drop_it {
                        log.push(format!("{name} left out (clashed)"));
                    }
                }
            }
            if drop_it {
                layers.remove(li);
                fitted.remove(li);
                continue;
            }
            against.extend(ns.iter().cloned());
            fitted[li] = Some(ns);
            li += 1;
        }
        // call and response: a second lead plays where the hook rests
        if layered && sm.answer && layers[0].cat == Cat::Lead {
            if let Some(hook) = fitted[0].clone() {
                let bars = (sb / 4.0).ceil().max(1.0) as usize;
                let act: Vec<f64> = (0..bars)
                    .map(|b| {
                        let (b0, b1) = (b as f64 * 4.0, b as f64 * 4.0 + 4.0);
                        hook.iter().map(|x| ((x.start + x.len).min(b1) - x.start.max(b0)).max(0.0)).sum::<f64>() / 4.0
                    })
                    .collect();
                for l in layers.iter_mut().skip(1).filter(|l| l.cat == Cat::Lead) {
                    let mut on: Vec<usize> = (0..bars).filter(|b| act[*b] < 0.25).collect();
                    if on.len() * 4 < bars || on.len() * 4 > bars * 3 {
                        on = (0..bars).filter(|b| if bars >= 4 { (b / 2) % 2 == 1 } else { b % 2 == 1 }).collect();
                    }
                    if !on.is_empty() && on.len() < bars {
                        l.mask = on.iter().map(|b| (*b as f64 * 4.0, (*b as f64 * 4.0 + 4.0).min(sb))).collect();
                        log.push("a second lead answers the hook".into());
                    }
                }
            }
        }
        // every layer's role and patch: its category's role for the energy and mood, the next patch each time
        for l in layers.iter_mut() {
            let k = uses[l.cat as usize];
            uses[l.cat as usize] += 1;
            l.pick = k;
            l.role = role_for(l.cat, e, m, &sm.genre, k);
        }
        // registers
        let pitched: Vec<usize> = (0..layers.len()).filter(|i| layers[*i].cat != Cat::Drums && fitted[*i].as_ref().map(|f| !f.is_empty()).unwrap_or(false)).collect();
        if regs == Registers::Off {
            for i in pitched.iter().copied() {
                if layers[i].cat == Cat::Bass {
                    layers[i].octave = bass_fold(fitted[i].as_deref().unwrap_or(&[]), 0);
                }
            }
        } else if !pitched.is_empty() {
            let places: Vec<Placing> = pitched
                .iter()
                .map(|i| {
                    let l = &layers[*i];
                    let reg = analyze(fitted[*i].as_deref().unwrap_or(&[]), sb).expect("pitched");
                    let vc = voice(l.cat).expect("pitched");
                    let busy = reg.busy();
                    Placing { reg, voice: vc, limits: limits(vc, brightness(l.role), busy), weight: if l.spine { 1.5 } else { 1.0 }, keep: l.spine || l.cat == Cat::Bass }
                })
                .collect();
            let placed: Vec<crate::register::Placed> = if regs == Registers::Layer && places.len() >= 2 {
                allocate(&places)
            } else {
                places.iter().map(|pl| crate::register::Placed { octave: guard(&pl.reg, &pl.limits), behind: false, thinned: false }).collect()
            };
            let mut gone: Vec<usize> = vec![];
            for ((i, pl), pd) in pitched.iter().zip(&places).zip(&placed) {
                let l = &mut layers[*i];
                let name = l.cat.name().to_lowercase();
                l.octave = pd.octave;
                l.behind = pd.behind;
                if pd.octave != 0 {
                    let g = guard(&pl.reg, &pl.limits);
                    let why = if g != 0 && g.signum() == pd.octave.signum() { reason(pl.voice, g) } else { "own octave" };
                    let o = pd.octave.abs();
                    log.push(format!("{name} {} {o} octave{} ({why})", if pd.octave > 0 { "up" } else { "down" }, if o > 1 { "s" } else { "" }));
                }
                if pd.behind {
                    log.push(format!("{name} behind (shares an octave)"));
                }
                if pd.thinned && !l.spine {
                    log.push(format!("{name} left out (no octave free)"));
                    gone.push(*i);
                }
            }
            gone.sort();
            for i in gone.into_iter().rev() {
                layers.remove(i);
            }
        }
        slots.push(Slot { beats: sb, key: (t, m), energy: e, mood: v, spine: pi, layers, roll_after: Roll::None, move_label: label });
    }
    if rolls > 0.01 && n >= 2 {
        let thr = 0.4 - 0.3 * rolls.clamp(0.0, 1.0);
        for i in 0..n - 1 {
            let de = mv[i + 1].0 - mv[i].0;
            // a big step in energy, or (from 0.3) the line into a drop and out of one
            let (into, out) = (mv[i].0 < 0.75 && mv[i + 1].0 >= 0.75, mv[i].0 >= 0.75 && mv[i + 1].0 < 0.75);
            slots[i].roll_after = if de > thr || (rolls >= 0.3 && into) {
                Roll::Shrink
            } else if de < -thr || (rolls >= 0.3 && out) {
                Roll::Grow
            } else {
                Roll::None
            };
        }
        if rolls >= 0.5 && n >= 3 {
            let peak = (1..n).max_by(|a, b| mv[*a].0.total_cmp(&mv[*b].0)).unwrap_or(1);
            if slots[peak - 1].roll_after == Roll::None {
                slots[peak - 1].roll_after = Roll::Shrink;
            }
        }
        // the build out of the breakdown into the last drop
        if let Some(b) = breakdown {
            slots[b].roll_after = Roll::Shrink;
        }
    }
    Mashup { arc: arc.to_string(), slots, log, breakdown }
}

/// "a3f9c1" - the short id a clip name carries
pub fn short_id(id: u64) -> String {
    format!("{:06x}", id & 0xff_ffff)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_survives_the_cipher_and_the_wrong_key_does_not_read_it() {
        for n in [2usize, 3, 7, 23, 40, 200] {
            for msg in ["", "hi", "meet me at the chorus", "ling ☺ 123"] {
                let idx = encode(msg, n, "correct horse");
                assert!(idx.iter().all(|i| *i < n));
                assert_eq!(decode(&idx, n, "correct horse").as_deref(), Some(msg), "n {n} msg {msg}");
            }
        }
        let idx = encode("secret plans", 23, "key one");
        assert_ne!(decode(&idx, 23, "key two").as_deref(), Some("secret plans"));
        // the permutation is a permutation, keyed and repeatable
        let p = keyed_permutation(50, "x");
        let mut s = p.clone();
        s.sort();
        assert_eq!(s, (0..50).collect::<Vec<_>>());
        assert_eq!(p, keyed_permutation(50, "x"));
        assert_ne!(p, keyed_permutation(50, "y"));
    }

    fn file_with(tonic: u8, minor: bool, name: &str) -> ImportedMidi {
        // 16 bars: a lead line in the key, a bass, chords; a rest every 4 bars so phrases are found
        let scale: [u8; 7] = if minor { [0, 2, 3, 5, 7, 8, 10] } else { [0, 2, 4, 5, 7, 9, 11] };
        let mut lead = vec![];
        let mut bass = vec![];
        for bar in 0..16 {
            if bar % 4 == 3 {
                continue;
            }
            for b in 0..4 {
                let deg = (bar * 3 + b) % 7;
                lead.push(Note::new(bar as f64 * 4.0 + b as f64, 0.9, 60 + tonic + scale[deg], 0.8));
            }
            bass.push(Note::new(bar as f64 * 4.0, 3.5, 36 + tonic, 0.8));
        }
        let mk = |notes: Vec<Note>, ch: u8, nm: &str| crate::midi_import::ImportedTrack { name: nm.into(), channel: ch, notes, program: Some(80), ..Default::default() };
        let _ = name;
        ImportedMidi { tracks: vec![mk(lead, 0, "lead"), mk(bass, 1, "bass")], bpm: Some(120.0), tempos: vec![], length_beats: 64.0, ..Default::default() }
    }


    #[test]
    fn a_story_walks_the_wheel_layers_parts_and_rolls_the_loops() {
        // the key path only takes harmonic-mixing steps
        for arc in ["triumph", "melancholy", "journey", "hero", "storm"] {
            let mv = movement(arc, 16, 0.0);
            let path = key_path(&mv, (0, false), 7);
            let boosts = path.iter().filter(|p| p.2 == "boost").count();
            assert!(boosts <= 2, "{arc}");
            for w in path.windows(2) {
                let ((a, am, _), (b, bm, l)) = (w[0], w[1]);
                let d = (b as i32 - a as i32).rem_euclid(12);
                let ok = match l {
                    "stay" => d == 0 && am == bm,
                    "a fifth up" => d == 7 && am == bm,
                    "a fifth down" => d == 5 && am == bm,
                    "relative minor" => d == 9 && !am && bm,
                    "relative major" => d == 3 && am && !bm,
                    "boost" => d == 2,
                    "home" => true,
                    _ => false,
                };
                assert!(ok, "{arc}: {a}{am} -> {b}{bm} called {l}");
            }
        }
        let tri = movement("triumph", 12, 0.0);
        assert!(tri[11].0 > tri[0].0 + 0.4 && tri[11].1 > tri[0].1, "a triumph rises and brightens");
        let mel = key_path(&movement("melancholy", 8, 0.0), (0, false), 1);
        assert!(mel[0].1, "a melancholy story starts in the relative minor");
        assert_eq!(resolve_arc("", "the hero returns home", 0), "hero");
        assert_eq!(resolve_arc("Storm", "", 0), "storm");

        // a mashup layers parts of other files on the spine, more of them when the energy is up
        let mut ph = chop(&file_with(0, false, "a"), "a", 2.0);
        ph.extend(chop(&file_with(7, false, "b"), "b", 2.0));
        ph.extend(chop(&file_with(9, true, "c"), "c", 2.0));
        let spine: Vec<usize> = (0..10).map(|i| i % ph.len()).collect();
        let m = mashup(&ph, &spine, "slow burn", 0.0, 0.9, 1.0, 3, (0, false));
        assert_eq!(m.slots.len(), 10);
        assert!(m.slots.iter().all(|s| s.layers.iter().filter(|l| l.spine).count() == 1));
        let phr = &ph;
        let foreign = m.slots.iter().flat_map(|s| s.layers.iter().map(move |l| phr[l.phrase].file != phr[s.spine].file)).filter(|x| *x).count();
        assert!(foreign > 0, "parts of other files are layered in");
        assert!(m.slots[9].layers.len() > m.slots[0].layers.len(), "the peak is fuller than the start");
        assert!(m.slots.iter().any(|s| s.roll_after == Roll::Shrink), "a roll leads into the lift");
        // every pitched layer lands in its slot's key
        for s in &m.slots {
            let key = crate::theory::Key { tonic: s.key.0, minor: s.key.1 };
            for l in s.layers.iter().filter(|l| l.cat != Cat::Drums) {
                let notes = layer_notes(&ph, l, s.beats);
                let inside = notes.iter().filter(|n| key.in_scale(n.key % 12)).count();
                assert!(inside * 10 >= notes.len() * 9, "{:?} in {}", l, key.name());
                assert!(notes.iter().all(|n| n.start < s.beats));
            }
        }
        // no layering = the medley: the spine's phrase whole
        let w = mashup(&ph, &spine, "slow burn", 0.0, 0.0, 0.0, 3, (0, false));
        assert!(w.slots.iter().all(|s| s.layers.iter().all(|l| l.phrase == s.spine) && s.roll_after == Roll::None));

        // a roll: 8 bars of loops halving, the last two half-bar loops alike; the filter stairs with it
        let src: Vec<Note> = (0..16).map(|i| Note::new(i as f64, 0.5, 60 + (i % 5) as u8, 0.6)).collect();
        let r = roll_notes(&src, 16.0, Roll::Shrink);
        assert!(r.iter().all(|n| n.start < ROLL_BEATS));
        let tail: Vec<u8> = r.iter().filter(|n| n.start >= 28.0).map(|n| n.key).collect();
        assert_eq!(tail, vec![60, 61, 60, 61], "two half-bar loops of the first two beats");
        let (hp, lp) = roll_contour(&[(64.0, Roll::Shrink), (160.0, Roll::Grow)]);
        let at = |pts: &[(f64, f32)], b: f64| crate::model::AutoLane { param: String::new(), points: pts.to_vec() }.value_at(b).unwrap();
        assert!(at(&hp, 60.0) < 0.01 && at(&hp, 66.0) > 0.1 && at(&hp, 95.0) > at(&hp, 81.0) && at(&hp, 96.5) < 0.05);
        assert!(at(&lp, 161.0) < 0.05 && at(&lp, 190.0) > 0.9 && at(&lp, 200.0) > 0.99);
    }

    /// 16 bars: a lead line from `lead` (step `ls` beats), a bass from `bass` (step `bs`), held chords around `pad`; rests every 4 bars
    fn file_reg(tonic: u8, lead: u8, ls: f64, bass: u8, bs: f64, pad: u8) -> ImportedMidi {
        let scale: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];
        let (mut l, mut b, mut c) = (vec![], vec![], vec![]);
        for bar in 0..16 {
            if bar % 4 == 3 {
                continue;
            }
            let b0 = bar as f64 * 4.0;
            let mut t = 0.0;
            let mut i = 0;
            while t < 4.0 - 1e-6 {
                l.push(Note::new(b0 + t, ls * 0.9, lead + tonic + scale[(bar * 3 + i) % 7], 0.8));
                t += ls;
                i += 1;
            }
            let mut t = 0.0;
            let mut i = 0;
            while t < 4.0 - 1e-6 {
                b.push(Note::new(b0 + t, bs * 0.9, bass + tonic + [0u8, 0, 7, 5][i % 4], 0.8));
                t += bs;
                i += 1;
            }
            for d in [0u8, 4, 7] {
                c.push(Note::new(b0, 3.8, pad + tonic + d, 0.6));
            }
        }
        let mk = |notes: Vec<Note>, ch: u8, nm: &str| crate::midi_import::ImportedTrack { name: nm.into(), channel: ch, notes, program: Some(80), ..Default::default() };
        ImportedMidi { tracks: vec![mk(l, 0, "lead"), mk(b, 1, "bass"), mk(c, 2, "chords")], bpm: Some(120.0), tempos: vec![], length_beats: 64.0, ..Default::default() }
    }

    #[test]
    fn mashup_registers_lift_a_muddy_bass_tame_a_shrill_lead_and_spread_the_layers() {
        // a busy bass around C1 (24), a lead way up around C8, chords around middle C
        let mut ph = chop(&file_reg(0, 96, 0.5, 24, 0.25, 60), "a", 2.0);
        ph.extend(chop(&file_reg(7, 96, 0.5, 24, 0.25, 60), "b", 2.0));
        ph.extend(chop(&file_reg(5, 60, 1.0, 36, 1.0, 60), "c", 2.0));
        let spine: Vec<usize> = (0..12).map(|i| i % ph.len()).collect();
        let sm = Smarts { genre: "dubstep".into(), ..Smarts::default() };
        let m = mashup_with(&ph, &spine, "slow burn", 0.0, 0.9, 1.0, 5, (0, false), &sm);
        let log = summarize(&m.log);
        eprintln!("registers: {log}");
        assert!(log.contains("bass up") && log.contains("(was muddy)"), "{log}");
        assert!(log.contains("lead down") && log.contains("(was shrill)"), "{log}");
        let mut basses = 0;
        for s in &m.slots {
            let mut cen: Vec<(f64, bool, Cat)> = vec![];
            for l in s.layers.iter().filter(|l| l.cat != Cat::Drums) {
                let ns = layer_notes(&ph, l, s.beats);
                if ns.is_empty() {
                    continue;
                }
                let r = crate::register::analyze(&ns, s.beats).unwrap();
                // the shape is kept: every note the source's moved by the key shift and whole octaves
                let q = &ph[l.phrase];
                let src = tile(&q.parts[l.part].2, q.beats, s.beats);
                for (a, b) in ns.iter().zip(&src) {
                    assert_eq!(a.key as i32 - b.key as i32, l.shift + 12 * l.octave);
                }
                if l.cat == Cat::Bass {
                    basses += 1;
                    assert!(r.median >= 31.0, "no farting bass: {r:?} {:?}", l.role);
                    assert!(r.median <= 55.0);
                }
                if matches!(l.cat, Cat::Lead | Cat::Arp) {
                    assert!(r.median <= 96.0, "no shrill lead: {r:?}");
                }
                cen.push((r.centre, l.behind, l.cat));
            }
            // no two parts in front share the octave of their centre
            for i in 0..cen.len() {
                for j in 0..i {
                    if cen[i].1 || cen[j].1 {
                        continue;
                    }
                    assert_ne!((cen[i].0 / 12.0).floor(), (cen[j].0 / 12.0).floor(), "{:?} and {:?} share an octave in a slot: {cen:?}", cen[i].2, cen[j].2);
                }
            }
        }
        assert!(basses > 0);
        // registers off: nothing is moved for its register (the bass only kept on the keyboard)
        let off = Smarts { registers: Registers::Off, ..sm.clone() };
        let m2 = mashup_with(&ph, &spine, "slow burn", 0.0, 0.9, 1.0, 5, (0, false), &off);
        assert!(m2.log.iter().all(|l| !l.contains("octave")), "{:?}", m2.log);
        // the guard alone moves the bass and the lead but does not spread the rest
        let gd = Smarts { registers: Registers::Guard, ..sm.clone() };
        let m3 = mashup_with(&ph, &spine, "slow burn", 0.0, 0.9, 1.0, 5, (0, false), &gd);
        assert!(m3.log.iter().any(|l| l.contains("was muddy")) && m3.log.iter().all(|l| !l.contains("own octave")), "{:?}", m3.log);
        // a whole-phrase medley gets the guard too, and keeps its phrase lengths
        let w = mashup_with(&ph, &spine, "slow burn", 0.0, 0.0, 0.0, 5, (0, false), &sm);
        assert!(w.slots.iter().flat_map(|s| s.layers.iter()).any(|l| l.cat == Cat::Bass && l.octave > 0));
        assert!(w.slots.iter().all(|s| s.beats == ph[s.spine].beats));
    }

    #[test]
    fn a_fine_bass_and_a_good_lead_stay_where_they_are() {
        // a slow bass on A1 (33), a lead around C5, chords around G3: the guard leaves them alone
        let mut ph = chop(&file_reg(0, 72, 1.0, 33, 2.0, 55), "a", 2.0);
        ph.extend(chop(&file_reg(0, 72, 1.0, 33, 2.0, 55), "b", 2.0));
        let spine: Vec<usize> = (0..6).map(|i| i % ph.len()).collect();
        let sm = Smarts { registers: Registers::Guard, ..Smarts::default() };
        let m = mashup_with(&ph, &spine, "release", 0.0, 0.5, 0.0, 2, (0, false), &sm);
        assert!(m.slots.iter().flat_map(|s| s.layers.iter()).all(|l| l.octave == 0), "{:?}", m.log);
    }

    #[test]
    fn mashup_smarts_align_phrases_break_down_before_the_last_drop_and_reduce_clashes() {
        assert_eq!(aligned_beats(12.0), 16.0);
        assert_eq!(aligned_beats(20.0), 16.0);
        assert_eq!(aligned_beats(8.0), 8.0);
        assert_eq!(aligned_beats(24.0), 32.0);
        assert_eq!(camelot_steps((0, false), (7, false)), 1);
        assert_eq!(camelot_steps((0, false), (9, true)), 1);
        assert_eq!(camelot_steps((0, false), (0, false)), 0);
        assert_eq!(camelot_steps((0, false), (6, false)), 6);
        // the slot before the last drop breaks down to a pad and the hook, and a roll builds out of it
        let mut ph = chop(&file_with(0, false, "a"), "a", 2.0);
        ph.extend(chop(&file_with(2, false, "b"), "b", 2.0));
        ph.extend(chop(&file_with(9, true, "c"), "c", 2.0));
        let spine: Vec<usize> = (0..14).map(|i| i % ph.len()).collect();
        let m = mashup(&ph, &spine, "triumph", 0.0, 0.9, 0.6, 4, (0, false));
        let b = m.breakdown.unwrap_or_else(|| panic!("a triumph builds to a last drop: {:?}", m.slots.iter().map(|s| s.energy).collect::<Vec<_>>()));
        let s = &m.slots[b];
        assert!(s.energy < 0.4 && s.layers.iter().all(|l| !matches!(l.cat, Cat::Drums | Cat::Bass) || l.spine), "{:?}", s.layers);
        assert_eq!(s.roll_after, Roll::Shrink);
        assert!(m.slots[b + 1].energy >= 0.75);
        // chord-aware fitting: the harsh overlap of the layers is no worse than without it
        let harsh = |m: &Mashup| -> f64 {
            let mut v = vec![];
            for s in &m.slots {
                let all: Vec<Vec<Note>> = s.layers.iter().filter(|l| l.cat != Cat::Drums).map(|l| layer_notes(&ph, l, s.beats)).collect();
                for i in 0..all.len() {
                    let others: Vec<Note> = all.iter().enumerate().filter(|(j, _)| *j != i).flat_map(|(_, x)| x.iter().cloned()).collect();
                    v.push(crate::register::clash(&all[i], &others));
                }
            }
            v.iter().sum::<f64>() / v.len().max(1) as f64
        };
        let on = mashup(&ph, &spine, "storm", 0.0, 0.9, 0.0, 9, (0, false));
        let off = mashup_with(&ph, &spine, "storm", 0.0, 0.9, 0.0, 9, (0, false), &Smarts { clash: false, ..Smarts::default() });
        assert!(harsh(&on) <= harsh(&off) + 1e-9, "{} vs {}", harsh(&on), harsh(&off));
        // the riser climbs in the key and stops before the drop
        let r = riser_notes((9, true), 32.0);
        let k = crate::theory::Key::new(9, true);
        assert!(r.iter().all(|n| k.in_scale(n.key % 12)) && r.last().unwrap().key > r[0].key + 12 && r.iter().all(|n| n.start + n.len <= 31.5 + 1e-9));
    }

    #[test]
    fn a_part_is_transposed_where_it_fits_by_ear() {
        // a D dorian tune (no sharps or flats) that a key finder may call D minor or C major: into C major it needs no move;
        // the same tune a whole step up (E dorian, F# and C#) comes down two semitones, not up ten or to some other fit
        let tune: Vec<Note> = [62u8, 64, 65, 67, 69, 71, 72, 74, 69, 65].iter().enumerate().map(|(i, k)| Note::new(i as f64 * 0.5, 0.5, *k, 0.7)).collect();
        assert_eq!(fit_shift(&tune, 0, (0, false), &[]), 0);
        let up: Vec<Note> = tune.iter().map(|n| Note { key: n.key + 2, ..n.clone() }).collect();
        assert_eq!(fit_shift(&up, 3, (0, false), &[]), -2, "the key finder's guess loses to the fit");
        // against a held C - E - G, a line is moved where it is consonant rather than merely in the scale
        let hold = vec![Note::new(0.0, 4.0, 48, 0.7), Note::new(0.0, 4.0, 52, 0.7), Note::new(0.0, 4.0, 55, 0.7)];
        let line: Vec<Note> = [61u8, 65, 68, 65].iter().enumerate().map(|(i, k)| Note::new(i as f64, 1.0, *k, 0.7)).collect();
        assert_eq!(fit_shift(&line, 0, (0, false), &hold), -1, "C# F G# goes to C E G");
    }

    #[test]
    fn files_chop_into_phrases_with_parts_keys_and_a_smart_order() {
        let a = chop(&file_with(0, false, "a"), "a.mid", 0.0);
        let b = chop(&file_with(9, true, "b"), "b.mid", 0.0);
        let c = chop(&file_with(7, false, "c"), "c.mid", 4.0);
        assert!(a.len() >= 2 && c.len() >= 3, "{} {}", a.len(), c.len());
        assert!(a[0].parts.iter().any(|p| p.0 == Part::Lead) && a[0].parts.iter().any(|p| p.0 == Part::Bass));
        assert_eq!((a[0].tonic, a[0].minor), (0, false));
        assert!(b[0].minor);
        let all: Vec<Phrase> = a.into_iter().chain(b).chain(c).collect();
        let key = common_key(&all);
        // A minor into C major: its relative, no move; G major into C: down a fifth (or up a fourth)
        let am = all.iter().find(|p| p.minor).unwrap();
        assert_eq!(shift_to(am, 0, false), 0);
        let g = all.iter().find(|p| p.tonic == 7).unwrap();
        assert_eq!(shift_to(g, 0, false), 5);
        let o = order(&all, all.len(), Mode::Smart, 1, "", key);
        let mut s = o.clone();
        s.sort();
        s.dedup();
        assert_eq!(s.len(), all.len(), "the smart order plays every phrase once before repeating: {o:?}");
        assert_eq!(order(&all, 6, Mode::Cipher, 0, "pw", key), order(&all, 6, Mode::Cipher, 99, "pw", key));
        assert_ne!(order(&all, 6, Mode::Cipher, 0, "pw", key), order(&all, 6, Mode::Cipher, 0, "other", key));
    }

    #[test]
    fn a_beat_repeat_halves_its_loop_and_ends_on_the_beat() {
        for total in [3.75, 7.5, 15.5, 16.0, 28.0, 32.0, 60.0] {
            let st = beat_repeat(total, 0.125);
            assert!(!st.is_empty());
            let end = st.last().map(|s| s.at + s.len * s.reps as f64).unwrap();
            assert!((end - total).abs() < 1e-9, "{total}: ends at {end}");
            for w in st.windows(2) {
                assert!((w[0].len / 2.0 - w[1].len).abs() < 1e-9, "each loop is half the one before: {st:?}");
                assert!((w[0].at + w[0].len * w[0].reps as f64 - w[1].at).abs() < 1e-9);
            }
            // the lead-in is shorter than one loop of the first stage
            assert!(st[0].at < st[0].len + 1e-9 || total < 8.0, "{total}: {st:?}");
        }
        let long = beat_repeat(32.0, 0.125);
        assert_eq!(long.iter().map(|s| s.len).collect::<Vec<_>>(), vec![4.0, 2.0, 1.0, 0.5, 0.25, 0.125], "a long build goes 1 bar down to 1/32");
        assert!(beat_repeat(8.0, 0.25).iter().all(|s| s.len >= 0.25));
    }
}
