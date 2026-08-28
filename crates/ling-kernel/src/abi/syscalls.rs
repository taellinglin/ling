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
//! Not every number below has a working implementation yet: `read`, `open`,
//! `close`, `lseek`, `mmap`, `munmap`, `poll_input`, `fb_map`, and `uname`
//! return [`ENOSYS`] — disclosed here rather than silently stubbed, since
//! the ABI table is what Phase 4's userland runtime links against and needs
//! to be able to tell "not implemented" from "failed."
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

#[allow(dead_code)]
pub const ENOENT: u64 = -2i64 as u64;
#[allow(dead_code)]
pub const EIO: u64 = -5i64 as u64;
#[allow(dead_code)]
pub const EBADF: u64 = -9i64 as u64;
#[allow(dead_code)]
pub const EINVAL: u64 = -22i64 as u64;
#[allow(dead_code)]
pub const EFAULT: u64 = -14i64 as u64;
#[allow(dead_code)]
pub const ENOSYS: u64 = -38i64 as u64;

/// Ticks (at the 100Hz heartbeat `timer::start_periodic` programs) per
/// millisecond of `sleep_ms`, rounded up so a sub-tick request still sleeps
/// at least one tick rather than returning immediately.
fn ms_to_ticks(ms: u64) -> u64 {
    ms.div_ceil(10)
}

pub fn dispatch(frame: &mut TrapFrame) {
    trap::enable_interrupts_in_syscall();
    let pml4 = uproc::current_pml4();
    let num = frame.syscall_num();
    let arg0 = frame.arg(0);
    let arg1 = frame.arg(1);
    let arg2 = frame.arg(2);
    match num {
        SYS_EXIT => uproc::exit_current(arg0 as i32),
        SYS_WRITE => {
            let ret = sys_write(pml4, arg0, arg1, arg2);
            frame.set_return(ret);
        }
        SYS_YIELD => frame.set_return(0),
        SYS_GETPID => frame.set_return(uproc::current_pid()),
        SYS_SLEEP_MS => {
            let wake_at = trap::ticks() + ms_to_ticks(arg0);
            uproc::block_current_until_tick(wake_at);
            // Return value deliberately left unset: `uproc::wake_blocked` writes
            // the real return value (0) once the sleep is actually over.
        }
        SYS_SPAWN => {
            let ret = sys_spawn(pml4, arg0, arg1);
            frame.set_return(ret);
        }
        SYS_WAITPID => match uproc::waitpid_start(arg0 as usize) {
            uproc::WaitOutcome::Invalid => frame.set_return(EINVAL),
            uproc::WaitOutcome::Immediate(code) => frame.set_return(code as u64),
            // Return value deliberately left unset — `uproc::wake_blocked` fills
            // in the child's real exit code once it exits.
            uproc::WaitOutcome::Blocked => {}
        },
        SYS_FB_MAP => {
            let ret = sys_fb_map(pml4, arg0);
            frame.set_return(ret);
        }
        SYS_POLL_INPUT => frame.set_return(sys_poll_input()),
        SYS_READ | SYS_OPEN | SYS_CLOSE | SYS_LSEEK | SYS_MMAP | SYS_MUNMAP | SYS_UNAME => {
            frame.set_return(ENOSYS)
        }
        _ => frame.set_return(ENOSYS),
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

/// Load an ELF64 executable found at `path` in lingfs into a new process
/// and return its pid. Fails without touching any process state on a bad
/// pointer, a missing file, or a malformed ELF — `spawn` either fully
/// succeeds or has no effect, matching `elf::load`'s own all-or-nothing
/// contract.
fn sys_spawn(pml4: u64, path_ptr: u64, path_len: u64) -> u64 {
    if path_len == 0 || path_len as usize > 255 {
        return EINVAL;
    }
    if !paging::user_range_ok(pml4, path_ptr, path_len, false) {
        return EFAULT;
    }
    let mut path_buf = [0u8; 256];
    unsafe {
        core::ptr::copy_nonoverlapping(
            path_ptr as *const u8,
            path_buf.as_mut_ptr(),
            path_len as usize,
        )
    };
    let path = match core::str::from_utf8(&path_buf[..path_len as usize]) {
        Ok(s) => s,
        Err(_) => return EFAULT,
    };

    #[cfg(target_arch = "x86_64")]
    {
        let mut elf_buf = [0u8; crate::fs::lingfs::BLOCK_SIZE];
        match crate::fs::lingfs::read_file(path, &mut elf_buf) {
            Ok(Some(len)) => match uproc::spawn(&elf_buf[..len]) {
                Ok(pid) => pid as u64,
                Err(_) => EIO,
            },
            Ok(None) | Err(_) => ENOENT,
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = path;
        ENOSYS
    }
}

/// Where the framebuffer gets mapped in a process's address space — well
/// above the app's code (0x80_0000_0000) and stack (0x80_1000_0000), still in
/// the canonical lower (user) half.
const FB_USER_BASE: u64 = 0x0000_00A0_0000_0000;

/// Map the hardware framebuffer into the calling process's address space so a
/// native app can draw pixels directly (the fast path — no per-pixel syscall).
/// `info_ptr`, if non-null, receives `[width, height, pitch_bytes, bpp]` as
/// four u64s. Returns the user virtual base of the framebuffer, or an errno.
#[cfg(target_arch = "x86_64")]
fn sys_fb_map(pml4: u64, info_ptr: u64) -> u64 {
    let fb = match crate::drivers::framebuffer::get() {
        Some(f) => f,
        None => return ENOSYS,
    };
    let size = fb.pitch as u64 * fb.height as u64;
    let phys_base = fb.addr & !0xFFF;
    let off = fb.addr & 0xFFF;
    let pages = (off + size + 0xFFF) / 0x1000;
    let flags = paging::PRESENT | paging::WRITABLE | paging::USER | paging::NX;
    let mut i = 0;
    while i < pages {
        paging::map4k(pml4, FB_USER_BASE + i * 0x1000, phys_base + i * 0x1000, flags);
        i += 1;
    }
    if info_ptr != 0 {
        if !paging::user_range_ok(pml4, info_ptr, 32, true) {
            return EFAULT;
        }
        let p = info_ptr as *mut u64;
        unsafe {
            p.write(fb.width as u64);
            p.add(1).write(fb.height as u64);
            p.add(2).write(fb.pitch as u64);
            p.add(3).write(fb.bpp as u64);
        }
    }
    FB_USER_BASE + off
}

#[cfg(not(target_arch = "x86_64"))]
fn sys_fb_map(_pml4: u64, _info_ptr: u64) -> u64 {
    ENOSYS
}

/// Poll input for a native app. Returns a packed word:
/// `key<<40 | buttons<<32 | mouse_y<<16 | mouse_x`. `key` is a consumed
/// keycode (0 if none) — safe because the desktop loop is paused while a
/// spawned app runs to completion.
#[cfg(target_arch = "x86_64")]
fn sys_poll_input() -> u64 {
    let key = crate::drivers::keyboard::poll_char() as u64;
    let mx = (crate::drivers::mouse::x().max(0) as u64) & 0xFFFF;
    let my = (crate::drivers::mouse::y().max(0) as u64) & 0xFFFF;
    let btn = crate::drivers::mouse::buttons() as u64 & 0xFF;
    (key << 40) | (btn << 32) | (my << 16) | mx
}

#[cfg(not(target_arch = "x86_64"))]
fn sys_poll_input() -> u64 {
    0
}
