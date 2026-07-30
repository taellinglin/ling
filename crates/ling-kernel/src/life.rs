//! `ling-life`: a screensaver — Conway's Game of Life on the VGA text grid,
//! styled after a browser demo (LingOS's `packages/tensormap/script.js`):
//! B3/S23 rules, quarter-grid mirror symmetry (so patterns read as a
//! kaleidoscope rather than a plain grid), random ROYGBIV-ish letters for
//! live cells, and a small per-generation chance for any cell to flip
//! regardless of the rules so it keeps moving indefinitely instead of
//! settling into stable/dead patterns the way unmodified Life usually
//! does. Runs until a key is pressed or the mouse moves/clicks.
use crate::{keyboard, mouse, term, vga};

const WIDTH: usize = 80;
const HEIGHT: usize = 25;
const HALF_W: usize = WIDTH.div_ceil(2);
const HALF_H: usize = HEIGHT.div_ceil(2);

/// Red/blue only — matches the reference demo's `randomColor()` exactly
/// (`packages/tensormap/script.js`), not the fuller ROYGBIV set an earlier
/// pass used here.
const COLORS: [u8; 2] = [4, 1];

/// Spin count between generations — there's no calibrated timer in this
/// kernel (see `keyboard.rs`'s idle-threshold doc comment for the same
/// caveat), just a large enough busy-wait to make each generation
/// watchable rather than an instant flicker.
const GEN_PACE: u32 = 4_000_000;

#[derive(Copy, Clone)]
struct Cell {
    alive: bool,
    ch: u8,
    color: u8,
}
const EMPTY_CELL: Cell = Cell { alive: false, ch: b' ', color: 0 };

static mut QUARTER: [[Cell; HALF_W]; HALF_H] = [[EMPTY_CELL; HALF_W]; HALF_H];
static mut RNG_STATE: u32 = 1;

fn seed_rng() {
    unsafe {
        let t = crate::cpu::rdtsc();
        RNG_STATE = (t as u32) | 1;
    }
}

/// xorshift32 — not cryptographic (same caveat as `users.rs`'s password
/// salt), just needs to look random for a screensaver pattern.
fn rand_u32() -> u32 {
    unsafe {
        let mut x = RNG_STATE;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        RNG_STATE = x;
        x
    }
}

fn rand_below(n: u32) -> u32 {
    rand_u32() % n
}

fn rand_char() -> u8 {
    b'a' + rand_below(26) as u8
}

fn rand_color() -> u8 {
    COLORS[rand_below(COLORS.len() as u32) as usize]
}

fn random_cell() -> Cell {
    Cell { alive: rand_below(2) == 1, ch: rand_char(), color: rand_color() }
}

fn init_quarter() {
    unsafe {
        let grid: &mut [[Cell; HALF_W]; HALF_H] = &mut *&raw mut QUARTER;
        for row in grid.iter_mut() {
            for cell in row.iter_mut() {
                *cell = random_cell();
            }
        }
    }
}

/// Mirror a full-grid coordinate back into the quarter grid that actually
/// holds state — the same 4-way symmetry the reference demo builds via
/// `sourceX`/`sourceY`.
fn mirror_source(x: usize, y: usize) -> (usize, usize) {
    let sx = if x < HALF_W { x } else { WIDTH - 1 - x };
    let sy = if y < HALF_H { y } else { HEIGHT - 1 - y };
    (sx.min(HALF_W - 1), sy.min(HALF_H - 1))
}

fn full_cell(x: usize, y: usize) -> Cell {
    let (sx, sy) = mirror_source(x, y);
    unsafe { QUARTER[sy][sx] }
}

fn count_neighbors(x: usize, y: usize) -> u32 {
    let mut count = 0;
    for dy in [HEIGHT - 1, 0, 1] {
        for dx in [WIDTH - 1, 0, 1] {
            if dx == 0 && dy == 0 {
                continue;
            }
            let nx = (x + dx) % WIDTH;
            let ny = (y + dy) % HEIGHT;
            if full_cell(nx, ny).alive {
                count += 1;
            }
        }
    }
    count
}

fn step() {
    let mut next = unsafe { QUARTER };
    for y in 0..HALF_H {
        for x in 0..HALF_W {
            let cell = unsafe { QUARTER[y][x] };
            let neighbors = count_neighbors(x, y);
            let mut new_cell = cell;
            if cell.alive {
                if !(2..=3).contains(&neighbors) {
                    new_cell.alive = false;
                }
            } else if neighbors == 3 {
                new_cell.alive = true;
                new_cell.ch = rand_char();
                new_cell.color = rand_color();
            }
            // Perpetual nudge: keeps the board alive forever instead of
            // settling into a static/empty pattern like plain Life would.
            if rand_below(2000) == 0 {
                new_cell.alive = !new_cell.alive;
            }
            next[y][x] = new_cell;
        }
    }
    unsafe { QUARTER = next };
}

fn draw() {
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let cell = full_cell(x, y);
            if cell.alive {
                term::set_cell_active(y, x, cell.ch, vga::color_byte(cell.color, 0));
            } else {
                term::set_cell_active(y, x, b' ', vga::color_byte(0, 0));
            }
        }
    }
    term::blit_active();
}

/// Run the screensaver until a key is pressed or the mouse moves/clicks;
/// clears the screen on the way out so the shell's next prompt starts
/// clean.
pub fn run() {
    seed_rng();
    init_quarter();
    // Always the idle font (font/square.otf, whatever the build registered
    // as the idle font — see vga.rs's set_idle_font/use_idle_font), not
    // whichever font happened to be active when the command was typed.
    vga::use_idle_font();

    let start_x = mouse::x();
    let start_y = mouse::y();
    let start_buttons = mouse::buttons();

    loop {
        step();
        draw();
        for _ in 0..GEN_PACE {
            unsafe { crate::cpu::pause() };
        }

        let key = keyboard::poll_char();
        mouse::poll();
        if key != 0
            || mouse::x() != start_x
            || mouse::y() != start_y
            || mouse::buttons() != start_buttons
        {
            break;
        }
    }

    vga::restore_normal_font();
    term::clear_active();
}
