//! X25519 ‖ ML-KEM-1024 hybrid KEM (DESIGN §4). Secure if either half holds.

use ml_kem::array::Array;
use ml_kem::{Decapsulate, DecapsulationKey, EncapsulationKey, KeyExport, MlKem1024};
use x25519_dalek::{PublicKey as XPublic, StaticSecret};
use zeroize::{Zeroize, Zeroizing};

use crate::lw::hash::h256;
use crate::lw::kdf::random_bytes;
use crate::lw::{CryptoError, Result};

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
/// Panics on an invalid `pk`; [`try_encapsulate`] is the non-panicking form.
pub fn encapsulate(pk: &KemPublic, context: &[u8]) -> (KemCiphertext, [u8; 32]) {
    try_encapsulate(pk, context).expect("validated KemPublic")
}

/// [`encapsulate`] that returns `Malformed` instead of panicking when `pk`
/// is not a valid public key (wrong length or failing the FIPS 203 check).
pub fn try_encapsulate(pk: &KemPublic, context: &[u8]) -> Result<(KemCiphertext, [u8; 32])> {
    if pk.0.len() != PUBLIC_KEY_LEN {
        return Err(CryptoError::Malformed);
    }
    let ek_arr = Array::try_from(&pk.0[X_LEN..]).map_err(|_| CryptoError::Malformed)?;
    let ek = EncapsulationKey::<MlKem1024>::new(&ek_arr).map_err(|_| CryptoError::Malformed)?;

    let mut their_x = [0u8; 32];
    their_x.copy_from_slice(&pk.0[..X_LEN]);
    let mut eph_bytes = Zeroizing::new([0u8; 32]);
    random_bytes(eph_bytes.as_mut());
    let eph = StaticSecret::from(*eph_bytes);
    let ss_x = Zeroizing::new(*eph.diffie_hellman(&XPublic::from(their_x)).as_bytes());

    let mut m = Zeroizing::new([0u8; 32]);
    random_bytes(m.as_mut());
    let (ct_k, ss_k) = ek.encapsulate_deterministic(&Array::from(*m));

    let mut ct = Vec::with_capacity(CIPHERTEXT_LEN);
    ct.extend_from_slice(XPublic::from(&eph).as_bytes());
    ct.extend_from_slice(ct_k.as_slice());
    let ss = combine(context, &ss_x, ss_k.as_slice(), &ct, &pk.0);
    Ok((KemCiphertext(ct), ss))
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
