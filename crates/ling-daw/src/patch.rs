//! Instrument patches — plain `.ling` text files, the "plugin" format of LingStationX.
//!
//! ```text
//! # Ling Synth X — Sub Bass
//! name = "Sub Bass"
//! type = synth                 # synth | drums
//! wave=0.66 sub=1.0 cutoff=0.3 reso=0.4 detune=0.1
//! attack=0.07 release=0.16
//! fx Delay time=0.2 feedback=0.35 mix=0.25
//! ```
//! Keys are the instrument's parameter names with spaces written as `_`
//! (`filt_env`). Drum kits use `<piece>.<param>` (`snare.tune=0.4`).
//! Values are normalised 0..1. `fx <Name> k=v ...` adds an effect to the track.
use crate::dsp::{make_instrument, Instrument};

#[derive(Clone, Debug, Default)]
pub struct FxSpec {
    pub name: String,
    pub params: Vec<(String, f32)>,
}

#[derive(Clone, Debug, Default)]
pub struct Patch {
    pub kind: String,
    pub name: String,
    pub params: Vec<(String, f32)>,
    pub fx: Vec<FxSpec>,
    /// soundfont path for `type = sf2` patches
    pub file: String,
}

fn strip_quotes(s: &str) -> &str {
    s.trim().trim_matches('"')
}

pub fn parse(text: &str) -> Patch {
    let mut p = Patch { kind: "synth".into(), ..Default::default() };
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("fx ") {
            let mut it = rest.split_whitespace();
            if let Some(name) = it.next() {
                let mut spec = FxSpec { name: name.to_string(), params: vec![] };
                for tok in it {
                    if let Some((k, v)) = tok.split_once('=') {
                        if let Ok(v) = v.parse::<f32>() {
                            spec.params.push((k.to_string(), v));
                        }
                    }
                }
                p.fx.push(spec);
            }
            continue;
        }
        // `name = "Sub Bass"` may contain spaces; handle before token split
        if let Some(rest) = line.strip_prefix("file") {
            if let Some(v) = rest.trim_start().strip_prefix('=') {
                p.file = strip_quotes(v).to_string();
                continue;
            }
        }
        if let Some(rest) = line.strip_prefix("name") {
            if let Some(v) = rest.trim_start().strip_prefix('=') {
                p.name = strip_quotes(v).to_string();
                continue;
            }
        }
        // tokens may be `k=v` or `k = v`; normalise spaces around '='
        let norm = line.replace(" = ", "=").replace("= ", "=").replace(" =", "=");
        for tok in norm.split_whitespace() {
            let Some((k, v)) = tok.split_once('=') else { continue };
            let v = strip_quotes(v);
            match k {
                "type" => p.kind = v.to_string(),
                _ => {
                    if let Ok(x) = v.parse::<f32>() {
                        let key = if k.contains('.') { k.to_string() } else { k.replace('_', " ") };
                        p.params.push((key, x));
                    }
                }
            }
        }
    }
    p
}

pub fn build(p: &Patch, sr: f32) -> Option<Box<dyn Instrument>> {
    let mut inst = crate::dsp::make_instrument_file(&p.kind, sr, &p.file)?;
    inst.load_state(&p.params);
    // a factory preset's file: the level trim that puts it with the rest of its instrument (presetinfo.rs)
    if !p.name.is_empty() {
        crate::presetinfo::apply_trim(inst.as_mut(), &p.kind, &p.name);
    }
    Some(inst)
}

pub fn dump(kind: &str, name: &str, state: &[(String, f32)], fx: &[FxSpec]) -> String {
    let mut s = String::new();
    s.push_str(&format!("# {} patch — edit freely; load from the Instrument tab\n", match kind { "drums" => "Ling Membrane", "growl" => "Ling Growl", "voice" => "Ling Voice",
        "prism" => "Ling Prism", _ => "Ling Synth X" }));
    s.push_str(&format!("name = \"{name}\"\ntype = {kind}\n"));
    let mut line = String::new();
    for (k, v) in state {
        let key = if k.contains('.') { k.clone() } else { k.replace(' ', "_") };
        let tok = format!("{key}={v:.3} ");
        if line.len() + tok.len() > 88 {
            s.push_str(line.trim_end());
            s.push('\n');
            line.clear();
        }
        line.push_str(&tok);
    }
    if !line.is_empty() {
        s.push_str(line.trim_end());
        s.push('\n');
    }
    for f in fx {
        s.push_str(&format!("fx {}", f.name));
        for (k, v) in &f.params {
            s.push_str(&format!(" {k}={v:.3}"));
        }
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        let mut inst = make_instrument("synth", 48000.0).unwrap();
        inst.load_preset("FM Bell");
        let text = dump("synth", "FM Bell", &inst.state(), &[FxSpec { name: "Delay".into(), params: vec![("mix".into(), 0.2)] }]);
        let p = parse(&text);
        assert_eq!(p.kind, "synth");
        assert_eq!(p.name, "FM Bell");
        assert_eq!(p.fx.len(), 1);
        let inst2 = build(&p, 48000.0).unwrap();
        for ((a, x), (b, y)) in inst.state().iter().zip(inst2.state().iter()) {
            assert_eq!(a, b);
            assert!((x - y).abs() < 0.002, "{a}: {x} vs {y}");
        }
    }
    #[test]
    fn drum_kit_roundtrip() {
        let mut d = make_instrument("drums", 48000.0).unwrap();
        d.select_part(1);
        d.set_param("tune", 0.9);
        let text = dump("drums", "Kit", &d.state(), &[]);
        assert!(text.contains("snare.tune=0.900"));
        let d2 = build(&parse(&text), 48000.0).unwrap();
        assert!(d2.state().iter().any(|(k, v)| k == "snare.tune" && (*v - 0.9).abs() < 0.002));
    }
}
