//! A boot-relative clock/delay source built on the ARM generic timer's
//! physical counter (`CNTPCT_EL0`), the aarch64 counterpart of the x86_64
//! backend's TSC-based `timer.rs`. Unlike the x86_64 side, no PIT-style
//! calibration is needed: `CNTFRQ_EL0` is a fixed, firmware-provided
//! frequency (62.5MHz on QEMU's `raspi3b` and real Pi 3/4 hardware), not
//! something that has to be measured against a second reference clock.
use core::arch::asm;

static mut CNTFRQ: u64 = 0;
static mut CNT_AT_BOOT: u64 = 0;

unsafe fn cntpct() -> u64 {
    let val: u64;
    asm!("mrs {}, cntpct_el0", out(reg) val, options(nomem, nostack));
    val
}

unsafe fn cntfrq() -> u64 {
    let val: u64;
    asm!("mrs {}, cntfrq_el0", out(reg) val, options(nomem, nostack));
    val
}

/// Stamp the boot-time counter reading `now_us`/`now_ms`/`delay_us` are
/// relative to. Call once, early in `init()`, before anything that needs
/// timing.
pub fn calibrate() {
    unsafe {
        CNTFRQ = cntfrq();
        if CNTFRQ == 0 {
            // A real board/QEMU always provides this; 0 would make every
            // call below divide-by-zero. Never observed in practice, but a
            // plausible fallback (Pi 3/4's actual 62.5MHz) beats a fault.
            CNTFRQ = 62_500_000;
        }
        CNT_AT_BOOT = cntpct();
    }
}

/// Microseconds since `calibrate()` was called.
pub fn now_us() -> u64 {
    unsafe { (cntpct() - CNT_AT_BOOT) * 1_000_000 / CNTFRQ }
}

pub fn now_ms() -> u64 {
    now_us() / 1000
}

/// Busy-wait for at least `us` microseconds.
pub fn delay_us(us: u64) {
    let target = unsafe { cntpct() } + us * unsafe { CNTFRQ } / 1_000_000;
    while unsafe { cntpct() } < target {
        unsafe { super::cpu::pause() };
    }
}

/// Poll `cond` until it's true or `timeout_us` microseconds pass. Returns
/// whether `cond` was actually satisfied — `false` means it timed out. See
/// the x86_64 backend's `poll_until` doc for why every hardware-wait loop
/// in this kernel goes through this instead of an unbounded spin.
pub fn poll_until(timeout_us: u64, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = now_us() + timeout_us;
    loop {
        if cond() {
            return true;
        }
        if now_us() >= deadline {
            return false;
        }
        unsafe { super::cpu::pause() };
    }
}

/// Arm the physical timer (`CNTP_*_EL0`) to fire an IRQ `hz` times per
/// second via the ARM-local per-core timer line (`intc::init` routes
/// `CNTPNSIRQ` to core 0's IRQ line; `vectors.rs`'s IRQ handler re-arms this
/// on every tick, since the physical timer is a one-shot compare, not a
/// free-running periodic source like x86's PIT mode 3).
pub fn start_periodic(hz: u32) {
    unsafe {
        let interval = CNTFRQ / hz as u64;
        asm!("msr cntp_tval_el0, {}", in(reg) interval, options(nomem, nostack));
        asm!("msr cntp_ctl_el0, {}", in(reg) 1u64, options(nomem, nostack)); // ENABLE, unmasked
    }
}

/// Re-arm the next tick — called from the IRQ handler after each fire.
pub fn rearm_periodic(hz: u32) {
    unsafe {
        let interval = CNTFRQ / hz as u64;
        asm!("msr cntp_tval_el0, {}", in(reg) interval, options(nomem, nostack));
    }
}
