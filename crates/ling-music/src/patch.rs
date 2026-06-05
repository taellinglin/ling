//! Parse a `.ling` instrument **patch** into a [`Patch`] for the synth.
//!
//! Compact key=value DSL (whitespace- or newline-separated), e.g.:
//! ```text
//! name = "Rhodes EP"
//! gm = 5
//! poly = 16
//! op1.wave=sine  op1.ratio=1.0  op1.level=1.0  op1.adsr=0.002,1.2,0.0,0.4
//! op2.wave=sine  op2.ratio=14.0 op2.level=0.5  op2.fm=op1  op2.adsr=0.001,0.2,0.0,0.2
//! filter.cutoff=0.8 filter.reso=0.2 filter.env=0.3 filter.adsr=0,0.5,0.2,0.3
//! lfo.rate=5.0 lfo.pitch=0.05
//! amp = 0.7
//! ```
//! Operators are 1-indexed in the file. FM convention: an operator may modulate a
//! **lower-indexed** one (`op2.fm=op1`). Any `filter.*` key turns the filter on.

use crate::synth::{Adsr, Filter, Op, Patch, Wave};

fn adsr(v: &str) -> Adsr {
    let mut it = v.split(',').map(|s| s.trim().parse::<f32>().unwrap_or(0.0));
    Adsr {
        a: it.next().unwrap_or(0.005),
        d: it.next().unwrap_or(0.1),
        s: it.next().unwrap_or(0.8),
        r: it.next().unwrap_or(0.3),
    }
}

fn op_index(s: &str) -> Option<usize> {
    // "op2" → 1, "2" → 1
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    digits.parse::<usize>().ok().filter(|&n| n >= 1).map(|n| n - 1)
}

/// Parse a patch from DSL text.
pub fn from_str(text: &str) -> Patch {
    let mut p = Patch { ops: Vec::new(), filter: Filter::default(), ..Patch::default() };
    let ensure = |ops: &mut Vec<Op>, idx: usize| { while ops.len() <= idx { ops.push(Op::default()); } };

    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() { continue; }
        // name may contain spaces → take the rest of the line
        if let Some(rest) = line.strip_prefix("name") {
            if let Some(eq) = rest.find('=') {
                p.name = rest[eq + 1..].trim().trim_matches('"').to_string();
                continue;
            }
        }
        // Collapse spaces around '=' so both `gm = 5` and `op1.wave=sine` tokenize.
        // (Values never contain spaces; the spaced `name = "..."` line is handled above.)
        let norm: String = line.split('=').map(|p| p.trim()).collect::<Vec<_>>().join("=");
        for tok in norm.split_whitespace() {
            let (key, val) = match tok.split_once('=') { Some(kv) => kv, None => continue };
            let key = key.trim(); let val = val.trim();
            match key {
                "gm"   => p.gm   = val.parse().unwrap_or(0),
                "poly" => p.poly = val.parse::<usize>().unwrap_or(16).max(1),
                "amp"  => p.amp  = val.parse().unwrap_or(0.8),
                "lfo.rate"   => p.lfo.rate   = val.parse().unwrap_or(5.0),
                "lfo.pitch"  => p.lfo.pitch  = val.parse().unwrap_or(0.0),
                "lfo.cutoff" => p.lfo.cutoff = val.parse().unwrap_or(0.0),
                "lfo.amp"    => p.lfo.amp    = val.parse().unwrap_or(0.0),
                "filter.cutoff" => { p.filter.cutoff = val.parse().unwrap_or(1.0); p.filter.on = true; }
                "filter.reso"   => { p.filter.reso   = val.parse().unwrap_or(0.2); p.filter.on = true; }
                "filter.env"    => { p.filter.env    = val.parse().unwrap_or(0.0); p.filter.on = true; }
                "filter.adsr"   => { p.filter.adsr   = adsr(val); p.filter.on = true; }
                "filter.on"     => { p.filter.on = val != "0" && val != "false"; }
                k if k.starts_with("op") => {
                    let mut parts = k.splitn(2, '.');
                    let opn = parts.next().unwrap_or("");
                    let field = parts.next().unwrap_or("");
                    if let Some(idx) = op_index(opn) {
                        ensure(&mut p.ops, idx);
                        let op = &mut p.ops[idx];
                        match field {
                            "wave"   => op.wave = Wave::from_name(val),
                            "ratio"  => op.ratio = val.parse().unwrap_or(1.0),
                            "fixed"  => op.fixed_hz = val.parse().unwrap_or(0.0),
                            "level"  => op.level = val.parse().unwrap_or(1.0),
                            "detune" => op.detune = val.parse().unwrap_or(0.0),
                            "adsr"   => op.adsr = adsr(val),
                            "fm"     => op.fm_target = op_index(val).map(|i| i as i32).unwrap_or(-1),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }
    if p.ops.is_empty() { p.ops.push(Op::default()); }
    p
}

/// Parse a patch from a file path.
pub fn from_path(path: &str) -> std::io::Result<Patch> {
    Ok(from_str(&std::fs::read_to_string(path)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_fm_patch() {
        let p = from_str(r#"
            name = "Rhodes EP"
            gm = 5
            poly = 12
            op1.wave=sine op1.ratio=1.0 op1.level=1.0 op1.adsr=0.002,1.2,0.0,0.4
            op2.wave=sine op2.ratio=14.0 op2.level=0.5 op2.fm=op1 op2.adsr=0.001,0.2,0.0,0.2
            filter.cutoff=0.8 filter.env=0.3
            lfo.rate=5.0 lfo.pitch=0.05
            amp = 0.7
        "#);
        assert_eq!(p.name, "Rhodes EP");
        assert_eq!(p.gm, 5);
        assert_eq!(p.poly, 12);
        assert_eq!(p.ops.len(), 2);
        assert_eq!(p.ops[1].fm_target, 0);
        assert_eq!(p.ops[1].wave, Wave::Sine);
        assert!((p.ops[0].adsr.d - 1.2).abs() < 1e-4);
        assert!(p.filter.on);
        assert!((p.amp - 0.7).abs() < 1e-4);
    }
}
