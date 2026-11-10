//! Building blocks shared by the Qwen-Image-Edit pieces: a linear layer whose weight may be a GGUF
//! quantized tensor (dequantized per call, so a 20B model fits a 24 GB card) with optional LoRA
//! side paths, attention that never materialises more than a few hundred MB of scores, and a conv2d
//! that splits into horizontal stripes so candle's im2col scratch stays bounded.

use candle_core::quantized::QTensor;
use candle_core::{DType, Device, Result, Tensor, D};
use std::sync::Arc;

pub enum Wt {
    /// quantized; dequantized to f16 (then the compute dtype) on every call
    Q(Arc<QTensor>),
    /// dense, already in the compute dtype
    D(Tensor),
}

pub struct QLinear {
    pub w: Wt,
    pub b: Option<Tensor>,
    /// (down [r, in], up [out, r], scale)
    pub lora: Vec<(Tensor, Tensor, f64)>,
    pub dtype: DType,
}

impl QLinear {
    pub fn dense(w: Tensor, b: Option<Tensor>, dtype: DType) -> Result<Self> {
        Ok(Self { w: Wt::D(w.to_dtype(dtype)?), b: b.map(|b| b.to_dtype(DType::F32)).transpose()?, lora: Vec::new(), dtype })
    }
    pub fn quant(q: QTensor, b: Option<Tensor>, dtype: DType) -> Result<Self> {
        Ok(Self { w: Wt::Q(Arc::new(q)), b: b.map(|b| b.to_dtype(DType::F32)).transpose()?, lora: Vec::new(), dtype })
    }
    pub fn out_dim(&self) -> usize {
        match &self.w {
            Wt::Q(q) => q.shape().dims()[0],
            Wt::D(t) => t.dims()[0],
        }
    }
    fn weight(&self, dev: &Device) -> Result<Tensor> {
        match &self.w {
            Wt::D(t) => Ok(t.clone()),
            Wt::Q(q) => {
                let w = q.dequantize_f16(dev)?;
                if self.dtype == DType::F16 {
                    Ok(w)
                } else {
                    w.to_dtype(self.dtype)
                }
            }
        }
    }
    /// x [..., in] (any float dtype) -> [..., out] in f32
    pub fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let dims = x.dims().to_vec();
        let inn = *dims.last().unwrap();
        let n = x.elem_count() / inn;
        let x2 = x.reshape((n, inn))?.to_dtype(self.dtype)?;
        let w = self.weight(x.device())?;
        let mut y = x2.matmul(&w.t()?)?.to_dtype(DType::F32)?;
        for (down, up, scale) in &self.lora {
            let l = x2.matmul(&down.t()?)?.matmul(&up.t()?)?.to_dtype(DType::F32)?;
            y = (y + (l * *scale)?)?;
        }
        if let Some(b) = &self.b {
            y = y.broadcast_add(b)?;
        }
        let mut od = dims;
        *od.last_mut().unwrap() = y.dim(1)?;
        y.reshape(od)
    }
}

/// LayerNorm without affine parameters over the last dim, in f32.
pub fn layer_norm(x: &Tensor, eps: f64) -> Result<Tensor> {
    let x = x.to_dtype(DType::F32)?;
    let d = x.dim(D::Minus1)?;
    let ones = Tensor::ones(d, DType::F32, x.device())?;
    let zeros = Tensor::zeros(d, DType::F32, x.device())?;
    candle_nn::ops::layer_norm(&x.contiguous()?, &ones, &zeros, eps as f32)
}

/// RMSNorm over the last dim with a weight, in f32.
pub fn rms_norm(x: &Tensor, w: &Tensor, eps: f64) -> Result<Tensor> {
    candle_nn::ops::rms_norm(&x.to_dtype(DType::F32)?.contiguous()?, &w.to_dtype(DType::F32)?, eps as f32)
}

/// softmax(q k^T * scale + mask) v, queries processed in chunks so the score matrix stays near
/// `budget` bytes. q [B,H,Lq,Dh], k/v [B,H,Lk,Dh] in the same dtype; mask broadcastable to
/// [B,H,Lq,Lk] (f32, 0 / -inf). Returns [B,H,Lq,Dh] in q's dtype.
/// Bytes one chunk of attention scores may take: 1 GB, less when the card is nearly full, so a
/// big hires pass slows down a little instead of pushing the GPU (often the display GPU) into
/// Windows' shared-memory fallback, which stalls the whole desktop.
fn score_budget(dev: &Device) -> usize {
    #[cfg(feature = "cuda")]
    if let Device::Cuda(cd) = dev {
        if let Ok((free, _)) = cd.cuda_stream().context().mem_get_info() {
            return (free / 6).clamp(64 << 20, 1 << 30);
        }
    }
    let _ = dev;
    1 << 30
}

pub fn attention(q: &Tensor, k: &Tensor, v: &Tensor, mask: Option<&Tensor>, scale: f64) -> Result<Tensor> {
    let (b, h, lq, _) = q.dims4()?;
    let budget = score_budget(q.device());
    let lk = k.dim(2)?;
    // Half-precision inputs without a mask (the MMDiT's joint attention, the vision tower): scores
    // stay f16 end to end — the scale folded into q, candle's fused softmax (f32 inside) in place —
    // instead of an f32 copy of every score matrix, which made attention ~5x slower than the GEMMs.
    // f16 rather than bf16: logits near 30 keep ~0.02 precision (bf16 would be ~0.1, visibly
    // skewing the weights). Masked or f32 attention (the text LM, the VAE) keeps the exact path.
    if mask.is_none() && q.dtype() != DType::F32 {
        let in_dt = q.dtype();
        let qs = (q.to_dtype(DType::F32)? * scale)?.to_dtype(DType::F16)?;
        let kt = k.to_dtype(DType::F16)?.t()?.contiguous()?;
        let v16 = v.to_dtype(DType::F16)?.contiguous()?;
        let per_row = (b * h * lk * 2).max(1);
        let chunk = ((budget / per_row).max(1)).clamp(1, lq);
        let mut outs = Vec::new();
        let mut s = 0;
        while s < lq {
            let n = chunk.min(lq - s);
            let att = qs.narrow(2, s, n)?.contiguous()?.matmul(&kt)?;
            let att = candle_nn::ops::softmax_last_dim(&att)?;
            outs.push(att.matmul(&v16)?.to_dtype(in_dt)?);
            s += n;
        }
        return Tensor::cat(&outs.iter().collect::<Vec<_>>(), 2);
    }
    let kt = k.t()?.contiguous()?;
    let v = v.contiguous()?;
    let budget = budget / 2;
    let per_row = (b * h * lk * 4).max(1);
    let chunk = (budget / per_row).clamp(1, lq);
    let mut outs = Vec::new();
    let mut s = 0;
    while s < lq {
        let n = chunk.min(lq - s);
        let qc = q.narrow(2, s, n)?.contiguous()?;
        let mut att = (qc.matmul(&kt)?.to_dtype(DType::F32)? * scale)?;
        if let Some(m) = mask {
            let mc = if m.dim(2)? == lq { m.narrow(2, s, n)? } else { m.clone() };
            att = att.broadcast_add(&mc)?;
        }
        let att = candle_nn::ops::softmax_last_dim(&att)?.to_dtype(v.dtype())?;
        outs.push(att.matmul(&v)?);
        s += n;
    }
    Tensor::cat(&outs.iter().collect::<Vec<_>>(), 2)
}

/// conv2d on an already padded [1,C,H,W] input with no further padding. Splits the output into
/// row stripes when im2col would need more than ~1 GB.
pub fn conv2d(x: &Tensor, w: &Tensor, b: Option<&Tensor>, stride: usize) -> Result<Tensor> {
    let (_, c, h, wd) = x.dims4()?;
    let k = w.dim(2)?;
    let oh = (h - k) / stride + 1;
    let ow = (wd - k) / stride + 1;
    let row_bytes = ow * c * k * k * x.dtype().size_in_bytes();
    let rows = ((1usize << 30) / row_bytes.max(1)).max(1);
    let y = if rows >= oh || k == 1 {
        x.conv2d(w, 0, stride, 1, 1)?
    } else {
        let mut parts = Vec::new();
        let mut r0 = 0;
        while r0 < oh {
            let r1 = (r0 + rows).min(oh);
            let i0 = r0 * stride;
            let ilen = (r1 - 1) * stride + k - i0;
            parts.push(x.narrow(2, i0, ilen)?.conv2d(w, 0, stride, 1, 1)?);
            r0 = r1;
        }
        Tensor::cat(&parts.iter().collect::<Vec<_>>(), 2)?
    };
    match b {
        Some(b) => y.broadcast_add(&b.reshape((1, b.dim(0)?, 1, 1))?),
        None => Ok(y),
    }
}

/// x * (1 + scale) + shift with per-token rows: `rows` lists (token_count, shift, scale) segments
/// in order; shift/scale are [dim]. Used for Qwen-Image's zero_cond_t modulation, where the
/// noisy-latent tokens and the reference-image tokens get different timestep embeddings.
pub fn modulate_segments(x: &Tensor, segs: &[(usize, Tensor, Tensor)]) -> Result<Tensor> {
    let mut parts = Vec::new();
    let mut at = 0;
    for (n, shift, scale) in segs {
        let xs = x.narrow(1, at, *n)?;
        parts.push(xs.broadcast_mul(&(scale + 1.0)?)?.broadcast_add(shift)?);
        at += n;
    }
    Tensor::cat(&parts.iter().collect::<Vec<_>>(), 1)
}

/// The gate per token for the same segments: [1, L, dim]
pub fn gate_segments(segs: &[(usize, Tensor)], dim: usize) -> Result<Tensor> {
    let mut parts = Vec::new();
    for (n, g) in segs {
        parts.push(g.reshape((1, 1, dim))?.broadcast_as((1, *n, dim))?);
    }
    Tensor::cat(&parts.iter().collect::<Vec<_>>(), 1)
}

/// rotate_half RoPE: x [.., L, H, Dh] or [L, H, Dh]; cos/sin [L, Dh] (already cat(f, f)).
pub fn rope_half(x: &Tensor, cos: &Tensor, sin: &Tensor) -> Result<Tensor> {
    let rank = x.rank();
    let dh = x.dim(D::Minus1)?;
    let x = x.to_dtype(DType::F32)?;
    let x1 = x.narrow(D::Minus1, 0, dh / 2)?;
    let x2 = x.narrow(D::Minus1, dh / 2, dh / 2)?;
    let rot = Tensor::cat(&[&x2.neg()?, &x1], D::Minus1)?;
    // cos/sin [L, Dh] -> broadcast over heads: [L, 1, Dh] (+ leading dims)
    let (l, _) = cos.dims2()?;
    let mut shape = vec![1usize; rank];
    shape[rank - 3] = l;
    shape[rank - 1] = dh;
    let c = cos.reshape(shape.clone())?;
    let s = sin.reshape(shape)?;
    x.broadcast_mul(&c)? + rot.broadcast_mul(&s)?
}

/// interleaved-pair RoPE (Qwen-Image MMDiT): x [1, L, H, Dh]; cos/sin [L, Dh/2]
pub fn rope_pairs(x: &Tensor, cos: &Tensor, sin: &Tensor) -> Result<Tensor> {
    let (b, l, h, dh) = x.dims4()?;
    let x = x.to_dtype(DType::F32)?.reshape((b, l, h, dh / 2, 2))?;
    let x0 = x.narrow(4, 0, 1)?.squeeze(4)?;
    let x1 = x.narrow(4, 1, 1)?.squeeze(4)?;
    let c = cos.reshape((1, l, 1, dh / 2))?;
    let s = sin.reshape((1, l, 1, dh / 2))?;
    let o0 = (x0.broadcast_mul(&c)? - x1.broadcast_mul(&s)?)?;
    let o1 = (x0.broadcast_mul(&s)? + x1.broadcast_mul(&c)?)?;
    Tensor::stack(&[&o0, &o1], 4)?.reshape((b, l, h, dh))
}
