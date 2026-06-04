//! Post-quantum cryptography: **ML-KEM-768** (FIPS 203), the NIST-standardized
//! Module-Lattice Key Encapsulation Mechanism (formerly CRYSTALS-Kyber).
//!
//! ML-KEM-768 targets NIST security category 3 (~AES-192): roughly 192-bit
//! classical and ~161-bit quantum security against the best known lattice
//! attacks (core-SVP). Its security rests on the Module Learning-With-Errors
//! (MLWE) problem, which has no known efficient quantum algorithm — unlike the
//! discrete-log / factoring problems behind X25519, Ed25519 and RSA, all of
//! which fall to Shor's algorithm on a cryptographically-relevant quantum
//! computer.
//!
//! This is a real implementation backed by the pure-Rust [`ml_kem`] crate
//! (the RustCrypto formal-verification-adjacent implementation), not a stub.
//!
//! ## Wire sizes (ML-KEM-768)
//! | Object | Bytes |
//! |--------|------:|
//! | seed (private)        | 64   |
//! | encapsulation key (pk)| 1184 |
//! | ciphertext            | 1088 |
//! | shared secret         | 32   |
//!
//! ## Example
//! ```
//! use ling_crypto::pq::{MlKem768Keypair, encapsulate};
//! let kp = MlKem768Keypair::generate();
//! let ek = kp.encapsulation_key();              // send to the sender
//! let (ct, ss_send) = encapsulate(&ek).unwrap();// sender → recipient
//! let ss_recv = kp.decapsulate(&ct).unwrap();
//! assert_eq!(ss_send, ss_recv);                 // shared 32-byte secret
//! ```

use ml_kem::{MlKem768, EncapsulationKey, DecapsulationKey, Kem, KeyExport};
use ml_kem::{Encapsulate, Decapsulate};
use ml_kem::array::Array;

/// Byte length of an ML-KEM-768 encapsulation (public) key.
pub const ENCAPS_KEY_LEN: usize = 1184;
/// Byte length of an ML-KEM-768 ciphertext.
pub const CIPHERTEXT_LEN: usize = 1088;
/// Byte length of the private seed (the preferred serialization of the key).
pub const SEED_LEN: usize = 64;
/// Byte length of the established shared secret.
pub const SHARED_SECRET_LEN: usize = 32;

/// An ML-KEM-768 keypair (decapsulation key + its encapsulation key).
///
/// The decapsulation key is kept as a library type so secret material never
/// has to be round-tripped through raw bytes unless you explicitly ask for the
/// [`seed`](Self::seed).
pub struct MlKem768Keypair {
    dk: DecapsulationKey<MlKem768>,
    ek: EncapsulationKey<MlKem768>,
}

impl MlKem768Keypair {
    /// Generate a fresh keypair from the system CSPRNG.
    pub fn generate() -> Self {
        let (dk, ek) = MlKem768::generate_keypair();
        Self { dk, ek }
    }

    /// Reconstruct a keypair from its 64-byte private seed (deterministic).
    pub fn from_seed(seed: [u8; SEED_LEN]) -> Self {
        let dk = DecapsulationKey::<MlKem768>::from_seed(Array::from(seed));
        let ek = dk.encapsulation_key().clone();
        Self { dk, ek }
    }

    /// Export the 64-byte private seed (FIPS 203's preferred key serialization).
    pub fn seed(&self) -> Option<[u8; SEED_LEN]> {
        self.dk.to_seed().map(|s| {
            let mut out = [0u8; SEED_LEN];
            out.copy_from_slice(s.as_slice());
            out
        })
    }

    /// Export the public encapsulation key (1184 bytes) to send to a peer.
    pub fn encapsulation_key(&self) -> Vec<u8> {
        self.ek.to_bytes().as_slice().to_vec()
    }

    /// Decapsulate a ciphertext to recover the shared secret.
    ///
    /// ML-KEM uses *implicit rejection*: a malformed/forged ciphertext yields a
    /// deterministic pseudo-random secret rather than an error, so this only
    /// fails on a wrong-length input.
    pub fn decapsulate(&self, ciphertext: &[u8]) -> Result<[u8; SHARED_SECRET_LEN], &'static str> {
        let ss = self.dk.decapsulate_slice(ciphertext).map_err(|_| "ciphertext wrong length")?;
        let mut out = [0u8; SHARED_SECRET_LEN];
        out.copy_from_slice(ss.as_slice());
        Ok(out)
    }
}

/// Encapsulate to a peer's encapsulation key.
///
/// Returns `(ciphertext, shared_secret)`. Send the ciphertext to the holder of
/// the matching decapsulation key; both sides then share `shared_secret`.
pub fn encapsulate(encaps_key: &[u8]) -> Result<(Vec<u8>, [u8; SHARED_SECRET_LEN]), &'static str> {
    let ek_arr = Array::try_from(encaps_key).map_err(|_| "encapsulation key wrong length")?;
    let ek = EncapsulationKey::<MlKem768>::new(&ek_arr).map_err(|_| "invalid encapsulation key")?;
    let (ct, ss) = ek.encapsulate();
    let mut secret = [0u8; SHARED_SECRET_LEN];
    secret.copy_from_slice(ss.as_slice());
    Ok((ct.as_slice().to_vec(), secret))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let kp = MlKem768Keypair::generate();
        let ek = kp.encapsulation_key();
        assert_eq!(ek.len(), ENCAPS_KEY_LEN);

        let (ct, ss_send) = encapsulate(&ek).expect("encapsulate");
        assert_eq!(ct.len(), CIPHERTEXT_LEN);

        let ss_recv = kp.decapsulate(&ct).expect("decapsulate");
        assert_eq!(ss_send, ss_recv, "both parties derive the same shared secret");
    }

    #[test]
    fn seed_is_deterministic() {
        let kp = MlKem768Keypair::generate();
        let seed = kp.seed().expect("seed");
        let kp2 = MlKem768Keypair::from_seed(seed);
        assert_eq!(kp.encapsulation_key(), kp2.encapsulation_key());
    }

    #[test]
    fn wrong_length_rejected() {
        let kp = MlKem768Keypair::generate();
        assert!(kp.decapsulate(&[0u8; 10]).is_err());
        assert!(encapsulate(&[0u8; 10]).is_err());
    }
}
