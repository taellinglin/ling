//! Cryptographic hashing: BLAKE3, SHA3-256, SHA3-512, SHAKE-256.

use sha3::digest::{ExtendableOutput, Update as XofUpdate};
use sha3::{Digest, Sha3_256 as Sha3_256Inner, Sha3_512 as Sha3_512Inner};

// ── BLAKE3 ────────────────────────────────────────────────────────────────────

pub struct Blake3;

impl Blake3 {
    pub fn hash(data: &[u8]) -> [u8; 32] {
        let hash = blake3::hash(data);
        *hash.as_bytes()
    }

    pub fn keyed_hash(key: &[u8; 32], data: &[u8]) -> [u8; 32] {
        let hash = blake3::keyed_hash(key, data);
        *hash.as_bytes()
    }

    /// Derive a subkey from a context string and key material.
    pub fn derive_key(context: &str, key_material: &[u8]) -> [u8; 32] {
        blake3::derive_key(context, key_material)
    }

    /// Extended output (XOF): produce `len` bytes from `data`.
    pub fn xof(data: &[u8], len: usize) -> Vec<u8> {
        let mut hasher = blake3::Hasher::new();
        hasher.update(data);
        let mut out = vec![0u8; len];
        let mut reader = hasher.finalize_xof();
        reader.fill(&mut out);
        out
    }
}

// ── SHA3-256 ──────────────────────────────────────────────────────────────────

pub struct Sha3_256;

impl Sha3_256 {
    pub fn hash(data: &[u8]) -> [u8; 32] {
        let mut h = Sha3_256Inner::new();
        Digest::update(&mut h, data);
        h.finalize().into()
    }

    pub fn hash_many(parts: &[&[u8]]) -> [u8; 32] {
        let mut h = Sha3_256Inner::new();
        for part in parts {
            Digest::update(&mut h, part);
        }
        h.finalize().into()
    }
}

// ── SHA3-512 ──────────────────────────────────────────────────────────────────

pub struct Sha3_512;

impl Sha3_512 {
    pub fn hash(data: &[u8]) -> [u8; 64] {
        let mut h = Sha3_512Inner::new();
        Digest::update(&mut h, data);
        h.finalize().into()
    }
}

// ── SHAKE-256 (XOF) ───────────────────────────────────────────────────────────

pub struct Shake256;

impl Shake256 {
    /// Produce `len` bytes of output from `data`.
    pub fn hash(data: &[u8], len: usize) -> Vec<u8> {
        let mut h = sha3::Shake256::default();
        XofUpdate::update(&mut h, data);
        let mut out = vec![0u8; len];
        h.finalize_xof_into(&mut out);
        out
    }
}
