//! Interrupt Descriptor Table: all 32 CPU exception vectors with a readable
//! fault dump (vector, name, error code, RIP/CS/RFLAGS/RSP/SS, and CR2 for
//! page faults) instead of the silent triple-fault this kernel had before —
//! and the three hardware IRQs currently in use (PIT heartbeat, PS/2
//! keyboard, PS/2 mouse), remapped by `pic.rs` onto vectors 32+.
//!
//! Requires `#![feature(abi_x86_interrupt)]` at the crate root: this is the
//! only correct way to write an ISR in Rust without hand-writing the
//! prologue/epilogue in `global_asm!` — the compiler emits the right
//! register save/restore and `iretq` itself, using the same hardware-pushed
//! frame layout `InterruptFrame` mirrors below.
use super::{cpu, gdt, io, pic};
use core::mem::size_of;

#[repr(C)]
pub struct InterruptFrame {
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

#[repr(C, packed)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

const IDT_ENTRIES: usize = 256;
const NULL_ENTRY: IdtEntry = IdtEntry {
    offset_low: 0,
    selector: 0,
    ist: 0,
    type_attr: 0,
    offset_mid: 0,
    offset_high: 0,
    reserved: 0,
};
static mut IDT: [IdtEntry; IDT_ENTRIES] = [NULL_ENTRY; IDT_ENTRIES];

const GATE_PRESENT_INTERRUPT: u8 = 0x8E;

fn set_gate(vector: u8, handler: u64, ist: u8) {
    unsafe {
        IDT[vector as usize] = IdtEntry {
            offset_low: (handler & 0xFFFF) as u16,
            selector: gdt::KERNEL_CODE_SEL,
            ist,
            type_attr: GATE_PRESENT_INTERRUPT,
            offset_mid: ((handler >> 16) & 0xFFFF) as u16,
            offset_high: ((handler >> 32) & 0xFFFF_FFFF) as u32,
            reserved: 0,
        };
    }
}

// ─── Fault dump ──────────────────────────────────────────────────────────

fn print_hex64(label: &str, val: u64) {
    crate::console_write(label.as_bytes());
    crate::console_write(b"0x");
    let mut buf = [0u8; 16];
    for i in 0..16 {
        let nibble = (val >> ((15 - i) * 4)) & 0xF;
        buf[i] = if nibble < 10 {
            b'0' + nibble as u8
        } else {
            b'a' + (nibble - 10) as u8
        };
    }
    crate::console_write(&buf);
    crate::console_write(b"\n");
}

fn fault_dump(vector: u8, name: &str, frame: &InterruptFrame, error_code: Option<u64>) {
    crate::console_write(b"\n!!! CPU EXCEPTION: ");
    crate::console_write(name.as_bytes());
    crate::console_write(b" (vector ");
    print_hex64("", vector as u64);
    if let Some(code) = error_code {
        print_hex64("  error_code=", code);
    }
    print_hex64("  rip=", frame.rip);
    print_hex64("  cs=", frame.cs);
    print_hex64("  rflags=", frame.rflags);
    print_hex64("  rsp=", frame.rsp);
    print_hex64("  ss=", frame.ss);
    if vector == 14 {
        print_hex64("  cr2=", unsafe { cpu::read_cr2() });
    }
    print_hex64("  cr3=", unsafe { cpu::read_cr3() });
}

fn fault_halt(vector: u8, name: &str, frame: &InterruptFrame, error_code: Option<u64>) -> ! {
    fault_dump(vector, name, frame, error_code);
    crate::console_write(b"!!! kernel halted\n");
    loop {
        unsafe { cpu::halt() };
    }
}

macro_rules! exception_fatal_no_err {
    ($name:ident, $vec:expr, $desc:expr) => {
        extern "x86-interrupt" fn $name(frame: InterruptFrame) {
            fault_halt($vec, $desc, &frame, None);
        }
    };
}

macro_rules! exception_fatal_with_err {
    ($name:ident, $vec:expr, $desc:expr) => {
        extern "x86-interrupt" fn $name(frame: InterruptFrame, error_code: u64) {
            fault_halt($vec, $desc, &frame, Some(error_code));
        }
    };
}

extern "x86-interrupt" fn isr_breakpoint(frame: InterruptFrame) {
    // Not fatal: a debugger hitting `int3` expects execution to resume, and
    // there is no reason a breakpoint should take the kernel down. RIP
    // already points past the `int3` opcode (a trap, not a fault), so a
    // plain `iretq` continues correctly.
    fault_dump(3, "Breakpoint", &frame, None);
}

exception_fatal_no_err!(isr_divide_error, 0, "Divide Error");
exception_fatal_no_err!(isr_debug, 1, "Debug");
exception_fatal_no_err!(isr_nmi, 2, "Non-Maskable Interrupt");
exception_fatal_no_err!(isr_overflow, 4, "Overflow");
exception_fatal_no_err!(isr_bound_range, 5, "Bound Range Exceeded");
exception_fatal_no_err!(isr_invalid_opcode, 6, "Invalid Opcode");
exception_fatal_no_err!(isr_device_not_available, 7, "Device Not Available");
exception_fatal_with_err!(isr_double_fault, 8, "Double Fault");
exception_fatal_no_err!(isr_coprocessor_overrun, 9, "Coprocessor Segment Overrun");
exception_fatal_with_err!(isr_invalid_tss, 10, "Invalid TSS");
exception_fatal_with_err!(isr_segment_not_present, 11, "Segment Not Present");
exception_fatal_with_err!(isr_stack_segment_fault, 12, "Stack-Segment Fault");
exception_fatal_with_err!(isr_general_protection, 13, "General Protection Fault");
exception_fatal_with_err!(isr_page_fault, 14, "Page Fault");
exception_fatal_no_err!(isr_reserved_15, 15, "Reserved");
exception_fatal_no_err!(isr_x87_fp, 16, "x87 Floating-Point Exception");
exception_fatal_with_err!(isr_alignment_check, 17, "Alignment Check");
exception_fatal_no_err!(isr_machine_check, 18, "Machine Check");
exception_fatal_no_err!(isr_simd_fp, 19, "SIMD Floating-Point Exception");
exception_fatal_no_err!(isr_virtualization, 20, "Virtualization Exception");
exception_fatal_with_err!(isr_control_protection, 21, "Control Protection Exception");
exception_fatal_no_err!(isr_reserved_22, 22, "Reserved");
exception_fatal_no_err!(isr_reserved_23, 23, "Reserved");
exception_fatal_no_err!(isr_reserved_24, 24, "Reserved");
exception_fatal_no_err!(isr_reserved_25, 25, "Reserved");
exception_fatal_no_err!(isr_reserved_26, 26, "Reserved");
exception_fatal_no_err!(isr_reserved_27, 27, "Reserved");
exception_fatal_no_err!(
    isr_hypervisor_injection,
    28,
    "Hypervisor Injection Exception"
);
exception_fatal_with_err!(isr_vmm_communication, 29, "VMM Communication Exception");
exception_fatal_with_err!(isr_security, 30, "Security Exception");
exception_fatal_no_err!(isr_reserved_31, 31, "Reserved");

// ─── Hardware IRQs (vectors 32+, remapped by pic.rs) ────────────────────

// Vector 32 (the PIT heartbeat) is *not* wired here: it needs full
// general-purpose-register save/restore to drive `proc::uproc`'s
// preemptive scheduler, which the `extern "x86-interrupt"` ABI's
// compiler-managed prologue never exposes to Rust. See `trap.rs` for that
// handler (`timer_trap_entry`) and `ticks()` (moved there too, alongside
// the state it now also drives scheduling decisions from) — `init` below
// still installs it, just onto `gdt::TIMER_IST` instead of the shared
// no-IST gates every other vector uses.

/// Drain every byte the 8042 currently holds, routing each by the status
/// register's aux-origin bit (bit 5: set = mouse, clear = keyboard). Both
/// the IRQ1 and IRQ12 handlers call this instead of reading exactly one
/// byte, for two load-bearing reasons found the hard way (QEMU headless
/// `mouse_move` test, `wm.rs`'s serial diagnostics):
///
/// 1. The 8259 PIC is edge-triggered and the 8042 keeps its interrupt line
///    HIGH while more bytes are queued -- a one-byte-per-interrupt handler
///    reads byte 1, the line never falls, no new edge ever arrives, and the
///    remaining bytes wedge there forever (observed: status 0x3D -- OBF set,
///    aux origin, byte count frozen at 1).
/// 2. With both devices live, "which IRQ fired" does not reliably identify
///    the byte's origin -- the status bit does. Routing by IRQ number sent
///    mouse packets into the keyboard decoder on real interleavings.
fn drain_8042() {
    loop {
        let status = unsafe { io::inb(0x64) };
        if status & 0x01 == 0 {
            break;
        }
        let byte = unsafe { io::inb(0x60) };
        if status & 0x20 != 0 {
            crate::drivers::mouse::irq_byte(byte);
        } else {
            crate::drivers::keyboard::irq_scancode(byte);
        }
    }
}

/// Polled variant of the same drain, for callers outside interrupt context
/// (the WM's once-per-frame `ling_kernel_mouse_poll`). Interrupts are
/// masked across the drain so an IRQ1/IRQ12 can't interleave and split a
/// 3-byte mouse packet between the two paths. This exists because QEMU's
/// 8042 was observed holding OBF high with aux data queued while never
/// raising IRQ12 at all (see `drain_8042`'s doc) -- with a per-frame poll,
/// input keeps flowing on any 8042, IRQ-generous or not.
pub(crate) fn poll_drain_8042() {
    unsafe {
        core::arch::asm!("cli");
    }
    drain_8042();
    unsafe {
        core::arch::asm!("sti");
    }
}

extern "x86-interrupt" fn isr_keyboard(_frame: InterruptFrame) {
    drain_8042();
    pic::eoi(1);
}

extern "x86-interrupt" fn isr_mouse(_frame: InterruptFrame) {
    drain_8042();
    pic::eoi(12);
}

extern "x86-interrupt" fn isr_spurious(_frame: InterruptFrame) {
    // No handler registered for this vector but the line still fired (e.g.
    // a legitimately spurious IRQ7/IRQ15, or a device never masked). EOI
    // unconditionally so the controller doesn't wedge; there is nothing
    // useful to dump since no specific device claims this vector.
}

pub fn init() {
    set_gate(0, isr_divide_error as *const () as u64, 0);
    set_gate(1, isr_debug as *const () as u64, 0);
    set_gate(2, isr_nmi as *const () as u64, gdt::DOUBLE_FAULT_IST);
    set_gate(3, isr_breakpoint as *const () as u64, 0);
    set_gate(4, isr_overflow as *const () as u64, 0);
    set_gate(5, isr_bound_range as *const () as u64, 0);
    set_gate(6, isr_invalid_opcode as *const () as u64, 0);
    set_gate(7, isr_device_not_available as *const () as u64, 0);
    set_gate(
        8,
        isr_double_fault as *const () as u64,
        gdt::DOUBLE_FAULT_IST,
    );
    set_gate(9, isr_coprocessor_overrun as *const () as u64, 0);
    set_gate(10, isr_invalid_tss as *const () as u64, 0);
    set_gate(11, isr_segment_not_present as *const () as u64, 0);
    set_gate(12, isr_stack_segment_fault as *const () as u64, 0);
    set_gate(13, isr_general_protection as *const () as u64, 0);
    set_gate(14, isr_page_fault as *const () as u64, 0);
    set_gate(15, isr_reserved_15 as *const () as u64, 0);
    set_gate(16, isr_x87_fp as *const () as u64, 0);
    set_gate(17, isr_alignment_check as *const () as u64, 0);
    set_gate(
        18,
        isr_machine_check as *const () as u64,
        gdt::DOUBLE_FAULT_IST,
    );
    set_gate(19, isr_simd_fp as *const () as u64, 0);
    set_gate(20, isr_virtualization as *const () as u64, 0);
    set_gate(21, isr_control_protection as *const () as u64, 0);
    set_gate(22, isr_reserved_22 as *const () as u64, 0);
    set_gate(23, isr_reserved_23 as *const () as u64, 0);
    set_gate(24, isr_reserved_24 as *const () as u64, 0);
    set_gate(25, isr_reserved_25 as *const () as u64, 0);
    set_gate(26, isr_reserved_26 as *const () as u64, 0);
    set_gate(27, isr_reserved_27 as *const () as u64, 0);
    set_gate(28, isr_hypervisor_injection as *const () as u64, 0);
    set_gate(29, isr_vmm_communication as *const () as u64, 0);
    set_gate(30, isr_security as *const () as u64, 0);
    set_gate(31, isr_reserved_31 as *const () as u64, 0);

    for v in 32..IDT_ENTRIES {
        set_gate(v as u8, isr_spurious as *const () as u64, 0);
    }
    set_gate(pic::IRQ0_VECTOR, super::trap::timer_trap_entry as *const () as u64, gdt::TIMER_IST);
    set_gate(pic::IRQ0_VECTOR + 1, isr_keyboard as *const () as u64, 0);
    set_gate(pic::IRQ0_VECTOR + 12, isr_mouse as *const () as u64, 0);

    #[repr(C, packed)]
    struct IdtPointer {
        limit: u16,
        base: u64,
    }
    unsafe {
        let pointer = IdtPointer {
            limit: (size_of::<[IdtEntry; IDT_ENTRIES]>() - 1) as u16,
            base: (&raw const IDT) as u64,
        };
        core::arch::asm!("lidt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags));
    }
}
