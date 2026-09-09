use rand::RngCore;
use subtle::ConstantTimeEq;

pub fn random_32() -> [u8; 32] {
    let mut buf = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    buf
}

pub fn random_24() -> [u8; 24] {
    let mut buf = [0u8; 24];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    buf
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Constant-time equality for handshake Finished-MAC style checks.
pub fn ct_eq32(a: &[u8; 32], b: &[u8; 32]) -> bool {
    bool::from(a.ct_eq(b))
}
