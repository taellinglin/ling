//! Multiboot2 module (tag type 3) lookup — how the installer gets the
//! disk-boot payload (`diskboot.img`: stage1 + stage2 + the flattened
//! kernel, see `LingOS/live/build-diskboot-x86_64.sh`) into its hands
//! without needing an ISO9660 driver. `live/grub.cfg`'s Install entry loads
//! it via a `module2` directive; GRUB reads the file off the ISO itself
//! (that's its job already) and hands back a plain (start, end) physical
//! address range already sitting in RAM, described by this tag in the same
//! Multiboot2 info structure `framebuffer.rs` reads for its own tag.
use core::ptr;

extern "C" {
    static mb2_info_ptr: u32;
}

/// The first Multiboot2 module's (start, end) physical address range, if
/// `grub.cfg` loaded one via `module2`. `None` if there's no Multiboot2
/// info at all (e.g. the disk-boot path, which never sets `mb2_info_ptr`),
/// or no module tag in it (e.g. the Live entry, which doesn't load one).
pub fn first_module() -> Option<(u32, u32)> {
    unsafe {
        let info_ptr = ptr::read_volatile(&raw const mb2_info_ptr);
        if info_ptr == 0 {
            return None;
        }
        let total_size = ptr::read_unaligned(info_ptr as *const u32);
        let mut offset: u32 = 8;
        while offset + 8 <= total_size {
            let tag_ptr = (info_ptr + offset) as *const u32;
            let tag_type = ptr::read_unaligned(tag_ptr);
            let tag_size = ptr::read_unaligned(tag_ptr.add(1));
            if tag_type == 0 {
                break;
            }
            if tag_type == 3 && tag_size >= 16 {
                let base = info_ptr + offset;
                let mod_start = ptr::read_unaligned((base + 8) as *const u32);
                let mod_end = ptr::read_unaligned((base + 12) as *const u32);
                return Some((mod_start, mod_end));
            }
            offset += (tag_size + 7) & !7;
        }
        None
    }
}
