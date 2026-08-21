//! The one register-frame shape every ring3→ring0 transition produces:
//! timer preemption (vector 32, via [`gdt::TIMER_IST`] so the hardware
//! frame is always 5 qwords — see that constant's doc) and `syscall` entry
//! (which pushes nothing automatically, so the entry stub below fabricates
//! the same 5-qword shape by hand before falling into the identical GPR
//! push sequence). Unifying the two means one epilogue (`iretq`) and one
//! Rust-side scheduling decision serve both a syscall return and a
//! timer-preempted resume — a process's saved context is always "whatever
//! sits at the top of its own kernel stack", regardless of which of the two
//! ways it last stopped running.
//!
//! Exit is always `iretq`, never `sysretq`, even for the syscall path this
//! is deliberate: `sysretq` needs its own restore of RCX/R11/RSP outside
//! this frame shape, which would mean a second epilogue and a second place
//! for the two to drift out of sync. `iretq` is measurably slower; that's a
//! real, accepted cost of shipping one correct path first (Rule 22 —
//! architecture before optimization) rather than two paths that are each
//! individually faster but riskier to keep both correct.
use super::{cpu, gdt, pic};
use core::arch::{asm, global_asm};

#[repr(C)]
pub struct TrapFrame {
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

impl TrapFrame {
    /// `true` if this frame belongs to code that was running in ring 3 —
    /// the only case [`timer_trap_rust`] hands off to the preemptive
    /// scheduler; a kernel-mode tick (the cooperative `.ling`-task
    /// scheduler, the shell, any interrupt/syscall handler) just gets
    /// ticked and returns to itself unchanged.
    pub fn from_userspace(&self) -> bool {
        self.cs & 3 == 3
    }

    pub fn syscall_num(&self) -> u64 {
        self.rax
    }

    /// Syscall argument `i` (0-indexed), in the `syscall` instruction's own
    /// register order: `rdi, rsi, rdx, r10, r8, r9` — `r10` stands in for
    /// `rcx`, which `syscall` itself clobbers.
    pub fn arg(&self, i: usize) -> u64 {
        match i {
            0 => self.rdi,
            1 => self.rsi,
            2 => self.rdx,
            3 => self.r10,
            4 => self.r8,
            5 => self.r9,
            _ => 0,
        }
    }

    pub fn set_return(&mut self, value: u64) {
        self.rax = value;
    }

    pub fn return_value(&self) -> u64 {
        self.rax
    }
}

static mut TICKS: u64 = 0;

pub fn ticks() -> u64 {
    unsafe { TICKS }
}

global_asm!(
    r#"
.section .text
.global timer_trap_entry
timer_trap_entry:
    push r15
    push r14
    push r13
    push r12
    push r11
    push r10
    push r9
    push r8
    push rbp
    push rdi
    push rsi
    push rdx
    push rcx
    push rbx
    push rax
    mov rdi, rsp
    call timer_trap_rust
    mov rsp, rax
    pop rax
    pop rbx
    pop rcx
    pop rdx
    pop rsi
    pop rdi
    pop rbp
    pop r8
    pop r9
    pop r10
    pop r11
    pop r12
    pop r13
    pop r14
    pop r15
    iretq

.global syscall_entry
syscall_entry:
    mov [syscall_user_rsp_scratch], rsp
    mov rsp, [syscall_kernel_rsp]
    push {user_ss}
    push [syscall_user_rsp_scratch]
    push r11
    push {user_cs}
    push rcx
    push r15
    push r14
    push r13
    push r12
    push r11
    push r10
    push r9
    push r8
    push rbp
    push rdi
    push rsi
    push rdx
    push rcx
    push rbx
    push rax
    mov rdi, rsp
    call syscall_trap_rust
    mov rsp, rax
    pop rax
    pop rbx
    pop rcx
    pop rdx
    pop rsi
    pop rdi
    pop rbp
    pop r8
    pop r9
    pop r10
    pop r11
    pop r12
    pop r13
    pop r14
    pop r15
    iretq

.global launch_process_frame
launch_process_frame:
    push rbx
    push rbp
    push r12
    push r13
    push r14
    push r15
    mov [kernel_resume_sp], rsp
    mov rsp, rdi
    pop rax
    pop rbx
    pop rcx
    pop rdx
    pop rsi
    pop rdi
    pop rbp
    pop r8
    pop r9
    pop r10
    pop r11
    pop r12
    pop r13
    pop r14
    pop r15
    iretq

.global resume_kernel_context
resume_kernel_context:
    mov rsp, [kernel_resume_sp]
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbp
    pop rbx
    ret

.section .bss
.align 8
.global syscall_user_rsp_scratch
syscall_user_rsp_scratch:
    .skip 8
.global syscall_kernel_rsp
syscall_kernel_rsp:
    .skip 8
.global kernel_resume_sp
kernel_resume_sp:
    .skip 8
"#,
    user_ss = const super::gdt::USER_DATA_SEL as u64,
    user_cs = const super::gdt::USER_CODE_SEL as u64,
);

extern "C" {
    pub fn timer_trap_entry();
    pub fn syscall_entry();
    /// Kernel `rsp` the next `syscall` instruction switches onto — set by
    /// the scheduler on every switch to a different process (mirrors
    /// `gdt::set_kernel_stack`, which covers the same need for IDT-gate
    /// entries; `syscall` doesn't consult the TSS at all, so it needs its
    /// own copy of "where is this process's kernel stack").
    static mut syscall_kernel_rsp: u64;

    /// Save the calling kernel context's callee-saved registers (the same
    /// convention `proc::sched::switch_to` already uses) and jump into ring
    /// 3 at the `TrapFrame` `frame_addr` points to. Declared as an
    /// ordinary (non-diverging) call even though control doesn't return
    /// here the normal way: the `call` instruction still pushes a return
    /// address, and [`resume_kernel_context`] later `ret`s straight to it —
    /// exactly the same "returns, just not from where you'd expect" shape
    /// `proc::sched::switch_to` already relies on. Marking this `-> !`
    /// would tell LLVM the code after the call is unreachable, which is
    /// false: it runs once, later, from a different logical caller.
    fn launch_process_frame(frame_addr: u64);

    /// The other half of [`launch_process_frame`]: restore the kernel
    /// context it saved and resume right after that call. Declared as an
    /// ordinary, non-diverging call for the same reason
    /// `launch_process_frame` is (a real `call`/return-address pair is
    /// still emitted); [`resume_to_kernel`] below is the safe wrapper that
    /// tells Rust the code after *its* call is genuinely unreachable, which
    /// is true from that specific call site.
    fn resume_kernel_context();
}

pub const TRAPFRAME_SIZE: u64 = core::mem::size_of::<TrapFrame>() as u64;

/// Point both of the places x86_64 hardware lazily consults a "this
/// process's kernel stack" value at — `TSS.rsp0` (IDT-gate ring3->ring0
/// transitions: faults) and `LSTAR`'s `syscall` fast path (its own separate
/// stored stack, `gdt::set_kernel_stack` doesn't cover it) — at
/// `stack_top`, on every switch to a different process. The portable
/// counterpart of the aarch64 backend's `trap::switch_kernel_stack`, which
/// is a no-op there — see that module's doc for why the two architectures
/// differ here.
pub fn switch_kernel_stack(stack_top: u64) {
    gdt::set_kernel_stack(stack_top);
    unsafe { syscall_kernel_rsp = stack_top };
}

/// Fabricate a fresh process's first `TrapFrame`: ring 3, entry point,
/// `IF` set, its own stack.
pub unsafe fn init_user_frame(frame: *mut TrapFrame, entry: u64, user_stack_top: u64) {
    core::ptr::write_bytes(frame as *mut u8, 0, TRAPFRAME_SIZE as usize);
    (*frame).rip = entry;
    (*frame).cs = gdt::USER_CODE_SEL as u64;
    (*frame).rflags = 0x202;
    (*frame).rsp = user_stack_top;
    (*frame).ss = gdt::USER_DATA_SEL as u64;
}

/// Enter ring 3 for the very first time, running whatever `TrapFrame` sits
/// at `frame_addr` (fabricated by `proc::elf::load`'s caller for a fresh
/// process). Blocks the calling kernel context until every process this
/// launches (and anything it `spawn`s) has exited and control naturally
/// falls back to [`resume_to_kernel`].
pub fn launch_first_process(frame_addr: u64) {
    unsafe { launch_process_frame(frame_addr) };
}

/// Called from inside `timer_trap_rust`/`syscall_trap_rust` when there is
/// no runnable process left: hands control back to whichever kernel
/// context called [`launch_first_process`], resuming right after that
/// call. Genuinely diverges from *this* call site — the underlying asm
/// overwrites `rsp` before it `ret`s, so nothing after the call below ever
/// runs.
pub fn resume_to_kernel() -> ! {
    unsafe { resume_kernel_context() };
    unreachable!()
}

#[no_mangle]
extern "C" fn timer_trap_rust(frame: *mut TrapFrame) -> u64 {
    unsafe { TICKS += 1 };
    pic::eoi(0);
    let frame_ref = unsafe { &mut *frame };
    if frame_ref.from_userspace() {
        crate::proc::uproc::on_timer_preempt(frame as u64)
    } else {
        frame as u64
    }
}

#[no_mangle]
extern "C" fn syscall_trap_rust(frame: *mut TrapFrame) -> u64 {
    crate::abi::syscalls::dispatch(unsafe { &mut *frame });
    crate::proc::uproc::on_syscall_return(frame as u64)
}

const EFER_MSR: u32 = 0xC000_0080;
const EFER_SCE: u64 = 1 << 0;
const STAR_MSR: u32 = 0xC000_0081;
const LSTAR_MSR: u32 = 0xC000_0082;
const SFMASK_MSR: u32 = 0xC000_0084;
/// Cleared out of `rflags` on `syscall` entry: `IF` (bit 9), so an async
/// interrupt can't land mid-transition before the entry stub has finished
/// switching onto the kernel stack. `sti` right after is what actually
/// re-enables interrupts for the body of a syscall handler that might
/// block.
const RFLAGS_IF: u64 = 1 << 9;

/// Enable `syscall`/`sysret` and point `LSTAR` at [`syscall_entry`]. Must
/// run after `gdt::init` (the kernel/user selector layout `STAR` encodes is
/// fixed there) and before any ring-3 code exists to execute `syscall`.
pub fn init() {
    unsafe {
        let efer = cpu::read_msr(EFER_MSR);
        cpu::write_msr(EFER_MSR, efer | EFER_SCE);

        // STAR[47:32]: kernel CS on syscall entry, kernel SS = that + 8
        // (`gdt::KERNEL_DATA_SEL` is indeed `KERNEL_CODE_SEL + 8` — see
        // `gdt.rs`'s entry layout). STAR[63:48] (the `sysret` half) is left
        // zero: this kernel never executes `sysret` (see the module doc),
        // so it has no reader — left at its architectural default rather
        // than filled in for a path that doesn't exist, which would be the
        // kind of dead configuration Rule 5 says not to leave behind.
        let star = (gdt::KERNEL_CODE_SEL as u64) << 32;
        cpu::write_msr(STAR_MSR, star);
        cpu::write_msr(LSTAR_MSR, syscall_entry as *const () as u64);
        cpu::write_msr(SFMASK_MSR, RFLAGS_IF);
    }
}

/// Re-enable interrupts once a syscall handler is safely on its own kernel
/// stack — called by [`crate::abi::syscalls::dispatch`] before doing
/// anything that might run long enough to need preemption (a blocking
/// wait), never before.
pub fn enable_interrupts_in_syscall() {
    unsafe { asm!("sti", options(nomem, nostack)) };
}
