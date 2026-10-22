//! Builtins for the theory assistant (`daw_th_*`), the style thumbprint (`daw_style_*`) and the MIDI library (`daw_lib_*`).
//! The analysis of the clip being edited is cached per project revision, so the piano roll can ask for it every frame.
use super::{Daw, A, Val};
use crate::compose::{self, Kind, Part, Spec};
use crate::model::*;
use crate::theory::{self, Chord, Key, KeySection, Phrase, N};
use std::sync::Arc;

pub struct Analysis {
    pub notes: Vec<N>,
    pub keys: Vec<KeySection>,
    pub chords: Vec<Chord>,
    pub roles: Vec<(u8, i32)>,
    pub phrases: Vec<Phrase>,
    pub main: Key,
    pub confidence: f64,
    pub mono: bool,
    pub harmony_from_others: bool,
    pub clip_start: f64,
    pub clip_len: f64,
    /// per note: another instrument plays the same pitch class at that moment
    pub support: Vec<bool>,
    /// per note: the same pitch comes back at the same place in the bar again and again
    pub repeated: Vec<bool>,
    /// per note: it grinds (half step / major seventh) against another instrument sounding at that moment
    pub clash: Vec<bool>,
    /// the other instruments' notes ordered by start (clip-relative), for checking fixes
    pub others_sorted: Vec<N>,
}

fn is_drums(t: &Track) -> bool {
    t.inst_kind == "drums"
}

/// the notes of a clip in absolute beats, with the loop unrolled
pub(super) fn expand(c: &Clip) -> Vec<N> {
    let ClipData::Midi(ns) = &c.data else { return vec![] };
    let ll = if c.loop_len >= 0.25 && c.loop_len < c.len { c.loop_len } else { 0.0 };
    let mut out = vec![];
    for n in ns {
        if ll > 0.0 {
            if n.start >= ll {
                continue;
            }
            let mut s = n.start;
            while s < c.len {
                out.push(N { start: c.start + s, len: n.len.min(c.len - s), key: n.key, vel: n.vel });
                s += ll;
            }
        } else if n.start < c.len {
            out.push(N { start: c.start + n.start, len: n.len.min(c.len - n.start).max(0.03), key: n.key, vel: n.vel });
        }
    }
    out
}

fn mono_score(notes: &[N]) -> f64 {
    if notes.is_empty() {
        return 1.0;
    }
    let mut on: Vec<i64> = notes.iter().map(|n| (n.start * 8.0).round() as i64).collect();
    on.sort();
    let mut distinct = on.clone();
    distinct.dedup();
    notes.len() as f64 / distinct.len().max(1) as f64
}

pub fn analyze(en: &crate::engine::Engine, t: usize, c: usize) -> Option<Analysis> {
    let track = en.project.tracks.get(t)?;
    if is_drums(track) {
        return None;
    }
    let clip = track.clips.get(c)?;
    let ClipData::Midi(own_notes) = &clip.data else { return None };
    let own: Vec<N> = own_notes.iter().map(|n| N { start: n.start, len: n.len, key: n.key, vel: n.vel }).collect();
    let (cs, ce) = (clip.start, clip.start + clip.len.max(4.0));
    // everything else that plays during the clip, shifted so the clip starts at beat 0; each instrument also gets a vote on the key
    let mut others: Vec<N> = vec![];
    let mut votes: Vec<theory::TrackVote> = vec![];
    for (ti, tr) in en.project.tracks.iter().enumerate() {
        if ti == t || is_drums(tr) {
            continue;
        }
        let mut mine: Vec<N> = vec![];
        for cl in &tr.clips {
            if cl.end() <= cs || cl.start >= ce {
                continue;
            }
            for n in expand(cl) {
                if n.end() > cs && n.start < ce {
                    let start = (n.start - cs).max(0.0);
                    mine.push(N { start, len: (n.end() - cs - start).max(0.03), ..n });
                }
            }
        }
        if mine.is_empty() {
            continue;
        }
        // a loud single-note line decides the structure; the harmony supports it (its notes count a little more or less)
        let (_, w) = theory::prominence(&mine);
        votes.push(theory::TrackVote { notes: mine.clone(), weight: w });
        for n in &mine {
            others.push(N { vel: (n.vel as f64 * w).clamp(0.05, 2.5) as f32, ..*n });
        }
    }
    let total = clip.len.clamp(4.0, 16384.0);
    let (_, own_w) = theory::prominence(&own);
    votes.push(theory::TrackVote { notes: own.clone(), weight: own_w.max(0.5) });
    let mut all = own.clone();
    all.extend(others.iter().copied());
    let keys = theory::key_sections_vote(&votes, total, 4.0);
    let keys = if keys.is_empty() { theory::key_sections(&all, total, 4.0) } else { keys };
    let keys = theory::merge_short_keys(keys, 20.0);
    // the song's key for this clip is the one that holds the longest
    let main = keys.iter().max_by(|a, b| (a.end - a.start).partial_cmp(&(b.end - b.start)).unwrap()).map(|k| k.key).unwrap_or(Key::new(0, true));
    let (_, conf, _) = theory::detect_key(&all, 0.0, total);
    let mono = mono_score(&own) < 1.4;
    let from_others = mono && others.len() >= 6;
    let harmony: Vec<N> = if from_others {
        others.clone()
    } else {
        let mut h = own.clone();
        h.extend(others.iter().copied());
        h
    };
    let poly = mono_score(&harmony) >= 1.4;
    let chords = theory::detect_chords(&harmony, total, if poly { 2.0 } else { 4.0 }, !poly);
    let roles = theory::note_roles(&own, &keys, &chords);
    let phrases = theory::phrases(&own, &chords, &keys);
    // support: does anything else sound this pitch class when the note starts?
    let mut sorted: Vec<&N> = others.iter().collect();
    sorted.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap());
    let support: Vec<bool> = own
        .iter()
        .map(|n| {
            let lo = sorted.partition_point(|o| o.start < n.start - 8.0);
            sorted[lo..].iter().take_while(|o| o.start <= n.start + 0.02).any(|o| o.end() > n.start + 0.02 && o.key % 12 == n.key % 12)
        })
        .collect();
    // repetition: the same pitch at the same place in the bar, three times or more in this clip
    let mut seen: std::collections::HashMap<(u8, i64), usize> = Default::default();
    let cell = |n: &N| (n.key, ((n.start % 4.0) * 4.0).round() as i64);
    for n in &own {
        *seen.entry(cell(n)).or_insert(0) += 1;
    }
    let repeated: Vec<bool> = own.iter().map(|n| seen.get(&cell(n)).copied().unwrap_or(0) >= 3).collect();
    let others_sorted: Vec<N> = sorted.iter().map(|o| **o).collect();
    let clash: Vec<bool> = own.iter().map(|n| theory::clashes_with_others(&others_sorted, n.start, n.len, n.key)).collect();
    Some(Analysis { notes: own, keys, chords, roles, phrases, main, confidence: conf, mono, harmony_from_others: from_others, clip_start: cs, clip_len: clip.len, support, repeated, clash, others_sorted })
}

fn qual_of(name: &str) -> usize {
    theory::qual_index(name).unwrap_or(0)
}

fn project_parts(en: &crate::engine::Engine) -> Vec<Part> {
    let mut parts = vec![];
    for tr in &en.project.tracks {
        if matches!(tr.kind, TrackKind::Audio) {
            continue;
        }
        let mut notes = vec![];
        for c in &tr.clips {
            notes.extend(expand(c));
        }
        if !notes.is_empty() {
            parts.push(Part { name: tr.name.clone(), drums: is_drums(tr), notes });
        }
    }
    parts
}

fn file_parts(path: &str) -> Option<(Vec<Part>, f64)> {
    let data = std::fs::read(path).ok()?;
    let m = crate::midi_import::parse_any(path, &data).ok()?;
    let parts = m
        .tracks
        .iter()
        .filter(|t| t.samples.is_empty() && !t.notes.is_empty())
        .map(|t| Part { name: t.name.clone(), drums: t.program.is_none(), notes: t.notes.iter().map(|n| N { start: n.start, len: n.len, key: n.key, vel: n.vel }).collect() })
        .collect();
    Some((parts, m.bpm.unwrap_or(120.0)))
}

/// parameters worth describing to a model: the sound itself, not the envelope points and the modulation matrix
/// not the tuning table, the root note or the matrix wheel source: knobs a person would not call part of the sound
fn derive_musical(name: &str) -> bool {
    let b = name.as_bytes();
    let tuning = b.len() >= 2 && b[0] == b't' && b[1..].iter().all(|c| c.is_ascii_digit());
    !(tuning || matches!(name, "root" | "scale" | "stretch" | "whl" | "out" | "gain" | "level" | "tempo" | "bpm" | "vol" | "volume" | "pan"))
}

fn derive_core(name: &str) -> bool {
    let b = name.as_bytes();
    let mseg = b.len() > 3 && b[0] == b'm' && b[1].is_ascii_digit() && b[2] == b' ';
    !(name.starts_with("x ") || mseg)
}

fn field<'a>(fields: &'a str, k: &str) -> &'a str {
    fields.split(';').find_map(|p| p.strip_prefix(&format!("{k}="))).unwrap_or("")
}

/// "Ling Membrane Snare" for a drum part, the instrument's name otherwise: the model has to know which sound it is changing
fn derive_label(i: &dyn crate::dsp::Instrument) -> String {
    let parts = i.parts();
    match parts.get(i.part()) {
        Some(p) if !parts.is_empty() => format!("{} {} drum", i.name(), p),
        _ => i.name().to_string(),
    }
}

/// up to 24 of the parameters worth changing: the ones that shape a sound come first, then a seeded handful of the others
fn derive_pick(i: &dyn crate::dsp::Instrument, seed: u64) -> Vec<(String, f32)> {
    const KEY: [&str; 36] = ["tune", "tension", "hardness", "body", "strike", "wires", "cut", "res", "drive", "amp a", "amp d", "amp s", "amp r", "flt d", "fenv", "a det", "b det", "a wave", "b wave", "a uni", "sub", "noise", "hz mix", "ch mix", "dl mix", "rv mix", "rv size", "ph mix", "glide", "cutoff", "resonance", "attack", "decay", "sustain", "release", "detune"];
    let all: Vec<(String, f32)> = i.params().into_iter().filter(|(n, _)| derive_core(n) && derive_musical(n)).map(|(n, v)| (n.to_string(), v)).collect();
    let mut rng = compose::Rng::new(seed + 3);
    let mut order: Vec<usize> = (0..all.len()).filter(|k| KEY.contains(&all[*k].0.as_str())).collect();
    order.truncate(14);
    let mut rest: Vec<usize> = (0..all.len()).filter(|k| !order.contains(k)).collect();
    for k in (1..rest.len()).rev() {
        let j = (rng.next() % (k as u64 + 1)) as usize;
        rest.swap(k, j);
    }
    order.extend(rest.into_iter().take(24 - order.len()));
    order.sort_unstable();
    order.iter().map(|k| (all[*k].0.replace(' ', "_"), all[*k].1)).collect()
}

fn derive_json(p: &[(String, f32)]) -> String {
    format!("{{{}}}", p.iter().map(|(n, v)| format!("\"{n}\": {v:.2}")).collect::<Vec<_>>().join(", "))
}

impl Daw {
    fn th(&mut self, t: usize, c: usize) -> Option<Arc<Analysis>> {
        if let Some((rev, ct, cc, a)) = &self.th_cache {
            if *rev == self.rev && *ct == t && *cc == c {
                return a.clone();
            }
        }
        let a = {
            let en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
            analyze(&en, t, c).map(Arc::new)
        };
        self.th_cache = Some((self.rev, t, c, a.clone()));
        a
    }

    /// recompute the style story into the project's metadata when the notes changed since the last time
    pub(super) fn refresh_style(&mut self) {
        if self.style_rev == self.rev {
            return;
        }
        let eng = self.out.engine.clone();
        let (story, fields) = {
            let en = eng.lock().unwrap_or_else(|p| p.into_inner());
            let st = compose::thumbprint(&project_parts(&en), en.project.bpm);
            (st.story, st.fields)
        };
        let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
        for (k, v) in [("style_story", story), ("style_fields", fields)] {
            if let Some(slot) = en.project.meta.iter_mut().find(|(x, _)| x == k) {
                slot.1 = v;
            } else {
                en.project.meta.push((k.to_string(), v));
            }
        }
        self.style_rev = self.rev;
    }

    pub(super) fn theory_dispatch(&mut self, name: &str, a: &A) -> Result<Val, String> {
        let eng = self.out.engine.clone();
        macro_rules! ok {
            ($v:expr) => {
                return Ok(Val::from($v))
            };
        }
        let empty = || Ok(Val::List(vec![]));
        match name {
            "daw_th_summary" => {
                // (t, c) -> [key, tonic, minor, confidence, chords, out of key, tension, passing, monophonic, key changes, chords come from other tracks]
                let Some(an) = self.th(a.u(0), a.u(1)) else { return empty() };
                let count = |r: u8| an.roles.iter().filter(|x| x.0 == r).count() as f64;
                ok!(Val::List(vec![
                    Val::Str(an.main.name()),
                    Val::Num(an.main.tonic as f64),
                    Val::Num(an.main.minor as i32 as f64),
                    Val::Num(an.confidence),
                    Val::Num(an.chords.len() as f64),
                    Val::Num(count(theory::R_OUT)),
                    Val::Num(count(theory::R_TENSION)),
                    Val::Num(count(theory::R_PASS)),
                    Val::Num(an.mono as i32 as f64),
                    Val::Num(an.keys.len().saturating_sub(1) as f64),
                    Val::Num(an.harmony_from_others as i32 as f64),
                ]))
            }
            "daw_th_keys" => {
                let Some(an) = self.th(a.u(0), a.u(1)) else { return empty() };
                ok!(Val::List(an.keys.iter().flat_map(|k| [Val::Num(k.start), Val::Num(k.end.min(an.clip_len.max(4.0))), Val::Num(k.key.tonic as f64), Val::Num(k.key.minor as i32 as f64), Val::Str(k.key.name())]).collect()))
            }
            "daw_th_chords" => {
                // flat [start, len, root, quality, name, roman, diatonic, implied] per chord
                let Some(an) = self.th(a.u(0), a.u(1)) else { return empty() };
                let mut v = vec![];
                for ch in &an.chords {
                    let k = theory::key_at(&an.keys, ch.start).unwrap_or(an.main);
                    v.extend([
                        Val::Num(ch.start),
                        Val::Num(ch.len),
                        Val::Num(ch.root as f64),
                        Val::Num(ch.q as f64),
                        Val::Str(ch.name(k)),
                        Val::Str(theory::roman(ch, k)),
                        Val::Num(theory::chord_diatonic(ch, k) as i32 as f64),
                        Val::Num(ch.implied as i32 as f64),
                    ]);
                }
                ok!(Val::List(v))
            }
            "daw_th_roles" => {
                // per note in clip order: [role 0 chord tone, 1 scale tone, 2 passing, 3 tension, 4 out of key; suggested key or -1]
                let Some(an) = self.th(a.u(0), a.u(1)) else { return empty() };
                ok!(Val::List(an.roles.iter().flat_map(|(r, s)| [Val::Num(*r as f64), Val::Num(*s as f64)]).collect()))
            }
            "daw_th_phrases" => {
                let Some(an) = self.th(a.u(0), a.u(1)) else { return empty() };
                ok!(Val::List(an.phrases.iter().flat_map(|p| [Val::Num(p.start), Val::Num(p.end), Val::Str(p.kind.to_string()), Val::Str(p.label.clone())]).collect()))
            }
            "daw_th_harm_summary" => {
                // (prompt) -> what the prompt asks for, in words
                ok!(crate::harmonize::parse_prompt(&a.s(0)).summary())
            }
            "daw_th_harmonize" => {
                // (t, c, [selected note indices, empty = all], ref track (-1 = every other track), prompt, dest track) -> notes written
                // Harmonises the selected notes of clip (t, c) in the key and chords of the reference track and writes the voices as a new
                // clip on the destination track.
                let (t, c, rt, dst) = (a.u(0), a.u(1), a.n(3), a.u(5));
                let spec = crate::harmonize::parse_prompt(&a.s(4));
                let sel: Vec<usize> = match a.0.get(2) {
                    Some(Val::List(l)) => l.iter().filter_map(|v| if let Val::Num(n) = v { Some(n.max(0.0) as usize) } else { None }).collect(),
                    _ => vec![],
                };
                let Some(an) = self.th(t, c) else { return ok!(0.0) };
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let (cs, clen) = match en.project.tracks.get(t).and_then(|x| x.clips.get(c)) {
                    Some(cl) => (cl.start, cl.len),
                    None => return ok!(0.0),
                };
                // the melody: the chosen notes of the clip (the clip's own note order), relative to the clip start
                let melody: Vec<N> = an.notes.iter().enumerate().filter(|(i, _)| sel.is_empty() || sel.contains(i)).map(|(_, n)| *n).collect();
                if melody.is_empty() || dst >= en.project.tracks.len() {
                    return ok!(0.0);
                }
                // the chords: from the reference track when one is chosen, else from everything else that plays (the analysis already did that)
                let chords: Vec<Chord> = if rt >= 0.0 && rt as usize != t {
                    let mut ref_notes: Vec<N> = vec![];
                    if let Some(tr) = en.project.tracks.get(rt as usize) {
                        for cl in &tr.clips {
                            if cl.end() <= cs || cl.start >= cs + clen {
                                continue;
                            }
                            for n in expand(cl) {
                                if n.end() > cs && n.start < cs + clen {
                                    let start = (n.start - cs).max(0.0);
                                    ref_notes.push(N { start, len: (n.end() - cs - start).max(0.03), ..n });
                                }
                            }
                        }
                    }
                    if ref_notes.len() >= 3 {
                        let poly = mono_score(&ref_notes) >= 1.4;
                        theory::detect_chords(&ref_notes, clen.clamp(4.0, 16384.0), if poly { 2.0 } else { 4.0 }, !poly)
                    } else {
                        an.chords.clone()
                    }
                } else {
                    an.chords.clone()
                };
                let mut out = crate::harmonize::harmonize(&melody, &chords, &an.keys, an.main, &spec);
                if out.is_empty() {
                    return ok!(0.0);
                }
                out.sort_by(|x, y| x.start.partial_cmp(&y.start).unwrap());
                let mut clip = Clip::midi(cs, clen, "Harmony");
                if let ClipData::Midi(ns) = &mut clip.data {
                    for n in &out {
                        ns.push(Note::new(n.start, n.len, n.key, n.vel));
                    }
                }
                let count = out.len();
                en.project.tracks[dst].clips.push(clip);
                drop(en);
                self.touch();
                ok!(count)
            }
            "daw_th_next" => {
                // (t, c, beat) -> chords that work after the one before `beat`: [roman, name, root, quality, score]*
                let Some(an) = self.th(a.u(0), a.u(1)) else { return empty() };
                let beat = a.n(2);
                let prev = an.chords.iter().rev().find(|c| c.start < beat - 0.01);
                let hist: Vec<Chord> = an.chords.iter().filter(|c| c.start < beat - 0.01).cloned().collect();
                let key = theory::key_at(&an.keys, beat).unwrap_or(an.main);
                let cands = theory::next_chords(key, prev, &hist);
                ok!(Val::List(
                    cands
                        .into_iter()
                        .flat_map(|(root, q, rom, s)| {
                            let ch = Chord { start: 0.0, len: 1.0, root, q, bass: root, implied: false };
                            [Val::Str(rom), Val::Str(ch.name(key)), Val::Num(root as f64), Val::Num(q as f64), Val::Num(s)]
                        })
                        .collect()
                ))
            }
            "daw_th_insert_chord" => {
                // (t, c, beat, len, root, quality) -> notes added, voiced close to whatever sounds just before
                let (t, c, beat, len, root, q) = (a.u(0), a.u(1), a.n(2).max(0.0), a.n(3).max(0.25), a.u(4) as u8 % 12, a.u(5).min(theory::QUAL.len() - 1));
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let Some(Clip { data: ClipData::Midi(n), .. }) = en.project.tracks.get_mut(t).and_then(|x| x.clips.get_mut(c)) else { return ok!(0.0) };
                let prev: Vec<u8> = n.iter().filter(|x| x.start < beat - 0.01 && x.start + x.len > beat - 0.05).map(|x| x.key).collect();
                let notes = theory::voice(root, q, &prev, 48, 76);
                for k in &notes {
                    n.push(Note::new(beat, len * 0.97, *k, 0.8));
                }
                drop(en);
                self.touch();
                ok!(notes.len())
            }
            "daw_th_autocorrect" => {
                // (t, c, mask, apply) -> [total, out-of-key, tension, timing, duplicates, passing]; apply = 1 changes the notes
                let (t, c, mask, apply) = (a.u(0), a.u(1), a.u(2) as u32, a.n(3) > 0.5);
                let Some(an) = self.th(t, c) else { return empty() };
                let order = if a.0.len() > 4 { a.n(4).clamp(0.0, 1.0) } else { 0.75 };
                let (fixed, changed, rep) = theory::autocorrect_ctx(&an.notes, 0.0, &an.keys, &an.chords, mask, &theory::FixCtx { order, support: &an.support, repeated: &an.repeated, clash: &an.clash, others: &an.others_sorted });
                if apply && rep.total() > 0 {
                    let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    if let Some(Clip { data: ClipData::Midi(n), .. }) = en.project.tracks.get_mut(t).and_then(|x| x.clips.get_mut(c)) {
                        for (i, f) in fixed.iter().enumerate() {
                            if changed.get(i) == Some(&true) {
                                if let Some(x) = n.get_mut(i) {
                                    x.start = f.start;
                                    x.len = f.len;
                                    x.key = f.key;
                                }
                            }
                        }
                        // duplicates were marked with a zero length
                        let mut idx = 0usize;
                        n.retain(|_| {
                            let keep = fixed.get(idx).map_or(true, |f| f.len > 0.0);
                            idx += 1;
                            keep
                        });
                    }
                    drop(en);
                    self.touch();
                }
                ok!(Val::List(vec![Val::Num(rep.total() as f64), Val::Num(rep.key as f64), Val::Num(rep.tension as f64), Val::Num(rep.timing as f64), Val::Num(rep.dupes as f64), Val::Num(rep.passing as f64)]))
            }
            "daw_th_retune" => {
                // (t, c, tonic, minor) -> notes moved: the clip's melody in a new key, scale degrees kept
                let (t, c) = (a.u(0), a.u(1));
                let Some(an) = self.th(t, c) else { return ok!(0.0) };
                let to = Key::new(a.u(2) as u8 % 12, a.n(3) > 0.5);
                if to == an.main {
                    return ok!(0.0);
                }
                let moved = theory::retune(&an.notes, an.main, to);
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let mut n_changed = 0;
                if let Some(Clip { data: ClipData::Midi(n), .. }) = en.project.tracks.get_mut(t).and_then(|x| x.clips.get_mut(c)) {
                    for (i, m) in moved.iter().enumerate() {
                        if let Some(x) = n.get_mut(i) {
                            if x.key != m.key {
                                x.key = m.key;
                                n_changed += 1;
                            }
                        }
                    }
                }
                drop(en);
                self.touch();
                ok!(n_changed)
            }
            "daw_th_vocab" => ok!(format!("styles: {}. kinds: melody, chords, bass, arp, pad. contours: arch, rise, fall, wave, flat. feels: straight, syncopated, offbeat, swing, driving, staccato, sustained. densities: sparse, medium, busy.", compose::style_names().join(", "))),
            "daw_th_spec" => {
                // (prompt, model reply, style fields, t, c) -> normalised spec text. Priority: the prompt's own words, then the model, then the style.
                let kw = compose::parse_fields(&a.s(0));
                let mut js = compose::fields_from_json(&a.s(1));
                // a small model likes to invent a key and a progression; those come from the request or from the project, not from it
                let lower = a.s(0).to_lowercase();
                js.retain(|(k, _)| match k.as_str() {
                    "key" => lower.contains("key"),
                    "prog" => lower.contains("progression") || lower.contains("chords"),
                    _ => true,
                });
                let style = a.s(2);
                let (t, c) = (a.u(3), a.u(4));
                let (key, _) = match self.th(t, c) {
                    Some(an) => (an.main, an.mono),
                    None => (Key::new(0, true), true),
                };
                let kind0 = Spec::from_fields(&[&js, &kw], key, Kind::Melody).kind;
                let mut layers: Vec<Vec<(String, String)>> = vec![];
                if !style.is_empty() {
                    // a style carries the feel of a project, not its key or its chords: those come from this one
                    let sf: Vec<(String, String)> = compose::style_fields_for(&style, kind0).into_iter().filter(|(k, _)| k != "key" && k != "prog" && k != "cpb").collect();
                    layers.push(sf);
                }
                layers.push(js);
                layers.push(kw);
                let refs: Vec<&[(String, String)]> = layers.iter().map(|l| &l[..]).collect();
                let spec = Spec::from_fields(&refs, key, kind0);
                let mut text = spec.to_fields();
                if spec.prog.is_empty() {
                    text += ";harmony=project";
                }
                ok!(text)
            }
            "daw_th_spec_summary" => {
                let s = Spec::from_text(&a.s(0), Key::new(0, true), Kind::Melody);
                let mut line = s.summary();
                if a.s(0).contains("harmony=project") {
                    if !s.prog_text.is_empty() {
                        line = line.replace(&format!(" | {}", s.prog_text), "");
                    }
                    line += " | over the project's chords";
                }
                ok!(line)
            }
            "daw_th_generate" => {
                // (t, c, spec text, start beat, mode) -> notes written. mode 0 replaces the clip's notes, 1 adds at `start`.
                let (t, c) = (a.u(0), a.u(1));
                let text = a.s(2);
                let start = a.n(3).max(0.0);
                let key0 = self.th(t, c).map(|an| an.main).unwrap_or(Key::new(0, true));
                let mut spec = Spec::from_text(&text, key0, Kind::Melody);
                // follow the chords already in the project when the request names none
                if spec.prog.is_empty() && text.contains("harmony=project") {
                    if let Some(an) = self.th(t, c) {
                        let end = start + spec.bars as f64 * 4.0;
                        let mut h: Vec<(f64, f64, u8, usize)> = an.chords.iter().filter(|ch| ch.end() > start && ch.start < end).map(|ch| ((ch.start - start).max(0.0), (ch.end().min(end) - ch.start.max(start)).max(0.25), ch.root, ch.q)).collect();
                        h.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
                        if h.len() >= 2 {
                            // make the plan continuous: each chord lasts until the next one, the first one starts at 0
                            let n = h.len();
                            for i in 0..n {
                                let next = if i + 1 < n { h[i + 1].0 } else { spec.bars as f64 * 4.0 };
                                if i == 0 {
                                    h[i].0 = 0.0;
                                }
                                h[i].1 = (next - h[i].0).max(0.25);
                            }
                            spec.harmony = h;
                        }
                    }
                }
                let notes = compose::generate(&spec);
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let Some(Clip { data: ClipData::Midi(dst), len, .. }) = en.project.tracks.get_mut(t).and_then(|x| x.clips.get_mut(c)) else { return ok!(0.0) };
                if a.n(4) < 0.5 {
                    dst.clear();
                } else if a.n(4) > 1.5 {
                    // replace only the stretch being written: the notes before and after it stay
                    let end = start + spec.bars as f64 * 4.0;
                    dst.retain(|n| n.start < start - 1e-6 || n.start >= end - 1e-6);
                }
                let before = dst.len();
                for n in &notes {
                    dst.push(Note::new(n.start + start, n.len, n.key, n.vel));
                }
                let need = start + spec.bars as f64 * 4.0;
                if *len < need {
                    *len = need;
                }
                let added = dst.len() - before;
                drop(en);
                self.touch();
                ok!(added)
            }
            // ── style thumbprint ──
            "daw_style_thumbprint" => {
                // () -> the project as a story; stored in the project's metadata (and refreshed only when the notes changed)
                self.refresh_style();
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                ok!(en.project.meta.iter().find(|(k, _)| k == "style_story").map(|(_, v)| v.clone()).unwrap_or_default())
            }
            "daw_style_fields" => {
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                ok!(en.project.meta.iter().find(|(k, _)| k == "style_fields").map(|(_, v)| v.clone()).unwrap_or_default())
            }
            "daw_style_from_file" => {
                // path -> [story, fields] of a MIDI / FL file
                let Some((parts, bpm)) = file_parts(&a.s(0)) else { return empty() };
                let st = compose::thumbprint(&parts, bpm);
                ok!(Val::List(vec![Val::Str(st.story), Val::Str(st.fields)]))
            }
            "daw_style_edit" => {
                // (story text) -> fields parsed back out of an edited story, merged over the stored ones
                let mut f = compose::split_fields(&{
                    let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    en.project.meta.iter().find(|(k, _)| k == "style_fields").map(|(_, v)| v.clone()).unwrap_or_default()
                });
                for (k, v) in compose::parse_fields(&a.s(0)) {
                    let key = match k.as_str() {
                        "density" => "lead.density".to_string(),
                        "contour" => "lead.contour".to_string(),
                        _ => k,
                    };
                    f.retain(|(x, _)| *x != key);
                    f.push((key, v));
                }
                ok!(f.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(";"))
            }
            // ── library ──
            "daw_lib_scan" => {
                // dir -> flat [path, name, key, bpm, bars, progression, story, fields] per MIDI / FL file, cached next to the files
                let dir = a.s(0);
                let mut files: Vec<std::path::PathBuf> = vec![];
                fn walk(d: &std::path::Path, out: &mut Vec<std::path::PathBuf>, depth: usize) {
                    let Ok(rd) = std::fs::read_dir(d) else { return };
                    for e in rd.flatten() {
                        let p = e.path();
                        if p.is_dir() {
                            if depth < 4 && !p.file_name().map_or(false, |n| n.to_string_lossy().starts_with('.')) {
                                walk(&p, out, depth + 1);
                            }
                        } else if matches!(p.extension().map(|x| x.to_string_lossy().to_lowercase()).as_deref(), Some("mid" | "midi" | "flp")) {
                            out.push(p);
                        }
                    }
                }
                walk(std::path::Path::new(&dir), &mut files, 0);
                files.sort();
                let cache_dir = std::path::Path::new(&dir).join(".theory");
                let mut v = vec![];
                for f in files.into_iter().take(2000) {
                    let meta = std::fs::metadata(&f).ok();
                    let sig = format!("{}-{}", meta.as_ref().map(|m| m.len()).unwrap_or(0), meta.and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0));
                    let stem = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    let cache = cache_dir.join(format!("{}_{:x}.txt", stem.replace(|c: char| !c.is_alphanumeric(), "_"), sig.bytes().fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(b as u64))));
                    let (story, fields, bars) = match std::fs::read_to_string(&cache).ok().and_then(|s| {
                        let mut l = s.splitn(3, '\n');
                        Some((l.next()?.to_string(), l.next()?.to_string(), l.next()?.trim().parse::<f64>().ok()?))
                    }) {
                        Some(x) => x,
                        None => {
                            let Some((parts, bpm)) = file_parts(&f.to_string_lossy()) else { continue };
                            let total = parts.iter().flat_map(|p| p.notes.iter()).map(|n| n.end()).fold(0.0, f64::max);
                            let st = compose::thumbprint(&parts, bpm);
                            let _ = std::fs::create_dir_all(&cache_dir);
                            let _ = std::fs::write(&cache, format!("{}\n{}\n{}", st.story, st.fields, (total / 4.0).ceil()));
                            (st.story, st.fields, (total / 4.0).ceil())
                        }
                    };
                    v.extend([Val::Str(f.to_string_lossy().replace('\\', "/")), Val::Str(stem), Val::Str(field(&fields, "key").to_string()), Val::Str(field(&fields, "bpm").to_string()), Val::Num(bars), Val::Str(field(&fields, "prog").to_string()), Val::Str(story), Val::Str(fields)]);
                }
                ok!(Val::List(v))
            }
            "daw_roll_arp" => {
                // (t, c, [note indices] (empty = every note), rate in beats, order, octaves, gate, rhythm, seed) -> notes now in the clip from the chords
                let (t, c) = (a.u(0), a.u(1));
                let want: Vec<usize> = match a.0.get(2) {
                    Some(Val::List(l)) => l.iter().filter_map(|v| if let Val::Num(x) = v { Some(*x as usize) } else { None }).collect(),
                    _ => vec![],
                };
                let rate = compose::parse_rate(&a.s(3)).max(0.0625);
                let rate = if rate <= 0.0625 && a.n(3) > 0.0 { a.n(3).clamp(0.0625, 4.0) } else { rate };
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let Some(Clip { data: ClipData::Midi(dst), .. }) = en.project.tracks.get_mut(t).and_then(|x| x.clips.get_mut(c)) else { return ok!(0.0) };
                let picked: Vec<usize> = if want.is_empty() { (0..dst.len()).collect() } else { want.into_iter().filter(|i| *i < dst.len()).collect() };
                let src: Vec<N> = picked.iter().map(|i| N { start: dst[*i].start, len: dst[*i].len, key: dst[*i].key, vel: dst[*i].vel }).collect();
                let out = compose::arpeggiate(&src, rate, &a.s(4).to_lowercase(), a.u(5).clamp(1, 4) as u32, a.n(6).clamp(0.05, 1.5), &a.s(7), a.u(8) as u64);
                let mut drop_idx = picked.clone();
                drop_idx.sort_unstable();
                drop_idx.dedup();
                for i in drop_idx.into_iter().rev() {
                    dst.remove(i);
                }
                for n in &out {
                    dst.push(Note::new(n.start, n.len, n.key, n.vel));
                }
                let made = out.len();
                drop(en);
                self.touch();
                ok!(made)
            }
            // ── derive: describe a sound's parameters, apply a model's variation of them ──
            "daw_derive_prompt" => {
                // (t, seed) -> a JSON object of up to 24 of the sound's parameters at their current values: the model is asked to return it
                // with new values (a small model copies a given shape reliably; a different handful is picked every time)
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let Some(i) = en.project.tracks.get(a.u(0)).and_then(|t| t.inst.as_ref()) else { return ok!(String::new()) };
                let picked = derive_pick(i.as_ref(), a.u(1) as u64);
                ok!(format!("{} parameters (each 0 to 1): {}", derive_label(i.as_ref()), derive_json(&picked)))
            }
            "daw_derive_example" => {
                // (t, seed) -> one worked question and answer in the history format, made from THIS instrument's own parameter names so the
                // model copies those names (a fixed synth example made it answer with `waveform` / `filter_cutoff` for a drum part)
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let Some(i) = en.project.tracks.get(a.u(0)).and_then(|t| t.inst.as_ref()) else { return ok!(String::new()) };
                let picked = derive_pick(i.as_ref(), a.u(1) as u64 + 101);
                const MOVES: [f32; 6] = [0.18, -0.15, 0.22, -0.12, 0.14, -0.2];
                let changed: Vec<(String, f32)> = picked
                    .iter()
                    .enumerate()
                    .map(|(k, (n, v))| {
                        let d = MOVES[k % MOVES.len()];
                        let nv = if (v + d).clamp(0.0, 1.0) == *v { v - d } else { v + d };
                        (n.clone(), nv.clamp(0.0, 1.0))
                    })
                    .collect();
                ok!(format!(
                    "<|user|>\n{} parameters (each 0 to 1): {}\nRequest: a similar but different sound\n<|assistant|>\n{}\n",
                    derive_label(i.as_ref()),
                    derive_json(&picked),
                    derive_json(&changed)
                ))
            }
            "daw_derive_apply" => {
                // (t, model reply, amount 0..1) -> parameters changed; names the instrument does not have are ignored
                let t = a.u(0);
                let amount = a.n(2).clamp(0.0, 4.0);
                let fields = compose::fields_from_json(&a.s(1));
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let Some(i) = en.project.tracks.get_mut(t).and_then(|t| t.inst.as_mut()) else { return ok!(0.0) };
                let params = i.params();
                let mut changed = 0usize;
                for (k, v) in fields {
                    let Ok(target) = v.trim().parse::<f64>() else { continue };
                    let key = k.replace('_', " ");
                    let Some((name, old)) = params.iter().find(|(n, _)| n.to_lowercase() == key || n.to_lowercase().replace(' ', "_") == k) else { continue };
                    if !derive_core(name) || !derive_musical(name) || !target.is_finite() {
                        continue;
                    }
                    let nv = (*old as f64 + (target.clamp(0.0, 1.0) - *old as f64) * amount).clamp(0.0, 1.0) as f32;
                    if (nv - old).abs() > 0.002 {
                        i.set_param(name, nv);
                        changed += 1;
                    }
                    if changed >= 48 {
                        break;
                    }
                }
                drop(en);
                self.touch();
                ok!(changed)
            }
            "daw_derive_local" => {
                // (t, seed, amount) -> a similar but different sound without a model: about a third of the parameters drift
                let t = a.u(0);
                let amount = a.n(2).clamp(0.0, 1.0);
                let mut rng = compose::Rng::new(a.u(1) as u64 + 7);
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let Some(i) = en.project.tracks.get_mut(t).and_then(|t| t.inst.as_mut()) else { return ok!(0.0) };
                let params = i.params();
                let mut changed = 0usize;
                for (name, old) in params {
                    if !derive_core(name) || !derive_musical(name) || !rng.chance(0.35) {
                        continue;
                    }
                    let nv = (old as f64 + (rng.f() - 0.5) * 2.0 * amount * 0.3).clamp(0.0, 1.0) as f32;
                    if (nv - old).abs() > 0.002 {
                        i.set_param(name, nv);
                        changed += 1;
                    }
                }
                drop(en);
                self.touch();
                ok!(changed)
            }
            _ => Err(format!("unknown builtin {name}")),
        }
    }
}

#[allow(dead_code)]
fn _unused() {
    let _ = qual_of;
}
