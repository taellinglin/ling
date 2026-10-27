//! Builtins for AI Mix & Master (`daw_mix_*`, see mixai.rs).
use super::{Daw, A, Val};
use crate::engine::Engine;
use crate::mixai::{self, Change, Role, Stat, Style, TrackInfo};
use crate::model::*;
use crate::project;
use crate::space::{self, SRole, Space};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

pub struct MixResult {
    pub tracks: Vec<(usize, Stat)>,
    pub master: Stat,
    pub secs: f64,
}

pub struct MixJob {
    progress: AtomicU32,
    cancel: AtomicBool,
    result: Mutex<Option<Result<MixResult, String>>>,
}

#[derive(Default)]
pub struct MixState {
    job: Option<Arc<MixJob>>,
    res: Option<MixResult>,
    /// roles chosen by hand or by the language model, by track index
    roles: HashMap<usize, Role>,
    style: Option<Style>,
    plan: Vec<Change>,
}

fn role_of(st: &MixState, en: &Engine, i: usize) -> Role {
    if let Some(r) = st.roles.get(&i) {
        return *r;
    }
    let stat = st.res.as_ref().and_then(|r| r.tracks.iter().find(|t| t.0 == i)).map(|t| t.1.clone()).unwrap_or_default();
    let t = &en.project.tracks[i];
    mixai::guess_role(&t.name, &t.inst_kind, &stat)
}

fn has_fx(t: &Track, name: &str) -> bool {
    t.effects.iter().any(|e| e.name == name)
}

fn set_fx(slot: &mut EffectSlot, name: &str, v: f32) {
    if let Some(i) = slot.params.iter().position(|(n, _)| *n == name) {
        slot.set(i, v.clamp(0.0, 1.0));
    }
}

/// The genre the mix is treated as: the project's "ai.genre" meta when set, else guessed from the music (tempo, drum notes per beat, mean note length).
pub fn project_space(en: &Engine) -> (Space, bool) {
    if let Some((_, g)) = en.project.meta.iter().find(|(k, _)| k == "ai.genre") {
        if !g.trim().is_empty() {
            return (space::profile(g), true);
        }
    }
    let end = en.project.end_beat().max(1.0);
    let (mut drums, mut notes, mut len) = (0usize, 0usize, 0.0f64);
    for t in &en.project.tracks {
        for c in &t.clips {
            if let ClipData::Midi(ns) = &c.data {
                let reps = if c.loop_len > 0.0 { (c.len / c.loop_len).max(1.0) } else { 1.0 };
                if t.inst_kind == "drums" {
                    drums += (ns.len() as f64 * reps) as usize;
                } else {
                    notes += ns.len();
                    len += ns.iter().map(|n| n.len).sum::<f64>();
                }
            }
        }
    }
    let avg = if notes > 0 { len / notes as f64 } else { 0.5 };
    (space::profile(space::guess_genre(en.project.bpm, drums as f64 / end, avg)), false)
}

/// the moves that put a track's reverb, echo, sends, compressor timing and its instrument's own effects into the genre's space
/// values: [reverb, echo, feedback, size, damp, send cap, echo beats, attack, release, wetness, role index, dub, add a reverb]
pub fn space_changes(en: &Engine, sp: &Space, infos: &[(usize, TrackInfo)]) -> Vec<Change> {
    let mut out = vec![];
    for (i, info) in infos {
        let t = &en.project.tracks[*i];
        let sr = SRole::from_mix(info.role);
        let rs = sp.role(sr);
        let sends = en.project.routes.iter().any(|r| r.kind == RouteKind::Send && r.from == *i);
        let tw = space::inst_tweaks(sp, sr, &t.inst_kind, false);
        let inst_changes = t
            .inst
            .as_ref()
            .map(|ins| {
                let cur = ins.params();
                tw.iter().any(|w| {
                    cur.iter().any(|p| {
                        p.0 == w.param
                            && match w.mode {
                                space::Tw::Set => (p.1 - w.v).abs() > 1e-5,
                                space::Tw::Max => p.1 > w.v + 1e-5,
                                space::Tw::Min => p.1 < w.v - 1e-5,
                            }
                    })
                })
            })
            .unwrap_or(false);
        let adds = !sends && matches!(sr, SRole::Lead | SRole::Pad | SRole::Pluck | SRole::Vocal) && rs.reverb >= 0.08 && !has_fx(t, "Reverb");
        let timed = has_fx(t, "Compressor") && matches!(sr, SRole::Drums | SRole::Bass | SRole::Vocal);
        if !(has_fx(t, "Reverb") || has_fx(t, "Delay") || sends || inst_changes || adds || timed) {
            continue;
        }
        let ri = SRole::ALL.iter().position(|r| *r == sr).unwrap_or(0) as f32;
        out.push(Change {
            track: *i as i32,
            what: "space".into(),
            values: vec![rs.reverb, rs.delay, rs.feedback, rs.size, rs.damp, rs.send, rs.delay_beats, rs.attack, rs.release, sp.wet, ri, if sp.dub { 1.0 } else { 0.0 }, if adds { 1.0 } else { 0.0 }],
            reason: format!(
                "{} ({}) in a {} mix ({}): reverb {:.0}%, echo {:.0}% on {} beat, sends up to {:.0}%, compressor {:.0} ms / {:.0} ms",
                t.name,
                sr.name(),
                sp.tier(),
                sp.genre,
                rs.reverb * 100.0,
                rs.delay * 100.0,
                rs.delay_beats,
                rs.send * 100.0,
                1.0 + rs.attack * 199.0,
                10.0 + rs.release * 990.0
            ),
        });
    }
    if en.project.master_effects.iter().any(|e| matches!(e.name.as_str(), "Compressor" | "Reverb" | "Delay")) {
        let (a, r, rv, dl) = sp.master();
        out.push(Change {
            track: -1,
            what: "space".into(),
            values: vec![a, r, rv, dl],
            reason: format!("master bus for a {} mix ({}): glue compressor {:.0} ms / {:.0} ms, reverb at most {:.0}%, echo at most {:.0}%", sp.tier(), sp.genre, 1.0 + a * 199.0, 10.0 + r * 990.0, rv * 100.0, dl * 100.0),
        });
    }
    out
}

/// apply one "space" change to a track (and the project's sends from it)
fn apply_space(en: &mut Engine, ti: usize, v: &[f32], sr: f32) {
    let bpm = en.project.bpm;
    for r in en.project.routes.iter_mut().filter(|r| r.kind == RouteKind::Send && r.from == ti) {
        r.amount = r.amount.min(v[5]);
    }
    let Some(t) = en.project.tracks.get_mut(ti) else { return };
    // reverb and echo mixes go into the genre's band: half to a quarter above the target
    let band = |cur: f32, target: f32| cur.clamp(target * 0.5, target * 1.25);
    if let Some(e) = t.effects.iter_mut().find(|e| e.name == "Reverb") {
        if let Some(i) = e.params.iter().position(|p| p.0 == "mix") {
            let cur = e.params[i].1;
            e.set(i, band(cur, v[0]));
        }
        if let Some(i) = e.params.iter().position(|p| p.0 == "size") {
            let cur = e.params[i].1;
            e.set(i, cur.min(v[3].max(0.1)));
        }
    } else if v[12] > 0.5 {
        if let Some(mut s) = EffectSlot::new("Reverb", sr) {
            set_fx(&mut s, "mix", v[0]);
            set_fx(&mut s, "size", v[3]);
            set_fx(&mut s, "damp", v[4]);
            t.effects.push(s);
        }
    }
    if let Some(e) = t.effects.iter_mut().find(|e| e.name == "Delay") {
        if let Some(i) = e.params.iter().position(|p| p.0 == "mix") {
            let cur = e.params[i].1;
            e.set(i, band(cur, v[1]));
        }
        if let Some(i) = e.params.iter().position(|p| p.0 == "feedback") {
            let cur = e.params[i].1;
            e.set(i, cur.min(v[2]));
        }
        // synced to the tempo, as short as the genre likes
        set_fx(e, "time", Space::delay_time_param(v[6], bpm));
    }
    if let Some(e) = t.effects.iter_mut().find(|e| e.name == "Compressor") {
        set_fx(e, "attack", v[7]);
        set_fx(e, "release", v[8]);
    }
    // the instrument's own reverb / delay / chorus, and no vibrato or pitch sweep
    let sp = Space { genre: "mix", wet: v[9], dub: v[11] > 0.5 };
    let role = SRole::ALL[(v[10] as usize).min(SRole::ALL.len() - 1)];
    let kind = t.inst_kind.clone();
    if let Some(ins) = t.inst.as_mut() {
        space::apply_tweaks(ins.as_mut(), &space::inst_tweaks(&sp, role, &kind, false));
    }
}

impl Daw {
    fn mix_infos(&self, en: &Engine) -> Vec<(usize, TrackInfo)> {
        let Some(res) = self.mix.res.as_ref() else { return vec![] };
        res.tracks
            .iter()
            .filter(|(i, _)| *i < en.project.tracks.len())
            .map(|(i, s)| {
                let t = &en.project.tracks[*i];
                (*i, TrackInfo { name: t.name.clone(), volume: t.volume, pan: t.pan, has_eq: has_fx(t, "EQ"), has_comp: has_fx(t, "Compressor"), stat: s.clone(), role: role_of(&self.mix, en, *i) })
            })
            .collect()
    }

    pub(super) fn mix_dispatch(&mut self, name: &str, a: &A) -> Result<Val, String> {
        let eng = self.out.engine.clone();
        macro_rules! ok {
            ($v:expr) => {
                return Ok(Val::from($v))
            };
        }
        match name {
            "daw_mix_start" => {
                // (seconds to analyse, from the start; 0 = up to 150) -> started?
                if self.mix.job.as_ref().map(|j| j.result.lock().unwrap_or_else(|p| p.into_inner()).is_none()).unwrap_or(false) {
                    return ok!(false);
                }
                let doc = project::to_doc(&eng.lock().unwrap_or_else(|p| p.into_inner()));
                let base = std::path::Path::new(&self.path).parent().map(|p| p.to_path_buf());
                let samples = self.samples.clone();
                let sr = self.out.sr;
                let job = Arc::new(MixJob { progress: AtomicU32::new(0), cancel: AtomicBool::new(false), result: Mutex::new(None) });
                let secs_cap = if a.n(0) > 1.0 { a.n(0) } else { 150.0 };
                self.mix.job = Some(job.clone());
                self.mix.res = None;
                self.mix.plan.clear();
                std::thread::Builder::new()
                    .name("ling-mix".into())
                    .spawn(move || {
                        let make = |cache: &mut HashMap<String, Arc<Sample>>| {
                            let mut e = Engine::new(sr);
                            project::apply_doc(&mut e, &doc, base.as_deref(), cache);
                            e
                        };
                        let mut cache = samples.clone();
                        let probe = make(&mut cache);
                        let bpm = probe.project.bpm.max(20.0);
                        let end = probe.project.end_beat().min(secs_cap * bpm / 60.0).max(4.0);
                        let todo: Vec<usize> = (0..probe.project.tracks.len()).filter(|i| !probe.project.tracks[*i].clips.is_empty() || probe.project.tracks[*i].inst.is_some() && !probe.project.tracks[*i].session.is_empty()).collect();
                        drop(probe);
                        if todo.is_empty() {
                            *job.result.lock().unwrap_or_else(|p| p.into_inner()) = Some(Err("there is nothing to analyse: no clips".into()));
                            return;
                        }
                        let n = todo.len();
                        let next = std::sync::atomic::AtomicUsize::new(0);
                        let done = std::sync::atomic::AtomicUsize::new(0);
                        let out: Mutex<Vec<(usize, Stat)>> = Mutex::new(vec![]);
                        let workers = std::thread::available_parallelism().map(|c| c.get()).unwrap_or(1).min(n).max(1);
                        std::thread::scope(|sc| {
                            for _ in 0..workers {
                                sc.spawn(|| loop {
                                    let k = next.fetch_add(1, Ordering::Relaxed);
                                    if k >= n || job.cancel.load(Ordering::Relaxed) {
                                        break;
                                    }
                                    let ti = todo[k];
                                    let mut local = samples.clone();
                                    let mut e = Engine::new(sr);
                                    project::apply_doc(&mut e, &doc, base.as_deref(), &mut local);
                                    let keys: Vec<usize> = e.project.routes.iter().filter(|r| r.to == ti && matches!(r.kind, RouteKind::Sidechain | RouteKind::Duck | RouteKind::Mod | RouteKind::Send)).map(|r| r.from).collect();
                                    for (j, t) in e.project.tracks.iter_mut().enumerate() {
                                        t.mute = j != ti;
                                        t.solo = false;
                                        if j != ti && !keys.contains(&j) {
                                            t.clips.clear();
                                            t.session.clear();
                                        }
                                    }
                                    e.project.master_effects.clear();
                                    e.project.limiter = false;
                                    e.project.master_volume = 1.0;
                                    let dummy = AtomicU32::new(0);
                                    if let Some(buf) = e.bounce_range_progress(0.0, end, 1.0, &dummy, &job.cancel, 0, 1000) {
                                        out.lock().unwrap_or_else(|p| p.into_inner()).push((ti, mixai::analyze(&buf, sr)));
                                    }
                                    let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                                    job.progress.store((d * 900 / (n + 1)) as u32, Ordering::Relaxed);
                                });
                            }
                        });
                        if job.cancel.load(Ordering::Relaxed) {
                            *job.result.lock().unwrap_or_else(|p| p.into_inner()) = Some(Err("cancelled".into()));
                            return;
                        }
                        // the whole mix through the master chain
                        let mut e = make(&mut cache);
                        let dummy = AtomicU32::new(0);
                        let master = e.bounce_range_progress(0.0, end, 1.0, &dummy, &job.cancel, 0, 1000).map(|b| mixai::analyze(&b, sr)).unwrap_or_default();
                        let mut tracks = out.into_inner().unwrap_or_default();
                        tracks.sort_by_key(|t| t.0);
                        job.progress.store(1000, Ordering::Relaxed);
                        *job.result.lock().unwrap_or_else(|p| p.into_inner()) = Some(Ok(MixResult { tracks, master, secs: end * 60.0 / bpm }));
                    })
                    .map_err(|e| e.to_string())?;
                ok!(true)
            }
            "daw_mix_poll" => {
                // -> [progress 0..1, done 0/1, error text]
                let Some(job) = self.mix.job.clone() else { return ok!(Val::List(vec![Val::Num(0.0), Val::Num(1.0), Val::Str(String::new())])) };
                let r = job.result.lock().unwrap_or_else(|p| p.into_inner()).take();
                match r {
                    Some(Ok(res)) => {
                        self.mix.res = Some(res);
                        self.mix.job = None;
                        ok!(Val::List(vec![Val::Num(1.0), Val::Num(1.0), Val::Str(String::new())]))
                    }
                    Some(Err(e)) => {
                        self.mix.job = None;
                        ok!(Val::List(vec![Val::Num(0.0), Val::Num(1.0), Val::Str(e)]))
                    }
                    None => ok!(Val::List(vec![Val::Num(job.progress.load(Ordering::Relaxed) as f64 / 1000.0), Val::Num(0.0), Val::Str(String::new())])),
                }
            }
            "daw_mix_cancel" => {
                if let Some(j) = &self.mix.job {
                    j.cancel.store(true, Ordering::Relaxed);
                }
                ok!(true)
            }
            "daw_mix_tracks" => {
                // -> [track, role, active rms dB, peak dB, crest dB, centroid Hz, width dB, active share, sub, bass, low-mid, mid, presence, air] per analysed track
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let mut v = vec![];
                for (i, info) in self.mix_infos(&en) {
                    let s = &info.stat;
                    v.extend([Val::Num(i as f64), Val::Str(info.role.name().into())]);
                    v.extend([s.active_rms_db, s.peak_db, s.crest_db, s.centroid_hz, s.width_db, s.active].iter().map(|x| Val::Num(*x as f64)));
                    v.extend(s.bands.iter().map(|x| Val::Num(*x as f64)));
                }
                ok!(Val::List(v))
            }
            "daw_mix_master" => {
                // -> [lufs, peak dB, rms dB, crest, centroid, width dB, correlation, seconds analysed, clipped samples, sub..air x6]
                let Some(r) = self.mix.res.as_ref() else { return ok!(Val::List(vec![])) };
                let s = &r.master;
                let mut v: Vec<Val> = [s.lufs, s.peak_db, s.rms_db, s.crest_db, s.centroid_hz, s.width_db, s.corr, r.secs as f32, s.clipped as f32].iter().map(|x| Val::Num(*x as f64)).collect();
                v.extend(s.bands.iter().map(|x| Val::Num(*x as f64)));
                ok!(Val::List(v))
            }
            "daw_mix_set_role" => {
                // (track, role name)
                match Role::parse(&a.s(1)) {
                    Some(r) => {
                        self.mix.roles.insert(a.u(0), r);
                        ok!(true)
                    }
                    None => ok!(false),
                }
            }
            "daw_mix_style" => {
                // (loudness LUFS, brightness -1..1, punch 0..1, width 0..1)
                self.mix.style = Some(Style { loudness: a.n(0).clamp(-24.0, -6.0) as f32, brightness: a.n(1).clamp(-1.0, 1.0) as f32, punch: a.n(2).clamp(0.0, 1.0) as f32, width: a.n(3).clamp(0.0, 1.0) as f32 });
                ok!(true)
            }
            "daw_mix_plan" => {
                // -> [track, what, text, value 1, value 2, value 3]* ; track -1 = master
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let infos = self.mix_infos(&en);
                let Some(res) = self.mix.res.as_ref() else { return ok!(Val::List(vec![])) };
                let list: Vec<TrackInfo> = infos.into_iter().map(|(_, t)| t).collect();
                // plan() indexes tracks by position in the list: map back to project indices
                let idx: Vec<usize> = res.tracks.iter().map(|t| t.0).filter(|i| *i < en.project.tracks.len()).collect();
                let style = self.mix.style.clone().unwrap_or_default();
                let mut plan = mixai::plan(&list, &res.master, en.project.master_volume, &style);
                for c in plan.iter_mut() {
                    if c.track >= 0 {
                        c.track = idx[c.track as usize] as i32;
                    }
                }
                let mut v = vec![];
                for c in &plan {
                    v.extend([Val::Num(c.track as f64), Val::Str(c.what.clone()), Val::Str(c.reason.clone())]);
                    for k in 0..3 {
                        v.push(Val::Num(c.values.get(k).copied().unwrap_or(0.0) as f64));
                    }
                }
                // the genre's space: compressor timing for the planned compressors, then reverb / echo / sends / the instruments' own effects
                let (sp, _) = project_space(&en);
                let infos2 = self.mix_infos(&en);
                for c in plan.iter_mut().filter(|c| c.what == "comp" && c.track >= 0) {
                    if let Some((_, info)) = infos2.iter().find(|(i, _)| *i as i32 == c.track) {
                        let rs = sp.role(SRole::from_mix(info.role));
                        c.values.push(rs.attack);
                        c.values.push(rs.release);
                        c.reason.push_str(&format!("; {:.0} ms attack / {:.0} ms release for a {} mix", 1.0 + rs.attack * 199.0, 10.0 + rs.release * 990.0, sp.tier()));
                    }
                }
                plan.extend(space_changes(&en, &sp, &infos2));
                drop(en);
                self.mix.plan = plan;
                ok!(Val::List(v))
            }
            "daw_mix_apply" => {
                // (skip list "0,3" of plan entries to leave out) -> changes applied
                let skip: Vec<usize> = a.s(0).split(',').filter_map(|x| x.trim().parse().ok()).collect();
                let plan = std::mem::take(&mut self.mix.plan);
                let sr = self.out.sr;
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let mut n = 0;
                for (k, c) in plan.iter().enumerate() {
                    if skip.contains(&k) {
                        continue;
                    }
                    if c.track < 0 {
                        if c.what == "space" && c.values.len() >= 4 {
                            for e in en.project.master_effects.iter_mut() {
                                match e.name.as_str() {
                                    "Compressor" => {
                                        set_fx(e, "attack", c.values[0]);
                                        set_fx(e, "release", c.values[1]);
                                    }
                                    "Reverb" | "Delay" => {
                                        let cap = if e.name == "Reverb" { c.values[2] } else { c.values[3] };
                                        if let Some(i) = e.params.iter().position(|p| p.0 == "mix") {
                                            let cur = e.params[i].1;
                                            e.set(i, cur.min(cap));
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            n += 1;
                            continue;
                        }
                        if c.what == "master" {
                            en.project.master_volume = c.values[0].clamp(0.0, 2.0);
                            en.project.limiter = true;
                            n += 1;
                        }
                        continue;
                    }
                    if c.what == "space" && c.values.len() >= 13 {
                        apply_space(&mut en, c.track as usize, &c.values, sr);
                        n += 1;
                        continue;
                    }
                    let Some(t) = en.project.tracks.get_mut(c.track as usize) else { continue };
                    match c.what.as_str() {
                        "gain" => t.volume = c.values[0].clamp(0.0, 1.5),
                        "pan" => t.pan = c.values[0].clamp(-1.0, 1.0),
                        "eq" => {
                            if !has_fx(t, "EQ") {
                                if let Some(s) = EffectSlot::new("EQ", sr) {
                                    t.effects.push(s);
                                }
                            }
                            if let Some(slot) = t.effects.iter_mut().find(|e| e.name == "EQ") {
                                for (pi, name) in ["low", "mid", "high"].iter().enumerate() {
                                    if let Some(i) = slot.params.iter().position(|(n, _)| n == name) {
                                        let cur = (slot.params[i].1 - 0.5) * 24.0;
                                        slot.set(i, ((cur + c.values[pi]) / 24.0 + 0.5).clamp(0.0, 1.0));
                                    }
                                }
                            }
                        }
                        "comp" => {
                            if !has_fx(t, "Compressor") {
                                if let Some(s) = EffectSlot::new("Compressor", sr) {
                                    t.effects.push(s);
                                }
                            }
                            if let Some(slot) = t.effects.iter_mut().find(|e| e.name == "Compressor") {
                                let set = |slot: &mut EffectSlot, name: &str, v: f32| {
                                    if let Some(i) = slot.params.iter().position(|(n, _)| *n == name) {
                                        slot.set(i, v.clamp(0.0, 1.0));
                                    }
                                };
                                set(slot, "threshold", (c.values[0] + 60.0) / 60.0);
                                set(slot, "ratio", (c.values[1] - 1.0) / 19.0);
                                set(slot, "attack", c.values.get(2).copied().unwrap_or(0.08));
                                set(slot, "release", c.values.get(3).copied().unwrap_or(0.18));
                            }
                        }
                        _ => continue,
                    }
                    n += 1;
                }
                drop(en);
                if n > 0 {
                    self.touch();
                }
                ok!(n)
            }
            "daw_mix_prompt" => {
                // the question for the language model: the tracks as a table, asking for roles and a style as JSON
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let mut s = String::from("Mix analysis of a song. For each track: index, name, instrument, spectral centroid in Hz, level where it plays in dBFS, share of time it plays, and its guessed role.\n");
                for (i, info) in self.mix_infos(&en) {
                    let t = &en.project.tracks[i];
                    s.push_str(&format!("{i}: \"{}\" ({}) centroid {:.0} Hz, level {:.0} dB, plays {:.0}% of the time, guessed {}\n", t.name, if t.inst_kind.is_empty() { "audio" } else { &t.inst_kind }, info.stat.centroid_hz, info.stat.active_rms_db, info.stat.active * 100.0, info.role.name()));
                }
                s.push_str("\nAnswer with JSON only: {\"style\": {\"loudness\": -14, \"brightness\": 0.0, \"punch\": 0.5, \"width\": 0.5}, \"roles\": {\"0\": \"bass\", ...}}\nroles: drums, bass, vocal, lead, chords, pad, fx, other. loudness is the master target in LUFS (-16 gentle .. -9 club). brightness -1 dark .. 1 bright, punch 0..1, width 0..1. Correct a wrong guessed role, keep the rest.");
                ok!(s)
            }
            "daw_mix_system" => ok!("You are a mixing and mastering engineer. You choose the role of every track and a target style from measurements. Reply with JSON only."),
            "daw_mix_parse" => {
                // (the model's reply) -> roles set
                let text = a.s(0);
                let (Some(i), Some(j)) = (text.find('{'), text.rfind('}')) else { return ok!(0.0) };
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&text[i..=j]) else { return ok!(0.0) };
                let mut n = 0;
                if let Some(roles) = v["roles"].as_object() {
                    for (k, r) in roles {
                        if let (Ok(ti), Some(role)) = (k.parse::<usize>(), r.as_str().and_then(Role::parse)) {
                            self.mix.roles.insert(ti, role);
                            n += 1;
                        }
                    }
                }
                if v["style"].is_object() {
                    let st = &v["style"];
                    self.mix.style = Some(Style {
                        loudness: st["loudness"].as_f64().unwrap_or(-14.0).clamp(-24.0, -6.0) as f32,
                        brightness: st["brightness"].as_f64().unwrap_or(0.0).clamp(-1.0, 1.0) as f32,
                        punch: st["punch"].as_f64().unwrap_or(0.5).clamp(0.0, 1.0) as f32,
                        width: st["width"].as_f64().unwrap_or(0.5).clamp(0.0, 1.0) as f32,
                    });
                }
                ok!(n)
            }
            _ => Err(format!("unknown builtin {name}")),
        }
    }
}

#[cfg(test)]
mod space_tests {
    use super::*;

    /// a lead with a wet reverb and echo, a drum track and a bass with a reverb, a vibrato chip lead
    fn project(genre: &str) -> Engine {
        let sr = 44100.0;
        let mut e = Engine::new(sr);
        e.project.bpm = 140.0;
        e.project.meta.push(("ai.genre".into(), genre.into()));
        for (name, kind) in [("Lead", "prism"), ("Drums", "drums"), ("Bass", "prism"), ("Chip", "chip")] {
            let mut t = Track::new(name, TrackKind::Instrument, 0, crate::dsp::make_instrument(kind, sr));
            t.inst_kind = kind.into();
            for fx in ["Reverb", "Delay", "Compressor"] {
                let mut s = EffectSlot::new(fx, sr).unwrap();
                if fx != "Compressor" {
                    set_fx(&mut s, "mix", 0.6);
                    set_fx(&mut s, "feedback", 0.8);
                    set_fx(&mut s, "size", 0.95);
                }
                t.effects.push(s);
            }
            e.project.tracks.push(t);
        }
        if let Some(i) = e.project.tracks[3].inst.as_mut() {
            i.set_param("vib", 0.8);
            i.set_param("sweep", 0.9);
        }
        e
    }

    fn infos(e: &Engine) -> Vec<(usize, TrackInfo)> {
        let roles = [Role::Lead, Role::Drums, Role::Bass, Role::Lead];
        (0..4).map(|i| (i, TrackInfo { name: e.project.tracks[i].name.clone(), volume: 0.8, pan: 0.0, has_eq: false, has_comp: true, stat: Stat::default(), role: roles[i] })).collect()
    }

    fn mixed(genre: &str) -> Engine {
        let mut e = project(genre);
        let (sp, from_meta) = project_space(&e);
        assert!(from_meta);
        for c in space_changes(&e, &sp, &infos(&e)) {
            if c.track >= 0 {
                apply_space(&mut e, c.track as usize, &c.values, 44100.0);
            }
        }
        e
    }
    fn fxp(e: &Engine, t: usize, fx: &str, p: &str) -> f32 {
        let s = e.project.tracks[t].effects.iter().find(|x| x.name == fx).unwrap();
        s.params.iter().find(|x| x.0 == p).unwrap().1
    }

    #[test]
    fn a_breakbeat_mix_ends_drier_and_punchier_than_an_ambient_one() {
        let (b, a) = (mixed("breakbeat"), mixed("ambient"));
        for t in 0..3 {
            assert!(fxp(&b, t, "Reverb", "mix") <= fxp(&a, t, "Reverb", "mix"), "track {t} reverb");
            assert!(fxp(&b, t, "Reverb", "size") <= fxp(&a, t, "Reverb", "size"), "track {t} room");
            assert!(fxp(&b, t, "Delay", "mix") <= fxp(&a, t, "Delay", "mix"), "track {t} echo");
            assert!(fxp(&b, t, "Delay", "feedback") <= fxp(&a, t, "Delay", "feedback"), "track {t} feedback");
            assert!(fxp(&b, t, "Compressor", "release") <= fxp(&a, t, "Compressor", "release"), "track {t} release");
        }
        // the kick and the bass are dry in the breakbeat mix, the lead is still a little wet
        assert!(fxp(&b, 1, "Reverb", "mix") < 0.03 && fxp(&b, 2, "Reverb", "mix") == 0.0 && fxp(&b, 0, "Reverb", "mix") < 0.2);
        assert!(fxp(&a, 0, "Reverb", "mix") > 3.0 * fxp(&b, 0, "Reverb", "mix"));
        assert!(fxp(&b, 0, "Delay", "time") < fxp(&a, 0, "Delay", "time"));
        // vibrato and the pitch sweep of the chip lead are gone in both
        for e in [&b, &a] {
            let p = e.project.tracks[3].inst.as_ref().unwrap().params();
            assert_eq!(p.iter().find(|x| x.0 == "vib").unwrap().1, 0.0);
            assert_eq!(p.iter().find(|x| x.0 == "sweep").unwrap().1, 0.5);
        }
    }

    #[test]
    fn the_genre_comes_from_the_meta_or_from_the_content() {
        let mut e = project("");
        e.project.meta.clear();
        e.project.bpm = 172.0;
        // no genre set and no notes at all: tempo alone says house at 172, never ambient
        assert!(!project_space(&e).1);
        e.project.bpm = 70.0;
        assert_eq!(project_space(&e).0.genre, "lofi");
        e.project.meta.push(("ai.genre".into(), "drum and bass".into()));
        assert_eq!(project_space(&e).0.genre, "drum and bass");
    }
}
