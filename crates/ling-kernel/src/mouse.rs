//! PS/2 mouse driver: polled, like `keyboard.rs` — there's no interrupt
//! handling wired up in this kernel yet, so `poll()` must be called
//! periodically (from a program's main loop) to drain any waiting bytes and
//! update position/buttons. Standard 3-byte packet protocol only (no
//! scroll wheel/5-button IntelliMouse extension) — enough for a basic
//! pointer, which is all a first window manager needs.
//!
//! Shares the 8042 controller's ports with `keyboard.rs` (0x60 data, 0x64
//! status/command) — the status register's bit 5 says whether a waiting
//! byte came from the auxiliary (mouse) device or the primary (keyboard)
//! one, which is how `poll()` avoids stealing keyboard bytes.
use crate::io;

const DATA_PORT: u16 = 0x60;
const STATUS_PORT: u16 = 0x64;
const CMD_PORT: u16 = 0x64;
const OUTPUT_FULL: u8 = 0x01;
const INPUT_FULL: u8 = 0x02;
const AUX_DATA: u8 = 0x20;

static mut MOUSE_X: i32 = 0;
static mut MOUSE_Y: i32 = 0;
static mut BUTTONS: u8 = 0;
static mut PACKET: [u8; 3] = [0; 3];
static mut PACKET_IDX: usize = 0;
static mut BOUND_W: i32 = 640;
static mut BOUND_H: i32 = 480;

fn wait_input_clear() {
    while unsafe { io::inb(STATUS_PORT) } & INPUT_FULL != 0 {}
}

fn wait_output_full() {
    while unsafe { io::inb(STATUS_PORT) } & OUTPUT_FULL == 0 {}
}

fn write_command(cmd: u8) {
    wait_input_clear();
    unsafe { io::outb(CMD_PORT, cmd) };
}

fn write_data(data: u8) {
    wait_input_clear();
    unsafe { io::outb(DATA_PORT, data) };
}

fn read_data() -> u8 {
    wait_output_full();
    unsafe { io::inb(DATA_PORT) }
}

/// Send a byte to the mouse itself (as opposed to the 8042 controller) —
/// every write to the auxiliary device needs the 0xD4 "next byte goes to
/// the second PS/2 port" prefix first.
fn mouse_write(data: u8) {
    write_command(0xD4);
    write_data(data);
}

/// Enable the auxiliary (mouse) port and put the mouse into its default
/// streaming-report mode. No interrupt setup — nothing here relies on
/// IRQ12, only on `poll()` being called.
pub fn init() {
    write_command(0xA8); // enable the auxiliary device
    mouse_write(0xF6); // set defaults
    read_data(); // ack (0xFA)
    mouse_write(0xF4); // enable data reporting
    read_data(); // ack (0xFA)
}

/// Clamp tracked position to `0..w`/`0..h` — set this to the framebuffer's
/// actual width/height once one is available (see `framebuffer.rs`); the
/// default is just a placeholder so a caller that forgets still gets sane
/// (if wrong) bounds instead of an unbounded position.
pub fn set_bounds(w: i32, h: i32) {
    unsafe {
        BOUND_W = w.max(1);
        BOUND_H = h.max(1);
    }
}

/// Drain any mouse bytes currently waiting, updating position/buttons as
/// complete 3-byte packets arrive. Non-blocking: returns as soon as nothing
/// (more) is waiting, or the next waiting byte belongs to the keyboard
/// (leaves it there for `keyboard::read_char`/`poll_char` to pick up).
pub fn poll() {
    loop {
        let status = unsafe { io::inb(STATUS_PORT) };
        if status & OUTPUT_FULL == 0 || status & AUX_DATA == 0 {
            return;
        }
        crate::keyboard::note_activity();
        let byte = unsafe { io::inb(DATA_PORT) };
        unsafe {
            PACKET[PACKET_IDX] = byte;
            PACKET_IDX += 1;
            if PACKET_IDX == 3 {
                PACKET_IDX = 0;
                apply_packet(PACKET);
            }
        }
    }
}

fn apply_packet(p: [u8; 3]) {
    let flags = p[0];
    if flags & 0x08 == 0 {
        // The "always 1" bit isn't set — this packet is misaligned (e.g. we
        // started listening mid-stream). Drop it; the next 3 bytes resync.
        return;
    }
    let mut dx = p[1] as i32;
    let mut dy = p[2] as i32;
    if flags & 0x10 != 0 {
        dx -= 256;
    }
    if flags & 0x20 != 0 {
        dy -= 256;
    }
    unsafe {
        MOUSE_X = (MOUSE_X + dx).clamp(0, BOUND_W - 1);
        // PS/2 reports +Y as "up"; screen/framebuffer +Y is "down".
        MOUSE_Y = (MOUSE_Y - dy).clamp(0, BOUND_H - 1);
        BUTTONS = flags & 0x07;
    }
}

pub fn x() -> i32 {
    unsafe { MOUSE_X }
}

pub fn y() -> i32 {
    unsafe { MOUSE_Y }
}

/// Bit 0 = left, bit 1 = right, bit 2 = middle.
pub fn buttons() -> u8 {
    unsafe { BUTTONS }
}
