//! Builtins for per-note expression in the piano roll (`daw_nx_*`, routed in api.rs): pitch gestures (scoop, fall, slide, dive,
//! riser, or drawn), pressure and timbre curves on notes, and the velocity tools (freehand values, the line with influence).
//! Curves are (position 0..1 of the note, value); see gesture.rs. Every edit here is the user's own, so the pitch guard does not
//! apply. Undo: the roll pushes one undo step per stroke / command before calling these.
use super::{Daw, A, Val};
use crate::gesture;
use crate::model::*;

fn which_of(s: &str) -> usize {
    match s {
        "press" | "pressure" | "at" | "aftertouch" => 1,
        "timbre" | "slide" | "y" => 2,
        _ => 0, // bend / pitch
    }
}
fn rest_of(w: usize) -> f32 {
    if w == 2 {
        0.5
    } else {
        0.0
    }
}
fn curve_mut(n: &mut Note, w: usize) -> &mut Curve {
    match w {
        1 => &mut n.press,
        2 => &mut n.timbre,
        _ => &mut n.bend,
    }
}
fn curve_ref(n: &Note, w: usize) -> &Curve {
    match w {
        1 => &n.press,
        2 => &n.timbre,
        _ => &n.bend,
    }
}
fn nums(v: Option<&Val>) -> Vec<f64> {
    match v {
        Some(Val::List(l)) => l.iter().map(|x| if let Val::Num(n) = x { *n } else { 0.0 }).collect(),
        Some(Val::Num(n)) => vec![*n],
        _ => vec![],
    }
}

impl Daw {
    pub(super) fn nx_dispatch(&mut self, name: &str, a: &A) -> Result<Val, String> {
        let eng = self.out.engine.clone();
        macro_rules! ok {
            ($v:expr) => {
                return Ok(Val::from($v))
            };
        }
        macro_rules! notes_mut {
            ($en:ident) => {
                match $en.project.tracks.get_mut(a.u(0)).and_then(|t| t.clips.get_mut(a.u(1))) {
                    Some(Clip { data: ClipData::Midi(n), .. }) => n,
                    _ => return Ok(Val::from(false)),
                }
            };
        }
        match name {
            "daw_nx_shapes" => ok!(Val::List(gesture::SHAPES.iter().map(|s| Val::Str(s.to_string())).collect())),
            "daw_nx_flags" => {
                // (t, c) -> per note: 1 pitch gesture + 2 pressure + 4 timbre (in daw_clip_notes order)
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let Some(Clip { data: ClipData::Midi(n), .. }) = en.project.tracks.get(a.u(0)).and_then(|t| t.clips.get(a.u(1))) else { return ok!(Val::List(vec![])) };
                ok!(Val::List(n.iter().map(|x| Val::Num((!x.bend.is_empty()) as u8 as f64 + 2.0 * (!x.press.is_empty()) as u8 as f64 + 4.0 * (!x.timbre.is_empty()) as u8 as f64)).collect()))
            }
            "daw_nx_get" => {
                // (t, c, which "bend" | "press" | "timbre") -> per note: [count, pos, value, pos, value ...] one after another
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let Some(Clip { data: ClipData::Midi(n), .. }) = en.project.tracks.get(a.u(0)).and_then(|t| t.clips.get(a.u(1))) else { return ok!(Val::List(vec![])) };
                let w = which_of(&a.s(2));
                let mut v = vec![];
                for x in n {
                    let c = curve_ref(x, w);
                    v.push(Val::Num(c.len() as f64));
                    for p in c {
                        v.push(Val::Num(p.0 as f64));
                        v.push(Val::Num(p.1 as f64));
                    }
                }
                ok!(Val::List(v))
            }
            "daw_nx_at" => {
                // (t, c, idx, which, pos 0..1) -> the curve's value there (the rest value when it has none)
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let Some(Clip { data: ClipData::Midi(n), .. }) = en.project.tracks.get(a.u(0)).and_then(|t| t.clips.get(a.u(1))) else { return ok!(0.0) };
                let w = which_of(&a.s(3));
                let Some(x) = n.get(a.u(2)) else { return ok!(rest_of(w) as f64) };
                let c = curve_ref(x, w);
                ok!(if c.is_empty() { rest_of(w) as f64 } else { gesture::at(c, a.n(4) as f32) as f64 })
            }
            "daw_nx_shape" => {
                // (t, c, [indices] or idx, shape "scoop" | "fall" | "slide" | "dive" | "riser" | "clear", depth semitones or 0 = the
                // shape's default) -> how many notes got it
                let idx = nums(a.0.get(2));
                let (shape, depth) = (a.s(3).to_lowercase(), a.n(4) as f32);
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let n = notes_mut!(en);
                let mut k = 0;
                for i in idx {
                    if i >= 0.0 && gesture::apply_shape(n, i as usize, &shape, depth) {
                        k += 1;
                    }
                }
                drop(en);
                self.touch();
                ok!(k)
            }
            "daw_nx_depth" => {
                // (t, c, [indices], depth semitones): the gestures keep their shape at a new depth
                let idx = nums(a.0.get(2));
                let d = a.n(3) as f32;
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let n = notes_mut!(en);
                for i in idx {
                    if let Some(x) = n.get_mut(i.max(0.0) as usize) {
                        x.bend = gesture::with_depth(&x.bend, d);
                    }
                }
                drop(en);
                self.touch();
                ok!(true)
            }
            "daw_nx_draw" => {
                // (t, c, idx, which, p0, v0, p1, v1): a stroke on the note's curve (positions 0..1 of the note) replaces what it
                // passes over with a line
                let w = which_of(&a.s(3));
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let n = notes_mut!(en);
                if let Some(x) = n.get_mut(a.u(2)) {
                    let lim = if w == 0 { gesture::MAX_SEMIS } else { 1.0 };
                    let lo = if w == 0 { -lim } else { 0.0 };
                    let c = curve_mut(x, w);
                    *c = gesture::draw(c, rest_of(w), a.n(4) as f32, (a.n(5) as f32).clamp(lo, lim), a.n(6) as f32, (a.n(7) as f32).clamp(lo, lim));
                }
                drop(en);
                self.touch();
                ok!(true)
            }
            "daw_nx_clear" => {
                // (t, c, [indices], which or "all")
                let idx = nums(a.0.get(2));
                let all = a.s(3) == "all";
                let w = which_of(&a.s(3));
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let n = notes_mut!(en);
                for i in idx {
                    if let Some(x) = n.get_mut(i.max(0.0) as usize) {
                        if all {
                            x.bend.clear();
                            x.press.clear();
                            x.timbre.clear();
                        } else {
                            curve_mut(x, w).clear();
                        }
                    }
                }
                drop(en);
                self.touch();
                ok!(true)
            }
            "daw_nx_tidy" => {
                // (t, c, idx): drop curves that never leave their rest value (after a stroke)
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let n = notes_mut!(en);
                if let Some(x) = n.get_mut(a.u(2)) {
                    gesture::prune(&mut x.bend, 0.0);
                    gesture::prune(&mut x.press, 0.0);
                    gesture::prune(&mut x.timbre, 0.5);
                }
                ok!(true)
            }
            "daw_nx_text" => {
                // (t, c, idx) -> the note's curves as text "bend|press|timbre" (the roll's clipboard and duplicates)
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let Some(Clip { data: ClipData::Midi(n), .. }) = en.project.tracks.get(a.u(0)).and_then(|t| t.clips.get(a.u(1))) else { return ok!("") };
                ok!(n.get(a.u(2)).map(|x| if x.has_expr() { format!("{}|{}|{}", gesture::to_text(&x.bend), gesture::to_text(&x.press), gesture::to_text(&x.timbre)) } else { String::new() }).unwrap_or_default())
            }
            "daw_nx_set_text" => {
                // (t, c, idx, text from daw_nx_text)
                let text = a.s(3);
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let n = notes_mut!(en);
                if let Some(x) = n.get_mut(a.u(2)) {
                    let mut parts = text.split('|');
                    x.bend = gesture::from_text(parts.next().unwrap_or(""));
                    x.press = gesture::from_text(parts.next().unwrap_or(""));
                    x.timbre = gesture::from_text(parts.next().unwrap_or(""));
                }
                drop(en);
                self.touch();
                ok!(true)
            }
            "daw_nx_vel_set" => {
                // (t, c, [indices], [velocities]) in one call (the freehand velocity stroke)
                let (idx, vals) = (nums(a.0.get(2)), nums(a.0.get(3)));
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let n = notes_mut!(en);
                for (i, v) in idx.iter().zip(vals.iter()) {
                    if let Some(x) = n.get_mut(i.max(0.0) as usize) {
                        x.vel = (*v as f32).clamp(0.02, 1.0);
                    }
                }
                drop(en);
                self.touch();
                ok!(true)
            }
            "daw_nx_vel_line" => {
                // (t, c, [indices], [their velocities when the stroke began], [the line's value at each], influence 0..1,
                //  absolute 0 | 1) -> the new velocities. Relative: new = old + influence x (line - mean(old)) (a crescendo that
                // keeps the accents); absolute: new = old + influence x (line - old).
                let (idx, old, line) = (nums(a.0.get(2)), nums(a.0.get(3)), nums(a.0.get(4)));
                let (inf, abs) = (a.n(5) as f32, a.n(6) > 0.5);
                let o: Vec<f32> = old.iter().map(|x| *x as f32).collect();
                let l: Vec<f32> = line.iter().map(|x| *x as f32).collect();
                let new = gesture::velocity_line(&o, &l, inf, abs);
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let n = notes_mut!(en);
                for (i, v) in idx.iter().zip(new.iter()) {
                    if let Some(x) = n.get_mut(i.max(0.0) as usize) {
                        x.vel = *v;
                    }
                }
                drop(en);
                self.touch();
                ok!(Val::List(new.iter().map(|v| Val::Num(*v as f64)).collect()))
            }
            other => Err(format!("unknown builtin {other}")),
        }
    }
}
