use core::arch::asm;

/// Wait-for-interrupt: park the core until the next interrupt/event (aarch64
/// equivalent of x86 `hlt`).
pub unsafe fn wfi() {
    asm!("wfi", options(nomem, nostack));
}

/// Alias for `wfi`, so arch-neutral callers can just say `cpu::halt()`.
pub unsafe fn halt() {
    wfi();
}

/// Wait-for-event (used to park secondary cores at boot).
pub unsafe fn wfe() {
    asm!("wfe", options(nomem, nostack));
}

/// Mask IRQs and FIQs (aarch64 equivalent of x86 `cli`).
pub unsafe fn cli() {
    asm!("msr daifset, #3", options(nomem, nostack));
}

/// Unmask IRQs and FIQs (aarch64 equivalent of x86 `sti`).
pub unsafe fn sti() {
    asm!("msr daifclr, #3", options(nomem, nostack));
}

pub unsafe fn pause() {
    asm!("yield", options(nomem, nostack));
}

/// Physical counter, free-running since boot (aarch64 equivalent of `rdtsc`).
pub unsafe fn cntpct() -> u64 {
    let val: u64;
    asm!("mrs {}, cntpct_el0", out(reg) val, options(nomem, nostack));
    val
}

/// Current core's affinity ID (low 8 bits identify the core on Pi 3/4/5).
pub unsafe fn core_id() -> u64 {
    let val: u64;
    asm!("mrs {}, mpidr_el1", out(reg) val, options(nomem, nostack));
    val & 0xFF
}

pub unsafe fn brk() {
    asm!("brk #0", options(nomem, nostack));
}
