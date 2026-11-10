//! Host-runtime builtins that desktop apps (Ling Arcade) call and the LingOS app
//! runtime ([`crate::lingapp`]) didn't have yet: `grad_rect`, the bitmap-font
//! `font_text_px` / `font_width_px` pair, binary file I/O (`read_file_bytes` /
//! `write_file_bytes`), `list_dir` / `path_name`, a few string/maths helpers,
//! the host's named-button gamepad API (`pad_poll`, `pad_pressed(i, "a")`,
//! `pad_lx` ...), the `audio_tone` synth voices, and `emu_*` answering "no native
//! emulator cores here". Semantics follow `ling/src/runtime/mod.rs`.
//!
//! Reached from the tail of [`crate::lingapp::dispatch`], so anything the app
//! runtime already answers keeps its own meaning.

use alloc::string::String;
use alloc::vec::Vec;

use crate::lingrt::{boolean, is_str, list_from, list_items, mk_str, num, to_f64, to_string, UNIT};

fn f(args: &[u64], i: usize, d: f64) -> f64 {
    args.get(i).map(|&v| to_f64(v)).unwrap_or(d)
}

fn s(args: &[u64], i: usize) -> String {
    args.get(i).map(|&v| to_string(v)).unwrap_or_default()
}

fn path(args: &[u64], i: usize) -> String {
    s(args, i).replace('\\', "/")
}

// ── gamepad: the host's named buttons over the USB pad's bit mask ───────────

/// Buttons as of the last `pad_poll` and the poll before it (edge detection).
static mut PAD_NOW: u32 = 0;
static mut PAD_PREV: u32 = 0;

fn pad() -> crate::drivers::hidpad::PadState {
    #[cfg(target_arch = "aarch64")]
    return crate::arch::xhci::pad_state();
    #[cfg(not(target_arch = "aarch64"))]
    crate::drivers::hidpad::PadState::default()
}

/// The pad's buttons as a mask, with the hat folded onto the d-pad bits 12..15
/// (some HID pads only report the hat) and the triggers as bits 16 / 17.
fn pad_mask(p: &crate::drivers::hidpad::PadState) -> u32 {
    if !p.connected {
        return 0;
    }
    let mut m = p.buttons;
    if p.hat >= 0 {
        let h = p.hat;
        if h == 7 || h == 0 || h == 1 {
            m |= 1 << 12;
        }
        if (3..=5).contains(&h) {
            m |= 1 << 13;
        }
        if (5..=7).contains(&h) {
            m |= 1 << 14;
        }
        if (1..=3).contains(&h) {
            m |= 1 << 15;
        }
    }
    if p.axes[4] > 0.5 {
        m |= 1 << 16;
    }
    if p.axes[5] > 0.5 {
        m |= 1 << 17;
    }
    m
}

/// Host button name -> bit (A B X Y LB RB Back Start LS RS Guide, d-pad 12..15).
pub(crate) fn pad_bit(name: &str) -> Option<u32> {
    Some(match name.to_ascii_lowercase().as_str() {
        "a" | "south" | "cross" => 0,
        "b" | "east" | "circle" => 1,
        "x" | "west" | "square" => 2,
        "y" | "north" | "triangle" => 3,
        "lb" | "l1" | "left_shoulder" => 4,
        "rb" | "r1" | "right_shoulder" => 5,
        "select" | "back" | "share" | "view" => 6,
        "start" | "menu" | "options" => 7,
        "l3" | "left_stick" => 8,
        "r3" | "right_stick" => 9,
        "guide" | "home" => 10,
        "up" | "dpad_up" => 12,
        "down" | "dpad_down" => 13,
        "left" | "dpad_left" => 14,
        "right" | "dpad_right" => 15,
        "lt" | "l2" | "left_trigger" => 16,
        "rt" | "r2" | "right_trigger" => 17,
        _ => return None,
    })
}

/// `pad_button(i, "a")` (host form) — also used by lingapp's `pad_button`.
pub(crate) fn pad_button_named(name: &str) -> bool {
    let p = pad();
    pad_bit(name).is_some_and(|b| (pad_mask(&p) >> b) & 1 == 1)
}

// ── drawing ──────────────────────────────────────────────────────────────────

/// `grad_rect(x, y, w, h, r0, g0, b0, r1, g1, b1, dir)`: dir 0 = left→right,
/// otherwise top→bottom. Equal end colours are a plain solid fill (the arcade
/// draws the NES picture that way, one span per run of a colour).
fn grad_rect(args: &[u64]) {
    let Some((buf, bw, bh)) = crate::lingapp::surface() else { return };
    let (x, y, w, h) = (f(args, 0, 0.0), f(args, 1, 0.0), f(args, 2, 0.0), f(args, 3, 0.0));
    let c = |i: usize, d: f64| f(args, i, d).clamp(0.0, 255.0);
    let (r0, g0, b0) = (c(4, 255.0), c(5, 255.0), c(6, 255.0));
    let (r1, g1, b1) = (c(7, 0.0), c(8, 0.0), c(9, 0.0));
    let vertical = f(args, 10, 1.0) as i64 != 0;
    let x0 = (libm::round(x) as i64).max(0);
    let y0 = (libm::round(y) as i64).max(0);
    let x1 = (libm::round(x + w) as i64).min(bw as i64);
    let y1 = (libm::round(y + h) as i64).min(bh as i64);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let pack = |r: f64, g: f64, b: f64| ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
    let lerp = |t: f64| pack(r0 + (r1 - r0) * t, g0 + (g1 - g0) * t, b0 + (b1 - b0) * t);
    let solid = r0 == r1 && g0 == g1 && b0 == b1;
    let (fx, fy, fw, fh) = (libm::round(x), libm::round(y), w.max(1.0), h.max(1.0));
    for yy in y0..y1 {
        let row = yy as usize * bw;
        let line = &mut buf[row + x0 as usize..row + x1 as usize];
        if solid {
            line.fill(pack(r0, g0, b0));
        } else if vertical {
            line.fill(lerp(((yy as f64 - fy) / fh).clamp(0.0, 1.0)));
        } else {
            for (k, px) in line.iter_mut().enumerate() {
                *px = lerp((((x0 + k as i64) as f64 - fx) / fw).clamp(0.0, 1.0));
            }
        }
    }
}

// ── dispatch ─────────────────────────────────────────────────────────────────

pub fn dispatch(name: &[u8], args: &[u64]) -> Option<u64> {
    Some(match name {
        b"grad_rect" => {
            grad_rect(args);
            UNIT
        },
        // Same argument layout as the app runtime's font_text / font_width
        // (handle, x, y, px, text) / (handle, px, text), drawn in set_color's colour.
        b"font_text_px" => return crate::lingapp::dispatch(b"font_text", args),
        b"font_width_px" => return crate::lingapp::dispatch(b"font_width", args),
        b"monitor_height" | b"screen_height" => num(crate::drivers::framebuffer::height() as f64),
        b"monitor_width" | b"screen_width" => num(crate::drivers::framebuffer::width() as f64),
        // LingOS windows are always the whole screen: "maximized" is a no-op and
        // the work area is the screen (safe area).
        b"window_maximize" | b"maximize_window" => UNIT,
        b"window_work_area" | b"work_area" => list_from(alloc::vec![
            num(crate::drivers::framebuffer::width() as f64),
            num(crate::drivers::framebuffer::height() as f64),
        ]),

        // Files.
        b"read_file_bytes" => {
            let bytes = crate::vfs::read(&path(args, 0)).unwrap_or_default();
            let off = (f(args, 1, 0.0).max(0.0) as usize).min(bytes.len());
            let want = f(args, 2, -1.0);
            let end = if want < 0.0 { bytes.len() } else { (off + want as usize).min(bytes.len()) };
            list_from(bytes[off..end].iter().map(|&b| num(b as f64)).collect())
        },
        b"write_file_bytes" => {
            let data: Vec<u8> = args
                .get(1)
                .map(|&l| list_items(l).iter().map(|&v| to_f64(v) as i64 as u8).collect())
                .unwrap_or_default();
            boolean(crate::vfs::write(&path(args, 0), &data))
        },
        // The host's list_dir: full paths ("roms/x.nes"), sorted.
        b"list_dir" => {
            let dir = {
                let d = path(args, 0);
                if d.is_empty() { String::from(".") } else { d }
            };
            let base = dir.trim_end_matches('/');
            let mut out: Vec<String> = crate::vfs::list(if base == "." { "" } else { base })
                .into_iter()
                .map(|(n, _, _)| if base == "." || base.is_empty() { n } else { alloc::format!("{base}/{n}") })
                .collect();
            out.sort();
            list_from(out.iter().map(|p| mk_str(p)).collect())
        },
        b"path_name" => {
            let p = path(args, 0);
            mk_str(p.trim_end_matches('/').rsplit('/').next().unwrap_or(""))
        },
        b"path_ext" => {
            let p = path(args, 0);
            let n = p.rsplit('/').next().unwrap_or("");
            mk_str(match n.rfind('.') {
                Some(i) if i > 0 => &n[i + 1..],
                _ => "",
            })
        },

        // Strings / maths.
        b"ends_with" | b"str_ends_with" => boolean(s(args, 0).ends_with(s(args, 1).as_str())),
        b"starts_with" | b"str_starts_with" => boolean(s(args, 0).starts_with(s(args, 1).as_str())),
        b"chr" => {
            let c = char::from_u32(f(args, 0, 0.0) as u32).unwrap_or(char::REPLACEMENT_CHARACTER);
            let mut b = [0u8; 4];
            mk_str(c.encode_utf8(&mut b))
        },
        b"ord" => num(s(args, 0).chars().next().map(|c| c as u32 as f64).unwrap_or(-1.0)),
        b"clamp" => {
            let (x, lo, hi) = (f(args, 0, 0.0), f(args, 1, 0.0), f(args, 2, 1.0));
            num(if x < lo { lo } else if x > hi { hi } else { x })
        },

        // Gamepad, host API (pad index ignored: one USB pad).
        b"pad_poll" => {
            let p = pad();
            unsafe {
                PAD_PREV = PAD_NOW;
                PAD_NOW = pad_mask(&p);
            }
            num(p.connected as i32 as f64)
        },
        b"pad_count" => num(pad().connected as i32 as f64),
        b"pad_pressed" | b"pad_released" => {
            let Some(b) = pad_bit(&s(args, 1)) else { return Some(boolean(false)) };
            let (now, prev) = unsafe { ((PAD_NOW >> b) & 1, (PAD_PREV >> b) & 1) };
            boolean(if name == b"pad_pressed" { now == 1 && prev == 0 } else { now == 0 && prev == 1 })
        },
        b"pad_lx" => num(pad().axes[0] as f64),
        b"pad_ly" => num(pad().axes[1] as f64),
        b"pad_rx" => num(pad().axes[2] as f64),
        b"pad_ry" => num(pad().axes[3] as f64),
        b"pad_lt" => num(pad().axes[4].max(0.0) as f64),
        b"pad_rt" => num(pad().axes[5].max(0.0) as f64),

        // audio_tone(idx, x, y, z, w, freq, amp, lfo_rate, lfo_depth [, wave, duty])
        b"audio_tone" => {
            let wave = match s(args, 9).as_str() {
                "square" | "pulse" => 1,
                "saw" | "sawtooth" => 2,
                "triangle" | "tri" => 3,
                "noise" => 4,
                _ => 0,
            };
            crate::drivers::sfx::set_tone(
                f(args, 0, 0.0).max(0.0) as usize,
                f(args, 5, 220.0) as f32,
                f(args, 6, 0.15) as f32,
                wave,
                f(args, 10, 0.5) as f32,
                f(args, 1, 0.0) as f32,
            );
            UNIT
        },
        b"audio_glide" => {
            crate::drivers::sfx::set_tone_glide(f(args, 0, 0.0) as f32, f(args, 1, 0.0) as f32);
            UNIT
        },

        // No native emulator cores (ling-emu) on LingOS: the arcade hides SNES/N64.
        b"emu_available" | b"emu_loaded" | b"emu_load" => boolean(false),
        b"emu_status" => mk_str(""),
        b"emu_fb_size" => list_from(alloc::vec![num(0.0), num(0.0)]),
        b"emu_frame" | b"emu_blit" | b"emu_button" | b"emu_stick" | b"emu_flush" | b"emu_stop" => UNIT,

        _ => {
            let _ = is_str;
            return None;
        },
    })
}
