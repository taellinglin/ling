//! Ed25519 asymmetric signing stub.

pub struct Ed25519 {
    secret: [u8; 32],
}

impl Ed25519 {
    pub fn generate() -> Self {
        Self { secret: [0u8; 32] }
    }

    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self { secret: seed }
    }

    /// Sign `msg`. Returns 64-byte signature.
    pub fn sign(&self, _msg: &[u8]) -> [u8; 64] {
        let _ = &self.secret;
        [0u8; 64]
    }

    /// Verify `sig` over `msg` with `pubkey` (32 bytes). Returns true on success.
    pub fn verify(_pubkey: &[u8], _msg: &[u8], _sig: &[u8]) -> bool {
        false
    }

    /// Export the 32-byte public key.
    pub fn public_key(&self) -> [u8; 32] {
        [0u8; 32]
    }
}
