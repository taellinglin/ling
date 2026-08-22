use crate::arch::io::outb;
use core::ptr;

const VGA_BUF: *mut u8 = 0xB8000 as *mut u8;
const WIDTH: usize = 80;
const HEIGHT: usize = 25;

/// Upload a custom 256-glyph, 8x16, 1bpp bitmap font into the VGA text-mode
/// character generator (plane 2), replacing the BIOS default font. This is
/// the classic "VGA custom font" register sequence (Sequencer + Graphics
/// Controller reprogramming to reach font plane 2, write glyphs, then
/// restore normal text-mode addressing) — see the OSDev wiki's "VGA Fonts"
/// article. `font` is 256 glyphs * 16 rows, one byte per row (MSB = leftmost
/// pixel), i.e. the standard raw PC screen-font glyph layout.
pub unsafe fn load_font(font: &[u8; 4096]) {
    // Enter font-access mode: select plane 2, sequential addressing.
    outb(0x3C4, 0x00); outb(0x3C5, 0x01); // sequencer reset
    outb(0x3C4, 0x02); outb(0x3C5, 0x04); // map mask: plane 2 only
    outb(0x3C4, 0x04); outb(0x3C5, 0x07); // sequential addressing, extended mem
    outb(0x3C4, 0x00); outb(0x3C5, 0x03); // end reset

    outb(0x3CE, 0x04); outb(0x3CF, 0x02); // read map select: plane 2
    outb(0x3CE, 0x05); outb(0x3CF, 0x00); // write mode 0, read mode 0
    outb(0x3CE, 0x06); outb(0x3CF, 0x04); // map at 0xA0000, disable odd/even

    // Glyphs are stored on 32-byte centers even though only 16 bytes (an 8x16
    // cell) are meaningful, matching the VGA font plane's fixed glyph pitch.
    let font_mem = 0xA0000 as *mut u8;
    for glyph in 0..256usize {
        for row in 0..16usize {
            ptr::write_volatile(font_mem.add(glyph * 32 + row), font[glyph * 16 + row]);
        }
    }

    // Restore normal text-mode addressing (planes 0/1, odd/even, 0xB8000).
    outb(0x3C4, 0x00); outb(0x3C5, 0x01);
    outb(0x3C4, 0x02); outb(0x3C5, 0x03); // map mask: planes 0+1
    outb(0x3C4, 0x04); outb(0x3C5, 0x03); // odd/even addressing
    outb(0x3C4, 0x00); outb(0x3C5, 0x03);

    outb(0x3CE, 0x04); outb(0x3CF, 0x00);
    outb(0x3CE, 0x05); outb(0x3CF, 0x10); // odd/even mode
    outb(0x3CE, 0x06); outb(0x3CF, 0x0E); // map at 0xB8000, odd/even enabled
}

/// Registered by the generated per-project `_start` (if a build-time idle
/// font was found and rasterized) so `keyboard::read_char`'s idle detector
/// can swap to it without ling-kernel needing to know about any specific
/// project's font asset.
static mut IDLE_FONT: Option<&'static [u8; 4096]> = None;

pub fn set_idle_font(font: &'static [u8; 4096]) {
    unsafe { IDLE_FONT = Some(font); }
}

/// A snapshot of whatever font was actually active (the BIOS default,
/// almost always — nothing else has loaded a custom one yet) taken the
/// first time `use_idle_font` swaps away from it, so `restore_normal_font`
/// has something real to bring back. There's no way to ask the hardware
/// "what was the original font" after overwriting it, so this has to be
/// captured before that first swap, not reconstructed after.
static mut ORIGINAL_FONT: [u8; 4096] = [0u8; 4096];
static mut ORIGINAL_FONT_SAVED: bool = false;

unsafe fn save_original_font_if_needed() {
    if ptr::read(&raw const ORIGINAL_FONT_SAVED) {
        return;
    }
    // Same font-plane-2 access dance as `load_font`, but reading instead of
    // writing (Graphics Controller "read map select" instead of the
    // Sequencer's write "map mask").
    outb(0x3C4, 0x00); outb(0x3C5, 0x01);
    outb(0x3C4, 0x02); outb(0x3C5, 0x04);
    outb(0x3C4, 0x04); outb(0x3C5, 0x07);
    outb(0x3C4, 0x00); outb(0x3C5, 0x03);

    outb(0x3CE, 0x04); outb(0x3CF, 0x02); // read map select: plane 2
    outb(0x3CE, 0x05); outb(0x3CF, 0x00);
    outb(0x3CE, 0x06); outb(0x3CF, 0x04);

    let font_mem = 0xA0000 as *const u8;
    for glyph in 0..256usize {
        for row in 0..16usize {
            let byte = ptr::read_volatile(font_mem.add(glyph * 32 + row));
            (*&raw mut ORIGINAL_FONT)[glyph * 16 + row] = byte;
        }
    }

    outb(0x3C4, 0x00); outb(0x3C5, 0x01);
    outb(0x3C4, 0x02); outb(0x3C5, 0x03);
    outb(0x3C4, 0x04); outb(0x3C5, 0x03);
    outb(0x3C4, 0x00); outb(0x3C5, 0x03);
    outb(0x3CE, 0x04); outb(0x3CF, 0x00);
    outb(0x3CE, 0x05); outb(0x3CF, 0x10);
    outb(0x3CE, 0x06); outb(0x3CF, 0x0E);

    ORIGINAL_FONT_SAVED = true;
}

/// Swap to the registered idle font, if one was set. No-op otherwise.
/// Snapshots whatever font was active first, so `restore_normal_font` can
/// bring it back later.
pub fn use_idle_font() {
    unsafe {
        save_original_font_if_needed();
        if let Some(font) = IDLE_FONT {
            load_font(font);
        }
    }
}

/// Swap back to the font that was active before `use_idle_font` — called
/// on real keyboard/mouse activity after the idle font fired. No-op if
/// `use_idle_font` was never called (nothing to restore).
pub fn restore_normal_font() {
    unsafe {
        if ptr::read(&raw const ORIGINAL_FONT_SAVED) {
            load_font(&*&raw const ORIGINAL_FONT);
        }
    }
}

/// Disable the hardware text-mode cursor (CRTC "Cursor Start" register,
/// index 0x0A, bit 5). Nothing in this driver ever programs the CRTC
/// cursor-position registers (0x3D4/0x3D5 index 0x0E/0x0F) either, so
/// leaving the hardware cursor enabled means it just sits wherever GRUB/BIOS
/// last left it, blinking, never tracking real writes -- the reported "cursor
/// in a random position" bug. `term.rs` draws its own cursor instead (always
/// derived from the live write position), so the hardware one just needs to
/// get out of the way, not be kept in sync. Call once, at boot.
pub unsafe fn disable_hardware_cursor() {
    outb(0x3D4, 0x0A);
    outb(0x3D5, 0x20);
}

/// Set one VGA DAC palette entry (index 0..=15, matching `Color`'s ordinal
/// values). `r`/`g`/`b` are 6-bit (0..=63) — the VGA DAC's native precision,
/// not the 8-bit values a CSS-style hex color gives you; scale by dividing
/// an 8-bit channel by 4 (see `THEME_LINGOS`'s doc comment).
pub unsafe fn set_palette_entry(index: u8, r: u8, g: u8, b: u8) {
    outb(0x3C8, index);
    outb(0x3C9, r);
    outb(0x3C9, g);
    outb(0x3C9, b);
}

/// A full 16-entry palette, in `Color` ordinal order, each channel 6-bit.
pub type Theme = [(u8, u8, u8); 16];

/// The standard VGA/EGA default palette — every theme starts as a copy of
/// this and overrides only the entries it actually wants to restyle, so
/// colors nobody themed still look like reasonable, conventional colors.
pub const DEFAULT_THEME: Theme = [
    (0, 0, 0),      // 0  Black
    (0, 0, 42),     // 1  Blue
    (0, 42, 0),     // 2  Green
    (0, 42, 42),    // 3  Cyan
    (42, 0, 0),     // 4  Red
    (42, 0, 42),    // 5  Magenta
    (42, 21, 0),    // 6  Brown
    (42, 42, 42),   // 7  LightGrey
    (21, 21, 21),   // 8  DarkGrey
    (21, 21, 63),   // 9  LightBlue
    (21, 63, 21),   // 10 LightGreen
    (21, 63, 63),   // 11 LightCyan
    (63, 21, 21),   // 12 LightRed
    (63, 21, 63),   // 13 LightMagenta
    (63, 63, 21),   // 14 Yellow
    (63, 63, 63),   // 15 White
];

/// LingOS's default theme: a dark indigo background with taupe, navy,
/// red, gold, cyan, and green accents — restyles the palette slots the
/// boot banner and shell actually use (`Black`/bg, `Blue`, `Green`, `Red`,
/// `LightGrey`, `LightCyan`, `Yellow`), leaving the rest at their standard
/// VGA values. 8-bit-RGB-to-6-bit-DAC: divide each channel by 4.
pub const THEME_LINGOS: Theme = {
    let mut t = DEFAULT_THEME;
    t[Color::Black as usize] = (0, 0, 0); // true black background
    t[Color::Blue as usize] = (7, 5, 25); // dark navy-blue accent
    t[Color::Green as usize] = (7, 35, 7); // green accent
    t[Color::Red as usize] = (63, 15, 16); // red/coral accent
    t[Color::LightGrey as usize] = (49, 44, 43); // taupe/mauve accent
    t[Color::LightCyan as usize] = (0, 63, 60); // bright cyan accent
    t[Color::Yellow as usize] = (63, 49, 0); // gold accent
    t
};

/// A light theme: soft-white background with darkened variants of every
/// color slot the boot banner/shell actually use, so switching themes
/// stays readable everywhere those show up, not just the background.
pub const THEME_LIGHT: Theme = {
    let mut t = DEFAULT_THEME;
    t[Color::Black as usize] = (60, 60, 58); // background: soft white
    t[Color::Blue as usize] = (0, 0, 35);
    t[Color::Green as usize] = (0, 30, 0);
    t[Color::Red as usize] = (35, 0, 0);
    t[Color::LightGrey as usize] = (8, 8, 8); // near-black body text
    t[Color::LightCyan as usize] = (0, 28, 28);
    t[Color::Yellow as usize] = (35, 22, 0);
    t[Color::LightRed as usize] = (40, 8, 8);
    t[Color::LightGreen as usize] = (0, 35, 0);
    t[Color::LightBlue as usize] = (5, 5, 40);
    t[Color::LightMagenta as usize] = (30, 0, 30);
    t
};

/// Program every DAC entry from `theme` — the whole point of keeping
/// themes as plain data (`Theme` = `[(u8,u8,u8); 16]`) is that "switch
/// theme" is just "call this again with a different table."
pub fn apply_theme(theme: &Theme) {
    for (i, &(r, g, b)) in theme.iter().enumerate() {
        unsafe { set_palette_entry(i as u8, r, g, b) };
    }
}

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

pub(crate) fn color_byte(fg: u8, bg: u8) -> u8 {
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
