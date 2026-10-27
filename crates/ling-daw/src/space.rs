//! Genre space profiles: how wet or dry a genre is mixed, per role.
//!
//! A breakbeat or drum and bass mix is dry and tight (short rooms at most, little or no reverb on the kick and the bass, short echoes,
//! fast compressor attack and release), an ambient or cinematic mix is wet (long tails, slow envelopes), house, trance and pop sit in
//! between. One number per genre, its *wetness* 0..1, drives every setting through the formulas in `Space::role`, so the genres are
//! ordered consistently (a drier genre never gets more reverb, a longer echo or a slower release than a wetter one).
//!
//! Used by the song build (sounddesign.rs: the chains, the lanes, the instruments' own reverb / delay / chorus) and by AI Mix & Master
//! (api/mix_api.rs: sends, reverb and delay mixes, compressor timing, master processing).
use crate::compose::Kind;
use crate::dsp::Instrument;

/// the part a track plays in the space of a mix
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SRole {
    Drums,
    Bass,
    Pad,
    Lead,
    Pluck,
    Vocal,
    Fx,
}

impl SRole {
    pub const ALL: [SRole; 7] = [SRole::Drums, SRole::Bass, SRole::Pad, SRole::Lead, SRole::Pluck, SRole::Vocal, SRole::Fx];
    pub fn name(self) -> &'static str {
        match self {
            SRole::Drums => "drums",
            SRole::Bass => "bass",
            SRole::Pad => "pad",
            SRole::Lead => "lead",
            SRole::Pluck => "pluck",
            SRole::Vocal => "vocal",
            SRole::Fx => "fx",
        }
    }
    /// from the song plan's role name and its kind
    pub fn from_plan(role: &str, kind: Kind) -> SRole {
        let r = role.to_lowercase();
        if r.contains("riser") || r.contains("sfx") || r.contains("fx") || r.contains("noise") {
            return SRole::Fx;
        }
        if r == "voice" || r.contains("vocal") || r == "choir" {
            return SRole::Vocal;
        }
        match kind {
            Kind::Drums => SRole::Drums,
            Kind::Bass => SRole::Bass,
            Kind::Melody => SRole::Lead,
            Kind::Pad => SRole::Pad,
            Kind::Arp => SRole::Pluck,
            Kind::Chords => {
                if r.contains("pad") || r.contains("string") {
                    SRole::Pad
                } else {
                    SRole::Pluck
                }
            }
        }
    }
    /// from AI Mix & Master's role
    pub fn from_mix(r: crate::mixai::Role) -> SRole {
        use crate::mixai::Role;
        match r {
            Role::Drums => SRole::Drums,
            Role::Bass => SRole::Bass,
            Role::Vocal => SRole::Vocal,
            Role::Lead => SRole::Lead,
            Role::Chords => SRole::Pluck,
            Role::Pad => SRole::Pad,
            Role::Fx | Role::Other => SRole::Fx,
        }
    }
}

/// a genre's place on the dry .. wet scale
#[derive(Clone, Copy, Debug)]
pub struct Space {
    pub genre: &'static str,
    /// 0 bone dry .. 1 as wet as it gets
    pub wet: f32,
    /// dub: the echo is the instrument (more delay on the stabs and leads)
    pub dub: bool,
}

/// the settings of one role in one genre; every value 0..1 as the effect's own parameter unless noted
#[derive(Clone, Copy, Debug)]
pub struct RoleSpace {
    /// Reverb insert: mix, size (tail length), damp
    pub reverb: f32,
    pub size: f32,
    pub damp: f32,
    /// Delay insert: mix, feedback, time in beats (tempo-synced)
    pub delay: f32,
    pub feedback: f32,
    pub delay_beats: f32,
    /// Compressor attack / release parameters (attack = 1 + v * 199 ms, release = 10 + v * 990 ms)
    pub attack: f32,
    pub release: f32,
    /// Transient shaper: attack and sustain
    pub punch: f32,
    pub body: f32,
    /// Imager width
    pub width: f32,
    /// the most the instrument's OWN effects may use: reverb, delay and chorus / phaser mixes
    pub inner_reverb: f32,
    pub inner_delay: f32,
    pub inner_mod: f32,
    /// the longest amp release the instrument may have (normalised), 1 = unlimited
    pub max_release: f32,
    /// the slowest amp attack a pad must have at least (0 = none)
    pub min_attack: f32,
    /// the post-fader send level this role may feed a return with
    pub send: f32,
}

/// the wetness of a genre (names as the song generator and the tag meta know them; unknown = medium)
pub fn wetness(genre: &str) -> f32 {
    match norm(genre) {
        "breakbeat" => 0.10,
        "drum and bass" => 0.18,
        "hardstyle" | "techno" | "dubstep" => 0.20,
        "chiptune" => 0.15,
        "garage" | "hip hop" | "trap" => 0.25,
        "hyperpop" | "reggaeton" | "rock" | "afrobeat" => 0.30,
        "disco" => 0.35,
        "jazz" | "folk" => 0.40,
        "house" | "pop" => 0.45,
        "deep house" => 0.50,
        "trance" => 0.55,
        "dub" | "synthwave" => 0.60,
        "witch house" => 0.65,
        "lofi" => 0.70,
        "orchestral" => 0.75,
        "cinematic" => 0.85,
        "ambient" => 1.0,
        _ => 0.5,
    }
}

fn norm(genre: &str) -> &'static str {
    match genre.trim().to_lowercase().as_str() {
        "breakbeat" | "breaks" | "breakbeats" | "big beat" => "breakbeat",
        "dnb" | "d&b" | "jungle" | "drum & bass" | "drum n bass" | "drum and bass" | "neurofunk" => "drum and bass",
        "deep house" => "deep house",
        "house" | "edm" => "house",
        "techno" => "techno",
        "dubstep" | "riddim" | "brostep" => "dubstep",
        "hardstyle" | "hardcore" | "gabber" => "hardstyle",
        "chiptune" | "8-bit" | "8bit" => "chiptune",
        "garage" | "uk garage" | "2-step" | "2step" => "garage",
        "hip hop" | "hiphop" | "hip-hop" | "boom bap" | "rap" => "hip hop",
        "trap" | "drill" => "trap",
        "hyperpop" => "hyperpop",
        "reggaeton" => "reggaeton",
        "rock" | "punk" | "metal" => "rock",
        "afrobeat" | "afrobeats" => "afrobeat",
        "disco" | "funk" => "disco",
        "jazz" => "jazz",
        "folk" | "country" => "folk",
        "pop" => "pop",
        "trance" | "psytrance" => "trance",
        "dub" | "reggae" => "dub",
        "synthwave" | "retrowave" | "vaporwave" => "synthwave",
        "witch house" => "witch house",
        "lofi" | "lo-fi" | "chillhop" | "chill" => "lofi",
        "orchestral" | "classical" => "orchestral",
        "cinematic" | "score" | "soundtrack" => "cinematic",
        "ambient" | "drone" | "new age" => "ambient",
        _ => "",
    }
}

pub fn profile(genre: &str) -> Space {
    let n = norm(genre);
    Space { genre: if n.is_empty() { "unknown" } else { n }, wet: wetness(genre), dub: n == "dub" }
}

impl Space {
    /// "dry" (tight), "medium" or "wet"
    pub fn tier(&self) -> &'static str {
        if self.wet < 0.35 {
            "dry"
        } else if self.wet <= 0.65 {
            "medium"
        } else {
            "wet"
        }
    }

    pub fn role(&self, r: SRole) -> RoleSpace {
        let w = self.wet.clamp(0.0, 1.0);
        let dly = if self.dub { 1.8 } else { 1.0 };
        let beats = if w < 0.35 { 0.5 } else if w <= 0.65 { 0.75 } else { 1.5 };
        let (reverb, size, damp, delay, send) = match r {
            SRole::Drums => (0.02 + 0.14 * w * w, 0.10 + 0.40 * w, 0.75 - 0.35 * w, 0.0, 0.03 + 0.10 * w),
            SRole::Bass => (if w > 0.5 { 0.08 * (w - 0.5) * 2.0 } else { 0.0 }, 0.15 + 0.2 * w, 0.8, 0.0, 0.0),
            SRole::Pad => (0.12 + 0.38 * w, 0.30 + 0.65 * w, 0.5 - 0.1 * w, (0.04 + 0.12 * w) * dly, 0.2 + 0.4 * w),
            SRole::Lead => (0.05 + 0.32 * w, 0.20 + 0.60 * w, 0.55 - 0.15 * w, (0.05 + 0.18 * w) * dly, 0.1 + 0.3 * w),
            SRole::Pluck => (0.04 + 0.28 * w, 0.18 + 0.55 * w, 0.55 - 0.15 * w, (0.04 + 0.15 * w) * dly, 0.1 + 0.28 * w),
            SRole::Vocal => (0.05 + 0.26 * w, 0.20 + 0.55 * w, 0.5, (0.03 + 0.12 * w) * dly, 0.1 + 0.25 * w),
            SRole::Fx => (0.12 + 0.35 * w, 0.30 + 0.60 * w, 0.5, (0.08 + 0.15 * w) * dly, 0.2 + 0.4 * w),
        };
        let width = match r {
            SRole::Drums => 0.45,
            SRole::Bass => 0.35 + 0.1 * w,
            SRole::Pad => 0.55 + 0.30 * w,
            SRole::Lead => 0.50 + 0.20 * w,
            SRole::Pluck => 0.55 + 0.20 * w,
            SRole::Vocal => 0.40 + 0.10 * w,
            SRole::Fx => 0.6 + 0.2 * w,
        };
        let (max_release, min_attack) = match r {
            SRole::Drums => (0.15 + 0.5 * w, 0.0),
            SRole::Bass => (0.20 + 0.5 * w, 0.0),
            SRole::Lead | SRole::Pluck => (0.25 + 0.6 * w, 0.0),
            SRole::Pad => (1.0, if w > 0.5 { 0.3 * (w - 0.5) * 2.0 } else { 0.0 }),
            SRole::Vocal | SRole::Fx => (1.0, 0.0),
        };
        let modcap = if matches!(r, SRole::Drums | SRole::Bass) { 0.0 } else { 0.05 + 0.25 * w };
        RoleSpace {
            reverb,
            size,
            damp,
            delay,
            feedback: 0.15 + 0.40 * w,
            delay_beats: beats,
            attack: 0.03 + 0.10 * w,
            release: 0.05 + 0.30 * w,
            punch: 0.55 + 0.25 * (1.0 - w),
            body: 0.30 + 0.25 * w,
            width,
            inner_reverb: reverb * 0.5,
            inner_delay: delay,
            inner_mod: modcap,
            max_release,
            min_attack,
            send,
        }
    }

    /// the master bus: compressor timing, the most reverb / delay left on it
    pub fn master(&self) -> (f32, f32, f32, f32) {
        let w = self.wet;
        (0.04 + 0.10 * w, 0.08 + 0.30 * w, 0.03 + 0.15 * w, 0.02 + 0.12 * w)
    }

    /// the delay time (Delay's `time` parameter 0..1) of `beats` beats at `bpm`
    pub fn delay_time_param(beats: f32, bpm: f64) -> f32 {
        let secs = beats as f64 * 60.0 / bpm.max(30.0);
        (((secs - 0.02) / 1.5) as f32).clamp(0.0, 1.0)
    }
}

// ── instrument-internal effects and pitch safety ───────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tw {
    /// set the value
    Set,
    /// at most this value
    Max,
    /// at least this value
    Min,
}

#[derive(Clone, Copy, Debug)]
pub struct InstTweak {
    pub param: &'static str,
    pub mode: Tw,
    pub v: f32,
}

/// Effects whose job is to change the pitch (or detune): never in an automatic chain, except on a riser
pub fn pitch_altering(effect: &str) -> bool {
    matches!(effect, "Freq Shifter" | "Chorus" | "Flanger" | "Vocoder" | "Ringstorm")
}

/// The limits an instrument's own reverb / delay / chorus / release get in this space, and the pitch-altering parameters (vibrato, pitch
/// sweep, mod wheel) it must not use. `riser`: a riser or sweep part may bend and sweep, so the pitch parameters stay.
pub fn inst_tweaks(sp: &Space, r: SRole, inst: &str, riser: bool) -> Vec<InstTweak> {
    let rs = sp.role(r);
    let mut v = vec![];
    let mut mx = |param: &'static str, x: f32| v.push(InstTweak { param, mode: Tw::Max, v: x.clamp(0.0, 1.0) });
    match inst {
        "prism" => {
            mx("rv mix", rs.inner_reverb);
            mx("rv size", rs.size);
            mx("dl mix", rs.inner_delay);
            mx("dl fb", rs.feedback);
            mx("ch mix", rs.inner_mod);
            mx("ph mix", rs.inner_mod);
            if r != SRole::Fx {
                mx("drift", 0.2);
            }
            if rs.max_release < 1.0 {
                mx("amp r", rs.max_release);
            }
            if rs.min_attack > 0.0 {
                v.push(InstTweak { param: "amp a", mode: Tw::Min, v: rs.min_attack });
            }
        }
        "growl" => {
            mx("verb", rs.inner_reverb);
            mx("dely", rs.inner_delay);
            if rs.max_release < 1.0 {
                mx("amp r", rs.max_release);
            }
        }
        "chip" => {
            mx("echo", rs.inner_delay);
            mx("echo fb", rs.feedback);
            if rs.max_release < 1.0 {
                mx("amp r", rs.max_release);
            }
        }
        _ => {}
    }
    if !riser {
        match inst {
            "chip" => {
                v.push(InstTweak { param: "vib", mode: Tw::Set, v: 0.0 });
                v.push(InstTweak { param: "sweep", mode: Tw::Set, v: 0.5 });
            }
            "prism" | "growl" => v.push(InstTweak { param: "whl", mode: Tw::Set, v: 0.0 }),
            _ => {}
        }
    }
    v
}

/// apply tweaks to an instrument (parameters it does not have are skipped); returns how many values changed
pub fn apply_tweaks(inst: &mut dyn Instrument, tw: &[InstTweak]) -> usize {
    let cur = inst.params();
    let mut n = 0;
    for t in tw {
        let Some(c) = cur.iter().find(|p| p.0 == t.param).map(|p| p.1) else { continue };
        let nv = match t.mode {
            Tw::Set => t.v,
            Tw::Max => c.min(t.v),
            Tw::Min => c.max(t.v),
        };
        if (nv - c).abs() > 1e-6 {
            inst.set_param(t.param, nv);
            n += 1;
        }
    }
    n
}

// ── genre from the content ────────────────────────────────────────────────────

/// Guess a genre from the music itself: tempo, drum notes per beat and the mean note length in beats (long notes and few drums = ambient)
pub fn guess_genre(bpm: f64, drum_per_beat: f64, avg_len_beats: f64) -> &'static str {
    if drum_per_beat < 0.3 && avg_len_beats >= 1.5 {
        "ambient"
    } else if bpm >= 160.0 && drum_per_beat >= 1.2 {
        "drum and bass"
    } else if bpm >= 125.0 && drum_per_beat >= 1.5 && avg_len_beats < 1.0 {
        "techno"
    } else if bpm >= 125.0 {
        "house"
    } else if bpm < 95.0 && drum_per_beat >= 0.8 {
        "hip hop"
    } else if bpm < 95.0 {
        "lofi"
    } else {
        "pop"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genres_are_ordered_dry_to_wet() {
        for g in ["breakbeat", "drum and bass", "techno", "dubstep", "garage", "hip hop", "trap"] {
            assert_eq!(profile(g).tier(), "dry", "{g}");
        }
        for g in ["house", "deep house", "trance", "synthwave", "pop"] {
            assert_eq!(profile(g).tier(), "medium", "{g}");
        }
        for g in ["ambient", "lofi", "cinematic"] {
            assert_eq!(profile(g).tier(), "wet", "{g}");
        }
        for r in SRole::ALL {
            let (b, h, a) = (profile("breakbeat").role(r), profile("house").role(r), profile("ambient").role(r));
            assert!(b.reverb <= h.reverb && h.reverb <= a.reverb, "{r:?} reverb");
            assert!(b.size <= h.size && h.size <= a.size, "{r:?} size");
            assert!(b.delay <= h.delay && h.delay <= a.delay, "{r:?} delay");
            assert!(b.release <= h.release && h.release <= a.release && b.attack <= a.attack, "{r:?} timing");
            assert!(b.delay_beats <= h.delay_beats && h.delay_beats <= a.delay_beats, "{r:?} echo time");
        }
        // the kick and the bass stay dry in a dry genre
        assert!(profile("breakbeat").role(SRole::Bass).reverb == 0.0 && profile("breakbeat").role(SRole::Drums).reverb < 0.03);
        assert!(profile("dub").role(SRole::Lead).delay > profile("house").role(SRole::Lead).delay);
    }

    #[test]
    fn the_genre_is_guessed_from_the_content() {
        assert_eq!(guess_genre(70.0, 0.1, 4.0), "ambient");
        assert_eq!(guess_genre(174.0, 3.0, 0.5), "drum and bass");
        assert_eq!(wetness("nonsense"), 0.5);
    }

    #[test]
    fn tweaks_lower_an_instruments_own_effects_and_kill_vibrato() {
        let mut p = crate::dsp::make_instrument("prism", 44100.0).unwrap();
        p.set_param("rv mix", 0.6);
        p.set_param("dl mix", 0.5);
        let tw = inst_tweaks(&profile("breakbeat"), SRole::Lead, "prism", false);
        assert!(apply_tweaks(p.as_mut(), &tw) >= 2);
        let get = |p: &dyn Instrument, n: &str| p.params().iter().find(|x| x.0 == n).unwrap().1;
        assert!(get(p.as_ref(), "rv mix") <= profile("breakbeat").role(SRole::Lead).inner_reverb + 1e-5);
        assert!(get(p.as_ref(), "dl mix") <= profile("breakbeat").role(SRole::Lead).inner_delay + 1e-5);
        let mut c = crate::dsp::make_instrument("chip", 44100.0).unwrap();
        c.set_param("vib", 0.8);
        c.set_param("sweep", 0.9);
        apply_tweaks(c.as_mut(), &inst_tweaks(&profile("trance"), SRole::Lead, "chip", false));
        assert_eq!(get(c.as_ref(), "vib"), 0.0);
        assert_eq!(get(c.as_ref(), "sweep"), 0.5);
        // a riser keeps its sweep
        c.set_param("sweep", 0.9);
        apply_tweaks(c.as_mut(), &inst_tweaks(&profile("trance"), SRole::Fx, "chip", true));
        assert_eq!(get(c.as_ref(), "sweep"), 0.9);
    }
}
