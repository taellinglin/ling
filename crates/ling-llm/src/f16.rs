//! IEEE half floats: the scales of the quantized blocks and the KV cache are stored as f16.

/// f16 bits to f32 (exact)
#[inline]
pub fn to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) & 1) as u32;
    let exp = ((h >> 10) & 0x1f) as u32;
    let man = (h & 0x3ff) as u32;
    let bits = if exp == 0 {
        if man == 0 {
            sign << 31
        } else {
            // a subnormal half: normalise it
            let mut e = 127 - 15 + 1;
            let mut m = man;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            (sign << 31) | ((e as u32) << 23) | ((m & 0x3ff) << 13)
        }
    } else if exp == 31 {
        (sign << 31) | (0xff << 23) | (man << 13)
    } else {
        (sign << 31) | ((exp + 112) << 23) | (man << 13)
    };
    f32::from_bits(bits)
}

/// f32 to the nearest f16 (ties to even)
pub fn from_f32(f: f32) -> u16 {
    let bits = f.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let man = bits & 0x7fffff;
    if exp == 0xff {
        // inf / nan
        return sign | 0x7c00 | if man != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 31 {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        // subnormal
        let m = man | 0x800000;
        let shift = (14 - e) as u32;
        let half = m >> shift;
        let rem = m & ((1 << shift) - 1);
        let mid = 1 << (shift - 1);
        let r = if rem > mid || (rem == mid && (half & 1) == 1) { half + 1 } else { half };
        return sign | r as u16;
    }
    let half = ((e as u32) << 10) | (man >> 13);
    let rem = man & 0x1fff;
    let r = if rem > 0x1000 || (rem == 0x1000 && (half & 1) == 1) { half + 1 } else { half };
    sign | r as u16
}

/// the whole table, for reading the cache with a lookup instead of bit twiddling
pub fn table() -> alloc::vec::Vec<f32> {
    (0..=u16::MAX).map(to_f32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halves_round_trip() {
        for h in 0..=u16::MAX {
            let f = to_f32(h);
            if f.is_nan() {
                continue;
            }
            assert_eq!(from_f32(f), h, "{h:#06x} -> {f}");
        }
        assert_eq!(to_f32(0x3c00), 1.0);
        assert_eq!(to_f32(0xc000), -2.0);
        assert_eq!(from_f32(0.1), 0x2e66);
        assert_eq!(from_f32(65504.0), 0x7bff);
        assert_eq!(from_f32(1e9), 0x7c00);
    }
}
