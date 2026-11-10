//! cargo run -p ling-diffusion --release --features cuda --example upscale -- <model.pth> <in.png> <out.png> [cuda:N|cpu] [factor]
use ling_diffusion::esrgan::Esrgan;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let dev = match a.get(4).map(|s| s.as_str()).unwrap_or("cuda:1") {
        "cpu" => candle_core::Device::Cpu,
        d => candle_core::Device::new_cuda(d.trim_start_matches("cuda:").parse()?)?,
    };
    let t = Instant::now();
    let m = Esrgan::load(&a[1], &dev).map_err(anyhow::Error::msg)?;
    println!("loaded {} (x{}) in {:.2}s", m.name, m.scale, t.elapsed().as_secs_f64());
    let img = image::open(&a[2])?.to_rgb8();
    let f: f64 = a.get(5).map(|s| s.parse().unwrap()).unwrap_or(m.scale as f64);
    let (tw, th) = ((img.width() as f64 * f) as usize, (img.height() as f64 * f) as usize);
    let t = Instant::now();
    let out = m.upscale_to(&img, tw, th, &|| false).map_err(anyhow::Error::msg)?;
    println!("{}x{} -> {}x{} in {:.2}s", img.width(), img.height(), out.width(), out.height(), t.elapsed().as_secs_f64());
    out.save(&a[3])?;
    Ok(())
}
