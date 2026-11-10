//! The picture clipboard in the no_std build, with the same API as the desktop's `clipboard` module. The host
//! can hand over its own system-wide picture clipboard ([`set_hooks`]: the LingOS kernel does, so a picture
//! copied in Files or LingDiffusion pastes here and ours pastes there); without hooks it is one picture kept
//! in memory.

#[allow(unused_imports)]
use crate::prelude::*;
use crate::os::sync::Mutex;

/// Straight (non-premultiplied) RGBA8 picture.
#[derive(Clone)]
pub struct ClipImage {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
    /// a file name to label the layer with, when it came from a copied file
    pub name: Option<String>,
}

pub fn sig(w: usize, h: usize, px: &[u8]) -> u64 {
    // FNV-1a over the size and the pixels
    let mut s: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |b: u8| {
        s ^= b as u64;
        s = s.wrapping_mul(0x0100_0000_01b3);
    };
    for b in (w as u64).to_le_bytes().into_iter().chain((h as u64).to_le_bytes()) {
        eat(b);
    }
    for &b in px {
        eat(b);
    }
    s
}

struct Slot(Mutex<Option<ClipImage>>);
// the engine runs on one thread in the kernel
unsafe impl Sync for Slot {}
static CLIP: Slot = Slot(Mutex::new_const_with(None));

/// The host's system clipboard: `get` the picture on it (straight RGBA8), `set` one (false when it is refused,
/// e.g. too big).
pub struct Hooks {
    pub get: fn() -> Option<ClipImage>,
    pub set: fn(usize, usize, &[u8]) -> bool,
}
static HOOKS: core::sync::atomic::AtomicPtr<Hooks> = core::sync::atomic::AtomicPtr::new(core::ptr::null_mut());

/// Use the host's clipboard from now on.
pub fn set_hooks(h: &'static Hooks) {
    HOOKS.store(h as *const Hooks as *mut Hooks, core::sync::atomic::Ordering::Release);
}
fn hooks() -> Option<&'static Hooks> {
    let p = HOOKS.load(core::sync::atomic::Ordering::Acquire);
    if p.is_null() {
        None
    } else {
        Some(unsafe { &*p })
    }
}

pub fn get() -> Option<ClipImage> {
    if let Some(h) = hooks() {
        return (h.get)();
    }
    CLIP.0.lock().ok().and_then(|c| c.clone())
}

pub fn set(w: usize, h: usize, px: Vec<u8>) -> Option<u64> {
    let s = sig(w, h, &px);
    if let Some(hk) = hooks() {
        return if (hk.set)(w, h, &px) { Some(s) } else { None };
    }
    *CLIP.0.lock().ok()? = Some(ClipImage { w, h, px, name: None });
    Some(s)
}
