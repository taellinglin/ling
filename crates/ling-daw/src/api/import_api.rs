//! Builtins for producing an import (`daw_imp_*`): after `daw_midi_import` has made the tracks, `daw_imp_produce` plays and
//! produces them the way New AI Song and Make Medley produce theirs, reusing their code: the analysis (key, tempo, sections on the
//! ruler: fix_api's section finder), the clean-up and the performance style (importprod.rs), the Articulator's lanes (artic_api),
//! the roles for Mix AI (mix_api's roles), the Nova production (nova_api `nova_dress`: song variants, story automation, sub / top
//! layers, switching siblings, the pump), the ducking under the kick (duck_api, with novagen's Ducker settings) and the master
//! chain (a Ling Master preset for the genre). The mix itself (levels, pan, EQ, compression, space and the loudness move into the
//! master's limiter) is Mix AI (`daw_mix_*`), which measures in the background: the caller starts it after this returns.
use super::{Daw, A, Val};
use crate::importprod::{self as ip, STYLES};
use crate::model::*;

/// "k=v;k=v" -> the value of `k`
fn opt<'a>(opts: &'a str, k: &str) -> Option<&'a str> {
    opts.split(';').filter_map(|kv| kv.split_once('=')).find(|(a, _)| a.trim() == k).map(|(_, v)| v.trim())
}

fn flag(opts: &str, k: &str) -> bool {
    matches!(opt(opts, k), Some("1" | "on" | "true" | "yes"))
}

impl Daw {
    pub(super) fn imp_dispatch(&mut self, name: &str, a: &A) -> Result<Val, String> {
        macro_rules! ok {
            ($v:expr) => {
                return Ok(Val::from($v))
            };
        }
        match name {
            // the version of these builtins (a script can check it before using one added later)
            "daw_imp_version" => ok!(2.0),
            // ── the catalogue (soundcat.rs): every sound the import can choose, one list the dialog, the pickers and the prompt share ──
            "daw_imp_sounds" => {
                // (kind, sample roots "a|b" ("" = the Sampler's)) -> [label, value, group, source]* : the sounds of one plugin kind, as
                // the dialog lists them (value = the choice string daw_midi_import takes)
                let roots = if a.s(1).is_empty() { self.smp_roots.clone() } else { a.s(1) };
                let cat = crate::soundcat::catalogue(&roots);
                let k = a.s(0);
                ok!(Val::List(cat.iter().filter(|s| s.kind == k).flat_map(|s| [Val::Str(s.label.clone()), Val::Str(s.value.clone()), Val::Str(s.group.to_string()), Val::Str(s.source.to_string())]).collect()))
            }
            "daw_imp_catalog" => {
                // (sample roots) -> [source, count]* : how many sounds each source has
                let roots = if a.s(0).is_empty() { self.smp_roots.clone() } else { a.s(0) };
                let cat = crate::soundcat::catalogue(&roots);
                ok!(Val::List(crate::soundcat::counts(&cat).into_iter().flat_map(|(s, n)| [Val::Str(s.to_string()), Val::Num(n as f64)]).collect()))
            }
            "daw_imp_choice" => {
                // (kind, label or name) -> the choice string for daw_midi_import, "" when the catalogue has no such sound
                let cat = crate::soundcat::catalogue(&self.smp_roots);
                ok!(crate::soundcat::find_sound(&cat, &a.s(0), &a.s(1)).map(|s| s.value.clone()).unwrap_or_default())
            }
            "daw_imp_kit" => ok!(crate::soundcat::kit_for(&a.s(0), &a.s(1), a.n(2)).to_string()),
            "daw_imp_pick" => {
                // (path, model reply ("" = the rules), genre ("" = guessed), seed) -> [part, kind, label, value, why, role]* for every part:
                // the picker's choice as the dialog lists it (label) and as the import takes it (value)
                let Ok(data) = std::fs::read(a.s(0)) else { return ok!(Val::List(vec![])) };
                let Ok(m) = crate::midi_import::parse_any(&a.s(0), &data) else { return ok!(Val::List(vec![])) };
                let reply = crate::ai_import::parse_reply(&a.s(1));
                let genre = if a.s(2).trim().is_empty() { crate::ai_import::guess_genre(&m).to_string() } else { a.s(2).trim().to_lowercase() };
                let c = crate::ai_import::choose_seeded(&m, &reply, &genre, a.n(3).abs() as u64);
                ok!(Val::List(c.into_iter().flat_map(|c| [Val::Num(c.part as f64), Val::Str(c.plugin), Val::Str(c.patch), Val::Str(c.value), Val::Str(c.why), Val::Str(c.role)]).collect()))
            }
            "daw_imp_perf_styles" => ok!(Val::List(STYLES.iter().map(|s| Val::Str(s.to_string())).collect())),
            "daw_imp_master_preset" => ok!(ip::master_preset(&a.s(0)).to_string()),
            "daw_imp_mix_style" => {
                // (genre) -> [brightness, punch, width] for daw_mix_style
                let (b, p, w) = ip::mix_style(&a.s(0));
                ok!(Val::List(vec![Val::Num(b), Val::Num(p), Val::Num(w)]))
            }
            "daw_imp_produce" => {
                let r = self.imp_produce(a.n(0), a.n(1), &a.s(2), &a.s(3));
                ok!(r)
            }
            _ => Err(format!("unknown builtin {name}")),
        }
    }

    /// (first track, track count (<= 0 = to the end), genre ("" = the project's / guessed), options) -> the report. Options
    /// "k=v;...": perf=<style> (importprod::STYLES), amt=0..1, parts=<style>|<style>|... (per track from `first`; "" = the song's
    /// style), seed=, loud=<loudness preset index (loudness::PRESETS), 0 = the genre's>, and the stages, each 0 / 1 (off when not
    /// given): analyse, clean, roles, nova, duck, master. The performance runs whenever its style is not "off".
    fn imp_produce(&mut self, first: f64, count: f64, genre_in: &str, opts: &str) -> String {
        let eng = self.out.engine.clone();
        let sr = self.out.sr;
        let seed = opt(opts, "seed").and_then(|v| v.parse::<f64>().ok()).unwrap_or(1.0).abs() as u64;
        let (which, genre, bpm) = {
            let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
            let n = en.project.tracks.len();
            let f = (first.max(0.0) as usize).min(n);
            let which: Vec<usize> = if count <= 0.0 { (f..n).collect() } else { (f..f.saturating_add(count as usize).min(n)).collect() };
            let meta_g = en.project.meta.iter().find(|(k, _)| k == "ai.genre").map(|(_, v)| v.trim().to_string()).unwrap_or_default();
            let g = if !genre_in.trim().is_empty() { genre_in.trim().to_lowercase() } else if !meta_g.is_empty() { meta_g } else { "pop".to_string() };
            // the genre the mix, the space, the Nova parts and the master follow (only when a stage runs: a plain performance
            // leaves the project's genre alone)
            if ["analyse", "clean", "roles", "nova", "duck", "mix", "master"].iter().any(|k| flag(opts, k)) {
                en.project.meta.retain(|(k, _)| k != "ai.genre");
                en.project.meta.push(("ai.genre".into(), g.clone()));
            }
            (which, g, en.project.bpm)
        };
        if which.is_empty() {
            return "Nothing to produce: no imported tracks".to_string();
        }
        let mut said: Vec<String> = vec![];

        // ── analyse: the key, the tempo, the sections (tagged on the ruler unless it has tags) ──
        let tonic = {
            let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
            let mut h = [0f64; 12];
            for t in which.iter().filter_map(|t| en.project.tracks.get(*t)).filter(|t| t.inst_kind != "drums") {
                for c in &t.clips {
                    if let ClipData::Midi(ns) = &c.data {
                        for x in ns {
                            h[(x.key % 12) as usize] += x.len.max(0.1);
                        }
                    }
                }
            }
            let (key, _, _) = crate::theory::detect_key_hist(&h);
            if flag(opts, "analyse") {
                if !en.project.meta.iter().any(|(k, v)| k == "song_key" && !v.trim().is_empty()) {
                    en.project.meta.push(("song_key".into(), key.name()));
                }
                let tagged = en.project.meta.iter().any(|(k, v)| k == "sections" && !v.trim().is_empty());
                let mut how = "the ruler's tags kept";
                if !tagged {
                    let (secs, _) = super::fix_api::sections_of(&en);
                    // a dense file the arranger's finder reads as one long tag: the feel's energy sections instead
                    let one_tag = secs.len() > 1 && secs.iter().all(|x| x.tag == secs[0].tag);
                    let text: String = if secs.is_empty() || one_tag {
                        let end = en.project.end_beat();
                        crate::feel::infer_sections(&en.project.tracks, end).iter().map(|s| format!("{}:{}:{}", s.start, s.end, s.tag)).collect::<Vec<_>>().join(";")
                    } else {
                        secs.iter().map(|s| format!("{}:{}:{}", s.start_bar as f64 * 4.0, s.end_bar as f64 * 4.0, s.tag)).collect::<Vec<_>>().join(";")
                    };
                    if !text.is_empty() {
                        en.project.meta.retain(|(k, _)| k != "sections");
                        en.project.meta.push(("sections".into(), text));
                        how = "tagged on the ruler";
                    }
                }
                let secs = super::song_api::project_sections(&en);
                let mut path: Vec<String> = secs.iter().take(8).map(|s| s.tag.clone()).collect();
                if secs.len() > 8 {
                    path.push("...".into());
                }
                said.push(format!("analysed: {}, {:.0} bpm, {} sections ({}) {how}", key.name(), bpm, secs.len(), path.join(" > ")));
            }
            key.tonic
        };

        // ── clean-up, then the performance ──
        let song_style = ip::style_name(opt(opts, "perf").unwrap_or("off"));
        let amt = opt(opts, "amt").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.6).clamp(0.0, 1.0);
        let per: Vec<&str> = opt(opts, "parts").map(|p| p.split('|').collect()).unwrap_or_default();
        let style_of = |k: usize| -> &'static str {
            match per.get(k).map(|s| s.trim()).filter(|s| !s.is_empty() && *s != "song") {
                Some(s) => ip::style_name(s),
                None => song_style,
            }
        };
        let mut artic_jobs: Vec<(usize, f64)> = vec![];
        {
            let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
            if flag(opts, "clean") {
                let mut c = ip::Cleaned::default();
                for t in &which {
                    if let Some(tr) = en.project.tracks.get_mut(*t) {
                        c.add(&ip::clean(tr));
                    }
                }
                said.push(if c.total() == 0 {
                    "clean-up: nothing stuck, doubled or overlapping".to_string()
                } else {
                    format!("clean-up: {} duplicate(s), {} overlap(s), {} stuck note(s), {} past their clip, {} sliver(s)", c.dupes, c.overlaps, c.stuck, c.trimmed, c.slivers)
                });
            }
            let mut styles_used: Vec<String> = vec![];
            let (mut moved, mut quant, mut parts) = (0usize, 0usize, 0usize);
            for (k, t) in which.iter().enumerate() {
                let st = style_of(k);
                if st == "off" {
                    continue;
                }
                let Some(tr) = en.project.tracks.get_mut(*t) else { continue };
                let d = ip::perform(tr, st, amt, seed, *t);
                if d.notes == 0 {
                    continue;
                }
                parts += 1;
                moved += d.moved;
                quant += d.quantized;
                if !styles_used.iter().any(|s| s == st) {
                    styles_used.push(st.to_string());
                }
                let sp = ip::spec(st);
                if sp.artic > 0.0 && tr.inst_kind != "drums" && tr.inst.is_some() {
                    artic_jobs.push((*t, sp.artic * amt));
                }
            }
            if parts > 0 {
                said.push(format!("performance: {} at {:.0}% on {parts} part(s) ({moved} notes moved, {quant} pulled towards the grid)", styles_used.join(" / "), amt * 100.0));
            }
        }
        // the Articulator's lanes for the expressive styles (the local chooser, by the song's form)
        let mut lanes = 0usize;
        for (t, am) in artic_jobs {
            let n = match self.dispatch("daw_artic_begin", &A(&[Val::Str("song".into()), Val::Num(t as f64), Val::Num(0.0), Val::Num(seed as f64 + t as f64), Val::Num(am.clamp(0.05, 1.0)), Val::Str(String::new()), Val::Num(0.0)])) {
                Ok(Val::Num(x)) => x as usize,
                _ => 0,
            };
            for i in 0..n {
                if let Ok(Val::Num(x)) = self.dispatch("daw_artic_apply", &A(&[Val::Num(i as f64), Val::Str(String::new())])) {
                    lanes += x.max(0.0) as usize;
                }
            }
            let _ = self.dispatch("daw_artic_end", &A(&[]));
        }
        if lanes > 0 {
            said.push(format!("articulation: {lanes} lane(s) from the Articulator"));
        }

        // ── roles: what every track is (the mix, the Nova layers and the ducking read them) ──
        let roles: Vec<(usize, &'static str, crate::mixai::Role)> = {
            let en = eng.lock().unwrap_or_else(|p| p.into_inner());
            which.iter().filter_map(|t| en.project.tracks.get(*t).map(|tr| (*t, ip::role_word(tr), ip::mix_role(ip::role_word(tr))))).collect()
        };
        if flag(opts, "roles") {
            let mut tally: Vec<(&str, usize)> = vec![];
            for (t, _, r) in &roles {
                let _ = self.dispatch("daw_mix_set_role", &A(&[Val::Num(*t as f64), Val::Str(r.name().into())]));
                match tally.iter_mut().find(|x| x.0 == r.name()) {
                    Some(x) => x.1 += 1,
                    None => tally.push((r.name(), 1)),
                }
            }
            said.push(format!("roles: {}", tally.iter().map(|(r, n)| format!("{n} {r}")).collect::<Vec<_>>().join(", ")));
        }

        // ── Nova: each Nova part designed for the song (nova_api, as New AI Song and Make Medley do) ──
        let mut made: Vec<usize> = vec![];
        if flag(opts, "nova") {
            let secs = {
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                super::song_api::project_sections(&en)
            };
            let mut lines = vec![];
            // one bass trades its drop bars with sibling presets (the main one: the first Nova bass); the others keep theirs
            let mut switched = false;
            for (t, word, r) in &roles {
                let bass = *r == crate::mixai::Role::Bass || word.contains("bass");
                let nova = {
                    let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    en.project.tracks.get(*t).map_or(false, |x| x.inst_kind == "nova")
                };
                let switch = bass && nova && !switched;
                switched |= switch;
                let (m, line) = self.nova_dress(*t, &secs, seed.wrapping_add(*t as u64 * 31), switch);
                made.extend(m);
                if !line.is_empty() {
                    lines.push(line);
                }
            }
            if !lines.is_empty() {
                said.push(format!("Nova: {}", lines.join("; ")));
            }
        }

        // ── ducking under the kick where the genre pumps (the Nova tracks did theirs above) ──
        if flag(opts, "duck") {
            let ctx = crate::novagen::Ctx::new(&genre, bpm, tonic, seed);
            let kick: Option<usize> = {
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                en.project.tracks.iter().position(|tr| {
                    tr.inst_kind == "drums" && tr.clips.iter().any(|c| matches!(&c.data, ClipData::Midi(ns) if ns.iter().any(|n| n.key == 35 || n.key == 36)))
                })
            };
            let mut ducked = 0usize;
            let mut deepest = 0f32;
            if let Some(src) = kick {
                for (t, word, r) in &roles {
                    let Some(role) = ip::duck_role(*r, word) else { continue };
                    let Some(set) = crate::novagen::ducker(role, crate::novagen::Layer::Main, &ctx) else { continue };
                    let has = {
                        let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                        *t == src || en.project.routes.iter().any(|x| x.kind == RouteKind::Duck && x.to == *t)
                    };
                    if has {
                        continue;
                    }
                    let fi = match self.dispatch("daw_duck_add", &A(&[Val::Num(src as f64), Val::Num(*t as f64), Val::Str("35,36".into()), Val::Num(1.0)])) {
                        Ok(Val::List(v)) if v.len() >= 2 => match v[1] {
                            Val::Num(x) => x as usize,
                            _ => continue,
                        },
                        _ => continue,
                    };
                    let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    if let Some(slot) = en.project.tracks.get_mut(*t).and_then(|tr| tr.effects.get_mut(fi)) {
                        for (k, v) in &set {
                            if let Some(pi) = slot.params.iter().position(|q| q.0 == *k) {
                                slot.set(pi, *v);
                            }
                        }
                    }
                    ducked += 1;
                    deepest = deepest.max(set.iter().find(|s| s.0 == "depth").map(|s| s.1 * 30.0).unwrap_or(0.0));
                }
            }
            let nova_ducks = {
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                en.project.routes.iter().filter(|x| x.kind == RouteKind::Duck && en.project.tracks.get(x.to).map_or(false, |t| t.inst_kind == "nova")).count()
            };
            said.push(match (kick, ducked) {
                (None, _) => "ducking: no kick to duck under".to_string(),
                (_, 0) if crate::novagen::ducker("pad", crate::novagen::Layer::Main, &ctx).is_none() && crate::novagen::ducker("bass", crate::novagen::Layer::Main, &ctx).is_none() => format!("ducking: none ({genre} does not pump)"),
                (_, 0) => format!("ducking: {nova_ducks} Nova track(s), nothing else to duck"),
                (_, d) => format!("ducking: {d} track(s) under the kick (up to {deepest:.0} dB){}", if nova_ducks > 0 { format!(", {nova_ducks} Nova route(s)") } else { String::new() }),
            });
        }

        // ── the master chain: a Ling Master preset for the genre and the loudness target Mix AI drives it to ──
        if flag(opts, "master") {
            let pname = ip::master_preset(&genre);
            let li = opt(opts, "loud").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
            let (lufs, why) = match crate::loudness::PRESETS.get(li).filter(|p| p.lufs.is_finite()) {
                Some(p) => (p.lufs, p.name.to_string()),
                None => (crate::loudness::genre_target(crate::space::profile(&genre).genre), format!("the {genre} target")),
            };
            let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
            let has = en.project.master_effects.iter().any(|e| e.name == "Ling Master" && !e.bypass);
            let line = if has {
                format!("master: kept the Ling Master already there, aiming at {lufs:.1} LUFS ({why})")
            } else if let Some(mut slot) = EffectSlot::new("Ling Master", sr) {
                if let Some(pr) = crate::dsp::fx::presets("Ling Master").iter().find(|p| p.0 == pname) {
                    for (pn, v) in pr.1 {
                        if let Some(i) = slot.params.iter().position(|x| x.0 == *pn) {
                            slot.set(i, *v);
                        }
                    }
                }
                // one limiter only: a plain Limiter already on the master gives way to the Ling Master's
                for e in en.project.master_effects.iter_mut().filter(|e| e.name == "Limiter") {
                    e.bypass = true;
                }
                en.project.master_effects.push(slot);
                en.project.limiter = true;
                format!("master: Ling Master \"{pname}\" at the end of the chain, aiming at {lufs:.1} LUFS ({why})")
            } else {
                String::new()
            };
            if !line.is_empty() {
                said.push(line);
            }
        }
        if !made.is_empty() {
            said.push(format!("{} layer track(s) added", made.len()));
        }
        self.touch();
        if said.is_empty() {
            return "Imported as written (no production stage on)".to_string();
        }
        format!("Produced as {genre}: {}", said.join("; "))
    }
}
