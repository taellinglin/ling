#![cfg(feature = "lwp")]
use ling_net::lwp::crypto::identity::IdentityKey;
use ling_net::lwp::{SeedPeer, SignedSeedList};

fn list(auth: &IdentityKey) -> SignedSeedList {
    let seeds = vec![
        SeedPeer { addr: "1.2.3.4:4747".into(), identity: IdentityKey::generate().public() },
        SeedPeer { addr: "[2001:db8::1]:4747".into(), identity: IdentityKey::generate().public() },
        SeedPeer { addr: "abcdefghijklmnop.onion:4747".into(), identity: IdentityKey::generate().public() },
    ];
    SignedSeedList::sign(1, 1_700_000_000, seeds, auth)
}

#[test]
fn sign_verify_roundtrip() {
    let auth = IdentityKey::generate();
    let l = list(&auth);
    assert_eq!(l.verify(&auth.public()).unwrap().len(), 3);
    let b = l.to_bytes();
    let back = SignedSeedList::from_bytes(&b).unwrap();
    assert_eq!(back, l);
    assert!(back.verify(&auth.public()).is_some());
}

#[test]
fn tamper_and_wrong_authority_fail() {
    let auth = IdentityKey::generate();
    let l = list(&auth);
    assert!(l.verify(&IdentityKey::generate().public()).is_none());
    let b = l.to_bytes();
    // Flip each region: header, an address byte, an identity byte, the signature.
    for &i in &[7usize, 18, 40, 2000, b.len() - 10] {
        let mut t = b.clone();
        t[i] ^= 0x01;
        if let Some(x) = SignedSeedList::from_bytes(&t) {
            assert!(x.verify(&auth.public()).is_none(), "tamper at {i} verified");
        }
    }
    let mut l2 = l.clone();
    l2.issued_at += 1;
    assert!(l2.verify(&auth.public()).is_none());
    let mut l3 = l.clone();
    l3.network = 2;
    assert!(l3.verify(&auth.public()).is_none());
}

#[test]
fn garbage_is_rejected_without_panic() {
    assert!(SignedSeedList::from_bytes(&[]).is_none());
    assert!(SignedSeedList::from_bytes(b"LWSD").is_none());
    assert!(SignedSeedList::from_bytes(&vec![0xff; 10_000_000]).is_none());
    let auth = IdentityKey::generate();
    let b = list(&auth).to_bytes();
    for cut in [0, 1, 5, 16, 17, 100, b.len() - 1] {
        assert!(SignedSeedList::from_bytes(&b[..cut]).is_none());
    }
    let mut extra = b.clone();
    extra.push(0);
    assert!(SignedSeedList::from_bytes(&extra).is_none());
    // huge count
    let mut big = b[..16].to_vec();
    big[14] = 0xff;
    big[15] = 0xff;
    assert!(SignedSeedList::from_bytes(&big).is_none());
    for _ in 0..3000 {
        let n = rand::random::<u16>() as usize % 512;
        let mut v: Vec<u8> = (0..n).map(|_| rand::random()).collect();
        if n >= 4 && rand::random::<bool>() {
            v[..4].copy_from_slice(b"LWSD");
        }
        let _ = SignedSeedList::from_bytes(&v);
    }
}
