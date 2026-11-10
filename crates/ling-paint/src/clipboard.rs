//! System clipboard images. Reads what other apps copy (a browser image, a screenshot, Clip Studio /
//! Photoshop / Krita selections, a picture file copied in Explorer, or a path to one as text) and writes our
//! own copies out as PNG + 32-bit DIB so they paste into other programs too.
//!
//! On Windows this talks to user32 directly: arboard's clipboard-win layer failed to read images back
//! (error 1418) on a machine where the plain API works.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Straight (non-premultiplied) RGBA8 picture.
pub struct ClipImage {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
    /// a file name to label the layer with, when it came from a copied file
    pub name: Option<String>,
}

pub fn sig(w: usize, h: usize, px: &[u8]) -> u64 {
    let mut s = DefaultHasher::new();
    (w, h).hash(&mut s);
    px.hash(&mut s);
    s.finish()
}

fn from_file(p: &std::path::Path) -> Option<ClipImage> {
    let img = image::open(p).ok()?.to_rgba8();
    let name = p.file_stem().map(|s| s.to_string_lossy().to_string());
    Some(ClipImage { w: img.width() as usize, h: img.height() as usize, px: img.into_raw(), name })
}

fn from_text(t: &str) -> Option<ClipImage> {
    let t = t.trim().trim_matches('"');
    let p = std::path::Path::new(t);
    if t.len() < 1024 && p.is_file() {
        from_file(p)
    } else {
        None
    }
}

fn from_png(b: &[u8]) -> Option<ClipImage> {
    let img = image::load_from_memory_with_format(b, image::ImageFormat::Png).ok()?.to_rgba8();
    Some(ClipImage { w: img.width() as usize, h: img.height() as usize, px: img.into_raw(), name: None })
}

/// A packed DIB (header + optional masks / palette + pixels) decoded through a synthesised BMP file.
#[allow(dead_code)]
fn from_dib(d: &[u8]) -> Option<ClipImage> {
    if d.len() < 40 {
        return None;
    }
    let rd = |o: usize| u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
    let hs = rd(0) as usize;
    let bpp = u16::from_le_bytes([d[14], d[15]]) as usize;
    let comp = rd(16);
    let used = rd(32) as usize;
    let masks = if hs == 40 && comp == 3 { 12 } else if hs == 40 && comp == 6 { 16 } else { 0 };
    let pal = if bpp <= 8 { (if used > 0 { used } else { 1 << bpp }) * 4 } else { used * 4 };
    let off = 14 + hs + masks + pal;
    let mut f = Vec::with_capacity(14 + d.len());
    f.extend_from_slice(b"BM");
    f.extend_from_slice(&((14 + d.len()) as u32).to_le_bytes());
    f.extend_from_slice(&[0, 0, 0, 0]);
    f.extend_from_slice(&(off as u32).to_le_bytes());
    f.extend_from_slice(d);
    let mut img = image::load_from_memory_with_format(&f, image::ImageFormat::Bmp).ok()?.to_rgba8();
    // 32-bit DIBs from many apps leave alpha at 0: treat that as opaque
    if bpp == 32 && img.pixels().all(|p| p[3] == 0) {
        img.pixels_mut().for_each(|p| p[3] = 255);
    }
    Some(ClipImage { w: img.width() as usize, h: img.height() as usize, px: img.into_raw(), name: None })
}

/// A bottom-up 32-bit BITMAPV5HEADER DIB with alpha, the format Windows apps read with transparency.
#[allow(dead_code)]
fn to_dibv5(w: usize, h: usize, px: &[u8]) -> Vec<u8> {
    let mut d = Vec::with_capacity(124 + w * h * 4);
    let p32 = |d: &mut Vec<u8>, v: u32| d.extend_from_slice(&v.to_le_bytes());
    p32(&mut d, 124);
    p32(&mut d, w as u32);
    p32(&mut d, h as u32);
    d.extend_from_slice(&1u16.to_le_bytes());
    d.extend_from_slice(&32u16.to_le_bytes());
    p32(&mut d, 3); // BI_BITFIELDS
    p32(&mut d, (w * h * 4) as u32);
    p32(&mut d, 2835);
    p32(&mut d, 2835);
    p32(&mut d, 0);
    p32(&mut d, 0);
    p32(&mut d, 0x00FF_0000);
    p32(&mut d, 0x0000_FF00);
    p32(&mut d, 0x0000_00FF);
    p32(&mut d, 0xFF00_0000);
    p32(&mut d, 0x7352_4742); // 'sRGB'
    d.extend_from_slice(&[0u8; 36]); // endpoints
    d.extend_from_slice(&[0u8; 12]); // gamma
    p32(&mut d, 4); // LCS_GM_IMAGES
    d.extend_from_slice(&[0u8; 12]); // profile data / size / reserved
    for y in (0..h).rev() {
        for x in 0..w {
            let o = (y * w + x) * 4;
            d.extend_from_slice(&[px[o + 2], px[o + 1], px[o], px[o + 3]]);
        }
    }
    d
}

fn to_png(w: usize, h: usize, px: &[u8]) -> Option<Vec<u8>> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::write_buffer_with_format(&mut out, px, w as u32, h as u32, image::ExtendedColorType::Rgba8, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    #[link(name = "user32")]
    extern "system" {
        fn OpenClipboard(h: isize) -> i32;
        fn CloseClipboard() -> i32;
        fn EmptyClipboard() -> i32;
        fn IsClipboardFormatAvailable(f: u32) -> i32;
        fn GetClipboardData(f: u32) -> isize;
        fn SetClipboardData(f: u32, h: isize) -> isize;
        fn RegisterClipboardFormatW(name: *const u16) -> u32;
        fn CreateWindowExW(ex: u32, class: *const u16, title: *const u16, style: u32, x: i32, y: i32, w: i32, h: i32, parent: isize, menu: isize, inst: isize, param: *const c_void) -> isize;
    }
    thread_local! {
        // A hidden message-only window that owns what we put on the clipboard. With a NULL owner Windows
        // accepts the data but this process can no longer read it back.
        static OWNER: isize = {
            let class: Vec<u16> = "STATIC".encode_utf16().chain(Some(0)).collect();
            unsafe { CreateWindowExW(0, class.as_ptr(), std::ptr::null(), 0, 0, 0, 0, 0, -3, 0, 0, std::ptr::null()) }
        };
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalAlloc(flags: u32, n: usize) -> isize;
        fn GlobalFree(h: isize) -> isize;
        fn GlobalLock(h: isize) -> *mut c_void;
        fn GlobalUnlock(h: isize) -> i32;
        fn GlobalSize(h: isize) -> usize;
    }
    #[link(name = "shell32")]
    extern "system" {
        fn DragQueryFileW(h: isize, i: u32, buf: *mut u16, n: u32) -> u32;
    }
    pub const CF_UNICODETEXT: u32 = 13;
    pub const CF_HDROP: u32 = 15;
    pub const CF_DIB: u32 = 8;
    pub const CF_DIBV5: u32 = 17;

    /// The clipboard, open for this thread until dropped (retries while another app holds it).
    pub struct Open;
    impl Open {
        pub fn new() -> Option<Open> {
            for _ in 0..20 {
                if unsafe { OpenClipboard(OWNER.with(|h| *h)) } != 0 {
                    return Some(Open);
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            None
        }
        pub fn format(name: &str) -> u32 {
            let w: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            unsafe { RegisterClipboardFormatW(w.as_ptr()) }
        }
        pub fn has(&self, f: u32) -> bool {
            f != 0 && unsafe { IsClipboardFormatAvailable(f) } != 0
        }
        pub fn bytes(&self, f: u32) -> Option<Vec<u8>> {
            if !self.has(f) {
                return None;
            }
            unsafe {
                let h = GetClipboardData(f);
                if h == 0 {
                    return None;
                }
                let n = GlobalSize(h);
                let p = GlobalLock(h) as *const u8;
                if p.is_null() || n == 0 {
                    return None;
                }
                let v = std::slice::from_raw_parts(p, n).to_vec();
                GlobalUnlock(h);
                Some(v)
            }
        }
        pub fn text(&self) -> Option<String> {
            let b = self.bytes(CF_UNICODETEXT)?;
            let w: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
            Some(String::from_utf16_lossy(&w))
        }
        pub fn files(&self) -> Vec<std::path::PathBuf> {
            if !self.has(CF_HDROP) {
                return vec![];
            }
            unsafe {
                let h = GetClipboardData(CF_HDROP);
                if h == 0 {
                    return vec![];
                }
                let n = DragQueryFileW(h, u32::MAX, std::ptr::null_mut(), 0);
                (0..n)
                    .map(|i| {
                        let len = DragQueryFileW(h, i, std::ptr::null_mut(), 0) as usize;
                        let mut buf = vec![0u16; len + 1];
                        DragQueryFileW(h, i, buf.as_mut_ptr(), buf.len() as u32);
                        std::path::PathBuf::from(String::from_utf16_lossy(&buf[..len]))
                    })
                    .collect()
            }
        }
        pub fn clear(&self) -> bool {
            unsafe { EmptyClipboard() != 0 }
        }
        pub fn put(&self, f: u32, data: &[u8]) -> bool {
            unsafe {
                let h = GlobalAlloc(2, data.len()); // GMEM_MOVEABLE
                if h == 0 {
                    return false;
                }
                let p = GlobalLock(h) as *mut u8;
                if p.is_null() {
                    GlobalFree(h);
                    return false;
                }
                std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
                GlobalUnlock(h);
                if SetClipboardData(f, h) == 0 {
                    GlobalFree(h);
                    return false;
                }
                true
            }
        }
    }
    impl Drop for Open {
        fn drop(&mut self) {
            unsafe { CloseClipboard() };
        }
    }
}

/// The picture on the system clipboard, if there is one.
#[cfg(windows)]
pub fn get() -> Option<ClipImage> {
    let cb = win::Open::new()?;
    for name in ["PNG", "image/png"] {
        if let Some(c) = cb.bytes(win::Open::format(name)).and_then(|b| from_png(&b)) {
            return Some(c);
        }
    }
    for f in [win::CF_DIBV5, win::CF_DIB] {
        if let Some(c) = cb.bytes(f).and_then(|b| from_dib(&b)) {
            return Some(c);
        }
    }
    let files = cb.files();
    let text = cb.text();
    drop(cb); // decode files with the clipboard closed again
    files.iter().find_map(|f| from_file(f)).or_else(|| text.and_then(|t| from_text(&t)))
}

/// Puts a straight RGBA8 picture on the system clipboard; returns its signature.
#[cfg(windows)]
pub fn set(w: usize, h: usize, px: Vec<u8>) -> Option<u64> {
    let s = sig(w, h, &px);
    let png = to_png(w, h, &px)?;
    let dib = to_dibv5(w, h, &px);
    let cb = win::Open::new()?;
    if !cb.clear() {
        return None;
    }
    let a = cb.put(win::CF_DIBV5, &dib);
    let b = cb.put(win::Open::format("PNG"), &png);
    (a || b).then_some(s)
}

#[cfg(not(windows))]
pub fn get() -> Option<ClipImage> {
    let mut cb = arboard::Clipboard::new().ok()?;
    if let Ok(img) = cb.get_image() {
        if img.width > 0 && img.height > 0 && img.bytes.len() >= img.width * img.height * 4 {
            return Some(ClipImage { w: img.width, h: img.height, px: img.bytes.into_owned(), name: None });
        }
    }
    if let Some(c) = cb.get().file_list().ok().and_then(|f| f.iter().find_map(|f| from_file(f))) {
        return Some(c);
    }
    cb.get_text().ok().and_then(|t| from_text(&t))
}

#[cfg(not(windows))]
pub fn set(w: usize, h: usize, px: Vec<u8>) -> Option<u64> {
    let s = sig(w, h, &px);
    let _ = to_png; // PNG encoding is the Windows path's; arboard encodes for itself
    let mut cb = arboard::Clipboard::new().ok()?;
    cb.set_image(arboard::ImageData { width: w, height: h, bytes: px.into() }).ok()?;
    Some(s)
}
