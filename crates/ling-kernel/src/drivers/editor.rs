//! A small VSCode-flavored text editor for the desktop's Edit window:
//! a real editable buffer with a cursor, a selection, cut/copy/paste
//! through the system clipboard, select-all, load/save to lingfs, integer
//! zoom, and Ling syntax highlighting (keywords/types/constants/strings/
//! comments/numbers), reusing the same word classes as the `ling` VS Code
//! extension (`editors/vscode-ling/src/lingdata.js`).
//!
//! Buffer model, kept deliberately simple: one flat byte array with a
//! cursor offset and a selection anchor. Insert/delete memmove within the
//! array -- fine at this file-size scale (kilobytes), no gap buffer or
//! rope needed, and honest about that. Newlines are ordinary bytes;
//! line/column are derived, not stored.

use crate::drivers::{clipboard, font8x8, framebuffer, theme};
use crate::fs::lingfs;

const BUF_MAX: usize = 32 * 1024;
static mut BUF: [u8; BUF_MAX] = [0; BUF_MAX];
static mut LEN: usize = 0;
static mut CURSOR: usize = 0;
static mut ANCHOR: usize = 0; // == CURSOR when there's no selection
static mut SCROLL_LINE: usize = 0;
static mut ZOOM: u32 = 1; // integer glyph scale (1 or 2)
static mut DIRTY: bool = false;
const NAME_MAX: usize = 64;
static mut NAME: [u8; NAME_MAX] = [0; NAME_MAX];
static mut NAME_LEN: usize = 0;
static mut STATUS: [u8; 64] = [0; 64];
static mut STATUS_LEN: usize = 0;

fn buf() -> &'static mut [u8; BUF_MAX] {
    unsafe { &mut *&raw mut BUF }
}

fn set_status(s: &[u8]) {
    unsafe {
        let n = s.len().min(64);
        let st = &mut *&raw mut STATUS;
        st[..n].copy_from_slice(&s[..n]);
        STATUS_LEN = n;
    }
}

pub fn zoom() -> u32 {
    unsafe { ZOOM }
}
pub fn zoom_in() {
    unsafe { ZOOM = (ZOOM + 1).min(3) };
}
pub fn zoom_out() {
    unsafe { ZOOM = (ZOOM - 1).max(1) };
}
pub fn zoom_reset() {
    unsafe { ZOOM = 1 };
}

fn name() -> &'static str {
    unsafe { core::str::from_utf8(&(&*&raw const NAME)[..NAME_LEN]).unwrap_or("") }
}

/// Load a file into the editor (or start an empty buffer named `n` if it
/// doesn't exist -- "new file" semantics).
pub fn load(n: &str) {
    unsafe {
        let nn = n.len().min(NAME_MAX);
        let nm = &mut *&raw mut NAME;
        nm[..nn].copy_from_slice(&n.as_bytes()[..nn]);
        NAME_LEN = nn;
        let b = buf();
        match lingfs::read_file_all(n, b) {
            Ok(Some(len)) => {
                LEN = len.min(BUF_MAX);
                set_status(b"loaded");
            },
            _ => {
                LEN = 0;
                set_status(b"new file");
            },
        }
        CURSOR = 0;
        ANCHOR = 0;
        SCROLL_LINE = 0;
        DIRTY = false;
    }
}

pub fn save() {
    unsafe {
        if NAME_LEN == 0 {
            set_status(b"no filename");
            return;
        }
        let b = &(&*&raw const BUF)[..LEN];
        if lingfs::write_file_any(name(), b).is_ok() {
            DIRTY = false;
            set_status(b"saved");
        } else {
            set_status(b"save failed");
        }
    }
}

fn sel_range() -> (usize, usize) {
    unsafe {
        if CURSOR <= ANCHOR {
            (CURSOR, ANCHOR)
        } else {
            (ANCHOR, CURSOR)
        }
    }
}

fn has_selection() -> bool {
    unsafe { CURSOR != ANCHOR }
}

fn delete_range(lo: usize, hi: usize) {
    unsafe {
        if hi <= lo || hi > LEN {
            return;
        }
        let b = buf();
        b.copy_within(hi..LEN, lo);
        LEN -= hi - lo;
        CURSOR = lo;
        ANCHOR = lo;
        DIRTY = true;
    }
}

fn insert_bytes(bytes: &[u8]) {
    unsafe {
        if has_selection() {
            let (lo, hi) = sel_range();
            delete_range(lo, hi);
        }
        let n = bytes.len();
        if LEN + n > BUF_MAX {
            set_status(b"buffer full");
            return;
        }
        let b = buf();
        b.copy_within(CURSOR..LEN, CURSOR + n);
        b[CURSOR..CURSOR + n].copy_from_slice(bytes);
        LEN += n;
        CURSOR += n;
        ANCHOR = CURSOR;
        DIRTY = true;
    }
}

fn line_start(mut off: usize) -> usize {
    let b = buf();
    while off > 0 && b[off - 1] != b'\n' {
        off -= 1;
    }
    off
}

fn line_end(mut off: usize) -> usize {
    let b = buf();
    let len = unsafe { LEN };
    while off < len && b[off] != b'\n' {
        off += 1;
    }
    off
}

/// Route one key (raw byte / arrow / Ctrl-chord code from the keyboard
/// driver) into the editor. `shift` marks a shift-held arrow for
/// selection extension.
pub fn key(k: u8, shift: bool) {
    use crate::drivers::keyboard as kb;
    unsafe {
        match k {
            kb::CTRL_S => save(),
            kb::CTRL_A => {
                ANCHOR = 0;
                CURSOR = LEN;
            },
            kb::CTRL_C => {
                if has_selection() {
                    let (lo, hi) = sel_range();
                    clipboard::set(&(&*&raw const BUF)[lo..hi]);
                    set_status(b"copied");
                }
            },
            kb::CTRL_X => {
                if has_selection() {
                    let (lo, hi) = sel_range();
                    clipboard::set(&(&*&raw const BUF)[lo..hi]);
                    delete_range(lo, hi);
                    set_status(b"cut");
                }
            },
            kb::CTRL_V => {
                // clipboard (CLIP static) and the edit buffer (BUF static)
                // don't alias, so insert directly -- no stack temp.
                let clip = clipboard::get();
                if !clip.is_empty() {
                    insert_bytes(clip);
                    set_status(b"pasted");
                }
            },
            kb::CTRL_ZOOM_IN => zoom_in(),
            kb::CTRL_ZOOM_OUT => zoom_out(),
            kb::CTRL_ZOOM_RESET => zoom_reset(),
            kb::LEFT_ARROW => {
                if CURSOR > 0 {
                    CURSOR -= 1;
                }
                if !shift {
                    ANCHOR = CURSOR;
                }
            },
            kb::RIGHT_ARROW => {
                if CURSOR < LEN {
                    CURSOR += 1;
                }
                if !shift {
                    ANCHOR = CURSOR;
                }
            },
            kb::UP_ARROW => {
                let col = CURSOR - line_start(CURSOR);
                let ls = line_start(CURSOR);
                if ls > 0 {
                    let prev_start = line_start(ls - 1);
                    let prev_end = ls - 1;
                    CURSOR = (prev_start + col).min(prev_end);
                }
                if !shift {
                    ANCHOR = CURSOR;
                }
            },
            kb::DOWN_ARROW => {
                let col = CURSOR - line_start(CURSOR);
                let le = line_end(CURSOR);
                if le < LEN {
                    let next_start = le + 1;
                    let next_end = line_end(next_start);
                    CURSOR = (next_start + col).min(next_end);
                }
                if !shift {
                    ANCHOR = CURSOR;
                }
            },
            0x08 => {
                // Backspace: delete selection, else the char before cursor.
                if has_selection() {
                    let (lo, hi) = sel_range();
                    delete_range(lo, hi);
                } else if CURSOR > 0 {
                    delete_range(CURSOR - 1, CURSOR);
                }
            },
            b'\n' | b'\r' => insert_bytes(b"\n"),
            b'\t' => insert_bytes(b"    "), // soft tabs, 4 spaces
            0x20..=0x7E => insert_bytes(&[k]),
            _ => {},
        }
    }
}

// -- Ling syntax classes (mirrors the vscode extension's lingdata) -----------

fn is_keyword(w: &[u8]) -> bool {
    matches!(
        w,
        b"bind" | b"do" | b"fn" | b"mod" | b"type" | b"use" | b"if" | b"else" | b"while"
            | b"for" | b"in" | b"match" | b"return" | b"post" | b"give" | b"again" | b"stop"
            | b"own" | b"lend" | b"share" | b"move" | b"copy" | b"async" | b"wait" | b"spawn"
            | b"as" | b"where" | b"fit" | b"form" | b"choose" | b"can" | b"change" | b"try"
            | b"sure" | b"maybe" | b"pure" | b"start" | b"result"
    )
}

fn is_type(w: &[u8]) -> bool {
    matches!(w, b"number" | b"text" | b"bool" | b"list" | b"map" | b"tuple")
}

fn is_const(w: &[u8]) -> bool {
    matches!(w, b"true" | b"false" | b"ok" | b"bad" | b"none")
}

fn word_color(w: &[u8]) -> Option<u32> {
    if is_keyword(w) {
        Some(0xC58AE0) // purple-ish keyword
    } else if is_type(w) {
        Some(0x4AC0E0) // cyan type
    } else if is_const(w) {
        Some(0xE0A44A) // amber constant
    } else {
        None
    }
}

// -- Rendering ---------------------------------------------------------------

pub fn status_line() -> &'static str {
    unsafe { core::str::from_utf8(&(&*&raw const STATUS)[..STATUS_LEN]).unwrap_or("") }
}

/// Draw the editor into a window content rect. Renders visible lines with
/// per-token Ling highlighting, the selection band, and a cursor caret.
/// Glyphs are integer-scaled by ZOOM via `font8x8::draw_char_scaled`.
pub fn draw(x: u32, y: u32, w: u32, h: u32) {
    let panel = theme::color(theme::SLOT_PANEL);
    let text = theme::color(theme::SLOT_TEXT);
    let dim = theme::color(theme::SLOT_DIM);
    let accent = theme::color(theme::SLOT_ACCENT);
    let z = unsafe { ZOOM };
    let gw = 8 * z;
    let gh = 8 * z + 2 * z;

    // Header: name + dirty marker + status + zoom.
    let mut hdr = [0u8; 96];
    let mut n = 0;
    for &b in name().as_bytes() {
        if n < hdr.len() {
            hdr[n] = b;
            n += 1;
        }
    }
    if unsafe { DIRTY } && n + 1 < hdr.len() {
        hdr[n] = b'*';
        n += 1;
    }
    font8x8::draw_str(x, y, &hdr[..n], accent, panel);
    font8x8::draw_str(x + 260, y, status_line().as_bytes(), dim, panel);

    let text_top = y + 18;
    let rows = (h.saturating_sub(24) / gh) as usize;
    let (sel_lo, sel_hi) = sel_range();
    let b = buf();
    let len = unsafe { LEN };
    let scroll = unsafe { SCROLL_LINE };
    let cursor = unsafe { CURSOR };

    // Walk the buffer line by line, drawing only visible ones.
    let mut off = 0usize;
    let mut line = 0usize;
    while off <= len && line < scroll + rows {
        let le = line_end(off);
        if line >= scroll {
            let ry = text_top + (line - scroll) as u32 * gh;
            draw_line(x, ry, gw, gh, b, off, le, sel_lo, sel_hi, cursor, text, accent, panel);
        }
        off = le + 1;
        line += 1;
        if le >= len {
            break;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_line(
    x: u32,
    ry: u32,
    gw: u32,
    gh: u32,
    b: &[u8],
    start: usize,
    end: usize,
    sel_lo: usize,
    sel_hi: usize,
    cursor: usize,
    text: u32,
    accent: u32,
    panel: u32,
) {
    let z = gw / 8;
    // Selection band for this line.
    if sel_hi > sel_lo {
        let a = sel_lo.max(start);
        let e = sel_hi.min(end + 1).min(end.max(start) + 1);
        if e > a && a >= start {
            let sx = x + (a - start) as u32 * gw;
            let sw = (e - a) as u32 * gw;
            framebuffer::back_fill_rect(sx, ry, sw, gh, theme::color(theme::SLOT_PANEL_BORDER));
        }
    }
    // Tokenized draw: identifier runs get a class color, strings/comments/
    // numbers their own, everything else plain text.
    let mut i = start;
    let mut col = 0u32;
    while i < end {
        let c = b[i];
        // Line comment `//` to end of line.
        if c == b'/' && i + 1 < end && b[i + 1] == b'/' {
            while i < end {
                font8x8::draw_char_scaled(x + col * gw, ry, b[i], theme::color(theme::SLOT_DIM), panel, z);
                col += 1;
                i += 1;
            }
            break;
        }
        // String literal.
        if c == b'"' {
            let scol = 0x6AC06A;
            font8x8::draw_char_scaled(x + col * gw, ry, c, scol, panel, z);
            col += 1;
            i += 1;
            while i < end {
                let ch = b[i];
                font8x8::draw_char_scaled(x + col * gw, ry, ch, scol, panel, z);
                col += 1;
                i += 1;
                if ch == b'"' {
                    break;
                }
            }
            continue;
        }
        // Identifier / keyword run.
        if c.is_ascii_alphabetic() || c == b'_' {
            let wstart = i;
            while i < end && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let word = &b[wstart..i];
            let color = word_color(word).unwrap_or(text);
            for (k, &ch) in word.iter().enumerate() {
                font8x8::draw_char_scaled(x + (col + k as u32) * gw, ry, ch, color, panel, z);
            }
            col += word.len() as u32;
            continue;
        }
        // Number.
        if c.is_ascii_digit() {
            let ncol = 0xD0B060;
            while i < end && (b[i].is_ascii_alphanumeric() || b[i] == b'.') {
                font8x8::draw_char_scaled(x + col * gw, ry, b[i], ncol, panel, z);
                col += 1;
                i += 1;
            }
            continue;
        }
        font8x8::draw_char_scaled(x + col * gw, ry, c, text, panel, z);
        col += 1;
        i += 1;
    }
    // Caret.
    if cursor >= start && cursor <= end {
        let cx = x + (cursor - start) as u32 * gw;
        framebuffer::back_fill_rect(cx, ry, (gw / 8).max(1) * 2, gh, accent);
    }
}
