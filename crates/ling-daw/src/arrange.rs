//! The arrangement as a whole: snapping clip onsets and durations to the song's grid, trimming overlaps and closing gaps, and
//! suggesting section tags (intro, verse, build, chorus / drop, bridge / breakdown, outro, fill) from how busy each bar is.
//! Pure functions over plain numbers; `api/fix_api.rs` gathers the data from the project and applies the plans.

pub const A_ONSET: u32 = 1;
pub const A_LEN: u32 = 2;
pub const A_OVERLAP: u32 = 4;
pub const A_GAPS: u32 = 8;

#[derive(Clone, Debug)]
pub struct ClipInfo {
    pub track: usize,
    pub clip: usize,
    pub start: f64,
    pub len: f64,
    /// where the last note ends, relative to the clip start (0 for audio / empty clips)
    pub content_end: f64,
    /// a clip whose content loops keeps its length
    pub looped: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClipFix {
    pub track: usize,
    pub clip: usize,
    pub new_start: f64,
    pub new_len: f64,
    /// which kinds of change this is (A_ONSET | A_LEN | ...)
    pub what: u32,
}

/// the coarsest grid (4, 2, 1, 0.5 beats) that most of `vals` sit on, within a tolerance
fn grid_of(vals: &[f64], options: &[f64]) -> f64 {
    if vals.is_empty() {
        return 1.0;
    }
    for g in options {
        let on = vals.iter().filter(|v| ((**v / g).round() * g - **v).abs() <= g * 0.12 + 0.01).count();
        if on as f64 >= vals.len() as f64 * 0.7 {
            return *g;
        }
    }
    *options.last().unwrap()
}

pub fn plan_clips(clips: &[ClipInfo], mask: u32) -> Vec<ClipFix> {
    let g_start = grid_of(&clips.iter().map(|c| c.start).collect::<Vec<_>>(), &[4.0, 2.0, 1.0, 0.5]);
    let g_len = grid_of(&clips.iter().map(|c| c.len).collect::<Vec<_>>(), &[4.0, 2.0, 1.0]);
    let mut out: Vec<ClipFix> = clips.iter().map(|c| ClipFix { track: c.track, clip: c.clip, new_start: c.start, new_len: c.len, what: 0 }).collect();
    for (i, c) in clips.iter().enumerate() {
        let f = &mut out[i];
        if mask & A_ONSET != 0 {
            let near = ((c.start / g_start).round() * g_start).max(0.0);
            if (near - c.start).abs() > 1e-4 && (near - c.start).abs() <= g_start * 0.35 {
                f.new_start = near;
                f.what |= A_ONSET;
            }
        }
        if mask & A_LEN != 0 && !c.looped {
            let mut near = ((c.len / g_len).round() * g_len).max(g_len);
            // never cut the notes off
            if c.content_end > near + 1e-6 {
                near = (c.content_end / g_len).ceil() * g_len;
            }
            if (near - c.len).abs() > 1e-4 && (near - c.len).abs() <= g_len * 0.35 + 1e-6 {
                f.new_len = near;
                f.what |= A_LEN;
            }
        }
    }
    // overlaps and gaps between neighbours on one track (using the snapped positions)
    let mut tracks: Vec<usize> = clips.iter().map(|c| c.track).collect();
    tracks.sort_unstable();
    tracks.dedup();
    for t in tracks {
        let mut idx: Vec<usize> = (0..out.len()).filter(|i| out[*i].track == t).collect();
        idx.sort_by(|a, b| out[*a].new_start.partial_cmp(&out[*b].new_start).unwrap());
        for w in idx.windows(2) {
            let (a, b) = (w[0], w[1]);
            let end = out[a].new_start + out[a].new_len;
            let next = out[b].new_start;
            let overlap = end - next;
            if mask & A_OVERLAP != 0 && overlap > 1e-4 && overlap <= 1.0 && out[a].new_len - overlap >= 0.25 && !clips.iter().any(|c| c.track == t && c.clip == out[a].clip && c.looped) {
                out[a].new_len -= overlap;
                out[a].what |= A_OVERLAP;
            } else if mask & A_GAPS != 0 && overlap < -1e-4 && overlap >= -0.5 && !clips.iter().any(|c| c.track == t && c.clip == out[a].clip && c.looped) {
                out[a].new_len = next - out[a].new_start;
                out[a].what |= A_GAPS;
            }
        }
    }
    out.into_iter().filter(|f| f.what != 0).collect()
}

// ── sections ────────────────────────────────────────────────────────────────
#[derive(Clone, Debug, Default)]
pub struct BarFeat {
    /// melodic tracks with a note in the bar
    pub tracks: usize,
    /// notes of all melodic tracks starting in the bar
    pub notes: usize,
    pub drum_notes: usize,
    pub kicks: usize,
    pub crash: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub start_bar: usize,
    pub end_bar: usize,
    pub tag: String,
    pub energy: f64,
}

fn median(v: &mut Vec<usize>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_unstable();
    v[v.len() / 2] as f64
}

/// (sections, fills): fills are single bars, as (bar, bar + 1)
pub fn detect_sections(bars: &[BarFeat]) -> (Vec<Section>, Vec<(usize, usize)>) {
    let n = bars.len();
    if n == 0 {
        return (vec![], vec![]);
    }
    let max_tracks = bars.iter().map(|b| b.tracks).max().unwrap_or(1).max(1) as f64;
    let max_notes = bars.iter().map(|b| b.notes).max().unwrap_or(1).max(1) as f64;
    let max_drums = bars.iter().map(|b| b.drum_notes).max().unwrap_or(1).max(1) as f64;
    let raw: Vec<f64> = bars
        .iter()
        .map(|b| 0.35 * b.tracks as f64 / max_tracks + 0.25 * b.notes as f64 / max_notes + 0.25 * b.drum_notes as f64 / max_drums + 0.15 * (b.kicks > 0) as i32 as f64)
        .collect();
    // light smoothing so a single fill bar does not split a section
    let en: Vec<f64> = (0..n)
        .map(|i| {
            let lo = i.saturating_sub(1);
            let hi = (i + 1).min(n - 1);
            (lo..=hi).map(|k| raw[k]).sum::<f64>() / (hi - lo + 1) as f64
        })
        .collect();
    // boundaries: a clear change of energy, of the number of players, or the drums starting / stopping, at an even bar
    let mut cuts = vec![0usize];
    for i in 1..n {
        let jump = (en[i] - en[i - 1]).abs() > 0.14 || (bars[i].tracks as i64 - bars[i - 1].tracks as i64).abs() >= 2 || ((bars[i].drum_notes == 0) != (bars[i - 1].drum_notes == 0));
        let phrase = i % 8 == 0 && i - cuts.last().unwrap() >= 8;
        if (jump || phrase) && i % 2 == 0 && i - cuts.last().unwrap() >= 4 {
            cuts.push(i);
        } else if jump && i - cuts.last().unwrap() >= 4 && (i % 2 == 1) {
            // move an odd-bar change to the next even bar
            if i + 1 < n && i + 1 - cuts.last().unwrap() >= 4 && !cuts.contains(&(i + 1)) {
                cuts.push(i + 1);
            }
        }
    }
    cuts.dedup();
    let mut secs: Vec<Section> = vec![];
    for (k, s) in cuts.iter().enumerate() {
        let e = cuts.get(k + 1).copied().unwrap_or(n);
        let mean = (*s..e).map(|i| raw[i]).sum::<f64>() / (e - s) as f64;
        secs.push(Section { start_bar: *s, end_bar: e, tag: String::new(), energy: mean });
    }
    // merge a too-short last section into the one before it
    if secs.len() > 1 && secs.last().map_or(false, |s| s.end_bar - s.start_bar < 4) {
        let last = secs.pop().unwrap();
        secs.last_mut().unwrap().end_bar = last.end_bar;
    }
    let maxe = secs.iter().map(|s| s.energy).fold(0.0, f64::max).max(1e-6);
    // is it four-on-the-floor dance music?
    let drum_bars: Vec<&BarFeat> = bars.iter().filter(|b| b.drum_notes > 0).collect();
    let floor = !drum_bars.is_empty() && drum_bars.iter().filter(|b| b.kicks >= 4).count() as f64 >= drum_bars.len() as f64 * 0.6;
    let ns = secs.len();
    let high = |s: &Section| s.energy >= maxe * 0.82;
    let mut seen_high = false;
    let mut verse_n = 0;
    for i in 0..ns {
        let (prev_e, next_e) = (if i > 0 { secs[i - 1].energy } else { 0.0 }, if i + 1 < ns { secs[i + 1].energy } else { 0.0 });
        let s = secs[i].clone();
        let rising = i + 1 < ns && s.end_bar - s.start_bar >= 2 && en[s.end_bar - 1] - en[s.start_bar] > 0.2 && next_e >= maxe * 0.78;
        let drums_here = (s.start_bar..s.end_bar).filter(|b| bars[*b].drum_notes > 0).count() as f64 / (s.end_bar - s.start_bar) as f64;
        let tag = if ns == 1 {
            "main"
        } else if i == 0 && (s.energy < maxe * 0.6 || drums_here < 0.5) && !high(&s) {
            "intro"
        } else if i == ns - 1 && s.energy < maxe * 0.6 {
            "outro"
        } else if rising {
            "build"
        } else if high(&s) {
            seen_high = true;
            if floor { "drop" } else { "chorus" }
        } else if seen_high && s.energy < maxe * 0.55 && next_e >= maxe * 0.7 {
            if drums_here < 0.4 { "breakdown" } else { "bridge" }
        } else if s.energy < maxe * 0.45 && i > 0 && i + 1 < ns && prev_e >= maxe * 0.7 {
            "breakdown"
        } else {
            verse_n += 1;
            if i == ns - 1 { "outro" } else { "verse" }
        };
        secs[i].tag = tag.to_string();
    }
    let _ = verse_n;
    // fills: a bar at the end of a phrase where the drums get much busier, usually into a crash
    let mut fills = vec![];
    for (si, s) in secs.iter().enumerate() {
        let mut d: Vec<usize> = (s.start_bar..s.end_bar).map(|b| bars[b].drum_notes).collect();
        let med = median(&mut d).max(1.0);
        for b in s.start_bar..s.end_bar {
            let end_of_phrase = (b + 1) % 4 == 0 || (b + 1) % 8 == 0 || b + 1 == s.end_bar;
            let busier = bars[b].drum_notes as f64 >= med * 1.6 && bars[b].drum_notes >= 6;
            let crash_next = b + 1 < n && bars[b + 1].crash;
            if end_of_phrase && b > s.start_bar && (busier || (crash_next && bars[b].drum_notes as f64 >= med * 1.2)) && !(si == 0 && b < 2) {
                fills.push((b, b + 1));
            }
        }
    }
    (secs, fills)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ci(t: usize, c: usize, start: f64, len: f64, ce: f64) -> ClipInfo {
        ClipInfo { track: t, clip: c, start, len, content_end: ce, looped: false }
    }

    #[test]
    fn onsets_durations_overlaps_and_gaps_are_lined_up() {
        let clips = vec![ci(0, 0, 0.0, 16.0, 15.0), ci(0, 1, 16.1, 15.8, 14.0), ci(0, 2, 32.0, 16.0, 16.0), ci(1, 0, 0.0, 8.0, 7.0), ci(1, 1, 8.3, 8.0, 8.0), ci(1, 2, 16.0, 7.8, 7.0)];
        let fixes = plan_clips(&clips, A_ONSET | A_LEN | A_OVERLAP | A_GAPS);
        let get = |t, c| fixes.iter().find(|f| f.track == t && f.clip == c);
        let f = get(0, 1).expect("the off-grid clip is fixed");
        assert_eq!(f.new_start, 16.0);
        assert_eq!(f.new_len, 16.0);
        assert!(get(0, 0).is_none() && get(0, 2).is_none(), "clips already on the grid are left alone");
        assert_eq!(get(1, 1).unwrap().new_start, 8.0);
        assert!(get(1, 2).unwrap().new_len >= 8.0 - 1e-9);
        // notes are never cut: content past the rounded length keeps the clip long enough
        let long = plan_clips(&[ci(0, 0, 0.0, 15.9, 16.0), ci(0, 1, 16.0, 16.0, 16.0), ci(0, 2, 32.0, 16.0, 16.0)], A_LEN);
        assert!(long.iter().all(|f| f.new_len >= 16.0), "{long:?}");
        // a clip that loops keeps its length
        let mut lp = ci(0, 0, 0.0, 15.7, 4.0);
        lp.looped = true;
        assert!(plan_clips(&[lp, ci(0, 1, 16.0, 16.0, 16.0), ci(0, 2, 32.0, 16.0, 16.0)], A_LEN).is_empty());
    }

    fn bar(tracks: usize, notes: usize, drums: usize, kicks: usize) -> BarFeat {
        BarFeat { tracks, notes, drum_notes: drums, kicks, crash: false }
    }

    #[test]
    fn a_dance_track_gets_intro_build_drop_breakdown_and_outro() {
        let mut bars = vec![];
        for _ in 0..8 {
            bars.push(bar(1, 8, 0, 0)); // intro: one pad, no drums
        }
        for i in 0..8 {
            bars.push(bar(2 + i / 4, 20 + i * 4, 16 + i * 6, 4)); // build: layers and drums arrive
        }
        for _ in 0..16 {
            bars.push(bar(5, 60, 40, 4)); // drop
        }
        for _ in 0..8 {
            bars.push(bar(2, 16, 0, 0)); // breakdown
        }
        for _ in 0..16 {
            bars.push(bar(5, 60, 40, 4)); // drop again
        }
        for _ in 0..8 {
            bars.push(bar(1, 8, 4, 1)); // outro
        }
        let (secs, _) = detect_sections(&bars);
        let tags: Vec<&str> = secs.iter().map(|s| s.tag.as_str()).collect();
        assert_eq!(tags.first(), Some(&"intro"), "{tags:?}");
        assert_eq!(tags.last(), Some(&"outro"), "{tags:?}");
        assert!(tags.contains(&"drop") && tags.contains(&"build") && tags.contains(&"breakdown"), "{tags:?}");
        assert!(secs.windows(2).all(|w| w[0].end_bar == w[1].start_bar), "sections tile the song");
        assert_eq!(secs.last().unwrap().end_bar, bars.len());
    }

    #[test]
    fn a_fill_before_a_crash_is_found() {
        let mut bars: Vec<BarFeat> = (0..16).map(|_| bar(3, 30, 16, 4)).collect();
        bars[7].drum_notes = 40;
        bars[8].crash = true;
        let (_, fills) = detect_sections(&bars);
        assert!(fills.contains(&(7, 8)), "{fills:?}");
    }
}
