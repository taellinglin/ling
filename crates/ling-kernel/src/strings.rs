//! Ling's generic string runtime (`ling_str_*`), backed by `mm::heap`'s
//! slab allocator. Layout: a length-prefixed buffer — 4-byte LE length,
//! then that many content bytes — addressed by a NaN-boxed heap value.
//! Encoding matches `ling_codegen::cranelift::runtime` exactly (there's no
//! way to share that module directly across the no_std boundary, so the
//! tag constants are duplicated here — keep in sync with
//! `crates/ling-codegen/src/cranelift/runtime.rs` if that ever changes).
//!
//! Nothing here ever calls `mm::heap::free`: the Ling compiler doesn't emit
//! drop/refcount calls for kernel-target code, so every string this runtime
//! allocates is intentionally leaked for the process's lifetime, same as
//! before this module moved off the old bump allocator — a real fix needs
//! ownership tracking at the language/compiler level, not an allocator
//! change, and is out of scope here.
use core::ptr;

const TAG_PATTERN: u64 = 0x7F00_0000_0000_0000;
pub(crate) const TAG_KIND_STRING: u64 = 0x0003_0000_0000_0000;
const PTR_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

pub(crate) fn tag_kind(v: u64) -> u64 {
    v & 0x00FF_0000_0000_0000
}

fn encode(ptr: *const u8) -> u64 {
    TAG_PATTERN | TAG_KIND_STRING | (ptr as u64 & PTR_MASK)
}

pub(crate) fn decode_ptr(val: u64) -> *mut u8 {
    (val & PTR_MASK) as *mut u8
}

unsafe fn header_len(ptr: *const u8) -> usize {
    u32::from_le_bytes([*ptr, *ptr.add(1), *ptr.add(2), *ptr.add(3)]) as usize
}

unsafe fn data_ptr(ptr: *mut u8) -> *mut u8 {
    ptr.add(4)
}

/// Read the bytes out of a *tagged* boxed string value (masks off the tag
/// bits itself).
pub(crate) unsafe fn bytes_of(val: u64) -> &'static [u8] {
    bytes_of_ptr(decode_ptr(val))
}

/// Read the bytes out of an already-untagged buffer pointer — what a
/// `ling_kernel_*` intrinsic's argument is by the time it arrives (the
/// compiler's `dynamic_to_raw` strips the tag bits at the call site, since
/// kernel intrinsics always want a raw pointer, never the NaN-boxed bit
/// pattern). Nothing here ever frees a string (see the module doc), so
/// this is safe to hand back as `'static`.
pub(crate) unsafe fn bytes_of_ptr(p: *mut u8) -> &'static [u8] {
    let len = header_len(p);
    core::slice::from_raw_parts(data_ptr(p), len)
}

fn alloc_string(len: usize) -> *mut u8 {
    let buf = crate::mm::heap::alloc(4 + len);
    if buf.is_null() {
        crate::kernel_panic("out of memory (heap allocator exhausted)");
    }
    let len_bytes = (len as u32).to_le_bytes();
    unsafe { ptr::copy_nonoverlapping(len_bytes.as_ptr(), buf, 4) };
    buf
}

/// Build a new Ling string object from raw bytes (e.g. a string literal's
/// data, or a byte buffer assembled by a kernel intrinsic like
/// `ling_kernel_read_line`).
#[no_mangle]
pub unsafe extern "C" fn ling_str_new(bytes: *const u8, len: usize) -> u64 {
    let buf = alloc_string(len);
    ptr::copy_nonoverlapping(bytes, data_ptr(buf), len);
    encode(buf)
}

#[no_mangle]
pub unsafe extern "C" fn ling_str_len(val: u64) -> u64 {
    let len = header_len(decode_ptr(val));
    (len as f64).to_bits()
}

#[no_mangle]
pub unsafe extern "C" fn ling_str_concat(a: u64, b: u64) -> u64 {
    let (pa, pb) = (decode_ptr(a), decode_ptr(b));
    let (la, lb) = (header_len(pa), header_len(pb));
    let buf = alloc_string(la + lb);
    ptr::copy_nonoverlapping(data_ptr(pa), data_ptr(buf), la);
    ptr::copy_nonoverlapping(data_ptr(pb), data_ptr(buf).add(la), lb);
    encode(buf)
}

#[no_mangle]
pub unsafe extern "C" fn ling_str_eq(a: u64, b: u64) -> u64 {
    let eq = bytes_of(a) == bytes_of(b);
    if eq {
        0x7F02_0000_0000_0000
    } else {
        0x7F01_0000_0000_0000
    }
}
