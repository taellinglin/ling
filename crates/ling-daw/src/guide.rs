//! A MIDI file as guidance for a generated song: File > New AI Song can be given a MIDI file and a "follow the MIDI" amount.
//!
//! The file is read the way a reference recording is ([`crate::songgen::analyze_reference`]), but from the notes instead of the audio:
//! tempo, key (from the pitch classes, weighted by length and velocity), the chord loop of its busiest stretch (one diatonic triad per bar),
//! the energy of every bar (which becomes the sections) and the lines the parts play (bass, chords, pad, melody, arpeggio, drums), each
//! tagged with the role it most likely has.
//!
//! How much of it the song keeps is the *conform* amount, 0..1, and works bar by bar:
//!   0       the file is ignored;
//!   > 0     tempo, key, chords and structure are taken from it (the prompt still wins);
//!   >= 0.4  the instruments the file has and the genre lacks are added to the song;
//!   between  every bar of a part that has a counterpart in the file plays the FILE's notes with that probability (the rest is written
//!           fresh over the same chords), so 0.5 is half the bars from the file, half new, chosen the same way every time for one seed;
//!   1       the song's parts play the file's notes everywhere (in the song's key, in the part's register).
use crate::compose::Kind;
use crate::midi_import::ImportedMidi;
use crate::songgen::{self, Section};
use crate::theory::{self, Key, N};

#[derive(Clone, Debug)]
pub struct GuideTrack {
    pub name: String,
    pub role: &'static str,
    pub kind: Kind,
    /// notes in beats from the start of the file
    pub notes: Vec<N>,
}

#[derive(Clone, Debug)]
pub struct Guide {
    pub bpm: f64,
    pub key: Key,
    pub key_conf: f64,
    /// the chord loop as Roman numerals ("i VI III VII"), empty when no clear loop
    pub prog: String,
    pub sections: Vec<Section>,
    pub bars: u32,
    pub genre: &'static str,
    pub tracks: Vec<GuideTrack>,
    /// what else the file says (songanalysis.rs): labelled structure, energy / tension per bar, the emotional story, hook, groove
    pub shape: crate::songanalysis::Shape,
}

fn bar_hist(notes: &[(&N, f32)], bar: usize) -> [f64; 12] {
    let (a, b) = (bar as f64 * 4.0, bar as f64 * 4.0 + 4.0);
    let mut h = [0.0f64; 12];
    for (n, w) in notes {
        let (s, e) = (n.start.max(a), (n.start + n.len).min(b));
        if e > s {
            h[(n.key % 12) as usize] += (e - s) * n.vel as f64 * *w as f64;
        }
    }
    h
}

/// energy per bar (0..1) -> sections: 4-bar blocks merged while they are alike, tagged intro / verse / chorus / break / outro
fn sections_from_energy(en: &[f64]) -> Vec<Section> {
    let nb = en.len();
    let mut blocks: Vec<(usize, usize, f64)> = vec![];
    let mut b = 0;
    while b < nb {
        let l = 4.min(nb - b);
        let m = en[b..b + l].iter().sum::<f64>() / l as f64;
        match blocks.last_mut() {
            Some(last) if (last.2 - m).abs() < 0.16 && last.1 < 16 => {
                last.2 = (last.2 * last.1 as f64 + m * l as f64) / (last.1 + l) as f64;
                last.1 += l;
            }
            _ => blocks.push((b, l, m)),
        }
        b += l;
    }
    let mut merged: Vec<(usize, usize, f64)> = vec![];
    for blk in blocks {
        match merged.last_mut() {
            Some(last) if blk.1 < 8 || last.1 < 4 => {
                last.2 = (last.2 * last.1 as f64 + blk.2 * blk.1 as f64) / (last.1 + blk.1) as f64;
                last.1 += blk.1;
            }
            _ => merged.push(blk),
        }
    }
    let hi = merged.iter().map(|x| x.2).fold(0.0, f64::max).max(1e-6);
    let n = merged.len();
    let mut seen_high = false;
    let mut out: Vec<Section> = vec![];
    for (i, (_, bars, m)) in merged.iter().enumerate() {
        let rel = m / hi;
        let tag = if i == 0 && rel < 0.7 {
            "intro"
        } else if i + 1 == n && rel < 0.7 {
            "outro"
        } else if rel >= 0.82 {
            seen_high = true;
            "chorus"
        } else if rel < 0.55 && seen_high {
            "break"
        } else {
            "verse"
        };
        out.push(Section { tag: tag.into(), bars: ((((*bars as f64) / 4.0).round() * 4.0) as u32).clamp(4, 24), energy: (0.25 + 0.75 * rel).min(1.0) });
    }
    for i in 1..out.len() {
        if out[i].tag == "chorus" && out[i - 1].tag == "chorus" && i % 2 == 1 {
            out[i].tag = "drop".into();
        }
    }
    out
}

/// Read a MIDI file as a guide; `None` when it holds no notes.
pub fn analyze(mid: &ImportedMidi) -> Option<Guide> {
    let mut tracks: Vec<GuideTrack> = vec![];
    for t in &mid.tracks {
        if t.notes.is_empty() || !t.samples.is_empty() {
            continue;
        }
        let role = crate::ai_import::guess_role(t);
        let kind = songgen::role_kind(role);
        tracks.push(GuideTrack { name: t.name.clone(), role, kind, notes: t.notes.iter().map(|n| N { start: n.start, len: n.len, key: n.key, vel: n.vel }).collect() });
    }
    if tracks.is_empty() {
        return None;
    }
    let end = tracks.iter().flat_map(|t| t.notes.iter()).map(|n| n.start + n.len).fold(0.0, f64::max);
    let bars = ((end.max(mid.length_beats) / 4.0).ceil() as u32).max(1);
    // key from every pitched note, long and loud ones counting more; the bass counts double (it names the root)
    let pitched: Vec<(&N, f32)> = tracks.iter().filter(|t| t.kind != Kind::Drums).flat_map(|t| t.notes.iter().map(move |n| (n, if t.kind == Kind::Bass { 2.0 } else { 1.0 }))).collect();
    let mut hist = [0.0f64; 12];
    for (n, w) in &pitched {
        hist[(n.key % 12) as usize] += n.len.min(4.0) * n.vel as f64 * *w as f64;
    }
    let (key, key_conf) = if hist.iter().sum::<f64>() > 0.0 {
        let (k, c, _) = theory::detect_key_hist(&hist);
        (k, c)
    } else {
        (Key::new(9, true), 0.0)
    };
    // energy per bar: how many notes, how loud, over every part
    let all: Vec<&N> = tracks.iter().flat_map(|t| t.notes.iter()).collect();
    let mut e = vec![0.0f64; bars as usize];
    for n in &all {
        let b = (n.start / 4.0).floor().max(0.0) as usize;
        if b < e.len() {
            e[b] += n.vel as f64 * n.len.min(1.0).max(0.1);
        }
    }
    let top = e.iter().cloned().fold(1e-9, f64::max);
    let en: Vec<f64> = e.iter().map(|x| (x / top).clamp(0.0, 1.0)).collect();
    // the structure from repetition, energy and drums (songanalysis.rs), else 4-bar energy blocks
    let shape = {
        let parts: Vec<crate::songanalysis::PartIn> = tracks.iter().map(|t| crate::songanalysis::PartIn { name: &t.name, role: t.role, kind: t.kind, notes: &t.notes }).collect();
        crate::songanalysis::analyze_notes(&parts, bars as usize, key)
    };
    let sections = if shape.sections.len() >= 2 { shape.sections.clone() } else if bars >= 8 { sections_from_energy(&en) } else { vec![] };
    // chords: one triad per bar over the busiest eight bars, then the loop that repeats
    let mut prog = String::new();
    if hist.iter().sum::<f64>() > 0.0 && bars >= 2 {
        let span = 8.min(bars as usize);
        let start = (0..=(bars as usize - span)).max_by(|a, b| en[*a..*a + span].iter().sum::<f64>().total_cmp(&en[*b..*b + span].iter().sum::<f64>())).unwrap_or(0);
        let chords: Vec<usize> = (0..span)
            .filter_map(|j| {
                let h = bar_hist(&pitched, start + j);
                if h.iter().sum::<f64>() <= 0.0 { None } else { Some(songgen::best_triad(&h, key)) }
            })
            .collect();
        let loop_len = if chords.len() == 8 && chords[..4] == chords[4..] { 4 } else { chords.len().min(8) };
        let mut p: Vec<&str> = chords[..loop_len].iter().map(|d| songgen::roman(*d, key.minor)).collect();
        p.dedup();
        if let Some(l) = (2..p.len()).find(|l| (0..p.len()).all(|i| p[i] == p[i % l])) {
            p.truncate(l);
        }
        if p.len() >= 2 {
            prog = p.join(" ");
        }
    }
    Some(Guide { bpm: mid.bpm.unwrap_or(120.0).clamp(40.0, 220.0).round(), key, key_conf, prog, sections, bars, genre: crate::ai_import::guess_genre(mid), tracks, shape })
}

/// An acapella (transcribe.rs) as a guide: its tempo, key, the chord loop fitted to its melody and its layout (an intro before the
/// singing, the singer's sections, the band's sections where it rests, an outro), so every bar of the vocal lands where it was sung.
/// It has no parts of its own (the plan's chords follow the fitted loops; the exact chords go on a track of their own).
pub fn from_acapella(a: &crate::transcribe::Acapella) -> Guide {
    Guide {
        bpm: a.bpm,
        key: a.key,
        key_conf: a.key_conf,
        prog: a.prog.clone(),
        sections: a.sections.clone(),
        bars: a.sections.iter().map(|s| s.bars).sum(),
        genre: "pop",
        tracks: vec![],
        shape: a.shape.clone(),
    }
}

impl Guide {
    /// the n-th busiest track of a kind
    pub fn nth_of_kind(&self, kind: Kind, n: usize) -> Option<&GuideTrack> {
        let mut v: Vec<&GuideTrack> = self.tracks.iter().filter(|t| t.kind == kind).collect();
        v.sort_by(|a, b| b.notes.len().cmp(&a.notes.len()));
        v.get(n).copied()
    }

    /// one sentence for the dialog
    pub fn describe(&self) -> String {
        let kinds: Vec<String> = self.tracks.iter().map(|t| format!("{} ({})", t.name, t.role)).take(8).collect();
        format!(
            "{} bars at {:.0} bpm in {}{}{}; {}",
            self.bars,
            self.bpm,
            self.key.name(),
            if self.prog.is_empty() { String::new() } else { format!(", chords {}", self.prog) },
            if self.sections.is_empty() { String::new() } else { format!(", {} sections", self.sections.len()) },
            kinds.join(", ")
        )
    }
}

/// semitones to move the guide into the song's key: the shortest way, -6..=5
pub fn key_shift(from: Key, to: Key) -> i32 {
    // compare relative keys by their tonic when the mode matches, else by the relative major/minor
    let f = if from.minor == to.minor { from.tonic as i32 } else if from.minor { (from.tonic as i32 + 3) % 12 } else { (from.tonic as i32 + 9) % 12 };
    let mut d = (to.tonic as i32 - f).rem_euclid(12);
    if d > 6 {
        d -= 12;
    }
    d
}

/// 0..1, the same for the same (seed, part, bar): does this bar play the file's notes?
pub fn take_bar(seed: u64, part: usize, bar: u32, conform: f64) -> bool {
    if conform >= 0.98 {
        return true;
    }
    if conform <= 0.0 {
        return false;
    }
    let mut x = seed ^ (part as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (bar as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    ((x >> 11) as f64 / (1u64 << 53) as f64) < conform
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi_import::ImportedTrack;
    use crate::model::Note;

    fn track(name: &str, program: Option<u8>, notes: Vec<Note>) -> ImportedTrack {
        ImportedTrack { named: true, plugin: String::new(), name: name.into(), program, bank: 0, channel: 0, notes, volume: 1.0, pan: 0.0, vol_auto: vec![], pan_auto: vec![], bend_auto: vec![], bend_range: 2.0, samples: vec![], sampler: None, percussive: false, extra_auto: vec![] }
    }

    fn midi() -> ImportedMidi {
        // A minor: i VI III VII (Am F C G) for 16 bars, a bass on the roots, a melody on A C E
        let roots = [57u8, 53, 48, 55];
        let chords: Vec<Note> = (0..16).flat_map(|b| { let r = roots[b % 4]; [r, r + if b % 4 == 0 { 3 } else if b % 4 == 3 { 4 } else { 4 }, r + 7].into_iter().map(move |k| Note::new(b as f64 * 4.0, 3.8, k, 0.7)) }).collect();
        let bass: Vec<Note> = (0..16).flat_map(|b| (0..4).map(move |i| Note::new(b as f64 * 4.0 + i as f64, 0.8, roots[b % 4] - 24, 0.9))).collect();
        let lead: Vec<Note> = (0..64).map(|b| Note::new(b as f64, 0.5, 69 + [0, 3, 7, 3][b % 4] as u8, 0.8)).collect();
        ImportedMidi { tracks: vec![track("Chords", Some(89), chords), track("Bass", Some(38), bass), track("Lead", Some(80), lead)], bpm: Some(124.0), tempos: vec![], length_beats: 64.0 }
    }

    #[test]
    fn reads_tempo_key_and_chords() {
        let g = analyze(&midi()).unwrap();
        assert_eq!(g.bpm, 124.0);
        // Am F C G is A minor or its relative C major: both name the same notes
        assert!(matches!((g.key.tonic, g.key.minor), (9, true) | (0, false)), "{}", g.key.name());
        assert_eq!(g.bars, 16);
        assert!(!g.prog.is_empty(), "no chord loop found");
        assert!(g.tracks.iter().any(|t| t.kind == Kind::Bass));
    }

    #[test]
    fn the_amount_decides_how_many_bars_come_from_the_file() {
        let n = |c: f64| (0..200).filter(|b| take_bar(7, 1, *b, c)).count();
        assert_eq!(n(0.0), 0);
        assert_eq!(n(1.0), 200);
        let half = n(0.5);
        assert!((70..130).contains(&half), "{half}");
        // the same bars every time
        assert_eq!((0..50).filter(|b| take_bar(7, 1, *b, 0.5)).count(), (0..50).filter(|b| take_bar(7, 1, *b, 0.5)).count());
    }

    #[test]
    fn a_song_that_follows_the_file_plays_its_notes() {
        use crate::songgen;
        let g = std::sync::Arc::new(analyze(&midi()).unwrap());
        let bass_of = |conform: f64| {
            let plan = songgen::plan_with("house", "", &[], None, 1.0, 3, Some(&g), conform);
            assert_eq!(plan.bpm, if conform > 0.0 { 124.0 } else { plan.bpm });
            let bi = plan.parts.iter().position(|p| songgen::role_kind(&p.role) == Kind::Bass).expect("a bass part");
            let sec = plan.sections[0].clone();
            let notes = songgen::section_notes(&plan, bi, &plan.sections[0], None);
            let _ = sec;
            (plan, notes)
        };
        // all the way: the first bar is the file's bass, four notes on the root A
        let (plan, notes) = bass_of(1.0);
        assert!(plan.notes.iter().any(|n| n.contains("guide")), "{:?}", plan.notes);
        let bar0: Vec<&N> = notes.iter().filter(|n| n.start < 4.0).collect();
        assert_eq!(bar0.len(), 4, "{notes:?}");
        assert!(bar0.iter().all(|n| n.key % 12 == 9), "{bar0:?}");
        // not at all: the guide is ignored (nothing in the plan mentions it)
        let (plan0, _) = bass_of(0.0);
        assert!(!plan0.notes.iter().any(|n| n.contains("guide")));
        // halfway: some bars from the file, some written fresh
        let (planh, noteh) = bass_of(0.5);
        let bars_from_file = (0..planh.sections[0].bars).filter(|b| crate::guide::take_bar(planh.seed, planh.parts.iter().position(|p| songgen::role_kind(&p.role) == Kind::Bass).unwrap(), *b, 0.5)).count();
        assert!(bars_from_file > 0 && (bars_from_file as u32) < planh.sections[0].bars, "{bars_from_file} of {}", planh.sections[0].bars);
        assert!(!noteh.is_empty());
    }

    #[test]
    fn shifts_to_the_nearest_key() {
        assert_eq!(key_shift(Key::new(9, true), Key::new(9, true)), 0);
        assert_eq!(key_shift(Key::new(9, true), Key::new(11, true)), 2);
        assert_eq!(key_shift(Key::new(9, true), Key::new(8, true)), -1);
        assert_eq!(key_shift(Key::new(0, false), Key::new(9, true)), 0);   // C major to A minor: relative keys, no shift
    }
}
