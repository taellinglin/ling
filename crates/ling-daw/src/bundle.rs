//! "Export stems" bundle: everything another DAW (FL Studio, Ableton Live, Bitwig, Studio One, Cubase, Logic ...) needs to rebuild a song.
//!
//! The audio stems themselves are rendered by the render job (one file per track, all starting at the same point, so dropping them at bar 1
//! lines them up). This module writes the rest into the same folder:
//!
//! * `midi/project.mid` (every track, with the tempo map) and `midi/NN name.mid` (one track each) - notes shifted so the range start is time 0;
//! * `instruments/` - the SoundFonts the song plays, and the Ling patch of each synth (its full parameter state and effect chain);
//! * `params.json` - tempo, every track's volume / pan / mute, instrument state, effect chains with parameters, automation lanes, routing;
//! * `project.dawproject` - the open DAWproject format (Bitwig Studio, Studio One, Cubase 14, ...): the stems as audio tracks, plus the notes as
//!   muted note tracks, tempo, levels and pans;
//! * `README.txt` - how to bring it into the common DAWs.
use crate::engine::Engine;
use crate::model::*;
use midly::{num::*, Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};
use serde_json::{json, Value};
use std::path::Path;

pub struct Stem {
    pub track: usize,
    /// path relative to the bundle folder, forward slashes
    pub file: String,
}

fn safe(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == ' ' || c == '_' { c } else { '_' }).collect();
    let t = t.trim().to_string();
    if t.is_empty() { "track".into() } else { t }
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

// ── zip (stored) ──────────────────────────────────────────────────────────────

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for i in 0..256u32 {
        let mut c = i;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB88320 ^ (c >> 1) } else { c >> 1 };
        }
        table[i as usize] = c;
    }
    let mut c = 0xFFFF_FFFFu32;
    for b in data {
        c = table[((c ^ *b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

/// A zip archive with the files stored uncompressed (what DAWproject readers need; the audio is already compact enough).
pub fn zip_stored(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out: Vec<u8> = vec![];
    let mut central: Vec<u8> = vec![];
    for (name, data) in files {
        let off = out.len() as u32;
        let crc = crc32(data);
        let nb = name.as_bytes();
        out.extend(0x04034b50u32.to_le_bytes());
        out.extend([20, 0, 0, 0x08, 0, 0, 0, 0, 0x21, 0]); // version, flags (utf-8), method 0, time, date 1980-01-01
        out.extend(crc.to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend((nb.len() as u16).to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(nb);
        out.extend(data);
        central.extend(0x02014b50u32.to_le_bytes());
        central.extend([20, 0, 20, 0, 0, 0x08, 0, 0, 0, 0, 0x21, 0]);
        central.extend(crc.to_le_bytes());
        central.extend((data.len() as u32).to_le_bytes());
        central.extend((data.len() as u32).to_le_bytes());
        central.extend((nb.len() as u16).to_le_bytes());
        central.extend([0u8; 12]); // extra, comment, disk, internal attrs, external attrs (4)
        central.extend(off.to_le_bytes());
        central.extend(nb);
    }
    let cd_off = out.len() as u32;
    let cd_len = central.len() as u32;
    out.extend(central);
    out.extend(0x06054b50u32.to_le_bytes());
    out.extend([0u8; 4]);
    out.extend((files.len() as u16).to_le_bytes());
    out.extend((files.len() as u16).to_le_bytes());
    out.extend(cd_len.to_le_bytes());
    out.extend(cd_off.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

// ── MIDI ──────────────────────────────────────────────────────────────────────

const PPQ: f64 = 480.0;

fn track_events(t: &Track, shift: f64, chan: u8) -> Vec<(u32, u8, u8, u8)> {
    let mut evs: Vec<(u32, u8, u8, u8)> = vec![];
    for c in &t.clips {
        let ClipData::Midi(notes) = &c.data else { continue };
        let ll = if c.loop_len >= 0.25 && c.loop_len < c.len { c.loop_len } else { 0.0 };
        let mut put = |base: f64, limit: f64| {
            for n in notes.iter().filter(|n| n.start < limit) {
                let s = base + n.start - shift;
                let e = s + n.len.min(limit - n.start).max(0.03);
                if e <= 0.0 {
                    continue;
                }
                let (s, e) = ((s.max(0.0) * PPQ).round() as u32, (e * PPQ).round() as u32);
                evs.push((s, 1, n.key, (n.vel * 127.0).clamp(1.0, 127.0) as u8));
                evs.push((e.max(s + 1), 0, n.key, 0));
            }
        };
        if ll > 0.0 {
            let mut off = 0.0;
            while off < c.len - 1e-9 {
                put(c.start + off, ll.min(c.len - off));
                off += ll;
            }
        } else {
            put(c.start, c.len);
        }
    }
    let _ = chan;
    evs.sort_by_key(|x| (x.0, x.1));
    evs
}

fn smf_track(name: &str, evs: &[(u32, u8, u8, u8)], chan: u8) -> Vec<TrackEvent<'static>> {
    // the name outlives the call: leak-free by boxing into a static-lifetime copy is not possible, so names are written by the caller's buffer
    let _ = name;
    let mut te: Vec<TrackEvent<'static>> = vec![];
    let mut last = 0u32;
    for (tick, order, key, vel) in evs {
        let msg = if *order == 1 { MidiMessage::NoteOn { key: u7::new(*key), vel: u7::new(*vel) } } else { MidiMessage::NoteOff { key: u7::new(*key), vel: u7::new(0) } };
        te.push(TrackEvent { delta: u28::new(tick - last), kind: TrackEventKind::Midi { channel: u4::new(chan), message: msg } });
        last = *tick;
    }
    te.push(TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::EndOfTrack) });
    te
}

fn tempo_events(e: &Engine, shift: f64) -> Vec<(u32, u32)> {
    // (tick, microseconds per beat) with the tempo in force at the range start first
    let p = &e.project;
    let mut bpm = p.bpm;
    let mut out = vec![];
    for (b, v) in &p.tempo_map {
        if *b <= shift {
            bpm = *v;
        }
    }
    out.push((0u32, (60_000_000.0 / bpm.clamp(20.0, 400.0)) as u32));
    for (b, v) in &p.tempo_map {
        if *b > shift {
            out.push((((b - shift) * PPQ).round() as u32, (60_000_000.0 / v.clamp(20.0, 400.0)) as u32));
        }
    }
    out
}

fn write_smf(path: &Path, e: &Engine, shift: f64, tracks: &[usize], single: bool) -> Result<bool, String> {
    let mut smf = Smf::new(Header::new(if single { Format::SingleTrack } else { Format::Parallel }, Timing::Metrical(u15::new(480))));
    // the names must live as long as the Smf: keep them here
    let names: Vec<String> = tracks.iter().map(|i| e.project.tracks[*i].name.clone()).collect();
    let pname = e.project.name.clone();
    let tempos = tempo_events(e, shift);
    let mut conductor: Vec<TrackEvent> = vec![];
    let mut last = 0u32;
    for (tick, us) in &tempos {
        conductor.push(TrackEvent { delta: u28::new(tick - last), kind: TrackEventKind::Meta(MetaMessage::Tempo(u24::new(*us))) });
        last = *tick;
    }
    conductor.insert(0, TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::TrackName(pname.as_bytes())) });
    conductor.push(TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::EndOfTrack) });
    let mut any = false;
    let mut body: Vec<Vec<TrackEvent>> = vec![];
    let mut chan_no = 0u8;
    for (k, &ti) in tracks.iter().enumerate() {
        let t = &e.project.tracks[ti];
        let evs = track_events(t, shift, 0);
        if evs.is_empty() {
            continue;
        }
        any = true;
        let chan = if t.inst_kind == "drums" {
            9
        } else {
            let c = chan_no % 16;
            chan_no += 1;
            if c == 9 { 10 } else { c }
        };
        let mut te = vec![TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::TrackName(names[k].as_bytes())) }];
        let mut tail = smf_track(&names[k], &evs, chan);
        te.append(&mut tail);
        body.push(te);
    }
    if !any {
        return Ok(false);
    }
    if single {
        // one track: tempo + notes merged is not needed for a single instrument - keep the tempo events in front
        let mut te = conductor;
        te.pop();
        for b in body.into_iter().take(1) {
            te.extend(b);
        }
        smf.tracks.push(te);
    } else {
        smf.tracks.push(conductor);
        smf.tracks.extend(body);
    }
    let mut buf = vec![];
    smf.write_std(&mut buf).map_err(|x| x.to_string())?;
    std::fs::write(path, buf).map_err(|x| format!("{}: {x}", path.display()))?;
    Ok(true)
}

// ── params.json ───────────────────────────────────────────────────────────────

fn lane_json(l: &AutoLane, shift: f64) -> Value {
    json!({ "param": l.param, "points": l.points.iter().map(|(b, v)| json!([b - shift, v])).collect::<Vec<_>>() })
}

fn fx_json(f: &EffectSlot) -> Value {
    let mut params = serde_json::Map::new();
    for (n, v) in &f.params {
        params.insert(n.to_string(), json!(v));
    }
    json!({ "name": f.name, "bypass": f.bypass, "params": params })
}

fn build_params(e: &Engine, shift: f64, end: f64, stems: &[Stem], midi: &[(usize, String)], patches: &[(usize, String)]) -> Value {
    let p = &e.project;
    let tracks: Vec<Value> = p
        .tracks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let state: Vec<Value> = t.inst.as_ref().map(|x| x.state().into_iter().map(|(n, v)| json!([n, v])).collect()).unwrap_or_default();
            json!({
                "index": i + 1,
                "name": t.name,
                "kind": match t.kind { TrackKind::Instrument => "instrument", TrackKind::Audio => "audio" },
                "instrument": t.inst_kind,
                "soundfont": if t.inst_kind == "sf2" { Value::String(t.patch.clone()) } else { Value::Null },
                "volume": t.volume,
                "pan": t.pan,
                "mute": t.mute,
                "solo": t.solo,
                "stem": stems.iter().find(|s| s.track == i).map(|s| s.file.clone()),
                "midi": midi.iter().find(|m| m.0 == i).map(|m| m.1.clone()),
                "patch": patches.iter().find(|m| m.0 == i).map(|m| m.1.clone()),
                "instrument_state": state,
                "effects": t.effects.iter().map(fx_json).collect::<Vec<_>>(),
                "automation": t.auto.iter().map(|l| lane_json(l, shift)).collect::<Vec<_>>(),
            })
        })
        .collect();
    let routes: Vec<Value> = p
        .routes
        .iter()
        .map(|r| json!({ "kind": match r.kind { RouteKind::Send => "send", RouteKind::Sidechain => "sidechain", RouteKind::Duck => "duck", RouteKind::Mod => "mod" }, "from": r.from + 1, "to": r.to + 1, "amount": r.amount, "keys": r.keys }))
        .collect();
    json!({
        "format": "LingStation stems bundle 1",
        "project": p.name,
        "bpm": p.bpm,
        "tempo_map": p.tempo_map.iter().map(|(b, v)| json!([b - shift, v])).collect::<Vec<_>>(),
        "time_signature": [4, 4],
        "range_beats": [shift, end],
        "master": { "volume": p.master_volume, "limiter": p.limiter, "effects": p.master_effects.iter().map(fx_json).collect::<Vec<_>>(), "automation": p.master_auto.iter().map(|l| lane_json(l, shift)).collect::<Vec<_>>() },
        "tracks": tracks,
        "routes": routes,
        "note": "Stems are rendered through each track's own effects (not the master chain); volume and pan are already in the stems unless the stem is dry.",
    })
}

// ── DAWproject ────────────────────────────────────────────────────────────────

fn dawproject(e: &Engine, shift: f64, end: f64, tail: f64, sr: u32, stems: &[Stem], audio: &[(String, Vec<u8>)]) -> Vec<(String, Vec<u8>)> {
    let p = &e.project;
    let mut s = String::new();
    s.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Project version=\"1.0\">\n  <Application name=\"LingStation\" version=\"1.0\"/>\n");
    s.push_str(&format!("  <Transport>\n    <Tempo unit=\"bpm\" value=\"{:.4}\" min=\"20\" max=\"666\"/>\n    <TimeSignature numerator=\"4\" denominator=\"4\"/>\n  </Transport>\n", {
        let mut b = p.bpm;
        for (bt, v) in &p.tempo_map {
            if *bt <= shift {
                b = *v;
            }
        }
        b
    }));
    s.push_str("  <Structure>\n");
    let mut lanes = String::new();
    let len_beats = (end - shift).max(1.0);
    let secs = (len_beats * 60.0 / p.bpm) + tail;
    let mut id = 0;
    let mut nid = || {
        id += 1;
        format!("id{id}")
    };
    let master = "master";
    let mut parts = String::new();
    for (i, t) in p.tracks.iter().enumerate() {
        let col = format!("#{:06x}", t.color & 0xFFFFFF);
        if let Some(st) = stems.iter().find(|x| x.track == i) {
            let (tid, cid) = (nid(), nid());
            parts.push_str(&format!(
                "    <Track contentType=\"audio\" loaded=\"true\" id=\"{tid}\" name=\"{}\" color=\"{col}\">\n      <Channel role=\"regular\" audioChannels=\"2\" destination=\"{master}c\" id=\"{cid}\">\n        <Volume unit=\"linear\" value=\"1\"/>\n        <Pan unit=\"normalized\" value=\"0.5\"/>\n        <Mute value=\"{}\"/>\n      </Channel>\n    </Track>\n",
                xml(&t.name),
                t.mute
            ));
            let (lid, clid) = (nid(), nid());
            lanes.push_str(&format!(
                "      <Lanes track=\"{tid}\" id=\"{lid}\">\n        <Clips id=\"{clid}\">\n          <Clip time=\"0\" duration=\"{len_beats:.4}\" name=\"{}\">\n            <Audio sampleRate=\"{sr}\" channels=\"2\" duration=\"{secs:.4}\">\n              <File path=\"{}\"/>\n            </Audio>\n          </Clip>\n        </Clips>\n      </Lanes>\n",
                xml(&t.name),
                xml(&st.file)
            ));
        }
        // the notes ride along on a muted track so they can be re-voiced in the new DAW
        let has_notes = t.clips.iter().any(|c| matches!(&c.data, ClipData::Midi(n) if !n.is_empty()));
        if has_notes {
            let (tid, cid) = (nid(), nid());
            parts.push_str(&format!(
                "    <Track contentType=\"notes\" loaded=\"true\" id=\"{tid}\" name=\"{} (notes)\" color=\"{col}\">\n      <Channel role=\"regular\" audioChannels=\"2\" destination=\"{master}c\" id=\"{cid}\">\n        <Volume unit=\"linear\" value=\"{:.4}\"/>\n        <Pan unit=\"normalized\" value=\"{:.4}\"/>\n        <Mute value=\"true\"/>\n      </Channel>\n    </Track>\n",
                xml(&t.name),
                t.volume,
                (t.pan as f64 * 0.5 + 0.5).clamp(0.0, 1.0)
            ));
            let (lid, clid) = (nid(), nid());
            lanes.push_str(&format!("      <Lanes track=\"{tid}\" id=\"{lid}\">\n        <Clips id=\"{clid}\">\n"));
            for c in &t.clips {
                let ClipData::Midi(notes) = &c.data else { continue };
                if c.start + c.len <= shift {
                    continue;
                }
                let ll = if c.loop_len >= 0.25 && c.loop_len < c.len { c.loop_len } else { c.len };
                let mut body = String::new();
                for n in notes.iter().filter(|n| n.start < ll) {
                    body.push_str(&format!("              <Note time=\"{:.5}\" duration=\"{:.5}\" channel=\"0\" key=\"{}\" vel=\"{:.3}\" rel=\"{:.3}\"/>\n", n.start, n.len.min(ll - n.start).max(0.03), n.key, n.vel, n.vel));
                }
                let (nid2, loop_attr) = (nid(), if ll < c.len { format!(" loopStart=\"0\" loopEnd=\"{ll:.4}\" playStart=\"0\"") } else { String::new() });
                lanes.push_str(&format!("          <Clip time=\"{:.5}\" duration=\"{:.5}\" name=\"{}\"{loop_attr}>\n            <Notes id=\"{nid2}\">\n{body}            </Notes>\n          </Clip>\n", c.start - shift, c.len, xml(&t.name)));
            }
            lanes.push_str("        </Clips>\n      </Lanes>\n");
        }
    }
    s.push_str(&parts);
    s.push_str(&format!("    <Track contentType=\"audio\" loaded=\"true\" id=\"{master}\" name=\"Master\">\n      <Channel role=\"master\" audioChannels=\"2\" id=\"{master}c\">\n        <Volume unit=\"linear\" value=\"{:.4}\"/>\n      </Channel>\n    </Track>\n  </Structure>\n", p.master_volume));
    s.push_str(&format!("  <Arrangement>\n    <Lanes timeUnit=\"beats\" id=\"arr\">\n{lanes}    </Lanes>\n  </Arrangement>\n</Project>\n"));
    let meta = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<MetaData>\n  <Title>{}</Title>\n  <Application>LingStation</Application>\n</MetaData>\n", xml(&p.name));
    let mut files: Vec<(String, Vec<u8>)> = vec![("metadata.xml".into(), meta.into_bytes()), ("project.xml".into(), s.into_bytes())];
    files.extend(audio.iter().cloned());
    files
}

// ── README ────────────────────────────────────────────────────────────────────

fn readme(p: &Project, shift: f64, stems: &[Stem]) -> String {
    format!(
        "{name} - stems bundle from LingStation\n\
         ======================================\n\n\
         Tempo: {bpm:.2} BPM, 4/4{tm}. Every file starts at the same point{from}, so drop them all at bar 1.\n\n\
         audio/            one WAV stem per track ({n}), each rendered alone through its own effects (no master chain)\n\
         midi/project.mid  all tracks with the tempo map; midi/NN name.mid one track each (channel 10 = drums)\n\
         instruments/      the SoundFonts the song plays and each synth's Ling patch (full parameter state)\n\
         params.json       tempo, levels, pans, instrument state, effect chains + parameters, automation, routing\n\
         project.dawproject  open DAWproject file: stems as audio tracks, notes on muted note tracks\n\n\
         FL Studio   Set the tempo, then drag audio/*.wav into the Playlist at bar 1 (or File > Import > MIDI file for midi/project.mid).\n\
         Ableton     Set the tempo; drag the WAVs into the Arrangement at 1.1.1 and the MIDI files onto MIDI tracks (Live 12 also reads the tempo from the MIDI file).\n\
         Bitwig      File > Open project.dawproject (or drag the folder's audio and MIDI in).\n\
         Studio One / Cubase 14   File > Open project.dawproject.\n\
         Logic / Reaper / others  Import audio/*.wav at 0:00 and midi/project.mid (type 1, tempo track included).\n\n\
         The DAWproject reader support differs between programs; if one rejects it, the WAV + MIDI route above always works.\n",
        name = p.name,
        bpm = p.bpm,
        tm = if p.tempo_map.is_empty() { String::new() } else { format!(" (with {} tempo changes - see the MIDI tempo track)", p.tempo_map.len()) },
        from = if shift > 0.0 { format!(" (beat {shift})") } else { String::new() },
        n = stems.len()
    )
}

// ── the bundle ────────────────────────────────────────────────────────────────

/// Write the companions of the audio stems already in `root/audio`. `start`/`end` are the exported beat range and `tail` the seconds after it.
pub fn write(e: &Engine, root: &Path, stems: &[Stem], start: f64, end: f64, tail: f64, sr: u32) -> Result<String, String> {
    for d in ["midi", "instruments"] {
        std::fs::create_dir_all(root.join(d)).map_err(|x| format!("{}: {x}", root.join(d).display()))?;
    }
    let p = &e.project;
    let inst_tracks: Vec<usize> = (0..p.tracks.len()).filter(|i| matches!(p.tracks[*i].kind, TrackKind::Instrument)).collect();
    let mut midi: Vec<(usize, String)> = vec![];
    write_smf(&root.join("midi").join("project.mid"), e, start, &inst_tracks, false)?;
    for &i in &inst_tracks {
        let rel = format!("midi/{:02} {}.mid", i + 1, safe(&p.tracks[i].name));
        if write_smf(&root.join(&rel), e, start, &[i], true)? {
            midi.push((i, rel));
        }
    }
    let mut patches: Vec<(usize, String)> = vec![];
    let mut copied: Vec<String> = vec![];
    for (i, t) in p.tracks.iter().enumerate() {
        if !matches!(t.kind, TrackKind::Instrument) {
            continue;
        }
        if t.inst_kind == "sf2" {
            if !t.patch.is_empty() && Path::new(&t.patch).is_file() {
                let name = Path::new(&t.patch).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| "font.sf2".into());
                if !copied.contains(&name) {
                    std::fs::copy(&t.patch, root.join("instruments").join(&name)).map_err(|x| format!("{}: {x}", t.patch))?;
                    copied.push(name.clone());
                }
                patches.push((i, format!("instruments/{name}")));
            }
        } else if let Some(inst) = t.inst.as_ref() {
            let fx: Vec<crate::patch::FxSpec> = t.effects.iter().map(|f| crate::patch::FxSpec { name: f.name.clone(), params: f.params.iter().map(|(a, v)| (a.to_string(), *v)).collect() }).collect();
            let kind = if t.inst_kind.is_empty() { "synth" } else { t.inst_kind.as_str() };
            let rel = format!("instruments/{:02} {}.ling", i + 1, safe(&t.name));
            std::fs::write(root.join(&rel), crate::patch::dump(kind, &t.name, &inst.state(), &fx)).map_err(|x| x.to_string())?;
            patches.push((i, rel));
        }
    }
    let params = build_params(e, start, end, stems, &midi, &patches);
    std::fs::write(root.join("params.json"), serde_json::to_string_pretty(&params).map_err(|x| x.to_string())?).map_err(|x| x.to_string())?;
    // the DAWproject: its audio is the stems (a reference to a file inside the archive)
    let mut audio: Vec<(String, Vec<u8>)> = vec![];
    for s in stems {
        audio.push((s.file.clone(), std::fs::read(root.join(&s.file)).map_err(|x| format!("{}: {x}", s.file))?));
    }
    let zip = zip_stored(&dawproject(e, start, end, tail, sr, stems, &audio));
    std::fs::write(root.join("project.dawproject"), zip).map_err(|x| x.to_string())?;
    std::fs::write(root.join("README.txt"), readme(p, start, stems)).map_err(|x| x.to_string())?;
    Ok(format!("{} stems, {} MIDI files, {} instruments", stems.len(), midi.len(), patches.len()))
}
