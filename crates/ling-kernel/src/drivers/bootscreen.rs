//! The LingOS boot screen (x86_64 + Raspberry Pi): a spinning ROYGBIV ring, a
//! counter-rotating ring of status text (English + Korean) in DaemonChesilin,
//! the current boot stage + live console lines in the centre, a per-letter
//! ROYGBIV "Loading... <Stage>" header in ZCOOL XiaoWei, and animated
//! cryptography readouts in the corners. Design: live/tests/boot-preview.
//!
//! It animates from wherever boot already waits -- `console_write`, the
//! timers' `poll_until` and long `delay_us` calls -- so no boot step has to
//! know about it beyond naming its stage with [`stage`]. Frames are throttled
//! (at least 50 ms apart, and never more than ~1/3 of the CPU) and cheap: the
//! two rings are drawn from a per-pixel angle/radius table built once, and only
//! the ring square, the centre and the corners are re-blitted.
//!
//! The first full-screen draw by anything else (`framebuffer::present`, a Ling
//! app's frame) ends it, so a greeter, the desktop or a setup prompt simply
//! takes over. Fonts are build-time subsets (assets/boot_daemon.otf: ASCII +
//! the Hangul syllables of the stage names; assets/boot_zcool.ttf: ASCII; OFL).

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::drivers::framebuffer;

static DAEMON_OTF: &[u8] = include_bytes!("../../assets/boot_daemon.otf");
static ZCOOL_TTF: &[u8] = include_bytes!("../../assets/boot_zcool.ttf");

const ROYGBIV: [u32; 7] = [0xff2d3b, 0xff7a18, 0xffc400, 0x2fd16a, 0x1f8bff, 0x4b4bff, 0xb24bff];

pub const FILESYSTEM: usize = 0;
pub const DRIVERS: usize = 1;
pub const NETWORK: usize = 2;
pub const GRAPHICS: usize = 3;
pub const AUDIO: usize = 4;
pub const SERVICES: usize = 5;

/// (header word, centre label, Korean for the text ring)
const STAGES: [(&str, &str, &str); 6] = [
    ("Filesystem", "Loading Filesystem", "파일 시스템 로딩"),
    ("Drivers", "Loading Drivers", "드라이버 로딩"),
    ("Network", "Loading Network", "네트워크 로딩"),
    ("Graphics", "Loading Graphics", "그래픽 로딩"),
    ("Audio", "Loading Audio", "오디오 로딩"),
    ("Services", "Starting Services", "서비스 시작"),
];
/// How many `stage` calls a boot makes (the progress bar's denominator).
const STEPS: u32 = 6;
/// Each stage stays on screen at least this long (x86 boots in a blink).
const MIN_STAGE_MS: u64 = 280;

const LINES: usize = 10;
const LINE_MAX: usize = 96;

const F_DAEMON: u8 = 0;
const F_ZCOOL: u8 = 1;

struct Glyph {
    xmin: i32,
    ymin: i32,
    w: usize,
    h: usize,
    adv: f32,
    cov: Vec<u8>,
}

struct Glyphs {
    fonts: [fontdue::Font; 2],
    map: BTreeMap<(u8, u16, u32), Glyph>,
}

impl Glyphs {
    fn glyph(&mut self, f: u8, px: u16, c: char) -> &Glyph {
        let fonts = &self.fonts;
        self.map.entry((f, px, c as u32)).or_insert_with(|| {
            let (m, cov) = fonts[f as usize].rasterize(c, px as f32);
            Glyph { xmin: m.xmin, ymin: m.ymin, w: m.width, h: m.height, adv: m.advance_width, cov }
        })
    }

    fn advance(&mut self, f: u8, px: u16, c: char) -> f32 {
        match c {
            '◆' => px as f32 * 0.9,
            '·' => px as f32 * 0.5,
            _ => self.glyph(f, px, c).adv,
        }
    }

    fn width(&mut self, f: u8, px: u16, text: &str, spacing: f32) -> f32 {
        let mut w = 0.0;
        for c in text.chars() {
            w += self.advance(f, px, c) + spacing;
        }
        (w - spacing).max(0.0)
    }

    /// Lay `text` out from (`x`, baseline `base`), handing every covered pixel
    /// to `put(x, y, coverage)`; `color_of(i)` is unused here (callers that
    /// colour per letter draw letter by letter). Returns the pen's end x.
    fn draw(&mut self, f: u8, px: u16, x: f32, base: f32, text: &str, spacing: f32, put: &mut dyn FnMut(i32, i32, u8)) -> f32 {
        let mut pen = x;
        let pxf = px as f32;
        for c in text.chars() {
            match c {
                '◆' => {
                    let (mx, my, half) = (pen + pxf * 0.45, base - pxf * 0.36, pxf * 0.3);
                    let r = half as i32 + 1;
                    for dy in -r..=r {
                        for dx in -r..=r {
                            let d = abs(dx as f32 + 0.5 - (mx - libm::floorf(mx))) + abs(dy as f32 + 0.5 - (my - libm::floorf(my)));
                            let a = clamp01(half - d + 0.5);
                            if a > 0.0 {
                                put(mx as i32 + dx, my as i32 + dy, (a * 255.0) as u8);
                            }
                        }
                    }
                },
                '·' => {
                    let (mx, my, rad) = (pen + pxf * 0.25, base - pxf * 0.33, (pxf * 0.09).max(1.0));
                    let r = rad as i32 + 1;
                    for dy in -r..=r {
                        for dx in -r..=r {
                            let d = libm::sqrtf((dx * dx + dy * dy) as f32);
                            let a = clamp01(rad - d + 0.5);
                            if a > 0.0 {
                                put(mx as i32 + dx, my as i32 + dy, (a * 255.0) as u8);
                            }
                        }
                    }
                },
                _ => {
                    let g = self.glyph(f, px, c);
                    let ox = libm::roundf(pen) as i32 + g.xmin;
                    let oy = libm::roundf(base) as i32 - g.ymin - g.h as i32;
                    for by in 0..g.h {
                        for bx in 0..g.w {
                            let a = g.cov[by * g.w + bx];
                            if a != 0 {
                                put(ox + bx as i32, oy + by as i32, a);
                            }
                        }
                    }
                },
            }
            pen += self.advance(f, px, c) + spacing;
        }
        pen
    }
}

#[inline]
fn abs(v: f32) -> f32 {
    if v < 0.0 { -v } else { v }
}

#[inline]
fn clamp01(v: f32) -> f32 {
    if v < 0.0 {
        0.0
    } else if v > 1.0 {
        1.0
    } else {
        v
    }
}

#[inline]
fn blend(d: u32, c: u32, a: u32) -> u32 {
    if a >= 255 {
        return c;
    }
    let mix = |sh: u32| {
        let dv = (d >> sh) & 0xff;
        let cv = (c >> sh) & 0xff;
        ((dv * (255 - a) + cv * a) / 255) << sh
    };
    mix(16) | mix(8) | mix(0)
}

fn lerp_rgb(a: u32, b: u32, t: f32) -> u32 {
    let t = clamp01(t);
    let ch = |sh: u32| {
        let av = ((a >> sh) & 0xff) as f32;
        let bv = ((b >> sh) & 0xff) as f32;
        ((av + (bv - av) * t) as u32) << sh
    };
    ch(16) | ch(8) | ch(0)
}

fn dim(c: u32, a: f32) -> u32 {
    lerp_rgb(0, c, a)
}

/// One rectangle of the screen, for re-blitting.
#[derive(Clone, Copy)]
struct Rect {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

struct Boot {
    w: usize,
    h: usize,
    s: f32,
    cx: f32,
    cy: f32,
    bg: Vec<u32>,
    cv: Vec<u32>,
    g: Glyphs,
    /// ring pixels: (index into the screen, angle in 1/65536 turn clockwise
    /// from 12 o'clock, radius * 16)
    ring: Vec<(u32, u16, u16)>,
    square: Rect,
    centre: Rect,
    header: Rect,
    corners: [Rect; 4],
    // the text ring, unrolled: coverage, strip_w x strip_h, baseline row
    strip: Vec<u8>,
    strip_w: usize,
    strip_h: usize,
    strip_base: f32,
    ring_r_in: f32,
    ring_r_out: f32,
    text_r: f32,
    stage: usize,
    done: u32,
    stage_t: u64,
    lines: [[u8; LINE_MAX]; LINES],
    line_len: [usize; LINES],
    nlines: usize,
    partial: [u8; LINE_MAX],
    plen: usize,
    t0: u64,
    next_frame: u64,
    next_crypto: u64,
    next_data: u64,
    data_i: usize,
    rng: u64,
    crypto: [String; 12],
}

static mut BOOT: Option<Boot> = None;
static mut ACTIVE: bool = false;
static mut BUSY: bool = false;
static mut BEGUN: bool = false;

fn now_ms() -> u64 {
    crate::arch::timer::now_ms()
}

/// Is the boot screen up? (Cheap; the console checks it on every write.)
#[inline]
pub fn active() -> bool {
    unsafe { ACTIVE }
}

fn boot() -> Option<&'static mut Boot> {
    unsafe { (*(&raw mut BOOT)).as_mut() }
}

/// Paint the boot screen and start animating it. Once per boot; needs the
/// framebuffer and the heap.
pub fn begin() {
    unsafe {
        if BEGUN {
            return;
        }
        BEGUN = true;
    }
    if !framebuffer::available() {
        return;
    }
    let (w, h) = (framebuffer::width() as usize, framebuffer::height() as usize);
    if w < 320 || h < 240 {
        return;
    }
    let Ok(daemon) = fontdue::Font::from_bytes(DAEMON_OTF, fontdue::FontSettings::default()) else { return };
    let Ok(zcool) = fontdue::Font::from_bytes(ZCOOL_TTF, fontdue::FontSettings::default()) else { return };
    let s = (h.min(w) as f32) / 760.0;
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let t = now_ms();
    let mut b = Boot {
        w,
        h,
        s,
        cx,
        cy,
        bg: vec![0u32; w * h],
        cv: Vec::new(),
        g: Glyphs { fonts: [daemon, zcool], map: BTreeMap::new() },
        ring: Vec::new(),
        square: Rect { x: 0, y: 0, w: 0, h: 0 },
        centre: Rect { x: 0, y: 0, w: 0, h: 0 },
        header: Rect { x: 0, y: 0, w, h: 0 },
        corners: [Rect { x: 0, y: 0, w: 0, h: 0 }; 4],
        strip: Vec::new(),
        strip_w: 1,
        strip_h: 1,
        strip_base: 0.0,
        ring_r_in: 158.0 * s,
        ring_r_out: 192.0 * s,
        text_r: 236.0 * s,
        stage: GRAPHICS,
        done: 0,
        stage_t: t,
        lines: [[0; LINE_MAX]; LINES],
        line_len: [0; LINES],
        nlines: 0,
        partial: [0; LINE_MAX],
        plen: 0,
        t0: t,
        next_frame: 0,
        next_crypto: 0,
        next_data: 0,
        data_i: 0,
        rng: 0x9E37_79B9_7F4A_7C15 ^ crate::arch::timer::now_us(),
        crypto: Default::default(),
    };
    b.build_background();
    b.build_ring_table();
    b.cv = b.bg.clone();
    // the overscan margin too: no banner left peeking out around the edges
    framebuffer::clear_physical(0x040407);
    unsafe {
        *(&raw mut BOOT) = Some(b);
        ACTIVE = true;
    }
    let b = boot().unwrap();
    unsafe { BUSY = true };
    b.set_stage(GRAPHICS);
    unsafe { BUSY = false };
    framebuffer::blit_rect(&b.cv, w, 0, 0, w, h);
    frame(true);
}

/// Name the boot step that starts now (one of the stage constants).
pub fn stage(i: usize) {
    if !active() || i >= STAGES.len() {
        return;
    }
    let Some(b) = boot() else { return };
    // x86 boots in well under a second: hold each stage long enough to read
    while now_ms() < b.stage_t + MIN_STAGE_MS {
        tick();
        unsafe { crate::arch::cpu::pause() };
    }
    unsafe { BUSY = true };
    b.set_stage(i);
    unsafe { BUSY = false };
    frame(true);
}

/// Boot is done: show 100% for a moment, then give the screen up.
pub fn finish() {
    if !active() {
        return;
    }
    if let Some(b) = boot() {
        while now_ms() < b.stage_t + MIN_STAGE_MS {
            tick();
            unsafe { crate::arch::cpu::pause() };
        }
        b.done = STEPS + 1;
        frame(true);
    }
    end();
}

/// Stop drawing and free the screen buffers. Anything that takes the whole
/// screen calls this (idempotent, cheap when already off).
pub fn end() {
    unsafe {
        if !ACTIVE || BUSY {
            ACTIVE = false;
            return;
        }
        ACTIVE = false;
        *(&raw mut BOOT) = None;
    }
}

/// End the boot screen and bring the Pi's text console back underneath (for
/// images that boot to the console rather than the desktop).
#[cfg(target_arch = "aarch64")]
pub fn end_to_console() {
    if active() {
        end();
        crate::rpi_boot_screen();
        crate::drivers::fbcon::init();
    }
}

/// Console output while the boot screen is up: becomes the centre's data
/// lines (the newest on top, older ones cycling underneath). Allocation-free
/// (fixed buffers), so the serial driver can call it too.
pub fn feed(bytes: &[u8]) {
    unsafe {
        if !ACTIVE || BUSY {
            return;
        }
    }
    // never from an interrupt handler (the timer's heartbeat line): the main
    // thread may be halfway through changing this state
    if !crate::arch::cpu::irqs_enabled() {
        return;
    }
    let Some(b) = boot() else { return };
    for &c in bytes {
        match c {
            b'\n' | b'\r' => {
                let mut st = 0;
                let mut en = b.plen;
                while st < en && b.partial[st] == b' ' {
                    st += 1;
                }
                while en > st && b.partial[en - 1] == b' ' {
                    en -= 1;
                }
                if en > st {
                    if b.nlines == LINES {
                        b.lines.copy_within(1.., 0);
                        b.line_len.copy_within(1.., 0);
                        b.nlines -= 1;
                    }
                    let k = b.nlines;
                    b.lines[k][..en - st].copy_from_slice(&b.partial[st..en]);
                    b.line_len[k] = en - st;
                    b.nlines += 1;
                }
                b.plen = 0;
            },
            0x20..=0x7e if b.plen < LINE_MAX => {
                b.partial[b.plen] = c;
                b.plen += 1;
            },
            _ => {},
        }
    }
}

/// Advance the animation if a frame is due. Called from boot's wait loops.
#[inline]
pub fn tick() {
    if active() {
        frame(false);
    }
}

fn frame(force: bool) {
    unsafe {
        if BUSY || !ACTIVE {
            return;
        }
    }
    // drawing allocates, rasterizes and races the main thread: interrupt
    // context (IRQs masked) only ever skips a tick
    if !crate::arch::cpu::irqs_enabled() {
        return;
    }
    let Some(b) = boot() else { return };
    let t = now_ms();
    if !force && t < b.next_frame {
        return;
    }
    unsafe { BUSY = true };
    b.render(t);
    let cost = now_ms().saturating_sub(t);
    b.next_frame = t + (cost * 3).max(50);
    unsafe { BUSY = false };
}

impl Boot {
    /// Screen y for a y in the 760-unit design.
    fn vy(&self, y: f32) -> f32 {
        self.cy + (y - 380.0) * self.s
    }

    fn grad(&self, x: usize, y: usize) -> u32 {
        // radial-gradient(60% 60% at 50% 46%, #10101e 0, #07070e 55%, #040407 100%)
        let dx = (x as f32 + 0.5 - self.w as f32 * 0.5) / (self.w as f32 * 0.6);
        let dy = (y as f32 + 0.5 - self.h as f32 * 0.46) / (self.h as f32 * 0.6);
        let t = libm::sqrtf(dx * dx + dy * dy);
        if t < 0.55 { lerp_rgb(0x10101e, 0x07070e, t / 0.55) } else { lerp_rgb(0x07070e, 0x040407, (t - 0.55) / 0.45) }
    }

    fn build_background(&mut self) {
        let (w, h, s) = (self.w, self.h, self.s);
        for y in 0..h {
            for x in 0..w {
                self.bg[y * w + x] = self.grad(x, y);
            }
        }
        // the glow behind the ring, the dark core, its thin rim
        let (r_glow, r_core) = (168.0 * s, 150.0 * s);
        let x0 = (self.cx - r_glow - 2.0).max(0.0) as usize;
        let y0 = (self.cy - r_glow - 2.0).max(0.0) as usize;
        let x1 = ((self.cx + r_glow + 2.0) as usize).min(w);
        let y1 = ((self.cy + r_glow + 2.0) as usize).min(h);
        for y in y0..y1 {
            for x in x0..x1 {
                let dx = x as f32 + 0.5 - self.cx;
                let dy = y as f32 + 0.5 - self.cy;
                let r = libm::sqrtf(dx * dx + dy * dy);
                let i = y * w + x;
                let t = r / r_glow;
                if t < 1.0 {
                    self.bg[i] = if t < 0.78 {
                        lerp_rgb(0x1a1a30, 0x0b0b16, t / 0.78)
                    } else {
                        blend(self.bg[i], 0x0b0b16, ((1.0 - (t - 0.78) / 0.22) * 255.0) as u32)
                    };
                }
                let a = clamp01(r_core - r + 0.5);
                if a > 0.0 {
                    self.bg[i] = blend(self.bg[i], 0x0a0a14, (a * 255.0) as u32);
                }
                let rim = clamp01(0.75 * s.max(1.0) - abs(r - r_core) + 0.5);
                if rim > 0.0 {
                    self.bg[i] = blend(self.bg[i], 0x23233a, (rim * 255.0) as u32);
                }
            }
        }
        // the LINGOS wordmark at the bottom
        let px = (18.0 * s) as u16;
        let sp = 8.0 * s;
        let tw = self.g.width(F_DAEMON, px, "LINGOS", sp);
        let base = self.vy(726.0);
        let (bg, ww, hh) = (&mut self.bg, w, h);
        self.g.draw(F_DAEMON, px, self.cx - tw / 2.0, base, "LINGOS", sp, &mut |x, y, a| {
            if x >= 0 && y >= 0 && (x as usize) < ww && (y as usize) < hh {
                let i = y as usize * ww + x as usize;
                bg[i] = blend(bg[i], 0x53536e, a as u32);
            }
        });

        // regions re-blitted per frame
        let rmax = self.text_r + 26.0 * s;
        let sx = (self.cx - rmax).max(0.0) as usize;
        let sy = (self.cy - rmax).max(0.0) as usize;
        self.square = Rect { x: sx, y: sy, w: ((self.cx + rmax) as usize).min(w) - sx, h: ((self.cy + rmax) as usize).min(h) - sy };
        let rc = 150.0 * s;
        let (ccx, ccy) = ((self.cx - rc) as usize, (self.cy - rc) as usize);
        self.centre = Rect { x: ccx, y: ccy, w: (2.0 * rc) as usize + 1, h: (2.0 * rc) as usize + 1 };
        let hy0 = self.vy(60.0 - 34.0).max(0.0) as usize;
        let hy1 = (self.vy(60.0 + 12.0) as usize).min(h);
        let cw = ((270.0 * s) as usize).min(w / 2);
        // between the corner readouts, so a stage change never wipes them
        self.header = Rect { x: cw, y: hy0, w: w - 2 * cw, h: hy1 - hy0 };
        // corner rows hug the screen's top / bottom edge (see render)
        let top0 = ((26.0 - 14.0) * s) as usize;
        let top1 = ((76.0 + 6.0) * s) as usize;
        let bot0 = (h as f32 - (760.0 - 698.0 + 14.0) * s).max(0.0) as usize;
        let bot1 = ((h as f32 - (760.0 - 748.0 - 6.0) * s) as usize).min(h);
        let tl = |x: usize, y0: usize, y1: usize| Rect { x, y: y0, w: cw, h: y1.saturating_sub(y0) };
        self.corners = [tl(0, top0, top1), tl(w - cw, top0, top1), tl(0, bot0, bot1), tl(w - cw, bot0, bot1)];
    }

    fn build_ring_table(&mut self) {
        let s = self.s;
        let (ri, ro) = (self.ring_r_in - 1.5, self.ring_r_out + 1.5);
        let px = 20.0 * s;
        let (ti, to) = (self.text_r - px * 0.35, self.text_r + px * 1.05);
        let q = self.square;
        let mut v = Vec::new();
        for y in q.y..q.y + q.h {
            for x in q.x..q.x + q.w {
                let dx = x as f32 + 0.5 - self.cx;
                let dy = y as f32 + 0.5 - self.cy;
                let r = libm::sqrtf(dx * dx + dy * dy);
                if (r >= ri && r <= ro) || (r >= ti && r <= to) {
                    // clockwise from 12 o'clock, in turns
                    let mut a = libm::atan2f(dx, -dy) / (2.0 * core::f32::consts::PI);
                    if a < 0.0 {
                        a += 1.0;
                    }
                    v.push(((y * self.w + x) as u32, ((a * 65536.0) as u32 & 0xffff) as u16, (r * 16.0) as u16));
                }
            }
        }
        self.ring = v;
    }

    fn set_stage(&mut self, i: usize) {
        self.stage = i;
        self.done = (self.done + 1).min(STEPS);
        self.stage_t = now_ms();
        self.data_i = 0;
        // header: "Loading... <Stage>", one ROYGBIV colour per letter
        let s = self.s;
        let hr = self.header;
        for y in hr.y..hr.y + hr.h {
            for x in hr.x..hr.x + hr.w {
                self.bg[y * self.w + x] = self.grad(x, y);
            }
        }
        let mut text = String::from("Loading... ");
        text.push_str(STAGES[i].0);
        let px = (32.0 * s) as u16;
        let sp = 3.0 * s;
        let tw = self.g.width(F_ZCOOL, px, &text, sp);
        let mut pen = self.cx - tw / 2.0;
        let base = self.vy(60.0);
        let mut ci = 0usize;
        let (w, h) = (self.w, self.h);
        let mut buf = [0u8; 4];
        for c in text.chars() {
            let col = ROYGBIV[ci % 7];
            if c != ' ' {
                ci += 1;
            }
            let bg = &mut self.bg;
            pen = self.g.draw(F_ZCOOL, px, pen, base, c.encode_utf8(&mut buf), 0.0, &mut |x, y, a| {
                if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
                    let i = y as usize * w + x as usize;
                    bg[i] = blend(bg[i], col, a as u32);
                }
            }) + sp;
        }
        for y in hr.y..hr.y + hr.h {
            let r = y * w + hr.x;
            self.cv[r..r + hr.w].copy_from_slice(&self.bg[r..r + hr.w]);
        }
        framebuffer::blit_rect(&self.cv, w, hr.x, hr.y, hr.w, hr.h);
        self.build_strip();
    }

    /// The text ring unrolled into a strip as long as its circumference, with
    /// whole repeats of "<STAGE>  ◆  <Korean>  ◆  " stretched to close seamlessly.
    fn build_strip(&mut self) {
        let s = self.s;
        let px = (20.0 * s) as u16;
        let pxf = px as f32;
        let mut unit = String::from(STAGES[self.stage].1);
        unit.make_ascii_uppercase();
        unit.push_str("  ◆  ");
        unit.push_str(STAGES[self.stage].2);
        unit.push_str("  ◆  ");
        let sp = 2.5 * s;
        let sw = (2.0 * core::f32::consts::PI * self.text_r) as usize;
        let uw = self.g.width(F_DAEMON, px, &unit, sp) + sp;
        let n = ((sw as f32 / uw) as usize).max(1);
        let chars = unit.chars().count() as f32;
        let extra = (sw as f32 - n as f32 * uw) / (n as f32 * chars);
        let sh = (pxf * 1.45) as usize + 3;
        let base = pxf * 1.05 + 1.0;
        let mut strip = vec![0u8; sw * sh];
        let mut pen = 0.0f32;
        for _ in 0..n {
            pen = self.g.draw(F_DAEMON, px, pen, base, &unit, sp + extra, &mut |x, y, a| {
                if x >= 0 && y >= 0 && (y as usize) < sh {
                    let i = y as usize * sw + (x as usize % sw);
                    if a > strip[i] {
                        strip[i] = a;
                    }
                }
            }) + sp + extra;
        }
        self.strip = strip;
        self.strip_w = sw;
        self.strip_h = sh;
        self.strip_base = base;
    }

    fn rand(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        x
    }

    fn hex(&mut self, n: usize, sep: char) -> String {
        const D: &[u8; 16] = b"0123456789abcdef";
        let mut s = String::new();
        for i in 0..n {
            let v = self.rand() as u8;
            if i > 0 {
                s.push(sep);
            }
            s.push(D[(v >> 4) as usize] as char);
            s.push(D[(v & 15) as usize] as char);
        }
        s
    }

    fn render(&mut self, t: u64) {
        let (w, s) = (self.w, self.s);
        let el = t.saturating_sub(self.t0);

        // the centre square first: its corners reach into the ring band
        let c = self.centre;
        for y in c.y..c.y + c.h {
            let r = y * w;
            self.cv[r + c.x..r + c.x + c.w].copy_from_slice(&self.bg[r + c.x..r + c.x + c.w]);
        }

        // ── both rings, from the angle/radius table ──
        let rot1 = (el % 9000) as f32 / 9000.0;
        let rot2 = (el % 34000) as f32 / 34000.0;
        let gap = 1.6 / 360.0;
        let seg = 1.0 / 7.0;
        let (ri, ro) = (self.ring_r_in, self.ring_r_out);
        let tau = 2.0 * core::f32::consts::PI;
        let (sw, sh) = (self.strip_w, self.strip_h);
        for &(idx, ang, rq) in self.ring.iter() {
            let i = idx as usize;
            let r = rq as f32 / 16.0;
            let a = ang as f32 / 65536.0;
            let bgc = self.bg[i];
            if r <= ro + 2.0 {
                let mut local = a - rot1;
                if local < 0.0 {
                    local += 1.0;
                }
                let k = ((local * 7.0) as usize).min(6);
                let within = local - k as f32 * seg;
                let edge = (within.min(seg - within) - gap) * tau * r;
                let aa = clamp01(edge + 0.5) * clamp01((r - ri).min(ro - r) + 0.5);
                self.cv[i] = if aa > 0.0 { blend(bgc, ROYGBIV[k], (aa * 255.0) as u32) } else { bgc };
            } else {
                // arc position along the text path (starts at 9 o'clock, runs
                // clockwise; the ring itself turns counter-clockwise)
                let mut p = a - 0.75 + rot2;
                p -= libm::floorf(p);
                let fx = p * sw as f32;
                let fy = self.strip_base - (r - self.text_r);
                let cov = if fy < 0.0 || fy >= (sh - 1) as f32 {
                    0.0
                } else {
                    let x0 = fx as usize % sw;
                    let x1 = (x0 + 1) % sw;
                    let y0 = fy as usize;
                    let (tx, ty) = (fx - libm::floorf(fx), fy - y0 as f32);
                    let g = |x: usize, y: usize| self.strip[y * sw + x] as f32;
                    let top = g(x0, y0) * (1.0 - tx) + g(x1, y0) * tx;
                    let bot = g(x0, y0 + 1) * (1.0 - tx) + g(x1, y0 + 1) * tx;
                    top * (1.0 - ty) + bot * ty
                };
                self.cv[i] = if cov > 1.0 { blend(bgc, 0xc9c9e4, cov as u32) } else { bgc };
            }
        }

        // ── the centre: progress, stage, live boot data ──
        let pct = if self.done > STEPS {
            100
        } else {
            let lo = (self.done.saturating_sub(1) * 100 / STEPS) as f32;
            let hi = (self.done * 100 / STEPS) as f32 - 1.0;
            let e = t.saturating_sub(self.stage_t) as f32;
            (lo + (hi - lo) * (1.0 - 1.0 / (1.0 + e / 900.0))) as u32
        };
        let mut pcts = String::new();
        let _ = core::fmt::write(&mut pcts, format_args!("{}%", pct));
        if t >= self.next_data {
            self.next_data = t + 150;
            self.data_i += 1;
        }
        let (hero, sub1, sub2) = self.data_lines();
        self.text_c(F_DAEMON, 13.0, 322.0, &pcts, 0xb7b7d6, 2.0);
        self.text_c(F_DAEMON, 20.0, 352.0, STAGES[self.stage].1, 0xffffff, 0.0);
        self.text_c(F_DAEMON, 27.0, 392.0, &hero, 0xf2f2fb, 0.0);
        self.text_c(F_DAEMON, 15.0, 424.0, &sub1, 0x8f8fb0, 0.0);
        self.text_c(F_DAEMON, 12.0, 446.0, &sub2, dim(0x5f5f7e, 0.8), 0.0);

        // ── the corners: live hex / fingerprints / bits ──
        let crypto_due = t >= self.next_crypto;
        if crypto_due {
            self.next_crypto = t + 240;
            for k in [1usize, 2, 3, 5, 6, 7, 10] {
                self.crypto[k] = self.hex(7, ' ');
            }
            self.crypto[9] = self.hex(6, ':');
            let bits = self.rand();
            let mut b = String::new();
            for i in 0..16 {
                if i == 8 {
                    b.push(' ');
                }
                b.push(if bits >> i & 1 == 1 { '1' } else { '0' });
            }
            self.crypto[8] = b;
            for k in 0..4 {
                let r = self.corners[k];
                for y in r.y..r.y + r.h {
                    let o = y * w;
                    self.cv[o + r.x..o + r.x + r.w].copy_from_slice(&self.bg[o + r.x..o + r.x + r.w]);
                }
            }
            let (l, rt) = (14.0 * s, w as f32 - 14.0 * s);
            let rows: [(f32, bool, &str, u32); 16] = [
                (26.0, false, "X25519 · CHACHA20", 0xff7a18),
                (44.0, false, "", 0x6f6f92),
                (60.0, false, "", 0x55556f),
                (76.0, false, "", 0x44445c),
                (26.0, true, "ML-KEM-768  OK", 0x2fd16a),
                (44.0, true, "ML-DSA-65  OK", 0x2aa85a),
                (60.0, true, "", 0x55556f),
                (76.0, true, "", 0x44445c),
                (698.0, false, "", 0x44445c),
                (714.0, false, "", 0x55556f),
                (732.0, false, "fingerprint", 0x1f8bff),
                (748.0, false, "", 0x6f6f92),
                (698.0, true, "", 0x44445c),
                (714.0, true, "", 0x55556f),
                (732.0, true, "BLAKE3 · SHA256", 0xb24bff),
                (748.0, true, "", 0x6f6f92),
            ];
            // which crypto[] string fills each empty row
            let fill: [usize; 16] = [0, 1, 2, 3, 0, 0, 5, 6, 7, 10, 0, 9, 8, 11, 0, 4];
            self.crypto[11] = self.hex(7, ' ');
            self.crypto[4] = self.hex(7, ' ');
            for (n, &(y, right, txt, col)) in rows.iter().enumerate() {
                let txt: String = if txt.is_empty() { self.crypto[fill[n]].clone() } else { String::from(txt) };
                let px = (12.5 * s) as u16;
                let sp = 1.0 * s;
                let x = if right { rt - self.g.width(F_DAEMON, px, &txt, sp) } else { l };
                let base = if y > 380.0 { self.h as f32 - (760.0 - y) * s } else { y * s };
                self.text_at(F_DAEMON, px, x, base, &txt, col, sp, None);
            }
        }

        let q = self.square;
        framebuffer::blit_rect(&self.cv, w, q.x, q.y, q.w, q.h);
        if crypto_due {
            for k in 0..4 {
                let r = self.corners[k];
                framebuffer::blit_rect(&self.cv, w, r.x, r.y, r.w, r.h);
            }
        }
    }

    /// (hero, sub line, cycling older line) from the console's recent output.
    fn data_lines(&self) -> (String, String, String) {
        let n = self.nlines;
        if n == 0 {
            return (String::from("LingOS"), String::from("booting"), String::new());
        }
        let line = |k: usize| String::from(core::str::from_utf8(&self.lines[k][..self.line_len[k]]).unwrap_or(""));
        let last = &line(n - 1);
        let (hero, sub) = match last.find(": ") {
            Some(k) if k <= 22 => (String::from(&last[..k]), String::from(last[k + 2..].trim())),
            _ => match last.find(' ') {
                Some(k) if k <= 22 => (String::from(&last[..k]), String::from(last[k + 1..].trim())),
                _ => (String::from(last.as_str()), String::new()),
            },
        };
        let older = if n >= 2 { line(n - 2 - self.data_i % (n - 1)) } else { String::new() };
        (hero, sub, older)
    }

    /// Centred text inside the core circle, truncated to the chord it sits on.
    fn text_c(&mut self, f: u8, size: f32, y: f32, text: &str, col: u32, spacing: f32) {
        let s = self.s;
        let px = (size * s) as u16;
        let sp = spacing * s;
        let base = self.vy(y);
        let rc = 150.0 * s;
        let dy = (base - px as f32 * 0.35 - self.cy).min(rc - 1.0);
        let maxw = 2.0 * (libm::sqrtf(rc * rc - dy * dy) - 10.0 * s);
        let mut t = String::from(text);
        while !t.is_empty() && self.g.width(f, px, &t, sp) > maxw {
            t.pop();
        }
        let tw = self.g.width(f, px, &t, sp);
        let clip = (self.cx, self.cy, rc - 1.0);
        self.text_at(f, px, self.cx - tw / 2.0, base, &t, col, sp, Some(clip));
    }

    fn text_at(&mut self, f: u8, px: u16, x: f32, base: f32, text: &str, col: u32, sp: f32, clip: Option<(f32, f32, f32)>) {
        let (w, h) = (self.w, self.h);
        let cv = &mut self.cv;
        self.g.draw(f, px, x, base, text, sp, &mut |x, y, a| {
            if x < 0 || y < 0 || x as usize >= w || y as usize >= h {
                return;
            }
            if let Some((ccx, ccy, r)) = clip {
                let (dx, dy) = (x as f32 + 0.5 - ccx, y as f32 + 0.5 - ccy);
                if dx * dx + dy * dy > r * r {
                    return;
                }
            }
            let i = y as usize * w + x as usize;
            cv[i] = blend(cv[i], col, a as u32);
        });
    }
}
