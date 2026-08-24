//! Real 4-level AArch64 stage-1 page tables (4KiB granule, 48-bit VA via
//! `TTBR0_EL1` only — `TCR_EL1.EPD1` disables `TTBR1_EL1` walks entirely,
//! since this kernel has no higher-half split: everything, kernel and every
//! process, lives under one low-VA table, exactly like the x86_64 backend's
//! single-PML4 identity map (see that module's doc for why: relocating to a
//! higher half buys address-space ergonomics, not isolation, and the actual
//! boundary here is the AP EL0-access bit, enforced per-page below).
//!
//! Level indexing is bit-for-bit the same split as x86_64's PML4/PDPT/PD/PT
//! (bits `[47:39]`, `[38:30]`, `[29:21]`, `[20:12]`) — the two backends'
//! `map4k` are structurally the same walk, only the leaf descriptor encoding
//! differs. [`USER`]/[`WRITABLE`]/[`NX`] are this kernel's own logical
//! flags, not hardware bit positions: AArch64's AP\[2:1\] field couples
//! "writable" and "EL0-accessible" into one 2-bit encoding rather than two
//! independent bits, so the translation happens inside [`map4k`] instead of
//! being a direct bit copy.
//!
//! The identity range additionally needs a real memory-type distinction
//! that x86_64 gets from firmware-configured MTRRs for free: everything
//! below [`PERIPHERAL_BASE`] is Normal (cacheable) memory, everything at or
//! above it — the BCM2837 peripheral block and the ARM-local timer/IRQ
//! controller `intc.rs` programs — is Device-nGnRnE. Mapping the peripheral
//! range as cacheable Normal memory would silently break the UART and
//! mailbox (reordered/cached MMIO accesses), not just cost performance.
use crate::arch::mmio::PERIPHERAL_BASE;
use core::arch::asm;

pub const WRITABLE: u64 = 1 << 0;
pub const USER: u64 = 1 << 1;
pub const NX: u64 = 1 << 2;

const VALID: u64 = 1 << 0;
const TABLE_OR_PAGE: u64 = 1 << 1; // set at every level: table descriptor (L0-L2) or page descriptor (L3)
const BLOCK: u64 = VALID; // block descriptor (L1/L2 leaf): bit1 clear
const AF: u64 = 1 << 10;
const SH_INNER: u64 = 0b11 << 8;
const ATTR_NORMAL: u64 = 0 << 2; // MAIR_EL1 index 0
const ATTR_DEVICE: u64 = 1 << 2; // MAIR_EL1 index 1
const UXN: u64 = 1 << 54;
const PXN: u64 = 1 << 53;

const ADDR_MASK: u64 = 0x0000_FFFF_FFFF_F000;
const PAGE_SIZE: u64 = 4096;
const BLOCK_SIZE: u64 = 2 * 1024 * 1024;
/// Covers RAM, the BCM2837 peripheral block, and the ARM-local controller
/// (`intc::ARM_LOCAL_BASE` = `0x4000_0000`) with one extra 2MiB block of
/// headroom — see the module doc for why going further would need this
/// kernel's mailbox-reported RAM size checked against it first.
const IDENTITY_LIMIT: u64 = 0x4020_0000;

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

static mut KERNEL_L0: u64 = 0;

unsafe fn table_entry(table_phys: u64, index: usize) -> *mut u64 {
    (table_phys as *mut u64).add(index)
}

fn alloc_table() -> u64 {
    let phys = crate::mm::frame::alloc_frames(0).unwrap_or_else(|| {
        crate::kernel_panic("out of memory (page table allocation failed)")
    });
    unsafe { core::ptr::write_bytes(phys as *mut u8, 0, PAGE_SIZE as usize) };
    phys
}

/// Walk from `table_phys` down to `index`, creating (as a permissive,
/// non-leaf table descriptor) any missing intermediate table — same
/// reasoning as the x86_64 backend's `next_level`: the restriction always
/// lives on the leaf, since a restrictive intermediate entry would silently
/// override a permissive leaf underneath it.
unsafe fn next_level(table_phys: u64, index: usize) -> u64 {
    let entry = table_entry(table_phys, index);
    let raw = core::ptr::read_volatile(entry);
    if raw & VALID != 0 {
        return raw & ADDR_MASK;
    }
    let child = alloc_table();
    core::ptr::write_volatile(entry, (child & ADDR_MASK) | VALID | TABLE_OR_PAGE);
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

/// AArch64 leaf attribute bits (AF, SH, AttrIndx, AP\[2:1\], UXN, PXN) for
/// this kernel's logical [`WRITABLE`]/[`USER`]/[`NX`] flags. `device`
/// selects the MAIR_EL1 memory-type index — see the module doc.
fn leaf_attrs(flags: u64, device: bool) -> u64 {
    let user = flags & USER != 0;
    let writable = flags & WRITABLE != 0;
    let nx = flags & NX != 0;

    // AP[2:1]: (readonly, el0) -> 00=RW/EL1, 01=RW/EL1+EL0, 10=RO/EL1, 11=RO/EL1+EL0.
    let ap: u64 = match (writable, user) {
        (true, true) => 0b01,
        (true, false) => 0b00,
        (false, true) => 0b11,
        (false, false) => 0b10,
    };
    // The kernel must never execute out of a user-writable page and a
    // process must never execute kernel code by jumping into its mapping —
    // enforced independently of `nx`, which only controls the permission
    // the *owning* privilege level actually needs.
    let (uxn, pxn) = if user { (nx, true) } else { (true, nx) };

    let mut attrs = AF | SH_INNER | (ap << 6);
    attrs |= if device { ATTR_DEVICE } else { ATTR_NORMAL };
    if uxn {
        attrs |= UXN;
    }
    if pxn {
        attrs |= PXN;
    }
    attrs
}

/// Map one 4KiB page. `flags` is a combination of [`WRITABLE`], [`USER`],
/// [`NX`].
pub fn map4k(l0_phys: u64, virt: u64, phys: u64, flags: u64) {
    let (i0, i1, i2, i3) = indices(virt);
    let device = phys >= PERIPHERAL_BASE as u64;
    unsafe {
        let l1 = next_level(l0_phys, i0);
        let l2 = next_level(l1, i1);
        let l3 = next_level(l2, i2);
        let entry = table_entry(l3, i3);
        let desc = (phys & ADDR_MASK) | VALID | TABLE_OR_PAGE | leaf_attrs(flags, device);
        core::ptr::write_volatile(entry, desc);
    }
}

/// Map one 2MiB block at the L2 level (`virt`/`phys` must be 2MiB aligned) —
/// the bulk identity map, same role as the x86_64 backend's `map2m`.
fn map2m(l0_phys: u64, virt: u64, phys: u64, flags: u64, device: bool) {
    let (i0, i1, i2, _) = indices(virt);
    unsafe {
        let l1 = next_level(l0_phys, i0);
        let l2 = next_level(l1, i1);
        let entry = table_entry(l2, i2);
        let desc = (phys & ADDR_MASK) | BLOCK | leaf_attrs(flags, device);
        core::ptr::write_volatile(entry, desc);
    }
}

fn kernel_page_flags(addr: u64) -> u64 {
    if addr >= text_start() && addr < text_end() {
        0 // R+X, EL1 only
    } else if addr >= text_end() && addr < rodata_end() {
        NX // R only, EL1 only
    } else {
        WRITABLE | NX // RW, EL1 only, never executable
    }
}

fn tlb_flush_all() {
    unsafe {
        asm!(
            "dsb ishst",
            "tlbi vmalle1is",
            "dsb ish",
            "isb",
            options(nostack)
        );
    }
}

/// Build the kernel's real page tables, point `TTBR0_EL1` at them, and
/// enable the MMU — replacing the identity-implied direct physical
/// addressing every access has used since boot. Must run after
/// `mm::init_physical_memory` (page tables themselves come from the frame
/// allocator). Call exactly once, at boot.
pub fn init() {
    let l0 = alloc_table();

    let fine_start = text_start() & !(BLOCK_SIZE - 1);
    let fine_end = (kernel_image_end() + BLOCK_SIZE - 1) & !(BLOCK_SIZE - 1);

    let mut addr = 0u64;
    while addr < IDENTITY_LIMIT {
        if addr >= fine_start && addr < fine_end {
            let mut page = addr;
            while page < addr + BLOCK_SIZE {
                map4k(l0, page, page, kernel_page_flags(page));
                page += PAGE_SIZE;
            }
        } else {
            let device = addr >= PERIPHERAL_BASE as u64;
            map2m(l0, addr, addr, WRITABLE | NX, device);
        }
        addr += BLOCK_SIZE;
    }

    unsafe {
        KERNEL_L0 = l0;

        let mair: u64 = 0xFF; // index0: Normal WB non-transient RW-allocate; index1 (0x00): Device-nGnRnE
        asm!("msr mair_el1, {}", in(reg) mair, options(nostack));

        // T0SZ=16 (48-bit VA), 4KiB granule, inner+outer WB write-allocate
        // for the walk itself, inner-shareable, TTBR1 disabled (EPD1).
        let tcr: u64 = 16
            | (0b01 << 8)  // IRGN0
            | (0b01 << 10) // ORGN0
            | (0b11 << 12) // SH0
            | (0b00 << 14) // TG0 = 4KiB
            | (1 << 23); // EPD1: no TTBR1 walks
        asm!("msr tcr_el1, {}", in(reg) tcr, options(nostack));

        asm!("msr ttbr0_el1, {}", in(reg) l0, options(nostack));
        asm!("isb", options(nostack, nomem));

        tlb_flush_all();

        let mut sctlr: u64;
        asm!("mrs {}, sctlr_el1", out(reg) sctlr, options(nostack, nomem));
        sctlr |= 1 << 0; // M: enable MMU
        sctlr |= 1 << 2; // C: data cache
        sctlr |= 1 << 12; // I: instruction cache
        asm!("msr sctlr_el1, {}", in(reg) sctlr, options(nostack));
        asm!("isb", options(nostack, nomem));
    }
}

/// A fresh address space for a new process — its own L0 table, sharing the
/// kernel's identity-mapped range by copying `KERNEL_L0`'s low entries
/// (pointers to the same physical L1 tables, not a deep copy). Mirrors the
/// x86_64 backend's `new_address_space` exactly.
pub fn new_address_space() -> u64 {
    let l0 = alloc_table();
    let kernel_entries = indices(IDENTITY_LIMIT).0.max(1);
    unsafe {
        for i in 0..kernel_entries {
            let src = core::ptr::read_volatile(table_entry(KERNEL_L0, i));
            core::ptr::write_volatile(table_entry(l0, i), src);
        }
    }
    l0
}

pub fn switch_to(l0_phys: u64) {
    unsafe {
        asm!("msr ttbr0_el1, {}", in(reg) l0_phys, options(nostack));
        asm!("isb", options(nostack, nomem));
    }
    tlb_flush_all();
}

fn walk_leaf(l0_phys: u64, virt: u64) -> Option<u64> {
    let (i0, i1, i2, i3) = indices(virt);
    unsafe {
        let e = core::ptr::read_volatile(table_entry(l0_phys, i0));
        if e & VALID == 0 {
            return None;
        }
        let e = core::ptr::read_volatile(table_entry(e & ADDR_MASK, i1));
        if e & VALID == 0 {
            return None;
        }
        let e = core::ptr::read_volatile(table_entry(e & ADDR_MASK, i2));
        if e & VALID == 0 {
            return None;
        }
        if e & TABLE_OR_PAGE == 0 {
            return Some(e); // 2MiB block leaf
        }
        let e = core::ptr::read_volatile(table_entry(e & ADDR_MASK, i3));
        if e & VALID == 0 {
            return None;
        }
        Some(e)
    }
}

/// Whether every page in `[start, start+len)` is present, EL0-accessible
/// (and, if `need_write`, also writable) in `l0_phys`'s address space —
/// mirrors the x86_64 backend's `user_range_ok` exactly; every syscall
/// taking a userspace pointer goes through this before dereferencing it.
pub fn user_range_ok(l0_phys: u64, start: u64, len: u64, need_write: bool) -> bool {
    if len == 0 {
        return true;
    }
    let end = match start.checked_add(len) {
        Some(e) => e,
        None => return false,
    };
    let mut page = start & !(PAGE_SIZE - 1);
    while page < end {
        match walk_leaf(l0_phys, page) {
            Some(e) => {
                let ap = (e >> 6) & 0b11;
                let el0 = ap & 0b01 != 0;
                let readonly = ap & 0b10 != 0;
                if !el0 || (need_write && readonly) {
                    return false;
                }
            }
            None => return false,
        }
        page += PAGE_SIZE;
    }
    true
}
