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

#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub unsafe fn syscall0(n: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") n => ret,
        out("rcx") _,
        out("r11") _,
        options(nostack)
    );
    ret
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub unsafe fn syscall1(n: u64, a1: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") n => ret,
        in("rdi") a1,
        out("rcx") _,
        out("r11") _,
        options(nostack)
    );
    ret
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub unsafe fn syscall2(n: u64, a1: u64, a2: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") n => ret,
        in("rdi") a1,
        in("rsi") a2,
        out("rcx") _,
        out("r11") _,
        options(nostack)
    );
    ret
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub unsafe fn syscall3(n: u64, a1: u64, a2: u64, a3: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") n => ret,
        in("rdi") a1,
        in("rsi") a2,
        in("rdx") a3,
        out("rcx") _,
        out("r11") _,
        options(nostack)
    );
    ret
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub unsafe fn syscall4(n: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") n => ret,
        in("rdi") a1,
        in("rsi") a2,
        in("rdx") a3,
        in("r10") a4,
        out("rcx") _,
        out("r11") _,
        options(nostack)
    );
    ret
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub unsafe fn syscall0(n: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "svc #0",
        in("x8") n,
        out("x0") ret,
        options(nostack)
    );
    ret
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub unsafe fn syscall1(n: u64, a1: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "svc #0",
        in("x8") n,
        inlateout("x0") a1 => ret,
        options(nostack)
    );
    ret
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub unsafe fn syscall2(n: u64, a1: u64, a2: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "svc #0",
        in("x8") n,
        inlateout("x0") a1 => ret,
        in("x1") a2,
        options(nostack)
    );
    ret
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub unsafe fn syscall3(n: u64, a1: u64, a2: u64, a3: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "svc #0",
        in("x8") n,
        inlateout("x0") a1 => ret,
        in("x1") a2,
        in("x2") a3,
        options(nostack)
    );
    ret
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub unsafe fn syscall4(n: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "svc #0",
        in("x8") n,
        inlateout("x0") a1 => ret,
        in("x1") a2,
        in("x2") a3,
        in("x3") a4,
        options(nostack)
    );
    ret
}

#[no_mangle]
pub unsafe extern "C" fn ling_sys_exit(code: u64) -> u64 {
    syscall1(SYS_EXIT, code)
}

#[no_mangle]
pub unsafe extern "C" fn ling_sys_write(fd: u64, ptr: u64, len: u64) -> u64 {
    syscall3(SYS_WRITE, fd, ptr, len)
}

#[no_mangle]
pub unsafe extern "C" fn ling_sys_read(fd: u64, ptr: u64, len: u64) -> u64 {
    syscall3(SYS_READ, fd, ptr, len)
}

#[no_mangle]
pub unsafe extern "C" fn ling_sys_open(path: u64, len: u64) -> u64 {
    syscall2(SYS_OPEN, path, len)
}

#[no_mangle]
pub unsafe extern "C" fn ling_sys_close(fd: u64) -> u64 {
    syscall1(SYS_CLOSE, fd)
}

#[no_mangle]
pub unsafe extern "C" fn ling_sys_lseek(fd: u64, offset: u64, whence: u64) -> u64 {
    syscall3(SYS_LSEEK, fd, offset, whence)
}

#[no_mangle]
pub unsafe extern "C" fn ling_sys_mmap(addr: u64, len: u64, prot: u64, flags: u64) -> u64 {
    syscall4(SYS_MMAP, addr, len, prot, flags)
}

#[no_mangle]
pub unsafe extern "C" fn ling_sys_munmap(addr: u64, len: u64) -> u64 {
    syscall2(SYS_MUNMAP, addr, len)
}

#[no_mangle]
pub unsafe extern "C" fn ling_sys_spawn(path: u64, len: u64) -> u64 {
    syscall2(SYS_SPAWN, path, len)
}

#[no_mangle]
pub unsafe extern "C" fn ling_sys_waitpid(pid: u64) -> u64 {
    syscall1(SYS_WAITPID, pid)
}

#[no_mangle]
pub unsafe extern "C" fn ling_sys_yield() -> u64 {
    syscall0(SYS_YIELD)
}

#[no_mangle]
pub unsafe extern "C" fn ling_sys_getpid() -> u64 {
    syscall0(SYS_GETPID)
}

#[no_mangle]
pub unsafe extern "C" fn ling_sys_sleep_ms(ms: u64) -> u64 {
    syscall1(SYS_SLEEP_MS, ms)
}

#[no_mangle]
pub unsafe extern "C" fn ling_sys_poll_input() -> u64 {
    syscall0(SYS_POLL_INPUT)
}

#[no_mangle]
pub unsafe extern "C" fn ling_sys_fb_map() -> u64 {
    syscall0(SYS_FB_MAP)
}

#[no_mangle]
pub unsafe extern "C" fn ling_sys_uname(buf: u64, len: u64) -> u64 {
    syscall2(SYS_UNAME, buf, len)
}
