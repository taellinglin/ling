//! A/B for the per-note expression (pitch gesture / pressure / timbre) work: the instruments it touches and an engine render of
//! plain notes must sound the same as before when no note carries a gesture. `AB_OUT=<file>` writes the reference, `AB_REF=<file>`
//! compares (only float rounding apart). Growl and Prism are covered by perf_ab.rs.
use ling_daw::dsp::Instrument;
use ling_daw::*;

fn plain_inst(kind: &str, sr: f32) -> Box<dyn Instrument> {
    if kind == "sampler" {
        let mut sm = dsp::sampler::Sampler::new(sr);
        // a one-second saw at C4, looped
        let n = sr as usize;
        let mut st = Vec::with_capacity(n * 2);
        for i in 0..n {
            let ph = (i as f32 * 261.63 / sr).fract();
            let v = (ph * 2.0 - 1.0) * 0.5;
            st.extend([v, v]);
        }
        let mut z = dsp::sampler::Zone::new("saw", "");
        z.root = 60;
        z.loop_mode = 1;
        z.loop_end = n as u32;
        sm.add_loaded(z, st, sr as u32);
        return Box::new(sm);
    }
    dsp::make_instrument(kind, sr).unwrap()
}

fn render_all() -> Vec<f32> {
    let sr = 48000.0;
    let mut out = vec![];
    let mut buf = vec![0f32; 512];
    for kind in ["synth", "chip", "pluck", "sampler", "voice", "script"] {
        for glide in [false, true] {
            let mut i = plain_inst(kind, sr);
            if glide {
                for p in ["glide", "poly"] {
                    if i.get_param(p).is_some() {
                        i.set_param(p, if p == "poly" { 0.0 } else { 0.4 });
                    }
                }
            }
            for b in 0..240usize {
                if b % 40 == 0 {
                    for k in [48u8, 55, 60, 64] {
                        i.note_off(k);
                        i.note_on(k, 0.8);
                    }
                }
                if b % 40 == 20 {
                    // legato: the next note before the old one ends
                    i.note_on(67, 0.7);
                    i.note_off(64);
                }
                buf.fill(0.0);
                i.render(&mut buf);
                out.extend_from_slice(&buf);
            }
        }
    }
    // the engine: plain notes on several instruments, overlaps, a looped clip
    let mut e = Engine::new(sr);
    for kind in ["synth", "chip", "pluck", "voice"] {
        let mut t = Track::new(kind, TrackKind::Instrument, 0, Some(dsp::make_instrument(kind, sr).unwrap()));
        t.inst_kind = kind.to_string();
        let mut c = Clip::midi(0.0, 8.0, "c");
        if let ClipData::Midi(n) = &mut c.data {
            for b in 0..8 {
                n.push(Note::new(b as f64 * 0.75, 0.9, 45 + b as u8 * 3, 0.8));
            }
        }
        if kind == "pluck" {
            c.loop_len = 4.0;
        }
        t.clips.push(c);
        e.project.tracks.push(t);
    }
    e.project.bpm = 132.0;
    out.extend(e.bounce(0.5));
    out
}

#[test]
#[ignore]
fn gesture_ab() {
    let v = render_all();
    if let Ok(p) = std::env::var("AB_OUT") {
        let bytes: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
        std::fs::write(p, bytes).unwrap();
    }
    if let Ok(p) = std::env::var("AB_REF") {
        let b = std::fs::read(p).unwrap();
        let r: Vec<f32> = b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        assert_eq!(r.len(), v.len());
        let worst = v.iter().zip(&r).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
        let rms = |x: &[f32]| (x.iter().map(|y| y * y).sum::<f32>() / x.len().max(1) as f32).sqrt();
        eprintln!("signal rms {:.5}, worst difference {:.2e}", rms(&r), worst);
        assert!(worst == 0.0, "notes without gestures must render bit-identically (worst {worst})");
    }
}
