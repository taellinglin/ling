//! N64 core: HLE IPL3 boot (CIC-detected), R4300i interpreter, VI scanout.

pub mod cpu;
pub mod mem;
pub mod rdp;
pub mod rsp;
pub mod rsp_core;

use crate::{Core, System};
use cpu::Cpu;
use mem::{Bus, MI_INTR_VI};


const CYCLES_PER_FRAME: u64 = 1_562_500; // 93.75 MHz / 60

pub struct N64 {
    pub cpu: Cpu,
    pub bus: Bus,
    fb: Vec<u32>,
    cvg: Vec<u8>,
    fb_w: usize,
    fb_h: usize,
    /// VI output after horizontal scaling (X_SCALE / H_VIDEO); empty = fb as-is.
    disp: Vec<u32>,
    disp_w: usize,
    disp_h: usize,
    gfx_task_seen: bool,
    last_task_kind: u32,
    pub task_log: Vec<String>,
    pub prof_cpu_ns: u128,
    pub prof_rsp_ns: u128,
}

impl N64 {
    /// Normalize byte order, detect the CIC, HLE the IPL3 boot.
    pub fn boot(raw: &[u8]) -> Result<N64, String> {
        if raw.len() < 0x1000 {
            return Err("ROM too small".into());
        }
        // → big-endian (.z64) order
        let rom: Vec<u8> = match [raw[0], raw[1]] {
            [0x80, 0x37] => raw.to_vec(),
            [0x37, 0x80] => raw.chunks(2).flat_map(|c| [c[1], c[0]]).collect(),
            [0x40, 0x12] => raw
                .chunks(4)
                .flat_map(|c| [c[3], c[2], c[1], c[0]])
                .collect(),
            _ => return Err("not an N64 ROM".into()),
        };

        let be32 = |i: usize| u32::from_be_bytes([rom[i], rom[i + 1], rom[i + 2], rom[i + 3]]);
        let mut entry = be32(8);

        // CIC detection by CRC32 of the IPL3 blob (0x40..0x1000)
        let crc = crc32(&rom[0x40..0x1000]);
        let (cic, seed) = match crc {
            0x6170_A4A1 => ("6101", 0x3F),
            0x90BB_6CB5 => ("6102", 0x3F),
            0x0B05_0EE0 => ("6103", 0x78),
            0x98BC_2C86 => ("6105", 0x91),
            0xACC8_580A => ("6106", 0x85),
            _ => ("6102?", 0x3F),
        };
        if cic == "6103" {
            entry = entry.wrapping_sub(0x0010_0000);
        }
        if cic == "6106" {
            entry = entry.wrapping_sub(0x0020_0000);
        }

        let mut bus = Bus::new(rom);
        // IPL3 copies 1 MB of the ROM's code segment to the entry address.
        let dst = (entry & 0x1FFF_FFFF) as usize;
        let n = (bus.rom.len() - 0x1000).min(0x0010_0000);
        for i in 0..n {
            if dst + i < bus.rdram.len() {
                bus.rdram[dst + i] = bus.rom[0x1000 + i];
            }
        }
        // libultra boot globals the IPL3/entry stub normally fills in:
        // osTvType(0x300)=NTSC · osRomType(0x304)=cart · osRomBase(0x308) ·
        // osResetType(0x30C)=cold · osVersion(0x314) · osMemSize(0x318)=8MB.
        // A zero osTvType reads as PAL and NTSC carts hang on purpose.
        bus.rdram[0x300..0x304].copy_from_slice(&1u32.to_be_bytes());
        bus.rdram[0x304..0x308].copy_from_slice(&0u32.to_be_bytes());
        bus.rdram[0x308..0x30C].copy_from_slice(&0xB000_0000u32.to_be_bytes());
        bus.rdram[0x30C..0x310].copy_from_slice(&0u32.to_be_bytes());
        bus.rdram[0x314..0x318].copy_from_slice(&0u32.to_be_bytes());
        bus.rdram[0x318..0x31C].copy_from_slice(&0x0080_0000u32.to_be_bytes());

        let mut cpu = Cpu::new();
        cpu.set_pc(entry);
        cpu.gpr[29] = 0xFFFF_FFFF_A400_1FF0; // sp
        cpu.gpr[31] = 0xFFFF_FFFF_A400_1550; // ra
        cpu.gpr[11] = 0xFFFF_FFFF_A400_0040; // t3
        cpu.gpr[20] = 1; // s4: TV type NTSC
        cpu.gpr[22] = seed; // s6: CIC seed
        cpu.gpr[23] = 6; // s7

        Ok(N64 {
            cpu,
            bus,
            fb: vec![0; 640 * 480],
            cvg: vec![8; 640 * 480],
            fb_w: 320,
            fb_h: 240,
            disp: Vec::new(),
            disp_w: 0,
            disp_h: 0,
            gfx_task_seen: false,
            last_task_kind: 0,
            task_log: Vec::new(),
            prof_cpu_ns: 0,
            prof_rsp_ns: 0,
        })
    }

    /// The VI shows `(h_end - h_start) * x_scale` framebuffer pixels across
    /// the full TV width, so a framebuffer whose width differs from that
    /// (e.g. Majora's Mask's ~440-wide title screen) must be resampled to keep
    /// the picture 4:3. Output: 320x240, or 640x480 for wider sources.
    fn vi_scale(&mut self) {
        let (w, h) = (self.fb_w, self.fb_h);
        let hv = self.bus.vi_regs[0x24 >> 2];
        let xs = (self.bus.vi_regs[0x30 >> 2] & 0xFFF) as usize;
        let (h0, h1) = (((hv >> 16) & 0x3FF) as usize, (hv & 0x3FF) as usize);
        let mut src_w = w;
        if h1 > h0 && xs > 0 {
            let d = ((h1 - h0) * xs) >> 10;
            if d >= 16 && d <= w {
                src_w = d;
            }
        }
        let (ow, oh) = if h >= 480 || src_w > 320 { (640, 480) } else { (320, 240) };
        if src_w == ow && h == oh {
            self.disp_w = 0;
            return;
        }
        if self.disp.len() < ow * oh {
            self.disp.resize(ow * oh, 0);
        }
        // horizontal linear filter (the VI interpolates too); nearest rows
        let xmap: Vec<(usize, u32)> = (0..ow)
            .map(|ox| {
                let fx = ((ox as f32 + 0.5) * src_w as f32 / ow as f32 - 0.5).max(0.0);
                let x0 = (fx as usize).min(src_w - 1);
                (x0, ((fx - x0 as f32) * 256.0) as u32)
            })
            .collect();
        for oy in 0..oh {
            let sy = oy * h / oh;
            let row = &self.fb[sy * w..sy * w + w];
            let out = &mut self.disp[oy * ow..oy * ow + ow];
            for (o, &(x0, f)) in out.iter_mut().zip(xmap.iter()) {
                let a = row[x0];
                let b = row[(x0 + 1).min(src_w - 1)];
                let mix = |sh: u32| -> u32 {
                    let (ca, cb) = ((a >> sh) & 255, (b >> sh) & 255);
                    ((ca * (256 - f) + cb * f) >> 8) << sh
                };
                *o = mix(16) | mix(8) | mix(0);
            }
        }
        self.disp_w = ow;
        self.disp_h = oh;
    }

    fn scanout(&mut self) {
        let fmt = self.bus.vi_ctrl & 3;
        let w = (self.bus.vi_width as usize).clamp(1, 640);
        let h = if w >= 512 { 480 } else { 240 };
        self.fb_w = w;
        self.fb_h = h;
        if self.fb.len() < w * h {
            self.fb.resize(w * h, 0);
        }
        let org = self.bus.vi_origin as usize & 0x7F_FFFF;
        match fmt {
            3 => {
                // 32bpp RGBA8888
                for i in 0..w * h {
                    let s = org + i * 4;
                    if s + 3 < self.bus.rdram.len() {
                        let r = self.bus.rdram[s] as u32;
                        let g = self.bus.rdram[s + 1] as u32;
                        let b = self.bus.rdram[s + 2] as u32;
                        self.fb[i] = (r << 16) | (g << 8) | b;
                    }
                }
            },
            2 => {
                // 16bpp RGBA5551, upgraded to the RDP's 24-bit shadow where valid
                if self.cvg.len() < w * h {
                    self.cvg.resize(w * h, 8);
                }
                let mut partial = false;
                for i in 0..w * h {
                    let s = org + i * 2;
                    if s + 1 < self.bus.rdram.len() {
                        let p =
                            u16::from_be_bytes([self.bus.rdram[s], self.bus.rdram[s + 1]]);
                        match self.bus.rdp.hi_lookup(s, p) {
                            Some((rgb, c)) => {
                                self.fb[i] = rgb;
                                self.cvg[i] = c as u8;
                                partial |= c < 8;
                            },
                            None => {
                                let r = ((p >> 11) & 31) as u32 * 255 / 31;
                                let g = ((p >> 6) & 31) as u32 * 255 / 31;
                                let b = ((p >> 1) & 31) as u32 * 255 / 31;
                                self.fb[i] = (r << 16) | (g << 8) | b;
                                self.cvg[i] = 8;
                            },
                        }
                    }
                }
                // the VI only displays ((v_end - v_start) / 2) * y_scale lines;
                // anything below is off-screen memory, not picture
                let vv = self.bus.vi_regs[0x28 >> 2];
                let ys = self.bus.vi_regs[0x34 >> 2] & 0xFFF;
                let (v0, v1) = ((vv >> 16) & 0x3FF, vv & 0x3FF);
                if v1 > v0 && ys > 0 {
                    let shown = ((((v1 - v0) / 2) * ys) >> 10) as usize;
                    if shown > h / 2 && shown < h {
                        for p in &mut self.fb[shown * w..h * w] {
                            *p = 0;
                        }
                        for c in &mut self.cvg[shown * w..h * w] {
                            *c = 8;
                        }
                    }
                }
                // VI anti-alias filter (AA modes 0/1): a partially covered
                // pixel is pulled toward the background, estimated from its
                // fully covered neighbours' penultimate max + min.
                if partial && (self.bus.vi_ctrl >> 8) & 3 <= 1 {
                    self.vi_aa_filter(w, h);
                }
            },
            _ => {
                for p in self.fb.iter_mut() {
                    *p = 0;
                }
            },
        }
    }
}

impl N64 {
    fn vi_aa_filter(&mut self, w: usize, h: usize) {
        let src = self.fb[..w * h].to_vec();
        for y in 1..h.saturating_sub(1) {
            for x in 1..w.saturating_sub(1) {
                let i = y * w + x;
                let c = self.cvg[i] as i32;
                if c >= 8 {
                    continue;
                }
                // like the VI: partially covered neighbours count as the
                // centre colour; the centre is one of the samples
                let p = src[i];
                let center = [(p >> 16) as i32 & 255, (p >> 8) as i32 & 255, p as i32 & 255];
                let mut nb = [[0i32; 9]; 3];
                let mut full = 0;
                for (n, (dy, dx)) in
                    [(-1i32, -1i32), (-1, 0), (-1, 1), (0, -1), (0, 1), (1, -1), (1, 0), (1, 1), (0, 0)]
                        .into_iter()
                        .enumerate()
                {
                    let j = ((y as i32 + dy) as usize) * w + (x as i32 + dx) as usize;
                    let q = if self.cvg[j] >= 8 {
                        full += 1;
                        src[j]
                    } else {
                        p
                    };
                    nb[0][n] = (q >> 16) as i32 & 255;
                    nb[1][n] = (q >> 8) as i32 & 255;
                    nb[2][n] = q as i32 & 255;
                }
                if full == 0 {
                    continue;
                }
                let mut out = [0i32; 3];
                for ch in 0..3 {
                    let v = &mut nb[ch];
                    v.sort_unstable();
                    // penultimate extremes reject a single outlier neighbour
                    let (lo, hi) = (v[1], v[7]);
                    let col = center[ch];
                    let o = col + ((8 - c) * (hi + lo - 2 * col) + 4) / 8;
                    out[ch] = o.clamp(lo.min(col), hi.max(col));
                }
                self.fb[i] = ((out[0] as u32) << 16) | ((out[1] as u32) << 8) | out[2] as u32;
            }
        }
    }
}

impl Core for N64 {
    fn system(&self) -> System {
        System::N64
    }

    fn run_frame(&mut self) {
        // slice the frame so RSP execution interleaves with CPU progress
        for _ in 0..8 {
            let t0 = std::time::Instant::now();
            self.cpu.run(&mut self.bus, CYCLES_PER_FRAME / 8);
            self.prof_cpu_ns += t0.elapsed().as_nanos();
            self.bus.ai_tick(CYCLES_PER_FRAME / 8);
            if self.bus.sp_kicked {
                self.bus.sp_kicked = false;
                let t = rsp::peek_task(&self.bus);
                if t.kind == 1 {
                    self.gfx_task_seen = true;
                }
                self.last_task_kind = t.kind;
                if let Some(n) = &t.ucode_name {
                    self.bus.last_ucode = Some(n.clone());
                }
                if self.task_log.len() < 16 {
                    self.task_log.push(format!(
                        "kind={} ucode={}",
                        t.kind,
                        t.ucode_name.unwrap_or_else(|| "?".into())
                    ));
                }
            }
            // backstop for tasks that outlived the synchronous budget
            self.bus.run_rsp(400_000);
        }
        self.bus.raise(MI_INTR_VI);
        self.scanout();
        self.vi_scale();
        if !self.bus.ai_out.is_empty() {
            let rate = self.bus.ai_rate().round() as u32;
            crate::audio::push_stereo(rate, &self.bus.ai_out);
            self.bus.ai_out.clear();
        }
    }

    fn framebuffer(&self) -> (&[u32], usize, usize) {
        if self.disp_w > 0 {
            (&self.disp[..self.disp_w * self.disp_h], self.disp_w, self.disp_h)
        } else {
            (&self.fb[..self.fb_w * self.fb_h], self.fb_w, self.fb_h)
        }
    }

    fn set_button(&mut self, pad: usize, button: usize, down: bool) {
        if pad >= 4 || button >= 14 {
            return;
        }
        const MASKS: [u16; 14] = [
            0x8000, 0x4000, 0x2000, 0x1000, // A B Z Start
            0x0800, 0x0400, 0x0200, 0x0100, // D-pad U D L R
            0x0020, 0x0010, // L R shoulders
            0x0008, 0x0004, 0x0002, 0x0001, // C U D L R
        ];
        if down {
            self.bus.pads[pad].buttons |= MASKS[button];
        } else {
            self.bus.pads[pad].buttons &= !MASKS[button];
        }
    }

    fn set_stick(&mut self, pad: usize, x: f32, y: f32) {
        if pad < 4 {
            self.bus.pads[pad].stick_x = (x.clamp(-1.0, 1.0) * 80.0) as i8;
            self.bus.pads[pad].stick_y = (y.clamp(-1.0, 1.0) * 80.0) as i8;
        }
    }

    fn status(&self) -> String {
        if let Some(e) = &self.cpu.stuck {
            return format!("cpu stuck: {e}");
        }
        if let Some(e) = self.bus.rsp.as_ref().and_then(|r| r.stuck.as_ref()) {
            return format!("rsp stuck: {e}");
        }
        if self.bus.rdp_cmds > 0 {
            let uc = self
                .bus
                .last_ucode
                .clone()
                .unwrap_or_else(|| "microcode".into());
            return format!(
                "RDP: {} commands · {} triangles · {} px · AA + 24-bit colour · {uc}",
                self.bus.rdp_cmds, self.bus.rdp.triangles, self.bus.rdp.pixels
            );
        }
        if self.last_task_kind != 0 {
            let uc = self
                .bus
                .last_ucode
                .clone()
                .unwrap_or_else(|| "unidentified microcode".into());
            return format!("RSP LLE running task kind {} ({uc})", self.last_task_kind);
        }
        "ok (CPU-rendered video)".into()
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for n in 0..256u32 {
        let mut c = n;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        table[n as usize] = c;
    }
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c = table[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}
