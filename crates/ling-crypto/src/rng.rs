//! The crate's single entropy abstraction.
//!
//! Every randomized primitive in `ling-crypto` — AEAD key/nonce generation,
//! Ed25519 / X25519 / Ristretto secrets, ML-KEM / ML-DSA seeds, the ML-KEM
//! encapsulation message, Shamir coefficients, the AONT session key — draws its
//! bytes from [`fill`] here rather than reaching for `OsRng` directly. That is
//! what makes the crate portable:
//!
//! - **`std` (default)** — if no source is installed, [`fill`] falls back to the
//!   OS CSPRNG (`rand::rngs::OsRng`), so behaviour is unchanged for the runtime.
//! - **`no_std`** (e.g. the LingOS kernel) — there is no OS, so the embedder
//!   MUST install a source once at boot with [`set_entropy_source`] (the kernel
//!   points it at RDRAND). Using randomized crypto before that panics rather than
//!   silently producing predictable keys.
//!
//! Deterministic APIs (`*::from_seed`, `from_bytes`, hashing, `MobiusHelixReactor`,
//! decapsulation, verification) never touch this module and work everywhere with
//! no setup.

use core::sync::atomic::{AtomicUsize, Ordering};

/// An entropy callback: fill the buffer with cryptographically secure bytes.
pub type EntropyFn = fn(&mut [u8]);

/// Holds the installed [`EntropyFn`] as a raw address (0 = none installed).
static ENTROPY: AtomicUsize = AtomicUsize::new(0);

/// Install the process/kernel-wide entropy source. Call once, early.
///
/// In the LingOS kernel this is wired to the RDRAND-backed CSPRNG. Under `std`
/// it is optional (the OS CSPRNG is the default), but may be used to route all
/// crypto randomness through a custom source.
pub fn set_entropy_source(f: EntropyFn) {
    ENTROPY.store(f as usize, Ordering::SeqCst);
}

/// Fill `buf` with secure random bytes from the installed source.
///
/// Falls back to the OS CSPRNG under `std`; panics under `no_std` if no source
/// has been installed (fail-closed — never returns predictable bytes).
pub fn fill(buf: &mut [u8]) {
    let p = ENTROPY.load(Ordering::SeqCst);
    if p != 0 {
        // SAFETY: `ENTROPY` is only ever written in `set_entropy_source`, and
        // only from a valid `EntropyFn`; a function pointer and `usize` are the
        // same width on every target this crate builds for.
        let f: EntropyFn = unsafe { core::mem::transmute::<usize, EntropyFn>(p) };
        f(buf);
        return;
    }
    #[cfg(feature = "std")]
    {
        default_std_fill(buf);
    }
    #[cfg(not(feature = "std"))]
    {
        panic!(
            "ling-crypto: no entropy source installed — call \
             ling_crypto::rng::set_entropy_source() before using randomized crypto"
        );
    }
}

#[cfg(feature = "std")]
fn default_std_fill(buf: &mut [u8]) {
    use rand::RngCore;
    rand::rngs::OsRng.fill_bytes(buf);
}

/// Return `N` fresh secure random bytes.
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    fill(&mut out);
    out
}

/// A fresh random `u32`.
pub fn next_u32() -> u32 {
    u32::from_le_bytes(random_bytes::<4>())
}

/// A fresh random byte.
pub fn next_u8() -> u8 {
    random_bytes::<1>()[0]
}

/// Zero-sized convenience handle mirroring the classic `Rng::fill_bytes` shape.
pub struct LingRng;

impl LingRng {
    /// Fill `buf` with secure random bytes (see [`fill`]).
    pub fn fill_bytes(buf: &mut [u8]) {
        fill(buf)
    }
}
