//! Ed25519 digital signatures and X25519 Diffie-Hellman key exchange.

use ed25519_dalek::{SigningKey, VerifyingKey, Signer, Verifier, Signature};
use x25519_dalek::{StaticSecret, PublicKey};
use rand::rngs::OsRng;
use zeroize::Zeroizing;

// ── Ed25519 ───────────────────────────────────────────────────────────────────

pub struct Ed25519Keypair {
    signing_key: SigningKey,
}

impl Ed25519Keypair {
    pub fn generate() -> Self {
        Self { signing_key: SigningKey::generate(&mut OsRng) }
    }

    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self { signing_key: SigningKey::from_bytes(&seed) }
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.signing_key.verifying_key().to_bytes()
    }

    pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
        self.signing_key.sign(msg).to_bytes()
    }

    pub fn verify(pubkey: &[u8; 32], msg: &[u8], sig: &[u8; 64]) -> Result<(), &'static str> {
        let vk = VerifyingKey::from_bytes(pubkey).map_err(|_| "invalid pubkey")?;
        let signature = Signature::from_bytes(sig);
        vk.verify(msg, &signature).map_err(|_| "signature invalid")
    }

    pub fn to_bytes(&self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(self.signing_key.to_bytes())
    }
}

// ── X25519 ECDH ───────────────────────────────────────────────────────────────

pub struct X25519Secret {
    secret: StaticSecret,
}

impl X25519Secret {
    pub fn generate() -> Self {
        Self { secret: StaticSecret::random_from_rng(OsRng) }
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self { secret: StaticSecret::from(bytes) }
    }

    pub fn public_key(&self) -> [u8; 32] {
        PublicKey::from(&self.secret).to_bytes()
    }

    /// Perform ECDH and return 32-byte shared secret.
    pub fn diffie_hellman(&self, their_pubkey: &[u8; 32]) -> [u8; 32] {
        let their_pk = PublicKey::from(*their_pubkey);
        self.secret.diffie_hellman(&their_pk).to_bytes()
    }
}
