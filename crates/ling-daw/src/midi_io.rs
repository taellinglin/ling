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
        let mut evs: Vec<(u32, u8, u8, u8)> = vec![];
        for c in &t.clips {
            if let ClipData::Midi(notes) = &c.data {
                for n in notes {
                    if n.start >= c.len {
                        continue;
                    }
                    let s = ((c.start + n.start) * PPQ).round() as u32;
                    let en = ((c.start + n.start + n.len.min(c.len - n.start).max(0.03)) * PPQ).round() as u32;
                    evs.push((s, 1, n.key, (n.vel * 127.0).clamp(1.0, 127.0) as u8));
                    evs.push((en.max(s + 1), 0, n.key, 0));
                }
            }
        }
        if evs.is_empty() {
            continue;
        }
        evs.sort_by_key(|x| (x.0, x.1));
        let chan = if t.inst_kind == "drums" {
            9
        } else {
            let c = chan_no % 16;
            chan_no += 1;
            if c == 9 { 10 } else { c }
        };
        let mut te: Vec<TrackEvent> = vec![TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::TrackName(t.name.as_bytes())) }];
        let mut last = 0u32;
        for (tick, order, key, vel) in &evs {
            let msg = if *order == 1 { MidiMessage::NoteOn { key: u7::new(*key), vel: u7::new(*vel) } } else { MidiMessage::NoteOff { key: u7::new(*key), vel: u7::new(0) } };
            te.push(TrackEvent { delta: u28::new(tick - last), kind: TrackEventKind::Midi { channel: u4::new(chan), message: msg } });
            last = *tick;
        }
        te.push(TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::EndOfTrack) });
        smf.tracks.push(te);
    }
    let mut buf = vec![];
    smf.write_std(&mut buf).map_err(|x| x.to_string())?;
    std::fs::write(path, buf).map_err(|x| x.to_string())
}

/// Export every MIDI clip of a track as a single-track SMF at 480 PPQ.
pub fn export_track(e: &crate::engine::Engine, track: usize, path: &str) -> Result<(), String> {
    let t = e.project.tracks.get(track).ok_or("no such track")?;
    const PPQ: f64 = 480.0;
    let mut evs: Vec<(u32, u8, u8, u8)> = vec![]; // tick, order (0 = off, 1 = on), key, vel
    for c in &t.clips {
        if let ClipData::Midi(notes) = &c.data {
            for n in notes {
                let s = ((c.start + n.start) * PPQ).round() as u32;
                let en = ((c.start + n.start + n.len.min(c.len - n.start).max(0.03)) * PPQ).round() as u32;
                evs.push((s, 1, n.key, (n.vel * 127.0).clamp(1.0, 127.0) as u8));
                evs.push((en.max(s + 1), 0, n.key, 0));
            }
        }
    }
    evs.sort_by_key(|x| (x.0, x.1));
    let chan = if t.inst_kind == "drums" { 9 } else { 0 };
    let mut track_events: Vec<TrackEvent> = vec![];
    let us_per_beat = (60_000_000.0 / e.project.bpm) as u32;
    track_events.push(TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::Tempo(u24::new(us_per_beat))) });
    track_events.push(TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::TrackName(t.name.as_bytes())) });
    let mut last = 0u32;
    for (tick, order, key, vel) in &evs {
        let msg = if *order == 1 {
            MidiMessage::NoteOn { key: u7::new(*key), vel: u7::new(*vel) }
        } else {
            MidiMessage::NoteOff { key: u7::new(*key), vel: u7::new(0) }
        };
        track_events.push(TrackEvent { delta: u28::new(tick - last), kind: TrackEventKind::Midi { channel: u4::new(chan), message: msg } });
        last = *tick;
    }
    track_events.push(TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::EndOfTrack) });
    let mut smf = Smf::new(Header::new(Format::SingleTrack, Timing::Metrical(u15::new(480))));
    smf.tracks.push(track_events);
    let mut buf = vec![];
    smf.write_std(&mut buf).map_err(|x| x.to_string())?;
    std::fs::write(path, buf).map_err(|x| x.to_string())
}
