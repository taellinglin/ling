use crate::io;

const DATA_PORT: u16 = 0x60;
const STATUS_PORT: u16 = 0x64;
const OUTPUT_FULL: u8 = 0x01;

// F1..F4 (scancode-set-1 make codes) switch the visible terminal (term.rs).
const F1: u8 = 0x3B;
const F4: u8 = 0x3E;

// Left Ctrl + C: reported to callers as ASCII ETX (0x03), the conventional
// "interrupt" byte — `ling_kernel_read_line` uses it to abort the line
// currently being typed. Release codes have the top bit set (0x80).
const LCTRL: u8 = 0x1D;
const C_KEY: u8 = 0x2E;
const ETX: u8 = 0x03;

static mut CTRL_DOWN: bool = false;

// Left/Right Shift: swap in `SCANCODE_ASCII_SHIFTED` (uppercase letters and
// the punctuation each digit/symbol key produces when shifted) for as long
// as either is held. Caps Lock isn't tracked — only Shift toggles this.
const LSHIFT: u8 = 0x2A;
const RSHIFT: u8 = 0x36;

static mut SHIFT_DOWN: bool = false;

// Extended (0xE0-prefixed) keys: only Up/Down are handled, for command
// history recall (`ling_kernel_read_line`). Reported to callers as DC1/DC2
// (0x11/0x12) — control codes nothing else uses. Left/Right aren't handled
// yet (no mid-line cursor movement in the line editor).
const EXT_PREFIX: u8 = 0xE0;
const EXT_UP: u8 = 0x48;
const EXT_DOWN: u8 = 0x50;
pub const UP_ARROW: u8 = 0x11;
pub const DOWN_ARROW: u8 = 0x12;

static mut PENDING_EXTENDED: bool = false;

/// Roughly how many idle poll spins before swapping to the idle font
/// (`vga::use_idle_font`). There's no calibrated timer/interrupt source yet
/// (this is a "basic" kernel) so this is just a large enough spin count to
/// represent a few idle seconds at typical hardware/QEMU speeds, not an
/// exact duration.
const IDLE_THRESHOLD: u32 = 50_000_000;

static mut IDLE_TICKS: u32 = 0;
static mut IDLE_FIRED: bool = false;

/// US QWERTY scancode-set-1 (make codes only) -> ASCII, unshifted.
static SCANCODE_ASCII: [u8; 88] = [
    0, 27, b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', // 0x00-0x09
    b'9', b'0', b'-', b'=', 8, b'\t', // 0x0A-0x0F
    b'q', b'w', b'e', b'r', b't', b'y', b'u', b'i', b'o', b'p', b'[', b']', b'\n', 0, // 0x10-0x1D (0x1D = LCtrl)
    b'a', b's', b'd', b'f', b'g', b'h', b'j', b'k', b'l', b';', b'\'', b'`', 0, b'\\', // 0x1E-0x2B (0x2A = LShift)
    b'z', b'x', b'c', b'v', b'b', b'n', b'm', b',', b'.', b'/', 0, // 0x2C-0x36 (0x36 = RShift)
    b'*', 0, b' ', // 0x37-0x39 (0x38 = LAlt, 0x39 = space)
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 0x3A-0x43 (function/lock keys, unmapped)
    0, 0, 0, 0, 0, 0, 0, b'7', b'8', b'9', b'-', b'4', b'5', b'6', b'+', b'1', b'2', b'3', b'0', b'.', // keypad
];

/// Same layout, shifted: digit row becomes its symbols, letters go
/// uppercase, and the rest of the punctuation keys produce their shifted
/// character. Modifier/lock/keypad slots are left identical to the unshifted
/// table (keypad ignores Shift here; only NumLock would change it, and that
/// isn't tracked).
static SCANCODE_ASCII_SHIFTED: [u8; 88] = [
    0, 27, b'!', b'@', b'#', b'$', b'%', b'^', b'&', b'*', // 0x00-0x09
    b'(', b')', b'_', b'+', 8, b'\t', // 0x0A-0x0F
    b'Q', b'W', b'E', b'R', b'T', b'Y', b'U', b'I', b'O', b'P', b'{', b'}', b'\n', 0, // 0x10-0x1D
    b'A', b'S', b'D', b'F', b'G', b'H', b'J', b'K', b'L', b':', b'"', b'~', 0, b'|', // 0x1E-0x2B
    b'Z', b'X', b'C', b'V', b'B', b'N', b'M', b'<', b'>', b'?', 0, // 0x2C-0x36
    b'*', 0, b' ', // 0x37-0x39
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 0x3A-0x43
    0, 0, 0, 0, 0, 0, 0, b'7', b'8', b'9', b'-', b'4', b'5', b'6', b'+', b'1', b'2', b'3', b'0', b'.', // keypad
];

fn lookup_ascii(code: u8) -> u8 {
    let table = if unsafe { SHIFT_DOWN } { &SCANCODE_ASCII_SHIFTED } else { &SCANCODE_ASCII };
    table.get(code as usize).copied().unwrap_or(0)
}

fn status_ready() -> bool {
    unsafe { io::inb(STATUS_PORT) & OUTPUT_FULL != 0 }
}

/// Non-blocking scancode read. Returns `None` if no key is waiting.
fn read_scancode() -> Option<u8> {
    if status_ready() {
        Some(unsafe { io::inb(DATA_PORT) })
    } else {
        None
    }
}

/// Any real key event (or, via `mouse::poll`, mouse movement/click) resets
/// the idle clock, swaps back to the normal font if the idle one had
/// fired, and re-arms the idle-font swap so it can fire again next time
/// activity stops for a while.
pub(crate) fn note_activity() {
    unsafe {
        if IDLE_FIRED {
            crate::vga::restore_normal_font();
        }
        IDLE_TICKS = 0;
        IDLE_FIRED = false;
    }
}

/// Called once per no-key-waiting spin; swaps to the idle font the first
/// time the idle threshold is crossed (and only that once, until activity
/// resets it).
fn note_idle_tick() {
    unsafe {
        if IDLE_FIRED {
            return;
        }
        IDLE_TICKS += 1;
        if IDLE_TICKS >= IDLE_THRESHOLD {
            IDLE_FIRED = true;
            crate::vga::use_idle_font();
        }
    }
}

/// If `code` is an F1..F4 make code, switch terminals and report handled.
fn try_switch_term(code: u8) -> bool {
    if (F1..=F4).contains(&code) {
        crate::term::switch_to((code - F1) as usize);
        true
    } else {
        false
    }
}

/// Poll for a single ASCII character (blocking). Release codes (top bit set)
/// and unmapped/extended scancodes are skipped over rather than returned;
/// F1..F4 switch terminals instead of producing a character.
pub fn read_char() -> u8 {
    loop {
        if let Some(code) = read_scancode() {
            note_activity();
            if code == EXT_PREFIX {
                unsafe { PENDING_EXTENDED = true; }
                continue;
            }
            if unsafe { PENDING_EXTENDED } {
                unsafe { PENDING_EXTENDED = false; }
                if code == EXT_UP {
                    return UP_ARROW;
                }
                if code == EXT_DOWN {
                    return DOWN_ARROW;
                }
                continue;
            }
            if code == LCTRL {
                unsafe { CTRL_DOWN = true; }
                continue;
            }
            if code == LCTRL | 0x80 {
                unsafe { CTRL_DOWN = false; }
                continue;
            }
            if code == LSHIFT || code == RSHIFT {
                unsafe { SHIFT_DOWN = true; }
                continue;
            }
            if code == LSHIFT | 0x80 || code == RSHIFT | 0x80 {
                unsafe { SHIFT_DOWN = false; }
                continue;
            }
            // Release (key-up) codes have the top bit set; ignore them.
            if code & 0x80 != 0 {
                continue;
            }
            if try_switch_term(code) {
                continue;
            }
            if code == C_KEY && unsafe { CTRL_DOWN } {
                return ETX;
            }
            let ascii = lookup_ascii(code);
            if ascii != 0 {
                return ascii;
            }
        } else {
            note_idle_tick();
            unsafe { crate::cpu::pause(); }
        }
    }
}

/// Non-blocking poll: returns 0 if no mapped key is currently waiting.
pub fn poll_char() -> u8 {
    match read_scancode() {
        Some(code) => {
            note_activity();
            if code == EXT_PREFIX {
                unsafe { PENDING_EXTENDED = true; }
                return 0;
            }
            if unsafe { PENDING_EXTENDED } {
                unsafe { PENDING_EXTENDED = false; }
                if code == EXT_UP {
                    return UP_ARROW;
                }
                if code == EXT_DOWN {
                    return DOWN_ARROW;
                }
                return 0;
            }
            if code == LCTRL {
                unsafe { CTRL_DOWN = true; }
                return 0;
            }
            if code == LCTRL | 0x80 {
                unsafe { CTRL_DOWN = false; }
                return 0;
            }
            if code == LSHIFT || code == RSHIFT {
                unsafe { SHIFT_DOWN = true; }
                return 0;
            }
            if code == LSHIFT | 0x80 || code == RSHIFT | 0x80 {
                unsafe { SHIFT_DOWN = false; }
                return 0;
            }
            if code & 0x80 != 0 || try_switch_term(code) {
                return 0;
            }
            if code == C_KEY && unsafe { CTRL_DOWN } {
                return ETX;
            }
            lookup_ascii(code)
        },
        None => 0,
    }
}
