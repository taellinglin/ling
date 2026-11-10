//! Smoke test without the interpreter:
//!   cargo run -p ling-diffusion --release --features cuda --example gen -- <checkpoint> <clip_dir> <outdir> "<prompt>" [steps] [sampler] [w] [h]
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let get = |i: usize, d: &str| a.get(i).cloned().unwrap_or_else(|| d.to_string());
    ling_diffusion::set_config(&serde_json::json!({"clip_dir": get(2, "models/clip"), "outdir": get(3, "out"), "device": std::env::var("SD_DEVICE").unwrap_or("cuda:0".into())}).to_string());
    let body = serde_json::json!({
        "checkpoint": get(1, ""),
        "prompt": get(4, "a cat"),
        "negative_prompt": "lowres, blurry",
        "steps": get(5, "20").parse::<f64>().unwrap(),
        "sampler_name": get(6, "Euler a"),
        "width": get(7, "512").parse::<f64>().unwrap(),
        "height": get(8, "512").parse::<f64>().unwrap(),
        "seed": 1234,
        "cfg_scale": 7,
    });
    let t0 = std::time::Instant::now();
    let id = ling_diffusion::submit(&body.to_string());
    let mut last = String::new();
    while !ling_diffusion::wait(&id, 1000) {
        let s = ling_diffusion::status(&id);
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        let line = format!("{} {} {}/{}", v["state"], v["message"], v["step"], v["steps"]);
        if line != last {
            eprintln!("[{:5.1}s] {line}", t0.elapsed().as_secs_f64());
            last = line;
        }
    }
    println!("{}", ling_diffusion::status(&id));
    eprintln!("total {:.1}s", t0.elapsed().as_secs_f64());
}
