use ling_daw::{Daw, Val};
use midly::{num::*, Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};

fn num(v: Option<Result<Val, String>>) -> f64 {
    match v {
        Some(Ok(Val::Num(x))) => x,
        other => panic!("unexpected {other:?}"),
    }
}
fn text(v: Option<Result<Val, String>>) -> String {
    match v {
        Some(Ok(Val::Str(s))) => s,
        other => panic!("unexpected {other:?}"),
    }
}
fn font() -> Option<String> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../LingStationX/assets/soundfonts/Ling.sf2");
    p.exists().then(|| p.to_string_lossy().into_owned())
}
fn ev(delta: u32, kind: TrackEventKind<'static>) -> TrackEvent<'static> {
    TrackEvent { delta: u28::new(delta), kind }
}
fn midi(ch: u8, m: MidiMessage) -> TrackEventKind<'static> {
    TrackEventKind::Midi { channel: u4::new(ch), message: m }
}

#[test]
fn midi_import_uses_soundfont_banks_and_renders_audio() {
    let Some(sf) = font() else { return };
    let mut smf = Smf::new(Header::new(Format::Parallel, Timing::Metrical(u15::new(480))));
    smf.tracks.push(vec![ev(0, TrackEventKind::Meta(MetaMessage::Tempo(u24::new(500_000)))), ev(0, TrackEventKind::Meta(MetaMessage::EndOfTrack))]);
    smf.tracks.push(vec![
        ev(0, midi(0, MidiMessage::Controller { controller: u7::new(0), value: u7::new(1) })), // bank 1
        ev(0, midi(0, MidiMessage::ProgramChange { program: u7::new(1) })),                     // PIANO
        ev(0, midi(0, MidiMessage::NoteOn { key: u7::new(60), vel: u7::new(100) })),
        ev(960, midi(0, MidiMessage::NoteOff { key: u7::new(60), vel: u7::new(0) })),
        ev(0, TrackEventKind::Meta(MetaMessage::EndOfTrack)),
    ]);
    let mut bytes = Vec::new();
    smf.write(&mut bytes).unwrap();
    let dir = std::env::temp_dir().join("ling_sf2_import");
    std::fs::create_dir_all(&dir).unwrap();
    let mid = dir.join("t.mid");
    std::fs::write(&mid, &bytes).unwrap();

    let mut d = Daw::new();
    assert!(num(d.call("daw_gm_sf2_set", &[Val::Str(sf.clone())])) > 100.0);
    assert_eq!(num(d.call("daw_midi_import", &[Val::Str(mid.to_string_lossy().into()), Val::Num(0.0)])), 1.0);
    assert_eq!(text(d.call("daw_track_kind", &[Val::Num(0.0)])), "sf2");
    let names = d.call("daw_sf_presets", &[Val::Num(0.0)]);
    let cur = num(d.call("daw_sf_preset", &[Val::Num(0.0)])) as usize;
    match names {
        Some(Ok(Val::List(l))) => assert!(matches!(&l[cur], Val::Str(s) if s.contains("PIANO")), "{:?}", l[cur]),
        _ => panic!(),
    }
    // the project round-trips with its soundfont track
    let proj = dir.join("p.json");
    assert_eq!(num(d.call("daw_project_save", &[Val::Str(proj.to_string_lossy().into())])), 1.0);
    let mut d2 = Daw::new();
    assert_eq!(num(d2.call("daw_project_load", &[Val::Str(proj.to_string_lossy().into())])), 1.0);
    assert_eq!(text(d2.call("daw_track_kind", &[Val::Num(0.0)])), "sf2");
    assert_eq!(num(d2.call("daw_sf_preset", &[Val::Num(0.0)])) as usize, cur);
    // and renders audible sound
    let out = dir.join("out.wav");
    let args = [Val::Str(out.to_string_lossy().into()), Val::Num(0.0), Val::Num(4.0), Val::Num(0.5), Val::Num(16.0), Val::Str("wav".into()), Val::Num(0.5)];
    assert_eq!(num(d2.call("daw_render_start", &args)), 1.0);
    for _ in 0..4000 {
        if num(d2.call("daw_render_state", &[])) >= 2.0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(num(d2.call("daw_render_state", &[])), 2.0);
    let mut r = hound::WavReader::open(&out).unwrap();
    let peak = r.samples::<i16>().map(|s| s.unwrap().unsigned_abs()).max().unwrap();
    assert!(peak > 1500, "rendered piano is too quiet: {peak}");
}
