//! Verifiable Random Function (VRF) using Ed25519.
//!
//! A VRF produces a pseudorandom output together with a proof that the
//! output was computed correctly. Given `(pubkey, input)`, anyone can
//! verify that `output = VRF(privkey, input)` without seeing privkey.
//!
//! Construction: ECVRF-EDWARDS25519-SHA512-TAI (simplified, not full RFC 9381).

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use rand::rngs::OsRng;
use sha3::{Digest, Sha3_512};

pub struct VrfKeypair {
    signing_key: SigningKey,
}

#[derive(Clone, Debug)]
pub struct VrfProof {
    pub proof_bytes: [u8; 64], // Ed25519 signature over H(input)
    pub output: [u8; 32],      // pseudorandom output derived from signature
}

impl VrfKeypair {
    pub fn generate() -> Self {
        Self { signing_key: SigningKey::generate(&mut OsRng) }
    }

    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self { signing_key: SigningKey::from_bytes(&seed) }
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.signing_key.verifying_key().to_bytes()
    }

    /// Evaluate the VRF: sign H("vrf:"||input) and hash the signature.
    pub fn evaluate(&self, input: &[u8]) -> VrfProof {
        let h = domain_hash(input);
        let sig = self.signing_key.sign(&h);
        let proof_bytes = sig.to_bytes();
        let output = output_hash(&proof_bytes);
        VrfProof { proof_bytes, output }
    }
}

/// Verify that `proof.output` is the correct VRF output for `pubkey` + `input`.
pub fn vrf_verify(pubkey: &[u8; 32], input: &[u8], proof: &VrfProof) -> bool {
    let vk = match VerifyingKey::from_bytes(pubkey) {
        Ok(k) => k,
        Err(_) => return false,
    };
    let h = domain_hash(input);
    let sig = Signature::from_bytes(&proof.proof_bytes);
    if vk.verify_strict(&h, &sig).is_err() {
        return false;
    }
    // Verify output matches
    proof.output == output_hash(&proof.proof_bytes)
}

fn domain_hash(input: &[u8]) -> Vec<u8> {
    let mut h = Sha3_512::new();
    h.update(b"ling-vrf-v1:");
    h.update(input);
    h.finalize().to_vec()
}

fn output_hash(sig: &[u8; 64]) -> [u8; 32] {
    let mut h = Sha3_512::new();
    h.update(b"ling-vrf-out-v1:");
    h.update(sig);
    let out = h.finalize();
    out[..32].try_into().unwrap()
}
