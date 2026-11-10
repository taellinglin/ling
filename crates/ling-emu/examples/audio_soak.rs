//! Real-time audio soak: run a ROM through `ling_emu::frame()` with the host
//! loop paced at a chosen rate (volume 0, but the device really pulls), then
//! report underruns/overruns.   audio_soak rom [host_fps] [seconds]
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let fps: f64 = a.get(2).map(|s| s.parse().unwrap()).unwrap_or(60.0);
    let secs: f64 = a.get(3).map(|s| s.parse().unwrap()).unwrap_or(20.0);
    ling_emu::audio::set_volume(0.0);
    ling_emu::load(&std::fs::read(&a[1]).unwrap()).unwrap();
    let t0 = Instant::now();
    let tick = Duration::from_secs_f64(1.0 / fps);
    let mut next = t0;
    let mut ticks = 0u64;
    let (mut u0, mut o0) = (0, 0);
    while t0.elapsed().as_secs_f64() < secs {
        ling_emu::frame();
        ticks += 1;
        if ticks == (fps * 3.0) as u64 {
            // ignore the start-up transient
            u0 = ling_emu::audio::UNDERRUNS.load(Ordering::Relaxed);
            o0 = ling_emu::audio::OVERRUNS.load(Ordering::Relaxed);
        }
        next += tick;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        }
    }
    println!(
        "host {fps} fps for {secs} s: underrun callbacks {} · overrun drops {} · live {} · fill {:.0} ms",
        ling_emu::audio::UNDERRUNS.load(Ordering::Relaxed) - u0,
        ling_emu::audio::OVERRUNS.load(Ordering::Relaxed) - o0,
        ling_emu::audio::is_live(),
        ling_emu::audio::fill_secs() * 1000.0
    );
    ling_emu::unload();
}
