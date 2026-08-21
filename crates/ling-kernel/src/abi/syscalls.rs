//! Syscall ABI v1 — numbers are frozen from here on (a process built
//! against number `N` must keep meaning the same thing in every later
//! kernel). Calling convention matches the hardware `syscall` instruction's
//! own register clobber list: number in `rax`, args in `rdi, rsi, rdx,
//! r10, r8, r9` (`r10` instead of `rcx` — `rcx`/`r11` are clobbered by
//! `syscall` itself), return value in `rax`.
//!
//! Every pointer argument is validated against the calling process's own
//! address space (`paging::user_range_ok`) before use — a bad pointer
//! fails the syscall (`-EFAULT`), it never gets dereferenced by the
//! kernel.
//!
//! Not every number below has a working implementation yet: `spawn`,
//! `waitpid`, `open`, `close`, `lseek`, `mmap`, `munmap`, `poll_input`,
//! `fb_map`, and `uname` return [`ENOSYS`] — disclosed here rather than
//! silently stubbed, since the ABI table is what Phase 4's userland
//! runtime links against and needs to be able to tell "not implemented"
//! from "failed."
use crate::arch::paging;
use crate::arch::trap::{self, TrapFrame};
use crate::proc::uproc;

pub const SYS_EXIT: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_READ: u64 = 2;
pub const SYS_OPEN: u64 = 3;
pub const SYS_CLOSE: u64 = 4;
pub const SYS_LSEEK: u64 = 5;
pub const SYS_MMAP: u64 = 6;
pub const SYS_MUNMAP: u64 = 7;
pub const SYS_SPAWN: u64 = 8;
pub const SYS_WAITPID: u64 = 9;
pub const SYS_YIELD: u64 = 10;
pub const SYS_GETPID: u64 = 11;
pub const SYS_SLEEP_MS: u64 = 12;
pub const SYS_POLL_INPUT: u64 = 13;
pub const SYS_FB_MAP: u64 = 14;
pub const SYS_UNAME: u64 = 15;

const ENOSYS: u64 = -38i64 as u64;
const EFAULT: u64 = -14i64 as u64;
const EBADF: u64 = -9i64 as u64;

/// Ticks (at the 100Hz heartbeat `timer::start_periodic` programs) per
/// millisecond of `sleep_ms`, rounded up so a sub-tick request still sleeps
/// at least one tick rather than returning immediately.
fn ms_to_ticks(ms: u64) -> u64 {
    ms.div_ceil(10)
}

pub fn dispatch(frame: &mut TrapFrame) {
    trap::enable_interrupts_in_syscall();
    let pml4 = uproc::current_pml4();
    match frame.rax {
        SYS_EXIT => uproc::exit_current(frame.rdi as i32),
        SYS_WRITE => frame.rax = sys_write(pml4, frame.rdi, frame.rsi, frame.rdx),
        SYS_YIELD => frame.rax = 0,
        SYS_GETPID => frame.rax = uproc::current_pid(),
        SYS_SLEEP_MS => {
            let wake_at = trap::ticks() + ms_to_ticks(frame.rdi);
            uproc::block_current_until_tick(wake_at);
            // `rax` deliberately left unset: `uproc::wake_blocked` writes
            // the real return value (0) once the sleep is actually over —
            // see that function's doc for why.
        }
        SYS_READ | SYS_OPEN | SYS_CLOSE | SYS_LSEEK | SYS_MMAP | SYS_MUNMAP | SYS_SPAWN
        | SYS_WAITPID | SYS_POLL_INPUT | SYS_FB_MAP | SYS_UNAME => frame.rax = ENOSYS,
        _ => frame.rax = ENOSYS,
    }
}

fn sys_write(pml4: u64, fd: u64, ptr: u64, len: u64) -> u64 {
    if fd != 1 && fd != 2 {
        return EBADF;
    }
    if len > 4096 {
        return EFAULT;
    }
    if !paging::user_range_ok(pml4, ptr, len, false) {
        return EFAULT;
    }
    let bytes = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    crate::console_write(bytes);
    len
}
