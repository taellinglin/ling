//! RP1 south-bridge access for the Raspberry Pi 5 (BCM2712) — and the first
//! stage of USB bring-up.
//!
//! On the Pi 5 almost all external-facing I/O (the USB-A ports, Ethernet, the
//! GPIO header, the GPIO UART…) lives on a separate chip, **RP1**, which hangs
//! off the BCM2712 over a PCIe x4 link. RP1's register space is reached through
//! a PCIe outbound window the firmware sets up:
//!
//! ```text
//!   CPU physical <WINDOW>  ->  RP1 internal 0x4000_0000
//! ```
//!
//! so an RP1 peripheral at internal address `A` is reached at
//! `<WINDOW> + (A - 0x4000_0000)`. **The window base is not agreed on between
//! references** — the Circle project / RPi forums quote `0x1f_0000_0000`, while
//! the working BatMetal bare-metal kernel uses `0x1c_0000_0000`. On this board
//! `0x1f_0000_0000` reads back `0xDEADDEAD` poison (the address decodes but
//! nothing answers — the BCM2712 PCIe RC returns that for a read the endpoint
//! never completes), so the base is wrong or the link is down. This probe
//! therefore **scans the candidate bases** and tests each by reading RP1 UART0's
//! PL011 identification registers — read-only hardware constants
//! (`0x11,0x10,…`/`0x0D,0xF0,0x05,0xB1`) that come back correct *only* if the
//! window genuinely reaches RP1, regardless of whether the UART was configured.
//!
//! RP1's two USB controllers are Synopsys **DWC3** cores (host mode ⇒ xHCI):
//! usb0 @ RP1 `0x4020_0000`, usb1 @ RP1 `0x4030_0000`.
//!
//! Once a live window is found, stages 2–4 follow: DWC3 core init (soft reset,
//! GCTL host mode) + xHCI reset/run/rings/port-power, USB enumeration, and the
//! HID boot keyboard + mouse. If *no* candidate base reaches RP1, the firmware
//! did not leave the window up (`pciex4_reset=0` is reportedly insufficient on
//! some setups) and a full BCM2712 PCIe root-complex bring-up is required first.

use crate::arch::mmio::{board, read32, write32, RpiBoard};

/// RP1-internal address the window's base maps to.
const RP1_PERIPH_BASE: usize = 0x4000_0000;

/// Candidate CPU-physical bases for the firmware's RP1 outbound window.
const WINDOW_CANDIDATES: [usize; 2] = [0x1f_0000_0000, 0x1c_0000_0000];

/// RP1-internal addresses.
const RP1_UART0: usize = 0x4003_0000; // PL011 — ID registers used for liveness
const USB0_RP1: usize = 0x4020_0000; // DWC3 #0
const USB1_RP1: usize = 0x4030_0000; // DWC3 #1

/// DWC3 global register block lives at +0xC100 within a controller; the
/// Synopsys ID register (GSNPSID) is at +0xC120. Its high half is 0x5533 for a
/// DesignWare USB3 core.
const DWC3_GSNPSID: usize = 0xC120;

// ── xHCI register map (host-mode view of the DWC3) ───────────────────────────
// Capability registers are at the controller base; operational registers start
// at base + CAPLENGTH; each root-hub port's PORTSC is at op + 0x400 + (n-1)*0x10.
const OP_USBCMD: usize = 0x00;
const OP_USBSTS: usize = 0x04;
const OP_PORTSC0: usize = 0x400;
const OP_PORT_STRIDE: usize = 0x10;

const CMD_RUN: u32 = 1 << 0;
const CMD_HCRST: u32 = 1 << 1;
const STS_HCH: u32 = 1 << 0; // HCHalted
const STS_CNR: u32 = 1 << 11; // Controller Not Ready

const PORTSC_CCS: u32 = 1 << 0; // Current Connect Status
const PORTSC_PED: u32 = 1 << 1; // Port Enabled/Disabled
const PORTSC_PP: u32 = 1 << 9; // Port Power
const PORTSC_SPEED_SHIFT: u32 = 10; // bits [13:10]

/// The RP1 outbound-window base found live by the last `usb_probe` scan, for
/// later USB stages to reuse instead of re-scanning.
static mut RP1_WINDOW: Option<usize> = None;

#[allow(dead_code)]
pub(crate) fn rp1_window() -> Option<usize> {
    unsafe { core::ptr::read(&raw const RP1_WINDOW) }
}

fn wait_clear(addr: usize, mask: u32, tries: u32) -> bool {
    for _ in 0..tries {
        if unsafe { read32(addr) } & mask == 0 {
            return true;
        }
        crate::arch::timer::delay_us(50);
    }
    false
}

fn usb_speed_name(s: u32) -> &'static [u8] {
    match s {
        1 => b"Full-speed (USB1)",
        2 => b"Low-speed (USB1)",
        3 => b"High-speed (USB2)",
        4 => b"SuperSpeed (USB3)",
        5 => b"SuperSpeed+ (USB3.1)",
        _ => b"unknown",
    }
}

/// Stage 2 (DMA-free): reset one xHCI controller and report its root-hub ports.
/// Powers each port and prints whether a device is connected and at what speed —
/// which tells us if the keyboard/mouse are seen at the hardware level before we
/// build the DMA rings + enumeration. Everything here is MMIO register access
/// (outbound, CPU→RP1); no controller DMA, so no inbound-window setup needed yet.
fn xhci_reset_and_ports(name: &[u8], window: usize, usb_rp1: usize) {
    let cap = at(window, usb_rp1);
    let caplength = (unsafe { read32(cap) } & 0xFF) as usize;
    let op = cap + caplength;
    let hcs1 = unsafe { read32(cap + 0x04) };
    let max_ports = (hcs1 >> 24) & 0xFF;

    crate::console_write(name);
    crate::console_write(b" xHCI reset:\n");

    // Halt the controller if it's running, then wait for HCHalted.
    let cmd = unsafe { read32(op + OP_USBCMD) };
    if cmd & CMD_RUN != 0 {
        unsafe { write32(op + OP_USBCMD, cmd & !CMD_RUN) };
        let _ = wait_set(op + OP_USBSTS, STS_HCH, 20_000);
    }
    // Host controller reset, then wait for HCRST to self-clear and CNR to clear.
    unsafe { write32(op + OP_USBCMD, CMD_HCRST) };
    let reset_ok =
        wait_clear(op + OP_USBCMD, CMD_HCRST, 20_000) && wait_clear(op + OP_USBSTS, STS_CNR, 20_000);
    crate::console_write(if reset_ok {
        b"  HCRST complete, controller ready\n"
    } else {
        b"  HCRST TIMEOUT (controller did not come ready)\n"
    });

    // Power each root-hub port and report connection status.
    let mut p = 1u32;
    while p <= max_ports {
        let psc_off = op + OP_PORTSC0 + (p as usize - 1) * OP_PORT_STRIDE;
        let mut psc = unsafe { read32(psc_off) };
        if psc & PORTSC_PP == 0 {
            unsafe { write32(psc_off, psc | PORTSC_PP) };
            crate::arch::timer::delay_us(20_000); // power stabilization
            psc = unsafe { read32(psc_off) };
        }
        crate::console_write(b"  port ");
        print_dec(p);
        hexln(b" PORTSC=", psc as u64, 8);
        if psc & PORTSC_CCS != 0 {
            let speed = (psc >> PORTSC_SPEED_SHIFT) & 0xF;
            crate::console_write(b"    device CONNECTED, ");
            crate::console_write(usb_speed_name(speed));
            crate::console_write(if psc & PORTSC_PED != 0 { b" (enabled)\n" } else { b"\n" });
        } else {
            crate::console_write(b"    (empty)\n");
        }
        p += 1;
    }
}

fn wait_set(addr: usize, mask: u32, tries: u32) -> bool {
    for _ in 0..tries {
        if unsafe { read32(addr) } & mask != 0 {
            return true;
        }
        crate::arch::timer::delay_us(50);
    }
    false
}

fn print_dec(mut v: u32) {
    if v >= 100 {
        crate::console_write(&[b'0' + (v / 100) as u8]);
        v %= 100;
    }
    if v >= 10 {
        crate::console_write(&[b'0' + (v / 10) as u8]);
    }
    crate::console_write(&[b'0' + (v % 10) as u8]);
}

/// Map an RP1-internal address to CPU-physical through a given window base.
#[inline]
fn at(window: usize, internal: usize) -> usize {
    window + (internal - RP1_PERIPH_BASE)
}

/// Print `label` then `val` as `0x…` with `nibbles` hex digits, then newline —
/// over whatever `console_write` fans out to (serial + HDMI fbcon on Pi 5).
fn hexln(label: &[u8], val: u64, nibbles: usize) {
    crate::console_write(label);
    crate::console_write(b"0x");
    let mut buf = [0u8; 16];
    let n = nibbles.min(16);
    for i in 0..n {
        let nib = (val >> ((n - 1 - i) * 4)) & 0xF;
        buf[i] = if nib < 10 { b'0' + nib as u8 } else { b'a' + (nib - 10) as u8 };
    }
    crate::console_write(&buf[..n]);
    crate::console_write(b"\n");
}

/// Test whether `window` actually reaches RP1 by reading UART0's PL011 ID
/// registers (read-only constants: PeriphID0=0x11, PeriphID1=0x10, PCellID0=0x0D).
/// Prints the raw bytes and returns true if they match.
fn window_live(window: usize) -> bool {
    let u = at(window, RP1_UART0);
    let id0 = unsafe { read32(u + 0xFE0) } & 0xFF;
    let id1 = unsafe { read32(u + 0xFE4) } & 0xFF;
    let id2 = unsafe { read32(u + 0xFE8) } & 0xFF;
    let pcell0 = unsafe { read32(u + 0xFF0) } & 0xFF;
    hexln(b"  uart0 PeriphID0=", id0 as u64, 2);
    hexln(b"  uart0 PeriphID1=", id1 as u64, 2);
    hexln(b"  uart0 PeriphID2=", id2 as u64, 2);
    hexln(b"  uart0 PCellID0 =", pcell0 as u64, 2);
    id0 == 0x11 && id1 == 0x10 && pcell0 == 0x0D
}

/// Read and report one DWC3/xHCI controller's identity at a live window base.
/// Returns true if it looks like a usable DWC3/xHCI controller.
fn probe_controller(name: &[u8], window: usize, usb_rp1: usize) -> bool {
    let base = at(window, usb_rp1);
    crate::console_write(name);
    hexln(b" @ phys=", base as u64, 10);

    // xHCI capability registers (host-mode view of the DWC3).
    //   +0x00: [7:0] CAPLENGTH, [31:16] HCIVERSION
    //   +0x04: HCSPARAMS1 — [7:0] MaxSlots, [31:24] MaxPorts
    let cap0 = unsafe { read32(base + 0x00) };
    let caplength = cap0 & 0xFF;
    let hciversion = (cap0 >> 16) & 0xFFFF;
    let hcsparams1 = unsafe { read32(base + 0x04) };
    let max_slots = hcsparams1 & 0xFF;
    let max_ports = (hcsparams1 >> 24) & 0xFF;
    let gsnpsid = unsafe { read32(base + DWC3_GSNPSID) };

    hexln(b"  caplength=", caplength as u64, 2);
    hexln(b"  hciversion=", hciversion as u64, 4);
    hexln(b"  max_slots=", max_slots as u64, 2);
    hexln(b"  max_ports=", max_ports as u64, 2);
    hexln(b"  dwc3_gsnpsid=", gsnpsid as u64, 8);

    let is_dwc3 = (gsnpsid >> 16) == 0x5533;
    let xhci_ok =
        (0x18..=0x40).contains(&caplength) && (hciversion == 0x0100 || hciversion == 0x0110);
    if is_dwc3 {
        crate::console_write(b"  -> DesignWare USB3 (DWC3) core confirmed\n");
    } else if xhci_ok {
        crate::console_write(b"  -> xHCI capability registers look valid\n");
    } else {
        crate::console_write(b"  -> no sane xHCI/DWC3 id (controller gated, or window wrong)\n");
    }
    is_dwc3 || xhci_ok
}

/// Stage-1 USB probe: find a working RP1 window base, then report the USB
/// controllers there. Wired to the serial shell's `usb` command and auto-run at
/// boot. On any board other than a Pi 5 there is no RP1, so this is a no-op with
/// a note (keeps the command safe under QEMU raspi3b).
pub fn usb_probe() {
    if board() != RpiBoard::Pi5 {
        crate::console_write(b"usb: RP1/xHCI host is Pi 5 only (this board has no RP1)\n");
        return;
    }
    crate::console_write(b"RP1 USB probe (Pi 5)\n");

    let mut live: Option<usize> = None;
    for &w in WINDOW_CANDIDATES.iter() {
        hexln(b"scan window ", w as u64, 10);
        if window_live(w) {
            crate::console_write(b"  -> RP1 REACHED at this window\n");
            live = Some(w);
            break;
        }
        crate::console_write(b"  -> no RP1 here (poison/unmapped)\n");
    }

    match live {
        Some(w) => {
            unsafe { core::ptr::write(&raw mut RP1_WINDOW, Some(w)) };
            hexln(b"RP1 window = ", w as u64, 10);
            if probe_controller(b"usb0", w, USB0_RP1) {
                xhci_reset_and_ports(b"usb0", w, USB0_RP1);
            }
            if probe_controller(b"usb1", w, USB1_RP1) {
                xhci_reset_and_ports(b"usb1", w, USB1_RP1);
            }
        },
        None => {
            crate::console_write(b"RP1 unreachable at every candidate base.\n");
            crate::console_write(b"-> firmware did not leave the PCIe window up;\n");
            crate::console_write(b"   a BCM2712 PCIe root-complex bring-up is needed first.\n");
        },
    }
    crate::console_write(b"probe done.\n");
}
