//! The emotional plan: "think before singing". Before a note is written, the song gets a plan of how it feels section by section -
//! how happy or sad (valence), how intense (arousal), how much tension is held, how hard the singer pushes (restrained ... forceful),
//! how much breath is between the lines, and how much room the band leaves the voice. The notes, the vocal performance (its
//! dynamics, breath, vibrato, brightness, scoops) and the accompaniment (softer under the voice, a lead that answers in the gaps
//! instead of playing over it) then follow the plan.
//!
//! The plan comes from an arc (release, slow burn, bittersweet, triumph, melancholy, storm, lullaby) - named in the plan line, picked by
//! a model, or rolled from the genre and the lyrics - plus dice: every section gets a little jitter, and now and then a surprise
//! a producer would make (a hushed last pre-chorus, a breakdown that drops to almost nothing, a final chorus a notch harder).
//! The lyrics' own words lean each section's valence (a small lexicon of feeling words).
use crate::compose::Rng;
use crate::songgen::Section;

pub const ARCS: [&str; 7] = ["release", "slow burn", "bittersweet", "triumph", "melancholy", "storm", "lullaby"];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Feel {
    /// -1 sad / dark .. 1 happy / bright
    pub valence: f32,
    /// 0 calm .. 1 intense
    pub arousal: f32,
    /// 0 settled .. 1 held tension
    pub tension: f32,
    /// 0 restrained, intimate .. 1 forceful, belted
    pub vocal: f32,
    /// 0 legato .. 1 lots of breath between the lines
    pub breath: f32,
    /// 0 the band plays over the voice .. 1 the band makes room (softer, the lead answers in the gaps)
    pub space: f32,
}

#[derive(Clone, Debug, Default)]
pub struct EmotionPlan {
    pub arc: String,
    pub feels: Vec<Feel>,
    /// what the dice did, in words
    pub notes: Vec<String>,
}

fn kind(tag: &str) -> &'static str {
    match tag {
        "chorus" | "drop" | "hook" | "climax" | "final" | "drop2" => "chorus",
        "pre" | "prechorus" | "pre-chorus" | "build" | "buildup" | "rise" => "pre",
        "bridge" | "break" | "breakdown" | "interlude" => "bridge",
        "intro" => "intro",
        "outro" | "end" => "outro",
        _ => "verse",
    }
}

/// a feeling word's pull on valence and arousal
const LEXICON: &[(&str, f32, f32)] = &[
    ("love", 0.6, 0.1), ("light", 0.4, 0.0), ("alive", 0.6, 0.4), ("tonight", 0.2, 0.3), ("dance", 0.5, 0.5), ("fly", 0.4, 0.3), ("free", 0.6, 0.3),
    ("shine", 0.5, 0.2), ("smile", 0.6, 0.1), ("home", 0.3, -0.1), ("sun", 0.4, 0.1), ("dream", 0.2, -0.1), ("hope", 0.5, 0.1), ("forever", 0.3, 0.1),
    ("fire", 0.1, 0.6), ("run", 0.0, 0.5), ("fight", -0.2, 0.7), ("rise", 0.3, 0.5), ("burn", -0.1, 0.6), ("scream", -0.3, 0.8),
    ("alone", -0.6, -0.2), ("cry", -0.6, 0.1), ("tears", -0.6, 0.0), ("dark", -0.5, 0.0), ("cold", -0.4, -0.1), ("lost", -0.5, 0.0), ("goodbye", -0.5, -0.1),
    ("broken", -0.6, 0.1), ("pain", -0.6, 0.3), ("rain", -0.3, -0.1), ("night", -0.1, 0.0), ("fall", -0.3, 0.1), ("gone", -0.5, -0.2), ("miss", -0.5, -0.1),
    ("hurt", -0.6, 0.2), ("die", -0.7, 0.2), ("empty", -0.6, -0.3), ("sorry", -0.4, -0.2), ("quiet", -0.1, -0.4), ("sleep", 0.0, -0.5),
];

/// one word's pull on (valence, arousal) when it is a feeling word ("tears", "alive" ...); the singer leans on these
pub fn word_feel(word: &str) -> Option<(f32, f32)> {
    let w = word.to_lowercase();
    let w = w.trim_matches(|c: char| !c.is_alphabetic());
    LEXICON.iter().find(|(k, _, _)| *k == w || (w.len() > 4 && w.starts_with(k))).map(|(_, v, a)| (*v, *a))
}

/// the lyrics of a section as (valence, arousal) leanings, -0.4 .. 0.4 each
pub fn lyric_lean(text: &str) -> (f32, f32) {
    let t = text.to_lowercase();
    let (mut v, mut a, mut n) = (0.0f32, 0.0f32, 0);
    for w in t.split(|c: char| !c.is_alphabetic()) {
        if let Some((_, lv, la)) = LEXICON.iter().find(|(k, _, _)| *k == w || (w.len() > 4 && w.starts_with(k))) {
            v += lv;
            a += la;
            n += 1;
        }
    }
    if n == 0 {
        return (0.0, 0.0);
    }
    let k = 1.0 / (n as f32).sqrt();
    ((v * k * 0.5).clamp(-0.4, 0.4), (a * k * 0.5).clamp(-0.4, 0.4))
}

/// the arc a genre and its lyrics lean to (before the dice)
fn arc_weights(genre: &str, lyric_valence: f32) -> [f32; 7] {
    // release, slow burn, bittersweet, triumph, melancholy, storm, lullaby
    let mut w = match genre {
        "dubstep" | "drum and bass" | "hardstyle" | "techno" => [3.0, 2.0, 0.5, 1.0, 0.3, 2.5, 0.0],
        "trance" | "house" | "pop" | "hyperpop" | "garage" => [3.0, 1.5, 1.0, 2.0, 0.5, 0.5, 0.0],
        "ambient" | "lofi" | "witch house" => [0.5, 2.0, 1.5, 0.3, 2.0, 0.2, 2.0],
        "cinematic" | "orchestral" => [1.5, 2.5, 1.0, 2.5, 1.0, 1.5, 0.3],
        "folk" | "jazz" => [1.5, 1.0, 2.0, 0.8, 1.5, 0.2, 1.0],
        "rock" => [2.5, 1.5, 1.0, 1.5, 0.8, 1.5, 0.0],
        _ => [2.0, 1.5, 1.0, 1.0, 1.0, 0.8, 0.3],
    };
    if lyric_valence < -0.1 {
        w[2] *= 2.6;
        w[4] *= 3.0;
        w[3] *= 0.4;
        w[0] *= 0.7;
    } else if lyric_valence > 0.1 {
        w[3] *= 1.6;
        w[0] *= 1.3;
        w[4] *= 0.5;
    }
    w
}

/// arousal / vocal / valence shape of an arc for a section kind at a position 0..1 through the song: (arousal, vocal, valence)
fn arc_shape(arc: &str, k: &str, pos: f32) -> (f32, f32, f32) {
    let base = match k {
        "intro" => (0.25, 0.2, 0.0),
        "verse" => (0.45, 0.35, 0.0),
        "pre" => (0.62, 0.55, 0.05),
        "chorus" => (0.85, 0.85, 0.2),
        "bridge" => (0.4, 0.4, -0.1),
        _ => (0.3, 0.3, 0.0),
    };
    match arc {
        // restrained verses, the chorus explodes
        "release" => (base.0, if k == "verse" { 0.28 } else { base.1 }, base.2),
        // everything climbs with the song
        "slow burn" => (0.25 + 0.7 * pos, 0.25 + 0.7 * pos, base.2 + 0.1 * pos),
        // warm choruses over sad verses, a low bridge
        "bittersweet" => (base.0 * 0.9, base.1 * 0.9, if k == "chorus" { 0.25 } else if k == "bridge" { -0.5 } else { -0.3 }),
        // from dark to bright, the end the biggest
        "triumph" => (base.0 * (0.75 + 0.35 * pos), base.1 * (0.7 + 0.4 * pos), -0.4 + 0.9 * pos),
        // low and sad, the chorus does not explode
        "melancholy" => (base.0 * 0.75, (base.1 * 0.7).min(0.6), -0.45),
        // tension peaks in the middle, then calm
        "storm" => {
            let peak = 1.0 - ((pos - 0.6) / 0.4).abs().min(1.0);
            ((0.4 + 0.55 * peak).max(base.0 * 0.8), 0.35 + 0.55 * peak, base.2 - 0.2 * peak)
        }
        // soft all the way
        "lullaby" => (base.0 * 0.5, base.1 * 0.45, 0.2),
        _ => base,
    }
}

/// Plan the feelings of the sections. `arc` "" or "auto" rolls one; `lyrics` per section (may be empty); `mood` the plan's mood dial
/// (how far the dice may swing).
pub fn plan(sections: &[Section], arc: &str, genre: &str, lyrics: &[String], mood: f64, seed: u64) -> EmotionPlan {
    let mut rng = Rng::new(seed ^ 0xE40710);
    let leans: Vec<(f32, f32)> = (0..sections.len()).map(|i| lyric_lean(lyrics.get(i).map(|s| s.as_str()).unwrap_or(""))).collect();
    let lv = leans.iter().map(|l| l.0).sum::<f32>() / leans.len().max(1) as f32;
    let mut notes = vec![];
    let arc = if ARCS.contains(&arc) {
        arc.to_string()
    } else {
        // roll the arc: the genre's and the lyrics' weights
        let w = arc_weights(genre, lv);
        let total: f32 = w.iter().sum();
        let mut x = rng.f() as f32 * total;
        let mut pick = 0;
        for (i, wi) in w.iter().enumerate() {
            if x < *wi {
                pick = i;
                break;
            }
            x -= wi;
        }
        notes.push(format!("the dice chose a {} arc", ARCS[pick]));
        ARCS[pick].to_string()
    };
    let n = sections.len().max(1);
    let swing = 0.06 + 0.1 * mood as f32;
    let mut feels: Vec<Feel> = sections
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let k = kind(&s.tag);
            let pos = i as f32 / (n - 1).max(1) as f32;
            let (ar, vo, va) = arc_shape(&arc, k, pos);
            let (lval, lar) = leans[i];
            // the section's own energy (the arrangement) and the arc agree on how intense it is
            let arousal = (0.5 * ar + 0.5 * s.energy as f32 + lar * 0.3 + (rng.f() as f32 - 0.5) * swing).clamp(0.05, 1.0);
            let vocal = (vo + lar * 0.25 + (rng.f() as f32 - 0.5) * swing).clamp(0.1, 1.0);
            let valence = (va + lval + (rng.f() as f32 - 0.5) * swing).clamp(-1.0, 1.0);
            let tension: f32 = match k {
                "pre" => 0.75,
                "bridge" => 0.55,
                "chorus" => 0.35,
                _ => 0.3,
            } + if arc == "storm" { 0.2 } else { 0.0 };
            // a quiet, intimate vocal gets air between the lines and room from the band; a belted one less breath, still room
            let breath = (0.65 - 0.45 * vocal).clamp(0.1, 0.8);
            let space = (0.35 + 0.5 * vocal).clamp(0.2, 0.9);
            Feel { valence, arousal, tension: tension.min(1.0), vocal, breath, space }
        })
        .collect();
    // the surprise: one roll in three, a move a producer would make
    if n >= 4 && rng.f() < 0.34 {
        let choruses: Vec<usize> = (0..n).filter(|i| kind(&sections[*i].tag) == "chorus").collect();
        let pres: Vec<usize> = (0..n).filter(|i| kind(&sections[*i].tag) == "pre").collect();
        match (rng.next() % 3, choruses.last(), pres.last()) {
            (0, _, Some(&p)) => {
                feels[p].vocal *= 0.55;
                feels[p].arousal *= 0.7;
                feels[p].space = 0.9;
                notes.push(format!("surprise: a hushed {} before the last chorus", sections[p].tag));
            }
            (1, Some(&c), _) => {
                feels[c].vocal = (feels[c].vocal + 0.12).min(1.0);
                feels[c].arousal = (feels[c].arousal + 0.1).min(1.0);
                notes.push(format!("surprise: the last {} a notch harder", sections[c].tag));
            }
            _ => {
                if let Some(b) = (0..n).find(|i| kind(&sections[*i].tag) == "bridge") {
                    feels[b].arousal *= 0.55;
                    feels[b].vocal *= 0.6;
                    feels[b].space = 0.95;
                    notes.push(format!("surprise: the {} drops to almost nothing", sections[b].tag));
                }
            }
        }
    }
    EmotionPlan { arc, feels, notes }
}

/// "release arc: intro calm, verse restrained, chorus forceful ..."
pub fn summary(p: &EmotionPlan, sections: &[Section]) -> String {
    let word = |f: &Feel| {
        if f.vocal < 0.33 {
            "restrained"
        } else if f.vocal < 0.6 {
            "warm"
        } else if f.vocal < 0.8 {
            "lifting"
        } else {
            "forceful"
        }
    };
    let mut seen: Vec<String> = vec![];
    let mut parts = vec![];
    for (s, f) in sections.iter().zip(&p.feels) {
        if seen.contains(&s.tag) {
            continue;
        }
        seen.push(s.tag.clone());
        parts.push(format!("{} {}{}", s.tag, word(f), if f.valence < -0.25 { " and sad" } else if f.valence > 0.3 { " and bright" } else { "" }));
    }
    let mut out = format!("{} arc: {}", p.arc, parts.join(", "));
    if !p.notes.is_empty() {
        out.push_str(&format!(" ({})", p.notes.join("; ")));
    }
    out
}

/// A model's per-section plan (`[{"section": "verse", "vocal": 0.3, "valence": -0.2, "arousal": 0.4}, ...]` or just
/// `{"arc": "triumph"}`) laid over a rolled one: what it names wins, by section kind.
pub fn apply_model(p: &mut EmotionPlan, sections: &[Section], reply: &str) {
    let Some(a) = reply.find(['[', '{']) else { return };
    let Some(b) = reply.rfind([']', '}']) else { return };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&reply[a..=b]) else { return };
    let items: Vec<serde_json::Value> = match &v {
        serde_json::Value::Array(x) => x.clone(),
        serde_json::Value::Object(o) => o.get("feel").or_else(|| o.get("sections")).and_then(|s| s.as_array()).cloned().unwrap_or_default(),
        _ => vec![],
    };
    for it in items {
        let Some(tag) = it.get("section").and_then(|s| s.as_str()) else { continue };
        let k = kind(&tag.to_lowercase().split_whitespace().next().unwrap_or("").to_string());
        let num = |key: &str| it.get(key).and_then(|x| x.as_f64().or_else(|| x.as_str().and_then(|s| s.parse().ok()))).map(|x| x as f32);
        for (s, f) in sections.iter().zip(p.feels.iter_mut()) {
            if kind(&s.tag) != k {
                continue;
            }
            if let Some(x) = num("vocal") {
                f.vocal = x.clamp(0.1, 1.0);
                f.breath = (0.65 - 0.45 * f.vocal).clamp(0.1, 0.8);
                f.space = (0.35 + 0.5 * f.vocal).clamp(0.2, 0.9);
            }
            if let Some(x) = num("valence") {
                f.valence = x.clamp(-1.0, 1.0);
            }
            if let Some(x) = num("arousal") {
                f.arousal = x.clamp(0.05, 1.0);
            }
        }
    }
    p.notes.push("the model shaped the sections".into());
}

// ── what the feeling decides in the arrangement (movements, builds, breaths) ────────────────────────────────

/// Is `tag` a section where the music lets go (a break, a bridge, an interlude)?
pub fn is_release_tag(tag: &str) -> bool {
    kind(tag) == "bridge"
}

/// How long the silence before a drop / chorus is, in beats, for the feel of the section that builds: a held tension is let to hang
/// (two beats, half a bar), a lighter one gets a beat, and a calm one only an eighth-note breath.
pub fn gap_beats(f: &Feel) -> f64 {
    if f.tension >= 0.7 {
        2.0
    } else if f.tension >= 0.45 {
        1.0
    } else {
        0.5
    }
}

/// Where a build's roll speeds up, as the shares of the build at which it moves 1/4 > 1/8 > 1/16 > 1/32: an even climb when the
/// tension is low, a long wait and a hard last stretch when the tension is high.
pub fn roll_stages(f: &Feel) -> [f64; 3] {
    let t = f.tension.clamp(0.0, 1.0) as f64;
    [0.38 + 0.12 * t, 0.66 + 0.1 * t, 0.86 + 0.06 * t]
}

/// How many of a break's bars breathe (pads alone) before a groove may return: a calm break breathes the whole way, a tense one
/// only part of it.
pub fn breath_bars(f: &Feel, bars: u32) -> u32 {
    let share = (1.0 - 0.5 * f.tension as f64 - 0.3 * f.arousal as f64).clamp(0.4, 1.0);
    ((bars as f64 * share).round() as u32).clamp(1, bars.max(1))
}

/// A movement of pads (drums out, sustained chords) instead of a half-time groove: when the feeling is soft and the tension is let go.
pub fn wants_pads(f: &Feel) -> bool {
    f.arousal < 0.5 && f.tension < 0.62
}

/// The key lift of a final movement in semitones: a bright, high-arousal ending goes up a whole step, the rest a half step.
pub fn lift_semitones(f: &Feel) -> u8 {
    if f.valence > 0.15 && f.arousal > 0.7 {
        2
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_tension_shapes_the_build_and_the_breath() {
        let f = |tension: f32, arousal: f32, valence: f32| Feel { valence, arousal, tension, vocal: 0.5, breath: 0.3, space: 0.5 };
        assert!(gap_beats(&f(0.8, 0.5, 0.0)) > gap_beats(&f(0.2, 0.5, 0.0)));
        let (a, b) = (roll_stages(&f(0.1, 0.5, 0.0)), roll_stages(&f(0.9, 0.5, 0.0)));
        assert!(a[0] < a[1] && a[1] < a[2] && b[0] > a[0] && b[2] > a[2]);
        assert!(breath_bars(&f(0.1, 0.2, 0.0), 8) > breath_bars(&f(0.9, 0.9, 0.0), 8));
        assert!(wants_pads(&f(0.3, 0.3, 0.0)) && !wants_pads(&f(0.8, 0.8, 0.0)));
        assert_eq!(lift_semitones(&f(0.3, 0.9, 0.5)), 2);
        assert_eq!(lift_semitones(&f(0.3, 0.4, -0.5)), 1);
    }
    fn secs() -> Vec<Section> {
        [("intro", 0.2), ("verse", 0.45), ("pre", 0.6), ("chorus", 0.9), ("verse", 0.5), ("chorus", 0.95), ("bridge", 0.4), ("chorus", 1.0), ("outro", 0.3)]
            .iter()
            .map(|(t, e)| Section { tag: t.to_string(), bars: 8, energy: *e })
            .collect()
    }

    #[test]
    fn a_release_arc_holds_back_the_verse_and_opens_the_chorus() {
        let s = secs();
        let p = plan(&s, "release", "pop", &[], 0.3, 1);
        assert_eq!(p.arc, "release");
        assert!(p.feels[1].vocal < 0.45 && p.feels[3].vocal > 0.7, "{:?} {:?}", p.feels[1], p.feels[3]);
        // the band leaves more room under a big vocal; a restrained one breathes more
        assert!(p.feels[3].space > p.feels[1].space && p.feels[1].breath > p.feels[3].breath);
        assert!(summary(&p, &s).starts_with("release arc: intro"), "{}", summary(&p, &s));
    }

    #[test]
    fn the_dice_and_the_lyrics_choose() {
        let s = secs();
        let sad: Vec<String> = s.iter().map(|_| "alone in the cold rain, tears and goodbye".to_string()).collect();
        let happy: Vec<String> = s.iter().map(|_| "dance tonight, we are alive and free in the light".to_string()).collect();
        assert!(lyric_lean(&sad[0]).0 < -0.2 && lyric_lean(&happy[0]).0 > 0.2);
        // sad lyrics lean the arcs to bittersweet / melancholy over many rolls
        let count = |ly: &Vec<String>| (0..60).filter(|sd| ["bittersweet", "melancholy"].contains(&plan(&s, "auto", "pop", ly, 0.3, *sd).arc.as_str())).count();
        assert!(count(&sad) > count(&happy) + 10, "{} vs {}", count(&sad), count(&happy));
        // the same seed, the same plan
        assert_eq!(plan(&s, "auto", "pop", &sad, 0.3, 9).feels, plan(&s, "auto", "pop", &sad, 0.3, 9).feels);
        // the valence follows the words
        let p = plan(&s, "release", "pop", &sad, 0.3, 2);
        assert!(p.feels.iter().all(|f| f.valence < 0.2));
    }

    #[test]
    fn a_models_plan_is_laid_over_the_roll() {
        let s = secs();
        let mut p = plan(&s, "release", "pop", &[], 0.3, 1);
        apply_model(&mut p, &s, r#"Here: [{"section": "verse", "vocal": 0.9, "valence": -0.6}, {"section": "Chorus 2", "vocal": "0.2"}]"#);
        // inside the plan object, as the song planner's model answers
        let mut q = plan(&s, "release", "pop", &[], 0.3, 1);
        apply_model(&mut q, &s, r#"{"genre": "pop", "arc": "triumph", "feel": [{"section": "bridge", "vocal": 0.15}]}"#);
        assert!((q.feels[6].vocal - 0.15).abs() < 1e-6);
        assert!((p.feels[1].vocal - 0.9).abs() < 1e-6 && (p.feels[4].vocal - 0.9).abs() < 1e-6 && p.feels[1].valence < -0.5);
        assert!((p.feels[3].vocal - 0.2).abs() < 1e-6);
    }
}
