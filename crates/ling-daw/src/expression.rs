//! Expression: the singer's performance line by line and word by word, on top of the emotional plan (emotion.rs).
//!
//! The plan says how each section feels; this decides how each sung line and each word is *delivered* in that feeling, the way a
//! singer phrases rather than the way a sequencer plays:
//!
//! * **Lines.** The sung notes split into lines (the lyric's line breaks, or a rest). Every line is an arch: it starts a little
//!   under, grows to its high point and tapers at the end - more taper when the feeling is calm or sad. A long last note is a
//!   gesture of its own: swelled and held when the section is intense, let go of (diminuendo) when it is not. Breathy onsets
//!   start the lines of an intimate section.
//! * **Words.** Each syllable gets a stress from the word (function words stay light, the stressed syllable of a longer word
//!   leans: "a-LONE", "FOR-est"), the beat it falls on and how long it is. Feeling words ("tears", "alive" - emotion.rs's
//!   lexicon) are leaned on harder: louder, a deeper scoop into the pitch, a cry (a quick dip) on the sad ones, brightness on
//!   the happy ones, and the vibrato blooms on them when they are held.
//! * **Intonation.** The `tune` lane (cents) carries what a singer does with pitch: a sad section sings a few cents under, a
//!   climax pushes a few cents sharp on its stressed notes, the end of a sad line falls off, and a little human scatter.
//!
//! Everything comes out as velocities and automation lanes on the voice track (`expr`, `tune`, `scoop`, `vib depth`, `breath`,
//! `bright`), so it can be seen and edited in the DAW; `amount` 0 leaves the plan's flat per-section values.
use crate::compose::Rng;
use crate::emotion::{word_feel, Feel};
use crate::lyricfit::{self, NoteIn, Opts};
use crate::model::{AutoLane, ClipData, Track};

/// One sung note, absolute in beats, with the words it carries.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sung {
    pub start: f64,
    pub len: f64,
    pub key: u8,
    pub vel: f32,
    /// the written syllable (without markers); "" when the note holds the syllable before it or sings a vowel
    pub text: String,
    /// holds the syllable before it (a melisma or a tie): no new attack
    pub held: bool,
    pub word_start: bool,
    pub line_start: bool,
}

/// A stretch of the song with one feeling.
#[derive(Clone, Copy, Debug)]
pub struct Span {
    pub start: f64,
    pub end: f64,
    pub feel: Feel,
}

#[derive(Clone, Debug, Default)]
pub struct Performance {
    /// the new velocity of every note, in the order given
    pub vels: Vec<f32>,
    /// (instrument parameter, points (beat, 0..1) sorted)
    pub lanes: Vec<(&'static str, Vec<(f64, f32)>)>,
    /// what was done, in words
    pub notes: Vec<String>,
}

/// the cents either way at the ends of the voice's `tune` parameter (dsp/voice.rs TUNE_RANGE)
const TUNE_RANGE: f32 = 200.0;

fn tune_val(cents: f32) -> f32 {
    (0.5 + cents / (2.0 * TUNE_RANGE)).clamp(0.0, 1.0)
}

const FUNCTION_WORDS: &[&str] = &[
    "a", "an", "the", "and", "or", "but", "so", "to", "of", "in", "on", "at", "by", "for", "from", "with", "as", "is", "am", "are", "was",
    "were", "be", "been", "it", "its", "i", "me", "my", "you", "your", "we", "our", "us", "he", "him", "his", "she", "her", "they",
    "them", "their", "this", "that", "these", "those", "do", "does", "did", "will", "would", "can", "could", "if", "then", "than",
    "oh", "ah", "ooh", "la", "na", "yeah", "just", "all", "not", "no",
];

/// prefixes that leave the stress on the second syllable ("a-LONE", "be-CAUSE", "for-GET", "to-NIGHT")
const LIGHT_PREFIXES: &[&str] = &["a", "be", "de", "re", "un", "for", "to", "ex", "en", "em", "im", "in", "con", "com", "sur", "pre", "pro", "with", "a-"];

/// which syllable of a word of `n` syllables carries the stress (English by rule)
fn stressed_syllable(word: &str, first_syl: &str, n: usize) -> usize {
    if n < 2 {
        return 0;
    }
    let w = word.to_lowercase();
    let f = first_syl.to_lowercase();
    if LIGHT_PREFIXES.contains(&f.as_str()) && w.len() > f.len() + 2 {
        return 1;
    }
    0
}

/// how strong a beat position is in 4/4: the one, the three, the backbeats, the offbeats
fn metric(beat: f64) -> f32 {
    let p = beat.rem_euclid(4.0);
    let near = |x: f64| (p - x).abs() < 0.06;
    if near(0.0) || near(4.0) {
        1.0
    } else if near(2.0) {
        0.75
    } else if near(1.0) || near(3.0) {
        0.55
    } else if (p * 2.0 - (p * 2.0).round()).abs() < 0.06 {
        0.3
    } else {
        0.15
    }
}

/// The sung notes of a voice track's clips, with their words (from each clip's typed lyric and its fit options, as karaoke.rs
/// reads them), and where each came from: (clip, note) per entry. Sorted by start.
pub fn sung_notes(tr: &Track) -> (Vec<Sung>, Vec<(usize, usize)>) {
    let mut out: Vec<(Sung, (usize, usize))> = vec![];
    for (ci, c) in tr.clips.iter().enumerate() {
        let ClipData::Midi(ns) = &c.data else { continue };
        let texts: Vec<String> = if !c.lyric_text.trim().is_empty() {
            let opts = Opts::parse(&c.lyric_lang).unwrap_or_default();
            let input: Vec<NoteIn> = ns.iter().map(|n| NoteIn { start: n.start, len: n.len, key: n.key }).collect();
            lyricfit::fit_text(&input, &c.lyric_text, &opts)
        } else {
            ns.iter().map(|n| if n.lyric.is_empty() || n.lyric == "~" || n.lyric.starts_with('~') { "~".to_string() } else { format!(" {}", n.lyric) }).collect()
        };
        for (ni, n) in ns.iter().enumerate() {
            if n.start >= c.len {
                continue;
            }
            let raw = texts.get(ni).cloned().unwrap_or_default();
            let line_start = raw.starts_with('\n');
            let body = raw.trim_start_matches('\n');
            let word_start = body.starts_with(' ');
            let clean = body.trim().trim_matches('-').to_string();
            let held = clean.is_empty() || clean == "~" || n.lyric.starts_with('~');
            out.push((
                Sung { start: c.start + n.start, len: n.len, key: n.key, vel: n.vel, text: if held { String::new() } else { clean }, held, word_start: word_start && !held, line_start },
                (ci, ni),
            ));
        }
    }
    out.sort_by(|a, b| a.0.start.total_cmp(&b.0.start));
    out.into_iter().unzip()
}

fn feel_at(spans: &[Span], beat: f64) -> Feel {
    spans
        .iter()
        .find(|s| beat >= s.start - 1e-6 && beat < s.end)
        .or_else(|| spans.iter().rev().find(|s| s.start <= beat))
        .or(spans.first())
        .map(|s| s.feel)
        .unwrap_or(Feel { valence: 0.0, arousal: 0.5, tension: 0.3, vocal: 0.5, breath: 0.4, space: 0.6 })
}

/// per-section base values of the voice lanes (the flat plan song_api.rs writes when expression is off)
pub fn base(f: &Feel) -> [(&'static str, f32); 4] {
    [
        ("breath", 0.12 + 0.45 * f.breath),
        ("vib depth", 0.2 + 0.45 * f.vocal),
        ("bright", (0.35 + 0.4 * f.vocal + 0.1 * f.valence).clamp(0.1, 0.95)),
        ("scoop", (0.1 + 0.3 * f.vocal + if f.valence < -0.2 { 0.1 } else { 0.0 }).min(0.6)),
    ]
}

/// Perform the sung notes in the feelings of the spans. `bpm` turns beats into seconds (what a singer can do in a note depends
/// on how long it really is); `amount` 0..1 how far the performance goes; `seed` the scatter.
pub fn perform(notes: &[Sung], spans: &[Span], bpm: f64, amount: f32, seed: u64) -> Performance {
    let amount = amount.clamp(0.0, 1.0);
    let n = notes.len();
    let mut p = Performance { vels: notes.iter().map(|s| s.vel).collect(), ..Performance::default() };
    if n == 0 {
        return p;
    }
    let secs = |beats: f64| beats * 60.0 / bpm.max(20.0);
    let beats_of = |s: f64| s * bpm.max(20.0) / 60.0;
    let mut rng = Rng::new(seed ^ 0xE4B2E55);

    // ── words: the notes of each word, its text, and which syllable leans ──
    let mut word_of = vec![0usize; n];
    let mut words: Vec<Vec<usize>> = vec![];
    for i in 0..n {
        let new = words.is_empty() || notes[i].word_start || (!notes[i].held && notes[i].text.is_empty());
        if new {
            words.push(vec![]);
        }
        let w = words.len() - 1;
        words[w].push(i);
        word_of[i] = w;
    }
    let word_text: Vec<String> = words.iter().map(|ix| ix.iter().map(|&i| notes[i].text.as_str()).collect::<String>()).collect();
    let word_emo: Vec<Option<(f32, f32)>> = word_text.iter().map(|w| word_feel(w)).collect();

    // ── lines: the lyric's breaks, a rest, or a new section ──
    let mut lines: Vec<Vec<usize>> = vec![];
    for i in 0..n {
        let gap = if i == 0 { f64::MAX } else { notes[i].start - (notes[i - 1].start + notes[i - 1].len) };
        let new_span = i > 0 && spans.iter().any(|s| s.start > notes[i - 1].start + 1e-6 && s.start <= notes[i].start + 1e-6);
        if i == 0 || (notes[i].line_start && !notes[i].held) || gap >= 0.75 || new_span {
            lines.push(vec![]);
        }
        lines.last_mut().unwrap().push(i);
    }

    // ── stress per note ──
    let mut stress = vec![0.0f32; n];
    for line in &lines {
        let mut lens: Vec<f64> = line.iter().map(|&i| notes[i].len).collect();
        lens.sort_by(|a, b| a.total_cmp(b));
        let median = lens[lens.len() / 2].max(0.05);
        for &i in line {
            if notes[i].held {
                stress[i] = if i > 0 { stress[i - 1] } else { 0.4 };
                continue;
            }
            let w = word_of[i];
            let syls: Vec<usize> = words[w].iter().copied().filter(|&k| !notes[k].held).collect();
            let idx = syls.iter().position(|&k| k == i).unwrap_or(0);
            let lw = word_text[w].to_lowercase();
            let lex = if notes[i].text.is_empty() {
                0.5
            } else if syls.len() <= 1 {
                if FUNCTION_WORDS.contains(&lw.as_str()) {
                    0.15
                } else {
                    0.8
                }
            } else {
                // the dictionary's stress when it splits the word into as many syllables as were sung, else the prefix rule
                let dict = crate::cmudict::stress(&lw).filter(|v| v.len() == syls.len());
                match dict.as_ref().and_then(|v| v.get(idx)) {
                    Some(1) => 0.9,
                    Some(2) => 0.55,
                    Some(_) => 0.25,
                    None if idx == stressed_syllable(&lw, &notes[syls[0]].text, syls.len()) => 0.9,
                    None => 0.25,
                }
            };
            let len_k = ((notes[i].len / median).clamp(0.5, 2.0) / 2.0) as f32;
            let emo = word_emo[w].map(|(v, a)| (v.abs() + a.max(0.0)).min(1.0)).unwrap_or(0.0);
            // only the stressed syllable of a feeling word carries its weight
            let emo = if lex >= 0.8 { emo } else { emo * 0.3 };
            stress[i] = (0.5 * lex + 0.3 * metric(notes[i].start) + 0.2 * len_k + 0.3 * emo).clamp(0.0, 1.0);
        }
    }

    let mut lanes: Vec<(&'static str, Vec<(f64, f32)>)> = ["expr", "tune", "scoop", "vib depth", "breath", "bright"].iter().map(|k| (*k, vec![])).collect();
    let lane = |lanes: &mut Vec<(&'static str, Vec<(f64, f32)>)>, k: &str, t: f64, v: f32| {
        if let Some(l) = lanes.iter_mut().find(|l| l.0 == k) {
            l.1.push((t.max(0.0), v.clamp(0.0, 1.0)));
        }
    };
    let (mut leaned, mut falls, mut swells, mut cries) = (vec![], 0usize, 0usize, 0usize);

    for line in &lines {
        let first = line[0];
        let last = *line.last().unwrap();
        let l0 = notes[first].start;
        let l1 = notes[last].start + notes[last].len;
        let f = feel_at(spans, l0);
        let b = base(&f);
        let bv = |k: &str| b.iter().find(|x| x.0 == k).map(|x| x.1).unwrap_or(0.5);
        // the arch: its high point is the line's highest note (the longest of those)
        let peak = line.iter().copied().filter(|&i| !notes[i].held).max_by(|&a, &c| (notes[a].key, (notes[a].len * 100.0) as i64).cmp(&(notes[c].key, (notes[c].len * 100.0) as i64))).unwrap_or(first);
        let peak_u = (((notes[peak].start - l0) / (l1 - l0).max(0.01)) as f32).clamp(0.15, 0.85);
        let depth = amount * (0.5 + 0.5 * f.arousal);
        let taper = 1.0 - amount * (0.18 + 0.2 * (1.0 - f.arousal) + 0.15 * (-f.valence).max(0.0));
        let onset = 1.0 - depth * (0.22 - 0.1 * f.vocal);
        let contour = |t: f64| -> f32 {
            let u = (((t - l0) / (l1 - l0).max(0.01)) as f32).clamp(0.0, 1.0);
            if u <= peak_u {
                onset + (1.0 - onset) * (u / peak_u).powf(0.8)
            } else {
                1.0 - (1.0 - taper) * ((u - peak_u) / (1.0 - peak_u)).powf(1.3)
            }
        };

        // dynamics within the line: the arch on the expression lane, a breath of a gap reset to the onset level
        lane(&mut lanes, "expr", l0 - 0.03, onset);
        for &i in line {
            if !notes[i].held {
                lane(&mut lanes, "expr", notes[i].start, contour(notes[i].start));
            }
        }
        let fin = &notes[last];
        let fin_s = secs(fin.len);
        if fin_s >= 0.8 && amount > 0.0 {
            if f.arousal > 0.6 {
                // a held climax: swell into it, hold it
                lane(&mut lanes, "expr", fin.start + fin.len * 0.45, (contour(fin.start) + 0.12 * amount).min(1.0));
                lane(&mut lanes, "expr", l1 - 0.02, (contour(fin.start) + 0.06 * amount).min(1.0));
                swells += 1;
            } else {
                // let it go
                lane(&mut lanes, "expr", fin.start + fin.len * 0.3, contour(fin.start));
                lane(&mut lanes, "expr", l1 - 0.02, taper * (1.0 - 0.15 * amount));
            }
        } else {
            lane(&mut lanes, "expr", l1 - 0.02, taper);
        }

        // breathy onsets in an intimate section
        let air = amount * 0.3 * f.breath;
        lane(&mut lanes, "breath", l0 - 0.03, bv("breath") + air + 0.05 * amount);
        lane(&mut lanes, "breath", (l0 + beats_of(0.35)).min(l1), bv("breath"));
        lane(&mut lanes, "breath", l1, bv("breath"));

        let mut cried = false;
        for (pos, &i) in line.iter().enumerate() {
            let s = &notes[i];
            let w = word_of[i];
            let emo = word_emo[w];
            let st = stress[i];
            let end = s.start + s.len;
            let ns = secs(s.len);
            let line_end = pos + 1 == line.len();
            // velocity: the word's stress and the line's arch
            if !s.held {
                let mult = 1.0 + amount * (0.3 * (st - 0.45) + 0.25 * (contour(s.start) - 0.85));
                p.vels[i] = (s.vel * mult).clamp(0.12, 1.0);
            }
            // brightness: stressed and peak notes open up, happy words shine
            let warm = emo.map(|(v, _)| v.max(0.0)).unwrap_or(0.0);
            lane(&mut lanes, "bright", s.start, bv("bright") + amount * (0.14 * (st - 0.5) + 0.1 * (contour(s.start) - 0.85) + 0.1 * warm));

            // vibrato: straight on short notes, blooming on held ones - most on line ends and feeling words
            let vb = bv("vib depth");
            if ns >= 0.45 {
                let bloom = 0.25 + 0.35 * line_end as u8 as f32 + 0.3 * emo.map(|(v, a)| v.abs().max(a)).unwrap_or(0.0) + 0.2 * f.vocal;
                lane(&mut lanes, "vib depth", s.start, vb * (1.0 - 0.35 * amount));
                lane(&mut lanes, "vib depth", end - 0.02, (vb * (1.0 + amount * bloom)).min(0.95));
            } else {
                lane(&mut lanes, "vib depth", s.start, vb * (1.0 - 0.2 * amount));
            }

            if s.held {
                continue;
            }
            // the scoop into a new note: deeper on leaned-on feeling words, none on a note too short to slide in
            let mut scoop = bv("scoop");
            if ns < 0.18 {
                scoop = 0.0;
            } else if st < 0.35 {
                scoop *= 1.0 - 0.6 * amount;
            } else if let (Some((v, a)), true) = (emo, st >= 0.6) {
                scoop += amount * 0.25 * (v.abs() + a.max(0.0)).min(1.0);
            }
            lane(&mut lanes, "scoop", s.start - 0.04, scoop);
            lane(&mut lanes, "scoop", s.start + 0.01, scoop);

            // intonation: a sad section sings a little under, a climax pushes sharp on its stressed notes, a human scatter
            let sag = -12.0 * (-f.valence).max(0.0) * (1.0 - 0.5 * f.arousal);
            let push = 7.0 * ((f.arousal - 0.7) / 0.3).max(0.0) * st;
            let scatter = (rng.f() as f32 - 0.5) * 6.0;
            let colour = amount * (sag + push + scatter);
            lane(&mut lanes, "tune", s.start - 0.02, tune_val(colour));
            // a cry: a sad feeling word, leaned on, dips and recovers at its start
            if let Some((v, _)) = emo {
                // at most one cry a line, on a really sad word in a sad section: more is melodrama
                if v < -0.45 && f.valence < -0.2 && st >= 0.6 && ns >= 0.3 && amount > 0.0 && !cried {
                    cried = true;
                    let dip = colour - amount * 35.0 * (-v);
                    lane(&mut lanes, "tune", s.start + 0.005, tune_val(dip));
                    lane(&mut lanes, "tune", s.start + beats_of(0.12).min(s.len * 0.4), tune_val(colour));
                    cries += 1;
                }
                if !leaned.contains(&word_text[w]) && st >= 0.6 {
                    leaned.push(word_text[w].clone());
                }
            }
            // the end of the line: the sung word (with the notes that hold it) falls off when the feeling is sad
            let mut tail_end = end;
            let mut k = i + 1;
            while k <= last && notes[k].held {
                tail_end = notes[k].start + notes[k].len;
                k += 1;
            }
            let ends_line = k > last;
            let tail_s = secs(tail_end - s.start);
            let sad = f.valence < -0.15 || emo.map(|(v, _)| v < -0.3).unwrap_or(false);
            if ends_line && sad && tail_s >= 0.35 && amount > 0.0 {
                let fall = amount * (40.0 + 110.0 * (-f.valence).max(0.0) + 40.0 * (1.0 - f.arousal));
                lane(&mut lanes, "tune", s.start + (tail_end - s.start) * 0.7, tune_val(colour));
                lane(&mut lanes, "tune", tail_end - 0.01, tune_val(colour - fall));
                falls += 1;
            } else {
                lane(&mut lanes, "tune", tail_end - 0.02, tune_val(colour));
            }
        }
    }

    for l in lanes.iter_mut() {
        l.1.sort_by(|a, b| a.0.total_cmp(&b.0));
        // the same beat twice: the later point wins
        let mut out: Vec<(f64, f32)> = Vec::with_capacity(l.1.len());
        for pt in l.1.drain(..) {
            match out.last_mut() {
                Some(last) if (last.0 - pt.0).abs() < 1e-6 => *last = pt,
                _ => out.push(pt),
            }
        }
        l.1 = out;
    }
    p.lanes = lanes;
    if amount > 0.0 {
        let mut s = format!("expression: {} lines", lines.len());
        if !leaned.is_empty() {
            let show: Vec<&str> = leaned.iter().take(6).map(|w| w.as_str()).collect();
            s.push_str(&format!(", leaned on {}", show.join(", ")));
        }
        if falls > 0 {
            s.push_str(&format!(", {falls} falls"));
        }
        if cries > 0 {
            s.push_str(&format!(", {cries} cries"));
        }
        if swells > 0 {
            s.push_str(&format!(", {swells} held swells"));
        }
        p.notes.push(s);
    }
    p
}

/// Perform a voice track in place: its notes' velocities and the expression lanes (only the parameters its instrument has; a lane
/// of the same name is replaced). Returns what was done.
pub fn perform_track(tr: &mut Track, spans: &[Span], bpm: f64, amount: f32, seed: u64) -> Vec<String> {
    let (sung, at) = sung_notes(tr);
    if sung.is_empty() {
        return vec![];
    }
    let perf = perform(&sung, spans, bpm, amount, seed);
    for (k, (ci, ni)) in at.iter().enumerate() {
        if let Some(ClipData::Midi(ns)) = tr.clips.get_mut(*ci).map(|c| &mut c.data) {
            if let Some(n) = ns.get_mut(*ni) {
                n.vel = perf.vels[k];
            }
        }
    }
    let names: Vec<String> = tr.inst.as_ref().map(|i| i.params().into_iter().map(|(n, _)| n.to_string()).collect()).unwrap_or_default();
    for (name, pts) in perf.lanes {
        if pts.is_empty() || !names.iter().any(|n| n == name) {
            continue;
        }
        tr.auto.retain(|l| l.param != name);
        tr.auto.push(AutoLane { param: name.to_string(), points: pts });
    }
    perf.notes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feel(valence: f32, arousal: f32, vocal: f32) -> Feel {
        Feel { valence, arousal, tension: 0.3, vocal, breath: (0.65 - 0.45 * vocal).clamp(0.1, 0.8), space: 0.6 }
    }

    /// "\n alone in the cold" style syllables on a line of notes
    fn line(at: f64, sylls: &[(&str, bool, f64)]) -> Vec<Sung> {
        let mut t = at;
        let mut out = vec![];
        for (k, (txt, ws, len)) in sylls.iter().enumerate() {
            out.push(Sung { start: t, len: *len, key: 64 + (k % 3) as u8, vel: 0.7, text: txt.to_string(), held: txt.is_empty(), word_start: *ws, line_start: k == 0 });
            t += len;
        }
        out
    }

    fn lane<'a>(p: &'a Performance, k: &str) -> &'a [(f64, f32)] {
        &p.lanes.iter().find(|l| l.0 == k).unwrap().1
    }

    #[test]
    fn words_lean_where_they_are_stressed() {
        // "I am a-lone" : the function words light, LONE leaned on (a feeling word on its stressed syllable)
        let ns = line(0.0, &[("I", true, 0.5), ("am", true, 0.5), ("a", true, 0.5), ("lone", false, 1.5)]);
        let p = perform(&ns, &[Span { start: 0.0, end: 16.0, feel: feel(-0.4, 0.4, 0.4) }], 100.0, 1.0, 1);
        assert!(p.vels[3] > p.vels[2] && p.vels[3] > p.vels[1], "{:?}", p.vels);
        assert!(p.notes[0].contains("alone"), "{:?}", p.notes);
        // the leaned-on word scoops deeper than the light one
        let sc = lane(&p, "scoop");
        let at = |t: f64| sc.iter().find(|x| (x.0 - (t + 0.01)).abs() < 1e-6).unwrap().1;
        assert!(at(1.5) > at(1.0), "{:?}", sc);
    }

    #[test]
    fn a_sad_line_falls_off_and_a_happy_one_does_not() {
        let ns = line(0.0, &[("cold", true, 1.0), ("and", true, 0.5), ("gone", true, 2.5)]);
        let sad = perform(&ns, &[Span { start: 0.0, end: 16.0, feel: feel(-0.6, 0.3, 0.3) }], 90.0, 1.0, 2);
        let tune = lane(&sad, "tune");
        let last = tune.last().unwrap();
        assert!(last.1 < 0.5 - 40.0 / 400.0, "the end falls: {:?}", tune);
        let ns2 = line(0.0, &[("dance", true, 1.0), ("to", true, 0.5), ("night", false, 2.5)]);
        let happy = perform(&ns2, &[Span { start: 0.0, end: 16.0, feel: feel(0.6, 0.9, 0.9) }], 128.0, 1.0, 2);
        assert!(lane(&happy, "tune").iter().all(|x| (x.1 - 0.5).abs() < 0.06), "{:?}", lane(&happy, "tune"));
        assert!(happy.notes[0].contains("swell"), "{:?}", happy.notes);
    }

    #[test]
    fn lines_arch_and_amount_zero_is_flat() {
        let ns = line(0.0, &[("we", true, 0.5), ("are", true, 0.5), ("a", true, 0.5), ("live", false, 0.5), ("to", true, 0.5), ("night", false, 2.0)]);
        let span = [Span { start: 0.0, end: 32.0, feel: feel(0.3, 0.5, 0.5) }];
        let p = perform(&ns, &span, 120.0, 1.0, 3);
        let ex = lane(&p, "expr");
        let start = ex.first().unwrap().1;
        let top = ex.iter().map(|x| x.1).fold(0.0f32, f32::max);
        let end = ex.last().unwrap().1;
        assert!(top > start + 0.05 && top > end + 0.05, "{:?}", ex);
        // every lane sorted, in range
        for l in &p.lanes {
            assert!(l.1.windows(2).all(|w| w[0].0 < w[1].0) && l.1.iter().all(|x| (0.0..=1.0).contains(&x.1)), "{} {:?}", l.0, l.1);
        }
        let flat = perform(&ns, &span, 120.0, 0.0, 3);
        assert!(flat.vels.iter().zip(&ns).all(|(v, n)| (v - n.vel).abs() < 1e-6));
        assert!(lane(&flat, "tune").iter().all(|x| (x.1 - 0.5).abs() < 1e-6) && lane(&flat, "expr").iter().all(|x| (x.1 - 1.0).abs() < 1e-6));
        // the same seed, the same performance
        assert_eq!(perform(&ns, &span, 120.0, 1.0, 3).lanes, p.lanes);
    }

    #[test]
    fn held_syllables_follow_their_word() {
        // "cry-" held over two more notes: no new scoop on the held ones, and the fall rides the end of the last
        let mut ns = line(0.0, &[("cry", true, 1.0), ("", false, 1.0), ("", false, 1.0)]);
        ns[1].key = 62;
        let p = perform(&ns, &[Span { start: 0.0, end: 16.0, feel: feel(-0.7, 0.4, 0.5) }], 80.0, 1.0, 4);
        let sc = lane(&p, "scoop");
        assert!(sc.iter().all(|x| x.0 < 0.5), "{:?}", sc);
        let tune = lane(&p, "tune");
        assert!(tune.last().unwrap().0 > 2.9 && tune.last().unwrap().1 < 0.45, "{:?}", tune);
        assert_eq!(p.vels[1], ns[1].vel);
        assert!(p.notes[0].contains("cries"), "{:?}", p.notes);
    }
}
