//! Physics state snapshots: per-entity transforms, delta compression against a
//! baseline, and a client-side interpolation buffer for smooth rendering.
//!
//! A [`Snapshot`] is the unit of state the host signs and ships to clients each
//! network tick. It is intentionally transform-centric (position / velocity /
//! orientation) plus a sidecar of [`Stats`] so the hot path stays small while
//! still letting the cryptographic commitment cover gameplay-relevant numbers.

use std::collections::{HashMap, HashSet, VecDeque};

use glam::{Quat, Vec3};

use super::codec::{ByteReader, ByteWriter, CodecError};
use super::stats::Stats;

// Entity flag bits (extend freely; the high bits are yours).
pub const FLAG_GROUNDED: u16 = 1 << 0;
pub const FLAG_SPRINTING: u16 = 1 << 1;
pub const FLAG_AIRBORNE: u16 = 1 << 2;
pub const FLAG_DEAD: u16 = 1 << 3;
pub const FLAG_HIDDEN: u16 = 1 << 4;

/// Position tolerance (world units) below which two transforms are treated as
/// equal for delta compression.
const POS_EPS: f32 = 1e-3;
/// Orientation tolerance (1 - |dot|) below which two rotations are equal.
const ROT_EPS: f32 = 1e-4;

/// The networked transform of a single entity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EntityState {
    pub id: u64,
    pub flags: u16,
    pub pos: Vec3,
    pub vel: Vec3,
    pub rot: Quat,
}

impl EntityState {
    pub fn new(id: u64) -> Self {
        Self { id, flags: 0, pos: Vec3::ZERO, vel: Vec3::ZERO, rot: Quat::IDENTITY }
    }

    pub fn at(id: u64, pos: Vec3) -> Self {
        Self { pos, ..Self::new(id) }
    }

    /// True when `self` and `other` are close enough that re-sending `self`
    /// in a delta would be wasteful.
    pub fn approx_eq(&self, other: &EntityState) -> bool {
        self.flags == other.flags
            && self.pos.distance_squared(other.pos) < POS_EPS * POS_EPS
            && self.vel.distance_squared(other.vel) < POS_EPS * POS_EPS
            && (1.0 - self.rot.dot(other.rot).abs()) < ROT_EPS
    }

    /// Linear/spherical blend used by the interpolation buffer.
    pub fn lerp(&self, other: &EntityState, t: f32) -> EntityState {
        EntityState {
            id: self.id,
            // Discrete flags can't be interpolated — take the more recent sample.
            flags: if t < 0.5 { self.flags } else { other.flags },
            pos: self.pos.lerp(other.pos, t),
            vel: self.vel.lerp(other.vel, t),
            rot: self.rot.slerp(other.rot, t),
        }
    }

    pub(crate) fn encode(&self, w: &mut ByteWriter) {
        w.u64(self.id).u16(self.flags).vec3(self.pos).vec3(self.vel).quat(self.rot);
    }

    pub(crate) fn decode(r: &mut ByteReader) -> Result<Self, CodecError> {
        Ok(Self {
            id: r.u64()?,
            flags: r.u16()?,
            pos: r.vec3()?,
            vel: r.vec3()?,
            rot: r.quat()?,
        })
    }
}

/// A set of entity states at a single logical tick.
///
/// A *keyframe* carries the full world (or the full subset relevant to a
/// client). A *delta* carries only entities that changed since `baseline_tick`,
/// plus the ids that were removed.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub tick: u64,
    pub baseline_tick: u64,
    pub keyframe: bool,
    pub states: Vec<EntityState>,
    pub removed: Vec<u64>,
    /// Sidecar stat blocks, by entity id. Not every entity needs an entry.
    pub stats: Vec<(u64, Stats)>,
}

impl Snapshot {
    /// A full, self-contained snapshot at `tick`.
    pub fn keyframe(tick: u64, states: Vec<EntityState>) -> Self {
        Self { tick, baseline_tick: 0, keyframe: true, states, removed: Vec::new(), stats: Vec::new() }
    }

    pub fn with_stats(mut self, stats: Vec<(u64, Stats)>) -> Self {
        self.stats = stats;
        self
    }

    pub fn get(&self, id: u64) -> Option<&EntityState> {
        self.states.iter().find(|e| e.id == id)
    }

    pub fn stats_of(&self, id: u64) -> Option<&Stats> {
        self.stats.iter().find(|(eid, _)| *eid == id).map(|(_, s)| s)
    }

    // ── Delta compression ────────────────────────────────────────────────────

    /// Produce a delta that, applied onto `baseline`, reconstructs `current`.
    ///
    /// Only entities new-or-changed vs the baseline are included; ids present in
    /// the baseline but gone from `current` go into `removed`. Stats are sent
    /// for any entity whose stats differ from the baseline.
    pub fn delta_from(baseline: &Snapshot, current: &Snapshot) -> Snapshot {
        let base: HashMap<u64, &EntityState> =
            baseline.states.iter().map(|e| (e.id, e)).collect();
        let base_stats: HashMap<u64, &Stats> =
            baseline.stats.iter().map(|(id, s)| (*id, s)).collect();

        let mut states = Vec::new();
        for e in &current.states {
            match base.get(&e.id) {
                Some(b) if b.approx_eq(e) => {}
                _ => states.push(*e),
            }
        }

        let cur_ids: HashSet<u64> = current.states.iter().map(|e| e.id).collect();
        let removed: Vec<u64> = baseline
            .states
            .iter()
            .map(|e| e.id)
            .filter(|id| !cur_ids.contains(id))
            .collect();

        let mut stats = Vec::new();
        for (id, s) in &current.stats {
            match base_stats.get(id) {
                Some(b) if *b == s => {}
                _ => stats.push((*id, s.clone())),
            }
        }

        Snapshot {
            tick: current.tick,
            baseline_tick: baseline.tick,
            keyframe: false,
            states,
            removed,
            stats,
        }
    }

    /// Reconstruct the full snapshot by applying `delta` onto `baseline`.
    /// The result is a keyframe at `delta.tick`.
    pub fn apply_delta(baseline: &Snapshot, delta: &Snapshot) -> Snapshot {
        let mut by_id: HashMap<u64, EntityState> =
            baseline.states.iter().map(|e| (e.id, *e)).collect();
        for e in &delta.states {
            by_id.insert(e.id, *e);
        }
        for id in &delta.removed {
            by_id.remove(id);
        }
        let mut states: Vec<EntityState> = by_id.into_values().collect();
        states.sort_by_key(|e| e.id);

        let mut stats_map: HashMap<u64, Stats> =
            baseline.stats.iter().cloned().collect();
        for (id, s) in &delta.stats {
            stats_map.insert(*id, s.clone());
        }
        for id in &delta.removed {
            stats_map.remove(id);
        }
        let mut stats: Vec<(u64, Stats)> = stats_map.into_iter().collect();
        stats.sort_by_key(|(id, _)| *id);

        Snapshot { tick: delta.tick, baseline_tick: 0, keyframe: true, states, removed: Vec::new(), stats }
    }

    // ── Wire format ──────────────────────────────────────────────────────────

    pub(crate) fn encode(&self, w: &mut ByteWriter) {
        w.u64(self.tick).u64(self.baseline_tick).bool(self.keyframe);
        w.u32(self.states.len() as u32);
        for e in &self.states {
            e.encode(w);
        }
        w.u32(self.removed.len() as u32);
        for id in &self.removed {
            w.u64(*id);
        }
        w.u32(self.stats.len() as u32);
        for (id, s) in &self.stats {
            w.u64(*id);
            s.encode(w);
        }
    }

    pub(crate) fn decode(r: &mut ByteReader) -> Result<Self, CodecError> {
        let tick = r.u64()?;
        let baseline_tick = r.u64()?;
        let keyframe = r.bool()?;
        let n = r.u32()? as usize;
        let mut states = Vec::with_capacity(n.min(4096));
        for _ in 0..n {
            states.push(EntityState::decode(r)?);
        }
        let rn = r.u32()? as usize;
        let mut removed = Vec::with_capacity(rn.min(4096));
        for _ in 0..rn {
            removed.push(r.u64()?);
        }
        let sn = r.u32()? as usize;
        let mut stats = Vec::with_capacity(sn.min(4096));
        for _ in 0..sn {
            let id = r.u64()?;
            stats.push((id, Stats::decode(r)?));
        }
        Ok(Self { tick, baseline_tick, keyframe, states, removed, stats })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = ByteWriter::new();
        self.encode(&mut w);
        w.finish()
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut r = ByteReader::new(bytes);
        let s = Self::decode(&mut r)?;
        r.expect_end()?;
        Ok(s)
    }
}

// ─── Interpolation buffer ─────────────────────────────────────────────────────

struct TimedSnapshot {
    /// Local wall-clock time (seconds) the snapshot became available.
    recv_time: f64,
    snap: Snapshot,
}

/// Holds recent (full) snapshots and reconstructs smoothly interpolated entity
/// states at a render time held slightly in the past.
///
/// Rendering "in the past" by `delay` seconds means there are almost always two
/// snapshots bracketing the render time, so motion stays smooth even when the
/// network jitters.
pub struct InterpolationBuffer {
    delay: f64,
    capacity: usize,
    frames: VecDeque<TimedSnapshot>,
}

impl InterpolationBuffer {
    /// `delay` is how far behind real time to render (e.g. 0.1 s ≈ two ticks at
    /// 20 Hz). Larger = smoother but laggier.
    pub fn new(delay: f64) -> Self {
        Self { delay, capacity: 32, frames: VecDeque::new() }
    }

    pub fn delay(&self) -> f64 {
        self.delay
    }

    pub fn set_delay(&mut self, delay: f64) {
        self.delay = delay;
    }

    pub fn clear(&mut self) {
        self.frames.clear();
    }

    pub fn latest_tick(&self) -> Option<u64> {
        self.frames.back().map(|f| f.snap.tick)
    }

    /// Insert a reconstructed full snapshot. Out-of-order / stale snapshots
    /// (older tick than the newest held) are dropped.
    pub fn push(&mut self, snap: Snapshot, now: f64) {
        if let Some(back) = self.frames.back() {
            if snap.tick <= back.snap.tick {
                return; // stale or duplicate
            }
        }
        self.frames.push_back(TimedSnapshot { recv_time: now, snap });
        while self.frames.len() > self.capacity {
            self.frames.pop_front();
        }
    }

    /// Sample interpolated entity states for render time `now`.
    ///
    /// Entities present in both bracketing snapshots are blended; entities only
    /// in the newer one are taken as-is (they just spawned / entered interest).
    pub fn sample(&self, now: f64) -> Vec<EntityState> {
        let target = now - self.delay;

        if self.frames.is_empty() {
            return Vec::new();
        }
        if self.frames.len() == 1 {
            return self.frames[0].snap.states.clone();
        }

        // Find the pair (a, b) with a.recv_time <= target <= b.recv_time.
        for w in 0..self.frames.len() - 1 {
            let a = &self.frames[w];
            let b = &self.frames[w + 1];
            if target >= a.recv_time && target <= b.recv_time {
                let span = (b.recv_time - a.recv_time).max(1e-6);
                let t = ((target - a.recv_time) / span).clamp(0.0, 1.0) as f32;
                return blend(&a.snap, &b.snap, t);
            }
        }

        // Target is before the oldest frame → show oldest; after newest → newest.
        if target < self.frames[0].recv_time {
            self.frames[0].snap.states.clone()
        } else {
            self.frames.back().unwrap().snap.states.clone()
        }
    }
}

fn blend(a: &Snapshot, b: &Snapshot, t: f32) -> Vec<EntityState> {
    let a_map: HashMap<u64, &EntityState> = a.states.iter().map(|e| (e.id, e)).collect();
    b.states
        .iter()
        .map(|be| match a_map.get(&be.id) {
            Some(ae) => ae.lerp(be, t),
            None => *be,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ent(id: u64, x: f32) -> EntityState {
        EntityState::at(id, Vec3::new(x, 0.0, 0.0))
    }

    #[test]
    fn snapshot_roundtrip() {
        let snap = Snapshot::keyframe(7, vec![ent(1, 1.0), ent(2, 2.0)])
            .with_stats(vec![(1, Stats::new().with(0, 50))]);
        let bytes = snap.to_bytes();
        let back = Snapshot::from_bytes(&bytes).unwrap();
        assert_eq!(back.tick, 7);
        assert_eq!(back.states.len(), 2);
        assert_eq!(back.stats_of(1).unwrap().get(0), 50);
    }

    #[test]
    fn delta_then_apply_reconstructs() {
        let base = Snapshot::keyframe(1, vec![ent(1, 0.0), ent(2, 0.0), ent(3, 0.0)]);
        // entity 1 moved, entity 2 unchanged, entity 3 removed, entity 4 added
        let current = Snapshot::keyframe(2, vec![ent(1, 5.0), ent(2, 0.0), ent(4, 9.0)]);

        let delta = Snapshot::delta_from(&base, &current);
        assert!(!delta.keyframe);
        // only moved (1) and new (4) are carried; unchanged (2) is omitted
        assert_eq!(delta.states.len(), 2);
        assert_eq!(delta.removed, vec![3]);

        let rebuilt = Snapshot::apply_delta(&base, &delta);
        let ids: Vec<u64> = rebuilt.states.iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![1, 2, 4]);
        assert_eq!(rebuilt.get(1).unwrap().pos.x, 5.0);
    }

    #[test]
    fn interpolation_midpoint() {
        let mut buf = InterpolationBuffer::new(1.0);
        buf.push(Snapshot::keyframe(1, vec![ent(1, 0.0)]), 0.0);
        buf.push(Snapshot::keyframe(2, vec![ent(1, 10.0)]), 1.0);
        // render time 2.0, delay 1.0 → target 1.0 ... use 1.5 to land mid-span
        let s = buf.sample(1.5);
        assert!((s[0].pos.x - 5.0).abs() < 1e-4, "got {}", s[0].pos.x);
    }

    #[test]
    fn stale_snapshot_dropped() {
        let mut buf = InterpolationBuffer::new(0.1);
        buf.push(Snapshot::keyframe(5, vec![ent(1, 0.0)]), 0.0);
        buf.push(Snapshot::keyframe(3, vec![ent(1, 9.0)]), 0.1); // older tick
        assert_eq!(buf.latest_tick(), Some(5));
    }
}
