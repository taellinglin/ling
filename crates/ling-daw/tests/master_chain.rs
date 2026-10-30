//! The master channel's effect chain: Ling Warp and Ling Master on the master, in order, bypassable, saved with the project,
//! following the transport and keeping the ceiling in a real render.
use ling_daw::dsp::synth::LingSynth;
use ling_daw::*;
use std::collections::HashMap;

const SR: f32 = 48000.0;

fn song() -> Engine {
    let mut e = Engine::new(SR);
    e.project.bpm = 128.0;
    let mut syn = Track::new("Synth", TrackKind::Instrument, 0xff8040, Some(Box::new(LingSynth::new(SR))));
    let mut c = Clip::midi(0.0, 8.0, "riff");
    if let ClipData::Midi(n) = &mut c.data {
        for i in 0..16 {
            n.push(Note::new(i as f64 * 0.5, 0.45, 48 + (i % 4) as u8 * 5, 1.0));
        }
    }
    syn.clips.push(c);
    syn.volume = 1.0;
    e.project.tracks.push(syn);
    e
}

fn param(s: &EffectSlot, n: &str) -> f32 {
    s.params.iter().find(|p| p.0 == n).unwrap().1
}

#[test]
fn the_master_chain_is_ordered_bypassable_and_saved() {
    let mut e = song();
    for name in ["Compressor", "Ling Warp", "Ling Master"] {
        e.project.master_effects.push(EffectSlot::new(name, SR).unwrap());
    }
    // reorder: the warp first, the master last; bypass the compressor
    let w = e.project.master_effects.remove(1);
    e.project.master_effects.insert(0, w);
    e.project.master_effects[1].bypass = true;
    // a drawn time curve on the warp and a preset-like setting on the master
    let i = e.project.master_effects[0].params.iter().position(|p| p.0 == "s1 time").unwrap();
    e.project.master_effects[0].set(i, 0.99);
    e.project.master_effects[0].fx.command("tpoint", &[20.0, -0.4], "");
    let k = e.project.master_effects[2].params.iter().position(|p| p.0 == "push").unwrap();
    e.project.master_effects[2].set(k, 0.25);
    let doc = project::to_doc(&e);
    let text = serde_json::to_string(&doc).unwrap();
    let back: project::ProjectDoc = serde_json::from_str(&text).unwrap();
    let mut e2 = Engine::new(SR);
    project::apply_doc(&mut e2, &back, None, &mut HashMap::new());
    let names: Vec<&str> = e2.project.master_effects.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Ling Warp", "Compressor", "Ling Master"]);
    assert!(e2.project.master_effects[1].bypass && !e2.project.master_effects[0].bypass);
    assert!((param(&e2.project.master_effects[2], "push") - 0.25).abs() < 1e-6);
    assert!((param(&e2.project.master_effects[0], "s1 time") - 0.99).abs() < 1e-6);
    let cfg = e2.project.master_effects[0].fx.info("config");
    assert!(cfg.contains("-0.4"), "the drawn curve came back: {cfg}");
}

#[test]
fn a_render_through_the_master_chain_follows_the_tempo_and_keeps_the_ceiling() {
    let mut e = song();
    e.project.master_volume = 1.0;
    let mut lm = EffectSlot::new("Ling Master", SR).unwrap();
    let k = lm.params.iter().position(|p| p.0 == "push").unwrap();
    lm.set(k, 0.75); // +18 dB into the limiter
    let mut gate = EffectSlot::new("Ling Warp", SR).unwrap();
    // slot 1: straight playback with the trance gate
    let i = gate.params.iter().position(|p| p.0 == "s1 time").unwrap();
    gate.set(i, 0.5 / 25.0);
    let j = gate.params.iter().position(|p| p.0 == "s1 gate").unwrap();
    gate.set(j, 7.5 / 17.0);
    e.project.master_effects.push(gate);
    e.project.master_effects.push(lm);
    let out = e.bounce(0.5);
    assert!(out.iter().all(|x| x.is_finite()));
    let pk = out.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(pk <= 10f32.powf(-1.0 / 20.0) + 1e-4, "sample peak {pk} over -1 dBFS");
    // the trance gate is closed on step 2 of every bar (beat 0.25 - 0.5): that 16th is far quieter than step 1
    let spb = 60.0 / 128.0 * SR;
    let rms = |b0: f32, b1: f32| {
        let s = &out[(b0 * spb) as usize * 2..(b1 * spb) as usize * 2];
        (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt()
    };
    for bar in 0..2 {
        let b = bar as f32 * 4.0;
        let (open, shut) = (rms(b + 0.05, b + 0.2), rms(b + 0.3, b + 0.45));
        assert!(shut < open * 0.2, "bar {bar}: gate closed {shut} vs open {open}");
    }
}
