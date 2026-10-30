//! Builtins for fixing a whole project: `daw_fix_*` runs the score fixer over every melodic clip until nothing is left to fix,
//! `daw_arr_*` lines the arrangement up (clip onsets, durations, overlaps, gaps) and suggests section tags.
use super::theory_api::{analyze, expand};
use super::{Daw, A, Val};
use crate::arrange::{self, BarFeat, ClipInfo};
use crate::structure::{self, Page, Span};
use crate::model::*;
use crate::theory::{self, N};

fn is_drums(t: &Track) -> bool {
    t.inst_kind == "drums"
}

/// (track, clip, name) of every melodic MIDI clip
fn melodic_clips(en: &crate::engine::Engine) -> Vec<(usize, usize, String)> {
    let mut v = vec![];
    for (ti, t) in en.project.tracks.iter().enumerate() {
        if is_drums(t) || matches!(t.kind, TrackKind::Audio) {
            continue;
        }
        for (ci, c) in t.clips.iter().enumerate() {
            if matches!(c.data, ClipData::Midi(_)) {
                v.push((ti, ci, format!("{} / {}", t.name, c.name)));
            }
        }
    }
    v
}

fn write_back(en: &mut crate::engine::Engine, t: usize, c: usize, fixed: &[N], changed: &[bool]) {
    if let Some(Clip { data: ClipData::Midi(n), .. }) = en.project.tracks.get_mut(t).and_then(|x| x.clips.get_mut(c)) {
        for (i, f) in fixed.iter().enumerate() {
            if changed.get(i) == Some(&true) {
                if let Some(x) = n.get_mut(i) {
                    x.start = f.start;
                    x.len = f.len;
                    x.key = f.key;
                }
            }
        }
        let mut idx = 0usize;
        n.retain(|_| {
            let keep = fixed.get(idx).map_or(true, |f| f.len > 0.0);
            idx += 1;
            keep
        });
    }
}

fn clip_infos(en: &crate::engine::Engine) -> Vec<ClipInfo> {
    let mut out = vec![];
    for (ti, t) in en.project.tracks.iter().enumerate() {
        for (ci, c) in t.clips.iter().enumerate() {
            let (content_end, audio) = match &c.data {
                ClipData::Midi(n) => (n.iter().map(|x| x.start + x.len).fold(0.0, f64::max), false),
                _ => (0.0, true),
            };
            // audio clips keep their length (it is the length of the recording); loops keep theirs too
            out.push(ClipInfo { track: ti, clip: ci, start: c.start, len: c.len, content_end, looped: audio || c.loop_len >= 0.25 });
        }
    }
    out
}

pub const ST_PAGES: u32 = 1;
pub const ST_RIFFS: u32 = 2;

/// what the structure conformer would do, as data (applied by `struct_apply`)
enum StAction {
    Pages { t: usize, c: usize, name: String, page: Page, bars: usize },
    Riff { t: usize, name: String, spans: Vec<(f64, f64, Vec<usize>)>, statements: Vec<structure::Statement> },
    Skip { t: usize, c: usize, name: String, why: String },
}

pub(super) fn sections_of(en: &crate::engine::Engine) -> (Vec<arrange::Section>, usize) {
    let end = en.project.tracks.iter().flat_map(|t| t.clips.iter()).map(|c| c.end()).fold(0.0, f64::max);
    let nbars = ((end / 4.0).ceil() as usize).clamp(0, 2048);
    let mut feats = vec![BarFeat::default(); nbars];
    for t in &en.project.tracks {
        if matches!(t.kind, TrackKind::Audio) {
            continue;
        }
        let drums = is_drums(t);
        let mut active = vec![false; nbars];
        for c in &t.clips {
            for n in expand(c) {
                let b = (n.start / 4.0).floor() as isize;
                if b < 0 || b as usize >= nbars {
                    continue;
                }
                let b = b as usize;
                if drums {
                    feats[b].drum_notes += 1;
                    if n.key == 35 || n.key == 36 {
                        feats[b].kicks += 1;
                    }
                    if (n.key == 49 || n.key == 57) && n.start - b as f64 * 4.0 < 1.0 {
                        feats[b].crash = true;
                    }
                } else {
                    feats[b].notes += 1;
                    active[b] = true;
                }
            }
        }
        for (b, on) in active.iter().enumerate() {
            if *on {
                feats[b].tracks += 1;
            }
        }
    }
    (arrange::detect_sections(&feats).0, nbars)
}

fn struct_plan(en: &crate::engine::Engine, mask: u32) -> Vec<StAction> {
    let mut out = vec![];
    let (secs, nbars) = sections_of(en);
    let song_beats = nbars as f64 * 4.0;
    let highs: Vec<(f64, f64)> = secs.iter().filter(|s| s.tag == "chorus" || s.tag == "drop").map(|s| (s.start_bar as f64 * 4.0, s.end_bar as f64 * 4.0)).collect();
    for (ti, t) in en.project.tracks.iter().enumerate() {
        if is_drums(t) || matches!(t.kind, TrackKind::Audio) {
            continue;
        }
        let all: Vec<N> = t.clips.iter().flat_map(|c| expand(c)).collect();
        if all.is_empty() {
            continue;
        }
        let (role, _) = theory::prominence(&all);
        let riff = role == "lead";
        if riff && mask & ST_RIFFS != 0 {
            // clips that touch or nearly touch are one statement
            let mut idx: Vec<usize> = (0..t.clips.len()).filter(|i| matches!(t.clips[*i].data, ClipData::Midi(_))).collect();
            idx.sort_by(|a, b| t.clips[*a].start.partial_cmp(&t.clips[*b].start).unwrap());
            let mut spans: Vec<(f64, f64, Vec<usize>)> = vec![];
            for i in idx {
                let c = &t.clips[i];
                match spans.last_mut() {
                    Some(l) if c.start <= l.1 + 4.0 => {
                        l.1 = l.1.max(c.end());
                        l.2.push(i);
                    }
                    _ => spans.push((c.start, c.end(), vec![i])),
                }
            }
            let sp: Vec<Span> = spans
                .iter()
                .map(|(s, e, ids)| {
                    let energy: f64 = ids.iter().flat_map(|i| expand(&t.clips[*i])).filter(|n| n.start >= *s && n.start < *e).map(|n| n.vel as f64 * n.len).sum();
                    Span { start: *s, end: *e, energy }
                })
                .collect();
            let statements = structure::plan_statements(&sp, song_beats, &highs);
            if !statements.is_empty() {
                out.push(StAction::Riff { t: ti, name: t.name.clone(), spans, statements });
            }
        } else if !riff && mask & ST_PAGES != 0 {
            for (ci, c) in t.clips.iter().enumerate() {
                let ClipData::Midi(ns) = &c.data else { continue };
                let bars = (c.len / 4.0).round() as usize;
                let name = format!("{} / {}", t.name, c.name);
                if c.loop_len >= 0.25 {
                    out.push(StAction::Skip { t: ti, c: ci, name, why: "already a looping clip".into() });
                    continue;
                }
                if bars < 4 {
                    continue;
                }
                let notes: Vec<N> = ns.iter().map(|n| N { start: n.start, len: n.len, key: n.key, vel: n.vel }).collect();
                match structure::best_page(&notes, bars) {
                    Some(p) if p.score >= 0.45 && bars >= p.bars * 2 => out.push(StAction::Pages { t: ti, c: ci, name, page: p, bars }),
                    Some(p) => out.push(StAction::Skip { t: ti, c: ci, name, why: format!("no clear repeating page ({}% alike)", (p.score * 100.0).round()) }),
                    None => {}
                }
            }
        }
    }
    out
}

fn struct_apply(en: &mut crate::engine::Engine, plan: &[StAction]) -> usize {
    let mut done = 0;
    for act in plan {
        match act {
            StAction::Pages { t, c, page, bars, .. } => {
                let Some(clip) = en.project.tracks.get_mut(*t).and_then(|x| x.clips.get_mut(*c)) else { continue };
                let ClipData::Midi(ns) = &mut clip.data else { continue };
                let notes: Vec<N> = ns.iter().map(|n| N { start: n.start, len: n.len, key: n.key, vel: n.vel }).collect();
                let rebuilt = structure::loopify(&notes, *bars as f64 * 4.0, page, (*t * 31 + *c) as u32);
                *ns = rebuilt.iter().map(|n| Note::new(n.start, n.len, n.key, n.vel)).collect();
                done += 1;
            }
            StAction::Riff { t, spans, statements, .. } => {
                let Some(track) = en.project.tracks.get_mut(*t) else { continue };
                // gather the notes of each existing span (relative to its start) before touching the clips
                let span_notes: Vec<Vec<N>> = spans
                    .iter()
                    .map(|(s, e, ids)| ids.iter().flat_map(|i| expand(&track.clips[*i])).filter(|n| n.start >= *s - 1e-6 && n.start < *e).map(|n| N { start: n.start - *s, ..n }).collect())
                    .collect();
                let base_name = track.clips.first().map(|c| c.name.clone()).unwrap_or_else(|| "Riff".into());
                let mut remove: Vec<usize> = vec![];
                let mut add: Vec<Clip> = vec![];
                let mut built: Vec<Option<Vec<N>>> = vec![None; spans.len()];
                for st in statements {
                    let len = st.bars as f64 * 4.0;
                    if let Some(si) = st.from {
                        let (s, e, ids) = &spans[si];
                        let notes = &span_notes[si];
                        let orig_bars = ((e - s) / 4.0).round() as usize;
                        if st.bars == orig_bars {
                            built[si] = Some(notes.clone());
                            continue; // already the right length: the riff is left alone
                        }
                        let out: Vec<N> = if st.bars < orig_bars {
                            notes.iter().filter(|n| n.start < len - 1e-6).map(|n| N { len: n.len.min(len - n.start), ..*n }).collect()
                        } else {
                            let page = structure::best_page(notes, orig_bars).filter(|p| p.score >= 0.3).unwrap_or(Page { bars: orig_bars.min(8).max(1), start_bar: 0, score: 0.0 });
                            structure::loopify(notes, len, &page, *t as u32)
                        };
                        remove.extend(ids.iter().copied());
                        let mut c = Clip::midi(*s, len, &base_name);
                        c.data = ClipData::Midi(out.iter().map(|n| Note::new(n.start, n.len, n.key, n.vel)).collect());
                        built[si] = Some(out);
                        add.push(c);
                        done += 1;
                    } else if let Some(src) = st.copy_of {
                        let notes = built[src].clone().unwrap_or_else(|| span_notes[src].clone());
                        // the second statement is the first again, with its last bar turned
                        let mut out: Vec<N> = notes.iter().filter(|n| n.start < len - 1e-6).copied().collect();
                        let mut k = 0usize;
                        out.retain(|n| {
                            k += 1;
                            !(n.start >= len - 4.0 && (n.start % 1.0).abs() > 0.01 && k % 3 == 0)
                        });
                        if let Some(l) = out.last_mut() {
                            if l.key <= 115 {
                                l.key += 12;
                            }
                        }
                        let mut c = Clip::midi(st.start, len, &base_name);
                        c.data = ClipData::Midi(out.iter().map(|n| Note::new(n.start, n.len, n.key, n.vel)).collect());
                        add.push(c);
                        done += 1;
                    }
                }
                remove.sort_unstable();
                remove.dedup();
                for i in remove.into_iter().rev() {
                    track.clips.remove(i);
                }
                track.clips.extend(add);
            }
            StAction::Skip { .. } => {}
        }
    }
    done
}

/// "String Ensemble 1_Track 3" -> "String Ensemble 1"
fn clean_track_name(n: &str) -> String {
    let mut t = n.trim().to_string();
    if let Some(i) = t.find("_Track ") {
        t.truncate(i);
    }
    if let Some(i) = t.rfind(" Track ") {
        if t[i + 7..].chars().all(|c| c.is_ascii_digit()) {
            t.truncate(i);
        }
    }
    dedupe_name(t.replace('_', " ").trim())
}

/// "Trumpet (Trumpet)" -> "Trumpet"; "Violin (Solo Violin)" stays
fn dedupe_name(n: &str) -> String {
    if let (Some(i), true) = (n.rfind(" ("), n.ends_with(')')) {
        let (a, b) = (n[..i].trim(), n[i + 2..n.len() - 1].trim());
        if a.eq_ignore_ascii_case(b) {
            return a.to_string();
        }
    }
    n.to_string()
}

fn title_case(w: &str) -> String {
    let mut c = w.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// The section a beat falls in, written "Verse 2" (the number only when the tag comes back later).
fn section_label(tags: &[(f64, f64, String)], beat: f64) -> Option<String> {
    let i = tags.iter().position(|t| beat >= t.0 - 1e-6 && beat < t.1 - 1e-6)?;
    let tag = &tags[i].2;
    let same = tags.iter().filter(|t| &t.2 == tag).count();
    if same <= 1 {
        return Some(title_case(tag));
    }
    let n = tags[..=i].iter().filter(|t| &t.2 == tag).count();
    Some(format!("{} {}", title_case(tag), n))
}

/// Name arrangement clips ("Violin - Verse 2"), the scenes of the performance view ("Chorus 1") and its session clips from the section tags.
/// mask 1 = arrangement clips, 2 = scenes, 4 = session clips, 8 = tidy the track names. Returns how many were renamed.
fn auto_name(p: &mut crate::model::Project, mask: u32) -> usize {
    let text = p.meta.iter().find(|(k, _)| k == "sections").map(|(_, v)| v.clone()).unwrap_or_default();
    let mut tags: Vec<(f64, f64, String)> = vec![];
    for part in text.split(';') {
        let f: Vec<&str> = part.split(':').collect();
        if f.len() == 3 {
            if let (Ok(a), Ok(b)) = (f[0].parse::<f64>(), f[1].parse::<f64>()) {
                tags.push((a, b, f[2].to_string()));
            }
        }
    }
    let mut done = 0;
    if mask & 8 != 0 {
        for t in p.tracks.iter_mut() {
            let nm = clean_track_name(&t.name);
            if nm != t.name && !nm.is_empty() {
                t.name = nm;
                done += 1;
            }
        }
    }
    if mask & 1 != 0 {
        for t in p.tracks.iter_mut() {
            let base = clean_track_name(&t.name);
            let mut seen: std::collections::HashMap<String, usize> = Default::default();
            let total = t.clips.len();
            for (ci, c) in t.clips.iter_mut().enumerate() {
                if !matches!(c.data, ClipData::Midi(_)) {
                    continue;
                }
                // a clip that runs on into the next section belongs to no single one
                let inside = tags.iter().find(|t| c.start + 0.01 >= t.0 && c.start + 0.01 < t.1).map(|t| c.end() <= t.1 + 4.0).unwrap_or(false);
                let label = if inside { section_label(&tags, c.start + 0.01) } else { None };
                let name = match label {
                    Some(l) => {
                        let k = seen.entry(l.clone()).or_insert(0);
                        *k += 1;
                        if *k > 1 {
                            format!("{base} - {l} ({k})")
                        } else {
                            format!("{base} - {l}")
                        }
                    }
                    None if total > 1 => format!("{base} {}", ci + 1),
                    None => base.clone(),
                };
                // a sliced pattern keeps its letter, so the same pattern coming back later is recognisable
                c.name = match c.name.strip_prefix("Pattern ") {
                    Some(letter) if letter.len() <= 3 => format!("{name} [{letter}]"),
                    _ => name,
                };
                done += 1;
            }
        }
    }
    // scenes: the contiguous slices auto_scenes made; a scene starts where the longest clip of the one before it ended
    let nscenes = p.scenes.len();
    let mut scene_names: Vec<String> = vec![];
    let mut start = 0.0;
    for sc in 0..nscenes {
        let mut len = 0.0f64;
        for ti in 0..p.tracks.len() {
            let ix = p.slot(sc, ti);
            if ix >= 0 {
                if let Some(c) = p.tracks[ti].session.get(ix as usize) {
                    len = len.max(c.len);
                }
            }
        }
        scene_names.push(section_label(&tags, start + 0.01).unwrap_or_else(|| format!("Scene {}", sc + 1)));
        start += len.max(0.0);
    }
    // two scenes that would share a name are told apart
    let mut counts: std::collections::HashMap<String, usize> = Default::default();
    let totals: std::collections::HashMap<String, usize> = scene_names.iter().fold(Default::default(), |mut m, n| {
        *m.entry(n.clone()).or_insert(0) += 1;
        m
    });
    for n in scene_names.iter_mut() {
        if totals[n.as_str()] > 1 {
            let k = counts.entry(n.clone()).or_insert(0);
            *k += 1;
            *n = format!("{n} ({k})");
        }
    }
    if mask & 2 != 0 {
        for (i, n) in scene_names.iter().enumerate() {
            if let Some(s) = p.scenes.get_mut(i) {
                *s = n.clone();
                done += 1;
            }
        }
    }
    if mask & 4 != 0 {
        for sc in 0..nscenes {
            for ti in 0..p.tracks.len() {
                let ix = p.slot(sc, ti);
                if ix < 0 {
                    continue;
                }
                let base = clean_track_name(&p.tracks[ti].name);
                if let Some(c) = p.tracks[ti].session.get_mut(ix as usize) {
                    c.name = format!("{base} - {}", scene_names[sc]);
                    done += 1;
                }
            }
        }
    }
    done
}

fn group_letters(g: usize) -> String {
    let mut n = g;
    let mut out = String::new();
    loop {
        out.insert(0, (b'A' + (n % 26) as u8) as char);
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    out
}

/// the long solid MIDI clips (at least `min_bars` bars) and how each would be cut: (track, clip, segments)
fn slice_plans(en: &crate::engine::Engine, min_bars: usize) -> Vec<(usize, usize, Vec<structure::SliceSeg>)> {
    let mut out = vec![];
    for (ti, t) in en.project.tracks.iter().enumerate() {
        if matches!(t.kind, TrackKind::Audio) {
            continue;
        }
        for (ci, c) in t.clips.iter().enumerate() {
            let ClipData::Midi(ns) = &c.data else { continue };
            if c.loop_len >= 0.25 {
                continue;
            }
            let bars = (c.len / 4.0).ceil() as usize;
            if bars < min_bars || ns.len() < 8 {
                continue;
            }
            let notes: Vec<N> = ns.iter().map(|n| N { start: n.start, len: n.len, key: n.key, vel: n.vel }).collect();
            let segs = structure::slice_plan(&notes, bars);
            // worth doing only when it makes more than the one clip it started as, or folds a repeat
            if segs.len() > 1 || segs.iter().any(|s| s.repeats > 1) {
                out.push((ti, ci, segs));
            }
        }
    }
    out
}

/// Replace each planned clip with its slices. Returns how many clips were made.
fn apply_slices(en: &mut crate::engine::Engine, plans: &[(usize, usize, Vec<structure::SliceSeg>)]) -> usize {
    let mut made = 0;
    // from the last clip of a track to the first, so the indices of the clips still to do stay valid
    let mut order: Vec<&(usize, usize, Vec<structure::SliceSeg>)> = plans.iter().collect();
    order.sort_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)));
    for (ti, ci, segs) in order {
        let Some(track) = en.project.tracks.get_mut(*ti) else { continue };
        if *ci >= track.clips.len() {
            continue;
        }
        let old = track.clips.remove(*ci);
        let ClipData::Midi(ns) = &old.data else {
            track.clips.insert(*ci, old);
            continue;
        };
        let mut fresh: Vec<Clip> = vec![];
        for sg in segs {
            let (b0, b1) = (sg.start_bar as f64 * 4.0, (sg.start_bar + sg.page_bars) as f64 * 4.0);
            let mut page: Vec<Note> = ns.iter().filter(|n| n.start >= b0 - 1e-6 && n.start < b1 - 1e-6).cloned().collect();
            for n in page.iter_mut() {
                n.start -= b0;
            }
            let page_len = sg.page_bars as f64 * 4.0;
            // a sustained note that runs past the end of the page keeps the clip long enough to sound
            let end = page.iter().map(|n| n.start + n.len).fold(0.0, f64::max);
            let len = if sg.repeats > 1 { page_len * sg.repeats as f64 } else { page_len.max((end / 4.0).ceil() * 4.0) };
            let mut c = Clip::midi(old.start + b0, len, &format!("{} {}", old.name.split(" - ").next().unwrap_or("Pattern"), group_letters(sg.group)));
            c.name = format!("Pattern {}", group_letters(sg.group));
            c.data = ClipData::Midi(page);
            if sg.repeats > 1 {
                c.loop_len = page_len;
            }
            fresh.push(c);
        }
        made += fresh.len();
        for (k, c) in fresh.into_iter().enumerate() {
            track.clips.insert(*ci + k, c);
        }
    }
    made
}

/// Glue the MIDI clips of one track into phrases (the reverse of slicing): clips that touch (or are at most `gap` beats apart) join into one
/// clip whose notes are the concatenation, up to `max_len` beats long (0 = no limit). With `fold`, a run of identical touching clips becomes
/// one looped clip instead (a bar repeated eight times is one clip of eight bars that loops its bar). Audio clips and clips that already loop
/// stay as they are. Returns the track's new clips when that is fewer than it had.
fn glued_clips(t: &Track, gap: f64, max_len: f64, fold: bool) -> Option<Vec<Clip>> {
    struct Item {
        c: Clip,
        notes: Vec<Note>,
        atomic: bool,
        folded: bool,
    }
    let mut midi: Vec<&Clip> = t.clips.iter().filter(|c| matches!(c.data, ClipData::Midi(_))).collect();
    if midi.len() < 2 {
        return None;
    }
    midi.sort_by(|a, b| a.start.total_cmp(&b.start));
    let same = |a: &[Note], b: &[Note]| -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x.start - y.start).abs() < 0.02 && (x.len - y.len).abs() < 0.02 && x.key == y.key && (x.vel - y.vel).abs() < 0.02)
    };
    let sorted_notes = |c: &Clip| -> Vec<Note> {
        let ClipData::Midi(n) = &c.data else { return vec![] };
        let mut v = n.clone();
        v.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.key.cmp(&b.key)));
        v
    };
    // 1. runs of identical clips fold into looped clips
    let mut items: Vec<Item> = vec![];
    for c in midi {
        let notes = sorted_notes(c);
        if fold && c.loop_len < 0.25 && !notes.is_empty() {
            if let Some(last) = items.last_mut() {
                let one = if last.c.loop_len >= 0.25 { last.c.loop_len } else { last.c.len };
                let touching = (c.start - last.c.end()).abs() < 0.01;
                if touching && (c.len - one).abs() < 0.01 && (!last.atomic || last.folded) && same(&last.notes, &notes) {
                    last.c.len = c.end() - last.c.start;
                    last.c.loop_len = one;
                    last.atomic = true;
                    last.folded = true;
                    continue;
                }
            }
        }
        items.push(Item { c: c.clone(), notes, atomic: c.loop_len >= 0.25, folded: false });
    }
    // 2. the rest join into phrases
    let mut out: Vec<Clip> = vec![];
    let mut k = 0;
    let mut phrase = 0;
    while k < items.len() {
        if items[k].atomic {
            out.push(items[k].c.clone());
            k += 1;
            continue;
        }
        let start = items[k].c.start;
        let mut end = items[k].c.end();
        let mut notes: Vec<Note> = items[k].notes.clone();
        let mut lyr = items[k].c.lyric_text.clone();
        let mut lang = items[k].c.lyric_lang.clone();
        let mut count = 1;
        let first_name = items[k].c.name.clone();
        let mut j = k + 1;
        while j < items.len() && !items[j].atomic && items[j].c.start - end <= gap + 1e-6 && (max_len <= 0.0 || items[j].c.end() - start <= max_len + 1e-6) {
            for n in &items[j].notes {
                let mut n = n.clone();
                n.start += items[j].c.start - start;
                notes.push(n);
            }
            if !items[j].c.lyric_text.is_empty() {
                if !lyr.is_empty() {
                    lyr.push('\n');
                }
                lyr.push_str(&items[j].c.lyric_text);
                if lang.is_empty() {
                    lang = items[j].c.lyric_lang.clone();
                }
            }
            end = end.max(items[j].c.end());
            count += 1;
            j += 1;
        }
        if count == 1 {
            out.push(items[k].c.clone());
        } else {
            phrase += 1;
            let mut c = Clip::midi(start, end - start, &if first_name.is_empty() { format!("Phrase {phrase}") } else { format!("{first_name} +{}", count - 1) });
            c.lyric_text = lyr;
            c.lyric_lang = lang;
            c.data = ClipData::Midi(notes);
            out.push(c);
        }
        k = j;
    }
    let before = t.clips.iter().filter(|c| matches!(c.data, ClipData::Midi(_))).count();
    if out.len() >= before {
        return None;
    }
    // audio clips stay where they are
    let mut all: Vec<Clip> = t.clips.iter().filter(|c| !matches!(c.data, ClipData::Midi(_))).cloned().collect();
    all.extend(out);
    all.sort_by(|a, b| a.start.total_cmp(&b.start));
    Some(all)
}

impl Daw {
    pub(super) fn fix_dispatch(&mut self, name: &str, a: &A) -> Result<Val, String> {
        let eng = self.out.engine.clone();
        macro_rules! ok {
            ($v:expr) => {
                return Ok(Val::from($v))
            };
        }
        match name {
            "daw_fix_scan" => {
                // (mask) -> per melodic clip, from the first pass only: [track, clip, name, total, out of key, clashes, timing, doubles, passing]
                let mask = a.u(0) as u32;
                let order = if a.0.len() > 1 { a.n(1).clamp(0.0, 1.0) } else { 0.75 };
                let clips = { melodic_clips(&eng.lock().unwrap_or_else(|p| p.into_inner())) };
                let mut v = vec![];
                for (t, c, nm) in clips {
                    let rep = {
                        let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                        analyze(&en, t, c).map(|an| theory::autocorrect_ctx(&an.notes, 0.0, &an.keys, &an.chords, mask, &theory::FixCtx { order, support: &an.support, repeated: &an.repeated, clash: &an.clash, others: &an.others_sorted }).2)
                    };
                    if let Some(r) = rep {
                        v.extend([Val::Num(t as f64), Val::Num(c as f64), Val::Str(nm), Val::Num(r.total() as f64), Val::Num(r.key as f64), Val::Num(r.tension as f64), Val::Num(r.timing as f64), Val::Num(r.dupes as f64), Val::Num(r.passing as f64)]);
                    }
                }
                ok!(Val::List(v))
            }
            "daw_fix_run" => {
                // (mask, max passes) -> [clean (1 / 0), passes run, total changes, changes in pass 1, pass 2, ...]; daw_fix_report has the per-clip sums
                let mask = a.u(0) as u32;
                let max_passes = a.u(1).clamp(1, 12);
                let order = if a.0.len() > 2 { a.n(2).clamp(0.0, 1.0) } else { 0.75 };
                let clips = { melodic_clips(&eng.lock().unwrap_or_else(|p| p.into_inner())) };
                let mut sums: Vec<[usize; 6]> = vec![[0; 6]; clips.len()];
                let mut per_pass: Vec<usize> = vec![];
                let mut clean = false;
                // The key and the chords of every clip are read ONCE, from the score as it was before any fixing. Re-reading them after
                // each pass let the fixer chase its own changes: a few notes moved, the next pass saw a different key (an invented
                // E minor / D major) and "fixed" more notes toward it, and a motif drifted a semitone away from the music.
                let frozen: Vec<Option<(Vec<theory::KeySection>, Vec<theory::Chord>)>> = {
                    let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    clips.iter().map(|(t, c, _)| analyze(&en, *t, *c).map(|an| (an.keys, an.chords))).collect()
                };
                for pass in 0..max_passes {
                    let mut total = 0usize;
                    for (k, (t, c, _)) in clips.iter().enumerate() {
                        let res = {
                            let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                            analyze(&en, *t, *c).map(|an| {
                                let (keys, chords) = match &frozen[k] {
                                    Some((ks, cs)) => (ks.clone(), cs.clone()),
                                    None => (an.keys.clone(), an.chords.clone()),
                                };
                                theory::autocorrect_ctx(&an.notes, 0.0, &keys, &chords, mask, &theory::FixCtx { order, support: &an.support, repeated: &an.repeated, clash: &an.clash, others: &an.others_sorted })
                            })
                        };
                        if let Some((fixed, changed, rep)) = res {
                            if rep.total() > 0 {
                                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                                write_back(&mut en, *t, *c, &fixed, &changed);
                                drop(en);
                                self.touch();
                                total += rep.total();
                                let s = &mut sums[k];
                                s[0] += rep.total();
                                s[1] += rep.key;
                                s[2] += rep.tension;
                                s[3] += rep.timing;
                                s[4] += rep.dupes;
                                s[5] += rep.passing;
                            }
                        }
                    }
                    per_pass.push(total);
                    if total == 0 {
                        clean = true;
                        break;
                    }
                    // two passes in a row that did not reduce the work: stop instead of going round forever
                    if pass >= 2 && total >= per_pass[pass - 1] && per_pass[pass - 1] >= per_pass[pass - 2] {
                        break;
                    }
                }
                self.fix_last = clips
                    .iter()
                    .zip(sums.iter())
                    .filter(|(_, s)| s[0] > 0)
                    .flat_map(|((t, c, nm), s)| [Val::Num(*t as f64), Val::Num(*c as f64), Val::Str(nm.clone()), Val::Num(s[0] as f64), Val::Num(s[1] as f64), Val::Num(s[2] as f64), Val::Num(s[3] as f64), Val::Num(s[4] as f64), Val::Num(s[5] as f64)])
                    .collect();
                let mut v = vec![Val::Num(clean as i32 as f64), Val::Num(per_pass.len() as f64), Val::Num(per_pass.iter().sum::<usize>() as f64)];
                v.extend(per_pass.iter().map(|p| Val::Num(*p as f64)));
                ok!(Val::List(v))
            }
            "daw_fix_report" => ok!(Val::List(self.fix_last.clone())),
            // ── arrangement ──
            "daw_arr_scan" | "daw_arr_fix" => {
                // (mask) -> scan: [track, clip, name, what, old start, old len, new start, new len]*; fix: how many clips changed
                let mask = a.u(0) as u32;
                let apply = name == "daw_arr_fix";
                let plan = { arrange::plan_clips(&clip_infos(&eng.lock().unwrap_or_else(|p| p.into_inner())), mask) };
                if apply {
                    let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    for f in &plan {
                        if let Some(c) = en.project.tracks.get_mut(f.track).and_then(|t| t.clips.get_mut(f.clip)) {
                            c.start = f.new_start;
                            c.len = f.new_len;
                        }
                    }
                    drop(en);
                    if !plan.is_empty() {
                        self.touch();
                    }
                    ok!(plan.len())
                }
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let mut v = vec![];
                for f in &plan {
                    if let Some((t, c)) = en.project.tracks.get(f.track).and_then(|t| t.clips.get(f.clip).map(|c| (t, c))) {
                        v.extend([Val::Num(f.track as f64), Val::Num(f.clip as f64), Val::Str(format!("{} / {}", t.name, c.name)), Val::Num(f.what as f64), Val::Num(c.start), Val::Num(c.len), Val::Num(f.new_start), Val::Num(f.new_len)]);
                    }
                }
                ok!(Val::List(v))
            }
            "daw_arr_sections" => {
                // () -> suggested sections as [start beat, end beat, tag, energy 0..1]*, fills included as 1-bar "fill" entries
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let end = en.project.tracks.iter().flat_map(|t| t.clips.iter()).map(|c| c.end()).fold(0.0, f64::max);
                let nbars = ((end / 4.0).ceil() as usize).clamp(0, 2048);
                let mut feats = vec![BarFeat::default(); nbars];
                for t in &en.project.tracks {
                    if matches!(t.kind, TrackKind::Audio) {
                        continue;
                    }
                    let drums = is_drums(t);
                    let mut active = vec![false; nbars];
                    for c in &t.clips {
                        for n in expand(c) {
                            let b = (n.start / 4.0).floor() as isize;
                            if b < 0 || b as usize >= nbars {
                                continue;
                            }
                            let b = b as usize;
                            if drums {
                                feats[b].drum_notes += 1;
                                if n.key == 35 || n.key == 36 {
                                    feats[b].kicks += 1;
                                }
                                if (n.key == 49 || n.key == 57) && n.start - b as f64 * 4.0 < 1.0 {
                                    feats[b].crash = true;
                                }
                            } else {
                                feats[b].notes += 1;
                                active[b] = true;
                            }
                        }
                    }
                    for (b, on) in active.iter().enumerate() {
                        if *on {
                            feats[b].tracks += 1;
                        }
                    }
                }
                drop(en);
                let (secs, fills) = arrange::detect_sections(&feats);
                let mut v: Vec<Val> = vec![];
                for s in secs {
                    v.extend([Val::Num(s.start_bar as f64 * 4.0), Val::Num(s.end_bar as f64 * 4.0), Val::Str(s.tag), Val::Num(s.energy)]);
                }
                for (a, b) in fills {
                    v.extend([Val::Num(a as f64 * 4.0), Val::Num(b as f64 * 4.0), Val::Str("fill".into()), Val::Num(1.0)]);
                }
                ok!(Val::List(v))
            }
            "daw_arr_struct_scan" => {
                // (mask) -> [track, clip (-1 = whole track), name, kind (pages | riff | skip), what it would do]*
                let mask = a.u(0) as u32;
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let mut v = vec![];
                for act in struct_plan(&en, mask) {
                    match act {
                        StAction::Pages { t, c, name, page, bars } => v.extend([Val::Num(t as f64), Val::Num(c as f64), Val::Str(name), Val::Str("pages".into()), Val::Str(format!("{bars} bars become a {}-bar page repeated, turning every 4th and 8th time ({}% alike now)", page.bars, (page.score * 100.0).round()))]),
                        StAction::Riff { t, name, statements, .. } => {
                            let txt = statements.iter().map(|s| format!("{} bars at bar {}{}", s.bars, (s.start / 4.0).floor() as i64 + 1, if s.from.is_none() { " (a repeat of the first)" } else { "" })).collect::<Vec<_>>().join("; ");
                            v.extend([Val::Num(t as f64), Val::Num(-1.0), Val::Str(name), Val::Str("riff".into()), Val::Str(format!("the riff gets {} statement(s): {txt}", statements.len()))]);
                        }
                        StAction::Skip { t, c, name, why } => v.extend([Val::Num(t as f64), Val::Num(c as f64), Val::Str(name), Val::Str("skip".into()), Val::Str(why)]),
                    }
                }
                ok!(Val::List(v))
            }
            "daw_arr_struct_apply" => {
                let mask = a.u(0) as u32;
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let plan = struct_plan(&en, mask);
                let n = struct_apply(&mut en, &plan);
                drop(en);
                if n > 0 {
                    self.touch();
                }
                ok!(n)
            }
            "daw_arr_tags_set" => {
                // (text "start:end:tag;...") stored in the project
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let text = a.s(0);
                if let Some(slot) = en.project.meta.iter_mut().find(|(k, _)| k == "sections") {
                    slot.1 = text;
                } else {
                    en.project.meta.push(("sections".into(), text));
                }
                drop(en);
                self.touch();
                Ok(Val::Unit)
            }
            "daw_arr_name" => {
                // (mask: 1 arrangement clips, 2 scenes, 4 session clips) -> how many names were written
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let n = auto_name(&mut en.project, a.u(0) as u32);
                drop(en);
                if n > 0 {
                    self.touch();
                }
                ok!(n)
            }
            "daw_arr_slice_scan" => {
                // (min bars) -> [track, clip, name, slices, text]*: the long solid clips and what cutting them into patterns would do
                let min_bars = (a.u(0)).max(2);
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let mut v = vec![];
                for (t, c, segs) in slice_plans(&en, min_bars) {
                    let cl = &en.project.tracks[t].clips[c];
                    let patterns = segs.iter().map(|s| s.group).collect::<std::collections::BTreeSet<_>>().len();
                    let folded: usize = segs.iter().filter(|s| s.repeats > 1).map(|s| s.page_bars * (s.repeats - 1)).sum();
                    v.extend([
                        Val::Num(t as f64),
                        Val::Num(c as f64),
                        Val::Str(format!("{} / {}", en.project.tracks[t].name, cl.name)),
                        Val::Num(segs.len() as f64),
                        Val::Str(format!("{} bars become {} clip(s) of {} pattern(s){}", (cl.len / 4.0).round(), segs.len(), patterns, if folded > 0 { format!(", {folded} bars folded into repeats") } else { String::new() })),
                    ]);
                }
                ok!(Val::List(v))
            }
            "daw_arr_slice_apply" => {
                // (min bars) -> clips made
                let min_bars = (a.u(0)).max(2);
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let plans = slice_plans(&en, min_bars);
                let n = apply_slices(&mut en, &plans);
                drop(en);
                if n > 0 {
                    self.touch();
                }
                ok!(n)
            }
            "daw_arr_glue_scan" | "daw_arr_glue_apply" => {
                // (max gap in beats, phrase limit in bars (0 = none), fold identical repeats into a loop 0/1, track (-1 = every track))
                // scan -> [track, name, clips now, clips after]*; apply -> clips removed (one undo step is the caller's)
                let (gap, max_len, fold) = (a.n(0).max(0.0), a.n(1).max(0.0) * 4.0, a.n(2) > 0.5);
                let only: Option<usize> = if a.0.len() > 3 && a.n(3) >= 0.0 { Some(a.n(3) as usize) } else { None };
                let apply = name == "daw_arr_glue_apply";
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let mut v = vec![];
                let mut removed = 0usize;
                for ti in 0..en.project.tracks.len() {
                    if only.map(|o| o != ti).unwrap_or(false) {
                        continue;
                    }
                    let Some(new) = glued_clips(&en.project.tracks[ti], gap, max_len, fold) else { continue };
                    let before = en.project.tracks[ti].clips.len();
                    removed += before - new.len();
                    v.extend([Val::Num(ti as f64), Val::Str(en.project.tracks[ti].name.clone()), Val::Num(before as f64), Val::Num(new.len() as f64)]);
                    if apply {
                        en.project.tracks[ti].clips = new;
                    }
                }
                drop(en);
                if apply {
                    if removed > 0 {
                        self.touch();
                    }
                    ok!(removed)
                }
                ok!(Val::List(v))
            }
            "daw_arr_tags" => {
                // () -> [start, end, tag]* of the tags stored in the project
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let text = en.project.meta.iter().find(|(k, _)| k == "sections").map(|(_, v)| v.clone()).unwrap_or_default();
                let mut v = vec![];
                for part in text.split(';') {
                    let f: Vec<&str> = part.split(':').collect();
                    if f.len() == 3 {
                        if let (Ok(s), Ok(e)) = (f[0].parse::<f64>(), f[1].parse::<f64>()) {
                            v.extend([Val::Num(s), Val::Num(e), Val::Str(f[2].to_string())]);
                        }
                    }
                }
                ok!(Val::List(v))
            }
            _ => Err(format!("unknown builtin {name}")),
        }
    }
}
