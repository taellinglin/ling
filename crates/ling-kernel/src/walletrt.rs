//! Ling Wallet builtins in the kernel: a port of the host runtime's
//! `src/runtime/bytes.rs` (hex / bytes / U256 / base32 / base64url / varint /
//! file-hex helpers) and `src/runtime/lwcrypto.rs` (the `lw_*` wallet crypto,
//! backed by `ling_crypto::lw` and `ling_crypto::hypertorus`), plus the few
//! general builtins the wallet calls that the kernel did not have yet.
//!
//! **Semantics follow the host exactly** -- these feed the wallet's consensus
//! code, so every output must match byte for byte. Argument rules are the
//! host's: hex is validated strictly (even length, `[0-9a-fA-F]`, output
//! lowercase); integers must be exact non-negative Ling numbers below 2^53.
//!
//! Error convention (host bytes.rs): builtins documented as returning
//! `ok(v)`/`bad(reason)` report malformed input with `bad(reason)` (same
//! reason strings as the host); plain-valued builtins raise a runtime error on
//! the host -- the kernel has no runtime errors, so they return `()` and log
//! the reason once per builtin name. Verifiers answer `false` on bad input.
//!
//! Secret keys never cross into Ling: they live in a per-isolate [`LwState`]
//! behind random 128-bit handles, zeroized when dropped (`lw_key_drop`, or the
//! isolate going away -- see [`on_kill`]). Isolate 0 (an app's main program)
//! is additionally keyed by the running app's name, since every app's main
//! runs as isolate 0.
//!
//! The pure helpers live in [`pure`] (between the PURE markers) so a host
//! test can compile them unchanged against the reference implementations.

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use ling_crypto::hypertorus::{self, HyperTorus, TracePoint};
use ling_crypto::lw::identity::{IdentityKey, IdentityPublic, IdentitySignature};
use ling_crypto::lw::kem1024::{self, KemCiphertext, KemPublic, KemSecret};
use ling_crypto::lw::lrs::{self, KeyImage, OneTimeSecret, PublicKey, RingSignature, SpendSecret};
use ling_crypto::lw::{aead, hash, Zeroizing};

use crate::lingrec::{mk_bad_str, mk_ok};
use crate::lingrt::{boolean, is_list, is_num, is_str, list_from, list_items, mk_str, num, str_of, to_f64, to_string, UNIT};

// ==== PURE BEGIN ====
/// Byte / number / encoding helpers with no kernel dependencies (core + alloc
/// + the `base64`, `blake2` and `argon2` crates only).
pub(crate) mod pure {
    use alloc::string::{String, ToString};
    use alloc::vec::Vec;
    use alloc::{format, vec};

    /// 2^53: Ling numbers are exact integers strictly below this.
    pub const MAX_SAFE: u64 = 1 << 53;

    // ── hex ──────────────────────────────────────────────────────────────────

    /// Lowercase hex.
    pub fn to_hex(bytes: &[u8]) -> String {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut out = Vec::with_capacity(bytes.len() * 2);
        for &b in bytes {
            out.push(DIGITS[(b >> 4) as usize]);
            out.push(DIGITS[(b & 15) as usize]);
        }
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
    pub fn from_hex(s: &str) -> Option<Vec<u8>> {
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

    /// Best-effort volatile wipe of a byte buffer.
    pub fn wipe_bytes(buf: &mut [u8]) {
        for b in buf.iter_mut() {
            // SAFETY: `b` is a valid, aligned, exclusive reference.
            unsafe { core::ptr::write_volatile(b, 0) };
        }
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    }

    /// Constant-time equality of two equal-length byte strings (the host
    /// uses `subtle::ConstantTimeEq`; only the boolean result is observable).
    pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
        if a.len() != b.len() {
            return false;
        }
        let mut d = 0u8;
        for (x, y) in a.iter().zip(b) {
            d |= x ^ y;
        }
        // Keep the optimizer from short-circuiting the loop.
        core::hint::black_box(d) == 0
    }

    // ── base32 / base64url ───────────────────────────────────────────────────

    const B32: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

    pub fn base32_encode(data: &[u8]) -> String {
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
    /// foreign characters, impossible lengths and non-zero trailing bits.
    pub fn base32_decode(s: &str) -> Option<Vec<u8>> {
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
            return None;
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

    pub fn base64url_encode(data: &[u8]) -> String {
        use base64::Engine as _;
        b64url_engine().encode(data)
    }

    pub fn base64url_decode(s: &str) -> Option<Vec<u8>> {
        use base64::Engine as _;
        b64url_engine().decode(s).ok()
    }

    // ── u256 decimal ─────────────────────────────────────────────────────────

    /// Unsigned 256-bit integer, little-endian u64 limbs.
    #[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
    pub struct U256(pub [u64; 4]);

    impl U256 {
        pub const ZERO: U256 = U256([0; 4]);

        pub fn is_zero(&self) -> bool {
            self.0 == [0; 4]
        }

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

        /// Decimal digits only (leading zeros allowed), at most 2^256 - 1.
        pub fn parse_decimal(s: &str) -> Option<U256> {
            if s.is_empty() || s.len() > 100 || !s.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let mut v = U256::ZERO;
            for c in s.bytes() {
                v = v.mul_small_add(10, (c - b'0') as u64)?;
            }
            Some(v)
        }

        pub fn to_decimal(&self) -> String {
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

        pub fn checked_add(&self, o: &U256) -> Option<U256> {
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

        pub fn checked_sub(&self, o: &U256) -> Option<U256> {
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

        pub fn checked_mul(&self, o: &U256) -> Option<U256> {
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

        pub fn cmp_u(&self, o: &U256) -> core::cmp::Ordering {
            for i in (0..4).rev() {
                match self.0[i].cmp(&o.0[i]) {
                    core::cmp::Ordering::Equal => continue,
                    ord => return ord,
                }
            }
            core::cmp::Ordering::Equal
        }

        /// Floor division; None on division by zero.
        pub fn checked_div(&self, d: &U256) -> Option<U256> {
            if d.is_zero() {
                return None;
            }
            let mut q = [0u64; 4];
            let mut r = U256::ZERO;
            for bit in (0..256).rev() {
                let top = r.0[3] >> 63;
                for i in (1..4).rev() {
                    r.0[i] = (r.0[i] << 1) | (r.0[i - 1] >> 63);
                }
                r.0[0] = (r.0[0] << 1) | ((self.0[bit / 64] >> (bit % 64)) & 1);
                if top == 1 || r.cmp_u(d) != core::cmp::Ordering::Less {
                    r = r.overflowing_sub(d).0;
                    q[bit / 64] |= 1 << (bit % 64);
                }
            }
            Some(U256(q))
        }
    }

    // ── fixed-width / varint ─────────────────────────────────────────────────

    /// Read a fixed-width little-endian integer at byte `pos`.
    pub fn read_le(data: &[u8], pos: u64, width: usize) -> Option<u64> {
        let pos = usize::try_from(pos).ok()?;
        let end = pos.checked_add(width)?;
        let b = data.get(pos..end)?;
        Some(b.iter().rev().fold(0u64, |acc, &x| (acc << 8) | x as u64))
    }

    /// Minimal LEB128 decode at `pos` -> (value, new_pos). Rejects truncation,
    /// non-minimal encodings and values >= 2^53.
    pub fn read_varint(data: &[u8], pos: usize) -> Result<(u64, usize), &'static str> {
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

    pub fn write_varint(mut n: u64) -> Vec<u8> {
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

    // ── Argon2id over caller-provided, possibly non-contiguous memory ────────
    //
    // A line-by-line port of the `argon2` 0.5 crate's `hash_password_into`
    // (Argon2id, v0x13, no secret / associated data), the one the host's
    // `ling_crypto::lw::kdf::password_key` calls. The only change: blocks are
    // reached through [`BlockStore`] instead of one `&mut [Block]` slice, so
    // the kernel can back the wallet's 256 MiB with 4 MiB frame chunks (its
    // allocators cannot hand out 256 MiB contiguously).

    /// One 1 KiB Argon2 memory block.
    #[derive(Copy, Clone)]
    #[repr(C, align(64))]
    pub struct Block(pub [u64; 128]);

    impl Block {
        pub const SIZE: usize = 1024;
        pub const fn new() -> Self {
            Block([0; 128])
        }
        fn load(&mut self, input: &[u8; Block::SIZE]) {
            for (i, chunk) in input.chunks(8).enumerate() {
                let mut w = [0u8; 8];
                w.copy_from_slice(chunk);
                self.0[i] = u64::from_le_bytes(w);
            }
        }
        fn xor_with(&mut self, o: &Block) {
            for (d, s) in self.0.iter_mut().zip(o.0.iter()) {
                *d ^= s;
            }
        }
        pub fn wipe(&mut self) {
            for w in self.0.iter_mut() {
                // SAFETY: valid exclusive reference.
                unsafe { core::ptr::write_volatile(w, 0) };
            }
        }
    }

    pub trait BlockStore {
        fn blk(&self, i: usize) -> &Block;
        fn blk_mut(&mut self, i: usize) -> &mut Block;
    }

    const TRUNC: u64 = u32::MAX as u64;

    #[inline(always)]
    fn fbla(a: u64, b: u64) -> u64 {
        a.wrapping_add(b).wrapping_add(2u64.wrapping_mul((a & TRUNC).wrapping_mul(b & TRUNC)))
    }

    #[inline(always)]
    fn gb(v: &mut [u64; 128], a: usize, b: usize, c: usize, d: usize) {
        v[a] = fbla(v[a], v[b]);
        v[d] = (v[d] ^ v[a]).rotate_right(32);
        v[c] = fbla(v[c], v[d]);
        v[b] = (v[b] ^ v[c]).rotate_right(24);
        v[a] = fbla(v[a], v[b]);
        v[d] = (v[d] ^ v[a]).rotate_right(16);
        v[c] = fbla(v[c], v[d]);
        v[b] = (v[b] ^ v[c]).rotate_right(63);
    }

    #[inline(always)]
    fn permute(v: &mut [u64; 128], i: [usize; 16]) {
        gb(v, i[0], i[4], i[8], i[12]);
        gb(v, i[1], i[5], i[9], i[13]);
        gb(v, i[2], i[6], i[10], i[14]);
        gb(v, i[3], i[7], i[11], i[15]);
        gb(v, i[0], i[5], i[10], i[15]);
        gb(v, i[1], i[6], i[11], i[12]);
        gb(v, i[2], i[7], i[8], i[13]);
        gb(v, i[3], i[4], i[9], i[14]);
    }

    /// The Argon2 compression function G (`Block::compress` in the crate).
    pub fn compress(rhs: &Block, lhs: &Block) -> Block {
        let mut r = *rhs;
        r.xor_with(lhs);
        let mut q = r;
        for row in 0..8 {
            let o = row * 16;
            let mut idx = [0usize; 16];
            for (k, x) in idx.iter_mut().enumerate() {
                *x = o + k;
            }
            permute(&mut q.0, idx);
        }
        for i in 0..8 {
            let b = i * 2;
            permute(
                &mut q.0,
                [b, b + 1, b + 16, b + 17, b + 32, b + 33, b + 48, b + 49, b + 64, b + 65, b + 80, b + 81, b + 96, b + 97, b + 112, b + 113],
            );
        }
        q.xor_with(&r);
        q
    }

    /// Argon2's variable-length hash H' (`blake2b_long`).
    pub fn blake2b_long(inputs: &[&[u8]], out: &mut [u8]) -> Result<(), ()> {
        use blake2::digest::{Digest, Update, VariableOutput};
        use blake2::{Blake2b512, Blake2bVar};
        if out.is_empty() {
            return Err(());
        }
        let len_bytes = u32::try_from(out.len()).map(|v| v.to_le_bytes()).map_err(|_| ())?;
        if out.len() <= 64 {
            let mut d = Blake2bVar::new(out.len()).map_err(|_| ())?;
            Update::update(&mut d, &len_bytes);
            for input in inputs {
                Update::update(&mut d, input);
            }
            d.finalize_variable(out).map_err(|_| ())?;
            return Ok(());
        }
        let half = 32;
        let mut d = Blake2b512::new();
        Digest::update(&mut d, len_bytes);
        for input in inputs {
            Digest::update(&mut d, input);
        }
        let mut last = d.finalize();
        out[..half].copy_from_slice(&last[..half]);
        let mut counter = 0;
        let out_len = out.len();
        for chunk in out[half..].chunks_exact_mut(half).take_while(|_| {
            counter += half;
            out_len - counter > 64
        }) {
            last = Blake2b512::digest(last);
            chunk.copy_from_slice(&last[..half]);
        }
        let last_size = out.len() - counter;
        let mut d = Blake2bVar::new(last_size).map_err(|_| ())?;
        Update::update(&mut d, &last);
        d.finalize_variable(&mut out[counter..]).map_err(|_| ())?;
        Ok(())
    }

    /// Argon2id parameters exactly as `argon2::Params::new(m, t, p, Some(32))`
    /// accepts them.
    pub fn argon2_params_ok(m_kib: u32, t: u32, p: u32) -> bool {
        argon2::Params::new(m_kib, t, p, Some(32)).is_ok()
    }

    const SYNC_POINTS: usize = 4;
    const ADDRESSES_IN_BLOCK: usize = 128;

    fn segment_length(m_kib: u32, p: u32) -> usize {
        let lanes = p as usize;
        let m = m_kib as usize;
        let memory_blocks = if m < 2 * SYNC_POINTS * lanes { 2 * SYNC_POINTS * lanes } else { m };
        memory_blocks / (lanes * SYNC_POINTS)
    }

    /// Blocks the store must hold (`Params::block_count`).
    pub fn argon2_block_count(m_kib: u32, p: u32) -> usize {
        segment_length(m_kib, p) * p as usize * SYNC_POINTS
    }

    /// Argon2id v0x13 with a 32-byte output into `out`. `mem` must hold
    /// [`argon2_block_count`] blocks. `between` runs after every slice (the
    /// kernel yields there). Err on parameters the crate would reject.
    pub fn argon2id<S: BlockStore>(
        mem: &mut S,
        pwd: &[u8],
        salt: &[u8],
        m_kib: u32,
        t_cost: u32,
        p_cost: u32,
        out: &mut [u8; 32],
        mut between: impl FnMut(),
    ) -> Result<(), ()> {
        use blake2::digest::Digest;
        use blake2::Blake2b512;
        if !argon2_params_ok(m_kib, t_cost, p_cost) {
            return Err(());
        }
        // verify_inputs: MAX_PWD_LEN = u32::MAX, salt 8..=64 bytes.
        if pwd.len() > u32::MAX as usize || salt.len() < 8 || salt.len() > 64 {
            return Err(());
        }
        // initial_hash (no secret, no associated data)
        let mut d = Blake2b512::new();
        Digest::update(&mut d, p_cost.to_le_bytes());
        Digest::update(&mut d, 32u32.to_le_bytes());
        Digest::update(&mut d, m_kib.to_le_bytes());
        Digest::update(&mut d, t_cost.to_le_bytes());
        Digest::update(&mut d, 0x13u32.to_le_bytes()); // Version::V0x13
        Digest::update(&mut d, 2u32.to_le_bytes()); // Algorithm::Argon2id
        Digest::update(&mut d, (pwd.len() as u32).to_le_bytes());
        Digest::update(&mut d, pwd);
        Digest::update(&mut d, (salt.len() as u32).to_le_bytes());
        Digest::update(&mut d, salt);
        Digest::update(&mut d, 0u32.to_le_bytes()); // no secret
        Digest::update(&mut d, 0u32.to_le_bytes()); // no data
        let mut initial_hash = d.finalize();

        let segment_length = segment_length(m_kib, p_cost);
        let lanes = p_cost as usize;
        let lane_length = segment_length * SYNC_POINTS;
        let block_count = lane_length * lanes;
        let iterations = t_cost as usize;

        let mut hash = [0u8; Block::SIZE];
        for l in 0..lanes {
            for i in 0..2usize {
                let inputs: [&[u8]; 3] = [initial_hash.as_ref(), &(i as u32).to_le_bytes(), &(l as u32).to_le_bytes()];
                blake2b_long(&inputs, &mut hash)?;
                mem.blk_mut(l * lane_length + i).load(&hash);
            }
        }
        wipe_bytes(&mut hash);
        for b in initial_hash.iter_mut() {
            // SAFETY: valid exclusive reference.
            unsafe { core::ptr::write_volatile(b, 0) };
        }

        for pass in 0..iterations {
            for slice in 0..SYNC_POINTS {
                let data_independent_addressing = pass == 0 && slice < SYNC_POINTS / 2;
                for lane in 0..lanes {
                    let mut address_block = Block::new();
                    let mut input_block = Block::new();
                    let zero_block = Block::new();
                    if data_independent_addressing {
                        input_block.0[..6].copy_from_slice(&[
                            pass as u64,
                            lane as u64,
                            slice as u64,
                            block_count as u64,
                            iterations as u64,
                            2u64, // Argon2id
                        ]);
                    }
                    let first_block = if pass == 0 && slice == 0 {
                        if data_independent_addressing {
                            update_address_block(&mut address_block, &mut input_block, &zero_block);
                        }
                        2
                    } else {
                        0
                    };
                    let mut cur_index = lane * lane_length + slice * segment_length + first_block;
                    let mut prev_index = if slice == 0 && first_block == 0 { cur_index + lane_length - 1 } else { cur_index - 1 };
                    for block in first_block..segment_length {
                        let rand = if data_independent_addressing {
                            let address_index = block % ADDRESSES_IN_BLOCK;
                            if address_index == 0 {
                                update_address_block(&mut address_block, &mut input_block, &zero_block);
                            }
                            address_block.0[address_index]
                        } else {
                            mem.blk(prev_index).0[0]
                        };
                        let ref_lane = if pass == 0 && slice == 0 { lane } else { (rand >> 32) as usize % lanes };
                        let reference_area_size = if pass == 0 {
                            if slice == 0 {
                                block - 1
                            } else if ref_lane == lane {
                                slice * segment_length + block - 1
                            } else {
                                slice * segment_length - if block == 0 { 1 } else { 0 }
                            }
                        } else if ref_lane == lane {
                            lane_length - segment_length + block - 1
                        } else {
                            lane_length - segment_length - if block == 0 { 1 } else { 0 }
                        };
                        let mut map = rand & 0xFFFF_FFFF;
                        map = (map * map) >> 32;
                        let relative_position = reference_area_size - 1 - ((reference_area_size as u64 * map) >> 32) as usize;
                        let start_position = if pass != 0 && slice != SYNC_POINTS - 1 { (slice + 1) * segment_length } else { 0 };
                        let lane_index = (start_position + relative_position) % lane_length;
                        let ref_index = ref_lane * lane_length + lane_index;
                        let result = compress(mem.blk(prev_index), mem.blk(ref_index));
                        if pass == 0 {
                            *mem.blk_mut(cur_index) = result;
                        } else {
                            mem.blk_mut(cur_index).xor_with(&result);
                        }
                        prev_index = cur_index;
                        cur_index += 1;
                    }
                    address_block.wipe();
                    input_block.wipe();
                }
                between();
            }
        }

        // finalize
        let mut blockhash = *mem.blk(lane_length - 1);
        for l in 1..lanes {
            blockhash.xor_with(mem.blk(l * lane_length + (lane_length - 1)));
        }
        let mut bytes = [0u8; Block::SIZE];
        for (chunk, v) in bytes.chunks_mut(8).zip(blockhash.0.iter()) {
            chunk.copy_from_slice(&v.to_le_bytes());
        }
        let r = blake2b_long(&[&bytes], &mut out[..]);
        blockhash.wipe();
        wipe_bytes(&mut bytes);
        r
    }

    fn update_address_block(address_block: &mut Block, input_block: &mut Block, zero_block: &Block) {
        input_block.0[6] += 1;
        *address_block = compress(zero_block, input_block);
        *address_block = compress(zero_block, address_block);
    }

    // ── QR codes ─────────────────────────────────────────────────────────────
    //
    // The host renders `qr_svg` / `lw_qr` with the `qrcode` crate (std-only).
    // This is a compact standard encoder (ISO/IEC 18004, the algorithm of
    // Project Nayuki's reference encoder): one segment in the densest mode
    // that fits the whole text (numeric / alphanumeric / byte), error
    // correction level M, the smallest version that fits, the lowest-penalty
    // mask. It encodes the same text at the same level; the module pattern can
    // differ from the qrcode crate's where that crate chooses a mixed-mode
    // segmentation or another mask. The SVG markup is the crate's, exactly.

    const ECC_M_CODEWORDS_PER_BLOCK: [u8; 41] = [
        0, 10, 16, 26, 18, 24, 16, 18, 22, 22, 26, 30, 22, 22, 24, 24, 28, 28, 26, 26, 26, 26, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28,
        28, 28, 28, 28, 28, 28, 28, 28,
    ];
    const ECC_M_BLOCKS: [u8; 41] = [
        0, 1, 1, 1, 2, 2, 4, 4, 4, 5, 5, 5, 8, 9, 9, 10, 10, 11, 13, 14, 16, 17, 17, 18, 20, 21, 23, 25, 26, 28, 29, 31, 33, 35, 37, 38, 40,
        43, 45, 47, 49,
    ];
    const ALNUM: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:";

    fn raw_data_modules(ver: usize) -> usize {
        let mut r = (16 * ver + 128) * ver + 64;
        if ver >= 2 {
            let na = ver / 7 + 2;
            r -= (25 * na - 10) * na - 55;
            if ver >= 7 {
                r -= 36;
            }
        }
        r
    }

    fn data_codewords(ver: usize) -> usize {
        raw_data_modules(ver) / 8 - ECC_M_CODEWORDS_PER_BLOCK[ver] as usize * ECC_M_BLOCKS[ver] as usize
    }

    fn gf_mul(x: u8, y: u8) -> u8 {
        let mut z: u32 = 0;
        for i in (0..8).rev() {
            z = (z << 1) ^ ((z >> 7) * 0x11D);
            z ^= ((y as u32 >> i) & 1) * x as u32;
        }
        z as u8
    }

    fn rs_divisor(degree: usize) -> Vec<u8> {
        let mut r = vec![0u8; degree];
        r[degree - 1] = 1;
        let mut root: u8 = 1;
        for _ in 0..degree {
            for j in 0..degree {
                r[j] = gf_mul(r[j], root);
                if j + 1 < degree {
                    r[j] ^= r[j + 1];
                }
            }
            root = gf_mul(root, 0x02);
        }
        r
    }

    fn rs_remainder(data: &[u8], div: &[u8]) -> Vec<u8> {
        let mut r = vec![0u8; div.len()];
        for &b in data {
            let factor = b ^ r[0];
            r.remove(0);
            r.push(0);
            for (i, &c) in div.iter().enumerate() {
                r[i] ^= gf_mul(c, factor);
            }
        }
        r
    }

    struct Bits(Vec<bool>);
    impl Bits {
        fn put(&mut self, val: u32, n: usize) {
            for i in (0..n).rev() {
                self.0.push((val >> i) & 1 != 0);
            }
        }
    }

    struct Qr {
        size: usize,
        m: Vec<bool>,
        f: Vec<bool>,
    }

    impl Qr {
        fn get(&self, x: usize, y: usize) -> bool {
            self.m[y * self.size + x]
        }
        fn setf(&mut self, x: usize, y: usize, dark: bool) {
            self.m[y * self.size + x] = dark;
            self.f[y * self.size + x] = true;
        }
        fn finder(&mut self, cx: i32, cy: i32) {
            for dy in -4..=4i32 {
                for dx in -4..=4i32 {
                    let dist = dx.abs().max(dy.abs());
                    let (xx, yy) = (cx + dx, cy + dy);
                    if xx >= 0 && yy >= 0 && (xx as usize) < self.size && (yy as usize) < self.size {
                        self.setf(xx as usize, yy as usize, dist != 2 && dist != 4);
                    }
                }
            }
        }
        fn align(&mut self, cx: usize, cy: usize) {
            for dy in -2..=2i32 {
                for dx in -2..=2i32 {
                    self.setf((cx as i32 + dx) as usize, (cy as i32 + dy) as usize, dx.abs().max(dy.abs()) != 1);
                }
            }
        }
        fn format_bits(&mut self, mask: u32) {
            let data = (0u32 << 3) | mask; // level M = 0b00
            let mut rem = data;
            for _ in 0..10 {
                rem = (rem << 1) ^ ((rem >> 9) * 0x537);
            }
            let bits = (data << 10 | rem) ^ 0x5412;
            let bit = |i: u32| (bits >> i) & 1 != 0;
            let s = self.size;
            for i in 0..6 {
                self.setf(8, i, bit(i as u32));
            }
            self.setf(8, 7, bit(6));
            self.setf(8, 8, bit(7));
            self.setf(7, 8, bit(8));
            for i in 9..15 {
                self.setf(14 - i, 8, bit(i as u32));
            }
            for i in 0..8 {
                self.setf(s - 1 - i, 8, bit(i as u32));
            }
            for i in 8..15 {
                self.setf(8, s - 15 + i, bit(i as u32));
            }
            self.setf(8, s - 8, true);
        }
        fn version_bits(&mut self, ver: usize) {
            if ver < 7 {
                return;
            }
            let mut rem = ver as u32;
            for _ in 0..12 {
                rem = (rem << 1) ^ ((rem >> 11) * 0x1F25);
            }
            let bits = (ver as u32) << 12 | rem;
            for i in 0..18 {
                let bit = (bits >> i) & 1 != 0;
                let a = self.size - 11 + i % 3;
                let b = i / 3;
                self.setf(a, b, bit);
                self.setf(b, a, bit);
            }
        }
        fn apply_mask(&mut self, mask: u32) {
            for y in 0..self.size {
                for x in 0..self.size {
                    let inv = match mask {
                        0 => (x + y) % 2 == 0,
                        1 => y % 2 == 0,
                        2 => x % 3 == 0,
                        3 => (x + y) % 3 == 0,
                        4 => (x / 3 + y / 2) % 2 == 0,
                        5 => x * y % 2 + x * y % 3 == 0,
                        6 => (x * y % 2 + x * y % 3) % 2 == 0,
                        _ => ((x + y) % 2 + x * y % 3) % 2 == 0,
                    };
                    let i = y * self.size + x;
                    if !self.f[i] && inv {
                        self.m[i] = !self.m[i];
                    }
                }
            }
        }
        fn hist_add(&self, mut run: usize, h: &mut [usize; 7]) {
            if h[0] == 0 {
                run += self.size;
            }
            h.copy_within(0..6, 1);
            h[0] = run;
        }
        fn hist_count(&self, h: &[usize; 7]) -> usize {
            let n = h[1];
            let core = n > 0 && h[2] == n && h[3] == n * 3 && h[4] == n && h[5] == n;
            (core && h[0] >= n * 4 && h[6] >= n) as usize + (core && h[6] >= n * 4 && h[0] >= n) as usize
        }
        fn hist_terminate(&self, color: bool, mut run: usize, h: &mut [usize; 7]) -> usize {
            if color {
                self.hist_add(run, h);
                run = 0;
            }
            run += self.size;
            self.hist_add(run, h);
            self.hist_count(h)
        }
        fn penalty(&self) -> usize {
            let s = self.size;
            let mut result = 0usize;
            for pass in 0..2 {
                for a in 0..s {
                    let mut color = false;
                    let mut run = 0usize;
                    let mut h = [0usize; 7];
                    for b in 0..s {
                        let v = if pass == 0 { self.get(b, a) } else { self.get(a, b) };
                        if v == color {
                            run += 1;
                            if run == 5 {
                                result += 3;
                            } else if run > 5 {
                                result += 1;
                            }
                        } else {
                            self.hist_add(run, &mut h);
                            if !color {
                                result += self.hist_count(&h) * 40;
                            }
                            color = v;
                            run = 1;
                        }
                    }
                    result += self.hist_terminate(color, run, &mut h) * 40;
                }
            }
            for y in 0..s - 1 {
                for x in 0..s - 1 {
                    let c = self.get(x, y);
                    if c == self.get(x + 1, y) && c == self.get(x, y + 1) && c == self.get(x + 1, y + 1) {
                        result += 3;
                    }
                }
            }
            let dark = self.m.iter().filter(|&&d| d).count();
            let total = s * s;
            let k = ((dark * 20).abs_diff(total * 10) + total - 1) / total - 1;
            result + k * 10
        }
    }

    /// Encode `text` as a QR code (level M). Returns (size, modules row-major,
    /// true = dark), or None when it does not fit version 40.
    pub fn qr_encode(text: &str) -> Option<(usize, Vec<bool>)> {
        let data = text.as_bytes();
        let numeric = data.iter().all(|c| c.is_ascii_digit());
        let alnum = data.iter().all(|c| ALNUM.contains(c));
        // (mode indicator, count bits per version range, data bits)
        let (mode, cc): (u32, [usize; 3]) = if numeric {
            (1, [10, 12, 14])
        } else if alnum {
            (2, [9, 11, 13])
        } else {
            (4, [8, 16, 16])
        };
        let payload_bits = if numeric {
            data.len() / 3 * 10 + [0, 4, 7][data.len() % 3]
        } else if alnum {
            data.len() / 2 * 11 + (data.len() % 2) * 6
        } else {
            data.len() * 8
        };
        let mut ver = 0;
        for v in 1..=40usize {
            let ccb = cc[if v <= 9 { 0 } else if v <= 26 { 1 } else { 2 }];
            if data.len() >= (1usize << ccb) {
                continue;
            }
            if 4 + ccb + payload_bits <= data_codewords(v) * 8 {
                ver = v;
                break;
            }
        }
        if ver == 0 {
            return None;
        }
        let ccb = cc[if ver <= 9 { 0 } else if ver <= 26 { 1 } else { 2 }];
        let mut bb = Bits(Vec::new());
        bb.put(mode, 4);
        bb.put(data.len() as u32, ccb);
        if numeric {
            for ch in data.chunks(3) {
                let v = ch.iter().fold(0u32, |a, &c| a * 10 + (c - b'0') as u32);
                bb.put(v, ch.len() * 3 + 1);
            }
        } else if alnum {
            let idx = |c: u8| ALNUM.iter().position(|&a| a == c).unwrap_or(0) as u32;
            for ch in data.chunks(2) {
                if ch.len() == 2 {
                    bb.put(idx(ch[0]) * 45 + idx(ch[1]), 11);
                } else {
                    bb.put(idx(ch[0]), 6);
                }
            }
        } else {
            for &c in data {
                bb.put(c as u32, 8);
            }
        }
        let cap_bits = data_codewords(ver) * 8;
        let term = (cap_bits - bb.0.len()).min(4);
        bb.put(0, term);
        let pad = (8 - bb.0.len() % 8) % 8;
        bb.put(0, pad);
        let mut pb = 0xECu32;
        while bb.0.len() < cap_bits {
            bb.put(pb, 8);
            pb ^= 0xEC ^ 0x11;
        }
        let mut cw = vec![0u8; bb.0.len() / 8];
        for (i, &b) in bb.0.iter().enumerate() {
            if b {
                cw[i >> 3] |= 1 << (7 - (i & 7));
            }
        }
        // error correction + interleave
        let nblocks = ECC_M_BLOCKS[ver] as usize;
        let ecc_len = ECC_M_CODEWORDS_PER_BLOCK[ver] as usize;
        let raw = raw_data_modules(ver) / 8;
        let nshort = nblocks - raw % nblocks;
        let short_len = raw / nblocks;
        let div = rs_divisor(ecc_len);
        let mut blocks: Vec<Vec<u8>> = Vec::new();
        let mut k = 0;
        for i in 0..nblocks {
            let dl = short_len - ecc_len + if i < nshort { 0 } else { 1 };
            let mut dat = cw[k..k + dl].to_vec();
            k += dl;
            let ecc = rs_remainder(&dat, &div);
            if i < nshort {
                dat.push(0);
            }
            dat.extend_from_slice(&ecc);
            blocks.push(dat);
        }
        let mut all = Vec::with_capacity(raw);
        for i in 0..blocks[0].len() {
            for (j, blk) in blocks.iter().enumerate() {
                if i != short_len - ecc_len || j >= nshort {
                    all.push(blk[i]);
                }
            }
        }
        // function patterns
        let size = ver * 4 + 17;
        let mut q = Qr { size, m: vec![false; size * size], f: vec![false; size * size] };
        for i in 0..size {
            q.setf(6, i, i % 2 == 0);
            q.setf(i, 6, i % 2 == 0);
        }
        q.finder(3, 3);
        q.finder(size as i32 - 4, 3);
        q.finder(3, size as i32 - 4);
        if ver > 1 {
            let na = ver / 7 + 2;
            let step = (ver * 8 + na * 3 + 5) / (na * 4 - 4) * 2;
            let mut pos: Vec<usize> = (0..na - 1).map(|i| size - 7 - i * step).collect();
            pos.push(6);
            pos.reverse();
            for i in 0..na {
                for j in 0..na {
                    if (i == 0 && j == 0) || (i == 0 && j == na - 1) || (i == na - 1 && j == 0) {
                        continue;
                    }
                    q.align(pos[i], pos[j]);
                }
            }
        }
        q.format_bits(0);
        q.version_bits(ver);
        // codewords
        let mut i = 0usize;
        let mut right = size as isize - 1;
        while right >= 1 {
            if right == 6 {
                right = 5;
            }
            for vert in 0..size {
                for j in 0..2 {
                    let x = right as usize - j;
                    let upward = (right + 1) & 2 == 0;
                    let y = if upward { size - 1 - vert } else { vert };
                    if !q.f[y * size + x] && i < all.len() * 8 {
                        q.m[y * size + x] = (all[i >> 3] >> (7 - (i & 7))) & 1 != 0;
                        i += 1;
                    }
                }
            }
            right -= 2;
        }
        // mask
        let mut best = 0u32;
        let mut min_pen = usize::MAX;
        for mask in 0..8 {
            q.apply_mask(mask);
            q.format_bits(mask);
            let p = q.penalty();
            if p < min_pen {
                min_pen = p;
                best = mask;
            }
            q.apply_mask(mask);
        }
        q.apply_mask(best);
        q.format_bits(best);
        Some((size, q.m))
    }

    /// The host's `qr_svg`: qrcode's SVG renderer with `min_dimensions(240,
    /// 240)`, dark `#1a0f3d`, light `#ffffff`, a 4-module quiet zone.
    /// "" when the text does not fit a QR code.
    pub fn qr_svg(text: &str) -> String {
        let Some((n, m)) = qr_encode(text) else { return String::new() };
        let qz = 4usize;
        let width = n + 2 * qz;
        let unit = (240 + width - 1) / width;
        let real = width * unit;
        let mut svg = format!(
            concat!(
                r#"<?xml version="1.0" standalone="yes"?>"#,
                r#"<svg xmlns="http://www.w3.org/2000/svg""#,
                r#" version="1.1" width="{w}" height="{h}""#,
                r#" viewBox="0 0 {w} {h}" shape-rendering="crispEdges">"#,
                r#"<rect x="0" y="0" width="{w}" height="{h}" fill="{bg}"/>"#,
                r#"<path fill="{fg}" d=""#,
            ),
            w = real,
            h = real,
            fg = "#1a0f3d",
            bg = "#ffffff"
        );
        for y in 0..n {
            for x in 0..n {
                if m[y * n + x] {
                    let (l, t) = ((x + qz) * unit, (y + qz) * unit);
                    svg.push_str(&format!("M{l} {t}h{unit}v{unit}H{l}V{t}"));
                }
            }
        }
        svg.push_str(r#""/></svg>"#);
        svg
    }

    /// `lw_qr` rows: one "0"/"1" string per module row.
    pub fn qr_rows(text: &str) -> Option<Vec<String>> {
        let (n, m) = qr_encode(text)?;
        Some(m.chunks(n.max(1)).map(|row| row.iter().map(|&d| if d { '1' } else { '0' }).collect()).collect())
    }
}
// ==== PURE END ====

use pure::*;

// ── Ling argument parsing (host bytes.rs helpers over NaN-boxed values) ─────

/// Argument `i` as a string (must be a Ling string, not coerced).
fn str_arg(args: &[u64], i: usize) -> Result<&'static str, String> {
    match args.get(i) {
        Some(&v) if is_str(v) => Ok(str_of(v)),
        Some(_) => Err(format!("argument {} must be a string", i + 1)),
        None => Err(format!("missing argument {}", i + 1)),
    }
}

fn hex_arg(args: &[u64], i: usize) -> Result<Vec<u8>, String> {
    let s = str_arg(args, i)?;
    from_hex(s).ok_or_else(|| format!("argument {} is not valid hex", i + 1))
}

fn hex_arg_n<const N: usize>(args: &[u64], i: usize) -> Result<[u8; N], String> {
    let mut v = hex_arg(args, i)?;
    let out: Result<[u8; N], _> = v.as_slice().try_into();
    let len = v.len();
    wipe_bytes(&mut v);
    out.map_err(|_| format!("argument {} must be {} bytes (got {})", i + 1, N, len))
}

fn hex_list_arg(args: &[u64], i: usize) -> Result<Vec<Vec<u8>>, String> {
    match args.get(i) {
        Some(&l) if is_list(l) => list_items(l)
            .iter()
            .enumerate()
            .map(|(k, &v)| {
                if is_str(v) {
                    from_hex(str_of(v)).ok_or_else(|| format!("list item {k} is not valid hex"))
                } else {
                    Err(format!("list item {k} must be a hex string"))
                }
            })
            .collect(),
        Some(_) => Err(format!("argument {} must be a list", i + 1)),
        None => Err(format!("missing argument {}", i + 1)),
    }
}

fn uint_arg(args: &[u64], i: usize, max: u64) -> Result<u64, String> {
    match args.get(i) {
        Some(&v) if is_num(v) => {
            let n = f64::from_bits(v);
            if !n.is_finite() || n < 0.0 || n - libm::trunc(n) != 0.0 || n >= MAX_SAFE as f64 || n as u64 > max {
                return Err(format!("argument {} must be an integer in 0..={max}", i + 1));
            }
            Ok(n as u64)
        },
        Some(_) => Err(format!("argument {} must be a number", i + 1)),
        None => Err(format!("missing argument {}", i + 1)),
    }
}

fn u256_arg(args: &[u64], i: usize) -> Result<U256, String> {
    match args.get(i) {
        Some(&v) if is_str(v) => {
            U256::parse_decimal(str_of(v)).ok_or_else(|| format!("argument {} is not an unsigned 256-bit decimal", i + 1))
        },
        Some(&v) if is_num(v) => Ok(U256([uint_arg(args, i, MAX_SAFE - 1)?, 0, 0, 0])),
        Some(_) => Err(format!("argument {} must be a decimal string", i + 1)),
        None => Err(format!("missing argument {}", i + 1)),
    }
}

fn s(v: &str) -> u64 {
    mk_str(v)
}

// ── Errors ───────────────────────────────────────────────────────────────────

/// Outcome of a builtin body: `Plain(m)` is a host runtime error (here `()`
/// plus a one-time log line), `Bad(m)` becomes `bad(m)`.
enum Fail {
    Plain(String),
    Bad(String),
}

type R = Result<u64, Fail>;

fn plain<T>(r: Result<T, String>) -> Result<T, Fail> {
    r.map_err(Fail::Plain)
}

fn badr<T>(r: Result<T, String>) -> Result<T, Fail> {
    r.map_err(Fail::Bad)
}

static mut LOGGED: BTreeSet<Vec<u8>> = BTreeSet::new();

/// Where the host raises a runtime error: log it (once per builtin name).
fn runtime_error(name: &[u8], msg: &str) -> u64 {
    let seen = unsafe { &mut *(&raw mut LOGGED) };
    if seen.insert(name.to_vec()) {
        crate::console_write(b"[walletrt] ");
        crate::console_write(name);
        crate::console_write(b": ");
        crate::console_write(msg.as_bytes());
        crate::console_write(b"\n");
    }
    UNIT
}

// ── Randomness ───────────────────────────────────────────────────────────────

/// ling_crypto's entropy source: the hardware RNG. Callers check
/// [`rng_ok`] first, so a failure here means the RNG broke mid-call; never
/// hand back predictable bytes for key material.
fn kernel_entropy(buf: &mut [u8]) {
    for _ in 0..64 {
        if crate::crypto::random_bytes(buf) {
            return;
        }
    }
    panic!("walletrt: hardware random number generator failed");
}

fn rng_ok() -> bool {
    static mut INSTALLED: bool = false;
    let mut probe = [0u8; 8];
    if !crate::crypto::random_bytes(&mut probe) {
        return false;
    }
    unsafe {
        if !INSTALLED {
            INSTALLED = true;
            ling_crypto::rng::set_entropy_source(kernel_entropy);
        }
    }
    true
}

fn random_vec(n: usize) -> Vec<u8> {
    let mut v = vec![0u8; n];
    ling_crypto::rng::fill(&mut v);
    v
}

// ── Per-isolate key state ────────────────────────────────────────────────────

/// A secret held for Ling code; every variant zeroizes itself on drop.
enum LwKey {
    Kem(Box<KemSecret>),
    Id(Box<IdentityKey>),
    Spend(Box<SpendSecret>),
    OneTime(Box<OneTimeSecret>),
}

#[derive(Default)]
struct LwState {
    keys: BTreeMap<String, LwKey>,
    /// HyperTorus hasher with its 16 MiB scratchpad, allocated on first use.
    pow: Option<Box<HyperTorus>>,
}

impl LwState {
    fn insert(&mut self, key: LwKey) -> String {
        loop {
            let h = to_hex(&ling_crypto::rng::random_bytes::<16>());
            if !self.keys.contains_key(&h) {
                self.keys.insert(h.clone(), key);
                return h;
            }
        }
    }
}

/// (isolate, app name for isolate 0 / "" otherwise) -> state.
static mut STATES: BTreeMap<(u64, String), LwState> = BTreeMap::new();

fn states() -> &'static mut BTreeMap<(u64, String), LwState> {
    unsafe { &mut *(&raw mut STATES) }
}

fn state_key() -> (u64, String) {
    let iso = crate::coro::current_isolate();
    if iso == 0 {
        (0, crate::lingapp::active_app_name())
    } else {
        (iso, String::new())
    }
}

fn state() -> &'static mut LwState {
    crate::coro::on_kill(on_kill);
    states().entry(state_key()).or_default()
}

/// Coroutines ended (an app quit): drop (and so zeroize) the key state of
/// every isolate that is gone. An app's main program (isolate 0, keyed by
/// app name) counts as gone once no coroutine it owns is left running; the
/// desktop's own state stays.
fn on_kill() {
    let live = crate::coro::list();
    states().retain(|(iso, app), _| {
        if *iso != 0 {
            crate::coro::alive(*iso)
        } else {
            app == "desktop" || live.iter().any(|c| !c.4 && c.3 == *app)
        }
    });
}

fn key_ref(args: &[u64], i: usize) -> Result<&'static LwKey, String> {
    let h = str_arg(args, i)?;
    state().keys.get(h).ok_or_else(|| "unknown key handle".to_string())
}

// ── Argon2id memory from the frame allocator ─────────────────────────────────

/// Argon2 blocks in power-of-two frame chunks (at most 4 MiB each), wiped
/// and returned on drop.
struct FrameMem {
    chunks: Vec<u64>,
    order: usize,
    shift: u32,
}

impl FrameMem {
    fn new(blocks: usize) -> Option<FrameMem> {
        // A 4 KiB frame holds 4 blocks; chunks of up to 4096 blocks (order 10).
        let mut per = 4usize;
        while per < blocks && per < 4096 {
            per *= 2;
        }
        let order = (per / 4).trailing_zeros() as usize;
        let mut fm = FrameMem { chunks: Vec::new(), order, shift: per.trailing_zeros() };
        let n = blocks.div_ceil(per);
        for _ in 0..n {
            let a = crate::mm::frame::alloc_frames(order)?;
            fm.chunks.push(a);
        }
        Some(fm)
    }
}

impl BlockStore for FrameMem {
    fn blk(&self, i: usize) -> &Block {
        let c = self.chunks[i >> self.shift] as *const Block;
        unsafe { &*c.add(i & ((1 << self.shift) - 1)) }
    }
    fn blk_mut(&mut self, i: usize) -> &mut Block {
        let c = self.chunks[i >> self.shift] as *mut Block;
        unsafe { &mut *c.add(i & ((1 << self.shift) - 1)) }
    }
}

impl Drop for FrameMem {
    fn drop(&mut self) {
        let per = 1usize << self.shift;
        for &c in &self.chunks {
            let p = c as *mut Block;
            for k in 0..per {
                unsafe { (*p.add(k)).wipe() };
            }
            crate::mm::frame::free_frames(c, self.order);
        }
    }
}

/// Is a heap block of `bytes` obtainable right now? The heap's large path
/// takes a power-of-two run of frames (size + its 8-byte header) and a failed
/// allocation aborts the kernel, so ask the frame allocator first (nothing
/// yields in between).
fn heap_room(bytes: usize) -> bool {
    let frames = (bytes + 8).div_ceil(4096);
    let order = frames.next_power_of_two().trailing_zeros() as usize;
    match crate::mm::frame::alloc_frames(order) {
        Some(a) => {
            crate::mm::frame::free_frames(a, order);
            true
        },
        None => false,
    }
}

// ── Dispatch ─────────────────────────────────────────────────────────────────

/// Registered with the value runtime by `lingrt::ensure_init`.
pub fn dispatch(name: &[u8], args: &[u64]) -> Option<u64> {
    if let Some(v) = bytes_builtin(name, args) {
        return Some(v);
    }
    if let Some(v) = misc_builtin(name, args) {
        return Some(v);
    }
    let r = match name {
        b"lw_h256" => lw_h256(args),
        b"lw_xof" => lw_xof(args),
        b"lw_random" => lw_random(args),
        b"lw_kem_keypair" | b"lw_kem_public" | b"lw_kem_decaps" | b"lw_id_keypair" | b"lw_id_public" | b"lw_id_sign"
        | b"lw_spend_keypair" | b"lw_spend_public" | b"lw_onetime_secret" | b"lw_onetime_key_image"
        | b"lw_onetime_public_of" | b"lw_ring_sign" => lw_keyed(name, args),
        b"lw_key_drop" => Ok(boolean(match args.first() {
            Some(&h) if is_str(h) => state().keys.remove(str_of(h)).is_some(),
            _ => false,
        })),
        b"lw_kem_encaps" => lw_kem_encaps(args),
        b"lw_id_verify" => Ok(boolean(lw_id_verify(args).unwrap_or(false))),
        b"lw_id_fingerprint" => plain(hex_arg(args, 0)).and_then(|pk| {
            let pk = plain(IdentityPublic::from_bytes(&pk).map_err(|_| "public key must be 2592 bytes".to_string()))?;
            Ok(s(&pk.fingerprint()))
        }),
        b"lw_onetime_public" => lw_onetime_public(args),
        b"lw_ring_verify" => Ok(boolean(lw_ring_verify(args).unwrap_or(false))),
        b"lw_pk_valid" => Ok(boolean(hex_arg(args, 0).ok().is_some_and(|b| PublicKey::from_bytes(&b).is_ok()))),
        b"lw_aead_seal" => lw_aead_seal(args),
        b"lw_aead_open" => lw_aead_open(args),
        b"lw_password_key" => lw_password_key(args),
        b"lw_mnemonic_from" => lw_mnemonic_from(args),
        b"lw_mnemonic_to" => lw_mnemonic_to(args),
        b"lw_pow_hash" | b"lw_pow_trace" => lw_pow(name, args),
        b"lw_meets_difficulty" => lw_meets_difficulty(args),
        b"lw_qr" => lw_qr(args),
        _ => return None,
    };
    Some(match r {
        Ok(v) => v,
        Err(Fail::Plain(m)) => runtime_error(name, &m),
        Err(Fail::Bad(m)) => mk_bad_str(&m),
    })
}

// ── bytes.rs ─────────────────────────────────────────────────────────────────

fn bytes_builtin(name: &[u8], args: &[u64]) -> Option<u64> {
    Some(match name {
        b"hex_concat" | b"hex_len" | b"hex_slice" | b"hex_eq" | b"u8_hex" | b"u32le_hex" | b"u64le_hex" | b"varint_hex"
        | b"text_hex" | b"base32_encode" | b"base64url_encode" | b"big_add" | b"big_sub" | b"big_mul" | b"big_div"
        | b"big_cmp" => match bytes_plain(name, args) {
            Ok(v) => v,
            Err(e) => runtime_error(name, &e),
        },
        b"hex_u8" | b"hex_u32le" | b"hex_u64le" | b"hex_varint" | b"hex_text" | b"base32_decode" | b"base64url_decode" => {
            match bytes_fallible(name, args) {
                Ok(v) => mk_ok(v),
                Err(e) => mk_bad_str(&e),
            }
        },
        b"file_read_hex" | b"file_write_hex" => file_hex(name, args),
        // Best-effort no-op, as on the host (builtins get copies).
        b"wipe" => UNIT,
        b"sys_idle_secs" => num(crate::startup::idle_secs()),
        // No battery to ask on LingOS hardware; the host's fallback.
        b"sys_on_battery" => boolean(false),
        _ => return None,
    })
}

fn bytes_plain(name: &[u8], args: &[u64]) -> Result<u64, String> {
    Ok(match name {
        b"hex_concat" => {
            let parts = hex_list_arg(args, 0)?;
            s(&to_hex(&parts.concat()))
        },
        b"hex_len" => num(hex_arg(args, 0)?.len() as f64),
        b"hex_slice" => {
            let data = hex_arg(args, 0)?;
            let start = uint_arg(args, 1, MAX_SAFE - 1)? as usize;
            let len = uint_arg(args, 2, MAX_SAFE - 1)? as usize;
            let end = start.checked_add(len).filter(|&e| e <= data.len()).ok_or_else(|| {
                format!("range {start}..{} out of bounds (length {})", start.saturating_add(len), data.len())
            })?;
            s(&to_hex(&data[start..end]))
        },
        b"hex_eq" => {
            let a = hex_arg(args, 0)?;
            let b = hex_arg(args, 1)?;
            boolean(ct_eq(&a, &b))
        },
        b"u8_hex" => s(&to_hex(&[uint_arg(args, 0, 0xff)? as u8])),
        b"u32le_hex" => s(&to_hex(&(uint_arg(args, 0, u32::MAX as u64)? as u32).to_le_bytes())),
        b"u64le_hex" => s(&to_hex(&uint_arg(args, 0, MAX_SAFE - 1)?.to_le_bytes())),
        b"varint_hex" => s(&to_hex(&write_varint(uint_arg(args, 0, MAX_SAFE - 1)?))),
        b"text_hex" => s(&to_hex(str_arg(args, 0)?.as_bytes())),
        b"base32_encode" => s(&base32_encode(&hex_arg(args, 0)?)),
        b"base64url_encode" => s(&base64url_encode(&hex_arg(args, 0)?)),
        b"big_add" | b"big_sub" | b"big_mul" | b"big_div" | b"big_cmp" => {
            let a = u256_arg(args, 0)?;
            let b = u256_arg(args, 1)?;
            let r = match name {
                b"big_add" => a.checked_add(&b).ok_or("overflow (result ≥ 2^256)")?,
                b"big_sub" => a.checked_sub(&b).ok_or("underflow (b > a)")?,
                b"big_mul" => a.checked_mul(&b).ok_or("overflow (result ≥ 2^256)")?,
                b"big_div" => a.checked_div(&b).ok_or("division by zero")?,
                _ => {
                    return Ok(num(match a.cmp_u(&b) {
                        core::cmp::Ordering::Less => -1.0,
                        core::cmp::Ordering::Equal => 0.0,
                        core::cmp::Ordering::Greater => 1.0,
                    }))
                },
            };
            s(&r.to_decimal())
        },
        _ => return Err("unknown builtin".into()),
    })
}

fn bytes_fallible(name: &[u8], args: &[u64]) -> Result<u64, String> {
    Ok(match name {
        b"hex_u8" | b"hex_u32le" | b"hex_u64le" => {
            let data = hex_arg(args, 0)?;
            let pos = uint_arg(args, 1, MAX_SAFE - 1)?;
            let width = match name {
                b"hex_u8" => 1,
                b"hex_u32le" => 4,
                _ => 8,
            };
            let n = read_le(&data, pos, width).ok_or("read past end of data")?;
            if n >= MAX_SAFE {
                return Err("u64 value ≥ 2^53 is not exactly representable".into());
            }
            num(n as f64)
        },
        b"hex_varint" => {
            let data = hex_arg(args, 0)?;
            let pos = uint_arg(args, 1, MAX_SAFE - 1)? as usize;
            let (v, np) = read_varint(&data, pos)?;
            list_from(vec![num(v as f64), num(np as f64)])
        },
        b"hex_text" => {
            let data = hex_arg(args, 0)?;
            s(&String::from_utf8(data).map_err(|_| "invalid UTF-8".to_string())?)
        },
        b"base32_decode" => s(&to_hex(&base32_decode(str_arg(args, 0)?).ok_or("invalid base32")?)),
        b"base64url_decode" => s(&to_hex(&base64url_decode(str_arg(args, 0)?).ok_or("invalid base64url")?)),
        _ => return Err("unknown builtin".into()),
    })
}

/// `file_read_hex(path)` -> ok(hex) | bad(..); `file_write_hex(path, hex)` ->
/// ok(()) | bad(..), through the kernel's vfs (app-relative paths resolve in
/// the app's data folder). The host's error text is `{op} failed: {kind}`.
fn file_hex(name: &[u8], args: &[u64]) -> u64 {
    let path = match str_arg(args, 0) {
        Ok(p) => p.replace('\\', "/"),
        Err(e) => return runtime_error(name, &e),
    };
    if name == b"file_read_hex" {
        return match crate::vfs::read(&path) {
            Some(mut data) => {
                let h = to_hex(&data);
                wipe_bytes(&mut data);
                mk_ok(s(&h))
            },
            None => mk_bad_str(if crate::vfs::exists(&path) {
                "read failed: permission denied"
            } else {
                "read failed: entity not found"
            }),
        };
    }
    let mut data = match hex_arg(args, 1) {
        Ok(d) => d,
        Err(e) => return mk_bad_str(&e),
    };
    let ok = crate::vfs::write(&path, &data);
    wipe_bytes(&mut data);
    if ok {
        crate::lingrec::ok_unit()
    } else {
        mk_bad_str("write failed: other error")
    }
}

// ── lwcrypto.rs ──────────────────────────────────────────────────────────────

/// Largest output of `lw_random` / `lw_xof` (bytes).
const MAX_RANDOM: u64 = 1 << 20;
const MAX_ARGON_M_KIB: u64 = 4 * 1024 * 1024;
const MAX_ARGON_T: u64 = 1024;
const MAX_ARGON_P: u64 = 64;
const NO_RNG: &str = "no hardware random number generator";

fn lw_keyed(name: &[u8], args: &[u64]) -> R {
    match name {
        b"lw_kem_keypair" => {
            let seed = Zeroizing::new(badr(hex_arg_n::<{ kem1024::SEED_LEN }>(args, 0))?);
            let k = KemSecret::from_seed(&seed);
            Ok(mk_ok(s(&state().insert(LwKey::Kem(Box::new(k))))))
        },
        b"lw_id_keypair" => {
            let seed = Zeroizing::new(badr(hex_arg_n::<32>(args, 0))?);
            let k = IdentityKey::from_seed(&seed);
            Ok(mk_ok(s(&state().insert(LwKey::Id(Box::new(k))))))
        },
        b"lw_spend_keypair" => {
            let seed = Zeroizing::new(badr(hex_arg_n::<32>(args, 0))?);
            let k = SpendSecret::from_seed(&seed);
            Ok(mk_ok(s(&state().insert(LwKey::Spend(Box::new(k))))))
        },
        b"lw_kem_public" => match plain(key_ref(args, 0))? {
            LwKey::Kem(k) => Ok(s(&to_hex(k.public().as_bytes()))),
            _ => Err(Fail::Plain("handle is not a KEM key".into())),
        },
        b"lw_id_public" => match plain(key_ref(args, 0))? {
            LwKey::Id(k) => Ok(s(&to_hex(&k.public().0))),
            _ => Err(Fail::Plain("handle is not an identity key".into())),
        },
        b"lw_spend_public" => match plain(key_ref(args, 0))? {
            LwKey::Spend(k) => Ok(s(&to_hex(&k.public().0))),
            _ => Err(Fail::Plain("handle is not a spend key".into())),
        },
        b"lw_onetime_key_image" | b"lw_onetime_public_of" => match plain(key_ref(args, 0))? {
            LwKey::OneTime(k) => {
                let p = if name == b"lw_onetime_key_image" { k.key_image().0 } else { k.public().0 };
                Ok(s(&to_hex(&p)))
            },
            _ => Err(Fail::Plain("handle is not a one-time key".into())),
        },
        b"lw_id_sign" => {
            let context = plain(str_arg(args, 1))?;
            let msg = plain(hex_arg(args, 2))?;
            match plain(key_ref(args, 0))? {
                LwKey::Id(k) => Ok(s(&to_hex(&k.sign(context, &msg).0))),
                _ => Err(Fail::Plain("handle is not an identity key".into())),
            }
        },
        b"lw_kem_decaps" => {
            let mut ct = badr(hex_arg(args, 1))?;
            let context = badr(hex_arg(args, 2))?;
            let ct_ok = KemCiphertext::from_bytes(&ct);
            wipe_bytes(&mut ct);
            let ct = ct_ok.map_err(|_| Fail::Bad("ciphertext must be 1600 bytes".into()))?;
            match badr(key_ref(args, 0))? {
                LwKey::Kem(k) => {
                    let ss = Zeroizing::new(
                        k.decapsulate(&ct, &context).map_err(|_| Fail::Bad("malformed ciphertext".into()))?,
                    );
                    Ok(mk_ok(s(&to_hex(&ss[..]))))
                },
                _ => Err(Fail::Bad("handle is not a KEM key".into())),
            }
        },
        b"lw_onetime_secret" => {
            let ss = Zeroizing::new(badr(hex_arg_n::<32>(args, 1))?);
            let index = badr(uint_arg(args, 2, u32::MAX as u64))? as u32;
            let ot = match badr(key_ref(args, 0))? {
                LwKey::Spend(k) => lrs::derive_onetime_secret(k, &ss, index),
                _ => return Err(Fail::Bad("handle is not a spend key".into())),
            };
            Ok(mk_ok(s(&state().insert(LwKey::OneTime(Box::new(ot))))))
        },
        b"lw_ring_sign" => {
            let msg = badr(hex_arg_n::<32>(args, 0))?;
            let ring = badr(parse_ring(args, 1))?;
            let signer = badr(uint_arg(args, 2, lrs::MAX_RING_LEN as u64))? as usize;
            match badr(key_ref(args, 3))? {
                LwKey::OneTime(k) => {
                    if !rng_ok() {
                        return Err(Fail::Bad(NO_RNG.into()));
                    }
                    let sig = lrs::try_sign(&msg, &ring, signer, k).map_err(|_| {
                        Fail::Bad(
                            "cannot sign: ring size must be 1..=64, signer index in range, and the \
                             one-time key must match ring[signer]"
                                .into(),
                        )
                    })?;
                    Ok(mk_ok(s(&to_hex(&sig.to_bytes()))))
                },
                _ => Err(Fail::Bad("handle is not a one-time key".into())),
            }
        },
        _ => Err(Fail::Plain("unknown builtin".into())),
    }
}

fn lw_pow(name: &[u8], args: &[u64]) -> R {
    let header = plain(hex_arg(args, 0))?;
    let st = state();
    if st.pow.is_none() {
        // 16 MiB scratchpad: a failed heap allocation would abort the kernel.
        if !heap_room(hypertorus::SCRATCH_WORDS * 8) {
            return Err(Fail::Plain("not enough memory for the 16 MiB HyperTorus scratchpad".into()));
        }
        st.pow = Some(Box::new(HyperTorus::new()));
    }
    let ht = st.pow.as_mut().ok_or_else(|| Fail::Plain("no scratchpad".into()))?;
    if name == b"lw_pow_hash" {
        return Ok(s(&to_hex(&ht.hash(&header))));
    }
    let every = plain(uint_arg(args, 1, MAX_SAFE - 1))? as usize;
    let mut trace: Vec<TracePoint> = Vec::new();
    let h = ht.hash_traced(&header, every, &mut trace);
    let pts = trace
        .iter()
        .map(|t| {
            list_from(vec![num(t.p[0] as f64), num(t.p[1] as f64), num(t.p[2] as f64), num(t.p[3] as f64), num(t.twist as f64)])
        })
        .collect();
    Ok(list_from(vec![s(&to_hex(&h)), list_from(pts)]))
}

/// Ring members: a list of hex public keys, each canonical (2944 B, < q).
fn parse_ring(args: &[u64], i: usize) -> Result<Vec<PublicKey>, String> {
    let raw = hex_list_arg(args, i)?;
    if raw.is_empty() || raw.len() > lrs::MAX_RING_LEN {
        return Err(format!("ring must have 1..={} members", lrs::MAX_RING_LEN));
    }
    raw.iter()
        .enumerate()
        .map(|(k, b)| PublicKey::from_bytes(b).map_err(|_| format!("ring member {k} is not a valid public key")))
        .collect()
}

fn domain_parts(args: &[u64]) -> Result<(&'static str, Vec<Vec<u8>>), String> {
    Ok((str_arg(args, 0)?, hex_list_arg(args, 1)?))
}

fn lw_h256(args: &[u64]) -> R {
    let (domain, parts) = plain(domain_parts(args))?;
    let refs: Vec<&[u8]> = parts.iter().map(|p| p.as_slice()).collect();
    Ok(s(&to_hex(&hash::h256(domain, &refs))))
}

fn lw_xof(args: &[u64]) -> R {
    let (domain, parts) = plain(domain_parts(args))?;
    let len = plain(uint_arg(args, 2, MAX_RANDOM))? as usize;
    let refs: Vec<&[u8]> = parts.iter().map(|p| p.as_slice()).collect();
    Ok(s(&to_hex(&hash::xof_vec(domain, &refs, len))))
}

fn lw_random(args: &[u64]) -> R {
    let n = plain(uint_arg(args, 0, MAX_RANDOM))? as usize;
    if !rng_ok() {
        return Err(Fail::Plain(NO_RNG.into()));
    }
    let mut buf = random_vec(n);
    let h = to_hex(&buf);
    wipe_bytes(&mut buf);
    Ok(s(&h))
}

fn lw_kem_encaps(args: &[u64]) -> R {
    let pk = badr(hex_arg(args, 0))?;
    let context = badr(hex_arg(args, 1))?;
    let pk = KemPublic::from_bytes(&pk).map_err(|_| Fail::Bad("invalid KEM public key".into()))?;
    if !rng_ok() {
        return Err(Fail::Bad(NO_RNG.into()));
    }
    let (ct, ss) = kem1024::try_encapsulate(&pk, &context).map_err(|_| Fail::Bad("invalid KEM public key".into()))?;
    let ss = Zeroizing::new(ss);
    Ok(mk_ok(list_from(vec![s(&to_hex(ct.as_bytes())), s(&to_hex(&ss[..]))])))
}

fn lw_id_verify(args: &[u64]) -> Option<bool> {
    let pk = IdentityPublic::from_bytes(&hex_arg(args, 0).ok()?).ok()?;
    let context = str_arg(args, 1).ok()?;
    let msg = hex_arg(args, 2).ok()?;
    let sig = IdentitySignature::from_bytes(&hex_arg(args, 3).ok()?).ok()?;
    Some(pk.verify(context, &msg, &sig))
}

fn lw_onetime_public(args: &[u64]) -> R {
    let spk = badr(hex_arg(args, 0))?;
    let ss = Zeroizing::new(badr(hex_arg_n::<32>(args, 1))?);
    let index = badr(uint_arg(args, 2, u32::MAX as u64))? as u32;
    let spk = PublicKey::from_bytes(&spk).map_err(|_| Fail::Bad("invalid spend public key".into()))?;
    let t = lrs::try_derive_onetime_public(&spk, &ss, index).map_err(|_| Fail::Bad("invalid spend public key".into()))?;
    Ok(mk_ok(s(&to_hex(&t.0))))
}

fn lw_ring_verify(args: &[u64]) -> Option<bool> {
    let msg = hex_arg_n::<32>(args, 0).ok()?;
    let ring = parse_ring(args, 1).ok()?;
    let ki = KeyImage::from_bytes(&hex_arg(args, 2).ok()?).ok()?;
    let sig = RingSignature::from_bytes(&hex_arg(args, 3).ok()?, ring.len()).ok()?;
    Some(lrs::verify(&msg, &ring, &ki, &sig))
}

fn lw_aead_seal(args: &[u64]) -> R {
    let key = Zeroizing::new(plain(hex_arg_n::<32>(args, 0))?);
    let nonce = plain(hex_arg_n::<24>(args, 1))?;
    let aad = plain(hex_arg(args, 2))?;
    let mut pt = plain(hex_arg(args, 3))?;
    let ct = aead::seal(&key, &nonce, &aad, &pt);
    wipe_bytes(&mut pt);
    Ok(s(&to_hex(&ct)))
}

fn lw_aead_open(args: &[u64]) -> R {
    let key = Zeroizing::new(badr(hex_arg_n::<32>(args, 0))?);
    let nonce = badr(hex_arg_n::<24>(args, 1))?;
    let aad = badr(hex_arg(args, 2))?;
    let ct = badr(hex_arg(args, 3))?;
    let mut pt = aead::open(&key, &nonce, &aad, &ct).map_err(|_| Fail::Bad("authentication failed".into()))?;
    let h = to_hex(&pt);
    wipe_bytes(&mut pt);
    Ok(mk_ok(s(&h)))
}

/// `ling_crypto::lw::kdf::password_key` (Argon2id v0x13, 32-byte key) with
/// its memory in frame chunks: the wallet's default is 256 MiB, which neither
/// the heap nor the frame allocator can give in one piece.
fn lw_password_key(args: &[u64]) -> R {
    let password = badr(str_arg(args, 0))?;
    let salt = badr(hex_arg_n::<16>(args, 1))?;
    let m = badr(uint_arg(args, 2, MAX_ARGON_M_KIB))? as u32;
    let t = badr(uint_arg(args, 3, MAX_ARGON_T))? as u32;
    let p = badr(uint_arg(args, 4, MAX_ARGON_P))? as u32;
    if !argon2_params_ok(m, t, p) {
        return Err(Fail::Bad("invalid Argon2id parameters".into()));
    }
    let mut mem = FrameMem::new(argon2_block_count(m, p))
        .ok_or_else(|| Fail::Bad("not enough memory for these Argon2id parameters".into()))?;
    let mut key = Zeroizing::new([0u8; 32]);
    let yield_ok = crate::coro::in_coro();
    argon2id(&mut mem, password.as_bytes(), &salt, m, t, p, &mut key, || {
        // Let the rest of the machine breathe during a multi-second hash.
        if yield_ok {
            crate::coro::yield_now();
        }
    })
    .map_err(|_| Fail::Bad("invalid Argon2id parameters".into()))?;
    drop(mem);
    Ok(mk_ok(s(&to_hex(&key[..]))))
}

fn lw_mnemonic_from(args: &[u64]) -> R {
    let entropy = Zeroizing::new(plain(hex_arg_n::<32>(args, 0))?);
    let m = bip39::Mnemonic::from_entropy_in(bip39::Language::English, &entropy[..])
        .map_err(|_| Fail::Plain("invalid entropy".into()))?;
    Ok(list_from(m.words().map(s).collect()))
}

fn lw_mnemonic_to(args: &[u64]) -> R {
    let phrase = badr(str_arg(args, 0))?;
    let words: Vec<String> = phrase.split_whitespace().map(|w| w.to_lowercase()).collect();
    if words.len() != 24 {
        return Err(Fail::Bad("a recovery phrase has exactly 24 words".into()));
    }
    let normalized = Zeroizing::new(words.join(" "));
    let m = bip39::Mnemonic::parse_in_normalized(bip39::Language::English, &normalized).map_err(|e| {
        Fail::Bad(match e {
            bip39::Error::UnknownWord(i) => format!("word {} is not in the BIP-39 English list", i + 1),
            bip39::Error::InvalidChecksum => "checksum mismatch (check each word and the order)".into(),
            _ => "invalid recovery phrase".into(),
        })
    })?;
    let mut entropy = m.to_entropy();
    let h = to_hex(&entropy);
    wipe_bytes(&mut entropy);
    if h.len() != 64 {
        return Err(Fail::Bad("a recovery phrase has exactly 24 words".into()));
    }
    Ok(mk_ok(s(&h)))
}

fn lw_meets_difficulty(args: &[u64]) -> R {
    let hash = plain(hex_arg_n::<32>(args, 0))?;
    let d = match args.get(1) {
        Some(&v) if is_str(v) => U256::parse_decimal(str_of(v)),
        Some(&v) if is_num(v) => uint_arg(args, 1, MAX_SAFE - 1).ok().map(|n| U256([n, 0, 0, 0])),
        _ => None,
    }
    .filter(|d| d.0[2] == 0 && d.0[3] == 0)
    .ok_or_else(|| Fail::Plain("difficulty must be an unsigned 128-bit decimal".into()))?;
    let d = (d.0[1] as u128) << 64 | d.0[0] as u128;
    Ok(boolean(hypertorus::meets_difficulty(&hash, d)))
}

fn lw_qr(args: &[u64]) -> R {
    let text = plain(str_arg(args, 0))?;
    let rows = qr_rows(text).ok_or_else(|| Fail::Plain("text too long for a QR code".into()))?;
    Ok(list_from(rows.iter().map(|r| s(r)).collect()))
}

// ── General builtins the wallet uses that the kernel lacked ──────────────────

fn arg_string(args: &[u64], i: usize, d: &str) -> String {
    match args.get(i) {
        Some(&v) => to_string(v),
        None => String::from(d),
    }
}

/// Wall-clock seconds since the Unix epoch with sub-second resolution: the
/// RTC's second at first use, carried forward by the microsecond timer.
fn epoch_secs() -> f64 {
    static mut BASE: Option<(u64, u64)> = None;
    let now_us = crate::arch::timer::now_us();
    let (wall, at_us) = unsafe {
        match BASE {
            Some(b) => b,
            None => {
                let b = (crate::lingapp::wall_time(), now_us);
                BASE = Some(b);
                b
            },
        }
    };
    wall as f64 + now_us.saturating_sub(at_us) as f64 / 1_000_000.0
}

/// The host's `format(fmt, args..)`: `{}` takes the next argument's display,
/// `{:.N}` / `{:.Nf}` a number with N decimals; other `{..}` act like `{}`.
fn format_builtin(args: &[u64]) -> String {
    let Some(&first) = args.first() else { return String::new() };
    if !is_str(first) {
        return to_string(first);
    }
    let fmt = str_of(first);
    let mut result = String::new();
    let mut arg_idx = 1usize;
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            if chars.peek() == Some(&'}') {
                chars.next();
                if arg_idx < args.len() {
                    crate::lingrt::fmt_into(&mut result, args[arg_idx]);
                    arg_idx += 1;
                }
            } else {
                let mut spec = String::new();
                for ch in chars.by_ref() {
                    if ch == '}' {
                        break;
                    }
                    spec.push(ch);
                }
                if arg_idx < args.len() {
                    if let Some(suffix) = spec.strip_prefix(":.") {
                        if is_num(args[arg_idx]) {
                            let n = f64::from_bits(args[arg_idx]);
                            let prec: usize = suffix.trim_end_matches('f').parse().unwrap_or(2);
                            result.push_str(&format!("{:.prec$}", n));
                            arg_idx += 1;
                            continue;
                        }
                    }
                    crate::lingrt::fmt_into(&mut result, args[arg_idx]);
                    arg_idx += 1;
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}

/// `cli_arg(name, default)`: the value after `--name` in the program's
/// command line (`get_args()`), else `default`.
fn cli_arg(args: &[u64]) -> u64 {
    let name = arg_string(args, 0, "");
    let default = arg_string(args, 1, "");
    let flag = format!("--{name}");
    let argv = unsafe { crate::lingrt::ling_builtin(b"get_args".as_ptr(), 8, core::ptr::null(), 0) };
    let items = list_items(argv);
    let found = items
        .iter()
        .position(|&a| to_string(a) == flag)
        .and_then(|i| items.get(i + 1))
        .map(|&v| to_string(v))
        .unwrap_or(default);
    s(&found)
}

fn misc_builtin(name: &[u8], args: &[u64]) -> Option<u64> {
    Some(match name {
        b"format" => s(&format_builtin(args)),
        b"str_repeat" => {
            let t = arg_string(args, 0, "");
            let n = args.get(1).map(|&v| to_f64(v)).unwrap_or(1.0);
            let n = if n.is_finite() && n > 0.0 { n as usize } else { 0 };
            if t.len().saturating_mul(n) > 64 << 20 {
                return Some(runtime_error(name, "result too large"));
            }
            s(&t.repeat(n))
        },
        b"starts_with" => boolean(arg_string(args, 0, "").starts_with(arg_string(args, 1, "").as_str())),
        b"to_str" => s(&to_string(args.first().copied().unwrap_or(UNIT))),
        b"list_join" => {
            let sep = args.get(1).map(|&v| to_string(v)).unwrap_or_default();
            match args.first() {
                Some(&l) if is_list(l) => {
                    let parts: Vec<String> = list_items(l).iter().map(|&x| to_string(x)).collect();
                    s(&parts.join(&sep))
                },
                _ => s(""),
            }
        },
        b"hex_to_number" => {
            let t = arg_string(args, 0, "");
            num(u64::from_str_radix(t.trim(), 16).unwrap_or(0) as f64)
        },
        b"epoch_now" | b"now_unix" => num(epoch_secs()),
        b"make_dir" => boolean(crate::vfs::mkdir(&arg_string(args, 0, "").replace('\\', "/"))),
        b"remove_file" => {
            let p = arg_string(args, 0, "").replace('\\', "/");
            boolean(crate::vfs::exists(&p) && !crate::vfs::is_dir(&p) && crate::vfs::delete(&p))
        },
        b"rename_file" => boolean(crate::vfs::rename(
            &arg_string(args, 0, "").replace('\\', "/"),
            &arg_string(args, 1, "").replace('\\', "/"),
        )),
        // No process environment on LingOS: always the default.
        // LingOS has no environment; the folders apps ask it for are the user's home
        b"env_get" => match arg_string(args, 0, "").as_str() {
            "APPDATA" | "LOCALAPPDATA" | "XDG_DATA_HOME" | "XDG_CONFIG_HOME" => s(&alloc::format!("{}/Programs", crate::vfs::home())),
            "HOME" | "USERPROFILE" => s(&crate::vfs::home()),
            _ => s(&arg_string(args, 1, "")),
        },
        b"cli_arg" => cli_arg(args),
        b"set_fps" => UNIT,
        b"daw_clipboard_get" => s(&String::from_utf8_lossy(crate::drivers::clipboard::get())),
        b"daw_clipboard_set" => {
            crate::drivers::clipboard::set_str(&arg_string(args, 0, ""));
            boolean(true)
        },
        b"qr_svg" => s(&qr_svg(&arg_string(args, 0, ""))),
        _ => return None,
    })
}
