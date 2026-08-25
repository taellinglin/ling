extern "C" {
    fn __main__() -> u64;
}

#[no_mangle]
pub unsafe extern "C" fn _start() -> ! {
    let ret = __main__();
    let code = if (ret >> 56) == 0x7F {
        0
    } else {
        (f64::from_bits(ret) as i32) as u64
    };
    crate::syscall::ling_sys_exit(code);
    loop {}
}

/// Fixed-size stack buffer implementing `core::fmt::Write`, so the panic
/// handler can format the real message + location without touching the
/// heap — appropriate here since the panic itself might *be* an allocator
/// failure. Truncates silently past capacity rather than failing to write.
struct PanicBuf {
    buf: [u8; 512],
    len: usize,
}

impl core::fmt::Write for PanicBuf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let bytes = s.as_bytes();
        let space = self.buf.len() - self.len;
        let n = bytes.len().min(space);
        self.buf[self.len..self.len + n].copy_from_slice(&bytes[..n]);
        self.len += n;
        Ok(())
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    use core::fmt::Write;
    let mut out = PanicBuf { buf: [0; 512], len: 0 };
    // PanicInfo's own Display impl already includes the message and
    // location ("panicked at src/foo.rs:12:5:\n<message>") — writing to a
    // fixed buffer can't itself allocate or panic, so this stays safe even
    // when the panic is an allocator failure. `ling_panic`'s msg_ptr==0
    // fallback (a Ling-string pointer convention this raw buffer isn't) is
    // reserved for contexts with no PanicInfo to report, e.g. alloc.rs's
    // own OOM checks.
    let _ = write!(out, "{info}\n");
    unsafe {
        crate::syscall::ling_sys_write(2, out.buf.as_ptr() as u64, out.len as u64);
        crate::syscall::ling_sys_exit(1);
    }
    loop {}
}
