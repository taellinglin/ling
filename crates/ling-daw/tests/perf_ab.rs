//! A/B for optimizations: renders a fixed drum pattern and a few patches to a raw file (`AB_OUT`), or compares against one
//! (`AB_REF`): the optimized code must sound the same (only float rounding apart).
use ling_daw::*;

fn render_all() -> Vec<f32> {
    let sr = 48000.0;
    let mut out = vec![];
    let mut kit = dsp::make_instrument("drums", sr).unwrap();
    let mut buf = vec![0f32; 512];
    let keys = [36u8, 42, 38, 42, 36, 46, 39, 49, 41, 45, 56, 51, 54, 37, 40, 75];
    for b in 0..1500usize {
        if b % 12 == 0 {
            kit.note_on(keys[(b / 12) % keys.len()], 0.9);
        }
        buf.fill(0.0);
        kit.render(&mut buf);
        out.extend_from_slice(&buf);
    }
    for kind in ["synth", "growl", "prism", "chip", "pluck"] {
        let mut i = dsp::make_instrument(kind, sr).unwrap();
        for b in 0..400usize {
            if b % 60 == 0 {
                for k in [48u8, 55, 60, 64] {
                    i.note_off(k);
                    i.note_on(k, 0.8);
                }
            }
            buf.fill(0.0);
            i.render(&mut buf);
            out.extend_from_slice(&buf);
        }
    }
    // existing preset files of Prism and Growl (a spread of the library), when the LingStationX folder is there
    let root = std::env::var("LSX_ROOT").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../../LingStationX").into());
    for (kind, names) in [
        ("Prism", vec!["Prism_Festival_Supersaw", "Prism_Future_Bass_Chord", "Prism_Hoover_Dark", "Prism_Rolling_Bass", "Prism_Warm_Sub", "Prism_Supersaw_Lead", "Prism_Amber_Pad", "Prism_Neuro_Bass"]),
        ("Growl", vec!["Growl_neuro_reese", "Growl_riddim_fold", "Growl_comb_growl", "Growl_kick_bass", "Growl_big_room_lead", "Growl_flanged_reese"]),
    ] {
        for n in names {
            let path = format!("{root}/instruments/{kind}/{n}.ling");
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let Some(mut i) = patch::build(&patch::parse(&text), sr) else { continue };
            for b in 0..400usize {
                if b % 60 == 0 {
                    for k in [48u8, 55, 60, 64] {
                        i.note_off(k);
                        i.note_on(k, 0.8);
                    }
                }
                buf.fill(0.0);
                i.render(&mut buf);
                out.extend_from_slice(&buf);
            }
        }
    }
    out
}

#[test]
#[ignore]
fn ab() {
    let v = render_all();
    if let Ok(p) = std::env::var("AB_OUT") {
        let bytes: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
        std::fs::write(p, bytes).unwrap();
    }
    if let Ok(p) = std::env::var("AB_REF") {
        let b = std::fs::read(p).unwrap();
        let r: Vec<f32> = b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        assert_eq!(r.len(), v.len());
        let rms = |x: &[f32]| (x.iter().map(|y| y * y).sum::<f32>() / x.len().max(1) as f32).sqrt();
        let n = 1500 * 512;
        let parts = [("drums", 0, n), ("instruments", n, v.len())];
        for (name, a, b) in parts {
            let d: Vec<f32> = v[a..b].iter().zip(&r[a..b]).map(|(x, y)| x - y).collect();
            let worst = d.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            eprintln!("{name}: signal rms {:.5}, difference rms {:.2e}, worst {:.2e}", rms(&r[a..b]), rms(&d), worst);
            assert!(rms(&d) < rms(&r[a..b]) * 1e-3, "{name} sounds different");
        }
    }
}
