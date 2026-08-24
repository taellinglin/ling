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

pub fn init() {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let info_ptr = ptr::read_volatile(&raw const mb2_info_ptr);
        if info_ptr == 0 {
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

pub fn present() {
    unsafe {
        let Some(fb) = get() else { return };
        let total = (fb.pitch as usize * fb.height as usize).min(BACKBUFFER_MAX);
        let dst = fb.addr as *mut u8;
        let buf = back_buf();
        ptr::copy_nonoverlapping(buf.as_ptr(), dst, total);
    }
}
