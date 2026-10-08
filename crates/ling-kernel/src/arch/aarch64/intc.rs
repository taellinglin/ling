//! Interrupt routing for BCM2837 (Raspberry Pi 3 / QEMU's `raspi3b`, which
//! models the real SoC's controllers rather than a synthetic one). This
//! chip predates the GIC-400 that shows up starting with the Pi 4's
//! BCM2711 — there are two separate, older controllers instead:
//!
//! - The **ARM-local ("QA7") controller**, fixed at physical `0x4000_0000`
//!   regardless of the main peripheral base — per-core registers for
//!   routing the ARM generic timer's IRQ lines (`CNTPNSIRQ`, the one
//!   `CNTP_*_EL0` drives) to a specific core's IRQ input.
//! - The **legacy VC interrupt controller**, inside the main peripheral
//!   block (`mmio::PERIPHERAL_BASE + 0xB000`) — where every shared
//!   peripheral IRQ (UART, I2C, SPI, GPIO, ...) is individually
//!   enable/pending-checked.
//!
//! Both are handled here rather than a `gic.rs`: writing a GIC driver for a
//! controller this SoC doesn't have would be dead code on every board this
//! kernel actually targets.
use crate::arch::mmio::{board, peripheral_base, read32, write32, RpiBoard};

const ARM_LOCAL_BASE: usize = 0x4000_0000;
const CORE0_TIMER_IRQCNTL: usize = ARM_LOCAL_BASE + 0x40;
const CORE0_IRQ_SOURCE: usize = ARM_LOCAL_BASE + 0x60;
const CNTPNSIRQ_ENABLE: u32 = 1 << 1;
const CORE_SOURCE_CNTPNSIRQ: u32 = 1 << 1;

#[inline]
fn vc_irq_base() -> usize {
    peripheral_base() + 0xB000
}
const IRQ_PENDING_2_OFF: usize = 0x208;
const ENABLE_IRQS_2_OFF: usize = 0x214;
/// UART0 is shared peripheral IRQ 57; the "IRQx2" pending/enable registers
/// cover peripheral IRQs 32..63, so it's bit (57-32).
const UART0_IRQ_BIT: u32 = 1 << (57 - 32);

/// Pi 5 (BCM2712) has neither this legacy VC controller nor the QA7 ARM-local
/// block at 0x4000_0000 — it uses a GIC-400. Until a `gic.rs` exists, interrupt
/// routing is simply skipped there (the Pi 5 bring-up path runs IRQ-free), so
/// this must not poke those dead addresses.
#[inline]
fn legacy_absent() -> bool {
    board() == RpiBoard::Pi5
}

/// Route the non-secure physical timer's IRQ to core 0 and enable UART0's
/// shared peripheral IRQ. Both lines stay masked at the CPU level (DAIF)
/// until `lib.rs::init` explicitly unmasks them, same discipline as the
/// x86_64 side's PIC setup before its first `cpu::sti`.
pub fn init() {
    if legacy_absent() {
        return;
    }
    unsafe {
        write32(CORE0_TIMER_IRQCNTL, CNTPNSIRQ_ENABLE);
        write32(vc_irq_base() + ENABLE_IRQS_2_OFF, UART0_IRQ_BIT);
    }
}

pub fn core_timer_pending() -> bool {
    if legacy_absent() {
        return false;
    }
    unsafe { read32(CORE0_IRQ_SOURCE) & CORE_SOURCE_CNTPNSIRQ != 0 }
}

pub fn uart_pending() -> bool {
    if legacy_absent() {
        return false;
    }
    unsafe { read32(vc_irq_base() + IRQ_PENDING_2_OFF) & UART0_IRQ_BIT != 0 }
}
