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

// Multiboot2 header: plain (no video tag, GRUB leaves VGA text mode alone —
// what every existing text-console kernel target needs) vs. one that also
// carries a framebuffer request tag (type 5), gated behind the
// `request_framebuffer` feature so only a kernel target with its own
// framebuffer-based renderer opts into it (see the Cargo.toml feature doc
// comment for why this must not be on by default).
#[cfg(not(feature = "request_framebuffer"))]
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

#[cfg(feature = "request_framebuffer")]
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
.align 16
stack_bottom:
    .skip 65536
stack_top:
"#
);
