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

fn blit_active_to_hardware() {
    unsafe {
        let t = &TERMS[ACTIVE];
        for i in 0..(WIDTH * HEIGHT * 2) {
            ptr::write_volatile(VGA_BUF.add(i), t.cells[i]);
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
    unsafe { TERMS[ACTIVE].color = color; }
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
