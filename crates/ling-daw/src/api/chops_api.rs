//! Vocal Chops (`daw_chopx_*`): chop a vocal at its syllables and arrange the chops EDM style over the project's sections.
//!
//! The vocal is a track: an audio track made by SingTTS / Voice Skin (its source MIDI part, still in the project, says which syllable
//! starts where), a Ling Voice track (bounced alone to a WAV next to the project), or any audio (cut at its transients). The cutting
//! and the arrangement are `chops.rs` (`Vocal::from_notes`, `compose_edm`); this file reads the project, writes the chop tracks -
//! Ling Slicer kits (chops/kit.rs: one slice per key, every syllable the arrangement plays, tuned by the Slicer to the chords at
//! the beat each note plays; [`Daw::chops_on_slicer`]), or with `slicer=0` the Sampler tracks of old (one zone holds the whole
//! vocal, a note's lyric "@start:end:cents" plays a slice; projects that have them still load and play them) - the Vox Filter
//! lane of the builds and the sidechain from the kick. The production edits (chops/edits.rs, option `edits=`) are planned over
//! the song's sections and put on the composed chops; their risers, pads, gates, doubles and ear candy play on a third track,
//! "Vocal Chops edits", their sweeps and throws are lanes on the chop tracks.
use super::{Daw, A, Val};
use crate::chops as ch;
use crate::model::*;
use std::path::{Path, PathBuf};

/// "a=1;b=two" -> lookup
struct Opt(Vec<(String, String)>);
impl Opt {
    fn parse(s: &str) -> Opt {
        Opt(s.split(|c| c == ';' || c == '\n').filter_map(|p| p.split_once('=')).map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string())).collect())
    }
    fn s(&self, k: &str) -> Option<&str> {
        self.0.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str())
    }
    fn f(&self, k: &str, d: f64) -> f64 {
        self.s(k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(d)
    }
    fn b(&self, k: &str, d: bool) -> bool {
        match self.s(k) {
            Some(v) => !matches!(v.to_lowercase().as_str(), "0" | "false" | "no" | "off"),
            None => d,
        }
    }
}

/// the sung notes of a track: (absolute beat, beats, key, written syllable) and whether the clips keep their lyric text
fn sung_notes(en: &crate::engine::Engine, t: usize) -> (Vec<(f64, f64, u8, String)>, bool) {
    let mut v: Vec<(f64, f64, u8, String)> = vec![];
    let mut texted = false;
    let Some(tr) = en.project.tracks.get(t) else { return (v, false) };
    for c in &tr.clips {
        let ClipData::Midi(ns) = &c.data else { continue };
        let ns: Vec<&Note> = ns.iter().filter(|n| n.start < c.len).collect();
        if !c.lyric_text.trim().is_empty() {
            texted = true;
            let opts = crate::lyricfit::Opts::parse(&c.lyric_lang).unwrap_or_default();
            let input: Vec<crate::lyricfit::NoteIn> = ns.iter().map(|n| crate::lyricfit::NoteIn { start: n.start, len: n.len, key: n.key }).collect();
            let words = crate::lyricfit::fit_text(&input, &c.lyric_text, &opts);
            for (n, w) in ns.iter().zip(words) {
                if !w.trim().is_empty() {
                    v.push((c.start + n.start, n.len.min(c.len - n.start), n.key, w));
                }
            }
        } else {
            // only the lyric of the note itself (IPA of a Ling Voice part): a note is a syllable, a word
            for n in ns {
                if !n.lyric.trim().is_empty() {
                    v.push((c.start + n.start, n.len.min(c.len - n.start), n.key, n.lyric.clone()));
                }
            }
        }
    }
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    (v, texted)
}

/// the MIDI track an audio track was sung from: the track with lyrics whose first note starts where the audio does (SingTTS puts the
/// audio `lead` before the first note; a Voice Skin starts at the part), best when the audio's name starts with the track's
fn paired_midi(en: &crate::engine::Engine, audio: usize) -> Option<usize> {
    let tr = en.project.tracks.get(audio)?;
    let clip = tr.clips.iter().find(|c| matches!(c.data, ClipData::Audio { .. }))?;
    let lead = 0.2 * en.project.bpm / 60.0;
    let mut best: Option<(f64, usize)> = None;
    for (i, o) in en.project.tracks.iter().enumerate() {
        if i == audio || !matches!(o.kind, TrackKind::Instrument) {
            continue;
        }
        let (notes, _) = sung_notes(en, i);
        let Some(first) = notes.first() else { continue };
        let d = (first.0 - clip.start).abs().min((first.0 - clip.start - lead).abs());
        if d > 0.75 {
            continue;
        }
        let named = tr.name.starts_with(&o.name) || clip.name.starts_with(&o.name);
        let score = d - if named { 1.0 } else { 0.0 };
        if best.map(|b| score < b.0).unwrap_or(true) {
            best = Some((score, i));
        }
    }
    best.map(|b| b.1)
}

/// the sections of the project: ("sections" meta), else found from the notes, else the whole song
fn sections_of(en: &crate::engine::Engine) -> Vec<(f64, f64, String)> {
    let mut v: Vec<(f64, f64, String)> = vec![];
    if let Some((_, text)) = en.project.meta.iter().find(|(k, _)| k == "sections") {
        for item in text.split(';') {
            let f: Vec<&str> = item.split(':').collect();
            if f.len() >= 3 {
                if let (Ok(a), Ok(b)) = (f[0].parse::<f64>(), f[1].parse::<f64>()) {
                    if b > a {
                        v.push((a, b, f[2].to_string()));
                    }
                }
            }
        }
    }
    if v.is_empty() {
        let end = en.project.end_beat();
        if end >= 16.0 {
            v = crate::feel::infer_sections(&en.project.tracks, end).into_iter().map(|s| (s.start, s.end, s.tag)).filter(|s| s.1 > s.0).collect();
        }
    }
    if v.is_empty() {
        v.push((0.0, en.project.end_beat().max(16.0), "chorus".into()));
    }
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v
}

/// does a section pass the "which sections" option: "auto" = drops, choruses, builds, breaks; "all"; or tags to match ("chorus,drop")
fn wanted(tag: &str, which: &str, density: f64) -> bool {
    let w = which.trim().to_lowercase();
    let t = tag.to_lowercase();
    match w.as_str() {
        "" | "auto" => ch::role_of(tag) != ch::Role::Calm || density >= 0.75,
        "all" | "*" => true,
        _ => w.split(|c| c == ',' || c == ' ').filter(|x| !x.is_empty()).any(|x| t.contains(x)),
    }
}

/// interleaved stereo as a 16-bit WAV
fn write_wav(path: &Path, stereo: &[f32], rate: u32) -> Result<(), String> {
    let spec = hound::WavSpec { channels: 2, sample_rate: rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;
    for x in stereo {
        w.write_sample((x.clamp(-1.0, 1.0) * 32767.0) as i16).map_err(|e| e.to_string())?;
    }
    w.finalize().map_err(|e| e.to_string())
}

/// every note of the tracks `which` accepts, absolute beats, loops unrolled
fn notes_of(en: &crate::engine::Engine, which: impl Fn(usize, &Track) -> bool) -> Vec<Note> {
    let mut v = vec![];
    for (i, tr) in en.project.tracks.iter().enumerate() {
        if !which(i, tr) {
            continue;
        }
        for c in &tr.clips {
            let ClipData::Midi(ns) = &c.data else { continue };
            let ll = if c.loop_len >= 0.25 && c.loop_len < c.len { c.loop_len } else { 0.0 };
            let reps = if ll > 0.0 { ((c.len / ll).ceil() as usize).min(512) } else { 1 };
            for r in 0..reps {
                let base = c.start + r as f64 * ll;
                for n in ns {
                    if ll > 0.0 && n.start >= ll {
                        continue;
                    }
                    let at = base + n.start;
                    if at < c.start + c.len - 1e-9 {
                        let mut m = n.clone();
                        m.start = at;
                        v.push(m);
                    }
                }
            }
        }
    }
    v
}

/// does a track sing (notes with lyrics, or an audio track a sung part was skinned / rendered onto)
fn is_vocal(en: &crate::engine::Engine, i: usize) -> bool {
    let Some(tr) = en.project.tracks.get(i) else { return false };
    match tr.kind {
        TrackKind::Audio => false,
        TrackKind::Instrument => tr.clips.iter().any(|c| !c.lyric_text.trim().is_empty() || matches!(&c.data, ClipData::Midi(ns) if ns.iter().any(|n| !n.lyric.trim().is_empty() && !n.lyric.starts_with('@')))),
    }
}

impl Daw {
    /// The chop tracks `tracks` (notes with Sampler slice lyrics over the bank `file`, laid out as `lay` says) moved onto Ling Slicer
    /// kits of `file` (chops/kit.rs), tuned by the Slicer in the chops' tuning mode on the song's harmony (read as the Slicer reads
    /// it: every harmonic track but the chops), then synced (the shifted slices rendered ahead). -> (slices, notes, variants merged)
    pub(super) fn chops_on_slicer(&mut self, tracks: &[usize], file: &str, lay: &ch::kit::Layout, tune: &ch::Tune) -> (usize, usize, usize) {
        let sr = self.out.sr;
        let Ok((st, rate)) = crate::dsp::sampler::load_audio(file) else { return (0, 0, 0) };
        let mono = crate::dsp::slicer::mono_of(&st);
        let mode = ch::kit::tune_mode(tune);
        let mut tot = (0usize, 0usize, 0usize);
        {
            let mut en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
            // the chop tracks are Slicers from here on: none of them is harmony for the others
            for t in tracks {
                if let Some(tr) = en.project.tracks.get_mut(*t) {
                    tr.inst_kind = "slicer".into();
                }
            }
            for t in tracks {
                let tl = crate::dsp::slicer::timeline(&super::slicer_api::harmony_for(&en.project, *t, 0), 4.0);
                let Some(tr) = en.project.tracks.get_mut(*t) else { continue };
                let notes: Vec<(f64, Note)> = tr.clips.iter().flat_map(|c| match &c.data {
                    ClipData::Midi(ns) => ns.iter().map(|n| (c.start + n.start, n.clone())).collect::<Vec<_>>(),
                    _ => vec![],
                }).collect();
                let input: Vec<(f64, &Note)> = notes.iter().map(|(b, n)| (*b, n)).collect();
                let kit = ch::kit::build(&input, lay, &mono, rate, &tl, mode, ch::kit::ROOT);
                let mut k = 0usize;
                for c in tr.clips.iter_mut() {
                    let ClipData::Midi(ns) = &mut c.data else { continue };
                    let mut keep: Vec<Note> = vec![];
                    for n in ns.drain(..) {
                        if let Some(key) = kit.keys.get(k).copied().flatten() {
                            keep.push(Note { key, lyric: String::new(), ..n });
                        }
                        k += 1;
                    }
                    *ns = keep;
                }
                tot.0 += kit.slices.len();
                tot.1 += kit.keys.iter().flatten().count();
                tot.2 += kit.merged;
                tr.inst = Some(Box::new(crate::dsp::slicer::Slicer::with_slices(sr, file, kit.slices, &ch::kit::params(mode))));
            }
        }
        for t in tracks {
            let _ = self.slc_sync(*t);
        }
        tot
    }

    pub(super) fn chopx_dispatch(&mut self, name: &str, a: &A) -> Result<Val, String> {
        let eng = self.out.engine.clone();
        macro_rules! ok {
            ($v:expr) => {
                return Ok(Val::from($v))
            };
        }
        match name {
            "daw_chopx_info" => {
                // (track) -> [kind "audio" | "midi" | "", sung notes, source MIDI track of an audio vocal (-1 none), lyrics known 0/1, name]
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let t = a.u(0);
                let Some(tr) = en.project.tracks.get(t) else { return ok!(Val::List(vec![])) };
                let (kind, src) = match tr.kind {
                    TrackKind::Audio => ("audio", paired_midi(&en, t)),
                    TrackKind::Instrument => ("midi", Some(t)),
                };
                let (notes, texted) = src.map(|s| sung_notes(&en, s)).unwrap_or((vec![], false));
                let has_audio = tr.clips.iter().any(|c| matches!(c.data, ClipData::Audio { .. }));
                let usable = (kind == "audio" && has_audio) || (kind == "midi" && !notes.is_empty());
                ok!(Val::List(vec![
                    Val::Str(if usable { kind.to_string() } else { String::new() }),
                    Val::Num(notes.len() as f64),
                    Val::Num(if kind == "audio" { src.map(|s| s as f64).unwrap_or(-1.0) } else { -1.0 }),
                    Val::Num((texted || notes.iter().any(|n| n.3.starts_with(' '))) as u8 as f64),
                    Val::Str(tr.name.clone()),
                ]))
            }
            "daw_chopx_vocals" => {
                // () -> [track index, name, kind]* of the tracks that sing (a Ling Voice part or an audio track skinned / sung from one)
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let mut v = vec![];
                for i in 0..en.project.tracks.len() {
                    let tr = &en.project.tracks[i];
                    let is_chop = tr.name.starts_with("Vocal Chops");
                    if is_chop {
                        continue;
                    }
                    let kind = if is_vocal(&en, i) {
                        "midi"
                    } else if matches!(tr.kind, TrackKind::Audio) && tr.clips.iter().any(|c| matches!(c.data, ClipData::Audio { .. })) {
                        "audio"
                    } else {
                        continue;
                    };
                    v.extend([Val::Num(i as f64), Val::Str(tr.name.clone()), Val::Str(kind.to_string())]);
                }
                ok!(Val::List(v))
            }
            "daw_chopx_sections" => {
                // () -> [start beat, end beat, tag, role "hot" | "build" | "break" | "calm", energy 0..1]*
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let mut v = vec![];
                for (s, e, tag) in sections_of(&en) {
                    let role = match ch::role_of(&tag) {
                        ch::Role::Hot => "hot",
                        ch::Role::Build => "build",
                        ch::Role::Break => "break",
                        ch::Role::Calm => "calm",
                    };
                    v.extend([Val::Num(s), Val::Num(e), Val::Str(tag.clone()), Val::Str(role.into()), Val::Num(ch::tag_energy(&tag))]);
                }
                ok!(Val::List(v))
            }
            "daw_chopx_make" => {
                // (vocal track, options "density=0.6;stutter=0.5;build=dj|stutter|off;sections=auto|all|chorus,drop;layer=1;reverse=1;dry=1;
                //  riser=0;duck=0.4;seed=1;tune=off|key|chords|fine;tunefine=0..1;tunemax=5;genre=<the song's>;slicer=1;
                //  edits=auto|light|more|off|<list: swell,tapestop,...>") -> a sentence about what was made, or "" (the status line says
                //  why). slicer=0: the chops on Sampler tracks, as before Ling Slicer.
                //  An option this build does not know is ignored (Opt only looks up the keys it reads).
                let t = a.u(0);
                let o = Opt::parse(&a.s(1));
                let seed = o.f("seed", 1.0).max(0.0) as u64;
                let on_slicer = o.b("slicer", true);
                let genre = match o.s("genre") {
                    Some(g) if !g.is_empty() => g.to_string(),
                    _ => {
                        let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                        en.project.meta.iter().find(|(k, _)| k == "ai.genre").map(|(_, v)| v.clone()).unwrap_or_default()
                    }
                };
                let edm = ch::Edm {
                    density: o.f("density", 0.6).clamp(0.0, 1.0),
                    stutter: o.f("stutter", 0.5).clamp(0.0, 1.0),
                    build: ch::Build::parse(o.s("build").unwrap_or("dj")),
                    layer: o.b("layer", true),
                    reverse: o.b("reverse", true),
                    dry: o.b("dry", true),
                    riser: o.b("riser", false),
                    tune: ch::Tune::parse(o.s("tune").unwrap_or("fine"), o.s("tunefine").and_then(|v| v.parse().ok()), o.s("tunemax").and_then(|v| v.parse().ok())),
                    genre: genre.clone(),
                };
                let which = o.s("sections").unwrap_or("auto").to_string();
                let edits = ch::edits::Which::parse(o.s("edits").unwrap_or("auto"));
                let duck = o.f("duck", 0.4).clamp(0.0, 1.0);
                let sr = self.out.sr;
                // ── the vocal: audio and, when known, its syllables ──
                struct Src {
                    name: String,
                    stereo: Vec<f32>,
                    rate: u32,
                    notes: Vec<ch::VNote>,
                    origin: f64,
                    how: &'static str,
                    bpm: f64,
                }
                let src: Src = {
                    let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    let Some(tr) = en.project.tracks.get(t) else {
                        self.status = "Vocal chops: choose a vocal track".into();
                        ok!("")
                    };
                    let bpm = en.project.bpm;
                    let tname = tr.name.clone();
                    match tr.kind {
                        TrackKind::Audio => {
                            let Some(clip) = tr.clips.iter().find(|c| matches!(c.data, ClipData::Audio { .. })) else {
                                self.status = "Vocal chops: the track has no audio".into();
                                ok!("")
                            };
                            let ClipData::Audio { sample, offset_s, pitch, warp, .. } = &clip.data else { ok!("") };
                            let (stereo, rate, off0, cs) = (sample.stereo.clone(), sample.rate, *offset_s, clip.start);
                            let plain = pitch.abs() < 1e-6 && warp.abs() < 1e-6;
                            let src_t = if plain { paired_midi(&en, t) } else { None };
                            let mut notes = vec![];
                            if let Some(s) = src_t {
                                let (ns, _) = sung_notes(&en, s);
                                notes = ns.iter().map(|n| ch::VNote { start: n.0, len: n.1, key: n.2, text: n.3.clone(), s0: en.project.beats_to_secs(cs, n.0) + off0, s1: en.project.beats_to_secs(cs, n.0 + n.1) + off0 }).collect();
                            }
                            let how = if notes.is_empty() { "audio, cut at its transients" } else { "audio with the syllables of its MIDI part" };
                            Src { name: tname, stereo, rate, notes, origin: cs - off0 * bpm / 60.0, how, bpm }
                        }
                        TrackKind::Instrument => {
                            let (ns, _) = sung_notes(&en, t);
                            if ns.is_empty() {
                                self.status = "Vocal chops: the track has no sung notes (give the notes lyrics first)".into();
                                ok!("")
                            }
                            let b0 = (ns[0].0 - 0.5).max(0.0);
                            let b1 = ns.iter().map(|n| n.0 + n.1).fold(0.0, f64::max) + 0.5;
                            if en.project.beats_to_secs(b0, b1) > 1800.0 {
                                self.status = "Vocal chops: that vocal is over 30 minutes".into();
                                ok!("")
                            }
                            // the part alone, as it sounds on its track
                            let saved: Vec<(bool, bool)> = en.project.tracks.iter().map(|x| (x.mute, x.solo)).collect();
                            for (i, x) in en.project.tracks.iter_mut().enumerate() {
                                x.mute = i != t;
                                x.solo = i == t;
                            }
                            let stereo = en.bounce_range(b0, b1, 1.0);
                            for (x, s) in en.project.tracks.iter_mut().zip(saved) {
                                x.mute = s.0;
                                x.solo = s.1;
                            }
                            let rate = en.sr as u32;
                            let notes = ns.iter().map(|n| ch::VNote { start: n.0, len: n.1, key: n.2, text: n.3.clone(), s0: en.project.beats_to_secs(b0, n.0), s1: en.project.beats_to_secs(b0, n.0 + n.1) }).collect();
                            Src { name: tname, stereo, rate, notes, origin: b0, how: "Ling Voice part, bounced alone", bpm }
                        }
                    }
                };
                if src.stereo.len() < 4 || src.stereo.iter().all(|x| x.abs() < 1e-5) {
                    self.status = "Vocal chops: the vocal is silent".into();
                    ok!("")
                }
                // ── the project: sections, drums, harmony, key ──
                let (secs, drums, harmony, key, sec_energy) = {
                    let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    let vocal_tracks: Vec<bool> = (0..en.project.tracks.len()).map(|i| i == t || is_vocal(&en, i) || en.project.tracks[i].name.starts_with("Vocal Chops")).collect();
                    let secs = sections_of(&en);
                    let drums = notes_of(&en, |i, tr| tr.inst_kind == "drums" && !vocal_tracks[i]);
                    let harmony = notes_of(&en, |i, tr| tr.inst_kind != "drums" && !vocal_tracks[i] && matches!(tr.kind, TrackKind::Instrument));
                    let mut h = [0f64; 12];
                    for n in &harmony {
                        h[(n.key % 12) as usize] += n.len.max(0.1);
                    }
                    let key = if h.iter().sum::<f64>() > 1.0 {
                        let k = crate::theory::detect_key_hist(&h).0;
                        (k.tonic, k.minor)
                    } else {
                        let mut h2 = [0f64; 12];
                        for n in &src.notes {
                            h2[(n.key % 12) as usize] += n.len.max(0.1);
                        }
                        let k = crate::theory::detect_key_hist(&h2).0;
                        (k.tonic, k.minor)
                    };
                    // how busy each section is: its tag, and how many notes the other tracks play in it
                    let sec_energy: Vec<f64> = secs
                        .iter()
                        .map(|(s, e, tag)| {
                            let n = drums.iter().chain(harmony.iter()).filter(|n| n.start >= *s && n.start < *e).count() as f64;
                            let npb = n / (e - s).max(1.0);
                            (0.65 * ch::tag_energy(tag) + 0.35 * (npb / 3.0).clamp(0.0, 1.0)).clamp(0.05, 1.0)
                        })
                        .collect();
                    (secs, drums, harmony, key, sec_energy)
                };
                // ── the cut ──
                let hot: Vec<(f64, f64)> = secs.iter().filter(|s| ch::role_of(&s.2) == ch::Role::Hot).map(|s| (s.0, s.1)).collect();
                let mut vocal = if src.notes.is_empty() {
                    ch::Vocal::from_audio(&src.stereo, src.rate, src.bpm, src.origin, &hot)
                } else {
                    // the recording may sit a few ms off the notes: the first onset says by how much
                    let mut notes = src.notes.clone();
                    if let (Some(first), Some(onset)) = (notes.iter().map(|n| n.s0).reduce(f64::min), ch::transients(&src.stereo, src.rate).first().map(|x| x.0 as f64 / src.rate as f64)) {
                        let d = onset - first;
                        if d.abs() > 0.015 && d > -0.1 && d < 0.3 {
                            for n in notes.iter_mut() {
                                n.s0 += d;
                                n.s1 += d;
                            }
                        }
                    }
                    ch::Vocal::from_notes(&src.stereo, src.rate, &notes, src.bpm, &ch::CutOpts::default(), &hot)
                };
                if vocal.pieces.len() < 2 {
                    self.status = "Vocal chops: found nothing to chop in that vocal".into();
                    ok!("")
                }
                let mut bank = vocal.add_reverse(&src.stereo);
                // on the Slicer a reversed chop is the slice played backwards: the file is the recording alone
                let lay = ch::kit::Layout::of_vocal(&vocal, (src.stereo.len() / 2) as u32);
                if on_slicer {
                    bank.truncate(src.stereo.len());
                }
                // a quiet recording (a bounced voice) is brought up to a healthy level so the chops sit in the mix
                let peak = bank.iter().fold(0.0f32, |m, x| m.max(x.abs()));
                if peak > 1e-4 && peak < 0.7 {
                    let g = 0.9 / peak;
                    bank.iter_mut().for_each(|x| *x *= g);
                }
                let dir = if self.path.is_empty() { PathBuf::from("cache/chops") } else { Path::new(&self.path).parent().map(|p| p.join("samples")).unwrap_or_else(|| PathBuf::from("cache/chops")) };
                let _ = std::fs::create_dir_all(&dir);
                let clean: String = src.name.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
                let file = dir.join(format!("chops_{clean}_{}.wav", seed % 1000));
                write_wav(&file, &bank, src.rate)?;
                let file_s = file.to_string_lossy().replace('\\', "/");
                // ── the arrangement ──
                struct Part {
                    start: f64,
                    len: f64,
                    tag: String,
                    out: ch::EdmOut,
                    sec: usize,
                    only_swell: bool,
                    drums: Vec<Note>,
                    harm: Vec<Note>,
                    fx: Vec<Note>,
                }
                let mut parts: Vec<Part> = vec![];
                for (i, (s, e, tag)) in secs.iter().enumerate() {
                    let energy = sec_energy[i];
                    let role = ch::role_of(tag);
                    let next = secs.get(i + 1);
                    let next_hot = next.map(|n| ch::role_of(&n.2) == ch::Role::Hot).unwrap_or(false);
                    // a section with no chops of its own still swells into a chopped one
                    let only_swell = !wanted(tag, &which, edm.density);
                    if only_swell && !(edm.reverse && next.map(|n| matches!(ch::role_of(&n.2), ch::Role::Hot | ch::Role::Build) && wanted(&n.2, &which, edm.density)).unwrap_or(false)) {
                        continue;
                    }
                    let len = e - s;
                    let build_beats = if !only_swell && edm.build != ch::Build::Off && next_hot && role != ch::Role::Hot && role != ch::Role::Build {
                        if len >= 64.0 { 32.0 } else if len >= 32.0 { 16.0 } else if len >= 16.0 { 8.0 } else if len >= 8.0 { 4.0 } else { 0.0 }
                    } else {
                        0.0
                    };
                    let rel = |ns: &[Note], keep: &dyn Fn(&Note) -> bool| -> Vec<Note> {
                        ns.iter()
                            .filter(|n| n.start >= *s - 1e-9 && n.start < *e && keep(n))
                            .map(|n| {
                                let mut m = n.clone();
                                m.start -= *s;
                                m
                            })
                            .collect()
                    };
                    let d = rel(&drums, &|_| true);
                    // every note of the other tracks: the chops are tuned to the chord that sounds (chops.rs chord_at)
                    let h = rel(&harmony, &|_| true);
                    let end_abs = s + len - 0.5;
                    let out = ch::compose_edm(&ch::EdmCtx {
                        beats: len,
                        key,
                        energy,
                        tag,
                        drums: &d,
                        harmony: &h,
                        vocal: &vocal,
                        bpm: src.bpm,
                        seed: seed.wrapping_mul(131).wrapping_add(i as u64),
                        o: &edm,
                        build_beats,
                        next_hot: next_hot || (only_swell && next.is_some()),
                        loop_line: vocal.line_before(end_abs),
                        only_swell,
                    });
                    if !out.main.is_empty() {
                        parts.push(Part { start: *s, len, tag: tag.clone(), out, sec: i, only_swell, drums: d, harm: h, fx: vec![] });
                    }
                }
                if parts.is_empty() {
                    self.status = "Vocal chops: no section to put chops in (tag the sections on the ruler, or choose \"all\")".into();
                    ok!("")
                }
                // ── the production edits: planned over the song's sections, put on each section's chops ──
                let (mut sweeps, mut throws, mut used): (Vec<(f64, f64)>, Vec<(f64, f64)>, Vec<ch::edits::Kind>) = (vec![], vec![], vec![]);
                if edits.on {
                    let content = ch::edits::Content::of(&vocal, on_slicer, 1);
                    let spots: Vec<ch::edits::Spot> = secs
                        .iter()
                        .enumerate()
                        .map(|(i, (s, e, tag))| {
                            let p = parts.iter().find(|p| p.sec == i);
                            let build = p.map(|p| !p.out.stages.is_empty()).unwrap_or(false);
                            ch::edits::Spot { tag: tag.clone(), beats: e - s, energy: sec_energy[i], chopped: p.map(|p| !p.only_swell).unwrap_or(false), song: 0, build, swell: p.map(|p| p.out.swells > 0).unwrap_or(false), riser: build && edm.riser, sings: false }
                        })
                        .collect();
                    let eplan = ch::edits::plan(&spots, &genre, &edits, &content, seed);
                    // how many sections of each role came before (the edits vary with the repeats)
                    let mut nth = [0usize; 4];
                    for p in parts.iter_mut() {
                        let ri = ch::role_of(&p.tag) as usize;
                        let k = nth[ri];
                        if !p.only_swell {
                            nth[ri] += 1;
                        }
                        let kinds = &eplan[p.sec];
                        if kinds.is_empty() {
                            continue;
                        }
                        let next_hot = secs.get(p.sec + 1).map(|n| ch::role_of(&n.2) == ch::Role::Hot).unwrap_or(false);
                        let at = ch::edits::At { beats: p.len, key, energy: sec_energy[p.sec], drums: &p.drums, harmony: &p.harm, vocal: &vocal, tune: edm.tune, genre: &genre, bpm: src.bpm, seed: seed.wrapping_mul(977).wrapping_add(p.sec as u64), silence: p.out.silence, slicer: on_slicer, next_hot, nth: k };
                        let eo = ch::edits::apply(&at, kinds, &mut p.out.main, &mut p.out.layer);
                        sweeps.extend(eo.sweeps.iter().map(|(a, b)| (p.start + a, p.start + b)));
                        throws.extend(eo.throws.iter().map(|(a, b)| (p.start + a, p.start + b)));
                        p.fx = eo.fx;
                        used.extend(eo.used);
                    }
                }
                // ── the tracks ──
                let cfg = ch::config(&file_s);
                let json = serde_json::to_string(&cfg).unwrap_or_default();
                let any_layer = parts.iter().any(|p| !p.out.layer.is_empty());
                let any_fx = parts.iter().any(|p| !p.fx.is_empty());
                let mut tracks: Vec<usize> = vec![];
                // what each track plays: 0 the chops, 1 their layer, 2 the edits
                let mut plays: Vec<usize> = vec![];
                for (k, nm) in [format!("Vocal Chops · {}", src.name), format!("Vocal Chops layer · {}", src.name), format!("Vocal Chops edits · {}", src.name)].into_iter().enumerate() {
                    if (k == 1 && !any_layer) || (k == 2 && !any_fx) {
                        continue;
                    }
                    let n = match self.dispatch("daw_track_add", &A(&[Val::Str("sampler".into()), Val::Str(String::new())])) {
                        Ok(Val::Num(x)) if x >= 0.0 => x as usize,
                        _ => {
                            self.status = "Vocal chops: no room for another track".into();
                            ok!("")
                        }
                    };
                    let _ = self.dispatch("daw_track_set_name", &A(&[Val::Num(n as f64), Val::Str(nm)]));
                    tracks.push(n);
                    plays.push(k);
                }
                let mut summary_builds = 0usize;
                {
                    let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    for (ti, tk) in tracks.iter().enumerate() {
                        if let Ok(smp) = crate::dsp::sampler::Sampler::from_config(sr, &json, None) {
                            if let Some(tr) = en.project.tracks.get_mut(*tk) {
                                tr.inst = Some(Box::new(smp));
                                tr.inst_kind = "sampler".into();
                                tr.volume = [0.85, 0.55, 0.5][plays[ti]];
                            }
                        }
                        for p in &parts {
                            let notes = match plays[ti] {
                                0 => &p.out.main,
                                1 => &p.out.layer,
                                _ => &p.fx,
                            };
                            if notes.is_empty() {
                                continue;
                            }
                            let mut c = Clip::midi(p.start, p.len, &format!("chops {}", p.tag));
                            c.tag = "chopx".into();
                            if let ClipData::Midi(v) = &mut c.data {
                                v.extend(notes.iter().cloned());
                            }
                            if let Some(tr) = en.project.tracks.get_mut(*tk) {
                                tr.clips.push(c);
                            }
                        }
                    }
                    // the Vox Filter: the high-pass lane of every build, open (0) elsewhere
                    let mut hp: Vec<(f64, f32)> = vec![(0.0, 0.0)];
                    for p in &parts {
                        if !p.out.hp.is_empty() {
                            summary_builds += 1;
                            hp.extend(p.out.hp.iter().map(|(b, v)| (p.start + b, *v)));
                        }
                    }
                    if hp.len() > 1 {
                        hp.sort_by(|x, y| x.0.total_cmp(&y.0));
                        for tk in &tracks {
                            let Some(tr) = en.project.tracks.get_mut(*tk) else { continue };
                            let Some(mut fx) = EffectSlot::new("Vox Filter", sr) else { continue };
                            for (k, v) in [("hp", 0.0f32), ("lp", 1.0), ("res", 0.3), ("dyn", 0.0), ("mix", 1.0)] {
                                if let Some(pi) = fx.params.iter().position(|p| p.0 == k) {
                                    fx.set(pi, v);
                                }
                            }
                            let idx = tr.effects.len();
                            let hpi = fx.params.iter().position(|p| p.0 == "hp");
                            tr.effects.push(fx);
                            if let Some(pi) = hpi {
                                tr.auto.push(AutoLane { param: format!("fx{idx}.{pi}"), points: hp.clone() });
                            }
                        }
                    }
                }
                // the chops on Ling Slicer kits, tuned by it to the song's harmony
                let kit = if on_slicer { Some(self.chops_on_slicer(&tracks, &file_s, &lay, &edm.tune)) } else { None };
                // the edits' sound: the edits track's filter and reverb, the sweeps on every chop track, the throws on the chops
                if !used.is_empty() {
                    let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    let bpm = en.project.bpm;
                    for (ti, tk) in tracks.iter().enumerate() {
                        let Some(tr) = en.project.tracks.get_mut(*tk) else { continue };
                        if plays[ti] == 2 {
                            ch::edits::dress_fx_track(tr, sr);
                        }
                        if !sweeps.is_empty() {
                            ch::edits::add_sweeps(tr, &sweeps);
                        }
                        if plays[ti] == 0 {
                            ch::edits::add_throws(tr, &throws, bpm, &genre, sr);
                        }
                    }
                }
                // the kick ducks the chops
                let kick_track = {
                    let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    en.project.tracks.iter().position(|x| x.inst_kind == "drums" && x.clips.iter().any(|c| matches!(&c.data, ClipData::Midi(ns) if ns.iter().any(|n| n.key == 35 || n.key == 36))))
                };
                if let (Some(k), true) = (kick_track, duck > 0.0) {
                    for tk in &tracks {
                        let _ = self.dispatch("daw_duck_add", &A(&[Val::Num(k as f64), Val::Num(*tk as f64), Val::Str("36,35".into()), Val::Num(duck * 1.5)]));
                    }
                }
                self.touch();
                let notes: usize = parts.iter().map(|p| p.out.main.len() + p.out.layer.len() + p.fx.len()).sum();
                let stutters: usize = parts.iter().map(|p| p.out.stutters).sum();
                let swells: usize = parts.iter().map(|p| p.out.swells).sum();
                let hook = vocal.hook_text();
                let mut ts = ch::TuneStats::default();
                for p in &parts {
                    ts.add(&p.out.tune);
                }
                let tune_text = if !edm.tune.on() {
                    "; not tuned (as sung)".to_string()
                } else {
                    format!(
                        "; tuned to the {}: {} of {} chop notes moved (median shift {:.0} cents){}{}",
                        if edm.tune.mode == ch::TuneMode::Key { "key" } else { "chords" },
                        ts.tuned,
                        ts.tuned + ts.kept,
                        ts.median_shift(),
                        if ts.noise > 0 { format!(", {} unvoiced as sung", ts.noise) } else { String::new() },
                        if ts.capped > 0 { format!(", {} moved an octave / to another chord tone to stay within {} semitones", ts.capped, edm.tune.max_shift) } else { String::new() }
                    )
                };
                let stage_text = parts.iter().find(|p| !p.out.stages.is_empty()).map(|p| p.out.stages.iter().map(|s| if s.len >= 4.0 { "1".to_string() } else { format!("1/{}", (4.0 / s.len).round()) }).collect::<Vec<_>>().join(" > ")).unwrap_or_default();
                let kit_text = match kit {
                    Some((sl, _, merged)) => format!("; on Ling Slicer: {sl} slices from C2 up (a key each){}", if merged > 0 { format!(" ({merged} rare variants share a neighbour's key)") } else { String::new() }),
                    None => "; on a Sampler".to_string(),
                };
                let edit_text = match ch::edits::tally(&used) {
                    t if t.is_empty() => String::new(),
                    t => format!("; edits: {t}"),
                };
                let msg = format!(
                    "Vocal chops: {} syllables / {} words of {} ({}); hook: \"{}\"; {} notes in {} sections ({} builds{}), {} stutters, {} reverse swells, key {}{}{}{}",
                    vocal.pieces.len(),
                    vocal.words.len(),
                    src.name,
                    src.how,
                    if hook.is_empty() { "-".to_string() } else { hook },
                    notes,
                    parts.len(),
                    summary_builds,
                    if stage_text.is_empty() { String::new() } else { format!(": {} bar loops", stage_text) },
                    stutters,
                    swells,
                    crate::theory::Key { tonic: key.0, minor: key.1 }.name(),
                    tune_text,
                    kit_text,
                    edit_text
                );
                self.status = msg.clone();
                ok!(msg)
            }
            _ => Err(format!("unknown builtin {name}")),
        }
    }
}
