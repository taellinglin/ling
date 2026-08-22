//! The slice of Ling's generic (NaN-boxed) value runtime that doesn't need
//! a heap: arithmetic, comparison, and logic on numbers and booleans.
//! Enough for `if`/`while` conditions and arithmetic on values the
//! compiler can't prove are raw integers at compile time (anything besides
//! a literal or a `ling_kernel_*` call argument — see
//! `ling_codegen`'s `numtype` analysis and `translate_op_int`).
//!
//! List/struct values (`ling_list_*`/`ling_struct_*`) are deliberately NOT
//! implemented here: nothing kernel-side needs them yet, and they'd need
//! more than the bump allocator `ling_str_*` (see `strings.rs`) gets by
//! with. A kernel `.ling` program that uses them will fail to link with a
//! clear "undefined symbol" — correct, not a bug, until they exist.
//!
//! Encoding matches `ling_codegen::cranelift::runtime` exactly: numbers
//! are raw f64 bit patterns (top byte != 0x7F); `unit`/`false`/`true` are
//! tagged values with top byte 0x7F and a discriminator in bits 48-55.

const TAG_FALSE: u64 = 0x7F01_0000_0000_0000;
const TAG_TRUE: u64 = 0x7F02_0000_0000_0000;

fn is_number(v: u64) -> bool {
    (v >> 56) != 0x7F
}

fn as_f64(v: u64) -> f64 {
    f64::from_bits(v)
}

fn from_f64(v: f64) -> u64 {
    v.to_bits()
}

fn encode_bool(b: bool) -> u64 {
    if b {
        TAG_TRUE
    } else {
        TAG_FALSE
    }
}

fn is_truthy(v: u64) -> bool {
    v == TAG_TRUE
}

#[no_mangle]
pub extern "C" fn ling_add(a: u64, b: u64) -> u64 {
    from_f64(as_f64(a) + as_f64(b))
}
#[no_mangle]
pub extern "C" fn ling_sub(a: u64, b: u64) -> u64 {
    from_f64(as_f64(a) - as_f64(b))
}
#[no_mangle]
pub extern "C" fn ling_mul(a: u64, b: u64) -> u64 {
    from_f64(as_f64(a) * as_f64(b))
}
#[no_mangle]
pub extern "C" fn ling_div(a: u64, b: u64) -> u64 {
    from_f64(as_f64(a) / as_f64(b))
}
#[no_mangle]
pub extern "C" fn ling_rem(a: u64, b: u64) -> u64 {
    from_f64(as_f64(a) % as_f64(b))
}
#[no_mangle]
pub extern "C" fn ling_neg(a: u64) -> u64 {
    from_f64(-as_f64(a))
}

/// `==`/`!=`: numbers compare by value, strings by *content* (so
/// `if cmd == "ls"` in a shell works regardless of where each string was
/// allocated), everything else (unit, bool) by raw bit-equality — which is
/// exactly identity for those, since they're singleton tag values anyway.
#[no_mangle]
pub extern "C" fn ling_eq(a: u64, b: u64) -> u64 {
    encode_bool(value_eq(a, b))
}
#[no_mangle]
pub extern "C" fn ling_ne(a: u64, b: u64) -> u64 {
    encode_bool(!value_eq(a, b))
}

fn value_eq(a: u64, b: u64) -> bool {
    use crate::strings::{bytes_of, tag_kind, TAG_KIND_STRING};
    if is_number(a) && is_number(b) {
        as_f64(a) == as_f64(b)
    } else if !is_number(a)
        && !is_number(b)
        && tag_kind(a) == TAG_KIND_STRING
        && tag_kind(b) == TAG_KIND_STRING
    {
        unsafe { bytes_of(a) == bytes_of(b) }
    } else {
        a == b
    }
}
#[no_mangle]
pub extern "C" fn ling_lt(a: u64, b: u64) -> u64 {
    encode_bool(as_f64(a) < as_f64(b))
}
#[no_mangle]
pub extern "C" fn ling_le(a: u64, b: u64) -> u64 {
    encode_bool(as_f64(a) <= as_f64(b))
}
#[no_mangle]
pub extern "C" fn ling_gt(a: u64, b: u64) -> u64 {
    encode_bool(as_f64(a) > as_f64(b))
}
#[no_mangle]
pub extern "C" fn ling_ge(a: u64, b: u64) -> u64 {
    encode_bool(as_f64(a) >= as_f64(b))
}

#[no_mangle]
pub extern "C" fn ling_and(a: u64, b: u64) -> u64 {
    encode_bool(is_truthy(a) && is_truthy(b))
}
#[no_mangle]
pub extern "C" fn ling_or(a: u64, b: u64) -> u64 {
    encode_bool(is_truthy(a) || is_truthy(b))
}
#[no_mangle]
pub extern "C" fn ling_not(a: u64) -> u64 {
    encode_bool(!is_truthy(a))
}

#[no_mangle]
pub extern "C" fn ling_bool_to_u64(b: u64) -> u64 {
    is_truthy(b) as u64
}
