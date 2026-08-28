//! Minimal framebuffer graphics for native LingOS apps. Maps the hardware
//! framebuffer once via SYS_FB_MAP, then offers clear / z-buffered plot /
//! input polling -- enough for a software-rendered, depth-correct spinning
//! donut. The z-buffer lives here in native `.bss` (not a `.ling` list), so
//! per-pixel depth testing is free in the app's own code. Depth is stored as
//! "one over z" (ooz): larger = nearer, matching the classic donut renderer.

use crate::syscall::{ling_sys_fb_map, ling_sys_poll_input};

/// Covers up to 1280x1024. Larger framebuffers simply skip plots past the end
/// (the donut is centered, so this never clips it in practice).
const ZMAX: usize = 1_310_720;

static mut FB_BASE: u64 = 0;
static mut FB_W: u32 = 0;
static mut FB_H: u32 = 0;
static mut FB_PITCH: u32 = 0;
static mut FB_BPP: u32 = 0;
static mut ZBUF: [f32; ZMAX] = [0.0; ZMAX];

pub fn open() {
    unsafe {
        if FB_BASE != 0 {
            return;
        }
        let mut info: [u64; 4] = [0; 4];
        let base = ling_sys_fb_map(info.as_mut_ptr() as u64);
        FB_BASE = base;
        FB_W = info[0] as u32;
        FB_H = info[1] as u32;
        FB_PITCH = info[2] as u32;
        FB_BPP = info[3] as u32;
    }
}

pub fn width() -> u32 {
    unsafe { FB_W }
}
pub fn height() -> u32 {
    unsafe { FB_H }
}

#[inline]
unsafe fn put(x: u32, y: u32, color: u32) {
    let bypp = FB_BPP / 8;
    let p = (FB_BASE + y as u64 * FB_PITCH as u64 + x as u64 * bypp as u64) as *mut u32;
    if bypp == 4 {
        core::ptr::write_volatile(p, color);
    } else {
        let pb = p as *mut u8;
        core::ptr::write_volatile(pb, (color & 0xFF) as u8);
        core::ptr::write_volatile(pb.add(1), ((color >> 8) & 0xFF) as u8);
        core::ptr::write_volatile(pb.add(2), ((color >> 16) & 0xFF) as u8);
    }
}

/// Clear the whole framebuffer to `color` and reset the depth buffer.
pub fn clear(color: u32) {
    unsafe {
        if FB_BASE == 0 {
            return;
        }
        let (w, h) = (FB_W, FB_H);
        let mut y = 0;
        while y < h {
            let mut x = 0;
            while x < w {
                put(x, y, color);
                x += 1;
            }
            y += 1;
        }
        let zn = ((w as usize) * (h as usize)).min(ZMAX);
        let z = &mut *&raw mut ZBUF;
        let mut i = 0;
        while i < zn {
            z[i] = 0.0;
            i += 1;
        }
    }
}

#[inline]
fn chan(v: f64) -> u32 {
    // Clamp a colour channel to 0..=255 (the caller passes bare f64s; packing
    // in native code truncates cleanly, avoiding the fractional-bit spill you
    // get trying to pack floats in Ling without a working floor()).
    if v <= 0.0 {
        0
    } else if v >= 255.0 {
        255
    } else {
        v as u32
    }
}

/// One depth-tested pixel at (x,y) with packed colour.
#[inline]
unsafe fn plot_px(x: i32, y: i32, ooz: f32, color: u32) {
    if x < 0 || y < 0 {
        return;
    }
    let (xu, yu) = (x as u32, y as u32);
    if xu >= FB_W || yu >= FB_H {
        return;
    }
    let idx = yu as usize * FB_W as usize + xu as usize;
    if idx >= ZMAX {
        return;
    }
    let z = &mut *&raw mut ZBUF;
    if ooz <= z[idx] {
        return;
    }
    z[idx] = ooz;
    put(xu, yu, color);
}

/// Depth-tested plot of a 2x2 block at (x,y) with colour from bare r,g,b
/// channels. The 2x2 block fills the gaps between torus samples so the donut
/// reads solid rather than as scattered dots.
pub fn plot(x: i32, y: i32, ooz: f32, r: f64, g: f64, b: f64) {
    unsafe {
        if FB_BASE == 0 {
            return;
        }
        let color = (chan(r) << 16) | (chan(g) << 8) | chan(b);
        plot_px(x, y, ooz, color);
        plot_px(x + 1, y, ooz, color);
        plot_px(x, y + 1, ooz, color);
        plot_px(x + 1, y + 1, ooz, color);
    }
}

/// Depth-tested filled disc of radius `rad` at (x,y) -- used to draw a torus
/// knot as a solid glowing tube rather than a thin wire.
pub fn plot_disc(x: i32, y: i32, ooz: f32, rad: i32, r: f64, g: f64, b: f64) {
    unsafe {
        if FB_BASE == 0 {
            return;
        }
        let color = (chan(r) << 16) | (chan(g) << 8) | chan(b);
        let r2 = rad * rad;
        let mut dy = -rad;
        while dy <= rad {
            let mut dx = -rad;
            while dx <= rad {
                if dx * dx + dy * dy <= r2 {
                    plot_px(x + dx, y + dy, ooz, color);
                }
                dx += 1;
            }
            dy += 1;
        }
    }
}

/// The last key pressed (0 if none), so an app can exit on a keypress.
pub fn poll_key() -> u64 {
    unsafe { (ling_sys_poll_input() >> 40) & 0xFF }
}
