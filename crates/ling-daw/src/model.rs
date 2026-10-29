//! Project data model: tracks, clips, notes, samples.
use crate::dsp::fx::{self, Effect};
use crate::dsp::Instrument;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub const PPQ: f64 = 1.0; // positions are measured in beats (quarter notes)

#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub start: f64, // beats, relative to clip start
    pub len: f64,
    pub key: u8,
    pub vel: f32,
    pub pan: f32,    // -1..1
    pub cutoff: f32, // 0..1, 0.5 = neutral
    pub reso: f32,   // 0..1, 0.5 = neutral
    /// lyric for sung instruments: `~` when tied (legato) + IPA syllables joined by `-`; empty = none
    pub lyric: String,
    /// per-note pitch gesture (scoop, fall, slide, dive, riser): (position 0..1 of the note, semitones) breakpoints, see
    /// gesture.rs; empty = the note plays its key
    pub bend: Curve,
    /// per-note pressure (polyphonic aftertouch) 0..1 over the note, same breakpoints; empty = none (0)
    pub press: Curve,
    /// per-note timbre (MPE slide / Y) 0..1 over the note, 0.5 = neutral; empty = none
    pub timbre: Curve,
}
/// A per-note expression curve: (position 0..1 of the note's length, value) breakpoints in position order, linear between them,
/// held before the first and after the last. Empty = none.
pub type Curve = Vec<(f32, f32)>;
impl Note {
    pub fn new(start: f64, len: f64, key: u8, vel: f32) -> Note {
        Note { start, len, key, vel, pan: 0.0, cutoff: 0.5, reso: 0.5, lyric: String::new(), bend: vec![], press: vec![], timbre: vec![] }
    }
    /// Does the note carry a per-note expression curve (pitch gesture, pressure or timbre)?
    pub fn has_expr(&self) -> bool {
        !self.bend.is_empty() || !self.press.is_empty() || !self.timbre.is_empty()
    }
    /// Keep only the part of the note from `a` to `b` beats after its start (a cut, a split, a trimmed tail): the start moves by
    /// `a`, the length becomes `b - a`, and the expression curves keep what sounded in that part, in place.
    pub fn cut(&mut self, a: f64, b: f64) {
        let (a, b) = (a.max(0.0), b.min(self.len).max(a.max(0.0)));
        if self.has_expr() && self.len > 0.0 && (a > 1e-9 || b < self.len - 1e-9) {
            let (fa, fb) = ((a / self.len) as f32, (b / self.len) as f32);
            for c in [&mut self.bend, &mut self.press, &mut self.timbre] {
                *c = crate::gesture::trim(c, fa, fb);
            }
        }
        self.start += a;
        self.len = b - a;
    }
}

/// Decoded audio kept as interleaved stereo at its native rate.
pub struct Sample {
    pub name: String,
    pub path: String,
    pub rate: u32,
    pub stereo: Vec<f32>,
    /// Cache of time-stretched / pitch-shifted renditions, keyed (pitch cents, stretch permille).
    pub variants: Mutex<Variants>,
}

#[derive(Default)]
pub struct Variants {
    map: HashMap<(i32, i32), Arc<Sample>>,
    order: Vec<(i32, i32)>,
    wanted: Vec<(i32, i32)>,
    busy: bool,
}

pub const MAX_SCENES: usize = 1024;
pub const MAX_TRACKS: usize = 256;

impl Sample {
    pub fn new(name: String, path: String, rate: u32, stereo: Vec<f32>) -> Sample {
        Sample { name, path, rate, stereo, variants: Mutex::new(Variants::default()) }
    }
    /// Non-blocking lookup (used from the audio thread).
    pub fn cached(&self, key: (i32, i32)) -> Option<Arc<Sample>> {
        self.variants.try_lock().ok()?.map.get(&key).cloned()
    }
}

/// (pitch in cents, stretch in permille). Stretch = source_bpm / project_bpm (duration multiplier).
pub fn variant_key(pitch: f32, warp: f32, bpm: f64) -> (i32, i32) {
    let cents = (pitch * 100.0).round() as i32;
    let permille = if warp > 1.0 && bpm > 1.0 { (1000.0 * warp as f64 / bpm).round() as i32 } else { 1000 };
    (cents, permille.clamp(200, 4000))
}

/// Ask a background thread to render the variant (coalesces rapid requests).
pub fn request_variant(s: &Arc<Sample>, key: (i32, i32)) {
    if key == (0, 1000) {
        return;
    }
    {
        let mut v = s.variants.lock().unwrap_or_else(|p| p.into_inner());
        if v.map.contains_key(&key) {
            return;
        }
        if !v.wanted.contains(&key) {
            v.wanted.push(key);
        }
        if v.busy {
            return;
        }
        v.busy = true;
    }
    let s2 = s.clone();
    std::thread::spawn(move || loop {
        let key = {
            let mut v = s2.variants.lock().unwrap_or_else(|p| p.into_inner());
            let mut found = None;
            while let Some(k) = v.wanted.pop() {
                if !v.map.contains_key(&k) {
                    found = Some(k);
                    break;
                }
            }
            match found {
                Some(k) => k,
                None => {
                    v.busy = false;
                    return;
                }
            }
        };
        let out = crate::stretch::process(&s2.stereo, key.1 as f64 / 1000.0, key.0 as f64 / 100.0);
        let ns = Arc::new(Sample::new(s2.name.clone(), String::new(), s2.rate, out));
        let mut v = s2.variants.lock().unwrap_or_else(|p| p.into_inner());
        v.map.insert(key, ns);
        v.order.push(key);
        while v.order.len() > 8 {
            let old = v.order.remove(0);
            v.map.remove(&old);
        }
    });
}
impl Sample {
    pub fn frames(&self) -> usize {
        self.stereo.len() / 2
    }
    pub fn seconds(&self) -> f64 {
        self.frames() as f64 / self.rate as f64
    }
    /// Peak envelope with `n` buckets, for waveform drawing.
    pub fn peaks(&self, n: usize) -> Vec<f32> {
        if n == 0 || self.frames() == 0 {
            return vec![0.0; n];
        }
        let frames = self.frames();
        (0..n)
            .map(|i| {
                let a = i * frames / n;
                let b = (((i + 1) * frames / n).max(a + 1)).min(frames);
                (a..b).map(|f| self.stereo[f * 2].abs().max(self.stereo[f * 2 + 1].abs())).fold(0.0, f32::max)
            })
            .collect()
    }
}

#[derive(Clone)]
pub enum ClipData {
    Midi(Vec<Note>),
    Audio { sample: Arc<Sample>, offset_s: f64, gain: f32, pitch: f32, warp: f32 },
}

#[derive(Clone)]
pub struct Clip {
    pub start: f64, // beats
    pub len: f64,   // beats
    pub name: String,
    /// Content length that repeats to fill the clip (0 = play once).
    pub loop_len: f64,
    /// the lyric phrase as typed for this clip and the language it was typed in (see lyrics.rs)
    pub lyric_text: String,
    pub lyric_lang: String,
    /// free-form marker: "vox:<id>" on the MIDI clip of a vocal converted to notes (see vocmidi.rs)
    pub tag: String,
    pub data: ClipData,
}
impl Clip {
    pub fn end(&self) -> f64 {
        self.start + self.len
    }
    pub fn midi(start: f64, len: f64, name: &str) -> Clip {
        Clip { start, len, name: name.into(), loop_len: 0.0, lyric_text: String::new(), lyric_lang: String::new(), tag: String::new(), data: ClipData::Midi(vec![]) }
    }
}

pub struct EffectSlot {
    pub name: String,
    pub params: Vec<(&'static str, f32)>,
    pub bypass: bool,
    /// dry / wet of the whole effect, 0 = only the input, 1 = only the effect (any effect, not just the ones with a mix knob);
    /// automatable as "fx<i>.wet"
    pub wet: f32,
    pub fx: Box<dyn Effect>,
}
impl EffectSlot {
    pub fn new(name: &str, sr: f32) -> Option<EffectSlot> {
        let fx = fx::make(name, sr)?;
        Some(EffectSlot { name: name.into(), params: fx::defaults(name).to_vec(), bypass: false, wet: 1.0, fx })
    }
    /// Run the effect on a block (interleaved stereo, in place) and blend it with the untouched input by `wet`.
    pub fn process(&mut self, buf: &mut [f32]) {
        let w = self.wet.clamp(0.0, 1.0);
        if w >= 0.999 {
            self.fx.process(buf);
            return;
        }
        let dry = buf.to_vec();
        self.fx.process(buf);
        let (kd, kw) = (1.0 - w, w);
        for (o, d) in buf.iter_mut().zip(dry.iter()) {
            *o = *d * kd + *o * kw;
        }
    }
    pub fn set(&mut self, idx: usize, v: f32) {
        if !v.is_finite() {
            return;
        }
        if let Some(p) = self.params.get_mut(idx) {
            p.1 = v.clamp(0.0, 1.0);
            self.fx.set_param(p.0, p.1);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum RouteKind {
    Send,
    Sidechain,
    /// notes of the source track (only the ones in `keys`, or all) trigger the target's Ducker effect: a kick can duck the bass
    Duck,
    /// the source track's audio (even when it is muted) is the modulator input of the target's effects (the Vocoder)
    Mod,
}

/// Track-to-track audio route (post-fader send, or sidechain key for the target's compressor).
#[derive(Clone, Debug)]
pub struct Route {
    pub kind: RouteKind,
    pub from: usize,
    pub to: usize,
    pub amount: f32,
    /// Duck routes: the MIDI note numbers that trigger it (empty = every note)
    pub keys: Vec<u8>,
}

/// Automation lane: `param` is "vol", "pan", an instrument parameter name, or "fx<i>.<p>".
#[derive(Clone, Debug, Default)]
pub struct AutoLane {
    pub param: String,
    pub points: Vec<(f64, f32)>, // (beat, value) sorted by beat
}
impl AutoLane {
    pub fn value_at(&self, beat: f64) -> Option<f32> {
        let p = &self.points;
        if p.is_empty() {
            return None;
        }
        if beat <= p[0].0 {
            return Some(p[0].1);
        }
        if beat >= p[p.len() - 1].0 {
            return Some(p[p.len() - 1].1);
        }
        let i = p.partition_point(|x| x.0 <= beat);
        if i == 0 || i >= p.len() {
            return Some(p[p.len() - 1].1);
        }
        let (a, b) = (p[i - 1], p[i]);
        let t = ((beat - a.0) / (b.0 - a.0).max(1e-9)) as f32;
        Some(a.1 + (b.1 - a.1) * t)
    }
}

pub enum TrackKind {
    Instrument,
    Audio,
}

pub struct Track {
    pub name: String,
    pub kind: TrackKind,
    pub color: u32,
    pub patch: String, // path or builtin id the instrument was built from
    pub inst_kind: String, // "synth" | "drums"
    pub inst: Option<Box<dyn Instrument>>,
    pub clips: Vec<Clip>,
    pub effects: Vec<EffectSlot>,
    pub volume: f32, // linear 0..1.5
    pub pan: f32,    // -1..1
    pub mute: bool,
    pub solo: bool,
    pub meter: [f32; 2],
    pub auto: Vec<AutoLane>,
    /// Clips that live in the Performance launcher (not on the timeline).
    pub session: Vec<Clip>,
    /// Notes currently sounding: (key, absolute end sample). Used for note-offs.
    pub(crate) held: Vec<(u8, u64)>,
    /// Sounding notes with per-note expression curves (pitch gesture, pressure, timbre), fed to the instrument every block.
    pub(crate) expr: Vec<ExprPlay>,
}
/// A sounding note with expression curves: its key, where it sits on the clock that scheduled it (`sess`: the launcher clock,
/// else the arrangement's), in beats, the sample its note-off is due, and the curves.
#[derive(Clone, Debug)]
pub(crate) struct ExprPlay {
    pub key: u8,
    pub start: f64,
    pub len: f64,
    pub sess: bool,
    pub end_clock: u64,
    pub bend: Curve,
    pub press: Curve,
    pub timbre: Curve,
}
impl Track {
    pub fn new(name: &str, kind: TrackKind, color: u32, inst: Option<Box<dyn Instrument>>) -> Track {
        Track {
            name: name.into(),
            kind,
            color,
            patch: String::new(),
            inst_kind: String::new(),
            inst,
            clips: vec![],
            effects: vec![],
            volume: 0.8,
            pan: 0.0,
            mute: false,
            solo: false,
            meter: [0.0; 2],
            auto: vec![],
            session: vec![],
            held: Vec::with_capacity(64),
            expr: vec![],
        }
    }
}

pub struct Project {
    pub name: String,
    pub bpm: f64,
    /// tempo changes (beat, bpm), sorted; `bpm` applies before the first one
    pub tempo_map: Vec<(f64, f64)>,
    pub tracks: Vec<Track>,
    pub master_volume: f32,
    /// master brick-wall limiter (on by default)
    pub limiter: bool,
    pub master_effects: Vec<EffectSlot>,
    pub master_meter: [f32; 2],
    pub loop_on: bool,
    pub loop_start: f64,
    pub loop_end: f64,
    pub samples: Vec<Arc<Sample>>,
    pub meta: Vec<(String, String)>,
    pub routes: Vec<Route>,
    pub master_auto: Vec<AutoLane>,
    pub scenes: Vec<String>,
    /// slots[scene][track] = index into that track's `session` clips, or -1
    pub slots: Vec<Vec<i32>>,
    pub session_only: bool,
    /// Launch quantize in beats (0 = immediate).
    pub quant: f64,
}
impl Project {
    pub fn new() -> Project {
        Project {
            name: "Untitled".into(),
            bpm: 120.0,
            tempo_map: vec![],
            tracks: vec![],
            master_volume: 0.9,
            limiter: true,
            master_effects: vec![],
            master_meter: [0.0; 2],
            loop_on: false,
            loop_start: 0.0,
            loop_end: 16.0,
            samples: vec![],
            meta: vec![],
            routes: vec![],
            master_auto: vec![],
            scenes: vec![],
            slots: vec![],
            session_only: false,
            quant: 4.0,
        }
    }
    pub fn slot(&self, scene: usize, track: usize) -> i32 {
        self.slots.get(scene).and_then(|r| r.get(track)).copied().unwrap_or(-1)
    }
    pub fn set_slot(&mut self, scene: usize, track: usize, v: i32) {
        if scene >= MAX_SCENES || track >= MAX_TRACKS {
            return;
        }
        while self.slots.len() <= scene {
            self.slots.push(vec![]);
        }
        let row = &mut self.slots[scene];
        while row.len() <= track {
            row.push(-1);
        }
        row[track] = v;
    }
    /// Remove a track and keep routes / launcher slots consistent.
    pub fn remove_track(&mut self, i: usize) {
        if i >= self.tracks.len() {
            return;
        }
        self.tracks.remove(i);
        self.routes.retain(|r| r.from != i && r.to != i);
        for r in self.routes.iter_mut() {
            if r.from > i {
                r.from -= 1;
            }
            if r.to > i {
                r.to -= 1;
            }
        }
        for row in self.slots.iter_mut() {
            if i < row.len() {
                row.remove(i);
            }
        }
    }
    /// Move track `i` to index `j`, remapping route endpoints and launcher columns.
    pub fn move_track(&mut self, i: usize, j: usize) {
        let n = self.tracks.len();
        if i >= n || j >= n || i == j {
            return;
        }
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
        let t = self.tracks.remove(i);
        self.tracks.insert(j, t);
        for r in self.routes.iter_mut() {
            r.from = map(r.from);
            r.to = map(r.to);
        }
        for row in self.slots.iter_mut() {
            if row.len() < n {
                row.resize(n, -1);
            }
            let v = row.remove(i);
            row.insert(j, v);
        }
    }
    /// True if adding a route from -> to would create a cycle.
    pub fn would_cycle(&self, from: usize, to: usize) -> bool {
        if from == to {
            return true;
        }
        let mut stack = vec![to];
        let mut seen = vec![false; self.tracks.len().max(from.max(to) + 1)];
        while let Some(n) = stack.pop() {
            if n == from {
                return true;
            }
            if n >= seen.len() || seen[n] {
                continue;
            }
            seen[n] = true;
            for r in &self.routes {
                if r.from == n {
                    stack.push(r.to);
                }
            }
        }
        false
    }
    pub fn remove_session_clip(&mut self, t: usize, c: usize) {
        if let Some(tr) = self.tracks.get_mut(t) {
            if c < tr.session.len() {
                tr.session.remove(c);
            }
        }
        for row in self.slots.iter_mut() {
            if let Some(v) = row.get_mut(t) {
                if *v == c as i32 {
                    *v = -1;
                } else if *v > c as i32 {
                    *v -= 1;
                }
            }
        }
    }
    /// Kick off background rendering of every warped / transposed audio clip at the current tempo.
    pub fn prepare_audio(&self) {
        for t in &self.tracks {
            for c in t.clips.iter().chain(t.session.iter()) {
                if let ClipData::Audio { sample, pitch, warp, .. } = &c.data {
                    request_variant(sample, variant_key(*pitch, *warp, self.bpm));
                }
            }
        }
    }
    /// Tempo at a beat.
    pub fn bpm_at(&self, beat: f64) -> f64 {
        let mut b = self.bpm;
        for (pb, v) in &self.tempo_map {
            if *pb <= beat {
                b = *v;
            } else {
                break;
            }
        }
        b
    }
    /// Seconds between two beats, following the tempo map.
    pub fn beats_to_secs(&self, a: f64, b: f64) -> f64 {
        if b <= a {
            return 0.0;
        }
        let (mut cur, mut bpm, mut secs) = (a, self.bpm_at(a), 0.0);
        for (pb, v) in &self.tempo_map {
            if *pb > cur && *pb < b {
                secs += (pb - cur) * 60.0 / bpm;
                cur = *pb;
                bpm = *v;
            }
        }
        secs + (b - cur) * 60.0 / bpm
    }
    pub fn end_beat(&self) -> f64 {
        self.tracks.iter().flat_map(|t| t.clips.iter().map(|c| c.end())).fold(0.0, f64::max)
    }
}
/// A bass preset whose sound already sits an octave (or more) below the written note: its name (the patch file or the preset) says
/// "bass" and, for Prism, the sub oscillator is up or the louder oscillator is set below octave 0. A bassline written in the bass
/// register on such a track sounds two octaves down - see [`lift_low_bass`].
pub fn plays_octave_down(t: &Track, preset_name: &str) -> bool {
    let stem = std::path::Path::new(&t.patch).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    let named = stem.contains("bass") || preset_name.to_lowercase().contains("bass");
    if !named {
        return false;
    }
    let Some(i) = t.inst.as_ref() else { return false };
    if t.inst_kind != "prism" {
        return false;
    }
    let g = |n: &str, d: f32| i.get_param(n).unwrap_or(d);
    // Prism's octave knobs have five steps, 0.5 = octave 0
    let below = |oct: f32| ((oct * 4.0).round() as i32) < 2;
    g("sub", 0.0) >= 0.3 || (below(g("a oct", 0.5)) && g("a lvl", 0.8) >= 0.3) || (below(g("b oct", 0.5)) && g("b lvl", 0.0) >= 0.3)
}

/// Every MIDI note of the track an octave up when the track plays an octave down ([`plays_octave_down`]) and its notes are written
/// low (median below C3, MIDI 48). Returns whether it moved anything.
pub fn lift_low_bass(t: &mut Track, preset_name: &str) -> bool {
    if !plays_octave_down(t, preset_name) {
        return false;
    }
    let mut keys: Vec<u8> = t.clips.iter().filter_map(|c| if let ClipData::Midi(n) = &c.data { Some(n.iter().map(|x| x.key).collect::<Vec<_>>()) } else { None }).flatten().collect();
    if keys.is_empty() {
        return false;
    }
    keys.sort_unstable();
    if keys[keys.len() / 2] >= 48 {
        return false;
    }
    for c in t.clips.iter_mut() {
        if let ClipData::Midi(n) = &mut c.data {
            for x in n.iter_mut() {
                x.key = (x.key as u16 + 12).min(127) as u8;
            }
        }
    }
    true
}

impl Default for Project {
    fn default() -> Self {
        Self::new()
    }
}

/// Load a WAV/FLAC/OGG/MP3 file through ling-music's decoder.
pub fn load_sample(path: &str) -> Result<Arc<Sample>, String> {
    let d = ling_music::decode::load(path)?;
    let name = std::path::Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(Arc::new(Sample::new(name, path.to_string(), d.rate, d.stereo)))
}

/// Import a .mid as one clip of notes (beats derived from the file's seconds at `bpm`).
pub fn load_midi_clip(path: &str, bpm: f64, start: f64) -> Result<Clip, String> {
    let song = ling_music::midi::load(path)?;
    let bps = bpm / 60.0;
    let notes: Vec<Note> = song
        .notes
        .iter()
        .map(|n| Note::new(n.time as f64 * bps, (n.dur as f64 * bps).max(0.05), n.midi, n.vel as f32 / 127.0))
        .collect();
    let len = notes.iter().map(|n| n.start + n.len).fold(0.0, f64::max).ceil().max(1.0);
    let name = std::path::Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(Clip { start, len, name, loop_len: 0.0, lyric_text: String::new(), lyric_lang: String::new(), tag: String::new(), data: ClipData::Midi(notes) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bass_preset_that_sounds_an_octave_down_lifts_a_low_bassline_once() {
        let inst = crate::dsp::make_instrument("prism", 44100.0);
        let mut t = Track::new("Bass", TrackKind::Instrument, 0, inst);
        t.inst_kind = "prism".into();
        t.patch = "instruments/Prism/Prism_rolling_bass.ling".into();
        if let Some(i) = t.inst.as_mut() {
            i.set_param("sub", 0.45);
            i.set_param("b oct", 0.25);
            i.set_param("b lvl", 0.55);
        }
        let mut c = Clip::midi(0.0, 4.0, "bass");
        if let ClipData::Midi(n) = &mut c.data {
            for k in [33u8, 36, 40, 33] {
                n.push(Note::new(0.0, 1.0, k, 0.8));
            }
        }
        t.clips.push(c);
        assert!(plays_octave_down(&t, ""));
        assert!(lift_low_bass(&mut t, ""));
        let keys = |t: &Track| if let ClipData::Midi(n) = &t.clips[0].data { n.iter().map(|x| x.key).collect::<Vec<_>>() } else { vec![] };
        assert_eq!(keys(&t), vec![45, 48, 52, 45]);
        // already up there: not again
        assert!(!lift_low_bass(&mut t, ""));
        // a pad named nothing like a bass is never touched
        t.patch = "instruments/Prism/Prism_uplift_pad.ling".into();
        assert!(!plays_octave_down(&t, "Uplift Pad"));
    }
}
