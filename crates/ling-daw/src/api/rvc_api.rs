//! Voice skins (`daw_rvc_*`): a vocal is converted to another voice with an RVC model. One job at a time runs in the background, by
//! one of two engines:
//! * native (`script` = "native", builds with the `llm` feature): `ling_ai::rvc` on candle (CUDA or CPU) in a thread - HuBERT, the RVC
//!   synthesizer and NSF-HiFiGAN in Rust, the pitch from the DAW's own tracker (`vocal::track_pitch`); no Python;
//! * an RVC WebUI install on this machine (its own Python, HuBERT, RMVPE; LingStationX's `tools/rvc_skin.py` drives it).
//!
//! * an audio clip: its recording is converted and the clip plays the result (the original is kept: `rvc.orig.<result path>` in the
//!   project meta, so Restore puts it back);
//! * a MIDI clip (a Ling Voice part singing its lyrics, or any instrument): the track is rendered alone over the clip, converted, and the
//!   result lands on a new audio track under it; the source track is muted.
//!
//! Models: `.pth` files with an optional `.index` beside them (`Name.pth` + `Name*.index`, or a folder `Name/` holding both), or an RVC
//! install's layout (`weights/Name.pth` + `logs/Name/added_*.index`).
use super::{Daw, A, Val};
use crate::model::*;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::{Arc, Mutex, OnceLock};

enum Target {
    Skin { t: usize, c: usize, orig: String },
    NewTrack { t: usize, start: f64, name: String },
}

enum Run {
    /// rendering the part alone (a copy of the project, off the interface thread); it starts the conversion when done and hands
    /// back the next stage; progress 0..1000, a cancel switch
    Prep(Option<std::thread::JoinHandle<Result<Run, String>>>, Arc<std::sync::atomic::AtomicU32>, Arc<std::sync::atomic::AtomicBool>),
    Proc(Child),
    /// the native conversion: its thread and its progress 0..100
    Native(Option<std::thread::JoinHandle<Result<(), String>>>, Arc<std::sync::atomic::AtomicU32>),
}

struct Job {
    run: Run,
    target: Target,
    inp: PathBuf,
    out: PathBuf,
    log: PathBuf,
    model: String,
    started: std::time::Instant,
}

/// everything needed to start a conversion (now, or after the background render of the part)
struct Launch {
    native: bool,
    py: PathBuf,
    rvc: PathBuf,
    script: PathBuf,
    model: PathBuf,
    index: String,
    semis: f64,
    method: String,
    index_rate: f64,
    protect: f64,
    mix: f64,
    cpu: bool,
    inp: PathBuf,
    out: PathBuf,
    log: PathBuf,
    id: u64,
}

impl Launch {
    fn start(&self) -> Result<Run, String> {
        let abs = |p: PathBuf| if p.is_absolute() { p } else { std::env::current_dir().map(|d| d.join(&p)).unwrap_or(p) };
        if self.native {
            #[cfg(feature = "llm")]
            {
                let hub = abs(hubert_for(&self.rvc).unwrap_or_default());
                let prog = Arc::new(std::sync::atomic::AtomicU32::new(0));
                let opts = ling_ai::rvc::Options { transpose: self.semis, rms_mix: self.mix.clamp(0.0, 1.0) as f32, speaker: 0, seed: self.id };
                let device = if self.cpu { -1 } else { 0 };
                let (p2, i2, o2, m2) = (prog.clone(), self.inp.clone(), self.out.clone(), self.model.clone());
                let h = std::thread::spawn(move || native_convert(&hub, &m2, &i2, &o2, &opts, device, &p2));
                return Ok(Run::Native(Some(h), prog));
            }
            #[cfg(not(feature = "llm"))]
            return Err("this ling.exe has no native engine".into());
        }
        let logf = std::fs::File::create(&self.log).map_err(|e| e.to_string())?;
        let mut cmd = std::process::Command::new(&self.py);
        cmd.arg(&self.script)
            .arg(&self.inp)
            .arg(&self.out)
            .arg(&self.model)
            .arg(if self.index.is_empty() { "-".to_string() } else { self.index.clone() })
            .arg(format!("{}", self.semis))
            .arg(if self.method.is_empty() { "rmvpe".to_string() } else { self.method.clone() })
            .arg(format!("{}", self.index_rate.clamp(0.0, 1.0)))
            .arg(format!("{}", self.protect.clamp(0.0, 0.5)))
            .arg(format!("{}", self.mix.clamp(0.0, 1.0)))
            .arg(if self.cpu { "1" } else { "0" })
            .current_dir(&self.rvc)
            .env("PYTHONUNBUFFERED", "1")
            .stdout(logf.try_clone().map_err(|e| e.to_string())?)
            .stderr(logf);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // no console window
        }
        let child = cmd.spawn().map_err(|e| format!("could not start {}: {e}", self.py.display()))?;
        Ok(Run::Proc(child))
    }
}

fn job() -> &'static Mutex<Option<Job>> {
    static J: OnceLock<Mutex<Option<Job>>> = OnceLock::new();
    J.get_or_init(|| Mutex::new(None))
}

/// the Python of an RVC install
pub fn rvc_python(dir: &Path) -> Option<PathBuf> {
    [".venv/Scripts/python.exe", "venv/Scripts/python.exe", "runtime/python.exe", ".venv/bin/python", "venv/bin/python"].iter().map(|p| dir.join(p)).find(|p| p.is_file())
}

fn stem(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// the index file that belongs with a model: next to it, in a folder of its name, or in an install's logs/<name>/ ("added_" first)
fn index_for(pth: &Path) -> Option<PathBuf> {
    let name = stem(pth).to_lowercase();
    let mut dirs: Vec<PathBuf> = vec![];
    if let Some(d) = pth.parent() {
        dirs.push(d.to_path_buf());
        dirs.push(d.join(stem(pth)));
        if let Some(root) = d.parent() {
            dirs.push(root.join("logs").join(stem(pth)));
        }
    }
    let mut found: Vec<PathBuf> = vec![];
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let f = p.file_name().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
            // beside the model the index must carry the model's name; in its own folder any index is its index
            let own_folder = d.file_name().map(|s| s.to_string_lossy().to_lowercase() == name).unwrap_or(false);
            if f.ends_with(".index") && (own_folder || f.contains(&name)) {
                found.push(p);
            }
        }
        if !found.is_empty() {
            break;
        }
    }
    found.sort_by_key(|p| !p.file_name().map(|s| s.to_string_lossy().contains("added")).unwrap_or(false));
    found.into_iter().next()
}

/// every model in `dirs`: (name, model path, index path or "")
fn scan(dirs: &[PathBuf]) -> Vec<(String, String, String)> {
    let mut out: Vec<(String, String, String)> = vec![];
    let add = |p: PathBuf, out: &mut Vec<(String, String, String)>| {
        let n = stem(&p);
        // training checkpoints (G_/D_) are not voices
        if n.starts_with("G_") || n.starts_with("D_") || out.iter().any(|x| x.0.eq_ignore_ascii_case(&n)) {
            return;
        }
        let idx = index_for(&p).map(|i| i.to_string_lossy().into_owned()).unwrap_or_default();
        out.push((n, p.to_string_lossy().into_owned(), idx));
    };
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(d) else { continue };
        let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if p.is_file() && p.extension().map(|e| e.eq_ignore_ascii_case("pth")).unwrap_or(false) {
                add(p, &mut out);
            } else if p.is_dir() {
                // a folder per voice
                if let Ok(sub) = std::fs::read_dir(&p) {
                    for s in sub.flatten() {
                        let sp = s.path();
                        if sp.extension().map(|e| e.eq_ignore_ascii_case("pth")).unwrap_or(false) {
                            add(sp, &mut out);
                        }
                    }
                }
            }
        }
    }
    out
}

/// the loaded native converter, kept for the next job with the same files
#[cfg(feature = "llm")]
fn native_cache() -> &'static Mutex<Option<(PathBuf, PathBuf, Arc<ling_ai::rvc::Rvc>)>> {
    static C: OnceLock<Mutex<Option<(PathBuf, PathBuf, Arc<ling_ai::rvc::Rvc>)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

/// the HuBERT model the native engine reads: in the RVC folder, or in models/rvc
fn hubert_for(rvc: &Path) -> Option<PathBuf> {
    [rvc.join("hubert_base.pt"), PathBuf::from("models/rvc/hubert_base.pt")].into_iter().find(|p| p.is_file())
}

#[cfg(feature = "llm")]
fn native_convert(hub: &Path, model: &Path, inp: &Path, out: &Path, opts: &ling_ai::rvc::Options, device: i64, prog: &std::sync::atomic::AtomicU32) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    prog.store(2, Ordering::Relaxed);
    let rvc = {
        let mut c = native_cache().lock().unwrap_or_else(|p| p.into_inner());
        match c.as_ref() {
            Some((h, m, r)) if h == hub && m == model => r.clone(),
            _ => {
                let r = Arc::new(ling_ai::rvc::Rvc::load(hub, model, device)?);
                *c = Some((hub.to_path_buf(), model.to_path_buf(), r.clone()));
                r
            }
        }
    };
    prog.store(10, Ordering::Relaxed);
    let s = load_sample(&inp.to_string_lossy())?;
    let mono: Vec<f32> = s.stereo.chunks_exact(2).map(|f| (f[0] + f[1]) * 0.5).collect();
    let a16 = ling_ai::whisper::resample_to_16k(&mono, s.rate);
    // the pitch every 10 ms (the tracker gives every 5 ms)
    let p = crate::vocal::track_pitch(&a16, 16000.0);
    let step = (0.01 / p.hop).round().max(1.0) as usize;
    let f0: Vec<f32> = (0..a16.len() / 160).map(|j| p.f0.get(j * step).copied().unwrap_or(0.0)).collect();
    prog.store(20, Ordering::Relaxed);
    let y = rvc.convert(&a16, &f0, opts, &mut |f| prog.store((20.0 + f * 75.0) as u32, Ordering::Relaxed))?;
    let stereo: Vec<f32> = y.iter().flat_map(|x| [*x, *x]).collect();
    write_wav(out, &stereo, rvc.voice.sr)?;
    prog.store(100, Ordering::Relaxed);
    Ok(())
}

fn write_wav(path: &Path, stereo: &[f32], rate: u32) -> Result<(), String> {
    let spec = hound::WavSpec { channels: 2, sample_rate: rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;
    for s in stereo {
        w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16).map_err(|e| e.to_string())?;
    }
    w.finalize().map_err(|e| e.to_string())
}

/// the last lines of the job log (the reason a conversion failed)
fn log_tail(p: &Path, n: usize) -> String {
    let t = std::fs::read_to_string(p).unwrap_or_default();
    let lines: Vec<&str> = t.lines().filter(|l| !l.trim().is_empty() && !l.contains("| INFO |") && !l.contains("FutureWarning")).collect();
    lines[lines.len().saturating_sub(n)..].join(" / ")
}

impl Daw {
    fn rvc_dir(&self) -> PathBuf {
        if self.path.is_empty() {
            PathBuf::from("cache").join("rvc")
        } else {
            Path::new(&self.path).parent().map(|p| p.join("audio").join("rvc")).unwrap_or_else(|| PathBuf::from("cache").join("rvc"))
        }
    }

    /// apply a finished job: the clip plays the converted voice, or a new track carries it
    fn rvc_apply(&mut self, j: Job) -> Result<String, String> {
        let s = load_sample(&j.out.to_string_lossy())?;
        let _ = self.dispatch("daw_undo_push", &A(&[]));
        let eng = self.out.engine.clone();
        let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
        let bpm = en.project.bpm.max(20.0);
        let path = s.path.clone();
        match j.target {
            Target::Skin { t, c, orig } => {
                let Some(clip) = en.project.tracks.get_mut(t).and_then(|x| x.clips.get_mut(c)) else { return Err("the clip is gone".into()) };
                let ClipData::Audio { sample, .. } = &mut clip.data else { return Err("the clip is no longer audio".into()) };
                let named = Arc::new(Sample::new(format!("{} ({})", sample.name.split(" (").next().unwrap_or(&sample.name), j.model), path.clone(), s.rate, s.stereo.clone()));
                *sample = named.clone();
                let key = format!("rvc.orig.{path}");
                match en.project.meta.iter_mut().find(|(k, _)| *k == key) {
                    Some(e) => e.1 = orig,
                    None => en.project.meta.push((key, orig)),
                }
                drop(en);
                self.samples.insert(path, named);
                self.touch();
                Ok(format!("Voice skin: the clip now sings with {} ({:.0} s)", j.model, j.started.elapsed().as_secs_f64()))
            }
            Target::NewTrack { t, start, name } => {
                let n = en.project.tracks.len();
                if n >= MAX_TRACKS {
                    return Err("no room for another track".into());
                }
                let col = en.project.tracks.get(t).map(|x| x.color).unwrap_or(0x9A53D6);
                let mut tr = Track::new(&format!("{name} ({})", j.model), TrackKind::Audio, col, None);
                let len = (s.seconds() * bpm / 60.0).max(0.25);
                let named = Arc::new(Sample::new(format!("{name} ({})", j.model), path.clone(), s.rate, s.stereo.clone()));
                tr.clips.push(Clip { start, len, name: format!("{name} ({})", j.model), loop_len: 0.0, lyric_text: String::new(), lyric_lang: String::new(), tag: String::new(), data: ClipData::Audio { sample: named.clone(), offset_s: 0.0, gain: 1.0, pitch: 0.0, warp: 0.0 } });
                tr.volume = en.project.tracks.get(t).map(|x| x.volume).unwrap_or(0.9);
                en.project.tracks.push(tr);
                if let Some(src) = en.project.tracks.get_mut(t) {
                    src.mute = true;
                }
                drop(en);
                self.samples.insert(path, named);
                self.touch();
                Ok(format!("Voice skin: {name} sung by {} on a new track (the part is muted) ({:.0} s)", j.model, j.started.elapsed().as_secs_f64()))
            }
        }
    }

    pub(super) fn rvc_dispatch(&mut self, name: &str, a: &A) -> Result<Val, String> {
        let eng = self.out.engine.clone();
        macro_rules! ok {
            ($v:expr) => {
                return Ok(Val::from($v))
            };
        }
        match name {
            "daw_rvc_info" => {
                // (RVC folder) -> [python ("" = not an RVC install), HuBERT 0/1, RMVPE 0/1]
                let d = PathBuf::from(a.s(0));
                let py = rvc_python(&d).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                let ok_pipe = d.join("vc_infer_pipeline.py").is_file() || d.join("infer").is_dir();
                // [3]: the native engine can run (this build has it and a HuBERT is there)
                let native = cfg!(feature = "llm") && hubert_for(&d).is_some();
                ok!(Val::List(vec![Val::Str(if ok_pipe { py } else { String::new() }), Val::Num(d.join("hubert_base.pt").is_file() as u8 as f64), Val::Num(d.join("rmvpe.pt").is_file() as u8 as f64), Val::Num(native as u8 as f64)]))
            }
            "daw_rvc_models" => {
                // (folders separated by ';') -> [name, model path, index path or ""] * N
                let dirs: Vec<PathBuf> = a.s(0).split(';').filter(|s| !s.trim().is_empty()).map(|s| PathBuf::from(s.trim())).collect();
                let mut v = vec![];
                for (n, p, i) in scan(&dirs) {
                    v.extend([Val::Str(n), Val::Str(p), Val::Str(i)]);
                }
                ok!(Val::List(v))
            }
            "daw_rvc_start" => {
                // (track, clip (-1 = the whole track), RVC folder, script, model, index, semitones, method, index rate, protect, volume mix, cpu 0/1)
                // -> true when started
                let (t, c) = (a.u(0), a.u(1));
                let rvc = PathBuf::from(a.s(2));
                if job().lock().unwrap_or_else(|p| p.into_inner()).is_some() {
                    self.status = "Voice skin: a conversion is already running".into();
                    ok!(false)
                }
                // script "native": the Rust engine (no Python); otherwise the RVC install's Python runs tools/rvc_skin.py
                let native = a.s(3) == "native";
                let py = if native {
                    PathBuf::new()
                } else {
                    let Some(py) = rvc_python(&rvc) else {
                        self.status = format!("Voice skin: no RVC install (with its Python) in {}", rvc.display());
                        ok!(false)
                    };
                    py
                };
                if native && (!cfg!(feature = "llm") || hubert_for(&rvc).is_none()) {
                    self.status = if cfg!(feature = "llm") { "Voice skin: hubert_base.pt not found (put it in models/rvc or the RVC folder)".into() } else { "Voice skin: this ling.exe has no native engine (build with --features llm)".into() };
                    ok!(false)
                }
                let model = PathBuf::from(a.s(4));
                if !model.is_file() {
                    self.status = format!("Voice skin: model not found ({})", model.display());
                    ok!(false)
                }
                let id = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() % 100_000_000).unwrap_or(0);
                // RVC runs in its own folder: every path it gets is absolute
                let abs = |p: PathBuf| if p.is_absolute() { p } else { std::env::current_dir().map(|d| d.join(&p)).unwrap_or(p) };
                let work = abs(self.rvc_dir());
                std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
                let model = abs(model);
                let model_name = stem(&model);
                let inp = work.join(format!("rvc_{id}_in.wav"));
                let out = work.join(format!("rvc_{id}_{}.wav", model_name.replace(' ', "_")));
                // what is converted: an audio clip's recording, or the track rendered alone over a MIDI clip (that render runs in the
                // background job, from a copy of the project, so neither the interface nor the audio waits for it)
                let mut prep: Option<(crate::project::ProjectDoc, f32, f64, f64)> = None;
                let target = {
                    let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    let Some(tr) = en.project.tracks.get(t) else { ok!(false) };
                    // clip -1: the whole track (every MIDI clip on it), rendered alone and sung as one take
                    let whole = a.n(1) < 0.0;
                    let span = if whole && !tr.clips.is_empty() {
                        Some((tr.clips.iter().map(|k| k.start).fold(f64::MAX, f64::min), tr.clips.iter().map(|k| k.end()).fold(0.0, f64::max)))
                    } else {
                        None
                    };
                    let Some(clip) = tr.clips.get(if whole { 0 } else { c }) else { ok!(false) };
                    match &clip.data {
                        _ if span.is_some() => {
                            let (start, end) = span.unwrap();
                            let tname = tr.name.clone();
                            prep = Some((crate::project::to_doc(&en), en.sr, start, end));
                            drop(en);
                            Target::NewTrack { t, start, name: tname }
                        }
                        ClipData::Audio { sample, .. } => {
                            // a clip skinned before is converted from its original recording again
                            let cur = sample.clone();
                            let key = format!("rvc.orig.{}", cur.path);
                            let orig = en.project.meta.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone()).unwrap_or_else(|| cur.path.clone());
                            drop(en);
                            if orig != cur.path {
                                let o = load_sample(&orig)?;
                                write_wav(&inp, &o.stereo, o.rate)?;
                            } else {
                                write_wav(&inp, &cur.stereo, cur.rate)?;
                            }
                            Target::Skin { t, c, orig }
                        }
                        ClipData::Midi(_) => {
                            let (start, end, tname) = (clip.start, clip.end(), tr.name.clone());
                            prep = Some((crate::project::to_doc(&en), en.sr, start, end));
                            drop(en);
                            Target::NewTrack { t, start, name: tname }
                        }
                    }
                };
                let log = work.join(format!("rvc_{id}.log"));
                let launch = Launch {
                    native,
                    py: py.clone(),
                    rvc: rvc.clone(),
                    script: abs(PathBuf::from(a.s(3))),
                    model: model.clone(),
                    index: a.s(5),
                    semis: a.n(6).round(),
                    method: a.s(7),
                    index_rate: a.n(8),
                    protect: a.n(9),
                    mix: a.n(10),
                    cpu: a.n(11) > 0.5,
                    inp: inp.clone(),
                    out: out.clone(),
                    log: log.clone(),
                    id: id as u64,
                };
                if let Some((doc, sr, start, end)) = prep {
                    let solo = match &target {
                        Target::NewTrack { t, .. } => *t,
                        Target::Skin { t, .. } => *t,
                    };
                    let base = Path::new(&self.path).parent().map(|p| p.to_path_buf());
                    let mut samples = self.samples.clone();
                    let prog = Arc::new(std::sync::atomic::AtomicU32::new(0));
                    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
                    let (p2, c2) = (prog.clone(), cancel.clone());
                    let h = std::thread::spawn(move || -> Result<Run, String> {
                        let mut eng = crate::engine::Engine::new(sr);
                        crate::project::apply_doc(&mut eng, &doc, base.as_deref(), &mut samples);
                        for (i, x) in eng.project.tracks.iter_mut().enumerate() {
                            x.solo = i == solo;
                            if i == solo {
                                x.mute = false;
                            }
                        }
                        let Some(data) = eng.bounce_range_progress(start, end, 0.6, &p2, &c2, 0, 1000) else { return Err("cancelled".into()) };
                        write_wav(&launch.inp, &data, sr as u32)?;
                        if c2.load(std::sync::atomic::Ordering::Relaxed) {
                            return Err("cancelled".into());
                        }
                        launch.start()
                    });
                    *job().lock().unwrap_or_else(|p| p.into_inner()) = Some(Job { run: Run::Prep(Some(h), prog, cancel), target, inp, out, log, model: model_name.clone(), started: std::time::Instant::now() });
                    self.status = format!("Voice skin: rendering the part, then {model_name} sings it ...");
                    ok!(true)
                }
                let run = launch.start()?;
                *job().lock().unwrap_or_else(|p| p.into_inner()) = Some(Job { run, target, inp, out, log, model: model_name.clone(), started: std::time::Instant::now() });
                self.status = format!("Voice skin: converting with {model_name} ...");
                ok!(true)
            }
            "daw_singer_start" => {
                // (track, MRVC folder, voice model name, singing model folder, gpu) -> true when started. Ling Singer: the track's notes
                // and their IPA lyrics become a score that tools/singer/sing.py sings (the singing model -> HuBERT features -> the RVC
                // voice); the result lands on a new audio track at the first note, as a Voice Skin of a MIDI part does (daw_rvc_poll)
                let t = a.u(0);
                if job().lock().unwrap_or_else(|p| p.into_inner()).is_some() {
                    self.status = "Ling Singer: a voice job is already running".into();
                    ok!(false)
                }
                let abs = |p: PathBuf| if p.is_absolute() { p } else { std::env::current_dir().map(|d| d.join(&p)).unwrap_or(p) };
                let mrvc = abs(PathBuf::from(a.s(1)));
                let name = a.s(2);
                let singer = abs(PathBuf::from(a.s(3)));
                let model = mrvc.join("weights").join(format!("{name}.pth"));
                if !model.is_file() || !singer.join("singer.pt").is_file() {
                    self.status = format!("Ling Singer: not trained yet (needs {} and {})", model.display(), singer.join("singer.pt").display());
                    ok!(false)
                }
                let script = abs(PathBuf::from("tools/singer/sing.py"));
                if !script.is_file() {
                    self.status = "Ling Singer: tools/singer/sing.py is missing".into();
                    ok!(false)
                }
                let py = rvc_python(&mrvc).unwrap_or_else(|| PathBuf::from("python"));
                // the score: every note of the track (absolute beats), relative to the first one
                let (notes, first, bpm, tname) = {
                    let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    let Some(tr) = en.project.tracks.get(t) else { ok!(false) };
                    let mut v: Vec<(f64, f64, u8, f32, String)> = vec![];
                    for c in &tr.clips {
                        if let ClipData::Midi(ns) = &c.data {
                            for n in ns.iter().filter(|n| n.start < c.len) {
                                v.push((c.start + n.start, n.len.min(c.len - n.start), n.key, n.vel, n.lyric.clone()));
                            }
                        }
                    }
                    let first = v.iter().map(|x| x.0).fold(f64::MAX, f64::min);
                    (v, first, en.project.bpm, tr.name.clone())
                };
                if notes.is_empty() {
                    self.status = "Ling Singer: the track has no notes".into();
                    ok!(false)
                }
                let score = serde_json::json!({
                    "bpm": bpm,
                    "notes": notes.iter().map(|(s0, l, k, v, ly)| serde_json::json!({"start": s0 - first, "len": l, "key": k, "vel": v, "lyric": ly})).collect::<Vec<_>>(),
                });
                let id = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() % 100_000_000).unwrap_or(0);
                let work = abs(self.rvc_dir());
                std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
                let inp = work.join(format!("singer_{id}.json"));
                let out = work.join(format!("singer_{id}.wav"));
                let log = work.join(format!("singer_{id}.log"));
                std::fs::write(&inp, score.to_string()).map_err(|e| e.to_string())?;
                let logf = std::fs::File::create(&log).map_err(|e| e.to_string())?;
                let mut cmd = std::process::Command::new(&py);
                cmd.arg(&script)
                    .args(["--score", &inp.to_string_lossy(), "--out", &out.to_string_lossy(), "--mrvc", &mrvc.to_string_lossy(), "--name", &name, "--singer", &singer.to_string_lossy(), "--gpu", &format!("{}", a.n(4).max(0.0) as u32)])
                    .current_dir(&mrvc)
                    .env("PYTHONUNBUFFERED", "1")
                    .env("PYTHONIOENCODING", "utf-8")
                    // the RVC weights file carries its config next to the tensors
                    .env("TORCH_FORCE_NO_WEIGHTS_ONLY_LOAD", "1")
                    .stdout(logf.try_clone().map_err(|e| e.to_string())?)
                    .stderr(logf);
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    cmd.creation_flags(0x0800_0000);
                }
                let child = match cmd.spawn() {
                    Ok(c) => c,
                    Err(e) => {
                        self.status = format!("Ling Singer: could not start {}: {e}", py.display());
                        ok!(false)
                    }
                };
                *job().lock().unwrap_or_else(|p| p.into_inner()) = Some(Job { run: Run::Proc(child), target: Target::NewTrack { t, start: first, name: tname }, inp, out, log, model: name.clone(), started: std::time::Instant::now() });
                self.status = format!("Ling Singer: singing {} notes with {name}", notes.len());
                ok!(true)
            }
            "daw_singer_singtts" => {
                // (track, reference clips "a.wav|b.wav|c.wav", package folder, python, seed, language) -> true when started. SingTTS sings the
                // track: every clip's notes with the syllables its lyrics were fitted with, as written (lyricfit::fit_text with the clip's
                // own text and options) - "~" holds the vowel, a word over several notes carries word_end on its last; the package's
                // sing_onnx.py (onnxruntime, no torch) renders it in the voice of the clips (pooled), onto a new audio track at the first note
                let t = a.u(0);
                if job().lock().unwrap_or_else(|p| p.into_inner()).is_some() {
                    self.status = "SingTTS: a voice job is already running".into();
                    ok!(false)
                }
                let abs = |p: PathBuf| if p.is_absolute() { p } else { std::env::current_dir().map(|d| d.join(&p)).unwrap_or(p) };
                let refs: Vec<String> = a.s(1).split('|').map(|r| r.trim().to_string()).filter(|r| !r.is_empty() && Path::new(r).is_file()).map(|r| abs(PathBuf::from(r)).to_string_lossy().into_owned()).collect();
                if refs.is_empty() {
                    self.status = "SingTTS: choose a voice clip (a few seconds of clean singing or speech)".into();
                    ok!(false)
                }
                let pkg = abs(PathBuf::from(if a.s(2).is_empty() { "models/singtts".to_string() } else { a.s(2) }));
                let script = pkg.join("sing_onnx.py");
                if !script.is_file() {
                    self.status = format!("SingTTS: no sing_onnx.py in {}", pkg.display());
                    ok!(false)
                }
                let py = if a.s(3).is_empty() { PathBuf::from("python") } else { PathBuf::from(a.s(3)) };
                let lang = if a.s(5).is_empty() { "en".to_string() } else { a.s(5) };
                const LEAD: f64 = 0.15;
                // the notes and their written syllables, clip by clip
                let (notes, first, bpm, tname, missing) = {
                    let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    let Some(tr) = en.project.tracks.get(t) else { ok!(false) };
                    let mut v: Vec<(f64, f64, u8, f32, String)> = vec![];
                    let mut missing = 0usize;
                    for c in &tr.clips {
                        let ClipData::Midi(ns) = &c.data else { continue };
                        let ns: Vec<&crate::model::Note> = ns.iter().filter(|n| n.start < c.len).collect();
                        if c.lyric_text.trim().is_empty() {
                            missing += ns.len();
                            continue;
                        }
                        let opts = crate::lyricfit::Opts::parse(&c.lyric_lang).unwrap_or_default();
                        let input: Vec<crate::lyricfit::NoteIn> = ns.iter().map(|n| crate::lyricfit::NoteIn { start: n.start, len: n.len, key: n.key }).collect();
                        let words = crate::lyricfit::fit_text(&input, &c.lyric_text, &opts);
                        for (n, w) in ns.iter().zip(words) {
                            if w.is_empty() {
                                continue;
                            }
                            v.push((c.start + n.start, n.len.min(c.len - n.start), n.key, n.vel, w));
                        }
                    }
                    v.sort_by(|x, y| x.0.total_cmp(&y.0));
                    let first = v.first().map(|x| x.0).unwrap_or(0.0);
                    (v, first, en.project.bpm, tr.name.clone(), missing)
                };
                if notes.is_empty() {
                    self.status = if missing > 0 { "SingTTS: fit lyrics to the notes first (the clips have no lyric text)".into() } else { "SingTTS: the track has no notes".into() };
                    ok!(false)
                }
                // a written unit: "\n ly-ric" -> starts a word when it had a leading space; "~" holds
                let starts_word = |w: &str| w.trim_start_matches('\n').starts_with(' ');
                let mut out_notes = vec![];
                for (i, (s0, l, k, vel, w)) in notes.iter().enumerate() {
                    let held = w.trim() == "~";
                    let text: String = w.trim().replace('-', "");
                    // the word ends here when the next sung unit starts a new word (or there is none)
                    let next = notes[i + 1..].iter().find(|x| x.4.trim() != "~");
                    let word_end = !held && next.map(|x| starts_word(&x.4)).unwrap_or(true);
                    out_notes.push(serde_json::json!({"start": s0 - first, "len": l, "key": k, "vel": vel, "lyric": if held { "~".to_string() } else { text }, "word_end": word_end}));
                }
                let score = serde_json::json!({"bpm": bpm, "lang": lang, "notes": out_notes});
                let id = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() % 100_000_000).unwrap_or(0);
                let work = abs(self.rvc_dir());
                std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
                let inp = work.join(format!("singtts_{id}.json"));
                let out = work.join(format!("singtts_{id}.wav"));
                let log = work.join(format!("singtts_{id}.log"));
                std::fs::write(&inp, score.to_string()).map_err(|e| e.to_string())?;
                let logf = std::fs::File::create(&log).map_err(|e| e.to_string())?;
                let mut cmd = std::process::Command::new(&py);
                // sing_onnx.py SCORE OUT.wav --ref A [--ref B --ref C] --seed S --lead L: L seconds of room before beat 0 so a first word's
                // consonants are not cut; the audio track starts that much earlier
                cmd.arg(&script).arg(&inp).arg(&out).args(refs.iter().flat_map(|r| ["--ref".to_string(), r.clone()])).args(["--seed", &format!("{}", a.n(4).max(0.0) as u64), "--lead", &format!("{LEAD}")]).current_dir(&pkg).env("PYTHONUNBUFFERED", "1").env("PYTHONIOENCODING", "utf-8").stdout(logf.try_clone().map_err(|e| e.to_string())?).stderr(logf);
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    cmd.creation_flags(0x0800_0000);
                }
                let child = match cmd.spawn() {
                    Ok(c) => c,
                    Err(e) => {
                        self.status = format!("SingTTS: could not start {}: {e}", py.display());
                        ok!(false)
                    }
                };
                let lead_beats = LEAD * bpm / 60.0;
                *job().lock().unwrap_or_else(|p| p.into_inner()) = Some(Job { run: Run::Proc(child), target: Target::NewTrack { t, start: (first - lead_beats).max(0.0), name: tname }, inp, out, log, model: "SingTTS".into(), started: std::time::Instant::now() });
                self.status = format!("SingTTS: singing {} notes{}", notes.len(), if missing > 0 { format!(" ({missing} notes in clips without lyrics are left out)") } else { String::new() });
                ok!(true)
            }
            "daw_rvc_poll" => {
                // () -> [state (0 nothing, 1 running, 2 just finished and applied, -1 failed), message, seconds, progress 0..100]
                let mut g = job().lock().unwrap_or_else(|p| p.into_inner());
                let Some(j) = g.as_mut() else { ok!(Val::List(vec![Val::Num(0.0), Val::Str(String::new()), Val::Num(0.0), Val::Num(0.0)])) };
                let secs = j.started.elapsed().as_secs_f64();
                // where the job is: (still running? progress, what it does) and, when it ended, success or the reason
                // the prep stage: rendering the part; when it is done its thread has started the conversion - the job moves on
                if let Run::Prep(h, prog, _) = &mut j.run {
                    if h.as_ref().map_or(false, |h| !h.is_finished()) {
                        let pct = prog.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1000.0 * 15.0;
                        ok!(Val::List(vec![Val::Num(1.0), Val::Str(format!("rendering the part alone ({:.0} %)", pct / 15.0 * 100.0)), Val::Num(secs), Val::Num(pct)]))
                    }
                    let r = h.take().map_or(Err("the render stopped".into()), |h| h.join().unwrap_or_else(|_| Err("the render crashed".into())));
                    match r {
                        Ok(next) => {
                            j.run = next;
                            ok!(Val::List(vec![Val::Num(1.0), Val::Str("starting the voice model".into()), Val::Num(secs), Val::Num(15.0)]))
                        }
                        Err(e) => {
                            let j = g.take().unwrap();
                            drop(g);
                            let _ = std::fs::remove_file(&j.inp);
                            let m = format!("Voice skin failed: {e}");
                            self.status = m.clone();
                            ok!(Val::List(vec![Val::Num(-1.0), Val::Str(m), Val::Num(secs), Val::Num(0.0)]))
                        }
                    }
                }
                let (done, pct, what): (Option<Result<(), String>>, f64, String) = match &mut j.run {
                    Run::Proc(child) => {
                        let log = std::fs::read_to_string(&j.log).unwrap_or_default();
                        let prog = log.lines().rev().find_map(|l| l.strip_prefix("PROGRESS ")).unwrap_or("0 starting").to_string();
                        let (pct, what) = prog.split_once(' ').map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or((prog.clone(), String::new()));
                        let done = match child.try_wait() {
                            Ok(None) => None,
                            Ok(Some(st)) => Some(if st.success() { Ok(()) } else { Err(log_tail(&j.log, 3)) }),
                            Err(e) => Some(Err(e.to_string())),
                        };
                        (done, pct.parse().unwrap_or(0.0), what)
                    }
                    Run::Prep(..) => (None, 0.0, String::new()),
                    Run::Native(h, prog) => {
                        let pct = prog.load(std::sync::atomic::Ordering::Relaxed) as f64;
                        (if h.as_ref().map_or(true, |h| h.is_finished()) { Some(Ok(())) } else { None }, pct, if pct < 10.0 { "loading the voice model".into() } else if pct < 20.0 { "tracking the pitch".into() } else { "converting (native)".into() })
                    }
                };
                match done {
                    None => ok!(Val::List(vec![Val::Num(1.0), Val::Str(what), Val::Num(secs), Val::Num(pct)])),
                    Some(res) => {
                        let mut j = g.take().unwrap();
                        drop(g);
                        // a native thread's own answer
                        let res = match &mut j.run {
                            Run::Native(h, _) => h.take().map_or(Ok(()), |h| h.join().unwrap_or_else(|_| Err("the conversion crashed".into()))),
                            Run::Proc(_) | Run::Prep(..) => res,
                        };
                        let reason = res.as_ref().err().cloned().unwrap_or_default();
                        // the copy of the vocal that was sent is not needed any more
                        let _ = std::fs::remove_file(&j.inp);
                        if res.is_ok() && j.out.is_file() {
                            let _ = std::fs::remove_file(&j.log);
                            match self.rvc_apply(j) {
                                Ok(m) => {
                                    self.status = m.clone();
                                    ok!(Val::List(vec![Val::Num(2.0), Val::Str(m), Val::Num(secs), Val::Num(100.0)]))
                                }
                                Err(e) => {
                                    self.status = format!("Voice skin failed: {e}");
                                    ok!(Val::List(vec![Val::Num(-1.0), Val::Str(self.status.clone()), Val::Num(secs), Val::Num(0.0)]))
                                }
                            }
                        }
                        let m = format!("Voice skin failed: {}", if reason.is_empty() { log_tail(&j.log, 3) } else { reason });
                        self.status = m.clone();
                        ok!(Val::List(vec![Val::Num(-1.0), Val::Str(m), Val::Num(secs), Val::Num(0.0)]))
                    }
                }
            }
            "daw_rvc_cancel" => {
                if let Some(j) = job().lock().unwrap_or_else(|p| p.into_inner()).take() {
                    // a process is stopped; a native thread finishes on its own and its result is dropped
                    match j.run {
                        Run::Proc(mut child) => {
                            let _ = child.kill();
                            let _ = child.wait();
                        }
                        // the render stops at its next block and starts nothing
                        Run::Prep(_, _, cancel) => cancel.store(true, std::sync::atomic::Ordering::Relaxed),
                        Run::Native(..) => {}
                    }
                    for p in [&j.inp, &j.out, &j.log] {
                        let _ = std::fs::remove_file(p);
                    }
                    self.status = "Voice skin: cancelled".into();
                }
                ok!(true)
            }
            "daw_rvc_original" => {
                // (track, clip) -> the recording a skinned clip came from, "" when the clip is not skinned
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let Some(Clip { data: ClipData::Audio { sample, .. }, .. }) = en.project.tracks.get(a.u(0)).and_then(|t| t.clips.get(a.u(1))) else { ok!("") };
                let key = format!("rvc.orig.{}", sample.path);
                ok!(en.project.meta.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone()).unwrap_or_default())
            }
            "daw_rvc_restore" => {
                // (track, clip) -> true: a skinned clip plays its original recording again
                let orig = {
                    let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    let Some(Clip { data: ClipData::Audio { sample, .. }, .. }) = en.project.tracks.get(a.u(0)).and_then(|t| t.clips.get(a.u(1))) else { ok!(false) };
                    let key = format!("rvc.orig.{}", sample.path);
                    en.project.meta.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone())
                };
                let Some(orig) = orig else { ok!(false) };
                let s = self.load_sample_cached(&orig)?;
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                if let Some(Clip { data: ClipData::Audio { sample, .. }, .. }) = en.project.tracks.get_mut(a.u(0)).and_then(|t| t.clips.get_mut(a.u(1))) {
                    *sample = s;
                }
                drop(en);
                self.touch();
                ok!(true)
            }
            _ => Err(format!("unknown builtin {name}")),
        }
    }
}
