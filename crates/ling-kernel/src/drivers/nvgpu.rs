//! NVIDIA GPU driver, stage 1 (x86-64): find the card on PCI, size and map its
//! register window (BAR0), identify the chip, and report what the CPU can see
//! of it -- VRAM size, the PCIe link, and whether the screen LingOS draws on
//! (the firmware's framebuffer) lies in the card's VRAM aperture (BAR1).
//!
//! Read-only on purpose. Nothing here writes a GPU register: on Turing and
//! newer chips (the RTX 3060 Ti is GA104, Ampere) engine bring-up belongs to
//! the GSP firmware, which this kernel does not load yet. Every read below is
//! of a register that is safe before GSP: PMC_BOOT_0 / PMC_BOOT_42 (chip id)
//! and, on GA102+ chips, the usable-framebuffer-size register the open-source
//! drivers (nouveau, nova-core) read the same way.
//!
//! Stages that would follow, in order: VRAM access through BAR1 (with the
//! GSP-resets-BAR1 caveat), loading the GSP firmware (booter + FWSEC + the
//! gsp_ga10x image from linux-firmware), a copy engine, then display.
use crate::arch::pci::{self, Location};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

const NV_VENDOR: u16 = 0x10DE;
const NV_PMC_BOOT_0: u64 = 0x0000_0000;
const NV_PMC_BOOT_42: u64 = 0x0000_0A00;
/// GA102 and newer: usable framebuffer size in MiB.
const NV_USABLE_FB_SIZE_IN_MB: u64 = 0x0011_83A4;

#[derive(Clone)]
pub struct Gpu {
    pub loc: Location,
    pub device_id: u16,
    pub subsys: u32,
    pub bar0: u64,
    pub bar0_len: u64,
    pub bar1: u64,
    pub bar1_len: u64,
    pub boot0: u32,
    pub boot42: u32,
    pub vram_mib: Option<u32>,
    /// PCIe link now (generation, lanes) and what the slot + card can do.
    pub link: Option<((u8, u8), (u8, u8))>,
    pub note: String,
}

static mut GPUS: Option<Vec<Gpu>> = None;

/// A BAR's base and size. Sizing writes all-ones and restores the value;
/// the caller has memory decoding switched off meanwhile.
fn bar(loc: &Location, idx: u8) -> (u64, u64, u8) {
    let off = 0x10 + idx * 4;
    let lo = loc.read32(off);
    if lo & 1 != 0 {
        return (0, 0, 1); // I/O BAR
    }
    let is64 = (lo >> 1) & 3 == 2;
    let hi = if is64 { loc.read32(off + 4) } else { 0 };
    let base = ((hi as u64) << 32) | (lo & 0xFFFF_FFF0) as u64;
    pci::write32(loc.bus, loc.device, loc.function, off, 0xFFFF_FFFF);
    let mlo = loc.read32(off) & 0xFFFF_FFF0;
    pci::write32(loc.bus, loc.device, loc.function, off, lo);
    let size = if is64 {
        pci::write32(loc.bus, loc.device, loc.function, off + 4, 0xFFFF_FFFF);
        let mhi = loc.read32(off + 4);
        pci::write32(loc.bus, loc.device, loc.function, off + 4, hi);
        let mask = ((mhi as u64) << 32) | mlo as u64;
        if mask == 0 { 0 } else { (!mask).wrapping_add(1) }
    } else if mlo == 0 {
        0
    } else {
        ((!mlo).wrapping_add(1)) as u64
    };
    (base, size, if is64 { 2 } else { 1 })
}

/// The PCI Express capability: (current gen, width), (max gen, width).
fn pcie_link(loc: &Location) -> Option<((u8, u8), (u8, u8))> {
    if (loc.read32(0x04) >> 16) & 0x10 == 0 {
        return None; // no capability list
    }
    let mut p = (loc.read32(0x34) & 0xFC) as u8;
    let mut guard = 0;
    while p != 0 && guard < 48 {
        let h = loc.read32(p);
        if h & 0xFF == 0x10 {
            let cap = loc.read32(p + 0x0C);
            let st = loc.read32(p + 0x10) >> 16;
            return Some((((st & 0xF) as u8, ((st >> 4) & 0x3F) as u8), ((cap & 0xF) as u8, ((cap >> 4) & 0x3F) as u8)));
        }
        p = ((h >> 8) & 0xFC) as u8;
        guard += 1;
    }
    None
}

unsafe fn mmio32(base: u64, reg: u64) -> u32 {
    core::ptr::read_volatile((base + reg) as *const u32)
}

/// GPU chip name from PMC_BOOT_42's chipset field.
pub fn chip_name(chipset: u32) -> &'static str {
    match chipset {
        0x162 => "TU102",
        0x164 => "TU104",
        0x166 => "TU106",
        0x167 => "TU117",
        0x168 => "TU116",
        0x170 => "GA100",
        0x172 => "GA102",
        0x173 => "GA103",
        0x174 => "GA104",
        0x176 => "GA106",
        0x177 => "GA107",
        0x192 => "AD102",
        0x193 => "AD103",
        0x194 => "AD104",
        0x196 => "AD106",
        0x197 => "AD107",
        _ => "unknown",
    }
}

fn arch_name(chipset: u32) -> &'static str {
    match chipset & 0x1F0 {
        0x160 => "Turing",
        0x170 => "Ampere",
        0x190 => "Ada Lovelace",
        0x1A0 | 0x1B0 => "Blackwell",
        0x140 => "Volta",
        0x130 => "Pascal",
        0x110 | 0x120 => "Maxwell",
        _ => "older / unknown",
    }
}

impl Gpu {
    pub fn chipset(&self) -> u32 {
        if self.boot42 != 0 && self.boot42 != 0xFFFF_FFFF {
            (self.boot42 >> 20) & 0x1FF
        } else {
            (self.boot0 >> 20) & 0x1FF
        }
    }
}

fn probe_one(loc: Location) -> Gpu {
    let id = loc.read32(0x00);
    let device_id = (id >> 16) as u16;
    let subsys = loc.read32(0x2C);
    let cmd = loc.read32(0x04);
    // size BARs 0 and 1 with memory decoding off (interrupts off: nothing else
    // may touch the card's windows -- the framebuffer is in BAR1) and restore
    let (bar0, bar0_len, bar1, bar1_len) = unsafe {
        let flags = crate::arch::cpu::read_flags();
        crate::arch::cpu::cli();
        pci::write32(loc.bus, loc.device, loc.function, 0x04, cmd & !0x0002);
        let (b0, l0, n0) = bar(&loc, 0);
        let (b1, l1, _) = bar(&loc, n0);
        pci::write32(loc.bus, loc.device, loc.function, 0x04, cmd);
        crate::arch::cpu::write_flags(flags);
        (b0, l0, b1, l1)
    };
    let mut g = Gpu { loc, device_id, subsys, bar0, bar0_len, bar1, bar1_len, boot0: 0, boot42: 0, vram_mib: None, link: pcie_link(&loc), note: String::new() };
    if cmd & 0x0002 == 0 {
        g.note = String::from("memory decoding is off (firmware did not enable the card); registers not read");
        return g;
    }
    // BAR0 must lie in the kernel's identity map (the low 4 GiB)
    if bar0 == 0 || bar0 + bar0_len.max(0x100_0000) > 1 << 32 {
        g.note = format!("BAR0 at 0x{bar0:x} is outside the mapped low 4 GiB; registers not read");
        return g;
    }
    unsafe {
        g.boot0 = mmio32(bar0, NV_PMC_BOOT_0);
        if g.boot0 == 0xFFFF_FFFF {
            g.note = String::from("PMC_BOOT_0 reads all-ones: the card is powered down (D3) or not decoding");
            return g;
        }
        g.boot42 = mmio32(bar0, NV_PMC_BOOT_42);
        let cs = g.chipset();
        if cs >= 0x172 && matches!(cs & 0x1F0, 0x170 | 0x190) {
            let mb = mmio32(bar0, NV_USABLE_FB_SIZE_IN_MB);
            if mb > 0 && mb < 1 << 20 && mb & 0xBAD0_0000 != 0xBAD0_0000 {
                g.vram_mib = Some(mb);
            }
        }
    }
    g
}

/// Find every NVIDIA display controller (once) and return them.
pub fn probe() -> &'static [Gpu] {
    let g = unsafe { &mut *(&raw mut GPUS) };
    if g.is_none() {
        *g = Some(pci::find_all(NV_VENDOR, 0x03).into_iter().map(probe_one).collect());
    }
    g.as_ref().unwrap()
}

fn mib(n: u64) -> String {
    if n >= 1 << 30 {
        format!("{} GiB", n >> 30)
    } else {
        format!("{} MiB", n >> 20)
    }
}

/// The `gpu` command (console / SSH).
pub fn command(_arg: &str) -> String {
    let gpus = probe();
    if gpus.is_empty() {
        return String::from("gpu: no NVIDIA GPU on the PCI bus (QEMU and VirtualBox have none: this needs the real machine)\n");
    }
    let fb = crate::drivers::framebuffer::get();
    let mut s = String::new();
    for g in gpus {
        let cs = g.chipset();
        s.push_str(&format!(
            "gpu {:02x}:{:02x}.{}  NVIDIA 10de:{:04x} (subsystem {:04x}:{:04x})\n",
            g.loc.bus,
            g.loc.device,
            g.loc.function,
            g.device_id,
            g.subsys & 0xFFFF,
            g.subsys >> 16
        ));
        if g.boot0 != 0 {
            s.push_str(&format!(
                "  chip {} ({}), chipset 0x{:03x}, PMC_BOOT_0 0x{:08x}, PMC_BOOT_42 0x{:08x}\n",
                chip_name(cs),
                arch_name(cs),
                cs,
                g.boot0,
                g.boot42
            ));
        }
        s.push_str(&format!("  BAR0 registers 0x{:x} ({}), BAR1 VRAM aperture 0x{:x} ({})\n", g.bar0, mib(g.bar0_len), g.bar1, mib(g.bar1_len)));
        if let Some(v) = g.vram_mib {
            s.push_str(&format!("  VRAM {} MiB ({:.1} GiB)\n", v, v as f64 / 1024.0));
        }
        if let Some(((g0, w0), (g1, w1))) = g.link {
            s.push_str(&format!("  PCIe link Gen{g0} x{w0} (card + slot can do Gen{g1} x{w1})\n"));
        }
        if let Some(f) = fb {
            if g.bar1_len > 0 && f.addr >= g.bar1 && f.addr < g.bar1 + g.bar1_len {
                s.push_str(&format!("  the screen ({}x{}) is the firmware's framebuffer, in this card's VRAM at BAR1+0x{:x}\n", f.width, f.height, f.addr - g.bar1));
            }
        }
        if !g.note.is_empty() {
            s.push_str(&format!("  note: {}\n", g.note));
        }
    }
    s.push_str("  stage 1 driver: identify only. No GSP firmware is loaded, so no engines (copy, graphics, CUDA compute) run yet.\n");
    s
}
