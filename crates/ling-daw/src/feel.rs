//! Performing with feel: an emotional plan (emotion.rs) laid onto parts that already exist - an imported MIDI file, a mashup, any
//! project. The AI song writer plans the feel before writing; here the notes are there already and get *played* with it.
//!
//! * Sections: the project's tags on the ruler, else found from the music ([`infer_sections`]: the energy - notes x velocity - of every
//!   4 bars, runs of similar energy joined into sections of 8+ bars, tagged by level and place: intro, verse, chorus, break, outro).
//! * The arc: named (an emotional arc or a story arc), or read from the music ([`infer_arc`]: how the energy travels and the mode).
//! * The plan: emotion::plan over those sections (valence, arousal, tension, vocal push, breath, space per section, with its dice).
//! * The performance ([`perform`]), every part by its role (drums, bass, pad, chords, lead, voice):
//!   - dynamics follow the arousal (drums and bass most, pads least), the downbeats leaned on as it pushes, ghost notes fade when calm;
//!   - the lead / voice follows the vocal push, and where it plays the pads and chords make room as the space asks;
//!   - articulation: held tension shortens the chords and leads, calm sections let them ring into the next note;
//!   - a level lane rides each part a little with the arousal, and a filter lane (an instrument's cutoff) darkens sad sections and
//!     opens happy ones. Lanes the project already has are never overwritten.
//!   Velocities are scaled relative to the middle of the range, so a part keeps its own dynamics and the arc moves them.
use crate::emotion::{EmotionPlan, Feel};
use crate::model::{AutoLane, ClipData, Track};
use crate::songgen::Section;

/// a span of the timeline, in beats, with its tag
#[derive(Clone, Debug, PartialEq)]
pub struct Sec {
    pub start: f64,
    pub end: f64,
    pub tag: String,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Role {
    Drums,
    Bass,
    Pad,
    Chords,
    Lead,
    Voice,
}

/// what a track plays, from its instrument and its notes
pub fn role_of(t: &Track) -> Role {
    let name = t.name.to_lowercase();
    if t.inst_kind == "drums" || name.contains("drum") || name.contains("break") || name.contains("kit") || name.contains("perc") {
        return Role::Drums;
    }
    if t.inst_kind == "voice" || name.contains("vocal") || name.contains("voice") || name.contains("chops") {
        return Role::Voice;
    }
    let notes: Vec<&crate::model::Note> = t.clips.iter().flat_map(|c| match &c.data {
        ClipData::Midi(n) => n.iter().collect::<Vec<_>>(),
        _ => vec![],
    }).collect();
    if notes.is_empty() {
        return Role::Pad;
    }
    let mean = notes.iter().map(|n| n.key as f64).sum::<f64>() / notes.len() as f64;
    if mean < 50.0 || name.contains("bass") {
        return Role::Bass;
    }
    let avg = notes.iter().map(|n| n.len).sum::<f64>() / notes.len() as f64;
    let poly = notes.iter().map(|a| notes.iter().filter(|b| (b.start - a.start).abs() < 0.02).count()).sum::<usize>() as f64 / notes.len() as f64;
    if poly > 1.6 {
        if avg >= 1.5 { Role::Pad } else { Role::Chords }
    } else if name.contains("pad") {
        Role::Pad
    } else {
        Role::Lead
    }
}

/// every note of a track, absolute: (start, length, velocity)
fn notes_abs(t: &Track) -> Vec<(f64, f64, f32)> {
    t.clips.iter().flat_map(|c| match &c.data {
        ClipData::Midi(n) => n.iter().filter(|x| x.start < c.len + 1e-6).map(|x| (c.start + x.start, x.len, x.vel)).collect::<Vec<_>>(),
        _ => vec![],
    }).collect()
}

/// Sections found from the music: the energy of every 4 bars (notes x velocity, all tracks), neighbours of similar energy joined into
/// sections of at least 8 bars, tagged by their level (the loudest third: chorus, the quietest: break; intro / outro at the ends).
pub fn infer_sections(tracks: &[Track], end: f64) -> Vec<Sec> {
    let blocks = ((end / 16.0).ceil() as usize).max(1);
    let mut e = vec![0f64; blocks];
    for t in tracks {
        for (s, _, v) in notes_abs(t) {
            let b = ((s / 16.0) as usize).min(blocks - 1);
            e[b] += v as f64;
        }
    }
    let peak = e.iter().copied().fold(0.0, f64::max).max(1e-9);
    let lv: Vec<f64> = e.iter().map(|x| x / peak).collect();
    // join: a new section where the level steps by more than 0.2, never shorter than 2 blocks
    let mut cuts = vec![0usize];
    for b in 1..blocks {
        let start = *cuts.last().unwrap_or(&0);
        let mean = lv[start..b].iter().sum::<f64>() / (b - start) as f64;
        if b - start >= 2 && (lv[b] - mean).abs() > 0.2 {
            cuts.push(b);
        }
    }
    cuts.push(blocks);
    let spans: Vec<(usize, usize, f64)> = cuts.windows(2).map(|w| (w[0], w[1], lv[w[0]..w[1]].iter().sum::<f64>() / (w[1] - w[0]) as f64)).collect();
    let mut sorted: Vec<f64> = spans.iter().map(|s| s.2).collect();
    sorted.sort_by(f64::total_cmp);
    let hi = sorted[(sorted.len() * 2 / 3).min(sorted.len() - 1)];
    let lo = sorted[sorted.len() / 3];
    let n = spans.len();
    spans
        .iter()
        .enumerate()
        .map(|(i, (a, b, l))| {
            let tag = if i == 0 && n > 2 && *l <= lo + 1e-9 {
                "intro"
            } else if i + 1 == n && n > 2 && *l <= lo + 1e-9 {
                "outro"
            } else if *l >= hi - 1e-9 && n > 1 {
                "chorus"
            } else if *l <= lo + 1e-9 && n > 2 {
                "break"
            } else {
                "verse"
            };
            Sec { start: *a as f64 * 16.0, end: (*b as f64 * 16.0).min(end.max(16.0)), tag: tag.into() }
        })
        .collect()
}

/// the energy (0..1) of each section
pub fn section_energy(tracks: &[Track], secs: &[Sec]) -> Vec<f64> {
    let mut e = vec![0f64; secs.len()];
    for t in tracks {
        for (s, _, v) in notes_abs(t) {
            if let Some(i) = secs.iter().position(|x| s >= x.start - 1e-6 && s < x.end - 1e-6) {
                e[i] += v as f64;
            }
        }
    }
    let per: Vec<f64> = e.iter().zip(secs).map(|(x, s)| x / (s.end - s.start).max(1.0)).collect();
    let peak = per.iter().copied().fold(0.0, f64::max).max(1e-9);
    per.iter().map(|x| (x / peak).clamp(0.05, 1.0)).collect()
}

/// The arc the music itself suggests: how its energy travels (rising, falling, peaking late, swinging) and its mode.
pub fn infer_arc(energy: &[f64], minor: bool) -> &'static str {
    let n = energy.len();
    if n < 2 {
        return if minor { "melancholy" } else { "release" };
    }
    let first = energy[..n / 2].iter().sum::<f64>() / (n / 2) as f64;
    let second = energy[n / 2..].iter().sum::<f64>() / (n - n / 2) as f64;
    let peak_at = (0..n).max_by(|a, b| energy[*a].total_cmp(&energy[*b])).unwrap_or(0) as f64 / (n - 1) as f64;
    let swing = energy.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f64>() / (n - 1) as f64;
    let mean = energy.iter().sum::<f64>() / n as f64;
    match (minor, second > first + 0.12, first > second + 0.12) {
        (_, _, _) if swing > 0.35 => if minor { "storm" } else { "release" },
        (_, _, _) if mean < 0.35 => if minor { "melancholy" } else { "lullaby" },
        (true, true, _) => "slow burn",
        (true, _, true) => "melancholy",
        (true, _, _) => if peak_at > 0.6 { "bittersweet" } else { "storm" },
        (false, true, _) => "triumph",
        (false, _, true) => "bittersweet",
        (false, _, _) => if peak_at > 0.55 { "release" } else { "slow burn" },
    }
}

/// a story arc (songgen) as the emotional arc that plays it
pub fn emotion_arc(a: &str) -> String {
    match a.trim().to_lowercase().as_str() {
        "rise" => "slow burn".into(),
        "fall" => "melancholy".into(),
        "journey" => "bittersweet".into(),
        "dream" => "lullaby".into(),
        "battle" => "storm".into(),
        "hero" => "triumph".into(),
        other => other.into(),
    }
}

/// An arc from words ("make it sadder", "more triumphant", "calmer", "stormy", "a slow build" ...): an arc's own name, a story arc,
/// or a feeling word; None when the words name none.
pub fn arc_from_words(text: &str) -> Option<&'static str> {
    let t = format!(" {} ", text.to_lowercase());
    let has = |ws: &[&str]| ws.iter().any(|w| t.contains(w));
    for a in crate::emotion::ARCS {
        if t.contains(a) {
            return crate::emotion::ARCS.iter().copied().find(|x| *x == a);
        }
    }
    Some(if has(&["bittersweet", "nostalg", "wistful", "longing", "happy-sad", "happy sad"]) {
        "bittersweet"
    } else if has(&["sad", "melanchol", "heartbr", "lonely", "grief", "mourn", "blue ", "depress", "somber", "sombre", "fall"]) {
        "melancholy"
    } else if has(&["triumph", "victor", "heroic", "hero", "epic", "anthem", "uplift", "glorious", "proud"]) {
        "triumph"
    } else if has(&["storm", "angry", "anger", "rage", "aggress", "furious", "dark and intense", "intense", "battle", "chaotic", "frantic"]) {
        "storm"
    } else if has(&["calm", "lullaby", "dream", "sleep", "gentle", "soft", "peaceful", "relax", "tender", "intimate"]) {
        "lullaby"
    } else if has(&["slow burn", "build", "grow", "rise", "rising", "crescendo", "gradual"]) {
        "slow burn"
    } else if has(&["happy", "happier", "joy", "euphor", "release", "cathar", "free", "bright", "fun", "upbeat", "emotional"]) {
        "release"
    } else {
        return None;
    })
}

/// the emotional plan for these sections (their energy measured), the arc named or read from the music
pub fn plan_for(tracks: &[Track], secs: &[Sec], arc: &str, genre: &str, words: &[String], minor: bool, seed: u64) -> EmotionPlan {
    let energy = section_energy(tracks, secs);
    let arc = {
        let a = emotion_arc(arc);
        if crate::emotion::ARCS.contains(&a.as_str()) { a } else { infer_arc(&energy, minor).to_string() }
    };
    let sections: Vec<Section> = secs.iter().zip(&energy).map(|(s, e)| Section { tag: s.tag.clone(), bars: ((s.end - s.start) / 4.0).round().max(1.0) as u32, energy: *e }).collect();
    crate::emotion::plan(&sections, &arc, genre, words, 0.5, seed)
}

/// the feel at a beat: the section's, eased in over its first beat from the one before
fn feel_at(secs: &[Sec], feels: &[Feel], beat: f64) -> Feel {
    let i = secs.iter().rposition(|s| s.start <= beat + 1e-6).unwrap_or(0).min(feels.len().saturating_sub(1));
    let f = feels[i];
    let x = ((beat - secs[i].start) / 1.0).clamp(0.0, 1.0) as f32;
    if i == 0 || x >= 1.0 {
        return f;
    }
    let p = feels[i - 1];
    let mix = |a: f32, b: f32| a + (b - a) * x;
    Feel { valence: mix(p.valence, f.valence), arousal: mix(p.arousal, f.arousal), tension: mix(p.tension, f.tension), vocal: mix(p.vocal, f.vocal), breath: mix(p.breath, f.breath), space: mix(p.space, f.space) }
}

/// Play the tracks `which` with the plan's feel, `amount` 0..1 (how far the performance moves). Returns what it did, in words.
pub fn perform(tracks: &mut [Track], which: &[usize], secs: &[Sec], plan: &EmotionPlan, amount: f32) -> Vec<String> {
    let mut out = vec![];
    if secs.is_empty() || plan.feels.is_empty() || amount <= 0.0 {
        return out;
    }
    let amt = amount.clamp(0.0, 1.0);
    let feels = &plan.feels;
    let roles: Vec<(usize, Role)> = which.iter().copied().filter(|t| *t < tracks.len()).map(|t| (t, role_of(&tracks[t]))).collect();
    // where the lead / voice sounds (for the room the others make)
    let mut lead_spans: Vec<(f64, f64)> = vec![];
    for (t, r) in &roles {
        if matches!(r, Role::Lead | Role::Voice) {
            lead_spans.extend(notes_abs(&tracks[*t]).into_iter().map(|(s, l, _)| (s, s + l)));
        }
    }
    let has_lead = !lead_spans.is_empty();
    let lerp = |x: f32| 1.0 + (x - 1.0) * amt;
    let mut touched = 0usize;
    for (t, role) in &roles {
        let tr = &mut tracks[*t];
        for c in tr.clips.iter_mut() {
            let cs = c.start;
            let ClipData::Midi(ns) = &mut c.data else { continue };
            let starts: Vec<f64> = ns.iter().map(|n| n.start).collect();
            for n in ns.iter_mut() {
                let at = cs + n.start;
                let f = feel_at(secs, feels, at);
                let (a, push) = (f.arousal, f.vocal);
                let down = (at.rem_euclid(4.0)) < 0.05;
                // dynamics, relative to the middle of the range (a = 0.5 leaves the part as it was)
                let mut k = match role {
                    Role::Drums => (0.6 + 0.55 * a) / 0.875 + if down { 0.1 * (a - 0.5) } else { 0.0 },
                    Role::Bass => (0.65 + 0.45 * a) / 0.875,
                    Role::Pad => (0.75 + 0.3 * a) / 0.9,
                    Role::Chords => (0.7 + 0.35 * a) / 0.875,
                    Role::Lead | Role::Voice => (0.55 + 0.5 * push) / 0.8 + if down { 0.08 * (push - 0.5) } else { 0.0 },
                };
                if *role == Role::Drums && n.vel < 0.4 {
                    // ghost notes fade in the calm sections
                    k *= 0.7 + 0.6 * a;
                }
                // room for the lead: the pads and chords soften where it sounds
                if has_lead && matches!(role, Role::Pad | Role::Chords) {
                    let over = lead_spans.iter().any(|(s, e)| at < *e && at + n.len > *s);
                    if over {
                        k *= 1.0 - 0.3 * f.space;
                    }
                }
                n.vel = (n.vel * lerp(k)).clamp(0.05, 1.0);
                // articulation: held tension shortens, calm lets it ring into the next note
                if matches!(role, Role::Chords | Role::Lead) {
                    let next = starts.iter().copied().filter(|s| *s > n.start + 1e-6).fold(f64::MAX, f64::min);
                    let room = if next < f64::MAX { next - n.start } else { n.len };
                    let scale = 1.0 - 0.3 * f.tension as f64 + 0.25 * (1.0 - f.arousal as f64) * (1.0 - f.tension as f64);
                    let scale = 1.0 + (scale - 1.0) * amt as f64;
                    n.len = (n.len * scale).min(room.max(n.len)).max(0.06);
                }
                touched += 1;
            }
        }
        // a level lane and a filter lane, section by section, where the project has none
        let lane = |base: f32, fun: &dyn Fn(&Feel) -> f32| -> Vec<(f64, f32)> {
            let mut pts = vec![];
            for (i, s) in secs.iter().enumerate() {
                let Some(f) = feels.get(i) else { continue };
                let v = fun(f);
                pts.push((s.start + if i == 0 { 0.0 } else { 0.5 }, base * v));
                pts.push((s.end - 0.01, base * v));
            }
            pts
        };
        if !tr.auto.iter().any(|l| l.param == "vol") {
            let base = tr.volume;
            let depth = if *role == Role::Pad { 0.6 } else { 1.0 };
            let pts = lane(base, &|f| 10f32.powf(depth * 3.0 * (f.arousal - 0.5) * amt / 20.0));
            tr.auto.push(AutoLane { param: "vol".into(), points: pts.into_iter().map(|(b, v)| (b, v.clamp(0.0, 2.0))).collect() });
        }
        if *role != Role::Drums {
            let params: Vec<(&'static str, f32)> = tr.inst.as_ref().map(|i| i.params()).unwrap_or_default();
            if let Some((name, cur)) = params.iter().find(|(n, _)| matches!(*n, "cutoff" | "cut" | "filter cutoff")).copied() {
                if !tr.auto.iter().any(|l| l.param == name) && cur > 0.05 {
                    let pts = lane(1.0, &|f| (cur + (0.12 * f.valence + 0.08 * (f.arousal - 0.5)) * amt).clamp(0.02, 1.0));
                    tr.auto.push(AutoLane { param: name.into(), points: pts });
                }
            }
        }
    }
    if touched > 0 {
        let count = |r: Role| roles.iter().filter(|x| x.1 == r).count();
        out.push(format!(
            "{touched} notes played with the feel ({} drums, {} bass, {} pads, {} chords, {} lead, {} voice parts)",
            count(Role::Drums),
            count(Role::Bass),
            count(Role::Pad),
            count(Role::Chords),
            count(Role::Lead),
            count(Role::Voice)
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Clip, Note, TrackKind};

    fn track(name: &str, kind: &str, notes: Vec<Note>, len: f64) -> Track {
        let mut t = Track::new(name, TrackKind::Instrument, 0, crate::dsp::make_instrument(if kind == "drums" { "drums" } else { "synth" }, 48000.0));
        t.inst_kind = kind.into();
        let mut c = Clip::midi(0.0, len, "c");
        if let ClipData::Midi(v) = &mut c.data {
            v.extend(notes);
        }
        t.clips.push(c);
        t
    }

    /// 64 bars: quiet 16, louder 16, loud 16, quiet 16 - drums, a bass line, a chord part and a lead
    fn song() -> Vec<Track> {
        let len = 256.0;
        let dens = |b: f64| if b < 64.0 || b >= 192.0 { 1 } else if b < 128.0 { 2 } else { 4 };
        let mut drums = vec![];
        let mut bass = vec![];
        let mut chords = vec![];
        let mut lead = vec![];
        let mut b = 0.0;
        while b < len {
            let d = dens(b);
            for k in 0..d {
                drums.push(Note::new(b + k as f64 / d as f64, 0.1, if k % 2 == 0 { 36 } else { 38 }, 0.8));
            }
            bass.push(Note::new(b, 0.9, 40, 0.7));
            if (b as i64) % 4 == 0 {
                for k in [57u8, 60, 64] {
                    chords.push(Note::new(b, 1.0, k, 0.6));
                }
            }
            if d >= 2 {
                lead.push(Note::new(b + 0.5, 0.4, 72, 0.7));
            }
            b += 1.0;
        }
        vec![track("Drums", "drums", drums, len), track("Bass", "synth", bass, len), track("Keys", "synth", chords, len), track("Lead", "synth", lead, len)]
    }

    #[test]
    fn sections_and_an_arc_are_read_from_the_music_and_the_parts_play_with_the_feel() {
        let mut t = song();
        let roles: Vec<Role> = t.iter().map(role_of).collect();
        assert_eq!(roles, vec![Role::Drums, Role::Bass, Role::Chords, Role::Lead]);
        let secs = infer_sections(&t, 256.0);
        assert!(secs.len() >= 3, "{secs:?}");
        assert_eq!(secs.first().unwrap().start, 0.0);
        assert_eq!(secs.last().unwrap().end, 256.0);
        let loud = secs.iter().find(|s| s.start <= 150.0 && s.end > 150.0).unwrap();
        assert_eq!(loud.tag, "chorus", "{secs:?}");
        let e = section_energy(&t, &secs);
        assert!(["release", "slow burn", "triumph", "bittersweet"].contains(&infer_arc(&e, false)), "{}", infer_arc(&e, false));
        assert_eq!(infer_arc(&[0.2, 0.3, 0.5, 0.8, 1.0], true), "slow burn");
        assert_eq!(emotion_arc("hero"), "triumph");
        assert_eq!(arc_from_words("make it sadder"), Some("melancholy"));
        assert_eq!(arc_from_words("more triumphant please"), Some("triumph"));
        assert_eq!(arc_from_words("calmer, like a lullaby"), Some("lullaby"));
        assert_eq!(arc_from_words("a slow build to the end"), Some("slow burn"));
        assert_eq!(arc_from_words("bittersweet"), Some("bittersweet"));
        assert_eq!(arc_from_words("add reverb"), None);
        // the plan, then the performance
        let plan = plan_for(&t, &secs, "storm", "pop", &[], false, 3);
        assert_eq!(plan.arc, "storm");
        assert_eq!(plan.feels.len(), secs.len());
        let all: Vec<usize> = (0..t.len()).collect();
        let notes = perform(&mut t, &all, &secs, &plan, 1.0);
        assert!(notes[0].contains("notes played"), "{notes:?}");
        // the drums are louder where the plan is more intense
        let vel_in = |tr: &Track, s: &Sec| {
            let v: Vec<f32> = notes_abs(tr).into_iter().filter(|(b, _, _)| *b >= s.start && *b < s.end).map(|x| x.2).collect();
            v.iter().sum::<f32>() / v.len().max(1) as f32
        };
        let (hot, cold) = {
            let mut idx: Vec<usize> = (0..secs.len()).collect();
            idx.sort_by(|a, b| plan.feels[*a].arousal.total_cmp(&plan.feels[*b].arousal));
            (idx[idx.len() - 1], idx[0])
        };
        if plan.feels[hot].arousal > plan.feels[cold].arousal + 0.15 {
            assert!(vel_in(&t[0], &secs[hot]) > vel_in(&t[0], &secs[cold]) + 0.02);
        }
        assert!(t[0].auto.iter().any(|l| l.param == "vol"), "a level lane");
        assert!(t[2].auto.iter().any(|l| l.param == "cutoff"), "a filter lane on the keys");
        // nothing changes at amount 0, and a lane the project has is kept
        let mut u = song();
        let before0 = notes_abs(&u[0]);
        u[1].auto.push(AutoLane { param: "vol".into(), points: vec![(0.0, 0.3)] });
        let _ = perform(&mut u, &all, &secs, &plan, 0.0);
        assert_eq!(notes_abs(&u[0]), before0);
        let _ = perform(&mut u, &all, &secs, &plan, 1.0);
        assert_eq!(u[1].auto.iter().filter(|l| l.param == "vol").count(), 1);
        assert_eq!(u[1].auto[0].points, vec![(0.0, 0.3)]);
    }
}
