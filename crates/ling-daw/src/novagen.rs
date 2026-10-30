//! Ling Nova in the generators (New AI Song, the Medley maker): which Nova preset plays a role in a genre, and how a part on it is
//! played and moved, all read from the presets' usage notes (`dsp/nova/presets.rs`, `nova_presets.txt`):
//! * the notes go into the preset's range, in its playing style (riddim and neuro presets chop long notes into 1/16 hits in the
//!   drops, growls and wobbles into 1/8 ones, plucks are short);
//! * the macros move section by section as the preset's `auto` line says: open or rise through a build, growl / step on a 1/8 or
//!   1/16 grid in a drop (the second half of every drop varies the pattern, the last bar doubles it as a fill), closed in a
//!   breakdown;
//! * a bass in a drop trades bars with one or two sibling presets (the same `switch` group, or a partner group: growl with riddim,
//!   neuro with reese) - the call and response of riddim and colour-bass drops.
//!
//! And it produces with the patch the way a sound designer would (`Ctx` = the song: genre, tempo, tonic, energy, seed):
//! * `variant`: the preset's real parameters moved for the song - wavetable position and warps, unison spread, filter brightness,
//!   envelopes and free LFOs on the tempo, drive / OTT by the genre's energy, a fixed comb tuned to the key, a drop movement put on
//!   a free LFO and a free macro in the matrix - never a pitch parameter ("Hungry Growl · song variant");
//! * `story_lanes` + `enforce_budget`: builds open the filter and the wavetable, drops gate in the matrix movement (another note
//!   value on the next drop), breakdowns go wide and soft, the outro closes - a few lanes moving per section, by a capped span;
//! * layers (`sub_line`, `layer_eq`): one track owns the sub - a clean Nova sub layer under the growls, the mid layers' own sub
//!   off and their lows cut - plus a top layer of the main growl in the drops and a pluck doubling a supersaw lead;
//! * `ducker`: the pump against the kick where the genre pumps, by role; `spread_voicing` and the synced pluck gate in
//!   `shape_notes_ctx`;
//! * `pick_seeded` + `palette`: different songs play different presets (the seed rolls), and the pads, keys, plucks and arps of
//!   pop, cinematic, ambient, lo-fi, synthwave and techno come from a short list that fits each palette.
use crate::dsp::nova::layout::{self as nl, layout, sel};
use crate::dsp::nova::presets::{self, Preset, Usage};
use crate::model::{AutoLane, Clip, ClipData, Note};
use crate::sounddesign::{phase, Phase, Sec};

/// genres where Nova takes the electronic bass, lead, chord and pluck parts
pub fn edm(genre: &str) -> bool {
    matches!(genre, "dubstep" | "riddim" | "future bass" | "house" | "deep house" | "bass house" | "drum and bass" | "breakbeat" | "trap" | "hyperpop" | "trance" | "garage" | "hardstyle" | "edm" | "big room" | "melodic dubstep" | "colour bass" | "neurofunk")
}

/// presets the generators never choose: Siren Screech's slow pitch LFO fails the tune check (Laser Screech plays the part)
pub const AVOID: [&str; 1] = ["Siren Screech"];
/// presets heavy in the low end of their own: their sub goes down, and their lows are cut higher when a sub layer plays under them
pub const SUB_HEAVY: [&str; 3] = ["Hungry Growl", "Wet Gargle", "Laser Wobble"];

/// Outside the club genres Nova plays only what fits the genre's palette: (genre, role) -> the presets, best first. Pads, keys,
/// plucks, arps and a synthwave lead; never a growl, a riser or a bass.
pub fn palette(genre: &str, role: &str) -> &'static [&'static str] {
    match (genre, role) {
        ("pop" | "reggaeton", "warm pad") => &["Aurora Pad", "Breathing Pad", "Shimmer Pad"],
        ("pop" | "reggaeton", "pluck") => &["Tropical Pluck", "Kalimba Pluck", "Crystal Pluck", "Water Pluck"],
        ("synthwave", "warm pad") => &["Wide Supersaw Pad", "Digital Pad", "Aurora Pad"],
        ("synthwave", "arp") => &["Saw Pluck", "Crystal Pluck", "Formant Pluck"],
        ("synthwave", "synth lead") => &["Saw Wail", "Brass Lead"],
        ("cinematic", "dark pad") => &["Dark Drone Pad", "Breathing Pad", "Glass Choir Pad"],
        ("ambient", "ambient pad") => &["Aurora Pad", "Shimmer Pad", "Glass Choir Pad", "Breathing Pad"],
        ("ambient", "warm pad") => &["Breathing Pad", "Organ Pad", "Digital Pad"],
        ("ambient", "bell") => &["Water Pluck", "Kalimba Pluck"],
        ("ambient", "texture") => &["Dark Drone Pad"],
        ("lofi" | "hip hop", "keys") => &["Lofi Keys"],
        ("lofi", "warm pad") => &["Breathing Pad", "Organ Pad"],
        ("techno" | "acid techno", "stab chord") => &["Detroit Stab"],
        ("techno", "texture") => &["Dark Drone Pad"],
        _ => &[],
    }
}

/// FNV-1a of a name (a roll per role or preset)
fn hs(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

fn title(role: &str) -> String {
    role.split(' ')
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// the preset families that play a song role, best first (the genre narrows them below)
fn families(genre: &str, role: &str) -> &'static [&'static str] {
    let dnb = matches!(genre, "drum and bass" | "breakbeat" | "neurofunk");
    let house = matches!(genre, "house" | "deep house" | "bass house" | "garage");
    match role {
        "wobble bass" => &["wobble", "growl", "fm"],
        "growl bass" if dnb => &["neuro", "reese"],
        "growl bass" if genre == "riddim" => &["riddim", "growl"],
        "growl bass" if genre == "future bass" => &["colour", "fm"],
        "growl bass" => &["growl", "talk", "riddim"],
        "sub bass" if matches!(genre, "trap" | "future bass" | "hyperpop") => &["sub"],
        "sub bass" => &["sub"],
        "rolling bass" | "pluck bass" if dnb => &["mid", "reese"],
        "rolling bass" | "pluck bass" if house => &["mid"],
        "rolling bass" | "pluck bass" => &["mid", "reese"],
        "acid bass" => &["mid"],
        "supersaw lead" => &["supersaw"],
        "laser lead" => &["screech"],
        "synth lead" if dnb => &["supersaw", "lead"],
        "synth lead" => &["lead", "pluck lead", "supersaw", "chip"],
        "brass" => &["brass"],
        "stab chord" => &["stab", "chord"],
        "chords" | "keys" if house => &["stab", "keys"],
        "chords" => &["chord"],
        "pluck" | "arp" => &["pluck"],
        "warm pad" | "ambient pad" | "dark pad" if genre == "future bass" || genre == "dubstep" || dnb => &["pad"],
        _ => &[],
    }
}

/// The Nova preset for a song role in a genre (`part` rolls among the fitting ones); None when Nova has nothing for the role.
pub fn pick(genre: &str, role: &str, part: usize) -> Option<&'static str> {
    pick_seeded(genre, role, part, 0)
}

/// `pick` rolled by the song's seed: every seed starts somewhere else among the role's candidates (seed 0 = the first), so
/// different songs showcase different presets; `part + 1`, `part + 2` ... are the next ones (the tune check's fallbacks).
pub fn pick_seeded(genre: &str, role: &str, part: usize, seed: u64) -> Option<&'static str> {
    pick_for_part(genre, role, part, seed, crate::polyphony::role_expect(role))
}

/// `pick_seeded` for a part whose polyphony is known (`poly`: Some(true) = it plays chords, Some(false) = one note at a time, None =
/// unknown; crate::polyphony measures it from the notes, `role_expect` guesses it from the role). Among the role's candidates the
/// ones whose voice fits come first: a chord part never gets a mono / legato preset while a poly one plays the role (a family
/// without one is skipped for the next), a single line prefers a mono one. When nothing fits a chord part, the first family's
/// preset is returned anyway and the track's voice is set to poly afterwards (polyphony::guard_track).
pub fn pick_for_part(genre: &str, role: &str, part: usize, seed: u64, poly: Option<bool>) -> Option<&'static str> {
    let roll = |n: usize| if seed == 0 { part % n } else { (part + (h(seed, hs(role)) * 997.0) as usize) % n };
    // the best-voiced candidates of a list (all of them when none fits better)
    let best = |c: Vec<&'static Preset>| -> (Vec<&'static Preset>, u8) {
        let top = c.iter().map(|p| crate::polyphony::score(p.is_mono(), poly)).max().unwrap_or(0);
        (c.into_iter().filter(|p| crate::polyphony::score(p.is_mono(), poly) == top).collect(), top)
    };
    if !edm(genre) {
        let c: Vec<&'static Preset> = palette(genre, role).iter().filter_map(|n| presets::find(n)).collect();
        if c.is_empty() {
            return None;
        }
        let (c, _) = best(c);
        return Some(c[roll(c.len())].name.as_str());
    }
    let all = presets::all();
    let mut fallback: Option<&'static str> = None;
    for fam in families(genre, role) {
        let ok = |p: &&Preset| p.usage.family == *fam && !AVOID.contains(&p.name.as_str());
        let mut c: Vec<&'static Preset> = all.iter().filter(|p| ok(p) && p.usage.suits(genre)).collect();
        if c.is_empty() || !c.iter().any(|p| crate::polyphony::fits(p.is_mono(), poly)) {
            let wide: Vec<&'static Preset> = all.iter().filter(ok).collect();
            if c.is_empty() || wide.iter().any(|p| crate::polyphony::fits(p.is_mono(), poly)) {
                c = wide;
            }
        }
        if c.is_empty() {
            continue;
        }
        let (c, top) = best(c);
        let name = c[roll(c.len())].name.as_str();
        if top > 0 {
            return Some(name);
        }
        // a chord part and a family of mono voices only: try the next family first
        fallback.get_or_insert(name);
    }
    fallback
}

/// "Nova presets: Growl Bass Hungry Growl, Laser Lead Glass Screech" for the plan's notes - the presets the build starts these
/// parts from (rolled the way the build rolls them: the part's index plus the seed's small roll); None when no part plays Nova
pub fn plan_note(genre: &str, roles: &[&str], seed: u64) -> Option<String> {
    let roll = (seed % 3) as usize;
    let v: Vec<String> = roles.iter().enumerate().filter_map(|(i, r)| pick_seeded(genre, r, i + roll, seed).map(|p| format!("{} {p}", title(r)))).collect();
    (!v.is_empty()).then(|| format!("Nova presets: {} (each gets its own song variant)", v.join(", ")))
}

/// the factory preset a track plays: its patch file's name (instruments/Nova/Nova_Yoi_Talker.ling -> "Yoi Talker")
pub fn preset_of_patch(path: &str) -> Option<&'static Preset> {
    if path.is_empty() {
        return None;
    }
    if let Ok(text) = std::fs::read_to_string(path) {
        let name = text.lines().find_map(|l| l.trim().strip_prefix("name").and_then(|r| r.split_once('"')).and_then(|(_, r)| r.split_once('"')).map(|(n, _)| n.to_string()));
        if let Some(p) = name.and_then(|n| presets::find(&n)) {
            return Some(p);
        }
    }
    let stem = std::path::Path::new(path).file_stem()?.to_string_lossy().replace('_', " ");
    presets::find(stem.strip_prefix("Nova ").unwrap_or(&stem))
}

/// the presets a part trades drop bars with: one of its own switch group, one of the partner group
pub fn siblings(p: &Preset, seed: u64) -> Vec<&'static str> {
    let u = &p.usage;
    // (a sub trading bars with another sub is the same sound: subs do not switch)
    if u.switch.is_empty() || u.role != "bass" || u.family == "sub" {
        return vec![];
    }
    let partner = match u.switch.as_str() {
        "growl" | "wobble" | "talk" | "fm" => "riddim",
        "riddim" => "growl",
        "colour" => "growl",
        "neuro" => "reese",
        "reese" => "neuro",
        _ => "",
    };
    let all = presets::all();
    let mut out = vec![];
    for g in [u.switch.as_str(), partner] {
        if g.is_empty() {
            continue;
        }
        let c: Vec<&Preset> = all.iter().filter(|q| q.usage.switch == g && q.name != p.name && q.usage.role == "bass").collect();
        if !c.is_empty() {
            out.push(c[(seed as usize).wrapping_mul(7) % c.len()].name.as_str());
        }
    }
    out
}

fn level(word: &str, base: f32) -> f32 {
    match word {
        "low" => 0.1,
        "mid" => 0.5,
        "high" => 0.92,
        "hold" => base,
        w => w.parse::<f32>().unwrap_or(base),
    }
}

/// a small seeded hash -> 0..1
fn h(seed: u64, k: u64) -> f32 {
    let mut x = seed ^ k.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 29;
    (x & 0xFFFF) as f32 / 65535.0
}

/// the points of one move over one section
fn move_points(mv: &str, s: &Sec, base: f32, seed: u64, pts: &mut Vec<(f64, f32)>) {
    let (a, b) = (s.start, s.end);
    let len = (b - a).max(0.25);
    let grid = |w: &str| -> f64 {
        match w.trim_start_matches(|c: char| c.is_alphabetic()) {
            "4" => 1.0,
            "8" => 0.5,
            "16" => 0.25,
            "32" => 0.125,
            "T" | "8T" => 1.0 / 3.0,
            _ => 0.5,
        }
    };
    if mv == "rise" || mv == "fall" {
        let (v0, v1) = if mv == "rise" { (0.1, 0.95) } else { (0.9, 0.1) };
        for (f, k) in [(0.0, 0.0f32), (0.5, 0.3), (0.8, 0.65), (1.0, 1.0)] {
            let t = (a + len * f).min(b - 0.01);
            pts.push((t, v0 + (v1 - v0) * k));
        }
        return;
    }
    if mv.starts_with("wob") || mv.starts_with("step") {
        let g = grid(mv);
        let half = a + (len * 0.5 / 4.0).floor() * 4.0;
        let step_vals: [f32; 8] = std::array::from_fn(|k| [0.25, 0.55, 0.85, 1.0][(h(seed, k as u64) * 3.99) as usize]);
        let step_vals2: [f32; 8] = std::array::from_fn(|k| [0.2, 0.6, 0.9, 1.0][(h(seed ^ 0xABCD, k as u64) * 3.99) as usize]);
        let mut t = a;
        let mut k = 0u64;
        while t < b - 1e-6 {
            // the last bar of the drop doubles the rate (a fill), the second half moves another way
            let fill = t >= b - 4.0 && len >= 16.0;
            let second = t >= half && len >= 16.0;
            let gg = if fill { g * 0.5 } else { g };
            let v = if mv.starts_with("wob") {
                let (hi, lo) = if second { (1.0, 0.35) } else { (0.92, 0.15) };
                if k % 2 == 0 { lo } else { hi }
            } else {
                let i = ((((t - a) / gg) as u64) % 8) as usize;
                if second { step_vals2[i] } else { step_vals[i] }
            };
            let end = (t + gg).min(b);
            if mv.starts_with("wob") {
                pts.push((t, v));
            } else {
                pts.push((t, v));
                pts.push((end - 0.01, v));
            }
            t = end;
            k += 1;
        }
        return;
    }
    let v = level(mv, base);
    pts.push((a, v));
    pts.push((b - 0.01, v));
}

/// The macro lanes of a part on preset `p` over the song's sections (`m1`..`m8`, absolute beats). Builds take the first move,
/// drops the second, breakdowns, intros and outros the third; verses sit between the breakdown and the drop.
pub fn macro_lanes(p: &Preset, secs: &[Sec], seed: u64) -> Vec<AutoLane> {
    let mut out = vec![];
    let defaults: Vec<f32> = {
        let l = crate::dsp::nova::layout::layout();
        l.macros.iter().map(|&i| p.vals[i]).collect()
    };
    for (m, moves) in &p.usage.auto {
        let base = defaults.get(*m).copied().unwrap_or(0.5);
        let mut pts: Vec<(f64, f32)> = vec![];
        for (si, s) in secs.iter().enumerate() {
            let sd = seed.wrapping_add(si as u64 * 131 + *m as u64 * 17);
            match phase(&s.tag) {
                Phase::Build => move_points(&moves[0], s, base, sd, &mut pts),
                Phase::Drop => move_points(&moves[1], s, base, sd, &mut pts),
                Phase::Mid => {
                    // a verse: the breakdown's setting, nudged towards the drop's
                    let lo = level(&moves[2], base);
                    let hi = if moves[1].starts_with("wob") || moves[1].starts_with("step") || moves[1] == "rise" { 0.6 } else { level(&moves[1], base) };
                    let v = lo + (hi - lo) * 0.4;
                    pts.push((s.start, v));
                    pts.push((s.end - 0.01, v));
                }
                _ => move_points(&moves[2], s, base, sd, &mut pts),
            }
        }
        if pts.is_empty() {
            continue;
        }
        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
        out.push(AutoLane { param: format!("m{}", m + 1), points: pts });
    }
    out
}

/// The notes of a part into the preset's range and playing style. `clips` are the part's clips; `secs` the song's sections.
pub fn shape_notes(u: &Usage, clips: &mut [Clip], secs: &[Sec]) {
    shape_notes_ctx(u, clips, secs, None)
}

/// `shape_notes` for a song: plucks gated on the tempo's grid (`pluck_gate`), the chords and pads of the club genres in a
/// supersaw spread (`spread_chords`)
pub fn shape_notes_ctx(u: &Usage, clips: &mut [Clip], secs: &[Sec], ctx: Option<&Ctx>) {
    let (lo, hi) = (u.lo as i32, u.hi as i32);
    let gate = ctx.map(pluck_gate).unwrap_or(0.45);
    let spread = ctx.map_or(false, |c| edm(&c.genre)) && matches!(u.role.as_str(), "chord" | "pad");
    let mid = (lo + hi) / 2;
    for c in clips.iter_mut() {
        let cstart = c.start;
        let ClipData::Midi(ns) = &mut c.data else { continue };
        if ns.is_empty() {
            continue;
        }
        // the range: the whole clip moved by octaves so its middle note sits in the middle of the preset's range, then single
        // notes outside it folded in
        let mut keys: Vec<i32> = ns.iter().map(|n| n.key as i32).collect();
        keys.sort();
        let med = keys[keys.len() / 2];
        let shift = ((mid - med) as f32 / 12.0).round() as i32 * 12;
        for n in ns.iter_mut() {
            let mut k = n.key as i32 + shift;
            while k < lo && k + 12 <= 127 {
                k += 12;
            }
            while k > hi && k - 12 >= lo.min(hi - 11) {
                k -= 12;
            }
            n.key = k.clamp(0, 127) as u8;
        }
        // the playing style
        let in_drop = |b: f64| secs.iter().any(|s| b >= s.start && b < s.end && phase(&s.tag) == Phase::Drop);
        match u.style.as_str() {
            "stab16" | "stab8" => {
                let g = if u.style == "stab16" { 0.25 } else { 0.5 };
                let mut out: Vec<Note> = Vec::with_capacity(ns.len() * 2);
                for n in ns.iter() {
                    if n.len > g * 2.5 && in_drop(cstart + n.start) {
                        // a held note in a drop becomes hits on the grid, a gap on every fourth (the space is the groove)
                        let mut t = 0.0;
                        let mut k = 0;
                        while t < n.len - g * 0.5 {
                            if k % 4 != 2 || g > 0.3 {
                                let mut m = n.clone();
                                m.start = n.start + t;
                                m.len = g * 0.7;
                                m.bend.clear();
                                out.push(m);
                            }
                            t += g;
                            k += 1;
                        }
                    } else {
                        let mut m = n.clone();
                        if in_drop(cstart + n.start) {
                            m.len = m.len.min(g * 1.5);
                        }
                        out.push(m);
                    }
                }
                *ns = out;
            }
            "pluck" => {
                for n in ns.iter_mut() {
                    n.len = n.len.min(gate);
                }
            }
            _ => {}
        }
        if spread {
            spread_chords(ns, u.lo, u.hi);
        }
    }
}

/// a pluck's note length on the tempo's grid: about 0.2 s rounded to one or two 1/16 notes, 90 % of it (the slow film and
/// ambient genres let a pluck ring)
pub fn pluck_gate(ctx: &Ctx) -> f64 {
    if matches!(ctx.genre.as_str(), "ambient" | "cinematic") {
        return 4.0;
    }
    let beats = 0.2 * ctx.bpm / 60.0;
    (beats / 0.25).round().clamp(1.0, 2.0) * 0.25 * 0.9
}

/// One chord as a supersaw spread: the root at the bottom, its fifth above, the other tones an octave up, the root doubled on top
/// while there are fewer than five notes - moved by octaves into `lo..=hi` (top notes past `hi` dropped, three kept at least; a
/// chord that will not fit keeps its own voicing). Fewer than three different notes are left as they are.
pub fn spread_voicing(keys: &[u8], lo: u8, hi: u8) -> Vec<u8> {
    let mut ks: Vec<i32> = keys.iter().map(|k| *k as i32).collect();
    ks.sort();
    ks.dedup();
    if ks.len() < 3 {
        return keys.to_vec();
    }
    let root = ks[0];
    let mut pcs: Vec<i32> = ks.iter().map(|k| (k - root).rem_euclid(12)).filter(|p| *p != 0).collect();
    pcs.sort();
    pcs.dedup();
    let mut v: Vec<i32> = vec![root];
    if pcs.contains(&7) {
        v.push(root + 7);
    }
    v.extend(pcs.iter().filter(|p| **p != 7).map(|p| root + 12 + p));
    // (not against a major seventh: a semitone under the top root rubs)
    if v.len() < 5 && root + 24 - v.last().copied().unwrap_or(root) > 2 {
        v.push(root + 24);
    }
    let (lo, hi) = (lo as i32, hi as i32);
    let mut shift = 0;
    while v[0] + shift < lo {
        shift += 12;
    }
    while v[0] + shift - 12 >= lo && v[v.len().min(3) - 1] + shift > hi {
        shift -= 12;
    }
    let mut out: Vec<i32> = v.iter().map(|k| k + shift).collect();
    while out.len() > 3 && *out.last().unwrap() > hi {
        out.pop();
    }
    if out.iter().any(|k| *k > hi || *k < lo) || out.len() < 3 {
        return keys.to_vec();
    }
    out.into_iter().map(|k| k.clamp(0, 127) as u8).collect()
}

/// Every chord of a clip (three or more notes starting together) in `spread_voicing`; the group's longest note sets the length
pub fn spread_chords(ns: &mut Vec<Note>, lo: u8, hi: u8) {
    ns.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.key.cmp(&b.key)));
    let mut out: Vec<Note> = Vec::with_capacity(ns.len() + 8);
    let mut i = 0;
    while i < ns.len() {
        let mut j = i + 1;
        while j < ns.len() && (ns[j].start - ns[i].start).abs() < 0.03 {
            j += 1;
        }
        let group = &ns[i..j];
        if group.len() < 3 {
            out.extend(group.iter().cloned());
            i = j;
            continue;
        }
        let keys = spread_voicing(&group.iter().map(|n| n.key).collect::<Vec<_>>(), lo, hi);
        let proto = group.iter().max_by(|a, b| a.len.total_cmp(&b.len)).cloned().unwrap_or_else(|| group[0].clone());
        for (k, key) in keys.into_iter().enumerate() {
            let mut n = group.get(k).cloned().unwrap_or_else(|| proto.clone());
            n.key = key;
            n.len = proto.len;
            n.start = group[0].start;
            out.push(n);
        }
        i = j;
    }
    *ns = out;
}

// ── the song's own patch ────────────────────────────────────────────────────────

/// The song a Nova part plays in
#[derive(Clone, Debug)]
pub struct Ctx {
    pub genre: String,
    pub bpm: f64,
    /// the key's tonic, 0..11
    pub tonic: u8,
    /// how hard the genre hits, 0..1 (the sound-design dial's default for it, `sounddesign::genre_design`)
    pub energy: f32,
    pub seed: u64,
}

impl Ctx {
    pub fn new(genre: &str, bpm: f64, tonic: u8, seed: u64) -> Ctx {
        let gd = crate::sounddesign::genre_design(genre);
        let gd = if gd <= 0.0 && edm(genre) { 0.6 } else { gd };
        Ctx { genre: genre.to_string(), bpm: if bpm.is_finite() && bpm > 30.0 { bpm } else { 120.0 }, tonic: tonic % 12, energy: (gd / 0.85).clamp(0.0, 1.0) as f32, seed }
    }
    /// seconds per beat
    pub fn beat(&self) -> f32 {
        60.0 / self.bpm.clamp(40.0, 220.0) as f32
    }
}

/// A Nova patch designed for one song: the factory preset's parameters moved, in words, and the drop movement it carries
#[derive(Clone, Debug, Default)]
pub struct Variant {
    /// "Hungry Growl · song variant"
    pub name: String,
    /// the parameters moved and their new values (only what differs from the preset)
    pub set: Vec<(&'static str, f32)>,
    pub said: Vec<String>,
    /// the drop movement: the macro that gates it in the matrix, the LFO that moves it, its note value per drop (the selector's
    /// 0..1 values, one per repeat of the drop)
    pub drop_macro: Option<usize>,
    pub drop_lfo: usize,
    pub drop_divs: Vec<f32>,
}

impl Variant {
    /// the preset's values with the variant on top
    pub fn values(&self, p: &Preset) -> Vec<f32> {
        let l = layout();
        let mut v = p.vals.clone();
        for (n, x) in &self.set {
            if let Some(&i) = l.index.get(n) {
                v[i] = *x;
            }
        }
        v
    }
}

/// low-pass filter types (lp 12, lp 24, ladder, diode): the ones a cutoff sweep opens and closes
const LOWPASS: [usize; 4] = [0, 1, 7, 8];

fn ix(name: &str) -> usize {
    layout().index[name]
}
/// an envelope knob for a time in seconds (the inverse of `env_time`: 0.5 ms .. 15 s)
fn env_v(secs: f32) -> f32 {
    ((secs.max(0.0006) / 0.0005).ln() / 30000f32.ln()).clamp(0.0, 1.0)
}
/// a selector's 0..1 value for choice `i` of `n`
fn sv(i: usize, n: usize) -> f32 {
    if n > 1 { i as f32 / (n - 1) as f32 } else { 0.0 }
}
/// is matrix destination `d` the pitch (the virtual pitch destinations, an oscillator's fine tune)?
fn pitch_dest(d: usize) -> bool {
    d < nl::D_AMP || layout().dests.get(d).map_or(false, |n| n.ends_with(" fine"))
}
/// the matrix slots in use: (source, destination, aux) indices
fn slots_in_use(v: &[f32]) -> Vec<(usize, usize, usize)> {
    let l = layout();
    l.mat.iter().filter(|m| sel(v[m.src], nl::NSRC) != 0 && (v[m.amt] - 0.5).abs() > 0.002).map(|m| (sel(v[m.src], nl::NSRC), sel(v[m.dst], l.ndst()), sel(v[m.aux], nl::NSRC))).collect()
}
/// set value `i` to `x` (clamped) and remember it, when it changes (the value comes first: it may read `v`)
fn put(i: usize, x: f32, v: &mut [f32], set: &mut Vec<(&'static str, f32)>) {
    let x = x.clamp(0.0, 1.0);
    if (v[i] - x).abs() > 1e-4 {
        v[i] = x;
        let name = layout().specs[i].name;
        set.retain(|s| s.0 != name);
        set.push((name, x));
    }
}

/// The preset designed for the song (see the module notes). Deterministic from the seed and the preset's name; pitch, octave,
/// semitone and fine tune, the mod amounts into the pitch and the glide are never touched, so a variant plays in tune.
pub fn variant(p: &Preset, ctx: &Ctx) -> Variant {
    let l = layout();
    let u = &p.usage;
    let role = u.role.as_str();
    let g = ctx.genre.as_str();
    let mut v = p.vals.clone();
    let mut set: Vec<(&'static str, f32)> = vec![];
    let mut said: Vec<String> = vec![];
    let sd = ctx.seed ^ hs(&p.name);
    let r = |k: u64| h(sd, k) * 2.0 - 1.0;
    let sub = u.family == "sub";
    // the wavetables a little further into (or back from) each table, the warps a touch more or less
    if !sub {
        let mut moved = 0.0f32;
        for (k, o) in l.osc.iter().enumerate() {
            if v[o.on] < 0.5 {
                continue;
            }
            // at least a little either way (0.02 .. 0.07)
            let x = r(1 + k as u64);
            let d = x.signum() * (0.02 + 0.05 * x.abs());
            put(o.pos, v[o.pos] + d, &mut v, &mut set);
            for w in 0..2 {
                if sel(v[o.wt[w]], nl::WARPS.len()) != nl::W_OFF && v[o.wa[w]] > 0.01 {
                    let x = (v[o.wa[w]] + 0.05 * r(10 + k as u64 * 2 + w as u64)).max(0.02);
                    put(o.wa[w], x, &mut v, &mut set);
                }
            }
            if k == 0 {
                moved = d;
            }
        }
        said.push(format!("wavetable {moved:+.2}"));
    }
    // the unison wider for the big-room genres, narrower for the soft ones (a bass keeps its own)
    let spread = match g {
        "future bass" | "trance" | "hyperpop" | "big room" | "edm" | "melodic dubstep" | "hardstyle" => 0.05,
        "lofi" | "hip hop" | "ambient" | "cinematic" => -0.05,
        "synthwave" => 0.02,
        _ => 0.0,
    };
    if matches!(role, "lead" | "pad" | "chord" | "pluck") {
        let mut any = false;
        for (k, o) in l.osc.iter().enumerate() {
            if v[o.on] < 0.5 || v[o.uni] < 0.5 / 15.0 {
                continue;
            }
            let cur = v[o.det];
            put(o.det, (cur + spread + 0.015 * r(20 + k as u64)).clamp(0.04, (cur + 0.06).min(0.5)), &mut v, &mut set);
            if matches!(g, "lofi" | "hip hop") && v[o.uni] > 4.5 / 15.0 {
                put(o.uni, v[o.uni] - 2.0 / 15.0, &mut v, &mut set);
            }
            any = true;
        }
        if any && spread != 0.0 {
            said.push(if spread > 0.0 { "unison wider".into() } else { "unison narrower".into() });
        }
    }
    // the filters: low-passes brighter or darker for the genre, a formant's throat nudged, a fixed comb tuned to the key
    let bright = match g {
        "hyperpop" | "future bass" | "trance" | "big room" | "hardstyle" | "edm" => 0.05,
        "lofi" | "hip hop" | "ambient" | "cinematic" | "deep house" | "dub" => -0.12,
        "synthwave" | "garage" => -0.04,
        _ => 0.0,
    };
    let mut lp = false;
    for (k, f) in l.filt.iter().enumerate() {
        if v[f.on] < 0.5 {
            continue;
        }
        let kind = sel(v[f.kind], nl::FTYPES.len());
        if LOWPASS.contains(&kind) {
            put(f.cut, (v[f.cut] + bright + 0.03 * r(30 + k as u64)).clamp(0.12, 1.0), &mut v, &mut set);
            lp = true;
        } else if kind == 13 {
            put(f.var, v[f.var] + 0.05 * r(32 + k as u64), &mut v, &mut set);
        } else if matches!(kind, 9 | 10 | 11) && v[f.key] < 0.05 {
            // the comb's resonance on the tonic, in the octave nearest its own
            let f0 = 20.0 * 1000f32.powf(v[f.cut]);
            let best = (1..10).map(|o| 440.0 * 2f32.powf(((ctx.tonic as i32 + 12 * o) as f32 - 69.0) / 12.0)).min_by(|a, b| (a / f0).ln().abs().total_cmp(&(b / f0).ln().abs())).unwrap_or(f0);
            put(f.cut, (best / 20.0).ln() / 1000f32.ln(), &mut v, &mut set);
            said.push(format!("comb on {}", presets::note_name(ctx.tonic + 60).trim_end_matches(char::is_numeric)));
        }
    }
    if lp && bright != 0.0 {
        said.push(if bright > 0.0 { "brighter".into() } else { "darker".into() });
    }
    // the envelopes on the tempo, half way from the preset's own times (its character stays)
    let b = ctx.beat();
    let slow = matches!(g, "ambient" | "cinematic");
    let blend = |cur: f32, secs: f32| (cur + env_v(secs)) * 0.5;
    let e1 = l.env[0];
    match role {
        "pluck" => {
            put(e1.d, blend(v[e1.d], b * if slow { 2.0 } else { 0.75 }), &mut v, &mut set);
            put(e1.r, blend(v[e1.r], b * if slow { 1.0 } else { 0.25 }), &mut v, &mut set);
            if slots_in_use(&v).iter().any(|s| s.0 == nl::S_E1 + 1) {
                put(l.env[1].d, blend(v[l.env[1].d], b * 0.5), &mut v, &mut set);
            }
            said.push("decay on the beat".into());
        }
        "pad" => {
            put(e1.a, blend(v[e1.a], b * if slow { 2.0 } else { 0.5 }), &mut v, &mut set);
            put(e1.r, blend(v[e1.r], b * if slow { 2.0 } else { 1.0 }), &mut v, &mut set);
            said.push("swell and tail on the beat".into());
        }
        "chord" if v[e1.s] < 0.5 => {
            put(e1.r, blend(v[e1.r], b * 0.5), &mut v, &mut set);
        }
        _ => {}
    }
    // free LFOs that move the tone (never the pitch) locked to the song's tempo, on the nearest note value
    let used = slots_in_use(&v);
    for (k, lf) in l.lfo.iter().enumerate() {
        let src = nl::S_L1 + k;
        let uses: Vec<&(usize, usize, usize)> = used.iter().filter(|s| s.0 == src || s.2 == src).collect();
        if uses.is_empty() || v[lf.sync] > 0.5 || uses.iter().any(|s| s.0 == src && pitch_dest(s.1)) {
            continue;
        }
        let hz = crate::dsp::nova::lfo_hz(v[lf.rate]);
        let beats = (ctx.bpm as f32 / 60.0) / hz.max(1e-3);
        let di = (0..nl::DIVS.len()).min_by(|&a, &c| (nl::DIV_BEATS[a] / beats).ln().abs().total_cmp(&(nl::DIV_BEATS[c] / beats).ln().abs())).unwrap_or(10);
        put(lf.sync, 1.0, &mut v, &mut set);
        put(lf.div, sv(di, nl::DIVS.len()), &mut v, &mut set);
        said.push(format!("LFO {} on {}", k + 1, nl::DIVS[di]));
    }
    // the grit by the genre's energy
    let e = ctx.energy;
    for (on, knob) in [("dist on", "dist drive"), ("ott on", "ott depth")] {
        if v[ix(on)] > 0.5 {
            let i = ix(knob);
            put(i, (v[i] * (0.8 + 0.4 * e)).min(0.95), &mut v, &mut set);
        }
    }
    if v[ix("dist on")] > 0.5 && v[ix("dist drive")] > 0.01 {
        said.push(format!("drive {:.2}", v[ix("dist drive")]));
    }
    // the room: more for the slow genres, a drier bass in bass music
    if v[ix("rev on")] > 0.5 {
        let d = match g {
            "ambient" | "cinematic" => 0.08,
            "lofi" => 0.04,
            _ if role == "bass" => -0.03,
            _ => 0.0,
        };
        let i = ix("rev mix");
        put(i, v[i] + d, &mut v, &mut set);
    }
    // a sub-heavy preset's own sub down (under a sub layer it goes off entirely, see `layer_eq`)
    if SUB_HEAVY.contains(&p.name.as_str()) && v[l.sub.on] > 0.5 {
        put(l.sub.level, v[l.sub.level] * 0.75, &mut v, &mut set);
        said.push("its sub down".into());
    }
    // the drop movement: a free LFO into the tone through a free matrix slot, scaled by a free macro (the macro's lane opens it
    // in the drops only); a bass has its own movement on its macros
    let mut out = Variant { name: format!("{} · song variant", p.name), ..Default::default() };
    if matches!(role, "pad" | "chord" | "pluck" | "lead") {
        let used = slots_in_use(&v);
        let free_src = |s: usize| !used.iter().any(|u| u.0 == s || u.2 == s);
        let lfo = (0..4).rev().find(|&k| free_src(nl::S_L1 + k));
        let mac = (0..8).rev().find(|&m| free_src(nl::S_M1 + m) && !u.auto.iter().any(|a| a.0 == m));
        let slot = l.mat.iter().position(|m| sel(v[m.src], nl::NSRC) == 0 || (v[m.amt] - 0.5).abs() < 0.002);
        let f1 = l.filt[0];
        let lpf = v[f1.on] > 0.5 && LOWPASS.contains(&sel(v[f1.kind], nl::FTYPES.len())) && v[f1.cut] < 0.85;
        if let (Some(lfo), Some(mac), Some(slot)) = (lfo, mac, slot) {
            let (dst, amt) = match role {
                "chord" | "pluck" if lpf => ("f1 cut", 0.22),
                "pad" if lpf => ("f1 cut", 0.15),
                "pad" => ("a pos", 0.2),
                "lead" => ("a pos", 0.12),
                _ => ("a pos", 0.18),
            };
            let d = l.dst_of[ix(dst)];
            if d != usize::MAX && (dst != "a pos" || v[l.osc[0].on] > 0.5) {
                let (shape, divs): (usize, &[&str]) = match role {
                    "pad" => (0, &["1/2", "1/4", "1/2D"]),
                    "chord" => (3, &["1/8", "1/4", "1/8T"]),
                    "pluck" => (0, &["1/4", "1/8D"]),
                    _ => (1, &["1/4", "1/2"]),
                };
                let m = l.mat[slot];
                put(m.src, sv(nl::S_L1 + lfo, nl::NSRC), &mut v, &mut set);
                put(m.dst, sv(d, l.ndst()), &mut v, &mut set);
                put(m.amt, 0.5 + amt * 0.5, &mut v, &mut set);
                put(m.bi, 0.0, &mut v, &mut set);
                put(m.crv, 0.5, &mut v, &mut set);
                put(m.aux, sv(nl::S_M1 + mac, nl::NSRC), &mut v, &mut set);
                let lf = l.lfo[lfo];
                // the note values rolled by the seed: the first drop's differs between songs, the next drop takes the next
                let k0 = (h(sd, 40) * divs.len() as f32) as usize % divs.len();
                let dv: Vec<f32> = (0..divs.len()).map(|i| sv(nl::DIVS.iter().position(|x| *x == divs[(k0 + i) % divs.len()]).unwrap_or(10), nl::DIVS.len())).collect();
                put(lf.shape, sv(shape, nl::LFO_SHAPES.len()), &mut v, &mut set);
                put(lf.sync, 1.0, &mut v, &mut set);
                put(lf.mode, 0.0, &mut v, &mut set);
                put(lf.div, dv[0], &mut v, &mut set);
                put(l.macros[mac], 0.0, &mut v, &mut set);
                said.push(format!("drop movement: LFO {} into {dst} on macro {}", lfo + 1, mac + 1));
                out.drop_macro = Some(mac);
                out.drop_lfo = lfo;
                out.drop_divs = dv;
            }
        }
    }
    out.set = set;
    out.said = said;
    out
}

// ── the story: the patch's own lanes, inside a budget ───────────────────────────

/// a lane from per-section shapes: `f(phase, repeat of that phase)` -> (fraction of the section, value) points
fn shaped(secs: &[Sec], f: &dyn Fn(Phase, usize) -> Vec<(f64, f32)>) -> Vec<(f64, f32)> {
    let mut seen = [0usize; 5];
    let mut pts = vec![];
    for s in secs {
        let ph = phase(&s.tag);
        let rep = seen[ph as usize];
        seen[ph as usize] += 1;
        let len = (s.end - s.start).max(0.25);
        for (fr, val) in f(ph, rep) {
            pts.push(((s.start + len * fr).min(s.end - 0.01), val.clamp(0.0, 1.0)));
        }
    }
    pts
}

/// a build's ramp from `a` to `b` (slow first, fast at the end)
fn ramp(a: f32, b: f32) -> Vec<(f64, f32)> {
    vec![(0.0, a), (0.5, a + (b - a) * 0.3), (0.8, a + (b - a) * 0.65), (1.0, b)]
}
fn hold(x: f32) -> Vec<(f64, f32)> {
    vec![(0.0, x), (1.0, x)]
}

/// The lanes that tell the song's story on the variant's own parameters (absolute beats): the low-pass opens through every build
/// (deeper on the next), sits full in the drops, down in a breakdown, closes over the outro; the wavetable opens into a drop; the
/// drop movement's macro opens in the drops, its LFO on another note value on the next one; a breakdown goes wide and wet. Softer
/// outside the club genres. Only lanes that move are returned; `enforce_budget` keeps them in bounds.
pub fn story_lanes(p: &Preset, var: &Variant, secs: &[Sec], ctx: &Ctx) -> Vec<AutoLane> {
    let l = layout();
    let v = var.values(p);
    let role = p.usage.role.as_str();
    if role == "fx" || secs.is_empty() {
        return vec![];
    }
    let k = if edm(&ctx.genre) { 1.0f32 } else { 0.6 };
    let bass = role == "bass";
    let mut out: Vec<AutoLane> = vec![];
    let f1 = l.filt[0];
    if v[f1.on] > 0.5 && LOWPASS.contains(&sel(v[f1.kind], nl::FTYPES.len())) && v[f1.cut] >= 0.2 {
        let c = v[f1.cut];
        let pts = shaped(secs, &|ph, rep| match ph {
            Phase::Build => ramp(c * (1.0 - (0.35 + 0.1 * rep.min(1) as f32) * k), c),
            Phase::Drop => hold(c),
            Phase::Low => hold(c * (1.0 - if bass { 0.2 } else { 0.25 } * k)),
            Phase::Mid => hold(c * (1.0 - 0.12 * k)),
            Phase::Out => ramp(c, c * (1.0 - 0.45 * k)),
        });
        out.push(AutoLane { param: "f1 cut".into(), points: pts });
    }
    // the wavetable into the drop, unless the preset's matrix already moves it
    let a = l.osc[0];
    let pos_moved = slots_in_use(&p.vals).iter().any(|s| l.dests.get(s.1) == Some(&"a pos"));
    if !bass && v[a.on] > 0.5 && !pos_moved && var.drop_macro.map_or(true, |_| !var.said.iter().any(|s| s.contains("into a pos"))) {
        let p0 = v[a.pos];
        let dir = if p0 > 0.8 { -1.0 } else { 1.0 };
        let top = (p0 + dir * 0.2 * k).clamp(0.0, 1.0);
        let pts = shaped(secs, &|ph, _| match ph {
            Phase::Build => ramp(p0, top),
            Phase::Drop => hold(p0 + (top - p0) * 0.5),
            _ => hold(p0),
        });
        out.push(AutoLane { param: "a pos".into(), points: pts });
    }
    // the drop movement: its macro open in the drops (a little less on every other one), its note value per drop
    if let Some(m) = var.drop_macro {
        let pts = shaped(secs, &|ph, rep| if ph == Phase::Drop { hold(if rep % 2 == 0 { 1.0 } else { 0.8 }) } else { hold(0.0) });
        out.push(AutoLane { param: format!("m{}", m + 1), points: pts });
        let drops = secs.iter().filter(|s| phase(&s.tag) == Phase::Drop).count();
        if drops >= 2 && var.drop_divs.len() >= 2 {
            // every section holds the note value of the drop it leads to (it changes where the macro is shut)
            let mut pts = vec![];
            let mut seen = 0usize;
            for s in secs {
                let dv = var.drop_divs[seen.min(drops - 1) % var.drop_divs.len()];
                if phase(&s.tag) == Phase::Drop {
                    seen += 1;
                }
                pts.push((s.start, dv));
                pts.push((s.end - 0.01, dv));
            }
            out.push(AutoLane { param: format!("l{} div", var.drop_lfo + 1), points: pts });
        }
    }
    // a breakdown wide and wet (a bass stays narrow and dry)
    if !bass {
        if v[ix("img on")] > 0.5 {
            let w = v[ix("img width")];
            out.push(AutoLane { param: "img width".into(), points: shaped(secs, &|ph, _| hold(if ph == Phase::Low { (w + 0.25 * k).min(0.95) } else { w })) });
        }
        if v[ix("rev on")] > 0.5 {
            let w = v[ix("rev mix")];
            out.push(AutoLane { param: "rev mix".into(), points: shaped(secs, &|ph, _| hold(if ph == Phase::Low { (w + 0.12 * k).min(0.9) } else { w })) });
        }
    }
    out.retain(|ln| {
        let (lo, hi) = ln.points.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
        hi - lo > 0.01
    });
    out
}

/// How much may move inside one kind of section: (lanes moving at once, the widest span one may move by)
pub fn budget(ph: Phase) -> (usize, f32) {
    match ph {
        Phase::Build => (3, 0.9),
        Phase::Drop => (3, 0.9),
        Phase::Low => (2, 0.45),
        Phase::Mid => (2, 0.35),
        Phase::Out => (2, 0.6),
    }
}

/// the span a lane moves by inside a section (sampled on 1/32 notes and at its points, the section's edges left out)
pub fn moving_span(lane: &AutoLane, a: f64, b: f64) -> f32 {
    let (a, b) = (a + 0.02, b - 0.02);
    if b <= a {
        return 0.0;
    }
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    let mut t = a;
    while t <= b {
        if let Some(x) = lane.value_at(t) {
            lo = lo.min(x);
            hi = hi.max(x);
        }
        t += 0.125;
    }
    for p in lane.points.iter().filter(|p| p.0 >= a && p.0 <= b) {
        lo = lo.min(p.1);
        hi = hi.max(p.1);
    }
    if let Some(x) = lane.value_at(b) {
        lo = lo.min(x);
        hi = hi.max(x);
    }
    if hi < lo { 0.0 } else { hi - lo }
}

/// Keep every section inside its budget: past the number of lanes allowed to move (the first lanes win - the preset's own macros
/// come first) a lane is held at the value it enters the section with, and a span past the cap is scaled down around that value.
/// Returns how many (lane, section) pairs were changed.
pub fn enforce_budget(lanes: &mut [AutoLane], secs: &[Sec]) -> usize {
    let mut changed = 0;
    for s in secs {
        let (n, cap) = budget(phase(&s.tag));
        let inside = |t: f64| t >= s.start && t < s.end;
        let mut moving = 0;
        for lane in lanes.iter_mut() {
            let span = moving_span(lane, s.start, s.end);
            if span <= 0.02 {
                continue;
            }
            let v0 = lane.value_at(s.start + 0.02).unwrap_or(0.0);
            if moving >= n {
                lane.points.retain(|p| !inside(p.0));
                lane.points.push((s.start, v0));
                lane.points.push((s.end - 0.01, v0));
                lane.points.sort_by(|a, b| a.0.total_cmp(&b.0));
                changed += 1;
                continue;
            }
            moving += 1;
            if span > cap {
                let f = cap / span * 0.98;
                for p in lane.points.iter_mut().filter(|p| inside(p.0)) {
                    p.1 = (v0 + (p.1 - v0) * f).clamp(0.0, 1.0);
                }
                changed += 1;
            }
        }
    }
    changed
}

// ── layers, the pump, the mix ───────────────────────────────────────────────────

/// a Nova track's job beside the part it was made for
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Layer {
    /// the part itself (a pad, a lead ... or a bass with no sub layer under it)
    Main,
    /// a bass over a sub layer (the part or one of its switching siblings)
    Mid,
    Sub,
    Top,
    /// a pluck doubling a lead
    Double,
}

/// does the preset play the sub (a sub patch)?
pub fn is_sub(p: &Preset) -> bool {
    p.usage.family == "sub"
}

/// the bass families that get a top layer in bass music (a growl's highs driven harder, in the drops)
pub fn tops(p: &Preset, genre: &str) -> bool {
    matches!(p.usage.family.as_str(), "growl" | "riddim" | "neuro" | "wobble" | "talk" | "fm") && matches!(genre, "dubstep" | "riddim" | "drum and bass" | "neurofunk" | "hardstyle" | "bass house" | "colour bass")
}

/// does a lead want a pluck doubling it in the drops (a supersaw lead of the club genres)?
pub fn doubles(p: &Preset, genre: &str) -> bool {
    p.usage.family == "supersaw" && edm(genre)
}

/// The Nova sub that plays under the mid basses of a genre: a clean sine for bass music, a rounder one for house
pub fn sub_preset(genre: &str, seed: u64) -> &'static str {
    let c: &[&str] = match genre {
        "house" | "deep house" | "bass house" | "garage" => &["Warm Sub", "Clean Sub"],
        _ => &["Clean Sub", "Warm Sub"],
    };
    c[((h(seed, 77) * c.len() as f32) as usize).min(c.len() - 1)]
}

/// The sub layer's line from a bass part's notes (absolute start, length, key, velocity): one note at a time (each cut where the
/// next starts, the lowest of notes starting together), joined where the same key comes back within a 1/16 (a chopped riddim
/// becomes one held sub), moved by octaves into `lo..=hi`
pub fn sub_line(notes: &[(f64, f64, u8, f32)], lo: u8, hi: u8) -> Vec<(f64, f64, u8, f32)> {
    let mut v = notes.to_vec();
    v.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.2.cmp(&b.2)));
    v.dedup_by(|b, a| (b.0 - a.0).abs() < 1e-3);
    let fold = |k: u8| {
        let mut k = k as i32;
        while k < lo as i32 {
            k += 12;
        }
        while k > hi as i32 && k - 12 >= lo as i32 {
            k -= 12;
        }
        k.clamp(0, 127) as u8
    };
    let mut out: Vec<(f64, f64, u8, f32)> = vec![];
    for i in 0..v.len() {
        let (s, mut len, k, vel) = v[i];
        if let Some(n) = v.get(i + 1) {
            len = len.min(n.0 - s);
        }
        let key = fold(k);
        if let Some(last) = out.last_mut() {
            if last.2 == key && s - (last.0 + last.1) <= 0.26 {
                last.1 = s + len - last.0;
                last.3 = last.3.max(vel);
                continue;
            }
        }
        if len > 0.05 {
            out.push((s, len, key, vel));
        }
    }
    out
}

/// The Param EQ band a Nova track gets for its job: (band type 0..1, frequency in Hz) - a mid bass over a sub layer cut below
/// 95 Hz (120 Hz for the sub-heavy presets) with 24 dB, the sub layer above 140 Hz, a top layer below 600 Hz; pads, chords,
/// plucks and leads leave the low end to the bass with a 12 dB cut by role (never above 70 % of the preset's lowest note, and
/// lower outside the club genres). None = no cut (a bass alone, a sub part, an effect).
pub fn layer_eq(p: &Preset, layer: Layer, genre: &str) -> Option<(f32, f32)> {
    // Param EQ band types: off, bell, lo shelf, hi shelf, lo cut, hi cut, lo cut 24, hi cut 24 (index / 8, centred)
    const LO12: f32 = 4.5 / 8.0;
    const LO24: f32 = 6.5 / 8.0;
    const HI24: f32 = 7.5 / 8.0;
    let lowest = 440.0 * 2f32.powf((p.usage.lo as f32 - 69.0) / 12.0) * 0.7;
    match layer {
        Layer::Sub => Some((HI24, 140.0)),
        Layer::Top => Some((LO24, 600.0)),
        Layer::Mid => Some((LO24, if SUB_HEAVY.contains(&p.name.as_str()) { 120.0 } else { 95.0 })),
        Layer::Main | Layer::Double => {
            let club = edm(genre);
            let hz: f32 = match p.usage.role.as_str() {
                "pad" => if club { 160.0 } else { 100.0 },
                "chord" => if club { 140.0 } else { 90.0 },
                "pluck" => if club { 200.0 } else { 110.0 },
                "lead" => if club { 170.0 } else { 110.0 },
                _ => return None,
            };
            Some((LO12, hz.min(lowest).max(40.0)))
        }
    }
}

/// a Param EQ frequency knob for `hz` (20 Hz x 2^(10 v))
pub fn eq_freq(hz: f32) -> f32 {
    ((hz / 20.0).log2() / 10.0).clamp(0.0, 1.0)
}

/// The top layer's settings on a copy of the growl's patch: its sub off, driven harder by the genre's energy, wide (the Param EQ
/// keeps only its highs)
pub fn top_settings(vals: &[f32], ctx: &Ctx) -> Vec<(&'static str, f32)> {
    let drive = (vals[ix("dist drive")].max(0.45) + 0.15 * ctx.energy).min(0.9);
    vec![("sub on", 0.0), ("dist on", 1.0), ("dist drive", drive), ("img on", 1.0), ("img width", 0.8), ("img mono", 0.0)]
}

/// The pump against the kick for a Nova track where the genre pumps: the Ducker's settings (name, 0..1) - deeper on pads and
/// chords, light on plucks and leads, a fast one on a bass, the deepest on the sub, the release on the tempo (a 1/8 note for pads
/// and chords, a 1/16 for basses). None where the genre does not pump (pop, lo-fi, film, ambient) or for an effect.
pub fn ducker(role: &str, layer: Layer, ctx: &Ctx) -> Option<Vec<(&'static str, f32)>> {
    let g: f32 = match ctx.genre.as_str() {
        "house" | "deep house" | "bass house" | "garage" | "future bass" | "big room" | "edm" | "trance" | "melodic dubstep" | "colour bass" => 1.0,
        "dubstep" | "riddim" | "hardstyle" => 0.8,
        "techno" | "acid techno" => 0.7,
        "drum and bass" | "neurofunk" | "breakbeat" | "synthwave" => 0.6,
        "trap" | "hyperpop" => 0.5,
        _ => return None,
    };
    let low = role == "bass" || layer == Layer::Sub;
    if matches!(ctx.genre.as_str(), "trap" | "hyperpop") && !low {
        return None;
    }
    let (db, rel_beats, hold_ms): (f32, f32, f32) = match (role, layer) {
        (_, Layer::Sub) => (10.0, 0.25, 30.0),
        ("bass", _) => (7.0, 0.25, 20.0),
        ("pad", _) => (12.0, 0.5, 40.0),
        ("chord", _) => (9.0, 0.5, 30.0),
        ("pluck", _) => (5.0, 0.375, 20.0),
        ("lead", _) => (4.0, 0.375, 20.0),
        _ => return None,
    };
    // the Ducker's knobs (as duck_api.rs reads them): depth = dB / 30, attack = sqrt((s - 0.5 ms) / 40 ms), hold = ms / 200,
    // release = sqrt((s - 30 ms) / 1.17 s)
    let rel = (rel_beats * ctx.beat() - hold_ms / 1000.0).max(0.04);
    Some(vec![
        ("depth", (db * g / 30.0).clamp(0.0, 1.0)),
        ("attack", ((0.004f32 - 0.0005) / 0.04).sqrt()),
        ("hold", (hold_ms / 200.0).clamp(0.0, 1.0)),
        ("release", ((rel - 0.03) / 1.17).clamp(0.0, 1.0).sqrt()),
        ("curve", 0.35),
    ])
}

/// Split the drop bars of a part between it and its siblings: bar k of a drop goes to member `order[k % order.len()]` (0 = the
/// part itself). Returns, per sibling, the notes taken out as (absolute start of the clip, clip length, notes relative to it).
pub fn split_for_switch(clips: &mut [Clip], secs: &[Sec], nsib: usize) -> Vec<Vec<(f64, f64, Vec<Note>)>> {
    let mut out: Vec<Vec<(f64, f64, Vec<Note>)>> = vec![vec![]; nsib];
    if nsib == 0 {
        return out;
    }
    // A B A C with two siblings, A B with one
    let order: Vec<usize> = if nsib >= 2 { vec![0, 1, 0, 2] } else { vec![0, 1] };
    for c in clips.iter_mut() {
        let (cs, cl) = (c.start, c.len);
        let ClipData::Midi(ns) = &mut c.data else { continue };
        let mut keep = vec![];
        let mut moved: Vec<Vec<Note>> = vec![vec![]; nsib];
        for n in ns.drain(..) {
            let at = cs + n.start;
            let who = secs.iter().find(|s| at >= s.start && at < s.end && phase(&s.tag) == Phase::Drop).map(|s| order[(((at - s.start) / 4.0).floor() as usize) % order.len()]).unwrap_or(0);
            if who == 0 {
                keep.push(n);
            } else {
                moved[who - 1].push(n);
            }
        }
        *ns = keep;
        for (k, m) in moved.into_iter().enumerate() {
            if !m.is_empty() {
                out[k].push((cs, cl, m));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs() -> Vec<Sec> {
        let tags = [("intro", 16.0), ("build", 16.0), ("drop", 32.0), ("break", 16.0), ("build", 16.0), ("drop", 32.0)];
        let mut t = 0.0;
        tags.iter()
            .map(|(g, l)| {
                let s = Sec { tag: g.to_string(), start: t, end: t + l };
                t += l;
                s
            })
            .collect()
    }

    #[test]
    fn edm_roles_get_nova_presets_and_others_do_not() {
        assert!(pick("dubstep", "growl bass", 0).is_some());
        assert!(pick("future bass", "supersaw lead", 0).is_some());
        assert!(pick("house", "stab chord", 1).is_some());
        assert!(pick("drum and bass", "growl bass", 0).map(|p| presets::find(p).unwrap().usage.family == "neuro" || presets::find(p).unwrap().usage.family == "reese").unwrap());
        assert!(pick("jazz", "synth lead", 0).is_none());
        assert!(pick("dubstep", "drums", 0).is_none());
    }

    #[test]
    fn macro_lanes_rise_in_builds_move_in_drops_and_close_in_breaks() {
        let p = presets::find("Yoi Talker").unwrap();
        let s = secs();
        let lanes = macro_lanes(p, &s, 7);
        let m1 = lanes.iter().find(|l| l.param == "m1").unwrap();
        let m2 = lanes.iter().find(|l| l.param == "m2").unwrap();
        // m1 rises across the first build
        assert!(m1.value_at(31.5).unwrap() > m1.value_at(16.0).unwrap() + 0.5);
        // m2 moves on 1/8 notes in the drop, sits low in the break
        let drop: Vec<f32> = (0..16).map(|i| m2.value_at(32.0 + i as f64 * 0.25).unwrap()).collect();
        let span = drop.iter().cloned().fold(0.0, f32::max) - drop.iter().cloned().fold(1.0, f32::min);
        assert!(span > 0.5, "{drop:?}");
        assert!(m2.value_at(70.0).unwrap() < 0.2);
        // the two halves of a drop differ
        let a: Vec<f32> = (0..16).map(|i| m2.value_at(32.0 + i as f64 * 0.25 + 0.1).unwrap()).collect();
        let b: Vec<f32> = (0..16).map(|i| m2.value_at(48.0 + i as f64 * 0.25 + 0.1).unwrap()).collect();
        assert_ne!(a, b);
    }

    #[test]
    fn notes_move_into_the_range_and_drop_bars_are_shared() {
        let p = presets::find("Hollow Riddim").unwrap();
        let s = secs();
        let mut c = Clip::midi(32.0, 32.0, "drop");
        if let ClipData::Midi(v) = &mut c.data {
            for b in 0..8 {
                v.push(Note::new(b as f64 * 4.0, 2.0, 52, 0.9));
            }
        }
        let mut clips = vec![c];
        shape_notes(&p.usage, &mut clips, &s);
        let ClipData::Midi(v) = &clips[0].data else { panic!() };
        assert!(v.iter().all(|n| n.key >= p.usage.lo && n.key <= p.usage.hi), "in range");
        assert!(v.len() > 8 * 4, "held notes chopped into 1/16 hits: {}", v.len());
        let sib = siblings(p, 3);
        assert!(!sib.is_empty());
        let moved = split_for_switch(&mut clips, &s, sib.len());
        assert!(moved.iter().all(|m| !m.is_empty()));
    }

    fn ctxs() -> Vec<Ctx> {
        vec![Ctx::new("dubstep", 140.0, 5, 3), Ctx::new("future bass", 150.0, 1, 8), Ctx::new("lofi", 82.0, 9, 21), Ctx::new("ambient", 72.0, 2, 4)]
    }

    #[test]
    fn variants_move_real_parameters_and_stay_in_tune() {
        use crate::dsp::nova::Nova;
        let sr = 44100.0;
        // one preset per family the generators play, each in two songs
        let names = ["Yoi Talker", "Hungry Growl", "Hollow Riddim", "Classic Reese", "Neuro Grinder", "Clean Sub", "Bass House Wob", "Neon Supersaw", "Glass Screech", "Future Chord Swell", "Crystal Pluck", "Aurora Pad", "Lofi Keys", "Detroit Stab"];
        for (i, name) in names.iter().enumerate() {
            let p = presets::find(name).unwrap_or_else(|| panic!("{name}"));
            let mut factory = Nova::new(sr);
            factory.set_values(&p.vals);
            let f = crate::tunecheck::check(&mut factory, sr);
            let cs = ctxs();
            for c in [&cs[i % 4], &cs[(i + 1) % 4]] {
                let v = variant(p, c);
                assert!(!v.set.is_empty() && v.name == format!("{name} · song variant"), "{name}: {v:?}");
                // never a pitch parameter
                for (k, _) in &v.set {
                    assert!(!k.ends_with(" oct") && !k.ends_with(" semi") && !k.ends_with(" fine") && !k.starts_with("bend") && !k.starts_with("porta"), "{name}: {k}");
                }
                let mut n = Nova::new(sr);
                n.set_values(&v.values(p));
                let t = crate::tunecheck::check(&mut n, sr);
                if f.in_tune() {
                    assert!(t.in_tune(), "{name} in {}: {t:?} (factory {f:?}) {:?}", c.genre, v.said);
                }
                // deterministic from the seed
                assert_eq!(variant(p, c).set, v.set);
            }
        }
    }

    #[test]
    fn the_story_stays_inside_each_sections_budget_and_varies_its_repeats() {
        let s = secs();
        let mut moved_drops = 0;
        for c in ctxs() {
            for p in presets::all().iter().filter(|p| p.usage.role != "fx") {
                let v = variant(p, &c);
                let mut lanes = macro_lanes(p, &s, c.seed);
                lanes.extend(story_lanes(p, &v, &s, &c));
                enforce_budget(&mut lanes, &s);
                for sec in &s {
                    let (n, cap) = budget(phase(&sec.tag));
                    let spans: Vec<f32> = lanes.iter().map(|l| moving_span(l, sec.start, sec.end)).collect();
                    assert!(spans.iter().filter(|x| **x > 0.02).count() <= n, "{} {}: {spans:?}", p.name, sec.tag);
                    assert!(spans.iter().all(|x| *x <= cap + 1e-3), "{} {}: {spans:?}", p.name, sec.tag);
                }
                for l in &lanes {
                    assert!(l.points.iter().all(|q| (0.0..=1.0).contains(&q.1) && q.0 >= 0.0), "{} {}", p.name, l.param);
                }
                // the drop movement opens in the drops only, on another note value the second time
                if let Some(m) = v.drop_macro {
                    let ml = lanes.iter().find(|l| l.param == format!("m{}", m + 1)).unwrap();
                    assert!(ml.value_at(40.0).unwrap() > 0.7 && ml.value_at(70.0).unwrap() < 0.05, "{}", p.name);
                    let dl = lanes.iter().find(|l| l.param == format!("l{} div", v.drop_lfo + 1)).unwrap();
                    assert_ne!(dl.value_at(40.0), dl.value_at(100.0), "{}", p.name);
                    moved_drops += 1;
                }
            }
        }
        assert!(moved_drops > 20, "pads, chords, plucks and leads get a drop movement ({moved_drops})");
        // a budget with too much in it is cut back: four lanes all rising through a build keep three, a wild one is scaled
        let b = Sec { tag: "build".into(), start: 0.0, end: 16.0 };
        let mut lanes: Vec<AutoLane> = (0..4).map(|i| AutoLane { param: format!("m{}", i + 1), points: vec![(0.0, 0.0), (15.99, 1.0)] }).collect();
        assert!(enforce_budget(&mut lanes, &[b.clone()]) >= 2);
        assert_eq!(lanes.iter().filter(|l| moving_span(l, 0.0, 16.0) > 0.02).count(), 3);
        assert!(lanes.iter().all(|l| moving_span(l, 0.0, 16.0) <= 0.9 + 1e-3));
    }

    #[test]
    fn layers_keep_one_sub_and_cut_the_mid_basses() {
        // a chopped riddim line becomes one held sub note per pitch, one note at a time, in the sub's range
        let mut line = vec![];
        for k in 0..16 {
            line.push((k as f64 * 0.25, 0.18, 40u8, 0.9f32));
        }
        line.push((4.0, 2.0, 43, 0.8));
        line.push((4.0, 2.0, 55, 0.8));
        let sub = presets::find("Clean Sub").unwrap();
        let s = sub_line(&line, sub.usage.lo, sub.usage.hi);
        assert_eq!(s.len(), 2, "{s:?}");
        assert!(s.iter().all(|n| n.2 >= sub.usage.lo && n.2 <= sub.usage.hi && (n.2 % 12 == 40 % 12 || n.2 % 12 == 43 % 12)), "{s:?}");
        for w in s.windows(2) {
            assert!(w[0].0 + w[0].1 <= w[1].0 + 1e-9, "one note at a time: {s:?}");
        }
        // the mid layer is cut below the sub, harder for a sub-heavy preset; the sub layer above it; the top above 600 Hz
        let g = presets::find("Yoi Talker").unwrap();
        let hg = presets::find("Hungry Growl").unwrap();
        let (t_mid, f_mid) = layer_eq(g, Layer::Mid, "dubstep").unwrap();
        let (t_sub, f_sub) = layer_eq(sub, Layer::Sub, "dubstep").unwrap();
        assert!(t_mid < t_sub && f_mid < f_sub && layer_eq(hg, Layer::Mid, "dubstep").unwrap().1 > f_mid);
        assert!(layer_eq(g, Layer::Top, "dubstep").unwrap().1 >= 600.0);
        assert!(layer_eq(g, Layer::Main, "dubstep").is_none(), "a bass alone keeps its lows");
        // a deep pluck is never cut above its own lowest note
        let dp = presets::find("Deep Pluck").unwrap();
        let low = 440.0 * 2f32.powf((dp.usage.lo as f32 - 69.0) / 12.0);
        assert!(layer_eq(dp, Layer::Main, "house").unwrap().1 < low);
        let top = top_settings(&g.vals, &Ctx::new("dubstep", 140.0, 5, 1));
        assert!(top.contains(&("sub on", 0.0)));
        // subs do not switch; the sub-heavy presets turn their own sub down
        assert!(siblings(sub, 4).is_empty());
        let v = variant(hg, &Ctx::new("dubstep", 140.0, 5, 1));
        assert!(v.values(hg)[layout().sub.level] < hg.vals[layout().sub.level]);
    }

    #[test]
    fn different_seeds_showcase_different_presets() {
        for (g, r) in [("dubstep", "growl bass"), ("dubstep", "wobble bass"), ("future bass", "supersaw lead"), ("future bass", "warm pad"), ("drum and bass", "growl bass"), ("house", "stab chord"), ("trance", "pluck")] {
            let picks: std::collections::BTreeSet<&str> = (1..40u64).filter_map(|s| pick_seeded(g, r, 0, s)).collect();
            // as many as the first family has presets that suit the genre (three at most asked for)
            let n = families(g, r).iter().map(|f| presets::all().iter().filter(|p| p.usage.family == *f && p.usage.suits(g)).count()).next().unwrap_or(0);
            assert!(picks.len() >= 3.min(n), "{g} {r}: {picks:?}");
            assert_eq!(pick_seeded(g, r, 0, 7), pick_seeded(g, r, 0, 7), "deterministic");
        }
        // Siren Screech fails the tune check: never picked
        for s in 0..60u64 {
            for part in 0..4 {
                assert_ne!(pick_seeded("dubstep", "laser lead", part, s), Some("Siren Screech"));
            }
        }
        assert_eq!(pick("dubstep", "growl bass", 0), pick_seeded("dubstep", "growl bass", 0, 0));
        let note = plan_note("dubstep", &["drums", "wobble bass", "growl bass", "laser lead"], 5).unwrap();
        assert!(note.contains("Growl Bass") && note.contains("Laser Lead") && !note.contains("Drums"), "{note}");
    }

    #[test]
    fn genres_beyond_edm_get_only_presets_that_fit_their_palette() {
        let mut any = 0;
        for g in crate::songgen::GENRES.iter().filter(|g| !edm(g.name)) {
            for role in g.roles {
                for s in 0..12u64 {
                    let Some(name) = pick_seeded(g.name, role, s as usize % 3, s) else { continue };
                    any += 1;
                    assert!(palette(g.name, role).contains(&name), "{} {role}: {name}", g.name);
                    let u = &presets::find(name).unwrap().usage;
                    let want: &[&str] = match crate::songgen::role_kind(role) {
                        crate::compose::Kind::Pad => &["pad"],
                        crate::compose::Kind::Arp => &["pluck"],
                        crate::compose::Kind::Chords => &["chord"],
                        crate::compose::Kind::Melody => &["lead"],
                        _ => &[],
                    };
                    assert!(want.contains(&u.role.as_str()), "{} {role}: {name} is a {}", g.name, u.role);
                }
            }
            if matches!(g.name, "jazz" | "rock" | "folk" | "orchestral" | "disco" | "afrobeat" | "ballad") {
                assert!(g.roles.iter().all(|r| pick_seeded(g.name, r, 0, 3).is_none()), "{}: no Nova", g.name);
            }
        }
        assert!(any > 20);
        for (g, r) in [("pop", "warm pad"), ("cinematic", "dark pad"), ("ambient", "ambient pad"), ("lofi", "keys"), ("synthwave", "arp")] {
            assert!(pick_seeded(g, r, 0, 1).is_some(), "{g} {r}");
        }
        assert!(pick_seeded("pop", "sub bass", 0, 1).is_none() && pick_seeded("cinematic", "brass", 0, 1).is_none());
    }

    #[test]
    fn chords_spread_plucks_gate_on_the_tempo_and_the_pump_follows_the_genre() {
        // C minor in close position -> root, fifth, then the third and the root an octave up
        assert_eq!(spread_voicing(&[60, 63, 67], 48, 84), vec![60, 67, 75, 84]);
        assert_eq!(spread_voicing(&[48, 52, 55, 59], 48, 72), vec![48, 55, 64, 71]);
        assert_eq!(spread_voicing(&[60, 64], 48, 72), vec![60, 64], "two notes stay");
        let fast = pluck_gate(&Ctx::new("future bass", 150.0, 0, 1));
        let slow = pluck_gate(&Ctx::new("lofi", 82.0, 0, 1));
        assert!(fast > slow && (fast / 0.225).fract().abs() < 1e-9 && (slow / 0.225).fract().abs() < 1e-9, "{fast} {slow}");
        let house = Ctx::new("house", 124.0, 0, 1);
        let pad = ducker("pad", Layer::Main, &house).unwrap();
        let lead = ducker("lead", Layer::Main, &house).unwrap();
        let dep = |v: &[(&str, f32)]| v.iter().find(|x| x.0 == "depth").unwrap().1;
        assert!(dep(&pad) > dep(&lead));
        assert!(ducker("pad", Layer::Main, &Ctx::new("pop", 112.0, 0, 1)).is_none());
        assert!(ducker("pad", Layer::Main, &Ctx::new("trap", 140.0, 0, 1)).is_none() && ducker("bass", Layer::Sub, &Ctx::new("trap", 140.0, 0, 1)).is_some());
        // a clip's chord re-voiced, its single notes untouched
        let u = &presets::find("Future Chord Swell").unwrap().usage;
        let mut c = Clip::midi(0.0, 8.0, "c");
        if let ClipData::Midi(v) = &mut c.data {
            for k in [60, 63, 67] {
                v.push(Note::new(0.0, 4.0, k, 0.8));
            }
            v.push(Note::new(4.0, 1.0, 70, 0.8));
        }
        let mut clips = vec![c];
        shape_notes_ctx(u, &mut clips, &secs(), Some(&Ctx::new("future bass", 150.0, 0, 1)));
        let ClipData::Midi(v) = &clips[0].data else { panic!() };
        let chord: Vec<u8> = v.iter().filter(|n| n.start == 0.0).map(|n| n.key).collect();
        assert!(chord.len() >= 3 && chord.iter().max().unwrap() - chord.iter().min().unwrap() > 12, "{chord:?}");
        assert_eq!(v.iter().filter(|n| n.start == 4.0).count(), 1);
    }
}
