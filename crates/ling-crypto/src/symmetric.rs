//! Authenticated encryption: AES-256-GCM and XChaCha20-Poly1305.

use aes_gcm::{Aes256Gcm, Key, Nonce, aead::{Aead, KeyInit, OsRng, AeadCore}};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use zeroize::Zeroizing;

pub struct AesGcm256 {
    key: Zeroizing<[u8; 32]>,
}

impl AesGcm256 {
    pub fn new(key: [u8; 32]) -> Self {
        Self { key: Zeroizing::new(key) }
    }

    pub fn generate_key() -> [u8; 32] {
        Aes256Gcm::generate_key(OsRng).into()
    }

    /// Returns nonce (12 bytes) + ciphertext + tag.
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, &'static str> {
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&*self.key));
        let nonce = Aes256Gcm::generate_nonce(OsRng);
        let ct = cipher.encrypt(&nonce, plaintext).map_err(|_| "encryption failed")?;
        let mut out = nonce.to_vec();
        out.extend_from_slice(&ct);
        Ok(out)
    }

    /// Input: nonce (12 bytes) + ciphertext + tag.
    pub fn decrypt(&self, nonce_and_ct: &[u8]) -> Result<Vec<u8>, &'static str> {
        if nonce_and_ct.len() < 12 { return Err("too short"); }
        let (nonce_bytes, ct) = nonce_and_ct.split_at(12);
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&*self.key));
        let nonce = Nonce::from_slice(nonce_bytes);
        cipher.decrypt(nonce, ct).map_err(|_| "decryption/auth failed")
    }
}

pub struct XChaCha20 {
    key: Zeroizing<[u8; 32]>,
}

impl XChaCha20 {
    pub fn new(key: [u8; 32]) -> Self {
        Self { key: Zeroizing::new(key) }
    }

    pub fn generate_key() -> [u8; 32] {
        XChaCha20Poly1305::generate_key(OsRng).into()
    }

    /// Returns nonce (24 bytes) + ciphertext + tag.
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, &'static str> {
        let cipher = XChaCha20Poly1305::new(chacha20poly1305::Key::from_slice(&*self.key));
        let nonce = XChaCha20Poly1305::generate_nonce(OsRng);
        let ct = cipher.encrypt(&nonce, plaintext).map_err(|_| "encryption failed")?;
        let mut out = nonce.to_vec();
        out.extend_from_slice(&ct);
        Ok(out)
    }

    /// Input: nonce (24 bytes) + ciphertext + tag.
    pub fn decrypt(&self, nonce_and_ct: &[u8]) -> Result<Vec<u8>, &'static str> {
        if nonce_and_ct.len() < 24 { return Err("too short"); }
        let (nonce_bytes, ct) = nonce_and_ct.split_at(24);
        let cipher = XChaCha20Poly1305::new(chacha20poly1305::Key::from_slice(&*self.key));
        let nonce = XNonce::from_slice(nonce_bytes);
        cipher.decrypt(nonce, ct).map_err(|_| "decryption/auth failed")
    }
}
