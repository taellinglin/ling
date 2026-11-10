//! A loaded SD 1.x checkpoint and txt2img.
use crate::act::Act;
use crate::clip::{self, Clip};
use crate::ops::{Arena, Eng, Loader};
use crate::rng::StdRng;
use crate::sample::{self, Host, Table};
use crate::st::{SafeTensors, Source};
use crate::tok::Tokenizer;
use crate::unet::UNet;
use crate::vae::Vae;
use crate::Error;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub const UNET: &str = "model.diffusion_model.";
pub const VAE: &str = "first_stage_model.";
pub const CLIP: &str = "cond_stage_model.transformer.";
pub const CLIP_ALT: &str = "cond_stage_model.model.transformer.";

pub struct Model {
    pub clip: Clip,
    pub unet: UNet,
    pub vae: Vae,
    pub tok: Tokenizer,
    pub eng: Eng,
    /// bytes of weights held
    pub weight_bytes: usize,
    _arena: Arena,
}

pub struct Params {
    pub prompt: String,
    pub negative: String,
    pub sampler: String,
    pub steps: usize,
    pub cfg: f32,
    pub width: usize,
    pub height: usize,
    pub seed: u64,
    pub clip_skip: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Stage {
    Text,
    Sampling,
    Decoding,
}

pub struct Progress<'a> {
    pub stage: Stage,
    /// steps done / of
    pub step: usize,
    pub steps: usize,
    /// a rough RGB of the current estimate (latent size), while sampling
    pub preview: Option<(&'a [u8], usize, usize)>,
}

pub struct Image {
    pub rgb: Vec<u8>,
    pub w: usize,
    pub h: usize,
    pub seed: u64,
    /// the WebUI's `parameters` text
    pub info: String,
}

/// What a checkpoint holds, without loading it: Ok(bytes of weights to load) or why it cannot be used.
pub fn check(st: &SafeTensors) -> Result<u64, Error> {
    let has = |p: &str| st.tensors.keys().any(|k| k.starts_with(p));
    if !has(UNET) {
        if st.tensors.keys().any(|k| k.contains("lora_down") || k.contains("lora_up")) {
            return Err("this file is a LoRA, not a checkpoint".to_string());
        }
        return Err("not a Stable Diffusion checkpoint (no model.diffusion_model.* weights)".to_string());
    }
    if has("conditioner.embedders.1.") {
        return Err("SDXL checkpoints are too big for the CPU engine on LingOS; use an SD 1.5 one (or the GPU server)".to_string());
    }
    if let Some(i) = st.get("model.diffusion_model.input_blocks.1.1.transformer_blocks.0.attn2.to_k.weight") {
        if i.shape.get(1) == Some(&1024) {
            return Err("SD 2.x checkpoints are not supported (SD 1.x are)".to_string());
        }
    }
    if !has(CLIP) && !has(CLIP_ALT) {
        return Err("the checkpoint has no text encoder".to_string());
    }
    if !has("first_stage_model.decoder.") {
        return Err("the checkpoint has no VAE".to_string());
    }
    let mut total = 0u64;
    for (k, v) in &st.tensors {
        if k.starts_with(UNET) || k.starts_with(CLIP) || k.starts_with(CLIP_ALT) || k.starts_with("first_stage_model.decoder.") || k.starts_with("first_stage_model.post_quant_conv.") {
            // stored as f16 whatever the file's type
            total += (v.bytes / v.dtype.size() as u64) * 2;
        }
    }
    Ok(total)
}

impl Model {
    /// Read a checkpoint. `progress(done, total)` with bytes read.
    pub fn load(src: &dyn Source, vocab: &[u8], merges: &[u8], progress: &mut dyn FnMut(u64, u64)) -> Result<Model, Error> {
        let tok = Tokenizer::new(vocab, merges)?;
        let st = SafeTensors::parse(src)?;
        check(&st)?;
        let mut file_total = 0u64;
        for (k, v) in &st.tensors {
            if k.starts_with(UNET) || k.starts_with(CLIP) || k.starts_with(CLIP_ALT) || k.starts_with("first_stage_model.decoder.") || k.starts_with("first_stage_model.post_quant_conv.") {
                file_total += v.bytes;
            }
        }
        let mut arena = Arena::new();
        let mut eng = Eng::new();
        let clip_prefix = if st.tensors.keys().any(|k| k.starts_with(CLIP)) { CLIP } else { CLIP_ALT };
        let mut done = 0u64;
        let (clip, unet, vae);
        {
            let mut prog = |d: u64| progress(d, file_total);
            let mut l = Loader { st: &st, src, arena: &mut arena, prefix: clip_prefix.to_string(), done, progress: &mut prog };
            clip = Clip::load(&mut l)?;
            l.prefix = UNET.to_string();
            unet = UNet::load(&mut l)?;
            l.prefix = VAE.to_string();
            vae = Vae::load(&mut l)?;
            done = l.done;
        }
        let _ = done;
        eng.trim();
        let weight_bytes = arena.bytes;
        Ok(Model { clip, unet, vae, tok, eng, weight_bytes, _arena: arena })
    }

    /// (cond, uncond) prompt contexts.
    pub fn encode(&mut self, prompt: &str, negative: &str, clip_skip: usize) -> (Act, Act) {
        clip::encode_pair(&self.clip, &self.tok, &mut self.eng, prompt, negative, clip_skip)
    }

    /// The guided denoiser output `x - eps * sigma` for a CHW latent (what the host engine's Denoiser::denoise returns).
    pub fn denoise(&mut self, x: &[f32], lh: usize, lw: usize, sigma: f64, cond: &Act, uncond: &Act, cfg: f32, table: &Table) -> Vec<f32> {
        let c_in = 1.0 / libm::sqrt(sigma * sigma + 1.0);
        let t = table.sigma_to_t(sigma);
        let xs: Vec<f32> = x.iter().map(|v| (*v as f64 * c_in) as f32).collect();
        let xin = Act::from_chw(4, lh, lw, &xs);
        let ec = self.unet.forward(&mut self.eng, &xin, t, cond).to_chw();
        let eps: Vec<f32> = if (cfg - 1.0).abs() > 1e-6 {
            if crate::kern::cancelled() {
                return ec;
            }
            let eu = self.unet.forward(&mut self.eng, &xin, t, uncond).to_chw();
            eu.iter().zip(ec.iter()).map(|(u, c)| u + (c - u) * cfg).collect()
        } else {
            ec
        };
        x.iter().zip(eps.iter()).map(|(xv, e)| (*xv as f64 - *e as f64 * sigma) as f32).collect()
    }

    pub fn txt2img(&mut self, p: &Params, cb: &mut dyn FnMut(&Progress) -> bool) -> Result<Image, Error> {
        if p.width % 64 != 0 || p.height % 64 != 0 || p.width < 64 || p.height < 64 {
            return Err("width and height must be multiples of 64".to_string());
        }
        crate::kern::set_cancel(false);
        let (sampler, schedule, sname) = sample::parse(&p.sampler);
        let steps = p.steps.clamp(1, 150);
        cb(&Progress { stage: Stage::Text, step: 0, steps, preview: None });
        let (cond, uncond) = self.encode(&p.prompt, &p.negative, p.clip_skip.max(1));
        let (lh, lw) = (p.height / 8, p.width / 8);
        let table = Table::sd();
        let mut rng = StdRng::seed_from_u64(p.seed);
        let mut noise = Vec::new();
        rng.randn(4 * lh * lw, &mut noise);
        let sig = table.sigmas(steps, schedule);
        let x: Vec<f32> = noise.iter().map(|v| (*v as f64 * sig[0]) as f32).collect();
        struct D<'a> {
            m: &'a mut Model,
            cond: &'a Act,
            uncond: &'a Act,
            cfg: f32,
            table: &'a Table,
            rng: StdRng,
            lh: usize,
            lw: usize,
            steps: usize,
            cb: &'a mut dyn FnMut(&Progress) -> bool,
            prev: Vec<u8>,
        }
        impl Host for D<'_> {
            fn denoise(&mut self, x: &[f32], sigma: f64) -> Vec<f32> {
                self.m.denoise(x, self.lh, self.lw, sigma, self.cond, self.uncond, self.cfg, self.table)
            }
            fn noise(&mut self, n: usize) -> Vec<f32> {
                let mut v = Vec::new();
                self.rng.randn(n, &mut v);
                v
            }
            fn step(&mut self, i: usize, den: &[f32]) -> bool {
                preview_rgb(den, self.lh, self.lw, &mut self.prev);
                let go = (self.cb)(&Progress { stage: Stage::Sampling, step: i + 1, steps: self.steps, preview: Some((&self.prev, self.lw, self.lh)) });
                go && !crate::kern::cancelled()
            }
        }
        let lat = {
            let mut d = D { m: &mut *self, cond: &cond, uncond: &uncond, cfg: p.cfg, table: &table, rng, lh, lw, steps, cb: &mut *cb, prev: Vec::new() };
            let r = sample::run(sampler, &sig, x, &mut d);
            match r {
                Some(l) => l,
                None => return Err("stopped".to_string()),
            }
        };
        drop((cond, uncond));
        cb(&Progress { stage: Stage::Decoding, step: steps, steps, preview: None });
        let z = Act::from_chw(4, lh, lw, &lat);
        let (rgb, w, h) = self.vae.decode(&mut self.eng, &z);
        self.eng.trim();
        let info = alloc::format!(
            "{}\nNegative prompt: {}\nSteps: {}, Sampler: {}, CFG scale: {}, Seed: {}, Size: {}x{}, Clip skip: {}, Engine: LingOS CPU",
            p.prompt, p.negative, steps, sname, p.cfg, p.seed, p.width, p.height, p.clip_skip.max(1)
        );
        Ok(Image { rgb, w, h, seed: p.seed, info })
    }
}

/// latent (CHW) -> rough RGB, the WebUI's cheap approximation (for live previews)
pub fn preview_rgb(lat: &[f32], lh: usize, lw: usize, out: &mut Vec<u8>) {
    const M: [[f32; 3]; 4] = [[0.298, 0.207, 0.208], [0.187, 0.286, 0.173], [-0.158, 0.189, 0.264], [-0.184, -0.271, -0.473]];
    let n = lh * lw;
    out.clear();
    out.resize(n * 3, 0);
    for p in 0..n {
        for ch in 0..3 {
            let mut s = 0.0;
            for k in 0..4 {
                s += lat[k * n + p] * M[k][ch];
            }
            out[p * 3 + ch] = (((s + 1.0) * 0.5).clamp(0.0, 1.0) * 255.0) as u8;
        }
    }
}
