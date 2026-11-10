//! Loading a single-file checkpoint into candle modules, and running one generation request.

use crate::convert::{self, GKey};
use crate::prompt::{self, Piece};
use crate::sampling::{self, Host, Table};
use crate::tokenizer::{ClipTokenizer, BOS, EOS};
use crate::unet::{UNet, UNetConfig};
use candle_core::{DType, Device, IndexOp, Module, Tensor, D};
use candle_nn::VarBuilder;
use candle_transformers::models::stable_diffusion::{clip, vae};
use std::collections::HashMap;
use std::path::Path;

pub type R<T> = std::result::Result<T, String>;

fn e<E: std::fmt::Display>(x: E) -> String {
    x.to_string()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Arch {
    Sd15,
    Sdxl,
}

impl Arch {
    pub fn label(self) -> &'static str {
        match self {
            Arch::Sd15 => "SD 1.5",
            Arch::Sdxl => "SDXL",
        }
    }
    fn vae_scale(self) -> f64 {
        match self {
            Arch::Sd15 => 0.18215,
            Arch::Sdxl => 0.13025,
        }
    }
    /// latent -> rough RGB, for live previews (the WebUI's "cheap approximation")
    fn preview_matrix(self) -> [[f32; 3]; 4] {
        match self {
            Arch::Sd15 => [[0.298, 0.207, 0.208], [0.187, 0.286, 0.173], [-0.158, 0.189, 0.264], [-0.184, -0.271, -0.473]],
            Arch::Sdxl => [[0.3448, 0.4168, 0.4395], [-0.1953, -0.0290, 0.0250], [0.1074, 0.0886, -0.0163], [-0.3730, -0.2499, -0.2088]],
        }
    }
}

pub struct LoadOpts {
    pub checkpoint: String,
    pub vae: String,
    pub loras: Vec<(String, f32)>, // resolved file path, weight
    pub device: Device,
    pub dtype: DType,
    pub sliced_attention: bool,
}

pub struct Model {
    pub arch: Arch,
    pub key: String,
    pub device: Device,
    pub dtype: DType,
    clip_l: clip::ClipTextTransformer,
    clip_l_ln: candle_nn::LayerNorm,
    clip_g: Option<(clip::ClipTextTransformer, Tensor)>,
    unet: UNet,
    vae: vae::AutoEncoderKL,
    pub lora_notes: Vec<String>,
}

pub fn model_key(o: &LoadOpts) -> String {
    let loras: Vec<String> = o.loras.iter().map(|(p, w)| format!("{p}@{w:.3}")).collect();
    format!("{}|{}|{}|{:?}|{:?}|{}", o.checkpoint, o.vae, loras.join(","), o.device.location(), o.dtype, o.sliced_attention)
}

/// Every tensor name in a checkpoint without loading data (safetensors header / pickle index).
pub fn tensor_names(path: &str) -> R<Vec<(String, Vec<usize>)>> {
    let p = Path::new(path);
    let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
    if ext == "safetensors" {
        let mut f = std::fs::File::open(p).map_err(e)?;
        use std::io::Read;
        let mut n = [0u8; 8];
        f.read_exact(&mut n).map_err(e)?;
        let n = u64::from_le_bytes(n) as usize;
        if n > 200_000_000 {
            return Err("not a safetensors file (header too large)".into());
        }
        let mut hdr = vec![0u8; n];
        f.read_exact(&mut hdr).map_err(e)?;
        let j: serde_json::Value = serde_json::from_slice(&hdr).map_err(e)?;
        let mut out = Vec::new();
        if let Some(o) = j.as_object() {
            for (k, v) in o {
                if k == "__metadata__" {
                    continue;
                }
                let shape = v.get("shape").and_then(|s| s.as_array()).map(|a| a.iter().filter_map(|x| x.as_u64()).map(|x| x as usize).collect()).unwrap_or_default();
                out.push((k.clone(), shape));
            }
        }
        Ok(out)
    } else {
        let infos = candle_core::pickle::read_pth_tensor_info(p, false, Some("state_dict"))
            .or_else(|_| candle_core::pickle::read_pth_tensor_info(p, false, None))
            .map_err(e)?;
        Ok(infos.into_iter().map(|i| (i.name, i.layout.shape().dims().to_vec())).collect())
    }
}

/// Calls `f(name, tensor)` for every tensor `want(name)` accepts, loaded onto `dev`.
fn each_tensor(path: &str, dev: &Device, want: &dyn Fn(&str) -> bool, f: &mut dyn FnMut(String, Tensor) -> R<()>) -> R<()> {
    let p = Path::new(path);
    if !p.exists() {
        return Err(format!("file not found: {path}"));
    }
    let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
    if ext == "safetensors" {
        let st = unsafe { candle_core::safetensors::MmapedSafetensors::new(p) }.map_err(e)?;
        let names: Vec<String> = st.tensors().into_iter().map(|(n, _)| n).collect();
        for n in names {
            if want(&n) {
                let t = st.load(&n, dev).map_err(|x| format!("{n}: {x}"))?;
                f(n, t)?;
            }
        }
    } else {
        let all = candle_core::pickle::read_all_with_key(p, Some("state_dict"))
            .or_else(|_| candle_core::pickle::read_all(p))
            .map_err(|x| format!("reading {path}: {x}"))?;
        for (n, t) in all {
            if want(&n) {
                let t = t.to_device(dev).map_err(e)?;
                f(n, t)?;
            }
        }
    }
    Ok(())
}

fn underscore(s: &str) -> String {
    s.strip_suffix(".weight").unwrap_or(s).replace('.', "_")
}

/// Merges kohya / PEFT LoRAs into the converted weight maps (W += w * alpha/rank * up @ down).
fn apply_loras(
    loras: &[(String, f32)],
    dev: &Device,
    unet: &mut HashMap<String, Tensor>,
    unet_alias: &HashMap<String, String>,
    cl: &mut HashMap<String, Tensor>,
    cg: &mut HashMap<String, Tensor>,
    sdxl: bool,
) -> R<Vec<String>> {
    let mut notes = Vec::new();
    // underscore spelling -> key, per target map
    let cl_alias: HashMap<String, String> = cl.keys().map(|k| (underscore(k), k.clone())).collect();
    let cg_alias: HashMap<String, String> = cg.keys().map(|k| (underscore(k), k.clone())).collect();
    for (path, weight) in loras {
        let mut t: HashMap<String, Tensor> = HashMap::new();
        each_tensor(path, dev, &|_| true, &mut |n, x| {
            t.insert(n, x.to_dtype(DType::F32).map_err(e)?);
            Ok(())
        })?;
        let mut applied = 0usize;
        let mut missed = 0usize;
        let downs: Vec<String> = t.keys().filter(|k| k.ends_with(".lora_down.weight") || k.ends_with(".lora_A.weight")).cloned().collect();
        for dk in downs {
            let (base, upk) = if let Some(b) = dk.strip_suffix(".lora_down.weight") {
                (b.to_string(), format!("{b}.lora_up.weight"))
            } else {
                let b = dk.strip_suffix(".lora_A.weight").unwrap();
                (b.to_string(), format!("{b}.lora_B.weight"))
            };
            let (Some(down), Some(up)) = (t.get(&dk), t.get(&upk)) else { continue };
            let rank = down.dim(0).map_err(e)? as f64;
            let alpha = t.get(&format!("{base}.alpha")).and_then(|a| a.flatten_all().ok()?.to_vec1::<f32>().ok()?.first().copied()).map(|a| a as f64).unwrap_or(rank);
            // which map + key
            let (map, key): (&mut HashMap<String, Tensor>, Option<String>) = if let Some(r) = base.strip_prefix("lora_unet_") {
                (&mut *unet, unet_alias.get(r).cloned())
            } else if let Some(r) = base.strip_prefix("unet.") {
                (&mut *unet, unet_alias.get(&r.replace('.', "_")).cloned())
            } else if let Some(r) = base.strip_prefix("lora_te2_") {
                (&mut *cg, cg_alias.get(r).cloned())
            } else if let Some(r) = base.strip_prefix("lora_te1_").or_else(|| base.strip_prefix("lora_te_")) {
                (&mut *cl, cl_alias.get(r).cloned())
            } else if let Some(r) = base.strip_prefix("text_encoder_2.") {
                (&mut *cg, cg_alias.get(&r.replace('.', "_")).cloned())
            } else if let Some(r) = base.strip_prefix("text_encoder.") {
                (&mut *cl, cl_alias.get(&r.replace('.', "_")).cloned())
            } else {
                missed += 1;
                continue;
            };
            let Some(key) = key else {
                missed += 1;
                continue;
            };
            let Some(w0) = map.get(&key) else {
                missed += 1;
                continue;
            };
            let shape = w0.dims().to_vec();
            let d2 = down.flatten_from(1).map_err(e)?;
            let u2 = up.flatten_from(1).map_err(e)?;
            let delta = u2.matmul(&d2).map_err(e)?;
            let delta = if delta.elem_count() == w0.elem_count() {
                delta.reshape(shape.as_slice()).map_err(e)?
            } else {
                missed += 1;
                continue;
            };
            let scale = *weight as f64 * alpha / rank;
            let nw = (w0.to_dtype(DType::F32).map_err(e)? + (delta * scale).map_err(e)?).map_err(e)?.to_dtype(w0.dtype()).map_err(e)?;
            map.insert(key, nw);
            applied += 1;
        }
        let name = Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or(path).to_string();
        if applied == 0 {
            notes.push(format!("LoRA {name}: no layers matched this {} checkpoint", if sdxl { "SDXL" } else { "SD 1.x" }));
        } else if missed > 0 {
            notes.push(format!("LoRA {name}: {applied} layers applied, {missed} skipped"));
        }
    }
    Ok(notes)
}

impl Model {
    pub fn load(o: &LoadOpts, status: &dyn Fn(&str)) -> R<Model> {
        let names = tensor_names(&o.checkpoint)?;
        let has = |p: &str| names.iter().any(|(n, _)| n.starts_with(p));
        if !has(convert::UNET) {
            if names.iter().any(|(n, _)| n.contains("lora_down") || n.contains("lora_up") || n.starts_with("lora_")) {
                return Err("this file is a LoRA, not a checkpoint: use it as <lora:name:1> in the prompt".into());
            }
            return Err("not a Stable Diffusion checkpoint (no model.diffusion_model.* weights)".into());
        }
        let arch = if has(convert::CLIP_G_XL) { Arch::Sdxl } else { Arch::Sd15 };
        if let Some((_, s)) = names.iter().find(|(n, _)| n == "model.diffusion_model.input_blocks.1.1.transformer_blocks.0.attn2.to_k.weight") {
            if arch == Arch::Sd15 && s.get(1) == Some(&1024) {
                return Err("SD 2.x checkpoints are not supported yet (SD 1.x and SDXL are)".into());
            }
        }
        let in_ch = names.iter().find(|(n, _)| n == "model.diffusion_model.input_blocks.0.0.weight").and_then(|(_, s)| s.get(1).copied()).unwrap_or(4);
        if in_ch != 4 {
            return Err(format!("inpainting / {in_ch}-channel checkpoints are not supported yet"));
        }
        let dev = &o.device;
        status(&format!("reading {} weights", arch.label()));
        let mut unet: HashMap<String, Tensor> = HashMap::new();
        let mut unet_alias: HashMap<String, String> = HashMap::new();
        let mut vaem: HashMap<String, Tensor> = HashMap::new();
        let mut cl: HashMap<String, Tensor> = HashMap::new();
        let mut cg: HashMap<String, Tensor> = HashMap::new();
        let mut proj: Option<Tensor> = None;
        let separate_vae = !o.vae.is_empty();
        let want = |n: &str| {
            n.starts_with(convert::UNET)
                || (!separate_vae && n.starts_with(convert::VAE))
                || n.starts_with(convert::CLIP_SD1)
                || n.starts_with(convert::CLIP_SD1_ALT)
                || n.starts_with(convert::CLIP_L_XL)
                || n.starts_with(convert::CLIP_G_XL)
        };
        each_tensor(&o.checkpoint, dev, &want, &mut |n, t| {
            if let Some(k) = n.strip_prefix(convert::UNET) {
                if let Some(d) = convert::unet(k, 2) {
                    unet_alias.insert(underscore(k), d.clone());
                    unet_alias.insert(underscore(&d), d.clone());
                    unet.insert(d, t);
                }
            } else if let Some(k) = n.strip_prefix(convert::VAE) {
                if let Some(d) = convert::vae(k) {
                    vaem.insert(d, t);
                }
            } else if let Some(k) = n.strip_prefix(convert::CLIP_SD1).or_else(|| n.strip_prefix(convert::CLIP_SD1_ALT)).or_else(|| n.strip_prefix(convert::CLIP_L_XL)) {
                if let Some(d) = convert::clip_l(k) {
                    cl.insert(d, t);
                }
            } else if let Some(k) = n.strip_prefix(convert::CLIP_G_XL) {
                match convert::clip_g(k) {
                    Some(GKey::Plain(d)) => {
                        cg.insert(d, t);
                    }
                    Some(GKey::Qkv(ns)) => {
                        let third = t.dim(0).map_err(e)? / 3;
                        for (i, d) in ns.into_iter().enumerate() {
                            cg.insert(d, t.narrow(0, i * third, third).map_err(e)?.contiguous().map_err(e)?);
                        }
                    }
                    Some(GKey::Projection) => proj = Some(t),
                    None => {}
                }
            }
            Ok(())
        })?;
        if separate_vae {
            status("reading VAE");
            each_tensor(&o.vae, dev, &|_| true, &mut |n, t| {
                let k = n.strip_prefix(convert::VAE).unwrap_or(&n);
                let d = if k.starts_with("encoder.down_blocks") || k.starts_with("decoder.up_blocks") || k.contains("mid_block") { Some(k.to_string()) } else { convert::vae(k) };
                if let Some(d) = d {
                    vaem.insert(d, t);
                }
                Ok(())
            })?;
        }
        if vaem.is_empty() {
            return Err("the checkpoint has no VAE weights: pick a VAE file".into());
        }
        let lora_notes = if o.loras.is_empty() {
            Vec::new()
        } else {
            status("merging LoRA");
            apply_loras(&o.loras, dev, &mut unet, &unet_alias, &mut cl, &mut cg, arch == Arch::Sdxl)?
        };

        status("building text encoder");
        let vb_cl = VarBuilder::from_tensors(cl, DType::F32, dev);
        let clip_l_cfg = clip::Config::v1_5();
        let clip_l = clip::ClipTextTransformer::new(vb_cl.clone(), &clip_l_cfg).map_err(|x| format!("CLIP: {x}"))?;
        let clip_l_ln = candle_nn::layer_norm(768, 1e-5, vb_cl.pp("text_model").pp("final_layer_norm")).map_err(e)?;
        let clip_g = if arch == Arch::Sdxl {
            let p = proj.ok_or("SDXL checkpoint without text_projection")?;
            let vb = VarBuilder::from_tensors(cg, o.dtype, dev);
            let m = clip::ClipTextTransformer::new(vb, &clip::Config::sdxl2()).map_err(|x| format!("OpenCLIP: {x}"))?;
            Some((m, p.to_dtype(DType::F32).map_err(e)?))
        } else {
            None
        };
        status("building UNet");
        let sliced = if o.sliced_attention { Some(0) } else { None };
        let ucfg = match arch {
            Arch::Sd15 => UNetConfig::sd15(sliced),
            Arch::Sdxl => UNetConfig::sdxl(sliced),
        };
        let unet = UNet::new(VarBuilder::from_tensors(unet, o.dtype, dev), 4, &ucfg).map_err(|x| format!("UNet: {x}"))?;
        status("building VAE");
        let vcfg = vae::AutoEncoderKLConfig {
            block_out_channels: vec![128, 256, 512, 512],
            layers_per_block: 2,
            latent_channels: 4,
            norm_num_groups: 32,
            use_quant_conv: true,
            use_post_quant_conv: true,
        };
        // the VAE always runs in f32: SDXL's VAE overflows f16 (the WebUI's --no-half-vae)
        let vae = vae::AutoEncoderKL::new(VarBuilder::from_tensors(vaem, DType::F32, dev), 3, 3, vcfg).map_err(|x| format!("VAE: {x}"))?;
        Ok(Model { arch, key: model_key(o), device: dev.clone(), dtype: o.dtype, clip_l, clip_l_ln, clip_g, unet, vae, lora_notes })
    }
}

// ── prompt encoding ──────────────────────────────────────────────────────

type Chunk = (Vec<u32>, Vec<f32>);

fn chunk_prompt(tok: &ClipTokenizer, text: &str) -> Vec<Chunk> {
    let comma = tok.encode(",").first().copied().unwrap_or(267);
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut ids: Vec<u32> = Vec::new();
    let mut ws: Vec<f32> = Vec::new();
    let mut last_comma: Option<usize> = None;
    for piece in prompt::parse_attention(text) {
        match piece {
            Piece::Break => {
                chunks.push((std::mem::take(&mut ids), std::mem::take(&mut ws)));
                last_comma = None;
            }
            Piece::Text(t, w) => {
                for id in tok.encode(&t) {
                    if ids.len() == 75 {
                        // like the WebUI: if a comma is close to the end, carry what follows it over
                        match last_comma {
                            Some(c) if 75 - c <= 20 => {
                                let ri = ids.split_off(c + 1);
                                let rw = ws.split_off(c + 1);
                                chunks.push((std::mem::replace(&mut ids, ri), std::mem::replace(&mut ws, rw)));
                            }
                            _ => chunks.push((std::mem::take(&mut ids), std::mem::take(&mut ws))),
                        }
                        last_comma = None;
                    }
                    ids.push(id);
                    ws.push(w);
                    if id == comma {
                        last_comma = Some(ids.len() - 1);
                    }
                }
            }
        }
    }
    if !ids.is_empty() || chunks.is_empty() {
        chunks.push((ids, ws));
    }
    chunks
}

fn padded(c: &Chunk, pad: u32) -> (Vec<u32>, Vec<f32>, usize) {
    let mut ids = vec![BOS];
    ids.extend(&c.0);
    ids.push(EOS);
    let eos_at = ids.len() - 1;
    let mut ws = vec![1.0];
    ws.extend(&c.1);
    ws.push(1.0);
    while ids.len() < 77 {
        ids.push(pad);
        ws.push(1.0);
    }
    (ids, ws, eos_at)
}

/// z * w, then rescaled so the mean is what it was (the WebUI's emphasis)
fn emphasize(z: &Tensor, ws: &[f32]) -> candle_core::Result<Tensor> {
    if ws.iter().all(|w| (*w - 1.0).abs() < 1e-6) {
        return Ok(z.clone());
    }
    let before = z.mean_all()?.to_scalar::<f32>()?;
    let w = Tensor::from_slice(ws, (1, ws.len(), 1), z.device())?;
    let z2 = z.broadcast_mul(&w)?;
    let after = z2.mean_all()?.to_scalar::<f32>()?;
    if after.abs() < 1e-12 {
        return Ok(z2);
    }
    z2 * (before / after) as f64
}

pub struct Cond {
    pub ctx: Tensor,            // [1, 77*k, D] f32
    pub pooled: Option<Tensor>, // [1, 1280] f32 (SDXL)
}

impl Model {
    fn encode_chunks(&self, chunks: &[Chunk], clip_skip: usize) -> R<Cond> {
        let dev = &self.device;
        let mut parts = Vec::new();
        let mut pooled = None;
        for (ci, c) in chunks.iter().enumerate() {
            let (ids_l, ws, _) = padded(c, EOS);
            let t = Tensor::new(ids_l.as_slice(), dev).map_err(e)?.unsqueeze(0).map_err(e)?;
            let z = match &self.clip_g {
                None => {
                    let skip = clip_skip.clamp(1, 11);
                    let (fin, inter) = self.clip_l.forward_until_encoder_layer(&t, usize::MAX, -(skip as isize)).map_err(e)?;
                    if skip <= 1 { fin } else { self.clip_l_ln.forward(&inter).map_err(e)? }
                }
                Some((g, proj)) => {
                    let (_, zl) = self.clip_l.forward_until_encoder_layer(&t, usize::MAX, -2).map_err(e)?;
                    let (ids_g, _, eos_at) = padded(c, 0);
                    let tg = Tensor::new(ids_g.as_slice(), dev).map_err(e)?.unsqueeze(0).map_err(e)?;
                    let (fin_g, zg) = g.forward_until_encoder_layer(&tg, usize::MAX, -2).map_err(e)?;
                    if ci == 0 {
                        let p = fin_g.i((0..1, eos_at)).map_err(e)?.to_dtype(DType::F32).map_err(e)?;
                        pooled = Some(p.matmul(proj).map_err(e)?);
                    }
                    Tensor::cat(&[&zl.to_dtype(DType::F32).map_err(e)?, &zg.to_dtype(DType::F32).map_err(e)?], D::Minus1).map_err(e)?
                }
            };
            let z = z.to_dtype(DType::F32).map_err(e)?;
            parts.push(emphasize(&z, &ws).map_err(e)?);
        }
        let ctx = Tensor::cat(&parts.iter().collect::<Vec<_>>(), 1).map_err(e)?;
        Ok(Cond { ctx, pooled })
    }

    /// (cond, uncond) with matching lengths
    pub fn encode_pair(&self, tok: &ClipTokenizer, prompt: &str, negative: &str, clip_skip: usize) -> R<(Cond, Cond)> {
        let mut pc = chunk_prompt(tok, prompt);
        let mut nc = chunk_prompt(tok, negative);
        let n = pc.len().max(nc.len());
        while pc.len() < n {
            pc.push((Vec::new(), Vec::new()));
        }
        while nc.len() < n {
            nc.push((Vec::new(), Vec::new()));
        }
        let cond = self.encode_chunks(&pc, clip_skip)?;
        let unc = if self.arch == Arch::Sdxl && negative.trim().is_empty() {
            // the WebUI feeds SDXL zeros, not an encoded "", for an empty negative prompt
            Cond { ctx: cond.ctx.zeros_like().map_err(e)?, pooled: cond.pooled.as_ref().map(|p| p.zeros_like()).transpose().map_err(e)? }
        } else {
            self.encode_chunks(&nc, clip_skip)?
        };
        Ok((cond, unc))
    }

    // candle's CUDA conv2d goes through im2col, whose scratch buffer is H*W*Cin*9 floats: a
    // 1024x1024 VAE pass wants ~10 GB of it. Big images are therefore encoded / decoded in
    // overlapping tiles (like the WebUI's Tiled VAE) and feathered together on the CPU.
    const TILE: usize = 64; // latent pixels (512 image pixels)
    const OVERLAP: usize = 16;

    fn encode_direct(&self, x: &Tensor) -> R<Tensor> {
        self.vae.encode(x).map_err(e)?.sample().map_err(e)
    }

    pub fn encode_image(&self, rgb: &[u8], w: usize, h: usize) -> R<Tensor> {
        let x = Tensor::from_slice(rgb, (h, w, 3), &self.device).map_err(e)?;
        let x = x.permute((2, 0, 1)).map_err(e)?.to_dtype(DType::F32).map_err(e)?;
        let x = ((x / 127.5).map_err(e)? - 1.0).map_err(e)?.unsqueeze(0).map_err(e)?;
        let (lh, lw) = (h / 8, w / 8);
        let lat = if lh * lw <= 80 * 80 {
            self.encode_direct(&x)?
        } else {
            let out = tiled(lh, lw, 4, 1, |y, x0, th, tw| {
                let tile = x.narrow(2, y * 8, th * 8).map_err(e)?.narrow(3, x0 * 8, tw * 8).map_err(e)?;
                self.encode_direct(&tile)
            })?;
            Tensor::from_vec(out, (1, 4, lh, lw), &self.device).map_err(e)?
        };
        (lat * self.arch.vae_scale()).map_err(e)
    }

    /// latent [1,4,h,w] -> [1,3,H,W] in 0..1
    fn decode_direct(&self, lat: &Tensor) -> R<Tensor> {
        let x = self.vae.decode(&(lat / self.arch.vae_scale()).map_err(e)?).map_err(e)?;
        ((x / 2.0).map_err(e)? + 0.5).map_err(e)?.clamp(0f32, 1f32).map_err(e)
    }

    /// latent [1,4,h,w] -> RGB bytes, H x W x 3
    pub fn decode(&self, lat: &Tensor) -> R<Vec<u8>> {
        let (_, _, lh, lw) = lat.dims4().map_err(e)?;
        let chw: Vec<f32> = if lh * lw <= 80 * 80 {
            self.decode_direct(lat)?.flatten_all().map_err(e)?.to_vec1::<f32>().map_err(e)?
        } else {
            tiled(lh, lw, 3, 8, |y, x, th, tw| self.decode_direct(&lat.narrow(2, y, th).map_err(e)?.narrow(3, x, tw).map_err(e)?))?
        };
        let (hh, ww) = (lh * 8, lw * 8);
        let plane = hh * ww;
        let mut out = vec![0u8; plane * 3];
        for p in 0..plane {
            for c in 0..3 {
                out[p * 3 + c] = (chw[c * plane + p] * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
        Ok(out)
    }

    pub fn preview(&self, lat: &Tensor) -> R<(Vec<u8>, usize, usize)> {
        let l = lat.i(0).map_err(e)?.to_dtype(DType::F32).map_err(e)?.to_device(&Device::Cpu).map_err(e)?;
        let (c, h, w) = l.dims3().map_err(e)?;
        let v = l.flatten_all().map_err(e)?.to_vec1::<f32>().map_err(e)?;
        let m = self.arch.preview_matrix();
        let mut out = vec![0u8; h * w * 3];
        for p in 0..h * w {
            for ch in 0..3 {
                let mut s = 0.0;
                for k in 0..c.min(4) {
                    s += v[k * h * w + p] * m[k][ch];
                }
                out[p * 3 + ch] = (((s + 1.0) * 0.5).clamp(0.0, 1.0) * 255.0) as u8;
            }
        }
        Ok((out, w, h))
    }
}

/// Runs `f(y, x, th, tw)` over overlapping tiles of an `lh` x `lw` latent grid. Each call returns
/// a [1, ch, th*scale, tw*scale] tensor; the results are feathered together across the overlaps
/// and returned as one CHW f32 buffer of (lh*scale) x (lw*scale).
fn tiled(lh: usize, lw: usize, ch: usize, scale: usize, mut f: impl FnMut(usize, usize, usize, usize) -> R<Tensor>) -> R<Vec<f32>> {
    let starts = |n: usize| -> Vec<usize> {
        let t = Model::TILE.min(n);
        let stride = t - Model::OVERLAP.min(t / 2);
        let mut v = Vec::new();
        let mut s = 0;
        loop {
            let s2 = s.min(n - t);
            if v.last() != Some(&s2) {
                v.push(s2);
            }
            if s2 + t >= n {
                break;
            }
            s += stride;
        }
        v
    };
    let (th, tw) = (Model::TILE.min(lh), Model::TILE.min(lw));
    let (oh, ow) = (lh * scale, lw * scale);
    let feather = Model::OVERLAP * scale;
    let mut acc = vec![0f32; ch * oh * ow];
    let mut wsum = vec![0f32; oh * ow];
    // weight ramps up across an interior edge, flat at the image border
    let ramp = |p: usize, len: usize, at_start: bool, at_end: bool| -> f32 {
        let a = if at_start { 1.0 } else { ((p as f32 + 0.5) / feather as f32).min(1.0) };
        let b = if at_end { 1.0 } else { (((len - p) as f32 - 0.5) / feather as f32).min(1.0) };
        a.min(b).max(1e-3)
    };
    for &y in &starts(lh) {
        for &x in &starts(lw) {
            let t = f(y, x, th, tw)?.to_device(&Device::Cpu).map_err(e)?.flatten_all().map_err(e)?.to_vec1::<f32>().map_err(e)?;
            let (ph, pw) = (th * scale, tw * scale);
            let (y0, x0) = (y * scale, x * scale);
            for py in 0..ph {
                let wy = ramp(py, ph, y == 0, y + th >= lh);
                for px in 0..pw {
                    let w = wy * ramp(px, pw, x == 0, x + tw >= lw);
                    let o = (y0 + py) * ow + x0 + px;
                    wsum[o] += w;
                    for c in 0..ch {
                        acc[c * oh * ow + o] += w * t[c * ph * pw + py * pw + px];
                    }
                }
            }
        }
    }
    for c in 0..ch {
        for o in 0..oh * ow {
            acc[c * oh * ow + o] /= wsum[o];
        }
    }
    Ok(acc)
}

// ── sampling host: classifier-free guidance around the UNet ──────────────

pub struct Denoiser<'a> {
    pub model: &'a Model,
    pub table: &'a Table,
    pub ctx2: Tensor,                       // [2B or B, T, D] in model dtype
    pub added2: Option<(Tensor, Tensor)>,   // (pooled, size ids)
    pub cfg: f64,
    pub batch: usize,
    pub rngs: Vec<rand::rngs::StdRng>,
    pub on_step: &'a mut dyn FnMut(usize, &Tensor) -> bool,
}

pub fn randn(rng: &mut rand::rngs::StdRng, n: usize) -> Vec<f32> {
    use rand::Rng;
    let mut v = Vec::with_capacity(n + 1);
    while v.len() < n {
        let u1: f64 = rng.gen::<f64>().max(1e-12);
        let u2: f64 = rng.gen::<f64>();
        let r = (-2.0 * u1.ln()).sqrt();
        v.push((r * (2.0 * std::f64::consts::PI * u2).cos()) as f32);
        v.push((r * (2.0 * std::f64::consts::PI * u2).sin()) as f32);
    }
    v.truncate(n);
    v
}

impl Host for Denoiser<'_> {
    fn denoise(&mut self, x: &Tensor, sigma: f64) -> candle_core::Result<Tensor> {
        let c_in = 1.0 / (sigma * sigma + 1.0).sqrt();
        let t = self.table.sigma_to_t(sigma);
        let xin = (x * c_in)?.to_dtype(self.model.dtype)?;
        let guided = self.cfg != 1.0;
        let xin = if guided { Tensor::cat(&[&xin, &xin], 0)? } else { xin };
        let added = self.added2.as_ref().map(|(a, b)| (a, b));
        let eps = self.model.unet.forward(&xin, t, &self.ctx2, added)?.to_dtype(DType::F32)?;
        let eps = if guided {
            let u = eps.narrow(0, 0, self.batch)?;
            let c = eps.narrow(0, self.batch, self.batch)?;
            (&u + ((c - &u)? * self.cfg)?)?
        } else {
            eps
        };
        x - (eps * sigma)?
    }
    fn noise_like(&mut self, x: &Tensor) -> candle_core::Result<Tensor> {
        let dims = x.dims().to_vec();
        let per = x.elem_count() / self.batch;
        let mut v = Vec::with_capacity(x.elem_count());
        for r in self.rngs.iter_mut() {
            v.extend(randn(r, per));
        }
        Tensor::from_vec(v, dims, x.device())
    }
    fn step(&mut self, i: usize, den: &Tensor) -> bool {
        (self.on_step)(i, den)
    }
}

pub fn run_sampler(sampler: sampling::Sampler, sigmas: &[f64], x: Tensor, d: &mut Denoiser) -> R<Tensor> {
    sampling::run(sampler, sigmas, x, d).map_err(e)
}
