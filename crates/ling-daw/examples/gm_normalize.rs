//! Balance the loudness of the 128 GM patches and write them out in full.
//!   cargo run --release -p ling-daw --example gm_normalize [instruments/GM dir]
//! reads src/gm_patches.txt (from LingStationX/tools/gen_gm.py), rewrites it with tuned `gain`
//! values and complete parameter lists, and writes one .ling file per patch.
use ling_daw::dsp::{make_instrument, Instrument};
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
    // loudest ~250 ms window (percussive patches are judged on their strike, sustained ones on the body)
    let n = 47;
    let mut best = 0.0f32;
    for i in 0..win.len().saturating_sub(n) {
        best = best.max(win[i..i + n].iter().sum::<f32>() / n as f32);
    }
    (best.sqrt(), peak)
}

fn main() {
    let dir = std::env::args().nth(1);
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/gm_patches.txt");
    let text = std::fs::read_to_string(path).expect("gm_patches.txt");
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
    let target = 0.09f32;
    let mut out_txt = String::new();
    for (n, body) in &sections {
        let mut p = patch::parse(body);
        let keys = [45u8, 60, 76];
        let mut db = 0.0;
        for k in keys {
            db += (measure(&p, k).0.max(1e-5)).ln();
        }
        let rms = (db / keys.len() as f32).exp();
        let gain = p.params.iter().find(|(k, _)| k == "gain").map(|x| x.1).unwrap_or(0.5);
        let ng = (gain * target / rms).clamp(0.02, 1.0);
        match p.params.iter_mut().find(|(k, _)| k == "gain") {
            Some(e) => e.1 = ng,
            None => p.params.push(("gain".into(), ng)),
        }
        // the loudest note at any pitch must stay clear of clipping
        let peak = [24u8, 45, 60, 84, 100].iter().map(|k| measure(&p, *k).1).fold(0.0, f32::max);
        if peak > 1.0 {
            let ng2 = ng / peak * 0.95;
            p.params.iter_mut().find(|(k, _)| k == "gain").unwrap().1 = ng2;
        }
        let inst = patch::build(&p, SR).unwrap();
        // full state, with this patch's values on top
        let mut st = inst.state();
        for (k, v) in &p.params {
            if let Some(e) = st.iter_mut().find(|(sk, _)| sk == k) {
                e.1 = *v;
            }
        }
        let dumped = patch::dump("synth", &p.name, &st, &p.fx);
        println!("{n:3} {:<24} rms {rms:.3} -> gain {:.2}  peak {peak:.2}", p.name, st.iter().find(|(k, _)| k == "gain").unwrap().1);
        out_txt.push_str(&format!("[{n}]\n{dumped}\n"));
        if let Some(d) = &dir {
            std::fs::create_dir_all(d).unwrap();
            let fname = format!("{d}/{:03} {}.ling", n + 1, p.name.replace('/', "-"));
            std::fs::write(fname, dumped).unwrap();
        }
    }
    std::fs::write(path, out_txt).unwrap();
    let _ = make_instrument; // keep the import used in all cfgs
    let _: Option<Box<dyn Instrument>> = None;
}
