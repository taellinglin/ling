//! Linear framebuffer support across x86_64 (Multiboot2) and aarch64 (BCM2835 mailbox).

use core::ptr;

#[cfg(target_arch = "x86_64")]
extern "C" {
    static mb2_info_ptr: u32;
}

#[derive(Clone, Copy)]
pub struct FbInfo {
    pub addr: u64,
    pub pitch: u32,
    pub width: u32,
    pub height: u32,
    pub bpp: u8,
}

static mut FB: Option<FbInfo> = None;

const BACKBUFFER_MAX: usize = 1920 * 1080 * 4;
static mut BACKBUFFER: [u8; BACKBUFFER_MAX] = [0u8; BACKBUFFER_MAX];

fn back_buf() -> &'static mut [u8; BACKBUFFER_MAX] {
    unsafe { &mut *&raw mut BACKBUFFER }
}

/// Physical address + magic of the VBE handoff block the disk-boot
/// stage2 leaves behind (bootloader/stage2.asm's LFB_INFO_ADDR/_MAGIC --
/// keep in sync). Layout: magic, physbase, pitch, width, height, bpp,
/// each a u32.
#[cfg(target_arch = "x86_64")]
const LFB_INFO_ADDR: u64 = 0x6000;
#[cfg(target_arch = "x86_64")]
const LFB_INFO_MAGIC: u32 = 0x4942_464C; // "LFBI"

/// True when this kernel was loaded by the disk-boot path (stage1/stage2,
/// no GRUB): there is no Multiboot2 info structure at all. The desktop
/// uses this to decide between the Live auto-login flow and the installed
/// system's login greeter.
#[cfg(target_arch = "x86_64")]
pub fn booted_from_disk() -> bool {
    unsafe { ptr::read_volatile(&raw const mb2_info_ptr) == 0 }
}

pub fn init() {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let info_ptr = ptr::read_volatile(&raw const mb2_info_ptr);
        if info_ptr == 0 {
            // Disk boot: no Multiboot2 info exists. stage2 may have set a
            // VBE mode and left its tagged handoff block instead.
            if ptr::read_volatile(LFB_INFO_ADDR as *const u32) == LFB_INFO_MAGIC {
                let addr = ptr::read_volatile((LFB_INFO_ADDR + 4) as *const u32) as u64;
                let pitch = ptr::read_volatile((LFB_INFO_ADDR + 8) as *const u32);
                let width = ptr::read_volatile((LFB_INFO_ADDR + 12) as *const u32);
                let height = ptr::read_volatile((LFB_INFO_ADDR + 16) as *const u32);
                let bpp = ptr::read_volatile((LFB_INFO_ADDR + 20) as *const u32) as u8;
                if addr != 0 && width > 0 && height > 0 && (bpp == 24 || bpp == 32) {
                    ptr::write(&raw mut FB, Some(FbInfo { addr, pitch, width, height, bpp }));
                }
            }
            return;
        }
        let total_size = ptr::read_unaligned(info_ptr as *const u32);
        let mut offset: u32 = 8;
        while offset + 8 <= total_size {
            let tag_ptr = (info_ptr + offset) as *const u32;
            let tag_type = ptr::read_unaligned(tag_ptr);
            let tag_size = ptr::read_unaligned(tag_ptr.add(1));
            if tag_type == 0 {
                break;
            }
            if tag_type == 8 && tag_size >= 29 {
                let base = info_ptr + offset;
                let addr = ptr::read_unaligned((base + 8) as *const u64);
                let pitch = ptr::read_unaligned((base + 16) as *const u32);
                let width = ptr::read_unaligned((base + 20) as *const u32);
                let height = ptr::read_unaligned((base + 24) as *const u32);
                let bpp = ptr::read_volatile((base + 28) as *const u8);
                ptr::write(
                    &raw mut FB,
                    Some(FbInfo { addr, pitch, width, height, bpp }),
                );
                return;
            }
            offset += (tag_size + 7) & !7;
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        if let Some((addr, pitch, width, height, bpp)) =
            crate::arch::aarch64::mailbox::allocate_framebuffer(1024, 768, 32)
        {
            unsafe {
                ptr::write(&raw mut FB, Some(FbInfo { addr, pitch, width, height, bpp }));
            }
        }
    }
}

pub fn get() -> Option<FbInfo> {
    unsafe { ptr::read(&raw const FB) }
}

pub fn available() -> bool {
    get().is_some()
}

pub fn width() -> u32 {
    get().map(|f| f.width).unwrap_or(0)
}

pub fn height() -> u32 {
    get().map(|f| f.height).unwrap_or(0)
}

pub fn set_pixel(x: u32, y: u32, color: u32) {
    unsafe {
        let Some(fb) = get() else { return };
        if x >= fb.width || y >= fb.height {
            return;
        }
        let bypp = fb.bpp as u32 / 8;
        let byte_offset = y as u64 * fb.pitch as u64 + x as u64 * bypp as u64;
        let p = (fb.addr + byte_offset) as *mut u8;
        match bypp {
            4 => ptr::write_volatile(p as *mut u32, color),
            3 => {
                ptr::write_volatile(p, (color & 0xFF) as u8);
                ptr::write_volatile(p.add(1), ((color >> 8) & 0xFF) as u8);
                ptr::write_volatile(p.add(2), ((color >> 16) & 0xFF) as u8);
            },
            _ => {},
        }
    }
}

pub fn fill_rect(x: u32, y: u32, w: u32, h: u32, color: u32) {
    let Some(fb) = get() else { return };
    if x >= fb.width || y >= fb.height {
        return;
    }
    let w = w.min(fb.width - x);
    let h = h.min(fb.height - y);
    if w == 0 || h == 0 {
        return;
    }
    let bypp = fb.bpp as u32 / 8;
    if bypp == 4 {
        for row in y..y + h {
            let row_start = (fb.addr + row as u64 * fb.pitch as u64 + x as u64 * 4) as *mut u32;
            for col in 0..w as usize {
                unsafe { ptr::write_volatile(row_start.add(col), color) };
            }
        }
    } else if bypp == 3 {
        let b0 = (color & 0xFF) as u8;
        let b1 = ((color >> 8) & 0xFF) as u8;
        let b2 = ((color >> 16) & 0xFF) as u8;
        for row in y..y + h {
            let mut p = (fb.addr + row as u64 * fb.pitch as u64 + x as u64 * 3) as *mut u8;
            for _ in 0..w {
                unsafe {
                    ptr::write_volatile(p, b0);
                    ptr::write_volatile(p.add(1), b1);
                    ptr::write_volatile(p.add(2), b2);
                    p = p.add(3);
                }
            }
        }
    }
}

pub fn clear(color: u32) {
    let (w, h) = (width(), height());
    fill_rect(0, 0, w, h, color);
}

pub fn back_set_pixel(x: u32, y: u32, color: u32) {
    let Some(fb) = get() else { return };
    if x >= fb.width || y >= fb.height {
        return;
    }
    let bypp = fb.bpp as u32 / 8;
    let offset = y as usize * fb.pitch as usize + x as usize * bypp as usize;
    if offset + bypp as usize > BACKBUFFER_MAX {
        return;
    }
    let buf = back_buf();
    match bypp {
        4 => buf[offset..offset + 4].copy_from_slice(&color.to_le_bytes()),
        3 => {
            buf[offset] = (color & 0xFF) as u8;
            buf[offset + 1] = ((color >> 8) & 0xFF) as u8;
            buf[offset + 2] = ((color >> 16) & 0xFF) as u8;
        },
        _ => {},
    }
}

pub fn back_fill_rect(x: u32, y: u32, w: u32, h: u32, color: u32) {
    let Some(fb) = get() else { return };
    if x >= fb.width || y >= fb.height {
        return;
    }
    let w = w.min(fb.width - x);
    let h = h.min(fb.height - y);
    if w == 0 || h == 0 {
        return;
    }
    let bypp = fb.bpp as u32 / 8;
    let buf = back_buf();
    if bypp == 4 {
        let col_bytes = color.to_le_bytes();
        for row in y..y + h {
            let row_offset = row as usize * fb.pitch as usize + x as usize * 4;
            if row_offset + (w as usize) * 4 <= BACKBUFFER_MAX {
                for col in 0..w as usize {
                    let off = row_offset + col * 4;
                    buf[off..off + 4].copy_from_slice(&col_bytes);
                }
            }
        }
    } else if bypp == 3 {
        let b0 = (color & 0xFF) as u8;
        let b1 = ((color >> 8) & 0xFF) as u8;
        let b2 = ((color >> 16) & 0xFF) as u8;
        for row in y..y + h {
            let row_offset = row as usize * fb.pitch as usize + x as usize * 3;
            if row_offset + (w as usize) * 3 <= BACKBUFFER_MAX {
                for col in 0..w as usize {
                    let off = row_offset + col * 3;
                    buf[off] = b0;
                    buf[off + 1] = b1;
                    buf[off + 2] = b2;
                }
            }
        }
    }
}

pub fn back_clear(color: u32) {
    let (w, h) = (width(), height());
    back_fill_rect(0, 0, w, h, color);
}

/// Integer square root (largest `r` with `r*r <= v`) -- the corner-arc code
/// below needs sqrt but this crate is no_std with no libm (same constraint
/// `wm_liquid::scale_w_pct` documents), and Newton over u32 converges in a
/// handful of iterations at these magnitudes (radii are tens of pixels).
fn isqrt(v: u32) -> u32 {
    if v < 2 {
        return v;
    }
    let mut x = v;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + v / x) / 2;
    }
    x
}

/// Horizontal inset of a rounded rect's row `dy` (0-based from the top)
/// for corner radius `r` in a rect of height `h`: 0 for the straight middle
/// rows, quarter-circle-shaped for the top/bottom `r` rows. Shared by the
/// fill and blend variants so the two can never disagree on shape.
fn rounded_row_inset(dy: u32, h: u32, r: u32) -> u32 {
    let r = r.min(h / 2);
    if r == 0 {
        return 0;
    }
    let d = if dy < r {
        r - dy
    } else if dy >= h - r {
        dy - (h - 1 - r)
    } else {
        return 0;
    };
    if d > r {
        return r;
    }
    r - isqrt(r * r - d * d)
}

/// Filled rounded rectangle into the back buffer: plain row spans with
/// quarter-circle insets on the top/bottom `radius` rows. No antialiasing
/// -- honest hard-edged spans, same as every other primitive here.
pub fn back_fill_rounded_rect(x: u32, y: u32, w: u32, h: u32, radius: u32, color: u32) {
    if h == 0 || w == 0 {
        return;
    }
    for dy in 0..h {
        let inset = rounded_row_inset(dy, h, radius).min(w / 2);
        back_fill_rect(x + inset, y + dy, w - inset * 2, 1, color);
    }
}

/// Filled circle into the back buffer -- row spans via `isqrt`, the same
/// arc math as `rounded_row_inset` without the straight middle section.
pub fn back_fill_circle(cx: u32, cy: u32, radius: u32, color: u32) {
    if radius == 0 {
        return;
    }
    let r = radius as i64;
    for dy in -r..=r {
        let e = isqrt((r * r - dy * dy) as u32) as i64;
        let row_y = cy as i64 + dy;
        let row_x = cx as i64 - e;
        if row_y < 0 || row_x + 2 * e < 0 {
            continue;
        }
        back_fill_rect(row_x.max(0) as u32, row_y as u32, (2 * e + 1) as u32, 1, color);
    }
}

/// Alpha-blend one back-buffer pixel: `out = src*a + dst*(255-a)` per
/// channel, `alpha` in 0..=255. Reads the back buffer (not the live
/// framebuffer) so blending composes correctly with everything drawn
/// earlier this frame, before the single `present()` blit.
fn back_blend_pixel(x: u32, y: u32, color: u32, alpha: u32) {
    let Some(fb) = get() else { return };
    if x >= fb.width || y >= fb.height {
        return;
    }
    let bypp = fb.bpp as u32 / 8;
    let offset = y as usize * fb.pitch as usize + x as usize * bypp as usize;
    if offset + bypp as usize > BACKBUFFER_MAX || (bypp != 3 && bypp != 4) {
        return;
    }
    let buf = back_buf();
    let inv = 255 - alpha;
    for ch in 0..3usize {
        let src = (color >> (ch * 8)) & 0xFF;
        let dst = buf[offset + ch] as u32;
        buf[offset + ch] = ((src * alpha + dst * inv) / 255) as u8;
    }
}

/// Alpha-blended rectangle into the back buffer, `alpha` 0..=255 (255 =
/// opaque, use `back_fill_rect` instead there -- this is per-pixel
/// read-modify-write and costs accordingly). This is what makes real
/// drop shadows and glass effects possible; before it existed every
/// "shadow" would have been a fake solid grey.
pub fn back_blend_rect(x: u32, y: u32, w: u32, h: u32, color: u32, alpha: u32) {
    let alpha = alpha.min(255);
    if alpha == 0 {
        return;
    }
    for dy in 0..h {
        for dx in 0..w {
            back_blend_pixel(x + dx, y + dy, color, alpha);
        }
    }
}

/// Alpha-blended rounded rectangle -- the drop-shadow primitive (a dark
/// blended rounded rect offset a few pixels under a window reads as soft
/// shadow even without a real gaussian falloff).
pub fn back_blend_rounded_rect(x: u32, y: u32, w: u32, h: u32, radius: u32, color: u32, alpha: u32) {
    let alpha = alpha.min(255);
    if alpha == 0 || h == 0 {
        return;
    }
    for dy in 0..h {
        let inset = rounded_row_inset(dy, h, radius).min(w / 2);
        for dx in inset..w - inset {
            back_blend_pixel(x + dx, y + dy, color, alpha);
        }
    }
}

pub fn present() {
    unsafe {
        let Some(fb) = get() else { return };
        let total = (fb.pitch as usize * fb.height as usize).min(BACKBUFFER_MAX);
        let dst = fb.addr as *mut u8;
        let buf = back_buf();
        ptr::copy_nonoverlapping(buf.as_ptr(), dst, total);
    }
}
