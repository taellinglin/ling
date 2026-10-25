//! The flattened device tree the Raspberry Pi firmware hands over in `x0`:
//! just enough of a parser to read the `/memory` node's `reg` ranges (the
//! mailbox `GET_ARM_MEMORY` reports only the first region, which on a
//! 4 / 8 GB Pi is the first gigabyte) and the root `model` string.
//!
//! `boot.rs` saves the pointer before `x0` is reused; nothing here allocates,
//! so it can run before the frame allocator exists.

use crate::arch::boot::dtb_ptr;

const FDT_MAGIC: u32 = 0xD00D_FEED;
const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_PROP: u32 = 3;
const FDT_NOP: u32 = 4;
const FDT_END: u32 = 9;

pub const MAX_REGIONS: usize = 8;

/// The tree, copied out of whatever RAM the firmware (or QEMU) put it in: the
/// frame allocator keeps its free list inside the free frames, so the original
/// is overwritten the moment memory init runs. 256 KiB holds every Pi tree.
#[repr(C, align(8))]
struct DtbCopy([u8; 256 * 1024]);
static mut DTB_COPY: DtbCopy = DtbCopy([0; 256 * 1024]);

/// Call once, first thing after the board is known and before anything can
/// allocate; afterwards `dtb_ptr()` points at the copy.
pub fn preserve() {
    let Some(h) = header() else { return };
    if h.total > 256 * 1024 {
        return;
    }
    unsafe {
        let dst = (&raw mut DTB_COPY) as *mut u8;
        core::ptr::copy_nonoverlapping(h.base, dst, h.total);
        crate::arch::boot::set_dtb_ptr(dst as u64);
    }
}

#[inline]
fn be32(p: *const u8) -> u32 {
    unsafe { u32::from_be_bytes([*p, *p.add(1), *p.add(2), *p.add(3)]) }
}

fn cstr<'a>(p: *const u8, max: usize) -> &'a [u8] {
    let mut n = 0;
    while n < max && unsafe { *p.add(n) } != 0 {
        n += 1;
    }
    unsafe { core::slice::from_raw_parts(p, n) }
}

struct Header {
    base: *const u8,
    struct_off: usize,
    strings_off: usize,
    struct_size: usize,
    total: usize,
}

fn header() -> Option<Header> {
    let base = dtb_ptr() as *const u8;
    if base.is_null() || (base as usize) & 3 != 0 {
        return None;
    }
    if be32(base) != FDT_MAGIC {
        return None;
    }
    let total = be32(unsafe { base.add(4) }) as usize;
    let struct_off = be32(unsafe { base.add(8) }) as usize;
    let strings_off = be32(unsafe { base.add(12) }) as usize;
    let version = be32(unsafe { base.add(20) });
    let struct_size = if version >= 17 { be32(unsafe { base.add(36) }) as usize } else { total.saturating_sub(struct_off) };
    if total < 40 || total > 16 << 20 || struct_off >= total || strings_off >= total {
        return None;
    }
    Some(Header { base, struct_off, strings_off, struct_size: struct_size.min(total - struct_off), total })
}

/// Walk the structure block calling `prop(depth, node_name, prop_name, data)`
/// for every property.
fn walk(h: &Header, mut prop: impl FnMut(usize, &[u8], &[u8], &[u8])) {
    let mut off = h.struct_off;
    let end = h.struct_off + h.struct_size;
    let mut depth = 0usize;
    let mut names: [&[u8]; 16] = [&[]; 16];
    while off + 4 <= end {
        let tok = be32(unsafe { h.base.add(off) });
        off += 4;
        match tok {
            FDT_BEGIN_NODE => {
                let name = cstr(unsafe { h.base.add(off) }, end - off);
                if depth < names.len() {
                    names[depth] = name;
                }
                depth += 1;
                off = (off + name.len() + 1 + 3) & !3;
            },
            FDT_END_NODE => {
                depth = depth.saturating_sub(1);
            },
            FDT_PROP => {
                if off + 8 > end {
                    return;
                }
                let len = be32(unsafe { h.base.add(off) }) as usize;
                let nameoff = be32(unsafe { h.base.add(off + 4) }) as usize;
                off += 8;
                if off + len > end || h.strings_off + nameoff >= h.total {
                    return;
                }
                let pname = cstr(unsafe { h.base.add(h.strings_off + nameoff) }, h.total - h.strings_off - nameoff);
                let data = unsafe { core::slice::from_raw_parts(h.base.add(off), len) };
                let node = if depth >= 1 && depth - 1 < names.len() { names[depth - 1] } else { &[] };
                prop(depth, node, pname, data);
                off = (off + len + 3) & !3;
            },
            FDT_NOP => {},
            FDT_END => return,
            _ => return,
        }
    }
}

fn cells(data: &[u8], n: usize, at: usize) -> u64 {
    let mut v = 0u64;
    for i in 0..n {
        let o = (at + i) * 4;
        if o + 4 > data.len() {
            return v;
        }
        v = (v << 32) | be32(data[o..].as_ptr()) as u64;
    }
    v
}

/// Usable RAM as `(base, size)` pairs from every `/memory` node (device_type
/// "memory"), in device-tree order. Empty when there is no tree.
pub fn memory_regions() -> ([(u64, u64); MAX_REGIONS], usize) {
    let mut out = [(0u64, 0u64); MAX_REGIONS];
    let mut n = 0usize;
    let Some(h) = header() else { return (out, 0) };
    let (mut ac, mut sc) = (2usize, 1usize);
    // root cells first
    walk(&h, |depth, _node, pname, data| {
        if depth == 1 {
            if pname == b"#address-cells" && data.len() >= 4 {
                ac = be32(data.as_ptr()) as usize;
            } else if pname == b"#size-cells" && data.len() >= 4 {
                sc = be32(data.as_ptr()) as usize;
            }
        }
    });
    let (ac, sc) = (ac.clamp(1, 2), sc.clamp(1, 2));
    walk(&h, |depth, node, pname, data| {
        if depth != 2 || pname != b"reg" {
            return;
        }
        let is_mem = node.starts_with(b"memory");
        if !is_mem {
            return;
        }
        let per = ac + sc;
        let count = data.len() / (per * 4);
        for i in 0..count {
            if n >= MAX_REGIONS {
                break;
            }
            let base = cells(data, ac, i * per);
            let size = cells(data, sc, i * per + ac);
            if size > 0 {
                out[n] = (base, size);
                n += 1;
            }
        }
    });
    (out, n)
}

/// The root `model` string ("Raspberry Pi 5 Model B Rev 1.0"), if the tree has one.
pub fn model() -> Option<&'static str> {
    let h = header()?;
    let mut found: Option<&'static str> = None;
    walk(&h, |depth, _node, pname, data| {
        if depth == 1 && pname == b"model" && found.is_none() {
            let s = if data.ends_with(&[0]) { &data[..data.len() - 1] } else { data };
            // the tree outlives the kernel: it is never freed or overwritten
            let s: &'static [u8] = unsafe { core::slice::from_raw_parts(s.as_ptr(), s.len()) };
            found = core::str::from_utf8(s).ok();
        }
    });
    found
}
