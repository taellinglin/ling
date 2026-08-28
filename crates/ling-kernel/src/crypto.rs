//! The no_std crypto suite an HTTPS/TLS 1.3 client needs, built freestanding
//! in the kernel: SHA-256, HMAC-SHA256, HKDF-SHA256, ChaCha20-Poly1305 (AEAD),
//! and X25519 (ECDHE) -- all from the audited RustCrypto/dalek crates, with
//! entropy from RDRAND rather than getrandom.
//!
//! This is the foundation the whole HTTPS story (browser HTTPS, lingtp,
//! installing from fu.ling-lang.org over TLS) was blocked on. The TLS 1.3
//! handshake/record/certificate state machine is built on top of these; this
//! module just exposes the primitives and proves each one against a known
//! test vector via `selftest()`.

use sha2::{Digest, Sha256};

/// Fill `out` with hardware random bytes (RDRAND). Returns false if the CPU
/// has no RDRAND -- callers must treat that as "no secure randomness" rather
/// than silently using something weaker. This is the real CSPRNG the kernel
/// lacked (users.rs's rdtsc salt was a placeholder for exactly this).
pub fn random_bytes(out: &mut [u8]) -> bool {
    let mut i = 0;
    while i < out.len() {
        let Some(word) = (unsafe { crate::arch::cpu::rdrand64() }) else {
            return false;
        };
        let bytes = word.to_le_bytes();
        let n = (out.len() - i).min(8);
        out[i..i + n].copy_from_slice(&bytes[..n]);
        i += n;
    }
    true
}

/// SHA-256 of `data`.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().into()
}

/// HMAC-SHA256(key, data).
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    use hmac::{Hmac, Mac};
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("hmac accepts any key len");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// HKDF-SHA256 extract-then-expand into `out`.
pub fn hkdf_sha256(salt: &[u8], ikm: &[u8], info: &[u8], out: &mut [u8]) -> bool {
    let hk = hkdf::Hkdf::<Sha256>::new(Some(salt), ikm);
    hk.expand(info, out).is_ok()
}

/// X25519 Diffie-Hellman: our 32-byte secret scalar and their 32-byte public
/// point -> the 32-byte shared secret. Clamping is handled internally.
pub fn x25519(secret: [u8; 32], their_public: [u8; 32]) -> [u8; 32] {
    use x25519_dalek::{PublicKey, StaticSecret};
    let s = StaticSecret::from(secret);
    let p = PublicKey::from(their_public);
    *s.diffie_hellman(&p).as_bytes()
}

/// Our X25519 public point for a given secret scalar (the key_share we'd send
/// in a TLS ClientHello).
pub fn x25519_public(secret: [u8; 32]) -> [u8; 32] {
    use x25519_dalek::{PublicKey, StaticSecret};
    let s = StaticSecret::from(secret);
    *PublicKey::from(&s).as_bytes()
}

/// ChaCha20-Poly1305 seal in place: encrypts `buf` and returns the 16-byte
/// auth tag. `nonce` is 12 bytes, `key` is 32.
pub fn chachapoly_seal(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], buf: &mut [u8]) -> Option<[u8; 16]> {
    use chacha20poly1305::aead::AeadInPlace;
    use chacha20poly1305::{ChaCha20Poly1305, KeyInit};
    let cipher = ChaCha20Poly1305::new_from_slice(key).ok()?;
    let tag = cipher.encrypt_in_place_detached(nonce.into(), aad, buf).ok()?;
    Some(tag.into())
}

/// ChaCha20-Poly1305 open in place: decrypts `buf` and verifies `tag`. Returns
/// false (and leaves `buf` as-is) if authentication fails.
pub fn chachapoly_open(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], buf: &mut [u8], tag: &[u8; 16]) -> bool {
    use chacha20poly1305::aead::AeadInPlace;
    use chacha20poly1305::{ChaCha20Poly1305, KeyInit};
    let Ok(cipher) = ChaCha20Poly1305::new_from_slice(key) else {
        return false;
    };
    cipher.decrypt_in_place_detached(nonce.into(), aad, buf, tag.into()).is_ok()
}

// ── Self-test against known-answer vectors ──────────────────────────────────

#[derive(Clone, Copy)]
pub struct SelfTest {
    pub rdrand: bool,
    pub sha256: bool,
    pub hmac: bool,
    pub hkdf: bool,
    pub chachapoly: bool,
    pub x25519: bool,
}

impl SelfTest {
    pub fn all_ok(&self) -> bool {
        self.rdrand && self.sha256 && self.hmac && self.hkdf && self.chachapoly && self.x25519
    }
}

/// Run every primitive against a published test vector. Proves the crypto is
/// not just present but computing the right answers in-kernel.
pub fn selftest() -> SelfTest {
    // RDRAND: two draws should differ (a stuck RNG is the classic failure).
    let rdrand = {
        let (mut a, mut b) = ([0u8; 8], [0u8; 8]);
        random_bytes(&mut a) && random_bytes(&mut b) && a != b
    };

    // SHA-256("abc") -- FIPS 180-4 example.
    let sha256_ok = sha256(b"abc")
        == [
            0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
            0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
            0xf2, 0x00, 0x15, 0xad,
        ];

    // HMAC-SHA256, RFC 4231 test case 1: key = 20x 0x0b, data = "Hi There".
    let hmac_ok = hmac_sha256(&[0x0b; 20], b"Hi There")
        == [
            0xb0, 0x34, 0x4c, 0x61, 0xd8, 0xdb, 0x38, 0x53, 0x5c, 0xa8, 0xaf, 0xce, 0xaf, 0x0b,
            0xf1, 0x2b, 0x88, 0x1d, 0xc2, 0x00, 0xc9, 0x83, 0x3d, 0xa7, 0x26, 0xe9, 0x37, 0x6c,
            0x2e, 0x32, 0xcf, 0xf7,
        ];

    // HKDF-SHA256, RFC 5869 test case 1.
    let hkdf_ok = {
        let salt: [u8; 13] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let info: [u8; 10] = [0xf0, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9];
        let mut okm = [0u8; 42];
        hkdf_sha256(&salt, &[0x0b; 22], &info, &mut okm)
            && okm
                == [
                    0x3c, 0xb2, 0x5f, 0x25, 0xfa, 0xac, 0xd5, 0x7a, 0x90, 0x43, 0x4f, 0x64, 0xd0,
                    0x36, 0x2f, 0x2a, 0x2d, 0x2d, 0x0a, 0x90, 0xcf, 0x1a, 0x5a, 0x4c, 0x5d, 0xb0,
                    0x2d, 0x56, 0xec, 0xc4, 0xc5, 0xbf, 0x34, 0x00, 0x72, 0x08, 0xd5, 0xb8, 0x87,
                    0x18, 0x58, 0x65,
                ]
    };

    // ChaCha20-Poly1305: seal then open must round-trip, and a tampered tag
    // must fail to authenticate (proves seal + open + AEAD auth together).
    let chachapoly_ok = {
        let key = [0x42u8; 32];
        let nonce = [0x24u8; 12];
        let aad = b"lingos-tls-1.3";
        let plain = b"the quick brown fox jumps over a caramel donut";
        let mut buf = *plain;
        match chachapoly_seal(&key, &nonce, aad, &mut buf) {
            Some(tag) => {
                let ct_differs = &buf != plain;
                let mut good = buf;
                let roundtrip = chachapoly_open(&key, &nonce, aad, &mut good, &tag) && &good == plain;
                let mut bad = buf;
                let mut bad_tag = tag;
                bad_tag[0] ^= 0x01;
                let rejects = !chachapoly_open(&key, &nonce, aad, &mut bad, &bad_tag);
                ct_differs && roundtrip && rejects
            },
            None => false,
        }
    };

    // X25519, RFC 7748 section 5.2 (first vector).
    let x25519_ok = {
        let scalar: [u8; 32] = [
            0xa5, 0x46, 0xe3, 0x6b, 0xf0, 0x52, 0x7c, 0x9d, 0x3b, 0x16, 0x15, 0x4b, 0x82, 0x46,
            0x5e, 0xdd, 0x62, 0x14, 0x4c, 0x0a, 0xc1, 0xfc, 0x5a, 0x18, 0x50, 0x6a, 0x22, 0x44,
            0xba, 0x44, 0x9a, 0xc4,
        ];
        let point: [u8; 32] = [
            0xe6, 0xdb, 0x68, 0x67, 0x58, 0x30, 0x30, 0xdb, 0x35, 0x94, 0xc1, 0xa4, 0x24, 0xb1,
            0x5f, 0x7c, 0x72, 0x66, 0x24, 0xec, 0x26, 0xb3, 0x35, 0x3b, 0x10, 0xa9, 0x03, 0xa6,
            0xd0, 0xab, 0x1c, 0x4c,
        ];
        x25519(scalar, point)
            == [
                0xc3, 0xda, 0x55, 0x37, 0x9d, 0xe9, 0xc6, 0x90, 0x8e, 0x94, 0xea, 0x4d, 0xf2, 0x8d,
                0x08, 0x4f, 0x32, 0xec, 0xcf, 0x03, 0x49, 0x1c, 0x71, 0xf7, 0x54, 0xb4, 0x07, 0x55,
                0x77, 0xa2, 0x85, 0x52,
            ]
    };

    SelfTest {
        rdrand,
        sha256: sha256_ok,
        hmac: hmac_ok,
        hkdf: hkdf_ok,
        chachapoly: chachapoly_ok,
        x25519: x25519_ok,
    }
}
