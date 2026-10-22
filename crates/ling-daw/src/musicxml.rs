//! MusicXML export: the project's notes as an uncompressed MusicXML 3.1 score (partwise), for MuseScore, Sibelius, Dorico, Finale and friends.
//!
//! One part per instrument track. Notes sit on a sixteenth-note grid; a track that spans the keyboard becomes a grand staff (treble above
//! bass, split at middle C) like the sheet-music export; drums become an unpitched percussion part. Notes that start together are a chord, a
//! note that runs past a barline is split and tied, gaps are rests, lyrics ride on the notes they are sung on. Where two notes overlap in
//! time on one staff the earlier one is cut at the next onset (the score is a readable copy of the music, the MIDI stays the master).
use crate::model::*;

const UNITS_PER_BEAT: i64 = 4; // sixteenths
const UNITS_PER_BAR: i64 = 16;

#[derive(Clone, Debug)]
pub struct MxOpts {
    pub tracks: Vec<usize>,
    pub from_bar: usize,
    pub to_bar: usize,
    pub title: String,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[derive(Clone)]
struct Nt {
    s: i64,
    e: i64,
    key: u8,
}

fn track_notes(t: &Track) -> Vec<Nt> {
    let mut out = vec![];
    for c in &t.clips {
        let ClipData::Midi(notes) = &c.data else { continue };
        let ll = if c.loop_len >= 0.25 && c.loop_len < c.len { c.loop_len } else { 0.0 };
        let reps = if ll > 0.0 { ((c.len / ll).ceil() as usize).min(512) } else { 1 };
        for k in 0..reps {
            let base = c.start + k as f64 * ll;
            let limit = if ll > 0.0 { ll.min(c.len - k as f64 * ll) } else { c.len };
            for n in notes.iter().filter(|n| n.start < limit) {
                let s = ((base + n.start) * UNITS_PER_BEAT as f64).round() as i64;
                let e = (((base + n.start + n.len.min(limit - n.start)) * UNITS_PER_BEAT as f64).round() as i64).max(s + 1);
                out.push(Nt { s, e, key: n.key });
            }
        }
    }
    out.sort_by(|a, b| a.s.cmp(&b.s).then(a.key.cmp(&b.key)));
    out
}

/// note type name and dot for a length in sixteenths
const DURS: [(i64, &str, bool); 8] = [(16, "whole", false), (12, "half", true), (8, "half", false), (6, "quarter", true), (4, "quarter", false), (3, "eighth", true), (2, "eighth", false), (1, "16th", false)];

fn split_dur(mut units: i64, pos_in_bar: i64) -> Vec<i64> {
    // a value may start only where its kind sits in a bar: a whole note on the downbeat, a half on beats 1 and 3, a quarter or a dotted value on a beat
    let align = |d: i64| -> i64 {
        match d {
            16 => 16,
            12 => 4,
            8 => 8,
            6 => 4,
            4 => 4,
            3 => 2,
            2 => 2,
            _ => 1,
        }
    };
    let mut out = vec![];
    let mut pos = pos_in_bar;
    while units > 0 {
        let pick = DURS.iter().map(|d| d.0).find(|d| *d <= units && pos % align(*d) == 0).unwrap_or(1);
        out.push(pick);
        units -= pick;
        pos += pick;
    }
    out
}

fn type_of(d: i64) -> (&'static str, bool) {
    DURS.iter().find(|x| x.0 == d).map(|x| (x.1, x.2)).unwrap_or(("16th", false))
}

fn pitch_xml(key: u8) -> String {
    const STEP: [(&str, i32); 12] = [("C", 0), ("C", 1), ("D", 0), ("D", 1), ("E", 0), ("F", 0), ("F", 1), ("G", 0), ("G", 1), ("A", 0), ("A", 1), ("B", 0)];
    let (st, alter) = STEP[(key % 12) as usize];
    let octave = key as i32 / 12 - 1;
    format!("<pitch><step>{st}</step>{}<octave>{octave}</octave></pitch>", if alter != 0 { format!("<alter>{alter}</alter>") } else { String::new() })
}

/// unpitched display position and notehead of a GM drum key
fn drum_xml(key: u8) -> (&'static str, i32, &'static str) {
    match key {
        35 | 36 => ("F", 4, "normal"),
        37..=40 => ("C", 5, "normal"),
        41 | 43 | 45 => ("A", 4, "normal"),
        47 | 48 | 50 => ("D", 5, "normal"),
        42 | 44 => ("G", 5, "x"),
        46 => ("G", 5, "x"),
        49 | 52 | 55 | 57 => ("A", 5, "x"),
        51 | 53 | 56 | 59 => ("F", 5, "x"),
        _ => ("E", 5, "triangle"),
    }
}

struct Chord {
    s: i64,
    e: i64,
    keys: Vec<u8>,
}

/// the chords of one staff, cut so that none overlaps the next
fn chords_of(notes: &[Nt]) -> Vec<Chord> {
    let mut out: Vec<Chord> = vec![];
    let mut i = 0;
    while i < notes.len() {
        let s = notes[i].s;
        let mut j = i;
        let mut keys = vec![];
        let mut e = i64::MAX;
        let mut emax = 0;
        while j < notes.len() && notes[j].s == s {
            keys.push(notes[j].key);
            e = e.min(notes[j].e);
            emax = emax.max(notes[j].e);
            j += 1;
        }
        // a lone note keeps its whole length, a chord takes its shortest member's
        let end = if keys.len() == 1 { emax } else { e };
        keys.dedup();
        out.push(Chord { s, e: end, keys });
        i = j;
    }
    for k in 0..out.len().saturating_sub(1) {
        let next = out[k + 1].s;
        if out[k].e > next {
            out[k].e = next;
        }
    }
    out
}

/// every <note> of one staff for the whole piece, measure by measure: returns the XML per measure
#[allow(clippy::too_many_arguments)]
fn staff_measures(chords: &[Chord], bars: usize, from: usize, staff: usize, drums: bool, voice: usize, lyr: &[(i64, String, &'static str)]) -> Vec<String> {
    let mut out = vec![];
    let mut ci = 0usize;
    for bar in from..from + bars {
        let (b0, b1) = (bar as i64 * UNITS_PER_BAR, (bar as i64 + 1) * UNITS_PER_BAR);
        let mut x = String::new();
        let mut at = b0;
        // chords that end before this bar are behind us
        while ci < chords.len() && chords[ci].e <= b0 {
            ci += 1;
        }
        let mut k = ci;
        while k < chords.len() && chords[k].s < b1 {
            let c = &chords[k];
            let (s, e) = (c.s.max(b0), c.e.min(b1));
            let tie_in = c.s < b0;
            let tie_out = c.e > b1;
            if s > at {
                for d in split_dur(s - at, at - b0) {
                    let (ty, dot) = type_of(d);
                    x.push_str(&format!("<note><rest/><duration>{d}</duration><voice>{voice}</voice><type>{ty}</type>{}<staff>{staff}</staff></note>\n", if dot { "<dot/>" } else { "" }));
                }
            }
            // a chord starts in this measure or is held into it: write its pieces
            let pieces = split_dur(e - s, s - b0);
            for (pi, d) in pieces.iter().enumerate() {
                let (ty, dot) = type_of(*d);
                let first = pi == 0;
                let last = pi + 1 == pieces.len();
                let start_tie = !last || tie_out;
                let stop_tie = !first || tie_in;
                for (ki, key) in c.keys.iter().enumerate() {
                    let mut n = String::from("<note>");
                    if ki > 0 {
                        n.push_str("<chord/>");
                    }
                    if drums {
                        let (st, oc, head) = drum_xml(*key);
                        n.push_str(&format!("<unpitched><display-step>{st}</display-step><display-octave>{oc}</display-octave></unpitched>"));
                        n.push_str(&format!("<duration>{d}</duration><voice>{voice}</voice><type>{ty}</type>"));
                        if dot {
                            n.push_str("<dot/>");
                        }
                        n.push_str(&format!("<notehead>{head}</notehead><staff>{staff}</staff>"));
                    } else {
                        n.push_str(&pitch_xml(*key));
                        n.push_str(&format!("<duration>{d}</duration>"));
                        if stop_tie {
                            n.push_str("<tie type=\"stop\"/>");
                        }
                        if start_tie {
                            n.push_str("<tie type=\"start\"/>");
                        }
                        n.push_str(&format!("<voice>{voice}</voice><type>{ty}</type>"));
                        if dot {
                            n.push_str("<dot/>");
                        }
                        n.push_str(&format!("<staff>{staff}</staff>"));
                        if stop_tie || start_tie {
                            n.push_str("<notations>");
                            if stop_tie {
                                n.push_str("<tied type=\"stop\"/>");
                            }
                            if start_tie {
                                n.push_str("<tied type=\"start\"/>");
                            }
                            n.push_str("</notations>");
                        }
                        if ki == 0 && first && !tie_in {
                            if let Some((_, text, syl)) = lyr.iter().find(|l| l.0 == c.s) {
                                n.push_str(&format!("<lyric number=\"1\"><syllabic>{syl}</syllabic><text>{}</text></lyric>", esc(text)));
                            }
                        }
                    }
                    n.push_str("</note>\n");
                    x.push_str(&n);
                }
            }
            at = e;
            k += 1;
        }
        if at < b1 {
            // the rest of the bar: a whole-bar rest when nothing played in it
            if at == b0 {
                x.push_str(&format!("<note><rest measure=\"yes\"/><duration>16</duration><voice>{voice}</voice><staff>{staff}</staff></note>\n"));
            } else {
                for d in split_dur(b1 - at, at - b0) {
                    let (ty, dot) = type_of(d);
                    x.push_str(&format!("<note><rest/><duration>{d}</duration><voice>{voice}</voice><type>{ty}</type>{}<staff>{staff}</staff></note>\n", if dot { "<dot/>" } else { "" }));
                }
            }
        }
        out.push(x);
    }
    out
}

pub fn musicxml(p: &Project, o: &MxOpts, lyric_fn: &dyn Fn(usize) -> Vec<(f64, f64, String, bool)>) -> (String, usize) {
    let end_beat = p.end_beat().max(4.0);
    let total_bars = (end_beat / 4.0).ceil() as usize;
    let from = o.from_bar.max(1) - 1;
    let to = if o.to_bar == 0 { total_bars } else { o.to_bar.min(total_bars) };
    let bars = to.saturating_sub(from).max(1);
    let want: Vec<usize> = if o.tracks.is_empty() { (0..p.tracks.len()).collect() } else { o.tracks.clone() };
    let title = if o.title.is_empty() { p.name.clone() } else { o.title.clone() };
    let mut x = String::new();
    x.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE score-partwise PUBLIC \"-//Recordare//DTD MusicXML 3.1 Partwise//EN\" \"http://www.musicxml.org/dtds/partwise.dtd\">\n<score-partwise version=\"3.1\">\n");
    x.push_str(&format!("<work><work-title>{}</work-title></work>\n<identification><encoding><software>LingStation</software></encoding></identification>\n", esc(&title)));
    let mut parts: Vec<(usize, String, Vec<Nt>, bool)> = vec![];
    for ti in want {
        let Some(t) = p.tracks.get(ti) else { continue };
        if !matches!(t.kind, TrackKind::Instrument) {
            continue;
        }
        let notes: Vec<Nt> = track_notes(t).into_iter().filter(|n| n.e > from as i64 * UNITS_PER_BAR && n.s < to as i64 * UNITS_PER_BAR).collect();
        if notes.is_empty() {
            continue;
        }
        parts.push((ti, t.name.clone(), notes, t.inst_kind == "drums"));
    }
    if parts.is_empty() {
        return (String::new(), 0);
    }
    x.push_str("<part-list>\n");
    for (k, (_, name, _, _)) in parts.iter().enumerate() {
        x.push_str(&format!("<score-part id=\"P{}\"><part-name>{}</part-name></score-part>\n", k + 1, esc(name)));
    }
    x.push_str("</part-list>\n");
    for (k, (ti, _, notes, drums)) in parts.iter().enumerate() {
        x.push_str(&format!("<part id=\"P{}\">\n", k + 1));
        let (lo, hi) = (notes.iter().map(|n| n.key).min().unwrap_or(60), notes.iter().map(|n| n.key).max().unwrap_or(60));
        let two = !*drums && lo < 55 && hi >= 62;
        let bass_only = !*drums && hi < 62;
        // syllables with the grid position they sit on and how a score marks them (single / begin / middle / end)
        let syl = lyric_fn(*ti);
        let mut lyr: Vec<(i64, String, &'static str)> = vec![];
        for (i, (s0, _e, text, ws)) in syl.iter().enumerate() {
            let next_cont = syl.get(i + 1).map(|n| !n.3).unwrap_or(false);
            let tag = match (*ws, next_cont) {
                (true, false) => "single",
                (true, true) => "begin",
                (false, true) => "middle",
                (false, false) => "end",
            };
            lyr.push(((*s0 * UNITS_PER_BEAT as f64).round() as i64, text.clone(), tag));
        }
        let (upper, lower): (Vec<Nt>, Vec<Nt>) = if two { notes.iter().cloned().partition(|n| n.key >= 60) } else { (notes.clone(), vec![]) };
        let m1 = staff_measures(&chords_of(&upper), bars, from, 1, *drums, 1, &lyr);
        let m2 = if two { staff_measures(&chords_of(&lower), bars, from, 2, false, 5, &[]) } else { vec![] };
        for b in 0..bars {
            x.push_str(&format!("<measure number=\"{}\">\n", from + b + 1));
            if b == 0 {
                x.push_str("<attributes><divisions>4</divisions><key><fifths>0</fifths></key><time><beats>4</beats><beat-type>4</beat-type></time>");
                if two {
                    x.push_str("<staves>2</staves><clef number=\"1\"><sign>G</sign><line>2</line></clef><clef number=\"2\"><sign>F</sign><line>4</line></clef>");
                } else if *drums {
                    x.push_str("<clef><sign>percussion</sign></clef>");
                } else if bass_only {
                    x.push_str("<clef><sign>F</sign><line>4</line></clef>");
                } else {
                    x.push_str("<clef><sign>G</sign><line>2</line></clef>");
                }
                x.push_str("</attributes>\n");
                if k == 0 {
                    x.push_str(&format!("<direction placement=\"above\"><direction-type><metronome><beat-unit>quarter</beat-unit><per-minute>{:.0}</per-minute></metronome></direction-type><sound tempo=\"{:.0}\"/></direction>\n", p.bpm, p.bpm));
                }
            }
            x.push_str(&m1[b]);
            if two {
                x.push_str("<backup><duration>16</duration></backup>\n");
                x.push_str(&m2[b]);
            }
            x.push_str("</measure>\n");
        }
        x.push_str("</part>\n");
    }
    x.push_str("</score-partwise>\n");
    (x, parts.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_split_into_notation_values() {
        assert_eq!(split_dur(4, 0), vec![4]);
        assert_eq!(split_dur(6, 0), vec![6]);
        assert_eq!(split_dur(5, 0), vec![4, 1]);
        assert_eq!(split_dur(16, 0), vec![16]);
        assert_eq!(split_dur(8, 2).iter().sum::<i64>(), 8);
        for u in 1..=16 {
            for pos in 0..16 {
                if pos + u <= 16 {
                    assert_eq!(split_dur(u, pos).iter().sum::<i64>(), u, "{u} at {pos}");
                }
            }
        }
    }

    #[test]
    fn a_small_piece_becomes_well_formed_parts_with_ties_and_rests() {
        let mut p = Project::new();
        let mut t = Track::new("Piano", TrackKind::Instrument, 0xff0000, None);
        let mut c = Clip::midi(0.0, 8.0, "a");
        let mut notes = vec![Note::new(0.0, 1.0, 60, 0.8), Note::new(0.0, 1.0, 64, 0.8), Note::new(3.0, 2.0, 67, 0.8), Note::new(2.0, 1.0, 40, 0.8)];
        notes.sort_by(|a, b| a.start.total_cmp(&b.start));
        c.data = ClipData::Midi(notes);
        t.clips.push(c);
        p.tracks.push(t);
        let (xml, n) = musicxml(&p, &MxOpts { tracks: vec![], from_bar: 1, to_bar: 0, title: "T".into() }, &|_| vec![]);
        assert_eq!(n, 1);
        assert!(xml.starts_with("<?xml") && xml.contains("<score-partwise") && xml.trim_end().ends_with("</score-partwise>"));
        assert!(xml.contains("<chord/>"), "C and E together are a chord");
        assert!(xml.contains("<tie type=\"start\"/>") && xml.contains("<tie type=\"stop\"/>"), "the G held over the barline is tied");
        assert!(xml.contains("<staves>2</staves>"), "the range needs a grand staff");
        assert!(xml.contains("<rest"), "gaps are rests");
        // every measure of the staff adds up to a bar: sum the <duration> of notes per measure (staff 1)
        for m in xml.split("<measure ").skip(1) {
            let body = m.split("</measure>").next().unwrap();
            let first_staff: i64 = body.split("<backup>").next().unwrap().split("<note>").skip(1).filter(|n| !n.contains("<chord/>")).map(|n| n.split("<duration>").nth(1).and_then(|d| d.split('<').next()).and_then(|d| d.parse::<i64>().ok()).unwrap_or(0)).sum();
            assert_eq!(first_staff, 16, "a bar of 4/4 in measure: {body}");
        }
    }
}
