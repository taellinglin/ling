//! Conforming a song to a steady shape, "page by page, book by book".
//!
//! * A **backing** part (chords, pad, bass, arpeggio) is made of one repeating *page* of 1, 2, 4 or 8 bars: the page that best
//!   represents the part is found, and the part is rebuilt as that page over and over. Every 4th time a small variation turns the
//!   page (a thinned last bar, a lifted last note) and every 8th time a slightly bigger one does, so it stays alive but predictable.
//! * A prominent **riff** (a loud single-note line) is given attention twice in the song, each statement at most 32 bars: it is
//!   trimmed or extended by repeating its own pattern to a whole number of 8-bar phrases, and when it only appears once in a long
//!   song its statement is repeated later, in the next high-energy section.
//! Pure functions over notes in beats; `api/fix_api.rs` applies them to clips.
use crate::theory::N;

/// the notes of one bar as a set of (pitch, start inside the bar in sixteenths)
fn bar_cells(notes: &[N], bar: usize) -> Vec<(u8, i32)> {
    let (lo, hi) = (bar as f64 * 4.0, (bar + 1) as f64 * 4.0);
    let mut v: Vec<(u8, i32)> = notes.iter().filter(|n| n.start >= lo - 1e-6 && n.start < hi - 1e-6).map(|n| (n.key, ((n.start - lo) * 4.0).round() as i32)).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// 0..1: how much two bars share (same pitch at the same place), an empty bar only matching an empty bar
fn bar_sim(a: &[(u8, i32)], b: &[(u8, i32)]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.iter().filter(|x| b.contains(x)).count();
    inter as f64 / (a.len() + b.len() - inter) as f64
}

/// how alike two windows of `p` bars are
fn window_sim(cells: &[Vec<(u8, i32)>], a: usize, b: usize, p: usize) -> f64 {
    (0..p).map(|i| bar_sim(&cells[a + i], &cells[b + i])).sum::<f64>() / p as f64
}

#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub bars: usize,
    /// the bar the representative page starts at
    pub start_bar: usize,
    /// 0..1, how alike the pages of the part already are
    pub score: f64,
}

/// the page length (1, 2, 4 or 8 bars) and the page that best stands for the part; `None` when there is too little to tell
pub fn best_page(notes: &[N], total_bars: usize) -> Option<Page> {
    if total_bars < 2 || notes.len() < 4 {
        return None;
    }
    let cells: Vec<Vec<(u8, i32)>> = (0..total_bars).map(|b| bar_cells(notes, b)).collect();
    let mut best: Option<Page> = None;
    for p in [1usize, 2, 4, 8] {
        let windows = total_bars / p;
        if windows < 2 {
            continue;
        }
        // the medoid: the window most like all the others
        let mut top = (0usize, -1.0f64);
        for w in 0..windows {
            let s = (0..windows).filter(|o| *o != w).map(|o| window_sim(&cells, w * p, o * p, p)).sum::<f64>() / (windows - 1) as f64;
            if s > top.1 {
                top = (w, s);
            }
        }
        // a longer page has to be clearly better to win over a shorter one
        let cand = Page { bars: p, start_bar: top.0 * p, score: top.1 };
        let take = match &best {
            None => true,
            Some(b) => cand.score > b.score + 0.08 && cand.bars > b.bars || cand.score > b.score + 0.2,
        };
        if take {
            best = Some(cand);
        }
    }
    best
}

/// how strong a variation to give the n-th repeat (0 none, 1 small every 4th, 2 bigger every 8th)
pub fn variation_level(cycle: usize) -> u8 {
    if cycle > 0 && (cycle + 1) % 8 == 0 {
        2
    } else if cycle > 0 && (cycle + 1) % 4 == 0 {
        1
    } else {
        0
    }
}

/// Rebuild `len_beats` of music from one page: `page` bars starting at `start_bar` of `notes`, repeated, with a turn at every 4th and 8th repeat.
pub fn loopify(notes: &[N], len_beats: f64, page: &Page, seed: u32) -> Vec<N> {
    let (p0, p1) = (page.start_bar as f64 * 4.0, (page.start_bar + page.bars) as f64 * 4.0);
    let base: Vec<N> = notes.iter().filter(|n| n.start >= p0 - 1e-6 && n.start < p1 - 1e-6).map(|n| N { start: n.start - p0, ..*n }).collect();
    let plen = page.bars as f64 * 4.0;
    let cycles = (len_beats / plen).ceil() as usize;
    let mut out = vec![];
    for c in 0..cycles {
        let level = variation_level(c);
        let off = c as f64 * plen;
        for (i, n) in base.iter().enumerate() {
            let mut m = *n;
            m.start += off;
            if m.start >= len_beats - 1e-6 {
                continue;
            }
            let in_last_bar = n.start >= plen - 4.0;
            let weak = (n.start % 1.0).abs() > 0.01;
            // a deterministic little hash decides which notes the variation touches
            let h = (i as u32).wrapping_mul(2654435761).wrapping_add(seed).wrapping_add(c as u32 * 40503);
            if level >= 1 && in_last_bar && weak && h % 3 == 0 {
                continue; // thin the last bar
            }
            if level >= 1 && in_last_bar && i + 1 == base.len() && m.key <= 115 {
                m.key += 12; // lift the last note of the page
            }
            if level >= 2 && n.start < 0.01 {
                m.vel = (m.vel + 0.1).min(1.0);
            }
            if level == 0 && c % 2 == 1 {
                m.vel = (m.vel * (0.94 + (h % 5) as f32 * 0.03)).clamp(0.05, 1.0);
            }
            m.len = m.len.min(len_beats - m.start).max(0.03);
            out.push(m);
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub start: f64,
    pub end: f64,
    /// loudness times length: how much attention it asks for
    pub energy: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Statement {
    /// the existing span this statement rebuilds, or None for a new copy
    pub from: Option<usize>,
    /// the span a new copy is made from
    pub copy_of: Option<usize>,
    pub start: f64,
    pub bars: usize,
}

/// A riff gets attention twice, each statement a whole number of 8-bar phrases and at most 32 bars.
/// `highs` are the high-energy sections (start, end) of the song, where a second statement would go.
pub fn plan_statements(spans: &[Span], song_beats: f64, highs: &[(f64, f64)]) -> Vec<Statement> {
    if spans.is_empty() {
        return vec![];
    }
    let mut order: Vec<usize> = (0..spans.len()).collect();
    order.sort_by(|a, b| spans[*b].energy.partial_cmp(&spans[*a].energy).unwrap());
    order.truncate(2);
    order.sort_by(|a, b| spans[*a].start.partial_cmp(&spans[*b].start).unwrap());
    let mut out: Vec<Statement> = order
        .iter()
        .map(|i| {
            let s = &spans[*i];
            let bars = ((s.end - s.start) / 4.0).round().max(1.0) as usize;
            let target = if bars <= 4 { bars } else { (((bars + 7) / 8) * 8).min(32) };
            Statement { from: Some(*i), copy_of: None, start: s.start, bars: target }
        })
        .collect();
    let song_bars = (song_beats / 4.0).ceil() as usize;
    if out.len() == 1 && song_bars >= 64 && out[0].bars >= 4 {
        let first = out[0].clone();
        let first_end = first.start + first.bars as f64 * 4.0;
        let len = first.bars as f64 * 4.0;
        let fits = |s: f64| s >= first_end + 16.0 && s + len <= song_beats + 8.0;
        let at = highs.iter().map(|h| (h.0 / 4.0).ceil() * 4.0).find(|s| fits(*s)).or_else(|| {
            let s = ((song_beats * 0.62) / 32.0).round() * 32.0;
            if fits(s) {
                Some(s)
            } else {
                None
            }
        });
        if let Some(s) = at {
            out.push(Statement { from: None, copy_of: first.from, start: s, bars: first.bars });
        }
    }
    out
}

/// One piece of a long clip: `repeats` copies of a page of `page_bars` bars starting at `start_bar`.
#[derive(Clone, Debug, PartialEq)]
pub struct SliceSeg {
    pub start_bar: usize,
    pub page_bars: usize,
    pub repeats: usize,
    /// segments with the same group hold the same pattern (a letter in the name)
    pub group: usize,
}

fn window_empty(cells: &[Vec<(u8, i32)>], b: usize, p: usize) -> bool {
    (b..b + p).all(|i| cells[i].is_empty())
}

/// How alike two windows of `p` bars are, empty bars only matching empty bars.
fn same_window(cells: &[Vec<(u8, i32)>], a: usize, b: usize, p: usize) -> f64 {
    window_sim(cells, a, b, p)
}

/// Cut a long clip into patterns. Silence is dropped; a run of identical pages (1, 2, 4 or 8 bars) becomes one segment that repeats;
/// material that does not repeat becomes phrases of 4 bars. Segments with the same content share a group, so identical patterns that
/// come back later are recognised as the same one.
pub fn slice_plan(notes: &[N], total_bars: usize) -> Vec<SliceSeg> {
    const SAME: f64 = 0.97;
    if total_bars == 0 || notes.is_empty() {
        return vec![];
    }
    let cells: Vec<Vec<(u8, i32)>> = (0..total_bars).map(|b| bar_cells(notes, b)).collect();
    let mut segs: Vec<SliceSeg> = vec![];
    let mut b = 0usize;
    while b < total_bars {
        if cells[b].is_empty() {
            b += 1;
            continue;
        }
        // the page length whose run of repeats covers the most bars (the smallest page wins a tie: the shortest pattern)
        let mut best: Option<(usize, usize)> = None; // (page, repeats)
        for p in [1usize, 2, 4, 8] {
            if b + p > total_bars || window_empty(&cells, b, p) {
                continue;
            }
            let mut r = 1;
            while b + (r + 1) * p <= total_bars && same_window(&cells, b, b + r * p, p) >= SAME {
                r += 1;
            }
            if r >= 2 && best.map_or(true, |(bp, br)| r * p > br * bp) {
                best = Some((p, r));
            }
        }
        match best {
            Some((p, r)) => {
                segs.push(SliceSeg { start_bar: b, page_bars: p, repeats: r, group: 0 });
                b += p * r;
            }
            None => {
                // a phrase of up to 4 bars that ends where silence of two bars or more begins
                let mut e = (b + 4).min(total_bars);
                for k in b + 1..e {
                    if cells[k].is_empty() && k + 1 < total_bars && cells[k + 1].is_empty() {
                        e = k;
                        break;
                    }
                }
                while e > b + 1 && cells[e - 1].is_empty() {
                    e -= 1;
                }
                segs.push(SliceSeg { start_bar: b, page_bars: e - b, repeats: 1, group: 0 });
                b = e;
            }
        }
    }
    // the same page content gets the same group
    let mut reps: Vec<(usize, usize, usize)> = vec![]; // (start bar, page bars, group)
    let mut next = 0;
    for sg in segs.iter_mut() {
        let found = reps.iter().find(|(rb, rp, _)| *rp == sg.page_bars && same_window(&cells, *rb, sg.start_bar, sg.page_bars) >= SAME).map(|r| r.2);
        sg.group = match found {
            Some(g) => g,
            None => {
                reps.push((sg.start_bar, sg.page_bars, next));
                next += 1;
                next - 1
            }
        };
    }
    segs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(start: f64, key: u8) -> N {
        N { start, len: 0.4, key, vel: 0.8 }
    }

    /// a 2-bar figure repeated for `bars` bars, with a stray note now and then
    fn figure(bars: usize) -> Vec<N> {
        let mut v = vec![];
        for b in (0..bars).step_by(2) {
            let o = b as f64 * 4.0;
            for (t, k) in [(0.0, 60u8), (1.0, 64), (2.0, 67), (3.0, 64), (4.0, 62), (5.0, 65), (6.0, 69), (7.0, 65)] {
                v.push(n(o + t, k));
            }
        }
        v
    }

    #[test]
    fn the_repeating_page_is_found() {
        let mut notes = figure(16);
        notes.push(n(21.5, 71)); // a stray note in bar 5
        let p = best_page(&notes, 16).expect("a page");
        assert!(p.bars == 2 || p.bars == 4, "{p:?}");
        assert!(p.score > 0.8, "{p:?}");
        // pure noise has no good page
        let noise: Vec<N> = (0..64).map(|i| n(i as f64 * 0.5 + (i % 3) as f64 * 0.25, 40 + ((i * 37) % 40) as u8)).collect();
        let q = best_page(&noise, 8).map(|p| p.score).unwrap_or(0.0);
        assert!(q < 0.5, "{q}");
        assert!(best_page(&[n(0.0, 60)], 8).is_none());
    }

    #[test]
    fn a_page_is_rebuilt_across_the_length_with_turns_every_fourth_and_eighth_time() {
        let notes = figure(8);
        let page = best_page(&notes, 8).unwrap();
        let out = loopify(&notes, 64.0, &page, 7);
        assert!(out.iter().all(|x| x.start < 64.0 && x.start + x.len <= 64.0 + 1e-9));
        // the same figure keeps coming back: bar 0 and bar 8 have the same cells
        let cells = |b: usize| bar_cells(&out, b);
        assert_eq!(cells(0), cells(8));
        assert_eq!(cells(0), cells(12));
        // a turn: the fourth repeat of a 2-bar page (bars 6-7) is not identical to the first
        assert_ne!(cells(7), cells(1), "the last bar of the 4th page turns");
        assert_eq!(variation_level(3), 1);
        assert_eq!(variation_level(7), 2);
        assert_eq!(variation_level(1), 0);
        // everything still in the original pitches or an octave above
        assert!(out.iter().all(|x| [60, 62, 64, 65, 67, 69, 72, 74, 76, 77, 79, 81].contains(&x.key)), "{:?}", out.iter().map(|x| x.key).collect::<Vec<_>>());
    }

    #[test]
    fn a_riff_gets_two_statements_of_at_most_32_bars() {
        // three spans: the two loudest are kept
        let spans = vec![Span { start: 16.0, end: 80.0, energy: 90.0 }, Span { start: 200.0, end: 232.0, energy: 20.0 }, Span { start: 320.0, end: 400.0, energy: 70.0 }];
        let st = plan_statements(&spans, 480.0, &[]);
        assert_eq!(st.len(), 2);
        assert!(st.iter().all(|s| s.bars <= 32 && (s.bars % 8 == 0 || s.bars <= 4)), "{st:?}");
        assert_eq!(st[0].from, Some(0));
        assert_eq!(st[1].from, Some(2));
        // a single short statement in a long song is repeated in the next high section
        let one = vec![Span { start: 64.0, end: 128.0, energy: 50.0 }];
        let st = plan_statements(&one, 640.0, &[(0.0, 64.0), (256.0, 384.0)]);
        assert_eq!(st.len(), 2, "{st:?}");
        assert_eq!(st[1].copy_of, Some(0));
        assert_eq!(st[1].start, 256.0);
        assert_eq!(st[0].bars, 16, "64 beats = 16 bars stays 16");
        // a short song is left with one
        assert_eq!(plan_statements(&one, 200.0, &[]).len(), 1);
    }

    #[test]
    fn a_solid_clip_is_cut_into_patterns_with_repeats_folded_and_silence_dropped() {
        // 8 bars of a 2-bar figure, 4 bars of silence, a 4-bar phrase that does not repeat, then the figure again for 4 bars
        let mut notes = figure(8);
        for (i, k) in [60u8, 62, 64, 65, 67, 69, 71, 72, 74, 76, 77, 79, 81, 83, 84, 86].iter().enumerate() {
            notes.push(n(48.0 + i as f64, *k));
        }
        for x in figure(4) {
            notes.push(N { start: x.start + 64.0, ..x });
        }
        let segs = slice_plan(&notes, 20);
        assert_eq!(segs.len(), 3, "{segs:?}");
        assert_eq!((segs[0].start_bar, segs[0].page_bars, segs[0].repeats), (0, 2, 4));
        assert_eq!((segs[1].start_bar, segs[1].repeats), (12, 1));
        assert_eq!((segs[2].start_bar, segs[2].page_bars, segs[2].repeats), (16, 2, 2));
        // the figure coming back later is the same pattern as the first
        assert_eq!(segs[0].group, segs[2].group);
        assert_ne!(segs[0].group, segs[1].group);
        // nothing is lost: bars 8..12 (silence) are the only bars not covered
        let covered: usize = segs.iter().map(|s| s.page_bars * s.repeats).sum();
        assert_eq!(covered, 16);
    }

    #[test]
    fn a_pattern_with_one_changed_bar_is_not_folded_over_the_change() {
        let mut notes = figure(16);
        notes.retain(|x| !(x.start >= 24.0 && x.start < 28.0)); // bar 7 emptied
        notes.push(n(24.0, 70));
        let segs = slice_plan(&notes, 16);
        // 6 bars of the figure fold up to bar 6, the changed bar starts a new segment
        assert_eq!((segs[0].start_bar, segs[0].page_bars * segs[0].repeats), (0, 6));
        assert!(segs.len() >= 2 && segs[1].start_bar == 6, "{segs:?}");
    }
}
