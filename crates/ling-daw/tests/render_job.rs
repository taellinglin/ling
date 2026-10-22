use ling_daw::{Daw, Val};

fn num(v: Option<Result<Val, String>>) -> f64 {
    match v {
        Some(Ok(Val::Num(x))) => x,
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn background_render_reports_progress_and_writes_file() {
    let mut d = Daw::new();
    let t = num(d.call("daw_track_add", &[Val::Str("synth".into()), Val::Str(String::new())]));
    assert!(t >= 0.0);
    let c = num(d.call("daw_clip_add_midi", &[Val::Num(t), Val::Num(0.0), Val::Num(4.0)]));
    assert!(c >= 0.0);
    d.call("daw_note_add", &[Val::Num(t), Val::Num(c), Val::Num(0.0), Val::Num(1.0), Val::Num(60.0), Val::Num(0.9)]);
    let path = std::env::temp_dir().join("ling_render_job_test").join("out.wav");
    let _ = std::fs::remove_file(&path);
    let args = [Val::Str(path.to_string_lossy().into()), Val::Num(0.0), Val::Num(4.0), Val::Num(0.5), Val::Num(16.0), Val::Str("wav".into()), Val::Num(0.5)];
    assert_eq!(num(d.call("daw_render_start", &args)), 1.0);
    // a second start while running is refused (or already finished: state 2)
    let mut last = 0.0;
    let mut seen_running = false;
    for _ in 0..4000 {
        let st = num(d.call("daw_render_state", &[]));
        let p = num(d.call("daw_render_progress", &[]));
        assert!(p >= last - 1e-9, "progress went backwards");
        last = p;
        if st == 1.0 {
            seen_running = true;
        }
        if st >= 2.0 {
            assert_eq!(st, 2.0, "render failed");
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let _ = seen_running;
    assert!((num(d.call("daw_render_progress", &[])) - 1.0).abs() < 1e-9);
    let len = std::fs::metadata(&path).expect("file written").len();
    assert!(len > 10_000, "wav too small: {len}");
    d.call("daw_render_ack", &[]);
    assert_eq!(num(d.call("daw_render_state", &[])), 0.0);
}

#[test]
fn stems_write_one_file_per_track() {
    let mut d = Daw::new();
    for _ in 0..2 {
        let t = num(d.call("daw_track_add", &[Val::Str("synth".into()), Val::Str(String::new())]));
        let c = num(d.call("daw_clip_add_midi", &[Val::Num(t), Val::Num(0.0), Val::Num(4.0)]));
        d.call("daw_note_add", &[Val::Num(t), Val::Num(c), Val::Num(0.0), Val::Num(1.0), Val::Num(60.0 + t * 7.0), Val::Num(0.9)]);
    }
    let dir = std::env::temp_dir().join("ling_stems_test");
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("mix.wav");
    let args = [Val::Str(path.to_string_lossy().into()), Val::Num(0.0), Val::Num(4.0), Val::Num(0.2), Val::Num(16.0), Val::Str("wav".into()), Val::Num(0.5), Val::Num(1.0)];
    assert_eq!(num(d.call("daw_render_start", &args)), 1.0);
    for _ in 0..4000 {
        if num(d.call("daw_render_state", &[])) >= 2.0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(num(d.call("daw_render_state", &[])), 2.0);
    let files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(files.len(), 2, "{files:?}");
    assert!(files.iter().all(|f| f.starts_with("mix_0") && f.ends_with(".wav")));
    for f in &files {
        assert!(std::fs::metadata(dir.join(f)).unwrap().len() > 10_000);
    }
}
