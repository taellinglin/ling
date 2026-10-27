//! Ling Voice — a formant singer driven by IPA lyrics: monophonic, or up to eight voices when the `voices` knob is up.
//!
//! Source-filter model: a glottal pulse train (pitch from the MIDI note, vibrato, portamento) is shaped by five
//! cascaded formant resonators whose frequencies glide between phonemes; consonants add noise bursts, frication and
//! nasal murmur. Each note carries a lyric (see `lyrics.rs`): one or more IPA syllables sharing the note, with a `~`
//! prefix meaning "legato": no new start, the pitch slurs into the note and the vowel carries over. A note with no
//! lyric sings the default vowel; `~` alone sustains the previous vowel.
//!
//! It is a vocal *instrument*, not a text-to-speech engine: intelligible-ish, robotic, and fully offline.
use super::{midi_hz, Instrument, Rng, TAU};
use std::sync::OnceLock;

const NAMES: [&str; 30] = [
    "vowel", "gender", "bright", "breath", "whisp", "vib rate", "vib depth", "vib delay", "attack", "release", "glide", "cons", "gain", "pan",
    "voices", "choir", "open", "fry", "drift", "scoop", "ring", "trem", "nasal", "arp", "arp mode", "arp rate", "arp gate", "arp oct",
    "tune", "expr",
];
const P_VOWEL: usize = 0;
const P_GENDER: usize = 1;
const P_BRIGHT: usize = 2;
const P_BREATH: usize = 3;
const P_WHISP: usize = 4;
const P_VRATE: usize = 5;
const P_VDEPTH: usize = 6;
const P_VDELAY: usize = 7;
const P_ATT: usize = 8;
const P_REL: usize = 9;
const P_GLIDE: usize = 10;
const P_CONS: usize = 11;
const P_GAIN: usize = 12;
const P_PAN: usize = 13;
/// 0 = one voice, up to 8 notes at once
const P_VOICES: usize = 14;
/// three detuned glottal sources through one tract
const P_CHOIR: usize = 15;
/// glottal open quotient: pressed ... breathy
const P_OPEN: usize = 16;
/// vocal fry: every other pulse weaker and irregular
const P_FRY: usize = 17;
/// pitch wander of a human voice
const P_DRIFT: usize = 18;
/// each note slides up to pitch from below
const P_SCOOP: usize = 19;
/// singer's formant (the ring around 3 kHz)
const P_RING: usize = 20;
/// amplitude vibrato
const P_TREM: usize = 21;
/// constant nasality
const P_NASAL: usize = 22;
/// arpeggiator: on / off
const P_ARP: usize = 23;
/// 0..1 picks up, down, up-down, random, played order
const P_ARP_MODE: usize = 24;
/// 0..1 picks 1/4, 1/8, 1/8 triplet, 1/16, 1/16 triplet, 1/32
const P_ARP_RATE: usize = 25;
/// how much of each step the note sounds
const P_ARP_GATE: usize = 26;
/// 0..1 = one to three octaves
const P_ARP_OCT: usize = 27;
/// intonation: 0.5 in tune, 0 / 1 = a whole tone flat / sharp. Automated per note it draws scoops, falls and the
/// cents a singer colours a word with (expression.rs writes it)
const P_TUNE: usize = 28;
/// expression (like MIDI CC 11): the level within a note, for swells and tapers across a held line; 1 = full
const P_EXPR: usize = 29;
/// cents either way at the ends of `tune`
const TUNE_RANGE: f32 = 200.0;

/// beats per arpeggiator step for each `arp rate` position
const ARP_BEATS: [f64; 6] = [1.0, 0.5, 1.0 / 3.0, 0.25, 1.0 / 6.0, 0.125];

// ── phonemes ─────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Vowel,
    Glide,
    Nasal,
    Liquid,
    Stop,
    Fric,
    Affr,
    Aspir,
}

#[derive(Clone, Debug)]
pub struct Ph {
    pub kind: Kind,
    pub voiced: bool,
    /// vowel / sonorant formants F1..F3 (Hz, male reference)
    pub f: [f32; 3],
    /// voicing amplitude for sonorants
    pub amp: f32,
    /// frication / burst: centre, bandwidth, amplitude
    pub nf: f32,
    pub nbw: f32,
    pub namp: f32,
    pub dur: f32,
    pub aspirated: bool,
    pub unreleased: bool,
    pub long: bool,
    pub nasalized: bool,
    pub tense: bool,
}

fn base(kind: Kind) -> Ph {
    Ph { kind, voiced: true, f: [500.0, 1500.0, 2500.0], amp: 1.0, nf: 0.0, nbw: 1000.0, namp: 0.0, dur: 0.05, aspirated: false, unreleased: false, long: false, nasalized: false, tense: false }
}

fn vowel(f1: f32, f2: f32, f3: f32) -> Ph {
    Ph { f: [f1, f2, f3], ..base(Kind::Vowel) }
}
fn sonorant(kind: Kind, f1: f32, f2: f32, f3: f32, amp: f32, dur: f32) -> Ph {
    Ph { f: [f1, f2, f3], amp, dur, ..base(kind) }
}
fn stop(voiced: bool, nf: f32, nbw: f32) -> Ph {
    Ph { voiced, nf, nbw, namp: 0.55, dur: 0.045, amp: 0.0, ..base(Kind::Stop) }
}
fn fric(voiced: bool, nf: f32, nbw: f32, namp: f32, dur: f32) -> Ph {
    Ph { voiced, nf, nbw, namp, dur, amp: if voiced { 0.4 } else { 0.0 }, ..base(Kind::Fric) }
}

/// Look up one IPA base symbol.
fn lookup(sym: &str) -> Option<Ph> {
    Some(match sym {
        // vowels (F1 / F2 / F3)
        "i" => vowel(270.0, 2290.0, 3010.0),
        "y" => vowel(270.0, 1900.0, 2200.0),
        "ɨ" => vowel(300.0, 1700.0, 2400.0),
        "ʉ" => vowel(290.0, 1500.0, 2200.0),
        "ɯ" => vowel(300.0, 1350.0, 2400.0),
        "u" => vowel(300.0, 870.0, 2240.0),
        "ɪ" => vowel(390.0, 1990.0, 2550.0),
        "ʏ" => vowel(400.0, 1600.0, 2400.0),
        "ʊ" => vowel(440.0, 1020.0, 2240.0),
        "e" => vowel(390.0, 2090.0, 2800.0),
        "ø" => vowel(390.0, 1600.0, 2300.0),
        "ɘ" | "ə" => vowel(500.0, 1500.0, 2500.0),
        "ɵ" => vowel(350.0, 1100.0, 2200.0),
        "ɤ" => vowel(460.0, 1200.0, 2400.0),
        "o" => vowel(400.0, 800.0, 2400.0),
        "ɛ" => vowel(530.0, 1840.0, 2480.0),
        "œ" => vowel(540.0, 1400.0, 2300.0),
        "ɜ" => vowel(540.0, 1350.0, 2400.0),
        "ʌ" => vowel(640.0, 1190.0, 2390.0),
        "ɔ" => vowel(570.0, 840.0, 2410.0),
        "æ" => vowel(660.0, 1720.0, 2410.0),
        "ɐ" => vowel(650.0, 1300.0, 2400.0),
        "a" => vowel(730.0, 1090.0, 2440.0),
        "ä" => vowel(720.0, 1250.0, 2440.0),
        "ɑ" => vowel(750.0, 1000.0, 2400.0),
        "ɒ" => vowel(700.0, 800.0, 2400.0),
        "ɚ" | "ɝ" => vowel(500.0, 1350.0, 1650.0),
        // glides
        "j" => sonorant(Kind::Glide, 270.0, 2290.0, 3010.0, 0.85, 0.05),
        "w" => sonorant(Kind::Glide, 300.0, 870.0, 2240.0, 0.85, 0.05),
        "ɥ" => sonorant(Kind::Glide, 270.0, 1900.0, 2200.0, 0.85, 0.05),
        "ɰ" => sonorant(Kind::Glide, 300.0, 1350.0, 2400.0, 0.85, 0.05),
        // liquids
        "l" => sonorant(Kind::Liquid, 360.0, 1300.0, 2700.0, 0.8, 0.05),
        "ɭ" | "ɫ" => sonorant(Kind::Liquid, 350.0, 900.0, 2500.0, 0.8, 0.05),
        "ʎ" => sonorant(Kind::Liquid, 300.0, 2000.0, 3000.0, 0.8, 0.055),
        "ɾ" | "ɽ" => sonorant(Kind::Liquid, 400.0, 1400.0, 2300.0, 0.4, 0.03),
        "r" | "ʀ" => sonorant(Kind::Liquid, 400.0, 1400.0, 2300.0, 0.4, 0.07),
        "ɹ" | "ɻ" => sonorant(Kind::Liquid, 350.0, 1200.0, 1600.0, 0.8, 0.05),
        // nasals
        "m" => sonorant(Kind::Nasal, 250.0, 1100.0, 2500.0, 0.6, 0.06),
        "n" => sonorant(Kind::Nasal, 250.0, 1700.0, 2600.0, 0.6, 0.06),
        "ɲ" => sonorant(Kind::Nasal, 250.0, 2100.0, 2800.0, 0.6, 0.06),
        "ŋ" => sonorant(Kind::Nasal, 250.0, 1500.0, 2500.0, 0.6, 0.06),
        "ɴ" => sonorant(Kind::Nasal, 250.0, 1300.0, 2400.0, 0.6, 0.06),
        // stops
        "p" => stop(false, 1100.0, 1800.0),
        "b" => stop(true, 1100.0, 1800.0),
        "t" => stop(false, 4500.0, 2500.0),
        "d" => stop(true, 4500.0, 2500.0),
        "k" | "q" => stop(false, 2200.0, 1600.0),
        "g" | "ɡ" => stop(true, 2200.0, 1600.0),
        "c" => stop(false, 3200.0, 2000.0),
        "ɟ" => stop(true, 3200.0, 2000.0),
        "ʈ" => stop(false, 3500.0, 2200.0),
        "ɖ" => stop(true, 3500.0, 2200.0),
        "ʔ" => Ph { dur: 0.035, namp: 0.0, ..stop(false, 1000.0, 1000.0) },
        // fricatives
        "f" => fric(false, 5500.0, 4500.0, 0.12, 0.09),
        "v" => fric(true, 5500.0, 4500.0, 0.1, 0.08),
        "ɸ" => fric(false, 4500.0, 4000.0, 0.09, 0.09),
        "β" => fric(true, 4500.0, 4000.0, 0.08, 0.08),
        "θ" => fric(false, 6000.0, 4000.0, 0.1, 0.09),
        "ð" => fric(true, 6000.0, 4000.0, 0.08, 0.07),
        "s" => fric(false, 7500.0, 3500.0, 0.34, 0.11),
        "z" => fric(true, 7500.0, 3500.0, 0.26, 0.09),
        "ʃ" | "ʂ" => fric(false, 3500.0, 2200.0, 0.34, 0.11),
        "ʒ" | "ʐ" => fric(true, 3500.0, 2200.0, 0.26, 0.09),
        "ɕ" => fric(false, 6000.0, 2500.0, 0.3, 0.1),
        "ʑ" => fric(true, 6000.0, 2500.0, 0.24, 0.09),
        "ç" => fric(false, 5000.0, 2500.0, 0.22, 0.09),
        "x" | "χ" => fric(false, 2500.0, 1500.0, 0.24, 0.09),
        "ɣ" | "ʁ" => fric(true, 2500.0, 1500.0, 0.18, 0.07),
        "h" | "ɦ" => Ph { dur: 0.07, namp: 0.0, amp: 0.0, voiced: sym == "ɦ", ..base(Kind::Aspir) },
        _ => return None,
    })
}

/// Parse the symbols of one syllable into phonemes (affricates, aspiration ʰ, palatalisation ʲ, length ː, ...).
pub fn phonemes(syl: &str) -> Vec<Ph> {
    let chars: Vec<char> = syl.chars().filter(|c| *c != '͡' && *c != '͜' && *c != '.' && *c != 'ˈ' && *c != 'ˌ').collect();
    let mut out: Vec<Ph> = vec![];
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // modifiers apply to the previous phoneme
        match c {
            'ʰ' | 'ʱ' => {
                if let Some(p) = out.last_mut() {
                    p.aspirated = true;
                }
                i += 1;
                continue;
            }
            'ː' | 'ˑ' => {
                if let Some(p) = out.last_mut() {
                    p.long = true;
                }
                i += 1;
                continue;
            }
            '\u{031A}' => {
                if let Some(p) = out.last_mut() {
                    p.unreleased = true;
                }
                i += 1;
                continue;
            }
            '\u{0348}' => {
                if let Some(p) = out.last_mut() {
                    p.tense = true;
                }
                i += 1;
                continue;
            }
            '\u{0303}' => {
                if let Some(p) = out.last_mut() {
                    p.nasalized = true;
                }
                i += 1;
                continue;
            }
            'ʲ' => {
                if let Some(mut g) = lookup("j") {
                    g.dur = 0.03;
                    out.push(g);
                }
                i += 1;
                continue;
            }
            'ʷ' => {
                if let Some(mut g) = lookup("w") {
                    g.dur = 0.03;
                    out.push(g);
                }
                i += 1;
                continue;
            }
            _ => {}
        }
        // affricates: two symbols, one phoneme
        let nx: Option<char> = chars.get(i + 1).copied();
        let pair: Option<String> = nx.map(|n| format!("{c}{n}"));
        let aff = match pair.as_deref() {
            Some("tʃ") | Some("ʈʂ") => Some(("ʃ", false)),
            Some("dʒ") | Some("ɖʐ") => Some(("ʒ", true)),
            Some("ts") => Some(("s", false)),
            Some("dz") => Some(("z", true)),
            Some("tɕ") => Some(("ɕ", false)),
            Some("dʑ") => Some(("ʑ", true)),
            Some("pf") => Some(("f", false)),
            _ => None,
        };
        if let Some((fr, voiced)) = aff {
            if let Some(mut p) = lookup(fr) {
                p.kind = Kind::Affr;
                p.voiced = voiced;
                p.amp = if voiced { 0.3 } else { 0.0 };
                p.dur = 0.1;
                out.push(p);
            }
            i += 2;
            continue;
        }
        if let Some(p) = lookup(&c.to_string()) {
            out.push(p);
        }
        i += 1;
    }
    out
}

// ── score: what the voice does over one note ───────────────────────────────────

/// A target for the vocal tract, held for `dur` samples (with a smoothing time towards it).
#[derive(Clone, Debug)]
struct Seg {
    dur: u32,
    /// formants F1..F3 (Hz at the reference voice) — F4 / F5 are fixed
    f: [f32; 3],
    /// voiced (glottal) amplitude 0..1
    voice: f32,
    /// aspiration through the tract
    asp: f32,
    /// frication noise (centre Hz, bandwidth Hz, amplitude)
    fric: (f32, f32, f32),
    /// nasal murmur 0..1
    nasal: f32,
    /// noise burst at the start of this segment: (centre, bw, amp)
    burst: Option<(f32, f32, f32)>,
    /// smoothing time constant towards this target, seconds
    tau: f32,
}

fn vowel_seg(f: [f32; 3], dur: u32, nasal: f32) -> Seg {
    Seg { dur, f, voice: 1.0, asp: 0.0, fric: (0.0, 1000.0, 0.0), nasal, burst: None, tau: 0.018 }
}

struct Syllable {
    onset: Vec<Ph>,
    nucleus: Vec<Ph>,
    coda: Vec<Ph>,
}

fn is_vowel_like(p: &Ph) -> bool {
    p.kind == Kind::Vowel
}

fn split_syllable(s: &str) -> Syllable {
    let ph = phonemes(s);
    let first_v = ph.iter().position(is_vowel_like);
    match first_v {
        Some(v) => {
            let mut end = v;
            while end < ph.len() && is_vowel_like(&ph[end]) {
                end += 1;
            }
            // an off-glide (j / w) right after the vowels is part of the nucleus when nothing follows it but consonants
            let mut nucleus: Vec<Ph> = ph[v..end].to_vec();
            while end < ph.len() && ph[end].kind == Kind::Glide && ph.get(end + 1).map(|n| n.kind != Kind::Vowel).unwrap_or(true) {
                let mut g = ph[end].clone();
                g.kind = Kind::Vowel;
                nucleus.push(g);
                end += 1;
            }
            Syllable { onset: ph[..v].to_vec(), nucleus, coda: ph[end..].to_vec() }
        }
        None => {
            // no vowel: a syllabic nasal / liquid carries the note
            match ph.iter().position(|p| matches!(p.kind, Kind::Nasal | Kind::Liquid)) {
                Some(n) => {
                    let mut nuc = ph[n].clone();
                    nuc.dur = 0.2;
                    Syllable { onset: ph[..n].to_vec(), nucleus: vec![nuc], coda: ph[n + 1..].to_vec() }
                }
                None => Syllable { onset: ph, nucleus: vec![], coda: vec![] },
            }
        }
    }
}

/// Segments for one consonant. `next_f` = formants of the following vowel (aspiration takes its colour).
fn consonant_segs(p: &Ph, next_f: [f32; 3], sr: f32, strength: f32, in_coda: bool) -> Vec<Seg> {
    let n = |sec: f32| ((sec * strength.max(0.3)) * sr).max(8.0) as u32;
    let mut v: Vec<Seg> = vec![];
    let silent = |dur: u32, voice: f32| Seg { dur, f: next_f, voice, asp: 0.0, fric: (0.0, 1000.0, 0.0), nasal: 0.0, burst: None, tau: 0.008 };
    match p.kind {
        Kind::Stop => {
            let closure = if p.tense { 0.07 } else { 0.04 };
            // voiced stops keep a quiet murmur through the closure
            v.push(silent(n(closure), if p.voiced { 0.2 } else { 0.0 }));
            if p.nf > 0.0 && p.namp > 0.0 && !(in_coda && p.unreleased) {
                v.push(Seg { dur: n(0.012), f: next_f, voice: 0.0, asp: 0.0, fric: (p.nf, p.nbw, p.namp), nasal: 0.0, burst: Some((p.nf, p.nbw, p.namp * 1.4)), tau: 0.002 });
                if !p.voiced && !p.tense {
                    // voice onset time: aspiration colours the following vowel
                    let vot = if p.aspirated { 0.055 } else { 0.022 };
                    v.push(Seg { dur: n(vot), f: next_f, voice: 0.0, asp: 0.55, fric: (0.0, 1000.0, 0.0), nasal: 0.0, burst: None, tau: 0.004 });
                }
            }
        }
        Kind::Fric => {
            v.push(Seg { dur: n(p.dur), f: next_f, voice: p.amp, asp: 0.0, fric: (p.nf, p.nbw, p.namp), nasal: 0.0, burst: None, tau: 0.01 });
            if p.aspirated {
                v.push(Seg { dur: n(0.03), f: next_f, voice: 0.0, asp: 0.4, fric: (0.0, 1000.0, 0.0), nasal: 0.0, burst: None, tau: 0.004 });
            }
        }
        Kind::Affr => {
            v.push(silent(n(0.04), if p.voiced { 0.2 } else { 0.0 }));
            v.push(Seg { dur: n(p.dur - 0.04 + 0.03), f: next_f, voice: p.amp, asp: 0.0, fric: (p.nf, p.nbw, p.namp), nasal: 0.0, burst: Some((p.nf, p.nbw, p.namp * 0.8)), tau: 0.004 });
            if p.aspirated {
                v.push(Seg { dur: n(0.04), f: next_f, voice: 0.0, asp: 0.45, fric: (0.0, 1000.0, 0.0), nasal: 0.0, burst: None, tau: 0.004 });
            }
        }
        Kind::Aspir => {
            v.push(Seg { dur: n(p.dur), f: next_f, voice: if p.voiced { 0.5 } else { 0.0 }, asp: 0.6, fric: (0.0, 1000.0, 0.0), nasal: 0.0, burst: None, tau: 0.006 });
        }
        Kind::Nasal => {
            v.push(Seg { dur: n(p.dur), f: p.f, voice: p.amp, asp: 0.0, fric: (0.0, 1000.0, 0.0), nasal: 1.0, burst: None, tau: 0.012 });
        }
        Kind::Liquid | Kind::Glide => {
            let taps = if p.kind == Kind::Liquid && p.dur > 0.06 && p.f[0] > 390.0 { 2 } else { 1 };
            for _ in 0..taps {
                v.push(Seg { dur: n(p.dur / taps as f32), f: p.f, voice: p.amp, asp: 0.0, fric: (0.0, 1000.0, 0.0), nasal: 0.0, burst: None, tau: 0.01 });
            }
        }
        Kind::Vowel => {}
    }
    v
}

/// One syllable of length `total` samples: onset consonants, the vowel, coda consonants.
fn syllable_segs(syl: &Syllable, total: u32, sr: f32, strength: f32, tied: bool) -> Vec<Seg> {
    let nuc_f: [f32; 3] = syl.nucleus.first().map(|p| p.f).unwrap_or([600.0, 1300.0, 2500.0]);
    let last_f: [f32; 3] = syl.nucleus.last().map(|p| p.f).unwrap_or(nuc_f);
    let nasal_v = syl.nucleus.first().map(|p| if p.nasalized { 0.45 } else { 0.0 }).unwrap_or(0.0);
    let mut onset: Vec<Seg> = vec![];
    for p in &syl.onset {
        // a legato note cannot restart the voice: only voiced sonorants survive
        if tied && !matches!(p.kind, Kind::Nasal | Kind::Liquid | Kind::Glide) {
            continue;
        }
        onset.extend(consonant_segs(p, nuc_f, sr, strength, false));
    }
    let mut coda: Vec<Seg> = vec![];
    for p in &syl.coda {
        coda.extend(consonant_segs(p, last_f, sr, strength, true));
    }
    let consonant: u32 = onset.iter().chain(coda.iter()).map(|s| s.dur).sum();
    // consonants may take at most ~45% of the syllable
    let cap = (total as f32 * 0.45) as u32;
    if consonant > cap && consonant > 0 {
        let k = cap as f32 / consonant as f32;
        for s in onset.iter_mut().chain(coda.iter_mut()) {
            s.dur = ((s.dur as f32 * k) as u32).max(4);
        }
    }
    let consonant: u32 = onset.iter().chain(coda.iter()).map(|s| s.dur).sum();
    let nuc_total = total.saturating_sub(consonant).max((sr * 0.03) as u32);
    let mut out = onset;
    // vowels: diphthongs split the time, the first vowel gets the larger share
    let nv = syl.nucleus.len().max(1);
    if syl.nucleus.is_empty() {
        out.push(vowel_seg(nuc_f, nuc_total, 0.0));
    } else {
        let mut left = nuc_total;
        for (i, p) in syl.nucleus.iter().enumerate() {
            let share = if nv == 1 {
                left
            } else if i == 0 {
                (nuc_total as f32 * 0.62) as u32
            } else if i + 1 == nv {
                left
            } else {
                (nuc_total as f32 * 0.2) as u32
            };
            let share = share.min(left).max(4);
            left = left.saturating_sub(share);
            let mut seg = vowel_seg(p.f, share, nasal_v);
            // syllabic nasal / liquid keeps its colour
            if matches!(p.kind, Kind::Nasal) {
                seg.nasal = 1.0;
                seg.voice = p.amp;
            }
            out.push(seg);
        }
    }
    out.extend(coda);
    out
}

// ── the instrument ──────────────────────────────────────────────────────────

#[derive(Clone, Copy, Default)]
struct Res {
    y1: f32,
    y2: f32,
}
impl Res {
    #[inline]
    fn run(&mut self, x: f32, f: f32, bw: f32, sr: f32) -> f32 {
        let f = f.clamp(40.0, sr * 0.45);
        let c = -(-TAU * bw / sr).exp();
        let b = 2.0 * (-std::f32::consts::PI * bw / sr).exp() * (TAU * f / sr).cos();
        let a = 1.0 - b - c;
        let y = a * x + b * self.y1 + c * self.y2;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

pub struct Voice {
    sr: f32,
    v: [f32; NAMES.len()],
    rng: Rng,
    held: Vec<u8>,
    gate: bool,
    /// current / target pitch in MIDI notes
    note: f32,
    target: f32,
    vel: f32,
    pan: f32,
    phase: f32,
    vib_t: f32,
    jitter: f32,
    env: f32,
    // score
    score: Vec<Seg>,
    seg_i: usize,
    seg_left: u32,
    live_coda: Vec<Seg>,
    sustaining: bool,
    // smoothed tract state
    f: [f32; 3],
    voice_amp: f32,
    asp_amp: f32,
    fric: (f32, f32, f32),
    nasal: f32,
    tau: f32,
    // burst
    burst: Option<(f32, f32, f32)>,
    burst_env: f32,
    // dsp state
    res: [Res; 5],
    fric_res: Res,
    nasal_res: Res,
    prev_flow: f32,
    tilt: f32,
    /// running power of the vocal tract output and the gain that levels every vowel to the same loudness
    agc_pow: f32,
    agc_gain: f32,
    /// second smoothing stage of the amplitude targets (steps never reach the signal as a click)
    voice_s: f32,
    asp_s: f32,
    fric_s: f32,
    burst_res: Res,
    burst_t: f32,
    dc: f32,
    // polyphony: voice 0 is this one, the others live in `pool`; `slot_key[s]` is the note slot s sings
    pool: Vec<Voice>,
    slot_key: Vec<Option<u8>>,
    slot_age: Vec<u64>,
    /// voices already given a note since the last render: a chord of tied notes must not all slur onto the same voice
    claimed: Vec<bool>,
    clock: u64,
    /// level trim when several voices are summed
    trim: f32,
    // choir / fry / scoop state
    phase2: f32,
    phase3: f32,
    prev2: f32,
    prev3: f32,
    jit2: f32,
    jit3: f32,
    cyc: bool,
    cyc_gain: f32,
    scoop_c: f32,
    /// `tune` (cents) and `expr` (level) smoothed per sample: lanes step every 64 frames and must not zipper
    tune_s: f32,
    expr_s: f32,
    ring_res: Res,
    // arpeggiator: the keys held (key, velocity, pan) in the order they went down, the lyric of the latest one, the step clock
    bpm: f64,
    arp_keys: Vec<(u8, f32, f32)>,
    arp_lyric: String,
    arp_idx: usize,
    arp_cur: Option<u8>,
    arp_left: u64,
    arp_gate_left: u64,
}

impl Voice {
    pub const PRESETS: [&'static str; 11] = ["aria", "mei", "kai", "ren", "choir", "whisper", "robot", "ensemble", "smoky", "gospel", "arpeggio"];

    pub fn new(sr: f32) -> Self {
        let mut v = [0.0f32; NAMES.len()];
        for (k, d) in defaults().iter().enumerate() {
            v[k] = *d;
        }
        Voice {
            sr,
            v,
            rng: Rng::new(0x5eed1234),
            held: vec![],
            gate: false,
            note: 60.0,
            target: 60.0,
            vel: 0.8,
            pan: 0.0,
            phase: 0.0,
            vib_t: 0.0,
            jitter: 0.0,
            env: 0.0,
            score: vec![],
            seg_i: 0,
            seg_left: 0,
            live_coda: vec![],
            sustaining: false,
            f: [730.0, 1090.0, 2440.0],
            voice_amp: 0.0,
            asp_amp: 0.0,
            fric: (0.0, 1000.0, 0.0),
            nasal: 0.0,
            tau: 0.02,
            burst: None,
            burst_env: 0.0,
            res: [Res::default(); 5],
            fric_res: Res::default(),
            nasal_res: Res::default(),
            prev_flow: 0.0,
            tilt: 0.0,
            agc_pow: 0.01,
            agc_gain: 0.3,
            voice_s: 0.0,
            asp_s: 0.0,
            fric_s: 0.0,
            burst_res: Res::default(),
            burst_t: 1.0,
            dc: 0.0,
            pool: vec![],
            slot_key: vec![None],
            slot_age: vec![0],
            claimed: vec![false],
            clock: 0,
            trim: 1.0,
            phase2: 0.31,
            phase3: 0.67,
            prev2: 0.0,
            prev3: 0.0,
            jit2: 0.0,
            jit3: 0.0,
            cyc: false,
            cyc_gain: 1.0,
            scoop_c: 0.0,
            tune_s: 0.0,
            expr_s: 1.0,
            ring_res: Res::default(),
            bpm: 120.0,
            arp_keys: vec![],
            arp_lyric: String::new(),
            arp_idx: 0,
            arp_cur: None,
            arp_left: 0,
            arp_gate_left: 0,
        }
    }

    fn arp_on(&self) -> bool {
        self.v[P_ARP] > 0.5
    }

    /// samples in one arpeggiator step
    fn arp_step_len(&self) -> u64 {
        let k = (self.v[P_ARP_RATE] * 5.0).round().clamp(0.0, 5.0) as usize;
        ((self.sr as f64 * 60.0 / self.bpm.clamp(20.0, 400.0) * ARP_BEATS[k]) as u64).max(32)
    }

    /// The notes the arpeggiator cycles through: the held keys, repeated up the octaves, ordered by the mode.
    fn arp_sequence(&self) -> Vec<(u8, f32, f32)> {
        let octs = 1 + (self.v[P_ARP_OCT] * 2.0).round() as u8;
        let mut base = self.arp_keys.clone();
        let mode = (self.v[P_ARP_MODE] * 4.0).round() as u8;
        if mode != 4 {
            base.sort_by_key(|k| k.0);
        }
        let mut seq = vec![];
        for o in 0..octs {
            for k in &base {
                if k.0 as u32 + 12 * o as u32 <= 127 {
                    seq.push((k.0 + 12 * o, k.1, k.2));
                }
            }
        }
        if mode == 1 {
            seq.reverse();
        }
        seq
    }

    /// Start the next step: let the previous note go, sound the next one of the sequence.
    fn arp_step(&mut self) {
        let step = self.arp_step_len();
        self.arp_left = step;
        if let Some(k) = self.arp_cur.take() {
            self.poly_off(k);
        }
        let seq = self.arp_sequence();
        if seq.is_empty() {
            self.arp_gate_left = 0;
            self.arp_idx = 0;
            return;
        }
        let n = seq.len();
        let mode = (self.v[P_ARP_MODE] * 4.0).round() as u8;
        let at = match mode {
            2 if n > 1 => {
                let period = 2 * (n - 1);
                let i = self.arp_idx % period;
                if i < n {
                    i
                } else {
                    period - i
                }
            }
            3 => 0,
            _ => self.arp_idx % n,
        };
        // the random mode draws from the whole sequence
        let at = if mode == 3 { ((self.rng.next().abs() * n as f32) as usize).min(n - 1) } else { at };
        self.arp_idx = self.arp_idx.wrapping_add(1);
        let (key, vel, pan) = seq[at];
        let gate = (self.v[P_ARP_GATE].clamp(0.05, 1.0) * step as f32) as u64;
        self.arp_gate_left = gate.max(16);
        // a note that came from the piano roll carries its syllables: they are fitted to the step; live notes (no length) hold
        let len = if self.arp_lyric.is_empty() { 0.0 } else { gate as f32 };
        let lyric = self.arp_lyric.clone();
        self.arp_cur = Some(key);
        self.poly_on(key, vel, pan, &lyric, len);
    }

    fn render_voices(&mut self, out: &mut [f32]) {
        self.m_render(out);
        for c in self.pool.iter_mut() {
            c.m_render(out);
        }
    }

    /// how many notes can sound at once
    fn n_voices(&self) -> usize {
        1 + (self.v[P_VOICES] * 7.0).round() as usize
    }

    /// Bring the pool in line with the `voices` knob: build voices, share the parameters, silence the ones no longer used.
    fn sync_pool(&mut self) {
        let n = self.n_voices();
        while self.pool.len() + 1 < n {
            self.pool.push(Voice::new(self.sr));
        }
        let total = self.pool.len() + 1;
        self.slot_key.resize(total, None);
        self.slot_age.resize(total, 0);
        self.claimed.resize(total, false);
        let trim = if n > 1 { (1.3 / (n as f32).sqrt()).min(1.0) } else { 1.0 };
        self.trim = trim;
        let v = self.v;
        for (k, c) in self.pool.iter_mut().enumerate() {
            c.v = v;
            c.trim = trim;
            if k + 1 >= n && self.slot_key[k + 1].take().is_some() {
                c.m_all_off();
            }
        }
    }

    fn voice_at(&mut self, s: usize) -> &mut Voice {
        if s == 0 {
            self
        } else {
            &mut self.pool[s - 1]
        }
    }

    fn default_vowel(&self) -> [f32; 3] {
        let k = ((self.v[P_VOWEL] * 4.0).round() as usize).min(4);
        match k {
            0 => [730.0, 1090.0, 2440.0],
            1 => [390.0, 2090.0, 2800.0],
            2 => [270.0, 2290.0, 3010.0],
            3 => [400.0, 800.0, 2400.0],
            _ => [300.0, 870.0, 2240.0],
        }
    }

    /// Replace the score with the syllables of a note.
    fn plan(&mut self, lyric: &str, len_samples: f32) {
        let tied = lyric.starts_with('~');
        let body = lyric.trim_start_matches('~');
        let strength = 0.6 + self.v[P_CONS] * 0.9;
        let live = len_samples <= 1.0;
        let total = if live { (self.sr * 600.0) as u32 } else { len_samples as u32 };
        self.live_coda.clear();
        self.sustaining = false;
        let syls: Vec<&str> = body.split('-').filter(|s| !s.is_empty()).collect();
        let mut score: Vec<Seg> = vec![];
        if syls.is_empty() {
            // sustain the current vowel (tied, no text) or sing the default vowel (no lyric at all)
            let f = if tied { self.f } else { self.default_vowel() };
            score.push(vowel_seg(f, total, 0.0));
        } else if live {
            // live play: sing the first syllable, hold its vowel, put the coda on note-off
            let s = split_syllable(syls[0]);
            let mut segs = syllable_segs(&s, (self.sr * 0.5) as u32, self.sr, strength, tied);
            // everything up to and including the vowel now, the rest at note-off
            let vi = s.onset.iter().filter(|p| !tied || matches!(p.kind, Kind::Nasal | Kind::Liquid | Kind::Glide)).map(|p| consonant_segs(p, [600.0, 1300.0, 2500.0], self.sr, strength, false).len()).sum::<usize>();
            let nv = s.nucleus.len().max(1);
            let keep = (vi + nv).min(segs.len());
            self.live_coda = segs.split_off(keep);
            if let Some(last) = segs.last_mut() {
                last.dur = total;
            }
            score = segs;
        } else {
            let n = syls.len() as u32;
            let each = (total / n).max(1);
            for (k, s) in syls.iter().enumerate() {
                let syl = split_syllable(s);
                // the very first syllable of a tied note keeps the voice running; later ones articulate normally
                score.extend(syllable_segs(&syl, each, self.sr, strength, tied && k == 0));
            }
        }
        if score.is_empty() {
            score.push(vowel_seg(self.default_vowel(), total, 0.0));
        }
        self.score = score;
        self.seg_i = 0;
        self.seg_left = self.score[0].dur;
        self.apply_seg();
    }

    fn apply_seg(&mut self) {
        if let Some(s) = self.score.get(self.seg_i) {
            self.burst = s.burst;
            if s.burst.is_some() {
                self.burst_env = 1.0;
                self.burst_t = 0.0;
            }
        }
    }
}

fn defaults() -> [f32; NAMES.len()] {
    // vowel a, mid gender, bright, no breath, no whisper, vibrato 5.5 Hz, depth, delay, attack, release, glide, cons, gain, pan,
    // one voice, no choir, open quotient 0.62, no fry, normal drift, no scoop, no ring, no tremolo, no nasality,
    // arpeggiator off: up, 1/16, gate 0.6, one octave
    // in tune, full expression
    [0.0, 0.5, 0.55, 0.12, 0.0, 0.5, 0.3, 0.4, 0.15, 0.25, 0.15, 0.5, 0.7, 0.5, 0.0, 0.0, 0.5, 0.0, 0.3, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.6, 0.6, 0.0, 0.5, 1.0]
}

impl Voice {
    /// one voice starts a note
    fn m_on(&mut self, key: u8, vel: f32, pan: f32, lyric: &str, len_samples: f32) {
        self.held.retain(|k| *k != key);
        self.held.push(key);
        self.target = key as f32;
        self.vel = vel.clamp(0.0, 1.0);
        self.pan = pan.clamp(-1.0, 1.0);
        // a tied note carries on from a voice that is sounding or still dying away (its release has not finished): no jump, the pitch slurs
        let tied = lyric.starts_with('~') && (self.gate || self.env > 0.02);
        if !self.gate && !tied {
            // fresh start: the pitch jumps
            self.note = self.target;
            self.vib_t = 0.0;
            self.scoop_c = -self.v[P_SCOOP] * 300.0;
        }
        self.plan(lyric, len_samples);
        self.gate = true;
    }
    fn m_off(&mut self, key: u8) {
        self.held.retain(|k| *k != key);
        if let Some(&k) = self.held.last() {
            // another note is still down (legato overlap): carry on with it
            self.target = k as f32;
            return;
        }
        // live play: sing the coda while the voice releases
        if !self.live_coda.is_empty() {
            let coda = std::mem::take(&mut self.live_coda);
            self.score = coda;
            self.seg_i = 0;
            self.seg_left = self.score[0].dur;
            self.apply_seg();
        }
        self.gate = false;
    }
    fn m_all_off(&mut self) {
        self.held.clear();
        self.gate = false;
        self.live_coda.clear();
        self.score.clear();
    }

    fn m_render(&mut self, out: &mut [f32]) {
        let frames = out.len() / 2;
        if frames == 0 {
            return;
        }
        let sr = self.sr;
        let dt = 1.0 / sr;
        let gender = self.v[P_GENDER];
        let fscale = 0.82 + gender * 0.5;
        let bright = self.v[P_BRIGHT];
        let tilt_k = 1.0 - (-TAU * (1200.0 + bright * 7000.0) / sr).exp();
        let whisp = self.v[P_WHISP];
        let breath = self.v[P_BREATH] * 0.35;
        let att = 0.004 + self.v[P_ATT].powi(2) * 0.4;
        let rel = 0.02 + self.v[P_REL].powi(2) * 0.8;
        let glide = 1.0 - (-dt / (0.002 + self.v[P_GLIDE].powi(2) * 0.3)).exp();
        let vib_rate = 3.5 + self.v[P_VRATE] * 4.0;
        let vib_depth = self.v[P_VDEPTH] * 70.0;
        let vib_delay = self.v[P_VDELAY] * 1.2;
        let gain = self.v[P_GAIN] * 2.0 * self.trim;
        let oq = 0.35 + self.v[P_OPEN] * 0.54;
        let choir = self.v[P_CHOIR];
        let spread = choir * 28.0;
        let fry = self.v[P_FRY];
        let drift = self.v[P_DRIFT] / 0.3;
        let ring = self.v[P_RING] * 1.2;
        let trem = self.v[P_TREM] * 0.45;
        let nasal_base = self.v[P_NASAL] * 0.6;
        let scoop_k = (-dt / 0.09).exp();
        let tune_c = (self.v[P_TUNE] - 0.5) * 2.0 * TUNE_RANGE;
        let expr = self.v[P_EXPR];
        let smooth_k = 1.0 - (-dt / 0.012).exp();
        let pan = (self.pan + 1.0) * std::f32::consts::FRAC_PI_4;
        let (pl, pr) = (pan.cos() * std::f32::consts::SQRT_2, pan.sin() * std::f32::consts::SQRT_2);
        let bw = [70.0f32, 90.0, 140.0, 220.0, 300.0];
        for i in 0..frames {
            // ── score ──
            if self.seg_i < self.score.len() {
                if self.seg_left == 0 {
                    if self.seg_i + 1 < self.score.len() {
                        self.seg_i += 1;
                        self.seg_left = self.score[self.seg_i].dur;
                        self.apply_seg();
                    }
                } else {
                    self.seg_left -= 1;
                }
            }
            if let Some(s) = self.score.get(self.seg_i) {
                let k = 1.0 - (-dt / s.tau.max(0.001)).exp();
                for j in 0..3 {
                    self.f[j] += (s.f[j] - self.f[j]) * k;
                }
                self.voice_amp += (s.voice - self.voice_amp) * k;
                self.asp_amp += (s.asp - self.asp_amp) * k;
                // the noise filter's centre must not sweep up from 0 Hz when the noise starts: place it while it is silent
                if self.fric.2 < 0.01 && s.fric.2 > 0.0 {
                    self.fric.0 = s.fric.0;
                    self.fric.1 = s.fric.1;
                } else {
                    self.fric.0 += (s.fric.0 - self.fric.0) * k;
                    self.fric.1 += (s.fric.1 - self.fric.1) * k;
                }
                self.fric.2 += (s.fric.2 - self.fric.2) * k;
                self.nasal += (s.nasal - self.nasal) * k;
            }
            let k2 = 1.0 - (-dt / 0.003).exp();
            self.voice_s += (self.voice_amp - self.voice_s) * k2;
            self.asp_s += (self.asp_amp - self.asp_s) * k2;
            self.fric_s += (self.fric.2 - self.fric_s) * k2;
            // ── pitch ──
            self.note += (self.target - self.note) * glide;
            self.vib_t += dt;
            let vib = if self.vib_t > vib_delay { (TAU * vib_rate * (self.vib_t - vib_delay)).sin() * vib_depth * ((self.vib_t - vib_delay) * 2.0).min(1.0) } else { 0.0 };
            self.jitter += (self.rng.next() * 6.0 * drift - self.jitter) * 0.002;
            self.scoop_c *= scoop_k;
            self.tune_s += (tune_c - self.tune_s) * smooth_k;
            self.expr_s += (expr - self.expr_s) * smooth_k;
            let f0 = midi_hz(self.note + (vib + self.jitter + self.scoop_c + self.tune_s) / 100.0);
            // ── amplitude envelope ──
            let goal = if self.gate { 1.0 } else { 0.0 };
            let rate = if goal > self.env { dt / att } else { dt / rel };
            self.env += (goal - self.env).clamp(-rate, rate);
            if self.env < 0.0005 && !self.gate {
                // idle: nothing to compute (keeps state silent)
                out[i * 2] += 0.0;
                continue;
            }
            // ── glottal source: derivative of an asymmetric flow pulse ──
            self.phase += f0 * dt;
            if self.phase >= 1.0 {
                self.phase -= self.phase.floor();
                // vocal fry: every other pulse is weaker, and all of them wobble
                self.cyc = !self.cyc;
                self.cyc_gain = if fry > 0.0 { (if self.cyc { 1.0 - fry * 0.6 } else { 1.0 }) * (1.0 - fry * 0.25 * self.rng.next().abs()) } else { 1.0 };
            }
            let pulse = |ph: f32| -> f32 {
                if ph < oq {
                    let x = ph / oq;
                    if x < 0.7 {
                        0.5 * (1.0 - (std::f32::consts::PI * x / 0.7).cos())
                    } else {
                        ((std::f32::consts::FRAC_PI_2) * (x - 0.7) / 0.3).cos()
                    }
                } else {
                    0.0
                }
            };
            let flow = pulse(self.phase);
            let pitch_k = (f0 / 110.0).sqrt() * 9.0;
            let mut src_raw = (flow - self.prev_flow) * pitch_k * self.cyc_gain;
            self.prev_flow = flow;
            if choir > 0.001 {
                // two more glottal sources, detuned either way, each drifting a little on its own
                self.jit2 += (self.rng.next() * 8.0 - self.jit2) * 0.001;
                self.jit3 += (self.rng.next() * 8.0 - self.jit3) * 0.001;
                self.phase2 = (self.phase2 + f0 * dt * (1.0 + (spread + self.jit2 * choir) * 0.000578)).fract();
                self.phase3 = (self.phase3 + f0 * dt * (1.0 - (spread * 0.8 + self.jit3 * choir) * 0.000578)).fract();
                let (fl2, fl3) = (pulse(self.phase2), pulse(self.phase3));
                let extra = ((fl2 - self.prev2) + (fl3 - self.prev3)) * pitch_k;
                self.prev2 = fl2;
                self.prev3 = fl3;
                src_raw = (src_raw + extra * choir * 0.7) / (1.0 + 0.9 * choir);
            }
            self.tilt += (src_raw - self.tilt) * tilt_k;
            let voiced = self.tilt * self.voice_s * (1.0 - whisp);
            let noise = self.rng.next();
            let asp = noise * (self.asp_s * 0.5 + breath * 0.4 + whisp * 0.12 * (0.3 + self.voice_s));
            // ── vocal tract: five cascaded formants ──
            let mut x = voiced + asp;
            let fr = [self.f[0] * fscale, self.f[1] * fscale, self.f[2] * fscale, 3300.0 * fscale.sqrt(), 3750.0 * fscale.sqrt()];
            for k in 0..5 {
                x = self.res[k].run(x, fr[k], bw[k], sr);
            }
            // level every vowel to the same loudness (the cascade's gain differs a lot between /i/ and /o/)
            if self.voice_s > 0.3 {
                self.agc_pow += (x * x - self.agc_pow) * (1.0 / (0.01 * sr));
                let want = (0.09 / (self.agc_pow.sqrt() + 1e-4)).clamp(0.05, 8.0);
                // too loud: come down at once; too quiet: come up slowly, so a vowel change never overshoots
                let rate = if want < self.agc_gain { 0.03 } else { 0.002 };
                self.agc_gain += (want - self.agc_gain) * rate;
            }
            // the singers formant: a ring around 3 kHz that carries over a choir or a band
            if ring > 0.001 {
                x += self.ring_res.run(x, 2850.0 * fscale.sqrt(), 380.0, sr) * ring;
            }
            let x = x * self.agc_gain;
            // nasal murmur: a low resonance in parallel, upper formants pulled down
            let nz = (self.nasal + nasal_base * (1.0 - self.nasal)).min(1.0);
            let mut y = x * (1.0 - 0.55 * nz);
            if nz > 0.01 {
                y += self.nasal_res.run(voiced * 3.0, 260.0, 90.0, sr) * 0.3 * nz;
            }
            // frication and bursts bypass the tract
            if self.fric_s > 0.001 {
                y += self.fric_res.run(noise, self.fric.0 * (0.9 + 0.1 * fscale), self.fric.1, sr) * self.fric_s * 0.45;
            }
            if let Some((bf, bbw, ba)) = self.burst {
                self.burst_t += dt;
                // 0.5 ms attack, then a 4 ms decay: the burst starts and ends smoothly
                self.burst_env = (self.burst_t / 0.0005).min(1.0) * (-self.burst_t / 0.004).exp();
                if self.burst_t > 0.03 {
                    self.burst = None;
                } else {
                    y += self.burst_res.run(self.rng.next(), bf, bbw, sr) * ba * self.burst_env * 0.5;
                }
            }
            // keep the output free of DC and never hard-clip: a gentle saturation above ~0.5
            self.dc += (y - self.dc) * (TAU * 25.0 / sr);
            let y = y - self.dc;
            let y = (y * 1.5).tanh() / 1.5;
            let a = self.env * gain * self.expr_s * (0.35 + 0.65 * self.vel) * (1.0 - trem * (0.5 - 0.5 * (TAU * vib_rate * self.vib_t).sin()));
            let s = y * a;
            out[i * 2] += s * pl;
            out[i * 2 + 1] += s * pr;
        }
    }

}

impl Voice {
    /// a note starts: on the one voice, or on a free one of the pool
    fn poly_on(&mut self, key: u8, vel: f32, pan: f32, lyric: &str, len_samples: f32) {
        self.sync_pool();
        let n = self.n_voices();
        if n == 1 {
            self.m_on(key, vel, pan, lyric, len_samples);
            return;
        }
        let tied = lyric.starts_with('~');
        self.clock += 1;
        // the slot: the one already singing this key, else (a tied note) the newest sounding one so the voice slurs, else a free one, else the oldest
        let same = (0..n).find(|s| self.slot_key[*s] == Some(key));
        // a tied note slurs from the sounding (or just released) voice whose pitch is nearest and that no other note of this chord has taken yet
        let slur = if same.is_none() && tied {
            let state = |me: &Voice, s: usize| if s == 0 { (me.env, me.note) } else { (me.pool[s - 1].env, me.pool[s - 1].note) };
            (0..n)
                .filter(|s| !self.claimed[*s] && (self.slot_key[*s].is_some() || state(self, *s).0 > 0.02))
                .min_by(|a, b| (state(self, *a).1 - key as f32).abs().total_cmp(&(state(self, *b).1 - key as f32).abs()))
        } else {
            None
        };
        let free = (0..n).filter(|s| self.slot_key[*s].is_none()).min_by(|a, b| {
            let (ea, eb) = (if *a == 0 { self.env } else { self.pool[*a - 1].env }, if *b == 0 { self.env } else { self.pool[*b - 1].env });
            ea.total_cmp(&eb)
        });
        let slot = same.or(slur).or(free).unwrap_or_else(|| (0..n).min_by_key(|s| self.slot_age[*s]).unwrap_or(0));
        let stolen = same.is_none() && slur.is_none() && free.is_none();
        let old = self.slot_key[slot];
        self.slot_key[slot] = Some(key);
        self.slot_age[slot] = self.clock;
        self.claimed[slot] = true;
        let v = self.voice_at(slot);
        if let Some(o) = old {
            if o != key {
                v.held.retain(|k| *k != o);
            }
        }
        v.m_on(key, vel, pan, lyric, len_samples);
        if stolen && !tied {
            v.note = key as f32;
            v.vib_t = 0.0;
        }
    }
    fn poly_off(&mut self, key: u8) {
        let n = self.n_voices();
        if n == 1 {
            self.m_off(key);
            return;
        }
        match (0..n).find(|s| self.slot_key[*s] == Some(key)) {
            Some(s) => {
                self.slot_key[s] = None;
                self.voice_at(s).m_off(key);
            }
            None => {
                for s in 0..n {
                    self.voice_at(s).m_off(key);
                }
            }
        }
    }
    fn poly_all_off(&mut self) {
        self.m_all_off();
        for c in self.pool.iter_mut() {
            c.m_all_off();
        }
        for k in self.slot_key.iter_mut() {
            *k = None;
        }
    }
}

impl Instrument for Voice {
    // vibrato and breath run free between phrases: always rendered (see Instrument::can_sleep)
    fn can_sleep(&self) -> bool {
        false
    }
    fn note_on(&mut self, key: u8, vel: f32) {
        self.note_on_lyric(key, vel, 0.0, 0.5, 0.5, "", 0.0);
    }
    fn note_on_ex(&mut self, key: u8, vel: f32, pan: f32, cutoff: f32, reso: f32) {
        self.note_on_lyric(key, vel, pan, cutoff, reso, "", 0.0);
    }
    fn note_on_lyric(&mut self, key: u8, vel: f32, pan: f32, _cutoff: f32, _reso: f32, lyric: &str, len_samples: f32) {
        if !self.arp_on() {
            self.poly_on(key, vel, pan, lyric, len_samples);
            return;
        }
        // arpeggiator: the key joins the held set; the first one of a chord starts the clock at once
        self.arp_keys.retain(|k| k.0 != key);
        if self.arp_keys.is_empty() {
            self.arp_left = 0;
            self.arp_idx = 0;
        }
        self.arp_keys.push((key, vel.clamp(0.0, 1.0), pan.clamp(-1.0, 1.0)));
        self.arp_lyric = lyric.trim_start_matches('~').to_string();
    }
    fn note_off(&mut self, key: u8) {
        if self.arp_on() || !self.arp_keys.is_empty() {
            self.arp_keys.retain(|k| k.0 != key);
            if self.arp_keys.is_empty() {
                if let Some(k) = self.arp_cur.take() {
                    self.poly_off(k);
                }
                self.arp_gate_left = 0;
            }
            return;
        }
        self.poly_off(key);
    }
    fn all_off(&mut self) {
        self.arp_keys.clear();
        self.arp_cur = None;
        self.arp_gate_left = 0;
        self.poly_all_off();
    }
    fn set_tempo(&mut self, bpm: f64) {
        if bpm.is_finite() && bpm > 1.0 {
            self.bpm = bpm;
        }
    }
    fn render(&mut self, out: &mut [f32]) {
        self.sync_pool();
        self.claimed.iter_mut().for_each(|c| *c = false);
        if !self.arp_on() {
            // switched off while a note was held: let the arpeggiated note go
            if let Some(k) = self.arp_cur.take() {
                self.poly_off(k);
            }
            self.arp_keys.clear();
            self.render_voices(out);
            return;
        }
        let frames = out.len() / 2;
        let mut at = 0usize;
        while at < frames {
            if self.arp_left == 0 {
                if self.arp_keys.is_empty() {
                    // nothing held: idle (the voices finish their release) until a key goes down
                    self.arp_left = u64::MAX;
                } else {
                    self.arp_step();
                }
            }
            let mut n = (frames - at) as u64;
            n = n.min(self.arp_left);
            if self.arp_gate_left > 0 {
                n = n.min(self.arp_gate_left);
            }
            let n = n.max(1) as usize;
            self.render_voices(&mut out[at * 2..(at + n) * 2]);
            if self.arp_left != u64::MAX {
                self.arp_left = self.arp_left.saturating_sub(n as u64);
            }
            if self.arp_gate_left > 0 {
                self.arp_gate_left = self.arp_gate_left.saturating_sub(n as u64);
                if self.arp_gate_left == 0 {
                    if let Some(k) = self.arp_cur.take() {
                        self.poly_off(k);
                    }
                }
            }
            at += n;
        }
    }

    fn params(&self) -> Vec<(&'static str, f32)> {
        NAMES.iter().zip(self.v.iter()).map(|(n, v)| (*n, *v)).collect()
    }
    fn set_param(&mut self, name: &str, v: f32) {
        if !v.is_finite() {
            return;
        }
        if let Some(i) = NAMES.iter().position(|n| *n == name) {
            self.v[i] = v.clamp(0.0, 1.0);
        }
    }
    fn name(&self) -> &'static str {
        "Ling Voice"
    }
    fn presets(&self) -> &'static [&'static str] {
        &Self::PRESETS
    }
    fn load_preset(&mut self, name: &str) {
        if let Some(st) = preset_state(name) {
            self.load_state(&st);
        }
    }
    fn load_state(&mut self, st: &[(String, f32)]) {
        self.v = defaults();
        for (n, v) in st {
            self.set_param(n, *v);
        }
    }
    fn plot(&self, what: &str, n: usize) -> Vec<f32> {
        if what != "filter" {
            return vec![];
        }
        // magnitude response of the vocal tract for the default vowel
        let n = n.clamp(2, 512);
        let fscale = 0.82 + self.v[P_GENDER] * 0.5;
        let vf = self.default_vowel();
        let fr = [vf[0] * fscale, vf[1] * fscale, vf[2] * fscale, 3300.0 * fscale.sqrt(), 3750.0 * fscale.sqrt()];
        let bw = [70.0f32, 90.0, 140.0, 220.0, 300.0];
        (0..n)
            .map(|i| {
                let f = 80.0 * 100f32.powf(i as f32 / (n - 1) as f32);
                let mut db = 0.0f32;
                for k in 0..5 {
                    let r = (f / fr[k]).max(1e-3);
                    let q = fr[k] / bw[k];
                    let den = ((1.0 - r * r).powi(2) + (r / q).powi(2)).sqrt();
                    db += -20.0 * den.log10();
                }
                ((db + 24.0) / 60.0).clamp(0.0, 1.0)
            })
            .collect()
    }
}

fn preset_state(name: &str) -> Option<Vec<(String, f32)>> {
    static P: OnceLock<Vec<(String, Vec<(String, f32)>)>> = OnceLock::new();
    let all = P.get_or_init(|| {
        let mut out = vec![];
        let mut cur_name = String::new();
        let mut cur = String::new();
        let flush = |out: &mut Vec<(String, Vec<(String, f32)>)>, n: &str, t: &str| {
            if !n.is_empty() {
                out.push((n.to_string(), crate::patch::parse(t).params));
            }
        };
        for line in include_str!("../voice_presets.txt").lines() {
            if line.starts_with('[') && line.ends_with(']') {
                flush(&mut out, &cur_name, &cur);
                cur_name = line[1..line.len() - 1].to_string();
                cur.clear();
            } else {
                cur.push_str(line);
                cur.push('\n');
            }
        }
        flush(&mut out, &cur_name, &cur);
        out
    });
    all.iter().find(|(n, _)| n == name).map(|(_, p)| p.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sing(v: &mut Voice, key: u8, lyric: &str, secs: f32) -> Vec<f32> {
        let n = (v.sr * secs) as usize;
        v.note_on_lyric(key, 0.9, 0.0, 0.5, 0.5, lyric, n as f32);
        let mut out = vec![0.0f32; n * 2];
        for c in out.chunks_mut(128) {
            v.render(c);
        }
        v.note_off(key);
        out
    }

    fn mono(v: &[f32]) -> Vec<f32> {
        v.chunks_exact(2).map(|f| f[0]).collect()
    }

    /// energy in [lo, hi) Hz via a plain DFT on a windowed slice
    fn band(x: &[f32], sr: f32, lo: f32, hi: f32) -> f32 {
        let n = 4096.min(x.len());
        let mut e = 0.0;
        let mut f = lo;
        while f < hi {
            let (mut re, mut im) = (0.0, 0.0);
            for (i, s) in x.iter().take(n).enumerate() {
                let w = 0.5 - 0.5 * (TAU * i as f32 / n as f32).cos();
                let a = TAU * f * i as f32 / sr;
                re += s * w * a.cos();
                im += s * w * a.sin();
            }
            e += re * re + im * im;
            f += 40.0;
        }
        e
    }

    #[test]
    fn tune_bends_the_pitch_and_expr_the_level() {
        let sr = 24000.0;
        let median_f0 = |tune: f32| -> f32 {
            let mut v = Voice::new(sr);
            v.set_param("vib depth", 0.0);
            v.set_param("drift", 0.0);
            v.set_param("tune", tune);
            let m = mono(&sing(&mut v, 57, "a", 0.8));
            let p = crate::vocal::track_pitch(&m, sr);
            let mut f: Vec<f32> = p.f0.iter().copied().filter(|x| *x > 0.0).collect();
            f.sort_by(|a, b| a.total_cmp(b));
            f[f.len() / 2]
        };
        let in_tune = median_f0(0.5);
        let flat = median_f0(0.0);
        // a whole tone flat at the bottom of the range
        let cents = 1200.0 * (flat / in_tune).log2();
        assert!((cents + 200.0).abs() < 25.0, "{in_tune} Hz vs {flat} Hz: {cents} cents");
        assert!((in_tune - 220.0).abs() < 8.0, "{in_tune}");
        let energy = |expr: f32| {
            let mut v = Voice::new(sr);
            v.set_param("expr", expr);
            mono(&sing(&mut v, 57, "a", 0.5)).iter().map(|x| x * x).sum::<f32>()
        };
        let (loud, soft) = (energy(1.0), energy(0.25));
        assert!(soft < loud * 0.2 && soft > 0.0, "{soft} vs {loud}");
    }

    #[test]
    fn parses_syllables_into_onset_nucleus_coda() {
        let s = split_syllable("tʃiːn");
        assert_eq!(s.onset.len(), 1);
        assert_eq!(s.nucleus.len(), 1);
        assert_eq!(s.coda.len(), 1);
        let d = split_syllable("ɾei");
        assert_eq!(d.nucleus.len(), 2, "ei is a diphthong");
        let a = split_syllable("kja");
        assert_eq!(a.onset.len(), 2);
        let m = split_syllable("m");
        assert_eq!(m.nucleus.len(), 1, "a syllabic nasal carries the note");
    }

    #[test]
    fn every_preset_sings_finite_audible_audio() {
        for name in Voice::PRESETS {
            let mut v = Voice::new(48000.0);
            v.load_preset(name);
            let o = sing(&mut v, 60, "ɾa", 0.8);
            assert!(o.iter().all(|x| x.is_finite()), "{name}");
            let pk = o.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            eprintln!("{name}: peak {pk:.3}");
            assert!(pk > 0.03 && pk < 1.0, "{name}: peak {pk}");
        }
    }

    #[test]
    fn vowels_have_different_spectra() {
        let sr = 48000.0;
        let mut v = Voice::new(sr);
        let a = mono(&sing(&mut v, 57, "a", 0.8));
        let mut v2 = Voice::new(sr);
        let i = mono(&sing(&mut v2, 57, "i", 0.8));
        let mid = |x: &[f32]| x[(sr * 0.3) as usize..].to_vec();
        let (a, i) = (mid(&a), mid(&i));
        let (a, i) = (&a[..], &i[..]);
        // /i/ has far more energy near F2 (~2.3 kHz) than /a/, and /a/ more near F1 (~730 Hz)
        let (a_hi, i_hi) = (band(a, sr, 2000.0, 2600.0), band(i, sr, 2000.0, 2600.0));
        let (a_lo, i_lo) = (band(a, sr, 600.0, 900.0), band(i, sr, 600.0, 900.0));
        assert!(i_hi / i_lo.max(1e-9) > 3.0 * a_hi / a_lo.max(1e-9), "i/a hi-lo ratio: {} vs {}", i_hi / i_lo, a_hi / a_lo);
    }

    #[test]
    fn a_fricative_onset_is_noisy_in_the_highs() {
        let sr = 48000.0;
        let mut v = Voice::new(sr);
        let o = mono(&sing(&mut v, 57, "sa", 0.8));
        let early = &o[..(sr * 0.09) as usize];
        let late = &o[(sr * 0.4) as usize..];
        let hi = |x: &[f32]| band(x, sr, 5500.0, 9000.0) / x.len().min(4096) as f32;
        assert!(hi(early) > 4.0 * hi(late), "the s must be much brighter than the vowel");
    }

    #[test]
    fn pitch_follows_the_note() {
        let sr = 48000.0;
        let mut lo = Voice::new(sr);
        let mut hi = Voice::new(sr);
        lo.set_param("vib depth", 0.0);
        hi.set_param("vib depth", 0.0);
        // fundamental by autocorrelation
        let f0 = |x: &[f32]| -> f32 {
            let x = &x[(sr * 0.3) as usize..(sr * 0.3) as usize + 4096];
            let (lo, hi) = ((sr / 700.0) as usize, (sr / 70.0) as usize);
            let mut best = (lo, f32::MIN);
            for lag in lo..hi {
                let c: f32 = (0..x.len() - lag).map(|i| x[i] * x[i + lag]).sum();
                if c > best.1 {
                    best = (lag, c);
                }
            }
            sr / best.0 as f32
        };
        let a = mono(&sing(&mut lo, 48, "a", 1.0));
        let b = mono(&sing(&mut hi, 60, "a", 1.0));
        let (fa, fb) = (f0(&a), f0(&b));
        assert!((fa - 130.8).abs() < 6.0, "C3 should be ~131 Hz, got {fa}");
        assert!((fb - 261.6).abs() < 12.0, "C4 should be ~262 Hz, got {fb}");
    }

    #[test]
    fn a_tied_note_keeps_the_voice_running() {
        let sr = 48000.0;
        let mut v = Voice::new(sr);
        let n = (sr * 0.4) as usize;
        let mut out = vec![0.0f32; n * 4];
        v.note_on_lyric(60, 0.9, 0.0, 0.5, 0.5, "ka", n as f32);
        for c in out[..n * 2].chunks_mut(128) {
            v.render(c);
        }
        // the next note begins as the first ends, tied: no note-off gap in the sound
        v.note_on_lyric(64, 0.9, 0.0, 0.5, 0.5, "~ɾei", n as f32);
        v.note_off(60);
        for c in out[n * 2..].chunks_mut(128) {
            v.render(c);
        }
        let m = mono(&out);
        let around = &m[n - 600..n + 600];
        let rms = (around.iter().map(|x| x * x).sum::<f32>() / around.len() as f32).sqrt();
        let body = &m[n / 2..n / 2 + 1200];
        let rms_body = (body.iter().map(|x| x * x).sum::<f32>() / body.len() as f32).sqrt();
        assert!(rms > 0.4 * rms_body, "the join must not drop out: {rms} vs {rms_body}");
    }

    #[test]
    fn multiple_syllables_share_one_note() {
        let sr = 48000.0;
        let mut v = Voice::new(sr);
        let o = mono(&sing(&mut v, 57, "so-fa", 1.0));
        // two syllables = two onsets: a brighter burst of s near the start and an f/a articulation around the middle
        let first = &o[..(sr * 0.1) as usize];
        let second = &o[(sr * 0.5) as usize..(sr * 0.6) as usize];
        let e = |x: &[f32]| x.iter().map(|s| s * s).sum::<f32>();
        assert!(e(first) > 0.0 && e(second) > 0.0);
    }

    /// How much louder the loudest 1 ms of the consonant is than the 3 ms on either side of it: a click is a spike
    /// its neighbours do not share (a smooth onset or a decaying burst scores around 1-3). Also returns the peak.
    fn click_ratio(syl: &str) -> (f32, f32) {
        let sr = 48000.0;
        let mut v = Voice::new(sr);
        v.set_param("vib depth", 0.0);
        let o = mono(&sing(&mut v, 57, syl, 0.8));
        let vowel = &o[(sr * 0.45) as usize..(sr * 0.7) as usize];
        let vrms = (vowel.iter().map(|x| x * x).sum::<f32>() / vowel.len() as f32).sqrt().max(1e-6);
        let win = (sr / 1000.0) as usize;
        let env: Vec<f32> = o[..(sr * 0.3) as usize].chunks(win).map(|c| (c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).sqrt()).collect();
        let mut worst = 0.0f32;
        // skip the first 10 ms: that is the note's own start, not the consonant
        for k in 10..env.len().saturating_sub(3) {
            let left = env[k - 3..k].iter().sum::<f32>() / 3.0;
            let right = env[k + 1..k + 4].iter().sum::<f32>() / 3.0;
            worst = worst.max(env[k] / (left.max(right) + 0.05 * vrms));
        }
        let peak = o.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        let vpeak = vowel.iter().fold(0.0f32, |m, x| m.max(x.abs())).max(1e-6);
        eprintln!("{syl:>5}: spike {worst:5.2}  peak {peak:.2} = {:.1}x the vowel", peak / vpeak);
        (worst, peak / vpeak)
    }

    #[test]
    fn consonants_do_not_click() {
        let (mut worst, mut loudest) = (0.0f32, 0.0f32);
        for syl in ["ta", "ka", "pa", "ba", "da", "ga", "tʃa", "dʒa", "sa", "ʃa", "fa", "za", "ma", "na", "ɾa", "la", "ha", "tsa"] {
            let (r, p) = click_ratio(syl);
            worst = worst.max(r);
            loudest = loudest.max(p);
        }
        assert!(worst < 4.0, "a consonant spikes {worst}x above its neighbours");
        assert!(loudest < 1.8, "a consonant is {loudest}x louder than the vowel's peak");
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn polyphony_sings_a_chord_and_mono_does_not() {
        let sr = 48000.0;
        let run = |voices: f32| -> Vec<f32> {
            let mut v = Voice::new(sr);
            v.set_param("voices", voices);
            v.set_param("vib depth", 0.0);
            for k in [60u8, 64, 67] {
                v.note_on_lyric(k, 0.9, 0.0, 0.5, 0.5, "a", 0.0);
            }
            let mut out = vec![0.0f32; (sr * 0.8) as usize * 2];
            for c in out.chunks_mut(128) {
                v.render(c);
            }
            assert!(out.iter().all(|x| x.is_finite()));
            mono(&out)[(sr * 0.4) as usize..].to_vec()
        };
        let one = run(0.0);
        let poly = run(0.5);
        // three pitches at once have energy at all three fundamentals; one voice only at the last note
        let at = |x: &[f32], hz: f32| band(x, sr, hz - 12.0, hz + 12.0);
        let (c4, e4, g4) = (261.6, 329.6, 392.0);
        assert!(at(&poly, c4) > 5.0 * at(&one, c4), "the C is missing in the chord");
        assert!(at(&poly, e4) > 5.0 * at(&one, e4), "the E is missing in the chord");
        assert!(at(&poly, g4) > 0.3 * at(&one, g4));
        assert!(rms(&poly) < 3.0 * rms(&one), "a chord must not be wildly louder");
    }

    #[test]
    fn poly_note_offs_release_every_voice_and_steal_when_full() {
        let sr = 48000.0;
        let mut v = Voice::new(sr);
        v.set_param("voices", 3.0 / 7.0);
        // 3 slots, 5 notes: the oldest are stolen, nothing panics, everything ends silent after the offs
        let mut out = vec![0.0f32; 256];
        for k in 60..65u8 {
            v.note_on_lyric(k, 0.8, 0.0, 0.5, 0.5, "", 0.0);
            v.render(&mut out);
        }
        for k in 60..65u8 {
            v.note_off(k);
        }
        let mut tail = vec![0.0f32; (sr * 2.0) as usize * 2];
        for c in tail.chunks_mut(256) {
            v.render(c);
        }
        assert!(tail.iter().all(|x| x.is_finite()));
        assert!(rms(&tail[tail.len() - 2000..]) < 1e-3, "a voice is stuck on");
    }

    #[test]
    fn the_new_voice_knobs_change_the_sound_and_stay_clean() {
        let sr = 48000.0;
        let base = {
            let mut v = Voice::new(sr);
            v.set_param("vib depth", 0.0);
            mono(&sing(&mut v, 57, "a", 0.8))
        };
        for (name, val) in [("choir", 0.8), ("open", 0.95), ("open", 0.05), ("fry", 0.9), ("drift", 0.9), ("scoop", 0.8), ("ring", 0.9), ("trem", 0.9), ("nasal", 0.9)] {
            let mut v = Voice::new(sr);
            v.set_param("vib depth", 0.0);
            v.set_param(name, val);
            let o = mono(&sing(&mut v, 57, "a", 0.8));
            assert!(o.iter().all(|x| x.is_finite()), "{name}");
            let pk = o.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            assert!(pk > 0.02 && pk < 1.2, "{name}: peak {pk}");
            let diff: f32 = o.iter().zip(&base).map(|(a, b)| (a - b).abs()).sum::<f32>() / o.len() as f32;
            assert!(diff > 0.0005, "{name} {val} does nothing ({diff})");
        }
    }

    /// which of C4 / E4 / G4 is loudest in each step-long window of a rendered arpeggio (0, 1, 2)
    fn arp_notes(v: &mut Voice, bpm: f64, steps: usize, step_s: f32) -> Vec<usize> {
        let sr = v.sr;
        v.set_tempo(bpm);
        for k in [60u8, 64, 67] {
            v.note_on_lyric(k, 0.9, 0.0, 0.5, 0.5, "", 0.0);
        }
        let mut out = vec![0.0f32; (sr * step_s * steps as f32) as usize * 2];
        for c in out.chunks_mut(128) {
            v.render(c);
        }
        assert!(out.iter().all(|x| x.is_finite()));
        let m = mono(&out);
        (0..steps)
            .map(|i| {
                let a = ((i as f32 * step_s + 0.35 * step_s) * sr) as usize;
                let w = &m[a..a + (0.5 * step_s * sr) as usize];
                let e: Vec<f32> = [261.6, 329.6, 392.0].iter().map(|f| band(w, sr, f - 12.0, f + 12.0)).collect();
                (0..3).max_by(|x, y| e[*x].total_cmp(&e[*y])).unwrap()
            })
            .collect()
    }

    #[test]
    fn the_arpeggiator_walks_through_a_held_chord_in_time() {
        let mut v = Voice::new(48000.0);
        v.set_param("vib depth", 0.0);
        v.set_param("arp", 1.0);
        v.set_param("arp rate", 0.2); // 1/8
        // 120 BPM: an eighth is 0.25 s. Up: C E G C ...
        assert_eq!(arp_notes(&mut v, 120.0, 6, 0.25), vec![0, 1, 2, 0, 1, 2]);
        // a faster tempo shortens the steps
        let mut v = Voice::new(48000.0);
        v.set_param("vib depth", 0.0);
        v.set_param("arp", 1.0);
        v.set_param("arp rate", 0.2);
        assert_eq!(arp_notes(&mut v, 240.0, 6, 0.125), vec![0, 1, 2, 0, 1, 2]);
    }

    #[test]
    fn the_arpeggiator_modes_change_the_order() {
        let run = |mode: f32| {
            let mut v = Voice::new(48000.0);
            v.set_param("vib depth", 0.0);
            v.set_param("arp", 1.0);
            v.set_param("arp rate", 0.2);
            v.set_param("arp mode", mode);
            arp_notes(&mut v, 120.0, 6, 0.25)
        };
        assert_eq!(run(0.25), vec![2, 1, 0, 2, 1, 0], "down");
        assert_eq!(run(0.5), vec![0, 1, 2, 1, 0, 1], "up and down, no repeated ends");
        let r = run(0.75);
        assert!(r.iter().collect::<std::collections::HashSet<_>>().len() >= 2, "random visits several notes: {r:?}");
    }

    #[test]
    fn the_arpeggiator_stops_with_the_keys_and_climbs_octaves() {
        let sr = 48000.0;
        let mut v = Voice::new(sr);
        v.set_param("vib depth", 0.0);
        v.set_param("arp", 1.0);
        v.set_param("arp rate", 0.2);
        v.set_param("arp oct", 1.0); // three octaves
        v.set_tempo(120.0);
        v.note_on_lyric(60, 0.9, 0.0, 0.5, 0.5, "", 0.0);
        let mut out = vec![0.0f32; (sr * 1.5) as usize * 2];
        for c in out.chunks_mut(100) {
            v.render(c);
        }
        let m = mono(&out);
        let at = |t: f32, hz: f32| band(&m[(sr * t) as usize..(sr * (t + 0.12)) as usize], sr, hz - 12.0, hz + 12.0);
        // C4, C5, C6 in the first three steps: each octave up loses the fundamental below it (the harmonics stay)
        assert!(at(0.09, 261.6) > 0.0, "step 1 is C4");
        assert!(at(0.34, 261.6) < 0.1 * at(0.09, 261.6), "step 2 has no C4 left");
        assert!(at(0.34, 523.3) > 5.0 * at(0.34, 261.6), "step 2 is C5");
        assert!(at(0.59, 523.3) < 0.1 * at(0.34, 523.3), "step 3 has no C5 left");
        assert!(at(0.59, 1046.5) > 5.0 * at(0.59, 523.3), "step 3 is C6");
        // releasing the key ends the sequence
        v.note_off(60);
        let mut tail = vec![0.0f32; (sr * 1.5) as usize * 2];
        for c in tail.chunks_mut(100) {
            v.render(c);
        }
        assert!(rms(&mono(&tail)[(sr * 1.0) as usize..]) < 1e-3, "the arpeggio keeps playing after the key came up");
    }

    #[test]
    fn switching_the_arpeggiator_off_releases_its_note() {
        let sr = 48000.0;
        let mut v = Voice::new(sr);
        v.set_param("arp", 1.0);
        v.note_on_lyric(60, 0.9, 0.0, 0.5, 0.5, "", 0.0);
        let mut b = vec![0.0f32; 4800 * 2];
        v.render(&mut b);
        v.set_param("arp", 0.0);
        v.note_off(60);
        let mut tail = vec![0.0f32; (sr * 2.0) as usize * 2];
        for c in tail.chunks_mut(256) {
            v.render(c);
        }
        assert!(rms(&mono(&tail)[(sr * 1.5) as usize..]) < 1e-3, "a note is stuck");
    }

    #[test]
    fn a_chord_of_tied_notes_slurs_each_voice_to_the_nearest_new_pitch() {
        let sr = 48000.0;
        let mut v = Voice::new(sr);
        v.set_param("vib depth", 0.0);
        v.set_param("voices", 3.0 / 7.0);
        for k in [60u8, 64, 67] {
            v.note_on_lyric(k, 0.9, 0.0, 0.5, 0.5, "a", 0.0);
        }
        let mut first = vec![0.0f32; (sr * 0.5) as usize * 2];
        for c in first.chunks_mut(128) {
            v.render(c);
        }
        // the next chord arrives tied ("~") as the first ends: three notes, one sustained vowel
        for k in [60u8, 64, 67] {
            v.note_off(k);
        }
        for k in [62u8, 65, 69] {
            v.note_on_lyric(k, 0.9, 0.0, 0.5, 0.5, "~", 0.0);
        }
        let mut second = vec![0.0f32; (sr * 0.6) as usize * 2];
        for c in second.chunks_mut(128) {
            v.render(c);
        }
        assert!(second.iter().all(|x| x.is_finite()));
        let m = mono(&second);
        let w = &m[(sr * 0.3) as usize..(sr * 0.5) as usize];
        let e: Vec<f32> = [293.7f32, 349.2, 440.0].iter().map(|f| band(w, sr, f - 10.0, f + 10.0)).collect();
        let old = band(w, sr, 261.6 - 10.0, 261.6 + 10.0);
        let top = e.iter().cloned().fold(0.0f32, f32::max);
        assert!(e.iter().all(|x| *x > 0.1 * top), "all three voices of the new chord sound, not one: {e:?}");
        assert!(old < 0.05 * top, "the old C is gone: {old} vs {top}");
        assert!(rms(w) > 0.01, "the sustained vowel did not drop out");
    }

    #[test]
    fn state_round_trips_through_a_patch() {
        let mut v = Voice::new(48000.0);
        v.load_preset("ren");
        let text = crate::patch::dump("voice", "ren", &v.state(), &[]);
        let p = crate::patch::parse(&text);
        assert_eq!(p.kind, "voice");
        let v2 = crate::patch::build(&p, 48000.0).unwrap();
        for ((a, x), (b, y)) in v.params().iter().zip(v2.params().iter()) {
            assert_eq!(a, b);
            assert!((x - y).abs() < 0.002, "{a}: {x} vs {y}");
        }
    }
}
