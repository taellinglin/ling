//! Arrangement edits for the project agent (`daw_arrx_*`): repeat / remove / swap / move / resize a section (every track's clips,
//! the automation lanes, the tempo map and the section tags move together), insert a build or a breakdown, change the tempo of a
//! section, half-time / double-time drums, layered harmonies (thirds, sixths, fifths, octave, a choir pad), synth parameters and
//! automation by words, and taking a part out of (or soloing it in) one section.
//!
//! Positions are beats. Sections are the ruler tags (project meta "sections": `start:end:tag;...`). Every parameter write passes the
//! pitch guard (`redesign::param_allowed`): vibrato, sweep, glide, detune, bend ... only move when the request asks for pitch.
use super::{Daw, Val, A};
use crate::articulate::{role_of, Role};
use crate::model::*;
use crate::redesign;

const EPS: f64 = 1e-6;

// ── section tags ──────────────────────────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct Tag {
    pub s: f64,
    pub e: f64,
    pub name: String,
}

fn read_tags(p: &Project) -> Vec<Tag> {
    let text = p.meta.iter().find(|(k, _)| k == "sections").map(|(_, v)| v.clone()).unwrap_or_default();
    let mut v = vec![];
    for part in text.split(';') {
        let f: Vec<&str> = part.split(':').collect();
        if f.len() == 3 {
            if let (Ok(s), Ok(e)) = (f[0].trim().parse::<f64>(), f[1].trim().parse::<f64>()) {
                v.push(Tag { s, e, name: f[2].trim().to_string() });
            }
        }
    }
    v
}

fn write_tags(p: &mut Project, mut tags: Vec<Tag>) {
    tags.retain(|t| t.e - t.s > EPS && t.s.is_finite() && t.e.is_finite());
    for t in tags.iter_mut() {
        t.s = (t.s * 1e5).round() / 1e5;
        t.e = (t.e * 1e5).round() / 1e5;
    }
    tags.sort_by(|a, b| a.s.total_cmp(&b.s));
    let text = tags.iter().map(|t| format!("{}:{}:{}", t.s, t.e, t.name)).collect::<Vec<_>>().join(";");
    match p.meta.iter_mut().find(|(k, _)| k == "sections") {
        Some(slot) => slot.1 = text,
        None => p.meta.push(("sections".into(), text)),
    }
}

fn song_end(p: &Project) -> f64 {
    p.end_beat()
}

// ── words -> a section ────────────────────────────────────────────────────────────────────────────────────────────────

fn words_of(req: &str) -> Vec<String> {
    req.to_lowercase().replace([',', '.', '!', '?', ';', ':', '(', ')', '-'], " ").split_whitespace().map(|w| w.to_string()).collect()
}

/// the section word a request names last, as the tag family it means
pub fn tag_family(w: &str) -> Option<&'static str> {
    Some(match w {
        "verse" | "verses" => "verse",
        "chorus" | "choruses" | "hook" | "refrain" => "chorus",
        "drop" | "drops" => "drop",
        "bridge" | "middle8" => "bridge",
        "intro" | "introduction" => "intro",
        "outro" | "ending" => "outro",
        "breakdown" | "break" => "breakdown",
        "buildup" | "build-up" => "build",
        "solo" => "solo",
        _ => return None,
    })
}

fn family_matches(fam: &str, tag: &str) -> bool {
    let t = tag.to_lowercase();
    match fam {
        "chorus" => t.contains("chorus") || t.contains("hook") || t.contains("refrain"),
        "breakdown" => t.contains("breakdown") || t == "break",
        "build" => t.contains("build") || t.contains("riser"),
        "outro" => t.contains("outro") || t.contains("ending"),
        "bridge" => t.contains("bridge") || t.contains("middle"),
        f => t.contains(f),
    }
}

fn ordinal(w: &str) -> Option<i32> {
    Some(match w {
        "first" | "1st" | "1" | "one" => 1,
        "second" | "2nd" | "2" | "two" => 2,
        "third" | "3rd" | "3" | "three" => 3,
        "fourth" | "4th" | "4" | "four" => 4,
        "fifth" | "5th" | "5" => 5,
        "last" | "final" => -1,
        _ => return None,
    })
}

/// bars named in the words: "bars 9 to 16", "bars 9-16", "bar 5" -> (start beat, end beat)
fn bars_range(w: &[String]) -> Option<(f64, f64)> {
    for (i, x) in w.iter().enumerate() {
        if x == "bars" || x == "bar" {
            let a = w.get(i + 1)?;
            if let Some((l, r)) = a.split_once('-') {
                let (l, r) = (l.parse::<f64>().ok()?, r.parse::<f64>().ok()?);
                return Some(((l - 1.0).max(0.0) * 4.0, r * 4.0));
            }
            let l = a.parse::<f64>().ok()?;
            let r = match (w.get(i + 2).map(|s| s.as_str()), w.get(i + 3)) {
                (Some("to") | Some("through") | Some("until"), Some(b)) => b.parse::<f64>().ok().unwrap_or(l),
                (Some(n), _) if n.parse::<f64>().is_ok() => n.parse::<f64>().unwrap_or(l),
                _ => l,
            };
            return Some(((l - 1.0).max(0.0) * 4.0, r.max(l) * 4.0));
        }
    }
    None
}

/// The section a request means: bars, or a tag family with an optional ordinal ("the second verse", "the last chorus"), else the
/// selection when the words say "this" / "selected". `default_fam` supplies the family for "swap verse 1 and 2".
pub fn find_section(p: &Project, req: &str, sel: (f64, f64), default_fam: Option<&str>) -> Option<Tag> {
    let w = words_of(req);
    if let Some((s, e)) = bars_range(&w) {
        if e > s {
            return Some(Tag { s, e, name: "bars".into() });
        }
    }
    let tags = read_tags(p);
    let mut fam: Option<(&'static str, usize)> = None;
    for (i, x) in w.iter().enumerate() {
        if let Some(f) = tag_family(x) {
            fam = Some((f, i));
        }
    }
    let (fam, at) = match fam {
        Some((f, i)) => (f, Some(i)),
        None => match default_fam.and_then(tag_family) {
            Some(f) => (f, None),
            None => {
                let this = w.iter().any(|x| matches!(x.as_str(), "this" | "selected" | "selection" | "clip" | "here"));
                return if this && sel.1 > sel.0 { Some(Tag { s: sel.0, e: sel.1, name: "selection".into() }) } else { None };
            }
        },
    };
    let cands: Vec<&Tag> = tags.iter().filter(|t| family_matches(fam, &t.name)).collect();
    if cands.is_empty() {
        return None;
    }
    // the ordinal next to the section word ("2 times" and "4 bars" are counts, not ordinals)
    let ord_at = |j: usize| -> Option<i32> {
        let o = ordinal(w.get(j)?)?;
        let counted = w.get(j + 1).map(|n| matches!(n.as_str(), "times" | "time" | "bars" | "bar" | "bpm" | "beats" | "x" | "percent")).unwrap_or(false);
        if counted { None } else { Some(o) }
    };
    let mut nth: Option<i32> = None;
    match at {
        Some(i) => {
            for j in (i.saturating_sub(2)..i).rev() {
                if let Some(o) = ord_at(j) {
                    nth = Some(o);
                    break;
                }
            }
            if nth.is_none() {
                nth = ord_at(i + 1);
            }
        }
        None => {
            nth = (0..w.len()).filter_map(ord_at).next();
        }
    }
    let pick = match nth {
        Some(-1) => cands.last().copied(),
        Some(n) => cands.get((n - 1) as usize).copied(),
        None => cands.first().copied(),
    };
    pick.cloned()
}

// ── splitting and shifting ────────────────────────────────────────────────────────────────────────────────────────────

fn unloop(c: &mut Clip) {
    if c.loop_len < 0.25 {
        return;
    }
    let l = c.loop_len;
    let len = c.len;
    if let ClipData::Midi(n) = &mut c.data {
        let base: Vec<Note> = n.iter().filter(|x| x.start < l).cloned().collect();
        let mut out = vec![];
        let mut k = 0.0;
        while k * l < len - EPS && out.len() < 200_000 {
            for b in &base {
                let st = b.start + k * l;
                if st < len - EPS {
                    let mut x = b.clone();
                    x.start = st;
                    if x.len > len - st {
                        x.cut(0.0, len - st);
                    }
                    out.push(x);
                }
            }
            k += 1.0;
        }
        *n = out;
        c.loop_len = 0.0;
    }
}

/// cut a clip in two at a beat inside it; the clip keeps the left part, the right part is returned
fn split_clip(c: &mut Clip, at: f64, secs: f64) -> Option<Clip> {
    let rel = at - c.start;
    if rel <= EPS || rel >= c.len - EPS {
        return None;
    }
    unloop(c);
    let mut right = c.clone();
    right.start = at;
    right.len = c.len - rel;
    c.len = rel;
    if let (ClipData::Midi(l), ClipData::Midi(r)) = (&mut c.data, &mut right.data) {
        r.clear();
        let all = std::mem::take(l);
        for n in all {
            if n.start >= rel - EPS {
                let mut m = n;
                m.start -= rel;
                r.push(m);
            } else {
                let end = n.start + n.len;
                let mut a = n.clone();
                if end > rel {
                    // cut with its expression curves kept in place on both sides
                    a.cut(0.0, rel - n.start);
                    if end - rel >= 0.25 {
                        let mut b = n;
                        let l0 = b.len;
                        b.cut(rel - b.start, l0);
                        b.start = 0.0;
                        r.push(b);
                    }
                }
                l.push(a);
            }
        }
    }
    if let ClipData::Audio { offset_s, .. } = &mut right.data {
        *offset_s += secs;
    }
    Some(right)
}

fn split_all(p: &mut Project, at: f64) {
    let mut todo = vec![];
    for (ti, t) in p.tracks.iter().enumerate() {
        for (ci, c) in t.clips.iter().enumerate() {
            if c.start + EPS < at && at < c.end() - EPS {
                todo.push((ti, ci, p.beats_to_secs(c.start, at)));
            }
        }
    }
    for (ti, ci, secs) in todo {
        if let Some(r) = split_clip(&mut p.tracks[ti].clips[ci], at, secs) {
            p.tracks[ti].clips.push(r);
        }
    }
    for t in p.tracks.iter_mut() {
        t.clips.sort_by(|a, b| a.start.total_cmp(&b.start));
    }
}

fn all_lanes(p: &mut Project) -> Vec<&mut AutoLane> {
    let mut v: Vec<&mut AutoLane> = vec![];
    for t in p.tracks.iter_mut() {
        for l in t.auto.iter_mut() {
            v.push(l);
        }
    }
    for l in p.master_auto.iter_mut() {
        v.push(l);
    }
    v
}

/// open a gap: everything at or after `at` moves by `delta` (clips, lane points, tempo changes, tags)
fn shift_after(p: &mut Project, at: f64, delta: f64) {
    for t in p.tracks.iter_mut() {
        for c in t.clips.iter_mut() {
            if c.start >= at - EPS {
                c.start += delta;
            }
        }
    }
    for l in all_lanes(p) {
        for pt in l.points.iter_mut() {
            if pt.0 >= at - EPS {
                pt.0 += delta;
            }
        }
        l.points.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
    for tm in p.tempo_map.iter_mut() {
        if tm.0 >= at - EPS {
            tm.0 += delta;
        }
    }
    let mut tags = read_tags(p);
    for t in tags.iter_mut() {
        if t.s >= at - EPS {
            t.s += delta;
        }
        if t.e > at + EPS {
            t.e += delta;
        }
    }
    if !tags.is_empty() {
        write_tags(p, tags);
    }
}

/// move pieces of the timeline by their own offsets (after splitting at every piece boundary)
fn remap(p: &mut Project, pieces: &[(f64, f64, f64)]) {
    let f = |x: f64| -> f64 { pieces.iter().find(|(a, b, _)| x >= *a - EPS && x < *b - EPS).map(|(_, _, d)| x + d).unwrap_or(x) };
    for t in p.tracks.iter_mut() {
        for c in t.clips.iter_mut() {
            c.start = f(c.start);
        }
        t.clips.sort_by(|a, b| a.start.total_cmp(&b.start));
    }
    for l in all_lanes(p) {
        for pt in l.points.iter_mut() {
            pt.0 = f(pt.0);
        }
        l.points.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
    for tm in p.tempo_map.iter_mut() {
        tm.0 = f(tm.0);
    }
    p.tempo_map.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut tags = read_tags(p);
    for t in tags.iter_mut() {
        let len = t.e - t.s;
        let ns = f(t.s);
        let ne = f(t.e - 2.0 * EPS) + 2.0 * EPS;
        t.s = ns;
        t.e = if ne > ns + EPS { ne } else { ns + len };
    }
    if !tags.is_empty() {
        write_tags(p, tags);
    }
}

/// take a span out: everything inside goes, everything after closes the gap
fn remove_range(p: &mut Project, s: f64, e: f64) {
    split_all(p, s);
    split_all(p, e);
    let len = e - s;
    for t in p.tracks.iter_mut() {
        t.clips.retain(|c| !(c.start >= s - EPS && c.start < e - EPS));
    }
    // the tempo in force at the end of the span carries on after it
    let at_e = p.bpm_at(e);
    p.tempo_map.retain(|(b, _)| !(*b >= s - EPS && *b < e - EPS));
    let before = p.bpm_at(s);
    if (at_e - before).abs() > 1e-9 && !p.tempo_map.iter().any(|(b, _)| (*b - e).abs() < EPS) {
        p.tempo_map.push((e, at_e));
        p.tempo_map.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
    for l in all_lanes(p) {
        l.points.retain(|pt| !(pt.0 >= s - EPS && pt.0 < e - EPS));
    }
    let mut tags = read_tags(p);
    let g = |x: f64| if x <= s { x } else if x < e { s } else { x - len };
    tags.retain(|t| !(t.s >= s - EPS && t.e <= e + EPS));
    for t in tags.iter_mut() {
        t.s = g(t.s);
        t.e = g(t.e);
    }
    shift_after(p, e, -len);
    // the tags were mapped above (shift_after moved the stored ones a second time): write the mapped copy
    if p.meta.iter().any(|(k, v)| k == "sections" && !v.trim().is_empty()) {
        write_tags(p, tags);
    }
}

// ── copying a span ────────────────────────────────────────────────────────────────────────────────────────────────────

struct Snap {
    len: f64,
    clips: Vec<(usize, Clip)>,
    lanes: Vec<(Option<usize>, String, Vec<(f64, f32)>)>,
    tempos: Vec<(f64, f64)>,
    tags: Vec<Tag>,
}

/// the span [s, e) as data, positions relative to s (split at both ends first)
fn snapshot(p: &mut Project, s: f64, e: f64) -> Snap {
    split_all(p, s);
    split_all(p, e);
    let mut sn = Snap { len: e - s, clips: vec![], lanes: vec![], tempos: vec![], tags: vec![] };
    for (ti, t) in p.tracks.iter().enumerate() {
        for c in &t.clips {
            if c.start >= s - EPS && c.end() <= e + EPS && c.len > EPS {
                let mut x = c.clone();
                x.start = c.start - s;
                sn.clips.push((ti, x));
            }
        }
        for l in &t.auto {
            if let Some(pts) = lane_slice(l, s, e) {
                sn.lanes.push((Some(ti), l.param.clone(), pts));
            }
        }
    }
    for l in &p.master_auto {
        if let Some(pts) = lane_slice(l, s, e) {
            sn.lanes.push((None, l.param.clone(), pts));
        }
    }
    for (b, v) in &p.tempo_map {
        if *b >= s - EPS && *b < e - EPS {
            sn.tempos.push((b - s, *v));
        }
    }
    if p.tempo_map.iter().any(|(b, _)| *b >= s - EPS && *b < e - EPS) && !sn.tempos.iter().any(|(b, _)| *b < EPS) {
        sn.tempos.insert(0, (0.0, p.bpm_at(s)));
    }
    for t in read_tags(p) {
        if t.s >= s - EPS && t.e <= e + EPS {
            sn.tags.push(Tag { s: t.s - s, e: t.e - s, name: t.name });
        }
    }
    sn
}

fn lane_slice(l: &AutoLane, s: f64, e: f64) -> Option<Vec<(f64, f32)>> {
    if l.points.is_empty() {
        return None;
    }
    let mut pts: Vec<(f64, f32)> = l.points.iter().filter(|p| p.0 >= s - EPS && p.0 < e - EPS).map(|p| (p.0 - s, p.1)).collect();
    if !pts.iter().any(|p| p.0 < EPS) {
        pts.insert(0, (0.0, l.value_at(s).unwrap_or(0.0)));
    }
    Some(pts)
}

fn cut_tail(c: &mut Clip, newlen: f64) {
    if let ClipData::Midi(n) = &mut c.data {
        n.retain(|x| x.start < newlen - EPS);
        for x in n.iter_mut() {
            if x.len > newlen - x.start {
                x.cut(0.0, newlen - x.start);
            }
        }
    }
    c.len = newlen;
}

/// put a snapshot (the first `limit` beats of it) down at `at`
fn place(p: &mut Project, sn: &Snap, at: f64, limit: f64, with_tags: bool) {
    for (ti, c) in &sn.clips {
        if c.start >= limit - EPS {
            continue;
        }
        let mut x = c.clone();
        x.start += at;
        if c.start + c.len > limit + EPS {
            cut_tail(&mut x, limit - c.start);
        }
        if let Some(t) = p.tracks.get_mut(*ti) {
            t.clips.push(x);
            t.clips.sort_by(|a, b| a.start.total_cmp(&b.start));
        }
    }
    for (ti, param, pts) in &sn.lanes {
        let lanes = match ti {
            Some(i) => match p.tracks.get_mut(*i) {
                Some(t) => &mut t.auto,
                None => continue,
            },
            None => &mut p.master_auto,
        };
        let li = match lanes.iter().position(|l| &l.param == param) {
            Some(i) => i,
            None => {
                lanes.push(AutoLane { param: param.clone(), points: vec![] });
                lanes.len() - 1
            }
        };
        let l = &mut lanes[li];
        for (b, v) in pts {
            if *b < limit - EPS {
                l.points.retain(|q| (q.0 - (at + b)).abs() > EPS);
                l.points.push((at + b, *v));
            }
        }
        l.points.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
    for (b, v) in &sn.tempos {
        if *b < limit - EPS {
            p.tempo_map.retain(|q| (q.0 - (at + b)).abs() > EPS);
            p.tempo_map.push((at + b, *v));
        }
    }
    p.tempo_map.sort_by(|a, b| a.0.total_cmp(&b.0));
    if with_tags && limit >= sn.len - EPS {
        let mut tags = read_tags(p);
        for t in &sn.tags {
            tags.push(Tag { s: t.s + at, e: t.e + at, name: t.name.clone() });
        }
        write_tags(p, tags);
    }
}

/// copy [src_s, src_e) into a new gap of `dlen` beats opened at `at`, looping it to fill
fn insert_copy(p: &mut Project, src_s: f64, src_e: f64, at: f64, dlen: f64, with_tags: bool) {
    let sn = snapshot(p, src_s, src_e);
    split_all(p, at);
    shift_after(p, at, dlen);
    let mut pos = 0.0;
    while pos < dlen - EPS {
        let lim = sn.len.min(dlen - pos);
        place(p, &sn, at + pos, lim, with_tags);
        pos += sn.len;
    }
}

// ── the section operations ────────────────────────────────────────────────────────────────────────────────────────────

pub fn repeat_section(p: &mut Project, s: f64, e: f64, times: usize) -> Result<f64, String> {
    if e - s < 0.5 || times == 0 {
        return Err("nothing to repeat".into());
    }
    if times > 16 {
        return Err("that is too many repeats".into());
    }
    let len = e - s;
    let sn = snapshot(p, s, e);
    shift_after(p, e, len * times as f64);
    for k in 1..=times {
        place(p, &sn, s + len * k as f64, len, true);
    }
    Ok(e + len * times as f64)
}

pub fn resize_section(p: &mut Project, s: f64, e: f64, new_len: f64) -> Result<f64, String> {
    let len = e - s;
    if len < 0.5 || new_len < 1.0 {
        return Err("a section needs at least a beat".into());
    }
    if (new_len - len).abs() < EPS {
        return Ok(e);
    }
    if new_len < len {
        // shorten: the tail goes
        remove_range(p, s + new_len, e);
        return Ok(s + new_len);
    }
    let d = new_len - len;
    insert_copy(p, s, e, e, d, false);
    // the tag that ended where the section ended grows with it
    let mut tags = read_tags(p);
    if let Some(t) = tags.iter_mut().find(|t| (t.e - e).abs() < EPS && t.s <= s + EPS) {
        t.e = e + d;
    }
    write_tags(p, tags);
    Ok(e + d)
}

pub fn swap_sections(p: &mut Project, a: (f64, f64), b: (f64, f64)) -> Result<(), String> {
    let (a, b) = if a.0 <= b.0 { (a, b) } else { (b, a) };
    if a.1 > b.0 + EPS || a.1 - a.0 < 0.5 || b.1 - b.0 < 0.5 {
        return Err("the two sections overlap".into());
    }
    for x in [a.0, a.1, b.0, b.1] {
        split_all(p, x);
    }
    let (la, lb, mid) = (a.1 - a.0, b.1 - b.0, b.0 - a.1);
    let mut pieces = vec![(a.0, a.1, lb + mid), (b.0, b.1, -(b.0 - a.0))];
    if mid > EPS {
        pieces.push((a.1, b.0, lb - la));
    }
    remap(p, &pieces);
    Ok(())
}

/// move [a0, a1) so that it starts where the boundary `to` is (a boundary of the song as it is now)
pub fn move_section(p: &mut Project, a0: f64, a1: f64, to: f64) -> Result<(), String> {
    if to > a0 + EPS && to < a1 - EPS {
        return Err("that is inside the section itself".into());
    }
    let la = a1 - a0;
    for x in [a0, a1, to] {
        split_all(p, x);
    }
    let pieces = if to >= a1 - EPS { vec![(a0, a1, to - a1), (a1, to, -la)] } else { vec![(to, a0, la), (a0, a1, -(a0 - to))] };
    remap(p, &pieces);
    Ok(())
}

pub fn set_tempo(p: &mut Project, from: f64, to: f64, bpm: f64) {
    let bpm = bpm.clamp(20.0, 400.0);
    let after = if to > from + EPS { Some(p.bpm_at(to)) } else { None };
    if from <= EPS && after.is_none() {
        p.bpm = bpm;
        p.tempo_map.retain(|(b, _)| *b > EPS);
        return;
    }
    p.tempo_map.retain(|(b, _)| !(*b >= from - EPS && *b < to.max(from) - EPS) && (*b - from).abs() > EPS);
    p.tempo_map.push((from, bpm));
    if let Some(a) = after {
        if !p.tempo_map.iter().any(|(b, _)| (*b - to).abs() < EPS) {
            p.tempo_map.push((to, a));
        }
    }
    p.tempo_map.sort_by(|a, b| a.0.total_cmp(&b.0));
}

// ── parts of the song ─────────────────────────────────────────────────────────────────────────────────────────────────

#[derive(PartialEq, Clone, Copy, Debug)]
pub enum Part {
    Drums,
    Bass,
    Pad,
    Other,
}

pub fn part_of(t: &Track) -> Part {
    let n = t.name.to_lowercase();
    let has = |ws: &[&str]| ws.iter().any(|w| n.contains(w));
    if t.inst_kind == "drums" || has(&["drum", "kick", "snare", "hat", "perc", "beat"]) {
        Part::Drums
    } else if has(&["bass", "sub", "808", "reese", "growl"]) && !has(&["lead"]) {
        Part::Bass
    } else if has(&["pad", "string", "choir", "chord", "ambient", "atmos", "organ", "keys"]) {
        Part::Pad
    } else {
        Part::Other
    }
}

fn is_melodic(t: &Track) -> bool {
    !matches!(t.kind, TrackKind::Audio) && part_of(t) != Part::Drums
}

/// the track a request names ("the bass", "the drums", "the lead"), else the selected one
pub fn track_by_words(p: &Project, req: &str, selected: i64) -> Option<usize> {
    let w = words_of(req);
    let roles: [(&[&str], &[&str]); 8] = [
        (&["bass", "sub", "808"], &["bass", "sub", "808", "reese", "growl"]),
        (&["drums", "drum", "kick", "beat", "percussion"], &["drum", "kick", "perc"]),
        (&["lead", "melody", "synth"], &["lead", "melody", "synth", "saw"]),
        (&["pad", "pads", "strings"], &["pad", "string"]),
        (&["chords", "chord", "keys", "piano"], &["chord", "keys", "piano"]),
        (&["arp", "arpeggio"], &["arp", "pluck"]),
        (&["vocals", "vocal", "voice", "singer"], &["vocal", "voice", "aria", "sing"]),
        (&["guitar"], &["guitar"]),
    ];
    for (words, names) in roles {
        if w.iter().any(|x| words.contains(&x.as_str())) {
            for (i, t) in p.tracks.iter().enumerate() {
                let n = t.name.to_lowercase();
                let drums = t.inst_kind == "drums";
                if names.iter().any(|k| n.contains(k)) || (words[0] == "drums" && drums) {
                    return Some(i);
                }
            }
        }
    }
    for (i, t) in p.tracks.iter().enumerate() {
        let n = t.name.to_lowercase();
        if n.len() > 3 && w.iter().any(|x| x.len() > 3 && n.contains(x.as_str())) {
            return Some(i);
        }
    }
    if selected >= 0 && (selected as usize) < p.tracks.len() {
        return Some(selected as usize);
    }
    None
}

// ── lanes ─────────────────────────────────────────────────────────────────────────────────────────────────────────────

fn lane_base(t: &Track, param: &str, beat: f64) -> f32 {
    if let Some(l) = t.auto.iter().find(|l| l.param == param && !l.points.is_empty()) {
        return l.value_at(beat).unwrap_or(0.0);
    }
    match param {
        "vol" => t.volume,
        "pan" => t.pan,
        _ => t.inst.as_ref().and_then(|i| i.params().into_iter().find(|(n, _)| *n == param).map(|x| x.1)).unwrap_or(0.5),
    }
}

/// Write points of a lane over [s, e]. The values before and after stay what they were (`restore`), or the lane holds its last
/// value (a fade out). A pitch-altering parameter only gets written when the request asks for pitch.
fn lane_write(t: &mut Track, param: &str, request: &str, s: f64, e: f64, pts: &[(f64, f32)], restore: bool) -> bool {
    if param != "vol" && param != "pan" && !redesign::param_allowed(param, request) {
        return false;
    }
    if pts.is_empty() {
        return false;
    }
    let base_s = lane_base(t, param, s);
    let base_e = lane_base(t, param, e);
    let li = match t.auto.iter().position(|l| l.param == param) {
        Some(i) => i,
        None => {
            t.auto.push(AutoLane { param: param.to_string(), points: vec![] });
            t.auto.len() - 1
        }
    };
    let l = &mut t.auto[li];
    l.points.retain(|p| p.0 < s - 0.02 || p.0 > e + 0.02);
    if s > 0.02 {
        l.points.push((s - 0.01, base_s));
    }
    for (b, v) in pts {
        l.points.push((*b, *v));
    }
    if restore {
        l.points.push((e + 0.01, base_e));
    }
    l.points.sort_by(|a, b| a.0.total_cmp(&b.0));
    true
}

fn filter_param(t: &Track) -> Option<String> {
    let inst = t.inst.as_ref()?;
    let names: Vec<&'static str> = inst.params().into_iter().map(|(n, _)| n).collect();
    for want in ["cutoff", "cut"] {
        if names.contains(&want) {
            return Some(want.to_string());
        }
    }
    names.into_iter().find(|n| role_of(n) == Role::Filter && redesign::movable(n)).map(|n| n.to_string())
}

/// automation by words over [s, e]: what was written
pub fn auto_words(t: &mut Track, request: &str, s: f64, e: f64) -> Vec<String> {
    let q = request.to_lowercase();
    let has = |ws: &[&str]| ws.iter().any(|w| q.contains(w));
    let mut done = vec![];
    if e - s < 0.5 {
        return done;
    }
    let vol = if t.volume > 0.0 { t.volume } else { 0.8 };
    let fp = filter_param(t);
    let ramp_words = has(&["sweep up", "riser", "rise", "build", "rising", "filter sweep", "sweep"]) && !has(&["sweep down", "close", "muffle"]);
    if let Some(fp) = &fp {
        if has(&["close the filter", "close filter", "muffle", "sweep down", "filter down", "darken", "low pass down"]) {
            let b = lane_base(t, fp, s);
            if lane_write(t, fp, request, s, e, &[(s, b), (e - 0.01, 0.15)], true) {
                done.push("filter closing".to_string());
            }
        } else if ramp_words && !has(&["open the filter", "open up the filter", "filter open"]) {
            let b = lane_base(t, fp, s);
            let lo = b.min(0.25);
            if lane_write(t, fp, request, s, e, &[(s, lo), (e - 0.01, 0.95)], true) {
                done.push("filter sweeping up".to_string());
            }
        } else if has(&["open the filter", "open up the filter", "filter open", "open filter", "brighter in the", "opens up"]) {
            if lane_write(t, fp, request, s, e, &[(s, 0.92), (e - 0.01, 0.92)], true) {
                done.push("filter open".to_string());
            }
        }
    }
    if has(&["swell", "crescendo", "fade in", "fade-in", "fades in", "riser"]) {
        let b = lane_base(t, "vol", s).max(vol * 0.5);
        if lane_write(t, "vol", request, s, e, &[(s, b * 0.2), (e - 0.01, b)], true) {
            done.push("volume swelling".to_string());
        }
    } else if has(&["fade out", "fade-out", "fadeout", "fade away", "decrescendo", "fades out"]) {
        let b = lane_base(t, "vol", s);
        if lane_write(t, "vol", request, s, e, &[(s, b), (e, 0.0)], false) {
            done.push("fading out".to_string());
        }
    }
    if has(&["pump", "sidechain", "side-chain", "duck"]) {
        let b = lane_base(t, "vol", s).max(vol * 0.5);
        let mut pts = vec![];
        let mut x = s;
        while x < e - 0.5 && pts.len() < 2000 {
            pts.push((x, b * 0.28));
            pts.push((x + 0.7, b));
            x += 1.0;
        }
        if lane_write(t, "vol", request, s, e, &pts, true) {
            done.push("pumping".to_string());
        }
    }
    // a momentary bend lead-in, only for a riser or an explicit bend, at most the last bar, back to rest at the end
    if has(&["riser", "bend", "pitch"]) && redesign::request_allows_pitch(request) {
        if let Some(inst) = t.inst.as_ref() {
            if inst.params().iter().any(|(n, _)| *n == "bend") {
                let b = lane_base(t, "bend", e);
                let s2 = (e - 4.0).max(s);
                if lane_write(t, "bend", request, s2, e, &[(s2, b), (e - 0.05, (b + 0.12).min(1.0))], true) {
                    done.push("a short bend lead-in".to_string());
                }
            }
        }
    }
    done
}

/// take a part out (mode 0), bring it back (1), or duck everything else under it (2) over [s, e]
pub fn part_in_section(p: &mut Project, ti: usize, s: f64, e: f64, mode: u8) -> Result<usize, String> {
    if ti >= p.tracks.len() || e - s < 0.5 {
        return Err("no such track or section".into());
    }
    let mut n = 0;
    match mode {
        0 | 1 => {
            let t = &mut p.tracks[ti];
            let b = lane_base(t, "vol", s);
            let b = if b > 0.01 { b } else if t.volume > 0.05 { t.volume } else { 0.8 };
            let pts: Vec<(f64, f32)> = if mode == 0 { vec![(s, 0.0), (e - 0.02, 0.0)] } else { vec![(s, b), (e - 0.02, b)] };
            if lane_write(t, "vol", "", s, e, &pts, true) {
                n = 1;
            }
        }
        _ => {
            for (i, t) in p.tracks.iter_mut().enumerate() {
                if i == ti || matches!(t.kind, TrackKind::Audio) && t.clips.is_empty() {
                    continue;
                }
                let b = lane_base(t, "vol", s);
                if lane_write(t, "vol", "", s, e, &[(s, b * 0.4), (e - 0.02, b * 0.4)], true) {
                    n += 1;
                }
            }
        }
    }
    Ok(n)
}

// ── drums ─────────────────────────────────────────────────────────────────────────────────────────────────────────────

fn is_kick(k: u8) -> bool {
    matches!(k, 35 | 36)
}
fn is_snare(k: u8) -> bool {
    matches!(k, 37..=40)
}
fn is_hat(k: u8) -> bool {
    matches!(k, 42 | 44 | 46)
}

/// half-time (mode 0): the backbeat moves to beat 3, kicks thin out, hats halve. double-time (1): each bar's pattern plays twice
/// as fast, twice. Returns the notes changed.
pub fn drum_feel(p: &mut Project, s: f64, e: f64, mode: u8) -> usize {
    split_all(p, s);
    split_all(p, e);
    let mut changed = 0;
    for t in p.tracks.iter_mut() {
        if part_of(t) != Part::Drums || matches!(t.kind, TrackKind::Audio) {
            continue;
        }
        for c in t.clips.iter_mut() {
            if c.start < s - EPS || c.end() > e + EPS {
                continue;
            }
            unloop(c);
            let cs = c.start;
            let clen = c.len;
            let ClipData::Midi(notes) = &mut c.data else { continue };
            let old = std::mem::take(notes);
            let mut out: Vec<Note> = vec![];
            if mode == 0 {
                let mut hat_idx = std::collections::HashMap::<i64, usize>::new();
                let mut sorted = old.clone();
                sorted.sort_by(|a, b| a.start.total_cmp(&b.start));
                for n in sorted {
                    let abs = cs + n.start;
                    let pos = abs.rem_euclid(4.0);
                    let bar = (abs / 4.0).floor() as i64;
                    if is_kick(n.key) {
                        if pos < 0.5 || (2.5..3.0).contains(&pos) {
                            out.push(n);
                        } else {
                            changed += 1;
                        }
                    } else if is_snare(n.key) {
                        if (1.0..2.0).contains(&pos) {
                            let mut m = n;
                            m.start += 1.0;
                            out.push(m);
                            changed += 1;
                        } else if pos >= 3.0 {
                            changed += 1;
                        } else {
                            out.push(n);
                        }
                    } else if is_hat(n.key) {
                        let i = hat_idx.entry(bar).or_insert(0);
                        *i += 1;
                        if *i % 2 == 1 {
                            out.push(n);
                        } else {
                            changed += 1;
                        }
                    } else {
                        out.push(n);
                    }
                }
            } else {
                for n in old {
                    let abs = cs + n.start;
                    let pos = abs.rem_euclid(4.0);
                    let bar0 = abs - pos;
                    let half = pos / 2.0;
                    for k in 0..2 {
                        let mut m = n.clone();
                        m.start = bar0 + half + 2.0 * k as f64 - cs;
                        m.len = (n.len / 2.0).max(0.05);
                        if m.start < clen - EPS {
                            out.push(m);
                        }
                    }
                    changed += 1;
                }
            }
            out.sort_by(|a, b| a.start.total_cmp(&b.start));
            *notes = out;
        }
    }
    changed
}

fn clip_index_at(t: &Track, beat: f64) -> Option<usize> {
    t.clips.iter().position(|c| matches!(c.data, ClipData::Midi(_)) && c.start <= beat + EPS && beat < c.end() - EPS)
}

fn add_note_abs(t: &mut Track, abs: f64, len: f64, key: u8, vel: f32) -> bool {
    let Some(ci) = clip_index_at(t, abs) else { return false };
    let c = &mut t.clips[ci];
    let rel = abs - c.start;
    if let ClipData::Midi(n) = &mut c.data {
        n.push(Note::new(rel, len.min(c.len - rel).max(0.05), key, vel));
        return true;
    }
    false
}

/// Insert a build of `bars` bars before `at`: the material just before it is copied into the gap and thinned (bass out for the first
/// half), drums get a snare roll that speeds up, velocities and filters rise. Returns the build's span.
pub fn insert_build(p: &mut Project, at: f64, bars: f64) -> Result<(f64, f64), String> {
    let dlen = (bars.clamp(1.0, 32.0)) * 4.0;
    if at < 4.0 {
        return Err("there is no music before that to build from".into());
    }
    let src_s = (at - dlen).max(0.0);
    insert_copy(p, src_s, at, at, dlen, false);
    let (b0, b1) = (at, at + dlen);
    let half = b0 + dlen / 2.0;
    for t in p.tracks.iter_mut() {
        if matches!(t.kind, TrackKind::Audio) {
            continue;
        }
        let part = part_of(t);
        for c in t.clips.iter_mut() {
            if c.start < b0 - EPS || c.end() > b1 + EPS {
                continue;
            }
            let cs = c.start;
            if let ClipData::Midi(ns) = &mut c.data {
                match part {
                    Part::Drums => ns.retain(|n| !(is_kick(n.key) && cs + n.start < half)),
                    Part::Bass => ns.retain(|n| cs + n.start >= half),
                    _ => {}
                }
                if part != Part::Drums {
                    for n in ns.iter_mut() {
                        let f = ((cs + n.start - b0) / dlen).clamp(0.0, 1.0) as f32;
                        n.vel = (n.vel * (0.55 + 0.45 * f)).clamp(0.05, 1.0);
                    }
                }
            }
        }
        match part {
            Part::Drums => {
                // the roll: quarter notes, then eighths, then sixteenths, then thirty-seconds in the last bar
                let mut x = half;
                while x < b1 - EPS {
                    let step = if x < b0 + dlen * 0.625 { 1.0 } else if x < b0 + dlen * 0.8125 { 0.5 } else if x < b1 - 4.0 { 0.25 } else { 0.125 };
                    let f = ((x - half) / (b1 - half)).clamp(0.0, 1.0) as f32;
                    add_note_abs(t, x, step * 0.9, 38, 0.4 + 0.6 * f);
                    x += step;
                }
            }
            Part::Bass => {}
            _ => {
                if let Some(fp) = filter_param(t) {
                    let b = lane_base(t, &fp, b0);
                    lane_write(t, &fp, "build", b0, b1, &[(b0, b.min(0.25)), (b1 - 0.01, 0.95)], true);
                }
            }
        }
    }
    let mut tags = read_tags(p);
    tags.push(Tag { s: b0, e: b1, name: "build".into() });
    write_tags(p, tags);
    Ok((b0, b1))
}

/// Strip a span down: drums and bass out, pads stay, the rest softer. Returns the names of the parts taken out.
pub fn breakdown(p: &mut Project, s: f64, e: f64) -> Result<Vec<String>, String> {
    if e - s < 2.0 {
        return Err("that span is too short for a breakdown".into());
    }
    split_all(p, s);
    split_all(p, e);
    let mut out = vec![];
    let mut has_pad = false;
    for t in p.tracks.iter_mut() {
        if matches!(t.kind, TrackKind::Audio) {
            continue;
        }
        let part = part_of(t);
        if part == Part::Pad && t.clips.iter().any(|c| c.start >= s - EPS && c.end() <= e + EPS) {
            has_pad = true;
        }
        match part {
            Part::Drums | Part::Bass => {
                let before = t.clips.len();
                t.clips.retain(|c| !(c.start >= s - EPS && c.end() <= e + EPS));
                if t.clips.len() < before {
                    out.push(t.name.clone());
                }
            }
            Part::Other => {
                for c in t.clips.iter_mut() {
                    if c.start >= s - EPS && c.end() <= e + EPS {
                        if let ClipData::Midi(ns) = &mut c.data {
                            for n in ns.iter_mut() {
                                n.vel = (n.vel * 0.8).max(0.05);
                            }
                        }
                    }
                }
            }
            Part::Pad => {}
        }
    }
    let mut tags = read_tags(p);
    match tags.iter_mut().find(|t| (t.s - s).abs() < EPS && (t.e - e).abs() < EPS) {
        Some(t) => t.name = "breakdown".into(),
        None => tags.push(Tag { s, e, name: "breakdown".into() }),
    }
    write_tags(p, tags);
    if !has_pad {
        out.push("(no pad track to carry it)".into());
    }
    Ok(out)
}

// ── layered harmonies ─────────────────────────────────────────────────────────────────────────────────────────────────

const MAJOR: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
const MINOR: [i32; 7] = [0, 2, 3, 5, 7, 8, 10];

/// (tonic pitch class, minor?) that fits the notes best
pub fn estimate_key(notes: &[Note]) -> (i32, bool) {
    let mut w = [0.0f64; 12];
    for n in notes {
        w[(n.key % 12) as usize] += n.len.max(0.1);
    }
    let mut best = (0, false, f64::MIN);
    for tonic in 0..12 {
        for minor in [false, true] {
            let sc = if minor { &MINOR } else { &MAJOR };
            let mut score = 0.0;
            for pc in 0..12 {
                let rel = (pc as i32 - tonic + 12) % 12;
                if sc.contains(&rel) {
                    score += w[pc];
                } else {
                    score -= 0.8 * w[pc];
                }
            }
            score += 0.35 * w[tonic as usize];
            if score > best.2 {
                best = (tonic, minor, score);
            }
        }
    }
    (best.0, best.1)
}

/// the note `deg` scale steps from `key` in the scale
pub fn shift_degree(key: u8, tonic: i32, minor: bool, deg: i32) -> u8 {
    let sc = if minor { &MINOR } else { &MAJOR };
    let rel = key as i32 - tonic;
    let oct = rel.div_euclid(12);
    let pc = rel.rem_euclid(12);
    let idx = sc.iter().rposition(|s| *s <= pc).unwrap_or(0) as i32;
    let extra = pc - sc[idx as usize];
    let total = oct * 7 + idx + deg;
    let n = tonic + total.div_euclid(7) * 12 + sc[total.rem_euclid(7) as usize] + extra;
    n.clamp(0, 127) as u8
}

/// the notes of a layer over `src` (kind: thirds, sixths, fifths, octave, choir; "below" / "down" puts it underneath)
pub fn layer_notes(src: &[Note], kind: &str, clip_len: f64) -> (Vec<Note>, &'static str) {
    let k = kind.to_lowercase();
    let down = ["below", "under", "lower", "down", "beneath"].iter().any(|w| k.contains(w));
    let (tonic, minor) = estimate_key(src);
    let mut out = vec![];
    if k.contains("choir") || k.contains("pad") {
        // one slow chord a bar from the line's strongest note, an octave down
        let bars = (clip_len / 4.0).ceil() as usize;
        for b in 0..bars {
            let (b0, b1) = (b as f64 * 4.0, b as f64 * 4.0 + 4.0);
            let mut w = std::collections::HashMap::<u8, f64>::new();
            for n in src.iter().filter(|n| n.start >= b0 - EPS && n.start < b1 - EPS) {
                *w.entry(n.key).or_insert(0.0) += n.len;
            }
            let Some((root, _)) = w.into_iter().max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0))) else { continue };
            let len = (clip_len - b0).min(4.0) - 0.05;
            for d in [0, 2, 4] {
                let key = shift_degree(root, tonic, minor, d).saturating_sub(12).max(24);
                let mut n = Note::new(b0, len, key, 0.55);
                n.pan = 0.0;
                out.push(n);
            }
        }
        return (out, "choir");
    }
    let (deg, semis, label): (i32, i32, &'static str) = if k.contains("sixth") {
        (5, 0, "sixths")
    } else if k.contains("fifth") || k.contains("power") {
        (4, 0, "fifths")
    } else if k.contains("octave") {
        (0, 12, "octave")
    } else {
        (2, 0, "thirds")
    };
    for n in src {
        let mut m = n.clone();
        m.key = if semis != 0 {
            (n.key as i32 + if down { -semis } else { semis }).clamp(0, 127) as u8
        } else {
            shift_degree(n.key, tonic, minor, if down { -deg } else { deg })
        };
        m.vel = (n.vel * 0.8).clamp(0.05, 1.0);
        out.push(m);
    }
    (out, label)
}

// ── the builtins ──────────────────────────────────────────────────────────────────────────────────────────────────────

fn num_in(w: &[String], before: &[&str]) -> Option<f64> {
    for (i, x) in w.iter().enumerate() {
        if let Ok(v) = x.trim_end_matches("bpm").trim_end_matches('%').trim_end_matches('x').parse::<f64>() {
            if w.get(i + 1).map(|n| before.contains(&n.as_str())).unwrap_or(false) || (before.contains(&"bpm") && x.ends_with("bpm")) || (before.contains(&"x") && x.ends_with('x')) {
                return Some(v);
            }
        }
    }
    None
}

fn word_count(w: &[String], names: &[&str]) -> bool {
    w.iter().any(|x| names.contains(&x.as_str()))
}

fn sel_span(a: &A, i: usize) -> (f64, f64) {
    (a.n(i), a.n(i + 1))
}

impl Daw {
    pub(super) fn arrx_dispatch(&mut self, name: &str, a: &A) -> Result<Val, String> {
        let eng = self.out.engine.clone();
        macro_rules! e {
            () => {
                eng.lock().unwrap_or_else(|p| p.into_inner())
            };
        }
        macro_rules! num {
            ($v:expr) => {
                return Ok(Val::Num($v as f64))
            };
        }
        let r = match name {
            "daw_arrx_is_pitch" => num!(redesign::pitch_altering(&a.s(0)) as i32),
            "daw_arrx_find" => {
                // (request, sel_start, sel_end) -> [start, end, tag] or []
                let en = e!();
                return Ok(match find_section(&en.project, &a.s(0), sel_span(a, 1), None) {
                    Some(t) => Val::List(vec![Val::Num(t.s), Val::Num(t.e), Val::Str(t.name)]),
                    None => Val::List(vec![]),
                });
            }
            "daw_arrx_repeat" => {
                let r = repeat_section(&mut e!().project, a.n(0), a.n(1), a.n(2).max(1.0) as usize);
                r.map(|x| Val::Num(x))
            }
            "daw_arrx_remove" => {
                let (s, en_) = (a.n(0), a.n(1));
                if en_ - s < 0.5 {
                    Err("nothing to remove".to_string())
                } else {
                    remove_range(&mut e!().project, s, en_);
                    Ok(Val::Num(en_ - s))
                }
            }
            "daw_arrx_swap" => swap_sections(&mut e!().project, (a.n(0), a.n(1)), (a.n(2), a.n(3))).map(|_| Val::Num(1.0)),
            "daw_arrx_move" => move_section(&mut e!().project, a.n(0), a.n(1), a.n(2)).map(|_| Val::Num(1.0)),
            "daw_arrx_resize" => resize_section(&mut e!().project, a.n(0), a.n(1), a.n(2)).map(Val::Num),
            "daw_arrx_build" => insert_build(&mut e!().project, a.n(0), if a.n(1) > 0.0 { a.n(1) } else { 8.0 }).map(|(s, e)| Val::List(vec![Val::Num(s), Val::Num(e)])),
            "daw_arrx_breakdown" => breakdown(&mut e!().project, a.n(0), a.n(1)).map(|v| Val::List(v.into_iter().map(Val::Str).collect())),
            "daw_arrx_tempo" => {
                // (from beat, to beat or -1 for "to the end", bpm)
                set_tempo(&mut e!().project, a.n(0).max(0.0), a.n(1), a.n(2));
                Ok(Val::Num(1.0))
            }
            "daw_arrx_drumfeel" => {
                // (start, end, mode 0 half / 1 double)
                let n = drum_feel(&mut e!().project, a.n(0), a.n(1), if a.n(2) > 0.5 { 1 } else { 0 });
                Ok(Val::Num(n as f64))
            }
            "daw_arrx_part" => {
                // (track, start, end, mode 0 out / 1 in / 2 solo)
                part_in_section(&mut e!().project, a.u(0), a.n(1), a.n(2), a.n(3) as u8).map(|n| Val::Num(n as f64))
            }
            "daw_arrx_auto" => {
                // (track, words, start, end) -> what was written
                let mut en = e!();
                let end = if a.n(3) > a.n(2) { a.n(3) } else { song_end(&en.project) };
                match en.project.tracks.get_mut(a.u(0)) {
                    Some(t) => Ok(Val::List(auto_words(t, &a.s(1), a.n(2).max(0.0), end).into_iter().map(Val::Str).collect())),
                    None => Err("no such track".to_string()),
                }
            }
            "daw_arrx_synth" => {
                // (track, words) -> a sentence
                let req = a.s(1);
                let mut en = e!();
                let Some(tr) = en.project.tracks.get_mut(a.u(0)) else { return Ok(Val::Str(String::new())) };
                let tn = tr.name.clone();
                let Some(inst) = tr.inst.as_mut() else { return Ok(Val::Str("This track has no instrument".into())) };
                let params: Vec<(String, f32)> = inst.params().into_iter().map(|(n, v)| (n.to_string(), v)).collect();
                let pw = redesign::param_words(&req);
                let mut said: Vec<String> = pw.iter().map(|m| format!("{} {}", if m.1 > 0.0 { "more" } else { "less" }, m.2)).collect();
                let mut changes = redesign::apply_param_words(&params, &pw, &req, 1.0);
                if changes.is_empty() {
                    let (mv, words) = redesign::moves(&req);
                    if !mv.is_empty() {
                        said = words.iter().map(|w| w.to_string()).collect();
                        changes = redesign::tweak(&params, &mv, 1.0);
                    }
                }
                if changes.is_empty() {
                    return Ok(Val::Str(format!("{tn}: nothing to change (try longer release, shorter decay, more drive, wider, brighter ...). Pitch parameters (vibrato, glide, detune, bend) only move when you ask for pitch.")));
                }
                for (n, v) in &changes {
                    inst.set_param(n, *v);
                }
                drop(en);
                self.touch();
                return Ok(Val::Str(format!("{tn}: {} ({} parameters)", said.join(", "), changes.len())));
            }
            "daw_arrx_layer" => {
                // (track, clip or -1 for every MIDI clip, kind) -> the new track
                let (t, c, kind) = (a.u(0), a.n(1), a.s(2));
                let notes_for = |en: &crate::engine::Engine| -> Vec<(f64, f64, String, Vec<Note>)> {
                    let Some(tr) = en.project.tracks.get(t) else { return vec![] };
                    tr.clips.iter().enumerate().filter(|(i, _)| c < 0.0 || *i == c as usize).filter_map(|(_, cl)| if let ClipData::Midi(n) = &cl.data { if n.is_empty() { None } else { Some((cl.start, cl.len, cl.name.clone(), n.clone())) } } else { None }).collect()
                };
                let clips = notes_for(&e!());
                if clips.is_empty() {
                    return Err("there are no notes to layer".into());
                }
                let nt = match self.dispatch("daw_track_dup", &A(&[Val::Num(t as f64), Val::Num(0.0)]))? {
                    Val::Num(x) if x >= 0.0 => x as usize,
                    _ => return Err("could not add a track".into()),
                };
                let mut label = "thirds";
                let mut total = 0;
                {
                    let mut en = e!();
                    for (start, len, name, src) in clips {
                        let (notes, l) = layer_notes(&src, &kind, len);
                        label = l;
                        total += notes.len();
                        let mut cl = Clip::midi(start, len, &name);
                        cl.data = ClipData::Midi(notes);
                        if let Some(tr) = en.project.tracks.get_mut(nt) {
                            tr.clips.push(cl);
                        }
                    }
                    if let Some(tr) = en.project.tracks.get_mut(nt) {
                        let base = tr.name.split(" copy").next().unwrap_or("Track").to_string();
                        tr.name = format!("{base} {label}");
                    }
                }
                self.touch();
                return Ok(Val::List(vec![Val::Num(nt as f64), Val::Num(total as f64), Val::Str(label.into())]));
            }
            "daw_arrx_run" => return self.arrx_run(a),
            _ => return Err(format!("unknown builtin {name}")),
        };
        if r.is_ok() {
            self.touch();
        }
        r
    }

    /// (tool, request, selected track or -1, clip track or -1, clip or -1, seed) -> a sentence for the agent's log
    fn arrx_run(&mut self, a: &A) -> Result<Val, String> {
        let eng = self.out.engine.clone();
        let tool = a.s(0);
        let req = a.s(1);
        let sel_t = a.n(2) as i64;
        let (clip_t, clip_c) = (a.n(3) as i64, a.n(4) as i64);
        let seed = a.n(5);
        let w = words_of(&req);
        // the two tools that build on other builtins
        if tool == "synth" || tool == "layer" {
            let t = {
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let sel = if clip_t >= 0 { clip_t } else { sel_t };
                track_by_words(&en.project, if tool == "layer" { "" } else { &req }, sel)
            };
            let Some(t) = t else { return Ok(Val::Str("Select a track first (click its name).".into())) };
            if tool == "synth" {
                let r = self.dispatch("daw_arrx_synth", &A(&[Val::Num(t as f64), Val::Str(req)]))?;
                return Ok(r);
            }
            let c = if clip_t == t as i64 { clip_c } else { -1 };
            return Ok(Val::Str(match self.dispatch("daw_arrx_layer", &A(&[Val::Num(t as f64), Val::Num(c as f64), Val::Str(req.clone())])) {
                Ok(Val::List(v)) => {
                    let nt = if let Some(Val::Num(x)) = v.first() { *x } else { -1.0 };
                    let label = if let Some(Val::Str(s)) = v.get(2) { s.clone() } else { String::new() };
                    if label == "choir" && nt >= 0.0 {
                        let _ = self.dispatch("daw_song_redesign", &A(&[Val::Num(nt), Val::Str(String::new()), Val::Str("warm choir pad".into()), Val::Str(req.clone()), Val::Num(0.6), Val::Num(seed), Val::Str(String::new()), Val::Num(0.0)]));
                    }
                    let n = if let Some(Val::Num(x)) = v.get(1) { *x } else { 0.0 };
                    format!("Layered {label} on a new track ({n} notes), under the same melody.")
                }
                Ok(_) => "Nothing was layered.".to_string(),
                Err(m) => m,
            }));
        }
        let msg: Result<String, String> = (|| -> Result<String, String> {
            // the selected clip's span, for "this section"
            let sel = {
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                if clip_t >= 0 && clip_c >= 0 {
                    en.project.tracks.get(clip_t as usize).and_then(|t| t.clips.get(clip_c as usize)).map(|c| (c.start, c.end())).unwrap_or((0.0, 0.0))
                } else {
                    (0.0, 0.0)
                }
            };
            let section = |en: &crate::engine::Engine, r: &str, def: Option<&str>| find_section(&en.project, r, sel, def);
            let need = "Name a section that is tagged on the ruler (the second verse, the chorus, bars 9 to 16) or select its clip.";
            match tool.as_str() {
                "repeat_section" => {
                    let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    let sec = section(&en, &req, None).ok_or(need)?;
                    let times = if word_count(&w, &["twice"]) { 2 } else if let Some(n) = num_in(&w, &["times", "x", "more"]) { n as usize } else { 1 };
                    repeat_section(&mut en.project, sec.s, sec.e, times)?;
                    Ok(format!("Repeated the {} ({} bars) {} time(s); everything after it moved down, automation and tags with it.", sec.name, (sec.e - sec.s) / 4.0, times))
                }
                "remove_section" => {
                    let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    let sec = section(&en, &req, None).ok_or(need)?;
                    remove_range(&mut en.project, sec.s, sec.e);
                    Ok(format!("Removed the {} ({} bars) and closed the gap.", sec.name, (sec.e - sec.s) / 4.0))
                }
                "swap_sections" | "move_section" => {
                    let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    let lw = req.to_ascii_lowercase();
                    let seps = [" before ", " after ", " with ", " and ", " <-> ", " to "];
                    let (i, sep) = seps.iter().filter_map(|s| lw.find(s).map(|i| (i, *s))).min_by_key(|x| x.0).ok_or("Say which two sections: swap the verse and the chorus, move the chorus before the verse.")?;
                    let (left, right) = (&req[..i], &req[i + sep.len()..]);
                    let a_ = section(&en, left, None).ok_or(need)?;
                    let fam = left.to_lowercase().split_whitespace().rev().find_map(|x| tag_family(x.trim_matches(|c: char| !c.is_alphanumeric()))).map(|s| s.to_string());
                    let b_ = section(&en, right, fam.as_deref()).ok_or(need)?;
                    if tool == "swap_sections" || sep == " and " || sep == " with " {
                        swap_sections(&mut en.project, (a_.s, a_.e), (b_.s, b_.e))?;
                        Ok(format!("Swapped the {} and the {}.", a_.name, b_.name))
                    } else {
                        let to = if sep == " after " { b_.e } else { b_.s };
                        move_section(&mut en.project, a_.s, a_.e, to)?;
                        Ok(format!("Moved the {} {} the {}.", a_.name, if sep == " after " { "after" } else { "before" }, b_.name))
                    }
                }
                "resize_section" => {
                    let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    let sec = section(&en, &req, None).ok_or(need)?;
                    let len = sec.e - sec.s;
                    let n = num_in(&w, &["bars", "bar"]).ok_or("Say how many bars: extend the intro by 4 bars, shorten the verse to 8 bars.")?;
                    let shorter = word_count(&w, &["shorten", "shorter", "trim", "cut", "reduce"]);
                    let by = word_count(&w, &["by"]);
                    let target = if by { len + if shorter { -n * 4.0 } else { n * 4.0 } } else { n * 4.0 };
                    let end = resize_section(&mut en.project, sec.s, sec.e, target)?;
                    Ok(format!("The {} is now {} bars (ends at bar {}); the material loops to fill, or the tail is cut.", sec.name, (target / 4.0).round(), (end / 4.0).round() + 1.0))
                }
                "add_build" => {
                    let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    let bars = num_in(&w, &["bar", "bars"]).unwrap_or(8.0);
                    let tags = read_tags(&en.project);
                    let target = match section(&en, &req, None) {
                        Some(s) => s,
                        None => tags.iter().find(|t| t.s >= 4.0 && (family_matches("drop", &t.name) || family_matches("chorus", &t.name))).cloned().or_else(|| tags.get(1).cloned()).ok_or(need)?,
                    };
                    let (b0, b1) = insert_build(&mut en.project, target.s, bars)?;
                    Ok(format!("Inserted a {}-bar build before the {}: bars {}-{}, the material thinned (bass and kicks out for the first half), a snare roll, rising velocities and filters.", ((b1 - b0) / 4.0).round(), target.name, (b0 / 4.0) as i64 + 1, (b1 / 4.0) as i64))
                }
                "add_breakdown" => {
                    let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    let named = w.iter().filter_map(|x| tag_family(x)).any(|f| f != "breakdown" && f != "drop" && f != "build");
                    let sec = if named || bars_range(&w).is_some() || w.iter().any(|x| x == "this" || x == "selected") { section(&en, &req, None) } else { None };
                    let (s, e) = match sec {
                        Some(sec) => (sec.s, sec.e),
                        None => {
                            // a new breakdown of 8 bars before the first drop / chorus, copied from the music before it
                            let tags = read_tags(&en.project);
                            let t = tags.iter().find(|t| t.s >= 4.0 && (family_matches("drop", &t.name) || family_matches("chorus", &t.name))).cloned().ok_or(need)?;
                            let (src_s, dl) = ((t.s - 32.0).max(0.0), 32.0f64.min(t.s));
                            insert_copy(&mut en.project, src_s, t.s, t.s, dl, false);
                            (t.s, t.s + dl)
                        }
                    };
                    let outv = breakdown(&mut en.project, s, e)?;
                    Ok(format!("Breakdown at bars {}-{}: {}.", (s / 4.0) as i64 + 1, (e / 4.0) as i64, if outv.is_empty() { "nothing to take out".to_string() } else { format!("out: {}", outv.join(", ")) }))
                }
                "section_tempo" => {
                    let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    let sec = section(&en, &req, None);
                    let from_bar = w.iter().position(|x| x == "bar" || x == "bars").and_then(|i| w.get(i + 1)).and_then(|x| x.parse::<f64>().ok());
                    let from_beat = w.iter().position(|x| x == "beat").and_then(|i| w.get(i + 1)).and_then(|x| x.parse::<f64>().ok());
                    let (s, e) = if w.contains(&"from".to_string()) && (from_bar.is_some() || from_beat.is_some()) {
                        (from_bar.map(|b| (b - 1.0).max(0.0) * 4.0).or(from_beat).unwrap_or(0.0), -1.0)
                    } else if let Some(sec) = &sec {
                        (sec.s, sec.e)
                    } else {
                        return Err("Say where: set the bridge to 90 bpm, from bar 17 go to 150 bpm.".into());
                    };
                    let here = en.project.bpm_at(s);
                    let bpm = if let Some(v) = num_in(&w, &["bpm"]).filter(|v| (20.0..=400.0).contains(v)) {
                        v
                    } else if let Some(pc) = num_in(&w, &["%", "percent"]).or_else(|| w.iter().find(|x| x.ends_with('%')).and_then(|x| x.trim_end_matches('%').parse().ok())) {
                        let slower = word_count(&w, &["slower", "slow", "down", "decrease"]);
                        here * (1.0 + if slower { -pc } else { pc } / 100.0)
                    } else if word_count(&w, &["half"]) {
                        here / 2.0
                    } else if word_count(&w, &["double", "twice"]) {
                        here * 2.0
                    } else {
                        return Err("Say the tempo: 90 bpm, or 10% slower.".into());
                    };
                    set_tempo(&mut en.project, s, e, bpm);
                    Ok(format!("Tempo {} bpm from bar {}{}.", bpm.round(), (s / 4.0) as i64 + 1, if e > s { format!(" to bar {}, then back to {} bpm", (e / 4.0) as i64, here.round()) } else { String::new() }))
                }
                "drum_feel" => {
                    let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    let (s, e) = match section(&en, &req, None) {
                        Some(sec) => (sec.s, sec.e),
                        None => (0.0, song_end(&en.project)),
                    };
                    let double = w.iter().any(|x| x == "double" || x == "doubletime" || x == "double-time") ;
                    let n = drum_feel(&mut en.project, s, e, if double { 1 } else { 0 });
                    Ok(format!("{} drums at bars {}-{}: {} notes changed.", if double { "Double-time" } else { "Half-time" }, (s / 4.0) as i64 + 1, (e / 4.0).ceil() as i64, n))
                }
                "part" => {
                    let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    let ti = track_by_words(&en.project, &req, sel_t).ok_or("Which part? Name it (the bass, the drums, the lead) or select its track.")?;
                    let sec = section(&en, &req, None).ok_or(need)?;
                    let mode = if word_count(&w, &["solo", "only", "alone"]) { 2 } else if word_count(&w, &["back", "unmute", "restore", "bring", "return"]) && !word_count(&w, &["out"]) { 1 } else { 0 };
                    part_in_section(&mut en.project, ti, sec.s, sec.e, mode)?;
                    let name = en.project.tracks[ti].name.clone();
                    Ok(match mode {
                        0 => format!("Took {name} out of the {} (a volume lane; delete it to undo, or Ctrl+Z).", sec.name),
                        1 => format!("{name} is back in the {}.", sec.name),
                        _ => format!("{name} stands alone in the {}: the other parts are ducked.", sec.name),
                    })
                }
                "shape" => {
                    let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    let ti = track_by_words(&en.project, &req, sel_t).ok_or("Select a track first (click its name).")?;
                    let (s, e) = match section(&en, &req, None) {
                        Some(sec) => (sec.s, sec.e),
                        None => (0.0, song_end(&en.project)),
                    };
                    let done = auto_words(&mut en.project.tracks[ti], &req, s, e);
                    let name = en.project.tracks[ti].name.clone();
                    if done.is_empty() {
                        Err(format!("Nothing to draw on {name} from that (try: filter sweep up over the build, volume swell, pump, fade out, open the filter in the drop)."))
                    } else {
                        Ok(format!("{name}, bars {}-{}: {}.", (s / 4.0) as i64 + 1, (e / 4.0).ceil() as i64, done.join(", ")))
                    }
                }
                "gesture" => {
                    // per-note pitch gestures, only when the request names one (the pitch guard: redesign::gesture_request)
                    let Some(kind) = crate::redesign::gesture_request(&req) else {
                        return Err("Name the pitch gesture: scoop, slide / glide, fall, dive or pitch riser.".into());
                    };
                    let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    let sel = if clip_t >= 0 { clip_t } else { sel_t };
                    let ti = track_by_words(&en.project, &req, sel).ok_or("Select a track first (click its name).")?;
                    let (s, e) = match section(&en, &req, None) {
                        Some(sec) => (sec.s, sec.e),
                        None => (0.0, f64::MAX),
                    };
                    let depth = num_in(&w, &["semitones", "semitone", "st"]).unwrap_or(0.0) as f32;
                    let mut n = 0;
                    for c in en.project.tracks[ti].clips.iter_mut() {
                        if c.end() <= s || c.start >= e {
                            continue;
                        }
                        unloop(c);
                        let (cs, cl) = (c.start, c.len);
                        if let ClipData::Midi(notes) = &mut c.data {
                            n += crate::gesture::write_kind(notes, kind, (s - cs).max(0.0), (e - cs).min(cl), depth);
                        }
                    }
                    let name = en.project.tracks[ti].name.clone();
                    let what = match kind {
                        "scoop" => "scoops into the chords",
                        "slide" => "slides into the next notes",
                        "fall" => "falls at the phrase ends",
                        "dive" => "dives",
                        _ => "pitch risers",
                    };
                    if n == 0 {
                        Err(format!("No notes of {name} fit {what} there (scoops need chord entries or long notes, slides a legato line)."))
                    } else {
                        Ok(format!("{name}: {what} on {n} notes (piano roll: Pitch lane to edit, right-click a note to clear)."))
                    }
                }
                other => Err(format!("I do not know how to {other} yet.")),
            }
        })();
        match msg {
            Ok(m) => {
                self.touch();
                Ok(Val::Str(m))
            }
            Err(m) => Ok(Val::Str(m)),
        }
    }
}
