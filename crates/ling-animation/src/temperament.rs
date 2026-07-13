//! The temperament axis — the idea that makes Anima different.
//!
//! Every chain sits somewhere on a continuum from **organic (灵)** to
//! **mechanical (机)**. The scalar both *selects* a solver bias and *cross-fades*
//! secondary dynamics: organic chains get muscle/jiggle/breath, mechanical chains
//! get exact, rigid coupling. A single creature can mix both per-chain.

/// `0.0` = fully organic (灵), `1.0` = fully mechanical (机).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Temperament(pub f32);

impl Temperament {
    /// Half-and-half — e.g. a hydraulic limb with organic sag.
    pub const HYBRID: Self = Self(0.5);
    pub const MECHANICAL: Self = Self(1.0);
    pub const ORGANIC: Self = Self(0.0);

    pub fn new(t: f32) -> Self {
        Self(t.clamp(0.0, 1.0))
    }

    pub fn value(self) -> f32 {
        self.0
    }

    pub fn is_organic(self) -> bool {
        self.0 < 0.5
    }

    pub fn is_mechanical(self) -> bool {
        self.0 >= 0.5
    }

    /// Blend an organic result with a mechanical one by this temperament.
    /// `0` → all organic, `1` → all mechanical.
    pub fn blend(self, organic: f32, mechanical: f32) -> f32 {
        organic + (mechanical - organic) * self.0
    }

    /// How much secondary "life" (idle sway, jiggle, breath) to apply — fades out
    /// as the chain becomes mechanical.
    pub fn liveliness(self) -> f32 {
        1.0 - self.0
    }

    /// How exact/rigid the coupling should be — rises as the chain becomes mechanical.
    pub fn rigidity(self) -> f32 {
        self.0
    }
}

impl Default for Temperament {
    fn default() -> Self {
        Self::ORGANIC
    }
}

impl From<f32> for Temperament {
    fn from(v: f32) -> Self {
        Self::new(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blend_crossfades() {
        assert_eq!(Temperament::ORGANIC.blend(2.0, 8.0), 2.0);
        assert_eq!(Temperament::MECHANICAL.blend(2.0, 8.0), 8.0);
        assert_eq!(Temperament::HYBRID.blend(2.0, 8.0), 5.0);
    }
}
