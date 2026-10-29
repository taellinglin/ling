//! Sound recipes in the app (`daw_recipe_*`): list, read, suggest and apply the recipes of recipes.rs to tracks and clips, for the
//! Sound Recipes window, the AI song build, Medley and the Project Agent.
use super::{Daw, A, Val};
use crate::model::*;
use crate::recipes::{self, Recipe};
use crate::sounddesign::{self as sd, Phase, Sec};

/// the project's song key ("song_key" meta), if set
fn song_key(en: &crate::engine::Engine) -> Option<crate::theory::Key> {
    en.project.meta.iter().find(|(k, _)| k == "song_key").and_then(|(_, v)| crate::theory::parse_key(v))
}

/// the project's genre ("ai.genre" meta), "" when unknown
pub(super) fn project_genre(en: &crate::engine::Engine) -> String {
    en.project.meta.iter().find(|(k, _)| k == "ai.genre").map(|(_, v)| v.clone()).unwrap_or_default()
}

/// the sections to apply over: the ruler's tags, or (none tagged) the whole project as one drop
pub(super) fn sections_for(en: &crate::engine::Engine) -> Vec<Sec> {
    let tagged = en.project.meta.iter().any(|(k, v)| k == "sections" && !v.trim().is_empty());
    let mut s = super::song_api::project_sections(en);
    if !tagged {
        for x in s.iter_mut() {
            x.tag = "drop".into();
        }
    }
    s
}

fn phase_at(secs: &[Sec], beat: f64) -> (Phase, String) {
    secs.iter().find(|s| beat >= s.start - 1e-6 && beat < s.end - 1e-6).or(secs.last()).map(|s| (sd::phase(&s.tag), s.tag.clone())).unwrap_or((Phase::Drop, "drop".into()))
}

/// every non-drum note of the project between two beats, relative to the first
fn notes_between(en: &crate::engine::Engine, a: f64, b: f64) -> Vec<crate::theory::N> {
    let mut v = vec![];
    for tr in &en.project.tracks {
        if tr.inst_kind == "drums" {
            continue;
        }
        for c in &tr.clips {
            let ClipData::Midi(ns) = &c.data else { continue };
            for n in ns {
                let (s, e) = (c.start + n.start, c.start + n.start + n.len);
                if n.start < c.len && e > a && s < b {
                    v.push(crate::theory::N { start: (s - a).max(0.0), len: (e.min(b) - s.max(a)).max(0.05), key: n.key, vel: n.vel });
                }
            }
        }
    }
    v
}

/// a Duck route from the first drum track to `t` (the kick ducks the part), when the recipe asks for a sidechain
fn ensure_duck(en: &mut crate::engine::Engine, t: usize, r: &Recipe) -> bool {
    let Some(side) = &r.mix.sidechain else { return false };
    let Some(d) = en.project.tracks.iter().position(|x| x.inst_kind == "drums") else { return false };
    if d == t || en.project.would_cycle(d, t) {
        return false;
    }
    let keys = super::duck_api::parse_keys(&side.keys);
    match en.project.routes.iter().position(|x| x.kind == RouteKind::Duck && x.from == d && x.to == t) {
        Some(i) => en.project.routes[i].keys = keys,
        None => en.project.routes.push(Route { kind: RouteKind::Duck, from: d, to: t, amount: 1.0, keys }),
    }
    true
}

/// lane points over [a, b) replaced by `pts` (beats from a)
fn merge_lane(tr: &mut Track, param: &str, a: f64, b: f64, pts: &[(f64, f32)]) {
    let mut keep: Vec<(f64, f32)> = tr.auto.iter().find(|l| l.param == param).map(|l| l.points.iter().copied().filter(|p| p.0 < a - 1e-6 || p.0 >= b + 1e-6).collect()).unwrap_or_default();
    keep.extend(pts.iter().map(|(x, v)| (a + x, *v)));
    keep.sort_by(|x, y| x.0.total_cmp(&y.0));
    keep.dedup_by(|y, x| (y.0 - x.0).abs() < 1e-6);
    tr.auto.retain(|l| l.param != param);
    if !keep.is_empty() {
        tr.auto.push(AutoLane { param: param.to_string(), points: keep });
    }
}

/// the context a recipe is applied with on this project
fn ctx_for(en: &crate::engine::Engine, r: &Recipe, genre: &str, role: &str) -> recipes::Ctx {
    let g = if genre.is_empty() { project_genre(en) } else { genre.to_string() };
    let g = if g.is_empty() || recipes::genre_fit(r, &g) == 0.0 { r.genres.first().cloned().unwrap_or(g) } else { g };
    recipes::Ctx { genre: g, bpm: en.project.bpm, secs: sections_for(en), sr: en.sr, role: role.to_string() }
}

/// Write a clip's notes the way the recipe plays them (and its switching set's bars on sibling tracks). Returns what was done.
pub(super) fn write_clip(en: &mut crate::engine::Engine, t: usize, ci: usize, r: &Recipe, seed: u64, secs: &[Sec], genre: &str) -> Vec<String> {
    let lib = recipes::library();
    let Some((start, len, src)) = en.project.tracks.get(t).and_then(|tr| tr.clips.get(ci)).and_then(|c| match &c.data {
        ClipData::Midi(ns) => Some((c.start, c.len, ns.clone())),
        _ => None,
    }) else {
        return vec!["that is not a MIDI clip".into()];
    };
    let notes = notes_between(en, start, start + len);
    let fallback = song_key(en).unwrap_or(crate::theory::Key::new(9, true));
    let (chords, key) = recipes::harmony(&notes, len, fallback);
    let key = song_key(en).unwrap_or(key);
    let (phase, _) = phase_at(secs, start + 0.01);
    let energy = match phase {
        Phase::Drop => 0.95,
        Phase::Build => 0.75,
        Phase::Mid => 0.65,
        _ => 0.45,
    };
    let w = recipes::write(r, &recipes::WriteCtx { len, chords: &chords, key, phase, energy, seed, source: &src, bar0: (start / 4.0).floor().max(0.0) as usize });
    let mut said = w.said.clone();
    let base_name = en.project.tracks[t].name.clone();
    let color = en.project.tracks[t].color;
    let set: Vec<String> = r.composition.switching.as_ref().map(|s| s.set.clone()).unwrap_or_else(|| vec![r.id.clone()]);
    let members: std::collections::BTreeSet<usize> = w.notes.iter().map(|x| x.0).collect();
    for m in members.iter().copied().chain(std::iter::once(0)).collect::<std::collections::BTreeSet<usize>>() {
        let ns: Vec<Note> = w.notes.iter().filter(|x| x.0 == m).map(|x| x.1.clone()).collect();
        let target = if m == 0 {
            t
        } else {
            let Some(sr) = set.get(m).and_then(|id| lib.get(id)) else { continue };
            let name = format!("{base_name} · {}", sr.name);
            match en.project.tracks.iter().position(|x| x.name == name) {
                Some(i) => i,
                None => {
                    if en.project.tracks.len() >= MAX_TRACKS {
                        continue;
                    }
                    let mut tr = Track::new(&name, TrackKind::Instrument, color, None);
                    let c = ctx_for(en, sr, genre, "");
                    recipes::apply_sound(&mut tr, sr, &c);
                    en.project.tracks.push(tr);
                    let i = en.project.tracks.len() - 1;
                    ensure_duck(en, i, sr);
                    said.push(format!("{} on a new track", sr.name));
                    i
                }
            }
        };
        let tr = &mut en.project.tracks[target];
        if m == 0 {
            if let Some(ClipData::Midi(v)) = tr.clips.get_mut(ci).map(|c| &mut c.data) {
                *v = ns;
            }
        } else {
            // the sibling's clips over this stretch are replaced by its bars
            tr.clips.retain(|c| c.end() <= start + 1e-6 || c.start >= start + len - 1e-6);
            let mut c = Clip::midi(start, len, &r.name);
            c.data = ClipData::Midi(ns);
            tr.clips.push(c);
            tr.clips.sort_by(|a, b| a.start.total_cmp(&b.start));
        }
        if m == 0 {
            for (p, pts) in &w.lanes {
                merge_lane(&mut en.project.tracks[t], p, start, start + len, pts);
            }
        } else if let Some(sr) = set.get(m).and_then(|id| lib.get(id)) {
            // the sibling plays under the same expression rules as its recipe says
            let g = if genre.is_empty() { sr.genres.first().cloned().unwrap_or_default() } else { genre.to_string() };
            let x = recipes::expression(sr, &g, en.project.bpm);
            let _ = sd::apply_expression(&mut en.project.tracks[target], &x);
        }
    }
    said
}

/// Apply a recipe to a track: its sound, the duck route, and (with `clips`) its way of playing on those clips; the expression rules
/// last. Returns a sentence.
pub(super) fn apply_recipe(en: &mut crate::engine::Engine, t: usize, r: &Recipe, clips: &[usize], genre: &str, role: &str, seed: u64) -> String {
    let secs = sections_for(en);
    if r.is_master() {
        let mv = en.project.master_volume;
        let mut dummy = Track::new("", TrackKind::Instrument, 0, None);
        let n = recipes::render_automation(&mut dummy, r, &secs, Some((&mut en.project.master_auto, mv)));
        return format!("{}: {n} master lane(s) over the sections", r.name);
    }
    if t >= en.project.tracks.len() {
        return "no such track".into();
    }
    if matches!(en.project.tracks[t].kind, TrackKind::Audio) {
        return "an audio track cannot take a recipe (it has no instrument)".into();
    }
    let c = ctx_for(en, r, genre, role);
    let mut said = recipes::apply_sound(&mut en.project.tracks[t], r, &c);
    if ensure_duck(en, t, r) {
        said.push("ducks under the kick".into());
    }
    for ci in clips {
        said.extend(write_clip(en, t, *ci, r, seed.wrapping_add(*ci as u64 * 131), &c.secs, &c.genre));
    }
    let x = recipes::expression(r, &c.genre, c.bpm);
    let ex = sd::apply_expression(&mut en.project.tracks[t], &x);
    if !ex.is_empty() {
        said.push(ex);
    }
    said.dedup();
    let fx: Vec<String> = en.project.tracks[t].effects.iter().map(|e| e.name.clone()).collect();
    format!("{}: {} ({}; {}){}", en.project.tracks[t].name, r.name, r.instrument, if fx.is_empty() { "no effects".to_string() } else { fx.join(" > ") }, if said.is_empty() { String::new() } else { format!("; {}", said.join(", ")) })
}

fn list_row(r: &Recipe) -> [Val; 6] {
    [Val::Str(r.id.clone()), Val::Str(r.name.clone()), Val::Str(r.role.clone()), Val::Str(r.genres.join(", ")), Val::Str(r.instrument.clone()), Val::Str(recipes::summary(r))]
}

/// the role a request or a track name points at ("" = none)
fn role_word(words: &str) -> &'static str {
    let w = format!(" {} ", words.to_lowercase().replace([',', '.', '!', '?'], " "));
    for (k, role) in [(" sub", "sub bass"), (" 808", "sub bass"), (" bass", "bass"), (" lead", "lead"), (" chord", "chord"), (" stab", "chord"), (" pad", "pad"), (" arp", "arp"), (" pluck", "arp"), (" riser", "riser"), (" sweep", "riser"), (" impact", "fx")] {
        if w.contains(k) {
            return role;
        }
    }
    ""
}

impl Daw {
    pub(super) fn recipe_dispatch(&mut self, name: &str, a: &A) -> Result<Val, String> {
        let eng = self.out.engine.clone();
        macro_rules! ok {
            ($v:expr) => {
                return Ok(Val::from($v))
            };
        }
        match name {
            "daw_recipe_list" => {
                // (role "" = all, genre "" = all) -> [id, name, role, genres, instrument, summary]*
                let lib = recipes::library();
                let (role, genre) = (a.s(0).to_lowercase(), a.s(1).to_lowercase());
                let mut v = vec![];
                let mut rs: Vec<&Recipe> = lib.recipes.iter().filter(|r| (role.is_empty() || r.role == role) && (genre.is_empty() || r.genres.iter().any(|g| *g == genre) || r.styles.iter().any(|g| *g == genre))).collect();
                // by role (bass, lead, chord, pad, arp, fx), then as the files list them
                rs.sort_by_key(|r| recipes::ROLES.iter().position(|x| *x == r.role).unwrap_or(9));
                for r in rs {
                    v.extend(list_row(r));
                }
                ok!(Val::List(v))
            }
            "daw_recipe_genres" => {
                // -> every genre the recipes name, sorted
                let lib = recipes::library();
                let mut g: Vec<String> = lib.recipes.iter().flat_map(|r| r.genres.iter().cloned()).collect();
                g.sort();
                g.dedup();
                ok!(Val::List(g.into_iter().map(Val::Str).collect()))
            }
            "daw_recipe_info" => {
                // (id) -> [name, role, genres, styles, inspired by, description, techniques, effects, composition, mix, instrument,
                //  notes, sources, file] (lines joined with "\n"); [] when unknown
                let lib = recipes::library();
                let Some(r) = lib.get(&a.s(0)) else { ok!(Val::List(vec![])) };
                let fx: Vec<String> = r.fx_chain.iter().enumerate().map(|(i, f)| format!("{}. {}: {}", i + 1, f.effect, f.why)).collect();
                let c = &r.composition;
                let (lo, hi) = r.register();
                let nn = |k: u8| format!("{}{}", ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"][(k % 12) as usize], k as i32 / 12 - 1);
                let mut comp = vec![];
                if !r.is_master() {
                    comp.push(format!("Register: {} to {} (MIDI {lo}-{hi})", nn(lo), nn(hi)));
                }
                for (k, v) in [("Rhythm", &c.rhythm), ("Note lengths", &c.note_lengths), ("Voicing", &c.voicing_notes)] {
                    if !v.is_empty() {
                        comp.push(format!("{k}: {v}"));
                    }
                }
                if let Some(sw) = &c.switching {
                    comp.push(format!("Sound switching: {} per {} over {}", sw.order.trim(), if sw.unit == "hit" { "hit".to_string() } else { format!("{} bar(s)", sw.bars) }, sw.set.join(" / ")));
                }
                let p = &c.pitch;
                comp.push(if p.riser {
                    "Pitch: a riser, it may sweep and bend".to_string()
                } else {
                    let mut s = vec![];
                    if p.slides > 0.0 {
                        s.push(format!("slides on {:.0}% of the held steps ({} ms glide)", p.slides * 100.0, p.glide_ms));
                    }
                    if p.scoop != 0.0 {
                        s.push(format!("a {:+.1} semitone scoop into each chord", p.scoop));
                    }
                    if !p.designed.is_empty() {
                        s.push(p.designed.clone());
                    }
                    format!("Pitch: {}", if s.is_empty() { "none (the pitch guard keeps it still)".to_string() } else { s.join("; ") })
                });
                let autos: Vec<String> = r.automation.iter().map(|x| format!("Automation: {} ({} in {}){}", x.target, x.shape, x.sections.join("/"), if x.why.is_empty() { String::new() } else { format!(": {}", x.why) })).collect();
                comp.extend(autos);
                let m = &r.mix;
                let mix = format!(
                    "Level {:+.0} dB, pan {:+.2}{}{}{}{}{}",
                    m.level_db,
                    m.pan,
                    m.width.map(|w| format!(", width {:.0}%", w * 200.0)).unwrap_or_default(),
                    if m.mono_below_hz > 20.0 { format!(", mono below {:.0} Hz", m.mono_below_hz) } else { String::new() },
                    if m.sidechain.is_some() { ", ducks under the kick" } else { "" },
                    if m.reverb > 0.0 { format!(", reverb {:.0}%", m.reverb * 100.0) } else { String::new() },
                    if m.delay > 0.0 { format!(", delay {:.0}%", m.delay * 100.0) } else { String::new() }
                );
                ok!(Val::List(vec![
                    Val::Str(r.name.clone()),
                    Val::Str(r.role.clone()),
                    Val::Str(r.genres.join(", ")),
                    Val::Str(r.styles.join(", ")),
                    Val::Str(r.inspired_by.clone()),
                    Val::Str(r.description.clone()),
                    Val::Str(r.techniques.join("\n")),
                    Val::Str(fx.join("\n")),
                    Val::Str(comp.join("\n")),
                    Val::Str(mix),
                    Val::Str(r.instrument.clone()),
                    Val::Str(r.notes.clone()),
                    Val::Str(r.sources.join("\n")),
                    Val::Str(r.file.clone()),
                ]))
            }
            "daw_recipe_suggest" => {
                // (role, genre, words) -> [id, score]* best first
                let lib = recipes::library();
                let v: Vec<Val> = recipes::rank(&lib, &a.s(0), &a.s(1), &a.s(2), -1.0).into_iter().take(12).flat_map(|(s, r)| [Val::Str(r.id.clone()), Val::Num((s * 100.0).round() / 100.0)]).collect();
                ok!(Val::List(v))
            }
            "daw_recipe_problems" => {
                let lib = recipes::library();
                ok!(Val::List(lib.problems.iter().map(|p| Val::Str(p.clone())).collect()))
            }
            "daw_recipe_reload" => {
                let lib = recipes::reload();
                self.status = format!("Sound recipes: {} loaded{}", lib.recipes.len(), if lib.problems.is_empty() { String::new() } else { format!(", {} problem(s)", lib.problems.len()) });
                ok!(lib.recipes.len())
            }
            "daw_recipe_apply" => {
                // (track, id) -> a sentence: the recipe's sound on the track (instrument, chain, mix, automation, duck route)
                let lib = recipes::library();
                let Some(r) = lib.get(&a.s(1)) else { ok!(format!("No recipe '{}'", a.s(1))) };
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let s = apply_recipe(&mut en, a.u(0), r, &[], "", "", a.n(2).abs() as u64 + 1);
                drop(en);
                self.touch();
                self.status = s.clone();
                ok!(s)
            }
            "daw_recipe_apply_part" => {
                // (track, clip, id) -> a sentence: the sound, and the clip's notes rewritten the recipe's way (switching-set bars on
                // sibling tracks); clip < 0 = every MIDI clip of the track
                let lib = recipes::library();
                let Some(r) = lib.get(&a.s(2)) else { ok!(format!("No recipe '{}'", a.s(2))) };
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let t = a.u(0);
                let clips: Vec<usize> = match en.project.tracks.get(t) {
                    Some(tr) if a.n(1) < 0.0 => (0..tr.clips.len()).filter(|i| matches!(tr.clips[*i].data, ClipData::Midi(_))).collect(),
                    Some(tr) if a.u(1) < tr.clips.len() => vec![a.u(1)],
                    _ => vec![],
                };
                let s = apply_recipe(&mut en, t, r, &clips, "", "", a.n(3).abs() as u64 + 7);
                drop(en);
                self.touch();
                self.status = s.clone();
                ok!(s)
            }
            "daw_recipe_agent" => {
                // (selected track or -1, request) -> a sentence: "list recipes", "make the bass a riddim growl", "use a reese",
                // "give the lead a supersaw recipe" (the notes are rewritten when the request says play / write / pattern / part)
                let words = a.s(1);
                let low = format!(" {} ", words.to_lowercase());
                let lib = recipes::library();
                let rw = role_word(&words);
                if ["list", "show", "which", "what"].iter().any(|k| low.contains(&format!(" {k} "))) {
                    let role = if rw.is_empty() { String::new() } else { recipes::recipe_role(rw).to_string() };
                    let mut by: Vec<String> = vec![];
                    for ro in recipes::ROLES {
                        if !role.is_empty() && role != ro {
                            continue;
                        }
                        let names: Vec<String> = lib.recipes.iter().filter(|r| r.role == ro && !r.is_master()).map(|r| r.name.clone()).collect();
                        if !names.is_empty() {
                            by.push(format!("{ro}: {}", names.join(", ")));
                        }
                    }
                    ok!(format!("{} sound recipes. {}. (Edit > Selected Track > Sound Recipes... shows how each is made.)", lib.recipes.len(), by.join("; ")))
                }
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let n = en.project.tracks.len();
                // the track: one named for the role in the request, else the selected one
                let sel = if a.n(0) >= 0.0 && a.u(0) < n { Some(a.u(0)) } else { None };
                let named = if rw.is_empty() {
                    None
                } else {
                    let key = rw.split(' ').next_back().unwrap_or(rw);
                    en.project.tracks.iter().position(|tr| tr.name.to_lowercase().contains(key) && !matches!(tr.kind, TrackKind::Audio) && tr.inst_kind != "drums")
                };
                let Some(t) = named.or(sel) else { ok!("Select a track (or name it: the bass, the lead, the pad ...) for a sound recipe.".to_string()) };
                let tname = en.project.tracks[t].name.to_lowercase();
                let role = if rw.is_empty() { tname.clone() } else { rw.to_string() };
                let genre = project_genre(&en);
                let ranked = recipes::rank(&lib, &role, &genre, &words, -1.0);
                let Some((_, r)) = ranked.first() else { ok!(format!("No recipe fits a {} part.", if role.is_empty() { "that".to_string() } else { role.clone() })) };
                let write = [" play", " write", " rewrite", " pattern", " rhythm", " notes", " part", " riff"].iter().any(|k| low.contains(k));
                let clips: Vec<usize> = if write { (0..en.project.tracks[t].clips.len()).filter(|i| matches!(en.project.tracks[t].clips[*i].data, ClipData::Midi(_))).collect() } else { vec![] };
                let s = apply_recipe(&mut en, t, r, &clips, &genre, &role, words.len() as u64 + 3);
                drop(en);
                self.touch();
                ok!(format!("{s}.{}", if write { String::new() } else { " (Say 'and write the part' to have the notes rewritten the recipe's way.)".to_string() }))
            }
            _ => Err(format!("unknown builtin {name}")),
        }
    }
}
