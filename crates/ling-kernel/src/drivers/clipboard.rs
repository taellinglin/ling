//! System clipboard: one process-wide text buffer that every app shares,
//! so Ctrl+C in the terminal and Ctrl+V in the editor (or Files, or the
//! browser's URL bar) move the same bytes. Deliberately a single global
//! (no MIME types, no multiple selections, no X-style primary/clipboard
//! split) -- the simplest thing that makes cut/copy/paste universal, which
//! is the actual request. Plain UTF-8 text only; a "copy" of anything
//! non-text (a file in Files) stores its path as text, which is what the
//! paste targets can use.

const CLIP_MAX: usize = 16 * 1024;
static mut CLIP: [u8; CLIP_MAX] = [0; CLIP_MAX];
static mut CLIP_LEN: usize = 0;

pub fn set(bytes: &[u8]) {
    let n = bytes.len().min(CLIP_MAX);
    unsafe {
        let buf = &mut *&raw mut CLIP;
        buf[..n].copy_from_slice(&bytes[..n]);
        CLIP_LEN = n;
    }
}

pub fn set_str(s: &str) {
    set(s.as_bytes());
}

pub fn get() -> &'static [u8] {
    unsafe { &(&*&raw const CLIP)[..CLIP_LEN] }
}

pub fn len() -> usize {
    unsafe { CLIP_LEN }
}

pub fn is_empty() -> bool {
    unsafe { CLIP_LEN == 0 }
}

// ── the picture clipboard ─────────────────────────────────────────────────────
//
// One picture shared by every app (Files, LingDiffusion, LingAnime through
// ling-paint's clipboard hooks): straight RGBA8 pixels plus, when it came from
// a file, that file's path and original bytes (so a pasted PNG keeps its
// metadata, e.g. LingDiffusion's generation parameters) and a "cut" flag (the
// next paste into a folder moves the file). Plain Rust bytes, never Ling
// values: nothing here for the GC to root.

use alloc::string::String;
use alloc::vec::Vec;

/// the pixels of the biggest picture kept (64 MiB: 4096 x 4096)
pub const IMG_MAX: usize = 64 << 20;
/// the original file is kept for lossless pastes up to this size
const BYTES_MAX: usize = 32 << 20;

pub struct ClipImage {
    pub w: usize,
    pub h: usize,
    /// straight RGBA8, row-major
    pub rgba: Vec<u8>,
    /// the file it was copied from ("" for pixels from an app)
    pub src: String,
    /// that file's bytes, when small enough (else empty)
    pub bytes: Vec<u8>,
    /// copied with Cut: the next paste into a folder moves `src`
    pub cut: bool,
}

static mut IMG: Option<ClipImage> = None;
/// bumped on every change (a cheap "is it still mine" test for apps)
static mut IMG_SEQ: u64 = 0;

pub fn image() -> Option<&'static ClipImage> {
    unsafe { (*(&raw const IMG)).as_ref() }
}

pub fn image_seq() -> u64 {
    unsafe { IMG_SEQ }
}

pub fn image_clear() {
    unsafe {
        *(&raw mut IMG) = None;
        IMG_SEQ += 1;
    }
}

/// Pixels from an app (straight RGBA8). False when too big or malformed.
pub fn image_set_rgba(w: usize, h: usize, rgba: Vec<u8>, src: &str, bytes: Vec<u8>, cut: bool) -> bool {
    if w == 0 || h == 0 || w.saturating_mul(h).saturating_mul(4) > IMG_MAX || rgba.len() != w * h * 4 {
        return false;
    }
    let bytes = if bytes.len() <= BYTES_MAX { bytes } else { Vec::new() };
    unsafe {
        // the old picture goes first: two 64 MiB pictures need not fit at once
        *(&raw mut IMG) = None;
        *(&raw mut IMG) = Some(ClipImage { w, h, rgba, src: String::from(src), bytes, cut });
        IMG_SEQ += 1;
    }
    true
}

/// Is this file name a picture the clipboard can read?
pub fn is_image_name(p: &str) -> bool {
    let l = p.to_ascii_lowercase();
    l.ends_with(".png") || l.ends_with(".bmp") || l.ends_with(".svg")
}

/// A picture file as straight RGBA8 (PNG of any depth / palette, BMP 24/32, SVG).
pub fn decode_rgba(data: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
    use crate::drivers::image;
    if ling_paint::os::png::is_png(data) {
        let (w, h) = ling_paint::os::png::dimensions(data)?;
        if w.saturating_mul(h).saturating_mul(4) > IMG_MAX {
            return None;
        }
        let pic = ling_paint::os::png::decode(data).ok()?;
        let rgba: Vec<u8> = pic.px.iter().map(|&v| (v >> 8) as u8).collect();
        return Some((pic.w, pic.h, rgba));
    }
    let img = if image::looks_svg(data) { image::decode_svg_alpha(data, 2048) } else { image::decode_bmp_rgba(data) }?;
    let (w, h) = (img.w as usize, img.h as usize);
    if w.saturating_mul(h).saturating_mul(4) > IMG_MAX {
        return None;
    }
    let mut rgba = Vec::with_capacity(w * h * 4);
    for &p in &img.px {
        rgba.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8, (p >> 24) as u8]);
    }
    Some((w, h, rgba))
}

/// Copy (or cut) a picture file: its pixels, path and bytes. False when it is not a picture we can read.
pub fn image_copy_file(path: &str, cut: bool) -> bool {
    let Some(data) = crate::vfs::read(path) else { return false };
    let Some((w, h, rgba)) = decode_rgba(&data) else { return false };
    image_set_rgba(w, h, rgba, path, data, cut)
}

fn ext_of(p: &str) -> String {
    let b = crate::vfs::basename(p);
    match b.rfind('.') {
        Some(i) if i > 0 => b[i..].to_ascii_lowercase(),
        _ => String::new(),
    }
}

fn join(dir: &str, name: &str) -> String {
    let d = dir.trim_end_matches('/');
    if d.is_empty() {
        String::from(name)
    } else {
        alloc::format!("{d}/{name}")
    }
}

/// `path`, or `stem (2).ext`, `(3)`, ... next to it when that is taken.
pub fn unique_path(path: &str) -> String {
    if !crate::vfs::exists(path) {
        return String::from(path);
    }
    let dir = crate::vfs::parent(path);
    let base = crate::vfs::basename(path);
    let (stem, ext) = match base.rfind('.') {
        Some(i) if i > 0 => (&base[..i], &base[i..]),
        _ => (base.as_str(), ""),
    };
    for n in 2..10_000 {
        let p = join(&dir, &alloc::format!("{stem} ({n}){ext}"));
        if !crate::vfs::exists(&p) {
            return p;
        }
    }
    String::from(path)
}

/// Write the clipboard picture as a file at `path` (a free name next to it when taken). A cut file is moved
/// there (then the clipboard holds a plain copy of the new file); a copied file keeps its own bytes when the
/// extension matches; anything else becomes a PNG (the extension is made `.png`). The path written, or None.
pub fn image_paste_to(path: &str) -> Option<String> {
    let c = unsafe { (*(&raw mut IMG)).as_mut()? };
    let src_ext = ext_of(&c.src);
    let mut dst = String::from(path);
    let same_ext = !c.src.is_empty() && ext_of(&dst) == src_ext;
    if !same_ext && ext_of(&dst) != ".png" {
        let b = crate::vfs::basename(&dst);
        let stem = match b.rfind('.') {
            Some(i) if i > 0 => &b[..i],
            _ => b.as_str(),
        };
        dst = join(&crate::vfs::parent(&dst), &alloc::format!("{stem}.png"));
    }
    let src_there = !c.src.is_empty() && crate::vfs::exists(&c.src);
    let keep = same_ext && (!c.bytes.is_empty() || src_there);
    // pasting a file onto itself makes a copy next to it
    let dst = unique_path(&dst);
    let parent = crate::vfs::parent(&dst);
    if !parent.is_empty() && !crate::vfs::is_dir(&parent) {
        if crate::fs::media::is_media_path(&parent) {
            crate::fs::media::mkdir_p(&parent);
        } else {
            crate::vfs::mkdir(&parent);
        }
    }
    let png = |c: &ClipImage| ling_paint::os::png::encode_rgba8(c.w, c.h, &c.rgba);
    if c.cut && src_there {
        let ok = if keep { crate::vfs::rename(&c.src, &dst) } else { crate::vfs::write(&dst, &png(c)) && crate::vfs::delete(&c.src) };
        if !ok {
            return None;
        }
        c.cut = false;
        c.src = dst.clone();
        unsafe { IMG_SEQ += 1 };
        return Some(dst);
    }
    let ok = if keep && !c.bytes.is_empty() {
        crate::vfs::write(&dst, &c.bytes)
    } else if keep {
        crate::vfs::copy(&c.src, &dst)
    } else {
        crate::vfs::write(&dst, &png(c))
    };
    if !ok {
        return None;
    }
    // a cut whose file is gone pasted as a copy; later pastes are copies too
    c.cut = false;
    Some(dst)
}

/// Paste into the folder `dir`, under the copied file's name (or "Pasted image.png").
pub fn image_paste_into(dir: &str) -> Option<String> {
    let c = image()?;
    let name = if c.src.is_empty() { String::from("Pasted image.png") } else { crate::vfs::basename(&c.src) };
    image_paste_to(&join(dir, &name))
}

/// A pending Cut's source path, clearing the flag (the caller moves the file itself); "" when none.
pub fn image_take_cut() -> String {
    unsafe {
        match (*(&raw mut IMG)).as_mut() {
            Some(c) if c.cut => {
                c.cut = false;
                IMG_SEQ += 1;
                c.src.clone()
            },
            _ => String::new(),
        }
    }
}

// ling-paint (LingAnime) reads and writes this clipboard
fn paint_get() -> Option<ling_paint::clipboard::ClipImage> {
    let c = image()?;
    let name = if c.src.is_empty() {
        None
    } else {
        let b = crate::vfs::basename(&c.src);
        Some(match b.rfind('.') {
            Some(i) if i > 0 => String::from(&b[..i]),
            _ => b,
        })
    };
    Some(ling_paint::clipboard::ClipImage { w: c.w, h: c.h, px: c.rgba.clone(), name })
}
fn paint_set(w: usize, h: usize, px: &[u8]) -> bool {
    image_set_rgba(w, h, px.to_vec(), "", Vec::new(), false)
}
pub static PAINT_HOOKS: ling_paint::clipboard::Hooks = ling_paint::clipboard::Hooks { get: paint_get, set: paint_set };
