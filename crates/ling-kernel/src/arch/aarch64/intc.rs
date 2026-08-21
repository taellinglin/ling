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
use crate::arch::mmio::{read32, write32, PERIPHERAL_BASE};

const ARM_LOCAL_BASE: usize = 0x4000_0000;
const CORE0_TIMER_IRQCNTL: usize = ARM_LOCAL_BASE + 0x40;
const CORE0_IRQ_SOURCE: usize = ARM_LOCAL_BASE + 0x60;
const CNTPNSIRQ_ENABLE: u32 = 1 << 1;
const CORE_SOURCE_CNTPNSIRQ: u32 = 1 << 1;

const VC_IRQ_BASE: usize = PERIPHERAL_BASE + 0xB000;
const IRQ_PENDING_2: usize = VC_IRQ_BASE + 0x208;
const ENABLE_IRQS_2: usize = VC_IRQ_BASE + 0x214;
/// UART0 is shared peripheral IRQ 57; the "IRQx2" pending/enable registers
/// cover peripheral IRQs 32..63, so it's bit (57-32).
const UART0_IRQ_BIT: u32 = 1 << (57 - 32);

/// Route the non-secure physical timer's IRQ to core 0 and enable UART0's
/// shared peripheral IRQ. Both lines stay masked at the CPU level (DAIF)
/// until `lib.rs::init` explicitly unmasks them, same discipline as the
/// x86_64 side's PIC setup before its first `cpu::sti`.
pub fn init() {
    unsafe {
        write32(CORE0_TIMER_IRQCNTL, CNTPNSIRQ_ENABLE);
        write32(ENABLE_IRQS_2, UART0_IRQ_BIT);
    }
}

pub fn core_timer_pending() -> bool {
    unsafe { read32(CORE0_IRQ_SOURCE) & CORE_SOURCE_CNTPNSIRQ != 0 }
}

pub fn uart_pending() -> bool {
    unsafe { read32(IRQ_PENDING_2) & UART0_IRQ_BIT != 0 }
}
