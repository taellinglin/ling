//! SNES core: 65C816 + PPU + DMA/HDMA + SPC700/DSP, scheduled per scanline
//! (1364 master cycles: CPU to the start of HBlank → render the line → HDMA →
//! CPU to line end). The APU free-runs on a credit counter synchronised at
//! every port access, so CPU↔APU handshakes see consistent timing.

pub mod cpu;
pub mod dsp;
pub mod ppu;
pub mod smp;

use crate::{Core, System};
use cpu::{Cpu, CpuBus};
use ppu::Ppu;
use smp::Smp;

const LINE_CYCLES: u32 = 1364;
const HBLANK_START: u32 = 1104;

#[derive(Clone, Copy, Default)]
struct Dma {
    r: [u8; 16],
    // HDMA state
    hdma_on: bool,
    hdma_done: bool,
    do_transfer: bool,
    line_counter: u8,
    table: u16,
    indirect: u16,
}

pub struct SBus {
    rom: Vec<u8>,
    hirom: bool,
    pub sram: Vec<u8>,
    pub sram_dirty: bool,
    wram: Vec<u8>,
    pub ppu: Ppu,
    pub smp: Smp,
    cycles: u32,
    abs_base: u64,
    smp_last: u64,
    smp_credit: i64,
    master_hz: i64,
    open_bus: u8,

    nmitimen: u8,
    wrmpya: u8,
    wrdiva: u16,
    rddiv: u16,
    rdmpy: u16,
    htime: u16,
    vtime: u16,
    memsel: u8,
    rdnmi: bool,
    timeup: bool,
    vblank: bool,
    auto_busy: bool,
    nmi_latched: bool,
    nmi_prev: bool,
    pub vcounter: u16,

    joy: [u16; 2],
    joy_auto: [u16; 4],
    joy_shift: [u32; 2],
    strobe: bool,
    wram_addr: u32,
    dma: [Dma; 8],
    hdmaen: u8,
    pub dbg_auto_reads: u64,
    pub dbg_serial_reads: u64,
    pub dbg_irqs: u64,
}

impl SBus {
    fn speed(&self, addr: u32) -> u32 {
        let bank = (addr >> 16) as u8;
        let off = addr & 0xFFFF;
        match bank {
            0x00..=0x3F | 0x80..=0xBF => match off {
                0x0000..=0x1FFF => 8,
                0x2000..=0x3FFF => 6,
                0x4000..=0x41FF => 12,
                0x4200..=0x5FFF => 6,
                0x6000..=0x7FFF => 8,
                _ => {
                    if bank >= 0x80 && self.memsel & 1 != 0 {
                        6
                    } else {
                        8
                    }
                },
            },
            0xC0..=0xFF => {
                if self.memsel & 1 != 0 {
                    6
                } else {
                    8
                }
            },
            _ => 8,
        }
    }

    fn rom_index(&self, bank: u8, off: u16) -> Option<usize> {
        let len = self.rom.len();
        if len == 0 {
            return None;
        }
        if !self.hirom {
            let half = (off as usize) & 0x7FFF;
            match bank {
                0x00..=0x3F | 0x80..=0xBF => {
                    if off >= 0x8000 {
                        Some(((bank as usize & 0x3F) * 0x8000 + half) % len)
                    } else {
                        None
                    }
                },
                0x40..=0x6F | 0xC0..=0xEF => Some(((bank as usize & 0x3F) * 0x8000 + half) % len),
                0x70..=0x7D | 0xF0..=0xFF => {
                    if off >= 0x8000 {
                        Some(((bank as usize & 0x3F) * 0x8000 + half) % len)
                    } else {
                        None
                    }
                },
                _ => None,
            }
        } else {
            match bank {
                0x00..=0x3F | 0x80..=0xBF => {
                    if off >= 0x8000 {
                        Some(((bank as usize & 0x3F) * 0x10000 + off as usize) % len)
                    } else {
                        None
                    }
                },
                0x40..=0x7D | 0xC0..=0xFF => Some(((bank as usize & 0x3F) * 0x10000 + off as usize) % len),
                _ => None,
            }
        }
    }

    fn sram_index(&self, bank: u8, off: u16) -> Option<usize> {
        if self.sram.is_empty() {
            return None;
        }
        let n = self.sram.len();
        if !self.hirom {
            match bank {
                0x70..=0x7D | 0xF0..=0xFF if off < 0x8000 => {
                    Some((((bank as usize & 0x0F) << 15) | off as usize) % n)
                },
                _ => None,
            }
        } else {
            match bank {
                0x20..=0x3F | 0xA0..=0xBF if (0x6000..0x8000).contains(&off) => {
                    Some((((bank as usize & 0x1F) << 13) | (off as usize - 0x6000)) % n)
                },
                _ => None,
            }
        }
    }

    // ── APU sync ─────────────────────────────────────────────────────────────
    fn apu_sync(&mut self) {
        let now = self.abs_base + self.cycles as u64;
        let delta = (now - self.smp_last) as i64;
        self.smp_last = now;
        self.smp_credit += delta * 1_024_000;
        while self.smp_credit > 0 {
            let c = self.smp.step() as i64;
            self.smp_credit -= c * self.master_hz;
        }
    }

    // ── memory ───────────────────────────────────────────────────────────────
    fn read_mem(&mut self, addr: u32) -> u8 {
        let bank = (addr >> 16) as u8;
        let off = (addr & 0xFFFF) as u16;
        match bank {
            0x7E | 0x7F => return self.wram[(((bank as usize) & 1) << 16) | off as usize],
            _ => {},
        }
        let sys = matches!(bank, 0x00..=0x3F | 0x80..=0xBF);
        if sys {
            match off {
                0x0000..=0x1FFF => return self.wram[off as usize],
                0x2100..=0x213F => {
                    let reg = (off & 0xFF) as u8;
                    if reg == 0x37 {
                        self.ppu.ophct = (self.cycles / 4) as u16;
                        self.ppu.opvct = self.vcounter;
                        self.ppu.counter_latched = true;
                        return self.open_bus;
                    }
                    if (0x34..=0x3F).contains(&reg) {
                        return self.ppu.read(reg, self.open_bus);
                    }
                    return self.open_bus;
                },
                0x2140..=0x217F => {
                    self.apu_sync();
                    return self.smp.smp_out[(off & 3) as usize];
                },
                0x2180 => {
                    let v = self.wram[(self.wram_addr & 0x1FFFF) as usize];
                    self.wram_addr = (self.wram_addr + 1) & 0x1FFFF;
                    return v;
                },
                0x4016 | 0x4017 => {
                    self.dbg_serial_reads += 1;
                    let i = (off & 1) as usize;
                    let bit = if self.strobe {
                        (self.joy[i] >> 15) as u8 & 1
                    } else {
                        let b = (self.joy_shift[i] >> 15) as u8 & 1;
                        self.joy_shift[i] = (self.joy_shift[i] << 1) | 1;
                        b
                    };
                    return if i == 0 { (self.open_bus & 0xFC) | bit } else { (self.open_bus & 0xE0) | 0x1C | bit };
                },
                0x4210 => {
                    let v = ((self.rdnmi as u8) << 7) | 0x02 | (self.open_bus & 0x70);
                    self.rdnmi = false;
                    self.update_nmi();
                    return v;
                },
                0x4211 => {
                    let v = ((self.timeup as u8) << 7) | (self.open_bus & 0x7F);
                    self.timeup = false;
                    return v;
                },
                0x4212 => {
                    let hblank = self.cycles >= 1096 || self.cycles < 8;
                    return ((self.vblank as u8) << 7)
                        | ((hblank as u8) << 6)
                        | (self.auto_busy as u8)
                        | (self.open_bus & 0x3E);
                },
                0x4213 => return 0xC0,
                0x4214 => return self.rddiv as u8,
                0x4215 => return (self.rddiv >> 8) as u8,
                0x4216 => return self.rdmpy as u8,
                0x4217 => return (self.rdmpy >> 8) as u8,
                0x4218..=0x421F => {
                    self.dbg_auto_reads += 1;
                    let v = self.joy_auto[((off - 0x4218) >> 1) as usize];
                    return if off & 1 == 0 { v as u8 } else { (v >> 8) as u8 };
                },
                0x4300..=0x437F => {
                    let ch = ((off >> 4) & 7) as usize;
                    return self.dma[ch].r[(off & 0xF) as usize];
                },
                0x6000..=0x7FFF => {
                    if let Some(i) = self.sram_index(bank, off) {
                        return self.sram[i];
                    }
                    return self.open_bus;
                },
                0x8000..=0xFFFF => {
                    return self.rom_index(bank, off).map(|i| self.rom[i]).unwrap_or(self.open_bus)
                },
                _ => return self.open_bus,
            }
        }
        if let Some(i) = self.sram_index(bank, off) {
            return self.sram[i];
        }
        self.rom_index(bank, off).map(|i| self.rom[i]).unwrap_or(self.open_bus)
    }

    fn write_mem(&mut self, addr: u32, v: u8) {
        let bank = (addr >> 16) as u8;
        let off = (addr & 0xFFFF) as u16;
        if bank == 0x7E || bank == 0x7F {
            self.wram[(((bank as usize) & 1) << 16) | off as usize] = v;
            return;
        }
        if matches!(bank, 0x00..=0x3F | 0x80..=0xBF) {
            match off {
                0x0000..=0x1FFF => self.wram[off as usize] = v,
                0x2100..=0x2133 => self.ppu.write((off & 0xFF) as u8, v),
                0x2140..=0x217F => {
                    self.apu_sync();
                    self.smp.cpu_in[(off & 3) as usize] = v;
                },
                0x2180 => {
                    self.wram[(self.wram_addr & 0x1FFFF) as usize] = v;
                    self.wram_addr = (self.wram_addr + 1) & 0x1FFFF;
                },
                0x2181 => self.wram_addr = (self.wram_addr & 0x1FF00) | v as u32,
                0x2182 => self.wram_addr = (self.wram_addr & 0x100FF) | ((v as u32) << 8),
                0x2183 => self.wram_addr = (self.wram_addr & 0x0FFFF) | (((v & 1) as u32) << 16),
                0x4016 => {
                    self.strobe = v & 1 != 0;
                    if self.strobe {
                        self.joy_shift = [self.joy[0] as u32, self.joy[1] as u32];
                    }
                },
                0x4200 => {
                    self.nmitimen = v;
                    if v & 0x30 == 0 {
                        self.timeup = false;
                    }
                    self.update_nmi();
                },
                0x4201 => {},
                0x4202 => self.wrmpya = v,
                0x4203 => {
                    self.rdmpy = self.wrmpya as u16 * v as u16;
                    self.rddiv = v as u16;
                },
                0x4204 => self.wrdiva = (self.wrdiva & 0xFF00) | v as u16,
                0x4205 => self.wrdiva = (self.wrdiva & 0x00FF) | ((v as u16) << 8),
                0x4206 => {
                    if v == 0 {
                        self.rddiv = 0xFFFF;
                        self.rdmpy = self.wrdiva;
                    } else {
                        self.rddiv = self.wrdiva / v as u16;
                        self.rdmpy = self.wrdiva % v as u16;
                    }
                },
                0x4207 => self.htime = (self.htime & 0x100) | v as u16,
                0x4208 => self.htime = (self.htime & 0x0FF) | (((v & 1) as u16) << 8),
                0x4209 => self.vtime = (self.vtime & 0x100) | v as u16,
                0x420A => self.vtime = (self.vtime & 0x0FF) | (((v & 1) as u16) << 8),
                0x420B => self.run_dma(v),
                0x420C => self.hdmaen = v,
                0x420D => self.memsel = v,
                0x4300..=0x437F => {
                    let ch = ((off >> 4) & 7) as usize;
                    self.dma[ch].r[(off & 0xF) as usize] = v;
                },
                0x6000..=0x7FFF => {
                    if let Some(i) = self.sram_index(bank, off) {
                        self.sram[i] = v;
                        self.sram_dirty = true;
                    }
                },
                _ => {},
            }
            return;
        }
        if let Some(i) = self.sram_index(bank, off) {
            self.sram[i] = v;
            self.sram_dirty = true;
        }
    }

    fn update_nmi(&mut self) {
        let line = self.rdnmi && self.nmitimen & 0x80 != 0;
        if line && !self.nmi_prev {
            self.nmi_latched = true;
        }
        self.nmi_prev = line;
    }

    // ── DMA ──────────────────────────────────────────────────────────────────
    const PATTERNS: [&'static [u8]; 8] =
        [&[0], &[0, 1], &[0, 0], &[0, 0, 1, 1], &[0, 1, 2, 3], &[0, 1, 0, 1], &[0, 0], &[0, 0, 1, 1]];

    fn run_dma(&mut self, mask: u8) {
        let mut cost = 8u32;
        for ch in 0..8 {
            if mask & (1 << ch) == 0 {
                continue;
            }
            cost += 8;
            let r = self.dma[ch].r;
            let b_to_a = r[0] & 0x80 != 0;
            let fixed = r[0] & 0x08 != 0;
            let dec = r[0] & 0x10 != 0;
            let pattern = Self::PATTERNS[(r[0] & 7) as usize];
            let bbad = r[1] as u32;
            let mut a = ((r[4] as u32) << 16) | ((r[3] as u32) << 8) | r[2] as u32;
            let mut count = ((r[6] as u32) << 8) | r[5] as u32;
            if count == 0 {
                count = 0x10000;
            }
            let mut pi = 0;
            while count > 0 {
                let boff = pattern[pi % pattern.len()] as u32;
                pi += 1;
                let b_addr = 0x2100 + ((bbad + boff) & 0xFF);
                if b_to_a {
                    let v = self.read_mem(b_addr);
                    self.write_mem(a, v);
                } else {
                    let v = self.read_mem(a);
                    self.write_mem(b_addr, v);
                }
                if !fixed {
                    let lo = (a & 0xFFFF) as u16;
                    let lo = if dec { lo.wrapping_sub(1) } else { lo.wrapping_add(1) };
                    a = (a & 0xFF0000) | lo as u32;
                }
                count -= 1;
                cost += 8;
            }
            self.dma[ch].r[2] = a as u8;
            self.dma[ch].r[3] = (a >> 8) as u8;
            self.dma[ch].r[5] = 0;
            self.dma[ch].r[6] = 0;
        }
        self.cycles += cost;
    }

    fn hdma_init(&mut self) {
        for ch in 0..8 {
            let on = self.hdmaen & (1 << ch) != 0;
            self.dma[ch].hdma_on = on;
            self.dma[ch].hdma_done = !on;
            if !on {
                continue;
            }
            let r = self.dma[ch].r;
            let mut table = ((r[3] as u16) << 8) | r[2] as u16;
            let bank = (r[4] as u32) << 16;
            let lc = self.read_mem(bank | table as u32);
            table = table.wrapping_add(1);
            self.dma[ch].line_counter = lc;
            self.dma[ch].do_transfer = true;
            if lc == 0 {
                self.dma[ch].hdma_done = true;
            } else if r[0] & 0x40 != 0 {
                let lo = self.read_mem(bank | table as u32) as u16;
                let hi = self.read_mem(bank | table.wrapping_add(1) as u32) as u16;
                table = table.wrapping_add(2);
                self.dma[ch].indirect = (hi << 8) | lo;
            }
            self.dma[ch].table = table;
        }
    }

    fn hdma_run(&mut self) {
        for ch in 0..8 {
            if self.hdmaen & (1 << ch) == 0 || self.dma[ch].hdma_done || !self.dma[ch].hdma_on {
                continue;
            }
            let r = self.dma[ch].r;
            let indirect = r[0] & 0x40 != 0;
            let bank = (r[4] as u32) << 16;
            let ibank = (r[7] as u32) << 16;
            if self.dma[ch].do_transfer {
                let pattern = Self::PATTERNS[(r[0] & 7) as usize];
                for &boff in pattern {
                    let v = if indirect {
                        let a = ibank | self.dma[ch].indirect as u32;
                        self.dma[ch].indirect = self.dma[ch].indirect.wrapping_add(1);
                        self.read_mem(a)
                    } else {
                        let a = bank | self.dma[ch].table as u32;
                        self.dma[ch].table = self.dma[ch].table.wrapping_add(1);
                        self.read_mem(a)
                    };
                    let b_addr = 0x2100 + ((r[1] as u32 + boff as u32) & 0xFF);
                    if r[0] & 0x80 != 0 {
                        // B→A HDMA is not used by games; skip data movement
                        let _ = v;
                    } else {
                        self.write_mem(b_addr, v);
                    }
                    self.cycles += 8;
                }
            }
            let lc = self.dma[ch].line_counter.wrapping_sub(1);
            self.dma[ch].line_counter = lc;
            self.dma[ch].do_transfer = lc & 0x80 != 0;
            if lc & 0x7F == 0 {
                let mut table = self.dma[ch].table;
                let nlc = self.read_mem(bank | table as u32);
                table = table.wrapping_add(1);
                self.dma[ch].line_counter = nlc;
                if indirect && nlc != 0 {
                    let lo = self.read_mem(bank | table as u32) as u16;
                    let hi = self.read_mem(bank | table.wrapping_add(1) as u32) as u16;
                    table = table.wrapping_add(2);
                    self.dma[ch].indirect = (hi << 8) | lo;
                }
                self.dma[ch].table = table;
                if nlc == 0 {
                    self.dma[ch].hdma_done = true;
                }
                self.dma[ch].do_transfer = true;
            }
        }
    }
}

impl CpuBus for SBus {
    fn read(&mut self, addr: u32) -> u8 {
        self.cycles += self.speed(addr);
        let v = self.read_mem(addr);
        self.open_bus = v;
        v
    }
    fn write(&mut self, addr: u32, v: u8) {
        self.cycles += self.speed(addr);
        self.open_bus = v;
        self.write_mem(addr, v);
    }
    fn idle(&mut self) {
        self.cycles += 6;
    }
    fn cycle_at(&mut self, addr: u32) {
        self.cycles += self.speed(addr);
    }
    fn take_nmi(&mut self) -> bool {
        let n = self.nmi_latched;
        self.nmi_latched = false;
        n
    }
    fn irq(&self) -> bool {
        self.timeup
    }
}

pub struct Snes {
    cpu: Cpu,
    bus: SBus,
    pal: bool,
    frame_acc: f64,
    pub frames: u64,
    note: String,
    title: String,
}

/// Locate a cartridge header by plausibility score (real hardware never checks
/// the checksum, and homebrew often leaves it unset). Returns (header offset
/// including any 512-byte copier header, hirom).
fn find_header(rom: &[u8]) -> Option<(usize, bool)> {
    let off = if rom.len() % 1024 == 512 { 512 } else { 0 };
    let mut best: Option<(i32, usize, bool)> = None;
    for (base, hirom) in [(0x7FC0usize, false), (0xFFC0usize, true)] {
        let h = off + base;
        if rom.len() < h + 64 {
            continue;
        }
        let cks = rom[h + 28] as u16 | ((rom[h + 29] as u16) << 8);
        let cmp = rom[h + 30] as u16 | ((rom[h + 31] as u16) << 8);
        let map = rom[h + 21];
        let reset = rom[h + 0x3C] as u16 | ((rom[h + 0x3D] as u16) << 8);
        let printable = rom[h..h + 21].iter().filter(|&&c| (32..127).contains(&c)).count();
        let mut score = 0;
        if cks ^ cmp == 0xFFFF {
            score += 4;
        }
        if reset >= 0x8000 {
            score += 2;
        }
        if map & 0xE0 == 0x20 && (map & 1 == 1) == hirom {
            score += 2;
        }
        if printable >= 16 {
            score += 1;
        }
        if best.map_or(true, |(b, _, _)| score > b) {
            best = Some((score, h, hirom));
        }
    }
    best.filter(|&(sc, _, _)| sc >= 5).map(|(_, h, hi)| (h, hi))
}

pub fn looks_like_snes(rom: &[u8]) -> bool {
    find_header(rom).is_some()
}

impl Snes {
    pub fn boot(raw: &[u8]) -> Result<Snes, String> {
        let (h, hirom) = find_header(raw).ok_or("no valid SNES header")?;
        let copier = if raw.len() % 1024 == 512 { 512 } else { 0 };
        let rom = raw[copier..].to_vec();
        let h = h - copier;
        let title: String = rom[h..h + 21]
            .iter()
            .map(|&c| if (32..127).contains(&c) { c as char } else { ' ' })
            .collect::<String>()
            .trim_end()
            .to_string();
        let sram_kb = rom[h + 24] as u32;
        let sram_size = if sram_kb == 0 { 0 } else { (1usize << sram_kb.min(8)) * 1024 };
        let region = rom[h + 25];
        let pal = !matches!(region, 0 | 1 | 0x0D | 0x0F | 0x10);
        let master_hz: i64 = if pal { 21_281_370 } else { 21_477_272 };

        let mut bus = SBus {
            rom,
            hirom,
            sram: vec![0; sram_size],
            sram_dirty: false,
            wram: vec![0; 131072],
            ppu: Ppu::new(pal),
            smp: Smp::new(),
            cycles: 0,
            abs_base: 0,
            smp_last: 0,
            smp_credit: 0,
            master_hz,
            open_bus: 0,
            nmitimen: 0,
            wrmpya: 0xFF,
            wrdiva: 0xFFFF,
            rddiv: 0,
            rdmpy: 0,
            htime: 0x1FF,
            vtime: 0x1FF,
            memsel: 0,
            rdnmi: false,
            timeup: false,
            vblank: false,
            auto_busy: false,
            nmi_latched: false,
            nmi_prev: false,
            vcounter: 0,
            joy: [0; 2],
            joy_auto: [0; 4],
            joy_shift: [0; 2],
            strobe: false,
            wram_addr: 0,
            dma: [Dma::default(); 8],
            hdmaen: 0,
            dbg_auto_reads: 0,
            dbg_serial_reads: 0,
            dbg_irqs: 0,
        };
        for d in bus.dma.iter_mut() {
            d.r = [0xFF; 16];
        }
        let mut cpu = Cpu::new();
        cpu.reset(&mut bus);
        bus.cycles = 0;
        Ok(Snes { cpu, bus, pal, frame_acc: 1.0, frames: 0, note: String::new(), title })
    }

    pub fn ppu(&self) -> &Ppu {
        &self.bus.ppu
    }

    /// Debug snapshot: CPU position, flags and APU port traffic.
    pub fn debug(&self) -> String {
        format!(
            "auto_reads={} serial_reads={} pc={:02X}:{:04X} a={:04X} x={:04X} y={:04X} p={:02X} e={} nmitimen={:02X} vtime={} apu_out={:02X?} apu_in={:02X?} inidisp={:02X} joy_auto={:04X}",
            self.bus.dbg_auto_reads, self.bus.dbg_serial_reads,
            self.cpu.pb, self.cpu.pc, self.cpu.a, self.cpu.x, self.cpu.y, self.cpu.p, self.cpu.e as u8,
            self.bus.nmitimen, self.bus.vtime, self.bus.smp.smp_out, self.bus.smp.cpu_in,
            self.bus.ppu.inidisp, self.bus.joy_auto[0]
        )
    }

    /// WRAM peek for debugging (bank 7E offset).
    pub fn wram(&self, off: usize) -> u8 {
        self.bus.wram[off & 0x1FFFF]
    }

    pub fn title(&self) -> &str {
        &self.title
    }
    pub fn is_pal(&self) -> bool {
        self.pal
    }
    pub fn sram(&self) -> &[u8] {
        &self.bus.sram
    }
    pub fn load_sram(&mut self, data: &[u8]) {
        if data.len() == self.bus.sram.len() {
            self.bus.sram.copy_from_slice(data);
        }
    }
    pub fn take_sram_if_dirty(&mut self) -> Option<Vec<u8>> {
        if self.bus.sram_dirty && !self.bus.sram.is_empty() {
            self.bus.sram_dirty = false;
            Some(self.bus.sram.clone())
        } else {
            None
        }
    }

    fn run_until(&mut self, target: u32, irq_at: &mut Option<u32>) {
        while self.bus.cycles < target {
            if let Some(c) = *irq_at {
                if self.bus.cycles >= c {
                    self.bus.timeup = true;
                    *irq_at = None;
                }
            }
            self.cpu.step(&mut self.bus);
        }
    }

    fn run_scanline(&mut self, v: u16, total: u16) {
        let vdisp = self.bus.ppu.visible_lines as u16;
        self.bus.vcounter = v;

        if v == 0 {
            self.bus.rdnmi = false;
            self.bus.update_nmi();
            self.bus.vblank = false;
            self.bus.ppu.frame_notes = 0;
            if self.bus.hdmaen != 0 {
                self.bus.hdma_init();
            }
        }
        if v == vdisp + 1 {
            self.bus.vblank = true;
            self.bus.rdnmi = true;
            self.bus.update_nmi();
            if self.bus.ppu.inidisp & 0x80 == 0 {
                self.bus.ppu.oam_reload();
            }
            if self.bus.nmitimen & 1 != 0 {
                self.bus.joy_auto = [self.bus.joy[0], self.bus.joy[1], 0, 0];
                self.bus.auto_busy = true;
            }
        }
        if v == vdisp + 4 {
            self.bus.auto_busy = false;
        }

        let mode = (self.bus.nmitimen >> 4) & 3;
        let htime_cycle = self.bus.htime as u32 * 4 + 14;
        let mut irq_at: Option<u32> = match mode {
            1 => Some(htime_cycle),
            2 if v == self.bus.vtime => Some(14),
            3 if v == self.bus.vtime => Some(htime_cycle),
            _ => None,
        };
        if matches!(irq_at, Some(c) if c >= LINE_CYCLES) {
            irq_at = None;
        }

        self.run_until(HBLANK_START, &mut irq_at);
        if v >= 1 && v <= vdisp {
            self.bus.ppu.render_line(v as usize);
        }
        if v < vdisp && self.bus.hdmaen != 0 {
            self.bus.hdma_run();
        }
        self.run_until(LINE_CYCLES, &mut irq_at);
        let _ = total;

        self.bus.apu_sync();
        // rebase: absolute time (abs_base + cycles) is unchanged
        self.bus.cycles -= LINE_CYCLES;
        self.bus.abs_base += LINE_CYCLES as u64;
    }

    fn emulate_frame(&mut self) {
        let total: u16 = if self.pal { 312 } else { 262 };
        for v in 0..total {
            self.run_scanline(v, total);
        }
        self.frames += 1;
        // forward whatever the DSP produced to the audio device
        if !self.bus.smp.samples.is_empty() {
            crate::audio::push_stereo_32k(&self.bus.smp.samples);
            self.bus.smp.samples.clear();
        }
        self.note = self.bus.ppu.unsupported().map(|s| format!(" · not yet: {s}")).unwrap_or_default();
    }
}

impl Core for Snes {
    fn system(&self) -> System {
        System::Snes
    }

    fn run_frame(&mut self) {
        // The arcade ticks at 60 Hz; PAL carts run at 50 Hz → repeat frames.
        self.frame_acc += if self.pal { 50.0 / 60.0 } else { 60.0988 / 60.0 };
        while self.frame_acc >= 1.0 {
            self.frame_acc -= 1.0;
            self.emulate_frame();
        }
    }

    fn framebuffer(&self) -> (&[u32], usize, usize) {
        let h = self.bus.ppu.visible_lines;
        (&self.bus.ppu.fb[..ppu::WIDTH * h], ppu::WIDTH, h)
    }

    fn set_button(&mut self, pad: usize, button: usize, down: bool) {
        if pad > 1 || button > 11 {
            return;
        }
        let mask = 1u16 << (15 - button);
        if down {
            self.bus.joy[pad] |= mask;
        } else {
            self.bus.joy[pad] &= !mask;
        }
    }

    fn status(&self) -> String {
        if self.cpu.stopped {
            return "cpu stopped (STP)".into();
        }
        format!(
            "ok · {} · {}{}",
            self.title,
            if self.pal { "PAL 50 Hz" } else { "NTSC 60 Hz" },
            self.note
        )
    }

    fn take_save(&mut self) -> Option<Vec<u8>> {
        self.take_sram_if_dirty()
    }

    fn load_save(&mut self, data: &[u8]) {
        self.load_sram(data);
    }
}
