//! aarch64 entry point. Raspberry Pi firmware loads `kernel8.img` as a raw
//! binary (no ELF program headers), so unlike the x86/Multiboot2 path, nobody
//! else sets up a stack or zeroes .bss for us — this module does both before
//! handing off to the per-project generated `kernel_entry`.
use core::arch::global_asm;

#[cfg(not(test))]
global_asm!(
    ".section .text.boot",
    ".global _start",
    "_start:",
    "    mrs x0, mpidr_el1",
    "    and x0, x0, #0xFF",
    "    cbz x0, primary",
    "1:  wfe", // secondary cores: park forever
    "    b 1b",
    "primary:",
    // Drop EL2 -> EL1 if the firmware handed off with the hypervisor
    // extension present (QEMU's `raspi3b` and real Pi firmware both do)
    // but unused -- this kernel has no hypervisor role, and every register
    // Phase 2 touches (VBAR_EL1, the physical generic timer's CNTP_*_EL0)
    // is either EL1-only or traps to EL2 by default, so staying at EL2
    // would silently break both.
    "    mrs x0, CurrentEL",
    "    lsr x0, x0, #2",
    "    cmp x0, #2",
    "    b.ne el1_entry",
    "    mov x0, #(1 << 31)", // HCR_EL2.RW: EL1 runs AArch64
    "    msr hcr_el2, x0",
    "    mrs x0, cnthctl_el2",
    "    orr x0, x0, #3", // EL1PCTEN | EL1PCEN: untrap CNTP_*_EL0 from EL1
    "    msr cnthctl_el2, x0",
    "    msr cntvoff_el2, xzr",
    // CPTR_EL2.TFP (bit 10) defaults to an implementation-defined value on
    // real hardware and QEMU alike -- leaving it set traps every EL1 FP/
    // NEON instruction to EL2 instead of EL1's own CPACR_EL1 (below) ever
    // getting a say. Clearing it here is the aarch64 counterpart of
    // `boot.rs`'s x86_64 twin explicitly enabling SSE via CR0/CR4 before
    // any Rust code runs: confirmed necessary the same way that one was --
    // LLVM auto-vectorizes even an ordinary `[u8; 16]` fill loop into a
    // NEON `str q0` on this target (NEON is baseline, not optional, for
    // AArch64 unlike x86's SSE), and every such instruction trapped until
    // this was set, recursively re-trapping the fault handler's own
    // `console_write` calls into a self-sustaining exception storm.
    "    msr cptr_el2, xzr",
    "    mov x0, #0x3C5", // SPSR: EL1h, D|A|I|F all masked
    "    msr spsr_el2, x0",
    "    adr x0, el1_entry",
    "    msr elr_el2, x0",
    "    eret",
    "el1_entry:",
    // CPACR_EL1.FPEN = 0b11 (bits 21:20): don't trap FP/NEON access at
    // EL1 (or, later, EL0) to EL1 itself either -- the other half of the
    // same fix, needed even when boot skipped the EL2 drop entirely (an
    // EL1-native boot path would still reset with FP/NEON trapped).
    "    mov x0, #(0b11 << 20)",
    "    msr cpacr_el1, x0",
    "    isb",
    "    ldr x0, =_stack_top",
    "    mov sp, x0",
    "    bl kernel_entry",
    "    b 1b",
);

extern "C" {
    static mut __bss_start: u8;
    static mut __bss_end: u8;
    static _kernel_end: u8;
}

/// First physical address past the kernel's own image + boot stack
/// (`RPI_LINKER_SCRIPT`'s `_kernel_end`, right after `_stack_top`) — the
/// low-water mark `mm::frame`'s mailbox-based memory detection clips
/// against, same reasoning as the x86_64 backend's `kernel_end`.
pub fn kernel_end() -> u64 {
    (&raw const _kernel_end) as u64
}

/// Zero the .bss region (linker-defined `__bss_start`/`__bss_end`). Must run
/// before any Rust code relies on zero-initialized statics.
pub unsafe fn zero_bss() {
    let start = &raw mut __bss_start as usize;
    let end = &raw mut __bss_end as usize;
    let mut p = start as *mut u8;
    while (p as usize) < end {
        core::ptr::write_volatile(p, 0);
        p = p.add(1);
    }
}
