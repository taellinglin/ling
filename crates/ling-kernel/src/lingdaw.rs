//! The DAW engine behind LingStationX's `daw_*` builtins on LingOS, step 1:
//! the project model. On the desktop these are served by `ling-daw` (a std
//! engine with cpal audio, threads and file dialogs); in the kernel this
//! module holds the same project — parsed from the very same `project.json`
//! format (`ProjectDoc`/`TrackDoc`/`ClipDoc`/`NoteDoc`, field for field from
//! ling-daw's `project.rs`, with its serde defaults) — and answers the reads
//! and simple edits with ling-daw `api.rs` semantics, so the Arranger, track
//! list and clip views show a real project.
//!
//! Not here yet: instruments/effects DSP and audio output (the transport
//! clock runs, so the playhead moves, but nothing sounds), audio clips'
//! samples (no storage driver to read stems from), note editing, undo.
//!
//! LingStationX itself opens its "Anthem of Ling Country" starter template on
//! first run; the image embeds that template (and FunKey's project file,
//! listed as a template too — see `expose_projects_as_templates`).

use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::lingrt::{boolean, list_from, mk_str, num, to_f64, to_string, UNIT};

// ── project.json (ling-daw project.rs) ───────────────────────────────────────

fn half() -> f32 {
    0.5
}
fn one() -> f32 {
    1.0
}
fn four() -> f64 {
    4.0
}
fn yes() -> bool {
    true
}

#[derive(Deserialize, Serialize, Clone, Default)]
struct FxDoc {
    #[serde(default)]
    name: String,
    #[serde(default)]
    params: Vec<(String, f32)>,
    #[serde(default)]
    bypass: bool,
    /// dry/wet mix, 0..1 (projects from before the mix knob have none: fully wet)
    #[serde(default = "full_wet")]
    wet: f32,
}

fn full_wet() -> f32 {
    1.0
}

#[derive(Deserialize, Serialize, Clone)]
struct NoteDoc {
    start: f64,
    len: f64,
    key: u8,
    vel: f32,
    #[serde(default)]
    pan: f32,
    #[serde(default = "half")]
    cutoff: f32,
    #[serde(default = "half")]
    reso: f32,
}

#[derive(Deserialize, Serialize, Clone)]
struct ClipDoc {
    #[serde(default)]
    name: String,
    start: f64,
    len: f64,
    #[serde(default)]
    midi_notes: Vec<NoteDoc>,
    #[serde(default)]
    audio_path: String,
    #[serde(default)]
    offset_s: f64,
    #[serde(default = "one")]
    gain: f32,
    #[serde(default)]
    pitch: f32,
    #[serde(default)]
    warp: f32,
    #[serde(default)]
    loop_len: f64,
}

impl ClipDoc {
    fn is_audio(&self) -> bool {
        !self.audio_path.is_empty()
    }
    fn end(&self) -> f64 {
        self.start + self.len
    }
}

#[derive(Deserialize, Serialize, Clone)]
struct TrackDoc {
    #[serde(default)]
    name: String,
    #[serde(default)]
    color: u32,
    #[serde(default)]
    kind: String, // instrument | audio
    #[serde(default)]
    instrument: String, // synth | drums | ...
    #[serde(default)]
    patch: String,
    #[serde(default = "one")]
    level: f32,
    #[serde(default)]
    pan: f32,
    #[serde(default)]
    muted: bool,
    #[serde(default)]
    solo: bool,
    #[serde(default)]
    effects: Vec<FxDoc>,
    #[serde(default)]
    clips: Vec<ClipDoc>,
    /// The instrument's knobs, (name, 0..1), exactly the PC's `state` field.
    #[serde(default)]
    state: Vec<(String, f32)>,
    #[serde(default)]
    config: String,
}

impl TrackDoc {
    fn is_audio(&self) -> bool {
        self.kind == "audio"
    }

    /// The knobs the Parameters view shows: the saved state, else the
    /// instrument kind's defaults (the PC synth's parameter list).
    fn params(&self) -> Vec<(String, f32)> {
        if !self.state.is_empty() {
            return self.state.clone();
        }
        let defaults: &[(&str, f32)] = if self.instrument == "drums" || self.instrument == "drum" {
            &[("tune", 0.5), ("decay", 0.5), ("tone", 0.5), ("snap", 0.5), ("room", 0.2), ("level", 0.8)]
        } else {
            &[
                ("wave", 0.0), ("sub", 0.3), ("add", 0.0), ("fm", 0.0), ("cutoff", 0.6), ("reso", 0.2), ("filt env", 0.3),
                ("partials", 0.5), ("rolloff", 0.5), ("odd/even", 0.5), ("fm ratio", 0.25), ("fm index", 0.0), ("detune", 0.15),
                ("noise", 0.0), ("attack", 0.1), ("decay", 0.4), ("sustain", 0.7), ("release", 0.3), ("lfo rate", 0.3),
                ("lfo cut", 0.0), ("lfo pitch", 0.0), ("fm env", 0.0),
            ]
        };
        defaults.iter().map(|(n, v)| (String::from(*n), *v)).collect()
    }

    fn param(&self, name: &str) -> Option<f32> {
        self.params().into_iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    fn set_param(&mut self, name: &str, v: f32) {
        if self.state.is_empty() {
            self.state = self.params();
        }
        let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { return };
        match self.state.iter_mut().find(|(n, _)| n == name) {
            Some(slot) => slot.1 = v,
            None => self.state.push((String::from(name), v)),
        }
    }
}

#[derive(Deserialize, Serialize, Clone)]
struct ProjectDoc {
    #[serde(default)]
    name: String,
    #[serde(default)]
    tempo_bpm: f64,
    #[serde(default)]
    tempo_map: Vec<(f64, f64)>,
    #[serde(default = "one")]
    master_level: f32,
    #[serde(default = "yes")]
    limiter: bool,
    #[serde(default)]
    master_effects: Vec<FxDoc>,
    #[serde(default)]
    loop_on: bool,
    #[serde(default)]
    loop_start: f64,
    #[serde(default)]
    loop_end: f64,
    #[serde(default)]
    meta: Vec<(String, String)>,
    #[serde(default = "four")]
    #[allow(dead_code)]
    performance_launch_quantize_beats: f64,
    #[serde(default)]
    tracks: Vec<TrackDoc>,
}

impl ProjectDoc {
    fn empty(name: &str, bpm: f64) -> ProjectDoc {
        ProjectDoc {
            name: String::from(name),
            tempo_bpm: bpm,
            tempo_map: Vec::new(),
            master_level: 0.9,
            limiter: true,
            master_effects: Vec::new(),
            loop_on: false,
            loop_start: 0.0,
            loop_end: 16.0,
            meta: Vec::new(),
            performance_launch_quantize_beats: 4.0,
            tracks: Vec::new(),
        }
    }

    /// ling-daw `apply_doc`'s clean-ups: clamped tempo, sane loop.
    fn normalised(mut self) -> ProjectDoc {
        self.tempo_bpm = if self.tempo_bpm.is_finite() && self.tempo_bpm > 0.0 { self.tempo_bpm.clamp(20.0, 400.0) } else { 120.0 };
        self.tempo_map.retain(|(b, v)| b.is_finite() && v.is_finite() && *b >= 0.0);
        for p in self.tempo_map.iter_mut() {
            p.1 = p.1.clamp(20.0, 400.0);
        }
        self.tempo_map.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(core::cmp::Ordering::Equal));
        if !(self.loop_end > self.loop_start) {
            self.loop_end = self.loop_start + 16.0;
        }
        self.master_level = if self.master_level.is_finite() { self.master_level.clamp(0.0, 2.0) } else { 0.9 };
        self
    }

    fn bpm_at(&self, beat: f64) -> f64 {
        let mut b = self.tempo_bpm;
        for (pb, v) in &self.tempo_map {
            if *pb <= beat {
                b = *v;
            } else {
                break;
            }
        }
        b
    }

    /// Seconds between two beats, following the tempo map (model.rs).
    fn beats_to_secs(&self, a: f64, b: f64) -> f64 {
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

    fn end_beat(&self) -> f64 {
        self.tracks.iter().flat_map(|t| t.clips.iter().map(|c| c.end())).fold(0.0, f64::max)
    }
}

const PALETTE: [u32; 7] = [0xED4A37, 0xF7941E, 0xF7D846, 0x45C870, 0x3BAAEB, 0x4A64D8, 0x9A53D6];

// ── engine state ─────────────────────────────────────────────────────────────

struct Daw {
    project: ProjectDoc,
    path: String,
    status: String,
    dirty: bool,
    /// Bumped on every change (ling-daw's `daw_rev`: the UI re-reads on change).
    rev: u64,
    playing: bool,
    /// Beat position when the transport last started / was seeked.
    base_beat: f64,
    base_ms: u64,
    position: f64,
}

static mut DAW: Option<Daw> = None;

fn daw() -> &'static mut Daw {
    let d = unsafe { &mut *(&raw mut DAW) };
    if d.is_none() {
        let mut fresh = Daw {
            project: ProjectDoc::empty("Untitled", 120.0),
            path: String::new(),
            status: String::new(),
            dirty: false,
            rev: 0,
            playing: false,
            base_beat: 0.0,
            base_ms: 0,
            position: 0.0,
        };
        expose_projects_as_templates();
        fresh.rev = 1;
        *d = Some(fresh);
    }
    d.as_mut().unwrap()
}

/// Every embedded `projects/<name>/project.json` also appears as
/// `assets/templates/<name>/project.json`, so LingStationX's own
/// File > New From Template menu lists it (there's no file dialog on LingOS
/// to open a project with yet).
fn expose_projects_as_templates() {
    let mut names: Vec<String> = Vec::new();
    crate::appfs::list("projects", |name, is_dir| {
        if is_dir {
            names.push(String::from(name));
        }
    });
    for name in names {
        let src = alloc::format!("projects/{name}/project.json");
        if let Some(bytes) = crate::appfs::get(&src) {
            let alias = alloc::format!("assets/templates/{name}/project.json");
            if !crate::appfs::exists(&alias) {
                crate::appfs::register(alloc::boxed::Box::leak(alias.into_boxed_str()), bytes);
            }
        }
    }
}

fn load_into(d: &mut Daw, path: &str) -> Result<(), String> {
    let Some(bytes) = crate::vfs::read(path) else {
        return Err(alloc::format!("{path}: not found"));
    };
    let doc: ProjectDoc = serde_json::from_slice(&bytes).map_err(|e| alloc::format!("{path}: {e}"))?;
    d.project = doc.normalised();
    d.path = String::from(path);
    d.dirty = false;
    d.rev += 1;
    d.playing = false;
    d.position = 0.0;
    d.base_beat = 0.0;
    d.status = alloc::format!("Opened {path}");
    Ok(())
}

/// Advance the transport clock: while playing, the position follows real time
/// at the tempo map's speed, wrapping at the loop end when the loop is on.
fn tick(d: &mut Daw) {
    if !d.playing {
        return;
    }
    let now = crate::arch::timer::now_ms();
    let secs = now.saturating_sub(d.base_ms) as f64 / 1000.0;
    // Walk forward through the tempo map from base_beat for `secs` seconds.
    let p = &d.project;
    let (mut beat, mut left) = (d.base_beat, secs);
    loop {
        let bpm = p.bpm_at(beat);
        let next = p.tempo_map.iter().map(|(b, _)| *b).find(|b| *b > beat);
        match next {
            Some(nb) if (nb - beat) * 60.0 / bpm < left => {
                left -= (nb - beat) * 60.0 / bpm;
                beat = nb;
            },
            _ => {
                beat += left * bpm / 60.0;
                break;
            },
        }
    }
    if p.loop_on && p.loop_end > p.loop_start && beat >= p.loop_end {
        let span = p.loop_end - p.loop_start;
        beat = p.loop_start + (beat - p.loop_start) % span;
        d.base_beat = beat;
        d.base_ms = now;
    } else if !p.loop_on && beat > p.end_beat().max(4.0) + 4.0 {
        d.playing = false;
        beat = 0.0;
    }
    d.position = beat;
}

// ── Tracker view (daw_trk_*): the arrangement as rows and channels ─────────
// Every instrument track becomes as many channels as it has notes sounding at
// once (up to 8): notes are dealt to the lowest free channel, so a chord reads
// as a row of notes across its channels. Audio tracks get one channel whose
// cells mark where clips start. A row is 1 / rows-per-beat beats. Ported from
// ling-daw's tracker_api.rs; the layout is rebuilt when the project changes.
const TRK_VOICES: usize = 8;
const AUDIO_KEY: i64 = 128;

#[derive(Clone)]
struct TrkEv {
    s: f64,
    e: f64,
    key: i64,
    vel: f32,
    clip: usize,
    note: usize,
}

struct TrkChan {
    track: usize,
    voice: usize,
    evs: Vec<TrkEv>,
}

struct TrkLayout {
    rev: u64,
    chans: Vec<TrkChan>,
    end: f64,
}

static mut TRK: Option<TrkLayout> = None;

fn trk_track_events(t: &TrackDoc) -> Vec<TrkEv> {
    let mut out: Vec<TrkEv> = Vec::new();
    for (ci, c) in t.clips.iter().enumerate() {
        if c.is_audio() {
            out.push(TrkEv { s: c.start, e: c.start + c.len, key: AUDIO_KEY, vel: 1.0, clip: ci, note: usize::MAX });
            continue;
        }
        let ll = if c.loop_len >= 0.25 && c.loop_len < c.len { c.loop_len } else { 0.0 };
        let reps = if ll > 0.0 { (libm::ceil(c.len / ll) as usize).min(4096) } else { 1 };
        for k in 0..reps {
            let base = c.start + k as f64 * ll;
            let limit = if ll > 0.0 { ll.min(c.len - k as f64 * ll) } else { c.len };
            for (ni, n) in c.midi_notes.iter().enumerate() {
                if n.start >= limit {
                    continue;
                }
                let s = base + n.start;
                out.push(TrkEv { s, e: s + n.len.min(limit - n.start).max(0.02), key: n.key as i64, vel: n.vel, clip: ci, note: ni });
            }
        }
    }
    out.sort_by(|a, b| a.s.total_cmp(&b.s).then(a.key.cmp(&b.key)));
    out
}

fn trk_build(p: &ProjectDoc, rev: u64) -> TrkLayout {
    let mut chans = Vec::new();
    let mut end = 0f64;
    for (ti, t) in p.tracks.iter().enumerate() {
        let evs = trk_track_events(t);
        end = end.max(evs.iter().map(|e| e.e).fold(0.0, f64::max));
        let mut lanes: Vec<Vec<TrkEv>> = alloc::vec![Vec::new()];
        let mut last_end: Vec<f64> = alloc::vec![f64::MIN];
        if t.is_audio() {
            lanes[0] = evs;
        } else {
            for ev in evs {
                let free = last_end.iter().position(|le| *le <= ev.s + 1e-6);
                let v = match free {
                    Some(v) => v,
                    None if lanes.len() < TRK_VOICES => {
                        lanes.push(Vec::new());
                        last_end.push(f64::MIN);
                        lanes.len() - 1
                    },
                    None => (0..lanes.len()).min_by(|a, b| last_end[*a].total_cmp(&last_end[*b])).unwrap_or(0),
                };
                last_end[v] = ev.e;
                lanes[v].push(ev);
            }
        }
        for (v, evs) in lanes.into_iter().enumerate() {
            chans.push(TrkChan { track: ti, voice: v, evs });
        }
    }
    TrkLayout { rev, chans, end }
}

fn trk_layout(p: &ProjectDoc, rev: u64) -> &'static TrkLayout {
    let slot = unsafe { &mut *(&raw mut TRK) };
    if slot.as_ref().map(|l| l.rev != rev).unwrap_or(true) {
        *slot = Some(trk_build(p, rev));
    }
    slot.as_ref().unwrap()
}

/// Cells of a block of rows: (key, velocity 0..1, hold) row by row for the
/// given channels. key -1 none, 128 an audio clip start; hold 0 none, 1 an
/// earlier note still sounds, 2 an earlier note ended in this row.
fn trk_page_cells(l: &TrkLayout, beat0: f64, rows: usize, rpb: f64, chans: &[usize]) -> Vec<(i64, f32, u8)> {
    let cn = chans.len();
    let rl = 1.0 / rpb;
    let mut cells = alloc::vec![(-1i64, 0f32, 0u8); rows * cn];
    for (ci, chi) in chans.iter().enumerate() {
        let Some(ch) = l.chans.get(*chi) else { continue };
        let i0 = ch.evs.partition_point(|e| e.e <= beat0 - 1e-9);
        for ev in &ch.evs[i0..] {
            if ev.s >= beat0 + rows as f64 * rl {
                break;
            }
            let on_row = libm::floor((ev.s - beat0) / rl + 1e-6);
            let off_row = libm::ceil((ev.e - beat0) / rl - 1e-6);
            if on_row >= 0.0 && (on_row as usize) < rows {
                let c = &mut cells[on_row as usize * cn + ci];
                c.0 = ev.key;
                c.1 = ev.vel;
            }
            let from = (on_row + 1.0).max(0.0) as usize;
            let to = (off_row.max(0.0) as usize).min(rows);
            for r in from..to {
                let c = &mut cells[r * cn + ci];
                if c.0 < 0 && c.2 == 0 {
                    c.2 = 1;
                }
            }
            if off_row >= 0.0 && (off_row as usize) < rows && ev.key != AUDIO_KEY {
                let c = &mut cells[off_row as usize * cn + ci];
                if c.0 < 0 {
                    c.2 = 2;
                }
            }
        }
    }
    cells
}

/// A list argument's numbers (a non-list gives an empty vector).
fn nums_arg(args: &[u64], i: usize) -> Vec<f64> {
    use crate::lingrt::{is_list, list_items};
    match args.get(i) {
        Some(&v) if is_list(v) => list_items(v).iter().map(|&x| to_f64(x)).collect(),
        _ => Vec::new(),
    }
}

// ── Sheet view (daw_sheet_*): a compact score layout of our own ──────────────
// Page 990 x 1400 (portrait). Every `bars`-bar stretch of every chosen
// instrument track with notes in it is one system: one staff (treble, bass by
// the track's average pitch, percussion for drums), five lines 10 apart. The
// geometry list is the PC's: [pw, ph, nsys, (track, b0, b1, x0, bar_w, y top,
// y bottom, nstaves, (clef, y bottom line, half-step)*)*, nnotes, (track, key,
// start, x, y)*] in page units.
const SH_PW: f64 = 990.0;
const SH_PH: f64 = 1400.0;
const SH_LEFT: f64 = 60.0;
const SH_RIGHT: f64 = 40.0;
const SH_TOP: f64 = 90.0;
const SH_BOTTOM: f64 = 60.0;
const SH_STEP: f64 = 5.0;
const SH_SYS_H: f64 = 110.0;

struct ShSys {
    track: usize,
    b0: f64,
    b1: f64,
    x0: f64,
    bar_w: f64,
    y0: f64,
    y1: f64,
    clef: u8,
    yb: f64,
}

struct ShNote {
    track: usize,
    key: u8,
    start: f64,
    x: f64,
    y: f64,
}

struct ShPage {
    systems: Vec<ShSys>,
    notes: Vec<ShNote>,
}

struct ShSheet {
    rev: u64,
    opts: String,
    pages: Vec<ShPage>,
}

static mut SHEET: Option<ShSheet> = None;

fn sh_opt<'a>(opts: &'a str, key: &str) -> &'a str {
    for kv in opts.split(';') {
        if let Some((k, v)) = kv.split_once('=') {
            if k.trim() == key {
                return v.trim();
            }
        }
    }
    ""
}

/// Diatonic number of a key (C-major letters; a sharp sits on the natural below).
fn sh_dn(key: u8) -> i32 {
    const LETTER: [i32; 12] = [0, 0, 1, 1, 2, 3, 3, 4, 4, 5, 5, 6];
    (key as i32 / 12) * 7 + LETTER[(key % 12) as usize]
}

/// Notes of a track in beats of the arrangement (clip loops unrolled).
fn sh_track_notes(t: &TrackDoc) -> Vec<(f64, f64, u8)> {
    let mut out = Vec::new();
    for c in &t.clips {
        if c.is_audio() {
            continue;
        }
        let ll = if c.loop_len >= 0.25 && c.loop_len < c.len { c.loop_len } else { 0.0 };
        let reps = if ll > 0.0 { (libm::ceil(c.len / ll) as usize).min(512) } else { 1 };
        for k in 0..reps {
            let base = c.start + k as f64 * ll;
            let limit = if ll > 0.0 { ll.min(c.len - k as f64 * ll) } else { c.len };
            for n in &c.midi_notes {
                if n.start < limit {
                    out.push((base + n.start, n.len.min(limit - n.start), n.key));
                }
            }
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

fn sh_build(p: &ProjectDoc, opts: &str) -> Vec<ShPage> {
    let bars = sh_opt(opts, "bars").parse::<f64>().unwrap_or(4.0).clamp(1.0, 8.0);
    let from_bar = sh_opt(opts, "from").parse::<f64>().unwrap_or(1.0).max(1.0);
    let to_bar = sh_opt(opts, "to").parse::<f64>().unwrap_or(0.0);
    let chosen: Vec<usize> = sh_opt(opts, "tracks").split(',').filter_map(|s| s.trim().parse::<usize>().ok()).collect();
    let mut pages: Vec<ShPage> = Vec::new();
    let mut page = ShPage { systems: Vec::new(), notes: Vec::new() };
    let mut y = SH_TOP;
    let end = p.end_beat().max(4.0);
    let last_bar = if to_bar > 0.0 { to_bar } else { libm::ceil(end / 4.0) };
    let mut bar = from_bar;
    let x0 = SH_LEFT + 30.0;
    let bar_w = (SH_PW - x0 - SH_RIGHT) / bars;
    while bar <= last_bar {
        let b0 = (bar - 1.0) * 4.0;
        let b1 = b0 + bars * 4.0;
        for (ti, t) in p.tracks.iter().enumerate() {
            if t.is_audio() || (!chosen.is_empty() && !chosen.contains(&ti)) {
                continue;
            }
            let notes = sh_track_notes(t);
            let here: Vec<&(f64, f64, u8)> = notes.iter().filter(|n| n.0 >= b0 - 1e-9 && n.0 < b1 - 1e-9).collect();
            if here.is_empty() {
                continue;
            }
            if y + SH_SYS_H > SH_PH - SH_BOTTOM {
                pages.push(core::mem::replace(&mut page, ShPage { systems: Vec::new(), notes: Vec::new() }));
                y = SH_TOP;
            }
            let drums = t.instrument == "drums" || t.instrument == "drum";
            let avg = here.iter().map(|n| n.2 as f64).sum::<f64>() / here.len() as f64;
            let clef: u8 = if drums { 2 } else if avg < 57.0 { 1 } else { 0 };
            let yb = y + 20.0 + 8.0 * SH_STEP;
            let reference = if clef == 1 { sh_dn(43) } else { sh_dn(64) };
            for n in &here {
                let x = x0 + 14.0 + (n.0 - b0) / 4.0 * bar_w;
                let ny = if drums {
                    yb - ((n.2 as i32 % 8) as f64) * SH_STEP
                } else {
                    yb - ((sh_dn(n.2) - reference) as f64) * SH_STEP
                };
                page.notes.push(ShNote { track: ti, key: n.2, start: n.0, x, y: ny });
            }
            page.systems.push(ShSys { track: ti, b0, b1, x0, bar_w, y0: y, y1: y + SH_SYS_H - 20.0, clef, yb });
            y += SH_SYS_H;
        }
        bar += bars;
    }
    if !page.systems.is_empty() || pages.is_empty() {
        pages.push(page);
    }
    pages
}

fn sh_sheet(p: &ProjectDoc, rev: u64, opts: &str) -> &'static ShSheet {
    let slot = unsafe { &mut *(&raw mut SHEET) };
    if slot.as_ref().map(|s| s.rev != rev || s.opts != opts).unwrap_or(true) {
        *slot = Some(ShSheet { rev, opts: String::from(opts), pages: sh_build(p, opts) });
    }
    slot.as_ref().unwrap()
}

fn effects_of(p: &ProjectDoc, t: i64) -> Option<&Vec<FxDoc>> {
    if t < 0 {
        Some(&p.master_effects)
    } else {
        p.tracks.get(t as usize).map(|t| &t.effects)
    }
}

// ── dispatch ─────────────────────────────────────────────────────────────────

/// The `daw_*` builtins this engine implements; `None` for the rest (logged
/// once by the runtime as not implemented).
pub fn dispatch(name: &[u8], args: &[u64]) -> Option<u64> {
    if !name.starts_with(b"daw_") {
        return None;
    }
    let n = |i: usize| args.get(i).map(|&v| to_f64(v)).unwrap_or(0.0);
    let u = |i: usize| n(i).max(0.0) as usize;
    let s = |i: usize| args.get(i).map(|&v| to_string(v)).unwrap_or_default();
    let d = daw();
    tick(d);
    let p = &mut d.project;
    let track = |p: &ProjectDoc, i: usize| p.tracks.get(i).cloned();
    Some(match name {
        // ── project ──
        b"daw_project_load" => {
            let path = s(0).replace('\\', "/");
            match load_into(d, &path) {
                Ok(()) => boolean(true),
                Err(e) => {
                    d.status = alloc::format!("Open failed: {e}");
                    num(0.0)
                },
            }
        },
        b"daw_new" => {
            d.project = ProjectDoc::empty("Untitled", 120.0);
            d.path.clear();
            d.dirty = false;
            d.rev += 1;
            d.playing = false;
            d.position = 0.0;
            UNIT
        },
        b"daw_project_name" => mk_str(&p.name),
        b"daw_set_project_name" => {
            p.name = s(0);
            d.dirty = true;
            d.rev += 1;
            UNIT
        },
        b"daw_project_path" => mk_str(&d.path),
        b"daw_set_project_path" => {
            d.path = s(0);
            UNIT
        },
        b"daw_status" => mk_str(&core::mem::take(&mut d.status)),
        b"daw_dirty" => boolean(d.dirty),
        b"daw_set_dirty" => {
            d.dirty = n(0) > 0.5;
            UNIT
        },
        b"daw_rev" => num(d.rev as f64),
        // No audio/MIDI devices or recording on LingOS yet: report none.
        b"daw_device" => mk_str("LingOS (no audio output yet)"),
        b"daw_midi_inputs" | b"daw_audio_rec_info" => list_from(Vec::new()),
        b"daw_midi_target" => num(-1.0),
        b"daw_rec_arm" | b"daw_auto_rec" | b"daw_perf_rec" | b"daw_audio_rec_active" => boolean(false),
        b"daw_audio_gain_set" | b"daw_audio_norm_set" | b"daw_audio_input_set" | b"daw_midi_channel"
        | b"daw_audio_config" | b"daw_set_device" | b"daw_midi_open" | b"daw_gm_sf2_set" => UNIT,
        // Saving needs a writable disk (no storage driver yet).
        // Save: `daw_project_save("")` to the project's own path,
        // `_save_as(path)` moves the project there, `_save_copy(path)` writes a
        // copy. project.json as ling-daw writes it, through crate::vfs.
        b"daw_project_save" | b"daw_project_save_copy" | b"daw_project_save_as" => {
            let arg = args.first().map(|&v| to_string(v)).unwrap_or_default();
            let target = if arg.is_empty() { d.path.clone() } else { arg };
            if target.is_empty() {
                d.status = String::from("No project path yet -- use Save As");
                boolean(false)
            } else {
                match serde_json::to_vec_pretty(&d.project) {
                    Ok(json) if crate::vfs::write(&target, &json) => {
                        if name != b"daw_project_save_copy" {
                            d.path = target.clone();
                            d.dirty = false;
                        }
                        d.status = alloc::format!("Saved {target}");
                        boolean(true)
                    },
                    _ => {
                        d.status = if crate::vfs::ready() {
                            alloc::format!("Could not write {target}")
                        } else {
                            String::from("Saving needs storage (no SD card driver on the Pi yet)")
                        };
                        boolean(false)
                    },
                }
            }
        },
        // Undo isn't implemented yet: snapshots are accepted and dropped.
        b"daw_undo_push" | b"daw_undo" | b"daw_redo" => UNIT,
        b"daw_can_undo" | b"daw_can_redo" => boolean(false),

        // ── transport ──
        b"daw_play" => {
            if !d.playing {
                d.playing = true;
                d.base_beat = d.position;
                d.base_ms = crate::arch::timer::now_ms();
                audio_start(d.position);
            }
            UNIT
        },
        b"daw_pause" => {
            audio_stop();
            d.playing = false;
            UNIT
        },
        b"daw_stop" => {
            audio_stop();
            d.playing = false;
            d.position = 0.0;
            UNIT
        },
        b"daw_seek" => {
            d.position = n(0).max(0.0);
            if d.playing {
                audio_start(d.position);
            }
            d.base_beat = d.position;
            d.base_ms = crate::arch::timer::now_ms();
            UNIT
        },
        b"daw_playing" => boolean(d.playing),
        b"daw_position" => num(d.position),
        b"daw_bpm" => num(p.tempo_bpm),
        b"daw_set_bpm" => {
            p.tempo_bpm = n(0).clamp(20.0, 400.0);
            d.dirty = true;
            d.rev += 1;
            UNIT
        },
        b"daw_tempo_points" => {
            list_from(p.tempo_map.iter().flat_map(|(b, v)| [num(*b), num(*v)]).collect())
        },
        b"daw_secs_at" => num(p.beats_to_secs(0.0, n(0).max(0.0))),
        b"daw_loop_on" => boolean(p.loop_on),
        b"daw_loop_start" => num(p.loop_start),
        b"daw_loop_end" => num(p.loop_end),
        b"daw_set_loop" => {
            p.loop_on = n(0) > 0.5;
            if args.len() >= 3 {
                p.loop_start = n(1).max(0.0);
                p.loop_end = n(2).max(n(1) + 0.25);
            }
            UNIT
        },
        b"daw_end_beat" => num(p.end_beat()),

        // ── master ──
        b"daw_master_get" => num(match s(0).as_str() {
            "vol" => p.master_level as f64,
            _ => 0.0,
        }),
        b"daw_master_set" => {
            if s(0) == "vol" {
                p.master_level = n(1).clamp(0.0, 2.0) as f32;
                d.dirty = true;
            d.rev += 1;
            }
            UNIT
        },
        b"daw_limiter_get" => boolean(p.limiter),
        b"daw_limiter_set" => {
            p.limiter = n(0) > 0.5;
            UNIT
        },
        b"daw_arr_tags" => {
            // [start, end, tag]* from the project's "sections" meta
            let text = p.meta.iter().find(|(k, _)| k == "sections").map(|(_, v)| v.clone()).unwrap_or_default();
            let mut v = Vec::new();
            for part in text.split(';') {
                let f: Vec<&str> = part.split(':').collect();
                if f.len() >= 3 {
                    if let (Ok(a), Ok(b)) = (f[0].trim().parse::<f64>(), f[1].trim().parse::<f64>()) {
                        v.push(num(a));
                        v.push(num(b));
                        v.push(mk_str(f[2]));
                    }
                }
            }
            list_from(v)
        },

        // ── tracks ──
        b"daw_track_count" => num(p.tracks.len() as f64),
        b"daw_track_name" => mk_str(&track(p, u(0)).map(|t| t.name).unwrap_or_default()),
        b"daw_track_set_name" => {
            if let Some(t) = p.tracks.get_mut(u(0)) {
                t.name = s(1);
                d.dirty = true;
            d.rev += 1;
            }
            UNIT
        },
        b"daw_track_kind" => mk_str(
            &track(p, u(0)).map(|t| if t.is_audio() { String::from("audio") } else { t.instrument }).unwrap_or_default(),
        ),
        b"daw_track_inst" => mk_str(&match track(p, u(0)) {
            Some(t) if !t.is_audio() => {
                let mut name = t.instrument.clone();
                if let Some(c) = name.get_mut(0..1) {
                    c.make_ascii_uppercase();
                }
                name
            },
            _ => String::from("Audio"),
        }),
        b"daw_track_patch" => mk_str(&track(p, u(0)).map(|t| t.patch).unwrap_or_default()),
        b"daw_track_get" => {
            let Some(t) = p.tracks.get(u(0)) else { return Some(num(0.0)) };
            num(match s(1).as_str() {
                "vol" => t.level as f64,
                "pan" => t.pan as f64,
                "mute" => t.muted as i32 as f64,
                "solo" => t.solo as i32 as f64,
                "audio" => t.is_audio() as i32 as f64,
                "cr" => ((t.color >> 16) & 255) as f64,
                "cg" => ((t.color >> 8) & 255) as f64,
                "cb" => (t.color & 255) as f64,
                "clips" => t.clips.len() as f64,
                "fx" => t.effects.len() as f64,
                _ => 0.0,
            })
        },
        b"daw_track_set" => {
            let v = n(2);
            if let Some(t) = p.tracks.get_mut(u(0)) {
                match s(1).as_str() {
                    "vol" => t.level = v.clamp(0.0, 2.0) as f32,
                    "pan" => t.pan = v.clamp(-1.0, 1.0) as f32,
                    "mute" => t.muted = v > 0.5,
                    "solo" => t.solo = v > 0.5,
                    "color" => t.color = PALETTE[(v.max(0.0) as usize) % 7],
                    _ => {},
                }
                d.dirty = true;
            d.rev += 1;
            }
            UNIT
        },
        b"daw_palette" => list_from(PALETTE.iter().map(|c| num(*c as f64)).collect()),

        // ── effects (the list only; no DSP yet) ──
        b"daw_fx_count" => num(effects_of(p, n(0) as i64).map(|v| v.len()).unwrap_or(0) as f64),
        b"daw_fx_name" => mk_str(&effects_of(p, n(0) as i64).and_then(|v| v.get(u(1))).map(|f| f.name.clone()).unwrap_or_default()),
        b"daw_fx_wet" => num(effects_of(p, n(0) as i64).and_then(|v| v.get(u(1))).map(|f| f.wet).unwrap_or(1.0) as f64),
        // offline render (WAV export) runs on the host only: never in progress here
        b"daw_render_state" => num(0.0),
        b"daw_render_progress" => num(0.0),
        b"daw_fx_bypassed" => boolean(effects_of(p, n(0) as i64).and_then(|v| v.get(u(1))).map(|f| f.bypass).unwrap_or(false)),
        b"daw_fx_params" => list_from(
            effects_of(p, n(0) as i64)
                .and_then(|v| v.get(u(1)))
                .map(|f| f.params.iter().flat_map(|(k, v)| [mk_str(k), num(*v as f64)]).collect())
                .unwrap_or_default(),
        ),

        // ── clips ──
        b"daw_clip_count" => num(p.tracks.get(u(0)).map(|t| t.clips.len()).unwrap_or(0) as f64),
        b"daw_clip_get" => {
            let Some(c) = p.tracks.get(u(0)).and_then(|t| t.clips.get(u(1))) else { return Some(num(0.0)) };
            num(match s(2).as_str() {
                "start" => c.start,
                "len" => c.len,
                "loop" => c.loop_len,
                "end" => c.end(),
                "audio" => c.is_audio() as i32 as f64,
                "notes" if !c.is_audio() => c.midi_notes.len() as f64,
                "gain" if c.is_audio() => c.gain as f64,
                "pitch" if c.is_audio() => c.pitch as f64,
                "warp" if c.is_audio() => c.warp as f64,
                "offset" if c.is_audio() => c.offset_s,
                _ => 0.0,
            })
        },
        b"daw_clip_set" => {
            let v = n(3);
            if let Some(c) = p.tracks.get_mut(u(0)).and_then(|t| t.clips.get_mut(u(1))) {
                let audio = c.is_audio();
                match s(2).as_str() {
                    "start" => c.start = v.max(0.0),
                    "len" => c.len = v.max(0.25),
                    "loop" => c.loop_len = if v < 0.25 { 0.0 } else { v },
                    "gain" if audio => c.gain = v.clamp(0.0, 4.0) as f32,
                    "pitch" if audio => c.pitch = v.clamp(-24.0, 24.0) as f32,
                    "offset" if audio => c.offset_s = v.max(0.0),
                    _ => {},
                }
                d.dirty = true;
            d.rev += 1;
            }
            UNIT
        },
        b"daw_clip_name" => mk_str(&p.tracks.get(u(0)).and_then(|t| t.clips.get(u(1))).map(|c| c.name.clone()).unwrap_or_default()),
        b"daw_clip_set_name" => {
            if let Some(c) = p.tracks.get_mut(u(0)).and_then(|t| t.clips.get_mut(u(1))) {
                c.name = s(2);
                d.dirty = true;
            d.rev += 1;
            }
            UNIT
        },
        b"daw_clip_notes" => {
            // flat [start,len,key,vel,pan,cutoff,reso] * N
            let Some(c) = p.tracks.get(u(0)).and_then(|t| t.clips.get(u(1))) else { return Some(list_from(Vec::new())) };
            if c.is_audio() {
                return Some(list_from(Vec::new()));
            }
            let mut v = Vec::with_capacity(c.midi_notes.len() * 7);
            for x in &c.midi_notes {
                v.extend([
                    num(x.start),
                    num(x.len),
                    num(x.key as f64),
                    num(x.vel as f64),
                    num(x.pan as f64),
                    num(x.cutoff as f64),
                    num(x.reso as f64),
                ]);
            }
            list_from(v)
        },
        // No samples loaded yet (no storage driver): audio clips draw empty.
        b"daw_clip_peaks" => list_from(Vec::new()),
        b"daw_clip_remove" => {
            if let Some(t) = p.tracks.get_mut(u(0)) {
                if u(1) < t.clips.len() {
                    t.clips.remove(u(1));
                    d.dirty = true;
            d.rev += 1;
                }
            }
            UNIT
        },
        b"daw_clip_dup" => {
            let Some(t) = p.tracks.get_mut(u(0)) else { return Some(num(-1.0)) };
            let Some(mut c) = t.clips.get(u(1)).cloned() else { return Some(num(-1.0)) };
            c.start = n(2).max(0.0);
            t.clips.push(c);
            d.dirty = true;
            d.rev += 1;
            num((t.clips.len() - 1) as f64)
        },
        // ── project meta ──
        b"daw_meta_get" => mk_str(&p.meta.iter().find(|(k, _)| *k == s(0)).map(|(_, v)| v.clone()).unwrap_or_default()),
        b"daw_meta_set" => {
            let (key, val) = (s(0), s(1));
            match p.meta.iter_mut().find(|(k, _)| *k == key) {
                Some(slot) => slot.1 = val,
                None => p.meta.push((key, val)),
            }
            d.dirty = true;
            d.rev += 1;
            UNIT
        },
        // ── notes ──
        b"daw_note_add" => {
            // (t, c, start, len, key, vel) -> idx
            let Some(c) = p.tracks.get_mut(u(0)).and_then(|t| t.clips.get_mut(u(1))) else { return Some(num(-1.0)) };
            if c.is_audio() {
                return Some(num(-1.0));
            }
            c.midi_notes.push(NoteDoc {
                start: n(2).max(0.0),
                len: n(3).max(0.03),
                key: n(4).clamp(0.0, 127.0) as u8,
                vel: n(5).clamp(0.01, 1.0) as f32,
                pan: 0.0,
                cutoff: 0.5,
                reso: 0.5,
            });
            d.dirty = true;
            d.rev += 1;
            num((c.midi_notes.len() - 1) as f64)
        },
        b"daw_note_set" => {
            // (t, c, idx, field, v)
            if let Some(x) = p.tracks.get_mut(u(0)).and_then(|t| t.clips.get_mut(u(1))).and_then(|c| c.midi_notes.get_mut(u(2))) {
                let v = n(4);
                match s(3).as_str() {
                    "start" => x.start = v.max(0.0),
                    "len" => x.len = v.max(0.03),
                    "key" => x.key = v.clamp(0.0, 127.0) as u8,
                    "vel" => x.vel = v.clamp(0.01, 1.0) as f32,
                    "pan" => x.pan = v.clamp(-1.0, 1.0) as f32,
                    "cutoff" => x.cutoff = v.clamp(0.0, 1.0) as f32,
                    "reso" => x.reso = v.clamp(0.0, 1.0) as f32,
                    _ => {},
                }
            }
            d.dirty = true;
            d.rev += 1;
            UNIT
        },
        b"daw_note_remove" => {
            if let Some(c) = p.tracks.get_mut(u(0)).and_then(|t| t.clips.get_mut(u(1))) {
                if u(2) < c.midi_notes.len() {
                    c.midi_notes.remove(u(2));
                }
            }
            d.dirty = true;
            d.rev += 1;
            UNIT
        },
        b"daw_notes_apply" => {
            // (t, c, mode, selected indices, origin [start, key]* by note, origin lengths by note, p1, p2)
            // "move": p1 beats, p2 semitones  "resize": p1 extra beats, p2 shortest  "stretch": p1 anchor, p2 factor
            let (sel, orig, olen) = (nums_arg(args, 3), nums_arg(args, 4), nums_arg(args, 5));
            let (mode, p1, p2) = (s(2), n(6), n(7));
            if p1.is_finite() && p2.is_finite() {
                if let Some(c) = p.tracks.get_mut(u(0)).and_then(|t| t.clips.get_mut(u(1))) {
                    for si in sel {
                        let i = si.max(0.0) as usize;
                        let Some(x) = c.midi_notes.get_mut(i) else { continue };
                        let o_start = orig.get(i * 2).copied();
                        match mode.as_str() {
                            "move" => {
                                if let (Some(os), Some(ok)) = (o_start, orig.get(i * 2 + 1)) {
                                    x.start = (os + p1).max(0.0);
                                    x.key = (ok + p2).clamp(0.0, 127.0) as u8;
                                }
                            },
                            "resize" => {
                                if let Some(ol) = olen.get(i) {
                                    x.len = (ol + p1).max(p2.max(0.03));
                                }
                            },
                            "stretch" => {
                                if let (Some(os), Some(ol)) = (o_start, olen.get(i)) {
                                    x.start = (p1 + (os - p1) * p2).max(0.0);
                                    x.len = (ol * p2).max(0.06);
                                }
                            },
                            _ => {},
                        }
                    }
                }
                d.dirty = true;
                d.rev += 1;
            }
            UNIT
        },
        b"daw_note_lyrics" => {
            // one string per note; the Pi engine keeps no per-note lyrics yet
            let count = p.tracks.get(u(0)).and_then(|t| t.clips.get(u(1))).map(|c| c.midi_notes.len()).unwrap_or(0);
            list_from((0..count).map(|_| mk_str("")).collect())
        },
        // ── instrument parameters (the knobs of the Parameters view) ──
        b"daw_inst_params" => list_from(
            p.tracks.get(u(0)).map(|t| t.params()).unwrap_or_default().iter().flat_map(|(k, v)| [mk_str(k), num(*v as f64)]).collect(),
        ),
        b"daw_inst_get" => num(p.tracks.get(u(0)).and_then(|t| t.param(&s(1))).map(|v| v as f64).unwrap_or(-1.0)),
        b"daw_inst_get_many" => {
            use crate::lingrt::{is_list, list_items};
            let names: Vec<String> = match args.get(1) {
                Some(&v) if is_list(v) => list_items(v).iter().map(|&x| to_string(x)).collect(),
                _ => Vec::new(),
            };
            let t = p.tracks.get(u(0));
            list_from(names.iter().map(|nm| num(t.and_then(|t| t.param(nm)).map(|v| v as f64).unwrap_or(-1.0))).collect())
        },
        b"daw_inst_set" => {
            if let Some(t) = p.tracks.get_mut(u(0)) {
                t.set_param(&s(1), n(2) as f32);
                d.dirty = true;
                d.rev += 1;
            }
            UNIT
        },
        b"daw_inst_parts" | b"daw_inst_presets" | b"daw_inst_categories" | b"daw_inst_plot" => list_from(Vec::new()),
        b"daw_inst_part" => num(0.0),
        b"daw_inst_ab_side" => num(-1.0),
        b"daw_inst_select_part" | b"daw_inst_preset" | b"daw_inst_ab" | b"daw_inst_default" => UNIT,
        b"daw_inst_random" => boolean(false),
        // ── not on the Pi yet: theory analysis, sends / sidechains ──
        b"daw_th_keys" | b"daw_th_chords" | b"daw_th_phrases" | b"daw_th_roles" | b"daw_th_vocab" => list_from(Vec::new()),
        b"daw_route_count" => num(0.0),
        // ── Tracker view ──
        b"daw_trk_channels" => {
            let l = trk_layout(p, d.rev);
            list_from(l.chans.iter().flat_map(|c| [num(c.track as f64), num(c.voice as f64)]).collect())
        },
        b"daw_trk_length" => num(trk_layout(p, d.rev).end),
        b"daw_trk_page" => {
            // (first beat, rows, rows per beat, first channel, channels) -> [key, velocity 0..127, hold]* row by row
            let l = trk_layout(p, d.rev);
            let (beat0, rows, rpb) = (n(0), u(1).min(256), n(2).clamp(1.0, 16.0));
            let (c0, cn) = (u(3), u(4).min(64));
            let idx: Vec<usize> = (0..cn).map(|ci| c0 + ci).collect();
            let cells = trk_page_cells(l, beat0, rows, rpb, &idx);
            list_from(cells.into_iter().flat_map(|(k, v, h)| [num(k as f64), num(libm::round((v * 127.0) as f64).clamp(0.0, 127.0)), num(h as f64)]).collect())
        },
        b"daw_trk_ring" => {
            // (track, first beat, beats, slots) -> loudest note start (0..1) in each slot
            let l = trk_layout(p, d.rev);
            let (t, beat0, beats, slots) = (u(0), n(1), n(2).max(0.25), u(3).clamp(1, 128));
            let mut out = alloc::vec![0f64; slots];
            for ch in l.chans.iter().filter(|c| c.track == t) {
                let i0 = ch.evs.partition_point(|e| e.e <= beat0 - 1e-9);
                for ev in &ch.evs[i0..] {
                    if ev.s >= beat0 + beats {
                        break;
                    }
                    if ev.s >= beat0 - 1e-9 {
                        let k = (((ev.s - beat0) / beats) * slots as f64) as usize;
                        if k < slots {
                            out[k] = out[k].max(ev.vel as f64);
                        }
                    }
                }
            }
            list_from(out.into_iter().map(num).collect())
        },
        b"daw_trk_set" => {
            // (channel, beat, row length, key (-1 clears), velocity 0..127, length in beats (0 = one row)) -> 1 when changed
            let (chi, beat, rl, key) = (u(0), n(1), n(2).max(0.01), n(3) as i64);
            let vel = (n(4) / 127.0).clamp(0.02, 1.0) as f32;
            let len = if n(5) > 0.0 { n(5) } else { rl };
            let (track, kill) = {
                let l = trk_layout(p, d.rev);
                let Some(ch) = l.chans.get(chi) else { return Some(boolean(false)) };
                let mut kill: Vec<(usize, usize)> =
                    ch.evs.iter().filter(|e| e.s >= beat - 1e-6 && e.s < beat + rl - 1e-6 && e.note != usize::MAX).map(|e| (e.clip, e.note)).collect();
                kill.sort_unstable();
                kill.dedup();
                (ch.track, kill)
            };
            let Some(t) = p.tracks.get_mut(track) else { return Some(boolean(false)) };
            if t.is_audio() {
                return Some(boolean(false));
            }
            let mut changed = !kill.is_empty();
            for (ci, ni) in kill.into_iter().rev() {
                if let Some(c) = t.clips.get_mut(ci) {
                    if ni < c.midi_notes.len() {
                        c.midi_notes.remove(ni);
                    }
                }
            }
            if (0..128).contains(&key) {
                let ci = match t.clips.iter().position(|c| !c.is_audio() && c.start <= beat + 1e-9 && beat < c.start + c.len - 1e-9) {
                    Some(i) => i,
                    None => {
                        let start = libm::floor(beat / 4.0) * 4.0;
                        t.clips.push(ClipDoc {
                            name: String::from("Pattern"),
                            start,
                            len: 4.0,
                            midi_notes: Vec::new(),
                            audio_path: String::new(),
                            offset_s: 0.0,
                            gain: 1.0,
                            pitch: 0.0,
                            warp: 0.0,
                            loop_len: 0.0,
                        });
                        t.clips.len() - 1
                    },
                };
                let c = &mut t.clips[ci];
                let mut local = beat - c.start;
                if c.loop_len >= 0.25 && c.loop_len < c.len {
                    local %= c.loop_len;
                }
                c.midi_notes.push(NoteDoc { start: local, len, key: key as u8, vel, pan: 0.0, cutoff: 0.5, reso: 0.5 });
                changed = true;
            }
            if changed {
                d.dirty = true;
                d.rev += 1;
            }
            boolean(changed)
        },
        b"daw_trk_pdf" | b"daw_trk_export" => list_from(Vec::new()),
        // ── Sheet view ──
        b"daw_sheet_scan" => {
            // -> [bars, then (track, name, notes, drums 0/1, has lyrics 0/1)*] for every instrument track with notes
            let mut v = alloc::vec![num(libm::ceil(p.end_beat().max(4.0) / 4.0))];
            for (i, t) in p.tracks.iter().enumerate() {
                if t.is_audio() {
                    continue;
                }
                let count: usize = t.clips.iter().map(|c| c.midi_notes.len()).sum();
                if count == 0 {
                    continue;
                }
                let drums = t.instrument == "drums" || t.instrument == "drum";
                v.extend([num(i as f64), mk_str(&t.name), num(count as f64), num(drums as u8 as f64), num(0.0)]);
            }
            list_from(v)
        },
        b"daw_sheet_png" => {
            // (options, fonts folder, page, page width on screen) -> [pages, 0, 0, 0, page height on screen, generation, ""]:
            // no picture on the Pi (the view draws the page from daw_sheet_geo), the rest as the PC reports it
            let opts = s(0);
            let pw = n(3).max(100.0);
            let sheet = sh_sheet(p, d.rev, &opts);
            let pages = sheet.pages.iter().filter(|pg| !pg.systems.is_empty()).count();
            if pages == 0 {
                list_from(Vec::new())
            } else {
                list_from(alloc::vec![num(pages as f64), num(0.0), num(0.0), num(0.0), num(SH_PH * pw / SH_PW), num(d.rev as f64), mk_str("")])
            }
        },
        b"daw_sheet_geo" => {
            // (options, page) -> the geometry list (see sh_build); [] for no such page
            let opts = s(0);
            let sheet = sh_sheet(p, d.rev, &opts);
            let Some(pg) = sheet.pages.get(u(1)) else { return Some(list_from(Vec::new())) };
            if pg.systems.is_empty() {
                return Some(list_from(Vec::new()));
            }
            let mut v = alloc::vec![num(SH_PW), num(SH_PH), num(pg.systems.len() as f64)];
            for sy in &pg.systems {
                v.extend([num(sy.track as f64), num(sy.b0), num(sy.b1), num(sy.x0), num(sy.bar_w), num(sy.y0), num(sy.y1), num(1.0)]);
                v.extend([num(sy.clef as f64), num(sy.yb), num(SH_STEP)]);
            }
            v.push(num(pg.notes.len() as f64));
            for q in &pg.notes {
                v.extend([num(q.track as f64), num(q.key as f64), num(q.start), num(q.x), num(q.y)]);
            }
            list_from(v)
        },
        b"daw_sheet_edit" => {
            // (track, op, key, start beat, value) -> 1 when something changed. op: 0 delete, 1 transpose by value,
            // 2 move by value beats, 3 set length, 4 add a note of length value at that beat
            let (ti, op, key, start, val) = (u(0), n(1) as i64, n(2).clamp(0.0, 127.0) as u8, n(3).max(0.0), n(4));
            if !start.is_finite() || !val.is_finite() {
                return Some(num(0.0));
            }
            let mut changed = false;
            if let Some(t) = p.tracks.get_mut(ti) {
                if op == 4 {
                    let ci = match t.clips.iter().position(|c| !c.is_audio() && start >= c.start && start < c.start + c.len) {
                        Some(i) => i,
                        None => {
                            let s0 = libm::floor(start / 4.0) * 4.0;
                            t.clips.push(ClipDoc {
                                name: String::from("Pattern"),
                                start: s0,
                                len: 4.0,
                                midi_notes: Vec::new(),
                                audio_path: String::new(),
                                offset_s: 0.0,
                                gain: 1.0,
                                pitch: 0.0,
                                warp: 0.0,
                                loop_len: 0.0,
                            });
                            t.clips.len() - 1
                        },
                    };
                    let c = &mut t.clips[ci];
                    let rel = start - c.start;
                    let len = val.max(0.0625);
                    if rel + len > c.len {
                        c.len = libm::ceil((rel + len) / 4.0) * 4.0;
                    }
                    c.midi_notes.push(NoteDoc { start: rel, len, key, vel: 0.8, pan: 0.0, cutoff: 0.5, reso: 0.5 });
                    changed = true;
                } else {
                    'find: for c in t.clips.iter_mut() {
                        if c.is_audio() {
                            continue;
                        }
                        let (cs, clen) = (c.start, c.len);
                        let ll = if c.loop_len >= 0.25 && c.loop_len < c.len { c.loop_len } else { 0.0 };
                        let reps = if ll > 0.0 { (libm::ceil(clen / ll) as usize).min(512) } else { 1 };
                        for k in 0..reps {
                            let base = cs + k as f64 * ll;
                            let rel = start - base;
                            for i in 0..c.midi_notes.len() {
                                let nn = &c.midi_notes[i];
                                if nn.key == key && rel >= nn.start - 0.01 && rel < nn.start + nn.len.max(0.05) + 0.01 {
                                    match op {
                                        0 => {
                                            c.midi_notes.remove(i);
                                        },
                                        1 => c.midi_notes[i].key = (nn.key as f64 + libm::round(val)).clamp(0.0, 127.0) as u8,
                                        2 => c.midi_notes[i].start = (nn.start + val).max(0.0),
                                        3 => c.midi_notes[i].len = val.max(0.0625),
                                        _ => break 'find,
                                    }
                                    changed = true;
                                    break 'find;
                                }
                            }
                        }
                    }
                }
            }
            if changed {
                d.dirty = true;
                d.rev += 1;
            }
            num(changed as u8 as f64)
        },
        b"daw_sheet_write" | b"daw_sheet_xml" => list_from(Vec::new()),
        b"daw_sheet_musescore_find" | b"daw_sheet_musescore_open" => mk_str(""),
        // ── Performance view: no scenes on the Pi yet ──
        b"daw_perf_scenes" | b"daw_perf_state" => list_from(Vec::new()),
        b"daw_perf_only" => num(-1.0),
        b"daw_perf_quant" => num(4.0),
        b"daw_perf_scene_name" => mk_str(""),
        b"daw_perf_launch" | b"daw_perf_launch_scene" | b"daw_perf_stop_all" | b"daw_perf_stop_track" | b"daw_perf_set_only" | b"daw_perf_set_quant" | b"daw_perf_scene_add" | b"daw_perf_scene_remove" | b"daw_perf_scene_set_name" | b"daw_perf_slot_clear" | b"daw_perf_slot_copy" | b"daw_perf_slot_from_clip" | b"daw_perf_slot_new" => UNIT,
        b"daw_perf_slot_info" => list_from(Vec::new()),
        _ => return None,
    })
}

// ── step 2: sound ────────────────────────────────────────────────────────────
// A polyphonic synth + note scheduler rendering the loaded project into the
// kernel mixer (x86: AC97 via `drivers::mixer`, which pulls `block`). It
// follows the project's own clock: notes of every audible track (mute/solo,
// clip loops) are scheduled sample-accurately per block, synth tracks play
// detuned saws + a square sub through a resonant low-pass with an envelope,
// drum tracks a synthesized General MIDI kit. Track level/pan, note velocity/
// pan/cutoff/resonance, master level and the limiter are honoured. It is not
// ling-daw's instrument/effect DSP (Prism, the sampler, reverb...) — a stand-in
// so a project plays recognisably until those are ported.

const SR: f64 = 48_000.0;
const MAX_VOICES: usize = 64;

#[derive(Clone, Copy, PartialEq)]
enum VKind {
    Synth,
    Kick,
    Snare,
    Hat,
    OpenHat,
    Tom,
    Cymbal,
    Click,
}

#[derive(Clone, Copy)]
struct Voice {
    track: usize,
    kind: VKind,
    freq: f64,
    vel: f64,
    pan: f64,
    cutoff: f64,
    reso: f64,
    /// seconds since note-on / since release
    t: f64,
    rel_t: f64,
    gate: bool,
    off_beat: f64,
    phase: [f64; 3],
    lp: [f64; 2],
    noise: u32,
    env: f64,
}

struct Audio {
    running: bool,
    beat: f64,
    voices: Vec<Voice>,
    buf: Vec<i16>,
}

static mut AUDIO: Audio = Audio { running: false, beat: 0.0, voices: Vec::new(), buf: Vec::new() };

fn audio() -> &'static mut Audio {
    unsafe { &mut *(&raw mut AUDIO) }
}

/// Whether the engine is producing sound (the transport runs or notes ring out).
pub fn engine_running() -> bool {
    let a = unsafe { &*(&raw const AUDIO) };
    a.running || !a.voices.is_empty()
}

/// Pause the transport (the desktop does this when LingStationX is suspended
/// or closed, if the "pause music" setting is on). Nothing if no project is up.
pub fn pause_transport() {
    if let Some(d) = unsafe { (*(&raw mut DAW)).as_mut() } {
        if d.playing {
            audio_stop();
            d.playing = false;
        }
    }
}

fn audio_start(beat: f64) {
    let a = audio();
    a.voices.clear();
    a.beat = beat;
    a.running = true;
}

fn audio_stop() {
    let a = audio();
    a.running = false;
    for v in a.voices.iter_mut() {
        v.gate = false;
    }
}

fn drum_kind(key: u8) -> (VKind, f64) {
    match key {
        35 | 36 => (VKind::Kick, 55.0),
        37 | 39 => (VKind::Click, 1800.0),
        38 | 40 => (VKind::Snare, 190.0),
        42 | 44 => (VKind::Hat, 0.0),
        46 => (VKind::OpenHat, 0.0),
        41 | 43 | 45 | 47 | 48 | 50 => (VKind::Tom, 70.0 + (key as f64 - 41.0) * 14.0),
        49 | 51 | 52 | 53 | 55 | 57 | 59 => (VKind::Cymbal, 0.0),
        _ => (VKind::Click, 900.0),
    }
}

fn note_on(a: &mut Audio, track: usize, drums: bool, n: &NoteDoc, off_beat: f64) {
    let (kind, freq) = if drums {
        drum_kind(n.key)
    } else {
        (VKind::Synth, 440.0 * libm::pow(2.0, (n.key as f64 - 69.0) / 12.0))
    };
    let v = Voice {
        track,
        kind,
        freq,
        vel: (n.vel as f64).clamp(0.0, 1.0),
        pan: (n.pan as f64).clamp(-1.0, 1.0),
        cutoff: (n.cutoff as f64).clamp(0.0, 1.0),
        reso: (n.reso as f64).clamp(0.0, 1.0),
        t: 0.0,
        rel_t: 0.0,
        gate: true,
        off_beat,
        phase: [0.0; 3],
        lp: [0.0; 2],
        noise: 0x1234_5678 ^ (n.key as u32 * 2654435761),
        env: 0.0,
    };
    if a.voices.len() >= MAX_VOICES {
        // steal the oldest released voice, else the oldest
        let i = a.voices.iter().position(|v| !v.gate).unwrap_or(0);
        a.voices.remove(i);
    }
    a.voices.push(v);
}

#[inline]
fn noise(s: &mut u32) -> f64 {
    let mut x = *s;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *s = x;
    (x as f64 / u32::MAX as f64) * 2.0 - 1.0
}

/// One sample of a voice; false once it has died away.
fn voice_sample(v: &mut Voice, dt: f64) -> Option<f64> {
    v.t += dt;
    if !v.gate {
        v.rel_t += dt;
    }
    let s = match v.kind {
        VKind::Synth => {
            // ADSR: 8ms attack, decay to 0.7, 250ms release
            let target = if v.gate {
                if v.t < 0.008 { v.t / 0.008 } else { 0.7 + 0.3 * libm::exp(-(v.t - 0.008) * 6.0) }
            } else {
                v.env * libm::exp(-dt / 0.08)
            };
            v.env = target;
            if !v.gate && v.env < 0.0005 {
                return None;
            }
            let mut osc = 0.0;
            for (i, det) in [1.0f64, 1.0058, 0.5].iter().enumerate() {
                v.phase[i] += v.freq * det * dt;
                v.phase[i] -= libm::floor(v.phase[i]);
                osc += if i < 2 { 2.0 * v.phase[i] - 1.0 } else if v.phase[i] < 0.5 { 0.6 } else { -0.6 };
            }
            osc *= 0.33;
            // two-pole low-pass: cutoff follows the note's cutoff + envelope
            let fc = (300.0 + 6000.0 * v.cutoff * v.cutoff + 2500.0 * v.env).min(16_000.0);
            let g = 1.0 - libm::exp(-2.0 * core::f64::consts::PI * fc * dt);
            let fb = v.reso * 0.6;
            v.lp[0] += g * (osc - v.lp[0] - fb * (v.lp[1] - v.lp[0]));
            v.lp[1] += g * (v.lp[0] - v.lp[1]);
            v.lp[1] * v.env
        },
        VKind::Kick => {
            let f = v.freq + 110.0 * libm::exp(-v.t * 30.0);
            v.phase[0] += f * dt;
            let e = libm::exp(-v.t * 9.0);
            if e < 0.0005 {
                return None;
            }
            libm::sin(2.0 * core::f64::consts::PI * v.phase[0]) * e * 1.2
        },
        VKind::Snare => {
            v.phase[0] += v.freq * dt;
            let tone = libm::sin(2.0 * core::f64::consts::PI * v.phase[0]) * libm::exp(-v.t * 25.0);
            let nz = noise(&mut v.noise) * libm::exp(-v.t * 14.0);
            if v.t > 0.5 {
                return None;
            }
            (tone * 0.5 + nz * 0.7) * 0.8
        },
        VKind::Hat | VKind::OpenHat | VKind::Cymbal => {
            let decay = match v.kind {
                VKind::Hat => 45.0,
                VKind::OpenHat => 9.0,
                _ => 3.5,
            };
            let e = libm::exp(-v.t * decay);
            if e < 0.0005 {
                return None;
            }
            // high-passed noise
            let nz = noise(&mut v.noise);
            let hp = nz - v.lp[0];
            v.lp[0] += 0.35 * (nz - v.lp[0]);
            hp * e * 0.45
        },
        VKind::Tom => {
            let f = v.freq * (1.0 + 0.5 * libm::exp(-v.t * 20.0));
            v.phase[0] += f * dt;
            let e = libm::exp(-v.t * 7.0);
            if e < 0.0005 {
                return None;
            }
            libm::sin(2.0 * core::f64::consts::PI * v.phase[0]) * e * 0.9
        },
        VKind::Click => {
            v.phase[0] += v.freq * dt;
            let e = libm::exp(-v.t * 40.0);
            if e < 0.0005 {
                return None;
            }
            (libm::sin(2.0 * core::f64::consts::PI * v.phase[0]) * 0.5 + noise(&mut v.noise) * 0.3) * e
        },
    };
    Some(s * (0.35 + 0.65 * v.vel))
}

/// Schedule every note starting in [b0, b1) on audible tracks.
fn schedule(a: &mut Audio, p: &ProjectDoc, b0: f64, b1: f64) {
    let any_solo = p.tracks.iter().any(|t| t.solo);
    for (ti, t) in p.tracks.iter().enumerate() {
        if t.is_audio() || t.muted || (any_solo && !t.solo) {
            continue;
        }
        let drums = t.instrument == "drums" || t.instrument == "drum";
        for c in &t.clips {
            if c.is_audio() || c.end() <= b0 || c.start >= b1 {
                continue;
            }
            let span = if c.loop_len > 0.0 { c.loop_len } else { c.len };
            let reps = if c.loop_len > 0.0 { libm::ceil(c.len / c.loop_len).max(1.0) as usize } else { 1 };
            for k in 0..reps {
                let off = c.start + k as f64 * span;
                if off >= b1 || off + span <= b0 {
                    continue;
                }
                for n in &c.midi_notes {
                    if n.start >= span {
                        continue;
                    }
                    let on = off + n.start;
                    if on >= c.end() || on < b0 || on >= b1 {
                        continue;
                    }
                    let off_beat = (on + n.len.max(0.02)).min(c.end());
                    note_on(a, ti, drums, n, off_beat);
                }
            }
        }
    }
}

/// Render `frames` stereo frames of the playing project, or None when silent
/// (nothing playing and every voice has died away). Called by the mixer.
pub fn block(frames: usize) -> Option<&'static [i16]> {
    let a = audio();
    if !a.running && a.voices.is_empty() {
        return None;
    }
    let d = unsafe { (*(&raw mut DAW)).as_ref() }?;
    let p = &d.project;
    if a.buf.len() < frames * 2 {
        a.buf.resize(frames * 2, 0);
    }
    let dt = 1.0 / SR;
    let master = p.master_level as f64;
    // Gains per track (level + constant-power pan).
    let gains: Vec<(f64, f64, f64)> = p
        .tracks
        .iter()
        .map(|t| {
            let pan = (t.pan as f64).clamp(-1.0, 1.0);
            (t.level as f64, pan, 0.0)
        })
        .collect();
    let mut f = 0usize;
    while f < frames {
        // Sub-blocks of 64 frames: schedule notes, then render.
        let n = (frames - f).min(64);
        if a.running {
            let bpm = p.bpm_at(a.beat);
            let b0 = a.beat;
            let mut b1 = b0 + n as f64 * dt * bpm / 60.0;
            if p.loop_on && p.loop_end > p.loop_start && b1 >= p.loop_end && b0 < p.loop_end {
                schedule(a, p, b0, p.loop_end);
                for v in a.voices.iter_mut() {
                    v.gate = false;
                }
                b1 = p.loop_start + (b1 - p.loop_end);
                schedule(a, p, p.loop_start, b1);
            } else {
                schedule(a, p, b0, b1);
            }
            a.beat = b1;
            for v in a.voices.iter_mut() {
                if v.gate && a.beat >= v.off_beat {
                    v.gate = false;
                }
            }
        }
        for i in 0..n {
            let (mut l, mut r) = (0.0f64, 0.0f64);
            a.voices.retain_mut(|v| match voice_sample(v, dt) {
                Some(s) => {
                    let (lvl, tpan, _) = gains.get(v.track).copied().unwrap_or((1.0, 0.0, 0.0));
                    let pan = (tpan + v.pan).clamp(-1.0, 1.0);
                    let x = (pan + 1.0) * core::f64::consts::FRAC_PI_4;
                    l += s * lvl * libm::cos(x);
                    r += s * lvl * libm::sin(x);
                    true
                },
                None => false,
            });
            // master + soft limiter
            let sl = libm::tanh(l * master * 0.35);
            let sr = libm::tanh(r * master * 0.35);
            a.buf[(f + i) * 2] = (sl * 32000.0) as i16;
            a.buf[(f + i) * 2 + 1] = (sr * 32000.0) as i16;
        }
        f += n;
    }
    Some(&a.buf[..frames * 2])
}
