//! 32-bit-to-long-mode bootstrap for the x86_64/GRUB target.
//!
//! GRUB's Multiboot2 handoff lands in 32-bit protected mode with paging
//! disabled *even for a 64-bit ELF kernel* — that's the Multiboot spec, not
//! a GRUB quirk (confirmed empirically: booting the plain 64-bit `_start`
//! directly triple-faults immediately, CR0/CR4/EFER all showing no long
//! mode / no paging at handoff). Nothing else sets up long mode for us, so
//! this is the real ELF entry point: it builds minimal identity-mapped
//! page tables (2MiB pages covering the full 4GiB 32-bit address space —
//! not just the first 1GiB of RAM: `ahci.rs` needs to reach a PCI BAR's
//! MMIO window, which firmware typically places up near the top of that
//! range, not down in RAM), enables PAE + long mode (EFER.LME) + paging,
//! loads a 64-bit GDT, and far-jumps into `kernel_entry` (defined by the
//! generated per-project `main.rs`, same name/convention as the
//! aarch64/Raspberry Pi boot path in `boot.rs`) — only past that jump is it
//! safe to run any 64-bit-compiled code, including the rest of this crate.
use core::arch::global_asm;

extern "C" {
    static stack_top: u8;
    static _kernel_end: u8;
    static ling_boot_pml4: u8;
    static ling_boot_pd_hi: u8;
}

/// IA32_PAT with entry 1 (PWT=1, PCD=0) turned from write-through into
/// write-combining, the memory type a framebuffer wants. Every core must use
/// the same PAT; `smp.rs` / `cpudrv.rs` call this on the others.
pub fn set_pat_wc() {
    unsafe {
        if super::cpu::cpuid(1, 0).3 & (1 << 16) == 0 {
            return; // no PAT
        }
        let pat = super::cpu::read_msr(0x277);
        super::cpu::write_msr(0x277, (pat & !0xFF00) | 0x0100);
    }
}

/// Map [phys, phys+len) above 4 GiB -- a framebuffer -- into the boot page
/// tables (2 MiB pages, write-combining), before `paging::init` builds the
/// real ones. Only one 1 GiB-aligned gigabyte can be added; false otherwise.
pub fn map_high_early(phys: u64, len: u64) -> bool {
    let gb = phys >> 30;
    if len == 0 || (phys + len - 1) >> 30 != gb || gb >= 512 || gb < 4 {
        return false;
    }
    set_pat_wc();
    unsafe {
        let pdpt = ((&raw const ling_boot_pml4) as u64 + 4096) as *mut u64;
        let pd = (&raw const ling_boot_pd_hi) as u64;
        for i in 0..512u64 {
            // present | writable | PWT (PAT entry 1 = write-combining) | 2 MiB
            core::ptr::write_volatile((pd as *mut u64).add(i as usize), ((gb << 30) + (i << 21)) | 0x8B);
        }
        core::ptr::write_volatile(pdpt.add(gb as usize), pd | 0x3);
        super::cpu::write_cr3(super::cpu::read_cr3());
    }
    true
}

extern "C" {
    static mut stack_bottom: u8;
}
const STACK_CANARY: u64 = 0x4C49_4E47_5354_4B21; // "LINGSTK!"

/// Plant the canary in the lowest 256 bytes of the boot stack (call early, while
/// the stack is shallow). If anything ever runs the stack that deep, the next
/// `stack_guard_ok` says so instead of the overflow silently trashing `.got`.
pub fn stack_guard_arm() {
    unsafe {
        let p = (&raw mut stack_bottom) as *mut u64;
        for i in 0..32 {
            core::ptr::write_volatile(p.add(i), STACK_CANARY);
        }
    }
}

/// False once the boot stack has been used down to its last 256 bytes.
pub fn stack_guard_ok() -> bool {
    unsafe {
        let p = (&raw const stack_bottom) as *const u64;
        (0..32).all(|i| core::ptr::read_volatile(p.add(i)) == STACK_CANARY)
    }
}

/// Top of the 4 MiB stack `_start` switched onto before `kernel_entry` —
/// the only stack that exists this early, so it's what `gdt::init` points
/// the TSS's `RSP0` at (used if a future ring-3 interrupt/syscall needs a
/// known-good kernel stack; unused while everything still runs in ring 0).
pub fn boot_stack_top() -> u64 {
    (&raw const stack_top) as u64
}

/// First physical address past the kernel's own image (`.text`/`.rodata`/
/// `.data`/`.bss`, page-4KiB-aligned by `KERNEL_LINKER_SCRIPT`) — the
/// low-water mark `mm::frame`'s Multiboot2 memory-map detection clips
/// every usable region against, since a bootloader's memory map has no
/// concept of "already occupied by the kernel it just loaded."
pub fn kernel_end() -> u64 {
    (&raw const _kernel_end) as u64
}

// Multiboot2 header: plain (no video tag, GRUB leaves VGA text mode alone —
// what every existing text-console kernel target needs) vs. one that also
// carries a framebuffer request tag (type 5), gated behind the
// `request_framebuffer` feature so only a kernel target with its own
// framebuffer-based renderer opts into it (see the Cargo.toml feature doc
// comment for why this must not be on by default).
#[cfg(all(not(test), not(feature = "request_framebuffer")))]
global_asm!(
    r#"
.section .ling_multiboot, "a"
.align 8
multiboot_header:
    .long 0xE85250D6
    .long 0
    .long multiboot_header_end - multiboot_header
    .long -(0xE85250D6 + 0 + (multiboot_header_end - multiboot_header))
    .align 8
    .short 0
    .short 0
    .long 8
multiboot_header_end:
"#
);

#[cfg(all(not(test), feature = "request_framebuffer"))]
global_asm!(
    r#"
.section .ling_multiboot, "a"
.align 8
multiboot_header:
    .long 0xE85250D6
    .long 0
    .long multiboot_header_end - multiboot_header
    .long -(0xE85250D6 + 0 + (multiboot_header_end - multiboot_header))
    // 0/0/0 means "any resolution/depth GRUB's video BIOS/VBE can give us"
    // — `framebuffer.rs` reads back whatever it actually picked from the
    // Multiboot2 info structure rather than assuming these numbers were
    // honored exactly.
    .align 8
    .short 5
    .short 0
    .long 20
    .long 0
    .long 0
    .long 0
    .align 8
    .short 0
    .short 0
    .long 8
multiboot_header_end:
"#
);

#[cfg(not(test))]
global_asm!(
    r#"
.section .text.boot, "ax"
.code32
.global _start
_start:
    cli
    lea esp, [stack_top]

    // Bare `mov reg, symbol` (no brackets) for "load this label's address"
    // is ambiguous across assemblers/dialects; `lea reg, [symbol]` is the
    // unambiguous, portable idiom for it, used everywhere below.
    lea eax, [pdpt]
    or eax, 0x3
    mov [pml4], eax

    // PDPT[0..4] -> pd0..pd3, each a 512-entry, 2MiB-page table covering
    // 1GiB, so the 4 of them together identity-map the entire 4GiB 32-bit
    // physical address space.
    lea eax, [pd0]
    or eax, 0x3
    mov [pdpt], eax
    lea eax, [pd1]
    or eax, 0x3
    mov [pdpt + 8], eax
    lea eax, [pd2]
    or eax, 0x3
    mov [pdpt + 16], eax
    lea eax, [pd3]
    or eax, 0x3
    mov [pdpt + 24], eax

    // Fill all 4 PDs with consecutive 2MiB pages — eax is the running
    // physical address, carried across all four unrolled loops (0 ->
    // 0x40000000 -> 0x80000000 -> 0xC0000000 -> wraps to 0 after the last
    // entry, which is fine since nothing reads it again after that).
    xor eax, eax
    lea edi, [pd0]
    mov ecx, 512
fill_pd0:
    mov edx, eax
    or edx, 0x83
    mov [edi], edx
    add edi, 8
    add eax, 0x200000
    dec ecx
    jnz fill_pd0

    lea edi, [pd1]
    mov ecx, 512
fill_pd1:
    mov edx, eax
    or edx, 0x83
    mov [edi], edx
    add edi, 8
    add eax, 0x200000
    dec ecx
    jnz fill_pd1

    lea edi, [pd2]
    mov ecx, 512
fill_pd2:
    mov edx, eax
    or edx, 0x83
    mov [edi], edx
    add edi, 8
    add eax, 0x200000
    dec ecx
    jnz fill_pd2

    lea edi, [pd3]
    mov ecx, 512
fill_pd3:
    mov edx, eax
    or edx, 0x83
    mov [edi], edx
    add edi, 8
    add eax, 0x200000
    dec ecx
    jnz fill_pd3

    lea eax, [pml4]
    mov cr3, eax

    mov eax, cr4
    or eax, 1 << 5
    mov cr4, eax

    mov ecx, 0xC0000080
    rdmsr
    or eax, 1 << 8
    wrmsr

    mov eax, cr0
    or eax, 1 << 31
    mov cr0, eax

    // Enable SSE: Ling's boxed/dynamic value representation NaN-boxes every
    // value as an f64 (even a plain integer comparison like `1 == 1` goes
    // through it unless a call's arguments specifically get the raw-int
    // fast path — see `translate_op_int`), so ordinary kernel .ling source
    // emits SSE2 instructions (movq to/from xmm, cvttsd2si, ...) that fault
    // with #UD until this is done (confirmed empirically: an actual QEMU
    // boot faulted on exactly such an instruction with SSE left disabled).
    mov eax, cr0
    and eax, 0xFFFFFFFB    // clear EM (bit 2): don't emulate/trap FP+SSE
    or eax, 0x2            // set MP (bit 1): required alongside EM handling
    mov cr0, eax

    mov eax, cr4
    or eax, (1 << 9) | (1 << 10)   // OSFXSR + OSXMMEXCPT
    mov cr4, eax

    lgdt [gdt64_pointer]
    // A direct far jump (`jmp seg:offset`) isn't accepted by the assembler
    // Rust's `global_asm!` uses here; push+far-return is the standard
    // portable way to reload CS with the 64-bit code segment instead.
    push 0x08
    lea eax, [long_mode_entry]
    push eax
    retf

.code64
long_mode_entry:
    mov ax, 0
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax
    // `ebx` still holds GRUB's Multiboot2 info-structure pointer here —
    // nothing between `_start` and this point touches it (the page-table
    // setup above only ever uses eax/edx/edi/ecx as scratch), and a far
    // jump/return only changes CS, not general-purpose registers. Stash it
    // in a plain global before it becomes just another scratch register, so
    // `framebuffer::init` (called later, from Rust, via `ling_kernel_init`)
    // can find the framebuffer tag GRUB filled in for the request above.
    mov [mb2_info_ptr], ebx
    call kernel_entry
halt_loop:
    hlt
    jmp halt_loop

.section .rodata
.align 16
gdt64:
    .quad 0
    .quad 0x00AF9A000000FFFF
gdt64_pointer:
    .short . - gdt64 - 1
    .long gdt64

.section .bss
.align 4
.global mb2_info_ptr
mb2_info_ptr:
    .skip 4
.align 4096
// Global so the SMP trampoline (smp.rs) can enter long mode on the other cores
// through this same flat 4 GiB identity map before they switch to the real one.
.global ling_boot_pml4
ling_boot_pml4:
pml4:
    .skip 4096
pdpt:
    .skip 4096
pd0:
    .skip 4096
pd1:
    .skip 4096
pd2:
    .skip 4096
pd3:
    .skip 4096
// One spare page directory: maps the 1 GiB around a framebuffer above 4 GiB
// (UEFI + a discrete GPU puts it in BAR1, e.g. 0x40_0000_0000) until the
// kernel's own page tables exist. Filled from Rust (`map_high_early`).
.global ling_boot_pd_hi
ling_boot_pd_hi:
    .skip 4096
.align 16
.global stack_bottom
stack_bottom:
    // 4 MiB (was 512 KiB until 2026-11-09: sign-in's keyring + Messenger
    // identity -- ML-DSA / ML-KEM frames of 364 KiB and 128 KiB -- ran this
    // stack down through .bss's start into .got, the linker's address table
    // just below it, and every bundled app (LingDev, LingStation) then jumped
    // through trashed pointers; `stack_guard_ok` now reports such an
    // overflow). Slot 0 (the desktop loop / main task) never gets one of
    // sched.rs's spawned STACKS -- it keeps running on THIS stack. It does far
    // more than early boot now: the whole window-manager frame loop plus, on
    // demand, post-quantum crypto (opening Messenger calls `my_ling_id()` ->
    // `Identity::from_seed`, an ML-DSA-87 keygen that expands a ~57KiB matrix,
    // if the background task hasn't cached the identity first; browser TLS is
    // similar). The old 64KiB silently overflowed the moment a second, richer
    // window (Messenger) was opened -- a corrupted return address faulted with
    // a non-canonical rip. This is BSS (zero-filled at boot, not on disk).
    .skip 4194304
.global stack_top
stack_top:
"#
);
