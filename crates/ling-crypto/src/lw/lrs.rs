//! LRS-87: experimental lattice linkable ring signature (DESIGN §5).
//!
//! **Experimental.** See DESIGN §5.5 for exactly what is and isn't known.
//!
//! Implementation notes:
//! - Secrets (`s_spend`, `s_o`, the mask `y`, their NTT images and `c·s`) are
//!   zeroized after use / on drop. Arithmetic on them is branch-free.
//! - Every ring member costs the same work in each signing attempt: the
//!   signer's commitment goes through the same routine as a simulated member,
//!   with the challenge polynomial set to zero.
//! - Signing randomness is hedged: the stream seed is
//!   `H("LW1/lrs/sign-seed", s_o, μ, R, I, 32 B OS randomness)`.

use alloc::{vec, vec::Vec};

use sha3::digest::{ExtendableOutput, Update, XofReader};
use sha3::Shake256;
use zeroize::{Zeroize, Zeroizing};

use crate::lw::hash::{h256, xof, XofStream};
use crate::lw::lattice::{self, center, matrix_a, matrix_b, pack23, to_mod_q, unpack23, Poly, PolyVec, N, Q};
use crate::lw::{CryptoError, Result};

pub const K: usize = 4;
pub const M: usize = 4;
pub const L: usize = 15;
pub const ETA_SPEND: i32 = 2;
pub const ETA_OFFSET: i32 = 2;
pub const ETA: i32 = 4;
pub const TAU: usize = 60;
pub const BETA: i32 = 240;
pub const GAMMA: i32 = 1 << 19;
pub const RING_SIZE: usize = 11;
/// Largest ring `sign`/`verify`/`from_bytes` accept (DESIGN §5.1).
pub const MAX_RING_LEN: usize = 64;

pub const PUBLIC_KEY_LEN: usize = 2944; // k polys × 256 × 23 bits
pub const KEY_IMAGE_LEN: usize = 2944; // m polys × 256 × 23 bits
pub const Z_LEN: usize = 9600; // l polys × 256 × 20 bits

const _: () = {
    assert!(K == lattice::K && M == lattice::M && L == lattice::L);
    assert!(PUBLIC_KEY_LEN == lattice::packed23_len(K));
    assert!(KEY_IMAGE_LEN == lattice::packed23_len(M));
    assert!(Z_LEN == L * N * 20 / 8);
    assert!(BETA == TAU as i32 * ETA && ETA == ETA_SPEND + ETA_OFFSET);
};

/// Responses are accepted iff every coefficient satisfies `|z| < Z_BOUND`.
const Z_BOUND: i32 = GAMMA - BETA;

/// One-time (or spend) public key `t = A·s`.
#[derive(Clone, PartialEq, Eq, Debug, Hash)]
pub struct PublicKey(pub Vec<u8>); // PUBLIC_KEY_LEN packed bytes

/// Key image `I = B·s_o`.
#[derive(Clone, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub struct KeyImage(pub Vec<u8>); // KEY_IMAGE_LEN packed bytes

/// Long-term spend secret `s_spend` (coefficients in [−2, 2]). Zeroized on drop.
pub struct SpendSecret {
    /// L polys, coefficients mod q in [0, q).
    s: PolyVec,
}

/// One-time secret `s_o = s_spend + δ` (coefficients in [−4, 4]). Zeroized on drop.
pub struct OneTimeSecret {
    /// L polys, coefficients mod q in [0, q).
    s: PolyVec,
}

impl Drop for SpendSecret {
    fn drop(&mut self) {
        self.s.zeroize();
    }
}

impl Drop for OneTimeSecret {
    fn drop(&mut self) {
        self.s.zeroize();
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RingSignature {
    pub ch0: [u8; 32],
    pub z: Vec<Vec<u8>>, // RING_SIZE (or bootstrap-ring) entries of Z_LEN bytes
}

// ====================================================================== helpers

/// L polynomials of centered integer coefficients (a response or a mask).
type ZVec = Vec<[i32; N]>;

/// `M·s` for a normal-form secret/public vector `s` (length L), normal form out.
fn mat_apply(m: &lattice::Matrix, s: &PolyVec) -> Vec<Poly> {
    let mut s_hat = s.clone();
    s_hat.ntt();
    let mut r = PolyVec(m.mul_ntt(&s_hat.0));
    s_hat.zeroize();
    r.inv_ntt();
    r.0
}

/// SampleSmall(seed, η = 2): for each poly r, RejBoundedPoly over
/// `XOF("LW1/lrs/small", seed, u32_le(r))`: each byte gives nibbles b0 (low)
/// then b1 (high); a nibble b < 15 yields coefficient `2 − (b mod 5)`.
fn sample_small(seed: &[u8; 32]) -> PolyVec {
    let mut out = PolyVec::zero(L);
    let mut buf = Zeroizing::new([0u8; 136]);
    for (r, poly) in out.0.iter_mut().enumerate() {
        let mut xs = XofStream::new("LW1/lrs/small", &[seed, &(r as u32).to_le_bytes()]);
        let mut i = 0usize;
        'fill: loop {
            xs.read(buf.as_mut());
            for &byte in buf.iter() {
                for nib in [byte & 0x0f, byte >> 4] {
                    if nib < 15 {
                        poly.0[i] = to_mod_q(2 - (nib as i32 % 5));
                        i += 1;
                        if i == N {
                            break 'fill;
                        }
                    }
                }
            }
        }
    }
    out
}

/// One-time offset `δ = SampleSmall(XOF("LW1/onetime", ss, u32_le(j), 32))`.
fn onetime_offset(shared: &[u8; 32], index: u32) -> PolyVec {
    let mut seed = Zeroizing::new([0u8; 32]);
    xof("LW1/onetime", &[shared, &index.to_le_bytes()], seed.as_mut());
    sample_small(&seed)
}

fn decode_pk_polys(pk: &PublicKey) -> Option<Vec<Poly>> {
    unpack23(&pk.0, K)
}

fn ntt_all(mut v: Vec<Poly>) -> Vec<Poly> {
    v.iter_mut().for_each(Poly::ntt);
    v
}

/// ML-DSA SampleInBall (FIPS 204 Alg. 29) over SHAKE256(ch), τ = 60.
/// Inputs are public, so the data-dependent loop is fine.
fn sample_in_ball(ch: &[u8; 32]) -> Poly {
    let mut h = Shake256::default();
    h.update(ch);
    let mut rd = h.finalize_xof();
    let mut sb = [0u8; 8];
    rd.read(&mut sb);
    let signs = u64::from_le_bytes(sb);
    let mut c = Poly::zero();
    for (k, i) in (N - TAU..N).enumerate() {
        let j = loop {
            let mut b = [0u8; 1];
            rd.read(&mut b);
            if (b[0] as usize) <= i {
                break b[0] as usize;
            }
        };
        c.0[i] = c.0[j];
        c.0[j] = if (signs >> k) & 1 == 1 { Q - 1 } else { 1 };
    }
    c
}

/// Response encoding (DESIGN §5.2): `γ − 1 − z` in 20 bits, l·256 coefficients.
fn encode_z(z: &[[i32; N]]) -> Vec<u8> {
    debug_assert_eq!(z.len(), L);
    let mut out = vec![0u8; Z_LEN];
    lattice::pack_bits(
        z.iter().flat_map(|p| p.iter()).map(|&c| {
            let v = GAMMA - 1 - c;
            assert!((0..1 << 20).contains(&v), "response coefficient not encodable");
            v as u32
        }),
        20,
        &mut out,
    );
    out
}

/// Decode a response. Wrong length ⇒ None. With `enforce_bound`, any
/// coefficient with `|z| ≥ γ − β` ⇒ None (the normal, strict decoder).
fn decode_z_inner(b: &[u8], enforce_bound: bool) -> Option<ZVec> {
    if b.len() != Z_LEN {
        return None;
    }
    let mut vals = vec![0u32; L * N];
    lattice::unpack_bits(b, 20, &mut vals);
    let mut z = vec![[0i32; N]; L];
    for (p, chunk) in z.iter_mut().zip(vals.chunks(N)) {
        for (d, &v) in p.iter_mut().zip(chunk) {
            let c = GAMMA - 1 - v as i32;
            if enforce_bound && c.abs() >= Z_BOUND {
                return None;
            }
            *d = c;
        }
    }
    Some(z)
}

fn decode_z(b: &[u8]) -> Option<ZVec> {
    decode_z_inner(b, true)
}

/// `R = H("LW1/lrs/ring", t_0 ‖ … ‖ t_{N−1})` — one part, the concatenation.
fn ring_hash(ring: &[PublicKey]) -> [u8; 32] {
    let mut cat = Vec::with_capacity(ring.len() * PUBLIC_KEY_LEN);
    for t in ring {
        cat.extend_from_slice(&t.0);
    }
    h256("LW1/lrs/ring", &[&cat])
}

/// Per-signature public context of the challenge chain.
struct ChainCtx<'a> {
    msg: &'a [u8; 32],
    ring_hash: [u8; 32],
    ki_bytes: &'a [u8],
    ki_hat: Vec<Poly>,
    t_hat: Vec<Vec<Poly>>,
}

impl ChainCtx<'_> {
    /// `H("LW1/lrs/ch", μ, R, I, u32_le(i), pack23(A·v − c·t_i), pack23(B·v − c·I))`
    /// with `c_hat` = NTT(c). Identical work whatever `v` and `c` are.
    fn step(&self, i: usize, v: &[[i32; N]], c_hat: &Poly) -> [u8; 32] {
        let mut v_hat: Vec<Poly> = Vec::with_capacity(L);
        for coeffs in v {
            let mut p = Poly::zero();
            for (d, &s) in p.0.iter_mut().zip(coeffs.iter()) {
                *d = to_mod_q(s);
            }
            p.ntt();
            v_hat.push(p);
        }
        let mut w1 = matrix_a().mul_ntt(&v_hat);
        let mut w2 = matrix_b().mul_ntt(&v_hat);
        for (w, t) in w1.iter_mut().zip(&self.t_hat[i]) {
            *w = w.sub(&c_hat.pointwise(t));
            w.inv_ntt();
        }
        for (w, ki) in w2.iter_mut().zip(&self.ki_hat) {
            *w = w.sub(&c_hat.pointwise(ki));
            w.inv_ntt();
        }
        let mut p1 = Zeroizing::new(pack23(&w1));
        let mut p2 = Zeroizing::new(pack23(&w2));
        let out = h256(
            "LW1/lrs/ch",
            &[self.msg, &self.ring_hash, self.ki_bytes, &(i as u32).to_le_bytes(), &p1, &p2],
        );
        v_hat.iter_mut().for_each(Zeroize::zeroize);
        w1.iter_mut().for_each(Zeroize::zeroize);
        w2.iter_mut().for_each(Zeroize::zeroize);
        p1.zeroize();
        p2.zeroize();
        out
    }
}

/// Signing randomness: an XOF stream, read in 640-byte blocks
/// (256 candidates of 20 bits each).
struct SignRng {
    xs: XofStream,
    buf: Zeroizing<[u8; 640]>,
    pos: usize, // in 20-bit candidates
}

impl SignRng {
    fn new(seed: &[u8; 32]) -> Self {
        SignRng { xs: XofStream::new("LW1/lrs/sign-rng", &[seed]), buf: Zeroizing::new([0u8; 640]), pos: 256 }
    }

    fn next20(&mut self) -> u32 {
        if self.pos == 256 {
            self.xs.read(self.buf.as_mut());
            self.pos = 0;
        }
        let bit = self.pos * 20;
        let byte = bit / 8;
        let mut w = [0u8; 4];
        let take = (self.buf.len() - byte).min(4);
        w[..take].copy_from_slice(&self.buf[byte..byte + take]);
        let v = (u32::from_le_bytes(w) >> (bit % 8)) & 0xF_FFFF;
        w.zeroize();
        self.pos += 1;
        v
    }

    /// Uniform in `[−half, half]` for every one of the L·256 coefficients.
    /// Rejection depends only on the random stream, never on secrets.
    fn uniform_vec(&mut self, half: i32) -> ZVec {
        let range = (2 * half + 1) as u32;
        debug_assert!(range <= 1 << 20);
        let mut z = vec![[0i32; N]; L];
        for p in z.iter_mut() {
            for c in p.iter_mut() {
                let v = loop {
                    let v = self.next20();
                    if v < range {
                        break v;
                    }
                };
                *c = v as i32 - half;
            }
        }
        z
    }
}

/// Branch-free: 0 iff every coefficient has `|x| < bound`, else nonzero.
fn exceeds(v: &[[i32; N]], bound: i32) -> i32 {
    let mut flag = 0i32;
    for p in v {
        for &x in p.iter() {
            let m = x >> 31;
            let a = (x ^ m) - m;
            flag |= (bound - 1 - a) >> 31;
        }
    }
    flag
}

// ====================================================================== API

impl PublicKey {
    /// Validates length and that every coefficient is < q.
    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        unpack23(b, K).ok_or(CryptoError::Malformed)?;
        Ok(PublicKey(b.to_vec()))
    }
}

impl KeyImage {
    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        unpack23(b, M).ok_or(CryptoError::Malformed)?;
        Ok(KeyImage(b.to_vec()))
    }
    /// Compact 32-byte id for indexing: `H("LW1/lrs/ki-id", bytes)`.
    pub fn id(&self) -> [u8; 32] {
        h256("LW1/lrs/ki-id", &[&self.0])
    }
}

impl SpendSecret {
    /// `SampleSmall(spend_seed, η_spend)` (DESIGN §3.1).
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        SpendSecret { s: sample_small(seed) }
    }
    pub fn public(&self) -> PublicKey {
        PublicKey(pack23(&mat_apply(matrix_a(), &self.s)))
    }
}

impl OneTimeSecret {
    pub fn public(&self) -> PublicKey {
        PublicKey(pack23(&mat_apply(matrix_a(), &self.s)))
    }
    pub fn key_image(&self) -> KeyImage {
        KeyImage(pack23(&mat_apply(matrix_b(), &self.s)))
    }
}

/// Sender side: `t_o = t_spend + A·δ(shared, index)`.
///
/// Panics if `spend_pub` is not a valid encoding (construct it with
/// [`PublicKey::from_bytes`]).
pub fn derive_onetime_public(spend_pub: &PublicKey, shared: &[u8; 32], index: u32) -> PublicKey {
    let t = decode_pk_polys(spend_pub).expect("derive_onetime_public: malformed spend public key");
    let mut delta = onetime_offset(shared, index);
    let ad = mat_apply(matrix_a(), &delta);
    delta.zeroize();
    let to: Vec<Poly> = t.iter().zip(&ad).map(|(a, b)| a.add(b)).collect();
    PublicKey(pack23(&to))
}

/// Recipient side: `s_o = s_spend + δ(shared, index)`.
pub fn derive_onetime_secret(spend: &SpendSecret, shared: &[u8; 32], index: u32) -> OneTimeSecret {
    let mut delta = onetime_offset(shared, index);
    let s = spend.s.add(&delta);
    delta.zeroize();
    OneTimeSecret { s }
}

/// Sign `msg` with `secret`, which must correspond to `ring[signer]`.
/// Panics if it doesn't (caller bug). Ring order is the caller's (sorted by
/// global index in practice).
pub fn sign(msg: &[u8; 32], ring: &[PublicKey], signer: usize, secret: &OneTimeSecret) -> RingSignature {
    let mut rnd = Zeroizing::new([0u8; 32]);
    crate::lw::kdf::random_bytes(rnd.as_mut());
    sign_inner(msg, ring, signer, secret, &rnd, |_, _| {})
}

/// [`sign`] that returns `Malformed` instead of panicking when the ring size
/// is outside `1..=MAX_RING_LEN`, `signer` is out of range, a ring member
/// does not decode canonically, or `secret` does not match `ring[signer]`.
pub fn try_sign(msg: &[u8; 32], ring: &[PublicKey], signer: usize, secret: &OneTimeSecret) -> Result<RingSignature> {
    let n = ring.len();
    if !(1..=MAX_RING_LEN).contains(&n) || signer >= n {
        return Err(CryptoError::Malformed);
    }
    if ring.iter().any(|pk| decode_pk_polys(pk).is_none()) {
        return Err(CryptoError::Malformed);
    }
    if secret.public() != ring[signer] {
        return Err(CryptoError::Malformed);
    }
    Ok(sign(msg, ring, signer, secret))
}

/// [`derive_onetime_public`] that returns `Malformed` instead of panicking on
/// a non-canonical spend public key.
pub fn try_derive_onetime_public(spend_pub: &PublicKey, shared: &[u8; 32], index: u32) -> Result<PublicKey> {
    decode_pk_polys(spend_pub).ok_or(CryptoError::Malformed)?;
    Ok(derive_onetime_public(spend_pub, shared, index))
}

/// The signing algorithm (DESIGN §5.4). `tamper` is a test hook applied to
/// each simulated response before it is hashed; production passes a no-op.
fn sign_inner(
    msg: &[u8; 32],
    ring: &[PublicKey],
    signer: usize,
    secret: &OneTimeSecret,
    os_rnd: &[u8; 32],
    tamper: impl Fn(usize, &mut ZVec),
) -> RingSignature {
    let n = ring.len();
    assert!((1..=MAX_RING_LEN).contains(&n), "lrs::sign: ring size must be 1..={MAX_RING_LEN}");
    assert!(signer < n, "lrs::sign: signer index out of range");
    let t_hat: Vec<Vec<Poly>> = ring
        .iter()
        .map(|pk| ntt_all(decode_pk_polys(pk).expect("lrs::sign: malformed ring member")))
        .collect();
    assert!(secret.public() == ring[signer], "lrs::sign: secret does not match ring[signer]");

    let ki = mat_apply(matrix_b(), &secret.s);
    let ki_bytes = pack23(&ki);
    let ctx = ChainCtx { msg, ring_hash: ring_hash(ring), ki_bytes: &ki_bytes, ki_hat: ntt_all(ki), t_hat };

    // Hedged seed: secret ‖ μ ‖ R ‖ I ‖ OS randomness.
    let mut s_bytes = Zeroizing::new(Vec::with_capacity(L * N * 4));
    for p in &secret.s.0 {
        for &c in p.0.iter() {
            s_bytes.extend_from_slice(&(c as u32).to_le_bytes());
        }
    }
    let seed = Zeroizing::new(h256(
        "LW1/lrs/sign-seed",
        &[&s_bytes, msg, &ctx.ring_hash, &ki_bytes, os_rnd],
    ));
    drop(s_bytes);
    let mut rng = SignRng::new(&seed);
    drop(seed);

    let mut s_hat = secret.s.clone();
    s_hat.ntt();
    let zero_c = Poly::zero();
    let mut ch = vec![[0u8; 32]; n];
    let mut z: Vec<ZVec> = vec![Vec::new(); n];

    loop {
        // Signer commitment: same routine as everyone else, with c = 0.
        let mut y = Zeroizing::new(rng.uniform_vec(GAMMA - 1));
        ch[(signer + 1) % n] = ctx.step(signer, &y, &zero_c);
        for k in 1..n {
            let i = (signer + k) % n;
            let mut zi = rng.uniform_vec(Z_BOUND - 1);
            tamper(i, &mut zi);
            let mut c_hat = sample_in_ball(&ch[i]);
            c_hat.ntt();
            ch[(i + 1) % n] = ctx.step(i, &zi, &c_hat);
            z[i] = zi;
        }

        // z_π = y + c_π·s_o, exact over the integers since |c·s|∞ ≤ τ·η = β.
        let mut c_hat = sample_in_ball(&ch[signer]);
        c_hat.ntt();
        let mut zp: Zeroizing<ZVec> = Zeroizing::new(vec![[0i32; N]; L]);
        let mut cs_flag = 0i32;
        for (r, s) in s_hat.0.iter().enumerate() {
            let mut cs = c_hat.pointwise(s);
            cs.inv_ntt();
            for j in 0..N {
                let v = center(cs.0[j]);
                let m = v >> 31;
                cs_flag |= (BETA - ((v ^ m) - m)) >> 31; // |v| > β ⇒ flag
                zp[r][j] = y[r][j] + v;
            }
            cs.zeroize();
        }
        y.zeroize();
        assert!(cs_flag == 0, "lrs::sign: one-time secret out of range");

        if exceeds(&zp, Z_BOUND) == 0 {
            z[signer] = zp.to_vec();
            zp.zeroize();
            break;
        }
        // Rejected: the would-be response (y + c·s) is not uniform, discard it.
        zp.zeroize();
    }
    s_hat.zeroize();

    let out = RingSignature { ch0: ch[0], z: z.iter().map(|zi| encode_z(zi)).collect() };
    if let Some(zs) = z.get_mut(signer) {
        zs.zeroize();
    }
    out
}

pub fn verify(msg: &[u8; 32], ring: &[PublicKey], key_image: &KeyImage, sig: &RingSignature) -> bool {
    verify_inner(msg, ring, key_image, sig, true).unwrap_or(false)
}

fn verify_inner(
    msg: &[u8; 32],
    ring: &[PublicKey],
    key_image: &KeyImage,
    sig: &RingSignature,
    enforce_bound: bool,
) -> Option<bool> {
    let n = ring.len();
    if !(1..=MAX_RING_LEN).contains(&n) || sig.z.len() != n {
        return None;
    }
    let ki = unpack23(&key_image.0, M)?;
    let mut t_hat = Vec::with_capacity(n);
    for pk in ring {
        t_hat.push(ntt_all(decode_pk_polys(pk)?));
    }
    let mut zs = Vec::with_capacity(n);
    for zb in &sig.z {
        zs.push(decode_z_inner(zb, enforce_bound)?);
    }
    let ctx = ChainCtx { msg, ring_hash: ring_hash(ring), ki_bytes: &key_image.0, ki_hat: ntt_all(ki), t_hat };
    let mut ch = sig.ch0;
    for (i, zi) in zs.iter().enumerate() {
        let mut c_hat = sample_in_ball(&ch);
        c_hat.ntt();
        ch = ctx.step(i, zi, &c_hat);
    }
    Some(ch == sig.ch0)
}

impl RingSignature {
    /// `ch0 ‖ z_0 ‖ … ‖ z_{n-1}` (n known from the input's ring length).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32 + self.z.len() * Z_LEN);
        out.extend_from_slice(&self.ch0);
        for z in &self.z {
            out.extend_from_slice(z);
        }
        out
    }
    pub fn from_bytes(b: &[u8], ring_len: usize) -> Result<Self> {
        if !(1..=MAX_RING_LEN).contains(&ring_len) || b.len() != 32 + ring_len * Z_LEN {
            return Err(CryptoError::Malformed);
        }
        let mut ch0 = [0u8; 32];
        ch0.copy_from_slice(&b[..32]);
        let mut z = Vec::with_capacity(ring_len);
        for chunk in b[32..].chunks_exact(Z_LEN) {
            decode_z(chunk).ok_or(CryptoError::Malformed)?;
            z.push(chunk.to_vec());
        }
        Ok(RingSignature { ch0, z })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn seed(tag: u32) -> [u8; 32] {
        h256("LW1/test/seed", &[&tag.to_le_bytes()])
    }

    /// `n` one-time secrets (each from its own spend key) and their public keys.
    fn make_ring(n: usize, tag: u32) -> (Vec<OneTimeSecret>, Vec<PublicKey>) {
        let mut secrets = Vec::new();
        let mut ring = Vec::new();
        for i in 0..n {
            let sp = SpendSecret::from_seed(&seed(tag * 1000 + i as u32));
            let ss = seed(tag * 1000 + 500 + i as u32);
            let ot = derive_onetime_secret(&sp, &ss, i as u32);
            ring.push(ot.public());
            secrets.push(ot);
        }
        (secrets, ring)
    }

    fn coeffs_mod_q_small(v: &PolyVec, eta: i32) -> bool {
        v.0.iter().flat_map(|p| p.0.iter()).all(|&c| center(c).abs() <= eta)
    }

    #[test]
    fn sample_small_range_and_distribution() {
        let s = sample_small(&seed(1));
        assert_eq!(s.0.len(), L);
        assert!(coeffs_mod_q_small(&s, 2));
        let mut hist = [0usize; 5];
        for c in s.0.iter().flat_map(|p| p.0.iter()) {
            hist[(center(*c) + 2) as usize] += 1;
        }
        for h in hist {
            // 3840/5 = 768 expected each.
            assert!((600..940).contains(&h), "{hist:?}");
        }
        assert_ne!(sample_small(&seed(1)), sample_small(&seed(2)));
    }

    #[test]
    fn sample_in_ball_weight() {
        for t in 0..50u32 {
            let c = sample_in_ball(&seed(t));
            let nz = c.0.iter().filter(|&&x| x != 0).count();
            assert_eq!(nz, TAU);
            assert!(c.0.iter().all(|&x| x == 0 || x == 1 || x == Q - 1));
        }
    }

    #[test]
    fn keys_deterministic_and_onetime_consistent() {
        let sp = SpendSecret::from_seed(&seed(7));
        let sp2 = SpendSecret::from_seed(&seed(7));
        assert_eq!(sp.public(), sp2.public());
        assert_ne!(sp.public(), SpendSecret::from_seed(&seed(8)).public());
        let pk = sp.public();
        assert_eq!(pk.0.len(), PUBLIC_KEY_LEN);
        assert!(PublicKey::from_bytes(&pk.0).is_ok());
        for j in 0..5u32 {
            let ss = seed(100 + j);
            let ot = derive_onetime_secret(&sp, &ss, j);
            assert!(coeffs_mod_q_small(&ot.s, ETA));
            assert_eq!(derive_onetime_public(&pk, &ss, j), ot.public());
            assert_ne!(ot.public(), pk);
            // Different index ⇒ different key and image.
            let ot2 = derive_onetime_secret(&sp, &ss, j + 1);
            assert_ne!(ot.public(), ot2.public());
            assert_ne!(ot.key_image(), ot2.key_image());
            assert_eq!(ot.key_image().0.len(), KEY_IMAGE_LEN);
            assert_eq!(ot.key_image(), derive_onetime_secret(&sp2, &ss, j).key_image());
        }
    }

    #[test]
    fn decoders_reject_bad_input() {
        let pk = SpendSecret::from_seed(&seed(9)).public();
        assert!(PublicKey::from_bytes(&pk.0[..PUBLIC_KEY_LEN - 1]).is_err());
        assert!(PublicKey::from_bytes(&[pk.0.clone(), vec![0]].concat()).is_err());
        assert!(PublicKey::from_bytes(&[0xff; PUBLIC_KEY_LEN]).is_err());
        assert!(KeyImage::from_bytes(&[0xff; KEY_IMAGE_LEN]).is_err());
        assert!(KeyImage::from_bytes(&[]).is_err());
        assert!(KeyImage::from_bytes(&[0; KEY_IMAGE_LEN]).is_ok());

        // z: roundtrip, extremes, out-of-range.
        let mut z = vec![[0i32; N]; L];
        z[0][0] = Z_BOUND - 1;
        z[14][255] = -(Z_BOUND - 1);
        z[3][7] = -12345;
        let b = encode_z(&z);
        assert_eq!(b.len(), Z_LEN);
        assert_eq!(decode_z(&b).unwrap(), z);
        for bad in [Z_BOUND, -Z_BOUND, GAMMA - 1, -GAMMA] {
            let mut zb = z.clone();
            zb[5][5] = bad;
            let enc = encode_z(&zb);
            assert!(decode_z(&enc).is_none(), "{bad}");
            assert_eq!(decode_z_inner(&enc, false).unwrap(), zb);
        }
        assert!(decode_z(&b[1..]).is_none());
        // All-ones 20-bit fields ⇒ z = γ − 1 − (2^20 − 1) = −γ: rejected.
        assert!(decode_z(&[0xff; Z_LEN]).is_none());
        assert!(decode_z(&[0x00; Z_LEN]).is_none()); // z = γ − 1

        assert!(RingSignature::from_bytes(&[0; 32], 0).is_err());
        assert!(RingSignature::from_bytes(&vec![0; 32 + 65 * Z_LEN], 65).is_err());
    }

    #[test]
    fn sign_verify_ring_sizes_and_positions() {
        let msg = seed(42);
        for &n in &[1usize, 2, 3, 11] {
            let (secrets, ring) = make_ring(n, n as u32);
            let mut positions = vec![0, n / 2, n - 1];
            positions.dedup();
            for &pi in &positions {
                let sig = sign(&msg, &ring, pi, &secrets[pi]);
                let ki = secrets[pi].key_image();
                assert!(verify(&msg, &ring, &ki, &sig), "n={n} pi={pi}");
                let bytes = sig.to_bytes();
                assert_eq!(bytes.len(), 32 + n * Z_LEN);
                let sig2 = RingSignature::from_bytes(&bytes, n).unwrap();
                assert_eq!(sig2, sig);
                assert!(verify(&msg, &ring, &ki, &sig2));
                assert!(RingSignature::from_bytes(&bytes, n + 1).is_err());
                assert!(RingSignature::from_bytes(&bytes[..bytes.len() - 1], n).is_err());
            }
        }
    }

    #[test]
    fn signing_is_randomized_and_hedged() {
        let (secrets, ring) = make_ring(3, 77);
        let msg = seed(1);
        let a = sign(&msg, &ring, 1, &secrets[1]);
        let b = sign(&msg, &ring, 1, &secrets[1]);
        assert_ne!(a, b);
        // With a fixed (e.g. broken) OS RNG the output is still a valid
        // signature, deterministic in (secret, μ, ring), and differs per message.
        let c1 = sign_inner(&msg, &ring, 1, &secrets[1], &[0; 32], |_, _| {});
        let c2 = sign_inner(&msg, &ring, 1, &secrets[1], &[0; 32], |_, _| {});
        let c3 = sign_inner(&seed(2), &ring, 1, &secrets[1], &[0; 32], |_, _| {});
        assert_eq!(c1, c2);
        assert_ne!(c1.ch0, c3.ch0);
        assert!(verify(&msg, &ring, &secrets[1].key_image(), &c1));
    }

    #[test]
    #[should_panic(expected = "does not match")]
    fn sign_panics_on_wrong_secret() {
        let (secrets, ring) = make_ring(3, 78);
        sign(&seed(0), &ring, 0, &secrets[1]);
    }

    #[test]
    fn verify_rejects_tampering() {
        let n = 11;
        let (secrets, ring) = make_ring(n, 21);
        let pi = 4;
        let msg = seed(5);
        let ki = secrets[pi].key_image();
        let sig = sign(&msg, &ring, pi, &secrets[pi]);
        assert!(verify(&msg, &ring, &ki, &sig));

        // Different message.
        assert!(!verify(&seed(6), &ring, &ki, &sig));
        // A ring member swapped for an outsider (signer and non-signer slots).
        let (others, _) = make_ring(1, 99);
        for slot in [pi, 0, n - 1] {
            let mut r2 = ring.clone();
            r2[slot] = others[0].public();
            assert!(!verify(&msg, &r2, &ki, &sig), "slot {slot}");
        }
        // Ring reordered.
        let mut r3 = ring.clone();
        r3.swap(1, 2);
        assert!(!verify(&msg, &r3, &ki, &sig));
        let mut r4 = ring.clone();
        r4.rotate_left(1);
        assert!(!verify(&msg, &r4, &ki, &sig));
        // Ring with a member dropped / added.
        assert!(!verify(&msg, &ring[..n - 1], &ki, &sig));
        // Wrong key images.
        assert!(!verify(&msg, &ring, &secrets[0].key_image(), &sig));
        assert!(!verify(&msg, &ring, &others[0].key_image(), &sig));
        let mut ki_bad = ki.clone();
        ki_bad.0[100] ^= 1;
        assert!(!verify(&msg, &ring, &ki_bad, &sig));
        assert!(!verify(&msg, &ring, &KeyImage(vec![0xff; KEY_IMAGE_LEN]), &sig));
        // ch0 flipped.
        for bit in [0usize, 77, 255] {
            let mut s2 = sig.clone();
            s2.ch0[bit / 8] ^= 1 << (bit % 8);
            assert!(!verify(&msg, &ring, &ki, &s2));
        }
        // Any z bit flipped (every entry, several positions incl. signer's).
        for i in 0..n {
            for pos in [0usize, 1234, Z_LEN - 1] {
                let mut s2 = sig.clone();
                s2.z[i][pos] ^= 0x01;
                assert!(!verify(&msg, &ring, &ki, &s2), "z[{i}][{pos}]");
            }
        }
        // z entries swapped, truncated, missing.
        let mut s3 = sig.clone();
        s3.z.swap(0, 1);
        assert!(!verify(&msg, &ring, &ki, &s3));
        let mut s4 = sig.clone();
        s4.z[3].pop();
        assert!(!verify(&msg, &ring, &ki, &s4));
        let mut s5 = sig.clone();
        s5.z.pop();
        assert!(!verify(&msg, &ring, &ki, &s5));
        // Empty ring.
        assert!(!verify(&msg, &[], &ki, &RingSignature { ch0: sig.ch0, z: vec![] }));
    }

    #[test]
    fn verify_rejects_out_of_bound_z_even_if_chain_closes() {
        let (secrets, ring) = make_ring(3, 31);
        let msg = seed(3);
        let ki = secrets[0].key_image();
        for big in [Z_BOUND, -Z_BOUND, GAMMA - 1] {
            // Simulated member 2 gets one coefficient at |z| ≥ γ − β; the
            // chain is computed over the tampered value so it still closes.
            let sig = sign_inner(&msg, &ring, 0, &secrets[0], &[1; 32], |i, z| {
                if i == 2 {
                    z[7][9] = big;
                }
            });
            assert_eq!(verify_inner(&msg, &ring, &ki, &sig, false), Some(true), "chain must close");
            assert!(!verify(&msg, &ring, &ki, &sig));
            assert!(RingSignature::from_bytes(&sig.to_bytes(), 3).is_err());
        }
    }

    #[test]
    fn linkability() {
        let (secrets, ring_a) = make_ring(4, 41);
        let (_, mut ring_b) = make_ring(5, 42);
        ring_b[3] = ring_a[2].clone();
        let s = &secrets[2];
        let sig_a = sign(&seed(10), &ring_a, 2, s);
        let sig_b = sign(&seed(11), &ring_b, 3, s);
        let ki = s.key_image();
        assert!(verify(&seed(10), &ring_a, &ki, &sig_a));
        assert!(verify(&seed(11), &ring_b, &ki, &sig_b));
        // The only key image either verifies under is the same one.
        assert!(!verify(&seed(10), &ring_a, &secrets[1].key_image(), &sig_a));
        // Different secrets ⇒ different images.
        let images: Vec<_> = secrets.iter().map(|s| s.key_image().id()).collect();
        for i in 0..images.len() {
            for j in i + 1..images.len() {
                assert_ne!(images[i], images[j]);
            }
        }
    }

    /// Documents DESIGN §5.5 "sender spend-tracing": a sender who knows two
    /// offsets δ1, δ2 for the same recipient can link the two key images.
    #[test]
    fn known_issue_sender_can_link_key_images() {
        let sp = SpendSecret::from_seed(&seed(300));
        let ss = seed(301);
        let i1 = unpack23(&derive_onetime_secret(&sp, &ss, 0).key_image().0, M).unwrap();
        let i2 = unpack23(&derive_onetime_secret(&sp, &ss, 1).key_image().0, M).unwrap();
        let bd1 = mat_apply(matrix_b(), &onetime_offset(&ss, 0));
        let bd2 = mat_apply(matrix_b(), &onetime_offset(&ss, 1));
        let x1: Vec<Poly> = i1.iter().zip(&bd1).map(|(a, b)| a.sub(b)).collect();
        let x2: Vec<Poly> = i2.iter().zip(&bd2).map(|(a, b)| a.sub(b)).collect();
        assert_eq!(x1, x2, "if this ever fails, the key image construction changed: update DESIGN §5.5");
    }

    #[test]
    fn statistical_signer_indistinguishable_by_norm() {
        let (secrets, ring) = make_ring(2, 51);
        let sigs = 300;
        let (mut sum_sig, mut sum_other) = (0f64, 0f64);
        let mut max_sig = 0i32;
        for k in 0..sigs {
            let pi = k % 2;
            let sig = sign(&seed(1000 + k as u32), &ring, pi, &secrets[pi]);
            let zs = decode_z(&sig.z[pi]).expect("valid");
            let zo = decode_z(&sig.z[1 - pi]).expect("valid");
            for (a, b) in zs.iter().flatten().zip(zo.iter().flatten()) {
                sum_sig += a.abs() as f64;
                sum_other += b.abs() as f64;
                max_sig = max_sig.max(a.abs());
            }
        }
        assert!(max_sig < Z_BOUND);
        let cnt = (sigs * L * N) as f64;
        let (m_sig, m_other) = (sum_sig / cnt, sum_other / cnt);
        let expect = Z_BOUND as f64 / 2.0;
        assert!((m_sig / m_other - 1.0).abs() < 0.01, "{m_sig} vs {m_other}");
        assert!((m_sig / expect - 1.0).abs() < 0.01, "{m_sig} vs {expect}");
    }

    #[test]
    fn timing_ring_11() {
        let (secrets, ring) = make_ring(RING_SIZE, 61);
        let msg = seed(9);
        let ki = secrets[5].key_image();
        let _ = sign(&msg, &ring, 5, &secrets[5]); // warm caches (matrix expansion)
        let iters = 10u32;
        let t0 = Instant::now();
        let mut sigs = Vec::new();
        for _ in 0..iters {
            sigs.push(sign(&msg, &ring, 5, &secrets[5]));
        }
        let ts = t0.elapsed() / iters;
        let t1 = Instant::now();
        for s in &sigs {
            assert!(verify(&msg, &ring, &ki, s));
        }
        let tv = t1.elapsed() / iters;
        println!(
            "LRS ring-11: sign {:?}/sig, verify {:?}/sig, signature {} B",
            ts,
            tv,
            sigs[0].to_bytes().len()
        );
    }
}
