//! Instruction editing jobs (Qwen-Image-Edit-2511) on the same queue, progress and PNG/infotext
//! handling as the Stable Diffusion jobs. Selected with `"task": "edit"`.

use super::*;

fn edit_loaded() -> &'static Mutex<String> {
    static E: OnceLock<Mutex<String>> = OnceLock::new();
    E.get_or_init(|| Mutex::new(String::new()))
}

/// What the edit engine has loaded ("" when nothing).
pub fn edit_status() -> String {
    edit_loaded().lock().map(|s| s.clone()).unwrap_or_default()
}

pub(crate) fn clear_edit_status() {
    if let Ok(mut l) = edit_loaded().lock() {
        l.clear();
    }
}

fn cfg_bool(k: &str, d: bool) -> bool {
    config().lock().ok().and_then(|c| c.get(k).and_then(|v| v.as_bool())).unwrap_or(d)
}

fn pick(p: &J, key: &str, cfg_key: &str, default: &str) -> String {
    let a = pstr(p, key);
    if !a.is_empty() {
        return a.to_string();
    }
    let c = cfg_str(cfg_key);
    if !c.is_empty() {
        return c;
    }
    default.to_string()
}

/// params: task "edit", init_images[0], prompt, negative_prompt, edit_mode ("lightning8" |
/// "lightning4" | "full"), steps, cfg_scale (true CFG, full mode), seed, megapixels, n_iter,
/// lora_strength, outdir, save_images
pub(crate) fn run_edit(job: &Arc<Job>, sd: &mut Option<Model>, slot: &mut Option<qwen::EditModel>) -> Result<(), String> {
    let p = &job.params;
    let set = |s: &str| {
        if let Ok(mut st) = job.st.lock() {
            st.message = s.to_string();
        }
    };
    let dir = pick(p, "edit_dir", "edit_dir", "models/qwen-image-edit-2511");
    let gguf = pick(p, "edit_gguf", "edit_gguf", "qwen-image-edit-2511-Q4_K_M.gguf");
    let mode = match pstr(p, "edit_mode") {
        "" => "lightning8".to_string(),
        m => m.to_string(),
    };
    let (lightning, def_steps, def_cfg) = match mode.as_str() {
        "lightning4" => (Some("Qwen-Image-Edit-2511-Lightning-4steps-V1.0-bf16.safetensors"), 4.0, 1.0),
        "full" => (None, 40.0, 4.0),
        _ => (Some("Qwen-Image-Edit-2511-Lightning-8steps-V1.0-bf16.safetensors"), 8.0, 1.0),
    };
    let mut loras = Vec::new();
    if let Some(f) = lightning {
        let lp = Path::new(&dir).join("lightning").join(f);
        if !lp.is_file() {
            return Err(format!("Lightning LoRA missing: {} (download the edit model with: python tools/download_edit_model.py)", lp.display()));
        }
        loras.push((lp.to_string_lossy().replace('\\', "/"), pnum(p, "lora_strength", 1.0)));
    }
    let (device, dev_label) = parse_device(&pick(p, "edit_device", "edit_device", "cuda:1"));
    let (encoder_device, enc_label) = parse_device(&pick(p, "edit_encoder_device", "edit_encoder_device", "cuda:0"));
    let opts = qwen::EditOpts { dir: dir.clone(), gguf: gguf.clone(), loras, device, encoder_device, encoder_q8: cfg_bool("edit_encoder_q8", true) };
    let key = qwen::opts_key(&opts);
    if slot.as_ref().map(|m| m.key != key).unwrap_or(true) {
        *slot = None;
        // an SD checkpoint on one of the same GPUs is dropped first; it reloads in seconds
        if let Some(m) = sd.as_ref() {
            let loc = format!("{:?}", m.device.location());
            if loc == format!("{:?}", opts.device.location()) || loc == format!("{:?}", opts.encoder_device.location()) {
                *sd = None;
                if let Ok(mut l) = loaded_info().lock() {
                    *l = LoadedInfo::default();
                }
            }
        }
        clear_edit_status();
        set("loading Qwen-Image-Edit");
        let m = qwen::EditModel::load(&opts, &|s| set(s))?;
        let quant = gguf.trim_end_matches(".gguf").rsplit('-').next().unwrap_or("").to_string();
        if let Ok(mut l) = edit_loaded().lock() {
            *l = format!("Qwen-Image-Edit-2511 {quant} · transformer {dev_label} · encoder {enc_label} · {mode}");
        }
        *slot = Some(m);
    }
    let m = slot.as_ref().unwrap();
    let src = p.get("init_images").and_then(|a| a.as_array()).and_then(|a| a.first()).and_then(|v| v.as_str()).ok_or("edit needs an image (init_images)")?;
    let img = decode_image(src)?;
    let steps = (pnum(p, "steps", def_steps) as usize).clamp(1, 100);
    let cfg = pnum(p, "cfg_scale", def_cfg).max(1.0);
    let mp = pnum(p, "megapixels", 1.0).clamp(0.25, 2.0);
    let n_iter = (pnum(p, "n_iter", 1.0) as usize).clamp(1, 16);
    let mut seed = pnum(p, "seed", -1.0) as i64;
    if seed < 0 {
        seed = (rand::random::<u32>() & 0x7fff_ffff) as i64;
    }
    let prompt = pstr(p, "prompt").to_string();
    let negative = pstr(p, "negative_prompt").to_string();
    let save = pbool(p, "save_images", true);
    let outdir = PathBuf::from(match pstr(p, "outdir") {
        "" => format!("{}/edit-images", cfg_str("outputs_root")),
        d => d.to_string(),
    });
    if save {
        std::fs::create_dir_all(&outdir).map_err(|e| format!("output folder {}: {e}", outdir.display()))?;
    }
    let mut index = if save { next_index(&outdir) } else { 0 };
    if let Ok(mut s) = job.st.lock() {
        s.state = "running".into();
        s.steps = steps;
        s.batches = n_iter;
        s.notes = m.notes.clone();
        s.model = "Qwen-Image-Edit-2511".into();
    }
    for it in 0..n_iter {
        if job.interrupt.load(Ordering::Relaxed) {
            break;
        }
        job.skip.store(false, Ordering::Relaxed);
        if let Ok(mut s) = job.st.lock() {
            s.batch = it;
            s.step = 0;
        }
        let this_seed = seed + it as i64;
        let ep = qwen::EditParams { prompt: prompt.clone(), negative: negative.clone(), steps, true_cfg: cfg, seed: this_seed as u64, area: mp * 1024.0 * 1024.0, lightning: lightning.is_some() };
        let mut on_step = |i: usize, _n: usize| -> bool {
            if let Ok(mut s) = job.st.lock() {
                s.step = i;
                s.message = "editing".into();
            }
            !(job.interrupt.load(Ordering::Relaxed) || job.skip.load(Ordering::Relaxed))
        };
        let (rgb, w, h) = m.edit(&img, &ep, &mut on_step, &|s| set(s))?;
        if let Ok(mut s) = job.st.lock() {
            s.state = "decoding".into();
        }
        let mut info = prompt.clone();
        if cfg > 1.0 && !negative.trim().is_empty() {
            info.push_str(&format!("\nNegative prompt: {negative}"));
        }
        info.push_str(&format!(
            "\nSteps: {steps}, Sampler: Flow Euler, CFG scale: {cfg}, Seed: {this_seed}, Size: {w}x{h}, Model: Qwen-Image-Edit-2511, Edit mode: {mode}, Source size: {}x{}, Version: LingDiffusion",
            img.width(),
            img.height()
        ));
        let png = png_bytes(&rgb, w, h, Some(&info))?;
        let path = if save {
            let f = outdir.join(format!("{index:05}-{this_seed}.png"));
            index += 1;
            std::fs::write(&f, &png).map_err(|e| format!("saving {}: {e}", f.display()))?;
            f.to_string_lossy().replace('\\', "/")
        } else {
            String::new()
        };
        if let Ok(mut s) = job.st.lock() {
            s.images.push(path);
            s.pngs.push(if save { Vec::new() } else { png });
            s.seeds.push(this_seed);
            s.infotexts.push(info);
            s.state = "running".into();
        }
    }
    Ok(())
}
