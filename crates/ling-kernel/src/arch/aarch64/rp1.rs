//! RP1 south-bridge access for the Raspberry Pi 5 (BCM2712) — window discovery
//! and the USB controller probe.
//!
//! On the Pi 5 almost all external-facing I/O (USB-A ports, Ethernet, GPIO, the
//! GPIO UART…) lives on the **RP1** chip, which hangs off the BCM2712 over a
//! PCIe x4 link. RP1's register space is reached through a PCIe outbound window
//! the firmware leaves mapped (with `pciex4_reset=0`):
//!
//! ```text
//!   CPU physical <WINDOW>  ->  RP1 internal 0x4000_0000
//! ```
//!
//! **Confirmed on real hardware: the window base is `0x1c_0000_0000`** (not the
//! `0x1f_0000_0000` the Circle project / forums quote — that reads back
//! `0xDEADDEAD` poison here). We scan both and verify by reading RP1 UART0's
//! PL011 identification registers, which are read-only hardware constants
//! (`0x11,0x10,…`) that only come back correct if the window truly reaches RP1.
//!
//! RP1's two USB controllers are Synopsys **DWC3** cores (host mode ⇒ xHCI):
//! usb0 @ RP1 `0x4020_0000`, usb1 @ RP1 `0x4030_0000`. Once a live window is
//! found and a controller identified, enumeration is handed to [`crate::arch::xhci`].

use crate::arch::mmio::{board, read32, RpiBoard};

/// RP1-internal address the window's base maps to.
const RP1_PERIPH_BASE: usize = 0x4000_0000;

/// Candidate CPU-physical bases for the firmware's RP1 outbound window.
const WINDOW_CANDIDATES: [usize; 2] = [0x1f_0000_0000, 0x1c_0000_0000];

/// RP1-internal addresses.
const RP1_UART0: usize = 0x4003_0000; // PL011 — ID registers used for liveness
const USB0_RP1: usize = 0x4020_0000; // DWC3 #0
const USB1_RP1: usize = 0x4030_0000; // DWC3 #1

/// DWC3 Synopsys ID register (GSNPSID) at controller +0xC120; high half 0x5533.
const DWC3_GSNPSID: usize = 0xC120;

/// The RP1 outbound-window base found live by the last `usb_probe` scan.
static mut RP1_WINDOW: Option<usize> = None;

#[allow(dead_code)]
pub(crate) fn rp1_window() -> Option<usize> {
    unsafe { core::ptr::read(&raw const RP1_WINDOW) }
}

/// Map an RP1-internal address to CPU-physical through a given window base.
#[inline]
pub(crate) fn at(window: usize, internal: usize) -> usize {
    window + (internal - RP1_PERIPH_BASE)
}

/// Print `label` then `val` as `0x…` with `nibbles` hex digits, then newline.
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

/// Test whether `window` reaches RP1 by reading UART0's PL011 ID registers
/// (read-only constants: PeriphID0=0x11, PeriphID1=0x10, PCellID0=0x0D).
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

/// Read and report one DWC3/xHCI controller's identity. Returns true if usable.
fn probe_controller(name: &[u8], window: usize, usb_rp1: usize) -> bool {
    let base = at(window, usb_rp1);
    crate::console_write(name);
    hexln(b" @ phys=", base as u64, 10);

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

/// USB probe + enumeration: find the live RP1 window, identify both USB
/// controllers, and run xHCI enumeration on each. Auto-run at boot and wired to
/// the serial shell's `usb` command. No-op note on non-Pi-5 boards.
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
                crate::arch::xhci::enumerate(at(w, USB0_RP1), b"usb0");
            }
            if probe_controller(b"usb1", w, USB1_RP1) {
                crate::arch::xhci::enumerate(at(w, USB1_RP1), b"usb1");
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
