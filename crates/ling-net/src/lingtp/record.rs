//! Per-record cascade: XChaCha20-Poly1305 (the real AEAD) first, then
//! DICE-42 ([`ling_crypto::mobius_helix`]) wraps the entire AEAD output
//! under an independently-derived key. Textbook Encrypt-then-Encrypt with
//! independent keys — it can only add work factor for an attacker, never
//! subtract it. See `ling_crypto::mobius_helix` for why this isn't a
//! cryptanalytic upgrade over the AEAD, just defense-in-depth.

use ling_crypto::{MobiusHelixReactor, XChaCha20};

use super::error::LingtpError;
use super::util::random_24;

pub struct RecordCipher {
    xchacha: XChaCha20,
    reactor: MobiusHelixReactor,
}

impl RecordCipher {
    pub fn new(xchacha_key: [u8; 32], reactor_seed: [u8; 32]) -> Self {
        Self {
            xchacha: XChaCha20::new(xchacha_key),
            reactor: MobiusHelixReactor::derive(&reactor_seed),
        }
    }

    pub fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, LingtpError> {
        let inner = self.xchacha.encrypt(plaintext).map_err(LingtpError::Crypto)?;
        let nonce = random_24();
        Ok(self.reactor.seal(&nonce, &inner))
    }

    pub fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, LingtpError> {
        let inner = self.reactor.open(sealed).map_err(LingtpError::Crypto)?;
        self.xchacha.decrypt(&inner).map_err(LingtpError::Crypto)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let cipher = RecordCipher::new([7u8; 32], [9u8; 32]);
        let sealed = cipher.seal(b"order #1042 confirmed").unwrap();
        let opened = cipher.open(&sealed).unwrap();
        assert_eq!(opened, b"order #1042 confirmed");
    }

    #[test]
    fn tamper_is_rejected() {
        let cipher = RecordCipher::new([1u8; 32], [2u8; 32]);
        let mut sealed = cipher.seal(b"escrow release").unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 0x01;
        assert!(cipher.open(&sealed).is_err());
    }

    #[test]
    fn wrong_keys_cannot_open() {
        let a = RecordCipher::new([1u8; 32], [2u8; 32]);
        let b = RecordCipher::new([3u8; 32], [4u8; 32]);
        let sealed = a.seal(b"hello").unwrap();
        assert!(b.open(&sealed).is_err());
    }
}
