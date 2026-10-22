//! The Sheet tab: a score page drawn to a bitmap in-process, MusicXML / .mxl for MuseScore, finding MuseScore.
use ling_daw::{Daw, Val};

fn n(x: f64) -> Val {
    Val::Num(x)
}
fn s(x: &str) -> Val {
    Val::Str(x.into())
}
fn call(d: &mut Daw, name: &str, a: &[Val]) -> Val {
    d.call(name, a).unwrap_or_else(|| panic!("{name} not handled")).unwrap_or_else(|e| panic!("{name}: {e}"))
}
fn list(v: Val) -> Vec<Val> {
    match v {
        Val::List(l) => l,
        o => panic!("{o:?}"),
    }
}
fn num(v: &Val) -> f64 {
    if let Val::Num(x) = v {
        *x
    } else {
        f64::NAN
    }
}

fn melody() -> Daw {
    let mut d = Daw::new();
    call(&mut d, "daw_new", &[]);
    call(&mut d, "daw_track_add", &[s("synth"), s("")]);
    call(&mut d, "daw_clip_add_midi", &[n(0.0), n(0.0), n(16.0)]);
    for (i, k) in [60.0, 62.0, 64.0, 65.0, 67.0, 69.0, 71.0, 72.0].iter().enumerate() {
        call(&mut d, "daw_note_add", &[n(0.0), n(0.0), n(i as f64), n(1.0), n(*k), n(0.8)]);
    }
    d
}

const FONTS: &str = "../../../LingStationX/assets/fonts";

#[test]
fn a_score_page_is_drawn_to_a_png() {
    let mut d = melody();
    let r = list(call(&mut d, "daw_sheet_png", &[s("style=staff;bars=4"), s(FONTS), n(0.0), n(900.0)]));
    assert_eq!(r.len(), 7, "{r:?}");
    assert!(num(&r[0]) >= 1.0 && num(&r[2]) == 1.0, "pages and tracks");
    assert_eq!(num(&r[3]), 900.0);
    assert!(num(&r[4]) > 300.0, "a page has height");
    let Val::Str(path) = &r[6] else { panic!() };
    let png = std::fs::read(path).expect("the png exists");
    assert_eq!(&png[1..4], b"PNG");
    // the picture is not blank: the staff lines and noteheads are dark pixels on white
    let img = image_dark_pixels(&png);
    assert!(img > 500, "dark pixels {img}");
    // asking again for the same page is free: the same file, no new generation
    let r2 = list(call(&mut d, "daw_sheet_png", &[s("style=staff;bars=4"), s(FONTS), n(0.0), n(900.0)]));
    assert_eq!(num(&r2[5]), num(&r[5]));
    // an edit makes a new picture
    call(&mut d, "daw_note_add", &[n(0.0), n(0.0), n(8.0), n(1.0), n(76.0), n(0.8)]);
    let r3 = list(call(&mut d, "daw_sheet_png", &[s("style=staff;bars=4"), s(FONTS), n(0.0), n(900.0)]));
    assert!(num(&r3[5]) > num(&r[5]));
    // the grid style and a page past the end are both fine; an empty project draws nothing
    assert_eq!(list(call(&mut d, "daw_sheet_png", &[s("style=grid"), s(FONTS), n(99.0), n(600.0)])).len(), 7);
    let mut e = Daw::new();
    call(&mut e, "daw_new", &[]);
    assert!(list(call(&mut e, "daw_sheet_png", &[s("style=staff"), s(FONTS), n(0.0), n(600.0)])).is_empty());
    // hostile arguments
    for w in [-5.0, 0.0, 1e12, f64::NAN] {
        let _ = call(&mut d, "daw_sheet_png", &[s("style=staff;bars=junk;from=-3;to=99999"), s("/no/such/folder"), n(w), n(w)]);
    }
}

fn image_dark_pixels(png: &[u8]) -> usize {
    let pm = resvg::tiny_skia::Pixmap::decode_png(png).expect("decodes");
    pm.pixels().iter().filter(|p| (p.red() as u32 + p.green() as u32 + p.blue() as u32) / 3 < 100).count()
}

#[test]
fn musicxml_and_mxl_are_written_and_the_mxl_is_a_valid_zip() {
    let mut d = melody();
    let dir = std::env::temp_dir().join(format!("lsx_xml_{}", std::process::id()));
    let files = list(call(&mut d, "daw_sheet_xml", &[s(&dir.to_string_lossy()), s("tune"), s("bars=4;title=Scale")]));
    assert_eq!(files.len(), 2);
    let xml = std::fs::read_to_string(dir.join("tune.musicxml")).unwrap();
    assert!(xml.contains("<score-partwise") && xml.contains("Scale"));
    let z = std::fs::read(dir.join("tune.mxl")).unwrap();
    assert_eq!(&z[0..4], &[0x50, 0x4b, 0x03, 0x04]);
    // end-of-central-directory record: two entries; the second entry's bytes are exactly the score
    let eocd = z.len() - 22;
    assert_eq!(&z[eocd..eocd + 4], &[0x50, 0x4b, 0x05, 0x06]);
    assert_eq!(u16::from_le_bytes([z[eocd + 10], z[eocd + 11]]), 2);
    let cd = u32::from_le_bytes([z[eocd + 16], z[eocd + 17], z[eocd + 18], z[eocd + 19]]) as usize;
    assert_eq!(&z[cd..cd + 4], &[0x50, 0x4b, 0x01, 0x02], "the central directory starts where the end record says");
    assert_eq!(cd + u32::from_le_bytes([z[eocd + 12], z[eocd + 13], z[eocd + 14], z[eocd + 15]]) as usize, eocd, "and ends at the end record");
    assert!(z.windows(xml.len().min(200)).any(|w| w == &xml.as_bytes()[..xml.len().min(200)]), "the score is inside");
    assert!(String::from_utf8_lossy(&z).contains("META-INF/container.xml"));
    // an empty project writes nothing
    let mut e = Daw::new();
    call(&mut e, "daw_new", &[]);
    assert!(list(call(&mut e, "daw_sheet_xml", &[s(&dir.to_string_lossy()), s("none"), s("")])).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn musescore_is_looked_up_and_opening_without_it_fails_politely() {
    let mut d = Daw::new();
    let Val::Str(p) = call(&mut d, "daw_sheet_musescore_find", &[]) else { panic!() };
    // empty when it is not installed, an existing program when it is
    assert!(p.is_empty() || std::path::Path::new(&p).is_file(), "{p}");
    if p.is_empty() {
        assert_eq!(num(&call(&mut d, "daw_sheet_musescore_open", &[s("x.mxl")])), 0.0);
        let Val::Str(st) = call(&mut d, "daw_status", &[]) else { panic!() };
        assert!(st.contains("MuseScore"), "{st}");
    }
}

/// manual: `LSX_XML_OUT=<dir> cargo test -p ling-daw --test sheettab -- --ignored` writes a three-part score to open in MuseScore
#[test]
#[ignore]
fn write_a_sample_score_for_musescore() {
    let Ok(out) = std::env::var("LSX_XML_OUT") else { return };
    let mut d = melody();
    call(&mut d, "daw_track_add", &[s("synth"), s("")]);
    call(&mut d, "daw_clip_add_midi", &[n(1.0), n(0.0), n(16.0)]);
    for i in 0..8 {
        call(&mut d, "daw_note_add", &[n(1.0), n(0.0), n(i as f64 * 2.0), n(1.5), n(36.0 + (i % 4) as f64 * 3.0), n(0.8)]);
    }
    call(&mut d, "daw_track_add", &[s("drums"), s("")]);
    call(&mut d, "daw_clip_add_midi", &[n(2.0), n(0.0), n(16.0)]);
    for i in 0..16 {
        call(&mut d, "daw_note_add", &[n(2.0), n(0.0), n(i as f64), n(0.25), n(if i % 2 == 0 { 36.0 } else { 42.0 }), n(0.8)]);
        if i % 4 == 2 {
            call(&mut d, "daw_note_add", &[n(2.0), n(0.0), n(i as f64), n(0.25), n(38.0), n(0.9)]);
        }
    }
    call(&mut d, "daw_sheet_xml", &[s(&out), s("sample"), s("title=Sample")]);
}

fn geo(d: &mut Daw, opts: &str) -> Vec<f64> {
    list(call(d, "daw_sheet_geo", &[s(opts), n(0.0)])).iter().map(num).collect()
}

/// (key, start, x, y) of every notehead of a geo list
fn heads(g: &[f64]) -> Vec<(u8, f64, f64, f64)> {
    let mut i = 2;
    let ns = g[i] as usize;
    i += 1;
    for _ in 0..ns {
        let nst = g[i + 7] as usize;
        i += 8 + nst * 3;
    }
    let nn = g[i] as usize;
    i += 1;
    (0..nn).map(|k| (g[i + k * 5 + 1] as u8, g[i + k * 5 + 2], g[i + k * 5 + 3], g[i + k * 5 + 4])).collect()
}

#[test]
fn the_sheet_reports_where_notes_are_and_edits_them() {
    let mut d = melody();
    let o = "style=staff;bars=4;dark=1;landscape=0";
    let g = geo(&mut d, o);
    assert_eq!((g[0], g[1]), (990.0, 1400.0), "a portrait A4 page");
    let h = heads(&g);
    assert_eq!(h.len(), 8, "a head for every note: {h:?}");
    // left to right in time, higher notes higher up the page (smaller y)
    assert!(h.windows(2).all(|w| w[1].2 > w[0].2), "x follows the beat");
    let y60 = h.iter().find(|x| x.0 == 60).unwrap().3;
    let y72 = h.iter().find(|x| x.0 == 72).unwrap().3;
    assert!(y72 < y60, "C5 above C4");
    // a system: its beats, its horizontal scale, a staff
    let (nsys, b0, b1, bar_w) = (g[2], g[4], g[5], g[7]);
    assert!(nsys >= 1.0 && b0 == 0.0 && b1 >= 4.0 && bar_w > 100.0);
    // the playhead maps beat -> x with the same scale the heads use: head x == x0 + (beat - b0) / 4 * bar_w + 14 (+ a little for the head offset)
    let x0 = g[6];
    let x_of = |beat: f64| x0 + (beat - b0) / 4.0 * bar_w + 14.0;
    assert!((h[3].2 - x_of(h[3].1)).abs() < 12.0, "{} vs {}", h[3].2, x_of(h[3].1));

    // transpose the note at beat 2 (E4 = 64) up two semitones
    assert_eq!(num(&call(&mut d, "daw_sheet_edit", &[n(0.0), n(1.0), n(64.0), n(2.0), n(2.0)])), 1.0);
    let h2 = heads(&geo(&mut d, o));
    assert!(h2.iter().any(|x| x.0 == 66 && (x.1 - 2.0).abs() < 1e-6) && !h2.iter().any(|x| x.0 == 64));
    // move it a beat later, shorten it, delete another, add a new one
    assert_eq!(num(&call(&mut d, "daw_sheet_edit", &[n(0.0), n(2.0), n(66.0), n(2.5), n(1.0)])), 1.0, "a beat inside the note finds it");
    assert!(heads(&geo(&mut d, o)).iter().any(|x| x.0 == 66 && (x.1 - 3.0).abs() < 1e-6));
    assert_eq!(num(&call(&mut d, "daw_sheet_edit", &[n(0.0), n(3.0), n(66.0), n(3.0), n(0.25)])), 1.0);
    assert_eq!(num(&call(&mut d, "daw_sheet_edit", &[n(0.0), n(0.0), n(60.0), n(0.0), n(0.0)])), 1.0);
    assert!(!heads(&geo(&mut d, o)).iter().any(|x| x.0 == 60));
    assert_eq!(num(&call(&mut d, "daw_sheet_edit", &[n(0.0), n(4.0), n(79.0), n(9.0), n(1.0)])), 1.0, "adding past the clip makes room");
    assert!(heads(&geo(&mut d, o)).iter().any(|x| x.0 == 79 && (x.1 - 9.0).abs() < 1e-6));
    // nothing there: nothing changes; hostile arguments are safe
    assert_eq!(num(&call(&mut d, "daw_sheet_edit", &[n(0.0), n(0.0), n(11.0), n(1.0), n(0.0)])), 0.0);
    for v in [f64::NAN, -1.0, 1e18, f64::INFINITY] {
        let _ = call(&mut d, "daw_sheet_edit", &[n(v), n(v), n(v), n(v), n(v)]);
    }
    // the grid style has no geometry
    assert!(list(call(&mut d, "daw_sheet_geo", &[s("style=grid"), n(0.0)])).is_empty());
}

#[test]
fn editing_a_note_never_starts_the_transport() {
    let mut d = melody();
    assert_eq!(num(&call(&mut d, "daw_playing", &[])), 0.0);
    call(&mut d, "daw_sheet_edit", &[n(0.0), n(4.0), n(67.0), n(9.0), n(1.0)]);
    call(&mut d, "daw_sheet_edit", &[n(0.0), n(1.0), n(67.0), n(9.0), n(2.0)]);
    assert_eq!(num(&call(&mut d, "daw_playing", &[])), 0.0);
}
