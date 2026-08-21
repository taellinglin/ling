pub const TAG_TRUE: u64 = 0x7F02_0000_0000_0000;
pub const TAG_FALSE: u64 = 0x7F01_0000_0000_0000;
pub const TAG_UNIT: u64 = 0x7F00_0000_0000_0000;

#[inline(always)]
fn to_f64(bits: u64) -> f64 {
    f64::from_bits(bits)
}

#[inline(always)]
fn from_f64(v: f64) -> u64 {
    v.to_bits()
}

#[inline(always)]
fn bool_val(b: bool) -> u64 {
    if b { TAG_TRUE } else { TAG_FALSE }
}

#[no_mangle]
pub unsafe extern "C" fn ling_f64_add(a: u64, b: u64) -> u64 {
    from_f64(to_f64(a) + to_f64(b))
}

#[no_mangle]
pub unsafe extern "C" fn ling_f64_sub(a: u64, b: u64) -> u64 {
    from_f64(to_f64(a) - to_f64(b))
}

#[no_mangle]
pub unsafe extern "C" fn ling_f64_mul(a: u64, b: u64) -> u64 {
    from_f64(to_f64(a) * to_f64(b))
}

#[no_mangle]
pub unsafe extern "C" fn ling_f64_div(a: u64, b: u64) -> u64 {
    from_f64(to_f64(a) / to_f64(b))
}

#[no_mangle]
pub unsafe extern "C" fn ling_f64_rem(a: u64, b: u64) -> u64 {
    let (fa, fb) = (to_f64(a), to_f64(b));
    let q = (fa / fb) as i64;
    from_f64(fa - (q as f64) * fb)
}

#[no_mangle]
pub unsafe extern "C" fn ling_f64_neg(a: u64) -> u64 {
    from_f64(-to_f64(a))
}

#[no_mangle]
pub unsafe extern "C" fn ling_f64_eq(a: u64, b: u64) -> u64 {
    bool_val(to_f64(a) == to_f64(b))
}

#[no_mangle]
pub unsafe extern "C" fn ling_f64_lt(a: u64, b: u64) -> u64 {
    bool_val(to_f64(a) < to_f64(b))
}

#[no_mangle]
pub unsafe extern "C" fn ling_f64_gt(a: u64, b: u64) -> u64 {
    bool_val(to_f64(a) > to_f64(b))
}

#[no_mangle]
pub unsafe extern "C" fn ling_f64_le(a: u64, b: u64) -> u64 {
    bool_val(to_f64(a) <= to_f64(b))
}

#[no_mangle]
pub unsafe extern "C" fn ling_f64_ge(a: u64, b: u64) -> u64 {
    bool_val(to_f64(a) >= to_f64(b))
}

#[no_mangle]
pub unsafe extern "C" fn ling_sqrt(a: u64) -> u64 {
    let x = to_f64(a);
    if x <= 0.0 {
        return from_f64(0.0);
    }
    // Newton-Raphson approximation
    let mut guess = x;
    for _ in 0..12 {
        guess = 0.5 * (guess + x / guess);
    }
    from_f64(guess)
}

#[no_mangle]
pub unsafe extern "C" fn ling_abs(a: u64) -> u64 {
    let v = to_f64(a);
    from_f64(if v < 0.0 { -v } else { v })
}

#[no_mangle]
pub unsafe extern "C" fn ling_floor(a: u64) -> u64 {
    let v = to_f64(a);
    let i = v as i64;
    let fi = i as f64;
    from_f64(if v < fi { fi - 1.0 } else { fi })
}

#[no_mangle]
pub unsafe extern "C" fn ling_ceil(a: u64) -> u64 {
    let v = to_f64(a);
    let i = v as i64;
    let fi = i as f64;
    from_f64(if v > fi { fi + 1.0 } else { fi })
}

#[no_mangle]
pub unsafe extern "C" fn ling_round(a: u64) -> u64 {
    let v = to_f64(a);
    ling_floor(from_f64(v + 0.5))
}

const PI: f64 = 3.14159265358979323846;
const TWO_PI: f64 = 2.0 * PI;

#[no_mangle]
pub unsafe extern "C" fn ling_sin(a: u64) -> u64 {
    let mut x = to_f64(a);
    x -= (x / TWO_PI) as i64 as f64 * TWO_PI;
    if x > PI {
        x -= TWO_PI;
    } else if x < -PI {
        x += TWO_PI;
    }
    let x2 = x * x;
    let sin = x * (1.0 - x2 * (1.0 / 6.0 - x2 * (1.0 / 120.0 - x2 * (1.0 / 5040.0 - x2 / 362880.0))));
    from_f64(sin)
}

#[no_mangle]
pub unsafe extern "C" fn ling_cos(a: u64) -> u64 {
    ling_sin(from_f64(to_f64(a) + PI * 0.5))
}

#[no_mangle]
pub unsafe extern "C" fn ling_add(a: u64, b: u64) -> u64 {
    if (a >> 56) == 0x7F && (a & 0x00FF_0000_0000_0000) == crate::strings::TAG_KIND_STRING {
        return crate::strings::ling_str_concat(a, b);
    }
    ling_f64_add(a, b)
}

#[no_mangle]
pub unsafe extern "C" fn ling_sub(a: u64, b: u64) -> u64 {
    ling_f64_sub(a, b)
}

#[no_mangle]
pub unsafe extern "C" fn ling_mul(a: u64, b: u64) -> u64 {
    ling_f64_mul(a, b)
}

#[no_mangle]
pub unsafe extern "C" fn ling_div(a: u64, b: u64) -> u64 {
    ling_f64_div(a, b)
}

#[no_mangle]
pub unsafe extern "C" fn ling_rem(a: u64, b: u64) -> u64 {
    ling_f64_rem(a, b)
}

#[no_mangle]
pub unsafe extern "C" fn ling_neg(a: u64, _: u64) -> u64 {
    ling_f64_neg(a)
}

#[no_mangle]
pub unsafe extern "C" fn ling_eq(a: u64, b: u64) -> u64 {
    if a == b {
        return TAG_TRUE;
    }
    if (a >> 56) == 0x7F && (b >> 56) == 0x7F {
        if (a & 0x00FF_0000_0000_0000) == crate::strings::TAG_KIND_STRING
            && (b & 0x00FF_0000_0000_0000) == crate::strings::TAG_KIND_STRING
        {
            return crate::strings::ling_str_eq(a, b);
        }
        return TAG_FALSE;
    }
    ling_f64_eq(a, b)
}

#[no_mangle]
pub unsafe extern "C" fn ling_ne(a: u64, b: u64) -> u64 {
    let eq = ling_eq(a, b);
    if eq == TAG_TRUE { TAG_FALSE } else { TAG_TRUE }
}

#[no_mangle]
pub unsafe extern "C" fn ling_lt(a: u64, b: u64) -> u64 {
    ling_f64_lt(a, b)
}

#[no_mangle]
pub unsafe extern "C" fn ling_le(a: u64, b: u64) -> u64 {
    ling_f64_le(a, b)
}

#[no_mangle]
pub unsafe extern "C" fn ling_gt(a: u64, b: u64) -> u64 {
    ling_f64_gt(a, b)
}

#[no_mangle]
pub unsafe extern "C" fn ling_ge(a: u64, b: u64) -> u64 {
    ling_f64_ge(a, b)
}

#[no_mangle]
pub unsafe extern "C" fn ling_and(a: u64, b: u64) -> u64 {
    let a_bool = a == TAG_TRUE;
    let b_bool = b == TAG_TRUE;
    bool_val(a_bool && b_bool)
}

#[no_mangle]
pub unsafe extern "C" fn ling_or(a: u64, b: u64) -> u64 {
    let a_bool = a == TAG_TRUE;
    let b_bool = b == TAG_TRUE;
    bool_val(a_bool || b_bool)
}

#[no_mangle]
pub unsafe extern "C" fn ling_not(a: u64) -> u64 {
    if a == TAG_TRUE { TAG_FALSE } else { TAG_TRUE }
}

#[no_mangle]
pub unsafe extern "C" fn ling_bool_to_u64(a: u64) -> u64 {
    if a == TAG_TRUE { 1 } else { 0 }
}
