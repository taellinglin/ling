//! MIDI hardware input (midir) and Standard MIDI File export (midly).
use crate::engine::SharedEngine;
use crate::model::*;
use midly::num::{u15, u24, u28, u4, u7};
use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;

pub struct MidiIn {
    _conn: midir::MidiInputConnection<()>,
    pub name: String,
}

pub fn list_inputs() -> Vec<String> {
    match midir::MidiInput::new("ling-daw") {
        Ok(m) => m.ports().iter().filter_map(|p| m.port_name(p).ok()).collect(),
        Err(_) => vec![],
    }
}

/// Open input `idx`; incoming notes play (and record into) the target track.
/// `chan`: 0 = all channels, 1..=16 = only that channel.
pub fn open_input(idx: usize, engine: SharedEngine, target: Arc<AtomicUsize>, chan: Arc<AtomicU8>) -> Result<MidiIn, String> {
    let mut m = midir::MidiInput::new("ling-daw").map_err(|e| e.to_string())?;
    m.ignore(midir::Ignore::None);
    let ports = m.ports();
    let port = ports.get(idx).ok_or("no such MIDI port")?;
    let name = m.port_name(port).unwrap_or_default();
    let conn = m
        .connect(
            port,
            "ling-daw-in",
            move |_ts, msg, _| {
                if msg.len() < 3 {
                    return;
                }
                let want = chan.load(Ordering::Relaxed);
                if want != 0 && (msg[0] & 0x0F) + 1 != want {
                    return;
                }
                let (st, key, vel) = (msg[0] & 0xF0, msg[1], msg[2]);
                let on = st == 0x90 && vel > 0;
                let off = st == 0x80 || (st == 0x90 && vel == 0);
                if on || off {
                    if let Ok(mut e) = engine.lock() {
                        e.live_note(target.load(Ordering::Relaxed), key, (vel as f32 / 127.0).max(0.05), on);
                    }
                }
            },
            (),
        )
        .map_err(|e| e.to_string())?;
    Ok(MidiIn { _conn: conn, name })
}

/// The events of one track's MIDI clips: (tick, order, message). Notes, and their per-note expression: a pitch gesture as the
/// channel's pitch wheel (range 24 semitones, set by RPN 0 at the start; the wheel is per channel, so overlapping notes share
/// it), pressure as polyphonic aftertouch on the note's key, timbre as CC74 (MPE slide), each sent every 1/32 beat while it
/// moves. Order inside a tick: note-offs, wheel resets, the next note's first wheel / CC74 value, note-ons, aftertouch.
fn track_midi(t: &Track, ppq: f64, project_len_check: bool) -> Vec<(u32, u8, MidiMessage)> {
    let mut evs: Vec<(u32, u8, MidiMessage)> = vec![];
    let mut any_bend = false;
    for c in &t.clips {
        let ClipData::Midi(notes) = &c.data else { continue };
        for n in notes {
            if project_len_check && n.start >= c.len {
                continue;
            }
            let len = n.len.min(c.len - n.start).max(0.03);
            let s = ((c.start + n.start) * ppq).round() as u32;
            let en = (((c.start + n.start + len) * ppq).round() as u32).max(s + 1);
            evs.push((s, 3, MidiMessage::NoteOn { key: u7::new(n.key), vel: u7::new((n.vel * 127.0).clamp(1.0, 127.0) as u8) }));
            evs.push((en, 0, MidiMessage::NoteOff { key: u7::new(n.key), vel: u7::new(0) }));
            if !n.has_expr() {
                continue;
            }
            // one value per 1/32 beat over the sounding part, sent only when it changes
            let step = (ppq / 8.0).max(1.0) as u32;
            let (mut lb, mut lp, mut lt) = (i32::MIN, -1i32, -1i32);
            let mut tk = s;
            while tk < en {
                let f = ((tk - s) as f64 / ppq / n.len.max(1e-6)) as f32;
                let first = tk == s;
                if !n.bend.is_empty() {
                    any_bend = true;
                    let b = ((crate::gesture::at(&n.bend, f) / 24.0).clamp(-1.0, 1.0) * 8191.0).round() as i32;
                    if b != lb {
                        evs.push((tk, if first { 2 } else { 4 }, MidiMessage::PitchBend { bend: midly::PitchBend::from_int(b as i16) }));
                        lb = b;
                    }
                }
                if !n.press.is_empty() {
                    let p = (crate::gesture::at(&n.press, f).clamp(0.0, 1.0) * 127.0).round() as i32;
                    if p != lp {
                        evs.push((tk, 4, MidiMessage::Aftertouch { key: u7::new(n.key), vel: u7::new(p as u8) }));
                        lp = p;
                    }
                }
                if !n.timbre.is_empty() {
                    let v = (crate::gesture::at(&n.timbre, f).clamp(0.0, 1.0) * 127.0).round() as i32;
                    if v != lt {
                        evs.push((tk, if first { 2 } else { 4 }, MidiMessage::Controller { controller: u7::new(74), value: u7::new(v as u8) }));
                        lt = v;
                    }
                }
                tk += step;
            }
            // the curve's end value on the note's last tick
            if en > s + 1 {
                let last = en - 1;
                if !n.bend.is_empty() {
                    let b = ((crate::gesture::at(&n.bend, 1.0) / 24.0).clamp(-1.0, 1.0) * 8191.0).round() as i32;
                    if b != lb {
                        evs.push((last, 4, MidiMessage::PitchBend { bend: midly::PitchBend::from_int(b as i16) }));
                    }
                }
                if !n.press.is_empty() {
                    let p = (crate::gesture::at(&n.press, 1.0).clamp(0.0, 1.0) * 127.0).round() as i32;
                    if p != lp {
                        evs.push((last, 4, MidiMessage::Aftertouch { key: u7::new(n.key), vel: u7::new(p as u8) }));
                    }
                }
                if !n.timbre.is_empty() {
                    let v = (crate::gesture::at(&n.timbre, 1.0).clamp(0.0, 1.0) * 127.0).round() as i32;
                    if v != lt {
                        evs.push((last, 4, MidiMessage::Controller { controller: u7::new(74), value: u7::new(v as u8) }));
                    }
                }
            }
            if !n.bend.is_empty() {
                evs.push((en, 1, MidiMessage::PitchBend { bend: midly::PitchBend::from_int(0) }));
            }
            if !n.timbre.is_empty() {
                evs.push((en, 1, MidiMessage::Controller { controller: u7::new(74), value: u7::new(64) }));
            }
        }
    }
    if any_bend {
        // RPN 0 (pitch-bend sensitivity) = 24 semitones, then the null RPN
        let rpn: Vec<(u32, u8, MidiMessage)> = [(101u8, 0u8), (100, 0), (6, 24), (38, 0), (101, 127), (100, 127)].iter().map(|(cc, v)| (0, 0, MidiMessage::Controller { controller: u7::new(*cc), value: u7::new(*v) })).collect();
        evs.splice(0..0, rpn);
    }
    // stable: the RPN set-up keeps its order at tick 0
    evs.sort_by_key(|x| (x.0, x.1));
    evs
}

fn to_track_events<'a>(evs: &[(u32, u8, MidiMessage)], chan: u8, mut te: Vec<TrackEvent<'a>>) -> Vec<TrackEvent<'a>> {
    let mut last = 0u32;
    for (tick, _, msg) in evs {
        te.push(TrackEvent { delta: u28::new(tick - last), kind: TrackEventKind::Midi { channel: u4::new(chan), message: *msg } });
        last = *tick;
    }
    te.push(TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::EndOfTrack) });
    te
}

/// Whole-project export: a type-1 SMF with a tempo track and one track per instrument track.
pub fn export_project(e: &crate::engine::Engine, path: &str) -> Result<(), String> {
    const PPQ: f64 = 480.0;
    let mut smf = Smf::new(Header::new(Format::Parallel, Timing::Metrical(u15::new(480))));
    let us_per_beat = (60_000_000.0 / e.project.bpm) as u32;
    smf.tracks.push(vec![
        TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::Tempo(u24::new(us_per_beat))) },
        TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::TrackName(e.project.name.as_bytes())) },
        TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::EndOfTrack) },
    ]);
    let mut chan_no = 0u8;
    for t in e.project.tracks.iter().filter(|t| matches!(t.kind, TrackKind::Instrument)) {
        let evs = track_midi(t, PPQ, true);
        if evs.is_empty() {
            continue;
        }
        let chan = if t.inst_kind == "drums" {
            9
        } else {
            let c = chan_no % 16;
            chan_no += 1;
            if c == 9 { 10 } else { c }
        };
        let te: Vec<TrackEvent> = vec![TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::TrackName(t.name.as_bytes())) }];
        smf.tracks.push(to_track_events(&evs, chan, te));
    }
    let mut buf = vec![];
    smf.write_std(&mut buf).map_err(|x| x.to_string())?;
    std::fs::write(path, buf).map_err(|x| x.to_string())
}

/// Export every MIDI clip of a track as a single-track SMF at 480 PPQ.
pub fn export_track(e: &crate::engine::Engine, track: usize, path: &str) -> Result<(), String> {
    let t = e.project.tracks.get(track).ok_or("no such track")?;
    const PPQ: f64 = 480.0;
    let evs = track_midi(t, PPQ, false);
    let chan = if t.inst_kind == "drums" { 9 } else { 0 };
    let us_per_beat = (60_000_000.0 / e.project.bpm) as u32;
    let te = vec![
        TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::Tempo(u24::new(us_per_beat))) },
        TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::TrackName(t.name.as_bytes())) },
    ];
    let mut smf = Smf::new(Header::new(Format::SingleTrack, Timing::Metrical(u15::new(480))));
    smf.tracks.push(to_track_events(&evs, chan, te));
    let mut buf = vec![];
    smf.write_std(&mut buf).map_err(|x| x.to_string())?;
    std::fs::write(path, buf).map_err(|x| x.to_string())
}
