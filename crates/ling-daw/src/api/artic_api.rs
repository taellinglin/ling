//! Builtins for "Articulate": generate automation lanes for all, random or one track's parameters, optionally steered by a prompt.
//!
//! `daw_artic_begin(mode, track, count, seed, amount, prompt, phrase)` collects the candidate parameters (never switches, levels or
//! sequence steps; never a lane the user drew) and splits them into chunks of up to ten for one track each. For every chunk
//! `daw_artic_prompt(i)` is the question for the model and `daw_artic_apply(i, reply)` writes the lanes: the model's picks where it gave
//! usable ones, a local choice for everything else, so an empty or broken reply still produces a full result.
//!
//! **The song's form.** Every lane is laid out section by section (articulate.rs, `section_move` / `render_form`): the sections come
//! from the ruler tags (project meta "sections"), else from clip names that are section tags (what the song build writes), else
//! they are read from the music (`feel::infer_sections`, energy over time). The genre ("ai.genre") and the track's part (bass, pad,
//! vocal ...) decide what may move where. The model's / the words' choice plays only in the choruses and drops, where it fits.
//!
//! **Mode "song"** (Edit > Re-automate All, and the AI song build): every track with clips (`track` < 0) or one track gets at most
//! three lanes, one per job its part needs (filter, send, drive / width), replacing the lanes this generator wrote before.
//!
//! **Which lanes are ours.** A lane has no origin flag in the project format, so every lane the generator writes is registered by a
//! fingerprint of its points in project meta "artic.owned" (`param#hash`). A lane whose points still match is the generator's and may
//! be replaced; a lane the user drew - or a generated one the user has since edited - no longer matches and is never touched.
use super::{Daw, A, Val};
use crate::articulate::{self as art, Intent, Part, Role, SecCtx, Slot, Style};
use crate::compose::Rng;
use crate::model::*;

pub struct Cand {
    track: usize,
    /// the lane's `param`: an instrument parameter name, "pan" or "fx<i>.<p>"
    key: String,
    label: String,
    /// the parameter's own name ("cut", "mix")
    name: String,
    role: Role,
    cur: f32,
    part: Part,
    slot: Slot,
    /// the first parameter of its slot on the track: moves in every section (the rest only where the music is at full energy)
    primary: bool,
}

pub struct Job {
    chunks: Vec<Vec<Cand>>,
    prompt: String,
    intent: Intent,
    amount: f32,
    seed: u64,
    written: usize,
    kept: usize,
    tracks: Vec<usize>,
    /// bars per phrase, 0 = read from the music
    phrase: f64,
    /// the form of the last track written (8-bar phrases: A A B A)
    form: String,
    /// mode "song": capped lanes per part, the generator's old lanes replaced
    song: bool,
    /// the song's sections and where they came from ("ruler tags", "clip names", "the music")
    secs: Vec<SecCtx>,
    src: &'static str,
    genre: String,
    style: Style,
    /// tracks that got at least one lane
    wrote_tracks: Vec<usize>,
}

const CHUNK: usize = 10;
const PER_TRACK: usize = 40;
pub const OWNED: &str = "artic.owned";

/// a fingerprint of a lane: its parameter and its points (rounded, so a save / load round trip keeps it)
pub(crate) fn lane_sig(l: &AutoLane) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |b: &[u8]| {
        for x in b {
            h ^= *x as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    eat(l.param.as_bytes());
    for p in &l.points {
        eat(&(((p.0 * 1000.0).round()) as i64).to_le_bytes());
        eat(&(((p.1 as f64 * 10000.0).round()) as i64).to_le_bytes());
    }
    format!("{}#{:016x}", l.param, h)
}

/// the fingerprints of every lane the generator wrote and nobody has touched since
pub(crate) fn owned_sigs(p: &Project) -> std::collections::HashSet<String> {
    p.meta.iter().find(|(k, _)| k == OWNED).map(|(_, v)| v.split(';').filter(|x| !x.is_empty()).map(|x| x.to_string()).collect()).unwrap_or_default()
}

/// Register lanes as the generator's (`new` = (track, param)), and forget fingerprints that no longer match any lane.
pub(crate) fn own_lanes(p: &mut Project, new: &[(usize, String)]) {
    let mut set = owned_sigs(p);
    let now: std::collections::HashSet<String> = p.tracks.iter().flat_map(|t| t.auto.iter().map(lane_sig)).collect();
    set.retain(|s| now.contains(s));
    for (ti, key) in new {
        if let Some(l) = p.tracks.get(*ti).and_then(|t| t.auto.iter().find(|l| l.param == *key)) {
            set.insert(lane_sig(l));
        }
    }
    let mut v: Vec<String> = set.into_iter().collect();
    v.sort();
    p.meta.retain(|(k, _)| k != OWNED);
    if !v.is_empty() {
        p.meta.push((OWNED.to_string(), v.join(";")));
    }
}

/// the parameter name behind a lane key ("cut", "pan", "fx2.1" -> that effect's parameter), None when it does not exist
fn key_name(t: &Track, key: &str) -> Option<String> {
    if key == "pan" {
        return Some("pan".into());
    }
    if let Some(rest) = key.strip_prefix("fx") {
        let (f, p) = rest.split_once('.')?;
        let (f, p) = (f.parse::<usize>().ok()?, p.parse::<usize>().ok()?);
        return t.effects.get(f).and_then(|e| e.params.get(p)).map(|x| x.0.to_string());
    }
    t.inst.as_ref().and_then(|i| i.params().into_iter().find(|(n, _)| *n == key).map(|(n, _)| n.to_string()))
}

/// Register every lane of these tracks on an automatable parameter (what a song build wrote) as the generator's.
pub(crate) fn own_all(p: &mut Project, tracks: &[usize]) {
    let mut new = vec![];
    for &ti in tracks {
        let Some(t) = p.tracks.get(ti) else { continue };
        for l in &t.auto {
            if l.points.len() >= 2 && key_name(t, &l.param).map(|n| art::eligible(&n)).unwrap_or(false) {
                new.push((ti, l.param.clone()));
            }
        }
    }
    own_lanes(p, &new);
}

fn lane_has_points(t: &Track, key: &str) -> bool {
    t.auto.iter().any(|l| l.param == key && l.points.len() >= 2)
}

/// every parameter of a track that may be automated; a lane already drawn is left alone (counted in `kept`) unless the generator
/// wrote it (`owned`)
fn candidates(t: &Track, ti: usize, kept: &mut usize, owned: &std::collections::HashSet<String>) -> Vec<Cand> {
    let mut out = vec![];
    let mut push = |key: String, label: String, name: &str, cur: f32, kept: &mut usize| {
        if !art::eligible(name) {
            return;
        }
        if lane_has_points(t, &key) && !t.auto.iter().any(|l| l.param == key && owned.contains(&lane_sig(l))) {
            *kept += 1;
            return;
        }
        let role = art::role_of(name);
        out.push(Cand { track: ti, slot: art::slot_of(&key, name, role), key, label, name: name.to_string(), role, cur, part: Part::Other, primary: false });
    };
    if let Some(i) = t.inst.as_ref() {
        for (name, v) in i.params() {
            push(name.to_string(), name.to_string(), name, v, kept);
        }
    }
    for (fi, slot) in t.effects.iter().enumerate() {
        if slot.bypass {
            continue;
        }
        for (pi, (name, v)) in slot.params.iter().enumerate() {
            push(format!("fx{fi}.{pi}"), format!("{} {}", slot.name, name), name, *v, kept);
        }
    }
    push("pan".to_string(), "pan".to_string(), "pan", ((t.pan + 1.0) * 0.5).clamp(0.0, 1.0), kept);
    out
}

/// what a track plays: its words (name, instrument, patch), else its notes (mean key, chords, note lengths)
fn track_part(t: &Track) -> Part {
    let words = format!("{} {} {}", t.name, t.inst.as_ref().map(|i| i.name().to_string()).unwrap_or_default(), t.patch);
    let (mut keys, mut n, mut len) = (0.0f64, 0usize, 0.0f64);
    let mut onsets: Vec<f64> = vec![];
    for c in &t.clips {
        if let ClipData::Midi(ns) = &c.data {
            for x in ns {
                keys += x.key as f64;
                len += x.len;
                n += 1;
                onsets.push(c.start + x.start);
            }
        }
    }
    onsets.sort_by(f64::total_cmp);
    let distinct = {
        let mut d = onsets.clone();
        d.dedup_by(|b, a| (*b - *a).abs() < 0.06);
        d.len()
    };
    let chordy = if n == 0 { 0.0 } else { (1.0 - distinct as f32 / n as f32) * 2.0 };
    let (mk, ml) = if n == 0 { (0.0, 0.0) } else { (keys / n as f64, len / n as f64) };
    art::part_of(&words, t.inst_kind == "drums", mk, chordy.clamp(0.0, 1.0), ml)
}

/// The song's sections with their energies, and where they came from: the ruler tags, the clip names, or the music itself.
pub(crate) fn song_form(en: &crate::engine::Engine) -> (Vec<SecCtx>, &'static str) {
    let p = &en.project;
    let end = p.end_beat().max(16.0);
    let mut spans: Vec<(f64, f64, String)> = vec![];
    let mut src = "the ruler tags";
    if let Some((_, text)) = p.meta.iter().find(|(k, v)| k == "sections" && !v.trim().is_empty()) {
        for item in text.split(';') {
            let f: Vec<&str> = item.split(':').collect();
            if f.len() >= 3 {
                if let (Ok(a), Ok(b)) = (f[0].trim().parse::<f64>(), f[1].trim().parse::<f64>()) {
                    if b > a {
                        spans.push((a, b, f[2].trim().to_string()));
                    }
                }
            }
        }
    }
    if spans.len() < 2 {
        // the clip names a song build or a medley writes ("intro", "verse 2", "drop")
        spans.clear();
        src = "the clip names";
        for t in &p.tracks {
            for c in &t.clips {
                if art::is_section_name(&c.name) && !spans.iter().any(|s| (s.0 - c.start).abs() < 1.0) {
                    spans.push((c.start, c.end(), c.name.clone()));
                }
            }
        }
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        if spans.len() < 2 {
            spans.clear();
        }
    }
    let inferred = spans.is_empty();
    if inferred {
        src = "the music";
        spans = crate::feel::infer_sections(&p.tracks, end).into_iter().map(|s| (s.start, s.end, s.tag)).collect();
        // one stretch of music with no form: treat it as the main part, a chorus
        if spans.len() == 1 {
            spans[0].2 = "chorus".into();
        }
    }
    let fs: Vec<crate::feel::Sec> = spans.iter().map(|s| crate::feel::Sec { start: s.0, end: s.1, tag: s.2.clone() }).collect();
    let measured: Vec<f32> = crate::feel::section_energy(&p.tracks, &fs).into_iter().map(|x| x as f32).collect();
    (art::form_of(&spans, &measured), src)
}

/// where a track's automation goes: the loop when one is set, else the stretch its clips cover, else the song; always whole bars
fn track_range(en: &crate::engine::Engine, ti: usize) -> (f64, f64) {
    let p = &en.project;
    let (mut b0, mut b1) = if p.loop_on && p.loop_end > p.loop_start + 1.0 {
        (p.loop_start, p.loop_end)
    } else {
        match p.tracks.get(ti) {
            Some(t) if !t.clips.is_empty() => (t.clips.iter().map(|c| c.start).fold(f64::MAX, f64::min), t.clips.iter().map(|c| c.end()).fold(0.0, f64::max)),
            _ => (0.0, p.end_beat()),
        }
    };
    if !(b0.is_finite() && b1.is_finite()) || b1 <= b0 {
        b0 = 0.0;
        b1 = 32.0;
    }
    b0 = (b0 / 4.0).floor() * 4.0;
    b1 = (b1 / 4.0).ceil() * 4.0;
    if b1 - b0 < 8.0 {
        b1 = b0 + 8.0;
    }
    (b0.max(0.0), b1.min(b0.max(0.0) + 2048.0))
}

/// onset and length of every note of a track in song beats (looped clips repeated)
fn track_notes(t: &Track) -> Vec<(f64, f64)> {
    let mut v: Vec<(f64, f64)> = vec![];
    for c in &t.clips {
        let ClipData::Midi(notes) = &c.data else { continue };
        let reps = if c.loop_len > 0.1 { ((c.len / c.loop_len).ceil() as usize).clamp(1, 512) } else { 1 };
        for r in 0..reps {
            let base = c.start + r as f64 * c.loop_len;
            for n in notes {
                if c.loop_len > 0.1 && n.start >= c.loop_len {
                    continue;
                }
                let on = base + n.start;
                if on < c.end() {
                    v.push((on, n.len));
                }
            }
        }
    }
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    // a chord is one onset
    v.dedup_by(|b, a| {
        let same = (b.0 - a.0).abs() < 0.06;
        if same {
            a.1 = a.1.max(b.1);
        }
        same
    });
    v
}

impl Daw {
    pub(super) fn artic_dispatch(&mut self, name: &str, a: &A) -> Result<Val, String> {
        let eng = self.out.engine.clone();
        macro_rules! ok {
            ($v:expr) => {
                return Ok(Val::from($v))
            };
        }
        match name {
            "daw_artic_begin" => {
                // (mode "single" | "random" | "all" | "song", track (song: < 0 = every track with clips), count, seed, amount 0..1,
                // prompt, phrase bars) -> chunks to ask about (0 = nothing to do)
                let mode = a.s(0);
                let song = mode == "song";
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let owned = owned_sigs(&en.project);
                let (secs, src) = song_form(&en);
                let genre = super::recipe_api::project_genre(&en);
                let style = art::style_of(&genre, secs.iter().any(|s| s.kind == art::SecKind::Drop));
                let mut kept = 0usize;
                let skip_drums = mode == "random" || mode == "all";
                let ntr = en.project.tracks.len();
                let which: Vec<usize> = if song && a.n(1) < 0.0 {
                    (0..ntr).filter(|t| !en.project.tracks[*t].clips.is_empty()).collect()
                } else if mode == "single" || song {
                    vec![a.u(1)].into_iter().filter(|t| *t < ntr).collect()
                } else {
                    (0..ntr).collect()
                };
                let mut per: Vec<(usize, Vec<Cand>)> = vec![];
                let mut cleared: Vec<(usize, String)> = vec![];
                for ti in which {
                    let t = &en.project.tracks[ti];
                    if skip_drums && t.inst_kind == "drums" {
                        continue;
                    }
                    let part = track_part(t);
                    let mut c = candidates(t, ti, &mut kept, &owned);
                    for x in c.iter_mut() {
                        x.part = part;
                    }
                    if song {
                        // one parameter per job the part needs; the generator's other old lanes on this track go
                        let params: Vec<(String, String, Role)> = c.iter().map(|x| (x.key.clone(), x.name.clone(), x.role)).collect();
                        let pick = art::song_lanes(part, &params);
                        for x in c.iter() {
                            if !pick.iter().any(|p| params[p.0].0 == x.key) && t.auto.iter().any(|l| l.param == x.key) {
                                cleared.push((ti, x.key.clone()));
                            }
                        }
                        let mut chosen: Vec<Cand> = vec![];
                        for (i, slot) in pick {
                            let mut x = std::mem::replace(&mut c[i], Cand { track: ti, key: String::new(), label: String::new(), name: String::new(), role: Role::Other, cur: 0.0, part, slot: Slot::Extra, primary: false });
                            x.slot = slot;
                            x.primary = true;
                            chosen.push(x);
                        }
                        c = chosen;
                    } else if mode != "random" {
                        // the parameters that shape a sound most come first when the cap bites; the effects keep up to 12 places so a
                        // big instrument does not crowd them out
                        c.sort_by_key(|x| art::priority(x.role));
                        let (mut fx, mut inst): (Vec<Cand>, Vec<Cand>) = c.into_iter().partition(|x| x.key.starts_with("fx"));
                        fx.truncate(12);
                        inst.truncate(PER_TRACK.saturating_sub(fx.len()));
                        inst.extend(fx);
                        c = inst;
                    }
                    if !song {
                        // the first parameter of each job on the track moves through the whole form; the rest only at full energy
                        let mut seen: Vec<Slot> = vec![];
                        for x in c.iter_mut() {
                            if !seen.contains(&x.slot) {
                                seen.push(x.slot);
                                x.primary = true;
                            }
                        }
                    }
                    if !c.is_empty() {
                        per.push((ti, c));
                    }
                }
                // the generator's own lanes that the new layout does not use any more
                for (ti, key) in &cleared {
                    if let Some(t) = en.project.tracks.get_mut(*ti) {
                        t.auto.retain(|l| !(l.param == *key && owned.contains(&lane_sig(l))));
                    }
                }
                let seed = a.n(3).abs() as u64 + 11;
                let mut rng = Rng::new(seed);
                if mode == "random" {
                    // a random handful across every track
                    let mut pool: Vec<Cand> = per.into_iter().flat_map(|(_, c)| c).collect();
                    for k in (1..pool.len()).rev() {
                        let j = (rng.next() % (k as u64 + 1)) as usize;
                        pool.swap(k, j);
                    }
                    pool.truncate(a.u(2).clamp(1, 64));
                    pool.sort_by_key(|c| c.track);
                    per = vec![];
                    for c in pool {
                        match per.last_mut() {
                            Some((t, v)) if *t == c.track => v.push(c),
                            _ => per.push((c.track, vec![c])),
                        }
                    }
                }
                let mut chunks: Vec<Vec<Cand>> = vec![];
                let mut tracks = vec![];
                for (ti, c) in per {
                    tracks.push(ti);
                    let mut it = c.into_iter().peekable();
                    while it.peek().is_some() {
                        chunks.push(it.by_ref().take(CHUNK).collect());
                    }
                }
                let prompt = a.s(5);
                let n = chunks.len();
                if !cleared.is_empty() {
                    own_lanes(&mut en.project, &[]);
                }
                drop(en);
                // (7th) phrase length in bars: 4 .. 64, 0 = read from the music
                let phrase = a.n(6).clamp(0.0, 64.0);
                self.artic = Some(Job { chunks, intent: art::read_prompt(&prompt), prompt, amount: a.n(4).clamp(0.0, 1.0) as f32, seed, written: 0, kept, tracks, phrase, form: String::new(), song, secs, src, genre, style, wrote_tracks: vec![] });
                if !cleared.is_empty() {
                    self.touch();
                }
                ok!(n)
            }
            "daw_artic_prompt" => {
                // chunk -> the history for the model: a worked example, then this track's parameters and the song's form
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let Some(job) = self.artic.as_ref() else { return ok!(String::new()) };
                let Some(chunk) = job.chunks.get(a.u(0)) else { return ok!(String::new()) };
                let ti = chunk[0].track;
                let Some(t) = en.project.tracks.get(ti) else { return ok!(String::new()) };
                let label = match t.inst.as_ref() {
                    Some(i) => format!("{} ({})", t.name, i.name()),
                    None => t.name.clone(),
                };
                let (b0, b1) = track_range(&en, ti);
                let secs = art::secs_in(&job.secs, b0, b1);
                let form = if secs.len() >= 2 { art::form_prompt(&secs, &job.genre, chunk[0].part) } else { String::new() };
                let params: Vec<(String, Role, f32)> = chunk.iter().map(|c| (c.label.clone(), c.role, c.cur)).collect();
                ok!(format!("{}<|user|>\n{}\n", art::example(), art::user_prompt(&label, en.project.bpm, (b1 - b0) / 4.0, &job.prompt, &form, &params)))
            }
            "daw_artic_system" => ok!(art::system()),
            "daw_artic_apply" => {
                // (chunk, model reply) -> lanes written. An empty reply writes everything from the local chooser.
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let Some(job) = self.artic.as_mut() else { return ok!(0.0) };
                let Some(chunk) = job.chunks.get(a.u(0)) else { return ok!(0.0) };
                let ti = chunk[0].track;
                let picks = art::parse_reply(&a.s(1));
                let (b0, b1) = track_range(&en, ti);
                let notes = en.project.tracks.get(ti).map(track_notes).unwrap_or_default();
                // the rest of the song's onsets, drums first: what a lane locks to when its own track has no notes
                let mut groove: Vec<f64> = vec![];
                for (oi, ot) in en.project.tracks.iter().enumerate() {
                    if oi != ti && ot.inst_kind == "drums" {
                        groove.extend(track_notes(ot).into_iter().map(|n| n.0));
                    }
                }
                if groove.len() < 8 {
                    for (oi, ot) in en.project.tracks.iter().enumerate() {
                        if oi != ti && ot.inst_kind != "drums" {
                            groove.extend(track_notes(ot).into_iter().map(|n| n.0));
                        }
                    }
                }
                groove.sort_by(f64::total_cmp);
                job.form = art::form_text(&art::phrase_map(b0, b1, &notes, job.phrase));
                // the sections this track spans: with fewer than two (a loop, a short clip) the lane is one gesture as before
                let secs = art::secs_in(&job.secs, b0, b1);
                let sectioned = job.song || secs.len() >= 2;
                let mut wrote = 0usize;
                let mut written_keys: Vec<(usize, String)> = vec![];
                // a small model tends to repeat one answer for every parameter: after the second copy of the same pick the rest are chosen locally
                let sig = |p: &art::Pick| format!("{}|{:?}|{:?}|{:?}", p.pattern.to_lowercase(), p.rate, p.lo.map(|x| (x * 20.0) as i32), p.hi.map(|x| (x * 20.0) as i32));
                let mut seen: std::collections::HashMap<String, usize> = Default::default();
                let usable: Vec<Option<&art::Pick>> = (0..chunk.len())
                    .map(|k| {
                        let p = picks.iter().find(|p| p.p == k)?;
                        let c = seen.entry(sig(p)).or_insert(0);
                        *c += 1;
                        if *c > 2 {
                            None
                        } else {
                            Some(p)
                        }
                    })
                    .collect();
                let has_reply = !a.s(1).trim().is_empty();
                for (k, c) in chunk.iter().enumerate() {
                    let mut rng = Rng::new(job.seed ^ ((a.u(0) as u64) << 20) ^ ((k as u64 + 1) * 0x9E37));
                    let pick = usable[k];
                    let spec = art::spec_for(c.role, c.cur, job.amount, &job.intent, pick, &mut rng, !notes.is_empty());
                    let lseed = (job.seed as u32).wrapping_add((ti * 131 + k) as u32);
                    let ctx = art::Ctx { b0, b1, phrase: job.phrase, notes: &notes, groove: &groove, seed: lseed, fills: job.intent.fills };
                    let mut pts = if sectioned {
                        // song mode keeps the section defaults unless a model answered or the words asked for something
                        let main = if !job.song || pick.is_some() || has_reply || !job.intent.bias.is_empty() { Some(&spec) } else { None };
                        let lc = art::LaneCtx { slot: c.slot, role: c.role, part: c.part, cur: c.cur, amount: if job.intent.amount > 0.0 { job.intent.amount } else { job.amount }, style: job.style, has_notes: !notes.is_empty(), main, asked: &job.intent.bias, primary: c.primary };
                        let moves: Vec<(SecCtx, art::Move)> = (0..secs.len()).map(|i| (secs[i].clone(), art::section_move(&lc, &secs, i, lseed))).collect();
                        art::render_form(&moves, c.cur, &ctx)
                    } else {
                        art::render(&spec, &ctx)
                    };
                    if pts.len() < 2 {
                        // nothing moves: an old generated lane of this parameter goes too
                        if job.song {
                            if let Some(t) = en.project.tracks.get_mut(ti) {
                                t.auto.retain(|l| l.param != c.key);
                            }
                        }
                        continue;
                    }
                    // lanes hold real values: pan runs -1..1
                    if c.key == "pan" {
                        for p in pts.iter_mut() {
                            p.1 = p.1 * 2.0 - 1.0;
                        }
                    }
                    let Some(t) = en.project.tracks.get_mut(ti) else { continue };
                    let li = match t.auto.iter().position(|l| l.param == c.key) {
                        Some(i) => i,
                        None => {
                            t.auto.push(AutoLane { param: c.key.clone(), points: vec![] });
                            t.auto.len() - 1
                        }
                    };
                    t.auto[li].points = pts;
                    written_keys.push((ti, c.key.clone()));
                    wrote += 1;
                }
                job.written += wrote;
                if wrote > 0 && !job.wrote_tracks.contains(&ti) {
                    job.wrote_tracks.push(ti);
                }
                own_lanes(&mut en.project, &written_keys);
                drop(en);
                self.touch();
                ok!(wrote)
            }
            "daw_artic_info" => {
                // -> a sentence about what the last run did
                let Some(job) = self.artic.as_ref() else { return ok!(String::new()) };
                if job.song {
                    let n = job.wrote_tracks.len();
                    let form = art::form_summary(&job.secs);
                    return ok!(format!(
                        "Re-automated {n} track{}, {} lane{}{} (sections from {}{}){}",
                        if n == 1 { "" } else { "s" },
                        job.written,
                        if job.written == 1 { "" } else { "s" },
                        if form.is_empty() { String::new() } else { format!(": {form}") },
                        job.src,
                        if job.genre.trim().is_empty() { String::new() } else { format!(", {}", job.genre.trim()) },
                        if job.kept > 0 { format!("; {} hand-drawn lane{} kept", job.kept, if job.kept == 1 { "" } else { "s" }) } else { String::new() }
                    ));
                }
                let mut tr = job.tracks.clone();
                tr.sort_unstable();
                tr.dedup();
                ok!(format!(
                    "{} automation lane{} on {} track{}{}",
                    job.written,
                    if job.written == 1 { "" } else { "s" },
                    tr.len(),
                    if tr.len() == 1 { "" } else { "s" },
                    if job.kept > 0 { format!(" ({} you drew yourself left alone)", job.kept) } else { String::new() }
                ) + &if job.form.is_empty() { String::new() } else { format!(", {}", job.form) })
            }
            "daw_artic_skin" => {
                // (track, style "auto"|"riddim"|"brostep"|"neuro"|"melodic", amount 0..1, flair 0..1, seed) -> a sentence: the track's
                // MIDI is re-performed as a dubstep bass (gated hits, per-hit envelopes, risers, chirps, glides) - see growlskin.rs.
                // The loop range when one is set, else the track's clips. Ctrl+Z (the caller pushes undo) brings it all back.
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let (b0, b1) = track_range(&en, a.u(0));
                let r = skin_track(&mut en, a.u(0), b0, b1, &a.s(1), a.n(2) as f32, a.n(3) as f32, a.n(4).abs() as u64 + 1);
                drop(en);
                self.touch();
                ok!(r)
            }
            "daw_artic_end" => {
                self.artic = None;
                Ok(Val::Unit)
            }
            _ => Err(format!("unknown builtin {name}")),
        }
    }
}

/// Skin a Growl track's notes in b0..b1 (see growlskin.rs): the notes inside are re-performed and the lanes' points inside are
/// replaced; everything outside the range stays. Returns the summary sentence (or why nothing was done).
pub(crate) fn skin_track(en: &mut crate::engine::Engine, ti: usize, b0: f64, b1: f64, style: &str, amount: f32, flair: f32, seed: u64) -> String {
    let Some(t) = en.project.tracks.get_mut(ti) else { return String::new() };
    let params: Vec<(String, f32)> = t.inst.as_ref().map(|i| i.params().into_iter().map(|(n, v)| (n.to_string(), v)).collect()).unwrap_or_default();
    if !params.iter().any(|p| p.0 == "cut") {
        return "This track's instrument has no filter cutoff to shape: the skin is made for Ling Growl".to_string();
    }
    // every MIDI note in absolute beats, looped clips unrolled; (key, start, len, vel), the clip it lives in, the note itself
    let mut src: Vec<((u8, f64, f64, f32), usize, Note)> = vec![];
    for (ci, c) in t.clips.iter().enumerate() {
        let ClipData::Midi(notes) = &c.data else { continue };
        let reps = if c.loop_len > 0.1 { ((c.len / c.loop_len).ceil() as usize).clamp(1, 512) } else { 1 };
        for r in 0..reps {
            let off = r as f64 * c.loop_len;
            for n in notes {
                if c.loop_len > 0.1 && n.start >= c.loop_len {
                    continue;
                }
                let on = c.start + off + n.start;
                if on < c.end() - 1e-6 {
                    let mut m = n.clone();
                    m.start = off + n.start;
                    src.push(((n.key, on, n.len.min(c.end() - on), n.vel), ci, m));
                }
            }
        }
    }
    src.sort_by(|x, y| x.0 .1.total_cmp(&y.0 .1));
    let inr: Vec<usize> = (0..src.len()).filter(|k| src[*k].0 .1 >= b0 - 1e-6 && src[*k].0 .1 < b1).collect();
    if inr.is_empty() {
        return "No notes to skin in this track (or in the loop range)".to_string();
    }
    let part: Vec<(u8, f64, f64, f32)> = inr.iter().map(|k| src[*k].0).collect();
    let style = crate::growlskin::style_for(style, &part);
    let lookup = |n: &str| params.iter().find(|p| p.0 == n).map(|p| p.1).unwrap_or(f32::NAN);
    let sk = crate::growlskin::skin(&part, b0, b1, style, amount, flair, seed, &lookup);
    // back into the clips: the notes outside the range stay, the skinned ones go to the clip their source came from
    let mut per: Vec<Vec<Note>> = t.clips.iter().map(|_| vec![]).collect();
    for (k, (n, ci, m)) in src.iter().enumerate() {
        if !inr.contains(&k) {
            let mut m = m.clone();
            m.len = m.len.min(n.2.max(0.01));
            per[*ci].push(m);
        }
    }
    for sn in &sk.notes {
        let (_, ci, m) = &src[inr[sn.src.min(inr.len() - 1)]];
        let c = &t.clips[*ci];
        let mut m = m.clone();
        m.key = sn.key;
        m.start = sn.start - c.start;
        m.len = sn.len.min(c.len - m.start).max(0.01);
        m.vel = sn.vel;
        m.lyric = String::new();
        if m.start >= 0.0 && m.start < c.len {
            per[*ci].push(m);
        }
    }
    for (ci, c) in t.clips.iter_mut().enumerate() {
        if !matches!(c.data, ClipData::Midi(_)) {
            continue;
        }
        let mut v = std::mem::take(&mut per[ci]);
        v.sort_by(|x, y| x.start.total_cmp(&y.start));
        c.loop_len = 0.0;
        c.data = ClipData::Midi(v);
    }
    // the lanes: points inside the range are replaced, the rest of a lane is kept
    for (name, pts) in sk.lanes {
        if !params.iter().any(|p| p.0 == name) || pts.len() < 2 {
            continue;
        }
        let li = match t.auto.iter().position(|l| l.param == name) {
            Some(i) => i,
            None => {
                t.auto.push(AutoLane { param: name.to_string(), points: vec![] });
                t.auto.len() - 1
            }
        };
        let lane = &mut t.auto[li].points;
        lane.retain(|p| p.0 < b0 - 1e-6 || p.0 > b1 + 1e-6);
        lane.extend(pts);
        lane.sort_by(|x, y| x.0.total_cmp(&y.0));
    }
    sk.summary
}
