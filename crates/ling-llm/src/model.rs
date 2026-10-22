//! The Qwen3 transformer: per layer RMSNorm, q / k / v projections, a per-head RMSNorm on q and k, NeoX-style rotary embeddings,
//! grouped-query attention over an f16 KV cache, an output projection, then a SwiGLU feed-forward block; a last RMSNorm and the logits
//! (the output matrix, or the embedding table again when the model ties them).
//!
//! `forward` takes a batch of consecutive tokens: for prompt processing every weight row is read once per batch, for generation the
//! batch is one token.
use crate::error::Error;
use crate::f16;
use crate::gguf::{Gguf, Source};
use crate::m;
use crate::quant::{dot_f32, matmul, QTensor, XBatch};
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

/// the most tokens one `forward` call takes
pub const MAX_BATCH: usize = 32;

#[derive(Clone, Debug)]
pub struct Config {
    pub n_layer: usize,
    pub n_embd: usize,
    pub n_ff: usize,
    pub n_head: usize,
    pub n_kv: usize,
    pub head_dim: usize,
    pub n_ctx: usize,
    pub vocab: usize,
    pub rope_base: f32,
    pub eps: f32,
}

impl Config {
    pub fn from_gguf(g: &Gguf) -> Result<Config, Error> {
        let arch = g.get_str("general.architecture").ok_or_else(|| Error::Missing("general.architecture".to_string()))?;
        if arch != "qwen3" {
            return Err(Error::Unsupported(alloc::format!("architecture '{arch}' (this engine runs qwen3)")));
        }
        let need = |k: &str| -> Result<u64, Error> { g.get_u64(&alloc::format!("{arch}.{k}")).ok_or_else(|| Error::Missing(alloc::format!("{arch}.{k}"))) };
        let n_embd = need("embedding_length")? as usize;
        let n_head = need("attention.head_count")? as usize;
        let n_kv = g.get_u64(&alloc::format!("{arch}.attention.head_count_kv")).unwrap_or(n_head as u64) as usize;
        let head_dim = g.get_u64(&alloc::format!("{arch}.attention.key_length")).map(|x| x as usize).unwrap_or(n_embd / n_head.max(1));
        let vocab = match g.get("tokenizer.ggml.tokens") {
            Some(crate::gguf::Value::Arr(a)) => a.len(),
            _ => g.tensor("token_embd.weight").map(|t| t.shape2().0).unwrap_or(0),
        };
        let cfg = Config {
            n_layer: need("block_count")? as usize,
            n_embd,
            n_ff: need("feed_forward_length")? as usize,
            n_head,
            n_kv,
            head_dim,
            n_ctx: g.get_u64(&alloc::format!("{arch}.context_length")).unwrap_or(4096) as usize,
            vocab,
            rope_base: g.get_f32(&alloc::format!("{arch}.rope.freq_base")).unwrap_or(1_000_000.0),
            eps: g.get_f32(&alloc::format!("{arch}.attention.layer_norm_rms_epsilon")).unwrap_or(1e-6),
        };
        if cfg.n_head == 0 || cfg.n_kv == 0 || cfg.n_head % cfg.n_kv != 0 || cfg.head_dim % 2 != 0 || cfg.vocab == 0 {
            return Err(Error::Format("the model's shape is not one this engine can run"));
        }
        Ok(cfg)
    }
    fn q_dim(&self) -> usize {
        self.n_head * self.head_dim
    }
    fn kv_dim(&self) -> usize {
        self.n_kv * self.head_dim
    }
}

struct Layer {
    attn_norm: Vec<f32>,
    wq: QTensor,
    wk: QTensor,
    wv: QTensor,
    wo: QTensor,
    q_norm: Vec<f32>,
    k_norm: Vec<f32>,
    ffn_norm: Vec<f32>,
    wg: QTensor,
    wu: QTensor,
    wd: QTensor,
}

pub struct Model {
    pub cfg: Config,
    embd: QTensor,
    out_norm: Vec<f32>,
    output: Option<QTensor>,
    layers: Vec<Layer>,
    inv_freq: Vec<f32>,
    lut: Vec<f32>,
}

/// everything that changes while the model runs: the KV cache and the scratch buffers
pub struct State {
    /// how many tokens are in the cache
    pub pos: usize,
    pub max_ctx: usize,
    kc: Vec<Vec<u16>>,
    vc: Vec<Vec<u16>>,
    xb: XBatch,
    hb: XBatch,
    ab: XBatch,
    fb: XBatch,
    q: Vec<f32>,
    k: Vec<f32>,
    v: Vec<f32>,
    gate: Vec<f32>,
    up: Vec<f32>,
    proj: Vec<f32>,
    scores: Vec<f32>,
    pub logits: Vec<f32>,
}

impl State {
    /// forget the conversation (the cache is simply overwritten as new tokens come in)
    pub fn reset(&mut self) {
        self.pos = 0;
    }
}

fn rmsnorm(x: &[f32], w: &[f32], eps: f32, out: &mut [f32]) {
    let ss = dot_f32(x, x);
    let inv = 1.0 / m::sqrt(ss / x.len() as f32 + eps);
    for i in 0..x.len() {
        out[i] = x[i] * inv * w[i];
    }
}

fn rmsnorm_inplace(x: &mut [f32], w: &[f32], eps: f32) {
    let ss = dot_f32(x, x);
    let inv = 1.0 / m::sqrt(ss / x.len() as f32 + eps);
    for i in 0..x.len() {
        x[i] = x[i] * inv * w[i];
    }
}

/// NeoX rotary embedding: the pairs are (i, i + head_dim / 2)
fn rope(v: &mut [f32], pos: usize, inv_freq: &[f32]) {
    let half = v.len() / 2;
    for i in 0..half {
        let a = pos as f32 * inv_freq[i];
        let (s, c) = (m::sin(a), m::cos(a));
        let (x0, x1) = (v[i], v[i + half]);
        v[i] = x0 * c - x1 * s;
        v[i + half] = x0 * s + x1 * c;
    }
}

fn load_f32(src: &dyn Source, g: &Gguf, name: &str, n: usize) -> Result<Vec<f32>, Error> {
    let t = g.tensor(name).ok_or_else(|| Error::Missing(name.to_string()))?;
    let q = QTensor::load(src, g.data_start + t.offset, 1, t.shape2().1, t.dtype_or_err()?)?;
    if q.cols != n {
        return Err(Error::Format("a norm vector has the wrong length"));
    }
    let mut v = vec![0.0f32; n];
    q.dequant_row(0, &mut v);
    Ok(v)
}

fn load_mat(src: &dyn Source, g: &Gguf, name: &str, rows: usize, cols: usize) -> Result<QTensor, Error> {
    let t = g.tensor(name).ok_or_else(|| Error::Missing(name.to_string()))?;
    let (r, c) = t.shape2();
    if r != rows || c != cols {
        return Err(Error::Format("a weight matrix has an unexpected shape"));
    }
    QTensor::load(src, g.data_start + t.offset, r, c, t.dtype_or_err()?)
}

impl Model {
    /// Read the weights. `progress(done, total)` is called after each tensor group (a loading bar; loading a model takes a while).
    pub fn load(src: &dyn Source, g: &Gguf, progress: &mut dyn FnMut(usize, usize)) -> Result<Model, Error> {
        let cfg = Config::from_gguf(g)?;
        let (e, qd, kd, ff) = (cfg.n_embd, cfg.q_dim(), cfg.kv_dim(), cfg.n_ff);
        let total = cfg.n_layer + 2;
        let embd = load_mat(src, g, "token_embd.weight", cfg.vocab, e)?;
        let out_norm = load_f32(src, g, "output_norm.weight", e)?;
        let output = if g.tensor("output.weight").is_some() { Some(load_mat(src, g, "output.weight", cfg.vocab, e)?) } else { None };
        progress(1, total);
        let mut layers = Vec::with_capacity(cfg.n_layer);
        for l in 0..cfg.n_layer {
            let nm = |s: &str| alloc::format!("blk.{l}.{s}");
            layers.push(Layer {
                attn_norm: load_f32(src, g, &nm("attn_norm.weight"), e)?,
                wq: load_mat(src, g, &nm("attn_q.weight"), qd, e)?,
                wk: load_mat(src, g, &nm("attn_k.weight"), kd, e)?,
                wv: load_mat(src, g, &nm("attn_v.weight"), kd, e)?,
                wo: load_mat(src, g, &nm("attn_output.weight"), e, qd)?,
                q_norm: load_f32(src, g, &nm("attn_q_norm.weight"), cfg.head_dim)?,
                k_norm: load_f32(src, g, &nm("attn_k_norm.weight"), cfg.head_dim)?,
                ffn_norm: load_f32(src, g, &nm("ffn_norm.weight"), e)?,
                wg: load_mat(src, g, &nm("ffn_gate.weight"), ff, e)?,
                wu: load_mat(src, g, &nm("ffn_up.weight"), ff, e)?,
                wd: load_mat(src, g, &nm("ffn_down.weight"), e, ff)?,
            });
            progress(2 + l, total);
        }
        let inv_freq = (0..cfg.head_dim / 2).map(|i| m::powf(cfg.rope_base, -(2.0 * i as f32) / cfg.head_dim as f32)).collect();
        progress(total, total);
        Ok(Model { cfg, embd, out_norm, output, layers, inv_freq, lut: f16::table() })
    }

    /// bytes of weights held
    pub fn weight_bytes(&self) -> usize {
        self.embd.bytes()
            + self.output.as_ref().map_or(0, |o| o.bytes())
            + self.layers.iter().map(|l| l.wq.bytes() + l.wk.bytes() + l.wv.bytes() + l.wo.bytes() + l.wg.bytes() + l.wu.bytes() + l.wd.bytes()).sum::<usize>()
    }

    /// a fresh KV cache and scratch space for up to `max_ctx` tokens (at most the model's context length)
    pub fn new_state(&self, max_ctx: usize) -> State {
        let c = &self.cfg;
        let max_ctx = max_ctx.clamp(1, c.n_ctx.max(1));
        let kv = c.kv_dim();
        State {
            pos: 0,
            max_ctx,
            kc: (0..c.n_layer).map(|_| vec![0u16; max_ctx * kv]).collect(),
            vc: (0..c.n_layer).map(|_| vec![0u16; max_ctx * kv]).collect(),
            xb: XBatch::new(MAX_BATCH, c.n_embd),
            hb: XBatch::new(MAX_BATCH, c.n_embd),
            ab: XBatch::new(MAX_BATCH, c.q_dim()),
            fb: XBatch::new(MAX_BATCH, c.n_ff),
            q: vec![0.0; MAX_BATCH * c.q_dim()],
            k: vec![0.0; MAX_BATCH * kv],
            v: vec![0.0; MAX_BATCH * kv],
            gate: vec![0.0; MAX_BATCH * c.n_ff],
            up: vec![0.0; MAX_BATCH * c.n_ff],
            proj: vec![0.0; MAX_BATCH * c.n_embd],
            scores: vec![0.0; max_ctx],
            logits: vec![0.0; c.vocab],
        }
    }

    /// Run `tokens` (consecutive positions starting at `st.pos`). With `want_logits` the logits of the LAST token are left in `st.logits`.
    pub fn forward(&self, st: &mut State, tokens: &[u32], want_logits: bool) -> Result<(), Error> {
        let c = &self.cfg;
        let b = tokens.len();
        if b == 0 || b > MAX_BATCH {
            return Err(Error::Format("a batch is 1 to 32 tokens"));
        }
        if st.pos + b > st.max_ctx {
            return Err(Error::Format("the context is full"));
        }
        let (e, hd, nh, nkv) = (c.n_embd, c.head_dim, c.n_head, c.n_kv);
        let (qd, kd, ff) = (c.q_dim(), c.kv_dim(), c.n_ff);
        let group = nh / nkv;
        let scale = 1.0 / m::sqrt(hd as f32);
        for (t, tok) in tokens.iter().enumerate() {
            if *tok as usize >= c.vocab {
                return Err(Error::Format("a token id is outside the vocabulary"));
            }
            self.embd.dequant_row(*tok as usize, st.xb.vec_mut(t));
        }
        st.xb.set_len(b);
        for (li, layer) in self.layers.iter().enumerate() {
            // ── attention ──
            for t in 0..b {
                let (x, h) = (&st.xb.x[t * e..(t + 1) * e], &mut st.hb.x[t * e..(t + 1) * e]);
                rmsnorm(x, &layer.attn_norm, c.eps, h);
            }
            st.hb.set_len(b);
            matmul(&layer.wq, &mut st.hb, &mut st.q);
            matmul(&layer.wk, &mut st.hb, &mut st.k);
            matmul(&layer.wv, &mut st.hb, &mut st.v);
            for t in 0..b {
                let pos = st.pos + t;
                for h in 0..nh {
                    let qh = &mut st.q[(t * nh + h) * hd..(t * nh + h + 1) * hd];
                    rmsnorm_inplace(qh, &layer.q_norm, c.eps);
                    rope(qh, pos, &self.inv_freq);
                }
                for h in 0..nkv {
                    let kh = &mut st.k[(t * nkv + h) * hd..(t * nkv + h + 1) * hd];
                    rmsnorm_inplace(kh, &layer.k_norm, c.eps);
                    rope(kh, pos, &self.inv_freq);
                }
                // into the cache as halves
                let base = pos * kd;
                for i in 0..kd {
                    st.kc[li][base + i] = f16::from_f32(st.k[t * kd + i]);
                    st.vc[li][base + i] = f16::from_f32(st.v[t * kd + i]);
                }
                for h in 0..nh {
                    let kvh = h / group;
                    let qh = &st.q[(t * nh + h) * hd..(t * nh + h + 1) * hd];
                    let mut maxs = f32::NEG_INFINITY;
                    for s in 0..=pos {
                        let kk = &st.kc[li][s * kd + kvh * hd..s * kd + (kvh + 1) * hd];
                        let mut acc = 0.0f32;
                        for i in 0..hd {
                            acc += qh[i] * self.lut[kk[i] as usize];
                        }
                        let sc = acc * scale;
                        st.scores[s] = sc;
                        if sc > maxs {
                            maxs = sc;
                        }
                    }
                    let mut sum = 0.0f32;
                    for s in 0..=pos {
                        let p = m::exp(st.scores[s] - maxs);
                        st.scores[s] = p;
                        sum += p;
                    }
                    let inv = 1.0 / sum;
                    let out = &mut st.ab.x[t * qd + h * hd..t * qd + (h + 1) * hd];
                    out.iter_mut().for_each(|o| *o = 0.0);
                    for s in 0..=pos {
                        let p = st.scores[s] * inv;
                        let vv = &st.vc[li][s * kd + kvh * hd..s * kd + (kvh + 1) * hd];
                        for i in 0..hd {
                            out[i] += p * self.lut[vv[i] as usize];
                        }
                    }
                }
            }
            st.ab.set_len(b);
            matmul(&layer.wo, &mut st.ab, &mut st.proj);
            for i in 0..b * e {
                st.xb.x[i] += st.proj[i];
            }
            // ── feed-forward ──
            for t in 0..b {
                let (x, h) = (&st.xb.x[t * e..(t + 1) * e], &mut st.hb.x[t * e..(t + 1) * e]);
                rmsnorm(x, &layer.ffn_norm, c.eps, h);
            }
            st.hb.set_len(b);
            matmul(&layer.wg, &mut st.hb, &mut st.gate);
            matmul(&layer.wu, &mut st.hb, &mut st.up);
            for i in 0..b * ff {
                let g = st.gate[i];
                st.fb.x[i] = g / (1.0 + m::exp(-g)) * st.up[i];
            }
            st.fb.set_len(b);
            matmul(&layer.wd, &mut st.fb, &mut st.proj);
            for i in 0..b * e {
                st.xb.x[i] += st.proj[i];
            }
        }
        st.pos += b;
        if want_logits {
            let last = b - 1;
            let (x, h) = (&st.xb.x[last * e..(last + 1) * e], &mut st.hb.x[0..e]);
            rmsnorm(x, &self.out_norm, c.eps, h);
            st.hb.set_len(1);
            matmul(self.output.as_ref().unwrap_or(&self.embd), &mut st.hb, &mut st.logits);
        }
        Ok(())
    }
}

/// a printable summary for logs
pub fn describe(cfg: &Config) -> String {
    alloc::format!("{} layers, {} wide, {}/{} heads, {} feed-forward, context {}, vocabulary {}", cfg.n_layer, cfg.n_embd, cfg.n_head, cfg.n_kv, cfg.n_ff, cfg.n_ctx, cfg.vocab)
}
