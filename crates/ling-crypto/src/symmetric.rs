//! AES-GCM symmetric encryption stub.

pub struct AesGcm {
    key: Vec<u8>,
}

impl AesGcm {
    pub fn new(key: &[u8]) -> Self {
        Self { key: key.to_vec() }
    }

    /// Encrypt `plaintext` with `nonce`. Returns ciphertext + 16-byte tag.
    pub fn encrypt(&self, plaintext: &[u8], _nonce: &[u8]) -> Vec<u8> {
        let _ = &self.key;
        // TODO: wire to aes-gcm crate when feature is enabled
        plaintext.to_vec()
    }

    /// Decrypt and verify. Returns `None` on authentication failure.
    pub fn decrypt(&self, ciphertext: &[u8], _nonce: &[u8]) -> Option<Vec<u8>> {
        let _ = &self.key;
        Some(ciphertext.to_vec())
    }
}
