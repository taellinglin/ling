// src/runtime/llm.rs — in-process chat-completion builtins backing
// `ling-ai::LoadedModel` (chat.ling-lang.org, Milestone 2).
//
// Two global tables, same single-global convention as ai.rs: a model slab
// (handle = slot index, mirrors NETS/TREES/LMS in ai.rs) and a job map
// (string id, mirrors AsyncJobs in web.rs) since generation jobs are created
// per-request rather than explicitly by the Ling program. Generation itself
// runs on a spawned OS thread — `ling.exe`'s HTTP dispatch loop is
// single-threaded end to end, so a multi-second decode must never run on it
// directly. All accessors fail soft (empty string / "done" / -1), never
// panic, so a bad handle or job id can't crash the interpreter.
//
// Job status is deliberately three-state ("pending" / "tool_call" / "done")
// even though only pending/done are reachable today — see
// `ling_ai::llm`'s module doc for why tool_call detection isn't wired up yet.

#![cfg(feature = "llm")]

use ling_ai::LoadedModel;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

static MODELS: Mutex<Vec<Option<Arc<LoadedModel>>>> = Mutex::new(Vec::new());

fn push_model(model: LoadedModel) -> i64 {
    match MODELS.lock() {
        Ok(mut g) => {
            g.push(Some(Arc::new(model)));
            (g.len() - 1) as i64
        },
        Err(_) => -1,
    }
}

fn get_model(id: i64) -> Option<Arc<LoadedModel>> {
    let idx = usize::try_from(id).ok()?;
    MODELS.lock().ok()?.get(idx)?.clone()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum JobStatus {
    Pending,
    ToolCall,
    Done,
}

struct Job {
    status: Mutex<JobStatus>,
    text: Mutex<String>,
    tool_name: Mutex<String>,
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
    // Random, not sequential — a sequential per-process counter collided
    // with leftover rows in the `.ling` app's persisted `active_jobs` table
    // (SQLite state outlives a restart; the in-memory JOBS map here doesn't,
    // so a fresh process's "job 1" reused an old row's primary key). Same
    // random-hex-id convention as AsyncJobs/OAuthJobs in web.rs.
    use rand::RngCore;
    let mut buf = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    format!("llm{}", buf.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

// ── builtins ─────────────────────────────────────────────────────────────

/// Loads a GGUF checkpoint + tokenizer onto `device_index` (negative = CPU).
/// Returns a handle, or -1 on any load failure.
pub fn llm_load(gguf_path: &str, tokenizer_path: &str, device_index: i64) -> i64 {
    match LoadedModel::load(gguf_path, tokenizer_path, device_index) {
        Ok(model) => push_model(model),
        Err(_) => -1,
    }
}

/// Starts a generation job on a background thread; returns its id
/// immediately, or "" if `handle` doesn't resolve to a loaded model.
pub fn llm_generate_start(
    handle: i64,
    system_prompt: &str,
    history: &str,
    max_tokens: i64,
    temperature: f64,
    top_p: f64,
) -> String {
    let model = match get_model(handle) {
        Some(m) => m,
        None => return String::new(),
    };

    let job_id = new_job_id();
    let job = Arc::new(Job {
        status: Mutex::new(JobStatus::Pending),
        text: Mutex::new(String::new()),
        tool_name: Mutex::new(String::new()),
    });
    jobs().as_mut().unwrap().insert(job_id.clone(), job.clone());

    let system_prompt = system_prompt.to_string();
    let history = history.to_string();
    let max_tokens = max_tokens.max(1) as usize;
    let seed = rand::random::<u64>();

    std::thread::spawn(move || {
        let mut generated = String::new();
        model.generate(&system_prompt, &history, max_tokens, temperature, top_p, seed, |piece| {
            generated.push_str(piece);
            *job.text.lock().unwrap() = generated.clone();
        });
        *job.status.lock().unwrap() = JobStatus::Done;
    });

    job_id
}

/// Text generated so far (grows across polls while status is "pending").
pub fn llm_generate_poll(job_id: &str) -> String {
    match jobs().as_ref().unwrap().get(job_id) {
        Some(j) => j.text.lock().unwrap().clone(),
        None => String::new(),
    }
}

pub fn llm_generate_done(job_id: &str) -> bool {
    match jobs().as_ref().unwrap().get(job_id) {
        Some(j) => *j.status.lock().unwrap() == JobStatus::Done,
        None => true,
    }
}

pub fn llm_job_status(job_id: &str) -> String {
    match jobs().as_ref().unwrap().get(job_id) {
        Some(j) => match *j.status.lock().unwrap() {
            JobStatus::Pending => "pending",
            JobStatus::ToolCall => "tool_call",
            JobStatus::Done => "done",
        },
        None => "done",
    }
    .to_string()
}

/// Empty until Milestone 5's tool-call detection lands (see
/// `ling_ai::llm`'s module doc for why).
pub fn llm_job_tool_name(job_id: &str) -> String {
    match jobs().as_ref().unwrap().get(job_id) {
        Some(j) => j.tool_name.lock().unwrap().clone(),
        None => String::new(),
    }
}

/// No-op until Milestone 5 — no job ever reaches `tool_call` status yet, so
/// there is nothing to resume. Always returns false for now.
pub fn llm_job_provide_tool_result(_job_id: &str, _result_text: &str) -> bool {
    false
}

pub fn llm_job_cancel(job_id: &str) {
    if let Some(j) = jobs().as_ref().unwrap().get(job_id) {
        *j.status.lock().unwrap() = JobStatus::Done;
    }
}
