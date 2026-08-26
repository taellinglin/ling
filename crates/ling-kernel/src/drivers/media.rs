//! Media player: plays real WAV audio from lingfs through the mixer's PCM
//! source. Opened when a `.wav` is picked in Files. Parses the RIFF
//! container properly (walks chunks to find `fmt `/`data` -- real WAVs
//! carry LIST/INFO chunks, so the data isn't at a fixed offset), supports
//! u8 and s16le, mono/stereo, and drives the mixer's resampling PCM path.
//!
//! The bundled `music/Pelipo.wav` is seeded into lingfs at mount (see
//! `seed_pelipo`) from a baked, downsampled clip -- honestly a 45s /
//! 22 kHz / 8-bit mono reduction of the 48 MB master, because baking the
//! full-resolution file into the kernel image is impractical. The player
//! itself plays any WAV lingfs holds at full quality the format allows.

use crate::drivers::{font8x8, framebuffer, mixer, theme};
use crate::fs::lingfs;

/// The baked clip -- only linked into graphics kernels (they're the ones
/// with a Media window and a framebuffer). Keeps the text/installer/rpi
/// kernels lean.
#[cfg(feature = "request_framebuffer")]
pub static PELIPO_WAV: &[u8] = include_bytes!("../../assets/pelipo.wav");
#[cfg(not(feature = "request_framebuffer"))]
pub static PELIPO_WAV: &[u8] = &[];

/// The Ling Country anthem -- same baking rationale as Pelipo (a
/// downsampled clip of the 36MB master).
#[cfg(feature = "request_framebuffer")]
pub static ANTHEM_WAV: &[u8] = include_bytes!("../../assets/anthem.wav");
#[cfg(not(feature = "request_framebuffer"))]
pub static ANTHEM_WAV: &[u8] = &[];

const BUF_MAX: usize = 4 * 1024 * 1024; // room for a few minutes of 8-bit mono
static mut MEDIA_BUF: [u8; BUF_MAX] = [0; BUF_MAX];
static mut LOADED: bool = false;
static mut NAME: [u8; 64] = [0; 64];
static mut NAME_LEN: usize = 0;
static mut STATUS: &'static str = "no file loaded";

fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

/// Parse a WAV image already in `buf[..len]`, returning
/// (data_offset, data_len, rate, channels, bits). Walks RIFF chunks.
fn parse_wav(buf: &[u8], len: usize) -> Option<(usize, usize, u32, u8, u8)> {
    if len < 12 || &buf[0..4] != b"RIFF" || &buf[8..12] != b"WAVE" {
        return None;
    }
    let mut off = 12;
    let mut rate = 0u32;
    let mut ch = 0u8;
    let mut bits = 0u8;
    let mut have_fmt = false;
    while off + 8 <= len {
        let id = &buf[off..off + 4];
        let sz = u32le(buf, off + 4) as usize;
        let body = off + 8;
        if id == b"fmt " && body + 16 <= len {
            ch = u16le(buf, body + 2) as u8;
            rate = u32le(buf, body + 4);
            bits = u16le(buf, body + 14) as u8;
            have_fmt = true;
        } else if id == b"data" {
            let dlen = sz.min(len - body);
            if have_fmt {
                return Some((body, dlen, rate, ch, bits));
            }
        }
        off = body + sz + (sz & 1); // chunks are word-aligned
    }
    None
}

fn name() -> &'static str {
    unsafe { core::str::from_utf8(&(&*&raw const NAME)[..NAME_LEN]).unwrap_or("") }
}

pub fn status() -> &'static str {
    unsafe { STATUS }
}

/// Load a WAV from lingfs and hand its PCM to the mixer (stopped). Returns
/// false if it isn't a WAV we can parse.
pub fn open(path: &str) -> bool {
    let buf = unsafe { &mut *&raw mut MEDIA_BUF };
    let len = match lingfs::read_file_all(path, buf) {
        Ok(Some(n)) => n,
        _ => {
            unsafe { STATUS = "could not read file" };
            return false;
        },
    };
    let Some((doff, dlen, rate, ch, bits)) = parse_wav(buf, len) else {
        unsafe { STATUS = "not a supported WAV (need PCM u8/s16)" };
        return false;
    };
    let frame_bytes = (ch.max(1) as usize) * (bits.max(8) as usize / 8);
    let frames = (dlen / frame_bytes.max(1)) as u64;
    unsafe {
        let n = path.len().min(64);
        let nm = &mut *&raw mut NAME;
        nm[..n].copy_from_slice(&path.as_bytes()[..n]);
        NAME_LEN = n;
        // Pointer into MEDIA_BUF at the data chunk -- stable static.
        mixer::pcm_load(PCM_data_ptr(doff), frames, rate, ch, bits);
        LOADED = true;
        STATUS = "loaded -- space to play";
    }
    true
}

#[allow(non_snake_case)]
unsafe fn PCM_data_ptr(off: usize) -> *const u8 {
    (&*&raw const MEDIA_BUF).as_ptr().add(off)
}

pub fn key(k: u8) {
    unsafe {
        if !LOADED {
            return;
        }
        match k {
            b' ' => {
                match mixer::pcm_state() {
                    mixer::PCM_STOPPED => {
                        mixer::pcm_play();
                        STATUS = "playing";
                    },
                    mixer::PCM_PLAYING => {
                        mixer::pcm_pause();
                        STATUS = "paused";
                    },
                    _ => {
                        mixer::pcm_pause();
                        STATUS = "playing";
                    },
                }
            },
            b's' | b'S' => {
                mixer::pcm_stop();
                STATUS = "stopped";
            },
            _ => {},
        }
    }
}

fn fmt_time(ms: u64, out: &mut [u8]) -> usize {
    let total = ms / 1000;
    let (m, s) = (total / 60, total % 60);
    out[0] = b'0' + ((m / 10) % 10) as u8;
    out[1] = b'0' + (m % 10) as u8;
    out[2] = b':';
    out[3] = b'0' + (s / 10) as u8;
    out[4] = b'0' + (s % 10) as u8;
    5
}

pub fn draw(x: u32, y: u32, w: u32, _h: u32) {
    let panel = theme::color(theme::SLOT_PANEL);
    let text = theme::color(theme::SLOT_TEXT);
    let dim = theme::color(theme::SLOT_DIM);
    let accent = theme::color(theme::SLOT_ACCENT);
    font8x8::draw_str(x, y, b"Media Player", accent, panel);
    if !unsafe { LOADED } {
        font8x8::draw_str(x, y + 22, b"open a .wav from Files to play it", dim, panel);
        return;
    }
    font8x8::draw_str(x, y + 22, name().as_bytes(), text, panel);
    font8x8::draw_str(x, y + 40, status().as_bytes(), dim, panel);

    // Transport bar.
    let pos = mixer::pcm_pos_ms();
    let len = mixer::pcm_len_ms().max(1);
    let bar_y = y + 70;
    let bar_w = w.saturating_sub(20);
    framebuffer::back_fill_rounded_rect(x, bar_y, bar_w, 8, 4, theme::color(theme::SLOT_DOT_DIM));
    let fill = (pos * bar_w as u64 / len).min(bar_w as u64) as u32;
    framebuffer::back_fill_rounded_rect(x, bar_y, fill.max(1), 8, 4, accent);

    // A big round play/pause indicator.
    let st = mixer::pcm_state();
    let cx = x + 30;
    let cy = y + 110;
    framebuffer::back_fill_circle(cx, cy, 18, theme::color(theme::SLOT_PANEL_BORDER));
    framebuffer::back_fill_circle(cx, cy, 16, panel);
    if st == mixer::PCM_PLAYING {
        // pause bars
        framebuffer::back_fill_rect(cx - 6, cy - 8, 4, 16, accent);
        framebuffer::back_fill_rect(cx + 2, cy - 8, 4, 16, accent);
    } else {
        // play triangle (approximate with stacked rects)
        for i in 0..12u32 {
            framebuffer::back_fill_rect(cx - 5 + i / 2, cy - 6 + i / 2, 2, 12 - i, accent);
        }
    }

    // Times.
    let mut tb = [0u8; 16];
    let mut n = fmt_time(pos, &mut tb);
    tb[n] = b' '; tb[n + 1] = b'/'; tb[n + 2] = b' '; n += 3;
    n += fmt_time(mixer::pcm_len_ms(), &mut tb[n..]);
    font8x8::draw_str(x + 70, cy - 4, &tb[..n], text, panel);
    font8x8::draw_str(x, y + 150, b"space: play/pause    s: stop", dim, panel);
}

static mut SEED_STEP: u32 = 0;

/// Seed the bundled songs into `music/`, at most ONE file per call, so no
/// single desktop frame blocks for the whole ~1.5MiB library write (that
/// froze the UI badly enough to drop input). Call every frame until it
/// returns true; each writing frame hitches once (a few hundred KiB), then
/// the desktop is responsive again. Skips files already present (installed
/// disks persist them). MUST use write_in_dir_any -- the clips are far
/// past the single-block write_in_dir cap. Returns true when done.
pub fn seed_pelipo() -> bool {
    unsafe {
        match SEED_STEP {
            0 => {
                if !PELIPO_WAV.is_empty() && !music_has("Pelipo.wav") {
                    let _ = lingfs::write_in_dir_any("music", "Pelipo.wav", PELIPO_WAV);
                }
                SEED_STEP = 1;
                false
            },
            1 => {
                if !ANTHEM_WAV.is_empty() && !music_has("Anthem.wav") {
                    let _ = lingfs::write_in_dir_any("music", "Anthem.wav", ANTHEM_WAV);
                }
                SEED_STEP = 2;
                true
            },
            _ => true,
        }
    }
}

fn music_has(fname: &str) -> bool {
    let mut nm = [0u8; 64];
    let mut i = 0;
    while let Some((len, _)) = lingfs::list_entry("music", i, &mut nm) {
        if &nm[..len] == fname.as_bytes() {
            return true;
        }
        i += 1;
    }
    false
}
