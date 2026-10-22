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
fn nums(v: &Val) -> Vec<f64> {
    match v {
        Val::List(l) => l.iter().map(|x| if let Val::Num(x) = x { *x } else { f64::NAN }).collect(),
        _ => vec![],
    }
}

#[test]
fn karaoke_files_from_a_fitted_lyric_clip() {
    let mut d = Daw::new();
    call(&mut d, "daw_new", &[]);
    let tr = match call(&mut d, "daw_track_add", &[s("voice"), s("")]) {
        Val::Num(x) => x,
        o => panic!("{o:?}"),
    };
    let c = match call(&mut d, "daw_clip_add_midi", &[n(tr), n(0.0), n(16.0)]) {
        Val::Num(x) => x,
        o => panic!("{o:?}"),
    };
    // eight quarter notes, then a rest, then two more
    for (i, k) in [60.0, 62.0, 64.0, 65.0, 67.0, 69.0, 71.0, 72.0].iter().enumerate() {
        call(&mut d, "daw_note_add", &[n(tr), n(c), n(i as f64), n(1.0), n(*k), n(0.8)]);
    }
    call(&mut d, "daw_note_add", &[n(tr), n(c), n(12.0), n(2.0), n(67.0), n(0.8)]);
    call(&mut d, "daw_note_add", &[n(tr), n(c), n(14.0), n(2.0), n(64.0), n(0.8)]);
    call(&mut d, "daw_lyrf_apply", &[n(tr), n(c), s("Hello wonderful world tonight\nGoodbye now"), s("fit:fit:0:0.3:all:1.0:auto")]);
    let scan = nums(&call(&mut d, "daw_kar_scan", &[]));
    assert!(scan[0] >= 8.0, "syllables: {scan:?}");
    assert_eq!(scan[3], 1.0);
    let dir = std::env::temp_dir().join(format!("lsx_kar_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let files = call(&mut d, "daw_kar_write", &[s(&dir.to_string_lossy()), s("song"), s("sung=FFC83C;outw=6;outa=0.5;cursor=ball;audio=song.wav;fontsdir=../../../../LingStationX/assets/fonts"), s("ass,srt,lrc,html")]);
    let names: Vec<String> = match files {
        Val::List(l) => l.into_iter().map(|v| if let Val::Str(s) = v { s } else { String::new() }).collect(),
        o => panic!("{o:?}"),
    };
    assert_eq!(names, vec!["song.ass", "song.srt", "song_karaoke.srt", "song.lrc", "song.html"]);
    let ass = std::fs::read_to_string(dir.join("song.ass")).unwrap();
    assert!(ass.contains("Hel") && ass.contains(r"{\kf"), "{ass}");
    let lrc = std::fs::read_to_string(dir.join("song.lrc")).unwrap();
    assert!(lrc.contains("<00:00.00>"), "{lrc}");
    let html = std::fs::read_to_string(dir.join("song.html")).unwrap();
    assert!(html.contains("const D={\"lines\":[[[") && html.contains("\"song.wav\""), "player has the data");
    let _ = std::fs::copy(dir.join("song.html"), std::env::temp_dir().join("lsx_last_karaoke.html"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sheet_music_pages_for_notes_chords_drums_and_lyrics() {
    let mut d = Daw::new();
    call(&mut d, "daw_new", &[]);
    let num = |v: Val| if let Val::Num(x) = v { x } else { panic!() };
    let voice = num(call(&mut d, "daw_track_add", &[s("voice"), s("")]));
    let vc = num(call(&mut d, "daw_clip_add_midi", &[n(voice), n(0.0), n(16.0)]));
    for (i, k) in [64.0, 66.0, 68.0, 69.0, 71.0, 73.0, 75.0, 76.0, 75.0, 73.0, 71.0, 69.0].iter().enumerate() {
        call(&mut d, "daw_note_add", &[n(voice), n(vc), n(i as f64 * 1.25), n(1.0), n(*k), n(0.5 + (i % 4) as f64 * 0.12)]);
    }
    call(&mut d, "daw_lyrf_apply", &[n(voice), n(vc), s("Twin-kle twin-kle lit-tle star\nHow I won-der what you are"), s("fit:fit:0:0.3:all:1.0:auto")]);
    let keys = num(call(&mut d, "daw_track_add", &[s("synth"), s("")]));
    let kc = num(call(&mut d, "daw_clip_add_midi", &[n(keys), n(0.0), n(16.0)]));
    for b in 0..4 {
        for k in [48.0, 52.0, 55.0, 60.0] {
            call(&mut d, "daw_note_add", &[n(keys), n(kc), n(b as f64 * 4.0), n(3.5), n(k + (b % 2) as f64 * 5.0), n(0.8)]);
        }
    }
    let dr = num(call(&mut d, "daw_track_add", &[s("drums"), s("")]));
    let dc = num(call(&mut d, "daw_clip_add_midi", &[n(dr), n(0.0), n(16.0)]));
    for b in 0..16 {
        call(&mut d, "daw_note_add", &[n(dr), n(dc), n(b as f64), n(0.25), n(if b % 2 == 0 { 36.0 } else { 38.0 }), n(0.9)]);
        call(&mut d, "daw_note_add", &[n(dr), n(dc), n(b as f64 + 0.5), n(0.25), n(42.0), n(0.6)]);
    }
    let scan = nums(&call(&mut d, "daw_sheet_scan", &[]));
    assert_eq!(scan[0], 4.0);
    assert_eq!(scan.len(), 1 + 5 * 3);
    let dir = std::env::temp_dir().join(format!("lsx_sheet_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let files = call(&mut d, "daw_sheet_write", &[s(&dir.to_string_lossy()), s("song"), s("font=daemon;bars=4;title=Twinkle"), s("../../../LingStationX/assets/fonts")]);
    let Val::List(files) = files else { panic!() };
    assert!(files.len() >= 2, "svg + html");
    let svg = std::fs::read_to_string(dir.join("song.svg")).unwrap();
    assert!(svg.contains("<rect") && svg.contains("Twinkle") && svg.contains("LSXDaemon"), "page has tiles, title and the font");
    let html = std::fs::read_to_string(dir.join("song.html")).unwrap();
    assert!(html.contains("@page"));
    let _ = std::fs::copy(dir.join("song.html"), std::env::temp_dir().join("lsx_last_sheet.html"));
    // PDF of the whole score and of every part (needs a Chromium-family browser; skipped where there is none)
    let Val::List(pf) = call(&mut d, "daw_sheet_write", &[s(&dir.to_string_lossy()), s("song"), s("font=daemon;bars=4;title=Twinkle"), s("../../../LingStationX/assets/fonts"), s("pdf+parts")]) else { panic!() };
    let pdfs: Vec<String> = pf.iter().filter_map(|v| if let Val::Str(x) = v { Some(x.clone()) } else { None }).filter(|x| x.ends_with(".pdf")).collect();
    if !pdfs.is_empty() {
        assert_eq!(pdfs.len(), 4, "the score and three parts: {pdfs:?}");
        for p in &pdfs {
            let b = std::fs::read(dir.join(p)).unwrap();
            assert!(b.starts_with(b"%PDF"), "{p} is a PDF");
            let _ = std::fs::copy(dir.join(p), std::env::temp_dir().join(format!("lsx_last_{}", p.replace(' ', "_"))));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sheet_music_with_many_tracks_flows_onto_more_pages_without_overflow() {
    let mut d = Daw::new();
    call(&mut d, "daw_new", &[]);
    let num = |v: Val| if let Val::Num(x) = v { x } else { panic!() };
    for t in 0..9 {
        let tr = num(call(&mut d, "daw_track_add", &[s("synth"), s("")]));
        let c = num(call(&mut d, "daw_clip_add_midi", &[n(tr), n(0.0), n(32.0)]));
        for i in 0..32 {
            call(&mut d, "daw_note_add", &[n(tr), n(c), n(i as f64), n(0.5), n(40.0 + (t * 6) as f64 + (i % 9) as f64 * 3.0), n(0.8)]);
        }
    }
    let dir = std::env::temp_dir().join(format!("lsx_sheet_many_{}", std::process::id()));
    for (style, name) in [("staff", "many_staff"), ("grid", "many_grid")] {
        let Val::List(f) = call(&mut d, "daw_sheet_write", &[s(&dir.to_string_lossy()), s(name), s(&format!("style={style};bars=4")), s("../../../LingStationX/assets/fonts")]) else { panic!() };
        assert!(f.len() >= 2);
        let html = std::fs::read_to_string(dir.join(format!("{name}.html"))).unwrap();
        // every y coordinate drawn stays inside the 990 px page
        let mut worst = 0.0f64;
        for part in html.split(" y=\"").skip(1) {
            if let Some(v) = part.split('"').next().and_then(|x| x.parse::<f64>().ok()) {
                if (v - 978.0).abs() > 0.5 {
                    worst = worst.max(v);
                }
            }
        }
        assert!(worst <= 975.0, "{style}: something is drawn at y={worst}");
        let _ = std::fs::copy(dir.join(format!("{name}.html")), std::env::temp_dir().join(format!("lsx_last_{name}.html")));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn festival_project_sheet_stays_on_its_pages() {
    let path = "../../../LingStationX/projects/Festival/Festival.json";
    if !std::path::Path::new(path).is_file() {
        return;
    }
    let mut d = Daw::new();
    call(&mut d, "daw_project_load", &[s(path)]);
    let dir = std::env::temp_dir().join(format!("lsx_sheet_fest_{}", std::process::id()));
    let Val::List(f) = call(&mut d, "daw_sheet_write", &[s(&dir.to_string_lossy()), s("fest"), s("style=staff;dark=1;bars=4"), s("../../../LingStationX/assets/fonts")]) else { panic!() };
    assert!(f.len() >= 2, "written");
    let svgs: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.path().extension().map(|x| x == "svg").unwrap_or(false)).collect();
    eprintln!("festival: {} pages", svgs.len());
    let html = std::fs::read_to_string(dir.join("fest.html")).unwrap();
    let mut worst = 0.0f64;
    for part in html.split(" y=\"").skip(1) {
        if let Some(v) = part.split('"').next().and_then(|x| x.parse::<f64>().ok()) {
            if (v - 978.0).abs() > 0.5 {
                worst = worst.max(v);
            }
        }
    }
    assert!(worst <= 975.0, "something is drawn at y={worst}");
    let _ = std::fs::copy(dir.join("fest.html"), std::env::temp_dir().join("lsx_last_fest.html"));
    call(&mut d, "daw_sheet_write", &[s(&dir.to_string_lossy()), s("fest"), s("style=staff;bars=4"), s("../../../LingStationX/assets/fonts"), s("xml")]);
    let xml = std::fs::read_to_string(dir.join("fest.musicxml")).expect("musicxml written");
    assert!(xml.contains("<score-partwise") && xml.contains("<lyric"), "a score with lyrics");
    let _ = std::fs::copy(dir.join("fest.musicxml"), std::env::temp_dir().join("lsx_last_fest.musicxml"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn export_tracker_pages_as_pdf() {
    let path = "../../../LingStationX/projects/Festival/Festival.json";
    if !std::path::Path::new(path).is_file() {
        return;
    }
    let mut d = Daw::new();
    call(&mut d, "daw_project_load", &[s(path)]);
    let dir = std::env::temp_dir().join(format!("lsx_trk_{}", std::process::id()));
    let Val::List(f) = call(&mut d, "daw_trk_pdf", &[s(&dir.to_string_lossy()), s("trk"), s("font=daemon;rows=64;rpb=4;dark=0"), s("../../../LingStationX/assets/fonts"), s("pdf")]) else { panic!() };
    assert!(f.len() >= 1, "html written");
    let html = std::fs::read_to_string(dir.join("trk.html")).unwrap();
    assert!(html.contains("PATTERN") && html.contains("C-") , "pages have patterns and notes");
    let mut worst = 0.0f64;
    for part in html.split(" y=\"").skip(1) {
        if let Some(v) = part.split('"').next().and_then(|x| x.parse::<f64>().ok()) {
            if (v - 978.0).abs() > 0.5 {
                worst = worst.max(v);
            }
        }
    }
    assert!(worst <= 975.0, "something is drawn at y={worst}");
    if dir.join("trk.pdf").is_file() {
        assert!(std::fs::read(dir.join("trk.pdf")).unwrap().starts_with(b"%PDF"));
        let _ = std::fs::copy(dir.join("trk.pdf"), std::env::temp_dir().join("lsx_last_trk.pdf"));
    }
    let _ = std::fs::copy(dir.join("trk.html"), std::env::temp_dir().join("lsx_last_trk.html"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn glue_clips_into_phrases_and_fold_repeats() {
    let mut d = Daw::new();
    call(&mut d, "daw_new", &[]);
    let num = |v: Val| if let Val::Num(x) = v { x } else { panic!() };
    let tr = num(call(&mut d, "daw_track_add", &[s("synth"), s("")]));
    // eight identical bars, then four different touching bars, then a bar after a gap of two bars
    for b in 0..8 {
        let c = num(call(&mut d, "daw_clip_add_midi", &[n(tr), n(b as f64 * 4.0), n(4.0)]));
        call(&mut d, "daw_note_add", &[n(tr), n(c), n(0.0), n(1.0), n(60.0), n(0.8)]);
    }
    for b in 0..4 {
        let c = num(call(&mut d, "daw_clip_add_midi", &[n(tr), n(32.0 + b as f64 * 4.0), n(4.0)]));
        call(&mut d, "daw_note_add", &[n(tr), n(c), n(0.0), n(1.0), n(62.0 + b as f64), n(0.8)]);
    }
    let c = num(call(&mut d, "daw_clip_add_midi", &[n(tr), n(56.0), n(4.0)]));
    call(&mut d, "daw_note_add", &[n(tr), n(c), n(0.0), n(1.0), n(70.0), n(0.8)]);
    assert_eq!(num(call(&mut d, "daw_clip_count", &[n(tr)])), 13.0);
    let Val::List(scan) = call(&mut d, "daw_arr_glue_scan", &[n(0.0), n(0.0), n(1.0), n(-1.0)]) else { panic!() };
    assert_eq!(scan.len(), 4, "one track would change");
    // 8 repeats -> 1 looped clip, 4 different -> 1 phrase, the gapped bar stays: 3 clips
    let removed = num(call(&mut d, "daw_arr_glue_apply", &[n(0.0), n(0.0), n(1.0), n(-1.0)]));
    assert_eq!(removed, 10.0);
    assert_eq!(num(call(&mut d, "daw_clip_count", &[n(tr)])), 3.0);
    let g = |d: &mut Daw, c: f64, k: &str| num(call(d, "daw_clip_get", &[n(tr), n(c), s(k)]));
    assert_eq!(g(&mut d, 0.0, "len"), 32.0);
    assert_eq!(g(&mut d, 0.0, "loop"), 4.0);
    assert_eq!(g(&mut d, 1.0, "len"), 16.0);
    // a gap of two bars is bridged when allowed
    let removed = num(call(&mut d, "daw_arr_glue_apply", &[n(8.0), n(0.0), n(1.0), n(-1.0)]));
    assert_eq!(removed, 1.0, "the phrase and the late bar join");
}

fn voice_wav(path: &std::path::Path, notes: &[(f64, f64)], wobble: f64) -> f64 {
    // a buzzy "voice": a few harmonics with a little vibrato, a gap between the notes
    let sr = 24000u32;
    let spec = hound::WavSpec { channels: 2, sample_rate: sr, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    let mut t_total = 0.0;
    for (hz, dur) in notes {
        let n = (dur * sr as f64) as usize;
        let mut ph = 0.0f64;
        for i in 0..n {
            let t = i as f64 / sr as f64;
            let f = hz * (1.0 + wobble * (6.28318 * 5.0 * t).sin());
            ph += 6.28318 * f / sr as f64;
            let env = ((t / 0.03).min(1.0)) * (((dur - t) / 0.03).clamp(0.0, 1.0));
            let v = (ph.sin() + 0.5 * (2.0 * ph).sin() + 0.3 * (3.0 * ph).sin() + 0.15 * (4.0 * ph).sin()) * 0.3 * env;
            let s = (v * 30000.0) as i16;
            w.write_sample(s).unwrap();
            w.write_sample(s).unwrap();
        }
        for _ in 0..(0.12 * sr as f64) as usize * 2 {
            w.write_sample(0i16).unwrap();
        }
        t_total += dur + 0.12;
    }
    w.finalize().unwrap();
    t_total
}

#[test]
fn vocal_to_midi_edit_and_render_retunes_the_recording() {
    let mut d = Daw::new();
    call(&mut d, "daw_new", &[]);
    let num = |v: Val| if let Val::Num(x) = v { x } else { panic!() };
    let dir = std::env::temp_dir().join(format!("lsx_vm_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let wav = dir.join("sung.wav");
    // A3 (but a little flat), C4, E4
    let secs = voice_wav(&wav, &[(216.0, 0.7), (261.63, 0.7), (329.63, 0.7)], 0.004);
    assert!(secs > 2.0);
    call(&mut d, "daw_set_project_path", &[s(&dir.join("proj.json").to_string_lossy())]);
    let at = num(call(&mut d, "daw_track_add", &[s("audio"), s("")]));
    let ac = num(call(&mut d, "daw_clip_add_audio", &[n(at), s(&wav.to_string_lossy()), n(0.0)]));
    assert!(ac >= 0.0);
    let mt = num(call(&mut d, "daw_vm_convert", &[n(at), n(ac)]));
    assert!(mt >= 0.0, "converted: {}", num(call(&mut d, "daw_track_count", &[])));
    let Val::List(notes) = call(&mut d, "daw_clip_notes", &[n(mt), n(0.0)]) else { panic!() };
    let stride = notes.len() / 3;
    assert!(notes.len() % 3 == 0 && stride >= 5, "three notes, {} values", notes.len());
    let Val::List(info) = call(&mut d, "daw_vm_info", &[n(mt), n(0.0)]) else { panic!() };
    assert_eq!(info.len(), 14);
    // snap them to C major, then raise the first by a whole tone in the roll and render
    call(&mut d, "daw_vm_set", &[n(mt), n(0.0), s("vdepth"), n(20.0)]);
    call(&mut d, "daw_vm_set", &[n(mt), n(0.0), s("fin"), n(0.05)]);
    let Val::List(rend) = call(&mut d, "daw_vm_render", &[n(mt), n(0.0)]) else { panic!() };
    assert_eq!(num(match &rend[0] { Val::Num(x) => Val::Num(*x), _ => Val::Num(0.0) }), 1.0, "render ok: {:?}", rend.iter().map(|v| if let Val::Str(s) = v { s.clone() } else { String::new() }).collect::<Vec<_>>());
    // a paired audio track now exists and the recording track is muted
    let count = num(call(&mut d, "daw_track_count", &[]));
    assert_eq!(count, 3.0, "audio, MIDI, tuned voice");
    assert_eq!(num(call(&mut d, "daw_track_get", &[n(at), s("mute")])), 1.0);
    // the tuned recording: the first note, 216 Hz when sung, now sits on A3 = 220 Hz
    let tuned = std::fs::read_dir(dir.join("audio")).unwrap().flatten().find(|e| e.file_name().to_string_lossy().starts_with("vocal_")).expect("tuned wav").path();
    let mut r = hound::WavReader::open(&tuned).unwrap();
    let sr = r.spec().sample_rate;
    let xs: Vec<f32> = r.samples::<f32>().map(|x| x.unwrap()).collect();
    let mono: Vec<f32> = xs.chunks(2).map(|c| c[0]).collect();
    let p = ling_daw::vocal::track_pitch(&mono, sr as f32);
    let mut f: Vec<f32> = (40..120).map(|j| p.f0[j]).filter(|x| *x > 0.0).collect();
    f.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = f[f.len() / 2];
    assert!((med - 220.0).abs() < 4.0, "first note retuned to A3, got {med} Hz");
    let _ = std::fs::remove_dir_all(&dir);
}
