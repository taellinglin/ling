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
