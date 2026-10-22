//! What the instruments sound like, measured rather than assumed.
//!
//! Choosing an instrument for a generated or imported part by name alone goes wrong in two ways: some patches are not tuned to the
//! twelve-tone scale the song is written in (a Prism patch in 19-EDO or slendro, an oscillator pair a tritone apart, a bell with
//! stretched partials), and a name says little about what a plugin can do. So every synth patch can be *played*: a few notes are rendered
//! offline and measured (pitch against equal temperament, how much of the spectrum is not on the harmonic series, attack, sustain,
//! brightness, low end). The result is cached per patch file and used to
//!   * keep patches that would play out of tune or clash with the song's chords away from the tables the song builder and the import
//!     chooser pick from ([`ok_for`]), and
//!   * tell the language model what each plugin is for and how each candidate patch sounds ([`digest`], [`describe`]).
use crate::dsp::Instrument;
use crate::patch;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

const SR: f32 = 48_000.0;

/// What playing a patch showed.
#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    /// the worst pitch error over the test notes, in cents against twelve-tone equal temperament (octave folded: a patch an octave up is fine)
    pub tune_dev: f32,
    /// share of the spectrum's peak energy that is NOT on the harmonic series of the note (0 = perfectly harmonic, 1 = all clashing)
    pub inharm: f32,
    /// a pitch could be heard at all (false for noise, percussion, risers)
    pub pitched: bool,
    /// spectral centroid of middle C, Hz
    pub centroid: f32,
    pub attack_ms: f32,
    /// level one second in against the peak, 0..1 (1 = holds, 0 = dies away)
    pub sustain: f32,
    /// share of the energy below 250 Hz on a bass note (C2)
    pub lows: f32,
}

impl Profile {
    /// plays the notes it is asked for
    pub fn tuned(&self) -> bool {
        self.pitched && self.tune_dev <= 20.0
    }
    /// Short words for a language model (and the UI): how it sounds and what it is for.
    pub fn tags(&self) -> Vec<&'static str> {
        let mut t = vec![];
        if !self.pitched {
            t.push("unpitched");
        } else if self.tune_dev > 20.0 {
            t.push("out of tune with 12-tone");
        } else {
            t.push("in tune");
        }
        if self.pitched {
            if self.inharm > 0.5 {
                t.push("clashing partials");
            } else if self.inharm > 0.3 {
                t.push("metallic");
            }
        }
        t.push(if self.attack_ms < 12.0 { "sharp attack" } else if self.attack_ms < 120.0 { "soft attack" } else { "slow swell" });
        t.push(if self.sustain > 0.55 { "sustained" } else if self.sustain > 0.15 { "decaying" } else { "short" });
        t.push(if self.centroid < 700.0 { "dark" } else if self.centroid < 2200.0 { "warm" } else { "bright" });
        t
    }
}

// ── measuring ───────────────────────────────────────────────────────────────────

fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -std::f32::consts::TAU / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        let mut i = 0;
        while i < n {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (ur, ui) = (re[i + k], im[i + k]);
                let (vr, vi) = (re[i + k + len / 2] * cr - im[i + k + len / 2] * ci, re[i + k + len / 2] * ci + im[i + k + len / 2] * cr);
                re[i + k] = ur + vr;
                im[i + k] = ui + vi;
                re[i + k + len / 2] = ur - vr;
                im[i + k + len / 2] = ui - vi;
                let nr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = nr;
            }
            i += len;
        }
        len <<= 1;
    }
}

fn hz(key: u8) -> f32 {
    440.0 * 2f32.powf((key as f32 - 69.0) / 12.0)
}

/// Play one note (held) and return `secs` of mono audio.
fn play(inst: &mut dyn Instrument, key: u8, secs: f32) -> Vec<f32> {
    inst.all_off();
    inst.set_tempo(120.0);
    inst.note_on(key, 0.8);
    let total = (secs * SR) as usize;
    let mut out = Vec::with_capacity(total);
    let mut buf = vec![0.0f32; 1024];
    while out.len() < total {
        buf.iter_mut().for_each(|x| *x = 0.0);
        inst.render(&mut buf);
        out.extend(buf.chunks_exact(2).map(|f| (f[0] + f[1]) * 0.5));
    }
    out.truncate(total);
    inst.all_off();
    out
}

struct Spectrum {
    mag: Vec<f32>,
    bin_hz: f32,
}

fn spectrum(x: &[f32], from: usize, len: usize) -> Spectrum {
    let n = 262144;                  // zero padded: the bins are 0.18 Hz apart, fine enough to read pitch to a couple of cents
    let mut re = vec![0.0f32; n];
    let mut im = vec![0.0f32; n];
    let len = len.min(x.len().saturating_sub(from)).min(n);
    for i in 0..len {
        let w = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / len as f32).cos();
        re[i] = x[from + i] * w;
    }
    fft(&mut re, &mut im);
    let mag: Vec<f32> = (0..n / 2).map(|i| (re[i] * re[i] + im[i] * im[i]).sqrt()).collect();
    Spectrum { mag, bin_hz: SR / n as f32 }
}

impl Spectrum {
    fn at(&self, f: f32) -> f32 {
        let p = f / self.bin_hz;
        let i = p.floor() as usize;
        if i + 2 >= self.mag.len() {
            return 0.0;
        }
        // linear between the two bins around f
        let fr = p - i as f32;
        self.mag[i] * (1.0 - fr) + self.mag[i + 1] * fr
    }
}

/// (pitch error in cents against `expect` folded to the nearest octave, harmonic strength, f0 found, the octave of `expect` it sits in)
fn find_pitch(sp: &Spectrum, expect: f32) -> (f32, f32, f32, f32) {
    let mut best = (0.0f32, -1.0f32, expect, expect);
    for oct in -2i32..=2 {
        let centre = expect * 2f32.powi(oct);
        if !(30.0..4000.0).contains(&centre) {
            continue;
        }
        let mut c = -60.0f32;
        while c <= 60.0 {
            let f = centre * 2f32.powf(c / 1200.0);
            let mut s = 0.0;
            for h in 1..=8 {
                if f * h as f32 > 12000.0 {
                    break;
                }
                let m = sp.at(f * h as f32);
                s += m * m / h as f32;
            }
            if s > best.1 {
                best = (c, s, f, centre);
            }
            c += 1.0;
        }
    }
    (best.0, best.1, best.2, best.3)
}

/// Where the energy around each of the first harmonics really sits, against where an equal-tempered note puts it: the average over the
/// harmonics, in cents. Unison detune and chorus spread a partial evenly around its place and leave this near zero; a different tuning
/// table moves the whole cluster.
fn cluster_dev(sp: &Spectrum, centre: f32) -> f32 {
    let (mut num, mut den) = (0.0f32, 1e-12f32);
    for h in 1..=6 {
        let target = centre * h as f32;
        if target > 9000.0 {
            break;
        }
        let (a, b) = (((target * 2f32.powf(-70.0 / 1200.0)) / sp.bin_hz) as usize, ((target * 2f32.powf(70.0 / 1200.0)) / sp.bin_hz) as usize + 1);
        for i in a..b.min(sp.mag.len()) {
            let e = sp.mag[i] * sp.mag[i];
            num += e * 1200.0 * (i as f32 * sp.bin_hz / target).log2();
            den += e;
        }
    }
    num / den
}

fn analyse_note(x: &[f32], key: u8) -> (f32, f32, bool, f32) {
    let start = (0.3 * SR) as usize;
    let sp = spectrum(x, start, (0.9 * SR) as usize);
    let expect = hz(key);
    let (_peak_cents, strength, f0, centre) = find_pitch(&sp, expect);
    let cents = cluster_dev(&sp, centre);
    // total energy in the band the ear uses for pitch, against the energy on the first eight harmonics
    let lo = (60.0 / sp.bin_hz) as usize;
    let hi = ((9000.0 / sp.bin_hz) as usize).min(sp.mag.len() - 1);
    let total: f32 = (sp.mag[lo..hi].iter().map(|m| m * m).sum::<f32>() / 6.0).max(1e-12);   // zero padding spreads each peak over ~6 bins
    let mut on = 0.0;
    for h in 1..=8 {
        let m = sp.at(f0 * h as f32);
        on += m * m * 3.0;
    }
    let pitched = strength > 0.0 && on / total > 0.05;
    let _ = strength;
    // peaks: local maxima within 40 dB of the strongest; how many sit away from a multiple of f0
    let top = sp.mag[lo..hi].iter().cloned().fold(0.0, f32::max).max(1e-9);
    let (mut harm_e, mut off_e) = (0.0f32, 0.0f32);
    for i in lo.max(2)..hi.saturating_sub(2) {
        let m = sp.mag[i];
        if m < top * 0.01 || m < sp.mag[i - 1] || m < sp.mag[i + 1] || m < sp.mag[i - 2] || m < sp.mag[i + 2] {
            continue;
        }
        let f = i as f32 * sp.bin_hz;
        let ratio = f / f0;
        let h = ratio.round().max(1.0);
        let off_cents = 1200.0 * (ratio / h).log2().abs();
        if off_cents <= 60.0 {
            harm_e += m * m;
        } else {
            off_e += m * m;
        }
    }
    let inharm = if harm_e + off_e > 0.0 { off_e / (harm_e + off_e) } else { 0.0 };
    (cents, inharm, pitched, centroid(&sp, 100.0, 8000.0))
}

fn centroid(sp: &Spectrum, a: f32, b: f32) -> f32 {
    let (lo, hi) = ((a / sp.bin_hz) as usize, ((b / sp.bin_hz) as usize).min(sp.mag.len() - 1));
    let (mut num, mut den) = (0.0f32, 1e-12f32);
    for i in lo..hi {
        num += i as f32 * sp.bin_hz * sp.mag[i];
        den += sp.mag[i];
    }
    num / den
}

fn rms_env(x: &[f32]) -> Vec<f32> {
    let w = (SR * 0.005) as usize;
    x.chunks(w).map(|c| (c.iter().map(|s| s * s).sum::<f32>() / c.len() as f32).sqrt()).collect()
}

/// Play a patch's instrument and measure it. `None` for drums and instruments that cannot be built offline (soundfonts are taken as tuned).
pub fn measure(p: &patch::Patch) -> Option<Profile> {
    if p.kind == "drums" || p.kind == "sf2" || p.kind == "sampler" || p.kind == "script" {
        return None;
    }
    let mut inst = patch::build(p, SR)?;
    let mut worst = 0.0f32;
    let (mut inh, mut pitched_n, mut cen_c4) = (0.0f32, 0usize, 1000.0f32);
    let mut env_c4: Vec<f32> = vec![];
    // C, E, G and A in two octaves: a tuning table that is not twelve-tone shows on the third and the fifth
    for &key in &[48u8, 60, 64, 67, 69, 72] {
        let x = play(inst.as_mut(), key, 1.6);
        let (cents, inharm, pitched, cen) = analyse_note(&x, key);
        if pitched {
            pitched_n += 1;
            worst = worst.max(cents.abs());
            inh = inh.max(inharm);
        }
        if key == 60 {
            cen_c4 = cen;
            env_c4 = rms_env(&x);
        }
    }
    let pitched = pitched_n >= 4;
    let peak = env_c4.iter().cloned().fold(1e-9, f32::max);
    let attack_ms = env_c4.iter().position(|e| *e >= peak * 0.9).map(|i| i as f32 * 5.0).unwrap_or(0.0);
    let at = |t: f32| env_c4.get((t / 0.005) as usize).copied().unwrap_or(0.0) / peak;
    let sustain = at(1.0).clamp(0.0, 1.0);
    // low end on C2
    let x = play(inst.as_mut(), 36, 1.0);
    let sp = spectrum(&x, (0.2 * SR) as usize, (0.7 * SR) as usize);
    let tot: f32 = sp.mag.iter().map(|m| m * m).sum::<f32>().max(1e-12);
    let low: f32 = sp.mag[..((250.0 / sp.bin_hz) as usize).min(sp.mag.len())].iter().map(|m| m * m).sum();
    Some(Profile { tune_dev: if pitched { worst } else { 0.0 }, inharm: inh, pitched, centroid: cen_c4, attack_ms, sustain, lows: low / tot })
}

fn cache() -> &'static Mutex<HashMap<String, Option<Profile>>> {
    static C: OnceLock<Mutex<HashMap<String, Option<Profile>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// measurements are kept between runs in cache/soundcat.tsv, keyed by the patch file's size and date (a changed patch is measured again)
const DISK: &str = "cache/soundcat.tsv";

fn stamp(path: &str) -> String {
    match std::fs::metadata(path) {
        Ok(m) => format!("{}:{}", m.len(), m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)),
        Err(_) => String::new(),
    }
}

fn disk_load() -> HashMap<String, (String, Option<Profile>)> {
    let mut m = HashMap::new();
    for line in std::fs::read_to_string(DISK).unwrap_or_default().lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 3 {
            continue;
        }
        let p = if f[2] == "none" {
            None
        } else if f.len() >= 10 {
            let n = |i: usize| f[i].parse::<f32>().unwrap_or(0.0);
            Some(Profile { tune_dev: n(2), inharm: n(3), pitched: f[4] == "1", centroid: n(5), attack_ms: n(6), sustain: n(7), lows: n(8) })
        } else {
            continue;
        };
        m.insert(f[0].to_string(), (f[1].to_string(), p));
    }
    m
}

fn disk_append(key: &str, stamp: &str, p: &Option<Profile>) {
    use std::io::Write;
    let line = match p {
        Some(p) => format!("{key}\t{stamp}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\tv1\n", p.tune_dev, p.inharm, p.pitched as u8, p.centroid, p.attack_ms, p.sustain, p.lows),
        None => format!("{key}\t{stamp}\tnone\n"),
    };
    let _ = std::fs::create_dir_all("cache");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(DISK) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// The profile of a patch file (measured once, then remembered, also on disk; `None` when it has none: drums, soundfonts, a file that cannot be read).
pub fn profile_file(path: &str) -> Option<Profile> {
    let key = path.replace('\\', "/");
    if let Some(p) = cache().lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return p.clone();
    }
    static DISKMAP: OnceLock<HashMap<String, (String, Option<Profile>)>> = OnceLock::new();
    let st = stamp(path);
    if let Some((s, p)) = DISKMAP.get_or_init(disk_load).get(&key) {
        if *s == st && !st.is_empty() {
            cache().lock().unwrap_or_else(|e| e.into_inner()).insert(key, p.clone());
            return p.clone();
        }
    }
    let p = std::fs::read_to_string(path).ok().and_then(|t| measure(&patch::parse(&t)));
    cache().lock().unwrap_or_else(|e| e.into_inner()).insert(key.clone(), p.clone());
    if !st.is_empty() {
        disk_append(&key, &st, &p);
    }
    p
}

/// The plugin a role table names, in words.
fn plugin_label(plugin: &str) -> &'static str {
    match plugin {
        "prism" => "Ling Prism",
        "growl" => "Ling Growl",
        "synth" => "Ling Synth X",
        "pluck" => "Ling Pluck",
        "voice" => "Ling Voice",
        "drums" => "Ling Membrane",
        "auto" => "General MIDI soundfont",
        _ => "a sampler",
    }
}

/// Everything the model should know before it names roles: what each plugin is for, then for every role the instrument that would play it in
/// this genre and how that instrument measured (in tune, attack, sustain, brightness). Only tuned, non-clashing patches ever appear here.
pub fn palette(genre: &str) -> String {
    let mut s = digest();
    s.push_str(role_notes());
    s.push_str("\nWhat each role will sound like in this song (every one of these is in tune):\n");
    for role in crate::ai_import::ROLES {
        let (plugin, name) = crate::ai_import::pick(genre, role, 0, 120.0);
        if plugin == "auto" {
            s.push_str(&format!("- {role}: {} (real sampled instruments)\n", plugin_label(plugin)));
        } else {
            let tags = crate::api::song_find_patch(plugin, name).and_then(|p| profile_file(&p)).map(|p| format!(" ({})", p.tags().into_iter().filter(|t| *t != "in tune").collect::<Vec<_>>().join(", "))).unwrap_or_default();
            s.push_str(&format!("- {role}: {} \"{name}\"{tags}\n", plugin_label(plugin)));
        }
    }
    s
}

/// May this patch play `role` in a song written in twelve-tone equal temperament? Unmeasured patches (drums, soundfonts) pass.
pub fn ok_for(role: &str, path: &str) -> bool {
    let Some(p) = profile_file(path) else { return true };
    let r = role.to_lowercase();
    // noise, risers, drums and sung parts are judged on pitch alone (and only when they do have one)
    if matches!(r.as_str(), "texture" | "riser" | "drums") {
        return !p.pitched || p.tune_dev <= 25.0;
    }
    if r == "bell" {
        return p.pitched && p.tune_dev <= 15.0;
    }
    if r.contains("vocal") || r == "voice" || r == "choir" {
        return !p.pitched || p.tune_dev <= 20.0;
    }
    // electric pianos ring with bell-like tines and an electro lead may be harsh: both are fine as long as they are in tune
    let limit = match r.as_str() {
        "keys" | "piano" | "stab chord" => 0.8,
        "laser lead" => 0.9,
        _ => 0.6,
    };
    p.tuned() && p.inharm <= limit
}

/// May the patch named in a role table (plugin, name as the dialogs list it) play `role`? Names that cannot be resolved to a file
/// (soundfont programs, "auto") pass: they are General MIDI, tuned.
pub fn pair_ok(role: &str, plugin: &str, name: &str) -> bool {
    match crate::api::song_find_patch(plugin, name) {
        Some(path) => ok_for(role, &path),
        None => true,
    }
}

/// the entries of a role table that are safe, in order; when none is, the table unchanged (better an odd sound than none)
pub fn safe_list(role: &str, list: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
    let v: Vec<_> = list.iter().copied().filter(|(p, n)| pair_ok(role, p, n)).collect();
    if v.is_empty() { list.to_vec() } else { v }
}

/// words about a patch for the model / the dialog: "Supersaw Lead (in tune, sharp attack, sustained, bright)"
pub fn describe(name: &str, path: &str) -> String {
    match profile_file(path) {
        Some(p) => format!("{name} ({})", p.tags().join(", ")),
        None => name.to_string(),
    }
}

// ── the plugins ─────────────────────────────────────────────────────────────────

/// What each plugin is, what it does well and what to keep it away from: the language model reads this before it picks instruments.
pub const PLUGINS: [(&str, &str); 9] = [
    ("prism", "Ling Prism: polyphonic analog-style synth (two morphing wavetable oscillators with unison, ring/FM/sync, sub, filter, envelopes, built-in chorus/delay/reverb). Hundreds of patches: supersaw and hoover leads, trance/ambient/string pads, sub and rolling basses, plucks, arps, keys, bells. The workhorse for electronic music. Some patches use other tunings (EDO, slendro, just) and are not safe in ordinary songs."),
    ("growl", "Ling Growl: monophonic bass/lead for dubstep, drum and bass and electro: wobble, reese, neuro, hard-sync screech, talking vowel bass. One note at a time: bass lines and lead lines only, never chords or pads."),
    ("synth", "Ling Synth X: polyphonic subtractive + additive + FM + string synth that behaves like a General MIDI set: e-pianos, house pianos, organs, brass, string ensembles, plucked guitars and basses, simple square/sine/breath leads. Good where a recognisable keyboard or acoustic-like sound is wanted."),
    ("pluck", "Ling Pluck: physical-modelling plucked and struck strings (guitar, harp, koto, sitar-style buzz, bass) with a wooden body and sympathetic strings. Polyphonic. Natural, decaying attacks: folk, acoustic, world and soft parts."),
    ("voice", "Ling Voice: formant singer (aria, mei, kai, ren, choir) that sings lyrics from IPA; robotic but intelligible. For vocal lines and choir pads, one voice per part."),
    ("drums", "Ling Membrane: physically modelled drum kits (kick, snare, hats, clap, toms, percussion) for each genre; unpitched."),
    ("sf2", "SoundFont / General MIDI player: sampled acoustic instruments (piano, strings, brass, woodwinds, guitars, choir, orchestral percussion). Best for orchestral, folk, jazz and film music where real instruments are expected."),
    ("sampler", "Sampler: plays audio files across the keyboard (drum hits, one-shots, tracker samples); needs audio files, not for generated synth parts."),
    ("script", "Script synth: a user-written instrument; unknown sound, never chosen automatically."),
];

/// A short text for the model: the plugins and what to use them for, then the rule that matters most.
pub fn digest() -> String {
    let mut s = String::from("Instruments available (choose a role; each role is played by the plugin that suits it):\n");
    for (k, d) in PLUGINS.iter() {
        if matches!(*k, "sampler" | "script") {
            continue;
        }
        s.push_str(&format!("- {d}\n"));
    }
    s.push_str("Every part must be in tune with the song's twelve-tone scale and must not clash with the chords: never choose microtonal, detuned or clangorous sounds for harmony parts.\n");
    s
}

/// the role names with what they sound like, for the model
pub fn role_notes() -> &'static str {
    "Roles: drums = a drum kit; sub bass = deep sine/triangle low end; rolling bass = moving synth bass line; wobble bass / growl bass = dubstep and DnB bass (single notes); acid bass = squelchy resonant bass; pluck bass = short plucked bass; supersaw lead = wide bright trance/pop lead; synth lead = simple square/saw lead; laser lead = sharp electro lead; vocal lead = voice-like lead; pluck = short plucked chord/arp sound; arp = bright repeating arpeggio; warm pad = soft sustained chords; dark pad = low brooding sustained chords; ambient pad = slow wide texture chords; strings = string ensemble chords; brass = brass stabs/lines; stab chord = short chord hits; keys = electric piano/keys; piano = acoustic-style piano; organ = organ chords; bell = bell/glass tones; choir = sung 'ah' chords; voice = a sung line; riser = rising noise sweep before a drop; texture = quiet noise/atmosphere; guitar = plucked guitar; woodwind = flute/clarinet-like line; hyper lead = very bright pop lead."
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lsx() -> String {
        std::env::var("LSX_DIR").unwrap_or_else(|_| "../../../LingStationX".into())
    }

    #[test]
    fn a_plain_saw_is_in_tune_and_harmonic() {
        let text = "name = \"saw\"\ntype = prism\n";
        let p = measure(&patch::parse(text)).expect("prism builds");
        assert!(p.pitched, "{p:?}");
        assert!(p.tune_dev < 8.0, "{p:?}");
        assert!(p.inharm < 0.5, "{p:?}");
    }

    /// `LSX_SCAN=1 cargo test -p ling-daw scan_patches -- --nocapture --ignored`: a table of every synth patch
    #[test]
    #[ignore]
    fn scan_patches() {
        let dir = lsx();
        for sub in ["Prism", "Growl", "Voice"] {
            let mut files: Vec<_> = std::fs::read_dir(format!("{dir}/instruments/{sub}")).unwrap().flatten().map(|e| e.path()).collect();
            files.sort();
            for f in files {
                let path = f.to_string_lossy().to_string();
                if let Some(p) = profile_file(&path) {
                    println!("{sub:6} {:34} tune {:6.1}c inharm {:.2} pitched {} cen {:5.0} atk {:4.0}ms sus {:.2} low {:.2}  ok:{}", f.file_stem().unwrap().to_string_lossy(), p.tune_dev, p.inharm, p.pitched, p.centroid, p.attack_ms, p.sustain, p.lows, ok_for("warm pad", &path));
                }
            }
        }
    }
}

#[cfg(test)]
mod table_tests {
    use super::*;

    /// `cargo test -p ling-daw --release --lib table_candidates -- --nocapture --ignored`: which entries of the role tables are not safe
    #[test]
    #[ignore]
    fn table_candidates() {
        std::env::set_current_dir(std::env::var("LSX_DIR").unwrap_or_else(|_| "../../../LingStationX".into())).unwrap();
        for role in crate::ai_import::ROLES {
            for (plugin, name) in crate::ai_import::candidates_for_tests(role) {
                if !pair_ok(role, plugin, name) {
                    let path = crate::api::song_find_patch(plugin, name).unwrap_or_default();
                    println!("UNSAFE  {role:14} {plugin:6} {name:28} {:?}", profile_file(&path));
                }
            }
        }
        for genre in crate::ai_import::GENRES {
            for role in crate::ai_import::ROLES {
                if let Some(l) = crate::ai_import::genre_candidates(genre, role) {
                    for (plugin, name) in l {
                        if !pair_ok(role, plugin, name) {
                            println!("UNSAFE  {genre:12} {role:14} {plugin:6} {name}");
                        }
                    }
                }
            }
        }
    }
}
