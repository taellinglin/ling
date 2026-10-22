//! Builtins for scripted plugins (`daw_script_*`): custom effects and instruments written in LingDSP (see dsp/scriptdsp.rs).
use super::{Daw, A, Val};
use crate::api::PALETTE;
use crate::dsp::scriptdsp::{self, ScriptFx, ScriptSynth};
use crate::dsp::Instrument;
use crate::model::*;

fn target_fx(en: &mut crate::engine::Engine, target: i64) -> Option<&mut Vec<EffectSlot>> {
    if target < 0 {
        Some(&mut en.project.master_effects)
    } else {
        en.project.tracks.get_mut(target as usize).map(|t| &mut t.effects)
    }
}

impl Daw {
    pub(super) fn scriptdsp_dispatch(&mut self, name: &str, a: &A) -> Result<Val, String> {
        let eng = self.out.engine.clone();
        macro_rules! ok {
            ($v:expr) => {
                return Ok(Val::from($v))
            };
        }
        match name {
            "daw_script_check" => {
                // (source, "fx" | "instrument") -> "" when it compiles, else the error text
                match scriptdsp::compile(&a.s(0), a.s(1) == "instrument") {
                    Ok(_) => ok!(""),
                    Err(e) => ok!(e),
                }
            }
            "daw_script_fx_add" => {
                // (target track or -1 for the master, label, source, script path) -> effect index, or -1 (the error is in the status)
                let sr = { eng.lock().unwrap_or_else(|p| p.into_inner()).sr };
                let mut fx = match ScriptFx::from_source(sr, &a.s(2), &a.s(3)) {
                    Ok(f) => f,
                    Err(e) => {
                        self.status = format!("Script effect: {e}");
                        return ok!(-1.0);
                    }
                };
                fx.set_label(&a.s(1));
                let slot = EffectSlot { name: a.s(1), params: fx.defaults(), bypass: false, fx: Box::new(fx) };
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let Some(v) = target_fx(&mut en, a.n(0) as i64) else { return ok!(-1.0) };
                v.push(slot);
                let i = v.len() - 1;
                drop(en);
                self.touch();
                ok!(i)
            }
            "daw_script_inst_add" => {
                // (label, source, script path) -> the new track, or -1
                let sr = { eng.lock().unwrap_or_else(|p| p.into_inner()).sr };
                let mut s = ScriptSynth::new(sr);
                if let Err(e) = s.load(&a.s(1)) {
                    self.status = format!("Script instrument: {e}");
                    return ok!(-1.0);
                }
                s.command("load_source", &[], &serde_json::json!({ "path": a.s(2), "source": a.s(1) }).to_string());
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let n = en.project.tracks.len();
                if n >= MAX_TRACKS {
                    return ok!(-1.0);
                }
                let mut t = Track::new(&a.s(0), TrackKind::Instrument, PALETTE[n % 7], Some(Box::new(s)));
                t.inst_kind = "script".into();
                en.project.tracks.push(t);
                drop(en);
                self.touch();
                ok!(n)
            }
            "daw_script_reload" => {
                // (script path, source) -> instances updated: every effect and instrument made from that script takes the new program
                let (path, src) = (a.s(0), a.s(1));
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let mut n = 0;
                let leak = |s: &str| -> &'static str { Box::leak(s.to_string().into_boxed_str()) };
                let fix = |slots: &mut Vec<EffectSlot>, n: &mut usize| {
                    for s in slots.iter_mut() {
                        if !path.is_empty() && s.fx.info("path") == path {
                            if s.fx.command("load_source", &[], &src).is_empty() {
                                continue;
                            }
                            // the knob list may have changed
                            if let Ok(v) = serde_json::from_str::<Vec<(String, f32)>>(&s.fx.info("params")) {
                                s.params = v.into_iter().map(|(k, x)| (leak(&k), x)).collect();
                            }
                            *n += 1;
                        }
                    }
                };
                fix(&mut en.project.master_effects, &mut n);
                for t in en.project.tracks.iter_mut() {
                    fix(&mut t.effects, &mut n);
                    if t.inst_kind == "script" {
                        if let Some(i) = t.inst.as_mut() {
                            if i.info("path") == path && !i.command("load_source", &[], &serde_json::json!({ "path": path, "source": src }).to_string()).is_empty() {
                                n += 1;
                            }
                        }
                    }
                }
                drop(en);
                if n > 0 {
                    self.touch();
                }
                ok!(n)
            }
            _ => Err(format!("unknown builtin {name}")),
        }
    }
}
