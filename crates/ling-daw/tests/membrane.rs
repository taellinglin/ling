use ling_daw::dsp::drums::DrumSynth;
use ling_daw::dsp::Instrument;

const SR: f32 = 48000.0;
// part indices: Kick 0, Snare 1, Tom 2, Closed Hat 3, Open Hat 4, Crash 5, Ride 6, Clap 7

fn hit(setup: &dyn Fn(&mut DrumSynth), part: usize, key: u8, vel: f32, secs: f32) -> Vec<f32> {
    let mut d = DrumSynth::new(SR);
    setup(&mut d);
    d.select_part(part);
    d.note_on(key, vel);
    let mut out = vec![0f32; (secs * SR) as usize * 2];
    for c in out.chunks_mut(1024) {
        d.render(c);
    }
    out
}

fn set(d: &mut DrumSynth, part: usize, name: &str, v: f32) {
    d.select_part(part);
    d.set_param(name, v);
}

fn energy(v: &[f32]) -> f32 {
    v.iter().step_by(2).map(|x| x * x).sum()
}
fn brightness(v: &[f32]) -> f32 {
    // the share of the energy that is in the first difference: a rising ratio is a brighter sound
    let l: Vec<f32> = v.iter().step_by(2).copied().collect();
    let d: f32 = l.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum();
    d / energy(v).max(1e-9)
}

#[test]
fn the_new_parameters_are_listed_with_neutral_defaults() {
    let d = DrumSynth::new(SR);
    let names: Vec<&str> = d.params().iter().map(|p| p.0).collect();
    for n in ["strike", "shell", "wires", "tail", "human", "veltune", "dynamics", "fx filter", "fx cutoff", "fx reso", "fx drive", "fx crush", "fx rate", "fx punch", "fx squash", "fx echo", "fx time", "fx fb", "fx verb", "fx size", "fx widen"] {
        assert!(names.contains(&n), "missing {n}");
    }
    assert!(names.len() >= 37, "{}", names.len());
    let v = |n: &str| d.params().iter().find(|p| p.0 == n).unwrap().1;
    assert_eq!((v("strike"), v("wires"), v("tail"), v("dynamics"), v("fx filter"), v("fx punch"), v("fx widen")), (0.5, 0.5, 0.5, 0.5, 0.0, 0.5, 0.5));
}

#[test]
fn strike_moves_from_a_dark_centre_to_a_bright_edge() {
    let centre = hit(&|d| set(d, 2, "strike", 0.0), 2, 45, 0.9, 0.5);
    let edge = hit(&|d| set(d, 2, "strike", 1.0), 2, 45, 0.9, 0.5);
    assert!(brightness(&edge) > brightness(&centre) * 1.2, "{} vs {}", brightness(&edge), brightness(&centre));
}

#[test]
fn shell_wires_tail_and_dynamics_do_what_they_say() {
    // the shell adds a resonance: different, and more energy in the low part of a tom
    let dry = hit(&|_| {}, 2, 45, 0.9, 0.6);
    let shell = hit(&|d| set(d, 2, "shell", 1.0), 2, 45, 0.9, 0.6);
    assert!(energy(&shell) > energy(&dry) * 1.05);
    // wires: a snare with the wires up is noisier (brighter) than with them down
    let w0 = hit(&|d| set(d, 1, "wires", 0.0), 1, 38, 0.9, 0.4);
    let w1 = hit(&|d| set(d, 1, "wires", 1.0), 1, 38, 0.9, 0.4);
    assert!(brightness(&w1) > brightness(&w0) * 1.3, "{} vs {}", brightness(&w1), brightness(&w0));
    // tail: an open hat rings longer with the tail up
    let late = |v: &[f32]| energy(&v[(0.45 * SR) as usize * 2..]);
    let t0 = hit(&|d| set(d, 4, "tail", 0.0), 4, 46, 0.9, 1.2);
    let t1 = hit(&|d| set(d, 4, "tail", 1.0), 4, 46, 0.9, 1.2);
    assert!(late(&t1) > late(&t0) * 3.0 + 1e-6, "{} vs {}", late(&t1), late(&t0));
    // dynamics: a soft hit keeps more of its level when the response is flat
    let flat = hit(&|d| set(d, 0, "dynamics", 0.0), 0, 36, 0.3, 0.3);
    let wide = hit(&|d| set(d, 0, "dynamics", 1.0), 0, 36, 0.3, 0.3);
    assert!(energy(&flat) > energy(&wide) * 2.0);
}

#[test]
fn human_and_velocity_pitch() {
    // the pitch of a hit, from its zero crossings over 0.3 s (a pure-sine tom, so the click does not matter)
    let zc = |v: &[f32]| v.iter().step_by(2).take(14400).collect::<Vec<_>>().windows(2).filter(|w| *w[0] <= 0.0 && *w[1] > 0.0).count();
    // no humanising: every hit of one note has the same pitch; with it the pitch wanders
    let eight = |h: f32| {
        let mut d = DrumSynth::new(SR);
        set(&mut d, 2, "human", h);
        set(&mut d, 2, "purity", 1.0);
        set(&mut d, 2, "tension", 0.0);
        let mut counts = vec![];
        for _ in 0..8 {
            d.all_off();
            d.note_on(45, 0.8);
            let mut o = vec![0f32; 28800];
            d.render(&mut o);
            counts.push(zc(&o));
        }
        counts
    };
    let still = eight(0.0);
    let spread = |v: &[usize]| v.iter().max().unwrap() - v.iter().min().unwrap();
    assert!(spread(&still) <= 1, "human 0 is repeatable (a crossing of jitter at most): {still:?}");
    let wander = eight(1.0);
    assert!(spread(&wander) >= 3, "human 1 varies: {wander:?}");
    // velocity to pitch: a hard hit is higher than a soft one when veltune is up
    let zc = |v: &[f32]| v.iter().step_by(2).take(9600).collect::<Vec<_>>().windows(2).filter(|w| *w[0] <= 0.0 && *w[1] > 0.0).count();
    let soft = hit(&|d| set(d, 2, "veltune", 1.0), 2, 45, 0.2, 0.3);
    let hard = hit(&|d| set(d, 2, "veltune", 1.0), 2, 45, 1.0, 0.3);
    assert!(zc(&hard) > zc(&soft), "{} vs {}", zc(&hard), zc(&soft));
}

#[test]
fn each_piece_has_its_own_effects() {
    // a low-pass on the closed hat only: the hat loses its highs, the kick is bit-for-bit the same
    let setup = |d: &mut DrumSynth| {
        set(d, 3, "fx filter", 0.4);
        set(d, 3, "fx cutoff", 0.25);
    };
    let hat_dry = hit(&|_| {}, 3, 42, 0.9, 0.3);
    let hat_lp = hit(&setup, 3, 42, 0.9, 0.3);
    assert!(brightness(&hat_lp) < brightness(&hat_dry) * 0.3, "{} vs {}", brightness(&hat_lp), brightness(&hat_dry));
    let kick_dry = hit(&|_| {}, 0, 36, 0.9, 0.3);
    let kick_other = hit(&setup, 0, 36, 0.9, 0.3);
    assert!(kick_dry == kick_other, "the kick has no rack");
    // an echo on the snare rings after the hit has died (a snare is ~0.3 s)
    let tail = |v: &[f32]| energy(&v[(0.9 * SR) as usize * 2..]);
    let e0 = hit(&|_| {}, 1, 38, 0.9, 1.4);
    let e1 = hit(&|d| { set(d, 1, "fx echo", 0.7); set(d, 1, "fx time", 0.5); set(d, 1, "fx fb", 0.6) }, 1, 38, 0.9, 1.4);
    assert!(tail(&e1) > tail(&e0) * 3.0 + 1e-6, "{} vs {}", tail(&e1), tail(&e0));
    // every effect together on every piece never produces a NaN or runaway level
    let all = |d: &mut DrumSynth| {
        for p in 0..8 {
            for n in ["fx drive", "fx crush", "fx rate", "fx punch", "fx squash", "fx echo", "fx verb", "fx widen"] {
                set(d, p, n, 1.0);
            }
            set(d, p, "fx filter", 0.9);
            set(d, p, "fx reso", 1.0);
        }
    };
    for (part, key) in [(0usize, 36u8), (1, 38), (4, 46), (7, 39)] {
        let o = hit(&all, part, key, 1.0, 0.6);
        assert!(o.iter().all(|x| x.is_finite() && x.abs() < 40.0), "part {part}");
    }
}

#[test]
fn settings_survive_a_save() {
    let mut d = DrumSynth::new(SR);
    set(&mut d, 1, "strike", 0.9);
    set(&mut d, 1, "wires", 0.2);
    set(&mut d, 1, "fx drive", 0.6);
    set(&mut d, 1, "fx echo", 0.4);
    let st = d.state();
    // the rack writes only the knobs that moved
    let fx_keys: Vec<&str> = st.iter().map(|(k, _)| k.as_str()).filter(|k| k.contains(".fx ")).collect();
    assert_eq!(fx_keys.len(), 2, "{fx_keys:?}");
    let mut e = DrumSynth::new(SR);
    e.load_state(&st);
    e.select_part(1);
    let v = |n: &str| e.params().iter().find(|p| p.0 == n).unwrap().1;
    assert!((v("strike") - 0.9).abs() < 1e-6 && (v("wires") - 0.2).abs() < 1e-6 && (v("fx drive") - 0.6).abs() < 1e-6 && (v("fx echo") - 0.4).abs() < 1e-6);
    // an old project (no new keys) loads to the neutral defaults
    let old: Vec<(String, f32)> = st.into_iter().filter(|(k, _)| k.starts_with("kick.tune") || k.starts_with("snare.tune")).collect();
    let mut f = DrumSynth::new(SR);
    f.load_state(&old);
    f.select_part(1);
    assert_eq!(f.params().iter().find(|p| p.0 == "strike").unwrap().1, 0.5);
}

// ── character controls and the kit bus ──────────────────────────────────────

/// energy below ~120 Hz (a one-pole low-pass of the left channel)
fn low_energy(v: &[f32]) -> f32 {
    let a = 1.0 - (-std::f32::consts::TAU * 120.0 / SR).exp();
    let mut y = 0.0f32;
    v.iter().step_by(2).map(|x| {
        y += a * (x - y);
        y * y
    }).sum()
}

#[test]
fn character_knobs_are_neutral_at_zero_and_do_what_they_say() {
    let dry = hit(&|_| {}, 0, 36, 0.9, 0.6);
    // every new knob at 0 and the bus untouched: the kick is bit-for-bit what it was
    let zero = hit(&|d| {
        for n in ["sub", "snap", "ring", "fold", "vary", "bus duck", "bus room", "bus glue", "bus smash", "bus tape"] {
            set(d, 0, n, 0.0);
        }
        set(d, 0, "bus boom", 0.5);
        set(d, 0, "bus air", 0.5);
    }, 0, 36, 0.9, 0.6);
    assert!(dry == zero, "neutral knobs change nothing");
    let sub = hit(&|d| set(d, 0, "sub", 1.0), 0, 36, 0.9, 0.6);
    assert!(low_energy(&sub) > low_energy(&dry) * 1.3, "sub adds low end: {} vs {}", low_energy(&sub), low_energy(&dry));
    let first = |v: &[f32]| v[..(0.01 * SR) as usize * 2].to_vec();
    // snap: more high-frequency energy in the attack (first difference energy of the first 10 ms)
    let hf = |v: &[f32]| brightness(v) * energy(v);
    let soft = hit(&|_| {}, 0, 36, 0.5, 0.6);
    let snap = hit(&|d| set(d, 0, "snap", 1.0), 0, 36, 0.5, 0.6);
    assert!(hf(&first(&snap)) > hf(&first(&soft)) * 1.3, "snap adds attack: {} vs {}", hf(&first(&snap)), hf(&first(&soft)));
    let ring = hit(&|d| set(d, 2, "ring", 1.0), 2, 45, 0.9, 0.5);
    let tom = hit(&|_| {}, 2, 45, 0.9, 0.5);
    assert!(brightness(&ring) > brightness(&tom) * 1.2, "ring adds sidebands: {} vs {}", brightness(&ring), brightness(&tom));
    let fold = hit(&|d| set(d, 0, "fold", 1.0), 0, 36, 1.0, 0.4);
    assert!(brightness(&fold) > brightness(&hit(&|_| {}, 0, 36, 1.0, 0.4)) * 1.3, "fold makes it harder");
    // vary: repeated hits differ; without it they are identical
    let mut d = DrumSynth::new(SR);
    d.select_part(1);
    d.set_param("vary", 1.0);
    d.set_param("human", 0.0);
    let mut hits = vec![];
    for _ in 0..4 {
        d.all_off();
        d.note_on(38, 0.8);
        let mut o = vec![0f32; (0.2 * SR) as usize * 2];
        d.render(&mut o);
        hits.push(energy(&o));
    }
    let spread = hits.iter().cloned().fold(f32::MIN, f32::max) / hits.iter().cloned().fold(f32::MAX, f32::min);
    assert!(spread > 1.03, "vary changes the hits: {hits:?}");
}

#[test]
fn the_kit_bus_works_on_the_whole_kit() {
    // kick and hat together, with and without duck: the kick pushes the hat down
    let both = |setup: &dyn Fn(&mut DrumSynth)| {
        let mut d = DrumSynth::new(SR);
        setup(&mut d);
        d.note_on(36, 1.0);
        d.note_on(46, 0.8);
        let mut o = vec![0f32; (0.5 * SR) as usize * 2];
        for c in o.chunks_mut(1024) {
            d.render(c);
        }
        o
    };
    let hat_only = |setup: &dyn Fn(&mut DrumSynth)| {
        let mut d = DrumSynth::new(SR);
        setup(&mut d);
        d.note_on(46, 0.8);
        let mut o = vec![0f32; (0.5 * SR) as usize * 2];
        for c in o.chunks_mut(1024) {
            d.render(c);
        }
        o
    };
    let ducked = both(&|d| d.set_param("bus duck", 1.0));
    let plain = both(&|_| {});
    // the hat part is what is left after the kick: compare the bright part of the mix
    assert!(brightness(&ducked) * energy(&ducked) < brightness(&plain) * energy(&plain) * 0.85, "duck lowers the rest of the kit");
    assert!(hat_only(&|d| d.set_param("bus duck", 1.0)) == hat_only(&|_| {}), "no kick, no ducking");
    // glue and smash even the level out: the tail gets louder against the peak
    let tail_ratio = |v: &[f32]| {
        let pk = v.iter().cloned().fold(0.0f32, |m, x| m.max(x.abs()));
        let t: f32 = v[(0.15 * SR) as usize * 2..].iter().map(|x| x.abs()).sum::<f32>() / (v.len() as f32 / 2.0);
        t / pk.max(1e-9)
    };
    let sm = both(&|d| d.set_param("bus smash", 1.0));
    assert!(tail_ratio(&sm) > tail_ratio(&plain) * 1.2, "smash brings the tail up");
    // glue narrows the range between a soft and a hard hit
    let kick_at = |vel: f32, glue: f32| {
        let o = hit(&|d| d.set_param("bus glue", glue), 0, 36, vel, 0.3);
        energy(&o)
    };
    let range = |g: f32| kick_at(1.0, g) / kick_at(0.35, g);
    assert!(range(1.0) < range(0.0) * 0.8, "glue: {} vs {}", range(1.0), range(0.0));
    let gl = both(&|d| d.set_param("bus glue", 1.0));
    let room = both(&|d| d.set_param("bus room", 1.0));
    let late = |v: &[f32]| energy(&v[(0.35 * SR) as usize * 2..]);
    assert!(late(&room) > late(&plain) * 1.5, "the room rings on");
    let boom = both(&|d| d.set_param("bus boom", 1.0));
    assert!(low_energy(&boom) > low_energy(&plain) * 1.5, "boom lifts the low end");
    let air = both(&|d| d.set_param("bus air", 1.0));
    assert!(brightness(&air) > brightness(&plain) * 1.2, "air lifts the top");
    for o in [&ducked, &sm, &gl, &room, &boom, &air, &both(&|d| d.set_param("bus tape", 1.0))] {
        assert!(o.iter().all(|x| x.is_finite() && x.abs() < 40.0));
    }
    // the bus is saved and loaded with the kit, from any piece
    let mut d = DrumSynth::new(SR);
    d.select_part(3);
    d.set_param("bus glue", 0.7);
    d.set_param("sub", 0.4);
    let st = d.state();
    assert!(st.iter().any(|(k, v)| k == "bus.glue" && (*v - 0.7).abs() < 1e-6));
    let mut e = DrumSynth::new(SR);
    e.load_state(&st);
    e.select_part(0);
    let v = |n: &str| e.params().iter().find(|p| p.0 == n).unwrap().1;
    assert!((v("bus glue") - 0.7).abs() < 1e-6, "the bus is the same on every piece");
    e.select_part(3);
    assert!((e.params().iter().find(|p| p.0 == "sub").unwrap().1 - 0.4).abs() < 1e-6);
}
