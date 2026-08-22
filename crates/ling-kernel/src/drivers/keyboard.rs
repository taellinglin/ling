use crate::arch::cpu;

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

/// Idle-font swap threshold, now a real duration off `timer::now_ms` (a
/// calibrated TSC clock, independent of IRQ delivery) rather than a busy-poll
/// spin count guessed at "a few idle seconds at typical hardware/QEMU
/// speeds" — replaced now that IRQ-driven input makes counting poll
/// iterations meaningless (there often aren't any: `read_char` parks in
/// `hlt` between keystrokes instead of spinning).
const IDLE_MS_THRESHOLD: u64 = 300_000; // 5 minutes

static mut LAST_ACTIVITY_MS: u64 = 0;
static mut IDLE_FIRED: bool = false;

/// A gap this much longer than any real inter-keystroke timing most likely
/// means something interrupted input mid-sequence -- a VirtualBox host
/// focus change or Ctrl+Alt+Del was the reported trigger. Without this,
/// e.g. `PENDING_EXTENDED` left `true` by a make code that never got its
/// follow-up byte before focus was lost would silently misroute every
/// subsequent scancode through the extended-key branch forever, with no way
/// to self-recover. Needs real VirtualBox confirmation, not just QEMU --
/// this is the best fix available without reproducing the exact PS/2
/// resync behavior VirtualBox's host-focus handling triggers.
const FOCUS_LOSS_GAP_MS: u64 = 2_000;

/// Reset modifier/sequence state if it's been stale long enough that
/// trusting it would misinterpret the next scancode. Call before processing
/// any newly-read scancode, using the *old* `LAST_ACTIVITY_MS` (i.e. before
/// `note_activity()` updates it).
fn reset_stale_modifier_state() {
    unsafe {
        if LAST_ACTIVITY_MS == 0 {
            return; // first keypress ever -- nothing to have gotten stuck
        }
        let now = crate::arch::timer::now_ms();
        if now.saturating_sub(LAST_ACTIVITY_MS) > FOCUS_LOSS_GAP_MS {
            PENDING_EXTENDED = false;
            CTRL_DOWN = false;
            SHIFT_DOWN = false;
        }
    }
}

/// Fixed-capacity scancode queue the IRQ1 handler (`arch::x86_64::idt`)
/// pushes into and `read_scancode` drains. Single producer (the ISR, which
/// the "interrupt gate" IDT entry type guarantees can't re-enter itself),
/// single consumer (whichever kernel/task context calls `read_char`/
/// `poll_char`) — safe without a lock: the producer only ever advances
/// `RING_HEAD` and writes the slot it just claimed, the consumer only ever
/// advances `RING_TAIL` after reading the slot it names, and x86's strong
/// memory ordering means a plain `u8` index (which wraps for free at 256,
/// matching `RING`'s length) is enough for both sides to agree on what's
/// been produced vs. consumed.
const RING_CAPACITY: usize = 256;
static mut RING: [u8; RING_CAPACITY] = [0; RING_CAPACITY];
static mut RING_HEAD: u8 = 0;
static mut RING_TAIL: u8 = 0;

/// Called from the IRQ1 handler with the raw scancode byte just read off
/// port 0x60. Drops the byte (rather than overwriting an unread one) if the
/// ring is full — a burst of unread input is a starved consumer, not a
/// reason to corrupt older, still-unread bytes.
pub(crate) fn irq_scancode(code: u8) {
    unsafe {
        let next = RING_HEAD.wrapping_add(1);
        if next != RING_TAIL {
            RING[RING_HEAD as usize] = code;
            RING_HEAD = next;
        }
    }
}

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

/// Non-blocking scancode read from the IRQ-fed ring buffer. Returns `None`
/// if nothing is waiting.
fn read_scancode() -> Option<u8> {
    unsafe {
        if RING_HEAD == RING_TAIL {
            return None;
        }
        let byte = RING[RING_TAIL as usize];
        RING_TAIL = RING_TAIL.wrapping_add(1);
        Some(byte)
    }
}

/// Any real key event (or, via the IRQ12 mouse handler, mouse movement/
/// click) resets the idle clock, swaps back to the normal font if the idle
/// one had fired, and re-arms the idle-font swap so it can fire again next
/// time activity stops for a while.
pub(crate) fn note_activity() {
    unsafe {
        if IDLE_FIRED {
            crate::drivers::vga::restore_normal_font();
        }
        LAST_ACTIVITY_MS = crate::arch::timer::now_ms();
        IDLE_FIRED = false;
    }
}

/// Called whenever `read_char` wakes from `hlt` with nothing waiting;
/// swaps to the idle font the first time `IDLE_MS_THRESHOLD` elapses since
/// the last real activity (and only that once, until activity resets it).
fn note_idle_check() {
    unsafe {
        if IDLE_FIRED {
            return;
        }
        if crate::arch::timer::now_ms().saturating_sub(LAST_ACTIVITY_MS) >= IDLE_MS_THRESHOLD {
            IDLE_FIRED = true;
            crate::drivers::vga::use_idle_font();
        }
    }
}

/// If `code` is an F1..F4 make code, switch terminals and report handled.
fn try_switch_term(code: u8) -> bool {
    if (F1..=F4).contains(&code) {
        crate::drivers::term::switch_to((code - F1) as usize);
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
            reset_stale_modifier_state();
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
            note_idle_check();
            // The PIT heartbeat wakes this `hlt` ~100x/sec even with no key
            // activity, which is exactly the periodic hook the cursor blink
            // needs (see `term::tick_cursor_blink`'s doc comment) -- no new
            // interrupt wiring required to get it ticking while idle.
            crate::drivers::term::tick_cursor_blink();
            // Park until the next interrupt (keyboard, mouse, or the PIT
            // heartbeat, whichever comes first) instead of burning CPU in a
            // busy-poll — real work now that input is IRQ-delivered rather
            // than requiring a spin to notice a new byte.
            unsafe { cpu::halt() };
        }
    }
}

/// Non-blocking poll: returns 0 if no mapped key is currently waiting.
pub fn poll_char() -> u8 {
    match read_scancode() {
        Some(code) => {
            reset_stale_modifier_state();
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
