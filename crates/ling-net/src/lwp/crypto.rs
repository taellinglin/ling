//! Crypto shim for LWP-1: a verbatim copy of the parts of ling-wallet's
//! `lw-crypto` (hash / kem / identity / aead / random) that the transport
//! needs, byte-for-byte per DESIGN §2–§4. Every crypto import in `lwp` goes
//! through this one module, so it can later be replaced wholesale by
//! `pub use ling_crypto::lw::{hash, kem1024 as kem, identity, aead, kdf, ...}`.

/// Coarse error, as in lw-crypto.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CryptoError {
    #[error("malformed input")]
    Malformed,
    #[error("authentication failed")]
    AuthFailed,
    #[error("invalid signature")]
    BadSignature,
    #[error("key derivation failed")]
    Kdf,
}

pub type Result<T> = core::result::Result<T, CryptoError>;

/// OS CSPRNG (the `random_bytes` half of lw-crypto's `kdf`; Argon2id is not needed here).
pub mod kdf {
    use rand::RngCore;
    /// Fill `buf` from the OS CSPRNG.
    pub fn random_bytes(buf: &mut [u8]) {
        rand::rngs::OsRng.fill_bytes(buf);
    }
}

pub mod hash {
    //! Domain-separated SHAKE256 (DESIGN §2).
    //!
    //! `input = enc(domain) ‖ enc(part_1) ‖ … ‖ enc(part_n)`, `enc(x) = u32_le(len) ‖ x`.
    //! Length-prefixing every field makes the encoding injective, so two
    //! different (domain, parts) inputs can never collide by concatenation.

    use sha3::digest::{ExtendableOutput, Update, XofReader};
    use sha3::Shake256;

    fn absorb(domain: &str, parts: &[&[u8]]) -> Shake256 {
        let mut h = Shake256::default();
        h.update(&(domain.len() as u32).to_le_bytes());
        h.update(domain.as_bytes());
        for p in parts {
            h.update(&(p.len() as u32).to_le_bytes());
            h.update(p);
        }
        h
    }

    /// 32-byte hash `H(domain, parts…)`.
    pub fn h256(domain: &str, parts: &[&[u8]]) -> [u8; 32] {
        let mut out = [0u8; 32];
        xof(domain, parts, &mut out);
        out
    }

    /// Fill `out` with `XOF(domain, parts…)`.
    pub fn xof(domain: &str, parts: &[&[u8]], out: &mut [u8]) {
        absorb(domain, parts).finalize_xof().read(out);
    }

    /// Convenience: `XOF` into a fresh `Vec` of `len` bytes.
    pub fn xof_vec(domain: &str, parts: &[&[u8]], len: usize) -> Vec<u8> {
        let mut v = vec![0u8; len];
        xof(domain, parts, &mut v);
        v
    }

    /// A streaming XOF reader, for samplers that consume an unknown amount.
    pub struct XofStream(sha3::Shake256Reader);

    impl XofStream {
        pub fn new(domain: &str, parts: &[&[u8]]) -> Self {
            XofStream(absorb(domain, parts).finalize_xof())
        }
        pub fn read(&mut self, out: &mut [u8]) {
            self.0.read(out);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn domain_and_boundaries_separate() {
            assert_ne!(h256("LW1/a", &[b"x"]), h256("LW1/b", &[b"x"]));
            // Without length prefixes these two would collide.
            assert_ne!(h256("LW1/a", &[b"ab", b"c"]), h256("LW1/a", &[b"a", b"bc"]));
            assert_eq!(h256("LW1/a", &[b"x"]), h256("LW1/a", &[b"x"]));
        }

        #[test]
        fn xof_prefix_consistent() {
            let long = xof_vec("LW1/t", &[b"seed"], 64);
            assert_eq!(&long[..32], &h256("LW1/t", &[b"seed"]));
        }
    }
}

pub mod aead {
    //! XChaCha20-Poly1305 with associated data. Output of [`seal`] is `ct ‖ tag16`
    //! (the caller owns the nonce; nonces must never repeat under one key).

    use chacha20poly1305::aead::{Aead, KeyInit, Payload};
    use chacha20poly1305::{XChaCha20Poly1305, XNonce};

    use super::{CryptoError, Result};

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
}

pub mod kem {
    //! X25519 ‖ ML-KEM-1024 hybrid KEM (DESIGN §4). Secure if either half holds.

    use ml_kem::array::Array;
    use ml_kem::{Decapsulate, DecapsulationKey, EncapsulationKey, KeyExport, MlKem1024};
    use x25519_dalek::{PublicKey as XPublic, StaticSecret};
    use zeroize::{Zeroize, Zeroizing};

    use super::hash::h256;
    use super::kdf::random_bytes;
    use super::{CryptoError, Result};

    const X_LEN: usize = 32;
    const EK_LEN: usize = 1568;
    const CT_LEN: usize = 1568;

    pub const PUBLIC_KEY_LEN: usize = X_LEN + EK_LEN;
    pub const CIPHERTEXT_LEN: usize = X_LEN + CT_LEN;
    pub const SEED_LEN: usize = 96; // 32 X25519 secret ‖ 64 ML-KEM (d ‖ z)

    #[derive(Clone, PartialEq, Eq, Debug)]
    pub struct KemPublic(pub Vec<u8>); // exactly PUBLIC_KEY_LEN bytes

    #[derive(Clone, PartialEq, Eq, Debug)]
    pub struct KemCiphertext(pub Vec<u8>); // exactly CIPHERTEXT_LEN bytes

    /// Decapsulation key. The X25519 secret zeroizes itself on drop; the ML-KEM
    /// key is rebuilt from `seed`, which is zeroized here.
    pub struct KemSecret {
        x: StaticSecret,
        dk: DecapsulationKey<MlKem1024>,
        public: KemPublic,
    }

    impl KemPublic {
        pub fn from_bytes(b: &[u8]) -> Result<Self> {
            if b.len() != PUBLIC_KEY_LEN {
                return Err(CryptoError::Malformed);
            }
            // FIPS 203 §7.2 encapsulation-key check (coefficients reduced mod q).
            let arr = Array::try_from(&b[X_LEN..]).map_err(|_| CryptoError::Malformed)?;
            EncapsulationKey::<MlKem1024>::new(&arr).map_err(|_| CryptoError::Malformed)?;
            Ok(KemPublic(b.to_vec()))
        }
        pub fn as_bytes(&self) -> &[u8] {
            &self.0
        }
    }

    impl KemCiphertext {
        pub fn from_bytes(b: &[u8]) -> Result<Self> {
            if b.len() != CIPHERTEXT_LEN {
                return Err(CryptoError::Malformed);
            }
            Ok(KemCiphertext(b.to_vec()))
        }
        pub fn as_bytes(&self) -> &[u8] {
            &self.0
        }
    }

    fn combine(context: &[u8], ss_x: &[u8; 32], ss_k: &[u8], ct: &[u8], pk: &[u8]) -> [u8; 32] {
        h256("LW1/kem", &[context, ss_x, ss_k, ct, pk])
    }

    impl KemSecret {
        /// Deterministic from a 96-byte seed (DESIGN §3.1 `view_seed`).
        pub fn from_seed(seed: &[u8; SEED_LEN]) -> Self {
            let mut xs = [0u8; 32];
            xs.copy_from_slice(&seed[..32]);
            let x = StaticSecret::from(xs);
            xs.zeroize();
            let mut ks = Zeroizing::new([0u8; 64]);
            ks.copy_from_slice(&seed[32..]);
            let dk = DecapsulationKey::<MlKem1024>::from_seed(Array::from(*ks));
            let mut pk = Vec::with_capacity(PUBLIC_KEY_LEN);
            pk.extend_from_slice(XPublic::from(&x).as_bytes());
            pk.extend_from_slice(dk.encapsulation_key().to_bytes().as_slice());
            KemSecret { x, dk, public: KemPublic(pk) }
        }

        /// Fresh random key (used for ephemeral P2P handshakes).
        pub fn generate() -> Self {
            let mut seed = Zeroizing::new([0u8; SEED_LEN]);
            random_bytes(seed.as_mut());
            Self::from_seed(&seed)
        }

        pub fn public(&self) -> KemPublic {
            self.public.clone()
        }

        /// `ss = H("LW1/kem", context, ss_x25519, ss_mlkem, ct, pk)`.
        pub fn decapsulate(&self, ct: &KemCiphertext, context: &[u8]) -> Result<[u8; 32]> {
            if ct.0.len() != CIPHERTEXT_LEN {
                return Err(CryptoError::Malformed);
            }
            let mut eph = [0u8; 32];
            eph.copy_from_slice(&ct.0[..X_LEN]);
            let ss_x = Zeroizing::new(*self.x.diffie_hellman(&XPublic::from(eph)).as_bytes());
            // ML-KEM decapsulation never fails on a well-sized ciphertext (implicit
            // rejection returns a pseudorandom secret), so tampering surfaces as a
            // mismatched shared secret, not an error oracle.
            let ss_k = self.dk.decapsulate_slice(&ct.0[X_LEN..]).map_err(|_| CryptoError::Malformed)?;
            Ok(combine(context, &ss_x, ss_k.as_slice(), &ct.0, &self.public.0))
        }
    }

    /// Encapsulate to `pk`; returns the ciphertext and the 32-byte shared secret.
    /// `pk` must come from [`KemPublic::from_bytes`] (already validated).
    pub fn encapsulate(pk: &KemPublic, context: &[u8]) -> (KemCiphertext, [u8; 32]) {
        let mut their_x = [0u8; 32];
        their_x.copy_from_slice(&pk.0[..X_LEN]);
        let mut eph_bytes = Zeroizing::new([0u8; 32]);
        random_bytes(eph_bytes.as_mut());
        let eph = StaticSecret::from(*eph_bytes);
        let ss_x = Zeroizing::new(*eph.diffie_hellman(&XPublic::from(their_x)).as_bytes());

        let ek_arr = Array::try_from(&pk.0[X_LEN..]).expect("validated KemPublic");
        let ek = EncapsulationKey::<MlKem1024>::new(&ek_arr).expect("validated KemPublic");
        let mut m = Zeroizing::new([0u8; 32]);
        random_bytes(m.as_mut());
        let (ct_k, ss_k) = ek.encapsulate_deterministic(&Array::from(*m));

        let mut ct = Vec::with_capacity(CIPHERTEXT_LEN);
        ct.extend_from_slice(XPublic::from(&eph).as_bytes());
        ct.extend_from_slice(ct_k.as_slice());
        let ss = combine(context, &ss_x, ss_k.as_slice(), &ct, &pk.0);
        (KemCiphertext(ct), ss)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn roundtrip_sizes_and_context() {
            let sk = KemSecret::from_seed(&[3u8; SEED_LEN]);
            let pk = sk.public();
            assert_eq!(pk.0.len(), PUBLIC_KEY_LEN);
            assert_eq!(KemPublic::from_bytes(&pk.0).unwrap(), pk);
            let (ct, ss) = encapsulate(&pk, b"ctx");
            assert_eq!(ct.0.len(), CIPHERTEXT_LEN);
            assert_eq!(sk.decapsulate(&ct, b"ctx").unwrap(), ss);
            assert_ne!(sk.decapsulate(&ct, b"other").unwrap(), ss);
        }

        #[test]
        fn deterministic_from_seed_and_tamper() {
            assert_eq!(KemSecret::from_seed(&[5; SEED_LEN]).public(), KemSecret::from_seed(&[5; SEED_LEN]).public());
            let sk = KemSecret::generate();
            let (mut ct, ss) = encapsulate(&sk.public(), b"c");
            ct.0[100] ^= 1;
            assert_ne!(sk.decapsulate(&ct, b"c").unwrap(), ss);
            assert!(KemPublic::from_bytes(&[0u8; 10]).is_err());
        }
    }
}

pub mod identity {
    //! ML-DSA-87 (FIPS 204) identity keys, wrapping `ling_crypto::MlDsa87Keypair`.
    //!
    //! Every signature is over `H("LW1/sig/" + context, msg)` so a signature made
    //! for one purpose (address bundle, seed list, P2P handshake, receipt) can
    //! never be replayed as another.

    use ling_crypto::MlDsa87Keypair;
    use zeroize::Zeroizing;

    use super::hash::h256;
    use super::kdf::random_bytes;
    use super::{CryptoError, Result};

    pub const PUBLIC_KEY_LEN: usize = 2592;
    pub const SIGNATURE_LEN: usize = 4627;

    #[derive(Clone, PartialEq, Eq, Debug)]
    pub struct IdentityPublic(pub Vec<u8>); // exactly PUBLIC_KEY_LEN

    #[derive(Clone, PartialEq, Eq, Debug)]
    pub struct IdentitySignature(pub Vec<u8>); // exactly SIGNATURE_LEN

    /// Signing key. Only the 32-byte seed is kept, zeroized on drop; the
    /// expanded key is rebuilt per signature (ML-DSA keygen is ~0.1 ms).
    pub struct IdentityKey {
        seed: Zeroizing<[u8; 32]>,
        public: IdentityPublic,
    }

    fn digest(context: &str, msg: &[u8]) -> [u8; 32] {
        h256(&format!("LW1/sig/{context}"), &[msg])
    }

    impl IdentityKey {
        pub fn from_seed(seed: &[u8; 32]) -> Self {
            let public = IdentityPublic(MlDsa87Keypair::from_seed(*seed).public_key());
            IdentityKey { seed: Zeroizing::new(*seed), public }
        }
        pub fn generate() -> Self {
            let mut s = Zeroizing::new([0u8; 32]);
            random_bytes(s.as_mut());
            Self::from_seed(&s)
        }
        /// The 32-byte seed, for persisting a node identity (caller must protect it).
        pub fn seed(&self) -> Zeroizing<[u8; 32]> {
            self.seed.clone()
        }
        pub fn public(&self) -> IdentityPublic {
            self.public.clone()
        }
        pub fn sign(&self, context: &str, msg: &[u8]) -> IdentitySignature {
            IdentitySignature(MlDsa87Keypair::from_seed(*self.seed).sign(&digest(context, msg)))
        }
    }

    impl IdentityPublic {
        pub fn from_bytes(b: &[u8]) -> Result<Self> {
            if b.len() != PUBLIC_KEY_LEN {
                return Err(CryptoError::Malformed);
            }
            Ok(IdentityPublic(b.to_vec()))
        }
        pub fn verify(&self, context: &str, msg: &[u8], sig: &IdentitySignature) -> bool {
            sig.0.len() == SIGNATURE_LEN && MlDsa87Keypair::verify(&self.0, &digest(context, msg), &sig.0).is_ok()
        }
        /// Short human-comparable id: first 8 bytes of `H("LW1/identity/fp", pk)` as hex.
        pub fn fingerprint(&self) -> String {
            let h = h256("LW1/identity/fp", &[&self.0]);
            format!("{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}", h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7])
        }
    }

    impl IdentitySignature {
        pub fn from_bytes(b: &[u8]) -> Result<Self> {
            if b.len() != SIGNATURE_LEN {
                return Err(CryptoError::Malformed);
            }
            Ok(IdentitySignature(b.to_vec()))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn sign_verify_context_bound() {
            let k = IdentityKey::from_seed(&[4u8; 32]);
            let pk = k.public();
            assert_eq!(pk.0.len(), PUBLIC_KEY_LEN);
            let sig = k.sign("bundle", b"msg");
            assert_eq!(sig.0.len(), SIGNATURE_LEN);
            assert!(pk.verify("bundle", b"msg", &sig));
            assert!(!pk.verify("seeds", b"msg", &sig), "context separation");
            assert!(!pk.verify("bundle", b"msh", &sig));
            assert!(!IdentityKey::generate().public().verify("bundle", b"msg", &sig));
            assert_eq!(pk.fingerprint().len(), 19);
        }
    }
}

