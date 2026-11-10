//! The no_std side of the engine (built without the `std` feature, for the LingOS kernel).
//!
//! The host hands over what an OS would give: a clock, files (by path, the host resolves them), and a parallel
//! `for` over its worker cores - see [`Host`] and [`set_host`]. Everything else lives here: sequential stand-ins
//! for rayon's slice / range adaptors, a `Mutex` / `OnceLock` for a single-threaded engine, libm float maths, a
//! PNG codec (8 / 16 bit), a zip reader / writer for .lanime projects, and the slice of the `image` crate's API
//! the engine uses.

#![allow(dead_code)]

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicPtr, Ordering};

pub mod image;
pub mod png;
pub mod sync;
pub mod zip;

/// What the host supplies. Paths are passed through as the app gave them (the host resolves them).
pub struct Host {
    /// seconds since some fixed point
    pub now: fn() -> f64,
    /// a whole file, None when it cannot be read
    pub read: fn(&str) -> Option<Vec<u8>>,
    /// write a whole file (creating its folders); false on failure
    pub write: fn(&str, &[u8]) -> bool,
    /// make a folder and its parents
    pub mkdir: fn(&str) -> bool,
    /// `f(0) .. f(n-1)` over the host's cores, returning when all are done. The jobs never allocate.
    pub par_for: fn(usize, &(dyn Fn(usize) + Sync)),
    /// pictures the in-crate PNG codec cannot read (BMP, SVG, ...): straight RGBA8 (w, h, pixels)
    pub decode: fn(&[u8]) -> Option<(usize, usize, Vec<u8>)>,
}

fn no_now() -> f64 {
    0.0
}
fn no_read(_: &str) -> Option<Vec<u8>> {
    None
}
fn no_write(_: &str, _: &[u8]) -> bool {
    false
}
fn no_mkdir(_: &str) -> bool {
    false
}
fn seq_for(n: usize, f: &(dyn Fn(usize) + Sync)) {
    for i in 0..n {
        f(i);
    }
}
fn no_decode(_: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
    None
}
static DEFAULT: Host = Host { now: no_now, read: no_read, write: no_write, mkdir: no_mkdir, par_for: seq_for, decode: no_decode };
static HOST: AtomicPtr<Host> = AtomicPtr::new(core::ptr::null_mut());

/// Install the host's hooks (once, before the first [`crate::Studio`]).
pub fn set_host(h: &'static Host) {
    HOST.store(h as *const Host as *mut Host, Ordering::Release);
}
fn host() -> &'static Host {
    let p = HOST.load(Ordering::Acquire);
    if p.is_null() {
        &DEFAULT
    } else {
        unsafe { &*p }
    }
}

pub fn now() -> f64 {
    (host().now)()
}
pub fn read(path: &str) -> Result<Vec<u8>, String> {
    (host().read)(path).ok_or_else(|| format!("cannot read {path}"))
}
pub fn write(path: &str, data: &[u8]) -> Result<(), String> {
    if (host().write)(path, data) {
        Ok(())
    } else {
        Err(format!("cannot write {path}"))
    }
}
pub fn mkdir_all(path: &str) -> bool {
    (host().mkdir)(path)
}
pub fn decode_other(data: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
    (host().decode)(data)
}
/// `a/b`, one separator
pub fn join(a: &str, b: &str) -> String {
    if a.is_empty() {
        return String::from(b);
    }
    let mut s = String::from(a.trim_end_matches(['/', '\\']));
    s.push('/');
    s.push_str(b.trim_start_matches(['/', '\\']));
    s
}

/// `std::time::Instant` from the host clock
#[derive(Clone, Copy)]
pub struct Instant(f64);
impl Instant {
    pub fn now() -> Instant {
        Instant(now())
    }
    pub fn elapsed(&self) -> core::time::Duration {
        core::time::Duration::from_secs_f64((now() - self.0).max(0.0))
    }
}

/// A raw pointer the worker cores may share; every job touches only its own rows.
#[derive(Clone, Copy)]
struct Shared<T>(*mut T);
unsafe impl<T> Sync for Shared<T> {}
unsafe impl<T> Send for Shared<T> {}
impl<T> Shared<T> {
    #[inline(always)]
    fn get(&self) -> *mut T {
        self.0
    }
}

/// rows `y0..y1` of a `w`-wide buffer through `f(y, row)`, spread over the host's cores (8 rows a job)
pub fn par_rows(buf: &mut [u32], w: usize, y0: usize, y1: usize, f: &(dyn Fn(usize, &mut [u32]) + Sync)) {
    let h = buf.len() / w.max(1);
    let y1 = y1.min(h);
    if w == 0 || y0 >= y1 {
        return;
    }
    const PER: usize = 8;
    let jobs = (y1 - y0).div_ceil(PER);
    let base = Shared(buf.as_mut_ptr());
    (host().par_for)(jobs, &|j| {
        let a = y0 + j * PER;
        let b = (a + PER).min(y1);
        for y in a..b {
            // disjoint rows: each job owns rows a..b
            let row = unsafe { core::slice::from_raw_parts_mut(base.get().add(y * w), w) };
            f(y, row);
        }
    });
}

/// `buf` in chunks of `n` through `f(chunk index, chunk)`, spread over the host's cores
pub fn par_chunks<T: Send>(buf: &mut [T], n: usize, f: &(dyn Fn(usize, &mut [T]) + Sync)) {
    if n == 0 || buf.is_empty() {
        return;
    }
    let len = buf.len();
    let jobs = len.div_ceil(n);
    let base = Shared(buf.as_mut_ptr());
    (host().par_for)(jobs, &|j| {
        let a = j * n;
        let b = (a + n).min(len);
        let chunk = unsafe { core::slice::from_raw_parts_mut(base.get().add(a), b - a) };
        f(j, chunk);
    });
}

// ── rayon's adaptors, sequentially ───────────────────────────────────────────
pub trait SeqSlice<T> {
    fn par_iter(&self) -> core::slice::Iter<'_, T>;
    fn par_iter_mut(&mut self) -> core::slice::IterMut<'_, T>;
    fn par_chunks(&self, n: usize) -> core::slice::Chunks<'_, T>;
    fn par_chunks_mut(&mut self, n: usize) -> core::slice::ChunksMut<'_, T>;
}
impl<T> SeqSlice<T> for [T] {
    fn par_iter(&self) -> core::slice::Iter<'_, T> {
        self.iter()
    }
    fn par_iter_mut(&mut self) -> core::slice::IterMut<'_, T> {
        self.iter_mut()
    }
    fn par_chunks(&self, n: usize) -> core::slice::Chunks<'_, T> {
        self.chunks(n)
    }
    fn par_chunks_mut(&mut self, n: usize) -> core::slice::ChunksMut<'_, T> {
        self.chunks_mut(n)
    }
}
pub trait SeqIntoPar: IntoIterator + Sized {
    fn into_par_iter(self) -> Self::IntoIter {
        self.into_iter()
    }
}
impl<I: IntoIterator> SeqIntoPar for I {}

// ── float methods std has and core / num-traits' Float lack ─────────────────
pub trait FloatExt {
    fn rem_euclid(self, rhs: Self) -> Self;
    fn div_euclid(self, rhs: Self) -> Self;
}
impl FloatExt for f32 {
    fn rem_euclid(self, rhs: f32) -> f32 {
        let r = libm::fmodf(self, rhs);
        if r < 0.0 {
            r + if rhs < 0.0 { -rhs } else { rhs }
        } else {
            r
        }
    }
    fn div_euclid(self, rhs: f32) -> f32 {
        let q = libm::truncf(self / rhs);
        if libm::fmodf(self, rhs) < 0.0 {
            if rhs > 0.0 {
                q - 1.0
            } else {
                q + 1.0
            }
        } else {
            q
        }
    }
}
impl FloatExt for f64 {
    fn rem_euclid(self, rhs: f64) -> f64 {
        let r = libm::fmod(self, rhs);
        if r < 0.0 {
            r + if rhs < 0.0 { -rhs } else { rhs }
        } else {
            r
        }
    }
    fn div_euclid(self, rhs: f64) -> f64 {
        let q = libm::trunc(self / rhs);
        if libm::fmod(self, rhs) < 0.0 {
            if rhs > 0.0 {
                q - 1.0
            } else {
                q + 1.0
            }
        } else {
            q
        }
    }
}

/// CRC-32 (IEEE), for PNG chunks and zip entries
pub fn crc32(data: &[u8]) -> u32 {
    crc32_update(0, data)
}
pub fn crc32_update(crc: u32, data: &[u8]) -> u32 {
    static TABLE: sync::OnceLock<[u32; 256]> = sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *e = c;
        }
        t
    });
    let mut c = !crc;
    for &b in data {
        c = t[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}
