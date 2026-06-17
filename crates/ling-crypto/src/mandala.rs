//! Mandala Hash — Ling's custom geometric key derivation.
//!
//! A MandalaHash derives cryptographic key material from a set of visual
//! geometric parameters (rings, spokes, petals, spiral arms, scale).
//! The same mandala pattern always yields the same key, making it possible
//! to use visual/geometric patterns as cryptographic identities.
//!
//! Construction:
//!   1. Serialize parameters deterministically with domain label
//!   2. Hash with BLAKE3 (collision-resistant, fast, XOF-capable)
//!   3. Expand to requested length via BLAKE3 XOF
//!
//! This is a one-way function — you cannot recover the parameters from the key.

const DOMAIN: &str = "ling-mandala-v1";

/// Parameters describing a mandala visual pattern.
#[derive(Debug, Clone, PartialEq)]
pub struct MandalaParams {
    pub n_rings: u8,     // number of concentric rings (1–255)
    pub n_spokes: u8,    // chakra spokes (1–255)
    pub n_petals: u8,    // lotus petals (1–255)
    pub n_spiral: u8,    // spiral arms (0–255)
    pub inner_r: f32,    // inner radius (normalized 0.0–1.0)
    pub outer_r: f32,    // outer radius (normalized 0.0–1.0)
    pub twist: f32,      // spiral twist factor
    pub hue_offset: f32, // starting hue (0.0–1.0, visual only — included for uniqueness)
    pub seed: u64,       // extra entropy / version
}

impl MandalaParams {
    pub fn new(n_rings: u8, n_spokes: u8, n_petals: u8) -> Self {
        Self {
            n_rings,
            n_spokes,
            n_petals,
            n_spiral: 4,
            inner_r: 0.1,
            outer_r: 0.9,
            twist: 0.0,
            hue_offset: 0.0,
            seed: 0,
        }
    }

    fn serialize(&self) -> Vec<u8> {
        let mut v = DOMAIN.as_bytes().to_vec();
        v.push(self.n_rings);
        v.push(self.n_spokes);
        v.push(self.n_petals);
        v.push(self.n_spiral);
        v.extend_from_slice(&self.inner_r.to_bits().to_le_bytes());
        v.extend_from_slice(&self.outer_r.to_bits().to_le_bytes());
        v.extend_from_slice(&self.twist.to_bits().to_le_bytes());
        v.extend_from_slice(&self.hue_offset.to_bits().to_le_bytes());
        v.extend_from_slice(&self.seed.to_le_bytes());
        v
    }
}

pub struct MandalaHash {
    params: MandalaParams,
    base: [u8; 32],
}

impl MandalaHash {
    pub fn new(params: MandalaParams) -> Self {
        let serialized = params.serialize();
        let base = *blake3::hash(&serialized).as_bytes();
        Self { params, base }
    }

    /// 32-byte key derived from this mandala.
    pub fn key(&self) -> [u8; 32] {
        self.base
    }

    /// Derive `len` bytes of key material (XOF expansion).
    pub fn expand(&self, len: usize) -> Vec<u8> {
        let mut hasher = blake3::Hasher::new_keyed(&self.base);
        hasher.update(b"expand");
        let mut out = vec![0u8; len];
        hasher.finalize_xof().fill(&mut out);
        out
    }

    /// Derive a named subkey (different contexts → independent keys).
    pub fn subkey(&self, context: &str) -> [u8; 32] {
        *blake3::keyed_hash(&self.base, context.as_bytes()).as_bytes()
    }

    /// Derive an encryption key (AES-GCM or XChaCha20 ready).
    pub fn encryption_key(&self) -> [u8; 32] {
        self.subkey("ling:encryption:v1")
    }

    /// Derive a signing seed (Ed25519 ready).
    pub fn signing_seed(&self) -> [u8; 32] {
        self.subkey("ling:signing:v1")
    }

    /// Derive a KEM seed (ML-KEM ready).
    pub fn kem_seed(&self) -> [u8; 32] {
        self.subkey("ling:kem:v1")
    }

    pub fn params(&self) -> &MandalaParams {
        &self.params
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic() {
        let p = MandalaParams::new(8, 12, 8);
        let h1 = MandalaHash::new(p.clone());
        let h2 = MandalaHash::new(p);
        assert_eq!(h1.key(), h2.key());
    }

    #[test]
    fn different_params_different_keys() {
        let h1 = MandalaHash::new(MandalaParams::new(8, 12, 8));
        let h2 = MandalaHash::new(MandalaParams::new(9, 12, 8));
        assert_ne!(h1.key(), h2.key());
    }

    #[test]
    fn subkeys_independent() {
        let h = MandalaHash::new(MandalaParams::new(8, 12, 8));
        assert_ne!(h.encryption_key(), h.signing_seed());
        assert_ne!(h.signing_seed(), h.kem_seed());
    }
}
