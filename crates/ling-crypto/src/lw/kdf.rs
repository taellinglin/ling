//! Argon2id password-based keys for the wallet file (DESIGN §9).

use argon2::{Algorithm, Argon2, Params, Version};
use zeroize::Zeroizing;

use crate::lw::{CryptoError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PasswordParams {
    pub m_cost_kib: u32,
    pub t_cost: u32,
    pub p_cost: u32,
}

impl PasswordParams {
    /// 256 MiB, 3 passes, 1 lane (DESIGN §9).
    pub const WALLET: PasswordParams = PasswordParams { m_cost_kib: 256 * 1024, t_cost: 3, p_cost: 1 };
    /// Cheap parameters for unit tests only.
    pub const TEST: PasswordParams = PasswordParams { m_cost_kib: 64, t_cost: 1, p_cost: 1 };
}

pub fn password_key(password: &[u8], salt: &[u8; 16], params: PasswordParams) -> Result<Zeroizing<[u8; 32]>> {
    let p = Params::new(params.m_cost_kib, params.t_cost, params.p_cost, Some(32)).map_err(|_| CryptoError::Kdf)?;
    let mut out = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, p)
        .hash_password_into(password, salt, out.as_mut())
        .map_err(|_| CryptoError::Kdf)?;
    Ok(out)
}

/// Fill `buf` from the crate CSPRNG ([`crate::rng`]: the OS CSPRNG unless
/// a custom entropy source has been installed).
pub fn random_bytes(buf: &mut [u8]) {
    crate::rng::fill(buf);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_salted() {
        let a = password_key(b"pw", &[1; 16], PasswordParams::TEST).unwrap();
        let b = password_key(b"pw", &[1; 16], PasswordParams::TEST).unwrap();
        let c = password_key(b"pw", &[2; 16], PasswordParams::TEST).unwrap();
        assert_eq!(*a, *b);
        assert_ne!(*a, *c);
    }
}
