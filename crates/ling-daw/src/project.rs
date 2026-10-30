//! Project (de)serialisation: `project.json` inside a project folder.
use crate::engine::Engine;
use crate::model::*;
use crate::patch;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct FxDoc {
    pub name: String,
    pub params: Vec<(String, f32)>,
    pub bypass: bool,
    /// a file the effect works from (the growl's spectral sample), empty for none
    #[serde(default)]
    pub source: String,
    /// the whole state of an effect that has more than named knobs (the Ringstorm's pedal graph) as JSON, empty for none
    #[serde(default)]
    pub config: String,
    /// dry / wet of the whole effect (1 = all effect)
    #[serde(default = "one")]
    pub wet: f32,
}
#[derive(Serialize, Deserialize, Clone)]
pub struct NoteDoc {
    pub start: f64,
    pub len: f64,
    pub key: u8,
    pub vel: f32,
    #[serde(default)]
    pub pan: f32,
    #[serde(default = "half")]
    pub cutoff: f32,
    #[serde(default = "half")]
    pub reso: f32,
    #[serde(default)]
    pub lyric: String,
    /// per-note expression curves (model.rs `Note`): absent in older projects and skipped when empty
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bend: Vec<(f32, f32)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub press: Vec<(f32, f32)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub timbre: Vec<(f32, f32)>,
}
fn half() -> f32 {
    0.5
}
fn one() -> f32 {
    1.0
}
fn four() -> f64 {
    4.0
}
#[derive(Serialize, Deserialize, Clone)]
pub struct ClipDoc {
    pub name: String,
    pub start: f64,
    pub len: f64,
    #[serde(default)]
    pub midi_notes: Vec<NoteDoc>,
    #[serde(default)]
    pub audio_path: String,
    #[serde(default)]
    pub offset_s: f64,
    #[serde(default = "one")]
    pub gain: f32,
    #[serde(default)]
    pub pitch: f32,
    #[serde(default)]
    pub warp: f32,
    #[serde(default)]
    pub loop_len: f64,
    #[serde(default)]
    pub lyric_text: String,
    #[serde(default)]
    pub lyric_lang: String,
    #[serde(default)]
    pub tag: String,
}
#[derive(Serialize, Deserialize, Clone)]
pub struct AutoDoc {
    pub param: String,
    pub points: Vec<(f64, f32)>,
}
#[derive(Serialize, Deserialize, Clone)]
pub struct RouteDoc {
    pub kind: String, // send | sidechain | duck
    pub from: usize,
    pub to: usize,
    pub amount: f32,
    #[serde(default)]
    pub keys: Vec<u8>,
}
#[derive(Serialize, Deserialize, Clone)]
pub struct TrackDoc {
    pub name: String,
    pub color: u32,
    pub kind: String, // instrument | audio
    #[serde(default)]
    pub instrument: String, // synth | drums
    #[serde(default)]
    pub patch: String,
    #[serde(default)]
    pub state: Vec<(String, f32)>,
    pub level: f32,
    pub pan: f32,
    pub muted: bool,
    pub solo: bool,
    #[serde(default)]
    pub effects: Vec<FxDoc>,
    #[serde(default)]
    pub clips: Vec<ClipDoc>,
    #[serde(default)]
    pub session: Vec<ClipDoc>,
    #[serde(default)]
    pub auto: Vec<AutoDoc>,
    /// the Sampler's zones as JSON (sample files by path, relative to the project once saved)
    #[serde(default)]
    pub config: String,
}
#[derive(Serialize, Deserialize, Clone)]
pub struct ProjectDoc {
    pub name: String,
    pub tempo_bpm: f64,
    #[serde(default)]
    pub tempo_map: Vec<(f64, f64)>,
    pub master_level: f32,
    #[serde(default = "default_true")]
    pub limiter: bool,
    #[serde(default)]
    pub master_effects: Vec<FxDoc>,
    #[serde(default)]
    pub master_auto: Vec<AutoDoc>,
    #[serde(default)]
    pub loop_on: bool,
    #[serde(default)]
    pub loop_start: f64,
    #[serde(default)]
    pub loop_end: f64,
    #[serde(default)]
    pub meta: Vec<(String, String)>,
    #[serde(default)]
    pub routes: Vec<RouteDoc>,
    #[serde(default)]
    pub scenes: Vec<String>,
    #[serde(default)]
    pub slots: Vec<Vec<i32>>,
    #[serde(default)]
    pub session_only: bool,
    #[serde(default = "four")]
    pub performance_launch_quantize_beats: f64,
    pub tracks: Vec<TrackDoc>,
}

impl ProjectDoc {
    pub fn empty(name: &str, bpm: f64) -> ProjectDoc {
        ProjectDoc {
            name: name.into(),
            tempo_bpm: bpm,
            tempo_map: vec![],
            master_level: 0.9,
            limiter: true,
            master_effects: vec![],
            master_auto: vec![],
            loop_on: false,
            loop_start: 0.0,
            loop_end: 16.0,
            meta: vec![],
            routes: vec![],
            scenes: vec![],
            slots: vec![],
            session_only: false,
            performance_launch_quantize_beats: 4.0,
            tracks: vec![],
        }
    }
}

fn fx_doc(e: &EffectSlot) -> FxDoc {
    FxDoc { name: e.name.clone(), params: e.params.iter().map(|(n, v)| (n.to_string(), *v)).collect(), bypass: e.bypass, source: e.fx.source(), config: e.fx.info("config"), wet: e.wet }
}
fn auto_doc(l: &AutoLane) -> AutoDoc {
    AutoDoc { param: l.param.clone(), points: l.points.clone() }
}

fn clip_doc(c: &Clip) -> ClipDoc {
    match &c.data {
        ClipData::Midi(n) => ClipDoc {
            name: c.name.clone(),
            start: c.start,
            len: c.len,
            midi_notes: n.iter().map(|n| NoteDoc { start: n.start, len: n.len, key: n.key, vel: n.vel, pan: n.pan, cutoff: n.cutoff, reso: n.reso, lyric: n.lyric.clone(), bend: n.bend.clone(), press: n.press.clone(), timbre: n.timbre.clone() }).collect(),
            audio_path: String::new(),
            offset_s: 0.0,
            gain: 1.0,
            pitch: 0.0,
            warp: 0.0,
            loop_len: c.loop_len,
            lyric_text: c.lyric_text.clone(),
            lyric_lang: c.lyric_lang.clone(),
            tag: c.tag.clone(),
        },
        ClipData::Audio { sample, offset_s, gain, pitch, warp } => ClipDoc {
            name: c.name.clone(),
            start: c.start,
            len: c.len,
            midi_notes: vec![],
            audio_path: sample.path.clone(),
            offset_s: *offset_s,
            gain: *gain,
            pitch: *pitch,
            warp: *warp,
            loop_len: c.loop_len,
            lyric_text: String::new(),
            lyric_lang: String::new(),
            tag: String::new(),
        },
    }
}

fn default_true() -> bool {
    true
}

pub fn to_doc(e: &Engine) -> ProjectDoc {
    let p = &e.project;
    ProjectDoc {
        name: p.name.clone(),
        tempo_bpm: p.bpm,
        tempo_map: p.tempo_map.clone(),
        master_level: p.master_volume,
        limiter: p.limiter,
        master_effects: p.master_effects.iter().map(fx_doc).collect(),
        master_auto: p.master_auto.iter().map(auto_doc).collect(),
        loop_on: p.loop_on,
        loop_start: p.loop_start,
        loop_end: p.loop_end,
        meta: p.meta.clone(),
        routes: p
            .routes
            .iter()
            .map(|r| RouteDoc { kind: match r.kind { RouteKind::Send => "send", RouteKind::Sidechain => "sidechain", RouteKind::Duck => "duck", RouteKind::Mod => "mod" }.into(), from: r.from, to: r.to, amount: r.amount, keys: r.keys.clone() })
            .collect(),
        scenes: p.scenes.clone(),
        slots: p.slots.clone(),
        session_only: p.session_only,
        performance_launch_quantize_beats: p.quant,
        tracks: p
            .tracks
            .iter()
            .map(|t| TrackDoc {
                name: t.name.clone(),
                color: t.color,
                kind: if matches!(t.kind, TrackKind::Audio) { "audio".into() } else { "instrument".into() },
                instrument: t.inst_kind.clone(),
                patch: t.patch.clone(),
                state: t.inst.as_ref().map(|i| i.state()).unwrap_or_default(),
                level: t.volume,
                pan: t.pan,
                muted: t.mute,
                solo: t.solo,
                effects: t.effects.iter().map(fx_doc).collect(),
                clips: t.clips.iter().map(clip_doc).collect(),
                session: t.session.iter().map(clip_doc).collect(),
                auto: t.auto.iter().map(auto_doc).collect(),
                config: if t.inst_kind == "sampler" || t.inst_kind == "script" || t.inst_kind == "chip" || t.inst_kind == "growl" || t.inst_kind == "prism" || t.inst_kind == "slicer" || t.inst_kind == "nova" { t.inst.as_ref().map(|i| i.info("config")).unwrap_or_default() } else { String::new() },
            })
            .collect(),
    }
}

/// Sorted, finite automation points.
fn clean_lane(a: &AutoDoc) -> AutoLane {
    let mut pts: Vec<(f64, f32)> = a.points.iter().copied().filter(|(b, v)| b.is_finite() && v.is_finite()).collect();
    pts.sort_by(|x, y| x.0.total_cmp(&y.0));
    AutoLane { param: a.param.clone(), points: pts }
}

fn fx_slot(d: &FxDoc, sr: f32, base: Option<&std::path::Path>) -> Option<EffectSlot> {
    let mut s = EffectSlot::new(&d.name, sr)?;
    for (n, v) in &d.params {
        if let Some(i) = s.params.iter().position(|(pn, _)| pn == n) {
            s.set(i, *v);
        }
    }
    if !d.source.is_empty() {
        // a missing file leaves the effect on its built-in source
        s.fx.load_source(&resolve_file(&d.source, base));
    }
    if !d.config.is_empty() {
        s.fx.command("load_config", &[], &d.config);
    }
    s.bypass = d.bypass;
    s.wet = d.wet.clamp(0.0, 1.0);
    Some(s)
}

/// A file a project refers to: as written, or relative to the project folder.
pub fn resolve_file(path: &str, base: Option<&std::path::Path>) -> String {
    if path.is_empty() || std::path::Path::new(path).exists() {
        return path.to_string();
    }
    if let Some(b) = base {
        let j = b.join(path);
        if j.exists() {
            return j.to_string_lossy().into_owned();
        }
    }
    path.to_string()
}

/// Find (and cache) an audio file referenced by a project.
pub fn resolve_sample(path: &str, base: Option<&std::path::Path>, cache: &mut HashMap<String, Arc<Sample>>) -> Option<Arc<Sample>> {
    if let Some(s) = cache.get(path) {
        return Some(s.clone());
    }
    let mut cand = vec![path.to_string()];
    if let Some(b) = base {
        cand.push(b.join(path).to_string_lossy().into_owned());
        if let Some(f) = std::path::Path::new(path).file_name() {
            cand.push(b.join("audio").join(f).to_string_lossy().into_owned());
        }
    }
    cand.iter().find_map(|c| load_sample(c).ok()).map(|s| {
        cache.insert(path.to_string(), s.clone());
        s
    })
}

/// Decode every audio file a document references (call before taking the engine lock).
pub fn preload(d: &ProjectDoc, base: Option<&std::path::Path>, cache: &mut HashMap<String, Arc<Sample>>) {
    for t in &d.tracks {
        for c in t.clips.iter().chain(t.session.iter()) {
            if !c.audio_path.is_empty() {
                let _ = resolve_sample(&c.audio_path, base, cache);
            }
        }
    }
}

fn clip_from_doc(cd: &ClipDoc, base: Option<&std::path::Path>, cache: &mut HashMap<String, Arc<Sample>>) -> Option<Clip> {
    let data = if cd.audio_path.is_empty() {
        ClipData::Midi(cd.midi_notes.iter().map(|n| Note { start: n.start, len: n.len, key: n.key, vel: n.vel, pan: n.pan, cutoff: n.cutoff, reso: n.reso, lyric: n.lyric.clone(), bend: crate::gesture::tidy(&n.bend), press: crate::gesture::tidy(&n.press), timbre: crate::gesture::tidy(&n.timbre) }).collect())
    } else {
        let smp = resolve_sample(&cd.audio_path, base, cache);
        ClipData::Audio { sample: smp?, offset_s: cd.offset_s, gain: cd.gain, pitch: cd.pitch, warp: cd.warp }
    };
    Some(Clip { start: cd.start, len: cd.len, name: cd.name.clone(), loop_len: cd.loop_len, lyric_text: cd.lyric_text.clone(), lyric_lang: cd.lyric_lang.clone(), tag: cd.tag.clone(), data })
}

/// Rebuild the engine's project from a document. `base` resolves relative audio paths.
pub fn apply_doc(e: &mut Engine, d: &ProjectDoc, base: Option<&std::path::Path>, cache: &mut HashMap<String, Arc<Sample>>) {
    let sr = e.sr;
    e.stop();
    let mut p = Project::new();
    p.name = d.name.clone();
    p.tempo_map = {
        let mut m: Vec<(f64, f64)> = d.tempo_map.iter().copied().filter(|(b, v)| b.is_finite() && v.is_finite() && *b >= 0.0).map(|(b, v)| (b, v.clamp(20.0, 400.0))).collect();
        m.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        m.truncate(5000);
        m
    };
    p.bpm = if d.tempo_bpm.is_finite() { d.tempo_bpm.clamp(20.0, 400.0) } else { 120.0 };
    p.limiter = d.limiter;
    p.master_volume = if d.master_level.is_finite() { d.master_level.clamp(0.0, 2.0) } else { 0.9 };
    p.loop_on = d.loop_on;
    p.loop_start = d.loop_start;
    p.loop_end = if d.loop_end > d.loop_start { d.loop_end } else { d.loop_start + 16.0 };
    p.meta = d.meta.clone();
    p.master_effects = d.master_effects.iter().filter_map(|f| fx_slot(f, sr, base)).collect();
    p.master_auto = d.master_auto.iter().map(clean_lane).collect();
    p.routes = d
        .routes
        .iter()
        .map(|r| Route { kind: match r.kind.as_str() { "sidechain" => RouteKind::Sidechain, "duck" => RouteKind::Duck, "mod" => RouteKind::Mod, _ => RouteKind::Send }, from: r.from, to: r.to, amount: r.amount, keys: r.keys.clone() })
        .collect();
    p.scenes = d.scenes.clone();
    p.slots = d.slots.clone();
    p.session_only = d.session_only;
    p.quant = if d.performance_launch_quantize_beats.is_finite() { d.performance_launch_quantize_beats.clamp(0.0, 64.0) } else { 4.0 };
    for td in &d.tracks {
        let audio = td.kind == "audio";
        // A track may reference a .ling patch instead of embedding instrument state.
        let mut kind = td.instrument.clone();
        let mut state = td.state.clone();
        let mut patch_fx: Vec<EffectSlot> = vec![];
        if !audio && state.is_empty() && !td.patch.is_empty() && kind != "sf2" {
            if let Ok(text) = std::fs::read_to_string(resolve_file(&td.patch, base)) {
                let pd = patch::parse(&text);
                kind = pd.kind.clone();
                state = pd.params.clone();
                patch_fx = patch_effects(&pd, sr);
            }
        }
        let inst = if audio {
            None
        } else {
            let mut i = if kind == "sf2" {
                crate::dsp::make_instrument_file("sf2", sr, &resolve_file(&td.patch, base))
            } else if kind == "script" && !td.config.is_empty() {
                crate::dsp::scriptdsp::ScriptSynth::from_config(sr, &td.config).ok().map(|s| Box::new(s) as Box<dyn crate::dsp::Instrument>)
            } else if kind == "sampler" && !td.config.is_empty() {
                crate::dsp::sampler::Sampler::from_config(sr, &td.config, base).ok().map(|s| Box::new(s) as Box<dyn crate::dsp::Instrument>)
            } else {
                crate::dsp::make_instrument(if kind.is_empty() { "synth" } else { &kind }, sr)
            };
            if let Some(i) = i.as_mut() {
                i.load_state(&state);
                // Ling Chip: its own sample, stored relative to the project (samples/...); the Ling Slicer's file likewise
                if matches!(kind.as_str(), "chip" | "growl" | "prism" | "slicer" | "nova") && !td.config.is_empty() {
                    i.command("load", &[], &resolve_file(&td.config, base));
                }
            }
            i
        };
        let mut t = Track::new(&td.name, if audio { TrackKind::Audio } else { TrackKind::Instrument }, td.color, inst);
        t.inst_kind = if kind.is_empty() && !audio { "synth".into() } else { kind.clone() };
        t.patch = td.patch.clone();
        t.volume = if td.level.is_finite() { td.level.clamp(0.0, 2.0) } else { 0.8 };
        t.pan = if td.pan.is_finite() { td.pan.clamp(-1.0, 1.0) } else { 0.0 };
        t.mute = td.muted;
        t.solo = td.solo;
        t.effects = td.effects.iter().filter_map(|f| fx_slot(f, sr, base)).collect();
        if td.effects.is_empty() {
            t.effects = patch_fx;
        }
        t.auto = td.auto.iter().map(clean_lane).collect();
        for cd in &td.clips {
            if let Some(c) = clip_from_doc(cd, base, cache) {
                t.clips.push(c);
            }
        }
        for cd in &td.session {
            // keep launcher indices stable even when an audio file is missing: use an empty placeholder
            let c = clip_from_doc(cd, base, cache).unwrap_or_else(|| Clip::midi(0.0, cd.len.max(1.0), &format!("{} (missing)", cd.name)));
            t.session.push(c);
        }
        p.tracks.push(t);
    }
    p.prepare_audio();
    // validate cross references now that the tracks exist
    let nt = p.tracks.len();
    p.routes.retain(|r| r.from < nt && r.to < nt && r.from != r.to);
    for r in p.routes.iter_mut() {
        r.amount = if r.amount.is_finite() { r.amount.clamp(0.0, 4.0) } else { 0.5 };
    }
    p.slots.truncate(MAX_SCENES);
    for row in p.slots.iter_mut() {
        row.truncate(MAX_TRACKS);
        for (ti, v) in row.iter_mut().enumerate() {
            let ok = ti < nt && *v >= 0 && (*v as usize) < p.tracks[ti].session.len();
            if !ok {
                *v = -1;
            }
        }
    }
    p.scenes.truncate(MAX_SCENES);
    e.project = p;
    e.position = 0.0;
    if let Some(t) = e.rec_arm {
        if t >= e.project.tracks.len() {
            e.rec_arm = None;
        }
    }
}

pub fn save(e: &Engine, path: &str) -> Result<(), String> {
    save_with_media(e, path, false)
}

/// Save `project.json`; with `copy_media`, audio files that live outside the project
/// folder are copied into `<project>/audio/` and referenced relatively.
pub fn save_with_media(e: &Engine, path: &str, copy_media: bool) -> Result<(), String> {
    let mut doc = to_doc(e);
    if copy_media {
        if let Some(dir) = std::path::Path::new(path).parent() {
            let audio_dir = dir.join("audio");
            let mut mapped: HashMap<String, String> = HashMap::new();
            for t in doc.tracks.iter_mut() {
                for c in t.clips.iter_mut().chain(t.session.iter_mut()) {
                    if c.audio_path.is_empty() {
                        continue;
                    }
                    if let Some(rel) = mapped.get(&c.audio_path) {
                        c.audio_path = rel.clone();
                        continue;
                    }
                    let src = std::path::PathBuf::from(&c.audio_path);
                    if !src.is_absolute() || !src.exists() {
                        continue;
                    }
                    let canon_dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
                    let inside = src.canonicalize().map(|p| p.starts_with(&canon_dir)).unwrap_or(false);
                    let rel = if inside {
                        src.strip_prefix(dir).ok().map(|r| r.to_string_lossy().replace('\\', "/"))
                    } else {
                        let _ = std::fs::create_dir_all(&audio_dir);
                        let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "audio".into());
                        let ext = src.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "wav".into());
                        let mut dst = audio_dir.join(format!("{stem}.{ext}"));
                        let mut n = 2;
                        while dst.exists() && std::fs::metadata(&dst).map(|m| m.len()).ok() != std::fs::metadata(&src).map(|m| m.len()).ok() {
                            dst = audio_dir.join(format!("{stem}_{n}.{ext}"));
                            n += 1;
                        }
                        if !dst.exists() {
                            std::fs::copy(&src, &dst).map_err(|x| format!("copy {}: {x}", src.display()))?;
                        }
                        dst.file_name().map(|f| format!("audio/{}", f.to_string_lossy()))
                    };
                    if let Some(rel) = rel {
                        mapped.insert(c.audio_path.clone(), rel.clone());
                        c.audio_path = rel;
                    }
                }
            }
        }
    }
    if copy_media {
        if let Some(dir) = std::path::Path::new(path).parent() {
            pack_soundfonts(&mut doc, dir);
            pack_samples(&mut doc, dir);
        }
    }
    let text = serde_json::to_string_pretty(&doc).map_err(|x| x.to_string())?;
    if let Some(dir) = std::path::Path::new(path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // write beside, then rename over: a crash mid-save never leaves a half-written project
    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, text).map_err(|x| x.to_string())?;
    std::fs::rename(&tmp, path).map_err(|x| {
        let _ = std::fs::remove_file(&tmp);
        x.to_string()
    })?;
    if copy_media {
        if let Some(dir) = std::path::Path::new(path).parent() {
            export_companions(e, dir);
        }
    }
    Ok(())
}

/// SoundFonts a track plays are copied into `<project>/soundfonts/` and referenced relatively.
fn pack_soundfonts(doc: &mut ProjectDoc, dir: &std::path::Path) {
    let sf_dir = dir.join("soundfonts");
    for t in doc.tracks.iter_mut() {
        if t.instrument != "sf2" || t.patch.is_empty() {
            continue;
        }
        let src = std::path::PathBuf::from(&t.patch);
        if !src.is_absolute() || !src.is_file() {
            continue;
        }
        let canon_dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        if src.canonicalize().map(|p| p.starts_with(&canon_dir)).unwrap_or(false) {
            if let Ok(r) = src.strip_prefix(dir) {
                t.patch = r.to_string_lossy().replace('\\', "/");
            }
            continue;
        }
        let Some(name) = src.file_name() else { continue };
        let dst = sf_dir.join(name);
        let same = std::fs::metadata(&dst).map(|m| m.len()).ok() == std::fs::metadata(&src).map(|m| m.len()).ok();
        if !(dst.exists() && same) {
            let _ = std::fs::create_dir_all(&sf_dir);
            if std::fs::copy(&src, &dst).is_err() {
                continue;
            }
        }
        t.patch = format!("soundfonts/{}", name.to_string_lossy());
    }
}

/// a file into `<project>/samples/` (unless it is inside the project already); the path to use from then on, relative
fn pack_one(file: &str, dir: &std::path::Path) -> Option<String> {
    let src = std::path::PathBuf::from(file);
    if file.is_empty() || !src.is_absolute() || !src.is_file() {
        return None;
    }
    let canon_dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    if let Ok(cs) = src.canonicalize() {
        if let Ok(r) = cs.strip_prefix(&canon_dir) {
            return Some(r.to_string_lossy().replace('\\', "/"));
        }
    }
    let sdir = dir.join("samples");
    let name = src.file_name()?;
    let mut dst = sdir.join(name);
    let mut n = 1;
    while dst.exists() && std::fs::metadata(&dst).map(|m| m.len()).ok() != std::fs::metadata(&src).map(|m| m.len()).ok() {
        n += 1;
        dst = sdir.join(format!("{}_{n}.{}", src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), src.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()));
    }
    if !dst.exists() {
        let _ = std::fs::create_dir_all(&sdir);
        std::fs::copy(&src, &dst).ok()?;
    }
    Some(format!("samples/{}", dst.file_name()?.to_string_lossy()))
}

/// The audio files a Sampler plays are copied into `<project>/samples/` and the zones refer to them relatively; so is a Ling Chip's
/// own sample.
fn pack_samples(doc: &mut ProjectDoc, dir: &std::path::Path) {
    for t in doc.tracks.iter_mut().filter(|t| matches!(t.instrument.as_str(), "chip" | "growl" | "prism" | "slicer" | "nova") && !t.config.is_empty()) {
        if let Some(r) = pack_one(&t.config, dir) {
            t.config = r;
        }
    }
    let sdir = dir.join("samples");
    for t in doc.tracks.iter_mut().filter(|t| t.instrument == "sampler" && !t.config.is_empty()) {
        let Ok(mut cfg) = serde_json::from_str::<crate::dsp::sampler::Config>(&t.config) else { continue };
        for z in cfg.zones.iter_mut() {
            let src = std::path::PathBuf::from(&z.file);
            if z.file.is_empty() || !src.is_absolute() || !src.is_file() {
                continue;
            }
            let Some(name) = src.file_name() else { continue };
            let canon_dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
            if src.canonicalize().map(|p| p.starts_with(&canon_dir)).unwrap_or(false) {
                if let Ok(r) = src.strip_prefix(dir) {
                    z.file = r.to_string_lossy().replace('\\', "/");
                }
                continue;
            }
            // wavpack and the like were decoded to a cache wav: keep the original file name
            let mut dst = sdir.join(name);
            let mut n = 1;
            while dst.exists() && std::fs::metadata(&dst).map(|m| m.len()).ok() != std::fs::metadata(&src).map(|m| m.len()).ok() {
                n += 1;
                dst = sdir.join(format!("{}_{n}.{}", src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), src.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()));
            }
            if !dst.exists() {
                let _ = std::fs::create_dir_all(&sdir);
                if std::fs::copy(&src, &dst).is_err() {
                    continue;
                }
            }
            z.file = format!("samples/{}", dst.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
        }
        t.config = serde_json::to_string(&cfg).unwrap_or_default();
    }
}

fn safe_name(s: &str) -> String {
    let t: String = s.chars().map(|c| if matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '_' } else { c }).collect();
    let t = t.trim().trim_matches('.').to_string();
    if t.is_empty() {
        "track".into()
    } else {
        t
    }
}

/// Human-readable copies next to project.json: every track's MIDI as a standard file (`midi/`) and its instrument
/// as a patch (`patches/`). project.json stays the single source of truth; these are for other programs and backup.
fn export_companions(e: &Engine, dir: &std::path::Path) {
    let midi_dir = dir.join("midi");
    let patch_dir = dir.join("patches");
    for d in [&midi_dir, &patch_dir] {
        let _ = std::fs::create_dir_all(d);
        // only files this export wrote earlier (numbered names), so a stale track never lingers
        if let Ok(rd) = std::fs::read_dir(d) {
            for f in rd.flatten() {
                let n = f.file_name().to_string_lossy().into_owned();
                if (n.ends_with(".mid") || n.ends_with(".ling")) && n.chars().take(3).all(|c| c.is_ascii_digit()) {
                    let _ = std::fs::remove_file(f.path());
                }
            }
        }
    }
    let _ = crate::midi_io::export_project(e, &dir.join("midi").join("project.mid").to_string_lossy());
    for (i, t) in e.project.tracks.iter().enumerate() {
        let n = safe_name(&t.name);
        if matches!(t.kind, TrackKind::Instrument) {
            if t.clips.iter().any(|c| matches!(&c.data, ClipData::Midi(m) if !m.is_empty())) {
                let _ = crate::midi_io::export_track(e, i, &midi_dir.join(format!("{:03} {n}.mid", i + 1)).to_string_lossy());
            }
            if let Some(inst) = t.inst.as_ref() {
                let fx: Vec<patch::FxSpec> = t.effects.iter().map(|f| patch::FxSpec { name: f.name.clone(), params: f.params.iter().map(|(a, v)| (a.to_string(), *v)).collect() }).collect();
                let kind = if t.inst_kind.is_empty() { "synth" } else { t.inst_kind.as_str() };
                if kind != "sf2" {
                    let _ = std::fs::write(patch_dir.join(format!("{:03} {n}.ling", i + 1)), patch::dump(kind, &t.name, &inst.state(), &fx));
                }
            }
        }
    }
}

pub fn read(path: &str) -> Result<ProjectDoc, String> {
    let text = std::fs::read_to_string(path).map_err(|x| x.to_string())?;
    serde_json::from_str(&text).map_err(|x| x.to_string())
}

/// Build a track's default FX chain from a patch's `fx` lines.
pub fn patch_effects(p: &patch::Patch, sr: f32) -> Vec<EffectSlot> {
    p.fx
        .iter()
        .filter_map(|f| {
            let mut s = EffectSlot::new(&f.name, sr)?;
            for (n, v) in &f.params {
                if let Some(i) = s.params.iter().position(|(pn, _)| pn == n) {
                    s.set(i, *v);
                }
            }
            Some(s)
        })
        .collect()
}
