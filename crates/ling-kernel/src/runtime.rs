//! Ling's generic (NaN-boxed) arithmetic, comparison and logic for compiled
//! kernel `.ling` code — everything the compiler can't prove is a raw number
//! (see `ling_codegen`'s `numtype` analysis and `translate_op_int`).
//!
//! Semantics match the host interpreter's `apply_binop` (`ling/src/runtime`),
//! via the shared helpers in [`crate::lingrt`]:
//! - `+` concatenates when either side is a string (the other side formatted
//!   the interpreter's way), otherwise adds numbers;
//! - `- * / %` and `< <= > >=` convert operands to numbers (`to_number`);
//! - `==`/`!=` use `values_equal` (numbers within 1e-12, strings by content,
//!   bools/unit by value; lists never compare equal);
//! - `and`/`or`/`not` use the interpreter's truthiness (0, "", [], unit and
//!   false are false).
//!
//! Encoding matches `ling_codegen::cranelift::runtime` exactly: numbers are raw
//! f64 bit patterns (top byte != 0x7F); unit/false/true and heap values are
//! tagged with top byte 0x7F and a discriminator in bits 48-55.

use crate::lingrt::{boolean, is_num, is_str, num, to_f64, truthy, values_equal, UNIT};

#[no_mangle]
pub extern "C" fn ling_add(a: u64, b: u64) -> u64 {
    if is_num(a) && is_num(b) {
        return num(f64::from_bits(a) + f64::from_bits(b));
    }
    if is_str(a) || is_str(b) {
        return unsafe { crate::strings::ling_str_concat(a, b) };
    }
    // The interpreter errors here ("cannot add"); compiled code has no error
    // channel, so yield unit the way the JIT bridge does.
    UNIT
}
#[no_mangle]
pub extern "C" fn ling_sub(a: u64, b: u64) -> u64 {
    num(to_f64(a) - to_f64(b))
}
#[no_mangle]
pub extern "C" fn ling_mul(a: u64, b: u64) -> u64 {
    num(to_f64(a) * to_f64(b))
}
#[no_mangle]
pub extern "C" fn ling_div(a: u64, b: u64) -> u64 {
    num(to_f64(a) / to_f64(b))
}
#[no_mangle]
pub extern "C" fn ling_rem(a: u64, b: u64) -> u64 {
    num(to_f64(a) % to_f64(b))
}
#[no_mangle]
pub extern "C" fn ling_neg(a: u64) -> u64 {
    num(-to_f64(a))
}

#[no_mangle]
pub extern "C" fn ling_eq(a: u64, b: u64) -> u64 {
    boolean(values_equal(a, b))
}
#[no_mangle]
pub extern "C" fn ling_ne(a: u64, b: u64) -> u64 {
    boolean(!values_equal(a, b))
}
#[no_mangle]
pub extern "C" fn ling_lt(a: u64, b: u64) -> u64 {
    boolean(to_f64(a) < to_f64(b))
}
#[no_mangle]
pub extern "C" fn ling_le(a: u64, b: u64) -> u64 {
    boolean(to_f64(a) <= to_f64(b))
}
#[no_mangle]
pub extern "C" fn ling_gt(a: u64, b: u64) -> u64 {
    boolean(to_f64(a) > to_f64(b))
}
#[no_mangle]
pub extern "C" fn ling_ge(a: u64, b: u64) -> u64 {
    boolean(to_f64(a) >= to_f64(b))
}

#[no_mangle]
pub extern "C" fn ling_and(a: u64, b: u64) -> u64 {
    boolean(truthy(a) && truthy(b))
}
#[no_mangle]
pub extern "C" fn ling_or(a: u64, b: u64) -> u64 {
    boolean(truthy(a) || truthy(b))
}
#[no_mangle]
pub extern "C" fn ling_not(a: u64) -> u64 {
    boolean(!truthy(a))
}

#[no_mangle]
pub extern "C" fn ling_bool_to_u64(b: u64) -> u64 {
    truthy(b) as u64
}
