//! Slab heap allocator with a real, working `free` — what `alloc.rs`'s bump
//! arena never had. Small requests (up to 2040 bytes of usable space) are
//! served from one of eight fixed size-class slabs, each backed by 4KiB
//! frames from [`super::frame`] and threaded into a free list the same
//! intrusive way `frame`'s buddy free lists are (a freed slot's own first 8
//! bytes hold the next-free pointer — no separate bookkeeping structure,
//! no extra memory). Larger requests go straight to the frame allocator.
//!
//! Every allocation carries an 8-byte header immediately before the
//! pointer handed back, recording which size class (or, for a large
//! allocation, which frame order) it belongs to — the only way `free(ptr)`
//! can know how to reclaim it without every caller having to remember and
//! pass the size back, which would mean touching every call site (there is
//! exactly one today, `strings.rs::alloc_string`, but this is meant to
//! outlive that being true).
use core::ptr;

/// Slot sizes *including* the 8-byte header — an object requesting `n`
/// usable bytes needs `n + 8` to fit a class, and gets back a pointer 8
/// bytes into whichever class slot it landed in.
const CLASS_SIZES: [usize; 8] = [16, 32, 64, 128, 256, 512, 1024, 2048];
const FRAME_SIZE: usize = 4096;

/// Header encoding: the high bit marks a large (frame-backed) allocation,
/// whose remaining bits are its buddy-allocator order; otherwise the low
/// bits are a small allocation's class index (0..8, fits in 3 bits, stored
/// generously in a full `u64` for simplicity — this is one word of
/// overhead per object either way, shaving bits off it buys nothing).
const LARGE_FLAG: u64 = 1 << 63;

static mut FREE_HEAD: [u64; CLASS_SIZES.len()] = [0; CLASS_SIZES.len()];

fn class_for(needed: usize) -> Option<usize> {
    CLASS_SIZES.iter().position(|&sz| sz >= needed)
}

unsafe fn write_header(addr: u64, header: u64) {
    ptr::write_volatile(addr as *mut u64, header);
}

unsafe fn read_header(user_ptr: *mut u8) -> u64 {
    ptr::read_volatile((user_ptr as u64 - 8) as *const u64)
}

/// Pull one more frame into `class`'s free list, carving it into however
/// many same-size slots fit — called only when that class's list is empty,
/// so this is the sole place slab memory growth happens.
unsafe fn refill(class: usize) -> bool {
    let Some(frame_addr) = super::frame::alloc_frames(0) else {
        return false;
    };
    let slot_size = CLASS_SIZES[class] as u64;
    let count = FRAME_SIZE as u64 / slot_size;
    for i in (0..count).rev() {
        let slot = frame_addr + i * slot_size;
        // Slot's own body (past its future header) doubles as the free-
        // list link while it's unused — same trick `frame.rs`'s buddy free
        // lists use, avoiding any separate metadata array.
        write_header(slot, FREE_HEAD[class]);
        FREE_HEAD[class] = slot;
    }
    true
}

/// Allocate `size` bytes, 8-byte aligned (every class size and every frame
/// order is a multiple of 8). Returns null on genuine exhaustion — unlike
/// `alloc.rs`'s bump arena, callers get a chance to check rather than an
/// automatic panic, though nothing currently calls this except through
/// `alloc::alloc`, which preserves the panic-on-null behavior that
/// existing callers (`strings.rs`) were already written to assume.
pub fn alloc(size: usize) -> *mut u8 {
    let needed = size + 8;
    unsafe {
        if let Some(class) = class_for(needed) {
            if FREE_HEAD[class] == 0 && !refill(class) {
                return ptr::null_mut();
            }
            let addr = FREE_HEAD[class];
            FREE_HEAD[class] = ptr::read_volatile(addr as *const u64);
            write_header(addr, class as u64);
            return (addr + 8) as *mut u8;
        }
        let order = order_for(needed);
        match super::frame::alloc_frames(order) {
            Some(addr) => {
                write_header(addr, LARGE_FLAG | order as u64);
                (addr + 8) as *mut u8
            },
            None => ptr::null_mut(),
        }
    }
}

fn order_for(bytes: usize) -> usize {
    let mut order = 0;
    while (FRAME_SIZE << order) < bytes {
        order += 1;
    }
    order
}

// ── Rust `#[global_allocator]` adapter ─────────────────────────────────────
// Wiring this over the slab makes `alloc` (Vec/String/Box/BTreeMap) available
// crate-wide -- the prerequisite for the in-kernel `ling` tree-walking
// interpreter. The slab hands back 8-byte-aligned pointers with a header in
// the 8 bytes *before* the pointer, so:
//   * align <= 8 (the common case; everything the interpreter needs): use the
//     slab pointer directly.
//   * align > 8 (rare): over-allocate `size + align`, return an aligned
//     pointer inside the block, and stash the real slab base in the 8 bytes
//     just before it so `dealloc` can recover and free the true base.
use core::alloc::{GlobalAlloc, Layout};

unsafe fn galloc(layout: Layout) -> *mut u8 {
    let align = layout.align();
    if align <= 8 {
        return alloc(layout.size());
    }
    let raw = alloc(layout.size() + align);
    if raw.is_null() {
        return raw;
    }
    let base = raw as usize;
    // (base + align) & !(align-1) is strictly > base with a gap of at least 8
    // bytes (base is 8-aligned, align >= 16), so stashing a usize at
    // aligned-8 never touches the slab header at base-8.
    let aligned = (base + align) & !(align - 1);
    ptr::write((aligned - 8) as *mut usize, base);
    aligned as *mut u8
}

unsafe fn gdealloc(user_ptr: *mut u8, layout: Layout) {
    if user_ptr.is_null() {
        return;
    }
    if layout.align() <= 8 {
        free(user_ptr);
    } else {
        let base = ptr::read((user_ptr as usize - 8) as *const usize);
        free(base as *mut u8);
    }
}

/// The kernel's global allocator: every `alloc`/`dealloc` runs inside an
/// [`super::IrqLock`] critical section so the unlocked slab free lists are
/// safe against preemption on this single-core kernel.
pub struct KernelHeap;

unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _guard = super::IrqLock::acquire();
        galloc(layout)
    }
    unsafe fn dealloc(&self, user_ptr: *mut u8, layout: Layout) {
        let _guard = super::IrqLock::acquire();
        gdealloc(user_ptr, layout)
    }
}

/// Return an allocation from [`alloc`] to the heap. Safe to call on any
/// pointer this module handed out; undefined behavior (like any manual
/// allocator) on anything else, a double free, or a null pointer.
pub fn free(user_ptr: *mut u8) {
    if user_ptr.is_null() {
        return;
    }
    unsafe {
        let header = read_header(user_ptr);
        let addr = user_ptr as u64 - 8;
        if header & LARGE_FLAG != 0 {
            super::frame::free_frames(addr, (header & !LARGE_FLAG) as usize);
        } else {
            let class = header as usize;
            write_header(addr, FREE_HEAD[class]);
            FREE_HEAD[class] = addr;
        }
    }
}
