//! Scrolling framebuffer text console for aarch64 (Raspberry Pi).
//!
//! The x86_64 side has VGA text mode plus the `term`/`wm` compositor; the Pi
//! only has a linear framebuffer, so kernel console output is rendered here
//! with `font8x8`, straight to the live framebuffer. A character grid is kept
//! alongside the pixels so scrolling is a cheap grid shift + redraw rather than
//! reading pixels back out of (often uncached / write-combining) VRAM.
//!
//! Assumes `font8x8` scale 1 (8x8 glyphs). The boot banner above the console
//! area leaves the scale at 1 when it finishes.

use crate::drivers::{font8x8, framebuffer};

const MAX_COLS: usize = 200;
const MAX_ROWS: usize = 120;
const CELL_W: u32 = 8; // font8x8 advance at scale 1
const CELL_H: u32 = 10; // 8px glyph + 2px leading
const MARGIN: u32 = 8;
/// Pixels reserved at the top for the boot banner + format diagnostics
/// (see `lib.rs::rpi_boot_screen`).
const TOP: u32 = 190;

const FG: u32 = 0xC8_D0_E0;
const BG: u32 = 0x10_10_18;

static mut GRID: [u8; MAX_COLS * MAX_ROWS] = [b' '; MAX_COLS * MAX_ROWS];
static mut COL: usize = 0;
static mut ROW: usize = 0;
static mut READY: bool = false;

fn cols() -> usize {
    let w = framebuffer::width();
    if w <= MARGIN * 2 {
        return 0;
    }
    (((w - MARGIN * 2) / CELL_W) as usize).min(MAX_COLS)
}

fn rows() -> usize {
    let h = framebuffer::height();
    if h <= TOP + CELL_H {
        return 0;
    }
    (((h - TOP) / CELL_H) as usize).min(MAX_ROWS)
}

/// Clear the console area (below the banner) and home the cursor. Call once the
/// framebuffer and boot banner are up.
pub fn init() {
    if !framebuffer::available() {
        return;
    }
    let (w, h) = (framebuffer::width(), framebuffer::height());
    framebuffer::fill_rect(0, TOP, w, h.saturating_sub(TOP), BG);
    unsafe {
        for cell in (&mut *&raw mut GRID).iter_mut() {
            *cell = b' ';
        }
        COL = 0;
        ROW = 0;
        READY = true;
    }
}

fn draw_cell(col: usize, row: usize, c: u8) {
    let x = MARGIN + col as u32 * CELL_W;
    let y = TOP + row as u32 * CELL_H;
    font8x8::draw_char(x, y, c, FG, BG);
}

/// Shift the grid up one line and repaint the whole console from it.
fn scroll() {
    let (nc, nr) = (cols(), rows());
    if nr == 0 {
        return;
    }
    unsafe {
        let grid = &mut *&raw mut GRID;
        for r in 1..nr {
            for c in 0..nc {
                grid[(r - 1) * MAX_COLS + c] = grid[r * MAX_COLS + c];
            }
        }
        for c in 0..nc {
            grid[(nr - 1) * MAX_COLS + c] = b' ';
        }
    }
    let (w, h) = (framebuffer::width(), framebuffer::height());
    framebuffer::fill_rect(0, TOP, w, h.saturating_sub(TOP), BG);
    unsafe {
        let grid = &*&raw const GRID;
        for r in 0..nr {
            for c in 0..nc {
                let ch = grid[r * MAX_COLS + c];
                if ch != b' ' {
                    draw_cell(c, r, ch);
                }
            }
        }
    }
}

pub fn putc(ch: u8) {
    if unsafe { !READY } || !framebuffer::available() {
        return;
    }
    let nc = cols();
    let nr = rows();
    if nc == 0 || nr == 0 {
        return;
    }
    unsafe {
        if ch == b'\n' {
            COL = 0;
            ROW += 1;
        } else if ch == b'\r' {
            COL = 0;
        } else if ch == 0x08 {
            if COL > 0 {
                COL -= 1;
                GRID[ROW * MAX_COLS + COL] = b' ';
                draw_cell(COL, ROW, b' ');
            }
        } else if (0x20..0x7F).contains(&ch) {
            if COL >= nc {
                COL = 0;
                ROW += 1;
            }
            if ROW >= nr {
                scroll();
                ROW = nr - 1;
            }
            GRID[ROW * MAX_COLS + COL] = ch;
            draw_cell(COL, ROW, ch);
            COL += 1;
        }
        if ROW >= nr {
            scroll();
            ROW = nr - 1;
        }
    }
}

pub fn write(bytes: &[u8]) {
    for &b in bytes {
        putc(b);
    }
}
