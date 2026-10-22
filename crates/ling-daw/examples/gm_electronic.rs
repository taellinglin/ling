//! Balance the loudness of the electronic GM bank and write it out as patch files.
//!   cargo run --release -p ling-daw --example gm_electronic [raw file] [output dir]
//! `raw file` is tools/gm_electronic_raw.txt from LingStationX/tools/gen_gm_electronic.py
//! (sections `[n]`, one patch each); the output dir is instruments/Electronic GM.
use ling_daw::patch;

const SR: f32 = 48000.0;

fn measure(p: &patch::Patch, key: u8) -> (f32, f32) {
    let mut inst = patch::build(p, SR).unwrap();
    inst.note_on(key, 0.8);
    let mut out = vec![0.0f32; 256 * 2];
    let mut win: Vec<f32> = vec![];
    let mut peak = 0.0f32;
    for blk in 0..(SR as usize * 12 / 10 / 256) {
        out.fill(0.0);
        if blk == (SR as usize * 8 / 10 / 256) {
            inst.note_off(key);
        }
        inst.render(&mut out);
        for x in &out {
            assert!(x.is_finite(), "non-finite output");
            peak = peak.max(x.abs());
        }
        win.push(out.iter().map(|x| x * x).sum::<f32>() / out.len() as f32);
    }
    let n = 47;
    let mut best = 0.0f32;
    for i in 0..win.len().saturating_sub(n) {
        best = best.max(win[i..i + n].iter().sum::<f32>() / n as f32);
    }
    (best.sqrt(), peak)
}

fn main() {
    let raw = std::env::args().nth(1).unwrap_or_else(|| "tools/gm_electronic_raw.txt".into());
    let dir = std::env::args().nth(2).unwrap_or_else(|| "instruments/Electronic GM".into());
    let text = std::fs::read_to_string(&raw).expect("raw file");
    let mut sections: Vec<(usize, String)> = vec![];
    for line in text.lines() {
        if let Some(n) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            sections.push((n.parse().unwrap(), String::new()));
        } else if let Some(s) = sections.last_mut() {
            s.1.push_str(line);
            s.1.push('\n');
        }
    }
    assert_eq!(sections.len(), 128);
    std::fs::create_dir_all(&dir).unwrap();
    let target = 0.09f32;
    for (n, body) in &sections {
        let mut p = patch::parse(body);
        // the level control: Ling Synth X "gain", Growl "out" (squared)
        let (lvl, sq) = if p.kind == "growl" { ("out", true) } else { ("gain", false) };
        let keys = [45u8, 60, 76];
        let mut lg = 0.0;
        for k in keys {
            lg += measure(&p, k).0.max(1e-5).ln();
        }
        let rms = (lg / keys.len() as f32).exp();
        let cur = p.params.iter().find(|(k, _)| k == lvl).map(|x| x.1).unwrap_or(if sq { 0.8 } else { 0.5 });
        let scale = target / rms;
        let mut ng = if sq { (cur * cur * scale).sqrt() } else { cur * scale }.clamp(0.02, 1.0);
        let set = |p: &mut patch::Patch, v: f32| match p.params.iter_mut().find(|(k, _)| k == lvl) {
            Some(e) => e.1 = v,
            None => p.params.push((lvl.into(), v)),
        };
        set(&mut p, ng);
        let peak = [24u8, 45, 60, 84, 100].iter().map(|k| measure(&p, *k).1).fold(0.0, f32::max);
        if peak > 1.0 {
            ng = if sq { ng / peak.sqrt() * 0.97 } else { ng / peak * 0.95 };
            set(&mut p, ng);
        }
        let inst = patch::build(&p, SR).unwrap();
        let mut st = inst.state();
        for (k, v) in &p.params {
            if let Some(e) = st.iter_mut().find(|(sk, _)| sk == k) {
                e.1 = *v;
            }
        }
        let dumped = patch::dump(&p.kind, &p.name, &st, &p.fx);
        println!("{n:3} {:<22} {:<5} rms {rms:.3} -> {lvl} {ng:.2} peak {peak:.2}", p.name, p.kind);
        std::fs::write(format!("{dir}/{:03} {}.ling", n + 1, p.name.replace('/', "-")), dumped).unwrap();
    }
}
