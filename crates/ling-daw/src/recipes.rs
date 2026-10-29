//! Sound recipes: how an EDM production sound is made and used, as data the DAW can act on.
//!
//! A recipe is one channel of a track the way a producer builds it: the instrument and its parameters, the effect chain in order
//! (with why each effect is there), the mix (level, pan, width, mono below a corner, sidechain to the kick, reverb / delay), the
//! automation that moves it through the form, and how the part is *written* for that sound (register, rhythm patterns, note
//! lengths, chord voicing, sound switching across a set of patches, and where pitch movement is allowed).
//!
//! Recipes ship as JSON files in `assets/recipes/` (one file per family); users add their own `*.json` files to `recipes/` next
//! to the app (or point `LING_RECIPES` at folders, `;`-separated, which replaces the default folders: tests use it). A file
//! holds `{"recipes": [...]}`, a list, or one recipe object. Every name is checked against the code when it is loaded: unknown
//! instrument parameters, effects and effect parameters are dropped and reported, values are clamped to 0..1, a broken file or
//! recipe is reported and skipped - never fatal ([`Library::problems`]).
//!
//! Selection ([`rank`], [`pick`]): the part's role (bass, lead, chord, pad, arp, fx - from the song's role name), the genre (a
//! recipe lists the song generator's genre names it fits, plus style words like "riddim" or "colour bass" that the description
//! can ask for), description words (name, tags, styles), mood (valence / energy) and a priority. A sub-bass role only gets sub
//! recipes, a riser only riser recipes.
//!
//! Application ([`apply_sound`], [`write`]): the recipe REPLACES the track's instrument and effect chain (a recipe's chain is a
//! complete design, so the generic sound design of sounddesign.rs is not added on top of it); the genre's space rules still cap the
//! reverb / delay wetness and the instrument's own effects (space.rs), the pitch guard keeps every automation template off pitch
//! parameters unless the recipe is a riser or declares a scoop, and the expression rules (sounddesign::expression) still run:
//! glide only where the recipe's pitch block asks for slides (on the notes the writer overlaps), no LFO into pitch, fine offsets
//! snapped, delayed vibrato only on the leads of genres that have it. Effects a recipe declares on purpose (a frequency shifter
//! on a growl, a flanger on a comb growl, a vocoder carrier) are kept: they are the design, not an accident.
use crate::compose::Kind;
use crate::model::{AutoLane, EffectSlot, Note, Track};
use crate::sounddesign::{self as sd, Phase, Sec};
use crate::theory::{Chord, Key, QUAL};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// the roles a recipe can play
pub const ROLES: [&str; 6] = ["bass", "lead", "chord", "pad", "arp", "fx"];

// ── the schema ───────────────────────────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Mood {
    /// -1 dark / aggressive .. 1 bright / happy
    pub valence: f32,
    /// 0 calm .. 1 full on
    pub energy: f32,
}
impl Default for Mood {
    fn default() -> Self {
        Mood { valence: 0.0, energy: 0.7 }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct FxStep {
    /// an effect name as the effect list shows it ("OTT", "Split Drive" ...)
    pub effect: String,
    pub params: BTreeMap<String, f32>,
    /// the slot's dry / wet (1 = the effect only); None = 1
    pub wet: Option<f32>,
    pub why: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct AutoTpl {
    /// an instrument parameter ("cut"), "vol", "pan", "fx:<Effect>.<param>" / "fx:<Effect>.wet", or "master:vol"
    pub target: String,
    /// where it moves: phases ("build", "drop", "low" = intro / break, "mid", "out"), section tags, or "any"
    pub sections: Vec<String>,
    /// hold | ramp | ramp_exp | ramp_down | swell | sine | triangle | saw | square | steps | tail
    pub shape: String,
    /// beats per cycle of a periodic shape (and the length of a `tail`)
    pub period: f64,
    /// [from, to] in the target's 0..1 units (dB when `unit` is "db")
    pub range: Vec<f32>,
    /// the values of a `steps` shape, one per period, cycling
    pub steps: Vec<f32>,
    /// "" or "db" (vol targets: dB relative to the level)
    pub unit: String,
    /// beats of silence (0) at the end of a build: the breath before the drop
    pub end_gap: f64,
    pub why: String,
}
impl Default for AutoTpl {
    fn default() -> Self {
        AutoTpl { target: String::new(), sections: vec!["drop".into()], shape: "hold".into(), period: 0.0, range: vec![0.0, 1.0], steps: vec![], unit: String::new(), end_gap: 0.0, why: String::new() }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct PatternSpec {
    /// one character per grid step: `.` rest, `-` hold, `x` accent hit, `o` hit, `g` ghost hit, `^` octave up, `v` octave down,
    /// `5` fifth, `3` third, `7` seventh, `b` the scale step above the root, `n` the next arpeggio tone
    pub steps: String,
    /// "1/16", "1/8", "1/8T", "1/16T", "1/4", "1/8." or a number of beats
    pub grid: serde_json::Value,
    /// the phases it plays in (empty = all)
    pub phases: Vec<String>,
    /// a one-bar fill: plays on the last bar of every four
    pub fill: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum PatIn {
    Steps(String),
    Spec(PatternSpec),
}

/// a resolved pattern
#[derive(Clone, Debug, PartialEq)]
pub struct Pat {
    pub steps: Vec<char>,
    pub grid: f64,
    pub phases: Vec<String>,
    pub fill: bool,
}
impl Pat {
    pub fn beats(&self) -> f64 {
        self.steps.len() as f64 * self.grid
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Arp {
    /// up | down | updown | pingpong
    pub order: String,
    pub octaves: u32,
}
impl Default for Arp {
    fn default() -> Self {
        Arp { order: "up".into(), octaves: 1 }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Switching {
    /// recipe ids, A = the first (normally this recipe)
    pub set: Vec<String>,
    /// letters per unit, spaces ignored: "AABA ACBC"
    pub order: String,
    /// "bar" (each letter covers `bars` bars) or "hit" (each letter one hit)
    pub unit: String,
    pub bars: u32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct PitchUse {
    /// share of the eligible note pairs written legato so they glide (0 = none)
    pub slides: f32,
    /// the largest interval a slide spans (semitones)
    pub slide_max: u8,
    /// portamento time for those notes (milliseconds)
    pub glide_ms: f32,
    /// a scoop from this many semitones (negative = from below) into each chord entry: a per-note pitch gesture (0 = none)
    pub scoop: f32,
    /// a riser: pitch may sweep and bend (the pitch guard is off for this part)
    pub riser: bool,
    /// a dive of this many semitones on the last note of every four-bar phrase (the screech dive into a fill; 0 = none)
    pub dive: f32,
    /// the one-shot / last note rises this many semitones over its length (a per-note pitch riser; 0 = none)
    pub rise: f32,
    /// pitch movement designed into the patch itself (a pitch envelope knock or zap): a sentence for the UI
    pub designed: String,
    /// the genre's delayed vibrato on held lead notes (expression rules) may apply
    pub vibrato: bool,
}
impl Default for PitchUse {
    fn default() -> Self {
        PitchUse { slides: 0.0, slide_max: 7, glide_ms: 0.0, scoop: 0.0, riser: false, dive: 0.0, rise: 0.0, designed: String::new(), vibrato: true }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Composition {
    /// pattern | melody | hold | mirror | oneshot | none
    pub mode: String,
    /// [lowest, highest] MIDI note
    pub register: Vec<u8>,
    pub patterns: Vec<PatIn>,
    /// how much of its span a hit sounds (0..1, 1 = legato)
    pub gate: f32,
    /// single | octave | fifth | triad | seventh | add9 | open | sus2 | spread
    pub voicing: String,
    pub arp: Arp,
    pub switching: Option<Switching>,
    pub pitch: PitchUse,
    /// the length of a `oneshot` note (beats, 0 = the clip)
    pub hold_beats: f64,
    /// a patch parameter that follows the chord's quality (a growl's triad stack: minor or major), written as a lane
    pub quality_param: Option<QualityParam>,
    /// words for people: how it is played
    pub rhythm: String,
    pub note_lengths: String,
    pub voicing_notes: String,
    #[serde(skip)]
    pub pats: Vec<Pat>,
}
impl Default for Composition {
    fn default() -> Self {
        Composition {
            mode: "pattern".into(),
            register: vec![],
            patterns: vec![],
            gate: 0.9,
            voicing: "single".into(),
            arp: Arp::default(),
            switching: None,
            pitch: PitchUse::default(),
            hold_beats: 0.0,
            quality_param: None,
            rhythm: String::new(),
            note_lengths: String::new(),
            voicing_notes: String::new(),
            pats: vec![],
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct QualityParam {
    pub param: String,
    pub minor: f32,
    pub major: f32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Side {
    /// the Ducker's normalised settings (depth 1 = 30 dB)
    pub depth: f32,
    pub attack: f32,
    pub hold: f32,
    pub release: f32,
    pub curve: f32,
    /// the notes of the drum track that trigger it
    pub keys: String,
}
impl Default for Side {
    fn default() -> Self {
        Side { depth: 0.45, attack: 0.05, hold: 0.1, release: 0.2, curve: 0.35, keys: "35,36".into() }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Mix {
    pub level_db: f32,
    pub pan: f32,
    /// the final Imager's width (0.5 = as it is, 0 = mono, 1 = twice as wide); None = leave
    pub width: Option<f32>,
    /// everything below this is mono (0 = no rule)
    pub mono_below_hz: f32,
    /// duck under the kick (None = no sidechain)
    pub sidechain: Option<Side>,
    /// the reverb / delay insert mix the recipe wants (capped by the genre's space)
    pub reverb: f32,
    pub delay: f32,
    /// echo time in beats (0.75 = dotted eighth)
    pub delay_beats: f32,
}
impl Default for Mix {
    fn default() -> Self {
        Mix { level_db: -8.0, pan: 0.0, width: None, mono_below_hz: 0.0, sidechain: None, reverb: 0.0, delay: 0.0, delay_beats: 0.75 }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Recipe {
    pub id: String,
    pub name: String,
    /// bass | lead | chord | pad | arp | fx
    pub role: String,
    /// the song generator's genre names it fits
    pub genres: Vec<String>,
    /// sub-genre / style words a description can ask for ("riddim", "colour bass")
    pub styles: Vec<String>,
    /// sound words ("growl", "sub", "riser", "gated")
    pub tags: Vec<String>,
    pub mood: Mood,
    /// a small preference among recipes that fit equally (0 = none)
    pub priority: f32,
    pub description: String,
    /// the technique or genre style it follows (never an artist's preset)
    pub inspired_by: String,
    pub techniques: Vec<String>,
    /// growl | prism | synth | chip | pluck | ... or "none" (a master move)
    pub instrument: String,
    /// a patch file to start from (loaded through the patch loader), then `params` on top
    pub base_patch: String,
    pub params: BTreeMap<String, f32>,
    pub fx_chain: Vec<FxStep>,
    pub automation: Vec<AutoTpl>,
    pub composition: Composition,
    pub mix: Mix,
    pub sources: Vec<String>,
    pub notes: String,
    #[serde(skip)]
    pub file: String,
}

impl Recipe {
    pub fn is_master(&self) -> bool {
        self.instrument == "none"
    }
    pub fn has_tag(&self, t: &str) -> bool {
        self.tags.iter().any(|x| x == t)
    }
    pub fn register(&self) -> (u8, u8) {
        let r = &self.composition.register;
        if r.len() >= 2 && r[0] <= r[1] {
            (r[0], r[1])
        } else {
            default_register(&self.role)
        }
    }
    /// is this a riser / sweep (pitch may move)?
    pub fn riser(&self) -> bool {
        self.composition.pitch.riser || self.has_tag("riser")
    }
    /// is it meant to be heard as a pitch (tune-checked)?
    pub fn pitched(&self) -> bool {
        !self.riser() && !self.has_tag("noise") && !self.has_tag("impact") && !self.is_master()
    }
}

fn default_register(role: &str) -> (u8, u8) {
    match role {
        "bass" => (28, 48),
        "lead" => (60, 84),
        "chord" => (52, 79),
        "pad" => (48, 84),
        "arp" => (60, 88),
        _ => (48, 72),
    }
}

/// a grid name or number -> beats
pub fn parse_grid(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64().filter(|x| *x > 0.01 && *x <= 8.0),
        serde_json::Value::String(s) => {
            let s = s.trim().to_lowercase();
            let (base, mul) = if let Some(b) = s.strip_suffix('t') {
                (b.to_string(), 2.0 / 3.0)
            } else if let Some(b) = s.strip_suffix('.') {
                (b.to_string(), 1.5)
            } else {
                (s.clone(), 1.0)
            };
            let (a, b) = base.split_once('/')?;
            let (a, b): (f64, f64) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
            (b > 0.0).then(|| 4.0 * a / b * mul)
        }
        serde_json::Value::Null => Some(0.25),
        _ => None,
    }
}

const STEP_CHARS: &str = ".-xog^v537bn";

// ── loading and validation ───────────────────────────────────────────────────────────────────────────────────────────

/// every recipe that loaded, and what was wrong (file: problem)
#[derive(Clone, Debug, Default)]
pub struct Library {
    pub recipes: Vec<Recipe>,
    pub problems: Vec<String>,
    pub dirs: Vec<String>,
}

impl Library {
    pub fn get(&self, id: &str) -> Option<&Recipe> {
        let id = id.trim();
        self.recipes.iter().find(|r| r.id == id).or_else(|| self.recipes.iter().find(|r| r.id.eq_ignore_ascii_case(id) || r.name.eq_ignore_ascii_case(id)))
    }
}

/// the folders recipes are read from: `LING_RECIPES` (`;`-separated) or the app's `assets/recipes` + `recipes`
pub fn recipe_dirs() -> Vec<PathBuf> {
    if let Ok(v) = std::env::var("LING_RECIPES") {
        if !v.trim().is_empty() {
            return v.split(';').map(|s| PathBuf::from(s.trim())).filter(|p| !p.as_os_str().is_empty()).collect();
        }
    }
    let mut out = vec![];
    for d in ["assets/recipes", "../LingStationX/assets/recipes", "../../../LingStationX/assets/recipes"] {
        if Path::new(d).is_dir() {
            out.push(PathBuf::from(d));
            break;
        }
    }
    if Path::new("recipes").is_dir() {
        out.push(PathBuf::from("recipes"));
    }
    out
}

fn cache() -> &'static Mutex<Option<Arc<Library>>> {
    static L: OnceLock<Mutex<Option<Arc<Library>>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(None))
}

/// the loaded library (read on first use)
pub fn library() -> Arc<Library> {
    let mut g = cache().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(l) = g.as_ref() {
        return l.clone();
    }
    let l = Arc::new(load_from(&recipe_dirs()));
    *g = Some(l.clone());
    l
}

/// read the folders again (after a user edited a file)
pub fn reload() -> Arc<Library> {
    let l = Arc::new(load_from(&recipe_dirs()));
    *cache().lock().unwrap_or_else(|p| p.into_inner()) = Some(l.clone());
    l
}

pub fn load_from(dirs: &[PathBuf]) -> Library {
    let mut lib = Library { dirs: dirs.iter().map(|d| d.to_string_lossy().replace('\\', "/")).collect(), ..Default::default() };
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(d) else {
            lib.problems.push(format!("{}: not a folder", d.display()));
            continue;
        };
        let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().map(|e| e.eq_ignore_ascii_case("json")).unwrap_or(false)).collect();
        files.sort();
        for f in files {
            let name = f.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            match std::fs::read_to_string(&f) {
                Ok(text) => {
                    let (rs, probs) = parse_text(&text, &name);
                    lib.problems.extend(probs);
                    for r in rs {
                        if lib.recipes.iter().any(|x| x.id == r.id) {
                            lib.problems.push(format!("{name}: recipe '{}' is defined twice (the first one is kept)", r.id));
                        } else {
                            lib.recipes.push(r);
                        }
                    }
                }
                Err(e) => lib.problems.push(format!("{name}: {e}")),
            }
        }
    }
    // a switching set names recipes that must exist
    let ids: Vec<String> = lib.recipes.iter().map(|r| r.id.clone()).collect();
    for r in lib.recipes.iter_mut() {
        if let Some(sw) = r.composition.switching.as_mut() {
            let before = sw.set.len();
            let missing: Vec<String> = sw.set.iter().filter(|s| !ids.contains(s)).cloned().collect();
            sw.set.retain(|s| ids.contains(s));
            if sw.set.len() < before {
                lib.problems.push(format!("{}: {}: switching set names unknown recipes {}", r.file, r.id, missing.join(", ")));
            }
            if sw.set.len() < 2 {
                r.composition.switching = None;
            }
        }
    }
    lib
}

/// a file's text -> its recipes (each validated) and the problems found
pub fn parse_text(text: &str, file: &str) -> (Vec<Recipe>, Vec<String>) {
    let v: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => return (vec![], vec![format!("{file}: not valid JSON ({e})")]),
    };
    let items: Vec<serde_json::Value> = match v {
        serde_json::Value::Array(a) => a,
        serde_json::Value::Object(ref o) if o.contains_key("recipes") => o.get("recipes").and_then(|r| r.as_array()).cloned().unwrap_or_default(),
        serde_json::Value::Object(_) => vec![v],
        _ => return (vec![], vec![format!("{file}: expected a recipe object, a list or {{\"recipes\": [...]}}")]),
    };
    let mut out = vec![];
    let mut probs = vec![];
    for (i, it) in items.into_iter().enumerate() {
        match serde_json::from_value::<Recipe>(it) {
            Ok(mut r) => {
                r.file = file.to_string();
                let (ok, p) = validate(r);
                probs.extend(p);
                if let Some(r) = ok {
                    out.push(r);
                }
            }
            Err(e) => probs.push(format!("{file}: recipe #{}: {e}", i + 1)),
        }
    }
    (out, probs)
}

/// the parameter names of an instrument kind
pub fn inst_params(kind: &str) -> Vec<String> {
    crate::dsp::make_instrument(kind, 44100.0).map(|i| i.params().into_iter().map(|(n, _)| n.to_string()).collect()).unwrap_or_default()
}

/// Check a recipe against the code: (the recipe, cleaned; None when it cannot be used) and the problems
pub fn validate(mut r: Recipe) -> (Option<Recipe>, Vec<String>) {
    let mut p: Vec<String> = vec![];
    let at = format!("{}: {}", r.file, if r.id.is_empty() { "(no id)" } else { &r.id });
    if r.id.trim().is_empty() {
        p.push(format!("{at}: a recipe needs an \"id\""));
        return (None, p);
    }
    if r.name.is_empty() {
        r.name = r.id.replace('_', " ");
    }
    r.role = r.role.trim().to_lowercase();
    if !ROLES.contains(&r.role.as_str()) {
        p.push(format!("{at}: role '{}' is not one of {}", r.role, ROLES.join(", ")));
        return (None, p);
    }
    for g in &r.genres {
        if !crate::songgen::GENRES.iter().any(|x| x.name == g) {
            p.push(format!("{at}: genre '{g}' is not a song generator genre (put style words in \"styles\")"));
        }
    }
    r.instrument = r.instrument.trim().to_lowercase();
    let names = if r.is_master() { vec![] } else { inst_params(&r.instrument) };
    if !r.is_master() && names.is_empty() {
        p.push(format!("{at}: unknown instrument '{}'", r.instrument));
        return (None, p);
    }
    if !r.base_patch.is_empty() && !Path::new(&r.base_patch).is_file() {
        // relative to the app folder; a test run from the crate cannot see it, so this is only a note
        let alt = Path::new("../../../LingStationX").join(&r.base_patch);
        if !alt.is_file() {
            p.push(format!("{at}: base patch '{}' not found (the parameters alone are used)", r.base_patch));
        }
    }
    let mut params = BTreeMap::new();
    for (k, v) in std::mem::take(&mut r.params) {
        let k2 = if k.contains('.') { k.clone() } else { k.replace('_', " ") };
        if !names.iter().any(|n| *n == k2) {
            p.push(format!("{at}: '{}' has no parameter '{k}' (dropped)", r.instrument));
            continue;
        }
        if !v.is_finite() || !(0.0..=1.0).contains(&v) {
            p.push(format!("{at}: {k}={v} is outside 0..1 (clamped)"));
        }
        params.insert(k2, if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.5 });
    }
    r.params = params;
    let mut chain = vec![];
    for mut f in std::mem::take(&mut r.fx_chain) {
        let defs = crate::dsp::fx::defaults(&f.effect);
        if defs.is_empty() || !crate::dsp::fx::EFFECT_NAMES.contains(&f.effect.as_str()) {
            p.push(format!("{at}: unknown effect '{}' (dropped)", f.effect));
            continue;
        }
        let mut ps = BTreeMap::new();
        for (k, v) in std::mem::take(&mut f.params) {
            let k2 = k.replace('_', " ");
            if !defs.iter().any(|d| d.0 == k2) {
                p.push(format!("{at}: {} has no parameter '{k}' (dropped)", f.effect));
                continue;
            }
            if !v.is_finite() || !(0.0..=1.0).contains(&v) {
                p.push(format!("{at}: {} {k}={v} is outside 0..1 (clamped)", f.effect));
            }
            ps.insert(k2, if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.5 });
        }
        f.params = ps;
        if let Some(w) = f.wet {
            f.wet = Some(if w.is_finite() { w.clamp(0.0, 1.0) } else { 1.0 });
        }
        chain.push(f);
    }
    r.fx_chain = chain;
    // automation: real targets, the pitch guard
    let riser = r.riser();
    let scoop = r.composition.pitch.scoop != 0.0;
    let mut autos = vec![];
    for a in std::mem::take(&mut r.automation) {
        let t = a.target.trim().to_string();
        let ok = if t == "vol" || t == "pan" || t == "master:vol" {
            true
        } else if let Some(rest) = t.strip_prefix("fx:") {
            match rest.rsplit_once('.') {
                Some((fxn, prm)) => r.fx_chain.iter().any(|f| f.effect == fxn) && (prm == "wet" || crate::dsp::fx::defaults(fxn).iter().any(|d| d.0 == prm)),
                None => false,
            }
        } else {
            names.iter().any(|n| *n == t)
        };
        if !ok {
            p.push(format!("{at}: automation target '{t}' does not exist on this recipe (dropped)"));
            continue;
        }
        if crate::redesign::pitch_altering(&t) && !riser && !(scoop && t == "bend") {
            p.push(format!("{at}: automation on '{t}' moves the pitch: only a riser or a declared scoop may (dropped)"));
            continue;
        }
        if a.range.len() < 2 && a.shape != "steps" {
            p.push(format!("{at}: automation on '{t}' needs a range [from, to] (dropped)"));
            continue;
        }
        autos.push(a);
    }
    r.automation = autos;
    // composition
    let c = &mut r.composition;
    if !matches!(c.mode.as_str(), "pattern" | "melody" | "hold" | "mirror" | "oneshot" | "none") {
        p.push(format!("{at}: composition mode '{}' unknown (pattern used)", c.mode));
        c.mode = "pattern".into();
    }
    if c.register.len() >= 2 && (c.register[0] > c.register[1] || c.register[1] > 127) {
        p.push(format!("{at}: register {:?} is not [low, high] (the role's default used)", c.register));
        c.register.clear();
    }
    if !matches!(c.voicing.as_str(), "single" | "octave" | "fifth" | "triad" | "seventh" | "add9" | "open" | "sus2" | "spread") {
        p.push(format!("{at}: voicing '{}' unknown (single used)", c.voicing));
        c.voicing = "single".into();
    }
    c.gate = if c.gate.is_finite() { c.gate.clamp(0.05, 1.0) } else { 0.9 };
    let mut pats = vec![];
    for (i, pi) in c.patterns.iter().enumerate() {
        let spec = match pi {
            PatIn::Steps(s) => PatternSpec { steps: s.clone(), grid: serde_json::Value::Null, ..Default::default() },
            PatIn::Spec(s) => s.clone(),
        };
        let steps: Vec<char> = spec.steps.chars().filter(|c| !c.is_whitespace() && *c != '|').collect();
        if steps.is_empty() || steps.iter().any(|ch| !STEP_CHARS.contains(*ch)) {
            p.push(format!("{at}: pattern #{} has a step that is not one of '{STEP_CHARS}' (dropped)", i + 1));
            continue;
        }
        let Some(grid) = parse_grid(&spec.grid) else {
            p.push(format!("{at}: pattern #{} grid {} is not like \"1/16\", \"1/8T\" or beats (dropped)", i + 1, spec.grid));
            continue;
        };
        pats.push(Pat { steps, grid, phases: spec.phases.iter().map(|x| x.to_lowercase()).collect(), fill: spec.fill });
    }
    c.pats = pats;
    if let Some(qp) = &c.quality_param {
        if !names.iter().any(|n| *n == qp.param) || crate::redesign::pitch_altering(&qp.param) {
            p.push(format!("{at}: quality_param '{}' is not a usable parameter (dropped)", qp.param));
            c.quality_param = None;
        }
    }
    if c.mode == "pattern" && c.pats.is_empty() {
        p.push(format!("{at}: pattern mode without a usable pattern (the source notes are re-pitched instead)"));
        c.mode = "mirror".into();
    }
    if let Some(sw) = &mut c.switching {
        if sw.bars == 0 {
            sw.bars = 1;
        }
        if sw.unit.is_empty() {
            sw.unit = "bar".into();
        }
        if !sw.set.first().map(|s| *s == r.id).unwrap_or(false) {
            sw.set.insert(0, r.id.clone());
        }
    }
    let m = &mut r.mix;
    if !m.level_db.is_finite() || m.level_db > 6.0 || m.level_db < -60.0 {
        p.push(format!("{at}: level {} dB is outside -60..6", m.level_db));
        m.level_db = m.level_db.clamp(-60.0, 6.0);
    }
    m.pan = m.pan.clamp(-1.0, 1.0);
    m.reverb = m.reverb.clamp(0.0, 1.0);
    m.delay = m.delay.clamp(0.0, 1.0);
    (Some(r), p)
}

// ── selection ────────────────────────────────────────────────────────────────────────────────────────────────────────

/// the recipe role for a song generator role ("growl bass" -> bass, "supersaw lead" -> lead, "pluck" -> arp, "riser" -> fx)
pub fn recipe_role(song_role: &str) -> &'static str {
    let r = song_role.trim().to_lowercase();
    if let Some(x) = ROLES.iter().find(|x| **x == r) {
        return x;
    }
    // played instruments and voices are not synth recipes
    if ["piano", "keys", "organ", "guitar", "strings", "brass", "woodwind", "choir", "bell", "voice", "vocal", "drums"].iter().any(|w| r.split(' ').any(|x| x == *w)) {
        return "";
    }
    if r.contains("riser") || r.contains("sweep") || r.contains("impact") || r == "fx" || r.contains("sfx") {
        return "fx";
    }
    if r.contains("chord") || r.contains("stab") {
        return "chord";
    }
    match crate::songgen::role_kind(&r) {
        Kind::Bass => "bass",
        Kind::Melody => "lead",
        Kind::Pad => "pad",
        Kind::Arp => "arp",
        Kind::Chords => "chord",
        Kind::Drums => "",
    }
}

fn words_of(s: &str) -> Vec<String> {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 1).map(|w| w.to_string()).collect()
}

/// how well a recipe's genres fit a song genre: 1 = listed, 0.8 = a style word of that genre, 0 = no
pub fn genre_fit(r: &Recipe, genre: &str) -> f64 {
    let g = genre.trim().to_lowercase();
    if g.is_empty() {
        return 0.5;
    }
    if r.genres.iter().any(|x| *x == g) {
        return 1.0;
    }
    let words: &[&str] = crate::songgen::GENRES.iter().find(|x| x.name == g).map(|x| x.words).unwrap_or(&[]);
    if r.styles.iter().any(|s| words.contains(&s.as_str()) || *s == g) {
        return 0.8;
    }
    0.0
}

/// the mood words ask for: (valence -1..1 or None, energy 0..1 or None)
pub fn mood_of(words: &str) -> (Option<f32>, Option<f32>) {
    let w = words_of(words);
    let has = |xs: &[&str]| w.iter().any(|x| xs.contains(&x.as_str()));
    let mut v: Option<f32> = None;
    if has(&["dark", "darker", "aggressive", "angry", "heavy", "evil", "menacing", "filthy", "dirty", "nasty", "brutal", "tearout"]) {
        v = Some(-0.7);
    }
    if has(&["happy", "bright", "uplifting", "euphoric", "cute", "kawaii", "emotional", "melodic", "hopeful", "dreamy", "warm"]) {
        v = Some(v.map(|x| x + 0.9).unwrap_or(0.6));
    }
    if v.is_none() {
        let (lv, _) = crate::emotion::lyric_lean(words);
        if lv.abs() > 0.05 {
            v = Some((lv * 2.0).clamp(-1.0, 1.0));
        }
    }
    let e = if has(&["calm", "soft", "chill", "gentle", "ambient", "quiet", "intro"]) {
        Some(0.3)
    } else if has(&["hard", "heavy", "big", "huge", "massive", "drop", "energetic", "intense", "loud"]) {
        Some(0.95)
    } else {
        None
    };
    (v, e)
}

/// Score a recipe for a part: None when it does not fit the role (or the sub / riser rules). `role` is the song's role name or a
/// recipe role; `words` the description / request; `energy` 0..1 (negative = not known).
pub fn score(r: &Recipe, role: &str, genre: &str, words: &str, energy: f64) -> Option<f64> {
    if r.is_master() {
        return None;
    }
    let want = recipe_role(role);
    if want.is_empty() || r.role != want {
        return None;
    }
    let rl = role.to_lowercase();
    let ws = words_of(&format!("{words} {role}"));
    let has = |w: &str| ws.iter().any(|x| x == w);
    // a sub role gets a sub; a riser role a riser; an impact only when asked
    let sub_role = rl.contains("sub") || rl.contains("808");
    let is_sub = r.has_tag("sub") || r.has_tag("808");
    if want == "bass" && sub_role != is_sub && !(is_sub && (has("sub") || has("808"))) {
        return None;
    }
    if want == "fx" && r.has_tag("impact") && !(has("impact") || has("boom") || has("hit")) {
        return None;
    }
    if want == "fx" && rl.contains("riser") && !r.has_tag("riser") {
        return None;
    }
    // a soundscape (Ling Cinema's ambience) only for a texture part or when the words ask for an atmosphere
    if r.has_tag("soundscape") && !(rl.contains("texture") || ["ambience", "soundscape", "drone", "atmosphere", "atmospheric"].into_iter().any(|w| has(w))) {
        return None;
    }
    let mut s = 4.0 * genre_fit(r, genre);
    let id_words = words_of(&format!("{} {}", r.id, r.name));
    const GENERIC: [&str; 12] = ["bass", "lead", "pad", "chord", "chords", "arp", "the", "a", "an", "and", "make", "use"];
    // the part's own name ("wobble bass") counts more than the song's description ("riddim dubstep")
    let role_ws = words_of(role);
    for (w, k) in words_of(words).into_iter().map(|w| (w, 1.0)).chain(role_ws.into_iter().map(|w| (w, 1.5))) {
        if GENERIC.contains(&w.as_str()) {
            continue;
        }
        if r.styles.iter().any(|x| *x == w) {
            s += 3.0 * k;
        } else if r.tags.iter().any(|x| *x == w) {
            s += 2.5 * k;
        } else if id_words.contains(&w) {
            s += 1.5 * k;
        }
    }
    // multi-word styles ("colour bass", "melodic dubstep")
    let low = words.to_lowercase();
    for st in r.styles.iter().filter(|x| x.contains(' ')) {
        if low.contains(st.as_str()) {
            s += 2.0;
        }
    }
    let (v, e) = mood_of(words);
    if let Some(v) = v {
        s += 1.5 * (1.0 - (v - r.mood.valence).abs() as f64 / 2.0);
    }
    let e = e.map(|x| x as f64).or(if energy >= 0.0 { Some(energy) } else { None });
    if let Some(e) = e {
        s += 1.0 - (e - r.mood.energy as f64).abs();
    }
    Some(s + r.priority as f64)
}

/// every recipe that fits, best first
pub fn rank<'a>(lib: &'a Library, role: &str, genre: &str, words: &str, energy: f64) -> Vec<(f64, &'a Recipe)> {
    let mut v: Vec<(f64, &Recipe)> = lib.recipes.iter().filter_map(|r| score(r, role, genre, words, energy).map(|s| (s, r))).collect();
    v.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.id.cmp(&b.1.id)));
    v
}

/// The recipe for a part of a song, when one fits its genre (the song build and Medley use this): the best few within a point of
/// the top, chosen by the seed; `avoid` are recipes other parts already took.
pub fn pick<'a>(lib: &'a Library, role: &str, genre: &str, words: &str, energy: f64, seed: u64, avoid: &[String]) -> Option<&'a Recipe> {
    let ranked: Vec<(f64, &Recipe)> = rank(lib, role, genre, words, energy).into_iter().filter(|(_, r)| genre_fit(r, genre) > 0.0 && !avoid.contains(&r.id)).collect();
    let top = ranked.first()?.0;
    let close: Vec<&Recipe> = ranked.iter().filter(|(s, _)| *s >= top - 1.0).map(|(_, r)| *r).collect();
    let mut rng = crate::compose::Rng::new(seed ^ 0x5EC1_9E5);
    Some(close[(rng.next() % close.len() as u64) as usize])
}

/// the master moves (instrument "none") that fit a genre
pub fn master_moves<'a>(lib: &'a Library, genre: &str) -> Vec<&'a Recipe> {
    lib.recipes.iter().filter(|r| r.is_master() && genre_fit(r, genre) > 0.0).collect()
}

// ── the sound on a track ─────────────────────────────────────────────────────────────────────────────────────────────

/// what a recipe is applied with
#[derive(Clone, Debug)]
pub struct Ctx {
    pub genre: String,
    pub bpm: f64,
    pub secs: Vec<Sec>,
    pub sr: f32,
    /// the song's role name for the part ("" = the recipe's role)
    pub role: String,
}

/// the song-role name expression rules know for a recipe
pub fn expr_role(r: &Recipe) -> String {
    match r.role.as_str() {
        "bass" => {
            if r.has_tag("sub") || r.has_tag("808") {
                "sub bass".into()
            } else if r.has_tag("reese") {
                "reese bass".into()
            } else if r.has_tag("wobble") {
                "wobble bass".into()
            } else {
                "growl bass".into()
            }
        }
        "lead" => if r.has_tag("supersaw") { "supersaw lead".into() } else { "synth lead".into() },
        "chord" => "stab chord".into(),
        "pad" => "warm pad".into(),
        "arp" => "arp".into(),
        _ => if r.riser() { "riser".into() } else { "fx".into() },
    }
}

/// the recipe's instrument: the base patch through the patch loader (with its level trim), then the recipe's parameters
pub fn build_instrument(r: &Recipe, sr: f32, bpm: f64) -> Option<Box<dyn crate::dsp::Instrument>> {
    if r.is_master() {
        return None;
    }
    let base = [PathBuf::from(&r.base_patch), Path::new("../../../LingStationX").join(&r.base_patch)].into_iter().find(|p| !r.base_patch.is_empty() && p.is_file());
    let mut inst = match base.and_then(|p| std::fs::read_to_string(p).ok()) {
        Some(text) => {
            let pd = crate::patch::parse(&text);
            if pd.kind == r.instrument { crate::patch::build(&pd, sr) } else { None }
        }
        None => None,
    }
    .or_else(|| crate::dsp::make_instrument(&r.instrument, sr))?;
    for (k, v) in &r.params {
        inst.set_param(k, *v);
    }
    if matches!(r.instrument.as_str(), "growl" | "prism") {
        inst.set_param("tempo", ((bpm as f32 - 60.0) / 140.0).clamp(0.0, 1.0));
    }
    Some(inst)
}

fn slot_set(slot: &mut EffectSlot, k: &str, v: f32) {
    if let Some(i) = slot.params.iter().position(|p| p.0 == k) {
        slot.set(i, v);
    }
}

/// the Imager's `mono below` value for a corner in Hz (20 x 2^(5 v))
pub fn mono_below_param(hz: f32) -> f32 {
    if hz <= 20.0 {
        0.0
    } else {
        ((hz / 20.0).log2() / 5.0).clamp(0.03, 1.0)
    }
}

/// the linear track volume for a level in dB
pub fn level_volume(db: f32) -> f32 {
    10f32.powf(db / 20.0).clamp(0.0, 1.5)
}

/// Put a recipe's sound on a track: instrument, effect chain in order, mix, the space's limits, automation over the sections.
/// The track's previous chain and its automation on effects / instrument parameters go (vol / pan lanes stay). Returns what was
/// done, in words.
pub fn apply_sound(tr: &mut Track, r: &Recipe, c: &Ctx) -> Vec<String> {
    let mut said = vec![];
    let Some(inst) = build_instrument(r, c.sr, c.bpm) else { return said };
    tr.inst = Some(inst);
    tr.inst_kind = r.instrument.clone();
    tr.patch = if r.base_patch.is_empty() { String::new() } else { r.base_patch.clone() };
    tr.auto.retain(|l| l.param == "vol" || l.param == "pan");
    // the chain: the recipe's effects in order, its room and echo (capped by the genre's space), a final Imager for the mono low
    // end and the width, the Ducker last
    let genre = if c.genre.is_empty() { r.genres.first().cloned().unwrap_or_default() } else { c.genre.clone() };
    let sp = crate::space::profile(&genre);
    let srole = crate::space::SRole::from_plan(&expr_role(r), crate::songgen::role_kind(&expr_role(r)));
    let rs = sp.role(srole);
    let mut fx: Vec<EffectSlot> = vec![];
    let mut ducker: Option<EffectSlot> = None;
    for f in &r.fx_chain {
        let Some(mut slot) = EffectSlot::new(&f.effect, c.sr) else { continue };
        for (k, v) in &f.params {
            slot_set(&mut slot, k, *v);
        }
        if let Some(w) = f.wet {
            slot.wet = w;
        }
        if f.effect == "Ducker" {
            ducker = Some(slot);
        } else {
            fx.push(slot);
        }
    }
    let cap = |want: f32, space: f32| want.min(space * 1.6 + 0.06).max(0.0);
    let verb = cap(r.mix.reverb, rs.reverb);
    if verb >= 0.02 && !fx.iter().any(|e| e.name == "Reverb") {
        if let Some(mut slot) = EffectSlot::new("Reverb", c.sr) {
            slot_set(&mut slot, "mix", verb);
            slot_set(&mut slot, "size", rs.size.max(0.3));
            slot_set(&mut slot, "damp", rs.damp);
            fx.push(slot);
        }
    }
    if verb < r.mix.reverb - 0.01 {
        said.push(format!("reverb {:.2} (the {genre} space caps {:.2})", verb, r.mix.reverb));
    }
    let echo = cap(r.mix.delay, rs.delay.max(0.04));
    if echo >= 0.02 && !fx.iter().any(|e| e.name == "Delay") {
        if let Some(mut slot) = EffectSlot::new("Delay", c.sr) {
            slot_set(&mut slot, "time", crate::space::Space::delay_time_param(r.mix.delay_beats.max(0.125), c.bpm));
            slot_set(&mut slot, "feedback", rs.feedback.clamp(0.15, 0.45));
            slot_set(&mut slot, "mix", echo);
            fx.push(slot);
        }
    }
    if r.mix.mono_below_hz > 20.0 || r.mix.width.is_some() {
        let last_is_imager = fx.last().map(|e| e.name == "Imager").unwrap_or(false);
        if !last_is_imager {
            if let Some(slot) = EffectSlot::new("Imager", c.sr) {
                fx.push(slot);
            }
        }
        if let Some(im) = fx.last_mut() {
            slot_set(im, "width", r.mix.width.unwrap_or(0.5).clamp(0.0, 1.0));
            slot_set(im, "mono below", mono_below_param(r.mix.mono_below_hz));
        }
    }
    if let Some(side) = &r.mix.sidechain {
        let mut slot = ducker.take().or_else(|| EffectSlot::new("Ducker", c.sr));
        if let Some(s) = slot.as_mut() {
            if !r.fx_chain.iter().any(|f| f.effect == "Ducker") {
                for (k, v) in [("depth", side.depth), ("attack", side.attack), ("hold", side.hold), ("release", side.release), ("curve", side.curve)] {
                    slot_set(s, k, v);
                }
            }
        }
        ducker = slot;
    }
    if let Some(d) = ducker {
        fx.push(d);
    }
    tr.effects = fx;
    tr.volume = level_volume(r.mix.level_db);
    tr.pan = r.mix.pan;
    // the instrument's own room, echo and modulation stay within the genre's space (pitch parameters stay as the recipe set them)
    if let Some(i) = tr.inst.as_mut() {
        let tw = crate::space::inst_tweaks(&sp, srole, &r.instrument, r.riser());
        let n = crate::space::apply_tweaks(i.as_mut(), &tw);
        if n > 0 {
            said.push(format!("{n} of the instrument's own effects limited to the {genre} space"));
        }
    }
    let lanes = render_automation(tr, r, &c.secs, None);
    if lanes > 0 {
        said.push(format!("{lanes} automation lane(s)"));
    }
    said
}

/// Render the recipe's automation templates into lanes on the track (and the master when `master` is given). Returns the lanes made.
pub fn render_automation(tr: &mut Track, r: &Recipe, secs: &[Sec], mut master: Option<(&mut Vec<AutoLane>, f32)>) -> usize {
    let mut n = 0;
    for a in &r.automation {
        let t = a.target.as_str();
        if t == "master:vol" {
            if let Some((ml, mv)) = master.as_mut() {
                let pts = lane_points(a, secs, *mv);
                if pts.len() > 1 {
                    ml.retain(|l| l.param != "vol");
                    ml.push(AutoLane { param: "vol".into(), points: pts });
                    n += 1;
                }
            }
            continue;
        }
        let (key, base) = if t == "vol" {
            ("vol".to_string(), tr.volume)
        } else if t == "pan" {
            ("pan".to_string(), tr.pan)
        } else if let Some(rest) = t.strip_prefix("fx:") {
            let Some((fxn, prm)) = rest.rsplit_once('.') else { continue };
            let Some(fi) = tr.effects.iter().position(|e| e.name == fxn) else { continue };
            if prm == "wet" {
                (format!("fx{fi}.wet"), tr.effects[fi].wet)
            } else {
                let Some(pi) = tr.effects[fi].params.iter().position(|p| p.0 == prm) else { continue };
                (format!("fx{fi}.{pi}"), tr.effects[fi].params[pi].1)
            }
        } else {
            let Some(v) = tr.inst.as_ref().and_then(|i| i.params().into_iter().find(|(nm, _)| *nm == t).map(|x| x.1)) else { continue };
            (t.to_string(), v)
        };
        let pts = lane_points(a, secs, base);
        if pts.len() > 1 {
            tr.auto.retain(|l| l.param != key);
            tr.auto.push(AutoLane { param: key, points: pts });
            n += 1;
        }
    }
    n
}

fn phase_name(p: Phase) -> &'static str {
    match p {
        Phase::Low => "low",
        Phase::Mid => "mid",
        Phase::Build => "build",
        Phase::Drop => "drop",
        Phase::Out => "out",
    }
}

fn section_matches(want: &[String], s: &Sec) -> bool {
    let ph = phase_name(sd::phase(&s.tag));
    want.iter().any(|w| w == "any" || w == ph || *w == s.tag)
}

/// The points of one template over the sections: the template's shape in every section it names, the base value elsewhere.
pub fn lane_points(a: &AutoTpl, secs: &[Sec], base: f32) -> Vec<(f64, f32)> {
    let db = a.unit == "db";
    let conv = |v: f32| -> f32 {
        if db {
            (base * 10f32.powf(v / 20.0)).clamp(0.0, 2.0)
        } else {
            v.clamp(0.0, 1.0)
        }
    };
    let (r0, r1) = (a.range.first().copied().unwrap_or(0.0), a.range.get(1).copied().unwrap_or(1.0));
    let mut pts: Vec<(f64, f32)> = vec![(0.0, base)];
    let push = |pts: &mut Vec<(f64, f32)>, b: f64, v: f32| {
        let b = b.max(0.0);
        if let Some(last) = pts.last_mut() {
            if b <= last.0 + 1e-4 {
                if (b - last.0).abs() < 1e-4 {
                    last.1 = v;
                }
                return;
            }
        }
        pts.push((b, v));
    };
    let hit: Vec<bool> = secs.iter().map(|s| section_matches(&a.sections, s) && s.end - s.start >= 0.05).collect();
    for (si, s) in secs.iter().enumerate() {
        if !hit[si] {
            continue;
        }
        let (st, en) = (s.start, s.end);
        // the base value holds up to the section (unless the one before moved too)
        let joined = si > 0 && hit[si - 1] && (secs[si - 1].end - st).abs() < 0.02;
        if !joined && st > 0.02 {
            push(&mut pts, st - 0.01, base);
        }
        let gap = if sd::phase(&s.tag) == Phase::Build { a.end_gap.clamp(0.0, (en - st) * 0.5) } else { 0.0 };
        let body = en - gap;
        let len = body - st;
        let per = if a.period > 0.01 { a.period } else { len };
        match a.shape.as_str() {
            "ramp" | "ramp_exp" | "ramp_down" => {
                let (from, to) = if a.shape == "ramp_down" { (r1, r0) } else { (r0, r1) };
                push(&mut pts, st, conv(from));
                if a.shape == "ramp_exp" {
                    for k in 1..8 {
                        let x = k as f64 / 8.0;
                        push(&mut pts, st + len * x, conv(from + (to - from) * (x * x) as f32));
                    }
                }
                push(&mut pts, body - 0.01, conv(to));
            }
            "swell" => {
                push(&mut pts, st, conv(r0));
                push(&mut pts, st + len * 0.5, conv(r1));
                push(&mut pts, body - 0.01, conv(r0));
            }
            "tail" => {
                // the last `period` beats at `to`, the rest at `from`
                let p = per.min(len);
                push(&mut pts, st, conv(r0));
                push(&mut pts, body - p - 0.01, conv(r0));
                push(&mut pts, body - p, conv(r1));
                push(&mut pts, body - 0.01, conv(r1));
            }
            "steps" => {
                let vals: Vec<f32> = if a.steps.is_empty() { vec![r0, r1] } else { a.steps.clone() };
                let mut b = st;
                let mut k = 0;
                while b < body - 1e-6 && k < 4096 {
                    let v = conv(vals[k % vals.len()]);
                    push(&mut pts, b, v);
                    push(&mut pts, (b + per).min(body) - 0.01, v);
                    b += per;
                    k += 1;
                }
            }
            "sine" | "triangle" | "saw" | "square" => {
                let mut b = st;
                let mut k = 0;
                while b < body - 1e-6 && k < 4096 {
                    let e = (b + per).min(body);
                    match a.shape.as_str() {
                        "sine" => {
                            for i in 0..8 {
                                let x = i as f64 / 8.0;
                                let y = 0.5 - 0.5 * (x * std::f64::consts::TAU).cos();
                                push(&mut pts, b + per * x, conv(r0 + (r1 - r0) * y as f32));
                            }
                        }
                        "triangle" => {
                            push(&mut pts, b, conv(r0));
                            push(&mut pts, b + per * 0.5, conv(r1));
                        }
                        "saw" => {
                            push(&mut pts, b, conv(r0));
                            push(&mut pts, e - 0.01, conv(r1));
                        }
                        _ => {
                            push(&mut pts, b, conv(r1));
                            push(&mut pts, b + per * 0.5 - 0.01, conv(r1));
                            push(&mut pts, b + per * 0.5, conv(r0));
                            push(&mut pts, e - 0.01, conv(r0));
                        }
                    }
                    b += per;
                    k += 1;
                }
            }
            _ => {
                push(&mut pts, st, conv(r1));
                push(&mut pts, body - 0.01, conv(r1));
            }
        }
        if gap > 0.0 {
            push(&mut pts, body, if db { 0.0 } else { conv(r0) });
            push(&mut pts, en - 0.01, if db { 0.0 } else { conv(r0) });
        }
        let next_joined = hit.get(si + 1).copied().unwrap_or(false) && (secs[si + 1].start - en).abs() < 0.02;
        if !next_joined {
            push(&mut pts, en, base);
        }
    }
    pts.retain(|p| p.0.is_finite() && p.1.is_finite());
    pts
}

/// The expression plan for a recipe's part: the genre's rules (no LFO into pitch, snapped fine tuning, vibrato where the genre
/// has it) with the glide and slides the recipe's pitch block asks for, and its static unison kept.
pub fn expression(r: &Recipe, genre: &str, bpm: f64) -> sd::Expr {
    let role = expr_role(r);
    let mut x = sd::expression(genre, &role, crate::songgen::role_kind(&role), bpm);
    let p = &r.composition.pitch;
    if r.riser() {
        x.riser = true;
        return x;
    }
    let mono = matches!(r.composition.voicing.as_str(), "single" | "octave" | "fifth") && r.role != "pad" && r.role != "chord";
    x.mono = mono;
    if p.slides > 0.0 && mono {
        x.glide_secs = (p.glide_ms.max(10.0) / 1000.0).min(0.2);
        x.slides = p.slides.clamp(0.0, 1.0) as f64;
        x.slide_max = p.slide_max;
        x.why.retain(|w| !w.contains("slide"));
        x.why.push(format!("slides on {:.0}% of the held steps (the recipe)", p.slides * 100.0));
    } else {
        x.glide_secs = 0.0;
        x.slides = 0.0;
        x.why.retain(|w| !w.contains("slide"));
    }
    if !p.vibrato {
        x.vibrato = None;
        x.why.retain(|w| !w.contains("vibrato"));
    }
    // the recipe's unison spread is a static design choice: kept
    x.detune = None;
    x
}

// ── writing the part ─────────────────────────────────────────────────────────────────────────────────────────────────

/// what the writer works from
#[derive(Clone, Debug)]
pub struct WriteCtx<'a> {
    /// the clip's length in beats
    pub len: f64,
    /// chords over the clip (beats from its start)
    pub chords: &'a [Chord],
    pub key: Key,
    pub phase: Phase,
    pub energy: f64,
    pub seed: u64,
    /// the notes there now (a melody to keep, a rhythm to mirror)
    pub source: &'a [Note],
    /// the bar number of the clip's start in the song (for the switching order and the fills)
    pub bar0: usize,
}

/// what the writer made: notes per switching-set member (index into the set; 0 = this recipe) and lanes that go with them (a
/// chord-quality parameter), as (parameter, [(beat from the clip start, value)])
#[derive(Clone, Debug, Default)]
pub struct Written {
    pub notes: Vec<(usize, Note)>,
    pub lanes: Vec<(String, Vec<(f64, f32)>)>,
    pub said: Vec<String>,
}

fn chord_at(c: &WriteCtx, beat: f64) -> (u8, usize) {
    if let Some(ch) = crate::theory::chord_at(c.chords, beat) {
        return (ch.root, ch.q);
    }
    if let Some(ch) = c.chords.iter().filter(|ch| ch.start <= beat + 1e-6).last().or(c.chords.first()) {
        return (ch.root, ch.q);
    }
    (c.key.tonic, if c.key.minor { 1 } else { 0 })
}

/// a pitch class in a register: the lowest at or above `lo` (`low`) or the one nearest the middle
fn place(pc: u8, lo: u8, hi: u8, low: bool) -> u8 {
    let mut cands: Vec<u8> = (lo..=hi).filter(|k| k % 12 == pc % 12).collect();
    if cands.is_empty() {
        // a register narrower than an octave: the nearest one outside it
        let below = (0..lo).rev().find(|k| k % 12 == pc % 12);
        let above = (hi.saturating_add(1)..=127).find(|k| k % 12 == pc % 12);
        cands = [below, above].into_iter().flatten().collect();
        cands.sort_by_key(|k| if *k < lo { lo - k } else { k - hi });
        return cands.first().copied().unwrap_or(lo);
    }
    if low {
        cands[0]
    } else {
        let mid = (lo as i32 + hi as i32) / 2;
        *cands.iter().min_by_key(|k| (**k as i32 - mid).abs()).unwrap_or(&cands[0])
    }
}

fn fold_into(k: i32, lo: u8, hi: u8) -> u8 {
    let mut k = k;
    while k > hi as i32 && k - 12 >= lo as i32 {
        k -= 12;
    }
    while k < lo as i32 && k + 12 <= hi as i32 {
        k += 12;
    }
    k.clamp(0, 127) as u8
}

/// the chord's seventh in the key (a minor seventh unless the key has the major one over this root)
fn seventh(root: u8, q: usize, key: Key) -> u8 {
    let iv = QUAL[q].1;
    if iv.len() >= 4 {
        return iv[3];
    }
    if key.in_scale((root + 11) % 12) && !key.in_scale((root + 10) % 12) {
        11
    } else {
        10
    }
}

fn third(q: usize) -> u8 {
    QUAL[q].1.get(1).copied().unwrap_or(4)
}

fn fifth(q: usize) -> u8 {
    let iv = QUAL[q].1;
    if iv.contains(&7) {
        7
    } else {
        iv.get(2).copied().unwrap_or(7)
    }
}

/// a chord voiced in the register near the previous one
pub fn voice_chord(root: u8, q: usize, key: Key, voicing: &str, lo: u8, hi: u8, prev: &[u8]) -> Vec<u8> {
    let (t, f, s) = (third(q) as i32, fifth(q) as i32, seventh(root, q, key) as i32);
    let nine = if key.accepts((root + 2) % 12) { 14 } else { 12 };
    let shape: Vec<i32> = match voicing {
        "seventh" => vec![0, t, f, s],
        "add9" => vec![0, t, f, nine],
        "open" => vec![0, f, nine, t + 12],
        "sus2" => vec![0, 2, f, 12],
        "spread" => vec![0, f, s, t + 12, f + 12],
        "octave" => vec![0, 12],
        "fifth" => vec![0, f],
        "single" => vec![0],
        _ => vec![0, t, f],
    };
    let span = *shape.iter().max().unwrap_or(&0);
    // every octave the root can sit on with the whole shape inside the register (else the lowest), the one nearest the last
    let mut best: Option<(i32, Vec<u8>)> = None;
    let centre_prev = if prev.is_empty() { None } else { Some(prev.iter().map(|x| *x as f64).sum::<f64>() / prev.len() as f64) };
    for base in (lo as i32 - 12)..=(hi as i32) {
        if base.rem_euclid(12) != root as i32 % 12 || base < 0 {
            continue;
        }
        let notes: Vec<u8> = shape.iter().map(|i| base + i).filter(|k| (0..=127).contains(k)).map(|k| k as u8).collect();
        let outside = notes.iter().filter(|k| **k < lo || **k > hi).count() as i32;
        let centre = notes.iter().map(|x| *x as f64).sum::<f64>() / notes.len().max(1) as f64;
        let target = centre_prev.unwrap_or(lo as f64 + (hi as f64 - lo as f64 - span as f64).max(0.0) * 0.35 + span as f64 * 0.5);
        let cost = outside * 100 + (centre - target).abs().round() as i32;
        if best.as_ref().map(|b| cost < b.0).unwrap_or(true) {
            best = Some((cost, notes));
        }
    }
    let mut v = best.map(|b| b.1).unwrap_or_default();
    // whatever still lies outside folds in by octaves
    for k in v.iter_mut() {
        *k = fold_into(*k as i32, lo, hi);
    }
    v.sort_unstable();
    v.dedup();
    v
}

fn vel_of(ch: char, energy: f64) -> f32 {
    let base = match ch {
        'x' => 1.0,
        'o' | 'n' => 0.82,
        'g' => 0.55,
        _ => 0.9,
    };
    (base * (0.82 + 0.18 * energy.clamp(0.0, 1.0)) as f32).clamp(0.1, 1.0)
}

fn mono_voicing(v: &str) -> bool {
    matches!(v, "single" | "octave" | "fifth")
}

/// Write the part a recipe plays over a clip (see [`Composition`]): the notes for each member of the switching set and the scoop
/// lane. The source notes are kept as a melody (`melody`), mirrored as a rhythm (`mirror`), or ignored (`pattern`, `hold`, `oneshot`).
pub fn write(r: &Recipe, c: &WriteCtx) -> Written {
    let comp = &r.composition;
    let (lo, hi) = r.register();
    let mut w = Written::default();
    let mut rng = crate::compose::Rng::new(c.seed ^ 0xC0FFEE);
    let ph = phase_name(c.phase);
    let len = c.len.max(0.25);
    let mut mode = comp.mode.as_str();
    let usable: Vec<&Pat> = comp.pats.iter().filter(|p| p.phases.is_empty() || p.phases.iter().any(|x| x == ph)).collect();
    if mode == "pattern" && usable.is_empty() {
        // nothing written for this part of the form: the notes there keep their rhythm on the recipe's roots
        mode = if c.source.is_empty() { "pattern" } else { "mirror" };
        if !c.source.is_empty() {
            w.said.push(format!("no {ph} pattern: the rhythm kept on the roots"));
        }
    }
    if mode == "melody" && c.source.is_empty() {
        mode = if comp.pats.is_empty() { "hold" } else { "pattern" };
    }
    let pats: Vec<&Pat> = if usable.is_empty() { comp.pats.iter().collect() } else { usable };
    let chordy = !mono_voicing(&comp.voicing);
    let mut out: Vec<Note> = vec![];
    let mut prev_chord: Vec<u8> = vec![];
    match mode {
        "none" => {}
        "oneshot" => {
            let (root, _) = chord_at(c, 0.0);
            let k = place(root, lo, hi, true);
            let l = if comp.hold_beats > 0.0 { comp.hold_beats.min(len) } else { (len - 0.25).max(0.25) };
            out.push(Note::new(0.0, l, k, 0.95));
        }
        "hold" => {
            // a chord per chord change, held (legato, never overlapping)
            let mut cuts: Vec<f64> = vec![0.0];
            for ch in c.chords {
                if ch.start > 0.05 && ch.start < len - 0.05 {
                    cuts.push(ch.start);
                }
            }
            cuts.push(len);
            cuts.dedup_by(|a, b| (*a - *b).abs() < 0.05);
            for win in cuts.windows(2) {
                let (a, b) = (win[0], win[1]);
                let (root, q) = chord_at(c, a + 0.01);
                let v = voice_chord(root, q, c.key, &comp.voicing, lo, hi, &prev_chord);
                for k in &v {
                    out.push(Note::new(a, ((b - a) * comp.gate as f64).max(0.1) - 0.02, *k, (0.72 + 0.12 * c.energy) as f32));
                }
                prev_chord = v;
            }
        }
        "mirror" | "melody" => {
            let mut src: Vec<Note> = c.source.iter().filter(|n| n.start < len).cloned().collect();
            src.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.key.cmp(&b.key)));
            if mode == "mirror" {
                // one note per onset (the lowest), on the chord's root
                src.dedup_by(|b, a| (b.start - a.start).abs() < 0.02);
                for n in &src {
                    let (root, _) = chord_at(c, n.start + 0.01);
                    out.push(Note { key: place(root, lo, hi, true), ..n.clone() });
                }
            } else {
                // the melody moved by whole octaves so its middle sits in the register, the rest folded in
                let mut keys: Vec<u8> = src.iter().map(|n| n.key).collect();
                keys.sort_unstable();
                let med = keys.get(keys.len() / 2).copied().unwrap_or(lo) as i32;
                let mid = (lo as i32 + hi as i32) / 2;
                let shift = ((mid - med) as f64 / 12.0).round() as i32 * 12;
                for n in &src {
                    out.push(Note { key: fold_into(n.key as i32 + shift, lo, hi), ..n.clone() });
                }
            }
            // the recipe's articulation: a hit sounds `gate` of the way to the next onset
            let starts: Vec<f64> = out.iter().map(|n| n.start).collect();
            for (i, n) in out.iter_mut().enumerate() {
                let next = starts[i + 1..].iter().copied().find(|s| *s > n.start + 0.01).unwrap_or(len);
                let room = next - n.start;
                if n.len > room * comp.gate as f64 || comp.gate < 0.99 {
                    n.len = (room * comp.gate as f64).max(0.05).min(n.len.max(0.05));
                }
                n.len = n.len.min(len - n.start).max(0.03);
            }
        }
        _ => {
            // patterns over the bars: a fill on the last bar of four, the others in turn
            let fills: Vec<&&Pat> = pats.iter().filter(|p| p.fill).collect();
            let mains: Vec<&&Pat> = pats.iter().filter(|p| !p.fill).collect();
            let mains = if mains.is_empty() { pats.iter().collect::<Vec<_>>() } else { mains };
            let mut pos = 0.0;
            let mut turn = (rng.next() % mains.len().max(1) as u64) as usize;
            let mut arp_i = 0usize;
            let mut last_hit: Option<(usize, char)> = None;
            let mut grids: Vec<f64> = vec![];
            let mut guard = 0;
            while pos < len - 1e-6 && guard < 4096 {
                guard += 1;
                let bar = c.bar0 + (pos / 4.0).floor() as usize;
                let p: &Pat = if !fills.is_empty() && bar % 4 == 3 {
                    fills[(bar / 4) % fills.len()]
                } else {
                    let p = mains[turn % mains.len()];
                    turn += 1;
                    p
                };
                let span = p.beats().max(p.grid);
                for (i, ch) in p.steps.iter().enumerate() {
                    let t = pos + i as f64 * p.grid;
                    if t >= len - 1e-6 {
                        break;
                    }
                    match ch {
                        '.' => last_hit = None,
                        '-' => {
                            if let Some((j, _)) = last_hit {
                                let hits: Vec<usize> = (j..out.len()).collect();
                                for h in hits {
                                    out[h].len += p.grid;
                                }
                            }
                        }
                        _ => {
                            let (root, q) = chord_at(c, t + 0.01);
                            let vel = vel_of(*ch, c.energy);
                            let first = out.len();
                            let keys: Vec<u8> = if *ch == 'n' {
                                // the next tone of the arpeggio over the chord
                                let tones = arp_tones(root, q, c.key, &comp.voicing, lo, hi, &comp.arp);
                                let k = tones.get(arp_i % tones.len().max(1)).copied().unwrap_or(place(root, lo, hi, false));
                                arp_i += 1;
                                vec![k]
                            } else if chordy && matches!(ch, 'x' | 'o' | 'g') {
                                let v = voice_chord(root, q, c.key, &comp.voicing, lo, hi, &prev_chord);
                                prev_chord = v.clone();
                                v
                            } else {
                                let r0 = place(root, lo, hi, true);
                                let k = match ch {
                                    '^' => if r0 + 12 <= hi { r0 + 12 } else { r0 },
                                    'v' => if r0 >= lo + 12 { r0 - 12 } else { r0 },
                                    '5' => fold_into(r0 as i32 + fifth(q) as i32, lo, hi),
                                    '3' => fold_into(r0 as i32 + third(q) as i32, lo, hi),
                                    '7' => fold_into(r0 as i32 + seventh(root, q, c.key) as i32, lo, hi),
                                    'b' => {
                                        // the scale step above the root (a minor key's second), else the fifth
                                        let up = (1..=2).map(|d| (root + d) % 12).find(|pc| c.key.accepts(*pc));
                                        match up {
                                            Some(pc) => fold_into(r0 as i32 + ((pc + 12 - root) % 12) as i32, lo, hi),
                                            None => fold_into(r0 as i32 + 7, lo, hi),
                                        }
                                    }
                                    _ => r0,
                                };
                                let mut v = vec![k];
                                if comp.voicing == "octave" && k as u16 + 12 <= 127 {
                                    v.push(k + 12);
                                } else if comp.voicing == "fifth" {
                                    v.push((k + 7).min(127));
                                }
                                v
                            };
                            for k in keys {
                                out.push(Note::new(t, p.grid, k, vel));
                                grids.push(p.grid);
                            }
                            last_hit = Some((first, *ch));
                        }
                    }
                }
                pos += span;
                last_hit = None;
            }
            // the gate: each hit sounds part of its span (holds stay legato into the next step)
            let g = comp.gate as f64;
            for (n, grid) in out.iter_mut().zip(grids.iter()) {
                n.len = (n.len - grid * (1.0 - g)).max(0.03).min(len - n.start);
            }
        }
    }
    // mono lines: nothing overlaps (no accidental glide) except the slides the recipe asks for
    out.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.key.cmp(&b.key)));
    let mono = mono_voicing(&comp.voicing) && !matches!(mode, "hold" | "oneshot");
    // the switching set: each bar (or hit) goes to its member
    let mut tagged: Vec<(usize, Note)> = out.into_iter().map(|n| (0usize, n)).collect();
    if let Some(sw) = &comp.switching {
        let order: Vec<usize> = sw.order.chars().filter(|ch| ch.is_ascii_alphabetic()).map(|ch| (ch.to_ascii_uppercase() as u8 - b'A') as usize).map(|i| if i < sw.set.len() { i } else { 0 }).collect();
        if !order.is_empty() {
            let mut hit = 0usize;
            let mut last_start = -1.0;
            for (m, n) in tagged.iter_mut() {
                *m = if sw.unit == "hit" {
                    if (n.start - last_start).abs() > 0.01 {
                        hit += 1;
                        last_start = n.start;
                    }
                    order[(hit - 1) % order.len()]
                } else {
                    let bar = c.bar0 + (n.start / 4.0).floor() as usize;
                    order[(bar / sw.bars.max(1) as usize) % order.len()]
                };
            }
            let used: Vec<usize> = { let mut u: Vec<usize> = tagged.iter().map(|x| x.0).collect(); u.sort_unstable(); u.dedup(); u };
            w.said.push(format!("sound switching {} across {}", sw.order.trim(), used.iter().map(|i| sw.set[*i].clone()).collect::<Vec<_>>().join(" / ")));
        }
    }
    if mono {
        let p = &comp.pitch;
        let members: Vec<usize> = { let mut u: Vec<usize> = tagged.iter().map(|x| x.0).collect(); u.sort_unstable(); u.dedup(); u };
        let mut slid = 0;
        for m in members {
            let idx: Vec<usize> = (0..tagged.len()).filter(|i| tagged[*i].0 == m).collect();
            for pair in idx.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                let (bs, bk) = (tagged[b].1.start, tagged[b].1.key);
                if (bs - tagged[a].1.start).abs() < 0.01 {
                    continue; // a stacked octave / fifth
                }
                let n = &mut tagged[a].1;
                let step = (bk as i32 - n.key as i32).unsigned_abs();
                let reaches = n.start + n.len >= bs - 0.13;
                if p.slides > 0.0 && reaches && step >= 1 && step <= p.slide_max as u32 && n.len >= 0.2 && rng.chance(p.slides as f64) {
                    n.len = bs - n.start + 0.06;
                    slid += 1;
                } else if n.start + n.len > bs - 0.01 {
                    n.len = (bs - n.start - 0.01).max(0.03);
                }
            }
        }
        if slid > 0 {
            w.said.push(format!("{slid} slide(s)"));
        }
    }
    // scoops into the chord entries (per-note pitch gestures)
    let sc = comp.pitch.scoop;
    if sc != 0.0 {
        let mut entries: Vec<f64> = vec![];
        let mut last_end = -10.0;
        let mut last_root: Option<u8> = None;
        for (_, n) in &tagged {
            let (root, _) = chord_at(c, n.start + 0.01);
            if (n.start - last_end > 0.9 || last_root != Some(root)) && entries.last().map(|e| (n.start - e).abs() > 0.05).unwrap_or(true) {
                entries.push(n.start);
            }
            last_end = last_end.max(n.start + n.len);
            last_root = Some(root);
        }
        // a per-note pitch gesture on every note of each chord entry (declared by the recipe, so the pitch guard lets it through):
        // from `sc` semitones (negative = from below) back to the note within a sixteenth to an eighth
        let mut k = 0;
        for (_, n) in tagged.iter_mut() {
            if entries.iter().any(|e| (n.start - e).abs() < 0.02) {
                if let Some(c) = crate::gesture::shape("scoop", sc.abs(), n.len) {
                    n.bend = if sc > 0.0 { c.iter().map(|p| (p.0, -p.1)).collect() } else { c };
                    k += 1;
                }
            }
        }
        if k > 0 {
            w.said.push(format!("a {:+.0} semitone scoop into each chord (per note)", sc));
        }
    }
    // dives at the end of each four-bar phrase, a rising last note: declared by the recipe, written as per-note gestures
    if comp.pitch.dive > 0.0 || comp.pitch.rise > 0.0 {
        let mut ns: Vec<Note> = tagged.iter().map(|x| x.1.clone()).collect();
        let mut k = 0;
        if comp.pitch.dive > 0.0 {
            let mut ph = 0.0;
            while ph < len - 1e-6 {
                let end = (ph + 16.0).min(len);
                let last = (0..ns.len()).filter(|i| ns[*i].start >= end - 4.0 && ns[*i].start < end && ns[*i].len >= 0.24).max_by(|a, b| ns[*a].start.total_cmp(&ns[*b].start));
                if let Some(li) = last {
                    if ns[li].bend.is_empty() && crate::gesture::apply_shape(&mut ns, li, "dive", comp.pitch.dive) {
                        k += 1;
                    }
                }
                ph += 16.0;
            }
        }
        if comp.pitch.rise > 0.0 {
            k += crate::gesture::write_kind(&mut ns, "riser", 0.0, len, comp.pitch.rise);
        }
        for (x, n) in tagged.iter_mut().zip(ns) {
            x.1.bend = n.bend;
        }
        if k > 0 {
            w.said.push(format!("{k} pitch dive(s) / riser(s) (per note)"));
        }
    }
    // a parameter that follows the chord quality (minor / major), held per chord
    if let Some(qp) = &comp.quality_param {
        let mut pts: Vec<(f64, f32)> = vec![];
        let mut cuts: Vec<f64> = vec![0.0];
        cuts.extend(c.chords.iter().map(|ch| ch.start).filter(|s| *s > 0.01 && *s < len - 0.01));
        cuts.push(len);
        cuts.dedup_by(|a, b| (*a - *b).abs() < 0.01);
        for win in cuts.windows(2) {
            let (_, q) = chord_at(c, win[0] + 0.01);
            let v = if third(q) == 3 { qp.minor } else { qp.major }.clamp(0.0, 1.0);
            pts.push((win[0], v));
            pts.push((win[1] - 0.01, v));
        }
        w.lanes.push((qp.param.clone(), pts));
    }
    for (_, n) in tagged.iter_mut() {
        n.start = n.start.clamp(0.0, len);
        n.len = n.len.min(len - n.start + 0.1).max(0.03);
    }
    tagged.retain(|(_, n)| n.start < len - 1e-6);
    w.notes = tagged;
    w
}

fn arp_tones(root: u8, q: usize, key: Key, voicing: &str, lo: u8, hi: u8, arp: &Arp) -> Vec<u8> {
    let v = if mono_voicing(voicing) { voice_chord(root, q, key, "triad", lo, hi, &[]) } else { voice_chord(root, q, key, voicing, lo, hi, &[]) };
    let mut tones: Vec<u8> = vec![];
    for o in 0..arp.octaves.max(1) {
        for k in &v {
            let x = *k as u16 + 12 * o as u16;
            if x <= hi as u16 + 12 && x <= 127 {
                tones.push(x as u8);
            }
        }
    }
    tones.sort_unstable();
    tones.dedup();
    match arp.order.as_str() {
        "down" => tones.reverse(),
        "updown" | "pingpong" => {
            let mut back: Vec<u8> = tones.iter().rev().skip(1).copied().collect();
            if arp.order == "updown" && !back.is_empty() {
                back.pop();
            }
            tones.extend(back);
        }
        _ => {}
    }
    tones
}

/// the chords and key under a stretch of music (theory's chord detection on the notes, two-beat segments)
pub fn harmony(notes: &[crate::theory::N], len: f64, fallback: Key) -> (Vec<Chord>, Key) {
    if notes.is_empty() {
        return (vec![], fallback);
    }
    let key = {
        let (k, conf, _) = crate::theory::detect_key(notes, 0.0, len);
        if conf > 0.0 { k } else { fallback }
    };
    (crate::theory::detect_chords(notes, len, 2.0, true), key)
}

/// a one-line description of a recipe's sound, for lists and the agent
pub fn summary(r: &Recipe) -> String {
    let fx: Vec<&str> = r.fx_chain.iter().map(|f| f.effect.as_str()).collect();
    format!("{} ({} {}, {}{})", r.name, r.instrument, r.role, if fx.is_empty() { "no effects".to_string() } else { fx.join(" > ") }, if r.mix.sidechain.is_some() { ", ducks under the kick" } else { "" })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib() -> Arc<Library> {
        library()
    }

    #[test]
    fn grids_parse() {
        let g = |s: &str| parse_grid(&serde_json::Value::String(s.into())).unwrap();
        assert!((g("1/16") - 0.25).abs() < 1e-9 && (g("1/8") - 0.5).abs() < 1e-9 && (g("1/4") - 1.0).abs() < 1e-9);
        assert!((g("1/8T") - 1.0 / 3.0).abs() < 1e-9 && (g("1/16T") - 1.0 / 6.0).abs() < 1e-9 && (g("1/8.") - 0.75).abs() < 1e-9);
        assert!(parse_grid(&serde_json::json!(0.5)).unwrap() == 0.5 && parse_grid(&serde_json::json!("fast")).is_none());
    }

    #[test]
    fn malformed_files_and_names_are_reported_not_fatal() {
        let (rs, p) = parse_text("{ not json", "bad.json");
        assert!(rs.is_empty() && p.len() == 1 && p[0].starts_with("bad.json"));
        let text = r#"{"recipes": [
            {"id": "ok", "role": "bass", "instrument": "growl", "genres": ["dubstep", "polka"],
             "params": {"cut": 0.4, "nonsense": 0.3, "amp_a": 1.7},
             "fx_chain": [{"effect": "OTT", "params": {"depth": 0.5, "zap": 1}}, {"effect": "Magic"}],
             "automation": [{"target": "cut", "range": [0.3, 0.6]}, {"target": "detn", "range": [0, 1]}, {"target": "fx:OTT.depth", "range": [0.2, 0.5]}],
             "composition": {"patterns": ["x.o-", {"steps": "x!..", "grid": "1/16"}]}},
            {"role": "lead", "instrument": "prism"},
            {"id": "x2", "role": "wizard", "instrument": "prism"},
            {"id": "x3", "role": "lead", "instrument": "kazoo"}
        ]}"#;
        let (rs, p) = parse_text(text, "t.json");
        assert_eq!(rs.len(), 1, "{p:?}");
        let r = &rs[0];
        assert!(r.params.contains_key("cut") && !r.params.contains_key("nonsense") && r.params["amp a"] == 1.0);
        assert_eq!(r.fx_chain.len(), 1);
        assert!(!r.fx_chain[0].params.contains_key("zap"));
        assert_eq!(r.automation.len(), 2, "the pitch lane on detune is refused");
        assert_eq!(r.composition.pats.len(), 1);
        for want in ["polka", "nonsense", "amp_a", "zap", "Magic", "detn", "pattern #2", "needs an \"id\"", "wizard", "kazoo"] {
            assert!(p.iter().any(|x| x.contains(want)), "{want} reported: {p:?}");
        }
    }

    #[test]
    fn the_shipped_recipes_load_clean() {
        let l = lib();
        assert!(l.recipes.len() >= 30, "{} recipes from {:?}: {:?}", l.recipes.len(), l.dirs, l.problems);
        assert!(l.problems.is_empty(), "{:#?}", l.problems);
        for role in ROLES {
            assert!(l.recipes.iter().any(|r| r.role == role), "{role}");
        }
        for r in &l.recipes {
            assert!(!r.description.is_empty() && !r.inspired_by.is_empty(), "{}: description and inspired_by", r.id);
            assert!(r.fx_chain.iter().all(|f| !f.why.is_empty()), "{}: every effect says why", r.id);
            if !r.is_master() {
                let (lo, hi) = r.register();
                assert!(lo <= hi && hi <= 108, "{}", r.id);
            }
        }
    }

    #[test]
    fn selection_follows_role_genre_and_words() {
        let l = lib();
        let top = |role: &str, genre: &str, words: &str| rank(&l, role, genre, words, -1.0).first().map(|x| x.1.id.clone()).unwrap_or_default();
        assert!(top("growl bass", "dubstep", "riddim dubstep").starts_with("riddim"), "{}", top("growl bass", "dubstep", "riddim dubstep"));
        assert!(top("sub bass", "dubstep", "").contains("sub") || top("sub bass", "dubstep", "").contains("808"));
        assert_eq!(top("bass", "drum and bass", "neurofunk reese"), "reese_neuro");
        assert!(top("supersaw lead", "trance", "").contains("supersaw"));
        assert_eq!(top("wobble bass", "dubstep", ""), "wobble_triplet");
        assert!(top("riser", "dubstep", "").contains("riser"));
        // a piano, keys, strings or a vocal part never gets a synth recipe
        for r in ["piano", "keys", "organ", "strings", "vocal lead", "choir"] {
            assert!(rank(&l, r, "dubstep", "", -1.0).is_empty(), "{r}");
        }
        // a sub role never gets a growl, a growl role never a sub, a riser role never the impact
        for g in ["dubstep", "future bass", "trap", "drum and bass"] {
            for (_, r) in rank(&l, "sub bass", g, "", -1.0) {
                assert!(r.has_tag("sub") || r.has_tag("808"), "{g}: {}", r.id);
            }
            for (_, r) in rank(&l, "growl bass", g, "", -1.0) {
                assert!(!r.has_tag("sub"), "{g}: {}", r.id);
            }
        }
        // a genre with nothing for the role: no pick
        assert!(pick(&l, "growl bass", "folk", "", 0.5, 1, &[]).is_none());
        assert!(pick(&l, "growl bass", "dubstep", "", 0.9, 1, &[]).is_some());
    }

    fn am_ctx<'a>(chords: &'a [Chord], src: &'a [Note], phase: Phase) -> WriteCtx<'a> {
        WriteCtx { len: 16.0, chords, key: Key::new(9, true), phase, energy: 0.9, seed: 7, source: src, bar0: 0 }
    }

    fn am_chords() -> Vec<Chord> {
        // Am F C G, a bar each
        [(9u8, 1usize), (5, 0), (0, 0), (7, 0)].iter().enumerate().map(|(i, (r, q))| Chord { start: i as f64 * 4.0, len: 4.0, root: *r, q: *q, bass: *r, implied: false }).collect()
    }

    #[test]
    fn the_writer_keeps_register_rhythm_voicing_and_switching() {
        let l = lib();
        let ch = am_chords();
        // riddim: in its register, on its grid, the bars spread over the switching set, no accidental overlap
        let rid = l.get("riddim_gate_a").unwrap();
        let w = write(rid, &am_ctx(&ch, &[], Phase::Drop));
        let (lo, hi) = rid.register();
        assert!(!w.notes.is_empty());
        assert!(w.notes.iter().all(|(_, n)| n.key >= lo && n.key <= hi), "{:?}", w.notes);
        let members: std::collections::BTreeSet<usize> = w.notes.iter().map(|x| x.0).collect();
        assert!(members.len() >= 2, "the switching set is used: {members:?}");
        for m in &members {
            let mut ns: Vec<&Note> = w.notes.iter().filter(|x| x.0 == *m).map(|x| &x.1).collect();
            ns.sort_by(|a, b| a.start.total_cmp(&b.start));
            for p in ns.windows(2) {
                assert!(p[0].start + p[0].len <= p[1].start + 1e-6, "no overlap without a slide: {:?}", p);
            }
        }
        // the notes follow the chords (the root in each bar)
        for (_, n) in &w.notes {
            let root = ch.iter().find(|c| n.start >= c.start && n.start < c.end()).unwrap().root;
            let rel = (n.key as i32 - root as i32).rem_euclid(12);
            assert!([0, 7, 1, 2, 3, 4].contains(&rel), "{n:?} over root {root}");
        }
        // future bass stabs: seventh / spread chords, 4+ notes, in the register
        let fb = l.get("future_bass_chord_stab").unwrap();
        let w = write(fb, &am_ctx(&ch, &[], Phase::Drop));
        let mut by_start: BTreeMap<i64, Vec<u8>> = BTreeMap::new();
        for (_, n) in &w.notes {
            by_start.entry((n.start * 1000.0) as i64).or_default().push(n.key);
        }
        assert!(by_start.values().all(|v| v.len() >= 4), "{by_start:?}");
        assert!(w.notes.iter().filter(|(_, n)| !n.bend.is_empty()).count() >= 8 && !w.lanes.iter().any(|l| l.0 == "bend"), "the stabs scoop in, per note (declared)");
        assert!(w.notes.iter().all(|(_, n)| n.bend.is_empty() || crate::gesture::at(&n.bend, 0.0) < 0.0), "from below");
        // the arp: one note at a time on a sixteenth grid, chord tones only
        let arp = l.get("pluck_arp_16th").unwrap();
        let w = write(arp, &am_ctx(&ch, &[], Phase::Drop));
        assert!(w.notes.len() >= 32);
        for (_, n) in &w.notes {
            let c = ch.iter().find(|c| n.start >= c.start && n.start < c.end()).unwrap();
            assert!(c.has(n.key % 12) || crate::theory::Key::new(9, true).accepts(n.key % 12), "{n:?}");
            assert!(((n.start * 4.0) - (n.start * 4.0).round()).abs() < 1e-6);
        }
        // a lead keeps its melody, moved into the register by octaves
        let lead = l.get("supersaw_anthem").unwrap();
        let src: Vec<Note> = [(0.0, 69u8), (1.0, 72), (2.0, 76), (3.0, 74)].iter().map(|(s, k)| Note::new(*s, 1.0, *k - 24, 0.8)).collect();
        let w = write(lead, &am_ctx(&ch, &src, Phase::Drop));
        let ks: Vec<u8> = w.notes.iter().filter(|(_, n)| n.start < 4.0).map(|(_, n)| n.key).collect();
        let (lo, hi) = lead.register();
        assert!(ks.iter().all(|k| *k >= lo && *k <= hi), "{ks:?}");
        let pcs: Vec<u8> = src.iter().map(|n| n.key % 12).collect();
        assert!(ks.iter().all(|k| pcs.contains(&(k % 12))), "the melody's notes, {ks:?}");
    }

    #[test]
    fn lanes_follow_the_form() {
        let secs = vec![
            Sec { tag: "intro".into(), start: 0.0, end: 16.0 },
            Sec { tag: "build".into(), start: 16.0, end: 32.0 },
            Sec { tag: "drop".into(), start: 32.0, end: 64.0 },
        ];
        let a = AutoTpl { target: "cut".into(), sections: vec!["build".into()], shape: "ramp_exp".into(), range: vec![0.3, 0.8], end_gap: 1.0, ..Default::default() };
        let pts = lane_points(&a, &secs, 0.5);
        let lane = AutoLane { param: "cut".into(), points: pts.clone() };
        assert!(pts.windows(2).all(|w| w[1].0 > w[0].0), "{pts:?}");
        assert_eq!(lane.value_at(8.0), Some(0.5));
        assert!(lane.value_at(17.0).unwrap() < 0.35 && lane.value_at(30.5).unwrap() > 0.7);
        assert!((lane.value_at(31.5).unwrap() - 0.3).abs() < 1e-3, "the breath before the drop");
        assert_eq!(lane.value_at(40.0), Some(0.5));
        let s = AutoTpl { target: "l1 sync".into(), sections: vec!["drop".into()], shape: "steps".into(), period: 1.0, steps: vec![0.45, 0.7, 0.82, 0.94], ..Default::default() };
        let lane = AutoLane { param: "x".into(), points: lane_points(&s, &secs, 0.5) };
        assert!((lane.value_at(32.5).unwrap() - 0.45).abs() < 1e-6 && (lane.value_at(34.5).unwrap() - 0.82).abs() < 1e-6);
        let d = AutoTpl { target: "master:vol".into(), sections: vec!["build".into()], shape: "ramp_down".into(), unit: "db".into(), range: vec![-3.0, 0.0], ..Default::default() };
        let lane = AutoLane { param: "vol".into(), points: lane_points(&d, &secs, 0.9) };
        assert!((lane.value_at(31.9).unwrap() - 0.9 * 10f32.powf(-3.0 / 20.0)).abs() < 0.01 && (lane.value_at(33.0).unwrap() - 0.9).abs() < 1e-6);
    }

    #[test]
    fn applying_puts_the_chain_in_order_and_keeps_pitch_rules() {
        let l = lib();
        let r = l.get("vr_bandpass_ott_growl").unwrap();
        let mut t = Track::new("Bass", crate::model::TrackKind::Instrument, 0, crate::dsp::make_instrument("prism", 44100.0));
        t.inst_kind = "prism".into();
        t.effects.push(EffectSlot::new("Chorus", 44100.0).unwrap());
        t.auto.push(AutoLane { param: "cut".into(), points: vec![(0.0, 0.2)] });
        let secs = vec![Sec { tag: "drop".into(), start: 0.0, end: 32.0 }];
        apply_sound(&mut t, r, &Ctx { genre: "dubstep".into(), bpm: 140.0, secs, sr: 44100.0, role: "growl bass".into() });
        assert_eq!(t.inst_kind, "growl");
        let names: Vec<&str> = t.effects.iter().map(|e| e.name.as_str()).collect();
        let want: Vec<&str> = r.fx_chain.iter().map(|f| f.effect.as_str()).filter(|n| *n != "Ducker").collect();
        assert_eq!(&names[..want.len()], &want[..], "{names:?}");
        assert_eq!(*names.last().unwrap(), "Ducker");
        assert!(names.contains(&"Imager") && !names.contains(&"Chorus"));
        assert!((t.volume - level_volume(r.mix.level_db)).abs() < 1e-6);
        // no lane left on a pitch parameter
        assert!(t.auto.iter().all(|a| !crate::redesign::pitch_altering(&a.param)));
        let x = expression(r, "dubstep", 140.0);
        assert!(x.glide_secs == 0.0 && x.slides == 0.0 && x.vibrato.is_none());
    }
}
