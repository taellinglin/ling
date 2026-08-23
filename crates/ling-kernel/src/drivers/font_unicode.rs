//! Multi-script Unicode glyph rendering for the framebuffer, on top of a
//! build-time-rasterized bitmap atlas (see `ling`'s `gen_unicode_font_rs` in
//! `src/main.rs`, same pattern as `idle_font.rs`'s VGA idle-font atlas: real
//! font files rasterized once via `fontdue` at `ling build` time, since
//! there's no font-shaping engine in a no_std kernel to do it at runtime).
//!
//! Deliberately NOT full Unicode coverage — that would mean embedding tens
//! of thousands of CJK glyphs (megabytes) into a hobby kernel image. Instead
//! this covers a curated character set (see the generator's `UNICODE_CHARS_*`
//! constants) big enough for a handful of real UI strings — language names,
//! greetings — per script. A codepoint outside the curated set renders
//! blank, same fallback discipline as `font8x8`.
//!
//! Also deliberately NOT proper text shaping: Thai's combining vowel/tone
//! marks are rasterized as independent glyphs at their own advance width,
//! not composed onto the preceding consonant the way a real Thai text
//! renderer (which needs the font's GSUB/GPOS tables, well beyond what
//! `fontdue` or this module do) would. Readable, not typographically
//! correct — disclosed here rather than silently claimed as more than it is.
//!
//! ASCII stays on `font8x8` (8px advance); everything this module covers
//! renders at a 16x16 cell (16px advance) — CJK/Hangul/Thai glyphs need the
//! extra resolution to stay legible.
use crate::drivers::framebuffer;

pub type Glyph16 = [u8; 32]; // 16 rows * 2 bytes/row (16 columns), 1bpp

static mut UNICODE_ATLAS: Option<&'static [(u32, Glyph16)]> = None;
static mut DAEMON_ATLAS: Option<&'static [(u32, Glyph16)]> = None;

/// Register the build-time-baked CJK/Hangul/Thai atlas — called once from
/// generated kernel `main.rs`, same handoff as `vga::set_idle_font`.
pub fn set_unicode_atlas(atlas: &'static [(u32, Glyph16)]) {
    unsafe { UNICODE_ATLAS = Some(atlas) };
}

/// Register the Daemon (constructed-script) atlas — keyed by the *same*
/// codepoints as ASCII (it's a glyph reskin of Latin text, not a new
/// character repertoire), so it's a separate table rather than merged into
/// `UNICODE_ATLAS` where it would collide with real ASCII.
pub fn set_daemon_atlas(atlas: &'static [(u32, Glyph16)]) {
    unsafe { DAEMON_ATLAS = Some(atlas) };
}

fn find(atlas: Option<&'static [(u32, Glyph16)]>, cp: u32) -> Option<&'static Glyph16> {
    atlas?.iter().find(|(c, _)| *c == cp).map(|(_, g)| g)
}

/// Decode one UTF-8 codepoint starting at `bytes[i]`. Returns
/// `(codepoint, byte_len)`; a malformed leading byte decodes as `('?', 1)`
/// so a bad byte skips forward instead of stalling the caller's loop.
fn decode_utf8_at(bytes: &[u8], i: usize) -> (u32, usize) {
    let b0 = bytes[i];
    let (len, mut cp) = if b0 < 0x80 {
        return (b0 as u32, 1);
    } else if b0 & 0xE0 == 0xC0 {
        (2usize, (b0 & 0x1F) as u32)
    } else if b0 & 0xF0 == 0xE0 {
        (3usize, (b0 & 0x0F) as u32)
    } else if b0 & 0xF8 == 0xF0 {
        (4usize, (b0 & 0x07) as u32)
    } else {
        return (b'?' as u32, 1);
    };
    if i + len > bytes.len() {
        return (b'?' as u32, 1);
    }
    for k in 1..len {
        let b = bytes[i + k];
        if b & 0xC0 != 0x80 {
            return (b'?' as u32, 1);
        }
        cp = (cp << 6) | (b & 0x3F) as u32;
    }
    (cp, len)
}

fn draw_glyph16(x: u32, y: u32, g: &Glyph16, fg: u32, bg: u32) {
    for row in 0..16u32 {
        let word = ((g[row as usize * 2] as u16) << 8) | g[row as usize * 2 + 1] as u16;
        for col in 0..16u32 {
            let set = (word >> (15 - col)) & 1 != 0;
            framebuffer::back_set_pixel(x + col, y + row, if set { fg } else { bg });
        }
    }
}

/// Draw a UTF-8 byte string left-to-right, no wrapping/shaping (a
/// primitive, same discipline as `font8x8::draw_str`). ASCII bytes render
/// via `font8x8` at 8px advance; anything else looks up `daemon` ?
/// `DAEMON_ATLAS` : `UNICODE_ATLAS` at 16px advance, blank if uncovered.
/// Returns the total pixel width drawn, so a caller can lay out what comes
/// next (e.g. center text, or place a flag glyph after it).
pub fn draw_utf8_str(x: u32, y: u32, bytes: &[u8], fg: u32, bg: u32, daemon: bool) -> u32 {
    let mut cursor = 0u32;
    let mut i = 0usize;
    while i < bytes.len() {
        let (cp, len) = decode_utf8_at(bytes, i);
        i += len;
        if cp < 0x80 && !daemon {
            super::font8x8::draw_char(x + cursor, y, cp as u8, fg, bg);
            cursor += 8;
            continue;
        }
        let atlas = if daemon {
            unsafe { DAEMON_ATLAS }
        } else {
            unsafe { UNICODE_ATLAS }
        };
        if let Some(g) = find(atlas, cp) {
            draw_glyph16(x + cursor, y, g, fg, bg);
        } else {
            framebuffer::back_fill_rect(x + cursor, y, 16, 16, bg);
        }
        cursor += 16;
    }
    cursor
}
