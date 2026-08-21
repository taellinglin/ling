//! Multiboot2 linear framebuffer support. `boot32.rs` requests one in the
//! Multiboot2 header (a type-5 tag) and stashes GRUB's info-structure
//! pointer in the `mb2_info_ptr` symbol before Rust code ever runs; `init`
//! (called from `ling_kernel_init`) walks that structure's tags looking for
//! the type-8 framebuffer-info tag GRUB fills in in response. Text-mode VGA
//! (`vga.rs`/`term.rs`) stays the console the shell runs on — this is
//! additive, for graphics-mode programs (the window manager/desktop
//! environment) to draw into once they exist.
use core::ptr;

extern "C" {
    static mb2_info_ptr: u32;
}

#[derive(Clone, Copy)]
struct FbInfo {
    addr: u64,
    pitch: u32,
    width: u32,
    height: u32,
    bpp: u8,
}

static mut FB: Option<FbInfo> = None;

/// Parse the Multiboot2 info structure for a framebuffer tag. Safe to call
/// even if GRUB left us in text mode (no tag found, `FB` stays `None`) or on
/// a boot path with no Multiboot2 info at all (`mb2_info_ptr == 0`).
pub fn init() {
    unsafe {
        let info_ptr = ptr::read_volatile(&raw const mb2_info_ptr);
        if info_ptr == 0 {
            return;
        }
        let total_size = ptr::read_unaligned(info_ptr as *const u32);
        let mut offset: u32 = 8; // skip the fixed total_size+reserved header
        while offset + 8 <= total_size {
            let tag_ptr = (info_ptr + offset) as *const u32;
            let tag_type = ptr::read_unaligned(tag_ptr);
            let tag_size = ptr::read_unaligned(tag_ptr.add(1));
            if tag_type == 0 {
                break; // end tag
            }
            if tag_type == 8 && tag_size >= 29 {
                let base = info_ptr + offset;
                let addr = ptr::read_unaligned((base + 8) as *const u64);
                let pitch = ptr::read_unaligned((base + 16) as *const u32);
                let width = ptr::read_unaligned((base + 20) as *const u32);
                let height = ptr::read_unaligned((base + 24) as *const u32);
                let bpp = ptr::read_volatile((base + 28) as *const u8);
                ptr::write(&raw mut FB, Some(FbInfo { addr, pitch, width, height, bpp }));
                return;
            }
            // Tags are padded to 8-byte alignment; `tag_size` itself isn't.
            offset += (tag_size + 7) & !7;
        }
    }
}

fn get() -> Option<FbInfo> {
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

/// `color` is packed `0x00RRGGBB`; truncated to whatever the mode's actual
/// bit depth supports (24bpp/32bpp only — 15/16bpp packed modes aren't
/// handled and are silently ignored, same as an out-of-bounds pixel).
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
    for row in y..y.saturating_add(h) {
        for col in x..x.saturating_add(w) {
            set_pixel(col, row, color);
        }
    }
}

pub fn clear(color: u32) {
    let (w, h) = (width(), height());
    fill_rect(0, 0, w, h, color);
}

// ── Back buffer: draw a whole frame off-screen, then blit it in one shot ──
// Redrawing shapes directly into the live framebuffer (the functions above)
// tears visibly on every frame once anything moves — confirmed by an actual
// screendump catching a titlebar fill overwritten mid-redraw by the body
// fill after it. Drawing into this plain `.bss` buffer instead and blitting
// it with one `copy_nonoverlapping` at the end of a frame makes the visible
// update as close to atomic as this kernel can get without real vsync.
// Sized for the largest resolution GRUB is likely to hand back for a
// "basic" OS's test hardware; `present` clamps to whatever's actually
// negotiated, so a smaller real mode just uses the front part of it.
const BACKBUFFER_MAX: usize = 1920 * 1080 * 4;
static mut BACKBUFFER: [u8; BACKBUFFER_MAX] = [0u8; BACKBUFFER_MAX];

fn back_buf() -> &'static mut [u8; BACKBUFFER_MAX] {
    unsafe { &mut *&raw mut BACKBUFFER }
}

pub fn back_set_pixel(x: u32, y: u32, color: u32) {
    {
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
}

pub fn back_fill_rect(x: u32, y: u32, w: u32, h: u32, color: u32) {
    for row in y..y.saturating_add(h) {
        for col in x..x.saturating_add(w) {
            back_set_pixel(col, row, color);
        }
    }
}

pub fn back_clear(color: u32) {
    let (w, h) = (width(), height());
    back_fill_rect(0, 0, w, h, color);
}

/// Blit the back buffer to the real framebuffer in one copy — call once per
/// finished frame, after however many `back_*` draw calls it took to build
/// that frame.
pub fn present() {
    unsafe {
        let Some(fb) = get() else { return };
        let total = (fb.pitch as usize * fb.height as usize).min(BACKBUFFER_MAX);
        let dst = fb.addr as *mut u8;
        let buf = back_buf();
        ptr::copy_nonoverlapping(buf.as_ptr(), dst, total);
    }
}
