//! The mashup's sound effects, played by Ling Cinema (dsp/cinema.rs): the moves a DJ edit puts on its transitions.
//! - an impact on every downbeat a build (a loop roll) or a big energy jump lands on, and a sub drop under it when it is a drop;
//! - a downlifter where a drop falls back into something quiet;
//! - a reverse swell into the downbeat the acapella comes back after the chops (the "Pop Culture" move: the chops end, the vocal
//!   returns out of a swell);
//! - a short air swipe where one song cuts straight into the next (no roll between them);
//! - an ambience held through the intro and the breaks;
//! - film music: a braam on the loudest section instead of the club moves.
//! Acoustic genres and chiptune get none.
use super::acapella::Vox;
use crate::songgen::Cue;

/// A section of the medley as the cues see it
#[derive(Clone, Debug)]
pub struct CueSec {
    pub start: f64,
    pub end: f64,
    /// medley::slot_tag's names, or "build" / "break" for a roll
    pub tag: &'static str,
    /// the slot it plays (None for a roll)
    pub slot: Option<usize>,
    pub energy: f64,
    /// the section's tonic (0..11)
    pub tonic: u8,
}

fn acoustic(genre: &str) -> bool {
    matches!(genre, "folk" | "ballad" | "jazz" | "rock" | "country" | "acoustic" | "chiptune")
}

/// The cues over the medley's sections; `vocal` is the acapella plan per section (empty without acapellas).
pub fn cues(secs: &[CueSec], vocal: &[Vox], genre: &str) -> Vec<Cue> {
    if acoustic(genre) || secs.is_empty() {
        return vec![];
    }
    let film = matches!(genre, "cinematic" | "orchestral");
    let ambient = matches!(genre, "ambient" | "lofi");
    let club = !film && !ambient;
    let n = secs.len();
    let mut out: Vec<Cue> = vec![];
    let at = |out: &Vec<Cue>, t: f64| out.iter().any(|c| (c.what == "impact" || c.what == "swell") && (c.start - t).abs() < 0.01);
    for i in 0..n {
        let s = &secs[i];
        let beats = s.end - s.start;
        if i > 0 && !ambient {
            let p = &secs[i - 1];
            let built = p.tag == "build";
            // the impact on the landing, a sub drop under a drop
            if s.energy >= 0.7 && (built || s.energy >= p.energy + 0.3) {
                out.push(Cue { what: "impact", start: s.start, len: 4.0f64.min(beats), key: 24 + s.tonic % 12, vel: if built { 0.95 } else { 0.75 } });
                if club && built && s.tag == "drop" {
                    out.push(Cue { what: "sub drop", start: s.start, len: 4.0f64.min(beats), key: 48 + s.tonic % 12, vel: 0.8 });
                }
            }
            // the acapella back after the chops: a reverse swell into its downbeat
            if vocal.len() == n && !vocal[i - 1].sings() && vocal[i].sings() && !at(&out, s.start) {
                let len = 2.0f64.min(p.end - p.start);
                out.push(Cue { what: "swell", start: s.start - len, len, key: 60 + s.tonic % 12, vel: 0.65 });
            }
            // one song cutting straight into the next: a swipe across the cut
            if let (Some(a), Some(b)) = (p.slot, s.slot) {
                if a != b && !at(&out, s.start) {
                    out.push(Cue { what: "whoosh", start: s.start - 0.5, len: 1.0, key: 60 + s.tonic % 12, vel: 0.6 });
                }
            }
        }
        // a drop falling back into something quiet
        if club && s.energy >= 0.8 {
            if let Some(nx) = secs.get(i + 1) {
                if nx.energy <= s.energy - 0.3 {
                    out.push(Cue { what: "downlifter", start: nx.start, len: 4.0f64.min(nx.end - nx.start), key: 60 + nx.tonic % 12, vel: 0.8 });
                }
            }
        }
        // the ambience under the intro and the breaks
        if (s.tag == "intro" || s.tag == "break" || (ambient && s.energy < 0.5)) && beats >= 4.0 {
            out.push(Cue { what: "ambience", start: s.start, len: beats, key: 36 + s.tonic % 12, vel: 0.7 });
        }
    }
    if film {
        if let Some(i) = (1..n).max_by(|a, b| secs[*a].energy.total_cmp(&secs[*b].energy)) {
            out.push(Cue { what: "braam", start: secs[i].start, len: 8.0f64.min(secs[i].end - secs[i].start), key: 36 + secs[i].tonic % 12, vel: 0.85 });
        }
    }
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    out
}

/// The Ling Cinema preset of a cue (the song generator's for the kinds they share)
pub fn preset(what: &str, genre: &str) -> &'static str {
    match what {
        "sub drop" => "Sub Drop",
        "swell" => "Reverse Swell",
        "whoosh" => "Air Swipe",
        _ => crate::songgen::cue_preset(what, genre),
    }
}

/// The track name and the level against the music of a cue kind, in the order the tracks are made
pub const KINDS: [(&str, &str, f32); 7] = [
    ("impact", "Impact", 0.8),
    ("sub drop", "Sub Drop", 0.75),
    ("downlifter", "Downlifter", 0.8),
    ("swell", "Vocal Swell", 0.7),
    ("whoosh", "Swipe", 0.6),
    ("ambience", "Ambience", 0.55),
    ("braam", "Braam", 0.8),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn sec(start: f64, end: f64, tag: &'static str, slot: Option<usize>, energy: f64) -> CueSec {
        CueSec { start, end, tag, slot, energy, tonic: 9 }
    }

    // intro (slot 0) > verse (slot 1, a straight cut) > build roll > drop (slot 2) > break (slot 3) > drop (slot 4)
    fn medley() -> Vec<CueSec> {
        vec![
            sec(0.0, 16.0, "intro", Some(0), 0.4),
            sec(16.0, 32.0, "verse", Some(1), 0.6),
            sec(32.0, 40.0, "build", None, 0.7),
            sec(40.0, 56.0, "drop", Some(2), 0.95),
            sec(56.0, 72.0, "break", Some(3), 0.4),
            sec(72.0, 88.0, "drop", Some(4), 0.9),
        ]
    }

    #[test]
    fn club_moves() {
        let c = cues(&medley(), &[], "dubstep");
        let has = |w: &str, t: f64| c.iter().any(|x| x.what == w && (x.start - t).abs() < 1e-9);
        assert!(has("ambience", 0.0) && has("ambience", 56.0));
        assert!(has("whoosh", 15.5), "a swipe across the straight cut: {c:?}");
        assert!(has("impact", 40.0) && has("sub drop", 40.0), "the drop after the build lands with a hit and a sub drop");
        assert!(has("downlifter", 56.0), "the drop falls back into the break");
        // the jump from the break into the last drop: a hit, but no sub drop (no build before it) and no swipe on top of the hit
        assert!(has("impact", 72.0) && !has("sub drop", 72.0) && !has("whoosh", 71.5));
        for x in &c {
            assert!(x.len > 0.0 && x.start >= 0.0 && x.key < 100, "{x:?}");
            assert!(preset(x.what, "dubstep") != "", "{x:?}");
        }
    }

    #[test]
    fn swell_where_the_vocal_returns() {
        let v = [Vox::Full, Vox::Full, Vox::Chops, Vox::Chops, Vox::Chops, Vox::Full];
        let s = vec![sec(0.0, 16.0, "verse", Some(0), 0.6), sec(16.0, 32.0, "verse", Some(0), 0.6), sec(32.0, 48.0, "verse", Some(0), 0.6), sec(48.0, 64.0, "verse", Some(0), 0.6), sec(64.0, 80.0, "verse", Some(0), 0.6), sec(80.0, 96.0, "verse", Some(0), 0.6)];
        let c = cues(&s, &v, "house");
        let sw: Vec<&Cue> = c.iter().filter(|x| x.what == "swell").collect();
        assert_eq!(sw.len(), 1, "{c:?}");
        assert!((sw[0].start - 78.0).abs() < 1e-9 && (sw[0].len - 2.0).abs() < 1e-9, "it ends on the downbeat the vocal comes back");
        // where a hit already lands on the return, no swell
        let v2 = [Vox::Full, Vox::Full, Vox::Chops, Vox::Full, Vox::Full, Vox::Full];
        assert!(!cues(&medley(), &v2, "house").iter().any(|x| x.what == "swell"));
    }

    #[test]
    fn genres() {
        assert!(cues(&medley(), &[], "folk").is_empty());
        assert!(cues(&medley(), &[], "chiptune").is_empty());
        let amb = cues(&medley(), &[], "ambient");
        assert!(amb.iter().all(|x| x.what == "ambience" || x.what == "swell"), "{amb:?}");
        let film = cues(&medley(), &[], "cinematic");
        assert_eq!(film.iter().filter(|x| x.what == "braam").count(), 1);
        assert!(!film.iter().any(|x| x.what == "sub drop" || x.what == "downlifter"));
    }
}
