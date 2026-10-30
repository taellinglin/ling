//! Ling Nova parts in generated songs and medleys (`novagen.rs`): the patch designed for the song (a variant of the preset, kept
//! only while it plays in tune), the notes into the preset's range and style, the macro and story lanes per section inside their
//! budget, the drop bars traded with sibling presets on their own tracks (named "<part> · <preset>"), the bass layers (one sub for
//! the song: a "Sub Layer · <preset>" under the growls, the mid layers' own sub off and their lows cut; a "Top Layer · <preset>" in
//! bass-music drops), a "Pluck Double · <preset>" under a supersaw lead, and the mix by role (a Param EQ low cut, the pump against
//! the kick through a Duck route where the genre pumps).
use super::{Daw, Val};
use crate::dsp::nova::presets::Preset;
use crate::model::*;
use crate::novagen::{self, Ctx, Layer, Variant};
use crate::sounddesign::{phase, Phase, Sec};

/// a Nova parameter (not an effect, the fader or the pan)?
fn nova_param(name: &str) -> bool {
    crate::dsp::nova::layout::layout().index.contains_key(name)
}

/// the words of a track name ("Bass · Nova_Clean_Sub" -> bass, nova, clean, sub)
fn words(name: &str) -> Vec<String> {
    name.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

/// a part's notes as (absolute start, length, key, velocity)
fn abs_notes(tr: &Track) -> Vec<(f64, f64, u8, f32)> {
    let mut v = vec![];
    for c in &tr.clips {
        if let ClipData::Midi(ns) = &c.data {
            v.extend(ns.iter().map(|n| (c.start + n.start, n.len, n.key, n.vel)));
        }
    }
    v
}

impl Daw {
    /// the song the Nova parts play in, from the project: the genre the generator left ("ai.genre", else the preset's first), the
    /// tempo, the key's tonic ("song_key", else read from the notes)
    fn nova_ctx(&self, seed: u64, p: &Preset) -> Ctx {
        let en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
        let meta = |k: &str| en.project.meta.iter().find(|(m, _)| m == k).map(|(_, v)| v.clone());
        let genre = meta("ai.genre").filter(|g| !g.trim().is_empty()).or_else(|| p.usage.genres.first().cloned()).unwrap_or_else(|| "edm".into());
        let tonic = match meta("song_key").and_then(|k| crate::theory::parse_key(&k)) {
            Some(k) => k.tonic,
            None => {
                let mut h = [0f64; 12];
                for tr in en.project.tracks.iter().filter(|t| t.inst_kind != "drums") {
                    for (_, len, key, _) in abs_notes(tr) {
                        h[(key % 12) as usize] += len.max(0.1);
                    }
                }
                crate::theory::detect_key_hist(&h).0.tonic
            }
        };
        Ctx::new(&genre, en.project.bpm, tonic, seed)
    }

    /// Put a variant on track `t`'s Nova - kept only when it still plays in tune (a sound too unpitched to judge passes). Returns
    /// whether it went on.
    fn nova_apply_variant(&mut self, t: usize, p: &Preset, var: &Variant) -> bool {
        let sr = self.out.sr;
        if p.usage.role != "fx" {
            let mut n = crate::dsp::nova::Nova::new(sr);
            n.set_values(&var.values(p));
            if !crate::tunecheck::check(&mut n, sr).in_tune() {
                return false;
            }
        }
        let mut en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(inst) = en.project.tracks.get_mut(t).and_then(|tr| tr.inst.as_mut()) {
            for (n, v) in &var.set {
                inst.set_param(n, *v);
            }
        }
        true
    }

    /// The lanes of a Nova track: the preset's macro moves, the story on its variant (when it went on), then whatever else drew on
    /// its parameters (the articulation) - all inside the sections' budget. The generic filter lane gives way to the story's (on a
    /// formant or comb filter the cutoff is the tone, not a sweep).
    fn nova_lanes(&mut self, t: usize, p: &Preset, var: Option<&Variant>, secs: &[Sec], ctx: &Ctx, seed: u64) {
        let mut en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
        let Some(tr) = en.project.tracks.get_mut(t) else { return };
        let mut lanes = novagen::macro_lanes(p, secs, seed);
        if let Some(v) = var {
            lanes.extend(novagen::story_lanes(p, v, secs, ctx));
        }
        let mine: Vec<String> = lanes.iter().map(|l| l.param.clone()).collect();
        tr.auto.retain(|l| !mine.contains(&l.param) && !matches!(l.param.as_str(), "f1 cut" | "f2 cut" | "flt cut"));
        let (others, rest): (Vec<AutoLane>, Vec<AutoLane>) = tr.auto.drain(..).partition(|l| nova_param(&l.param));
        tr.auto = rest;
        lanes.extend(others);
        novagen::enforce_budget(&mut lanes, secs);
        tr.auto.extend(lanes);
    }

    /// a new Nova track on factory preset `name`, titled (None when its patch is missing)
    fn nova_track(&mut self, name: &str, title: &str) -> Option<usize> {
        let path = super::song_api::find_patch("nova", name)?;
        let nt = self.call_num("daw_track_add", &[Val::Str("nova".into()), Val::Str(path)]);
        if nt < 0.0 {
            return None;
        }
        let _ = self.dispatch("daw_track_set_name", &super::A(&[Val::Num(nt), Val::Str(title.to_string())]));
        Some(nt as usize)
    }

    /// Dress the Nova part on track `t` (nothing when it is not a Nova track or its preset is unknown). `switch`: trade the drop
    /// bars with sibling presets. Returns the tracks made (siblings, layers, a double) and a line for the summary ("" = nothing).
    pub(super) fn nova_dress(&mut self, t: usize, secs: &[Sec], seed: u64, switch: bool) -> (Vec<usize>, String) {
        let eng = self.out.engine.clone();
        let sr = self.out.sr;
        let (preset, name, vol) = {
            let en = eng.lock().unwrap_or_else(|p| p.into_inner());
            let Some(tr) = en.project.tracks.get(t) else { return (vec![], String::new()) };
            if tr.inst_kind != "nova" {
                return (vec![], String::new());
            }
            let Some(p) = novagen::preset_of_patch(&tr.patch) else { return (vec![], String::new()) };
            (p, tr.name.clone(), tr.volume)
        };
        let ctx = self.nova_ctx(seed, preset);
        let u = &preset.usage;
        // the song's own patch
        let var = novagen::variant(preset, &ctx);
        let kept = self.nova_apply_variant(t, preset, &var);
        let sibs = if switch && secs.iter().any(|s| phase(&s.tag) == Phase::Drop) { novagen::siblings(preset, seed) } else { vec![] };
        // a chord part on a mono / legato voice drops notes: its voice goes poly (polyphony.rs), siblings follow below
        let mut voiced: Vec<String> = vec![];
        let (moved, line_notes) = {
            let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
            let Some(tr) = en.project.tracks.get_mut(t) else { return (vec![], String::new()) };
            novagen::shape_notes_ctx(u, &mut tr.clips, secs, Some(&ctx));
            let vg = crate::polyphony::guard_track(tr);
            if !vg.is_empty() {
                voiced.push(vg);
            }
            // the whole line before the switching splits it (the sub and top layers play under every member)
            let notes = abs_notes(tr);
            (novagen::split_for_switch(&mut tr.clips, secs, sibs.len()), notes)
        };
        self.nova_lanes(t, preset, kept.then_some(&var), secs, &ctx, seed);
        let clip_spans: Vec<(f64, f64)> = {
            let en = eng.lock().unwrap_or_else(|p| p.into_inner());
            en.project.tracks.get(t).map(|tr| tr.clips.iter().map(|c| (c.start, c.len)).collect()).unwrap_or_default()
        };
        let mut made = vec![];
        let mut sib_tracks: Vec<(usize, &'static Preset)> = vec![];
        for (k, sname) in sibs.iter().enumerate() {
            let Some(path) = super::song_api::find_patch("nova", sname) else { continue };
            let nt = self.call_num("daw_track_add", &[Val::Str("nova".into()), Val::Str(path)]);
            if nt < 0.0 {
                continue;
            }
            let nt = nt as usize;
            let _ = self.dispatch("daw_track_set_name", &super::A(&[Val::Num(nt as f64), Val::Str(format!("{name} · {sname}"))]));
            {
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                // the sibling sounds through the same chain as the part (the same effects, settings and effect lanes, the duck)
                let chain: Vec<(String, Vec<f32>, bool, f32)> = en.project.tracks.get(t).map(|tr| tr.effects.iter().map(|e| (e.name.clone(), e.params.iter().map(|p| p.1).collect(), e.bypass, e.wet)).collect()).unwrap_or_default();
                let lanes: Vec<AutoLane> = en.project.tracks.get(t).map(|tr| tr.auto.iter().filter(|l| l.param.starts_with("fx") || l.param == "vol").map(|l| AutoLane { param: l.param.clone(), points: l.points.clone() }).collect()).unwrap_or_default();
                let Some(tr) = en.project.tracks.get_mut(nt) else { continue };
                tr.volume = vol;
                for (fname, vals, bypass, wet) in chain {
                    if let Some(mut slot) = EffectSlot::new(&fname, sr) {
                        for (i, v) in vals.into_iter().enumerate() {
                            slot.set(i, v);
                        }
                        slot.bypass = bypass;
                        slot.wet = wet;
                        tr.effects.push(slot);
                    }
                }
                tr.auto.extend(lanes);
                for (cs, cl, notes) in &moved[k] {
                    let mut c = Clip::midi(*cs, *cl, "switch");
                    if let ClipData::Midi(v) = &mut c.data {
                        v.extend(notes.iter().cloned());
                    }
                    tr.clips.push(c);
                }
            }
            made.push(nt);
            if let Some(sp) = crate::dsp::nova::presets::find(sname) {
                // each sibling is designed for the song too (its own roll)
                let sctx = Ctx { seed: seed.wrapping_add(k as u64 + 1), ..ctx.clone() };
                let sv = novagen::variant(sp, &sctx);
                let skept = self.nova_apply_variant(nt, sp, &sv);
                self.nova_lanes(nt, sp, skept.then_some(&sv), secs, &sctx, seed.wrapping_add(k as u64 + 1));
                sib_tracks.push((nt, sp));
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                if let Some(tr) = en.project.tracks.get_mut(nt) {
                    let vg = crate::polyphony::guard_track(tr);
                    if !vg.is_empty() {
                        voiced.push(vg);
                    }
                }
            }
        }
        // the layers and the mix: (track, its preset, its job)
        let mut mix: Vec<(usize, &'static Preset, Layer)> = vec![];
        let mut said: Vec<String> = voiced;
        let mut main_layer = Layer::Main;
        if u.role == "bass" && !novagen::is_sub(preset) && novagen::edm(&ctx.genre) {
            let sib_ts: Vec<usize> = sib_tracks.iter().map(|s| s.0).collect();
            let (ls, l_said) = self.nova_bass_layers(t, preset, &sib_ts, &line_notes, &clip_spans, secs, &ctx);
            main_layer = Layer::Mid;
            for (lt, lp, job) in ls {
                made.push(lt);
                mix.push((lt, lp, job));
            }
            said.extend(l_said);
        }
        mix.push((t, preset, main_layer));
        for (st, sp) in &sib_tracks {
            mix.push((*st, *sp, main_layer));
        }
        if novagen::doubles(preset, &ctx.genre) {
            if let Some((dt, dp, d_said)) = self.nova_pluck_double(t, vol, secs, &ctx) {
                made.push(dt);
                mix.push((dt, dp, Layer::Double));
                said.push(d_said);
            }
        }
        said.extend(self.nova_mix(&mix, &ctx));
        let moves: Vec<String> = u.auto.iter().map(|(m, mv)| format!("{} {}", u.macro_label(*m), mv.join("/"))).collect();
        let design = if kept { format!("{} ({})", var.name, var.said.join(", ")) } else { format!("{} (factory: the variant left the tune)", preset.name) };
        let mut line = format!("{name}: Nova {design} in {}-{}, macros {}", crate::dsp::nova::presets::note_name(u.lo), crate::dsp::nova::presets::note_name(u.hi), moves.join(", "));
        if !sibs.is_empty() {
            line.push_str(&format!("; drop bars traded with {}", sibs.join(" and ")));
        }
        if !said.is_empty() {
            line.push_str(&format!("; {}", said.join("; ")));
        }
        (made, line)
    }

    /// The low end of a bass part (see the module notes): the song's sub owner (a sub part, or the sub layer an earlier bass made -
    /// this bass then fills its gaps), else a new sub layer playing this line; the part and its siblings lose their own sub; a top
    /// layer of the growl families in bass-music drops. Returns the layer tracks (with their presets and jobs) and what was done.
    #[allow(clippy::too_many_arguments)]
    fn nova_bass_layers(&mut self, t: usize, p: &'static Preset, sibs: &[usize], line: &[(f64, f64, u8, f32)], spans: &[(f64, f64)], secs: &[Sec], ctx: &Ctx) -> (Vec<(usize, &'static Preset, Layer)>, Vec<String>) {
        let eng = self.out.engine.clone();
        let mut out = vec![];
        let mut said = vec![];
        // who owns the sub: a track with "sub" in its name or a Nova sub patch (not this part or its siblings)
        let owner: Option<(usize, String)> = {
            let en = eng.lock().unwrap_or_else(|p| p.into_inner());
            en.project
                .tracks
                .iter()
                .enumerate()
                .find(|(i, tr)| *i != t && !sibs.contains(i) && !matches!(tr.inst_kind.as_str(), "drums" | "audio" | "voice") && (words(&tr.name).iter().any(|w| w == "sub") || (tr.inst_kind == "nova" && novagen::preset_of_patch(&tr.patch).map_or(false, novagen::is_sub))))
                .map(|(i, tr)| (i, tr.name.clone()))
        };
        let (sub_t, ours) = match owner {
            Some((i, n)) => (Some(i), n.starts_with("Sub Layer")),
            None => {
                let sname = novagen::sub_preset(&ctx.genre, ctx.seed);
                match self.nova_track(sname, &format!("Sub Layer · {sname}")) {
                    Some(nt) => {
                        if let Some(sp) = crate::dsp::nova::presets::find(sname) {
                            out.push((nt, sp, Layer::Sub));
                        }
                        (Some(nt), true)
                    }
                    None => (None, false),
                }
            }
        };
        let Some(st) = sub_t else { return (out, said) };
        if ours {
            // this line in the sub's range, where the sub does not play yet
            let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
            let (lo, hi) = en.project.tracks.get(st).and_then(|tr| novagen::preset_of_patch(&tr.patch)).map(|sp| (sp.usage.lo, sp.usage.hi)).unwrap_or((24, 36));
            if let Some(tr) = en.project.tracks.get_mut(st) {
                let have = abs_notes(tr);
                let fresh: Vec<(f64, f64, u8, f32)> = novagen::sub_line(line, lo, hi)
                    .into_iter()
                    .filter(|n| {
                        let over: f64 = have.iter().map(|h| ((n.0 + n.1).min(h.0 + h.1) - n.0.max(h.0)).max(0.0)).sum();
                        over < 0.1 * n.1
                    })
                    .collect();
                for (cs, cl) in spans {
                    let mine: Vec<Note> = fresh.iter().filter(|n| n.0 >= *cs - 1e-6 && n.0 < cs + cl).map(|n| Note::new(n.0 - cs, n.1.min(cs + cl - n.0), n.2, n.3)).collect();
                    if mine.is_empty() {
                        continue;
                    }
                    let mut c = Clip::midi(*cs, *cl, "sub");
                    if let ClipData::Midi(v) = &mut c.data {
                        v.extend(mine);
                    }
                    tr.clips.push(c);
                }
            }
        }
        // the mid layers: their own sub off (the owner has the low end)
        {
            let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
            for tt in std::iter::once(t).chain(sibs.iter().copied()) {
                if let Some(inst) = en.project.tracks.get_mut(tt).and_then(|tr| tr.inst.as_mut()) {
                    inst.set_param("sub on", 0.0);
                }
            }
            let sname = en.project.tracks.get(st).map(|tr| tr.name.clone()).unwrap_or_default();
            let cut = novagen::layer_eq(p, Layer::Mid, &ctx.genre).map(|x| x.1).unwrap_or(95.0);
            said.push(format!("the sub on {sname}, its own sub off and cut below {cut:.0} Hz"));
        }
        // a top layer: the growl's highs driven harder and wide, in the drops only (one per song)
        let has_top = {
            let en = eng.lock().unwrap_or_else(|p| p.into_inner());
            en.project.tracks.iter().any(|tr| tr.name.starts_with("Top Layer"))
        };
        if novagen::tops(p, &ctx.genre) && !has_top {
            if let Some(nt) = self.nova_track(&p.name, &format!("Top Layer · {}", p.name)) {
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let (state, vol) = en.project.tracks.get(t).map(|tr| (tr.inst.as_ref().map(|i| i.state()).unwrap_or_default(), tr.volume)).unwrap_or_default();
                let in_drop = |b: f64| secs.iter().any(|s| b >= s.start && b < s.end && phase(&s.tag) == Phase::Drop);
                if let Some(tr) = en.project.tracks.get_mut(nt) {
                    if let Some(inst) = tr.inst.as_mut() {
                        inst.load_state(&state);
                        let vals: Vec<f32> = inst.params().into_iter().map(|(_, v)| v).collect();
                        for (n, v) in novagen::top_settings(&vals, ctx) {
                            inst.set_param(n, v);
                        }
                    }
                    tr.volume = vol * 0.55;
                    for (cs, cl) in spans {
                        let mine: Vec<Note> = line.iter().filter(|n| n.0 >= *cs - 1e-6 && n.0 < cs + cl && in_drop(n.0)).map(|n| Note::new(n.0 - cs, n.1, n.2, n.3)).collect();
                        if mine.is_empty() {
                            continue;
                        }
                        let mut c = Clip::midi(*cs, *cl, "top");
                        if let ClipData::Midi(v) = &mut c.data {
                            v.extend(mine);
                        }
                        tr.clips.push(c);
                    }
                }
                drop(en);
                self.nova_lanes(nt, p, None, secs, ctx, ctx.seed.wrapping_add(9));
                out.push((nt, p, Layer::Top));
                said.push("a top layer in the drops (driven, wide, above 600 Hz)".into());
            }
        }
        (out, said)
    }

    /// A pluck doubling a supersaw lead in the drops (unless the song has a pluck part already): the lead's drop notes on the
    /// tempo's pluck gate, half its level
    fn nova_pluck_double(&mut self, t: usize, vol: f32, secs: &[Sec], ctx: &Ctx) -> Option<(usize, &'static Preset, String)> {
        let eng = self.out.engine.clone();
        let has_pluck = {
            let en = eng.lock().unwrap_or_else(|p| p.into_inner());
            en.project.tracks.iter().any(|tr| words(&tr.name).iter().any(|w| w == "pluck"))
        };
        if has_pluck {
            return None;
        }
        let pname = novagen::pick_seeded(&ctx.genre, "pluck", 0, ctx.seed).unwrap_or("Crystal Pluck");
        let pp = crate::dsp::nova::presets::find(pname)?;
        let in_drop = |b: f64| secs.iter().any(|s| b >= s.start && b < s.end && phase(&s.tag) == Phase::Drop);
        let clips: Vec<(f64, f64, Vec<Note>)> = {
            let en = eng.lock().unwrap_or_else(|p| p.into_inner());
            en.project
                .tracks
                .get(t)
                .map(|tr| {
                    tr.clips
                        .iter()
                        .filter_map(|c| match &c.data {
                            ClipData::Midi(ns) => Some((c.start, c.len, ns.iter().filter(|n| in_drop(c.start + n.start)).cloned().collect::<Vec<Note>>())),
                            _ => None,
                        })
                        .filter(|c| !c.2.is_empty())
                        .collect()
                })
                .unwrap_or_default()
        };
        if clips.is_empty() {
            return None;
        }
        let nt = self.nova_track(pname, &format!("Pluck Double · {pname}"))?;
        let var = novagen::variant(pp, ctx);
        let kept = self.nova_apply_variant(nt, pp, &var);
        {
            let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
            let tr = en.project.tracks.get_mut(nt)?;
            tr.volume = vol * 0.5;
            for (cs, cl, ns) in clips {
                let mut c = Clip::midi(cs, cl, "double");
                if let ClipData::Midi(v) = &mut c.data {
                    v.extend(ns.into_iter().map(|mut n| {
                        n.vel *= 0.85;
                        n.bend.clear();
                        n
                    }));
                }
                tr.clips.push(c);
            }
            novagen::shape_notes_ctx(&pp.usage, &mut tr.clips, secs, Some(ctx));
        }
        self.nova_lanes(nt, pp, kept.then_some(&var), secs, ctx, ctx.seed.wrapping_add(13));
        Some((nt, pp, format!("doubled by {pname} in the drops")))
    }

    /// The mix of the Nova tracks by their job: the Param EQ cut (`novagen::layer_eq`), and where the genre
    /// pumps a Duck route from the kick with the role's Ducker (`novagen::ducker`); a pad's or chord's fader pump gives way to it
    fn nova_mix(&mut self, tracks: &[(usize, &'static Preset, Layer)], ctx: &Ctx) -> Vec<String> {
        let eng = self.out.engine.clone();
        let sr = self.out.sr;
        let kick: Option<usize> = {
            let en = eng.lock().unwrap_or_else(|p| p.into_inner());
            en.project.tracks.iter().position(|tr| tr.inst_kind == "drums" && abs_notes(tr).iter().any(|n| n.2 == 35 || n.2 == 36))
        };
        let mut ducked = 0;
        let mut depth_db = 0.0f32;
        for &(t, p, layer) in tracks {
            if let Some((ty, hz)) = novagen::layer_eq(p, layer, &ctx.genre) {
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                if let Some(tr) = en.project.tracks.get_mut(t) {
                    let idx = match tr.effects.iter().position(|e| e.name == "Param EQ") {
                        Some(i) => Some(i),
                        // last in the chain: it also takes the lows the drive and the compression made
                        None => EffectSlot::new("Param EQ", sr).map(|s| {
                            tr.effects.push(s);
                            tr.effects.len() - 1
                        }),
                    };
                    if let Some(i) = idx {
                        let slot = &mut tr.effects[i];
                        for (k, v) in [("b1 type", ty), ("b1 freq", novagen::eq_freq(hz))] {
                            if let Some(pi) = slot.params.iter().position(|q| q.0 == k) {
                                slot.set(pi, v);
                            }
                        }
                    }
                }
            }
            let (Some(src), Some(set)) = (kick, novagen::ducker(&p.usage.role, layer, ctx)) else { continue };
            let fi = match self.dispatch("daw_duck_add", &super::A(&[Val::Num(src as f64), Val::Num(t as f64), Val::Str("35,36".into()), Val::Num(1.0)])) {
                Ok(Val::List(v)) if v.len() >= 2 => match v[1] {
                    Val::Num(x) => x as usize,
                    _ => continue,
                },
                _ => continue,
            };
            let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
            let Some(tr) = en.project.tracks.get_mut(t) else { continue };
            if let Some(slot) = tr.effects.get_mut(fi) {
                for (k, v) in &set {
                    if let Some(pi) = slot.params.iter().position(|q| q.0 == *k) {
                        slot.set(pi, *v);
                    }
                }
            }
            // the generator's fader pump on club pads and chords: the Ducker does it now
            if matches!(p.usage.role.as_str(), "pad" | "chord") {
                tr.auto.retain(|l| !(l.param == "vol" && l.points.len() > 16));
            }
            ducked += 1;
            depth_db = depth_db.max(set.iter().find(|s| s.0 == "depth").map(|s| s.1 * 30.0).unwrap_or(0.0));
        }
        if ducked > 0 {
            vec![format!("{ducked} Nova track(s) duck under the kick (up to {depth_db:.0} dB)")]
        } else {
            vec![]
        }
    }
}
