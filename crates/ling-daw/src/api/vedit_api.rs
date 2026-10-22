//! The inline vocal editor (`daw_ve_*`, the piano roll on an audio clip): a recorded voice is cut into its sung notes ("blobs") and every
//! blob can be moved in pitch, have its drift flattened, be moved and stretched in time (the sound between notes stretches with it) and
//! change loudness. It joins what Vocal to MIDI, the Vocal Tuner and "Tune to the score" did one at a time: correct the pitch to a
//! scale (with a temperament), quantize the timing, align pitch and timing to a guide (a MIDI part or another edited vocal), and write the
//! notes out as MIDI.
//!
//! Non-destructive: the original recording is kept (`ved.<id>.src`), the edit lives in the project meta (`ved.<id>` options,
//! `ved.<id>.blobs`) and the clip's tag (`ved:<id>`), and a render (vocal.rs `render_segments`, PSOLA: formants stay) replaces the
//! clip's audio with a file next to the project. Times are seconds on the recording's own time line, like the clip's `offset_s`.
use super::theory_api::expand;
use super::vocmidi_api::SCALES;
use super::{Daw, A, Val};
use crate::model::*;
use crate::vocal::{self, EditOpts, Pitch, Seg};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

const MIN_LEN: f64 = 0.03;
/// renders of one vocal kept on disk and in memory (so undo can step back through them)
const KEEP_RENDERS: usize = 8;

#[derive(Clone, Debug)]
struct Blob {
    src_on: f64,
    src_off: f64,
    dst_on: f64,
    dst_off: f64,
    midi: f64,
    shift: f64,
    drift: f64,
    gain: f64,
    vel: f64,
}

#[derive(Clone, Debug)]
struct Edit {
    src: String,
    blobs: Vec<Blob>,
    vdepth: f64,
    vrate: f64,
    retune: f64,
    temp: usize,
    troot: i32,
    bypass: bool,
    rendered: String,
    rev: u64,
}

/// pitch and level of an original recording, by path (analysing is the slow part, so it is done once per recording)
struct Ana {
    pitch: Pitch,
    level: Vec<f32>,
    top: f32,
}
fn ana_cache() -> &'static Mutex<HashMap<String, Arc<Ana>>> {
    static C: OnceLock<Mutex<HashMap<String, Arc<Ana>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn kv_num(text: &str, key: &str, d: f64) -> f64 {
    text.split(';').find_map(|p| p.split_once('=').filter(|(k, _)| *k == key).and_then(|(_, v)| v.parse().ok())).unwrap_or(d)
}
fn meta_get(p: &Project, key: &str) -> Option<String> {
    p.meta.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}
fn meta_put(p: &mut Project, key: &str, v: String) {
    if let Some(e) = p.meta.iter_mut().find(|(k, _)| k == key) {
        e.1 = v;
    } else {
        p.meta.push((key.to_string(), v));
    }
}
fn edit_id(c: &Clip) -> Option<String> {
    c.tag.strip_prefix("ved:").map(|s| s.to_string())
}

fn load_edit(p: &Project, id: &str) -> Option<Edit> {
    let opts = meta_get(p, &format!("ved.{id}"))?;
    let src = meta_get(p, &format!("ved.{id}.src")).unwrap_or_default();
    let blobs = meta_get(p, &format!("ved.{id}.blobs"))
        .unwrap_or_default()
        .split('|')
        .filter_map(|b| {
            let v: Vec<f64> = b.split(',').filter_map(|x| x.parse().ok()).collect();
            (v.len() >= 9).then(|| Blob { src_on: v[0], src_off: v[1], dst_on: v[2], dst_off: v[3], midi: v[4], shift: v[5], drift: v[6], gain: v[7], vel: v[8] })
        })
        .collect();
    Some(Edit {
        src,
        blobs,
        vdepth: kv_num(&opts, "vdepth", 0.0),
        vrate: kv_num(&opts, "vrate", 5.5),
        retune: kv_num(&opts, "retune", 12.0),
        temp: kv_num(&opts, "temp", 0.0) as usize,
        troot: kv_num(&opts, "troot", 0.0) as i32,
        bypass: kv_num(&opts, "bypass", 0.0) > 0.5,
        rendered: meta_get(p, &format!("ved.{id}.out")).unwrap_or_default(),
        rev: kv_num(&opts, "rev", 0.0) as u64,
    })
}

fn save_edit(p: &mut Project, id: &str, e: &Edit) {
    let opts = format!("vdepth={};vrate={};retune={};temp={};troot={};bypass={};rev={}", e.vdepth, e.vrate, e.retune, e.temp, e.troot, e.bypass as u8, e.rev);
    meta_put(p, &format!("ved.{id}"), opts);
    meta_put(p, &format!("ved.{id}.src"), e.src.clone());
    meta_put(p, &format!("ved.{id}.out"), e.rendered.clone());
    let blobs: Vec<String> = e
        .blobs
        .iter()
        .map(|b| format!("{:.5},{:.5},{:.5},{:.5},{:.4},{:.4},{:.3},{:.4},{:.3}", b.src_on, b.src_off, b.dst_on, b.dst_off, b.midi, b.shift, b.drift, b.gain, b.vel))
        .collect();
    meta_put(p, &format!("ved.{id}.blobs"), blobs.join("|"));
}

/// the selection the script passes: "3,4,7" (empty = every blob)
fn sel_of(s: &str, n: usize) -> Vec<usize> {
    let v: Vec<usize> = s.split(',').filter_map(|x| x.trim().parse::<usize>().ok()).filter(|i| *i < n).collect();
    if v.is_empty() && s.trim().is_empty() { (0..n).collect() } else { v }
}

fn median_midi(p: &Pitch, a: f64, b: f64, fallback: f64) -> f64 {
    let mut v: Vec<f64> = (((a / p.hop) as usize)..((b / p.hop) as usize).min(p.f0.len())).map(|j| p.f0[j]).filter(|f| *f > 0.0).map(|f| 69.0 + 12.0 * (f as f64 / 440.0).log2()).collect();
    if v.len() < 3 {
        return fallback;
    }
    v.sort_by(|x, y| x.total_cmp(y));
    v[v.len() / 2]
}

/// keep the notes in order and apart: no note starts before the one before it ends, and none is shorter than MIN_LEN
fn tidy(blobs: &mut [Blob]) {
    blobs.sort_by(|a, b| a.dst_on.total_cmp(&b.dst_on));
    for i in 0..blobs.len() {
        blobs[i].dst_on = blobs[i].dst_on.max(0.0);
        if i > 0 && blobs[i].dst_on < blobs[i - 1].dst_off {
            // the earlier note gives way (it is shortened), unless that would make it too short
            let lo = blobs[i - 1].dst_on + MIN_LEN;
            if blobs[i].dst_on >= lo {
                blobs[i - 1].dst_off = blobs[i].dst_on;
            } else {
                blobs[i - 1].dst_off = lo;
                let len = blobs[i].dst_off - blobs[i].dst_on;
                blobs[i].dst_on = lo;
                blobs[i].dst_off = blobs[i].dst_off.max(lo + len.max(MIN_LEN));
            }
        }
        if blobs[i].dst_off < blobs[i].dst_on + MIN_LEN {
            blobs[i].dst_off = blobs[i].dst_on + MIN_LEN;
        }
    }
}

fn allowed_of(scale: usize, root: i32) -> [bool; 12] {
    let mut allowed = [false; 12];
    for (k, on) in SCALES[scale.min(SCALES.len() - 1)].1.iter().enumerate() {
        allowed[(k as i32 + root).rem_euclid(12) as usize] = *on;
    }
    allowed
}

/// a whole-number key in the edit's temperament, as a fractional MIDI pitch
fn tempered(e: &Edit, key: i32) -> f64 {
    let t = vocal::TEMPERAMENTS[e.temp.min(vocal::TEMPERAMENTS.len() - 1)];
    key as f64 + t[(key - e.troot).rem_euclid(12) as usize] / 100.0
}

struct Geo {
    offset_s: f64,
    spb: f64,
    start: f64,
    len: f64,
}
impl Geo {
    fn beat(&self, s: f64) -> f64 {
        self.start + (s - self.offset_s) / self.spb
    }
    fn secs(&self, b: f64) -> f64 {
        self.offset_s + (b - self.start) * self.spb
    }
}
fn geo_of(clip: &Clip, bpm: f64) -> Option<Geo> {
    let ClipData::Audio { offset_s, warp, .. } = &clip.data else { return None };
    let spb = if *warp > 1.0 { 60.0 / *warp as f64 } else { 60.0 / bpm.max(20.0) };
    Some(Geo { offset_s: *offset_s, spb, start: clip.start, len: clip.len })
}

impl Daw {
    fn ve_ana(&mut self, path: &str) -> Result<(Arc<Sample>, Arc<Ana>), String> {
        let s = self.load_sample_cached(path)?;
        if let Some(a) = ana_cache().lock().unwrap_or_else(|p| p.into_inner()).get(path) {
            return Ok((s, a.clone()));
        }
        let mono = vocal::mono_of(&s.stereo);
        let pitch = vocal::track_pitch(&mono, s.rate as f32);
        let level = vocal::level_track(&mono, s.rate as f32, pitch.hop);
        let top = level.iter().cloned().fold(1e-6, f32::max);
        let a = Arc::new(Ana { pitch, level, top });
        ana_cache().lock().unwrap_or_else(|p| p.into_inner()).insert(path.to_string(), a.clone());
        Ok((s, a))
    }

    /// (edit id, edit, clip geometry) of an edited audio clip
    fn ve_get(&self, t: usize, c: usize) -> Option<(String, Edit, Geo)> {
        let en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
        let clip = en.project.tracks.get(t)?.clips.get(c)?;
        let geo = geo_of(clip, en.project.bpm)?;
        let id = edit_id(clip)?;
        let e = load_edit(&en.project, &id)?;
        Some((id, e, geo))
    }

    fn ve_put(&mut self, id: &str, e: &Edit) {
        {
            let mut en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
            save_edit(&mut en.project, id, e);
        }
        self.touch();
    }

    /// point the clip at `sample` (the render or the original)
    fn ve_set_audio(&mut self, t: usize, c: usize, sample: Arc<Sample>) {
        let mut en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(clip) = en.project.tracks.get_mut(t).and_then(|x| x.clips.get_mut(c)) {
            if let ClipData::Audio { sample: s, .. } = &mut clip.data {
                *s = sample;
            }
        }
    }

    fn ve_render(&mut self, t: usize, c: usize, id: &str, e: &mut Edit) -> Result<usize, String> {
        let (orig, ana) = self.ve_ana(&e.src.clone())?;
        let segs: Vec<Seg> = e
            .blobs
            .iter()
            .map(|b| Seg { src_on: b.src_on, src_off: b.src_off, dst_on: b.dst_on, dst_off: b.dst_off, midi: b.midi, shift: b.shift, drift: b.drift, gain: b.gain })
            .collect();
        let untouched = e.blobs.iter().all(|b| b.shift.abs() < 1e-4 && b.drift < 1e-4 && (b.gain - 1.0).abs() < 1e-4 && (b.dst_on - b.src_on).abs() < 1e-5 && (b.dst_off - b.src_off).abs() < 1e-5) && e.vdepth <= 0.0;
        if untouched {
            // nothing to do: the clip plays the recording itself
            e.rendered.clear();
            e.bypass = false;
            self.ve_set_audio(t, c, orig);
            return Ok(0);
        }
        let opts = EditOpts { vib_depth_cents: e.vdepth.clamp(0.0, 100.0), vib_rate: e.vrate.clamp(2.0, 9.0), retune_ms: e.retune.clamp(1.0, 200.0) };
        let out = vocal::render_segments(&orig.stereo, orig.rate as f32, &ana.pitch, &segs, &opts);
        let dir = if self.path.is_empty() {
            std::path::PathBuf::from("cache/vox")
        } else {
            std::path::Path::new(&self.path).parent().map(|p| p.join("audio")).unwrap_or_else(|| "cache/vox".into())
        };
        std::fs::create_dir_all(&dir).map_err(|x| x.to_string())?;
        e.rev += 1;
        let file = dir.join(format!("vocal_edit_{id}_{}.wav", e.rev));
        let spec = hound::WavSpec { channels: 2, sample_rate: orig.rate, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
        {
            let mut w = hound::WavWriter::create(&file, spec).map_err(|x| x.to_string())?;
            for s in &out {
                w.write_sample(*s).map_err(|x| x.to_string())?;
            }
            w.finalize().map_err(|x| x.to_string())?;
        }
        // older renders of this vocal beyond the last few go (from disk and from memory)
        if e.rev > KEEP_RENDERS as u64 {
            let old = dir.join(format!("vocal_edit_{id}_{}.wav", e.rev - KEEP_RENDERS as u64));
            self.samples.remove(&old.to_string_lossy().into_owned());
            let _ = std::fs::remove_file(old);
        }
        let path = file.to_string_lossy().into_owned();
        let name = format!("{} (edited)", orig.name.trim_end_matches(" (edited)"));
        let new = Arc::new(Sample::new(name, path.clone(), orig.rate, out));
        self.samples.insert(path.clone(), new.clone());
        e.rendered = path;
        e.bypass = false;
        self.ve_set_audio(t, c, new);
        Ok(e.blobs.len())
    }

    pub(super) fn ve_dispatch(&mut self, name: &str, a: &A) -> Result<Val, String> {
        let eng = self.out.engine.clone();
        macro_rules! ok {
            ($v:expr) => {
                return Ok(Val::from($v))
            };
        }
        let (t, c) = (a.u(0), a.u(1));
        match name {
            "daw_ve_info" => {
                // (t, c) -> [] (not audio) | [edited 0/1, blobs, offset s, seconds per beat, recording seconds, comparing original 0/1,
                //            vibrato cents, vibrato Hz, glide ms, temperament, tonic, rendered 0/1]
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let Some(clip) = en.project.tracks.get(t).and_then(|x| x.clips.get(c)) else { ok!(Val::List(vec![])) };
                let Some(g) = geo_of(clip, en.project.bpm) else { ok!(Val::List(vec![])) };
                let ClipData::Audio { sample, .. } = &clip.data else { ok!(Val::List(vec![])) };
                let e = edit_id(clip).and_then(|id| load_edit(&en.project, &id));
                let secs = sample.seconds();
                let v = match e {
                    Some(e) => vec![1.0, e.blobs.len() as f64, g.offset_s, g.spb, secs, e.bypass as u8 as f64, e.vdepth, e.vrate, e.retune, e.temp as f64, e.troot as f64, (!e.rendered.is_empty()) as u8 as f64],
                    None => vec![0.0, 0.0, g.offset_s, g.spb, secs, 0.0, 0.0, 5.5, 12.0, 0.0, 0.0, 0.0],
                };
                ok!(Val::List(v.into_iter().map(Val::Num).collect()))
            }
            "daw_ve_analyze" => {
                // (t, c) -> number of notes found (0 = none; see the status line). Starts the edit over from the original recording.
                let (id, src) = {
                    let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    let Some(clip) = en.project.tracks.get(t).and_then(|x| x.clips.get(c)) else { ok!(0.0) };
                    let ClipData::Audio { sample, .. } = &clip.data else {
                        drop(en);
                        self.status = "Vocal editor: that is not an audio clip".into();
                        ok!(0.0)
                    };
                    let old = edit_id(clip).and_then(|id| load_edit(&en.project, &id).map(|e| (id, e.src)));
                    match old {
                        Some((id, src)) if !src.is_empty() => (id, src),
                        _ => {
                            let id = format!("{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() % 100_000_000).unwrap_or(0));
                            let src = if sample.path.is_empty() { format!("mem:vedit:{id}") } else { sample.path.clone() };
                            if sample.path.is_empty() {
                                self.samples.insert(src.clone(), sample.clone());
                            }
                            (id, src)
                        }
                    }
                };
                let (orig, ana) = self.ve_ana(&src)?;
                let found = vocal::detect_notes(&ana.pitch);
                if found.is_empty() {
                    self.status = "Vocal editor: no sung notes found in that clip".into();
                    ok!(0.0)
                }
                let lv = |a0: f64, a1: f64| -> f64 {
                    let (j0, j1) = ((a0 / ana.pitch.hop) as usize, ((a1 / ana.pitch.hop) as usize).min(ana.level.len()));
                    if j1 <= j0 { 0.0 } else { (ana.level[j0..j1].iter().cloned().fold(0.0, f32::max) / ana.top) as f64 }
                };
                let blobs: Vec<Blob> = found
                    .iter()
                    .map(|n| Blob { src_on: n.on, src_off: n.off, dst_on: n.on, dst_off: n.off, midi: n.midi, shift: 0.0, drift: 0.0, gain: 1.0, vel: lv(n.on, n.off).clamp(0.05, 1.0) })
                    .collect();
                let n = blobs.len();
                let e = Edit { src, blobs, vdepth: 0.0, vrate: 5.5, retune: 12.0, temp: 0, troot: 0, bypass: false, rendered: String::new(), rev: 0 };
                {
                    let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    if let Some(clip) = en.project.tracks.get_mut(t).and_then(|x| x.clips.get_mut(c)) {
                        clip.tag = format!("ved:{id}");
                        if let ClipData::Audio { sample, .. } = &mut clip.data {
                            *sample = orig;
                        }
                    }
                    // a later render continues the numbering, so undo never meets a file that was overwritten
                    let rev = meta_get(&en.project, &format!("ved.{id}")).map(|o| kv_num(&o, "rev", 0.0) as u64).unwrap_or(0);
                    let e = Edit { rev, ..e };
                    save_edit(&mut en.project, &id, &e);
                }
                self.touch();
                self.status = format!("Vocal editor: {n} notes");
                ok!(n)
            }
            "daw_ve_blobs" => {
                // (t, c) -> flat [src on, src off, dst on, dst off (s), detected pitch, shift (semitones), drift 0..1, gain, level 0..1] * N
                let Some((_, e, _)) = self.ve_get(t, c) else { ok!(Val::List(vec![])) };
                let mut v = Vec::with_capacity(e.blobs.len() * 9);
                for b in &e.blobs {
                    v.extend([b.src_on, b.src_off, b.dst_on, b.dst_off, b.midi, b.shift, b.drift, b.gain, b.vel].map(Val::Num));
                }
                ok!(Val::List(v))
            }
            "daw_ve_curve" => {
                // (t, c, blob, n) -> flat [pitch after the edit (fractional MIDI, -1 = unpitched), level 0..1] * n across the blob
                let Some((_, e, _)) = self.ve_get(t, c) else { ok!(Val::List(vec![])) };
                let Some(b) = e.blobs.get(a.u(2)).cloned() else { ok!(Val::List(vec![])) };
                let (_, ana) = self.ve_ana(&e.src)?;
                let n = a.u(3).clamp(2, 400);
                let mut v = Vec::with_capacity(n * 2);
                for k in 0..n {
                    let f = (k as f64 + 0.5) / n as f64;
                    let ts = b.src_on + f * (b.src_off - b.src_on);
                    let hz = ana.pitch.at(ts) as f64;
                    let m = if hz > 0.0 {
                        let m = 69.0 + 12.0 * (hz / 440.0).log2();
                        m + b.drift * (b.midi - m) + b.shift
                    } else {
                        -1.0
                    };
                    let j = ((ts / ana.pitch.hop) as usize).min(ana.level.len().saturating_sub(1));
                    let l = ana.level.get(j).copied().unwrap_or(0.0) / ana.top;
                    v.push(Val::Num(m));
                    v.push(Val::Num((l as f64 * b.gain).clamp(0.0, 1.5)));
                }
                ok!(Val::List(v))
            }
            "daw_ve_set" => {
                // (t, c, selection "i,j,..", key, value) -> true. Keys: shift (semitones, absolute), dshift (added), drift 0..1, gain
                // (linear), dgain (dB added), move (seconds added to the place, length kept), on / off (one blob's start / end, seconds)
                let Some((id, mut e, _)) = self.ve_get(t, c) else { ok!(false) };
                let n = e.blobs.len();
                let sel = sel_of(&a.s(2), n);
                let (key, v) = (a.s(3), a.n(4));
                match key.as_str() {
                    "shift" => sel.iter().for_each(|i| e.blobs[*i].shift = v.clamp(-36.0, 36.0)),
                    "dshift" => sel.iter().for_each(|i| e.blobs[*i].shift = (e.blobs[*i].shift + v).clamp(-36.0, 36.0)),
                    "drift" => sel.iter().for_each(|i| e.blobs[*i].drift = v.clamp(0.0, 1.0)),
                    "gain" => sel.iter().for_each(|i| e.blobs[*i].gain = v.clamp(0.0, 4.0)),
                    "dgain" => sel.iter().for_each(|i| e.blobs[*i].gain = (e.blobs[*i].gain * 10f64.powf(v / 20.0)).clamp(0.0, 4.0)),
                    "move" => {
                        // the selection moves as one block and stops at the first unselected neighbour on either side
                        let mut d = v;
                        let insel = |i: usize| sel.contains(&i);
                        for &i in &sel {
                            let b = &e.blobs[i];
                            d = d.max(-b.dst_on);
                            if let Some(p) = (0..i).rev().find(|j| !insel(*j)) {
                                d = d.max(e.blobs[p].dst_off - b.dst_on);
                            }
                            if let Some(q) = (i + 1..n).find(|j| !insel(*j)) {
                                d = d.min(e.blobs[q].dst_on - b.dst_off);
                            }
                        }
                        for &i in &sel {
                            e.blobs[i].dst_on += d;
                            e.blobs[i].dst_off += d;
                        }
                    }
                    "on" | "off" => {
                        let Some(&i) = sel.first() else { ok!(false) };
                        let prev_end = if i > 0 { e.blobs[i - 1].dst_off } else { 0.0 };
                        let next_on = e.blobs.get(i + 1).map(|b| b.dst_on).unwrap_or(f64::MAX);
                        let b = &mut e.blobs[i];
                        if key == "on" {
                            b.dst_on = v.clamp(prev_end, b.dst_off - MIN_LEN);
                        } else {
                            b.dst_off = v.clamp(b.dst_on + MIN_LEN, next_on);
                        }
                    }
                    _ => ok!(false),
                }
                tidy(&mut e.blobs);
                self.ve_put(&id, &e);
                ok!(true)
            }
            "daw_ve_split" => {
                // (t, c, blob, time s on the edited time line) -> true when the blob was cut in two there
                let Some((id, mut e, _)) = self.ve_get(t, c) else { ok!(false) };
                let (i, at) = (a.u(2), a.n(3));
                let Some(b) = e.blobs.get(i).cloned() else { ok!(false) };
                if at < b.dst_on + MIN_LEN || at > b.dst_off - MIN_LEN {
                    ok!(false)
                }
                let (_, ana) = self.ve_ana(&e.src)?;
                let f = (at - b.dst_on) / (b.dst_off - b.dst_on);
                let ts = b.src_on + f * (b.src_off - b.src_on);
                let mut l = b.clone();
                let mut r = b.clone();
                l.src_off = ts;
                l.dst_off = at;
                r.src_on = ts;
                r.dst_on = at;
                l.midi = median_midi(&ana.pitch, l.src_on, l.src_off, b.midi);
                r.midi = median_midi(&ana.pitch, r.src_on, r.src_off, b.midi);
                e.blobs[i] = l;
                e.blobs.insert(i + 1, r);
                self.ve_put(&id, &e);
                ok!(true)
            }
            "daw_ve_merge" => {
                // (t, c, blob) -> true when the blob and the one after it became one note
                let Some((id, mut e, _)) = self.ve_get(t, c) else { ok!(false) };
                let i = a.u(2);
                if i + 1 >= e.blobs.len() {
                    ok!(false)
                }
                let (_, ana) = self.ve_ana(&e.src)?;
                let r = e.blobs.remove(i + 1);
                let l = &mut e.blobs[i];
                let (ll, rl) = (l.dst_off - l.dst_on, r.dst_off - r.dst_on);
                l.src_off = r.src_off;
                l.dst_off = r.dst_off;
                if rl > ll {
                    l.shift = r.shift;
                }
                l.drift = l.drift.max(r.drift);
                l.gain = (l.gain * ll + r.gain * rl) / (ll + rl).max(1e-6);
                l.vel = l.vel.max(r.vel);
                l.midi = median_midi(&ana.pitch, l.src_on, l.src_off, l.midi);
                self.ve_put(&id, &e);
                ok!(true)
            }
            "daw_ve_correct" => {
                // (t, c, selection, centre 0..1, drift 0..1 (-1 = leave), scale index, tonic 0..11) -> notes changed.
                // Every note's centre goes that far toward the nearest note the scale allows (in the edit's temperament).
                let Some((id, mut e, _)) = self.ve_get(t, c) else { ok!(0.0) };
                let sel = sel_of(&a.s(2), e.blobs.len());
                let (centre, drift) = (a.n(3).clamp(0.0, 1.0), a.n(4));
                let allowed = allowed_of(a.u(5), (a.n(6) as i64).rem_euclid(12) as i32);
                let mut prev: Option<i32> = None;
                let mut changed = 0usize;
                let ed = e.clone();
                for i in sel {
                    let b = &mut e.blobs[i];
                    let cur = b.midi + b.shift;
                    let want = vocal::nearest_allowed(cur, &allowed, prev);
                    prev = Some(want);
                    let target = tempered(&ed, want);
                    let ns = b.shift + (target - cur) * centre;
                    if (ns - b.shift).abs() > 1e-4 || (drift >= 0.0 && (drift - b.drift).abs() > 1e-4) {
                        changed += 1;
                    }
                    b.shift = ns;
                    if drift >= 0.0 {
                        b.drift = drift.clamp(0.0, 1.0);
                    }
                }
                self.ve_put(&id, &e);
                ok!(changed)
            }
            "daw_ve_quantize" => {
                // (t, c, selection, grid in beats, strength 0..1) -> notes moved: starts go toward the grid, lengths stay
                let Some((id, mut e, g)) = self.ve_get(t, c) else { ok!(0.0) };
                let sel = sel_of(&a.s(2), e.blobs.len());
                let (grid, strength) = (a.n(3).max(1.0 / 64.0), a.n(4).clamp(0.0, 1.0));
                let mut moved = 0usize;
                for i in sel {
                    let b = &mut e.blobs[i];
                    let beat = g.beat(b.dst_on);
                    let d = (g.secs((beat / grid).round() * grid) - b.dst_on) * strength;
                    if d.abs() > 1e-4 {
                        b.dst_on += d;
                        b.dst_off += d;
                        moved += 1;
                    }
                }
                tidy(&mut e.blobs);
                self.ve_put(&id, &e);
                ok!(moved)
            }
            "daw_ve_guides" => {
                // (t, c) -> [track, name, kind (0 MIDI part, 1 edited vocal)] * N: what this vocal can be aligned to
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let Some(clip) = en.project.tracks.get(t).and_then(|x| x.clips.get(c)) else { ok!(Val::List(vec![])) };
                let (s, e) = (clip.start, clip.end());
                let mut v = vec![];
                for (ti, tr) in en.project.tracks.iter().enumerate() {
                    if ti == t || tr.inst_kind == "drums" {
                        continue;
                    }
                    if matches!(tr.kind, TrackKind::Audio) {
                        if tr.clips.iter().any(|k| k.start < e && k.end() > s && edit_id(k).is_some()) {
                            v.extend([Val::Num(ti as f64), Val::Str(tr.name.clone()), Val::Num(1.0)]);
                        }
                    } else if tr.clips.iter().flat_map(expand).any(|n| n.end() > s && n.start < e) {
                        v.extend([Val::Num(ti as f64), Val::Str(tr.name.clone()), Val::Num(0.0)]);
                    }
                }
                ok!(Val::List(v))
            }
            "daw_ve_align" => {
                // (t, c, guide track, pitch 0/1, timing 0/1, strength 0..1) -> notes changed, or -1 when the guide has nothing here.
                // Each sung note is matched to the guide note it overlaps most; the notes on one guide note keep their shares of it.
                let Some((id, mut e, g)) = self.ve_get(t, c) else { ok!(-1.0) };
                let (do_pitch, do_time, strength) = (a.n(3) > 0.5, a.n(4) > 0.5, a.n(5).clamp(0.0, 1.0));
                // guide notes in beats: (start, end, pitch)
                let guide: Vec<(f64, f64, f64)> = {
                    let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    let Some(gt) = en.project.tracks.get(a.u(2)) else { ok!(-1.0) };
                    if matches!(gt.kind, TrackKind::Audio) {
                        let mut v = vec![];
                        for k in &gt.clips {
                            if let (Some(gid), Some(gg)) = (edit_id(k), geo_of(k, en.project.bpm)) {
                                if let Some(ge) = load_edit(&en.project, &gid) {
                                    v.extend(ge.blobs.iter().map(|b| (gg.beat(b.dst_on), gg.beat(b.dst_off), b.midi + b.shift)).filter(|n| n.1 > k.start && n.0 < k.end()));
                                }
                            }
                        }
                        v
                    } else {
                        gt.clips.iter().flat_map(expand).map(|n| (n.start, n.end(), n.key as f64)).collect()
                    }
                };
                let window = (g.start, g.start + g.len);
                let guide: Vec<(f64, f64, f64)> = guide.into_iter().filter(|n| n.1 > window.0 - 1.0 && n.0 < window.1 + 1.0).collect();
                if guide.is_empty() {
                    ok!(-1.0)
                }
                // match every blob to a guide note
                let pick: Vec<Option<usize>> = e
                    .blobs
                    .iter()
                    .map(|b| {
                        let (bs, be) = (g.beat(b.dst_on), g.beat(b.dst_off));
                        let over = |n: &(f64, f64, f64)| (be.min(n.1) - bs.max(n.0)).max(0.0);
                        let best = guide.iter().enumerate().max_by(|x, y| over(x.1).total_cmp(&over(y.1)));
                        match best {
                            Some((k, n)) if over(n) > 0.0 => Some(k),
                            _ => guide.iter().enumerate().filter(|(_, n)| (n.0 - bs).abs() < 0.5).min_by(|x, y| (x.1 .0 - bs).abs().total_cmp(&(y.1 .0 - bs).abs())).map(|(k, _)| k),
                        }
                    })
                    .collect();
                let mut changed = 0usize;
                let ed = e.clone();
                let mut i = 0;
                while i < e.blobs.len() {
                    let Some(k) = pick[i] else {
                        i += 1;
                        continue;
                    };
                    let mut j = i;
                    while j + 1 < e.blobs.len() && pick[j + 1] == Some(k) {
                        j += 1;
                    }
                    let (gs, ge, gp) = guide[k];
                    if do_time {
                        // the group [i..=j] is remapped linearly onto the guide note
                        let (o0, o1) = (e.blobs[i].dst_on, e.blobs[j].dst_off);
                        let (n0, n1) = (o0 + (g.secs(gs) - o0) * strength, o1 + (g.secs(ge) - o1) * strength);
                        if n1 > n0 + MIN_LEN && o1 > o0 {
                            for b in &mut e.blobs[i..=j] {
                                let f0 = (b.dst_on - o0) / (o1 - o0);
                                let f1 = (b.dst_off - o0) / (o1 - o0);
                                b.dst_on = n0 + f0 * (n1 - n0);
                                b.dst_off = n0 + f1 * (n1 - n0);
                            }
                        }
                    }
                    if do_pitch {
                        for b in &mut e.blobs[i..=j] {
                            let cur = b.midi + b.shift;
                            // the guide's pitch in the singer's octave
                            let key = (gp + 12.0 * ((cur - gp) / 12.0).round()).round() as i32;
                            let target = if gp.fract().abs() < 1e-6 { tempered(&ed, key) } else { gp + 12.0 * ((cur - gp) / 12.0).round() };
                            b.shift += (target - cur) * strength;
                        }
                    }
                    changed += j - i + 1;
                    i = j + 1;
                }
                tidy(&mut e.blobs);
                self.ve_put(&id, &e);
                ok!(changed)
            }
            "daw_ve_opt" => {
                // (t, c, key, value): vdepth (cents), vrate (Hz), retune (glide ms), temp (0..4), troot (0..11)
                let Some((id, mut e, _)) = self.ve_get(t, c) else { ok!(false) };
                let v = a.n(3);
                match a.s(2).as_str() {
                    "vdepth" => e.vdepth = v.clamp(0.0, 100.0),
                    "vrate" => e.vrate = v.clamp(2.0, 9.0),
                    "retune" => e.retune = v.clamp(1.0, 200.0),
                    "temp" => e.temp = (v.max(0.0) as usize).min(vocal::TEMPERAMENTS.len() - 1),
                    "troot" => e.troot = (v as i64).rem_euclid(12) as i32,
                    _ => ok!(false),
                }
                self.ve_put(&id, &e);
                ok!(true)
            }
            "daw_ve_render" => {
                // (t, c) -> [1, notes, milliseconds] | [0, reason]
                let fail = |m: String| Ok(Val::List(vec![Val::Num(0.0), Val::Str(m)]));
                let Some((id, mut e, _)) = self.ve_get(t, c) else { return fail("analyse the vocal first".into()) };
                let t0 = std::time::Instant::now();
                match self.ve_render(t, c, &id, &mut e) {
                    Ok(n) => {
                        self.ve_put(&id, &e);
                        let ms = t0.elapsed().as_millis() as f64;
                        self.status = format!("Vocal editor: rendered {n} notes in {ms:.0} ms");
                        ok!(Val::List(vec![Val::Num(1.0), Val::Num(n as f64), Val::Num(ms)]))
                    }
                    Err(m) => fail(format!("could not render ({m})")),
                }
            }
            "daw_ve_compare" => {
                // (t, c, on 0/1) -> true: play the original recording (on) or the edit again (off), the edit is kept
                let Some((id, mut e, _)) = self.ve_get(t, c) else { ok!(false) };
                if a.n(2) > 0.5 {
                    let (orig, _) = self.ve_ana(&e.src.clone())?;
                    self.ve_set_audio(t, c, orig);
                    e.bypass = true;
                } else {
                    e.bypass = false;
                    let back = if e.rendered.is_empty() { None } else { self.load_sample_cached(&e.rendered.clone()).ok() };
                    match back {
                        Some(s) => self.ve_set_audio(t, c, s),
                        None => {
                            let _ = self.ve_render(t, c, &id, &mut e);
                        }
                    }
                }
                self.ve_put(&id, &e);
                ok!(true)
            }
            "daw_ve_reset" => {
                // (t, c, selection) -> true: the notes go back to where and how they were sung (every note when the selection is empty)
                let Some((id, mut e, _)) = self.ve_get(t, c) else { ok!(false) };
                for i in sel_of(&a.s(2), e.blobs.len()) {
                    let b = &mut e.blobs[i];
                    b.dst_on = b.src_on;
                    b.dst_off = b.src_off;
                    b.shift = 0.0;
                    b.drift = 0.0;
                    b.gain = 1.0;
                }
                tidy(&mut e.blobs);
                self.ve_put(&id, &e);
                ok!(true)
            }
            "daw_ve_to_midi" => {
                // (t, c) -> index of a new MIDI track with the notes as edited (pitch rounded, level as velocity), or -1
                let Some((_, e, g)) = self.ve_get(t, c) else { ok!(-1.0) };
                let notes: Vec<Note> = e
                    .blobs
                    .iter()
                    .filter_map(|b| {
                        let (s, en) = (g.beat(b.dst_on) - g.start, g.beat(b.dst_off) - g.start);
                        (en > 0.0 && s < g.len).then(|| Note::new(s.max(0.0), (en.min(g.len) - s.max(0.0)).max(0.05), (b.midi + b.shift).round().clamp(0.0, 127.0) as u8, (0.35 + 0.65 * (b.vel * b.gain).clamp(0.0, 1.0)) as f32))
                    })
                    .collect();
                if notes.is_empty() {
                    self.status = "Vocal editor: no notes inside the clip".into();
                    ok!(-1.0)
                }
                let sr_f = { eng.lock().unwrap_or_else(|p| p.into_inner()).sr };
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let n_tr = en.project.tracks.len();
                if n_tr >= MAX_TRACKS {
                    drop(en);
                    self.status = "Vocal editor: no room for another track".into();
                    ok!(-1.0)
                }
                let (tname, cname) = {
                    let tr = &en.project.tracks[t];
                    (tr.name.clone(), tr.clips.get(c).map(|k| k.name.clone()).unwrap_or_default())
                };
                let inst = crate::dsp::make_instrument("synth", sr_f);
                let mut mt = Track::new(&format!("{tname} notes"), TrackKind::Instrument, super::PALETTE[n_tr % 7], inst);
                mt.inst_kind = "synth".into();
                let mut mc = Clip::midi(g.start, g.len, &format!("{cname} notes"));
                let nn = notes.len();
                mc.data = ClipData::Midi(notes);
                mt.clips.push(mc);
                en.project.tracks.push(mt);
                drop(en);
                self.touch();
                self.status = format!("Vocal editor: {nn} notes written to a new MIDI track");
                ok!(n_tr)
            }
            _ => Err(format!("unknown builtin {name}")),
        }
    }
}
