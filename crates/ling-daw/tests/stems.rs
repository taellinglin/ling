//! Stem export renders every track to its own file, several at once.
use ling_daw::{Daw, Val};

fn n(x: f64) -> Val {
    Val::Num(x)
}
fn s(x: &str) -> Val {
    Val::Str(x.into())
}
fn num(v: Option<Result<Val, String>>) -> f64 {
    match v {
        Some(Ok(Val::Num(x))) => x,
        other => panic!("expected a number, got {other:?}"),
    }
}

#[test]
fn stems_render_in_parallel_and_each_has_its_own_sound() {
    let dir = std::env::temp_dir().join(format!("lsx_stems_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut d = Daw::new();
    d.call("daw_new", &[]);
    let kinds = ["synth", "growl", "voice", "drums", "synth", "growl"];
    for (i, k) in kinds.iter().enumerate() {
        let t = num(d.call("daw_track_add", &[s(k), s("")])) as usize;
        let c = num(d.call("daw_clip_add_midi", &[n(t as f64), n(0.0), n(4.0)])) as usize;
        for b in 0..4 {
            d.call("daw_note_add", &[n(t as f64), n(c as f64), n(b as f64), n(0.8), n(40.0 + (i * 4 + b) as f64), n(0.9)]);
        }
        d.call("daw_track_set_name", &[n(t as f64), s(&format!("Track {i}"))]);
    }
    let out = dir.join("mix.wav");
    // (path, start, end, tail s, bits, format, quality, stems)
    assert_eq!(num(d.call("daw_render_start", &[s(&out.to_string_lossy()), n(0.0), n(4.0), n(0.5), n(16.0), s("wav"), n(0.5), n(1.0)])), 1.0);
    let t0 = std::time::Instant::now();
    loop {
        let st = num(d.call("daw_render_state", &[]));
        if st != 1.0 {
            assert_eq!(st, 2.0, "render must finish");
            break;
        }
        assert!(t0.elapsed().as_secs() < 120, "render took too long");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|f| f.path()).collect();
    assert_eq!(files.len(), 6, "one stem per track: {files:?}");
    for f in &files {
        let mut r = hound::WavReader::open(f).unwrap();
        let peak = r.samples::<i16>().map(|x| x.unwrap().unsigned_abs()).max().unwrap_or(0);
        assert!(peak > 300, "{} is silent (peak {peak})", f.display());
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bundle_has_stems_midi_params_and_a_readable_dawproject() {
    let dir = std::env::temp_dir().join(format!("lsx_bundle_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut d = Daw::new();
    d.call("daw_new", &[]);
    for (i, k) in ["synth", "drums"].iter().enumerate() {
        let t = num(d.call("daw_track_add", &[s(k), s("")])) as usize;
        let c = num(d.call("daw_clip_add_midi", &[n(t as f64), n(0.0), n(4.0)])) as usize;
        for b in 0..4 {
            d.call("daw_note_add", &[n(t as f64), n(c as f64), n(b as f64), n(0.8), n(36.0 + (i * 8 + b) as f64), n(0.9)]);
        }
        d.call("daw_track_set_name", &[n(t as f64), s(&format!("Part {i}"))]);
    }
    let out = dir.join("song.wav");
    // bundle flag (9th argument) renders stems on its own
    assert_eq!(num(d.call("daw_render_start", &[s(&out.to_string_lossy()), n(0.0), n(4.0), n(0.5), n(16.0), s("wav"), n(0.5), n(0.0), n(1.0)])), 1.0);
    let t0 = std::time::Instant::now();
    loop {
        let st = num(d.call("daw_render_state", &[]));
        if st != 1.0 {
            assert_eq!(st, 2.0, "bundle must finish");
            break;
        }
        assert!(t0.elapsed().as_secs() < 120);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let root = dir.join("song bundle");
    for f in ["README.txt", "params.json", "project.dawproject", "midi/project.mid", "midi/01 Part 0.mid", "midi/02 Part 1.mid"] {
        assert!(root.join(f).is_file(), "{f} missing");
    }
    assert_eq!(std::fs::read_dir(root.join("audio")).unwrap().count(), 2);
    let params: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(root.join("params.json")).unwrap()).unwrap();
    assert_eq!(params["tracks"].as_array().unwrap().len(), 2);
    assert!(params["tracks"][0]["stem"].as_str().unwrap().starts_with("audio/"));
    // the MIDI file reads back with the right notes
    let smf_bytes = std::fs::read(root.join("midi/project.mid")).unwrap();
    let smf = midly::Smf::parse(&smf_bytes).unwrap();
    let ons = smf.tracks.iter().flatten().filter(|e| matches!(e.kind, midly::TrackEventKind::Midi { message: midly::MidiMessage::NoteOn { .. }, .. })).count();
    assert_eq!(ons, 8);
    // zip: local headers and the central directory agree on the entries
    let z = std::fs::read(root.join("project.dawproject")).unwrap();
    let _ = std::fs::write(std::env::temp_dir().join("lsx_last.dawproject"), &z);
    assert_eq!(&z[..4], b"PK\x03\x04");
    let eocd = z.len() - 22;
    assert_eq!(&z[eocd..eocd + 4], b"PK\x05\x06");
    assert_eq!(u16::from_le_bytes([z[eocd + 10], z[eocd + 11]]), 4, "metadata, project, 2 stems");
    let text = String::from_utf8_lossy(&z);
    assert!(text.contains("<Project version=\"1.0\">") && text.contains("contentType=\"notes\"") && text.contains("audio/song_01_Part 0.wav"));
    let _ = std::fs::remove_dir_all(&dir);
}
