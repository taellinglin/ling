//! ML-DSA-87 (FIPS 204) identity keys, wrapping `ling_crypto::MlDsa87Keypair`.
//!
//! Every signature is over `H("LW1/sig/" + context, msg)` so a signature made
//! for one purpose (address bundle, seed list, P2P handshake, receipt) can
//! never be replayed as another.

use crate::pq_sig::MlDsa87Keypair;
use zeroize::Zeroizing;

use crate::lw::hash::h256;
use crate::lw::kdf::random_bytes;
use crate::lw::{CryptoError, Result};

pub const PUBLIC_KEY_LEN: usize = 2592;
pub const SIGNATURE_LEN: usize = 4627;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct IdentityPublic(pub Vec<u8>); // exactly PUBLIC_KEY_LEN

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct IdentitySignature(pub Vec<u8>); // exactly SIGNATURE_LEN

/// Signing key. Only the 32-byte seed is kept, zeroized on drop; the
/// expanded key is rebuilt per signature (ML-DSA keygen is ~0.1 ms).
pub struct IdentityKey {
    seed: Zeroizing<[u8; 32]>,
    public: IdentityPublic,
}

fn digest(context: &str, msg: &[u8]) -> [u8; 32] {
    h256(&format!("LW1/sig/{context}"), &[msg])
}

impl IdentityKey {
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        let public = IdentityPublic(MlDsa87Keypair::from_seed(*seed).public_key());
        IdentityKey { seed: Zeroizing::new(*seed), public }
    }
    pub fn generate() -> Self {
        let mut s = Zeroizing::new([0u8; 32]);
        random_bytes(s.as_mut());
        Self::from_seed(&s)
    }
    /// The 32-byte seed, for persisting a node identity (caller must protect it).
    pub fn seed(&self) -> Zeroizing<[u8; 32]> {
        self.seed.clone()
    }
    pub fn public(&self) -> IdentityPublic {
        self.public.clone()
    }
    pub fn sign(&self, context: &str, msg: &[u8]) -> IdentitySignature {
        IdentitySignature(MlDsa87Keypair::from_seed(*self.seed).sign(&digest(context, msg)))
    }
}

impl IdentityPublic {
    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        if b.len() != PUBLIC_KEY_LEN {
            return Err(CryptoError::Malformed);
        }
        Ok(IdentityPublic(b.to_vec()))
    }
    pub fn verify(&self, context: &str, msg: &[u8], sig: &IdentitySignature) -> bool {
        sig.0.len() == SIGNATURE_LEN && MlDsa87Keypair::verify(&self.0, &digest(context, msg), &sig.0).is_ok()
    }
    /// Short human-comparable id: first 8 bytes of `H("LW1/identity/fp", pk)` as hex.
    pub fn fingerprint(&self) -> String {
        let h = h256("LW1/identity/fp", &[&self.0]);
        format!("{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}", h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7])
    }
}

impl IdentitySignature {
    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        if b.len() != SIGNATURE_LEN {
            return Err(CryptoError::Malformed);
        }
        Ok(IdentitySignature(b.to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_verify_context_bound() {
        let k = IdentityKey::from_seed(&[4u8; 32]);
        let pk = k.public();
        assert_eq!(pk.0.len(), PUBLIC_KEY_LEN);
        let sig = k.sign("bundle", b"msg");
        assert_eq!(sig.0.len(), SIGNATURE_LEN);
        assert!(pk.verify("bundle", b"msg", &sig));
        assert!(!pk.verify("seeds", b"msg", &sig), "context separation");
        assert!(!pk.verify("bundle", b"msh", &sig));
        assert!(!IdentityKey::generate().public().verify("bundle", b"msg", &sig));
        assert_eq!(pk.fingerprint().len(), 19);
    }
}
