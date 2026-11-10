//! Component checks for the Qwen-Image-Edit port against the PyTorch reference
//! (tools/qwen_reference.py in LingDiffusion writes the same tensors from diffusers/transformers).
//!   qtest vae   <model_dir> <image> <out.safetensors> [device]
//!   qtest vl    <model_dir> <image> "<prompt>" <out.safetensors> [device]
//!   qtest edit  <model_dir> <image> "<prompt>" <out.png> [steps] [mode lightning8|full] [mp]
use candle_core::{DType, Device, Tensor};
use ling_diffusion::qwen;
use std::collections::HashMap;

fn dev(s: Option<&String>) -> Device {
    match s.map(|s| s.as_str()) {
        Some("cpu") => Device::Cpu,
        Some(x) => Device::new_cuda(x.trim_start_matches("cuda:").parse().unwrap_or(0)).unwrap(),
        None => Device::new_cuda(1).unwrap_or(Device::Cpu),
    }
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let t0 = std::time::Instant::now();
    match a[1].as_str() {
        "vae" => {
            let d = dev(a.get(5));
            let vae = qwen::vae::Vae::load(&format!("{}/vae/diffusion_pytorch_model.safetensors", a[2]), &d)?;
            let img = image::open(&a[3])?.to_rgb8();
            let (w, h) = (img.width() as usize / 16 * 16, img.height() as usize / 16 * 16);
            let img = image::imageops::crop_imm(&img, 0, 0, w as u32, h as u32).to_image();
            let x = Tensor::from_slice(img.as_raw(), (h, w, 3), &d)?.permute((2, 0, 1))?.to_dtype(DType::F32)?;
            let x = ((x / 127.5)? - 1.0)?.unsqueeze(0)?;
            let z = vae.encode(&x)?;
            let y = vae.decode(&z)?;
            let mut m = HashMap::new();
            m.insert("latent".to_string(), z.to_device(&Device::Cpu)?);
            m.insert("decoded".to_string(), y.to_device(&Device::Cpu)?);
            candle_core::safetensors::save(&m, &a[4])?;
            println!("vae ok {:?} -> {:?} in {:.1}s", x.dims(), z.dims(), t0.elapsed().as_secs_f64());
        }
        "vl" => {
            let d = dev(a.get(6));
            let vl = qwen::vl::Vl::load(&format!("{}/text_encoder", a[2]), &format!("{}/processor/tokenizer.json", a[2]), &d, false, &|s| eprintln!("{s}"))?;
            eprintln!("loaded in {:.1}s", t0.elapsed().as_secs_f64());
            let img = image::open(&a[3])?.to_rgb8();
            let (w, h) = qwen::vl::smart_size(img.width() as usize, img.height() as usize);
            let rgb = image::imageops::resize(&img, w as u32, h as u32, image::imageops::FilterType::CatmullRom).into_raw();
            let t1 = std::time::Instant::now();
            let out = vl.encode(&a[4], &rgb, w, h)?;
            let mut m = HashMap::new();
            m.insert("hidden".to_string(), out.to_device(&Device::Cpu)?);
            m.insert("vis".to_string(), vl.vision(&rgb, w, h)?.to_device(&Device::Cpu)?);
            m.insert("pixels".to_string(), Tensor::from_vec(rgb, (h, w, 3), &Device::Cpu)?);
            candle_core::safetensors::save(&m, &a[5])?;
            println!("vl ok {:?} ({w}x{h}) in {:.2}s", out.dims(), t1.elapsed().as_secs_f64());
        }
        "edit" => {
            let mode = a.get(7).cloned().unwrap_or("lightning8".into());
            let dir = a[2].clone();
            let mut loras = Vec::new();
            if mode.starts_with("lightning") {
                loras.push((format!("{dir}/lightning/Qwen-Image-Edit-2511-Lightning-8steps-V1.0-bf16.safetensors"), 1.0));
            }
            let o = qwen::EditOpts { dir: dir.clone(), gguf: "qwen-image-edit-2511-Q4_K_M.gguf".into(), loras, device: dev(Some(&"cuda:1".into())), encoder_device: dev(Some(&"cuda:0".into())), encoder_q8: true };
            let m = qwen::EditModel::load(&o, &|s| eprintln!("[{:6.1}s] {s}", t0.elapsed().as_secs_f64())).map_err(anyhow::Error::msg)?;
            eprintln!("loaded in {:.1}s", t0.elapsed().as_secs_f64());
            let img = image::open(&a[3])?.to_rgb8();
            let steps = a.get(6).and_then(|s| s.parse().ok()).unwrap_or(8);
            let mp: f64 = a.get(8).and_then(|s| s.parse().ok()).unwrap_or(1.0);
            let p = qwen::EditParams { prompt: a[4].clone(), negative: " ".into(), steps, true_cfg: if mode == "full" { 4.0 } else { 1.0 }, seed: 42, area: mp * 1024.0 * 1024.0, lightning: mode.starts_with("lightning") };
            let t1 = std::time::Instant::now();
            let (rgb, w, h) = m
                .edit(&img, &p, &mut |i, n| {
                    eprintln!("[{:6.1}s] step {i}/{n}", t1.elapsed().as_secs_f64());
                    true
                }, &|s| eprintln!("[{:6.1}s] {s}", t1.elapsed().as_secs_f64()))
                .map_err(anyhow::Error::msg)?;
            image::RgbImage::from_raw(w as u32, h as u32, rgb).unwrap().save(&a[5])?;
            println!("edit ok {w}x{h} in {:.1}s", t1.elapsed().as_secs_f64());
        }
        "dit" => {
            // qtest dit <model_dir> <out.safetensors>  (LING_QWEN_MAX_BLOCKS limits the depth)
            let d = dev(Some(&"cuda:1".into()));
            let dit = qwen::mmdit::Mmdit::load(&format!("{}/transformer/qwen-image-edit-2511-Q4_K_M.gguf", a[2]), &d, &[], &|s| eprintln!("{s}"))?;
            let (gh, gw) = (16usize, 12usize);
            let n = gh * gw;
            let mut rng = <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(7);
            let x = Tensor::from_vec(ling_diffusion::pipeline::randn(&mut rng, 2 * n * 64), (1, 2 * n, 64), &d)?;
            let ctx = (Tensor::from_vec(ling_diffusion::pipeline::randn(&mut rng, 37 * 3584), (1, 37, 3584), &d)? * 3.0)?;
            let rope = qwen::mmdit::Rope::new(&[(gh, gw), (gh, gw)], 37, &d)?;
            let t1 = std::time::Instant::now();
            let v = dit.forward(&x, &ctx, 0.7, n, &rope)?;
            eprintln!("forward {:.2}s", t1.elapsed().as_secs_f64());
            let mut m = HashMap::new();
            m.insert("x".to_string(), x.to_device(&Device::Cpu)?);
            m.insert("ctx".to_string(), ctx.to_device(&Device::Cpu)?);
            m.insert("v".to_string(), v.to_device(&Device::Cpu)?);
            candle_core::safetensors::save(&m, &a[3])?;
            println!("dit ok {:?} zero_cond_t={}", v.dims(), dit.zero_cond_t);
        }
        "bench" => {
            // qtest bench <model_dir>: where a transformer step's time goes
            use candle_core::quantized::gguf_file;
            let d = dev(Some(&"cuda:1".into()));
            let sync = |d: &Device| { let _ = d.synchronize(); };
            let path = format!("{}/transformer/qwen-image-edit-2511-Q4_K_M.gguf", a[2]);
            let mut f = std::fs::File::open(&path)?;
            let ct = gguf_file::Content::read(&mut f)?;
            for name in ["transformer_blocks.0.img_mlp.net.0.proj.weight", "transformer_blocks.0.attn.to_q.weight", "transformer_blocks.0.img_mlp.net.2.weight"] {
                let q = ct.tensor(&mut f, name, &d)?;
                sync(&d);
                let t = std::time::Instant::now();
                for _ in 0..20 { let _w = q.dequantize_f16(&d)?; }
                sync(&d);
                let deq = t.elapsed().as_secs_f64() * 1000.0 / 20.0;
                let w = q.dequantize_f16(&d)?;
                let t = std::time::Instant::now();
                for _ in 0..20 { let _w = w.to_dtype(DType::BF16)?; }
                sync(&d);
                let conv = t.elapsed().as_secs_f64() * 1000.0 / 20.0;
                let wb = w.to_dtype(DType::BF16)?;
                let (o, i) = wb.dims2()?;
                let x = Tensor::randn(0f32, 1f32, (8300, i), &d)?.to_dtype(DType::BF16)?;
                let _ = x.matmul(&wb.t()?)?; sync(&d);
                let t = std::time::Instant::now();
                for _ in 0..20 { let _y = x.matmul(&wb.t()?)?; }
                sync(&d);
                let mm = t.elapsed().as_secs_f64() * 1000.0 / 20.0;
                let xh = x.to_dtype(DType::F16)?;
                let _ = xh.matmul(&w.t()?)?; sync(&d);
                let t = std::time::Instant::now();
                for _ in 0..20 { let _y = xh.matmul(&w.t()?)?; }
                sync(&d);
                let mmh = t.elapsed().as_secs_f64() * 1000.0 / 20.0;
                let wc = wb.t()?.contiguous()?;
                let t = std::time::Instant::now();
                for _ in 0..20 { let _y = x.matmul(&wc)?; }
                sync(&d);
                let mmc = t.elapsed().as_secs_f64() * 1000.0 / 20.0;
                println!("{name} {:?} {o}x{i}: dequant {deq:.2} ms, f16->bf16 {conv:.2} ms, bf16 matmul (w.t) {mm:.2} ms, f16 matmul {mmh:.2} ms, bf16 matmul (contig) {mmc:.2} ms", q.dtype());
            }
            // attention for one block at this size
            let l = 8300;
            let q = Tensor::randn(0f32, 1f32, (1, 24, l, 128), &d)?.to_dtype(DType::BF16)?;
            sync(&d);
            let t = std::time::Instant::now();
            for _ in 0..5 { let _o = qwen::ops::attention(&q, &q, &q, None, 0.088)?; }
            sync(&d);
            println!("attention L={l}: {:.2} ms", t.elapsed().as_secs_f64() * 1000.0 / 5.0);
            let xr = Tensor::randn(0f32, 1f32, (1, l, 24, 128), &d)?;
            let cs = Tensor::randn(0f32, 1f32, (l, 64), &d)?;
            sync(&d);
            let t = std::time::Instant::now();
            for _ in 0..10 { let _o = qwen::ops::rope_pairs(&xr, &cs, &cs)?; }
            sync(&d);
            println!("rope_pairs: {:.2} ms", t.elapsed().as_secs_f64() * 1000.0 / 10.0);
            let h = Tensor::randn(0f32, 1f32, (1, l, 3072), &d)?;
            sync(&d);
            let t = std::time::Instant::now();
            for _ in 0..10 { let _o = qwen::ops::layer_norm(&h, 1e-6)?; }
            sync(&d);
            println!("layer_norm: {:.2} ms", t.elapsed().as_secs_f64() * 1000.0 / 10.0);
        }
        _ => println!("qtest vae|vl|dit|edit|bench ..."),
    }
    Ok(())
}
