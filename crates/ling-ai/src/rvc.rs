//! RVC voice conversion in Rust, on candle (CPU or CUDA): no Python.
//!
//! Loads the files an RVC WebUI uses as they are - `hubert_base.pt` (fairseq HuBERT / ContentVec) and a voice model `Name.pth` - and runs
//! the inference path of RVC v1 / v2 (`SynthesizerTrnMs256NSFsid` / `SynthesizerTrnMs768NSFsid`):
//!
//! 1. HuBERT: 16 kHz audio -> content features every 20 ms (v2: the 12th layer, 768 wide; v1: through `final_proj`, 256 wide), doubled to
//!    10 ms frames.
//! 2. The pitch (Hz every 10 ms, given by the caller), moved by the transpose, quantised to the 256 mel bins the model embeds.
//! 3. The text encoder (6 transformer layers with relative position attention, window 10) -> mean / log-scale of the latent.
//! 4. A sample of the latent through the reversed coupling flow (4 WaveNet coupling layers conditioned on the speaker).
//! 5. NSF-HiFiGAN: a sine excitation at the pitch plus transposed convolutions and multi-receptive-field residual blocks -> audio at the
//!    model's rate (32 / 40 / 48 kHz).
//!
//! Long audio is converted in overlapping windows and cross-faded. The feature index (`.index`, faiss) is not used: the voice comes from
//! the model alone (an RVC "index rate" of 0).
#![cfg(feature = "llm")]

use candle_core::{DType, Device, IndexOp, Tensor, D};
use std::collections::HashMap;
use std::io::BufReader;
use std::path::Path;

type R<T> = Result<T, String>;
fn e<T: std::fmt::Display>(x: T) -> String {
    x.to_string()
}

// ── checkpoints ─────────────────────────────────────────────────────────────

struct W {
    t: HashMap<String, Tensor>,
}

impl W {
    fn load(path: &Path, key: Option<&str>, dev: &Device) -> R<W> {
        let all = candle_core::pickle::read_all_with_key(path, key).map_err(|x| format!("{}: {x}", path.display()))?;
        let mut t = HashMap::new();
        for (k, v) in all {
            let v = v.to_dtype(DType::F32).map_err(e)?.to_device(dev).map_err(e)?;
            t.insert(k, v);
        }
        if t.is_empty() {
            return Err(format!("{}: no weights found", path.display()));
        }
        Ok(W { t })
    }
    fn get(&self, k: &str) -> R<Tensor> {
        self.t.get(k).cloned().ok_or_else(|| format!("missing weight {k}"))
    }
    fn has(&self, k: &str) -> bool {
        self.t.contains_key(k)
    }
    /// a weight that may be stored weight-normed (`weight_g` / `weight_v`, or torch's newer `parametrizations.weight.original0/1`);
    /// `dim` is the dimension the norm keeps
    fn wn(&self, prefix: &str, dim: usize) -> R<Tensor> {
        let plain = format!("{prefix}.weight");
        if self.has(&plain) {
            return self.get(&plain);
        }
        let (g, v) = if self.has(&format!("{prefix}.weight_g")) {
            (self.get(&format!("{prefix}.weight_g"))?, self.get(&format!("{prefix}.weight_v"))?)
        } else {
            (self.get(&format!("{prefix}.parametrizations.weight.original0"))?, self.get(&format!("{prefix}.parametrizations.weight.original1"))?)
        };
        let rank = v.rank();
        let mut n = v.sqr().map_err(e)?;
        for d in 0..rank {
            if d != dim {
                n = n.sum_keepdim(d).map_err(e)?;
            }
        }
        let n = n.sqrt().map_err(e)?;
        v.broadcast_mul(&g.broadcast_div(&n).map_err(e)?).map_err(e)
    }
    fn bias(&self, prefix: &str) -> Option<Tensor> {
        self.t.get(&format!("{prefix}.bias")).cloned()
    }
}

/// the `config` list of an RVC .pth (the numbers next to the weights), read with candle's pickle machine
fn read_config(path: &Path) -> R<Vec<candle_core::pickle::Object>> {
    use candle_core::pickle::{Object, Stack};
    let f = std::fs::File::open(path).map_err(e)?;
    let mut zip = zip::ZipArchive::new(BufReader::new(f)).map_err(e)?;
    let name = zip.file_names().find(|n| n.ends_with("data.pkl")).map(|s| s.to_string()).ok_or("no data.pkl in the model")?;
    let r = zip.by_name(&name).map_err(e)?;
    let mut st = Stack::empty();
    st.read_loop(&mut BufReader::new(r)).map_err(e)?;
    let obj = st.finalize().map_err(e)?;
    let Object::Dict(kv) = obj else { return Err("the model is not a dict".into()) };
    for (k, v) in kv {
        if k == Object::Unicode("config".into()) {
            if let Object::List(items) | Object::Tuple(items) = v {
                return Ok(items);
            }
        }
    }
    Err("no config in the model".into())
}

fn obj_num(o: &candle_core::pickle::Object) -> Option<f64> {
    use candle_core::pickle::Object;
    match o {
        Object::Int(i) => Some(*i as f64),
        Object::Long(i) => Some(*i as f64),
        Object::Float(f) => Some(*f),
        Object::Unicode(s) => s.trim_end_matches('k').parse::<f64>().ok().map(|x| if s.ends_with('k') { x * 1000.0 } else { x }),
        _ => None,
    }
}
fn obj_nums(o: &candle_core::pickle::Object) -> Vec<usize> {
    use candle_core::pickle::Object;
    match o {
        Object::List(v) | Object::Tuple(v) => v.iter().filter_map(obj_num).map(|x| x as usize).collect(),
        _ => vec![],
    }
}

// ── layers ──────────────────────────────────────────────────────────────────

struct Conv {
    w: Tensor,
    b: Option<Tensor>,
    pad: usize,
    stride: usize,
    dil: usize,
    groups: usize,
}
impl Conv {
    fn new(w: &W, prefix: &str, wn: bool, pad: usize, stride: usize, dil: usize, groups: usize) -> R<Conv> {
        let wt = if wn { w.wn(prefix, 0)? } else { w.get(&format!("{prefix}.weight"))? };
        Ok(Conv { w: wt, b: w.bias(prefix), pad, stride, dil, groups })
    }
    fn fwd(&self, x: &Tensor) -> R<Tensor> {
        let y = x.conv1d(&self.w, self.pad, self.stride, self.dil, self.groups).map_err(e)?;
        match &self.b {
            Some(b) => y.broadcast_add(&b.reshape((1, (), 1)).map_err(e)?).map_err(e),
            None => Ok(y),
        }
    }
}

fn linear(x: &Tensor, w: &Tensor, b: Option<&Tensor>) -> R<Tensor> {
    // x [.., in], w [out, in]
    let y = x.broadcast_matmul(&w.t().map_err(e)?).map_err(e)?;
    match b {
        Some(b) => y.broadcast_add(b).map_err(e),
        None => Ok(y),
    }
}

fn layer_norm_last(x: &Tensor, g: &Tensor, b: &Tensor, eps: f64) -> R<Tensor> {
    let mean = x.mean_keepdim(D::Minus1).map_err(e)?;
    let xc = x.broadcast_sub(&mean).map_err(e)?;
    let var = xc.sqr().map_err(e)?.mean_keepdim(D::Minus1).map_err(e)?;
    let y = xc.broadcast_div(&(var + eps).map_err(e)?.sqrt().map_err(e)?).map_err(e)?;
    y.broadcast_mul(g).map_err(e)?.broadcast_add(b).map_err(e)
}

/// layer norm over the channels of [B, C, T]
fn layer_norm_ch(x: &Tensor, g: &Tensor, b: &Tensor) -> R<Tensor> {
    let y = layer_norm_last(&x.transpose(1, 2).map_err(e)?, g, b, 1e-5)?;
    y.transpose(1, 2).map_err(e)
}

fn leaky(x: &Tensor, slope: f64) -> R<Tensor> {
    x.maximum(&(x * slope).map_err(e)?).map_err(e)
}

// ── HuBERT ──────────────────────────────────────────────────────────────────

pub struct Hubert {
    convs: Vec<Conv>,
    gn: (Tensor, Tensor),
    ln: (Tensor, Tensor),
    proj: (Tensor, Tensor),
    pos: Conv,
    enc_ln: (Tensor, Tensor),
    layers: Vec<HLayer>,
    final_proj: Option<(Tensor, Tensor)>,
}

struct HLayer {
    q: (Tensor, Tensor),
    k: (Tensor, Tensor),
    v: (Tensor, Tensor),
    o: (Tensor, Tensor),
    ln1: (Tensor, Tensor),
    fc1: (Tensor, Tensor),
    fc2: (Tensor, Tensor),
    ln2: (Tensor, Tensor),
}

impl Hubert {
    pub fn load(path: &Path, dev: &Device) -> R<Hubert> {
        let w = W::load(path, Some("model"), dev)?;
        let pair = |k: &str| -> R<(Tensor, Tensor)> { Ok((w.get(&format!("{k}.weight"))?, w.get(&format!("{k}.bias"))?)) };
        let specs = [(10usize, 5usize), (3, 2), (3, 2), (3, 2), (3, 2), (2, 2), (2, 2)];
        let mut convs = vec![];
        for (i, (_, s)) in specs.iter().enumerate() {
            convs.push(Conv::new(&w, &format!("feature_extractor.conv_layers.{i}.0"), false, 0, *s, 1, 1)?);
        }
        let mut layers = vec![];
        let mut i = 0;
        while w.has(&format!("encoder.layers.{i}.fc1.weight")) {
            let p = |s: &str| pair(&format!("encoder.layers.{i}.{s}"));
            layers.push(HLayer { q: p("self_attn.q_proj")?, k: p("self_attn.k_proj")?, v: p("self_attn.v_proj")?, o: p("self_attn.out_proj")?, ln1: p("self_attn_layer_norm")?, fc1: p("fc1")?, fc2: p("fc2")?, ln2: p("final_layer_norm")? });
            i += 1;
        }
        let pos_w = w.wn("encoder.pos_conv.0", 2)?;
        let pos = Conv { w: pos_w, b: w.bias("encoder.pos_conv.0"), pad: 64, stride: 1, dil: 1, groups: 16 };
        let final_proj = if w.has("final_proj.weight") { Some(pair("final_proj")?) } else { None };
        Ok(Hubert { convs, gn: pair("feature_extractor.conv_layers.0.2")?, ln: pair("layer_norm")?, proj: pair("post_extract_proj")?, pos, enc_ln: pair("encoder.layer_norm")?, layers, final_proj })
    }

    /// 16 kHz mono -> [1, frames (20 ms), 768] (`layer` = how many transformer layers; RVC v2 takes 12) or, `v1`, through final_proj (256)
    pub fn features(&self, audio: &Tensor, layer: usize, v1: bool) -> R<Tensor> {
        let mut x = audio.reshape((1, 1, ())).map_err(e)?;
        for (i, c) in self.convs.iter().enumerate() {
            x = c.fwd(&x)?;
            if i == 0 {
                // GroupNorm(512 groups, 512 channels): every channel normalised over time
                let mean = x.mean_keepdim(2).map_err(e)?;
                let xc = x.broadcast_sub(&mean).map_err(e)?;
                let var = xc.sqr().map_err(e)?.mean_keepdim(2).map_err(e)?;
                x = xc.broadcast_div(&(var + 1e-5).map_err(e)?.sqrt().map_err(e)?).map_err(e)?;
                x = x.broadcast_mul(&self.gn.0.reshape((1, (), 1)).map_err(e)?).map_err(e)?.broadcast_add(&self.gn.1.reshape((1, (), 1)).map_err(e)?).map_err(e)?;
            }
            x = x.gelu_erf().map_err(e)?;
        }
        // [1, 512, T] -> [1, T, 512] -> layer norm -> 768
        let x = x.transpose(1, 2).map_err(e)?;
        let x = layer_norm_last(&x, &self.ln.0, &self.ln.1, 1e-5)?;
        let x = linear(&x, &self.proj.0, Some(&self.proj.1))?;
        // positional convolution (the even kernel leaves one frame too many: dropped)
        let t = x.dim(1).map_err(e)?;
        let pc = self.pos.fwd(&x.transpose(1, 2).map_err(e)?)?.narrow(2, 0, t).map_err(e)?.gelu_erf().map_err(e)?;
        let x = (x + pc.transpose(1, 2).map_err(e)?).map_err(e)?;
        let mut x = layer_norm_last(&x, &self.enc_ln.0, &self.enc_ln.1, 1e-5)?;
        let heads = 12usize;
        for l in self.layers.iter().take(layer) {
            let (b, t, c) = x.dims3().map_err(e)?;
            let hd = c / heads;
            let q = (linear(&x, &l.q.0, Some(&l.q.1))? * (1.0 / (hd as f64).sqrt())).map_err(e)?;
            let k = linear(&x, &l.k.0, Some(&l.k.1))?;
            let v = linear(&x, &l.v.0, Some(&l.v.1))?;
            let split = |y: Tensor| -> R<Tensor> { y.reshape((b, t, heads, hd)).map_err(e)?.transpose(1, 2).map_err(e)?.contiguous().map_err(e) };
            let (q, k, v) = (split(q)?, split(k)?, split(v)?);
            let att = q.matmul(&k.transpose(2, 3).map_err(e)?.contiguous().map_err(e)?).map_err(e)?;
            let att = candle_nn::ops::softmax_last_dim(&att).map_err(e)?;
            let y = att.matmul(&v).map_err(e)?.transpose(1, 2).map_err(e)?.reshape((b, t, c)).map_err(e)?;
            let y = linear(&y, &l.o.0, Some(&l.o.1))?;
            x = layer_norm_last(&(x + y).map_err(e)?, &l.ln1.0, &l.ln1.1, 1e-5)?;
            let h = linear(&x, &l.fc1.0, Some(&l.fc1.1))?.gelu_erf().map_err(e)?;
            let h = linear(&h, &l.fc2.0, Some(&l.fc2.1))?;
            x = layer_norm_last(&(x + h).map_err(e)?, &l.ln2.0, &l.ln2.1, 1e-5)?;
        }
        if v1 {
            if let Some((w, b)) = &self.final_proj {
                return linear(&x, w, Some(b));
            }
        }
        Ok(x)
    }
}

// ── the RVC synthesizer ─────────────────────────────────────────────────────

pub struct Voice {
    w: W,
    pub sr: u32,
    rates: Vec<usize>,
    ups_k: Vec<usize>,
    res_k: Vec<usize>,
    res_d: Vec<Vec<usize>>,
    up_ch: usize,
    hidden: usize,
    n_layers: usize,
    n_heads: usize,
    pub v1: bool,
    pub speakers: usize,
    dev: Device,
}

impl Voice {
    pub fn load(path: &Path, dev: &Device) -> R<Voice> {
        let w = W::load(path, Some("weight"), dev)?;
        if !w.has("enc_p.emb_pitch.weight") {
            return Err("this voice model has no pitch input (an RVC model trained without f0): not supported".into());
        }
        let cfg = read_config(path).unwrap_or_default();
        let num = |i: usize, d: f64| cfg.get(i).and_then(obj_num).unwrap_or(d);
        let list = |i: usize| cfg.get(i).map(obj_nums).unwrap_or_default();
        let hidden = num(3, 192.0) as usize;
        let mut rates = list(12);
        let mut ups_k = list(14);
        let res_k = {
            let l = list(10);
            if l.is_empty() { vec![3, 7, 11] } else { l }
        };
        let res_d = match cfg.get(11) {
            Some(candle_core::pickle::Object::List(v)) | Some(candle_core::pickle::Object::Tuple(v)) => v.iter().map(obj_nums).collect(),
            _ => vec![vec![1, 3, 5]; 3],
        };
        let mut sr = num(17, 0.0) as u32;
        // without a readable config: the rates from the shapes of the up-sampling layers (kernel = 2 x rate, the 48 kHz layout aside)
        if rates.is_empty() {
            let mut i = 0;
            while w.has(&format!("dec.ups.{i}.weight_v")) || w.has(&format!("dec.ups.{i}.weight")) || w.has(&format!("dec.ups.{i}.parametrizations.weight.original1")) {
                let t = w.wn(&format!("dec.ups.{i}"), 0)?;
                let k = t.dim(2).map_err(e)?;
                ups_k.push(k);
                rates.push(match k {
                    24 => 12,
                    20 | 16 => 10,
                    k => k / 2,
                });
                i += 1;
            }
        }
        if sr == 0 {
            sr = (rates.iter().product::<usize>() * 100) as u32;
        }
        let up_ch = num(13, 512.0) as usize;
        let speakers = w.get("emb_g.weight")?.dim(0).map_err(e)?;
        let v1 = w.get("enc_p.emb_phone.weight")?.dim(1).map_err(e)? == 256;
        let mut n_layers = 0;
        while w.has(&format!("enc_p.encoder.attn_layers.{n_layers}.conv_q.weight")) {
            n_layers += 1;
        }
        Ok(Voice { w, sr, rates, ups_k, res_k, res_d, up_ch, hidden, n_layers, n_heads: num(5, 2.0) as usize, v1, speakers, dev: dev.clone() })
    }

    /// relative-position self attention of the text encoder (window 10, heads share the embeddings) over x [1, C, T]
    fn rel_attention(&self, x: &Tensor, i: usize) -> R<Tensor> {
        let p = format!("enc_p.encoder.attn_layers.{i}");
        let conv = |n: &str| Conv::new(&self.w, &format!("{p}.{n}"), false, 0, 1, 1, 1);
        let (q, k, v) = (conv("conv_q")?.fwd(x)?, conv("conv_k")?.fwd(x)?, conv("conv_v")?.fwd(x)?);
        let (_, c, t) = x.dims3().map_err(e)?;
        let h = self.n_heads;
        let kc = c / h;
        let to_heads = |y: Tensor| -> R<Tensor> { y.reshape((1, h, kc, t)).map_err(e)?.transpose(2, 3).map_err(e)?.contiguous().map_err(e) };
        let q = (to_heads(q)? * (1.0 / (kc as f64).sqrt())).map_err(e)?;
        let (k, v) = (to_heads(k)?, to_heads(v)?);
        let mut scores = q.matmul(&k.transpose(2, 3).map_err(e)?.contiguous().map_err(e)?).map_err(e)?; // [1,h,t,t]
        let ek = self.w.get(&format!("{p}.emb_rel_k"))?.squeeze(0).map_err(e)?; // [2w+1, kc]
        let ev = self.w.get(&format!("{p}.emb_rel_v"))?.squeeze(0).map_err(e)?;
        let nw = ek.dim(0).map_err(e)?;
        let win = (nw - 1) / 2;
        // the score of query i with key j gets q_i . Ek[j - i + win] when |j - i| <= win
        let rq: Vec<f32> = q.matmul(&ek.t().map_err(e)?.contiguous().map_err(e)?.unsqueeze(0).map_err(e)?.unsqueeze(0).map_err(e)?.broadcast_as((1, h, kc, nw)).map_err(e)?.contiguous().map_err(e)?).map_err(e)?.flatten_all().map_err(e)?.to_vec1().map_err(e)?;
        let mut local = vec![0f32; h * t * t];
        for hh in 0..h {
            for a in 0..t {
                for r in 0..nw {
                    let j = a as isize + r as isize - win as isize;
                    if j >= 0 && (j as usize) < t {
                        local[hh * t * t + a * t + j as usize] = rq[hh * t * nw + a * nw + r];
                    }
                }
            }
        }
        scores = (scores + Tensor::from_vec(local, (1, h, t, t), &self.dev).map_err(e)?).map_err(e)?;
        let p_attn = candle_nn::ops::softmax_last_dim(&scores).map_err(e)?;
        let mut out = p_attn.matmul(&v).map_err(e)?; // [1,h,t,kc]
        // and the output gets sum_j p[i,j] Ev[j - i + win]
        let pa: Vec<f32> = p_attn.flatten_all().map_err(e)?.to_vec1().map_err(e)?;
        let mut rel = vec![0f32; h * t * nw];
        for hh in 0..h {
            for a in 0..t {
                for r in 0..nw {
                    let j = a as isize + r as isize - win as isize;
                    if j >= 0 && (j as usize) < t {
                        rel[hh * t * nw + a * nw + r] = pa[hh * t * t + a * t + j as usize];
                    }
                }
            }
        }
        let rel = Tensor::from_vec(rel, (1, h, t, nw), &self.dev).map_err(e)?;
        out = (out + rel.broadcast_matmul(&ev).map_err(e)?).map_err(e)?;
        let out = out.transpose(2, 3).map_err(e)?.reshape((1, c, t)).map_err(e)?;
        conv("conv_o")?.fwd(&out)
    }

    fn text_encoder(&self, phone: &Tensor, pitch: &Tensor) -> R<(Tensor, Tensor)> {
        let w = &self.w;
        let x = linear(phone, &w.get("enc_p.emb_phone.weight")?, Some(&w.get("enc_p.emb_phone.bias")?))?;
        let pe = w.get("enc_p.emb_pitch.weight")?.index_select(pitch, 0).map_err(e)?.unsqueeze(0).map_err(e)?;
        let x = ((x + pe).map_err(e)? * (self.hidden as f64).sqrt()).map_err(e)?;
        let mut x = leaky(&x, 0.1)?.transpose(1, 2).map_err(e)?.contiguous().map_err(e)?; // [1, hidden, T]
        for i in 0..self.n_layers {
            let y = self.rel_attention(&x, i)?;
            x = layer_norm_ch(&(x + y).map_err(e)?, &w.get(&format!("enc_p.encoder.norm_layers_1.{i}.gamma"))?, &w.get(&format!("enc_p.encoder.norm_layers_1.{i}.beta"))?)?;
            let c1 = Conv::new(w, &format!("enc_p.encoder.ffn_layers.{i}.conv_1"), false, 0, 1, 1, 1)?;
            let c2 = Conv::new(w, &format!("enc_p.encoder.ffn_layers.{i}.conv_2"), false, 0, 1, 1, 1)?;
            let k1 = c1.w.dim(2).map_err(e)?;
            let k2 = c2.w.dim(2).map_err(e)?;
            let y = c1.fwd(&x.pad_with_zeros(2, (k1 - 1) / 2, k1 / 2).map_err(e)?)?.relu().map_err(e)?;
            let y = c2.fwd(&y.pad_with_zeros(2, (k2 - 1) / 2, k2 / 2).map_err(e)?)?;
            x = layer_norm_ch(&(x + y).map_err(e)?, &w.get(&format!("enc_p.encoder.norm_layers_2.{i}.gamma"))?, &w.get(&format!("enc_p.encoder.norm_layers_2.{i}.beta"))?)?;
        }
        let stats = Conv::new(w, "enc_p.proj", false, 0, 1, 1, 1)?.fwd(&x)?;
        let half = stats.dim(1).map_err(e)? / 2;
        Ok((stats.narrow(1, 0, half).map_err(e)?, stats.narrow(1, half, half).map_err(e)?))
    }

    /// WaveNet of a coupling layer
    fn wavenet(&self, prefix: &str, x: &Tensor, g: &Tensor) -> R<Tensor> {
        let w = &self.w;
        let cond = Conv::new(w, &format!("{prefix}.cond_layer"), true, 0, 1, 1, 1)?.fwd(g)?; // [1, 2*h*n, 1]
        let hc = x.dim(1).map_err(e)?;
        let mut n = 0;
        while w.has(&format!("{prefix}.in_layers.{n}.bias")) {
            n += 1;
        }
        let mut x = x.clone();
        let mut out: Option<Tensor> = None;
        for i in 0..n {
            let pin = w.wn(&format!("{prefix}.in_layers.{i}"), 0)?;
            let k = pin.dim(2).map_err(e)?;
            let conv = Conv { w: pin, b: w.bias(&format!("{prefix}.in_layers.{i}")), pad: (k - 1) / 2, stride: 1, dil: 1, groups: 1 };
            let x_in = conv.fwd(&x)?;
            let g_l = cond.narrow(1, i * 2 * hc, 2 * hc).map_err(e)?;
            let a = x_in.broadcast_add(&g_l).map_err(e)?;
            let acts = (a.narrow(1, 0, hc).map_err(e)?.tanh().map_err(e)? * candle_nn::ops::sigmoid(&a.narrow(1, hc, hc).map_err(e)?).map_err(e)?).map_err(e)?;
            let rs = Conv::new(w, &format!("{prefix}.res_skip_layers.{i}"), true, 0, 1, 1, 1)?.fwd(&acts)?;
            if i + 1 < n {
                x = (x + rs.narrow(1, 0, hc).map_err(e)?).map_err(e)?;
                let skip = rs.narrow(1, hc, hc).map_err(e)?;
                out = Some(match out {
                    Some(o) => (o + skip).map_err(e)?,
                    None => skip,
                });
            } else {
                out = Some(match out {
                    Some(o) => (o + rs).map_err(e)?,
                    None => rs,
                });
            }
        }
        out.ok_or_else(|| "empty WaveNet".into())
    }

    /// the coupling flow, backwards (latent -> features the decoder reads)
    fn flow_reverse(&self, z: &Tensor, g: &Tensor) -> R<Tensor> {
        let mut layers = vec![];
        let mut i = 0;
        while self.w.has(&format!("flow.flows.{i}.pre.weight")) {
            layers.push(i);
            i += 2;
        }
        let mut x = z.clone();
        for &li in layers.iter().rev() {
            // the Flip after this layer, undone first
            let c = x.dim(1).map_err(e)?;
            let idx = Tensor::from_vec((0..c as u32).rev().collect::<Vec<u32>>(), c, &self.dev).map_err(e)?;
            x = x.index_select(&idx, 1).map_err(e)?;
            let half = c / 2;
            let x0 = x.narrow(1, 0, half).map_err(e)?;
            let x1 = x.narrow(1, half, half).map_err(e)?;
            let p = format!("flow.flows.{li}");
            let h = Conv::new(&self.w, &format!("{p}.pre"), false, 0, 1, 1, 1)?.fwd(&x0)?;
            let h = self.wavenet(&format!("{p}.enc"), &h, g)?;
            let m = Conv::new(&self.w, &format!("{p}.post"), false, 0, 1, 1, 1)?.fwd(&h)?;
            let x1 = (x1 - m).map_err(e)?;
            x = Tensor::cat(&[&x0, &x1], 1).map_err(e)?;
        }
        Ok(x)
    }

    /// NSF-HiFiGAN: latent [1, C, T] + pitch (Hz per frame) -> audio
    fn decode(&self, z: &Tensor, f0: &[f32], g: &Tensor, seed: u64) -> R<Vec<f32>> {
        let w = &self.w;
        let upp: usize = self.rates.iter().product();
        // the harmonic source: a sine at the pitch, noise where unvoiced, through the source module's linear layer and tanh
        let n = f0.len() * upp;
        let mut src = vec![0f32; n];
        let mut ph = 0f64;
        let mut rng = seed.max(1);
        let mut gauss = || {
            let mut s = 0.0f64;
            for _ in 0..4 {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                s += (rng % 10000) as f64 / 10000.0;
            }
            ((s - 2.0) * 1.732) as f32
        };
        let lw = w.get("dec.m_source.l_linear.weight")?.flatten_all().map_err(e)?.to_vec1::<f32>().map_err(e)?;
        let lb = w.get("dec.m_source.l_linear.bias")?.to_vec1::<f32>().map_err(e)?;
        for (i, s) in src.iter_mut().enumerate() {
            let f = f0[i / upp] as f64;
            let voiced = f > 0.0;
            ph = (ph + f / self.sr as f64).fract();
            let sine = if voiced { (ph * std::f64::consts::TAU).sin() as f32 * 0.1 + 0.003 * gauss() } else { 0.1 / 3.0 * gauss() };
            *s = (sine * lw[0] + lb[0]).tanh();
        }
        let har = Tensor::from_vec(src, (1, 1, n), &self.dev).map_err(e)?;
        let mut x = Conv::new(w, "dec.conv_pre", false, 3, 1, 1, 1)?.fwd(z)?;
        x = x.broadcast_add(&Conv::new(w, "dec.cond", false, 0, 1, 1, 1)?.fwd(g)?).map_err(e)?;
        let nk = self.res_k.len();
        for (i, &u) in self.rates.iter().enumerate() {
            x = leaky(&x, 0.1)?;
            let k = self.ups_k.get(i).copied().unwrap_or(u * 2);
            let wt = w.wn(&format!("dec.ups.{i}"), 0)?;
            x = x.conv_transpose1d(&wt, (k - u) / 2, 0, u, 1, 1).map_err(e)?;
            if let Some(b) = w.bias(&format!("dec.ups.{i}")) {
                x = x.broadcast_add(&b.reshape((1, (), 1)).map_err(e)?).map_err(e)?;
            }
            // the excitation at this rate
            let nc = w.get(&format!("dec.noise_convs.{i}.weight"))?;
            let ks = nc.dim(2).map_err(e)?;
            let stride: usize = self.rates[i + 1..].iter().product();
            let xs = if i + 1 < self.rates.len() { Conv { w: nc, b: w.bias(&format!("dec.noise_convs.{i}")), pad: stride / 2, stride, dil: 1, groups: 1 }.fwd(&har)? } else { Conv { w: nc, b: w.bias(&format!("dec.noise_convs.{i}")), pad: (ks - 1) / 2, stride: 1, dil: 1, groups: 1 }.fwd(&har)? };
            let len = x.dim(2).map_err(e)?.min(xs.dim(2).map_err(e)?);
            x = (x.narrow(2, 0, len).map_err(e)? + xs.narrow(2, 0, len).map_err(e)?).map_err(e)?;
            // multi-receptive-field fusion
            let mut acc: Option<Tensor> = None;
            for j in 0..nk {
                let rb = i * nk + j;
                let kk = self.res_k[j];
                let mut y = x.clone();
                for (m, &d) in self.res_d.get(j).cloned().unwrap_or(vec![1, 3, 5]).iter().enumerate() {
                    let c1 = Conv::new(w, &format!("dec.resblocks.{rb}.convs1.{m}"), true, (kk * d - d) / 2, 1, d, 1)?;
                    let c2 = Conv::new(w, &format!("dec.resblocks.{rb}.convs2.{m}"), true, (kk - 1) / 2, 1, 1, 1)?;
                    let t = c2.fwd(&leaky(&c1.fwd(&leaky(&y, 0.1)?)?, 0.1)?)?;
                    y = (t + y).map_err(e)?;
                }
                acc = Some(match acc {
                    Some(a) => (a + y).map_err(e)?,
                    None => y,
                });
            }
            x = (acc.ok_or("no resblocks")? / nk as f64).map_err(e)?;
        }
        let x = leaky(&x, 0.01)?;
        let x = Conv::new(w, "dec.conv_post", true, 3, 1, 1, 1)?.fwd(&x)?.tanh().map_err(e)?;
        x.flatten_all().map_err(e)?.to_vec1::<f32>().map_err(e)
    }
}

// ── the whole conversion ────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Options {
    /// semitones
    pub transpose: f64,
    /// 0..1: 1 keeps the model's own loudness, lower follows the input's loudness envelope
    pub rms_mix: f32,
    pub speaker: usize,
    pub seed: u64,
}
impl Default for Options {
    fn default() -> Self {
        Options { transpose: 0.0, rms_mix: 0.25, speaker: 0, seed: 1 }
    }
}

pub struct Rvc {
    pub hubert: Hubert,
    pub voice: Voice,
    pub dev: Device,
}

fn coarse(f0: f32) -> u32 {
    let mel = |f: f64| 1127.0 * (1.0 + f / 700.0).ln();
    let (lo, hi) = (mel(50.0), mel(1100.0));
    if f0 <= 0.0 {
        return 1;
    }
    let m = mel(f0 as f64);
    (((m - lo) * 254.0 / (hi - lo)) + 1.0).round().clamp(1.0, 255.0) as u32
}

impl Rvc {
    /// `device_index` < 0 = CPU
    pub fn load(hubert: &Path, model: &Path, device_index: i64) -> R<Rvc> {
        let dev = if device_index < 0 { Device::Cpu } else { Device::new_cuda(device_index as usize).unwrap_or(Device::Cpu) };
        Ok(Rvc { hubert: Hubert::load(hubert, &dev)?, voice: Voice::load(model, &dev)?, dev })
    }

    fn convert_window(&self, a16: &[f32], f0: &[f32], o: &Options) -> R<Vec<f32>> {
        let audio = Tensor::from_vec(a16.to_vec(), a16.len(), &self.dev).map_err(e)?;
        let feats = self.hubert.features(&audio, 12, self.voice.v1)?; // [1, T50, D]
        let (_, t50, d) = feats.dims3().map_err(e)?;
        // to 10 ms frames: every feature twice
        let feats = feats.unsqueeze(2).map_err(e)?.broadcast_as((1, t50, 2, d)).map_err(e)?.reshape((1, t50 * 2, d)).map_err(e)?;
        let p_len = (a16.len() / 160).min(t50 * 2).min(f0.len());
        let feats = feats.narrow(1, 0, p_len).map_err(e)?.contiguous().map_err(e)?;
        let k = 2f32.powf(o.transpose as f32 / 12.0);
        let f0s: Vec<f32> = f0[..p_len].iter().map(|f| if *f > 0.0 { f * k } else { 0.0 }).collect();
        let pitch = Tensor::from_vec(f0s.iter().map(|f| coarse(*f)).collect::<Vec<u32>>(), p_len, &self.dev).map_err(e)?;
        let (m, logs) = self.voice.text_encoder(&feats, &pitch)?;
        // z_p = m + exp(logs) * noise * 0.66666
        let noise = Tensor::randn(0f32, 1f32, m.dims(), &self.dev).map_err(e)?;
        let z_p = (m + (logs.exp().map_err(e)? * noise).map_err(e)? * 0.66666).map_err(e)?;
        let sid = Tensor::from_vec(vec![o.speaker.min(self.voice.speakers.saturating_sub(1)) as u32], 1, &self.dev).map_err(e)?;
        let g = self.voice.w.get("emb_g.weight")?.index_select(&sid, 0).map_err(e)?.unsqueeze(2).map_err(e)?; // [1, 256, 1]
        let z = self.voice.flow_reverse(&z_p, &g)?;
        self.voice.decode(&z, &f0s, &g, o.seed)
    }

    /// Convert 16 kHz mono audio with its pitch (Hz every 10 ms, 0 = unvoiced) -> audio at `self.voice.sr`. `progress` gets 0..1.
    pub fn convert(&self, a16: &[f32], f0: &[f32], o: &Options, progress: &mut dyn FnMut(f64)) -> R<Vec<f32>> {
        let sr = self.voice.sr as usize;
        let hop_out = sr / 100;
        let win = 16000 * 20; // 20 s windows
        let pad = 16000; // 1 s of context on each side
        let fade = sr / 20; // 50 ms cross-fade
        let total_frames = a16.len() / 160;
        let mut out = vec![0f32; total_frames * hop_out + fade];
        let mut start = 0usize;
        while start < a16.len() {
            let end = (start + win).min(a16.len());
            let a0 = start.saturating_sub(pad);
            let a1 = (end + pad).min(a16.len());
            // reflect at the very ends, as RVC pads its input
            let mut seg: Vec<f32> = a16[a0..a1].to_vec();
            if a0 == 0 {
                let r: Vec<f32> = a16[1..(pad + 1).min(a16.len())].iter().rev().cloned().collect();
                let lead = r.len();
                seg = [r, seg].concat();
                let f0seg = [vec![0.0; lead / 160], f0[(a0 / 160).min(f0.len())..(a1 / 160).min(f0.len())].to_vec()].concat();
                let y = self.convert_window(&seg, &f0seg, o)?;
                self.place(&mut out, &y, (lead / 160) * hop_out, start, end, a0, hop_out, fade);
            } else {
                let f0seg = f0[(a0 / 160).min(f0.len())..(a1 / 160).min(f0.len())].to_vec();
                let y = self.convert_window(&seg, &f0seg, o)?;
                self.place(&mut out, &y, 0, start, end, a0, hop_out, fade);
            }
            progress(end as f64 / a16.len() as f64);
            start = end;
        }
        out.truncate(total_frames * hop_out);
        // follow the input's loudness envelope
        if o.rms_mix < 0.999 {
            let fr_in = 16000 / 20;
            let fr_out = sr / 20;
            let n = out.len() / fr_out;
            for i in 0..n {
                let ai = &a16[(i * fr_in).min(a16.len())..((i + 1) * fr_in).min(a16.len())];
                let ri = (ai.iter().map(|x| x * x).sum::<f32>() / ai.len().max(1) as f32).sqrt();
                let bo = &mut out[i * fr_out..(i + 1) * fr_out];
                let ro = (bo.iter().map(|x| x * x).sum::<f32>() / bo.len().max(1) as f32).sqrt();
                if ro > 1e-6 {
                    let g = (ri.max(1e-6) / ro).powf(1.0 - o.rms_mix).min(8.0);
                    bo.iter_mut().for_each(|x| *x *= g);
                }
            }
        }
        Ok(out)
    }

    /// put the middle of a converted window (without its context) into `out`, cross-fading with what is there
    #[allow(clippy::too_many_arguments)]
    fn place(&self, out: &mut [f32], y: &[f32], lead_out: usize, start: usize, end: usize, a0: usize, hop_out: usize, fade: usize) {
        let ctx = ((start - a0) / 160) * hop_out + lead_out;
        let n = ((end - start) / 160) * hop_out;
        let o0 = (start / 160) * hop_out;
        for k in 0..n + fade {
            let (src, dst) = (ctx + k, o0 + k);
            if src >= y.len() || dst >= out.len() {
                break;
            }
            // fade in over the first `fade` samples (what the previous window left there fades out), and leave a tail to fade into
            let wi = if start == 0 { 1.0 } else { (k as f32 / fade as f32).min(1.0) };
            if k >= n {
                let wo = 1.0 - (k - n) as f32 / fade as f32;
                out[dst] = y[src] * wo;
            } else {
                out[dst] = out[dst] * (1.0 - wi) + y[src] * wi;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coarse_pitch_matches_rvc() {
        assert_eq!(coarse(0.0), 1);
        assert_eq!(coarse(50.0), 1);
        assert_eq!(coarse(1100.0), 255);
        let a = coarse(220.0);
        let b = coarse(440.0);
        assert!(a > 1 && b > a && b < 255, "{a} {b}");
    }

    #[test]
    #[ignore]
    fn loads_the_users_models() {
        // E:/SannyChat/talk (the RVC install on the dev machine): RVC_HUBERT / RVC_MODEL override
        let hub = std::env::var("RVC_HUBERT").unwrap_or("E:/SannyChat/talk/hubert_base.pt".into());
        let model = std::env::var("RVC_MODEL").unwrap_or("E:/SannyChat/talk/weights/LingLin.pth".into());
        if !Path::new(&hub).exists() {
            return;
        }
        let r = Rvc::load(Path::new(&hub), Path::new(&model), -1).unwrap();
        eprintln!("voice: sr {} rates {:?} ups {:?} res {:?} {:?} v1 {} layers {} speakers {}", r.voice.sr, r.voice.rates, r.voice.ups_k, r.voice.res_k, r.voice.res_d, r.voice.v1, r.voice.n_layers, r.voice.speakers);
        assert!(r.voice.sr >= 32000);
        assert_eq!(r.hubert.layers.len(), 12);
    }
    /// the native conversion of the reference input with the reference's pitch (scratch files from the comparison script)
    #[test]
    #[ignore]
    fn converts_like_the_webui() {
        let dir = std::env::var("RVC_CMP").unwrap_or_default();
        if dir.is_empty() {
            return;
        }
        let d = Path::new(&dir);
        let bytes = std::fs::read(d.join("in.wav")).unwrap();
        // a 16 / 32-bit WAV: find "fmt " and "data"
        let find = |tag: &[u8]| (12..bytes.len() - 8).find(|i| &bytes[*i..*i + 4] == tag).unwrap();
        let f = find(b"fmt ");
        let fmt = u16::from_le_bytes([bytes[f + 8], bytes[f + 9]]);
        let ch = u16::from_le_bytes([bytes[f + 10], bytes[f + 11]]) as usize;
        let rate = u32::from_le_bytes(bytes[f + 12..f + 16].try_into().unwrap());
        let bits = u16::from_le_bytes([bytes[f + 22], bytes[f + 23]]);
        let dp = find(b"data");
        let n = u32::from_le_bytes(bytes[dp + 4..dp + 8].try_into().unwrap()) as usize;
        let data = &bytes[dp + 8..dp + 8 + n];
        let s: Vec<f32> = if bits == 32 && (fmt == 3 || fmt == 0xFFFE) { data.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect() } else { data.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect() };
        let mono: Vec<f32> = s.chunks_exact(ch).map(|c| c.iter().sum::<f32>() / ch as f32).collect();
        let a16 = crate::whisper::resample_to_16k(&mono, rate);
        let f0: Vec<f32> = std::fs::read_to_string(d.join("f0.csv")).unwrap().lines().filter_map(|l| l.trim().parse().ok()).collect();
        let hub = "E:/SannyChat/talk/hubert_base.pt";
        let model = "E:/SannyChat/talk/weights/LingLin.pth";
        let t0 = std::time::Instant::now();
        let r = Rvc::load(Path::new(hub), Path::new(model), 0).unwrap();
        let t1 = std::time::Instant::now();
        let out = r.convert(&a16, &f0, &Options { rms_mix: 1.0, ..Options::default() }, &mut |_| {}).unwrap();
        eprintln!("loaded in {:.1} s, converted {:.2} s of audio in {:.2} s on {:?}; {} samples at {}", (t1 - t0).as_secs_f64(), a16.len() as f64 / 16000.0, t1.elapsed().as_secs_f64(), r.dev, out.len(), r.voice.sr);
        let mut w = vec![];
        let sr = r.voice.sr;
        let nb = (out.len() * 2) as u32;
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + nb).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&sr.to_le_bytes());
        w.extend_from_slice(&(sr * 2).to_le_bytes());
        w.extend_from_slice(&2u16.to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&nb.to_le_bytes());
        for x in &out {
            w.extend_from_slice(&((x.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
        }
        std::fs::write(d.join("native_out.wav"), w).unwrap();
        assert!(out.iter().all(|x| x.is_finite()));
        assert!(out.iter().any(|x| x.abs() > 0.02), "not silent");
    }

}
