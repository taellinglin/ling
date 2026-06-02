//! Key derivation: Argon2id (password hashing) and HKDF-SHA3-256.

use argon2::{Argon2, PasswordHasher, PasswordVerifier, password_hash::{SaltString, PasswordHash}};
use hkdf::Hkdf;
use sha3::Sha3_256;
use rand::rngs::OsRng;
use zeroize::Zeroizing;

// ── Argon2id ──────────────────────────────────────────────────────────────────

pub struct Argon2idParams {
    pub m_cost: u32,  // memory in KiB (default 65536 = 64 MiB)
    pub t_cost: u32,  // iterations (default 3)
    pub p_cost: u32,  // parallelism (default 4)
}

impl Default for Argon2idParams {
    fn default() -> Self {
        Self { m_cost: 65536, t_cost: 3, p_cost: 4 }
    }
}

impl Argon2idParams {
    /// Hash a password. Returns a PHC-formatted string (includes salt).
    pub fn hash_password(&self, password: &[u8]) -> Result<String, &'static str> {
        let params = argon2::Params::new(self.m_cost, self.t_cost, self.p_cost, None)
            .map_err(|_| "invalid argon2 params")?;
        let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
        let salt = SaltString::generate(&mut OsRng);
        argon2.hash_password(password, &salt)
            .map(|h| h.to_string())
            .map_err(|_| "argon2 hashing failed")
    }

    /// Verify a password against a PHC hash string.
    pub fn verify_password(password: &[u8], hash_str: &str) -> Result<(), &'static str> {
        let parsed = PasswordHash::new(hash_str).map_err(|_| "invalid hash string")?;
        Argon2::default().verify_password(password, &parsed).map_err(|_| "password incorrect")
    }

    /// Derive `out_len` bytes of key material from a password and salt.
    pub fn derive_key(&self, password: &[u8], salt: &[u8], out_len: usize) -> Result<Zeroizing<Vec<u8>>, &'static str> {
        let params = argon2::Params::new(self.m_cost, self.t_cost, self.p_cost, Some(out_len))
            .map_err(|_| "invalid params")?;
        let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
        let mut out = Zeroizing::new(vec![0u8; out_len]);
        argon2.hash_password_into(password, salt, &mut out).map_err(|_| "argon2 derive failed")?;
        Ok(out)
    }
}

// ── HKDF-SHA3-256 ─────────────────────────────────────────────────────────────

/// Extract and expand key material using HKDF with SHA3-256.
///
/// - `ikm`   — input key material (secret)
/// - `salt`  — optional salt (use `&[]` for no salt)
/// - `info`  — context/application label (not secret)
/// - `len`   — desired output length in bytes
pub fn hkdf_sha3(ikm: &[u8], salt: &[u8], info: &[u8], len: usize) -> Result<Zeroizing<Vec<u8>>, &'static str> {
    let salt_opt = if salt.is_empty() { None } else { Some(salt) };
    let (_, hk) = Hkdf::<Sha3_256>::extract(salt_opt, ikm);
    let mut out = Zeroizing::new(vec![0u8; len]);
    hk.expand(info, &mut out).map_err(|_| "hkdf expand failed")?;
    Ok(out)
}
