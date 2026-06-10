//! Player / entity stat blocks — a compact, extensible key→value table.
//!
//! Stats travel inside the signed [`crate::mmo::snapshot::Snapshot`], so they
//! are covered by the same cryptographic commitment as positions: a peer cannot
//! forge another player's health without breaking the host's signature.

use std::collections::BTreeMap;

use super::codec::{ByteReader, ByteWriter, CodecError};

/// Well-known stat keys. Games are free to use any `u16` above [`USER_BASE`]
/// for their own custom stats.
pub mod key {
    pub const HEALTH: u16 = 0;
    pub const MAX_HEALTH: u16 = 1;
    pub const MANA: u16 = 2;
    pub const MAX_MANA: u16 = 3;
    pub const STAMINA: u16 = 4;
    pub const MAX_STAMINA: u16 = 5;
    pub const LEVEL: u16 = 6;
    pub const XP: u16 = 7;
    pub const ARMOR: u16 = 8;
    pub const SPEED: u16 = 9;
    pub const TEAM: u16 = 10;

    /// First key value reserved for game-specific stats.
    pub const USER_BASE: u16 = 1024;
}

/// A sparse map of stat key → signed integer value.
///
/// Backed by a [`BTreeMap`] so iteration order (and therefore the encoded byte
/// layout, and therefore the hash) is deterministic.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    map: BTreeMap<u16, i32>,
}

impl Stats {
    pub fn new() -> Self {
        Self::default()
    }

    /// Read a stat, defaulting to `0` when unset.
    pub fn get(&self, key: u16) -> i32 {
        self.map.get(&key).copied().unwrap_or(0)
    }

    /// Set a stat. Setting any value (including 0) makes the key present.
    pub fn set(&mut self, key: u16, value: i32) {
        self.map.insert(key, value);
    }

    /// Builder-style set.
    pub fn with(mut self, key: u16, value: i32) -> Self {
        self.set(key, value);
        self
    }

    /// Add a delta to a stat (handy for damage / healing), saturating on overflow.
    pub fn add(&mut self, key: u16, delta: i32) {
        let v = self.get(key).saturating_add(delta);
        self.set(key, v);
    }

    pub fn contains(&self, key: u16) -> bool {
        self.map.contains_key(&key)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (u16, i32)> + '_ {
        self.map.iter().map(|(&k, &v)| (k, v))
    }

    pub(crate) fn encode(&self, w: &mut ByteWriter) {
        w.u16(self.map.len() as u16);
        for (&k, &v) in &self.map {
            w.u16(k).i32(v);
        }
    }

    pub(crate) fn decode(r: &mut ByteReader) -> Result<Self, CodecError> {
        let n = r.u16()? as usize;
        let mut map = BTreeMap::new();
        for _ in 0..n {
            let k = r.u16()?;
            let v = r.i32()?;
            map.insert(k, v);
        }
        Ok(Self { map })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_default() {
        let s = Stats::new().with(key::HEALTH, 90).with(key::MAX_HEALTH, 100).with(key::TEAM, 2);
        assert_eq!(s.get(key::HEALTH), 90);
        assert_eq!(s.get(key::MANA), 0); // unset → 0

        let mut w = ByteWriter::new();
        s.encode(&mut w);
        let bytes = w.finish();
        let mut r = ByteReader::new(&bytes);
        let back = Stats::decode(&mut r).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn encoding_is_deterministic_regardless_of_insert_order() {
        let a = Stats::new().with(5, 1).with(1, 2).with(9, 3);
        let b = Stats::new().with(9, 3).with(1, 2).with(5, 1);
        let enc = |s: &Stats| {
            let mut w = ByteWriter::new();
            s.encode(&mut w);
            w.finish()
        };
        assert_eq!(enc(&a), enc(&b));
    }
}
