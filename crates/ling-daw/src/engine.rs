//! Realtime engine: transport, event scheduling, audio-clip playback, routing,
//! clip launcher (session), automation and the mixer.
use crate::dsp::Instrument;
use crate::model::*;
use std::sync::{Arc, Mutex};

const SUB: usize = 64; // frames per scheduling slice

pub type SharedEngine = Arc<Mutex<Engine>>;

/// Tiny helper so the per-track `Vec<Option<_>>` fields can be edited uniformly.
trait VecOps {
    fn remove_at(&mut self, i: usize);
    fn move_item(&mut self, i: usize, j: usize);
}
impl<T> VecOps for Vec<T> {
    fn remove_at(&mut self, i: usize) {
        if i < self.len() {
            self.remove(i);
        }
    }
    fn move_item(&mut self, i: usize, j: usize) {
        if i < self.len() && j < self.len() && i != j {
            let v = self.remove(i);
            self.insert(j, v);
        }
    }
}

#[derive(Clone, Copy)]
struct Playing {
    clip: usize,
    scene: i32,
    start: f64, // session beats
}
#[derive(Clone, Copy, PartialEq)]
enum QCmd {
    Start(usize, i32),
    Stop,
}
#[derive(Clone, Copy)]
struct Queued {
    cmd: QCmd,
    at: f64,
}

pub struct Engine {
    pub project: Project,
    pub sr: f32,
    pub playing: bool,
    pub position: f64, // beats (arrangement)
    tmp: Vec<f32>,
    mix: Vec<f32>,
    outs: Vec<Vec<f32>>,
    /// Per-track pre-fader peak of the last slice (sidechain key tap).
    key_pk: Vec<f32>,
    /// samples each track has been silent for; past SLEEP_AFTER a track with nothing to play is not rendered at all
    quiet: Vec<u32>,
    order: Vec<usize>,
    /// `order` grouped into dependency levels (tracks within a level are independent)
    levels: Vec<Vec<usize>>,
    route_sig: u64,
    audible: Vec<bool>,
    sample_clock: u64,
    /// Output latency (render-ahead + device buffer) in seconds: recorded notes/audio are shifted back by it.
    pub rec_latency_s: f64,
    /// Parameters being moved by hand while recording automation: (track, param, last beat, clock of last move).
    touched: Vec<(Option<usize>, String, f64, u64)>,
    /// Track armed for MIDI recording (live notes while playing are captured).
    pub rec_arm: Option<usize>,
    rec_clip: Option<(usize, usize)>,
    rec_held: Vec<(u8, f64, f32)>,
    /// When true, moving a parameter while playing writes automation points.
    pub auto_rec: bool,
    /// Bumped whenever the engine itself edits the project (recording, launcher capture).
    pub rec_rev: u64,
    /// Session (launcher) clock in beats.
    pub spos: f64,
    sess_play: Vec<Option<Playing>>,
    sess_queue: Vec<Option<Queued>>,
    /// Record launched clips into the arrangement.
    pub perf_rec: bool,
    /// Per track: (launcher clip index, arrangement beat where it started) while recording.
    perf_open: Vec<Option<(usize, f64)>>,
    /// Offline rendering: arrangement only, launcher ignored.
    offline: bool,
    /// master limiter gain state and the deepest reduction since last read (linear)
    lim_gain: f32,
    pub lim_min: f32,
    /// audio-device test tone: samples left (L then R) and phase
    tone_left: u32,
    tone_ph: f32,
}

/// Schedule the notes of one clip whose absolute time falls inside [w0, w1).
/// `base` is the beat of note-time zero; `limit` the usable clip length.
#[allow(clippy::too_many_arguments)]
fn sched_notes(inst: &mut Option<Box<dyn Instrument>>, held: &mut Vec<(u8, u64)>, notes: &[Note], base: f64, limit: f64, w0: f64, w1: f64, t0: f64, bps: f64, start_clock: u64) {
    for n in notes {
        if n.start >= limit {
            continue;
        }
        let ns = base + n.start;
        if ns >= w0 && ns < w1 {
            if let Some(i) = inst.as_mut() {
                i.note_on_lyric(n.key, n.vel, n.pan, n.cutoff, n.reso, &n.lyric, (n.len.min(limit - n.start) / bps) as f32);
            }
            let end_beat = ns + n.len.min(limit - n.start);
            let end_clock = start_clock + ((end_beat - t0) / bps).max(0.0) as u64;
            held.push((n.key, end_clock));
        }
    }
}

/// Play an audio clip, using the pre-rendered warp / pitch variant when it is ready
/// (otherwise falls back to varispeed pitch on the original).
#[allow(clippy::too_many_arguments)]
fn mix_clip_audio(dst: &mut [f32], sample: &Arc<Sample>, offset_s: f64, gain: f32, pitch: f32, warp: f32, bpm: f64, local0: f64, bps: f64, spb: f64, frames: usize, len: f64, wrap: f64) {
    let key = variant_key(pitch, warp, bpm);
    if key == (0, 1000) {
        mix_audio(dst, sample, offset_s, gain, 0.0, local0, bps, spb, frames, len, wrap);
    } else if let Some(v) = sample.cached(key) {
        mix_audio(dst, &v, offset_s * key.1 as f64 / 1000.0, gain, 0.0, local0, bps, spb, frames, len, wrap);
    } else {
        mix_audio(dst, sample, offset_s, gain, pitch, local0, bps, spb, frames, len, wrap);
    }
}

#[allow(clippy::too_many_arguments)]
fn mix_audio(dst: &mut [f32], sample: &Sample, offset_s: f64, gain: f32, pitch: f32, local0: f64, bps: f64, spb: f64, frames: usize, len: f64, wrap: f64) {
    let speed = 2f64.powf(pitch as f64 / 12.0);
    let rate = sample.rate as f64;
    let nfr = sample.frames();
    for f in 0..frames {
        let mut l = local0 + bps * f as f64;
        if l < 0.0 {
            continue;
        }
        if l >= len {
            continue;
        }
        if wrap > 0.0 {
            l %= wrap;
        }
        let src = (l * spb * speed + offset_s) * rate;
        let i = src as usize;
        if i + 1 >= nfr {
            continue;
        }
        let fr = (src - i as f64) as f32;
        for c in 0..2 {
            let a = sample.stereo[i * 2 + c];
            let b = sample.stereo[(i + 1) * 2 + c];
            dst[f * 2 + c] += (a + (b - a) * fr) * gain;
        }
    }
}

impl Engine {
    pub fn new(sr: f32) -> Engine {
        Engine {
            project: Project::new(),
            sr,
            playing: false,
            position: 0.0,
            tmp: vec![],
            mix: vec![],
            outs: vec![],
            key_pk: vec![],
            quiet: vec![],
            order: vec![],
            levels: vec![],
            route_sig: u64::MAX,
            audible: vec![],
            sample_clock: 0,
            rec_latency_s: 0.0,
            touched: vec![],
            rec_arm: None,
            rec_clip: None,
            rec_held: vec![],
            auto_rec: false,
            rec_rev: 0,
            spos: 0.0,
            sess_play: vec![],
            sess_queue: vec![],
            perf_rec: false,
            perf_open: vec![],
            offline: false,
            lim_gain: 1.0,
            lim_min: 1.0,
            tone_left: 0,
            tone_ph: 0.0,
        }
    }

    pub fn shared(self) -> SharedEngine {
        Arc::new(Mutex::new(self))
    }

    /// Start the device test tone (440 Hz, left then right).
    pub fn start_test_tone(&mut self) {
        self.tone_left = (2.0 * self.sr) as u32;
        self.tone_ph = 0.0;
    }

    fn beats_per_sample(&self) -> f64 {
        self.project.bpm_at(self.position) / 60.0 / self.sr as f64
    }

    pub fn play(&mut self) {
        self.playing = true;
    }
    pub fn pause(&mut self) {
        self.finish_perf_all();
        self.finish_recording();
        self.playing = false;
        self.release_all();
    }
    pub fn stop(&mut self) {
        self.pause();
        self.position = 0.0;
        self.spos = 0.0;
        for p in self.sess_play.iter_mut() {
            *p = None;
        }
        for q in self.sess_queue.iter_mut() {
            *q = None;
        }
    }
    pub fn seek(&mut self, beat: f64) {
        self.release_all();
        self.position = beat.max(0.0);
    }
    fn release_all(&mut self) {
        for t in self.project.tracks.iter_mut() {
            if let Some(i) = t.inst.as_mut() {
                i.all_off();
            }
            t.held.clear();
        }
    }

    // ── live notes / recording ──────────────────────────────────────────────
    /// Live (keyboard/preview) note on the given track. Captured into a clip
    /// when the track is armed and the transport is running.
    pub fn live_note(&mut self, track: usize, key: u8, vel: f32, on: bool) {
        self.wake(track);
        if let Some(i) = self.project.tracks.get_mut(track).and_then(|t| t.inst.as_mut()) {
            if on {
                i.note_on(key, vel)
            } else {
                i.note_off(key)
            }
        }
        if self.playing && self.rec_arm == Some(track) {
            self.record_note(track, key, vel, on);
        }
    }

    /// Wake a sleeping track (it renders from the next block on).
    pub fn wake(&mut self, track: usize) {
        if let Some(q) = self.quiet.get_mut(track) {
            *q = 0;
        }
    }

    /// Is the track asleep (silent for a while with nothing to play, so not rendered)?
    pub fn asleep(&self, track: usize) -> bool {
        self.quiet.get(track).map(|q| *q >= SLEEP_AFTER).unwrap_or(false)
    }

    /// Transport position as the player heard it (the engine renders ahead of the speakers).
    pub fn heard_position(&self) -> f64 {
        (self.position - self.rec_latency_s * self.project.bpm_at(self.position) / 60.0).max(0.0)
    }

    fn record_note(&mut self, track: usize, key: u8, vel: f32, on: bool) {
        let pos = self.heard_position();
        if self.rec_clip.is_none() {
            if !on {
                return;
            }
            let start = (pos / 4.0).floor() * 4.0;
            let Some(t) = self.project.tracks.get_mut(track) else { return };
            t.clips.push(Clip::midi(start, 4.0, "Recording"));
            self.rec_rev += 1;
            self.rec_clip = Some((track, t.clips.len() - 1));
        }
        let Some((rt, rc)) = self.rec_clip else { return };
        if rt != track {
            return;
        }
        if on {
            self.rec_held.push((key, pos, vel));
        } else if let Some(i) = self.rec_held.iter().position(|h| h.0 == key) {
            let (k, start, v) = self.rec_held.remove(i);
            self.push_recorded(rt, rc, k, start, pos, v);
        }
    }

    fn push_recorded(&mut self, t: usize, c: usize, key: u8, start: f64, end: f64, vel: f32) {
        self.rec_rev += 1;
        if let Some(clip) = self.project.tracks.get_mut(t).and_then(|t| t.clips.get_mut(c)) {
            let rel = (start - clip.start).max(0.0);
            let len = (end - start).max(0.06);
            if let ClipData::Midi(n) = &mut clip.data {
                n.push(Note::new(rel, len, key, vel));
            }
            let need = ((rel + len) / 4.0).ceil() * 4.0;
            if need > clip.len {
                clip.len = need;
            }
        }
    }

    /// Close any notes still held and end the recording pass.
    pub fn finish_recording(&mut self) {
        let pos = self.heard_position();
        if let Some((t, c)) = self.rec_clip.take() {
            let held = std::mem::take(&mut self.rec_held);
            for (k, s, v) in held {
                self.push_recorded(t, c, k, s, pos, v);
            }
        }
        self.rec_held.clear();
    }

    // ── automation ──────────────────────────────────────────────────────────
    fn apply_lane(track: &mut Track, lane_idx: usize, beat: f64) {
        let (param, v) = {
            let lane = &track.auto[lane_idx];
            match lane.value_at(beat) {
                Some(v) => (lane.param.clone(), v),
                None => return,
            }
        };
        match param.as_str() {
            "vol" => track.volume = v.clamp(0.0, 2.0),
            "pan" => track.pan = v.clamp(-1.0, 1.0),
            p if p.starts_with("fx") => {
                // "fx<i>.<p>", or "fx<i>.wet" for the effect's dry / wet
                if let Some((a, b)) = p[2..].split_once('.') {
                    if let Some(e) = a.parse::<usize>().ok().and_then(|i| track.effects.get_mut(i)) {
                        if b == "wet" {
                            e.wet = v.clamp(0.0, 1.0);
                        } else if let Ok(pi) = b.parse::<usize>() {
                            e.set(pi, v);
                        }
                    }
                }
            }
            name => {
                if let Some(i) = track.inst.as_mut() {
                    i.set_param(name, v);
                }
            }
        }
    }

    fn apply_automation(&mut self, beat: f64) {
        // a parameter moved by hand in the last moment is "touched": the lane must not fight the hand
        let hold = (self.sr as f64 * 0.4) as u64;
        let clock = self.sample_clock;
        self.touched.retain(|t| clock.saturating_sub(t.3) < hold);
        let touched = &self.touched;
        for (ti, t) in self.project.tracks.iter_mut().enumerate() {
            for li in 0..t.auto.len() {
                if !t.auto[li].points.is_empty() && !touched.iter().any(|x| x.0 == Some(ti) && x.1 == t.auto[li].param) {
                    Self::apply_lane(t, li, beat);
                }
            }
        }
        for li in 0..self.project.master_auto.len() {
            let param = self.project.master_auto[li].param.clone();
            if touched.iter().any(|x| x.0.is_none() && x.1 == param) {
                continue;
            }
            let Some(v) = self.project.master_auto[li].value_at(beat) else { continue };
            if param == "vol" {
                self.project.master_volume = v.clamp(0.0, 2.0);
            } else if let Some((a, b)) = param.strip_prefix("fx").and_then(|r| r.split_once('.')) {
                if let Some(e) = a.parse::<usize>().ok().and_then(|i| self.project.master_effects.get_mut(i)) {
                    if b == "wet" {
                        e.wet = v.clamp(0.0, 1.0);
                    } else if let Ok(pi) = b.parse::<usize>() {
                        e.set(pi, v);
                    }
                }
            }
        }
    }

    /// Record a parameter move as an automation point at the playhead.
    pub fn record_auto(&mut self, track: Option<usize>, param: &str, value: f32) {
        if !(self.auto_rec && self.playing) || !value.is_finite() {
            return;
        }
        let beat = self.position;
        let clock = self.sample_clock;
        // continuing a touch: the stretch just passed over replaces whatever the lane held there
        let mut from = beat;
        match self.touched.iter_mut().find(|x| x.0 == track && x.1 == param) {
            Some(x) => {
                if beat >= x.2 && beat - x.2 < 2.0 {
                    from = x.2;
                }
                x.2 = beat;
                x.3 = clock;
            }
            None => self.touched.push((track, param.to_string(), beat, clock)),
        }
        let lanes = match track {
            Some(t) => match self.project.tracks.get_mut(t) {
                Some(t) => &mut t.auto,
                None => return,
            },
            None => &mut self.project.master_auto,
        };
        let li = match lanes.iter().position(|l| l.param == param) {
            Some(i) => i,
            None => {
                lanes.push(AutoLane { param: param.to_string(), points: vec![] });
                lanes.len() - 1
            }
        };
        let pts = &mut lanes[li].points;
        pts.retain(|p| (p.0 - beat).abs() > 0.04 && !(p.0 > from && p.0 < beat));
        let at = pts.partition_point(|p| p.0 < beat);
        pts.insert(at, (beat, value));
    }

    // ── launcher (session) ──────────────────────────────────────────────────
    fn ensure_sess_len(&mut self) {
        let n = self.project.tracks.len();
        self.sess_play.resize(n, None);
        self.sess_queue.resize(n, None);
        self.perf_open.resize(n, None);
    }

    /// Turn a recorded launcher pass on track `t` into arrangement clips (one copy per loop).
    fn finish_perf(&mut self, t: usize) {
        let Some((ci, start)) = self.perf_open.get_mut(t).and_then(|o| o.take()) else { return };
        let end = self.position;
        let Some(track) = self.project.tracks.get_mut(t) else { return };
        let Some(src) = track.session.get(ci).cloned() else { return };
        self.rec_rev += 1;
        let len = src.len.max(0.25);
        let mut k = 0.0;
        while start + k * len < end - 0.01 {
            let mut c = src.clone();
            c.start = start + k * len;
            c.len = len.min(end - c.start);
            track.clips.push(c);
            k += 1.0;
        }
    }
    fn finish_perf_all(&mut self) {
        for t in 0..self.perf_open.len() {
            self.finish_perf(t);
        }
    }
    fn quant_boundary(&self) -> f64 {
        let q = self.project.quant;
        if q <= 0.0 {
            self.spos
        } else {
            (self.spos / q - 1e-9).ceil().max(0.0) * q
        }
    }
    fn queue_at(&self) -> f64 {
        if self.playing {
            self.quant_boundary()
        } else {
            self.spos
        }
    }
    pub fn launch(&mut self, track: usize, scene: usize) {
        self.ensure_sess_len();
        let c = self.project.slot(scene, track);
        if c < 0 || track >= self.sess_queue.len() {
            return;
        }
        let at = self.queue_at();
        self.playing = true;
        self.sess_queue[track] = Some(Queued { cmd: QCmd::Start(c as usize, scene as i32), at });
    }
    pub fn launch_scene(&mut self, scene: usize) {
        self.ensure_sess_len();
        let at = self.queue_at();
        self.playing = true;
        for t in 0..self.project.tracks.len() {
            let c = self.project.slot(scene, t);
            self.sess_queue[t] = Some(if c >= 0 { Queued { cmd: QCmd::Start(c as usize, scene as i32), at } } else { Queued { cmd: QCmd::Stop, at } });
        }
    }
    pub fn stop_track(&mut self, track: usize) {
        self.ensure_sess_len();
        let at = self.queue_at();
        if track < self.sess_queue.len() {
            self.sess_queue[track] = Some(Queued { cmd: QCmd::Stop, at });
        }
    }
    pub fn stop_all_session(&mut self) {
        self.ensure_sess_len();
        for p in self.sess_play.iter_mut() {
            *p = None;
        }
        for q in self.sess_queue.iter_mut() {
            *q = None;
        }
        self.release_all();
    }
    /// (scene playing or -1, queued: -1 none / -2 stop / scene, progress 0..1)
    pub fn sess_state(&self, track: usize) -> (i32, i32, f64) {
        let play = self.sess_play.get(track).copied().flatten();
        let q = self.sess_queue.get(track).copied().flatten();
        let queued = match q {
            None => -1,
            Some(Queued { cmd: QCmd::Stop, .. }) => -2,
            Some(Queued { cmd: QCmd::Start(_, s), .. }) => s,
        };
        match play {
            Some(p) => {
                let len = self.project.tracks.get(track).and_then(|t| t.session.get(p.clip)).map(|c| c.len).unwrap_or(4.0).max(0.25);
                (p.scene, queued, ((self.spos - p.start).max(0.0) % len) / len)
            }
            None => (-1, queued, 0.0),
        }
    }

    /// Cheap fingerprint of the routing graph (no allocation).
    fn route_signature(&self) -> u64 {
        let mut h: u64 = self.project.tracks.len() as u64 + 0x9E37_79B9_7F4A_7C15;
        for r in &self.project.routes {
            h = h.rotate_left(7) ^ ((r.from as u64) << 20 | (r.to as u64) << 4 | r.kind as u64);
            h = h.wrapping_mul(0x100_0000_01B3);
        }
        h
    }

    /// Keep engine-side per-track state aligned after a track was removed.
    pub fn on_track_removed(&mut self, i: usize) {
        if let Some((t, _)) = self.rec_clip {
            if t == i {
                self.finish_recording();
            }
        }
        for v in [&mut self.sess_play as &mut dyn VecOps, &mut self.sess_queue, &mut self.perf_open] {
            v.remove_at(i);
        }
        if i < self.outs.len() {
            self.outs.remove(i);
        }
        if i < self.key_pk.len() {
            self.key_pk.remove(i);
        }
        if i < self.quiet.len() {
            self.quiet.remove(i);
        }
        self.rec_arm = match self.rec_arm {
            Some(t) if t == i => None,
            Some(t) if t > i => Some(t - 1),
            other => other,
        };
        if let Some((t, c)) = self.rec_clip {
            if t > i {
                self.rec_clip = Some((t - 1, c));
            }
        }
        self.route_sig = u64::MAX;
    }

    /// Keep engine-side per-track state aligned after track `i` moved to index `j`.
    pub fn on_track_moved(&mut self, i: usize, j: usize) {
        let map = |old: usize| -> usize {
            if old == i {
                j
            } else if i < j && old > i && old <= j {
                old - 1
            } else if i > j && old >= j && old < i {
                old + 1
            } else {
                old
            }
        };
        for v in [&mut self.sess_play as &mut dyn VecOps, &mut self.sess_queue, &mut self.perf_open] {
            v.move_item(i, j);
        }
        self.outs.clear();
        self.key_pk.clear();
        self.quiet.clear();
        self.rec_arm = self.rec_arm.map(map);
        if let Some((t, c)) = self.rec_clip {
            self.rec_clip = Some((map(t), c));
        }
        self.route_sig = u64::MAX;
    }

    /// A clip on track `t` was removed / moved away: stop a recording pass that pointed into it.
    pub fn on_clips_changed(&mut self, t: usize) {
        if matches!(self.rec_clip, Some((rt, _)) if rt == t) {
            self.finish_recording();
        }
    }

    /// Group a render order into levels: a track sits one level below the deepest track that feeds it.
    fn route_levels(&self, order: &[usize], routes: &[Route]) -> Vec<Vec<usize>> {
        let n = self.project.tracks.len();
        let mut pos = vec![usize::MAX; n];
        for (i, &t) in order.iter().enumerate() {
            if t < n {
                pos[t] = i;
            }
        }
        let mut lvl = vec![0usize; n];
        for &t in order {
            if t >= n {
                continue;
            }
            for r in routes.iter().filter(|r| r.to == t && r.from < n && r.from != t && pos[r.from] < pos[t]) {
                lvl[t] = lvl[t].max(lvl[r.from] + 1);
            }
        }
        let depth = lvl.iter().copied().max().map(|m| m + 1).unwrap_or(0);
        let mut out = vec![Vec::new(); depth];
        for &t in order {
            if t < n {
                out[lvl[t]].push(t);
            }
        }
        out
    }

    /// Topological render order: sources before the tracks they feed.
    fn route_order(&self) -> Vec<usize> {
        let n = self.project.tracks.len();
        let mut indeg = vec![0usize; n];
        for r in &self.project.routes {
            if r.from < n && r.to < n {
                indeg[r.to] += 1;
            }
        }
        let mut order = Vec::with_capacity(n);
        let mut done = vec![false; n];
        loop {
            let mut progressed = false;
            for i in 0..n {
                if !done[i] && indeg[i] == 0 {
                    done[i] = true;
                    order.push(i);
                    progressed = true;
                    for r in &self.project.routes {
                        if r.from == i && r.to < n {
                            indeg[r.to] = indeg[r.to].saturating_sub(1);
                        }
                    }
                }
            }
            if !progressed {
                break;
            }
        }
        for i in 0..n {
            if !done[i] {
                order.push(i);
            }
        }
        order
    }

    // ── rendering ───────────────────────────────────────────────────────────
    /// Fill `out` (interleaved stereo) with the next block of audio.
    pub fn render(&mut self, out: &mut [f32]) {
        // whichever thread renders (the audio thread, an export, a bounce): denormals flushed to zero
        crate::audio::flush_denormals();
        out.fill(0.0);
        let mut off = 0;
        while off < out.len() {
            let n = (SUB * 2).min(out.len() - off);
            self.render_slice(&mut out[off..off + n]);
            off += n;
        }
    }

    fn render_slice(&mut self, out: &mut [f32]) {
        let frames = out.len() / 2;
        let bps = self.beats_per_sample();
        // audio clips follow the project's base tempo (they are not re-timed by tempo changes)
        let bps_a = self.project.bpm / 60.0 / self.sr as f64;
        let spb = 60.0 / self.project.bpm;
        let n = self.project.tracks.len();
        let dur = bps * frames as f64;
        let (p0, p1) = (self.position, self.position + dur);
        let (s0, s1) = (self.spos, self.spos + dur);
        let playing = self.playing;
        let any_solo = self.project.tracks.iter().any(|t| t.solo);
        let start_clock = self.sample_clock;
        let arr_on = playing && (self.offline || !self.project.session_only);
        let sess_on = playing && !self.offline;
        self.ensure_sess_len();
        if self.outs.len() < n {
            self.outs.resize(n, vec![]);
        }
        if self.key_pk.len() < n {
            self.key_pk.resize(n, 0.0);
        }
        if self.quiet.len() < n {
            self.quiet.resize(n, 0);
        }
        self.mix.clear();
        self.mix.resize(out.len(), 0.0);
        if playing {
            self.apply_automation(p0);
        }
        // activate queued launcher commands that fall inside this slice
        if sess_on {
            for t in 0..n {
                if let Some(q) = self.sess_queue[t] {
                    if q.at < s1 {
                        self.finish_perf(t);
                        match q.cmd {
                            QCmd::Start(c, sc) => {
                                self.sess_play[t] = Some(Playing { clip: c, scene: sc, start: q.at });
                                if self.perf_rec {
                                    self.perf_open[t] = Some((c, self.position));
                                }
                            }
                            QCmd::Stop => {
                                self.sess_play[t] = None;
                                if let Some(i) = self.project.tracks[t].inst.as_mut() {
                                    i.all_off();
                                }
                                self.project.tracks[t].held.clear();
                            }
                        }
                        self.sess_queue[t] = None;
                    }
                }
            }
        }
        let sig = self.route_signature();
        if sig != self.route_sig {
            self.order = self.route_order();
            self.levels.clear();
            self.route_sig = sig;
        }
        let order = std::mem::take(&mut self.order);
        let routes = std::mem::take(&mut self.project.routes);
        let mut audible = std::mem::take(&mut self.audible);
        audible.clear();
        audible.extend(self.project.tracks.iter().map(|t| !t.mute && (!any_solo || t.solo)));

        // Tracks that do not feed each other render side by side: one pass per dependency level
        let n_cov: usize = self.levels.iter().map(|l| l.len()).sum();
        if n_cov != n {
            self.levels = self.route_levels(&order, &routes);
        }
        let levels = std::mem::take(&mut self.levels);
        let duck = duck_events(&self.project.tracks, &routes, p0, p1, bps, frames, arr_on);
        let ctx = SliceCtx { p0, p1, s0, s1, bps, bps_a, spb, bpm: self.project.bpm, frames, out_len: out.len(), start_clock, arr_on, sess_on, n, routes: &routes, audible: &audible, duck: &duck };
        for level in &levels {
            let mut in_level = vec![false; n];
            for &t in level {
                in_level[t] = true;
            }
            let results: Vec<(usize, TrackResult)> = {
                let (outs, key_pk, sess_play, quiet) = (&self.outs, &self.key_pk, &self.sess_play, &self.quiet);
                let items: Vec<(usize, &mut Track)> = self.project.tracks.iter_mut().enumerate().filter(|(i, _)| in_level[*i]).collect();
                run_parallel(items, |(ti, track)| (ti, render_track(track, ti, &ctx, sess_play[ti], outs, key_pk, quiet[ti])))
            };
            for (ti, r) in results {
                self.key_pk[ti] = r.key_pk;
                self.quiet[ti] = r.quiet;
                if audible[ti] {
                    for (d, s) in self.mix.iter_mut().zip(r.out.iter()) {
                        *d += *s;
                    }
                }
                self.outs[ti] = r.out;
            }
        }
        self.levels = levels;

        self.order = order;
        self.project.routes = routes;
        self.audible = audible;
        let bpm_now = self.project.bpm_at(p0);
        for e in self.project.master_effects.iter_mut() {
            e.fx.set_transport(bpm_now, p0);
        }
        for e in self.project.master_effects.iter_mut().filter(|e| !e.bypass) {
            e.process(&mut self.mix);
            scrub(&mut self.mix);
        }
        let mv = self.project.master_volume;
        let mut pk = [0.0f32; 2];
        let limiter = self.project.limiter;
        // ~60 ms release; the gain drops instantly when a peak would pass the ceiling
        let rel = 1.0 - (-1.0 / (0.06 * self.sr)).exp();
        const CEIL: f32 = 0.97;
        for f in 0..frames {
            let mut v = [self.mix[f * 2] * mv, self.mix[f * 2 + 1] * mv];
            // device test tone: 1 s on the left channel, then 1 s on the right
            if self.tone_left > 0 {
                let total = (2.0 * self.sr) as u32;
                let idx = total.saturating_sub(self.tone_left);
                let ch = if (idx as f32) < self.sr { 0 } else { 1 };
                let age = (idx as f32 % self.sr) / self.sr;
                let env = (age * 20.0).min(1.0) * ((1.0 - age) * 20.0).min(1.0);
                self.tone_ph = (self.tone_ph + 440.0 / self.sr).fract();
                v[ch] += (self.tone_ph * std::f32::consts::TAU).sin() * 0.2 * env;
                self.tone_left -= 1;
            }
            if limiter {
                let peak = v[0].abs().max(v[1].abs());
                if peak * self.lim_gain > CEIL {
                    self.lim_gain = CEIL / peak.max(1e-9);
                } else {
                    self.lim_gain += (1.0 - self.lim_gain) * rel;
                }
                self.lim_min = self.lim_min.min(self.lim_gain);
                v[0] *= self.lim_gain;
                v[1] *= self.lim_gain;
            }
            for c in 0..2 {
                let raw = v[c];
                let s = if raw.is_finite() { raw.clamp(-1.0, 1.0) } else { 0.0 };
                out[f * 2 + c] = s;
                pk[c] = pk[c].max(s.abs());
            }
        }
        let m = &mut self.project.master_meter;
        m[0] = pk[0].max(m[0] * 0.995);
        m[1] = pk[1].max(m[1] * 0.995);

        self.sample_clock += frames as u64;
        if self.playing {
            self.position = p1;
            self.spos = s1;
            if !self.offline && self.project.loop_on && self.position >= self.project.loop_end {
                // wrap without cutting sounding notes (they release on their own schedule)
                self.position = self.project.loop_start.max(0.0);
            }
        }
    }

    /// Offline render from beat 0 to the end of the last clip (+ tail seconds).
    pub fn bounce(&mut self, tail_s: f64) -> Vec<f32> {
        let end = self.project.end_beat();
        self.bounce_range(0.0, end, tail_s)
    }

    /// Offline render of `[start, end)` beats plus a release tail, ignoring loop and launcher.
    /// Like `bounce_range`, reporting progress (0..1000) and stopping early when `cancel` is set.
    pub fn bounce_range_progress(&mut self, start: f64, end: f64, tail_s: f64, progress: &std::sync::atomic::AtomicU32, cancel: &std::sync::atomic::AtomicBool, lo: u32, hi: u32) -> Option<Vec<f32>> {
        use std::sync::atomic::Ordering::Relaxed;
        self.project.loop_on = false;
        self.release_all();
        self.offline = true;
        self.position = start;
        self.playing = true;
        let secs = self.project.beats_to_secs(start, end.max(start)) + tail_s;
        let secs = if secs.is_finite() { secs.clamp(0.0, 4.0 * 3600.0) } else { 0.0 };
        let frames = (secs * self.sr as f64) as usize;
        let mut out = vec![0.0f32; frames * 2];
        let mut done = 0usize;
        for chunk in out.chunks_mut(2048) {
            if cancel.load(Relaxed) {
                return None;
            }
            self.render(chunk);
            done += chunk.len() / 2;
            progress.store(lo + (done as u64 * (hi - lo) as u64 / frames.max(1) as u64) as u32, Relaxed);
        }
        self.release_all();
        Some(out)
    }

    pub fn bounce_range(&mut self, start: f64, end: f64, tail_s: f64) -> Vec<f32> {
        let was = (self.playing, self.position, self.spos, self.project.loop_on);
        self.project.loop_on = false;
        self.release_all();
        self.offline = true;
        self.position = start;
        self.playing = true;
        let secs = self.project.beats_to_secs(start, end.max(start)) + tail_s;
        let secs = if secs.is_finite() { secs.clamp(0.0, 4.0 * 3600.0) } else { 0.0 };
        let frames = (secs * self.sr as f64) as usize;
        let mut out = vec![0.0f32; frames * 2];
        for chunk in out.chunks_mut(2048) {
            self.render(chunk);
        }
        self.release_all();
        self.offline = false;
        self.playing = was.0;
        self.position = was.1;
        self.spos = was.2;
        self.project.loop_on = was.3;
        out
    }

    pub fn bounce_wav(&mut self, path: &str, tail_s: f64) -> Result<(), String> {
        let data = self.bounce(tail_s);
        let spec = hound::WavSpec { channels: 2, sample_rate: self.sr as u32, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;
        for s in data {
            w.write_sample((s * 32767.0) as i16).map_err(|e| e.to_string())?;
        }
        w.finalize().map_err(|e| e.to_string())
    }
}

/// The triggers of every Duck route in one slice: for each target track the (frame, velocity) of each note of the source track that the route
/// lets through (its `keys`, or all notes). Read from the source's arrangement clips, so a muted kick still ducks, and exact to the frame.
fn duck_events(tracks: &[Track], routes: &[Route], p0: f64, p1: f64, bps: f64, frames: usize, arr_on: bool) -> Vec<Vec<(u32, f32)>> {
    let n = tracks.len();
    let mut out: Vec<Vec<(u32, f32)>> = vec![Vec::new(); n];
    if !arr_on || frames == 0 || !(bps > 0.0) {
        return out;
    }
    for r in routes.iter().filter(|r| r.kind == RouteKind::Duck && r.from < n && r.to < n && r.from != r.to) {
        for clip in &tracks[r.from].clips {
            let (cstart, clen) = (clip.start, clip.len);
            if p1 <= cstart || p0 >= cstart + clen {
                continue;
            }
            let ClipData::Midi(notes) = &clip.data else { continue };
            let ll = if clip.loop_len >= 0.25 && clip.loop_len < clen { clip.loop_len } else { 0.0 };
            let mut visit = |base: f64, limit: f64| {
                for note in notes {
                    if note.start >= limit {
                        continue;
                    }
                    let ns = base + note.start;
                    if ns >= p0 && ns < p1 && (r.keys.is_empty() || r.keys.contains(&note.key)) {
                        let f = (((ns - p0) / bps).floor().max(0.0) as usize).min(frames - 1) as u32;
                        out[r.to].push((f, (note.vel * r.amount).clamp(0.0, 1.0)));
                    }
                }
            };
            if ll > 0.0 {
                let k0 = (((p0 - cstart) / ll).floor() as i64).max(0);
                let k1 = (((p1 - cstart) / ll).floor() as i64).max(0);
                for k in k0..=k1 {
                    let off = k as f64 * ll;
                    if off >= clen {
                        break;
                    }
                    visit(cstart + off, ll.min(clen - off));
                }
            } else {
                visit(cstart, clen);
            }
        }
    }
    out
}

/// Everything one slice of rendering needs besides the tracks themselves.
struct SliceCtx<'a> {
    p0: f64,
    p1: f64,
    s0: f64,
    s1: f64,
    bps: f64,
    bps_a: f64,
    spb: f64,
    bpm: f64,
    frames: usize,
    out_len: usize,
    start_clock: u64,
    arr_on: bool,
    sess_on: bool,
    n: usize,
    routes: &'a [Route],
    audible: &'a [bool],
    /// per track: (frame, velocity) of every trigger the Duck routes into it fire in this slice
    duck: &'a [Vec<(u32, f32)>],
}

struct TrackResult {
    /// post-fader, post-pan stereo block
    out: Vec<f32>,
    /// pre-fader peak (sidechain key)
    key_pk: f32,
    /// samples of silence so far (see SLEEP_AFTER)
    quiet: u32,
}

/// A track silent (below -100 dB after its effects, so every tail has died away) for this many samples - about 3 s - with no
/// note held, no audio clip under the playhead and nothing routed into it, sleeps: its instrument and effects are skipped until
/// something arrives. What would have come out is silence, so the sound is the same; the CPU (and the engine lock the interface
/// waits on) is free for the tracks that play.
pub const SLEEP_AFTER: u32 = 131_072;
static SLEEP_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Switch track sleeping on or off (on by default; silent tracks only, so the output is the same either way).
pub fn set_track_sleep(on: bool) {
    SLEEP_ON.store(on, std::sync::atomic::Ordering::Relaxed);
}
const SILENT: f32 = 1e-5;

thread_local! {
    static TRACK_TMP: std::cell::RefCell<Vec<f32>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Render one track for one slice: its clips and notes, the instrument, sends from earlier tracks, its effect chain,
/// fader and pan. Touches only this track plus read-only data, so tracks in one dependency level can run in parallel.
fn render_track(track: &mut Track, ti: usize, c: &SliceCtx, play_state: Option<Playing>, outs: &[Vec<f32>], key_pk: &[f32], quiet_in: u32) -> TrackResult {
    let SliceCtx { p0, p1, s0, s1, bps, bps_a, spb, bpm, frames, out_len, start_clock, arr_on, sess_on, n, routes, audible, duck } = *c;
    // a muted track nothing listens to (no send, sidechain key or modulator route from it) is not rendered at all
    if !audible[ti] && SLEEP_ON.load(std::sync::atomic::Ordering::Relaxed) && !routes.iter().any(|r| r.from == ti && r.to != ti) {
        track.meter = [track.meter[0] * 0.995, track.meter[1] * 0.995];
        return TrackResult { out: vec![0.0; out_len], key_pk: 0.0, quiet: quiet_in.max(SLEEP_AFTER) };
    }
    TRACK_TMP.with(|cell| {
        let mut tmp_buf = cell.borrow_mut();
        let mut tmp: Vec<f32> = std::mem::take(&mut *tmp_buf);
        tmp.clear();
        tmp.resize(out_len, 0.0);

            if arr_on {
                for clip in track.clips.iter() {
                    let (cstart, clen) = (clip.start, clip.len);
                    if p1 <= cstart || p0 >= cstart + clen {
                        continue;
                    }
                    let ll = if clip.loop_len >= 0.25 && clip.loop_len < clen { clip.loop_len } else { 0.0 };
                    match &clip.data {
                        ClipData::Midi(notes) => {
                            if ll > 0.0 {
                                let k0 = (((p0 - cstart) / ll).floor() as i64).max(0);
                                let k1 = (((p1 - cstart) / ll).floor() as i64).max(0);
                                for k in k0..=k1 {
                                    let off = k as f64 * ll;
                                    if off >= clen {
                                        break;
                                    }
                                    sched_notes(&mut track.inst, &mut track.held, notes, cstart + off, ll.min(clen - off), p0, p1, p0, bps, start_clock);
                                }
                            } else {
                                sched_notes(&mut track.inst, &mut track.held, notes, cstart, clen, p0, p1, p0, bps, start_clock)
                            }
                        }
                        ClipData::Audio { sample, offset_s, gain, pitch, warp } => {
                            mix_clip_audio(&mut tmp, sample, *offset_s, *gain, *pitch, *warp, bpm, p0 - cstart, bps_a, spb, frames, clen, ll);
                        }
                    }
                }
            }
            if sess_on {
                if let Some(pl) = play_state {
                    if let Some(clip) = track.session.get(pl.clip) {
                        let len = clip.len.max(0.25);
                        let (l0, l1) = ((s0 - pl.start).max(0.0), (s1 - pl.start).max(0.0));
                        if l1 > l0 || s1 > pl.start {
                            match &clip.data {
                                ClipData::Midi(notes) => {
                                    // widen by a hair: the [w0, w1) test in sched_notes is the exact filter
                                    let k0 = (((l0 - 1e-6) / len).floor() as i64).max(0);
                                    let k1 = (((l1 + 1e-6) / len).floor() as i64).max(0);
                                    for k in k0..=k1 {
                                        let base = pl.start + k as f64 * len;
                                        sched_notes(&mut track.inst, &mut track.held, notes, base, len, s0.max(pl.start), s1, s0, bps, start_clock);
                                    }
                                }
                                ClipData::Audio { sample, offset_s, gain, pitch, warp } => {
                                    mix_clip_audio(&mut tmp, sample, *offset_s, *gain, *pitch, *warp, bpm, s0 - pl.start, bps_a, spb, frames, f64::MAX, len);
                                }
                            }
                        }
                    }
                }
            }
            // asleep and still nothing to do: no notes held, no audio under the playhead, nothing routed in -> silence, unrendered
            if quiet_in >= SLEEP_AFTER && SLEEP_ON.load(std::sync::atomic::Ordering::Relaxed) && track.held.is_empty() && track.inst.as_ref().map(|i| i.can_sleep()).unwrap_or(true) && tmp.iter().all(|x| x.abs() < SILENT) {
                let fed = routes.iter().any(|r| r.to == ti && r.from < n && r.from != ti && matches!(r.kind, RouteKind::Send | RouteKind::Mod) && outs[r.from].iter().any(|x| x.abs() >= SILENT));
                if !fed {
                    if let Some(i) = track.inst.as_mut() {
                        i.idle(frames);
                    }
                    track.meter = [track.meter[0] * 0.995, track.meter[1] * 0.995];
                    *tmp_buf = tmp;
                    return TrackResult { out: vec![0.0; out_len], key_pk: 0.0, quiet: quiet_in };
                }
            }
            // note-offs due within this slice
            let end_clock = start_clock + frames as u64;
            let mut k = 0;
            while k < track.held.len() {
                if track.held[k].1 <= end_clock {
                    let key = track.held.swap_remove(k).0;
                    if let Some(i) = track.inst.as_mut() {
                        i.note_off(key);
                    }
                } else {
                    k += 1;
                }
            }
            if let Some(i) = track.inst.as_mut() {
                i.set_tempo(bpm);
                i.render(&mut tmp);
            }
            // sends into this track (post-fader from already-processed sources)
            let mut sc_level: Option<f32> = None;
            let mut aux: Option<Vec<f32>> = None;
            for r in routes.iter().filter(|r| r.to == ti && r.from < n && r.from != ti) {
                if r.kind == RouteKind::Duck {
                    continue; // note-triggered: handled through `duck`
                }
                if r.kind == RouteKind::Mod {
                    // the modulator input of this track's effects: audible or not, the source feeds it
                    let src = &outs[r.from];
                    if src.len() == tmp.len() {
                        let a = aux.get_or_insert_with(|| vec![0.0; tmp.len()]);
                        for (d, s) in a.iter_mut().zip(src.iter()) {
                            *d += *s * r.amount;
                        }
                    }
                    continue;
                }
                if r.kind == RouteKind::Sidechain {
                    // key is tapped pre-fader / pre-mute so a muted trigger track still keys
                    let pk = key_pk[r.from] * r.amount;
                    sc_level = Some(sc_level.map_or(pk, |c: f32| c.max(pk)));
                    continue;
                }
                if !audible[r.from] {
                    continue;
                }
                let src = &outs[r.from];
                match r.kind {
                    RouteKind::Send => {
                        if src.len() == tmp.len() {
                            for (d, s) in tmp.iter_mut().zip(src.iter()) {
                                *d += *s * r.amount;
                            }
                        }
                    }
                    RouteKind::Sidechain | RouteKind::Duck | RouteKind::Mod => {}
                }
            }
            for e in track.effects.iter_mut() {
                e.fx.set_sidechain(sc_level);
                e.fx.set_aux(aux.as_deref());
                e.fx.set_triggers(&duck[ti]);
                e.fx.set_transport(bpm, p0);
            }
            for e in track.effects.iter_mut().filter(|e| !e.bypass) {
                e.process(&mut tmp);
                scrub(&mut tmp);
            }
            let kpk = tmp.iter().fold(0.0f32, |a, b| a.max(b.abs()));
            // fader + equal-power pan + meter
            let ang = (track.pan + 1.0) * std::f32::consts::FRAC_PI_4;
            let (gl, gr) = (ang.cos() * track.volume * std::f32::consts::SQRT_2, ang.sin() * track.volume * std::f32::consts::SQRT_2);
            let mut pk = [0.0f32; 2];
            let mut ob = vec![0.0f32; out_len];
            for f in 0..frames {
                let (l, r) = (tmp[f * 2] * gl, tmp[f * 2 + 1] * gr);
                ob[f * 2] = l;
                ob[f * 2 + 1] = r;
                pk[0] = pk[0].max(l.abs());
                pk[1] = pk[1].max(r.abs());
            }
            track.meter[0] = pk[0].max(track.meter[0] * 0.995);
            track.meter[1] = pk[1].max(track.meter[1] * 0.995);

        *tmp_buf = tmp;
        let quiet = if kpk < SILENT { quiet_in.saturating_add(frames as u32) } else { 0 };
        TrackResult { out: ob, key_pk: kpk, quiet }
    })
}

static PARALLEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Switch parallel track rendering on or off (on by default; the result is identical either way).
pub fn set_parallel_render(on: bool) {
    PARALLEL.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Run `f` over `items`, side by side on the render pool when there is enough work, in order otherwise.
fn run_parallel<T: Send, R: Send>(items: Vec<T>, f: impl Fn(T) -> R + Sync + Send) -> Vec<R> {
    use rayon::prelude::*;
    match render_pool() {
        Some(pool) if items.len() >= 3 && PARALLEL.load(std::sync::atomic::Ordering::Relaxed) => pool.install(|| items.into_par_iter().map(f).collect()),
        _ => items.into_iter().map(f).collect(),
    }
}

/// A small pool of high-priority DSP threads (one fewer than the cores, the audio thread is the other one).
/// A NaN or infinity out of any effect would travel on through the mix and the sound card driver (and stay in every feedback path):
/// a non-finite sample becomes silence, and everything is held inside +-32.
#[inline]
fn scrub(buf: &mut [f32]) {
    for x in buf.iter_mut() {
        if !x.is_finite() {
            *x = 0.0;
        } else if x.abs() > 32.0 {
            *x = x.signum() * 32.0;
        }
    }
}

fn render_pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: std::sync::OnceLock<Option<rayon::ThreadPool>> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        if cores < 3 {
            return None;
        }
        rayon::ThreadPoolBuilder::new()
            .num_threads((cores - 1).min(8))
            .thread_name(|i| format!("lsx-dsp-{i}"))
            .start_handler(|_| crate::audio::prepare_dsp_thread())
            .build()
            .ok()
    })
    .as_ref()
}

#[cfg(test)]
mod wet_tests {
    use super::*;
    use crate::model::{AutoLane, EffectSlot, Track, TrackKind};

    #[test]
    fn a_lane_on_fx_wet_moves_the_dry_wet_and_the_project_remembers_it() {
        let mut e = Engine::new(48000.0);
        let mut t = Track::new("T", TrackKind::Instrument, 0, None);
        t.effects.push(EffectSlot::new("Articulator", 48000.0).unwrap());
        t.auto.push(AutoLane { param: "fx0.wet".into(), points: vec![(0.0, 1.0), (4.0, 0.0)] });
        e.project.tracks.push(t);
        e.apply_automation(2.0);
        let w = e.project.tracks[0].effects[0].wet;
        assert!((w - 0.5).abs() < 1e-5, "halfway down the lane: {w}");
        e.apply_automation(4.0);
        assert!(e.project.tracks[0].effects[0].wet < 1e-5);
        // saved and opened again, the dry/wet comes back
        e.project.tracks[0].effects[0].wet = 0.25;
        let dir = std::env::temp_dir().join(format!("wet-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.json");
        crate::project::save(&e, path.to_str().unwrap()).unwrap();
        let mut e2 = Engine::new(48000.0);
        let doc = crate::project::read(path.to_str().unwrap()).unwrap();
        crate::project::apply_doc(&mut e2, &doc, path.parent(), &mut std::collections::HashMap::new());
        assert_eq!(e2.project.tracks[0].effects[0].name, "Articulator");
        assert!((e2.project.tracks[0].effects[0].wet - 0.25).abs() < 1e-6);
        assert_eq!(e2.project.tracks[0].auto[0].param, "fx0.wet");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
