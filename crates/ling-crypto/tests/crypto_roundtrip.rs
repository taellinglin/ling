//! ling-crypto smoke tests: hashing is deterministic, symmetric encryption
//! round-trips, and the post-quantum (ML-KEM-768) + hybrid (X25519+ML-KEM) KEMs
//! establish matching shared secrets.

use ling_crypto::hash::Blake3;
use ling_crypto::hybrid::HybridKeypair;
use ling_crypto::pq::MlKem768Keypair;
use ling_crypto::pq_sig::MlDsa65Keypair;
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
    assert_ne!(
        &ct[..],
        &plaintext[..],
        "ciphertext must differ from plaintext"
    );

    let pt = cipher.decrypt(&ct).expect("decrypt must succeed");
    assert_eq!(pt, plaintext, "decrypt(encrypt(x)) == x");
}

#[test]
fn ml_kem_768_establishes_shared_secret() {
    let recipient = MlKem768Keypair::generate();
    let ek = recipient.encapsulation_key();
    let (ct, ss_sender) = ling_crypto::pq::encapsulate(&ek).expect("encapsulate");
    let ss_recipient = recipient.decapsulate(&ct).expect("decapsulate");
    assert_eq!(
        ss_sender, ss_recipient,
        "ML-KEM-768 shared secrets must match"
    );
}

#[test]
fn hybrid_x25519_mlkem_establishes_shared_secret() {
    let recipient = HybridKeypair::generate();
    let pk = recipient.public_key();
    let (ct, ss_sender) = ling_crypto::hybrid::encapsulate(&pk).expect("encapsulate");
    let ss_recipient = recipient.decapsulate(&ct).expect("decapsulate");
    assert_eq!(ss_sender, ss_recipient, "hybrid shared secrets must match");
    // The hybrid secret must not equal either component trivially; just assert
    // it's 32 bytes of established key material.
    assert_eq!(ss_sender.len(), 32);
}

#[test]
fn ml_dsa_65_signs_and_verifies() {
    let kp = MlDsa65Keypair::generate();
    let pk = kp.public_key();
    let msg = b"LC-2026-000123 issued to Anita Bevill";
    let sig = kp.sign(msg);
    assert!(
        MlDsa65Keypair::verify(&pk, msg, &sig).is_ok(),
        "a genuine signature must verify"
    );
}

#[test]
fn ml_dsa_65_rejects_tampered_message_and_signature() {
    let kp = MlDsa65Keypair::generate();
    let pk = kp.public_key();
    let msg = b"original message";
    let sig = kp.sign(msg);

    assert!(
        MlDsa65Keypair::verify(&pk, b"tampered message", &sig).is_err(),
        "signature must not verify against a different message"
    );

    let mut bad_sig = sig.clone();
    let last = bad_sig.len() - 1;
    bad_sig[last] ^= 0xFF;
    assert!(
        MlDsa65Keypair::verify(&pk, msg, &bad_sig).is_err(),
        "tampered signature must not verify"
    );
}

#[test]
fn ml_dsa_65_seed_is_deterministic() {
    let seed = [7u8; 32];
    let kp_a = MlDsa65Keypair::from_seed(seed);
    let kp_b = MlDsa65Keypair::from_seed(seed);
    assert_eq!(
        kp_a.public_key(),
        kp_b.public_key(),
        "same seed must rederive the same public key across restarts"
    );

    let msg = b"issuer identity persisted as a 32-byte seed";
    let sig = kp_a.sign(msg);
    assert!(
        MlDsa65Keypair::verify(&kp_b.public_key(), msg, &sig).is_ok(),
        "a signature from one seed-derived keypair must verify against the other's public key"
    );

    assert_eq!(&*kp_a.to_bytes(), &seed, "to_bytes() must round-trip the original seed");
}

#[test]
fn aes_gcm_256_rejects_tampered_ciphertext() {
    let key = AesGcm256::generate_key();
    let cipher = AesGcm256::new(key);
    let mut ct = cipher.encrypt(b"secret").expect("encrypt");
    // Flip a byte in the ciphertext body.
    let last = ct.len() - 1;
    ct[last] ^= 0xFF;
    assert!(
        cipher.decrypt(&ct).is_err(),
        "tampered ciphertext must fail auth"
    );
}
