//! Ed25519 signatures for the kernel: verification, plus (now that a real
//! CSPRNG exists) seed-based key generation and deterministic RFC 8032
//! signing. When this was verify-only the blocker was entropy -- key
//! generation needs a real CSPRNG and `users.rs`'s `rdtsc()` salt was
//! explicitly not good enough. `crypto::random_bytes` (RDRAND, see crypto.rs)
//! is that CSPRNG now, so a caller draws one 32-byte seed, persists it, and
//! derives a stable keypair from it here (`public_from_seed` / `sign`). Signing
//! itself is deterministic and needs no per-signature randomness. This is what
//! an in-kernel SSH *server* host key is built on (it must sign the key-exchange
//! hash, or a real OpenSSH client refuses the connection).
//!
//! This wraps `ed25519-dalek` (audited, the same crate `ling-crypto::Ed25519Keypair`
//! uses in userland) rather than hand-rolling curve arithmetic — confirmed by
//! an actual `build-std=core,compiler_builtins` probe build that it compiles
//! with neither `alloc` nor a global allocator, with `default-features =
//! false` and the `curve25519_dalek_backend="serial"` cfg (see the kernel
//! build's rustflags in `src/main.rs`) forcing its portable, non-SIMD
//! backend — the default x86_64 backend emits AVX2 codegen a freestanding
//! target can't lower.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

/// The Ed25519 public (verifying) key, 32 bytes, for a secret `seed`.
pub fn public_from_seed(seed: &[u8; 32]) -> [u8; 32] {
    SigningKey::from_bytes(seed).verifying_key().to_bytes()
}

/// A deterministic RFC 8032 Ed25519 signature (64 bytes) over `msg` by the key
/// derived from `seed`. The seed is the only secret; the caller sources it from
/// RDRAND once and persists it so the identity/host key stays stable.
pub fn sign(seed: &[u8; 32], msg: &[u8]) -> [u8; 64] {
    SigningKey::from_bytes(seed).sign(msg).to_bytes()
}

/// `true` iff `sig` is a valid Ed25519 signature over `msg` by the holder of
/// `pubkey`. `false` on any malformed input (wrong-length key/signature,
/// non-canonical point encoding, etc.) as well as a genuine verification
/// failure — callers can't distinguish "malformed" from "wrong", which is
/// the same as every other verify-style builtin in this codebase
/// (`users::verify`, `totp_check`).
pub fn verify(pubkey: &[u8], msg: &[u8], sig: &[u8]) -> bool {
    let (Ok(pk), Ok(sig)) = (<[u8; 32]>::try_from(pubkey), <[u8; 64]>::try_from(sig)) else {
        return false;
    };
    let Ok(vk) = VerifyingKey::from_bytes(&pk) else { return false };
    let signature = Signature::from_bytes(&sig);
    vk.verify(msg, &signature).is_ok()
}
