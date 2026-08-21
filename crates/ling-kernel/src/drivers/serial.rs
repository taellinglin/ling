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
        unsafe { crate::arch::cpu::pause(); }
    }
    unsafe { io::outb(COM1, byte); }
}

pub fn write(bytes: &[u8]) {
    for &b in bytes {
        write_byte(b);
    }
}

pub fn write_str(s: &str) {
    write(s.as_bytes());
}
