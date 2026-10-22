//! Project parameters: a flat table of named numbers plus the acoustic source list.

use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Waveform {
    Ricker,
    SineBurst,
    Impulse,
}

impl Waveform {
    pub fn from_index(i: i64) -> Waveform {
        match i {
            1 => Waveform::SineBurst,
            2 => Waveform::Impulse,
            _ => Waveform::Ricker,
        }
    }

    pub fn index(self) -> i64 {
        match self {
            Waveform::Ricker => 0,
            Waveform::SineBurst => 1,
            Waveform::Impulse => 2,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Waveform::Ricker => "ricker",
            Waveform::SineBurst => "sine_burst",
            Waveform::Impulse => "impulse",
        }
    }

    pub fn from_name(s: &str) -> Waveform {
        match s {
            "sine_burst" => Waveform::SineBurst,
            "impulse" => Waveform::Impulse,
            _ => Waveform::Ricker,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub name: String,
    pub pos: [f32; 3],
    pub wave: Waveform,
    pub freq: f32,
    pub amp: f32,
    pub phase: f32,
    /// Seconds after the start of the simulation.
    pub start: f32,
    pub dur: f32,
}

impl Source {
    pub fn new(name: &str, pos: [f32; 3], wave: Waveform, freq: f32) -> Source {
        Source { name: name.to_string(), pos, wave, freq, amp: 1.0, phase: 0.0, start: 0.0, dur: 0.02 }
    }
}

/// `(name, default, min, max, group)`
pub const DEFS: &[(&str, f64, f64, f64, &str)] = &[
    ("height", 1.70, 0.8, 2.4, "body"),
    ("build", 1.0, 0.6, 1.6, "body"),
    ("arm_span", 1.0, 0.8, 1.25, "body"),
    ("head_scale", 1.0, 0.7, 1.4, "body"),
    ("seal", 3.0, 1.0, 8.0, "body"),
    ("sr", 22050.0, 8000.0, 96000.0, "sim"),
    ("steps", 800.0, 50.0, 40000.0, "sim"),
    ("margin", 16.0, 8.0, 40.0, "sim"),
    ("up", 2.0, 1.0, 4.0, "sim"),
    ("c_air", 343.0, 100.0, 2000.0, "medium"),
    ("c_body", 1.0, 0.5, 4.0, "medium"),
    ("rho_body", 4.0, 0.2, 12.0, "medium"),
    ("damping", 0.002, 0.0, 0.2, "medium"),
    ("sponge", 8.0, 2.0, 24.0, "medium"),
    ("mic_sep", 0.18, 0.02, 0.6, "mic"),
    ("mic_dist", 0.15, 0.02, 1.0, "mic"),
    ("tension", 0.5, 0.0, 5.0, "membrane"),
    ("coupling", 1.0, 0.0, 5.0, "membrane"),
    ("threshold", 0.0, -2.0, 3.0, "membrane"),
    ("stiffness", 1.0, 0.01, 10.0, "membrane"),
    ("relief", 0.008, 0.0, 0.06, "membrane"),
    ("mem_time", 3.0, 0.2, 20.0, "membrane"),
    ("mem_every", 8.0, 1.0, 64.0, "membrane"),
    ("reinit_every", 20.0, 1.0, 200.0, "membrane"),
    ("band", 6.0, 3.0, 16.0, "membrane"),
    ("drive_mode", 1.0, 0.0, 1.0, "membrane"),
    ("drive_tau", 0.01, 0.0005, 0.5, "membrane"),
    ("iso", 0.0, -0.05, 0.05, "mesh"),
    ("smooth", 3.0, 0.0, 30.0, "mesh"),
    ("decimate", 0.0, 0.0, 0.1, "mesh"),
    ("wav_bits", 16.0, 16.0, 32.0, "audio"),
    ("wav_gain", 0.9, 0.05, 1.0, "audio"),
    ("wav_repeat", 4.0, 1.0, 32.0, "audio"),
    ("wav_gap", 0.25, 0.0, 2.0, "audio"),
    ("crease_mode", 0.0, 0.0, 1.0, "detail"),
    ("crease_levels", 3.0, 1.0, 6.0, "detail"),
    ("crease_tol", 2.0, 0.5, 10.0, "detail"),
    ("crease_min", 60.0, 20.0, 400.0, "detail"),
    ("crease_depth", 3.5, 0.0, 10.0, "detail"),
    ("crease_width", 5.0, 1.0, 20.0, "detail"),
    ("noise_amp", 1.2, 0.0, 5.0, "detail"),
    ("noise_freq", 60.0, 10.0, 300.0, "detail"),
    ("detail_level", 1.0, 0.0, 2.0, "detail"),
    ("sig_dur", 0.5, 0.1, 2.0, "listen"),
    ("sig_sr", 16000.0, 8000.0, 32000.0, "listen"),
    ("sig_rho", 40.0, 1.0, 200.0, "listen"),
    ("sig_excite", 1.0, 0.0, 2.0, "listen"),
];

#[derive(Clone, Debug)]
pub struct Params {
    vals: BTreeMap<String, f64>,
    pub sources: Vec<Source>,
}

impl Default for Params {
    fn default() -> Self {
        let vals = DEFS.iter().map(|d| (d.0.to_string(), d.1)).collect();
        Params { vals, sources: Vec::new() }
    }
}

impl Params {
    pub fn def(name: &str) -> Option<&'static (&'static str, f64, f64, f64, &'static str)> {
        DEFS.iter().find(|d| d.0 == name)
    }

    pub fn get(&self, name: &str) -> f64 {
        self.vals.get(name).copied().or_else(|| Self::def(name).map(|d| d.1)).unwrap_or(0.0)
    }

    pub fn getf(&self, name: &str) -> f32 {
        self.get(name) as f32
    }

    /// Stores the value clamped to its declared range; false for unknown names.
    pub fn set(&mut self, name: &str, v: f64) -> bool {
        match Self::def(name) {
            Some(d) => {
                let v = if v.is_finite() { v.clamp(d.2, d.3) } else { d.1 };
                self.vals.insert(name.to_string(), v);
                true
            },
            None => false,
        }
    }

    pub fn reset(&mut self) {
        *self = Params::default();
    }

    pub fn to_json(&self) -> Value {
        let vals: serde_json::Map<String, Value> =
            DEFS.iter().map(|d| (d.0.to_string(), json!(self.get(d.0)))).collect();
        let sources: Vec<Value> = self
            .sources
            .iter()
            .map(|s| {
                json!({
                    "name": s.name, "pos": s.pos, "waveform": s.wave.name(), "freq": s.freq,
                    "amp": s.amp, "phase": s.phase, "start": s.start, "duration": s.dur,
                })
            })
            .collect();
        json!({ "format": "ling-body/1", "params": vals, "sources": sources })
    }

    pub fn from_json(v: &Value) -> Result<Params, String> {
        let mut p = Params::default();
        let obj = v.get("params").and_then(Value::as_object).ok_or("missing params")?;
        for (k, x) in obj {
            if let Some(n) = x.as_f64() {
                p.set(k, n);
            }
        }
        if let Some(list) = v.get("sources").and_then(Value::as_array) {
            for s in list {
                let f = |k: &str, d: f64| s.get(k).and_then(Value::as_f64).unwrap_or(d) as f32;
                let pos = s.get("pos").and_then(Value::as_array);
                let c = |i: usize| pos.and_then(|a| a.get(i)).and_then(Value::as_f64).unwrap_or(0.0) as f32;
                p.sources.push(Source {
                    name: s.get("name").and_then(Value::as_str).unwrap_or("src").to_string(),
                    pos: [c(0), c(1), c(2)],
                    wave: Waveform::from_name(s.get("waveform").and_then(Value::as_str).unwrap_or("")),
                    freq: f("freq", 220.0).clamp(1.0, 40000.0),
                    amp: f("amp", 1.0),
                    phase: f("phase", 0.0),
                    start: f("start", 0.0).max(0.0),
                    dur: f("duration", 0.02).max(0.0),
                });
            }
        }
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip() {
        let mut p = Params::default();
        p.set("height", 1.9);
        p.sources.push(Source::new("chest", [0.0, 1.2, 0.0], Waveform::SineBurst, 330.0));
        let q = Params::from_json(&p.to_json()).unwrap();
        assert_eq!(q.get("height"), 1.9);
        assert_eq!(q.sources, p.sources);
    }

    #[test]
    fn set_clamps() {
        let mut p = Params::default();
        p.set("sr", 1.0);
        assert_eq!(p.get("sr"), 8000.0);
        assert!(!p.set("nope", 1.0));
    }
}
