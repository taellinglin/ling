//! Probe tool: boot any N64 ROM headless, report where it gets.
//!
//!     cargo run --release -p ling-emu --example probe -- rom.z64 [frames] [out.png]

use ling_emu::n64::N64;
use ling_emu::Core;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let rom = args.get(1).expect("usage: probe rom.z64 [frames] [out.png]");
    let frames: u32 = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(60);
    let bytes = std::fs::read(rom).expect("read rom");
    let mut n64 = N64::boot(&bytes).expect("boot");
    let wav = std::env::var("N64_WAV").ok();
    if wav.is_some() {
        ling_emu::audio::start_capture();
    }
    // --tap NAME@frame,...  (A B Z START UP DOWN LEFT RIGHT L R CU CD CL CR), held 6 frames
    let names = ["A", "B", "Z", "START", "UP", "DOWN", "LEFT", "RIGHT", "L", "R", "CU", "CD", "CL", "CR"];
    let mut taps: Vec<(usize, u32)> = Vec::new();
    if let Some(i) = args.iter().position(|a| a == "--tap") {
        for t in args[i + 1].split(',') {
            let (nm, f) = t.split_once('@').unwrap();
            let idx = names.iter().position(|b| b.eq_ignore_ascii_case(nm)).expect("button");
            taps.push((idx, f.parse().unwrap()));
        }
    }
    for f in 0..frames {
        for btn in 0..14 {
            let down = taps.iter().any(|&(b, at)| b == btn && f >= at && f < at + 6);
            n64.set_button(0, btn, down);
        }
        n64.run_frame();
        if f % 30 == 29 {
            println!("frame {:>4}: pc={:08X} status: {}", f + 1, n64.cpu.pc, n64.status());
        }
        if n64.cpu.stuck.is_some() {
            break;
        }
    }
    println!("final: {}", n64.status());
    if let (Some(path), Some((rate, smp))) = (wav, ling_emu::audio::take_capture()) {
        let peak = smp.iter().map(|&(l, r)| (l as i32).abs().max((r as i32).abs())).max().unwrap_or(0);
        let mut b = Vec::new();
        let n = (smp.len() * 4) as u32;
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + n).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * 4).to_le_bytes());
        b.extend_from_slice(&4u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&n.to_le_bytes());
        for (l, r) in &smp {
            b.extend_from_slice(&l.to_le_bytes());
            b.extend_from_slice(&r.to_le_bytes());
        }
        std::fs::write(&path, b).unwrap();
        println!(
            "audio: {} samples @ {rate} Hz = {:.2} s (expected {:.2} s) · peak {peak}",
            smp.len(),
            smp.len() as f64 / rate.max(1) as f64,
            frames as f64 / 60.0
        );
    }
    let ms = |ns: u128| ns as f64 / 1e6 / frames.max(1) as f64;
    println!(
        "per frame: cpu(incl rsp+rdp) {:.2} ms · rsp {:.2} ms · rdp {:.2} ms",
        ms(n64.prof_cpu_ns), ms(n64.bus.prof_rsp_ns), ms(n64.bus.prof_rdp_ns)
    );
    // dump the instruction words around the parked PC (before fb borrow)
    let pcv = n64.cpu.pc;
    let pc = pcv & 0x1FFF_FFFF;
    for k in 0..16u32 {
        let a = (pc.wrapping_sub(32)).wrapping_add(k * 4);
        let w = n64.bus.read32(a);
        if k % 4 == 0 {
            println!();
            print!("  {:08X}: ", 0x8000_0000u32.wrapping_add(a));
        }
        print!("{w:08X} ");
    }
    println!();
    let (fb, w, h) = n64.framebuffer();
    println!("vi: {}x{} ctrl={:#X} origin={:#X}", w, h, n64.bus.vi_ctrl, n64.bus.vi_origin);
    println!(
        "exc={} eret={} vi_clears={}",
        n64.cpu.exc_count, n64.cpu.eret_count, n64.bus.vi_clear_count
    );
    for t in &n64.task_log {
        println!("task: {t}");
    }
    println!(
        "pi_dma={} si_rd={} si_wr={} challenges={}",
        n64.bus.pi_dma_count, n64.bus.si_rd_count, n64.bus.si_wr_count, n64.bus.challenge_count
    );
    println!(
        "mi_mask={:#X} mi_intr={:#X} cp0_status={:#X} cp0_cause={:#X} sp_pc={:#X} sp_status={:#X}",
        n64.bus.mi_mask,
        n64.bus.mi_intr,
        n64.cpu.cp0[12],
        n64.cpu.cp0[13],
        n64.bus.sp_pc,
        n64.bus.sp_status_read()
    );
    // locate the VADD test's VAQUAD/VDQUAD buffers: they follow the unique
    // VADDCVACHECKD pattern in RDRAM (vector-test debugging)
    let needle: Vec<u8> = [0xFFDCu16, 0xBB98, 0x7754, 0x3310, 0xEECC, 0xAA88, 0x6644, 0x2200]
        .iter()
        .flat_map(|v| v.to_be_bytes())
        .collect();
    let mut hits = Vec::new();
    for i in 0..n64.bus.rdram.len().saturating_sub(16) {
        if n64.bus.rdram[i..i + 16] == needle[..] {
            hits.push(i);
        }
    }
    println!("needle hits: {hits:X?}");
    if let Some(&last) = hits.last() {
        let va = last + 16; // VAQUAD follows VADDCVACHECKD's last quad
        print!("VAQUAD..VDQUAD + VCOVCC area @ {va:06X}:");
        for k in 0..72usize {
            if k % 16 == 0 {
                println!();
                print!("  +{k:02X}: ");
            }
            print!("{:02X} ", n64.bus.rdram[va + k]);
        }
        println!();
    }
    // DMEM snapshot (vector-test debugging)
    print!("dmem[0x00..0x48]:");
    for k in 0..0x48usize {
        if k % 16 == 0 {
            println!();
            print!("  {k:02X}: ");
        }
        print!("{:02X} ", n64.bus.dmem[k]);
    }
    println!();
    if let Some(out) = args.get(3) {
        write_png(std::path::Path::new(out), fb, w, h);
        println!("wrote {out}");
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for n in 0..256u32 {
        let mut c = n;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB88320 ^ (c >> 1) } else { c >> 1 };
        }
        table[n as usize] = c;
    }
    let mut c = 0xFFFFFFFFu32;
    for &b in data {
        c = table[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFFFFFF
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, tag: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(tag);
    out.extend_from_slice(body);
    let mut c = tag.to_vec();
    c.extend_from_slice(body);
    out.extend_from_slice(&crc32(&c).to_be_bytes());
}

fn write_png(path: &std::path::Path, fb: &[u32], w: usize, h: usize) {
    let mut raw = Vec::with_capacity(h * (1 + w * 3));
    for y in 0..h {
        raw.push(0u8);
        for x in 0..w {
            let p = fb[y * w + x];
            raw.push((p >> 16) as u8);
            raw.push((p >> 8) as u8);
            raw.push(p as u8);
        }
    }
    let mut z = vec![0x78u8, 0x01];
    for (i, block) in raw.chunks(65535).enumerate() {
        let last = (i + 1) * 65535 >= raw.len();
        z.push(if last { 1 } else { 0 });
        let len = block.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(block);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &z);
    chunk(&mut png, b"IEND", &[]);
    std::fs::write(path, png).expect("write png");
}
