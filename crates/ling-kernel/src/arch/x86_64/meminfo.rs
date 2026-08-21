//! Multiboot2 memory-map tag (type 6) parsing, feeding `mm::frame`'s buddy
//! allocator its usable physical ranges. Shares `bootmodule.rs`'s tag-walk
//! shape (same Multiboot2 info structure, same "no info at all on the
//! disk-boot path" caveat) but a separate module since it walks *every*
//! tag once — collecting both the memory map and every module range, so a
//! module's payload (loaded via `module2`, see `bootmodule.rs`) never gets
//! silently handed out as free memory just because GRUB's memory map has
//! no idea a module was loaded on top of it.
use core::ptr;

extern "C" {
    static mb2_info_ptr: u32;
}

const TAG_MEMORY_MAP: u32 = 6;
const TAG_MODULE: u32 = 3;
const MEMORY_AVAILABLE: u32 = 1;

const MAX_MODULES: usize = 8;
const MAX_SEGMENTS: usize = 32;

/// Fallback when there's no Multiboot2 info at all (the disk-boot path,
/// which never sets `mb2_info_ptr` — see `boot.rs`'s doc on tolerating
/// `ebx==0`). Conservative and disclosed as such, the same honesty pattern
/// `timer.rs::calibrate`'s PIT-timeout fallback already uses: 128MiB is
/// below any realistic modern VM/hardware RAM size, so this never claims
/// memory that doesn't exist, at the cost of not using memory above it on
/// that one boot path until it gets real detection of its own.
const FALLBACK_BASE: u64 = 16 * 1024 * 1024;
const FALLBACK_LEN: u64 = 112 * 1024 * 1024;

/// Register every usable range Multiboot2 reports (minus whatever's below
/// `kernel_end` and any loaded module) with `mm::frame`. Call once, after
/// `mm::frame`'s statics are zero-initialized (implicit — no explicit
/// init needed) and before anything calls `mm::frame::alloc_frames`.
pub fn detect_and_register(kernel_end: u64) {
    unsafe {
        let info_ptr = ptr::read_volatile(&raw const mb2_info_ptr);
        if info_ptr == 0 {
            crate::mm::frame::add_free_region(FALLBACK_BASE, FALLBACK_LEN, kernel_end);
            return;
        }

        let mut modules = [(0u64, 0u64); MAX_MODULES];
        let mut module_count = 0usize;

        let total_size = ptr::read_unaligned(info_ptr as *const u32);
        let mut offset: u32 = 8;
        while offset + 8 <= total_size {
            let tag_ptr = (info_ptr + offset) as *const u32;
            let tag_type = ptr::read_unaligned(tag_ptr);
            let tag_size = ptr::read_unaligned(tag_ptr.add(1));
            if tag_type == 0 {
                break;
            }
            if tag_type == TAG_MODULE && tag_size >= 16 && module_count < MAX_MODULES {
                let base = (info_ptr + offset) as u64;
                let mod_start = ptr::read_unaligned((base + 8) as *const u32) as u64;
                let mod_end = ptr::read_unaligned((base + 12) as *const u32) as u64;
                modules[module_count] = (mod_start, mod_end);
                module_count += 1;
            }
            if tag_type == TAG_MEMORY_MAP {
                register_memory_map(info_ptr + offset, tag_size, kernel_end, &modules[..module_count]);
            }
            offset += (tag_size + 7) & !7;
        }
    }
}

unsafe fn register_memory_map(tag_base: u32, tag_size: u32, kernel_end: u64, modules: &[(u64, u64)]) {
    let entry_size = ptr::read_unaligned((tag_base + 8) as *const u32);
    if entry_size == 0 {
        return;
    }
    let entries_end = tag_base + tag_size;
    let mut entry = tag_base + 16;
    while entry + entry_size <= entries_end {
        let base = ptr::read_unaligned(entry as *const u64);
        let length = ptr::read_unaligned((entry + 8) as *const u64);
        let region_type = ptr::read_unaligned((entry + 16) as *const u32);
        if region_type == MEMORY_AVAILABLE {
            register_region_excluding_modules(base, length, kernel_end, modules);
        }
        entry += entry_size;
    }
}

/// Split `[base, base+len)` around every overlapping module range, then
/// register whatever survives.
fn register_region_excluding_modules(base: u64, len: u64, kernel_end: u64, modules: &[(u64, u64)]) {
    let mut segments = [(0u64, 0u64); MAX_SEGMENTS];
    segments[0] = (base, base.saturating_add(len));
    let mut count = 1usize;

    for &(mod_start, mod_end) in modules {
        let mut i = 0;
        while i < count {
            let (seg_start, seg_end) = segments[i];
            if mod_end <= seg_start || mod_start >= seg_end {
                i += 1;
                continue;
            }
            // Overlaps: replace this segment with its non-overlapping
            // before/after remainders (either or both may be empty).
            let before = (seg_start, mod_start.max(seg_start).min(seg_end));
            let after = (mod_end.max(seg_start).min(seg_end), seg_end);
            segments[i] = before;
            if after.1 > after.0 && count < MAX_SEGMENTS {
                segments[count] = after;
                count += 1;
            }
            i += 1;
        }
    }

    for &(start, end) in &segments[..count] {
        if end > start {
            crate::mm::frame::add_free_region(start, end - start, kernel_end);
        }
    }
}
