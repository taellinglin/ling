//! FL Studio project (.flp) import.
//!
//! An .flp is a header (`FLhd`: format, channel count, PPQ) followed by one data chunk (`FLdt`) of events: a one-byte id,
//! then 1, 2 or 4 bytes of data for ids below 64 / 128 / 192, or a variable-length blob (7-bit length) above that.
//! Only what matters musically is read: tempo, the channel rack (plugin, name, sample), the patterns' notes and the
//! playlist (where each pattern / audio clip sits). The result is the same `ImportedMidi` the MIDI importer produces, so
//! every channel becomes a part the user can give any instrument to.
//!
//! Reverse-engineered layout, checked against FL Studio 21 projects (see tests):
//! * 64 new channel (u16), 65 new pattern (u16), 156 fine tempo (BPM x 1000), 199 version (text)
//! * 21 channel type (0 sampler, 2 plugin, 4 audio clip, 5 automation), 192 channel name, 196 sample path,
//!   201 default plugin name, 203 display name; the first of each per channel counts (later 201s are mixer effects)
//! * 224 pattern notes: 24 bytes each: position u32, flags u16, rack channel u16, length u32, key u16, group u16,
//!   fine pitch i8, reserved u8, release u8, midi channel u8, pan u8 (64 = centre), velocity u8, mod x u8, mod y u8
//! * 233 playlist items, 60 bytes each in FL 21 (32 in FL 20): position u32, base u16 (20480), item u16
//!   (> base: pattern number + base; otherwise a channel number = audio clip), length u32, track u16, ...
//! Positions and lengths are in ticks; `ppq` ticks make a beat.
use crate::midi_import::{ImportedMidi, ImportedTrack};
use crate::model::Note;
use std::collections::BTreeMap;

const ITEM_BASE: u32 = 20480;
const MAX_NOTES: usize = 1_500_000;

#[derive(Default, Clone)]
struct Chan {
    ctype: u8,
    plugin: String,
    name: String,
    display: String,
    sample: String,
    seen_plugin: bool,
    seen_display: bool,
}

#[derive(Default)]
struct Pat {
    name: String,
    /// (position ticks, length ticks, key, rack channel, velocity 0..127)
    notes: Vec<(u32, u32, u16, u16, u8)>,
}

struct Item {
    pos: u32,
    len: u32,
    index: u32,
}

struct Reader<'a> {
    d: &'a [u8],
    p: usize,
}
impl<'a> Reader<'a> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.d.get(self.p)?;
        self.p += 1;
        Some(v)
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.d.get(self.p..self.p.checked_add(n)?)?;
        self.p += n;
        Some(s)
    }
}

fn u16le(b: &[u8], o: usize) -> u16 {
    b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).unwrap_or(0)
}
fn u32le(b: &[u8], o: usize) -> u32 {
    b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])).unwrap_or(0)
}

/// FL 11.5+ stores text as UTF-16; older versions as single bytes. Both end at a NUL.
fn text(b: &[u8]) -> String {
    let utf16 = b.len() >= 2 && b[1] == 0 && b.len() % 2 == 0;
    let s = if utf16 {
        let w: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|c| *c != 0).collect();
        String::from_utf16_lossy(&w)
    } else {
        let end = b.iter().position(|c| *c == 0).unwrap_or(b.len());
        b[..end].iter().map(|c| *c as char).collect()
    };
    s.chars().filter(|c| !c.is_control()).collect::<String>().trim().to_string()
}

pub fn looks_like_flp(data: &[u8]) -> bool {
    data.len() > 22 && &data[..4] == b"FLhd"
}

// ── instrument guesses ─────────────────────────────────────────────────────────

fn tokens(s: &str) -> Vec<String> {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).map(|t| t.to_string()).collect()
}

/// A drum channel is recognised by what it is called; returns the General MIDI percussion key it should play.
fn drum_key(name: &str) -> Option<u8> {
    let toks = tokens(name);
    let has = |w: &str| toks.iter().any(|t| t == w);
    let has_part = |w: &str| toks.iter().any(|t| t.contains(w));
    let open = has_part("open") || has("oh") || has("ohh") || has("ohat");
    if has_part("kick") || has("bd") || has_part("bassdrum") {
        return Some(36);
    }
    if has_part("rim") || has_part("sidestick") {
        return Some(37);
    }
    if has_part("snare") || has("snr") || has("sd") {
        return Some(38);
    }
    if has_part("clap") || has("cp") || has_part("snap") {
        return Some(39);
    }
    if has_part("hat") || has("hh") || has("hihat") || has("chh") || has("ohh") || has("oh") {
        return Some(if open { 46 } else { 42 });
    }
    if has_part("tom") {
        return Some(if has_part("low") || has_part("floor") { 41 } else if has_part("hi") || has_part("high") { 48 } else { 45 });
    }
    if has_part("crash") || has_part("cymbal") {
        return Some(49);
    }
    if has_part("ride") {
        return Some(51);
    }
    if has_part("cowbell") {
        return Some(56);
    }
    if has_part("shaker") || has_part("maraca") {
        return Some(70);
    }
    if has_part("tamb") {
        return Some(54);
    }
    if has_part("conga") {
        return Some(63);
    }
    if has_part("bongo") {
        return Some(60);
    }
    if has_part("clave") || has_part("perc") {
        return Some(75);
    }
    None
}

/// Plugins that are drum machines: their notes already pick pads / pieces by key, so the keys are kept.
fn is_drum_plugin(plugin: &str) -> bool {
    let p = plugin.to_lowercase();
    ["fpc", "drumaxx", "drumpad", "drumsynth", "kick", "slicex"].iter().any(|k| p.contains(k))
}

/// A General MIDI program (0-based) that suits a channel, from what it is called.
pub fn guess_program(name: &str, plugin: &str) -> u8 {
    let toks = tokens(&format!("{name} {plugin}"));
    let any = |ws: &[&str]| toks.iter().any(|t| ws.iter().any(|w| t.contains(w)));
    if any(&["bass", "808", "sub", "reese", "wobble", "growl"]) {
        return 38;
    }
    if any(&["piano", "grand"]) {
        return 0;
    }
    if any(&["epiano", "rhodes", "keys", "wurli", "ep"]) {
        return 4;
    }
    if any(&["organ"]) {
        return 16;
    }
    if any(&["violin", "fiddle"]) {
        return 40;
    }
    if any(&["cello"]) {
        return 42;
    }
    if any(&["string", "orchestra", "orch", "ensemble"]) {
        return 48;
    }
    if any(&["choir", "vox", "voice", "vocal", "aah", "ooh"]) {
        return 52;
    }
    if any(&["trumpet"]) {
        return 56;
    }
    if any(&["brass", "horn", "trombone"]) {
        return 61;
    }
    if any(&["flute"]) {
        return 73;
    }
    if any(&["harp"]) {
        return 46;
    }
    if any(&["guitar", "pluck", "pizz", "staccato"]) {
        return 24;
    }
    if any(&["bell", "chime", "glock", "celesta", "mallet", "marimba"]) {
        return 9;
    }
    if any(&["pad", "atmos", "ambient", "ether", "drone", "warm", "wash", "sweep"]) {
        return 89;
    }
    if any(&["arp", "sequence", "seq"]) {
        return 82;
    }
    if any(&["lead", "solo", "saw", "square", "synth", "sid", "sega"]) {
        return 81;
    }
    81
}

fn resolve_sample(path: &str, base: Option<&std::path::Path>) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    let mut cands: Vec<std::path::PathBuf> = vec![std::path::PathBuf::from(path)];
    let file = std::path::Path::new(&path.replace('\\', "/")).file_name().map(|f| f.to_os_string());
    if let (Some(b), Some(f)) = (base, file.as_ref()) {
        cands.push(b.join(f));
        cands.push(b.join("Samples").join(f));
    }
    // %FLStudioFactoryData% and friends point into the FL Studio install
    if path.contains('%') {
        for pf in ["C:/Program Files/Image-Line", "C:/Program Files (x86)/Image-Line"] {
            if let Ok(rd) = std::fs::read_dir(pf) {
                for e in rd.flatten() {
                    let n = e.file_name().to_string_lossy().to_lowercase();
                    if n.starts_with("fl studio") {
                        if let Some(idx) = path.find('%').and_then(|a| path[a + 1..].find('%').map(|b| a + 1 + b + 1)) {
                            let rest = path[idx..].replace('\\', "/");
                            cands.push(e.path().join(rest.trim_start_matches('/')));
                        }
                    }
                }
            }
        }
    }
    cands.into_iter().find(|c| c.is_file()).map(|c| c.to_string_lossy().into_owned())
}

pub fn parse(data: &[u8], flp_path: Option<&std::path::Path>) -> Result<ImportedMidi, String> {
    if !looks_like_flp(data) {
        return Err("not an FL Studio project (no FLhd header)".into());
    }
    let hlen = u32le(data, 4) as usize;
    let ppq = (u16le(data, 12) as f64).max(1.0);
    let mut p = 8usize.checked_add(hlen).ok_or("bad header")?;
    if data.get(p..p + 4) != Some(b"FLdt") {
        return Err("no FLdt data chunk".into());
    }
    let size = u32le(data, p + 4) as usize;
    p += 8;
    let end = data.len().min(p.saturating_add(size));
    let mut r = Reader { d: &data[..end], p };

    let mut chans: BTreeMap<u16, Chan> = BTreeMap::new();
    let mut pats: BTreeMap<u16, Pat> = BTreeMap::new();
    let mut items: Vec<Item> = vec![];
    let mut cur_chan: Option<u16> = None;
    let mut cur_pat: Option<u16> = None;
    let mut tempo: Option<f64> = None;
    let mut title = String::new();
    let mut note_count = 0usize;
    while let Some(id) = r.u8() {
        let blob: &[u8] = if id < 64 {
            match r.take(1) {
                Some(b) => b,
                None => break,
            }
        } else if id < 128 {
            match r.take(2) {
                Some(b) => b,
                None => break,
            }
        } else if id < 192 {
            match r.take(4) {
                Some(b) => b,
                None => break,
            }
        } else {
            let mut n = 0usize;
            let mut shift = 0u32;
            loop {
                let Some(b) = r.u8() else { return finish(chans, pats, items, tempo, ppq, flp_path, title) };
                n |= ((b & 0x7f) as usize) << shift;
                shift += 7;
                if b & 0x80 == 0 || shift > 28 {
                    break;
                }
            }
            match r.take(n) {
                Some(b) => b,
                None => break,
            }
        };
        match id {
            64 => {
                let c = u16le(blob, 0);
                cur_chan = Some(c);
                chans.entry(c).or_default();
            }
            65 => {
                let n = u16le(blob, 0);
                cur_pat = Some(n);
                pats.entry(n).or_default();
            }
            156 => tempo = Some(u32le(blob, 0) as f64 / 1000.0),
            66 if tempo.is_none() => tempo = Some(u16le(blob, 0) as f64),
            194 => title = text(blob),
            21 => {
                if let Some(c) = cur_chan.and_then(|c| chans.get_mut(&c)) {
                    c.ctype = blob.first().copied().unwrap_or(0);
                }
            }
            192 => {
                if let Some(c) = cur_chan.and_then(|c| chans.get_mut(&c)) {
                    c.name = text(blob);
                }
            }
            196 => {
                if let Some(c) = cur_chan.and_then(|c| chans.get_mut(&c)) {
                    if c.sample.is_empty() {
                        c.sample = text(blob);
                    }
                }
            }
            201 => {
                if let Some(c) = cur_chan.and_then(|c| chans.get_mut(&c)) {
                    if !c.seen_plugin {
                        c.plugin = text(blob);
                        c.seen_plugin = true;
                    }
                }
            }
            203 => {
                if let Some(c) = cur_chan.and_then(|c| chans.get_mut(&c)) {
                    if !c.seen_display {
                        c.display = text(blob);
                        c.seen_display = true;
                    }
                }
            }
            193 => {
                if let Some(pt) = cur_pat.and_then(|n| pats.get_mut(&n)) {
                    pt.name = text(blob);
                }
            }
            224 => {
                if let Some(pt) = cur_pat.and_then(|n| pats.get_mut(&n)) {
                    for n in blob.chunks_exact(24) {
                        if note_count >= MAX_NOTES {
                            break;
                        }
                        note_count += 1;
                        pt.notes.push((u32le(n, 0), u32le(n, 8), u16le(n, 12), u16le(n, 6), n[21]));
                    }
                }
            }
            233 => {
                // item size depends on the FL version: 60 (21+), 32 (20), 12 (older); take the first that fits and looks right
                let size = [60usize, 32, 12].into_iter().find(|s| blob.len() % s == 0 && blob.len() >= *s && u16le(blob, 4) as u32 == ITEM_BASE).or_else(|| [60usize, 32].into_iter().find(|s| blob.len() >= *s && u16le(blob, 4) as u32 == ITEM_BASE));
                if let Some(s) = size {
                    for it in blob.chunks_exact(s) {
                        if u16le(it, 4) as u32 != ITEM_BASE {
                            continue;
                        }
                        items.push(Item { pos: u32le(it, 0), index: u16le(it, 6) as u32, len: u32le(it, 8) });
                    }
                }
            }
            _ => {}
        }
    }
    finish(chans, pats, items, tempo, ppq, flp_path, title)
}

fn finish(chans: BTreeMap<u16, Chan>, pats: BTreeMap<u16, Pat>, items: Vec<Item>, tempo: Option<f64>, ppq: f64, flp_path: Option<&std::path::Path>, title: String) -> Result<ImportedMidi, String> {
    let base = flp_path.and_then(|p| p.parent());
    let beats = |t: u32| t as f64 / ppq;
    // where each pattern plays: (start beat, length beats); with no arrangement the patterns follow each other
    let mut placements: Vec<(u16, f64, f64)> = vec![];
    for it in &items {
        if it.index > ITEM_BASE {
            let pn = (it.index - ITEM_BASE) as u16;
            if pats.contains_key(&pn) {
                placements.push((pn, beats(it.pos), beats(it.len)));
            }
        }
    }
    let pat_len = |pt: &Pat| -> f64 {
        let end = pt.notes.iter().map(|n| beats(n.0 + n.1)).fold(0.0, f64::max);
        ((end / 4.0).ceil() * 4.0).max(4.0)
    };
    if placements.is_empty() {
        let mut at = 0.0;
        for (n, pt) in &pats {
            if pt.notes.is_empty() {
                continue;
            }
            let l = pat_len(pt);
            placements.push((*n, at, l));
            at += l;
        }
    }
    // notes per channel on the song timeline
    let mut by_chan: BTreeMap<u16, Vec<Note>> = BTreeMap::new();
    for (pn, start, len) in &placements {
        let Some(pt) = pats.get(pn) else { continue };
        let plen = pat_len(pt);
        // an item that is a whole number of pattern lengths long repeats the pattern
        let reps = if *len > plen + 1e-6 && ((*len / plen).fract() < 1e-3 || (*len / plen).fract() > 0.999) { (*len / plen).round() as usize } else { 1 };
        for k in 0..reps.min(256) {
            for &(pos, nlen, key, rack, vel) in &pt.notes {
                let s = start + k as f64 * plen + beats(pos);
                if s >= start + len.max(plen) - 1e-9 && reps == 1 && *len < plen {
                    continue;
                }
                let mut n = Note::new(s, beats(nlen).max(0.03), key.min(127) as u8, (vel as f32 / 127.0).clamp(0.05, 1.0));
                n.pan = 0.0;
                by_chan.entry(rack).or_default().push(n);
            }
        }
    }
    // audio clips on the playlist: (channel, beat)
    let mut clips: BTreeMap<u16, Vec<f64>> = BTreeMap::new();
    for it in &items {
        if it.index <= ITEM_BASE && chans.get(&(it.index as u16)).map(|c| c.ctype == 4 || c.ctype == 0).unwrap_or(false) {
            clips.entry(it.index as u16).or_default().push(beats(it.pos));
        }
    }

    let mut parts: Vec<ImportedTrack> = vec![];
    let mut drum_notes: Vec<Note> = vec![];
    let mut missing = 0usize;
    let mk = |name: String, program: Option<u8>, notes: Vec<Note>, samples: Vec<(f64, String)>, channel: u8| ImportedTrack {
        named: true,
        plugin: String::new(),
        name,
        program,
        bank: 0,
        channel,
        notes,
        volume: 0.9,
        pan: 0.0,
        vol_auto: vec![],
        pan_auto: vec![],
        bend_auto: vec![],
        bend_range: 2.0,
        samples,
        sampler: None,
        percussive: false,
        extra_auto: vec![],
    };
    for (id, ch) in &chans {
        let before = parts.len();
        let label = if !ch.name.is_empty() {
            ch.name.clone()
        } else if !ch.display.is_empty() {
            ch.display.clone()
        } else if !ch.plugin.is_empty() {
            ch.plugin.clone()
        } else {
            format!("Channel {}", id + 1)
        };
        let notes = by_chan.remove(id).unwrap_or_default();
        match ch.ctype {
            5 => continue, // automation clips are not carried over
            4 => {
                // an audio clip channel: the sample at every playlist position
                let Some(path) = resolve_sample(&ch.sample, base) else {
                    if !ch.sample.is_empty() && clips.contains_key(id) {
                        missing += 1;
                    }
                    continue;
                };
                let at = clips.remove(id).unwrap_or_default();
                if at.is_empty() {
                    continue;
                }
                parts.push(mk(label, Some(0), vec![], at.into_iter().map(|b| (b, path.clone())).collect(), 0));
            }
            _ => {
                if notes.is_empty() {
                    continue;
                }
                let drum_by_plugin = is_drum_plugin(&ch.plugin);
                let name_key = drum_key(&label).or_else(|| drum_key(&ch.sample));
                if drum_by_plugin {
                    // a drum machine: keep its pad keys, all on one drum part
                    drum_notes.extend(notes);
                } else if let (Some(k), true) = (name_key, ch.ctype == 0 || ch.sample.is_empty() == false || ch.ctype == 2) {
                    // a drum sample channel: every hit becomes that GM percussion key
                    drum_notes.extend(notes.into_iter().map(|mut n| {
                        n.key = k;
                        n
                    }));
                } else if ch.ctype == 0 && !ch.sample.is_empty() {
                    // a pitched sample / vocal chop: the sample on every note start
                    match resolve_sample(&ch.sample, base) {
                        Some(path) => parts.push(mk(label, Some(0), vec![], notes.iter().map(|n| (n.start, path.clone())).collect(), 0)),
                        None => {
                            missing += 1;
                            parts.push(mk(label.clone(), Some(guess_program(&label, &ch.plugin)), notes, vec![], 0));
                        }
                    }
                } else {
                    let prog = guess_program(&label, &ch.plugin);
                    parts.push(mk(label, Some(prog), notes, vec![], (parts.len() % 15) as u8));
                }
            }
        }
        for p in parts.iter_mut().skip(before) {
            p.plugin = ch.plugin.clone();
        }
    }
    if !drum_notes.is_empty() {
        drum_notes.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
        parts.insert(0, mk("Drums".into(), None, drum_notes, vec![], 9));
    }
    if parts.is_empty() {
        return Err(if missing > 0 { format!("the project only uses samples that could not be found ({missing})") } else { "no notes or audio clips found in this project".into() });
    }
    let bpm = tempo.filter(|b| b.is_finite() && *b >= 20.0 && *b <= 400.0);
    let length_beats = parts.iter().map(|t| t.notes.iter().map(|n| n.start + n.len).chain(t.samples.iter().map(|s| s.0 + 1.0)).fold(0.0, f64::max)).fold(0.0, f64::max);
    let _ = title;
    Ok(ImportedMidi { tracks: parts, bpm, tempos: bpm.map(|b| vec![(0.0, b)]).unwrap_or_default(), length_beats })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(out: &mut Vec<u8>, id: u8, data: &[u8]) {
        out.push(id);
        if id >= 192 {
            let mut n = data.len();
            loop {
                let b = (n & 0x7f) as u8;
                n >>= 7;
                if n == 0 {
                    out.push(b);
                    break;
                }
                out.push(b | 0x80);
            }
        }
        out.extend_from_slice(data);
    }
    fn u16s(s: &str) -> Vec<u8> {
        s.encode_utf16().chain(std::iter::once(0)).flat_map(|c| c.to_le_bytes()).collect()
    }
    fn note(pos: u32, len: u32, rack: u16, key: u16, vel: u8) -> Vec<u8> {
        let mut n = vec![];
        n.extend(pos.to_le_bytes());
        n.extend(16384u16.to_le_bytes());
        n.extend(rack.to_le_bytes());
        n.extend(len.to_le_bytes());
        n.extend(key.to_le_bytes());
        n.extend(0u16.to_le_bytes());
        n.extend([120u8, 0, 64, 0, 64, vel, 128, 128]);
        assert_eq!(n.len(), 24);
        n
    }
    fn item(pos: u32, pat: u32, len: u32) -> Vec<u8> {
        let mut it = vec![];
        it.extend(pos.to_le_bytes());
        it.extend((ITEM_BASE as u16).to_le_bytes());
        it.extend(((ITEM_BASE + pat) as u16).to_le_bytes());
        it.extend(len.to_le_bytes());
        it.extend(499u16.to_le_bytes());
        it.resize(60, 0);
        it
    }
    fn build(with_playlist: bool) -> Vec<u8> {
        let mut d = vec![];
        ev(&mut d, 199, b"21.0.2.3399\0");
        ev(&mut d, 156, &131_000u32.to_le_bytes());
        for (id, ty, plugin, name) in [(0u16, 2u8, "Sytrus", "Fat Bass"), (1, 2, "Harmless", "Lead Sega"), (2, 0, "", "Kick 01")] {
            ev(&mut d, 64, &id.to_le_bytes());
            ev(&mut d, 21, &[ty]);
            ev(&mut d, 201, &u16s(plugin));
            ev(&mut d, 203, &u16s(name));
        }
        ev(&mut d, 65, &1u16.to_le_bytes());
        let mut notes = vec![];
        notes.extend(note(0, 96, 0, 36, 100));
        notes.extend(note(96, 96, 0, 43, 80));
        notes.extend(note(0, 192, 1, 72, 127));
        notes.extend(note(0, 48, 2, 60, 100));
        notes.extend(note(192, 48, 2, 60, 100));
        ev(&mut d, 224, &notes);
        if with_playlist {
            let mut pl = item(0, 1, 384);
            pl.extend(item(1536, 1, 384));
            ev(&mut d, 233, &pl);
        }
        let mut f = vec![];
        f.extend(b"FLhd");
        f.extend(6u32.to_le_bytes());
        f.extend(0u16.to_le_bytes());
        f.extend(3u16.to_le_bytes());
        f.extend(96u16.to_le_bytes());
        f.extend(b"FLdt");
        f.extend((d.len() as u32).to_le_bytes());
        f.extend(d);
        f
    }

    #[test]
    fn channels_become_parts_with_tempo_names_and_timing() {
        let m = parse(&build(true), None).unwrap();
        assert_eq!(m.bpm, Some(131.0));
        let names: Vec<&str> = m.tracks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["Drums", "Fat Bass", "Lead Sega"], "{names:?}");
        let bass = &m.tracks[1];
        assert_eq!(bass.program, Some(38), "a bass name picks a synth bass");
        // pattern 1 plays at beat 0 and again at beat 16 (1536 ticks)
        let starts: Vec<f64> = bass.notes.iter().map(|n| n.start).collect();
        assert_eq!(starts, vec![0.0, 1.0, 16.0, 17.0]);
        assert_eq!(bass.notes[0].key, 36);
        assert!((bass.notes[1].vel - 80.0 / 127.0).abs() < 1e-4);
        // the item is 4 beats long and the pattern one bar: no repeat
        assert_eq!(m.tracks[2].notes.len(), 2);
    }

    #[test]
    fn drum_samples_merge_into_one_part_on_gm_keys() {
        let m = parse(&build(true), None).unwrap();
        let drums = &m.tracks[0];
        assert_eq!(drums.program, None, "drums use the percussion channel");
        assert!(drums.notes.iter().all(|n| n.key == 36), "Kick 01 plays the GM kick");
        assert_eq!(drums.notes.len(), 4);
    }

    #[test]
    fn without_a_playlist_patterns_follow_each_other() {
        let m = parse(&build(false), None).unwrap();
        assert_eq!(m.tracks.iter().map(|t| t.notes.len()).sum::<usize>(), 5, "every note is imported once");
    }

    #[test]
    fn hostile_files_never_panic() {
        let good = build(true);
        assert!(parse(b"nope", None).is_err());
        assert!(parse(&good[..30], None).is_err());
        // every truncation of a valid file, and bytes flipped at every position
        for cut in 0..good.len() {
            let _ = parse(&good[..cut], None);
        }
        for i in 0..good.len() {
            let mut bad = good.clone();
            bad[i] ^= 0xff;
            let _ = parse(&bad, None);
        }
    }

    #[test]
    fn names_decide_drum_keys_and_programs() {
        assert_eq!(drum_key("Kick 01"), Some(36));
        assert_eq!(drum_key("Attack MidTom 03"), Some(45));
        assert_eq!(drum_key("HH open"), Some(46));
        assert_eq!(drum_key("Wisdom"), None);
        assert_eq!(guess_program("Cellos Sustain #2", "FLEX"), 42);
        assert_eq!(guess_program("Solo Vox", "FLEX"), 52);
        assert_eq!(guess_program("Polysynth", "FLEX"), 81);
        assert_eq!(guess_program("Slap", "Sytrus"), 81);
    }

    /// The projects on the author's machine, when present: a real-world check of the reverse-engineered layout.
    #[test]
    fn real_fl_studio_projects_import_when_available() {
        let home = std::env::var("USERPROFILE").unwrap_or_default();
        let candidates = [
            format!("{home}/Music/Anthem of Ling Country/Anthem of Ling Country.flp"),
            format!("{home}/Documents/Image-Line/FL Studio/Projects/Arena/Arena.flp"),
        ];
        let mut tried = 0;
        for c in candidates {
            let path = std::path::Path::new(&c);
            let Ok(data) = std::fs::read(path) else { continue };
            tried += 1;
            let m = parse(&data, Some(path)).unwrap_or_else(|e| panic!("{c}: {e}"));
            assert!(m.bpm.is_some(), "{c}: tempo");
            assert!(!m.tracks.is_empty());
            let notes: usize = m.tracks.iter().map(|t| t.notes.len()).sum();
            eprintln!("{c}: {} parts, {notes} notes, {} bpm, {:.0} beats", m.tracks.len(), m.bpm.unwrap_or(0.0), m.length_beats);
            for t in &m.tracks {
                eprintln!("   {:<28} prog {:?}  {} notes {} samples", t.name, t.program, t.notes.len(), t.samples.len());
            }
            assert!(notes > 0, "{c}: no notes");
        }
        eprintln!("({tried} real projects tried)");
    }
}

// ── automation clips ────────────────────────────────────────────────────────

/// An FL Studio automation clip on the song timeline. FL names a clip after what it moves ("SubSynth - Filter Cutoff"): `target` is
/// the part before " - " (the channel / plugin), `param` the rest. `points` are (song beat, value 0..1) wherever the playlist places it.
#[derive(Clone, Debug)]
pub struct FlAuto {
    pub name: String,
    pub target: String,
    pub param: String,
    pub points: Vec<(f64, f32)>,
}

/// Every automation clip of a project, placed on the song timeline.
/// Layout (checked against FL 20 / 21 projects): an automation channel (type 5) carries event 234: 21 bytes of header with the point
/// count as u32 at offset 17, then 24 bytes per point - the distance from the previous point in beats (f64), the value 0..1 (f64),
/// the tension (f32) and flags (u32) - then a trailer. The clip sits on the playlist like an audio clip (item = channel number) and
/// plays from its start for the item's length.
pub fn automation(data: &[u8]) -> Vec<FlAuto> {
    if !looks_like_flp(data) {
        return vec![];
    }
    let hlen = u32le(data, 4) as usize;
    let ppq = (u16le(data, 12) as f64).max(1.0);
    let Some(mut p) = 8usize.checked_add(hlen) else { return vec![] };
    if data.get(p..p + 4) != Some(b"FLdt") {
        return vec![];
    }
    let size = u32le(data, p + 4) as usize;
    p += 8;
    let end = data.len().min(p.saturating_add(size));
    let mut r = Reader { d: &data[..end], p };
    #[derive(Default)]
    struct AChan {
        ctype: u8,
        name: String,
        display: String,
        pts: Vec<(f64, f32)>,
    }
    let mut chans: BTreeMap<u16, AChan> = BTreeMap::new();
    let mut cur: Option<u16> = None;
    let mut items: Vec<Item> = vec![];
    while let Some(id) = r.u8() {
        let n = if id < 64 {
            1
        } else if id < 128 {
            2
        } else if id < 192 {
            4
        } else {
            let mut n = 0usize;
            let mut shift = 0u32;
            loop {
                let Some(b) = r.u8() else { break };
                n |= ((b & 0x7f) as usize) << shift;
                shift += 7;
                if b & 0x80 == 0 || shift > 28 {
                    break;
                }
            }
            n
        };
        let Some(blob) = r.take(n) else { break };
        match id {
            64 => {
                let c = u16le(blob, 0);
                cur = Some(c);
                chans.entry(c).or_default();
            }
            21 => {
                if let Some(c) = cur.and_then(|c| chans.get_mut(&c)) {
                    c.ctype = blob.first().copied().unwrap_or(0);
                }
            }
            192 => {
                if let Some(c) = cur.and_then(|c| chans.get_mut(&c)) {
                    c.name = text(blob);
                }
            }
            203 => {
                if let Some(c) = cur.and_then(|c| chans.get_mut(&c)) {
                    if c.display.is_empty() {
                        c.display = text(blob);
                    }
                }
            }
            234 => {
                if let Some(c) = cur.and_then(|c| chans.get_mut(&c)) {
                    if blob.len() >= 21 {
                        let np = u32le(blob, 17) as usize;
                        if np > 0 && np < 100_000 && blob.len() >= 21 + np * 24 {
                            let f64at = |o: usize| f64::from_le_bytes(blob[o..o + 8].try_into().unwrap_or([0; 8]));
                            let mut t = 0.0;
                            let mut pts = Vec::with_capacity(np);
                            let mut ok = true;
                            for k in 0..np {
                                let (d, v) = (f64at(21 + k * 24), f64at(29 + k * 24));
                                if !d.is_finite() || !v.is_finite() || !(0.0..1e6).contains(&d) {
                                    ok = false;
                                    break;
                                }
                                t += d;
                                pts.push((t, v.clamp(0.0, 1.0) as f32));
                            }
                            if ok {
                                c.pts = pts;
                            }
                        }
                    }
                }
            }
            233 => {
                let size = [60usize, 32, 12].into_iter().find(|s| blob.len() % s == 0 && blob.len() >= *s && u16le(blob, 4) as u32 == ITEM_BASE);
                if let Some(s) = size {
                    for it in blob.chunks_exact(s) {
                        if u16le(it, 4) as u32 == ITEM_BASE {
                            items.push(Item { pos: u32le(it, 0), index: u16le(it, 6) as u32, len: u32le(it, 8) });
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let mut out = vec![];
    for (id, c) in &chans {
        if c.ctype != 5 || c.pts.is_empty() {
            continue;
        }
        let mut pts: Vec<(f64, f32)> = vec![];
        let mut placed: Vec<&Item> = items.iter().filter(|it| it.index == *id as u32).collect();
        placed.sort_by_key(|it| it.pos);
        for it in placed {
            let (s, l) = (it.pos as f64 / ppq, (it.len as f64 / ppq).max(0.0));
            for (k, (t, v)) in c.pts.iter().enumerate() {
                if *t > l + 1e-9 {
                    // the clip is longer than the item: end on the value it has there
                    if k > 0 {
                        let (t0, v0) = c.pts[k - 1];
                        let f = ((l - t0) / (t - t0).max(1e-9)) as f32;
                        pts.push((s + l, v0 + (v - v0) * f.clamp(0.0, 1.0)));
                    }
                    break;
                }
                pts.push((s + t, *v));
            }
        }
        if pts.is_empty() {
            continue;
        }
        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
        let name = if !c.display.is_empty() { c.display.clone() } else if !c.name.is_empty() { c.name.clone() } else { format!("Automation {}", id + 1) };
        let (target, param) = match name.split_once(" - ") {
            Some((a, b)) => (a.trim().to_string(), b.trim().to_string()),
            None => (String::new(), name.clone()),
        };
        out.push(FlAuto { name, target, param, points: pts });
    }
    out
}

const PARAM_SYN: &[(&str, &[&str])] = &[
    ("cutoff", &["cutoff", "cut", "freq", "frequency", "filter", "fc", "brightness", "lpf", "hpf"]),
    ("reso", &["reso", "res", "resonance", "q", "emphasis"]),
    ("attack", &["attack", "att", "atk"]),
    ("decay", &["decay", "dec"]),
    ("sustain", &["sustain", "sus"]),
    ("release", &["release", "rel"]),
    ("drive", &["drive", "dist", "distortion", "saturation", "sat", "overdrive", "crush", "bitcrush"]),
    ("chorus", &["chorus", "ensemble"]),
    ("delay", &["delay", "echo"]),
    ("reverb", &["reverb", "verb", "room", "space"]),
    ("rate", &["rate", "speed", "lfo"]),
    ("depth", &["depth", "amount", "amt", "intensity"]),
    ("detune", &["detune", "spread", "unison"]),
    ("width", &["width", "stereo"]),
    ("mix", &["mix", "wet", "dry", "blend"]),
    ("wave", &["wave", "wavetable", "wt", "position", "morph", "shape", "table", "timbre"]),
    ("pitch", &["pitch", "tune", "transpose", "fine", "coarse"]),
    ("glide", &["glide", "porta", "portamento"]),
    ("volume", &["volume", "vol", "gain", "level", "master"]),
    ("pan", &["pan", "panning", "balance"]),
    ("noise", &["noise"]),
    ("sub", &["sub"]),
    ("fm", &["fm"]),
];

/// The words of a parameter name in a common vocabulary (FL's "Filter Cutoff", "Mod X", "Cut" and Ling's "cutoff" all become "cutoff").
pub fn param_words(s: &str) -> Vec<String> {
    let l = s.to_lowercase();
    let mut v: Vec<String> = vec![];
    if l.contains("mod x") {
        v.push("cutoff".into());
    }
    if l.contains("mod y") {
        v.push("reso".into());
    }
    for w in l.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()) {
        for (canon, words) in PARAM_SYN {
            if words.contains(&w) && !v.iter().any(|x| x == canon) {
                v.push(canon.to_string());
            }
        }
    }
    v
}

/// The Ling parameter an FL automation of `fl_param` should move on an instrument with `params`: the one sharing the most words,
/// else the track's own volume / pan / pitch bend. `None` when nothing fits (the lane is kept unassigned).
pub fn map_param(fl_param: &str, params: &[&str]) -> Option<String> {
    let want = param_words(fl_param);
    if want.is_empty() {
        return None;
    }
    let mut best: Option<(usize, &str)> = None;
    for p in params {
        let have = param_words(p);
        let shared = want.iter().filter(|w| have.contains(w)).count();
        // a plain name beats an effect knob with the same word ("cutoff" before "fx cutoff")
        let score = shared * 2 + if shared > 0 && !p.starts_with("fx ") && !p.starts_with("bus ") { 1 } else { 0 };
        if shared > 0 && best.map(|b| score > b.0).unwrap_or(true) {
            best = Some((score, p));
        }
    }
    let is_vol = want.iter().any(|w| w == "volume");
    if let Some((_, p)) = best {
        // a plain volume automation goes to the track fader unless the instrument has a volume of its own
        if !(is_vol && want.len() == 1 && !param_words(p).iter().any(|w| w == "volume")) {
            return Some(p.to_string());
        }
    }
    if is_vol {
        return Some("vol".into());
    }
    if want.iter().any(|w| w == "pan") {
        return Some("pan".into());
    }
    if want.iter().any(|w| w == "pitch") {
        return Some("bend".into());
    }
    None
}

#[cfg(test)]
mod auto_tests {
    use super::*;
    #[test]
    fn fl_names_map_to_ling_params() {
        let synth = ["cutoff", "reso", "attack", "release", "chorus mix", "fx cutoff", "detune"];
        assert_eq!(map_param("Filter Cutoff", &synth).as_deref(), Some("cutoff"));
        assert_eq!(map_param("Multi Filter B Cut", &synth).as_deref(), Some("cutoff"));
        assert_eq!(map_param("Mod X", &synth).as_deref(), Some("cutoff"));
        assert_eq!(map_param("Resonance", &synth).as_deref(), Some("reso"));
        assert_eq!(map_param("Chorus Depth", &synth).as_deref(), Some("chorus mix"));
        assert_eq!(map_param("Volume", &synth).as_deref(), Some("vol"));
        assert_eq!(map_param("Panning", &synth).as_deref(), Some("pan"));
        assert_eq!(map_param("Wobble Juice", &synth), None);
    }

    #[test]
    fn automation_clips_of_real_projects() {
        // the user's projects (skipped where they are not on this machine)
        let p = "C:/Users/User/Music/Aurora/Aurora.flp";
        let Ok(d) = std::fs::read(p) else { return };
        let a = automation(&d);
        let cut = a.iter().find(|x| x.name == "SubSynth - Filter Cutoff").expect("the cutoff clip");
        assert_eq!(cut.target, "SubSynth");
        assert_eq!(cut.param, "Filter Cutoff");
        // 0 -> 1 over 64 beats, back to 0 at 128
        assert_eq!(cut.points.len(), 3);
        assert!((cut.points[1].0 - 64.0).abs() < 1e-6 && (cut.points[1].1 - 1.0).abs() < 1e-6, "{:?}", cut.points);
        assert!((cut.points[2].0 - 128.0).abs() < 1e-6);
    }
}
