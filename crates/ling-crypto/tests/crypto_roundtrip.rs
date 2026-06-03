//! ling-crypto smoke tests: hashing is deterministic and symmetric
//! encryption round-trips.

use ling_crypto::hash::Blake3;
use ling_crypto::symmetric::AesGcm256;

#[test]
fn blake3_is_deterministic_and_sensitive() {
    let a = Blake3::hash(b"hello");
    let b = Blake3::hash(b"hello");
    let c = Blake3::hash(b"hellp");
    assert_eq!(a, b, "same input must hash identically");
    assert_ne!(a, c, "different input must hash differently");
    assert_eq!(a.len(), 32, "Blake3 digest is 32 bytes");
}

#[test]
fn aes_gcm_256_round_trips() {
    let key = AesGcm256::generate_key();
    let cipher = AesGcm256::new(key);
    let plaintext = b"the quick brown fox";

    let ct = cipher.encrypt(plaintext).expect("encrypt must succeed");
    assert_ne!(&ct[..], &plaintext[..], "ciphertext must differ from plaintext");

    let pt = cipher.decrypt(&ct).expect("decrypt must succeed");
    assert_eq!(pt, plaintext, "decrypt(encrypt(x)) == x");
}

#[test]
fn aes_gcm_256_rejects_tampered_ciphertext() {
    let key = AesGcm256::generate_key();
    let cipher = AesGcm256::new(key);
    let mut ct = cipher.encrypt(b"secret").expect("encrypt");
    // Flip a byte in the ciphertext body.
    let last = ct.len() - 1;
    ct[last] ^= 0xFF;
    assert!(cipher.decrypt(&ct).is_err(), "tampered ciphertext must fail auth");
}
