//! Real 4-level x86_64 page tables (PML4 → PDPT → PD → PT), replacing
//! `boot.rs`'s temporary flat RWX 2MiB identity map. Everything stays
//! identity-mapped (`virt == phys`) — this is not a higher-half kernel
//! relocation, it's the same addressing scheme `boot.rs`, `mm::frame`,
//! `mm::heap`, and `meminfo.rs` already assume, just now expressed as real
//! per-page-permission tables instead of one blanket RWX mapping. That's a
//! deliberate scope decision: relocating the kernel to a higher-half virtual
//! range buys canonical-address ergonomics, not additional isolation — the
//! actual security boundary is the U/S bit (enforced per-process below) and
//! NX/W^X (enforced here), both of which work identically at low addresses.
//!
//! The full [0, 4GiB) range stays identity-mapped, matching `boot.rs`'s
//! existing coverage exactly (`ahci.rs` reaches PCI BAR MMIO windows
//! anywhere in that range, not just within installed RAM) — using 2MiB huge
//! pages for the bulk of it (RW+NX: ordinary RAM and MMIO, never executed)
//! and dropping to 4KiB pages only for the 2MiB-aligned chunks that overlap
//! the kernel's own image, where real W^X separation
//! (`.text`=RX, `.rodata`=R, `.data`/`.bss`=RW+NX) is actually enforced.
//!
//! Every mapping below the split point keeps `USER=0` at every table level:
//! the CPU ANDs the U/S bit across the whole walk, so ring-3 code can never
//! dereference kernel memory even by accident, regardless of what a leaf
//! entry says — the actual privilege boundary Phase 3's ring-3 processes
//! depend on. Per-process user mappings ([`new_address_space`]) live in a
//! separate PML4 slot with `USER=1`, built fresh per process by `proc::elf`.
use super::cpu;
use core::arch::asm;

pub const PRESENT: u64 = 1 << 0;
pub const WRITABLE: u64 = 1 << 1;
pub const USER: u64 = 1 << 2;
pub const HUGE: u64 = 1 << 7;
pub const NX: u64 = 1 << 63;

const ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;
const PAGE_SIZE: u64 = 4096;
const HUGE_PAGE_SIZE: u64 = 2 * 1024 * 1024;
const IDENTITY_LIMIT: u64 = 4 * 1024 * 1024 * 1024;

const EFER_MSR: u32 = 0xC000_0080;
const EFER_NXE: u64 = 1 << 11;

extern "C" {
    static _text_start: u8;
    static _text_end: u8;
    static _rodata_end: u8;
    static _kernel_end: u8;
}

fn text_start() -> u64 {
    (&raw const _text_start) as u64
}
fn text_end() -> u64 {
    (&raw const _text_end) as u64
}
fn rodata_end() -> u64 {
    (&raw const _rodata_end) as u64
}
fn kernel_image_end() -> u64 {
    (&raw const _kernel_end) as u64
}

/// The kernel's own PML4, built once by [`init`] and shared (by physical
/// table pointer, not copy) into every process address space's low half.
static mut KERNEL_PML4: u64 = 0;

unsafe fn table_entry(table_phys: u64, index: usize) -> *mut u64 {
    (table_phys as *mut u64).add(index)
}

fn alloc_table() -> u64 {
    let phys = crate::mm::frame::alloc_frames(0).unwrap_or_else(|| {
        crate::kernel_panic("out of memory (page table allocation failed)")
    });
    unsafe {
        core::ptr::write_bytes(phys as *mut u8, 0, PAGE_SIZE as usize);
    }
    phys
}

/// Walk from `table_phys` down to `index`, creating (as permissive,
/// non-leaf entries) any missing intermediate table. Non-leaf entries are
/// always `PRESENT | WRITABLE | USER`: the actual restriction lives on the
/// leaf entry the walk bottoms out at, since x86_64 ANDs U/W across every
/// level — a restrictive intermediate entry would silently override a
/// permissive leaf, which is never what a caller here wants.
unsafe fn next_level(table_phys: u64, index: usize) -> u64 {
    let entry = table_entry(table_phys, index);
    let raw = core::ptr::read_volatile(entry);
    if raw & PRESENT != 0 {
        return raw & ADDR_MASK;
    }
    let child = alloc_table();
    core::ptr::write_volatile(entry, child | PRESENT | WRITABLE | USER);
    child
}

fn indices(virt: u64) -> (usize, usize, usize, usize) {
    (
        ((virt >> 39) & 0x1FF) as usize,
        ((virt >> 30) & 0x1FF) as usize,
        ((virt >> 21) & 0x1FF) as usize,
        ((virt >> 12) & 0x1FF) as usize,
    )
}

/// Map one 4KiB page. `flags` should be some combination of [`WRITABLE`],
/// [`USER`], [`NX`] — [`PRESENT`] is added automatically.
pub fn map4k(pml4_phys: u64, virt: u64, phys: u64, flags: u64) {
    let (i4, i3, i2, i1) = indices(virt);
    unsafe {
        let pdpt = next_level(pml4_phys, i4);
        let pd = next_level(pdpt, i3);
        let pt = next_level(pd, i2);
        let entry = table_entry(pt, i1);
        core::ptr::write_volatile(entry, (phys & ADDR_MASK) | flags | PRESENT);
    }
}

/// Map one 2MiB huge page at the PD level (`virt`/`phys` must be 2MiB
/// aligned) — used for the bulk identity map, where per-4KiB granularity
/// would cost megabytes of page tables for no permission benefit (every
/// page in the range gets the same flags anyway).
fn map2m(pml4_phys: u64, virt: u64, phys: u64, flags: u64) {
    let (i4, i3, i2, _) = indices(virt);
    unsafe {
        let pdpt = next_level(pml4_phys, i4);
        let pd = next_level(pdpt, i3);
        let entry = table_entry(pd, i2);
        core::ptr::write_volatile(entry, (phys & ADDR_MASK) | flags | HUGE | PRESENT);
    }
}

pub fn unmap4k(pml4_phys: u64, virt: u64) {
    let (i4, i3, i2, i1) = indices(virt);
    unsafe {
        let entry = table_entry(pml4_phys, i4);
        let pdpt = core::ptr::read_volatile(entry) & ADDR_MASK;
        if pdpt == 0 {
            return;
        }
        let entry = table_entry(pdpt, i3);
        let pd = core::ptr::read_volatile(entry) & ADDR_MASK;
        if pd == 0 {
            return;
        }
        let entry = table_entry(pd, i2);
        let pt = core::ptr::read_volatile(entry) & ADDR_MASK;
        if pt == 0 {
            return;
        }
        core::ptr::write_volatile(table_entry(pt, i1), 0);
        invlpg(virt);
    }
}

pub fn invlpg(virt: u64) {
    unsafe { asm!("invlpg [{}]", in(reg) virt, options(nostack)) };
}

/// R/W/X flags for a 4KiB page at `addr`, given the kernel's own linked
/// layout — real W^X: `.text` is executable and never writable, `.rodata`
/// is neither writable nor executable, everything else (pre-kernel padding,
/// `.data`/`.bss`, and ordinary RAM past `_kernel_end` that the frame/heap
/// allocators hand out) is writable and never executable.
fn kernel_page_flags(addr: u64) -> u64 {
    if addr >= text_start() && addr < text_end() {
        0 // R+X
    } else if addr >= text_end() && addr < rodata_end() {
        NX // R only
    } else {
        WRITABLE | NX // RW
    }
}

/// Build the kernel's real page tables and switch to them, replacing
/// `boot.rs`'s temporary flat map. Must run after `mm::init_physical_memory`
/// (page tables themselves come from the frame allocator) and before
/// anything that needs NX/W^X actually enforced. Idempotent is not
/// meaningful here — call exactly once, at boot.
pub fn init() {
    unsafe {
        let efer = cpu::read_msr(EFER_MSR);
        cpu::write_msr(EFER_MSR, efer | EFER_NXE);
    }

    let pml4 = alloc_table();

    let fine_start = text_start() & !(HUGE_PAGE_SIZE - 1);
    let fine_end = (kernel_image_end() + HUGE_PAGE_SIZE - 1) & !(HUGE_PAGE_SIZE - 1);

    let mut addr = 0u64;
    while addr < IDENTITY_LIMIT {
        if addr >= fine_start && addr < fine_end {
            let mut page = addr;
            while page < addr + HUGE_PAGE_SIZE {
                map4k(pml4, page, page, kernel_page_flags(page));
                page += PAGE_SIZE;
            }
        } else {
            map2m(pml4, addr, addr, WRITABLE | NX);
        }
        addr += HUGE_PAGE_SIZE;
    }

    unsafe {
        KERNEL_PML4 = pml4;
        cpu::write_cr3(pml4);
        // Without CR0.WP, a read-only PTE only blocks CPL=3 writes — the
        // kernel itself (CPL=0) can write through it regardless of the R/W
        // bit, silently defeating W^X for the one privilege level that
        // actually runs code out of this mapping today. Confirmed
        // empirically: a deliberate write to a `.rodata` byte succeeded
        // with no fault until this was added.
        let cr0 = cpu::read_cr0();
        cpu::write_cr0(cr0 | (1 << 16));
    }
}

pub fn kernel_pml4() -> u64 {
    unsafe { KERNEL_PML4 }
}

/// A fresh address space for a new process: its own PML4, sharing the
/// kernel's identity-mapped half by copying `KERNEL_PML4`'s low-address
/// entries (pointers to the *same* physical PDPT/PD/PT tables, not a deep
/// copy — a process never modifies the kernel half, so aliasing it is
/// exactly what should happen: one set of kernel tables, `USER=0`, backing
/// every process). The high entries (user mappings) start empty; `proc::elf`
/// fills them in per process via [`map4k`] with `USER` set.
pub fn new_address_space() -> u64 {
    let pml4 = alloc_table();
    let kernel_entries = indices(IDENTITY_LIMIT).0.max(1);
    unsafe {
        for i in 0..kernel_entries {
            let src = core::ptr::read_volatile(table_entry(KERNEL_PML4, i));
            core::ptr::write_volatile(table_entry(pml4, i), src);
        }
    }
    pml4
}

pub fn switch_to(pml4_phys: u64) {
    unsafe { cpu::write_cr3(pml4_phys) };
}

pub fn current() -> u64 {
    unsafe { cpu::read_cr3() }
}

fn walk_leaf(pml4_phys: u64, virt: u64) -> Option<u64> {
    let (i4, i3, i2, i1) = indices(virt);
    unsafe {
        let e = core::ptr::read_volatile(table_entry(pml4_phys, i4));
        if e & PRESENT == 0 {
            return None;
        }
        let e = core::ptr::read_volatile(table_entry(e & ADDR_MASK, i3));
        if e & PRESENT == 0 {
            return None;
        }
        let e = core::ptr::read_volatile(table_entry(e & ADDR_MASK, i2));
        if e & PRESENT == 0 {
            return None;
        }
        if e & HUGE != 0 {
            return Some(e);
        }
        let e = core::ptr::read_volatile(table_entry(e & ADDR_MASK, i1));
        if e & PRESENT == 0 {
            return None;
        }
        Some(e)
    }
}

/// Whether every page in `[start, start+len)` is present, `USER`-accessible
/// (and, if `need_write`, also writable) in `pml4_phys`'s address space —
/// what every syscall that takes a userspace pointer must check before
/// dereferencing it. A bad pointer here means the syscall fails; it must
/// never mean the kernel dereferences ring-3-supplied memory it hasn't
/// verified the calling process actually owns.
pub fn user_range_ok(pml4_phys: u64, start: u64, len: u64, need_write: bool) -> bool {
    if len == 0 {
        return true;
    }
    let end = match start.checked_add(len) {
        Some(e) => e,
        None => return false,
    };
    let mut page = start & !(PAGE_SIZE - 1);
    while page < end {
        match walk_leaf(pml4_phys, page) {
            Some(e) if e & USER != 0 && (!need_write || e & WRITABLE != 0) => {}
            _ => return false,
        }
        page += PAGE_SIZE;
    }
    true
}
