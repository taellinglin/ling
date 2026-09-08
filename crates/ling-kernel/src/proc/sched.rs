//! Cooperative (not preemptive) multitasking: real IDT/PIC/timer interrupts
//! exist now (`arch::x86_64::{idt,pic,timer}`), but the 100Hz heartbeat isn't
//! wired to preemption — there is still no way to interrupt a running task,
//! every task must voluntarily call [`yield_now`] for anything else to make
//! progress. All tasks share the one flat identity-mapped address space and
//! run in ring 0 (no TSS/ring-3 anywhere in this kernel); this gives real
//! concurrent progress (multiple independent stacks, a scheduler, actual
//! interleaving) without needing per-process paging/privilege separation
//! this kernel doesn't have yet.
//!
//! Slot 0 is always the caller of [`init_main_task`] (the shell) — it's
//! folded into the same fixed-size task table rather than treated as a
//! special case, so `yield_now`/`exit` work identically for it and for any
//! spawned task.
use core::arch::global_asm;

pub const MAX_TASKS: usize = 4;
/// 2MiB per spawned task. Post-quantum crypto on RustCrypto's `ml-dsa` is a
/// heavy stack user: Messenger's `messenger_task` (via `ling-messenger`) runs
/// ML-DSA-87 keygen at first beacon and then, on a real chat handshake, an
/// ML-DSA-87 *sign* (responder) and *verify* (both sides) plus ML-KEM-768 —
/// each expands a 56-polynomial matrix (~57KiB) and layers vectors,
/// rejection-loop temporaries, and by-value key/signature structs on top.
/// The original 64KiB overflowed even on keygen; 512KiB survived keygen but
/// still overflowed on the sign/verify handshake (a corrupted return address
/// → `ret` into a non-canonical rip → #GP). These stacks are a plain BSS array
/// with NO guard page, so an overflow silently smashes whatever's adjacent
/// (the slab free-list, or a neighbouring task's saved return address). 2MiB
/// gives ample headroom; × 4 tasks = 8MiB of BSS, zero-filled at boot and not
/// stored on disk — nothing against real RAM. [`STACK_CANARY`] still guards
/// the bottom so a *downward* overrun that survives to a yield is reported
/// cleanly rather than silently corrupting memory.
const STACK_SIZE: usize = 2 * 1024 * 1024;

/// Sentinel written at the lowest 8 bytes of every spawned stack (the end a
/// downward-growing stack hits first on overflow). Checked on every context
/// switch; if it's been clobbered, the task overran its stack and we halt
/// with a named error rather than letting the corruption propagate into some
/// unrelated allocation and fault there.
const STACK_CANARY: u64 = 0x5354_4143_4B_5F4744; // "STAC K_GD"

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
/// Lowest address (canary location) of each spawned task's stack, or 0 for
/// slots that never got a fabricated stack (slot 0, the boot/main task, keeps
/// running on the bootloader's stack — see `init_main_task`).
static mut STACK_GUARD: [u64; MAX_TASKS] = [0; MAX_TASKS];
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
                // Plant the overflow canary at the lowest 8 bytes — the first
                // thing a downward-growing stack overwrites if it overruns.
                *(stack_base as *mut u64) = STACK_CANARY;
                STACK_GUARD[i] = stack_base as u64;
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
/// Verify no spawned task has overrun its stack into the canary. Called on
/// every context switch: cheap (at most `MAX_TASKS` word compares) and it
/// converts a silent BSS-smashing overflow into an immediate, named halt.
fn check_stack_guards() {
    unsafe {
        for i in 0..MAX_TASKS {
            let g = STACK_GUARD[i];
            if g != 0 && *(g as *const u64) != STACK_CANARY {
                panic!("stack overflow: task slot {} overran its {}KiB stack", i, STACK_SIZE / 1024);
            }
        }
    }
}

pub fn yield_now() {
    unsafe {
        if !STARTED {
            return;
        }
        check_stack_guards();
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
        unsafe { crate::arch::cpu::halt() };
    }
}

pub fn getpid() -> u64 {
    unsafe { CURRENT as u64 }
}

/// Per-slot state, exposed outside this module for `ps`-style reporting.
/// A plain copy of the private `TaskState` rather than reusing it directly,
/// so callers don't get a handle onto anything they could mutate.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum TaskStateInfo {
    Unused,
    Ready,
    Running,
    Exited,
}

pub fn task_state(i: usize) -> Option<TaskStateInfo> {
    if i >= MAX_TASKS {
        return None;
    }
    unsafe {
        Some(match TASKS[i].state {
            TaskState::Unused => TaskStateInfo::Unused,
            TaskState::Ready => TaskStateInfo::Ready,
            TaskState::Running => TaskStateInfo::Running,
            TaskState::Exited => TaskStateInfo::Exited,
        })
    }
}

static mut SELFTEST_MARK: bool = false;

extern "C" fn selftest_task() {
    unsafe { SELFTEST_MARK = true };
    yield_now();
}

/// A real, honest self-test: spawn a task, yield to it (it sets a marker
/// then yields back), confirm the marker got set. Proves task creation +
/// context switch + voluntary yield-back genuinely work. Does **not** prove
/// or claim anything about preemption or isolation — this scheduler has
/// neither (see this module's doc comment).
pub fn selftest() -> bool {
    unsafe { SELFTEST_MARK = false };
    if spawn(selftest_task as *const () as usize as u64) == u64::MAX {
        return false;
    }
    yield_now(); // switches to the task: sets the marker, yields back here
    let result = unsafe { SELFTEST_MARK };
    // One more yield lets the task resume past its own `yield_now()` and
    // fall through to `exit()`, reaping its slot -- without this it's left
    // permanently `Ready`, never scheduled again on its own, which would
    // leak one of the 4 fixed slots per selftest run.
    yield_now();
    result
}
