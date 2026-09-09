//! DICE-42 — a keyed, invertible Möbius-Helix Reactor cascade layer for
//! `lingtp://`.
//!
//! "DICE-42" is this cascade suite's proper name — use it in handshake
//! capability strings, logs, and UI wherever the layer needs to be named
//! (analogous to how "AES-256-GCM" or "ChaCha20-Poly1305" name *their*
//! constructions). See [`NAME`].
//!
//! This is **not** a replacement for the vetted primitives elsewhere in this
//! crate. The baseline security guarantee for anything built on top of this
//! module still comes from [`crate::pq_sig::MlDsa87Keypair`] (transcript
//! authentication), [`crate::hybrid`] (X25519 + ML-KEM-768 key agreement),
//! and [`crate::symmetric::XChaCha20`] (the actual AEAD). This module is
//! meant to be layered *outside* that AEAD, under an independently-derived
//! key, as a second, deterministic cascade pass — textbook Encrypt-then-Encrypt
//! with independent keys, which can only add work factor for an attacker,
//! never subtract it. Treat it as defense-in-depth and pattern-hiding, not as
//! a cryptanalytic upgrade over ML-DSA-87/ML-KEM-768 themselves.
//!
//! # The construction, geometrically
//!
//! - **The dice** — a keyed 256-entry byte substitution box (`sbox`, and its
//!   exact inverse `inv_sbox`), built with a keyed Fisher-Yates shuffle over
//!   a BLAKE3-keyed XOF stream. A [`DieType`] (Tetra/Cube/Octa/Dodeca/Icosa/
//!   Hyper — deterministically chosen from the seed) sets how many
//!   reinforcement shuffle passes run, so the "die" choice has a real effect,
//!   not just a name. A `color_seed` is folded into the whitening keystream.
//! - **The hyper-möbius-torus transformer** — every message's bytes are
//!   walked around a ring of length `n` (the message length) in a keyed
//!   stride coprime to `n` (`new_pos = (i * stride) mod n`), which is a
//!   bijection by construction — this is what makes the whole transform
//!   *lossless*. Each time the stride walk wraps around the ring, the byte
//!   being placed is bit-complemented — an odd number of half-twists flips
//!   orientation, same as a physical Möbius strip. Both the stride and the
//!   wrap parity are exactly recoverable on the decrypting side via the
//!   modular inverse of the stride (extended Euclidean algorithm).
//! - **Whitening + authentication** — a BLAKE3-keyed-XOF keystream pass runs
//!   before substitution, nonce- and color-bound; a BLAKE3 keyed MAC tag is
//!   appended after the permutation and checked in constant time
//!   ([`subtle::ConstantTimeEq`]) before anything is decrypted.
//!
//! Every sub-layer key (sbox, helix/whitening, MAC) is derived independently
//! from the reactor seed via [`crate::kdf::hkdf_sha3`] under distinct labels,
//! so a break in one sub-layer's key doesn't hand over the others.

use subtle::ConstantTimeEq;

/// The proper name of this cascade suite, for handshake capability strings,
/// logs, and UI — "the dice-in-a-möbius-torus reactor."
pub const NAME: &str = "DICE-42";

/// Which "die" shape was selected for this reactor's sbox reinforcement.
/// Purely a deterministic function of the seed — callers never choose this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DieType {
    Tetra,
    Cube,
    Octa,
    Dodeca,
    Icosa,
    Hyper,
}

impl DieType {
    fn from_byte(b: u8) -> Self {
        match b % 6 {
            0 => DieType::Tetra,
            1 => DieType::Cube,
            2 => DieType::Octa,
            3 => DieType::Dodeca,
            4 => DieType::Icosa,
            _ => DieType::Hyper,
        }
    }

    /// Number of faces the die metaphor carries (informational).
    pub fn faces(self) -> u16 {
        match self {
            DieType::Tetra => 4,
            DieType::Cube => 6,
            DieType::Octa => 8,
            DieType::Dodeca => 12,
            DieType::Icosa => 20,
            DieType::Hyper => 256,
        }
    }

    /// How many keyed reinforcement shuffle passes build the sbox.
    fn reinforcement_passes(self) -> usize {
        match self {
            DieType::Tetra | DieType::Cube => 1,
            DieType::Octa | DieType::Dodeca => 2,
            DieType::Icosa | DieType::Hyper => 3,
        }
    }
}

/// Public, informational parameters chosen deterministically from a reactor
/// seed. None of these are secret on their own (the sbox/helix/MAC keys are
/// what actually matter), but they're exposed so a handshake can log/display
/// which "reactor shape" a session negotiated.
#[derive(Debug, Clone, Copy)]
pub struct MobiusHelixParams {
    pub die_faces: u16,
    pub die_type: DieType,
    pub color_seed: [u8; 3],
}

/// A derived Möbius-Helix Reactor: an independently-keyed, invertible
/// cascade cipher meant to wrap an already-AEAD-encrypted `lingtp://` record.
pub struct MobiusHelixReactor {
    sbox: [u8; 256],
    inv_sbox: [u8; 256],
    helix_key: [u8; 32],
    mac_key: [u8; 32],
    params: MobiusHelixParams,
}

impl MobiusHelixReactor {
    /// Derive a full reactor (sbox, helix/whitening key, MAC key, and the
    /// informational dice/color parameters) from a 32-byte seed. Callers are
    /// expected to derive `reactor_seed` themselves via `hkdf_sha3` off a
    /// handshake shared secret, under a domain-separated label distinct from
    /// any other session key.
    pub fn derive(reactor_seed: &[u8; 32]) -> Self {
        let die_selector = derive32(reactor_seed, b"mobius.die");
        let die_type = DieType::from_byte(die_selector[0]);

        let color_bytes = derive32(reactor_seed, b"mobius.color");
        let color_seed = [color_bytes[0], color_bytes[1], color_bytes[2]];

        let sbox_key = derive32(reactor_seed, b"mobius.sbox");
        let sbox = keyed_permutation(
            &sbox_key,
            b"lingtp-v1 mobius sbox",
            die_type.reinforcement_passes(),
        );
        let inv_sbox = invert_permutation(&sbox);

        let helix_key = derive32(reactor_seed, b"mobius.helix");
        let mac_key = derive32(reactor_seed, b"mobius.mac");

        Self {
            sbox,
            inv_sbox,
            helix_key,
            mac_key,
            params: MobiusHelixParams { die_faces: 256, die_type, color_seed },
        }
    }

    pub fn params(&self) -> MobiusHelixParams {
        self.params
    }

    /// Seal `plaintext` under a caller-supplied 24-byte nonce (never reuse a
    /// nonce with the same reactor). Output is `nonce(24) || ciphertext(n) ||
    /// tag(32)` — always exactly 56 bytes longer than the input, and always
    /// invertible by [`Self::open`] given the same reactor.
    pub fn seal(&self, nonce: &[u8; 24], plaintext: &[u8]) -> Vec<u8> {
        let n = plaintext.len();

        let ks = keyed_xof(&self.helix_key, &[b"whiten", nonce, &self.params.color_seed], n);
        let mut stage1 = vec![0u8; n];
        for i in 0..n {
            stage1[i] = plaintext[i] ^ ks[i];
        }

        let mut stage2 = vec![0u8; n];
        for i in 0..n {
            stage2[i] = self.sbox[stage1[i] as usize];
        }

        let stride = stride_for(&self.helix_key, nonce, n);
        let mut stage3 = vec![0u8; n];
        for i in 0..n {
            let raw = i as u128 * stride as u128;
            let newpos = (raw % n as u128) as usize;
            let wraps = raw / n as u128;
            let mut b = stage2[i];
            if wraps % 2 == 1 {
                b ^= 0xFF;
            }
            stage3[newpos] = b;
        }

        let tag = mac_tag(&self.mac_key, nonce, &stage3);

        let mut out = Vec::with_capacity(24 + n + 32);
        out.extend_from_slice(nonce);
        out.extend_from_slice(&stage3);
        out.extend_from_slice(&tag);
        out
    }

    /// Invert [`Self::seal`]. Rejects (without touching the ciphertext
    /// bytes further) if the MAC tag doesn't match, i.e. the message was
    /// tampered with or sealed under a different reactor.
    pub fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, &'static str> {
        if sealed.len() < 24 + 32 {
            return Err("mobius-helix: sealed message too short");
        }
        let nonce: [u8; 24] = sealed[0..24].try_into().unwrap();
        let n = sealed.len() - 24 - 32;
        let ct = &sealed[24..24 + n];
        let tag = &sealed[24 + n..24 + n + 32];

        let expected = mac_tag(&self.mac_key, &nonce, ct);
        if expected.ct_eq(tag).unwrap_u8() != 1 {
            return Err("mobius-helix: authentication tag mismatch");
        }

        let stride = stride_for(&self.helix_key, &nonce, n);
        let stride_inv = if n <= 1 { 1 } else { mod_inverse(stride, n as u64) };

        let mut stage2 = vec![0u8; n];
        for newpos in 0..n {
            let i = ((newpos as u128 * stride_inv as u128) % n.max(1) as u128) as usize;
            let raw = i as u128 * stride as u128;
            let wraps = raw / n.max(1) as u128;
            let mut b = ct[newpos];
            if wraps % 2 == 1 {
                b ^= 0xFF;
            }
            stage2[i] = b;
        }

        let mut stage1 = vec![0u8; n];
        for j in 0..n {
            stage1[j] = self.inv_sbox[stage2[j] as usize];
        }

        let ks = keyed_xof(&self.helix_key, &[b"whiten", &nonce, &self.params.color_seed], n);
        let mut out = vec![0u8; n];
        for i in 0..n {
            out[i] = stage1[i] ^ ks[i];
        }
        Ok(out)
    }
}

// ── internals ─────────────────────────────────────────────────────────────

fn derive32(ikm: &[u8], label: &[u8]) -> [u8; 32] {
    let out = crate::kdf::hkdf_sha3(ikm, &[], label, 32)
        .expect("hkdf_sha3: fixed 32-byte output never fails");
    let mut buf = [0u8; 32];
    buf.copy_from_slice(&out);
    buf
}

fn keyed_xof(key: &[u8; 32], parts: &[&[u8]], len: usize) -> Vec<u8> {
    let mut hasher = blake3::Hasher::new_keyed(key);
    for part in parts {
        hasher.update(part);
    }
    let mut out = vec![0u8; len];
    let mut reader = hasher.finalize_xof();
    reader.fill(&mut out);
    out
}

fn mac_tag(mac_key: &[u8; 32], nonce: &[u8; 24], ciphertext: &[u8]) -> [u8; 32] {
    let mut input = Vec::with_capacity(24 + ciphertext.len());
    input.extend_from_slice(nonce);
    input.extend_from_slice(ciphertext);
    *blake3::keyed_hash(mac_key, &input).as_bytes()
}

fn keyed_permutation(key: &[u8; 32], label: &[u8], passes: usize) -> [u8; 256] {
    let mut p = [0u8; 256];
    for (i, slot) in p.iter_mut().enumerate() {
        *slot = i as u8;
    }
    for pass in 0..passes.max(1) {
        let stream = keyed_xof(key, &[label, &(pass as u32).to_le_bytes()], 255 * 4);
        let mut draw = 0usize;
        for i in (1..256).rev() {
            let off = draw * 4;
            let r = u32::from_le_bytes([
                stream[off],
                stream[off + 1],
                stream[off + 2],
                stream[off + 3],
            ]) as usize;
            let j = r % (i + 1);
            p.swap(i, j);
            draw += 1;
        }
    }
    p
}

fn invert_permutation(p: &[u8; 256]) -> [u8; 256] {
    let mut inv = [0u8; 256];
    for (i, &v) in p.iter().enumerate() {
        inv[v as usize] = i as u8;
    }
    inv
}

/// Smallest-effort keyed stride, coprime with `n`, for the helical torus walk.
fn stride_for(key: &[u8; 32], nonce: &[u8; 24], n: usize) -> u64 {
    if n <= 1 {
        return 1;
    }
    let n64 = n as u64;
    let seed = keyed_xof(key, &[b"stride", nonce, &n64.to_le_bytes()], 8);
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&seed);
    let mut s = u64::from_le_bytes(bytes) % n64;
    if s == 0 {
        s = 1;
    }
    while gcd(s, n64) != 1 {
        s += 1;
        if s >= n64 {
            s = 1;
        }
    }
    s
}

fn gcd(a: u64, b: u64) -> u64 {
    let (mut a, mut b) = (a, b);
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

/// Modular inverse of `a` mod `m` via the extended Euclidean algorithm.
/// Only ever called with `gcd(a, m) == 1` (guaranteed by [`stride_for`]).
fn mod_inverse(a: u64, m: u64) -> u64 {
    if m == 1 {
        return 0;
    }
    let (mut old_r, mut r) = (a as i128, m as i128);
    let (mut old_s, mut s) = (1i128, 0i128);
    while r != 0 {
        let q = old_r / r;
        let new_r = old_r - q * r;
        old_r = r;
        r = new_r;
        let new_s = old_s - q * s;
        old_s = s;
        s = new_s;
    }
    let m_i = m as i128;
    (((old_s % m_i) + m_i) % m_i) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reactor(seed_byte: u8) -> MobiusHelixReactor {
        MobiusHelixReactor::derive(&[seed_byte; 32])
    }

    #[test]
    fn roundtrip_various_lengths() {
        let r = reactor(1);
        for len in [0usize, 1, 2, 3, 7, 31, 255, 256, 257, 4096] {
            let plaintext: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            let nonce = [7u8; 24];
            let sealed = r.seal(&nonce, &plaintext);
            assert_eq!(sealed.len(), plaintext.len() + 24 + 32);
            let opened = r.open(&sealed).expect("open should succeed");
            assert_eq!(opened, plaintext, "roundtrip mismatch at len {len}");
        }
    }

    #[test]
    fn tamper_is_rejected() {
        let r = reactor(2);
        let nonce = [9u8; 24];
        let mut sealed = r.seal(&nonce, b"cosmic sour belts, 3x");
        let last = sealed.len() - 1;
        sealed[last] ^= 0x01;
        assert!(r.open(&sealed).is_err());

        let mut sealed2 = r.seal(&nonce, b"cosmic sour belts, 3x");
        sealed2[30] ^= 0x01;
        assert!(r.open(&sealed2).is_err());
    }

    #[test]
    fn wrong_reactor_cannot_open() {
        let a = reactor(3);
        let b = reactor(4);
        let sealed = a.seal(&[1u8; 24], b"nebula gummy bears");
        assert!(b.open(&sealed).is_err());
    }

    #[test]
    fn different_nonces_differ() {
        let r = reactor(5);
        let s1 = r.seal(&[1u8; 24], b"starlight lollipops");
        let s2 = r.seal(&[2u8; 24], b"starlight lollipops");
        assert_ne!(s1, s2);
    }

    #[test]
    fn params_deterministic() {
        let a = reactor(6);
        let b = reactor(6);
        assert_eq!(a.params().die_type, b.params().die_type);
        assert_eq!(a.params().color_seed, b.params().color_seed);
    }
}
