//! Is a part monophonic (one note at a time: a bassline, a lead, an 808, a sub layer, a single-line melody) or polyphonic (chords,
//! pads, stabs, keys, arps whose notes ring over each other)? And does a Ling Nova patch play one note at a time (its `voice mode`
//! is mono or legato) or chords (poly)? Every chooser of a Nova patch asks both (novagen `pick_for_part`, ai_import `choose_seeded`,
//! the recipes, the catalogue's palette for the model), and `guard_track` makes sure no chord part ends up on a mono-voice patch
//! (it would drop notes): the import, New AI Song, the Medley maker and the import producer run it on every Nova track.
//!
//! The measure ignores the small overlaps of legato playing: a note that runs into the next one by at most a 1/16 note (and at most
//! half its own length) is cut where the next one starts; notes that start together are chord tones and are never cut.
use crate::model::{ClipData, Note, Track};

/// how a part plays, measured from its notes
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Poly {
    /// notes counted
    pub notes: usize,
    /// the most notes sounding at once (after the legato overlaps are cut)
    pub max: usize,
    /// share of the time something sounds during which two or more notes sound
    pub share: f32,
    /// share of the onsets that land while another note sounds (or start together with one): chord tones
    pub onsets: f32,
}

impl Poly {
    /// a chord part: more than one note sounds for a real share of the time or of the onsets
    pub fn is_poly(&self) -> bool {
        self.max >= 2 && (self.share >= 0.10 || self.onsets >= 0.08)
    }
    /// "mono", "poly", or "none" (no notes)
    pub fn class(&self) -> &'static str {
        if self.notes == 0 {
            "none"
        } else if self.is_poly() {
            "poly"
        } else {
            "mono"
        }
    }
    /// Some(true) = poly, Some(false) = mono, None = no notes
    pub fn wants_poly(&self) -> Option<bool> {
        (self.notes > 0).then(|| self.is_poly())
    }
    /// words for the model and the reason lines: "mono (one note at a time)", "poly (up to 4 notes at once, 62% of the time)"
    pub fn words(&self) -> String {
        match self.class() {
            "none" => "no notes".into(),
            "mono" if self.max <= 1 => "mono (one note at a time)".into(),
            "mono" => "mono (one note at a time, a rare overlap)".into(),
            _ => format!("poly (up to {} notes at once, {:.0}% of the time)", self.max, self.share * 100.0),
        }
    }
}

/// the 1/16-note legato tolerance, in beats
const LEGATO: f64 = 0.25;
/// notes starting closer than this (beats) start together
const TOGETHER: f64 = 0.02;

/// measure (start, length) pairs in beats
pub fn measure_spans(spans: &[(f64, f64)]) -> Poly {
    let mut v: Vec<(f64, f64)> = spans.iter().copied().filter(|(s, l)| s.is_finite() && l.is_finite() && *l > 0.0).collect();
    if v.is_empty() {
        return Poly::default();
    }
    v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    // cut the legato overlaps: the next note that starts after this one (not with it)
    let n = v.len();
    let mut ends: Vec<f64> = v.iter().map(|(s, l)| s + l).collect();
    for i in 0..n {
        let (s, l) = v[i];
        if let Some(j) = (i + 1..n).find(|&j| v[j].0 > s + TOGETHER) {
            let next = v[j].0;
            let over = ends[i] - next;
            if over > 0.0 && over <= LEGATO.min(l * 0.5) {
                ends[i] = next;
            }
        }
    }
    // sweep: ends before starts at one instant
    let mut ev: Vec<(f64, i32)> = Vec::with_capacity(n * 2);
    for i in 0..n {
        ev.push((v[i].0, 1));
        ev.push((ends[i].max(v[i].0 + 1e-6), -1));
    }
    ev.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)));
    let (mut cur, mut max, mut last) = (0i32, 0i32, ev[0].0);
    let (mut any, mut many, mut chord_on) = (0f64, 0f64, 0usize);
    // onsets at one instant: every one of a group of two or more counts as a chord tone
    let mut k = 0;
    while k < ev.len() {
        let t = ev[k].0;
        let dt = t - last;
        if cur >= 1 {
            any += dt;
        }
        if cur >= 2 {
            many += dt;
        }
        last = t;
        // all events at this instant (within TOGETHER for the onsets)
        let mut starts = 0usize;
        while k < ev.len() && (ev[k].0 - t).abs() <= if ev[k].1 > 0 { TOGETHER } else { 0.0 } {
            cur += ev[k].1;
            if ev[k].1 > 0 {
                starts += 1;
            }
            k += 1;
        }
        if starts > 0 {
            if cur >= 2 {
                // the new notes sound over another (or with each other)
                chord_on += if (cur as usize) > starts { starts } else if starts >= 2 { starts } else { 0 };
            }
        }
        max = max.max(cur);
    }
    Poly { notes: n, max: max.max(0) as usize, share: if any > 0.0 { (many / any) as f32 } else { 0.0 }, onsets: chord_on as f32 / n as f32 }
}

/// measure notes (their own clock: beats)
pub fn measure(notes: &[Note]) -> Poly {
    measure_spans(&notes.iter().map(|n| (n.start, n.len)).collect::<Vec<_>>())
}

/// measure a track's MIDI clips (placed where they sit)
pub fn measure_track(tr: &Track) -> Poly {
    let mut v = vec![];
    for c in &tr.clips {
        if let ClipData::Midi(ns) = &c.data {
            v.extend(ns.iter().map(|n| (c.start + n.start, n.len)));
        }
    }
    measure_spans(&v)
}

/// What a song / import role usually plays: Some(true) chords (pads, chords, stabs, keys, piano, organ, strings, choir), Some(false)
/// one note at a time (basses, 808s, subs, leads, voices, brass and woodwind lines), None either (plucks, arps, bells, risers ...).
pub fn role_expect(role: &str) -> Option<bool> {
    let r = role.to_lowercase();
    if r.contains("pad") || r.contains("chord") || r.contains("stab") || matches!(r.as_str(), "keys" | "piano" | "organ" | "strings" | "choir" | "warm pad") {
        return Some(true);
    }
    if r.contains("bass") || r.contains("808") || r.contains("sub") || r.contains("lead") || matches!(r.as_str(), "voice" | "woodwind" | "brass") {
        return Some(false);
    }
    None
}

/// a Nova `voice mode` value (0..1 over poly / mono / legato) -> its name
pub fn mode_name(v: f32) -> &'static str {
    let m = crate::dsp::nova::layout::VOICE_MODES;
    m[((v.clamp(0.0, 1.0) * (m.len() - 1) as f32).round() as usize).min(m.len() - 1)]
}

/// is a Nova `voice mode` value one note at a time (mono or legato)?
pub fn mode_is_mono(v: f32) -> bool {
    mode_name(v) != "poly"
}

/// The voice mode a Nova patch file plays in ("poly", "mono", "legato"): its own `voice_mode=` line, else the factory preset its
/// name says, else poly (Nova's default). None when the file is not a Nova patch.
pub fn nova_file_mode(path: &str) -> Option<&'static str> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut kind = String::new();
    let mut mode: Option<f32> = None;
    let mut name = String::new();
    for l in text.lines() {
        let l = l.trim();
        let Some((k, v)) = l.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "type" => kind = v.trim_matches('"').to_string(),
            "name" => name = v.trim_matches('"').to_string(),
            "voice_mode" | "voice mode" => {
                mode = v.parse::<f32>().ok().or_else(|| crate::dsp::nova::layout::VOICE_MODES.iter().position(|m| m.eq_ignore_ascii_case(v)).map(|i| i as f32 / 2.0));
            }
            _ => {}
        }
    }
    if kind != "nova" {
        return None;
    }
    if let Some(m) = mode {
        return Some(mode_name(m));
    }
    // no line: the patch keeps Nova's default (poly) - the factory preset of that name says the same unless the file lost the line
    Some(crate::dsp::nova::presets::find(&name).map(|p| p.voice_mode()).unwrap_or("poly"))
}

/// Is the Nova sound a choice names mono-voiced? `plugin` "nova" + a preset name or a patch path, or a recipe id ("recipe:<id>" or
/// plugin "recipe"); None when it is not a Nova sound.
pub fn nova_choice_mono(plugin: &str, name_or_path: &str) -> Option<bool> {
    let v = name_or_path.trim();
    if plugin == "recipe" || v.starts_with("recipe:") {
        let id = v.strip_prefix("recipe:").unwrap_or(v);
        let lib = crate::recipes::library();
        let r = lib.get(id)?;
        return recipe_mono(r);
    }
    if plugin != "nova" && !v.to_lowercase().contains("nova") {
        return None;
    }
    if v.ends_with(".ling") {
        if let Some(m) = nova_file_mode(v) {
            return Some(m != "poly");
        }
    }
    let stem = std::path::Path::new(v).file_stem().map(|s| s.to_string_lossy().replace('_', " ")).unwrap_or_else(|| v.to_string());
    let name = stem.strip_prefix("Nova ").unwrap_or(&stem).to_string();
    crate::dsp::nova::presets::find(&name).or_else(|| crate::dsp::nova::presets::find(v)).map(|p| p.is_mono())
}

/// a Nova recipe's voice: its `voice mode` parameter, else its base patch's; None when the recipe is not Nova
pub fn recipe_mono(r: &crate::recipes::Recipe) -> Option<bool> {
    if r.instrument != "nova" {
        return None;
    }
    if let Some(v) = r.params.get("voice mode").or_else(|| r.params.get("voice_mode")) {
        return Some(mode_is_mono(*v));
    }
    if r.base_patch.is_empty() {
        return Some(false);
    }
    let alt = std::path::Path::new("../../../LingStationX").join(&r.base_patch);
    let path = if std::path::Path::new(&r.base_patch).is_file() { r.base_patch.clone() } else { alt.to_string_lossy().into_owned() };
    match nova_file_mode(&path) {
        Some(m) => Some(m != "poly"),
        None => nova_choice_mono("nova", &r.base_patch),
    }
}

/// may a sound of this voice play a part of this polyphony? A mono-voiced sound never plays a chord part (it would drop notes)
pub fn fits(mono_sound: bool, poly_part: Option<bool>) -> bool {
    !(mono_sound && poly_part == Some(true))
}

/// how well: 2 = the voice matches the part, 1 = no preference (unknown part, or a poly sound on a mono line), 0 = it does not fit
pub fn score(mono_sound: bool, poly_part: Option<bool>) -> u8 {
    match (mono_sound, poly_part) {
        (true, Some(true)) => 0,
        (true, Some(false)) | (false, Some(true)) => 2,
        _ => 1,
    }
}

/// The guard: a Nova track whose notes play chords on a mono / legato voice gets its voice mode set to poly (the patch otherwise
/// drops every note but one of each chord). Returns what was done ("" = nothing).
pub fn guard_track(tr: &mut Track) -> String {
    if tr.inst_kind != "nova" {
        return String::new();
    }
    let p = measure_track(tr);
    if !p.is_poly() {
        return String::new();
    }
    let Some(inst) = tr.inst.as_mut() else { return String::new() };
    let Some(v) = inst.get_param("voice mode") else { return String::new() };
    if !mode_is_mono(v) {
        return String::new();
    }
    let was = mode_name(v);
    inst.set_param("voice mode", 0.0);
    format!("{}: voice mode {was} -> poly (the part plays chords: {})", tr.name, p.words())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: f64, l: f64, k: u8) -> Note {
        Note::new(s, l, k, 0.8)
    }

    #[test]
    fn a_bassline_is_mono_even_with_legato_overlaps() {
        // 1/8 notes that each run 0.1 beat into the next one (legato), and a slide note overlapping a 1/16
        let mut v: Vec<Note> = (0..32).map(|i| n(i as f64 * 0.5, 0.6, 36 + (i % 4) as u8)).collect();
        v.push(n(16.0, 1.25, 43));
        v.push(n(17.0, 1.0, 41));
        let p = measure(&v);
        assert_eq!(p.class(), "mono", "{p:?}");
        assert!(!p.is_poly());
        // held notes one after another
        let held: Vec<Note> = (0..8).map(|i| n(i as f64 * 4.0, 4.0, 28)).collect();
        assert_eq!(measure(&held).class(), "mono");
        assert_eq!(measure(&held).max, 1);
    }

    #[test]
    fn chords_and_overlapping_arps_are_poly() {
        let chords: Vec<Note> = (0..8).flat_map(|b| [52u8, 55, 59].into_iter().map(move |k| n(b as f64 * 4.0, 3.9, k))).collect();
        let p = measure(&chords);
        assert_eq!(p.class(), "poly");
        assert_eq!(p.max, 3);
        assert!(p.share > 0.9 && p.onsets > 0.9, "{p:?}");
        assert!(p.words().contains("up to 3 notes"));
        // an arp whose notes ring for a beat each on 1/4-beat steps
        let arp: Vec<Note> = (0..64).map(|i| n(i as f64 * 0.25, 1.0, 60 + (i % 4) as u8 * 4)).collect();
        assert!(measure(&arp).is_poly());
        // a bass with a held pedal under a moving line is two voices
        let mut ped: Vec<Note> = vec![n(0.0, 16.0, 24)];
        ped.extend((0..16).map(|i| n(i as f64, 0.9, 36 + (i % 3) as u8)));
        assert!(measure(&ped).is_poly());
    }

    #[test]
    fn a_lone_double_stop_does_not_make_a_line_poly() {
        let mut v: Vec<Note> = (0..64).map(|i| n(i as f64 * 0.5, 0.45, 72 + (i % 5) as u8)).collect();
        v.push(n(8.0, 0.45, 60));
        let p = measure(&v);
        assert_eq!(p.max, 2);
        assert_eq!(p.class(), "mono", "{p:?}");
        assert_eq!(measure(&[]).class(), "none");
    }

    #[test]
    fn roles_expect_a_polyphony() {
        assert_eq!(role_expect("warm pad"), Some(true));
        assert_eq!(role_expect("stab chord"), Some(true));
        assert_eq!(role_expect("keys"), Some(true));
        assert_eq!(role_expect("wobble bass"), Some(false));
        assert_eq!(role_expect("supersaw lead"), Some(false));
        assert_eq!(role_expect("pluck"), None);
        assert_eq!(role_expect("arp"), None);
        assert!(!fits(true, Some(true)) && fits(false, Some(true)) && fits(true, Some(false)) && fits(true, None));
        assert!(score(true, Some(false)) > score(false, Some(false)) && score(false, Some(true)) > score(true, Some(true)));
    }

    #[test]
    fn every_factory_preset_has_a_voice_class_from_its_voice_mode() {
        let all = crate::dsp::nova::presets::all();
        assert!(all.len() >= 100);
        for p in all {
            let r = p.usage.role.as_str();
            match r {
                // the colour basses play chords (future bass); every other bass is one note at a time
                "bass" => assert_eq!(p.is_mono(), p.usage.family != "colour", "{}", p.name),
                "lead" => assert!(p.is_mono(), "{} is a lead", p.name),
                "chord" | "pad" | "pluck" => assert!(!p.is_mono(), "{} plays chords", p.name),
                _ => {}
            }
            assert!(matches!(p.voice_class(), "mono" | "poly"));
        }
        assert_eq!(crate::dsp::nova::presets::find("Neon Supersaw").unwrap().voice_mode(), "legato");
        assert_eq!(crate::dsp::nova::presets::find("Clean Sub").unwrap().voice_mode(), "mono");
        assert_eq!(crate::dsp::nova::presets::find("Aurora Pad").unwrap().voice_mode(), "poly");
    }

    #[test]
    fn the_nova_chooser_matches_the_voice_to_the_part() {
        use crate::dsp::nova::presets::find;
        let mono = |n: Option<&str>| find(n.unwrap()).unwrap().is_mono();
        // a brass part: chords get the chord stab, a line gets a brass lead (the same family, the other voice)
        for seed in 0..12u64 {
            for part in 0..4 {
                assert_eq!(crate::novagen::pick_for_part("dubstep", "brass", part, seed, Some(true)), Some("Brass Chord Stab"));
                assert!(mono(crate::novagen::pick_for_part("dubstep", "brass", part, seed, Some(false))));
            }
        }
        // wherever a role has a poly preset, a chord part never gets a mono one
        let genres = ["dubstep", "riddim", "future bass", "house", "drum and bass", "trance", "trap", "pop", "synthwave", "ambient", "lofi", "techno"];
        let roles = ["growl bass", "wobble bass", "sub bass", "rolling bass", "supersaw lead", "synth lead", "brass", "stab chord", "chords", "keys", "pluck", "arp", "warm pad", "dark pad", "ambient pad"];
        for g in genres {
            for r in roles {
                for seed in [0u64, 3, 77] {
                    let Some(pc) = crate::novagen::pick_for_part(g, r, 0, seed, Some(true)) else { continue };
                    let any_poly = (0..40).filter_map(|k| crate::novagen::pick_for_part(g, r, k, seed, None)).any(|n| !find(n).unwrap().is_mono());
                    if any_poly {
                        assert!(!find(pc).unwrap().is_mono(), "{g} {r}: a chord part got {pc}");
                    }
                }
            }
        }
        // the import chooser: a chord part named "Lead" is not left on a mono voice, a bassline keeps a mono bass
        use crate::midi_import::{ImportedMidi, ImportedTrack};
        let mk = |name: &str, program: Option<u8>, notes: Vec<Note>| ImportedTrack { named: true, plugin: String::new(), name: name.into(), program, bank: 0, channel: 0, notes, volume: 1.0, pan: 0.0, vol_auto: vec![], pan_auto: vec![], bend_auto: vec![], bend_range: 2.0, samples: vec![], sampler: None, percussive: false, extra_auto: vec![] };
        let bass: Vec<Note> = (0..64).map(|i| n(i as f64 * 0.5, 0.55, 36 + (i % 4) as u8)).collect();
        let chords: Vec<Note> = (0..16).flat_map(|b| [60u8, 64, 67, 71].into_iter().map(move |k| n(b as f64 * 2.0, 1.9, k))).collect();
        let brass: Vec<Note> = (0..16).flat_map(|b| [55u8, 60, 64].into_iter().map(move |k| n(b as f64 * 2.0, 0.5, k))).collect();
        let mid = ImportedMidi { tracks: vec![mk("Bass", Some(38), bass), mk("Lead Chords", Some(81), chords), mk("Brass", Some(61), brass)], bpm: Some(140.0), tempos: vec![], length_beats: 32.0 };
        let reply = crate::ai_import::parse_reply(r#"{"parts":[{"part":0,"role":"wobble bass"},{"part":1,"role":"supersaw lead"},{"part":2,"role":"brass"}]}"#);
        let c = crate::ai_import::choose_seeded(&mid, &reply, "dubstep", 5);
        assert!(c[0].why.contains("mono part, mono voice"), "{}", c[0].why);
        // the model's "supersaw lead" for a four-note chord part is not taken
        assert!(!c[1].role.contains("lead") && c[1].why.contains("guessed"), "{}: {}", c[1].role, c[1].why);
        assert!(!c[1].why.contains("mono voice"), "{}: {}", c[1].role, c[1].why);
        assert!(c[2].why.contains("poly part, poly voice"), "{}: {}", c[2].role, c[2].why);
        // the prompt gives the model the measured voices
        let d = crate::ai_import::describe("x.mid", &mid);
        assert!(d.contains("voices: mono (one note at a time") && d.contains("voices: poly (up to 4 notes at once"), "{d}");
    }

    #[test]
    fn voice_mode_values_name_their_mode() {
        assert_eq!(mode_name(0.0), "poly");
        assert_eq!(mode_name(0.5), "mono");
        assert_eq!(mode_name(1.0), "legato");
        assert!(mode_is_mono(0.5) && mode_is_mono(1.0) && !mode_is_mono(0.0));
    }

    #[test]
    fn the_guard_puts_a_chord_part_on_a_poly_voice() {
        let p = crate::dsp::nova::presets::find("Neon Supersaw").unwrap();
        assert!(p.is_mono(), "the supersaw lead is a legato voice");
        let mut inst = crate::dsp::nova::Nova::new(48000.0);
        inst.set_values(&p.vals);
        let mut tr = Track::new("Chords", crate::model::TrackKind::Instrument, 0xFFFFFF, Some(Box::new(inst)));
        tr.inst_kind = "nova".into();
        let chords: Vec<Note> = (0..4).flat_map(|b| [60u8, 64, 67].into_iter().map(move |k| n(b as f64 * 4.0, 3.9, k))).collect();
        let mut c = crate::model::Clip::midi(0.0, 16.0, "c");
        if let ClipData::Midi(v) = &mut c.data {
            v.extend(chords);
        }
        tr.clips.push(c);
        let said = guard_track(&mut tr);
        assert!(said.contains("legato -> poly"), "{said}");
        assert_eq!(tr.inst.as_ref().unwrap().get_param("voice mode"), Some(0.0));
        // a second run has nothing to do; a mono line keeps its mono voice
        assert!(guard_track(&mut tr).is_empty());
        let mut inst = crate::dsp::nova::Nova::new(48000.0);
        inst.set_values(&p.vals);
        let mut lead = Track::new("Lead", crate::model::TrackKind::Instrument, 0xFFFFFF, Some(Box::new(inst)));
        lead.inst_kind = "nova".into();
        let mut c = crate::model::Clip::midi(0.0, 16.0, "c");
        if let ClipData::Midi(v) = &mut c.data {
            v.extend((0..16).map(|i| n(i as f64, 0.9, 72)));
        }
        lead.clips.push(c);
        assert!(guard_track(&mut lead).is_empty());
        assert_eq!(lead.inst.as_ref().unwrap().get_param("voice mode"), Some(1.0));
    }
}
