use ling_osdaw::{midi, mix, script, Cell, Engine, Song, CHANNELS, NOTE_OFF, SAMPLE_RATE};

fn render(e: &mut Engine, secs: f32) -> Vec<i16> {
    let n = (secs * SAMPLE_RATE as f32) as usize;
    let mut out = vec![0i16; n * 2];
    let mut p = 0;
    while p < n {
        let b = (n - p).min(1024);
        e.render(&mut out[p * 2..(p + b) * 2]);
        p += b;
    }
    out
}

fn f0(x: &[f32]) -> f32 {
    let (lo, hi) = (SAMPLE_RATE as usize / 1000, SAMPLE_RATE as usize / 60);
    let mut best = (0.0f32, lo);
    for lag in lo..hi.min(x.len() / 2) {
        let (mut c, mut e) = (0.0f32, 1e-9f32);
        for i in 0..x.len() - lag {
            c += x[i] * x[i + lag];
            e += x[i] * x[i];
        }
        if c / e > best.0 {
            best = (c / e, lag);
        }
    }
    SAMPLE_RATE as f32 / best.1 as f32
}

#[test]
fn new_instruments_sound_and_the_pitched_ones_hold_pitch() {
    for inst in [8u8, 9, 10, 11, 12, 13] {
        let mut s = Song::empty();
        s.chans[0].inst = inst;
        s.chans[0].pan = 0;
        s.chans[0].vol = 100;
        let mut e = Engine::new(s);
        e.trigger(0, 57, 15);
        let out = render(&mut e, 0.6);
        let m: Vec<f32> = out.chunks(2).map(|c| c[0] as f32 / 32768.0).collect();
        let peak = m.iter().cloned().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(peak > 0.03, "instrument {inst} audible: {peak}");
        assert!(peak < 1.0);
        if matches!(inst, 8 | 10 | 11) {
            let seg = &m[(0.1 * SAMPLE_RATE as f32) as usize..(0.5 * SAMPLE_RATE as f32) as usize];
            let fr = f0(seg);
            assert!((fr - 220.0).abs() < 14.0 || (fr - 110.0).abs() < 8.0, "instrument {inst} f0 {fr}");
        }
    }
}

fn energy(v: &[i16]) -> f64 {
    v.iter().map(|x| (*x as f64).powi(2)).sum::<f64>()
}

#[test]
fn effects_change_the_sound_and_leave_a_tail() {
    let base = |fx: [u8; 4]| {
        let mut s = Song::empty();
        s.cells[0][5] = Cell { note: 36, vel: 15 };
        s.length = 8;
        s.fx = fx;
        let mut e = Engine::new(s);
        e.looping = false;
        e.play();
        render(&mut e, 2.0)
    };
    let dry = base([0; 4]);
    let wet = base([0, 0, 8, 0]);
    // after the kick has gone (past 0.6 s) the dry mix is silent but the reverb still rings
    let tail = |v: &[i16]| energy(&v[(0.8 * 48000.0) as usize * 2..]);
    assert!(tail(&dry) < 1.0, "dry tail {}", tail(&dry));
    assert!(tail(&wet) > 10_000.0, "reverb tail {}", tail(&wet));
    // the echo repeats the hit 375 ms later
    let echo = base([0, 8, 0, 0]);
    let win = |v: &[i16], a: f32, b: f32| energy(&v[(a * 48000.0) as usize * 2..(b * 48000.0) as usize * 2]);
    assert!(win(&echo, 0.38, 0.45) > win(&dry, 0.38, 0.45) * 2.0 + 1000.0, "echo repeat");
    // drive and crush keep the output inside its ceiling and differ from the dry signal
    for fx in [[10, 0, 0, 0], [0, 0, 0, 10], [10, 10, 10, 10]] {
        let v = base(fx);
        assert!(v.iter().all(|x| (*x as i32).abs() <= 32000));
        assert!(v != dry);
    }
}

#[test]
fn scripts_edit_songs() {
    let mut s = Song::empty();
    s.length = 16;
    s.cells[0][2] = Cell { note: 60, vel: 10 };
    s.cells[8][2] = Cell { note: 65, vel: 10 };
    assert!(script::arpeggiate(&mut s, 2, 0, 0) > 0);
    let k: Vec<u8> = (0..4).map(|r| s.cells[r][2].note).collect();
    assert_eq!(k, [60, 64, 67, 72]);
    assert_eq!(s.cells[8][2].note, 65);
    // row 15 is the 8th step of the second note: 7 % 4 = 3, the octave
    assert_eq!(s.cells[15][2].note, 77);

    // humanize is repeatable and keeps velocities legal
    let (mut a, mut b) = (s.clone(), s.clone());
    script::humanize(&mut a, 2, 4);
    script::humanize(&mut b, 2, 4);
    assert!((0..16).all(|r| a.cells[r][2] == b.cells[r][2]));
    assert!((0..16).all(|r| (1..=15).contains(&a.cells[r][2].vel)));

    // reverse twice is the identity
    let mut r = s.clone();
    script::reverse(&mut r, 2);
    assert_eq!(r.cells[15][2], s.cells[0][2]);
    script::reverse(&mut r, 2);
    assert!((0..16).all(|i| r.cells[i][2] == s.cells[i][2]));

    // euclid(3, 8) is the tresillo: x..x..x.
    let mut e = Song::empty();
    e.length = 16;
    assert_eq!(script::euclid(&mut e, 5, 3, 8, 36, 12), 6);
    let on: Vec<usize> = (0..8).filter(|r| e.cells[*r][5].note > 0).collect();
    assert_eq!(on.len(), 3);
    assert!(on[1] - on[0] >= 2 && on[2] - on[1] >= 2);

    // transpose and snap
    let mut t = Song::empty();
    t.cells[0][0] = Cell { note: 61, vel: 9 }; // C#4 is not in C major
    assert_eq!(script::snap_to_scale(&mut t, 0, 0, false), 1);
    assert!(t.cells[0][0].note == 60 || t.cells[0][0].note == 62);
    script::transpose(&mut t, None, 12);
    assert!(t.cells[0][0].note >= 72);
    script::transpose(&mut t, None, -200);
    assert_eq!(t.cells[0][0].note, 1, "clamped, never wraps");

    // key detection returns a scale that fits most pitched notes of the demo (A minor, C major and F major share the same tones)
    let demo = Song::demo();
    let (root, minor) = script::detect_key(&demo);
    let sc = if minor { script::SCALE_MINOR } else { script::SCALE_MAJOR };
    let (mut fit, mut all) = (0, 0);
    for r in 0..demo.length as usize {
        for c in [1usize, 2, 3, 4] {
            let n = demo.cells[r][c].note;
            if n > 0 && n != NOTE_OFF {
                all += 1;
                fit += sc.iter().any(|t| (root + t) % 12 == n % 12) as usize;
            }
        }
    }
    assert!(fit * 10 >= all * 8, "{fit}/{all} notes fit {root} minor={minor}");
}

#[test]
fn mix_ai_balances_loud_and_quiet_channels() {
    let mut s = Song::demo();
    s.chans[0].mute = false;
    // an absurd imbalance: the hat as loud as it goes, the kick nearly silent
    s.chans[7].vol = 100;
    s.chans[5].vol = 10;
    let r = mix::auto_mix(&mut s);
    assert_eq!(r.roles[5], mix::Role::Kick);
    assert_eq!(r.roles[7], mix::Role::Hat);
    assert_eq!(r.roles[0], mix::Role::Silent);
    // what matters is what comes out: soloed at the new settings the kick is clearly louder than the hat, the bass than the pad
    let solo = |ch: usize| {
        let mut t = s.clone();
        t.fx = [0; 4];
        for (i, c) in t.chans.iter_mut().enumerate() {
            c.mute = i != ch;
        }
        let mut e = Engine::new(t);
        e.master = 1.0;
        e.looping = false;
        e.play();
        let v = render(&mut e, 4.0);
        let l: Vec<f64> = v.chunks(2).map(|c| c[0] as f64 / 32768.0).filter(|x| x.abs() > 0.004).collect();
        (l.iter().map(|x| x * x).sum::<f64>() / l.len().max(1) as f64).sqrt()
    };
    let (k, h, b, p) = (solo(5), solo(7), solo(1), solo(3));
    assert!(k > h * 2.0, "kick {k} over hat {h}");
    assert!(b > p * 1.5, "bass {b} over pad {p}");
    assert!(r.peak > 0.2 && r.peak < 1.0, "mix peak {}", r.peak);
    assert!(s.chans.iter().all(|c| c.vol <= 100 && (-100..=100).contains(&c.pan)));
}

#[test]
fn midi_round_trips_a_song() {
    let s = Song::demo();
    let m = midi::to_midi(&s);
    assert_eq!(&m[0..4], b"MThd");
    let t = midi::from_midi(&m).expect("our own file parses");
    assert_eq!(t.bpm, s.bpm);
    // the kick pattern survives: every 4th row
    let kch = (0..CHANNELS).find(|c| t.chans[*c].inst == 5).expect("a kick lane");
    for r in 0..32 {
        assert_eq!(t.cells[r][kch].note > 0 && t.cells[r][kch].note != NOTE_OFF, r % 4 == 0, "row {r}");
    }
    // the bass line survives with its pitches
    let bch = (0..CHANNELS).find(|c| t.chans[*c].inst == 1).expect("a bass lane");
    assert_eq!(t.cells[0][bch].note, 33);
    assert_eq!(t.cells[8][bch].note, 38);
    assert_eq!(t.cells[16][bch].note, 29);
    // and the result plays
    let mut e = Engine::new(t);
    e.play();
    let out = render(&mut e, 1.0);
    assert!(out.iter().any(|x| x.abs() > 3000));
}

#[test]
fn midi_import_rejects_junk_and_survives_hostile_bytes() {
    assert!(midi::from_midi(b"").is_none());
    assert!(midi::from_midi(b"RIFFxxxxxxxxxxxxxxxx").is_none());
    let good = midi::to_midi(&Song::demo());
    // every truncation and a spray of corrupted bytes: never a panic
    for n in 0..good.len().min(400) {
        let _ = midi::from_midi(&good[..n]);
    }
    let mut x = 0x1234_5678u32;
    for _ in 0..300 {
        let mut b = good.clone();
        for _ in 0..8 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let i = (x as usize) % b.len();
            b[i] = (x >> 8) as u8;
        }
        if let Some(s) = midi::from_midi(&b) {
            assert!(s.length >= 8 && s.length as usize <= 64 && (40..=240).contains(&s.bpm));
        }
    }
}

#[test]
fn songs_with_effects_round_trip_and_old_files_still_load() {
    let s = Song::demo();
    let b = s.to_bytes();
    assert_eq!(&b[0..4], b"LSX2");
    assert_eq!(Song::from_bytes(&b).unwrap().fx, s.fx);
    // a version-1 file (no effect bytes) loads with the rack off
    let mut old = b[..b.len() - 4].to_vec();
    old[3] = b'1';
    let o = Song::from_bytes(&old).unwrap();
    assert_eq!(o.fx, [0; 4]);
    assert_eq!(o.cells[0][5], s.cells[0][5]);
}
