//! Hybrid post-quantum key encapsulation: **X25519 + ML-KEM-768**.
//!
//! This is the real-world "2030 migration" primitive. During the transition to
//! post-quantum cryptography nobody fully trusts a brand-new lattice scheme on
//! its own, and nobody wants to keep relying solely on X25519 once large quantum
//! computers exist. The answer the whole industry converged on (TLS 1.3's
//! `X25519MLKEM768`, Signal's PQXDH, the IETF X-Wing draft) is a **hybrid**: run
//! both KEMs and combine their shared secrets so the result is secure as long as
//! **at least one** of the two is unbroken.
//!
//! - X25519 protects against *classical* attackers today (battle-tested ECDH).
//! - ML-KEM-768 protects against a future *quantum* attacker who has recorded
//!   today's traffic ("harvest now, decrypt later").
//!
//! ## Construction (X-Wing-inspired)
//! ```text
//! public key  = X25519_pk ‖ MLKEM_ek                      (32 + 1184 = 1216 B)
//! ciphertext  = X25519_eph_pk ‖ MLKEM_ct                  (32 + 1088 = 1120 B)
//! shared key  = SHA3-256(LABEL ‖ ss_mlkem ‖ ss_x25519 ‖ eph_pk ‖ recipient_pk)
//! ```
//! The combiner **hashes both shared secrets together** (not XOR) and binds the
//! two X25519 public values, which gives IND-CCA2 security under the standard
//! hybrid KEM combiner assumption.

use crate::pq::{self, MlKem768Keypair};
use sha3::{Digest, Sha3_256};
use x25519_dalek::{PublicKey, StaticSecret};
use rand::rngs::OsRng;
use zeroize::Zeroize;

const COMBINER_LABEL: &[u8] = b"ling-hybrid-x25519-mlkem768-v1";

const X25519_LEN: usize = 32;
/// Hybrid public key length: X25519 pk + ML-KEM-768 ek.
pub const PUBLIC_KEY_LEN: usize = X25519_LEN + pq::ENCAPS_KEY_LEN; // 1216
/// Hybrid ciphertext length: X25519 ephemeral pk + ML-KEM-768 ct.
pub const CIPHERTEXT_LEN: usize = X25519_LEN + pq::CIPHERTEXT_LEN; // 1120
/// Established shared-secret length.
pub const SHARED_SECRET_LEN: usize = 32;

/// Mix two component shared secrets and the bound transcript into one key.
fn combine(ss_mlkem: &[u8], ss_x25519: &[u8], eph_pk: &[u8], recipient_pk: &[u8]) -> [u8; 32] {
    let mut h = Sha3_256::new();
    h.update(COMBINER_LABEL);
    h.update(ss_mlkem);
    h.update(ss_x25519);
    h.update(eph_pk);
    h.update(recipient_pk);
    h.finalize().into()
}

/// A hybrid keypair holding both an X25519 static secret and an ML-KEM keypair.
pub struct HybridKeypair {
    x25519_secret: StaticSecret,
    x25519_public: [u8; X25519_LEN],
    mlkem: MlKem768Keypair,
}

impl HybridKeypair {
    /// Generate a fresh hybrid keypair from the system CSPRNG.
    pub fn generate() -> Self {
        let x25519_secret = StaticSecret::random_from_rng(OsRng);
        let x25519_public = PublicKey::from(&x25519_secret).to_bytes();
        Self { x25519_secret, x25519_public, mlkem: MlKem768Keypair::generate() }
    }

    /// The hybrid public key to publish: `X25519_pk ‖ MLKEM_ek` (1216 bytes).
    pub fn public_key(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(PUBLIC_KEY_LEN);
        out.extend_from_slice(&self.x25519_public);
        out.extend_from_slice(&self.mlkem.encapsulation_key());
        out
    }

    /// Decapsulate a hybrid ciphertext to recover the shared secret.
    pub fn decapsulate(&self, ciphertext: &[u8]) -> Result<[u8; SHARED_SECRET_LEN], &'static str> {
        if ciphertext.len() != CIPHERTEXT_LEN {
            return Err("hybrid ciphertext wrong length");
        }
        let (eph_pk_bytes, ct_pq) = ciphertext.split_at(X25519_LEN);
        let mut eph_arr = [0u8; X25519_LEN];
        eph_arr.copy_from_slice(eph_pk_bytes);

        // X25519 leg: DH between our static secret and the sender's ephemeral pk.
        let eph_pk = PublicKey::from(eph_arr);
        let mut ss_x = self.x25519_secret.diffie_hellman(&eph_pk).to_bytes();

        // ML-KEM leg.
        let ss_pq = self.mlkem.decapsulate(ct_pq)?;

        let out = combine(&ss_pq, &ss_x, eph_pk_bytes, &self.x25519_public);
        ss_x.zeroize();
        Ok(out)
    }
}

/// Encapsulate to a peer's hybrid public key.
///
/// Returns `(ciphertext, shared_secret)`. Send the ciphertext to the peer; both
/// sides then share `shared_secret`.
pub fn encapsulate(hybrid_public_key: &[u8]) -> Result<(Vec<u8>, [u8; SHARED_SECRET_LEN]), &'static str> {
    if hybrid_public_key.len() != PUBLIC_KEY_LEN {
        return Err("hybrid public key wrong length");
    }
    let (x_pk_bytes, mlkem_ek) = hybrid_public_key.split_at(X25519_LEN);
    let mut x_pk_arr = [0u8; X25519_LEN];
    x_pk_arr.copy_from_slice(x_pk_bytes);

    // X25519 leg: ephemeral DH against the recipient's static X25519 key.
    let eph_secret = StaticSecret::random_from_rng(OsRng);
    let eph_public = PublicKey::from(&eph_secret).to_bytes();
    let mut ss_x = eph_secret.diffie_hellman(&PublicKey::from(x_pk_arr)).to_bytes();

    // ML-KEM leg.
    let (ct_pq, ss_pq) = pq::encapsulate(mlkem_ek)?;

    let shared = combine(&ss_pq, &ss_x, &eph_public, x_pk_bytes);
    ss_x.zeroize();

    let mut ciphertext = Vec::with_capacity(CIPHERTEXT_LEN);
    ciphertext.extend_from_slice(&eph_public);
    ciphertext.extend_from_slice(&ct_pq);
    Ok((ciphertext, shared))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let kp = HybridKeypair::generate();
        let pk = kp.public_key();
        assert_eq!(pk.len(), PUBLIC_KEY_LEN);

        let (ct, ss_send) = encapsulate(&pk).expect("encapsulate");
        assert_eq!(ct.len(), CIPHERTEXT_LEN);

        let ss_recv = kp.decapsulate(&ct).expect("decapsulate");
        assert_eq!(ss_send, ss_recv, "hybrid shared secrets agree");
    }

    #[test]
    fn distinct_encapsulations_differ() {
        let kp = HybridKeypair::generate();
        let pk = kp.public_key();
        let (_, a) = encapsulate(&pk).unwrap();
        let (_, b) = encapsulate(&pk).unwrap();
        assert_ne!(a, b, "fresh randomness yields distinct shared secrets");
    }

    #[test]
    fn tampered_ciphertext_changes_secret() {
        // Hybrid binds the transcript: flipping the ML-KEM ciphertext (implicit
        // rejection) yields a different secret on the recipient side, so the two
        // sides no longer agree.
        let kp = HybridKeypair::generate();
        let pk = kp.public_key();
        let (mut ct, ss_send) = encapsulate(&pk).unwrap();
        let last = ct.len() - 1;
        ct[last] ^= 0xFF;
        let ss_recv = kp.decapsulate(&ct).unwrap();
        assert_ne!(ss_send, ss_recv);
    }

    #[test]
    fn wrong_lengths_rejected() {
        let kp = HybridKeypair::generate();
        assert!(encapsulate(&[0u8; 10]).is_err());
        assert!(kp.decapsulate(&[0u8; 10]).is_err());
    }
}
