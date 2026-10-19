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

use alloc::boxed::Box;
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

struct ListBuf {
    items: Vec<u64>,
}

struct ListView {
    buf: *mut ListBuf,
    len: usize,
}

fn new_list(items: Vec<u64>) -> u64 {
    ensure_init();
    let len = items.len();
    let buf = Box::into_raw(Box::new(ListBuf { items }));
    let view = Box::into_raw(Box::new(ListView { buf, len }));
    TAG_PATTERN | K_LIST | (view as u64 & PTR_MASK)
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
            let nv = Box::into_raw(Box::new(ListView { buf: view.buf, len: view.len + 1 }));
            return TAG_PATTERN | K_LIST | (nv as u64 & PTR_MASK);
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
        b"str_trim" => mk_str(arg_str(args, 0, "").trim()),
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
        b"str_slice" => {
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

        // The program's command line: just its name, in-kernel.
        b"get_args" => new_list(alloc::vec![mk_str("lingstation")]),

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

/// Note (once per name) that a builtin isn't implemented in-kernel yet.
fn report_missing(name: &[u8]) {
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
    if let Some(v) = core_builtin(name, args) {
        return v;
    }
    if let Some(v) = st_builtin(name, args) {
        return v;
    }
    if let Some(v) = crate::lingapp::dispatch(name, args) {
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
