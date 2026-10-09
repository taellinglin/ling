//! RP1 south-bridge access for the Raspberry Pi 5 (BCM2712) — and the first
//! stage of USB bring-up.
//!
//! On the Pi 5 almost all external-facing I/O (the USB-A ports, Ethernet, the
//! GPIO header, the GPIO UART…) lives on a separate chip, **RP1**, which hangs
//! off the BCM2712 over a PCIe x4 link. Bringing that PCIe root complex up from
//! scratch — RESCAL analog calibration, SerDes, 54→100 MHz PLL, PERST#
//! deassert, link training, inbound/outbound window programming — is a large
//! driver in its own right (see Broadcom's `pcie-brcmstb`). So we take the
//! pragmatic bare-metal path the Circle project uses: put
//!
//! ```text
//! pciex4_reset=0
//! ```
//!
//! in `config.txt` so the **firmware's** PCIe setup survives into our kernel
//! instead of being torn down just before the kernel loads. After that, RP1's
//! register space is directly addressable through a fixed outbound window the
//! firmware leaves in place:
//!
//! ```text
//!   CPU physical 0x1F_0000_0000  ->  RP1 internal 0x4000_0000   (a 4 MiB window)
//! ```
//!
//! so an RP1 peripheral at internal address `A` is reached at
//! `0x1F_0000_0000 + (A - 0x4000_0000)`. (The MMU is off on this kernel, so a
//! 64-bit pointer to that physical address is a plain device access — valid as
//! long as PCIe is up. If it is *not* up, the first read takes a synchronous
//! external abort, which the exception handler dumps as `far_el1=0x1f…` and
//! halts: itself a clear "PCIe window not mapped" signal rather than a silent
//! hang.)
//!
//! RP1 has two Synopsys **DWC3** USB controllers (host mode ⇒ they present an
//! xHCI register interface):
//!
//! ```text
//!   usb0 @ RP1 0x4020_0000  ->  0x1F_0020_0000
//!   usb1 @ RP1 0x4030_0000  ->  0x1F_0030_0000
//! ```
//!
//! **This module currently implements stage 1: reachability + identification.**
//! It proves the window is live and the DWC3/xHCI controllers answer with sane
//! capability registers, all reported over the serial console. The remaining
//! stages build on a stage-1 that's been confirmed on real hardware:
//!
//!   1. *(here)* RP1 window reachable; xHCI CAPLENGTH/HCIVERSION/HCSPARAMS and
//!      the DWC3 GSNPSID read back sane.
//!   2. DWC3 core init (soft reset, GCTL port direction = host) + xHCI reset &
//!      run (CRCR command ring, DCBAA, ERST event ring, port power).
//!   3. USB enumeration of the root-hub port a device is on (address device,
//!      read descriptors, set configuration).
//!   4. HID boot-protocol keyboard: SET_PROTOCOL(boot), poll the interrupt-IN
//!      endpoint, decode the 8-byte boot report, feed keys into the console.

use crate::arch::mmio::read32;
use crate::arch::mmio::{board, RpiBoard};

/// CPU-physical base of the firmware's outbound window onto RP1.
const RP1_WINDOW_PHYS: usize = 0x1F_0000_0000;
/// RP1-internal address the window's base maps to.
const RP1_PERIPH_BASE: usize = 0x4000_0000;

/// RP1-internal base addresses of the two DWC3 USB controllers.
const USB0_RP1: usize = 0x4020_0000;
const USB1_RP1: usize = 0x4030_0000;

/// DWC3 global register block lives at +0xC100 within a controller; the
/// Synopsys ID register (GSNPSID) is at +0xC120. Its high half is 0x5533 for a
/// DesignWare USB3 core.
const DWC3_GSNPSID: usize = 0xC120;

/// Map an RP1-internal address to its CPU-physical address through the window.
#[inline]
fn win(internal: usize) -> usize {
    RP1_WINDOW_PHYS + (internal - RP1_PERIPH_BASE)
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

/// Read and report one DWC3/xHCI controller's identity.
fn probe_controller(name: &[u8], usb_rp1: usize) {
    let base = win(usb_rp1);
    crate::console_write(name);
    hexln(b" @ phys=", base as u64, 10);

    // xHCI capability registers (host-mode view of the DWC3).
    //   +0x00: [7:0] CAPLENGTH, [31:16] HCIVERSION
    //   +0x04: HCSPARAMS1 — [7:0] MaxSlots, [18:8] MaxIntrs, [31:24] MaxPorts
    //   +0x10: HCCPARAMS1
    let cap0 = unsafe { read32(base + 0x00) };
    let caplength = cap0 & 0xFF;
    let hciversion = (cap0 >> 16) & 0xFFFF;
    let hcsparams1 = unsafe { read32(base + 0x04) };
    let hccparams1 = unsafe { read32(base + 0x10) };
    let max_slots = hcsparams1 & 0xFF;
    let max_ports = (hcsparams1 >> 24) & 0xFF;
    let gsnpsid = unsafe { read32(base + DWC3_GSNPSID) };

    hexln(b"  caplength=", caplength as u64, 2);
    hexln(b"  hciversion=", hciversion as u64, 4);
    hexln(b"  hcsparams1=", hcsparams1 as u64, 8);
    hexln(b"  hccparams1=", hccparams1 as u64, 8);
    hexln(b"  max_slots=", max_slots as u64, 2);
    hexln(b"  max_ports=", max_ports as u64, 2);
    hexln(b"  dwc3_gsnpsid=", gsnpsid as u64, 8);

    if (gsnpsid >> 16) == 0x5533 {
        crate::console_write(b"  -> DesignWare USB3 (DWC3) core confirmed\n");
    } else if (0x18..=0x40).contains(&caplength) && (hciversion == 0x0100 || hciversion == 0x0110)
    {
        crate::console_write(b"  -> xHCI capability registers look valid\n");
    } else {
        crate::console_write(b"  -> no sane xHCI/DWC3 id (controller off, or window not mapped)\n");
    }
}

/// Stage-1 USB probe: confirm the RP1 window is live and both USB controllers
/// identify themselves. Wired to the serial shell's `usb` command. On any board
/// other than a Pi 5 there is no RP1, so this is a no-op with a note (keeps the
/// command safe to run under QEMU raspi3b). If `pciex4_reset=0` is missing, the
/// first read faults into the CPU exception dumper — which is itself a result.
pub fn usb_probe() {
    if board() != RpiBoard::Pi5 {
        crate::console_write(b"usb: RP1/xHCI host is Pi 5 only (this board has no RP1)\n");
        return;
    }
    crate::console_write(b"RP1 USB probe (Pi 5) -- needs pciex4_reset=0 in config.txt\n");
    hexln(b"window phys=", RP1_WINDOW_PHYS as u64, 10);
    probe_controller(b"usb0", USB0_RP1);
    probe_controller(b"usb1", USB1_RP1);
    crate::console_write(b"probe done.\n");
}
