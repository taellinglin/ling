//! Builtins for sidechain ducking driven by notes (`daw_duck_*`): a kick (or any chosen notes) of one track dips the volume of others.
//!
//! A *Duck route* (`RouteKind::Duck`, from the source track to the target track, with the note numbers that trigger it) feeds the
//! target's **Ducker** effect, which these builtins add when it is missing. Unlike an audio sidechain, the trigger is read from the notes,
//! so only the kick of a whole drum kit ducks, it is exact to the sample, and it works while the source track is muted.
use super::{Daw, A, Val};
use crate::dsp::fx::{Ducker, Effect};
use crate::model::*;

/// "36,35,40-44" -> note numbers
pub fn parse_keys(s: &str) -> Vec<u8> {
    let mut out: Vec<u8> = vec![];
    for part in s.split(|c: char| c == ',' || c == ' ' || c == ';').filter(|p| !p.is_empty()) {
        let nums: Vec<u8> = part.split('-').filter_map(|x| x.trim().parse::<u8>().ok()).collect();
        match nums.as_slice() {
            [a] if *a < 128 => out.push(*a),
            [a, b] if *a < 128 && *b < 128 => out.extend(*a.min(b)..=*a.max(b)),
            _ => {}
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

pub fn keys_string(keys: &[u8]) -> String {
    keys.iter().map(|k| k.to_string()).collect::<Vec<_>>().join(",")
}

/// what to key from: (label, notes) for a source track
fn suggestions(t: &Track) -> Vec<(&'static str, &'static str)> {
    if t.inst_kind == "drums" {
        vec![("Kick", "35,36"), ("Snare", "37,38,40"), ("Clap", "39"), ("Closed hat", "42,44"), ("Open hat", "46"), ("Toms", "41,43,45,47,48,50"), ("Every hit", "")]
    } else {
        vec![("Every note", ""), ("Low notes (below C3)", "0-47"), ("High notes (C4 and up)", "60-127")]
    }
}

fn note_count(t: &Track, keys: &[u8]) -> usize {
    t.clips
        .iter()
        .map(|c| match &c.data {
            ClipData::Midi(n) => {
                let per = n.iter().filter(|x| keys.is_empty() || keys.contains(&x.key)).count();
                let reps = if c.loop_len >= 0.25 && c.loop_len < c.len { (c.len / c.loop_len).ceil() as usize } else { 1 };
                per * reps.min(512)
            }
            _ => 0,
        })
        .sum()
}

// real units <-> the Ducker's normalised parameters
fn depth_n(db: f64) -> f32 {
    (db / 30.0).clamp(0.0, 1.0) as f32
}
fn attack_n(ms: f64) -> f32 {
    ((ms / 1000.0 - 0.0005) / 0.04).clamp(0.0, 1.0).sqrt() as f32
}
fn hold_n(ms: f64) -> f32 {
    (ms / 200.0).clamp(0.0, 1.0) as f32
}
fn release_n(ms: f64) -> f32 {
    ((ms / 1000.0 - 0.03) / 1.17).clamp(0.0, 1.0).sqrt() as f32
}
fn depth_r(v: f32) -> f64 {
    v as f64 * 30.0
}
fn attack_r(v: f32) -> f64 {
    (0.0005 + (v as f64).powi(2) * 0.04) * 1000.0
}
fn hold_r(v: f32) -> f64 {
    v as f64 * 200.0
}
fn release_r(v: f32) -> f64 {
    (0.03 + (v as f64).powi(2) * 1.17) * 1000.0
}

fn ducker_index(t: &Track) -> Option<usize> {
    t.effects.iter().position(|e| e.name == "Ducker")
}

impl Daw {
    pub(super) fn duck_dispatch(&mut self, name: &str, a: &A) -> Result<Val, String> {
        let eng = self.out.engine.clone();
        macro_rules! ok {
            ($v:expr) => {
                return Ok(Val::from($v))
            };
        }
        match name {
            "daw_duck_keys" => {
                // (source track) -> [label, notes, how many of them play]*
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let Some(t) = en.project.tracks.get(a.u(0)) else { return ok!(Val::List(vec![])) };
                let mut v = vec![];
                for (label, keys) in suggestions(t) {
                    v.extend([Val::Str(label.to_string()), Val::Str(keys.to_string()), Val::Num(note_count(t, &parse_keys(keys)) as f64)]);
                }
                ok!(Val::List(v))
            }
            "daw_duck_add" => {
                // (source, target, notes "36,35" or "" for every note, amount 0..2) -> [route, effect index] or [] when it cannot be made
                let (from, to) = (a.u(0), a.u(1));
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let n = en.project.tracks.len();
                if from >= n || to >= n || from == to || en.project.would_cycle(from, to) {
                    return ok!(Val::List(vec![]));
                }
                let keys = parse_keys(&a.s(2));
                let amount = if a.0.len() > 3 { a.n(3).clamp(0.0, 2.0) as f32 } else { 1.0 };
                let ri = match en.project.routes.iter().position(|r| r.kind == RouteKind::Duck && r.from == from && r.to == to) {
                    Some(i) => {
                        en.project.routes[i].keys = keys;
                        en.project.routes[i].amount = amount;
                        i
                    }
                    None => {
                        en.project.routes.push(Route { kind: RouteKind::Duck, from, to, amount, keys });
                        en.project.routes.len() - 1
                    }
                };
                let sr = en.sr;
                let fi = match ducker_index(&en.project.tracks[to]) {
                    Some(i) => i,
                    None => match EffectSlot::new("Ducker", sr) {
                        Some(slot) => {
                            en.project.tracks[to].effects.push(slot);
                            en.project.tracks[to].effects.len() - 1
                        }
                        None => return ok!(Val::List(vec![])),
                    },
                };
                drop(en);
                self.touch();
                ok!(Val::List(vec![Val::Num(ri as f64), Val::Num(fi as f64)]))
            }
            "daw_duck_list" => {
                // -> [route, from, to, notes, effect index on the target (-1 none)]* for every Duck route
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let mut v = vec![];
                for (i, r) in en.project.routes.iter().enumerate().filter(|(_, r)| r.kind == RouteKind::Duck) {
                    let fi = en.project.tracks.get(r.to).and_then(ducker_index).map(|x| x as f64).unwrap_or(-1.0);
                    v.extend([Val::Num(i as f64), Val::Num(r.from as f64), Val::Num(r.to as f64), Val::Str(keys_string(&r.keys)), Val::Num(fi)]);
                }
                ok!(Val::List(v))
            }
            "daw_duck_remove" => {
                // (route, also remove the Ducker when nothing else feeds it 0/1)
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let i = a.u(0);
                if en.project.routes.get(i).map(|r| r.kind == RouteKind::Duck) != Some(true) {
                    return ok!(false);
                }
                let to = en.project.routes[i].to;
                en.project.routes.remove(i);
                if a.n(1) > 0.5 && !en.project.routes.iter().any(|r| r.to == to && r.kind == RouteKind::Duck) {
                    if let Some(t) = en.project.tracks.get_mut(to) {
                        t.effects.retain(|e| e.name != "Ducker");
                    }
                }
                drop(en);
                self.touch();
                ok!(true)
            }
            "daw_duck_set" => {
                // (target track, depth dB 0..30, attack ms, hold ms, release ms, curve 0..1) on the target's Ducker
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let Some(t) = en.project.tracks.get_mut(a.u(0)) else { return ok!(false) };
                let Some(fi) = ducker_index(t) else { return ok!(false) };
                let slot = &mut t.effects[fi];
                for (k, v) in [(0usize, depth_n(a.n(1))), (1, attack_n(a.n(2))), (2, hold_n(a.n(3))), (3, release_n(a.n(4))), (4, a.n(5).clamp(0.0, 1.0) as f32)] {
                    slot.set(k, v);
                }
                drop(en);
                self.touch();
                ok!(true)
            }
            "daw_duck_get" => {
                // (target track) -> [depth dB, attack ms, hold ms, release ms, curve 0..1], [] when it has no Ducker
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let Some(t) = en.project.tracks.get(a.u(0)) else { return ok!(Val::List(vec![])) };
                let Some(fi) = ducker_index(t) else { return ok!(Val::List(vec![])) };
                let p = &t.effects[fi].params;
                if p.len() < 5 {
                    return ok!(Val::List(vec![]));
                }
                ok!(Val::List(vec![Val::Num(depth_r(p[0].1)), Val::Num(attack_r(p[1].1)), Val::Num(hold_r(p[2].1)), Val::Num(release_r(p[3].1)), Val::Num(p[4].1 as f64)]))
            }
            "daw_duck_shape" => {
                // (depth dB, attack ms, hold ms, release ms, curve 0..1, points) -> the volume of one dip, 1 = full volume
                let sr = 48000.0;
                let mut d = Ducker::new(sr);
                d.set_param("depth", depth_n(a.n(0)));
                d.set_param("attack", attack_n(a.n(1)));
                d.set_param("hold", hold_n(a.n(2)));
                d.set_param("release", release_n(a.n(3)));
                d.set_param("curve", a.n(4).clamp(0.0, 1.0) as f32);
                ok!(Val::List(d.shape(a.u(5).clamp(2, 256)).into_iter().map(|x| Val::Num(x as f64)).collect()))
            }
            _ => Err(format!("unknown builtin {name}")),
        }
    }
}
