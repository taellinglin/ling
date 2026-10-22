//! Speech to text with Whisper, in Rust, on candle (CPU or CUDA): no Python, no server.
//!
//! It loads the CTranslate2 models faster-whisper uses (`model.bin` + `tokenizer.json` + `config.json`, as found in the Hugging Face cache
//! after `huggingface-cli download Systran/faster-whisper-small`), so the same weights serve both. The fused attention projections are split
//! into candle's layout when the model is loaded.
//!
//! The decoding follows OpenAI's Whisper and faster-whisper: 30 s windows, language detection, timestamp tokens (with their pairing and
//! monotonic rules), temperature fallback on a failed window, no-speech skipping, and word timestamps from the cross-attention of the
//! model's alignment heads (median filter, DTW). Long audio is cut in windows that move on to where the last segment ended.
#![cfg(feature = "llm")]

use candle_core::{DType, Device, IndexOp, Module, Tensor};
use candle_nn::{Conv1d, Conv1dConfig, Embedding, LayerNorm, Linear};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

type R<T> = Result<T, String>;
fn e<T: std::fmt::Display>(x: T) -> String {
    x.to_string()
}

pub const SAMPLE_RATE: usize = 16000;
const N_FFT: usize = 400;
const HOP: usize = 160;
const CHUNK: usize = 30;
const N_SAMPLES: usize = CHUNK * SAMPLE_RATE;
const N_FRAMES: usize = N_SAMPLES / HOP;
const TOKENS_PER_SECOND: f64 = 50.0;

// ── CTranslate2 model.bin ────────────────────────────────────────────────────

struct Var {
    dims: Vec<usize>,
    dtype: u8,
    data: Vec<u8>,
}

struct Reader<'a> {
    d: &'a [u8],
    p: usize,
}
impl Reader<'_> {
    fn take(&mut self, n: usize) -> R<&[u8]> {
        if self.p + n > self.d.len() {
            return Err("model.bin ends early".into());
        }
        let s = &self.d[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }
    fn u8(&mut self) -> R<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> R<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> R<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn string(&mut self) -> R<String> {
        let n = self.u16()? as usize;
        Ok(String::from_utf8_lossy(self.take(n)?).trim_end_matches('\0').to_string())
    }
}

fn read_ct2(path: &Path) -> R<HashMap<String, Var>> {
    let data = std::fs::read(path).map_err(|x| format!("{}: {x}", path.display()))?;
    let mut r = Reader { d: &data, p: 0 };
    let version = r.u32()?;
    if !(5..=7).contains(&version) {
        return Err(format!("unsupported CTranslate2 model version {version}"));
    }
    let spec = r.string()?;
    if !spec.contains("Whisper") {
        return Err(format!("{spec} is not a Whisper model"));
    }
    let _rev = r.u32()?;
    let n = r.u32()? as usize;
    let mut vars = HashMap::new();
    for _ in 0..n {
        let name = r.string()?;
        let rank = r.u8()? as usize;
        let dims: Vec<usize> = (0..rank).map(|_| r.u32().map(|x| x as usize)).collect::<R<_>>()?;
        let dtype = r.u8()?;
        let size = r.u32()? as usize;
        let bytes = r.take(size)?.to_vec();
        vars.insert(name, Var { dims, dtype, data: bytes });
    }
    // aliases (the output projection shares the embeddings)
    if let Ok(na) = r.u32() {
        for _ in 0..na {
            let (a, b) = (r.string()?, r.string()?);
            if let Some(v) = vars.get(&b) {
                let copy = Var { dims: v.dims.clone(), dtype: v.dtype, data: v.data.clone() };
                vars.entry(a).or_insert(copy);
            }
        }
    }
    Ok(vars)
}

fn tensor(v: &Var, dev: &Device) -> R<Tensor> {
    let dt = match v.dtype {
        0 => DType::F32,
        4 => DType::F16,
        5 => DType::BF16,
        d => return Err(format!("weights of type {d} (quantised models) are not supported: use a float16 / float32 model")),
    };
    Tensor::from_raw_buffer(&v.data, dt, &v.dims, dev).and_then(|t| t.to_dtype(DType::F32)).map_err(e)
}

fn scalar_i16(v: &Var) -> usize {
    if v.data.len() >= 2 {
        i16::from_le_bytes([v.data[0], v.data[1]]).max(0) as usize
    } else {
        0
    }
}

// ── the model (candle's Whisper, with the cross-attention exposed) ───────────

struct Attn {
    q: Linear,
    k: Linear,
    v: Linear,
    o: Linear,
    heads: usize,
    kv: Option<(Tensor, Tensor)>,
}

impl Attn {
    /// (output, the scaled q·k before the softmax when `capture`)
    fn forward(&mut self, x: &Tensor, xa: Option<&Tensor>, mask: Option<&Tensor>, flush: bool, capture: bool) -> R<(Tensor, Option<Tensor>)> {
        let q = self.q.forward(x).map_err(e)?;
        let (k, v) = match xa {
            None => (self.k.forward(x).map_err(e)?, self.v.forward(x).map_err(e)?),
            Some(xa) => {
                if flush {
                    self.kv = None;
                }
                if let Some((k, v)) = &self.kv {
                    (k.clone(), v.clone())
                } else {
                    let (k, v) = (self.k.forward(xa).map_err(e)?, self.v.forward(xa).map_err(e)?);
                    self.kv = Some((k.clone(), v.clone()));
                    (k, v)
                }
            }
        };
        let (_, n_ctx, n_state) = q.dims3().map_err(e)?;
        let scale = ((n_state / self.heads) as f64).powf(-0.25);
        let split = |t: &Tensor| -> R<Tensor> {
            let (b, n, s) = t.dims3().map_err(e)?;
            t.reshape(&[b, n, self.heads, s / self.heads]).and_then(|t| t.transpose(1, 2)).map_err(e)
        };
        let q = (split(&q)? * scale).map_err(e)?;
        let k = (split(&k)?.transpose(2, 3).map_err(e)? * scale).map_err(e)?;
        let v = split(&v)?.contiguous().map_err(e)?;
        let mut qk = q.matmul(&k).map_err(e)?;
        if let Some(m) = mask {
            let m = m.i((0..n_ctx, 0..n_ctx)).map_err(e)?;
            qk = qk.broadcast_add(&m).map_err(e)?;
        }
        let cap = if capture { Some(qk.clone()) } else { None };
        let w = candle_nn::ops::softmax_last_dim(&qk).map_err(e)?;
        let wv = w.matmul(&v).map_err(e)?.transpose(1, 2).and_then(|t| t.flatten_from(2)).map_err(e)?;
        Ok((self.o.forward(&wv).map_err(e)?, cap))
    }
}

struct Block {
    attn: Attn,
    attn_ln: LayerNorm,
    cross: Option<(Attn, LayerNorm)>,
    fc1: Linear,
    fc2: Linear,
    mlp_ln: LayerNorm,
}

impl Block {
    fn forward(&mut self, x: &Tensor, xa: Option<&Tensor>, mask: Option<&Tensor>, flush: bool, capture: bool) -> R<(Tensor, Option<Tensor>)> {
        let (a, _) = self.attn.forward(&self.attn_ln.forward(x).map_err(e)?, None, mask, flush, false)?;
        let mut x = (x + a).map_err(e)?;
        let mut cap = None;
        if let Some((ca, ln)) = &mut self.cross {
            let (c, qk) = ca.forward(&ln.forward(&x).map_err(e)?, xa, None, flush, capture)?;
            x = (&x + c).map_err(e)?;
            cap = qk;
        }
        let m = self.fc2.forward(&self.fc1.forward(&self.mlp_ln.forward(&x).map_err(e)?).and_then(|t| t.gelu()).map_err(e)?).map_err(e)?;
        Ok(((x + m).map_err(e)?, cap))
    }
}

fn sinusoids(length: usize, channels: usize, dev: &Device) -> R<Tensor> {
    let inc = (10000f32).ln() / (channels / 2 - 1) as f32;
    let mut v = vec![0f32; length * channels];
    for t in 0..length {
        for i in 0..channels / 2 {
            let a = t as f32 * (-(i as f32) * inc).exp();
            v[t * channels + i] = a.sin();
            v[t * channels + channels / 2 + i] = a.cos();
        }
    }
    Tensor::from_vec(v, (length, channels), dev).map_err(e)
}

pub struct Model {
    conv1: Conv1d,
    conv2: Conv1d,
    enc_pos: Tensor,
    enc: Vec<Block>,
    enc_ln: LayerNorm,
    embed: Embedding,
    dec_pos: Tensor,
    dec: Vec<Block>,
    dec_ln: LayerNorm,
    dev: Device,
    pub n_mels: usize,
    pub vocab: usize,
}

impl Model {
    fn encode(&mut self, mel: &Tensor) -> R<Tensor> {
        let x = self.conv1.forward(mel).and_then(|t| t.gelu()).map_err(e)?;
        let x = self.conv2.forward(&x).and_then(|t| t.gelu()).map_err(e)?;
        let x = x.transpose(1, 2).map_err(e)?;
        let (_, seq, _) = x.dims3().map_err(e)?;
        let pos = self.enc_pos.narrow(0, 0, seq).map_err(e)?;
        let mut x = x.broadcast_add(&pos).map_err(e)?;
        for b in self.enc.iter_mut() {
            x = b.forward(&x, None, None, true, false)?.0;
        }
        self.enc_ln.forward(&x).map_err(e)
    }

    /// logits for every position, and (when capturing) the pre-softmax cross-attention of every decoder layer
    fn decode(&mut self, tokens: &[u32], xa: &Tensor, flush: bool, capture: bool) -> R<(Tensor, Vec<Tensor>)> {
        let n = tokens.len();
        let t = Tensor::new(tokens, &self.dev).and_then(|t| t.unsqueeze(0)).map_err(e)?;
        let te = self.embed.forward(&t).map_err(e)?;
        let pos = self.dec_pos.narrow(0, 0, n).map_err(e)?;
        let mut x = te.broadcast_add(&pos).map_err(e)?;
        let mask: Vec<f32> = (0..n).flat_map(|i| (0..n).map(move |j| if j > i { f32::NEG_INFINITY } else { 0.0 })).collect();
        let mask = Tensor::from_vec(mask, (n, n), &self.dev).map_err(e)?;
        let mut caps = vec![];
        for b in self.dec.iter_mut() {
            let (y, c) = b.forward(&x, Some(xa), Some(&mask), flush, capture)?;
            x = y;
            if let Some(c) = c {
                caps.push(c);
            }
        }
        let x = self.dec_ln.forward(&x).map_err(e)?;
        let w = self.embed.embeddings().t().map_err(e)?;
        let logits = x.broadcast_matmul(&w).map_err(e)?;
        Ok((logits, caps))
    }
}

fn load_model(vars: &HashMap<String, Var>, dev: &Device) -> R<Model> {
    let get = |name: &str| -> R<Tensor> { tensor(vars.get(name).ok_or_else(|| format!("missing weight {name}"))?, dev) };
    let conv = |name: &str, stride: usize| -> R<Conv1d> {
        Ok(Conv1d::new(get(&format!("{name}/weight"))?, Some(get(&format!("{name}/bias"))?), Conv1dConfig { padding: 1, stride, groups: 1, dilation: 1, cudnn_fwd_algo: None }))
    };
    let ln = |name: &str| -> R<LayerNorm> { Ok(LayerNorm::new(get(&format!("{name}/gamma"))?, get(&format!("{name}/beta"))?, 1e-5)) };
    let lin = |name: &str| -> R<Linear> { Ok(Linear::new(get(&format!("{name}/weight"))?, Some(get(&format!("{name}/bias"))?))) };
    // a fused projection (rows q|k|v or k|v) split into separate linears
    let split = |name: &str, parts: usize| -> R<Vec<Linear>> {
        let (w, b) = (get(&format!("{name}/weight"))?, get(&format!("{name}/bias"))?);
        let rows = w.dim(0).map_err(e)? / parts;
        (0..parts).map(|i| Ok(Linear::new(w.narrow(0, i * rows, rows).map_err(e)?, Some(b.narrow(0, i * rows, rows).map_err(e)?)))).collect()
    };
    let enc_heads = vars.get("encoder/num_heads").map(scalar_i16).filter(|h| *h > 0);
    let dec_heads = vars.get("decoder/num_heads").map(scalar_i16).filter(|h| *h > 0);
    let conv1_w = vars.get("encoder/conv1/weight").ok_or("not a Whisper model: no encoder/conv1")?;
    let (d_model, n_mels) = (conv1_w.dims[0], conv1_w.dims[1]);
    let enc_layers = (0..).take_while(|i| vars.contains_key(&format!("encoder/layer_{i}/ffn/linear_0/weight"))).count();
    let dec_layers = (0..).take_while(|i| vars.contains_key(&format!("decoder/layer_{i}/ffn/linear_0/weight"))).count();
    let (eh, dh) = (enc_heads.unwrap_or(d_model / 64), dec_heads.unwrap_or(d_model / 64));
    let block = |prefix: &str, heads: usize, cross: bool| -> R<Block> {
        let qkv = split(&format!("{prefix}/self_attention/linear_0"), 3)?;
        let mut it = qkv.into_iter();
        let attn = Attn { q: it.next().unwrap(), k: it.next().unwrap(), v: it.next().unwrap(), o: lin(&format!("{prefix}/self_attention/linear_1"))?, heads, kv: None };
        let cross = if cross {
            let q = lin(&format!("{prefix}/attention/linear_0"))?;
            let mut kv = split(&format!("{prefix}/attention/linear_1"), 2)?.into_iter();
            let ca = Attn { q, k: kv.next().unwrap(), v: kv.next().unwrap(), o: lin(&format!("{prefix}/attention/linear_2"))?, heads, kv: None };
            Some((ca, ln(&format!("{prefix}/attention/layer_norm"))?))
        } else {
            None
        };
        Ok(Block { attn, attn_ln: ln(&format!("{prefix}/self_attention/layer_norm"))?, cross, fc1: lin(&format!("{prefix}/ffn/linear_0"))?, fc2: lin(&format!("{prefix}/ffn/linear_1"))?, mlp_ln: ln(&format!("{prefix}/ffn/layer_norm"))? })
    };
    let enc = (0..enc_layers).map(|i| block(&format!("encoder/layer_{i}"), eh, false)).collect::<R<Vec<_>>>()?;
    let dec = (0..dec_layers).map(|i| block(&format!("decoder/layer_{i}"), dh, true)).collect::<R<Vec<_>>>()?;
    let emb = get("decoder/embeddings/weight")?;
    let vocab = emb.dim(0).map_err(e)?;
    Ok(Model {
        conv1: conv("encoder/conv1", 1)?,
        conv2: conv("encoder/conv2", 2)?,
        enc_pos: sinusoids(1500, d_model, dev)?,
        enc,
        enc_ln: ln("encoder/layer_norm")?,
        embed: Embedding::new(emb, d_model),
        dec_pos: get("decoder/position_encodings/encodings")?,
        dec,
        dec_ln: ln("decoder/layer_norm")?,
        dev: dev.clone(),
        n_mels,
        vocab,
    })
}

// ── audio ────────────────────────────────────────────────────────────────────

/// mel filter bank (librosa's Slaney scale), `n_mels` x (N_FFT / 2 + 1), row by row
fn mel_filters(n_mels: usize) -> Vec<f32> {
    let hz_to_mel = |f: f64| -> f64 {
        let (f_sp, min_log_hz) = (200.0 / 3.0, 1000.0);
        let (min_log_mel, logstep) = (min_log_hz / f_sp, (6.4f64).ln() / 27.0);
        if f >= min_log_hz { min_log_mel + (f / min_log_hz).ln() / logstep } else { f / f_sp }
    };
    let mel_to_hz = |m: f64| -> f64 {
        let (f_sp, min_log_hz) = (200.0 / 3.0, 1000.0);
        let (min_log_mel, logstep) = (min_log_hz / f_sp, (6.4f64).ln() / 27.0);
        if m >= min_log_mel { min_log_hz * (logstep * (m - min_log_mel)).exp() } else { m * f_sp }
    };
    let n_freq = N_FFT / 2 + 1;
    let (lo, hi) = (hz_to_mel(0.0), hz_to_mel(SAMPLE_RATE as f64 / 2.0));
    let mel_f: Vec<f64> = (0..n_mels + 2).map(|i| mel_to_hz(lo + (hi - lo) * i as f64 / (n_mels + 1) as f64)).collect();
    let fft: Vec<f64> = (0..n_freq).map(|k| k as f64 * SAMPLE_RATE as f64 / N_FFT as f64).collect();
    let mut out = vec![0f32; n_mels * n_freq];
    for i in 0..n_mels {
        let (f0, f1, f2) = (mel_f[i], mel_f[i + 1], mel_f[i + 2]);
        let enorm = 2.0 / (f2 - f0);
        for (k, f) in fft.iter().enumerate() {
            let lower = (f - f0) / (f1 - f0);
            let upper = (f2 - f) / (f2 - f1);
            out[i * n_freq + k] = (lower.min(upper).max(0.0) * enorm) as f32;
        }
    }
    out
}

/// log-mel spectrogram of 16 kHz mono samples as (n_mels, frames), with the 30 s of silence Whisper appends
fn log_mel(samples: &[f32], n_mels: usize, filters: &[f32]) -> Vec<f32> {
    use rustfft_free::fft_real_power;
    let mut padded = samples.to_vec();
    padded.extend(std::iter::repeat(0.0).take(N_SAMPLES));
    let n_freq = N_FFT / 2 + 1;
    // reflect padding of N_FFT / 2 on both sides, a periodic Hann window
    let pad = N_FFT / 2;
    let mut x = Vec::with_capacity(padded.len() + 2 * pad);
    for i in (1..=pad).rev() {
        x.push(padded[i.min(padded.len() - 1)]);
    }
    x.extend_from_slice(&padded);
    for i in 0..pad {
        x.push(padded[padded.len().saturating_sub(2 + i)]);
    }
    let window: Vec<f32> = (0..N_FFT).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / N_FFT as f32).cos()).collect();
    let frames = (x.len() - N_FFT) / HOP; // librosa drops the last frame
    let mut mel = vec![0f32; n_mels * frames];
    let mut buf = vec![0f32; N_FFT];
    let mut power = vec![0f32; n_freq];
    for t in 0..frames {
        for i in 0..N_FFT {
            buf[i] = x[t * HOP + i] * window[i];
        }
        fft_real_power(&buf, &mut power);
        for m in 0..n_mels {
            let row = &filters[m * n_freq..(m + 1) * n_freq];
            let mut s = 0f32;
            for k in 0..n_freq {
                s += row[k] * power[k];
            }
            mel[m * frames + t] = s.max(1e-10).log10();
        }
    }
    let mx = mel.iter().cloned().fold(f32::MIN, f32::max);
    for v in mel.iter_mut() {
        *v = (v.max(mx - 8.0) + 4.0) / 4.0;
    }
    mel
}

/// a small radix-2 / Bluestein-free real FFT for N_FFT = 400 (= 16 * 25): a mixed split into a direct DFT of 25 points and a radix-2 FFT of 16
mod rustfft_free {
    pub fn fft_real_power(x: &[f32], out: &mut [f32]) {
        let n = x.len();
        let k_max = n / 2 + 1;
        // a straightforward DFT with a precomputed twiddle table, vectorisable and fast enough for 400 points
        thread_local! {
            static TABLE: std::cell::RefCell<(usize, Vec<f32>, Vec<f32>)> = std::cell::RefCell::new((0, vec![], vec![]));
        }
        TABLE.with(|t| {
            let mut t = t.borrow_mut();
            if t.0 != n {
                let (mut c, mut s) = (vec![0f32; n], vec![0f32; n]);
                for i in 0..n {
                    let a = 2.0 * std::f64::consts::PI * i as f64 / n as f64;
                    c[i] = a.cos() as f32;
                    s[i] = -a.sin() as f32;
                }
                *t = (n, c, s);
            }
            let (_, c, s) = &*t;
            for k in 0..k_max {
                let (mut re, mut im) = (0f32, 0f32);
                let mut idx = 0usize;
                for &v in x.iter() {
                    re += v * c[idx];
                    im += v * s[idx];
                    idx += k;
                    if idx >= n {
                        idx -= n;
                    }
                }
                out[k] = re * re + im * im;
            }
        });
    }
}

/// resample mono audio to 16 kHz (windowed sinc)
pub fn resample_to_16k(samples: &[f32], rate: u32) -> Vec<f32> {
    if rate as usize == SAMPLE_RATE {
        return samples.to_vec();
    }
    let ratio = rate as f64 / SAMPLE_RATE as f64;
    let n_out = (samples.len() as f64 / ratio) as usize;
    let cutoff = (1.0 / ratio).min(1.0) * 0.97;
    let half = 16.0 / cutoff.max(0.25);
    let mut out = Vec::with_capacity(n_out);
    for i in 0..n_out {
        let c = i as f64 * ratio;
        let (a, b) = (((c - half).ceil().max(0.0)) as usize, ((c + half).floor() as usize).min(samples.len().saturating_sub(1)));
        let mut acc = 0f64;
        let mut wsum = 0f64;
        for j in a..=b {
            let x = (j as f64 - c) * cutoff;
            let sinc = if x.abs() < 1e-9 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
            let w = 0.5 + 0.5 * (std::f64::consts::PI * (j as f64 - c) / half).cos();
            let k = sinc * w * cutoff;
            acc += samples[j] as f64 * k;
            wsum += k;
        }
        out.push(if wsum.abs() > 1e-9 { (acc / wsum) as f32 } else { 0.0 });
    }
    out
}

// ── the transcriber ──────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Word {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub prob: f64,
}

#[derive(Clone, Debug)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub avg_logprob: f64,
    pub words: Vec<Word>,
}

#[derive(Clone, Debug)]
pub struct Options {
    /// "auto" or a language code like "en"
    pub language: String,
    pub word_timestamps: bool,
    pub translate: bool,
}
impl Default for Options {
    fn default() -> Options {
        Options { language: "auto".into(), word_timestamps: true, translate: false }
    }
}

pub struct Whisper {
    model: Model,
    tok: tokenizers::Tokenizer,
    filters: Vec<f32>,
    alignment_heads: Vec<(usize, usize)>,
    suppress: Vec<u32>,
    suppress_begin: Vec<u32>,
    lang_ids: Vec<u32>,
    sot: u32,
    eot: u32,
    transcribe: u32,
    translate: u32,
    no_timestamps: u32,
    ts_begin: u32,
    no_speech: Option<u32>,
    pub device_name: String,
}

/// models found in the Hugging Face caches: (name, directory)
pub fn find_models() -> Vec<(String, PathBuf)> {
    let mut roots: Vec<PathBuf> = vec![];
    for var in ["HF_HUB_CACHE", "HUGGINGFACE_HUB_CACHE"] {
        if let Some(p) = std::env::var_os(var) {
            roots.push(p.into());
        }
    }
    if let Some(p) = std::env::var_os("HF_HOME") {
        roots.push(PathBuf::from(p).join("hub"));
    }
    for home in ["USERPROFILE", "HOME"] {
        if let Some(h) = std::env::var_os(home) {
            roots.push(PathBuf::from(h).join(".cache").join("huggingface").join("hub"));
        }
    }
    let mut out: Vec<(String, PathBuf)> = vec![];
    for root in roots {
        let Ok(rd) = std::fs::read_dir(&root) else { continue };
        for d in rd.flatten() {
            let dn = d.file_name().to_string_lossy().into_owned();
            if !dn.starts_with("models--") || !dn.to_lowercase().contains("whisper") {
                continue;
            }
            let Ok(snaps) = std::fs::read_dir(d.path().join("snapshots")) else { continue };
            for s in snaps.flatten() {
                if s.path().join("model.bin").is_file() && s.path().join("tokenizer.json").is_file() {
                    let mut name = dn.rsplit("--").next().unwrap_or(&dn).to_string();
                    for pre in ["faster-whisper-", "whisper-"] {
                        if let Some(r) = name.strip_prefix(pre) {
                            name = r.to_string();
                            break;
                        }
                    }
                    if !out.iter().any(|x| x.0 == name) {
                        out.push((name, s.path()));
                    }
                }
            }
        }
    }
    // models shipped with the program: <exe folder>/models/whisper/<name>/ and ./assets/models/whisper/<name>/ (and LING_WHISPER_DIR)
    let mut local: Vec<PathBuf> = vec![];
    if let Some(d) = std::env::var_os("LING_WHISPER_DIR") {
        local.push(d.into());
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            local.push(d.join("models").join("whisper"));
            local.push(d.join("assets").join("models").join("whisper"));
        }
    }
    local.push(PathBuf::from("models/whisper"));
    local.push(PathBuf::from("assets/models/whisper"));
    for root in local {
        let Ok(rd) = std::fs::read_dir(&root) else { continue };
        for d in rd.flatten() {
            let p = d.path();
            if p.join("model.bin").is_file() && p.join("tokenizer.json").is_file() {
                let mut name = d.file_name().to_string_lossy().into_owned();
                for pre in ["faster-whisper-", "whisper-"] {
                    if let Some(r) = name.strip_prefix(pre) {
                        name = r.to_string();
                        break;
                    }
                }
                // a bundled copy wins over a cached one of the same name
                out.retain(|x| x.0 != name);
                out.push((name, p));
            }
        }
    }
    out.sort();
    out
}

/// the directory of a model given by name ("small", "large-v3") or by path
pub fn resolve_model(name: &str) -> Option<PathBuf> {
    let p = Path::new(name);
    if p.join("model.bin").is_file() {
        return Some(p.to_path_buf());
    }
    let models = find_models();
    models.iter().find(|m| m.0 == name).or_else(|| models.iter().rev().find(|m| m.0.starts_with(name))).map(|m| m.1.clone())
}

impl Whisper {
    /// `device_index` < 0 = CPU, otherwise a CUDA device (falls back to the CPU when CUDA is not there)
    pub fn load(dir: &Path, device_index: i64) -> R<Whisper> {
        let dev = if device_index < 0 { Device::Cpu } else { Device::new_cuda(device_index as usize).unwrap_or(Device::Cpu) };
        let device_name = if matches!(dev, Device::Cpu) { "cpu".to_string() } else { format!("cuda:{device_index}") };
        let vars = read_ct2(&dir.join("model.bin"))?;
        let model = load_model(&vars, &dev)?;
        let tok = tokenizers::Tokenizer::from_file(dir.join("tokenizer.json")).map_err(e)?;
        let cfg: serde_json::Value = std::fs::read_to_string(dir.join("config.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(serde_json::Value::Null);
        let ids = |k: &str| -> Vec<u32> { cfg[k].as_array().map(|a| a.iter().filter_map(|v| v.as_i64()).filter(|v| *v >= 0).map(|v| v as u32).collect()).unwrap_or_default() };
        let id = |t: &str| -> R<u32> { tok.token_to_id(t).ok_or_else(|| format!("tokenizer has no {t}")) };
        let alignment_heads: Vec<(usize, usize)> = cfg["alignment_heads"].as_array().map(|a| a.iter().filter_map(|p| Some((p[0].as_u64()? as usize, p[1].as_u64()? as usize))).collect()).unwrap_or_default();
        let mut lang_ids = ids("lang_ids");
        if lang_ids.is_empty() {
            // the language tokens sit between <|startoftranscript|> and <|translate|>
            let (a, b) = (id("<|startoftranscript|>")? + 1, id("<|translate|>")?);
            lang_ids = (a..b).collect();
        }
        Ok(Whisper {
            filters: mel_filters(model.n_mels),
            model,
            sot: id("<|startoftranscript|>")?,
            eot: id("<|endoftext|>")?,
            transcribe: id("<|transcribe|>")?,
            translate: id("<|translate|>")?,
            no_timestamps: id("<|notimestamps|>")?,
            ts_begin: id("<|notimestamps|>")? + 1, // the timestamp tokens <|0.00|>.. follow it; the tokenizer file does not list them
            no_speech: tok.token_to_id("<|nospeech|>").or_else(|| tok.token_to_id("<|nocaptions|>")),
            suppress: ids("suppress_ids"),
            suppress_begin: ids("suppress_ids_begin"),
            lang_ids,
            alignment_heads,
            tok,
            device_name,
        })
    }

    fn mel_tensor(&self, mel: &[f32], frames_total: usize, seek: usize) -> R<(Tensor, usize)> {
        let n = (frames_total - seek).min(N_FRAMES);
        let nm = self.model.n_mels;
        // one 30 s window, zero-padded in time where the audio ends
        let mut win = vec![-0.5f32; nm * N_FRAMES]; // the log-mel floor after normalisation
        let fill = mel.len() / nm;
        for m in 0..nm {
            for t in 0..n {
                win[m * N_FRAMES + t] = mel[m * fill + seek + t];
            }
        }
        Ok((Tensor::from_vec(win, (1, nm, N_FRAMES), &self.model.dev).map_err(e)?, n))
    }

    /// the language token the audio is in, with its probability
    pub fn detect_language(&mut self, samples: &[f32]) -> R<(String, f64)> {
        let mel = log_mel(&samples[..samples.len().min(N_SAMPLES)], self.model.n_mels, &self.filters);
        let frames = mel.len() / self.model.n_mels;
        let (t, _) = self.mel_tensor(&mel, frames, 0)?;
        let xa = self.model.encode(&t)?;
        let (logits, _) = self.model.decode(&[self.sot], &xa, true, false)?;
        let l = logits.i((0, 0)).and_then(|t| t.to_vec1::<f32>()).map_err(e)?;
        let (mut best, mut best_l) = (self.lang_ids[0], f32::MIN);
        let mx = self.lang_ids.iter().map(|i| l[*i as usize]).fold(f32::MIN, f32::max);
        let z: f64 = self.lang_ids.iter().map(|i| ((l[*i as usize] - mx) as f64).exp()).sum();
        for i in &self.lang_ids {
            if l[*i as usize] > best_l {
                best_l = l[*i as usize];
                best = *i;
            }
        }
        let name = self.tok.id_to_token(best).unwrap_or_default().trim_matches(|c| c == '<' || c == '|' || c == '>').to_string();
        Ok((name, (((best_l - mx) as f64).exp()) / z))
    }

    /// Transcribe 16 kHz mono samples. `progress(seconds done, total seconds)` is called as windows finish; `cancel` stops early.
    pub fn transcribe(&mut self, samples: &[f32], opts: &Options, cancel: &AtomicBool, progress: &mut dyn FnMut(f64, f64), on_segment: &mut dyn FnMut(&Segment)) -> R<(Vec<Segment>, String)> {
        let nm = self.model.n_mels;
        let total_secs = samples.len() as f64 / SAMPLE_RATE as f64;
        let mel = log_mel(samples, nm, &self.filters);
        let frames_total = (samples.len() / HOP).max(1);
        // the language
        let lang = if opts.language == "auto" || opts.language.is_empty() { self.detect_language(samples)?.0 } else { opts.language.clone() };
        let lang_tok = self.tok.token_to_id(&format!("<|{lang}|>")).ok_or_else(|| format!("unknown language '{lang}'"))?;
        let mut prompt = vec![self.sot, lang_tok, if opts.translate { self.translate } else { self.transcribe }];
        let sample_begin = prompt.len();
        let mut out: Vec<Segment> = vec![];
        let mut seek = 0usize;
        while seek < frames_total {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let (t, seg_frames) = self.mel_tensor(&mel, frames_total, seek)?;
            let time_offset = seek as f64 * HOP as f64 / SAMPLE_RATE as f64;
            let xa = self.model.encode(&t)?;
            // decode, falling back to sampling when the greedy answer is a loop or a mumble
            let mut best: Option<(Vec<u32>, Vec<f32>, f64, f64)> = None;
            for (ti, temp) in [0.0f64, 0.2, 0.4, 0.6].iter().enumerate() {
                let r = self.decode_window(&xa, &mut prompt, sample_begin, *temp, ti == 0)?;
                let loops = repetition(&r.0, self.ts_begin);
                let ok = r.2 > -1.0 && loops < 0.55;
                best = Some(r.clone());
                if ok {
                    break;
                }
            }
            let (tokens, probs, avg_lp, no_speech_p) = best.unwrap();
            if std::env::var_os("LING_STT_DEBUG").is_some() {
                eprintln!("window at {:.2}s: avg_lp {:.2} no_speech {:.2} tokens {:?}", time_offset, avg_lp, no_speech_p, tokens);
            }
            prompt.truncate(sample_begin);
            if no_speech_p > 0.45 && avg_lp < -1.0 {
                // silence: nothing here, move on a whole window
                seek += seg_frames.max(N_FRAMES.min(frames_total - seek));
                progress((seek as f64 * HOP as f64 / SAMPLE_RATE as f64).min(total_secs), total_secs);
                continue;
            }
            // cut the tokens into segments at the timestamp tokens
            let is_ts = |x: u32| x >= self.ts_begin;
            let mut segs: Vec<(f64, f64, Vec<usize>)> = vec![]; // start, end, token indices of the text
            let mut cur: Vec<usize> = vec![];
            let mut start: Option<f64> = None;
            let mut last_end_frames = 0usize;
            let mut single_ending = false;
            for (i, &tk) in tokens.iter().enumerate() {
                if is_ts(tk) {
                    let ts = (tk - self.ts_begin) as f64 * 0.02;
                    match start {
                        None => start = Some(ts),
                        Some(s) => {
                            if !cur.is_empty() {
                                segs.push((s, ts, std::mem::take(&mut cur)));
                                last_end_frames = (ts * 100.0).round() as usize; // mel frames are 10 ms
                            }
                            start = None;
                        }
                    }
                    if i + 1 == tokens.len() && start.is_some() {
                        single_ending = true;
                    }
                } else if tk < self.eot {
                    cur.push(i);
                }
            }
            if !cur.is_empty() {
                let s = start.unwrap_or(0.0);
                let dur = seg_frames as f64 * HOP as f64 / SAMPLE_RATE as f64;
                segs.push((s, dur.min(CHUNK as f64), std::mem::take(&mut cur)));
                single_ending = true;
            }
            // word timestamps for the whole window
            let mut words_by_seg: Vec<Vec<Word>> = vec![vec![]; segs.len()];
            if opts.word_timestamps && !segs.is_empty() && !self.alignment_heads.is_empty() {
                let all: Vec<u32> = segs.iter().flat_map(|s| s.2.iter().map(|i| tokens[*i])).collect();
                let all_probs: Vec<f32> = segs.iter().flat_map(|s| s.2.iter().map(|i| probs[*i])).collect();
                let aligned = self.align_words(&xa, &prompt_for_align(&prompt[..sample_begin], self.no_timestamps), &all, &all_probs, seg_frames);
                if let (Err(e), true) = (&aligned, std::env::var_os("LING_STT_DEBUG").is_some()) {
                    eprintln!("word alignment failed: {e}");
                }
                if let Ok(ws) = aligned {
                    // hand the words to the segments by token counts
                    let mut ti = 0usize;
                    for (si, s) in segs.iter().enumerate() {
                        let n = s.2.len();
                        for w in &ws {
                            if w.2 >= ti && w.2 < ti + n {
                                words_by_seg[si].push(Word { start: w.0.max(s.0), end: w.1.min(s.1.max(w.0)), text: w.3.clone(), prob: w.4 });
                            }
                        }
                        ti += n;
                    }
                }
            }
            for (si, (s0, s1, idx)) in segs.iter().enumerate() {
                let ids: Vec<u32> = idx.iter().map(|i| tokens[*i]).collect();
                let text = self.tok.decode(&ids, true).unwrap_or_default().trim().to_string();
                if text.is_empty() {
                    continue;
                }
                let lp: f64 = idx.iter().map(|i| (probs[*i].max(1e-9) as f64).ln()).sum::<f64>() / idx.len().max(1) as f64;
                let words: Vec<Word> = words_by_seg[si].iter().map(|w| Word { start: w.start + time_offset, end: (w.end + time_offset).max(w.start + time_offset), text: w.text.clone(), prob: w.prob }).collect();
                let seg = Segment { start: s0 + time_offset, end: s1 + time_offset, text, avg_logprob: lp.min(avg_lp.max(lp)), words };
                on_segment(&seg);
                out.push(seg);
            }
            // where the next window starts
            let advance = if single_ending || last_end_frames == 0 { seg_frames.max(1) } else { last_end_frames.clamp(1, seg_frames.max(1)) };
            seek += advance.max(1);
            progress((seek as f64 * HOP as f64 / SAMPLE_RATE as f64).min(total_secs), total_secs);
        }
        Ok((out, lang))
    }

    /// One pass over a window: (tokens after the prompt, their probabilities, average log-probability, no-speech probability)
    fn decode_window(&mut self, xa: &Tensor, prompt: &mut Vec<u32>, sample_begin: usize, temperature: f64, first_try: bool) -> R<(Vec<u32>, Vec<f32>, f64, f64)> {
        let _ = first_try;
        let mut tokens: Vec<u32> = prompt.clone();
        let mut probs: Vec<f32> = vec![];
        let mut sum_lp = 0f64;
        let mut no_speech_p = 0f64;
        let max_new = 224usize;
        let n_vocab = self.model.vocab;
        for step in 0..max_new {
            let (logits, _) = self.model.decode(&tokens, xa, step == 0, false)?;
            let last = tokens.len() - 1;
            let mut l = logits.i((0, last)).and_then(|t| t.to_vec1::<f32>()).map_err(e)?;
            l.truncate(n_vocab);
            if step == 0 {
                if let Some(ns) = self.no_speech {
                    // the no-speech probability is read from the first step of the prompt alone (the position after <|startoftranscript|>)
                    let l0 = logits.i((0, 0)).and_then(|t| t.to_vec1::<f32>()).map_err(e)?;
                    no_speech_p = softmax_at(&l0, ns as usize);
                }
            }
            // suppress
            for s in &self.suppress {
                if (*s as usize) < l.len() {
                    l[*s as usize] = f32::NEG_INFINITY;
                }
            }
            if tokens.len() == sample_begin {
                for s in &self.suppress_begin {
                    if (*s as usize) < l.len() {
                        l[*s as usize] = f32::NEG_INFINITY;
                    }
                }
            }
            for x in [self.sot, self.transcribe, self.translate, self.no_timestamps] {
                l[x as usize] = f32::NEG_INFINITY;
            }
            // the timestamp rules
            let tb = self.ts_begin as usize;
            let gtok: &[u32] = &tokens[sample_begin..];
            let last_ts = gtok.last().map(|x| *x >= self.ts_begin).unwrap_or(false);
            let pen_ts = gtok.len() < 2 || gtok[gtok.len() - 2] >= self.ts_begin;
            if last_ts {
                if pen_ts {
                    for v in l[tb..].iter_mut() {
                        *v = f32::NEG_INFINITY;
                    }
                } else {
                    for v in l[..self.eot as usize].iter_mut() {
                        *v = f32::NEG_INFINITY;
                    }
                }
            }
            if let Some(lt) = gtok.iter().rev().find(|x| **x >= self.ts_begin) {
                let from = if last_ts && !pen_ts { *lt as usize } else { *lt as usize + 1 };
                let upto = from.min(l.len());
                for v in l[tb..upto.max(tb)].iter_mut() {
                    *v = f32::NEG_INFINITY;
                }
            }
            if gtok.is_empty() {
                for v in l[..tb].iter_mut() {
                    *v = f32::NEG_INFINITY;
                }
                let last_allowed = tb + 50; // the first timestamp may not be later than one second
                let from = (last_allowed + 1).min(l.len());
                for v in l[from..].iter_mut() {
                    *v = f32::NEG_INFINITY;
                }
            }
            // timestamps win when their total probability beats the best text token
            let lse = |xs: &[f32]| -> f32 {
                let mx = xs.iter().cloned().fold(f32::MIN, f32::max);
                if mx == f32::NEG_INFINITY { return f32::NEG_INFINITY }
                mx + xs.iter().map(|x| (x - mx).exp()).sum::<f32>().ln()
            };
            let all = lse(&l);
            let ts_lp = lse(&l[tb..]) - all;
            let text_max = l[..tb].iter().cloned().fold(f32::NEG_INFINITY, f32::max) - all;
            if ts_lp > text_max {
                for v in l[..tb].iter_mut() {
                    *v = f32::NEG_INFINITY;
                }
            }
            let all = lse(&l);
            let next = if temperature > 0.0 {
                // sample from the softened distribution
                let weights: Vec<f64> = l.iter().map(|x| (((x - all) as f64) / temperature).exp()).collect();
                let z: f64 = weights.iter().sum();
                let mut r = rand_unit() * z;
                let mut pick = 0usize;
                for (i, w) in weights.iter().enumerate() {
                    r -= w;
                    if r <= 0.0 {
                        pick = i;
                        break;
                    }
                }
                pick as u32
            } else {
                l.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal)).map(|x| x.0 as u32).unwrap_or(self.eot)
            };
            let p = (l[next as usize] - all).exp();
            sum_lp += (p.max(1e-9) as f64).ln();
            if next == self.eot {
                break;
            }
            tokens.push(next);
            probs.push(p);
        }
        let gtok: Vec<u32> = tokens[sample_begin..].to_vec();
        let n = gtok.len().max(1) as f64;
        Ok((gtok, probs, sum_lp / (n + 1.0), no_speech_p))
    }

    /// word boundaries from the cross-attention of the alignment heads: (start, end, index of its first token, text, probability)
    fn align_words(&mut self, xa: &Tensor, sot_seq: &[u32], text: &[u32], probs: &[f32], frames: usize) -> R<Vec<(f64, f64, usize, String, f64)>> {
        if text.is_empty() {
            return Ok(vec![]);
        }
        let mut tokens: Vec<u32> = sot_seq.to_vec();
        tokens.extend_from_slice(text);
        tokens.push(self.eot);
        let (_, caps) = self.model.decode(&tokens, xa, true, true)?;
        let n_frames_enc = (frames / 2).clamp(1, 1500);
        // the selected heads: (layer, head) -> (tokens, frames)
        let mut mats: Vec<Vec<f32>> = vec![];
        for (layer, head) in &self.alignment_heads {
            let Some(qk) = caps.get(*layer) else { continue };
            let m = qk.i((0, *head)).and_then(|t| t.narrow(1, 0, n_frames_enc)).and_then(|t| t.contiguous()).and_then(|t| candle_nn::ops::softmax_last_dim(&t)).and_then(|t| t.to_vec2::<f32>()).map_err(e)?;
            mats.push(m.into_iter().flatten().collect());
        }
        if mats.is_empty() {
            return Ok(vec![]);
        }
        let n_tok = tokens.len();
        let (start_row, end_row) = (sot_seq.len(), n_tok - 1);
        let rows = end_row - start_row;
        // normalise each head over the tokens, median filter along time, average the heads
        let mut avg = vec![0f32; rows * n_frames_enc];
        for m in &mats {
            for f in 0..n_frames_enc {
                let col: Vec<f32> = (0..n_tok).map(|t| m[t * n_frames_enc + f]).collect();
                let mean = col.iter().sum::<f32>() / n_tok as f32;
                let var = col.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / n_tok as f32;
                let sd = var.sqrt().max(1e-8);
                for r in 0..rows {
                    avg[r * n_frames_enc + f] += (m[(start_row + r) * n_frames_enc + f] - mean) / sd;
                }
            }
        }
        for v in avg.iter_mut() {
            *v /= mats.len() as f32;
        }
        let filtered = median_filter_rows(&avg, rows, n_frames_enc, 7);
        let path = dtw(&filtered, rows, n_frames_enc);
        // times where the token index changes
        let mut jump_times: Vec<f64> = vec![];
        let mut prev: i64 = -1;
        for (ti, fi) in &path {
            if *ti as i64 != prev {
                jump_times.push(*fi as f64 / TOKENS_PER_SECOND);
                prev = *ti as i64;
            }
        }
        // group the text tokens into words
        let words = self.split_words(text);
        let mut out = vec![];
        let mut at = 0usize;
        for (wtext, ntok) in words {
            let (s, e_) = (jump_times.get(at).copied().unwrap_or(0.0), jump_times.get(at + ntok).copied().unwrap_or_else(|| jump_times.last().copied().unwrap_or(0.0)));
            let p = if ntok > 0 { probs[at..(at + ntok).min(probs.len())].iter().map(|x| *x as f64).sum::<f64>() / ntok as f64 } else { 0.0 };
            out.push((s, e_.max(s), at, wtext, p));
            at += ntok;
        }
        Ok(out)
    }

    /// (word, number of tokens) in order; tokens are joined into whole characters first, then words at spaces / punctuation
    fn split_words(&self, text: &[u32]) -> Vec<(String, usize)> {
        // whole unicode characters
        let mut subs: Vec<(String, usize)> = vec![];
        let mut cur: Vec<u32> = vec![];
        for &t in text {
            cur.push(t);
            let s = self.tok.decode(&cur, false).unwrap_or_default();
            if !s.contains('\u{fffd}') {
                subs.push((s, cur.len()));
                cur.clear();
            }
        }
        if !cur.is_empty() {
            subs.push((self.tok.decode(&cur, false).unwrap_or_default(), cur.len()));
        }
        let lang_no_space = false;
        let mut words: Vec<(String, usize)> = vec![];
        for (s, n) in subs {
            let punct = s.trim().chars().all(|c| matches!(c, '.' | ',' | '!' | '?' | ':' | '"' | '\'' | ')' | ']' | '}' | '-' | ';' | '…' | '。' | '，' | '！' | '？' | '」' | '』'));
            if words.is_empty() || s.starts_with(' ') || (punct && false) || lang_no_space {
                words.push((s.trim().to_string(), n));
            } else if let Some(last) = words.last_mut() {
                last.0.push_str(s.trim());
                last.1 += n;
            }
        }
        words.retain(|w| !w.0.is_empty() || w.1 > 0);
        words
    }
}

fn prompt_for_align(prompt: &[u32], no_timestamps: u32) -> Vec<u32> {
    let mut v = prompt.to_vec();
    v.push(no_timestamps);
    v
}

fn softmax_at(logits: &[f32], i: usize) -> f64 {
    let mx = logits.iter().cloned().fold(f32::MIN, f32::max);
    let z: f64 = logits.iter().map(|x| ((x - mx) as f64).exp()).sum();
    ((logits[i] - mx) as f64).exp() / z
}

fn rand_unit() -> f64 {
    use std::cell::Cell;
    thread_local!(static S: Cell<u64> = Cell::new(0x9E3779B97F4A7C15));
    S.with(|s| {
        let mut x = s.get();
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        s.set(x);
        (x >> 11) as f64 / (1u64 << 53) as f64
    })
}

/// how repetitive a token run is: 0 = varied, near 1 = the same few tokens over and over
fn repetition(tokens: &[u32], ts_begin: u32) -> f64 {
    let t: Vec<u32> = tokens.iter().cloned().filter(|x| *x < ts_begin).collect();
    if t.len() < 12 {
        return 0.0;
    }
    let mut grams = std::collections::HashSet::new();
    for w in t.windows(3) {
        grams.insert((w[0], w[1], w[2]));
    }
    1.0 - grams.len() as f64 / (t.len() - 2) as f64
}

fn median_filter_rows(x: &[f32], rows: usize, cols: usize, width: usize) -> Vec<f32> {
    let h = width / 2;
    let mut out = vec![0f32; x.len()];
    let mut win = vec![0f32; width];
    for r in 0..rows {
        for c in 0..cols {
            for k in 0..width {
                // reflect at the edges
                let mut j = c as i64 + k as i64 - h as i64;
                if j < 0 {
                    j = -j;
                }
                if j >= cols as i64 {
                    j = 2 * (cols as i64 - 1) - j;
                }
                win[k] = x[r * cols + j.clamp(0, cols as i64 - 1) as usize];
            }
            win.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            out[r * cols + c] = win[h];
        }
    }
    out
}

/// dynamic time warping over -matrix; the path as (token row, frame) pairs
fn dtw(m: &[f32], rows: usize, cols: usize) -> Vec<(usize, usize)> {
    let inf = f32::INFINITY;
    let mut cost = vec![inf; (rows + 1) * (cols + 1)];
    let mut trace = vec![0u8; (rows + 1) * (cols + 1)];
    cost[0] = 0.0;
    let w = cols + 1;
    for i in 1..=rows {
        for j in 1..=cols {
            let (c0, c1, c2) = (cost[(i - 1) * w + j - 1], cost[(i - 1) * w + j], cost[i * w + j - 1]);
            let (c, t) = if c0 <= c1 && c0 <= c2 { (c0, 0) } else if c1 <= c0 && c1 <= c2 { (c1, 1) } else { (c2, 2) };
            cost[i * w + j] = -m[(i - 1) * cols + j - 1] + c;
            trace[i * w + j] = t;
        }
    }
    let (mut i, mut j) = (rows, cols);
    let mut path = vec![];
    while i > 0 || j > 0 {
        path.push((i.saturating_sub(1), j.saturating_sub(1)));
        if i == 0 {
            j -= 1;
            continue;
        }
        if j == 0 {
            i -= 1;
            continue;
        }
        match trace[i * w + j] {
            0 => {
                i -= 1;
                j -= 1;
            }
            1 => i -= 1,
            _ => j -= 1,
        }
    }
    path.reverse();
    path
}
