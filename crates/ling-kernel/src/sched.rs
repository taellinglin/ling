//! Cooperative (not preemptive) multitasking: no IDT/timer/ring-3 exist in
//! this kernel yet (see the module doc in `boot32.rs` for the one-time
//! long-mode bring-up this deliberately never touches), so there is no way
//! to interrupt a running task — every task must voluntarily call
//! [`yield_now`] for anything else to make progress. All tasks share the one
//! flat identity-mapped address space and run in ring 0, same as everything
//! else in this kernel; this gives real concurrent progress (multiple
//! independent stacks, a scheduler, actual interleaving) without needing an
//! interrupt/privilege/paging subsystem this kernel doesn't have.
//!
//! Slot 0 is always the caller of [`init_main_task`] (the shell) — it's
//! folded into the same fixed-size task table rather than treated as a
//! special case, so `yield_now`/`exit` work identically for it and for any
//! spawned task.
use core::arch::global_asm;

pub const MAX_TASKS: usize = 4;
const STACK_SIZE: usize = 65536;

#[derive(Copy, Clone, PartialEq, Eq)]
enum TaskState {
    Unused,
    Ready,
    Running,
    Exited,
}

#[derive(Copy, Clone)]
struct Tcb {
    state: TaskState,
    /// Saved `rsp`, valid while this task isn't the one running.
    sp: u64,
}

const EMPTY_TCB: Tcb = Tcb { state: TaskState::Unused, sp: 0 };

static mut TASKS: [Tcb; MAX_TASKS] = [EMPTY_TCB; MAX_TASKS];
static mut STACKS: [[u8; STACK_SIZE]; MAX_TASKS] = [[0; STACK_SIZE]; MAX_TASKS];
/// Entry point for a not-yet-first-run task, read by `trampoline` (see
/// below) once the scheduler switches into it for the first time — there's
/// no register-argument convention across a fabricated initial stack, so
/// this is the handoff instead, indexed by slot like everything else here.
static mut TASK_ENTRY: [u64; MAX_TASKS] = [0; MAX_TASKS];
static mut CURRENT: usize = 0;
static mut STARTED: bool = false;

extern "C" {
    /// Save the caller's callee-saved registers + `rsp` to `*save_sp`, then
    /// load `rsp` from `new_sp` and restore *its* callee-saved registers
    /// before returning — control resumes wherever `new_sp`'s owner last
    /// called `switch_to` from (or `trampoline`, for a task's first run; see
    /// `spawn`'s fabricated stack). Plain SysV AMD64 (`rdi`, `rsi`), the
    /// same convention every `ling_kernel_*` intrinsic already uses.
    fn switch_to(save_sp: *mut u64, new_sp: u64);
}

global_asm!(
    r#"
.section .text
.global switch_to
switch_to:
    push rbx
    push rbp
    push r12
    push r13
    push r14
    push r15
    mov [rdi], rsp
    mov rsp, rsi
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbp
    pop rbx
    ret
"#
);

/// Lands here (via `ret`, out of `switch_to`) the first time a spawned
/// task's slot is switched into — never called directly.
extern "C" fn trampoline() -> ! {
    let idx = unsafe { CURRENT };
    let entry = unsafe { TASK_ENTRY[idx] };
    if entry != 0 {
        let f: extern "C" fn() = unsafe { core::mem::transmute(entry) };
        f();
    }
    exit();
}

/// Establish the calling context (the shell, at boot) as slot 0's running
/// task, without switching anything — there's nothing to switch away from
/// yet. Idempotent; call once before any `spawn`/`yield_now`.
pub fn init_main_task() {
    unsafe {
        if STARTED {
            return;
        }
        TASKS[0].state = TaskState::Running;
        CURRENT = 0;
        STARTED = true;
    }
}

/// Spawn `entry` (an `extern "C" fn()`, passed as its address) into the
/// first free slot. Returns the slot index, or `u64::MAX` if every slot is
/// taken — there is no dynamic growth here, same fixed-capacity style as
/// this kernel's bump allocator/other static tables.
pub fn spawn(entry: u64) -> u64 {
    unsafe {
        for i in 0..MAX_TASKS {
            if i != CURRENT
                && (TASKS[i].state == TaskState::Unused || TASKS[i].state == TaskState::Exited)
            {
                TASK_ENTRY[i] = entry;
                let stack_base = (&raw mut STACKS[i]) as *mut u8;
                let top = (stack_base as u64 + STACK_SIZE as u64) & !0xF;

                // Fabricate the frame `switch_to`'s pop sequence expects:
                // r15,r14,r13,r12,rbp,rbx (unused, zeroed) then a return
                // address landing in `trampoline`.
                let mut sp = top - 8;
                *(sp as *mut u64) = trampoline as *const () as usize as u64;
                for _ in 0..6 {
                    sp -= 8;
                    *(sp as *mut u64) = 0;
                }

                TASKS[i].sp = sp;
                TASKS[i].state = TaskState::Ready;
                return i as u64;
            }
        }
        u64::MAX
    }
}

/// Round-robin to the next `Ready` task, if any; returns immediately (no-op)
/// if nothing else is runnable. Safe to call from any task, including one
/// that's about to fall through into `exit`.
pub fn yield_now() {
    unsafe {
        if !STARTED {
            return;
        }
        let prev = CURRENT;
        let mut next = (prev + 1) % MAX_TASKS;
        while next != prev && TASKS[next].state != TaskState::Ready {
            next = (next + 1) % MAX_TASKS;
        }
        if next == prev || TASKS[next].state != TaskState::Ready {
            return;
        }

        if TASKS[prev].state == TaskState::Running {
            TASKS[prev].state = TaskState::Ready;
        }
        TASKS[next].state = TaskState::Running;
        CURRENT = next;

        let save_ptr = &raw mut TASKS[prev].sp;
        let new_sp = TASKS[next].sp;
        switch_to(save_ptr, new_sp);
    }
}

/// Mark the running task `Exited` and hand off to whatever's `Ready` next.
/// Never returns: nothing will ever switch back into an `Exited` slot's
/// stack. If it's the only task, there's nothing to switch to either — halt
/// rather than spin.
pub fn exit() -> ! {
    unsafe {
        TASKS[CURRENT].state = TaskState::Exited;
    }
    loop {
        yield_now();
        unsafe { crate::cpu::halt() };
    }
}

pub fn getpid() -> u64 {
    unsafe { CURRENT as u64 }
}

// ─── Milestone 0 demo tasks ─────────────────────────────────────────────
// Temporary: prove the scheduler actually interleaves two independent
// stacks (observed via the serial log) before anything real depends on it.
// Remove once Phase 1's shell-driven demo command replaces these.

pub extern "C" fn demo_task_a() {
    for _ in 0..5 {
        crate::serial::write(b"A");
        yield_now();
    }
}

pub extern "C" fn demo_task_b() {
    for _ in 0..5 {
        crate::serial::write(b"B");
        yield_now();
    }
}
