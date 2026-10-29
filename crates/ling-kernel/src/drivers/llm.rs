//! Qwen3 inside LingOS: the language model that runs in the kernel, on the CPU, with no network -- for the Terminal's `ask` and for
//! apps through the same `llm_*` builtins the host ling runtime has (LingStationX's lyrics, song descriptions and stories).
//!
//! The engine is `ling-llm` (no_std + alloc + libm). The weights are NOT in the kernel image. They come from either
//!   * a GGUF file on a FAT volume -- the Pi's SD-card data partition (`media/card`), x86's second disk (`media/disk1`), a USB
//!     stick -- read in pieces through `fs::media::open_range` (random access over the file's cluster runs; the file is never held
//!     in one buffer), found by `vfs::locate_model` (the app's data folder, an installed package, `<volume>/assets/models/`,
//!     `<volume>/models/`), or
//!   * x86 only: a raw second IDE disk written by `live/make-model-disk.py` (sector 0: `LINGLLM1` + the GGUF's length, the file from
//!     sector 1).
//!
//! The weights are copied into RAM (ling-llm keeps them in 8 MiB chunks; the frame allocator cannot give more than 64 MiB at once):
//! generation reads every weight once per token, far too often for an SD card. Before loading, free RAM is checked against the
//! file's size plus the KV cache, and the context is shortened (2048 down to 512 tokens) when RAM is short; a model that still does
//! not fit is refused with a message instead of an allocation failure. A 0.6B Q8_0 model (640 MB) needs about 900 MB.
//!
//! All the work happens in ONE background coroutine (`coro`, a "system" one: it outlives the app that started it), which yields
//! every ~16 ms from inside the engine's matrix loops and between disk pieces, so the app's frames and the desktop keep drawing while
//! the model loads and thinks. `present()` (an app's frame) and the desktop's frame both give it its turn.
//!
//! The builtins, as on the host (`ling/src/runtime/llm.rs`):
//!   * `llm_load(gguf, tokenizer, device)` -> handle, or -1 when the file is not there, is not readable or will not fit in RAM (the
//!     reason goes to the console). The tokenizer argument and the device are ignored: the GGUF carries its own vocabulary and this
//!     is CPU only. The load itself runs in the background; jobs started meanwhile wait for it.
//!   * `llm_generate_start(h, system, history, max_tokens, temperature, top_p)` -> job id ("" for a bad or failed handle)
//!   * `llm_generate_poll(job)` -> ALL the text generated so far (it grows; "" while the model is still loading)
//!   * `llm_generate_done(job)` -> true when finished, failed or unknown
//!   * `llm_job_status(job)` -> "pending" | "done", `llm_job_cancel(job)`, `llm_status()` -> a line about the model (LingOS only)
//!   * `daw_ai_available()` -> true (LingStationX's switch for its AI buttons)
use crate::arch::timer;
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use ling_llm::gguf::{Gguf, Source};
use ling_llm::{Error, Llm, Model, State, Tokenizer};

/// tokens of context the cache is sized for (prompt + answer), when RAM allows; never fewer than MIN_CTX
const CTX: usize = 2048;
const MIN_CTX: usize = 512;
/// RAM kept free for everything else (the desktop, the app, the engine's scratch buffers)
const RESERVE: u64 = 96 << 20;
/// one device read at most this big, with a chance to yield between them
const PIECE: usize = 1 << 20;
const SYSTEM: &str = "You are the assistant built into LingOS, a small operating system written in the Ling language. Answer briefly and plainly.";
/// the model files the Terminal looks for when none is named (smallest first: it answers fastest)
const DEFAULT_MODELS: [&str; 3] = ["assets/models/Qwen3-0.6B-Q8_0.gguf", "assets/models/Qwen3-1.7B-Q8_0.gguf", "assets/models/Qwen3-4B-Q4_K_M.gguf"];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Loading,
    Ready,
    Generating,
    Failed,
}

struct Loaded {
    handle: i64,
    /// where the weights came from ("disk", or a media/... path)
    loc: String,
    llm: Llm,
    st: State,
}

struct GenJob {
    id: String,
    handle: i64,
    system: String,
    history: String,
    max_tokens: usize,
    temperature: f32,
    top_p: f32,
    text: String,
    done: bool,
    running: bool,
    cancel: bool,
    /// the Terminal's question: its text is also streamed to the Terminal
    echo: bool,
}

static mut PHASE: Phase = Phase::Idle;
static mut LOADED: Option<Box<Loaded>> = None;
/// a load the task should do: (handle, location)
static mut WANT: Option<(i64, String)> = None;
/// the handle apps hold now, and the location it stands for
static mut CUR: (i64, String) = (-1, String::new());
static mut NEXT_HANDLE: i64 = 0;
static mut FAILED: Vec<i64> = Vec::new();
static mut JOBS: Vec<GenJob> = Vec::new();
static mut NEXT_JOB: u64 = 1;
static mut TASK: u64 = 0;
static mut LAST_YIELD: u64 = 0;
static mut PROGRESS: (usize, usize) = (0, 0);
static mut LAST_ERR: String = String::new();
/// text produced for the Terminal and not yet shown, and the line being assembled from it
static mut OUT: String = String::new();
static mut LINE: String = String::new();
/// status messages for the Terminal ("loading ...", errors)
static mut NOTES: Vec<(bool, String)> = Vec::new();

/// to the serial log only (the screen belongs to the desktop): what the host prints to its console
fn log(s: &str) {
    #[cfg(target_arch = "x86_64")]
    {
        crate::drivers::serial::write(s.as_bytes());
        crate::drivers::serial::write(b"\n");
    }
    #[cfg(target_arch = "aarch64")]
    {
        crate::drivers::uart::write(s.as_bytes());
        crate::drivers::uart::write(b"\n");
    }
}

fn note(s: String) {
    log(&s);
    unsafe {
        let n = &mut *&raw mut NOTES;
        if n.len() < 64 {
            n.push((false, s));
        }
    }
}
fn note_err(s: String) {
    log(&s);
    unsafe {
        *(&raw mut LAST_ERR) = s.clone();
        let n = &mut *&raw mut NOTES;
        if n.len() < 64 {
            n.push((true, s));
        }
    }
}

fn jobs() -> &'static mut Vec<GenJob> {
    unsafe { &mut *&raw mut JOBS }
}

fn job(id: &str) -> Option<&'static mut GenJob> {
    jobs().iter_mut().find(|j| j.id == id)
}

/// called from inside the engine's loops and between disk pieces: hand the CPU to the app and the desktop at most every 16 ms
fn maybe_yield() {
    if !crate::coro::in_coro() {
        return;
    }
    let t = timer::now_us();
    unsafe {
        if t.wrapping_sub(LAST_YIELD) >= 16_000 {
            crate::coro::yield_now();
            LAST_YIELD = timer::now_us();
        }
    }
}

// ── where the weights come from ─────────────────────────────────────────────

/// the GGUF on the raw model disk (x86), read through the ATA driver
#[cfg(target_arch = "x86_64")]
struct DiskSource {
    len: u64,
}

#[cfg(target_arch = "x86_64")]
impl DiskSource {
    fn read(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        use crate::drivers::ata;
        // the file starts at sector 1
        let abs = 512 + offset;
        let first = (abs / 512) as u32;
        let skip = (abs % 512) as usize;
        if skip == 0 && buf.len() % 512 == 0 {
            return ata::read_slave(first, buf).map_err(|_| Error::Io);
        }
        let need = (skip + buf.len() + 511) / 512;
        let mut tmp = alloc::vec![0u8; need * 512];
        ata::read_slave(first, &mut tmp).map_err(|_| Error::Io)?;
        buf.copy_from_slice(&tmp[skip..skip + buf.len()]);
        Ok(())
    }
}

#[cfg(target_arch = "x86_64")]
const MAGIC: &[u8; 8] = b"LINGLLM1";

#[cfg(target_arch = "x86_64")]
fn open_disk() -> Result<DiskSource, String> {
    use crate::drivers::ata;
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

enum Src {
    #[cfg(target_arch = "x86_64")]
    Disk(DiskSource),
    Fat(crate::fs::media::MediaRange),
}

impl Source for Src {
    fn len(&self) -> u64 {
        match self {
            #[cfg(target_arch = "x86_64")]
            Src::Disk(d) => d.len,
            Src::Fat(f) => f.size(),
        }
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        if offset.checked_add(buf.len() as u64).map_or(true, |e| e > self.len()) {
            return Err(Error::Io);
        }
        // in pieces, yielding between them: an 8 MiB chunk from an SD card takes long enough to stall a frame
        let mut done = 0usize;
        while done < buf.len() {
            let n = (buf.len() - done).min(PIECE);
            let at = offset + done as u64;
            let part = &mut buf[done..done + n];
            match self {
                #[cfg(target_arch = "x86_64")]
                Src::Disk(d) => d.read(at, part)?,
                Src::Fat(f) => {
                    if !f.read_at(at, part) {
                        return Err(Error::Io);
                    }
                },
            }
            done += n;
            maybe_yield();
        }
        Ok(())
    }
}

/// Where a model spec points: "disk" (x86's raw model disk), "" (the first default model found: the raw disk, then the usual
/// file names), or a path (`vfs::locate_model`). The location string identifies the file ("disk" or its media/... path).
fn locate(spec: &str) -> Result<String, String> {
    let spec = spec.trim();
    if spec == "disk" {
        return Ok("disk".to_string());
    }
    if spec.is_empty() {
        #[cfg(target_arch = "x86_64")]
        if open_disk().is_ok() {
            return Ok("disk".to_string());
        }
        for m in DEFAULT_MODELS {
            if let Some(p) = crate::vfs::locate_model(m) {
                return Ok(p);
            }
        }
        return Err("no model found: put Qwen3-0.6B-Q8_0.gguf in models/ or assets/models/ on the SD card / second disk (or name one: llm load <path>)".to_string());
    }
    crate::vfs::locate_model(spec).ok_or_else(|| {
        alloc::format!("{spec}: no such model file (looked in the app's folder, installed packages, and assets/models/ and models/ on every volume)")
    })
}

fn open(loc: &str) -> Result<Src, String> {
    if loc == "disk" {
        #[cfg(target_arch = "x86_64")]
        return open_disk().map(Src::Disk);
        #[allow(unreachable_code)]
        return Err("there is no raw model disk on this machine".to_string());
    }
    crate::fs::media::open_range(loc).map(Src::Fat).ok_or_else(|| alloc::format!("{loc}: could not open it"))
}

fn free_bytes() -> u64 {
    crate::mm::frame::free_frame_count() as u64 * 4096
}

// ── the task ───────────────────────────────────────────────────────────────────

fn fail_load(h: i64, msg: String) {
    note_err(alloc::format!("llm: {msg}"));
    unsafe {
        (*&raw mut FAILED).push(h);
        PHASE = Phase::Failed;
    }
    // the jobs waiting for this model end empty
    for j in jobs().iter_mut().filter(|j| j.handle == h && !j.done) {
        j.done = true;
    }
}

fn do_load(h: i64, loc: &str) {
    unsafe {
        if let Some(m) = (*&raw mut LOADED).as_mut() {
            if m.loc == loc {
                m.handle = h;
                PHASE = Phase::Ready;
                return;
            }
        }
        // a different model: free the old one first (its RAM is needed)
        *(&raw mut LOADED) = None;
        PHASE = Phase::Loading;
        PROGRESS = (0, 1);
    }
    let src = match open(loc) {
        Ok(s) => s,
        Err(e) => return fail_load(h, e),
    };
    note(alloc::format!("llm: reading {loc} ({} MB, {} MB of RAM free)...", src.len() >> 20, free_bytes() >> 20));
    let t0 = timer::now_us();
    let g = match Gguf::parse(&src) {
        Ok(g) => g,
        Err(e) => return fail_load(h, alloc::format!("{loc}: {e}")),
    };
    let cfg = match ling_llm::Config::from_gguf(&g) {
        Ok(c) => c,
        Err(e) => return fail_load(h, alloc::format!("{loc}: {e}")),
    };
    // RAM: the weights (about the file's size) + the f16 KV cache + the reserve
    let kv_per_tok = (cfg.n_layer * cfg.n_kv * cfg.head_dim * 2 * 2) as u64;
    let free = free_bytes();
    let weights = src.len();
    let room = free.saturating_sub(weights + RESERVE);
    let ctx = ((room / kv_per_tok.max(1)) as usize).min(CTX).min(cfg.n_ctx.max(1));
    if ctx < MIN_CTX {
        let need = weights + RESERVE + kv_per_tok * MIN_CTX as u64;
        return fail_load(h, alloc::format!("not enough memory for {loc}: {} MB free, the model needs about {} MB (try a smaller model, e.g. Qwen3-0.6B-Q8_0)", free >> 20, need >> 20));
    }
    note(alloc::format!("llm: {}; context {ctx} tokens; loading (the desktop stays live)...", ling_llm::model::describe(&cfg)));
    let mut last_pct = 0;
    let model = Model::load(&src, &g, &mut |done, total| {
        unsafe { PROGRESS = (done, total) };
        let pct = done * 100 / total.max(1);
        if pct >= last_pct + 10 {
            last_pct = pct;
            note(alloc::format!("llm: {pct}%"));
        }
        maybe_yield();
    });
    let model = match model {
        Ok(m) => m,
        Err(e) => return fail_load(h, alloc::format!("{loc}: {e}")),
    };
    let tok = match Tokenizer::from_gguf(&g) {
        Ok(t) => t,
        Err(e) => return fail_load(h, alloc::format!("{loc}: {e}")),
    };
    drop(g);
    let st = model.new_state(ctx);
    note(alloc::format!(
        "llm: ready. {} MB of weights loaded in {} s; {} MB of RAM free.",
        model.weight_bytes() >> 20,
        (timer::now_us() - t0) / 1_000_000,
        free_bytes() >> 20
    ));
    unsafe {
        *(&raw mut LOADED) = Some(Box::new(Loaded { handle: h, loc: loc.to_string(), llm: Llm { model, tok }, st }));
        PHASE = Phase::Ready;
    }
}

fn run_job(id: String) {
    let Some(m) = (unsafe { (*&raw mut LOADED).as_mut() }) else { return };
    let Some(j) = job(&id) else { return };
    j.running = true;
    let (system, history, max_tokens, temperature, top_p, echo) = (j.system.clone(), j.history.clone(), j.max_tokens, j.temperature, j.top_p, j.echo);
    unsafe { PHASE = Phase::Generating };
    let t0 = timer::now_us();
    let seed = t0 ^ 0x9E37_79B9;
    let res = m.llm.generate(&mut m.st, &system, &history, max_tokens, temperature, top_p, seed, &mut |piece| {
        // look the job up every time: the app may start other jobs (the list grows) while this one runs
        let go = match job(&id) {
            Some(j) => {
                j.text.push_str(piece);
                !j.cancel
            },
            None => false,
        };
        if echo {
            unsafe { (*&raw mut OUT).push_str(piece) };
        }
        maybe_yield();
        go
    });
    if echo {
        unsafe { (*&raw mut OUT).push('\n') };
    }
    match res {
        Ok(n) => {
            let ms = ((timer::now_us() - t0) / 1000).max(1);
            let msg = alloc::format!("llm: {n} tokens in {}.{} s ({} ms a token)", ms / 1000, (ms % 1000) / 100, ms / (n as u64).max(1));
            if echo {
                note(msg);
            } else {
                log(&msg);
            }
        },
        Err(e) => note_err(alloc::format!("llm: {e}")),
    }
    if let Some(j) = job(&id) {
        j.done = true;
        j.running = false;
        if !echo {
            // what the app got, for the serial log (the host prints its replies to the console too)
            let t: String = j.text.chars().take(600).collect();
            log(&alloc::format!("llm: job {id} done ({} chars): {}", j.text.len(), t.replace('\n', " / ")));
        }
    }
    unsafe { PHASE = Phase::Ready };
}

fn task() {
    ling_llm::quant::set_yield(maybe_yield);
    loop {
        if let Some((h, loc)) = unsafe { (*&raw mut WANT).take() } {
            do_load(h, &loc);
        }
        let loaded = unsafe { (*&raw const LOADED).as_ref().map(|m| m.handle) };
        let cur = unsafe { (*&raw const CUR).0 };
        let next = jobs().iter().find(|j| !j.done && !j.running).map(|j| (j.id.clone(), j.handle));
        if let Some((id, h)) = next {
            if loaded == Some(h) {
                run_job(id);
            } else if h != cur || unsafe { (*&raw const FAILED).contains(&h) } {
                // a model that was replaced or failed: nothing to run it on
                if let Some(j) = job(&id) {
                    j.done = true;
                }
            }
            // else: its model is still on the way (WANT is set) -- wait
        }
        // keep the job list short: the oldest finished ones go
        let js = jobs();
        while js.len() > 32 {
            match js.iter().position(|j| j.done) {
                Some(i) => {
                    js.remove(i);
                },
                None => break,
            }
        }
        crate::coro::yield_now();
    }
}

fn ensure_task() -> bool {
    unsafe {
        if TASK != 0 && crate::coro::alive(TASK) {
            return true;
        }
        TASK = crate::coro::spawn_owned("llm", false, "system", Box::new(task));
        TASK != 0
    }
}

/// Make `loc` the model apps get; returns its handle. The load happens in the task.
fn request(loc: String) -> Option<i64> {
    if !ensure_task() {
        note_err("llm: no memory for the model's task".to_string());
        return None;
    }
    unsafe {
        let cur = &mut *&raw mut CUR;
        let failed = (*&raw const FAILED).contains(&cur.0);
        if cur.0 >= 0 && cur.1 == loc && !failed {
            return Some(cur.0);
        }
        NEXT_HANDLE += 1;
        let h = NEXT_HANDLE;
        *cur = (h, loc.clone());
        if PHASE == Phase::Failed {
            PHASE = Phase::Idle;
        }
        *(&raw mut WANT) = Some((h, loc));
        Some(h)
    }
}

fn new_job(handle: i64, system: &str, history: &str, max_tokens: usize, temperature: f32, top_p: f32, echo: bool) -> String {
    let id = unsafe {
        let n = NEXT_JOB;
        NEXT_JOB += 1;
        // the boot time in the id: an id an app saved before a reboot never names a new job
        alloc::format!("llm{:x}-{n}", timer::now_us())
    };
    jobs().push(GenJob {
        id: id.clone(),
        handle,
        system: system.to_string(),
        history: history.to_string(),
        max_tokens: max_tokens.max(1),
        temperature,
        top_p,
        text: String::new(),
        done: false,
        running: false,
        cancel: false,
        echo,
    });
    id
}

// ── the app builtins ──────────────────────────────────────────────────────────

/// `llm_load`: a handle, or -1 (the reason on the console and in `llm_status()`).
pub fn load(path: &str) -> i64 {
    let loc = match locate(path) {
        Ok(l) => l,
        Err(e) => {
            note_err(alloc::format!("llm_load: {e}"));
            return -1;
        },
    };
    // the quick checks now, so the app hears "no" at once: readable, and not hopelessly bigger than the RAM
    let already = unsafe { (*&raw const LOADED).as_ref().is_some_and(|m| m.loc == loc) };
    if !already {
        let src = match open(&loc) {
            Ok(s) => s,
            Err(e) => {
                note_err(alloc::format!("llm_load: {e}"));
                return -1;
            },
        };
        // RAM the current model holds comes back when it is replaced
        let held = unsafe { (*&raw const LOADED).as_ref().map_or(0, |m| m.llm.model.weight_bytes() as u64) };
        let free = free_bytes() + held;
        if free < src.len() + RESERVE + (32 << 20) {
            note_err(alloc::format!(
                "llm_load: not enough memory for {loc}: {} MB free, the model alone is {} MB (try a smaller model, e.g. Qwen3-0.6B-Q8_0)",
                free >> 20,
                src.len() >> 20
            ));
            return -1;
        }
    }
    request(loc).unwrap_or(-1)
}

/// `llm_generate_start`: a job id, or "" when `handle` is not the model's (or its load failed).
pub fn generate_start(handle: i64, system: &str, history: &str, max_tokens: usize, temperature: f32, top_p: f32) -> String {
    let ok = unsafe { handle >= 0 && (*&raw const CUR).0 == handle && !(*&raw const FAILED).contains(&handle) };
    if !ok || !ensure_task() {
        return String::new();
    }
    new_job(handle, system, history, max_tokens, temperature, top_p, false)
}

pub fn generate_poll(id: &str) -> String {
    job(id).map(|j| j.text.clone()).unwrap_or_default()
}

pub fn generate_done(id: &str) -> bool {
    job(id).map_or(true, |j| j.done)
}

pub fn job_cancel(id: &str) {
    if let Some(j) = job(id) {
        j.cancel = true;
        if !j.running {
            j.done = true;
        }
    }
}

/// The builtins (from `lingapp::dispatch`); None for any other name.
pub fn dispatch(name: &[u8], args: &[u64]) -> Option<u64> {
    use crate::lingrt::{boolean, mk_str, num, to_f64, to_string, UNIT};
    let s = |i: usize| args.get(i).map(|&v| to_string(v)).unwrap_or_default();
    let n = |i: usize, d: f64| args.get(i).map(|&v| to_f64(v)).filter(|x| x.is_finite()).unwrap_or(d);
    Some(match name {
        b"daw_ai_available" => boolean(true),
        b"llm_load" => num(load(&s(0).replace('\\', "/")) as f64),
        b"llm_generate_start" => {
            let h = n(0, -1.0) as i64;
            mk_str(&generate_start(h, &s(1), &s(2), n(3, 512.0).max(1.0) as usize, n(4, 0.7) as f32, n(5, 0.9) as f32))
        },
        b"llm_generate_poll" => mk_str(&generate_poll(&s(0))),
        b"llm_generate_done" => boolean(generate_done(&s(0))),
        b"llm_job_status" => mk_str(if generate_done(&s(0)) { "done" } else { "pending" }),
        b"llm_job_tool_name" => mk_str(""),
        b"llm_job_provide_tool_result" => boolean(false),
        b"llm_job_cancel" => {
            job_cancel(&s(0));
            UNIT
        },
        b"llm_status" => mk_str(&status()),
        _ => return None,
    })
}

// ── what the Terminal calls ───────────────────────────────────────────────────

/// Start the background task (once).
pub fn start() -> bool {
    ensure_task()
}

/// Load a model: "" = the default one, "disk" = x86's raw model disk, else a path. False (with a note) when there is none.
pub fn request_load(spec: &str) -> bool {
    match locate(spec) {
        Ok(loc) => request(loc).is_some(),
        Err(e) => {
            note_err(alloc::format!("llm: {e}"));
            false
        },
    }
}

/// Queue a question (on the current model, or the default one). False when the model is busy with another.
pub fn ask(question: &str) -> bool {
    if jobs().iter().any(|j| j.echo && !j.done) {
        return false;
    }
    let cur = unsafe { (*&raw const CUR).0 };
    let failed = unsafe { (*&raw const FAILED).contains(&cur) };
    let h = if cur >= 0 && !failed {
        cur
    } else {
        // the last load failed or none was asked for: try (again) with the default model
        if failed {
            unsafe { (*&raw mut CUR) = (-1, String::new()) };
        }
        match locate("") {
            Ok(loc) => match request(loc) {
                Some(h) => h,
                None => return true,
            },
            Err(e) => {
                note_err(alloc::format!("llm: {e}"));
                return true;
            },
        }
    };
    new_job(h, SYSTEM, &alloc::format!("<|user|>\n{question}\n"), 256, 0.6, 0.9, true);
    true
}

pub fn cancel() {
    for j in jobs().iter_mut().filter(|j| !j.done) {
        j.cancel = true;
        if !j.running {
            j.done = true;
        }
    }
}

pub fn phase() -> Phase {
    unsafe { PHASE }
}

pub fn status() -> String {
    unsafe {
        let loc = (*&raw const LOADED).as_ref().map(|m| m.loc.clone()).unwrap_or_default();
        match PHASE {
            Phase::Idle => "llm: not loaded (ask something, or `llm load [path]`)".to_string(),
            Phase::Loading => alloc::format!("llm: loading {} ({} of {} steps)", (*&raw const CUR).1, PROGRESS.0, PROGRESS.1),
            Phase::Ready => alloc::format!("llm: ready ({loc}; {} MB of RAM free)", free_bytes() >> 20),
            Phase::Generating => alloc::format!("llm: thinking with {loc} (`llm stop` to interrupt)"),
            Phase::Failed => alloc::format!("llm: the last load failed: {}", &*&raw const LAST_ERR),
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
                    },
                    None => {
                        f(0, line);
                        line.clear();
                    },
                }
            }
        }
    }
}

/// The Pi's text console (and its Terminal window, through `os_shell`): `llm [status|load [path]|stop]` and `ask <question>`,
/// waiting until the model has loaded / answered and printing as it goes. The model's coroutine runs while this waits.
pub fn console_command(line: &str) {
    let (cmd, arg) = match line.trim().find(' ') {
        Some(i) => (&line.trim()[..i], line.trim()[i + 1..].trim()),
        None => (line.trim(), ""),
    };
    match (cmd, arg) {
        ("ask", "") => crate::console_write(b"usage: ask <question>\n"),
        ("ask", q) => {
            if !ask(q) {
                crate::console_write(b"llm is still busy with the last question\n");
                return;
            }
            console_wait();
        },
        ("llm", "" | "status") => {
            crate::console_write(status().as_bytes());
            crate::console_write(b"\n");
        },
        ("llm", "stop") => cancel(),
        ("llm", a) if a == "load" || a.starts_with("load ") => {
            request_load(a[4..].trim());
            console_wait();
            crate::console_write(status().as_bytes());
            crate::console_write(b"\n");
        },
        _ => crate::console_write(b"usage: llm [status|load [path.gguf]|stop], ask <question>\n"),
    }
}

fn console_wait() {
    let mut out = |k: u8, s: &str| {
        let _ = k;
        crate::console_write(s.as_bytes());
        crate::console_write(b"\n");
    };
    loop {
        // notes went to the serial log already; on the console show everything
        drain_lines(100, &mut out);
        let busy = unsafe { (*&raw const WANT).is_some() || PHASE == Phase::Loading } || jobs().iter().any(|j| j.echo && !j.done);
        if !busy {
            break;
        }
        crate::coro::yield_now();
        crate::proc::sched::yield_now();
    }
    drain_lines(100, &mut out);
}
