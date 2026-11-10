//! Qwen2.5-VL-7B as Qwen-Image-Edit's text encoder: the vision tower (window attention, 2D RoPE,
//! 2x2 patch merger) turns the condition image into tokens that sit inside the chat prompt, and the
//! language model (M-RoPE) runs over the whole thing; its last hidden state, minus the 64 tokens of
//! the system prompt, conditions the MMDiT. Port of transformers' modeling_qwen2_5_vl.py.
//!
//! The language model's linears are quantized to Q8_0 as they load (~7.6 GB instead of 15 GB);
//! the vision tower stays bf16. The token embedding table stays on the CPU.

use super::ops::{attention, rms_norm, rope_half, QLinear};
use candle_core::quantized::{GgmlDType, QTensor};
use candle_core::{DType, Device, IndexOp, Result, Tensor, D};

const IMAGE_PAD: u32 = 151655;
const MEAN: [f32; 3] = [0.48145466, 0.4578275, 0.40821073];
const STD: [f32; 3] = [0.26862954, 0.26130258, 0.27577711];
pub const DROP: usize = 64;
pub const TEMPLATE: &str = "<|im_start|>system\nDescribe the key features of the input image (color, shape, size, texture, objects, background), then explain how the user's text instruction should alter or modify the image. Generate a new image that meets the user's requirements while maintaining consistency with the original input where appropriate.<|im_end|>\n<|im_start|>user\n{}<|im_end|>\n<|im_start|>assistant\n";

struct VBlock {
    n1: Tensor,
    n2: Tensor,
    qkv: QLinear,
    proj: QLinear,
    gate: QLinear,
    up: QLinear,
    down: QLinear,
}

struct LBlock {
    ln1: Tensor,
    ln2: Tensor,
    q: QLinear,
    k: QLinear,
    v: QLinear,
    o: QLinear,
    gate: QLinear,
    up: QLinear,
    down: QLinear,
}

pub struct Vl {
    patch: Tensor, // [1280, 3*14*14] (both temporal frames summed)
    vblocks: Vec<VBlock>,
    merger_ln: Tensor,
    m0: QLinear,
    m2: QLinear,
    embed: Tensor, // CPU, bf16 [vocab, 3584]
    layers: Vec<LBlock>,
    norm: Tensor,
    tok: tokenizers::Tokenizer,
    pub device: Device,
}

/// Qwen2-VL's smart_resize: both sides to multiples of 28, area within the processor's bounds.
pub fn smart_size(w: usize, h: usize) -> (usize, usize) {
    let f = 28.0;
    let (mut hb, mut wb) = (((h as f64 / f).round() * f).max(f), ((w as f64 / f).round() * f).max(f));
    let (minp, maxp) = (3136.0, 12845056.0);
    if hb * wb > maxp {
        let beta = ((h * w) as f64 / maxp).sqrt();
        hb = ((h as f64 / beta / f).floor() * f).max(f);
        wb = ((w as f64 / beta / f).floor() * f).max(f);
    } else if hb * wb < minp {
        let beta = (minp / (h * w) as f64).sqrt();
        hb = (h as f64 * beta / f).ceil() * f;
        wb = (w as f64 * beta / f).ceil() * f;
    }
    (wb as usize, hb as usize)
}

impl Vl {
    pub fn load(dir: &str, tokenizer_json: &str, dev: &Device, lm_q8: bool, status: &dyn Fn(&str)) -> Result<Self> {
        let mut shards: Vec<std::path::PathBuf> = std::fs::read_dir(dir)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("safetensors"))
            .collect();
        shards.sort();
        let st = unsafe { candle_core::safetensors::MmapedSafetensors::multi(&shards)? };
        let get = |n: &str| st.load(n, dev);
        let dense = |p: &str, bias: bool| -> Result<QLinear> {
            let b = if bias { Some(st.load(&format!("{p}.bias"), dev)?) } else { None };
            QLinear::dense(st.load(&format!("{p}.weight"), dev)?, b, DType::BF16)
        };
        // The language model's 196 linears: quantized to Q8_0 on all CPU cores at once (one by
        // one this took ~2 minutes), then uploaded in order.
        const LM_PARTS: [(&str, bool); 7] = [
            ("self_attn.q_proj", true),
            ("self_attn.k_proj", true),
            ("self_attn.v_proj", true),
            ("self_attn.o_proj", false),
            ("mlp.gate_proj", false),
            ("mlp.up_proj", false),
            ("mlp.down_proj", false),
        ];
        let names: Vec<String> = (0..28).flat_map(|i| LM_PARTS.iter().map(move |(n, _)| format!("model.layers.{i}.{n}"))).collect();
        let mut quantized: Vec<Option<QTensor>> = (0..names.len()).map(|_| None).collect();
        if lm_q8 {
            status("quantizing the text encoder");
            let next = std::sync::atomic::AtomicUsize::new(0);
            let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8).clamp(2, 12);
            let results: Vec<Vec<(usize, QTensor)>> = std::thread::scope(|sc| {
                let hs: Vec<_> = (0..workers)
                    .map(|_| {
                        sc.spawn(|| -> Result<Vec<(usize, QTensor)>> {
                            let mut out = Vec::new();
                            loop {
                                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                if i >= names.len() {
                                    break;
                                }
                                let w = st.load(&format!("{}.weight", names[i]), &Device::Cpu)?;
                                out.push((i, QTensor::quantize(&w, GgmlDType::Q8_0)?));
                            }
                            Ok(out)
                        })
                    })
                    .collect();
                hs.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(candle_core::Error::Msg("quantize thread panicked".into())))).collect::<Result<Vec<_>>>()
            })?;
            for (i, q) in results.into_iter().flatten() {
                quantized[i] = Some(q);
            }
        }
        let mut qi = 0usize;
        let mut lm = |p: &str, bias: bool| -> Result<QLinear> {
            let b = if bias { Some(st.load(&format!("{p}.bias"), dev)?) } else { None };
            let slot = qi;
            qi += 1;
            if let Some(q) = quantized[slot].take() {
                // CPU blocks -> the same blocks on the GPU
                let data = q.data()?;
                let storage = candle_core::quantized::QStorage::from_data(data, dev, q.dtype())?;
                QLinear::quant(QTensor::new(storage, q.shape().clone())?, b, DType::BF16)
            } else {
                QLinear::dense(st.load(&format!("{p}.weight"), dev)?, b, DType::BF16)
            }
        };
        status("loading the vision tower");
        let pw = get("visual.patch_embed.proj.weight")?.to_dtype(DType::F32)?; // [1280, 3, 2, 14, 14]
        let patch = (pw.narrow(2, 0, 1)? + pw.narrow(2, 1, 1)?)?.reshape((1280, 3 * 14 * 14))?.to_dtype(DType::BF16)?;
        let mut vblocks = Vec::new();
        for i in 0..32 {
            let p = format!("visual.blocks.{i}");
            vblocks.push(VBlock {
                n1: get(&format!("{p}.norm1.weight"))?,
                n2: get(&format!("{p}.norm2.weight"))?,
                qkv: dense(&format!("{p}.attn.qkv"), true)?,
                proj: dense(&format!("{p}.attn.proj"), true)?,
                gate: dense(&format!("{p}.mlp.gate_proj"), true)?,
                up: dense(&format!("{p}.mlp.up_proj"), true)?,
                down: dense(&format!("{p}.mlp.down_proj"), true)?,
            });
        }
        let merger_ln = get("visual.merger.ln_q.weight")?;
        let m0 = dense("visual.merger.mlp.0", true)?;
        let m2 = dense("visual.merger.mlp.2", true)?;
        let embed = st.load("model.embed_tokens.weight", &Device::Cpu)?;
        let mut layers = Vec::new();
        for i in 0..28 {
            status(&format!("loading the text encoder {}/28", i + 1));
            let p = format!("model.layers.{i}");
            layers.push(LBlock {
                ln1: get(&format!("{p}.input_layernorm.weight"))?,
                ln2: get(&format!("{p}.post_attention_layernorm.weight"))?,
                q: lm(&format!("{p}.{}", LM_PARTS[0].0), LM_PARTS[0].1)?,
                k: lm(&format!("{p}.{}", LM_PARTS[1].0), LM_PARTS[1].1)?,
                v: lm(&format!("{p}.{}", LM_PARTS[2].0), LM_PARTS[2].1)?,
                o: lm(&format!("{p}.{}", LM_PARTS[3].0), LM_PARTS[3].1)?,
                gate: lm(&format!("{p}.{}", LM_PARTS[4].0), LM_PARTS[4].1)?,
                up: lm(&format!("{p}.{}", LM_PARTS[5].0), LM_PARTS[5].1)?,
                down: lm(&format!("{p}.{}", LM_PARTS[6].0), LM_PARTS[6].1)?,
            });
        }
        let norm = get("model.norm.weight")?;
        let tok = tokenizers::Tokenizer::from_file(tokenizer_json).map_err(|e| candle_core::Error::Msg(format!("tokenizer: {e}")))?;
        Ok(Self { patch, vblocks, merger_ln, m0, m2, embed, layers, norm, tok, device: dev.clone() })
    }

    /// RGB bytes (w x h, both multiples of 28) -> image tokens [(h/28)*(w/28), 3584] f32
    pub fn vision(&self, rgb: &[u8], w: usize, h: usize) -> Result<Tensor> {
        let dev = &self.device;
        let (gh, gw) = (h / 14, w / 14);
        let (mh, mw) = (gh / 2, gw / 2);
        let n = gh * gw;
        // patches in the processor's order: merge blocks row-major, 2x2 inside, each (c, y, x)
        let mut data = Vec::with_capacity(n * 588);
        let mut pos = Vec::with_capacity(n * 2);
        for by in 0..mh {
            for bx in 0..mw {
                for iy in 0..2 {
                    for ix in 0..2 {
                        let (py, px) = (by * 2 + iy, bx * 2 + ix);
                        pos.push((py, px));
                        for c in 0..3 {
                            for y in 0..14 {
                                for x in 0..14 {
                                    let v = rgb[((py * 14 + y) * w + px * 14 + x) * 3 + c] as f32 / 255.0;
                                    data.push((v - MEAN[c]) / STD[c]);
                                }
                            }
                        }
                    }
                }
            }
        }
        let patches = Tensor::from_vec(data, (n, 588), dev)?.to_dtype(DType::BF16)?;
        let mut x = patches.matmul(&self.patch.t()?)?.to_dtype(DType::F32)?; // [n,1280]
        // 2D rotary: 20 frequencies for rows, 20 for columns (head_dim 80)
        let inv: Vec<f32> = (0..20).map(|i| 1.0 / 10000f32.powf((2 * i) as f32 / 40.0)).collect();
        let mut ang: Vec<f32> = Vec::with_capacity(n * 80);
        for &(py, px) in &pos {
            let mut row: Vec<f32> = Vec::with_capacity(40);
            row.extend(inv.iter().map(|f| py as f32 * f));
            row.extend(inv.iter().map(|f| px as f32 * f));
            ang.extend(row.iter());
            ang.extend(row.iter());
        }
        let ang = Tensor::from_vec(ang, (n, 80), dev)?;
        // window order: 4x4 merged blocks per window
        let ws = 4;
        let (nwh, nww) = ((mh + ws - (mh % ws)) / ws, (mw + ws - (mw % ws)) / ws);
        let mut order: Vec<u32> = Vec::new();
        let mut windows: Vec<(usize, usize)> = Vec::new(); // (start token, len) after reordering
        for wy in 0..nwh {
            for wx in 0..nww {
                let st = order.len() * 4;
                for y in wy * ws..(wy + 1) * ws {
                    for xx in wx * ws..(wx + 1) * ws {
                        if y < mh && xx < mw {
                            order.push((y * mw + xx) as u32);
                        }
                    }
                }
                let len = order.len() * 4 - st;
                if len > 0 {
                    windows.push((st, len));
                }
            }
        }
        let idx = Tensor::new(order.as_slice(), dev)?;
        let reorder = |t: &Tensor| -> Result<Tensor> {
            let d = t.dim(1)?;
            t.reshape((n / 4, 4 * d))?.index_select(&idx, 0)?.reshape((n, d))
        };
        x = reorder(&x)?;
        let ang = reorder(&ang)?;
        let (cos, sin) = (ang.cos()?, ang.sin()?);
        let full = [7usize, 15, 23, 31];
        for (li, b) in self.vblocks.iter().enumerate() {
            let hdn = rms_norm(&x, &b.n1, 1e-6)?;
            let qkv = b.qkv.forward(&hdn)?.reshape((n, 3, 16, 80))?;
            let q = rope_half(&qkv.i((.., 0))?, &cos, &sin)?; // [n,16,80]
            let k = rope_half(&qkv.i((.., 1))?, &cos, &sin)?;
            let v = qkv.i((.., 2))?;
            let segs: Vec<(usize, usize)> = if full.contains(&li) { vec![(0, n)] } else { windows.clone() };
            let mut outs = Vec::new();
            for (s, l) in segs {
                let qs = q.narrow(0, s, l)?.transpose(0, 1)?.unsqueeze(0)?.to_dtype(DType::BF16)?;
                let ks = k.narrow(0, s, l)?.transpose(0, 1)?.unsqueeze(0)?.to_dtype(DType::BF16)?;
                let vs = v.narrow(0, s, l)?.transpose(0, 1)?.unsqueeze(0)?.to_dtype(DType::BF16)?;
                let o = attention(&qs, &ks, &vs, None, 1.0 / 80f64.sqrt())?; // [1,16,l,80]
                outs.push(o.squeeze(0)?.transpose(0, 1)?.reshape((l, 1280))?);
            }
            let a = Tensor::cat(&outs.iter().collect::<Vec<_>>(), 0)?;
            x = (x + b.proj.forward(&a)?)?;
            let hdn = rms_norm(&x, &b.n2, 1e-6)?;
            let m = (candle_nn::ops::silu(&b.gate.forward(&hdn)?)? * b.up.forward(&hdn)?)?;
            x = (x + b.down.forward(&m)?)?;
        }
        let x = rms_norm(&x, &self.merger_ln, 1e-6)?.reshape((n / 4, 5120))?;
        let x = self.m2.forward(&self.m0.forward(&x)?.gelu_erf()?)?; // [n/4, 3584]
        // undo the window order
        let mut inv_order = vec![0u32; order.len()];
        for (i, &o) in order.iter().enumerate() {
            inv_order[o as usize] = i as u32;
        }
        x.index_select(&Tensor::new(inv_order.as_slice(), dev)?, 0)
    }

    /// The conditioning sequence for one instruction and one image: [L - 64, 3584] f32.
    pub fn encode(&self, prompt: &str, rgb: &[u8], w: usize, h: usize) -> Result<Tensor> {
        let dev = &self.device;
        let text = TEMPLATE.replace("{}", &format!("Picture 1: <|vision_start|><|image_pad|><|vision_end|>{prompt}"));
        let enc = self.tok.encode(text, false).map_err(|e| candle_core::Error::Msg(format!("tokenizer: {e}")))?;
        let (mh, mw) = (h / 28, w / 28);
        let n_img = mh * mw;
        let mut ids: Vec<u32> = Vec::new();
        let mut img_at = None;
        for &t in enc.get_ids() {
            if t == IMAGE_PAD && img_at.is_none() {
                img_at = Some(ids.len());
                ids.extend(std::iter::repeat(IMAGE_PAD).take(n_img));
            } else {
                ids.push(t);
            }
        }
        let img_at = img_at.ok_or_else(|| candle_core::Error::Msg("no image token in the prompt".into()))?;
        let l = ids.len();
        // M-RoPE positions (t, h, w)
        let mut pt = Vec::with_capacity(l);
        let mut ph = Vec::with_capacity(l);
        let mut pw = Vec::with_capacity(l);
        for i in 0..img_at {
            pt.push(i as f32);
            ph.push(i as f32);
            pw.push(i as f32);
        }
        let st = img_at as f32;
        for y in 0..mh {
            for x in 0..mw {
                pt.push(st);
                ph.push(st + y as f32);
                pw.push(st + x as f32);
            }
        }
        let next = st + mh.max(mw) as f32;
        for j in 0..(l - img_at - n_img) {
            let p = next + j as f32;
            pt.push(p);
            ph.push(p);
            pw.push(p);
        }
        let inv: Vec<f32> = (0..64).map(|i| 1.0 / 1_000_000f32.powf((2 * i) as f32 / 128.0)).collect();
        let mut ang: Vec<f32> = Vec::with_capacity(l * 128);
        for i in 0..l {
            let mut row: Vec<f32> = Vec::with_capacity(64);
            for (f, invf) in inv.iter().enumerate() {
                let p = if f < 16 { pt[i] } else if f < 40 { ph[i] } else { pw[i] };
                row.push(p * invf);
            }
            ang.extend(row.iter());
            ang.extend(row.iter());
        }
        let ang = Tensor::from_vec(ang, (l, 128), dev)?;
        let (cos, sin) = (ang.cos()?, ang.sin()?);
        // embeddings with the image tokens spliced in
        let emb = self.embed.index_select(&Tensor::new(ids.as_slice(), &Device::Cpu)?, 0)?.to_device(dev)?.to_dtype(DType::F32)?;
        let vis = self.vision(rgb, w, h)?;
        let mut x = Tensor::cat(&[&emb.narrow(0, 0, img_at)?, &vis, &emb.narrow(0, img_at + n_img, l - img_at - n_img)?], 0)?;
        // causal mask
        let mask: Vec<f32> = (0..l).flat_map(|i| (0..l).map(move |j| if j > i { f32::NEG_INFINITY } else { 0.0 })).collect();
        let mask = Tensor::from_vec(mask, (1, 1, l, l), dev)?;
        for b in &self.layers {
            let hdn = rms_norm(&x, &b.ln1, 1e-6)?;
            let q = rope_half(&b.q.forward(&hdn)?.reshape((l, 28, 128))?, &cos, &sin)?;
            let k = rope_half(&b.k.forward(&hdn)?.reshape((l, 4, 128))?, &cos, &sin)?;
            let v = b.v.forward(&hdn)?.reshape((l, 4, 128))?;
            // grouped-query attention: each of the 4 kv heads serves 7 query heads
            let q = q.transpose(0, 1)?.unsqueeze(0)?.to_dtype(DType::BF16)?; // [1,28,l,128]
            let rep = |t: &Tensor| -> Result<Tensor> { t.transpose(0, 1)?.unsqueeze(1)?.broadcast_as((4, 7, l, 128))?.reshape((1, 28, l, 128))?.to_dtype(DType::BF16) };
            let o = attention(&q, &rep(&k)?, &rep(&v)?, Some(&mask), 1.0 / 128f64.sqrt())?;
            let o = o.squeeze(0)?.transpose(0, 1)?.reshape((l, 3584))?;
            x = (x + b.o.forward(&o)?)?;
            let hdn = rms_norm(&x, &b.ln2, 1e-6)?;
            let m = (candle_nn::ops::silu(&b.gate.forward(&hdn)?)? * b.up.forward(&hdn)?)?;
            x = (x + b.down.forward(&m)?)?;
        }
        let x = rms_norm(&x, &self.norm, 1e-6)?;
        x.narrow(0, DROP, l - DROP)?.to_dtype(DType::F32)
    }
}

#[allow(dead_code)]
fn _d(_: D) {}
