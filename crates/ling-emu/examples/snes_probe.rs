//! Headless SNES probe.
//!
//!     cargo run --release -p ling-emu --example snes_probe -- rom.sfc [frames] [out.png]
//!         [--tap START@120,A@200,...]   buttons: B Y SELECT START UP DOWN LEFT RIGHT A X L R

use ling_emu::snes::Snes;
use ling_emu::Core;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let rom = args.get(1).expect("usage: snes_probe rom.sfc [frames] [out.png] [--tap NAME@frame,...]");
    let frames: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(120);
    let out = args.get(3).filter(|s| !s.starts_with("--"));
    let mut taps: Vec<(usize, u32)> = Vec::new();
    if let Some(i) = args.iter().position(|a| a == "--tap") {
        for t in args[i + 1].split(',') {
            let (n, f) = t.split_once('@').unwrap();
            let idx = ["B", "Y", "SELECT", "START", "UP", "DOWN", "LEFT", "RIGHT", "A", "X", "L", "R"]
                .iter()
                .position(|b| b.eq_ignore_ascii_case(n))
                .expect("button name");
            taps.push((idx, f.parse().unwrap()));
        }
    }
    let bytes = std::fs::read(rom).expect("read rom");
    let mut snes = Snes::boot(&bytes).expect("boot");
    let wav = std::env::var("SNES_WAV").ok();
    if wav.is_some() {
        ling_emu::audio::start_capture();
    }
    println!("title: '{}' · {}", snes.title(), if snes.is_pal() { "PAL" } else { "NTSC" });
    for f in 0..frames {
        for btn in 0..12 {
            let down = taps.iter().any(|&(b, at)| b == btn && f >= at && f < at + 8);
            snes.set_button(0, btn, down);
        }
        snes.run_frame();
        if std::env::var("SNES_TRACE").is_ok() && f % 60 == 0 && f >= 1600 {
            println!("f={f:5} {}", snes.debug());
        }
        if std::env::var("SNES_MOD").is_ok() && f >= 1500 && f % 25 == 0 {
            println!(
                "f={f} module={:02X} sub={:02X} sub2={:02X} frame1A={:02X} inidisp13={:02X} irq-shadow ({}) 0x0200={:02X}",
                snes.wram(0x10), snes.wram(0x11), snes.wram(0xB0), snes.wram(0x1A), snes.wram(0x13),
                snes.debug().split(' ').filter(|t| t.starts_with("nmitimen")).collect::<String>(),
                snes.wram(0x200)
            );
        }
        if std::env::var("SNES_JOY").is_ok() && f >= 1940 && f <= 1975 {
            println!(
                "f={f} F0={:02X}{:02X} F2={:02X} F4={:02X} F6={:02X} F8={:02X} FA={:02X}",
                snes.wram(0xF0), snes.wram(0xF1), snes.wram(0xF2), snes.wram(0xF4), snes.wram(0xF6),
                snes.wram(0xF8), snes.wram(0xFA)
            );
        }
    }
    println!("status: {}", snes.status());
    if let (Some(path), Some((rate, smp))) = (wav, ling_emu::audio::take_capture()) {
        let mut b = Vec::new();
        let n = (smp.len() * 4) as u32;
        for part in [&b"RIFF"[..], &(36 + n).to_le_bytes(), b"WAVEfmt ", &16u32.to_le_bytes(), &1u16.to_le_bytes(),
            &2u16.to_le_bytes(), &rate.to_le_bytes(), &(rate * 4).to_le_bytes(), &4u16.to_le_bytes(),
            &16u16.to_le_bytes(), b"data", &n.to_le_bytes()] {
            b.extend_from_slice(part);
        }
        for (l, r) in &smp {
            b.extend_from_slice(&l.to_le_bytes());
            b.extend_from_slice(&r.to_le_bytes());
        }
        std::fs::write(&path, b).unwrap();
        println!("audio: {} samples @ {rate} Hz", smp.len());
    }
    let (fb, w, h) = snes.framebuffer();
    if let Some(path) = out {
        write_png(std::path::Path::new(path), fb, w, h);
        println!("wrote {path} ({w}x{h})");
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
