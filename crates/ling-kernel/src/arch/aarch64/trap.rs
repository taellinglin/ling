//! aarch64 counterpart of the x86_64 backend's `trap.rs`: the portable
//! surface [`crate::proc::uproc`] and [`crate::abi::syscalls`] call without
//! knowing which architecture they're on — [`TrapFrame`] (re-exported from
//! [`super::vectors`], which owns the struct because the exception-vector
//! asm there has to know its exact layout), [`launch_first_process`] /
//! [`resume_to_kernel`] (the same "park the calling kernel context, `eret`
//! into a fabricated frame; later, resume right after that call" pair as
//! the x86_64 backend's `launch_process_frame`/`resume_kernel_context`),
//! and [`init_user_frame`] (this architecture's half of fabricating a fresh
//! process's very first frame — the other half, x86_64's, lives in its own
//! `trap.rs`).
//!
//! One thing the x86_64 backend needs that this one doesn't:
//! `set_syscall_kernel_stack`. There, `TSS.rsp0`/`LSTAR`'s target stack are
//! *stored* values the hardware consults lazily, decoupled from whatever
//! `rsp` the kernel happens to be running on right now — switching
//! processes means updating that stored value ahead of time. Here, `SP_EL1`
//! *is* the kernel's currently active stack pointer while at EL1h; there is
//! no separate stored copy to update. Whichever process's frame the
//! exception-vector epilogue resumes (via `mov sp, x0` — see `vectors.rs`)
//! *is* the next process's kernel stack, so [`switch_kernel_stack`] has
//! nothing to do.
pub use super::vectors::{TrapFrame, TRAPFRAME_SIZE};
use core::arch::global_asm;

pub fn ticks() -> u64 {
    super::vectors::ticks()
}

/// No-op on aarch64 — see the module doc.
pub fn switch_kernel_stack(_stack_top: u64) {}

/// Fabricate a fresh process's first `TrapFrame`: `elr_el1` = entry point,
/// `sp_el0` = its user stack, `spsr_el1` = 0 (`EL0t`, every DAIF mask
/// clear — the aarch64 counterpart of the x86_64 backend's `rflags = 0x202`
/// setting `IF`). Every GPR starts zeroed.
pub unsafe fn init_user_frame(frame: *mut TrapFrame, entry: u64, user_stack_top: u64) {
    core::ptr::write_bytes(frame as *mut u8, 0, TRAPFRAME_SIZE as usize);
    (*frame).elr = entry;
    (*frame).sp_el0 = user_stack_top;
    (*frame).spsr = 0;
}

pub fn enable_interrupts_in_syscall() {
    unsafe { super::cpu::sti() };
}

global_asm!(
    r#"
.section .text
.global launch_process_frame
launch_process_frame:
    stp x19, x20, [sp, #-16]!
    stp x21, x22, [sp, #-16]!
    stp x23, x24, [sp, #-16]!
    stp x25, x26, [sp, #-16]!
    stp x27, x28, [sp, #-16]!
    stp x29, x30, [sp, #-16]!
    ldr x9, =kernel_resume_sp
    mov x10, sp
    str x10, [x9]
    mov sp, x0
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

.global resume_kernel_context
resume_kernel_context:
    ldr x9, =kernel_resume_sp
    ldr x9, [x9]
    mov sp, x9
    ldp x29, x30, [sp], #16
    ldp x27, x28, [sp], #16
    ldp x25, x26, [sp], #16
    ldp x23, x24, [sp], #16
    ldp x21, x22, [sp], #16
    ldp x19, x20, [sp], #16
    ret

.section .bss
.align 8
kernel_resume_sp:
    .skip 8
"#,
    frame_size = const TRAPFRAME_SIZE,
);

extern "C" {
    fn launch_process_frame(frame_addr: u64);
    fn resume_kernel_context();
}

/// `eret` into `frame_addr` for the very first time. Blocks the calling
/// kernel context until every process this launches (and anything it
/// `spawn`s) has exited and control naturally falls back to
/// [`resume_to_kernel`].
pub fn launch_first_process(frame_addr: u64) {
    unsafe { launch_process_frame(frame_addr) };
}

/// Called when there is no runnable process left: hands control back to
/// whichever kernel context called [`launch_first_process`]. Genuinely
/// diverges from *this* call site.
pub fn resume_to_kernel() -> ! {
    unsafe { resume_kernel_context() };
    unreachable!()
}
