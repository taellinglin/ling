//! Saving a project packs everything it needs into its folder, and the folder opens again unchanged.
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
fn list(v: Option<Result<Val, String>>) -> Vec<Val> {
    match v {
        Some(Ok(Val::List(l))) => l,
        other => panic!("expected a list, got {other:?}"),
    }
}

fn write_wav(path: &std::path::Path) {
    let spec = hound::WavSpec { channels: 2, sample_rate: 44100, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for i in 0..44100 {
        let v = ((i as f32 * 0.05).sin() * 8000.0) as i16;
        w.write_sample(v).unwrap();
        w.write_sample(v).unwrap();
    }
    w.finalize().unwrap();
}

#[test]
fn a_saved_project_is_self_contained_and_reopens_identically() {
    let root = std::env::temp_dir().join(format!("lsx_pack_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    // a "recording" that lives outside the project (like a take saved to the temp folder)
    let take = root.join("elsewhere").join("take_1.wav");
    std::fs::create_dir_all(take.parent().unwrap()).unwrap();
    write_wav(&take);

    let mut d = Daw::new();
    d.call("daw_new", &[]);
    let vox = num(d.call("daw_track_add", &[s("voice"), s("")])) as usize;
    let growl = num(d.call("daw_track_add", &[s("growl"), s("")])) as usize;
    let aud = num(d.call("daw_track_add", &[s("audio"), s("")])) as usize;
    let c = num(d.call("daw_clip_add_midi", &[n(vox as f64), n(0.0), n(4.0)])) as usize;
    for k in 0..3 {
        d.call("daw_note_add", &[n(vox as f64), n(c as f64), n(k as f64), n(1.0), n(60.0 + k as f64), n(0.8)]);
    }
    d.call("daw_lyric_apply", &[n(vox as f64), n(c as f64), s("[so-fa]-[rei]"), s("auto")]);
    let gc = num(d.call("daw_clip_add_midi", &[n(growl as f64), n(0.0), n(4.0)])) as usize;
    d.call("daw_note_add", &[n(growl as f64), n(gc as f64), n(0.0), n(2.0), n(36.0), n(0.9)]);
    d.call("daw_inst_set", &[n(growl as f64), s("cut"), n(0.77)]);
    num(d.call("daw_clip_add_audio", &[n(aud as f64), s(&take.to_string_lossy()), n(0.0)]));
    d.call("daw_auto_lane_add", &[n(growl as f64), s("cut")]);
    d.call("daw_auto_set_points", &[n(growl as f64), n(0.0), Val::List(vec![n(0.0), n(0.2), n(2.0), n(0.9)])]);
    d.call("daw_set_project_name", &[s("Pack Test")]);

    let proj = root.join("Pack Test").join("project.json");
    assert_eq!(num(d.call("daw_project_save", &[s(&proj.to_string_lossy())])), 1.0);
    let dir = root.join("Pack Test");
    assert!(dir.join("project.json").is_file());
    assert!(!dir.join("project.json.tmp").exists(), "no half-written leftovers");
    // recordings are copied in and referenced relatively
    let json = std::fs::read_to_string(dir.join("project.json")).unwrap();
    assert!(json.contains("audio/take_1.wav"), "audio path must be relative: {json}");
    assert!(dir.join("audio").join("take_1.wav").is_file());
    // MIDI and patch copies
    assert!(dir.join("midi").join("project.mid").is_file());
    let mids: Vec<_> = std::fs::read_dir(dir.join("midi")).unwrap().flatten().map(|f| f.file_name().to_string_lossy().into_owned()).collect();
    assert!(mids.iter().any(|m| m.starts_with("001 ")), "{mids:?}");
    let pats: Vec<_> = std::fs::read_dir(dir.join("patches")).unwrap().flatten().map(|f| f.file_name().to_string_lossy().into_owned()).collect();
    assert!(pats.iter().any(|p| p.contains("growl") || p.starts_with("002")), "{pats:?}");

    // delete the original recording: the project must still open from its own folder
    std::fs::remove_dir_all(root.join("elsewhere")).unwrap();
    let mut e = Daw::new();
    assert_eq!(num(e.call("daw_project_load", &[s(&proj.to_string_lossy())])), 1.0);
    assert_eq!(num(e.call("daw_track_count", &[])), 3.0);
    let notes = list(e.call("daw_note_lyrics", &[n(vox as f64), n(c as f64)]));
    let notes: Vec<String> = notes.into_iter().map(|v| if let Val::Str(x) = v { x } else { String::new() }).collect();
    assert_eq!(notes, vec!["so-fa", "~ɾei", ""], "lyrics survive the save");
    assert!((num(e.call("daw_inst_get", &[n(growl as f64), s("cut")])) - 0.77).abs() < 1e-3, "instrument state survives");
    assert_eq!(num(e.call("daw_clip_count", &[n(aud as f64)])), 1.0, "the recording is found in the project's audio folder");
    assert!(num(e.call("daw_clip_get", &[n(aud as f64), n(0.0), s("seconds")])) > 0.9);
    let pts = list(e.call("daw_auto_points", &[n(growl as f64), n(0.0)]));
    assert_eq!(pts.len(), 4, "automation survives");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_track_can_swap_plugins_and_keeps_its_clips() {
    let mut d = Daw::new();
    d.call("daw_new", &[]);
    let t = num(d.call("daw_track_add", &[s("synth"), s("")]));
    let c = num(d.call("daw_clip_add_midi", &[n(t), n(0.0), n(4.0)]));
    d.call("daw_note_add", &[n(t), n(c), n(0.0), n(1.0), n(48.0), n(0.9)]);
    for kind in ["growl", "voice", "drums", "synth"] {
        assert_eq!(num(d.call("daw_track_set_kind", &[n(t), s(kind)])), 1.0, "{kind}");
        match d.call("daw_track_kind", &[n(t)]) {
            Some(Ok(Val::Str(k))) => assert_eq!(k, kind),
            other => panic!("{other:?}"),
        }
        assert_eq!(num(d.call("daw_clip_get", &[n(t), n(c), s("notes")])), 1.0, "notes survive the swap to {kind}");
    }
    assert_eq!(num(d.call("daw_track_set_kind", &[n(t), s("nonsense")])), 0.0);
    let a = num(d.call("daw_track_add", &[s("audio"), s("")]));
    assert_eq!(num(d.call("daw_track_set_kind", &[n(a), s("growl")])), 0.0, "audio tracks have no instrument");
}
