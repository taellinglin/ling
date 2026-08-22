//! Virtual terminal multiplexer for the VGA text console: term0..term3,
//! switchable via F1-F4 (see `keyboard.rs`). There's a single input/shell
//! stream driving the kernel, so this is a simple screen-mux rather than
//! independent per-terminal processes — writes always target "whichever
//! terminal is currently active," and switching just changes which saved
//! screen buffer is visible on the real hardware. Inactive terminals keep
//! whatever was last drawn to them (blank, until first switched to).
use core::ptr;

pub const NUM_TERMS: usize = 4;
const WIDTH: usize = 80;
const HEIGHT: usize = 25;
const VGA_BUF: *mut u8 = 0xB8000 as *mut u8;

#[derive(Copy, Clone)]
struct Term {
    cells: [u8; WIDTH * HEIGHT * 2], // (char, attr) pairs, same layout as VGA memory
    col: usize,
    row: usize,
    color: u8,
}

impl Term {
    const fn new() -> Self {
        Self { cells: [0; WIDTH * HEIGHT * 2], col: 0, row: 0, color: 0x07 }
    }
}

static mut TERMS: [Term; NUM_TERMS] = [Term::new(); NUM_TERMS];
static mut ACTIVE: usize = 0;

// ─── Software cursor ──────────────────────────────────────────────────────
// See `vga::disable_hardware_cursor`'s doc comment for why this exists
// instead of programming the CRTC cursor-position registers: drawing it
// ourselves, re-derived from the live `col`/`row` on every blit, means it's
// always exactly where the cursor actually is rather than a stale register
// nobody ever updates. Overlaid on top of the real hardware copy in
// `blit_active_to_hardware`, never written into `cells` itself, so it never
// needs "restoring" -- the next blit just omits it during the off phase.

const CURSOR_CHAR: u8 = b'|';
const CURSOR_BLINK_MS: u64 = 250; // faster than the ~530ms hardware default
                                  // Cycles one step through these brighter palette slots each time the
                                  // cursor blinks back on (Color::{LightCyan, LightGreen, Yellow, LightMagenta}).
const CURSOR_COLORS: [u8; 4] = [0x0B, 0x0A, 0x0E, 0x0D];

static mut CURSOR_VISIBLE: bool = true;
static mut CURSOR_COLOR_IDX: usize = 0;
static mut CURSOR_LAST_TOGGLE_MS: u64 = 0;

/// Called from the keyboard idle loop, which already wakes ~100x/sec off
/// the timer heartbeat even with no key activity (see
/// `keyboard::read_char`'s `hlt`) -- advances the blink clock and re-blits
/// if the phase changed. A no-op the rest of the time; no new interrupt
/// wiring needed.
pub fn tick_cursor_blink() {
    let now = crate::arch::timer::now_ms();
    unsafe {
        if now.saturating_sub(CURSOR_LAST_TOGGLE_MS) < CURSOR_BLINK_MS {
            return;
        }
        CURSOR_LAST_TOGGLE_MS = now;
        CURSOR_VISIBLE = !CURSOR_VISIBLE;
        if CURSOR_VISIBLE {
            CURSOR_COLOR_IDX = (CURSOR_COLOR_IDX + 1) % CURSOR_COLORS.len();
        }
    }
    blit_active_to_hardware();
}

fn blit_active_to_hardware() {
    unsafe {
        let t = &TERMS[ACTIVE];
        for i in 0..(WIDTH * HEIGHT * 2) {
            ptr::write_volatile(VGA_BUF.add(i), t.cells[i]);
        }
        if CURSOR_VISIBLE && t.row < HEIGHT && t.col < WIDTH {
            let pos = t.row * WIDTH + t.col;
            ptr::write_volatile(VGA_BUF.add(pos * 2), CURSOR_CHAR);
            ptr::write_volatile(VGA_BUF.add(pos * 2 + 1), CURSOR_COLORS[CURSOR_COLOR_IDX]);
        }
    }
}

/// Switch the visible terminal (F1..F4 map to 0..3). Out-of-range indices
/// and switching to the already-active terminal are no-ops.
pub fn switch_to(n: usize) {
    if n >= NUM_TERMS {
        return;
    }
    unsafe {
        if ACTIVE == n {
            return;
        }
        ACTIVE = n;
    }
    blit_active_to_hardware();
}

fn scroll(t: &mut Term) {
    for row in 1..HEIGHT {
        for col in 0..WIDTH {
            let src = (row * WIDTH + col) * 2;
            let dst = ((row - 1) * WIDTH + col) * 2;
            t.cells[dst] = t.cells[src];
            t.cells[dst + 1] = t.cells[src + 1];
        }
    }
    let last = (HEIGHT - 1) * WIDTH;
    for col in 0..WIDTH {
        t.cells[(last + col) * 2] = b' ';
        t.cells[(last + col) * 2 + 1] = t.color;
    }
}

/// Write one byte to the active terminal (and, since it's the visible one,
/// the real screen).
pub fn write_char_active(c: u8) {
    unsafe {
        let active = ACTIVE;
        {
            let t = &mut TERMS[active];
            match c {
                b'\n' => {
                    t.col = 0;
                    t.row += 1;
                },
                b'\r' => t.col = 0,
                0x08 => {
                    // Backspace: move left and erase the cell there (doesn't
                    // walk back across a line wrap onto the previous row —
                    // a real shell's line buffer only ever calls this while
                    // the cursor is still on the row it started the line at).
                    if t.col > 0 {
                        t.col -= 1;
                        let pos = (t.row * WIDTH + t.col) * 2;
                        if pos < t.cells.len() {
                            t.cells[pos] = b' ';
                            t.cells[pos + 1] = t.color;
                        }
                    }
                },
                _ => {
                    let pos = (t.row * WIDTH + t.col) * 2;
                    if pos < t.cells.len() {
                        t.cells[pos] = c;
                        t.cells[pos + 1] = t.color;
                    }
                    t.col += 1;
                    if t.col >= WIDTH {
                        t.col = 0;
                        t.row += 1;
                    }
                },
            }
            if t.row >= HEIGHT {
                scroll(t);
                t.row = HEIGHT - 1;
            }
        }
    }
    blit_active_to_hardware();
}

pub fn clear_active() {
    unsafe {
        let active = ACTIVE;
        let t = &mut TERMS[active];
        for i in 0..(WIDTH * HEIGHT) {
            t.cells[i * 2] = b' ';
            t.cells[i * 2 + 1] = t.color;
        }
        t.col = 0;
        t.row = 0;
    }
    blit_active_to_hardware();
}

pub fn set_color_active(color: u8) {
    unsafe {
        TERMS[ACTIVE].color = color;
    }
}

/// Direct random-access cell write (row/col, not the streaming
/// cursor/scroll model `write_char_active` uses) — for a full-grid
/// animation like `life.rs` that redraws arbitrary cells every generation.
/// Doesn't blit to hardware itself; call `blit_active` once after a batch
/// of these so a whole frame updates together.
pub fn set_cell_active(row: usize, col: usize, ch: u8, color: u8) {
    unsafe {
        if row >= HEIGHT || col >= WIDTH {
            return;
        }
        let t = &mut TERMS[ACTIVE];
        let pos = (row * WIDTH + col) * 2;
        t.cells[pos] = ch;
        t.cells[pos + 1] = color;
    }
}

pub fn blit_active() {
    blit_active_to_hardware();
}
