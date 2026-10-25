//! Builtins for "Articulate": generate automation lanes for all, random or one track's parameters, optionally steered by a prompt.
//!
//! `daw_artic_begin(mode, track, count, seed, amount, prompt)` collects the candidate parameters (never switches, levels or sequence
//! steps; never a lane the user has already drawn) and splits them into chunks of up to ten for one track each. For every chunk
//! `daw_artic_prompt(i)` is the question for the model and `daw_artic_apply(i, reply)` writes the lanes: the model's picks where it gave
//! usable ones, a local choice for everything else, so an empty or broken reply still produces a full result.
use super::{Daw, A, Val};
use crate::articulate::{self as art, Intent, Role};
use crate::compose::Rng;
use crate::model::*;

pub struct Cand {
    track: usize,
    /// the lane's `param`: an instrument parameter name, "pan" or "fx<i>.<p>"
    key: String,
    label: String,
    role: Role,
    cur: f32,
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
}

const CHUNK: usize = 10;
const PER_TRACK: usize = 40;

fn lane_has_points(t: &Track, key: &str) -> bool {
    t.auto.iter().any(|l| l.param == key && l.points.len() >= 2)
}

/// every parameter of a track that may be automated
fn candidates(t: &Track, ti: usize, kept: &mut usize) -> Vec<Cand> {
    let mut out = vec![];
    let mut push = |key: String, label: String, name: &str, cur: f32, kept: &mut usize| {
        if !art::eligible(name) {
            return;
        }
        if lane_has_points(t, &key) {
            *kept += 1;
            return;
        }
        out.push(Cand { track: ti, key, label, role: art::role_of(name), cur });
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
                // (mode "single" | "random" | "all", track, count, seed, amount 0..1, prompt) -> chunks to ask about (0 = nothing to do)
                let mode = a.s(0);
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let mut kept = 0usize;
                let skip_drums = mode != "single";
                let which: Vec<usize> = if mode == "single" { vec![a.u(1)].into_iter().filter(|t| *t < en.project.tracks.len()).collect() } else { (0..en.project.tracks.len()).collect() };
                let mut per: Vec<(usize, Vec<Cand>)> = vec![];
                for ti in which {
                    let t = &en.project.tracks[ti];
                    if skip_drums && t.inst_kind == "drums" {
                        continue;
                    }
                    let mut c = candidates(t, ti, &mut kept);
                    if mode != "random" {
                        // the parameters that shape a sound most come first when the cap bites; the effects keep up to 12 places so a
                        // big instrument does not crowd them out
                        c.sort_by_key(|x| art::priority(x.role));
                        let (mut fx, mut inst): (Vec<Cand>, Vec<Cand>) = c.into_iter().partition(|x| x.key.starts_with("fx"));
                        fx.truncate(12);
                        inst.truncate(PER_TRACK.saturating_sub(fx.len()));
                        inst.extend(fx);
                        c = inst;
                    }
                    if !c.is_empty() {
                        per.push((ti, c));
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
                drop(en);
                // (7th) phrase length in bars: 4 .. 64, 0 = read from the music
                let phrase = a.n(6).clamp(0.0, 64.0);
                self.artic = Some(Job { chunks, intent: art::read_prompt(&prompt), prompt, amount: a.n(4).clamp(0.0, 1.0) as f32, seed, written: 0, kept, tracks, phrase, form: String::new() });
                ok!(n)
            }
            "daw_artic_prompt" => {
                // chunk -> the history for the model: a worked example, then this track's parameters
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
                let params: Vec<(String, Role, f32)> = chunk.iter().map(|c| (c.label.clone(), c.role, c.cur)).collect();
                ok!(format!("{}<|user|>\n{}\n", art::example(), art::user_prompt(&label, en.project.bpm, (b1 - b0) / 4.0, &job.prompt, &params)))
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
                let mut wrote = 0usize;
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
                for (k, c) in chunk.iter().enumerate() {
                    let mut rng = Rng::new(job.seed ^ ((a.u(0) as u64) << 20) ^ ((k as u64 + 1) * 0x9E37));
                    let pick = usable[k];
                    let spec = art::spec_for(c.role, c.cur, job.amount, &job.intent, pick, &mut rng, !notes.is_empty());
                    let ctx = art::Ctx { b0, b1, phrase: job.phrase, notes: &notes, groove: &groove, seed: (job.seed as u32).wrapping_add((ti * 131 + k) as u32), fills: job.intent.fills };
                    let mut pts = art::render(&spec, &ctx);
                    if pts.len() < 2 {
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
                    wrote += 1;
                }
                job.written += wrote;
                drop(en);
                self.touch();
                ok!(wrote)
            }
            "daw_artic_info" => {
                // -> a sentence about what the last run did
                let Some(job) = self.artic.as_ref() else { return ok!(String::new()) };
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
