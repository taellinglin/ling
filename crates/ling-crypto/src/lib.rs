//! Ling cryptography — classical + post-quantum primitives.
//!
//! # Modules
//! - [`hash`] — BLAKE3, SHA3-256/512, SHAKE-256
//! - [`symmetric`] — AES-256-GCM, XChaCha20-Poly1305
//! - [`asymmetric`] — Ed25519 signatures, X25519 ECDH
//! - [`kdf`] — Argon2id, HKDF-SHA3
//! - [`pq`] — ML-KEM-768 (post-quantum KEM, FIPS 203) — real implementation
//! - [`pq_sig`] — ML-DSA-65 (post-quantum signatures, FIPS 204)
//! - [`hybrid`] — X25519 + ML-KEM-768 hybrid KEM (the PQ-migration primitive)
//! - [`geo`] — Geometric suite: knot identities (PQ KEM), 3-D knot key
//!   fingerprints, and a 4-D holographic all-or-nothing transform
//! - [`shamir`] — Shamir's Secret Sharing over GF(2⁸)
//! - [`zkp`] — Schnorr zero-knowledge proof of knowledge
//! - [`vrf`] — Verifiable Random Function (Ed25519-based)
//! - [`mandala`] — Mandala Hash — custom geometric key derivation

pub mod asymmetric;
pub mod geo;
pub mod hash;
pub mod hybrid;
pub mod kdf;
pub mod mandala;
pub mod pq;
pub mod pq_sig;
pub mod shamir;
pub mod symmetric;
pub mod vrf;
pub mod zkp;

pub use asymmetric::{Ed25519Keypair, X25519Secret};
pub use geo::{
    gather, holo_hash, holo_open, holo_seal, knot_encapsulate, scatter, HoloFragment, KnotIdentity,
    KnotShape,
};
pub use hash::{Blake3, Sha3_256, Sha3_512, Shake256};
pub use hybrid::{encapsulate as hybrid_encapsulate, HybridKeypair};
pub use kdf::{hkdf_sha3, Argon2idParams};
pub use mandala::{MandalaHash, MandalaParams};
pub use pq::{encapsulate as mlkem768_encapsulate, MlKem768Keypair};
pub use pq_sig::MlDsa65Keypair;
pub use shamir::{reconstruct_secret, split_secret, Share};
pub use symmetric::{AesGcm256, XChaCha20};
pub use vrf::{VrfKeypair, VrfProof};
pub use zkp::{SchnorrKeypair, SchnorrProof};
