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
use serde::Deserialize;

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

#[derive(Deserialize, Clone, Default)]
struct FxDoc {
    #[serde(default)]
    name: String,
    #[serde(default)]
    params: Vec<(String, f32)>,
    #[serde(default)]
    bypass: bool,
}

#[derive(Deserialize, Clone)]
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

#[derive(Deserialize, Clone)]
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

#[derive(Deserialize, Clone)]
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
}

impl TrackDoc {
    fn is_audio(&self) -> bool {
        self.kind == "audio"
    }
}

#[derive(Deserialize, Clone)]
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
    let Some(bytes) = crate::appfs::get(path) else {
        return Err(alloc::format!("{path}: not found"));
    };
    let doc: ProjectDoc = serde_json::from_slice(bytes).map_err(|e| alloc::format!("{path}: {e}"))?;
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
        b"daw_project_save" | b"daw_project_save_copy" | b"daw_project_save_as" => {
            d.status = String::from("Saving needs storage -- not available on LingOS yet");
            boolean(false)
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
