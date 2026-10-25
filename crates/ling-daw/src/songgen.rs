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
        (_, false) => &["I V vi IV", "vi IV I V", "I vi IV V", "IV I V vi", "I IV vi V"],
        (_, true) => &["i VI III VII", "i iv VII III", "i VII VI VII", "i VI iv V", "VI VII i i"],
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
    let gst = genre(&plan.genre).style;
    let style = if d.style == "auto" || d.style.is_empty() { if matches!(gst, "jazz" | "lofi" | "disco" | "deep house") { "circle" } else { "loop" } } else { d.style.as_str() };
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

/// the verse progression without the chorus rule (for the comparison above)
fn section_prog_plain(plan: &SongPlan, si: usize) -> String {
    let d = &plan.dials;
    if !plan.prog.is_empty() {
        return plan.prog.clone();
    }
    let key = section_key(plan, si);
    let gst = genre(&plan.genre).style;
    let style = if d.style == "auto" || d.style.is_empty() { if matches!(gst, "jazz" | "lofi" | "disco" | "deep house") { "circle" } else { "loop" } } else { d.style.as_str() };
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
    Genre { name: "folk", style: "pop", bpm: 96.0, kit: "Kit Brushes", drums: "halftime", form: Form::Song, roles: &["guitar", "piano", "strings", "woodwind", "drums"], lufs: -13.0, words: &["folk", "acoustic", "country", "ballad", "singer-songwriter"] },
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
                tag == "build" || tag == "intro" && form == Form::Club
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
    let hs = harmony_summary(&p);
    p.notes.push(hs);
    let a = arrange(&p);
    p.notes.extend(a.notes);
    // the emotional plan, before a note is written (emotion.rs)
    let words = lyrics_for_sections(lyrics, &p.sections);
    let ep = crate::emotion::plan(&p.sections, &p.arc, &p.genre, &words, p.dials.mood, p.seed);
    p.notes.push(format!("feel: {}", crate::emotion::summary(&ep, &p.sections)));
    p
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
            "genre={}; bpm={}; key={}; prog={}; chorus={}; structure={}; parts={}; voice={}; human={:.2}; story={}; energy={:+.2}; drive={:.2}; color={:.2}; ornament={:.2}; temp={:.2}; variety={:.2}; mood={:.2}; fifths={:+}; progstyle={}; layering={:.2}; design={:.2}; arc={}; seed={}",
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

// ── writing the notes ───────────────────────────────────────────────────────

/// The notes of one part in one section (clip-relative beats). Sections with the same tag get the same seed, so a chorus comes back as
/// the same chorus; energy scales the density; the last bar of a section gets a fill on the drums.
pub fn section_notes(plan: &SongPlan, part: usize, sec: &Section, singer_range: Option<(u8, u8)>) -> Vec<N> {
    let (spec, kind, mut notes) = written(plan, part, sec, singer_range);
    if plan.parts[part].role == "riser" {
        return notes;
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
    if kind == Kind::Bass {
        lock_to_kick(plan, sec, &spec, &mut notes);
    }
    blend_guide(plan, part, sec, kind, &spec, &mut notes);
    humanize(plan, part, sec, kind, &mut notes);
    notes
}

/// the phrase the composer writes for a part in a section, before the song shapes it
fn written(plan: &SongPlan, part: usize, sec: &Section, singer_range: Option<(u8, u8)>) -> (compose::Spec, Kind, Vec<N>) {
    let g = genre(&plan.genre);
    let role = plan.parts[part].role.as_str();
    let kind = if role == "voice" { Kind::Melody } else { role_kind(role) };
    let tag_seed = sec.tag.bytes().fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(b as u64));
    let seed = plan.seed.wrapping_mul(1_000_003).wrapping_add(part as u64 * 7919).wrapping_add(tag_seed);
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
        spec.pattern = g.drums.to_string();
        spec.fills = if sec.tag == "build" { 1.0 } else { 0.5 };
    }
    if let Some((lo, hi)) = singer_range {
        spec.lo = lo;
        spec.hi = hi;
        // a singable line: fewer, longer notes and a motif that comes back
        spec.density = (0.35 + 0.3 * sec.energy).min(0.65);
        spec.motif = 2;
    }
    if role == "riser" {
        // one long note per section, the automation does the rest
        return (spec, kind, vec![N { start: 0.0, len: sec.bars as f64 * 4.0, key: plan.key.tonic + 60, vel: 0.7 }]);
    }
    let mut notes = compose::generate(&spec);
    // drums in a quiet section: no kick or snare in a break, only the hats and the rim
    if kind == Kind::Drums && sec.energy < 0.45 {
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
        if si == 0 && sec.tag == "intro" && playing.len() > 2 {
            let mut ps = playing.clone();
            ps.sort_by_key(|p| rank(kinds[*p]));
            for p in &ps[2..] {
                on[*p][si] = false;
            }
            notes.push(format!("the intro starts with the {} and the {}", name(ps[0]), name(ps[1])));
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
        let pcs = [scale[d], scale[(d + 2) % 7], scale[(d + 4) % 7]].map(|x| (x + key.tonic) % 12);
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
        let p = plan("pop song in C major", "", &[], None, 1.0, 3);
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
        let mean = |n: &[N]| n.iter().map(|x| x.key as f64).sum::<f64>() / n.len().max(1) as f64;
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
}
