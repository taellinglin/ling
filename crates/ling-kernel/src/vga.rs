use core::ptr;

const VGA_BUF: *mut u8 = 0xB8000 as *mut u8;
const WIDTH: usize = 80;
const HEIGHT: usize = 25;

#[repr(u8)]
#[derive(Copy, Clone)]
pub enum Color {
    Black = 0,
    Blue = 1,
    Green = 2,
    Cyan = 3,
    Red = 4,
    Magenta = 5,
    Brown = 6,
    LightGrey = 7,
    DarkGrey = 8,
    LightBlue = 9,
    LightGreen = 10,
    LightCyan = 11,
    LightRed = 12,
    LightMagenta = 13,
    Yellow = 14,
    White = 15,
}

fn color_byte(fg: u8, bg: u8) -> u8 {
    (bg as u8) << 4 | (fg as u8)
}

static mut FG: u8 = 7;
static mut BG: u8 = 0;
static mut COL: usize = 0;
static mut ROW: usize = 0;

pub fn set_color(fg: Color, bg: Color) {
    unsafe {
        FG = fg as u8;
        BG = bg as u8;
    }
}

pub fn clear() {
    unsafe {
        let color = color_byte(FG, BG);
        for i in 0..(WIDTH * HEIGHT) {
            ptr::write_volatile(VGA_BUF.add(i * 2), b' ');
            ptr::write_volatile(VGA_BUF.add(i * 2 + 1), color);
        }
        COL = 0;
        ROW = 0;
    }
}

pub fn write_char(c: u8) {
    unsafe {
        if c == b'\n' {
            COL = 0;
            ROW += 1;
            if ROW >= HEIGHT {
                scroll();
                ROW = HEIGHT - 1;
            }
            return;
        }
        if c == b'\r' {
            COL = 0;
            return;
        }
        let pos = ROW * WIDTH + COL;
        let color = color_byte(FG, BG);
        if pos < WIDTH * HEIGHT {
            ptr::write_volatile(VGA_BUF.add(pos * 2), c);
            ptr::write_volatile(VGA_BUF.add(pos * 2 + 1), color);
        }
        COL += 1;
        if COL >= WIDTH {
            COL = 0;
            ROW += 1;
            if ROW >= HEIGHT {
                scroll();
                ROW = HEIGHT - 1;
            }
        }
    }
}

pub fn write(s: &[u8]) {
    for &c in s {
        write_char(c);
    }
}

pub fn write_str(s: &str) {
    write(s.as_bytes());
}

fn scroll() {
    unsafe {
        let color = color_byte(FG, BG);
        for row in 1..HEIGHT {
            for col in 0..WIDTH {
                let src = row * WIDTH + col;
                let dst = (row - 1) * WIDTH + col;
                let byte = ptr::read_volatile(VGA_BUF.add(src * 2));
                let attr = ptr::read_volatile(VGA_BUF.add(src * 2 + 1));
                ptr::write_volatile(VGA_BUF.add(dst * 2), byte);
                ptr::write_volatile(VGA_BUF.add(dst * 2 + 1), attr);
            }
        }
        let last_row = (HEIGHT - 1) * WIDTH;
        for col in 0..WIDTH {
            ptr::write_volatile(VGA_BUF.add((last_row + col) * 2), b' ');
            ptr::write_volatile(VGA_BUF.add((last_row + col) * 2 + 1), color);
        }
    }
}
