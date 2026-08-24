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

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    unsafe { crate::alloc::ling_panic(0) };
}
