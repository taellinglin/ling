//! Domain-separated SHAKE256 (DESIGN §2).
//!
//! `input = enc(domain) ‖ enc(part_1) ‖ … ‖ enc(part_n)`, `enc(x) = u32_le(len) ‖ x`.
//! Length-prefixing every field makes the encoding injective, so two
//! different (domain, parts) inputs can never collide by concatenation.

use alloc::{vec, vec::Vec};

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
