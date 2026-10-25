//! Sound design for generated songs: what effects each part gets, how they are set, and how they move through the form.
//!
//! The plan follows how electronic music is mixed and designed:
//! * **bass** - the sub stays clean and mono (Imager below ~113 Hz, on every part), the mid-bass is distorted above a split (Split Drive: fold or
//!   clip for dubstep, tube for drum and bass / house) and glued by OTT; growl and wobble basses in dubstep and drum and bass get a
//!   comb tuned to the song's tonic in drops (it rings on the key's own notes, so it never fights the harmony) and a Ling Growl
//!   gets the Growl Technique skin in its drops (see growlskin.rs);
//! * **leads** - OTT for presence, width with a mono low end, a frequency-shift shimmer of a few Hz in drops (well under the point
//!   where it would sound out of tune);
//! * **pads / chords** - width (mono lows), a slow phaser in the quiet sections;
//! * **plucks / arps / drums** - transient punch; **risers** a jet flanger that speeds up through every build.
//!
//! And through the form: builds narrow the stereo image and push drive and OTT up, drops open everything (wide, full drive), breaks
//! and intros pull the dirt back, outros fade it. `amount` (the plan's `design` dial, 0..1) scales all of it; 0 adds nothing.
use crate::compose::Kind;

/// a section on the timeline, in beats
#[derive(Clone, Debug)]
pub struct Sec {
    pub tag: String,
    pub start: f64,
    pub end: f64,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Phase {
    Low,
    Mid,
    Build,
    Drop,
    Out,
}

pub fn phase(tag: &str) -> Phase {
    match tag {
        "build" | "buildup" | "pre" | "prechorus" | "pre-chorus" | "rise" => Phase::Build,
        "drop" | "chorus" | "hook" | "drop2" | "final" => Phase::Drop,
        "intro" | "break" | "breakdown" | "bridge" | "interlude" => Phase::Low,
        "outro" | "end" => Phase::Out,
        _ => Phase::Mid,
    }
}

/// one effect of a part's chain: its settings and the lanes that move it (parameter -> (beat, 0..1) points)
#[derive(Clone, Debug)]
pub struct FxPlan {
    pub name: &'static str,
    pub params: Vec<(&'static str, f32)>,
    pub lanes: Vec<(&'static str, Vec<(f64, f32)>)>,
}

#[derive(Clone, Debug, Default)]
pub struct PartDesign {
    pub chain: Vec<FxPlan>,
    /// the Growl Technique style for the drops ("riddim", "brostep", "neuro"), None = no skin
    pub skin: Option<&'static str>,
}

/// the dial's default per genre: full sound design for bass music, a light touch for other club music, gentle elsewhere, none for
/// acoustic genres
pub fn genre_design(genre: &str) -> f64 {
    match genre {
        "dubstep" | "drum and bass" | "hardstyle" => 0.85,
        "trance" | "house" | "techno" | "garage" | "hyperpop" | "trap" | "synthwave" | "witch house" => 0.6,
        "deep house" | "disco" | "dub" | "reggaeton" | "afrobeat" | "hip hop" | "pop" | "chiptune" => 0.4,
        "lofi" | "ambient" | "cinematic" => 0.25,
        _ => 0.0,
    }
}

fn bass_music(genre: &str) -> bool {
    matches!(genre, "dubstep" | "drum and bass")
}

/// a lane over the sections: `f(phase, 0..1 through the section)` at the start and end of each (builds ramp, the rest hold)
fn lane(secs: &[Sec], f: &dyn Fn(Phase, f64) -> f32) -> Vec<(f64, f32)> {
    let mut v = vec![];
    for s in secs {
        let p = phase(&s.tag);
        v.push((s.start, f(p, 0.0).clamp(0.0, 1.0)));
        v.push(((s.end - 0.01).max(s.start + 0.005), f(p, 1.0).clamp(0.0, 1.0)));
    }
    v
}

/// value at a phase: low / mid / build (from -> to) / drop / out (from -> to)
fn by_phase(p: Phase, x: f64, low: f32, mid: f32, drop: f32) -> f32 {
    match p {
        Phase::Low => low,
        Phase::Mid => mid,
        Phase::Drop => drop,
        Phase::Build => low + (drop - low) * (x as f32).powf(1.5),
        Phase::Out => mid + (low * 0.7 - mid) * x as f32,
    }
}

/// the comb's `note` knob for the song's tonic (pitch class 0..11) in the bass register (MIDI 36..47)
pub fn comb_note(tonic: u8) -> f32 {
    let n = 36.0 + (tonic % 12) as f32;
    (n - 12.0) / 96.0
}

/// Plan the chain for one part. `role` as the plan names it, `inst` the instrument kind ("growl", "prism" ...), `has` the effects
/// the part already carries (a preset's own chain is kept and only moved, never doubled), `desc` the song's description.
pub fn design(genre: &str, desc: &str, role: &str, kind: Kind, inst: &str, amount: f64, tonic: u8, secs: &[Sec], has: &[String]) -> PartDesign {
    let a = amount.clamp(0.0, 1.0) as f32;
    let mut d = PartDesign::default();
    if a < 0.05 || role == "voice" || inst == "voice" || secs.is_empty() {
        return d;
    }
    let r = role.to_lowercase();
    let bassm = bass_music(genre);
    let mut add = |name: &'static str, params: Vec<(&'static str, f32)>, lanes: Vec<(&'static str, Vec<(f64, f32)>)>| {
        // a preset that already brought this effect keeps its settings; it is only automated
        let params = if has.iter().any(|h| h == name) { vec![] } else { params };
        d.chain.push(FxPlan { name, params, lanes });
    };
    match kind {
        Kind::Bass if r.contains("sub") => {
            // the sub: clean, mono, nothing else
            add("Imager", vec![("width", 0.0), ("mono below", 0.6)], vec![]);
        }
        Kind::Bass => {
            let (mode, base) = match genre {
                "dubstep" => (if desc.contains("riddim") { 0.667 } else { 0.333 }, 0.35 + 0.25 * a),
                "hardstyle" => (0.667, 0.35 + 0.25 * a),
                "drum and bass" => (0.0, 0.3 + 0.25 * a),
                _ => (0.0, 0.15 + 0.2 * a),
            };
            let drv = lane(secs, &|p, x| by_phase(p, x, base * 0.55, base * 0.85, (base * 1.15).min(0.85)));
            add("Split Drive", vec![("split", 0.3), ("drive", base), ("mode", mode), ("tone", 0.68), ("mix", 1.0), ("mono low", 1.0)], vec![("drive", drv)]);
            let depth = 0.3 + 0.45 * a;
            let ott = lane(secs, &|p, x| by_phase(p, x, depth * 0.6, depth * 0.85, depth));
            add("OTT", vec![("depth", depth), ("time", 0.3), ("upward", 0.6), ("downward", 0.8), ("out", 0.45)], vec![("depth", ott)]);
            if bassm && (r.contains("growl") || r.contains("wobble") || r.contains("reese")) && a > 0.4 {
                let mix = 0.15 + 0.2 * a;
                let cm = lane(secs, &|p, x| by_phase(p, x, 0.0, 0.0, mix) * if p == Phase::Build { x as f32 * 0.6 } else { 1.0 });
                add("Comb", vec![("note", comb_note(tonic)), ("fine", 0.5), ("feedback", 0.88), ("damp", 0.3), ("rate", 0.25), ("sweep", 0.1), ("stereo", 0.2), ("mix", 0.0)], vec![("mix", cm)]);
            }
            let w = lane(secs, &|p, x| match p {
                Phase::Build => 0.45 - 0.2 * x as f32,
                Phase::Drop => 0.55,
                Phase::Low => 0.4,
                _ => 0.45,
            });
            add("Imager", vec![("width", 0.45), ("mono below", 0.5)], vec![("width", w)]);
            if inst == "growl" && bassm && a > 0.35 {
                d.skin = Some(if genre == "drum and bass" { "neuro" } else if desc.contains("riddim") { "riddim" } else { "brostep" });
            }
        }
        Kind::Melody => {
            let depth = 0.2 + 0.3 * a;
            let ott = lane(secs, &|p, x| by_phase(p, x, depth * 0.6, depth * 0.85, depth));
            add("OTT", vec![("depth", depth), ("upward", 0.55), ("out", 0.47)], vec![("depth", ott)]);
            if bassm || genre == "hardstyle" || genre == "hyperpop" {
                // a few Hz of shimmer in drops: alive, never out of tune
                let mix = 0.12 + 0.18 * a;
                let fm = lane(secs, &|p, x| by_phase(p, x, 0.0, 0.0, mix) * if p == Phase::Build { x as f32 } else { 1.0 });
                add("Freq Shifter", vec![("shift", 0.52), ("fine", 0.5), ("stereo", 0.6), ("mix", 0.0)], vec![("mix", fm)]);
            }
            let w = lane(secs, &|p, x| match p {
                Phase::Build => 0.6 - 0.25 * x as f32,
                Phase::Drop => 0.72,
                Phase::Low => 0.55,
                _ => 0.6,
            });
            add("Imager", vec![("width", 0.6), ("mono below", 0.5)], vec![("width", w)]);
        }
        Kind::Pad | Kind::Chords if r == "riser" || r.contains("riser") => {
            // the jet: the flanger speeds up through every build
            let rate = lane(secs, &|p, x| if p == Phase::Build { 0.15 + 0.55 * x as f32 } else { 0.15 });
            let mix = lane(secs, &|p, x| if p == Phase::Build { 0.2 + 0.4 * a * x as f32 } else { 0.2 });
            add("Flanger", vec![("delay", 0.25), ("depth", 0.7), ("rate", 0.15), ("feedback", 0.85), ("stereo", 0.6), ("mix", 0.25)], vec![("rate", rate), ("mix", mix)]);
            add("Imager", vec![("width", 0.6), ("mono below", 0.5)], vec![]);
        }
        Kind::Pad | Kind::Chords => {
            if matches!(genre, "synthwave" | "trance" | "ambient" | "house" | "deep house" | "dub" | "witch house" | "lofi" | "dubstep" | "drum and bass") {
                let mix = lane(secs, &|p, x| by_phase(p, x, 0.2 + 0.25 * a, 0.15 + 0.15 * a, 0.1 + 0.1 * a));
                add("Phaser", vec![("stages", 0.5), ("rate", 0.12), ("depth", 0.6), ("center", 0.45), ("feedback", 0.6), ("stereo", 0.6), ("mix", 0.2)], vec![("mix", mix)]);
            }
            // narrow through the build, wide in the drop
            let wide = 0.6 + 0.15 * a;
            let w = lane(secs, &|p, x| match p {
                Phase::Build => wide - (wide - 0.35) * x as f32,
                Phase::Drop => (wide + 0.1).min(0.9),
                _ => wide,
            });
            add("Imager", vec![("width", wide), ("mono below", 0.5)], vec![("width", w)]);
        }
        Kind::Arp => {
            add("Transient", vec![("attack", 0.65 + 0.1 * a), ("sustain", 0.45), ("speed", 0.6)], vec![]);
            add("Imager", vec![("width", 0.65 + 0.1 * a), ("mono below", 0.5)], vec![]);
        }
        Kind::Drums => {
            if genre_design(genre) >= 0.4 {
                add("Transient", vec![("attack", 0.6 + 0.1 * a), ("sustain", 0.45), ("speed", 0.55)], vec![]);
                add("Imager", vec![("width", 0.5), ("mono below", 0.5)], vec![]);
            }
        }
    }
    d
}

/// "Growl Bass: Split Drive, OTT, Comb, Imager (brostep skin in the drops)"
pub fn describe(title: &str, d: &PartDesign) -> String {
    let names: Vec<&str> = d.chain.iter().map(|f| f.name).collect();
    format!("{title}: {}{}", names.join(", "), d.skin.map(|s| format!(" ({s} skin in the drops)")).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs() -> Vec<Sec> {
        let mut v = vec![];
        let mut t = 0.0;
        for (tag, bars) in [("intro", 8.0), ("build", 8.0), ("drop", 16.0), ("break", 8.0), ("build", 8.0), ("drop", 16.0), ("outro", 8.0)] {
            v.push(Sec { tag: tag.into(), start: t, end: t + bars * 4.0 });
            t += bars * 4.0;
        }
        v
    }
    fn at(pts: &[(f64, f32)], beat: f64) -> f32 {
        let k = pts.iter().position(|p| p.0 > beat).unwrap_or(pts.len());
        if k == 0 {
            return pts[0].1;
        }
        if k == pts.len() {
            return pts[k - 1].1;
        }
        let (a, b) = (pts[k - 1], pts[k]);
        a.1 + (b.1 - a.1) * ((beat - a.0) / (b.0 - a.0)) as f32
    }

    #[test]
    fn a_dubstep_bass_gets_the_full_chain_and_it_moves_with_the_form() {
        let s = secs();
        let d = design("dubstep", "riddim dubstep", "growl bass", Kind::Bass, "growl", 0.85, 9, &s, &[]);
        let names: Vec<&str> = d.chain.iter().map(|f| f.name).collect();
        assert_eq!(names, vec!["Split Drive", "OTT", "Comb", "Imager"]);
        assert_eq!(d.skin, Some("riddim"));
        // every effect and lane names real parameters, every value is 0..1 and the lanes run forward
        for f in &d.chain {
            let real: Vec<&str> = crate::dsp::fx::defaults(f.name).iter().map(|p| p.0).collect();
            for (p, v) in &f.params {
                assert!(real.contains(p) && (0.0..=1.0).contains(v), "{} {p}={v}", f.name);
            }
            for (p, pts) in &f.lanes {
                assert!(real.contains(p), "{} lane {p}", f.name);
                assert!(pts.windows(2).all(|w| w[1].0 > w[0].0) && pts.iter().all(|x| (0.0..=1.0).contains(&x.1)), "{} {p}", f.name);
            }
        }
        let lane = |fx: &str, p: &str| d.chain.iter().find(|f| f.name == fx).unwrap().lanes.iter().find(|l| l.0 == p).unwrap().1.clone();
        // drive: low in the intro, climbing through the build, full in the drop
        let drv = lane("Split Drive", "drive");
        assert!(at(&drv, 4.0) < at(&drv, 60.0) && at(&drv, 60.0) <= at(&drv, 80.0));
        // the comb only in the drops; the image narrows into the drop and opens in it
        let cm = lane("Comb", "mix");
        assert!(at(&cm, 4.0) == 0.0 && at(&cm, 160.0) == 0.0 && at(&cm, 80.0) > 0.2);
        let w = lane("Imager", "width");
        assert!(at(&w, 62.0) < at(&w, 33.0) && at(&w, 80.0) > at(&w, 62.0));
        // the comb rings on the tonic (A = 9): MIDI 45
        let note = d.chain[2].params.iter().find(|p| p.0 == "note").unwrap().1;
        assert!(((12.0 + note * 96.0) - 45.0).abs() < 0.01);
    }

    #[test]
    fn the_sub_stays_clean_voices_and_acoustic_songs_are_left_alone_and_presets_are_not_doubled() {
        let s = secs();
        let sub = design("dubstep", "", "sub bass", Kind::Bass, "prism", 0.9, 0, &s, &[]);
        assert_eq!(sub.chain.len(), 1);
        assert_eq!(sub.chain[0].name, "Imager");
        assert!(design("dubstep", "", "voice", Kind::Melody, "voice", 0.9, 0, &s, &[]).chain.is_empty());
        assert!(design("folk", "", "lead", Kind::Melody, "synth", genre_design("folk"), 0, &s, &[]).chain.is_empty());
        // a preset chain that already has OTT: the OTT is kept (no settings), only automated
        let d = design("dubstep", "", "growl bass", Kind::Bass, "growl", 0.8, 0, &s, &["OTT".to_string()]);
        let ott = d.chain.iter().find(|f| f.name == "OTT").unwrap();
        assert!(ott.params.is_empty() && !ott.lanes.is_empty());
        // the lead's shimmer: a few Hz at most
        let lead = design("dubstep", "", "laser lead", Kind::Melody, "growl", 0.85, 0, &s, &[]);
        let fs = lead.chain.iter().find(|f| f.name == "Freq Shifter").unwrap();
        let shift = fs.params.iter().find(|p| p.0 == "shift").unwrap().1;
        assert!(crate::dsp::bassfx::shift_hz(shift).abs() < 5.0);
        assert!(genre_design("dubstep") > genre_design("house") && genre_design("house") > genre_design("folk"));
    }
}
