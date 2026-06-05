//! Rhythm-game kit: a beatmap model, hit-timing judgement and a scorer.
//!
//! Timing windows (ms, |Δ| ≤): Perfect 30, Great 60, Good 100, Ok 140; beyond → Miss.

/// One tappable note in a beatmap.
#[derive(Clone, Copy, Debug)]
pub struct HitNote { pub lane: u8, pub time: f32 }

/// A lane-based beatmap.
#[derive(Clone, Debug, Default)]
pub struct Beatmap { pub lanes: u8, pub notes: Vec<HitNote> }

impl Beatmap {
    /// Auto-generate a beatmap by spreading beat timestamps across `lanes`
    /// (a deterministic pseudo-pattern so a song's grid becomes playable).
    pub fn from_beats(beats: &[f32], lanes: u8) -> Self {
        let lanes = lanes.max(1);
        let notes = beats.iter().enumerate()
            .map(|(i, &t)| HitNote { lane: ((i * 5 + 2) % lanes as usize) as u8, time: t })
            .collect();
        Self { lanes, notes }
    }
}

/// Judgement grade for a hit, best → worst.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grade { Perfect, Great, Good, Ok, Miss }

impl Grade {
    /// Judge by absolute timing error in milliseconds.
    pub fn judge(delta_ms: f32) -> Grade {
        let d = delta_ms.abs();
        if d <= 30.0 { Grade::Perfect }
        else if d <= 60.0 { Grade::Great }
        else if d <= 100.0 { Grade::Good }
        else if d <= 140.0 { Grade::Ok }
        else { Grade::Miss }
    }
    pub fn name(self) -> &'static str {
        match self { Grade::Perfect => "PERFECT", Grade::Great => "GREAT", Grade::Good => "GOOD", Grade::Ok => "OK", Grade::Miss => "MISS" }
    }
    /// Index 0=Perfect … 4=Miss (handy as a return value for scripts).
    pub fn index(self) -> i32 {
        match self { Grade::Perfect => 0, Grade::Great => 1, Grade::Good => 2, Grade::Ok => 3, Grade::Miss => 4 }
    }
    pub fn from_index(i: i32) -> Grade {
        match i { 0 => Grade::Perfect, 1 => Grade::Great, 2 => Grade::Good, 3 => Grade::Ok, _ => Grade::Miss }
    }
    fn points(self) -> u32 {
        match self { Grade::Perfect => 300, Grade::Great => 200, Grade::Good => 100, Grade::Ok => 50, Grade::Miss => 0 }
    }
    fn accuracy(self) -> f32 {
        match self { Grade::Perfect => 1.0, Grade::Great => 0.85, Grade::Good => 0.6, Grade::Ok => 0.3, Grade::Miss => 0.0 }
    }
}

/// Running score / combo / accuracy state.
#[derive(Clone, Debug, Default)]
pub struct Scorer {
    pub score: u32,
    pub combo: u32,
    pub max_combo: u32,
    pub hits: u32,
    acc_sum: f32,
    pub last: Option<Grade>,
}

impl Scorer {
    pub fn new() -> Self { Self::default() }

    /// Register a graded hit; combo multiplier grows the score.
    pub fn register(&mut self, g: Grade) {
        if g == Grade::Miss {
            self.combo = 0;
        } else {
            self.combo += 1;
            self.max_combo = self.max_combo.max(self.combo);
        }
        let mult = 1.0 + (self.combo as f32 / 25.0); // gentle combo bonus
        self.score += (g.points() as f32 * mult) as u32;
        self.hits += 1;
        self.acc_sum += g.accuracy();
        self.last = Some(g);
    }

    /// Overall accuracy in [0,1].
    pub fn accuracy(&self) -> f32 {
        if self.hits == 0 { 0.0 } else { self.acc_sum / self.hits as f32 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows() {
        assert_eq!(Grade::judge(0.0), Grade::Perfect);
        assert_eq!(Grade::judge(-45.0), Grade::Great);
        assert_eq!(Grade::judge(90.0), Grade::Good);
        assert_eq!(Grade::judge(130.0), Grade::Ok);
        assert_eq!(Grade::judge(200.0), Grade::Miss);
    }
    #[test]
    fn scoring_and_combo() {
        let mut s = Scorer::new();
        s.register(Grade::Perfect);
        s.register(Grade::Perfect);
        s.register(Grade::Miss);
        assert_eq!(s.combo, 0);
        assert_eq!(s.max_combo, 2);
        assert!(s.score > 0);
        assert!(s.accuracy() > 0.0 && s.accuracy() <= 1.0);
    }
    #[test]
    fn auto_beatmap() {
        let beats: Vec<f32> = (0..16).map(|i| i as f32 * 0.5).collect();
        let bm = Beatmap::from_beats(&beats, 4);
        assert_eq!(bm.notes.len(), 16);
        assert!(bm.notes.iter().all(|n| n.lane < 4));
    }
}
