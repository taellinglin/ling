//! Make Medley (`daw_song_medley_build`): a folder of MIDI / tracker files chopped into phrases and mixed into one piece - a medley
//! of whole phrases, or a layered mashup (medley.rs) - built with the same engine features as New AI Song where they apply:
//! the story / arc and the feel (emotion.rs, feel.rs), the flow (songgen.rs: drum fills and crashes into every new section, snare
//! rolls in the builds, the breath before a drop, noise sweeps and risers into the drops), sound design and space (sounddesign.rs,
//! space.rs), sound recipes (recipes.rs: the sound, and the notes where the medley's role allows), the expression rules and the
//! recipes' master moves. Acapellas (medley/acapella.rs) play under it the way a live mashup uses them: the chops (chops.rs
//! `compose_edm`, tuned to the chords) carry the vocal where the plan says, the acapella's level lane at 0 there and back up on the
//! downbeat they end, and the key lines stressed with harmonies, a vocoder, doubles, a telephone filter or a stutter.
use super::song_api::{apply_design, find_patch, instrument_for, medley_phrases, write_wav16};
use super::{Daw, A, Val};
use crate::chops::{self, Vocal};
use crate::emotion::Feel;
use crate::medley::acapella::{self as ac, Stress, Vox};
use crate::medley::{Cat, Roll};
use crate::model::*;
use crate::songgen;
use crate::theory::N;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// a section of the medley as laid out: a slot, or a loop roll after a slot
#[derive(Clone, Debug)]
struct Sect {
    start: f64,
    end: f64,
    tag: &'static str,
    slot: Option<usize>,
    /// a roll: the slot its loops come from and which way it goes
    roll: Option<(usize, Roll)>,
    energy: f64,
    key: (u8, bool),
}
impl Sect {
    fn beats(&self) -> f64 {
        self.end - self.start
    }
}

/// an acapella in the medley
struct Acap {
    name: String,
    main: bool,
    rate: u32,
    bpm_src: f64,
    bpm_how: &'static str,
    stretch: f64,
    key: (u8, bool),
    /// the recording at the medley's tempo (no shift): the chops' bank and the analysis
    prep: Vec<f32>,
    vocal: Vocal,
    phase: f64,
    hooks: Vec<usize>,
    reps: Vec<usize>,
    /// the sampler's file: the prepared recording and reversed copies of the hook word (for the swells into the drops)
    bank: Vec<f32>,
    /// the sampler config of its chops
    bank_json: String,
    /// rendered shifts: semitones -> the sample (written next to the project)
    shifted: HashMap<i32, Arc<Sample>>,
}

fn sanitize(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
    t.chars().take(40).collect()
}

fn feel_or(emo: &crate::emotion::EmotionPlan, i: usize) -> Feel {
    emo.feels.get(i).copied().unwrap_or(Feel { valence: 0.0, arousal: 0.5, tension: 0.4, vocal: 0.5, breath: 0.3, space: 0.4 })
}

fn to_n(v: &[Note]) -> Vec<N> {
    v.iter().map(|n| N { start: n.start, len: n.len, key: n.key, vel: n.vel }).collect()
}

fn from_n(v: &[N]) -> Vec<Note> {
    v.iter().map(|n| Note::new(n.start, n.len, n.key, n.vel)).collect()
}

/// a numbered name for a section ("verse 2")
fn sec_names(secs: &[Sect]) -> Vec<String> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    let tot: HashMap<&str, usize> = secs.iter().fold(HashMap::new(), |mut m, s| {
        *m.entry(s.tag).or_insert(0) += 1;
        m
    });
    secs.iter()
        .map(|s| {
            let k = seen.entry(s.tag).or_insert(0);
            *k += 1;
            if tot[s.tag] > 1 {
                format!("{} {}", s.tag, k)
            } else {
                s.tag.to_string()
            }
        })
        .collect()
}

impl Daw {
    /// (genre) -> [loudness LUFS, brightness]: the Mix AI target the medley hands to daw_mix_style (as New AI Song does)
    pub(super) fn medley_mix(&mut self, a: &A) -> Result<Val, String> {
        let g = songgen::genre(&a.s(0));
        Ok(Val::List(vec![Val::Num(g.lufs), Val::Num(0.0)]))
    }

    /// (folder, phrase bars 0 = auto, phrases, mode "smart"|"random"|"cipher", passphrase, message, seed, bpm 0 = the files' (or the
    /// first main acapella's), options) -> a sentence. Options "k=v;..." with the story last ("story=<any text>"):
    /// layering=0..1; rolls=0..1; arc=<story arc or feel>; feelarc=<emotion arc>; genre=<sounds>; registers=layer|guard|off;
    /// align= / breakdown= / answer= / clash= / risers= (0 = off); design=0..1; feel=0..1; recipes=0|1; recipenotes=0|1;
    /// master=0|1; flow=0|1; expr=0|1; chops=off|chops|breakcore; chopn=; choplen=; chopvar=; chopick=random|best; vocal=<file>;
    /// acap=<file>|<file> (main acapellas: they sing); acapx=<file>|... (acapellas only chopped); replace=0|1 (the chops replace the
    /// vocal: its level 0 where they play, back up after); vocret=fade|cut; stress=0..1 (how many lines are stressed);
    /// stressways=auto|harmonies|vocoder|doubles.
    pub(super) fn medley_build(&mut self, a: &A) -> Result<Val, String> {
        let eng = self.out.engine.clone();
        macro_rules! ok {
            ($v:expr) => {
                return Ok(Val::from($v))
            };
        }
        let (ph, files) = medley_phrases(&a.s(0), a.n(1));
        if ph.len() < 2 {
            ok!("Not enough phrases: the folder needs MIDI files with notes".to_string());
        }
        let mode = match a.s(3).as_str() {
            "random" => crate::medley::Mode::Random,
            "cipher" => crate::medley::Mode::Cipher,
            _ => crate::medley::Mode::Smart,
        };
        let pass = a.s(4);
        let message = a.s(5);
        let seed = a.n(6).abs() as u64;
        let opts = a.s(8);
        let (head, story) = match opts.find("story=") {
            Some(i) => (opts[..i].to_string(), opts[i + 6..].trim().to_string()),
            None => (opts.clone(), String::new()),
        };
        let opt = |k: &str| head.split(';').filter_map(|kv| kv.split_once('=')).find(|(kk, _)| kk.trim() == k).map(|(_, v)| v.trim().to_string());
        let num = |k: &str| opt(k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0).clamp(0.0, 1.0);
        let numd = |k: &str, d: f64| opt(k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(d);
        let flag = |k: &str| opt(k).map(|v| !matches!(v.as_str(), "0" | "off" | "false" | "no")).unwrap_or(true);
        let on = |k: &str| opt(k).map(|v| matches!(v.as_str(), "1" | "on" | "true" | "yes")).unwrap_or(false);
        let (layering, rolls) = (num("layering"), num("rolls"));
        let g = songgen::genre(&opt("genre").filter(|x| !x.is_empty()).unwrap_or_else(|| "pop".into()));
        let sr = self.out.sr;
        let samples_dir = if self.path.is_empty() { PathBuf::from("cache/medley") } else { Path::new(&self.path).parent().map(|p| p.join("samples")).unwrap_or_else(|| PathBuf::from("cache/medley")) };

        // ── the acapellas: loaded, their tempo (name, else measured) and key (sung pitches) ──
        let mut acaps: Vec<Acap> = vec![];
        let mut acap_notes: Vec<String> = vec![];
        for (list, main) in [(opt("acap").unwrap_or_default(), true), (opt("acapx").unwrap_or_default(), false)] {
            for f in list.split('|').map(str::trim).filter(|f| !f.is_empty()) {
                let Ok((stereo, rate)) = crate::dsp::sampler::load_audio(f) else {
                    acap_notes.push(format!("{} could not be read", Path::new(f).file_name().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default()));
                    continue;
                };
                let name = Path::new(f).file_stem().map(|s| s.to_string_lossy().replace('_', " ")).unwrap_or_default();
                let (bpm_src, how) = match ac::bpm_from_name(&name) {
                    Some(b) => (b, "from its name"),
                    None => match ac::detect_bpm(&stereo, rate) {
                        Some((b, c)) if c > 0.02 => ((b * 10.0).round() / 10.0, "measured"),
                        _ => (0.0, "unknown"),
                    },
                };
                acaps.push(Acap { name, main, rate, bpm_src, bpm_how: how, stretch: 1.0, key: (0, false), prep: stereo, vocal: Vocal::default(), phase: 0.0, hooks: vec![], reps: vec![], bank: vec![], bank_json: String::new(), shifted: HashMap::new() });
            }
        }
        if !acaps.is_empty() && !acaps.iter().any(|x| x.main) {
            acaps[0].main = true;
        }
        let mut bpms: Vec<f64> = ph.iter().map(|p| p.bpm).collect();
        bpms.sort_by(f64::total_cmp);
        let first_main = acaps.iter().find(|x| x.main && x.bpm_src > 1.0).map(|x| x.bpm_src);
        let bpm = if a.n(7) > 20.0 {
            a.n(7)
        } else if let Some(b) = first_main {
            // the medley takes the singer's tempo (the MIDI parts follow it freely; a recording cannot)
            ac::tempo_ratio(b, 124.0).0.clamp(60.0, 200.0)
        } else {
            bpms[bpms.len() / 2]
        };
        for x in acaps.iter_mut() {
            let src = if x.bpm_src > 1.0 { x.bpm_src } else { bpm };
            let (_, st) = ac::tempo_ratio(src, bpm);
            x.stretch = st;
            x.prep = ac::prepare(&x.prep, st, 0.0);
            x.vocal = Vocal::from_audio(&x.prep, x.rate, bpm, 0.0, &[]);
            x.key = ac::key_of(&x.vocal).unwrap_or(crate::medley::common_key(&ph));
            x.phase = ac::bar_phase(&x.vocal);
            x.bank = x.vocal.add_reverse(&x.prep);
            x.reps = ac::line_repeats(&x.vocal);
            let hl = x.vocal.hook_line;
            if let Some(h) = hl {
                // the hook line and the lines sung like it
                let hs: Vec<f32> = x.vocal.pieces[x.vocal.lines[h].first..x.vocal.lines[h].end].iter().filter_map(|p| p.chop.midi).collect();
                x.hooks = (0..x.vocal.lines.len())
                    .filter(|l| {
                        let ls: Vec<f32> = x.vocal.pieces[x.vocal.lines[*l].first..x.vocal.lines[*l].end].iter().filter_map(|p| p.chop.midi).collect();
                        *l == h || (!ls.is_empty() && (ls.len() as i64 - hs.len() as i64).abs() <= 1 && (0..ls.len().min(hs.len())).filter(|k| (ls[*k] - hs[*k]).abs() <= 1.0).count() * 4 >= 3 * ls.len().min(hs.len()))
                    })
                    .collect();
            }
        }
        // the home key: the first main acapella's (the MIDI moves to the voice), else the files'
        let key = acaps.iter().find(|x| x.main && !x.vocal.pieces.is_empty()).map(|x| x.key).unwrap_or_else(|| crate::medley::common_key(&ph));

        // ── the story: the arc (a story arc or a feel), the feel the parts are played with ──
        let arc_opt = opt("arc").unwrap_or_default();
        let feel_opt = opt("feelarc").unwrap_or_default();
        let arc_in = if !arc_opt.is_empty() && arc_opt != "none" { arc_opt.clone() } else { feel_opt.clone() };
        let arc = crate::medley::resolve_arc(&arc_in, &story, seed);
        let lean = if story.is_empty() { 0.0 } else { crate::emotion::lyric_lean(&story).0 as f64 * 0.5 };
        let count = (a.n(2).max(2.0) as usize).min(512);
        let order: Vec<usize> = if mode == crate::medley::Mode::Cipher && !message.is_empty() {
            crate::medley::encode(&message, ph.len(), &pass)
        } else {
            let curve: Vec<f64> = crate::medley::movement(&arc, count, lean).iter().map(|x| x.0).collect();
            crate::medley::order_to(&ph, count, mode, seed, &pass, key, Some(&curve))
        };
        // the arrangement smarts: registers (a muddy bass up, a shrill lead down, every layer an octave of its own), phrase
        // alignment, a breakdown before the last drop, chord-aware fitting, call and response (medley.rs mashup_with)
        let smarts = crate::medley::Smarts {
            registers: crate::medley::Registers::parse(&opt("registers").unwrap_or_default()),
            genre: g.name.to_string(),
            align: flag("align"),
            breakdown: flag("breakdown"),
            answer: flag("answer"),
            clash: flag("clash"),
        };
        let mut mash = crate::medley::mashup_with(&ph, &order, &arc, lean, layering, rolls, seed, key, &smarts);
        // vocal chops (and, in the breakcore style, programmed breaks in place of the files' drums); acapellas that the chops replace
        // turn the chops on
        let replace = !acaps.is_empty() && flag("replace");
        let mut style = chops::Style::parse(&opt("chops").unwrap_or_default());
        if replace && style == chops::Style::Off {
            style = chops::Style::Chops;
        }
        if style == chops::Style::Breakcore {
            for s in mash.slots.iter_mut() {
                s.layers.retain(|l| l.spine || l.cat != Cat::Drums);
            }
        }
        let start0 = {
            let en = eng.lock().unwrap_or_else(|p| p.into_inner());
            (en.project.end_beat() / 4.0).ceil() * 4.0
        };
        // ── the layout: every slot and every roll as a section ──
        let ns = mash.slots.len();
        let mut secs: Vec<Sect> = vec![];
        {
            let mut at = start0;
            for (si, s) in mash.slots.iter().enumerate() {
                secs.push(Sect { start: at, end: at + s.beats, tag: crate::medley::slot_tag(si, ns, s.energy), slot: Some(si), roll: None, energy: s.energy, key: s.key });
                at += s.beats;
                let src = match s.roll_after {
                    Roll::Shrink => Some(si),
                    Roll::Grow => Some(si + 1).filter(|x| *x < ns),
                    Roll::None => None,
                };
                if let Some(ri) = src {
                    let shrink = s.roll_after == Roll::Shrink;
                    let e = if shrink { 0.5 * (s.energy + mash.slots.get(si + 1).map(|x| x.energy).unwrap_or(s.energy)).max(0.6) } else { 0.5 };
                    secs.push(Sect { start: at, end: at + crate::medley::ROLL_BEATS, tag: if shrink { "build" } else { "break" }, slot: None, roll: Some((ri, s.roll_after)), energy: e, key: mash.slots[ri].key });
                    at += crate::medley::ROLL_BEATS;
                }
            }
        }
        let names = sec_names(&secs);
        // the emotional plan over the medley's sections (the story's feel per section): it plays the parts, sets the fills, the
        // rolls' breath before the drop, and where the vocal sings
        let feel_arc = if !feel_opt.is_empty() && crate::emotion::ARCS.contains(&feel_opt.as_str()) { feel_opt.clone() } else { crate::feel::emotion_arc(&mash.arc) };
        let emo = {
            let ss: Vec<songgen::Section> = secs.iter().map(|s| songgen::Section { tag: s.tag.to_string(), bars: (s.beats() / 4.0).round().max(1.0) as u32, energy: s.energy }).collect();
            let words: Vec<String> = vec![story.clone(); ss.len()];
            crate::emotion::plan(&ss, &feel_arc, g.name, &words, 0.5, seed)
        };
        // each section's context: (drums, harmony, lead) notes, relative to its start
        let ctx: Vec<(Vec<Note>, Vec<Note>, Vec<Note>)> = secs
            .iter()
            .map(|sc| {
                let (mut d, mut h, mut l) = (vec![], vec![], vec![]);
                let (slot, beats, roll) = match (sc.slot, sc.roll) {
                    (Some(si), _) => (si, mash.slots[si].beats, None),
                    (None, Some((ri, r))) => (ri, mash.slots[ri].beats, Some(r)),
                    _ => return (d, h, l),
                };
                for ly in &mash.slots[slot].layers {
                    let mut notes = crate::medley::layer_notes(&ph, ly, beats);
                    if let Some(r) = roll {
                        notes = crate::medley::roll_notes(&notes, beats, r);
                    }
                    match ly.cat {
                        Cat::Drums => d.extend(notes),
                        Cat::Bass | Cat::Pad | Cat::Stab => h.extend(notes),
                        _ => l.extend(notes),
                    }
                }
                (d, h, l)
            })
            .collect();

        // ── the acapellas' plan: which sections sing, which are chops, the excerpts, the stressed lines ──
        let plan: Vec<Vox> = if acaps.is_empty() {
            vec![]
        } else {
            let sin: Vec<ac::SecIn> = secs.iter().enumerate().map(|(i, s)| ac::SecIn { tag: s.tag.to_string(), roll: s.roll.is_some(), feel: feel_or(&emo, i) }).collect();
            let p = ac::plan_vox(&sin, replace);
            // no chops asked for and none replacing the vocal: it simply sings
            if style == chops::Style::Off { p.iter().map(|_| Vox::Full).collect() } else { p }
        };
        let mains: Vec<usize> = (0..acaps.len()).filter(|i| acaps[*i].main).collect();
        // who sings each section (the mains take turns on the singing sections; a chops section keeps the last singer) and whose
        // chops play (every acapella in turn)
        let mut owner: Vec<usize> = vec![];
        let mut chop_src: Vec<usize> = vec![];
        {
            let (mut k, mut c) = (0usize, 0usize);
            let mut last = mains.first().copied().unwrap_or(0);
            for v in &plan {
                if v.sings() && !mains.is_empty() {
                    last = mains[k % mains.len()];
                    k += 1;
                }
                owner.push(last);
                chop_src.push(if v.chops() { c += 1; (c - 1) % acaps.len().max(1) } else { last });
            }
        }
        // excerpts per main over the sections it sings (the hot ones take the hook)
        let mut excerpt: Vec<Option<ac::Excerpt>> = vec![None; secs.len()];
        for m in &mains {
            let x = &acaps[*m];
            let mine: Vec<usize> = (0..secs.len()).filter(|i| owner[*i] == *m).collect();
            let spans: Vec<(f64, f64, bool)> = mine.iter().map(|i| (secs[*i].start, secs[*i].beats(), secs[*i].tag == "drop" || plan[*i] == Vox::Both)).collect();
            let total = x.prep.len() as f64 / 2.0 / x.rate as f64 * bpm / 60.0;
            let ex = ac::excerpts(&x.vocal, x.phase, &spans, &x.hooks, total);
            for (k, i) in mine.iter().enumerate() {
                excerpt[*i] = ex[k].clone();
            }
        }
        // the key fit of each excerpt: whole semitones (medley fit_shift: in the section's scale, consonant with its chords)
        let mut shift: Vec<i32> = vec![0; secs.len()];
        for i in 0..secs.len() {
            let Some(e) = &excerpt[i] else { continue };
            let x = &acaps[owner[i]];
            let sung = ac::sung_notes(&x.vocal, e.from + e.pickup, e.from + e.len);
            let against: Vec<Note> = ctx[i].1.clone();
            let base = ((secs[i].key.0 as i32 - x.key.0 as i32 + if secs[i].key.1 == x.key.1 { 0 } else if x.key.1 { -3 } else { 3 }) + 6).rem_euclid(12) - 6;
            // whole semitones, at most three either way (further and the voice stops sounding like itself; the chops still tune)
            shift[i] = if sung.is_empty() { 0 } else { crate::medley::fit_shift_within(&sung, base, secs[i].key, &against, -3, 3) };
        }
        // the lines that sing, and which are stressed (and how)
        let stress_amt = if acaps.is_empty() { 0.0 } else { numd("stress", 0.5).clamp(0.0, 1.0) };
        let ways = ac::Ways::parse(&opt("stressways").unwrap_or_default());
        let last_sing = (0..secs.len()).rev().find(|i| plan.get(*i).map(|v| v.sings()).unwrap_or(false));
        let mut cands: Vec<ac::Cand> = vec![];
        for i in 0..secs.len() {
            let (Some(e), Some(v)) = (&excerpt[i], plan.get(i)) else { continue };
            if !v.sings() {
                continue;
            }
            let x = &acaps[owner[i]];
            for li in &e.lines {
                let l = &x.vocal.lines[*li];
                let s = e.at + (l.start - e.from);
                let en_ = (e.at + (l.stop - e.from)).min(e.at + e.len);
                if s >= e.at + e.len - 0.25 || en_ <= s + 0.5 {
                    continue;
                }
                cands.push(ac::Cand { sec: i, line: *li, start: s, end: en_, hook: x.hooks.contains(li) || x.vocal.hook_line == Some(*li), repeats: x.reps.get(*li).copied().unwrap_or(0), feel: feel_or(&emo, i), vox: *v, last: Some(i) == last_sing });
            }
        }
        let picks = ac::choose(&cands, stress_amt, ways);

        // ── the chops of every section (acapellas: lyric-aware EDM chops of the section's chop source, tuned to its chords) ──
        let edm = chops::Edm { density: 0.6, stutter: if style == chops::Style::Breakcore { 0.9 } else { 0.55 }, ..chops::Edm::default() };
        // per acapella: per section (main, layer) chop notes
        let mut sec_chops: Vec<Vec<(Vec<Note>, Vec<Note>)>> = (0..acaps.len()).map(|_| vec![(vec![], vec![]); secs.len()]).collect();
        let mut chop_tune = chops::TuneStats::default();
        if !acaps.is_empty() {
            for i in 0..secs.len() {
                let v = plan[i];
                let next_hot = secs.get(i + 1).map(|n| n.tag == "drop" && plan[i + 1].chops()).unwrap_or(false);
                let src = chop_src[i];
                let x = &acaps[src];
                if x.vocal.pieces.is_empty() || x.vocal.lines.is_empty() {
                    continue;
                }
                let only_swell = !v.chops();
                if only_swell && !next_hot {
                    continue;
                }
                let (d, h, _) = &ctx[i];
                let out = chops::compose_edm(&chops::EdmCtx {
                    beats: secs[i].beats(),
                    key: secs[i].key,
                    energy: secs[i].energy,
                    tag: secs[i].tag,
                    drums: d,
                    harmony: h,
                    vocal: &x.vocal,
                    bpm,
                    seed: seed.wrapping_mul(31).wrapping_add(i as u64),
                    o: &edm,
                    build_beats: 0.0,
                    next_hot,
                    loop_line: None,
                    only_swell,
                });
                chop_tune.add(&out.tune);
                sec_chops[src][i] = (out.main, out.layer);
            }
        }
        // stutters into stressed lines live on the chop track of the singer
        for (ci, how, _) in &picks {
            if *how != Stress::Stutter {
                continue;
            }
            let c = &cands[*ci];
            let x = &acaps[owner[c.sec]];
            let ns_ = ac::stutter_into(&x.vocal, c.line, c.start, secs[c.sec].start, secs[c.sec].key, &ctx[c.sec].1, shift[c.sec], &chops::Tune::default());
            sec_chops[owner[c.sec]][c.sec].0.extend(ns_);
        }

        // ── the acapellas' recordings: banks for the chops, a rendering per shift ──
        let _ = std::fs::create_dir_all(&samples_dir);
        for (ai, x) in acaps.iter_mut().enumerate() {
            let tag = format!("{}_{:.0}", sanitize(&x.name), bpm);
            if sec_chops[ai].iter().any(|s| !s.0.is_empty() || !s.1.is_empty()) {
                let f = samples_dir.join(format!("acap_{tag}_bank.wav"));
                if write_wav16(&f, &x.bank, x.rate).is_ok() {
                    // the bank is a new file: its chops' frames are those of the prepared recording, reversed copies after it
                    x.bank_json = serde_json::to_string(&chops::config(&f.to_string_lossy().replace('\\', "/"))).unwrap_or_default();
                }
            }
            let mut want: Vec<i32> = (0..secs.len()).filter(|i| owner.get(*i) == Some(&ai) && excerpt[*i].is_some()).map(|i| shift[i]).collect();
            want.sort_unstable();
            want.dedup();
            for s in want {
                let data = if s == 0 { x.prep.clone() } else { crate::stretch::pitch_shift(&x.prep, s as f64) };
                let f = samples_dir.join(format!("acap_{tag}_{s:+}.wav"));
                let path = f.to_string_lossy().replace('\\', "/");
                let _ = write_wav16(&f, &data, x.rate);
                let smp = Arc::new(Sample::new(format!("{} {s:+}", x.name), path.clone(), x.rate, data));
                self.samples.insert(path, smp.clone());
                x.shifted.insert(s, smp);
            }
        }
        // the stressed lines' audio: harmonies and octave layers rendered from the shifted recording, the rest are copies
        struct StressClip {
            how: Stress,
            /// 0 = up / the line, 1 = the harmony below
            voice: usize,
            at: f64,
            len: f64,
            sample: Arc<Sample>,
            offset_s: f64,
            acap: usize,
        }
        let mut sclips: Vec<StressClip> = vec![];
        let mut vocoded: Vec<(usize, f64, f64, usize)> = vec![];
        let mut holes: Vec<Vec<(f64, f64)>> = vec![vec![]; acaps.len()];
        let mut stress_said: Vec<String> = vec![];
        let spb = 60.0 / bpm;
        for (ci, how, why) in &picks {
            let c = &cands[*ci];
            let i = c.sec;
            if excerpt[i].is_none() {
                continue;
            }
            let m = owner[i];
            let x = &acaps[m];
            let Some(smp) = x.shifted.get(&shift[i]).cloned() else { continue };
            let l = &x.vocal.lines[c.line];
            let rate = x.rate as f64;
            // the line in the shifted recording, with a little air
            let pad = 0.06;
            let s0 = ((l.start * spb - pad).max(0.0) * rate) as usize;
            let s1 = (((l.stop * spb + pad) * rate) as usize).min(smp.frames());
            if s1 <= s0 + 64 {
                continue;
            }
            let at = c.start - (l.start - (s0 as f64 / rate / spb));
            let len = (s1 - s0) as f64 / rate / spb;
            let off = s0 as f64 / rate;
            let lname = format!("line {} of {} in {}", c.line + 1, x.name, names[i]);
            match how {
                Stress::Harmony => {
                    let seg = &smp.stereo[s0 * 2..s1 * 2];
                    let (key, harm, sec0) = (secs[i].key, &ctx[i].1, secs[i].start);
                    let tune = chops::Tune::default();
                    let voices = if c.feel.arousal >= 0.6 || stress_amt >= 0.7 { 2 } else { 1 };
                    for vi in 0..voices {
                        let up = vi == 0;
                        let target = |sg: &ac::Seg| -> Option<f32> {
                            let mid = 0.5 * (sg.a + sg.b) as f64 / rate / spb;
                            let t = at + mid - sec0;
                            let tones = tune.tones(crate::theory::Key::new(key.0, key.1), harm, t);
                            sg.midi.and_then(|mm| ac::harmony_note(mm, &tones, up)).map(|p| p as f32)
                        };
                        let (y, used) = ac::harmonize(seg, x.rate, 0, s1 - s0, &target);
                        if used.is_empty() {
                            continue;
                        }
                        let f = samples_dir.join(format!("acap_{}_harm{}_{}_{}.wav", sanitize(&x.name), if up { "up" } else { "down" }, i, c.line));
                        let path = f.to_string_lossy().replace('\\', "/");
                        let _ = write_wav16(&f, &y, x.rate);
                        let hs = Arc::new(Sample::new(format!("{} harmony", x.name), path.clone(), x.rate, y));
                        self.samples.insert(path, hs.clone());
                        sclips.push(StressClip { how: Stress::Harmony, voice: vi, at, len, sample: hs, offset_s: 0.0, acap: m });
                    }
                }
                Stress::Octave => {
                    let y = crate::stretch::pitch_shift(&smp.stereo[s0 * 2..s1 * 2], -12.0);
                    let f = samples_dir.join(format!("acap_{}_oct_{}_{}.wav", sanitize(&x.name), i, c.line));
                    let path = f.to_string_lossy().replace('\\', "/");
                    let _ = write_wav16(&f, &y, x.rate);
                    let os = Arc::new(Sample::new(format!("{} octave", x.name), path.clone(), x.rate, y));
                    self.samples.insert(path, os.clone());
                    sclips.push(StressClip { how: Stress::Octave, voice: 0, at, len, sample: os, offset_s: 0.0, acap: m });
                }
                Stress::Double => sclips.push(StressClip { how: Stress::Double, voice: 0, at: at + 0.022 / spb, len, sample: smp.clone(), offset_s: off, acap: m }),
                Stress::Telephone => {
                    holes[m].push((c.start, c.end));
                    sclips.push(StressClip { how: Stress::Telephone, voice: 0, at, len, sample: smp.clone(), offset_s: off, acap: m });
                }
                Stress::Vocoder => {
                    sclips.push(StressClip { how: Stress::Vocoder, voice: 0, at, len, sample: smp.clone(), offset_s: off, acap: m });
                    vocoded.push((i, c.start, c.end, m));
                }
                Stress::Stutter => {}
            }
            stress_said.push(format!("{lname}{}: {} ({why})", if c.hook { " (the hook)" } else { "" }, how.name()));
        }

        // ── tracks ──
        // an instrument for every clip: its category's role for the slot's energy and mood, the next patch each time
        const KITS: [&str; 7] = ["Kit House 909", "Kit Electro 808", "Kit UK Garage", "Kit Trap", "Kit New Wave", "Kit Hyperpop", "Kit Boom Bap"];
        let mut wanted: Vec<(Cat, String, Option<String>)> = vec![];
        let mut wanted_role: Vec<&'static str> = vec![];
        let mut assign: Vec<Vec<usize>> = vec![];
        for s in &mash.slots {
            let mut row = vec![];
            for l in &s.layers {
                let k = l.pick;
                let (kind, patch) = if l.cat == Cat::Drums {
                    let kits: Vec<&str> = std::iter::once(g.kit).chain(KITS.iter().copied().filter(|x| *x != g.kit)).collect();
                    ("drums".to_string(), find_patch("drums", kits[k % kits.len()]))
                } else {
                    instrument_for(g, l.role, k / 3)
                };
                let idx = match wanted.iter().position(|w| w.0 == l.cat && w.1 == kind && w.2 == patch) {
                    Some(i) => i,
                    None if wanted.len() < 24 => {
                        wanted.push((l.cat, kind, patch));
                        wanted_role.push(l.role);
                        wanted.len() - 1
                    }
                    // enough tracks: the category's first
                    None => match wanted.iter().position(|w| w.0 == l.cat) {
                        Some(i) => i,
                        None => {
                            wanted.push((l.cat, kind, patch));
                            wanted_role.push(l.role);
                            wanted.len() - 1
                        }
                    },
                };
                row.push(idx);
            }
            assign.push(row);
        }
        let mut tracks: Vec<Option<usize>> = vec![];
        for (cat, kind, patch) in &wanted {
            let t = self.call_num("daw_track_add", &[Val::Str(kind.clone()), Val::Str(patch.clone().unwrap_or_default())]);
            let stem = patch.as_deref().and_then(|p| Path::new(p).file_stem()).map(|s| s.to_string_lossy().replace('_', " ")).unwrap_or_else(|| kind.clone());
            if t >= 0.0 {
                let _ = self.dispatch("daw_track_set_name", &A(&[Val::Num(t), Val::Str(format!("{} · {}", cat.name(), stem))]));
                tracks.push(Some(t as usize));
            } else {
                tracks.push(None);
            }
        }
        // the vocal chops without acapellas: the chosen file, else a vocal audio file in the folder, else syllables sung by Ling Voice
        // (saved next to the project); cut into chops, tuned on a Sampler (chops.rs)
        let mut chop_cfg: Option<(String, Vec<chops::Chop>)> = None;
        let mut chop_name = String::new();
        if style != chops::Style::Off && acaps.is_empty() {
            let is_audio = |p: &Path| p.extension().map(|e| ["wav", "mp3", "ogg", "flac", "aiff", "m4a"].contains(&e.to_string_lossy().to_lowercase().as_str())).unwrap_or(false);
            let mut src = opt("vocal").filter(|v| !v.is_empty() && Path::new(v).is_file());
            if src.is_none() {
                src = std::fs::read_dir(a.s(0)).ok().and_then(|rd| rd.flatten().map(|e| e.path()).filter(|p| is_audio(p)).min()).map(|p| p.to_string_lossy().into_owned());
            }
            let got: Option<(String, Vec<f32>, u32)> = match src {
                Some(f) => crate::dsp::sampler::load_audio(&f).ok().map(|(d, r)| (f, d, r)),
                None => {
                    let d = chops::sung_syllables(sr, seed);
                    let dir = if self.path.is_empty() { PathBuf::from("cache/chops") } else { Path::new(&self.path).parent().map(|p| p.join("samples")).unwrap_or_else(|| PathBuf::from("cache/chops")) };
                    let _ = std::fs::create_dir_all(&dir);
                    let f = dir.join(format!("ling_voice_chops_{}.wav", seed % 3));
                    write_wav16(&f, &d, sr as u32).ok().map(|_| (f.to_string_lossy().replace('\\', "/"), d, sr as u32))
                }
            };
            if let Some((file, stereo, rate)) = got {
                let co = chops::Opts {
                    count: numd("chopn", 8.0).clamp(1.0, 64.0) as usize,
                    length: numd("choplen", 0.25).clamp(0.05, 1.5),
                    vary: numd("chopvar", 0.5).clamp(0.0, 1.0),
                    random: opt("chopick").map(|v| v != "best").unwrap_or(true),
                };
                let ch = chops::pick_chops(&stereo, rate, &co, seed);
                if !ch.is_empty() {
                    let cfg = chops::config(&file);
                    chop_name = Path::new(&file).file_stem().map(|s| s.to_string_lossy().replace('_', " ")).unwrap_or_default();
                    chop_cfg = Some((serde_json::to_string(&cfg).unwrap_or_default(), ch));
                }
            }
        }
        // each slot's chops (main, layer) and breaks, composed against its layers
        let mut chop_notes: Vec<(Vec<Note>, Vec<Note>, Vec<Note>)> = vec![];
        for (si, s) in mash.slots.iter().enumerate() {
            let br = if style == chops::Style::Breakcore { chops::breaks(s.beats, s.energy, bpm, seed.wrapping_add(si as u64)) } else { vec![] };
            let (mut drums, mut harmony, mut lead) = (br.clone(), vec![], vec![]);
            for l in &s.layers {
                let ns_ = crate::medley::layer_notes(&ph, l, s.beats);
                match l.cat {
                    Cat::Drums => drums.extend(ns_),
                    Cat::Bass | Cat::Pad | Cat::Stab => harmony.extend(ns_),
                    _ => lead.extend(ns_),
                }
            }
            let (m, y) = match &chop_cfg {
                Some((_, ch)) => chops::compose(&chops::Ctx { beats: s.beats, key: s.key, energy: s.energy, drums: &drums, harmony: &harmony, lead: &lead, style, seed: seed.wrapping_mul(31).wrapping_add(si as u64), chops: ch }),
                None => (vec![], vec![]),
            };
            chop_notes.push((m, y, br));
        }
        // their tracks: the chops, their layer, the breaks
        let mut extra: [Option<usize>; 3] = [None, None, None];
        if chop_cfg.is_some() {
            for (i, nm) in [format!("Chops · {chop_name}"), format!("Chops layer · {chop_name}")].into_iter().enumerate() {
                let t = self.call_num("daw_track_add", &[Val::Str("sampler".into()), Val::Str(String::new())]);
                if t >= 0.0 {
                    let _ = self.dispatch("daw_track_set_name", &A(&[Val::Num(t), Val::Str(nm)]));
                    extra[i] = Some(t as usize);
                }
            }
        }
        if chop_notes.iter().any(|c| !c.2.is_empty()) {
            let kit = if g.kit.contains("Drum and Bass") { "Kit Drum and Bass" } else { "Kit Industrial" };
            let t = self.call_num("daw_track_add", &[Val::Str("drums".into()), Val::Str(find_patch("drums", kit).unwrap_or_default())]);
            if t >= 0.0 {
                let _ = self.dispatch("daw_track_set_name", &A(&[Val::Num(t), Val::Str(format!("Breaks · {kit}"))]));
                extra[2] = Some(t as usize);
            }
        }
        // the acapellas' chop tracks (main, layer) per acapella
        let mut acap_chop_t: Vec<[Option<usize>; 2]> = vec![[None, None]; acaps.len()];
        for (ai, x) in acaps.iter().enumerate() {
            if x.bank_json.is_empty() {
                continue;
            }
            for (k, nm) in [format!("Chops · {}", x.name), format!("Chops layer · {}", x.name)].into_iter().enumerate() {
                if k == 1 && sec_chops[ai].iter().all(|s| s.1.is_empty()) {
                    continue;
                }
                let t = self.call_num("daw_track_add", &[Val::Str("sampler".into()), Val::Str(String::new())]);
                if t >= 0.0 {
                    let _ = self.dispatch("daw_track_set_name", &A(&[Val::Num(t), Val::Str(nm)]));
                    acap_chop_t[ai][k] = Some(t as usize);
                }
            }
        }
        // a riser into every drop a loop roll builds to
        let riser_into: Vec<usize> = (0..ns).filter(|i| mash.slots[*i].roll_after == Roll::Shrink && mash.slots.get(i + 1).map(|x| x.energy >= 0.7).unwrap_or(false)).collect();
        let mut riser_t: Option<usize> = None;
        if !riser_into.is_empty() && flag("risers") {
            let (kind, patch) = instrument_for(g, "riser", 0);
            let t = self.call_num("daw_track_add", &[Val::Str(kind), Val::Str(patch.unwrap_or_default())]);
            if t >= 0.0 {
                let _ = self.dispatch("daw_track_set_name", &A(&[Val::Num(t), Val::Str("Riser".into())]));
                riser_t = Some(t as usize);
            }
        }
        // the flow: noise sweeps into the drops the rolls build to (electronic genres, as songgen::sweeps does for a song)
        let flow = flag("flow");
        let electronic = !matches!(g.name, "folk" | "ballad" | "jazz" | "rock" | "orchestral" | "cinematic" | "ambient" | "lofi");
        let mut sweeps: Vec<(f64, f64)> = vec![];
        let mut gaps: Vec<(usize, f64)> = vec![];
        if flow {
            for (i, s) in secs.iter().enumerate() {
                if s.roll.map(|r| r.1) != Some(Roll::Shrink) || secs.get(i + 1).map(|n| n.energy < 0.7).unwrap_or(true) {
                    continue;
                }
                let f = feel_or(&emo, i);
                let gap: f64 = crate::emotion::gap_beats(&f).min(2.0);
                gaps.push((i, gap));
                if electronic {
                    let beats = (4.0 * (1.0 + (f.tension as f64 * 3.0).round())).min(s.beats() - gap);
                    sweeps.push((s.end - gap - beats, beats));
                }
            }
        }
        let mut sweep_t: Option<usize> = None;
        if !sweeps.is_empty() {
            let t = self.call_num("daw_track_add", &[Val::Str("prism".into()), Val::Str(String::new())]);
            if t >= 0.0 {
                let _ = self.dispatch("daw_track_set_name", &A(&[Val::Num(t), Val::Str("Noise Sweep".into())]));
                sweep_t = Some(t as usize);
            }
        }
        // the acapellas' tracks: the main vocal, and the stress tracks it needs (harmonies, double, octave, telephone, vocoder)
        let mut acap_t: Vec<Option<usize>> = vec![None; acaps.len()];
        for (ai, x) in acaps.iter().enumerate() {
            if !x.main || x.shifted.is_empty() {
                continue;
            }
            let t = self.call_num("daw_track_add", &[Val::Str("audio".into()), Val::Str(String::new())]);
            if t >= 0.0 {
                let _ = self.dispatch("daw_track_set_name", &A(&[Val::Num(t), Val::Str(format!("Acapella · {}", x.name))]));
                acap_t[ai] = Some(t as usize);
            }
        }
        let mut stress_t: HashMap<(usize, Stress, usize), usize> = HashMap::new();
        for c in &sclips {
            let k = (c.acap, c.how, c.voice);
            if stress_t.contains_key(&k) {
                continue;
            }
            let nm = match (c.how, c.voice) {
                (Stress::Harmony, 0) => "Harmony",
                (Stress::Harmony, _) => "Harmony low",
                (Stress::Double, _) => "Double",
                (Stress::Octave, _) => "Octave",
                (Stress::Telephone, _) => "Telephone",
                _ => "Vocoder in",
            };
            let t = self.call_num("daw_track_add", &[Val::Str("audio".into()), Val::Str(String::new())]);
            if t >= 0.0 {
                let _ = self.dispatch("daw_track_set_name", &A(&[Val::Num(t), Val::Str(format!("{nm} · {}", acaps[c.acap].name))]));
                stress_t.insert(k, t as usize);
            }
        }
        // a vocoder per acapella that has vocoded lines: a chord synth (the carrier) with the Vocoder, the line routed in
        let mut voc_t: HashMap<usize, usize> = HashMap::new();
        for (_, _, _, m) in &vocoded {
            if voc_t.contains_key(m) {
                continue;
            }
            let (kind, patch) = instrument_for(g, "pad", 1);
            let t = self.call_num("daw_track_add", &[Val::Str(kind), Val::Str(patch.unwrap_or_default())]);
            if t >= 0.0 {
                let _ = self.dispatch("daw_track_set_name", &A(&[Val::Num(t), Val::Str(format!("Vocoder · {}", acaps[*m].name))]));
                voc_t.insert(*m, t as usize);
            }
        }

        // ── the arrangement on the timeline ──
        let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((json, _)) = &chop_cfg {
            for t in extra[..2].iter().flatten() {
                if let Ok(smp) = crate::dsp::sampler::Sampler::from_config(sr, json, None) {
                    if let Some(tr) = en.project.tracks.get_mut(*t) {
                        tr.inst = Some(Box::new(smp));
                        tr.inst_kind = "sampler".into();
                    }
                }
            }
        }
        for (ai, x) in acaps.iter().enumerate() {
            for t in acap_chop_t[ai].iter().flatten() {
                if let Ok(smp) = crate::dsp::sampler::Sampler::from_config(sr, &x.bank_json, None) {
                    if let Some(tr) = en.project.tracks.get_mut(*t) {
                        tr.inst = Some(Box::new(smp));
                        tr.inst_kind = "sampler".into();
                    }
                }
            }
        }
        let empty = en.project.tracks.iter().all(|t| t.clips.is_empty());
        if empty {
            en.project.bpm = bpm.clamp(40.0, 240.0);
        }
        let mut spans: Vec<(f64, Roll)> = vec![];
        // (section, track, clip index) of the drum clips, for the fills
        let mut drum_clips: Vec<(usize, usize, usize)> = vec![];
        for (i, sc) in secs.iter().enumerate() {
            let at = sc.start;
            match (sc.slot, sc.roll) {
                (Some(si), _) => {
                    let s = &mash.slots[si];
                    for (li, l) in s.layers.iter().enumerate() {
                        let Some(t) = tracks[assign[si][li]] else { continue };
                        let p = &ph[l.phrase];
                        let name = if l.spine { format!("{} #{} · {}", p.file, p.index + 1, crate::medley::short_id(p.id)) } else { format!("{} #{} {}", p.file, p.index + 1, l.cat.name()) };
                        let mut c = Clip::midi(at, s.beats, &name);
                        if let ClipData::Midi(v) = &mut c.data {
                            v.extend(crate::medley::layer_notes(&ph, l, s.beats));
                        }
                        if let Some(tr) = en.project.tracks.get_mut(t) {
                            tr.clips.push(c);
                            if l.cat == Cat::Drums {
                                drum_clips.push((i, t, tr.clips.len() - 1));
                            }
                        }
                    }
                    for (k, label) in [(0usize, "chops"), (1, "chops layer"), (2, "breaks")] {
                        let notes = match k {
                            0 => &chop_notes[si].0,
                            1 => &chop_notes[si].1,
                            _ => &chop_notes[si].2,
                        };
                        if let (Some(t), false) = (extra[k], notes.is_empty()) {
                            let mut c = Clip::midi(at, s.beats, label);
                            if let ClipData::Midi(v) = &mut c.data {
                                v.extend(notes.iter().cloned());
                            }
                            if let Some(tr) = en.project.tracks.get_mut(t) {
                                tr.clips.push(c);
                                if k == 2 {
                                    drum_clips.push((i, t, tr.clips.len() - 1));
                                }
                            }
                        }
                    }
                }
                (None, Some((ri, kind))) => {
                    // a loop roll: the slot's layers shrinking into a lift, or the next slot's growing back out of a drop
                    let rs = &mash.slots[ri];
                    for (li, l) in rs.layers.iter().enumerate() {
                        let Some(t) = tracks[assign[ri][li]] else { continue };
                        let p = &ph[l.phrase];
                        let mut c = Clip::midi(at, crate::medley::ROLL_BEATS, &format!("{} #{} roll", p.file, p.index + 1));
                        if let ClipData::Midi(v) = &mut c.data {
                            v.extend(crate::medley::roll_notes(&crate::medley::layer_notes(&ph, l, rs.beats), rs.beats, kind));
                        }
                        if let Some(tr) = en.project.tracks.get_mut(t) {
                            tr.clips.push(c);
                            if l.cat == Cat::Drums {
                                drum_clips.push((i, t, tr.clips.len() - 1));
                            }
                        }
                    }
                    for (k, label) in [(0usize, "chops roll"), (1, "chops layer roll"), (2, "breaks roll")] {
                        let notes = match k {
                            0 => &chop_notes[ri].0,
                            1 => &chop_notes[ri].1,
                            _ => &chop_notes[ri].2,
                        };
                        if let (Some(t), false) = (extra[k], notes.is_empty()) {
                            let mut c = Clip::midi(at, crate::medley::ROLL_BEATS, label);
                            if let ClipData::Midi(v) = &mut c.data {
                                v.extend(crate::medley::roll_notes(notes, rs.beats, kind));
                            }
                            if let Some(tr) = en.project.tracks.get_mut(t) {
                                tr.clips.push(c);
                                if k == 2 {
                                    drum_clips.push((i, t, tr.clips.len() - 1));
                                }
                            }
                        }
                    }
                    let prev = i.checked_sub(1).and_then(|p| secs[p].slot).unwrap_or(ri);
                    if let (Some(t), true) = (riser_t, kind == Roll::Shrink && riser_into.contains(&prev)) {
                        let mut c = Clip::midi(at, crate::medley::ROLL_BEATS, "riser");
                        if let ClipData::Midi(v) = &mut c.data {
                            v.extend(crate::medley::riser_notes(mash.slots[(prev + 1).min(ns - 1)].key, crate::medley::ROLL_BEATS));
                        }
                        if let Some(tr) = en.project.tracks.get_mut(t) {
                            tr.clips.push(c);
                        }
                    }
                    spans.push((at, kind));
                }
                _ => {}
            }
            // the acapellas' chops of this section
            for (ai, row) in sec_chops.iter().enumerate() {
                for k in 0..2 {
                    let notes = if k == 0 { &row[i].0 } else { &row[i].1 };
                    let (Some(t), false) = (acap_chop_t[ai][k], notes.is_empty()) else { continue };
                    let label = match (plan.get(i).copied(), k) {
                        (Some(Vox::Full), _) => "into the vocal",
                        (Some(Vox::Both), 0) => "chops over the vocal",
                        (_, 0) => "chops",
                        _ => "chops layer",
                    };
                    let mut c = Clip::midi(at, sc.beats(), label);
                    if let ClipData::Midi(v) = &mut c.data {
                        v.extend(notes.iter().cloned());
                    }
                    if let Some(tr) = en.project.tracks.get_mut(t) {
                        tr.clips.push(c);
                    }
                }
            }
        }

        // ── the flow (songgen.rs): a fill into every new section, a crash on its downbeat, a snare roll through every build, the
        // breath before the drop ──
        let (mut fills, mut crashes, mut brolls) = (0usize, 0usize, 0usize);
        if flow {
            for i in 0..secs.len() {
                let f = feel_or(&emo, i);
                let first = drum_clips.iter().find(|d| d.0 == i).copied();
                let next_roll_build = secs.get(i + 1).map(|n| n.roll.map(|r| r.1) == Some(Roll::Shrink)).unwrap_or(false);
                // the build roll
                if secs[i].roll.map(|r| r.1) == Some(Roll::Shrink) {
                    let gap = gaps.iter().find(|g0| g0.0 == i).map(|g0| g0.1).unwrap_or(0.0);
                    let sec = songgen::Section { tag: "build".into(), bars: (secs[i].beats() / 4.0).round() as u32, energy: secs[i].energy };
                    let roll = from_n(&songgen::build_roll(&sec, &f, gap));
                    if !roll.is_empty() {
                        let target = first.map(|d| (d.1, Some(d.2))).or_else(|| drum_clips.first().map(|d| (d.1, None)));
                        if let Some((t, ci)) = target {
                            if let Some(tr) = en.project.tracks.get_mut(t) {
                                match ci.and_then(|ci| tr.clips.get_mut(ci)) {
                                    Some(c) => {
                                        if let ClipData::Midi(v) = &mut c.data {
                                            // the loop's own snares make way for the roll
                                            v.retain(|n| !matches!(n.key, 38 | 39 | 40));
                                            v.extend(roll);
                                        }
                                    }
                                    None => {
                                        let mut c = Clip::midi(secs[i].start, secs[i].beats(), "build roll");
                                        c.data = ClipData::Midi(roll);
                                        tr.clips.push(c);
                                    }
                                }
                                brolls += 1;
                            }
                        }
                    }
                    // the breath before the drop: every part stops `gap` beats before it (the chops have their own)
                    if gap > 0.0 {
                        let cut = secs[i].end - gap;
                        for tr in en.project.tracks.iter_mut() {
                            for c in tr.clips.iter_mut().filter(|c| (c.start - secs[i].start).abs() < 1e-6 && c.name.ends_with("roll")) {
                                let cs = c.start;
                                if let ClipData::Midi(v) = &mut c.data {
                                    v.retain(|n| cs + n.start < cut - 1e-6);
                                    for n in v.iter_mut() {
                                        if cs + n.start + n.len > cut {
                                            n.len = (cut - cs - n.start).max(0.02);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    continue;
                }
                let Some((_, t, ci)) = first else { continue };
                let len = secs[i].beats();
                let new_tag = secs.get(i + 1).map(|n| n.tag != secs[i].tag || n.slot.is_some()).unwrap_or(false);
                let Some(c) = en.project.tracks.get_mut(t).and_then(|tr| tr.clips.get_mut(ci)) else { continue };
                let ClipData::Midi(v) = &mut c.data else { continue };
                let mut nn = to_n(v);
                // the crash (and a kick under it) on the downbeat of a new section
                if i > 0 && secs[i - 1].tag != secs[i].tag && secs[i].slot.is_some() {
                    nn.retain(|n| !(n.start < 0.05 && n.key == crate::compose::K_CRASH));
                    nn.push(N { start: 0.0, len: 1.0, key: crate::compose::K_CRASH, vel: 0.85 });
                    if nn.iter().any(|n| n.key == 36 || n.key == 35) && !nn.iter().any(|n| n.start < 0.05 && (n.key == 36 || n.key == 35)) {
                        nn.push(N { start: 0.0, len: 0.25, key: 36, vel: 0.9 });
                    }
                    crashes += 1;
                }
                // a fill into a new section that is not a build (the build has the roll)
                if new_tag && !next_roll_build && secs[i].slot.is_some() {
                    let beats: f64 = if f.arousal < 0.4 || secs.get(i + 1).map(|n| n.energy < secs[i].energy - 0.15).unwrap_or(false) {
                        1.0
                    } else if f.arousal < 0.7 {
                        2.0
                    } else {
                        4.0
                    };
                    songgen::transition_fill(len, beats.min(len), f.arousal, &mut nn);
                    fills += 1;
                }
                nn.sort_by(|a0, b0| a0.start.total_cmp(&b0.start).then(a0.key.cmp(&b0.key)));
                *v = from_n(&nn);
            }
        }
        // the noise sweep: a Prism playing only its noise, a low-pass opening and a swell to the drop (as New AI Song's)
        if let Some(t) = sweep_t {
            if let Some(tr) = en.project.tracks.get_mut(t) {
                for (st, len) in &sweeps {
                    let mut c = Clip::midi(*st, *len, "sweep");
                    c.data = ClipData::Midi(vec![Note::new(0.0, *len, 72, 0.9)]);
                    tr.clips.push(c);
                }
                if let Some(inst) = tr.inst.as_mut() {
                    for (n, v) in [("a lvl", 0.0), ("b lvl", 0.0), ("sub", 0.0), ("noise", 1.0), ("noise c", 0.9), ("res", 0.35), ("keytr", 0.5), ("fenv", 0.5), ("amp a", 0.0), ("amp s", 1.0), ("amp r", 0.3), ("rv mix", 0.25), ("rv size", 0.7), ("drift", 0.0), ("glide", 0.0), ("out", 0.55)] {
                        inst.set_param(n, v);
                    }
                }
                let v = (tr.volume / 1.5).clamp(0.0, 1.0);
                let (mut cut, mut vol): (Vec<(f64, f32)>, Vec<(f64, f32)>) = (vec![(0.0, 0.12)], vec![(0.0, 0.0)]);
                for (st, len) in &sweeps {
                    let end = st + len;
                    cut.extend([(*st, 0.12), (st + len * 0.7, 0.5), (end - 0.02, 0.95), (end, 0.12)]);
                    vol.extend([(*st, 0.0), (st + len * 0.6, v * 0.35), (end - 0.02, v), (end, 0.0)]);
                }
                tr.auto.retain(|l| l.param != "cut" && l.param != "vol");
                tr.auto.push(AutoLane { param: "cut".into(), points: cut });
                tr.auto.push(AutoLane { param: "vol".into(), points: vol });
            }
        }
        // the riser swells through the build and is gone in the breath before the drop
        if let (Some(t), true) = (riser_t, flow) {
            if let Some(tr) = en.project.tracks.get_mut(t) {
                let v = (tr.volume / 1.5).clamp(0.0, 1.0);
                let mut pts: Vec<(f64, f32)> = vec![(0.0, v)];
                for (i, gap) in &gaps {
                    let s = &secs[*i];
                    let span = s.beats() - gap;
                    pts.extend([(s.start, v * 0.2), (s.start + span * 0.6, v * 0.5), (s.start + span - 0.01, v), (s.start + span, 0.0), (s.end - 0.01, 0.0), (s.end, v)]);
                }
                if pts.len() > 1 {
                    tr.auto.retain(|l| l.param != "vol");
                    tr.auto.push(AutoLane { param: "vol".into(), points: pts });
                }
            }
        }

        // ── the acapellas: the excerpts, the level lane, the stressed lines ──
        let fade = if opt("vocret").as_deref() == Some("cut") { 0.02 } else { 0.5 };
        let mut acap_said: Vec<String> = vec![];
        for (ai, x) in acaps.iter().enumerate() {
            let Some(t) = acap_t[ai] else { continue };
            let Some(tr) = en.project.tracks.get_mut(t) else { continue };
            let mut used_shifts: Vec<i32> = vec![];
            for i in 0..secs.len() {
                if owner[i] != ai {
                    continue;
                }
                let Some(e) = &excerpt[i] else { continue };
                let Some(smp) = x.shifted.get(&shift[i]).cloned() else { continue };
                let (mut at, mut from, mut len) = (e.at, e.from, e.len);
                if from < 0.0 {
                    at -= from;
                    len += from;
                    from = 0.0;
                }
                if len <= 0.1 {
                    continue;
                }
                tr.clips.push(Clip { start: at, len, name: format!("{} {}", x.name, names[i]), loop_len: 0.0, lyric_text: String::new(), lyric_lang: String::new(), tag: String::new(), data: ClipData::Audio { sample: smp, offset_s: from * spb, gain: 1.0, pitch: 0.0, warp: 0.0 } });
                if !used_shifts.contains(&shift[i]) {
                    used_shifts.push(shift[i]);
                }
            }
            tr.clips.sort_by(|a0, b0| a0.start.total_cmp(&b0.start));
            // the level lane over this acapella's sections (a section another acapella sings counts as not singing here)
            let level = 0.9f32;
            tr.volume = level;
            let mine: Vec<Vox> = (0..secs.len()).map(|i| if owner[i] == ai { plan[i] } else { Vox::Chops }).collect();
            let sp: Vec<(f64, f64)> = secs.iter().map(|s| (s.start, s.end)).collect();
            let pts = ac::vol_lane(&sp, &mine, level, fade, &holes[ai]);
            tr.auto.retain(|l| l.param != "vol");
            tr.auto.push(AutoLane { param: "vol".into(), points: pts });
            let sh: Vec<String> = used_shifts.iter().map(|s| format!("{s:+}")).collect();
            acap_said.push(format!(
                "{} ({}{} bpm {}{}, {}; {} semitones by section)",
                x.name,
                if x.bpm_src > 1.0 { format!("{:.0}", x.bpm_src) } else { "?".into() },
                "",
                x.bpm_how,
                if (x.stretch - 1.0).abs() > 0.005 { format!(", stretched x{:.2}", x.stretch) } else { String::new() },
                crate::theory::Key { tonic: x.key.0, minor: x.key.1 }.name(),
                if sh.is_empty() { "0".into() } else { sh.join(" / ") }
            ));
        }
        for x in acaps.iter().filter(|x| !x.main) {
            acap_said.push(format!("{} (chops only)", x.name));
        }
        // the stressed lines' clips and their tracks' sound
        for c in &sclips {
            let Some(t) = stress_t.get(&(c.acap, c.how, c.voice)).copied() else { continue };
            let Some(tr) = en.project.tracks.get_mut(t) else { continue };
            tr.clips.push(Clip { start: c.at, len: c.len, name: c.how.name().to_string(), loop_len: 0.0, lyric_text: String::new(), lyric_lang: String::new(), tag: String::new(), data: ClipData::Audio { sample: c.sample.clone(), offset_s: c.offset_s, gain: 1.0, pitch: 0.0, warp: 0.0 } });
            if tr.clips.len() > 1 {
                continue;
            }
            let (vol, pan) = match (c.how, c.voice) {
                (Stress::Harmony, 0) => (0.5, -0.35),
                (Stress::Harmony, _) => (0.42, 0.35),
                (Stress::Double, _) => (0.5, 0.45),
                (Stress::Octave, _) => (0.35, 0.0),
                (Stress::Telephone, _) => (0.85, 0.0),
                _ => (0.8, 0.0),
            };
            tr.volume = vol;
            tr.pan = pan;
            match c.how {
                Stress::Telephone => {
                    if let Some(mut fx) = EffectSlot::new("Vox Filter", sr) {
                        // about 300 Hz .. 3.4 kHz: a phone line
                        for (k, v) in [("hp", 0.73f32), ("lp", 0.41), ("res", 0.35), ("dyn", 0.0), ("mix", 1.0)] {
                            if let Some(pi) = fx.params.iter().position(|p| p.0 == k) {
                                fx.set(pi, v);
                            }
                        }
                        tr.effects.push(fx);
                    }
                    if let Some(mut fx) = EffectSlot::new("Drive", sr) {
                        fx.wet = 0.35;
                        tr.effects.push(fx);
                    }
                }
                Stress::Vocoder => tr.mute = true,
                _ => {}
            }
        }
        // the vocoder: chords held under each vocoded line, the line (a muted track) as the modulator
        for (m, vt) in &voc_t {
            let Some(src) = stress_t.get(&(*m, Stress::Vocoder, 0)).copied() else { continue };
            let lines: Vec<(usize, f64, f64)> = vocoded.iter().filter(|v| v.3 == *m).map(|v| (v.0, v.1, v.2)).collect();
            if let Some(tr) = en.project.tracks.get_mut(*vt) {
                for (i, s, e) in &lines {
                    let sec0 = secs[*i].start;
                    let (s, e) = (s - 0.1, e + 0.25);
                    let mut c = Clip::midi(s, e - s, "vocoder chords");
                    let mut notes: Vec<Note> = vec![];
                    let mut b = s;
                    let mut held: Vec<u8> = vec![];
                    let mut since = s;
                    while b < e {
                        let ch = ac::carrier_chord(secs[*i].key, &ctx[*i].1, b - sec0 + 0.01);
                        if ch != held {
                            for k in &held {
                                notes.push(Note::new(since - s, (b - since).max(0.1), *k, 0.8));
                            }
                            held = ch;
                            since = b;
                        }
                        b = (b + 1.0).floor().max(b + 0.25);
                    }
                    for k in &held {
                        notes.push(Note::new(since - s, (e - since).max(0.1), *k, 0.8));
                    }
                    c.data = ClipData::Midi(notes);
                    tr.clips.push(c);
                }
                if let Some(mut fx) = EffectSlot::new("Vocoder", sr) {
                    // aux only (the routed line), many bands, a quick release so the words stay clear
                    for (k, v) in [("source", 0.2f32), ("bands", 0.7), ("q", 0.6), ("attack", 0.1), ("release", 0.25), ("hiss", 0.35), ("flatten", 0.5), ("width", 0.6), ("modgain", 0.6), ("out", 0.85), ("mix", 1.0)] {
                        if let Some(pi) = fx.params.iter().position(|p| p.0 == k) {
                            fx.set(pi, v);
                        }
                    }
                    tr.effects.push(fx);
                }
                tr.volume = 0.8;
            }
            if src != *vt && !en.project.would_cycle(src, *vt) {
                en.project.routes.push(Route { kind: RouteKind::Mod, from: src, to: *vt, amount: 1.0, keys: vec![] });
            }
        }

        // the medley's sections on the ruler (added after the project's own tags), so the design, the recipes and the master
        // moves follow them
        {
            let text = secs.iter().map(|s| format!("{}:{}:{}", s.start, s.end, s.tag)).collect::<Vec<_>>().join(";");
            match en.project.meta.iter_mut().find(|(k, _)| k == "sections") {
                Some(slot) if empty || slot.1.trim().is_empty() => slot.1 = text,
                Some(slot) => slot.1 = format!("{};{text}", slot.1.trim_end_matches(';')),
                None => en.project.meta.push(("sections".into(), text)),
            }
            if empty {
                en.project.meta.retain(|(k, _)| k != "ai.genre");
                en.project.meta.push(("ai.genre".into(), g.name.to_string()));
            }
        }

        // ── per-role sound design and space (sounddesign.rs, space.rs), or a sound recipe where one fits (recipes.rs) ──
        let design_amt = opt("design").and_then(|v| v.parse::<f64>().ok()).unwrap_or_else(|| crate::sounddesign::genre_design(g.name).max(0.3)).clamp(0.0, 1.0);
        let dg = if style == chops::Style::Breakcore { "breakbeat" } else { g.name };
        let mut design_note = String::new();
        let mut recipe_note: Vec<String> = vec![];
        let mut recipe_tracks: Vec<usize> = vec![];
        let use_recipes = on("recipes");
        let mut jobs: Vec<(usize, &str)> = tracks.iter().zip(&wanted_role).filter_map(|(t, r)| t.map(|t| (t, *r))).collect();
        if let Some(t) = extra[2] {
            jobs.push((t, "drums"));
        }
        if let Some(t) = riser_t {
            jobs.push((t, "riser"));
        }
        if design_amt > 0.0 || use_recipes {
            let dsecs: Vec<crate::sounddesign::Sec> = secs.iter().map(|s| crate::sounddesign::Sec { tag: s.tag.to_string(), start: s.start, end: s.end }).collect();
            let lib = crate::recipes::library();
            let mut taken: Vec<String> = vec![];
            let recipe_notes = flag("recipenotes");
            let words = story.to_lowercase();
            for (t, role) in jobs.iter().copied() {
                if use_recipes && role != "drums" {
                    let energy = secs.iter().map(|s| s.energy).fold(0.0, f64::max).max(0.6);
                    if let Some(r) = crate::recipes::pick(&lib, role, dg, &words, energy, seed.wrapping_add(t as u64 * 7919), &taken) {
                        taken.push(r.id.clone());
                        if let Some(sw) = &r.composition.switching {
                            taken.extend(sw.set.iter().cloned());
                        }
                        // the notes too where the medley's role allows: the layered bass, chords, pads and arps (never the hook,
                        // which carries the medley's identity, never a lead, never a roll)
                        let kind = songgen::role_kind(role);
                        let clips: Vec<usize> = if recipe_notes && matches!(kind, crate::compose::Kind::Bass | crate::compose::Kind::Pad | crate::compose::Kind::Chords | crate::compose::Kind::Arp) {
                            en.project.tracks.get(t).map(|tr| tr.clips.iter().enumerate().filter(|(_, c)| matches!(c.data, ClipData::Midi(_)) && c.start >= start0 - 1e-6 && !c.name.contains(" · ") && !c.name.ends_with("roll")).map(|(k, _)| k).collect()).unwrap_or_default()
                        } else {
                            vec![]
                        };
                        let _ = super::recipe_api::apply_recipe(&mut en, t, r, &clips, dg, role, seed.wrapping_add(t as u64));
                        recipe_tracks.push(t);
                        let tn = en.project.tracks.get(t).map(|x| x.name.clone()).unwrap_or_default();
                        recipe_note.push(format!("{tn}: {}{}", r.name, if clips.is_empty() { String::new() } else { format!(" ({} clips written its way)", clips.len()) }));
                        continue;
                    }
                }
                if design_amt <= 0.0 {
                    continue;
                }
                let Some(tr) = en.project.tracks.get_mut(t) else { continue };
                let has: Vec<String> = tr.effects.iter().map(|e| e.name.clone()).collect();
                let inst = tr.inst_kind.clone();
                let pd = crate::sounddesign::design_at(dg, &words, role, songgen::role_kind(role), &inst, design_amt, key.0, &dsecs, &has, bpm);
                apply_design(tr, &pd, sr);
                crate::sounddesign::apply_inst(tr, &pd);
            }
            // the acapella and its stress tracks get the vocal's space
            if design_amt > 0.0 {
                let vt: Vec<usize> = acap_t.iter().flatten().copied().chain(stress_t.values().copied()).collect();
                for t in vt {
                    let Some(tr) = en.project.tracks.get_mut(t) else { continue };
                    if tr.mute {
                        continue;
                    }
                    let has: Vec<String> = tr.effects.iter().map(|e| e.name.clone()).collect();
                    let pd = crate::sounddesign::design_at(dg, &words, "vocal", crate::compose::Kind::Melody, "audio", design_amt * 0.6, key.0, &dsecs, &has, bpm);
                    apply_design(tr, &pd, sr);
                }
            }
            // the master moves of the genre's recipes (the master dips through each build, back on the drop)
            if use_recipes && flag("master") {
                for m in crate::recipes::master_moves(&lib, dg) {
                    let _ = super::recipe_api::apply_recipe(&mut en, 0, m, &[], dg, "", seed);
                    recipe_note.push(format!("master: {}", m.name));
                }
            }
            design_note = format!("; sound design: {dg} ({} space){}", crate::space::profile(dg).tier(), if recipe_tracks.is_empty() { String::new() } else { format!(", {} track(s) on sound recipes", recipe_tracks.len()) });
        }
        // the feel: the story / arc's emotional plan over the medley's sections, every medley track played with it
        let feel_amt = opt("feel").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.7).clamp(0.0, 1.0) as f32;
        let mut feel_note = String::new();
        let midi_tracks: Vec<usize> = tracks.iter().chain(extra.iter()).flatten().copied().chain(acap_chop_t.iter().flat_map(|r| r.iter().flatten().copied())).collect();
        if feel_amt > 0.0 {
            let fsecs: Vec<crate::feel::Sec> = secs.iter().map(|s| crate::feel::Sec { start: s.start, end: s.end, tag: s.tag.to_string() }).collect();
            let words: Vec<String> = vec![story.clone(); fsecs.len()];
            let fplan = crate::feel::plan_for(&en.project.tracks, &fsecs, &feel_arc, g.name, &words, key.1, seed);
            let _ = crate::feel::perform(&mut en.project.tracks, &midi_tracks, &fsecs, &fplan, feel_amt);
            let sections: Vec<songgen::Section> = fsecs.iter().map(|s| songgen::Section { tag: s.tag.clone(), bars: ((s.end - s.start) / 4.0).round().max(1.0) as u32, energy: 0.5 }).collect();
            feel_note = format!("; feel: {} ({})", fplan.arc, crate::emotion::summary(&fplan, &sections));
        }
        // the expression rules (sounddesign::expression: glide only where the genre slides, vibrato only on long expressive notes,
        // no stray pitch modulation) on every part a recipe did not already set up
        let mut expressed = 0usize;
        if flag("expr") {
            for (t, role) in jobs.iter().copied() {
                if role == "drums" || recipe_tracks.contains(&t) {
                    continue;
                }
                let x = crate::sounddesign::expression(dg, role, songgen::role_kind(role), bpm);
                if let Some(tr) = en.project.tracks.get_mut(t) {
                    if !crate::sounddesign::apply_expression(tr, &x).is_empty() {
                        expressed += 1;
                    }
                }
            }
        }
        // the rolls' filter contour on every medley MIDI track (the acapellas sit at 0 in the rolls, or sing over them)
        if !spans.is_empty() {
            let (hp, lp) = crate::medley::roll_contour(&spans);
            for t in midi_tracks.iter() {
                let Some(tr) = en.project.tracks.get_mut(*t) else { continue };
                let Some(mut fx) = EffectSlot::new("Vox Filter", sr) else { continue };
                for (k, v) in [("hp", 0.0f32), ("lp", 1.0), ("res", 0.3), ("dyn", 0.0), ("mix", 1.0)] {
                    if let Some(pi) = fx.params.iter().position(|p| p.0 == k) {
                        fx.set(pi, v);
                    }
                }
                let idx = tr.effects.len();
                let hpi = fx.params.iter().position(|p| p.0 == "hp");
                let lpi = fx.params.iter().position(|p| p.0 == "lp");
                tr.effects.push(fx);
                if let Some(pi) = hpi {
                    tr.auto.push(AutoLane { param: format!("fx{idx}.{pi}"), points: hp.clone() });
                }
                if let Some(pi) = lpi {
                    tr.auto.push(AutoLane { param: format!("fx{idx}.{pi}"), points: lp.clone() });
                }
            }
        }
        let end = secs.last().map(|s| s.end).unwrap_or(start0);
        drop(en);
        self.touch();

        // ── the sentence ──
        let mut path: Vec<String> = vec![];
        for s in &mash.slots {
            let k = crate::theory::Key { tonic: s.key.0, minor: s.key.1 }.name();
            if path.last() != Some(&k) {
                path.push(k);
            }
        }
        let layers: usize = mash.slots.iter().map(|s| s.layers.len()).sum();
        let foreign = mash.slots.iter().flat_map(|s| s.layers.iter().map(move |l| (l.phrase, s.spine))).filter(|(a0, b0)| ph[*a0].file != ph[*b0].file).count();
        let vocal_plan = if plan.is_empty() {
            String::new()
        } else {
            let parts: Vec<String> = (0..secs.len())
                .map(|i| {
                    let who = if mains.len() > 1 && plan[i].sings() { format!(" ({})", acaps[owner[i]].name) } else { String::new() };
                    format!("{} {}{who}", names[i], match plan[i] {
                        Vox::Chops => "chops",
                        Vox::Full => "vocal",
                        Vox::Both => "vocal + chops",
                    })
                })
                .collect();
            format!(
                "; acapellas: {}; vocal plan{}: {}{}",
                acap_said.join(", "),
                if replace { " (the chops replace the vocal, its level back up on the downbeat they end)" } else { "" },
                parts.join(" > "),
                if chop_tune.tuned + chop_tune.kept > 0 { format!("; chops tuned to the chords: {} of {} notes moved (median {:.0} cents)", chop_tune.tuned, chop_tune.tuned + chop_tune.kept, chop_tune.median_shift()) } else { String::new() }
            ) + &if stress_said.is_empty() { if stress_amt > 0.0 { "; no line stressed".to_string() } else { String::new() } } else { format!("; stressed lines: {}", stress_said.join("; ")) }
        };
        let flow_note = if flow && (fills + crashes + brolls + sweeps.len()) > 0 { format!("; flow: {fills} fill(s), {crashes} crash(es), {brolls} build roll(s), {} noise sweep(s)", sweeps.len()) } else { String::new() };
        ok!(format!(
            "{}: {} phrases from {} files ({} in all) at {:.0} bpm, {} bars; story: {}; keys: {}; {} clips ({} layered from other files) on {} instruments; {} loop rolls{}{}{}{}{}{}{}{}{}; order: {}{}",
            if layering > 0.05 { "Mashup" } else { "Medley" },
            order.len(),
            files,
            ph.len(),
            bpm,
            ((end - start0) / 4.0).round(),
            mash.arc,
            path.join(" > "),
            layers,
            foreign,
            tracks.iter().flatten().count(),
            spans.len(),
            match (&chop_cfg, extra[2].is_some()) {
                (Some((_, ch)), br) => format!("; vocal chops: {} chops at the transients of {chop_name} ({} pitched){}", ch.len(), ch.iter().filter(|c| c.midi.is_some()).count(), if br { ", with breaks" } else { "" }),
                (None, true) => "; breaks".to_string(),
                (None, false) if style != chops::Style::Off && acaps.is_empty() => "; no vocal chops (no usable vocal found)".to_string(),
                _ => String::new(),
            },
            vocal_plan,
            if acap_notes.is_empty() { String::new() } else { format!("; {}", acap_notes.join(", ")) },
            feel_note,
            if mash.log.is_empty() { String::new() } else { format!("; arrangement: {}", crate::medley::summarize(&mash.log)) },
            design_note,
            if recipe_note.is_empty() { String::new() } else { format!("; sound recipes: {}", recipe_note.join(", ")) },
            flow_note,
            if expressed > 0 { format!("; expression rules on {expressed} part(s)") } else { String::new() },
            a.s(3),
            if mode == crate::medley::Mode::Cipher && !message.is_empty() { format!(", the order spells a {}-character message (decode with the same folder and passphrase)", message.chars().count()) } else { String::new() }
        ))
    }
}

