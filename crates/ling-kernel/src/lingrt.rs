//! The Ling value runtime for AOT-compiled `.ling` programs running in-kernel.
//!
//! Compiled Ling code passes every value as a NaN-boxed `u64` (see
//! `ling_codegen::cranelift::runtime`): a raw f64 bit pattern for numbers, or a
//! top byte of `0x7F` plus a kind in bits 48-55 and a 48-bit heap pointer for
//! strings / lists. Operations the compiler can't do inline call `ling_*`
//! functions by a fixed C ABI; everything else — every builtin by name — goes
//! through [`ling_builtin`]. `runtime.rs`/`strings.rs` hold the arithmetic and
//! string primitives; this module adds what a real app (LingStationX) needs:
//! lists, module globals, the `st_*` state store, maths, time, printing, and
//! the by-name dispatcher.
//!
//! **Semantics follow the host interpreter** (`ling/src/runtime/mod.rs`), not
//! the older kernel stubs: number display drops `.0` for integers, `len` and
//! `str_*` count *characters*, `+` concatenates when either side is a string,
//! truthiness treats 0 / "" / [] / unit as false, and lists are *values* —
//! `list_push`/`list_set` return a new list and never change one another binding
//! still holds.
//!
//! **Memory:** nothing is freed yet (the AOT emits no drops for kernel code).
//! Every Ling object comes through this runtime's allocation path so a
//! conservative collector can be added without touching call sites.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

use crate::strings;

// ── Value encoding (keep in sync with ling_codegen::cranelift::runtime) ──────
pub const UNIT: u64 = 0x7F00_0000_0000_0000;
pub const FALSE: u64 = 0x7F01_0000_0000_0000;
pub const TRUE: u64 = 0x7F02_0000_0000_0000;
const TAG_PATTERN: u64 = 0x7F00_0000_0000_0000;
const PTR_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
const K_STR: u64 = 0x0003_0000_0000_0000;
const K_LIST: u64 = 0x0004_0000_0000_0000;

#[inline]
pub fn is_num(v: u64) -> bool {
    (v >> 56) != 0x7F
}
#[inline]
fn kind(v: u64) -> u64 {
    v & 0x00FF_0000_0000_0000
}
#[inline]
pub fn is_str(v: u64) -> bool {
    !is_num(v) && kind(v) == K_STR
}
#[inline]
pub fn is_list(v: u64) -> bool {
    !is_num(v) && kind(v) == K_LIST
}
#[inline]
pub fn num(n: f64) -> u64 {
    n.to_bits()
}
#[inline]
pub fn boolean(b: bool) -> u64 {
    if b {
        TRUE
    } else {
        FALSE
    }
}

/// Make sure the kernel (memory, framebuffer, console) is up before the first
/// allocation. A program like LingStationX never calls `ling_kernel_init`.
#[inline]
pub fn ensure_init() {
    if !crate::is_initialized() {
        crate::init();
    }
}

/// The interpreter's `to_number`: numbers as-is, bools 0/1, strings parsed
/// (trimmed; 0 if not a number), anything else 0.
pub fn to_f64(v: u64) -> f64 {
    if is_num(v) {
        f64::from_bits(v)
    } else if v == TRUE {
        1.0
    } else if is_str(v) {
        str_of(v).trim().parse::<f64>().unwrap_or(0.0)
    } else {
        0.0
    }
}

/// The interpreter's `is_truthy`.
pub fn truthy(v: u64) -> bool {
    if is_num(v) {
        f64::from_bits(v) != 0.0
    } else if v == TRUE {
        true
    } else if v == FALSE || v == UNIT {
        false
    } else if is_str(v) {
        !str_of(v).is_empty()
    } else if is_list(v) {
        !list_items(v).is_empty()
    } else {
        true
    }
}

/// The interpreter's `values_equal`: numbers within 1e-12, strings by content,
/// bools/unit by value; everything else (lists included) is never equal.
pub fn values_equal(a: u64, b: u64) -> bool {
    if is_num(a) && is_num(b) {
        (f64::from_bits(a) - f64::from_bits(b)).abs() < 1e-12
    } else if is_str(a) && is_str(b) {
        str_of(a) == str_of(b)
    } else if (a == TRUE || a == FALSE || a == UNIT) && (b == TRUE || b == FALSE || b == UNIT) {
        a == b
    } else {
        false
    }
}

/// Append the interpreter's `Display` of `v` to `out`.
pub fn fmt_into(out: &mut String, v: u64) {
    if is_num(v) {
        let n = f64::from_bits(v);
        if n - libm::trunc(n) == 0.0 && libm::fabs(n) < 1e15 {
            let _ = write!(out, "{}", n as i64);
        } else {
            let _ = write!(out, "{n}");
        }
    } else if v == TRUE {
        out.push_str("true");
    } else if v == FALSE {
        out.push_str("false");
    } else if v == UNIT {
        out.push_str("()");
    } else if is_str(v) {
        out.push_str(str_of(v));
    } else if is_list(v) {
        out.push('[');
        for (i, &x) in list_items(v).iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            fmt_into(out, x);
        }
        out.push(']');
    } else {
        out.push_str("<value>");
    }
}

pub fn to_string(v: u64) -> String {
    let mut s = String::new();
    fmt_into(&mut s, v);
    s
}

// ── Strings (layout owned by strings.rs: u32 length + UTF-8 bytes) ───────────

/// Borrow a Ling string's text. Non-strings read as "".
pub fn str_of(v: u64) -> &'static str {
    if !is_str(v) {
        return "";
    }
    let bytes = unsafe { strings::bytes_of(v) };
    core::str::from_utf8(bytes).unwrap_or("")
}

pub fn mk_str(s: &str) -> u64 {
    unsafe { strings::ling_str_new(s.as_ptr(), s.len()) }
}

// ── Lists ────────────────────────────────────────────────────────────────────
// A list handle points at a *view* {buf, len} over a shared, append-only
// buffer. Pushing onto the newest view of a buffer appends in place (O(1)
// amortised — the `acc = list_push(acc, x)` accumulator pattern) without
// disturbing older views, which still see only their own prefix. Pushing onto
// an older view, or `list_set`, copies — so lists keep value semantics with no
// reference counting.

#[repr(C)]
struct ListBuf {
    hdr: GcHdr,
    items: Vec<u64>,
}

#[repr(C)]
struct ListView {
    hdr: GcHdr,
    buf: *mut ListBuf,
    len: usize,
}

fn new_buf(items: Vec<u64>) -> *mut ListBuf {
    unsafe {
        GC_SINCE += items.capacity() * 8;
        let p = gc_alloc(core::mem::size_of::<ListBuf>(), GK_BUF) as *mut ListBuf;
        core::ptr::write(&raw mut (*p).items, items);
        p
    }
}

fn new_view(buf: *mut ListBuf, len: usize) -> u64 {
    let p = gc_alloc(core::mem::size_of::<ListView>(), GK_VIEW) as *mut ListView;
    unsafe {
        (*p).buf = buf;
        (*p).len = len;
    }
    TAG_PATTERN | K_LIST | (p as u64 & PTR_MASK)
}

fn new_list(items: Vec<u64>) -> u64 {
    ensure_init();
    let len = items.len();
    let buf = new_buf(items);
    new_view(buf, len)
}

fn view_of(v: u64) -> Option<&'static ListView> {
    if !is_list(v) {
        return None;
    }
    let p = (v & PTR_MASK) as *const ListView;
    if p.is_null() {
        None
    } else {
        Some(unsafe { &*p })
    }
}

/// The elements a list value holds (empty for non-lists).
pub fn list_items(v: u64) -> &'static [u64] {
    match view_of(v) {
        Some(view) => unsafe { &(&(*view.buf).items)[..view.len] },
        None => &[],
    }
}

pub fn list_from(items: Vec<u64>) -> u64 {
    new_list(items)
}

fn list_push(list: u64, x: u64) -> u64 {
    if let Some(view) = view_of(list) {
        let buf = unsafe { &mut *view.buf };
        if view.len == buf.items.len() {
            buf.items.push(x);
            unsafe { GC_SINCE += 8 };
            return new_view(view.buf, view.len + 1);
        }
        let mut items = buf.items[..view.len].to_vec();
        items.push(x);
        return new_list(items);
    }
    new_list(alloc::vec![x])
}

fn list_set(list: u64, idx: f64, x: u64) -> u64 {
    match view_of(list) {
        Some(_) => {
            let mut items = list_items(list).to_vec();
            if idx >= 0.0 && (idx as usize) < items.len() {
                items[idx as usize] = x;
            }
            new_list(items)
        },
        None => new_list(Vec::new()),
    }
}

fn list_get(list: u64, idx: f64) -> u64 {
    let items = list_items(list);
    if idx >= 0.0 && (idx as usize) < items.len() && is_list(list) {
        items[idx as usize]
    } else {
        // Out of range / not a list: the empty string, like the interpreter.
        mk_str("")
    }
}

#[no_mangle]
pub extern "C" fn ling_list_new() -> u64 {
    new_list(Vec::new())
}

#[no_mangle]
pub extern "C" fn ling_list_push(list: u64, val: u64) -> u64 {
    ensure_init();
    list_push(list, val)
}

#[no_mangle]
pub extern "C" fn ling_list_get(list: u64, idx: u64) -> u64 {
    ensure_init();
    list_get(list, to_f64(idx))
}

#[no_mangle]
pub extern "C" fn ling_list_set(list: u64, idx: u64, val: u64) -> u64 {
    ensure_init();
    list_set(list, to_f64(idx), val)
}

#[no_mangle]
pub extern "C" fn ling_list_len(list: u64) -> u64 {
    num(list_items(list).len() as f64)
}

// ── Garbage collection ───────────────────────────────────────────────────────
// Compiled Ling never frees a value (no refcounts, no drops), so a program that
// runs for an hour -- the desktop, a DAW -- allocates until the frame allocator
// is empty. Instead every string, list view and list buffer starts with a
// `GcHdr` and is threaded on one all-objects list; at a safepoint (`present()`,
// an app returning) a conservative mark-sweep walks the roots -- the globals,
// the `st_*` store, the scopes put aside by `run_isolated_with`, and every
// stack word (plus the callee-saved registers) that looks like a value -- and
// frees whatever nothing reached. Conservative: a stray integer can keep an
// object alive, but a live one is never freed, because compiled code only ever
// holds values in registers or on its stack, and Rust code only transiently
// inside a builtin (never across a safepoint). The in-kernel interpreter keeps
// values in its own heap structures, so it blocks collection while it runs.

#[repr(C)]
pub(crate) struct GcHdr {
    /// The next object on the all-objects list; bit 0 is the mark.
    next: usize,
    magic: u32,
    kind: u32,
}

pub(crate) const GC_HDR: usize = core::mem::size_of::<GcHdr>();
const GC_MAGIC: u32 = 0x4C47_4F42; // "LGOB"
const GK_TAG: u32 = 0x5A3C_0000;
pub(crate) const GK_STR: u32 = GK_TAG | 1;
const GK_VIEW: u32 = GK_TAG | 2;
const GK_BUF: u32 = GK_TAG | 3;
/// Allocation volume that triggers a collection at the next frame (the
/// console's `gc <MiB>` lowers it, to stress the collector).
static mut GC_TRIGGER: usize = 16 << 20;

static mut GC_HEAD: usize = 0;
static mut GC_OBJECTS: usize = 0;
static mut GC_SINCE: usize = 0;
static mut GC_BLOCK: u32 = 0;
static mut GC_BUSY: bool = false;
static mut GC_LAST_MS: u64 = 0;
static mut GC_RUNS: u32 = 0;
static mut GC_FREED: u64 = 0;

/// A new collector-managed object of `size` bytes (header included), linked
/// on the all-objects list. Never null: out of memory is a kernel panic.
pub(crate) fn gc_alloc(size: usize, kind: u32) -> *mut u8 {
    let p = {
        let _g = crate::mm::IrqLock::acquire();
        crate::mm::heap::alloc(size)
    };
    if p.is_null() {
        crate::kernel_panic("out of memory (heap allocator exhausted)");
    }
    unsafe {
        let h = p as *mut GcHdr;
        (*h).next = GC_HEAD;
        (*h).magic = GC_MAGIC;
        (*h).kind = kind;
        GC_HEAD = p as usize;
        GC_OBJECTS += 1;
        GC_SINCE += size;
    }
    p
}

/// The header at `addr`, if that really is a live object of `kind`: in heap
/// memory (so it can be read at all), aligned, with the magic and kind words.
unsafe fn gc_header(addr: usize, kind: u32) -> Option<*mut GcHdr> {
    if addr == 0 || addr & 7 != 0 || !crate::mm::heap::is_heap_page(addr as u64) {
        return None;
    }
    let h = addr as *mut GcHdr;
    if core::ptr::read_volatile(&raw const (*h).magic) != GC_MAGIC || core::ptr::read_volatile(&raw const (*h).kind) != kind {
        return None;
    }
    Some(h)
}

/// Mark `h`; false if it already was.
unsafe fn gc_set_mark(h: *mut GcHdr) -> bool {
    if (*h).next & 1 != 0 {
        return false;
    }
    (*h).next |= 1;
    true
}

unsafe fn mark_value(v: u64, work: &mut Vec<*mut ListBuf>) {
    if is_num(v) {
        return;
    }
    let addr = (v & PTR_MASK) as usize;
    match kind(v) {
        K_STR => {
            if addr >= GC_HDR {
                if let Some(h) = gc_header(addr - GC_HDR, GK_STR) {
                    gc_set_mark(h);
                }
            }
        },
        K_LIST => {
            if let Some(h) = gc_header(addr, GK_VIEW) {
                if gc_set_mark(h) {
                    let buf = (*(h as *mut ListView)).buf as usize;
                    if let Some(bh) = gc_header(buf, GK_BUF) {
                        if gc_set_mark(bh) {
                            work.push(bh as *mut ListBuf);
                        }
                    }
                }
            }
        },
        _ => {},
    }
}

/// A stack word: a tagged value, or a raw pointer to a string's length word
/// (what a kernel intrinsic's argument is once the compiler strips the tag --
/// it may outlive the tagged value it was made from).
unsafe fn mark_word(w: u64, work: &mut Vec<*mut ListBuf>) {
    if (w >> 56) == 0x7F {
        mark_value(w, work);
    } else if w >= GC_HDR as u64 && w < 1 << 32 {
        if let Some(h) = gc_header(w as usize - GC_HDR, GK_STR) {
            gc_set_mark(h);
        }
    }
}

/// Mark everything the buffers on the work list hold (every element of the
/// whole buffer, not one view's prefix: other views may cover more).
unsafe fn drain(work: &mut Vec<*mut ListBuf>) {
    while let Some(b) = work.pop() {
        let items = &*(&raw const (*b).items);
        for i in 0..items.len() {
            mark_value(items[i], work);
        }
    }
}

unsafe fn scan_range(lo: u64, hi: u64, work: &mut Vec<*mut ListBuf>) {
    let mut a = (lo + 7) & !7;
    while a + 8 <= hi {
        mark_word(core::ptr::read_volatile(a as *const u64), work);
        a += 8;
    }
}

/// Spill the callee-saved registers (a live value may sit in one across the
/// call that got us here) and scan this stack from the current sp up to `top`.
#[inline(never)]
unsafe fn scan_current_stack(top: u64, work: &mut Vec<*mut ListBuf>) {
    let mut regs = [0u64; 24];
    let sp: u64;
    #[cfg(target_arch = "aarch64")]
    core::arch::asm!(
        "stp x19, x20, [{p}]",
        "stp x21, x22, [{p}, #16]",
        "stp x23, x24, [{p}, #32]",
        "stp x25, x26, [{p}, #48]",
        "stp x27, x28, [{p}, #64]",
        "stp x29, x30, [{p}, #80]",
        "stp d8, d9, [{p}, #96]",
        "stp d10, d11, [{p}, #112]",
        "stp d12, d13, [{p}, #128]",
        "stp d14, d15, [{p}, #144]",
        "mov {o}, sp",
        p = in(reg) regs.as_mut_ptr(),
        o = out(reg) sp,
        options(nostack)
    );
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!(
        "mov [{p}], rbx",
        "mov [{p} + 8], rbp",
        "mov [{p} + 16], r12",
        "mov [{p} + 24], r13",
        "mov [{p} + 32], r14",
        "mov [{p} + 40], r15",
        "mov {o}, rsp",
        p = in(reg) regs.as_mut_ptr(),
        o = out(reg) sp,
        options(nostack)
    );
    scan_range(sp, top, work);
    core::hint::black_box(&regs);
}

unsafe fn scan_stacks(work: &mut Vec<*mut ListBuf>) {
    // The stack we are on (an app's own, or the host's), then every stack
    // parked by a stack switch: the host's while an app runs, suspended apps'.
    // Their registers were pushed on those stacks by the switch.
    let (top, others) = crate::lingapp::gc_stacks();
    scan_current_stack(top, work);
    for (lo, hi) in others {
        scan_range(lo, hi, work);
    }
    crate::proc::sched::for_each_suspended_stack(|lo, hi| scan_range(lo, hi, work));
    crate::coro::for_each_parked_stack(|lo, hi| scan_range(lo, hi, work));
}

/// Free every unmarked object and clear the marks; (objects, bytes) freed.
unsafe fn sweep() -> (usize, usize) {
    let (mut freed, mut bytes) = (0usize, 0usize);
    let mut link: *mut usize = &raw mut GC_HEAD;
    while *link != 0 {
        let h = *link as *mut GcHdr;
        if (*h).next & 1 != 0 {
            (*h).next &= !1;
            link = &raw mut (*h).next;
        } else {
            *link = (*h).next;
            bytes += free_object(h);
            freed += 1;
        }
    }
    GC_OBJECTS -= freed;
    (freed, bytes)
}

unsafe fn free_object(h: *mut GcHdr) -> usize {
    let size = match (*h).kind {
        GK_BUF => {
            let b = h as *mut ListBuf;
            let n = (*b).items.capacity() * 8;
            core::ptr::drop_in_place(&raw mut (*b).items);
            core::mem::size_of::<ListBuf>() + n
        },
        GK_VIEW => core::mem::size_of::<ListView>(),
        _ => GC_HDR + 4 + strings::header_len((h as *const u8).add(GC_HDR)),
    };
    // A stale pointer to this slot must never pass `gc_header` again.
    (*h).magic = 0;
    (*h).kind = 0;
    let _g = crate::mm::IrqLock::acquire();
    crate::mm::heap::free(h as *mut u8);
    size
}

/// Collect now. Only at a safepoint: `present()`, or an app having returned.
pub fn gc_collect() {
    unsafe {
        if GC_BLOCK > 0 || GC_BUSY || GC_HEAD == 0 || crate::coro::in_coro() {
            return;
        }
        GC_BUSY = true;
    }
    let t0 = crate::arch::timer::now_us();
    let before = unsafe { GC_OBJECTS };
    let mut work: Vec<*mut ListBuf> = Vec::new();
    let (freed, bytes) = unsafe {
        for &v in globals().values() {
            mark_value(v, &mut work);
        }
        for e in store().values() {
            mark_value(e.val, &mut work);
        }
        for (g, s) in saved_scopes().iter() {
            for &v in g.values() {
                mark_value(v, &mut work);
            }
            for e in s.values() {
                mark_value(e.val, &mut work);
            }
        }
        for p in parked().iter() {
            for &v in p.g.values() {
                mark_value(v, &mut work);
            }
            for e in p.s.values() {
                mark_value(e.val, &mut work);
            }
        }
        {
            let w = &mut work;
            let mut mark = |v: u64| mark_value(v, w);
            crate::lingrec::roots(&mut mark);
            crate::coro::roots(&mut mark);
            for f in (*(&raw const ROOT_FNS)).iter().flatten() {
                f(&mut mark);
            }
        }
        drain(&mut work);
        scan_stacks(&mut work);
        drain(&mut work);
        sweep()
    };
    let us = crate::arch::timer::now_us().saturating_sub(t0);
    unsafe {
        GC_SINCE = 0;
        GC_RUNS += 1;
        GC_FREED += freed as u64;
        GC_LAST_MS = crate::arch::timer::now_ms();
        GC_BUSY = false;
        let runs = GC_RUNS;
        if TRACE || runs <= 3 || runs % 25 == 0 {
            let s = alloc::format!(
                "[gc] #{}: freed {} of {} objects ({} KiB) in {} ms, {} MiB free\n",
                runs,
                freed,
                before,
                bytes >> 10,
                us / 1000,
                crate::mm::frame::free_frame_count() * 4 / 1024
            );
            crate::console_write(s.as_bytes());
        }
    }
}

/// Once per frame, from `present()`: collect when enough has been allocated
/// since the last time, sooner when memory is getting low.
pub fn gc_maybe() {
    unsafe {
        if GC_BLOCK > 0 {
            return;
        }
        let low = crate::mm::frame::free_frame_count() < (48 << 20) / 4096;
        let since = crate::arch::timer::now_ms().wrapping_sub(GC_LAST_MS);
        if GC_SINCE >= GC_TRIGGER || (low && GC_SINCE >= 1 << 20 && since >= 2000) {
            gc_collect();
        }
    }
}

/// Hold collection off (nested) while Rust code keeps values somewhere the
/// collector does not look -- the in-kernel interpreter's environments.
pub fn gc_block(on: bool) {
    unsafe {
        if on {
            GC_BLOCK += 1;
        } else {
            GC_BLOCK = GC_BLOCK.saturating_sub(1);
        }
    }
}

pub fn gc_set_trigger(bytes: usize) {
    unsafe { GC_TRIGGER = bytes.max(64 << 10) };
}

pub fn gc_stats() -> String {
    let (objects, since, runs, freed) = unsafe { (GC_OBJECTS, GC_SINCE, GC_RUNS, GC_FREED) };
    alloc::format!("gc: {objects} objects live, {} KiB allocated since the last of {runs} collections, {freed} objects freed in all", since >> 10)
}

// ── Maths / time / print ABI ─────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn ling_sin(x: f64) -> f64 {
    libm::sin(x)
}
#[no_mangle]
pub extern "C" fn ling_cos(x: f64) -> f64 {
    libm::cos(x)
}
#[no_mangle]
pub extern "C" fn ling_sqrt(x: f64) -> f64 {
    libm::sqrt(x)
}
#[no_mangle]
pub extern "C" fn ling_abs(x: f64) -> f64 {
    libm::fabs(x)
}
#[no_mangle]
pub extern "C" fn ling_floor(x: f64) -> f64 {
    libm::floor(x)
}
#[no_mangle]
pub extern "C" fn ling_ceil(x: f64) -> f64 {
    libm::ceil(x)
}
#[no_mangle]
pub extern "C" fn ling_round(x: f64) -> f64 {
    libm::round(x)
}

/// Seconds since the kernel came up (the interpreter reports seconds since the
/// program started — the same thing here, since the program *is* the kernel).
#[no_mangle]
pub extern "C" fn ling_time_now() -> u64 {
    num(crate::arch::timer::now_us() as f64 / 1_000_000.0)
}

#[no_mangle]
pub extern "C" fn ling_print_val(v: u64) -> u64 {
    ensure_init();
    let s = to_string(v);
    crate::console_write(s.as_bytes());
    UNIT
}

#[no_mangle]
pub extern "C" fn ling_print_newline() -> u64 {
    ensure_init();
    crate::console_write(b"\n");
    UNIT
}

// ── Module globals ───────────────────────────────────────────────────────────
// `compile_path_standalone` makes `__main__` record every top-level `bind`
// with `__ling_global_set` before the entry runs; functions read them back
// with `__ling_global`.

static mut GLOBALS: BTreeMap<Vec<u8>, u64> = BTreeMap::new();

fn globals() -> &'static mut BTreeMap<Vec<u8>, u64> {
    unsafe { &mut *(&raw mut GLOBALS) }
}

// ── `st_*` state store ───────────────────────────────────────────────────────
// A name → value map the app keeps its mutable state in (Ling binds are
// immutable). `st_lpush`/`st_lset`/`st_ldel` edit a stored list in place;
// `shared` marks a list that's been handed out (by `st_get` or stored from a
// caller's value), so the next in-place edit copies first — the same
// copy-on-write the host gets from `Rc::make_mut`.

struct StEntry {
    val: u64,
    shared: bool,
}

static mut STORE: BTreeMap<Vec<u8>, StEntry> = BTreeMap::new();

fn store() -> &'static mut BTreeMap<Vec<u8>, StEntry> {
    unsafe { &mut *(&raw mut STORE) }
}

/// Run a bundled app's entry with its own fresh globals and `st_*` store, then
/// restore the host program's (e.g. the desktop's) — two compiled programs in
/// one image must not see each other's module-level binds.
pub fn run_isolated(entry: unsafe extern "C" fn() -> u64) -> u64 {
    run_isolated_with(|| unsafe { entry() })
}

/// `run_isolated` for any program body -- the `ling run` interpreter's.
pub fn run_isolated_with<F: FnOnce() -> u64>(body: F) -> u64 {
    let saved_g = core::mem::take(globals());
    let saved_s = core::mem::take(store());
    let saved_a = core::mem::take(args_vec());
    // Put aside where the collector still sees them: the host's values must
    // survive the collections that happen while the app runs.
    saved_scopes().push((saved_g, saved_s));
    unsafe { APP_DEPTH += 1 };
    let r = body();
    unsafe { APP_DEPTH -= 1 };
    let (saved_g, saved_s) = saved_scopes().pop().unwrap_or_default();
    *globals() = saved_g;
    *store() = saved_s;
    *args_vec() = saved_a;
    // The app's globals and store just went away: everything it built is
    // garbage now, give it back before the host draws its next frame.
    gc_collect();
    r
}

type Scope = (BTreeMap<Vec<u8>, u64>, BTreeMap<Vec<u8>, StEntry>);
static mut SAVED: Vec<Scope> = Vec::new();

fn saved_scopes() -> &'static mut Vec<Scope> {
    unsafe { &mut *(&raw mut SAVED) }
}

/// A program's Ling scope (globals, `st_*` store, arguments) held aside by a
/// coroutine (`coro.rs`) while another one runs.
#[derive(Default)]
pub struct LingScope {
    g: BTreeMap<Vec<u8>, u64>,
    s: BTreeMap<Vec<u8>, StEntry>,
    a: Vec<String>,
}

/// Take the live scope out (the live one is empty afterwards).
pub fn scope_take() -> LingScope {
    LingScope { g: core::mem::take(globals()), s: core::mem::take(store()), a: core::mem::take(args_vec()) }
}

/// Make `sc` the live scope (the live one must have been taken).
pub fn scope_put(sc: LingScope) {
    *globals() = sc.g;
    *store() = sc.s;
    *args_vec() = sc.a;
}

pub fn scope_drop(sc: LingScope) {
    drop(sc);
}

/// Every value a held-aside scope keeps alive (for the collector).
pub fn scope_values(sc: &LingScope, mark: &mut dyn FnMut(u64)) {
    for &v in sc.g.values() {
        mark(v);
    }
    for e in sc.s.values() {
        mark(e.val);
    }
}

/// More GC roots: Rust-side holders of Ling values (channel queues, the
/// shared store, record forms) report them through these.
pub type RootFn = fn(&mut dyn FnMut(u64));
static mut ROOT_FNS: [Option<RootFn>; 16] = [None; 16];

pub fn add_roots(f: RootFn) {
    let r = unsafe { &mut *(&raw mut ROOT_FNS) };
    if r.iter().any(|x| *x == Some(f)) {
        return;
    }
    if let Some(slot) = r.iter_mut().find(|x| x.is_none()) {
        *slot = Some(f);
    }
}

/// A suspended app's globals, store, command line and nesting depth, parked
/// while its host (the desktop) runs again; collector roots like the rest.
struct Parked {
    id: u64,
    g: BTreeMap<Vec<u8>, u64>,
    s: BTreeMap<Vec<u8>, StEntry>,
    a: Vec<String>,
    depth: u32,
}
static mut PARKED: Vec<Parked> = Vec::new();
static mut PARK_SEQ: u64 = 0;

fn parked() -> &'static mut Vec<Parked> {
    unsafe { &mut *(&raw mut PARKED) }
}

/// Inside a running app that is about to be suspended: its scope goes aside
/// and the host's (the newest `run_isolated_with` put it on `SAVED`) comes
/// back. Returns the ticket `unpark_scope` takes.
pub fn park_scope() -> u64 {
    let id = unsafe {
        PARK_SEQ += 1;
        PARK_SEQ
    };
    let depth = unsafe { APP_DEPTH };
    parked().push(Parked { id, g: core::mem::take(globals()), s: core::mem::take(store()), a: core::mem::take(args_vec()), depth });
    if let Some((g, s)) = saved_scopes().pop() {
        *globals() = g;
        *store() = s;
    }
    unsafe { APP_DEPTH = depth.saturating_sub(1) };
    id
}

/// Back inside the app: the host's scope returns to `SAVED`, the app's comes back.
pub fn unpark_scope(id: u64) {
    let Some(i) = parked().iter().position(|p| p.id == id) else { return };
    let p = parked().remove(i);
    saved_scopes().push((core::mem::take(globals()), core::mem::take(store())));
    *globals() = p.g;
    *store() = p.s;
    *args_vec() = p.a;
    unsafe { APP_DEPTH = p.depth };
}

static mut APP_DEPTH: u32 = 0;

/// The running program's command line, what `get_args()` reports: empty for a
/// bundled app (which sees its own name), `[file, args...]` for `ling run`.
static mut ARGS: Vec<String> = Vec::new();

fn args_vec() -> &'static mut Vec<String> {
    unsafe { &mut *(&raw mut ARGS) }
}

/// Set the command line for the program `run_isolated_with` is about to run.
pub fn set_args(args: &[String]) {
    *args_vec() = args.to_vec();
}

/// How many launched apps are running (0 = only the image's own program, e.g.
/// the desktop) — Ctrl+Q closes only a launched app.
pub fn app_depth() -> u32 {
    unsafe { APP_DEPTH }
}

/// The stored list for `key`, made private (unshared) and created empty if
/// absent or not a list; returns its buffer so the caller can edit in place.
fn st_private_list(key: Vec<u8>) -> &'static mut StEntry {
    let st = store();
    let e = st.entry(key).or_insert(StEntry { val: UNIT, shared: false });
    if !is_list(e.val) || e.shared {
        let items = list_items(e.val).to_vec();
        e.val = new_list(items);
        e.shared = false;
    }
    e
}

/// In-place edit of a private stored list: `f` gets the element Vec; the view's
/// length is resynced afterwards. Safe because a private entry's buffer and
/// view are referenced only by the store.
fn st_edit_list(key: Vec<u8>, f: impl FnOnce(&mut Vec<u64>)) {
    let e = st_private_list(key);
    let view = (e.val & PTR_MASK) as *mut ListView;
    unsafe {
        let buf = &mut *(*view).buf;
        // A private view always covers the whole buffer.
        buf.items.truncate((*view).len);
        f(&mut buf.items);
        (*view).len = buf.items.len();
    }
}

fn st_builtin(name: &[u8], args: &[u64]) -> Option<u64> {
    let key = || -> Vec<u8> { args.first().map(|&v| to_string(v).into_bytes()).unwrap_or_default() };
    let arg = |i: usize| args.get(i).copied();
    Some(match name {
        b"st_set" => {
            let v = arg(1).unwrap_or(UNIT);
            store().insert(key(), StEntry { val: v, shared: true });
            UNIT
        },
        b"st_get" => match store().get_mut(&key()) {
            Some(e) => {
                e.shared = true;
                e.val
            },
            None => arg(1).unwrap_or(num(0.0)),
        },
        b"st_has" => num(store().contains_key(&key()) as i32 as f64),
        b"st_del" => num(store().remove(&key()).is_some() as i32 as f64),
        b"st_add" => {
            let k = key();
            let cur = match store().get(&k) {
                Some(e) if is_num(e.val) => f64::from_bits(e.val),
                _ => 0.0,
            };
            let nv = num(cur + arg(1).map(to_f64).unwrap_or(0.0));
            store().insert(k, StEntry { val: nv, shared: false });
            nv
        },
        b"st_llen" => num(match store().get(&key()) {
            Some(e) if is_list(e.val) => list_items(e.val).len() as f64,
            _ => 0.0,
        }),
        b"st_lget" => {
            let i = arg(1).map(to_f64).unwrap_or(0.0);
            match store().get(&key()) {
                Some(e) if is_list(e.val) => {
                    let items = list_items(e.val);
                    if i >= 0.0 && (i as usize) < items.len() {
                        items[i as usize]
                    } else {
                        num(0.0)
                    }
                },
                _ => num(0.0),
            }
        },
        b"st_lset" => {
            let i = arg(1).map(to_f64).unwrap_or(0.0).max(0.0) as usize;
            if i < (1 << 22) {
                let v = arg(2).unwrap_or(UNIT);
                st_edit_list(key(), |items| {
                    while items.len() <= i {
                        items.push(num(0.0));
                    }
                    items[i] = v;
                });
            }
            UNIT
        },
        b"st_lpush" => {
            let v = arg(1).unwrap_or(UNIT);
            st_edit_list(key(), |items| items.push(v));
            UNIT
        },
        b"st_ldel" => {
            let i = arg(1).map(to_f64).unwrap_or(0.0);
            let k = key();
            if matches!(store().get(&k), Some(e) if is_list(e.val)) {
                st_edit_list(k, |items| {
                    if i >= 0.0 && (i as usize) < items.len() {
                        items.remove(i as usize);
                    }
                });
            }
            UNIT
        },
        b"st_lclear" => {
            store().insert(key(), StEntry { val: new_list(Vec::new()), shared: false });
            UNIT
        },
        _ => return None,
    })
}

// ── Generic builtins (strings, lists, maths, misc) ───────────────────────────

static mut RAND_STATE: u64 = 0x9E37_79B9_7F4A_7C15;

fn rand_f64() -> f64 {
    unsafe {
        let mut x = RAND_STATE;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        RAND_STATE = x;
        (x >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn arg_str(args: &[u64], i: usize, d: &str) -> String {
    match args.get(i) {
        Some(&v) => to_string(v),
        None => String::from(d),
    }
}

fn arg_num(args: &[u64], i: usize, d: f64) -> f64 {
    args.get(i).map(|&v| to_f64(v)).unwrap_or(d)
}

fn char_count(s: &str) -> usize {
    s.chars().count()
}

fn core_builtin(name: &[u8], args: &[u64]) -> Option<u64> {
    let n = |i: usize, d: f64| arg_num(args, i, d);
    Some(match name {
        // Module globals.
        b"__ling_global" => {
            let g = args.first().map(|&v| str_of(v)).unwrap_or("");
            globals().get(g.as_bytes()).copied().unwrap_or(UNIT)
        },
        b"__ling_global_set" => {
            if let Some(&k) = args.first() {
                globals().insert(str_of(k).as_bytes().to_vec(), args.get(1).copied().unwrap_or(UNIT));
            }
            UNIT
        },

        // Output.
        b"print" | b"println" => {
            let mut s = String::new();
            for &a in args {
                fmt_into(&mut s, a);
            }
            s.push('\n');
            crate::console_write(s.as_bytes());
            UNIT
        },

        // Maths.
        b"abs" => num(libm::fabs(n(0, 0.0))),
        b"floor" => num(libm::floor(n(0, 0.0))),
        b"ceil" => num(libm::ceil(n(0, 0.0))),
        b"round" => num(libm::round(n(0, 0.0))),
        b"sqrt" => num(libm::sqrt(n(0, 0.0))),
        b"atan2" => num(libm::atan2(n(0, 0.0), n(1, 1.0))),
        b"sin" => num(libm::sin(n(0, 0.0))),
        b"cos" => num(libm::cos(n(0, 0.0))),
        b"exp" => num(libm::exp(n(0, 0.0))),
        b"log10" => num(libm::log10(n(0, 1.0))),
        b"pow" => num(libm::pow(n(0, 0.0), n(1, 0.0))),
        b"min" => num(n(0, 0.0).min(n(1, 0.0))),
        b"max" => num(n(0, 0.0).max(n(1, 0.0))),
        b"rand" => num(rand_f64()),
        b"time_now" => ling_time_now(),

        // Conversions.
        b"to_number" => num(arg_str(args, 0, "").trim().parse::<f64>().unwrap_or(0.0)),
        b"num_str" => mk_str(&to_string(args.first().copied().unwrap_or(UNIT))),

        // Lists.
        b"len" | b"str_len" => num(match args.first() {
            Some(&v) if is_str(v) => char_count(str_of(v)) as f64,
            Some(&v) if is_list(v) => list_items(v).len() as f64,
            _ => 0.0,
        }),
        b"list_new" => new_list(Vec::new()),
        b"list_get" => list_get(args.first().copied().unwrap_or(UNIT), n(1, 0.0)),
        b"list_push" => list_push(args.first().copied().unwrap_or(UNIT), args.get(1).copied().unwrap_or(UNIT)),
        b"list_set" => list_set(
            args.first().copied().unwrap_or(UNIT),
            n(1, 0.0),
            args.get(2).copied().unwrap_or(UNIT),
        ),

        // Strings.
        b"str_split" => {
            let s = arg_str(args, 0, "");
            let sep = arg_str(args, 1, "\n");
            let sep = if sep.is_empty() { String::from("\n") } else { sep };
            new_list(s.split(sep.as_str()).map(mk_str).collect())
        },
        b"str_trim" | b"trim" => mk_str(arg_str(args, 0, "").trim()),
        b"str_starts_with" => boolean(arg_str(args, 0, "").starts_with(arg_str(args, 1, "").as_str())),
        b"str_ends_with" => boolean(arg_str(args, 0, "").ends_with(arg_str(args, 1, "").as_str())),
        b"str_replace" => {
            let s = arg_str(args, 0, "");
            let from = arg_str(args, 1, "");
            if from.is_empty() {
                mk_str(&s)
            } else {
                mk_str(&s.replace(from.as_str(), arg_str(args, 2, "").as_str()))
            }
        },
        b"str_find" => {
            let s = arg_str(args, 0, "");
            let needle = arg_str(args, 1, "");
            num(s.find(needle.as_str()).map(|bi| char_count(&s[..bi]) as f64).unwrap_or(-1.0))
        },
        b"str_slice" | b"substr" => {
            let s = arg_str(args, 0, "");
            let start = n(1, 0.0).max(0.0) as usize;
            let total = char_count(&s);
            let len = match args.get(2) {
                Some(&v) => to_f64(v).max(0.0) as usize,
                None => total.saturating_sub(start),
            };
            let out: String = s.chars().skip(start).take(len).collect();
            mk_str(&out)
        },
        b"str_lower" => mk_str(&arg_str(args, 0, "").to_lowercase()),
        b"str_upper" => mk_str(&arg_str(args, 0, "").to_uppercase()),

        // The program's command line: `ling run <file> [args]`'s, or just the
        // bundled app's name.
        b"get_args" => {
            let a = args_vec();
            if a.is_empty() {
                new_list(alloc::vec![mk_str("lingstation")])
            } else {
                new_list(a.iter().map(|s| mk_str(s)).collect())
            }
        },

        _ => return None,
    })
}

// ── Builtin dispatch ─────────────────────────────────────────────────────────

/// Extension point for builtins implemented elsewhere in the kernel (pixel
/// drawing, input, files, the DAW engine). Returns `None` if `name` isn't one.
pub type ExtDispatch = fn(&[u8], &[u64]) -> Option<u64>;
static mut EXT: [Option<ExtDispatch>; 8] = [None; 8];

/// Register another builtin table (e.g. `px_*` drawing). Earlier registrations
/// are tried first.
pub fn register(f: ExtDispatch) {
    unsafe {
        let ext = &mut *(&raw mut EXT);
        if let Some(slot) = ext.iter_mut().find(|s| s.is_none()) {
            *slot = Some(f);
        }
    }
}

static mut REPORTED: BTreeSet<Vec<u8>> = BTreeSet::new();
/// Console `trace`: print each builtin name the first time a program calls
/// it -- a cheap way to see how far a start-up got before a hang.
pub static mut TRACE: bool = false;
/// Builtin calls so far (the frame-timing overlay shows the per-frame count).
pub static mut BUILTIN_CALLS: u64 = 0;
const DISPATCH_SLOTS: usize = 1024;
/// Off: with the cache on, serial input died after a Settings click in two
/// of three QEMU runs (builtin dispatch is logically unchanged by it, so the
/// mechanism is not understood yet -- see project memory); off, every run passes.
const DISPATCH_ON: bool = true;
/// (name pointer, FNV-1a of the name's bytes, dispatcher) per slot: the hash keeps a
/// reused pointer holding a different name from being routed to the wrong dispatcher.
static mut DISPATCH_CACHE: [(usize, u64, u8); DISPATCH_SLOTS] = [(0, 0, 0); DISPATCH_SLOTS];

#[inline]
fn name_hash(name: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in name {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

pub fn gc_runs() -> u32 {
    unsafe { GC_RUNS }
}
static mut TRACED: BTreeSet<Vec<u8>> = BTreeSet::new();

fn trace_first(name: &[u8]) {
    let seen = unsafe { &mut *(&raw mut TRACED) };
    if seen.insert(name.to_vec()) {
        crate::console_write(b"[trace] ");
        crate::console_write(name);
        crate::console_write(b"
");
    }
    // The last few calls, for the hang report below.
    let ring = unsafe { &mut *(&raw mut RING) };
    let i = unsafe { RING_N } % 16;
    let n = name.len().min(31);
    ring[i][..n].copy_from_slice(&name[..n]);
    ring[i][31] = n as u8;
    unsafe { RING_N += 1 };
}

static mut RING: [[u8; 32]; 16] = [[0; 32]; 16];
static mut RING_N: usize = 0;
static mut LAST_PRESENT_MS: u64 = 0;
static mut HANG_REPORTED: bool = false;

/// `present()` happened (a frame is on screen): the program is alive.
pub fn mark_present() {
    unsafe {
        LAST_PRESENT_MS = crate::arch::timer::now_ms();
        HANG_REPORTED = false;
    }
}

/// From the timer tick: with `trace` on, a program that has not presented a
/// frame for 10 s gets its last builtin calls printed once -- where it hangs.
pub fn hang_tick() {
    unsafe {
        if !TRACE || HANG_REPORTED || APP_DEPTH == 0 || LAST_PRESENT_MS == 0 {
            return;
        }
        if crate::arch::timer::now_ms().wrapping_sub(LAST_PRESENT_MS) < 10_000 {
            return;
        }
        HANG_REPORTED = true;
        crate::console_write(b"[trace] no frame for 10 s; last builtin calls (oldest first):
");
        let ring = &*(&raw const RING);
        let total = RING_N;
        for k in 0..16usize {
            let idx = (total + k) % 16;
            let n = ring[idx][31] as usize;
            if n > 0 {
                crate::console_write(b"  ");
                crate::console_write(&ring[idx][..n]);
                crate::console_write(b"
");
            }
        }
        crate::console_write(b"[trace] total builtin calls: ");
        let mut b = [0u8; 20];
        let mut m = 0;
        let mut x = total;
        if x == 0 {
            b[0] = b'0';
            m = 1;
        }
        while x > 0 {
            b[m] = b'0' + (x % 10) as u8;
            x /= 10;
            m += 1;
        }
        b[..m].reverse();
        crate::console_write(&b[..m]);
        crate::console_write(b"
");
    }
}


/// Note (once per name) that a builtin isn't implemented in-kernel yet.
pub(crate) fn report_missing(name: &[u8]) {
    let rep = unsafe { &mut *(&raw mut REPORTED) };
    if rep.insert(name.to_vec()) {
        crate::console_write(b"[lingrt] builtin not implemented: ");
        crate::console_write(name);
        crate::console_write(b"\n");
    }
}

/// Generic by-name builtin call emitted by the AOT for every builtin it doesn't
/// inline: `name` (UTF-8, `name_len` bytes) and `args_len` NaN-boxed arguments.
#[no_mangle]
pub unsafe extern "C" fn ling_builtin(
    name_ptr: *const u8,
    name_len: usize,
    args_ptr: *const u64,
    args_len: usize,
) -> u64 {
    ensure_init();
    let name = core::slice::from_raw_parts(name_ptr, name_len);
    let args: &[u64] = if args_len == 0 { &[] } else { core::slice::from_raw_parts(args_ptr, args_len) };
    if TRACE {
        trace_first(name);
    }
    BUILTIN_CALLS += 1;
    // Compiled code hands over the same static name for a call site every
    // time, so the dispatcher that answered it last time is tried first: a
    // drawing call skips the long core match (hundreds of names) that way.
    let slot = ((name_ptr as usize) >> 3) & (DISPATCH_SLOTS - 1);
    let (cached_ptr, cached_hash, stage) = DISPATCH_CACHE[slot];
    let hash = name_hash(name);
    if DISPATCH_ON && cached_ptr == name_ptr as usize && cached_hash == hash {
        let hit = match stage {
            1 => st_builtin(name, args),
            2 => crate::lingapp::dispatch(name, args),
            3 => crate::lingdaw::dispatch(name, args),
            4 => crate::netapi::dispatch(name, args),
            5 => crate::lingrec::dispatch(name, args),
            _ => core_builtin(name, args),
        };
        if let Some(v) = hit {
            return v;
        }
    }
    if let Some(v) = core_builtin(name, args) {
        DISPATCH_CACHE[slot] = (name_ptr as usize, hash, 0);
        return v;
    }
    if let Some(v) = st_builtin(name, args) {
        DISPATCH_CACHE[slot] = (name_ptr as usize, hash, 1);
        return v;
    }
    if let Some(v) = crate::lingapp::dispatch(name, args) {
        DISPATCH_CACHE[slot] = (name_ptr as usize, hash, 2);
        return v;
    }
    if let Some(v) = crate::lingdaw::dispatch(name, args) {
        DISPATCH_CACHE[slot] = (name_ptr as usize, hash, 3);
        return v;
    }
    if let Some(v) = crate::netapi::dispatch(name, args) {
        DISPATCH_CACHE[slot] = (name_ptr as usize, hash, 4);
        return v;
    }
    if let Some(v) = crate::lingrec::dispatch(name, args) {
        DISPATCH_CACHE[slot] = (name_ptr as usize, hash, 5);
        return v;
    }
    for f in (*(&raw const EXT)).iter().flatten() {
        if let Some(v) = f(name, args) {
            return v;
        }
    }
    report_missing(name);
    UNIT
}
