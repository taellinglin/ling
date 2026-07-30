//! A bump allocator — the whole heap `ling-kernel` has. No `free` (it's a
//! no-op): reclaiming memory needs tracking what's still reachable, which
//! needs more infrastructure than a "basic" kernel has yet. This unlocks
//! Ling's string/list/struct runtime (`strings.rs`) for kernel `.ling`
//! code; a long-running shell that allocates a huge number of strings will
//! eventually exhaust `ARENA_SIZE` and panic — a real, disclosed
//! limitation, not a bug, until a reclaiming allocator exists.
const ARENA_SIZE: usize = 4 * 1024 * 1024; // 4 MiB

static mut ARENA: [u8; ARENA_SIZE] = [0; ARENA_SIZE];
static mut CURSOR: usize = 0;

/// Allocate `size` bytes, 8-byte aligned. Panics (kernel_panic) on
/// exhaustion rather than returning null — every caller in this crate
/// assumes a valid pointer back, and there's no recovery path yet anyway.
pub fn alloc(size: usize) -> *mut u8 {
    unsafe {
        let aligned = (CURSOR + 7) & !7;
        if aligned + size > ARENA_SIZE {
            crate::kernel_panic("out of memory (bump allocator arena exhausted)");
        }
        let ptr = (&raw mut ARENA).cast::<u8>().add(aligned);
        CURSOR = aligned + size;
        ptr
    }
}

/// No-op — see the module doc.
pub fn free(_ptr: *mut u8) {}
