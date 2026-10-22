//! Script-facing API: a single name-based dispatcher used by the `daw_*`, `fs_*`
//! and `dlg_*` builtins of the Ling runtime. Everything is numbers, strings and
//! flat lists so it maps straight onto Ling values.
use crate::audio::{self, AudioOut};
use crate::dsp::fx::EFFECT_NAMES;
use crate::engine::Engine;
use crate::model::*;
use crate::patch;
use crate::project;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Num(f64),
    Str(String),
    List(Vec<Val>),
    Unit,
}
impl From<f64> for Val {
    fn from(x: f64) -> Val {
        Val::Num(x)
    }
}
impl From<f32> for Val {
    fn from(x: f32) -> Val {
        Val::Num(x as f64)
    }
}
impl From<bool> for Val {
    fn from(x: bool) -> Val {
        Val::Num(if x { 1.0 } else { 0.0 })
    }
}
impl From<usize> for Val {
    fn from(x: usize) -> Val {
        Val::Num(x as f64)
    }
}
impl From<&str> for Val {
    fn from(x: &str) -> Val {
        Val::Str(x.to_string())
    }
}
impl From<String> for Val {
    fn from(x: String) -> Val {
        Val::Str(x)
    }
}

pub struct Daw {
    out: AudioOut,
    undo: Vec<String>,
    redo: Vec<String>,
    last_snap: String,
    pub path: String,
    samples: HashMap<String, Arc<Sample>>,
    peaks: HashMap<(usize, usize), Vec<f32>>,
    dirty: bool,
    status: String,
    midi: Option<crate::midi_io::MidiIn>,
    midi_index: Option<usize>,
    rev: u64,
    clipboard: Vec<(usize, Clip)>,
    arec: Option<AudioRec>,
    render: Option<RenderJob>,
    /// A/B compare slots per track: (A, B, current side)
    ab: HashMap<usize, (Vec<(String, f32)>, Option<Vec<(String, f32)>>, u8)>,
    midi_target: Arc<AtomicUsize>,
    midi_chan: Arc<std::sync::atomic::AtomicU8>,
    audio_cfg: audio::AudioConfig,
    in_pref: String,
    /// input gain applied to recordings (dB) and whether quiet takes are normalised when they stop
    in_gain_db: f32,
    in_norm: bool,
    /// soundfont used by MIDI import when set
    gm_sf2: String,
    /// last folder used by a file dialog
    dlg_dir: String,
    th_cache: Option<(u64, usize, usize, Option<Arc<theory_api::Analysis>>)>,
    /// the last score page drawn for the Sheet tab: (cache key, [pages, bars, tracks, width, height, generation, png path])
    sheet_png: Vec<(String, Vec<Val>)>,
    sheet_gen: u64,
    /// the sheet last built (key: revision + options), shared by the page pictures and the geometry of the Sheet tab
    sheet_cache: Option<(String, Arc<crate::sheet::Sheet>)>,
    style_rev: u64,
    fix_last: Vec<Val>,
    artic: Option<artic_api::Job>,
    key_cache: HashMap<String, keytune_api::AudioKey>,
    trk_cache: Option<(u64, Arc<tracker_api::Layout>)>,
    mix: mix_api::MixState,
    kar_job: Option<karaoke_api::VideoJob>,
    smp_index: Option<(String, Vec<(String, String)>)>,
    smp_roots: String,
}

/// An offline render running on its own thread against a private copy of the project.
struct RenderJob {
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
    /// None while running; Some(Ok(path)) / Some(Err(message)) when finished
    result: Arc<std::sync::Mutex<Option<Result<String, String>>>>,
    started: std::time::Instant,
}

struct AudioRec {
    cap: crate::audio_in::Capture,
    start: f64,
    track: usize,
}

mod artic_api;
mod duck_api;
mod karaoke_api;
mod sheet_api;
mod smp_api;
mod tracker_api;
mod tracker_pdf;
mod keytune_api;
mod lyricfit_api;
mod fix_api;
mod vox_api;
mod mix_api;
mod scriptdsp_api;
mod vocmidi_api;
mod vedit_api;
mod rvc_api;
mod song_api;

/// The patch file a role table's (plugin, name) means, for the sound catalogue.
pub fn song_find_patch(kind: &str, name: &str) -> Option<String> {
    song_api::find_patch(kind, name)
}
mod theory_api;

struct A<'a>(&'a [Val]);
impl A<'_> {
    fn n(&self, i: usize) -> f64 {
        match self.0.get(i) {
            Some(Val::Num(x)) if x.is_finite() => *x,
            Some(Val::Str(s)) => s.parse::<f64>().ok().filter(|x| x.is_finite()).unwrap_or(0.0),
            _ => 0.0,
        }
    }
    fn u(&self, i: usize) -> usize {
        self.n(i).max(0.0) as usize
    }
    fn s(&self, i: usize) -> String {
        match self.0.get(i) {
            Some(Val::Str(s)) => s.clone(),
            Some(Val::Num(x)) => format!("{x}"),
            _ => String::new(),
        }
    }
}

/// Every new or imported track starts with a gentle compressor (2.5:1, a little makeup) so levels sit together; the master gets a slower glue compressor
fn default_comp(t: &mut Track, sr: f32) {
    if t.effects.iter().any(|f| f.name == "Compressor") {
        return;
    }
    if let Some(mut c) = EffectSlot::new("Compressor", sr) {
        for (n, v) in [("threshold", 0.767), ("ratio", 0.079), ("attack", 0.06), ("release", 0.13), ("makeup", 0.5417)] {
            if let Some(i) = c.params.iter().position(|(p, _)| *p == n) {
                c.set(i, v);
            }
        }
        t.effects.insert(0, c);
    }
}

pub(crate) fn ensure_master_comp(e: &mut Engine, sr: f32) {
    if e.project.master_effects.iter().any(|f| f.name == "Compressor") {
        return;
    }
    if let Some(mut c) = EffectSlot::new("Compressor", sr) {
        for (n, v) in [("threshold", 0.833), ("ratio", 0.0526), ("attack", 0.146), ("release", 0.192), ("makeup", 0.5417)] {
            if let Some(i) = c.params.iter().position(|(p, _)| *p == n) {
                c.set(i, v);
            }
        }
        e.project.master_effects.insert(0, c);
    }
}

const PALETTE: [u32; 7] = [0xED4A37, 0xF7941E, 0xF7D846, 0x45C870, 0x3BAAEB, 0x4A64D8, 0x9A53D6];

/// the key a percussion part gets on the shared kit: its General MIDI drum key when the name says what it is, else the next free key from middle C
fn kit_key(name: &str, used: &mut Vec<u8>) -> u8 {
    let n = name.to_lowercase();
    let table: [(&[&str], u8); 18] = [
        (&["bass drum", "kick", "bassdrum"], 36),
        (&["rim"], 37),
        (&["snare"], 38),
        (&["clap"], 39),
        (&["low floor tom", "floor tom"], 41),
        (&["closed hi", "closed hat", "hi-hat", "hihat", "hi hat"], 42),
        (&["low tom"], 45),
        (&["open hi", "open hat"], 46),
        (&["mid tom"], 47),
        (&["crash", "chinese"], 49),
        (&["high tom", "tom"], 50),
        (&["ride"], 51),
        (&["tambourine"], 54),
        (&["splash"], 55),
        (&["cowbell"], 56),
        (&["wood block", "woodblock"], 76),
        (&["claves"], 75),
        (&["shaker", "cabasa"], 69),
    ];
    for (words, key) in table {
        if words.iter().any(|w| n.contains(w)) && !used.contains(&key) {
            used.push(key);
            return key;
        }
    }
    let mut k = 60u8;
    while used.contains(&k) && k < 127 {
        k += 1;
    }
    used.push(k);
    k
}

fn fx_target(e: &mut Engine, t: i64) -> Option<&mut Vec<EffectSlot>> {
    if t < 0 {
        Some(&mut e.project.master_effects)
    } else {
        e.project.tracks.get_mut(t as usize).map(|x| &mut x.effects)
    }
}

impl Daw {
    pub fn new() -> Daw {
        install_panic_log();
        let audio_cfg = audio::AudioConfig::default();
        let out = audio::start(None, &audio_cfg);
        Daw { out, undo: vec![], redo: vec![], last_snap: String::new(), path: String::new(), samples: HashMap::new(), peaks: HashMap::new(), dirty: false, status: String::new(), midi: None, midi_index: None, rev: 0, clipboard: vec![], arec: None, render: None, ab: HashMap::new(), midi_target: Arc::new(AtomicUsize::new(0)), midi_chan: Arc::new(std::sync::atomic::AtomicU8::new(0)), audio_cfg, in_pref: String::new(), in_gain_db: 0.0, in_norm: true, gm_sf2: String::new(), dlg_dir: String::new(), th_cache: None, sheet_png: vec![], sheet_gen: 0, sheet_cache: None, style_rev: u64::MAX, fix_last: vec![], artic: None, key_cache: HashMap::new(), trk_cache: None, mix: Default::default(), kar_job: None, smp_index: None, smp_roots: String::new() }
    }

    fn touch(&mut self) {
        self.dirty = true;
        self.rev += 1;
    }

    /// The shared engine (for tests and tools).
    pub fn engine(&self) -> crate::engine::SharedEngine {
        self.out.engine.clone()
    }

    fn snap(&self) -> String {
        let e = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
        serde_json::to_string(&project::to_doc(&e)).unwrap_or_default()
    }

    fn restore(&mut self, s: &str) -> bool {
        let Ok(doc) = serde_json::from_str::<project::ProjectDoc>(s) else { return false };
        let base = std::path::Path::new(&self.path).parent().map(|p| p.to_path_buf());
        project::preload(&doc, base.as_deref(), &mut self.samples);
        let mut e = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
        let (pos, playing) = (e.position, e.playing);
        project::apply_doc(&mut e, &doc, base.as_deref(), &mut self.samples);
        e.position = pos;
        e.playing = playing;
        true
    }

    fn load_sample_cached(&mut self, path: &str) -> Result<Arc<Sample>, String> {
        if let Some(s) = self.samples.get(path) {
            return Ok(s.clone());
        }
        let s = load_sample(path)?;
        self.samples.insert(path.to_string(), s.clone());
        Ok(s)
    }

    /// Dispatch a builtin by name. `None` = not one of ours.
    pub fn call(&mut self, name: &str, args: &[Val]) -> Option<Result<Val, String>> {
        if !(name.starts_with("daw_") || name.starts_with("fs_") || name.starts_with("dlg_")) {
            return None;
        }
        Some(self.dispatch(name, &A(args)))
    }

    fn dispatch(&mut self, name: &str, a: &A) -> Result<Val, String> {
        if name.starts_with("daw_vox_") {
            return self.vox_dispatch(name, a);
        }
        if name.starts_with("daw_mix_") {
            return self.mix_dispatch(name, a);
        }
        if name.starts_with("daw_script_") {
            return self.scriptdsp_dispatch(name, a);
        }
        if name.starts_with("daw_song_") {
            return self.song_dispatch(name, a);
        }
        if name.starts_with("daw_rvc_") {
            return self.rvc_dispatch(name, a);
        }
        if name.starts_with("daw_ve_") {
            return self.ve_dispatch(name, a);
        }
        if name.starts_with("daw_vm_") {
            return self.vm_dispatch(name, a);
        }
        if name.starts_with("daw_smp_") {
            return self.smp_dispatch(name, a);
        }
        if name.starts_with("daw_sheet_") {
            return self.sheet_dispatch(name, a);
        }
        if name.starts_with("daw_kar_") {
            return self.kar_dispatch(name, a);
        }
        if name.starts_with("daw_trk_") {
            return self.trk_dispatch(name, a);
        }
        if name.starts_with("daw_duck_") {
            return self.duck_dispatch(name, a);
        }
        if name.starts_with("daw_key_") {
            return self.key_dispatch(name, a);
        }
        if name.starts_with("daw_lyrf_") {
            return self.lyrf_dispatch(name, a);
        }
        if name.starts_with("daw_artic_") {
            return self.artic_dispatch(name, a);
        }
        if name.starts_with("daw_fix_") || name.starts_with("daw_arr_") {
            return self.fix_dispatch(name, a);
        }
        if name.starts_with("daw_th_") || name.starts_with("daw_style_") || name.starts_with("daw_lib_") || name.starts_with("daw_roll_") || name.starts_with("daw_derive_") {
            return self.theory_dispatch(name, a);
        }
        let eng = self.out.engine.clone();
        let sr = self.out.sr;
        macro_rules! e {
            () => {
                eng.lock().unwrap_or_else(|p| p.into_inner())
            };
        }
        macro_rules! ok {
            ($v:expr) => {
                return Ok(Val::from($v))
            };
        }
        match name {
            // ── system ──
            "daw_sample_rate" => ok!(sr),
            "daw_device_rate" => ok!(self.out.device_sr),
            "daw_device" => ok!(self.out.device.clone()),
            "daw_devices" => ok!(Val::List(audio::device_names().into_iter().map(Val::Str).collect())),
            "daw_status" => ok!(std::mem::take(&mut self.status)),
            "daw_dirty" => ok!(self.dirty),
            "daw_set_dirty" => {
                self.dirty = a.n(0) > 0.5;
                Ok(Val::Unit)
            }
            "daw_new" => {
                self.ab.clear();
                let mut en = e!();
                project::apply_doc(
                    &mut en,
                    &project::ProjectDoc::empty("Untitled", 120.0),
                    None,
                    &mut self.samples,
                );
                ensure_master_comp(&mut en, sr);
                drop(en);
                self.undo.clear();
                self.redo.clear();
                self.path.clear();
                self.dirty = false;
                self.last_snap = self.snap();
                Ok(Val::Unit)
            }
            "daw_set_project_path" => {
                self.path = a.s(0);
                Ok(Val::Unit)
            }
            "daw_project_save_copy" => {
                self.refresh_style();
                let en = e!();
                ok!(project::save(&en, &a.s(0)).is_ok())
            }
            "daw_meta_get" => {
                let en = e!();
                ok!(en.project.meta.iter().find(|(k, _)| *k == a.s(0)).map(|(_, v)| v.clone()).unwrap_or_default())
            }
            "daw_meta_set" => {
                let mut en = e!();
                let key = a.s(0);
                let val = a.s(1);
                if let Some(slot) = en.project.meta.iter_mut().find(|(k, _)| *k == key) {
                    slot.1 = val;
                } else {
                    en.project.meta.push((key, val));
                }
                self.touch();
                Ok(Val::Unit)
            }
            "daw_midi_export" => {
                let en = e!();
                let r = crate::midi_io::export_track(&en, a.u(0), &a.s(1));
                drop(en);
                if let Err(x) = &r {
                    self.status = format!("MIDI export failed: {x}");
                }
                ok!(r.is_ok())
            }
            "daw_midi_export_project" => {
                let en = e!();
                let r = crate::midi_io::export_project(&en, &a.s(0));
                drop(en);
                if let Err(x) = &r {
                    self.status = format!("MIDI export failed: {x}");
                }
                ok!(r.is_ok())
            }
            "daw_midi_inputs" => ok!(Val::List(crate::midi_io::list_inputs().into_iter().map(Val::Str).collect())),
            "daw_midi_open" => match crate::midi_io::open_input(a.u(0), eng.clone(), self.midi_target.clone(), self.midi_chan.clone()) {
                Ok(m) => {
                    self.midi_index = Some(a.u(0));
                    self.status = format!("MIDI input: {}", m.name);
                    self.midi = Some(m);
                    ok!(true)
                }
                Err(x) => {
                    self.status = format!("MIDI input failed: {x}");
                    Ok(Val::Num(0.0))
                }
            },
            "daw_midi_close" => {
                self.midi = None;
                self.midi_index = None;
                self.status = "MIDI input closed".into();
                Ok(Val::Unit)
            }
            "daw_midi_channel" => {
                // (0 = all channels, 1..16)
                self.midi_chan.store(a.n(0).clamp(0.0, 16.0) as u8, Ordering::Relaxed);
                Ok(Val::Unit)
            }
            "daw_midi_channel_get" => ok!(self.midi_chan.load(Ordering::Relaxed) as f64),
            "daw_audio_config" => {
                // (buffer frames or 0, safety buffer ms, engine rate: 0 auto / 1 device / Hz) -> ok
                let old = self.audio_cfg;
                self.audio_cfg = audio::AudioConfig { buffer: a.n(0).max(0.0) as u32, latency_ms: (a.n(1).max(0.0) as u32).clamp(0, 500), rate: a.n(2).max(0.0) as u32 };
                let dev = self.out.device.clone();
                let r = self.dispatch("daw_set_device", &A(&[Val::Str(dev)]));
                if !matches!(r, Ok(Val::Num(1.0))) {
                    self.audio_cfg = old;
                }
                r
            }
            "daw_audio_cfg_get" => ok!(match a.s(0).as_str() {
                "buffer" => self.audio_cfg.buffer as f64,
                "latency" => self.audio_cfg.latency_ms as f64,
                "rate" => self.audio_cfg.rate as f64,
                _ => 0.0,
            }),
            "daw_audio_stats" => {
                let st = &self.out.stats;
                ok!(match a.s(0).as_str() {
                    "underruns" => st.underruns.load(Ordering::Relaxed) as f64,
                    "load" => st.load_permille.load(Ordering::Relaxed) as f64 / 10.0,
                    "buffer" => st.buffer_frames.load(Ordering::Relaxed) as f64,
                    "latency" => self.out.latency_ms() as f64,
                    _ => 0.0,
                })
            }
            "daw_audio_stats_reset" => {
                self.out.stats.underruns.store(0, Ordering::Relaxed);
                Ok(Val::Unit)
            }
            "daw_audio_input_set" => {
                self.in_pref = a.s(0);
                Ok(Val::Unit)
            }
            "daw_audio_input_get" => ok!(self.in_pref.clone()),
            "daw_audio_gain_set" => {
                self.in_gain_db = a.n(0).clamp(-24.0, 60.0) as f32;
                Ok(Val::Unit)
            }
            "daw_audio_gain_get" => ok!(self.in_gain_db as f64),
            "daw_audio_norm_set" => {
                self.in_norm = a.n(0) > 0.5;
                Ok(Val::Unit)
            }
            "daw_audio_norm_get" => ok!(self.in_norm),
            "daw_midi_current" => ok!(self.midi.as_ref().map(|m| m.name.clone()).unwrap_or_default()),
            "daw_midi_target" => {
                self.midi_target.store(a.u(0), Ordering::Relaxed);
                Ok(Val::Unit)
            }
            "daw_rec_arm" => {
                let mut en = e!();
                let t = a.n(0);
                let was = en.rec_arm;
                en.rec_latency_s = self.out.latency_ms() as f64 / 1000.0;
                en.rec_arm = if t >= 0.0 { Some(t as usize) } else { None };
                if en.rec_arm.is_none() && was.is_some() {
                    en.finish_recording();
                }
                Ok(Val::Unit)
            }
            "daw_set_device" => {
                let doc = {
                    let en = e!();
                    project::to_doc(&en)
                };
                let (rec_arm, auto_rec, perf_rec) = {
                    let en = e!();
                    (en.rec_arm, en.auto_rec, en.perf_rec)
                };
                let base = std::path::Path::new(&self.path).parent().map(|p| p.to_path_buf());
                project::preload(&doc, base.as_deref(), &mut self.samples);
                let new_out = audio::start(Some(&a.s(0)), &self.audio_cfg);
                if !new_out.is_running() {
                    self.status = format!("Could not open audio device: {}", a.s(0));
                    return Ok(Val::Num(0.0));
                }
                {
                    let mut en = new_out.engine.lock().unwrap_or_else(|p| p.into_inner());
                    project::apply_doc(&mut en, &doc, base.as_deref(), &mut self.samples);
                    en.rec_arm = rec_arm;
                    en.auto_rec = auto_rec;
                    en.perf_rec = perf_rec;
                }
                self.out = new_out;
                if let Some(idx) = self.midi_index {
                    if let Ok(m) = crate::midi_io::open_input(idx, self.out.engine.clone(), self.midi_target.clone(), self.midi_chan.clone()) {
                        self.midi = Some(m);
                    }
                }
                self.status = format!("Audio device: {}", self.out.device);
                ok!(true)
            }
            // ── routing (Node Editor) ──
            "daw_route_count" => ok!(e!().project.routes.len()),
            "daw_route_add" => {
                // (kind 0 send / 1 sidechain, from, to) -> index or -1
                let mut en = e!();
                let (from, to) = (a.u(1), a.u(2));
                let n = en.project.tracks.len();
                let kind = if a.n(0) > 2.5 { RouteKind::Mod } else if a.n(0) > 1.5 { RouteKind::Duck } else if a.n(0) > 0.5 { RouteKind::Sidechain } else { RouteKind::Send };
                if from >= n || to >= n || en.project.would_cycle(from, to) {
                    return Ok(Val::Num(-1.0));
                }
                if let Some(i) = en.project.routes.iter().position(|r| r.kind == kind && r.from == from && r.to == to) {
                    ok!(i)
                }
                en.project.routes.push(Route { kind, from, to, amount: if kind == RouteKind::Send { 0.5 } else { 1.0 }, keys: vec![] });
                self.touch();
                ok!(en.project.routes.len() - 1)
            }
            "daw_route_get" => {
                let en = e!();
                let Some(r) = en.project.routes.get(a.u(0)) else { return Ok(Val::Num(0.0)) };
                ok!(match a.s(1).as_str() {
                    "from" => r.from as f64,
                    "to" => r.to as f64,
                    "kind" => match r.kind {
                        RouteKind::Send => 0.0,
                        RouteKind::Sidechain => 1.0,
                        RouteKind::Duck => 2.0,
                        RouteKind::Mod => 3.0,
                    },
                    "amount" => r.amount as f64,
                    _ => 0.0,
                })
            }
            "daw_route_set" => {
                let mut en = e!();
                if let Some(r) = en.project.routes.get_mut(a.u(0)) {
                    if a.s(1) == "amount" {
                        r.amount = a.n(2).clamp(0.0, 2.0) as f32;
                    }
                }
                self.touch();
                Ok(Val::Unit)
            }
            "daw_route_remove" => {
                let mut en = e!();
                if a.u(0) < en.project.routes.len() {
                    en.project.routes.remove(a.u(0));
                }
                self.touch();
                Ok(Val::Unit)
            }
            // ── performance (clip launcher) ──
            "daw_perf_scenes" => ok!(e!().project.scenes.len()),
            "daw_perf_scene_add" => {
                let mut en = e!();
                if en.project.scenes.len() >= MAX_SCENES {
                    return Ok(Val::Num(-1.0));
                }
                let name = if a.s(0).is_empty() { format!("Scene {}", en.project.scenes.len() + 1) } else { a.s(0) };
                en.project.scenes.push(name);
                self.touch();
                ok!(en.project.scenes.len() - 1)
            }
            "daw_perf_scene_remove" => {
                let mut en = e!();
                let i = a.u(0);
                if i < en.project.scenes.len() {
                    en.project.scenes.remove(i);
                    if i < en.project.slots.len() {
                        en.project.slots.remove(i);
                    }
                }
                self.touch();
                Ok(Val::Unit)
            }
            "daw_perf_scene_name" => ok!(e!().project.scenes.get(a.u(0)).cloned().unwrap_or_default()),
            "daw_perf_scene_set_name" => {
                if let Some(s) = e!().project.scenes.get_mut(a.u(0)) {
                    *s = a.s(1);
                }
                self.touch();
                Ok(Val::Unit)
            }
            "daw_perf_slot" => ok!(e!().project.slot(a.u(0), a.u(1)) as f64),
            "daw_perf_slot_info" => {
                // (scene, track) -> [name, len, is_audio] or []
                let en = e!();
                let c = en.project.slot(a.u(0), a.u(1));
                let Some(clip) = (c >= 0).then(|| en.project.tracks.get(a.u(1)).and_then(|t| t.session.get(c as usize))).flatten() else {
                    return Ok(Val::List(vec![]));
                };
                ok!(Val::List(vec![Val::Str(clip.name.clone()), Val::Num(clip.len), Val::from(matches!(clip.data, ClipData::Audio { .. }))]))
            }
            "daw_perf_slot_from_clip" => {
                // (scene, track, arrangement clip) -> session clip index
                let mut en = e!();
                let (sc, t, c) = (a.u(0), a.u(1), a.u(2));
                if sc >= MAX_SCENES {
                    return Ok(Val::Num(-1.0));
                }
                let Some(mut clip) = en.project.tracks.get(t).and_then(|x| x.clips.get(c)).cloned() else { return Ok(Val::Num(-1.0)) };
                clip.start = 0.0;
                en.project.tracks[t].session.push(clip);
                let idx = (en.project.tracks[t].session.len() - 1) as i32;
                while en.project.scenes.len() <= sc {
                    let nm = format!("Scene {}", en.project.scenes.len() + 1);
                    en.project.scenes.push(nm);
                }
                en.project.set_slot(sc, t, idx);
                self.touch();
                ok!(idx as f64)
            }
            "daw_perf_slot_new" => {
                let mut en = e!();
                let (sc, t) = (a.u(0), a.u(1));
                if sc >= MAX_SCENES || t >= en.project.tracks.len() || matches!(en.project.tracks[t].kind, TrackKind::Audio) {
                    return Ok(Val::Num(-1.0));
                }
                en.project.tracks[t].session.push(Clip::midi(0.0, a.n(2).max(1.0), "Pattern"));
                let idx = (en.project.tracks[t].session.len() - 1) as i32;
                while en.project.scenes.len() <= sc {
                    let nm = format!("Scene {}", en.project.scenes.len() + 1);
                    en.project.scenes.push(nm);
                }
                en.project.set_slot(sc, t, idx);
                self.touch();
                ok!(idx as f64)
            }
            "daw_perf_slot_copy" => {
                // (fromScene, fromTrack, toScene, toTrack) -> new session clip index or -1
                let mut en = e!();
                let (fs, ft, ts, tt) = (a.u(0), a.u(1), a.u(2), a.u(3));
                if ts >= MAX_SCENES {
                    return Ok(Val::Num(-1.0));
                }
                let c = en.project.slot(fs, ft);
                let Some(clip) = (c >= 0).then(|| en.project.tracks.get(ft).and_then(|t| t.session.get(c as usize)).cloned()).flatten() else {
                    return Ok(Val::Num(-1.0));
                };
                let audio = matches!(clip.data, ClipData::Audio { .. });
                match en.project.tracks.get(tt) {
                    Some(t) if matches!(t.kind, TrackKind::Audio) == audio => {}
                    _ => return Ok(Val::Num(-1.0)),
                }
                en.project.tracks[tt].session.push(clip);
                let idx = (en.project.tracks[tt].session.len() - 1) as i32;
                while en.project.scenes.len() <= ts {
                    let nm = format!("Scene {}", en.project.scenes.len() + 1);
                    en.project.scenes.push(nm);
                }
                en.project.set_slot(ts, tt, idx);
                self.touch();
                ok!(idx as f64)
            }
            "daw_perf_slot_clear" => {
                let mut en = e!();
                let c = en.project.slot(a.u(0), a.u(1));
                if c >= 0 {
                    en.project.remove_session_clip(a.u(1), c as usize);
                    en.project.set_slot(a.u(0), a.u(1), -1);
                }
                self.touch();
                Ok(Val::Unit)
            }
            "daw_perf_launch" => {
                e!().launch(a.u(1), a.u(0));
                Ok(Val::Unit)
            }
            "daw_perf_launch_scene" => {
                e!().launch_scene(a.u(0));
                Ok(Val::Unit)
            }
            "daw_perf_stop_track" => {
                e!().stop_track(a.u(0));
                Ok(Val::Unit)
            }
            "daw_perf_stop_all" => {
                e!().stop_all_session();
                Ok(Val::Unit)
            }
            "daw_perf_state" => {
                let en = e!();
                let (p, q, prog) = en.sess_state(a.u(0));
                ok!(Val::List(vec![Val::Num(p as f64), Val::Num(q as f64), Val::Num(prog)]))
            }
            "daw_perf_only" => ok!(e!().project.session_only),
            "daw_perf_set_only" => {
                e!().project.session_only = a.n(0) > 0.5;
                self.touch();
                Ok(Val::Unit)
            }
            "daw_perf_quant" => ok!(e!().project.quant),
            "daw_perf_set_quant" => {
                e!().project.quant = a.n(0).max(0.0);
                self.touch();
                Ok(Val::Unit)
            }
            "daw_perf_auto" => {
                let mut en = e!();
                crate::perf::auto_scenes(&mut en.project, a.n(0) as u8);
                self.touch();
                ok!(en.project.scenes.len())
            }
            // ── automation ──
            "daw_auto_lanes" => {
                let en = e!();
                let lanes = if a.n(0) < 0.0 { Some(&en.project.master_auto) } else { en.project.tracks.get(a.u(0)).map(|t| &t.auto) };
                ok!(Val::List(lanes.map(|l| l.iter().map(|x| Val::Str(x.param.clone())).collect()).unwrap_or_default()))
            }
            "daw_auto_lane_add" => {
                let mut en = e!();
                let param = a.s(1);
                let valid = if a.n(0) < 0.0 {
                    param == "vol"
                        || param.strip_prefix("fx").and_then(|r| r.split_once('.')).map(|(i, p)| {
                            i.parse::<usize>().ok().and_then(|i| en.project.master_effects.get(i)).map(|e| p.parse::<usize>().map(|p| p < e.params.len()).unwrap_or(false)).unwrap_or(false)
                        }).unwrap_or(false)
                } else {
                    match en.project.tracks.get(a.u(0)) {
                        None => false,
                        Some(t) => {
                            param == "vol"
                                || param == "pan"
                                || t.inst.as_ref().map(|i| i.params().iter().any(|(n, _)| *n == param)).unwrap_or(false)
                                || param.strip_prefix("fx").and_then(|r| r.split_once('.')).map(|(i, p)| {
                                    i.parse::<usize>().ok().and_then(|i| t.effects.get(i)).map(|e| p.parse::<usize>().map(|p| p < e.params.len()).unwrap_or(false)).unwrap_or(false)
                                }).unwrap_or(false)
                        }
                    }
                };
                if !valid {
                    return Ok(Val::Num(-1.0));
                }
                let lanes = if a.n(0) < 0.0 { Some(&mut en.project.master_auto) } else { en.project.tracks.get_mut(a.u(0)).map(|t| &mut t.auto) };
                let Some(lanes) = lanes else { return Ok(Val::Num(-1.0)) };
                if let Some(i) = lanes.iter().position(|l| l.param == param) {
                    ok!(i)
                }
                lanes.push(AutoLane { param, points: vec![] });
                self.touch();
                ok!(lanes.len() - 1)
            }
            "daw_midi_autos" => {
                // (path) -> [name, guessed part (-1 = unknown), FL parameter, points] * N: the automation clips an FL Studio project brings
                let Ok(data) = std::fs::read(a.s(0)) else { ok!(Val::List(vec![])) };
                if !crate::flp_import::looks_like_flp(&data) {
                    ok!(Val::List(vec![]))
                }
                let parts = crate::midi_import::parse_any(&a.s(0), &data).map(|m| m.tracks).unwrap_or_default();
                let mut v = vec![];
                for au in crate::flp_import::automation(&data) {
                    let g = guess_part(&parts, &au.target).map(|x| x as f64).unwrap_or(-1.0);
                    v.extend([Val::Str(au.name), Val::Num(g), Val::Str(au.param), Val::Num(au.points.len() as f64)]);
                }
                ok!(Val::List(v))
            }
            "daw_auto_lane_set_param" => {
                // (track, lane, param) -> true when the lane now moves `param` (vol, pan, bend, an instrument parameter, fx<i>.<p>, or fl:<name>
                // to leave it unassigned). Another lane of the same parameter gives way to this one.
                let mut en = e!();
                let param = a.s(2);
                let Some(t) = en.project.tracks.get_mut(a.u(0)) else { ok!(false) };
                let valid = param == "vol"
                    || param == "pan"
                    || param == "bend"
                    || param.starts_with("fl:")
                    || t.inst.as_ref().map(|i| i.params().iter().any(|(n, _)| *n == param)).unwrap_or(false)
                    || param
                        .strip_prefix("fx")
                        .and_then(|r| r.split_once('.'))
                        .map(|(i, p)| i.parse::<usize>().ok().and_then(|i| t.effects.get(i)).map(|e| p.parse::<usize>().map(|p| p < e.params.len()).unwrap_or(false)).unwrap_or(false))
                        .unwrap_or(false);
                let li = a.u(1);
                if !valid || li >= t.auto.len() {
                    ok!(false)
                }
                t.auto[li].param = param.clone();
                if let Some(j) = (0..t.auto.len()).find(|&j| j != li && t.auto[j].param == param) {
                    t.auto.remove(j);
                }
                drop(en);
                self.touch();
                ok!(true)
            }
            "daw_auto_lane_remove" => {
                let mut en = e!();
                let lanes = if a.n(0) < 0.0 { Some(&mut en.project.master_auto) } else { en.project.tracks.get_mut(a.u(0)).map(|t| &mut t.auto) };
                if let Some(l) = lanes {
                    if a.u(1) < l.len() {
                        l.remove(a.u(1));
                    }
                }
                self.touch();
                Ok(Val::Unit)
            }
            "daw_auto_points" => {
                let en = e!();
                let lanes = if a.n(0) < 0.0 { Some(&en.project.master_auto) } else { en.project.tracks.get(a.u(0)).map(|t| &t.auto) };
                let mut v = vec![];
                if let Some(l) = lanes.and_then(|l| l.get(a.u(1))) {
                    for (b, x) in &l.points {
                        v.push(Val::Num(*b));
                        v.push(Val::from(*x));
                    }
                }
                ok!(Val::List(v))
            }
            "daw_auto_points_range" => {
                // (target or -1, lane, from beat, to beat) -> [index of the first point returned, beat, value, ...]: the points inside the range plus
                // one neighbour on each side (so the polyline reaches the edges). A lane drawn from this never copies a long lane every frame.
                let en = e!();
                let lanes = if a.n(0) < 0.0 { Some(&en.project.master_auto) } else { en.project.tracks.get(a.u(0)).map(|t| &t.auto) };
                let mut v = vec![Val::Num(0.0)];
                if let Some(l) = lanes.and_then(|l| l.get(a.u(1))) {
                    let p = &l.points;
                    let (b0, b1) = (a.n(2), a.n(3));
                    let lo = p.partition_point(|x| x.0 < b0).saturating_sub(1);
                    let hi = (p.partition_point(|x| x.0 <= b1) + 1).min(p.len());
                    v[0] = Val::Num(lo as f64);
                    for (b, x) in p.iter().take(hi).skip(lo) {
                        v.push(Val::Num(*b));
                        v.push(Val::from(*x));
                    }
                }
                ok!(Val::List(v))
            }
            "daw_auto_lane_empty" => {
                // (target or -1, lane) -> 1 when the lane holds no points at all
                let en = e!();
                let lanes = if a.n(0) < 0.0 { Some(&en.project.master_auto) } else { en.project.tracks.get(a.u(0)).map(|t| &t.auto) };
                ok!(lanes.and_then(|l| l.get(a.u(1))).map(|l| l.points.is_empty()).unwrap_or(true))
            }
            "daw_auto_set_points" => {
                // (track or -1, lane, flat list [beat, value, ...])
                let mut en = e!();
                let Some(Val::List(flat)) = a.0.get(2) else { return Ok(Val::Unit) };
                let mut pts: Vec<(f64, f32)> = flat
                    .chunks_exact(2)
                    .filter_map(|c| match (&c[0], &c[1]) {
                        (Val::Num(b), Val::Num(v)) => Some((*b, *v as f32)),
                        _ => None,
                    })
                    .collect();
                pts.retain(|p| p.0.is_finite() && p.1.is_finite());
                pts.sort_by(|x, y| x.0.total_cmp(&y.0));
                let lanes = if a.n(0) < 0.0 { Some(&mut en.project.master_auto) } else { en.project.tracks.get_mut(a.u(0)).map(|t| &mut t.auto) };
                if let Some(l) = lanes.and_then(|l| l.get_mut(a.u(1))) {
                    l.points = pts;
                }
                self.touch();
                Ok(Val::Unit)
            }
            "daw_inst_default" => {
                // (track, name) -> the value a fresh instrument of the track's kind has for that parameter, -1 when unknown
                let en = e!();
                let v = en.project.tracks.get(a.u(0)).and_then(|t| crate::dsp::make_instrument(&t.inst_kind, en.sr)).and_then(|i| i.get_param(&a.s(1)));
                ok!(v.map(|x| x as f64).unwrap_or(-1.0))
            }
            "daw_fx_default" => {
                // (target, effect, param index) -> the effect's default for that parameter, -1 when unknown
                let mut en = e!();
                let name = fx_target(&mut en, a.n(0) as i64).and_then(|v| v.get(a.u(1))).map(|s| s.name.clone());
                ok!(name.and_then(|n| crate::dsp::fx::defaults(&n).get(a.u(2)).map(|d| d.1 as f64)).unwrap_or(-1.0))
            }
            "daw_auto_shape" => {
                // (target or -1, lane, shape, start beat, end beat, period beats, lo, hi, seed): program a lane with a repeating shape
                let mut en = e!();
                let shape = a.s(2);
                let (b0, b1) = (a.n(3).max(0.0), a.n(4));
                let period = a.n(5).max(0.125);
                let (lo, hi) = (a.n(6).clamp(0.0, 1.0) as f32, a.n(7).clamp(0.0, 1.0) as f32);
                if !(b1 > b0) || !b1.is_finite() {
                    return Ok(Val::Num(0.0));
                }
                let pts = auto_shape_points(&shape, b0, b1.min(b0 + 4096.0), period, lo, hi, a.n(8) as u32);
                let lanes = if a.n(0) < 0.0 { Some(&mut en.project.master_auto) } else { en.project.tracks.get_mut(a.u(0)).map(|t| &mut t.auto) };
                let n = pts.len();
                if let Some(l) = lanes.and_then(|l| l.get_mut(a.u(1))) {
                    // the programmed stretch replaces whatever the lane held inside it
                    l.points.retain(|p| p.0 < b0 - 1e-9 || p.0 > b1 + 1e-9);
                    l.points.extend(pts);
                    l.points.sort_by(|x, y| x.0.total_cmp(&y.0));
                }
                self.touch();
                ok!(n)
            }
            "daw_auto_rec" => {
                e!().auto_rec = a.n(0) > 0.5;
                Ok(Val::Unit)
            }
            "daw_rev" => {
                let en = e!();
                ok!((self.rev + en.rec_rev) as f64)
            }
            // clip clipboard (holds real clip data, so cut + paste survives edits)
            "daw_clipboard_copy" => {
                // (track, clip, first 1 = clear the clipboard first)
                if a.n(2) > 0.5 {
                    self.clipboard.clear();
                }
                let en = e!();
                if let Some(c) = en.project.tracks.get(a.u(0)).and_then(|t| t.clips.get(a.u(1))) {
                    self.clipboard.push((a.u(0), c.clone()));
                }
                ok!(self.clipboard.len())
            }
            "daw_clipboard_count" => ok!(self.clipboard.len()),
            "daw_clipboard_info" => {
                // i -> [track, start, len, isAudio]
                let Some((t, c)) = self.clipboard.get(a.u(0)) else { return Ok(Val::List(vec![])) };
                ok!(Val::List(vec![Val::Num(*t as f64), Val::Num(c.start), Val::Num(c.len), Val::from(matches!(c.data, ClipData::Audio { .. }))]))
            }
            "daw_clipboard_paste" => {
                // (i, track, start) -> new clip index or -1 (kind mismatch)
                let Some((_, clip)) = self.clipboard.get(a.u(0)).cloned() else { return Ok(Val::Num(-1.0)) };
                let mut en = e!();
                let Some(t) = en.project.tracks.get_mut(a.u(1)) else { return Ok(Val::Num(-1.0)) };
                if matches!(t.kind, TrackKind::Audio) != matches!(clip.data, ClipData::Audio { .. }) {
                    return Ok(Val::Num(-1.0));
                }
                let mut clip = clip;
                clip.start = a.n(2).max(0.0);
                t.clips.push(clip);
                let idx = t.clips.len() - 1;
                drop(en);
                self.touch();
                ok!(idx)
            }
            "daw_clip_wave" => {
                // (t, c, frac0, frac1, bins): peaks of the [frac0, frac1) part of the source (fractions of its duration)
                let en = e!();
                let bins = a.u(4).clamp(1, 4096);
                let Some(Clip { data: ClipData::Audio { sample, .. }, .. }) = en.project.tracks.get(a.u(0)).and_then(|t| t.clips.get(a.u(1))) else {
                    return Ok(Val::List(vec![]));
                };
                let key = (Arc::as_ptr(sample) as usize, 2048usize);
                let pk = self.peaks.entry(key).or_insert_with(|| sample.peaks(2048));
                let (f0, f1) = (a.n(2), a.n(3));
                let mut out = Vec::with_capacity(bins);
                for i in 0..bins {
                    let u0 = f0 + (f1 - f0) * i as f64 / bins as f64;
                    let u1 = f0 + (f1 - f0) * (i + 1) as f64 / bins as f64;
                    if u1 <= 0.0 || u0 >= 1.0 {
                        out.push(Val::Num(0.0));
                        continue;
                    }
                    let b0 = ((u0.max(0.0) * 2048.0) as usize).min(2047);
                    let b1 = (((u1.min(1.0) * 2048.0).ceil()) as usize).clamp(b0 + 1, 2048);
                    let m = pk[b0..b1].iter().fold(0.0f32, |x, y| x.max(*y));
                    out.push(Val::from(m));
                }
                ok!(Val::List(out))
            }
            "daw_project_name" => ok!(e!().project.name.clone()),
            "daw_set_project_name" => {
                e!().project.name = a.s(0);
                self.touch();
                Ok(Val::Unit)
            }
            "daw_project_path" => ok!(self.path.clone()),
            // ── transport ──
            "daw_play" => {
                e!().play();
                Ok(Val::Unit)
            }
            "daw_pause" => {
                e!().pause();
                Ok(Val::Unit)
            }
            "daw_stop" => {
                e!().stop();
                Ok(Val::Unit)
            }
            "daw_seek" => {
                e!().seek(a.n(0));
                Ok(Val::Unit)
            }
            "daw_playing" => ok!(e!().playing),
            "daw_position" => ok!(e!().position),
            "daw_bpm" => ok!(e!().project.bpm),
            "daw_set_bpm" => {
                let mut en = e!();
                en.project.bpm = a.n(0).clamp(20.0, 400.0);
                en.project.prepare_audio();
                drop(en);
                self.touch();
                Ok(Val::Unit)
            }
            "daw_loop_on" => ok!(e!().project.loop_on),
            "daw_loop_start" => ok!(e!().project.loop_start),
            "daw_loop_end" => ok!(e!().project.loop_end),
            "daw_set_loop" => {
                let mut en = e!();
                en.project.loop_on = a.n(0) > 0.5;
                if a.0.len() >= 3 {
                    en.project.loop_start = a.n(1).max(0.0);
                    en.project.loop_end = a.n(2).max(a.n(1) + 0.25);
                }
                Ok(Val::Unit)
            }
            "daw_end_beat" => ok!(e!().project.end_beat()),
            // ── master ──
            "daw_master_get" => {
                let en = e!();
                ok!(match a.s(0).as_str() {
                    "vol" => en.project.master_volume as f64,
                    "meter_l" => en.project.master_meter[0] as f64,
                    "meter_r" => en.project.master_meter[1] as f64,
                    _ => 0.0,
                })
            }
            "daw_master_set" => {
                if a.s(0) == "vol" {
                    let mut en = e!();
                    en.project.master_volume = a.n(1).clamp(0.0, 2.0) as f32;
                    en.record_auto(None, "vol", a.n(1).clamp(0.0, 2.0) as f32);
                    drop(en);
                    self.touch();
                }
                Ok(Val::Unit)
            }
            // ── tracks ──
            "daw_track_count" => ok!(e!().project.tracks.len()),
            "daw_track_add" => {
                // (kind "synth"|"drums"|"audio", optional patch path)
                let kind = a.s(0);
                let patch_path = a.s(1);
                if kind == "sf2" {
                    // (sf2, path to a .sf2 file)
                    let patch_path = abs_path(&patch_path);
                    let Some(inst) = crate::dsp::make_instrument_file("sf2", sr, &patch_path) else {
                        self.status = format!("Could not load soundfont {patch_path}");
                        return Ok(Val::Num(-1.0));
                    };
                    let mut en = e!();
                    let n = en.project.tracks.len();
                    if n >= MAX_TRACKS {
                        return Ok(Val::Num(-1.0));
                    }
                    let label = std::path::Path::new(&patch_path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "SoundFont".into());
                    let mut t = Track::new(&label, TrackKind::Instrument, PALETTE[n % 7], Some(inst));
                    t.inst_kind = "sf2".into();
                    t.patch = patch_path;
                    default_comp(&mut t, sr);
                    en.project.tracks.push(t);
                    self.touch();
                    ok!(n)
                }
                let patch_text = if kind != "audio" && !patch_path.is_empty() { Some(std::fs::read_to_string(&patch_path).map_err(|x| format!("{patch_path}: {x}"))) } else { None };
                let mut en = e!();
                let n = en.project.tracks.len();
                if n >= MAX_TRACKS {
                    return Ok(Val::Num(-1.0));
                }
                let color = PALETTE[n % 7];
                let mut t;
                if kind == "audio" {
                    t = Track::new(&format!("Audio {}", n + 1), TrackKind::Audio, color, None);
                } else {
                    let path = a.s(1);
                    let mut patch_doc = patch::Patch { kind: kind.clone(), ..Default::default() };
                    if let Some(text) = patch_text {
                        patch_doc = patch::parse(&text?);
                    }
                    let inst = patch::build(&patch_doc, sr).ok_or_else(|| format!("unknown instrument type '{}'", patch_doc.kind))?;
                    let label = if !patch_doc.name.is_empty() {
                        patch_doc.name.clone()
                    } else if patch_doc.kind == "drums" {
                        format!("Drums {}", n + 1)
                    } else {
                        format!("Synth {}", n + 1)
                    };
                    t = Track::new(&label, TrackKind::Instrument, color, Some(inst));
                    t.inst_kind = patch_doc.kind.clone();
                    t.patch = path;
                    t.effects = project::patch_effects(&patch_doc, sr);
                }
                default_comp(&mut t, sr);
                en.project.tracks.push(t);
                self.touch();
                ok!(n)
            }
            "daw_track_remove" => {
                self.ab.clear();
                let mut en = e!();
                let i = a.u(0);
                if i < en.project.tracks.len() {
                    en.project.remove_track(i);
                    en.on_track_removed(i);
                }
                self.touch();
                Ok(Val::Unit)
            }
            "daw_track_move" => {
                self.ab.clear();
                // (i, delta) -> new index
                let mut en = e!();
                let i = a.u(0);
                let n = en.project.tracks.len();
                if n == 0 || i >= n {
                    return Ok(Val::Num(i as f64));
                }
                let j = (i as i64 + a.n(1) as i64).clamp(0, n as i64 - 1) as usize;
                if i != j {
                    en.project.move_track(i, j);
                    en.on_track_moved(i, j);
                    self.touch();
                }
                ok!(j)
            }
            "daw_track_dup" => {
                // (i, with_clips) -> new index; clones instrument state + fx
                let doc = {
                    let en = e!();
                    project::to_doc(&en)
                };
                let i = a.u(0);
                let Some(td) = doc.tracks.get(i).cloned() else { return Ok(Val::Num(-1.0)) };
                let mut td = td;
                {
                    // "Drums copy" / "Drums copy 3" -> "Drums copy N" with the next free N
                    let root = match td.name.rfind(" copy") {
                        Some(p) if td.name[p + 5..].trim().chars().all(|c| c.is_ascii_digit()) => td.name[..p].to_string(),
                        _ => td.name.clone(),
                    };
                    let mut cand = format!("{root} copy");
                    let mut k = 2;
                    while doc.tracks.iter().any(|t| t.name == cand) {
                        cand = format!("{root} copy {k}");
                        k += 1;
                    }
                    td.name = cand;
                    td.color = PALETTE[doc.tracks.len() % 7];
                }
                if a.n(1) < 0.5 {
                    td.clips.clear();
                    td.session.clear();
                }
                let with_clips = a.n(1) >= 0.5;
                let mut d2 = doc;
                d2.tracks.insert(i + 1, td);
                for r in d2.routes.iter_mut() {
                    if r.from > i {
                        r.from += 1;
                    }
                    if r.to > i {
                        r.to += 1;
                    }
                }
                for row in d2.slots.iter_mut() {
                    if row.len() > i {
                        let v = if with_clips { row[i] } else { -1 };
                        row.insert(i + 1, v);
                    }
                }
                let base = std::path::Path::new(&self.path).parent().map(|p| p.to_path_buf());
                project::preload(&d2, base.as_deref(), &mut self.samples);
                let mut en = e!();
                let pos = en.position;
                let playing = en.playing;
                project::apply_doc(&mut en, &d2, base.as_deref(), &mut self.samples);
                en.position = pos;
                en.playing = playing;
                self.touch();
                ok!(i + 1)
            }
            "daw_track_name" => ok!(e!().project.tracks.get(a.u(0)).map(|t| t.name.clone()).unwrap_or_default()),
            "daw_track_set_name" => {
                if let Some(t) = e!().project.tracks.get_mut(a.u(0)) {
                    t.name = a.s(1);
                }
                self.touch();
                Ok(Val::Unit)
            }
            "daw_track_inst" => ok!(e!().project.tracks.get(a.u(0)).and_then(|t| t.inst.as_ref().map(|i| i.name().to_string())).unwrap_or_else(|| "Audio".into())),
            "daw_track_patch" => ok!(e!().project.tracks.get(a.u(0)).map(|t| t.patch.clone()).unwrap_or_default()),
            "daw_track_kind" => ok!(e!().project.tracks.get(a.u(0)).map(|t| if matches!(t.kind, TrackKind::Audio) { "audio".to_string() } else { t.inst_kind.clone() }).unwrap_or_default()),
            "daw_track_get" => {
                let en = e!();
                let Some(t) = en.project.tracks.get(a.u(0)) else { return Ok(Val::Num(0.0)) };
                ok!(match a.s(1).as_str() {
                    "vol" => t.volume as f64,
                    "pan" => t.pan as f64,
                    "mute" => t.mute as i32 as f64,
                    "solo" => t.solo as i32 as f64,
                    "meter_l" => t.meter[0] as f64,
                    "meter_r" => t.meter[1] as f64,
                    "audio" => matches!(t.kind, TrackKind::Audio) as i32 as f64,
                    "cr" => ((t.color >> 16) & 255) as f64,
                    "cg" => ((t.color >> 8) & 255) as f64,
                    "cb" => (t.color & 255) as f64,
                    "clips" => t.clips.len() as f64,
                    "fx" => t.effects.len() as f64,
                    _ => 0.0,
                })
            }
            "daw_track_set" => {
                let mut en = e!();
                let v = a.n(2);
                if let Some(t) = en.project.tracks.get_mut(a.u(0)) {
                    match a.s(1).as_str() {
                        "vol" => t.volume = v.clamp(0.0, 2.0) as f32,
                        "pan" => t.pan = v.clamp(-1.0, 1.0) as f32,
                        "mute" => t.mute = v > 0.5,
                        "solo" => t.solo = v > 0.5,
                        "color" => t.color = PALETTE[(v as usize) % 7],
                        _ => {}
                    }
                }
                let field = a.s(1);
                if field == "vol" || field == "pan" {
                    en.record_auto(Some(a.u(0)), &field, v as f32);
                }
                self.touch();
                Ok(Val::Unit)
            }
            "daw_palette" => ok!(Val::List(PALETTE.iter().map(|c| Val::Num(*c as f64)).collect())),
            // ── clips ──
            "daw_clip_count" => ok!(e!().project.tracks.get(a.u(0)).map(|t| t.clips.len()).unwrap_or(0)),
            "daw_clip_add_midi" => {
                let mut en = e!();
                let Some(t) = en.project.tracks.get_mut(a.u(0)) else { return Ok(Val::Num(-1.0)) };
                t.clips.push(Clip::midi(a.n(1).max(0.0), a.n(2).max(0.25), "Pattern"));
                self.touch();
                ok!(t.clips.len() - 1)
            }
            "daw_clip_add_audio" => {
                let path = a.s(1);
                let smp = match self.load_sample_cached(&path) {
                    Ok(s) => s,
                    Err(x) => {
                        self.status = format!("Import failed: {x}");
                        return Ok(Val::Num(-1.0));
                    }
                };
                let mut en = e!();
                let len = (smp.seconds() * en.project.bpm / 60.0).max(0.25);
                let name = smp.name.clone();
                let Some(t) = en.project.tracks.get_mut(a.u(0)) else { return Ok(Val::Num(-1.0)) };
                t.clips.push(Clip { start: a.n(2).max(0.0), len, name, loop_len: 0.0, lyric_text: String::new(), lyric_lang: String::new(), tag: String::new(), data: ClipData::Audio { sample: smp, offset_s: 0.0, gain: 1.0, pitch: 0.0, warp: 0.0 } });
                self.touch();
                ok!(t.clips.len() - 1)
            }
            "daw_ai_available" => ok!(cfg!(feature = "llm")),
            "daw_ai_prompt" => {
                // path -> the question for the model: the parts, then the roles it may answer with
                let Ok(data) = std::fs::read(a.s(0)) else { return ok!(String::new()) };
                match crate::midi_import::parse_any(&a.s(0), &data) {
                    Ok(m) => {
                        let file = std::path::Path::new(&a.s(0)).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                        ok!(crate::ai_import::prompt(&file, &m))
                    }
                    Err(_) => ok!(String::new()),
                }
            }
            "daw_ai_choose" => {
                // (path, model reply) -> flat [part, plugin, patch, why] for EVERY part: the model's role where it gave one, a guess otherwise
                let Ok(data) = std::fs::read(a.s(0)) else { return ok!(Val::List(vec![])) };
                match crate::midi_import::parse_any(&a.s(0), &data) {
                    Ok(m) => {
                        let reply = crate::ai_import::parse_reply(&a.s(1));
                        let c = crate::ai_import::choose_for(&m, &reply, &a.s(2));
                        ok!(Val::List(c.into_iter().flat_map(|c| [Val::Num(c.part as f64), Val::Str(c.plugin), Val::Str(c.patch), Val::Str(c.why)]).collect()))
                    }
                    Err(_) => ok!(Val::List(vec![])),
                }
            }
            "daw_ai_example" => ok!(Val::List({
                let (q, a) = crate::ai_import::example();
                vec![Val::Str(q), Val::Str(a)]
            })),
            "daw_ai_genre_prompt" => {
                // path -> the first question for the model: what genre is this?
                let Ok(data) = std::fs::read(a.s(0)) else { return ok!(String::new()) };
                match crate::midi_import::parse_any(&a.s(0), &data) {
                    Ok(m) => {
                        let file = std::path::Path::new(&a.s(0)).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                        ok!(crate::ai_import::genre_prompt(&file, &m))
                    }
                    Err(_) => ok!(String::new()),
                }
            }
            "daw_ai_genres" => ok!(crate::ai_import::GENRES.join(", ")),
            "daw_ai_parse_genre" => ok!(crate::ai_import::parse_genre(&a.s(0)).unwrap_or("").to_string()),
            "daw_ai_guess_genre" => {
                let Ok(data) = std::fs::read(a.s(0)) else { return ok!(String::new()) };
                match crate::midi_import::parse_any(&a.s(0), &data) {
                    Ok(m) => ok!(crate::ai_import::guess_genre(&m).to_string()),
                    Err(_) => ok!(String::new()),
                }
            }
            "daw_ai_palette" => ok!(crate::soundcat::palette(&a.s(0))),
            "daw_sound_profile" => {
                // (plugin, patch name) -> a sentence about how the patch measured, "" when it is not measured
                let r = crate::api::song_find_patch(&a.s(0), &a.s(1));
                ok!(r.map(|p| crate::soundcat::describe(&a.s(1), &p)).unwrap_or_default())
            }
            "daw_ai_roles" => ok!(crate::ai_import::ROLES.join(", ")),
            "daw_ai_style" => ok!(crate::ai_import::parse_reply(&a.s(0)).style),
            "daw_midi_scan" => {
                // path -> flat [name, program (-1 = drums, -2 = audio, -3 = module samples), channel, notes, bank, GM name, percussive 0/1, has own samples 0/1] per part, in import order; [] on error
                let Ok(data) = std::fs::read(a.s(0)) else { return ok!(Val::List(vec![])) };
                match crate::midi_import::parse_any(&a.s(0), &data) {
                    Ok(m) => {
                        let mut v = vec![];
                        for t in &m.tracks {
                            v.push(Val::Str(t.name.clone()));
                            v.push(Val::Num(if !t.samples.is_empty() { -2.0 } else if t.sampler.is_some() { -3.0 } else { t.program.map(|p| p as f64).unwrap_or(-1.0) }));
                            v.push(Val::Num(t.channel as f64 + 1.0));
                            v.push(Val::Num(t.notes.len() as f64));
                            v.push(Val::Num(t.bank as f64));
                            v.push(Val::Str(if !t.samples.is_empty() { "Audio samples".to_string() } else if t.sampler.is_some() { "Module samples".to_string() } else { t.program.map(|p| crate::midi_import::gm_name(p).to_string()).unwrap_or_else(|| "Drums".into()) }));
                            v.push(Val::Num(t.percussive as u8 as f64));
                            v.push(Val::Num(t.sampler.is_some() as u8 as f64));
                        }
                        ok!(Val::List(v))
                    }
                    Err(x) => {
                        self.status = format!("MIDI import failed: {x}");
                        ok!(Val::List(vec![]))
                    }
                }
            }
            "daw_midi_import" => {
                // (path, beat) -> number of tracks created; each MIDI track/channel gets its own track,
                // GM program -> synth patch, channel 10 -> drum kit
                let data = std::fs::read(a.s(0)).map_err(|x| format!("{}: {x}", a.s(0)))?;
                let mid = match crate::midi_import::parse_any(&a.s(0), &data) {
                    Ok(m) => m,
                    Err(x) => {
                        self.status = format!("MIDI import failed: {x}");
                        return Ok(Val::Num(-1.0));
                    }
                };
                let start = a.n(1).max(0.0);
                // optional third argument: one choice per part - "" / "auto" (soundfont or built-in GM), "gm" (built-in GM
                // sound only), "sf2" (the configured soundfont only) or the path of a .ling patch to play the part with
                let choices: Vec<String> = match a.0.get(2) {
                    Some(Val::List(l)) => l.iter().map(|v| if let Val::Str(s) = v { s.clone() } else { String::new() }).collect(),
                    _ => vec![],
                };
                let file = std::path::Path::new(&a.s(0)).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                // library samples some parts asked for ("lib:snare", "kitlib:crash cymbal"): found before the engine is locked
                let mut lib_hits: HashMap<String, Option<String>> = HashMap::new();
                for c in &choices {
                    if let Some(kw) = c.strip_prefix("lib:").or_else(|| c.strip_prefix("kitlib:")) {
                        if !lib_hits.contains_key(kw) {
                            let hit = self.find_sample(kw);
                            lib_hits.insert(kw.to_string(), hit);
                        }
                    }
                }
                // audio parts (FL Studio audio clips / sample channels): decode the samples before taking the engine lock
                let mut loaded: HashMap<String, Arc<Sample>> = HashMap::new();
                for it in &mid.tracks {
                    for (_, p) in &it.samples {
                        if !loaded.contains_key(p) {
                            if let Ok(s) = self.load_sample_cached(p) {
                                loaded.insert(p.clone(), s);
                            }
                        }
                    }
                }
                let mut en = e!();
                let empty = en.project.tracks.iter().all(|t| t.clips.is_empty());
                if let (true, Some(b)) = (empty, mid.bpm) {
                    en.project.bpm = b.clamp(20.0, 300.0);
                    // later tempo changes become the project's tempo map
                    en.project.tempo_map = mid.tempos.iter().skip(1).map(|(beat, v)| (start + beat, v.clamp(20.0, 400.0))).collect();
                }
                ensure_master_comp(&mut en, sr);
                let n0 = en.project.tracks.len();
                let mut made = 0usize;
                let (mut kit_idx, mut kit_used, mut kit_notes): (Option<usize>, Vec<u8>, Vec<Note>) = (None, vec![], vec![]);
                for (ti, it) in mid.tracks.iter().enumerate() {
                    let n = en.project.tracks.len();
                    if n >= MAX_TRACKS {
                        break;
                    }
                    if !it.samples.is_empty() {
                        let mut t = Track::new(&it.name, TrackKind::Audio, PALETTE[n % 7], None);
                        t.volume = it.volume.clamp(0.0, 1.5);
                        let bpm = en.project.bpm;
                        for (beat, p) in &it.samples {
                            if let Some(smp) = loaded.get(p) {
                                let len = (smp.seconds() * bpm / 60.0).max(0.25);
                                t.clips.push(Clip { start: start + beat, len, name: smp.name.clone(), loop_len: 0.0, lyric_text: String::new(), lyric_lang: String::new(), tag: String::new(), data: ClipData::Audio { sample: smp.clone(), offset_s: 0.0, gain: 1.0, pitch: 0.0, warp: 0.0 } });
                            }
                        }
                        if !t.clips.is_empty() {
                            default_comp(&mut t, sr);
                            en.project.tracks.push(t);
                            made += 1;
                        }
                        continue;
                    }
                    let choice = choices.get(ti).map(|s| s.trim().to_string()).unwrap_or_default();
                    // a patch file chosen for this part wins over everything else
                    let chosen: Option<patch::Patch> = if choice.is_empty() || ["auto", "gm", "sf2", "orig", "kit"].contains(&choice.as_str()) || choice.starts_with("sf2:") || choice.starts_with("lib:") || choice.starts_with("kitlib:") || choice.starts_with("file:") || choice.starts_with("kitfile:") {
                        None
                    } else if let Some(kind) = choice.strip_prefix("kind:") {
                        // a plugin with its default sound
                        Some(patch::Patch { kind: kind.to_string(), name: String::new(), ..Default::default() })
                    } else {
                        match std::fs::read_to_string(&choice) {
                            Ok(text) => Some(patch::parse(&text)),
                            Err(x) => {
                                self.status = format!("{choice}: {x} - using the default sound");
                                None
                            }
                        }
                    };
                    // a soundfont, when one is configured and has a matching preset; otherwise the built-in instruments
                    let mut sf_inst: Option<Box<dyn crate::dsp::Instrument>> = None;
                    // the soundfont for this part: one picked in the dialog, else the configured one (unless the part asked for a plain GM sound)
                    // a tracker module's own samples play through a Sampler unless another sound was picked
                    let own = it.sampler.is_some() && chosen.is_none() && (choice.is_empty() || ["auto", "orig", "kit"].contains(&choice.as_str()) || choice.starts_with("lib:") || choice.starts_with("kitlib:") || choice.starts_with("file:") || choice.starts_with("kitfile:"));
                    let mut sampler_inst: Option<Box<dyn crate::dsp::Instrument>> = None;
                    if own {
                        let json = it.sampler.as_ref().map(|s| s.0.clone()).unwrap_or_default();
                        let kit = choice == "kit" || choice.starts_with("kitlib:") || choice.starts_with("kitfile:");
                        let lib_path: Option<String> = choice
                            .strip_prefix("file:")
                            .or_else(|| choice.strip_prefix("kitfile:"))
                            .map(|p| p.to_string())
                            .or_else(|| choice.strip_prefix("lib:").or_else(|| choice.strip_prefix("kitlib:")).and_then(|kw| lib_hits.get(kw).cloned().flatten()));
                        if kit {
                            // every kit part becomes one key of a shared Sampler track
                            let key = kit_key(&it.name, &mut kit_used);
                            if kit_idx.is_none() {
                                let mut kt = Track::new("Module Kit", TrackKind::Instrument, PALETTE[n % 7], Some(Box::new(crate::dsp::sampler::Sampler::new(sr))));
                                kt.inst_kind = "sampler".into();
                                default_comp(&mut kt, sr);
                                en.project.tracks.push(kt);
                                kit_idx = Some(en.project.tracks.len() - 1);
                                made += 1;
                            }
                            if let Some(inst) = kit_idx.and_then(|k| en.project.tracks.get_mut(k)).and_then(|t| t.inst.as_mut()) {
                                let r = match &lib_path {
                                    Some(p) => inst.command("add_lib", &[key as f64, 0.0], p),
                                    None => inst.command("merge", &[key as f64], &json),
                                };
                                if r.first().copied().unwrap_or(-1.0) < 0.0 {
                                    self.status = format!("Kit: {}", inst.info("error"));
                                }
                            }
                            for nn in &it.notes {
                                let mut nn = nn.clone();
                                nn.key = key;
                                kit_notes.push(nn);
                            }
                            continue;
                        }
                        let built: Option<Box<dyn crate::dsp::Instrument>> = match &lib_path {
                            Some(p) => {
                                let mut s = crate::dsp::sampler::Sampler::new(sr);
                                crate::dsp::Instrument::command(&mut s, "add_lib", &[60.0, 0.0], p);
                                crate::dsp::Instrument::command(&mut s, "set", &[0.0, 1.0, 0.0], "");
                                crate::dsp::Instrument::command(&mut s, "set", &[0.0, 2.0, 127.0], "");
                                Some(Box::new(s))
                            }
                            None => crate::dsp::sampler::Sampler::from_config(sr, &json, None).ok().map(|s| Box::new(s) as Box<dyn crate::dsp::Instrument>),
                        };
                        if let Some(b) = built {
                            if let Some(p) = &lib_path {
                                self.status = format!("{}: using {}", it.name, p.rsplit('/').next().unwrap_or(p));
                            }
                            sampler_inst = Some(b);
                        }
                    }
                    let sf_path: String = if let Some(p) = choice.strip_prefix("sf2:") { p.to_string() } else if choice == "gm" || own { String::new() } else { self.gm_sf2.clone() };
                    if !sf_path.is_empty() && chosen.is_none() {
                        if let Some(mut si) = crate::dsp::make_instrument_file("sf2", sr, &sf_path) {
                            if si.midi_select(it.bank, it.program.unwrap_or(0) as u16, it.program.is_none()) {
                                sf_inst = Some(si);
                            }
                        }
                    }
                    let mut t;
                    if let Some(si) = sampler_inst {
                        t = Track::new(&it.name, TrackKind::Instrument, PALETTE[n % 7], Some(si));
                        t.inst_kind = "sampler".into();
                    } else if let Some(si) = sf_inst {
                        // name the part after the soundfont preset unless the file named it
                        let label = if it.named {
                            it.name.clone()
                        } else {
                            let pn = si.preset_names();
                            pn.get(si.preset_index()).map(|x| x.split_once("  ").map(|(_, r)| r.to_string()).unwrap_or_else(|| x.clone())).unwrap_or_else(|| it.name.clone())
                        };
                        t = Track::new(&label, TrackKind::Instrument, PALETTE[n % 7], Some(si));
                        t.inst_kind = "sf2".into();
                        t.patch = sf_path.clone();
                        t.effects = project::patch_effects(&patch::Patch { fx: vec![patch::FxSpec { name: "Reverb".into(), params: vec![("mix".into(), 0.12), ("size".into(), 0.55), ("damp".into(), 0.5)] }], ..Default::default() }, sr);
                    } else {
                        let named_choice = chosen.is_some();
                        let doc = match chosen {
                            Some(c) => c,
                            None => match it.program {
                                None => patch::Patch { kind: "drums".into(), name: "GM Drums".into(), ..Default::default() },
                                Some(p) => crate::midi_import::gm_patch(p),
                            },
                        };
                        let Some(mut inst) = patch::build(&doc, sr) else { continue };
                        if doc.kind == "growl" {
                            // its lanes run at the instrument's own tempo: follow the project
                            inst.set_param("tempo", ((en.project.bpm as f32 - 60.0) / 140.0).clamp(0.0, 1.0));
                        }
                        // keep the file's own name for the part, except when a patch was picked (then the patch names it)
                        let label = if named_choice && !it.named && !doc.name.is_empty() { format!("{} - {}", it.name, doc.name) } else { it.name.clone() };
                        t = Track::new(&label, TrackKind::Instrument, PALETTE[n % 7], Some(inst));
                        t.inst_kind = doc.kind.clone();
                        if named_choice {
                            t.patch = choice.clone();
                        }
                        t.effects = project::patch_effects(&doc, sr);
                    }
                    t.volume = it.volume.clamp(0.0, 1.5);
                    t.pan = it.pan.clamp(-1.0, 1.0);
                    if !it.vol_auto.is_empty() {
                        t.auto.push(AutoLane { param: "vol".into(), points: it.vol_auto.iter().map(|(b, v)| (start + b, *v)).collect() });
                    }
                    if let Some(i) = t.inst.as_mut() {
                        if it.program.is_some() {
                            i.set_param("bend range", (it.bend_range - 1.0) / 23.0);
                        }
                        // soundfont parts remember the bank / program they asked for even without a soundfont match
                        let _ = it.bank;
                    }
                    if !it.bend_auto.is_empty() && (it.program.is_some() || t.inst_kind == "sf2" || t.inst_kind == "sampler") {
                        t.auto.push(AutoLane { param: "bend".into(), points: it.bend_auto.iter().map(|(b, v)| (start + b, *v)).collect() });
                    }
                    if !it.pan_auto.is_empty() {
                        t.auto.push(AutoLane { param: "pan".into(), points: it.pan_auto.iter().map(|(b, v)| (start + b, *v)).collect() });
                    }
                    for (param, pts) in &it.extra_auto {
                        if !pts.is_empty() {
                            t.auto.push(AutoLane { param: param.clone(), points: pts.iter().map(|(b, v)| (start + b, *v)).collect() });
                        }
                    }
                    let len = it.notes.iter().map(|x| x.start + x.len).fold(0.0, f64::max).ceil().max(1.0);
                    t.clips.push(Clip { start, len, name: file.clone(), loop_len: 0.0, lyric_text: String::new(), lyric_lang: String::new(), tag: String::new(), data: ClipData::Midi(it.notes.clone()) });
                    default_comp(&mut t, sr);
                    en.project.tracks.push(t);
                    made += 1;
                }
                if let (Some(k), false) = (kit_idx, kit_notes.is_empty()) {
                    if let Some(t) = en.project.tracks.get_mut(k) {
                        let len = kit_notes.iter().map(|x| x.start + x.len).fold(0.0, f64::max).ceil().max(1.0);
                        t.clips.push(Clip { start, len, name: file.clone(), loop_len: 0.0, lyric_text: String::new(), lyric_lang: String::new(), tag: String::new(), data: ClipData::Midi(std::mem::take(&mut kit_notes)) });
                    }
                }
                // FL Studio automation clips (4th argument "map" (default) | "lanes" | "skip"; 5th: per clip [part or -1, mode or ""]):
                // mapped onto the Ling parameter they move, kept as an unassigned lane "fl:<name>" to map later, or left out
                let auto_mode = a.s(3);
                let auto_over: Vec<(f64, String)> = match a.0.get(4) {
                    Some(Val::List(l)) => l
                        .chunks(2)
                        .map(|p| (match p.first() { Some(Val::Num(x)) => *x, _ => -1.0 }, match p.get(1) { Some(Val::Str(s)) => s.clone(), _ => String::new() }))
                        .collect(),
                    _ => vec![],
                };
                let (mut mapped, mut loose) = (0usize, 0usize);
                if auto_mode != "skip" && crate::flp_import::looks_like_flp(&data) {
                    let made_range = n0..en.project.tracks.len();
                    for (k, au) in crate::flp_import::automation(&data).iter().enumerate() {
                        let (opart, omode) = auto_over.get(k).cloned().unwrap_or((-1.0, String::new()));
                        let mode = if !omode.is_empty() { omode } else if auto_mode.is_empty() { "map".to_string() } else { auto_mode.clone() };
                        if mode == "skip" {
                            continue;
                        }
                        let part = if opart >= 0.0 { Some(opart as usize) } else { guess_part(&mid.tracks, &au.target) };
                        // the track that part became (an unknown target waits on the first imported track)
                        let ti = part
                            .and_then(|p| mid.tracks.get(p))
                            .and_then(|it| made_range.clone().find(|&i| {
                                let nm = &en.project.tracks[i].name;
                                *nm == it.name || nm.starts_with(&format!("{} - ", it.name))
                            }));
                        let known = ti.is_some();
                        let Some(ti) = ti.or_else(|| made_range.clone().next()) else { continue };
                        let t = &mut en.project.tracks[ti];
                        let param = if mode == "map" && known {
                            let names: Vec<&str> = t.inst.as_ref().map(|i| i.params().into_iter().map(|(n, _)| n).collect()).unwrap_or_default();
                            crate::flp_import::map_param(&au.param, &names)
                        } else {
                            None
                        };
                        let lane_param = param.unwrap_or_else(|| format!("fl:{}", au.name));
                        if lane_param.starts_with("fl:") {
                            loose += 1;
                        } else {
                            mapped += 1;
                        }
                        let pts: Vec<(f64, f32)> = au.points.iter().map(|(b, v)| (start + b, *v)).collect();
                        match t.auto.iter_mut().find(|l| l.param == lane_param) {
                            Some(l) => l.points = pts,
                            None => t.auto.push(AutoLane { param: lane_param, points: pts }),
                        }
                    }
                }
                self.status = format!("Imported {made} track(s) from {file} ({:.0} bars)", (mid.length_beats / 4.0).ceil());
                if mapped + loose > 0 {
                    self.status += &format!(" - automation: {mapped} mapped to parameters, {loose} waiting for a parameter (Automation lanes)");
                }
                self.touch();
                ok!(made)
            }
            "daw_clip_import_midi" => {
                let mut en = e!();
                let bpm = en.project.bpm;
                match load_midi_clip(&a.s(1), bpm, a.n(2).max(0.0)) {
                    Ok(c) => {
                        let Some(t) = en.project.tracks.get_mut(a.u(0)) else { return Ok(Val::Num(-1.0)) };
                        t.clips.push(c);
                        self.touch();
                        ok!(t.clips.len() - 1)
                    }
                    Err(x) => {
                        self.status = format!("MIDI import failed: {x}");
                        Ok(Val::Num(-1.0))
                    }
                }
            }
            "daw_clip_file" => {
                // (track, clip) -> the audio file of an audio clip, "" for a MIDI clip
                let en = e!();
                let path = en.project.tracks.get(a.u(0)).and_then(|t| t.clips.get(a.u(1))).and_then(|c| if let ClipData::Audio { sample, .. } = &c.data { Some(sample.path.clone()) } else { None }).unwrap_or_default();
                ok!(path)
            }
            "daw_clip_get" => {
                let en = e!();
                let Some(c) = en.project.tracks.get(a.u(0)).and_then(|t| t.clips.get(a.u(1))) else { return Ok(Val::Num(0.0)) };
                ok!(match (a.s(2).as_str(), &c.data) {
                    ("start", _) => c.start,
                    ("len", _) => c.len,
                    ("loop", _) => c.loop_len,
                    ("end", _) => c.end(),
                    ("audio", ClipData::Audio { .. }) => 1.0,
                    ("audio", _) => 0.0,
                    ("notes", ClipData::Midi(n)) => n.len() as f64,
                    ("gain", ClipData::Audio { gain, .. }) => *gain as f64,
                    ("pitch", ClipData::Audio { pitch, .. }) => *pitch as f64,
                    ("warp", ClipData::Audio { warp, .. }) => *warp as f64,
                    ("offset", ClipData::Audio { offset_s, .. }) => *offset_s,
                    ("seconds", ClipData::Audio { sample, .. }) => sample.seconds(),
                    _ => 0.0,
                })
            }
            "daw_clip_set" => {
                let mut en = e!();
                let v = a.n(3);
                if let Some(c) = en.project.tracks.get_mut(a.u(0)).and_then(|t| t.clips.get_mut(a.u(1))) {
                    match (a.s(2).as_str(), &mut c.data) {
                        ("start", _) => c.start = v.max(0.0),
                        ("len", _) => c.len = v.max(0.25),
                        ("loop", _) => c.loop_len = if v < 0.25 { 0.0 } else { v },
                        ("gain", ClipData::Audio { gain, .. }) => *gain = v.clamp(0.0, 4.0) as f32,
                        ("pitch", ClipData::Audio { pitch, .. }) => *pitch = v.clamp(-24.0, 24.0) as f32,
                        ("warp", ClipData::Audio { warp, .. }) => *warp = if v < 20.0 { 0.0 } else { v.clamp(20.0, 400.0) as f32 },
                        ("offset", ClipData::Audio { offset_s, .. }) => *offset_s = v.max(0.0),
                        _ => {}
                    }
                }
                en.project.prepare_audio();
                self.touch();
                Ok(Val::Unit)
            }
            "daw_clip_trim_left" => {
                // (track, clip, new start beat): the clip keeps its content in place, only the start moves
                let mut en = e!();
                let spb = 60.0 / en.project.bpm;
                let Some(clip) = en.project.tracks.get_mut(a.u(0)).and_then(|t| t.clips.get_mut(a.u(1))) else { return Ok(Val::Num(0.0)) };
                let (old, end) = (clip.start, clip.end());
                let mut ns = a.n(2).clamp(0.0, (end - 0.25).max(0.0));
                if let ClipData::Audio { offset_s, .. } = &clip.data {
                    // cannot reveal audio before the start of the sample
                    let min_ns = old - *offset_s / spb;
                    ns = ns.max(min_ns);
                }
                let d = ns - old;
                if d.abs() < 1e-9 {
                    return Ok(Val::Num(0.0));
                }
                match &mut clip.data {
                    ClipData::Midi(notes) => {
                        for n in notes.iter_mut() {
                            n.start -= d;
                        }
                        if d > 0.0 {
                            notes.retain(|n| n.start + n.len > 1e-9);
                            for n in notes.iter_mut() {
                                if n.start < 0.0 {
                                    n.len += n.start;
                                    n.start = 0.0;
                                }
                            }
                        }
                    }
                    ClipData::Audio { offset_s, .. } => *offset_s = (*offset_s + d * spb).max(0.0),
                }
                clip.start = ns;
                clip.len = end - ns;
                self.touch();
                ok!(true)
            }
            "daw_clip_detect_bpm" => {
                let en = e!();
                let Some(Clip { data: ClipData::Audio { sample, .. }, .. }) = en.project.tracks.get(a.u(0)).and_then(|t| t.clips.get(a.u(1))) else {
                    return Ok(Val::Num(0.0));
                };
                let mono: Vec<f32> = sample.stereo.chunks_exact(2).map(|f| (f[0] + f[1]) * 0.5).collect();
                ok!(ling_music::analysis::bpm(&mono, sample.rate) as f64)
            }
            "daw_clip_name" => ok!(e!().project.tracks.get(a.u(0)).and_then(|t| t.clips.get(a.u(1))).map(|c| c.name.clone()).unwrap_or_default()),
            "daw_clip_set_name" => {
                if let Some(c) = e!().project.tracks.get_mut(a.u(0)).and_then(|t| t.clips.get_mut(a.u(1))) {
                    c.name = a.s(2);
                }
                self.touch();
                Ok(Val::Unit)
            }
            "daw_clip_to_track" => {
                // (t, c, newTrack) -> index of the clip on the new track, or -1 (kind mismatch)
                let mut en = e!();
                let (t, c, nt) = (a.u(0), a.u(1), a.u(2));
                if t == nt {
                    return Ok(Val::Num(c as f64));
                }
                let n = en.project.tracks.len();
                if t >= n || nt >= n || c >= en.project.tracks[t].clips.len() {
                    return Ok(Val::Num(-1.0));
                }
                let audio_clip = matches!(en.project.tracks[t].clips[c].data, ClipData::Audio { .. });
                let audio_track = matches!(en.project.tracks[nt].kind, TrackKind::Audio);
                if audio_clip != audio_track {
                    return Ok(Val::Num(-1.0));
                }
                let clip = en.project.tracks[t].clips.remove(c);
                en.project.tracks[nt].clips.push(clip);
                en.on_clips_changed(t);
                self.touch();
                ok!(en.project.tracks[nt].clips.len() - 1)
            }
            "daw_clip_remove" => {
                let mut en = e!();
                if let Some(t) = en.project.tracks.get_mut(a.u(0)) {
                    if a.u(1) < t.clips.len() {
                        t.clips.remove(a.u(1));
                    }
                }
                en.on_clips_changed(a.u(0));
                self.touch();
                Ok(Val::Unit)
            }
            "daw_clip_dup" => {
                // (t, c, new_start) -> new clip index
                let mut en = e!();
                let Some(t) = en.project.tracks.get_mut(a.u(0)) else { return Ok(Val::Num(-1.0)) };
                let Some(mut c) = t.clips.get(a.u(1)).cloned() else { return Ok(Val::Num(-1.0)) };
                c.start = a.n(2).max(0.0);
                t.clips.push(c);
                self.touch();
                ok!(t.clips.len() - 1)
            }
            "daw_clip_split" => {
                // (t, c, beat) -> index of right half or -1
                let mut en = e!();
                let spb = 60.0 / en.project.bpm;
                let Some(t) = en.project.tracks.get_mut(a.u(0)) else { return Ok(Val::Num(-1.0)) };
                let ci = a.u(1);
                let b = a.n(2);
                let Some(c) = t.clips.get(ci).cloned() else { return Ok(Val::Num(-1.0)) };
                if b <= c.start + 0.01 || b >= c.end() - 0.01 {
                    return Ok(Val::Num(-1.0));
                }
                let cut = b - c.start;
                let mut left = c.clone();
                let mut right = c.clone();
                left.len = cut;
                right.start = b;
                right.len = c.len - cut;
                match (&mut left.data, &mut right.data) {
                    (ClipData::Midi(ln), ClipData::Midi(rn)) => {
                        ln.retain(|n| n.start < cut);
                        for n in ln.iter_mut() {
                            n.len = n.len.min(cut - n.start);
                        }
                        rn.retain(|n| n.start + n.len > cut);
                        for n in rn.iter_mut() {
                            let s = n.start - cut;
                            if s < 0.0 {
                                n.len += s;
                                n.start = 0.0;
                            } else {
                                n.start = s;
                            }
                        }
                    }
                    (_, ClipData::Audio { offset_s, warp, .. }) => {
                        // warped clips advance by source beats, others by real time
                        *offset_s += if *warp > 1.0 { cut * 60.0 / *warp as f64 } else { cut * spb };
                    }
                    _ => {}
                }
                right.name = format!("{} b", c.name);
                t.clips[ci] = left;
                t.clips.push(right);
                self.touch();
                ok!(t.clips.len() - 1)
            }
            "daw_clip_peaks" => {
                // (t, c, n) -> list of n peak values 0..1 (audio) / empty (midi)
                let en = e!();
                let n = a.u(2).clamp(1, 4096);
                let Some(Clip { data: ClipData::Audio { sample, .. }, .. }) = en.project.tracks.get(a.u(0)).and_then(|t| t.clips.get(a.u(1))) else {
                    return Ok(Val::List(vec![]));
                };
                let key = (Arc::as_ptr(sample) as usize, n);
                let pk = self.peaks.entry(key).or_insert_with(|| sample.peaks(n));
                ok!(Val::List(pk.iter().map(|x| Val::Num(*x as f64)).collect()))
            }
            "daw_clip_notes" => {
                // flat [start,len,key,vel,pan,cutoff,reso] * N
                let en = e!();
                let Some(Clip { data: ClipData::Midi(n), .. }) = en.project.tracks.get(a.u(0)).and_then(|t| t.clips.get(a.u(1))) else {
                    return Ok(Val::List(vec![]));
                };
                let mut v = Vec::with_capacity(n.len() * 7);
                for x in n {
                    v.extend([x.start.into(), x.len.into(), Val::Num(x.key as f64), x.vel.into(), x.pan.into(), x.cutoff.into(), x.reso.into()]);
                }
                ok!(Val::List(v))
            }
            "daw_clip_set_notes" => {
                // (t, c, flat [start,len,key,vel,pan,cutoff,reso] * N) replaces every note of a MIDI clip at once (what a script builds)
                let Some(Val::List(l)) = a.0.get(2) else { return ok!(false) };
                let num = |v: &Val| if let Val::Num(x) = v { *x } else { 0.0 };
                let mut notes: Vec<Note> = Vec::with_capacity(l.len() / 7);
                for ch in l.chunks(7) {
                    if ch.len() < 4 {
                        break;
                    }
                    let mut n = Note::new(num(&ch[0]).max(0.0), num(&ch[1]).max(0.02), num(&ch[2]).round().clamp(0.0, 127.0) as u8, (num(&ch[3]) as f32).clamp(0.01, 1.0));
                    if ch.len() >= 7 {
                        n.pan = (num(&ch[4]) as f32).clamp(-1.0, 1.0);
                        n.cutoff = (num(&ch[5]) as f32).clamp(0.0, 1.0);
                        n.reso = (num(&ch[6]) as f32).clamp(0.0, 1.0);
                    }
                    notes.push(n);
                }
                let mut en = e!();
                let Some(Clip { data: ClipData::Midi(n), .. }) = en.project.tracks.get_mut(a.u(0)).and_then(|t| t.clips.get_mut(a.u(1))) else { return ok!(false) };
                *n = notes;
                drop(en);
                self.touch();
                ok!(true)
            }
            "daw_lyric_script" => {
                // which script a lyric is written in: latin | ipa | korean | japanese | pinyin (Han characters) | thai
                let l = crate::lyrics::detect(&a.s(0));
                ok!(match l {
                    crate::lyrics::Lang::Ipa => "ipa",
                    crate::lyrics::Lang::Korean => "korean",
                    crate::lyrics::Lang::Japanese => "japanese",
                    crate::lyrics::Lang::Pinyin => "chinese",
                    crate::lyrics::Lang::Thai => "thai",
                    _ => "latin",
                }
                .to_string())
            }
            "daw_clipboard_set" => {
                // put text on the system clipboard
                ok!(arboard::Clipboard::new().and_then(|mut c| c.set_text(a.s(0))).is_ok())
            }
            "daw_clipboard_get" => {
                // text on the system clipboard (the lyrics field cannot type Hangul / kana / Thai, but it can paste them)
                ok!(arboard::Clipboard::new().and_then(|mut c| c.get_text()).unwrap_or_default())
            }
            // ── lyrics (sung instruments) ──
            "daw_lyric_ipa" => {
                // (text, lang) -> the phrase as it will be sung: per-note IPA, notes separated by spaces ("~" = tied)
                let v = crate::lyrics::parse_phrase(&a.s(0), crate::lyrics::Lang::parse(&a.s(1)));
                ok!(v.join(" "))
            }
            "daw_lyric_apply" => {
                // (t, c, text, lang) -> number of notes that got a lyric; notes are taken in time order
                let text = a.s(2);
                let lang = a.s(3);
                let lyrics = crate::lyrics::parse_phrase(&text, crate::lyrics::Lang::parse(&lang));
                let mut en = e!();
                let Some(clip) = en.project.tracks.get_mut(a.u(0)).and_then(|t| t.clips.get_mut(a.u(1))) else { return Ok(Val::Num(0.0)) };
                let ClipData::Midi(n) = &mut clip.data else { return Ok(Val::Num(0.0)) };
                let mut order: Vec<usize> = (0..n.len()).collect();
                order.sort_by(|x, y| n[*x].start.partial_cmp(&n[*y].start).unwrap_or(std::cmp::Ordering::Equal));
                let mut given = 0usize;
                for (rank, idx) in order.iter().enumerate() {
                    match lyrics.get(rank) {
                        Some(l) => {
                            n[*idx].lyric = l.clone();
                            given += 1;
                        }
                        None => n[*idx].lyric.clear(),
                    }
                }
                clip.lyric_text = text;
                clip.lyric_lang = lang;
                self.touch();
                ok!(given)
            }
            "daw_lyric_text" => {
                // (t, c) -> [phrase as typed, language]
                let en = e!();
                let Some(c) = en.project.tracks.get(a.u(0)).and_then(|t| t.clips.get(a.u(1))) else { return ok!(Val::List(vec![])) };
                ok!(Val::List(vec![Val::Str(c.lyric_text.clone()), Val::Str(c.lyric_lang.clone())]))
            }
            "daw_note_lyrics" => {
                // (t, c) -> one string per note, in the same order as daw_clip_notes
                let en = e!();
                let Some(Clip { data: ClipData::Midi(n), .. }) = en.project.tracks.get(a.u(0)).and_then(|t| t.clips.get(a.u(1))) else { return ok!(Val::List(vec![])) };
                ok!(Val::List(n.iter().map(|x| Val::Str(x.lyric.clone())).collect()))
            }
            "daw_note_lyric_set" => {
                // (t, c, idx, ipa text) set one note's stored lyric directly
                let mut en = e!();
                if let Some(Clip { data: ClipData::Midi(n), .. }) = en.project.tracks.get_mut(a.u(0)).and_then(|t| t.clips.get_mut(a.u(1))) {
                    if let Some(x) = n.get_mut(a.u(2)) {
                        x.lyric = a.s(3);
                    }
                }
                self.touch();
                Ok(Val::Unit)
            }
            "daw_note_add" => {
                // (t, c, start, len, key, vel) -> idx
                let mut en = e!();
                let Some(Clip { data: ClipData::Midi(n), .. }) = en.project.tracks.get_mut(a.u(0)).and_then(|t| t.clips.get_mut(a.u(1))) else {
                    return Ok(Val::Num(-1.0));
                };
                n.push(Note::new(a.n(2).max(0.0), a.n(3).max(0.03), a.n(4).clamp(0.0, 127.0) as u8, a.n(5).clamp(0.01, 1.0) as f32));
                self.touch();
                ok!(n.len() - 1)
            }
            "daw_note_set" => {
                // (t, c, idx, field, v)
                let mut en = e!();
                if let Some(Clip { data: ClipData::Midi(n), .. }) = en.project.tracks.get_mut(a.u(0)).and_then(|t| t.clips.get_mut(a.u(1))) {
                    if let Some(x) = n.get_mut(a.u(2)) {
                        let v = a.n(4);
                        match a.s(3).as_str() {
                            "start" => x.start = v.max(0.0),
                            "len" => x.len = v.max(0.03),
                            "key" => x.key = v.clamp(0.0, 127.0) as u8,
                            "vel" => x.vel = v.clamp(0.01, 1.0) as f32,
                            "pan" => x.pan = v.clamp(-1.0, 1.0) as f32,
                            "cutoff" => x.cutoff = v.clamp(0.0, 1.0) as f32,
                            "reso" => x.reso = v.clamp(0.0, 1.0) as f32,
                            _ => {}
                        }
                    }
                }
                self.touch();
                Ok(Val::Unit)
            }
            "daw_notes_apply" => {
                // (t, c, mode, selected indices, origin [start, key]* by note index, origin lengths by note index, p1, p2)
                // moves / resizes / stretches a whole selection in ONE engine call (a call per note made big selections crawl)
                //   "move":    p1 = beats, p2 = semitones      "resize": p1 = extra beats, p2 = shortest length
                //   "stretch": p1 = anchor beat, p2 = factor
                let nums = |v: Option<&Val>| -> Vec<f64> {
                    match v {
                        Some(Val::List(l)) => l.iter().map(|x| if let Val::Num(n) = x { *n } else { 0.0 }).collect(),
                        _ => vec![],
                    }
                };
                let (sel, orig, olen) = (nums(a.0.get(3)), nums(a.0.get(4)), nums(a.0.get(5)));
                let (mode, p1, p2) = (a.s(2), a.n(6), a.n(7));
                if !p1.is_finite() || !p2.is_finite() {
                    return Ok(Val::Unit);
                }
                let mut en = e!();
                if let Some(Clip { data: ClipData::Midi(n), .. }) = en.project.tracks.get_mut(a.u(0)).and_then(|t| t.clips.get_mut(a.u(1))) {
                    for si in sel {
                        let i = si.max(0.0) as usize;
                        let Some(x) = n.get_mut(i) else { continue };
                        let o_start = orig.get(i * 2).copied();
                        match mode.as_str() {
                            "move" => {
                                if let (Some(os), Some(ok)) = (o_start, orig.get(i * 2 + 1)) {
                                    x.start = (os + p1).max(0.0);
                                    x.key = (ok + p2).clamp(0.0, 127.0) as u8;
                                }
                            }
                            "resize" => {
                                if let Some(ol) = olen.get(i) {
                                    x.len = (ol + p1).max(p2.max(0.03));
                                }
                            }
                            "stretch" => {
                                if let (Some(os), Some(ol)) = (o_start, olen.get(i)) {
                                    x.start = (p1 + (os - p1) * p2).max(0.0);
                                    x.len = (ol * p2).max(0.06);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                self.touch();
                Ok(Val::Unit)
            }
            "daw_note_remove" => {
                let mut en = e!();
                if let Some(Clip { data: ClipData::Midi(n), .. }) = en.project.tracks.get_mut(a.u(0)).and_then(|t| t.clips.get_mut(a.u(1))) {
                    if a.u(2) < n.len() {
                        n.remove(a.u(2));
                    }
                }
                self.touch();
                Ok(Val::Unit)
            }
            // ── instrument ──
            "daw_inst_get" => {
                // (track, name) -> value, or -1 when the instrument has no such parameter
                let en = e!();
                ok!(en.project.tracks.get(a.u(0)).and_then(|t| t.inst.as_ref()).and_then(|i| i.get_param(&a.s(1))).map(|v| v as f64).unwrap_or(-1.0))
            }
            "daw_inst_get_many" => {
                // (track, [names]) -> [values] in one go (-1 for unknown names): one engine lock instead of one per knob
                let en = e!();
                let inst = en.project.tracks.get(a.u(0)).and_then(|t| t.inst.as_ref());
                let names: Vec<String> = match a.0.get(1) {
                    Some(Val::List(l)) => l.iter().map(|v| if let Val::Str(s) = v { s.clone() } else { String::new() }).collect(),
                    _ => vec![],
                };
                ok!(Val::List(names.iter().map(|n| Val::Num(inst.and_then(|i| i.get_param(n)).map(|v| v as f64).unwrap_or(-1.0))).collect()))
            }
            "daw_inst_plot" => {
                // (track, "wave"|"filter", points) -> list of values, [] when the instrument has no such curve
                let en = e!();
                let v = en.project.tracks.get(a.u(0)).and_then(|t| t.inst.as_ref()).map(|i| i.plot(&a.s(1), a.u(2))).unwrap_or_default();
                ok!(Val::List(v.into_iter().map(|x| Val::Num(x as f64)).collect()))
            }
            "daw_inst_params" => {
                let en = e!();
                let mut v = vec![];
                if let Some(i) = en.project.tracks.get(a.u(0)).and_then(|t| t.inst.as_ref()) {
                    for (n, x) in i.params() {
                        v.push(Val::Str(n.to_string()));
                        v.push(Val::Num(x as f64));
                    }
                }
                ok!(Val::List(v))
            }
            "daw_inst_set" => {
                let mut en = e!();
                if let Some(i) = en.project.tracks.get_mut(a.u(0)).and_then(|t| t.inst.as_mut()) {
                    i.set_param(&a.s(1), a.n(2) as f32);
                }
                en.record_auto(Some(a.u(0)), &a.s(1), a.n(2) as f32);
                self.touch();
                Ok(Val::Unit)
            }
            "daw_inst_random" => {
                // (track, seed) -> 1 when the instrument made itself a new random sound (the Growl does)
                let mut en = e!();
                let done = en.project.tracks.get_mut(a.u(0)).and_then(|t| t.inst.as_mut()).map(|i| i.randomize(a.n(1).abs() as u32)).unwrap_or(false);
                drop(en);
                if done {
                    self.touch();
                }
                ok!(done)
            }
            "daw_inst_presets" => {
                let en = e!();
                ok!(Val::List(en.project.tracks.get(a.u(0)).and_then(|t| t.inst.as_ref()).map(|i| i.presets().iter().map(|s| Val::Str(s.to_string())).collect()).unwrap_or_default()))
            }
            "daw_inst_categories" => {
                // (track) -> [preset, category, ...] for instruments whose presets are grouped (Prism); [] otherwise
                let en = e!();
                ok!(Val::List(en.project.tracks.get(a.u(0)).and_then(|t| t.inst.as_ref()).map(|i| i.preset_categories().into_iter().flat_map(|(n, c)| [Val::Str(n), Val::Str(c)]).collect()).unwrap_or_default()))
            }
            "daw_inst_preset" => {
                if let Some(i) = e!().project.tracks.get_mut(a.u(0)).and_then(|t| t.inst.as_mut()) {
                    i.load_preset(&a.s(1));
                }
                self.touch();
                Ok(Val::Unit)
            }
            "daw_inst_parts" => {
                let en = e!();
                ok!(Val::List(en.project.tracks.get(a.u(0)).and_then(|t| t.inst.as_ref()).map(|i| i.parts().iter().map(|s| Val::Str(s.to_string())).collect()).unwrap_or_default()))
            }
            "daw_inst_part" => ok!(e!().project.tracks.get(a.u(0)).and_then(|t| t.inst.as_ref()).map(|i| i.part()).unwrap_or(0)),
            "daw_inst_select_part" => {
                if let Some(i) = e!().project.tracks.get_mut(a.u(0)).and_then(|t| t.inst.as_mut()) {
                    i.select_part(a.u(1));
                }
                Ok(Val::Unit)
            }
            "daw_track_set_kind" => {
                // (track, "synth"|"drums"|"growl"|"voice"|"sf2", soundfont path for sf2) -> 1 on success.
                // Swaps the track's instrument for a default one of that kind; clips, effects, mixer and automation stay.
                let kind = a.s(1);
                let mut inst = if kind == "sf2" { crate::dsp::make_instrument_file("sf2", sr, &a.s(2)) } else { crate::dsp::make_instrument(&kind, sr) };
                let mut en = e!();
                let bpm = en.project.bpm as f32;
                let Some(t) = en.project.tracks.get_mut(a.u(0)) else { return Ok(Val::Num(0.0)) };
                if matches!(t.kind, TrackKind::Audio) || inst.is_none() {
                    return Ok(Val::Num(0.0));
                }
                if let Some(i) = inst.as_mut() {
                    if kind == "growl" {
                        i.set_param("tempo", ((bpm - 60.0) / 140.0).clamp(0.0, 1.0));
                    }
                }
                t.inst = inst;
                t.inst_kind = kind.clone();
                t.patch = if kind == "sf2" { a.s(2) } else { String::new() };
                drop(en);
                self.touch();
                ok!(true)
            }
            "daw_inst_load" => {
                // load a .ling patch onto an existing track (replaces instrument + adds its fx)
                let path = a.s(1);
                let text = std::fs::read_to_string(&path).map_err(|x| format!("{path}: {x}"))?;
                let p = patch::parse(&text);
                let mut en = e!();
                let Some(t) = en.project.tracks.get_mut(a.u(0)) else { return Ok(Val::Num(0.0)) };
                let Some(inst) = patch::build(&p, sr) else { return Ok(Val::Num(0.0)) };
                t.inst = Some(inst);
                t.inst_kind = p.kind.clone();
                t.patch = path;
                // third argument 1 = keep the track's own name (a preset chooser should not rename the track)
                if !p.name.is_empty() && a.n(2) < 0.5 {
                    t.name = p.name.clone();
                }
                t.effects = project::patch_effects(&p, sr);
                self.touch();
                ok!(true)
            }
            "daw_inst_save" => {
                let en = e!();
                let Some(t) = en.project.tracks.get(a.u(0)) else { return Ok(Val::Num(0.0)) };
                let Some(i) = t.inst.as_ref() else { return Ok(Val::Num(0.0)) };
                let fx: Vec<patch::FxSpec> = t
                    .effects
                    .iter()
                    .map(|f| patch::FxSpec { name: f.name.clone(), params: f.params.iter().map(|(n, v)| (n.to_string(), *v)).collect() })
                    .collect();
                let text = patch::dump(&t.inst_kind, &t.name, &i.state(), &fx);
                std::fs::write(a.s(1), text).map_err(|x| x.to_string())?;
                ok!(true)
            }
            "daw_live" => {
                // (track, key, vel, on)
                let mut en = e!();
                en.live_note(a.u(0), a.n(1).clamp(0.0, 127.0) as u8, a.n(2).clamp(0.01, 1.0) as f32, a.n(3) > 0.5);
                Ok(Val::Unit)
            }
            // ── effects ──
            "daw_fx_types" => ok!(Val::List(EFFECT_NAMES.iter().map(|s| Val::Str(s.to_string())).collect())),
            "daw_fx_command" => {
                // (target, effect index, command, [numbers], text) -> [numbers]: the effect's own edit commands (the Ringstorm's pedal graph)
                let args: Vec<f64> = match a.0.get(3) {
                    Some(Val::List(l)) => l.iter().map(|x| if let Val::Num(n) = x { *n } else { 0.0 }).collect(),
                    _ => vec![],
                };
                let mut en = e!();
                let r = fx_target(&mut en, a.n(0) as i64).and_then(|v| v.get_mut(a.u(1)).map(|s| s.fx.command(&a.s(2), &args, &a.s(4)))).unwrap_or_default();
                drop(en);
                if !matches!(a.s(2).as_str(), "meter" | "get_param" | "count" | "kinds") {
                    self.touch();
                }
                ok!(Val::List(r.into_iter().map(Val::Num).collect()))
            }
            "daw_fx_info" => {
                // (target, effect index, what) -> text ("config", "nodes", "kinds", "presets" ...)
                let mut en = e!();
                ok!(fx_target(&mut en, a.n(0) as i64).and_then(|v| v.get(a.u(1)).map(|s| s.fx.info(&a.s(2)))).unwrap_or_default())
            }
            "daw_rig_view" => {
                // (target, effect index) -> one list per pedal of the Ringstorm: [kind, input (-1 = the rig input), out 0/1, bypass 0/1, level, [name, value, ...], peak]
                let mut en = e!();
                let Some(slot) = fx_target(&mut en, a.n(0) as i64).and_then(|v| v.get_mut(a.u(1))) else { return ok!(Val::List(vec![])) };
                let text = slot.fx.info("nodes");
                let nodes: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap_or_default();
                let mut out = vec![];
                for (i, n) in nodes.iter().enumerate() {
                    let mut params = vec![];
                    if let Some(o) = n["params"].as_object() {
                        for (k, v) in o {
                            params.push(Val::Str(k.clone()));
                            params.push(Val::Num(v.as_f64().unwrap_or(0.0)));
                        }
                    }
                    let peak = slot.fx.command("meter", &[i as f64], "").first().copied().unwrap_or(0.0);
                    out.push(Val::List(vec![
                        Val::Str(n["kind"].as_str().unwrap_or("").to_string()),
                        Val::Num(n["input"].as_f64().unwrap_or(-1.0)),
                        Val::Num(if n["out"].as_bool().unwrap_or(false) { 1.0 } else { 0.0 }),
                        Val::Num(if n["bypass"].as_bool().unwrap_or(false) { 1.0 } else { 0.0 }),
                        Val::Num(n["level"].as_f64().unwrap_or(1.0)),
                        Val::List(params),
                        Val::Num(peak),
                    ]));
                }
                ok!(Val::List(out))
            }
            "daw_rig_catalog" => {
                // (target, effect index) -> [kind, [param name, default, ...]] for every pedal kind
                let mut en = e!();
                let Some(slot) = fx_target(&mut en, a.n(0) as i64).and_then(|v| v.get(a.u(1))) else { return ok!(Val::List(vec![])) };
                let kinds: Vec<serde_json::Value> = serde_json::from_str(&slot.fx.info("kinds")).unwrap_or_default();
                let mut out = vec![];
                for k in &kinds {
                    let mut ps = vec![];
                    if let Some(arr) = k["params"].as_array() {
                        for p in arr {
                            ps.push(Val::Str(p["name"].as_str().unwrap_or("").to_string()));
                            ps.push(Val::Num(p["def"].as_f64().unwrap_or(0.5)));
                        }
                    }
                    out.push(Val::List(vec![Val::Str(k["kind"].as_str().unwrap_or("").to_string()), Val::List(ps)]));
                }
                ok!(Val::List(out))
            }
            "daw_rig_presets" => {
                let mut en = e!();
                let Some(slot) = fx_target(&mut en, a.n(0) as i64).and_then(|v| v.get(a.u(1))) else { return ok!(Val::List(vec![])) };
                let names: Vec<String> = serde_json::from_str(&slot.fx.info("presets")).unwrap_or_default();
                ok!(Val::List(names.into_iter().map(Val::Str).collect()))
            }
            "daw_fx_count" => {
                let mut en = e!();
                ok!(fx_target(&mut en, a.n(0) as i64).map(|v| v.len()).unwrap_or(0))
            }
            "daw_fx_name" => {
                let mut en = e!();
                ok!(fx_target(&mut en, a.n(0) as i64).and_then(|v| v.get(a.u(1)).map(|s| s.name.clone())).unwrap_or_default())
            }
            "daw_fx_bypassed" => {
                let mut en = e!();
                ok!(fx_target(&mut en, a.n(0) as i64).and_then(|v| v.get(a.u(1)).map(|s| s.bypass)).unwrap_or(false))
            }
            "daw_fx_add" => {
                let mut en = e!();
                if let Some(s) = crate::model::EffectSlot::new(&a.s(1), sr) {
                    if let Some(v) = fx_target(&mut en, a.n(0) as i64) {
                        v.push(s);
                        self.touch();
                        ok!(v.len() - 1)
                    }
                }
                Ok(Val::Num(-1.0))
            }
            "daw_fx_remove" => {
                let mut en = e!();
                if let Some(v) = fx_target(&mut en, a.n(0) as i64) {
                    if a.u(1) < v.len() {
                        v.remove(a.u(1));
                    }
                }
                self.touch();
                Ok(Val::Unit)
            }
            "daw_fx_move" => {
                let mut en = e!();
                if let Some(v) = fx_target(&mut en, a.n(0) as i64) {
                    let i = a.u(1);
                    if i < v.len() {
                        let j = (i as i64 + a.n(2) as i64).clamp(0, v.len() as i64 - 1) as usize;
                        let s = v.remove(i);
                        v.insert(j, s);
                    }
                }
                self.touch();
                Ok(Val::Unit)
            }
            "daw_fx_bypass" => {
                let mut en = e!();
                if let Some(s) = fx_target(&mut en, a.n(0) as i64).and_then(|v| v.get_mut(a.u(1))) {
                    s.bypass = a.n(2) > 0.5;
                }
                self.touch();
                Ok(Val::Unit)
            }
            "daw_fx_params" => {
                let mut en = e!();
                let mut out = vec![];
                if let Some(s) = fx_target(&mut en, a.n(0) as i64).and_then(|v| v.get(a.u(1))) {
                    for (n, x) in &s.params {
                        out.push(Val::Str(n.to_string()));
                        out.push(Val::Num(*x as f64));
                    }
                }
                ok!(Val::List(out))
            }
            "daw_fx_view" => {
                // (target, idx) -> live values for the effect's display (0..1 each), [] when it has none
                let mut en = e!();
                let v = fx_target(&mut en, a.n(0) as i64).and_then(|v| v.get(a.u(1))).map(|s| s.fx.view()).unwrap_or_default();
                ok!(Val::List(v.into_iter().map(|x| Val::Num(x as f64)).collect()))
            }
            "daw_fx_load" => {
                // (target, idx, path) -> 1 when the effect took the file (the growl's spectral sample; "" = back to the built-in one)
                let mut en = e!();
                let ok_ = fx_target(&mut en, a.n(0) as i64).and_then(|v| v.get_mut(a.u(1))).map(|s| s.fx.load_source(&a.s(2))).unwrap_or(false);
                drop(en);
                if ok_ {
                    self.touch();
                }
                ok!(ok_)
            }
            "daw_fx_source" => {
                // (target, idx) -> the file the effect works from, "" for none
                let mut en = e!();
                let v = fx_target(&mut en, a.n(0) as i64).and_then(|v| v.get(a.u(1))).map(|s| s.fx.source()).unwrap_or_default();
                ok!(v)
            }
            "daw_fx_presets" => {
                // (effect name) -> preset names
                ok!(Val::List(crate::dsp::fx::presets(&a.s(0)).iter().map(|p| Val::Str(p.0.to_string())).collect()))
            }
            "daw_fx_preset" => {
                // (target, idx, preset index): every parameter goes back to its default, then the preset's overrides are applied
                let mut en = e!();
                let mut done = 0.0;
                if let Some(s) = fx_target(&mut en, a.n(0) as i64).and_then(|v| v.get_mut(a.u(1))) {
                    let name = s.name.clone();
                    if let Some(pr) = crate::dsp::fx::presets(&name).get(a.u(2)) {
                        for (i, (_, d)) in crate::dsp::fx::defaults(&name).iter().enumerate() {
                            s.set(i, *d);
                        }
                        for (pn, v) in pr.1 {
                            if let Some(i) = s.params.iter().position(|x| x.0 == *pn) {
                                s.set(i, *v);
                            }
                        }
                        done = 1.0;
                    }
                }
                drop(en);
                self.touch();
                ok!(done)
            }
            "daw_fx_reset" => {
                // (target, idx): every parameter back to its default
                let mut en = e!();
                if let Some(s) = fx_target(&mut en, a.n(0) as i64).and_then(|v| v.get_mut(a.u(1))) {
                    let name = s.name.clone();
                    for (i, (_, d)) in crate::dsp::fx::defaults(&name).iter().enumerate() {
                        s.set(i, *d);
                    }
                }
                drop(en);
                self.touch();
                Ok(Val::Unit)
            }
            "daw_fx_random" => {
                // (target, idx, seed): new values near the defaults, never touching mix / level / width
                let mut en = e!();
                if let Some(s) = fx_target(&mut en, a.n(0) as i64).and_then(|v| v.get_mut(a.u(1))) {
                    let name = s.name.clone();
                    let mut rng = (a.n(2) as u32).wrapping_mul(2654435761).wrapping_add(0x9e3779b9) | 1;
                    for (i, (pn, d)) in crate::dsp::fx::defaults(&name).iter().enumerate() {
                        rng ^= rng << 13;
                        rng ^= rng >> 17;
                        rng ^= rng << 5;
                        if matches!(*pn, "mix" | "level" | "width") {
                            continue;
                        }
                        let r = (rng & 0xffff) as f32 / 65535.0;
                        s.set(i, d + (r - d) * 0.85);
                    }
                }
                drop(en);
                self.touch();
                Ok(Val::Unit)
            }
            "daw_fx_set" => {
                // (target, idx, param_index, v)
                let mut en = e!();
                if let Some(s) = fx_target(&mut en, a.n(0) as i64).and_then(|v| v.get_mut(a.u(1))) {
                    s.set(a.u(2), a.n(3) as f32);
                }
                let pname = format!("fx{}.{}", a.u(1), a.u(2));
                en.record_auto(if a.n(0) >= 0.0 { Some(a.u(0)) } else { None }, &pname, a.n(3) as f32);
                self.touch();
                Ok(Val::Unit)
            }
            // ── files / project ──
            "daw_project_save" => {
                let path = if a.s(0).is_empty() { self.path.clone() } else { a.s(0) };
                if path.is_empty() {
                    return Ok(Val::Num(0.0));
                }
                self.refresh_style();
                let en = e!();
                match project::save_with_media(&en, &path, true) {
                    Ok(()) => {
                        drop(en);
                        self.path = path.clone();
                        self.dirty = false;
                        self.status = format!("Saved {path}");
                        ok!(true)
                    }
                    Err(x) => {
                        self.status = format!("Save failed: {x}");
                        Ok(Val::Num(0.0))
                    }
                }
            }
            "daw_project_load" => {
                self.ab.clear();
                let path = a.s(0);
                match project::read(&path) {
                    Ok(doc) => {
                        let base = std::path::Path::new(&path).parent().map(|p| p.to_path_buf());
                        project::preload(&doc, base.as_deref(), &mut self.samples);
                        let mut en = e!();
                        project::apply_doc(&mut en, &doc, base.as_deref(), &mut self.samples);
                        drop(en);
                        self.path = path.clone();
                        self.undo.clear();
                        self.redo.clear();
                        self.dirty = false;
                        self.last_snap = self.snap();
                        self.status = format!("Opened {path}");
                        ok!(true)
                    }
                    Err(x) => {
                        self.status = format!("Open failed: {x}");
                        Ok(Val::Num(0.0))
                    }
                }
            }
            "daw_undo_push" => {
                let s = self.snap();
                if s != self.last_snap {
                    let prev = std::mem::replace(&mut self.last_snap, s);
                    self.undo.push(prev);
                    if self.undo.len() > 100 {
                        self.undo.remove(0);
                    }
                    self.redo.clear();
                }
                Ok(Val::Unit)
            }
            "daw_can_undo" => ok!(!self.undo.is_empty()),
            "daw_can_redo" => ok!(!self.redo.is_empty()),
            "daw_undo" => {
                // commit any pending edits first so redo returns to the present
                let now = self.snap();
                if now != self.last_snap {
                    let prev = std::mem::replace(&mut self.last_snap, now);
                    self.undo.push(prev);
                }
                while let Some(s) = self.undo.pop() {
                    if self.restore(&s) {
                        self.redo.push(self.last_snap.clone());
                        self.last_snap = s;
                        self.touch();
                        break;
                    }
                }
                Ok(Val::Unit)
            }
            "daw_redo" => {
                while let Some(s) = self.redo.pop() {
                    if self.restore(&s) {
                        self.undo.push(self.last_snap.clone());
                        self.last_snap = s;
                        self.touch();
                        break;
                    }
                }
                Ok(Val::Unit)
            }
            "daw_tempo_points" => {
                let en = e!();
                ok!(Val::List(en.project.tempo_map.iter().flat_map(|(b, v)| [Val::Num(*b), Val::Num(*v)]).collect()))
            }
            "daw_tempo_clear" => {
                e!().project.tempo_map.clear();
                self.touch();
                Ok(Val::Unit)
            }
            "daw_tempo_add" => {
                // (beat, bpm): add or replace a tempo change
                let mut en = e!();
                let (b, v) = (a.n(0).max(0.0), a.n(1).clamp(20.0, 400.0));
                let m = &mut en.project.tempo_map;
                match m.iter_mut().find(|(pb, _)| (*pb - b).abs() < 1e-6) {
                    Some(x) => x.1 = v,
                    None => {
                        m.push((b, v));
                        m.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
                    }
                }
                self.touch();
                Ok(Val::Unit)
            }
            "daw_bpm_at" => ok!(e!().project.bpm_at(a.n(0))),
            "daw_secs_at" => ok!(e!().project.beats_to_secs(0.0, a.n(0).max(0.0))),
            // A/B compare of an instrument's settings: returns the side now active (0 = A, 1 = B)
            "daw_inst_ab" => {
                let ti = a.u(0);
                let mut en = e!();
                let Some(inst) = en.project.tracks.get_mut(ti).and_then(|t| t.inst.as_mut()) else { return Ok(Val::Num(-1.0)) };
                let now = inst.state();
                let entry = self.ab.entry(ti).or_insert_with(|| (now.clone(), None, 0));
                // park the current settings in the active slot, then switch
                if entry.2 == 0 {
                    entry.0 = now.clone();
                } else {
                    entry.1 = Some(now.clone());
                }
                let next = 1 - entry.2;
                let target = if next == 1 { entry.1.clone().unwrap_or(now.clone()) } else { entry.0.clone() };
                entry.2 = next;
                inst.load_state(&target);
                drop(en);
                self.touch();
                ok!(next as f64)
            }
            "daw_inst_ab_side" => ok!(self.ab.get(&a.u(0)).map(|e| e.2 as f64 + if e.1.is_some() { 0.0 } else { 0.0 }).unwrap_or(-1.0)),
            // ── soundfonts ──
            "daw_gm_sf2_set" => {
                // (path or "") -> number of presets, or -1 when it cannot be loaded
                let p = abs_path(&a.s(0));
                if p.is_empty() {
                    self.gm_sf2.clear();
                    ok!(0usize)
                }
                match crate::dsp::sf2::load(&p) {
                    Ok(sf) => {
                        self.gm_sf2 = p;
                        self.status = format!("Soundfont loaded: {} ({} presets)", sf.name, sf.presets.len());
                        ok!(sf.presets.len())
                    }
                    Err(x) => {
                        self.status = format!("Soundfont failed: {x}");
                        Ok(Val::Num(-1.0))
                    }
                }
            }
            "daw_gm_sf2_get" => ok!(self.gm_sf2.clone()),
            "daw_sf_presets" => {
                let en = e!();
                ok!(Val::List(en.project.tracks.get(a.u(0)).and_then(|t| t.inst.as_ref()).map(|i| i.preset_names().into_iter().map(Val::Str).collect()).unwrap_or_default()))
            }
            "daw_sf_preset" => ok!(e!().project.tracks.get(a.u(0)).and_then(|t| t.inst.as_ref()).map(|i| i.preset_index()).unwrap_or(0)),
            "daw_sf_select" => {
                if let Some(i) = e!().project.tracks.get_mut(a.u(0)).and_then(|t| t.inst.as_mut()) {
                    i.select_preset_index(a.u(1));
                }
                self.touch();
                Ok(Val::Unit)
            }
            "daw_limiter_get" => ok!(e!().project.limiter),
            "daw_limiter_set" => {
                e!().project.limiter = a.n(0) > 0.5;
                self.touch();
                Ok(Val::Unit)
            }
            // deepest gain reduction (dB, >= 0) since the last call
            "daw_limiter_gr" => {
                let mut en = e!();
                let g = std::mem::replace(&mut en.lim_min, 1.0).clamp(1e-4, 1.0);
                ok!(-20.0 * (g as f64).log10())
            }
            "daw_test_tone" => {
                e!().start_test_tone();
                self.status = format!("Test tone: left, then right ({})", self.out.device);
                Ok(Val::Unit)
            }
            "daw_render_start" => {
                // (path, start_beat, end_beat, tail_s, bits, format, ogg quality) -> started?
                if self.render.as_ref().map(|j| j.result.lock().unwrap_or_else(|p| p.into_inner()).is_none()).unwrap_or(false) {
                    return Ok(Val::Num(0.0)); // one render at a time
                }
                let mut doc = {
                    let en = e!();
                    project::to_doc(&en)
                };
                // optional 10th argument: tracks to leave out of the mix ("1,4"), e.g. the vocal for a karaoke backing track
                for i in a.s(9).split(',').filter_map(|x| x.trim().parse::<usize>().ok()) {
                    if let Some(t) = doc.tracks.get_mut(i) {
                        t.muted = true;
                        t.solo = false;
                    }
                }
                let base = std::path::Path::new(&self.path).parent().map(|p| p.to_path_buf());
                let mut samples = self.samples.clone();
                let (path, start, end0, tail) = (a.s(0), a.n(1), a.n(2), a.n(3).max(0.0));
                let bits = if a.n(4) > 0.5 { a.n(4) as u16 } else { 16 };
                let fmt = if a.s(5).is_empty() { if path.ends_with(".flac") { "flac".to_string() } else if path.ends_with(".ogg") { "ogg".to_string() } else { "wav".to_string() } } else { a.s(5) };
                let q = if a.0.len() > 6 { a.n(6) as f32 } else { 0.5 };
                let job = RenderJob { progress: Arc::new(AtomicU32::new(0)), cancel: Arc::new(AtomicBool::new(false)), result: Arc::new(std::sync::Mutex::new(None)), started: std::time::Instant::now() };
                let (pr, cn, res) = (job.progress.clone(), job.cancel.clone(), job.result.clone());
                let hint = |x: &str| -> String {
                    // Windows Controlled Folder Access reports blocked writes as "file not found"
                    if cfg!(windows) && (x.contains("find") || x.contains("denied") || x.contains("not found")) {
                        format!("{x} - if the folder exists, Windows Security > Ransomware protection may be blocking ling.exe (allow it under Controlled folder access)")
                    } else {
                        x.to_string()
                    }
                };
                let sr = self.out.sr;
                let bundle = a.n(8) > 0.5;
                let stems = a.n(7) > 0.5 || bundle;
                std::thread::Builder::new()
                    .name("ling-render".into())
                    .spawn(move || {
                        let make = |samples: &mut HashMap<String, Arc<Sample>>| {
                            let mut eng = Engine::new(sr);
                            project::apply_doc(&mut eng, &doc, base.as_deref(), samples);
                            eng
                        };
                        let write = |out: &[f32], p: &str| -> Result<String, String> {
                            if let Some(parent) = std::path::Path::new(p).parent() {
                                if !parent.as_os_str().is_empty() {
                                    let _ = std::fs::create_dir_all(parent);
                                }
                            }
                            crate::export::write_audio(p, out, sr as u32, &fmt, bits, q).map(|_| p.to_string()).map_err(|x| hint(&x))
                        };
                        if !stems {
                            let mut eng = make(&mut samples);
                            let end = if end0 > start { end0 } else { eng.project.end_beat() };
                            let Some(out) = eng.bounce_range_progress(start, end, tail, &pr, &cn, 0, 980) else {
                                *res.lock().unwrap_or_else(|p| p.into_inner()) = Some(Err("cancelled".into()));
                                return;
                            };
                            // the encoder has no progress hook: the bar waits at 98 % while it runs
                            let r = write(&out, &path);
                            pr.store(1000, Ordering::Relaxed);
                            *res.lock().unwrap_or_else(|p| p.into_inner()) = Some(r);
                            return;
                        }
                        // stems: one file per track with content, each rendered alone through its own effects
                        let probe = make(&mut samples);
                        let end = if end0 > start { end0 } else { probe.project.end_beat() };
                        let todo: Vec<usize> = (0..probe.project.tracks.len()).filter(|i| !probe.project.tracks[*i].clips.is_empty()).collect();
                        let names: Vec<String> = probe.project.tracks.iter().map(|t| t.name.clone()).collect();
                        drop(probe);
                        if todo.is_empty() {
                            *res.lock().unwrap_or_else(|p| p.into_inner()) = Some(Err("no tracks with clips to render".into()));
                            return;
                        }
                        let pth = std::path::Path::new(&path);
                        let stem = pth.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "stem".into());
                        // a bundle gets its own folder (next to the chosen file) with the stems in audio/
                        let root = pth.parent().map(|d| d.to_path_buf()).unwrap_or_default().join(format!("{stem} bundle"));
                        let dir = if bundle { root.join("audio") } else { pth.parent().map(|d| d.to_path_buf()).unwrap_or_default() };
                        let ext = pth.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| fmt.clone());
                        let n = todo.len();
                        // every stem is its own render, so they run side by side: one worker per core, each taking the next stem
                        let cache = samples.clone();
                        let next = std::sync::atomic::AtomicUsize::new(0);
                        let progress: Vec<std::sync::atomic::AtomicU32> = (0..n).map(|_| std::sync::atomic::AtomicU32::new(0)).collect();
                        let failure: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
                        let workers = std::thread::available_parallelism().map(|c| c.get()).unwrap_or(1).min(n).max(1);
                        let doc_ref = &doc;
                        let base_ref = &base;
                        let (names_ref, todo_ref, dir_ref, stem_ref, ext_ref, write_ref) = (&names, &todo, &dir, &stem, &ext, &write);
                        std::thread::scope(|sc| {
                            for _ in 0..workers {
                                sc.spawn(|| loop {
                                    let k = next.fetch_add(1, Ordering::Relaxed);
                                    if k >= n || cn.load(Ordering::Relaxed) || failure.lock().unwrap_or_else(|p| p.into_inner()).is_some() {
                                        break;
                                    }
                                    let ti = todo_ref[k];
                                    let mut local = cache.clone();
                                    let mut eng = Engine::new(sr);
                                    project::apply_doc(&mut eng, doc_ref, base_ref.as_deref(), &mut local);
                                    // tracks that cannot reach this stem render nothing (a sidechain key still needs its trigger)
                                    let keys: Vec<usize> = eng.project.routes.iter().filter(|r| r.to == ti && matches!(r.kind, RouteKind::Sidechain | RouteKind::Duck)).map(|r| r.from).collect();
                                    for (j, t) in eng.project.tracks.iter_mut().enumerate() {
                                        t.mute = j != ti;
                                        t.solo = false;
                                        if j != ti && !keys.contains(&j) {
                                            t.clips.clear();
                                            t.session.clear();
                                        }
                                    }
                                    eng.project.master_effects.clear();
                                    eng.project.limiter = false;
                                    let Some(out) = eng.bounce_range_progress(start, end, tail, &progress[k], &cn, 0, 1000) else { break };
                                    let safe: String = names_ref[ti].chars().map(|c| if c.is_alphanumeric() || c == '-' || c == ' ' { c } else { '_' }).collect();
                                    let file = dir_ref.join(format!("{stem_ref}_{:02}_{}.{ext_ref}", ti + 1, safe.trim()));
                                    if let Err(x) = write_ref(&out, &file.to_string_lossy()) {
                                        *failure.lock().unwrap_or_else(|p| p.into_inner()) = Some(x);
                                        break;
                                    }
                                    progress[k].store(1000, Ordering::Relaxed);
                                });
                            }
                            // the bar follows the average of all stems while they render
                            while next.load(Ordering::Relaxed) < n + workers && progress.iter().any(|p| p.load(Ordering::Relaxed) < 1000) && !cn.load(Ordering::Relaxed) && failure.lock().unwrap_or_else(|p| p.into_inner()).is_none() {
                                let total: u32 = progress.iter().map(|p| p.load(Ordering::Relaxed)).sum();
                                pr.store(total * 980 / (n as u32 * 1000).max(1), Ordering::Relaxed);
                                std::thread::sleep(std::time::Duration::from_millis(40));
                            }
                        });
                        if cn.load(Ordering::Relaxed) {
                            *res.lock().unwrap_or_else(|p| p.into_inner()) = Some(Err("cancelled".into()));
                            return;
                        }
                        if let Some(x) = failure.lock().unwrap_or_else(|p| p.into_inner()).take() {
                            *res.lock().unwrap_or_else(|p| p.into_inner()) = Some(Err(x));
                            return;
                        }
                        let mut extra = String::new();
                        if bundle {
                            let eng = make(&mut samples);
                            let files: Vec<crate::bundle::Stem> = todo
                                .iter()
                                .map(|ti| {
                                    let safe: String = names[*ti].chars().map(|c| if c.is_alphanumeric() || c == '-' || c == ' ' { c } else { '_' }).collect();
                                    crate::bundle::Stem { track: *ti, file: format!("audio/{stem}_{:02}_{}.{ext}", ti + 1, safe.trim()) }
                                })
                                .collect();
                            match crate::bundle::write(&eng, &root, &files, start, end, tail, sr as u32) {
                                Ok(m) => extra = format!(" ({m})"),
                                Err(x) => {
                                    *res.lock().unwrap_or_else(|p| p.into_inner()) = Some(Err(hint(&x)));
                                    return;
                                }
                            }
                        }
                        pr.store(1000, Ordering::Relaxed);
                        *res.lock().unwrap_or_else(|p| p.into_inner()) = Some(Ok(format!("{} stems in {}{extra}", todo.len(), if bundle { root.display() } else { dir.display() })));
                    })
                    .map_err(|x| x.to_string())?;
                self.render = Some(job);
                ok!(true)
            }
            // 0 idle, 1 running, 2 finished, 3 failed, 4 cancelled
            "daw_render_state" => ok!(match &self.render {
                None => 0.0,
                Some(j) => match &*j.result.lock().unwrap_or_else(|p| p.into_inner()) {
                    None => 1.0,
                    Some(Ok(_)) => 2.0,
                    Some(Err(m)) if m == "cancelled" => 4.0,
                    Some(Err(_)) => 3.0,
                },
            }),
            "daw_render_progress" => ok!(self.render.as_ref().map(|j| j.progress.load(Ordering::Relaxed) as f64 / 1000.0).unwrap_or(0.0)),
            "daw_render_elapsed" => ok!(self.render.as_ref().map(|j| j.started.elapsed().as_secs_f64()).unwrap_or(0.0)),
            "daw_render_message" => ok!(match &self.render {
                Some(j) => match &*j.result.lock().unwrap_or_else(|p| p.into_inner()) {
                    Some(Ok(p)) => format!("Rendered {p}"),
                    Some(Err(m)) => format!("Render failed: {m}"),
                    None => String::new(),
                },
                None => String::new(),
            }),
            "daw_render_cancel" => {
                if let Some(j) = &self.render {
                    j.cancel.store(true, Ordering::Relaxed);
                }
                Ok(Val::Unit)
            }
            "daw_render_ack" => {
                self.render = None;
                Ok(Val::Unit)
            }
            "daw_bounce" => {
                // (path, start_beat, end_beat, tail_s, bits, format "wav"|"flac"|"ogg", ogg quality 0..1)
                let mut en = e!();
                let end = if a.n(2) > a.n(1) { a.n(2) } else { en.project.end_beat() };
                let data = en.bounce_range(a.n(1), end, a.n(3).max(0.0));
                drop(en);
                let bits = if a.n(4) > 0.5 { a.n(4) as u16 } else { 16 };
                let path = a.s(0);
                if let Some(parent) = std::path::Path::new(&path).parent() {
                    if !parent.as_os_str().is_empty() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                }
                let fmt = if a.s(5).is_empty() { if path.ends_with(".flac") { "flac".into() } else if path.ends_with(".ogg") { "ogg".into() } else { "wav".into() } } else { a.s(5) };
                let q = if a.0.len() > 6 { a.n(6) as f32 } else { 0.5 };
                match crate::export::write_audio(&path, &data, sr as u32, &fmt, bits, q) {
                    Ok(()) => {
                        self.status = format!("Rendered {path}");
                        ok!(true)
                    }
                    Err(x) => {
                        // Windows Controlled Folder Access blocks unknown programs from Documents / Music / ... and
                        // reports it as "file not found"
                        let hint = if cfg!(windows) && (x.contains("find") || x.contains("denied") || x.contains("not found")) {
                            " - if the folder exists, Windows Security > Ransomware protection may be blocking ling.exe (allow it under Controlled folder access)"
                        } else {
                            ""
                        };
                        self.status = format!("Render failed: {x}{hint}");
                        Ok(Val::Num(0.0))
                    }
                }
            }
            // ── audio input recording ──
            "daw_audio_inputs" => ok!(Val::List(crate::audio_in::list_inputs().into_iter().map(Val::Str).collect())),
            "daw_audio_rec_start" => {
                // (track, input device name or "")
                if self.arec.is_some() {
                    return Ok(Val::Num(1.0));
                }
                let in_dev = if a.s(1).is_empty() { self.in_pref.clone() } else { a.s(1) };
                match crate::audio_in::start(Some(&in_dev), self.in_gain_db) {
                    Ok(cap) => {
                        let start = e!().heard_position();
                        self.status = format!("Recording audio from {}", cap.device);
                        self.arec = Some(AudioRec { cap, start, track: a.u(0) });
                        ok!(true)
                    }
                    Err(x) => {
                        self.status = format!("Audio input failed: {x}");
                        Ok(Val::Num(0.0))
                    }
                }
            }
            "daw_audio_rec_active" => ok!(self.arec.is_some()),
            "daw_audio_rec_level" => ok!(self.arec.as_ref().map(|r| f32::from_bits(r.cap.level.load(Ordering::Relaxed))).unwrap_or(0.0)),
            // live waveform of the take in progress: flat [min, max, ...] for at most `cols` columns
            "daw_audio_rec_wave" => ok!(Val::List(self.arec.as_ref().map(|r| r.cap.wave(a.u(0).clamp(1, 4096))).unwrap_or_default().into_iter().map(|x| Val::Num(x as f64)).collect())),
            // [start beat, length in beats, track] of the take in progress, [] when idle
            "daw_audio_rec_info" => match self.arec.as_ref() {
                Some(r) => {
                    let bpm = e!().project.bpm;
                    ok!(Val::List(vec![Val::Num(r.start), Val::Num(r.cap.seconds() * bpm / 60.0), Val::Num(r.track as f64)]))
                }
                None => ok!(Val::List(vec![])),
            },
            "daw_audio_rec_stop" => {
                let Some(rec) = self.arec.take() else { return Ok(Val::Num(-1.0)) };
                let mut data = std::mem::take(&mut *rec.cap.buf.lock().unwrap_or_else(|p| p.into_inner()));
                fix_take(&mut data, self.in_norm);
                if data.len() < 2 * 256 {
                    self.status = "Recording was empty".into();
                    return Ok(Val::Num(-1.0));
                }
                let dir = if self.path.is_empty() { std::env::temp_dir() } else { std::path::Path::new(&self.path).parent().map(|p| p.join("audio")).unwrap_or_else(std::env::temp_dir) };
                let _ = std::fs::create_dir_all(&dir);
                let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
                let mut file = dir.join(format!("take_{stamp}.wav")).to_string_lossy().into_owned();
                let mut dup = 1;
                while std::path::Path::new(&file).exists() {
                    file = dir.join(format!("take_{stamp}_{dup}.wav")).to_string_lossy().into_owned();
                    dup += 1;
                }
                self.samples.remove(&file);
                let spec = hound::WavSpec { channels: 2, sample_rate: rec.cap.rate, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
                let wrote = (|| -> Result<(), String> {
                    let mut w = hound::WavWriter::create(&file, spec).map_err(|x| x.to_string())?;
                    for s in &data {
                        w.write_sample(*s).map_err(|x| x.to_string())?;
                    }
                    w.finalize().map_err(|x| x.to_string())
                })();
                if let Err(x) = wrote {
                    self.status = format!("Could not save take: {x}");
                    return Ok(Val::Num(-1.0));
                }
                match self.load_sample_cached(&file) {
                    Ok(smp) => {
                        let mut en = e!();
                        let len = (smp.seconds() * en.project.bpm / 60.0).max(0.25);
                        let Some(t) = en.project.tracks.get_mut(rec.track) else { return Ok(Val::Num(-1.0)) };
                        t.clips.push(Clip { start: rec.start.max(0.0), len, name: "Recording".into(), loop_len: 0.0, lyric_text: String::new(), lyric_lang: String::new(), tag: String::new(), data: ClipData::Audio { sample: smp, offset_s: 0.0, gain: 1.0, pitch: 0.0, warp: 0.0 } });
                        self.touch();
                        self.status = format!("Recorded {file}");
                        ok!(t.clips.len() - 1)
                    }
                    Err(x) => {
                        self.status = format!("Could not load take: {x}");
                        Ok(Val::Num(-1.0))
                    }
                }
            }
            "daw_perf_rec" => {
                e!().perf_rec = a.n(0) > 0.5;
                Ok(Val::Unit)
            }
            // ── filesystem ──
            "fs_list" => {
                let mut items: Vec<(String, bool, u64)> = std::fs::read_dir(a.s(0))
                    .map(|rd| {
                        rd.filter_map(|e| e.ok())
                            .map(|e| {
                                let md = e.metadata().ok();
                                (e.file_name().to_string_lossy().into_owned(), md.as_ref().map(|m| m.is_dir()).unwrap_or(false), md.map(|m| m.len()).unwrap_or(0))
                            })
                            .filter(|(n, _, _)| !n.starts_with('.'))
                            .collect()
                    })
                    .unwrap_or_default();
                items.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.to_lowercase().cmp(&y.0.to_lowercase())));
                let mut v = Vec::with_capacity(items.len() * 3);
                for (n, d, s) in items {
                    v.extend([Val::Str(n), Val::from(d), Val::Num(s as f64)]);
                }
                ok!(Val::List(v))
            }
            "fs_is_dir" => ok!(std::path::Path::new(&a.s(0)).is_dir()),
            "fs_parent" => ok!(std::path::Path::new(&a.s(0)).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()),
            "fs_join" => ok!(std::path::Path::new(&a.s(0)).join(a.s(1)).to_string_lossy().into_owned()),
            "fs_home" => ok!(std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default()),
            "fs_cwd" => ok!(std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()),
            "fs_mkdir" => ok!(std::fs::create_dir_all(a.s(0)).is_ok()),
            "fs_copy" => ok!(std::fs::copy(a.s(0), a.s(1)).is_ok()),
            "fs_delete" => {
                let p = a.s(0);
                let path = std::path::Path::new(&p);
                ok!(if path.is_dir() { std::fs::remove_dir_all(path).is_ok() } else { std::fs::remove_file(path).is_ok() })
            }
            "fs_reveal" => {
                #[cfg(windows)]
                {
                    let _ = std::process::Command::new("explorer").arg(a.s(0).replace('/', "\\")).spawn();
                }
                #[cfg(target_os = "macos")]
                {
                    let _ = std::process::Command::new("open").arg(a.s(0)).spawn();
                }
                #[cfg(all(unix, not(target_os = "macos")))]
                {
                    let _ = std::process::Command::new("xdg-open").arg(a.s(0)).spawn();
                }
                Ok(Val::Unit)
            }
            // ── native dialogs ──
            "dlg_open" => {
                // (title, "ext1,ext2", optional start folder)
                let mut d = rfd::FileDialog::new().set_title(a.s(0));
                if let Some(dir) = dlg_dir(&a.s(2), &self.dlg_dir) {
                    d = d.set_directory(dir);
                }
                let exts: Vec<String> = a.s(1).split(',').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
                if !exts.is_empty() {
                    d = d.add_filter("files", &exts);
                }
                let r = d.pick_file();
                if let Some(dir) = r.as_ref().and_then(|p| p.parent()) {
                    self.dlg_dir = dir.to_string_lossy().into_owned();
                }
                ok!(r.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default())
            }
            "dlg_save" => {
                // (title, "ext1,ext2", suggested name, optional start folder)
                let mut d = rfd::FileDialog::new().set_title(a.s(0)).set_file_name(a.s(2));
                // always start in a folder that exists (a stale "last used" folder makes Windows report "File not found")
                if let Some(dir) = dlg_dir(&a.s(3), &self.dlg_dir) {
                    d = d.set_directory(dir);
                }
                let exts: Vec<String> = a.s(1).split(',').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
                if !exts.is_empty() {
                    d = d.add_filter("files", &exts);
                }
                let mut r = d.save_file();
                if let (Some(p), Some(ext)) = (r.as_mut(), exts.first()) {
                    if p.extension().is_none() {
                        p.set_extension(ext);
                    }
                }
                if let Some(dir) = r.as_ref().and_then(|p| p.parent()) {
                    self.dlg_dir = dir.to_string_lossy().into_owned();
                }
                ok!(r.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default())
            }
            "dlg_folder" => ok!({
                let mut d = rfd::FileDialog::new().set_title(a.s(0));
                if let Some(start) = dlg_dir(&abs_path(&a.s(1)), &self.dlg_dir) {
                    d = d.set_directory(start);
                }
                d.pick_folder().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
            }),
            other => Err(format!("unknown builtin {other}")),
        }
    }
}

/// First existing folder among the caller's suggestion, the last used one, the user's Documents / home.
fn dlg_dir(wanted: &str, last: &str) -> Option<std::path::PathBuf> {
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
    let docs = std::path::Path::new(&home).join("Documents");
    [std::path::PathBuf::from(wanted), std::path::PathBuf::from(last), docs, std::path::PathBuf::from(&home)]
        .into_iter()
        .find(|p| !p.as_os_str().is_empty() && p.is_dir())
}

/// Absolute form of a user-supplied path (empty stays empty).
fn abs_path(p: &str) -> String {
    if p.is_empty() {
        return String::new();
    }
    std::path::absolute(p).map(|x| x.to_string_lossy().into_owned()).unwrap_or_else(|_| p.to_string())
}

impl Default for Daw {
    fn default() -> Self {
        Self::new()
    }
}

/// Tidy a finished stereo take: a mono mic on one side of a stereo interface leaves the other side
/// (almost) silent, so mirror the live side; optionally lift a quiet take to about -3 dBFS.
fn fix_take(data: &mut [f32], normalize: bool) {
    let (mut el, mut er, mut pk) = (0.0f64, 0.0f64, 0.0f32);
    for f in data.chunks_exact(2) {
        el += (f[0] * f[0]) as f64;
        er += (f[1] * f[1]) as f64;
        pk = pk.max(f[0].abs()).max(f[1].abs());
    }
    if el.max(er) > 0.0 && el.min(er) < el.max(er) * 0.01 {
        let left_live = el > er;
        for f in data.chunks_exact_mut(2) {
            let v = if left_live { f[0] } else { f[1] };
            f[0] = v;
            f[1] = v;
        }
    }
    if normalize && pk > 1e-5 && pk < 0.5 {
        let g = 0.7 / pk;
        for s in data.iter_mut() {
            *s *= g;
        }
    }
}


/// Points of a repeating automation shape over [b0, b1] beats (`lo`..`hi` are 0..1 lane values).
pub(crate) fn auto_shape_points(shape: &str, b0: f64, b1: f64, period: f64, lo: f32, hi: f32, seed: u32) -> Vec<(f64, f32)> {
    let mut out: Vec<(f64, f32)> = vec![];
    let span = hi - lo;
    let mut rng = seed.wrapping_mul(2654435761).wrapping_add(12345);
    let mut rnd = move || {
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        (rng & 0xffff) as f32 / 65535.0
    };
    let mut t = b0;
    while t < b1 - 1e-9 && out.len() < 6000 {
        let end = (t + period).min(b1);
        let full = (end - t) >= period - 1e-9;
        let at = |f: f64| t + (end - t) * f;
        match shape {
            "sine" => {
                for k in 0..=8 {
                    let f = k as f64 / 8.0;
                    out.push((at(f), lo + span * (0.5 - 0.5 * (std::f64::consts::TAU * f).cos()) as f32));
                }
            }
            "tri" => {
                out.push((at(0.0), lo));
                out.push((at(0.5), hi));
                out.push((at(1.0), lo));
            }
            "up" => {
                out.push((at(0.0), lo));
                out.push((at(1.0) - 0.01, hi));
            }
            "down" => {
                out.push((at(0.0), hi));
                out.push((at(1.0) - 0.01, lo));
            }
            "pulse" => {
                out.push((at(0.0), hi));
                out.push((at(0.5) - 0.01, hi));
                out.push((at(0.5), lo));
                out.push((at(1.0) - 0.01, lo));
            }
            "steps" => {
                // a staircase of 8 equal steps climbing from lo to hi
                for k in 0..8 {
                    let v = lo + span * k as f32 / 7.0;
                    out.push((at(k as f64 / 8.0), v));
                    out.push((at((k + 1) as f64 / 8.0) - 0.01, v));
                }
            }
            _ => {
                // random: sample & hold, 4 values per period
                for k in 0..4 {
                    let v = lo + span * rnd();
                    out.push((at(k as f64 / 4.0), v));
                    out.push((at((k + 1) as f64 / 4.0) - 0.01, v));
                }
            }
        }
        if !full {
            break;
        }
        t += period;
    }
    out.retain(|p| p.0 >= b0 - 1e-9 && p.0 <= b1 + 1e-9);
    // drop duplicate beats from the joins between periods (sine / tri end where the next one starts)
    out.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-6);
    out
}


/// Every panic (any thread) is appended, with its location and a backtrace, to `lingstation_crash.log` in the working folder,
/// so a crash that closes the window still leaves its reason behind. Installed once.
fn install_panic_log() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let thread = std::thread::current();
            let text = format!("[{secs}] panic in thread '{}': {info}
{}

", thread.name().unwrap_or("?"), std::backtrace::Backtrace::force_capture());
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("lingstation_crash.log") {
                use std::io::Write;
                let _ = f.write_all(text.as_bytes());
            }
            previous(info);
        }));
    });
}

/// The imported part an FL automation clip's target ("SubSynth" of "SubSynth - Filter Cutoff") names: its channel name, its plugin, or a
/// name that contains it.
fn guess_part(parts: &[crate::midi_import::ImportedTrack], target: &str) -> Option<usize> {
    let t = target.trim().to_lowercase();
    if t.is_empty() {
        return None;
    }
    parts
        .iter()
        .position(|p| p.name.to_lowercase() == t)
        .or_else(|| parts.iter().position(|p| p.plugin.to_lowercase() == t))
        .or_else(|| {
            parts.iter().position(|p| {
                let n = p.name.to_lowercase();
                n.len() > 2 && (n.contains(&t) || t.contains(&n))
            })
        })
}
