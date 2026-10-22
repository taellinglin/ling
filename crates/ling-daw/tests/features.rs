use ling_daw::dsp::drums::DrumSynth;
use ling_daw::dsp::synth::LingSynth;
use ling_daw::*;

const SR: f32 = 48000.0;

fn synth_track(name: &str, key: u8, len: f64) -> Track {
    let mut t = Track::new(name, TrackKind::Instrument, 0xff8040, Some(Box::new(LingSynth::new(SR))));
    t.inst_kind = "synth".into();
    let mut c = Clip::midi(0.0, len, "n");
    if let ClipData::Midi(n) = &mut c.data {
        n.push(Note::new(0.0, len - 0.2, key, 0.9));
    }
    t.clips.push(c);
    t
}

fn rms(v: &[f32]) -> f32 {
    (v.iter().map(|x| x * x).sum::<f32>() / v.len().max(1) as f32).sqrt()
}

fn render_secs(e: &mut Engine, secs: f32) -> Vec<f32> {
    let mut out = vec![0.0; (secs * SR) as usize * 2];
    for ch in out.chunks_mut(1024) {
        e.render(ch);
    }
    out
}

#[test]
fn send_feeds_target_track() {
    let mut e = Engine::new(SR);
    e.project.bpm = 120.0;
    e.project.tracks.push(synth_track("src", 60, 8.0));
    let mut fxt = Track::new("bus", TrackKind::Instrument, 0x40a0ff, Some(Box::new(LingSynth::new(SR))));
    fxt.inst_kind = "synth".into();
    e.project.tracks.push(fxt);
    e.project.tracks[0].mute = true; // source silent in the mix, but still feeds the send? (muted sources don't send)
    e.project.tracks[0].mute = false;
    e.project.tracks[0].volume = 0.0; // dry level 0 -> only the send path (post-fader is 0 too)
    e.project.tracks[0].volume = 0.8;
    e.project.routes.push(Route { kind: RouteKind::Send, from: 0, to: 1, amount: 1.0, keys: vec![] });
    e.play();
    let out = render_secs(&mut e, 0.5);
    assert!(rms(&out) > 0.01, "send path silent");
    // the bus track's meter shows the received signal
    assert!(e.project.tracks[1].meter[0] > 0.001, "bus received nothing: {:?}", e.project.tracks[1].meter);
}

#[test]
fn cycles_are_rejected_and_indices_follow_moves() {
    let mut p = Project::new();
    for i in 0..3 {
        p.tracks.push(Track::new(&format!("t{i}"), TrackKind::Audio, 0, None));
    }
    p.routes.push(Route { kind: RouteKind::Send, from: 0, to: 1, amount: 1.0, keys: vec![] });
    p.routes.push(Route { kind: RouteKind::Send, from: 1, to: 2, amount: 1.0, keys: vec![] });
    assert!(p.would_cycle(2, 0));
    assert!(!p.would_cycle(0, 2));
    // move track 0 to the end: routes follow
    p.move_track(0, 2);
    assert_eq!((p.routes[0].from, p.routes[0].to), (2, 0));
    assert_eq!((p.routes[1].from, p.routes[1].to), (0, 1));
    p.remove_track(0);
    assert_eq!(p.routes.len(), 0, "routes touching a removed track are dropped ... {:?}", p.routes.len());
}

#[test]
fn sidechain_ducks_target() {
    // pad through a compressor, keyed by a (muted) kick every beat. Compare the level
    // right after a kick with the level between kicks, with and without the route.
    let build = |with_route: bool| {
        let mut e = Engine::new(SR);
        e.project.bpm = 120.0;
        e.project.tracks.push(synth_track("pad", 57, 16.0));
        let mut d = Track::new("kick", TrackKind::Instrument, 0, Some(Box::new(DrumSynth::new(SR))));
        d.inst_kind = "drums".into();
        d.mute = true;
        let mut c = Clip::midi(0.0, 16.0, "k");
        if let ClipData::Midi(n) = &mut c.data {
            for i in 0..8 {
                n.push(Note::new(i as f64 * 2.0, 0.2, 36, 1.0)); // one kick per second
            }
        }
        d.clips.push(c);
        e.project.tracks.push(d);
        let mut comp = EffectSlot::new("Compressor", SR).unwrap();
        comp.set(0, 0.5); // -30 dB threshold
        comp.set(1, 1.0); // 20:1
        comp.set(4, 0.0);
        e.project.tracks[0].effects.push(comp);
        if with_route {
            e.project.routes.push(Route { kind: RouteKind::Sidechain, from: 1, to: 0, amount: 1.0, keys: vec![] });
        }
        e.play();
        let out = render_secs(&mut e, 3.0);
        let win = |a: f32, b: f32| rms(&out[(a * SR) as usize * 2..(b * SR) as usize * 2]);
        // kick at t = 1.0 s
        win(1.02, 1.12) / win(1.6, 1.9)
    };
    let plain = build(false);
    let ducked = build(true);
    assert!(ducked < plain * 0.6, "sidechain had no effect: plain {plain} ducked {ducked}");
}

#[test]
fn session_launch_loops_and_stops() {
    let mut e = Engine::new(SR);
    e.project.bpm = 120.0;
    let mut t = Track::new("s", TrackKind::Instrument, 0, Some(Box::new(LingSynth::new(SR))));
    t.inst_kind = "synth".into();
    let mut c = Clip::midi(0.0, 2.0, "loop");
    if let ClipData::Midi(n) = &mut c.data {
        n.push(Note::new(0.0, 0.5, 60, 0.9));
    }
    t.session.push(c);
    e.project.tracks.push(t);
    e.project.scenes.push("Scene 1".into());
    e.project.set_slot(0, 0, 0);
    e.project.quant = 0.0;
    // nothing plays until launched
    e.play();
    let silent = render_secs(&mut e, 0.3);
    assert!(rms(&silent) < 1e-4);
    e.launch_scene(0);
    let out = render_secs(&mut e, 4.5); // > 2 loops of 2 beats @120bpm = 1 s each
    let sec = |a: f32, b: f32| rms(&out[(a * SR) as usize * 2..(b * SR) as usize * 2]);
    for k in 0..40 {
        eprintln!("{:.1}s rms {:.4}", k as f32 * 0.1, sec(k as f32 * 0.1, k as f32 * 0.1 + 0.1));
    }
    assert!(sec(0.0, 0.3) > 0.005, "first hit silent");
    assert!(sec(1.0, 1.3) > 0.005, "loop did not repeat");
    assert!(sec(2.0, 2.3) > 0.005, "loop did not repeat twice");
    let (playing, _q, prog) = e.sess_state(0);
    assert_eq!(playing, 0);
    assert!((0.0..=1.0).contains(&prog));
    e.stop_track(0);
    let after = render_secs(&mut e, 2.0);
    assert!(rms(&after[(1.0 * SR) as usize * 2..]) < 0.003, "stop did not silence the track");
}

#[test]
fn auto_scenes_from_arrangement() {
    let mut e = Engine::new(SR);
    e.project.tracks.push(synth_track("a", 60, 8.0));
    let mut t2 = synth_track("b", 64, 4.0);
    t2.clips[0].start = 8.0;
    e.project.tracks.push(t2);
    ling_daw::perf::auto_scenes(&mut e.project, 0);
    assert!(e.project.scenes.len() >= 2, "smart slicing found {} scenes", e.project.scenes.len());
    assert!(e.project.slot(0, 0) >= 0);
    assert!(e.project.slot(0, 1) < 0);
    let last = e.project.scenes.len() - 1;
    assert!(e.project.slot(last, 1) >= 0);
}

#[test]
fn automation_lane_moves_volume() {
    let mut e = Engine::new(SR);
    e.project.bpm = 120.0;
    e.project.tracks.push(synth_track("a", 60, 16.0));
    e.project.tracks[0].auto.push(AutoLane { param: "vol".into(), points: vec![(0.0, 0.0), (4.0, 1.0)] });
    e.play();
    let out = render_secs(&mut e, 2.0); // 4 beats
    let early = rms(&out[..(0.3 * SR) as usize * 2]);
    let late = rms(&out[(1.7 * SR) as usize * 2..]);
    assert!(late > early * 3.0, "automation did not ramp: {early} -> {late}");
}
