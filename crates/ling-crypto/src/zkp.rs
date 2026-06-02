//! Schnorr zero-knowledge proof of knowledge.
//!
//! Proves knowledge of a discrete log `x` s.t. `X = x·G`
//! on the Ristretto255 group, without revealing `x`.
//! Uses Fiat-Shamir transform (non-interactive via BLAKE3).

use curve25519_dalek::{RistrettoPoint, Scalar, constants::RISTRETTO_BASEPOINT_POINT as G};
use rand::rngs::OsRng;
use zeroize::Zeroizing;

pub struct SchnorrKeypair {
    secret: Zeroizing<Scalar>,
    pub public: RistrettoPoint,
}

impl SchnorrKeypair {
    pub fn generate() -> Self {
        let secret = Scalar::random(&mut OsRng);
        let public = secret * G;
        Self { secret: Zeroizing::new(secret), public }
    }

    pub fn from_scalar_bytes(bytes: [u8; 32]) -> Option<Self> {
        let secret = Scalar::from_canonical_bytes(bytes).into_option()?;
        let public = secret * G;
        Some(Self { secret: Zeroizing::new(secret), public })
    }

    pub fn public_bytes(&self) -> [u8; 32] {
        self.public.compress().to_bytes()
    }

    /// Create a proof of knowledge of the secret key, bound to `msg`.
    pub fn prove(&self, msg: &[u8]) -> SchnorrProof {
        let r = Scalar::random(&mut OsRng);
        let r_point = r * G;
        let c = challenge(&self.public, &r_point, msg);
        let s = r + c * *self.secret;
        SchnorrProof {
            r_bytes: r_point.compress().to_bytes(),
            s_bytes: s.to_bytes(),
        }
    }
}

/// Non-interactive Schnorr proof: (R, s) where s = r + c·x, c = H(X||R||msg).
#[derive(Clone, Debug)]
pub struct SchnorrProof {
    pub r_bytes: [u8; 32],   // commitment R = r·G
    pub s_bytes: [u8; 32],   // response s
}

/// Verify a Schnorr proof against a public key and message.
pub fn schnorr_verify(pubkey_bytes: &[u8; 32], msg: &[u8], proof: &SchnorrProof) -> bool {
    use curve25519_dalek::ristretto::CompressedRistretto;
    let x_point = match CompressedRistretto(*pubkey_bytes).decompress() { Some(p) => p, None => return false };
    let r_point = match CompressedRistretto(proof.r_bytes).decompress()  { Some(p) => p, None => return false };
    let s = match Scalar::from_canonical_bytes(proof.s_bytes).into_option() { Some(s) => s, None => return false };
    let c = challenge(&x_point, &r_point, msg);
    // Check: s·G == R + c·X
    s * G == r_point + c * x_point
}

fn challenge(x_point: &RistrettoPoint, r_point: &RistrettoPoint, msg: &[u8]) -> Scalar {
    let mut data = b"ling-schnorr-v1:".to_vec();
    data.extend_from_slice(&x_point.compress().to_bytes());
    data.extend_from_slice(&r_point.compress().to_bytes());
    data.extend_from_slice(msg);
    let h = blake3::hash(&data);
    Scalar::from_bytes_mod_order(*h.as_bytes())
}
