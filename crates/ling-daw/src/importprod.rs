//! Producing an imported MIDI / FL Studio / tracker file (`daw_imp_*`, api/import_api.rs): the note-level work that comes before
//! the mix.
//!
//! * **Performance** ([`perform`]): how the imported parts are *played*. A style (tight, natural, live player, laid back, pushed,
//!   expressive, robotic) is a set of moves on the existing notes, scaled by an amount: a light quantize towards the 1/16 or the
//!   triplet grid, one shared timing feel for the whole band (every sixteenth has its own small offset that every part plays, so the
//!   bass stays on the kick; the bar line stays put), laid-back off-beats and snares, pushed parts ahead of the kick, velocity
//!   accents on the beat, a phrase arch over every four bars, chords rolled low to high, a length scale (legato / clipped) and a
//!   squeeze of the velocities towards the part's own mean (robotic: one velocity). The expressive and live-player styles also get
//!   the Articulator's lanes (api/artic_api.rs, done by the caller).
//! * **Clean-up** ([`clean`]): duplicate notes (the same key starting together) go, overlapping notes of one key are cut where the
//!   next one starts, notes past their clip's end are trimmed, stuck notes (held far longer than the part ever holds a note) end at
//!   the part's next onset, and slivers shorter than 1/64 beat grow to it.
//! * **Roles** ([`role_of`]): what a track is, in the import's role words (ai_import::ROLES) and as a mix role (mixai::Role).
//! * **Master** ([`master_preset`]): the Ling Master preset that suits a genre.
use crate::mixai::Role;
use crate::model::{ClipData, Note, Track};

/// the performance styles, as the import dialog lists them ("off" = the notes as written)
pub const STYLES: [&str; 8] = ["off", "tight", "natural", "live player", "laid back", "pushed", "expressive", "robotic"];

/// what a style does at amount 1 (beats for timing, 0..1 for velocity)
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spec {
    /// how far each note moves towards its grid line (1 = onto it)
    pub quant: f64,
    /// the band's shared timing offset per sixteenth (largest, beats)
    pub jitter: f64,
    /// melodic parts (and every drum but the kick) moved late (+) or early (-)
    pub shift: f64,
    /// off-beat sixteenths and eighths moved late (swing-like, laid back)
    pub offbeat: f64,
    /// snares and claps moved late
    pub backbeat: f64,
    /// random velocity spread
    pub spread: f32,
    /// downbeat / beat accents (and off-beats softer)
    pub accent: f32,
    /// velocities squeezed towards the part's mean (1 = all the same)
    pub squeeze: f32,
    /// the phrase arch over every four bars (softer at the ends, fuller in the middle)
    pub phrase: f32,
    /// melodic note lengths scaled (1 = as written)
    pub length: f64,
    /// a chord rolled low to high, per note (beats)
    pub roll: f64,
    /// the Articulator's lanes, its amount (0 = none)
    pub artic: f64,
}

/// the moves of a style ("off" or an unknown name = nothing)
pub fn spec(style: &str) -> Spec {
    let z = Spec { length: 1.0, ..Default::default() };
    match style.trim().to_lowercase().as_str() {
        "tight" => Spec { quant: 0.6, jitter: 0.002, spread: 0.02, accent: 0.03, squeeze: 0.35, ..z },
        "natural" => Spec { quant: 0.2, jitter: 0.008, spread: 0.05, accent: 0.04, phrase: 0.05, roll: 0.004, ..z },
        "live player" | "live" => Spec { jitter: 0.016, offbeat: 0.006, spread: 0.09, accent: 0.06, phrase: 0.08, roll: 0.008, artic: 0.35, ..z },
        "laid back" | "laid-back" => Spec { quant: 0.15, jitter: 0.01, shift: 0.012, offbeat: 0.02, backbeat: 0.015, spread: 0.05, accent: 0.03, squeeze: 0.1, length: 1.06, roll: 0.01, ..z },
        "pushed" | "pushing" => Spec { quant: 0.2, jitter: 0.008, shift: -0.01, spread: 0.05, accent: 0.07, length: 0.9, ..z },
        "expressive" => Spec { quant: 0.1, jitter: 0.01, spread: 0.07, accent: 0.08, phrase: 0.18, roll: 0.012, artic: 0.6, ..z },
        "robotic" => Spec { quant: 1.0, squeeze: 1.0, ..z },
        _ => z,
    }
}

/// a style's name as listed (the closest; "" and unknown -> "off")
pub fn style_name(s: &str) -> &'static str {
    let t = s.trim().to_lowercase();
    STYLES.iter().find(|x| **x == t).copied().or_else(|| STYLES.iter().find(|x| !t.is_empty() && x.starts_with(&t)).copied()).unwrap_or("off")
}

fn mix64(a: u64, b: u64) -> u64 {
    let mut x = a ^ b.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// -1..1 from a hash
fn unit(h: u64) -> f64 {
    (h >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
}

fn is_drums(t: &Track) -> bool {
    t.inst_kind == "drums" || crate::feel::role_of(t) == crate::feel::Role::Drums
}

/// the nearest grid line for an absolute beat: the sixteenth or the eighth-note triplet, whichever is closer
fn grid_of(abs: f64) -> f64 {
    let g16 = (abs * 4.0).round() / 4.0;
    let g12 = (abs * 3.0).round() / 3.0;
    if (g12 - abs).abs() + 0.004 < (g16 - abs).abs() {
        g12
    } else {
        g16
    }
}

/// What the performance did, for the report.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Done {
    pub notes: usize,
    pub moved: usize,
    pub quantized: usize,
}

/// Play track `tr` in `style` at `amount` (0..1). `seed` is the band's: every track performed with the same seed shares the timing
/// feel of each sixteenth.
pub fn perform(tr: &mut Track, style: &str, amount: f64, seed: u64, part: usize) -> Done {
    let sp = spec(style);
    let amt = amount.clamp(0.0, 1.0);
    let mut d = Done::default();
    if amt <= 0.0 || sp == spec("off") {
        return d;
    }
    let drums = is_drums(tr);
    for c in tr.clips.iter_mut() {
        let (cs, cl) = (c.start, c.len);
        let ClipData::Midi(ns) = &mut c.data else { continue };
        if ns.is_empty() {
            continue;
        }
        let mean = ns.iter().map(|n| n.vel as f64).sum::<f64>() / ns.len() as f64;
        ns.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.key.cmp(&b.key)));
        let (mut order, mut last) = (0usize, f64::NAN);
        for n in ns.iter_mut() {
            d.notes += 1;
            let abs0 = cs + n.start;
            let mut abs = abs0;
            // a light quantize: only notes near a grid line (ornaments and free rubato stay)
            let g = grid_of(abs);
            let near = (g - abs).abs() < 0.125;
            if sp.quant > 0.0 && (near || sp.quant >= 0.999) {
                let q = (sp.quant * amt).min(1.0);
                if (g - abs).abs() > 1e-4 {
                    abs += (g - abs) * q;
                    d.quantized += 1;
                }
            }
            let step = (abs * 4.0).round().max(0.0) as i64;
            let on16 = (abs * 4.0 - step as f64).abs() < 0.03;
            let kick = drums && (n.key == 35 || n.key == 36);
            let snare = drums && matches!(n.key, 37 | 38 | 39 | 40);
            let mut off = unit(mix64(seed, step as u64)) * sp.jitter;
            if step % 2 == 1 && on16 {
                off += sp.offbeat;
            } else if step % 4 == 2 && on16 {
                off += sp.offbeat * 0.5;
            }
            if snare {
                off += sp.backbeat;
            }
            if !kick {
                off += sp.shift;
            }
            // the bar line is the band's anchor
            if step % 16 == 0 && on16 {
                off = if kick { 0.0 } else { sp.shift.min(0.0) * 0.5 };
            }
            if !drums && sp.roll > 0.0 {
                if (n.start - last).abs() < 0.02 {
                    order += 1;
                } else {
                    order = 0;
                    last = n.start;
                }
                off += order as f64 * sp.roll;
            }
            abs += off * amt;
            let ns_rel = (abs - cs).clamp(0.0, (cl - 1.0 / 64.0).max(0.0));
            if (ns_rel - n.start).abs() > 1e-5 {
                d.moved += 1;
            }
            n.start = ns_rel;
            // velocities: the part's own dynamics squeezed, accents on the beat, the phrase arch, a little scatter
            let mut v = n.vel as f64;
            v = mean + (v - mean) * (1.0 - (sp.squeeze as f64 * amt).min(1.0));
            let acc = if step % 16 == 0 { 1.0 } else if step % 4 == 0 { 0.5 } else if step % 2 == 1 { -0.8 } else { 0.0 };
            v += acc * sp.accent as f64 * amt;
            let ph = ((abs0 % 16.0) / 16.0).clamp(0.0, 1.0);
            v += ((std::f64::consts::PI * ph).sin() - 0.6) * sp.phrase as f64 * amt;
            let spread = if drums && matches!(n.key, 42 | 44 | 46 | 51) { sp.spread * 1.5 } else { sp.spread };
            v += unit(mix64(seed ^ (part as u64 + 1).wrapping_mul(0x51ED), ((step as u64) << 8) | n.key as u64)) * spread as f64 * amt;
            n.vel = v.clamp(0.05, 1.0) as f32;
            // lengths (melodic only): legato / clipped; robotic = onto the grid
            if !drums {
                if sp.quant >= 0.999 && sp.squeeze >= 0.999 {
                    let end = grid_of(n.start + n.len);
                    n.len = (end - n.start - 0.02).max(0.1);
                } else if (sp.length - 1.0).abs() > 1e-6 {
                    n.len = (n.len * (1.0 + (sp.length - 1.0) * amt)).max(1.0 / 64.0);
                }
            }
        }
        // a longer note never runs into the next one of the same key
        ns.sort_by(|a, b| a.start.total_cmp(&b.start));
        cut_overlaps(ns);
    }
    d
}

/// overlapping notes of one key: the earlier one ends where the next begins (returns how many were cut)
fn cut_overlaps(ns: &mut [Note]) -> usize {
    let mut cut = 0;
    for i in 0..ns.len() {
        let (k, s, e) = (ns[i].key, ns[i].start, ns[i].start + ns[i].len);
        if let Some(nx) = ns[i + 1..].iter().find(|x| x.key == k && x.start > s + 1e-6).map(|x| x.start) {
            if e > nx + 1e-6 {
                ns[i].len = (nx - s).max(1.0 / 64.0);
                cut += 1;
            }
        }
    }
    cut
}

/// What the clean-up found and fixed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cleaned {
    pub dupes: usize,
    pub overlaps: usize,
    pub stuck: usize,
    pub trimmed: usize,
    pub slivers: usize,
}
impl Cleaned {
    pub fn total(&self) -> usize {
        self.dupes + self.overlaps + self.stuck + self.trimmed + self.slivers
    }
    pub fn add(&mut self, o: &Cleaned) {
        self.dupes += o.dupes;
        self.overlaps += o.overlaps;
        self.stuck += o.stuck;
        self.trimmed += o.trimmed;
        self.slivers += o.slivers;
    }
}

/// Clean the notes of a track (see the module notes). Pads and drones may hold long notes: a note is stuck only when it is held
/// longer than 32 beats, or longer than 8 beats and six times the part's median length.
pub fn clean(tr: &mut Track) -> Cleaned {
    let mut r = Cleaned::default();
    let drums = is_drums(tr);
    for c in tr.clips.iter_mut() {
        let cl = c.len;
        let ClipData::Midi(ns) = &mut c.data else { continue };
        if ns.is_empty() {
            continue;
        }
        ns.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.key.cmp(&b.key)).then(b.vel.total_cmp(&a.vel)));
        // the same key starting together: keep the first (the loudest of them)
        let before = ns.len();
        let mut keep: Vec<Note> = Vec::with_capacity(before);
        for n in ns.drain(..) {
            let dup = keep.iter().rev().take_while(|k| n.start - k.start < 1.0 / 64.0).any(|k| k.key == n.key);
            if dup {
                if let Some(k) = keep.iter_mut().rev().find(|k| k.key == n.key) {
                    k.len = k.len.max(n.len);
                }
            } else {
                keep.push(n);
            }
        }
        r.dupes += before - keep.len();
        *ns = keep;
        r.overlaps += cut_overlaps(ns);
        let mut lens: Vec<f64> = ns.iter().map(|n| n.len).collect();
        lens.sort_by(f64::total_cmp);
        let med = lens[lens.len() / 2].max(0.25);
        let onsets: Vec<f64> = ns.iter().map(|n| n.start).collect();
        for n in ns.iter_mut() {
            if n.len < 1.0 / 64.0 {
                n.len = 1.0 / 64.0;
                r.slivers += 1;
            }
            let stuck = n.len > 32.0 || (n.len > 8.0 && n.len > med * 6.0) || (drums && n.len > 4.0);
            if stuck {
                let next = onsets.iter().copied().find(|s| *s > n.start + 0.05).unwrap_or(n.start + med.min(4.0));
                n.len = (next - n.start).clamp(1.0 / 64.0, if drums { 0.5 } else { med.max(1.0) * 2.0 });
                r.stuck += 1;
            }
            if cl > 0.0 && n.start + n.len > cl + 1e-6 && n.start < cl {
                n.len = (cl - n.start).max(1.0 / 64.0);
                r.trimmed += 1;
            }
        }
    }
    r
}

/// the median key and largest polyphony of a track's notes (for the role guess)
fn as_import(t: &Track) -> crate::midi_import::ImportedTrack {
    let notes: Vec<Note> = t
        .clips
        .iter()
        .flat_map(|c| match &c.data {
            ClipData::Midi(n) => n.iter().map(|x| Note::new(c.start + x.start, x.len, x.key, x.vel)).collect::<Vec<_>>(),
            _ => vec![],
        })
        .collect();
    crate::midi_import::ImportedTrack {
        named: true,
        plugin: String::new(),
        name: t.name.clone(),
        program: if is_drums(t) { None } else { Some(0) },
        bank: 0,
        channel: 0,
        notes,
        volume: t.volume,
        pan: t.pan,
        vol_auto: vec![],
        pan_auto: vec![],
        bend_auto: vec![],
        bend_range: 2.0,
        samples: vec![],
        sampler: None,
        percussive: false,
        extra_auto: vec![],
    }
}

/// a track's role in the import's words (ai_import::ROLES: "drums", "sub bass", "warm pad", "supersaw lead" ...), from its
/// instrument, name, register and playing; a Nova track says what its preset is for
pub fn role_word(t: &Track) -> &'static str {
    if matches!(t.kind, crate::model::TrackKind::Audio) {
        return "texture";
    }
    if t.inst_kind == "nova" {
        if let Some(p) = crate::novagen::preset_of_patch(&t.patch) {
            let fam = p.usage.family.as_str();
            return match p.usage.role.as_str() {
                "bass" if fam == "sub" => "sub bass",
                "bass" if matches!(fam, "wobble") => "wobble bass",
                "bass" if matches!(fam, "growl" | "neuro" | "riddim" | "talk" | "fm" | "colour" | "reese") => "growl bass",
                "bass" => "rolling bass",
                "lead" if fam == "supersaw" => "supersaw lead",
                "lead" if fam == "screech" => "laser lead",
                "lead" => "synth lead",
                "pluck" => "pluck",
                "chord" => "stab chord",
                "pad" => "warm pad",
                "fx" => "riser",
                _ => crate::ai_import::guess_role(&as_import(t)),
            };
        }
    }
    if t.inst_kind == "voice" {
        return "voice";
    }
    crate::ai_import::guess_role(&as_import(t))
}

/// the mix role (mixai) of an import role word
pub fn mix_role(word: &str) -> Role {
    match word {
        "drums" => Role::Drums,
        w if w.contains("bass") => Role::Bass,
        "vocal lead" | "voice" | "choir" => Role::Vocal,
        "supersaw lead" | "synth lead" | "laser lead" | "hyper lead" | "pluck" | "arp" | "woodwind" => Role::Lead,
        "warm pad" | "dark pad" | "ambient pad" | "strings" => Role::Pad,
        "stab chord" | "keys" | "piano" | "organ" | "guitar" | "brass" | "bell" => Role::Chords,
        "riser" | "texture" => Role::Fx,
        _ => Role::Other,
    }
}

/// the Nova preset role (novagen::ducker's words) a mix role ducks as; None = it does not duck
pub fn duck_role(r: Role, word: &str) -> Option<&'static str> {
    match r {
        Role::Bass => Some("bass"),
        Role::Pad => Some("pad"),
        Role::Chords => Some("chord"),
        Role::Lead if matches!(word, "pluck" | "arp") => Some("pluck"),
        Role::Lead => Some("lead"),
        _ => None,
    }
}

/// The Ling Master preset (dsp/master.rs MASTER_PRESETS) for a genre: slammed for bass music, loud and mono-safe for the club,
/// warm for lo-fi, gentle glue for acoustic and film music, streaming polish for the rest.
pub fn master_preset(genre: &str) -> &'static str {
    match genre.trim().to_lowercase().as_str() {
        "dubstep" | "riddim" | "drum and bass" | "neurofunk" | "hardstyle" | "trap" | "bass house" | "colour bass" | "breakbeat" => "Bass Music Slam",
        "house" | "deep house" | "techno" | "acid techno" | "garage" => "Mono-Safe Club",
        "trance" | "disco" | "hyperpop" | "future bass" | "big room" | "edm" => "Club Loud",
        "lofi" | "lo-fi" | "hip hop" | "witch house" => "Lo-Fi Warmth",
        "ambient" | "cinematic" | "orchestral" | "jazz" | "folk" | "ballad" => "Gentle Glue",
        "rock" | "pop" | "synthwave" | "chiptune" | "reggaeton" => "Streaming Polish (-14 LUFS)",
        _ => "Streaming Polish (-14 LUFS)",
    }
}

/// the mix style (brightness -1..1, punch 0..1, width 0..1) a genre wants from Mix AI
pub fn mix_style(genre: &str) -> (f64, f64, f64) {
    match genre.trim().to_lowercase().as_str() {
        "dubstep" | "riddim" | "drum and bass" | "hardstyle" | "trap" => (0.15, 0.8, 0.55),
        "house" | "deep house" | "techno" | "trance" | "disco" | "hyperpop" => (0.1, 0.7, 0.6),
        "lofi" | "jazz" | "folk" => (-0.3, 0.35, 0.45),
        "ambient" | "cinematic" | "orchestral" => (-0.1, 0.25, 0.75),
        "rock" => (0.0, 0.65, 0.5),
        _ => (0.0, 0.55, 0.55),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Clip, TrackKind};

    fn track(name: &str, kind: &str, notes: Vec<Note>) -> Track {
        let mut t = Track::new(name, TrackKind::Instrument, 0, None);
        t.inst_kind = kind.into();
        let mut c = Clip::midi(0.0, 64.0, "c");
        if let ClipData::Midi(v) = &mut c.data {
            v.extend(notes);
        }
        t.clips.push(c);
        t
    }
    fn notes(t: &Track) -> Vec<Note> {
        match &t.clips[0].data {
            ClipData::Midi(n) => n.clone(),
            _ => vec![],
        }
    }

    #[test]
    fn every_style_is_known_and_off_changes_nothing() {
        for s in STYLES {
            assert_eq!(style_name(s), s);
        }
        assert_eq!(style_name("laid"), "laid back");
        assert_eq!(style_name("???"), "off");
        let src: Vec<Note> = (0..32).map(|i| Note::new(i as f64 * 0.5 + 0.03, 0.4, 60 + (i % 5) as u8, 0.7)).collect();
        let mut t = track("Lead", "synth", src.clone());
        assert_eq!(perform(&mut t, "off", 1.0, 1, 0).moved, 0);
        assert_eq!(notes(&t).iter().map(|n| n.start).collect::<Vec<_>>(), src.iter().map(|n| n.start).collect::<Vec<_>>());
    }

    #[test]
    fn robotic_lands_on_the_grid_with_one_velocity_and_tight_pulls_towards_it() {
        let src: Vec<Note> = (0..32).map(|i| Note::new(i as f64 * 0.5 + if i % 2 == 0 { 0.03 } else { -0.02 }, 0.37, 60 + (i % 5) as u8, 0.4 + (i % 4) as f32 * 0.15)).collect();
        let mut r = track("Lead", "synth", src.clone());
        perform(&mut r, "robotic", 1.0, 7, 0);
        let rn = notes(&r);
        assert!(rn.iter().all(|n| ((n.start * 4.0) - (n.start * 4.0).round()).abs() < 1e-6 || ((n.start * 3.0) - (n.start * 3.0).round()).abs() < 1e-6), "{:?}", rn.iter().map(|n| n.start).collect::<Vec<_>>());
        let v0 = rn[0].vel;
        assert!(rn.iter().all(|n| (n.vel - v0).abs() < 1e-4));
        let mut t = track("Lead", "synth", src.clone());
        perform(&mut t, "tight", 1.0, 7, 0);
        let err = |v: &[Note]| v.iter().map(|n| (n.start - grid_of(n.start)).abs()).sum::<f64>();
        assert!(err(&notes(&t)) < err(&src) * 0.6, "tight should pull towards the grid");
    }

    #[test]
    fn laid_back_plays_the_snare_late_and_the_kick_on_time() {
        let kit: Vec<Note> = (0..16).flat_map(|b| [Note::new(b as f64, 0.1, 36, 0.9), Note::new(b as f64 + 0.5, 0.1, 38, 0.8)]).collect();
        let mut t = track("Drums", "drums", kit);
        perform(&mut t, "laid back", 1.0, 3, 0);
        let n = notes(&t);
        let kicks: Vec<&Note> = n.iter().filter(|x| x.key == 36).collect();
        let snares: Vec<&Note> = n.iter().filter(|x| x.key == 38).collect();
        assert!(kicks.iter().filter(|k| (k.start - k.start.round()).abs() < 1e-9).count() >= 4, "downbeat kicks stay on the bar line");
        let late = snares.iter().map(|s| s.start - (s.start * 2.0).round() / 2.0).sum::<f64>() / snares.len() as f64;
        assert!(late > 0.005, "snares late by {late}");
    }

    #[test]
    fn clean_up_removes_dupes_overlaps_and_stuck_notes() {
        let mut v: Vec<Note> = (0..16).map(|i| Note::new(i as f64, 0.5, 60, 0.8)).collect();
        v.push(Note::new(4.0, 0.5, 60, 0.6)); // a duplicate
        v.push(Note::new(8.0, 3.0, 62, 0.7)); // overlaps the next 62
        v.push(Note::new(9.0, 0.5, 62, 0.7));
        v.push(Note::new(2.0, 40.0, 64, 0.7)); // stuck
        v.push(Note::new(63.0, 4.0, 65, 0.7)); // past the clip end
        v.push(Note::new(20.0, 0.0001, 67, 0.7)); // a sliver
        let mut t = track("Keys", "synth", v);
        let r = clean(&mut t);
        assert_eq!(r.dupes, 1, "{r:?}");
        assert!(r.overlaps >= 1 && r.stuck == 1 && r.trimmed >= 1 && r.slivers == 1, "{r:?}");
        let n = notes(&t);
        assert!(n.iter().all(|x| x.len <= 16.0 && x.start + x.len <= 64.0 + 1e-6));
        assert_eq!(clean(&mut t).total(), 0, "a second pass finds nothing");
    }

    #[test]
    fn roles_and_master_presets_resolve() {
        let bass: Vec<Note> = (0..32).map(|i| Note::new(i as f64 * 0.5, 0.3, 36, 0.9)).collect();
        let t = track("Part 2", "synth", bass);
        assert!(role_word(&t).contains("bass"));
        assert_eq!(mix_role(role_word(&t)), Role::Bass);
        assert_eq!(duck_role(Role::Pad, "warm pad"), Some("pad"));
        let names: Vec<&str> = crate::dsp::master::MASTER_PRESETS.iter().map(|p| p.0).collect();
        for g in crate::ai_import::GENRES {
            assert!(names.contains(&master_preset(g)), "{g}");
        }
    }
}
