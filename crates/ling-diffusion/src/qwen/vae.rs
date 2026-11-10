//! Qwen-Image's VAE (AutoencoderKLQwenImage, a Wan-2.1 style causal 3D VAE) for single images.
//! With one frame and no feature cache, every causal 3D conv sees zero history, so it reduces to a
//! 2D conv with the kernel's last temporal slice, and the temporal resample convs are skipped (the
//! reference skips them on the first chunk too). 16 latent channels, 8x spatial compression.

use super::ops::conv2d;
use candle_core::{DType, Device, Result, Tensor, D};
use std::collections::HashMap;

pub const LATENTS_MEAN: [f32; 16] = [-0.7571, -0.7089, -0.9113, 0.1075, -0.1745, 0.9653, -0.1517, 1.5508, 0.4134, -0.0715, 0.5517, -0.3632, -0.1922, -0.9497, 0.2503, -0.2921];
pub const LATENTS_STD: [f32; 16] = [2.8184, 1.4541, 2.3275, 2.6558, 1.2196, 1.7708, 2.6052, 2.0743, 3.2687, 2.1526, 2.8652, 1.5579, 1.6382, 1.1253, 2.8251, 1.916];

struct Conv {
    w: Tensor,
    b: Tensor,
    pad: usize,
}

impl Conv {
    fn fwd(&self, x: &Tensor) -> Result<Tensor> {
        let x = if self.pad > 0 { x.pad_with_zeros(2, self.pad, self.pad)?.pad_with_zeros(3, self.pad, self.pad)? } else { x.clone() };
        conv2d(&x, &self.w, Some(&self.b), 1)
    }
}

struct Res {
    n1: Tensor,
    c1: Conv,
    n2: Tensor,
    c2: Conv,
    sc: Option<Conv>,
}

struct Attn {
    norm: Tensor,
    qkv: Conv,
    proj: Conv,
}

enum Layer {
    Res(Res),
    Down(Conv), // pad right/bottom by 1, 3x3 stride 2
    Up(Conv),   // nearest x2, 3x3
}

pub struct Vae {
    enc_in: Conv,
    enc: Vec<Layer>,
    enc_mid: (Res, Attn, Res),
    enc_norm: Tensor,
    enc_out: Conv,
    quant: Conv,
    post_quant: Conv,
    dec_in: Conv,
    dec_mid: (Res, Attn, Res),
    dec: Vec<Layer>,
    dec_norm: Tensor,
    dec_out: Conv,
    pub device: Device,
}

/// RMS over channels like F.normalize(x, dim=1) * sqrt(C) * gamma
fn rms(x: &Tensor, gamma: &Tensor) -> Result<Tensor> {
    let c = x.dim(1)?;
    let n = x.sqr()?.sum_keepdim(1)?.sqrt()?.clamp(1e-12f32, f32::MAX)?;
    x.broadcast_div(&n)?.broadcast_mul(&(gamma.reshape((1, c, 1, 1))? * (c as f64).sqrt())?)
}

impl Res {
    fn fwd(&self, x: &Tensor) -> Result<Tensor> {
        let h = match &self.sc {
            Some(s) => s.fwd(x)?,
            None => x.clone(),
        };
        let y = self.c1.fwd(&candle_nn::ops::silu(&rms(x, &self.n1)?)?)?;
        let y = self.c2.fwd(&candle_nn::ops::silu(&rms(&y, &self.n2)?)?)?;
        y + h
    }
}

impl Attn {
    fn fwd(&self, x: &Tensor) -> Result<Tensor> {
        let (_, c, h, w) = x.dims4()?;
        let y = self.qkv.fwd(&rms(x, &self.norm)?)?; // [1, 3C, H, W]
        let y = y.reshape((1, 3 * c, h * w))?.transpose(1, 2)?; // [1, HW, 3C]
        let q = y.narrow(2, 0, c)?.unsqueeze(1)?.contiguous()?;
        let k = y.narrow(2, c, c)?.unsqueeze(1)?.contiguous()?;
        let v = y.narrow(2, 2 * c, c)?.unsqueeze(1)?.contiguous()?;
        let o = super::ops::attention(&q, &k, &v, None, 1.0 / (c as f64).sqrt())?; // [1,1,HW,C]
        let o = o.squeeze(1)?.transpose(1, 2)?.reshape((1, c, h, w))?;
        self.proj.fwd(&o)? + x
    }
}

impl Layer {
    fn fwd(&self, x: &Tensor) -> Result<Tensor> {
        match self {
            Layer::Res(r) => r.fwd(x),
            Layer::Down(c) => {
                let x = x.pad_with_zeros(2, 0, 1)?.pad_with_zeros(3, 0, 1)?;
                conv2d(&x, &c.w, Some(&c.b), 2)
            }
            Layer::Up(c) => {
                let (_, _, h, w) = x.dims4()?;
                c.fwd(&x.upsample_nearest2d(h * 2, w * 2)?)
            }
        }
    }
}

struct W<'a> {
    m: &'a HashMap<String, Tensor>,
}

impl W<'_> {
    fn t(&self, k: &str) -> Result<Tensor> {
        self.m.get(k).cloned().ok_or_else(|| candle_core::Error::Msg(format!("VAE weight missing: {k}")))
    }
    fn conv(&self, p: &str, pad: usize) -> Result<Conv> {
        let w = self.t(&format!("{p}.weight"))?;
        let w = if w.rank() == 5 {
            let kt = w.dim(2)?;
            w.narrow(2, kt - 1, 1)?.squeeze(2)?.contiguous()?
        } else {
            w
        };
        Ok(Conv { w, b: self.t(&format!("{p}.bias"))?, pad })
    }
    fn gamma(&self, p: &str) -> Result<Tensor> {
        self.t(&format!("{p}.gamma"))?.flatten_all()
    }
    fn res(&self, p: &str) -> Result<Res> {
        let sc = if self.m.contains_key(&format!("{p}.conv_shortcut.weight")) { Some(self.conv(&format!("{p}.conv_shortcut"), 0)?) } else { None };
        Ok(Res { n1: self.gamma(&format!("{p}.norm1"))?, c1: self.conv(&format!("{p}.conv1"), 1)?, n2: self.gamma(&format!("{p}.norm2"))?, c2: self.conv(&format!("{p}.conv2"), 1)?, sc })
    }
    fn attn(&self, p: &str) -> Result<Attn> {
        Ok(Attn { norm: self.gamma(&format!("{p}.norm"))?, qkv: self.conv(&format!("{p}.to_qkv"), 0)?, proj: self.conv(&format!("{p}.proj"), 0)? })
    }
    fn mid(&self, p: &str) -> Result<(Res, Attn, Res)> {
        Ok((self.res(&format!("{p}.resnets.0"))?, self.attn(&format!("{p}.attentions.0"))?, self.res(&format!("{p}.resnets.1"))?))
    }
}

impl Vae {
    pub fn load(path: &str, dev: &Device) -> Result<Self> {
        let st = unsafe { candle_core::safetensors::MmapedSafetensors::new(path)? };
        let mut m = HashMap::new();
        for (n, _) in st.tensors() {
            m.insert(n.clone(), st.load(&n, dev)?.to_dtype(DType::F32)?);
        }
        let w = W { m: &m };
        let mut enc = Vec::new();
        for i in 0..11 {
            let p = format!("encoder.down_blocks.{i}");
            if m.contains_key(&format!("{p}.resample.1.weight")) {
                enc.push(Layer::Down(w.conv(&format!("{p}.resample.1"), 0)?));
            } else {
                enc.push(Layer::Res(w.res(&p)?));
            }
        }
        let mut dec = Vec::new();
        for i in 0..4 {
            for j in 0..3 {
                dec.push(Layer::Res(w.res(&format!("decoder.up_blocks.{i}.resnets.{j}"))?));
            }
            let up = format!("decoder.up_blocks.{i}.upsamplers.0.resample.1");
            if m.contains_key(&format!("{up}.weight")) {
                dec.push(Layer::Up(w.conv(&up, 1)?));
            }
        }
        Ok(Self {
            enc_in: w.conv("encoder.conv_in", 1)?,
            enc,
            enc_mid: w.mid("encoder.mid_block")?,
            enc_norm: w.gamma("encoder.norm_out")?,
            enc_out: w.conv("encoder.conv_out", 1)?,
            quant: w.conv("quant_conv", 0)?,
            post_quant: w.conv("post_quant_conv", 0)?,
            dec_in: w.conv("decoder.conv_in", 1)?,
            dec_mid: w.mid("decoder.mid_block")?,
            dec,
            dec_norm: w.gamma("decoder.norm_out")?,
            dec_out: w.conv("decoder.conv_out", 1)?,
            device: dev.clone(),
        })
    }

    fn norm_vec(&self, v: &[f32]) -> Result<Tensor> {
        Tensor::from_slice(v, (1, 16, 1, 1), &self.device)
    }

    /// image [1,3,H,W] in -1..1 -> normalized latent [1,16,H/8,W/8] ((z - mean) / std, the mode)
    pub fn encode(&self, x: &Tensor) -> Result<Tensor> {
        let mut x = self.enc_in.fwd(&x.to_dtype(DType::F32)?)?;
        for l in &self.enc {
            x = l.fwd(&x)?;
        }
        x = self.enc_mid.0.fwd(&x)?;
        x = self.enc_mid.1.fwd(&x)?;
        x = self.enc_mid.2.fwd(&x)?;
        x = self.enc_out.fwd(&candle_nn::ops::silu(&rms(&x, &self.enc_norm)?)?)?;
        let x = self.quant.fwd(&x)?;
        let mu = x.narrow(1, 0, 16)?;
        mu.broadcast_sub(&self.norm_vec(&LATENTS_MEAN)?)?.broadcast_div(&self.norm_vec(&LATENTS_STD)?)
    }

    /// normalized latent [1,16,h,w] -> image [1,3,8h,8w] in -1..1
    pub fn decode(&self, z: &Tensor) -> Result<Tensor> {
        let z = z.to_dtype(DType::F32)?.broadcast_mul(&self.norm_vec(&LATENTS_STD)?)?.broadcast_add(&self.norm_vec(&LATENTS_MEAN)?)?;
        let mut x = self.dec_in.fwd(&self.post_quant.fwd(&z)?)?;
        x = self.dec_mid.0.fwd(&x)?;
        x = self.dec_mid.1.fwd(&x)?;
        x = self.dec_mid.2.fwd(&x)?;
        for l in &self.dec {
            x = l.fwd(&x)?;
        }
        let x = self.dec_out.fwd(&candle_nn::ops::silu(&rms(&x, &self.dec_norm)?)?)?;
        x.clamp(-1f32, 1f32)
    }
}

#[allow(dead_code)]
fn _unused(_: D) {}
