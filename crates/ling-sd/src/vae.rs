//! The KL autoencoder's decoder (LDM layout `first_stage_model.*`): latent / 0.18215 -> post_quant_conv -> conv_in -> mid (ResNet,
//! single-head attention, ResNet) -> four up levels of three ResNets (2x nearest upsample + conv between them) -> norm, SiLU, conv.
use crate::act::Act;
use crate::ops::{Conv, Eng, In, Lin, Loader, Norm, Res};
use crate::Error;
use alloc::vec::Vec;

pub struct Vae {
    post_quant: Lin,
    conv_in: Conv,
    mid1: Res,
    attn_norm: Norm,
    q: Lin,
    k: Lin,
    v: Lin,
    proj: Lin,
    mid2: Res,
    /// highest level first (the order they run in), each with its upsampler (none at level 0)
    up: Vec<(Vec<Res>, Option<Conv>)>,
    norm_out: Norm,
    conv_out: Conv,
}

pub const SCALE: f32 = 0.18215;

impl Vae {
    /// `l.prefix` = `first_stage_model.`
    pub fn load(l: &mut Loader) -> Result<Vae, Error> {
        let post_quant = l.lin("post_quant_conv")?;
        let conv_in = l.conv("decoder.conv_in")?;
        let mid1 = Res::load(l, "decoder.mid.block_1", true)?;
        let attn_norm = l.norm("decoder.mid.attn_1.norm")?;
        let q = l.lin("decoder.mid.attn_1.q")?;
        let k = l.lin("decoder.mid.attn_1.k")?;
        let v = l.lin("decoder.mid.attn_1.v")?;
        let proj = l.lin("decoder.mid.attn_1.proj_out")?;
        let mid2 = Res::load(l, "decoder.mid.block_2", true)?;
        let mut levels = 0;
        while l.has(&alloc::format!("decoder.up.{levels}.block.0.conv1.weight")) {
            levels += 1;
        }
        if levels == 0 {
            return Err("the checkpoint has no VAE decoder".into());
        }
        let mut up = Vec::new();
        for lv in (0..levels).rev() {
            let mut blocks = Vec::new();
            let mut j = 0;
            while l.has(&alloc::format!("decoder.up.{lv}.block.{j}.conv1.weight")) {
                blocks.push(Res::load(l, &alloc::format!("decoder.up.{lv}.block.{j}"), true)?);
                j += 1;
            }
            let ups = if l.has(&alloc::format!("decoder.up.{lv}.upsample.conv.weight")) { Some(l.conv(&alloc::format!("decoder.up.{lv}.upsample.conv"))?) } else { None };
            up.push((blocks, ups));
        }
        let norm_out = l.norm("decoder.norm_out")?;
        let conv_out = l.conv("decoder.conv_out")?;
        Ok(Vae { post_quant, conv_in, mid1, attn_norm, q, k, v, proj, mid2, up, norm_out, conv_out })
    }

    /// latent (h x w x 4, as the sampler leaves it) -> RGB bytes (8h x 8w x 3).
    pub fn decode(&self, e: &mut Eng, z: &Act) -> (alloc::vec::Vec<u8>, usize, usize) {
        let mut zs = z.copy();
        for p in zs.pieces_mut() {
            for v in p.iter_mut() {
                *v /= SCALE;
            }
        }
        let z = e.linear(&self.post_quant, &zs, None);
        drop(zs);
        let h = e.conv3(&self.conv_in, In { x: &z, up: false, aff: None, silu: false }, 1, None, None);
        drop(z);
        let mut h = e.res(&self.mid1, h, None);
        {
            let a = e.gn_affine(&h, &self.attn_norm, 32, 1e-6);
            let n = e.apply(&h, &a, false);
            let q = e.linear(&self.q, &n, None);
            let k = e.linear(&self.k, &n, None);
            let v = e.linear(&self.v, &n, None);
            drop(n);
            let att = e.attention(&q, &k, &v, 1, false);
            drop((q, k, v));
            h = e.linear(&self.proj, &att, Some(h));
        }
        let mut h = e.res(&self.mid2, h, None);
        for (blocks, ups) in &self.up {
            for b in blocks {
                h = e.res(b, h, None);
                crate::kern::pump();
            }
            if let Some(c) = ups {
                h = e.conv3(c, In { x: &h, up: true, aff: None, silu: false }, 1, None, None);
            }
        }
        let a = e.gn_affine(&h, &self.norm_out, 32, 1e-6);
        let img = e.conv3(&self.conv_out, In { x: &h, up: false, aff: Some(&a), silu: true }, 1, None, None);
        drop(h);
        let (ih, iw) = (img.h, img.w);
        let mut out = alloc::vec![0u8; ih * iw * 3];
        for p in 0..ih * iw {
            let t = img.tok(p);
            for c in 0..3 {
                let v = ((t[c] / 2.0 + 0.5).clamp(0.0, 1.0) * 255.0) as f32;
                out[p * 3 + c] = libm::roundf(v).clamp(0.0, 255.0) as u8;
            }
        }
        (out, iw, ih)
    }
}
