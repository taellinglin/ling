//! Karaoke kit: `.lrc` timed-lyric parsing + sung-pitch scoring.

use crate::note::hz_to_midi;

/// One timed lyric line.
#[derive(Clone, Debug)]
pub struct LyricLine { pub time: f32, pub text: String }

/// Parsed `.lrc` lyrics (lines sorted by time).
#[derive(Clone, Debug, Default)]
pub struct Lyrics { pub lines: Vec<LyricLine> }

impl Lyrics {
    /// Parse `.lrc` content: lines like `[mm:ss.xx] text` (multiple stamps allowed).
    pub fn parse(text: &str) -> Self {
        let mut lines = Vec::new();
        for raw in text.lines() {
            // collect all [..] timestamps at the start, then the trailing text
            let mut rest = raw;
            let mut stamps = Vec::new();
            while rest.starts_with('[') {
                if let Some(end) = rest.find(']') {
                    let inside = &rest[1..end];
                    if let Some(t) = parse_stamp(inside) { stamps.push(t); }
                    rest = rest[end + 1..].trim_start();
                } else { break; }
            }
            let txt = rest.trim().to_string();
            for t in stamps {
                if !txt.is_empty() { lines.push(LyricLine { time: t, text: txt.clone() }); }
            }
        }
        lines.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
        Self { lines }
    }

    /// The lyric line active at time `t` (the latest line whose time ≤ t).
    pub fn line_at(&self, t: f32) -> &str {
        let mut cur = "";
        for l in &self.lines { if l.time <= t { cur = &l.text; } else { break; } }
        cur
    }

    /// Index of the active line at time `t`, or -1 before the first.
    pub fn index_at(&self, t: f32) -> i32 {
        let mut idx = -1;
        for (i, l) in self.lines.iter().enumerate() { if l.time <= t { idx = i as i32; } else { break; } }
        idx
    }
}

/// Parse `mm:ss.xx` or `mm:ss` → seconds.
fn parse_stamp(s: &str) -> Option<f32> {
    let mut parts = s.split(':');
    let mm: f32 = parts.next()?.trim().parse().ok()?;
    let ss: f32 = parts.next()?.trim().parse().ok()?;
    Some(mm * 60.0 + ss)
}

/// Score a sung pitch against a target, by absolute cents error.
/// Returns a value in [0,1] (1 = bang on, 0 = a semitone or more off).
pub fn pitch_score(sung_hz: f32, target_hz: f32) -> f32 {
    if sung_hz <= 0.0 || target_hz <= 0.0 { return 0.0; }
    let cents = ((hz_to_midi(sung_hz) - hz_to_midi(target_hz)).abs()) * 100.0;
    // ignore octave errors: fold to nearest octave
    let folded = (cents % 1200.0).min(1200.0 - (cents % 1200.0));
    (1.0 - folded / 100.0).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_lrc() {
        let lrc = "[00:01.00]hello\n[00:03.50]world\n[mal]formed\n";
        let ly = Lyrics::parse(lrc);
        assert_eq!(ly.lines.len(), 2);
        assert_eq!(ly.line_at(0.0), "");
        assert_eq!(ly.line_at(2.0), "hello");
        assert_eq!(ly.line_at(4.0), "world");
        assert_eq!(ly.index_at(4.0), 1);
    }
    #[test]
    fn pitch_scoring() {
        assert!((pitch_score(440.0, 440.0) - 1.0).abs() < 1e-4);
        assert!(pitch_score(466.16, 440.0) < 0.2); // a semitone off
        assert_eq!(pitch_score(0.0, 440.0), 0.0);
    }
}
