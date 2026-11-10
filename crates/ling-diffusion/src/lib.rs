//! ling-diffusion — Stable Diffusion inside the Ling runtime.
//!
//! One worker thread owns the GPU and the loaded checkpoint; requests are queued as JSON (the
//! AUTOMATIC1111 `/sdapi/v1/txt2img` / `img2img` body shape, plus a few LingDiffusion keys) and
//! polled by id. Everything here is fail-soft: bad input becomes a job in state "error", never a
//! panic in the interpreter. Ling has no JSON parser, so this side does all of the JSON work and
//! hands back strings the `.ling` program can pass straight to a browser.

pub mod convert;
pub mod pipeline;
pub mod prompt;
pub mod qwen;
mod edit_job;
pub use edit_job::edit_status;
use edit_job::run_edit;
pub mod sampling;
mod sd;
pub mod esrgan;
mod gpuinfo;
pub mod tokenizer;
pub mod unet;

use base64::Engine;
use candle_core::{DType, Device, Tensor};
use pipeline::{Arch, Denoiser, LoadOpts, Model};
use rand::SeedableRng;
use serde_json::{json, Value as J};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

// ── configuration ────────────────────────────────────────────────────────

fn config() -> &'static Mutex<J> {
    static C: OnceLock<Mutex<J>> = OnceLock::new();
    C.get_or_init(|| {
        Mutex::new(json!({
            "models_dir": "models/Stable-diffusion",
            "lora_dir": "models/Lora",
            "vae_dir": "models/VAE",
            "clip_dir": "models/clip",
            "outdir": "outputs",
            "device": "auto",
            "precision": "fp16",
            "checkpoint": "",
            "vae": "",
            "sliced_attention": false,
            "idle_unload_minutes": 15,
            "edit_dir": "models/qwen-image-edit-2511",
            "edit_gguf": "qwen-image-edit-2511-Q4_K_M.gguf",
            "edit_device": "cuda:1",
            "edit_encoder_device": "cuda:0",
            "edit_encoder_q8": true,
            "upscaler_dirs": ["models/ESRGAN", "models/RealESRGAN"],
        }))
    })
}

fn cfg_str(k: &str) -> String {
    config().lock().ok().and_then(|c| c.get(k).and_then(|v| v.as_str()).map(String::from)).unwrap_or_default()
}

/// Merges a JSON object into the configuration; returns the whole configuration.
pub fn set_config(patch: &str) -> String {
    if let Ok(J::Object(p)) = serde_json::from_str::<J>(patch) {
        if let Ok(mut c) = config().lock() {
            for (k, v) in p {
                c[k] = v;
            }
        }
    }
    get_config()
}

pub fn get_config() -> String {
    config().lock().map(|c| c.to_string()).unwrap_or_else(|_| "{}".into())
}

fn tokenizer() -> Result<&'static tokenizer::ClipTokenizer, String> {
    static T: OnceLock<Mutex<Option<&'static tokenizer::ClipTokenizer>>> = OnceLock::new();
    let m = T.get_or_init(|| Mutex::new(None));
    let mut g = m.lock().map_err(|_| "tokenizer lock")?;
    if let Some(t) = *g {
        return Ok(t);
    }
    let t: &'static tokenizer::ClipTokenizer = Box::leak(Box::new(tokenizer::ClipTokenizer::load(Path::new(&cfg_str("clip_dir")))?));
    *g = Some(t);
    Ok(t)
}

/// Token count of a prompt as the WebUI shows it ("26/75"), weights and lora tags removed.
pub fn count_tokens(text: &str) -> String {
    let Ok(tok) = tokenizer() else { return json!({"tokens": 0, "max": 75}).to_string() };
    let (clean, _, _) = prompt::extract_networks(text);
    let mut n = 0;
    for p in prompt::parse_attention(&clean) {
        if let prompt::Piece::Text(t, _) = p {
            n += tok.encode(&t).len();
        }
    }
    let max = ((n.max(1) + 74) / 75) * 75;
    json!({"tokens": n, "max": max}).to_string()
}

// ── jobs ─────────────────────────────────────────────────────────────────

#[derive(Default)]
struct St {
    state: String,
    message: String,
    step: usize,
    steps: usize,
    batch: usize,
    batches: usize,
    images: Vec<String>,
    pngs: Vec<Vec<u8>>,
    seeds: Vec<i64>,
    infotexts: Vec<String>,
    notes: Vec<String>,
    error: String,
    preview: String,
    preview_n: u64,
    started: Option<Instant>,
    finished: Option<Instant>,
    model: String,
}

struct Job {
    id: String,
    params: J,
    st: Mutex<St>,
    interrupt: AtomicBool,
    skip: AtomicBool,
}

struct Queue {
    q: Mutex<VecDeque<Arc<Job>>>,
    cv: Condvar,
}

fn queue() -> &'static Queue {
    static Q: OnceLock<Queue> = OnceLock::new();
    Q.get_or_init(|| {
        std::thread::Builder::new()
            .name("ling-diffusion".into())
            .spawn(worker)
            .expect("spawn the diffusion worker");
        Queue { q: Mutex::new(VecDeque::new()), cv: Condvar::new() }
    })
}

fn jobs() -> &'static Mutex<(HashMap<String, Arc<Job>>, VecDeque<String>)> {
    static J_: OnceLock<Mutex<(HashMap<String, Arc<Job>>, VecDeque<String>)>> = OnceLock::new();
    J_.get_or_init(|| Mutex::new((HashMap::new(), VecDeque::new())))
}

fn find(id: &str) -> Option<Arc<Job>> {
    jobs().lock().ok()?.0.get(id).cloned()
}

fn current() -> Option<Arc<Job>> {
    let g = jobs().lock().ok()?;
    g.1.iter().rev().filter_map(|id| g.0.get(id)).find(|j| j.st.lock().map(|s| matches!(s.state.as_str(), "loading" | "running" | "decoding")).unwrap_or(false)).cloned()
}

fn new_id() -> String {
    use rand::RngCore;
    let mut b = [0u8; 10];
    rand::rngs::OsRng.fill_bytes(&mut b);
    format!("sd{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>())
}

/// Queues a generation; returns its job id, or "ERROR: ..." for unreadable JSON.
pub fn submit(params: &str) -> String {
    let p: J = match serde_json::from_str(params) {
        Ok(J::Object(o)) => J::Object(o),
        Ok(_) => return "ERROR: the request must be a JSON object".into(),
        Err(x) => return format!("ERROR: bad JSON: {x}"),
    };
    let id = new_id();
    let job = Arc::new(Job {
        id: id.clone(),
        params: p,
        st: Mutex::new(St { state: "queued".into(), ..Default::default() }),
        interrupt: AtomicBool::new(false),
        skip: AtomicBool::new(false),
    });
    if let Ok(mut g) = jobs().lock() {
        g.0.insert(id.clone(), job.clone());
        g.1.push_back(id.clone());
        while g.1.len() > 200 {
            if let Some(old) = g.1.pop_front() {
                g.0.remove(&old);
            }
        }
        // results nobody fetched any more: keep the PNG bytes of the 16 newest jobs only
        let n = g.1.len();
        for id in g.1.iter().take(n.saturating_sub(16)) {
            if let Some(j) = g.0.get(id) {
                if let Ok(mut st) = j.st.try_lock() {
                    if st.pngs.iter().any(|b| !b.is_empty()) {
                        st.pngs.iter_mut().for_each(|b| *b = Vec::new());
                    }
                }
            }
        }
    }
    let q = queue();
    if let Ok(mut g) = q.q.lock() {
        g.push_back(job);
    }
    q.cv.notify_one();
    id
}

fn queue_pos(id: &str) -> i64 {
    queue().q.lock().ok().and_then(|g| g.iter().position(|j| j.id == id)).map(|p| p as i64 + 1).unwrap_or(0)
}

/// Job status as JSON (see the LingDiffusion API docs). Unknown ids read as state "unknown".
pub fn status(id: &str) -> String {
    let Some(j) = find(id) else { return json!({"id": id, "state": "unknown"}).to_string() };
    let s = match j.st.lock() {
        Ok(s) => s,
        Err(_) => return json!({"id": id, "state": "error", "error": "lock"}).to_string(),
    };
    let total = (s.steps * s.batches.max(1)).max(1) as f64;
    let done = (s.batch * s.steps + s.step) as f64;
    let progress = match s.state.as_str() {
        "done" | "interrupted" => 1.0,
        "queued" | "loading" => 0.0,
        _ => (done / total).min(0.999),
    };
    let elapsed = s.started.map(|t| s.finished.unwrap_or_else(Instant::now).duration_since(t).as_secs_f64()).unwrap_or(0.0);
    let eta = if progress > 0.01 && progress < 1.0 { elapsed / progress - elapsed } else { 0.0 };
    json!({
        "id": id,
        "state": s.state,
        "message": s.message,
        "queue": queue_pos(id),
        "step": s.step,
        "steps": s.steps,
        "batch": s.batch,
        "batches": s.batches,
        "progress": progress,
        "elapsed": elapsed,
        "eta": eta,
        "images": s.images,
        "seeds": s.seeds,
        "infotexts": s.infotexts,
        "notes": s.notes,
        "error": s.error,
        "preview_n": s.preview_n,
        "model": s.model,
    })
    .to_string()
}

/// The newest live preview of a job as a base64 PNG ("" if none yet).
pub fn preview(id: &str) -> String {
    find(id).and_then(|j| j.st.lock().ok().map(|s| s.preview.clone())).unwrap_or_default()
}

fn is_final(state: &str) -> bool {
    matches!(state, "done" | "error" | "interrupted" | "unknown")
}

/// Blocks up to `timeout_ms` for a job to finish; true if it did.
pub fn wait(id: &str, timeout_ms: u64) -> bool {
    let t0 = Instant::now();
    loop {
        let st = find(id).and_then(|j| j.st.lock().ok().map(|s| s.state.clone())).unwrap_or_else(|| "unknown".into());
        if is_final(&st) {
            return true;
        }
        if t0.elapsed() > Duration::from_millis(timeout_ms) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

/// `{"images": [base64 png...], "parameters": {...}, "info": "<json>"}` — the WebUI API reply.
pub fn result_a1111(id: &str) -> String {
    let Some(j) = find(id) else { return json!({"error": "unknown job", "detail": "unknown job"}).to_string() };
    let s = j.st.lock().unwrap();
    if s.state == "error" {
        return json!({"error": "GenerationError", "detail": s.error}).to_string();
    }
    let mut imgs = Vec::new();
    if j.params.get("send_images").and_then(|v| v.as_bool()).unwrap_or(true) {
        for (i, p) in s.pngs.iter().enumerate() {
            if !p.is_empty() {
                imgs.push(B64.encode(p));
            } else if let Ok(b) = std::fs::read(&s.images[i]) {
                imgs.push(B64.encode(b));
            }
        }
    }
    let prompt = j.params.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
    let info = json!({
        "prompt": prompt,
        "all_prompts": vec![prompt; s.seeds.len()],
        "negative_prompt": j.params.get("negative_prompt").and_then(|v| v.as_str()).unwrap_or(""),
        "seed": s.seeds.first().copied().unwrap_or(-1),
        "all_seeds": s.seeds,
        "infotexts": s.infotexts,
        "sd_model_name": s.model,
        "images": s.images,
        "job_timestamp": "",
    });
    json!({"images": imgs, "parameters": j.params, "info": info.to_string()}).to_string()
}

/// The WebUI's `/sdapi/v1/progress` shape, for whatever job is running now.
pub fn progress_a1111() -> String {
    let Some(j) = current() else {
        return json!({"progress": 0.0, "eta_relative": 0.0, "state": {"job": "", "job_count": 0, "sampling_step": 0, "sampling_steps": 0, "interrupted": false, "skipped": false}, "current_image": null, "textinfo": null}).to_string();
    };
    let st: J = serde_json::from_str(&status(&j.id)).unwrap_or(J::Null);
    let pv = preview(&j.id);
    json!({
        "progress": st["progress"],
        "eta_relative": st["eta"],
        "state": {"job": j.id, "job_count": st["batches"], "job_no": st["batch"], "sampling_step": st["step"], "sampling_steps": st["steps"], "interrupted": j.interrupt.load(Ordering::Relaxed), "skipped": j.skip.load(Ordering::Relaxed)},
        "current_image": if pv.is_empty() { J::Null } else { J::String(pv) },
        "textinfo": st["message"],
    })
    .to_string()
}

/// Stops a job after its current step (the images so far are still decoded and saved).
/// An empty id means "whatever is running now"; a queued job is simply dropped.
pub fn interrupt(id: &str) {
    let j = if id.is_empty() { current() } else { find(id) };
    if let Some(j) = j {
        j.interrupt.store(true, Ordering::Relaxed);
        if let Ok(mut q) = queue().q.lock() {
            if let Some(p) = q.iter().position(|x| x.id == j.id) {
                q.remove(p);
                if let Ok(mut s) = j.st.lock() {
                    s.state = "interrupted".into();
                    s.message = "removed from the queue".into();
                }
            }
        }
    }
}

/// Ends the current batch early and moves on to the next one.
pub fn skip(id: &str) {
    if let Some(j) = if id.is_empty() { current() } else { find(id) } {
        j.skip.store(true, Ordering::Relaxed);
    }
}

pub fn queue_len() -> usize {
    queue().q.lock().map(|q| q.len()).unwrap_or(0)
}

// ── what is loaded ───────────────────────────────────────────────────────

#[derive(Default, Clone)]
struct LoadedInfo {
    checkpoint: String,
    arch: String,
    device: String,
    vae: String,
    loras: Vec<String>,
}

fn loaded_info() -> &'static Mutex<LoadedInfo> {
    static L: OnceLock<Mutex<LoadedInfo>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(LoadedInfo::default()))
}

static UNLOAD: AtomicBool = AtomicBool::new(false);

pub fn loaded() -> String {
    let l = loaded_info().lock().map(|l| l.clone()).unwrap_or_default();
    json!({"checkpoint": l.checkpoint, "arch": l.arch, "device": l.device, "vae": l.vae, "loras": l.loras, "queue": queue_len(), "cuda": cfg!(feature = "cuda"), "edit": edit_status()}).to_string()
}

/// Frees the loaded model once the worker is idle.
pub fn unload() {
    UNLOAD.store(true, Ordering::Relaxed);
    queue().cv.notify_one();
}

// ── model files ──────────────────────────────────────────────────────────

fn inspect_cache() -> &'static Mutex<HashMap<String, (u64, String)>> {
    static C: OnceLock<Mutex<HashMap<String, (u64, String)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// What a weights file is, from its tensor names only (no weights are read):
/// {"kind": "sd15"|"sdxl"|"sd2"|"inpaint"|"lora"|"vae"|"unknown", "label", "size_mb"}
pub fn inspect(path: &str) -> String {
    let meta = std::fs::metadata(path);
    let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
    let stamp = size ^ meta.ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    if let Some((s, v)) = inspect_cache().lock().ok().and_then(|c| c.get(path).cloned()) {
        if s == stamp {
            return v;
        }
    }
    let kind = match pipeline::tensor_names(path) {
        Err(_) => "unknown",
        Ok(names) => {
            let has = |p: &str| names.iter().any(|(n, _)| n.starts_with(p));
            if has(convert::UNET) {
                let ctx = names.iter().find(|(n, _)| n == "model.diffusion_model.input_blocks.1.1.transformer_blocks.0.attn2.to_k.weight").and_then(|(_, s)| s.get(1).copied());
                let in_ch = names.iter().find(|(n, _)| n == "model.diffusion_model.input_blocks.0.0.weight").and_then(|(_, s)| s.get(1).copied()).unwrap_or(4);
                if in_ch != 4 {
                    "inpaint"
                } else if has(convert::CLIP_G_XL) {
                    "sdxl"
                } else if ctx == Some(1024) {
                    "sd2"
                } else {
                    "sd15"
                }
            } else if names.iter().any(|(n, _)| n.contains("lora_down") || n.contains("lora_up") || n.contains("lora_A") || n.starts_with("lora_")) {
                "lora"
            } else if has("encoder.down") || has("decoder.up") || has("first_stage_model.") || has("encoder.down_blocks") {
                "vae"
            } else {
                "unknown"
            }
        }
    };
    let label = match kind {
        "sd15" => "SD 1.5",
        "sdxl" => "SDXL",
        "sd2" => "SD 2.x (unsupported)",
        "inpaint" => "inpainting (unsupported)",
        "lora" => "LoRA",
        "vae" => "VAE",
        _ => "unknown",
    };
    let out = json!({"kind": kind, "label": label, "size_mb": (size as f64 / 1048576.0).round()}).to_string();
    if let Ok(mut c) = inspect_cache().lock() {
        c.insert(path.to_string(), (stamp, out.clone()));
    }
    out
}

/// "animeTWO.safetensors [abc123]" / "animeTWO" / a full path -> the file in models_dir.
fn resolve_in(dir: &str, name: &str, exts: &[&str]) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    if Path::new(name).is_file() {
        return Some(name.to_string());
    }
    let bare = match name.rfind(" [") {
        Some(i) if name.ends_with(']') => &name[..i],
        _ => name,
    };
    let joined = Path::new(dir).join(bare);
    if joined.is_file() {
        return Some(joined.to_string_lossy().replace('\\', "/"));
    }
    let want = Path::new(bare).file_stem().and_then(|s| s.to_str()).unwrap_or(bare).to_lowercase();
    let mut stack = vec![PathBuf::from(dir)];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
            if exts.contains(&ext.as_str()) && p.file_stem().and_then(|s| s.to_str()).map(|s| s.to_lowercase()) == Some(want.clone()) {
                return Some(p.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    None
}

// ── PNG ──────────────────────────────────────────────────────────────────

fn png_bytes(rgb: &[u8], w: usize, h: usize, text: Option<&str>) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        if let Some(t) = text {
            enc.add_itxt_chunk("parameters".into(), t.into()).map_err(|e| e.to_string())?;
        }
        let mut wr = enc.write_header().map_err(|e| e.to_string())?;
        wr.write_image_data(rgb).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// The generation parameters stored in a PNG ("parameters" text chunk), from a file path,
/// raw base64 or a data: URL. "" if there are none.
pub fn png_info(src: &str) -> String {
    let bytes = if Path::new(src).is_file() {
        std::fs::read(src).ok()
    } else {
        let b = src.split(',').next_back().unwrap_or(src);
        B64.decode(b.trim()).ok()
    };
    let Some(bytes) = bytes else { return String::new() };
    let dec = png::Decoder::new(std::io::Cursor::new(bytes));
    let Ok(r) = dec.read_info() else { return String::new() };
    let info = r.info();
    for t in &info.utf8_text {
        if t.keyword == "parameters" {
            return t.get_text().unwrap_or_default();
        }
    }
    for t in &info.uncompressed_latin1_text {
        if t.keyword == "parameters" {
            return t.text.clone();
        }
    }
    for t in &info.compressed_latin1_text {
        if t.keyword == "parameters" {
            return t.get_text().unwrap_or_default();
        }
    }
    String::new()
}

fn decode_image(src: &str) -> Result<image::RgbImage, String> {
    let bytes = if Path::new(src).is_file() {
        std::fs::read(src).map_err(|e| e.to_string())?
    } else {
        let b = src.split(',').next_back().unwrap_or(src);
        B64.decode(b.trim()).map_err(|e| format!("init image: {e}"))?
    };
    Ok(image::load_from_memory(&bytes).map_err(|e| format!("init image: {e}"))?.to_rgb8())
}

/// resize_mode: 0 just resize, 1 crop and resize, 2 resize and fill
fn fit(img: &image::RgbImage, w: u32, h: u32, mode: i64) -> image::RgbImage {
    use image::imageops::{self, FilterType};
    let (iw, ih) = img.dimensions();
    match mode {
        1 => {
            let s = (w as f64 / iw as f64).max(h as f64 / ih as f64);
            let (rw, rh) = (((iw as f64 * s).round() as u32).max(w), ((ih as f64 * s).round() as u32).max(h));
            let r = imageops::resize(img, rw, rh, FilterType::Lanczos3);
            imageops::crop_imm(&r, (rw - w) / 2, (rh - h) / 2, w, h).to_image()
        }
        2 => {
            let s = (w as f64 / iw as f64).min(h as f64 / ih as f64);
            let (rw, rh) = (((iw as f64 * s).round() as u32).clamp(1, w), ((ih as f64 * s).round() as u32).clamp(1, h));
            let r = imageops::resize(img, rw, rh, FilterType::Lanczos3);
            // fill with the stretched picture, then put the fitted one on top
            let mut bg = imageops::resize(img, w, h, FilterType::Triangle);
            imageops::overlay(&mut bg, &r, ((w - rw) / 2) as i64, ((h - rh) / 2) as i64);
            bg
        }
        _ => imageops::resize(img, w, h, FilterType::Lanczos3),
    }
}

// ── the worker ───────────────────────────────────────────────────────────

fn worker() {
    let mut model: Option<Model> = None;
    let mut edit: Option<qwen::EditModel> = None;
    let mut last_used = Instant::now();
    loop {
        let job = {
            let q = queue();
            let mut g = match q.q.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            loop {
                // idle_unload_minutes (0 = never): free the GPUs when nobody has generated for a while
                let idle_min = config().lock().ok().and_then(|c| c["idle_unload_minutes"].as_f64()).unwrap_or(15.0);
                let idle = idle_min > 0.0 && (model.is_some() || edit.is_some()) && last_used.elapsed() > Duration::from_secs_f64(idle_min * 60.0);
                if UNLOAD.swap(false, Ordering::Relaxed) || idle {
                    model = None;
                    edit = None;
                    if let Ok(mut u) = upscaler_slot().lock() {
                        *u = None;
                    }
                    edit_job::clear_edit_status();
                    if let Ok(mut l) = loaded_info().lock() {
                        *l = LoadedInfo::default();
                    }
                    trim_gpus();
                }
                if let Some(j) = g.pop_front() {
                    break j;
                }
                g = match q.cv.wait_timeout(g, Duration::from_millis(500)) {
                    Ok((g, _)) => g,
                    Err(_) => return,
                };
            }
        };
        if let Ok(mut s) = job.st.lock() {
            s.state = "loading".into();
            s.started = Some(Instant::now());
        }
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_job(&job, &mut model, &mut edit)));
        let mut s = job.st.lock().unwrap_or_else(|p| p.into_inner());
        match r {
            Ok(Ok(())) => {
                s.state = if job.interrupt.load(Ordering::Relaxed) { "interrupted".into() } else { "done".into() };
                s.message.clear();
            }
            Ok(Err(e)) => {
                s.state = "error".into();
                s.error = e;
            }
            Err(p) => {
                s.state = "error".into();
                s.error = format!("the engine panicked: {}", p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default());
                model = None; // its state is unknown now
                edit = None;
            }
        }
        s.finished = Some(Instant::now());
        drop(s);
        trim_gpus();
        last_used = Instant::now();
    }
}

fn pnum(p: &J, k: &str, d: f64) -> f64 {
    p.get(k).and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))).unwrap_or(d)
}
fn pstr<'a>(p: &'a J, k: &str) -> &'a str {
    p.get(k).and_then(|v| v.as_str()).unwrap_or("")
}
fn pbool(p: &J, k: &str, d: bool) -> bool {
    p.get(k).and_then(|v| v.as_bool().or_else(|| v.as_str().map(|s| s == "true" || s == "1" || s == "on")).or_else(|| v.as_f64().map(|x| x != 0.0))).unwrap_or(d)
}

fn devices() -> &'static Mutex<HashMap<usize, Device>> {
    static D: OnceLock<Mutex<HashMap<usize, Device>>> = OnceLock::new();
    D.get_or_init(|| Mutex::new(HashMap::new()))
}

/// "cuda:1" -> the one Device kept for that GPU. Creating a Device per job would build a new
/// stream and reload every kernel module each time.
fn parse_device(s: &str) -> (Device, String) {
    let s = s.trim().to_ascii_lowercase();
    if s == "cpu" {
        return (Device::Cpu, "cpu".into());
    }
    let n: usize = if s == "auto" || s.is_empty() { auto_gpu() } else { s.trim_start_matches("cuda").trim_start_matches(':').parse().unwrap_or(0) };
    #[cfg(feature = "cuda")]
    let n = if n >= gpu_count() { 0 } else { n };
    if let Some(d) = devices().lock().ok().and_then(|m| m.get(&n).cloned()) {
        return (d, format!("cuda:{n}"));
    }
    match Device::new_cuda(n) {
        Ok(d) => {
            if let Ok(mut m) = devices().lock() {
                m.insert(n, d.clone());
            }
            (d, format!("cuda:{n}"))
        }
        Err(_) => (Device::Cpu, "cpu".into()),
    }
}

#[cfg(feature = "cuda")]
fn gpu_count() -> usize {
    candle_core::cuda_backend::cudarc::driver::CudaContext::device_count().map(|n| n.max(0) as usize).unwrap_or(0)
}

/// "auto": a GPU not driving the monitors, else the one with the most free memory; picked once per run. On a two-GPU machine that is the
/// one not driving the monitors; rendering on the display GPU makes the desktop stall whenever a
/// large image pushes its memory to the limit.
/// The ordinal "auto" resolves to.
pub fn auto_device() -> usize {
    auto_gpu()
}

fn auto_gpu() -> usize {
    static PICK: OnceLock<usize> = OnceLock::new();
    *PICK.get_or_init(|| {
        #[cfg(feature = "cuda")]
        {
            use candle_core::cuda_backend::cudarc::driver::{sys::CUdevice_attribute as A, CudaContext};
            // not driving a monitor first, then the most memory free across all processes
            let mut best: Option<(usize, (bool, u64))> = None;
            for i in 0..gpu_count() {
                let Ok(ctx) = CudaContext::new(i) else { continue };
                let at = |a| ctx.attribute(a).unwrap_or(0);
                let pci = format!("{:08X}:{:02X}:{:02X}.0", at(A::CU_DEVICE_ATTRIBUTE_PCI_DOMAIN_ID), at(A::CU_DEVICE_ATTRIBUTE_PCI_BUS_ID), at(A::CU_DEVICE_ATTRIBUTE_PCI_DEVICE_ID));
                let Some(g) = gpuinfo::query(&pci) else { continue };
                let score = (!g.display, g.total.saturating_sub(g.used));
                if best.map(|(_, s)| score > s).unwrap_or(true) {
                    best = Some((i, score));
                }
            }
            best.map(|b| b.0).unwrap_or(0)
        }
        #[cfg(not(feature = "cuda"))]
        0
    })
}

/// Hands GPU memory the job no longer uses back to the driver. candle allocates from CUDA's
/// stream-ordered pool, which only trims itself on a stream synchronize, and candle never calls
/// one; on Windows every byte the pool keeps is also committed system memory, so without this
/// a finished SDXL run could keep ~14 GB committed while the app sits idle.
fn trim_gpus() {
    #[cfg(feature = "cuda")]
    {
        use candle_core::cuda_backend::cudarc::driver::result;
        let devs: Vec<Device> = devices().lock().map(|m| m.values().cloned().collect()).unwrap_or_default();
        for d in devs {
            if let Device::Cuda(cd) = d {
                let stream = cd.cuda_stream();
                let _ = stream.synchronize();
                let cu = stream.context().cu_device();
                unsafe {
                    if let Ok(pool) = result::device::get_default_mem_pool(cu) {
                        let _ = result::mem_pool::trim_to(pool, 0);
                    }
                }
            }
        }
    }
}

/// task "upscale": image (path / base64 / data URL), upscaler, upscaling_resize (factor) or
/// resize_mode 1 with upscaling_resize_w/_h, save_images. Saved to extras-images, like the WebUI's Extras.
fn run_upscale(job: &Arc<Job>, edit: &mut Option<qwen::EditModel>) -> Result<(), String> {
    let p = &job.params;
    let first_init = p.get("init_images").and_then(|a| a.as_array()).and_then(|a| a.first()).and_then(|v| v.as_str()).unwrap_or("");
    let src = [pstr(p, "image"), first_init].into_iter().find(|s| !s.is_empty()).ok_or("upscale needs an image")?.to_string();
    let img = decode_image(&src)?;
    let name = match pstr(p, "upscaler") {
        "" => pstr(p, "upscaler_1").to_string(),
        n => n.to_string(),
    };
    let label = if is_plain_resize(&name) { "Lanczos".to_string() } else { name.clone() };
    let factor = pnum(p, "upscaling_resize", 2.0).clamp(1.0, 8.0);
    let (mut tw, mut th) = ((img.width() as f64 * factor).round() as usize, (img.height() as f64 * factor).round() as usize);
    if pnum(p, "resize_mode", 0.0) as i64 == 1 {
        tw = pnum(p, "upscaling_resize_w", tw as f64) as usize;
        th = pnum(p, "upscaling_resize_h", th as f64) as usize;
    }
    if tw.max(th) > 8192 || tw == 0 || th == 0 {
        return Err(format!("{tw}x{th}: the output must be 1..8192 px per side"));
    }
    let (device, _) = parse_device(&[pstr(p, "device"), &cfg_str("device")].into_iter().find(|s| !s.is_empty()).unwrap_or("auto").to_string());
    if !is_plain_resize(&name) && edit.as_ref().map(|e| e.uses(&device)).unwrap_or(false) {
        // the edit model can fill the card; it reloads when the Edit tab is next used
        *edit = None;
        edit_job::clear_edit_status();
        trim_gpus();
    }
    if let Ok(mut s) = job.st.lock() {
        s.state = "running".into();
        s.steps = 1;
        s.batches = 1;
        s.message = format!("upscaling {}x{} to {tw}x{th} with {label}", img.width(), img.height());
        s.model = label.clone();
    }
    let big = upscale_with(&name, &img, tw, th, &device, &|| job.interrupt.load(Ordering::Relaxed))?;
    let info = format!("Postprocess upscale by: {factor}, Postprocess upscaler: {label}, Size: {tw}x{th}, Version: LingDiffusion");
    let png = png_bytes(big.as_raw(), tw, th, Some(&info))?;
    let save = pbool(p, "save_images", true);
    let path = if save {
        let dir = PathBuf::from(match pstr(p, "outdir") {
            "" => format!("{}/extras-images", cfg_str("outputs_root")),
            d => d.to_string(),
        });
        std::fs::create_dir_all(&dir).map_err(|e| format!("output folder {}: {e}", dir.display()))?;
        let f = dir.join(format!("{:05}.png", next_index(&dir)));
        std::fs::write(&f, &png).map_err(|e| format!("saving {}: {e}", f.display()))?;
        f.to_string_lossy().replace('\\', "/")
    } else {
        String::new()
    };
    if let Ok(mut s) = job.st.lock() {
        s.images.push(path);
        s.pngs.push(if save { Vec::new() } else { png });
        s.seeds.push(-1);
        s.infotexts.push(info);
        s.step = 1;
    }
    Ok(())
}

fn next_index(dir: &Path) -> usize {
    let mut n = 0;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let digits: String = name.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(v) = digits.parse::<usize>() {
                n = n.max(v + 1);
            }
        }
    }
    n
}

fn run_job(job: &Arc<Job>, slot: &mut Option<Model>, edit: &mut Option<qwen::EditModel>) -> Result<(), String> {
    let p = &job.params;
    if pstr(p, "task") == "edit" {
        return run_edit(job, slot, edit);
    }
    if pstr(p, "task") == "upscale" {
        return run_upscale(job, edit);
    }
    let ov = p.get("override_settings").cloned().unwrap_or(J::Null);
    let set = |s: &str| {
        if let Ok(mut st) = job.st.lock() {
            st.message = s.to_string();
        }
    };

    // ── what to load ──
    let models_dir = cfg_str("models_dir");
    let ck_name = [pstr(p, "checkpoint"), pstr(&ov, "sd_model_checkpoint"), &cfg_str("checkpoint")].into_iter().find(|s| !s.is_empty()).unwrap_or("").to_string();
    if ck_name.is_empty() {
        return Err("no checkpoint selected".into());
    }
    let checkpoint = resolve_in(&models_dir, &ck_name, &["safetensors", "ckpt"]).ok_or_else(|| format!("checkpoint not found: {ck_name}"))?;
    let vae_name = [pstr(p, "vae"), pstr(&ov, "sd_vae"), &cfg_str("vae")].into_iter().find(|s| !s.is_empty()).unwrap_or("").to_string();
    let vae_path = match vae_name.as_str() {
        "" | "Automatic" | "None" | "auto" => String::new(),
        n => resolve_in(&cfg_str("vae_dir"), n, &["safetensors", "ckpt", "pt"]).ok_or_else(|| format!("VAE not found: {n}"))?,
    };
    let raw_prompt = pstr(p, "prompt").to_string();
    let raw_neg = pstr(p, "negative_prompt").to_string();
    let (prompt_text, lora_tags, ignored) = prompt::extract_networks(&raw_prompt);
    let (neg_text, neg_loras, _) = prompt::extract_networks(&raw_neg);
    let mut notes: Vec<String> = ignored.iter().map(|t| format!("ignored <{t}>: hypernetworks are not supported")).collect();
    let mut loras = Vec::new();
    for t in lora_tags.iter().chain(neg_loras.iter()) {
        match resolve_in(&cfg_str("lora_dir"), &t.name, &["safetensors", "ckpt", "pt"]) {
            Some(path) if t.weight != 0.0 => loras.push((path, t.weight)),
            Some(_) => {}
            None => notes.push(format!("LoRA not found: {}", t.name)),
        }
    }
    let (device, dev_label) = parse_device(&[pstr(p, "device"), &cfg_str("device")].into_iter().find(|s| !s.is_empty()).unwrap_or("auto").to_string());
    let precision = [pstr(p, "precision"), &cfg_str("precision")].into_iter().find(|s| !s.is_empty()).unwrap_or("fp16").to_string();
    let dtype = match (device.is_cpu(), precision.as_str()) {
        (true, _) => DType::F32,
        (_, "fp32") => DType::F32,
        (_, "bf16") => DType::BF16,
        _ => DType::F16,
    };
    let sliced = pbool(p, "sliced_attention", config().lock().ok().and_then(|c| c["sliced_attention"].as_bool()).unwrap_or(false));
    let opts = LoadOpts { checkpoint: checkpoint.clone(), vae: vae_path.clone(), loras: loras.clone(), device, dtype, sliced_attention: sliced };
    let key = pipeline::model_key(&opts);
    // the edit model (~16 GB with its encoder) on the same GPU leaves no room to render: drop it,
    // the Edit tab reloads it when it is next used
    if edit.as_ref().map(|e| e.uses(&opts.device)).unwrap_or(false) {
        set("unloading the edit model from this GPU");
        *edit = None;
        edit_job::clear_edit_status();
        trim_gpus();
    }
    if slot.as_ref().map(|m| m.key != key).unwrap_or(true) {
        *slot = None; // free the old weights before reading the new ones
        if let Ok(mut l) = loaded_info().lock() {
            *l = LoadedInfo::default();
        }
        set(&format!("loading {}", Path::new(&checkpoint).file_name().and_then(|s| s.to_str()).unwrap_or("")));
        let m = Model::load(&opts, &|s| set(s))?;
        if let Ok(mut l) = loaded_info().lock() {
            *l = LoadedInfo {
                checkpoint: checkpoint.clone(),
                arch: m.arch.label().into(),
                device: dev_label.clone(),
                vae: vae_path.clone(),
                loras: loras.iter().map(|(p, w)| format!("{}:{w}", Path::new(p).file_stem().and_then(|s| s.to_str()).unwrap_or(""))).collect(),
            };
        }
        *slot = Some(m);
    }
    let m = slot.as_ref().unwrap();
    notes.extend(m.lora_notes.iter().cloned());
    let model_name = Path::new(&checkpoint).file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
    if pbool(p, "load_only", false) {
        // selecting a checkpoint in the UI: load it now so the first Generate is quick
        if let Ok(mut s) = job.st.lock() {
            s.model = model_name;
            s.notes = notes;
        }
        return Ok(());
    }

    // ── parameters ──
    let default_size = if m.arch == Arch::Sdxl { 1024.0 } else { 512.0 };
    let w = ((pnum(p, "width", default_size) as usize).clamp(64, 4096) / 8) * 8;
    let h = ((pnum(p, "height", default_size) as usize).clamp(64, 4096) / 8) * 8;
    let steps = (pnum(p, "steps", 20.0) as usize).clamp(1, 150);
    let cfg = pnum(p, "cfg_scale", 7.0).clamp(1.0, 30.0);
    let batch = (pnum(p, "batch_size", 1.0) as usize).clamp(1, 8);
    let n_iter = (pnum(p, "n_iter", 1.0) as usize).clamp(1, 100);
    let clip_skip = pnum(p, "clip_skip", pnum(&ov, "CLIP_stop_at_last_layers", 1.0)) as usize;
    let mut seed = pnum(p, "seed", -1.0) as i64;
    if seed < 0 {
        seed = (rand::random::<u32>() & 0x7fff_ffff) as i64;
    }
    let (sampler, schedule, sampler_name, schedule_name) = sampling::parse(pstr(p, "sampler_name").trim().trim_matches('"'), pstr(p, "scheduler"));
    let sampler_name = if sampler_name.is_empty() { "Euler a".into() } else { sampler_name };
    let init = p.get("init_images").and_then(|a| a.as_array()).and_then(|a| a.first()).and_then(|v| v.as_str()).map(String::from);
    let denoise = pnum(p, "denoising_strength", 0.75).clamp(0.0, 1.0);
    let enable_hr = init.is_none() && pbool(p, "enable_hr", false);
    let hr_scale = pnum(p, "hr_scale", 2.0).clamp(1.0, 4.0);
    let hr_steps = match pnum(p, "hr_second_pass_steps", 0.0) as usize {
        0 => steps,
        n => n.min(150),
    };
    let hr_upscaler = pstr(p, "hr_upscaler").to_string();
    let (hw, hh) = (((w as f64 * hr_scale) as usize / 8) * 8, ((h as f64 * hr_scale) as usize / 8) * 8);
    let save = pbool(p, "save_images", true);
    let outdir = PathBuf::from(match pstr(p, "outdir") {
        "" => cfg_str("outdir"),
        d => d.to_string(),
    });
    if save {
        std::fs::create_dir_all(&outdir).map_err(|e| format!("output folder {}: {e}", outdir.display()))?;
    }
    let tok = tokenizer()?;
    let table = sampling::Table::sd();

    // ── conditioning ──
    set("encoding prompt");
    let (cond, unc) = m.encode_pair(tok, &prompt_text, &neg_text, clip_skip)?;
    let er = |x: candle_core::Error| x.to_string();
    let rep = |t: &Tensor, n: usize| -> Result<Tensor, String> { Tensor::cat(&vec![t; n], 0).map_err(er) };
    let ctx_for = |n: usize| -> Result<Tensor, String> {
        let t = if cfg != 1.0 { Tensor::cat(&[&rep(&unc.ctx, n)?, &rep(&cond.ctx, n)?], 0).map_err(er)? } else { rep(&cond.ctx, n)? };
        t.to_dtype(m.dtype).map_err(er)
    };
    let added_for = |n: usize, ow: usize, oh: usize| -> Result<Option<(Tensor, Tensor)>, String> {
        let (Some(pc), Some(pu)) = (&cond.pooled, &unc.pooled) else { return Ok(None) };
        let ids = Tensor::new(&[oh as f32, ow as f32, 0.0, 0.0, oh as f32, ow as f32], &m.device).map_err(er)?.unsqueeze(0).map_err(er)?;
        let tot = if cfg != 1.0 { 2 * n } else { n };
        let pooled = if cfg != 1.0 { Tensor::cat(&[&rep(pu, n)?, &rep(pc, n)?], 0).map_err(er)? } else { rep(pc, n)? };
        Ok(Some((pooled, rep(&ids, tot)?)))
    };

    let init_latent = match &init {
        Some(src) => {
            set("encoding init image");
            let img = fit(&decode_image(src)?, w as u32, h as u32, pnum(p, "resize_mode", 0.0) as i64);
            Some(m.encode_image(img.as_raw(), w, h)?)
        }
        None => None,
    };

    let mut index = if save { next_index(&outdir) } else { 0 };
    // the WebUI (without "do exactly the amount of steps") runs steps * denoising for img2img and the
    // hires pass, from a schedule of the full step count
    let img_steps = if init.is_some() { ((steps as f64 * denoise) as usize).clamp(1, steps) } else { steps };
    let hr_run = ((hr_steps as f64 * denoise) as usize).clamp(1, hr_steps);
    let total_steps_per_batch = img_steps + if enable_hr { hr_run } else { 0 };
    if let Ok(mut s) = job.st.lock() {
        s.state = "running".into();
        s.steps = total_steps_per_batch;
        s.batches = n_iter;
        s.notes = notes.clone();
        s.model = model_name.clone();
    }

    for it in 0..n_iter {
        if job.interrupt.load(Ordering::Relaxed) {
            break;
        }
        job.skip.store(false, Ordering::Relaxed);
        if let Ok(mut s) = job.st.lock() {
            s.batch = it;
            s.step = 0;
            s.message = format!("batch {}/{}", it + 1, n_iter);
        }
        let seeds: Vec<i64> = (0..batch).map(|b| seed + (it * batch + b) as i64).collect();
        let mut rngs: Vec<rand::rngs::StdRng> = seeds.iter().map(|s| rand::rngs::StdRng::seed_from_u64(*s as u64)).collect();
        let (lh, lw) = (h / 8, w / 8);
        let mut noise = Vec::with_capacity(batch * 4 * lh * lw);
        for r in rngs.iter_mut() {
            noise.extend(pipeline::randn(r, 4 * lh * lw));
        }
        let noise = Tensor::from_vec(noise, (batch, 4, lh, lw), &m.device).map_err(er)?;

        // first pass
        let (sigmas, x0) = match &init_latent {
            None => {
                let s = table.sigmas(steps, schedule);
                let x = (&noise * s[0]).map_err(er)?;
                (s, x)
            }
            Some(lat) => {
                // like the WebUI: a schedule of `steps`, of which the last steps * denoising run
                let n = img_steps;
                let all = table.sigmas(steps, schedule);
                let s = all[steps - n..].to_vec();
                let x = (rep(lat, batch)? + (&noise * s[0]).map_err(er)?).map_err(er)?;
                (s, x)
            }
        };
        let run = |sigmas: &[f64], x: Tensor, ctx2: Tensor, added2: Option<(Tensor, Tensor)>, rngs: Vec<rand::rngs::StdRng>, base: usize| -> Result<Tensor, String> {
            let mut last_pv = Instant::now() - Duration::from_secs(5);
            let mut on_step = |i: usize, den: &Tensor| -> bool {
                if let Ok(mut s) = job.st.lock() {
                    s.step = base + i + 1;
                }
                if last_pv.elapsed() > Duration::from_millis(700) && i + 1 < sigmas.len() - 1 {
                    last_pv = Instant::now();
                    if let Ok((rgb, pw, ph)) = m.preview(den) {
                        if let Ok(png) = png_bytes(&rgb, pw, ph, None) {
                            if let Ok(mut s) = job.st.lock() {
                                s.preview = B64.encode(png);
                                s.preview_n += 1;
                            }
                        }
                    }
                }
                !(job.interrupt.load(Ordering::Relaxed) || job.skip.load(Ordering::Relaxed))
            };
            let mut d = Denoiser { model: m, table: &table, ctx2, added2, cfg, batch, rngs, on_step: &mut on_step };
            pipeline::run_sampler(sampler, sigmas, x, &mut d)
        };
        let mut lat = run(&sigmas, x0, ctx_for(batch)?, added_for(batch, w, h)?, rngs, 0)?;
        let (mut ow, mut oh) = (w, h);

        // hires fix: upscale, then a partial second pass at the new size
        if enable_hr && !job.interrupt.load(Ordering::Relaxed) && !job.skip.load(Ordering::Relaxed) {
            set(&format!("hires fix {hw}x{hh}"));
            let up = if hr_upscaler.to_ascii_lowercase().starts_with("latent") {
                lat.upsample_nearest2d(hh / 8, hw / 8).map_err(er)?
            } else {
                let mut parts = Vec::new();
                for b in 0..batch {
                    let rgb = m.decode(&lat.narrow(0, b, 1).map_err(er)?)?;
                    let img = image::RgbImage::from_raw(w as u32, h as u32, rgb).ok_or("decode size")?;
                    set(&format!("upscaling with {}", if hr_upscaler.is_empty() { "Lanczos" } else { &hr_upscaler }));
                    let big = upscale_with(&hr_upscaler, &img, hw, hh, &m.device, &|| job.interrupt.load(Ordering::Relaxed))?;
                    set(&format!("hires fix {hw}x{hh}"));
                    parts.push(m.encode_image(big.as_raw(), hw, hh)?);
                }
                Tensor::cat(&parts.iter().collect::<Vec<_>>(), 0).map_err(er)?
            };
            let all = table.sigmas(hr_steps, schedule);
            let s = all[hr_steps - hr_run..].to_vec();
            let mut rngs2: Vec<rand::rngs::StdRng> = seeds.iter().map(|s| rand::rngs::StdRng::seed_from_u64((*s as u64) ^ 0x9e37_79b9)).collect();
            let mut nz = Vec::new();
            for r in rngs2.iter_mut() {
                nz.extend(pipeline::randn(r, 4 * (hh / 8) * (hw / 8)));
            }
            let nz = Tensor::from_vec(nz, (batch, 4, hh / 8, hw / 8), &m.device).map_err(er)?;
            let x = (up + (nz * s[0]).map_err(er)?).map_err(er)?;
            lat = run(&s, x, ctx_for(batch)?, added_for(batch, hw, hh)?, rngs2, img_steps)?;
            ow = hw;
            oh = hh;
        }

        // decode + save
        if let Ok(mut s) = job.st.lock() {
            s.state = "decoding".into();
        }
        for b in 0..batch {
            let rgb = m.decode(&lat.narrow(0, b, 1).map_err(er)?)?;
            let mut info = raw_prompt.clone();
            if !raw_neg.trim().is_empty() {
                info.push_str(&format!("\nNegative prompt: {raw_neg}"));
            }
            info.push_str(&format!("\nSteps: {steps}, Sampler: {sampler_name}"));
            info.push_str(&format!(", Schedule type: {schedule_name}"));
            info.push_str(&format!(", CFG scale: {cfg}, Seed: {}, Size: {w}x{h}, Model: {model_name}", seeds[b]));
            if !vae_path.is_empty() {
                info.push_str(&format!(", VAE: {}", Path::new(&vae_path).file_name().and_then(|s| s.to_str()).unwrap_or("")));
            }
            if init.is_some() || enable_hr {
                info.push_str(&format!(", Denoising strength: {denoise}"));
            }
            if enable_hr {
                info.push_str(&format!(", Hires upscale: {hr_scale}, Hires steps: {hr_steps}, Hires upscaler: {}", if hr_upscaler.is_empty() { "Latent (nearest)" } else { &hr_upscaler }));
            }
            if clip_skip > 1 {
                info.push_str(&format!(", Clip skip: {clip_skip}"));
            }
            if !loras.is_empty() {
                let l: Vec<String> = loras.iter().map(|(p, _)| Path::new(p).file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string()).collect();
                info.push_str(&format!(", Lora: \"{}\"", l.join(", ")));
            }
            info.push_str(", Version: LingDiffusion");
            let png = png_bytes(&rgb, ow, oh, Some(&info))?;
            let path = if save {
                let f = outdir.join(format!("{index:05}-{}.png", seeds[b]));
                index += 1;
                std::fs::write(&f, &png).map_err(|e| format!("saving {}: {e}", f.display()))?;
                f.to_string_lossy().replace('\\', "/")
            } else {
                String::new()
            };
            if let Ok(mut s) = job.st.lock() {
                s.images.push(path);
                s.pngs.push(if save { Vec::new() } else { png });
                s.seeds.push(seeds[b]);
                s.infotexts.push(info);
            }
        }
        if let Ok(mut s) = job.st.lock() {
            s.state = "running".into();
            s.step = total_steps_per_batch;
        }
    }
    if let Ok(mut s) = job.st.lock() {
        s.batch = n_iter.saturating_sub(1);
        s.preview.clear();
    }
    Ok(())
}

/// A field of a JSON object as a string ("" if absent) — for .ling code, which has no JSON parser.
/// `path` is dot-separated ("override_settings.sd_model_checkpoint", "images.0").
pub fn json_get(src: &str, path: &str) -> String {
    let Ok(mut v) = serde_json::from_str::<J>(src) else { return String::new() };
    for k in path.split('.').filter(|k| !k.is_empty()) {
        v = match v {
            J::Object(mut o) => o.remove(k).unwrap_or(J::Null),
            J::Array(mut a) => match k.parse::<usize>() {
                Ok(i) if i < a.len() => a.swap_remove(i),
                _ => J::Null,
            },
            _ => J::Null,
        };
    }
    match v {
        J::Null => String::new(),
        J::String(s) => s,
        other => other.to_string(),
    }
}

// ── upscalers (ESRGAN / Real-ESRGAN) ──────────────────────────────────────

fn upscaler_dirs() -> Vec<String> {
    config().lock().ok().and_then(|c| c.get("upscaler_dirs").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())).unwrap_or_default()
}

fn upscaler_files() -> Vec<String> {
    let mut out = Vec::new();
    for d in upscaler_dirs() {
        let mut stack = vec![PathBuf::from(&d)];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for en in rd.flatten() {
                let p = en.path();
                if p.is_dir() {
                    stack.push(p);
                } else if matches!(p.extension().and_then(|x| x.to_str()).map(|x| x.to_ascii_lowercase()).as_deref(), Some("pth" | "pt" | "safetensors")) {
                    let s = p.to_string_lossy().replace('\\', "/");
                    if !out.contains(&s) {
                        out.push(s);
                    }
                }
            }
        }
    }
    // a converted copy (name.safetensors) hides the old-format name.pth it came from
    let st: Vec<String> = out.iter().filter(|s| s.to_ascii_lowercase().ends_with(".safetensors")).filter_map(|s| Path::new(s).file_stem().map(|x| x.to_string_lossy().to_lowercase())).collect();
    out.retain(|s| s.to_ascii_lowercase().ends_with(".safetensors") || !st.contains(&Path::new(s).file_stem().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default()));
    out.sort_by_key(|s| Path::new(s).file_stem().map(|x| x.to_string_lossy().to_lowercase()));
    out
}

fn guess_scale(name: &str) -> f64 {
    let n = name.to_ascii_lowercase();
    for k in [1, 2, 3, 4, 8] {
        if n.contains(&format!("x{k}")) || n.contains(&format!("{k}x")) {
            return k as f64;
        }
    }
    4.0
}

/// The upscaler file for a name ("RealESRGAN_x4plus_anime_6B", a file name, or a path).
fn find_upscaler(name: &str) -> Option<String> {
    if Path::new(name).is_file() {
        return Some(name.to_string());
    }
    let want = name.trim().to_lowercase();
    upscaler_files().into_iter().find(|f| {
        let p = Path::new(f);
        p.file_stem().map(|s| s.to_string_lossy().to_lowercase() == want).unwrap_or(false) || p.file_name().map(|s| s.to_string_lossy().to_lowercase() == want).unwrap_or(false)
    })
}

/// The WebUI's /sdapi/v1/upscalers list: None, Lanczos, Nearest, then every ESRGAN-family file.
pub fn upscalers_json() -> String {
    let mut v = vec![
        json!({"name": "None", "model_name": null, "model_path": null, "model_url": null, "scale": 4}),
        json!({"name": "Lanczos", "model_name": null, "model_path": null, "model_url": null, "scale": 4}),
        json!({"name": "Nearest", "model_name": null, "model_path": null, "model_url": null, "scale": 4}),
    ];
    for f in upscaler_files() {
        let n = Path::new(&f).file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        v.push(json!({"name": n, "model_name": "ESRGAN", "model_path": f, "model_url": null, "scale": guess_scale(&n)}));
    }
    J::Array(v).to_string()
}

fn upscaler_slot() -> &'static Mutex<Option<esrgan::Esrgan>> {
    static U: OnceLock<Mutex<Option<esrgan::Esrgan>>> = OnceLock::new();
    U.get_or_init(|| Mutex::new(None))
}

fn is_plain_resize(name: &str) -> bool {
    matches!(name.trim().to_ascii_lowercase().as_str(), "" | "none" | "lanczos" | "nearest")
}

/// Resizes `img` to (w, h) with the named upscaler: "Lanczos", "Nearest", or an ESRGAN model.
fn upscale_with(name: &str, img: &image::RgbImage, w: usize, h: usize, device: &Device, cancel: &dyn Fn() -> bool) -> Result<image::RgbImage, String> {
    use image::imageops::FilterType;
    match name.trim().to_ascii_lowercase().as_str() {
        "" | "none" | "lanczos" => return Ok(image::imageops::resize(img, w as u32, h as u32, FilterType::Lanczos3)),
        "nearest" => return Ok(image::imageops::resize(img, w as u32, h as u32, FilterType::Nearest)),
        _ => {}
    }
    let path = find_upscaler(name).ok_or_else(|| format!("upscaler not found: {name} (put ESRGAN .pth files in models/ESRGAN)"))?;
    let stem = Path::new(&path).file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
    let mut g = upscaler_slot().lock().map_err(|_| "upscaler lock")?;
    let stale = g.as_ref().map(|u| u.name != stem || format!("{:?}", u.device.location()) != format!("{:?}", device.location())).unwrap_or(true);
    if stale {
        *g = None;
        *g = Some(esrgan::Esrgan::load(&path, device).map_err(|x| format!("upscaler {name}: {x}"))?);
    }
    g.as_ref().unwrap().upscale_to(img, w, h, cancel)
}

pub fn samplers_json() -> String {
    J::Array(sampling::SAMPLERS.iter().map(|s| json!({"name": s, "aliases": [], "options": {}})).collect()).to_string()
}

pub fn schedulers_json() -> String {
    J::Array(sampling::SCHEDULES.iter().map(|s| json!({"name": s.to_lowercase(), "label": s})).collect()).to_string()
}
