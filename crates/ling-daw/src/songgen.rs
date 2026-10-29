//! Generate a whole song from a description (and, optionally, a reference recording): File > New AI Song.
//!
//! 1. A reference WAV / MP3 is measured ([`analyze_reference`]): tempo, key, the chord loop of its loudest stretch (chroma per bar against
//!    the key's triads), the energy of every bar (which becomes the sections: quiet intro, verses, loud choruses / drops, a break, an
//!    outro), its tone balance and how chordal it is. That is the "style and composition" a new song can borrow.
//! 2. The prompt is read ([`plan`]): a genre word ("trance", "lofi", "hip hop" ...), "128 bpm", a key ("in F# minor"), a progression
//!    ("i VI III VII", or chord names), section words ("intro verse chorus ... outro"), instrument roles, a singer ("female vocal"),
//!    "short" / "long". A language model may add fields as JSON (genre, bpm, key, progression, structure, instruments). Prompt beats
//!    model beats reference beats the genre's defaults.
//! 3. The plan is a single editable line ([`SongPlan::to_line`] / [`SongPlan::from_line`]) so the dialog can show it and the user can
//!    change anything before building; `api/song_api.rs` builds it into tracks, clips, lyrics, automation and a mix.
//!
//! 4. The notes ([`section_notes`]): each part writes its phrase per section with the composer; then the song is shaped as a whole -
//!    the model's chords are checked against the key ([`check_prog`]), the lead's motif is developed from section to section (quoted a
//!    third up in the chorus, inverted in the bridge, varied in a verse that comes back, lifted an octave the last time), the bass locks
//!    to the kick, every part plays with one shared human feel (timing per sixteenth, accents, velocity), and [`arrange`] decides who
//!    plays where (a small intro, a part dropped in the bridge, a returning verse that grows, a last chorus that has everything).
//!
//! Everything here is deterministic for a seed: the same description and seed always give the same song.
use crate::compose::{self, Kind};
use crate::theory::{self, Key, N};

/// One section of the song: its tag ("intro", "verse", "chorus", "drop" ...), length in bars and energy 0..1.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub tag: String,
    pub bars: u32,
    pub energy: f64,
}

/// What the reference recording told us.
#[derive(Clone, Debug, Default)]
pub struct RefInfo {
    pub bpm: f64,
    pub key: Option<Key>,
    pub key_conf: f64,
    /// the chord loop of the loudest stretch as Roman numerals ("i VI III VII")
    pub prog: String,
    pub sections: Vec<Section>,
    /// share of the energy above 2.5 kHz (0..1), and below 250 Hz
    pub bright: f64,
    pub bass: f64,
    /// pitches sounding at once (1 = a line, 3+ = chords)
    pub poly: f64,
    pub genre: String,
    pub seconds: f64,
}

/// A part of the song: the role decides the instrument and the kind of notes it writes.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub role: String,
}

#[derive(Clone, Debug)]
pub struct SongPlan {
    pub genre: String,
    pub bpm: f64,
    pub key: Key,
    pub prog: String,
    pub sections: Vec<Section>,
    pub parts: Vec<Part>,
    /// Ling Voice singer ("aria", "mei", "kai", "ren", "choir"), empty = instrumental
    pub singer: String,
    pub seed: u64,
    /// why each choice was made, for the dialog
    pub notes: Vec<String>,
    /// a MIDI file the song follows, and how closely (0 = not at all, 1 = its notes everywhere); see guide.rs
    pub guide: Option<std::sync::Arc<crate::guide::Guide>>,
    pub conform: f64,
    /// chords of the choruses / drops when they differ from `prog` ("" = the same)
    pub chorus_prog: String,
    /// how human the playing is: 0 = on the grid, 1 = a loose band (timing and velocity)
    pub human: f64,
    /// the story the song tells: an arc ("rise", "fall", "journey", "dream", "battle", "hero", or "" for none) that shapes the
    /// sections' energy, and a chapter title per section in the notes
    pub story: String,
    /// the emotional arc (emotion.rs): "auto" rolls one from the genre and the lyrics, else one of emotion::ARCS
    pub arc: String,
    /// how the song opens ("auto" rolls one from the arc and the form, else one of [`INTROS`]); see [`intro_style`]
    pub intro: String,
    /// how many movements the song has ("auto" lets the arc, the length and the dice decide, else "1" / "2" / "3"); see [`movements`]
    pub movements: String,
    /// added to every section's energy (-0.3 .. 0.3)
    pub energy: f64,
    /// how busy the parts play, 0.5 = as the style plays (scales the density)
    pub drive: f64,
    /// harmonic colour 0..1: the decorator on chord and pad parts (the key's 7ths / 9ths, voice leading, broken chords)
    pub color: f64,
    /// melodic ornament 0..1: the decorator on lead parts (passing and neighbour tones, turns, suspensions)
    pub ornament: f64,
    /// a theme (from a timeline selection) the melodic parts play in variations, section by section; normalized to start at 0
    pub theme: Option<std::sync::Arc<Vec<N>>>,
    /// the ensemble the description named ("" = the genre's instruments)
    pub ensemble: String,
    /// the composition dials (see [`Dials`]): how adventurous the choices are, how much the harmony changes between sections, how far
    /// the mood swings, the vector around the wheel of fifths, the progression style, how gradually the parts come in
    pub dials: Dials,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Dials {
    /// 0..1: low = the safest choices; high = wider choices and chord substitutions (secondary dominants, borrowed chords)
    pub temperature: f64,
    /// 0..1: low = one progression for the whole song (the Pachelbel loop); high = every kind of section its own, cadences at the
    /// section ends, a returning verse that moves
    pub variety: f64,
    /// 0..1: how far the mood swings: the energy contrast between sections, a bridge in the relative key, a last chorus a step up
    pub mood: f64,
    /// -3..3: where the bridge (break, development) goes on the wheel of fifths: + towards the dominant (brighter), - towards
    /// the subdominant (darker); the song comes home after it
    pub fifths: i32,
    /// "auto", "loop", "circle", "modal", "andalusian", "borrowed"
    pub style: String,
    /// 0..1: how gradually the parts come in (high = one or two new parts a section, a budget of parts that follows the energy)
    pub layering: f64,
    /// 0..1: how much sound design the parts get (effect chains, their movement through the form, the Growl skin) - sounddesign.rs
    pub design: f64,
}

pub const PROG_STYLES: [&str; 6] = ["auto", "loop", "circle", "modal", "andalusian", "borrowed"];

/// each genre's own setting of the dials
pub fn genre_dials(name: &str) -> Dials {
    let d = |temperature: f64, variety: f64, mood: f64, fifths: i32, style: &str, layering: f64| Dials { temperature, variety, mood, fifths, style: style.into(), layering, design: crate::sounddesign::genre_design(name) };
    match name {
        "trance" | "house" | "deep house" | "techno" | "hardstyle" | "garage" => d(0.3, 0.35, 0.3, 0, "loop", 0.75),
        "drum and bass" | "dubstep" => d(0.35, 0.4, 0.45, 0, "loop", 0.65),
        "jazz" => d(0.6, 0.8, 0.4, -1, "circle", 0.4),
        "lofi" | "hip hop" => d(0.45, 0.45, 0.25, 0, "circle", 0.45),
        "cinematic" | "orchestral" => d(0.5, 0.7, 0.6, -2, "borrowed", 0.8),
        "rock" => d(0.45, 0.5, 0.4, 1, "modal", 0.5),
        "folk" => d(0.4, 0.5, 0.4, 1, "loop", 0.5),
        "synthwave" => d(0.35, 0.45, 0.4, 0, "andalusian", 0.6),
        "ambient" => d(0.5, 0.4, 0.3, 1, "modal", 0.85),
        "witch house" => d(0.4, 0.4, 0.5, -1, "andalusian", 0.6),
        "disco" | "afrobeat" | "reggaeton" | "dub" => d(0.4, 0.45, 0.3, 0, "circle", 0.5),
        _ => d(0.4, 0.5, 0.35, 1, "auto", 0.55),
    }
}

// ── the harmonic plan: a key and chords for every section ───────────────────

/// the kind of section, harmonically: A = verse-like, B = chorus-like, C = bridge-like, D = a build into a chorus
fn harm_group(tag: &str) -> char {
    if is_chorus(tag) {
        'B'
    } else if is_bridge(tag) {
        'C'
    } else if matches!(tag, "build" | "buildup" | "pre" | "prechorus" | "pre-chorus") {
        'D'
    } else {
        'A'
    }
}

/// progressions of a style, for a major or a minor key
fn style_progs(style: &str, minor: bool) -> &'static [&'static str] {
    match (style, minor) {
        ("circle", false) => &["vi ii V I", "I vi ii V", "iii vi ii V", "I IV vii° iii vi ii V I"],
        ("circle", true) => &["i iv VII III", "VI ii° V i", "i iv VII III VI ii° V i", "iv VII III VI"],
        ("modal", false) => &["I bVII IV I", "I II IV I", "I bVII bVI bVII", "I IV bVII IV"],
        ("modal", true) => &["i IV i VII", "i bII i VII", "i IV VII i", "i ii VII i"],
        ("andalusian", false) => &["vi V IV iii", "I V IV iii", "vi V IV V"],
        ("andalusian", true) => &["i VII VI V", "i VII VI VII", "VI VII i i"],
        ("borrowed", false) => &["I iv I bVI", "I bIII IV iv", "IV iv I I", "I bVI bVII I"],
        ("borrowed", true) => &["i IV VI V", "i bII VI V", "VI iv V i", "i VI III bII"],
        (_, false) => &["I V vi IV", "I IV I V", "vi IV I V", "I iii IV V", "I vi IV V", "IV V iii vi", "I V IV V", "I IV vi V"],
        (_, true) => &["i VI III VII", "i iv v i", "i VII VI VII", "i iv VII III", "i v VI VII", "VI VII i i", "i VI iv V"],
    }
}

/// a family that sounds different from `style`, for a bridge
fn contrast_style(style: &str) -> &'static str {
    match style {
        "loop" | "auto" => "circle",
        "circle" => "borrowed",
        "modal" => "circle",
        "andalusian" => "borrowed",
        _ => "modal",
    }
}

fn group_seed(plan: &SongPlan, g: char, k: usize) -> u64 {
    plan.seed.wrapping_mul(0x9E37).wrapping_add(g as u64 * 131).wrapping_add(k as u64 * 7)
}

/// The key of a section: home, except a bridge (break, development) that goes `fifths` steps around the wheel (or, with a high
/// mood and no vector, to the relative key), a build halfway there, a verse that comes back one step that way when variety is high,
/// and a last chorus a whole step up when the mood is high.
pub fn section_key(plan: &SongPlan, si: usize) -> Key {
    let home = plan.key;
    let Some(sec) = plan.sections.get(si) else { return home };
    // a movement with a key of its own (the final lift, the reflective one) rules the sections in it
    if let Some(k) = movement_key(plan, si) {
        return k;
    }
    let d = &plan.dials;
    let g = harm_group(&sec.tag);
    let k = plan.sections[..si].iter().filter(|s| harm_group(&s.tag) == g).count();
    let count = plan.sections.iter().filter(|s| harm_group(&s.tag) == g).count();
    let around = |steps: i32| Key::new(((home.tonic as i32 + 7 * steps).rem_euclid(12)) as u8, home.minor);
    match g {
        'C' if d.fifths != 0 => around(d.fifths),
        'C' if d.mood > 0.5 => {
            if home.minor { Key::new((home.tonic + 3) % 12, false) } else { Key::new((home.tonic + 9) % 12, true) }
        }
        'D' if d.fifths.abs() >= 2 && d.variety > 0.5 && si + 1 < plan.sections.len() && harm_group(&plan.sections[si + 1].tag) == 'C' => around(d.fifths / 2),
        'A' if plan.sections[..si].iter().any(|s| s.tag == sec.tag) && d.variety > 0.7 && d.fifths != 0 && si + 1 < plan.sections.len() => around(d.fifths.signum()),
        'B' if count > 1 && k + 1 == count && d.mood > 0.65 => home.transposed(2),
        _ => home,
    }
}

/// The progression (Roman numerals in the section's key) of a section: the plan's own chords for the verses (and its chorus chords
/// for the choruses) when it has them; otherwise one from the progression style - a different one for each kind of section as the
/// variety rises (a bridge from a contrasting family, a build on predominant to dominant), the same loop everywhere when it is low;
/// the temperature widens the choice and, above 0.6, swaps a chord for its secondary dominant or a borrowed one.
pub fn section_prog(plan: &SongPlan, si: usize) -> String {
    let Some(sec) = plan.sections.get(si) else { return String::new() };
    let d = &plan.dials;
    let key = section_key(plan, si);
    let g = if d.variety < 0.25 { 'A' } else { harm_group(&sec.tag) };
    if g == 'A' && !plan.prog.is_empty() {
        return plan.prog.clone();
    }
    if g == 'B' && !plan.chorus_prog.is_empty() {
        return plan.chorus_prog.clone();
    }
    let style = prog_style(plan);
    if si == 0 && sec.tag == "intro" && d.variety >= 0.25 {
        if let Some(p) = intro_prog(plan, key) {
            return p;
        }
    }
    let k = plan.sections[..si].iter().filter(|s| harm_group(&s.tag) == g).count();
    let mut rng = compose::Rng::new(group_seed(plan, g, if d.variety > 0.75 { k } else { 0 }));
    let pool: Vec<&str> = match g {
        'C' => style_progs(contrast_style(style), key.minor).to_vec(),
        'D' => {
            if key.minor { vec!["iv V", "VI VII", "iv VII", "ii° V"] } else { vec!["IV V", "ii V", "IV I V V", "vi IV V V"] }
        }
        _ => style_progs(style, key.minor).to_vec(),
    };
    // how much of the pool the temperature lets in (the safest first)
    let n = ((pool.len() as f64 * (0.35 + 0.65 * d.temperature)).ceil() as usize).clamp(1, pool.len());
    let mut pick = pool[(rng.next() as usize) % n];
    // a chorus does not repeat the verse's loop when variety asks for change
    if g == 'B' && d.variety >= 0.3 {
        let a = {
            let ai = plan.sections.iter().position(|s| harm_group(&s.tag) == 'A');
            ai.map(|ai| section_prog_plain(plan, ai)).unwrap_or_default()
        };
        if pick == a {
            pick = pool[((rng.next() as usize) % pool.len().max(2)).max(1) % pool.len()];
            if pick == a {
                pick = pool[(pool.iter().position(|p| *p == a).unwrap_or(0) + 1) % pool.len()];
            }
        }
    }
    let mut out = pick.to_string();
    if d.temperature > 0.6 && rng.chance((d.temperature - 0.6) * 1.5) {
        // a secondary dominant (V/V) or, in a minor key, the Neapolitan, in place of the chord before the last
        let mut toks: Vec<String> = out.split_whitespace().map(str::to_string).collect();
        if toks.len() >= 3 {
            let i = toks.len() - 2;
            toks[i] = if key.minor { "bII".into() } else { "II".into() };
            out = toks.join(" ");
        }
    }
    out
}

/// The progression family: the plan's style, or for "auto" a family the emotional arc leans to (a bittersweet song borrows from the
/// minor, a melancholy one walks down the Andalusian cadence, a triumph rings on the flat seventh), rolled with the seed.
pub fn prog_style(plan: &SongPlan) -> &'static str {
    let d = &plan.dials;
    if d.style != "auto" && !d.style.is_empty() {
        return PROG_STYLES.iter().copied().find(|s| *s == d.style).unwrap_or("loop");
    }
    let gst = genre(&plan.genre).style;
    if matches!(gst, "jazz" | "lofi" | "disco" | "deep house") {
        return "circle";
    }
    let pool: &[&str] = match plan.arc.as_str() {
        "release" => &["loop", "circle", "loop"],
        "slow burn" => &["modal", "loop", "borrowed"],
        "bittersweet" => &["borrowed", "circle", "loop"],
        "triumph" => &["modal", "loop", "borrowed"],
        "melancholy" => &["andalusian", "circle", "borrowed"],
        "storm" => &["andalusian", "modal", "borrowed"],
        "lullaby" => &["circle", "loop"],
        _ => &["loop", "circle", "modal", "borrowed"],
    };
    let mut rng = compose::Rng::new(plan.seed.wrapping_mul(0x51F1).wrapping_add(0xC0DE));
    pool[(rng.next() as usize) % pool.len()]
}

/// the intro's harmony by its style: a pedal on the tonic (beat, drone), the chorus's chords (theme, cold), a suspense that leans
/// to the dominant or the verse's own loop (swell, solo); `None` = the verse's
fn intro_prog(plan: &SongPlan, key: Key) -> Option<String> {
    let mut rng = compose::Rng::new(group_seed(plan, 'I', 0));
    let pick = |rng: &mut compose::Rng, v: &[&str]| v[(rng.next() as usize) % v.len()].to_string();
    match plan.intro.as_str() {
        "beat" | "drone" => Some(if key.minor { pick(&mut rng, &["i i i i", "i i iv i", "i i VII i", "i VI i VI"]) } else { pick(&mut rng, &["I I I I", "I I IV I", "I bVII I I", "I ii I ii"]) }),
        "theme" | "cold" => plan.sections.iter().position(|s| is_chorus(&s.tag)).map(|ci| section_prog(plan, ci)),
        "swell" | "solo" => {
            if rng.next() % 2 == 0 {
                Some(if key.minor { pick(&mut rng, &["VI VI VII VII", "iv iv V V", "VI VII VI V", "i iv VI V"]) } else { pick(&mut rng, &["IV IV V V", "vi vi IV V", "IV iv I V", "ii ii IV V"]) })
            } else {
                None
            }
        }
        _ => None,
    }
}

/// the verse progression without the chorus rule (for the comparison above)
fn section_prog_plain(plan: &SongPlan, si: usize) -> String {
    let d = &plan.dials;
    if !plan.prog.is_empty() {
        return plan.prog.clone();
    }
    let key = section_key(plan, si);
    let style = prog_style(plan);
    let mut rng = compose::Rng::new(group_seed(plan, 'A', 0));
    let pool = style_progs(style, key.minor);
    let n = ((pool.len() as f64 * (0.35 + 0.65 * d.temperature)).ceil() as usize).clamp(1, pool.len());
    pool[(rng.next() as usize) % n].to_string()
}

/// The chords of a section (start, length, root, quality), a chord per bar (or as the style's chord rate says), with a cadence:
/// the last bar before a chorus / drop is the dominant (a half cadence) and the song's last bar the tonic, once there is variety.
pub fn section_chords(plan: &SongPlan, si: usize, cpb: f64) -> (Key, Vec<(f64, f64, u8, usize)>) {
    let key = section_key(plan, si);
    let Some(sec) = plan.sections.get(si) else { return (key, vec![]) };
    let prog = compose::parse_prog(&section_prog(plan, si), key);
    if prog.is_empty() {
        return (key, vec![]);
    }
    let total = sec.bars as f64 * 4.0;
    let span = 4.0 / cpb.clamp(0.25, 4.0);
    let n = (total / span).ceil() as usize;
    let mut out: Vec<(f64, f64, u8, usize)> = (0..n)
        .map(|i| {
            let (r, q) = prog[i % prog.len()];
            let st = i as f64 * span;
            (st, span.min(total - st), r, q)
        })
        .collect();
    if plan.dials.variety >= 0.3 && n >= 2 {
        let major = theory::qual_index("").unwrap_or(0);
        let next_chorus = plan.sections.get(si + 1).map(|s| is_chorus(&s.tag)).unwrap_or(false);
        if next_chorus && !is_chorus(&sec.tag) {
            let last = out.last_mut().unwrap();
            last.2 = key.scale()[4];
            last.3 = major;
        } else if si + 1 == plan.sections.len() {
            let last = out.last_mut().unwrap();
            last.2 = key.tonic;
            last.3 = theory::qual_index(if key.minor { "m" } else { "" }).unwrap_or(0);
        }
    }
    (key, out)
}

/// what the harmonic plan does, for the plan's notes
pub fn harmony_summary(plan: &SongPlan) -> String {
    let mut seen: Vec<String> = vec![];
    let mut parts = vec![];
    for (si, sec) in plan.sections.iter().enumerate() {
        let key = section_key(plan, si);
        let p = section_prog(plan, si);
        let line = format!("{} {}{}", sec.tag, p, if key != plan.key { format!(" in {}", key.name()) } else { String::new() });
        if !seen.contains(&line) {
            seen.push(line.clone());
            parts.push(line);
        }
    }
    format!("harmony: {}", parts.join("; "))
}

/// A genre the generator knows: its composer style, tempo, drum kit and pattern, song form, the roles it uses and its loudness.
pub struct Genre {
    pub name: &'static str,
    pub style: &'static str,
    pub bpm: f64,
    pub kit: &'static str,
    pub drums: &'static str,
    pub form: Form,
    pub roles: &'static [&'static str],
    pub lufs: f64,
    pub words: &'static [&'static str],
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Form {
    /// intro, build, drop, break, build, drop, outro
    Club,
    /// intro, verse, chorus, verse, chorus, bridge, chorus, outro
    Song,
    /// intro, theme, development, climax, outro
    Score,
}

pub const GENRES: &[Genre] = &[
    Genre { name: "trance", style: "trance", bpm: 138.0, kit: "Kit House 909", drums: "trance", form: Form::Club, roles: &["drums", "rolling bass", "supersaw lead", "warm pad", "pluck", "arp", "riser"], lufs: -9.0, words: &["trance", "uplifting", "psytrance"] },
    Genre { name: "house", style: "house", bpm: 124.0, kit: "Kit House 909", drums: "house", form: Form::Club, roles: &["drums", "rolling bass", "stab chord", "piano", "warm pad", "synth lead"], lufs: -9.0, words: &["house", "edm", "club", "dance"] },
    Genre { name: "deep house", style: "deep house", bpm: 122.0, kit: "Kit House 909", drums: "house", form: Form::Club, roles: &["drums", "sub bass", "stab chord", "keys", "ambient pad"], lufs: -10.0, words: &["deep house", "deep"] },
    Genre { name: "techno", style: "techno", bpm: 132.0, kit: "Kit Techno Hard", drums: "techno", form: Form::Club, roles: &["drums", "rolling bass", "stab chord", "texture", "arp", "riser"], lufs: -8.5, words: &["techno", "industrial techno", "rave"] },
    Genre { name: "disco", style: "disco", bpm: 118.0, kit: "Kit Funk", drums: "disco", form: Form::Song, roles: &["drums", "pluck bass", "strings", "keys", "brass"], lufs: -10.0, words: &["disco", "funk", "nu disco", "boogie"] },
    Genre { name: "hyperpop", style: "hyperpop", bpm: 150.0, kit: "Kit Hyperpop", drums: "trap", form: Form::Song, roles: &["drums", "sub bass", "hyper lead", "pluck", "warm pad"], lufs: -8.0, words: &["hyperpop", "glitch", "pc music"] },
    Genre { name: "trap", style: "hyperpop", bpm: 140.0, kit: "Kit Trap", drums: "trap", form: Form::Song, roles: &["drums", "sub bass", "bell", "dark pad", "synth lead"], lufs: -8.5, words: &["trap", "drill", "808"] },
    Genre { name: "hip hop", style: "lofi", bpm: 90.0, kit: "Kit Boom Bap", drums: "boombap", form: Form::Song, roles: &["drums", "sub bass", "keys", "warm pad", "bell"], lufs: -10.0, words: &["hip hop", "hiphop", "hip-hop", "boom bap", "rap"] },
    Genre { name: "witch house", style: "witch house", bpm: 70.0, kit: "Kit Trap", drums: "halftime", form: Form::Song, roles: &["drums", "sub bass", "dark pad", "choir", "bell"], lufs: -11.0, words: &["witch house", "dark", "occult"] },
    Genre { name: "lofi", style: "lofi", bpm: 82.0, kit: "Kit Lo-Fi Hip-Hop", drums: "boombap", form: Form::Song, roles: &["drums", "sub bass", "keys", "warm pad", "guitar"], lufs: -12.0, words: &["lofi", "lo-fi", "chill", "study", "chillhop"] },
    Genre { name: "ambient", style: "ambient", bpm: 72.0, kit: "Kit Brushes", drums: "halftime", form: Form::Score, roles: &["ambient pad", "warm pad", "bell", "texture", "sub bass"], lufs: -16.0, words: &["ambient", "drone", "meditation", "atmospheric", "new age"] },
    Genre { name: "pop", style: "pop", bpm: 112.0, kit: "Kit Electro 808", drums: "boombap", form: Form::Song, roles: &["drums", "sub bass", "piano", "warm pad", "synth lead", "pluck"], lufs: -9.0, words: &["pop", "k-pop", "j-pop", "kpop", "jpop", "radio", "anthem"] },
    Genre { name: "rock", style: "rock", bpm: 120.0, kit: "Kit Stadium Rock", drums: "boombap", form: Form::Song, roles: &["drums", "pluck bass", "guitar", "organ", "strings"], lufs: -9.5, words: &["rock", "punk", "indie", "grunge", "metal"] },
    Genre { name: "jazz", style: "jazz", bpm: 110.0, kit: "Kit Brushes", drums: "halftime", form: Form::Song, roles: &["drums", "rolling bass", "piano", "keys", "brass"], lufs: -14.0, words: &["jazz", "swing", "bebop", "bossa"] },
    Genre { name: "cinematic", style: "cinematic", bpm: 90.0, kit: "Kit Stadium Rock", drums: "halftime", form: Form::Score, roles: &["strings", "brass", "choir", "dark pad", "sub bass", "drums"], lufs: -14.0, words: &["cinematic", "epic", "trailer", "film", "soundtrack", "score"] },
    Genre { name: "orchestral", style: "cinematic", bpm: 84.0, kit: "Kit Stadium Rock", drums: "halftime", form: Form::Score, roles: &["strings", "brass", "woodwind", "choir", "piano"], lufs: -15.0, words: &["orchestral", "orchestra", "classical", "symphonic"] },
    Genre { name: "synthwave", style: "synthwave", bpm: 104.0, kit: "Kit New Wave", drums: "trance", form: Form::Song, roles: &["drums", "rolling bass", "synth lead", "warm pad", "arp"], lufs: -10.0, words: &["synthwave", "retrowave", "outrun", "80s", "vaporwave"] },
    Genre { name: "drum and bass", style: "dnb", bpm: 174.0, kit: "Kit Drum and Bass", drums: "dnb", form: Form::Club, roles: &["drums", "growl bass", "warm pad", "pluck", "synth lead", "riser"], lufs: -8.5, words: &["drum and bass", "dnb", "d&b", "jungle", "liquid", "neurofunk"] },
    Genre { name: "dubstep", style: "dnb", bpm: 140.0, kit: "Kit Industrial", drums: "halftime", form: Form::Club, roles: &["drums", "wobble bass", "growl bass", "laser lead", "dark pad", "riser"], lufs: -8.0, words: &["dubstep", "riddim", "brostep", "wobble"] },
    Genre { name: "hardstyle", style: "techno", bpm: 150.0, kit: "Kit Techno Hard", drums: "techno", form: Form::Club, roles: &["drums", "rolling bass", "supersaw lead", "warm pad", "riser"], lufs: -7.5, words: &["hardstyle", "hardcore", "gabber", "rawstyle"] },
    Genre { name: "dub", style: "dub", bpm: 75.0, kit: "Kit Dub", drums: "onedrop", form: Form::Song, roles: &["drums", "sub bass", "stab chord", "organ", "texture"], lufs: -11.0, words: &["dub", "reggae", "roots", "ska"] },
    Genre { name: "garage", style: "house", bpm: 132.0, kit: "Kit UK Garage", drums: "breakbeat", form: Form::Club, roles: &["drums", "sub bass", "stab chord", "vocal lead", "warm pad"], lufs: -9.0, words: &["garage", "uk garage", "2-step", "2step", "speed garage"] },
    Genre { name: "reggaeton", style: "pop", bpm: 95.0, kit: "Kit Dembow", drums: "trap", form: Form::Song, roles: &["drums", "sub bass", "pluck", "warm pad", "synth lead"], lufs: -8.5, words: &["reggaeton", "dembow", "latin", "urbano"] },
    Genre { name: "afrobeat", style: "disco", bpm: 108.0, kit: "Kit Afro Percussion", drums: "breakbeat", form: Form::Song, roles: &["drums", "pluck bass", "guitar", "keys", "brass"], lufs: -10.0, words: &["afrobeat", "afrobeats", "amapiano", "highlife"] },
    Genre { name: "chiptune", style: "pop", bpm: 140.0, kit: "Kit Electro 808", drums: "breakbeat", form: Form::Song, roles: &["drums", "pluck bass", "synth lead", "arp"], lufs: -10.0, words: &["chiptune", "8-bit", "8bit", "game", "nes"] },
    Genre { name: "folk", style: "pop", bpm: 96.0, kit: "Kit Brushes", drums: "halftime", form: Form::Song, roles: &["guitar", "piano", "strings", "woodwind", "drums"], lufs: -13.0, words: &["folk", "acoustic", "country", "singer-songwriter"] },
    Genre { name: "acid techno", style: "techno", bpm: 135.0, kit: "Kit Techno Hard", drums: "techno", form: Form::Club, roles: &["drums", "acid bass", "stab chord", "texture", "arp", "riser"], lufs: -8.5, words: &["acid techno", "acid", "acid house", "303"] },
    Genre { name: "future bass", style: "pop", bpm: 150.0, kit: "Kit Electro 808", drums: "trap", form: Form::Club, roles: &["drums", "sub bass", "supersaw lead", "warm pad", "pluck", "riser"], lufs: -8.0, words: &["future bass", "kawaii", "melodic bass", "kawaii future bass"] },
    Genre { name: "breakbeat", style: "dnb", bpm: 132.0, kit: "Kit UK Garage", drums: "breakbeat", form: Form::Club, roles: &["drums", "rolling bass", "stab chord", "synth lead", "warm pad", "riser"], lufs: -9.0, words: &["breakbeat", "breaks", "big beat", "nu skool breaks"] },
    Genre { name: "ballad", style: "pop", bpm: 72.0, kit: "Kit Brushes", drums: "halftime", form: Form::Song, roles: &["piano", "strings", "warm pad", "sub bass", "drums", "woodwind"], lufs: -13.0, words: &["ballad", "power ballad", "love song", "slow song"] },
];

pub fn genre(name: &str) -> &'static Genre {
    let n = name.trim().to_lowercase();
    GENRES.iter().find(|g| g.name == n).or_else(|| GENRES.iter().find(|g| g.words.contains(&n.as_str()))).unwrap_or(&GENRES[11])
}

/// the compose kind a role writes
pub fn role_kind(role: &str) -> Kind {
    if let Some(i) = crate::ensemble::instrument(role) {
        return i.kind;
    }
    let r = role.to_lowercase();
    if r == "drums" {
        Kind::Drums
    } else if r.contains("bass") {
        Kind::Bass
    } else if r.contains("lead") || r == "woodwind" || r == "brass" || r == "voice" {
        Kind::Melody
    } else if r.contains("pad") || r == "strings" || r == "choir" || r == "texture" || r == "riser" {
        Kind::Pad
    } else if r == "arp" || r == "pluck" || r == "bell" {
        Kind::Arp
    } else {
        Kind::Chords
    }
}

/// Does `role` play in a section of `energy` (and tag)? Drums, bass and the lead wait for the music to lift; pads carry the quiet parts.
pub fn plays(role: &str, sec: &Section, form: Form, has_vocal: bool) -> bool {
    let e = sec.energy;
    let tag = sec.tag.as_str();
    let r = role.to_lowercase();
    match role_kind(&r) {
        Kind::Drums => e >= 0.45 || (form == Form::Song && tag == "verse"),
        Kind::Bass => e >= 0.4,
        Kind::Melody => {
            if r == "riser" {
                false
            } else if has_vocal {
                // the singer has the tune in the verses: an instrument lead only answers in the loud parts
                e >= 0.8
            } else {
                e >= 0.62 || tag == "verse"
            }
        }
        Kind::Pad => {
            if r == "riser" {
                is_build_tag(tag) || is_chorus(tag) && form == Form::Club || tag == "intro" && form == Form::Club
            } else {
                // pads leave room in the drops of club music
                !(form == Form::Club && tag == "drop" && r != "strings")
            }
        }
        Kind::Arp => e >= 0.5 || tag == "intro" && form == Form::Score,
        Kind::Chords => e >= 0.3,
    }
}

pub(crate) fn round4(x: f64) -> u32 {
    (((x / 4.0).round() * 4.0) as u32).max(4)
}

/// the sections of a song form at a length scale (0.5 short .. 1.5 long)
pub fn form_sections(form: Form, scale: f64) -> Vec<Section> {
    let s = |tag: &str, bars: f64, energy: f64| Section { tag: tag.into(), bars: round4(bars * scale), energy };
    match form {
        Form::Club => vec![s("intro", 8.0, 0.3), s("build", 8.0, 0.55), s("drop", 16.0, 1.0), s("break", 8.0, 0.35), s("build", 8.0, 0.6), s("drop", 16.0, 1.0), s("outro", 8.0, 0.3)],
        Form::Song => vec![s("intro", 4.0, 0.35), s("verse", 16.0, 0.55), s("chorus", 8.0, 0.85), s("verse", 16.0, 0.55), s("chorus", 8.0, 0.9), s("bridge", 8.0, 0.5), s("chorus", 8.0, 1.0), s("outro", 4.0, 0.3)],
        Form::Score => vec![s("intro", 8.0, 0.2), s("theme", 16.0, 0.45), s("development", 16.0, 0.7), s("climax", 8.0, 0.95), s("outro", 8.0, 0.3)],
    }
}

/// energy of a section word (when the prompt names the structure)
fn tag_energy(tag: &str) -> f64 {
    match tag {
        "intro" | "outro" => 0.3,
        "verse" | "theme" => 0.55,
        "pre" | "prechorus" | "pre-chorus" | "build" | "buildup" => 0.6,
        "chorus" | "hook" | "refrain" => 0.88,
        "drop" | "climax" => 1.0,
        "bridge" | "development" => 0.5,
        "break" | "breakdown" => 0.35,
        _ => 0.6,
    }
}
const TAGS: [&str; 18] = ["intro", "verse", "prechorus", "pre-chorus", "pre", "chorus", "hook", "refrain", "bridge", "break", "breakdown", "build", "buildup", "drop", "outro", "theme", "development", "climax"];

fn canon_tag(t: &str) -> &'static str {
    match t {
        "pre" | "prechorus" | "pre-chorus" | "buildup" => "build",
        "hook" | "refrain" => "chorus",
        "breakdown" => "break",
        "intro" => "intro",
        "verse" => "verse",
        "chorus" => "chorus",
        "bridge" => "bridge",
        "break" => "break",
        "build" => "build",
        "drop" => "drop",
        "outro" => "outro",
        "theme" => "theme",
        "development" => "development",
        "climax" => "climax",
        _ => "verse",
    }
}

/// Section words in the prompt, in order: "intro verse chorus verse chorus outro" or "intro 4, verse 8, chorus 8"
fn prompt_sections(prompt: &str, form: Form) -> Option<Vec<Section>> {
    let lower = prompt.to_lowercase();
    // only a part of the prompt that is about structure: after "structure" / "form", else the whole prompt if it holds 3+ section words
    let text = lower.split_once("structure").or_else(|| lower.split_once("form:")).map(|(_, r)| r.to_string()).unwrap_or(lower.clone());
    let words: Vec<&str> = text.split(|c: char| !c.is_alphanumeric() && c != '-').filter(|w| !w.is_empty()).collect();
    let mut out: Vec<Section> = vec![];
    let mut i = 0;
    while i < words.len() {
        if TAGS.contains(&words[i]) {
            let tag = canon_tag(words[i]);
            let bars = words.get(i + 1).and_then(|w| w.parse::<u32>().ok()).filter(|b| (1..=64).contains(b));
            let def = form_sections(form, 1.0).iter().find(|s| s.tag == tag).map(|s| s.bars).unwrap_or(8);
            out.push(Section { tag: tag.into(), bars: bars.unwrap_or(def).max(1), energy: tag_energy(tag) });
            if bars.is_some() {
                i += 1;
            }
        }
        i += 1;
    }
    if out.len() >= 3 { Some(out) } else { None }
}

/// The genre a prompt names (the longest matching word wins: "deep house" before "house")
pub fn prompt_genre(prompt: &str) -> Option<&'static Genre> {
    let p = format!(" {} ", prompt.to_lowercase().replace(['.', ',', ';', ':', '!', '?', '(', ')', '/'], " "));
    let mut best: Option<(&'static Genre, usize)> = None;
    for g in GENRES {
        for w in g.words.iter().chain(std::iter::once(&g.name)) {
            if p.contains(&format!(" {w} ")) && best.map(|b| w.len() > b.1).unwrap_or(true) {
                best = Some((g, w.len()));
            }
        }
    }
    best.map(|b| b.0)
}

fn prompt_bpm(prompt: &str) -> Option<f64> {
    let l = prompt.to_lowercase();
    let w: Vec<&str> = l.split_whitespace().collect();
    for (i, x) in w.iter().enumerate() {
        let num = x.trim_end_matches("bpm");
        if let Ok(v) = num.parse::<f64>() {
            let follows = x.ends_with("bpm") || w.get(i + 1).map(|n| n.starts_with("bpm")).unwrap_or(false);
            if follows && (40.0..=220.0).contains(&v) {
                return Some(v);
            }
        }
    }
    None
}

/// roles the prompt asks for ("with strings, a piano and a wobble bass")
fn prompt_roles(prompt: &str) -> Vec<String> {
    let l = format!(" {} ", prompt.to_lowercase());
    let mut v: Vec<String> = vec![];
    for r in crate::ai_import::ROLES {
        if l.contains(&format!(" {r}")) && !v.iter().any(|x| x == r) {
            v.push(r.to_string());
        }
    }
    // everyday words for roles
    for (w, r) in [("guitars", "guitar"), ("violin", "strings"), ("cello", "strings"), ("orchestra", "strings"), ("horns", "brass"), ("trumpet", "brass"), ("saxophone", "brass"), ("sax", "brass"), ("flute", "woodwind"), ("rhodes", "keys"), ("synth pad", "warm pad"), ("808", "sub bass"), ("reese", "growl bass"), ("bells", "bell"), ("arpeggio", "arp")] {
        if l.contains(&format!(" {w}")) && !v.iter().any(|x| x == r) {
            v.push(r.to_string());
        }
    }
    v
}

fn prompt_singer(prompt: &str, has_lyrics: bool) -> String {
    let l = prompt.to_lowercase();
    if l.contains("instrumental") || l.contains("no vocal") {
        return String::new();
    }
    if l.contains("choir") && !has_lyrics {
        return "choir".into();
    }
    let male = ["male vocal", "male voice", "male singer", "man singing", "baritone", "tenor", "boy"].iter().any(|w| l.contains(w)) && !l.contains("female");
    let female = ["female", "woman", "girl", "soprano", "diva"].iter().any(|w| l.contains(w));
    let wants = has_lyrics || l.contains("vocal") || l.contains("singer") || l.contains("sung") || l.contains("sings");
    if !wants {
        return String::new();
    }
    if male { "kai".into() } else if female { "aria".into() } else { "mei".into() }
}

fn field<'a>(f: &'a [(String, String)], k: &str) -> Option<&'a str> {
    f.iter().rev().find(|(a, _)| a == k).map(|(_, b)| b.as_str())
}

/// Build the plan: prompt, then the model's fields, then the reference, then the genre's defaults.
pub fn plan(prompt: &str, lyrics: &str, model: &[(String, String)], reference: Option<&RefInfo>, length: f64, seed: u64) -> SongPlan {
    plan_with(prompt, lyrics, model, reference, length, seed, None, 0.0)
}

/// [`plan`] with a MIDI guide: tempo, key, chords and structure come from the file (the prompt still wins) when `conform` is above 0
#[allow(clippy::too_many_arguments)]
pub fn plan_with(prompt: &str, lyrics: &str, model: &[(String, String)], reference: Option<&RefInfo>, length: f64, seed: u64, guide: Option<&std::sync::Arc<crate::guide::Guide>>, conform: f64) -> SongPlan {
    plan_fit(prompt, lyrics, model, reference, length, seed, guide, conform, &Fit::Auto)
}

/// How long the song is.
#[derive(Clone, Debug, PartialEq)]
pub enum Fit {
    /// the form at the length scale; sung sections grow when their lyrics need more room
    Auto,
    /// a target duration in seconds (the lyrics still get their room; the instrumental sections give time back)
    Duration(f64),
    /// the guide MIDI's form, bars and tempo (even when its notes are not followed)
    Midi,
    /// every sung section exactly as long as its lyrics need
    Lyrics,
}

impl Fit {
    /// "", "auto", "lyrics", "midi", "duration:210", "duration:3:30", "3:30"
    pub fn parse(s: &str) -> Fit {
        let t = s.trim().to_lowercase();
        let secs = |v: &str| -> Option<f64> {
            let v = v.trim();
            if let Some((m, sec)) = v.split_once(':') {
                Some(m.trim().parse::<f64>().ok()? * 60.0 + sec.trim().parse::<f64>().ok()?)
            } else {
                let x = v.parse::<f64>().ok()?;
                // a small number is minutes ("3.5"), a large one seconds ("210")
                Some(if x < 15.0 { x * 60.0 } else { x })
            }
        };
        match t.as_str() {
            "lyrics" => Fit::Lyrics,
            "midi" => Fit::Midi,
            "" | "auto" => Fit::Auto,
            _ => secs(t.strip_prefix("duration:").unwrap_or(&t)).filter(|x| *x >= 10.0).map(|x| Fit::Duration(x.min(1800.0))).unwrap_or(Fit::Auto),
        }
    }
}

/// [`plan_with`] with a length rule (see [`Fit`])
#[allow(clippy::too_many_arguments)]
pub fn plan_fit(prompt: &str, lyrics: &str, model: &[(String, String)], reference: Option<&RefInfo>, length: f64, seed: u64, guide: Option<&std::sync::Arc<crate::guide::Guide>>, conform: f64, fit: &Fit) -> SongPlan {
    plan_story(prompt, lyrics, model, reference, length, seed, guide, conform, fit, "")
}

/// [`plan_fit`] with the story the song should tell (free text: "a hero leaves home, loses everything and comes back")
#[allow(clippy::too_many_arguments)]
pub fn plan_story(prompt: &str, lyrics: &str, model: &[(String, String)], reference: Option<&RefInfo>, length: f64, seed: u64, guide: Option<&std::sync::Arc<crate::guide::Guide>>, conform: f64, fit: &Fit, story_text: &str) -> SongPlan {
    let conform = conform.clamp(0.0, 1.0);
    // an ensemble named in the description (or the model's instruments) decides the parts, and the genre unless one is named
    let ens = crate::ensemble::ensemble_in(prompt).or_else(|| field(model, "instruments").and_then(crate::ensemble::ensemble_in));
    // fitting to the MIDI uses its form and tempo even when none of its notes are followed
    let guide_form = guide.filter(|_| *fit == Fit::Midi);
    let guide = guide.filter(|_| conform > 0.0);
    let mut notes = vec![];
    let has_lyrics = !lyrics.trim().is_empty();
    // genre
    let g = if let Some(g) = prompt_genre(prompt) {
        notes.push(format!("genre {} (from the prompt)", g.name));
        g
    } else if let Some(e) = ens {
        notes.push(format!("genre {} (a {})", e.genre, e.name));
        genre(e.genre)
    } else if let Some(m) = field(model, "genre").and_then(prompt_genre) {
        notes.push(format!("genre {} (from the model)", m.name));
        m
    } else if let Some(r) = reference.filter(|r| !r.genre.is_empty()) {
        notes.push(format!("genre {} (sounds like the reference)", r.genre));
        genre(&r.genre)
    } else {
        notes.push("genre pop (nothing named one)".into());
        genre("pop")
    };
    // tempo
    let bpm = if let Some(b) = prompt_bpm(prompt) {
        b
    } else if let Some(gd) = guide.or(guide_form) {
        notes.push(format!("{:.0} bpm from the guide MIDI", gd.bpm));
        gd.bpm
    } else if let Some(b) = field(model, "bpm").and_then(|s| s.split_whitespace().next()).and_then(|s| s.parse::<f64>().ok()).filter(|b| (40.0..=220.0).contains(b)) {
        b
    } else if let Some(r) = reference.filter(|r| r.bpm > 40.0) {
        notes.push(format!("{:.0} bpm from the reference", r.bpm));
        r.bpm
    } else {
        g.bpm
    };
    // key and progression: the composer's own parser reads "in F# minor", "i VI III VII", "Am F C G"
    let pf = compose::parse_fields(prompt);
    let mf: Vec<(String, String)> = model
        .iter()
        .filter(|(k, _)| k == "key" || k == "prog" || k == "progression" || k == "chorus_progression")
        .map(|(k, v)| (if k == "progression" { "prog".into() } else if k == "chorus_progression" { "chorus".into() } else { k.clone() }, v.clone()))
        .collect();
    let def_key = guide.map(|gd| gd.key).or_else(|| reference.and_then(|r| r.key)).unwrap_or(Key::new(9, compose::spec_from_style(g.style, Kind::Chords, 4, Key::new(9, true), 1).key.minor));
    let key = if field(&pf, "key").is_some() || field(&mf, "key").is_some() {
        compose::Spec::from_fields(&[&mf, &pf], def_key, Kind::Chords).key
    } else {
        if guide.is_some() {
            notes.push(format!("key {} from the guide MIDI", def_key.name()));
        } else if reference.and_then(|r| r.key).is_some() {
            notes.push(format!("key {} from the reference", def_key.name()));
        }
        def_key
    };
    let prog = if let Some(p) = field(&pf, "prog") {
        p.to_string()
    } else if let Some(gd) = guide.filter(|gd| conform >= 0.2 && !gd.prog.is_empty()) {
        notes.push(format!("chords {} from the guide MIDI", gd.prog));
        gd.prog.clone()
    } else if let Some((p, why)) = field(&mf, "prog").map(|p| (p, check_prog(p, key))).and_then(|(raw, (p, why))| {
        if p.is_empty() {
            notes.push(format!("the model's chords \"{raw}\" are not a progression in {}: the genre's are used", key.name()));
            None
        } else {
            Some((p, why))
        }
    }) {
        notes.push(format!("chords {p} from the model{}", if why.is_empty() { ", checked: they fit the key".to_string() } else { format!(", checked: {}", why.join(", ")) }));
        p
    } else if let Some(r) = reference.filter(|r| !r.prog.is_empty() && r.key.map(|k| k.minor == key.minor).unwrap_or(false)) {
        notes.push(format!("chords {} from the reference", r.prog));
        r.prog.clone()
    } else {
        String::new()
    };
    // structure
    let mut sections = if let Some(gd) = guide_form.filter(|gd| !gd.sections.is_empty()) {
        notes.push(format!("the form and length of the guide MIDI ({} bars)", gd.sections.iter().map(|s| s.bars).sum::<u32>()));
        gd.sections.clone()
    } else if let Some(s) = prompt_sections(prompt, g.form) {
        notes.push("structure from the prompt".into());
        s
    } else if let Some(gd) = guide.filter(|gd| conform >= 0.3 && gd.sections.len() >= 2) {
        notes.push(format!("structure of the guide MIDI ({} sections)", gd.sections.len()));
        gd.sections.clone()
    } else if let Some(s) = field(model, "structure").and_then(|t| prompt_sections(&format!("structure {t}"), g.form)) {
        s
    } else if let Some(r) = reference.filter(|r| r.sections.len() >= 3) {
        notes.push(format!("structure of the reference ({} sections)", r.sections.len()));
        r.sections.clone()
    } else {
        let l = prompt.to_lowercase();
        let scale = if l.contains("short") { 0.5 } else if l.contains("long") || l.contains("extended") { 1.5 } else { length.clamp(0.25, 2.0) };
        form_sections(g.form, scale)
    };
    if *fit == Fit::Midi && guide_form.is_none() {
        notes.push("no guide MIDI to fit to: the form's length is used".into());
    }
    fit_length(&mut sections, fit, lyrics, bpm.round().max(40.0), &mut notes);
    // the story: an arc over the sections' energy and a chapter title for each
    let story = story_arc(&format!("{story_text} {prompt}"));
    if !story.is_empty() {
        apply_arc(&mut sections, &story);
        let ch = chapters(&story, sections.len());
        notes.push(format!("story ({story}): {}", sections.iter().zip(&ch).map(|(s, c)| format!("{} = {c}", s.tag)).collect::<Vec<_>>().join(", ")));
    }
    // instruments: the ensemble's, or the genre's; plus what the prompt or the model asks for (instruments by name too)
    let mut roles: Vec<String> = match ens {
        Some(e) => {
            notes.push(format!("a {}: {}", e.name, e.parts.join(", ")));
            e.parts.iter().map(|r| r.to_string()).collect()
        }
        None => g.roles.iter().map(|r| r.to_string()).collect(),
    };
    let mut asked: Vec<String> = crate::ensemble::instruments_in(prompt).into_iter().map(str::to_string).collect();
    if ens.is_none() {
        asked.extend(prompt_roles(prompt));
    }
    if let Some(m) = field(model, "instruments") {
        asked.extend(prompt_roles(m));
    }
    let lower = prompt.to_lowercase();
    if lower.contains("no drums") || lower.contains("without drums") {
        roles.retain(|r| r != "drums");
    }
    for r in asked {
        if !roles.contains(&r) {
            roles.push(r);
        }
    }
    // the parts the file has and the genre lacks (a bass line, a melody ...) are added once the song leans on the file
    if let Some(gd) = guide.filter(|_| conform >= 0.4) {
        for t in &gd.tracks {
            if t.kind != Kind::Drums && !roles.iter().any(|r| role_kind(r) == t.kind) && roles.len() < 9 {
                roles.push(t.role.to_string());
                notes.push(format!("added a {} part like the guide's \"{}\"", t.role, t.name));
            }
        }
    }
    roles.truncate(9);
    let singer = prompt_singer(prompt, has_lyrics);
    if !singer.is_empty() {
        notes.push(format!("Ling Voice '{singer}' sings{}", if has_lyrics { " the lyrics" } else { "" }));
    }
    // the chorus may have chords of its own (from the model, checked like the verse's)
    let chorus_prog = match field(&mf, "chorus").filter(|c| !c.trim().is_empty()) {
        Some(c) if field(&pf, "prog").is_none() => {
            let (p, why) = check_prog(c, key);
            if !p.is_empty() && p != prog {
                notes.push(format!("chorus chords {p} from the model{}", if why.is_empty() { String::new() } else { format!(", checked: {}", why.join(", ")) }));
                p
            } else {
                String::new()
            }
        }
        _ => String::new(),
    };
    let human = if ["quantized", "quantised", "robotic", "on the grid", "tight"].iter().any(|w| lower.contains(w)) {
        0.0
    } else if ["live", "loose", "human", "band", "organic"].iter().any(|w| lower.contains(w)) {
        0.85
    } else {
        genre_feel(g.name)
    };
    let (color, ornament) = genre_spice(g.name);
    let mut p = SongPlan {
        genre: g.name.into(),
        bpm: bpm.round(),
        key,
        prog,
        sections,
        parts: roles.into_iter().map(|role| Part { role }).collect(),
        singer,
        seed,
        notes,
        guide: guide.cloned(),
        conform,
        chorus_prog,
        human,
        story,
        arc: field(model, "arc").map(|a| a.trim().to_lowercase()).filter(|a| crate::emotion::ARCS.contains(&a.as_str())).unwrap_or_else(|| "auto".into()),
        intro: prompt_intro(prompt).map(str::to_string).or_else(|| field(model, "intro").map(|a| a.trim().to_lowercase()).filter(|a| INTROS.contains(&a.as_str()))).unwrap_or_else(|| "auto".into()),
        movements: prompt_movements(prompt).map(str::to_string).or_else(|| field(model, "movements").map(|a| a.trim().to_lowercase()).filter(|a| MOVEMENT_CHOICES.contains(&a.as_str()))).unwrap_or_else(|| "auto".into()),
        energy: 0.0,
        drive: 0.5,
        color,
        ornament,
        theme: None,
        ensemble: ens.map(|e| e.name.to_string()).unwrap_or_default(),
        dials: genre_dials(g.name),
    };
    // the mood dial widens the contrast between quiet and loud sections (a form borrowed from a reference keeps its own energy)
    let borrowed = reference.map(|r| r.sections == p.sections).unwrap_or(false) || guide.map(|gd| gd.sections == p.sections).unwrap_or(false);
    let mean = p.sections.iter().map(|s| s.energy).sum::<f64>() / p.sections.len().max(1) as f64;
    let swing = if borrowed { 1.0 } else { 1.0 + (p.dials.mood - 0.35) * 0.8 };
    for s in p.sections.iter_mut() {
        s.energy = (mean + (s.energy - mean) * swing).clamp(0.05, 1.0);
    }
    // no jarring jumps: only a chorus / drop may leap up, only a break / bridge / outro may fall far (flow.rs-style continuity)
    let smoothed = smooth_energy(&mut p.sections);
    if smoothed > 0 {
        p.notes.push(format!("energy curve: {smoothed} section(s) eased so the song never jumps without a reason"));
    }
    // the emotional plan, before a note is written (emotion.rs); its arc steers the intro and the progression family below
    let words = lyrics_for_sections(lyrics, &p.sections);
    let ep = crate::emotion::plan(&p.sections, &p.arc, &p.genre, &words, p.dials.mood, p.seed);
    if p.arc == "auto" && crate::emotion::ARCS.contains(&ep.arc.as_str()) {
        p.arc = ep.arc.clone();
    }
    if p.intro == "auto" {
        p.intro = intro_style(&p).to_string();
    }
    apply_intro(&mut p.sections, &p.intro);
    if let Some(n) = intro_note(&p) {
        p.notes.push(n);
    }
    let hs = harmony_summary(&p);
    p.notes.push(hs);
    let a = arrange(&p);
    p.notes.extend(a.notes);
    p.notes.extend(movement_notes(&p));
    p.notes.extend(craft_notes(&p));
    p.notes.extend(flow_notes(&p));
    p.notes.push(format!("feel: {}", crate::emotion::summary(&ep, &p.sections)));
    p
}

/// movements the prompt asks for ("two movements", "3 movements", "single movement")
fn prompt_movements(prompt: &str) -> Option<&'static str> {
    let l = prompt.to_lowercase();
    if !l.contains("movement") {
        return None;
    }
    if l.contains("one movement") || l.contains("single movement") || l.contains("1 movement") {
        Some("1")
    } else if l.contains("two movements") || l.contains("2 movements") {
        Some("2")
    } else if l.contains("three movements") || l.contains("3 movements") || l.contains("movements") {
        Some("3")
    } else {
        None
    }
}

// ── the intro: how the song opens ───────────────────────────────────────────

/// swell = pads and chords fade in; solo = one instrument alone (piano, keys, guitar, pluck); beat = the groove first (drums, then
/// bass) over a held tonic; theme = the hook played by an instrument over a single chord part; cold = a cold open on the chorus;
/// drone = a pedal on the tonic with the arp shimmering over it
pub const INTROS: [&str; 7] = ["auto", "swell", "solo", "beat", "theme", "cold", "drone"];

/// an intro the prompt asks for ("cold open", "piano intro", "drum intro" ...)
fn prompt_intro(prompt: &str) -> Option<&'static str> {
    let l = prompt.to_lowercase();
    let has = |w: &[&str]| w.iter().any(|x| l.contains(x));
    if has(&["cold open", "start with the chorus", "starts with the chorus", "open on the chorus", "straight into"]) {
        Some("cold")
    } else if has(&["drum intro", "beat intro", "drums first", "starts with drums", "start with drums", "groove intro"]) {
        Some("beat")
    } else if has(&["piano intro", "solo intro", "guitar intro", "keys intro", "starts with piano", "start with piano"]) {
        Some("solo")
    } else if has(&["drone intro", "pedal intro", "ambient intro"]) {
        Some("drone")
    } else if has(&["theme intro", "hook intro", "melody intro", "starts with the hook", "start with the hook"]) {
        Some("theme")
    } else if has(&["pad intro", "swell intro", "fade in"]) {
        Some("swell")
    } else {
        None
    }
}

/// The intro an "auto" plan rolls: weighted dice over the styles, the weights from the emotional arc (a lullaby starts on a lone
/// piano, a storm on the beat or a drone, a triumph on the hook) and the form (club music opens on the groove, a score on a drone),
/// and only styles the parts can play.
pub fn intro_style(plan: &SongPlan) -> &'static str {
    if let Some(i) = INTROS[1..].iter().copied().find(|i| *i == plan.intro) {
        return i;
    }
    if plan.sections.first().map(|s| s.tag != "intro").unwrap_or(true) {
        return "swell";
    }
    let kinds: Vec<Kind> = plan.parts.iter().map(|p| role_kind(&p.role)).collect();
    let has = |k: Kind| kinds.contains(&k);
    let melody = plan.parts.iter().any(|p| role_kind(&p.role) == Kind::Melody && p.role != "riser");
    let form = genre(&plan.genre).form;
    //                       swell solo beat theme cold drone
    let mut w: [f64; 6] = match plan.arc.as_str() {
        "release" => [1.0, 1.0, 2.0, 2.0, 2.0, 0.5],
        "slow burn" => [2.0, 1.5, 1.5, 0.5, 0.0, 2.5],
        "bittersweet" => [1.5, 3.0, 0.5, 2.0, 0.5, 1.0],
        "triumph" => [1.0, 0.5, 2.0, 3.0, 2.0, 0.5],
        "melancholy" => [1.5, 3.0, 0.5, 1.0, 0.0, 2.0],
        "storm" => [0.5, 0.5, 3.0, 1.0, 2.0, 2.5],
        "lullaby" => [2.0, 3.5, 0.0, 1.5, 0.0, 1.5],
        _ => [1.5, 1.5, 1.5, 1.5, 1.0, 1.0],
    };
    match form {
        Form::Club => {
            w[2] *= 1.8;
            w[5] *= 1.4;
            w[4] *= 0.5;
        }
        Form::Score => {
            w[2] *= 0.3;
            w[5] *= 1.6;
            w[4] = 0.0;
        }
        Form::Song => {}
    }
    if !(has(Kind::Chords) || has(Kind::Arp)) {
        w[1] = 0.0;
    }
    if !has(Kind::Drums) {
        w[2] = 0.0;
    }
    if !melody {
        w[3] = 0.0;
    }
    if !plan.sections.iter().any(|s| is_chorus(&s.tag)) {
        w[4] = 0.0;
    }
    if !(has(Kind::Pad) || has(Kind::Arp)) {
        w[5] = 0.0;
    }
    let total: f64 = w.iter().sum();
    if total <= 0.0 {
        return "swell";
    }
    let mut rng = compose::Rng::new(plan.seed.wrapping_mul(0x2545_F491).wrapping_add(0x1A7));
    let mut x = (rng.next() % 1_000_000) as f64 / 1_000_000.0 * total;
    for (i, wi) in w.iter().enumerate() {
        if x < *wi {
            return INTROS[i + 1];
        }
        x -= wi;
    }
    "swell"
}

/// a cold open carries the chorus's energy (the rest keep theirs)
pub fn apply_intro(sections: &mut [Section], intro: &str) {
    if intro != "cold" || sections.first().map(|s| s.tag != "intro").unwrap_or(true) {
        return;
    }
    if let Some(e) = sections.iter().find(|s| is_chorus(&s.tag)).map(|s| s.energy) {
        sections[0].energy = sections[0].energy.max(e * 0.9);
    }
}

fn intro_note(plan: &SongPlan) -> Option<String> {
    if plan.sections.first().map(|s| s.tag != "intro").unwrap_or(true) {
        return None;
    }
    Some(match plan.intro.as_str() {
        "solo" => "intro: one instrument alone, the band waits".into(),
        "beat" => "intro: the groove first over a held tonic".into(),
        "theme" => "intro: the hook on an instrument over the chorus chords".into(),
        "cold" => "intro: a cold open on the chorus".into(),
        "drone" => "intro: a drone on the tonic with the arp over it".into(),
        _ => "intro: pads and chords swell in".into(),
    })
}

// ── length: a duration, the guide MIDI, or room for the lyrics ───────────────

/// syllables sung comfortably per second, with room to hold notes (fast rap is 5+, a ballad 1.5)
pub const SYL_PER_SEC: f64 = 2.2;

/// the syllables of some lyrics (sustain marks "~" "_" "-" do not count) and their lines
pub fn syllable_count(text: &str) -> (usize, usize) {
    let mut n = 0;
    let mut lines = 0;
    for l in text.lines().map(str::trim).filter(|l| !l.is_empty() && !(l.starts_with('[') && l.ends_with(']'))) {
        lines += 1;
        n += crate::lyricfit::syllabify_line(l, crate::lyrics::Lang::Auto).iter().filter(|s| s.orth != "~").count();
    }
    (n, lines)
}

/// the bars a section needs to sing `text` at a relaxed pace: [`SYL_PER_SEC`] plus a beat to breathe after every line, in 4-bar phrases
pub fn bars_for_lyrics(text: &str, bpm: f64) -> u32 {
    let (n, lines) = syllable_count(text);
    if n == 0 {
        return 0;
    }
    let beats = n as f64 / SYL_PER_SEC * bpm / 60.0 + lines as f64;
    (((beats / 4.0) / 4.0).ceil() * 4.0).max(4.0) as u32
}

fn mmss(secs: f64) -> String {
    let s = secs.round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// sections scaled to about `target` bars: proportional, 4-bar steps, at least 4 bars each; when every section is at 4 bars and it is
/// still too long, repeated sections in the middle go; when too short, the loudest sections grow
fn scale_to(sections: &mut Vec<Section>, target: u32) {
    let total = |s: &[Section]| s.iter().map(|x| x.bars).sum::<u32>();
    let cur = total(sections);
    if cur == 0 || target == 0 {
        return;
    }
    let f = target as f64 / cur as f64;
    for s in sections.iter_mut() {
        s.bars = round4(s.bars as f64 * f);
    }
    // the nearest 4-bar step: over by 3 or more shrinks, short by 3 or more grows
    while total(sections) >= target + 3 {
        if let Some(i) = (0..sections.len()).filter(|i| sections[*i].bars > 4).max_by_key(|i| (sections[*i].bars, usize::MAX - *i)) {
            sections[i].bars -= 4;
        } else if sections.len() > 3 {
            // a repeated section from the middle (never the first or the last)
            let n = sections.len();
            let rep = (1..n - 1).rev().find(|i| sections.iter().filter(|s| s.tag == sections[*i].tag).count() > 1).unwrap_or(n / 2);
            sections.remove(rep);
        } else {
            break;
        }
    }
    while total(sections) + 3 <= target {
        let Some(i) = (0..sections.len()).filter(|i| sections[*i].bars < 32).max_by(|a, b| sections[*a].energy.total_cmp(&sections[*b].energy).then(sections[*b].bars.cmp(&sections[*a].bars))) else { break };
        sections[i].bars += 4;
    }
}

/// Apply the length rule: a duration scales the form; the lyrics then get their room (a sung section is never shorter than its lyrics
/// need, and with "lyrics" it is exactly that long); with a duration the instrumental sections give the extra time back.
fn fit_length(sections: &mut Vec<Section>, fit: &Fit, lyrics: &str, bpm: f64, notes: &mut Vec<String>) {
    let bars_total = |s: &[Section]| s.iter().map(|x| x.bars).sum::<u32>();
    let secs = |bars: u32| bars as f64 * 240.0 / bpm;
    let target = if let Fit::Duration(t) = fit { Some(((t * bpm / 240.0).round() as u32).max(4)) } else { None };
    if let Some(t) = target {
        scale_to(sections, t);
    }
    let mut grown = 0u32;
    if !lyrics.trim().is_empty() && *fit != Fit::Midi {
        let words = lyrics_for_sections(lyrics, sections);
        for (s, w) in sections.iter_mut().zip(&words) {
            let need = bars_for_lyrics(w, bpm);
            if need == 0 {
                continue;
            }
            if *fit == Fit::Lyrics || s.bars < need {
                if s.bars < need {
                    grown += need - s.bars;
                }
                s.bars = need;
            }
        }
        if let Some(t) = target {
            // give the time back from the sections without words (intro, bridge, outro ...)
            let words = lyrics_for_sections(lyrics, sections);
            while bars_total(sections) >= t + 3 {
                let Some(i) = (0..sections.len()).filter(|i| words[*i].trim().is_empty() && sections[*i].bars > 4).max_by_key(|i| sections[*i].bars) else { break };
                sections[i].bars -= 4;
            }
        }
    }
    let total = bars_total(sections);
    let mut why = format!("{} long ({} bars)", mmss(secs(total)), total);
    match fit {
        Fit::Duration(t) => {
            why += &format!(", asked {}", mmss(*t));
            if secs(total) > t + 4.0 * 240.0 / bpm && grown > 0 {
                why += ": the lyrics need the extra time";
            }
        }
        Fit::Lyrics => why += &format!(", sung sections sized to their lyrics (about {SYL_PER_SEC} syllables a second)"),
        Fit::Midi => {}
        Fit::Auto => {
            if grown > 0 {
                why += &format!(", {grown} bars added so the lyrics are not rushed");
            }
        }
    }
    notes.push(why);
}

/// The sung line of a section reshaped for `syllables` syllables: about one note each, nothing shorter than a sixteenth (shorter notes
/// join the one before), long notes split on the eighth grid while there are too few, and with far too many the shortest notes become
/// sustains of the note before them - so the lyric fitter never has to cram syllables into 32nd notes and there is room to hold notes.
pub fn fit_line(notes: &mut Vec<N>, syllables: usize) {
    if notes.is_empty() || syllables == 0 {
        return;
    }
    notes.sort_by(|a, b| a.start.total_cmp(&b.start));
    let join = |notes: &mut Vec<N>, i: usize| {
        let end = notes[i].start + notes[i].len;
        notes.remove(i);
        if i > 0 {
            let p = &mut notes[i - 1];
            p.len = (end - p.start).max(p.len);
        }
    };
    let mut i = 1;
    while i < notes.len() {
        if notes[i].len < 0.24 {
            join(notes, i);
        } else {
            i += 1;
        }
    }
    while notes.len() < syllables {
        let min = if notes.iter().any(|n| n.len >= 1.0) { 1.0 } else { 0.5 };
        let Some(i) = (0..notes.len()).filter(|i| notes[*i].len >= min - 1e-9).max_by(|a, b| notes[*a].len.total_cmp(&notes[*b].len)) else { break };
        let n = notes[i];
        let half = ((n.len / 2.0 / 0.25).round() * 0.25).max(0.25);
        notes[i].len = half;
        notes.insert(i + 1, N { start: n.start + half, len: n.len - half, ..n });
    }
    let cap = (syllables as f64 * 1.3).ceil() as usize + 1;
    while notes.len() > cap {
        let Some(i) = (1..notes.len()).min_by(|a, b| notes[*a].len.total_cmp(&notes[*b].len)) else { break };
        join(notes, i);
    }
}

/// how much harmonic colour and melodic ornament a genre takes by default
pub fn genre_spice(name: &str) -> (f64, f64) {
    match name {
        "jazz" | "lofi" | "deep house" | "disco" | "hip hop" => (0.6, 0.35),
        "cinematic" | "orchestral" | "ambient" | "folk" => (0.35, 0.4),
        "trance" | "techno" | "hardstyle" | "dubstep" | "drum and bass" => (0.15, 0.2),
        _ => (0.25, 0.25),
    }
}

// ── the story ────────────────────────────────────────────────────────────────

/// the arc a story text describes ("" = none)
pub fn story_arc(text: &str) -> String {
    let t = text.to_lowercase();
    let has = |ws: &[&str]| ws.iter().any(|w| t.contains(w));
    let arc = if has(&["hero", "quest", "returns", "comes back", "homecoming", "redemption"]) {
        "hero"
    } else if has(&["tragedy", "tragic", "falls", "loses", "loss", "fades", "sunset", "farewell", "goodbye", "decline"]) {
        "fall"
    } else if has(&["battle", "war", "storm", "chase", "fight", "escape", "danger"]) {
        "battle"
    } else if has(&["dream", "sleep", "lullaby", "float", "calm", "meditat", "drift"]) {
        "dream"
    } else if has(&["journey", "travel", "voyage", "road", "adventure", "wander"]) {
        "journey"
    } else if has(&["rise", "rising", "triumph", "victory", "climb", "sunrise", "awaken", "hope"]) {
        "rise"
    } else {
        ""
    };
    arc.to_string()
}

/// the energy an arc wants at a point of the song (0..1 through it)
fn arc_level(arc: &str, x: f64) -> f64 {
    match arc {
        "rise" => 0.25 + 0.75 * x,
        "fall" => 1.0 - 0.7 * x,
        "battle" => 0.75 + 0.25 * (std::f64::consts::PI * x).sin(),
        "dream" => 0.3 + 0.15 * (std::f64::consts::PI * x).sin(),
        "journey" => 0.45 + 0.4 * (2.0 * std::f64::consts::PI * x - 1.2).sin().abs(),
        // the hero's journey: the call, the descent to the lowest point two thirds in, the return in triumph
        _ => {
            if x < 0.25 {
                0.35 + x
            } else if x < 0.65 {
                0.6 - (x - 0.25) * 0.8
            } else {
                0.3 + (x - 0.65) * 2.0
            }
        }
    }
}

/// the sections' energy pulled towards the arc (each keeps 40 % of its own, so a chorus is still louder than its verse)
pub fn apply_arc(sections: &mut [Section], arc: &str) {
    let total: u32 = sections.iter().map(|s| s.bars).sum::<u32>().max(1);
    let mut at = 0u32;
    for s in sections.iter_mut() {
        let x = (at as f64 + s.bars as f64 * 0.5) / total as f64;
        s.energy = (0.4 * s.energy + 0.6 * arc_level(arc, x).clamp(0.05, 1.0)).clamp(0.05, 1.0);
        at += s.bars;
    }
}

/// a chapter title per section for an arc
pub fn chapters(arc: &str, n: usize) -> Vec<String> {
    let names: &[&str] = match arc {
        "rise" => &["Before Dawn", "First Light", "Climbing", "The Summit", "Daylight"],
        "fall" => &["Golden Hour", "Cracks", "Letting Go", "Embers", "Silence"],
        "battle" => &["The Horizon Darkens", "First Clash", "The Storm", "The Turn", "Aftermath"],
        "dream" => &["Drifting Off", "Clouds", "The Deep Dream", "Floating", "Waking"],
        "journey" => &["Departure", "Open Road", "Strange Lands", "Crossroads", "Arrival"],
        _ => &["The Call", "Leaving Home", "Trials", "The Abyss", "Return"],
    };
    (0..n).map(|i| names[(i * names.len() / n.max(1)).min(names.len() - 1)].to_string()).collect()
}

// ── the radar: what the planned song is like ─────────────────────────────────

pub const METRICS: [&str; 7] = ["Energy", "Beat", "Harmony", "Melody", "Groove", "Structure", "Story"];

/// seven 0..1 measures of a plan, like a dance game's groove radar: energy (how loud the sections are, peak and average), beat
/// (tempo and how busy the drums / bass play), harmony (chord colour: sevenths, borrowed chords, a chorus with its own chords, the
/// colour amount), melody (a lead or a singer, a theme, ornament), groove (human feel and swing), structure (how many different
/// sections and how long), story (an arc and how far the energy travels)
pub fn metrics(p: &SongPlan) -> Vec<f64> {
    let e: Vec<f64> = p.sections.iter().map(|s| (s.energy + p.energy).clamp(0.0, 1.0)).collect();
    let mean = e.iter().sum::<f64>() / e.len().max(1) as f64;
    let peak = e.iter().copied().fold(0.0, f64::max);
    let low = e.iter().copied().fold(1.0, f64::min);
    let g = genre(&p.genre);
    let has = |k: Kind| p.parts.iter().any(|x| role_kind(&x.role) == k);
    let style = compose::spec_from_style(g.style, Kind::Drums, 4, p.key, 1);
    let beat = (0.45 * ((p.bpm - 60.0) / 120.0).clamp(0.0, 1.0) + 0.35 * style.density * (p.drive * 2.0).clamp(0.2, 1.6) / 1.6 + if has(Kind::Drums) { 0.2 } else { 0.0 }).clamp(0.0, 1.0);
    let prog = if p.prog.is_empty() { String::new() } else { p.prog.clone() };
    let toks: Vec<&str> = prog.split_whitespace().collect();
    let rich = toks.iter().filter(|t| t.contains('7') || t.contains('9') || t.starts_with('b') || t.starts_with('#') || t.contains("sus")).count() as f64 / toks.len().max(1) as f64;
    let harmony = (0.15 + 0.2 * rich + if p.chorus_prog.is_empty() { 0.0 } else { 0.1 } + 0.25 * p.color + 0.25 * p.dials.variety + 0.05 * p.dials.fifths.abs() as f64 + 0.1 * p.dials.temperature).clamp(0.0, 1.0);
    let lead = has(Kind::Melody) || !p.singer.is_empty();
    let melody = ((if lead { 0.4 } else { 0.1 }) + if p.theme.is_some() { 0.25 } else { 0.0 } + if !p.singer.is_empty() { 0.1 } else { 0.0 } + 0.35 * p.ornament).clamp(0.0, 1.0);
    let swing = matches!(style.feel.as_str(), "swing" | "syncopated" | "offbeat") as u8 as f64;
    let groove = (0.6 * p.human + 0.3 * swing + 0.1).clamp(0.0, 1.0);
    let mut tags: Vec<&str> = p.sections.iter().map(|s| s.tag.as_str()).collect();
    tags.sort_unstable();
    tags.dedup();
    let structure = (0.6 * (tags.len() as f64 / 6.0) + 0.4 * (p.sections.len() as f64 / 10.0)).clamp(0.0, 1.0);
    let story = ((if p.story.is_empty() { 0.15 } else { 0.5 }) + 0.5 * (peak - low)).clamp(0.0, 1.0);
    vec![(0.6 * mean + 0.4 * peak).clamp(0.0, 1.0), beat, harmony, melody, groove, structure, story]
}

/// how loosely a genre is played by default (0 = sequenced on the grid, 1 = a live band)
pub fn genre_feel(name: &str) -> f64 {
    match name {
        "trance" | "techno" | "hardstyle" | "hyperpop" | "chiptune" | "trap" => 0.2,
        "house" | "deep house" | "drum and bass" | "dubstep" | "synthwave" | "garage" | "reggaeton" | "pop" => 0.35,
        "ambient" | "cinematic" | "orchestral" | "witch house" => 0.5,
        _ => 0.75,
    }
}

impl SongPlan {
    pub fn bars(&self) -> u32 {
        self.sections.iter().map(|s| s.bars).sum()
    }
    /// "genre=trance; bpm=138; key=A minor; prog=i VI III VII; structure=intro 8, build 8, drop 16; parts=drums, rolling bass; voice=aria; seed=7"
    pub fn to_line(&self) -> String {
        let st: Vec<String> = self.sections.iter().map(|s| format!("{} {}", s.tag, s.bars)).collect();
        let parts: Vec<&str> = self.parts.iter().map(|p| p.role.as_str()).collect();
        format!(
            "genre={}; bpm={}; key={}; prog={}; chorus={}; structure={}; parts={}; voice={}; human={:.2}; story={}; energy={:+.2}; drive={:.2}; color={:.2}; ornament={:.2}; temp={:.2}; variety={:.2}; mood={:.2}; fifths={:+}; progstyle={}; layering={:.2}; design={:.2}; arc={}; intro={}; movements={}; seed={}",
            self.genre,
            self.bpm,
            self.key.name(),
            if self.prog.is_empty() { "auto" } else { &self.prog },
            if self.chorus_prog.is_empty() { "same" } else { &self.chorus_prog },
            st.join(", "),
            parts.join(", "),
            if self.singer.is_empty() { "none" } else { &self.singer },
            self.human,
            if self.story.is_empty() { "none" } else { &self.story },
            self.energy,
            self.drive,
            self.color,
            self.ornament,
            self.dials.temperature,
            self.dials.variety,
            self.dials.mood,
            self.dials.fifths,
            self.dials.style,
            self.dials.layering,
            self.dials.design,
            self.arc,
            self.intro,
            self.movements,
            self.seed
        )
    }
    /// the line back into a plan (what the user typed wins; anything missing stays as `base`)
    pub fn from_line(line: &str, base: &SongPlan) -> SongPlan {
        let mut p = base.clone();
        for item in line.split(';') {
            let Some((k, v)) = item.split_once('=') else { continue };
            let (k, v) = (k.trim().to_lowercase(), v.trim());
            match k.as_str() {
                "genre" => p.genre = genre(v).name.into(),
                "bpm" => {
                    if let Ok(b) = v.parse::<f64>() {
                        p.bpm = b.clamp(40.0, 220.0);
                    }
                }
                "key" => p.key = compose::Spec::from_fields(&[&[("key".to_string(), v.to_string())]], p.key, Kind::Chords).key,
                "prog" => p.prog = if v.eq_ignore_ascii_case("auto") { String::new() } else { v.to_string() },
                "chorus" => p.chorus_prog = if v.eq_ignore_ascii_case("same") || v.eq_ignore_ascii_case("auto") { String::new() } else { v.to_string() },
                "arc" => {
                    let lv = v.trim().to_lowercase();
                    p.arc = if crate::emotion::ARCS.contains(&lv.as_str()) { lv } else { "auto".into() };
                }
                "intro" => {
                    let lv = v.trim().to_lowercase();
                    p.intro = if INTROS.contains(&lv.as_str()) { lv } else { "auto".into() };
                    if p.intro == "auto" {
                        p.intro = intro_style(&p).to_string();
                    }
                    apply_intro(&mut p.sections, &p.intro);
                }
                "movements" => {
                    let lv = v.trim().to_lowercase();
                    p.movements = if MOVEMENT_CHOICES.contains(&lv.as_str()) { lv } else { "auto".into() };
                }
                "story" => {
                    let lv = v.trim().to_lowercase();
                    let arc = if lv == "none" { String::new() } else if ["rise", "fall", "battle", "dream", "journey", "hero"].contains(&lv.as_str()) { lv } else { story_arc(&lv) };
                    let arc = if ["rise", "fall", "battle", "dream", "journey", "hero"].contains(&arc.as_str()) { arc } else { String::new() };
                    if arc != p.story {
                        if !arc.is_empty() {
                            apply_arc(&mut p.sections, &arc);
                        }
                        p.story = arc;
                    }
                }
                "energy" => {
                    if let Ok(x) = v.trim_start_matches('+').parse::<f64>() {
                        p.energy = x.clamp(-0.3, 0.3);
                    }
                }
                "drive" => {
                    if let Ok(x) = v.parse::<f64>() {
                        p.drive = x.clamp(0.0, 1.0);
                    }
                }
                "color" => {
                    if let Ok(x) = v.parse::<f64>() {
                        p.color = x.clamp(0.0, 1.0);
                    }
                }
                "temp" | "temperature" => {
                    if let Ok(x) = v.parse::<f64>() {
                        p.dials.temperature = x.clamp(0.0, 1.0);
                    }
                }
                "variety" => {
                    if let Ok(x) = v.parse::<f64>() {
                        p.dials.variety = x.clamp(0.0, 1.0);
                    }
                }
                "mood" => {
                    if let Ok(x) = v.parse::<f64>() {
                        p.dials.mood = x.clamp(0.0, 1.0);
                    }
                }
                "fifths" => {
                    if let Ok(x) = v.trim_start_matches('+').parse::<i32>() {
                        p.dials.fifths = x.clamp(-3, 3);
                    }
                }
                "progstyle" => {
                    let lv = v.trim().to_lowercase();
                    if PROG_STYLES.contains(&lv.as_str()) {
                        p.dials.style = lv;
                    }
                }
                "layering" => {
                    if let Ok(x) = v.parse::<f64>() {
                        p.dials.layering = x.clamp(0.0, 1.0);
                    }
                }
                "design" => {
                    if let Ok(x) = v.parse::<f64>() {
                        p.dials.design = x.clamp(0.0, 1.0);
                    }
                }
                "ornament" => {
                    if let Ok(x) = v.parse::<f64>() {
                        p.ornament = x.clamp(0.0, 1.0);
                    }
                }
                "human" => {
                    if let Ok(h) = v.parse::<f64>() {
                        p.human = h.clamp(0.0, 1.0);
                    }
                }
                "structure" => {
                    // unchanged: keep the energies (the reference's, or the form's); edited: read it again
                    let was: Vec<String> = base.sections.iter().map(|s| format!("{} {}", s.tag, s.bars)).collect();
                    if v != was.join(", ") {
                        if let Some(s) = prompt_sections(&format!("structure {}", v.replace(',', " ")), genre(&p.genre).form) {
                            p.sections = s;
                        }
                    }
                }
                "parts" => p.parts = v.split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).map(|role| Part { role }).collect(),
                "voice" => p.singer = if v.eq_ignore_ascii_case("none") { String::new() } else { v.to_lowercase() },
                "seed" => {
                    if let Ok(s) = v.parse::<u64>() {
                        p.seed = s;
                    }
                }
                _ => {}
            }
        }
        p
    }
}

// ── movements: the song in two or three tempos, pads against drive ─────────────────────────────────────────

pub const MOVEMENT_CHOICES: [&str; 4] = ["auto", "1", "2", "3"];

/// One movement of the song: a run of sections with its own tempo, texture and (maybe) key. The emotion plan decides what kind it is
/// and how the tempo arrives (see [`movements`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Movement {
    /// "opening", "main", "reflective" or "lift"
    pub name: &'static str,
    /// "pads" (drums out, sustained chords), "halftime" (a slow groove), "drive" (the song's own band), "lift" (the final push)
    pub texture: &'static str,
    pub first: usize,
    pub last: usize,
    pub bpm: f64,
    /// how the tempo gets here: "start", "accel" / "rit" (a ramp over the last bars of the movement before), "break" (a beat of
    /// silence, then the new tempo)
    pub arrive: &'static str,
    /// semitones above the home key (the final lift)
    pub key_lift: i32,
    /// the key leans to the relative major / minor, or the subdominant (a reflective movement)
    pub relative: bool,
    pub why: String,
}

pub fn is_build_tag(tag: &str) -> bool {
    matches!(tag, "build" | "buildup" | "rise")
}
fn is_pre_tag(tag: &str) -> bool {
    matches!(tag, "pre" | "prechorus" | "pre-chorus")
}

/// the emotion plan the arrangement reads (no lyrics, so it only depends on the plan itself)
fn plan_feels(plan: &SongPlan) -> Vec<crate::emotion::Feel> {
    crate::emotion::plan(&plan.sections, &plan.arc, &plan.genre, &[], plan.dials.mood, plan.seed).feels
}

/// start beat of every section
fn section_starts(plan: &SongPlan) -> Vec<f64> {
    let mut at = 0.0;
    plan.sections
        .iter()
        .map(|s| {
            let st = at;
            at += s.bars as f64 * 4.0;
            st
        })
        .collect()
}

/// How the song divides into movements. `plan.movements` is "auto" (the arc, the length and the dice decide) or 1 / 2 / 3. An
/// ambient opening of pads, a driving main movement, a slower reflective one (pads alone when the feeling lets go, a half-time
/// groove when the tension is held) and a final lift (faster, a key up) are chosen from the emotional arc: triumph and release end
/// in a lift, the sadder arcs in a reflective movement. A movement can only begin on a section that exists: the sections after the
/// intro, the first break or bridge, the build before the last chorus or drop.
pub fn movements(plan: &SongPlan) -> Vec<Movement> {
    use crate::emotion;
    let ns = plan.sections.len();
    let one = |why: &str| {
        vec![Movement { name: "main", texture: "drive", first: 0, last: ns.saturating_sub(1), bpm: plan.bpm, arrive: "start", key_lift: 0, relative: false, why: why.into() }]
    };
    if ns < 3 || plan.movements == "1" {
        return one("one movement: the song keeps its tempo and key");
    }
    let bars = plan.bars();
    let roll = mix64(plan.seed, 0x30F3) % 100;
    let want = match plan.movements.as_str() {
        "2" => 2,
        "3" => 3,
        _ => {
            if ns < 5 || bars < 48 {
                1
            } else {
                match plan.arc.as_str() {
                    "slow burn" | "triumph" | "storm" | "bittersweet" => if bars >= 72 { 3 } else { 2 },
                    "lullaby" | "melancholy" => 2,
                    _ => if bars >= 88 && roll < 45 { 3 } else { 2 },
                }
            }
        }
    };
    if want == 1 {
        return one("one movement: the song is too short to change tempo");
    }
    let tag = |i: usize| plan.sections[i].tag.as_str();
    let b = (2..ns.saturating_sub(1)).find(|i| emotion::is_release_tag(tag(*i)));
    let l = (0..ns).rev().find(|i| is_chorus(tag(*i))).map(|mut l| {
        while l > 0 && (is_build_tag(tag(l - 1)) || is_pre_tag(tag(l - 1))) {
            l -= 1;
        }
        l
    });
    let l = l.filter(|l| *l >= 2 && b.map(|b| *l > b).unwrap_or(true));
    let open = if ns >= 4 && tag(0) == "intro" && !matches!(plan.intro.as_str(), "beat" | "cold" | "theme") { Some(1usize) } else { None };
    let prefer_lift = matches!(plan.arc.as_str(), "triumph" | "release");
    // the movement after the main one, as (name, first section) entries: a reflective one lasts as long as the release sections do,
    // then the main tempo returns (or the lift takes over); the lift runs to the end
    let second = |prefer_lift: bool, with_lift: bool| -> Vec<(&'static str, usize)> {
        match (prefer_lift, b, l) {
            (true, _, Some(l)) => vec![("lift", l)],
            (_, Some(b), _) => {
                let mut e = b;
                while e + 1 < ns && emotion::is_release_tag(tag(e + 1)) {
                    e += 1;
                }
                let mut v = vec![("reflective", b)];
                match l {
                    Some(l) if with_lift && l > e => {
                        if l > e + 1 {
                            v.push(("main", e + 1));
                        }
                        v.push(("lift", l));
                    }
                    _ if e + 1 < ns => v.push(("main", e + 1)),
                    _ => {}
                }
                v
            }
            (_, None, Some(l)) => vec![("lift", l)],
            _ => vec![],
        }
    };
    let mut layout: Vec<(&'static str, usize)> = vec![];
    if want == 3 {
        let x = second(prefer_lift, false);
        if let (Some(o), false) = (open, x.is_empty()) {
            layout = vec![("opening", 0), ("main", o)];
            layout.extend(x);
        } else if b.is_some() && l.is_some() {
            layout = vec![("main", 0)];
            layout.extend(second(false, true));
        }
    }
    if layout.is_empty() {
        let x = second(prefer_lift, false);
        if x.is_empty() {
            return one("one movement: no break or last chorus to change on");
        }
        layout = vec![("main", 0)];
        layout.extend(x);
    }
    layout.dedup_by_key(|x| x.1);
    let feels = plan_feels(plan);
    let f = |i: usize| feels.get(i).copied().unwrap_or(crate::emotion::Feel { valence: 0.0, arousal: 0.5, tension: 0.4, vocal: 0.5, breath: 0.3, space: 0.5 });
    let club = genre(&plan.genre).form == Form::Club;
    let round = |x: f64| x.round().clamp(52.0, 220.0);
    let mut out: Vec<Movement> = vec![];
    for (mi, (name, first)) in layout.iter().enumerate() {
        let last = layout.get(mi + 1).map(|n| n.1 - 1).unwrap_or(ns - 1);
        let ff = f(*first);
        let mv = match *name {
            "opening" => Movement {
                name: "opening",
                texture: "pads",
                first: *first,
                last,
                bpm: round(plan.bpm * (0.84 + 0.1 * ff.arousal as f64)),
                arrive: "start",
                key_lift: 0,
                relative: false,
                why: format!("opening: pads alone (arousal {:.2}), the drums wait for the main movement", ff.arousal),
            },
            "reflective" => {
                let pads = emotion::wants_pads(&ff);
                let half_dnb = plan.genre == "drum and bass" && !pads;
                let ratio = if half_dnb { 0.5 } else if pads { 0.72 + 0.12 * ff.arousal as f64 } else { 0.8 + 0.08 * ff.arousal as f64 };
                Movement {
                    name: "reflective",
                    texture: if pads { "pads" } else { "halftime" },
                    first: *first,
                    last,
                    bpm: round(plan.bpm * ratio),
                    arrive: if club { "break" } else { "rit" },
                    key_lift: 0,
                    relative: true,
                    why: format!(
                        "reflective from the {}: {} (tension {:.2}, arousal {:.2}) - {}, the key leans to the {}",
                        tag(*first),
                        if pads { "pads carry it, the drums rest" } else { "a half-time groove holds the tension" },
                        ff.tension,
                        ff.arousal,
                        if club { "the tempo changes after a beat of silence" } else { "a ritardando into it" },
                        if plan.key.minor { "subdominant" } else { "relative minor" }
                    ),
                }
            }
            "lift" => {
                let fin = f(ns - 1).arousal.max(ff.arousal);
                let semis = emotion::lift_semitones(&ff);
                Movement {
                    name: "lift",
                    texture: "lift",
                    first: *first,
                    last,
                    bpm: round(plan.bpm * (1.03 + 0.05 * fin as f64)),
                    arrive: if club { "break" } else { "accel" },
                    key_lift: semis as i32,
                    relative: false,
                    why: format!("final lift from the {}: faster ({}), the key goes up {} semitone{} (valence {:+.2}, arousal {:.2})", tag(*first), if club { "after a beat of silence" } else { "an accelerando into it" }, semis, if semis == 1 { "" } else { "s" }, ff.valence, ff.arousal),
                }
            }
            _ => Movement {
                name: "main",
                texture: "drive",
                first: *first,
                last,
                bpm: plan.bpm,
                arrive: if mi == 0 { "start" } else { "accel" },
                key_lift: 0,
                relative: false,
                why: if mi == 0 { "main movement: the song's tempo, key and full band".into() } else { format!("back to the main tempo and key at the {} ({})", tag(*first), if layout[0].0 == "opening" && mi == 1 { "an accelerando out of the pads" } else { "an accelerando out of the slow movement" }) },
            },
        };
        out.push(mv);
    }
    out
}

/// the movement a section is in (only when the song has more than one)
pub fn movement_of(plan: &SongPlan, si: usize) -> Option<Movement> {
    let mv = movements(plan);
    if mv.len() < 2 {
        return None;
    }
    mv.into_iter().find(|m| si >= m.first && si <= m.last)
}

/// the key a movement puts a section in (the lift a step up, a reflective movement towards the relative / subdominant)
fn movement_key(plan: &SongPlan, si: usize) -> Option<Key> {
    let m = movement_of(plan, si)?;
    let home = plan.key;
    if m.key_lift != 0 {
        return Some(home.transposed(m.key_lift));
    }
    if m.relative {
        return Some(if home.minor { home.transposed(5) } else { Key::new((home.tonic + 9) % 12, true) });
    }
    None
}

/// The tempo map of the song as (beat, bpm): the opening tempo when it is not the song's, and for every movement change either a
/// ritardando / accelerando (a step a beat over the last one or two bars of the section before) or a plain change on the downbeat (the
/// silence before it is [`gap_after`]).
pub fn tempo_changes(plan: &SongPlan) -> Vec<(f64, f64)> {
    let mv = movements(plan);
    if mv.len() < 2 {
        return vec![];
    }
    let starts = section_starts(plan);
    let mut out: Vec<(f64, f64)> = vec![];
    if (mv[0].bpm - plan.bpm).abs() > 0.5 {
        out.push((0.0, mv[0].bpm));
    }
    for i in 1..mv.len() {
        let (from, to) = (mv[i - 1].bpm, mv[i].bpm);
        let at = starts[mv[i].first];
        if matches!(mv[i].arrive, "accel" | "rit") {
            let prev_bars = plan.sections[mv[i].first - 1].bars as f64;
            let ramp = if prev_bars >= 8.0 { 8.0 } else { (prev_bars * 2.0).max(2.0) }.min(prev_bars * 4.0);
            let steps = ramp as usize;
            for s in 1..steps {
                let t = s as f64 / steps as f64;
                let e = t * t * (3.0 - 2.0 * t);
                out.push((at - ramp + s as f64, ((from + (to - from) * e) * 10.0).round() / 10.0));
            }
        }
        out.push((at, to));
    }
    out
}

/// the movements in words, with why (for the plan's notes and the build summary)
pub fn movement_notes(plan: &SongPlan) -> Vec<String> {
    let mv = movements(plan);
    if mv.len() < 2 {
        return vec![];
    }
    let starts = section_starts(plan);
    let bar = |si: usize| (starts[si] / 4.0).round() as u32 + 1;
    let head: Vec<String> = mv.iter().map(|m| format!("{} ({}, {:.0} bpm, from bar {})", m.name, m.texture, m.bpm, bar(m.first))).collect();
    let mut names: Vec<&str> = mv.iter().map(|m| m.name).collect();
    names.dedup();
    names.sort_unstable();
    names.dedup();
    let mut notes = vec![format!("movements={}: {}", names.len(), head.join(" > "))];
    notes.extend(mv.iter().map(|m| m.why.clone()).filter(|w| !w.starts_with("main movement")));
    notes
}

/// Beats of silence at the end of section `si`: the held breath before a drop or a chorus (a build whose next section is a drop; a
/// tense pre-chorus gets half a beat) and before a tempo change that comes after a break. How long it hangs is the tension.
pub fn gap_after(plan: &SongPlan, si: usize) -> f64 {
    let Some(next) = plan.sections.get(si + 1) else { return 0.0 };
    let sec = &plan.sections[si];
    let feels = plan_feels(plan);
    let Some(f) = feels.get(si) else { return 0.0 };
    let club = genre(&plan.genre).form == Form::Club;
    let build_like = is_build_tag(&sec.tag) || (club && is_pre_tag(&sec.tag));
    if is_chorus(&next.tag) && build_like {
        return crate::emotion::gap_beats(f).min(sec.bars as f64 * 2.0);
    }
    if is_chorus(&next.tag) && is_pre_tag(&sec.tag) && f.tension >= 0.7 {
        return 0.5;
    }
    if movements(plan).iter().any(|m| m.first == si + 1 && m.arrive == "break") {
        return 1.0;
    }
    0.0
}

/// what the build-ups do and why, for the plan's notes (the tension of each build decides how long the silence hangs and when the roll speeds up)
pub fn craft_notes(plan: &SongPlan) -> Vec<String> {
    let feels = plan_feels(plan);
    let mut out = vec![];
    let mut drops = 0;
    for si in 0..plan.sections.len() {
        if !leads_into_drop(plan, si) {
            continue;
        }
        drops += 1;
        let sec = &plan.sections[si];
        let gap = gap_after(plan, si);
        let t = feels.get(si).map(|f| f.tension).unwrap_or(0.5);
        let cuts = feels.get(si).map(crate::emotion::roll_stages).unwrap_or([0.45, 0.72, 0.9]);
        out.push(format!(
            "{} {}: snare roll 1/4 > 1/8 > 1/16 > 1/32 (it speeds up at {:.0}% / {:.0}% / {:.0}%), a riser and the filter opening, then {} of silence before the {} (tension {:.2}: {}); after the hit a crash and a downlifter",
            sec.tag,
            drops,
            cuts[0] * 100.0,
            cuts[1] * 100.0,
            cuts[2] * 100.0,
            if gap >= 2.0 { "half a bar".to_string() } else if gap >= 1.0 { "a beat".to_string() } else { "an eighth".to_string() },
            plan.sections[si + 1].tag,
            t,
            if t >= 0.7 { "held long" } else if t >= 0.45 { "a short stop" } else { "a quick breath" }
        ));
    }
    let nd = plan.sections.iter().filter(|s| is_chorus(&s.tag) && genre(&plan.genre).form == Form::Club).count();
    if nd >= 2 && plan.dials.variety >= 0.15 {
        out.push("the second drop switches the bass pattern over the same drums and lead".into());
    }
    out
}

/// Does the section lead into a drop (a build / a club pre-chorus before a chorus or drop)?
fn leads_into_drop(plan: &SongPlan, si: usize) -> bool {
    let Some(sec) = plan.sections.get(si) else { return false };
    let club = genre(&plan.genre).form == Form::Club;
    (is_build_tag(&sec.tag) || (club && is_pre_tag(&sec.tag))) && plan.sections.get(si + 1).map(|n| is_chorus(&n.tag)).unwrap_or(false)
}

/// An accelerating snare / clap roll for a build: quarter notes, then eighths, sixteenths and thirty-seconds; the stage changes
/// are where the feeling puts them ([`crate::emotion::roll_stages`]). It starts two bars in on a long build and stops where the
/// silence before the drop begins. The velocity climbs the whole way.
pub fn build_roll(sec: &Section, feel: &crate::emotion::Feel, gap: f64) -> Vec<N> {
    let len = sec.bars as f64 * 4.0;
    let end = len - gap;
    let t0 = if sec.bars >= 8 { 8.0 } else if sec.bars >= 4 { 4.0 } else { 0.0 };
    if end - t0 < 2.0 {
        return vec![];
    }
    let cuts = crate::emotion::roll_stages(feel);
    let mut out = vec![];
    let (mut t, mut i) = (t0, 0usize);
    while t < end - 1e-6 {
        let frac = (t - t0) / (end - t0);
        let stage = if frac < cuts[0] { 0 } else if frac < cuts[1] { 1 } else if frac < cuts[2] { 2 } else { 3 };
        let step: f64 = [1.0, 0.5, 0.25, 0.125][stage];
        let key = if stage >= 2 && i % 2 == 1 { compose::K_CLAP } else { compose::K_SNARE };
        out.push(N { start: t, len: (step * 0.9).min(end - t), key, vel: (0.32 + 0.66 * frac.powf(1.2)) as f32 });
        t += step;
        i += 1;
    }
    out
}

/// The riser track's notes: a rising ladder through the build (one step every two beats, then a beat, a half, a quarter, so it speeds
/// up like the roll) climbing two octaves of the key, and a falling downlifter in the first bars of the drop after it.
fn riser_notes(plan: &SongPlan, sec: &Section, si: Option<usize>) -> Vec<N> {
    let len = sec.bars as f64 * 4.0;
    let key = si.map(|i| section_key(plan, i)).unwrap_or(plan.key);
    let base = 48 + key.tonic;
    let tag = sec.tag.as_str();
    if is_build_tag(tag) || is_pre_tag(tag) {
        let feel = si.and_then(|i| plan_feels(plan).get(i).copied());
        let gap = si.map(|i| gap_after(plan, i)).unwrap_or(0.0);
        let end = len - gap;
        let cuts = feel.map(|f| crate::emotion::roll_stages(&f)).unwrap_or([0.45, 0.72, 0.9]);
        let mut out = vec![];
        let mut t = 0.0;
        while t < end - 1e-6 {
            let frac = t / end;
            let step: f64 = if frac < cuts[0] { 2.0 } else if frac < cuts[1] { 1.0 } else if frac < cuts[2] { 0.5 } else { 0.25 };
            out.push(N { start: t, len: (step * 1.05).min(end - t), key: dia(key, base, (frac * 14.0).round() as i32), vel: (0.3 + 0.6 * frac) as f32 });
            t += step;
        }
        return out;
    }
    if tag == "drop" || is_chorus(tag) {
        // the downlifter, only right after a build
        if si.map(|i| i > 0 && leads_into_drop(plan, i - 1)).unwrap_or(false) {
            return (0..8).map(|i| N { start: i as f64, len: 1.0, key: dia(key, base + 24, -(i * 2)), vel: 0.7 - 0.07 * i as f32 }).collect();
        }
        return vec![];
    }
    vec![N { start: 0.0, len, key: base, vel: 0.7 }]
}

/// A section's notes cut `gap` beats before its end (the held breath): notes starting in it go, notes ringing into it stop
fn cut_gap(notes: &mut Vec<N>, len: f64, gap: f64) {
    if gap <= 0.0 {
        return;
    }
    let cut = len - gap;
    notes.retain(|n| n.start < cut - 1e-6);
    for n in notes.iter_mut() {
        if n.start + n.len > cut {
            n.len = (cut - n.start).max(0.02);
        }
    }
}

// ── flow: how the sections connect ──────────────────────────────────────────
//
// A producer joins sections: the drums fill into every new section and crash on its downbeat, the lead runs into the next phrase
// (a pickup that walks to its first note), the bass approaches chord changes by step, the chords move as little as they can (common
// tones held), a noise sweep / reverse swell rises into a drop, the energy never jumps without a reason (a drop or a breakdown, with
// its setup) and the last bar resolves on the tonic. The emotion plan sets how big each gesture is.

/// The energy curve without jarring jumps: a section may rise more than 0.35 above the one before only when it is a chorus / drop /
/// climax or a build, and fall more than 0.45 only into a break, bridge, outro or intro. Returns how many sections were eased.
pub fn smooth_energy(sections: &mut [Section]) -> usize {
    let mut n = 0;
    for i in 1..sections.len() {
        let prev = sections[i - 1].energy;
        let tag = sections[i].tag.clone();
        let e = sections[i].energy;
        let up_ok = is_chorus(&tag) || is_build_tag(&tag);
        let down_ok = is_bridge(&tag) || matches!(tag.as_str(), "outro" | "intro" | "breakdown");
        let ne = if e - prev > 0.35 && !up_ok {
            prev + 0.35
        } else if prev - e > 0.45 && !down_ok {
            prev - 0.45
        } else {
            e
        };
        if (ne - e).abs() > 1e-9 {
            sections[i].energy = ne.clamp(0.05, 1.0);
            n += 1;
        }
    }
    n
}

/// A drum fill over the last `beats` of a section: eighths, sixteenths in the last two beats, snare then the toms high to low,
/// louder towards the barline; a calm feeling plays it on the snare and the rim only. The groove's backbeat, open hats and the
/// kicks after the fill's first beat make way.
pub fn transition_fill(len: f64, beats: f64, arousal: f32, notes: &mut Vec<N>) {
    if beats < 0.5 || len < 2.0 {
        return;
    }
    let from = (len - beats).max(0.0);
    let toms = [50u8, 48, 47, 45, 43, 41];
    notes.retain(|n| {
        let late = n.start >= from - 1e-6;
        !(late && (matches!(n.key, compose::K_SNARE | compose::K_CLAP | compose::K_RIM | compose::K_HAT_OPEN | compose::K_CRASH) || toms.contains(&n.key)))
            && !(n.start > from + 0.99 && n.key == compose::K_KICK)
            && !(n.start >= len - 1.0 - 1e-6 && n.key == compose::K_HAT)
    });
    let mut t = from;
    let mut i = 0usize;
    while t < len - 1e-6 {
        let frac = (t - from) / beats;
        let step = if len - t <= 2.0 + 1e-6 { 0.25 } else { 0.5 };
        let key = if arousal < 0.4 {
            if i % 2 == 0 { compose::K_SNARE } else { compose::K_RIM }
        } else if frac < 0.35 {
            compose::K_SNARE
        } else {
            toms[(((frac - 0.35) / 0.65) * toms.len() as f64) as usize % toms.len()]
        };
        notes.push(N { start: t, len: step * 0.8, key, vel: ((0.55 + 0.4 * frac) as f32 * (0.8 + 0.25 * arousal)).clamp(0.2, 1.0) });
        t += step;
        i += 1;
    }
    notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.key.cmp(&b.key)));
}

/// Bass approach notes: at some chord changes (more of them when the feeling pushes) the bass note in the beat before the change
/// becomes a scale step next to the new root, from the side the line comes from; with nothing there, an eighth is added.
fn bass_approach(plan: &SongPlan, si: usize, spec: &compose::Spec, arousal: f32, notes: &mut Vec<N>) {
    if spec.pattern == "walking" || notes.is_empty() {
        return;
    }
    let (key, chords) = section_chords(plan, si, spec.cpb);
    let len = plan.sections[si].bars as f64 * 4.0;
    let share = 0.3 + 0.4 * arousal as f64;
    notes.sort_by(|a, b| a.start.total_cmp(&b.start));
    for (ci, w) in chords.windows(2).enumerate() {
        let (t, root) = (w[1].0, w[1].2);
        if w[0].2 == root || t >= len - 0.01 {
            continue;
        }
        if unit(mix64(plan.seed ^ 0xA9_9A, tag_hash(&plan.sections[si].tag) << 20 | ci as u64)) * 0.5 + 0.5 >= share {
            continue;
        }
        let near = |k: u8| (0..128i32).filter(|x| (*x % 12) as u8 == root % 12 && *x >= spec.lo as i32 - 2 && *x <= spec.hi as i32 + 2).min_by_key(|x| (*x - k as i32).abs());
        // the last note in the beat before the change (not a bar's downbeat: that is the chord's own root)
        let cand = notes.iter().rposition(|n| n.start >= t - 1.0 - 1e-6 && n.start < t - 0.2 && n.start + n.len <= t + 0.06 && (n.start % 4.0).abs() > 1e-6);
        if let Some(i) = cand {
            let from = notes[i].key;
            let Some(target) = near(from) else { continue };
            let k = if target >= from as i32 { dia(key, target as u8, -1) } else { dia(key, target as u8, 1) };
            notes[i].key = k;
        } else if !notes.iter().any(|n| n.start >= t - 0.5 - 1e-6 && n.start < t) && arousal >= 0.45 {
            let Some(prev) = notes.iter().filter(|n| n.start < t - 0.5).last().map(|n| n.key) else { continue };
            let Some(target) = near(prev) else { continue };
            let k = if target >= prev as i32 { dia(key, target as u8, -1) } else { dia(key, target as u8, 1) };
            for n in notes.iter_mut() {
                if n.start < t - 0.5 && n.start + n.len > t - 0.5 {
                    n.len = (t - 0.5 - n.start).max(0.05);
                }
            }
            notes.push(N { start: t - 0.5, len: 0.45, key: k, vel: 0.7 });
            notes.sort_by(|a, b| a.start.total_cmp(&b.start));
        }
    }
}

/// a section tag as a number (repeated sections make the same choices)
fn tag_hash(tag: &str) -> u64 {
    tag.bytes().fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(b as u64)) & 0xFF_FFFF
}

/// Where the lead plays on into a section without a pickup (the same section again, or a quieter one), its last note lands on the
/// tone of its own chord nearest the next phrase's first note: the contour carries over the boundary instead of jumping.
fn land_near_next(plan: &SongPlan, si: usize, part: usize, spec: &compose::Spec, before: f64, notes: &mut [N]) {
    if !arrange(plan).on.get(part).and_then(|v| v.get(si + 1)).copied().unwrap_or(false) {
        return;
    }
    let Some(first) = shaped(plan, part, &plan.sections[si + 1], None).2.into_iter().min_by(|a, b| a.start.total_cmp(&b.start).then(b.key.cmp(&a.key))).map(|n| n.key as i32) else { return };
    let Some(i) = (0..notes.len()).filter(|i| notes[*i].start < before - 1e-6).max_by(|a, b| notes[*a].start.total_cmp(&notes[*b].start)) else { return };
    let (_, chords) = section_chords(plan, si, spec.cpb);
    let at = notes[i].start;
    let Some(ch) = chords.iter().find(|c| at >= c.0 - 1e-6 && at < c.0 + c.1 - 1e-6) else { return };
    let tones: Vec<u8> = theory::QUAL.get(ch.3).map(|q| q.1.iter().map(|x| (ch.2 + x) % 12).collect()).unwrap_or_default();
    let top = spec.hi.saturating_add(5).min(120);
    let best = (spec.lo..=top).filter(|k| tones.contains(&(k % 12))).min_by_key(|k| (*k as i32 - first).abs());
    if let Some(k) = best {
        if (k as i32 - first).abs() < (notes[i].key as i32 - first).abs() {
            notes[i].key = k;
        }
    }
}

/// notes that start together (2 or more) as groups of indices, in time order
fn chord_groups(notes: &[N]) -> Vec<Vec<usize>> {
    let mut idx: Vec<usize> = (0..notes.len()).collect();
    idx.sort_by(|a, b| notes[*a].start.total_cmp(&notes[*b].start).then(notes[*a].key.cmp(&notes[*b].key)));
    let mut out = vec![];
    let mut i = 0;
    while i < idx.len() {
        let mut j = i + 1;
        while j < idx.len() && (notes[idx[j]].start - notes[idx[i]].start).abs() < 0.03 {
            j += 1;
        }
        if j - i >= 2 {
            out.push(idx[i..j].to_vec());
        }
        i = j;
    }
    out
}

/// a chord's pitch classes placed as close as they can be to the previous voicing (and not far from `centre`), inside lo..=hi
fn revoice(keys: &[u8], prev: &[u8], lo: u8, hi: u8, centre: f64) -> Vec<u8> {
    let mut out: Vec<u8> = vec![];
    let mut ks = keys.to_vec();
    ks.sort_unstable();
    for k in ks {
        let pc = k % 12;
        let cost = |c: u8| prev.iter().map(|p| (*p as f64 - c as f64).abs()).fold(f64::MAX, f64::min) + 0.12 * (c as f64 - centre).abs();
        let best = (lo..=hi).filter(|c| c % 12 == pc && !out.contains(c)).min_by(|a, b| cost(*a).total_cmp(&cost(*b)));
        out.push(best.unwrap_or(k));
    }
    out.sort_unstable();
    out
}

/// Voice leading for chords and pads: every chord placed with the least motion from the one before - across the section boundary
/// too (the previous section's last chord when the part played there) - and a pad holds the tones two chords share.
fn lead_voices(plan: &SongPlan, part: usize, si: usize, spec: &compose::Spec, kind: Kind, notes: &mut Vec<N>) {
    let groups = chord_groups(notes);
    if groups.is_empty() {
        return;
    }
    let mut prev: Vec<u8> = vec![];
    if si > 0 && arrange(plan).on.get(part).and_then(|v| v.get(si - 1)).copied().unwrap_or(false) {
        prev = voiced_last_chord(plan, part, si - 1);
    }
    let (lo, hi) = (spec.lo, spec.hi.saturating_add(4).min(120));
    let home = (spec.lo as f64 + spec.hi as f64) * 0.5;
    for g in groups {
        let mut sorted = g.clone();
        sorted.sort_by_key(|i| notes[*i].key);
        let keys: Vec<u8> = sorted.iter().map(|i| notes[*i].key).collect();
        if prev.is_empty() {
            prev = keys;
            continue;
        }
        let pc = prev.iter().map(|p| *p as f64).sum::<f64>() / prev.len() as f64;
        let nv = revoice(&keys, &prev, lo, hi, 0.7 * pc + 0.3 * home);
        if nv.len() == sorted.len() {
            for (j, i) in sorted.iter().enumerate() {
                notes[*i].key = nv[j];
            }
            prev = nv;
        } else {
            prev = keys;
        }
    }
    if kind == Kind::Pad {
        hold_common_tones(notes);
    }
}

/// the chord a part ends section `si` on, as the voice leading placed it (the chain runs back to where the part came in)
fn voiced_last_chord(plan: &SongPlan, part: usize, si: usize) -> Vec<u8> {
    let (spec, kind, mut notes) = shaped(plan, part, &plan.sections[si], None);
    lead_voices(plan, part, si, &spec, kind, &mut notes);
    let t = notes.iter().map(|n| n.start).fold(f64::MIN, f64::max) + 0.01;
    notes.iter().filter(|n| n.start <= t && n.start + n.len > t).map(|n| n.key).collect()
}

/// A pad's common tones are held: a note that two chords in a row share is one long note over both.
fn hold_common_tones(notes: &mut Vec<N>) {
    let groups = chord_groups(notes);
    let mut holder: Vec<usize> = (0..notes.len()).collect();
    let mut gone = vec![false; notes.len()];
    for w in groups.windows(2) {
        let s2 = notes[w[1][0]].start;
        for &j in &w[1] {
            let Some(&i) = w[0].iter().find(|&&i| notes[i].key == notes[j].key) else { continue };
            let h = holder[i];
            if gone[h] || (notes[h].start + notes[h].len - s2).abs() > 0.2 {
                continue;
            }
            notes[h].len = notes[j].start + notes[j].len - notes[h].start;
            notes[h].vel = notes[h].vel.max(notes[j].vel);
            gone[j] = true;
            holder[j] = h;
        }
    }
    let mut k = 0;
    notes.retain(|_| {
        let keep = !gone[k];
        k += 1;
        keep
    });
}

/// The ending resolves: the last bar of the song is the tonic - the bass on its root, the chords / pads / arps on the tonic triad
/// (voiced from the chord before it, an arp breaks it once), the lead on the tonic nearest its last note; the drums hit a crash and
/// a kick on the downbeat and stop.
fn final_cadence(plan: &SongPlan, si: usize, kind: Kind, spec: &compose::Spec, notes: &mut Vec<N>) {
    let len = plan.sections[si].bars as f64 * 4.0;
    if len < 4.0 || notes.is_empty() {
        return;
    }
    let lb = len - 4.0;
    let key = section_key(plan, si);
    let tonic = key.tonic % 12;
    let triad = [tonic, (tonic + if key.minor { 3 } else { 4 }) % 12, (tonic + 7) % 12];
    let before: Vec<N> = notes.iter().filter(|n| n.start < lb - 1e-6).cloned().collect();
    let last = before.iter().max_by(|a, b| a.start.total_cmp(&b.start)).map(|n| n.key);
    let trim = |notes: &mut Vec<N>| {
        notes.retain(|n| n.start < lb - 1e-6);
        for n in notes.iter_mut() {
            if n.start + n.len > lb {
                n.len = (lb - n.start).max(0.05);
            }
        }
    };
    let nearest = |pc: u8, to: u8, lo: u8, hi: u8| (lo..=hi).filter(|k| k % 12 == pc).min_by_key(|k| (*k as i32 - to as i32).abs());
    match kind {
        Kind::Drums => {
            trim(notes);
            notes.push(N { start: lb, len: 4.0, key: compose::K_CRASH, vel: 0.9 });
            notes.push(N { start: lb, len: 1.0, key: compose::K_KICK, vel: 0.95 });
        }
        Kind::Bass => {
            let to = last.unwrap_or(spec.lo + 5);
            trim(notes);
            if let Some(k) = nearest(tonic, to, spec.lo, spec.hi.max(spec.lo + 12)) {
                notes.push(N { start: lb, len: 3.8, key: k, vel: 0.85 });
            }
        }
        Kind::Melody => {
            let to = last.unwrap_or((spec.lo + spec.hi) / 2);
            trim(notes);
            if let Some(k) = nearest(tonic, to, spec.lo, spec.hi.saturating_add(5).min(120)) {
                notes.push(N { start: lb, len: 3.6, key: k, vel: 0.74 });
            }
        }
        Kind::Chords | Kind::Pad | Kind::Arp => {
            let groups = chord_groups(&before);
            let prev: Vec<u8> = groups.last().map(|g| g.iter().map(|i| before[*i].key).collect()).unwrap_or_else(|| last.into_iter().collect());
            trim(notes);
            let centre = if prev.is_empty() { (spec.lo as f64 + spec.hi as f64) * 0.5 } else { prev.iter().map(|p| *p as f64).sum::<f64>() / prev.len() as f64 };
            let seed: Vec<u8> = triad.iter().map(|pc| nearest(*pc, centre as u8, spec.lo, spec.hi.saturating_add(4).min(120)).unwrap_or(60)).collect();
            let v = revoice(&seed, if prev.is_empty() { &seed } else { &prev }, spec.lo, spec.hi.saturating_add(4).min(120), centre);
            for (i, k) in v.iter().enumerate() {
                let at = if kind == Kind::Arp { i as f64 * 0.25 } else { 0.0 };
                notes.push(N { start: lb + at, len: 3.8 - at, key: *k, vel: 0.7 });
            }
        }
    }
}

/// Slides on a mono line that wants them (sounddesign::expression): some note pairs a step to an octave apart, the first long enough,
/// with a short gap between them, are written legato (the first note overlaps the next, so the instrument glides there); every other
/// pair of a mono line is detached, so nothing glides by accident. Chords, pads, arps and drums are never touched.
fn apply_slides(plan: &SongPlan, part: usize, si: usize, kind: Kind, singer: bool, notes: &mut [N]) {
    if singer || !matches!(kind, Kind::Bass | Kind::Melody) || notes.len() < 2 {
        return;
    }
    let x = crate::sounddesign::expression(&plan.genre, &plan.parts[part].role, kind, plan.bpm);
    if !x.mono {
        return;
    }
    notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.key.cmp(&b.key)));
    for i in 0..notes.len() - 1 {
        let (a, b) = (notes[i], notes[i + 1]);
        if b.start - a.start < 0.2 {
            continue;
        }
        // a per-note slide (an 808 glide, a lead's slide) leaves the notes detached: `note_gestures` bends them
        let pick = !x.slide_gesture && slide_picked(plan, part, si, &x, notes, i);
        if pick {
            notes[i].len = b.start - a.start + 0.06;
        } else if a.start + a.len > b.start - 0.01 {
            notes[i].len = (b.start - a.start - 0.02).max(0.05);
        }
    }
}

/// Is the pair (i, i + 1) of a sorted mono line one of the slides the expression rules ask for? (the same choice for the legato
/// overlap and the per-note gesture)
fn slide_picked(plan: &SongPlan, part: usize, si: usize, x: &crate::sounddesign::Expr, notes: &[N], i: usize) -> bool {
    let (a, b) = (notes[i], notes[i + 1]);
    if b.start - a.start < 0.2 {
        return false;
    }
    let arousal = plan_feels(plan).get(si).map(|f| f.arousal as f64).unwrap_or(0.5);
    let share = x.slides * (0.75 + 0.5 * arousal);
    let gap = b.start - (a.start + a.len);
    let iv = (b.key as i32 - a.key as i32).unsigned_abs();
    let eligible = x.glide_secs > 0.0 && gap <= 0.35 && a.len >= x.slide_min && iv >= 1 && iv <= x.slide_max as u32;
    eligible && unit(mix64(plan.seed ^ 0x511D_E5, (part as u64) << 40 | tag_hash(&plan.sections[si].tag) << 16 | i as u64)) * 0.5 + 0.5 < share
}

/// The per-note pitch gestures the expression rules write into a part's section (`section_notes` order) - the generator's only
/// way past the pitch guard, each one on purpose:
/// * 808 glides (sub in trap / hip hop / future bass) and EDM lead slides: the chosen pairs bend into the next note (`slide`);
/// * future bass: a scoop into every chord entry of the chords / pads in a drop or chorus;
/// * the riser part: its last note before the drop rises an octave (`riser`);
/// * dubstep: the laser / screech lead dives an octave on the last note of a section that ends in a fill (not into the gap of a
///   drop: the build has the riser there).
/// Returns (note index, curve). Acid / growl / reese slides stay legato overlaps (`apply_slides`).
pub fn note_gestures(plan: &SongPlan, part: usize, sec: &Section, notes: &[N]) -> Vec<(usize, crate::model::Curve)> {
    let Some(si) = section_index(plan, sec) else { return vec![] };
    let role = plan.parts[part].role.as_str();
    let kind = role_kind(role);
    let len = sec.bars as f64 * 4.0;
    let mut out: Vec<(usize, crate::model::Curve)> = vec![];
    let mut order: Vec<usize> = (0..notes.len()).collect();
    order.sort_by(|a, b| notes[*a].start.total_cmp(&notes[*b].start).then(notes[*a].key.cmp(&notes[*b].key)));
    let sorted: Vec<N> = order.iter().map(|i| notes[*i]).collect();
    let to_model = |v: &[N]| -> Vec<crate::model::Note> { v.iter().map(|n| crate::model::Note::new(n.start, n.len, n.key, n.vel)).collect() };
    let x = crate::sounddesign::expression(&plan.genre, role, kind, plan.bpm);
    if x.slide_gesture && x.mono && sorted.len() >= 2 {
        for i in 0..sorted.len() - 1 {
            if slide_picked(plan, part, si, &x, &sorted, i) {
                let iv = sorted[i + 1].key as f32 - sorted[i].key as f32;
                if let Some(c) = crate::gesture::shape("slide", iv, sorted[i].len) {
                    out.push((order[i], c));
                }
            }
        }
    }
    let tag = sec.tag.as_str();
    let kind_name = match kind {
        Kind::Chords | Kind::Pad if plan.genre == "future bass" && (tag == "drop" || is_chorus(tag)) && role != "riser" => Some("scoop"),
        _ if role == "riser" && (is_build_tag(tag) || leads_into_drop(plan, si)) => Some("riser"),
        Kind::Melody if plan.genre == "dubstep" && (role.contains("laser") || role.contains("screech")) && si + 1 < plan.sections.len() && !leads_into_drop(plan, si) => Some("dive"),
        _ => None,
    };
    if let Some(k) = kind_name {
        let (from, depth) = match k {
            // the fill is the last bar
            "dive" => ((len - 4.0).max(0.0), 12.0),
            "riser" => ((len - gap_after(plan, si) - 4.0).max(0.0), 12.0),
            _ => (0.0, 3.0),
        };
        let to = if k == "riser" { len - gap_after(plan, si) } else { len };
        let mut m2 = to_model(&sorted);
        if k == "dive" {
            // only the very last note of the section dives
            let last = (0..m2.len()).filter(|i| m2[*i].start >= from).max_by(|a, b| m2[*a].start.total_cmp(&m2[*b].start));
            if let Some(li) = last {
                crate::gesture::apply_shape(&mut m2, li, "dive", depth);
            }
        } else {
            crate::gesture::write_kind(&mut m2, k, from, to, depth);
        }
        for (j, n) in m2.into_iter().enumerate() {
            if !n.bend.is_empty() && !out.iter().any(|o| o.0 == order[j]) {
                out.push((order[j], n.bend));
            }
        }
    }
    out
}

/// genres that use a noise sweep / reverse swell (electronic production); acoustic ones keep their own crescendos
fn electronic(genre: &str) -> bool {
    !matches!(genre, "folk" | "ballad" | "jazz" | "rock" | "orchestral" | "cinematic" | "ambient" | "lofi")
}

/// The noise sweeps (white-noise riser / reverse cymbal) of the song as (start beat, length): one rising into every drop after a
/// build (as long as the tension says: one to four bars) and a short reverse swell (two beats, four when tense) into a chorus the
/// energy jumps to. Each ends where the silence before the drop begins (on the downbeat when there is none).
pub fn sweeps(plan: &SongPlan) -> Vec<(f64, f64)> {
    if !electronic(&plan.genre) {
        return vec![];
    }
    let starts = section_starts(plan);
    let feels = plan_feels(plan);
    let mut out = vec![];
    for si in 0..plan.sections.len().saturating_sub(1) {
        let (sec, next) = (&plan.sections[si], &plan.sections[si + 1]);
        let into_drop = leads_into_drop(plan, si);
        let jump = is_chorus(&next.tag) && next.energy >= sec.energy + 0.25;
        if !(into_drop || jump) {
            continue;
        }
        let gap = gap_after(plan, si);
        let span = sec.bars as f64 * 4.0 - gap;
        let t = feels.get(si).map(|f| f.tension as f64).unwrap_or(0.5);
        let beats = if into_drop { 4.0 * (1.0 + (t * 3.0).round()) } else if t >= 0.6 { 4.0 } else { 2.0 };
        let beats = beats.min(span);
        if beats >= 1.0 {
            out.push((starts[si] + span - beats, beats));
        }
    }
    out
}

/// A sound effect on the timeline for Ling Cinema (cinema.rs): which kind ("impact", "downlifter", "ambience", "braam"), when, how
/// long, on which key and how hard.
#[derive(Clone, Debug, PartialEq)]
pub struct Cue {
    pub what: &'static str,
    pub start: f64,
    pub len: f64,
    pub key: u8,
    pub vel: f32,
}

/// The transition effects and atmospheres of the song, played by Ling Cinema: an impact on the downbeat of every drop or chorus the
/// noise sweeps rise into ([`sweeps`]), a downlifter where a drop falls back to a quiet section, and an ambience held through the
/// intro and the breakdowns; film music gets a braam on its loudest section instead of the club moves. Acoustic genres get none.
pub fn cinema_cues(plan: &SongPlan) -> Vec<Cue> {
    let film = matches!(plan.genre.as_str(), "cinematic");
    let ambient = matches!(plan.genre.as_str(), "ambient");
    if !(electronic(&plan.genre) || film || ambient) || matches!(plan.genre.as_str(), "chiptune") {
        return vec![];
    }
    let starts = section_starts(plan);
    let mut out = vec![];
    let n = plan.sections.len();
    let tonic = plan.key.tonic % 12;
    for si in 0..n {
        let sec = &plan.sections[si];
        let len = sec.bars as f64 * 4.0;
        let key = section_key(plan, si).tonic % 12;
        let next = plan.sections.get(si + 1);
        // the impact: the downbeat a sweep rises into
        if !ambient && si + 1 < n {
            let nx = &plan.sections[si + 1];
            let into_drop = leads_into_drop(plan, si);
            let jump = is_chorus(&nx.tag) && nx.energy >= sec.energy + 0.25;
            if into_drop || jump || (film && nx.energy >= sec.energy + 0.3) {
                out.push(Cue { what: "impact", start: starts[si + 1], len: 4.0, key: 24 + section_key(plan, si + 1).tonic % 12, vel: if into_drop { 0.95 } else { 0.75 } });
            }
        }
        // the downlifter: a drop (or a loud chorus) falling back into something quiet
        if !ambient && !film && (sec.tag == "drop" || is_chorus(&sec.tag)) && sec.energy >= 0.8 {
            if let Some(nx) = next {
                if nx.energy <= sec.energy - 0.3 {
                    out.push(Cue { what: "downlifter", start: starts[si + 1], len: 4.0f64.min(nx.bars as f64 * 4.0), key: 60 + key, vel: 0.8 });
                }
            }
        }
        // the ambience: held through the intro and the breakdowns
        if sec.tag == "intro" || sec.tag == "break" || sec.tag == "breakdown" || (ambient && sec.energy < 0.5) {
            let gap = if si + 1 < n { gap_after(plan, si) } else { 0.0 };
            if len - gap >= 4.0 {
                out.push(Cue { what: "ambience", start: starts[si], len: len - gap, key: 36 + key, vel: 0.7 });
            }
        }
    }
    // film music: one braam on the loudest section's downbeat
    if film {
        if let Some(si) = (0..n).filter(|i| *i > 0).max_by(|a, b| plan.sections[*a].energy.total_cmp(&plan.sections[*b].energy)) {
            out.push(Cue { what: "braam", start: starts[si], len: 8.0f64.min(plan.sections[si].bars as f64 * 4.0), key: 36 + tonic, vel: 0.85 });
        }
    }
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    out
}

/// The Ling Cinema preset that plays a cue
pub fn cue_preset(what: &str, genre: &str) -> &'static str {
    match what {
        "impact" => if matches!(genre, "dubstep" | "drum and bass" | "hardstyle" | "trap" | "cinematic") { "Trailer Hit" } else { "Sub Boom" },
        "downlifter" => "Noise Downlifter",
        "braam" => "Deep Braam",
        _ => if matches!(genre, "ambient" | "cinematic" | "techno" | "witch house") { "Dark Space" } else { "Ambient Drift" },
    }
}

/// what the flow does, for the plan's notes
pub fn flow_notes(plan: &SongPlan) -> Vec<String> {
    let mut out = vec!["flow: a drum fill into every new section (a beat, two or a bar as the feeling pushes) and a crash on its downbeat; the lead runs into the next phrase, the bass approaches chord changes by step; chords voice-led across sections, pads hold their common tones".to_string()];
    let sw = sweeps(plan);
    if !sw.is_empty() {
        out.push(format!("{} noise sweep(s) rise into the drops / choruses", sw.len()));
    }
    let last = plan.sections.len().saturating_sub(1);
    out.push(format!("the ending resolves: the last bar is a {} chord", section_key(plan, last).name()));
    let x: Vec<String> = plan
        .parts
        .iter()
        .filter_map(|p| {
            let e = crate::sounddesign::expression(&plan.genre, &p.role, role_kind(&p.role), plan.bpm);
            if e.why.is_empty() || p.role == "voice" || p.role == "choir" {
                None
            } else {
                Some(format!("{}: {}", p.role, e.why.join(", ")))
            }
        })
        .collect();
    if !x.is_empty() {
        out.push(format!("expression: {}", x.join("; ")));
    }
    out
}

/// The craft on top of a part's phrase: the roll in builds (the groove's snares make way for it), the crash and kick on the first
/// beat after a build or a break, and a pickup into the next section's first chord on the bass and the lead.
fn craft(plan: &SongPlan, si: usize, part: usize, kind: Kind, spec: &compose::Spec, singer: bool, notes: &mut Vec<N>) {
    let sec = &plan.sections[si];
    let len = sec.bars as f64 * 4.0;
    let feels = plan_feels(plan);
    let gap = gap_after(plan, si);
    let after_build = si > 0 && leads_into_drop(plan, si - 1);
    let arousal = feels.get(si).map(|f| f.arousal).unwrap_or(0.5);
    if kind == Kind::Bass {
        bass_approach(plan, si, spec, arousal, notes);
    }
    match kind {
        Kind::Drums => {
            if leads_into_drop(plan, si) {
                if let Some(f) = feels.get(si) {
                    let roll = build_roll(sec, f, gap);
                    if let Some(first) = roll.first() {
                        let from = first.start;
                        notes.retain(|n| !(n.start >= from - 1e-6 && matches!(n.key, compose::K_SNARE | compose::K_CLAP | compose::K_RIM)));
                        notes.extend(roll);
                    }
                }
            }
            // a fill into every new section (a build has its roll instead), as long as the feeling: a beat, two, a whole bar
            if !leads_into_drop(plan, si) && gap <= 0.0 {
                if let Some(next) = plan.sections.get(si + 1) {
                    if next.tag != sec.tag || (next.energy - sec.energy).abs() >= 0.2 {
                        let into_quiet = next.energy < sec.energy - 0.25;
                        let beats: f64 = if into_quiet || arousal < 0.4 { 1.0 } else if arousal < 0.7 { 2.0 } else { 4.0 };
                        transition_fill(len, beats.min(len * 0.5), arousal, notes);
                    }
                }
            }
            // the crash marks the downbeat of every new section (with the kick when the section has one)
            let jump = si > 0 && sec.energy > plan.sections[si - 1].energy + 0.3;
            let new_section = si > 0 && plan.sections[si - 1].tag != sec.tag;
            if after_build || (jump && is_chorus(&sec.tag)) || new_section {
                let hard = after_build || jump;
                notes.retain(|n| !(n.start < 0.05 && matches!(n.key, compose::K_CRASH)));
                notes.push(N { start: 0.0, len: 2.0, key: compose::K_CRASH, vel: if hard { 1.0 } else { 0.72 + 0.25 * arousal } });
                let kicks = notes.iter().any(|n| n.key == compose::K_KICK);
                if (hard || kicks) && !notes.iter().any(|n| n.start < 0.05 && n.key == compose::K_KICK) {
                    notes.push(N { start: 0.0, len: 0.5, key: compose::K_KICK, vel: 1.0 });
                }
            }
            notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.key.cmp(&b.key)));
        }
        Kind::Bass | Kind::Melody if !singer && gap <= 0.0 && !notes.is_empty() => {
            // a pickup (anacrusis) into the next section: the bass a scale step below the next chord's root, an eighth before the
            // barline; the lead a run of one to three sixteenths (the tension decides) that walks into the next phrase's first note
            let Some(next) = plan.sections.get(si + 1) else { return };
            if next.tag == sec.tag || next.energy + 0.15 < sec.energy {
                if kind == Kind::Melody {
                    land_near_next(plan, si, part, spec, len, notes);
                }
                return;
            }
            let (nk, chords) = section_chords(plan, si + 1, spec.cpb);
            let Some(root) = chords.first().map(|c| c.2) else { return };
            let last = notes.iter().filter(|n| n.start < len - 0.5).max_by(|a, b| a.start.total_cmp(&b.start)).map(|n| n.key).unwrap_or(spec.lo);
            let plays_next = arrange(plan).on.get(part).and_then(|v| v.get(si + 1)).copied().unwrap_or(false);
            let phrase = if kind == Kind::Melody && plays_next {
                shaped(plan, part, &plan.sections[si + 1], None).2.into_iter().min_by(|a, b| a.start.total_cmp(&b.start).then(b.key.cmp(&a.key))).map(|n| n.key)
            } else {
                None
            };
            let target = phrase.map(|k| k as i32).or_else(|| (0..128i32).filter(|k| (*k % 12) as u8 == root % 12 && (*k as u8) >= spec.lo && (*k as u8) <= spec.hi).min_by_key(|k| (*k - last as i32).abs()));
            let Some(target) = target else { return };
            let tension = feels.get(si).map(|f| f.tension).unwrap_or(0.5);
            let run = if kind == Kind::Melody && phrase.is_some() { 1 + (tension >= 0.45) as usize + (tension >= 0.7 && is_chorus(&next.tag)) as usize } else { 1 };
            let step = if run > 1 { 0.25 } else { 0.5 };
            let from = len - step * run as f64;
            notes.retain(|n| n.start < from - 1e-6);
            for n in notes.iter_mut() {
                if n.start + n.len > from {
                    n.len = (from - n.start).max(0.05);
                }
            }
            // from below when the line climbs into the next phrase, from above when it comes down
            let dir = if kind == Kind::Melody && phrase.is_some() && (target as i32) < last as i32 { 1 } else { -1 };
            for i in 0..run {
                let k = dia(nk, target as u8, dir * (run - i) as i32);
                notes.push(N { start: from + i as f64 * step, len: step * 0.9, key: k, vel: 0.66 + 0.06 * i as f32 });
            }
        }
        // across the breath before a drop the lead still hands its contour on: its last note before the silence lands near the
        // next phrase's first
        Kind::Melody if !singer && gap > 0.0 && !notes.is_empty() && si + 1 < plan.sections.len() => land_near_next(plan, si, part, spec, len - gap, notes),
        _ => {}
    }
}

// ── writing the notes ───────────────────────────────────────────────────────

/// The notes of one part in one section (clip-relative beats). Sections with the same tag get the same seed, so a chorus comes back as
/// the same chorus; energy scales the density; the last bar of a section gets a fill on the drums.
pub fn section_notes(plan: &SongPlan, part: usize, sec: &Section, singer_range: Option<(u8, u8)>) -> Vec<N> {
    let (spec, kind, mut notes) = shaped(plan, part, sec, singer_range);
    let si = section_index(plan, sec);
    let len = sec.bars as f64 * 4.0;
    if plan.parts[part].role == "riser" {
        if let Some(si) = si {
            cut_gap(&mut notes, len, gap_after(plan, si));
        }
        return notes;
    }
    if kind == Kind::Bass {
        lock_to_kick(plan, sec, &spec, &mut notes);
    }
    blend_guide(plan, part, sec, kind, &spec, &mut notes);
    if let Some(si) = si {
        // chords and pads move as little as they can, across the section boundary too; a pad holds its common tones
        if matches!(kind, Kind::Chords | Kind::Pad) {
            lead_voices(plan, part, si, &spec, kind, &mut notes);
        }
        craft(plan, si, part, kind, &spec, singer_range.is_some(), &mut notes);
        if si + 1 == plan.sections.len() {
            final_cadence(plan, si, kind, &spec, &mut notes);
        }
    }
    humanize(plan, part, sec, kind, &mut notes);
    if let Some(si) = si {
        apply_slides(plan, part, si, kind, singer_range.is_some(), &mut notes);
        // the held breath before a drop: nothing sounds in the last beat(s) of the build
        cut_gap(&mut notes, len, gap_after(plan, si));
    }
    notes
}

/// The phrase as the song shapes it before the band plays it: the composer's notes, the theme or the motif developed, the
/// decorator's ornament / colour. (No bass lock, voice leading, pickups or feel yet: the next section's pickup reads this.)
fn shaped(plan: &SongPlan, part: usize, sec: &Section, singer_range: Option<(u8, u8)>) -> (compose::Spec, Kind, Vec<N>) {
    let (spec, kind, mut notes) = written(plan, part, sec, singer_range);
    if plan.parts[part].role == "riser" {
        return (spec, kind, notes);
    }
    let themed = kind == Kind::Melody && plan.theme.as_ref().map(|t| !t.is_empty()).unwrap_or(false);
    if themed {
        if let Some(v) = theme_notes(plan, part, sec, &spec, singer_range) {
            notes = v;
        }
    } else if kind == Kind::Melody {
        develop(plan, part, sec, &spec, singer_range, &mut notes);
    }
    // the decorator: ornament on the lead (not the singer: the lyrics want a plain line), colour on chords and pads
    let amount = match kind {
        Kind::Melody if singer_range.is_none() => plan.ornament,
        Kind::Chords | Kind::Pad => plan.color,
        _ => 0.0,
    };
    if amount > 0.05 && !notes.is_empty() {
        let mut rng = compose::Rng::new(spec.seed);
        let harmony: Vec<(f64, f64, u8, usize)> = compose::plan(&spec, &mut rng).iter().map(|s| (s.start, s.len, s.chord.root, s.chord.q)).collect();
        notes = crate::decorate::decorate(&notes, spec.key, &harmony, amount * (0.6 + 0.6 * sec.energy).min(1.0), spec.seed ^ 0xDEC0);
        let top = spec.hi.saturating_add(5);
        for n in notes.iter_mut() {
            while n.key > top && n.key >= 12 {
                n.key -= 12;
            }
        }
    }
    (spec, kind, notes)
}

/// the phrase the composer writes for a part in a section, before the song shapes it
fn written(plan: &SongPlan, part: usize, sec: &Section, singer_range: Option<(u8, u8)>) -> (compose::Spec, Kind, Vec<N>) {
    let g = genre(&plan.genre);
    let role = plan.parts[part].role.as_str();
    let kind = if role == "voice" { Kind::Melody } else { role_kind(role) };
    let tag_seed = sec.tag.bytes().fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(b as u64));
    let seed = plan.seed.wrapping_mul(1_000_003).wrapping_add(part as u64 * 7919).wrapping_add(tag_seed);
    // the second drop switches the bass pattern (a new line over the same drums and lead); a slow movement's drums play half-time
    let si = section_index(plan, sec);
    let nth = plan.sections[..si.unwrap_or(0)].iter().filter(|s| s.tag == sec.tag).count();
    let seed = if kind == Kind::Bass && role != "riser" && g.form == Form::Club && nth >= 1 && is_chorus(&sec.tag) && plan.dials.variety >= 0.15 { seed ^ (nth as u64).wrapping_mul(0xD50F_1A57) } else { seed };
    let halftime = si.and_then(|i| movement_of(plan, i)).map(|m| m.texture == "halftime").unwrap_or(false);
    let mut spec = compose::spec_from_style(g.style, kind, sec.bars, plan.key, seed);
    // the harmonic plan: the section's own key (a bridge somewhere else on the wheel of fifths) and chords, with its cadence
    if let Some(si) = section_index(plan, sec) {
        let (key, chords) = section_chords(plan, si, spec.cpb);
        spec.key = key;
        if !chords.is_empty() {
            spec.prog_text = section_prog(plan, si);
            spec.harmony = chords;
        }
    }
    let energy = (sec.energy + plan.energy).clamp(0.05, 1.0);
    spec.density = (spec.density * (0.55 + 0.65 * energy) * (0.6 + 0.8 * plan.drive)).clamp(0.05, 1.0);
    // a real instrument plays in its own register
    if let Some(inst) = crate::ensemble::instrument(role) {
        if inst.kind != Kind::Drums {
            spec.lo = inst.lo;
            spec.hi = inst.hi;
        }
    }
    if kind == Kind::Drums {
        spec.pattern = if halftime { "halftime".to_string() } else { g.drums.to_string() };
        spec.fills = if is_build_tag(&sec.tag) { 1.0 } else if is_chorus(&sec.tag) && g.form == Form::Club { 0.65 } else { 0.5 };
    }
    if let Some((lo, hi)) = singer_range {
        spec.lo = lo;
        spec.hi = hi;
        // a singable line: fewer, longer notes and a motif that comes back
        spec.density = (0.35 + 0.3 * sec.energy).min(0.65);
        spec.motif = 2;
    }
    if role == "riser" {
        // a rising ladder in the build (it speeds up with the roll), a falling downlifter after the drop hits, else one long note
        return (spec, kind, riser_notes(plan, sec, si));
    }
    if kind == Kind::Arp {
        // the genre's arpeggio: a tempo-locked step (1/16, 1/16 or 1/8 triplets, 1/8) and a pattern that follows the energy
        if let Some(ap) = crate::sounddesign::arp_plan(&plan.genre, energy) {
            spec.rate = ap.step;
            spec.order = ap.order.to_string();
            spec.octaves = ap.octaves;
            if (ap.step * 4.0 - (ap.step * 4.0).round()).abs() > 1e-6 {
                // a 16-step rhythm mask means nothing on a triplet grid
                spec.rhythm.clear();
            }
        }
        // never two arps in the same octave: a second one that plays here sits an octave (two) higher
        if let Some(si) = si {
            let arr = arrange(plan);
            let below = (0..part).filter(|&q| role_kind(&plan.parts[q].role) == Kind::Arp && arr.on.get(q).and_then(|v| v.get(si)).copied().unwrap_or(false)).count();
            if below > 0 {
                let up = (12 * below.min(2)) as u8;
                spec.lo = spec.lo.saturating_add(up).min(96);
                spec.hi = spec.hi.saturating_add(up).min(108);
            }
        }
    }
    let mut notes = compose::generate(&spec);
    // drums in a quiet section: no kick or snare in a break, only the hats and the rim
    if kind == Kind::Drums && sec.energy < 0.45 && !halftime {
        notes.retain(|n| ![compose::K_KICK, compose::K_SNARE, compose::K_CLAP].contains(&n.key));
    }
    for n in notes.iter_mut() {
        n.vel = (n.vel * (0.75 + 0.3 * sec.energy as f32)).clamp(0.05, 1.0);
    }
    notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.key.cmp(&b.key)));
    (spec, kind, notes)
}

/// The theme (from a timeline selection) as this part plays it in this section: a variation chosen by the section (the theme in
/// the first verse, decorated when a section comes back, a step higher in the chorus and as the finale in the last one, inverted in
/// a bridge, augmented in the intro / outro, in diminution in a build), moved into the song's key and the part's register, repeated
/// to fill the section and fitted onto its chords.
fn theme_notes(plan: &SongPlan, part: usize, sec: &Section, spec: &compose::Spec, singer_range: Option<(u8, u8)>) -> Option<Vec<N>> {
    let theme = plan.theme.as_ref()?;
    let si = section_index(plan, sec)?;
    let tag = sec.tag.as_str();
    let k = plan.sections[..si].iter().filter(|s| s.tag == tag).count();
    let count = plan.sections.iter().filter(|s| s.tag == tag).count();
    // the theme in the song's key: its own key is read from its notes
    let (tk, _, _) = theory::detect_key(theme, 0.0, theme.iter().map(|n| n.start + n.len).fold(0.0, f64::max));
    let shift = crate::guide::key_shift(tk, spec.key);
    let moved: Vec<N> = theme.iter().map(|n| N { key: (n.key as i32 + shift).clamp(0, 127) as u8, ..*n }).collect();
    let which = if is_chorus(tag) {
        if count > 1 && k + 1 == count { 6 } else { 2 }
    } else if is_bridge(tag) {
        3
    } else if tag == "intro" || tag == "outro" {
        4
    } else if tag == "build" || tag == "pre" || tag == "prechorus" {
        5
    } else if k > 0 {
        1
    } else {
        0
    };
    let vars = crate::decorate::variations(&moved, spec.key, 7, plan.ornament.max(0.35), plan.seed.wrapping_add(part as u64));
    let (_, v) = vars.get(which).or(vars.first())?;
    if v.is_empty() {
        return None;
    }
    let tl = (v.iter().map(|n| n.start + n.len).fold(0.0, f64::max) / 4.0).ceil().max(1.0) * 4.0;
    // into the part's register: the theme's middle to the range's middle, by octaves
    let mut keys: Vec<i32> = v.iter().map(|n| n.key as i32).collect();
    keys.sort_unstable();
    let mid = (spec.lo as i32 + spec.hi as i32) / 2;
    let oct = ((mid - keys[keys.len() / 2]) as f64 / 12.0).round() as i32 * 12;
    let len = sec.bars as f64 * 4.0;
    let mut out = vec![];
    let mut at = 0.0;
    while at < len - 1e-6 {
        for n in v {
            let st = at + n.start;
            if st < len - 1e-6 {
                out.push(N { start: st, len: n.len.min(len - st), key: (n.key as i32 + oct).clamp(0, 127) as u8, vel: n.vel });
            }
        }
        at += tl;
    }
    let top = spec.hi.saturating_add(if singer_range.is_some() { 2 } else { 5 });
    soft_fit(spec, &mut out, top);
    Some(out)
}

fn is_chorus(tag: &str) -> bool {
    matches!(tag, "chorus" | "hook" | "refrain" | "drop" | "climax")
}
fn is_bridge(tag: &str) -> bool {
    matches!(tag, "bridge" | "break" | "breakdown" | "development")
}

/// which section of the plan this is (by identity, else the first with its tag and length)
fn section_index(plan: &SongPlan, sec: &Section) -> Option<usize> {
    plan.sections.iter().position(|s| std::ptr::eq(s, sec)).or_else(|| plan.sections.iter().position(|s| s.tag == sec.tag && s.bars == sec.bars))
}

// ── shaping the song as a whole ─────────────────────────────────────────────

/// the scale tones of a key over the whole MIDI range
fn scale_keys(key: Key) -> Vec<u8> {
    (0..128u8).filter(|k| key.in_scale(k % 12)).collect()
}
/// a pitch moved by scale steps (a note outside the scale counts from the scale tone below it)
fn dia(key: Key, k: u8, steps: i32) -> u8 {
    let sc = scale_keys(key);
    let i = sc.iter().rposition(|p| *p <= k).unwrap_or(0) as i32;
    sc[(i + steps).clamp(0, sc.len() as i32 - 1) as usize]
}
/// a pitch mirrored around `pivot` in scale steps (the motif upside down)
fn invert(key: Key, k: u8, pivot: u8) -> u8 {
    let sc = scale_keys(key);
    let at = |x: u8| sc.iter().rposition(|p| *p <= x).unwrap_or(0) as i32;
    sc[(2 * at(pivot) - at(k)).clamp(0, sc.len() as i32 - 1) as usize]
}

/// bar `bar` of a phrase replaced by `new` (already placed in that bar); notes ringing into it are cut at the bar line
fn replace_bar(notes: &mut Vec<N>, bar: u32, new: Vec<N>, len: f64) {
    let (a, b) = (bar as f64 * 4.0, bar as f64 * 4.0 + 4.0);
    notes.retain(|n| !(n.start >= a - 1e-6 && n.start < b - 1e-6));
    for n in notes.iter_mut() {
        if n.start < a && n.start + n.len > a {
            n.len = (a - n.start).max(0.05);
        }
    }
    for mut n in new {
        if n.start < len {
            n.len = n.len.min(b.min(len) - n.start).max(0.05);
            notes.push(n);
        }
    }
    notes.sort_by(|x, y| x.start.total_cmp(&y.start).then(x.key.cmp(&y.key)));
}

/// notes on the beat (in the bars given) that are not in the chord of the moment move to its nearest tone; everything is kept in the part's range
fn fit_to_chords(spec: &compose::Spec, notes: &mut [N], top: u8, bars: &[(f64, f64)]) {
    let mut rng = compose::Rng::new(spec.seed);
    let spans = compose::plan(spec, &mut rng);
    for n in notes.iter_mut() {
        let on_beat = (n.start - n.start.round()).abs() < 0.02 && bars.iter().any(|(a, b)| n.start >= *a - 1e-6 && n.start < *b - 1e-6);
        if on_beat {
            if let Some(sp) = spans.iter().find(|s| n.start >= s.start - 1e-6 && n.start < s.start + s.len - 1e-6) {
                if !sp.chord.has(n.key % 12) {
                    let best = (1..=6i32).flat_map(|d| [-d, d]).map(|d| n.key as i32 + d).find(|k| (0..128).contains(k) && sp.chord.has((*k % 12) as u8));
                    if let Some(k) = best {
                        n.key = k as u8;
                    }
                }
            }
        }
        while n.key > top && n.key >= 12 {
            n.key -= 12;
        }
        while n.key < spec.lo && n.key <= 115 {
            n.key += 12;
        }
    }
}

/// A melody kept as it is except where it clashes: a note on a strong beat (1 or 3) that is not a chord tone and sits a semitone
/// or a tritone from one moves to the nearest chord tone; passing and neighbour notes, 9ths and 6ths stay. Kept in the range.
fn soft_fit(spec: &compose::Spec, notes: &mut [N], top: u8) {
    let mut rng = compose::Rng::new(spec.seed);
    let spans = compose::plan(spec, &mut rng);
    for n in notes.iter_mut() {
        let strong = (n.start / 2.0 - (n.start / 2.0).round()).abs() < 0.01;
        if strong {
            if let Some(sp) = spans.iter().find(|s| n.start >= s.start - 1e-6 && n.start < s.start + s.len - 1e-6) {
                let pc = n.key % 12;
                if !sp.chord.has(pc) {
                    let clash = sp.chord.pcs().iter().any(|c| matches!((pc + 12 - c) % 12, 1 | 11 | 6));
                    if clash {
                        if let Some(k) = (1..=6i32).flat_map(|d| [-d, d]).map(|d| n.key as i32 + d).find(|k| (0..128).contains(k) && sp.chord.has((*k % 12) as u8)) {
                            n.key = k as u8;
                        }
                    }
                }
            }
        }
        if !spec.key.accepts(n.key % 12) {
            n.key = dia(spec.key, n.key, 0);
        }
        while n.key > top && n.key >= 12 {
            n.key -= 12;
        }
    }
}

/// Motivic development of a melodic part (an instrument lead, or the singer's line): the song's motif is the first bar of the part in
/// its first main section (verse, theme, else the first drop / chorus). The chorus opens with it a third higher, a bridge turns it upside
/// down, a verse that comes back ends with its own first bar a step higher, and the last time a section comes back it is lifted an octave
/// (when the part's range allows).
fn develop(plan: &SongPlan, part: usize, sec: &Section, spec: &compose::Spec, singer_range: Option<(u8, u8)>, notes: &mut Vec<N>) {
    let Some(si) = section_index(plan, sec) else { return };
    if notes.is_empty() {
        return;
    }
    let tag = sec.tag.as_str();
    let k = plan.sections[..si].iter().filter(|s| s.tag == tag).count();
    let count = plan.sections.iter().filter(|s| s.tag == tag).count();
    let len = sec.bars as f64 * 4.0;
    let key = spec.key;
    let top = spec.hi.saturating_add(if singer_range.is_some() { 2 } else { 5 });
    let home = ["verse", "theme", "drop", "chorus", "climax"].iter().find_map(|t| plan.sections.iter().find(|s| s.tag == *t));
    let motif: Vec<N> = match home {
        Some(h) if h.tag != tag => written(plan, part, h, singer_range).2.into_iter().filter(|n| n.start < 4.0).collect(),
        _ => vec![],
    };
    // the bars that were rewritten (only those are fitted to the chords again)
    let mut fit: Vec<(f64, f64)> = vec![];
    if !motif.is_empty() && is_chorus(tag) {
        replace_bar(notes, 0, motif.iter().map(|n| N { key: dia(key, n.key, 2), ..*n }).collect(), len);
        fit.push((0.0, 4.0));
    } else if !motif.is_empty() && is_bridge(tag) {
        let pivot = motif[0].key;
        replace_bar(notes, 0, motif.iter().map(|n| N { key: invert(key, n.key, pivot), ..*n }).collect(), len);
        fit.push((0.0, 4.0));
    }
    // motivic development across the phrase: the home section states its motif twice (bars 1 and 3), the chorus sequences it a
    // step up (a lifting feeling) or down (a falling one) in its third bar
    let arousal = plan_feels(plan).get(si).map(|f| f.arousal).unwrap_or(0.5);
    if sec.bars >= 4 && home.map(|h| h.tag == tag).unwrap_or(false) {
        let first: Vec<N> = notes.iter().filter(|n| n.start < 4.0).cloned().collect();
        if !first.is_empty() {
            replace_bar(notes, 2, first.iter().map(|n| N { start: n.start + 8.0, ..*n }).collect(), len);
            fit.push((8.0, 12.0));
        }
    } else if sec.bars >= 4 && !motif.is_empty() && is_chorus(tag) {
        let step = if arousal >= 0.5 { 3 } else { 1 };
        replace_bar(notes, 2, motif.iter().map(|n| N { start: n.start + 8.0, key: dia(key, n.key, step), ..*n }).collect(), len);
        fit.push((8.0, 12.0));
    }
    let hi = notes.iter().map(|n| n.key).max().unwrap_or(0);
    let lifted = count > 1 && k > 0 && k + 1 == count && hi as u32 + 12 <= top as u32;
    if lifted {
        for n in notes.iter_mut() {
            n.key += 12;
        }
    } else if k > 0 && !is_chorus(tag) && sec.bars >= 4 {
        let lb = sec.bars - 1;
        let first: Vec<N> = notes.iter().filter(|n| n.start < 4.0).cloned().collect();
        replace_bar(notes, lb, first.iter().map(|n| N { start: n.start + lb as f64 * 4.0, key: dia(key, n.key, 1), ..*n }).collect(), len);
        fit.push((lb as f64 * 4.0, len));
    }
    if lifted || !fit.is_empty() {
        fit_to_chords(spec, notes, top.max(hi.saturating_add(if lifted { 12 } else { 0 })), &fit);
    }
    answer_bars(spec, sec.bars, notes);
}

/// Call and response: the second and fourth bar of every four answer the first and third - their last note lands on a tone of the
/// chord under it and is held until the next phrase begins (a breath of an eighth at most before it).
fn answer_bars(spec: &compose::Spec, bars: u32, notes: &mut [N]) {
    if bars < 2 || notes.is_empty() {
        return;
    }
    let len = bars as f64 * 4.0;
    let mut rng = compose::Rng::new(spec.seed);
    let spans = compose::plan(spec, &mut rng);
    notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.key.cmp(&b.key)));
    for b in (1..bars).step_by(2) {
        let (a0, a1) = (b as f64 * 4.0, (b as f64 * 4.0 + 4.0).min(len));
        let Some(i) = notes.iter().rposition(|n| n.start >= a0 - 1e-6 && n.start < a1 - 1e-6) else { continue };
        let n = notes[i];
        if let Some(sp) = spans.iter().find(|s| n.start >= s.start - 1e-6 && n.start < s.start + s.len - 1e-6) {
            if !sp.chord.has(n.key % 12) {
                if let Some(k) = (1..=4i32).flat_map(|d| [-d, d]).map(|d| n.key as i32 + d).find(|k| (0..128).contains(k) && sp.chord.has((*k % 12) as u8) && spec.key.accepts((*k % 12) as u8)) {
                    notes[i].key = k as u8;
                }
            }
        }
        let next = notes.get(i + 1).map(|x| x.start).unwrap_or(len).min(len);
        notes[i].len = notes[i].len.max((next - n.start - 0.125).min(a1 + 0.5 - n.start)).max(0.05);
    }
}

/// The bass plays with the kick: a note on every kick (the pitch the line had there), a few of its own notes between the kicks as pickups.
/// Basslines that are meant to sit around the kick (rolling, off-beat, walking) are left alone, and so is a section without the drums.
fn lock_to_kick(plan: &SongPlan, sec: &Section, spec: &compose::Spec, notes: &mut Vec<N>) {
    if matches!(spec.pattern.as_str(), "rolling" | "offbeat" | "walking") || notes.is_empty() {
        return;
    }
    let Some(si) = section_index(plan, sec) else { return };
    let Some(di) = plan.parts.iter().position(|p| role_kind(&p.role) == Kind::Drums) else { return };
    if !arrange(plan).on[di][si] {
        return;
    }
    let mut kicks: Vec<f64> = written(plan, di, sec, None).2.iter().filter(|n| n.key == compose::K_KICK).map(|n| n.start).collect();
    kicks.sort_by(f64::total_cmp);
    kicks.dedup_by(|a, b| (*a - *b).abs() < 0.05);
    if kicks.len() < sec.bars as usize {
        return;
    }
    let len = sec.bars as f64 * 4.0;
    notes.sort_by(|a, b| a.start.total_cmp(&b.start));
    let pitch_at = |t: f64| notes.iter().filter(|n| n.start <= t + 0.05).last().or(notes.first()).map(|n| n.key).unwrap_or(spec.lo);
    let mut out: Vec<N> = kicks.iter().map(|k| N { start: *k, len: 1.0, key: pitch_at(*k), vel: 0.92 }).collect();
    let mut rng = compose::Rng::new(spec.seed ^ 0xBA55);
    for n in notes.iter() {
        if kicks.iter().all(|k| (n.start - k).abs() > 0.24) && rng.chance(0.2 + 0.4 * spec.density) {
            out.push(N { vel: n.vel * 0.8, ..*n });
        }
    }
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    let hold = if spec.pattern == "sub" { 8.0 } else { 1.5 };
    for i in 0..out.len() {
        let next = out.get(i + 1).map(|n| n.start).unwrap_or(len);
        out[i].len = ((next - out[i].start) * 0.92).min(hold).max(0.1);
    }
    *notes = out;
}

fn mix64(a: u64, b: u64) -> u64 {
    let mut x = a ^ b.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 33;
    x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    x ^= x >> 33;
    x = x.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    x ^ (x >> 33)
}
fn unit(h: u64) -> f64 {
    (h >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
}

/// One human feel for the whole band: every sixteenth of the song has its own small timing offset that every part shares (so the bass
/// stays on the kick), bar lines stay put, laid-back styles play their off-beats and snare late; chords are rolled low to high; accents
/// lean on the downbeat and the velocity of every hit varies (the hats most).
fn humanize(plan: &SongPlan, part: usize, sec: &Section, kind: Kind, notes: &mut [N]) {
    let h = plan.human.clamp(0.0, 1.0);
    if h <= 0.0 || notes.is_empty() {
        return;
    }
    let at = section_start_bar(plan, sec) as f64 * 4.0;
    let len = sec.bars as f64 * 4.0;
    let laid_back = matches!(genre(&plan.genre).style, "lofi" | "jazz" | "dub");
    notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.key.cmp(&b.key)));
    let (mut order, mut last) = (0usize, f64::NAN);
    for n in notes.iter_mut() {
        let abs = at + n.start;
        let step = (abs * 4.0).round().max(0.0) as i64;
        let on_grid = (abs * 4.0 - step as f64).abs() < 0.03;
        let mut off = unit(mix64(plan.seed, step as u64)) * 0.02 * h;
        if laid_back && step % 2 == 1 {
            off += 0.025 * h;
        }
        if laid_back && kind == Kind::Drums && (n.key == compose::K_SNARE || n.key == compose::K_CLAP) {
            off += 0.012 * h;
        }
        // the bar line is the band's anchor: on the grid
        if step % 16 == 0 {
            off = 0.0;
        }
        if !on_grid {
            off *= 0.5;
        }
        if kind == Kind::Chords {
            if (n.start - last).abs() < 1e-9 {
                order += 1;
            } else {
                order = 0;
                last = n.start;
            }
            off += order as f64 * 0.006 * h;
        }
        n.start = (n.start + off).clamp(0.0, (len - 0.05).max(0.0));
        let acc = if step % 16 == 0 { 0.06 } else if step % 4 == 0 { 0.03 } else if step % 2 == 1 { -0.06 } else { 0.0 };
        let spread = if kind == Kind::Drums && matches!(n.key, compose::K_HAT | compose::K_HAT_OPEN | compose::K_RIDE) { 0.12 } else { 0.07 };
        let rnd = unit(mix64(plan.seed ^ (part as u64 + 1).wrapping_mul(0x51ED), ((step as u64) << 8) | n.key as u64)) * spread;
        n.vel = (n.vel as f64 + (acc + rnd) * h).clamp(0.05, 1.0) as f32;
    }
}

/// Who plays in which section: `on[part][section]`, and what the arrangement did (for the plan's notes).
pub struct Arrangement {
    pub on: Vec<Vec<bool>>,
    pub notes: Vec<String>,
}

/// The arrangement, section by section. It starts from [`plays`] (energy decides) and then: the intro starts with the two quietest
/// parts; a bridge drops the drums or the bass; a verse that comes back gains a part that rested the first time; the first chorus holds
/// a part back that the last chorus brings in, and the last chorus has every part the choruses have; no section is left empty.
pub fn arrange(plan: &SongPlan) -> Arrangement {
    let g = genre(&plan.genre);
    let has_vocal = !plan.singer.is_empty();
    let (np, ns) = (plan.parts.len(), plan.sections.len());
    let kinds: Vec<Kind> = plan.parts.iter().map(|p| role_kind(&p.role)).collect();
    let mut on: Vec<Vec<bool>> = plan.parts.iter().map(|p| plan.sections.iter().map(|s| plays(&p.role, s, g.form, has_vocal)).collect()).collect();
    let mut notes = vec![];
    let mut rng = compose::Rng::new(plan.seed ^ 0xA77A);
    let name = |p: usize| plan.parts[p].role.clone();
    let rank = |k: Kind| match k {
        Kind::Pad => 0,
        Kind::Chords => 1,
        Kind::Arp => 2,
        Kind::Melody => 3,
        Kind::Bass => 4,
        Kind::Drums => 5,
    };
    for si in 0..ns {
        let sec = &plan.sections[si];
        let k = plan.sections[..si].iter().filter(|s| s.tag == sec.tag).count();
        let playing: Vec<usize> = (0..np).filter(|p| on[*p][si]).collect();
        if si == 0 && sec.tag == "intro" {
            let first_of = |k: Kind| (0..np).find(|p| kinds[*p] == k && plan.parts[*p].role != "riser");
            let only = |on: &mut Vec<Vec<bool>>, keep: &[usize]| {
                for p in 0..np {
                    on[p][0] = keep.contains(&p) || (keep.len() < 2 && plan.parts[p].role == "riser" && on[p][0]);
                }
            };
            let chords = first_of(Kind::Chords).or(first_of(Kind::Arp)).or(first_of(Kind::Pad));
            let join = |v: &[usize]| v.iter().map(|p| name(*p)).collect::<Vec<_>>().join(" and ");
            match plan.intro.as_str() {
                "solo" if chords.is_some() => {
                    let c = chords.unwrap_or(0);
                    only(&mut on, &[c]);
                    notes.push(format!("the intro is the {} alone", name(c)));
                }
                "beat" if first_of(Kind::Drums).is_some() => {
                    let mut keep = vec![first_of(Kind::Drums).unwrap_or(0)];
                    if g.form == Form::Club || rng.next() % 2 == 0 {
                        keep.extend(first_of(Kind::Bass));
                    }
                    only(&mut on, &keep);
                    notes.push(format!("the intro starts on the beat: {}", join(&keep)));
                }
                "theme" if first_of(Kind::Melody).is_some() => {
                    let m = first_of(Kind::Melody).unwrap_or(0);
                    let mut keep = vec![m];
                    keep.extend(chords);
                    only(&mut on, &keep);
                    notes.push(format!("the {} plays the hook in the intro", name(m)));
                }
                "cold" if (0..ns).any(|s| is_chorus(&plan.sections[s].tag)) => {
                    let c = (0..ns).find(|s| is_chorus(&plan.sections[*s].tag)).unwrap_or(0);
                    for p in 0..np {
                        on[p][0] = on[p][c] && plan.parts[p].role != "riser";
                    }
                    notes.push("the song opens cold on the chorus band".into());
                }
                "drone" if first_of(Kind::Pad).is_some() || first_of(Kind::Arp).is_some() => {
                    let keep: Vec<usize> = [first_of(Kind::Pad), first_of(Kind::Arp)].into_iter().flatten().collect();
                    only(&mut on, &keep);
                    notes.push(format!("the intro is a drone: {}", join(&keep)));
                }
                _ if playing.len() > 2 => {
                    let mut ps = playing.clone();
                    ps.sort_by_key(|p| rank(kinds[*p]));
                    for p in &ps[2..] {
                        on[*p][si] = false;
                    }
                    notes.push(format!("the intro starts with the {} and the {}", name(ps[0]), name(ps[1])));
                }
                _ => {}
            }
        }
        if is_bridge(&sec.tag) && g.form != Form::Club && playing.len() > 1 {
            let cand: Vec<usize> = playing.iter().copied().filter(|p| matches!(kinds[*p], Kind::Drums | Kind::Bass)).collect();
            if !cand.is_empty() {
                let p = cand[(rng.next() % cand.len() as u64) as usize];
                on[p][si] = false;
                notes.push(format!("the {} {} out the {}", name(p), if name(p).ends_with('s') { "sit" } else { "sits" }, sec.tag));
            }
        }
        if k >= 1 && !is_chorus(&sec.tag) && sec.tag != "outro" {
            let first = plan.sections.iter().position(|s| s.tag == sec.tag).unwrap_or(0);
            let joins: Vec<usize> = (0..np)
                .filter(|p| !on[*p][first] && !on[*p][si] && plan.parts[*p].role != "riser" && !(has_vocal && kinds[*p] == Kind::Melody) && (0..ns).any(|s2| on[*p][s2]))
                .collect();
            if let Some(p) = joins.iter().copied().find(|p| matches!(kinds[*p], Kind::Arp | Kind::Chords | Kind::Pad)).or(joins.first().copied()) {
                on[p][si] = true;
                notes.push(format!("the {} {} in {} {}", name(p), if name(p).ends_with('s') { "join" } else { "joins" }, sec.tag, k + 1));
            }
        }
    }
    let ch: Vec<usize> = (0..ns).filter(|s| is_chorus(&plan.sections[*s].tag)).collect();
    if ch.len() >= 2 {
        let (first, last) = (ch[0], ch[ch.len() - 1]);
        for p in 0..np {
            if plan.parts[p].role != "riser" && ch.iter().any(|s| on[p][*s]) {
                on[p][last] = true;
            }
        }
        let playing: Vec<usize> = (0..np).filter(|p| on[*p][first]).collect();
        if playing.len() > 3 {
            let held = playing.iter().copied().find(|p| kinds[*p] == Kind::Arp).or_else(|| playing.iter().copied().filter(|p| kinds[*p] == Kind::Pad).nth(1));
            if let Some(p) = held {
                on[p][first] = false;
                notes.push(format!("the {} {} for the last {}", name(p), if name(p).ends_with('s') { "wait" } else { "waits" }, plan.sections[last].tag));
            }
        }
    }
    // gradual layering: a budget of parts that follows the energy, and only a few new parts at each section change (a chorus or a
    // drop may bring one more) - the rest wait for the next section
    let lay = plan.dials.layering.clamp(0.0, 1.0);
    if lay > 0.05 && np > 2 {
        let keep_rank = |k: Kind, e: f64| -> u8 {
            // what matters most in a quiet section and in a loud one
            match k {
                Kind::Melody => 0,
                Kind::Bass => if e >= 0.5 { 1 } else { 3 },
                Kind::Drums => if e >= 0.5 { 2 } else { 5 },
                Kind::Chords => if e >= 0.5 { 3 } else { 1 },
                Kind::Pad => if e >= 0.5 { 4 } else { 2 },
                Kind::Arp => 6,
            }
        };
        let mut prev: Vec<usize> = vec![];
        let mut held: Vec<String> = vec![];
        for si in 0..ns {
            let sec = &plan.sections[si];
            let e = sec.energy;
            let budget = ((np as f64 * (0.3 + 0.7 * e)) * (1.0 - 0.3 * lay)).round().clamp(1.0, np as f64) as usize;
            let mut playing: Vec<usize> = (0..np).filter(|p| on[*p][si]).collect();
            playing.sort_by_key(|p| (keep_rank(kinds[*p], e), *p));
            // the newcomers allowed now
            let allowed_new = if is_chorus(&sec.tag) { 3 } else { 2 } + if lay < 0.5 { 1 } else { 0 };
            let mut kept: Vec<usize> = vec![];
            let mut new_n = 0;
            for p in playing {
                let is_new = !prev.contains(&p) && si > 0;
                if kept.len() >= budget || (is_new && new_n >= allowed_new && plan.parts[p].role != "riser") {
                    on[p][si] = false;
                    if is_new {
                        held.push(format!("{} waits for {}", name(p), plan.sections.get(si + 1).map(|s| s.tag.as_str()).unwrap_or("the end")));
                    }
                    continue;
                }
                if is_new {
                    new_n += 1;
                }
                kept.push(p);
            }
            prev = kept;
        }
        if !held.is_empty() {
            held.dedup();
            notes.push(format!("layering: {}", held.into_iter().take(4).collect::<Vec<_>>().join(", ")));
        }
    }
    // movements: a pad-led section lays the drums, bass and lead out and lets the pad and chords carry it; a half-time one keeps a slow
    // groove (drums and bass) under the pads; a build into a drop always has its drums for the roll
    let mv = movements(plan);
    if mv.len() > 1 {
        let pad = (0..np).find(|p| kinds[*p] == Kind::Pad && plan.parts[*p].role != "riser").or_else(|| (0..np).find(|p| kinds[*p] == Kind::Chords));
        for m in mv.iter().filter(|m| matches!(m.texture, "pads" | "halftime")) {
            for si in m.first..=m.last {
                for p in 0..np {
                    let lead = kinds[p] == Kind::Melody;
                    let groove = matches!(kinds[p], Kind::Drums | Kind::Bass);
                    if plan.parts[p].role == "riser" || lead || (groove && m.texture == "pads") {
                        on[p][si] = false;
                    } else if groove && m.texture == "halftime" {
                        on[p][si] = true;
                    }
                }
                if let Some(pd) = pad {
                    on[pd][si] = true;
                }
            }
            let tags: Vec<&str> = (m.first..=m.last).map(|i| plan.sections[i].tag.as_str()).collect();
            notes.push(if m.texture == "pads" {
                format!("{} ({}) is pad-led: drums, bass and lead rest, the pads and chords carry it", m.name, tags.join(", "))
            } else {
                format!("{} ({}) is a half-time groove under the pads", m.name, tags.join(", "))
            });
        }
    }
    if let Some(d) = (0..np).find(|p| kinds[*p] == Kind::Drums) {
        let mut drummed = false;
        for si in 0..ns {
            if leads_into_drop(plan, si) && !on[d][si] {
                on[d][si] = true;
                drummed = true;
            }
        }
        if drummed {
            notes.push("the drums come in for the snare roll of a build".into());
        }
    }
    for si in 0..ns {
        if np > 0 && !(0..np).any(|p| on[p][si]) {
            let p = (0..np).find(|p| matches!(kinds[*p], Kind::Pad | Kind::Chords)).unwrap_or(0);
            on[p][si] = true;
        }
    }
    Arrangement { on, notes }
}

// ── the chords a language model proposes, checked against the key ───────────

/// a chord written with letters ("F#m", "Bbmaj7", "C") as (root pitch class, quality)
fn parse_letter_chord(tok: &str) -> Option<(u8, usize)> {
    let t = tok.trim().trim_matches(|c: char| c == ',' || c == '.' || c == ';');
    let mut ch = t.chars();
    let base: i32 = match ch.next()? {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest = ch.as_str();
    let (acc, rest) = if let Some(r) = rest.strip_prefix('#').or_else(|| rest.strip_prefix('♯')) {
        (1, r)
    } else if let Some(r) = rest.strip_prefix('b').or_else(|| rest.strip_prefix('♭')) {
        (-1, r)
    } else {
        (0, rest)
    };
    let rest = rest.split('/').next().unwrap_or("");
    let q = match rest {
        "min" | "-" => "m",
        "min7" | "-7" => "m7",
        "M7" | "Maj7" => "maj7",
        "°" | "o" => "dim",
        "ø" | "ø7" => "m7b5",
        "sus" => "sus4",
        "+" => "aug",
        other => other,
    };
    Some(((base + acc).rem_euclid(12) as u8, theory::qual_index(q)?))
}

/// the chord the key builds on a scale degree (a triad, or a seventh chord)
fn diatonic_chord(key: Key, deg: usize, seventh: bool) -> (u8, usize) {
    let s = key.scale();
    let r = s[deg % 7];
    let iv = |k: usize| (s[(deg + k) % 7] + 12 - r) % 12;
    let name = match (iv(2), iv(4), seventh, iv(6)) {
        (4, 7, false, _) => "",
        (3, 7, false, _) => "m",
        (3, 6, false, _) => "dim",
        (4, 7, true, 11) => "maj7",
        (4, 7, true, _) => "7",
        (3, 7, true, _) => "m7",
        (3, 6, true, _) => "m7b5",
        _ => "",
    };
    (r, theory::qual_index(name).unwrap_or(0))
}

fn roman_of(key: Key, c: (u8, usize)) -> String {
    theory::roman(&theory::Chord { start: 0.0, len: 4.0, root: c.0, q: c.1, bass: c.0, implied: false }, key)
}

/// A progression a language model proposed, checked against the key. Chords it cannot read are dropped (Roman numerals or letter names
/// are read); a chord outside the key stays when music borrows it that way (bVII, bVI, bIII and iv in a major key; the Neapolitan bII
/// and a major IV in a minor key; a secondary dominant that falls a fifth onto a chord of the key); anything else becomes the chord the
/// key has on the nearest scale degree. A loop that never touches the tonic (or its relative) starts on the tonic. More than 8 chords are
/// cut to 8; fewer than 2 give "" (not a progression). Returns the checked progression as Roman numerals and what was changed.
pub fn check_prog(text: &str, key: Key) -> (String, Vec<String>) {
    let mut why = vec![];
    let mut chords: Vec<(u8, usize)> = vec![];
    for t in text.split(|c: char| c.is_whitespace() || c == '-' || c == '–' || c == '>' || c == '|' || c == ',').filter(|t| !t.is_empty()) {
        match compose::parse_roman(t, key).or_else(|| parse_letter_chord(t)) {
            Some(c) => chords.push(c),
            None => why.push(format!("dropped \"{t}\"")),
        }
    }
    if chords.len() > 8 {
        chords.truncate(8);
        why.push("kept the first 8 chords".into());
    }
    if chords.len() < 2 {
        return (String::new(), why);
    }
    let n = chords.len();
    let mut out = chords.clone();
    for i in 0..n {
        let (root, q) = chords[i];
        let qn = theory::QUAL[q].0;
        if theory::QUAL[q].1.iter().all(|x| key.accepts((root + x) % 12)) {
            continue;
        }
        let rel = (root + 12 - key.tonic) % 12;
        let major_like = matches!(qn, "" | "7" | "maj7" | "add9" | "6" | "5" | "sus2" | "sus4");
        let minor_like = matches!(qn, "m" | "m7");
        let borrowed = if key.minor { (rel == 1 || rel == 5) && major_like } else { (matches!(rel, 3 | 8 | 10) && major_like) || (rel == 5 && minor_like) };
        let next = chords[(i + 1) % n];
        let secondary = matches!(qn, "" | "7") && (root + 5) % 12 == next.0 && theory::QUAL[next.1].1.iter().all(|x| key.accepts((next.0 + x) % 12));
        if borrowed || secondary {
            continue;
        }
        let s = key.scale();
        let deg = (0..7).min_by_key(|d| {
            let x = (s[*d] as i32 - root as i32).rem_euclid(12);
            x.min(12 - x)
        });
        let fixed = diatonic_chord(key, deg.unwrap_or(0), theory::QUAL[q].1.len() >= 4);
        why.push(format!("{} is not in {}: {}", roman_of(key, (root, q)), key.name(), roman_of(key, fixed)));
        out[i] = fixed;
    }
    let home = |c: &(u8, usize)| {
        let rel = (c.0 + 12 - key.tonic) % 12;
        rel == 0 || rel == if key.minor { 3 } else { 9 }
    };
    if !out.iter().any(home) {
        let t = diatonic_chord(key, 0, theory::QUAL[out[0].1].1.len() >= 4);
        why.push(format!("starts on {} so the loop has a home", roman_of(key, t)));
        out[0] = t;
    }
    (out.iter().map(|c| roman_of(key, *c)).collect::<Vec<_>>().join(" "), why)
}


/// where in the song a section starts, in bars
fn section_start_bar(plan: &SongPlan, sec: &Section) -> u32 {
    let mut at = 0;
    for s in &plan.sections {
        if std::ptr::eq(s, sec) {
            return at;
        }
        at += s.bars;
    }
    // a copy of a section of the plan: the first one with its tag and length
    let mut at = 0;
    for s in &plan.sections {
        if s.tag == sec.tag && s.bars == sec.bars {
            return at;
        }
        at += s.bars;
    }
    0
}

/// The guide MIDI's notes in place of the generated ones, bar by bar (see guide.rs): the file's counterpart of this part (the n-th busiest
/// track of the same kind) in the song's key and the part's register; which bars come from the file is decided by the conform amount.
fn blend_guide(plan: &SongPlan, part: usize, sec: &Section, kind: Kind, spec: &compose::Spec, notes: &mut Vec<N>) {
    let (Some(g), c) = (plan.guide.as_ref(), plan.conform) else { return };
    if c <= 0.0 || g.bars == 0 {
        return;
    }
    let occurrence = plan.parts[..part].iter().filter(|p| (if p.role == "voice" { Kind::Melody } else { role_kind(&p.role) }) == kind).count();
    let Some(gt) = g.nth_of_kind(kind, occurrence) else { return };
    let shift = if kind == Kind::Drums { 0 } else { crate::guide::key_shift(g.key, plan.key) };
    // octave: the file's melody may sit anywhere; its median goes to the middle of this part's range
    let mut keys: Vec<i32> = gt.notes.iter().map(|n| n.key as i32 + shift).collect();
    keys.sort_unstable();
    let octave = if kind == Kind::Drums || keys.is_empty() {
        0
    } else {
        let mid = (spec.lo as i32 + spec.hi as i32) / 2;
        ((mid - keys[keys.len() / 2]) as f64 / 12.0).round() as i32 * 12
    };
    let start_bar = section_start_bar(plan, sec);
    for bi in 0..sec.bars {
        let gb = start_bar + bi;
        if !crate::guide::take_bar(plan.seed, part, gb, c) {
            continue;
        }
        let src = (gb % g.bars) as f64 * 4.0;
        notes.retain(|n| !(n.start >= bi as f64 * 4.0 && n.start < bi as f64 * 4.0 + 4.0));
        for gn in gt.notes.iter().filter(|n| n.start >= src && n.start < src + 4.0) {
            let key = (gn.key as i32 + shift + octave).clamp(0, 127) as u8;
            notes.push(N { start: gn.start - src + bi as f64 * 4.0, len: gn.len.min(sec.bars as f64 * 4.0 - (gn.start - src + bi as f64 * 4.0)).max(0.05), key, vel: gn.vel });
        }
    }
    notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.key.cmp(&b.key)));
}

/// The pitch range of a Ling Voice singer (MIDI), for the sung line.
pub fn singer_range(singer: &str) -> (u8, u8) {
    match singer {
        "kai" | "ren" | "robot" => (48, 67),
        "choir" => (55, 74),
        _ => (60, 79),
    }
}

/// The lyrics for each section: "[Verse] ... [Chorus] ..." labels go to the matching sections (every chorus sings the chorus); without
/// labels the stanzas (blank lines, or " / / ") are dealt out: verse, chorus, verse, chorus ...
pub fn lyrics_for_sections(lyrics: &str, sections: &[Section]) -> Vec<String> {
    let text = lyrics.replace(" / ", "\n").replace("\r", "");
    let mut labeled: Vec<(String, String)> = vec![];
    let mut cur_tag = String::new();
    let mut cur = String::new();
    let mut any_label = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            if !cur.trim().is_empty() {
                labeled.push((cur_tag.clone(), cur.trim().to_string()));
            }
            cur.clear();
            let w = t.trim_matches(|c| c == '[' || c == ']').to_lowercase();
            let word = w.split(|c: char| !c.is_alphabetic() && c != '-').next().unwrap_or("").to_string();
            cur_tag = canon_tag(&word).to_string();
            any_label = true;
        } else {
            cur.push_str(t);
            cur.push('\n');
        }
    }
    if !cur.trim().is_empty() {
        labeled.push((cur_tag, cur.trim().to_string()));
    }
    let sung = |s: &Section| !matches!(s.tag.as_str(), "intro" | "outro" | "break" | "build");
    let mut out = vec![String::new(); sections.len()];
    if any_label {
        // verses are used in order; a chorus label is sung by every chorus
        let mut used: Vec<bool> = vec![false; labeled.len()];
        for (i, s) in sections.iter().enumerate() {
            if !sung(s) {
                continue;
            }
            let tag = if s.tag == "drop" { "chorus" } else { s.tag.as_str() };
            if let Some(j) = (0..labeled.len()).find(|j| !used[*j] && labeled[*j].0 == tag) {
                out[i] = labeled[j].1.clone();
                if tag != "chorus" {
                    used[j] = true;
                }
            } else if let Some(j) = (0..labeled.len()).find(|j| labeled[*j].0 == tag) {
                out[i] = labeled[j].1.clone();
            }
        }
        return out;
    }
    // stanzas
    let stanzas: Vec<String> = text.split("\n\n").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    let stanzas = if stanzas.len() == 1 {
        // one block: the first half is the verse, the second the chorus
        let lines: Vec<&str> = stanzas[0].lines().collect();
        let h = (lines.len() + 1) / 2;
        vec![lines[..h].join("\n"), lines[h..].join("\n")]
    } else {
        stanzas
    };
    let (mut v, mut chorus): (usize, Option<String>) = (0, None);
    for (i, s) in sections.iter().enumerate() {
        if !sung(s) || stanzas.is_empty() {
            continue;
        }
        if s.tag == "chorus" || s.tag == "drop" || s.tag == "climax" {
            if chorus.is_none() {
                chorus = Some(stanzas.get(v).cloned().unwrap_or_default());
                v += 1;
            }
            out[i] = chorus.clone().unwrap_or_default();
        } else {
            out[i] = stanzas.get(v % stanzas.len()).cloned().unwrap_or_default();
            v += 1;
        }
    }
    out
}

// ── the reference recording ─────────────────────────────────────────────────

fn rms(x: &[f32]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
}

/// the diatonic triad that fits a chroma best: (scale degree 0..6, score)
pub(crate) fn best_triad(hist: &[f64; 12], key: Key) -> usize {
    let scale = key.scale();
    let mut best = (0usize, f64::MIN);
    for d in 0..7 {
        // Key::scale() is already absolute pitch classes
        let pcs = [scale[d], scale[(d + 2) % 7], scale[(d + 4) % 7]];
        let inside: f64 = hist[pcs[0] as usize] * 1.3 + hist[pcs[1] as usize] + hist[pcs[2] as usize];
        let outside: f64 = (0..12).filter(|p| !pcs.contains(&(*p as u8))).map(|p| hist[p]).sum::<f64>() * 0.35;
        // the tonic and the fifth chord win ties (a little prior, as in real songs)
        let prior = match d {
            0 => 0.03,
            4 | 5 => 0.015,
            _ => 0.0,
        };
        let s = inside - outside + prior;
        if s > best.1 {
            best = (d, s);
        }
    }
    best.0
}

pub(crate) fn roman(d: usize, minor: bool) -> &'static str {
    if minor { ["i", "ii°", "III", "iv", "v", "VI", "VII"][d] } else { ["I", "ii", "iii", "IV", "V", "vi", "vii°"][d] }
}

/// Measure a recording (interleaved stereo at `sr`): tempo, key, the chord loop of its loudest stretch, sections from the energy per bar,
/// tone balance, how chordal it is, and the genre it sounds like. At most the first 4 minutes are read.
pub fn analyze_reference(stereo: &[f32], sr: u32) -> RefInfo {
    let srf = sr as f32;
    let max = (240.0 * srf) as usize * 2;
    let st = &stereo[..stereo.len().min(max)];
    let mono: Vec<f32> = st.chunks_exact(2).map(|f| (f[0] + f[1]) * 0.5).collect();
    let mut r = RefInfo { seconds: mono.len() as f64 / sr as f64, ..RefInfo::default() };
    if mono.len() < sr as usize * 4 {
        return r;
    }
    // tempo, folded into the range songs are written in
    let mut bpm = ling_music::analysis::bpm(&mono, sr) as f64;
    if bpm > 0.0 {
        while bpm < 70.0 {
            bpm *= 2.0;
        }
        while bpm > 180.0 {
            bpm /= 2.0;
        }
    }
    r.bpm = if bpm > 0.0 { bpm.round() } else { 120.0 };
    // key and polyphony
    let ch = crate::keydet::chroma(st, srf);
    r.poly = ch.poly;
    if ch.hist.iter().sum::<f64>() > 0.0 {
        let (k, conf, _) = theory::detect_key_hist(&ch.hist);
        r.key = Some(k);
        r.key_conf = conf;
    }
    // energy per bar -> sections
    let bar = (4.0 * 60.0 / r.bpm * sr as f64) as usize;
    let nb = mono.len() / bar.max(1);
    let e: Vec<f64> = (0..nb).map(|b| rms(&mono[b * bar..((b + 1) * bar).min(mono.len())])).collect();
    let top = e.iter().cloned().fold(1e-9, f64::max);
    let en: Vec<f64> = e.iter().map(|x| (x / top).clamp(0.0, 1.0)).collect();
    // 4-bar blocks, merged while they are alike
    let mut blocks: Vec<(usize, usize, f64)> = vec![];
    let mut b = 0;
    while b < nb {
        let l = 4.min(nb - b);
        let m = en[b..b + l].iter().sum::<f64>() / l as f64;
        match blocks.last_mut() {
            Some(last) if (last.2 - m).abs() < 0.16 && last.1 < 16 => {
                last.2 = (last.2 * last.1 as f64 + m * l as f64) / (last.1 + l) as f64;
                last.1 += l;
            }
            _ => blocks.push((b, l, m)),
        }
        b += l;
    }
    // short blocks join their neighbour
    let mut merged: Vec<(usize, usize, f64)> = vec![];
    for blk in blocks {
        match merged.last_mut() {
            Some(last) if blk.1 < 8 || last.1 < 4 => {
                last.2 = (last.2 * last.1 as f64 + blk.2 * blk.1 as f64) / (last.1 + blk.1) as f64;
                last.1 += blk.1;
            }
            _ => merged.push(blk),
        }
    }
    let hi = merged.iter().map(|x| x.2).fold(0.0, f64::max).max(1e-6);
    let n = merged.len();
    let mut seen_high = false;
    for (i, (_, bars, m)) in merged.iter().enumerate() {
        let rel = m / hi;
        let tag = if i == 0 && rel < 0.7 {
            "intro"
        } else if i + 1 == n && rel < 0.7 {
            "outro"
        } else if rel >= 0.82 {
            seen_high = true;
            "chorus"
        } else if rel < 0.55 && seen_high {
            "break"
        } else {
            "verse"
        };
        r.sections.push(Section { tag: tag.into(), bars: round4(*bars as f64).min(24), energy: (0.25 + 0.75 * rel).min(1.0) });
    }
    // a long run of loud sections trades two phrases (chorus / drop) instead of repeating one
    let mut run = 0;
    for i in 0..r.sections.len() {
        run = if i > 0 && r.sections[i].tag == r.sections[i - 1].tag.replace("drop", "chorus") && r.sections[i].tag == "chorus" { run + 1 } else { 0 };
        if run % 2 == 1 {
            r.sections[i].tag = "drop".into();
        }
    }
    // chords: one triad per bar over the loudest stretch, then the loop that repeats (4 or 8 bars)
    if let Some(k) = r.key {
        let start = merged.iter().max_by(|a, b| a.2.total_cmp(&b.2)).map(|x| x.0).unwrap_or(0);
        let bars8: Vec<usize> = (0..8)
            .filter_map(|j| {
                let b = start + j;
                if b >= nb {
                    return None;
                }
                let seg = &st[b * bar * 2..((b + 1) * bar * 2).min(st.len())];
                let c = crate::keydet::chroma(seg, srf);
                if c.hist.iter().sum::<f64>() <= 0.0 {
                    return None;
                }
                Some(best_triad(&c.hist, k))
            })
            .collect();
        let loop_len = if bars8.len() == 8 && bars8[..4] == bars8[4..] { 4 } else { bars8.len().min(8) };
        let mut prog: Vec<&str> = bars8[..loop_len].iter().map(|d| roman(*d, k.minor)).collect();
        // a chord held for two bars reads as one change, and a loop that comes round again is written once ("v i v i v" -> "v i")
        prog.dedup();
        if let Some(p) = (2..prog.len()).find(|p| (0..prog.len()).all(|i| prog[i] == prog[i % p])) {
            prog.truncate(p);
        }
        if prog.len() >= 2 {
            r.prog = prog.join(" ");
        }
    }
    // tone balance
    let s = crate::mixai::analyze(st, srf);
    let lin: Vec<f64> = s.bands.iter().map(|d| 10f64.powf(*d as f64 / 10.0)).collect();
    let tot = lin.iter().sum::<f64>().max(1e-12);
    r.bass = (lin[0] + lin[1]) / tot;
    r.bright = (lin[4] + lin[5]) / tot;
    // the genre it sounds like: tempo, low end, brightness, chords
    let b = r.bpm;
    r.genre = if (160.0..=180.0).contains(&b) {
        "drum and bass"
    } else if (136.0..=145.0).contains(&b) && r.bass > 0.55 {
        "dubstep"
    } else if (134.0..=142.0).contains(&b) {
        "trance"
    } else if (126.0..=134.0).contains(&b) {
        "techno"
    } else if (118.0..=126.0).contains(&b) && r.bass > 0.4 {
        "house"
    } else if b < 95.0 && r.bright < 0.12 && r.poly > 2.5 {
        "lofi"
    } else if b < 80.0 && r.poly > 2.5 {
        "ambient"
    } else if r.bright > 0.25 && r.bass < 0.35 {
        "rock"
    } else {
        "pop"
    }
    .to_string();
    r
}

/// a sentence about the reference, for the dialog
pub fn describe_reference(r: &RefInfo) -> String {
    if r.seconds < 4.0 {
        return "the reference is too short to read".into();
    }
    let key = r.key.map(|k| k.name()).unwrap_or_else(|| "no clear key".into());
    let st: Vec<String> = r.sections.iter().map(|s| format!("{} {}", s.tag, s.bars)).collect();
    format!(
        "{:.0} bpm, {} ({:.0}% sure), chords {}, sounds like {}, {} sections: {} - {} low end, {} top, {}",
        r.bpm,
        key,
        r.key_conf * 100.0,
        if r.prog.is_empty() { "unclear".to_string() } else { r.prog.clone() },
        r.genre,
        r.sections.len(),
        st.join(", "),
        if r.bass > 0.5 { "heavy" } else if r.bass > 0.3 { "solid" } else { "light" },
        if r.bright > 0.25 { "bright" } else if r.bright > 0.12 { "balanced" } else { "dark" },
        if r.poly > 2.5 { "chordal" } else { "sparse" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prompt_becomes_a_plan() {
        let p = plan("dark uplifting trance at 140 bpm in F# minor over i VI III VII with a female vocal", "", &[], None, 1.0, 7);
        assert_eq!(p.genre, "trance");
        assert_eq!(p.bpm, 140.0);
        assert_eq!(p.key.tonic, 6);
        assert!(p.key.minor);
        assert_eq!(p.prog.to_lowercase(), "i vi iii vii");
        assert_eq!(p.singer, "aria");
        assert!(p.sections.iter().any(|s| s.tag == "drop"));
        assert!(p.parts.iter().any(|x| x.role == "drums"));
        // the line round-trips, and an edited line wins
        let line = p.to_line();
        let q = SongPlan::from_line(&line.replace("bpm=140", "bpm=132").replace("voice=aria", "voice=none"), &p);
        assert_eq!(q.bpm, 132.0);
        assert_eq!(q.singer, "");
        assert_eq!(q.sections, p.sections);
        let s = plan("lofi hip hop, structure: intro 4, verse 8, chorus 8, outro 4", "", &[], None, 1.0, 1);
        assert_eq!(s.genre, "hip hop");
        assert_eq!(s.sections.iter().map(|x| x.bars).collect::<Vec<_>>(), vec![4, 8, 8, 4]);
    }

    #[test]
    fn notes_are_in_key_and_choruses_repeat() {
        let mut p = plan("pop song in C major", "", &[], None, 1.0, 3);
        // the diatonic family (borrowed and modal chords leave the scale on purpose)
        p.dials.style = "loop".into();
        let ch: Vec<&Section> = p.sections.iter().filter(|s| s.tag == "chorus").collect();
        assert!(ch.len() >= 2);
        for (i, part) in p.parts.iter().enumerate() {
            if role_kind(&part.role) == Kind::Drums {
                continue;
            }
            let a = section_notes(&p, i, ch[0], None);
            let b = section_notes(&p, i, ch[1], None);
            assert_eq!(a.len(), b.len(), "{} repeats its chorus", part.role);
            assert!(a.iter().all(|n| p.key.in_scale(n.key % 12) || part.role == "riser"), "{} stays in C major", part.role);
        }
    }

    #[test]
    fn the_models_chords_are_checked_against_the_key() {
        let c = Key::new(0, false);
        assert_eq!(check_prog("I V vi IV", c), ("I V vi IV".to_string(), vec![]));
        // borrowed chords and a secondary dominant are music, not mistakes
        assert!(check_prog("I bVII IV I", c).1.is_empty());
        assert!(check_prog("I II V I", c).1.is_empty(), "II falls a fifth onto V");
        // a chord nothing explains is replaced by the key's own chord on the nearest degree
        let (p, why) = check_prog("I #iv V I", c);
        assert_eq!(p, "I IV V I", "{why:?}");
        assert_eq!(why.len(), 1);
        // letter names are read too; words are not chords
        assert_eq!(check_prog("C G Am F", c).0, "I V vi IV");
        assert_eq!(check_prog("banana I V", c).0, "I V");
        assert_eq!(check_prog("banana", c).0, "");
        // a loop without a home gets one
        let (p, why) = check_prog("ii V", c);
        assert!(p.starts_with('I') && why.iter().any(|w| w.contains("home")), "{p} {why:?}");
        // in a plan: the model's chords are used once checked, and the plan says so
        let model = vec![("progression".to_string(), "i bII v VI".to_string()), ("chorus_progression".to_string(), "VI VII i i".to_string())];
        let pl = plan("dark synthwave in A minor", "", &model, None, 1.0, 2);
        assert_eq!(pl.prog, "i bII v VI");
        assert_eq!(pl.chorus_prog, "VI VII i i");
        assert!(pl.notes.iter().any(|n| n.contains("checked")), "{:?}", pl.notes);
        assert!(pl.to_line().contains("chorus=VI VII i i"));
        let back = SongPlan::from_line(&pl.to_line(), &pl);
        assert_eq!((back.chorus_prog.as_str(), back.human), ("VI VII i i", pl.human));
    }

    #[test]
    fn the_arrangement_varies_by_section() {
        let p = plan("pop song with synth lead and arp in C major", "", &[], None, 1.0, 5);
        let a = arrange(&p);
        let idx = |tag: &str, k: usize| p.sections.iter().enumerate().filter(|(_, s)| s.tag == tag).nth(k).map(|(i, _)| i).unwrap();
        let count = |si: usize| (0..p.parts.len()).filter(|pi| a.on[*pi][si]).count();
        assert!(count(idx("intro", 0)) <= 2, "a small intro");
        // the bridge drops the drums or the bass
        let br = idx("bridge", 0);
        let rhythm: Vec<usize> = (0..p.parts.len()).filter(|pi| matches!(role_kind(&p.parts[*pi].role), Kind::Drums | Kind::Bass)).collect();
        assert!(rhythm.iter().any(|pi| !a.on[*pi][br]), "{:?}", a.notes);
        // the last chorus is the biggest; the second verse is at least as big as the first
        let chs: Vec<usize> = p.sections.iter().enumerate().filter(|(_, s)| s.tag == "chorus").map(|(i, _)| i).collect();
        assert!(count(*chs.last().unwrap()) >= count(chs[0]));
        assert!(count(idx("verse", 1)) >= count(idx("verse", 0)), "the second verse grows");
        assert!(p.notes.iter().any(|n| n.contains("bridge")), "{:?}", p.notes);
    }

    #[test]
    fn the_bass_plays_with_the_kick_and_the_band_breathes_together() {
        let mut p = plan("pop song in C major", "", &[], None, 1.0, 9);
        let di = p.parts.iter().position(|x| x.role == "drums").unwrap();
        let bi = p.parts.iter().position(|x| role_kind(&x.role) == Kind::Bass).unwrap();
        let arr = arrange(&p);
        let v = (0..p.sections.len()).find(|si| arr.on[di][*si] && arr.on[bi][*si] && p.sections[*si].tag == "verse").or_else(|| (0..p.sections.len()).find(|si| arr.on[di][*si] && arr.on[bi][*si])).unwrap();
        let bars = p.sections[v].bars;
        for human in [0.0, 0.8] {
            p.human = human;
            let kicks: Vec<f64> = section_notes(&p, di, &p.sections[v], None).iter().filter(|n| n.key == compose::K_KICK).map(|n| n.start).collect();
            let bass = section_notes(&p, bi, &p.sections[v], None);
            assert!(kicks.len() >= bars as usize);
            for k in &kicks {
                assert!(bass.iter().any(|b| (b.start - k).abs() < 1e-9), "human {human}: no bass on the kick at {k}");
            }
            let off_grid = bass.iter().filter(|n| (n.start * 4.0 - (n.start * 4.0).round()).abs() > 1e-9).count();
            if human == 0.0 {
                assert_eq!(off_grid, 0);
            } else {
                assert!(off_grid > 0, "humanized timing");
                assert!(bass.iter().any(|n| n.start == 0.0), "the bar line stays put");
            }
        }
    }

    #[test]
    fn the_motif_develops_across_sections() {
        let mut p = plan("pop song with a synth lead in C major", "", &[], None, 1.0, 4);
        p.human = 0.0;
        p.movements = "1".into(); // the final lift would move the last chorus up a key by itself
        let li = p.parts.iter().position(|x| x.role == "synth lead").unwrap();
        let at = |tag: &str, k: usize| p.sections.iter().filter(|s| s.tag == tag).nth(k).unwrap();
        let v1 = section_notes(&p, li, at("verse", 0), None);
        let v2 = section_notes(&p, li, at("verse", 1), None);
        let bar = |n: &[N], b: f64| n.iter().filter(|x| x.start >= b * 4.0 && x.start < b * 4.0 + 4.0).map(|x| x.key).collect::<Vec<_>>();
        assert_eq!(bar(&v1, 0.0), bar(&v2, 0.0), "the second verse starts like the first");
        let last = at("verse", 0).bars as f64 - 1.0;
        assert_ne!(bar(&v1, last), bar(&v2, last), "and ends differently");
        // the chorus quotes the verse's motif; the last chorus is lifted
        let c1 = section_notes(&p, li, at("chorus", 0), None);
        let c3 = section_notes(&p, li, at("chorus", 2), None);
        // (the last bar hands over to whatever follows - a verse, the outro - so it is left out of the comparison)
        let body = at("chorus", 0).bars as f64 * 4.0 - 4.0;
        let mean = |n: &[N]| n.iter().filter(|x| x.start < body).map(|x| x.key as f64).sum::<f64>() / n.iter().filter(|x| x.start < body).count().max(1) as f64;
        assert!(mean(&c3) >= mean(&c1), "{} {}", mean(&c3), mean(&c1));
        assert!(v1.iter().chain(&c1).all(|n| p.key.in_scale(n.key % 12)));
    }

    #[test]
    fn the_length_follows_a_duration_the_midi_or_the_lyrics() {
        assert_eq!(Fit::parse("duration:3:30"), Fit::Duration(210.0));
        assert_eq!(Fit::parse("2.5"), Fit::Duration(150.0));
        assert_eq!(Fit::parse("lyrics"), Fit::Lyrics);
        assert_eq!(Fit::parse("nonsense"), Fit::Auto);
        let secs = |p: &SongPlan| p.bars() as f64 * 240.0 / p.bpm;
        // a duration: within a 4-bar step of it
        for want in [90.0, 210.0, 300.0] {
            let p = plan_fit("pop song at 120 bpm", "", &[], None, 1.0, 1, None, 0.0, &Fit::Duration(want));
            assert!((secs(&p) - want).abs() <= 4.01, "asked {want}, got {}", secs(&p));
        }
        // lyrics: every sung section has room for them (about 2.2 syllables a second), however short the form
        let lyr = "[Verse]\nwalking through the city lights tonight and every night\nall the faces passing by they never look me in the eye\nI keep on moving through the rain\nand I keep on calling out your name\n[Chorus]\nhold on hold on to me\nwe are the light we are the sea";
        let p = plan_fit("short pop song at 120 bpm with a vocal", lyr, &[], None, 0.5, 1, None, 0.0, &Fit::Auto);
        let words = lyrics_for_sections(lyr, &p.sections);
        for (s, w) in p.sections.iter().zip(&words) {
            let (n, _) = syllable_count(w);
            if n > 0 {
                let sec = s.bars as f64 * 2.0;
                assert!(n as f64 / sec <= SYL_PER_SEC + 0.01, "{}: {n} syllables in {sec} s", s.tag);
            }
        }
        // a duration too short for the lyrics: the lyrics win, the plan says so
        let p = plan_fit("pop song at 120 bpm with a vocal", lyr, &[], None, 1.0, 1, None, 0.0, &Fit::Duration(30.0));
        assert!(p.notes.iter().any(|n| n.contains("asked 0:30")), "{:?}", p.notes);
        // the sung line: one note per syllable, nothing shorter than a sixteenth
        let mut line: Vec<N> = (0..6).map(|i| N { start: i as f64 * 2.0, len: 2.0, key: 64, vel: 0.8 }).collect();
        line.push(N { start: 12.0, len: 0.1, key: 65, vel: 0.8 });
        fit_line(&mut line, 14);
        assert!(line.len() >= 14 && line.iter().all(|n| n.len >= 0.249), "{line:?}");
        let mut busy: Vec<N> = (0..32).map(|i| N { start: i as f64 * 0.25, len: 0.25, key: 64, vel: 0.8 }).collect();
        fit_line(&mut busy, 6);
        assert!(busy.len() <= 9 && busy.iter().map(|n| n.len).fold(0.0, f64::max) >= 1.0, "{busy:?}");
    }

    #[test]
    fn ensembles_story_theme_and_the_radar() {
        // an orchestra: its sections, each in its own register
        let p = plan("a sweeping full orchestra piece in D minor", "", &[], None, 1.0, 3);
        assert_eq!(p.ensemble, "orchestra");
        assert!(p.parts.iter().any(|x| x.role == "violins") && p.parts.iter().any(|x| x.role == "contrabasses") && p.parts.iter().any(|x| x.role == "timpani"), "{:?}", p.parts);
        for (i, part) in p.parts.iter().enumerate() {
            let Some(inst) = crate::ensemble::instrument(&part.role) else { continue };
            for sec in &p.sections {
                for n in section_notes(&p, i, sec, None) {
                    assert!(n.key + 12 >= inst.lo && n.key <= inst.hi + 12, "{} plays {} outside {}-{}", part.role, n.key, inst.lo, inst.hi);
                }
            }
        }
        // an exotic lute duet: two parts, no drums
        let d = plan("an exotic lute duet at dusk", "", &[], None, 1.0, 3);
        assert_eq!(d.parts.iter().map(|x| x.role.as_str()).collect::<Vec<_>>(), vec!["oud", "lute"]);
        // a story shapes the energy: a fall ends quieter than it starts
        let f = plan_story("pop song in C major", "", &[], None, 1.0, 3, None, 0.0, &Fit::Auto, "a tragic farewell that fades away");
        assert_eq!(f.story, "fall");
        let first = f.sections.iter().take(3).map(|s| s.energy).sum::<f64>();
        let last = f.sections.iter().rev().take(3).map(|s| s.energy).sum::<f64>();
        assert!(first > last, "{:?}", f.sections);
        assert!(f.notes.iter().any(|n| n.starts_with("story")), "{:?}", f.notes);
        // a theme: the lead plays it (its notes come back in the first verse)
        let mut t = plan("pop song with a synth lead in C major", "", &[], None, 1.0, 4);
        t.human = 0.0;
        t.ornament = 0.0;
        let theme: Vec<N> = [60u8, 62, 64, 67, 64, 62, 60, 55].iter().enumerate().map(|(i, k)| N { start: i as f64, len: 1.0, key: *k, vel: 0.8 }).collect();
        t.theme = Some(std::sync::Arc::new(theme));
        let li = t.parts.iter().position(|x| x.role == "synth lead").unwrap();
        let v = t.sections.iter().find(|s| s.tag == "verse").unwrap();
        let notes = section_notes(&t, li, v, None);
        assert!(notes.len() >= 8 && notes.iter().take(8).zip([60u8, 62, 64, 67, 64, 62, 60, 55]).filter(|(n, k)| n.key % 12 == k % 12).count() >= 5, "{notes:?}");
        // the radar: seven values in 0..1, and the steering moves them
        let m = metrics(&t);
        assert_eq!(m.len(), METRICS.len());
        assert!(m.iter().all(|x| (0.0..=1.0).contains(x)));
        let mut more = t.clone();
        more.color = 1.0;
        more.human = 1.0;
        let m2 = metrics(&more);
        assert!(m2[2] > m[2] && m2[4] > m[4]);
        // the line carries the new fields and reads them back
        let back = SongPlan::from_line(&more.to_line().replace("story=none", "story=journey").replace("drive=0.50", "drive=0.80"), &more);
        assert_eq!((back.story.as_str(), back.drive, back.color), ("journey", 0.8, 1.0));
    }

    #[test]
    fn the_harmony_travels_and_the_parts_come_in_gradually() {
        let mut p = plan("pop song in C major", "", &[], None, 1.0, 11);
        p.dials.variety = 0.8;
        p.dials.fifths = 1;
        p.dials.style = "loop".into();
        let idx = |tag: &str| p.sections.iter().position(|s| s.tag == tag).unwrap();
        let (v, c, b) = (idx("verse"), idx("chorus"), idx("bridge"));
        // each kind of section has its own chords, and the bridge sits a fifth up (G major)
        assert_ne!(section_prog(&p, v), section_prog(&p, c));
        assert_ne!(section_prog(&p, b), section_prog(&p, v));
        assert_eq!(section_key(&p, b), Key::new(7, false));
        assert_eq!(section_key(&p, v), Key::new(0, false));
        // the bar before a chorus is the dominant: a half cadence
        let before = c - 1;
        let cpb = 1.0;
        let (k, ch) = section_chords(&p, before, cpb);
        assert_eq!(ch.last().unwrap().2, k.scale()[4], "{:?}", ch);
        // low variety: one loop for the whole song (and the parts follow the section's chords)
        p.dials.variety = 0.1;
        let all: Vec<String> = (0..p.sections.len()).map(|i| section_prog(&p, i)).collect();
        assert!(all.iter().all(|x| *x == all[0]), "{all:?}");
        // a bridge's notes are in the bridge's key
        p.dials.variety = 0.8;
        let pi = p.parts.iter().position(|x| role_kind(&x.role) == Kind::Chords || role_kind(&x.role) == Kind::Pad).unwrap();
        let notes = section_notes(&p, pi, &p.sections[b].clone(), None);
        assert!(notes.iter().all(|n| Key::new(7, false).accepts(n.key % 12)), "{notes:?}");
        // gradual layering: no section change brings in more than three new parts, the first section is small
        let mut q = plan("trance anthem with supersaw lead, warm pad, arp and pluck", "", &[], None, 1.0, 5);
        q.dials.layering = 0.9;
        let a = arrange(&q);
        let count = |si: usize| (0..q.parts.len()).filter(|pi| a.on[*pi][si]).count();
        assert!(count(0) <= 2, "{}", count(0));
        for si in 1..q.sections.len() {
            let new = (0..q.parts.len()).filter(|pi| a.on[*pi][si] && !a.on[*pi][si - 1]).count();
            assert!(new <= 3, "section {si} brings {new} new parts");
        }
    }

    #[test]
    fn lyrics_go_to_their_sections() {
        let secs = form_sections(Form::Song, 1.0);
        let l = lyrics_for_sections("[Verse 1]\nwalking home\n[Chorus]\nwe are light\n[Verse 2]\nstars above", &secs);
        let at = |tag: &str, k: usize| secs.iter().enumerate().filter(|(_, s)| s.tag == tag).nth(k).map(|(i, _)| l[i].clone()).unwrap();
        assert_eq!(at("verse", 0), "walking home");
        assert_eq!(at("verse", 1), "stars above");
        assert_eq!(at("chorus", 0), "we are light");
        assert_eq!(at("chorus", 2), "we are light");
        assert_eq!(at("intro", 0), "");
        let plain = lyrics_for_sections("line one\nline two\n\nhook one\nhook two", &secs);
        assert_eq!(plain[secs.iter().position(|s| s.tag == "chorus").unwrap()], "hook one\nhook two");
    }

    #[test]
    fn a_reference_is_read() {
        // 32 bars at 120 bpm: 8 quiet bars of an A minor chord, 16 loud bars cycling Am F C G, 8 quiet bars
        let sr = 22050u32;
        let bar = (2.0 * sr as f64) as usize;
        let chords: [[f32; 3]; 4] = [[220.0, 261.63, 329.63], [174.61, 220.0, 261.63], [261.63, 329.63, 392.0], [196.0, 246.94, 293.66]];
        let mut st = vec![0f32; bar * 32 * 2];
        for b in 0..32 {
            let loud = (8..24).contains(&b);
            let c = if loud { chords[b % 4] } else { chords[0] };
            let amp = if loud { 0.3 } else { 0.06 };
            for i in 0..bar {
                let t = i as f32 / sr as f32;
                // a kick on every beat, so the tempo can be heard
                let beat = (i % (bar / 4)) as f32 / sr as f32;
                let kick = (std::f32::consts::TAU * 60.0 * beat).sin() * (-beat * 30.0).exp() * if loud { 0.8 } else { 0.2 };
                let x: f32 = c.iter().map(|f| (std::f32::consts::TAU * f * t).sin()).sum::<f32>() * amp / 3.0 + kick;
                st[(b * bar + i) * 2] = x;
                st[(b * bar + i) * 2 + 1] = x;
            }
        }
        let r = analyze_reference(&st, sr);
        assert!((r.bpm - 120.0).abs() < 3.0, "tempo {}", r.bpm);
        let k = r.key.expect("a key");
        assert!(k.tonic == 9 && k.minor || k.tonic == 0 && !k.minor, "A minor / C major, got {}", k.name());
        assert!(r.sections.len() >= 3, "{:?}", r.sections);
        assert_eq!(r.sections[0].tag, "intro");
        assert!(r.sections.iter().any(|s| s.tag == "chorus"), "{:?}", r.sections);
        assert!(!r.prog.is_empty(), "a progression");
        let p = plan("", "", &[], Some(&r), 1.0, 1);
        assert_eq!(p.sections, r.sections, "the song borrows the reference's form");
        assert_eq!(p.bpm, r.bpm);
    }

    #[test]
    fn intros_and_progressions_vary_with_the_dice() {
        let mut intros = std::collections::BTreeSet::new();
        let mut verses = std::collections::BTreeSet::new();
        let mut opening = std::collections::BTreeSet::new();
        for seed in 1..60u64 {
            let p = plan("a pop song", "", &[], None, 1.0, seed);
            assert!(INTROS[1..].contains(&p.intro.as_str()), "{}", p.intro);
            assert!(crate::emotion::ARCS.contains(&p.arc.as_str()), "the arc is rolled before the harmony: {}", p.arc);
            intros.insert(p.intro.clone());
            let vi = p.sections.iter().position(|s| s.tag == "verse").unwrap();
            verses.insert(section_prog(&p, vi));
            let a = arrange(&p);
            let first: Vec<String> = (0..p.parts.len()).filter(|i| a.on[*i][0]).map(|i| p.parts[i].role.clone()).collect();
            opening.insert(first.join("+"));
        }
        assert!(intros.len() >= 5, "intros: {intros:?}");
        assert!(verses.len() >= 6, "verse progressions: {verses:?}");
        assert!(opening.len() >= 4, "who opens the song: {opening:?}");
        // the prompt and the plan line choose it outright
        let p = plan("pop song with a cold open", "", &[], None, 1.0, 3);
        assert_eq!(p.intro, "cold");
        let ci = p.sections.iter().position(|s| is_chorus(&s.tag)).unwrap();
        assert_eq!(section_prog(&p, 0), section_prog(&p, ci), "a cold open plays the chorus's chords");
        let q = SongPlan::from_line("intro=beat", &p);
        assert_eq!(q.intro, "beat");
        let a = arrange(&q);
        let drums = q.parts.iter().position(|x| x.role == "drums").unwrap();
        assert!(a.on[drums][0], "a beat intro opens on the drums");
        assert!(q.to_line().contains("intro=beat"));
    }
}

#[cfg(test)]
mod movement_tests {
    use super::*;

    fn trance(extra: &str, seed: u64) -> SongPlan {
        let mut p = plan(&format!("uplifting trance at 138 bpm in A minor, {extra}"), "", &[], None, 1.0, seed);
        p.human = 0.0;
        p
    }

    #[test]
    fn movements_change_tempo_at_their_boundaries_with_a_transition() {
        let mut seen_ramp = false;
        for seed in 1..14 {
            let p = trance("three movements", seed);
            let mv = movements(&p);
            assert!(mv.len() >= 3, "seed {seed}: {mv:?}");
            let map = tempo_changes(&p);
            let st = section_starts(&p);
            for (i, m) in mv.iter().enumerate() {
                assert!(m.first < st.len() && (i == 0 || m.first > mv[i - 1].first));
                if i == 0 {
                    // an opening in its own tempo is in the map from beat 0, the song's own tempo is the project's
                    assert_eq!(map.iter().any(|(b, _)| *b == 0.0), (m.bpm - p.bpm).abs() > 0.5, "seed {seed}");
                    continue;
                }
                // the new tempo is in the map exactly on the movement's first beat
                assert!(map.iter().any(|(b, v)| (*b - st[m.first]).abs() < 1e-9 && (*v - m.bpm).abs() < 0.11), "seed {seed}: {} at {}: {map:?}", m.name, st[m.first]);
                if matches!(m.arrive, "accel" | "rit") {
                    // a ritardando / accelerando: the steps before the boundary move monotonically from the old tempo to the new
                    let ramp: Vec<f64> = map.iter().filter(|(b, _)| *b < st[m.first] && *b >= st[m.first] - 8.0).map(|(_, v)| *v).collect();
                    assert!(ramp.len() >= 2, "seed {seed}: {ramp:?}");
                    let up = m.bpm > mv[i - 1].bpm;
                    assert!(ramp.windows(2).all(|w| if up { w[1] >= w[0] } else { w[1] <= w[0] }), "{ramp:?}");
                    assert!(ramp.iter().all(|v| *v >= m.bpm.min(mv[i - 1].bpm) - 0.11 && *v <= m.bpm.max(mv[i - 1].bpm) + 0.11));
                    seen_ramp = true;
                } else {
                    // a break: a beat of silence closes the section before
                    assert_eq!(m.arrive, "break");
                    assert!(gap_after(&p, m.first - 1) >= 1.0, "seed {seed}");
                }
            }
            assert!(movement_notes(&p).iter().any(|n| n.starts_with("movements=")), "the plan says it");
        }
        assert!(seen_ramp);
        // one movement: no tempo changes; the dial survives the plan line
        let one = trance("one movement", 3);
        assert!(tempo_changes(&one).is_empty() && movements(&one).len() == 1);
        let two = SongPlan::from_line("movements=2", &one);
        assert_eq!(two.movements, "2");
        assert!(two.to_line().contains("movements=2") && movements(&two).len() >= 2);
    }

    #[test]
    fn the_emotional_arc_picks_the_kind_of_movement_and_the_key() {
        let mut p = trance("", 5);
        p.movements = "2".into();
        p.arc = "triumph".into();
        let mv = movements(&p);
        assert_eq!(mv.last().map(|m| m.name), Some("lift"), "{mv:?}");
        let m = mv.last().unwrap();
        assert!(m.bpm > p.bpm && m.key_lift >= 1);
        assert_eq!(section_key(&p, m.first).tonic, p.key.transposed(m.key_lift).tonic, "the lift sections are in the higher key");
        assert_eq!(section_key(&p, 0).tonic, p.key.tonic);
        p.arc = "melancholy".into();
        let mv = movements(&p);
        assert!(mv.iter().any(|m| m.name == "reflective" && m.bpm < p.bpm && m.relative), "{mv:?}");
    }

    #[test]
    fn builds_roll_faster_and_hold_a_breath_before_the_drop() {
        let mut p = trance("", 2);
        p.movements = "1".into();
        let di = p.parts.iter().position(|x| x.role == "drums").unwrap();
        let bi = p.sections.iter().position(|s| s.tag == "build").unwrap();
        assert!(leads_into_drop(&p, bi));
        let sec = &p.sections[bi];
        let len = sec.bars as f64 * 4.0;
        let gap = gap_after(&p, bi);
        assert!(gap >= 0.5, "{gap}");
        let notes = section_notes(&p, di, sec, None);
        let mut rolls: Vec<f64> = notes.iter().filter(|n| n.key == compose::K_SNARE || n.key == compose::K_CLAP).map(|n| n.start).collect();
        rolls.dedup();
        let gaps: Vec<f64> = rolls.windows(2).map(|w| w[1] - w[0]).filter(|g| *g > 1e-6).collect();
        assert!(gaps.first().copied().unwrap_or(0.0) >= 0.99, "starts on quarters: {gaps:?}");
        assert!(gaps.last().copied().unwrap_or(1.0) <= 0.126, "ends on thirty-seconds: {gaps:?}");
        for s in [1.0, 0.5, 0.25, 0.125] {
            assert!(gaps.iter().any(|g| (g - s).abs() < 1e-6), "a {s} stage: {gaps:?}");
        }
        assert!(gaps.windows(2).all(|w| w[1] <= w[0] + 1e-6), "the roll never slows: {gaps:?}");
        // the held breath: nothing starts in the last beat(s) of the build and nothing rings into them
        assert!(notes.iter().all(|n| n.start < len - gap + 1e-6 && n.start + n.len <= len - gap + 1e-6), "silence before the drop");
        let bass = p.parts.iter().position(|x| role_kind(&x.role) == Kind::Bass).unwrap();
        assert!(section_notes(&p, bass, sec, None).iter().all(|n| n.start + n.len <= len - gap + 1e-6));
        // the drop's first beat has a crash; the riser steps climb, then a downlifter falls
        let drop = &p.sections[bi + 1];
        assert!(section_notes(&p, di, drop, None).iter().any(|n| n.key == compose::K_CRASH && n.start < 0.05));
        let ri = p.parts.iter().position(|x| x.role == "riser").unwrap();
        let up = section_notes(&p, ri, sec, None);
        assert!(up.len() > 8 && up.last().unwrap().key > up[0].key, "a riser that climbs");
        let down = section_notes(&p, ri, drop, None);
        assert!(down.len() >= 4 && down.last().unwrap().key < down[0].key, "a downlifter after the hit");
        assert!(craft_notes(&p).iter().any(|n| n.contains("1/4 > 1/8 > 1/16 > 1/32")));
    }

    #[test]
    fn the_second_drop_switches_the_bass_and_pad_movements_rest_the_drums() {
        let mut p = trance("three movements", 4);
        p.dials.variety = 0.5;
        let bass = p.parts.iter().position(|x| role_kind(&x.role) == Kind::Bass).unwrap();
        let lead = p.parts.iter().position(|x| role_kind(&x.role) == Kind::Melody && x.role != "riser").unwrap();
        let drops: Vec<usize> = (0..p.sections.len()).filter(|i| p.sections[*i].tag == "drop").collect();
        assert!(drops.len() >= 2);
        let keys = |part: usize, si: usize| section_notes(&p, part, &p.sections[si], None).iter().map(|n| (n.key, (n.start * 8.0) as i64)).collect::<Vec<_>>();
        assert_ne!(keys(bass, drops[0]), keys(bass, drops[1]), "a new bass pattern");
        // the lead is the same in both drops when the key does not move between them
        let mut q = p.clone();
        q.movements = "1".into();
        let lead_in = |si: usize| section_notes(&q, lead, &q.sections[si], None).iter().map(|n| (n.key, (n.start * 8.0) as i64)).collect::<Vec<_>>();
        assert_eq!(lead_in(drops[0]), lead_in(drops[1]), "the drops repeat the lead");
        // pad-led sections: no drums, no bass, no lead; a pad plays
        let a = arrange(&p);
        let di = p.parts.iter().position(|x| x.role == "drums").unwrap();
        let pad = p.parts.iter().position(|x| role_kind(&x.role) == Kind::Pad && x.role != "riser").unwrap();
        let mv = movements(&p);
        for m in mv.iter().filter(|m| m.texture == "pads") {
            for si in m.first..=m.last {
                assert!(!a.on[di][si] && !a.on[bass][si] && !a.on[lead][si] && a.on[pad][si], "{} section {si} {}", m.name, p.sections[si].tag);
            }
        }
        assert!(a.notes.iter().any(|n| n.contains("pad-led")) || mv.iter().all(|m| m.texture != "pads"), "{:?}", a.notes);
    }

    #[test]
    fn a_tense_break_stays_a_half_time_groove() {
        // find a plan whose reflective movement is a half-time groove and check the drums play it
        let mut found = false;
        for seed in 1..60 {
            let mut p = trance("", seed);
            p.movements = "2".into();
            p.arc = "storm".into();
            let Some(m) = movements(&p).into_iter().find(|m| m.texture == "halftime") else { continue };
            let a = arrange(&p);
            let di = p.parts.iter().position(|x| x.role == "drums").unwrap();
            assert!(a.on[di][m.first]);
            let notes = section_notes(&p, di, &p.sections[m.first], None);
            assert!(notes.iter().any(|n| n.key == compose::K_KICK) && notes.iter().any(|n| n.key == compose::K_SNARE), "kick and snare play half-time");
            found = true;
            break;
        }
        assert!(found, "some tense feeling gives a half-time movement");
    }
}

#[cfg(test)]
mod flow_tests {
    use super::*;

    const SONGS: [(&str, &str); 8] = [
        ("trance", "uplifting trance at 138 bpm"),
        ("acid techno", "acid techno at 135 bpm"),
        ("future bass", "future bass at 150 bpm"),
        ("dubstep", "riddim dubstep at 140 bpm"),
        ("lofi", "lofi chill study beat"),
        ("ballad", "a piano ballad"),
        ("breakbeat", "breakbeat at 132 bpm"),
        ("ambient", "ambient drone"),
    ];

    fn song(prompt: &str, seed: u64) -> SongPlan {
        let mut p = plan(prompt, "", &[], None, 1.0, seed);
        p.human = 0.0;
        p
    }

    /// every part's notes per section (None where it rests)
    fn all_notes(p: &SongPlan) -> Vec<Vec<Option<Vec<N>>>> {
        let arr = arrange(p);
        (0..p.parts.len()).map(|pi| p.sections.iter().enumerate().map(|(si, s)| if arr.on[pi][si] { Some(section_notes(p, pi, s, None)) } else { None }).collect()).collect()
    }

    fn sounding(ns: &[(f64, f64, u8)], t: f64) -> Vec<u8> {
        ns.iter().filter(|n| n.0 <= t && n.0 + n.1 > t).map(|n| n.2).collect()
    }

    #[test]
    fn the_energy_curve_has_no_jarring_jumps() {
        let s = |tag: &str, e: f64| Section { tag: tag.into(), bars: 8, energy: e };
        let mut v = vec![s("intro", 0.2), s("verse", 0.9), s("chorus", 1.0), s("verse", 0.3), s("drop", 1.0), s("break", 0.2), s("outro", 0.1)];
        let n = smooth_energy(&mut v);
        assert_eq!(n, 2, "{v:?}");
        assert!((v[1].energy - 0.55).abs() < 1e-9, "a verse may not leap out of the intro");
        assert!((v[3].energy - 0.55).abs() < 1e-9, "nor fall off a cliff after the chorus");
        assert_eq!(v[4].energy, 1.0, "a drop may leap");
        assert_eq!(v[5].energy, 0.2, "a break may fall");
        for (_, prompt) in SONGS {
            for seed in [1, 5, 9] {
                let p = song(prompt, seed);
                for w in p.sections.windows(2) {
                    let d = w[1].energy - w[0].energy;
                    let up_ok = is_chorus(&w[1].tag) || is_build_tag(&w[1].tag);
                    let down_ok = is_bridge(&w[1].tag) || matches!(w[1].tag.as_str(), "outro" | "intro" | "breakdown");
                    assert!(d <= 0.35 + 1e-6 || up_ok, "{prompt}: {} -> {} jumps {d}", w[0].tag, w[1].tag);
                    assert!(-d <= 0.45 + 1e-6 || down_ok, "{prompt}: {} -> {} falls {d}", w[0].tag, w[1].tag);
                }
                // a deliberate leap into a drop / chorus has its setup: a build (roll, riser, sweep) or a fill and a crash
                assert!(flow_notes(&p).iter().any(|n| n.contains("fill")));
            }
        }
    }

    #[test]
    fn songs_flow_across_their_sections_in_every_genre() {
        let mut approaches_total = 0;
        for (genre, prompt) in SONGS {
            let p = song(prompt, 3);
            assert_eq!(p.genre, genre, "{prompt}");
            let notes = all_notes(&p);
            let starts = section_starts(&p);
            let ns = p.sections.len();
            let last = ns - 1;
            let key = section_key(&p, last);
            let triad = [key.tonic % 12, (key.tonic + if key.minor { 3 } else { 4 }) % 12, (key.tonic + 7) % 12];
            for (pi, part) in p.parts.iter().enumerate() {
                let kind = role_kind(&part.role);
                let x = crate::sounddesign::expression(&p.genre, &part.role, kind, p.bpm);
                for si in 0..ns {
                    let Some(n) = &notes[pi][si] else { continue };
                    let sec = &p.sections[si];
                    let len = sec.bars as f64 * 4.0;
                    let gap = gap_after(&p, si);
                    match kind {
                        Kind::Drums => {
                            // a fill into every new section (builds have their roll), a crash on the downbeat of the next one
                            if let Some(next) = p.sections.get(si + 1) {
                                if (next.tag != sec.tag || (next.energy - sec.energy).abs() >= 0.2) && !leads_into_drop(&p, si) && gap <= 0.0 && si + 1 != last {
                                    let hits = n.iter().filter(|x| x.start >= len - 1.0 - 1e-6 && (matches!(x.key, compose::K_SNARE | compose::K_RIM) || (41..=50).contains(&x.key))).count();
                                    assert!(hits >= 2, "{genre}: no fill at the end of {} {si}: {hits}", sec.tag);
                                }
                                if next.tag != sec.tag {
                                    if let Some(nn) = &notes[pi][si + 1] {
                                        assert!(nn.iter().any(|x| x.start < 0.05 && x.key == compose::K_CRASH), "{genre}: no crash on the downbeat of {}", next.tag);
                                    }
                                }
                            }
                        }
                        Kind::Arp if part.role != "riser" => {
                            if let Some(ap) = crate::sounddesign::arp_plan(&p.genre, (sec.energy + p.energy).clamp(0.05, 1.0)) {
                                for x in n.iter().filter(|x| !(si == last && x.start >= len - 4.0 - 1e-6) && !(x.start >= len - 1.0 && si + 1 < ns)) {
                                    let k = x.start / ap.step;
                                    assert!((k - k.round()).abs() < 1e-6, "{genre} {}: an arp note off its {} grid at {}", part.role, ap.step, x.start);
                                }
                            }
                        }
                        _ => {}
                    }
                    if matches!(kind, Kind::Bass | Kind::Melody) && x.mono {
                        // a mono line: only the chosen pairs overlap (they glide), within the slide's interval
                        let mut v = n.clone();
                        v.sort_by(|a, b| a.start.total_cmp(&b.start));
                        for w in v.windows(2) {
                            if w[1].start - w[0].start < 0.2 {
                                continue;
                            }
                            if w[0].start + w[0].len > w[1].start + 1e-6 {
                                let iv = (w[1].key as i32 - w[0].key as i32).unsigned_abs();
                                assert!(x.glide_secs > 0.0 && iv >= 1 && iv <= x.slide_max as u32, "{genre} {}: an unplanned overlap {w:?}", part.role);
                            }
                        }
                    }
                }
                // the lead walks into the next phrase: its last note is a step or two from the next section's first
                if kind == Kind::Melody && part.role != "voice" {
                    let mut leaps = vec![];
                    for si in 0..ns - 1 {
                        let (Some(a), Some(b)) = (&notes[pi][si], &notes[pi][si + 1]) else { continue };
                        let (sec, next) = (&p.sections[si], &p.sections[si + 1]);
                        let (Some(la), Some(fb)) = (a.iter().max_by(|x, y| x.start.total_cmp(&y.start)), b.iter().min_by(|x, y| x.start.total_cmp(&y.start).then(y.key.cmp(&x.key)))) else { continue };
                        let leap = (la.key as i32 - fb.key as i32).abs();
                        leaps.push(leap);
                        if next.tag != sec.tag && next.energy + 0.15 >= sec.energy && gap_after(&p, si) <= 0.0 && !(si + 1 == last && next.bars <= 1) {
                            assert!(leap <= 4, "{genre} {}: {} -> {} leaps {leap}", part.role, sec.tag, next.tag);
                        }
                    }
                    if !leaps.is_empty() {
                        let mean = leaps.iter().sum::<i32>() as f64 / leaps.len() as f64;
                        assert!(mean <= 7.0, "{genre} {}: the contour breaks at the boundaries {leaps:?}", part.role);
                    }
                }
                // chords and pads: little motion from chord to chord, across the section boundaries too; pads hold common tones
                if matches!(kind, Kind::Chords | Kind::Pad) && part.role != "riser" {
                    let abs: Vec<(f64, f64, u8)> = (0..ns).filter_map(|si| notes[pi][si].as_ref().map(|n| n.iter().map(|x| (starts[si] + x.start, x.len, x.key)).collect::<Vec<_>>())).flatten().collect();
                    let mut onsets: Vec<f64> = abs.iter().map(|n| n.0).collect();
                    onsets.sort_by(f64::total_cmp);
                    onsets.dedup_by(|a, b| (*a - *b).abs() < 0.03);
                    let (mut moved, mut voices, mut held) = (0.0, 0usize, 0usize);
                    for t in onsets.iter().skip(1) {
                        let before = sounding(&abs, t - 0.2);
                        let new: Vec<u8> = abs.iter().filter(|n| (n.0 - t).abs() < 0.03).map(|n| n.2).collect();
                        if before.len() < 2 || new.len() < 2 {
                            continue;
                        }
                        for k in &new {
                            moved += before.iter().map(|b| (*b as f64 - *k as f64).abs()).fold(f64::MAX, f64::min);
                            voices += 1;
                        }
                        held += sounding(&abs, t + 0.06).len().saturating_sub(new.len());
                    }
                    if voices > 0 {
                        let mean = moved / voices as f64;
                        assert!(mean <= 2.5, "{genre} {}: voices move {mean:.2} semitones a chord", part.role);
                    }
                    if kind == Kind::Pad && voices > 0 && part.role != "texture" {
                        assert!(held > 0, "{genre} {}: a pad holds its common tones", part.role);
                    }
                }
                // the ending resolves on the tonic
                if let Some(n) = &notes[pi][last] {
                    if kind != Kind::Drums && part.role != "riser" && !n.is_empty() {
                        let t_last = n.iter().map(|x| x.start).fold(0.0, f64::max);
                        let fin: Vec<u8> = n.iter().filter(|x| x.start >= t_last - 0.8).map(|x| x.key % 12).collect();
                        assert!(fin.iter().all(|pc| triad.contains(pc)), "{genre} {}: the last chord {fin:?} is not the tonic {}", part.role, key.name());
                        if matches!(kind, Kind::Bass | Kind::Melody) {
                            assert_eq!(fin.last().copied(), Some(key.tonic % 12), "{genre} {}: ends on the tonic", part.role);
                        }
                    }
                }
                // the bass approaches some chord changes by step
                if kind == Kind::Bass {
                    for si in 0..ns {
                        let Some(n) = &notes[pi][si] else { continue };
                        let spec_cpb = written(&p, pi, &p.sections[si], None).0.cpb;
                        let (_, chords) = section_chords(&p, si, spec_cpb);
                        for w in chords.windows(2) {
                            if w[0].2 == w[1].2 {
                                continue;
                            }
                            let t = w[1].0;
                            if let Some(b) = n.iter().filter(|x| x.start >= t - 1.0 && x.start < t - 0.05).max_by(|a, b| a.start.total_cmp(&b.start)) {
                                let d = (0..11i32).map(|o| ((b.key as i32 - (w[1].2 as i32 + 12 * o)).abs())).min().unwrap_or(99);
                                if (1..=2).contains(&d) {
                                    approaches_total += 1;
                                }
                            }
                        }
                    }
                }
            }
            // a noise sweep into the drops of electronic music, ending where the drop (or the breath before it) begins
            let sw = sweeps(&p);
            if matches!(genre, "ballad" | "ambient" | "lofi") {
                assert!(sw.is_empty(), "{genre}: no white-noise riser in acoustic music");
            } else if p.sections.windows(2).any(|w| is_build_tag(&w[0].tag) && is_chorus(&w[1].tag)) {
                assert!(!sw.is_empty(), "{genre}: a sweep into the drop");
                for (st, l) in &sw {
                    let end = st + l;
                    let si = starts.iter().rposition(|s| *s <= *st + 1e-6).unwrap();
                    let sec_end = starts[si] + p.sections[si].bars as f64 * 4.0;
                    assert!((end - (sec_end - gap_after(&p, si))).abs() < 1e-6, "{genre}: the sweep ends at {end}, the section at {sec_end}");
                }
            }
            // Ling Cinema: an impact on every downbeat a sweep rises into, none at all in acoustic music
            let cues = cinema_cues(&p);
            if matches!(genre, "ballad" | "lofi" | "folk" | "jazz" | "rock" | "orchestral") {
                assert!(cues.is_empty(), "{genre}: no cinema effects in acoustic music");
            }
            for (st, l) in &sw {
                let si = starts.iter().rposition(|s| *s <= *st + 1e-6).unwrap();
                if leads_into_drop(&p, si) {
                    assert!(cues.iter().any(|c| c.what == "impact" && (c.start - starts[si + 1]).abs() < 1e-6), "{genre}: an impact after the sweep at {st} (+{l})");
                }
            }
            for c in &cues {
                assert!(c.len > 0.0 && c.start >= 0.0 && c.key < 128, "{genre}: {c:?}");
            }
            assert!(p.notes.iter().any(|n| n.starts_with("flow:")) && p.notes.iter().any(|n| n.contains("resolves")), "{:?}", p.notes);
        }
        assert!(approaches_total >= 4, "bass approach notes: {approaches_total}");
    }

    #[test]
    fn two_arps_never_share_an_octave_and_the_acid_line_slides() {
        let mut both = 0;
        for seed in 1..=12 {
            let p = song("uplifting trance at 138 bpm", seed);
            let arr = arrange(&p);
            let arps: Vec<usize> = (0..p.parts.len()).filter(|i| role_kind(&p.parts[*i].role) == Kind::Arp).collect();
            assert!(arps.len() >= 2, "trance has a pluck and an arp: {:?}", p.parts);
            for (si, s) in p.sections.iter().enumerate() {
                let on: Vec<usize> = arps.iter().copied().filter(|i| arr.on[*i][si]).collect();
                if on.len() < 2 {
                    continue;
                }
                let med = |i: usize| {
                    let mut k: Vec<u8> = section_notes(&p, i, s, None).iter().map(|n| n.key).collect();
                    k.sort_unstable();
                    k.get(k.len() / 2).copied().unwrap_or(0) as i32
                };
                let (a, b) = (med(on[0]), med(on[1]));
                assert!((a - b).abs() >= 9, "{}: two arps at {a} and {b}", s.tag);
                both += 1;
            }
        }
        assert!(both >= 1, "the pluck and the arp play together somewhere");
        // the 303 line: some notes tied into the next (they glide), never more than an octave; the bass never arpeggiates
        let p = song("acid techno at 135 bpm", 2);
        let bi = p.parts.iter().position(|x| x.role == "acid bass").expect("an acid bass");
        let arr = arrange(&p);
        let mut ties = 0;
        for (si, s) in p.sections.iter().enumerate() {
            if !arr.on[bi][si] {
                continue;
            }
            let mut n = section_notes(&p, bi, s, None);
            n.sort_by(|a, b| a.start.total_cmp(&b.start));
            ties += n.windows(2).filter(|w| w[1].start - w[0].start >= 0.2 && w[0].start + w[0].len > w[1].start + 1e-6).count();
        }
        assert!(ties >= 2, "the acid line slides: {ties}");
        assert!(p.parts.iter().all(|x| !(role_kind(&x.role) == Kind::Bass && crate::sounddesign::arp_plan(&p.genre, 1.0).is_some() && x.role.contains("arp"))));
    }
}
