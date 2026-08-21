//! Physical frame (page) allocator: a real buddy allocator, not a bitmap
//! scan or a bump arena that never reclaims. Portable — everything here is
//! architecture-neutral; each `arch::*` backend only has to find its usable
//! physical memory ranges (Multiboot2's memory-map tag on x86_64, the
//! mailbox property interface's `GET_ARM_MEMORY` on aarch64) and hand them
//! to [`add_free_region`].
//!
//! Free blocks are tracked with one bit per block per order (1 = free) in a
//! single static bitmap sized for `MAX_PHYS_BYTES`, and chained through a
//! doubly-linked intrusive free list — the *frames themselves*, while free,
//! store their list's `next`/`prev` physical addresses in their first 16
//! bytes. That's what makes buddy-coalescing on free O(1) instead of an
//! O(n) list scan to find and unlink a specific buddy: a singly-linked list
//! can only remove from the head, but coalescing needs to remove an
//! arbitrary known node (the buddy) the moment it's identified.
use core::ptr;

const FRAME_SIZE: usize = 4096;
const FRAME_SHIFT: u32 = 12;
const MAX_ORDER: usize = 10; // orders 0..=10 => 4KiB..=4MiB blocks
/// Matches `boot.rs`'s identity-mapped ceiling (4 PDs of 2MiB pages = 4GiB).
/// Allocating physical memory this allocator can't address would be a
/// correctness bug, not a missed optimization — this cap keeps the two in
/// lockstep until Phase 3's paging rewrite can map (and this allocator can
/// track) more.
const MAX_PHYS_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const TOTAL_FRAMES: usize = (MAX_PHYS_BYTES / FRAME_SIZE as u64) as usize;

const fn bitmap_bits(order: usize) -> usize {
    TOTAL_FRAMES >> order
}

const fn bitmap_bytes(order: usize) -> usize {
    bitmap_bits(order).div_ceil(8)
}

const fn bitmap_offset(order: usize) -> usize {
    let mut off = 0;
    let mut o = 0;
    while o < order {
        off += bitmap_bytes(o);
        o += 1;
    }
    off
}

const TOTAL_BITMAP_BYTES: usize = bitmap_offset(MAX_ORDER + 1);

static mut BITMAP: [u8; TOTAL_BITMAP_BYTES] = [0; TOTAL_BITMAP_BYTES];
/// Physical address of each order's free-list head, or `NONE_ADDR` if empty.
/// Address 0 can't be used as the sentinel — frame 0 is a legitimate
/// (if never actually freed, since it's always below `kernel_end`) address.
const NONE_ADDR: u64 = u64::MAX;
static mut FREE_HEAD: [u64; MAX_ORDER + 1] = [NONE_ADDR; MAX_ORDER + 1];
static mut FREE_COUNT: [usize; MAX_ORDER + 1] = [0; MAX_ORDER + 1];

fn block_index(addr: u64, order: usize) -> usize {
    ((addr >> FRAME_SHIFT) >> order) as usize
}

fn bit_get(order: usize, idx: usize) -> bool {
    unsafe {
        let byte = BITMAP[bitmap_offset(order) + idx / 8];
        byte & (1 << (idx % 8)) != 0
    }
}

fn bit_set(order: usize, idx: usize, val: bool) {
    unsafe {
        let slot = &mut BITMAP[bitmap_offset(order) + idx / 8];
        if val {
            *slot |= 1 << (idx % 8);
        } else {
            *slot &= !(1 << (idx % 8));
        }
    }
}

#[repr(C)]
struct FreeNode {
    next: u64,
    prev: u64,
}

unsafe fn node_at(addr: u64) -> *mut FreeNode {
    addr as *mut FreeNode
}

fn list_push_front(order: usize, addr: u64) {
    unsafe {
        let head = FREE_HEAD[order];
        ptr::write_volatile(node_at(addr), FreeNode { next: head, prev: NONE_ADDR });
        if head != NONE_ADDR {
            (*node_at(head)).prev = addr;
        }
        FREE_HEAD[order] = addr;
        FREE_COUNT[order] += 1;
    }
}

fn list_remove(order: usize, addr: u64) {
    unsafe {
        let node = ptr::read_volatile(node_at(addr));
        if node.prev != NONE_ADDR {
            (*node_at(node.prev)).next = node.next;
        } else {
            FREE_HEAD[order] = node.next;
        }
        if node.next != NONE_ADDR {
            (*node_at(node.next)).prev = node.prev;
        }
        FREE_COUNT[order] -= 1;
    }
}

fn list_pop_front(order: usize) -> Option<u64> {
    unsafe {
        let addr = FREE_HEAD[order];
        if addr == NONE_ADDR {
            return None;
        }
        list_remove(order, addr);
        Some(addr)
    }
}

/// Mark one order-0 frame free, coalescing with its buddy up through
/// however many orders actually merge. Internal: callers use
/// [`add_free_region`] (boot-time seeding) or [`free_frames`] (an
/// allocation's owner giving it back).
fn free_one_frame(addr: u64) {
    free_frames(addr, 0);
}

/// Register `[base, base+len)` as usable physical memory. Automatically
/// clips anything below `kernel_end` (this kernel's own image, boot page
/// tables, and stack — a bootloader's memory map has no idea where the
/// ELF it just loaded landed, so it reports that range as ordinary
/// available RAM) and anything at or above `MAX_PHYS_BYTES`. Frame-at-a-
/// time via `free_one_frame` rather than inserting pre-aligned maximal
/// blocks directly: this runs once at boot over at most a few hundred
/// thousand frames (a few million cycles, not a hot path), and reusing the
/// exact same coalescing path `free_frames` uses at runtime is less code
/// and less risk than a second, boot-only insertion algorithm to keep
/// correct in lockstep.
pub fn add_free_region(base: u64, len: u64, kernel_end: u64) {
    let mut start = base.max(kernel_end);
    let end = (base.saturating_add(len)).min(MAX_PHYS_BYTES);
    start = (start + FRAME_SIZE as u64 - 1) & !(FRAME_SIZE as u64 - 1);
    let mut addr = start;
    while addr + FRAME_SIZE as u64 <= end {
        free_one_frame(addr);
        addr += FRAME_SIZE as u64;
    }
}

/// Allocate `2^order` contiguous frames, splitting a larger free block if
/// no exact-order block is free. `None` means genuine exhaustion at every
/// order `>= order`, not a transient failure — there is no swap/reclaim in
/// this kernel.
pub fn alloc_frames(order: usize) -> Option<u64> {
    if order > MAX_ORDER {
        return None;
    }
    let mut found_order = order;
    while found_order <= MAX_ORDER && unsafe { FREE_HEAD[found_order] } == NONE_ADDR {
        found_order += 1;
    }
    if found_order > MAX_ORDER {
        return None;
    }
    let addr = list_pop_front(found_order)?;
    bit_set(found_order, block_index(addr, found_order), false);

    // Split down to the requested order, banking the unused half of each
    // split as a newly free block at the next order down.
    let mut o = found_order;
    while o > order {
        o -= 1;
        let buddy_addr = addr + ((FRAME_SIZE as u64) << o);
        bit_set(o, block_index(buddy_addr, o), true);
        list_push_front(o, buddy_addr);
    }
    Some(addr)
}

/// Return `2^order` contiguous frames starting at `addr` (as returned by a
/// matching `alloc_frames`) to the allocator, coalescing with the buddy at
/// each order while it's also free.
pub fn free_frames(addr: u64, order: usize) {
    let mut addr = addr;
    let mut order = order;
    while order < MAX_ORDER {
        let idx = block_index(addr, order);
        let buddy_idx = idx ^ 1;
        let buddy_addr = (buddy_idx as u64) << (FRAME_SHIFT as u64 + order as u64);
        if buddy_addr >= MAX_PHYS_BYTES || !bit_get(order, buddy_idx) {
            break;
        }
        list_remove(order, buddy_addr);
        bit_set(order, buddy_idx, false);
        addr = addr.min(buddy_addr);
        order += 1;
    }
    bit_set(order, block_index(addr, order), true);
    list_push_front(order, addr);
}

/// Frames currently free, across all orders — a diagnostic (and what
/// `selftest`/a future `free` shell command reports), not used by the
/// allocator itself.
pub fn free_frame_count() -> usize {
    unsafe {
        let mut total = 0usize;
        for o in 0..=MAX_ORDER {
            total += FREE_COUNT[o] << o;
        }
        total
    }
}
