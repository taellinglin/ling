//! HyperTorus proof-of-work [novel] — ling-wallet DESIGN §6 is normative.
//! Ported byte-for-byte from the wallet reference crate `lw-pow`; the KAT
//! vectors in the tests below are consensus pins (changing them is a hard fork).
//!
//! A walk of `STEPS` dependent read-modify-write steps over a 16 MiB
//! scratchpad. The walker lives on the 4-torus `(Z/2^32)^4`; each step projects
//! its point onto one of four (scaled) regular-tetrahedron vertex vectors to
//! choose the scratchpad address, and seam crossings apply a Möbius-style
//! reflection. Everything is integer-only, wrapping and explicitly
//! little-endian, so results are identical on every platform.

use alloc::{vec, vec::Vec};

use crate::lw::hash::{h256, xof};

pub const SCRATCH_WORDS: usize = 1 << 21; // 16 MiB of u64
pub const STEPS: usize = 1 << 18;

const MASK: u64 = (SCRATCH_WORDS as u64) - 1;

/// Per-coordinate scale constants (distinct large odd 64-bit values).
const C: [u64; 4] = [
    0x9E37_79B9_7F4A_7C15,
    0xBF58_476D_1CE4_E5B9,
    0x94D0_49BB_1331_11EB,
    0xD6E8_FEB8_6659_FD93,
];

const fn neg(c: u64) -> u64 {
    c.wrapping_neg()
}

/// `TETRA[k][i] = sign_k[i] · C[i] mod 2^64` for the vertices
/// `(1,1,1,−1)`, `(1,−1,−1,1)`, `(−1,1,−1,1)`, `(−1,−1,1,1)`. Normative.
const TETRA: [[u64; 4]; 4] = [
    [C[0], C[1], C[2], neg(C[3])],
    [C[0], neg(C[1]), neg(C[2]), C[3]],
    [neg(C[0]), C[1], neg(C[2]), C[3]],
    [neg(C[0]), neg(C[1]), C[2], C[3]],
];

/// Byte chunk used when converting the blake3 XOF stream into words.
const FILL_CHUNK: usize = 64 * 1024;

/// A reusable hasher; owns the 16 MiB scratchpad so mining threads don't
/// reallocate per attempt.
pub struct HyperTorus {
    scratch: Vec<u64>,
    buf: Vec<u8>,
}

/// One sampled point of the walk, for the wallet's mining visualization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TracePoint {
    pub p: [u32; 4],
    pub twist: u32,
}

impl HyperTorus {
    pub fn new() -> Self {
        HyperTorus { scratch: vec![0u64; SCRATCH_WORDS], buf: vec![0u8; FILL_CHUNK] }
    }

    /// `pow_hash(input)` per DESIGN §6.
    pub fn hash(&mut self, input: &[u8]) -> [u8; 32] {
        self.run::<false>(input, 0, None)
    }

    /// Same result as [`hash`](Self::hash), also recording every `every`-th
    /// step's torus point into `trace` (cleared first). The point recorded is
    /// the state *after* step `s` for every `s` with `(s + 1) % every == 0`,
    /// so exactly `STEPS / every` points are pushed. `every == 0` records
    /// nothing.
    pub fn hash_traced(&mut self, input: &[u8], every: usize, trace: &mut Vec<TracePoint>) -> [u8; 32] {
        trace.clear();
        if every == 0 {
            return self.hash(input);
        }
        trace.reserve(STEPS / every);
        self.run::<true>(input, every, Some(trace))
    }

    /// Fill the scratchpad from `key` (DESIGN §6 "fill").
    fn fill(&mut self, key: &[u8; 32]) {
        let mut h = blake3::Hasher::new_keyed(key);
        h.update(b"LW1/pow/fill");
        let mut rd = h.finalize_xof();
        for chunk in self.scratch.chunks_mut(FILL_CHUNK / 8) {
            let bytes = &mut self.buf[..chunk.len() * 8];
            rd.fill(bytes);
            for (w, b) in chunk.iter_mut().zip(bytes.chunks_exact(8)) {
                *w = u64::from_le_bytes(b.try_into().unwrap());
            }
        }
        // Sequential dependency pass: every word depends on its predecessor
        // and on one earlier word, so no word can be recomputed on demand
        // without (most of) the prefix before it.
        let s = &mut self.scratch[..];
        for i in 1..SCRATCH_WORDS {
            let r = s[i];
            let j = ((r as u128 * i as u128) >> 64) as usize; // ∈ [0, i)
            s[i] = (s[i - 1].rotate_left(23) ^ r).wrapping_add(s[j]);
        }
    }

    fn run<const TRACE: bool>(
        &mut self,
        input: &[u8],
        every: usize,
        mut trace: Option<&mut Vec<TracePoint>>,
    ) -> [u8; 32] {
        let mut seed = [0u8; 64];
        xof("LW1/pow/seed", &[input], &mut seed);
        self.fill(seed[0..32].try_into().unwrap());

        let rd = |o: usize| u32::from_le_bytes(seed[o..o + 4].try_into().unwrap());
        let (mut x, mut y, mut z, mut w) = (rd(32), rd(36), rd(40), rd(44));
        let mut twist: u32 = 0;

        let s = &mut self.scratch[..];
        let mut countdown = every;
        for step in 0..STEPS {
            let v = &TETRA[(w >> 30) as usize];
            let proj = (x as u64)
                .wrapping_mul(v[0])
                .wrapping_add((y as u64).wrapping_mul(v[1]))
                .wrapping_add((z as u64).wrapping_mul(v[2]))
                .wrapping_add((w as u64).wrapping_mul(v[3]));
            let idx = ((proj ^ (proj >> 29)) & MASK) as usize;
            let m = s[idx];

            let (x2, cx) = x.overflowing_add((m as u32) | 1);
            let y2 = (if cx { !y } else { y }) ^ ((m >> 32) as u32).rotate_left(x2 & 31);
            let (z2, cz) = z.overflowing_add(x2.rotate_left(13) ^ y2);
            let w2 = (if cz { !w } else { w }).wrapping_mul(0x9E37_79B1) ^ z2;
            twist ^= (cx as u32) | ((cz as u32) << 1);
            x = x2;
            y = y2;
            z = z2;
            w = w2;
            s[idx] = m.rotate_left(17) ^ (((x as u64) << 32) | w as u64) ^ step as u64;

            if TRACE {
                countdown -= 1;
                if countdown == 0 {
                    countdown = every;
                    if let Some(t) = trace.as_deref_mut() {
                        t.push(TracePoint { p: [x, y, z, w], twist });
                    }
                }
            }
        }

        let mut p = [0u8; 16];
        for (k, c) in [x, y, z, w].iter().enumerate() {
            p[4 * k..4 * k + 4].copy_from_slice(&c.to_le_bytes());
        }
        let a = s[(x as u64 & MASK) as usize].to_le_bytes();
        let b = s[(z as u64 & MASK) as usize].to_le_bytes();
        h256("LW1/pow/final", &[input, &p, &twist.to_le_bytes(), &a, &b])
    }
}

impl Default for HyperTorus {
    fn default() -> Self {
        Self::new()
    }
}

/// `u256_le(hash) · difficulty < 2^256`.
pub fn meets_difficulty(hash: &[u8; 32], difficulty: u128) -> bool {
    if difficulty == 0 {
        return false;
    }
    let h: [u64; 4] = core::array::from_fn(|i| u64::from_le_bytes(hash[8 * i..8 * i + 8].try_into().unwrap()));
    let d = [difficulty as u64, (difficulty >> 64) as u64];
    // 256 × 128 → 384-bit schoolbook product; valid iff limbs 4 and 5 are 0.
    let mut prod = [0u64; 6];
    for (j, &dj) in d.iter().enumerate() {
        let mut carry: u128 = 0;
        for (i, &hi) in h.iter().enumerate() {
            // ≤ (2^64−1)^2 + 2·(2^64−1) = 2^128 − 1: never overflows u128.
            let t = (hi as u128) * (dj as u128) + prod[i + j] as u128 + carry;
            prod[i + j] = t as u64;
            carry = t >> 64;
        }
        prod[4 + j] = carry as u64; // slot untouched so far
    }
    prod[4] == 0 && prod[5] == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    fn header(nonce: u64) -> Vec<u8> {
        let mut h = b"LW1 test header ".to_vec();
        h.extend_from_slice(&nonce.to_le_bytes());
        h
    }

    /// CONSENSUS PIN: this vector freezes HyperTorus as specified in DESIGN §6.
    /// Any change to it is a hard fork — do not "fix" the expected value.
    #[test]
    fn known_answer_vector() {
        let mut ht = HyperTorus::new();
        assert_eq!(
            hex(&ht.hash(b"LW1/pow/kat/0")),
            "832df7038206cf1ba944c6ed24a74ae9e433d9c7fdb46ca502c2cf27831c048e"
        );
        assert_eq!(
            hex(&ht.hash(&header(0))),
            "dbfca3b3a03212d933c300f027cbd651b2ca6e710abf8b59cc16c5561ed76050"
        );
    }

    #[test]
    fn deterministic_across_instances_and_reuse() {
        let input = header(42);
        let mut a = HyperTorus::new();
        let first = a.hash(&input);
        let _ = a.hash(&header(7)); // dirty the scratchpad
        assert_eq!(a.hash(&input), first, "reused instance");
        assert_eq!(HyperTorus::default().hash(&input), first, "fresh instance");
    }

    #[test]
    fn nonce_changes_hash() {
        let mut ht = HyperTorus::new();
        let h0 = ht.hash(&header(0));
        let h1 = ht.hash(&header(1));
        let h2 = ht.hash(&header(1 << 63));
        assert_ne!(h0, h1);
        assert_ne!(h0, h2);
        assert_ne!(h1, h2);
        assert_ne!(ht.hash(b""), h0);
    }

    #[test]
    fn traced_matches_untraced() {
        let mut ht = HyperTorus::new();
        let input = header(9);
        let plain = ht.hash(&input);
        let mut trace = vec![TracePoint { p: [1, 2, 3, 4], twist: 9 }]; // must be cleared
        for every in [1usize, 1000, 4096, STEPS, STEPS + 1] {
            assert_eq!(ht.hash_traced(&input, every, &mut trace), plain, "every={every}");
            assert_eq!(trace.len(), STEPS / every, "every={every}");
        }
        assert_eq!(ht.hash_traced(&input, 0, &mut trace), plain);
        assert!(trace.is_empty());
        // The walk actually moves and the twist takes several values.
        ht.hash_traced(&input, 256, &mut trace);
        assert!(trace.windows(2).all(|w| w[0].p != w[1].p));
        let twists: std::collections::HashSet<u32> = trace.iter().map(|t| t.twist).collect();
        assert!(twists.len() >= 3, "{twists:?}");
    }

    #[test]
    fn leading_byte_distribution() {
        let mut ht = HyperTorus::new();
        let n = 256usize;
        let mut hist = [0u32; 16];
        let mut bits = 0u32;
        for nonce in 0..n as u64 {
            let h = ht.hash(&header(nonce));
            hist[(h[31] >> 4) as usize] += 1; // top nibble of the LE u256
            bits += h.iter().map(|b| b.count_ones()).sum::<u32>();
        }
        // Expected 16 per bucket; χ² with 15 dof — 50 is far beyond p = 1e-5.
        let chi2: f64 = hist.iter().map(|&c| (c as f64 - 16.0).powi(2) / 16.0).sum();
        assert!(chi2 < 50.0, "chi2={chi2} hist={hist:?}");
        // Mean popcount 128 per hash, σ_total = sqrt(256·64) = 128.
        let expect = 128 * n as i64;
        assert!((bits as i64 - expect).abs() < 6 * 128, "bits={bits}");
    }

    fn u256_from_limbs(l: [u64; 4]) -> [u8; 32] {
        let mut out = [0u8; 32];
        for i in 0..4 {
            out[8 * i..8 * i + 8].copy_from_slice(&l[i].to_le_bytes());
        }
        out
    }

    #[test]
    fn difficulty_zero_and_one() {
        assert!(!meets_difficulty(&[0u8; 32], 0));
        assert!(!meets_difficulty(&[0xFF; 32], 0));
        assert!(meets_difficulty(&[0u8; 32], 1));
        assert!(meets_difficulty(&[0xFF; 32], 1));
        assert!(meets_difficulty(&[0u8; 32], u128::MAX));
    }

    #[test]
    fn difficulty_two() {
        // 2^255 − 1 passes, 2^255 fails.
        let below = u256_from_limbs([u64::MAX, u64::MAX, u64::MAX, u64::MAX >> 1]);
        let at = u256_from_limbs([0, 0, 0, 1 << 63]);
        assert!(meets_difficulty(&below, 2));
        assert!(!meets_difficulty(&at, 2));
        assert!(!meets_difficulty(&[0xFF; 32], 2));
    }

    #[test]
    fn difficulty_powers_of_two() {
        // d = 2^k: threshold hash < 2^(256−k).
        for k in 1..128u32 {
            let d = 1u128 << k;
            let e = 256 - k; // first failing value is 2^e
            let mut at = [0u64; 4];
            at[(e / 64) as usize] = 1u64 << (e % 64);
            let mut below = [0u64; 4];
            for (i, limb) in below.iter_mut().enumerate() {
                let lo = 64 * i as u32;
                *limb = if e >= lo + 64 { u64::MAX } else if e > lo { (1u64 << (e - lo)) - 1 } else { 0 };
            }
            assert!(meets_difficulty(&u256_from_limbs(below), d), "k={k}");
            assert!(!meets_difficulty(&u256_from_limbs(at), d), "k={k}");
        }
    }

    #[test]
    fn difficulty_u128_max_threshold() {
        // D = 2^128 − 1. floor((2^256 − 1) / D) = 2^128 + 1, and
        // (2^128 + 1)(2^128 − 1) = 2^256 − 1 < 2^256 → passes; 2^128 + 2 fails.
        let d = u128::MAX;
        assert!(meets_difficulty(&u256_from_limbs([1, 0, 1, 0]), d));
        assert!(!meets_difficulty(&u256_from_limbs([2, 0, 1, 0]), d));
        assert!(meets_difficulty(&u256_from_limbs([u64::MAX, u64::MAX, 0, 0]), d));
        assert!(!meets_difficulty(&[0xFF; 32], d));
    }

    #[test]
    fn difficulty_odd_threshold() {
        // D = 3: floor((2^256 − 1)/3) = 0x5555…5555 passes (product 2^256 − 1),
        // 0x5555…5556 fails (product 2^256 + 2).
        let q = u256_from_limbs([0x5555_5555_5555_5555; 4]);
        let mut q1 = q;
        q1[0] += 1;
        assert!(meets_difficulty(&q, 3));
        assert!(!meets_difficulty(&q1, 3));
        // D = 2^64 + 1 with both limbs set: ⌊(2^256−1)/D⌋ computed by long division
        // check via property: h·D < 2^256 ⇔ h ≤ ⌊(2^256−1)/D⌋.
        // (2^64+1)·(2^192 − 2^128 + 2^64 − 1) = 2^256 − 1.
        let d = (1u128 << 64) + 1;
        let t = u256_from_limbs([u64::MAX, 0, u64::MAX, 0]); // 2^192 − 2^128 + 2^64 − 1
        let t1 = u256_from_limbs([0, 1, u64::MAX, 0]); // t + 1
        assert!(meets_difficulty(&t, d));
        assert!(!meets_difficulty(&t1, d));
    }

    /// Run with `--nocapture` to see timing; not an assertion.
    #[test]
    fn timing_report() {
        let mut ht = HyperTorus::new();
        ht.hash(b"warmup");
        let n = 10;
        let t = std::time::Instant::now();
        for i in 0..n as u64 {
            std::hint::black_box(ht.hash(&header(1000 + i)));
        }
        let ns = t.elapsed().as_nanos() / n;
        eprintln!("HyperTorus: {ns} ns/hash ({:.1} ms)", ns as f64 / 1e6);
    }
}
