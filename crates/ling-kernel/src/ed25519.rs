//! Ed25519 signature *verification* for the kernel — no signing, no key
//! generation. Both of those need a real CSPRNG (RFC 8032 signing is
//! deterministic and doesn't strictly need one per-signature, but generating
//! the private key in the first place does), and this kernel doesn't have
//! one: `users.rs` currently seeds its password salt from `rdtsc()`, which
//! its own doc comment already flags as not good enough for secret key
//! material. Verification needs no secrets and no randomness at all, so it's
//! safe to expose today; signing/keygen are future work once there's a real
//! entropy source (see `packages/README.md`'s no_std `ling-crypto` port
//! item, which this is the first slice of).
//!
//! This wraps `ed25519-dalek` (audited, the same crate `ling-crypto::Ed25519Keypair`
//! uses in userland) rather than hand-rolling curve arithmetic — confirmed by
//! an actual `build-std=core,compiler_builtins` probe build that it compiles
//! with neither `alloc` nor a global allocator, with `default-features =
//! false` and the `curve25519_dalek_backend="serial"` cfg (see the kernel
//! build's rustflags in `src/main.rs`) forcing its portable, non-SIMD
//! backend — the default x86_64 backend emits AVX2 codegen a freestanding
//! target can't lower.

use ed25519_dalek::{Signature, Verifier, VerifyingKey};

/// `true` iff `sig` is a valid Ed25519 signature over `msg` by the holder of
/// `pubkey`. `false` on any malformed input (wrong-length key/signature,
/// non-canonical point encoding, etc.) as well as a genuine verification
/// failure — callers can't distinguish "malformed" from "wrong", which is
/// the same as every other verify-style builtin in this codebase
/// (`users::verify`, `totp_check`).
pub fn verify(pubkey: &[u8], msg: &[u8], sig: &[u8]) -> bool {
    let (Ok(pk), Ok(sig)) = (
        <[u8; 32]>::try_from(pubkey),
        <[u8; 64]>::try_from(sig),
    ) else {
        return false;
    };
    let Ok(vk) = VerifyingKey::from_bytes(&pk) else { return false };
    let signature = Signature::from_bytes(&sig);
    vk.verify(msg, &signature).is_ok()
}
