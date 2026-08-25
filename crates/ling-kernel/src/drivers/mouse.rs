//! PS/2 mouse driver: IRQ12-driven (`arch::x86_64::idt`'s mouse ISR calls
//! `irq_byte` directly, one raw byte per interrupt) — position/buttons are
//! current as of the last IRQ, no polling needed. Standard 3-byte packet
//! protocol only (no scroll wheel/5-button IntelliMouse extension) — enough
//! for a basic pointer, which is all a first window manager needs.
//!
//! Shares the 8042 controller's ports with `keyboard.rs` (0x60 data, 0x64
//! status/command) for the one-time `init()` handshake below — but unlike
//! that shared-port polling era, IRQ1 and IRQ12 are separate lines wired to
//! separate devices, so which one fired already tells the ISR which device
//! sent the byte; there's no need to check the status register's
//! keyboard/mouse origin bit per byte the way a polled shared-port reader
//! would have to.
use crate::arch::io;

const DATA_PORT: u16 = 0x60;
const STATUS_PORT: u16 = 0x64;
const CMD_PORT: u16 = 0x64;
const OUTPUT_FULL: u8 = 0x01;
const INPUT_FULL: u8 = 0x02;

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
/// streaming-report mode. Must run with IRQ12 still masked: this is a
/// synchronous request/ack handshake over the same status-port bits an
/// in-flight mouse IRQ would also be consuming, so unmasking first would
/// race the handler for the 0xFA ack bytes below (`arch::x86_64::idt::init`
/// unmasks IRQ12 only after this returns).
pub fn init() {
    write_command(0xA8); // enable the auxiliary device

    // Enable IRQ12 delivery in the 8042 controller config byte (bit 1 =
    // aux-port interrupt enable; also make sure bit 5, aux clock disable,
    // is clear). SeaBIOS under QEMU boots with aux interrupts OFF, so
    // without this every mouse byte just sat in the output buffer and
    // IRQ12 never fired -- the keyboard kept working (its bit 0 defaults
    // on), which made this look like anything but an 8042 config issue.
    // Found via the headless monitor-driven WM test (`mouse_move` injected
    // packets, cursor never moved); VirtualBox's BIOS enables the bit
    // itself, which is why the earlier VBox screenshots had a live cursor.
    write_command(0x20); // read config byte
    let cfg = read_data();
    write_command(0x60); // write config byte
    write_data((cfg | 0x02) & !0x20);

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

/// Feed one raw byte from the IRQ12 handler, accumulating it into the
/// current 3-byte packet and applying position/button updates once a full
/// packet has arrived.
pub(crate) fn irq_byte(byte: u8) {
    crate::drivers::keyboard::note_activity();
    unsafe {
        BYTE_COUNT = BYTE_COUNT.wrapping_add(1);
        LAST_BYTE = byte;
        PACKET[PACKET_IDX] = byte;
        PACKET_IDX += 1;
        if PACKET_IDX == 3 {
            PACKET_IDX = 0;
            apply_packet(PACKET);
        }
    }
}

static mut BYTE_COUNT: u32 = 0;
static mut LAST_BYTE: u8 = 0;

/// Raw IRQ12 bytes received since boot -- diagnostic counter for verifying
/// the interrupt path end-to-end (a cursor that doesn't move could be a
/// hit-test bug or a dead IRQ line; this distinguishes them over serial).
pub fn byte_count() -> u32 {
    unsafe { BYTE_COUNT }
}

pub fn last_byte() -> u8 {
    unsafe { LAST_BYTE }
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
