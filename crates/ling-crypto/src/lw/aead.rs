//! XChaCha20-Poly1305 with associated data. Output of [`seal`] is `ct ‖ tag16`
//! (the caller owns the nonce; nonces must never repeat under one key).

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};

use crate::lw::{CryptoError, Result};

pub const TAG_LEN: usize = 16;

pub fn seal(key: &[u8; 32], nonce: &[u8; 24], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
    XChaCha20Poly1305::new(key.into())
        .encrypt(XNonce::from_slice(nonce), Payload { msg: plaintext, aad })
        .expect("XChaCha20-Poly1305 encryption cannot fail for in-memory buffers")
}

pub fn open(key: &[u8; 32], nonce: &[u8; 24], aad: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>> {
    XChaCha20Poly1305::new(key.into())
        .decrypt(XNonce::from_slice(nonce), Payload { msg: ciphertext, aad })
        .map_err(|_| CryptoError::AuthFailed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_tamper() {
        let key = [7u8; 32];
        let nonce = [9u8; 24];
        let ct = seal(&key, &nonce, b"hdr", b"hello");
        assert_eq!(ct.len(), 5 + TAG_LEN);
        assert_eq!(open(&key, &nonce, b"hdr", &ct).unwrap(), b"hello");
        assert!(open(&key, &nonce, b"other", &ct).is_err());
        let mut bad = ct.clone();
        bad[0] ^= 1;
        assert!(open(&key, &nonce, b"hdr", &bad).is_err());
    }
}
