//! Qwen-Image-Edit-2511 inside Ling: instruction-based image editing ("add a red bow in her hair",
//! "make the eyes golden", "more detailed hair"). Port of diffusers' QwenImageEditPlusPipeline.
//!
//!   vl     Qwen2.5-VL-7B encodes the instruction together with the picture (at ~384x384)
//!   vae    encodes the picture (at ~1 MP) into 16-channel latents: the reference tokens
//!   mmdit  the 20B transformer (GGUF, quantized) predicts a flow velocity for the noisy tokens,
//!          attending to the text and the reference tokens; Euler steps on a shifted schedule
//!   vae    decodes the result
//!
//! The transformer and the encoder can live on different GPUs (`device`, `encoder_device`).

pub mod mmdit;
pub mod ops;
pub mod vae;
pub mod vl;

use candle_core::{DType, Device, Tensor};
use std::path::Path;

pub type R<T> = std::result::Result<T, String>;
fn e<E: std::fmt::Display>(x: E) -> String {
    x.to_string()
}

pub struct EditOpts {
    pub dir: String,      // models/qwen-image-edit-2511
    pub gguf: String,     // the transformer file (in dir/transformer unless absolute)
    pub loras: Vec<(String, f64)>,
    pub device: Device,
    pub encoder_device: Device,
    pub encoder_q8: bool,
}

pub fn opts_key(o: &EditOpts) -> String {
    let l: Vec<String> = o.loras.iter().map(|(p, s)| format!("{p}@{s:.3}")).collect();
    format!("{}|{}|{}|{:?}|{:?}|{}", o.dir, o.gguf, l.join(","), o.device.location(), o.encoder_device.location(), o.encoder_q8)
}

pub struct EditModel {
    pub key: String,
    vl: vl::Vl,
    vae: vae::Vae,
    dit: mmdit::Mmdit,
    pub notes: Vec<String>,
}

/// width, height for an area with the image's aspect ratio, rounded to multiples of 32
pub fn dims_for_area(area: f64, ratio: f64) -> (usize, usize) {
    let w = (area * ratio).sqrt();
    let h = w / ratio;
    (((w / 32.0).round() * 32.0).max(32.0) as usize, ((h / 32.0).round() * 32.0).max(32.0) as usize)
}

impl EditModel {
    pub fn load(o: &EditOpts, status: &dyn Fn(&str)) -> R<Self> {
        let dir = Path::new(&o.dir);
        let gguf = if Path::new(&o.gguf).is_file() { o.gguf.clone() } else { dir.join("transformer").join(&o.gguf).to_string_lossy().to_string() };
        if !Path::new(&gguf).is_file() {
            return Err(format!("edit model not found: {gguf} (download it with: python tools/download_edit_model.py)"));
        }
        let mut loras = Vec::new();
        let mut notes = Vec::new();
        for (p, s) in &o.loras {
            status(&format!("reading LoRA {}", Path::new(p).file_name().and_then(|x| x.to_str()).unwrap_or(p)));
            loras.push((mmdit::read_lora(p).map_err(|x| format!("LoRA {p}: {x}"))?, *s));
        }
        status("loading the edit transformer");
        let dit = mmdit::Mmdit::load(&gguf, &o.device, &loras, status).map_err(|x| format!("transformer: {x}"))?;
        if !o.loras.is_empty() && dit.lora_layers == 0 {
            notes.push("the LoRA matched no layers of the edit transformer".into());
        }
        drop(loras);
        status("loading the VAE");
        let vae = vae::Vae::load(&dir.join("vae").join("diffusion_pytorch_model.safetensors").to_string_lossy(), &o.encoder_device).map_err(|x| format!("VAE: {x}"))?;
        let tok = dir.join("processor").join("tokenizer.json");
        let vl = vl::Vl::load(&dir.join("text_encoder").to_string_lossy(), &tok.to_string_lossy(), &o.encoder_device, o.encoder_q8, status).map_err(|x| format!("text encoder: {x}"))?;
        Ok(Self { key: opts_key(o), vl, vae, dit, notes })
    }
    /// true when the transformer or the encoder lives on `d`'s GPU
    pub fn uses(&self, d: &Device) -> bool {
        let l = format!("{:?}", d.location());
        format!("{:?}", self.dit.device.location()) == l || format!("{:?}", self.vae.device.location()) == l
    }
}

pub struct EditParams {
    pub prompt: String,
    pub negative: String,
    pub steps: usize,
    pub true_cfg: f64,
    pub seed: u64,
    /// output area in pixels (1024*1024 like the reference)
    pub area: f64,
    /// Lightning LoRA in use: constant shift ln(3), no terminal stretch
    pub lightning: bool,
}

/// image -> RGB bytes at (w, h)
fn resize(img: &image::RgbImage, w: usize, h: usize, filter: image::imageops::FilterType) -> Vec<u8> {
    image::imageops::resize(img, w as u32, h as u32, filter).into_raw()
}

fn pack(lat: &Tensor) -> candle_core::Result<Tensor> {
    // [1,16,h,w] -> [1, (h/2)(w/2), 64], channel order (c, py, px)
    let (_, c, h, w) = lat.dims4()?;
    lat.reshape((1, c, h / 2, 2, w / 2, 2))?.permute((0, 2, 4, 1, 3, 5))?.reshape((1, (h / 2) * (w / 2), c * 4))
}

fn unpack(x: &Tensor, h: usize, w: usize) -> candle_core::Result<Tensor> {
    // inverse of pack; h, w in latent pixels
    x.reshape((1, h / 2, w / 2, 16, 2, 2))?.permute((0, 3, 1, 4, 2, 5))?.reshape((1, 16, h, w))
}

/// The flow-matching schedule: N+1 sigmas from ~1 to 0.
pub fn sigmas(steps: usize, image_seq_len: usize, lightning: bool) -> Vec<f64> {
    let n = steps.max(1);
    let mu = if lightning {
        3f64.ln()
    } else {
        // calculate_shift with the 2511 scheduler config (256..8192 tokens, shift 0.5..0.9)
        let m = (0.9 - 0.5) / (8192.0 - 256.0);
        image_seq_len as f64 * m + (0.5 - m * 256.0)
    };
    let mut s: Vec<f64> = (0..n).map(|i| 1.0 - i as f64 * (1.0 - 1.0 / n as f64) / ((n - 1).max(1)) as f64).collect();
    if n == 1 {
        s = vec![1.0];
    }
    for v in s.iter_mut() {
        *v = mu.exp() / (mu.exp() + (1.0 / *v - 1.0));
    }
    if !lightning {
        // stretch so the last sigma lands on shift_terminal = 0.02
        let last = 1.0 - s[n - 1];
        let k = last / (1.0 - 0.02);
        for v in s.iter_mut() {
            *v = 1.0 - (1.0 - *v) / k;
        }
    }
    s.push(0.0);
    s
}

impl EditModel {
    /// Edits `img`; `step` is called after each step with (i, total) and returns false to stop.
    pub fn edit(&self, img: &image::RgbImage, p: &EditParams, step: &mut dyn FnMut(usize, usize) -> bool, status: &dyn Fn(&str)) -> R<(Vec<u8>, usize, usize)> {
        let (iw, ih) = img.dimensions();
        let ratio = iw as f64 / ih as f64;
        // the instruction sees the picture at ~384x384, then snapped to the ViT's 28 px grid
        let (cw, ch) = dims_for_area(384.0 * 384.0, ratio);
        let (sw, sh) = vl::smart_size(cw, ch);
        let small = image::RgbImage::from_raw(cw as u32, ch as u32, resize(img, cw, ch, image::imageops::FilterType::Lanczos3)).ok_or("resize")?;
        let cond_rgb = resize(&small, sw, sh, image::imageops::FilterType::CatmullRom);
        // the reference and the output at ~area
        let (w, h) = dims_for_area(p.area, ratio);
        let (w, h) = (w / 16 * 16, h / 16 * 16);
        let ref_rgb = resize(img, w, h, image::imageops::FilterType::Lanczos3);

        status("reading the instruction");
        let ctx = self.vl.encode(&p.prompt, &cond_rgb, sw, sh).map_err(e)?;
        let neg = if p.true_cfg > 1.0 { Some(self.vl.encode(if p.negative.trim().is_empty() { " " } else { &p.negative }, &cond_rgb, sw, sh).map_err(e)?) } else { None };

        status("encoding the picture");
        let vdev = &self.vae.device;
        let x = Tensor::from_slice(&ref_rgb, (h, w, 3), vdev).map_err(e)?.permute((2, 0, 1)).map_err(e)?.to_dtype(DType::F32).map_err(e)?;
        let x = ((x / 127.5).map_err(e)? - 1.0).map_err(e)?.unsqueeze(0).map_err(e)?;
        let ref_lat = self.vae.encode(&x).map_err(e)?;
        let dev = &self.dit.device;
        let ref_tok = pack(&ref_lat.to_device(dev).map_err(e)?).map_err(e)?;
        let (lh, lw) = (h / 8, w / 8);
        let n_noisy = (lh / 2) * (lw / 2);

        let mut rng = <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(p.seed);
        let noise = crate::pipeline::randn(&mut rng, n_noisy * 64);
        let mut lat = Tensor::from_vec(noise, (1, n_noisy, 64), dev).map_err(e)?;
        let grids = [(lh / 2, lw / 2), (lh / 2, lw / 2)];
        let ctx = ctx.unsqueeze(0).map_err(e)?.to_device(dev).map_err(e)?;
        let rope = mmdit::Rope::new(&grids, ctx.dim(1).map_err(e)?, dev).map_err(e)?;
        let neg = match neg {
            Some(n) => {
                let n = n.unsqueeze(0).map_err(e)?.to_device(dev).map_err(e)?;
                let r = mmdit::Rope::new(&grids, n.dim(1).map_err(e)?, dev).map_err(e)?;
                Some((n, r))
            }
            None => None,
        };
        let sig = sigmas(p.steps, n_noisy, p.lightning);
        status("editing");
        for i in 0..p.steps {
            let (s, sn) = (sig[i], sig[i + 1]);
            let inp = Tensor::cat(&[&lat, &ref_tok], 1).map_err(e)?;
            let mut v = self.dit.forward(&inp, &ctx, s, n_noisy, &rope).map_err(e)?;
            if let Some((nctx, nrope)) = &neg {
                let vn = self.dit.forward(&inp, nctx, s, n_noisy, nrope).map_err(e)?;
                let comb = (&vn + ((&v - &vn).map_err(e)? * p.true_cfg).map_err(e)?).map_err(e)?;
                // keep the guided velocity at the conditional one's per-token norm
                let cn = v.sqr().map_err(e)?.sum_keepdim(2).map_err(e)?.sqrt().map_err(e)?;
                let gn = comb.sqr().map_err(e)?.sum_keepdim(2).map_err(e)?.sqrt().map_err(e)?.clamp(1e-12f32, f32::MAX).map_err(e)?;
                v = comb.broadcast_mul(&cn.broadcast_div(&gn).map_err(e)?).map_err(e)?;
            }
            lat = (lat + (v * (sn - s)).map_err(e)?).map_err(e)?;
            if !step(i + 1, p.steps) {
                break;
            }
        }
        status("decoding");
        let z = unpack(&lat, lh, lw).map_err(e)?.to_device(vdev).map_err(e)?;
        let img = self.vae.decode(&z).map_err(e)?; // [1,3,H,W]
        let img = ((img + 1.0).map_err(e)? * 127.5).map_err(e)?.round().map_err(e)?.clamp(0f32, 255f32).map_err(e)?;
        let img = img.squeeze(0).map_err(e)?.permute((1, 2, 0)).map_err(e)?.to_dtype(DType::U8).map_err(e)?.flatten_all().map_err(e)?;
        Ok((img.to_vec1::<u8>().map_err(e)?, w, h))
    }
}
