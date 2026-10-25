//! Portable memory management: [`frame`] is the physical frame allocator
//! [`heap`] (a slab allocator with a real, working `free` — the direct
//! replacement for the crate's old `alloc.rs` bump-arena-that-never-frees)
//! and, later, per-process address spaces and userland `mmap` are built on.
//! Paging (NX/W^X, higher-half, guard pages, per-process address spaces) is
//! deliberately not built yet — see the crate's Phase 2/3 plan for why
//! that rewrite specifically is sequenced together with Phase 3's
//! per-process address spaces instead of being done twice.
pub mod frame;
pub mod heap;

/// A uniprocessor critical section: disables interrupts on construction and
/// restores the *prior* interrupt state on drop. LingOS is single-core, so
/// masking interrupts is enough mutual exclusion to make the unlocked slab/
/// frame free lists safe against the preemptive scheduler -- which matters
/// now that a `#[global_allocator]` routes every `alloc`/`dealloc` through
/// them. Restoring the saved flag (rather than unconditionally re-enabling)
/// keeps it correct when called from a context that already had interrupts
/// off (e.g. inside an interrupt handler).
pub struct IrqLock {
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    flags: u64,
}

impl IrqLock {
    #[inline]
    pub fn acquire() -> Self {
        #[cfg(target_arch = "x86_64")]
        unsafe {
            let flags = crate::arch::cpu::read_flags();
            crate::arch::cpu::cli();
            IrqLock { flags }
        }
        #[cfg(target_arch = "aarch64")]
        unsafe {
            // The timer interrupt mixes audio: the heap and the mixer's tables
            // must not change under it.
            IrqLock { flags: crate::arch::cpu::irq_save() }
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            IrqLock {}
        }
    }
}

impl Drop for IrqLock {
    #[inline]
    fn drop(&mut self) {
        #[cfg(target_arch = "x86_64")]
        unsafe {
            crate::arch::cpu::write_flags(self.flags);
        }
        #[cfg(target_arch = "aarch64")]
        unsafe {
            crate::arch::cpu::irq_restore(self.flags);
        }
    }
}

/// Detect this board's usable physical memory and hand every free range to
/// [`frame::add_free_region`]. Call once, early in `init()`, after the
/// console is up (so a fallback path can log that it's guessing) and
/// before anything calls `frame::alloc_frames`.
pub fn init_physical_memory() {
    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::meminfo::detect_and_register(crate::arch::boot::kernel_end());
    }
    #[cfg(target_arch = "aarch64")]
    {
        let kernel_end = crate::arch::boot::kernel_end();
        // The device tree lists every RAM range (a 4 / 8 GB Pi has several);
        // the mailbox knows only the first, below the VideoCore split.
        let (regions, n) = crate::arch::fdt::memory_regions();
        let mut count = 0;
        for &(base, size) in regions.iter().take(n) {
            if count < RAM_MAX {
                unsafe {
                    RAM_REGIONS[count] = (base, size);
                }
                count += 1;
            }
            frame::add_free_region(base, size, kernel_end);
        }
        if count == 0 {
            match crate::arch::mailbox::arm_memory() {
                Some((base, size)) => {
                    unsafe {
                        RAM_REGIONS[0] = (base, size);
                    }
                    count = 1;
                    frame::add_free_region(base, size, kernel_end);
                    // No device tree (config.txt's `device_tree=` leaves x0
                    // empty): the mailbox only knows the first bank, below
                    // the VideoCore split. The board revision code says how
                    // much RAM there is in all, and the BCM2711 / BCM2712 bank
                    // layout is fixed (the same ranges Linux's trees list).
                    for &(b, s) in upper_banks().iter() {
                        if s == 0 || count >= RAM_MAX {
                            break;
                        }
                        unsafe {
                            RAM_REGIONS[count] = (b, s);
                        }
                        count += 1;
                        frame::add_free_region(b, s, kernel_end);
                    }
                },
                None => {
                    // Same disclosed-fallback posture as the x86_64 backend's
                    // no-Multiboot-info case: a conservative fixed range, used
                    // only if the firmware mailbox call itself fails (not
                    // observed on QEMU's `raspi3b` or real Pi 3/4 hardware).
                    unsafe {
                        RAM_REGIONS[0] = (16 * 1024 * 1024, 112 * 1024 * 1024);
                    }
                    count = 1;
                    frame::add_free_region(16 * 1024 * 1024, 112 * 1024 * 1024, kernel_end);
                },
            }
        }
        unsafe {
            RAM_COUNT = count;
        }
    }
}

/// RAM above the first gigabyte on a Pi 4 / 5 without a device tree, from the
/// board revision code's memory-size field (bits 20..22 of a new-style code).
/// Up to two banks: `1 GiB..` (the Pi 4 stops at 0xFC00_0000, its peripheral
/// window; the Pi 5 has nothing but RAM below 4 GiB) and `4 GiB..total`.
#[cfg(target_arch = "aarch64")]
fn upper_banks() -> [(u64, u64); 2] {
    use crate::arch::mmio::{board, RpiBoard};
    const GIB: u64 = 1 << 30;
    let mut out = [(0u64, 0u64); 2];
    let Some(rev) = crate::arch::mailbox::board_revision() else { return out };
    if rev & (1 << 23) == 0 {
        return out; // old-style code: a 256 / 512 MB board
    }
    let total = match (rev >> 20) & 7 {
        2 => GIB,
        3 => 2 * GIB,
        4 => 4 * GIB,
        5 => 8 * GIB,
        6 => 16 * GIB,
        _ => return out,
    };
    // the frame allocator's bitmap covers 8 GiB
    let total = total.min(8 * GIB);
    if total <= GIB {
        return out;
    }
    let bank1_end = match board() {
        RpiBoard::Pi4 => total.min(0xFC00_0000),
        RpiBoard::Pi5 => total.min(4 * GIB),
        _ => return out,
    };
    out[0] = (GIB, bank1_end - GIB);
    if total > 4 * GIB {
        out[1] = (4 * GIB, total - 4 * GIB);
    }
    out
}

/// How many RAM banks [`init_physical_memory`] registered.
pub fn ram_region_count() -> usize {
    unsafe { RAM_COUNT }
}

/// The usable RAM ranges found at boot (aarch64), for the page tables: only
/// these are mapped as Normal memory, everything else as Device.
pub const RAM_MAX: usize = 8;
pub static mut RAM_REGIONS: [(u64, u64); RAM_MAX] = [(0, 0); RAM_MAX];
pub static mut RAM_COUNT: usize = 0;

/// One past the highest RAM address (0 when nothing was registered).
pub fn ram_end() -> u64 {
    let n = unsafe { RAM_COUNT };
    let mut end = 0u64;
    for &(b, s) in unsafe { &*(&raw const RAM_REGIONS) }.iter().take(n) {
        end = end.max(b.saturating_add(s));
    }
    end
}

/// Whether `addr` falls inside a registered RAM range.
pub fn in_ram(addr: u64) -> bool {
    let n = unsafe { RAM_COUNT };
    unsafe { &*(&raw const RAM_REGIONS) }.iter().take(n).any(|&(b, s)| addr >= b && addr < b.saturating_add(s))
}
