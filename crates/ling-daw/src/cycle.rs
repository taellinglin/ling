//! Song cycles: several songs that tell one story, the way a Liederzyklus does (a winter journey, a love story told in songs).
//!
//! A cycle has a *shape*: the chapters of its story, each with a feeling (valence, arousal), the emotional arc its song takes
//! (emotion.rs) and the story arc that shapes its sections (songgen.rs). From one description:
//!
//! * the shape is read from the words ("winter", "love", "grief" ...), leaned by the lyrics' feeling, or rolled;
//! * the chapters are spread over the songs asked for, each with a little dice;
//! * a tonal plan: every song's key follows its feeling - bright chapters move to the sharp side of the wheel of fifths, dark
//!   ones to the flat side and into minor - and a cycle that comes home ends in the key it started in;
//! * the tempo and energy follow the arousal; one singer sings the whole cycle;
//! * a motif (a two-bar theme) is written once and carried through every song in its key, so the songs share their material
//!   (the lead plays it in variations; see `SongPlan::theme`);
//! * the lyrics, when given for several songs ("---" between them, or "# Title" headers), go to their songs.
//!
//! Each song comes out as an ordinary plan line (`SongPlan::to_line`) plus its lyrics, story and motif, so `daw_song_build`
//! builds it like any other AI song - with the emotional plan and the vocal expression (expression.rs) on top.
use crate::compose::Rng;
use crate::songgen::{self, Fit};
use crate::theory::{Key, N};

pub const SHAPES: [&str; 6] = ["winter journey", "love story", "hero", "seasons", "grief", "dawn"];

#[derive(Clone, Copy, Debug)]
struct Chapter {
    name: &'static str,
    valence: f32,
    arousal: f32,
    /// emotion.rs arc of its song
    arc: &'static str,
    /// songgen.rs story arc of its sections
    story: &'static str,
    /// what happens, in words (the song's story text)
    words: &'static str,
}

const fn ch(name: &'static str, valence: f32, arousal: f32, arc: &'static str, story: &'static str, words: &'static str) -> Chapter {
    Chapter { name, valence, arousal, arc, story, words }
}

/// a shape's chapters, and whether the cycle comes home to its first key at the end
fn chapters(shape: &str) -> (&'static [Chapter], bool) {
    const WINTER: [Chapter; 6] = [
        ch("Departure", -0.3, 0.45, "bittersweet", "journey", "leaving at night, a last look back"),
        ch("The Road", -0.2, 0.55, "slow burn", "journey", "walking on through the cold"),
        ch("Memory", 0.25, 0.35, "lullaby", "dream", "a warm dream of spring"),
        ch("Frozen", -0.6, 0.3, "melancholy", "fall", "tears turn to ice"),
        ch("Storm", -0.5, 0.85, "storm", "battle", "the storm tears at everything"),
        ch("The Last Light", -0.7, 0.2, "melancholy", "fall", "alone at the end of the road"),
    ];
    const LOVE: [Chapter; 6] = [
        ch("Meeting", 0.4, 0.5, "release", "rise", "the first look"),
        ch("Bloom", 0.75, 0.75, "triumph", "rise", "falling in love"),
        ch("Doubt", -0.1, 0.55, "bittersweet", "journey", "something changes"),
        ch("The Break", -0.6, 0.8, "storm", "battle", "it falls apart"),
        ch("Absence", -0.55, 0.25, "melancholy", "fall", "the empty room"),
        ch("Remembrance", 0.15, 0.35, "bittersweet", "dream", "what remains is gratitude"),
    ];
    const HERO: [Chapter; 5] = [
        ch("The Call", 0.1, 0.45, "slow burn", "rise", "a voice calls far away"),
        ch("The Trial", -0.1, 0.75, "storm", "battle", "the fight begins"),
        ch("The Fall", -0.6, 0.35, "melancholy", "fall", "everything is lost"),
        ch("Rising", 0.3, 0.7, "slow burn", "rise", "standing up again"),
        ch("The Return", 0.7, 0.9, "triumph", "hero", "coming home changed"),
    ];
    const SEASONS: [Chapter; 4] = [
        ch("Spring", 0.5, 0.55, "release", "rise", "everything wakes"),
        ch("Summer", 0.75, 0.85, "triumph", "hero", "the long bright days"),
        ch("Autumn", -0.1, 0.4, "bittersweet", "fall", "the leaves let go"),
        ch("Winter", -0.4, 0.2, "lullaby", "dream", "the world sleeps under snow"),
    ];
    const GRIEF: [Chapter; 5] = [
        ch("Shock", -0.4, 0.3, "melancholy", "fall", "this cannot be"),
        ch("Anger", -0.5, 0.9, "storm", "battle", "why"),
        ch("Bargaining", -0.15, 0.55, "bittersweet", "journey", "if only"),
        ch("Sorrow", -0.7, 0.2, "melancholy", "fall", "the weight of it"),
        ch("Acceptance", 0.3, 0.35, "release", "dream", "carrying it gently"),
    ];
    const DAWN: [Chapter; 4] = [
        ch("Darkness", -0.5, 0.25, "lullaby", "dream", "the longest night"),
        ch("Stirring", -0.1, 0.45, "slow burn", "rise", "something moves"),
        ch("First Light", 0.3, 0.6, "release", "rise", "the sky turns grey, then gold"),
        ch("Morning", 0.7, 0.8, "triumph", "hero", "the sun is up"),
    ];
    match shape {
        "winter journey" => (&WINTER, false),
        "love story" => (&LOVE, true),
        "hero" => (&HERO, true),
        "seasons" => (&SEASONS, true),
        "grief" => (&GRIEF, true),
        _ => (&DAWN, true),
    }
}

/// the shape a description names ("" if none)
pub fn shape_from_words(text: &str) -> &'static str {
    let t = text.to_lowercase();
    let has = |ws: &[&str]| ws.iter().any(|w| t.contains(w));
    if has(&["winter", "winterreise", "wander", "journey", "exile", "snow"]) {
        "winter journey"
    } else if has(&["grief", "mourn", "loss", "funeral", "death", "lament", "requiem"]) {
        "grief"
    } else if has(&["love", "romance", "heart", "lover", "breakup", "break up"]) {
        "love story"
    } else if has(&["hero", "quest", "battle", "warrior", "adventure", "epic"]) {
        "hero"
    } else if has(&["season", "spring", "summer", "autumn", "year"]) {
        "seasons"
    } else if has(&["dawn", "sunrise", "morning", "hope", "healing", "awaken"]) {
        "dawn"
    } else {
        ""
    }
}

/// The lyrics of several songs: blocks split at "---" / "===" lines or at "# Title" / "## Title" / "[Song 2]" headers.
/// Returns (title or "", text) per block; one block when nothing splits them.
pub fn split_lyrics(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = vec![];
    let mut title = String::new();
    let mut cur: Vec<&str> = vec![];
    let flush = |out: &mut Vec<(String, String)>, title: &mut String, cur: &mut Vec<&str>| {
        let body = cur.join("\n").trim().to_string();
        if !body.is_empty() || !title.is_empty() {
            out.push((std::mem::take(title), body));
        }
        cur.clear();
    };
    for line in text.lines() {
        let l = line.trim();
        let rule = l.len() >= 3 && (l.chars().all(|c| c == '-') || l.chars().all(|c| c == '='));
        let header = l.starts_with('#') || (l.starts_with("[song") || l.starts_with("[Song")) && l.ends_with(']');
        if rule {
            flush(&mut out, &mut title, &mut cur);
        } else if header {
            flush(&mut out, &mut title, &mut cur);
            title = l.trim_start_matches('#').trim_start_matches('[').trim_end_matches(']').trim().to_string();
        } else {
            cur.push(line);
        }
    }
    flush(&mut out, &mut title, &mut cur);
    out
}

const MAJOR: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
const MINOR: [i32; 7] = [0, 2, 3, 5, 7, 8, 10];

/// a motif as scale degrees (0 = tonic, 7 = the octave) and rhythm, so it can be sung in every song's key and mode
#[derive(Clone, Debug, PartialEq)]
pub struct Motif {
    /// (start beats, length beats, degree)
    pub notes: Vec<(f64, f64, i32)>,
}

impl Motif {
    /// two bars that sound like a theme: it starts on the tonic or the fifth, moves mostly by step with one leap, and comes to
    /// rest on a long note of the triad
    pub fn roll(rng: &mut Rng) -> Motif {
        let rhythms: [&[f64]; 5] = [&[1.0, 0.5, 0.5, 1.0, 1.0, 2.0, 2.0], &[1.5, 0.5, 1.0, 1.0, 2.0, 2.0], &[0.5, 0.5, 1.0, 2.0, 1.0, 1.0, 2.0], &[1.0, 1.0, 1.5, 0.5, 2.0, 2.0], &[2.0, 1.0, 1.0, 1.0, 1.0, 2.0]];
        let r = rhythms[(rng.next() % rhythms.len() as u64) as usize];
        let mut deg: i32 = if rng.chance(0.5) { 0 } else { 4 };
        let leap_at = 1 + (rng.next() % (r.len() as u64 - 2)) as usize;
        let mut t = 0.0;
        let mut notes = vec![];
        for (k, len) in r.iter().enumerate() {
            if k == r.len() - 1 {
                // rest on the triad, near where the line is
                deg = [0i32, 2, 4, 7].iter().copied().min_by_key(|d| (d - deg).abs()).unwrap_or(0);
            } else if k > 0 {
                let step = if k == leap_at { [3, 4, -3][(rng.next() % 3) as usize] } else { [1, -1, 1, 2, -2, -1][(rng.next() % 6) as usize] };
                deg = (deg + step).clamp(-3, 9);
            }
            notes.push((t, *len, deg));
            t += len;
        }
        Motif { notes }
    }

    /// the motif in a key, around the C above middle C (where a singer and a lead both sit)
    pub fn realize(&self, key: Key) -> Vec<N> {
        let scale = if key.minor { MINOR } else { MAJOR };
        let base = 60 + key.tonic as i32 + if key.tonic > 6 { -12 } else { 0 };
        self.notes
            .iter()
            .map(|(s, l, d)| {
                let oct = d.div_euclid(7);
                let k = base + 12 * oct + scale[d.rem_euclid(7) as usize];
                N { start: *s, len: *l, key: k.clamp(36, 96) as u8, vel: if (s % 4.0).abs() < 1e-6 { 0.82 } else { 0.72 } }
            })
            .collect()
    }

    /// as a theme spec `daw_song_build` reads ("notes:start,len,key,vel;...")
    pub fn spec(&self, key: Key) -> String {
        let ns: Vec<String> = self.realize(key).iter().map(|n| format!("{},{},{},{:.2}", n.start, n.len, n.key, n.vel)).collect();
        format!("notes:{}", ns.join(";"))
    }
}

/// a theme spec "notes:start,len,key,vel;..." back into notes (None when it is not one)
pub fn parse_notes_spec(spec: &str) -> Option<Vec<N>> {
    let body = spec.trim().strip_prefix("notes:")?;
    let v: Vec<N> = body
        .split(';')
        .filter_map(|it| {
            let f: Vec<f64> = it.split(',').filter_map(|x| x.trim().parse::<f64>().ok()).collect();
            (f.len() >= 3 && f[1] > 0.0).then(|| N { start: f[0].max(0.0), len: f[1], key: f[2].clamp(0.0, 127.0) as u8, vel: f.get(3).copied().unwrap_or(0.75).clamp(0.05, 1.0) as f32 })
        })
        .collect();
    (!v.is_empty()).then_some(v)
}

#[derive(Clone, Debug)]
pub struct CycleSong {
    pub title: String,
    pub chapter: String,
    pub valence: f32,
    pub arousal: f32,
    /// the song's plan line, ready for `daw_song_build`
    pub line: String,
    pub lyrics: String,
    /// the story text the song is built with
    pub story: String,
    /// the cycle's motif in this song's key, as a theme spec
    pub theme: String,
    pub key: Key,
    pub bpm: f64,
    pub arc: String,
}

#[derive(Clone, Debug)]
pub struct CyclePlan {
    pub shape: String,
    pub songs: Vec<CycleSong>,
    pub motif: Motif,
    pub notes: Vec<String>,
}

impl CyclePlan {
    /// "winter journey cycle, 4 songs: I. Departure (D minor, 92 bpm, bittersweet) -> ..."
    pub fn summary(&self) -> String {
        let songs: Vec<String> = self.songs.iter().map(|s| format!("{} ({}, {} bpm, {})", s.title, s.key.name(), s.bpm, s.arc)).collect();
        let mut out = format!("{} cycle, {} songs: {}", self.shape, self.songs.len(), songs.join(" -> "));
        if !self.notes.is_empty() {
            out.push_str(&format!(" ({})", self.notes.join("; ")));
        }
        out
    }
}

fn roman(i: usize) -> &'static str {
    ["I", "II", "III", "IV", "V", "VI", "VII", "VIII", "IX", "X", "XI", "XII"].get(i).copied().unwrap_or("")
}

/// Plan a cycle of `count` songs (2..12) from a description and the lyrics (all songs' together, see [`split_lyrics`]). `shape`
/// "" or "auto" reads it from the words or rolls it; `length` the length scale of each song (1 = a normal song).
pub fn plan(desc: &str, lyrics: &str, count: usize, shape: &str, length: f64, seed: u64) -> CyclePlan {
    let mut rng = Rng::new(seed ^ 0xC1C1E);
    let mut notes = vec![];
    let blocks = split_lyrics(lyrics);
    let count = if count == 0 { blocks.len().max(4) } else { count }.clamp(2, 12);
    let shape: &'static str = if let Some(s) = SHAPES.iter().find(|s| **s == shape) {
        s
    } else {
        let named = shape_from_words(desc);
        if !named.is_empty() {
            named
        } else {
            let (v, _) = crate::emotion::lyric_lean(lyrics);
            let pool: &[&'static str] = if v < -0.1 { &["grief", "winter journey", "love story"] } else if v > 0.1 { &["dawn", "love story", "seasons", "hero"] } else { &SHAPES };
            let pick = pool[(rng.next() % pool.len() as u64) as usize];
            notes.push(format!("the dice chose a {pick} cycle"));
            pick
        }
    };
    let (chs, home_end) = chapters(shape);
    // the song the description makes is the cycle's home: its genre, key, tempo and singer
    let base = songgen::plan_story(desc, blocks.first().map(|b| b.1.as_str()).unwrap_or(""), &[], None, length, seed.max(1), None, 0.0, &Fit::Auto, "");
    let home = base.key;
    let home_major = if home.minor { (home.tonic + 3) % 12 } else { home.tonic };
    let singer = if base.singer.is_empty() { "aria".to_string() } else { base.singer.clone() };
    if base.singer.is_empty() {
        notes.push("a song cycle is sung: aria sings it".into());
    }
    let motif = Motif::roll(&mut rng);
    // which chapter each song tells: spread evenly through the story, but a song whose lyrics have a feeling of their own takes
    // the nearby chapter that feels like them (never going back in the story)
    let mut picks: Vec<usize> = vec![];
    for i in 0..count {
        let even = i as f64 / (count - 1) as f64 * (chs.len() - 1) as f64;
        let lo = picks.last().copied().unwrap_or(0);
        let hi = chs.len() - 1;
        let lean = blocks.get(i).map(|b| crate::emotion::lyric_lean(&b.1)).unwrap_or((0.0, 0.0));
        let ci = if lean.0.abs() < 0.05 {
            (even.round() as usize).max(lo)
        } else {
            (lo..=hi).min_by(|a, b| {
                let cost = |c: usize| (c as f64 - even).abs() * 0.35 + ((lean.0 * 2.5) as f64 - chs[c].valence as f64).abs();
                cost(*a).total_cmp(&cost(*b))
            }).unwrap_or(lo)
        };
        if lean.0.abs() >= 0.05 && ci != even.round() as usize {
            notes.push(format!("song {} is {} for its lyrics", i + 1, chs[ci].name));
        }
        picks.push(ci);
    }
    let mut songs = vec![];
    for i in 0..count {
        let ci = picks[i];
        let c = chs[ci];
        let again = picks[..i].iter().filter(|p| **p == ci).count();
        let jit = |rng: &mut Rng| (rng.f() as f32 - 0.5) * 0.12;
        // lyrics with a feeling of their own pull the song halfway to it (a happy dream inside a dark chapter stays a dream)
        let lean = blocks.get(i).map(|b| crate::emotion::lyric_lean(&b.1).0).unwrap_or(0.0);
        let felt = if lean.abs() >= 0.05 { 0.5 * c.valence + 0.5 * (lean * 2.5).clamp(-1.0, 1.0) } else { c.valence };
        let valence = (felt + jit(&mut rng)).clamp(-1.0, 1.0);
        let arousal = (c.arousal + jit(&mut rng)).clamp(0.05, 1.0);
        // the tonal plan: the feeling moves the key round the wheel of fifths and between major and minor
        // a song that the lyrics turned against its chapter's arc is sung bittersweet: a warm chorus over sad verses (a dream inside
        // a dark chapter), or a sad heart under a bright one
        let arc = if (valence > 0.1 && matches!(c.arc, "melancholy" | "storm")) || (valence < -0.3 && matches!(c.arc, "triumph" | "release")) {
            notes.push(format!("song {} is bittersweet for its lyrics", i + 1));
            "bittersweet"
        } else {
            c.arc
        };
        let minor = valence < -0.1 || (matches!(arc, "melancholy" | "storm") && valence < 0.15);
        let last = i + 1 == count;
        let fifths = if i == 0 || (last && home_end) { 0 } else { (valence * 2.2).round().clamp(-2.0, 2.0) as i32 };
        let major_tonic = (home_major as i32 + 7 * fifths).rem_euclid(12);
        let mut key = Key { tonic: if minor { ((major_tonic + 9) % 12) as u8 } else { major_tonic as u8 }, minor };
        if (i == 0 || (last && home_end)) && minor == home.minor {
            key = home;
        }
        let bpm = (base.bpm * (0.8 + 0.4 * arousal as f64)).round().clamp(56.0, 180.0);
        let (title_given, text) = blocks.get(i).cloned().unwrap_or_default();
        let name = if again > 0 { format!("{} {}", c.name, roman(again)) } else { c.name.to_string() };
        let title = if title_given.is_empty() { format!("{}. {}", roman(i), name) } else { format!("{}. {}", roman(i), title_given) };
        let sseed = seed.wrapping_add(7919 * (i as u64 + 1));
        let mut p = songgen::plan_story(desc, &text, &[], None, length, sseed, None, 0.0, &Fit::Auto, c.words);
        p.genre = base.genre.clone();
        p.key = key;
        p.bpm = bpm;
        p.singer = singer.clone();
        p.arc = arc.to_string();
        p.story = c.story.to_string();
        p.energy = ((arousal as f64 - 0.5) * 0.5).clamp(-0.3, 0.3);
        // the storms swing harder, the lullabies hardly at all
        p.dials.mood = (0.25 + 0.6 * arousal as f64 * (1.0 - valence.max(0.0) as f64 * 0.3)).clamp(0.1, 0.9);
        p.seed = sseed;
        songs.push(CycleSong {
            title,
            chapter: name,
            valence,
            arousal,
            line: p.to_line(),
            lyrics: text,
            story: c.words.to_string(),
            theme: motif.spec(key),
            key,
            bpm,
            arc: arc.to_string(),
        });
    }
    if blocks.len() > count {
        notes.push(format!("{} lyric blocks for {} songs: the last {} are not sung", blocks.len(), count, blocks.len() - count));
    }
    CyclePlan { shape: shape.to_string(), songs, motif, notes }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_winter_journey_darkens_and_does_not_come_home() {
        let c = plan("a winter journey, piano and voice, in D minor", "", 6, "", 1.0, 3);
        assert_eq!(c.shape, "winter journey");
        assert_eq!(c.songs.len(), 6);
        assert!(c.songs[0].title.starts_with("I. Departure"), "{}", c.songs[0].title);
        // the memory chapter is the bright one; the end is dark, slow and minor
        let mem = &c.songs[2];
        let end = c.songs.last().unwrap();
        assert!(mem.valence > end.valence && end.key.minor && end.bpm < c.songs[4].bpm, "{}", c.summary());
        // one singer, one motif, every song a buildable line
        for s in &c.songs {
            assert!(s.line.contains("voice=") && !s.line.contains("voice=none"), "{}", s.line);
            assert!(s.line.contains(&format!("arc={}", s.arc)));
            let n = parse_notes_spec(&s.theme).unwrap();
            assert_eq!(n.len(), c.motif.notes.len());
        }
    }

    #[test]
    fn a_cycle_that_comes_home_ends_in_its_first_key() {
        let c = plan("a hero's quest, cinematic, in C major", "", 5, "", 1.0, 11);
        assert_eq!(c.shape, "hero");
        let (first, last) = (c.songs[0].key, c.songs.last().unwrap().key);
        assert_eq!(first, last, "{}", c.summary());
        // the fall is darker than the return, and in minor
        assert!(c.songs[2].key.minor && !last.minor);
        assert!(c.songs[4].bpm > c.songs[2].bpm);
        // the same seed, the same cycle
        assert_eq!(plan("a hero's quest, cinematic, in C major", "", 5, "", 1.0, 11).summary(), c.summary());
    }

    #[test]
    fn lyrics_go_to_their_songs() {
        let ly = "# Snow\nwhite on white\nI walk alone\n---\nthe crow follows me\n## Spring Dream\nflowers in the ice";
        let b = split_lyrics(ly);
        assert_eq!(b.len(), 3, "{b:?}");
        assert_eq!(b[0].0, "Snow");
        assert_eq!(b[2], ("Spring Dream".to_string(), "flowers in the ice".to_string()));
        let c = plan("song cycle", ly, 0, "seasons", 1.0, 5);
        assert_eq!(c.songs.len(), 4);
        assert_eq!(c.songs[0].title, "I. Snow");
        assert!(c.songs[1].lyrics.contains("crow"));
        assert!(c.songs[3].lyrics.is_empty());
    }

    #[test]
    fn a_songs_lyrics_choose_its_chapter() {
        // four songs over six winter chapters: by position the second would be the bright Memory, but its words are frozen tears
        let ly = "# Good Night
I leave alone tonight
---
# Frozen Tears
cold tears, frozen and broken
---
# Spring Dream
I dream of light and a smile in the sun
---
# The End
the road goes on";
        let c = plan("a winter journey", ly, 4, "", 1.0, 21);
        let names: Vec<&str> = c.songs.iter().map(|s| s.chapter.as_str()).collect();
        assert_ne!(names[1], "Memory", "{names:?}");
        assert!(c.songs[1].valence < 0.0 && c.songs[1].key.minor, "{}", c.summary());
        assert!(c.songs[2].valence > c.songs[1].valence, "{}", c.summary());
        // the happy dream inside the frozen chapter is sung bittersweet, in major
        assert_eq!(c.songs[2].arc, "bittersweet", "{}", c.summary());
        assert!(!c.songs[2].key.minor, "{}", c.summary());
        // the story still only goes forward
        let order: Vec<usize> = names.iter().map(|n| ["Departure", "The Road", "Memory", "Frozen", "Storm", "The Last Light"].iter().position(|x| n.starts_with(x)).unwrap()).collect();
        assert!(order.windows(2).all(|w| w[0] <= w[1]), "{names:?}");
    }

    #[test]
    fn the_motif_moves_with_the_key_and_mode() {
        let mut rng = Rng::new(9);
        let m = Motif::roll(&mut rng);
        let total: f64 = m.notes.iter().map(|n| n.1).sum();
        assert!((total - 8.0).abs() < 1e-9, "two bars: {m:?}");
        let c = m.realize(Key { tonic: 0, minor: false });
        let g = m.realize(Key { tonic: 7, minor: false });
        // the same shape a fifth up (or a fourth down)
        let d: Vec<i32> = c.iter().zip(&g).map(|(a, b)| b.key as i32 - a.key as i32).collect();
        assert!(d.iter().all(|x| *x == d[0]) && (d[0] == 7 || d[0] == -5), "{d:?}");
        let back = parse_notes_spec(&m.spec(Key { tonic: 2, minor: true })).unwrap();
        assert_eq!(back.iter().map(|n| n.key).collect::<Vec<_>>(), m.realize(Key { tonic: 2, minor: true }).iter().map(|n| n.key).collect::<Vec<_>>());
    }
}
