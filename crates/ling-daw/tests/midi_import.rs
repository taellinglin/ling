use ling_daw::midi_import::{gm_patch, parse};
use midly::{num::*, Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};

fn ev(delta: u32, kind: TrackEventKind<'static>) -> TrackEvent<'static> {
    TrackEvent { delta: u28::new(delta), kind }
}
fn midi(ch: u8, m: MidiMessage) -> TrackEventKind<'static> {
    TrackEventKind::Midi { channel: u4::new(ch), message: m }
}
fn note_on(ch: u8, k: u8) -> TrackEventKind<'static> {
    midi(ch, MidiMessage::NoteOn { key: u7::new(k), vel: u7::new(100) })
}
fn note_off(ch: u8, k: u8) -> TrackEventKind<'static> {
    midi(ch, MidiMessage::NoteOff { key: u7::new(k), vel: u7::new(0) })
}

#[test]
fn tracks_split_with_gm_programs_and_drums() {
    let mut smf = Smf::new(Header::new(Format::Parallel, Timing::Metrical(u15::new(480))));
    // track 0: tempo only
    smf.tracks.push(vec![ev(0, TrackEventKind::Meta(MetaMessage::Tempo(u24::new(500_000)))), ev(0, TrackEventKind::Meta(MetaMessage::EndOfTrack))]);
    // track 1: strings (program 48) on channel 0
    smf.tracks.push(vec![
        ev(0, TrackEventKind::Meta(MetaMessage::TrackName(b"Pad"))),
        ev(0, midi(0, MidiMessage::ProgramChange { program: u7::new(48) })),
        ev(0, note_on(0, 60)),
        ev(480, note_off(0, 60)),
        ev(0, TrackEventKind::Meta(MetaMessage::EndOfTrack)),
    ]);
    // track 2: drums on channel 10 (index 9) and bass (program 33) on channel 1
    smf.tracks.push(vec![
        ev(0, midi(1, MidiMessage::ProgramChange { program: u7::new(33) })),
        ev(0, note_on(9, 36)),
        ev(0, note_on(1, 40)),
        ev(240, note_off(9, 36)),
        ev(240, note_off(1, 40)),
        ev(0, TrackEventKind::Meta(MetaMessage::EndOfTrack)),
    ]);
    let mut bytes = Vec::new();
    smf.write(&mut bytes).unwrap();
    let m = parse(&bytes).unwrap();
    assert_eq!(m.bpm, Some(120.0));
    assert_eq!(m.tracks.len(), 3);
    assert_eq!(m.tracks[0].name, "Pad");
    assert_eq!(m.tracks[0].program, Some(48));
    assert!((m.tracks[0].notes[0].len - 1.0).abs() < 1e-9);
    let drums = m.tracks.iter().find(|t| t.program.is_none()).expect("drum track");
    assert_eq!(drums.notes[0].key, 36);
    let bass = m.tracks.iter().find(|t| t.program == Some(33)).expect("bass track");
    assert!(bass.name.contains("Finger Bass"));
    // patches are complete and distinct per family
    assert_ne!(gm_patch(0).params, gm_patch(33).params);
    for p in 0..128u8 {
        let g = gm_patch(p);
        assert_eq!(g.kind, "synth");
        assert!(g.params.len() > 20, "patch {p} incomplete");
        assert!(g.params.iter().all(|(_, v)| (0.0..=1.0).contains(v)), "patch {p} out of range");
    }
}
