//! The layering Horizon's vault (ling-kernel `vault.rs`) uses: XChaCha20-Poly1305
//! with a caller nonce, inside a DICE-42 seal, signed with ML-DSA-87.

use ling_crypto::{hkdf_sha3, Blake3, MlDsa87Keypair, MobiusHelixReactor, XChaCha20};

fn key(root: &[u8], salt: &[u8], label: &[u8]) -> [u8; 32] {
    let k = hkdf_sha3(root, salt, label, 32).unwrap();
    let mut out = [0u8; 32];
    out.copy_from_slice(&k);
    out
}

#[test]
fn vault_layers_round_trip_and_reject_tampering() {
    let salt = [7u8; 32];
    let root = [42u8; 32];
    let (ka, kd, ks) = (key(&root, &salt, b"xchacha20-poly1305"), key(&root, &salt, b"dice-42"), key(&root, &salt, b"ml-dsa-87"));
    let text = b"L\tgithub.com\tling\thunter2\t\n";

    let inner = XChaCha20::new(ka).encrypt_with_nonce(&[1u8; 24], text).unwrap();
    assert_eq!(XChaCha20::new(ka).decrypt(&inner).unwrap(), text);
    let body = MobiusHelixReactor::derive(&kd).seal(&[2u8; 24], &inner);
    let kp = MlDsa87Keypair::from_seed(ks);
    let pk = kp.public_key();
    let sig = kp.sign(&body);
    assert_eq!(Blake3::hash(&MlDsa87Keypair::from_seed(ks).public_key()), Blake3::hash(&pk));

    // open
    MlDsa87Keypair::verify(&pk, &body, &sig).unwrap();
    let inner2 = MobiusHelixReactor::derive(&kd).open(&body).unwrap();
    assert_eq!(XChaCha20::new(ka).decrypt(&inner2).unwrap(), text);

    // one flipped bit anywhere is caught by every layer
    let mut bad = body.clone();
    bad[30] ^= 1;
    assert!(MlDsa87Keypair::verify(&pk, &bad, &sig).is_err());
    assert!(MobiusHelixReactor::derive(&kd).open(&bad).is_err());
    let mut bad_inner = inner.clone();
    bad_inner[30] ^= 1;
    assert!(XChaCha20::new(ka).decrypt(&bad_inner).is_err());

    // another key (a wrong password) opens nothing
    let other = key(&[43u8; 32], &salt, b"dice-42");
    assert!(MobiusHelixReactor::derive(&other).open(&body).is_err());
}
