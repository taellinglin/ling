//! Polynomial arithmetic in R_q = Z_q[X]/(X^256 + 1), q = 8380417 (the ML-DSA
//! ring), with an NTT for fast multiplication, and the public matrices A, B
//! (DESIGN §5.1). Implementation is free-form; only `N`, `Q`, `Poly` and
//! `PolyVec` are API that other crates rely on.
//!
//! Conventions:
//! - "Normal" coefficients are kept in `[0, q)`.
//! - The NTT is exactly FIPS 204's (Algorithms 41/42): `zetas[k] = ζ^{brv8(k)}`
//!   with ζ = 1753, output in bit-reversed order. The matrices are sampled
//!   directly in this NTT domain, as ML-DSA's ExpandA does.
//! - All arithmetic on potentially secret data is branch-free (`%` by a
//!   constant compiles to multiply/shift; conditional subtractions use masks).

use alloc::{vec, vec::Vec};
use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU8, Ordering};

use sha3::digest::{ExtendableOutput, Update, XofReader};
use sha3::Shake128;
use zeroize::Zeroize;

pub const N: usize = 256;
pub const Q: i32 = 8_380_417;

const QU: u32 = Q as u32;
const QU64: u64 = Q as u64;
/// Primitive 512th root of unity mod q (FIPS 204).
const ZETA: u64 = 1753;
/// 256^{-1} mod q.
const N_INV: u32 = 8_347_681;

/// Rows of A (public key polys).
pub(crate) const K: usize = 4;
/// Rows of B (key image polys).
pub(crate) const M: usize = 4;
/// Secret length (columns of A and B).
pub(crate) const L: usize = 15;

/// Coefficients kept in [0, q) at API boundaries.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Poly(pub [i32; N]);

/// A vector of polynomials (length k, m, or l depending on use).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PolyVec(pub Vec<Poly>);

impl Zeroize for Poly {
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}

impl Zeroize for PolyVec {
    fn zeroize(&mut self) {
        for p in self.0.iter_mut() {
            p.zeroize();
        }
    }
}

// ---------------------------------------------------------------- reduction

/// `a mod q` for `a ∈ [0, 2q)`, branch-free.
#[inline(always)]
fn csub(a: u32) -> u32 {
    let r = a.wrapping_sub(QU);
    r.wrapping_add(((r as i32) >> 31) as u32 & QU)
}

#[inline(always)]
fn mulmod(a: u32, b: u32) -> u32 {
    ((a as u64 * b as u64) % QU64) as u32
}

/// Map a signed integer with `|x| < q` to `[0, q)`, branch-free.
#[inline(always)]
pub(crate) fn to_mod_q(x: i32) -> i32 {
    x + (Q & (x >> 31))
}

/// Map `x ∈ [0, q)` to its centered representative in `(−q/2, q/2]`, branch-free.
#[inline(always)]
pub(crate) fn center(x: i32) -> i32 {
    // if x > (q-1)/2 { x - q } else { x }
    x - (Q & (((Q - 1) / 2 - x) >> 31))
}

fn powmod(mut b: u64, mut e: u64) -> u64 {
    let mut r = 1u64;
    b %= QU64;
    while e > 0 {
        if e & 1 == 1 {
            r = r * b % QU64;
        }
        b = b * b % QU64;
        e >>= 1;
    }
    r
}

/// A minimal `no_std` once-cell (stands in for `std::sync::OnceLock`, which
/// needs `std`): the first caller computes the value, later callers get the
/// same `&'static`. A concurrent caller (host threads) spins until the
/// winner has published it. The values cached here are pure functions of
/// constants, so which caller wins never changes any output.
struct OnceCell<T> {
    state: AtomicU8, // 0 = empty, 1 = being initialised, 2 = ready
    value: UnsafeCell<MaybeUninit<T>>,
}

// SAFETY: the value is written exactly once, by the single caller that won
// the 0 -> 1 transition, and only read after `state` is observed as 2
// (Release/Acquire), so shared access is never concurrent with the write.
unsafe impl<T: Send + Sync> Sync for OnceCell<T> {}

impl<T> OnceCell<T> {
    const EMPTY: u8 = 0;
    const BUSY: u8 = 1;
    const READY: u8 = 2;

    const fn new() -> Self {
        OnceCell { state: AtomicU8::new(Self::EMPTY), value: UnsafeCell::new(MaybeUninit::uninit()) }
    }

    fn get_or_init(&self, f: impl FnOnce() -> T) -> &T {
        if self.state.load(Ordering::Acquire) != Self::READY {
            if self
                .state
                .compare_exchange(Self::EMPTY, Self::BUSY, Ordering::Acquire, Ordering::Acquire)
                .is_ok()
            {
                let v = f();
                // SAFETY: we own the BUSY state; nobody reads until READY.
                unsafe { (*self.value.get()).write(v) };
                self.state.store(Self::READY, Ordering::Release);
            } else {
                while self.state.load(Ordering::Acquire) != Self::READY {
                    #[cfg(feature = "std")]
                    std::thread::yield_now();
                    #[cfg(not(feature = "std"))]
                    core::hint::spin_loop();
                }
            }
        }
        // SAFETY: state is READY, so the value was initialised and is never
        // written again.
        unsafe { (*self.value.get()).assume_init_ref() }
    }
}

fn brv8(k: usize) -> usize {
    (k as u8).reverse_bits() as usize
}

/// `zetas[k] = ζ^{brv8(k)} mod q`, computed once from ζ.
fn zetas() -> &'static [u32; N] {
    static Z: OnceCell<[u32; N]> = OnceCell::new();
    Z.get_or_init(|| {
        let mut z = [0u32; N];
        for (k, zk) in z.iter_mut().enumerate() {
            *zk = powmod(ZETA, brv8(k) as u64) as u32;
        }
        z
    })
}

// ---------------------------------------------------------------- Poly ops

impl Poly {
    pub const fn zero() -> Self {
        Poly([0; N])
    }

    /// In-place forward NTT (FIPS 204 Alg. 41). Input and output in [0, q).
    pub fn ntt(&mut self) {
        let z = zetas();
        let w = &mut self.0;
        let mut m = 0usize;
        let mut len = 128usize;
        while len >= 1 {
            let mut start = 0usize;
            while start < N {
                m += 1;
                let zeta = z[m];
                for j in start..start + len {
                    let t = mulmod(zeta, w[j + len] as u32);
                    let a = w[j] as u32;
                    w[j + len] = csub(a + QU - t) as i32;
                    w[j] = csub(a + t) as i32;
                }
                start += 2 * len;
            }
            len >>= 1;
        }
    }

    /// In-place inverse NTT (FIPS 204 Alg. 42). Input and output in [0, q).
    pub fn inv_ntt(&mut self) {
        let z = zetas();
        let w = &mut self.0;
        let mut m = N;
        let mut len = 1usize;
        while len < N {
            let mut start = 0usize;
            while start < N {
                m -= 1;
                let zeta = QU - z[m]; // −zetas[m]
                for j in start..start + len {
                    let a = w[j] as u32;
                    let b = w[j + len] as u32;
                    w[j] = csub(a + b) as i32;
                    w[j + len] = mulmod(zeta, csub(a + QU - b)) as i32;
                }
                start += 2 * len;
            }
            len <<= 1;
        }
        for c in w.iter_mut() {
            *c = mulmod(*c as u32, N_INV) as i32;
        }
    }

    /// Coefficient-wise product (both operands in the NTT domain).
    pub fn pointwise(&self, other: &Poly) -> Poly {
        let mut r = Poly::zero();
        for i in 0..N {
            r.0[i] = mulmod(self.0[i] as u32, other.0[i] as u32) as i32;
        }
        r
    }

    pub fn add(&self, other: &Poly) -> Poly {
        let mut r = Poly::zero();
        for i in 0..N {
            r.0[i] = csub(self.0[i] as u32 + other.0[i] as u32) as i32;
        }
        r
    }

    pub fn sub(&self, other: &Poly) -> Poly {
        let mut r = Poly::zero();
        for i in 0..N {
            r.0[i] = csub(self.0[i] as u32 + QU - other.0[i] as u32) as i32;
        }
        r
    }

    /// Full product in R_q via the NTT (inputs/outputs in normal form).
    pub fn mul(&self, other: &Poly) -> Poly {
        let mut a = self.clone();
        let mut b = other.clone();
        a.ntt();
        b.ntt();
        let mut r = a.pointwise(&b);
        r.inv_ntt();
        a.zeroize();
        b.zeroize();
        r
    }

    /// Reference O(n²) negacyclic product (used by tests).
    pub fn schoolbook_mul(&self, other: &Poly) -> Poly {
        let mut acc = [0i128; 2 * N];
        for i in 0..N {
            for j in 0..N {
                acc[i + j] += self.0[i] as i128 * other.0[j] as i128;
            }
        }
        let mut r = Poly::zero();
        for i in 0..N {
            r.0[i] = (acc[i] - acc[i + N]).rem_euclid(Q as i128) as i32;
        }
        r
    }

    /// True iff every coefficient is in [0, q).
    pub fn is_reduced(&self) -> bool {
        self.0.iter().all(|&c| (0..Q).contains(&c))
    }
}

impl PolyVec {
    pub fn zero(len: usize) -> Self {
        PolyVec(vec![Poly::zero(); len])
    }
    pub fn ntt(&mut self) {
        for p in self.0.iter_mut() {
            p.ntt();
        }
    }
    pub fn inv_ntt(&mut self) {
        for p in self.0.iter_mut() {
            p.inv_ntt();
        }
    }
    pub fn add(&self, other: &PolyVec) -> PolyVec {
        assert_eq!(self.0.len(), other.0.len());
        PolyVec(self.0.iter().zip(&other.0).map(|(a, b)| a.add(b)).collect())
    }
}

// ---------------------------------------------------------------- bit packing

/// Little-endian bit packing of `vals` at `bits` bits each into `out`
/// (bit b of the stream is bit `b mod 8` of byte `b / 8`). The total bit
/// count must equal `8·out.len()` exactly.
pub(crate) fn pack_bits(vals: impl Iterator<Item = u32>, bits: u32, out: &mut [u8]) {
    let mut acc: u64 = 0;
    let mut nacc: u32 = 0;
    let mut pos = 0usize;
    for v in vals {
        debug_assert!(v < (1u32 << bits));
        acc |= (v as u64) << nacc;
        nacc += bits;
        while nacc >= 8 {
            out[pos] = acc as u8;
            pos += 1;
            acc >>= 8;
            nacc -= 8;
        }
    }
    assert!(nacc == 0 && pos == out.len(), "pack_bits length mismatch");
}

/// Inverse of [`pack_bits`]; `out.len()·bits` must equal `8·inp.len()`.
pub(crate) fn unpack_bits(inp: &[u8], bits: u32, out: &mut [u32]) {
    assert_eq!(out.len() as u64 * bits as u64, inp.len() as u64 * 8);
    let mask = (1u64 << bits) - 1;
    let mut acc: u64 = 0;
    let mut nacc: u32 = 0;
    let mut bytes = inp.iter();
    for o in out.iter_mut() {
        while nacc < bits {
            acc |= (*bytes.next().expect("length checked") as u64) << nacc;
            nacc += 8;
        }
        *o = (acc & mask) as u32;
        acc >>= bits;
        nacc -= bits;
    }
}

/// Bytes for `n_polys` polynomials at 23 bits per coefficient.
pub(crate) const fn packed23_len(n_polys: usize) -> usize {
    n_polys * N * 23 / 8
}

/// 23-bit packing of reduced coefficients (DESIGN §5.2).
pub(crate) fn pack23(v: &[Poly]) -> Vec<u8> {
    let mut out = vec![0u8; packed23_len(v.len())];
    pack_bits(
        v.iter().flat_map(|p| p.0.iter()).map(|&c| {
            debug_assert!((0..Q).contains(&c));
            c as u32
        }),
        23,
        &mut out,
    );
    out
}

/// Strict inverse of [`pack23`]: wrong length or any coefficient ≥ q ⇒ None.
pub(crate) fn unpack23(b: &[u8], n_polys: usize) -> Option<Vec<Poly>> {
    if b.len() != packed23_len(n_polys) {
        return None;
    }
    let mut vals = vec![0u32; n_polys * N];
    unpack_bits(b, 23, &mut vals);
    let mut polys = Vec::with_capacity(n_polys);
    for chunk in vals.chunks(N) {
        let mut p = Poly::zero();
        for (d, &s) in p.0.iter_mut().zip(chunk) {
            if s >= QU {
                return None;
            }
            *d = s as i32;
        }
        polys.push(p);
    }
    Some(polys)
}

// ---------------------------------------------------------------- matrices

/// RejNTTPoly: SHAKE128(seed ‖ u8(col) ‖ u8(row)); 3-byte little-endian
/// candidates with the top bit cleared (23 bits), accepted iff < q. The
/// result is the NTT-domain representation (as in ML-DSA ExpandA).
fn rej_ntt_poly(seed: &[u8], col: u8, row: u8) -> Poly {
    let mut h = Shake128::default();
    h.update(seed);
    h.update(&[col, row]);
    let mut rd = h.finalize_xof();
    let mut p = Poly::zero();
    let mut i = 0usize;
    let mut buf = [0u8; 168];
    while i < N {
        rd.read(&mut buf);
        for c in buf.chunks_exact(3) {
            let v = c[0] as u32 | (c[1] as u32) << 8 | ((c[2] & 0x7f) as u32) << 16;
            if v < QU {
                p.0[i] = v as i32;
                i += 1;
                if i == N {
                    break;
                }
            }
        }
    }
    p
}

/// Row-major `rows × L` matrix in the NTT domain.
pub(crate) struct Matrix {
    pub rows: usize,
    pub e: Vec<Poly>,
}

impl Matrix {
    fn expand(seed: &[u8], rows: usize) -> Self {
        let mut e = Vec::with_capacity(rows * L);
        for r in 0..rows {
            for c in 0..L {
                e.push(rej_ntt_poly(seed, c as u8, r as u8));
            }
        }
        Matrix { rows, e }
    }

    /// `M·v̂` with `v̂` (length L) and result in the NTT domain.
    pub fn mul_ntt(&self, v_hat: &[Poly]) -> Vec<Poly> {
        assert_eq!(v_hat.len(), L);
        let mut out = Vec::with_capacity(self.rows);
        let mut acc = [0u64; N];
        for r in 0..self.rows {
            acc.iter_mut().for_each(|a| *a = 0);
            for (c, v) in v_hat.iter().enumerate() {
                let a = &self.e[r * L + c].0;
                for j in 0..N {
                    acc[j] += a[j] as u64 * v.0[j] as u64; // ≤ 15·q² < 2^51
                }
            }
            let mut p = Poly::zero();
            for j in 0..N {
                p.0[j] = (acc[j] % QU64) as i32;
            }
            out.push(p);
        }
        acc.zeroize();
        out
    }
}

/// `A ∈ R_q^{k×l}` from seed `"LW1/lrs/A"`, NTT domain, cached.
pub(crate) fn matrix_a() -> &'static Matrix {
    static A: OnceCell<Matrix> = OnceCell::new();
    A.get_or_init(|| Matrix::expand(b"LW1/lrs/A", K))
}

/// `B ∈ R_q^{m×l}` from seed `"LW1/lrs/B"`, NTT domain, cached.
pub(crate) fn matrix_b() -> &'static Matrix {
    static B: OnceCell<Matrix> = OnceCell::new();
    B.get_or_init(|| Matrix::expand(b"LW1/lrs/B", M))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{Rng, SeedableRng};

    fn rand_poly(rng: &mut impl Rng) -> Poly {
        let mut p = Poly::zero();
        for c in p.0.iter_mut() {
            *c = rng.gen_range(0..Q);
        }
        p
    }

    #[test]
    fn constants() {
        assert_eq!(256 * N_INV as u64 % QU64, 1);
        assert_eq!(powmod(ZETA, 256), QU64 - 1); // ζ is a primitive 512th root
        assert_eq!(powmod(ZETA, 512), 1);
        let z = zetas();
        // First entries of FIPS 204 Appendix B.
        assert_eq!(z[1], 4_808_194);
        assert_eq!(z[2], 3_765_607);
        assert_eq!(z[3], 3_761_513);
        assert_eq!(z[4], 5_178_923);
    }

    #[test]
    fn ntt_roundtrip_and_matches_schoolbook() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(7);
        for _ in 0..20 {
            let a = rand_poly(&mut rng);
            let b = rand_poly(&mut rng);
            let mut t = a.clone();
            t.ntt();
            assert!(t.is_reduced());
            t.inv_ntt();
            assert_eq!(t, a);
            assert_eq!(a.mul(&b), a.schoolbook_mul(&b));
        }
        // Edge values: all q−1, and X·X^255 = −1.
        let full = Poly([Q - 1; N]);
        assert_eq!(full.mul(&full), full.schoolbook_mul(&full));
        let mut x = Poly::zero();
        x.0[1] = 1;
        let mut x255 = Poly::zero();
        x255.0[255] = 1;
        let mut minus_one = Poly::zero();
        minus_one.0[0] = Q - 1;
        assert_eq!(x.mul(&x255), minus_one);
    }

    #[test]
    fn center_and_mod() {
        for x in [-(Q - 1) / 2, -5, -1, 0, 1, 5, (Q - 1) / 2] {
            let m = to_mod_q(x);
            assert!((0..Q).contains(&m));
            assert_eq!(center(m), x);
        }
    }

    #[test]
    fn pack23_roundtrip_and_rejects() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(8);
        let v: Vec<Poly> = (0..4).map(|_| rand_poly(&mut rng)).collect();
        let b = pack23(&v);
        assert_eq!(b.len(), 2944);
        assert_eq!(unpack23(&b, 4).unwrap(), v);
        assert!(unpack23(&b[..2943], 4).is_none());
        let mut longer = b.clone();
        longer.push(0);
        assert!(unpack23(&longer, 4).is_none());
        // First coefficient := q exactly.
        let mut bad = b.clone();
        bad[0] = (QU & 0xff) as u8;
        bad[1] = ((QU >> 8) & 0xff) as u8;
        bad[2] = (bad[2] & 0x80) | ((QU >> 16) as u8 & 0x7f);
        assert!(unpack23(&bad, 4).is_none());
        // First coefficient := q − 1 is fine.
        let mut ok = bad.clone();
        ok[0] -= 1;
        assert!(unpack23(&ok, 4).is_some());
        // Last coefficient := 2^23 − 1.
        let mut bad3 = b;
        let n = bad3.len();
        bad3[n - 1] = 0xff;
        bad3[n - 2] = 0xff;
        bad3[n - 3] |= 0xfe;
        assert!(unpack23(&bad3, 4).is_none());
    }

    #[test]
    fn matrices_deterministic_and_uniformish() {
        let a = matrix_a();
        let b = matrix_b();
        assert_eq!(a.e.len(), K * L);
        assert_eq!(b.e.len(), M * L);
        assert!(a.e.iter().chain(&b.e).all(|p| p.is_reduced()));
        assert_ne!(a.e[0], b.e[0]);
        assert_ne!(a.e[0], a.e[1]);
        assert_eq!(rej_ntt_poly(b"LW1/lrs/A", 0, 0), a.e[0]);
        assert_eq!(rej_ntt_poly(b"LW1/lrs/B", 3, 2), b.e[2 * L + 3]);
        let mean: f64 =
            a.e.iter().flat_map(|p| p.0.iter()).map(|&c| c as f64).sum::<f64>() / (K * L * N) as f64;
        assert!((mean / (Q as f64 / 2.0) - 1.0).abs() < 0.02);
    }

    #[test]
    fn matrix_mul_matches_naive() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(9);
        let v: Vec<Poly> = (0..L).map(|_| rand_poly(&mut rng)).collect();
        let mut v_hat = PolyVec(v.clone());
        v_hat.ntt();
        let mut r = PolyVec(matrix_a().mul_ntt(&v_hat.0));
        r.inv_ntt();
        let mut acc = Poly::zero();
        for (c, vc) in v.iter().enumerate() {
            let mut a = matrix_a().e[c].clone();
            a.inv_ntt();
            acc = acc.add(&a.schoolbook_mul(vc));
        }
        assert_eq!(r.0[0], acc);
    }
}
