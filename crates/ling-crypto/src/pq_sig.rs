//! ML-DSA-65 (FIPS 204, ex-Dilithium) post-quantum digital signatures.
//!
//! Separate from `pq.rs` (key encapsulation) since ML-DSA is a signature
//! scheme, not a KEM. Mirrors `asymmetric::Ed25519Keypair`'s shape so the
//! two can be composed into a hybrid signature by callers.

use ml_dsa::{
    EncodedSignature, EncodedVerifyingKey, Generate, Keypair, MlDsa65, Seed, Signature, Signer,
    SigningKey, Verifier, VerifyingKey,
};
use zeroize::Zeroizing;

pub struct MlDsa65Keypair {
    signing_key: SigningKey<MlDsa65>,
}

impl MlDsa65Keypair {
    pub fn generate() -> Self {
        Self { signing_key: SigningKey::<MlDsa65>::generate() }
    }

    pub fn from_seed(seed: [u8; 32]) -> Self {
        let seed: Seed = seed.into();
        Self { signing_key: SigningKey::<MlDsa65>::from_seed(&seed) }
    }

    pub fn public_key(&self) -> Vec<u8> {
        self.signing_key.verifying_key().encode().to_vec()
    }

    pub fn sign(&self, msg: &[u8]) -> Vec<u8> {
        self.signing_key.sign(msg).encode().to_vec()
    }

    pub fn verify(pubkey: &[u8], msg: &[u8], sig: &[u8]) -> Result<(), &'static str> {
        let vk_enc =
            EncodedVerifyingKey::<MlDsa65>::try_from(pubkey).map_err(|_| "invalid pubkey")?;
        let vk = VerifyingKey::<MlDsa65>::decode(&vk_enc);

        let sig_enc = EncodedSignature::<MlDsa65>::try_from(sig).map_err(|_| "invalid signature")?;
        let signature = Signature::<MlDsa65>::decode(&sig_enc).ok_or("invalid signature")?;

        vk.verify(msg, &signature).map_err(|_| "signature invalid")
    }

    pub fn to_bytes(&self) -> Zeroizing<[u8; 32]> {
        let seed: Seed = self.signing_key.to_seed();
        Zeroizing::new(seed.into())
    }
}
