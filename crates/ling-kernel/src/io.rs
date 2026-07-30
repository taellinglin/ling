use core::arch::asm;

pub unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    asm!("in al, dx", out("al") val, in("dx") port, options(nomem, nostack));
    val
}

pub unsafe fn inw(port: u16) -> u16 {
    let val: u16;
    asm!("in ax, dx", out("ax") val, in("dx") port, options(nomem, nostack));
    val
}

pub unsafe fn inl(port: u16) -> u32 {
    let val: u32;
    asm!("in eax, dx", out("eax") val, in("dx") port, options(nomem, nostack));
    val
}

pub unsafe fn outb(port: u16, val: u8) {
    asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack));
}

pub unsafe fn outw(port: u16, val: u16) {
    asm!("out dx, ax", in("dx") port, in("ax") val, options(nomem, nostack));
}

pub unsafe fn outl(port: u16, val: u32) {
    asm!("out dx, eax", in("dx") port, in("eax") val, options(nomem, nostack));
}

/// Read `count` words from `port` into `buf` (`rep insw`) — the bulk
/// transfer ATA PIO reads use to pull a whole sector in one go instead of
/// 256 individual `inw` calls.
pub unsafe fn insw(port: u16, buf: *mut u16, count: usize) {
    asm!(
        "rep insw",
        in("dx") port,
        inout("rdi") buf => _,
        inout("rcx") count => _,
        options(nostack),
    );
}

/// Write `count` words from `buf` to `port` (`rep outsw`).
pub unsafe fn outsw(port: u16, buf: *const u16, count: usize) {
    asm!(
        "rep outsw",
        in("dx") port,
        inout("rsi") buf => _,
        inout("rcx") count => _,
        options(nostack),
    );
}
