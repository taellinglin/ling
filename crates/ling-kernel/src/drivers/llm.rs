//! Qwen3 inside LingOS: `ask <question>` in the Terminal talks to a language model that runs in the kernel, on the CPU, with no network.
//!
//! The engine is `ling-llm` (no_std + alloc + libm). The weights are NOT in the kernel image: they sit on a raw second IDE disk (the
//! primary bus's slave drive) written by `live/make-model-disk.py`, whose sector 0 holds the magic `LINGLLM1` and the length of the GGUF
//! file that follows from sector 1. Loading reads the file in 8 MiB pieces (the frame allocator cannot give more than 64 MiB at once, and
//! the embedding table alone is 165 MB), so a 600 MB model needs about 800 MB of RAM with its KV cache: start the VM with `-m 1536`.
//!
//! All the work happens in ONE background kernel task on the cooperative scheduler, like the messenger and the icon loader: the Terminal
//! only queues a request and shows what the task has produced, and the engine yields to the desktop (every ~16 ms, from inside its
//! matrix loops) so the windows keep moving while the model thinks. A generation can be stopped with `llm stop`.
use crate::arch::timer;
use crate::drivers::ata;
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use ling_llm::gguf::Source;
use ling_llm::{Error, Llm, State};

const MAGIC: &[u8; 8] = b"LINGLLM1";
/// tokens of context the cache is sized for (prompt + answer)
const CTX: usize = 1024;
const SYSTEM: &str = "You are the assistant built into LingOS, a small operating system written in the Ling language. Answer briefly and plainly.";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Loading,
    Ready,
    Generating,
    Failed,
}

struct Job {
    question: String,
    max_tokens: usize,
}

static mut PHASE: Phase = Phase::Idle;
static mut LLM: Option<Box<Llm>> = None;
static mut STATE: Option<State> = None;
static mut WANT_LOAD: bool = false;
static mut JOB: Option<Job> = None;
static mut CANCEL: bool = false;
static mut TASK_SPAWNED: bool = false;
static mut LAST_YIELD: u64 = 0;
static mut PROGRESS: (usize, usize) = (0, 0);
/// text produced and not yet shown, and the line being assembled from it
static mut OUT: String = String::new();
static mut LINE: String = String::new();
/// status messages for the Terminal ("loading ...", errors)
static mut NOTES: Vec<(bool, String)> = Vec::new();

fn note(s: String) {
    unsafe { (*&raw mut NOTES).push((false, s)) };
}
fn note_err(s: String) {
    unsafe { (*&raw mut NOTES).push((true, s)) };
}

// ── the model disk ──────────────────────────────────────────────────────────────

/// the GGUF on the model disk, read through the ATA driver
struct DiskSource {
    len: u64,
}

impl Source for DiskSource {
    fn len(&self) -> u64 {
        self.len
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        if offset.checked_add(buf.len() as u64).map_or(true, |e| e > self.len) {
            return Err(Error::Io);
        }
        // the file starts at sector 1
        let abs = 512 + offset;
        let first = (abs / 512) as u32;
        let skip = (abs % 512) as usize;
        if skip == 0 && buf.len() % 512 == 0 {
            return ata::read_slave(first, buf).map_err(|_| Error::Io);
        }
        let need = (skip + buf.len() + 511) / 512;
        let mut tmp = vec![0u8; need * 512];
        ata::read_slave(first, &mut tmp).map_err(|_| Error::Io)?;
        buf.copy_from_slice(&tmp[skip..skip + buf.len()]);
        Ok(())
    }
}

fn open_disk() -> Result<DiskSource, String> {
    let sectors = ata::slave_sectors().ok_or_else(|| "no model disk: attach a second IDE disk made by make-model-disk.py".to_string())?;
    let mut head = [0u8; 512];
    ata::read_slave(0, &mut head).map_err(|_| "could not read the model disk".to_string())?;
    if &head[..8] != MAGIC {
        return Err("the second disk is not a model disk (no LINGLLM1 header)".to_string());
    }
    let len = u64::from_le_bytes(head[8..16].try_into().unwrap());
    if len == 0 || len > (sectors as u64 - 1) * 512 {
        return Err("the model disk's header does not fit the disk".to_string());
    }
    Ok(DiskSource { len })
}

// ── the task ───────────────────────────────────────────────────────────────────

/// called from inside the engine's matrix loops: hand the CPU to the desktop at most every 16 ms
fn maybe_yield() {
    let t = timer::now_us();
    unsafe {
        if t.wrapping_sub(LAST_YIELD) >= 16_000 {
            crate::proc::sched::yield_now();
            LAST_YIELD = timer::now_us();
        }
    }
}

fn do_load() {
    unsafe {
        WANT_LOAD = false;
        if LLM.is_some() {
            PHASE = Phase::Ready;
            return;
        }
        PHASE = Phase::Loading;
    }
    note("llm: opening the model disk...".to_string());
    let src = match open_disk() {
        Ok(s) => s,
        Err(e) => {
            note_err(alloc::format!("llm: {e}"));
            unsafe { PHASE = Phase::Failed };
            return;
        }
    };
    // a model of N bytes needs about N + 250 MB (cache, scratch, the rest of the system): refuse before the allocator fails for us
    let free = crate::mm::frame::free_frame_count() as u64 * 4096;
    if free < src.len + (250 << 20) {
        note_err(alloc::format!("llm: not enough memory ({} MB free, the model needs about {} MB): start the VM with -m 1536", free >> 20, (src.len + (250 << 20)) >> 20));
        unsafe { PHASE = Phase::Failed };
        return;
    }
    note(alloc::format!("llm: loading {} MB (this takes a while; the desktop stays live)...", src.len >> 20));
    let t0 = timer::now_us();
    let mut last_pct = 0;
    let res = Llm::load(&src, &mut |done, total| {
        unsafe { PROGRESS = (done, total) };
        let pct = done * 100 / total.max(1);
        if pct >= last_pct + 10 {
            last_pct = pct;
            note(alloc::format!("llm: {pct}%"));
        }
        maybe_yield();
    });
    match res {
        Ok(llm) => {
            let state = llm.model.new_state(CTX);
            note(alloc::format!("llm: ready. {} MB of weights, {}, loaded in {} s.", llm.model.weight_bytes() >> 20, ling_llm::model::describe(&llm.model.cfg), (timer::now_us() - t0) / 1_000_000));
            unsafe {
                LLM = Some(Box::new(llm));
                STATE = Some(state);
                PHASE = Phase::Ready;
            }
        }
        Err(e) => {
            note_err(alloc::format!("llm: {e}"));
            unsafe { PHASE = Phase::Failed };
        }
    }
}

fn do_generate(job: Job) {
    let (Some(llm), Some(st)) = (unsafe { (*&raw mut LLM).as_ref() }, unsafe { (*&raw mut STATE).as_mut() }) else { return };
    unsafe {
        PHASE = Phase::Generating;
        CANCEL = false;
    }
    let t0 = timer::now_us();
    let history = alloc::format!("<|user|>\n{}\n", job.question);
    let seed = t0 ^ 0x9E37_79B9;
    let res = llm.generate(st, SYSTEM, &history, job.max_tokens, 0.6, 0.9, seed, &mut |piece| {
        unsafe { (*&raw mut OUT).push_str(piece) };
        maybe_yield();
        unsafe { !CANCEL }
    });
    unsafe { (*&raw mut OUT).push('\n') };
    match res {
        Ok(n) => {
            let ms = ((timer::now_us() - t0) / 1000).max(1);
            note(alloc::format!("llm: {n} tokens in {} s", ms / 1000));
        }
        Err(e) => note_err(alloc::format!("llm: {e}")),
    }
    unsafe { PHASE = Phase::Ready };
}

extern "C" fn llm_task() {
    ling_llm::quant::set_yield(maybe_yield);
    loop {
        unsafe {
            let wants_job = (*&raw mut JOB).is_some();
            if WANT_LOAD || (wants_job && LLM.is_none() && PHASE != Phase::Failed) {
                do_load();
            }
            if let Some(job) = (*&raw mut JOB).take() {
                if LLM.is_some() {
                    do_generate(job);
                } else {
                    note_err("llm: there is no model loaded (see the messages above)".to_string());
                }
            }
        }
        crate::proc::sched::yield_now();
    }
}

// ── what the Terminal calls ───────────────────────────────────────────────────

/// Start the background task (once).
pub fn start() -> bool {
    unsafe {
        if TASK_SPAWNED {
            return true;
        }
        let slot = crate::proc::sched::spawn(llm_task as *const () as usize as u64);
        if slot == u64::MAX {
            return false;
        }
        TASK_SPAWNED = true;
    }
    true
}

pub fn request_load() {
    unsafe {
        if PHASE == Phase::Failed {
            PHASE = Phase::Idle;
        }
        WANT_LOAD = true;
    }
}

/// Queue a question. False when the model is busy with another.
pub fn ask(question: &str) -> bool {
    unsafe {
        if (*&raw mut JOB).is_some() || PHASE == Phase::Generating {
            return false;
        }
        if PHASE == Phase::Failed {
            PHASE = Phase::Idle;
        }
        *(&raw mut JOB) = Some(Job { question: question.to_string(), max_tokens: 256 });
    }
    true
}

pub fn cancel() {
    unsafe { CANCEL = true };
}

pub fn phase() -> Phase {
    unsafe { PHASE }
}

pub fn status() -> String {
    unsafe {
        match PHASE {
            Phase::Idle => "llm: not loaded (ask something, or `llm load`)".to_string(),
            Phase::Loading => alloc::format!("llm: loading ({} of {} steps)", PROGRESS.0, PROGRESS.1),
            Phase::Ready => "llm: ready".to_string(),
            Phase::Generating => "llm: thinking (`llm stop` to interrupt)".to_string(),
            Phase::Failed => "llm: the last load failed (see the message above; `llm load` tries again)".to_string(),
        }
    }
}

/// True while there is anything the Terminal has not shown yet.
pub fn has_output() -> bool {
    unsafe { !(&*&raw const NOTES).is_empty() || !(&*&raw const OUT).is_empty() }
}

/// Hand everything produced so far to `f` as lines of at most `cols` characters: `(kind, text)` with kind 0 = the model's text, 1 = a
/// status note, 2 = an error. A line still being written waits for its end.
pub fn drain_lines(cols: usize, f: &mut dyn FnMut(u8, &str)) {
    unsafe {
        for (is_err, s) in core::mem::take(&mut *&raw mut NOTES) {
            f(if is_err { 2 } else { 1 }, &s);
        }
        let text = core::mem::take(&mut *&raw mut OUT);
        let line = &mut *&raw mut LINE;
        for c in text.chars() {
            if c == '\n' {
                f(0, line);
                line.clear();
                continue;
            }
            line.push(c);
            if line.chars().count() >= cols {
                // break at the last space when there is one in the second half, else mid-word
                let cut = line.rfind(' ').filter(|i| *i > cols / 2);
                match cut {
                    Some(i) => {
                        let rest = line[i + 1..].to_string();
                        f(0, &line[..i]);
                        *line = rest;
                    }
                    None => {
                        f(0, line);
                        line.clear();
                    }
                }
            }
        }
    }
}
