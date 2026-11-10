//! PrtSc screenshots. The key only raises a flag (`request`); whoever presents
//! the next full frame -- the Ling desktop's `lingapp::present` or the native
//! window manager's `framebuffer::present` -- sends it out without the pointer
//! and then calls [`run`]. That keeps one code path for both desktops and
//! means the picture is exactly what was on the screen.
//!
//! The overlay is modal: the frame is frozen and dimmed, a drag selects an
//! area (shown at full brightness with its size), a click without dragging,
//! Enter or the "Full screen" button takes everything, Esc or the right button
//! cancels. The capture goes to `home/<user>/Photos/Screenshots/Screenshot-<n>.png`
//! and onto the picture clipboard (paste into Files, LingDiffusion,
//! LingAnime), then a notice shows the path for a few seconds.

use crate::drivers::{clipboard, font8x8, framebuffer, keyboard};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

static mut PENDING: bool = false;
/// The notice after a capture: title, detail line, shown until this `now_ms`.
static mut TOAST: Option<(String, String, u64)> = None;

const TOAST_MS: u64 = 6000;
const ACCENT: u32 = 0x9A53D6;
const PANEL: u32 = 0x1B1B1B;
const INK: u32 = 0xF5F5F5;
const INK_DIM: u32 = 0xB8B8C8;

/// PrtSc was pressed: the overlay opens at the next present.
pub fn request() {
    unsafe { PENDING = true };
}

/// A capture is waiting (the frame now being drawn should leave the pointer out).
pub fn pending() -> bool {
    unsafe { PENDING }
}

pub fn take_pending() -> bool {
    unsafe { core::mem::replace(&mut *(&raw mut PENDING), false) }
}

// ── drawing into a 0xRRGGBB frame ─────────────────────────────────────────────

struct Canvas<'a> {
    px: &'a mut [u32],
    w: usize,
    h: usize,
}

impl Canvas<'_> {
    fn fill(&mut self, x: i64, y: i64, w: i64, h: i64, c: u32) {
        let (x0, y0) = (x.max(0) as usize, y.max(0) as usize);
        let (x1, y1) = ((x + w).clamp(0, self.w as i64) as usize, (y + h).clamp(0, self.h as i64) as usize);
        for yy in y0..y1 {
            if x0 < x1 {
                self.px[yy * self.w + x0..yy * self.w + x1].fill(c);
            }
        }
    }

    /// A `t`-pixel frame just outside the rectangle.
    fn outline(&mut self, x: i64, y: i64, w: i64, h: i64, t: i64, c: u32) {
        self.fill(x - t, y - t, w + 2 * t, t, c);
        self.fill(x - t, y + h, w + 2 * t, t, c);
        self.fill(x - t, y, t, h, c);
        self.fill(x + w, y, t, h, c);
    }

    fn text(&mut self, x: i64, y: i64, s: &str, c: u32, scale: i64) {
        for (k, ch) in s.bytes().enumerate() {
            let gx = x + k as i64 * 8 * scale;
            for (r, bits) in font8x8::glyph(ch).iter().enumerate() {
                for b in 0..8i64 {
                    if (bits >> b) & 1 != 0 {
                        self.fill(gx + b * scale, y + r as i64 * scale, scale, scale, c);
                    }
                }
            }
        }
    }

    /// A dark box with an accent border holding one line of text.
    fn label(&mut self, x: i64, y: i64, s: &str, scale: i64) -> (i64, i64) {
        let (w, h) = (s.len() as i64 * 8 * scale + 12 * scale, 8 * scale + 8 * scale);
        self.fill(x, y, w, h, PANEL);
        self.outline(x, y, w, h, 1, ACCENT);
        self.text(x + 6 * scale, y + 4 * scale, s, INK, scale);
        (w, h)
    }
}

// ── the overlay ───────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct Rect {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

const HINT: &str = "Drag: capture an area   Enter / click: full screen   Esc: cancel";
const FULL_BTN: &str = "Full screen";

/// 2x text on screens wide enough for the hint bar at that size.
fn ui_scale(w: usize) -> i64 {
    if (HINT.len() + FULL_BTN.len() + 8) * 16 + 80 <= w {
        2
    } else {
        1
    }
}

/// The hint bar (top centre) and its "Full screen" button: (bar, button) as x, y, w, h.
fn bar_layout(w: usize) -> ((i64, i64, i64, i64), (i64, i64, i64, i64)) {
    let s = ui_scale(w);
    let h = 16 * s;
    let hint_w = HINT.len() as i64 * 8 * s + 12 * s;
    let btn_w = FULL_BTN.len() as i64 * 8 * s + 12 * s;
    let total = hint_w + 8 * s + btn_w;
    let x = (w as i64 - total) / 2;
    let y = 12 * s;
    ((x, y, hint_w, h), (x + hint_w + 8 * s, y, btn_w, h))
}

fn inside(p: (i64, i64), r: (i64, i64, i64, i64)) -> bool {
    p.0 >= r.0 && p.1 >= r.1 && p.0 < r.0 + r.2 && p.1 < r.1 + r.3
}

/// The pointer in screen (safe-area) coordinates and its buttons, after
/// draining PS/2 and USB input -- nothing else runs while the overlay is up.
fn pointer(w: usize, h: usize) -> (i64, i64, u8) {
    crate::arch::idt::poll_drain_8042();
    crate::arch::xhci::service();
    let (ox, oy) = framebuffer::safe_margin();
    use crate::drivers::mouse;
    let x = (mouse::x() as i64 - ox as i64).clamp(0, w as i64 - 1);
    let y = (mouse::y() as i64 - oy as i64).clamp(0, h as i64 - 1);
    (x, y, mouse::buttons())
}

fn span(a: (i64, i64), b: (i64, i64), w: usize, h: usize) -> Rect {
    let x0 = a.0.min(b.0).clamp(0, w as i64 - 1) as usize;
    let y0 = a.1.min(b.1).clamp(0, h as i64 - 1) as usize;
    let x1 = (a.0.max(b.0) + 1).clamp(1, w as i64) as usize;
    let y1 = (a.1.max(b.1) + 1).clamp(1, h as i64) as usize;
    Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
}

fn dim(c: u32) -> u32 {
    // ~45 % brightness: the frozen screen stays readable behind the selection
    let ch = |s: u32| ((c >> s) & 0xFF) * 115 / 256;
    ch(16) << 16 | ch(8) << 8 | ch(0)
}

/// One overlay frame: the dimmed screen, the selection at full brightness
/// with its size, the hint bar, a crosshair at the pointer.
fn draw(out: &mut [u32], frame: &[u32], dimmed: &[u32], w: usize, h: usize, sel: Option<Rect>, m: (i64, i64)) {
    out.copy_from_slice(dimmed);
    let s = ui_scale(w);
    let mut cv = Canvas { px: out, w, h };
    if let Some(r) = sel {
        for y in r.y..r.y + r.h {
            let o = y * w + r.x;
            cv.px[o..o + r.w].copy_from_slice(&frame[o..o + r.w]);
        }
        cv.outline(r.x as i64, r.y as i64, r.w as i64, r.h as i64, 2, ACCENT);
        let size = format!("{} x {}", r.w, r.h);
        let lw = size.len() as i64 * 8 * s + 12 * s;
        let lx = (r.x as i64 + r.w as i64 - lw).max(0);
        let mut ly = r.y as i64 + r.h as i64 + 6;
        if ly + 16 * s > h as i64 {
            ly = (r.y as i64 - 16 * s - 6).max(0);
        }
        cv.label(lx, ly, &size, s);
    }
    let (bar, btn) = bar_layout(w);
    cv.label(bar.0, bar.1, HINT, s);
    let hot = inside(m, btn) && sel.is_none();
    cv.fill(btn.0, btn.1, btn.2, btn.3, if hot { ACCENT } else { PANEL });
    cv.outline(btn.0, btn.1, btn.2, btn.3, 1, ACCENT);
    cv.text(btn.0 + 6 * s, btn.1 + 4 * s, FULL_BTN, INK, s);
    // crosshair: a dark-edged white cross that reads on any background
    let (x, y) = m;
    cv.fill(x - 11, y - 1, 23, 3, 0x101010);
    cv.fill(x - 1, y - 11, 3, 23, 0x101010);
    cv.fill(x - 10, y, 21, 1, 0xFFFFFF);
    cv.fill(x, y - 10, 1, 21, 0xFFFFFF);
}

/// What the user picked: an area, the whole screen, or nothing (cancel).
fn select(frame: &[u32], w: usize, h: usize) -> Option<Rect> {
    let full = Rect { x: 0, y: 0, w, h };
    let dimmed: Vec<u32> = frame.iter().map(|&c| dim(c)).collect();
    let mut out = alloc::vec![0u32; w * h];
    let (mut mx, mut my, b0) = pointer(w, h);
    // a button already held when PrtSc came is not a press here
    let mut was = b0;
    let mut anchor: Option<(i64, i64)> = None;
    let mut dirty = true;
    let mut last_draw = 0u64;
    loop {
        loop {
            match keyboard::poll_char() {
                0 => break,
                27 => return None,
                b'\n' | b'\r' => return Some(full),
                _ => {},
            }
        }
        let (x, y, b) = pointer(w, h);
        if (x, y) != (mx, my) {
            (mx, my, dirty) = (x, y, true);
        }
        let (left, right) = (b & 1 != 0, b & 2 != 0);
        if right && was & 2 == 0 {
            return None;
        }
        if left && was & 1 == 0 {
            if inside((mx, my), bar_layout(w).1) {
                return Some(full);
            }
            (anchor, dirty) = (Some((mx, my)), true);
        }
        if !left && was & 1 != 0 {
            if let Some(a) = anchor.take() {
                let r = span(a, (mx, my), w, h);
                // a click without a drag means "everything"
                return Some(if r.w < 4 && r.h < 4 { full } else { r });
            }
        }
        was = b;
        let now = crate::arch::timer::now_ms();
        if dirty && now.saturating_sub(last_draw) >= 15 {
            draw(&mut out, frame, &dimmed, w, h, anchor.map(|a| span(a, (mx, my), w, h)), (mx, my));
            framebuffer::blit_rgb(&out, w, h);
            (dirty, last_draw) = (false, now);
        } else {
            // the SSH server and chat listener keep running while the overlay waits
            crate::proc::sched::yield_now();
            crate::arch::timer::delay_us(1000);
        }
    }
}

// ── saving ────────────────────────────────────────────────────────────────────

/// `home/<user>/Photos/Screenshots` of whoever is signed in; None before a sign-in.
fn shots_dir() -> Option<String> {
    if !crate::vfs::ready() {
        return None;
    }
    let user = crate::keyring::user().or_else(|| {
        let c = crate::fs::lingfs::current_user();
        (!c.is_empty() && c != "root").then(|| String::from(c))
    })?;
    Some(format!("home/{user}/Photos/Screenshots"))
}

/// `Screenshot-<n>.png`, one past the highest number already in `dir`.
fn next_path(dir: &str) -> String {
    let mut n = 0u32;
    for (name, _, _) in crate::vfs::list(dir) {
        let base = crate::vfs::basename(&name);
        if let Some(k) = base.strip_prefix("Screenshot-").and_then(|s| s.strip_suffix(".png")).and_then(|s| s.parse::<u32>().ok()) {
            n = n.max(k);
        }
    }
    clipboard::unique_path(&format!("{dir}/Screenshot-{}.png", n + 1))
}

fn save(frame: &[u32], fw: usize, r: Rect) {
    let mut rgb = Vec::with_capacity(r.w * r.h * 3);
    let mut rgba = Vec::with_capacity(r.w * r.h * 4);
    for y in r.y..r.y + r.h {
        for &c in &frame[y * fw + r.x..y * fw + r.x + r.w] {
            let px = [(c >> 16) as u8, (c >> 8) as u8, c as u8];
            rgb.extend_from_slice(&px);
            rgba.extend_from_slice(&[px[0], px[1], px[2], 0xFF]);
        }
    }
    let t0 = crate::arch::timer::now_ms();
    let png = ling_paint::os::png::encode_raw(r.w, r.h, 3, 8, &rgb);
    drop(rgb);
    let t1 = crate::arch::timer::now_ms();
    let saved = shots_dir().map(|d| next_path(&d)).filter(|p| crate::vfs::write(p, &png));
    let t2 = crate::arch::timer::now_ms();
    crate::drivers::serial::write(format!("screenshot: png {} bytes, encode {} ms, write {} ms\n", png.len(), t1 - t0, t2 - t1).as_bytes());
    let size = format!("{} x {}", r.w, r.h);
    let (title, detail) = match &saved {
        Some(p) => (format!("Screenshot saved ({size}), also on the clipboard"), p.clone()),
        None if shots_dir().is_none() => (format!("Screenshot ({size}) copied to the clipboard"), String::from("sign in to save screenshots to Photos/Screenshots")),
        None => (format!("Screenshot ({size}) copied to the clipboard"), String::from("could not save it to Photos/Screenshots")),
    };
    // the clipboard keeps the saved file's path and bytes: a paste into a
    // folder copies Screenshot-<n>.png as it is
    let clip = match &saved {
        Some(p) => clipboard::image_set_rgba(r.w, r.h, rgba, p, png, false),
        None => clipboard::image_set_rgba(r.w, r.h, rgba, "", Vec::new(), false),
    };
    crate::drivers::serial::write(format!("screenshot: {size} -> {} (clipboard {})\n", saved.as_deref().unwrap_or("(not saved)"), if clip { "set" } else { "refused" }).as_bytes());
    set_toast(title, detail, TOAST_MS);
}

/// The overlay over what is on the screen now; returns when the user captured or cancelled.
pub fn run() {
    let Some((mut frame, w, h)) = framebuffer::read_screen() else { return };
    let picked = select(&frame, w, h);
    // the overlay leaves at once: an idle desktop may not present again for a
    // while, and writing the file to disk can take seconds
    if let Some(r) = picked {
        let t0 = crate::arch::timer::now_ms();
        set_toast(String::from("Saving screenshot..."), format!("{} x {}", r.w, r.h), 60_000);
        let shown = toast_patch(w, h);
        // the toast goes on a copy: `save` crops from the clean frame
        let mut screen = frame.clone();
        if let Some(mut t) = shown {
            swap_into(&mut screen, w, &mut t);
        }
        framebuffer::blit_rgb(&screen, w, h);
        drop(screen);
        save(&frame, w, r);
        crate::drivers::serial::write(format!("screenshot: done in {} ms\n", crate::arch::timer::now_ms() - t0).as_bytes());
        // the notice on the restored screen right away, not at the next present
        if let Some(mut t) = toast_patch(w, h) {
            swap_into(&mut frame, w, &mut t);
        }
    }
    framebuffer::blit_rgb(&frame, w, h);
    settle();
}

/// The key or button that ended the overlay is usually still down: wait (at
/// most a second) for its release, then drop it, so the app under the overlay
/// never sees a stray Esc / Enter / click meant for the overlay (a USB
/// keyboard's held report would otherwise show it as a fresh key-down).
fn settle() {
    let until = crate::arch::timer::now_ms() + 1000;
    while crate::arch::timer::now_ms() < until {
        crate::arch::idt::poll_drain_8042();
        let held = crate::arch::xhci::held_report();
        if held[2..8].iter().all(|&u| u == 0) && crate::drivers::mouse::buttons() & 3 == 0 {
            break;
        }
        crate::proc::sched::yield_now();
        crate::arch::timer::delay_us(1000);
    }
    keyboard::drain();
}

fn set_toast(title: String, detail: String, ms: u64) {
    unsafe { *(&raw mut TOAST) = Some((title, detail, crate::arch::timer::now_ms() + ms)) };
}

// ── the notice ────────────────────────────────────────────────────────────────

/// A rectangle of 0xRRGGBB pixels placed at (x, y) on a frame.
pub struct Patch {
    pub px: Vec<u32>,
    pub w: usize,
    pub h: usize,
    pub x: usize,
    pub y: usize,
}

/// The notice for a `sw` x `sh` frame (bottom centre, above the dock), while it is due.
pub fn toast_patch(sw: usize, sh: usize) -> Option<Patch> {
    // the frame a capture is taken from shows no earlier notice either
    if pending() {
        return None;
    }
    let (title, detail) = unsafe {
        let t = (*(&raw const TOAST)).as_ref()?;
        if crate::arch::timer::now_ms() > t.2 {
            *(&raw mut TOAST) = None;
            return None;
        }
        (t.0.clone(), t.1.clone())
    };
    let s: i64 = if sw >= 1200 { 2 } else { 1 };
    let cols = title.len().max(detail.len()) as i64;
    let w = (cols * 8 * s + 24 * s).min(sw as i64) as usize;
    let h = (8 * s * 2 + 14 * s) as usize;
    if w == 0 || h + 8 > sh {
        return None;
    }
    let mut px = alloc::vec![PANEL; w * h];
    let mut cv = Canvas { px: &mut px, w, h };
    cv.fill(0, 0, w as i64, 2, ACCENT);
    cv.fill(0, h as i64 - 2, w as i64, 2, ACCENT);
    cv.fill(0, 0, 2, h as i64, ACCENT);
    cv.fill(w as i64 - 2, 0, 2, h as i64, ACCENT);
    cv.text(12 * s, 5 * s, &title, INK, s);
    cv.text(12 * s, 9 * s + 8 * s, &detail, INK_DIM, s);
    let y = sh.saturating_sub(h + 90).max(4);
    Some(Patch { px, w, h, x: (sw - w) / 2, y })
}

/// The patch's pixels and the frame's trade places (twice = no change), so the
/// notice rides on one presented frame and the app's own buffer comes back intact.
pub fn swap_into(buf: &mut [u32], bw: usize, t: &mut Patch) {
    for row in 0..t.h {
        let o = (t.y + row) * bw + t.x;
        if o + t.w > buf.len() || t.x + t.w > bw {
            return;
        }
        buf[o..o + t.w].swap_with_slice(&mut t.px[row * t.w..(row + 1) * t.w]);
    }
}

/// The notice for the native window manager's back buffer (`framebuffer::present`).
pub fn toast_image() -> Option<Patch> {
    toast_patch(framebuffer::width() as usize, framebuffer::height() as usize)
}
