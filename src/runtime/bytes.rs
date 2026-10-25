//! ling-wallet runtime additions: byte and number helpers
//! (../ling-wallet/docs/LING_API.md §2).
//!
//! Bytes cross the Ling boundary as hex strings. Every hex input is validated
//! strictly (even length, `[0-9a-fA-F]` only); output is always lowercase.
//! Error convention (LING_API.md): builtins documented as returning
//! `ok(v)`/`bad(reason)` report malformed input with `bad(..)`; builtins that
//! return a plain value raise a runtime error. Nothing here panics on input.

use std::rc::Rc;

use super::{EvalErr, EvalResult, Interpreter, Value};

/// 2^53: Ling numbers are exact integers strictly below this.
pub(super) const MAX_SAFE: u64 = 1 << 53;

// ───────────────────────────────────────────────────────────── shared helpers

/// Lowercase hex (table-driven: signatures are ~100 KB).
pub(super) fn to_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = Vec::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(DIGITS[(b >> 4) as usize]);
        out.push(DIGITS[(b & 15) as usize]);
    }
    // Only ASCII hex digits were pushed.
    String::from_utf8(out).unwrap_or_default()
}

#[inline]
fn nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Strict hex decode: even length, hex digits only (no whitespace, no `0x`).
pub(super) fn from_hex(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if b.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(b.len() / 2);
    for pair in b.chunks_exact(2) {
        out.push(nibble(pair[0])? << 4 | nibble(pair[1])?);
    }
    Some(out)
}

pub(super) fn rt_err(name: &str, msg: &str) -> EvalErr {
    EvalErr::Runtime(format!("{name}: {msg}"))
}

pub(super) fn ok_val(v: Value) -> Value {
    Value::Ok(Box::new(v))
}

pub(super) fn bad_val(msg: &str) -> Value {
    Value::Err(Box::new(Value::Str(msg.to_string())))
}

pub(super) fn list_val(v: Vec<Value>) -> Value {
    Value::List(Rc::new(v))
}

/// Argument `i` as a string (must be a Ling string, not coerced).
pub(super) fn str_arg(args: &[Value], i: usize) -> Result<&str, String> {
    match args.get(i) {
        Some(Value::Str(s)) => Ok(s.as_str()),
        Some(_) => Err(format!("argument {} must be a string", i + 1)),
        None => Err(format!("missing argument {}", i + 1)),
    }
}

/// Argument `i` as strictly validated hex bytes.
pub(super) fn hex_arg(args: &[Value], i: usize) -> Result<Vec<u8>, String> {
    let s = str_arg(args, i)?;
    from_hex(s).ok_or_else(|| format!("argument {} is not valid hex", i + 1))
}

/// Argument `i` as exactly `N` hex-encoded bytes.
pub(super) fn hex_arg_n<const N: usize>(args: &[Value], i: usize) -> Result<[u8; N], String> {
    let mut v = hex_arg(args, i)?;
    let out: Result<[u8; N], _> = v.as_slice().try_into();
    let len = v.len();
    wipe_bytes(&mut v);
    out.map_err(|_| format!("argument {} must be {} bytes (got {})", i + 1, N, len))
}

/// Argument `i` as a list of hex strings.
pub(super) fn hex_list_arg(args: &[Value], i: usize) -> Result<Vec<Vec<u8>>, String> {
    match args.get(i) {
        Some(Value::List(items)) => items
            .iter()
            .enumerate()
            .map(|(k, v)| match v {
                Value::Str(s) => from_hex(s).ok_or_else(|| format!("list item {k} is not valid hex")),
                _ => Err(format!("list item {k} must be a hex string")),
            })
            .collect(),
        Some(_) => Err(format!("argument {} must be a list", i + 1)),
        None => Err(format!("missing argument {}", i + 1)),
    }
}

/// Argument `i` as a non-negative integer `<= max` (Ling numbers only).
pub(super) fn uint_arg(args: &[Value], i: usize, max: u64) -> Result<u64, String> {
    match args.get(i) {
        Some(Value::Number(n)) => {
            let n = *n;
            if !n.is_finite() || n < 0.0 || n.fract() != 0.0 || n >= MAX_SAFE as f64 || n as u64 > max {
                return Err(format!("argument {} must be an integer in 0..={max}", i + 1));
            }
            Ok(n as u64)
        },
        Some(_) => Err(format!("argument {} must be a number", i + 1)),
        None => Err(format!("missing argument {}", i + 1)),
    }
}

/// Best-effort volatile wipe of a byte buffer (the optimizer may not elide it).
pub(super) fn wipe_bytes(buf: &mut [u8]) {
    for b in buf.iter_mut() {
        // SAFETY: `b` is a valid, aligned, exclusive reference.
        unsafe { std::ptr::write_volatile(b, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

// ───────────────────────────────────────────────────────────── base32 / base64url

const B32: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

fn base32_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(5) * 8);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for &b in data {
        acc = (acc << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(B32[((acc >> bits) & 31) as usize] as char);
        }
        acc &= (1 << bits) - 1;
    }
    if bits > 0 {
        out.push(B32[((acc << (5 - bits)) & 31) as usize] as char);
    }
    out
}

/// RFC 4648 base32 without padding. Case-insensitive; rejects padding,
/// foreign characters, impossible lengths and non-zero trailing bits, so
/// every byte string has exactly one accepted (case-folded) encoding.
fn base32_decode(s: &str) -> Option<Vec<u8>> {
    if matches!(s.len() % 8, 1 | 3 | 6) {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() * 5 / 8);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for c in s.bytes() {
        let v = match c {
            b'a'..=b'z' => c - b'a',
            b'A'..=b'Z' => c - b'A',
            b'2'..=b'7' => c - b'2' + 26,
            _ => return None,
        } as u32;
        acc = (acc << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
        acc &= (1 << bits) - 1;
    }
    if acc != 0 {
        return None; // non-canonical trailing bits
    }
    Some(out)
}

fn b64url_engine() -> base64::engine::GeneralPurpose {
    use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
    GeneralPurpose::new(
        &base64::alphabet::URL_SAFE,
        GeneralPurposeConfig::new()
            .with_encode_padding(false)
            .with_decode_padding_mode(DecodePaddingMode::Indifferent)
            .with_decode_allow_trailing_bits(false),
    )
}

// ───────────────────────────────────────────────────────────── u256 decimal

/// Unsigned 256-bit integer, little-endian u64 limbs.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) struct U256(pub [u64; 4]);

impl U256 {
    const ZERO: U256 = U256([0; 4]);

    fn is_zero(&self) -> bool {
        self.0 == [0; 4]
    }

    /// `self * m + a`, or None on overflow past 2^256.
    fn mul_small_add(&self, m: u64, a: u64) -> Option<U256> {
        let mut out = [0u64; 4];
        let mut carry = a as u128;
        for i in 0..4 {
            let t = self.0[i] as u128 * m as u128 + carry;
            out[i] = t as u64;
            carry = t >> 64;
        }
        (carry == 0).then_some(U256(out))
    }

    /// `(self / d, self % d)` for a small non-zero divisor.
    fn divmod_small(&self, d: u64) -> (U256, u64) {
        let mut out = [0u64; 4];
        let mut rem: u128 = 0;
        for i in (0..4).rev() {
            let cur = (rem << 64) | self.0[i] as u128;
            out[i] = (cur / d as u128) as u64;
            rem = cur % d as u128;
        }
        (U256(out), rem as u64)
    }

    /// Decimal digits only (leading zeros allowed), at most 2^256 − 1.
    pub(super) fn parse_decimal(s: &str) -> Option<U256> {
        if s.is_empty() || s.len() > 100 || !s.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let mut v = U256::ZERO;
        for c in s.bytes() {
            v = v.mul_small_add(10, (c - b'0') as u64)?;
        }
        Some(v)
    }

    pub(super) fn to_decimal(&self) -> String {
        if self.is_zero() {
            return "0".to_string();
        }
        const CHUNK: u64 = 10_000_000_000_000_000_000; // 10^19
        let mut parts = Vec::new();
        let mut v = *self;
        while !v.is_zero() {
            let (q, r) = v.divmod_small(CHUNK);
            parts.push(r);
            v = q;
        }
        let mut s = parts.last().map(|p| p.to_string()).unwrap_or_default();
        for p in parts.iter().rev().skip(1) {
            s.push_str(&format!("{p:019}"));
        }
        s
    }

    fn checked_add(&self, o: &U256) -> Option<U256> {
        let mut out = [0u64; 4];
        let mut carry = false;
        for i in 0..4 {
            let (s1, c1) = self.0[i].overflowing_add(o.0[i]);
            let (s2, c2) = s1.overflowing_add(carry as u64);
            out[i] = s2;
            carry = c1 || c2;
        }
        (!carry).then_some(U256(out))
    }

    fn checked_sub(&self, o: &U256) -> Option<U256> {
        let (r, borrow) = self.overflowing_sub(o);
        (!borrow).then_some(r)
    }

    fn overflowing_sub(&self, o: &U256) -> (U256, bool) {
        let mut out = [0u64; 4];
        let mut borrow = false;
        for i in 0..4 {
            let (d1, b1) = self.0[i].overflowing_sub(o.0[i]);
            let (d2, b2) = d1.overflowing_sub(borrow as u64);
            out[i] = d2;
            borrow = b1 || b2;
        }
        (U256(out), borrow)
    }

    fn checked_mul(&self, o: &U256) -> Option<U256> {
        let mut prod = [0u64; 8];
        for i in 0..4 {
            let mut carry: u128 = 0;
            for j in 0..4 {
                let t = self.0[i] as u128 * o.0[j] as u128 + prod[i + j] as u128 + carry;
                prod[i + j] = t as u64;
                carry = t >> 64;
            }
            prod[i + 4] = carry as u64;
        }
        prod[4..].iter().all(|&l| l == 0).then_some(U256([prod[0], prod[1], prod[2], prod[3]]))
    }

    fn cmp_u(&self, o: &U256) -> std::cmp::Ordering {
        for i in (0..4).rev() {
            match self.0[i].cmp(&o.0[i]) {
                std::cmp::Ordering::Equal => continue,
                ord => return ord,
            }
        }
        std::cmp::Ordering::Equal
    }

    /// Floor division; None on division by zero.
    fn checked_div(&self, d: &U256) -> Option<U256> {
        if d.is_zero() {
            return None;
        }
        let mut q = [0u64; 4];
        let mut r = U256::ZERO;
        for bit in (0..256).rev() {
            // r = (r << 1) | bit_of(self); remember the bit shifted out.
            let top = r.0[3] >> 63;
            for i in (1..4).rev() {
                r.0[i] = (r.0[i] << 1) | (r.0[i - 1] >> 63);
            }
            r.0[0] = (r.0[0] << 1) | ((self.0[bit / 64] >> (bit % 64)) & 1);
            if top == 1 || r.cmp_u(d) != std::cmp::Ordering::Less {
                r = r.overflowing_sub(d).0; // exact mod 2^256 even when top == 1
                q[bit / 64] |= 1 << (bit % 64);
            }
        }
        Some(U256(q))
    }
}

/// A u256 argument: decimal string, or a non-negative integer Ling number.
fn u256_arg(args: &[Value], i: usize) -> Result<U256, String> {
    match args.get(i) {
        Some(Value::Str(s)) => {
            U256::parse_decimal(s).ok_or_else(|| format!("argument {} is not an unsigned 256-bit decimal", i + 1))
        },
        Some(Value::Number(_)) => Ok(U256([uint_arg(args, i, MAX_SAFE - 1)?, 0, 0, 0])),
        Some(_) => Err(format!("argument {} must be a decimal string", i + 1)),
        None => Err(format!("missing argument {}", i + 1)),
    }
}

// ───────────────────────────────────────────────────────────── builtins

/// Read a fixed-width little-endian integer at byte `pos`.
fn read_le(data: &[u8], pos: u64, width: usize) -> Option<u64> {
    let pos = usize::try_from(pos).ok()?;
    let end = pos.checked_add(width)?;
    let b = data.get(pos..end)?;
    Some(b.iter().rev().fold(0u64, |acc, &x| (acc << 8) | x as u64))
}

/// Minimal LEB128 decode at `pos` → (value, new_pos). Rejects truncation,
/// non-minimal encodings (a trailing 0x00 byte) and values ≥ 2^53.
fn read_varint(data: &[u8], pos: usize) -> Result<(u64, usize), &'static str> {
    let mut value: u64 = 0;
    for i in 0..8usize {
        let b = *data.get(pos.saturating_add(i)).ok_or("varint truncated")?;
        value |= ((b & 0x7f) as u64) << (7 * i);
        if b & 0x80 == 0 {
            if i > 0 && b == 0 {
                return Err("non-minimal varint");
            }
            if value >= MAX_SAFE {
                return Err("varint exceeds 2^53");
            }
            return Ok((value, pos + i + 1));
        }
    }
    Err("varint exceeds 2^53")
}

fn write_varint(mut n: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(8);
    loop {
        let b = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(b);
            return out;
        }
        out.push(b | 0x80);
    }
}

/// Write `data` to `path` atomically: a fresh temp file in the same
/// directory, fsync, then rename over the target.
fn atomic_write(path: &str, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let rnd = ling_crypto::rng::random_bytes::<8>();
    let tmp = format!("{path}.tmp-{}", to_hex(&rnd));
    let res = (|| {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

impl Interpreter {
    pub(super) fn bytes_builtin(&mut self, name: &str, args: &[Value]) -> Option<EvalResult> {
        let r = match name {
            "hex_concat" | "hex_len" | "hex_slice" | "hex_eq" | "u8_hex" | "u32le_hex" | "u64le_hex"
            | "varint_hex" | "text_hex" | "base32_encode" | "base64url_encode" | "big_add" | "big_sub"
            | "big_mul" | "big_div" | "big_cmp" => plain(name, args).map_err(|e| rt_err(name, &e)),
            "hex_u8" | "hex_u32le" | "hex_u64le" | "hex_varint" | "hex_text" | "base32_decode"
            | "base64url_decode" => Ok(fallible(name, args).unwrap_or_else(|e| bad_val(&e))),
            "file_read_hex" | "file_write_hex" => self.file_hex(name, args),
            // Best-effort no-op: builtins receive *copies* of their arguments
            // (`&[Value]`, already cloned out of the caller's env), so the
            // caller's string buffer is unreachable from here. Wiping it in
            // place needs an env-level hook in mod.rs (see report).
            "wipe" => Ok(Value::Unit),
            _ => return None,
        };
        Some(r)
    }

    fn file_hex(&mut self, name: &str, args: &[Value]) -> EvalResult {
        self.require_cap("fs")?;
        let path = str_arg(args, 0).map_err(|e| rt_err(name, &e))?;
        if name == "file_read_hex" {
            return Ok(match std::fs::read(path) {
                Ok(mut data) => {
                    let h = to_hex(&data);
                    wipe_bytes(&mut data);
                    ok_val(Value::Str(h))
                },
                Err(e) => bad_val(&format!("read failed: {}", e.kind())),
            });
        }
        let mut data = match hex_arg(args, 1) {
            Ok(d) => d,
            Err(e) => return Ok(bad_val(&e)),
        };
        let res = atomic_write(path, &data);
        wipe_bytes(&mut data);
        Ok(match res {
            Ok(()) => ok_val(Value::Unit),
            Err(e) => bad_val(&format!("write failed: {}", e.kind())),
        })
    }
}

/// Builtins returning a plain value; any malformed input is an error.
fn plain(name: &str, args: &[Value]) -> Result<Value, String> {
    Ok(match name {
        "hex_concat" => {
            let parts = hex_list_arg(args, 0)?;
            Value::Str(to_hex(&parts.concat()))
        },
        "hex_len" => Value::Number(hex_arg(args, 0)?.len() as f64),
        "hex_slice" => {
            let data = hex_arg(args, 0)?;
            let start = uint_arg(args, 1, MAX_SAFE - 1)? as usize;
            let len = uint_arg(args, 2, MAX_SAFE - 1)? as usize;
            let end = start.checked_add(len).filter(|&e| e <= data.len()).ok_or_else(|| {
                format!("range {start}..{} out of bounds (length {})", start.saturating_add(len), data.len())
            })?;
            Value::Str(to_hex(&data[start..end]))
        },
        "hex_eq" => {
            use subtle::ConstantTimeEq;
            let a = hex_arg(args, 0)?;
            let b = hex_arg(args, 1)?;
            Value::Bool(a.len() == b.len() && bool::from(a.ct_eq(&b)))
        },
        "u8_hex" => Value::Str(to_hex(&[uint_arg(args, 0, 0xff)? as u8])),
        "u32le_hex" => Value::Str(to_hex(&(uint_arg(args, 0, u32::MAX as u64)? as u32).to_le_bytes())),
        "u64le_hex" => Value::Str(to_hex(&uint_arg(args, 0, MAX_SAFE - 1)?.to_le_bytes())),
        "varint_hex" => Value::Str(to_hex(&write_varint(uint_arg(args, 0, MAX_SAFE - 1)?))),
        "text_hex" => Value::Str(to_hex(str_arg(args, 0)?.as_bytes())),
        "base32_encode" => Value::Str(base32_encode(&hex_arg(args, 0)?)),
        "base64url_encode" => {
            use base64::Engine as _;
            Value::Str(b64url_engine().encode(hex_arg(args, 0)?))
        },
        "big_add" | "big_sub" | "big_mul" | "big_div" | "big_cmp" => {
            let a = u256_arg(args, 0)?;
            let b = u256_arg(args, 1)?;
            let r = match name {
                "big_add" => a.checked_add(&b).ok_or("overflow (result ≥ 2^256)")?,
                "big_sub" => a.checked_sub(&b).ok_or("underflow (b > a)")?,
                "big_mul" => a.checked_mul(&b).ok_or("overflow (result ≥ 2^256)")?,
                "big_div" => a.checked_div(&b).ok_or("division by zero")?,
                _ => {
                    return Ok(Value::Number(match a.cmp_u(&b) {
                        std::cmp::Ordering::Less => -1.0,
                        std::cmp::Ordering::Equal => 0.0,
                        std::cmp::Ordering::Greater => 1.0,
                    }))
                },
            };
            Value::Str(r.to_decimal())
        },
        _ => return Err("unknown builtin".into()),
    })
}

/// Builtins returning `ok(v)`; the `Err` string becomes `bad(reason)`.
fn fallible(name: &str, args: &[Value]) -> Result<Value, String> {
    let v = match name {
        "hex_u8" | "hex_u32le" | "hex_u64le" => {
            let data = hex_arg(args, 0)?;
            let pos = uint_arg(args, 1, MAX_SAFE - 1)?;
            let width = match name {
                "hex_u8" => 1,
                "hex_u32le" => 4,
                _ => 8,
            };
            let n = read_le(&data, pos, width).ok_or("read past end of data")?;
            if n >= MAX_SAFE {
                return Err("u64 value ≥ 2^53 is not exactly representable".into());
            }
            Value::Number(n as f64)
        },
        "hex_varint" => {
            let data = hex_arg(args, 0)?;
            let pos = uint_arg(args, 1, MAX_SAFE - 1)? as usize;
            let (v, np) = read_varint(&data, pos)?;
            list_val(vec![Value::Number(v as f64), Value::Number(np as f64)])
        },
        "hex_text" => {
            let data = hex_arg(args, 0)?;
            Value::Str(String::from_utf8(data).map_err(|_| "invalid UTF-8".to_string())?)
        },
        "base32_decode" => {
            Value::Str(to_hex(&base32_decode(str_arg(args, 0)?).ok_or("invalid base32")?))
        },
        "base64url_decode" => {
            use base64::Engine as _;
            let d = b64url_engine().decode(str_arg(args, 0)?).map_err(|_| "invalid base64url".to_string())?;
            Value::Str(to_hex(&d))
        },
        _ => return Err("unknown builtin".into()),
    };
    Ok(ok_val(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_strict() {
        assert_eq!(from_hex("00ffAb").unwrap(), vec![0, 255, 0xab]);
        assert!(from_hex("0").is_none());
        assert!(from_hex("zz").is_none());
        assert!(from_hex(" 00").is_none());
        assert!(from_hex("é0").is_none());
        assert_eq!(to_hex(&[0, 255, 0xab]), "00ffab");
    }

    #[test]
    fn base32_rfc4648() {
        // RFC 4648 §10 vectors, lowercase, no padding.
        for (i, o) in [
            ("", ""),
            ("f", "my"),
            ("fo", "mzxq"),
            ("foo", "mzxw6"),
            ("foob", "mzxw6yq"),
            ("fooba", "mzxw6ytb"),
            ("foobar", "mzxw6ytboi"),
        ] {
            assert_eq!(base32_encode(i.as_bytes()), o);
            assert_eq!(base32_decode(o).unwrap(), i.as_bytes());
            assert_eq!(base32_decode(&o.to_uppercase()).unwrap(), i.as_bytes());
        }
        assert!(base32_decode("mz").is_none(), "non-zero trailing bits");
        assert!(base32_decode("m").is_none());
        assert!(base32_decode("my======").is_none());
        assert!(base32_decode("m1").is_none());
    }

    #[test]
    fn varint_rules() {
        for n in [0u64, 1, 127, 128, 300, 16384, MAX_SAFE - 1] {
            let e = write_varint(n);
            assert_eq!(read_varint(&e, 0).unwrap(), (n, e.len()));
        }
        assert!(read_varint(&[0x80, 0x00], 0).is_err());
        assert!(read_varint(&[0x80], 0).is_err());
        assert!(read_varint(&write_varint(MAX_SAFE), 0).is_err());
        assert!(read_varint(&[0xff; 9], 0).is_err());
        assert!(read_varint(&[1], usize::MAX).is_err());
    }

    #[test]
    fn u256_arith() {
        let max = "115792089237316195423570985008687907853269984665640564039457584007913129639935";
        let m = U256::parse_decimal(max).unwrap();
        assert_eq!(m.to_decimal(), max);
        assert!(U256::parse_decimal(
            "115792089237316195423570985008687907853269984665640564039457584007913129639936"
        )
        .is_none());
        let one = U256([1, 0, 0, 0]);
        assert!(m.checked_add(&one).is_none());
        assert!(one.checked_sub(&m).is_none());
        assert_eq!(m.checked_div(&m).unwrap(), one);
        assert_eq!(
            m.checked_div(&U256([2, 0, 0, 0])).unwrap().to_decimal(),
            "57896044618658097711785492504343953926634992332820282019728792003956564819967"
        );
        let a = U256::parse_decimal("340282366920938463463374607431768211456").unwrap(); // 2^128
        assert_eq!(a.checked_mul(&a), None);
        assert_eq!(a.checked_mul(&U256([3, 0, 0, 0])).unwrap().to_decimal(), "1020847100762815390390123822295304634368");
        assert_eq!(
            U256::parse_decimal("1000000000000000000000")
                .unwrap()
                .checked_div(&U256::parse_decimal("7").unwrap())
                .unwrap()
                .to_decimal(),
            "142857142857142857142"
        );
        assert!(m.checked_div(&U256::ZERO).is_none());
        assert_eq!(U256::parse_decimal("007").unwrap().to_decimal(), "7");
        assert!(U256::parse_decimal("").is_none());
        assert!(U256::parse_decimal("-1").is_none());
        assert!(U256::parse_decimal("1e3").is_none());
    }
}
