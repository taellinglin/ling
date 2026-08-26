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
    #[cfg(target_arch = "x86_64")]
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
        #[cfg(not(target_arch = "x86_64"))]
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
            // IF is bit 9 of RFLAGS; only re-enable if it was set on entry.
            if self.flags & (1 << 9) != 0 {
                crate::arch::cpu::sti();
            }
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
        let probe = crate::arch::mailbox::arm_memory();
        match probe {
            Some((base, size)) => frame::add_free_region(base, size, kernel_end),
            None => {
                // Same disclosed-fallback posture as the x86_64 backend's
                // no-Multiboot-info case: a conservative fixed range, used
                // only if the firmware mailbox call itself fails (not
                // observed on QEMU's `raspi3b` or real Pi 3/4 hardware).
                frame::add_free_region(16 * 1024 * 1024, 112 * 1024 * 1024, kernel_end);
            },
        }
    }
}
