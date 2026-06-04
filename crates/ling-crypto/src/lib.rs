//! Ling cryptography — classical + post-quantum primitives.
//!
//! # Modules
//! - [`hash`]      — BLAKE3, SHA3-256/512, SHAKE-256
//! - [`symmetric`] — AES-256-GCM, XChaCha20-Poly1305
//! - [`asymmetric`]— Ed25519 signatures, X25519 ECDH
//! - [`kdf`]       — Argon2id, HKDF-SHA3
//! - [`pq`]        — ML-KEM-768 (post-quantum KEM, FIPS 203) — real implementation
//! - [`hybrid`]    — X25519 + ML-KEM-768 hybrid KEM (the PQ-migration primitive)
//! - [`geo`]       — Geometric suite: knot identities (PQ KEM), 3-D knot key
//!                   fingerprints, and a 4-D holographic all-or-nothing transform
//! - [`shamir`]    — Shamir's Secret Sharing over GF(2⁸)
//! - [`zkp`]       — Schnorr zero-knowledge proof of knowledge
//! - [`vrf`]       — Verifiable Random Function (Ed25519-based)
//! - [`mandala`]   — Mandala Hash — custom geometric key derivation

pub mod hash;
pub mod symmetric;
pub mod asymmetric;
pub mod kdf;
pub mod pq;
pub mod hybrid;
pub mod geo;
pub mod shamir;
pub mod zkp;
pub mod vrf;
pub mod mandala;

pub use hash::{Blake3, Sha3_256, Sha3_512, Shake256};
pub use symmetric::{AesGcm256, XChaCha20};
pub use asymmetric::{Ed25519Keypair, X25519Secret};
pub use kdf::{Argon2idParams, hkdf_sha3};
pub use pq::{MlKem768Keypair, encapsulate as mlkem768_encapsulate};
pub use hybrid::{HybridKeypair, encapsulate as hybrid_encapsulate};
pub use geo::{KnotIdentity, KnotShape, HoloFragment, knot_encapsulate, holo_hash, holo_seal, holo_open, scatter, gather};
pub use shamir::{split_secret, reconstruct_secret, Share};
pub use zkp::{SchnorrProof, SchnorrKeypair};
pub use vrf::{VrfKeypair, VrfProof};
pub use mandala::{MandalaHash, MandalaParams};
