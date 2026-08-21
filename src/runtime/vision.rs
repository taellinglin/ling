// src/runtime/vision.rs — image-understanding builtins backing
// `ling-ai::vision::VisionModel` (chat.ling-lang.org's attach-an-image flow:
// UI screenshots and 3D/game screenshots — OCR/text-detection is a separate
// model, not this one).
//
// Same shape as runtime/llm.rs throughout: a model slab (handle = slot
// index) and a job map (string id), analysis runs on a background thread so
// http_serve's single-threaded dispatch loop never blocks on it. All
// accessors fail soft, never panic.
//
// `.ling` has no way to hold binary image bytes safely (Value::Str is a
// Rust String, which must be valid UTF-8 — raw PNG/JPEG bytes aren't), so
// the image crosses from the browser to here as a base64 string; this file
// is the one place that decodes it back to bytes before handing them to
// ling-ai, which only ever deals in plain `&[u8]`.

#![cfg(feature = "vision")]

use base64::Engine;
use ling_ai::vision::VisionModel;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

static MODELS: Mutex<Vec<Option<Arc<VisionModel>>>> = Mutex::new(Vec::new());

fn push_model(model: VisionModel) -> i64 {
    match MODELS.lock() {
        Ok(mut g) => {
            g.push(Some(Arc::new(model)));
            (g.len() - 1) as i64
        },
        Err(_) => -1,
    }
}

fn get_model(id: i64) -> Option<Arc<VisionModel>> {
    let idx = usize::try_from(id).ok()?;
    MODELS.lock().ok()?.get(idx)?.clone()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum JobStatus {
    Pending,
    Done,
}

struct Job {
    status: Mutex<JobStatus>,
    text: Mutex<String>,
    error: Mutex<String>,
}

static JOBS: Mutex<Option<HashMap<String, Arc<Job>>>> = Mutex::new(None);

fn jobs() -> std::sync::MutexGuard<'static, Option<HashMap<String, Arc<Job>>>> {
    let mut g = JOBS.lock().unwrap();
    if g.is_none() {
        *g = Some(HashMap::new());
    }
    g
}

fn new_job_id() -> String {
    use rand::RngCore;
    let mut buf = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    format!("vis{}", buf.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

// ── builtins ─────────────────────────────────────────────────────────────

/// Loads a quantized Moondream2 checkpoint + tokenizer onto `device_index`
/// (negative = CPU). Returns a handle, or -1 on any load failure.
pub fn vision_load(gguf_path: &str, tokenizer_path: &str, device_index: i64) -> i64 {
    match VisionModel::load(gguf_path, tokenizer_path, device_index) {
        Ok(model) => push_model(model),
        Err(_) => -1,
    }
}

/// Starts an image-analysis job on a background thread; returns its id
/// immediately, or "" if `handle` doesn't resolve to a loaded model or
/// `image_base64` fails to decode. `question` frames what to look for
/// (`.ling` callers pick the framing per category — UI critique vs. general
/// screenshot description).
pub fn vision_analyze_start(
    handle: i64,
    image_base64: &str,
    question: &str,
    max_tokens: i64,
    temperature: f64,
    top_p: f64,
) -> String {
    let model = match get_model(handle) {
        Some(m) => m,
        None => return String::new(),
    };
    let image_bytes = match base64::engine::general_purpose::STANDARD.decode(image_base64.trim()) {
        Ok(b) => b,
        Err(_) => return String::new(),
    };

    let job_id = new_job_id();
    let job = Arc::new(Job {
        status: Mutex::new(JobStatus::Pending),
        text: Mutex::new(String::new()),
        error: Mutex::new(String::new()),
    });
    jobs().as_mut().unwrap().insert(job_id.clone(), job.clone());

    let question = question.to_string();
    let max_tokens = max_tokens.max(1) as usize;
    let seed = rand::random::<u64>();

    std::thread::spawn(move || {
        let mut generated = String::new();
        let result = model.analyze(&image_bytes, &question, max_tokens, temperature, top_p, seed, |piece| {
            generated.push_str(piece);
            *job.text.lock().unwrap() = generated.clone();
        });
        if let Err(e) = result {
            *job.error.lock().unwrap() = e;
        }
        *job.status.lock().unwrap() = JobStatus::Done;
    });

    job_id
}

/// Text generated so far (grows across polls while status is "pending").
pub fn vision_analyze_poll(job_id: &str) -> String {
    match jobs().as_ref().unwrap().get(job_id) {
        Some(j) => j.text.lock().unwrap().clone(),
        None => String::new(),
    }
}

pub fn vision_analyze_done(job_id: &str) -> bool {
    match jobs().as_ref().unwrap().get(job_id) {
        Some(j) => *j.status.lock().unwrap() == JobStatus::Done,
        None => true,
    }
}

/// "" unless analysis failed (bad image data, decode error, etc).
pub fn vision_analyze_error(job_id: &str) -> String {
    match jobs().as_ref().unwrap().get(job_id) {
        Some(j) => j.error.lock().unwrap().clone(),
        None => String::new(),
    }
}
