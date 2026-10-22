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
                        // the parameters that shape a sound most come first when the cap bites
                        c.sort_by_key(|x| art::priority(x.role));
                        c.truncate(PER_TRACK);
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
                self.artic = Some(Job { chunks, intent: art::read_prompt(&prompt), prompt, amount: a.n(4).clamp(0.0, 1.0) as f32, seed, written: 0, kept, tracks });
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
                let block = if b1 - b0 >= 128.0 { 32.0 } else if b1 - b0 >= 32.0 { 16.0 } else { 8.0 };
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
                    let ctx = art::Ctx { b0, b1, block, notes: &notes, seed: (job.seed as u32).wrapping_add((ti * 131 + k) as u32), fills: job.intent.fills };
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
                ))
            }
            "daw_artic_end" => {
                self.artic = None;
                Ok(Val::Unit)
            }
            _ => Err(format!("unknown builtin {name}")),
        }
    }
}
