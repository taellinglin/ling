//! AArch64 exception vector table: all 16 entries (4 exception classes ×
//! {current EL SP0, current EL SPx, lower EL AArch64, lower EL AArch32}),
//! installed at `VBAR_EL1`. This kernel runs entirely in EL1h (SP_EL1), so
//! only the "current EL, SPx" synchronous and IRQ entries are reachable
//! today — the rest (SP0, and every lower-EL entry, unreachable until
//! Phase 3 runs code at EL0) still get a real handler rather than being
//! left empty: an unpopulated vector slot on a real fault is a silent
//! reset, exactly what this phase exists to replace with a readable dump.
use core::arch::global_asm;
use core::mem::size_of;

#[repr(C)]
pub struct TrapFrame {
    pub gpr: [u64; 31], // x0..x30
    pub sp_el0: u64,
    pub elr: u64,
    pub spsr: u64,
}

const FRAME_SIZE: usize = size_of::<TrapFrame>();

global_asm!(
    r#"
// Each vector-table slot below is a hardware-fixed 128 bytes (32
// instructions) apart -- the CPU computes the entry address as
// `VBAR_EL1 + a fixed per-type offset`, not by reading labels, so
// anything landing in a slot beyond that budget corrupts every later
// entry silently (confirmed: an earlier version put the full ~50-
// instruction save/dispatch/restore sequence directly in each slot, and
// `.align 7` between entries pushed later slots to the wrong offsets --
// the CPU still jumped to the *architected* offset, landed mid-instruction
// in a neighboring entry, and produced a self-sustaining fault storm with
// `esr_el1=0` nonsense on every boot). Slots hold only a branch; the real
// work lives in ordinary (unconstrained-size) code below the table.
.macro VECTOR target
.align 7
    b \target
.endm

.macro SAVE_AND_DISPATCH kind, handler
    sub sp, sp, #{frame_size}
    stp x0, x1, [sp, #0]
    stp x2, x3, [sp, #16]
    stp x4, x5, [sp, #32]
    stp x6, x7, [sp, #48]
    stp x8, x9, [sp, #64]
    stp x10, x11, [sp, #80]
    stp x12, x13, [sp, #96]
    stp x14, x15, [sp, #112]
    stp x16, x17, [sp, #128]
    stp x18, x19, [sp, #144]
    stp x20, x21, [sp, #160]
    stp x22, x23, [sp, #176]
    stp x24, x25, [sp, #192]
    stp x26, x27, [sp, #208]
    stp x28, x29, [sp, #224]
    str x30, [sp, #240]
    mrs x9, sp_el0
    str x9, [sp, #248]
    mrs x9, elr_el1
    str x9, [sp, #256]
    mrs x9, spsr_el1
    str x9, [sp, #264]
    mov x0, sp
    mov x1, \kind
    bl \handler
    ldr x9, [sp, #256]
    msr elr_el1, x9
    ldr x9, [sp, #264]
    msr spsr_el1, x9
    ldr x9, [sp, #248]
    msr sp_el0, x9
    ldp x0, x1, [sp, #0]
    ldp x2, x3, [sp, #16]
    ldp x4, x5, [sp, #32]
    ldp x6, x7, [sp, #48]
    ldp x8, x9, [sp, #64]
    ldp x10, x11, [sp, #80]
    ldp x12, x13, [sp, #96]
    ldp x14, x15, [sp, #112]
    ldp x16, x17, [sp, #128]
    ldp x18, x19, [sp, #144]
    ldp x20, x21, [sp, #160]
    ldp x22, x23, [sp, #176]
    ldp x24, x25, [sp, #192]
    ldp x26, x27, [sp, #208]
    ldp x28, x29, [sp, #224]
    ldr x30, [sp, #240]
    add sp, sp, #{frame_size}
    eret
.endm

.section .text
.align 11
.global vector_table_el1
vector_table_el1:
    VECTOR vec_unreachable   // sync, current EL, SP0 (unreachable)
    VECTOR vec_unreachable   // IRQ, current EL, SP0 (unreachable)
    VECTOR vec_unreachable   // FIQ, current EL, SP0 (unreachable)
    VECTOR vec_unreachable   // SError, current EL, SP0 (unreachable)
    VECTOR vec_sync          // sync, current EL, SPx -- real
    VECTOR vec_irq           // IRQ, current EL, SPx -- real
    VECTOR vec_unreachable   // FIQ, current EL, SPx (never enabled)
    VECTOR vec_unreachable   // SError, current EL, SPx
    VECTOR vec_unreachable   // sync, lower EL, AArch64 (Phase 3)
    VECTOR vec_unreachable   // IRQ, lower EL, AArch64 (Phase 3)
    VECTOR vec_unreachable   // FIQ, lower EL, AArch64
    VECTOR vec_unreachable   // SError, lower EL, AArch64
    VECTOR vec_unreachable   // sync, lower EL, AArch32 (never used)
    VECTOR vec_unreachable   // IRQ, lower EL, AArch32
    VECTOR vec_unreachable   // FIQ, lower EL, AArch32
    VECTOR vec_unreachable   // SError, lower EL, AArch32

// Out-of-line: unconstrained size, unlike the slots above.
vec_sync:
    SAVE_AND_DISPATCH 0, aarch64_trap_handler
vec_irq:
    SAVE_AND_DISPATCH 1, aarch64_trap_handler
vec_unreachable:
    SAVE_AND_DISPATCH 2, aarch64_trap_handler
"#,
    frame_size = const FRAME_SIZE,
);

extern "C" {
    static vector_table_el1: u8;
}

pub fn init() {
    unsafe {
        let base = (&raw const vector_table_el1) as u64;
        core::arch::asm!("msr vbar_el1, {}", in(reg) base, options(nomem, nostack));
    }
}

fn print_hex64(label: &str, val: u64) {
    crate::console_write(label.as_bytes());
    crate::console_write(b"0x");
    let mut buf = [0u8; 16];
    for i in 0..16 {
        let nibble = (val >> ((15 - i) * 4)) & 0xF;
        buf[i] = if nibble < 10 { b'0' + nibble as u8 } else { b'a' + (nibble - 10) as u8 };
    }
    crate::console_write(&buf);
    crate::console_write(b"\n");
}

fn read_esr_el1() -> u64 {
    let val: u64;
    unsafe { core::arch::asm!("mrs {}, esr_el1", out(reg) val, options(nomem, nostack)) };
    val
}

fn read_far_el1() -> u64 {
    let val: u64;
    unsafe { core::arch::asm!("mrs {}, far_el1", out(reg) val, options(nomem, nostack)) };
    val
}

/// `HEARTBEAT_HZ` must match `lib.rs::init`'s `timer::start_periodic` call —
/// the IRQ handler re-arms the one-shot physical timer compare at this same
/// rate every time it fires (see `timer.rs`'s doc: unlike x86's PIT mode 3,
/// `CNTP_*_EL0` has no free-running periodic mode of its own).
const HEARTBEAT_HZ: u32 = 100;

static mut TICKS: u64 = 0;

pub fn ticks() -> u64 {
    unsafe { TICKS }
}

/// Software breakpoint (`brk #imm`) exception class in `ESR_EL1[31:26]` —
/// the aarch64 counterpart of x86's `int3`. Unlike x86, `ELR_EL1` for a
/// `brk` trap points *at* the trapping instruction, not past it, so the
/// handler must advance it by 4 (one AArch64 instruction) itself before
/// returning, or it re-traps the same `brk` forever.
const ESR_EC_BRK: u64 = 0x3C;

#[no_mangle]
extern "C" fn aarch64_trap_handler(frame: &mut TrapFrame, kind: u64) {
    match kind {
        1 => {
            if crate::arch::intc::core_timer_pending() {
                unsafe { TICKS += 1 };
                crate::arch::timer::rearm_periodic(HEARTBEAT_HZ);
            }
            if crate::arch::intc::uart_pending() {
                crate::drivers::uart::irq_drain();
            }
        }
        0 => {
            let esr = read_esr_el1();
            let ec = (esr >> 26) & 0x3F;
            if ec == ESR_EC_BRK {
                crate::console_write(b"\n!!! Breakpoint (brk)\n");
                print_hex64("  elr=", frame.elr);
                frame.elr += 4;
                return;
            }
            fault_halt("Synchronous Exception", frame, esr, read_far_el1());
        }
        _ => fault_halt("Unhandled Exception", frame, read_esr_el1(), read_far_el1()),
    }
}

fn fault_halt(name: &str, frame: &TrapFrame, esr: u64, far: u64) -> ! {
    crate::console_write(b"\n!!! CPU EXCEPTION: ");
    crate::console_write(name.as_bytes());
    crate::console_write(b"\n");
    print_hex64("  esr_el1=", esr);
    print_hex64("  far_el1=", far);
    print_hex64("  elr_el1=", frame.elr);
    print_hex64("  spsr_el1=", frame.spsr);
    print_hex64("  sp_el0=", frame.sp_el0);
    print_hex64("  x30(lr)=", frame.gpr[30]);
    crate::console_write(b"!!! kernel halted\n");
    loop {
        unsafe { super::cpu::wfi() };
    }
}
