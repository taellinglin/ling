//! ESRGAN / Real-ESRGAN upscalers (the RRDBNet family: ESRGAN_4x, RealESRGAN_x4plus,
//! RealESRGAN_x4plus_anime_6B, 4x-AnimeSharp, 4x-UltraSharp, the x2 models) for the hires fix and
//! the upscale API. Reads the WebUI's .pth files as they are (old ESRGAN key names, the new
//! `body.N.rdbM` names, `params_ema` wrappers) and runs in tiles so any size fits.

use candle_core::{DType, Device, Tensor};
use std::collections::HashMap;
use std::path::Path;

type R<T> = std::result::Result<T, String>;
fn e<E: std::fmt::Display>(x: E) -> String {
    x.to_string()
}

struct Conv {
    w: Tensor,
    b: Tensor,
}

impl Conv {
    fn fwd(&self, x: &Tensor) -> candle_core::Result<Tensor> {
        let k = self.w.dim(2)?;
        x.conv2d(&self.w, k / 2, 1, 1, 1)?.broadcast_add(&self.b)
    }
}

fn lrelu(x: &Tensor) -> candle_core::Result<Tensor> {
    candle_nn::ops::leaky_relu(x, 0.2)
}

pub struct Esrgan {
    pub name: String,
    /// the model's own factor (4 for the x4 models)
    pub scale: usize,
    unshuffle: usize,
    first: Conv,
    /// blocks -> 3 dense blocks -> 5 convs
    body: Vec<[[Conv; 5]; 3]>,
    conv_body: Conv,
    ups: Vec<Conv>,
    hr: Conv,
    last: Conv,
    pub device: Device,
    dtype: DType,
}

/// Old ESRGAN names (model.N / RRDB_trunk / upconvN / HRconv) -> the Real-ESRGAN names.
fn normalize(raw: Vec<(String, Tensor)>) -> HashMap<String, Tensor> {
    let mut m = HashMap::new();
    let seq_old = raw.iter().any(|(k, _)| k.starts_with("model.0."));
    // in the sequential layout, the convs after the trunk are: ups..., hr, last
    let mut tail: Vec<usize> = Vec::new();
    if seq_old {
        for (k, _) in &raw {
            if let Some(r) = k.strip_prefix("model.") {
                if let Some(i) = r.split('.').next().and_then(|s| s.parse::<usize>().ok()) {
                    if i > 1 && r.ends_with(".weight") && !tail.contains(&i) {
                        tail.push(i);
                    }
                }
            }
        }
        tail.sort();
    }
    for (k, t) in raw {
        let k = k.strip_prefix("module.").unwrap_or(&k).to_string();
        let nk = if seq_old {
            let r = &k["model.".len().min(k.len())..];
            let parts: Vec<&str> = r.split('.').collect();
            let suffix = parts.last().copied().unwrap_or("");
            if parts.first() == Some(&"0") {
                format!("conv_first.{suffix}")
            } else if parts.first() == Some(&"1") && parts.get(1) == Some(&"sub") {
                // model.1.sub.I.RDBj.convk.0.weight  |  model.1.sub.N.weight (the trunk conv)
                if parts.len() >= 6 && parts[3].starts_with("RDB") {
                    format!("body.{}.rdb{}.conv{}.{suffix}", parts[2], &parts[3][3..], &parts[4][4..])
                } else {
                    format!("conv_body.{suffix}")
                }
            } else if let Some(i) = parts.first().and_then(|s| s.parse::<usize>().ok()) {
                let pos = tail.iter().position(|x| *x == i).unwrap_or(0);
                let n = tail.len();
                if pos + 2 == n {
                    format!("conv_hr.{suffix}")
                } else if pos + 1 == n {
                    format!("conv_last.{suffix}")
                } else {
                    format!("conv_up{}.{suffix}", pos + 1)
                }
            } else {
                k.clone()
            }
        } else {
            k.replace("RRDB_trunk.", "body.")
                .replace(".RDB1.", ".rdb1.")
                .replace(".RDB2.", ".rdb2.")
                .replace(".RDB3.", ".rdb3.")
                .replace("trunk_conv.", "conv_body.")
                .replace("upconv1.", "conv_up1.")
                .replace("upconv2.", "conv_up2.")
                .replace("upconv3.", "conv_up3.")
                .replace("HRconv.", "conv_hr.")
        };
        m.insert(nk, t);
    }
    m
}

fn read_weights(path: &str) -> R<Vec<(String, Tensor)>> {
    let ext = Path::new(path).extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
    if ext == "safetensors" {
        let m = candle_core::safetensors::load(path, &Device::Cpu).map_err(e)?;
        return Ok(m.into_iter().collect());
    }
    for key in [Some("params_ema"), Some("params"), None] {
        if let Ok(v) = candle_core::pickle::read_all_with_key(path, key) {
            if !v.is_empty() {
                return Ok(v);
            }
        }
    }
    let legacy = std::fs::File::open(path).ok().and_then(|mut f| {
        let mut m = [0u8; 2];
        std::io::Read::read_exact(&mut f, &mut m).ok().map(|_| m != *b"PK")
    });
    if legacy == Some(true) {
        return Err(format!("{path} is in PyTorch's old (pre-1.6) format; convert it once with: python tools/convert_upscaler.py \"{path}\""));
    }
    Err(format!("could not read {path} (not a PyTorch state dict)"))
}

impl Esrgan {
    pub fn load(path: &str, dev: &Device) -> R<Self> {
        let w = normalize(read_weights(path)?);
        let dtype = if dev.is_cpu() { DType::F32 } else { DType::F16 };
        let conv = |n: &str| -> R<Conv> {
            let wt = w.get(&format!("{n}.weight")).ok_or_else(|| format!("{n}.weight missing: not an ESRGAN (RRDBNet) model"))?;
            let b = w.get(&format!("{n}.bias")).ok_or_else(|| format!("{n}.bias missing"))?;
            Ok(Conv { w: wt.to_dtype(dtype).map_err(e)?.to_device(dev).map_err(e)?, b: b.to_dtype(dtype).map_err(e)?.to_device(dev).map_err(e)?.reshape((1, b.dim(0).map_err(e)?, 1, 1)).map_err(e)? })
        };
        let first = conv("conv_first")?;
        let in_ch = first.w.dim(1).map_err(e)?;
        let unshuffle = match in_ch {
            3 => 1,
            12 => 2,
            48 => 4,
            c => return Err(format!("unsupported input channels {c}")),
        };
        let mut nb = 0;
        while w.contains_key(&format!("body.{nb}.rdb1.conv1.weight")) {
            nb += 1;
        }
        if nb == 0 {
            return Err("no RRDB blocks found: only ESRGAN / Real-ESRGAN (RRDBNet) models are supported".into());
        }
        let mut body = Vec::with_capacity(nb);
        for i in 0..nb {
            let rdb = |j: usize| -> R<[Conv; 5]> { Ok([conv(&format!("body.{i}.rdb{j}.conv1"))?, conv(&format!("body.{i}.rdb{j}.conv2"))?, conv(&format!("body.{i}.rdb{j}.conv3"))?, conv(&format!("body.{i}.rdb{j}.conv4"))?, conv(&format!("body.{i}.rdb{j}.conv5"))?]) };
            body.push([rdb(1)?, rdb(2)?, rdb(3)?]);
        }
        let mut ups = Vec::new();
        while w.contains_key(&format!("conv_up{}.weight", ups.len() + 1)) {
            ups.push(conv(&format!("conv_up{}", ups.len() + 1))?);
        }
        let scale = (1usize << ups.len()) / unshuffle;
        Ok(Self {
            name: Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string(),
            scale: scale.max(1),
            unshuffle,
            first,
            body,
            conv_body: conv("conv_body")?,
            ups,
            hr: conv("conv_hr")?,
            last: conv("conv_last")?,
            device: dev.clone(),
            dtype,
        })
    }

    /// x [1,3,h,w] in 0..1 -> [1,3,h*scale,w*scale]
    fn forward(&self, x: &Tensor) -> candle_core::Result<Tensor> {
        let mut x = x.to_dtype(self.dtype)?;
        if self.unshuffle > 1 {
            let (b, c, h, w) = x.dims4()?;
            let s = self.unshuffle;
            x = x.reshape((b, c, h / s, s, w / s, s))?.permute((0, 1, 3, 5, 2, 4))?.reshape((b, c * s * s, h / s, w / s))?;
        }
        let fea = self.first.fwd(&x)?;
        let mut t = fea.clone();
        for rrdb in &self.body {
            let inp = t.clone();
            for rdb in rrdb {
                let x0 = t.clone();
                let x1 = lrelu(&rdb[0].fwd(&x0)?)?;
                let x2 = lrelu(&rdb[1].fwd(&Tensor::cat(&[&x0, &x1], 1)?)?)?;
                let x3 = lrelu(&rdb[2].fwd(&Tensor::cat(&[&x0, &x1, &x2], 1)?)?)?;
                let x4 = lrelu(&rdb[3].fwd(&Tensor::cat(&[&x0, &x1, &x2, &x3], 1)?)?)?;
                let x5 = rdb[4].fwd(&Tensor::cat(&[&x0, &x1, &x2, &x3, &x4], 1)?)?;
                t = ((x5 * 0.2)? + x0)?;
            }
            t = ((t * 0.2)? + inp)?;
        }
        let mut f = (fea + self.conv_body.fwd(&t)?)?;
        for up in &self.ups {
            let (_, _, h, w) = f.dims4()?;
            f = lrelu(&up.fwd(&f.upsample_nearest2d(h * 2, w * 2)?)?)?;
        }
        self.last.fwd(&lrelu(&self.hr.fwd(&f)?)?)
    }

    /// One pass of the model over an RGB image, in tiles of `tile` px with `pad` px of context.
    pub fn upscale(&self, rgb: &[u8], w: usize, h: usize, tile: usize, cancel: &dyn Fn() -> bool) -> R<(Vec<u8>, usize, usize)> {
        let s = self.scale;
        let (ow, oh) = (w * s, h * s);
        let mut out = vec![0u8; ow * oh * 3];
        let pad = 16usize;
        let tile = tile.max(32);
        let src = Tensor::from_slice(rgb, (h, w, 3), &Device::Cpu).map_err(e)?.permute((2, 0, 1)).map_err(e)?.to_dtype(DType::F32).map_err(e)?;
        let src = (src / 255.0).map_err(e)?;
        let mut y = 0;
        while y < h {
            let th = tile.min(h - y);
            let mut x = 0;
            while x < w {
                if cancel() {
                    return Err("interrupted".into());
                }
                let tw = tile.min(w - x);
                // context around the tile, kept a multiple of the unshuffle factor
                let u = self.unshuffle;
                let y0 = y.saturating_sub(pad) / u * u;
                let x0 = x.saturating_sub(pad) / u * u;
                let mut y1 = (y + th + pad).min(h);
                let mut x1 = (x + tw + pad).min(w);
                y1 = y0 + ((y1 - y0) / u * u).max(u);
                x1 = x0 + ((x1 - x0) / u * u).max(u);
                let (y1, x1) = (y1.min(h), x1.min(w));
                let part = src.narrow(1, y0, y1 - y0).map_err(e)?.narrow(2, x0, x1 - x0).map_err(e)?;
                // a ragged edge (smaller than the unshuffle factor) is padded by replication
                let (ph, pw) = (y1 - y0, x1 - x0);
                let (ph2, pw2) = (ph.div_ceil(u) * u, pw.div_ceil(u) * u);
                let mut part = part.contiguous().map_err(e)?;
                if ph2 != ph {
                    part = part.pad_with_same(1, 0, ph2 - ph).map_err(e)?;
                }
                if pw2 != pw {
                    part = part.pad_with_same(2, 0, pw2 - pw).map_err(e)?;
                }
                let inp = part.unsqueeze(0).map_err(e)?.to_device(&self.device).map_err(e)?;
                let o = self.forward(&inp).map_err(e)?.squeeze(0).map_err(e)?;
                let o = (o.to_dtype(DType::F32).map_err(e)?.clamp(0f32, 1f32).map_err(e)? * 255.0).map_err(e)?.round().map_err(e)?;
                let o = o.narrow(1, (y - y0) * s, th * s).map_err(e)?.narrow(2, (x - x0) * s, tw * s).map_err(e)?;
                let o = o.permute((1, 2, 0)).map_err(e)?.to_dtype(DType::U8).map_err(e)?.to_device(&Device::Cpu).map_err(e)?.flatten_all().map_err(e)?.to_vec1::<u8>().map_err(e)?;
                for r in 0..th * s {
                    let dst = ((y * s + r) * ow + x * s) * 3;
                    out[dst..dst + tw * s * 3].copy_from_slice(&o[r * tw * s * 3..(r + 1) * tw * s * 3]);
                }
                x += tw;
            }
            y += th;
        }
        Ok((out, ow, oh))
    }

    /// Upscales to exactly (tw, th): model passes until the image is at least that large (like the
    /// WebUI), then a Lanczos resize to the target.
    pub fn upscale_to(&self, img: &image::RgbImage, tw: usize, th: usize, cancel: &dyn Fn() -> bool) -> R<image::RgbImage> {
        let mut cur = img.clone();
        let mut passes = 0;
        while ((cur.width() as usize) < tw || (cur.height() as usize) < th) && passes < 3 {
            let (rgb, w, h) = self.upscale(cur.as_raw(), cur.width() as usize, cur.height() as usize, 192, cancel)?;
            cur = image::RgbImage::from_raw(w as u32, h as u32, rgb).ok_or("upscale size")?;
            passes += 1;
            if self.scale == 1 {
                break;
            }
        }
        if cur.width() as usize == tw && cur.height() as usize == th {
            return Ok(cur);
        }
        Ok(image::imageops::resize(&cur, tw as u32, th as u32, image::imageops::FilterType::Lanczos3))
    }
}
