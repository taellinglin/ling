use crate::arch::io;

const COM1: u16 = 0x3F8;

pub fn init() {
    unsafe {
        io::outb(COM1 + 1, 0x00);
        io::outb(COM1 + 3, 0x80);
        io::outb(COM1 + 0, 0x03);
        io::outb(COM1 + 1, 0x00);
        io::outb(COM1 + 3, 0x03);
        io::outb(COM1 + 2, 0xC7);
        io::outb(COM1 + 4, 0x0B);
        io::outb(COM1 + 4, 0x1E);
        io::outb(COM1 + 0, 0xAE);
        if io::inb(COM1 + 0) != 0xAE {
            return;
        }
        io::outb(COM1 + 4, 0x0F);
    }
}

pub fn write_ready() -> bool {
    unsafe { io::inb(COM1 + 5) & 0x20 != 0 }
}

pub fn write_byte(byte: u8) {
    while !write_ready() {
        unsafe {
            crate::arch::cpu::pause();
        }
    }
    unsafe {
        io::outb(COM1, byte);
    }
}

/// Everything printed since boot (the last 512 KiB), for `report`: a real PC
/// has no serial port to read, and photos of the screen cut long lines off.
const KLOG_SIZE: usize = 512 * 1024;
static mut KLOG: [u8; KLOG_SIZE] = [0; KLOG_SIZE];
static mut KLOG_TOTAL: usize = 0;

/// The kernel log, oldest first.
pub fn klog() -> alloc::vec::Vec<u8> {
    unsafe {
        let buf = &*(&raw const KLOG);
        let total = KLOG_TOTAL;
        if total <= KLOG_SIZE {
            return buf[..total].to_vec();
        }
        let at = total % KLOG_SIZE;
        let mut v = buf[at..].to_vec();
        v.extend_from_slice(&buf[..at]);
        v
    }
}

pub fn write(bytes: &[u8]) {
    unsafe {
        let buf = &mut *(&raw mut KLOG);
        for &b in bytes {
            buf[KLOG_TOTAL % KLOG_SIZE] = b;
            KLOG_TOTAL += 1;
        }
    }
    for &b in bytes {
        write_byte(b);
    }
    // boot messages are the boot screen's live data (no-op once it's gone)
    crate::drivers::bootscreen::feed(bytes);
}

pub fn write_str(s: &str) {
    write(s.as_bytes());
}
