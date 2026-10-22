//! Music theory for the piano roll: key and key-change detection, chord detection with Roman numerals, a role for every note
//! (chord tone / scale tone / chromatic passing / tension / out of key), recurring phrases, next-chord suggestions and an
//! auto-correct that lines a score up with the key and the chords. Pure functions over absolute-time notes; `api.rs` gathers the
//! notes from the project. Everything is deterministic: no model is involved in deciding what is wrong, only in what to write.

pub const SHARP: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
pub const FLAT: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B"];

/// A note in absolute beats (clip start already added).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct N {
    pub start: f64,
    pub len: f64,
    pub key: u8,
    pub vel: f32,
}
impl N {
    pub fn end(&self) -> f64 {
        self.start + self.len
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    pub tonic: u8,
    pub minor: bool,
}

const MAJOR: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];
const MINOR: [u8; 7] = [0, 2, 3, 5, 7, 8, 10];

impl Key {
    pub fn new(tonic: u8, minor: bool) -> Key {
        Key { tonic: tonic % 12, minor }
    }
    /// flats for the keys that are written with them
    pub fn flats(&self) -> bool {
        if self.minor {
            matches!(self.tonic, 2 | 5 | 7 | 10 | 3 | 0) // d g c f bb eb
        } else {
            matches!(self.tonic, 5 | 10 | 3 | 8 | 1 | 6) // F Bb Eb Ab Db Gb
        }
    }
    pub fn pc_name(&self, pc: u8) -> &'static str {
        if self.flats() {
            FLAT[(pc % 12) as usize]
        } else {
            SHARP[(pc % 12) as usize]
        }
    }
    pub fn name(&self) -> String {
        format!("{} {}", self.pc_name(self.tonic), if self.minor { "minor" } else { "major" })
    }
    pub fn scale(&self) -> [u8; 7] {
        let s = if self.minor { MINOR } else { MAJOR };
        let mut out = [0u8; 7];
        for (i, d) in s.iter().enumerate() {
            out[i] = (self.tonic + d) % 12;
        }
        out
    }
    pub fn in_scale(&self, pc: u8) -> bool {
        self.scale().contains(&(pc % 12))
    }
    /// notes a minor key borrows without leaving it: the raised 6th and 7th (melodic / harmonic minor)
    pub fn tolerated(&self, pc: u8) -> bool {
        let rel = (pc % 12 + 12 - self.tonic) % 12;
        self.minor && (rel == 9 || rel == 11)
    }
    pub fn accepts(&self, pc: u8) -> bool {
        self.in_scale(pc) || self.tolerated(pc)
    }
    /// the key a semitone count above this one, same mode
    pub fn transposed(&self, semis: i32) -> Key {
        Key::new(((self.tonic as i32 + semis).rem_euclid(12)) as u8, self.minor)
    }
}

pub fn parse_key(text: &str) -> Option<Key> {
    let t = text.trim().to_lowercase();
    let mut chars = t.chars().peekable();
    let letter = chars.next()?;
    let base = match letter {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'b' => 11,
        _ => return None,
    };
    let mut pc = base;
    let mut rest: String = chars.collect();
    if rest.starts_with('#') || rest.starts_with("sharp") {
        pc += 1;
        rest = rest.trim_start_matches('#').trim_start_matches("sharp").to_string();
    } else if rest.starts_with('b') && !rest.starts_with("blues") || rest.starts_with("flat") {
        pc += 11;
        rest = rest.trim_start_matches("flat").trim_start_matches('b').to_string();
    }
    let rest = rest.trim();
    let minor = rest.starts_with("min") || rest == "m" || rest.starts_with("aeolian") || rest.starts_with("dorian") || rest.starts_with("phrygian");
    Some(Key::new((pc % 12) as u8, minor))
}

// ── key detection (Krumhansl-Schmuckler) ────────────────────────────────────
const PROF_MAJOR: [f64; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
const PROF_MINOR: [f64; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];

fn corr(a: &[f64; 12], b: &[f64; 12]) -> f64 {
    let ma = a.iter().sum::<f64>() / 12.0;
    let mb = b.iter().sum::<f64>() / 12.0;
    let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
    for i in 0..12 {
        num += (a[i] - ma) * (b[i] - mb);
        da += (a[i] - ma).powi(2);
        db += (b[i] - mb).powi(2);
    }
    if da <= 1e-12 || db <= 1e-12 {
        0.0
    } else {
        num / (da * db).sqrt()
    }
}

/// duration-and-velocity weighted pitch-class histogram of the notes sounding in [lo, hi)
pub fn pc_hist(notes: &[N], lo: f64, hi: f64) -> [f64; 12] {
    let mut h = [0.0; 12];
    for n in notes {
        let a = n.start.max(lo);
        let b = n.end().min(hi);
        if b > a {
            // the bass names the tonic: low notes count extra, which also settles relative major / minor
            h[(n.key % 12) as usize] += (b - a).min(4.0) * (0.4 + n.vel as f64) * if n.key < 52 { 1.8 } else { 1.0 };
        }
    }
    h
}

fn key_scores(h: &[f64; 12]) -> [f64; 24] {
    let mut out = [0.0; 24];
    for t in 0..12 {
        let mut rot = [0.0; 12];
        for i in 0..12 {
            rot[(i + t) % 12] = PROF_MAJOR[i];
        }
        out[t] = corr(h, &rot);
        let mut rot = [0.0; 12];
        for i in 0..12 {
            rot[(i + t) % 12] = PROF_MINOR[i];
        }
        out[12 + t] = corr(h, &rot);
    }
    out
}

fn key_of_index(i: usize) -> Key {
    Key::new((i % 12) as u8, i >= 12)
}

/// best key for the notes in [lo, hi): (key, correlation, margin over the runner-up that is not its relative)
pub fn detect_key(notes: &[N], lo: f64, hi: f64) -> (Key, f64, f64) {
    let s = key_scores(&pc_hist(notes, lo, hi));
    let mut best = 0;
    for i in 1..24 {
        if s[i] > s[best] {
            best = i;
        }
    }
    let rel = if best < 12 { 12 + (best + 9) % 12 } else { (best - 12 + 3) % 12 };
    let second = (0..24).filter(|i| *i != best && *i != rel).map(|i| s[i]).fold(-1.0, f64::max);
    (key_of_index(best), s[best], s[best] - second)
}

/// best key for a pitch-class profile (notes or audio chroma): (key, correlation, margin over the runner-up that is not its relative)
pub fn detect_key_hist(h: &[f64; 12]) -> (Key, f64, f64) {
    let s = key_scores(h);
    let mut best = 0;
    for i in 1..24 {
        if s[i] > s[best] {
            best = i;
        }
    }
    let rel = if best < 12 { 12 + (best + 9) % 12 } else { (best - 12 + 3) % 12 };
    let second = (0..24).filter(|i| *i != best && *i != rel).map(|i| s[i]).fold(-1.0, f64::max);
    (key_of_index(best), s[best], s[best] - second)
}

/// the pitch classes a key allows: its scale, plus the leading tone of a minor key (harmonic minor)
pub fn allowed_pcs(k: Key) -> [bool; 12] {
    let mut a = [false; 12];
    for pc in k.scale() {
        a[pc as usize] = true;
    }
    if k.minor {
        a[((k.tonic + 11) % 12) as usize] = true;
    }
    a
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeySection {
    pub start: f64,
    pub end: f64,
    pub key: Key,
    pub score: f64,
}

/// the key over time: a two-bar window per bar, then the cheapest path through the 24 keys where changing key costs something,
/// so a modulation is reported but a stray accidental is not
pub fn key_sections(notes: &[N], total: f64, bar: f64) -> Vec<KeySection> {
    let total = if total.is_finite() { total.clamp(bar, 16384.0) } else { bar };
    let nbars = (total / bar).ceil() as usize;
    if notes.is_empty() {
        return vec![];
    }
    let mut sc = vec![[0.0f64; 24]; nbars];
    let mut empty = vec![false; nbars];
    for b in 0..nbars {
        let lo = (b as f64 - 1.0).max(0.0) * bar;
        let hi = (b as f64 + 2.0) * bar;
        let h = pc_hist(notes, lo, hi);
        empty[b] = h.iter().sum::<f64>() < 0.5;
        sc[b] = key_scores(&h);
    }
    sections_from_scores(sc, empty, bar, total)
}

/// One instrument's say in the key: its notes (relative to the region) and how much its vote counts.
pub struct TrackVote {
    pub notes: Vec<N>,
    pub weight: f64,
}

/// How much a part should count when the song's structure is decided, and what it is: a loud single-note line (a melody) leads, a
/// bass and chords support it, an arpeggio counts least. Loudness scales it.
pub fn prominence(notes: &[N]) -> (&'static str, f64) {
    if notes.is_empty() {
        return ("empty", 0.0);
    }
    let role = classify(notes, false);
    let avg_vel = notes.iter().map(|n| n.vel as f64).sum::<f64>() / notes.len() as f64;
    let loud = 0.7 + 0.6 * avg_vel.clamp(0.0, 1.0);
    let base = match role {
        "lead" => 1.7,
        "bass" => 1.1,
        "chords" | "pad" => 1.0,
        "arp" => 0.8,
        _ => 1.0,
    };
    (role, base * loud)
}

/// The key over time by majority vote: every instrument votes for the key of its own notes in each window, weighted by its
/// prominence, so a pad cannot out-shout a melody and one wandering part cannot drag the song to another key.
pub fn key_sections_vote(tracks: &[TrackVote], total: f64, bar: f64) -> Vec<KeySection> {
    let total = if total.is_finite() { total.clamp(bar, 16384.0) } else { bar };
    let nbars = (total / bar).ceil() as usize;
    if tracks.iter().all(|t| t.notes.is_empty()) {
        return vec![];
    }
    let mut sc = vec![[0.0f64; 24]; nbars];
    let mut empty = vec![false; nbars];
    for b in 0..nbars {
        let lo = (b as f64 - 1.0).max(0.0) * bar;
        let hi = (b as f64 + 2.0) * bar;
        let mut votes = [0.0f64; 24];
        let mut wsum = 0.0;
        for t in tracks {
            let h = pc_hist(&t.notes, lo, hi);
            let mass: f64 = h.iter().sum();
            if mass < 0.5 {
                continue;
            }
            let scores = key_scores(&h);
            let best = (0..24).max_by(|x, y| scores[*x].partial_cmp(&scores[*y]).unwrap()).unwrap();
            let rel = if best < 12 { 12 + (best + 9) % 12 } else { (best - 12 + 3) % 12 };
            // a part with only a few notes in the window says little
            let w = t.weight * (mass / 4.0).min(1.0);
            votes[best] += w * (0.55 + 0.45 * scores[best].max(0.0));
            votes[rel] += w * 0.3;
            wsum += w;
        }
        empty[b] = wsum <= 0.0;
        if wsum > 0.0 {
            for k in 0..24 {
                sc[b][k] = votes[k] / wsum * 1.4;
            }
        }
    }
    sections_from_scores(sc, empty, bar, total)
}

/// The cheapest path through the 24 keys, one per bar window, where changing key costs something, merged into sections.
fn sections_from_scores(mut sc: Vec<[f64; 24]>, empty: Vec<bool>, bar: f64, total: f64) -> Vec<KeySection> {
    let nbars = sc.len();
    // the key of the whole piece is the default: a section leaves it only on real evidence
    let mut sum = [0.0f64; 24];
    for b in 0..nbars {
        for k in 0..24 {
            sum[k] += sc[b][k];
        }
    }
    let global = (0..24).max_by(|a, b| sum[*a].partial_cmp(&sum[*b]).unwrap()).unwrap();
    let global_rel = if global < 12 { 12 + (global + 9) % 12 } else { (global - 12 + 3) % 12 };
    for b in 0..nbars {
        if !empty[b] {
            sc[b][global] += 0.12;
            sc[b][global_rel] += 0.06;
        }
    }
    let penalty = 1.0;
    let mut cost = vec![[0.0f64; 24]; nbars];
    let mut from = vec![[0usize; 24]; nbars];
    for k in 0..24 {
        cost[0][k] = -sc[0][k];
    }
    for b in 1..nbars {
        let (mut m1, mut a1) = (f64::MAX, 0usize);
        for k in 0..24 {
            if cost[b - 1][k] < m1 {
                m1 = cost[b - 1][k];
                a1 = k;
            }
        }
        for k in 0..24 {
            let stay = cost[b - 1][k];
            let jump = m1 + penalty;
            let (c, f) = if stay <= jump { (stay, k) } else { (jump, a1) };
            cost[b][k] = c - if empty[b] { 0.0 } else { sc[b][k] };
            from[b][k] = f;
        }
    }
    let mut k = (0..24).min_by(|a, b| cost[nbars - 1][*a].partial_cmp(&cost[nbars - 1][*b]).unwrap()).unwrap();
    let mut path = vec![0usize; nbars];
    for b in (0..nbars).rev() {
        path[b] = k;
        if b > 0 {
            k = from[b][k];
        }
    }
    let mut out: Vec<KeySection> = vec![];
    for b in 0..nbars {
        let key = key_of_index(path[b]);
        match out.last_mut() {
            Some(l) if l.key == key => {
                l.end = (b + 1) as f64 * bar;
                l.score = l.score.max(sc[b][path[b]]);
            }
            _ => out.push(KeySection { start: b as f64 * bar, end: (b + 1) as f64 * bar, key, score: sc[b][path[b]] }),
        }
    }
    if let Some(l) = out.last_mut() {
        l.end = l.end.max(total);
    }
    out
}

pub fn key_at(sections: &[KeySection], beat: f64) -> Option<Key> {
    sections.iter().find(|s| beat >= s.start && beat < s.end).or(sections.last()).map(|s| s.key)
}

// ── chords ──────────────────────────────────────────────────────────────────
/// (suffix, intervals from the root, complexity penalty)
pub const QUAL: [(&str, &[u8], f64); 14] = [
    ("", &[0, 4, 7], 0.0),
    ("m", &[0, 3, 7], 0.0),
    ("dim", &[0, 3, 6], 0.05),
    ("aug", &[0, 4, 8], 0.08),
    ("sus2", &[0, 2, 7], 0.06),
    ("sus4", &[0, 5, 7], 0.06),
    ("7", &[0, 4, 7, 10], 0.07),
    ("maj7", &[0, 4, 7, 11], 0.07),
    ("m7", &[0, 3, 7, 10], 0.06),
    ("m7b5", &[0, 3, 6, 10], 0.09),
    ("dim7", &[0, 3, 6, 9], 0.10),
    ("6", &[0, 4, 7, 9], 0.12),
    ("add9", &[0, 2, 4, 7], 0.12),
    ("5", &[0, 7], 0.10),
];

pub fn qual_index(suffix: &str) -> Option<usize> {
    QUAL.iter().position(|q| q.0 == suffix)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Chord {
    pub start: f64,
    pub len: f64,
    pub root: u8,
    pub q: usize,
    pub bass: u8,
    /// the chord was inferred from a single melodic line, not played
    pub implied: bool,
}
impl Chord {
    pub fn pcs(&self) -> Vec<u8> {
        QUAL[self.q].1.iter().map(|i| (self.root + i) % 12).collect()
    }
    pub fn name(&self, key: Key) -> String {
        let mut s = key.pc_name(self.root).to_string() + QUAL[self.q].0;
        if self.bass % 12 != self.root && self.pcs().contains(&(self.bass % 12)) {
            s += &format!("/{}", key.pc_name(self.bass));
        }
        s
    }
    pub fn has(&self, pc: u8) -> bool {
        let rel = (pc % 12 + 12 - self.root) % 12;
        QUAL[self.q].1.contains(&rel)
    }
    pub fn end(&self) -> f64 {
        self.start + self.len
    }
}

fn best_chord(w: &[f64; 12], bass: Option<u8>) -> Option<(u8, usize, f64)> {
    let tot: f64 = w.iter().sum();
    if tot < 1e-6 {
        return None;
    }
    let mut best: Option<(u8, usize, f64)> = None;
    for root in 0..12u8 {
        for (qi, (_, iv, pen)) in QUAL.iter().enumerate() {
            let mut inw = 0.0;
            let mut missing = 0.0;
            for (j, i) in iv.iter().enumerate() {
                let x = w[((root + i) % 12) as usize];
                // the added tone of a four-note chord only counts when it is really played (a passing melody note is not a 7th)
                if iv.len() == 4 && j == 3 && x < tot * 0.2 {
                    continue;
                }
                inw += x;
                if x < tot * 0.02 {
                    missing += if *i == 7 && j == 2 { 0.05 } else { 0.13 };
                }
            }
            let out = tot - inw;
            let mut s = inw / tot - 0.75 * out / tot - pen - missing + 0.2 * w[root as usize] / tot;
            if let Some(b) = bass {
                if b % 12 == root {
                    s += 0.12;
                } else if iv.contains(&((b % 12 + 12 - root) % 12)) {
                    s += 0.03;
                }
            }
            if best.map_or(true, |x| s > x.2) {
                best = Some((root, qi, s));
            }
        }
    }
    best.filter(|b| b.2 > 0.2)
}

/// chords over time: every `seg` beats take the pitch classes sounding (lowest note counts extra), match them to the closest
/// chord type, then join neighbours that are the same chord
pub fn detect_chords(notes: &[N], total: f64, seg: f64, implied: bool) -> Vec<Chord> {
    let mut out: Vec<Chord> = vec![];
    let total = if total.is_finite() { total.clamp(0.0, 16384.0) } else { 0.0 };
    let nseg = (total / seg).ceil() as usize;
    for s in 0..nseg {
        let lo = s as f64 * seg;
        let hi = lo + seg;
        let w = pc_hist(notes, lo, hi);
        let mut bass: Option<u8> = None;
        for n in notes {
            if n.end() > lo + seg * 0.25 && n.start < hi - seg * 0.1 && bass.map_or(true, |b| n.key < b) {
                bass = Some(n.key);
            }
        }
        let mut w2 = w;
        if let Some(b) = bass {
            w2[(b % 12) as usize] += w.iter().sum::<f64>() * 0.12;
        }
        if let Some((root, q, _)) = best_chord(&w2, bass) {
            let c = Chord { start: lo, len: seg, root, q, bass: bass.unwrap_or(root), implied };
            match out.last_mut() {
                Some(l) if l.root == c.root && l.q == c.q && (l.end() - c.start).abs() < 1e-6 => l.len += seg,
                _ => out.push(c),
            }
        }
    }
    out
}

pub fn chord_at(chords: &[Chord], beat: f64) -> Option<&Chord> {
    chords.iter().find(|c| beat >= c.start - 1e-6 && beat < c.end() - 1e-6)
}

/// Roman numeral of a chord in a key: case = quality, accidental = distance from the scale ("bVII", "#iv°")
pub fn roman(c: &Chord, key: Key) -> String {
    let rel = ((c.root + 12 - key.tonic) % 12) as usize;
    let table_major = ["I", "bII", "II", "bIII", "III", "IV", "#IV", "V", "bVI", "VI", "bVII", "VII"];
    let table_minor = ["I", "bII", "II", "III", "#III", "IV", "#IV", "V", "VI", "#VI", "VII", "#VII"];
    let base = if key.minor { table_minor[rel] } else { table_major[rel] };
    let small = matches!(QUAL[c.q].0, "m" | "dim" | "m7" | "m7b5" | "dim7");
    let (acc, num): (&str, &str) = match base.strip_prefix('b') {
        Some(r) => ("b", r),
        None => match base.strip_prefix('#') {
            Some(r) => ("#", r),
            None => ("", base),
        },
    };
    let num = if small { num.to_lowercase() } else { num.to_string() };
    let suffix = match QUAL[c.q].0 {
        "dim" => "°",
        "aug" => "+",
        "7" | "m7" => "7",
        "maj7" => "maj7",
        "m7b5" => "ø7",
        "dim7" => "°7",
        "sus2" => "sus2",
        "sus4" => "sus4",
        "6" => "6",
        "add9" => "add9",
        "5" => "5",
        _ => "",
    };
    format!("{acc}{num}{suffix}")
}

pub fn chord_diatonic(c: &Chord, key: Key) -> bool {
    c.pcs().iter().all(|p| key.accepts(*p))
}

// ── note roles ──────────────────────────────────────────────────────────────
pub const R_CHORD: u8 = 0;
pub const R_SCALE: u8 = 1;
pub const R_PASS: u8 = 2;
pub const R_TENSION: u8 = 3;
pub const R_OUT: u8 = 4;

fn nearest_in_key(key: Key, chord: Option<&Chord>, k: u8, prefer_dir: i32) -> u8 {
    let mut best = k;
    let mut cost = f64::MAX;
    for d in -3i32..=3 {
        let c = k as i32 + d;
        if !(0..=127).contains(&c) {
            continue;
        }
        let pc = (c % 12) as u8;
        if !key.in_scale(pc) {
            continue;
        }
        let mut x = d.abs() as f64;
        if chord.map_or(false, |ch| ch.has(pc)) {
            x -= 0.6;
        }
        if prefer_dir != 0 && d.signum() == prefer_dir {
            x -= 0.2;
        }
        if x < cost {
            cost = x;
            best = c as u8;
        }
    }
    best
}

/// (role, suggested replacement key or -1) for every note, in the order given
pub fn note_roles(notes: &[N], keys: &[KeySection], chords: &[Chord]) -> Vec<(u8, i32)> {
    let mut order: Vec<usize> = (0..notes.len()).collect();
    order.sort_by(|a, b| notes[*a].start.partial_cmp(&notes[*b].start).unwrap().then(notes[*a].key.cmp(&notes[*b].key)));
    let mut rank = vec![0usize; notes.len()];
    for (r, i) in order.iter().enumerate() {
        rank[*i] = r;
    }
    let mut out = Vec::with_capacity(notes.len());
    for (i, n) in notes.iter().enumerate() {
        let Some(key) = key_at(keys, n.start) else {
            out.push((R_SCALE, -1));
            continue;
        };
        let chord = chord_at(chords, n.start + 0.001);
        let pc = n.key % 12;
        let strong = (n.start % 2.0).abs() < 0.01 || ((n.start % 1.0).abs() < 0.01 && n.len >= 1.0);
        if chord.map_or(false, |c| c.has(pc)) {
            out.push((R_CHORD, -1));
        } else if key.accepts(pc) {
            // an in-key note a half step above a chord tone, held on a strong beat, grinds against the chord
            let clash = strong && n.len >= 0.5 && chord.map_or(false, |c| c.has((pc + 11) % 12) && !c.has((pc + 1) % 12));
            // the note a half step below is a chord tone; it is only a better note when it is also in the key (a chord borrowed from another
            // key would otherwise pull a diatonic melody note onto a chromatic one)
            let down = n.key.saturating_sub(1);
            if clash && key.accepts(down % 12) {
                out.push((R_TENSION, down as i32));
            } else {
                out.push((R_SCALE, -1));
            }
        } else {
            let r = rank[i];
            let prev = (0..r).rev().map(|j| &notes[order[j]]).find(|p| (p.start - n.start).abs() > 0.01);
            let next = (r + 1..order.len()).map(|j| &notes[order[j]]).find(|p| (p.start - n.start).abs() > 0.01);
            let near = |o: Option<&N>| o.map_or(false, |p| (p.key as i32 - n.key as i32).abs() <= 2 && key.accepts(p.key % 12));
            // an out-of-key note that steps by a half step onto a note of the key (a leading tone or an auxiliary) is an ornament
            // (the neighbours that matter are the next and previous notes on a DIFFERENT pitch: a repeated note or a trill is one figure)
            let next_diff = (r + 1..order.len()).map(|j| &notes[order[j]]).find(|p| p.key != n.key && p.start - n.start > 0.01);
            let prev_diff = (0..r).rev().map(|j| &notes[order[j]]).find(|p| p.key != n.key && n.start - p.start > 0.01);
            let resolves = |o: Option<&N>| o.map_or(false, |p| (p.key as i32 - n.key as i32).abs() == 1 && key.accepts(p.key % 12) && (p.start - n.start).abs() <= 2.5);
            let ornament = n.len <= 0.76 && (resolves(next_diff) || resolves(prev_diff));
            if (n.len <= 0.76 && near(prev) && near(next)) || ornament {
                out.push((R_PASS, -1));
            } else {
                let dir = match (prev, next) {
                    (Some(p), Some(q)) if q.key > p.key => 1,
                    (Some(p), Some(q)) if q.key < p.key => -1,
                    _ => 0,
                };
                out.push((R_OUT, nearest_in_key(key, chord, n.key, dir) as i32));
            }
        }
    }
    out
}

// ── next chord ──────────────────────────────────────────────────────────────
const NEXT_MAJOR: [[f64; 7]; 7] = [
    [0.10, 0.60, 0.35, 0.90, 0.90, 0.75, 0.10],
    [0.20, 0.05, 0.10, 0.40, 1.00, 0.30, 0.30],
    [0.10, 0.40, 0.05, 0.70, 0.30, 0.80, 0.05],
    [0.70, 0.50, 0.10, 0.05, 0.90, 0.30, 0.10],
    [1.00, 0.10, 0.20, 0.50, 0.05, 0.60, 0.10],
    [0.30, 0.80, 0.30, 0.80, 0.70, 0.05, 0.05],
    [1.00, 0.10, 0.40, 0.10, 0.10, 0.30, 0.05],
];
const NEXT_MINOR: [[f64; 7]; 7] = [
    [0.10, 0.20, 0.60, 0.80, 0.70, 0.85, 0.85],
    [0.30, 0.05, 0.10, 0.30, 1.00, 0.10, 0.20],
    [0.40, 0.10, 0.05, 0.60, 0.40, 0.80, 0.60],
    [0.70, 0.10, 0.30, 0.05, 0.90, 0.40, 0.60],
    [1.00, 0.10, 0.20, 0.30, 0.05, 0.70, 0.10],
    [0.50, 0.10, 0.50, 0.70, 0.60, 0.05, 0.90],
    [0.90, 0.10, 0.80, 0.40, 0.40, 0.60, 0.05],
];

/// scale-degree index (0..6) of a diatonic chord root, else None
pub fn degree_of(root: u8, key: Key) -> Option<usize> {
    key.scale().iter().position(|p| *p == root % 12)
}

/// chords that work after `prev` (None = at the start): (root, quality index, roman, score), best first
pub fn next_chords(key: Key, prev: Option<&Chord>, history: &[Chord]) -> Vec<(u8, usize, String, f64)> {
    let sc = key.scale();
    let mut cand: Vec<(u8, usize, f64)> = vec![];
    let from = prev.and_then(|p| degree_of(p.root, key));
    for d in 0..7 {
        let root = sc[d];
        // diatonic triad quality from the scale
        let third = (sc[(d + 2) % 7] + 12 - root) % 12;
        let fifth = (sc[(d + 4) % 7] + 12 - root) % 12;
        let mut q = match (third, fifth) {
            (4, 7) => 0,
            (3, 7) => 1,
            (3, 6) => 2,
            (4, 8) => 3,
            _ => 0,
        };
        // the dominant of a minor key is major (harmonic minor)
        if key.minor && d == 4 {
            q = 0;
        }
        let base = match from {
            Some(f) => {
                if key.minor {
                    NEXT_MINOR[f][d]
                } else {
                    NEXT_MAJOR[f][d]
                }
            }
            None => {
                if d == 0 {
                    1.0
                } else if d == 3 || d == 4 {
                    0.5
                } else {
                    0.3
                }
            }
        };
        let mut s = base;
        // not the same chord three times, and a small bonus for chords not used lately
        let recent = history.iter().rev().take(4).filter(|c| c.root == root).count() as f64;
        s -= 0.12 * recent;
        cand.push((root, q, s));
    }
    // borrowed colour
    if !key.minor {
        for (rel, q, s) in [(10u8, 0usize, 0.45), (8, 0, 0.30), (5, 1, 0.38)] {
            let after_sub = prev.map_or(false, |p| (p.root + 12 - key.tonic) % 12 == 5 || (p.root + 12 - key.tonic) % 12 == 0);
            cand.push(((key.tonic + rel) % 12, q, if after_sub { s } else { s * 0.5 }));
        }
    }
    let mut out: Vec<(u8, usize, String, f64)> = cand
        .into_iter()
        .map(|(root, q, s)| {
            let c = Chord { start: 0.0, len: 1.0, root, q, bass: root, implied: false };
            (root, q, roman(&c, key), s)
        })
        .collect();
    out.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap());
    out.truncate(8);
    out
}

/// pitches for a chord voiced near `prev` (smooth voice leading), starting around middle C when there is nothing before it
pub fn voice(root: u8, q: usize, prev: &[u8], lo: u8, hi: u8) -> Vec<u8> {
    let pcs: Vec<u8> = QUAL[q].1.iter().map(|i| (root + i) % 12).collect();
    let mut out: Vec<u8> = vec![];
    if prev.is_empty() {
        let base = (lo as i32 + ((root as i32 - lo as i32).rem_euclid(12))) as u8;
        let mut x = base;
        for i in QUAL[q].1 {
            x = base + i;
            if x <= hi {
                out.push(x);
            }
        }
        let _ = x;
        return out;
    }
    let centre = prev.iter().map(|p| *p as f64).sum::<f64>() / prev.len() as f64;
    for pc in pcs {
        let mut best = None;
        let mut bd = f64::MAX;
        for k in lo..=hi {
            if k % 12 == pc {
                let d = prev.iter().map(|p| (*p as f64 - k as f64).abs()).fold(f64::MAX, f64::min) + (k as f64 - centre).abs() * 0.15;
                if d < bd {
                    bd = d;
                    best = Some(k);
                }
            }
        }
        if let Some(k) = best {
            if !out.contains(&k) {
                out.push(k);
            }
        }
    }
    out.sort();
    out
}

// ── phrases ─────────────────────────────────────────────────────────────────
#[derive(Clone, Debug, PartialEq)]
pub struct Phrase {
    pub start: f64,
    pub end: f64,
    pub kind: &'static str, // progression | motif | sequence | cadence | run | arpeggio
    pub label: String,
}

/// the top line of a texture: the highest note at each onset
fn top_line(notes: &[N]) -> Vec<N> {
    let mut v: Vec<N> = notes.to_vec();
    v.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap().then(b.key.cmp(&a.key)));
    let mut out: Vec<N> = vec![];
    for n in v {
        if out.last().map_or(true, |l| (l.start - n.start).abs() > 0.02) {
            out.push(n);
        }
    }
    out
}

fn rhythm_class(len: f64) -> i32 {
    // 0 thirty-second .. 5 whole-ish
    if len < 0.2 {
        0
    } else if len < 0.4 {
        1
    } else if len < 0.8 {
        2
    } else if len < 1.6 {
        3
    } else if len < 3.2 {
        4
    } else {
        5
    }
}

pub fn phrases(notes: &[N], chords: &[Chord], keys: &[KeySection]) -> Vec<Phrase> {
    let mut out: Vec<Phrase> = vec![];
    // chord progression loops
    let key0 = keys.first().map(|k| k.key).unwrap_or(Key::new(0, false));
    let rom: Vec<String> = chords.iter().map(|c| roman(c, key_at(keys, c.start).unwrap_or(key0))).collect();
    let mut taken = vec![false; chords.len()];
    for plen in (2..=8usize).rev() {
        let mut i = 0;
        while i + plen * 2 <= chords.len() {
            if taken[i..i + plen].iter().any(|t| *t) {
                i += 1;
                continue;
            }
            let pat = &rom[i..i + plen];
            let mut reps = 1;
            while i + (reps + 1) * plen <= chords.len() && &rom[i + reps * plen..i + (reps + 1) * plen] == pat {
                reps += 1;
            }
            let distinct = pat.iter().collect::<std::collections::HashSet<_>>().len();
            if reps >= 2 && distinct >= 2 {
                let end = chords[i + reps * plen - 1].end();
                out.push(Phrase { start: chords[i].start, end, kind: "progression", label: format!("{} x{}", pat.join(" "), reps) });
                for t in taken[i..i + reps * plen].iter_mut() {
                    *t = true;
                }
                i += reps * plen;
            } else {
                i += 1;
            }
        }
    }
    // cadences
    for i in 1..chords.len() {
        let k = key_at(keys, chords[i].start).unwrap_or(key0);
        let (a, b) = (degree_of(chords[i - 1].root, k), degree_of(chords[i].root, k));
        let rel_b = (chords[i].root + 12 - k.tonic) % 12;
        let rel_a = (chords[i - 1].root + 12 - k.tonic) % 12;
        let name = match (rel_a, rel_b) {
            (7, 0) => Some("authentic cadence V-I"),
            (5, 0) => Some("plagal cadence IV-I"),
            (7, 9) | (7, 8) => Some("deceptive cadence V-vi"),
            (10, 0) if !k.minor => Some("backdoor cadence bVII-I"),
            (10, 0) | (2, 0) if k.minor && a.is_some() && b.is_some() => Some("modal cadence VII-i"),
            _ => None,
        };
        if let Some(n) = name {
            out.push(Phrase { start: chords[i - 1].start, end: chords[i].end(), kind: "cadence", label: n.to_string() });
        }
    }
    // melodic motifs: repeated interval + rhythm patterns in the top line
    let tl = top_line(notes);
    let sig = |i: usize, n: usize| -> Vec<(i32, i32)> { (0..n).map(|j| (tl[i + j + 1].key as i32 - tl[i + j].key as i32, rhythm_class(tl[i + j].len))).collect() };
    let mut used = vec![false; tl.len()];
    let mut letter = 0u8;
    for n in (4..=8usize).rev() {
        if tl.len() < n + 3 {
            continue;
        }
        let mut i = 0;
        while i + n < tl.len() {
            if used[i..=i + n].iter().any(|u| *u) {
                i += 1;
                continue;
            }
            let s = sig(i, n);
            if s.iter().all(|x| x.0 == 0) {
                i += 1;
                continue;
            }
            let mut hits = vec![i];
            let mut j = i + n;
            while j + n < tl.len() {
                if !used[j..=j + n].iter().any(|u| *u) && sig(j, n) == s {
                    hits.push(j);
                    j += n;
                } else {
                    j += 1;
                }
            }
            if hits.len() >= 2 {
                let name = (b'A' + letter % 26) as char;
                letter += 1;
                let firsts: Vec<i32> = hits.iter().map(|h| tl[*h].key as i32).collect();
                let step = firsts[1] - firsts[0];
                let seq = hits.len() >= 3 && step != 0 && firsts.windows(2).all(|w| w[1] - w[0] == step);
                for h in &hits {
                    for u in used[*h..=*h + n].iter_mut() {
                        *u = true;
                    }
                    let label = if seq {
                        format!("motif {name} ({} sequence)", if step > 0 { "rising" } else { "falling" })
                    } else if tl[*h].key as i32 != firsts[0] {
                        format!("motif {name} (transposed)")
                    } else {
                        format!("motif {name}")
                    };
                    out.push(Phrase { start: tl[*h].start, end: tl[*h + n].end(), kind: if seq { "sequence" } else { "motif" }, label });
                }
                i = hits[0] + 1;
            } else {
                i += 1;
            }
        }
    }
    // scale runs and arpeggios
    let mut i = 0;
    while i + 3 < tl.len() {
        let d = tl[i + 1].key as i32 - tl[i].key as i32;
        if d != 0 && d.abs() <= 2 {
            let mut j = i + 1;
            while j + 1 < tl.len() && (tl[j + 1].key as i32 - tl[j].key as i32).signum() == d.signum() && (tl[j + 1].key as i32 - tl[j].key as i32).abs() <= 2 && tl[j + 1].start - tl[j].start < 1.01 {
                j += 1;
            }
            if j - i >= 3 {
                out.push(Phrase { start: tl[i].start, end: tl[j].end(), kind: "run", label: format!("scale run {}", if d > 0 { "up" } else { "down" }) });
                i = j;
                continue;
            }
        }
        i += 1;
    }
    let mut i = 0;
    while i + 3 < tl.len() {
        let mut j = i;
        let c = chord_at(chords, tl[i].start);
        while j + 1 < tl.len() && (tl[j + 1].key as i32 - tl[j].key as i32).abs() >= 3 && (tl[j + 1].key as i32 - tl[j].key as i32).abs() <= 9 && c.map_or(false, |c| c.has(tl[j + 1].key % 12)) && tl[j + 1].start - tl[j].start < 1.01 {
            j += 1;
        }
        if j - i >= 3 {
            out.push(Phrase { start: tl[i].start, end: tl[j].end(), kind: "arpeggio", label: "arpeggio".into() });
            i = j;
        } else {
            i += 1;
        }
    }
    out.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap());
    out
}

// ── auto-correct ────────────────────────────────────────────────────────────
/// What the fixer knows beyond the notes themselves: how strict to be (0 = chaotic like jazz or breakcore, 1 = strict and predictable),
/// which notes the other instruments also play at that moment, and which notes are part of a figure that keeps coming back.
pub struct FixCtx<'a> {
    pub order: f64,
    pub support: &'a [bool],
    pub repeated: &'a [bool],
    /// per note: it grinds (a half step or a major seventh) against something another instrument is playing at that moment
    pub clash: &'a [bool],
    /// what the other instruments play, ordered by start, in the same time as the notes; empty = nothing is known about them
    pub others: &'a [N],
}

/// Key sections shorter than `min_len` beats are detection noise more often than modulations (a window of a few bars at the start
/// that leans toward the dominant key, say): they take the key of their neighbour, and equal neighbours merge.
pub fn merge_short_keys(keys: Vec<KeySection>, min_len: f64) -> Vec<KeySection> {
    if keys.len() < 2 {
        return keys;
    }
    let mut v = keys;
    loop {
        let short = v.iter().position(|k| k.end - k.start < min_len);
        let Some(i) = short else { break };
        if v.len() < 2 {
            break;
        }
        // into the longer neighbour
        let into = match (i.checked_sub(1), v.get(i + 1)) {
            (Some(p), Some(nx)) => {
                if v[p].end - v[p].start >= nx.end - nx.start {
                    p
                } else {
                    i + 1
                }
            }
            (Some(p), None) => p,
            (None, Some(_)) => i + 1,
            _ => break,
        };
        let (start, end) = (v[i].start.min(v[into].start), v[i].end.max(v[into].end));
        v[into].start = start;
        v[into].end = end;
        v.remove(i);
        // equal neighbours merge
        let mut k = 0;
        while k + 1 < v.len() {
            if v[k].key == v[k + 1].key {
                v[k].end = v[k + 1].end;
                v.remove(k + 1);
            } else {
                k += 1;
            }
        }
    }
    v
}

/// Is the pitch class of notes[i] a member of a major or minor triad that the notes around it (within two beats, same instrument) spell?
/// An arpeggio D - F# - A makes F# a chord tone even when no other instrument plays a D chord.
pub fn in_local_triad(notes: &[N], i: usize) -> bool {
    let n = &notes[i];
    let mut pcs = [false; 12];
    for o in notes.iter() {
        if (o.start - n.start).abs() <= 2.0 {
            pcs[(o.key % 12) as usize] = true;
        }
    }
    let pc = (n.key % 12) as usize;
    for r in 0..12usize {
        if !pcs[r] || !pcs[(r + 7) % 12] {
            continue;
        }
        for third in [3usize, 4] {
            if pcs[(r + third) % 12] && [r, (r + third) % 12, (r + 7) % 12].contains(&pc) {
                return true;
            }
        }
    }
    false
}

/// does a note of pitch `key` over [start, start + len) grind (minor second / major seventh) against any of the other instruments' notes?
pub fn clashes_with_others(others: &[N], start: f64, len: f64, key: u8) -> bool {
    // a trill, a grace note or a fast run is an ornament, not a held clash
    if len < 0.5 {
        return false;
    }
    let lo = others.partition_point(|o| o.start < start - 8.0);
    for o in others[lo..].iter().take_while(|o| o.start < start + len) {
        if o.len < 0.2 {
            continue;
        }
        let overlap = (o.end().min(start + len)) - o.start.max(start);
        if overlap < 0.25 {
            continue;
        }
        // a half step (or a major seventh / minor ninth) only grinds when the two notes are close: spread over more than an
        // octave and a bit it is just a wide chord voicing
        let apart = (o.key as i32 - key as i32).abs();
        let d = apart.rem_euclid(12);
        if (d == 1 || d == 11) && apart <= 14 {
            return true;
        }
    }
    false
}

pub const FIX_KEY: u32 = 1;
pub const FIX_TENSION: u32 = 2;
pub const FIX_TIMING: u32 = 4;
pub const FIX_DUPES: u32 = 8;
pub const FIX_PASSING: u32 = 16;

#[derive(Default, Debug, Clone, PartialEq)]
pub struct FixReport {
    pub key: usize,
    pub tension: usize,
    pub timing: usize,
    pub dupes: usize,
    pub passing: usize,
}
impl FixReport {
    pub fn total(&self) -> usize {
        self.key + self.tension + self.timing + self.dupes + self.passing
    }
}

/// the grid a score is on: the coarsest of 1/4, 1/8, 1/16 (and triplets) that most onsets sit within a tolerance of
pub fn detect_grid(starts: &[f64]) -> f64 {
    if starts.is_empty() {
        return 0.25;
    }
    for g in [1.0, 0.5, 0.25, 1.0 / 3.0, 1.0 / 6.0, 0.125] {
        let ok = starts.iter().filter(|s| {
            let r = (*s / g).round() * g;
            (*s - r).abs() < g * 0.18 + 0.01
        });
        if ok.count() as f64 >= starts.len() as f64 * 0.8 {
            return g;
        }
    }
    0.25
}

/// Line a clip's notes up with the key and the chords. `notes` are the clip's own (relative-start) notes, `abs_offset` is the
/// clip's start so the harmony (absolute) can be looked up. Returns the corrected notes (same order) and what changed.
pub fn autocorrect(notes: &[N], abs_offset: f64, keys: &[KeySection], chords: &[Chord], mask: u32) -> (Vec<N>, Vec<bool>, FixReport) {
    autocorrect_ctx(notes, abs_offset, keys, chords, mask, &FixCtx { order: 0.75, support: &[], repeated: &[], clash: &[], others: &[] })
}

/// `autocorrect` that weighs order against chaos: a wrong-looking note is only fixed when its dissonance, after what the rest of the
/// arrangement and the clip's own repetition say in its defence, is above a tolerance that loosens as `order` falls.
pub fn autocorrect_ctx(notes: &[N], abs_offset: f64, keys: &[KeySection], chords: &[Chord], mask: u32, ctx: &FixCtx) -> (Vec<N>, Vec<bool>, FixReport) {
    let order = ctx.order.clamp(0.0, 1.0);
    let abs: Vec<N> = notes.iter().map(|n| N { start: n.start + abs_offset, ..*n }).collect();
    let roles = note_roles(&abs, keys, chords);
    let mut out = notes.to_vec();
    let mut changed = vec![false; notes.len()];
    let mut rep = FixReport::default();
    let tolerance = 0.15 + (1.0 - order) * 0.7;
    for (i, (role, sug)) in roles.iter().enumerate() {
        let wanted = match *role {
            R_OUT => mask & FIX_KEY != 0,
            R_TENSION => mask & FIX_TENSION != 0,
            R_PASS => mask & FIX_PASSING != 0,
            _ => false,
        };
        let mut d: f64 = match *role {
            R_OUT => 1.0,
            R_TENSION => 0.7,
            R_PASS => 0.45,
            _ => 0.0,
        };
        if d > 0.0 {
            if ctx.support.get(i) == Some(&true) {
                d -= 0.45; // another instrument is playing the same pitch right then: it is harmony, not a mistake
            }
            if ctx.repeated.get(i) == Some(&true) {
                d -= 0.3; // it comes back in the same place again and again: it is the figure
            }
            if abs[i].len < 0.3 {
                d -= 0.1;
            }
            if abs[i].len >= 1.0 && (abs[i].start % 2.0).abs() < 0.01 {
                d += 0.1;
            }
        }
        // Evidence from the accompaniment: unless the order is strict, a note is only called wrong when it actually grinds against what
        // the other instruments play. A chromatic note that agrees with the harmony around it (a leading tone, a borrowed chord) is music.
        let know_others = !ctx.others.is_empty();
        let strict = order > 0.85;
        let grinds = ctx.clash.get(i) == Some(&true);
        // a diatonic note rubbing a chord tone is a suspension or a seventh, ordinary harmony: only Strict touches those
        // a long note outside the key that nothing around it explains (no triad spelled by its neighbours) is a mistake even when no
        // other instrument grinds against it; a short one is an ornament
        let exposed = abs[i].len >= 0.75 && !in_local_triad(&abs, i);
        let evidence = if *role == R_TENSION { strict || !know_others } else { !know_others || strict || ((grinds || exposed) && !in_local_triad(&abs, i)) || *role == R_PASS };
        let fix = wanted && d > tolerance && evidence;
        if fix {
            let key = key_at(keys, abs[i].start).unwrap_or(Key::new(0, false));
            let mut target = if *role == R_PASS {
                // chromatic passing notes only when asked: pull them to the nearest scale tone
                nearest_in_key(key, chord_at(chords, abs[i].start + 0.001), abs[i].key, 0) as i32
            } else {
                *sug
            };
            // a fix must not make a new clash: look for the nearest in-key pitch that is clean, or leave the note alone
            if know_others && target >= 0 && clashes_with_others(ctx.others, abs[i].start, abs[i].len, target as u8) {
                let mut found = -1;
                for dlt in [1i32, -1, 2, -2] {
                    let cand = *sug + dlt;
                    if (0..=127).contains(&cand) && key.accepts((cand % 12) as u8) && !clashes_with_others(ctx.others, abs[i].start, abs[i].len, cand as u8) {
                        found = cand;
                        break;
                    }
                }
                target = found;
            }
            if target >= 0 && target != out[i].key as i32 {
                out[i].key = target as u8;
                changed[i] = true;
                match *role {
                    R_OUT => rep.key += 1,
                    R_TENSION => rep.tension += 1,
                    _ => rep.passing += 1,
                }
            }
        }
    }
    if mask & FIX_TIMING != 0 {
        let starts: Vec<f64> = notes.iter().map(|n| n.start).collect();
        let g = detect_grid(&starts);
        for (i, n) in out.iter_mut().enumerate() {
            let near = (n.start / g).round() * g;
            let mut moved = false;
            // only notes that are a little off the grid move; a deliberate offset is left alone
            if (near - n.start).abs() > 1e-4 && (near - n.start).abs() <= g * 0.35 * (0.3 + 0.7 * order) {
                n.start = near;
                moved = true;
            }
            // an end that nearly meets a grid line is pulled onto it; articulation (a short or detached note) stays
            let end = n.start + n.len;
            let e = (end / g).round() * g;
            if (e - end).abs() > 1e-4 && (e - end).abs() <= 0.02 && e - n.start >= 0.03 {
                n.len = e - n.start;
                moved = true;
            }
            if moved {
                rep.timing += 1;
                changed[i] = true;
            }
        }
    }
    if mask & FIX_DUPES != 0 {
        // two notes on the same pitch starting together: keep the longer; same pitch overlapping: shorten the earlier one
        let mut idx: Vec<usize> = (0..out.len()).collect();
        idx.sort_by(|a, b| out[*a].key.cmp(&out[*b].key).then(out[*a].start.partial_cmp(&out[*b].start).unwrap()));
        for w in idx.windows(2) {
            let (a, b) = (w[0], w[1]);
            if out[a].key == out[b].key && out[a].len > 0.0 && out[b].len > 0.0 {
                if (out[a].start - out[b].start).abs() < 0.01 {
                    let (keep, drop) = if out[a].len >= out[b].len { (a, b) } else { (b, a) };
                    let _ = keep;
                    out[drop].len = 0.0;
                    changed[drop] = true;
                    rep.dupes += 1;
                } else if out[a].start + out[a].len > out[b].start + 0.001 {
                    out[a].len = (out[b].start - out[a].start).max(0.03);
                    changed[a] = true;
                    rep.dupes += 1;
                }
            }
        }
    }
    (out, changed, rep)
}

/// retune notes from one key to another keeping scale degrees, so a melody written in A minor becomes the same melody in
/// D minor (or in D major: same degrees, major scale); notes outside the old scale move by the plain interval
pub fn retune(notes: &[N], from: Key, to: Key) -> Vec<N> {
    let fs = if from.minor { MINOR } else { MAJOR };
    let ts = if to.minor { MINOR } else { MAJOR };
    notes
        .iter()
        .map(|n| {
            let rel = (n.key as i32 - from.tonic as i32).rem_euclid(12);
            let oct = (n.key as i32 - from.tonic as i32 - rel) / 12;
            let new_rel = match fs.iter().position(|d| *d as i32 == rel) {
                Some(deg) => ts[deg] as i32,
                None => rel,
            };
            let mut k = to.tonic as i32 + new_rel + oct * 12;
            // keep the register: move by whole octaves toward the old pitch
            while k - n.key as i32 > 6 {
                k -= 12;
            }
            while n.key as i32 - k > 6 {
                k += 12;
            }
            N { key: k.clamp(0, 127) as u8, ..*n }
        })
        .collect()
}

/// a rough role for a set of notes, used by the style thumbprint: drums | bass | chords | arp | lead | pad
pub fn classify(notes: &[N], drums: bool) -> &'static str {
    if drums {
        return "drums";
    }
    if notes.is_empty() {
        return "empty";
    }
    let mut keys: Vec<u8> = notes.iter().map(|n| n.key).collect();
    keys.sort();
    let med = keys[keys.len() / 2];
    let avg_len = notes.iter().map(|n| n.len).sum::<f64>() / notes.len() as f64;
    // polyphony: notes sharing an onset
    let mut onsets: Vec<f64> = notes.iter().map(|n| (n.start * 8.0).round() / 8.0).collect();
    onsets.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut distinct = onsets.clone();
    distinct.dedup();
    let poly = notes.len() as f64 / distinct.len().max(1) as f64;
    if med < 50 && poly < 1.6 {
        "bass"
    } else if poly >= 2.0 && avg_len >= 1.5 {
        "pad"
    } else if poly >= 2.0 {
        "chords"
    } else if avg_len < 0.4 && med >= 52 {
        "arp"
    } else if med < 50 {
        "bass"
    } else {
        "lead"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(start: f64, len: f64, key: u8) -> N {
        N { start, len, key, vel: 0.8 }
    }

    /// C major scale up and down with a C major / G / Am / F progression underneath
    fn song() -> Vec<N> {
        let mut v = vec![];
        for (bar, chord) in [[48u8, 52, 55], [43, 47, 50], [45, 48, 52], [41, 45, 48]].iter().enumerate() {
            for k in chord {
                v.push(n(bar as f64 * 4.0, 4.0, *k));
            }
        }
        for (i, k) in [72u8, 74, 76, 77, 79, 77, 76, 74, 72, 71, 72, 74, 76, 72, 69, 72].iter().enumerate() {
            v.push(n(i as f64, 0.9, *k));
        }
        v
    }

    #[test]
    fn detects_c_major_and_names_chords() {
        let notes = song();
        let (k, score, _) = detect_key(&notes, 0.0, 16.0);
        assert_eq!(k, Key::new(0, false), "{score}");
        let keys = key_sections(&notes, 16.0, 4.0);
        assert_eq!(keys.len(), 1, "{keys:?}");
        let chords = detect_chords(&notes, 16.0, 4.0, false);
        let names: Vec<String> = chords.iter().map(|c| c.name(k)).collect();
        assert_eq!(names, ["C", "G", "Am", "F"], "{names:?}");
        let rom: Vec<String> = chords.iter().map(|c| roman(c, k)).collect();
        assert_eq!(rom, ["I", "V", "vi", "IV"]);
    }

    #[test]
    fn finds_the_modulation() {
        let mut v = vec![];
        for i in 0..16 {
            v.push(n(i as f64, 0.9, [60u8, 64, 67, 64, 60, 62, 64, 60][i % 8]));
        }
        // then up a whole step to D major: F# and C# appear
        for i in 0..16 {
            v.push(n(16.0 + i as f64, 0.9, [62u8, 66, 69, 66, 62, 64, 66, 62][i % 8]));
        }
        let keys = key_sections(&v, 32.0, 4.0);
        assert!(keys.len() >= 2, "{keys:?}");
        assert_eq!(keys[0].key, Key::new(0, false));
        assert_eq!(keys.last().unwrap().key, Key::new(2, false));
    }

    #[test]
    fn marks_the_out_of_key_note_and_suggests_the_fix() {
        let mut notes = song();
        // a C# held for a beat and a half on a strong beat in the melody over C major
        notes.push(n(2.0, 1.5, 73));
        let keys = key_sections(&notes, 16.0, 4.0);
        let chords = detect_chords(&notes, 16.0, 4.0, false);
        let roles = note_roles(&notes, &keys, &chords);
        let last = roles.len() - 1;
        assert_eq!(roles[last].0, R_OUT, "{roles:?}");
        assert!(roles[last].1 == 72 || roles[last].1 == 74, "{:?}", roles[last]);
        // the real melody stays clean
        assert!(roles[..last].iter().all(|r| r.0 != R_OUT), "{roles:?}");
    }

    #[test]
    fn a_short_chromatic_note_between_scale_notes_is_a_passing_note() {
        let mut notes = song();
        notes.push(n(0.5, 0.25, 73)); // C# between C5 and D5? (72 at 0, 74 at 1)
        let keys = key_sections(&notes, 16.0, 4.0);
        let chords = detect_chords(&notes, 16.0, 4.0, false);
        let roles = note_roles(&notes, &keys, &chords);
        assert_eq!(roles.last().unwrap().0, R_PASS, "{roles:?}");
    }

    #[test]
    fn autocorrect_pulls_stray_notes_into_the_key_and_tidies_timing() {
        let mut notes = song();
        notes.push(n(2.0, 1.5, 73));
        notes.push(n(5.03, 0.9, 74)); // slightly off the grid
        let keys = key_sections(&notes, 16.0, 4.0);
        let chords = detect_chords(&notes, 16.0, 4.0, false);
        let (fixed, changed, rep) = autocorrect(&notes, 0.0, &keys, &chords, FIX_KEY | FIX_TIMING);
        assert!(rep.key >= 1 && rep.timing >= 1, "{rep:?}");
        assert!(fixed.iter().all(|x| Key::new(0, false).in_scale(x.key % 12)), "all notes now in key");
        assert!(changed.iter().any(|c| *c));
        assert!((fixed.last().unwrap().start - 5.0).abs() < 1e-9);
    }

    #[test]
    fn suggests_the_dominant_after_the_subdominant() {
        let k = Key::new(0, false);
        let prev = Chord { start: 0.0, len: 4.0, root: 5, q: 0, bass: 5, implied: false };
        let next = next_chords(k, Some(&prev), &[]);
        assert_eq!(next[0].2, "V", "{next:?}");
        assert!(next.iter().any(|c| c.2 == "I"));
        let start = next_chords(Key::new(9, true), None, &[]);
        assert_eq!(start[0].2, "i");
    }

    #[test]
    fn finds_a_repeated_progression_and_a_cadence() {
        let k = Key::new(0, false);
        let mut chords = vec![];
        for (i, r) in [9u8, 5, 0, 7, 9, 5, 0, 7].iter().enumerate() {
            chords.push(Chord { start: i as f64 * 4.0, len: 4.0, root: *r, q: if *r == 9 { 1 } else { 0 }, bass: *r, implied: false });
        }
        let keys = vec![KeySection { start: 0.0, end: 32.0, key: k, score: 1.0 }];
        let ph = phrases(&[], &chords, &keys);
        assert!(ph.iter().any(|p| p.kind == "progression" && p.label.starts_with("vi IV I V x2")), "{ph:?}");
        assert!(ph.iter().any(|p| p.kind == "cadence"), "{ph:?}");
    }

    #[test]
    fn finds_a_repeated_motif_and_a_run() {
        let mut v = vec![];
        for rep in 0..3 {
            for (i, k) in [60u8, 64, 62, 67, 65].iter().enumerate() {
                v.push(n(rep as f64 * 8.0 + i as f64 * 0.5, 0.4, *k + rep as u8 * 0));
            }
        }
        for (i, k) in [60u8, 62, 64, 65, 67].iter().enumerate() {
            v.push(n(30.0 + i as f64 * 0.5, 0.4, *k));
        }
        let ph = phrases(&v, &[], &[KeySection { start: 0.0, end: 40.0, key: Key::new(0, false), score: 1.0 }]);
        assert!(ph.iter().filter(|p| p.label.starts_with("motif")).count() >= 3, "{ph:?}");
        assert!(ph.iter().any(|p| p.kind == "run"), "{ph:?}");
    }

    #[test]
    fn retune_keeps_scale_degrees() {
        let a_minor = [n(0.0, 1.0, 69), n(1.0, 1.0, 72), n(2.0, 1.0, 76)];
        let d_minor = retune(&a_minor, Key::new(9, true), Key::new(2, true));
        assert_eq!(d_minor.iter().map(|x| x.key).collect::<Vec<_>>(), [74, 77, 81]);
        let c_major = retune(&[n(0.0, 1.0, 69), n(1.0, 1.0, 72), n(2.0, 1.0, 76)], Key::new(9, true), Key::new(0, false));
        assert_eq!(c_major.iter().map(|x| x.key).collect::<Vec<_>>(), [72, 76, 79]);
    }

    #[test]
    fn key_names_parse() {
        assert_eq!(parse_key("F# minor"), Some(Key::new(6, true)));
        assert_eq!(parse_key("Bb major"), Some(Key::new(10, false)));
        assert_eq!(parse_key("a minor").unwrap().name(), "A minor");
        assert_eq!(Key::new(10, false).name(), "Bb major");
    }
}
