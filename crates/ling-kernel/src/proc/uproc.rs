//! Preemptive scheduler for ring-3 processes — entirely separate from
//! [`super::sched`], which stays exactly as it was: cooperative, ring-0
//! only, serving `.ling` kernel-target intrinsics. This one drives real
//! isolated processes, each with its own address space (`arch::paging`)
//! and kernel stack, switched by [`crate::arch::trap`]'s timer/syscall trap
//! entries rather than by any code the scheduled process itself calls.
//!
//! Every process's suspended state is exactly one `TrapFrame`, sitting at
//! a fixed offset from that process's own kernel stack top — true whether
//! it got there by being timer-preempted, by making a syscall, or (for a
//! process that has never run yet) by [`spawn`] fabricating one. One shape,
//! one resume mechanism (`iretq`), no separate "blocked mid-syscall" case:
//! a syscall that can't complete yet (`sleep_ms` before its deadline,
//! eventually a blocking `read`) marks the process not-ready and returns
//! through the ordinary path *without* having written a result into the
//! frame's `rax` yet — [`wake_blocked`] fills that in later, from whichever
//! context notices the wait is over, and the process simply resumes
//! ring-3 execution right after its `syscall` instruction once it's
//! rescheduled, exactly as if the syscall had just returned.
use crate::arch::paging;
use crate::arch::trap::{self, TrapFrame, TRAPFRAME_SIZE};

pub const MAX_PROCESSES: usize = 8;
const KERNEL_STACK_ORDER: usize = 3; // 8 frames * 4KiB = 32KiB
const USER_STACK_PAGES: u64 = 4; // 16KiB
const USER_CODE_BASE: u64 = 0x0000_0080_0000_0000; // PML4 index 1 — outside the kernel's [0,4GiB) identity range entirely
const USER_STACK_TOP: u64 = 0x0000_0080_1000_0000;
const PAGE_SIZE: u64 = 4096;

#[derive(Copy, Clone, PartialEq, Eq)]
enum State {
    Unused,
    Ready,
    Blocked(u64),
    Exited(i32),
}

#[derive(Copy, Clone)]
struct Process {
    state: State,
    pml4: u64,
    kernel_stack_top: u64,
}

const EMPTY: Process = Process { state: State::Unused, pml4: 0, kernel_stack_top: 0 };
static mut TABLE: [Process; MAX_PROCESSES] = [EMPTY; MAX_PROCESSES];
static mut CURRENT: usize = 0;

fn frame_addr_of(p: &Process) -> u64 {
    p.kernel_stack_top - TRAPFRAME_SIZE
}

pub fn current_pid() -> u64 {
    unsafe { CURRENT as u64 }
}

pub fn current_pml4() -> u64 {
    unsafe { TABLE[CURRENT].pml4 }
}

/// Load `elf` into a fresh address space and register it as `Ready`.
/// Doesn't run it — [`run_to_completion`] (or, once `spawn` the syscall
/// exists, the scheduler's own next tick) does that.
pub fn spawn(elf: &[u8]) -> Result<usize, &'static str> {
    let mut slot = None;
    for i in 0..MAX_PROCESSES {
        if unsafe { TABLE[i].state } == State::Unused {
            slot = Some(i);
            break;
        }
    }
    let slot = slot.ok_or("process table full")?;

    let pml4 = paging::new_address_space();
    let entry = crate::proc::elf::load(pml4, elf)?;
    if entry < USER_CODE_BASE {
        return Err("entry point outside user address range");
    }

    let kstack_phys =
        crate::mm::frame::alloc_frames(KERNEL_STACK_ORDER).ok_or("out of memory (kernel stack)")?;
    let kernel_stack_top = kstack_phys + ((PAGE_SIZE as usize) << KERNEL_STACK_ORDER) as u64;

    for i in 0..USER_STACK_PAGES {
        let phys = crate::mm::frame::alloc_frames(0).ok_or("out of memory (user stack)")?;
        unsafe { core::ptr::write_bytes(phys as *mut u8, 0, PAGE_SIZE as usize) };
        paging::map4k(
            pml4,
            USER_STACK_TOP - (i + 1) * PAGE_SIZE,
            phys,
            paging::USER | paging::WRITABLE | paging::NX,
        );
    }

    let frame = (kernel_stack_top - TRAPFRAME_SIZE) as *mut TrapFrame;
    unsafe {
        core::ptr::write_bytes(frame as *mut u8, 0, TRAPFRAME_SIZE as usize);
        (*frame).rip = entry;
        (*frame).cs = crate::arch::gdt::USER_CODE_SEL as u64;
        (*frame).rflags = 0x202; // IF set
        (*frame).rsp = USER_STACK_TOP;
        (*frame).ss = crate::arch::gdt::USER_DATA_SEL as u64;
    }

    unsafe {
        TABLE[slot] = Process { state: State::Ready, pml4, kernel_stack_top };
    }
    Ok(slot)
}

/// Run `pid` (and, transitively, whatever it and anything it later spawns
/// contend for the CPU with) until every process is `Exited`, then return
/// `pid`'s own exit code. Blocks the calling kernel context — this is the
/// synchronous entry point `ling_kernel_proc_selftest` uses; a real
/// `spawn`/`waitpid`-driven multi-process shell doesn't need this at all,
/// since those syscalls drive scheduling from inside already-running
/// processes instead.
pub fn run_to_completion(pid: usize) -> i32 {
    unsafe {
        switch_to(pid);
        trap::launch_first_process(frame_addr_of(&TABLE[pid]));
        match TABLE[pid].state {
            State::Exited(code) => code,
            _ => -1,
        }
    }
}

unsafe fn switch_to(pid: usize) {
    CURRENT = pid;
    crate::arch::gdt::set_kernel_stack(TABLE[pid].kernel_stack_top);
    trap::set_syscall_kernel_stack(TABLE[pid].kernel_stack_top);
    paging::switch_to(TABLE[pid].pml4);
}

fn pick_next(cur: usize) -> Option<usize> {
    for step in 1..=MAX_PROCESSES {
        let i = (cur + step) % MAX_PROCESSES;
        if unsafe { TABLE[i].state } == State::Ready {
            return Some(i);
        }
    }
    None
}

fn wake_blocked() {
    let now = trap::ticks();
    for i in 0..MAX_PROCESSES {
        unsafe {
            if let State::Blocked(wake_at) = TABLE[i].state {
                if now >= wake_at {
                    // The frame's `rax` still holds the syscall number
                    // (whatever was in `rax` at entry) since the syscall
                    // that blocked never set a return value — set it now,
                    // so resuming ring 3 sees a real `sleep_ms` return (0),
                    // not its own syscall number echoed back.
                    let frame = frame_addr_of(&TABLE[i]) as *mut TrapFrame;
                    (*frame).rax = 0;
                    TABLE[i].state = State::Ready;
                }
            }
        }
    }
}

/// Called from `timer_trap_rust` when the interrupted context was ring 3.
/// `ist_frame_addr` points at the interrupted process's frame on the
/// timer's shared IST stack — valid only until the *next* timer
/// interrupt, so it must be copied onto that process's own durable kernel
/// stack before anything else can run.
pub fn on_timer_preempt(ist_frame_addr: u64) -> u64 {
    wake_blocked();
    let cur = unsafe { CURRENT };
    match pick_next(cur) {
        Some(next) if next == cur => ist_frame_addr,
        Some(next) => unsafe {
            // `cur` was actively running in ring 3 (the only way it could
            // have been the one interrupted), so its state is still
            // `Ready` — timeslicing it out doesn't change that, just where
            // its saved frame lives.
            let dst = frame_addr_of(&TABLE[cur]);
            core::ptr::copy_nonoverlapping(
                ist_frame_addr as *const u8,
                dst as *mut u8,
                TRAPFRAME_SIZE as usize,
            );
            switch_to(next);
            frame_addr_of(&TABLE[next])
        },
        None => trap::resume_to_kernel(),
    }
}

/// Called from `syscall_trap_rust` after `abi::syscalls::dispatch` has run.
/// `frame_addr` already sits on the current process's own kernel stack
/// (`syscall_entry` built it there directly), so no copy is needed here —
/// only the scheduling decision.
pub fn on_syscall_return(frame_addr: u64) -> u64 {
    wake_blocked();
    let cur = unsafe { CURRENT };
    match pick_next(cur) {
        Some(next) if next == cur => frame_addr,
        Some(next) => unsafe {
            switch_to(next);
            frame_addr_of(&TABLE[next])
        },
        None => trap::resume_to_kernel(),
    }
}

pub fn exit_current(code: i32) {
    unsafe { TABLE[CURRENT].state = State::Exited(code) };
}

pub fn block_current_until_tick(wake_at: u64) {
    unsafe { TABLE[CURRENT].state = State::Blocked(wake_at) };
}

pub fn current_frame_rax(value: u64) {
    unsafe {
        let frame = frame_addr_of(&TABLE[CURRENT]) as *mut TrapFrame;
        (*frame).rax = value;
    }
}
