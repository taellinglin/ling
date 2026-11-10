//! The Qwen-Image MMDiT (QwenImageTransformer2DModel): 60 dual-stream blocks, 24 heads x 128,
//! joint text+image attention with 3-axis RoPE, AdaLN modulation. Loaded from a GGUF (quantized
//! weights stay quantized on the GPU), with LoRAs (kohya `lora_down/lora_up/alpha`) attached as
//! low-rank side paths. `zero_cond_t` (Qwen-Image-Edit-2511): the reference-image tokens are
//! modulated with the timestep-0 embedding while the noisy tokens get the real timestep.

use super::ops::{attention, gate_segments, layer_norm, modulate_segments, rms_norm, rope_pairs, QLinear};
use candle_core::quantized::{gguf_file, GgmlDType};
use candle_core::{DType, Device, Result, Tensor};
use std::collections::HashMap;

const DIM: usize = 3072;
const HEADS: usize = 24;
const HD: usize = 128;
const CDT: DType = DType::BF16;

struct Block {
    img_mod: QLinear,
    txt_mod: QLinear,
    to_q: QLinear,
    to_k: QLinear,
    to_v: QLinear,
    to_out: QLinear,
    add_q: QLinear,
    add_k: QLinear,
    add_v: QLinear,
    to_add_out: QLinear,
    nq: Tensor,
    nk: Tensor,
    naq: Tensor,
    nak: Tensor,
    img_mlp0: QLinear,
    img_mlp2: QLinear,
    txt_mlp0: QLinear,
    txt_mlp2: QLinear,
}

pub struct Mmdit {
    img_in: QLinear,
    txt_norm: Tensor,
    txt_in: QLinear,
    t1: QLinear,
    t2: QLinear,
    blocks: Vec<Block>,
    norm_out: QLinear,
    proj_out: QLinear,
    pub zero_cond_t: bool,
    pub device: Device,
    pub lora_layers: usize,
}

/// Rotary angles for the joint sequence.
pub struct Rope {
    pub img_cos: Tensor,
    pub img_sin: Tensor,
    pub txt_cos: Tensor,
    pub txt_sin: Tensor,
}

fn axis_freqs(dim: usize) -> Vec<f64> {
    (0..dim / 2).map(|i| 1.0 / 10000f64.powf((2 * i) as f64 / dim as f64)).collect()
}

impl Rope {
    /// `images`: (grid_h, grid_w) per image, in sequence order (frame index = position in list);
    /// `txt_len`: text tokens. scale_rope: rows/cols centred on zero.
    pub fn new(images: &[(usize, usize)], txt_len: usize, dev: &Device) -> Result<Self> {
        let (ff, fh, fw) = (axis_freqs(16), axis_freqs(56), axis_freqs(56));
        let mut ic = Vec::new();
        let mut is = Vec::new();
        let mut max_idx = 0usize;
        for (frame, &(h, w)) in images.iter().enumerate() {
            max_idx = max_idx.max(h / 2).max(w / 2);
            for y in 0..h {
                let py = y as f64 - (h - h / 2) as f64;
                for x in 0..w {
                    let px = x as f64 - (w - w / 2) as f64;
                    for f in &ff {
                        let a = frame as f64 * f;
                        ic.push(a.cos() as f32);
                        is.push(a.sin() as f32);
                    }
                    for f in &fh {
                        let a = py * f;
                        ic.push(a.cos() as f32);
                        is.push(a.sin() as f32);
                    }
                    for f in &fw {
                        let a = px * f;
                        ic.push(a.cos() as f32);
                        is.push(a.sin() as f32);
                    }
                }
            }
        }
        let n_img = ic.len() / 64;
        let mut tc = Vec::new();
        let mut ts = Vec::new();
        for j in 0..txt_len {
            let p = (max_idx + j) as f64;
            for f in ff.iter().chain(fh.iter()).chain(fw.iter()) {
                tc.push((p * f).cos() as f32);
                ts.push((p * f).sin() as f32);
            }
        }
        Ok(Self {
            img_cos: Tensor::from_vec(ic, (n_img, 64), dev)?,
            img_sin: Tensor::from_vec(is, (n_img, 64), dev)?,
            txt_cos: Tensor::from_vec(tc, (txt_len, 64), dev)?,
            txt_sin: Tensor::from_vec(ts, (txt_len, 64), dev)?,
        })
    }
}

fn timestep_embedding(v: f64) -> Vec<f32> {
    // diffusers Timesteps(256, flip_sin_to_cos=True, downscale_freq_shift=0, scale=1000)
    let half = 128;
    let mut cos = Vec::with_capacity(half);
    let mut sin = Vec::with_capacity(half);
    for i in 0..half {
        let f = (-(10000f64.ln()) * i as f64 / half as f64).exp();
        let a = v * 1000.0 * f;
        cos.push(a.cos() as f32);
        sin.push(a.sin() as f32);
    }
    cos.extend(sin);
    cos
}

struct Loader<'a> {
    content: &'a gguf_file::Content,
    file: std::fs::File,
    dev: &'a Device,
    loras: &'a [(HashMap<String, Tensor>, f64)],
    lora_hits: usize,
}

impl Loader<'_> {
    fn tensor(&mut self, name: &str) -> Result<Tensor> {
        let q = self.content.tensor(&mut self.file, name, &Device::Cpu)?;
        q.dequantize(&Device::Cpu)?.to_device(self.dev)
    }
    fn linear(&mut self, p: &str) -> Result<QLinear> {
        let wname = format!("{p}.weight");
        let info = self.content.tensor_infos.get(&wname).ok_or_else(|| candle_core::Error::Msg(format!("missing {wname}")))?;
        let b = if self.content.tensor_infos.contains_key(&format!("{p}.bias")) { Some(self.tensor(&format!("{p}.bias"))?) } else { None };
        let mut l = match info.ggml_dtype {
            GgmlDType::F32 | GgmlDType::F16 | GgmlDType::BF16 => QLinear::dense(self.tensor(&wname)?, b, CDT)?,
            _ => QLinear::quant(self.content.tensor(&mut self.file, &wname, self.dev)?, b, CDT)?,
        };
        for (m, strength) in self.loras {
            let (Some(down), Some(up)) = (m.get(&format!("{p}.lora_down.weight")), m.get(&format!("{p}.lora_up.weight"))) else { continue };
            let rank = down.dim(0)? as f64;
            let alpha = match m.get(&format!("{p}.alpha")) {
                Some(a) => a.to_dtype(DType::F32)?.flatten_all()?.to_vec1::<f32>()?.first().copied().unwrap_or(rank as f32) as f64,
                None => rank,
            };
            l.lora.push((down.to_device(self.dev)?.to_dtype(CDT)?, up.to_device(self.dev)?.to_dtype(CDT)?, strength * alpha / rank));
            self.lora_hits += 1;
        }
        Ok(l)
    }
}

/// Reads a LoRA file (kohya / diffusers names, with or without a `transformer.` /
/// `diffusion_model.` prefix) into a map keyed by the bare layer names.
pub fn read_lora(path: &str) -> Result<HashMap<String, Tensor>> {
    let st = unsafe { candle_core::safetensors::MmapedSafetensors::new(path)? };
    let mut m = HashMap::new();
    for (n, _) in st.tensors() {
        let mut k = n.clone();
        for pre in ["transformer.", "diffusion_model.", "model.diffusion_model."] {
            if let Some(r) = k.strip_prefix(pre) {
                k = r.to_string();
            }
        }
        // PEFT spelling -> kohya
        let k = k.replace(".lora_A.weight", ".lora_down.weight").replace(".lora_B.weight", ".lora_up.weight").replace(".lora_A.default.weight", ".lora_down.weight").replace(".lora_B.default.weight", ".lora_up.weight");
        m.insert(k, st.load(&n, &Device::Cpu)?);
    }
    Ok(m)
}

impl Mmdit {
    pub fn load(gguf: &str, dev: &Device, loras: &[(HashMap<String, Tensor>, f64)], status: &dyn Fn(&str)) -> Result<Self> {
        let mut file = std::fs::File::open(gguf)?;
        let content = gguf_file::Content::read(&mut file).map_err(|e| e.with_path(gguf))?;
        let zero_cond_t = content.tensor_infos.contains_key("__index_timestep_zero__");
        let mut l = Loader { content: &content, file, dev, loras, lora_hits: 0 };
        let img_in = l.linear("img_in")?;
        let txt_norm = l.tensor("txt_norm.weight")?;
        let txt_in = l.linear("txt_in")?;
        let t1 = l.linear("time_text_embed.timestep_embedder.linear_1")?;
        let t2 = l.linear("time_text_embed.timestep_embedder.linear_2")?;
        let mut blocks = Vec::new();
        let mut n_blocks = (0..).take_while(|i| content.tensor_infos.contains_key(&format!("transformer_blocks.{i}.img_mod.1.weight"))).count();
        // only for checking the port against the reference block by block
        if let Some(m) = std::env::var("LING_QWEN_MAX_BLOCKS").ok().and_then(|v| v.parse::<usize>().ok()) {
            n_blocks = n_blocks.min(m);
        }
        for i in 0..n_blocks {
            if i % 6 == 0 {
                status(&format!("loading the edit transformer {}/{}", i + 1, n_blocks));
            }
            let p = format!("transformer_blocks.{i}");
            blocks.push(Block {
                img_mod: l.linear(&format!("{p}.img_mod.1"))?,
                txt_mod: l.linear(&format!("{p}.txt_mod.1"))?,
                to_q: l.linear(&format!("{p}.attn.to_q"))?,
                to_k: l.linear(&format!("{p}.attn.to_k"))?,
                to_v: l.linear(&format!("{p}.attn.to_v"))?,
                to_out: l.linear(&format!("{p}.attn.to_out.0"))?,
                add_q: l.linear(&format!("{p}.attn.add_q_proj"))?,
                add_k: l.linear(&format!("{p}.attn.add_k_proj"))?,
                add_v: l.linear(&format!("{p}.attn.add_v_proj"))?,
                to_add_out: l.linear(&format!("{p}.attn.to_add_out"))?,
                nq: l.tensor(&format!("{p}.attn.norm_q.weight"))?,
                nk: l.tensor(&format!("{p}.attn.norm_k.weight"))?,
                naq: l.tensor(&format!("{p}.attn.norm_added_q.weight"))?,
                nak: l.tensor(&format!("{p}.attn.norm_added_k.weight"))?,
                img_mlp0: l.linear(&format!("{p}.img_mlp.net.0.proj"))?,
                img_mlp2: l.linear(&format!("{p}.img_mlp.net.2"))?,
                txt_mlp0: l.linear(&format!("{p}.txt_mlp.net.0.proj"))?,
                txt_mlp2: l.linear(&format!("{p}.txt_mlp.net.2"))?,
            });
        }
        let norm_out = l.linear("norm_out.linear")?;
        let proj_out = l.linear("proj_out")?;
        let lora_layers = l.lora_hits;
        Ok(Self { img_in, txt_norm, txt_in, t1, t2, blocks, norm_out, proj_out, zero_cond_t, device: dev.clone(), lora_layers })
    }

    /// x: [1, Li, 64] (noisy tokens first, then reference tokens), ctx: [1, Lt, 3584].
    /// Returns the velocity for the first `n_noisy` tokens: [1, n_noisy, 64].
    pub fn forward(&self, x: &Tensor, ctx: &Tensor, sigma: f64, n_noisy: usize, rope: &Rope) -> Result<Tensor> {
        let dev = &self.device;
        let li = x.dim(1)?;
        let lt = ctx.dim(1)?;
        let mut h = self.img_in.forward(x)?; // f32 [1,Li,3072]
        let mut c = self.txt_in.forward(&rms_norm(ctx, &self.txt_norm, 1e-6)?)?;
        // timestep embeddings: row 0 = sigma, row 1 = 0 (reference tokens, zero_cond_t)
        let mut te = timestep_embedding(sigma);
        let rows = if self.zero_cond_t && n_noisy < li { 2 } else { 1 };
        if rows == 2 {
            te.extend(timestep_embedding(0.0));
        }
        let te = Tensor::from_vec(te, (rows, 256), dev)?;
        let temb = self.t2.forward(&candle_nn::ops::silu(&self.t1.forward(&te)?)?)?; // [rows, 3072]
        let temb_act = candle_nn::ops::silu(&temb)?;
        let temb0_act = temb_act.narrow(0, 0, 1)?;
        let segs_n: Vec<usize> = if rows == 2 { vec![n_noisy, li - n_noisy] } else { vec![li] };
        for b in &self.blocks {
            let im = b.img_mod.forward(&temb_act)?; // [rows, 6*D]
            let tm = b.txt_mod.forward(&temb0_act)?; // [1, 6*D]
            let part = |t: &Tensor, r: usize, k: usize| -> Result<Tensor> { t.narrow(0, r, 1)?.narrow(1, k * DIM, DIM)?.squeeze(0) };
            // chunks: shift1 scale1 gate1 shift2 scale2 gate2
            let img_seg = |k0: usize| -> Result<(Vec<(usize, Tensor, Tensor)>, Vec<(usize, Tensor)>)> {
                let mut ms = Vec::new();
                let mut gs = Vec::new();
                for (r, &n) in segs_n.iter().enumerate() {
                    ms.push((n, part(&im, r, k0)?, part(&im, r, k0 + 1)?));
                    gs.push((n, part(&im, r, k0 + 2)?));
                }
                Ok((ms, gs))
            };
            // attention
            let (ms, gs) = img_seg(0)?;
            let hm = modulate_segments(&layer_norm(&h, 1e-6)?, &ms)?;
            let ig1 = gate_segments(&gs, DIM)?;
            let cm = modulate_segments(&layer_norm(&c, 1e-6)?, &[(lt, part(&tm, 0, 0)?, part(&tm, 0, 1)?)])?;
            let tg1 = part(&tm, 0, 2)?;

            let heads = |t: Tensor, l: usize| t.reshape((1, l, HEADS, HD));
            let iq = rope_pairs(&rms_norm(&heads(b.to_q.forward(&hm)?, li)?, &b.nq, 1e-6)?, &rope.img_cos, &rope.img_sin)?;
            let ik = rope_pairs(&rms_norm(&heads(b.to_k.forward(&hm)?, li)?, &b.nk, 1e-6)?, &rope.img_cos, &rope.img_sin)?;
            let iv = heads(b.to_v.forward(&hm)?, li)?;
            let tq = rope_pairs(&rms_norm(&heads(b.add_q.forward(&cm)?, lt)?, &b.naq, 1e-6)?, &rope.txt_cos, &rope.txt_sin)?;
            let tk = rope_pairs(&rms_norm(&heads(b.add_k.forward(&cm)?, lt)?, &b.nak, 1e-6)?, &rope.txt_cos, &rope.txt_sin)?;
            let tv = heads(b.add_v.forward(&cm)?, lt)?;
            let jq = Tensor::cat(&[&tq, &iq], 1)?.transpose(1, 2)?.to_dtype(CDT)?; // [1,H,L,Dh]
            let jk = Tensor::cat(&[&tk, &ik], 1)?.transpose(1, 2)?.to_dtype(CDT)?;
            let jv = Tensor::cat(&[&tv, &iv], 1)?.transpose(1, 2)?.to_dtype(CDT)?;
            let o = attention(&jq, &jk, &jv, None, 1.0 / (HD as f64).sqrt())?; // [1,H,L,Dh]
            let o = o.transpose(1, 2)?.reshape((1, lt + li, DIM))?;
            let to = o.narrow(1, 0, lt)?;
            let io = o.narrow(1, lt, li)?;
            h = (h + ig1.mul(&b.to_out.forward(&io)?)?)?;
            c = (c + b.to_add_out.forward(&to)?.broadcast_mul(&tg1)?)?;
            // MLPs
            let (ms, gs) = img_seg(3)?;
            let hm = modulate_segments(&layer_norm(&h, 1e-6)?, &ms)?;
            let ig2 = gate_segments(&gs, DIM)?;
            let mlp = b.img_mlp2.forward(&b.img_mlp0.forward(&hm)?.gelu()?)?;
            h = (h + ig2.mul(&mlp)?)?;
            let cm = modulate_segments(&layer_norm(&c, 1e-6)?, &[(lt, part(&tm, 0, 3)?, part(&tm, 0, 4)?)])?;
            let mlp = b.txt_mlp2.forward(&b.txt_mlp0.forward(&cm)?.gelu()?)?;
            c = (c + mlp.broadcast_mul(&part(&tm, 0, 5)?)?)?;
        }
        // AdaLayerNormContinuous: scale first, then shift
        let e = self.norm_out.forward(&temb0_act)?; // [1, 2D]
        let scale = e.narrow(1, 0, DIM)?.squeeze(0)?;
        let shift = e.narrow(1, DIM, DIM)?.squeeze(0)?;
        let hn = layer_norm(&h.narrow(1, 0, n_noisy)?, 1e-6)?;
        let hn = hn.broadcast_mul(&(scale + 1.0)?)?.broadcast_add(&shift)?;
        self.proj_out.forward(&hn)
    }
}
