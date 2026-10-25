//! ling-wallet ("LW-1") cryptography, ported byte-for-byte from the wallet's
//! reference crate `lw-crypto`. `ling-wallet/docs/DESIGN.md` §2–§5 is the
//! normative spec; every constant, domain string and byte layout here must
//! match it exactly (the unit tests pin them).
//!
//! - [`hash`]: domain-separated SHAKE256 [standard]
//! - [`kem1024`]: X25519 ‖ ML-KEM-1024 hybrid KEM [standard]
//! - [`identity`]: ML-DSA-87 signatures via [`crate::pq_sig`] [standard]
//! - [`aead`]: XChaCha20-Poly1305 with AAD [standard]
//! - [`kdf`]: Argon2id password keys [standard]
//! - [`lattice`]: arithmetic in Z_q[X]/(X^256+1), q = 8380417, with NTT
//! - [`lrs`]: LRS-87 linkable ring signature [experimental]
//!
//! The HyperTorus proof-of-work lives next door in [`crate::hypertorus`].
//! Randomness comes from [`crate::rng`]. Only built with the `std` feature
//! (the matrices are cached in `std::sync::OnceLock`).

pub mod aead;
pub mod hash;
pub mod identity;
pub mod kdf;
pub mod kem1024;
pub mod lattice;
pub mod lrs;

/// Re-exported so callers (the Ling runtime) can wipe buffers holding lw
/// secrets with the same crate the key types use.
pub use zeroize::{Zeroize, Zeroizing};

/// Every fallible operation in this module returns this error. Messages are
/// deliberately coarse so they never leak which check failed in a way an
/// attacker could use as an oracle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoError {
    Malformed,
    AuthFailed,
    BadSignature,
    Kdf,
}

impl core::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            CryptoError::Malformed => "malformed input",
            CryptoError::AuthFailed => "authentication failed",
            CryptoError::BadSignature => "invalid signature",
            CryptoError::Kdf => "key derivation failed",
        })
    }
}

impl std::error::Error for CryptoError {}

pub type Result<T> = core::result::Result<T, CryptoError>;

/// Cross-implementation pins: every expected value below was produced by the
/// wallet's reference crate `lw-crypto` (same inputs), so this port stays
/// byte-for-byte compatible with it.
#[cfg(test)]
mod compat_tests {
    use super::*;

    fn hx(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }
    fn d(b: &[u8]) -> String {
        hx(&hash::h256("LW1/test/digest", &[b]))
    }

    #[test]
    fn matches_reference_lw_crypto() {
        assert_eq!(
            hx(&hash::h256("LW1/test/vec", &[b"abc", b""])),
            "775d4915136df882fdba4c065d7abc018a3511e94185473247f7751e130bf5ab"
        );
        assert_eq!(
            hx(&hash::h256("LW1/test/vec", &[])),
            "6b27c10171ff354603acbba5ac0a7f76c046a3ccd51faa20b3177451565dca90"
        );
        assert_eq!(
            hx(&hash::xof_vec("LW1/test/vec", &[b"abc"], 64)),
            "7cd66a0b9970b42a95d49007f2c8f4becc4d48a69759f60828a5933792a4dbec\
             1dae63013e6a921a7882d8cebcd8d53343604dc0345756a55fec2bf616adeccd"
        );
        let kp = kem1024::KemSecret::from_seed(&[3u8; 96]).public();
        assert_eq!(d(&kp.0), "ebb19e2b79a81e12fb8d087196fceb4df6639e3b64d6f2b20b6ca5a77f1339ef");
        let id = identity::IdentityKey::from_seed(&[4u8; 32]).public();
        assert_eq!(d(&id.0), "e9ef2dbd790716e02bfab783a35c0655f5b95e2b833bc0ea807596cfa657f563");
        assert_eq!(id.fingerprint(), "709b-fe8e-976e-63ff");
        let sp = lrs::SpendSecret::from_seed(&[7u8; 32]);
        let spk = sp.public();
        assert_eq!(d(&spk.0), "eae81f50d6b18df5d97f0f616a4ff0483622a5c1c5953c9f26bdda98a5935159");
        let ot = lrs::derive_onetime_public(&spk, &[9u8; 32], 5);
        assert_eq!(d(&ot.0), "b4c7ed815ebd55d6cd88813f4be182ef29c9c5fa29d82318f912dc4a0645c42f");
        let os = lrs::derive_onetime_secret(&sp, &[9u8; 32], 5);
        assert_eq!(os.public(), ot);
        assert_eq!(d(&os.key_image().0), "4ca71444488f116d72986c3eed7d57e6df072328848428e27358de9a2c9b964e");
        let pw = kdf::password_key(b"pw", &[1u8; 16], kdf::PasswordParams::TEST).unwrap();
        assert_eq!(hx(&*pw), "50f5d8d4c1ce1573e0e43dbb1a09ef19dbf4b82667d3f97608f0628b542a5ec2");
        assert_eq!(
            hx(&aead::seal(&[7u8; 32], &[9u8; 24], b"hdr", b"hello")),
            "d6178ee613489d126ae1aa2985ab6f5d9c41886328"
        );
    }

    #[test]
    fn fallible_variants_never_panic() {
        let sp = lrs::SpendSecret::from_seed(&[7u8; 32]);
        let ot = lrs::derive_onetime_secret(&sp, &[9u8; 32], 0);
        let other = lrs::derive_onetime_secret(&sp, &[9u8; 32], 1);
        let ring = vec![ot.public(), other.public()];
        let msg = [1u8; 32];
        assert!(lrs::try_sign(&msg, &ring, 0, &ot).is_ok());
        assert!(lrs::try_sign(&msg, &ring, 1, &ot).is_err(), "wrong secret");
        assert!(lrs::try_sign(&msg, &ring, 2, &ot).is_err(), "index out of range");
        assert!(lrs::try_sign(&msg, &[], 0, &ot).is_err(), "empty ring");
        let bad = vec![ot.public(), lrs::PublicKey(vec![0xff; lrs::PUBLIC_KEY_LEN])];
        assert!(lrs::try_sign(&msg, &bad, 0, &ot).is_err(), "malformed member");
        assert!(lrs::try_derive_onetime_public(&lrs::PublicKey(vec![0xff; 2944]), &[0; 32], 0).is_err());
        assert!(lrs::try_derive_onetime_public(&lrs::PublicKey(vec![1; 3]), &[0; 32], 0).is_err());
        assert!(kem1024::try_encapsulate(&kem1024::KemPublic(vec![0; 5]), b"c").is_err());
        assert!(kem1024::try_encapsulate(&kem1024::KemPublic(vec![0xff; kem1024::PUBLIC_KEY_LEN]), b"c").is_err());
    }
}
