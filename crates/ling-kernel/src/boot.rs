//! aarch64 entry point. Raspberry Pi firmware loads `kernel8.img` as a raw
//! binary (no ELF program headers), so unlike the x86/Multiboot2 path, nobody
//! else sets up a stack or zeroes .bss for us — this module does both before
//! handing off to the per-project generated `kernel_entry`.
use core::arch::global_asm;

global_asm!(
    ".section .text.boot",
    ".global _start",
    "_start:",
    "    mrs x0, mpidr_el1",
    "    and x0, x0, #0xFF",
    "    cbz x0, 2f",
    "1:  wfe",              // secondary cores: park forever
    "    b 1b",
    "2:  ldr x0, =_stack_top",
    "    mov sp, x0",
    "    bl kernel_entry",
    "    b 1b",
);

extern "C" {
    static mut __bss_start: u8;
    static mut __bss_end: u8;
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
