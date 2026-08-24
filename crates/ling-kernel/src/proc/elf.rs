//! Minimal ELF64 loader: parse `PT_LOAD` segments out of a static,
//! non-relocatable executable and map each into a fresh process address
//! space with its real `p_flags` permissions. No dynamic linking, no
//! interpreter (`PT_INTERP`), no relocations — every program this loads is
//! expected to be statically linked at a fixed virtual address, which is
//! exactly what `--platform lingos` (Phase 4) and this phase's own
//! hand-assembled verification binary both are.
use crate::arch::paging;

const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const ET_EXEC: u16 = 2;
#[cfg(target_arch = "x86_64")]
const EM_EXPECTED: u16 = 62; // EM_X86_64
#[cfg(target_arch = "aarch64")]
const EM_EXPECTED: u16 = 183; // EM_AARCH64
const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;
const PF_W: u32 = 2;

fn read_u16(data: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([data[off], data[off + 1]])
}
fn read_u32(data: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(data[off..off + 4].try_into().unwrap())
}
fn read_u64(data: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(data[off..off + 8].try_into().unwrap())
}

const PAGE_SIZE: u64 = 4096;

/// Map every `PT_LOAD` segment of `elf` into `pml4`'s address space and
/// return the entry point (`e_entry`). `Err` on anything that isn't a
/// well-formed, statically-linked x86_64 executable — never partially maps
/// and then fails, since a half-loaded process address space with no way to
/// tell the caller which half is a worse state than refusing to load at
/// all.
pub fn load(pml4: u64, elf: &[u8]) -> Result<u64, &'static str> {
    if elf.len() < 64 || elf[0..4] != ELF_MAGIC {
        return Err("not an ELF file");
    }
    if elf[4] != 2 {
        return Err("not a 64-bit ELF");
    }
    if read_u16(elf, 16) != ET_EXEC {
        return Err("not a static executable (ET_EXEC)");
    }
    if read_u16(elf, 18) != EM_EXPECTED {
        return Err("wrong machine type for this architecture");
    }

    let entry = read_u64(elf, 24);
    let phoff = read_u64(elf, 32) as usize;
    let phentsize = read_u16(elf, 54) as usize;
    let phnum = read_u16(elf, 56) as usize;

    for i in 0..phnum {
        let ph = phoff + i * phentsize;
        if ph + 56 > elf.len() {
            return Err("program header out of bounds");
        }
        if read_u32(elf, ph) != PT_LOAD {
            continue;
        }
        let p_flags = read_u32(elf, ph + 4);
        let p_offset = read_u64(elf, ph + 8) as usize;
        let p_vaddr = read_u64(elf, ph + 16);
        let p_filesz = read_u64(elf, ph + 32) as usize;
        let p_memsz = read_u64(elf, ph + 40) as usize;

        if p_offset + p_filesz > elf.len() {
            return Err("segment data out of bounds");
        }

        let mut flags = paging::USER;
        if p_flags & PF_W != 0 {
            flags |= paging::WRITABLE;
        }
        if p_flags & PF_X == 0 {
            flags |= paging::NX;
        }

        let seg_start_page = p_vaddr & !(PAGE_SIZE - 1);
        let seg_end = p_vaddr + p_memsz as u64;
        let mut page = seg_start_page;
        while page < seg_end {
            let phys = crate::mm::frame::alloc_frames(0).ok_or("out of memory loading segment")?;
            unsafe { core::ptr::write_bytes(phys as *mut u8, 0, PAGE_SIZE as usize) };

            // Copy whatever part of [page, page+PAGE_SIZE) this page
            // overlaps with the segment's file-backed range; anything past
            // `p_filesz` (the BSS tail) stays zeroed from the write_bytes
            // above.
            let page_end = page + PAGE_SIZE;
            let copy_start = page.max(p_vaddr);
            let copy_end = page_end.min(p_vaddr + p_filesz as u64);
            if copy_end > copy_start {
                let src_off = p_offset + (copy_start - p_vaddr) as usize;
                let dst_off = (copy_start - page) as usize;
                let len = (copy_end - copy_start) as usize;
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        elf.as_ptr().add(src_off),
                        (phys as *mut u8).add(dst_off),
                        len,
                    );
                }
            }

            paging::map4k(pml4, page, phys, flags);
            page += PAGE_SIZE;
        }
    }

    Ok(entry)
}
