#![no_std]

pub mod vga;
pub mod serial;
pub mod cpu;
pub mod io;

use core::ptr;

/// Initialize the kernel: set up serial, clear screen.
pub fn init() {
    serial::init();
    vga::clear();
    vga::set_color(vga::Color::LightGreen, vga::Color::Black);
    serial::write(b"ling-kernel initialized\n");
}

/// Panic on unreachable kernel path.
pub fn kernel_panic(msg: &str) -> ! {
    serial::write(b"KERNEL PANIC: ");
    serial::write(msg.as_bytes());
    serial::write(b"\n");
    loop {
        unsafe { cpu::halt(); }
    }
}

// ─── Extern "C" ABI for the AOT compiler's kernel intrinsics ────────────

#[no_mangle]
pub unsafe extern "C" fn ling_kernel_vga_write_str(s: u64) -> u64 {
    // s is a NaN-boxed string, but in kernel mode we pass raw ptr+len
    // For now, treat s as a pointer to a null-terminated string
    if s != 0 {
        let ptr = s as *const u8;
        let mut i = 0;
        loop {
            let c = ptr::read_volatile(ptr.add(i));
            if c == 0 {
                break;
            }
            vga::write_char(c);
            i += 1;
        }
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn ling_kernel_vga_write_char(c: u64) -> u64 {
    vga::write_char(c as u8);
    0
}

#[no_mangle]
pub unsafe extern "C" fn ling_kernel_vga_clear() -> u64 {
    vga::clear();
    0
}

#[no_mangle]
pub unsafe extern "C" fn ling_kernel_halt() -> u64 {
    loop {
        cpu::halt();
    }
}

#[no_mangle]
pub unsafe extern "C" fn ling_kernel_cli() -> u64 {
    cpu::cli();
    0
}

#[no_mangle]
pub unsafe extern "C" fn ling_kernel_sti() -> u64 {
    cpu::sti();
    0
}

#[no_mangle]
pub unsafe extern "C" fn ling_kernel_inb(port: u64) -> u64 {
    io::inb(port as u16) as u64
}

#[no_mangle]
pub unsafe extern "C" fn ling_kernel_outb(port: u64, val: u64) -> u64 {
    io::outb(port as u16, val as u8);
    0
}

#[no_mangle]
pub unsafe extern "C" fn ling_kernel_serial_write(ptr: u64, len: u64) -> u64 {
    let slice = core::slice::from_raw_parts(ptr as *const u8, len as usize);
    serial::write(slice);
    0
}

#[no_mangle]
pub unsafe extern "C" fn ling_kernel_panic(msg: u64) -> u64 {
    if msg != 0 {
        let ptr = msg as *const u8;
        let mut i = 0;
        loop {
            let c = ptr::read_volatile(ptr.add(i));
            if c == 0 {
                break;
            }
            i += 1;
        }
        let msg_str = core::str::from_utf8_unchecked(core::slice::from_raw_parts(ptr, i));
        kernel_panic(msg_str);
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn ling_kernel_init() -> u64 {
    init();
    0
}

#[no_mangle]
pub unsafe extern "C" fn ling_kernel_asm_exec(ptr: u64, len: u64) -> u64 {
    let template = core::str::from_utf8_unchecked(
        core::slice::from_raw_parts(ptr as *const u8, len as usize)
    );
    match template {
        "hlt" | "halt" => {
            loop { cpu::halt(); }
        }
        "cli" => { cpu::cli(); 0 },
        "sti" => { cpu::sti(); 0 },
        "nop" => { core::hint::spin_loop(); 0 },
        "pause" => { core::hint::spin_loop(); 0 },
        "int3" => { unsafe { core::arch::asm!("int3"); } 0 },
        _ => {
            serial::write(b"ling_kernel_asm_exec: unknown template: ");
            serial::write(template.as_bytes());
            serial::write(b"\n");
            loop { cpu::halt(); }
        }
    }
}
