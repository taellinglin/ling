//! Reading a whole song for its shape, from a MIDI file's notes ([`analyze_notes`]) or from a recording ([`analyze_audio`]):
//!
//! * **Structure**: bars are compared by what they hold (a 16-step grid of pitch classes per part, drums by drum key) and grouped into
//!   phrases of four bars; phrases that repeat share a letter (A, B ...), runs of a letter split where the energy steps or the drums come
//!   in or out are the sections. They are labelled ([`label_runs`]) by repetition, energy, drums, place and slope: intro, verse, pre
//!   (chorus), build, chorus, drop, break, bridge, outro. The repeated material that is loudest is the chorus; a short run that climbs
//!   into it is a pre-chorus (or a build when its drums roll / it climbs steeply, and then the loud part after it is a drop).
//! * **Energy and tension per bar**: note density, velocity, register span, how many parts play, harmonic tension (notes outside the
//!   key, semitone / tritone clashes, distance from the tonic triad) and drum intensity. For audio: loudness, onset density, brightness.
//! * **The emotional story**: per section a valence (the chords' quality - major or minor triads of the key per bar -, borrowed notes,
//!   cadences, register, tension) and an arousal (the energy). Every arc of the emotion engine (emotion.rs) is planned over the same
//!   sections and the closest one to the measured feel is the song's emotional arc; the energy curve against the story arcs' shapes picks
//!   the story arc (rise / fall / journey / dream / battle / hero), and [`story_text`] writes it as a STORY sentence that
//!   `songgen::story_arc` reads back as the same arc.
//! * **Hook**: the most repeated melodic phrase (one or two bars, transposition-invariant, rhythm on the eighth grid).
//! * **Groove**: swing (where the off-beat eighths sit), syncopation (held off-beat notes), half-time (snare on 3 instead of 2 and 4).
//! * **Audio extras**: section boundaries from a novelty curve (a checkerboard kernel over the bar self-similarity of chroma + log band
//!   energies, plus energy jumps), builds (rising brightness / noise, a snare roll) and drops (the low end comes back), a vocal / lead
//!   melody presence per section (the pitch tracker's voiced share), integrated loudness, brightness, stereo width, swing and a guess of
//!   the bass (sub / growl / reese) and lead (supersaw / pluck / screech) for the sound recipes.
use crate::compose::Kind;
use crate::songgen::Section;
use crate::theory::{Key, N};

/// one part of a MIDI file, as the analysis reads it
pub struct PartIn<'a> {
    pub name: &'a str,
    pub role: &'a str,
    pub kind: Kind,
    pub notes: &'a [N],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Groove {
    /// 0 straight .. 1 full triplet swing
    pub swing: f64,
    /// 0..1: share of the pitched notes that start off the beat and are held over it
    pub syncopation: f64,
    pub halftime: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Shape {
    pub bars: usize,
    /// per bar, 0..1 (the loudest bar is 1)
    pub energy: Vec<f64>,
    /// per bar, 0..1 harmonic tension
    pub tension: Vec<f64>,
    /// the sections for a song plan (tags songgen knows: a pre-chorus is a "build")
    pub sections: Vec<Section>,
    /// the label of each section as heard (intro, verse, pre, build, chorus, drop, break, bridge, outro)
    pub labels: Vec<String>,
    /// the repetition letter of each section (A, B, C ...)
    pub letters: Vec<char>,
    /// (valence -1..1, arousal 0..1) per section
    pub feels: Vec<(f32, f32)>,
    pub minor: Vec<bool>,
    /// the story arc (songgen: rise, fall, journey, dream, battle, hero)
    pub arc: String,
    /// the emotional arc (emotion::ARCS)
    pub emo_arc: String,
    /// a STORY sentence that names the arc
    pub story: String,
    /// the hook, from 0 (absolute keys), how often it is heard, and the part it is in
    pub hook: Vec<N>,
    pub hook_count: usize,
    pub hook_from: String,
    pub groove: Groove,
    /// (part name, role)
    pub roles: Vec<(String, String)>,
}

impl Shape {
    /// "intro 8, verse 16, chorus 8 ..." with the heard labels
    pub fn structure_text(&self) -> String {
        self.labels.iter().zip(&self.sections).map(|(l, s)| format!("{l} {}", s.bars)).collect::<Vec<_>>().join(", ")
    }

    /// one line about what else was found: hook, groove, parts
    pub fn details(&self) -> String {
        let mut v: Vec<String> = vec![];
        if self.hook_count >= 2 {
            v.push(format!("hook: {} notes heard {} times ({})", self.hook.len(), self.hook_count, self.hook_from));
        }
        let g = &self.groove;
        v.push(format!(
            "groove: {}{}{}",
            if g.swing > 0.6 { "swung" } else if g.swing > 0.25 { "lightly swung" } else { "straight" },
            if g.syncopation > 0.3 { ", syncopated" } else { "" },
            if g.halftime { ", half-time" } else { "" }
        ));
        if !self.roles.is_empty() {
            v.push(format!("parts: {}", self.roles.iter().take(8).map(|(n, r)| format!("{n} = {r}")).collect::<Vec<_>>().join(", ")));
        }
        v.join("; ")
    }
}

// ── small helpers ─────────────────────────────────────────────────────────────

fn norm_max(v: &[f64]) -> Vec<f64> {
    let m = v.iter().cloned().fold(0.0, f64::max);
    if m <= 1e-12 {
        return vec![0.0; v.len()];
    }
    v.iter().map(|x| (x / m).clamp(0.0, 1.0)).collect()
}

fn mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

fn is_drum_kind(k: Kind) -> bool {
    k == Kind::Drums
}

/// quality of the key's triad on a degree: true = minor or diminished
fn degree_minor(key: Key, d: usize) -> bool {
    let s = key.scale();
    let third = (s[(d + 2) % 7] + 12 - s[d]) % 12;
    third == 3
}

/// the story arcs' energy shapes (the same curves songgen pulls a plan towards)
fn arc_level(arc: &str, x: f64) -> f64 {
    use std::f64::consts::PI;
    match arc {
        "rise" => 0.25 + 0.75 * x,
        "fall" => 1.0 - 0.7 * x,
        "battle" => 0.75 + 0.25 * (PI * x).sin(),
        "dream" => 0.3 + 0.15 * (PI * x).sin(),
        "journey" => 0.45 + 0.4 * (2.0 * PI * x - 1.2).sin().abs(),
        _ => {
            if x < 0.25 {
                0.35 + x
            } else if x < 0.65 {
                0.6 - (x - 0.25) * 0.8
            } else {
                0.3 + (x - 0.65) * 2.0
            }
        }
    }
}

pub const STORY_ARCS: [&str; 6] = ["rise", "fall", "journey", "dream", "battle", "hero"];

/// the story arc an emotional arc tells (the inverse of feel::emotion_arc)
pub fn story_of_emotion(a: &str) -> &'static str {
    match a {
        "slow burn" | "release" => "rise",
        "melancholy" => "fall",
        "bittersweet" => "journey",
        "lullaby" => "dream",
        "storm" => "battle",
        "triumph" => "hero",
        _ => "",
    }
}

// ── structure: repetition, runs, labels ─────────────────────────────────────

/// a run of bars that belong together (one section), with what the labeller needs
#[derive(Clone, Debug, Default)]
pub struct Run {
    pub start: usize,
    pub bars: usize,
    pub letter: usize,
    /// mean energy 0..1 (relative to the loudest bar)
    pub e: f64,
    /// share of bars with drums
    pub drums: f64,
    /// energy of the second half minus the first
    pub slope: f64,
    /// 0..1: how much the run rolls into what follows (snare density / brightness rising)
    pub roll: f64,
    /// nothing plays (an acapella's rest: the band's part)
    pub silent: bool,
}

/// Label runs as sections are heard: intro, verse, pre, build, chorus, drop, break, bridge, outro.
pub fn label_runs(r: &[Run]) -> Vec<&'static str> {
    let n = r.len();
    if n == 0 {
        return vec![];
    }
    let emax = r.iter().filter(|x| !x.silent).map(|x| x.e).fold(1e-9, f64::max);
    let rel: Vec<f64> = r.iter().map(|x| (x.e / emax).clamp(0.0, 1.0)).collect();
    let letters = r.iter().map(|x| x.letter).max().unwrap_or(0) + 1;
    let mut count = vec![0usize; letters];
    let mut esum = vec![0.0f64; letters];
    for (i, x) in r.iter().enumerate() {
        if !x.silent {
            count[x.letter] += 1;
            esum[x.letter] += rel[i] + 0.05 * x.drums;
        }
    }
    // the chorus: the loudest material that repeats (and is loud enough to be one)
    let chorus_l = (0..letters).filter(|l| count[*l] >= 2).map(|l| (l, esum[l] / count[l] as f64)).filter(|(_, m)| *m >= 0.62).max_by(|a, b| a.1.total_cmp(&b.1)).map(|x| x.0);
    // the chorus material, or (when nothing loud repeats) the loudest runs
    let hot: Vec<bool> = (0..n).map(|i| !r[i].silent && (Some(r[i].letter) == chorus_l || rel[i] >= if chorus_l.is_some() { 0.98 } else { 0.9 })).collect();
    let first_hot = hot.iter().position(|h| *h);
    let mut lab: Vec<&'static str> = vec!["verse"; n];
    for i in 0..n {
        let x = &r[i];
        if x.silent {
            lab[i] = if i == 0 {
                "intro"
            } else if i + 1 == n {
                "outro"
            } else if lab[..i].iter().any(|l| matches!(*l, "chorus" | "drop")) && x.bars >= 4 {
                "drop"
            } else {
                "break"
            };
            continue;
        }
        // material heard as a verse before stays a verse (unless it is the chorus material)
        if Some(x.letter) != chorus_l && (0..i).any(|j| lab[j] == "verse" && r[j].letter == x.letter) {
            lab[i] = "verse";
            continue;
        }
        if hot[i] {
            lab[i] = "chorus";
            continue;
        }
        // an intro's material comes back at most as the outro (material that returns in the middle is a verse)
        let returns_mid = i + 1 < n && r[i + 1..n - 1].iter().any(|y| y.letter == x.letter && !y.silent);
        let drums_later = x.drums < 0.25 && r[i + 1..].iter().any(|y| y.drums > 0.6);
        let intro_like = rel[i] < 0.72 && (!returns_mid || rel[i] < 0.4 || drums_later);
        if i == 0 && n >= 3 && intro_like {
            lab[i] = "intro";
            continue;
        }
        if i + 1 == n && n >= 3 && rel[i] < 0.7 {
            lab[i] = "outro";
            continue;
        }
        let next_hot = i + 1 < n && hot[i + 1];
        if next_hot && i > 0 && x.bars <= 8 && r[i - 1].letter != x.letter {
            // a short run into the chorus: a build when its drums roll, else a pre-chorus (it climbs, or only ever leads into one)
            let always_before_hot = (0..n).filter(|j| r[*j].letter == x.letter && !r[*j].silent).all(|j| j + 1 < n && hot[j + 1]);
            // a pre-chorus comes between a verse and the chorus (right after an intro or a chorus it is the verse itself)
            let after_verse = lab[i - 1] == "verse";
            if x.roll > 0.3 {
                lab[i] = "build";
                continue;
            }
            if after_verse && (x.slope > 0.08 || always_before_hot) {
                lab[i] = "pre";
                continue;
            }
        }
        // material heard as a verse before stays a verse
        if (0..i).any(|j| lab[j] == "verse" && r[j].letter == x.letter) {
            lab[i] = "verse";
            continue;
        }
        if let Some(f) = first_hot {
            if i > f {
                if rel[i] < 0.5 && x.drums < 0.3 {
                    lab[i] = "break";
                    continue;
                }
                if count[x.letter] == 1 {
                    lab[i] = if rel[i] < 0.45 { "break" } else { "bridge" };
                    continue;
                }
                if rel[i] < 0.4 {
                    lab[i] = "break";
                    continue;
                }
            }
        }
        lab[i] = "verse";
    }
    // the loud part after a build is a drop, and so is every repeat of that material
    let mut drop_letters: Vec<usize> = vec![];
    for i in 1..n {
        if lab[i] == "chorus" && lab[i - 1] == "build" {
            lab[i] = "drop";
            drop_letters.push(r[i].letter);
        }
    }
    for i in 0..n {
        if lab[i] == "chorus" && drop_letters.contains(&r[i].letter) {
            lab[i] = "drop";
        }
    }
    lab
}

/// the song generator's tag for a heard label (a pre-chorus is planned as a build)
pub fn plan_tag(label: &str) -> &'static str {
    match label {
        "intro" => "intro",
        "pre" | "build" => "build",
        "chorus" => "chorus",
        "drop" => "drop",
        "break" => "break",
        "bridge" => "bridge",
        "outro" => "outro",
        _ => "verse",
    }
}

/// letters for blocks from a similarity function: a block joins the first earlier letter it is at least `th` alike, else starts one
fn letters_of(n: usize, sim: impl Fn(usize, usize) -> f64, th: f64) -> Vec<usize> {
    let mut reps: Vec<usize> = vec![];
    let mut out = vec![0usize; n];
    for i in 0..n {
        let best = reps.iter().enumerate().map(|(l, rep)| (l, sim(i, *rep))).max_by(|a, b| a.1.total_cmp(&b.1));
        match best {
            Some((l, s)) if s >= th => out[i] = l,
            _ => {
                out[i] = reps.len();
                reps.push(i);
            }
        }
    }
    out
}

/// blocks (start, len, letter) -> runs: a run continues while the letter stays and the energy / drums do not step
fn runs_of(blocks: &[(usize, usize, usize)], energy: &[f64], drums: &[f64], silent: &dyn Fn(usize, usize) -> bool) -> Vec<Run> {
    let mut out: Vec<Run> = vec![];
    let be = |s: usize, l: usize| mean(&energy[s..(s + l).min(energy.len())]);
    let bd = |s: usize, l: usize| mean(&drums[s..(s + l).min(drums.len())]);
    for &(s, l, letter) in blocks {
        let (e, d, sil) = (be(s, l), bd(s, l), silent(s, l));
        if let Some(last) = out.last_mut() {
            let le = be(last.start, last.bars);
            let ld = bd(last.start, last.bars);
            let drums_step = (ld < 0.25 && d > 0.6) || (ld > 0.6 && d < 0.25);
            if last.letter == letter && last.silent == sil && (le - e).abs() <= 0.2 && !drums_step && last.bars + l <= 16 {
                last.bars += l;
                continue;
            }
        }
        out.push(Run { start: s, bars: l, letter, e, drums: d, slope: 0.0, roll: 0.0, silent: sil });
    }
    for r in out.iter_mut() {
        let h = (r.bars / 2).max(1);
        let a = be(r.start, h);
        let b = be(r.start + r.bars - h, h);
        r.slope = b - a;
        r.e = be(r.start, r.bars);
        r.drums = bd(r.start, r.bars);
    }
    out
}

// ── the story ─────────────────────────────────────────────────────────────────

fn chorus_adj(v: f32, a: f32) -> &'static str {
    if v > 0.25 && a >= 0.7 {
        "euphoric"
    } else if v > 0.25 {
        "uplifting"
    } else if v < -0.2 && a >= 0.8 {
        "dark, heavy"
    } else if v < -0.2 {
        "bittersweet"
    } else if a >= 0.85 {
        "heavy"
    } else {
        "bright"
    }
}

fn verse_adj(v: f32, a: f32, t: f64) -> &'static str {
    match (v < -0.25, v > 0.25, a >= 0.6) {
        (true, _, true) => "dark, driving",
        (true, _, false) => "moody",
        (_, true, true) => "bouncing",
        (_, true, false) => "gentle",
        (_, _, true) => "driving",
        _ => {
            if t > 0.35 {
                "restless"
            } else {
                "steady"
            }
        }
    }
}

fn story_prefix(arc: &str) -> &'static str {
    match arc {
        "rise" => "Rising",
        "fall" => "Everything falls away",
        "battle" => "A battle",
        "dream" => "A dream",
        "journey" => "A journey",
        _ => "A hero's quest",
    }
}

/// The STORY sentence for an arc and the heard sections ("Rising: a quiet minor intro, tension builds through the pre-chorus, a
/// euphoric major chorus, a dark breakdown, a triumphant final chorus."). Written so `songgen::story_arc` reads the same arc back.
pub fn story_text(arc: &str, labels: &[String], feels: &[(f32, f32)], minor: &[bool], tension: &[f64]) -> String {
    let n = labels.len();
    let last_hot = (0..n).rev().find(|i| matches!(labels[*i].as_str(), "chorus" | "drop"));
    let hot_count = labels.iter().filter(|l| matches!(l.as_str(), "chorus" | "drop")).count();
    let mut phrases: Vec<String> = vec![];
    let mut seen: Vec<&str> = vec![];
    let mut prev: Option<(String, f32)> = None;
    for i in 0..n {
        let l = labels[i].as_str();
        let (v, a) = feels.get(i).copied().unwrap_or((0.0, 0.5));
        let mode = if minor.get(i).copied().unwrap_or(false) { "minor" } else { "major" };
        let t = tension.get(i).copied().unwrap_or(0.0);
        // a section like the one before it adds nothing to the story
        if let Some((pl, pa)) = &prev {
            if pl == l && (pa - a).abs() < 0.15 {
                continue;
            }
        }
        prev = Some((l.to_string(), a));
        let again = seen.contains(&l);
        let p = match l {
            "intro" => format!("a {} {mode} intro", if a < 0.35 { "quiet" } else if a < 0.6 { "steady" } else { "bold" }),
            "verse" => {
                if again {
                    let pa = (0..i).rev().find(|j| labels[*j] == "verse").map(|j| feels[j].1).unwrap_or(a);
                    format!("the verse again, {}", if a > pa + 0.1 { "fuller" } else if a < pa - 0.1 { "thinner" } else if v < -0.2 { "darker" } else { "brighter" })
                } else {
                    format!("a {} {mode} verse", verse_adj(v, a, t))
                }
            }
            "pre" => "tension builds through the pre-chorus".to_string(),
            "build" => format!("tension builds into the {}", if labels.get(i + 1).map(|x| x == "drop").unwrap_or(false) { "drop" } else { "chorus" }),
            "chorus" | "drop" => {
                if Some(i) == last_hot && hot_count >= 2 {
                    format!("a {} final {l}", if v > 0.2 { "triumphant" } else if v > -0.1 { "soaring" } else if a > 0.7 { "crushing" } else { "haunting" })
                } else if again {
                    format!("the {l} again")
                } else {
                    format!("a {} {mode} {l}", chorus_adj(v, a))
                }
            }
            "break" => format!("a {} breakdown", if v < -0.15 { "dark" } else if a < 0.3 { "hushed" } else { "tender" }),
            "bridge" => format!("a {} bridge", if v > 0.2 { "bright" } else if t > 0.45 { "tense" } else { "wistful" }),
            "outro" => format!("a {} outro", if a < 0.35 { "quiet" } else { "bold" }),
            _ => format!("a {l}"),
        };
        seen.push(l);
        phrases.push(p);
    }
    let body = match phrases.len() {
        0 => String::new(),
        1 => phrases[0].clone(),
        k => format!("{} and {}", phrases[..k - 1].join(", "), phrases[k - 1]),
    };
    let text = if body.is_empty() { format!("{}.", story_prefix(arc)) } else { format!("{}: {body}.", story_prefix(arc)) };
    if crate::songgen::story_arc(&text) == arc {
        text
    } else {
        format!("{}.", story_prefix(arc))
    }
}

/// feels per section -> (emotional arc, story arc): the emotion engine's plan of every arc over the sections, the closest wins; the
/// story arc from the energy curve against the arcs' shapes (leaning to the emotional arc's story)
pub fn choose_arcs(sections: &[Section], feels: &[(f32, f32)]) -> (String, String) {
    if sections.is_empty() {
        return ("release".into(), "rise".into());
    }
    let total: f64 = sections.iter().map(|s| s.bars as f64).sum::<f64>().max(1.0);
    let mut best = ("release", f64::MAX);
    for a in crate::emotion::ARCS {
        let p = crate::emotion::plan(sections, a, "pop", &[], 0.5, 7);
        let mut d = 0.0;
        for (i, f) in p.feels.iter().enumerate() {
            let (v, ar) = feels.get(i).copied().unwrap_or((0.0, 0.5));
            let w = sections[i].bars as f64 / total;
            d += w * (0.7 * ((f.valence - v) as f64).powi(2) + ((f.arousal - ar) as f64).powi(2));
        }
        if d < best.1 {
            best = (a, d);
        }
    }
    let emo = best.0.to_string();
    // the story arc: the arousal curve against each arc's shape, the valence trend and the emotional arc lean it
    let n = feels.len();
    let third = (n / 3).max(1);
    let v0 = feels[..third.min(n)].iter().map(|f| f.0 as f64).sum::<f64>() / third.min(n).max(1) as f64;
    let v1 = feels[n.saturating_sub(third)..].iter().map(|f| f.0 as f64).sum::<f64>() / third.min(n).max(1) as f64;
    let mut sbest = ("rise", f64::MAX);
    for sa in STORY_ARCS {
        let mut at = 0.0;
        let mut d = 0.0;
        for (i, s) in sections.iter().enumerate() {
            let x = (at + s.bars as f64 * 0.5) / total;
            at += s.bars as f64;
            let ar = feels.get(i).map(|f| f.1 as f64).unwrap_or(0.5);
            d += (s.bars as f64 / total) * (ar - arc_level(sa, x)).powi(2);
        }
        if sa == story_of_emotion(&emo) {
            d -= 0.008;
        }
        if sa == "rise" && v1 > v0 + 0.25 {
            d -= 0.01;
        }
        if sa == "fall" && v1 < v0 - 0.25 {
            d -= 0.01;
        }
        if d < sbest.1 {
            sbest = (sa, d);
        }
    }
    (emo, sbest.0.to_string())
}

// ── MIDI ──────────────────────────────────────────────────────────────────────

/// 16-step cells of a bar: (part, step, pitch class or drum key)
fn bar_cells(parts: &[PartIn], b: usize) -> Vec<u32> {
    let (a, z) = (b as f64 * 4.0, b as f64 * 4.0 + 4.0);
    let mut v: Vec<u32> = vec![];
    for (pi, p) in parts.iter().enumerate() {
        for n in p.notes.iter().filter(|n| n.start >= a - 1e-6 && n.start < z - 1e-6) {
            let step = ((n.start - a) * 4.0).round().clamp(0.0, 15.0) as u32;
            let k = if is_drum_kind(p.kind) { n.key as u32 } else { (n.key % 12) as u32 };
            v.push(((pi as u32) << 16) | (step << 8) | k);
        }
    }
    v.sort_unstable();
    v.dedup();
    v
}

fn cell_sim(a: &[u32], b: &[u32]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let (mut i, mut j, mut c) = (0, 0, 0usize);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Equal => {
                c += 1;
                i += 1;
                j += 1;
            }
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
        }
    }
    2.0 * c as f64 / (a.len() + b.len()) as f64
}

/// per bar: energy (0..1), tension (0..1), drum presence (0..1)
/// per bar: energy (0..1), tension (0..1), drum presence (0/1) and drum hits
pub fn note_curves(parts: &[PartIn], bars: usize, key: Key) -> (Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>) {
    let np = parts.iter().filter(|p| !p.notes.is_empty()).count().max(1) as f64;
    let mut dens = vec![0.0; bars];
    let mut drum = vec![0.0; bars];
    let mut hits = vec![0.0; bars];
    let mut vel = vec![0.0; bars];
    let mut veln = vec![0.0; bars];
    let mut span = vec![0.0; bars];
    let mut active = vec![0.0; bars];
    let mut tension = vec![0.0; bars];
    let mut reg = vec![0.0; bars];
    let tonic: [u8; 3] = {
        let s = key.scale();
        [s[0], s[2], s[4]]
    };
    for b in 0..bars {
        let (a, z) = (b as f64 * 4.0, b as f64 * 4.0 + 4.0);
        let (mut lo, mut hi) = (127u8, 0u8);
        let mut hist = [0.0f64; 12];
        let (mut rs, mut rn) = (0.0, 0.0);
        for p in parts {
            let mut any = false;
            for n in p.notes {
                let sounding = n.start < z - 1e-6 && n.start + n.len > a + 1e-6;
                let onset = n.start >= a - 1e-6 && n.start < z - 1e-6;
                if onset {
                    vel[b] += n.vel as f64;
                    veln[b] += 1.0;
                    if is_drum_kind(p.kind) {
                        drum[b] += n.vel as f64;
                        hits[b] += 1.0;
                    } else {
                        dens[b] += 1.0;
                    }
                    if p.kind == Kind::Melody {
                        rs += n.key as f64;
                        rn += 1.0;
                    }
                }
                if sounding {
                    any = true;
                    if !is_drum_kind(p.kind) {
                        lo = lo.min(n.key);
                        hi = hi.max(n.key);
                        hist[(n.key % 12) as usize] += (n.start + n.len).min(z) - n.start.max(a);
                    }
                }
            }
            if any {
                active[b] += 1.0;
            }
        }
        if hi >= lo {
            span[b] = (hi - lo) as f64;
        }
        if rn > 0.0 {
            // a melody sung / played higher is more intense
            reg[b] = ((rs / rn - 55.0) / 30.0).clamp(0.0, 1.0);
        }
        if veln[b] > 0.0 {
            vel[b] /= veln[b];
        }
        let tot: f64 = hist.iter().sum();
        if tot > 0.0 {
            let out: f64 = (0..12).filter(|pc| !key.accepts(*pc as u8)).map(|pc| hist[pc]).sum::<f64>() / tot;
            let home: f64 = tonic.iter().map(|pc| hist[*pc as usize]).sum::<f64>() / tot;
            // clashes: semitones and tritones between the pitch classes sounding in each eighth
            let mut rough = 0.0;
            let mut cnt = 0.0;
            for e in 0..8 {
                let t = a + e as f64 * 0.5 + 0.25;
                let mut pcs: Vec<u8> = parts.iter().filter(|p| !is_drum_kind(p.kind)).flat_map(|p| p.notes.iter().filter(move |n| n.start <= t && n.start + n.len > t).map(|n| n.key % 12)).collect();
                pcs.sort_unstable();
                pcs.dedup();
                if pcs.len() >= 2 {
                    let mut c = 0.0;
                    let mut pairs = 0.0;
                    for i in 0..pcs.len() {
                        for j in i + 1..pcs.len() {
                            let d = (pcs[j] - pcs[i]).min(12 - (pcs[j] - pcs[i]));
                            pairs += 1.0;
                            if d == 1 || d == 6 {
                                c += 1.0;
                            }
                        }
                    }
                    rough += c / pairs;
                    cnt += 1.0;
                }
            }
            let rough = if cnt > 0.0 { rough / cnt } else { 0.0 };
            tension[b] = (0.45 * out * 2.0 + 0.35 * rough * 2.0 + 0.2 * (1.0 - home)).clamp(0.0, 1.0);
        }
    }
    let dn = norm_max(&dens);
    let drn = norm_max(&drum);
    // a lone sung line (an acapella): its intensity is how loud and how high it is sung more than how many notes it has
    let lone = parts.iter().filter(|p| !p.notes.is_empty()).count() == 1 && parts.iter().any(|p| !p.notes.is_empty() && p.kind == Kind::Melody);
    let raw: Vec<f64> = if lone {
        (0..bars).map(|b| if veln[b] > 0.0 { 0.5 * vel[b] + 0.35 * reg[b] + 0.15 * dn[b] } else { 0.0 }).collect()
    } else {
        (0..bars).map(|b| 0.28 * dn[b] + 0.17 * vel[b] + 0.12 * (span[b] / 36.0).min(1.0) + 0.2 * active[b] / np + 0.18 * drn[b] + 0.05 * tension[b] + 0.1 * reg[b]).collect()
    };
    let energy = norm_max(&raw);
    let drums: Vec<f64> = drum.iter().map(|d| if *d > 0.0 { 1.0 } else { 0.0 }).collect();
    (energy, tension, drums, hits)
}

/// valence per section from the chords' quality (the key's triad that fits each bar best), borrowed notes, a closing cadence, the register
/// and the tension
fn note_feel(parts: &[PartIn], key: Key, start: usize, bars: usize, energy_rel: f64, tension: f64) -> (f32, f32, bool) {
    let mut maj = 0.0;
    let mut tot = 0.0;
    let mut borrowed = 0.0;
    let mut dur = 0.0;
    let mut degs: Vec<usize> = vec![];
    let mut pitch_sum = 0.0;
    let mut pitch_n = 0.0;
    for b in start..start + bars {
        let (a, z) = (b as f64 * 4.0, b as f64 * 4.0 + 4.0);
        let mut h = [0.0f64; 12];
        for p in parts.iter().filter(|p| !is_drum_kind(p.kind)) {
            let w = if p.kind == Kind::Bass { 2.0 } else { 1.0 };
            for n in p.notes.iter().filter(|n| n.start < z && n.start + n.len > a) {
                let d = (n.start + n.len).min(z) - n.start.max(a);
                h[(n.key % 12) as usize] += d * w;
                dur += d;
                if !key.accepts(n.key % 12) {
                    borrowed += d;
                }
                if p.kind == Kind::Melody || p.kind == Kind::Chords || p.kind == Kind::Pad {
                    pitch_sum += n.key as f64 * d;
                    pitch_n += d;
                }
            }
        }
        if h.iter().sum::<f64>() <= 0.0 {
            continue;
        }
        let d = crate::songgen::best_triad(&h, key);
        degs.push(d);
        let w = if b == start || b + 1 == start + bars { 1.5 } else { 1.0 };
        if !degree_minor(key, d) {
            maj += w;
        }
        tot += w;
    }
    let major_share = if tot > 0.0 { maj / tot } else if key.minor { 0.3 } else { 0.7 };
    let borrowed = if dur > 0.0 { borrowed / dur } else { 0.0 };
    let cadence = match degs.as_slice() {
        [.., 4, 0] => 0.1,
        [.., 4] => -0.05,
        _ => 0.0,
    };
    let reg = if pitch_n > 0.0 { ((pitch_sum / pitch_n - 64.0) / 24.0).clamp(-0.5, 0.5) } else { 0.0 };
    let v = ((major_share * 2.0 - 1.0) * 0.5 + 0.12 * reg - 0.25 * tension - 0.3 * borrowed + cadence + 0.12 * (energy_rel - 0.5)).clamp(-1.0, 1.0);
    let ar = (0.1 + 0.85 * energy_rel).clamp(0.0, 1.0);
    (v as f32, ar as f32, major_share < 0.5)
}

/// the most repeated melodic phrase of the melody part (1 or 2 bars, the same intervals and rhythm, any transposition)
fn find_hook(parts: &[PartIn], bars: usize) -> (Vec<N>, usize, String) {
    // the melody: a melodic part, else the highest mostly-monophonic one
    let mono = |p: &PartIn| {
        let mut starts: Vec<i64> = p.notes.iter().map(|n| (n.start * 8.0).round() as i64).collect();
        let total = starts.len();
        starts.dedup();
        total > 0 && (total as f64) < 1.3 * starts.len() as f64
    };
    let pick = parts
        .iter()
        .filter(|p| !is_drum_kind(p.kind) && p.notes.len() >= 6)
        .filter(|p| p.kind == Kind::Melody || mono(p))
        .max_by(|a, b| {
            let m = |p: &PartIn| (if p.kind == Kind::Melody { 100.0 } else { 0.0 }) + p.notes.iter().map(|n| n.key as f64).sum::<f64>() / p.notes.len() as f64;
            m(a).total_cmp(&m(b))
        });
    let Some(p) = pick else { return (vec![], 0, String::new()) };
    let mut best: Option<(f64, usize, usize, f64)> = None; // score, count, first bar, len
    for len in [8.0f64, 4.0] {
        let mut map: std::collections::HashMap<Vec<(i64, i32)>, (usize, usize)> = std::collections::HashMap::new();
        for b in 0..bars {
            let a = b as f64 * 4.0;
            let ns: Vec<&N> = p.notes.iter().filter(|n| n.start >= a - 1e-6 && n.start < a + len - 1e-6).collect();
            if ns.len() < 3 {
                continue;
            }
            let k0 = ns[0].key as i32;
            let sig: Vec<(i64, i32)> = ns.iter().map(|n| (((n.start - a) * 2.0).round() as i64, n.key as i32 - k0)).collect();
            let e = map.entry(sig).or_insert((0, b));
            e.0 += 1;
        }
        for (sig, (c, b0)) in map {
            if c < 2 {
                continue;
            }
            let score = c as f64 * (sig.len() as f64).sqrt() * (len / 4.0).sqrt();
            if best.map(|x| score > x.0).unwrap_or(true) {
                best = Some((score, c, b0, len));
            }
        }
    }
    let Some((_, c, b0, len)) = best else { return (vec![], 0, String::new()) };
    let a = b0 as f64 * 4.0;
    let hook: Vec<N> = p.notes.iter().filter(|n| n.start >= a - 1e-6 && n.start < a + len - 1e-6).map(|n| N { start: n.start - a, len: n.len.min(a + len - n.start), key: n.key, vel: n.vel }).collect();
    (hook, c, p.name.to_string())
}

/// swing, syncopation and half-time from the notes
fn find_groove(parts: &[PartIn]) -> Groove {
    let mut offs: Vec<f64> = vec![];
    let mut sync = 0.0;
    let mut all = 0.0;
    let (mut back, mut three) = (0.0, 0.0);
    for p in parts {
        for n in p.notes {
            let f = n.start.rem_euclid(1.0);
            if (0.4..0.78).contains(&f) {
                offs.push(f);
            }
            if is_drum_kind(p.kind) {
                if matches!(n.key, 37..=40) {
                    let pos = n.start.rem_euclid(4.0);
                    if (pos - 1.0).abs() < 0.1 || (pos - 3.0).abs() < 0.1 {
                        back += 1.0;
                    } else if (pos - 2.0).abs() < 0.1 {
                        three += 1.0;
                    }
                }
            } else {
                all += 1.0;
                if f > 0.1 && f < 0.9 && n.start + n.len > n.start.ceil() + 0.05 {
                    sync += 1.0;
                }
            }
        }
    }
    let swing = if offs.len() >= 8 { ((mean(&offs) - 0.5) / (2.0 / 3.0 - 0.5)).clamp(0.0, 1.0) } else { 0.0 };
    Groove { swing, syncopation: if all > 0.0 { sync / all } else { 0.0 }, halftime: three >= 4.0 && three > 1.5 * back }
}

/// sections (with labels, letters, feels) from per-bar energy / drums and a bar similarity
fn sections_from(bars: usize, energy: &[f64], drums: &[f64], hits: &[f64], sim: &dyn Fn(usize, usize) -> f64, silent: &dyn Fn(usize, usize) -> bool) -> (Vec<Run>, Vec<&'static str>) {
    let p = if bars >= 16 { 4 } else { 2 };
    let nph = bars.div_ceil(p);
    let phrase_sim = |i: usize, j: usize| {
        let mut s = 0.0;
        let mut c = 0.0;
        for k in 0..p {
            let (a, b) = (i * p + k, j * p + k);
            if a < bars && b < bars {
                s += sim(a, b);
                c += 1.0;
            }
        }
        if c > 0.0 { s / c } else { 0.0 }
    };
    let letters = letters_of(nph, phrase_sim, 0.5);
    let blocks: Vec<(usize, usize, usize)> = (0..nph).map(|i| (i * p, p.min(bars - i * p), letters[i])).collect();
    let mut runs = runs_of(&blocks, energy, drums, silent);
    // the roll into the next run: drums getting denser / louder in its last bars
    for i in 0..runs.len() {
        let r = &runs[i];
        if r.bars >= 2 {
            // a drum roll: the hits per bar grow (1/4 > 1/8 > 1/16) towards its end
            let last = hits[r.start + r.bars - 1];
            let first = hits[r.start].max(1.0);
            runs[i].roll = if last >= 6.0 && last >= 1.8 * first { (last / first / 3.0).clamp(0.4, 1.0) } else { 0.0 };
        }
    }
    let labels = label_runs(&runs);
    (runs, labels)
}

/// fill in a shape's sections, feels, arcs and story from runs, labels and a per-section feel function
fn finish(shape: &mut Shape, runs: &[Run], labels: &[&'static str], feel: &dyn Fn(&Run) -> (f32, f32, bool)) {
    let emax = runs.iter().map(|r| r.e).fold(1e-9, f64::max);
    shape.sections = runs.iter().zip(labels).map(|(r, l)| Section { tag: plan_tag(l).into(), bars: r.bars as u32, energy: (0.25 + 0.75 * r.e / emax).min(1.0) }).collect();
    shape.labels = labels.iter().map(|l| l.to_string()).collect();
    shape.letters = runs.iter().map(|r| (b'A' + (r.letter.min(25)) as u8) as char).collect();
    let fm: Vec<(f32, f32, bool)> = runs.iter().map(feel).collect();
    shape.feels = fm.iter().map(|f| (f.0, f.1)).collect();
    shape.minor = fm.iter().map(|f| f.2).collect();
    let (emo, arc) = choose_arcs(&shape.sections, &shape.feels);
    shape.emo_arc = emo;
    shape.arc = arc;
    let ten: Vec<f64> = runs.iter().map(|r| mean(&shape.tension[r.start.min(shape.tension.len())..(r.start + r.bars).min(shape.tension.len())])).collect();
    shape.story = story_text(&shape.arc, &shape.labels, &shape.feels, &shape.minor, &ten);
}

/// Read a MIDI file's parts (notes in beats from the start, 4/4) for its shape.
pub fn analyze_notes(parts: &[PartIn], bars: usize, key: Key) -> Shape {
    let mut shape = Shape { bars, roles: parts.iter().map(|p| (p.name.to_string(), p.role.to_string())).collect(), ..Shape::default() };
    if bars == 0 {
        return shape;
    }
    let (energy, tension, drums, hits) = note_curves(parts, bars, key);
    shape.energy = energy.clone();
    shape.tension = tension.clone();
    shape.groove = find_groove(parts);
    let (hook, hc, hf) = find_hook(parts, bars);
    shape.hook = hook;
    shape.hook_count = hc;
    shape.hook_from = hf;
    if bars < 8 {
        return shape;
    }
    let cells: Vec<Vec<u32>> = (0..bars).map(|b| bar_cells(parts, b)).collect();
    let sim = |a: usize, b: usize| cell_sim(&cells[a], &cells[b]);
    let silent = |s: usize, l: usize| cells[s..(s + l).min(bars)].iter().all(|c| c.is_empty());
    let (runs, labels) = sections_from(bars, &energy, &drums, &hits, &sim, &silent);
    let emax = runs.iter().map(|r| r.e).fold(1e-9, f64::max);
    let feel = |r: &Run| note_feel(parts, key, r.start, r.bars, r.e / emax, mean(&tension[r.start..r.start + r.bars]));
    finish(&mut shape, &runs, &labels, &feel);
    shape
}

// ── audio ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct AudioInfo {
    pub shape: Shape,
    pub bpm: f64,
    /// seconds of the first downbeat
    pub downbeat_s: f64,
    /// integrated loudness, LUFS
    pub lufs: f32,
    /// spectral centroid as a share of 5.5 kHz (0..1), and the mix's brightness target -1..1
    pub bright: f64,
    pub bright_target: f64,
    /// side / (mid + side) energy, and the mix's width target 0..1
    pub width: f64,
    pub width_target: f64,
    pub swing: f64,
    /// melody / vocal presence per section, 0..1 (the pitch tracker's voiced share of a few seconds in the middle)
    pub vocal: Vec<f64>,
    /// section starts in seconds
    pub boundaries_s: Vec<f64>,
    /// "sub", "growl", "reese" ("" = unsure) and "supersaw", "pluck", "screech", "soft" ("" = unsure)
    pub bass: String,
    pub lead: String,
}

impl AudioInfo {
    /// words for the description (sound recipes and roles read them): "growl bass, supersaw lead"; "" when unsure
    pub fn palette_words(&self) -> String {
        let mut v = vec![];
        if !self.bass.is_empty() {
            v.push(format!("{} bass", self.bass));
        }
        if !self.lead.is_empty() && self.lead != "soft" {
            v.push(format!("{} lead", self.lead));
        }
        v.join(", ")
    }

    pub fn details(&self) -> String {
        let voc: Vec<String> = self.shape.labels.iter().zip(&self.vocal).filter(|(_, v)| **v >= 0.35).map(|(l, _)| l.clone()).collect();
        let mut s = format!(
            "{:.1} LUFS, {} and {}, {}",
            self.lufs,
            if self.bright > 0.22 { "bright" } else if self.bright > 0.12 { "balanced" } else { "dark" },
            if self.width > 0.25 { "wide" } else if self.width > 0.08 { "stereo" } else { "narrow" },
            if self.swing > 0.5 { "swung" } else { "straight" }
        );
        if !voc.is_empty() {
            s.push_str(&format!("; a voice / lead melody in: {}", voc.join(", ")));
        }
        let pw = self.palette_words();
        if !pw.is_empty() {
            s.push_str(&format!("; sounds like: {pw}"));
        }
        s
    }
}

/// in-place radix-2 FFT
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -std::f32::consts::TAU / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        let mut i = 0;
        while i < n {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (i + k, i + k + len / 2);
                let (tr, ti) = (re[b] * cr - im[b] * ci, re[b] * ci + im[b] * cr);
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let nr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = nr;
            }
            i += len;
        }
        len <<= 1;
    }
}

/// per analysis frame (about 46 ms)
#[derive(Clone, Default)]
struct Frame {
    bands: [f32; 12],
    chroma: [f32; 12],
    power: f32,
    centroid: f32,
    flat: f32,
    low: f32,
    lowmid: f32,
    flux: f32,
    flux_lo: f32,
    flux_hi: f32,
}

/// Read a recording (interleaved stereo) for its shape: the bar grid from `bpm` (the reference's tempo) with the downbeat found from the
/// onsets, sections from novelty, builds / drops, the energy curve, the story, loudness / brightness / width / swing, vocal presence
/// and the bass / lead palette. At most the first 4 minutes are read.
pub fn analyze_audio(stereo: &[f32], sr: u32, bpm: f64, key: Option<Key>) -> AudioInfo {
    let mut info = AudioInfo { bpm, ..AudioInfo::default() };
    let max = (240.0 * sr as f64) as usize * 2;
    let st = &stereo[..stereo.len().min(max)];
    if st.len() < sr as usize * 8 || bpm < 30.0 {
        return info;
    }
    let mono: Vec<f32> = st.chunks_exact(2).map(|f| (f[0] + f[1]) * 0.5).collect();
    info.lufs = crate::mixai::lufs(st, sr as f32);
    let (mut mid, mut side) = (0.0f64, 0.0f64);
    for f in st.chunks_exact(2) {
        mid += ((f[0] + f[1]) as f64).powi(2);
        side += ((f[0] - f[1]) as f64).powi(2);
    }
    info.width = side / (mid + side).max(1e-12);
    info.width_target = (0.3 + 1.6 * info.width).clamp(0.2, 0.9);
    // frames on an ~11 kHz copy
    let dec = ((sr as f64 / 11025.0).round() as usize).max(1);
    let sr2 = sr as f64 / dec as f64;
    let x: Vec<f32> = mono.chunks(dec).map(|c| c.iter().sum::<f32>() / c.len() as f32).collect();
    const NF: usize = 1024;
    const HOP: usize = 512;
    if x.len() < NF * 4 {
        return info;
    }
    let win: Vec<f32> = (0..NF).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / NF as f32).cos()).collect();
    let binf = |k: usize| k as f64 * sr2 / NF as f64;
    let top = (sr2 * 0.5).min(5500.0);
    let edges: Vec<f64> = (0..=12).map(|i| 40.0 * (top / 40.0).powf(i as f64 / 12.0)).collect();
    let band_of: Vec<i32> = (0..NF / 2).map(|k| {
        let f = binf(k);
        edges.windows(2).position(|w| f >= w[0] && f < w[1]).map(|i| i as i32).unwrap_or(-1)
    }).collect();
    let pc_of: Vec<i32> = (0..NF / 2).map(|k| {
        let f = binf(k);
        if (80.0..2000.0).contains(&f) { ((12.0 * (f / 440.0).log2() + 69.0).round() as i32).rem_euclid(12) } else { -1 }
    }).collect();
    let nfr = (x.len() - NF) / HOP + 1;
    let mut frames: Vec<Frame> = Vec::with_capacity(nfr);
    let mut prev = vec![0.0f32; NF / 2];
    let (mut re, mut im) = (vec![0.0f32; NF], vec![0.0f32; NF]);
    for fi in 0..nfr {
        let s = fi * HOP;
        for i in 0..NF {
            re[i] = x[s + i] * win[i];
            im[i] = 0.0;
        }
        fft(&mut re, &mut im);
        let mut f = Frame::default();
        let (mut cnum, mut cden) = (0.0f64, 0.0f64);
        let (mut glog, mut gar, mut gn) = (0.0f64, 0.0f64, 0.0f64);
        for k in 1..NF / 2 {
            let m = (re[k] * re[k] + im[k] * im[k]).sqrt();
            let p = m * m;
            let fq = binf(k);
            f.power += p;
            cnum += fq * m as f64;
            cden += m as f64;
            if band_of[k] >= 0 {
                f.bands[band_of[k] as usize] += p;
            }
            if pc_of[k] >= 0 {
                f.chroma[pc_of[k] as usize] += m;
            }
            if fq < 120.0 {
                f.low += p;
            } else if fq < 400.0 {
                f.lowmid += p;
            }
            let d = (m - prev[k]).max(0.0);
            f.flux += d;
            if fq < 150.0 {
                f.flux_lo += d;
            } else if (1500.0..5000.0).contains(&fq) {
                f.flux_hi += d;
                glog += (p as f64 + 1e-12).ln();
                gar += p as f64 + 1e-12;
                gn += 1.0;
            }
            prev[k] = m;
        }
        f.centroid = if cden > 0.0 { (cnum / cden / 5500.0) as f32 } else { 0.0 };
        f.flat = if gn > 0.0 { ((glog / gn).exp() / (gar / gn)) as f32 } else { 0.0 };
        let cs: f32 = f.chroma.iter().sum();
        if cs > 0.0 {
            for c in f.chroma.iter_mut() {
                *c /= cs;
            }
        }
        frames.push(f);
    }
    // the beat grid: the phase that puts the most onset strength on the beats, then the beat of the bar where the kicks / changes are
    let fps = sr2 / HOP as f64;
    let t = 60.0 / bpm * fps; // frames per beat
    let env: Vec<f64> = norm_max(&frames.iter().map(|f| f.flux as f64).collect::<Vec<_>>());
    let lo: Vec<f64> = norm_max(&frames.iter().map(|f| f.flux_lo as f64).collect::<Vec<_>>());
    // harmonic change: how far the chroma moves from the frame two before (chords change on bar lines)
    let chg: Vec<f64> = norm_max(&(0..frames.len()).map(|i| if i < 2 { 0.0 } else { (0..12).map(|k| (frames[i].chroma[k] - frames[i - 2].chroma[k]).abs() as f64).sum::<f64>() * (frames[i].power as f64).sqrt().min(1e9) }).collect::<Vec<_>>());
    let at = |v: &[f64], pos: f64| -> f64 {
        let i = pos.floor();
        if i < 0.0 || i as usize + 1 >= v.len() {
            return 0.0;
        }
        let fr = pos - i;
        v[i as usize] * (1.0 - fr) + v[i as usize + 1] * fr
    };
    let mut best_phase = (0.0, f64::MIN);
    let mut ph = 0.0;
    while ph < t {
        let mut s = 0.0;
        let mut k = ph;
        while k < env.len() as f64 {
            s += at(&env, k).max(at(&env, k - 1.0)).max(at(&env, k + 1.0));
            k += t;
        }
        if s > best_phase.1 {
            best_phase = (ph, s);
        }
        ph += 0.25;
    }
    let mut best_bar = (0usize, f64::MIN);
    for m in 0..4 {
        let mut s = 0.0;
        let mut k = best_phase.0 + m as f64 * t;
        while k < lo.len() as f64 {
            s += at(&lo, k).max(at(&lo, k + 1.0)) + 1.5 * at(&chg, k).max(at(&chg, k + 1.0)).max(at(&chg, k + 2.0));
            k += 4.0 * t;
        }
        if s > best_bar.1 {
            best_bar = (m, s);
        }
    }
    let f0 = best_phase.0 + best_bar.0 as f64 * t;
    info.downbeat_s = f0 / fps;
    let barf = 4.0 * t;
    let nb = ((frames.len() as f64 - f0) / barf).floor().max(0.0) as usize;
    if nb < 8 {
        return info;
    }
    let range = |b: usize| -> (usize, usize) { ((f0 + b as f64 * barf).round() as usize, ((f0 + (b + 1) as f64 * barf).round() as usize).min(frames.len())) };
    // per bar features
    let mut loud = vec![0.0; nb];
    let mut cen = vec![0.0; nb];
    let mut flat = vec![0.0; nb];
    let mut hi_on = vec![0.0; nb];
    let mut low_e = vec![0.0; nb];
    let mut kick = vec![0.0; nb];
    let mut vecs: Vec<Vec<f64>> = vec![];
    let hi_env = norm_max(&frames.iter().map(|f| f.flux_hi as f64).collect::<Vec<_>>());
    for b in 0..nb {
        let (a, z) = range(b);
        let fr = &frames[a..z.max(a + 1).min(frames.len())];
        let n = fr.len().max(1) as f64;
        let p: f64 = fr.iter().map(|f| f.power as f64).sum::<f64>() / n;
        loud[b] = 10.0 * (p + 1e-12).log10();
        cen[b] = fr.iter().map(|f| f.centroid as f64).sum::<f64>() / n;
        flat[b] = fr.iter().map(|f| f.flat as f64).sum::<f64>() / n;
        low_e[b] = fr.iter().map(|f| (f.low + f.lowmid) as f64).sum::<f64>() / n;
        // onsets in the snare band: local peaks over a floor
        let mut c = 0.0;
        for i in a.max(1)..z.min(hi_env.len() - 1) {
            if hi_env[i] > 0.12 && hi_env[i] >= hi_env[i - 1] && hi_env[i] > hi_env[i + 1] {
                c += 1.0;
            }
        }
        hi_on[b] = c;
        // kicks: low-band onset strength near the beats
        let mut k = 0.0;
        for q in 0..4 {
            let pos = f0 + b as f64 * barf + q as f64 * t;
            k += at(&lo, pos).max(at(&lo, pos + 1.0)).max(at(&lo, pos - 1.0));
        }
        kick[b] = k / 4.0;
        let mut v: Vec<f64> = vec![];
        let mut ch = [0.0f64; 12];
        let mut bd = [0.0f64; 12];
        for f in fr {
            for i in 0..12 {
                ch[i] += f.chroma[i] as f64;
                bd[i] += f.bands[i] as f64;
            }
        }
        let cl = ch.iter().map(|x| x * x).sum::<f64>().sqrt().max(1e-9);
        v.extend(ch.iter().map(|x| x / cl));
        v.extend(bd.iter().map(|x| (x / n + 1e-12).log10()));
        vecs.push(v);
    }
    let lmax = loud.iter().cloned().fold(f64::MIN, f64::max);
    let loud_n: Vec<f64> = loud.iter().map(|l| ((l - (lmax - 24.0)) / 24.0).clamp(0.0, 1.0)).collect();
    let on_n = norm_max(&hi_on);
    let cen_n = norm_max(&cen);
    let energy_raw: Vec<f64> = (0..nb).map(|b| 0.65 * loud_n[b] + 0.2 * on_n[b] + 0.15 * cen_n[b]).collect();
    let energy = norm_max(&energy_raw);
    let kmax = kick.iter().cloned().fold(1e-9, f64::max);
    let drums: Vec<f64> = kick.iter().map(|k| if k / kmax > 0.3 { 1.0 } else { 0.0 }).collect();
    // standardize the features, self-similarity of bars
    let dim = vecs[0].len();
    // the chroma as heard (for the chords), then every dimension standardized across the bars so what all bars share (the key, the
    // mix) does not make them alike; chroma and bands weigh the same, the energy a little less
    let chroma_bars: Vec<Vec<f64>> = vecs.iter().map(|v| v[..12].to_vec()).collect();
    for (b, v) in vecs.iter_mut().enumerate() {
        v.push(energy[b]);
    }
    let dim = dim + 1;
    for d in 0..dim {
        let m = vecs.iter().map(|v| v[d]).sum::<f64>() / nb as f64;
        let sd = (vecs.iter().map(|v| (v[d] - m).powi(2)).sum::<f64>() / nb as f64).sqrt().max(1e-6);
        let w = if d < 24 { 1.0 / 12f64.sqrt() } else { 0.8 };
        for v in vecs.iter_mut() {
            v[d] = w * (v[d] - m) / sd;
        }
    }
    let cos = |a: &[f64], b: &[f64]| {
        let (mut d, mut na, mut nbb) = (0.0, 0.0, 0.0);
        for i in 0..a.len() {
            d += a[i] * b[i];
            na += a[i] * a[i];
            nbb += b[i] * b[i];
        }
        if na <= 0.0 || nbb <= 0.0 { 0.5 } else { 0.5 + 0.5 * d / (na * nbb).sqrt() }
    };
    let ssm: Vec<Vec<f64>> = (0..nb).map(|i| (0..nb).map(|j| cos(&vecs[i], &vecs[j])).collect()).collect();
    // novelty: a checkerboard kernel along the diagonal + energy jumps
    let l = if nb >= 16 { 4 } else { 2 };
    let mut nov = vec![0.0; nb];
    for (b, nv) in nov.iter_mut().enumerate().take(nb.saturating_sub(1)).skip(1) {
        let k = l.min(b).min(nb - b);
        let mut s = 0.0;
        for i in 0..2 * k {
            for j in 0..2 * k {
                let (ii, jj) = (b - k + i, b - k + j);
                let sign = if (i < k) == (j < k) { 1.0 } else { -1.0 };
                let g = (-(((i as f64 - k as f64 + 0.5).powi(2) + (j as f64 - k as f64 + 0.5).powi(2)) / (2.0 * (k as f64 * 0.75).powi(2)))).exp();
                s += sign * g * ssm[ii][jj];
            }
        }
        let ea = mean(&energy[b.saturating_sub(2)..b]);
        let eb = mean(&energy[b..(b + 2).min(nb)]);
        *nv = s.max(0.0) / (k * k) as f64 + 0.8 * (eb - ea).abs();
    }
    let nmax = nov.iter().cloned().fold(1e-9, f64::max);
    let nm = mean(&nov);
    let nsd = (nov.iter().map(|v| (v - nm).powi(2)).sum::<f64>() / nb as f64).sqrt();
    let mut cands: Vec<(usize, f64)> = (2..nb.saturating_sub(1))
        .filter(|b| nov[*b] >= nov[b - 1] && nov[*b] >= nov[(b + 1).min(nb - 1)] && nov[*b] > nm + 0.35 * nsd)
        .map(|b| (b, nov[b] + if b % 4 == 0 { 0.15 * nmax } else { 0.0 }))
        .collect();
    cands.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut cuts: Vec<usize> = vec![];
    for (b, _) in cands {
        if cuts.iter().all(|c| (*c as i64 - b as i64).abs() >= 4) && b >= 4 && nb - b >= 2 {
            cuts.push(b);
        }
    }
    cuts.sort_unstable();
    let mut segs: Vec<(usize, usize)> = vec![];
    let mut s0 = 0;
    for c in cuts.iter().chain(std::iter::once(&nb)) {
        if *c > s0 {
            segs.push((s0, c - s0));
            s0 = *c;
        }
    }
    let seg_sim = |i: usize, j: usize| {
        let (a, b) = (segs[i], segs[j]);
        let n = a.1.min(b.1);
        (0..n).map(|k| ssm[a.0 + k][b.0 + k]).sum::<f64>() / n.max(1) as f64
    };
    let letters = letters_of(segs.len(), seg_sim, 0.85);
    let mut runs: Vec<Run> = vec![];
    for (i, (s, l)) in segs.iter().enumerate() {
        let r = Run { start: *s, bars: *l, letter: letters[i], e: mean(&energy[*s..s + l]), drums: mean(&drums[*s..s + l]), slope: 0.0, roll: 0.0, silent: false };
        // the same material at the same level as the run before it is one section (a boundary from a passing change)
        if let Some(last) = runs.last_mut() {
            if last.letter == r.letter && (last.e - r.e).abs() < 0.12 && last.bars + r.bars <= 16 {
                last.bars += r.bars;
                last.e = mean(&energy[last.start..last.start + last.bars]);
                last.drums = mean(&drums[last.start..last.start + last.bars]);
                continue;
            }
        }
        runs.push(r);
    }
    // builds: brighter, noisier, more snare hits towards the end, then the low end comes back
    for i in 0..runs.len() {
        let r = runs[i].clone();
        let h = (r.bars / 2).max(1);
        runs[i].slope = mean(&energy[r.start + r.bars - h..r.start + r.bars]) - mean(&energy[r.start..r.start + h]);
        if i + 1 >= runs.len() || r.bars < 2 {
            continue;
        }
        let a = r.start + r.bars - h;
        let rising = |v: &[f64]| mean(&v[a..r.start + r.bars]) - mean(&v[r.start..r.start + h]);
        let bright_up = rising(&cen) > 0.004 || rising(&flat) > 0.01;
        let rolls = hi_on[r.start + r.bars - 1] >= 1.4 * hi_on[r.start].max(1.0);
        let nx = &runs[i + 1];
        let low_back = mean(&low_e[nx.start..(nx.start + 2).min(nx.start + nx.bars)]) > 2.0 * mean(&low_e[(r.start + r.bars).saturating_sub(2)..r.start + r.bars]).max(1e-12);
        if (bright_up || rolls) && low_back {
            runs[i].roll = if bright_up && rolls { 1.0 } else { 0.6 };
        }
    }
    let labels = label_runs(&runs);
    let mut shape = Shape { bars: nb, energy: energy.clone(), tension: vec![0.0; nb], ..Shape::default() };
    // feel per section: the chords (the key's triads per bar from the chroma), brightness and energy
    let key = key.unwrap_or(Key::new(9, true));
    let emax = runs.iter().map(|r| r.e).fold(1e-9, f64::max);
    let cmean = mean(&cen);
    let feel = |r: &Run| -> (f32, f32, bool) {
        let mut maj = 0.0;
        let mut tot = 0.0;
        for b in r.start..r.start + r.bars {
            let mut h = [0.0f64; 12];
            for (i, hv) in h.iter_mut().enumerate() {
                *hv = chroma_bars[b][i].max(0.0);
            }
            if h.iter().sum::<f64>() <= 0.0 {
                continue;
            }
            let d = crate::songgen::best_triad(&h, key);
            if !degree_minor(key, d) {
                maj += 1.0;
            }
            tot += 1.0;
        }
        let ms = if tot > 0.0 { maj / tot } else { 0.5 };
        let bright = mean(&cen[r.start..r.start + r.bars]) - cmean;
        let er = r.e / emax;
        let v = ((ms * 2.0 - 1.0) * 0.5 + (bright * 8.0).clamp(-0.2, 0.2) + 0.1 * (er - 0.5)).clamp(-1.0, 1.0);
        (v as f32, (0.1 + 0.85 * er) as f32, ms < 0.5)
    };
    finish(&mut shape, &runs, &labels, &feel);
    info.boundaries_s = runs.iter().map(|r| (f0 + r.start as f64 * barf) / fps).collect();
    // brightness / swing
    info.bright = mean(&cen);
    info.bright_target = ((info.bright - 0.16) * 5.0).clamp(-1.0, 1.0);
    let mut offs: Vec<f64> = vec![];
    let mut k = f0;
    while k + t < env.len() as f64 {
        let (a, z) = ((k + 0.35 * t).ceil() as usize, (k + 0.8 * t).floor() as usize);
        if let Some((i, v)) = (a..z.min(env.len() - 1)).map(|i| (i, env[i])).max_by(|x, y| x.1.total_cmp(&y.1)) {
            if v > 0.25 && i > 0 && i + 1 < env.len() {
                let (y0, y1, y2) = (env[i - 1], env[i], env[i + 1]);
                let den = y0 - 2.0 * y1 + y2;
                let d = if den.abs() > 1e-9 { 0.5 * (y0 - y2) / den } else { 0.0 };
                offs.push((i as f64 + d.clamp(-0.5, 0.5) - k) / t);
            }
        }
        k += t;
    }
    let sw: Vec<f64> = offs.into_iter().filter(|f| (0.42..0.78).contains(f)).collect();
    info.swing = if sw.len() >= 8 { ((mean(&sw) - 0.5) / (2.0 / 3.0 - 0.5)).clamp(0.0, 1.0) } else { 0.0 };
    shape.groove = Groove { swing: info.swing, syncopation: 0.0, halftime: false };
    // a voice / lead melody: the pitch tracker over a few seconds in the middle of each section
    let secs_per_bar = barf / fps;
    for r in &runs {
        let mid = (f0 / fps + (r.start as f64 + r.bars as f64 * 0.5) * secs_per_bar).max(0.0);
        let a = ((mid - 2.0).max(0.0) * sr as f64) as usize;
        let z = (((mid + 2.0) * sr as f64) as usize).min(mono.len());
        if z <= a + sr as usize {
            info.vocal.push(0.0);
            continue;
        }
        let p = crate::vocal::track_pitch(&mono[a..z], sr as f32);
        let voiced: Vec<f64> = p.f0.iter().filter(|f| (110.0..900.0).contains(*f)).map(|f| 69.0 + 12.0 * (*f as f64 / 440.0).log2()).collect();
        let smooth = voiced.windows(2).filter(|w| (w[1] - w[0]).abs() < 1.0).count() as f64 / voiced.len().max(2) as f64;
        info.vocal.push(((voiced.len() as f64 / p.f0.len().max(1) as f64) * smooth * 1.6).clamp(0.0, 1.0));
    }
    // the palette: the low end and the lead of the loud sections
    let hot: Vec<usize> = (0..runs.len()).filter(|i| matches!(labels[*i], "chorus" | "drop")).collect();
    if !hot.is_empty() && bpm >= 95.0 {
        let fr_of = |i: usize| {
            let (a, _) = range(runs[i].start);
            let (_, z) = range(runs[i].start + runs[i].bars - 1);
            a..z
        };
        let (mut low, mut lm, mut cs, mut fl, mut fx, mut pw) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
        let mut lmv: Vec<f64> = vec![];
        let mut n = 0.0;
        for i in &hot {
            for f in &frames[fr_of(*i)] {
                low += f.low as f64;
                lm += f.lowmid as f64;
                cs += f.centroid as f64;
                fl += f.flat as f64;
                fx += f.flux as f64;
                pw += (f.power as f64).sqrt();
                lmv.push(f.lowmid as f64);
                n += 1.0;
            }
        }
        if n > 0.0 {
            let m = mean(&lmv);
            let modu = (lmv.iter().map(|v| (v - m).powi(2)).sum::<f64>() / n).sqrt() / m.max(1e-12);
            let (cs, fl) = (cs / n, fl / n);
            info.bass = if low > 3.0 * lm && modu < 0.6 {
                "sub"
            } else if modu > 0.6 && lm > 0.4 * low {
                "growl"
            } else if lm > 0.5 * low {
                "reese"
            } else {
                ""
            }
            .into();
            let attack = fx / pw.max(1e-12);
            info.lead = if cs > 0.3 && fl > 0.3 {
                "screech"
            } else if attack > 0.9 {
                "pluck"
            } else if cs > 0.17 {
                "supersaw"
            } else {
                "soft"
            }
            .into();
        }
    }
    info.shape = shape;
    info
}

/// The reference's sections replaced by the labelled ones (when the novelty found a form), and what else the recording says.
pub fn enrich_reference(r: &mut crate::songgen::RefInfo, stereo: &[f32], sr: u32) -> AudioInfo {
    let info = analyze_audio(stereo, sr, r.bpm, r.key);
    if info.shape.sections.len() >= 2 {
        r.sections = info.shape.sections.clone();
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(start: f64, len: f64, key: u8, vel: f32) -> N {
        N { start, len, key, vel }
    }

    /// intro 4 (a pad), verse A 8, pre 4 (climbing), chorus B 8 (drums, loud, high), verse A 8, pre 4, chorus B 8, outro 4 (the pad):
    /// A minor verses, a C major chorus
    pub(crate) fn song_with(roll: bool) -> (Vec<(String, &'static str, Kind, Vec<N>)>, usize) {
        let form: [(&str, usize); 8] = [("intro", 4), ("verse", 8), ("pre", 4), ("chorus", 8), ("verse", 8), ("pre", 4), ("chorus", 8), ("outro", 4)];
        let (mut pad, mut bass, mut lead, mut drums) = (vec![], vec![], vec![], vec![]);
        let mut bar = 0usize;
        for (tag, bars) in form {
            for k in 0..bars {
                let b0 = (bar + k) as f64 * 4.0;
                match tag {
                    "intro" | "outro" => {
                        for key in [57u8, 60, 64] {
                            pad.push(n(b0, 4.0, key, 0.35));
                        }
                    }
                    "verse" => {
                        // Am | Dm alternating, a low melody
                        let (root, tri) = if k % 2 == 0 { (45u8, [57u8, 60, 64]) } else { (50, [62, 65, 69]) };
                        for key in tri {
                            pad.push(n(b0, 4.0, key, 0.5));
                        }
                        bass.push(n(b0, 2.0, root, 0.6));
                        bass.push(n(b0 + 2.0, 2.0, root, 0.6));
                        for (i, d) in [0u8, 3, 5, 3].iter().enumerate() {
                            lead.push(n(b0 + i as f64, 0.9, 69 + d, 0.55));
                        }
                    }
                    "pre" => {
                        let tri = [65u8, 69, 72];
                        for key in tri {
                            pad.push(n(b0, 4.0, key, 0.6));
                        }
                        // quarters, then eighths: it climbs
                        let steps = [4, 4, 8, 8][k.min(3)];
                        for i in 0..steps {
                            lead.push(n(b0 + i as f64 * 4.0 / steps as f64, 4.0 / steps as f64 * 0.9, 72 + (i % 4) as u8, 0.5 + 0.07 * k as f32));
                        }
                        // a steady snare, or (a build) a roll: 1/4, 1/8, 1/16, 1/32
                        let hits = if roll { [4, 8, 16, 32][k.min(3)] } else { 4 };
                        for i in 0..hits {
                            drums.push(n(b0 + i as f64 * 4.0 / hits as f64, 0.05, 38, 0.4 + 0.08 * k as f32));
                        }
                        bass.push(n(b0, 4.0, 41, 0.7));
                    }
                    _ => {
                        // C | G major, high melody, full drums
                        let (root, tri) = if k % 2 == 0 { (48u8, [60u8, 64, 67]) } else { (43, [67, 71, 74]) };
                        for key in tri {
                            pad.push(n(b0, 4.0, key, 0.85));
                        }
                        for i in 0..8 {
                            bass.push(n(b0 + i as f64 * 0.5, 0.45, root, 0.95));
                            drums.push(n(b0 + i as f64 * 0.5, 0.1, 42, 0.7));
                        }
                        for i in 0..4 {
                            drums.push(n(b0 + i as f64, 0.1, 36, 1.0));
                        }
                        drums.push(n(b0 + 1.0, 0.1, 38, 0.95));
                        drums.push(n(b0 + 3.0, 0.1, 38, 0.95));
                        for (i, d) in [0u8, 4, 7, 12, 7, 4, 2, 0].iter().enumerate() {
                            lead.push(n(b0 + i as f64 * 0.5, 0.45, 72 + d, 0.95));
                        }
                    }
                }
            }
            bar += bars;
        }
        (vec![("Pad".into(), "pad", Kind::Pad, pad), ("Bass".into(), "bass", Kind::Bass, bass), ("Lead".into(), "lead", Kind::Melody, lead), ("Drums".into(), "drums", Kind::Drums, drums)], bar)
    }

    pub(crate) fn song() -> (Vec<(String, &'static str, Kind, Vec<N>)>, usize) {
        song_with(false)
    }

    fn shape_of(parts: &[(String, &'static str, Kind, Vec<N>)], bars: usize, key: Key) -> Shape {
        let p: Vec<PartIn> = parts.iter().map(|(name, role, kind, notes)| PartIn { name, role, kind: *kind, notes }).collect();
        analyze_notes(&p, bars, key)
    }

    #[test]
    fn labels_a_midi_form_from_its_repetition() {
        let (parts, bars) = song();
        let s = shape_of(&parts, bars, Key::new(9, true));
        assert_eq!(s.labels, vec!["intro", "verse", "pre", "chorus", "verse", "pre", "chorus", "outro"], "{}", s.structure_text());
        assert_eq!(s.sections.iter().map(|x| x.bars).collect::<Vec<_>>(), vec![4, 8, 4, 8, 8, 4, 8, 4]);
        // the verses are one letter, the choruses another
        assert_eq!(s.letters[1], s.letters[4]);
        assert_eq!(s.letters[3], s.letters[6]);
        assert_ne!(s.letters[1], s.letters[3]);
        // a pre-chorus is planned as a build
        assert_eq!(s.sections[2].tag, "build");
        // a snare roll into it: a build, and the loud part after it a drop
        let (parts, bars) = song_with(true);
        let s = shape_of(&parts, bars, Key::new(9, true));
        assert_eq!(s.labels, vec!["intro", "verse", "build", "drop", "verse", "build", "drop", "outro"], "{}", s.structure_text());
    }

    #[test]
    fn the_energy_rises_into_the_chorus() {
        let (parts, bars) = song();
        let s = shape_of(&parts, bars, Key::new(9, true));
        let e = &s.energy;
        let verse = mean(&e[4..12]);
        let pre = mean(&e[12..16]);
        let chorus = mean(&e[16..24]);
        assert!(verse < pre && pre < chorus, "verse {verse:.2} pre {pre:.2} chorus {chorus:.2}");
        assert!(e[15] > e[12], "the pre-chorus climbs: {:?}", &e[12..16]);
        assert!(mean(&e[0..4]) < verse);
        // sections carry it too
        assert!(s.sections[3].energy > s.sections[1].energy);
    }

    #[test]
    fn a_quiet_minor_song_that_ends_loud_and_major_tells_a_rise() {
        // quiet A minor at first, loud C major at the end
        let mut pad = vec![];
        let mut drums = vec![];
        let mut lead = vec![];
        for b in 0..32usize {
            let b0 = b as f64 * 4.0;
            let lvl = b as f32 / 32.0;
            let tri: [u8; 3] = if b < 16 { if b % 2 == 0 { [57, 60, 64] } else { [62, 65, 69] } } else if b % 2 == 0 { [60, 64, 67] } else { [55, 59, 62] };
            for k in tri {
                pad.push(n(b0, 4.0, k, 0.3 + 0.6 * lvl));
            }
            let notes_per_bar = 2 + b / 4;
            for i in 0..notes_per_bar {
                lead.push(n(b0 + i as f64 * 4.0 / notes_per_bar as f64, 0.4, 69 + (b / 4) as u8 + (i % 3) as u8 * 2, 0.4 + 0.5 * lvl));
            }
            if b >= 12 {
                for i in 0..(if b >= 20 { 8 } else { 4 }) {
                    drums.push(n(b0 + i as f64 * if b >= 20 { 0.5 } else { 1.0 }, 0.1, 36, 0.6 + 0.4 * lvl));
                }
            }
        }
        let parts = vec![("Pad".to_string(), "pad", Kind::Pad, pad), ("Lead".to_string(), "lead", Kind::Melody, lead), ("Drums".to_string(), "drums", Kind::Drums, drums)];
        let s = shape_of(&parts, 32, Key::new(9, true));
        assert_eq!(s.arc, "rise", "{} / {} / {:?}", s.story, s.emo_arc, s.feels);
        assert!(matches!(s.emo_arc.as_str(), "slow burn" | "triumph" | "release"), "{} {:?} {}", s.emo_arc, s.feels, s.structure_text());
        assert!(s.minor.first().copied().unwrap_or(false), "the start is minor: {:?}", s.minor);
        assert!(!s.minor.last().copied().unwrap_or(true), "the end is major: {:?}", s.minor);
        assert!(s.feels.first().unwrap().0 < s.feels.last().unwrap().0);
        assert!(s.story.starts_with("Rising: a ") && s.story.contains("minor intro"), "{}", s.story);
        assert!(s.story.contains("major"), "{}", s.story);
        assert_eq!(crate::songgen::story_arc(&s.story), "rise");
    }

    #[test]
    fn story_sentences_read_back_as_their_arc() {
        let labels: Vec<String> = ["intro", "verse", "pre", "chorus", "break", "build", "drop", "bridge", "chorus", "outro"].iter().map(|s| s.to_string()).collect();
        for arc in STORY_ARCS {
            for v in [-0.6f32, -0.1, 0.4] {
                for a in [0.2f32, 0.5, 0.9] {
                    let feels: Vec<(f32, f32)> = (0..labels.len()).map(|i| (v + 0.05 * i as f32, (a + 0.07 * i as f32).min(1.0))).collect();
                    let minor: Vec<bool> = (0..labels.len()).map(|i| i % 2 == 0).collect();
                    let t = story_text(arc, &labels, &feels, &minor, &[0.5; 10]);
                    assert_eq!(crate::songgen::story_arc(&t), arc, "{t}");
                    assert!(t.len() > 40, "{t}");
                }
            }
        }
    }

    #[test]
    fn finds_the_hook_and_the_groove() {
        let (parts, bars) = song();
        let s = shape_of(&parts, bars, Key::new(9, true));
        assert!(s.hook_count >= 4, "{} {:?}", s.hook_count, s.hook);
        assert_eq!(s.hook_from, "Lead");
        assert!(!s.groove.halftime);
        assert!(s.groove.swing < 0.2);
        // a swung hat line
        let hats: Vec<N> = (0..64).flat_map(|b| [n(b as f64, 0.1, 42, 0.8), n(b as f64 + 0.66, 0.1, 42, 0.6)]).collect();
        let snare: Vec<N> = (0..16).map(|b| n(b as f64 * 4.0 + 2.0, 0.1, 38, 0.9)).collect();
        let p = [PartIn { name: "Hats", role: "drums", kind: Kind::Drums, notes: &hats }, PartIn { name: "Snare", role: "drums", kind: Kind::Drums, notes: &snare }];
        let g = find_groove(&p);
        assert!(g.swing > 0.8, "{g:?}");
        assert!(g.halftime, "{g:?}");
    }

    /// a synthetic recording: intro 8 bars (a soft pad), verse 8 (kick, bass, pad), chorus 8 (kick, snare, hats, bright saw chords,
    /// louder), verse 8, chorus 8, outro 4 (pad) at 120 bpm
    fn render(sr: u32) -> (Vec<f32>, Vec<usize>) {
        let bpm = 120.0;
        let bar_s = 4.0 * 60.0 / bpm;
        let form: [(&str, usize); 6] = [("intro", 8), ("verse", 8), ("chorus", 8), ("verse", 8), ("chorus", 8), ("outro", 4)];
        let total: usize = form.iter().map(|f| f.1).sum();
        let ns = (total as f64 * bar_s * sr as f64) as usize;
        let mut out = vec![0.0f32; ns * 2];
        let hz = |m: f64| 440.0 * 2f64.powf((m - 69.0) / 12.0);
        let mut seed = 12345u64;
        let mut noise = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed as f64 / u64::MAX as f64) * 2.0 - 1.0
        };
        let mut bar = 0usize;
        let mut starts = vec![];
        for (tag, bars) in form {
            starts.push(bar);
            for k in 0..bars {
                let t0 = (bar + k) as f64 * bar_s;
                let s0 = (t0 * sr as f64) as usize;
                let s1 = (((bar + k + 1) as f64 * bar_s) * sr as f64) as usize;
                let (chord, lvl, saw): ([f64; 3], f64, bool) = match tag {
                    "intro" | "outro" => ([57.0, 60.0, 64.0], 0.05, false),
                    "verse" => (if k % 2 == 0 { [57.0, 60.0, 64.0] } else { [53.0, 57.0, 60.0] }, 0.08, false),
                    _ => (if k % 2 == 0 { [60.0, 64.0, 67.0] } else { [55.0, 59.0, 62.0] }, 0.16, true),
                };
                for i in s0..s1.min(ns) {
                    let t = i as f64 / sr as f64;
                    let mut v = 0.0;
                    for m in chord {
                        let ph = (t * hz(m)).fract();
                        v += if saw { (2.0 * ph - 1.0) * 0.7 } else { (std::f64::consts::TAU * ph).sin() };
                    }
                    v *= lvl;
                    let tb = t - t0;
                    let beat = 60.0 / bpm;
                    if tag != "intro" && tag != "outro" {
                        // kick on every beat (harder in the chorus), bass on the root
                        let kb = tb % beat;
                        v += if tag == "chorus" { 0.5 } else { 0.3 } * (-kb * 30.0).exp() * (std::f64::consts::TAU * (50.0 + 80.0 * (-kb * 40.0).exp()) * kb).sin();
                        v += 0.12 * (std::f64::consts::TAU * hz(chord[0] - 24.0) * t).sin();
                    }
                    if tag == "chorus" {
                        let sb = (tb + beat) % (2.0 * beat);
                        v += 0.25 * (-sb * 25.0).exp() * noise();
                        let hb = tb % (beat * 0.5);
                        v += 0.06 * (-hb * 80.0).exp() * noise();
                    }
                    out[i * 2] = v as f32;
                    out[i * 2 + 1] = (v * if saw { 0.8 } else { 1.0 }) as f32;
                }
            }
            bar += bars;
        }
        (out, starts)
    }

    #[test]
    fn a_recording_is_cut_at_its_sections() {
        let sr = 22050;
        let (st, starts) = render(sr);
        let info = analyze_audio(&st, sr, 120.0, Some(Key::new(9, true)));
        let got: Vec<usize> = {
            let mut at = 0usize;
            info.shape.sections.iter().map(|s| {
                let a = at;
                at += s.bars as usize;
                a
            }).collect()
        };
        let bar_s = 2.0;
        let db = info.downbeat_s / bar_s;
        for want in &starts[1..] {
            assert!(got.iter().any(|g| (*g as f64 + db - *want as f64).abs() <= 1.0), "boundary at bar {want}: found {got:?} (downbeat {:.2} s) {}", info.downbeat_s, info.shape.structure_text());
        }
        assert!(info.shape.sections.len() <= starts.len() + 2, "{}", info.shape.structure_text());
        // the loud parts are choruses
        let lab = &info.shape.labels;
        let hot_at = |bar: usize| {
            let mut at = 0usize;
            for (i, s) in info.shape.sections.iter().enumerate() {
                if bar >= at && bar < at + s.bars as usize {
                    return lab[i].clone();
                }
                at += s.bars as usize;
            }
            String::new()
        };
        assert!(matches!(hot_at(20).as_str(), "chorus" | "drop"), "{}", info.shape.structure_text());
        assert_eq!(hot_at(2), "intro", "{} (downbeat {:.2} s)", info.shape.structure_text(), info.downbeat_s);
        assert_eq!(hot_at(10), "verse", "{} {:?}", info.shape.structure_text(), info.shape.letters);
        assert!(info.lufs < -5.0 && info.lufs > -40.0, "{}", info.lufs);
        assert!(!info.shape.story.is_empty());
        // energy: the choruses are louder than the verses
        assert!(mean(&info.shape.energy[17..23]) > mean(&info.shape.energy[9..15]));
    }
}
