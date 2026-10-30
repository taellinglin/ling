//! Ling Nova's factory presets and their usage notes, read from `nova_presets.txt` (compiled in).
//!
//! The file is a list of `[Name]` sections of `key = value` lines. A section whose name starts with `_` is a template: a preset
//! names one with `base = _growl` and starts from its sound and its notes. Values are numbers 0..1 or, for selectors, a choice by
//! name (`a table = Vowel AEIOU`, `f1 type = formant`, `l1 div = 1/8T`); `on` / `off` for switches. `mod = m1 > f1 cut : 0.4 bi
//! crv=0.7 aux=vel` fills the next free slot of the mod matrix.
//!
//! The usage keys (read by the panel and by the generators, see `Usage`): `cat`, `desc`, `role` (bass / lead / chord / pluck / pad
//! / fx), `family`, `genres`, `range` (`C1-C3`), `style` (how the generator writes notes: stab16, stab8, held, slide, chords, pluck,
//! arp, riser), `play`, `macros` (the eight labels, `|` between), `notes` (what each macro does and when to move it), `pattern`,
//! `mix`, `auto` (the macro moves per section: `m1 rise/wob8/low` = build / drop / breakdown, `;` between macros) and `switch`
//! (a group name: presets of one group trade bars in a call-and-response).
use super::layout::{layout, PK};
use std::sync::OnceLock;

#[derive(Clone, Debug, Default)]
pub struct Usage {
    pub role: String,
    pub family: String,
    pub genres: Vec<String>,
    pub lo: u8,
    pub hi: u8,
    pub style: String,
    pub play: String,
    pub macros: Vec<String>,
    pub notes: String,
    pub pattern: String,
    pub mix: String,
    /// (macro index 0..7, [build, drop, breakdown] moves)
    pub auto: Vec<(usize, [String; 3])>,
    pub switch: String,
}

impl Usage {
    /// a macro's label ("macro 3" when the preset leaves it unnamed)
    pub fn macro_label(&self, k: usize) -> String {
        self.macros.get(k).filter(|s| !s.is_empty()).cloned().unwrap_or_else(|| format!("macro {}", k + 1))
    }
    /// the whole note as readable text (the panel's "how to use", the guide)
    pub fn text(&self) -> String {
        let mut s = format!("Role: {} ({}). Genres: {}. Range: {}-{}. Play: {}", self.role, self.family, self.genres.join(", "), note_name(self.lo), note_name(self.hi), self.play);
        if !self.notes.is_empty() {
            s.push_str(&format!(" Macros: {}", self.notes));
        }
        if !self.pattern.is_empty() {
            s.push_str(&format!(" Pattern: {}", self.pattern));
        }
        if !self.mix.is_empty() {
            s.push_str(&format!(" Mix: {}", self.mix));
        }
        s
    }
    /// does the preset suit this genre (a word of it)?
    pub fn suits(&self, genre: &str) -> bool {
        let g = genre.to_lowercase();
        self.genres.iter().any(|x| g.contains(x.as_str()) || x.contains(g.as_str()))
    }
}

pub struct Preset {
    pub name: String,
    pub cat: String,
    pub desc: String,
    pub usage: Usage,
    pub vals: Vec<f32>,
}

impl Preset {
    /// the voice mode the preset plays in: "poly", "mono" or "legato" (its `voice mode` parameter; templates set it: basses mono,
    /// leads legato, the colour basses, chords, plucks, pads and effects poly)
    pub fn voice_mode(&self) -> &'static str {
        let l = layout();
        match l.index.get("voice mode") {
            Some(&i) => crate::polyphony::mode_name(self.vals[i]),
            None => "poly",
        }
    }
    /// one note at a time (mono or legato): never the patch for a chord part
    pub fn is_mono(&self) -> bool {
        self.voice_mode() != "poly"
    }
    /// "mono" or "poly": the class the choosers match against a part's measured polyphony (crate::polyphony)
    pub fn voice_class(&self) -> &'static str {
        if self.is_mono() {
            "mono"
        } else {
            "poly"
        }
    }
}

pub fn note_name(k: u8) -> String {
    const N: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", N[k as usize % 12], k as i32 / 12 - 1)
}

/// "C1" -> 24, "F#2" -> 42
pub fn parse_note(s: &str) -> Option<u8> {
    let s = s.trim();
    let mut ch = s.chars();
    let base = match ch.next()?.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest: String = ch.collect();
    let (acc, oct) = if let Some(r) = rest.strip_prefix('#') { (1, r) } else if let Some(r) = rest.strip_prefix('b') { (-1, r) } else { (0, rest.as_str()) };
    let o: i32 = oct.parse().ok()?;
    Some(((o + 1) * 12 + base + acc).clamp(0, 127) as u8)
}

pub fn all() -> &'static [Preset] {
    static P: OnceLock<Vec<Preset>> = OnceLock::new();
    P.get_or_init(|| parse(include_str!("../../nova_presets.txt")).into_iter().filter(|p| !p.name.starts_with('_')).collect())
}

pub fn names() -> &'static [&'static str] {
    static N: OnceLock<Vec<&'static str>> = OnceLock::new();
    N.get_or_init(|| all().iter().map(|p| &*Box::leak(p.name.clone().into_boxed_str())).collect())
}

pub fn find(name: &str) -> Option<&'static Preset> {
    all().iter().find(|p| p.name.eq_ignore_ascii_case(name))
}

/// one value of a parameter from its text: a number, a switch word, or a selector's choice by name
fn value_of(pi: usize, text: &str) -> Option<f32> {
    let l = layout();
    let s = &l.specs[pi];
    let t = text.trim();
    // a selector's choice by name first (octave choices such as "-1" look like numbers)
    if s.kind == PK::Sel {
        if let Some(i) = s.opts.iter().position(|o| o.eq_ignore_ascii_case(t)) {
            return Some(if s.opts.len() > 1 { i as f32 / (s.opts.len() - 1) as f32 } else { 0.0 });
        }
    }
    if let Ok(x) = t.parse::<f32>() {
        return Some(x.clamp(0.0, 1.0));
    }
    match s.kind {
        PK::Sw => match t {
            "on" | "yes" | "true" => Some(1.0),
            "off" | "no" | "false" => Some(0.0),
            _ => None,
        },
        PK::Sel => {
            let i = s.opts.iter().position(|o| o.eq_ignore_ascii_case(t))?;
            Some(if s.opts.len() > 1 { i as f32 / (s.opts.len() - 1) as f32 } else { 0.0 })
        }
        PK::Cont => None,
    }
}

/// parse the preset file (templates included, in file order)
pub fn parse(text: &str) -> Vec<Preset> {
    let mut errs = vec![];
    let out = parse_checked(text, &mut errs);
    for e in errs {
        eprintln!("{e}");
    }
    out
}

/// parse, collecting the problems (unknown parameters, bad values, a full matrix) in `errs`
pub fn parse_checked(text: &str, errs: &mut Vec<String>) -> Vec<Preset> {
    let l = layout();
    let defaults: Vec<f32> = l.specs.iter().map(|s| s.def).collect();
    let mut out: Vec<Preset> = vec![];
    let mut cur: Option<Preset> = None;
    let finish = |cur: &mut Option<Preset>, out: &mut Vec<Preset>| {
        if let Some(p) = cur.take() {
            out.push(p);
        }
    };
    for (ln, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            finish(&mut cur, &mut out);
            cur = Some(Preset { name: line[1..line.len() - 1].trim().to_string(), cat: String::new(), desc: String::new(), usage: Usage { lo: 24, hi: 60, ..Default::default() }, vals: defaults.clone() });
            continue;
        }
        let Some(p) = cur.as_mut() else { continue };
        let Some((k, val)) = line.split_once('=') else {
            errs.push(format!("nova_presets.txt:{}: no '=' in {line}", ln + 1));
            continue;
        };
        let (k, val) = (k.trim(), val.trim());
        match k {
            "base" => {
                if let Some(b) = out.iter().find(|b| b.name == val) {
                    p.vals = b.vals.clone();
                    p.usage = b.usage.clone();
                    if p.cat.is_empty() {
                        p.cat = b.cat.clone();
                    }
                } else {
                    errs.push(format!("nova_presets.txt:{}: no template {val}", ln + 1));
                }
            }
            "cat" => p.cat = val.to_string(),
            "desc" => p.desc = val.to_string(),
            "role" => p.usage.role = val.to_string(),
            "family" => p.usage.family = val.to_string(),
            "genres" => p.usage.genres = val.split(',').map(|g| g.trim().to_lowercase()).filter(|g| !g.is_empty()).collect(),
            "range" => {
                if let Some((a, b)) = val.split_once('-') {
                    if let (Some(a), Some(b)) = (parse_note(a), parse_note(b)) {
                        p.usage.lo = a.min(b);
                        p.usage.hi = a.max(b);
                    }
                }
            }
            "style" => p.usage.style = val.to_string(),
            "play" => p.usage.play = val.to_string(),
            "macros" => p.usage.macros = val.split('|').map(|m| m.trim().to_string()).collect(),
            "notes" => p.usage.notes = val.to_string(),
            "pattern" => p.usage.pattern = val.to_string(),
            "mix" => p.usage.mix = val.to_string(),
            "switch" => p.usage.switch = val.to_string(),
            "auto" => {
                p.usage.auto.clear();
                for part in val.split(';') {
                    let mut it = part.split_whitespace();
                    let (Some(m), Some(moves)) = (it.next(), it.next()) else { continue };
                    let Some(mi) = m.strip_prefix('m').and_then(|x| x.parse::<usize>().ok()).filter(|x| (1..=8).contains(x)) else { continue };
                    let mv: Vec<&str> = moves.split('/').collect();
                    if mv.len() == 3 {
                        p.usage.auto.push((mi - 1, [mv[0].to_string(), mv[1].to_string(), mv[2].to_string()]));
                    }
                }
            }
            "mod" => {
                // SRC > DST : AMT [bi] [crv=x] [aux=SRC]
                let Some((sd, rest)) = val.split_once(':') else { continue };
                let Some((s, d)) = sd.split_once('>') else { continue };
                let mut words = rest.split_whitespace();
                let amt: f32 = words.next().and_then(|a| a.parse().ok()).unwrap_or(0.0);
                let slot = l.mat.iter().position(|m| p.vals[m.src] == 0.0 || (p.vals[m.amt] - 0.5).abs() < 1e-4);
                let Some(slot) = slot else {
                    errs.push(format!("nova_presets.txt:{}: the matrix is full", ln + 1));
                    continue;
                };
                let m = l.mat[slot];
                match (value_of(m.src, s.trim()), value_of(m.dst, d.trim())) {
                    (Some(sv), Some(dv)) => {
                        p.vals[m.src] = sv;
                        p.vals[m.dst] = dv;
                        p.vals[m.amt] = (0.5 + amt * 0.5).clamp(0.0, 1.0);
                        p.vals[m.bi] = 0.0;
                        p.vals[m.crv] = 0.5;
                        p.vals[m.aux] = 0.0;
                    }
                    _ => {
                        errs.push(format!("nova_presets.txt:{}: unknown source or destination in {val}", ln + 1));
                        continue;
                    }
                }
                for w in words {
                    if w == "bi" {
                        p.vals[m.bi] = 1.0;
                    } else if let Some(c) = w.strip_prefix("crv=") {
                        p.vals[m.crv] = c.parse().unwrap_or(0.5);
                    } else if let Some(a) = w.strip_prefix("aux=") {
                        if let Some(av) = value_of(m.aux, a) {
                            p.vals[m.aux] = av;
                        }
                    }
                }
            }
            _ => match l.index.get(k) {
                Some(&pi) => match value_of(pi, val) {
                    Some(x) => p.vals[pi] = x,
                    None => errs.push(format!("nova_presets.txt:{}: bad value {val} for {k}", ln + 1)),
                },
                None => errs.push(format!("nova_presets.txt:{}: unknown parameter {k}", ln + 1)),
            },
        }
    }
    finish(&mut cur, &mut out);
    out
}

/// how New AI Song and the Medley maker produce with Nova (novagen.rs, api/nova_api.rs), for the guide
const GENERATORS: &str = "
## How the generators produce with Nova

New AI Song and the Medley maker do more than load a preset (`novagen.rs`, `api/nova_api.rs`):

- **Mono or poly.** Every preset has a voice class from its own `voice mode` (below, \"Voices\"): the basses are mono (the colour basses poly, they play chords), the leads mono or legato, chords, plucks, pads and effects poly. The choosers measure each part's polyphony from its notes (`polyphony.rs`, small legato overlaps ignored) or expect it from the role (pads, chords, stabs and keys poly; basses, subs, 808s and leads mono) and pick a preset whose voice fits: a chord part never gets a mono preset while a poly one plays the role (a brass part in chords gets Brass Chord Stab, a brass line Brass Lead). Where only mono presets play a role and the part plays chords, the track's voice is set to poly (New AI Song, the Medley maker, the import and its Nova stage all run this guard), so no chord loses notes.
- **Which presets.** The club genres (dubstep, riddim, future bass, house, deep house, bass house, garage, drum and bass, breakbeat, trap, hyperpop, trance, hardstyle) play their electronic bass, lead, chord, pluck and pad parts on Nova, by family (growl, riddim, neuro, reese, colour, supersaw, screech ...). The song's seed rolls among the fitting presets, so different songs showcase different ones, and the plan's notes name them (\"Nova presets: Growl Bass Snarl Bass, ...\"). Elsewhere Nova plays only what fits the palette: pads and plucks in pop and reggaeton, pads, an arp and a lead in synthwave, dark pads in cinematic music, pads and bell plucks in ambient, Lofi Keys in lo-fi and hip hop, Detroit Stab in techno; never in jazz, rock, folk, orchestral music or ballads. Siren Screech is never chosen (its pitch LFO fails the tune check; another screech plays the part).
- **A song variant of the patch** (\"Hungry Growl · song variant\"): the wavetable positions and warps moved a little by the seed, the unison wider for the big genres and narrower for the soft ones, the low-pass brighter or darker for the genre, a formant's throat nudged, a fixed comb tuned to the song's key, a pluck's decay and a pad's swell and tail on the tempo, free LFOs locked to the nearest note value, the drive and the multiband by the genre's energy, more room in ambient and film music, a sub-heavy preset's own sub down. Pitch, tuning and glide are never touched, and the variant is tune-checked before it goes on (it is dropped if it would play out of tune).
- **A drop movement in the matrix.** Pads, chords, plucks and leads get a free LFO routed into the filter or the wavetable through a free matrix slot, scaled by a free macro; the macro opens in the drops only, and the next drop plays the LFO on another note value (a chord's 1/8 becomes 1/4 or 1/8T).
- **The story per section.** On top of each preset's own macro moves: the low-pass opens through every build (deeper on the next one), sits full in the drop, comes down in a breakdown and closes over the outro; the wavetable opens into the drop; a breakdown goes wider and wetter (a bass stays narrow and dry). Every section has a budget: at most three lanes moving in a build or a drop, two elsewhere, each within a capped span; lanes past it are held, spans past it scaled down.
- **Layers.** One track owns the sub: a sub part, or a Nova \"Sub Layer\" (Clean or Warm Sub, mono, cut above 140 Hz) that plays the bass line one note at a time, joined where a riddim chops it. The growls above it and their switching siblings lose their own sub and are cut below 95 Hz (120 Hz for Hungry Growl, Wet Gargle and Laser Wobble). In dubstep, drum and bass, hardstyle and bass house a \"Top Layer\" of the main growl plays the drops, driven harder, wide, above 600 Hz. A supersaw lead of the club genres is doubled by a pluck in the drops (\"Pluck Double\", on the tempo's pluck gate, half its level).
- **Voicings and gates.** Chords and pads of the club genres are re-voiced as a supersaw spread (the root at the bottom, the fifth, the other tones an octave up, the root on top); plucks and arps are gated on the grid (about 0.2 s rounded to one or two 1/16 notes).
- **The pump and the mix.** Where the genre pumps (house, garage, future bass, trance; dubstep and hardstyle a little less; drum and bass, breakbeat, synthwave and techno lighter; trap and hyperpop only the basses) every Nova track ducks under the kick through a Duck route: pads about 12 dB, chords 9, the sub 10, basses 7, plucks 5, leads 4, the release on a 1/8 or a 1/16 note. Pads, chords, plucks and leads get a gentle low cut by role (never above their own lowest note).
";

/// the guide (docs/NOVA_PRESETS.md): every preset by category with its description and how to use it
pub fn guide_markdown() -> String {
    let mut s = String::from("# Ling Nova presets\n\nLing Nova is LingStationX's wavetable synthesizer for modern electronic and bass music. This guide lists every factory preset by category: what it is, where it sits, how to play it, what its macros do, and how to mix it. The same notes show under the preset chooser in the Nova panel and drive the song and medley generators (range, playing style, macro moves per section, preset switching).\n\nMacro moves in the tables are given as build / drop / breakdown: `rise` = opens across the build, `wob8` / `wob16` = moves on 1/8 or 1/16 notes, `step16` = a stepped 1/16 pattern, `low` / `mid` / `high` = held there.\n");
    s.push_str(GENERATORS);
    let mut cats: Vec<String> = vec![];
    for p in all() {
        if !cats.contains(&p.cat) {
            cats.push(p.cat.clone());
        }
    }
    for c in cats {
        s.push_str(&format!("\n## {c}\n"));
        for p in all().iter().filter(|p| p.cat == c) {
            let u = &p.usage;
            s.push_str(&format!("\n### {}\n\n{}\n\n", p.name, p.desc));
            s.push_str(&format!("- **Role:** {} ({}); **genres:** {}\n", u.role, u.family, u.genres.join(", ")));
            s.push_str(&format!("- **Range:** {} to {}; **play:** {}\n", note_name(u.lo), note_name(u.hi), u.play));
            s.push_str(&format!("- **Voices:** {} ({})\n", p.voice_class(), if p.is_mono() { format!("{} voice: one note at a time; for basslines, leads and single lines, never chords", p.voice_mode()) } else { "plays chords and overlapping notes".to_string() }));
            let labels: Vec<String> = (0..8).filter(|k| u.macros.get(*k).map_or(false, |m| !m.is_empty())).map(|k| format!("M{} {}", k + 1, u.macros[k])).collect();
            if !labels.is_empty() {
                s.push_str(&format!("- **Macros:** {}. {}\n", labels.join(", "), u.notes));
            }
            if !u.auto.is_empty() {
                let a: Vec<String> = u.auto.iter().map(|(m, mv)| format!("M{} {}", m + 1, mv.join(" / "))).collect();
                s.push_str(&format!("- **Per section (build / drop / breakdown):** {}\n", a.join("; ")));
            }
            if !u.pattern.is_empty() {
                s.push_str(&format!("- **Pattern:** {}\n", u.pattern));
            }
            if !u.switch.is_empty() {
                let sib: Vec<&str> = all().iter().filter(|q| q.usage.switch == u.switch && q.name != p.name).map(|q| q.name.as_str()).collect();
                if !sib.is_empty() {
                    s.push_str(&format!("- **Switch with:** {}\n", sib.join(", ")));
                }
            }
            if !u.mix.is_empty() {
                s.push_str(&format!("- **Mix:** {}\n", u.mix));
            }
        }
    }
    s
}
