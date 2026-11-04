//! Dump a PNG every N frames: probe_seq rom.z64 frames every out_prefix
use ling_emu::n64::N64;
use ling_emu::Core;
#[path = "common/png_util.rs"]
mod png_util;
fn a_args() -> Vec<String> { std::env::args().collect() }
fn a_out(a: &[String]) -> String { a[4].clone() }
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).unwrap();
    let frames: u32 = a[2].parse().unwrap();
    let every: u32 = a[3].parse().unwrap();
    let mut n = N64::boot(&bytes).unwrap();
    if std::env::var_os("N64_TEXSTATS").is_some() {
        ling_emu::n64::rdp::STATS_ON.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    let mut taps: Vec<(usize, u32)> = Vec::new();
    if let Some(t) = a.get(5) {
        for t in t.split(',') {
            let (b, f) = t.split_once('@').unwrap();
            taps.push((b.parse().unwrap(), f.parse().unwrap()));
        }
    }
    let t0 = std::time::Instant::now();
    for f in 1..=frames {
        for btn in 0..14 {
            n.set_button(0, btn, taps.iter().any(|&(b, at)| b == btn && f >= at && f < at + 6));
        }
        if let Some(from) = std::env::var("N64_STATS_FROM").ok().and_then(|v| v.parse::<u32>().ok()) {
            ling_emu::n64::rdp::STATS_ON.store(f >= from, std::sync::atomic::Ordering::Relaxed);
        }
        n.run_frame();
        if std::env::var_os("N64_VILOG").is_some() {
            let cur = (n.bus.vi_width, n.bus.vi_regs[0x24 >> 2], n.bus.vi_regs[0x30 >> 2], n.bus.vi_regs[0x28 >> 2], n.bus.vi_regs[0x34 >> 2], n.bus.vi_ctrl);
            static mut LAST: (u32, u32, u32, u32, u32, u32) = (0, 0, 0, 0, 0, 0);
            unsafe {
                if cur != LAST {
                    LAST = cur;
                    eprintln!("f={f} vi width={} h_video={:#X} x_scale={:#X} v_video={:#X} y_scale={:#X} ctrl={:#X} -> out {:?}", cur.0, cur.1, cur.2, cur.3, cur.4, cur.5, { let (_, w, h) = n.framebuffer(); (w, h) });
                }
            }
        }
        if std::env::var_os("N64_STATS_FROM").is_some() && f % 10 == 0 {
            eprintln!("f={f} fog colour {:?}", n.bus.rdp.fog_color());
        }
        if f % every == 0 {
            let (fb, w, h) = n.framebuffer();
            png_util::write_png(&format!("{}_{f}.png", a[4]), fb, w, h);
        }
    }
    if let Ok(spec) = std::env::var("N64_DUMPTEX") {
        // addr,width,height (8-bit intensity) → PNG
        let v: Vec<usize> = spec.split(',').map(|x| usize::from_str_radix(x.trim_start_matches("0x"), 16).unwrap()).collect();
        let (a, w, h) = (v[0], v[1], v[2]);
        let px: Vec<u32> = (0..w * h).map(|i| { let b = n.bus.rdram[a + i] as u32; (b << 16) | (b << 8) | b }).collect();
        png_util::write_png(&format!("{}_tex.png", a_out(&a_args())), &px, w, h);
    }
    println!("{:.2} ms/frame", t0.elapsed().as_secs_f64() * 1000.0 / frames as f64);
    if std::env::var_os("N64_TEXSTATS").is_some() {
        let f: Vec<u64> = ling_emu::n64::rdp::FOG_STATS.iter().map(|c| c.load(std::sync::atomic::Ordering::Relaxed)).collect();
        println!("fog factor histogram (0..7 by 32s): {:?} · fog-selected-but-unused: {}", &f[..8], f[8]);
        let m: Vec<u64> = ling_emu::n64::rdp::MISC_STATS.iter().map(|c| c.load(std::sync::atomic::Ordering::Relaxed)).collect();
        println!("force-blend additive {} · force-blend other {} · 2nd-cycle texel refs {}", m[0], m[1], m[2]);
        println!("color image writes by size: 4b {} · 8b {} · 16b {} · 32b {}", m[3] & 0xFFFF, (m[3] >> 16) & 0xFFFF, (m[3] >> 32) & 0xFFFF, m[3] >> 48);
        println!("framebuffer-sourced texture loads: {}", ling_emu::n64::rdp::FB_STATS[0].load(std::sync::atomic::Ordering::Relaxed));
        let ops: Vec<String> = ling_emu::n64::rdp::OP_STATS.iter().enumerate()
            .filter(|(_, c)| c.load(std::sync::atomic::Ordering::Relaxed) > 0)
            .map(|(i, c)| format!("{:02X}:{}", i, c.load(std::sync::atomic::Ordering::Relaxed))).collect();
        println!("rdp opcodes: {}", ops.join(" "));
        let l: Vec<u64> = ling_emu::n64::rdp::LOD_STATS.iter().map(|c| c.load(std::sync::atomic::Ordering::Relaxed)).collect();
        println!("textured px: lod off {} · detail {} · sharpen {} · plain mip {}", l[0], l[1], l[2], l[3]);
        let fmts = ["RGBA", "YUV", "CI", "IA", "I", "?5", "?6", "?7"];
        for (k, c) in ling_emu::n64::rdp::TEX_STATS.iter().enumerate() {
            let c = c.load(std::sync::atomic::Ordering::Relaxed);
            if c > 0 {
                println!("tlut={} {}{} : {c}", k / 32, fmts[k % 8], [4, 8, 16, 32][(k / 8) % 4]);
            }
        }
    }
}
