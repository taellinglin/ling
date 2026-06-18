//! Geometric post-quantum crypto suite — knot identities, holographic fingerprints,
//! and all-or-nothing "holographic" encoding.
//!
//! Designed in the spirit of China's SM2/SM3/SM4 national suite (an asymmetric
//! scheme, a hash, and a block cipher), but **post-quantum** and **geometric**:
//!
//! | SM analogue | This module | What it is |
//! |-------------|-------------|------------|
//! | SM2 (ECC)   | [`KnotIdentity`] | Hybrid X25519+ML-KEM-768 KEM whose public key is a hyperbolic knot |
//! | SM3 (hash)  | [`holo_hash`] + [`KnotShape::from_bytes`] | SHA3-256 with a 3-D knot **visual fingerprint** |
//! | SM4 (cipher)| [`holo_seal`]/[`holo_open`] + [`scatter`]/[`gather`] | XChaCha20-Poly1305 + a 4-D holographic all-or-nothing transform |
//!
//! # ⚠️ Security model — read this
//!
//! **All cryptographic hardness comes from vetted primitives**: ML-KEM-768
//! (FIPS 203), X25519, BLAKE3, SHA3-256, XChaCha20-Poly1305, and Rivest's
//! all-or-nothing package transform. The hyperbolic-knot and 4-D holographic
//! constructions are **deterministic encodings and visual fingerprints** — they
//! are *not* a new hardness assumption, and the geometry is never the secret.
//!
//! What the geometry genuinely buys you:
//! - A **human-verifiable 3-D fingerprint** of a public key (think SSH randomart
//!   / the drunken-bishop algorithm, but a renderable knot). Its security as a
//!   fingerprint reduces exactly to the collision/preimage resistance of SHA3.
//! - A themed, visual **identity** for a real post-quantum KEM.
//! - A "you need every shard" **holographic split** that is a sound AONT.

use crate::hybrid::{self, HybridKeypair};
use crate::symmetric::XChaCha20;
use sha3::{Digest, Sha3_256};

// ══════════════════════════════════════════════════════════════════════════
// SM3-analogue: holo_hash + a 3-D hyperbolic-knot visual fingerprint
// ══════════════════════════════════════════════════════════════════════════

/// Domain-separated geometric hash (SHA3-256). The cryptographic core of the
/// suite — everything visual is derived deterministically from this digest.
pub fn holo_hash(data: &[u8]) -> [u8; 32] {
    let mut h = Sha3_256::new();
    h.update(b"ling-geo-holo-hash-v1");
    h.update(data);
    h.finalize().into()
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

/// A deterministic 3-D **(p, q) torus knot** acting as a visual fingerprint of a
/// 32-byte digest. Identical digests → identical knots; a single bit flip
/// reshapes the whole curve. Render `points` as a tube/polyline to *see* a key.
#[derive(Clone, Debug, PartialEq)]
pub struct KnotShape {
    /// Longitudinal winding (coprime to `q`) — the knot is non-trivial for p,q ≥ 2.
    pub p: u32,
    /// Meridional winding.
    pub q: u32,
    /// Major torus radius.
    pub major_r: f32,
    /// Minor torus radius.
    pub minor_r: f32,
    /// A flavor-only "hyperbolic volume" label derived from the digest (decorative).
    pub volume: f32,
    /// Sampled 3-D points along the knot (closed curve).
    pub points: Vec<[f32; 3]>,
}

impl KnotShape {
    /// Number of samples along the knot curve.
    pub const SAMPLES: usize = 256;

    /// Derive a knot directly from arbitrary bytes (hashes them first).
    pub fn from_bytes(data: &[u8]) -> Self {
        Self::from_digest(&holo_hash(data))
    }

    /// Derive a knot from a 32-byte digest.
    pub fn from_digest(d: &[u8; 32]) -> Self {
        // p, q ∈ [2,17], forced coprime & distinct → a genuine torus knot.
        let mut p = 2 + (d[0] as u32 % 16);
        let mut q = 2 + (d[1] as u32 % 16);
        if p == q {
            q = 2 + ((q) % 16) + 1;
        }
        while gcd(p, q) != 1 {
            q += 1;
            if q > 18 {
                q = 2;
                p += 1;
                if p > 18 {
                    p = 2;
                }
            }
        }

        // Radii and a decorative "volume" from later digest bytes.
        let major_r = 2.0 + (d[2] as f32 / 255.0) * 1.5;
        let minor_r = 0.4 + (d[3] as f32 / 255.0) * 0.8;
        let phase = (u16::from_le_bytes([d[4], d[5]]) as f32 / 65535.0) * std::f32::consts::TAU;
        let volume =
            1.0 + (u32::from_le_bytes([d[6], d[7], d[8], d[9]]) as f32 / u32::MAX as f32) * 11.0;

        let mut points = Vec::with_capacity(Self::SAMPLES);
        for i in 0..Self::SAMPLES {
            let t = (i as f32 / Self::SAMPLES as f32) * std::f32::consts::TAU + phase;
            let qc = (q as f32 * t).cos();
            let r = major_r + minor_r * qc;
            let x = r * (p as f32 * t).cos();
            let y = r * (p as f32 * t).sin();
            let z = minor_r * (q as f32 * t).sin();
            points.push([x, y, z]);
        }
        Self { p, q, major_r, minor_r, volume, points }
    }

    /// Short human-readable fingerprint, e.g. `"knot-7_5-v8.2"`.
    pub fn label(&self) -> String {
        format!("knot-{}_{}-v{:.1}", self.p, self.q, self.volume)
    }
}

// ══════════════════════════════════════════════════════════════════════════
// SM2-analogue: KnotIdentity — a hybrid PQ KEM with a knot-shaped public key
// ══════════════════════════════════════════════════════════════════════════

/// A post-quantum identity. Under the hood it is a hybrid **X25519 + ML-KEM-768**
/// keypair (secure if either leg holds); its public key also renders as a unique
/// hyperbolic knot you can show a human to verify out-of-band.
pub struct KnotIdentity {
    inner: HybridKeypair,
    public: Vec<u8>,
}

impl KnotIdentity {
    /// Generate a fresh identity from the system CSPRNG.
    pub fn generate() -> Self {
        let inner = HybridKeypair::generate();
        let public = inner.public_key();
        Self { inner, public }
    }

    /// Raw hybrid public key bytes (X25519 pk ‖ ML-KEM ek) — share these.
    pub fn public_key(&self) -> &[u8] {
        &self.public
    }

    /// The public key's knot fingerprint — a renderable 3-D identity badge.
    pub fn public_knot(&self) -> KnotShape {
        KnotShape::from_bytes(&self.public)
    }

    /// Decapsulate a ciphertext to recover the shared secret.
    pub fn decapsulate(&self, ciphertext: &[u8]) -> Result<[u8; 32], &'static str> {
        self.inner.decapsulate(ciphertext)
    }
}

/// Encapsulate a shared secret to a knot identity's public key.
/// Returns `(ciphertext, shared_secret)`.
pub fn knot_encapsulate(public_key: &[u8]) -> Result<(Vec<u8>, [u8; 32]), &'static str> {
    hybrid::encapsulate(public_key)
}

/// The knot a peer's public key *should* produce — compare against the knot you
/// were shown to detect a man-in-the-middle (geometric key confirmation).
pub fn knot_for_public_key(public_key: &[u8]) -> KnotShape {
    KnotShape::from_bytes(public_key)
}

// ══════════════════════════════════════════════════════════════════════════
// SM4-analogue: holo_seal AEAD + a 4-D holographic all-or-nothing transform
// ══════════════════════════════════════════════════════════════════════════

/// Authenticated encryption (XChaCha20-Poly1305) under a 32-byte key — e.g. a
/// shared secret from [`knot_encapsulate`] or a key from [`KnotShape`]/mandala.
pub fn holo_seal(key: [u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, &'static str> {
    XChaCha20::new(key).encrypt(plaintext)
}

/// Inverse of [`holo_seal`].
pub fn holo_open(key: [u8; 32], ciphertext: &[u8]) -> Result<Vec<u8>, &'static str> {
    XChaCha20::new(key).decrypt(ciphertext)
}

/// One "hologram" fragment of an all-or-nothing transform. Like a real hologram,
/// no single fragment reveals any part of the original — you need every one.
#[derive(Clone, Debug, PartialEq)]
pub struct HoloFragment {
    /// Fragment index (0-based).
    pub index: u32,
    /// A 4-D coordinate on the unit 3-sphere for visualization (decorative).
    pub coord: [f32; 4],
    /// The 32-byte transformed block.
    pub block: [u8; 32],
}

const HOLO_BLOCK: usize = 32;

/// Derive a keystream block index `i` from the random session key `k`.
fn ks_block(k: &[u8; 32], i: u32) -> [u8; 32] {
    let mut h = blake3::Hasher::new_keyed(k);
    h.update(b"ling-holo-aont-v1");
    h.update(&i.to_le_bytes());
    *h.finalize().as_bytes()
}

/// **Holographic scatter** — Rivest's all-or-nothing package transform.
///
/// Splits `data` into fragments such that **all** fragments are required to
/// recover even one byte of plaintext. This is a sound AONT: a fresh random key
/// `k` masks every block via a BLAKE3 keystream, and a final *anchor* block
/// stores `k ⊕ H(all masked blocks)`. Lose any fragment and `H(...)` changes, so
/// `k` — and therefore everything — is unrecoverable.
pub fn scatter(data: &[u8]) -> Vec<HoloFragment> {
    use rand::RngCore;
    let mut k = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut k);

    // Length-prefix so we can trim padding on the way back.
    let mut msg = (data.len() as u64).to_le_bytes().to_vec();
    msg.extend_from_slice(data);
    while msg.len() % HOLO_BLOCK != 0 {
        msg.push(0);
    }
    let n = (msg.len() / HOLO_BLOCK) as u32;

    // Masked data blocks: c_i = m_i ⊕ KS_k(i)
    let mut blocks: Vec<[u8; 32]> = Vec::with_capacity(n as usize + 1);
    for i in 0..n {
        let ks = ks_block(&k, i);
        let mut c = [0u8; 32];
        for j in 0..HOLO_BLOCK {
            c[j] = msg[i as usize * HOLO_BLOCK + j] ^ ks[j];
        }
        blocks.push(c);
    }

    // Anchor block: c_n = k ⊕ H(c_0 ‖ … ‖ c_{n-1})
    let mut h = blake3::Hasher::new();
    h.update(b"ling-holo-anchor-v1");
    for c in &blocks {
        h.update(c);
    }
    let digest = *h.finalize().as_bytes();
    let mut anchor = [0u8; 32];
    for j in 0..HOLO_BLOCK {
        anchor[j] = k[j] ^ digest[j];
    }
    blocks.push(anchor);

    blocks
        .into_iter()
        .enumerate()
        .map(|(i, block)| HoloFragment {
            index: i as u32,
            coord: sphere4_point(i as u32, &block),
            block,
        })
        .collect()
}

/// **Holographic gather** — invert [`scatter`]. Returns `None` if any fragment is
/// missing/corrupt (the anchor hash won't match → no key → no plaintext).
pub fn gather(fragments: &[HoloFragment]) -> Option<Vec<u8>> {
    if fragments.len() < 2 {
        return None;
    }
    let mut frags = fragments.to_vec();
    frags.sort_by_key(|f| f.index);
    // Indices must be exactly 0..len with no gaps.
    for (i, f) in frags.iter().enumerate() {
        if f.index as usize != i {
            return None;
        }
    }
    let n = frags.len() - 1; // data blocks; last is the anchor

    // Recover k = anchor ⊕ H(c_0 ‖ … ‖ c_{n-1})
    let mut h = blake3::Hasher::new();
    h.update(b"ling-holo-anchor-v1");
    for f in &frags[..n] {
        h.update(&f.block);
    }
    let digest = *h.finalize().as_bytes();
    let mut k = [0u8; 32];
    for j in 0..HOLO_BLOCK {
        k[j] = frags[n].block[j] ^ digest[j];
    }

    // Unmask: m_i = c_i ⊕ KS_k(i)
    let mut msg = Vec::with_capacity(n * HOLO_BLOCK);
    for i in 0..n {
        let ks = ks_block(&k, i as u32);
        for j in 0..HOLO_BLOCK {
            msg.push(frags[i].block[j] ^ ks[j]);
        }
    }
    if msg.len() < 8 {
        return None;
    }
    let len = u64::from_le_bytes(msg[..8].try_into().ok()?) as usize;
    if 8 + len > msg.len() {
        return None;
    }
    Some(msg[8..8 + len].to_vec())
}

/// Map a fragment to a point on the unit 3-sphere in 4-D (visualization only).
fn sphere4_point(index: u32, block: &[u8; 32]) -> [f32; 4] {
    let a = (u16::from_le_bytes([block[0], block[1]]) as f32 / 65535.0) * std::f32::consts::PI;
    let b = (u16::from_le_bytes([block[2], block[3]]) as f32 / 65535.0) * std::f32::consts::TAU;
    let c = ((index as f32) * 0.61803398875).fract() * std::f32::consts::TAU;
    [
        a.sin() * b.cos(),
        a.sin() * b.sin(),
        a.cos() * c.cos(),
        a.cos() * c.sin(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knot_is_deterministic_and_avalanches() {
        let k1 = KnotShape::from_bytes(b"alice-public-key");
        let k2 = KnotShape::from_bytes(b"alice-public-key");
        let k3 = KnotShape::from_bytes(b"alice-public-keyX");
        assert_eq!(k1, k2, "same input → same knot");
        assert_ne!(k1.points, k3.points, "one byte change reshapes the knot");
        assert_eq!(k1.points.len(), KnotShape::SAMPLES);
        assert_eq!(gcd(k1.p, k1.q), 1, "p,q coprime → genuine torus knot");
    }

    #[test]
    fn knot_identity_kem_round_trip() {
        let id = KnotIdentity::generate();
        let pk = id.public_key().to_vec();
        // The knot a sender renders must match the recipient's own knot.
        assert_eq!(knot_for_public_key(&pk), id.public_knot());
        let (ct, ss_send) = knot_encapsulate(&pk).expect("encapsulate");
        let ss_recv = id.decapsulate(&ct).expect("decapsulate");
        assert_eq!(ss_send, ss_recv);
    }

    #[test]
    fn seal_open_round_trip() {
        let id = KnotIdentity::generate();
        let (ct, key) = knot_encapsulate(id.public_key()).unwrap();
        let sealed = holo_seal(key, b"meet at the temple at dusk").unwrap();
        let key2 = id.decapsulate(&ct).unwrap();
        let opened = holo_open(key2, &sealed).unwrap();
        assert_eq!(opened, b"meet at the temple at dusk");
    }

    #[test]
    fn holographic_aont_needs_every_fragment() {
        let secret = b"all-or-nothing holographic payload \x00\xff";
        let frags = scatter(secret);
        assert!(frags.len() >= 2);
        // Full set reconstructs.
        assert_eq!(gather(&frags).as_deref(), Some(&secret[..]));
        // Drop any one fragment → unrecoverable.
        for drop in 0..frags.len() {
            let partial: Vec<_> = frags
                .iter()
                .cloned()
                .filter(|f| f.index as usize != drop)
                .collect();
            assert!(
                gather(&partial).is_none(),
                "missing fragment {drop} must break recovery"
            );
        }
    }

    #[test]
    fn holographic_fragments_leak_nothing_individually() {
        // A single fragment's block must not equal any plaintext block.
        let secret = [0x41u8; 64]; // 'AAAA...'
        let frags = scatter(&secret);
        for f in &frags {
            assert_ne!(
                f.block, [0x41u8; 32],
                "a lone hologram fragment reveals plaintext"
            );
        }
    }
}
