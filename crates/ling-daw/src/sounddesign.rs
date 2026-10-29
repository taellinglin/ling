//! Sound design for generated songs: what effects each part gets, how they are set, and how they move through the form.
//!
//! The plan follows how electronic music is mixed and designed:
//! * **bass** - the sub stays clean and mono (Imager below ~113 Hz, on every part), the mid-bass is distorted above a split (Split Drive: fold or
//!   clip for dubstep, tube for drum and bass / house) and glued by OTT; growl and wobble basses in dubstep and drum and bass get a
//!   comb tuned to the song's tonic in drops (it rings on the key's own notes, so it never fights the harmony) and a Ling Growl
//!   gets the Growl Technique skin in its drops (see growlskin.rs);
//! * **leads** - OTT for presence, width with a mono low end, a touch of saturation in the drops (never a frequency shifter, a chorus
//!   or any other pitch-altering effect: those exist only on a riser);
//! * **pads / chords** - width (mono lows), a slow phaser in the quiet sections;
//! * **plucks / arps / drums** - transient punch; **risers** a jet flanger that speeds up through every build.
//!
//! * **space** - how wet or dry the part is follows the genre (space.rs): breakbeat, drum and bass, techno and dubstep get a tight
//!   room at most, fast compressor timing and short echoes, with none on the kick and the bass; house, trance and pop sit in the
//!   middle; ambient, lofi and cinematic get long tails and slow envelopes. The instruments' own reverb / delay / chorus are limited to
//!   the same amounts (`PartDesign::inst`, applied with `apply_inst`).
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
    /// limits for the instrument's own reverb / delay / chorus / release and its vibrato / sweep (apply with `apply_inst`)
    pub inst: Vec<crate::space::InstTweak>,
    /// pitch-altering effects the part's preset brought that must be bypassed (bass: chorus, frequency shifter)
    pub bypass: Vec<&'static str>,
}

/// Apply the instrument tweaks and bypasses of a design to its track; returns how many values changed
pub fn apply_inst(tr: &mut crate::model::Track, pd: &PartDesign) -> usize {
    let mut n = 0;
    if let Some(inst) = tr.inst.as_mut() {
        n += crate::space::apply_tweaks(inst.as_mut(), &pd.inst);
    }
    for e in tr.effects.iter_mut() {
        if pd.bypass.iter().any(|b| *b == e.name) && !e.bypass {
            e.bypass = true;
            n += 1;
        }
    }
    n
}

/// the dial's default per genre: full sound design for bass music, a light touch for other club music, gentle elsewhere, none for
/// acoustic genres
pub fn genre_design(genre: &str) -> f64 {
    match genre {
        "dubstep" | "drum and bass" | "hardstyle" => 0.85,
        "breakbeat" => 0.7,
        "trance" | "house" | "techno" | "acid techno" | "future bass" | "garage" | "hyperpop" | "trap" | "synthwave" | "witch house" => 0.6,
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
    let g = crate::songgen::GENRES.iter().find(|g| g.name == genre).map(|g| g.bpm).unwrap_or(120.0);
    design_at(genre, desc, role, kind, inst, amount, tonic, secs, has, g)
}

/// `design` at the song's tempo (the echoes are synced to it)
pub fn design_at(genre: &str, desc: &str, role: &str, kind: Kind, inst: &str, amount: f64, tonic: u8, secs: &[Sec], has: &[String], bpm: f64) -> PartDesign {
    let a = amount.clamp(0.0, 1.0) as f32;
    let mut d = PartDesign::default();
    if a < 0.05 || role == "voice" || inst == "voice" || secs.is_empty() {
        return d;
    }
    let r = role.to_lowercase();
    let sp = crate::space::profile(genre);
    let srole = crate::space::SRole::from_plan(role, kind);
    let rs = sp.role(srole);
    let riser = r.contains("riser");
    let sub = kind == Kind::Bass && r.contains("sub");
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
                // bite in the drops from saturation: alive, and never a pitch effect
                let mix = 0.1 + 0.2 * a;
                let dr = lane(secs, &|p, x| by_phase(p, x, 0.0, 0.0, mix) * if p == Phase::Build { x as f32 } else { 1.0 });
                add("Drive", vec![("amount", 0.3 + 0.2 * a), ("mix", 0.0)], vec![("mix", dr)]);
            }
            let wk = rs.width / 0.6;
            let w = lane(secs, &|p, x| wk * match p {
                Phase::Build => 0.6 - 0.25 * x as f32,
                Phase::Drop => 0.72,
                Phase::Low => 0.55,
                _ => 0.6,
            });
            add("Imager", vec![("width", (0.6 * wk).min(1.0)), ("mono below", 0.5)], vec![("width", w)]);
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
            let wide = (rs.width + 0.1 * a).min(0.9);
            let w = lane(secs, &|p, x| match p {
                Phase::Build => wide - (wide - 0.35) * x as f32,
                Phase::Drop => (wide + 0.1).min(0.9),
                _ => wide,
            });
            add("Imager", vec![("width", wide), ("mono below", 0.5)], vec![("width", w)]);
        }
        Kind::Arp => {
            add("Transient", vec![("attack", (rs.punch + 0.1 * a).min(0.95)), ("sustain", rs.body), ("speed", 0.6)], vec![]);
            add("Imager", vec![("width", (rs.width + 0.1 * a).min(0.95)), ("mono below", 0.5)], vec![]);
        }
        Kind::Drums => {
            if genre_design(genre) >= 0.4 {
                add("Transient", vec![("attack", (rs.punch + 0.05 * a).min(0.95)), ("sustain", rs.body), ("speed", 0.55)], vec![]);
                add("Imager", vec![("width", rs.width), ("mono below", 0.5)], vec![]);
            }
        }
    }
    // the space: the compressor's timing, the tail of the room, the echo, all from the genre's profile
    if !sub && !riser {
        if matches!(srole, crate::space::SRole::Drums | crate::space::SRole::Bass) {
            let mut params = vec![("attack", rs.attack), ("release", rs.release)];
            if !has.iter().any(|h| h == "Compressor") {
                params.extend([("threshold", 0.72), ("ratio", 0.12), ("makeup", 0.55)]);
            }
            d.chain.push(FxPlan { name: "Compressor", params, lanes: vec![] });
        }
        let wet_lane = |base: f32| lane(secs, &|p, x| by_phase(p, x, base * 1.25, base, base * 0.85));
        if rs.reverb >= 0.03 {
            let lanes = if srole == crate::space::SRole::Drums { vec![] } else { vec![("mix", wet_lane(rs.reverb))] };
            d.chain.push(FxPlan { name: "Reverb", params: vec![("mix", rs.reverb), ("size", rs.size), ("damp", rs.damp)], lanes });
        }
        if rs.delay >= 0.03 {
            let t = crate::space::Space::delay_time_param(rs.delay_beats, bpm);
            d.chain.push(FxPlan { name: "Delay", params: vec![("time", t), ("feedback", rs.feedback), ("mix", rs.delay)], lanes: vec![("mix", wet_lane(rs.delay))] });
        }
    }
    // the instrument's own effects follow the same space; no vibrato or pitch sweep unless this is a riser
    d.inst = crate::space::inst_tweaks(&sp, srole, inst, riser);
    if kind == Kind::Bass {
        d.bypass = vec!["Chorus", "Freq Shifter", "Flanger"];
    }
    // no pitch-altering effect in an automatic chain, except the riser's jet flanger
    if !riser {
        d.chain.retain(|f| !crate::space::pitch_altering(f.name));
    }
    d
}

/// "Growl Bass: Split Drive, OTT, Comb, Imager (brostep skin in the drops)"
pub fn describe(title: &str, d: &PartDesign) -> String {
    let names: Vec<&str> = d.chain.iter().map(|f| f.name).collect();
    format!("{title}: {}{}", names.join(", "), d.skin.map(|s| format!(" ({s} skin in the drops)")).unwrap_or_default())
}

// ── expression: glide, vibrato, arpeggio and tuning, the way a producer sets them per part ─────────────────────────────
//
// Pitch modulation is never sprinkled on a patch (the pitch guard in redesign.rs keeps tweaks and redesigns away from it). The
// generator sets these on purpose, where a player or a producer would:
// * **glide** only on a monophonic legato line that wants it - the acid line's slides, a reese / growl / 808 bass sliding between held
//   notes, a mono lead in trance / future bass / synthwave - with a short, tempo-scaled time, and only the notes the writer chose
//   overlap (songgen `apply_slides`); every other note is detached so nothing glides by accident. Never chords, pads, plucks, arps,
//   keys or drums (their glide is set to 0).
// * **vibrato** only on long held notes of an expressive lead, a wind, brass or string line: 5..6.5 Hz, a fraction of a semitone,
//   fading in after a third of the note (an automation lane per note on the depth, so short notes stay plain). None on bass, pads in
//   club music, plucks, arps, chords; techno, dubstep, drum and bass and other machine music get none, ballads / lo-fi the most.
// * **arpeggios** live in the notes (tempo-locked: 1/16, 1/16 triplets, 1/8 triplets or 1/8 by genre and energy, the pattern by
//   energy) so they follow the chords and the tempo map; the instruments' own arpeggiators (Ling Chip, Ling Voice) are switched off so
//   nothing arpeggiates twice. Never on the bass; two arps in one section sit an octave apart (songgen `written`).
// * **tuning** stays on the key: unison detune is static (set into a range for supersaws and pads, capped for basses and the rest), no
//   LFO into pitch or detune, fine offsets snapped back, a stacked oscillator only on an octave, fifth or fourth, analog drift small
//   and only in vintage genres.

/// delayed vibrato for long notes
#[derive(Clone, Debug, PartialEq)]
pub struct Vibrato {
    /// Hz (5..6.5)
    pub rate_hz: f32,
    /// semitones, peak deviation
    pub depth: f32,
    /// where in the note it starts to fade in (0..1 of the note)
    pub onset: f64,
    /// only notes at least this long (beats) get it
    pub min_beats: f64,
}

/// how an arpeggio is written: tempo-locked step (beats), order, octaves
#[derive(Clone, Debug, PartialEq)]
pub struct ArpPlan {
    pub step: f64,
    pub order: &'static str,
    pub octaves: u32,
}

/// The expression plan of one part (instrument-independent; `expr_settings` maps it onto a plugin's parameters)
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Expr {
    /// a monophonic line (bass, lead): may slide
    pub mono: bool,
    /// portamento time for the notes written legato (seconds, 0 = none)
    pub glide_secs: f32,
    /// share of the eligible note pairs written legato (they glide)
    pub slides: f64,
    /// the largest interval a slide spans (semitones)
    pub slide_max: u8,
    /// the note a slide leaves must be at least this long (beats)
    pub slide_min: f64,
    /// the slides are written as per-note pitch gestures (the note bends into the next one, the notes stay detached: an 808 glide,
    /// a lead's slide) instead of legato overlaps the instrument glides on (kept for mono legato lines: 303, growl, reese)
    pub slide_gesture: bool,
    pub vibrato: Option<Vibrato>,
    /// the static unison detune range (min, max) the patch is put into; None = untouched
    pub detune: Option<(f32, f32)>,
    /// the largest analog drift allowed (0 = none)
    pub drift: f32,
    /// a riser may bend and sweep: nothing is neutralised
    pub riser: bool,
    /// the part carries a designed pitch gesture (the Growl Technique skin of a bass): its lanes stay
    pub keep_pitch_lanes: bool,
    pub why: Vec<String>,
}

/// genres with an analog / tape character: a little drift is part of the sound
pub fn vintage(genre: &str) -> bool {
    matches!(genre, "synthwave" | "lofi" | "disco" | "dub" | "jazz" | "hip hop" | "folk" | "ballad" | "rock" | "afrobeat" | "witch house")
}

/// how much vibrato a genre's leads and strings carry (0 = none: machine music)
pub fn vibrato_amount(genre: &str) -> f32 {
    match genre {
        "lofi" | "ballad" | "folk" | "jazz" | "cinematic" | "orchestral" => 1.0,
        "pop" | "synthwave" | "disco" | "afrobeat" | "reggaeton" | "hip hop" | "chiptune" | "rock" | "dub" | "ambient" => 0.7,
        "trance" | "future bass" | "house" | "deep house" | "hyperpop" | "garage" => 0.45,
        _ => 0.0,
    }
}

fn is_lead(r: &str) -> bool {
    r.contains("lead") || matches!(r, "woodwind" | "brass")
}

/// The expression of a part: `role` as the plan names it, its compose kind, the song's tempo.
pub fn expression(genre: &str, role: &str, kind: Kind, bpm: f64) -> Expr {
    let r = role.to_lowercase();
    let beat = 60.0 / bpm.clamp(40.0, 240.0);
    let mut x = Expr { slide_max: 7, slide_min: 0.75, ..Default::default() };
    if r.contains("riser") {
        x.riser = true;
        return x;
    }
    if r == "voice" || r == "choir" {
        // the singer's own glide, vibrato and scoops are the voice's (expression.rs performs them)
        x.why.push("the singer's own portamento and vibrato".into());
        return x;
    }
    let edm_lead_genre = matches!(genre, "trance" | "future bass" | "hardstyle" | "synthwave" | "hyperpop" | "chiptune");
    match kind {
        Kind::Bass => {
            x.mono = true;
            if r.contains("acid") || genre == "acid techno" {
                x.glide_secs = (0.2 * beat).min(0.08) as f32;
                x.slides = 0.3;
                x.slide_max = 12;
                x.slide_min = 0.0;
                x.why.push("303 slides on the tied notes".into());
            } else if r.contains("growl") || r.contains("wobble") || r.contains("reese") {
                x.glide_secs = (0.12 * beat) as f32;
                x.slides = 0.2;
                x.why.push("the bass slides between held notes".into());
            } else if matches!(genre, "trap" | "hip hop" | "future bass") && r.contains("sub") {
                x.glide_secs = (0.15 * beat) as f32;
                x.slides = 0.15;
                x.slide_max = 12;
                x.slide_min = 0.5;
                x.slide_gesture = true;
                x.why.push("808 glides (per-note pitch gestures)".into());
            }
            x.detune = Some((0.0, if r.contains("sub") { 0.1 } else if r.contains("growl") || r.contains("wobble") || r.contains("reese") { 1.0 } else { 0.2 }));
        }
        Kind::Melody => {
            x.mono = true;
            if edm_lead_genre && (r.contains("supersaw") || r.contains("synth lead") || r.contains("hyper lead")) {
                x.glide_secs = (0.08 * beat) as f32;
                x.slides = 0.15;
                x.slide_max = 5;
                x.slide_gesture = true;
                x.why.push("slides on the held lead notes (per-note pitch gestures)".into());
            }
            let amt = vibrato_amount(genre);
            // every melodic voice that holds notes sings them with a vibrato: leads, winds, brass, a solo string (an orchestra's
            // instruments by name); a laser zap does not
            if amt > 0.0 && !r.contains("laser") {
                let synth_lead = is_lead(&r) && !matches!(r.as_str(), "woodwind" | "brass");
                let base = if !synth_lead { 0.25 } else if genre == "chiptune" { 0.35 } else { 0.22 };
                let rate = match genre {
                    "lofi" | "ballad" | "folk" | "jazz" => 5.2,
                    "cinematic" | "orchestral" => 5.5,
                    "chiptune" => 6.5,
                    _ => 5.8,
                };
                x.vibrato = Some(Vibrato { rate_hz: rate, depth: base * amt, onset: 0.33, min_beats: (0.45 / beat).max(1.0) });
                x.why.push(format!("delayed vibrato on held notes ({rate:.1} Hz)"));
            }
            x.detune = Some((if r.contains("supersaw") { 0.2 } else { 0.0 }, if r.contains("supersaw") { 0.5 } else { 0.4 }));
        }
        Kind::Pad => {
            // strings in an expressive genre carry a gentle vibrato on their held notes; club pads none
            let amt = vibrato_amount(genre);
            if r == "strings" && amt >= 0.7 {
                x.vibrato = Some(Vibrato { rate_hz: 5.5, depth: 0.2 * amt, onset: 0.33, min_beats: (0.45 / beat).max(1.5) });
                x.why.push("string vibrato on held notes".into());
            }
            x.detune = Some((0.2, 0.5));
        }
        Kind::Chords | Kind::Arp => x.detune = Some((0.0, 0.5)),
        Kind::Drums => {}
    }
    x.drift = if vintage(genre) { 0.2 } else { 0.0 };
    x
}

/// The arpeggio of an arp / pluck part in a genre that sequences one, at a section's energy: a tempo-locked step and a pattern.
/// `None` = the genre does not arpeggiate on purpose (the composer's own rate, still a beat fraction, is kept).
pub fn arp_plan(genre: &str, energy: f64) -> Option<ArpPlan> {
    let e = energy.clamp(0.0, 1.0);
    let p = |step: f64, order: &'static str, octaves: u32| Some(ArpPlan { step, order, octaves });
    match genre {
        "trance" => {
            if e >= 0.75 {
                p(0.25, "updown", 2)
            } else if e >= 0.45 {
                p(0.25, "up", 1)
            } else {
                p(0.5, "up", 1)
            }
        }
        "synthwave" => if e >= 0.6 { p(0.25, "up", 2) } else { p(0.5, "updown", 1) },
        "chiptune" => if e >= 0.7 { p(1.0 / 6.0, "up", 2) } else { p(0.25, "updown", 1) },
        "techno" | "acid techno" => if e >= 0.5 { p(0.25, "up", 1) } else { p(0.5, "up", 1) },
        "future bass" | "hyperpop" | "pop" | "house" | "deep house" => if e >= 0.6 { p(1.0 / 3.0, "pingpong", 2) } else { p(0.5, "up", 1) },
        _ => None,
    }
}

/// The parameter settings of an expression plan on one instrument, from its current parameters (`(name, value)` as `params()`).
/// Returns the values to set and, when the vibrato is drawn per note, the depth parameter with its rest and peak values.
pub fn expr_settings(inst: &str, x: &Expr, params: &[(String, f32)]) -> (Vec<(String, f32)>, Option<(String, f32, f32)>) {
    let get = |n: &str| params.iter().find(|p| p.0 == n).map(|p| p.1);
    let mut set: Vec<(String, f32)> = vec![];
    let mut put = |set: &mut Vec<(String, f32)>, n: &str, v: f32| {
        if let Some(cur) = get(n) {
            if (cur - v).abs() > 1e-5 {
                set.push((n.to_string(), v.clamp(0.0, 1.0)));
            }
        }
    };
    if x.riser {
        return (set, None);
    }
    let g = x.glide_secs.max(0.0);
    // glide: the plugin's own time curve, solved for the seconds wanted (0 = off)
    match inst {
        "prism" => put(&mut set, "glide", if g > 0.0 { ((g - 0.002).max(0.0) / 0.8).sqrt() } else { 0.0 }),
        "synth" => put(&mut set, "glide", if g > 0.0 { ((g - 0.002).max(0.0) / 0.8).sqrt() } else { 0.0 }),
        "growl" => put(&mut set, "glid", if g > 0.0 { ((g - 0.005).max(0.0) / 0.35).sqrt() } else { 0.0 }),
        "chip" => {
            put(&mut set, "glide", if g > 0.0 { ((g - 0.003).max(0.0) / 0.6).sqrt() } else { 0.0 });
            if x.mono && g > 0.0 {
                put(&mut set, "poly", 0.0);
            }
        }
        "pluck" => put(&mut set, "glide", if g > 0.01 { (g - 0.01) / 0.5 } else { 0.0 }),
        _ => {}
    }
    // the instruments' own arpeggiators are off: the arpeggio is in the notes
    if inst == "chip" || inst == "voice" {
        put(&mut set, "arp", 0.0);
    }
    // static tuning: unison detune into the role's range, no fine offsets, stacked oscillators on consonant intervals, drift
    if let Some((lo, hi)) = x.detune {
        for n in ["a det", "b det", "detune", "detn"] {
            if let Some(c) = get(n) {
                let lo = if n == "b det" && get("b lvl").unwrap_or(0.0) < 0.05 { 0.0 } else { lo };
                let lo = if n == "detn" { 0.0 } else { lo };
                let cap = if inst == "chip" { hi.min(0.3) } else { hi };
                put(&mut set, n, c.clamp(lo, cap.max(lo)));
            }
        }
    }
    for n in ["a fine", "b fine"] {
        if let Some(c) = get(n) {
            if (c - 0.5).abs() > 0.03 {
                put(&mut set, n, 0.5);
            }
        }
    }
    if let Some(c) = get("b semi") {
        // (v - 0.5) * 48 semitones: snapped to unison, a fourth, a fifth or octaves
        let s = (c - 0.5) * 48.0;
        let ok = [-24.0f32, -19.0, -17.0, -12.0, -7.0, -5.0, 0.0, 5.0, 7.0, 12.0, 17.0, 19.0, 24.0];
        let best = ok.iter().copied().min_by(|a, b| (a - s).abs().total_cmp(&(b - s).abs())).unwrap_or(0.0);
        if (best - s).abs() > 0.05 {
            put(&mut set, "b semi", 0.5 + best / 48.0);
        }
    }
    if let Some(c) = get("drift") {
        if inst != "voice" {
            put(&mut set, "drift", c.min(x.drift));
        }
    }
    // no LFO into pitch or detune (a modulated detune / vibrato on every note is seasick)
    let lfo_pitch_route = |n: &str| -> bool {
        match inst {
            "prism" => ["x l1", "x l2"].iter().any(|p| n.starts_with(p)) && ["pit", "det", "bpt"].iter().any(|d| n.ends_with(d)),
            "growl" => n.len() == 5 && n.starts_with("m l") && matches!(n.as_bytes()[4], b'p' | b'd' | b'k' | b's'),
            _ => false,
        }
    };
    for (n, v) in params {
        if lfo_pitch_route(n) && (v - 0.5).abs() > 1e-3 {
            set.push((n.clone(), 0.5));
        }
    }
    // the plain vibratos of the patches are off; the planned one is drawn per note
    match inst {
        "synth" => put(&mut set, "lfo pitch", 0.0),
        "chip" => {
            put(&mut set, "vib", 0.0);
            put(&mut set, "sweep", 0.5);
        }
        "pluck" => put(&mut set, "vibrato", 0.0),
        _ => {}
    }
    let mut vib_lane = None;
    if let Some(v) = &x.vibrato {
        match inst {
            "synth" => {
                // one LFO serves cutoff, amplitude and pitch: only when the patch does not use it for the others
                let free = get("lfo cut").unwrap_or(0.0) < 0.002 && get("lfo amp").unwrap_or(0.0) < 0.002;
                if free {
                    put(&mut set, "lfo rate", ((v.rate_hz - 0.1) / 12.0).clamp(0.0, 1.0));
                    put(&mut set, "vib delay", 0.0);
                    vib_lane = Some(("lfo pitch".to_string(), 0.0, (v.depth / 0.865).clamp(0.0, 1.0)));
                }
            }
            "chip" => {
                put(&mut set, "vib rate", ((v.rate_hz - 0.5) / 11.5).clamp(0.0, 1.0));
                put(&mut set, "vib delay", 0.0);
                vib_lane = Some(("vib".to_string(), 0.0, v.depth.clamp(0.0, 1.0)));
            }
            "prism" => {
                // a free LFO (nothing routed from it) becomes the vibrato: sine, free-running, its pitch route drawn per note
                for l in ["l1", "l2"] {
                    let used = params.iter().any(|(n, val)| n.starts_with(&format!("x {l}")) && (val - 0.5).abs() > 0.004 && !lfo_pitch_route(n));
                    if used {
                        continue;
                    }
                    put(&mut set, &format!("{l} rate"), ((v.rate_hz / 0.05).log2() / 9.0).clamp(0.0, 1.0));
                    put(&mut set, &format!("{l} shp"), 0.0);
                    put(&mut set, &format!("{l} sync"), 0.0);
                    vib_lane = Some((format!("x {l}pit"), 0.5, (0.5 + v.depth / 48.0).clamp(0.5, 1.0)));
                    break;
                }
            }
            _ => {}
        }
    }
    (set, vib_lane)
}

/// The vibrato lane for a mono line: at rest everywhere, fading in from a third of each long note to its peak by about 60 % of it,
/// held to the end, back at rest before the next note. `notes` are (start, length) in beats on the timeline.
pub fn vibrato_lane(notes: &[(f64, f64)], v: &Vibrato, rest: f32, peak: f32) -> Vec<(f64, f32)> {
    let mut ns: Vec<(f64, f64)> = notes.to_vec();
    ns.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut pts: Vec<(f64, f32)> = vec![(0.0, rest)];
    let mut last = 0.0f64;
    for (i, &(s, len)) in ns.iter().enumerate() {
        if len < v.min_beats {
            continue;
        }
        // a note cut short by the next one only vibrates while it sounds alone
        let end = ns.get(i + 1).map(|n| n.0).unwrap_or(s + len).min(s + len);
        let a = s + v.onset * len;
        let b = s + (v.onset + 0.27) * len;
        if end - a < 0.25 || a <= last + 1e-6 {
            continue;
        }
        let b = b.min(end - 0.06);
        pts.push((a, rest));
        pts.push((b.max(a + 0.02), peak));
        pts.push(((end - 0.03).max(b.max(a + 0.02) + 0.01), peak));
        pts.push(((end - 0.03).max(b.max(a + 0.02) + 0.01) + 0.02, rest));
        last = pts.last().map(|p| p.0).unwrap_or(last);
    }
    pts
}

/// Put a part's expression on its track: the settings, the per-note vibrato lane (from the track's notes) and no lanes left on the
/// vibrato's rate. Returns what was done, in words ("" = nothing).
pub fn apply_expression(tr: &mut crate::model::Track, x: &Expr) -> String {
    let Some(inst) = tr.inst.as_mut() else { return String::new() };
    let params: Vec<(String, f32)> = inst.params().into_iter().map(|(n, v)| (n.to_string(), v)).collect();
    let (set, lane) = expr_settings(&tr.inst_kind, x, &params);
    for (n, v) in &set {
        inst.set_param(n, *v);
    }
    let mut said: Vec<String> = x.why.clone();
    // pitch is never automated by accident (an articulation pattern on a matrix cell into pitch, a detune wobble): only the planned
    // vibrato lane, a momentary bend, a riser's sweep and a designed skin stay
    if !x.riser && !x.keep_pitch_lanes {
        let before = tr.auto.len();
        tr.auto.retain(|l| l.param == "bend" || !crate::redesign::pitch_altering(&l.param));
        if tr.auto.len() < before {
            said.push(format!("{} pitch lane(s) removed", before - tr.auto.len()));
        }
    }
    if let (Some((p, rest, peak)), Some(v)) = (lane, x.vibrato.as_ref()) {
        let mut notes: Vec<(f64, f64)> = vec![];
        for c in &tr.clips {
            if let crate::model::ClipData::Midi(ns) = &c.data {
                notes.extend(ns.iter().filter(|n| n.start < c.len).map(|n| (c.start + n.start, n.len.min(c.len - n.start))));
            }
        }
        let pts = vibrato_lane(&notes, v, rest, peak);
        tr.auto.retain(|l| l.param != p && !(l.param.ends_with(" rate") && (l.param.starts_with('l') || l.param.starts_with("vib"))));
        if pts.len() > 1 {
            tr.auto.push(crate::model::AutoLane { param: p, points: pts });
        }
    } else if x.vibrato.is_some() {
        said.retain(|w| !w.contains("vibrato"));
    }
    said.join(", ")
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
        let names: Vec<&str> = d.chain.iter().map(|f| f.name).filter(|n| !matches!(*n, "Compressor" | "Reverb" | "Delay")).collect();
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
        let note = d.chain.iter().find(|f| f.name == "Comb").unwrap().params.iter().find(|p| p.0 == "note").unwrap().1;
        assert!(((12.0 + note * 96.0) - 45.0).abs() < 0.01);
    }

    #[test]
    fn the_sub_stays_clean_voices_and_acoustic_songs_are_left_alone_and_presets_are_not_doubled() {
        let s = secs();
        let sub = design("dubstep", "", "sub bass", Kind::Bass, "prism", 0.9, 0, &s, &[]);
        assert_eq!(sub.chain.len(), 1, "no compressor, reverb or delay on the sub");
        assert_eq!(sub.chain[0].name, "Imager");
        assert!(design("dubstep", "", "voice", Kind::Melody, "voice", 0.9, 0, &s, &[]).chain.is_empty());
        assert!(design("folk", "", "lead", Kind::Melody, "synth", genre_design("folk"), 0, &s, &[]).chain.is_empty());
        // a preset chain that already has OTT: the OTT is kept (no settings), only automated
        let d = design("dubstep", "", "growl bass", Kind::Bass, "growl", 0.8, 0, &s, &["OTT".to_string()]);
        let ott = d.chain.iter().find(|f| f.name == "OTT").unwrap();
        assert!(ott.params.is_empty() && !ott.lanes.is_empty());
        // the lead's bite in the drops is saturation, never a frequency shifter
        let lead = design("dubstep", "", "laser lead", Kind::Melody, "growl", 0.85, 0, &s, &[]);
        assert!(lead.chain.iter().any(|f| f.name == "Drive") && !lead.chain.iter().any(|f| f.name == "Freq Shifter"));
        assert!(genre_design("dubstep") > genre_design("house") && genre_design("house") > genre_design("folk"));
    }

    fn total(d: &PartDesign, name: &str, p: &str) -> f32 {
        d.chain.iter().filter(|f| f.name == name).filter_map(|f| f.params.iter().find(|x| x.0 == p)).map(|x| x.1).sum()
    }

    #[test]
    fn a_breakbeat_is_dryer_and_tighter_than_an_ambient_piece() {
        let s = secs();
        for (role, kind, inst) in [("laser lead", Kind::Melody, "prism"), ("warm pad", Kind::Pad, "prism"), ("pluck", Kind::Arp, "prism"), ("drums", Kind::Drums, "drums"), ("rolling bass", Kind::Bass, "prism")] {
            let b = design("breakbeat", "", role, kind, inst, 0.7, 0, &s, &[]);
            let a = design("ambient", "", role, kind, inst, 0.7, 0, &s, &[]);
            assert!(total(&b, "Reverb", "mix") <= total(&a, "Reverb", "mix"), "{role} reverb");
            assert!(total(&b, "Reverb", "size") <= total(&a, "Reverb", "size"), "{role} room");
            assert!(total(&b, "Delay", "mix") <= total(&a, "Delay", "mix"), "{role} delay");
            assert!(total(&b, "Delay", "feedback") <= total(&a, "Delay", "feedback"), "{role} feedback");
            assert!(total(&b, "Delay", "time") <= total(&a, "Delay", "time"), "{role} echo time");
            assert!(total(&b, "Compressor", "release") <= total(&a, "Compressor", "release"), "{role} release");
            assert!(total(&b, "Compressor", "attack") <= total(&a, "Compressor", "attack"), "{role} attack");
        }
        let b = design("breakbeat", "", "laser lead", Kind::Melody, "prism", 0.7, 0, &s, &[]);
        let a = design("ambient", "", "laser lead", Kind::Melody, "prism", 0.7, 0, &s, &[]);
        assert!(total(&a, "Reverb", "mix") > 3.0 * total(&b, "Reverb", "mix"), "{} {}", total(&a, "Reverb", "mix"), total(&b, "Reverb", "mix"));
        // the kick and the bass have no reverb and no echo in a dry genre
        for (role, kind) in [("drums", Kind::Drums), ("rolling bass", Kind::Bass)] {
            let d = design("breakbeat", "", role, kind, "prism", 0.7, 0, &s, &[]);
            assert!(total(&d, "Delay", "mix") == 0.0 && total(&d, "Reverb", "mix") < 0.03, "{role}");
        }
        // the instruments' own effects are limited more for the dry genre
        let lim = |d: &PartDesign, n: &str| d.inst.iter().find(|t| t.param == n).unwrap().v;
        assert!(lim(&b, "rv mix") < lim(&a, "rv mix") && lim(&b, "dl mix") < lim(&a, "dl mix"));
    }

    #[test]
    fn no_automatic_chain_alters_the_pitch_except_the_riser() {
        let s = secs();
        let roles = [("laser lead", Kind::Melody), ("warm pad", Kind::Pad), ("pluck", Kind::Arp), ("drums", Kind::Drums), ("growl bass", Kind::Bass), ("sub bass", Kind::Bass), ("stab chord", Kind::Chords), ("riser", Kind::Pad)];
        for g in crate::songgen::GENRES {
            for (role, kind) in roles {
                for inst in ["prism", "growl", "chip"] {
                    let d = design(g.name, "wobble riddim", role, kind, inst, 1.0, 3, &s, &[]);
                    for f in &d.chain {
                        if crate::space::pitch_altering(f.name) {
                            assert!(role == "riser" && f.name == "Flanger", "{} {role}: {}", g.name, f.name);
                        }
                    }
                    // vibrato and pitch sweeps are zeroed unless it is a riser
                    if inst == "chip" && role != "riser" {
                        assert!(d.inst.iter().any(|t| t.param == "vib" && t.v == 0.0) && d.inst.iter().any(|t| t.param == "sweep" && t.v == 0.5), "{} {role}", g.name);
                    }
                }
            }
        }
    }

    /// energy of the tail of one lead note (after the note-off) against the note itself, rendered through the designed chain
    fn tail_ratio(genre: &str) -> f32 {
        use crate::model::{Clip, ClipData, EffectSlot, Note, Track, TrackKind};
        let sr = 44100.0;
        let mut e = crate::engine::Engine::new(sr);
        e.project.bpm = 120.0;
        let mut t = Track::new("Lead", TrackKind::Instrument, 0, crate::dsp::make_instrument("synth", sr));
        let d = design_at(genre, "", "synth lead", Kind::Melody, "synth", 0.7, 0, &secs(), &[], 120.0);
        for f in d.chain.iter().filter(|f| matches!(f.name, "Reverb" | "Delay")) {
            let mut slot = EffectSlot::new(f.name, sr).unwrap();
            for (k, v) in &f.params {
                let i = slot.params.iter().position(|p| p.0 == *k).unwrap();
                slot.set(i, *v);
            }
            t.effects.push(slot);
        }
        let mut c = Clip::midi(0.0, 2.0, "n");
        c.data = ClipData::Midi(vec![Note::new(0.0, 1.0, 60, 0.9)]);
        t.clips.push(c);
        e.project.tracks.push(t);
        let buf = e.bounce_range(0.0, 2.0, 4.0);
        let at = |a: f32, b: f32| -> f32 { buf[(a * sr) as usize * 2..((b * sr) as usize * 2).min(buf.len())].iter().map(|x| x * x).sum::<f32>() };
        // the note is 0..0.5 s; the tail is measured from 1.5 s to 4 s, long after the instrument's own release
        at(1.5, 4.0) / at(0.0, 0.5).max(1e-9)
    }

    #[test]
    fn rendered_the_dry_genre_decays_faster_than_the_wet_one() {
        let (b, a) = (tail_ratio("breakbeat"), tail_ratio("ambient"));
        eprintln!("tail energy / note energy: breakbeat {b:.5}, ambient {a:.5}");
        assert!(a > 3.0 * b, "ambient tail {a} vs breakbeat {b}");
    }
}

#[cfg(test)]
mod expression_tests {
    use super::*;

    const GENRES: [&str; 12] = ["trance", "acid techno", "future bass", "dubstep", "lofi", "ballad", "breakbeat", "ambient", "techno", "synthwave", "chiptune", "trap"];
    const ROLES: [&str; 20] = ["drums", "sub bass", "rolling bass", "acid bass", "growl bass", "wobble bass", "supersaw lead", "synth lead", "laser lead", "woodwind", "brass", "warm pad", "dark pad", "strings", "stab chord", "keys", "piano", "pluck", "arp", "bell"];

    fn params(kind: &str) -> Vec<(String, f32)> {
        crate::dsp::make_instrument(kind, 44100.0).unwrap().params().into_iter().map(|(n, v)| (n.to_string(), v)).collect()
    }

    #[test]
    fn glide_only_on_mono_lines_that_slide_and_scaled_to_the_tempo() {
        for g in GENRES {
            for r in ROLES {
                let k = crate::songgen::role_kind(r);
                let x = expression(g, r, k, 128.0);
                if matches!(k, Kind::Chords | Kind::Pad | Kind::Arp | Kind::Drums) {
                    assert!(x.glide_secs == 0.0 && x.slides == 0.0 && !x.mono, "{g} {r}: no glide on chords, pads, plucks, arps, drums");
                }
                if x.glide_secs > 0.0 {
                    assert!(x.mono && x.slides > 0.0 && x.slides <= 0.35, "{g} {r}: only some notes slide");
                    assert!(x.glide_secs <= 0.12, "{g} {r}: a short glide, {}", x.glide_secs);
                }
            }
        }
        // the acid line slides (up to an octave), a reese bass slides between held notes, the trance lead legato; a house piano never
        let acid = expression("acid techno", "acid bass", Kind::Bass, 135.0);
        assert!(acid.glide_secs > 0.03 && acid.slide_max == 12 && acid.slide_min == 0.0);
        assert!(expression("dubstep", "growl bass", Kind::Bass, 140.0).glide_secs > 0.0);
        assert!(expression("trance", "supersaw lead", Kind::Melody, 138.0).glide_secs > 0.0);
        assert_eq!(expression("trance", "rolling bass", Kind::Bass, 138.0).glide_secs, 0.0, "a rolling sixteenth bass does not slide");
        // faster songs glide faster
        let (slow, fast) = (expression("dubstep", "growl bass", Kind::Bass, 100.0), expression("dubstep", "growl bass", Kind::Bass, 170.0));
        assert!(fast.glide_secs < slow.glide_secs);
        // on the instruments: chords / pads / arps get their glide set to 0, a sliding bass gets a glide in its own units
        let mut p = params("prism");
        p.iter_mut().find(|x| x.0 == "glide").unwrap().1 = 0.4;
        let (set, _) = expr_settings("prism", &expression("trance", "warm pad", Kind::Pad, 138.0), &p);
        assert!(set.iter().any(|(n, v)| n == "glide" && *v == 0.0), "{set:?}");
        let (set, _) = expr_settings("growl", &expression("dubstep", "growl bass", Kind::Bass, 140.0), &params("growl"));
        let gv = set.iter().find(|(n, _)| n == "glid").map(|x| x.1).unwrap_or(0.15);
        assert!((0.005 + gv * gv * 0.35 - (0.12 * 60.0 / 140.0) as f32).abs() < 0.005, "growl glide {gv}");
        let (set, _) = expr_settings("synth", &expression("acid techno", "acid bass", Kind::Bass, 135.0), &params("synth"));
        assert!(set.iter().any(|(n, v)| n == "glide" && *v > 0.1), "the acid synth slides: {set:?}");
    }

    #[test]
    fn vibrato_only_on_long_notes_of_expressive_leads_and_strings() {
        for g in GENRES {
            for r in ROLES {
                let k = crate::songgen::role_kind(r);
                let x = expression(g, r, k, 100.0);
                if let Some(v) = &x.vibrato {
                    assert!(matches!(k, Kind::Melody) || r == "strings", "{g} {r}: vibrato only on leads, winds, brass and strings");
                    assert!((5.0..=6.5).contains(&v.rate_hz) && v.depth > 0.0 && v.depth <= 0.4, "{g} {r}: {v:?}");
                    assert!(v.onset >= 0.25 && v.min_beats >= 1.0, "{g} {r}: delayed and only on held notes");
                    assert!(!r.contains("laser"));
                }
                if matches!(k, Kind::Bass | Kind::Arp | Kind::Chords | Kind::Drums) {
                    assert!(x.vibrato.is_none(), "{g} {r}");
                }
            }
        }
        for g in ["techno", "acid techno", "dubstep", "breakbeat", "trap"] {
            assert!(expression(g, "synth lead", Kind::Melody, 130.0).vibrato.is_none(), "{g}: machine music has no vibrato");
        }
        assert!(expression("trance", "strings", Kind::Pad, 138.0).vibrato.is_none(), "no vibrato on pads in club music");
        let d = |g: &str| expression(g, "woodwind", Kind::Melody, 90.0).vibrato.map(|v| v.depth).unwrap_or(0.0);
        assert!(d("ballad") > d("trance") && d("lofi") > d("future bass") && d("trance") > 0.0);
        // the lane: rest everywhere, rising only from a third into each long note
        let v = expression("ballad", "woodwind", Kind::Melody, 72.0).vibrato.unwrap();
        let notes = [(0.0, 0.5), (1.0, 3.0), (4.0, 0.5), (4.5, 0.75), (6.0, 4.0)];
        let lane = vibrato_lane(&notes, &v, 0.0, 0.3);
        assert!(lane.windows(2).all(|w| w[1].0 > w[0].0), "{lane:?}");
        let model = crate::model::AutoLane { param: "x".into(), points: lane.clone() };
        let at = |b: f64| model.value_at(b).unwrap();
        for b in [0.2, 0.9, 1.5, 1.9, 4.2, 4.9, 5.2, 6.5, 7.2] {
            assert!(at(b) < 1e-6, "no vibrato at beat {b} ({})", at(b));
        }
        assert!(at(3.5) > 0.29 && at(9.0) > 0.29, "the long notes vibrate");
        // where the instrument has no free modulator for it, there is no vibrato (nothing else is changed for it)
        let mut sp = params("synth");
        sp.iter_mut().find(|x| x.0 == "lfo cut").unwrap().1 = 0.4;
        let (set, lane) = expr_settings("synth", &expression("ballad", "woodwind", Kind::Melody, 72.0), &sp);
        assert!(lane.is_none() && !set.iter().any(|(n, _)| n == "lfo rate"), "{set:?}");
        let (set, lane) = expr_settings("synth", &expression("ballad", "woodwind", Kind::Melody, 72.0), &params("synth"));
        let (p, rest, peak) = lane.expect("the synth's lfo becomes the vibrato");
        assert!(p == "lfo pitch" && rest == 0.0 && peak > 0.1 && peak < 0.5);
        let rate = set.iter().find(|(n, _)| n == "lfo rate").unwrap().1;
        assert!((0.1 + rate * 12.0 - 5.2).abs() < 0.01);
        // Prism: a free LFO, sine, drawn on its pitch route
        let (set, lane) = expr_settings("prism", &expression("pop", "synth lead", Kind::Melody, 110.0), &params("prism"));
        let (p, rest, peak) = lane.unwrap();
        assert!(p == "x l1pit" && rest == 0.5 && peak > 0.5 && peak < 0.51, "{p} {peak}");
        let r = set.iter().find(|(n, _)| n == "l1 rate").unwrap().1;
        assert!((0.05 * 2f32.powf(r * 9.0) - 5.8).abs() < 0.05);
        // Chip: its own vibrato drawn per note, the delay knob off (the lane is the delay)
        let (set, lane) = expr_settings("chip", &expression("chiptune", "synth lead", Kind::Melody, 150.0), &params("chip"));
        assert_eq!(lane.unwrap().0, "vib");
        assert!(set.iter().any(|(n, v)| n == "vib delay" && *v == 0.0));
    }

    #[test]
    fn arps_are_tempo_locked_and_follow_the_energy() {
        for g in ["trance", "synthwave", "chiptune", "techno", "acid techno", "future bass", "pop", "house"] {
            let mut orders = vec![];
            for e in [0.2, 0.5, 0.8, 1.0] {
                let a = arp_plan(g, e).expect(g);
                // a whole number of steps per beat: 1/8, 1/8 triplets, 1/16, 1/16 triplets
                let per_beat = 1.0 / a.step;
                assert!((per_beat - per_beat.round()).abs() < 1e-9 && [2.0, 3.0, 4.0, 6.0].contains(&per_beat.round()), "{g} {e}: {}", a.step);
                orders.push((a.step, a.order, a.octaves));
            }
            assert!(orders.first() != orders.last(), "{g}: the pattern changes with the energy");
            // more energy never slows it down
            assert!(orders.windows(2).all(|w| w[1].0 <= w[0].0 + 1e-9), "{g}: {orders:?}");
        }
        assert!(arp_plan("ballad", 1.0).is_none() && arp_plan("dubstep", 1.0).is_none());
        // the instruments' own arpeggiators are switched off (the arp is in the notes)
        let mut cp = params("chip");
        cp.iter_mut().find(|x| x.0 == "arp").unwrap().1 = 0.6;
        let (set, _) = expr_settings("chip", &expression("chiptune", "arp", Kind::Arp, 150.0), &cp);
        assert!(set.iter().any(|(n, v)| n == "arp" && *v == 0.0));
    }

    #[test]
    fn tuning_is_static_and_in_key() {
        let mut p = params("prism");
        let set_p = |p: &mut Vec<(String, f32)>, n: &str, v: f32| p.iter_mut().find(|x| x.0 == n).unwrap().1 = v;
        set_p(&mut p, "x l1pit", 0.6);
        set_p(&mut p, "x l2det", 0.3);
        set_p(&mut p, "x l1cut", 0.7);
        set_p(&mut p, "x m1pit", 0.8);
        set_p(&mut p, "a fine", 0.6);
        set_p(&mut p, "b semi", 0.5 + 3.0 / 48.0);
        set_p(&mut p, "a det", 0.05);
        set_p(&mut p, "drift", 0.3);
        let (set, _) = expr_settings("prism", &expression("trance", "supersaw lead", Kind::Melody, 138.0), &p);
        let got = |n: &str| set.iter().find(|x| x.0 == n).map(|x| x.1);
        assert_eq!(got("x l1pit"), Some(0.5), "no LFO into pitch");
        assert_eq!(got("x l2det"), Some(0.5), "no LFO into detune");
        assert_eq!(got("x l1cut"), None, "a filter LFO stays");
        assert_eq!(got("x m1pit"), None, "an envelope into pitch (the attack of the sound) stays");
        assert_eq!(got("a fine"), Some(0.5), "no fine offset");
        let semi = (got("b semi").unwrap() - 0.5) * 48.0;
        assert!([0.0f32, 5.0, 7.0].iter().any(|s| (semi - s).abs() < 1e-3), "a stacked oscillator on a consonant interval: {semi}");
        assert!(got("a det").unwrap() >= 0.2, "a supersaw has its unison spread");
        assert_eq!(got("drift"), Some(0.0), "no drift in trance");
        let (set, _) = expr_settings("prism", &expression("synthwave", "warm pad", Kind::Pad, 104.0), &p);
        assert_eq!(set.iter().find(|x| x.0 == "drift").map(|x| x.1), Some(0.2), "a little drift in a vintage genre");
        // a sub bass is not spread; nothing a part sets is a lane (static), and every name is a real parameter of the plugin
        let (set, _) = expr_settings("prism", &expression("trap", "sub bass", Kind::Bass, 140.0), &params("prism"));
        assert!(set.iter().find(|x| x.0 == "a det").map(|x| x.1).unwrap_or(1.0) <= 0.1);
        for kind in ["synth", "growl", "prism", "pluck", "chip", "sampler", "voice", "drums"] {
            let ps = params(kind);
            for g in GENRES {
                for r in ROLES {
                    let (set, lane) = expr_settings(kind, &expression(g, r, crate::songgen::role_kind(r), 120.0), &ps);
                    for (n, v) in &set {
                        assert!(ps.iter().any(|p| &p.0 == n) && (0.0..=1.0).contains(v), "{kind} {g} {r}: {n}={v}");
                    }
                    if let Some((n, _, _)) = lane {
                        assert!(ps.iter().any(|p| p.0 == n), "{kind}: lane {n}");
                    }
                }
            }
        }
        // a riser is left alone (it may sweep and bend)
        let (set, _) = expr_settings("prism", &expression("trance", "riser", Kind::Pad, 138.0), &p);
        assert!(set.is_empty());
    }

    #[test]
    fn the_synth_glides_on_legato_notes_only() {
        let sr = 44100.0;
        let mut s = crate::dsp::make_instrument("synth", sr).unwrap();
        for (n, v) in [("wave", 0.0), ("cutoff", 1.0), ("filt env", 0.0), ("detune", 0.0), ("glide", 0.3), ("release", 0.1)] {
            s.set_param(n, v);
        }
        let freq = |x: &[f32]| {
            let m: Vec<f32> = x.chunks_exact(2).map(|f| f[0]).collect();
            let z = m.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
            z as f32 * sr / m.len() as f32
        };
        let mut buf = vec![0f32; 2 * (sr as usize / 4)];
        s.note_on(48, 0.8);
        s.render(&mut buf);
        let f0 = freq(&buf[buf.len() / 2..]);
        // the next note starts while the first is held: one voice slides up
        s.note_on(60, 0.8);
        let mut early = vec![0f32; 2 * 1323];
        s.render(&mut early);
        s.note_off(48);
        let mut late = vec![0f32; 2 * (sr as usize / 2)];
        s.render(&mut late);
        let (fe, fl) = (freq(&early), freq(&late[late.len() / 2..]));
        assert!((f0 - 130.8).abs() < 6.0, "{f0}");
        assert!(fe > f0 + 10.0 && fe < 230.0, "gliding: {fe}");
        assert!((fl - 261.6).abs() < 8.0, "arrived: {fl}");
        // the old note's release did not cut the slid voice
        assert!(late[late.len() - 400..].iter().any(|x| x.abs() > 0.01));
    }
}
