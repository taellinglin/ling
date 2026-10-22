//! Note scripts: the LingStationX "Scripts" (arpeggiator, humanize, reverse, euclid, transpose ...) as pure edits of a `Song`.
//! Each returns how many cells it changed so the UI can say so.
use crate::{Cell, Song, CHANNELS, NOTE_OFF};

fn is_note(c: Cell) -> bool {
    c.note > 0 && c.note != NOTE_OFF
}

/// chord shapes the arpeggiator walks: 0 = major, 1 = minor, 2 = major 7th, 3 = minor 7th
pub const ARP_CHORDS: [&str; 4] = ["major", "minor", "maj7", "min7"];
const SHAPES: [[i32; 4]; 4] = [[0, 4, 7, 12], [0, 3, 7, 12], [0, 4, 7, 11], [0, 3, 7, 10]];

/// Turn every held note into a running arpeggio of its chord (`shape`), `dir` = 0 up, 1 down, 2 up-down, until the next note, note-off or the end.
pub fn arpeggiate(s: &mut Song, ch: usize, shape: usize, dir: u8) -> usize {
    if ch >= CHANNELS {
        return 0;
    }
    let tones = SHAPES[shape % 4];
    let len = s.length as usize;
    let mut changed = 0;
    let mut r = 0;
    while r < len {
        let c = s.cells[r][ch];
        if !is_note(c) {
            r += 1;
            continue;
        }
        let mut end = r + 1;
        while end < len && s.cells[end][ch].note == 0 {
            end += 1;
        }
        let root = c.note as i32;
        let mut k = 0usize;
        for row in r..end {
            let idx = match dir {
                1 => 3 - (k % 4),
                2 => {
                    let p = k % 6;
                    if p < 4 {
                        p
                    } else {
                        6 - p
                    }
                },
                _ => k % 4,
            };
            let key = (root + tones[idx]).clamp(1, 127) as u8;
            if s.cells[row][ch] != (Cell { note: key, vel: c.vel }) {
                s.cells[row][ch] = Cell { note: key, vel: c.vel };
                changed += 1;
            }
            k += 1;
        }
        r = end;
    }
    changed
}

/// Nudge velocities by up to +-`amount` (1..=7) with a repeatable pseudo-random walk.
pub fn humanize(s: &mut Song, ch: usize, amount: u8) -> usize {
    if ch >= CHANNELS {
        return 0;
    }
    let amount = amount.clamp(1, 7) as i32;
    let mut x = 0x2545_F491u32 ^ (ch as u32 * 7919);
    let mut changed = 0;
    for r in 0..s.length as usize {
        let c = &mut s.cells[r][ch];
        if is_note(*c) {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let d = (x % (2 * amount as u32 + 1)) as i32 - amount;
            let v = ((if c.vel == 0 { 11 } else { c.vel }) as i32 + d).clamp(1, 15) as u8;
            if v != c.vel {
                c.vel = v;
                changed += 1;
            }
        }
    }
    changed
}

/// Play the channel's pattern backwards.
pub fn reverse(s: &mut Song, ch: usize) -> usize {
    if ch >= CHANNELS {
        return 0;
    }
    let len = s.length as usize;
    let mut changed = 0;
    for r in 0..len / 2 {
        let (a, b) = (s.cells[r][ch], s.cells[len - 1 - r][ch]);
        if a != b {
            changed += 2;
        }
        s.cells[r][ch] = b;
        s.cells[len - 1 - r][ch] = a;
    }
    changed
}

/// Euclidean rhythm: `hits` evenly spread over `steps`, repeated across the pattern, replacing the channel.
pub fn euclid(s: &mut Song, ch: usize, hits: usize, steps: usize, key: u8, vel: u8) -> usize {
    if ch >= CHANNELS || steps == 0 {
        return 0;
    }
    let hits = hits.min(steps);
    let mut n = 0;
    for r in 0..s.length as usize {
        let i = r % steps;
        let on = hits > 0 && (i * hits) % steps < hits;
        s.cells[r][ch] = if on {
            n += 1;
            Cell { note: key.clamp(1, 127), vel: vel.clamp(1, 15) }
        } else {
            Cell::EMPTY
        };
    }
    n
}

/// Shift every note of a channel by `semis` (`ch` = None: every channel).
pub fn transpose(s: &mut Song, ch: Option<usize>, semis: i32) -> usize {
    let mut n = 0;
    for r in 0..s.length as usize {
        for c in 0..CHANNELS {
            if ch.map_or(false, |x| x != c) {
                continue;
            }
            let cell = &mut s.cells[r][c];
            if is_note(*cell) {
                cell.note = (cell.note as i32 + semis).clamp(1, 127) as u8;
                n += 1;
            }
        }
    }
    n
}

/// Shift every pitched channel (drums stay put) by `semis`.
pub fn transpose_melodic(s: &mut Song, semis: i32) -> usize {
    let mut n = 0;
    for c in 0..CHANNELS {
        if !matches!(s.chans[c].inst, 5 | 6 | 7 | 12 | 13) {
            n += transpose(s, Some(c), semis);
        }
    }
    n
}

/// Copy the first half of the pattern over the second (a quick way to build a repeating phrase).
pub fn repeat_half(s: &mut Song, ch: usize) -> usize {
    if ch >= CHANNELS {
        return 0;
    }
    let half = s.length as usize / 2;
    let mut n = 0;
    for r in 0..half {
        s.cells[half + r][ch] = s.cells[r][ch];
        if is_note(s.cells[r][ch]) {
            n += 1;
        }
    }
    n
}

pub const SCALE_MAJOR: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];
pub const SCALE_MINOR: [u8; 7] = [0, 2, 3, 5, 7, 8, 10];

/// Pull notes onto the nearest tone of a scale (`root` = pitch class 0..=11).
pub fn snap_to_scale(s: &mut Song, ch: usize, root: u8, minor: bool) -> usize {
    if ch >= CHANNELS {
        return 0;
    }
    let sc = if minor { SCALE_MINOR } else { SCALE_MAJOR };
    let mut n = 0;
    for r in 0..s.length as usize {
        let c = &mut s.cells[r][ch];
        if !is_note(*c) {
            continue;
        }
        let pc = (c.note as i32 - root as i32).rem_euclid(12);
        let mut best = (99, 0i32);
        for t in sc {
            for w in [-12i32, 0, 12] {
                let d = (t as i32 + w) - pc;
                if d.abs() < best.0 {
                    best = (d.abs(), d);
                }
            }
        }
        if best.1 != 0 {
            c.note = (c.note as i32 + best.1).clamp(1, 127) as u8;
            n += 1;
        }
    }
    n
}

/// Guess the key of a song from its pitch classes (drums ignored): (root 0..=11, minor).
pub fn detect_key(s: &Song) -> (u8, bool) {
    let mut h = [0u32; 12];
    for r in 0..s.length as usize {
        for c in 0..CHANNELS {
            let cell = s.cells[r][c];
            if is_note(cell) && !matches!(s.chans[c].inst, 5 | 6 | 7 | 12 | 13) {
                h[(cell.note % 12) as usize] += 1;
            }
        }
    }
    let mut best = (0u32, 0u8, false);
    for root in 0..12u8 {
        for minor in [false, true] {
            let sc = if minor { SCALE_MINOR } else { SCALE_MAJOR };
            let mut score = 0;
            for t in sc {
                score += h[((root + t) % 12) as usize];
            }
            // the tonic and its fifth weigh more: that separates a major key from its relative minor
            score += h[root as usize] / 2 + h[((root + 7) % 12) as usize] / 4;
            if score > best.0 {
                best = (score, root, minor);
            }
        }
    }
    (best.1, best.2)
}
