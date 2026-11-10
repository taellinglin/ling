//! RAM above 4 GiB, for big read-only-after-load data only (the diffusion engine's weights).
//!
//! The general allocators stay below 4 GiB (`frame::SEED_LIMIT`): DMA engines and parts of the runtime still assume 32-bit
//! physical addresses, and the boot identity map ends there. The firmware's map often lists gigabytes above that line, though
//! (any PC with more than ~3 GiB, QEMU `-m 6G`), and a Stable Diffusion checkpoint's 2 GB of weights fit there without touching
//! the memory everything else uses. This pool is deliberately small in scope:
//!   * `frame::add_free_region` hands it the parts of RAM regions above 4 GiB (x86-64 only);
//!   * `alloc` carves 2 MiB-aligned blocks off those regions in order and identity-maps them (2 MiB pages, write-back,
//!     no-execute) into the kernel's tables, which every address space shares;
//!   * `free` counts blocks back; when the last one returns, the whole pool is free again (its only user frees everything
//!     at once when it unloads a model). Nothing in here is ever handed to a device, the GC, or the heap.
#![allow(dead_code)]

const MAX_REGIONS: usize = 16;
const BLOCK: u64 = 2 << 20;

static mut REGIONS: [(u64, u64); MAX_REGIONS] = [(0, 0); MAX_REGIONS];
static mut COUNT: usize = 0;
/// next free byte: (region, offset in it)
static mut NEXT: (usize, u64) = (0, 0);
static mut LIVE: usize = 0;
static mut USED: u64 = 0;

/// A RAM range above 4 GiB from the firmware's map.
pub fn note(start: u64, end: u64) {
    let s = (start + BLOCK - 1) & !(BLOCK - 1);
    let e = end & !(BLOCK - 1);
    // the kernel's low PML4 slot spans 512 GiB
    let e = e.min(1 << 39);
    if e <= s {
        return;
    }
    unsafe {
        // the same range reported twice (two memory-map sources) counts once
        if (*(&raw const REGIONS)).iter().take(COUNT).any(|&(rs, re)| s < re && e > rs) {
            return;
        }
        if COUNT < MAX_REGIONS {
            REGIONS[COUNT] = (s, e);
            COUNT += 1;
        }
    }
}

/// Bytes of RAM above 4 GiB the pool knows about.
pub fn total() -> u64 {
    unsafe { (*(&raw const REGIONS)).iter().take(COUNT).map(|(s, e)| e - s).sum() }
}

/// Bytes still free.
pub fn free_bytes() -> u64 {
    total().saturating_sub(unsafe { USED })
}

/// A block of at least `min` bytes (rounded up to 2 MiB; `want` more if the region has it), mapped and zeroed... not
/// zeroed: callers overwrite it. None when the pool is exhausted or there is none.
pub fn alloc(min: usize, want: usize) -> Option<(*mut u8, usize)> {
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = (min, want);
        return None;
    }
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let need = ((min as u64) + BLOCK - 1) & !(BLOCK - 1);
        let wanted = ((want.max(min) as u64) + BLOCK - 1) & !(BLOCK - 1);
        while NEXT.0 < COUNT {
            let (s, e) = REGIONS[NEXT.0];
            let at = s + NEXT.1;
            let room = e - at;
            if room >= need {
                let len = wanted.min(room);
                if !crate::arch::paging::map_ram_high(at, len) {
                    return None;
                }
                NEXT.1 += len;
                LIVE += 1;
                USED += len;
                return Some((at as *mut u8, len as usize));
            }
            NEXT = (NEXT.0 + 1, 0);
        }
        None
    }
}

/// Is `p` inside the pool?
pub fn owns(p: *const u8) -> bool {
    let a = p as u64;
    unsafe { (*(&raw const REGIONS)).iter().take(COUNT).any(|&(s, e)| a >= s && a < e) }
}

/// Give a block back; the pool starts over once every block is back.
pub fn free(_p: *mut u8, len: usize) {
    unsafe {
        LIVE = LIVE.saturating_sub(1);
        USED = USED.saturating_sub(len as u64);
        if LIVE == 0 {
            NEXT = (0, 0);
            USED = 0;
        }
    }
}
