//! Möbius-Helix Reactor smoke tests, run from outside the crate against its
//! public API only: sealing is lossless across message lengths, tampering
//! with any byte is rejected, and distinct seeds/nonces never collide.

use ling_crypto::mobius_helix::MobiusHelixReactor;

#[test]
fn seal_open_is_lossless() {
    let reactor = MobiusHelixReactor::derive(&[42u8; 32]);
    let messages: &[&[u8]] = &[
        b"",
        b"x",
        b"lingtp-v1 mobius-helix-reactor",
        b"Cosmic Sour Belts x3, Nebula Gummy Bears x1, Vanilla Nova Pint x2",
    ];
    for msg in messages {
        let nonce = [3u8; 24];
        let sealed = reactor.seal(&nonce, msg);
        let opened = reactor.open(&sealed).expect("must open what was sealed");
        assert_eq!(&opened, msg);
    }
}

#[test]
fn tampering_any_byte_is_detected() {
    let reactor = MobiusHelixReactor::derive(&[9u8; 32]);
    let nonce = [1u8; 24];
    let sealed = reactor.seal(&nonce, b"escrow release: order #1042");

    for i in 0..sealed.len() {
        let mut tampered = sealed.clone();
        tampered[i] ^= 0x01;
        assert!(
            reactor.open(&tampered).is_err(),
            "flipping byte {i} should invalidate the seal"
        );
    }
}

#[test]
fn distinct_seeds_do_not_cross_open() {
    let a = MobiusHelixReactor::derive(&[1u8; 32]);
    let b = MobiusHelixReactor::derive(&[2u8; 32]);
    let sealed = a.seal(&[5u8; 24], b"vendor payout address lng1...");
    assert!(b.open(&sealed).is_err());
}
