// src/runtime/stt.rs — speech to text for ling-lang, through faster-whisper on this machine (no network, no hosted service).
//
// The model runs in Python (faster-whisper / CTranslate2, CPU or CUDA) in a child process started from `stt/ling_stt.py`, which is embedded
// in the binary and written to a per-user folder on first use. Jobs run on a background thread and are polled, like the llm_* builtins, so
// a long transcription never blocks the interpreter.
//
// Builtins (see runtime/mod.rs):
//   stt_available()                          -> bool       faster-whisper can be started
//   stt_models()                             -> [name, size_mb, ...]   models found in the local Hugging Face cache
//   stt_start(audio, model, lang, words)     -> job id ("" when it cannot start); lang "auto" or "en", "ja" ...; words true = word timestamps
//   stt_poll(job)                            -> progress 0..1
//   stt_done(job)                            -> bool
//   stt_error(job)                           -> "" or the reason it failed
//   stt_info(job)                            -> "loaded small on cuda"
//   stt_language(job)                        -> detected language code
//   stt_segments(job)                        -> [start, end, text, ...] of the segments so far
//   stt_words(job)                           -> [start, end, word, probability, ...] of the words so far
//   stt_srt(job, max_chars)                  -> SubRip text built from the words (cues of at most max_chars characters; 0 = whole segments)
//   stt_cancel(job)                          -> stops it
//
// Setup (once): `python -m venv <dir>` and `pip install faster-whisper`, then download a model into the Hugging Face cache
// (`huggingface-cli download Systran/faster-whisper-small`). LING_STT_PYTHON names the interpreter to use; otherwise
// %LOCALAPPDATA%\LingStation\stt\venv (Windows) / ~/.local/share/lingstation/stt/venv, then `python`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

const SCRIPT: &str = include_str!("../../stt/ling_stt.py");

#[derive(Default)]
struct JobState {
    progress: f64,
    done: bool,
    error: String,
    info: String,
    language: String,
    /// (start, end, text)
    segments: Vec<(f64, f64, String)>,
    /// (start, end, word, probability)
    words: Vec<(f64, f64, String, f64)>,
    pid: Option<u32>,
    cancelled: bool,
    /// set to stop an in-process (native) transcription
    flag: Option<Arc<std::sync::atomic::AtomicBool>>,
}

static JOBS: Mutex<Option<HashMap<String, Arc<Mutex<JobState>>>>> = Mutex::new(None);
static AVAILABLE: Mutex<Option<(String, bool)>> = Mutex::new(None);

fn data_dir() -> std::path::PathBuf {
    #[cfg(windows)]
    {
        if let Some(d) = std::env::var_os("LOCALAPPDATA") {
            return std::path::PathBuf::from(d).join("LingStation").join("stt");
        }
    }
    if let Some(h) = std::env::var_os("HOME") {
        return std::path::PathBuf::from(h).join(".local").join("share").join("lingstation").join("stt");
    }
    std::env::temp_dir().join("lingstation-stt")
}

/// the interpreter that has faster-whisper
fn python() -> String {
    if let Some(p) = std::env::var_os("LING_STT_PYTHON") {
        return p.to_string_lossy().into_owned();
    }
    let venv = data_dir().join("venv");
    for rel in ["Scripts/python.exe", "bin/python"] {
        let p = venv.join(rel);
        if p.is_file() {
            return p.to_string_lossy().into_owned();
        }
    }
    if cfg!(windows) { "python".into() } else { "python3".into() }
}

fn script_path() -> Option<std::path::PathBuf> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let p = dir.join("ling_stt.py");
    // keep the copy current with the one inside the binary
    if std::fs::read_to_string(&p).map(|s| s != SCRIPT).unwrap_or(true) {
        std::fs::write(&p, SCRIPT).ok()?;
    }
    Some(p)
}

fn command(py: &str) -> Command {
    let mut c = Command::new(py);
    c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // no console window
    }
    c
}

/// Whisper models the in-process engine can use (CTranslate2 `model.bin` folders), as (name, MB)
#[cfg(feature = "llm")]
fn native_models() -> Vec<(String, f64)> {
    ling_ai::whisper::find_models()
        .into_iter()
        .map(|(n, p)| {
            let mb = std::fs::metadata(p.join("model.bin")).map(|m| m.len() as f64 / 1048576.0).unwrap_or(0.0);
            (n, mb.round())
        })
        .collect()
}
#[cfg(not(feature = "llm"))]
fn native_models() -> Vec<(String, f64)> {
    vec![]
}

pub fn stt_available() -> bool {
    if !native_models().is_empty() {
        return true;
    }
    let py = python();
    if let Ok(g) = AVAILABLE.lock() {
        if let Some((p, ok)) = g.as_ref() {
            if *p == py && *ok {
                return true;
            }
        }
    }
    let ok = command(&py).args(["-c", "import faster_whisper"]).output().map(|o| o.status.success()).unwrap_or(false);
    if let Ok(mut g) = AVAILABLE.lock() {
        *g = Some((py, ok));
    }
    ok
}

/// [name, size in MB, ...]
pub fn stt_models() -> Vec<(String, f64)> {
    let nm = native_models();
    if !nm.is_empty() {
        return nm;
    }
    let Some(sp) = script_path() else { return vec![] };
    let Ok(o) = command(&python()).arg(&sp).arg("models").output() else { return vec![] };
    for line in String::from_utf8_lossy(&o.stdout).lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            if v["type"] == "models" {
                return v["models"].as_array().map(|a| a.iter().map(|m| (m["name"].as_str().unwrap_or("").to_string(), m["size_mb"].as_f64().unwrap_or(0.0))).collect()).unwrap_or_default();
            }
        }
    }
    vec![]
}

fn jobs() -> std::sync::MutexGuard<'static, Option<HashMap<String, Arc<Mutex<JobState>>>>> {
    let mut g = JOBS.lock().unwrap_or_else(|p| p.into_inner());
    if g.is_none() {
        *g = Some(HashMap::new());
    }
    g
}

fn job(id: &str) -> Option<Arc<Mutex<JobState>>> {
    jobs().as_ref().and_then(|m| m.get(id).cloned())
}

/// transcribe in this process with the candle Whisper port; Err(reason) when it cannot (the caller may fall back to Python)
#[cfg(feature = "llm")]
fn run_native(st: &Arc<Mutex<JobState>>, audio: &str, model: &str, lang: &str, words: bool) -> Result<(), String> {
    use std::sync::atomic::{AtomicBool, Ordering};
    let dir = ling_ai::whisper::resolve_model(model).ok_or_else(|| "no such model".to_string())?;
    let flag = Arc::new(AtomicBool::new(false));
    st.lock().unwrap_or_else(|p| p.into_inner()).flag = Some(flag.clone());
    let set_err = |m: &str| st.lock().unwrap_or_else(|p| p.into_inner()).info = m.to_string();
    set_err("reading the audio...");
    let a = ling_music::decode::load(audio)?;
    let samples = ling_ai::whisper::resample_to_16k(&a.mono, a.rate);
    if samples.is_empty() {
        return Err("the audio file is empty".into());
    }
    set_err("loading the model...");
    let want_gpu = cfg!(feature = "llm-cuda") && std::env::var("LING_STT_DEVICE").map(|v| v != "cpu").unwrap_or(true);
    let mut w = None;
    if want_gpu {
        w = ling_ai::whisper::Whisper::load(&dir, 0).ok().filter(|w| w.device_name != "cpu");
    }
    let mut w = match w {
        Some(w) => w,
        None => ling_ai::whisper::Whisper::load(&dir, -1)?,
    };
    {
        let mut s = st.lock().unwrap_or_else(|p| p.into_inner());
        s.info = format!("loaded {} on {}", model, w.device_name);
    }
    let opts = ling_ai::whisper::Options { language: lang.to_string(), word_timestamps: words, translate: false };
    let (st1, st2) = (st.clone(), st.clone());
    let (_segs, language) = w.transcribe(
        &samples,
        &opts,
        &flag,
        &mut |done, total| {
            st1.lock().unwrap_or_else(|p| p.into_inner()).progress = if total > 0.0 { (done / total).clamp(0.0, 1.0) } else { 0.0 };
        },
        &mut |seg| {
            let mut s = st2.lock().unwrap_or_else(|p| p.into_inner());
            s.segments.push((seg.start, seg.end, seg.text.clone()));
            for x in &seg.words {
                s.words.push((x.start, x.end, x.text.clone(), x.prob));
            }
        },
    )?;
    let mut s = st.lock().unwrap_or_else(|p| p.into_inner());
    s.language = language;
    s.progress = 1.0;
    if flag.load(Ordering::Relaxed) {
        s.error = "cancelled".into();
    }
    Ok(())
}

pub fn stt_start(audio: &str, model: &str, lang: &str, words: bool) -> String {
    let native_ok = !native_models().is_empty();
    let sp = script_path();
    if sp.is_none() && !native_ok {
        return String::new();
    }
    let sp = sp.unwrap_or_default();
    let id = format!("stt{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_micros()).unwrap_or(0));
    let st = Arc::new(Mutex::new(JobState::default()));
    jobs().as_mut().unwrap().insert(id.clone(), st.clone());
    let (audio, model, lang) = (audio.to_string(), if model.is_empty() { "small".to_string() } else { model.to_string() }, if lang.is_empty() { "auto".to_string() } else { lang.to_string() });
    std::thread::spawn(move || {
        let fail = |m: String| {
            let mut s = st.lock().unwrap_or_else(|p| p.into_inner());
            s.error = m;
            s.done = true;
        };
        // the in-process engine first: no Python, nothing to install
        #[cfg(feature = "llm")]
        if native_ok {
            match run_native(&st, &audio, &model, &lang, words) {
                Ok(()) => {
                    st.lock().unwrap_or_else(|p| p.into_inner()).done = true;
                    return;
                }
                Err(e) if e == "cancelled" => return fail(e),
                Err(e) => {
                    // fall back to faster-whisper in Python when it is there; otherwise report why
                    if !stt_python_ready() {
                        return fail(e);
                    }
                    let mut s = st.lock().unwrap_or_else(|p| p.into_inner());
                    s.segments.clear();
                    s.words.clear();
                }
            }
        }
        let _ = native_ok;
        let mut c = command(&python());
        c.arg(&sp).args(["transcribe", &audio, "--model", &model, "--lang", &lang, "--words", if words { "1" } else { "0" }]);
        let mut child = match c.spawn() {
            Ok(c) => c,
            Err(e) => return fail(format!("could not start Python ({e}); set LING_STT_PYTHON or create the faster-whisper venv")),
        };
        st.lock().unwrap_or_else(|p| p.into_inner()).pid = Some(child.id());
        let stderr = child.stderr.take();
        let err_text = std::thread::spawn(move || {
            let mut t = String::new();
            if let Some(mut e) = stderr {
                use std::io::Read;
                let _ = e.read_to_string(&mut t);
            }
            t
        });
        if let Some(out) = child.stdout.take() {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
                let mut s = st.lock().unwrap_or_else(|p| p.into_inner());
                match v["type"].as_str().unwrap_or("") {
                    "info" => s.info = v["message"].as_str().unwrap_or("").to_string(),
                    "progress" => {
                        let (d, t) = (v["done"].as_f64().unwrap_or(0.0), v["total"].as_f64().unwrap_or(0.0));
                        s.progress = if t > 0.0 { (d / t).clamp(0.0, 1.0) } else { 0.0 };
                    }
                    "segment" => {
                        let (a, b) = (v["start"].as_f64().unwrap_or(0.0), v["end"].as_f64().unwrap_or(0.0));
                        s.segments.push((a, b, v["text"].as_str().unwrap_or("").to_string()));
                        if let Some(ws) = v["words"].as_array() {
                            for w in ws {
                                s.words.push((w[0].as_f64().unwrap_or(0.0), w[1].as_f64().unwrap_or(0.0), w[2].as_str().unwrap_or("").to_string(), w[3].as_f64().unwrap_or(0.0)));
                            }
                        }
                    }
                    "done" => {
                        s.language = v["language"].as_str().unwrap_or("").to_string();
                        s.progress = 1.0;
                    }
                    "error" => s.error = v["message"].as_str().unwrap_or("failed").to_string(),
                    _ => {}
                }
            }
        }
        let status = child.wait();
        let errors = err_text.join().unwrap_or_default();
        let mut s = st.lock().unwrap_or_else(|p| p.into_inner());
        if s.error.is_empty() && !s.cancelled && status.map(|x| !x.success()).unwrap_or(true) {
            s.error = errors.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("the speech recognizer stopped unexpectedly").to_string();
        }
        s.done = true;
    });
    id
}

pub fn stt_poll(id: &str) -> f64 {
    job(id).map(|j| j.lock().unwrap_or_else(|p| p.into_inner()).progress).unwrap_or(0.0)
}
pub fn stt_done(id: &str) -> bool {
    job(id).map(|j| j.lock().unwrap_or_else(|p| p.into_inner()).done).unwrap_or(true)
}
pub fn stt_error(id: &str) -> String {
    job(id).map(|j| j.lock().unwrap_or_else(|p| p.into_inner()).error.clone()).unwrap_or_else(|| "no such job".into())
}
pub fn stt_info(id: &str) -> String {
    job(id).map(|j| j.lock().unwrap_or_else(|p| p.into_inner()).info.clone()).unwrap_or_default()
}
pub fn stt_language(id: &str) -> String {
    job(id).map(|j| j.lock().unwrap_or_else(|p| p.into_inner()).language.clone()).unwrap_or_default()
}
pub fn stt_segments(id: &str) -> Vec<(f64, f64, String)> {
    job(id).map(|j| j.lock().unwrap_or_else(|p| p.into_inner()).segments.clone()).unwrap_or_default()
}
pub fn stt_words(id: &str) -> Vec<(f64, f64, String, f64)> {
    job(id).map(|j| j.lock().unwrap_or_else(|p| p.into_inner()).words.clone()).unwrap_or_default()
}
/// faster-whisper importable in the Python this would use (cached)
fn stt_python_ready() -> bool {
    let py = python();
    command(&py).args(["-c", "import faster_whisper"]).output().map(|o| o.status.success()).unwrap_or(false)
}

pub fn stt_cancel(id: &str) {
    if let Some(j) = job(id) {
        let mut s = j.lock().unwrap_or_else(|p| p.into_inner());
        s.cancelled = true;
        if let Some(f) = &s.flag {
            f.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        if let Some(pid) = s.pid {
            #[cfg(windows)]
            {
                let _ = Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).output();
            }
            #[cfg(not(windows))]
            {
                let _ = Command::new("kill").arg(pid.to_string()).output();
            }
        }
        if s.error.is_empty() {
            s.error = "cancelled".into();
        }
    }
}

fn srt_time(t: f64) -> String {
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

/// SubRip text: from the words, in cues of at most `max_chars` characters (and a break at a pause of a second); from whole segments when
/// there are no words or `max_chars` is 0.
pub fn stt_srt(id: &str, max_chars: usize) -> String {
    let words = stt_words(id);
    let mut cues: Vec<(f64, f64, String)> = vec![];
    if max_chars == 0 || words.is_empty() {
        cues = stt_segments(id);
    } else {
        let mut cur: Option<(f64, f64, String)> = None;
        for (s, e, w, _) in words {
            match cur.as_mut() {
                Some(c) if c.2.chars().count() + 1 + w.chars().count() <= max_chars && s - c.1 < 1.0 && !c.2.ends_with(|ch: char| matches!(ch, '.' | '?' | '!' | '。' | '！' | '？')) => {
                    c.2.push(' ');
                    c.2.push_str(&w);
                    c.1 = e;
                }
                _ => {
                    if let Some(c) = cur.take() {
                        cues.push(c);
                    }
                    cur = Some((s, e, w));
                }
            }
        }
        if let Some(c) = cur {
            cues.push(c);
        }
    }
    let mut out = String::new();
    for (i, (s, e, t)) in cues.iter().enumerate() {
        out.push_str(&format!("{}\n{} --> {}\n{}\n\n", i + 1, srt_time(*s), srt_time((*e).max(*s + 0.2)), t));
    }
    out
}

// ── pasted lyrics: timings from the audio ─────────────────────────────────────

fn norm_char(c: char) -> Option<char> {
    if c.is_alphanumeric() { c.to_lowercase().next() } else { None }
}

/// Fit pasted lyrics to what was heard. Both sides are flattened to letters (so kana, Hangul, Han and Latin all work and spelling
/// differences only cost a few characters), aligned with a global edit-distance alignment, and every lyric line takes the time of its
/// first and last matched character. Lines nothing matched are spread between their timed neighbours.
/// Returns (start, end, line) for each non-empty line of `lyrics`.
pub fn stt_align(id: &str, lyrics: &str) -> Vec<(f64, f64, String)> {
    let words = stt_words(id);
    let lines: Vec<String> = lyrics.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
    if lines.is_empty() {
        return vec![];
    }
    // heard characters with the span of their word shared out between them
    let mut heard: Vec<(char, f64, f64)> = vec![];
    if words.is_empty() {
        for (s, e, t) in stt_segments(id) {
            let cs: Vec<char> = t.chars().filter_map(norm_char).collect();
            for (i, c) in cs.iter().enumerate() {
                let d = (e - s) / cs.len().max(1) as f64;
                heard.push((*c, s + d * i as f64, s + d * (i as f64 + 1.0)));
            }
        }
    } else {
        for (s, e, w) in words.iter().map(|x| (x.0, x.1, &x.2)) {
            let cs: Vec<char> = w.chars().filter_map(norm_char).collect();
            for (i, c) in cs.iter().enumerate() {
                let d = (e - s).max(0.0) / cs.len().max(1) as f64;
                heard.push((*c, s + d * i as f64, s + d * (i as f64 + 1.0)));
            }
        }
    }
    // lyric characters with their line
    let mut lc: Vec<(char, usize)> = vec![];
    for (li, l) in lines.iter().enumerate() {
        for c in l.chars().filter_map(norm_char) {
            lc.push((c, li));
        }
    }
    let (n, m) = (heard.len(), lc.len());
    let mut line_t: Vec<(f64, f64)> = vec![(f64::NAN, f64::NAN); lines.len()];
    if n > 0 && m > 0 && n.saturating_mul(m) <= 60_000_000 {
        // Needleman-Wunsch: match +2, mismatch -1, gap -1
        let w = m + 1;
        let mut score = vec![0i32; (n + 1) * w];
        let mut back = vec![0u8; (n + 1) * w]; // 0 diagonal, 1 up (skip a heard char), 2 left (skip a lyric char)
        for i in 1..=n {
            score[i * w] = -(i as i32);
            back[i * w] = 1;
        }
        for j in 1..=m {
            score[j] = -(j as i32);
            back[j] = 2;
        }
        for i in 1..=n {
            for j in 1..=m {
                let d = score[(i - 1) * w + j - 1] + if heard[i - 1].0 == lc[j - 1].0 { 2 } else { -1 };
                let u = score[(i - 1) * w + j] - 1;
                let l = score[i * w + j - 1] - 1;
                let (v, b) = if d >= u && d >= l { (d, 0) } else if u >= l { (u, 1) } else { (l, 2) };
                score[i * w + j] = v;
                back[i * w + j] = b;
            }
        }
        let (mut i, mut j) = (n, m);
        while i > 0 || j > 0 {
            match back[i * w + j] {
                0 => {
                    if heard[i - 1].0 == lc[j - 1].0 {
                        let (li, (a, b)) = (lc[j - 1].1, (heard[i - 1].1, heard[i - 1].2));
                        let t = &mut line_t[li];
                        t.0 = if t.0.is_nan() { a } else { t.0.min(a) };
                        t.1 = if t.1.is_nan() { b } else { t.1.max(b) };
                    }
                    i -= 1;
                    j -= 1;
                }
                1 => i -= 1,
                _ => j -= 1,
            }
        }
    }
    // lines with no match: evenly between the nearest timed lines (or across the audio)
    let total = heard.last().map(|h| h.2).unwrap_or(0.0);
    let mut k = 0;
    while k < lines.len() {
        if line_t[k].0.is_nan() {
            let mut e = k;
            while e < lines.len() && line_t[e].0.is_nan() {
                e += 1;
            }
            let lo = if k > 0 { line_t[k - 1].1 } else { 0.0 };
            let hi = if e < lines.len() { line_t[e].0 } else { total.max(lo) };
            let cnt = (e - k) as f64;
            for q in k..e {
                let a = lo + (hi - lo) * (q - k) as f64 / cnt;
                let b = lo + (hi - lo) * (q - k + 1) as f64 / cnt;
                line_t[q] = (a, b);
            }
            k = e;
        } else {
            k += 1;
        }
    }
    // keep the lines in order and not overlapping
    let mut out = vec![];
    for (i, l) in lines.iter().enumerate() {
        let (mut a, mut b) = line_t[i];
        if let Some(prev) = out.last().map(|p: &(f64, f64, String)| p.1) {
            if a < prev {
                a = prev;
            }
        }
        if i + 1 < lines.len() && line_t[i + 1].0 > a && b > line_t[i + 1].0 {
            b = line_t[i + 1].0;
        }
        if b < a + 0.3 {
            b = a + 0.3;
        }
        out.push((a, b, l.clone()));
    }
    out
}

/// SubRip text of the pasted lyrics, one cue per line, timed from the audio
pub fn stt_align_srt(id: &str, lyrics: &str) -> String {
    let mut out = String::new();
    for (i, (s, e, t)) in stt_align(id, lyrics).iter().enumerate() {
        out.push_str(&format!("{}
{} --> {}
{}

", i + 1, srt_time(*s), srt_time(*e), t));
    }
    out
}
