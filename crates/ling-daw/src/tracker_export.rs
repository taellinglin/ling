//! Tracker module export: the project as a ProTracker `.mod`, ScreamTracker `.s3m`, FastTracker `.xm`, Impulse Tracker `.it` or OpenMPT
//! `.mptm` (written as an Impulse Tracker module with OpenMPT's version tag and its larger limits).
//!
//! Modules have no plugins, so every sound becomes samples, rendered offline from a copy of the project (the live engine is untouched):
//!
//! * an instrument track is sampled with its own instrument (dry, at full velocity, with the track's level baked in). Formats with
//!   keymapped instruments (XM, IT, MPTM) get a sample every few semitones over the range the track plays, each note mapped to the nearest;
//!   MOD and S3M (one sample per instrument) get ONE sample at the middle of the track's range that the notes transpose. A drum kit (Ling
//!   Membrane) is sampled per drum key instead and every hit plays its own sample untransposed. A Ling Slicer is not sampled through
//!   a held note: every slice it plays becomes its own sample straight from the slice audio, with its trims, fades, reverse, gain,
//!   the filter and its pitch baked in (dsp/slicer.rs `bake`); a slice the auto-tune moves gets one sample per shift it plays at
//!   (the shift the DAW gives the note at its beat, so chord and key changes stay), on the keys above the slices. The slices sit on
//!   their own keys (root + i) in one keymapped instrument (IT / MPTM; XM: 16 a chunk), or one instrument each (MOD, S3M), and play
//!   untransposed. Gated and looped slices get note-offs / cuts at the note's end (a looped slice keeps its loop), one-shots ring;
//!   the slices of a choke group share one channel so each one cuts the last, as in the DAW.
//! * every sample is the note held for the track's typical note length plus its release; a sound that sustains gets a loop on whole
//!   periods of its pitch near the end of the held part, at upward zero crossings with the best-matching waveform (IT: a sustain loop, so
//!   the release still plays after the note-off; XM, S3M, MOD: a normal loop, the sample ends there and the note-off fades or cuts it).
//! * an audio track is bounced alone (its warping, pitch, gain and effects as the DAW plays them) and chopped into segments of a few
//!   beats; each segment is a sample triggered on its own row and named with the tempo it was cut at ("Drums 128bpm 03"), so the audio
//!   stays in time through tempo changes and a long recording does not drift. Identical segments (a looped clip) share one sample.
//!
//! The arrangement becomes rows (`rpb` per beat, speed 24 / rpb ticks, tracker tempo = BPM), the tempo map becomes tempo commands, notes
//! are dealt to the lowest free channel of their track, velocities go into the volume column (MOD: command C), note ends become note-offs
//! (IT, XM) or cuts (S3M, MOD) where the sample would otherwise ring on its loop. When a format's limits are reached (31 samples in a MOD,
//! 99 in S3M / IT, 16 samples per XM instrument, the channel counts) the segments get longer, fewer pitches are sampled, rare drum keys
//! share samples, and anything dropped is listed in the report.
use crate::engine::Engine;
use crate::model::*;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Fmt {
    Mod,
    S3m,
    Xm,
    It,
    Mptm,
}

impl Fmt {
    pub fn parse(s: &str) -> Option<Fmt> {
        match s.trim().trim_start_matches('.').to_lowercase().as_str() {
            "mod" => Some(Fmt::Mod),
            "s3m" => Some(Fmt::S3m),
            "xm" => Some(Fmt::Xm),
            "it" => Some(Fmt::It),
            "mptm" => Some(Fmt::Mptm),
            _ => None,
        }
    }
    pub fn ext(self) -> &'static str {
        match self {
            Fmt::Mod => "mod",
            Fmt::S3m => "s3m",
            Fmt::Xm => "xm",
            Fmt::It => "it",
            Fmt::Mptm => "mptm",
        }
    }
    fn max_ch(self) -> usize {
        match self {
            Fmt::Mod | Fmt::S3m | Fmt::Xm => 32,
            Fmt::It | Fmt::Mptm => 64,
        }
    }
    /// samples the format can hold
    fn max_smp(self) -> usize {
        match self {
            Fmt::Mod => 31,
            Fmt::S3m | Fmt::It => 99,
            Fmt::Xm => 128 * 16,
            Fmt::Mptm => 240,
        }
    }
    fn max_ins(self) -> usize {
        match self {
            Fmt::Mod => 31,
            Fmt::S3m | Fmt::It => 99,
            Fmt::Xm => 128,
            Fmt::Mptm => 240,
        }
    }
    /// instruments map keys to several samples
    fn keymap(self) -> bool {
        matches!(self, Fmt::Xm | Fmt::It | Fmt::Mptm)
    }
    /// samples one instrument can map
    fn per_ins(self) -> usize {
        match self {
            Fmt::Xm => 16,
            Fmt::It | Fmt::Mptm => 120,
            _ => 1,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Opts {
    /// rows per beat (2, 4 or 8)
    pub rpb: u32,
    /// beats per audio segment
    pub seg_beats: f64,
    /// semitones between sampled pitches (keymapped formats)
    pub roots_every: u8,
    pub title: String,
}

impl Default for Opts {
    fn default() -> Opts {
        Opts { rpb: 4, seg_beats: 4.0, roots_every: 6, title: String::new() }
    }
}

impl Opts {
    /// "rpb=4;seg=4;roots=6;title=..."
    pub fn parse(s: &str) -> Opts {
        let mut o = Opts::default();
        for kv in s.split(';') {
            let Some((k, v)) = kv.split_once('=') else { continue };
            match k.trim() {
                "rpb" => o.rpb = v.trim().parse::<u32>().ok().filter(|r| [1, 2, 3, 4, 6, 8].contains(r)).unwrap_or(4),
                "seg" => o.seg_beats = v.trim().parse::<f64>().ok().filter(|x| *x >= 0.5).unwrap_or(4.0).min(64.0),
                "roots" => o.roots_every = v.trim().parse::<u8>().ok().filter(|x| (1..=24).contains(x)).unwrap_or(6),
                "title" => o.title = v.trim().to_string(),
                _ => {}
            }
        }
        o
    }
}

// ── the module, independent of the file format ─────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct XSmp {
    pub name: String,
    pub data: Vec<i16>,
    /// the rate the data plays at when triggered on `root`
    pub rate: f64,
    pub root: u8,
    /// loop (start, end) in frames
    pub lp: Option<(u32, u32)>,
    /// the data goes on after the loop (a release): IT plays it after the note-off
    pub tail: bool,
}

#[derive(Clone, Debug)]
pub struct XIns {
    pub name: String,
    /// MIDI key 0..119 -> sample index (into XSong::smps), usize::MAX = none
    pub map: Vec<usize>,
    /// -1..1
    pub pan: f32,
    /// seconds the sound takes to fade after a note-off (0 = no fade)
    pub release: f32,
}

pub const NOTE_OFF: i16 = 200;
pub const NOTE_CUT: i16 = 201;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum XFx {
    None,
    Tempo(u8),
    Speed(u8),
}

#[derive(Clone, Copy, Debug)]
pub struct XCell {
    /// -1 none, MIDI key, NOTE_OFF or NOTE_CUT
    pub note: i16,
    /// 1-based, 0 = none
    pub ins: u16,
    /// 0..64, -1 = none
    pub vol: i16,
    pub fx: XFx,
}

impl Default for XCell {
    fn default() -> XCell {
        XCell { note: -1, ins: 0, vol: -1, fx: XFx::None }
    }
}

#[derive(Debug)]
pub struct XSong {
    pub title: String,
    pub speed: u8,
    pub tempo: u8,
    pub rpb: u32,
    pub nch: usize,
    /// row -> channel
    pub rows: Vec<Vec<XCell>>,
    pub smps: Vec<XSmp>,
    pub ins: Vec<XIns>,
    pub chan_pan: Vec<f32>,
    pub report: Vec<String>,
}

// ── what the project plays ───────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Melodic,
    Kit,
    Audio,
    /// a Ling Slicer: the event keys are the samples' keys (see `slicer_plan`)
    Slices,
}

/// What a Ling Slicer track plays: per event its gate (a note end) and choke group; per sample key the (slice, cents) it bakes
#[derive(Clone, Debug, Default)]
struct SlcInfo {
    gate: Vec<bool>,
    choke: Vec<u8>,
    vars: HashMap<u8, (usize, i32)>,
}

#[derive(Clone, Debug)]
struct Ev {
    s: f64,
    e: f64,
    key: u8,
    vel: f32,
}

#[derive(Clone, Debug)]
struct Plan {
    t: usize,
    name: String,
    kind: Kind,
    evs: Vec<Ev>,
    /// audio: the clips' spans (beats)
    spans: Vec<(f64, f64)>,
    pan: f32,
    vol: f32,
    slc: Option<SlcInfo>,
}

/// A Slicer track's events on sample keys: each (slice, cents) it plays at gets a key - the unshifted slices their own key (root + i),
/// the shifted ones the keys above the last slice - and every event keeps its gate and choke group
fn slicer_plan(inst: &mut dyn crate::dsp::Instrument, evs: Vec<Ev>) -> (Vec<Ev>, SlcInfo) {
    let flat: Vec<f64> = evs.iter().flat_map(|e| [e.s, e.key as f64]).collect();
    let r = inst.command("tracker", &flat, "");
    let mut info = SlcInfo::default();
    let mut out = vec![];
    let mut shifted: Vec<(usize, i32)> = vec![];
    let root = inst.get_param("root").map(|v| (v * 127.0).round() as i32).unwrap_or(48);
    let nslices = inst.command("count", &[], "").first().copied().unwrap_or(0.0) as i32;
    for (e, v) in evs.iter().zip(r.chunks_exact(5)) {
        if v[0] < 0.0 {
            continue;
        }
        let (slice, cents) = (v[0] as usize, v[1] as i32);
        let key = if cents == 0 {
            root + slice as i32
        } else {
            let j = shifted.iter().position(|x| *x == (slice, cents)).unwrap_or_else(|| {
                shifted.push((slice, cents));
                shifted.len() - 1
            });
            root + nslices + j as i32
        };
        if !(0..120).contains(&key) {
            continue;
        }
        info.vars.insert(key as u8, (slice, cents));
        info.gate.push(v[2] > 0.5);
        info.choke.push(v[4] as u8);
        out.push(Ev { key: key as u8, ..e.clone() });
    }
    (out, info)
}

fn track_notes(t: &Track) -> Vec<Ev> {
    let mut out = vec![];
    for c in &t.clips {
        let ClipData::Midi(notes) = &c.data else { continue };
        let ll = if c.loop_len >= 0.25 && c.loop_len < c.len { c.loop_len } else { 0.0 };
        let reps = if ll > 0.0 { ((c.len / ll).ceil() as usize).min(4096) } else { 1 };
        for k in 0..reps {
            let base = c.start + k as f64 * ll;
            let limit = if ll > 0.0 { ll.min(c.len - k as f64 * ll) } else { c.len };
            for n in notes.iter().filter(|n| n.start < limit && n.key < 120) {
                let s = base + n.start;
                out.push(Ev { s, e: s + n.len.min(limit - n.start).max(0.02), key: n.key, vel: n.vel });
            }
        }
    }
    out.sort_by(|a, b| a.s.total_cmp(&b.s).then(a.key.cmp(&b.key)));
    out
}

fn plans(p: &mut Project) -> Vec<Plan> {
    let solo = p.tracks.iter().any(|t| t.solo);
    let mut out = vec![];
    for (ti, t) in p.tracks.iter_mut().enumerate() {
        if t.mute || (solo && !t.solo) || t.clips.is_empty() {
            continue;
        }
        if matches!(t.kind, TrackKind::Audio) {
            let spans: Vec<(f64, f64)> = t.clips.iter().filter(|c| matches!(c.data, ClipData::Audio { .. })).map(|c| (c.start, c.end())).collect();
            if !spans.is_empty() {
                out.push(Plan { t: ti, name: t.name.clone(), kind: Kind::Audio, evs: vec![], spans, pan: t.pan, vol: 1.0, slc: None });
            }
            continue;
        }
        let evs = track_notes(t);
        if evs.is_empty() || t.inst.is_none() {
            continue;
        }
        if t.inst_kind == "slicer" {
            let (evs, info) = slicer_plan(t.inst.as_mut().unwrap().as_mut(), evs);
            if !evs.is_empty() {
                out.push(Plan { t: ti, name: t.name.clone(), kind: Kind::Slices, evs, spans: vec![], pan: t.pan, vol: t.volume, slc: Some(info) });
            }
            continue;
        }
        let kit = t.inst_kind == "drums" || (t.inst_kind != "voice" && crate::tracker::looks_percussive(&t.name) && evs.iter().map(|e| e.key).collect::<std::collections::BTreeSet<_>>().len() <= 24);
        out.push(Plan { t: ti, name: t.name.clone(), kind: if kit { Kind::Kit } else { Kind::Melodic }, evs, spans: vec![], pan: t.pan, vol: t.volume, slc: None });
    }
    out
}

// ── rendering ────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct Rendered {
    data: Vec<f32>,
    /// frames the note was held
    held: usize,
    lp: Option<(usize, usize)>,
    /// the rate the data was made at (the engine's, a Slicer's file rate)
    rate: f32,
}

/// The offline copy of the project the samples are rendered from, with what has been rendered so far (shared by every format).
pub struct Renderer {
    eng: Engine,
    sr: f32,
    notes: HashMap<(usize, u8), Rendered>,
    audio: HashMap<usize, Vec<f32>>,
}

impl Renderer {
    pub fn new(eng: Engine) -> Renderer {
        let sr = eng.sr;
        Renderer { eng, sr, notes: HashMap::new(), audio: HashMap::new() }
    }

    fn hold_for(&self, plan: &Plan) -> f64 {
        // the track's typical note (the median), in seconds, within reason; drums ring out on their own
        if plan.kind == Kind::Kit {
            return 0.25;
        }
        let mut d: Vec<f64> = plan.evs.iter().map(|e| self.eng.project.beats_to_secs(e.s, e.e)).collect();
        d.sort_by(f64::total_cmp);
        d.get(d.len() / 2).copied().unwrap_or(0.5).clamp(0.4, 2.5)
    }

    fn note(&mut self, plan: &Plan, key: u8) -> Rendered {
        if let Some(r) = self.notes.get(&(plan.t, key)) {
            return r.clone();
        }
        // a Slicer's slice: baked from the slice audio (at the file's rate: the sample plays at that rate on its own key)
        if let Some((slice, cents)) = plan.slc.as_ref().and_then(|s| s.vars.get(&key)).copied() {
            let v = self.eng.project.tracks.get_mut(plan.t).and_then(|t| t.inst.as_mut()).map(|i| i.command("bake", &[slice as f64, cents as f64], "")).unwrap_or_default();
            let r = if v.len() > 3 {
                let data: Vec<f32> = v[3..].iter().map(|x| *x as f32 * plan.vol).collect();
                let lp = (v[1] >= 0.0).then(|| (v[1] as usize, v[2] as usize));
                Rendered { held: data.len(), data, lp, rate: v[0] as f32 }
            } else {
                Rendered { data: vec![], held: 0, lp: None, rate: self.sr }
            };
            self.notes.insert((plan.t, key), r.clone());
            return r;
        }
        let hold = self.hold_for(plan);
        let sr = self.sr;
        let bpm = self.eng.project.bpm;
        let mut out: Vec<f32> = vec![];
        let mut held = 0usize;
        if let Some(inst) = self.eng.project.tracks.get_mut(plan.t).and_then(|t| t.inst.as_mut()) {
            inst.set_tempo(bpm);
            inst.all_off();
            let mut buf = vec![0f32; 512];
            for _ in 0..16 {
                buf.fill(0.0);
                inst.render(&mut buf);
            }
            inst.note_on(key, 1.0);
            let held_frames = (hold * sr as f64) as usize;
            while out.len() / 2 < held_frames {
                buf.fill(0.0);
                inst.render(&mut buf);
                out.extend_from_slice(&buf);
            }
            held = out.len() / 2;
            inst.note_off(key);
            let max = held + (4.0 * sr) as usize;
            let quiet = |b: &[f32]| (b.iter().map(|x| x * x).sum::<f32>() / b.len().max(1) as f32).sqrt() < 2e-4;
            while out.len() / 2 < max {
                buf.fill(0.0);
                inst.render(&mut buf);
                out.extend_from_slice(&buf);
                if out.len() / 2 > held + (sr * 0.05) as usize && quiet(&out[out.len() - 2048.min(out.len())..]) {
                    break;
                }
            }
            inst.all_off();
        }
        let mut mono: Vec<f32> = out.chunks_exact(2).map(|f| (f[0] + f[1]) * 0.5 * plan.vol).collect();
        // the silence after the sound goes
        let end = mono.iter().rposition(|x| x.abs() > 1e-4).map(|i| i + 1).unwrap_or(0).max(held.min(mono.len()));
        mono.truncate(end);
        let lp = if plan.kind == Kind::Melodic { find_loop(&mono, held, sr, key) } else { None };
        let r = Rendered { data: mono, held, lp, rate: sr };
        self.notes.insert((plan.t, key), r.clone());
        r
    }

    /// the audio track bounced alone (mono), from beat 0
    fn audio(&mut self, t: usize) -> &Vec<f32> {
        if !self.audio.contains_key(&t) {
            let p = &mut self.eng.project;
            let was: Vec<(bool, bool)> = p.tracks.iter().map(|x| (x.mute, x.solo)).collect();
            for (i, tr) in p.tracks.iter_mut().enumerate() {
                tr.mute = i != t;
                tr.solo = false;
            }
            p.master_effects.clear();
            p.master_auto.clear();
            p.master_volume = 1.0;
            let end = p.tracks.get(t).map(|tr| tr.clips.iter().map(|c| c.end()).fold(0.0, f64::max)).unwrap_or(0.0);
            let st = self.eng.bounce_range(0.0, end, 0.5);
            for (tr, (m, s)) in self.eng.project.tracks.iter_mut().zip(was) {
                tr.mute = m;
                tr.solo = s;
            }
            self.audio.insert(t, st.chunks_exact(2).map(|f| (f[0] + f[1]) * 0.5).collect());
        }
        &self.audio[&t]
    }
}

/// A loop for a sustained sound: none when the sound decays (its level at the end of the held part is under a quarter of its peak).
/// Otherwise a span of whole periods of `key`'s pitch (about a third of a second, at most half the steady part) ending near 90 % of the
/// held part, both ends at upward zero crossings, the start chosen where the waveform before it best matches the one before the end.
pub fn find_loop(x: &[f32], held: usize, sr: f32, key: u8) -> Option<(usize, usize)> {
    if held < (sr * 0.2) as usize || x.len() < held {
        return None;
    }
    let win = (sr * 0.01) as usize;
    let rms = |a: usize, b: usize| (x[a..b].iter().map(|v| v * v).sum::<f32>() / (b - a).max(1) as f32).sqrt();
    let peak = (0..held / win).map(|i| rms(i * win, (i + 1) * win)).fold(0.0, f32::max);
    let tail = rms(held * 8 / 10, held);
    if peak <= 1e-4 || tail < peak * 0.25 {
        return None;
    }
    let hz = 440.0 * 2f32.powf((key as f32 - 69.0) / 12.0);
    let period = (sr / hz).max(2.0);
    let steady = held * 45 / 100;
    let room = (held * 92 / 100).saturating_sub(steady) as f32;
    let periods = ((sr * 0.33).min(room * 0.5) / period).floor().max(1.0);
    let len = (periods * period).round() as usize;
    let up = |i: usize| i >= 1 && i < x.len() && x[i - 1] <= 0.0 && x[i] > 0.0;
    let near = |c: usize, r: usize| -> Option<usize> { (0..=r).flat_map(|d| [c + d, c.saturating_sub(d)]).find(|i| *i >= 1 && *i < held && up(*i)) };
    let e = near(held * 92 / 100, period.ceil() as usize * 2)?;
    if e < len + 2 {
        return None;
    }
    let w = (period.round() as usize).clamp(8, 512);
    let score = |s: usize| -> f32 {
        if s < w {
            return f32::MIN;
        }
        let (a, b) = (&x[s - w..s], &x[e - w..e]);
        let dot: f32 = a.iter().zip(b).map(|(p, q)| p * q).sum();
        let na: f32 = a.iter().map(|p| p * p).sum::<f32>().sqrt();
        let nb: f32 = b.iter().map(|p| p * p).sum::<f32>().sqrt();
        dot / (na * nb).max(1e-9)
    };
    let c = e - len;
    let r = (period * 0.6).ceil() as usize;
    let mut best: Option<(usize, f32)> = None;
    for s in c.saturating_sub(r)..=(c + r).min(e - 2) {
        if !up(s) {
            continue;
        }
        let v = score(s);
        if best.map(|b| v > b.1).unwrap_or(true) {
            best = Some((s, v));
        }
    }
    let (s, _) = best?;
    (e > s + 2).then_some((s, e))
}

// ── building the module ──────────────────────────────────────────────────────

/// a request for one sample, before rendering
#[derive(Clone, Debug)]
enum Want {
    /// plan, key sampled
    Note(usize, u8),
    /// plan, segment (start, end beats)
    Seg(usize, f64, f64),
}

fn to_i16(x: &[f32], gain: f32) -> Vec<i16> {
    x.iter().map(|v| (v * gain * 32767.0).round().clamp(-32768.0, 32767.0) as i16).collect()
}

fn roots(keys: &[u8], every: u8) -> Vec<u8> {
    let (lo, hi) = (keys[0] as i32, keys[keys.len() - 1] as i32);
    if every >= 48 || hi - lo < every as i32 {
        // one pitch in the middle of the range (the median note)
        return vec![keys[keys.len() / 2]];
    }
    let e = every as i32;
    let mut r = vec![];
    let mut k = lo + e / 2;
    while k - e / 2 <= hi {
        r.push(k.clamp(0, 119) as u8);
        k += e;
    }
    r
}

/// The module for `fmt`: what to sample (within the format's limits), the samples rendered, the rows written.
pub fn build(rend: &mut Renderer, fmt: Fmt, o: &Opts) -> XSong {
    let mut report = vec![];
    let ps = plans(&mut rend.eng.project);
    let p = &rend.eng.project;
    let title = if o.title.is_empty() { p.name.clone() } else { o.title.clone() };
    let bpm0 = p.bpm;
    let tempo_map = p.tempo_map.clone();
    let end_beat = ps
        .iter()
        .map(|pl| pl.evs.iter().map(|e| e.e).fold(0.0, f64::max).max(pl.spans.iter().map(|s| s.1).fold(0.0, f64::max)))
        .fold(0.0, f64::max);
    let rpb = o.rpb.max(1);
    // what each plan wants sampled, shrunk until it fits the format
    let mut every = if fmt.keymap() { o.roots_every } else { 255 };
    let mut seg = o.seg_beats;
    let mut kit_keep = usize::MAX;
    let mut dropped: Vec<usize> = vec![];
    let used_keys = |pl: &Plan| -> Vec<u8> {
        let mut k: Vec<u8> = pl.evs.iter().map(|e| e.key).collect();
        k.sort_unstable();
        k.dedup();
        k
    };
    let kit_keys = |pl: &Plan, keep: usize| -> Vec<u8> {
        let mut count: HashMap<u8, usize> = HashMap::new();
        for e in &pl.evs {
            *count.entry(e.key).or_default() += 1;
        }
        let mut k: Vec<(u8, usize)> = count.into_iter().collect();
        k.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let mut keys: Vec<u8> = k.into_iter().take(keep.max(1)).map(|x| x.0).collect();
        keys.sort_unstable();
        keys
    };
    let segs = |pl: &Plan, len: f64| -> Vec<(f64, f64)> {
        let mut v = vec![];
        for (a, b) in &pl.spans {
            let mut s = *a;
            while s < b - 1e-6 {
                v.push((s, (s + len).min(*b)));
                s += len;
            }
        }
        v
    };
    let wants = |every: u8, seg: f64, kit_keep: usize, dropped: &[usize]| -> Vec<Vec<Want>> {
        ps.iter()
            .enumerate()
            .map(|(pi, pl)| {
                if dropped.contains(&pi) {
                    return vec![];
                }
                match pl.kind {
                    Kind::Melodic => roots(&used_keys(pl), every).into_iter().map(|k| Want::Note(pi, k)).collect(),
                    Kind::Kit | Kind::Slices => kit_keys(pl, kit_keep).into_iter().map(|k| Want::Note(pi, k)).collect(),
                    Kind::Audio => segs(pl, seg).into_iter().map(|(a, b)| Want::Seg(pi, a, b)).collect(),
                }
            })
            .collect()
    };
    let count_ins = |w: &[Vec<Want>]| -> usize { w.iter().map(|v| if v.is_empty() { 0 } else if fmt.keymap() { v.len().div_ceil(fmt.per_ins()) } else { v.len() }).sum() };
    let mut w = wants(every, seg, kit_keep, &dropped);
    let total = |w: &[Vec<Want>]| w.iter().map(|v| v.len()).sum::<usize>();
    let mut guard = 0;
    while (total(&w) > fmt.max_smp() || count_ins(&w) > fmt.max_ins()) && guard < 64 {
        guard += 1;
        let longest_audio = ps.iter().filter(|pl| pl.kind == Kind::Audio).map(|pl| pl.spans.iter().map(|s| s.1 - s.0).fold(0.0, f64::max)).fold(0.0, f64::max);
        if seg < longest_audio && seg < 64.0 {
            seg *= 2.0;
        } else if every < 12 {
            every = 12;
        } else if every < 255 {
            every = 255;
        } else if kit_keep > 4 {
            let biggest = ps.iter().filter(|pl| matches!(pl.kind, Kind::Kit | Kind::Slices)).map(|pl| used_keys(pl).len()).max().unwrap_or(0);
            kit_keep = if kit_keep == usize::MAX { biggest.saturating_sub(1).max(4) } else { kit_keep - 1 };
        } else if let Some(last) = (0..ps.len()).rev().find(|i| !dropped.contains(i)) {
            dropped.push(last);
            report.push(format!("left out \"{}\" ({} has no room for more samples)", ps[last].name, fmt.ext().to_uppercase()));
        } else {
            break;
        }
        w = wants(every, seg, kit_keep, &dropped);
    }
    if every != o.roots_every && fmt.keymap() {
        report.push(if every == 255 { "one sampled pitch per instrument (the format's limits)".into() } else { format!("a sample every {every} semitones (the format's limits)") });
    }
    if seg != o.seg_beats && ps.iter().any(|pl| pl.kind == Kind::Audio) {
        report.push(format!("audio cut into {seg} beat segments (the format's limits)"));
    }
    if kit_keep != usize::MAX {
        report.push(format!("drum kits keep their {kit_keep} most used sounds; the others play the nearest"));
        // the Slicer slices that lost their sample, by name
        for pl in ps.iter().filter(|pl| pl.kind == Kind::Slices) {
            let keep = kit_keys(pl, kit_keep);
            let Some(info) = pl.slc.as_ref() else { continue };
            let mut gone: Vec<String> = used_keys(pl).into_iter().filter(|k| !keep.contains(k)).filter_map(|k| info.vars.get(&k)).map(|(s, c)| if *c == 0 { format!("slice {}", s + 1) } else { format!("slice {} {:+}c", s + 1, c) }).collect();
            gone.dedup();
            if !gone.is_empty() {
                report.push(format!("\"{}\": no room for {} (they play the slice's own sample, or the nearest)", pl.name, gone.join(", ")));
            }
        }
    }

    // render the samples; one instrument per plan (keymapped: chunks of per_ins), or per sample
    let mut smps: Vec<XSmp> = vec![];
    // the samples as rendered (float): converted together at the end, with one gain, so the balance of the tracks stays
    let mut raw: Vec<Vec<f32>> = vec![];
    let mut ins: Vec<XIns> = vec![];
    // per plan: key -> (instrument 1-based, note to write); audio: segment start -> the same
    let mut note_map: Vec<HashMap<u8, (u16, u8)>> = vec![HashMap::new(); ps.len()];
    let mut seg_map: Vec<Vec<(f64, f64, u16, u8)>> = vec![vec![]; ps.len()];
    let mut looped_plan = vec![false; ps.len()];
    let mut release_of = vec![0f32; ps.len()];
    for (pi, list) in w.iter().enumerate() {
        if list.is_empty() {
            continue;
        }
        let pl = &ps[pi];
        let sr = rend.sr;
        // the samples of this plan: (XSmp, trigger key)
        let mut made: Vec<(usize, u8)> = vec![];
        let mut seg_sample: Vec<(f64, f64, usize)> = vec![];
        for want in list {
            match want {
                Want::Note(_, key) => {
                    let r = rend.note(pl, *key);
                    if r.data.is_empty() {
                        continue;
                    }
                    release_of[pi] = release_of[pi].max((r.data.len().saturating_sub(r.held)) as f32 / sr);
                    let (data, lp, tail) = match r.lp {
                        Some((a, b)) => {
                            looped_plan[pi] = true;
                            // IT keeps the release after a sustain loop; the others end at the loop
                            let keep = if matches!(fmt, Fmt::It | Fmt::Mptm) { r.data.len() } else { b };
                            (r.data[..keep].to_vec(), Some((a as u32, b as u32)), keep > b)
                        }
                        None => (r.data.clone(), None, false),
                    };
                    let name = match pl.slc.as_ref().and_then(|s| s.vars.get(key)) {
                        Some((s, 0)) => format!("{} slice {}", pl.name, s + 1),
                        Some((s, c)) => format!("{} slice {} {:+}c", pl.name, s + 1, c),
                        None => format!("{} {}{}", pl.name, crate::theory::SHARP[(*key % 12) as usize], (*key / 12) as i32 - 1),
                    };
                    smps.push(XSmp { name, data: vec![], rate: r.rate as f64, root: *key, lp, tail });
                    raw.push(data);
                    made.push((smps.len() - 1, *key));
                }
                Want::Seg(_, a, b) => {
                    let (fa, fb) = {
                        let pr = &rend.eng.project;
                        ((pr.beats_to_secs(0.0, *a) * sr as f64) as usize, (pr.beats_to_secs(0.0, *b) * sr as f64) as usize)
                    };
                    let audio = rend.audio(pl.t);
                    if fa >= audio.len() {
                        continue;
                    }
                    let piece: Vec<f32> = audio[fa..fb.min(audio.len())].to_vec();
                    let loud = (piece.iter().map(|v| v * v).sum::<f32>() / piece.len().max(1) as f32).sqrt();
                    if loud < 1e-4 {
                        continue;
                    }
                    // a segment that sounds like an earlier one (a looped clip) shares its sample
                    let same = seg_sample.iter().find(|(_, _, si)| {
                        let d = &raw[*si];
                        d.len() == piece.len() && d.iter().zip(&piece).all(|(x, y)| (x - y).abs() < 2e-3)
                    });
                    if let Some((_, _, si)) = same {
                        seg_sample.push((*a, *b, *si));
                        continue;
                    }
                    let bpm = rend.eng.project.bpm_at(*a);
                    let n = seg_sample.iter().map(|x| x.2).collect::<std::collections::BTreeSet<_>>().len() + 1;
                    smps.push(XSmp { name: format!("{} {:.0}bpm {:02}", pl.name, bpm, n), data: vec![], rate: sr as f64, root: 60, lp: None, tail: false });
                    raw.push(piece);
                    seg_sample.push((*a, *b, smps.len() - 1));
                }
            }
        }
        // the audio segments' trigger keys: one key each inside its instrument (keymapped), or C-5 on its own instrument
        let mut seg_samples: Vec<usize> = seg_sample.iter().map(|x| x.2).collect();
        seg_samples.sort_unstable();
        seg_samples.dedup();
        if pl.kind == Kind::Audio {
            for (j, si) in seg_samples.iter().enumerate() {
                let key = if fmt.keymap() { (24 + j % fmt.per_ins().min(96)) as u8 } else { 60 };
                smps[*si].root = key;
                made.push((*si, key));
            }
        }
        if made.is_empty() {
            continue;
        }
        let pan = pl.pan.clamp(-1.0, 1.0);
        let release = release_of[pi].clamp(0.02, 4.0);
        if fmt.keymap() {
            for (ci, chunk) in made.chunks(fmt.per_ins()).enumerate() {
                let mut map = vec![usize::MAX; 120];
                match pl.kind {
                    Kind::Melodic => {
                        // every key to the nearest sampled pitch
                        for (k, slot) in map.iter_mut().enumerate() {
                            *slot = chunk.iter().min_by_key(|(_, r)| (*r as i32 - k as i32).abs()).map(|x| x.0).unwrap_or(usize::MAX);
                        }
                    }
                    _ => {
                        for (si, key) in chunk {
                            map[*key as usize] = *si;
                        }
                    }
                }
                let name = if made.len() > fmt.per_ins() { format!("{} {}", pl.name, ci + 1) } else { pl.name.clone() };
                ins.push(XIns { name, map, pan, release });
                let inum = ins.len() as u16;
                for (si, key) in chunk {
                    match pl.kind {
                        Kind::Melodic => {}
                        Kind::Kit | Kind::Slices => {
                            note_map[pi].insert(*key, (inum, *key));
                        }
                        Kind::Audio => {
                            for s in seg_sample.iter().filter(|x| x.2 == *si) {
                                seg_map[pi].push((s.0, s.1, inum, *key));
                            }
                        }
                    }
                }
                if pl.kind == Kind::Melodic {
                    for k in 0..120u8 {
                        note_map[pi].insert(k, (inum, k));
                    }
                }
            }
        } else {
            for (si, key) in &made {
                let mut map = vec![usize::MAX; 120];
                for slot in map.iter_mut() {
                    *slot = *si;
                }
                ins.push(XIns { name: smps[*si].name.clone(), map, pan, release });
                let inum = ins.len() as u16;
                match pl.kind {
                    Kind::Melodic => {
                        for k in 0..120u8 {
                            note_map[pi].insert(k, (inum, k));
                        }
                    }
                    Kind::Kit | Kind::Slices => {
                        note_map[pi].insert(*key, (inum, *key));
                    }
                    Kind::Audio => {
                        for s in seg_sample.iter().filter(|x| x.2 == *si) {
                            seg_map[pi].push((s.0, s.1, inum, *key));
                        }
                    }
                }
            }
        }
        // a shifted slice that lost its sample plays the slice's own sample
        if let (Kind::Slices, Some(info)) = (pl.kind, pl.slc.as_ref()) {
            for e in &pl.evs {
                if note_map[pi].contains_key(&e.key) {
                    continue;
                }
                let Some((slice, _)) = info.vars.get(&e.key) else { continue };
                if let Some(v) = info.vars.iter().find(|(k, (s, c))| s == slice && *c == 0 && note_map[pi].contains_key(k)).and_then(|(k, _)| note_map[pi].get(k)).copied() {
                    note_map[pi].insert(e.key, v);
                }
            }
        }
        // a kit key that was not sampled plays the nearest one that was
        if matches!(pl.kind, Kind::Kit | Kind::Slices) {
            let have: Vec<(u8, (u16, u8))> = note_map[pi].iter().map(|(k, v)| (*k, *v)).collect();
            for e in &pl.evs {
                if !note_map[pi].contains_key(&e.key) {
                    if let Some((_, v)) = have.iter().min_by_key(|(k, _)| (*k as i32 - e.key as i32).abs()) {
                        note_map[pi].insert(e.key, *v);
                    }
                }
            }
        }
    }

    let peak = raw.iter().flatten().fold(0f32, |m, v| m.max(v.abs()));
    let gain = if peak > 1e-6 { 0.98 / peak } else { 1.0 };
    for (x, r) in smps.iter_mut().zip(&raw) {
        x.data = to_i16(r, gain);
    }
    // the events of every plan as rows: (on row, off row, note, instrument, volume)
    let rows_total = ((end_beat * rpb as f64).ceil() as usize + 1).max(1);
    // (on row, off row, note, instrument, volume, gate: a note end is written, choke group)
    let mut lanes_of: Vec<Vec<Vec<(usize, usize, u8, u16, i16, bool, u8)>>> = vec![];
    for (pi, pl) in ps.iter().enumerate() {
        let mut items: Vec<(usize, usize, u8, u16, i16, bool, u8)> = vec![];
        if pl.kind == Kind::Audio {
            for (a, b, inum, key) in &seg_map[pi] {
                let on = (a * rpb as f64).round() as usize;
                let off = ((b * rpb as f64).round() as usize).max(on + 1);
                items.push((on, off, *key, *inum, 64, false, 0));
            }
        } else {
            for (ei, e) in pl.evs.iter().enumerate() {
                let Some((inum, note)) = note_map[pi].get(&e.key) else { continue };
                let on = (e.s * rpb as f64).round() as usize;
                let off = ((e.e * rpb as f64).round() as usize).max(on + 1);
                let (gate, choke) = pl.slc.as_ref().map(|s| (s.gate[ei], s.choke[ei])).unwrap_or((false, 0));
                items.push((on, off, *note, *inum, (e.vel.clamp(0.0, 1.0) * 64.0).round().max(1.0) as i16, gate, choke));
            }
        }
        items.sort_by_key(|x| (x.0, x.2));
        lanes_of.push(vec![]);
        let lanes = lanes_of.last_mut().unwrap();
        let mut ends: Vec<usize> = vec![];
        // a Slicer's choke group keeps one channel: the next slice of the group cuts the one sounding
        let mut group_lane: HashMap<u8, usize> = HashMap::new();
        for it in items {
            if it.6 > 0 {
                let li = match group_lane.get(&it.6) {
                    Some(l) => *l,
                    None if lanes.len() < 8 => {
                        lanes.push(vec![]);
                        ends.push(usize::MAX);
                        group_lane.insert(it.6, lanes.len() - 1);
                        lanes.len() - 1
                    }
                    None => continue,
                };
                if lanes[li].last().map(|x| x.0 == it.0).unwrap_or(false) {
                    continue;
                }
                lanes[li].push(it);
                continue;
            }
            let free = ends.iter().position(|e| *e <= it.0);
            let li = match free {
                Some(l) => l,
                None if lanes.len() < 8 => {
                    lanes.push(vec![]);
                    ends.push(0);
                    lanes.len() - 1
                }
                None => (0..lanes.len()).min_by_key(|l| ends[*l]).unwrap_or(0),
            };
            if lanes[li].last().map(|x| x.0 == it.0).unwrap_or(false) {
                continue;
            }
            ends[li] = it.1;
            lanes[li].push(it);
        }
    }
    // fewer voices per track until the channels fit
    let mut cap = 8usize;
    while lanes_of.iter().map(|l| l.len().min(cap)).sum::<usize>() > fmt.max_ch() && cap > 1 {
        cap -= 1;
    }
    let mut lost = 0usize;
    for l in lanes_of.iter_mut() {
        while l.len() > cap {
            lost += l.pop().map(|x| x.len()).unwrap_or(0);
        }
    }
    let mut nch = lanes_of.iter().map(|l| l.len()).sum::<usize>().min(fmt.max_ch());
    if lost > 0 {
        report.push(format!("{lost} overlapping notes did not fit in {} channels", fmt.max_ch()));
    }
    nch = nch.max(1);
    if fmt == Fmt::Mod {
        // ProTracker channel counts are even
        nch = (nch + nch % 2).max(4);
    }
    let mut rows = vec![vec![XCell::default(); nch]; rows_total];
    let mut chan_pan = vec![0f32; nch];
    let mut ch = 0usize;
    for (pi, lanes) in lanes_of.iter().enumerate() {
        for lane in lanes {
            if ch >= nch {
                break;
            }
            chan_pan[ch] = ps[pi].pan;
            for (k, (on, off, note, inum, vol, gate, _)) in lane.iter().enumerate() {
                if *on >= rows_total {
                    continue;
                }
                rows[*on][ch] = XCell { note: *note as i16, ins: *inum, vol: *vol, fx: XFx::None };
                // the end of the note, where the sound would otherwise go on: before the next note of the lane
                let next = lane.get(k + 1).map(|x| x.0).unwrap_or(usize::MAX);
                let needs = match ps[pi].kind {
                    Kind::Melodic => matches!(fmt, Fmt::It | Fmt::Mptm | Fmt::Xm) || looped_plan[pi],
                    Kind::Slices => *gate,
                    _ => false,
                };
                if needs && *off < next && *off < rows_total && rows[*off][ch].note < 0 {
                    rows[*off][ch].note = if matches!(fmt, Fmt::It | Fmt::Mptm | Fmt::Xm) { NOTE_OFF } else { NOTE_CUT };
                }
            }
            ch += 1;
        }
    }
    // tempo: the tracker tempo is the BPM at speed 24 / rpb (4 rows a beat: speed 6)
    let speed = (24 / rpb).clamp(1, 31) as u8;
    let tval = |b: f64| (b * (speed as f64 * rpb as f64) / 24.0).round().clamp(32.0, 255.0) as u8;
    let tempo = tval(bpm0);
    if (bpm0 * speed as f64 * rpb as f64 / 24.0) > 255.5 || bpm0 * speed as f64 * rpb as f64 / 24.0 < 31.5 {
        report.push(format!("{bpm0:.0} BPM is outside the tracker tempo range at {rpb} rows per beat"));
    }
    for (b, v) in &tempo_map {
        let r = (b * rpb as f64).round() as usize;
        if r < rows_total {
            if let Some(c) = (0..nch).find(|c| rows[r][*c].fx == XFx::None) {
                rows[r][c].fx = XFx::Tempo(tval(*v));
            }
        }
    }
    if !tempo_map.is_empty() {
        report.push(format!("{} tempo change(s) as tempo commands", tempo_map.len()));
    }
    XSong { title, speed, tempo, rpb, nch, rows, smps, ins, chan_pan, report }
}

// ── writers ──────────────────────────────────────────────────────────────────

fn put_str(out: &mut Vec<u8>, s: &str, n: usize) {
    let b: Vec<u8> = s.chars().map(|c| if c.is_ascii() && !c.is_ascii_control() { c as u8 } else { b'_' }).take(n).collect();
    out.extend_from_slice(&b);
    out.extend(std::iter::repeat_n(0u8, n - b.len()));
}
fn put16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn set32(out: &mut [u8], at: usize, v: u32) {
    out[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn set16(out: &mut [u8], at: usize, v: u16) {
    out[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

fn patterns(s: &XSong) -> Vec<&[Vec<XCell>]> {
    s.rows.chunks(64).collect()
}

/// a mixing volume for `nch` channels: `full` up to 6 channels, then falling with the square root of the count (so 24 channels get half)
fn mix_volume(full: f32, nch: usize) -> u8 {
    (full * (6.0 / nch.max(6) as f32).sqrt()).round().clamp(32.0, 128.0) as u8
}

/// rate at MIDI 60 for a sample (what C5Speed / C2Spd mean)
fn rate_at_60(x: &XSmp) -> f64 {
    x.rate * 2f64.powf((60.0 - x.root as f64) / 12.0)
}

/// Impulse Tracker (`mptm`: the same with OpenMPT's version tag)
pub fn write_it(s: &XSong, mptm: bool) -> Vec<u8> {
    let pats = patterns(s);
    let (nord, nins, nsmp, npat) = (pats.len() + 1, s.ins.len(), s.smps.len(), pats.len());
    let mut o = vec![];
    o.extend_from_slice(b"IMPM");
    put_str(&mut o, &s.title, 26);
    o.push(s.rpb as u8);
    o.push((s.rpb * 4) as u8);
    put16(&mut o, nord as u16);
    put16(&mut o, nins as u16);
    put16(&mut o, nsmp as u16);
    put16(&mut o, npat as u16);
    put16(&mut o, if mptm { 0x5130 } else { 0x0214 });
    put16(&mut o, 0x0214);
    put16(&mut o, 1 | 4 | 8); // stereo, instruments, linear slides
    put16(&mut o, 0);
    o.push(128); // global volume
    // mixing volume: the samples are normalized together, so about the DAW's level with a few channels, less as more play at once
    o.push(mix_volume(96.0, s.nch));
    o.push(s.speed);
    o.push(s.tempo);
    o.push(128);
    o.push(0);
    put16(&mut o, 0);
    put32(&mut o, 0);
    put32(&mut o, 0);
    for c in 0..64 {
        o.push(if c < s.nch { (32.0 + s.chan_pan[c] * 32.0).round().clamp(0.0, 64.0) as u8 } else { 128 + 32 });
    }
    for _ in 0..64 {
        o.push(64);
    }
    for p in 0..npat {
        o.push(p as u8);
    }
    o.push(255);
    let ptr_ins = o.len();
    o.resize(o.len() + 4 * (nins + nsmp + npat), 0);
    let ptr_smp = ptr_ins + 4 * nins;
    let ptr_pat = ptr_smp + 4 * nsmp;
    // instruments
    for (i, ins) in s.ins.iter().enumerate() {
        let at = o.len();
        set32(&mut o, ptr_ins + 4 * i, at as u32);
        let mut h = vec![0u8; 554];
        h[0..4].copy_from_slice(b"IMPI");
        h[0x11] = 3; // new note action: fade (the previous note fades out under the next one)
        let fade = if ins.release > 0.0 { (2048.0 * 2.5 / (s.tempo as f32 * ins.release * 1.5)).round().clamp(1.0, 1024.0) as u16 } else { 0 };
        set16(&mut h, 0x14, fade);
        h[0x17] = 60;
        h[0x18] = 128;
        h[0x19] = (32.0 + ins.pan * 32.0).round().clamp(0.0, 64.0) as u8;
        set16(&mut h, 0x1c, 0x0214);
        let used: std::collections::BTreeSet<usize> = ins.map.iter().copied().filter(|x| *x != usize::MAX).collect();
        h[0x1e] = used.len().min(255) as u8;
        let mut name = vec![];
        put_str(&mut name, &ins.name, 26);
        h[0x20..0x3a].copy_from_slice(&name);
        h[0x3a] = 0; // no filter
        h[0x3b] = 0;
        for k in 0..120usize {
            let si = ins.map[k];
            h[0x40 + 2 * k] = k as u8;
            h[0x41 + 2 * k] = if si == usize::MAX { 0 } else { (si + 1).min(255) as u8 };
        }
        // envelopes off
        o.extend_from_slice(&h);
    }
    // sample headers (the data comes after the patterns)
    let mut smp_hdr_at = vec![];
    for (i, x) in s.smps.iter().enumerate() {
        let at = o.len();
        set32(&mut o, ptr_smp + 4 * i, at as u32);
        smp_hdr_at.push(at);
        let mut h = vec![0u8; 80];
        h[0..4].copy_from_slice(b"IMPS");
        h[0x11] = 64;
        let mut fl = 1 | 2; // has data, 16-bit
        if let Some(_lp) = x.lp {
            fl |= if x.tail { 32 } else { 16 };
        }
        h[0x12] = fl;
        h[0x13] = 64;
        let mut name = vec![];
        put_str(&mut name, &x.name, 26);
        h[0x14..0x2e].copy_from_slice(&name);
        h[0x2e] = 1; // signed
        h[0x2f] = 32;
        set32(&mut h, 0x30, x.data.len() as u32);
        if let Some((a, b)) = x.lp {
            let (la, lb) = if x.tail { (0x40, 0x44) } else { (0x34, 0x38) };
            set32(&mut h, la, a);
            set32(&mut h, lb, b);
        }
        set32(&mut h, 0x3c, rate_at_60(x).round().clamp(256.0, 9_999_999.0) as u32);
        o.extend_from_slice(&h);
    }
    // patterns
    for (pi, pat) in pats.iter().enumerate() {
        let at = o.len();
        set32(&mut o, ptr_pat + 4 * pi, at as u32);
        let mut data = vec![];
        for row in pat.iter() {
            for (c, cell) in row.iter().enumerate() {
                let mut mask = 0u8;
                if cell.note >= 0 {
                    mask |= 1;
                }
                if cell.ins > 0 {
                    mask |= 2;
                }
                if cell.vol >= 0 {
                    mask |= 4;
                }
                if cell.fx != XFx::None {
                    mask |= 8;
                }
                if mask == 0 {
                    continue;
                }
                data.push((c as u8 + 1) | 0x80);
                data.push(mask);
                if mask & 1 != 0 {
                    data.push(match cell.note {
                        NOTE_OFF => 255,
                        NOTE_CUT => 254,
                        n => n.clamp(0, 119) as u8,
                    });
                }
                if mask & 2 != 0 {
                    data.push(cell.ins.min(255) as u8);
                }
                if mask & 4 != 0 {
                    data.push(cell.vol.clamp(0, 64) as u8);
                }
                if mask & 8 != 0 {
                    match cell.fx {
                        XFx::Tempo(t) => data.extend([20, t]),
                        XFx::Speed(v) => data.extend([1, v]),
                        XFx::None => data.extend([0, 0]),
                    }
                }
            }
            data.push(0);
        }
        put16(&mut o, data.len() as u16);
        put16(&mut o, pat.len() as u16);
        put32(&mut o, 0);
        o.extend_from_slice(&data);
    }
    // sample data
    for (i, x) in s.smps.iter().enumerate() {
        let at = o.len() as u32;
        set32(&mut o, smp_hdr_at[i] + 0x48, at);
        for v in &x.data {
            o.extend_from_slice(&v.to_le_bytes());
        }
    }
    o
}

/// FastTracker 2
pub fn write_xm(s: &XSong) -> Vec<u8> {
    let pats = patterns(s);
    let mut nch = s.nch.max(2);
    nch += nch % 2;
    let mut o = vec![];
    o.extend_from_slice(b"Extended Module: ");
    put_str(&mut o, &s.title, 20);
    o.push(0x1a);
    put_str(&mut o, "LingStationX", 20);
    put16(&mut o, 0x0104);
    put32(&mut o, 276);
    put16(&mut o, pats.len() as u16);
    put16(&mut o, 0);
    put16(&mut o, nch as u16);
    put16(&mut o, pats.len() as u16);
    put16(&mut o, s.ins.len() as u16);
    put16(&mut o, 1); // linear frequencies
    put16(&mut o, s.speed as u16);
    put16(&mut o, s.tempo as u16);
    for i in 0..256 {
        o.push(if i < pats.len() { i as u8 } else { 0 });
    }
    for pat in &pats {
        let mut data = vec![];
        for row in pat.iter() {
            for c in 0..nch {
                let cell = row.get(c).copied().unwrap_or_default();
                let note = match cell.note {
                    -1 => 0u8,
                    NOTE_OFF | NOTE_CUT => 97,
                    n => (n - 11).clamp(1, 96) as u8,
                };
                let (eff, par) = match cell.fx {
                    XFx::Tempo(t) => (0x0f, t),
                    XFx::Speed(v) => (0x0f, v.min(31)),
                    XFx::None => (0, 0),
                };
                let vol = if cell.vol >= 0 { 0x10 + cell.vol.clamp(0, 64) as u8 } else { 0 };
                let ins = cell.ins.min(128) as u8;
                let mut mask = 0x80u8;
                if note != 0 {
                    mask |= 1;
                }
                if ins != 0 {
                    mask |= 2;
                }
                if vol != 0 {
                    mask |= 4;
                }
                if eff != 0 {
                    mask |= 8 | 16;
                }
                data.push(mask);
                for (bit, v) in [(1u8, note), (2, ins), (4, vol), (8, eff), (16, par)] {
                    if mask & bit != 0 {
                        data.push(v);
                    }
                }
            }
        }
        put32(&mut o, 9);
        o.push(0);
        put16(&mut o, pat.len() as u16);
        put16(&mut o, data.len() as u16);
        o.extend_from_slice(&data);
    }
    for ins in &s.ins {
        // the samples of this instrument, in their order in the song
        let mut list: Vec<usize> = ins.map.iter().copied().filter(|x| *x != usize::MAX).collect();
        list.sort_unstable();
        list.dedup();
        list.truncate(16);
        let start = o.len();
        put32(&mut o, if list.is_empty() { 29 } else { 263 });
        put_str(&mut o, &ins.name, 22);
        o.push(0);
        put16(&mut o, list.len() as u16);
        if list.is_empty() {
            continue;
        }
        put32(&mut o, 40);
        for k in 0..96usize {
            // XM note k+1 is MIDI k + 12
            let si = ins.map.get(k + 12).copied().unwrap_or(usize::MAX);
            o.push(list.iter().position(|x| *x == si).unwrap_or(0) as u8);
        }
        // a volume envelope held at full until the key-off, so the fade-out applies (FT2 only fades with an envelope on)
        // every instrument: notes that get a key-off (the melodic ones) fade instead of stopping dead
        let looped = true;
        let mut env = vec![0u8; 48];
        set16(&mut env, 0, 0);
        set16(&mut env, 2, 64);
        set16(&mut env, 4, 1);
        set16(&mut env, 6, 64);
        o.extend_from_slice(&env);
        o.extend_from_slice(&[0u8; 48]);
        o.push(if looped { 2 } else { 0 });
        o.push(0);
        o.extend_from_slice(&[1, 0, 0, 0, 0, 0]); // vol sustain point 1, loops, pan
        o.push(if looped { 1 | 2 } else { 0 });
        o.push(0);
        o.extend_from_slice(&[0, 0, 0, 0]); // vibrato
        let fade = if looped && ins.release > 0.0 { (32768.0 * 2.5 / (s.tempo as f32 * ins.release * 1.5)).round().clamp(1.0, 4095.0) as u16 } else { 0 };
        put16(&mut o, fade);
        put16(&mut o, 0);
        o.resize(start + 263, 0);
        for si in &list {
            let x = &s.smps[*si];
            // relative note + finetune: the rate at C-4 (MIDI 60) relative to 8363 Hz
            let semis = 12.0 * (rate_at_60(x) / 8363.0).log2();
            let rel = semis.round().clamp(-96.0, 95.0);
            let fine = ((semis - rel) * 128.0).round().clamp(-128.0, 127.0);
            put32(&mut o, (x.data.len() * 2) as u32);
            let (ls, ll) = x.lp.map(|(a, b)| (a * 2, (b - a) * 2)).unwrap_or((0, 0));
            put32(&mut o, ls);
            put32(&mut o, ll);
            o.push(64);
            o.push(fine as i8 as u8);
            o.push(16 | if x.lp.is_some() { 1 } else { 0 });
            o.push((128.0 + ins.pan * 127.0).round().clamp(0.0, 255.0) as u8);
            o.push(rel as i8 as u8);
            o.push(0);
            put_str(&mut o, &x.name, 22);
        }
        for si in &list {
            let mut prev = 0i16;
            for v in &s.smps[*si].data {
                o.extend_from_slice(&v.wrapping_sub(prev).to_le_bytes());
                prev = *v;
            }
        }
    }
    o
}

/// note offset of S3M: its note byte is octave * 12 + semitone of MIDI key - S3M_SHIFT
const S3M_SHIFT: i16 = 12;

/// ScreamTracker 3
pub fn write_s3m(s: &XSong) -> Vec<u8> {
    let pats = patterns(s);
    let nch = s.nch.min(32);
    let nord = (pats.len() + 1 + (pats.len() + 1) % 2) as u16;
    let mut o = vec![];
    put_str(&mut o, &s.title, 28);
    o.push(0x1a);
    o.push(16);
    o.extend_from_slice(&[0, 0]);
    put16(&mut o, nord);
    put16(&mut o, s.smps.len() as u16);
    put16(&mut o, pats.len() as u16);
    put16(&mut o, 0);
    put16(&mut o, 0x1320);
    put16(&mut o, 2); // unsigned samples
    o.extend_from_slice(b"SCRM");
    o.push(64);
    o.push(s.speed);
    o.push(s.tempo);
    o.push(0x80 | mix_volume(80.0, s.nch).min(127));
    o.push(0);
    o.push(252); // a panning table follows
    o.extend_from_slice(&[0u8; 8]);
    put16(&mut o, 0);
    // one more channel carries the pattern break that ends a song shorter than its last pattern
    let nset = nch + (pats.last().map(|p| p.len() < 64).unwrap_or(false) && nch < 32) as usize;
    let cpan = |c: usize| s.chan_pan.get(c).copied().unwrap_or(0.0);
    for c in 0..32 {
        o.push(if c < nset { if cpan(c) < 0.0 { c as u8 % 8 } else { 8 + c as u8 % 8 } } else { 255 });
    }
    for i in 0..nord as usize {
        o.push(if i < pats.len() { i as u8 } else { 255 });
    }
    let ptr_ins = o.len();
    o.resize(o.len() + 2 * (s.smps.len() + pats.len()), 0);
    let ptr_pat = ptr_ins + 2 * s.smps.len();
    for c in 0..32 {
        o.push(if c < nset { 0x20 | (7.5 + cpan(c) * 7.5).round().clamp(0.0, 15.0) as u8 } else { 0 });
    }
    let align = |o: &mut Vec<u8>| {
        while o.len() % 16 != 0 {
            o.push(0);
        }
    };
    // instruments: one per sample (the samples are mapped 1:1 when the format has no keymaps)
    let mut hdr = vec![];
    for (i, x) in s.smps.iter().enumerate() {
        align(&mut o);
        let para = (o.len() / 16) as u16;
        set16(&mut o, ptr_ins + 2 * i, para);
        hdr.push(o.len());
        let mut h = vec![0u8; 0x50];
        h[0] = 1;
        set32(&mut h, 0x10, x.data.len() as u32);
        if let Some((a, b)) = x.lp {
            set32(&mut h, 0x14, a);
            set32(&mut h, 0x18, b);
        }
        h[0x1c] = 64;
        h[0x1f] = 4 | if x.lp.is_some() { 1 } else { 0 };
        set32(&mut h, 0x20, rate_at_60(x).round().clamp(1000.0, 999_999.0) as u32);
        let mut name = vec![];
        put_str(&mut name, &x.name, 28);
        h[0x30..0x4c].copy_from_slice(&name);
        h[0x4c..0x50].copy_from_slice(b"SCRS");
        o.extend_from_slice(&h);
    }
    // the instrument number in the patterns: S3M has no instruments over samples, so the song's instrument -> its (only) sample
    let ins_smp: Vec<usize> = s.ins.iter().map(|i| i.map.iter().copied().find(|x| *x != usize::MAX).unwrap_or(0)).collect();
    for (pi, pat) in pats.iter().enumerate() {
        align(&mut o);
        let para = (o.len() / 16) as u16;
        set16(&mut o, ptr_pat + 2 * pi, para);
        let mut data = vec![];
        let last = pi + 1 == pats.len() && pat.len() < 64;
        for r in 0..64 {
            if last && r + 1 == pat.len() && nch < 32 {
                // the song ends here: a pattern break (C00) in a channel of its own
                data.extend([nch as u8 | 128, 3, 0]);
            }
            if let Some(row) = pat.get(r) {
                for (c, cell) in row.iter().enumerate().take(nch) {
                    let mut what = c as u8;
                    if cell.note >= 0 || cell.ins > 0 {
                        what |= 32;
                    }
                    if cell.vol >= 0 {
                        what |= 64;
                    }
                    if cell.fx != XFx::None {
                        what |= 128;
                    }
                    if what & 0xe0 == 0 {
                        continue;
                    }
                    data.push(what);
                    if what & 32 != 0 {
                        data.push(match cell.note {
                            -1 => 255,
                            NOTE_OFF | NOTE_CUT => 254,
                            n => {
                                let m = (n - S3M_SHIFT).clamp(0, 119);
                                ((m / 12) as u8) << 4 | (m % 12) as u8
                            }
                        });
                        data.push(if cell.ins > 0 { ins_smp.get(cell.ins as usize - 1).map(|x| x + 1).unwrap_or(0) as u8 } else { 0 });
                    }
                    if what & 64 != 0 {
                        data.push(cell.vol.clamp(0, 64) as u8);
                    }
                    if what & 128 != 0 {
                        match cell.fx {
                            XFx::Tempo(t) => data.extend([20, t]),
                            XFx::Speed(v) => data.extend([1, v]),
                            XFx::None => data.extend([0, 0]),
                        }
                    }
                }
            }
            data.push(0);
        }
        put16(&mut o, (data.len() + 2) as u16);
        o.extend_from_slice(&data);
    }
    for (i, x) in s.smps.iter().enumerate() {
        align(&mut o);
        let para = o.len() / 16;
        o[hdr[i] + 0x0d] = (para >> 16) as u8;
        set16(&mut o, hdr[i] + 0x0e, (para & 0xffff) as u16);
        for v in &x.data {
            o.extend_from_slice(&((*v as i32 + 32768) as u16).to_le_bytes());
        }
    }
    o
}

const PT_PERIODS: [u16; 36] = [856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113];

/// a sample resampled (linear) to another rate
fn resample(d: &[i16], from: f64, to: f64) -> Vec<i16> {
    if d.is_empty() || (from - to).abs() < 0.5 {
        return d.to_vec();
    }
    let n = ((d.len() as f64) * to / from).floor().max(1.0) as usize;
    (0..n)
        .map(|i| {
            let x = i as f64 * from / to;
            let a = x.floor() as usize;
            let f = x - a as f64;
            let (p, q) = (d[a.min(d.len() - 1)] as f64, d[(a + 1).min(d.len() - 1)] as f64);
            (p + (q - p) * f).round() as i16
        })
        .collect()
}

/// ProTracker: 8-bit samples (resampled so the notes, moved into the three octaves MOD has, play at their pitch), effect C for volume
pub fn write_mod(s: &XSong) -> Vec<u8> {
    let pats = patterns(s);
    let nch = s.nch.clamp(4, 32);
    let mut o = vec![];
    put_str(&mut o, &s.title, 20);
    // per sample (= instrument): the notes it plays decide the shift that puts them in MIDI 48..83, as high as possible
    let ins_smp: Vec<usize> = s.ins.iter().map(|i| i.map.iter().copied().find(|x| *x != usize::MAX).unwrap_or(0)).collect();
    let mut lo = vec![i16::MAX; s.smps.len()];
    let mut hi = vec![i16::MIN; s.smps.len()];
    for row in &s.rows {
        for c in row {
            if c.note >= 0 && c.note < 120 && c.ins > 0 {
                if let Some(si) = ins_smp.get(c.ins as usize - 1) {
                    lo[*si] = lo[*si].min(c.note);
                    hi[*si] = hi[*si].max(c.note);
                }
            }
        }
    }
    let shift: Vec<i16> = (0..s.smps.len()).map(|i| if hi[i] == i16::MIN { 0 } else { (83 - hi[i]).max(48 - lo[i]).min(83 - hi[i] + 36) }).collect();
    let mut datas = vec![];
    for (i, x) in s.smps.iter().enumerate() {
        // MIDI 60 (period 428) plays at the Amiga's 7093789.2 / 856 = 8287 Hz: the note k + shift must play the data at the rate it
        // sounds right on k
        let target = 8287.14 * 2f64.powf((x.root as f64 + shift[i] as f64 - 60.0) / 12.0);
        let mut d = resample(&x.data, x.rate, target);
        let ratio = target / x.rate;
        let mut lp = x.lp.map(|(a, b)| ((a as f64 * ratio) as usize & !1, (b as f64 * ratio) as usize & !1));
        d.truncate(131070);
        if let Some((a, b)) = lp {
            if b > d.len() || b <= a + 2 {
                lp = None;
            } else {
                d.truncate(b);
            }
        }
        if d.len() % 2 == 1 {
            d.push(0);
        }
        datas.push((d, lp));
    }
    for i in 0..31 {
        match (s.smps.get(i), datas.get(i)) {
            (Some(x), Some((d, lp))) => {
                put_str(&mut o, &x.name, 22);
                o.extend_from_slice(&((d.len() / 2) as u16).to_be_bytes());
                o.push(0);
                o.push(64);
                let (a, len) = lp.map(|(a, b)| (a / 2, (b - a) / 2)).unwrap_or((0, 1));
                o.extend_from_slice(&(a as u16).to_be_bytes());
                o.extend_from_slice(&(len as u16).to_be_bytes());
            }
            _ => {
                o.extend_from_slice(&[0u8; 22]);
                o.extend_from_slice(&[0, 0, 0, 64, 0, 0, 0, 1]);
            }
        }
    }
    o.push(pats.len().min(128) as u8);
    o.push(127);
    for i in 0..128 {
        o.push(if i < pats.len() { i as u8 } else { 0 });
    }
    o.extend_from_slice(match nch {
        4 => b"M.K.",
        6 => b"6CHN",
        8 => b"8CHN",
        _ => b"xxCH",
    });
    if nch != 4 && nch != 6 && nch != 8 {
        let l = o.len();
        o[l - 4] = b'0' + (nch / 10) as u8;
        o[l - 3] = b'0' + (nch % 10) as u8;
    }
    for pat in pats.iter().take(128) {
        // speed and tempo at the start, in channels with no command
        for r in 0..64 {
            let row = pat.get(r);
            let mut extra: Vec<(u8, u8)> = vec![];
            if std::ptr::eq(pat.as_ptr(), s.rows.as_ptr()) && r == 0 {
                extra.push((0x0f, s.speed));
                extra.push((0x0f, s.tempo));
            }
            for c in 0..nch {
                let cell = row.and_then(|rw| rw.get(c)).copied().unwrap_or_default();
                let (mut period, mut ins) = (0u16, 0u8);
                let (mut cmd, mut par) = (0u8, 0u8);
                if cell.note >= 0 && cell.note < 120 && cell.ins > 0 {
                    let si = ins_smp.get(cell.ins as usize - 1).copied().unwrap_or(0);
                    let mut k = cell.note + shift.get(si).copied().unwrap_or(0);
                    while k > 83 {
                        k -= 12;
                    }
                    while k < 48 {
                        k += 12;
                    }
                    period = PT_PERIODS[(k - 48) as usize];
                    ins = (si + 1).min(31) as u8;
                } else if cell.note == NOTE_CUT || cell.note == NOTE_OFF {
                    cmd = 0x0e;
                    par = 0xc0;
                }
                if cell.vol >= 0 && cmd == 0 {
                    cmd = 0x0c;
                    par = cell.vol.clamp(0, 64) as u8;
                }
                match cell.fx {
                    XFx::Tempo(t) if cmd == 0 => {
                        cmd = 0x0f;
                        par = t.max(32);
                    }
                    XFx::Speed(v) if cmd == 0 => {
                        cmd = 0x0f;
                        par = v.min(31);
                    }
                    XFx::Tempo(t) => extra.push((0x0f, t.max(32))),
                    _ => {}
                }
                // the song ends on its last row, not at the end of the 64-row pattern
                if cmd == 0 && c == nch - 1 && std::ptr::eq(pat.as_ptr(), pats[pats.len() - 1].as_ptr()) && r + 1 == pat.len() && pat.len() < 64 {
                    cmd = 0x0d;
                    par = 0;
                }
                if cmd == 0 {
                    if let Some((c2, p2)) = extra.pop() {
                        cmd = c2;
                        par = p2;
                    }
                }
                o.push((ins & 0xf0) | (period >> 8) as u8 & 0x0f);
                o.push(period as u8);
                o.push((ins & 0x0f) << 4 | cmd);
                o.push(par);
            }
        }
    }
    for (d, _) in &datas {
        for v in d {
            o.push((*v >> 8) as i8 as u8);
        }
    }
    o
}

pub fn write(s: &XSong, f: Fmt) -> Vec<u8> {
    match f {
        Fmt::Mod => write_mod(s),
        Fmt::S3m => write_s3m(s),
        Fmt::Xm => write_xm(s),
        Fmt::It => write_it(s, false),
        Fmt::Mptm => write_it(s, true),
    }
}

/// a song for a format whose samples are 1:1 instruments (MOD, S3M): the instruments are the samples
pub fn summary(s: &XSong) -> String {
    let notes = s.rows.iter().flatten().filter(|c| c.note >= 0 && c.note < 120).count();
    format!("{} channels, {} samples, {} instruments, {} notes, {} rows", s.nch, s.smps.len(), s.ins.len(), notes, s.rows.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song() -> Engine {
        let mut e = Engine::new(22050.0);
        e.project.bpm = 120.0;
        // a pad that sustains, a pluck that decays, a drum kit, an audio clip
        let mut synth = Track::new("Pad", TrackKind::Instrument, 0, crate::dsp::make_instrument("synth", 22050.0));
        let mut c = Clip::midi(0.0, 16.0, "pad");
        if let ClipData::Midi(n) = &mut c.data {
            for (i, k) in [48u8, 55, 60, 64, 67, 72].iter().enumerate() {
                n.push(Note::new(i as f64 * 2.0, 2.0, *k, 0.8));
            }
        }
        synth.clips.push(c);
        synth.inst_kind = "synth".into();
        e.project.tracks.push(synth);
        let mut drums = Track::new("Drums", TrackKind::Instrument, 0, crate::dsp::make_instrument("drums", 22050.0));
        drums.inst_kind = "drums".into();
        let mut c = Clip::midi(0.0, 16.0, "beat");
        if let ClipData::Midi(n) = &mut c.data {
            for b in 0..16 {
                n.push(Note::new(b as f64, 0.25, if b % 2 == 0 { 36 } else { 38 }, 0.9));
                n.push(Note::new(b as f64 + 0.5, 0.25, 42, 0.6));
            }
        }
        drums.clips.push(c);
        e.project.tracks.push(drums);
        // 8 beats of a 220 Hz tone with a beat every second as audio
        let sr = 22050u32;
        let n = (4.0 * sr as f64) as usize;
        let mut st = vec![0f32; n * 2];
        for i in 0..n {
            let t = i as f32 / sr as f32;
            let v = (std::f32::consts::TAU * 220.0 * t).sin() * 0.3 * (1.0 - (t % 0.5) * 1.5).max(0.1);
            st[i * 2] = v;
            st[i * 2 + 1] = v;
        }
        let smp = std::sync::Arc::new(Sample { name: "tone".into(), path: String::new(), rate: sr, stereo: st, variants: std::sync::Mutex::new(Variants::default()) });
        let mut a = Track::new("Loop", TrackKind::Audio, 0, None);
        a.clips.push(Clip { start: 0.0, len: 8.0, name: "tone".into(), loop_len: 0.0, lyric_text: String::new(), lyric_lang: String::new(), tag: String::new(), data: ClipData::Audio { sample: smp, offset_s: 0.0, gain: 1.0, pitch: 0.0, warp: 0.0 } });
        e.project.tracks.push(a);
        e.project.tempo_map = vec![(8.0, 140.0)];
        e
    }

    #[test]
    fn every_format_reads_back_with_its_notes_samples_and_loops() {
        let mut r = Renderer::new(song());
        for f in [Fmt::Mod, Fmt::S3m, Fmt::Xm, Fmt::It, Fmt::Mptm] {
            let s = build(&mut r, f, &Opts::default());
            assert!(s.smps.len() <= f.max_smp() && s.nch <= f.max_ch().max(4), "{f:?}: {}", summary(&s));
            let bytes = write(&s, f);
            let m = crate::tracker::parse_module(&bytes).unwrap_or_else(|e| panic!("{f:?}: {e}"));
            let notes = |m: &crate::tracker::Module| m.orders.iter().flat_map(|p| m.patterns[*p].iter().flatten()).filter(|c| c.note >= 0 && c.note < 120).count();
            let mine = s.rows.iter().flatten().filter(|c| c.note >= 0 && c.note < 120).count();
            assert_eq!(notes(&m), mine, "{f:?}: the notes come back");
            // 6 pad notes, 32 drum hits, 2 audio segments
            assert_eq!(mine, 6 + 32 + 2, "{f:?}: {}", summary(&s));
            let smps: Vec<&crate::tracker::Smp> = m.ins.iter().flat_map(|i| i.smps.iter()).filter(|x| !x.data.is_empty()).collect();
            assert!(smps.len() >= 4, "{f:?}: {} samples", smps.len());
            assert!(smps.iter().any(|x| x.looped), "{f:?}: the pad loops");
            assert!(smps.iter().any(|x| x.name.contains("bpm")), "{f:?}: the audio segments carry their tempo");
            // the tempo change is in the patterns
            let tempo = m.orders.iter().flat_map(|p| m.patterns[*p].iter().flatten()).any(|c| c.fx == crate::tracker::Fx::Tempo(140));
            assert!(tempo, "{f:?}: tempo change");
            if f.keymap() {
                assert!(s.ins.iter().any(|i| i.map.iter().filter(|x| **x != usize::MAX).collect::<std::collections::BTreeSet<_>>().len() > 1), "{f:?}: a keymapped instrument");
            }
        }
    }

    /// writes the test song (and a tone-only song) in every format to $TRK_OUT, for checking with libopenmpt (ffmpeg)
    #[test]
    #[ignore]
    fn writes_files_for_libopenmpt() {
        let Ok(dir) = std::env::var("TRK_OUT") else { return };
        let mut r = Renderer::new(song());
        let mut tone = song();
        tone.project.tracks.drain(0..2);
        tone.project.tempo_map.clear();
        let mut rt = Renderer::new(tone);
        for f in [Fmt::Mod, Fmt::S3m, Fmt::Xm, Fmt::It, Fmt::Mptm] {
            let s = build(&mut r, f, &Opts::default());
            std::fs::write(format!("{dir}/full.{}", f.ext()), write(&s, f)).unwrap();
            let t = build(&mut rt, f, &Opts::default());
            std::fs::write(format!("{dir}/tone.{}", f.ext()), write(&t, f)).unwrap();
        }
        song().bounce_wav(&format!("{dir}/full.daw.wav"), 1.0).unwrap();
    }

    /// a Ling Slicer track: `n` slices (clicks, then a tone slice at D4 + 60 cents) played in order, then the tone slice once more
    /// over C major (tuned to E) on a gated, looped slice; the slicer's tune set to chords
    fn slicer_song(n: usize) -> (Engine, u8) {
        let sr = 22050u32;
        let mut x: Vec<f32> = vec![];
        let mut rng = crate::dsp::Rng::new(3);
        for _ in 0..n - 1 {
            for j in 0..(sr as usize / 5) {
                let v = if j < 1500 { rng.next() * (-(j as f32) / 300.0).exp() * 0.8 } else { 0.0 };
                x.extend([v, v]);
            }
        }
        let hz = 440.0 * 2f32.powf((62.6 - 69.0) / 12.0);
        for j in 0..(sr as usize * 6 / 10) {
            let t = j as f32 / sr as f32;
            let v = (std::f32::consts::TAU * hz * t).sin() * 0.5 * (t / 0.005).min(1.0);
            x.extend([v, v]);
        }
        let dir = std::env::temp_dir().join(format!("ling_trk_slicer_{}_{n}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("slices.wav");
        let spec = hound::WavSpec { channels: 2, sample_rate: sr, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for v in &x {
            w.write_sample((v * 32767.0) as i16).unwrap();
        }
        w.finalize().unwrap();
        use crate::dsp::Instrument;
        let mut s = crate::dsp::slicer::Slicer::new(22050.0);
        s.set_param("gap", 0.3);
        s.command("open", &[], &path.to_string_lossy());
        assert_eq!(s.slices.len(), n, "noise bursts, then the tone: {:?}", s.slices.iter().map(|x| x.start).collect::<Vec<_>>());
        let mut e = Engine::new(22050.0);
        e.project.bpm = 120.0;
        let root = s.root();
        let last = n - 1;
        s.command("set", &[last as f64, 11.0, 1.0], "");
        s.command("set", &[last as f64, 12.0, 1.0], "");
        let mut h = vec![];
        for k in [48u8, 60, 64, 67] {
            h.push(Note::new(0.0, 64.0, k, 0.8));
        }
        s.timeline = crate::dsp::slicer::timeline(&h, 4.0);
        s.set_param("tune", 1.0);
        let mut t = Track::new("Chops", TrackKind::Instrument, 0, Some(Box::new(s)));
        t.inst_kind = "slicer".into();
        let mut c = Clip::midi(0.0, n as f64 + 4.0, "chops");
        if let ClipData::Midi(v) = &mut c.data {
            for i in 0..n {
                v.push(Note::new(i as f64 * 0.5, 0.25, root + i as u8, 0.9));
            }
            v.push(Note::new(n as f64 * 0.5 + 1.0, 1.0, root + last as u8, 0.9));
        }
        t.clips.push(c);
        e.project.tracks.push(t);
        (e, root)
    }

    #[test]
    fn a_slicer_exports_one_sample_per_slice_in_every_format() {
        let n = 6;
        let mut r = Renderer::new(slicer_song(n).0);
        let root = slicer_song(n).1;
        for f in [Fmt::Mod, Fmt::S3m, Fmt::Xm, Fmt::It, Fmt::Mptm] {
            let s = build(&mut r, f, &Opts::default());
            // the slices, in order, and the tuned one (the tone, sung at D4 + 60c over C major: to E, +140 cents)
            let names: Vec<&str> = s.smps.iter().map(|x| x.name.as_str()).collect();
            // (the tone slice always plays over C major: only its tuned sample is used)
            for i in 0..n - 1 {
                assert!(names.iter().any(|nm| nm.ends_with(&format!("slice {}", i + 1))), "{f:?}: {names:?}");
            }
            let tuned = s.smps.iter().find(|x| x.name.contains("c") && x.name.contains(&format!("slice {n} +"))).unwrap_or_else(|| panic!("{f:?}: no tuned sample in {names:?}"));
            let data: Vec<f32> = tuned.data.iter().map(|v| *v as f32 / 32768.0).collect();
            let p = crate::chops::chop_f0(&data, 1000, data.len() - 1000, tuned.rate as u32).expect("pitched").midi;
            assert!((p - 64.0).abs() < 0.15, "{f:?}: the tuned sample sounds E4 ({p})");
            assert!(tuned.lp.is_some(), "{f:?}: the looped slice keeps its loop");
            // the notes: the slices in order on their keys, untransposed, then the tuned one on its own key
            let cells: Vec<XCell> = s.rows.iter().flatten().filter(|c| c.note >= 0 && c.note < 120).copied().collect();
            assert_eq!(cells.len(), n + 1, "{f:?}");
            for (i, c) in cells.iter().take(n - 1).enumerate() {
                assert_eq!(c.note, (root as usize + i) as i16, "{f:?}: slice {i} on root + {i}");
                let sm = &s.smps[s.ins[c.ins as usize - 1].map[c.note as usize]];
                assert!(sm.name.ends_with(&format!("slice {}", i + 1)) && sm.root as i16 == c.note, "{f:?}: {}", sm.name);
            }
            for t in [cells[n - 1], cells[n]] {
                assert!(s.smps[s.ins[t.ins as usize - 1].map[t.note as usize]].name.contains("+"), "{f:?}: the tuned notes play the tuned sample");
                assert_eq!(t.note, (root as usize + n) as i16, "{f:?}: the tuned sample sits on the first key above the slices");
            }
            // the gated slice ends with a note-off / cut; the one-shots ring
            let ends = s.rows.iter().flatten().filter(|c| c.note == NOTE_OFF || c.note == NOTE_CUT).count();
            assert!(ends >= 1, "{f:?}");
            // one choke group: one channel
            assert_eq!(s.nch, if f == Fmt::Mod { 4 } else { 1 }, "{f:?}");
            let m = crate::tracker::parse_module(&write(&s, f)).unwrap_or_else(|e| panic!("{f:?}: {e}"));
            let back = m.orders.iter().flat_map(|p| m.patterns[*p].iter().flatten()).filter(|c| c.note >= 0 && c.note < 120).count();
            assert_eq!(back, n + 1, "{f:?}: the notes come back");
            let sm: Vec<String> = m.ins.iter().flat_map(|i| i.smps.iter()).filter(|x| !x.data.is_empty()).map(|x| x.name.clone()).collect();
            assert!(sm.len() >= n, "{f:?}: {sm:?}");
        }
    }

    #[test]
    fn a_slicer_over_the_mod_limit_drops_the_rarest_slices_and_says_which() {
        let mut r = Renderer::new(slicer_song(40).0);
        let s = build(&mut r, Fmt::Mod, &Opts::default());
        assert!(s.smps.len() <= 31);
        assert!(s.report.iter().any(|l| l.contains("no room for") && l.contains("slice")), "{:?}", s.report);
    }

    #[test]
    fn loops_sit_on_whole_periods_of_a_sustained_tone() {
        let sr = 22050.0;
        let x: Vec<f32> = (0..22050).map(|i| (std::f32::consts::TAU * 220.0 * i as f32 / sr).sin() * 0.5).collect();
        let (a, b) = find_loop(&x, 20000, sr, 57).expect("a loop");
        let periods = (b - a) as f32 / (sr / 220.0);
        assert!((periods - periods.round()).abs() < 0.05, "{periods}");
        // a decaying pluck gets none
        let y: Vec<f32> = (0..22050).map(|i| (std::f32::consts::TAU * 220.0 * i as f32 / sr).sin() * (-(i as f32) / 2000.0).exp()).collect();
        assert!(find_loop(&y, 20000, sr, 57).is_none());
    }
}
