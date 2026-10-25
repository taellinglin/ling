//! GIC-400 (ARM Generic Interrupt Controller v2) driver for the boards that
//! have one: the Pi 4 (BCM2711) and the Pi 5 (BCM2712). The Pi 3's legacy
//! VC / QA7 controllers stay in `intc.rs`.
//!
//! Only what this kernel needs: the distributor and CPU interface are enabled,
//! the generic timer's non-secure physical PPI (interrupt 30, `CNTPNSIRQ`) is
//! unmasked, and the IRQ vector acknowledges / ends interrupts through the
//! CPU interface. Shared peripheral interrupts are left masked; the UART and
//! USB keep polling.
//!
//! The firmware's ARM stub hands the kernel over in the non-secure world with
//! every interrupt already in group 1, which is what lets `GICC_IAR` return
//! the timer's id here. The addresses are the SoC's:
//! Pi 4 GICD 0xFF84_1000 / GICC 0xFF84_2000, Pi 5 GICD 0x10_7FFF_9000 /
//! GICC 0x10_7FFF_A000 (inside the device windows the page tables map).
use crate::arch::mmio::{board, read32, write32, RpiBoard};

const GICD_CTLR: usize = 0x000;
const GICD_ISENABLER: usize = 0x100;
const GICD_ICENABLER: usize = 0x180;
const GICD_ICPENDR: usize = 0x280;
const GICD_IPRIORITYR: usize = 0x400;
const GICD_IGROUPR: usize = 0x080;
const GICC_CTLR: usize = 0x000;
const GICC_PMR: usize = 0x004;
const GICC_BPR: usize = 0x008;
const GICC_IAR: usize = 0x00C;
const GICC_EOIR: usize = 0x010;

/// The generic timer's non-secure physical interrupt (`CNTP_*_EL0`).
pub const IRQ_TIMER_PHYS_NS: u32 = 30;
/// The secure physical timer interrupt (`CNTPS`): what `CNTP_*_EL0` raises when
/// the kernel was entered in the secure world (QEMU without an ARM stub).
pub const IRQ_TIMER_PHYS_S: u32 = 29;

fn bases() -> Option<(usize, usize)> {
    match board() {
        RpiBoard::Pi4 => Some((0xFF84_1000, 0xFF84_2000)),
        RpiBoard::Pi5 => Some((0x10_7FFF_9000, 0x10_7FFF_A000)),
        _ => None,
    }
}

/// This board routes interrupts through a GIC (the legacy controllers are absent).
#[inline]
pub fn active() -> bool {
    bases().is_some()
}

/// Enable the distributor and this core's CPU interface and unmask the timer PPI.
/// IRQs stay masked at the core (DAIF) until the caller's `sti`.
pub fn init() {
    let Some((gicd, gicc)) = bases() else { return };
    unsafe {
        write32(gicd + GICD_CTLR, 0);
        // The banked private interrupts 0..31: mask all, clear pending, then
        // (where the write is honoured) make them group 1 so a non-secure core
        // sees them, and give the timer a middle priority.
        write32(gicd + GICD_ICENABLER, 0xFFFF_FFFF);
        write32(gicd + GICD_ICPENDR, 0xFFFF_FFFF);
        write32(gicd + GICD_IGROUPR, 0xFFFF_FFFF);
        for irq in [IRQ_TIMER_PHYS_NS, IRQ_TIMER_PHYS_S, 27, 26] {
            let pri_reg = gicd + GICD_IPRIORITYR + (irq as usize / 4) * 4;
            let shift = (irq % 4) * 8;
            let cur = read32(pri_reg) & !(0xFF << shift);
            write32(pri_reg, cur | (0x80 << shift));
        }
        write32(gicd + GICD_ISENABLER, (1 << IRQ_TIMER_PHYS_NS) | (1 << IRQ_TIMER_PHYS_S) | (1 << 27) | (1 << 26));
        // Bits 0 | 1: EnableGrp0 | EnableGrp1. Through the firmware's
        // non-secure view (real Pi 4 / 5) bit 0 is the group-1 enable and
        // bit 1 is ignored; on a GIC without security extensions (QEMU) the
        // interrupts made group 1 above need bit 1. Both worlds get IRQs.
        write32(gicd + GICD_CTLR, 3);
        write32(gicc + GICC_PMR, 0xFF);
        write32(gicc + GICC_BPR, 0);
        // bit 2 AckCtl: a secure-world read of IAR returns the real id of a
        // group-1 interrupt instead of 1022 (QEMU); ignored by the NS view
        write32(gicc + GICC_CTLR, 7);
    }
}

/// What the controller accepted, for the boot log: `(GICD_ISENABLER0, GICD_CTLR,
/// GICC_CTLR, CurrentEL)`. A zero timer bit in the enable mask means the
/// firmware left the private interrupts in group 0 and this (non-secure)
/// kernel cannot unmask them: no timer IRQs on that firmware.
pub fn status() -> (u32, u32, u32, u64) {
    let Some((gicd, gicc)) = bases() else { return (0, 0, 0, 0) };
    let el: u64;
    unsafe {
        core::arch::asm!("mrs {}, CurrentEL", out(reg) el, options(nomem, nostack));
        (read32(gicd + GICD_ISENABLER), read32(gicd + GICD_CTLR), read32(gicc + GICC_CTLR), el >> 2)
    }
}

/// `GICD_IGROUPR0` as this world reads it (all ones from the firmware's
/// non-secure view, whatever this kernel wrote on a GIC without security).
pub fn group0() -> u32 {
    match bases() {
        Some((gicd, _)) => unsafe { read32(gicd + GICD_IGROUPR) },
        None => 0,
    }
}

/// Whether the timer interrupt is really unmasked at the distributor.
pub fn timer_enabled() -> bool {
    status().0 & ((1 << IRQ_TIMER_PHYS_NS) | (1 << IRQ_TIMER_PHYS_S)) != 0
}

/// Acknowledge the highest-priority pending interrupt: the raw `GICC_IAR`
/// value (pass it back to [`eoi`]); `None` when nothing is pending (1023).
pub fn ack() -> Option<u32> {
    let (_, gicc) = bases()?;
    let iar = unsafe { read32(gicc + GICC_IAR) };
    if iar & 0x3FF >= 1020 {
        None
    } else {
        Some(iar)
    }
}

/// Signal the end of the interrupt `ack` returned.
pub fn eoi(iar: u32) {
    if let Some((_, gicc)) = bases() {
        unsafe { write32(gicc + GICC_EOIR, iar) };
    }
}
