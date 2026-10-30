//! Quality checks over every shipped sound recipe (assets/recipes): each one is applied to a track, a four-bar phrase is written
//! in its register by its own composition block over A minor (Am F C G) and rendered through its instrument and effect chain at
//! its level. The render must be finite, audible, peak under -1 dBFS, carry no DC, be mono below the recipe's mono corner, and
//! put its energy where its role wants it (a sub below 120 Hz, a pad above 150 Hz ...). Pitched recipes must also pass the tune
//! check within 15 cents. Set LING_RECIPE_DEMOS=<folder> to write each render there as a WAV.
use ling_daw::compose::Kind;
use ling_daw::model::{Clip, ClipData, Track, TrackKind};
use ling_daw::recipes::{self, Recipe};
use ling_daw::sounddesign::{Phase, Sec};
use ling_daw::theory::{Chord, Key};
use ling_daw::Engine;

const SR: f32 = 44100.0;

fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * std::f64::consts::PI / len as f64;
        for s in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (c, si) = ((ang * k as f64).cos(), (ang * k as f64).sin());
                let (a, b) = (s + k, s + k + len / 2);
                let (xr, xi) = (re[b] * c - im[b] * si, re[b] * si + im[b] * c);
                re[b] = re[a] - xr;
                im[b] = im[a] - xi;
                re[a] += xr;
                im[a] += xi;
            }
        }
        len <<= 1;
    }
}

/// power per frequency bin (Hann, 8192-point frames, half overlap), and the bin width in Hz
fn spectrum(x: &[f32]) -> (Vec<f64>, f64) {
    const N: usize = 8192;
    let mut acc = vec![0f64; N / 2];
    let win: Vec<f64> = (0..N).map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / N as f64).cos()).collect();
    let mut s = 0;
    while s + N <= x.len() {
        let mut re: Vec<f64> = (0..N).map(|i| x[s + i] as f64 * win[i]).collect();
        let mut im = vec![0f64; N];
        fft(&mut re, &mut im);
        for k in 0..N / 2 {
            acc[k] += re[k] * re[k] + im[k] * im[k];
        }
        s += N / 2;
    }
    (acc, SR as f64 / N as f64)
}

fn band(spec: &(Vec<f64>, f64), lo: f64, hi: f64) -> f64 {
    let tot: f64 = spec.0.iter().skip(1).sum::<f64>().max(1e-30);
    spec.0.iter().enumerate().skip(1).filter(|(k, _)| { let f = *k as f64 * spec.1; f >= lo && f < hi }).map(|(_, p)| p).sum::<f64>() / tot
}

/// a 4th-order low-pass (two RBJ biquads)
#[allow(dead_code)]
fn lowpass(x: &[f32], hz: f32) -> Vec<f32> {
    let w = 2.0 * std::f32::consts::PI * hz / SR;
    let q = std::f32::consts::FRAC_1_SQRT_2;
    let alpha = w.sin() / (2.0 * q);
    let cw = w.cos();
    let (b0, b1, b2, a0, a1, a2) = ((1.0 - cw) / 2.0, 1.0 - cw, (1.0 - cw) / 2.0, 1.0 + alpha, -2.0 * cw, 1.0 - alpha);
    let mut y = x.to_vec();
    for _ in 0..2 {
        let (mut x1, mut x2, mut y1, mut y2) = (0f32, 0f32, 0f32, 0f32);
        for v in y.iter_mut() {
            let x0 = *v;
            let o = (b0 * x0 + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2) / a0;
            x2 = x1;
            x1 = x0;
            y2 = y1;
            y1 = o;
            *v = o;
        }
    }
    y
}

fn am_chords() -> Vec<Chord> {
    [(9u8, 1usize), (5, 0), (0, 0), (7, 0)].iter().enumerate().map(|(i, (r, q))| Chord { start: i as f64 * 4.0, len: 4.0, root: *r, q: *q, bass: *r, implied: false }).collect()
}

/// what a composer would have written before the recipe (a lead melody, a bass rhythm on the roots)
fn source_for(r: &Recipe) -> Vec<ling_daw::model::Note> {
    use ling_daw::model::Note;
    if r.composition.mode == "melody" {
        let line = [(0.0, 1.0, 69u8), (1.0, 0.5, 72), (1.5, 0.5, 71), (2.0, 2.0, 69), (4.0, 1.0, 72), (5.0, 1.0, 69), (6.0, 2.0, 65), (8.0, 1.5, 67), (9.5, 0.5, 69), (10.0, 2.0, 72), (12.0, 1.0, 74), (13.0, 1.0, 72), (14.0, 2.0, 71)];
        return line.iter().map(|(s, l, k)| Note::new(*s, *l, *k, 0.85)).collect();
    }
    let roots = [45u8, 41, 48, 43];
    (0..32).map(|i| Note::new(i as f64 * 0.5, 0.45, roots[i / 8], if i % 2 == 0 { 0.95 } else { 0.75 })).collect()
}

struct Report {
    id: String,
    peak_db: f32,
    rms_db: f32,
    dc: f32,
    cents: Option<f32>,
    side_lo_db: Option<f32>,
    sub: f64,
    low: f64,
    mid: f64,
    high: f64,
    fails: Vec<String>,
}

fn render(r: &Recipe) -> (Vec<f32>, Report) {
    let genre = r.genres.first().cloned().unwrap_or_else(|| "dubstep".into());
    let bpm = ling_daw::songgen::genre(&genre).bpm;
    let riser = r.riser();
    let tag = if riser { "build" } else { "drop" };
    let secs = vec![Sec { tag: tag.into(), start: 0.0, end: 16.0 }];
    let mut e = Engine::new(SR);
    e.project.bpm = bpm;
    e.project.limiter = false;
    e.project.master_volume = 1.0;
    let mut t = Track::new(&r.name, TrackKind::Instrument, 0, None);
    recipes::apply_sound(&mut t, r, &recipes::Ctx { genre: genre.clone(), bpm, secs: secs.clone(), sr: SR, role: String::new() });
    let chords = am_chords();
    let src = source_for(r);
    let w = recipes::write(r, &recipes::WriteCtx { len: 16.0, chords: &chords, key: Key::new(9, true), phase: if riser { Phase::Build } else { Phase::Drop }, energy: 0.9, seed: 3, source: &src, bar0: 0 });
    let mut c = Clip::midi(0.0, 16.0, "phrase");
    c.data = ClipData::Midi(w.notes.iter().filter(|(m, _)| *m == 0).map(|(_, n)| n.clone()).collect());
    t.clips.push(c);
    for (p, pts) in &w.lanes {
        t.auto.retain(|l| l.param != *p);
        t.auto.push(ling_daw::model::AutoLane { param: p.clone(), points: pts.clone() });
    }
    let x = recipes::expression(r, &genre, bpm);
    ling_daw::sounddesign::apply_expression(&mut t, &x);
    e.project.tracks.push(t);
    let buf = e.bounce_range(0.0, 16.0, 1.5);
    let (l, rr): (Vec<f32>, Vec<f32>) = (buf.iter().step_by(2).copied().collect(), buf.iter().skip(1).step_by(2).copied().collect());
    let mut fails = vec![];
    let finite = buf.iter().all(|v| v.is_finite());
    if !finite {
        fails.push("not finite".to_string());
    }
    let peak = buf.iter().fold(0f32, |a, v| a.max(v.abs()));
    let rms = (buf.iter().map(|v| v * v).sum::<f32>() / buf.len().max(1) as f32).sqrt();
    let db = |v: f32| 20.0 * v.max(1e-9).log10();
    if peak > 10f32.powf(-1.0 / 20.0) {
        fails.push(format!("peak {:.1} dBFS", db(peak)));
    }
    if rms < 1e-3 {
        fails.push(format!("silent ({:.1} dB rms)", db(rms)));
    }
    let dc = (l.iter().sum::<f32>() / l.len() as f32).abs().max((rr.iter().sum::<f32>() / rr.len() as f32).abs());
    if dc > 0.005 {
        fails.push(format!("DC {dc:.4}"));
    }
    // mono below the recipe's corner: the side's lows against the mid's (judged when there are lows to judge)
    let mut side_lo = None;
    if r.mix.mono_below_hz > 20.0 {
        let corner = (r.mix.mono_below_hz * 0.7) as f64;
        let m: Vec<f32> = l.iter().zip(&rr).map(|(a, b)| 0.5 * (a + b)).collect();
        let s: Vec<f32> = l.iter().zip(&rr).map(|(a, b)| 0.5 * (a - b)).collect();
        let (ms, ss) = (spectrum(&m), spectrum(&s));
        let below = |sp: &(Vec<f64>, f64)| sp.0.iter().enumerate().skip(1).filter(|(k, _)| (*k as f64 * sp.1) < corner).map(|(_, p)| p).sum::<f64>();
        let (em, es) = (below(&ms), below(&ss));
        let total: f64 = ms.0.iter().skip(1).sum();
        if em > total * 0.01 {
            let ratio = (10.0 * (es.max(1e-30) / em).log10()) as f32;
            side_lo = Some(ratio);
            if ratio > -20.0 {
                fails.push(format!("side below {corner:.0} Hz only {ratio:.1} dB under the mid"));
            }
        }
    }
    let mono: Vec<f32> = l.iter().zip(&rr).map(|(a, b)| 0.5 * (a + b)).collect();
    let sp = spectrum(&mono);
    let (sub, low, mid, high) = (band(&sp, 0.0, 120.0), band(&sp, 0.0, 150.0), band(&sp, 150.0, 5000.0), band(&sp, 2000.0, 20000.0));
    let impact = r.has_tag("impact");
    match r.role.as_str() {
        _ if r.has_tag("sub") || r.has_tag("808") => {
            if sub < 0.5 {
                fails.push(format!("a sub with only {:.0}% below 120 Hz", sub * 100.0));
            }
        }
        "pad" if low > 0.12 => fails.push(format!("a pad with {:.0}% below 150 Hz", low * 100.0)),
        "lead" | "arp" | "chord" if low > 0.2 => fails.push(format!("{} with {:.0}% below 150 Hz", r.role, low * 100.0)),
        "bass" if mid < 0.12 => fails.push(format!("a mid bass with only {:.0}% in 150 Hz - 5 kHz", mid * 100.0)),
        "fx" if impact && band(&sp, 0.0, 200.0) < 0.4 => fails.push("an impact without its low boom".into()),
        "fx" if r.has_tag("noise") && band(&sp, 500.0, 20000.0) < 0.4 => fails.push("a noise sweep without its top".into()),
        _ => {}
    }
    // the tune check (the instrument alone, as the song build checks a patch)
    let mut cents = None;
    if r.pitched() {
        if let Some(mut inst) = recipes::build_instrument(r, SR, bpm) {
            let tc = ling_daw::tunecheck::check(inst.as_mut(), SR);
            if tc.pitched >= 0.25 {
                cents = Some(tc.cents);
                if tc.cents.abs() > 15.0 {
                    fails.push(format!("{:+.0} cents out of tune", tc.cents));
                }
            }
        }
    }
    let _ = Kind::Bass;
    (buf, Report { id: r.id.clone(), peak_db: db(peak), rms_db: db(rms), dc, cents, side_lo_db: side_lo, sub, low, mid, high, fails })
}

fn write_wav(path: &std::path::Path, stereo: &[f32]) {
    let spec = hound::WavSpec { channels: 2, sample_rate: SR as u32, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    if let Ok(mut w) = hound::WavWriter::create(path, spec) {
        for x in stereo {
            let _ = w.write_sample((x.clamp(-1.0, 1.0) * 32767.0) as i16);
        }
        let _ = w.finalize();
    }
}

#[test]
fn every_recipe_renders_clean_in_tune_and_balanced_for_its_role() {
    let lib = recipes::library();
    assert!(lib.problems.is_empty(), "{:#?}", lib.problems);
    assert!(lib.recipes.len() >= 30, "{:?}", lib.dirs);
    let demos = std::env::var("LING_RECIPE_DEMOS").ok().filter(|d| !d.is_empty());
    if let Some(d) = &demos {
        let _ = std::fs::create_dir_all(d);
    }
    let mut bad = vec![];
    eprintln!("{:<26} {:>6} {:>6} {:>7} {:>6} {:>8} {:>5} {:>5} {:>5} {:>5}  result", "recipe", "peak", "rms", "dc", "cents", "sideLo", "<120", "<150", "mid", ">2k");
    for r in lib.recipes.iter().filter(|r| !r.is_master()) {
        let (buf, rep) = render(r);
        eprintln!(
            "{:<26} {:>6.1} {:>6.1} {:>7.4} {:>6} {:>8} {:>4.0}% {:>4.0}% {:>4.0}% {:>4.0}%  {}",
            rep.id,
            rep.peak_db,
            rep.rms_db,
            rep.dc,
            rep.cents.map(|c| format!("{c:+.1}")).unwrap_or_else(|| "-".into()),
            rep.side_lo_db.map(|c| format!("{c:.1}")).unwrap_or_else(|| "-".into()),
            rep.sub * 100.0,
            rep.low * 100.0,
            rep.mid * 100.0,
            rep.high * 100.0,
            if rep.fails.is_empty() { "ok".to_string() } else { rep.fails.join("; ") }
        );
        if let Some(d) = &demos {
            write_wav(&std::path::Path::new(d).join(format!("{}.wav", r.id)), &buf);
        }
        if !rep.fails.is_empty() {
            bad.push(format!("{}: {}", rep.id, rep.fails.join("; ")));
        }
    }
    assert!(bad.is_empty(), "{:#?}", bad);
}

// ── through the app's builtins ──────────────────────────────────────────────────────────────────────────────────────────

use ling_daw::{Daw, Val};

fn n(x: f64) -> Val {
    Val::Num(x)
}
fn s(x: &str) -> Val {
    Val::Str(x.into())
}
fn call(d: &mut Daw, name: &str, a: &[Val]) -> Val {
    d.call(name, a).unwrap_or_else(|| panic!("{name} not handled")).unwrap_or_else(|e| panic!("{name}: {e}"))
}
fn list(v: &Val) -> Vec<Val> {
    if let Val::List(l) = v { l.clone() } else { vec![] }
}
fn num(v: &Val) -> f64 {
    if let Val::Num(x) = v { *x } else { f64::NAN }
}
fn st(v: &Val) -> String {
    match v {
        Val::Str(x) => x.clone(),
        Val::Num(x) => format!("{x}"),
        _ => String::new(),
    }
}

#[test]
fn the_builtins_list_explain_apply_and_rewrite() {
    let lsx = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../LingStationX");
    if std::env::set_current_dir(lsx).is_err() {
        return;
    }
    let mut d = Daw::new();
    let all = list(&call(&mut d, "daw_recipe_list", &[s(""), s("")]));
    assert!(all.len() >= 30 * 6 && all.len() % 6 == 0);
    let basses = list(&call(&mut d, "daw_recipe_list", &[s("bass"), s("dubstep")]));
    assert!(basses.chunks(6).all(|r| st(&r[2]) == "bass") && basses.len() >= 6 * 6);
    let info = list(&call(&mut d, "daw_recipe_info", &[s("riddim_gate_a")]));
    assert_eq!(info.len(), 14);
    assert!(st(&info[7]).contains("Ducker") && st(&info[8]).contains("Register") && st(&info[8]).contains("Sound switching") && st(&info[9]).contains("mono below 120"), "{info:?}");
    let sug = list(&call(&mut d, "daw_recipe_suggest", &[s("growl bass"), s("dubstep"), s("riddim")]));
    assert!(st(&sug[0]).starts_with("riddim"), "{sug:?}");
    assert!(list(&call(&mut d, "daw_recipe_problems", &[])).is_empty());
    // a drum track and a bass with eight-note roots over A F C G
    let dr = num(&call(&mut d, "daw_track_add", &[s("drums"), s("")]));
    let bass = num(&call(&mut d, "daw_track_add", &[s("prism"), s("")]));
    call(&mut d, "daw_track_set_name", &[n(bass), s("Bass")]);
    let dc = num(&call(&mut d, "daw_clip_add_midi", &[n(dr), n(0.0), n(16.0)]));
    let mut kick = vec![];
    for b in 0..4 {
        kick.extend([n(b as f64 * 4.0), n(0.25), n(36.0), n(1.0), n(0.0), n(0.5), n(0.5)]);
    }
    call(&mut d, "daw_clip_set_notes", &[n(dr), n(dc), Val::List(kick)]);
    let bc = num(&call(&mut d, "daw_clip_add_midi", &[n(bass), n(0.0), n(32.0)]));
    let mut ns = vec![];
    for i in 0..64 {
        ns.extend([n(i as f64 * 0.5), n(0.45), n([45.0, 41.0, 48.0, 43.0][(i / 8) % 4]), n(0.9), n(0.0), n(0.5), n(0.5)]);
    }
    call(&mut d, "daw_clip_set_notes", &[n(bass), n(bc), Val::List(ns)]);
    let r = st(&call(&mut d, "daw_recipe_apply_part", &[n(bass), n(bc), s("riddim_gate_a")]));
    assert!(r.contains("Riddim Gate A") && r.contains("ducks under the kick"), "{r}");
    assert_eq!(st(&call(&mut d, "daw_track_kind", &[n(bass)])), "growl");
    let nt = num(&call(&mut d, "daw_track_count", &[])) as usize;
    let names: Vec<String> = (0..nt).map(|t| st(&call(&mut d, "daw_track_name", &[n(t as f64)]))).collect();
    assert!(names.iter().any(|x| x.starts_with("Bass · Riddim Wonk B")) && names.iter().any(|x| x.starts_with("Bass · Riddim Crunch C")), "{names:?}");
    let notes = list(&call(&mut d, "daw_clip_notes", &[n(bass), n(bc)]));
    assert!(!notes.is_empty() && notes.chunks(7).all(|x| (31.0..=43.0).contains(&num(&x[2]))), "{notes:?}");
    let ducks = list(&call(&mut d, "daw_duck_list", &[]));
    assert!(ducks.chunks(5).any(|x| num(&x[1]) == dr && num(&x[2]) == bass), "{ducks:?}");
    // the agent: list, then a reese on the bass by name
    let r = st(&call(&mut d, "daw_recipe_agent", &[n(-1.0), s("list the bass recipes")]));
    assert!(r.contains("Neuro Reese") && !r.contains("Supersaw Anthem"), "{r}");
    let r = st(&call(&mut d, "daw_recipe_agent", &[n(-1.0), s("use a reese on the bass")]));
    assert!(r.contains("Neuro Reese"), "{r}");
    assert_eq!(st(&call(&mut d, "daw_track_kind", &[n(bass)])), "prism");
    // a sound only (no clip): the track keeps its notes
    let before = list(&call(&mut d, "daw_clip_notes", &[n(bass), n(bc)]));
    let r = st(&call(&mut d, "daw_recipe_apply", &[n(bass), s("clean_sub")]));
    assert!(r.contains("Clean Sine Sub"), "{r}");
    assert_eq!(list(&call(&mut d, "daw_clip_notes", &[n(bass), n(bc)])).len(), before.len());
}

#[test]
fn an_ai_dubstep_song_uses_recipes_when_switched_on() {
    let lsx = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../LingStationX");
    if std::env::set_current_dir(lsx).is_err() {
        return;
    }
    let desc = "heavy riddim dubstep at 140 bpm in F minor";
    let mut d = Daw::new();
    let p = list(&call(&mut d, "daw_song_plan", &[s(desc), s(""), s(""), s(""), n(1.0), n(11.0)]));
    let line = st(&p[0]);
    let r = list(&call(&mut d, "daw_song_build", &[s(&line), s(desc), s(""), s(""), n(11.0), s(""), n(0.0), s(""), s(""), s(""), s(""), n(0.7), n(1.0)]));
    let summary = st(&r[4]);
    assert!(summary.contains("Sound recipes") && summary.contains("Riddim") && summary.contains("master"), "{summary}");
    let nt = num(&call(&mut d, "daw_track_count", &[])) as usize;
    let names: Vec<String> = (0..nt).map(|t| st(&call(&mut d, "daw_track_name", &[n(t as f64)]))).collect();
    let gb = names.iter().position(|x| x == "Growl Bass").expect("the growl bass");
    // a Growl recipe or a Ling Nova riddim recipe (recipes/nova.json)
    let gk = st(&call(&mut d, "daw_track_kind", &[n(gb as f64)]));
    assert!(gk == "growl" || gk == "nova", "{gk}");
    assert!(names.iter().any(|x| x.starts_with("Growl Bass · ")), "the switching set on sibling tracks: {names:?}");
    // the same song with recipes off is the old build
    let mut d2 = Daw::new();
    let r2 = list(&call(&mut d2, "daw_song_build", &[s(&line), s(desc), s(""), s(""), n(11.0)]));
    let s2 = st(&r2[4]);
    // (its basses are Ling Nova presets now, moved by their macros, where a Growl took a riddim skin)
    assert!(!s2.contains("Sound recipes") && (s2.contains("riddim skin") || s2.contains("Nova")), "{s2}");
    // other club genres take their recipes too (supersaw lead, pads, plucks, sub, risers); a folk song takes none
    for (desc, want) in [("uplifting trance at 138 bpm in A minor", "Supersaw"), ("kawaii future bass at 150 bpm", "Sub"), ("liquid drum and bass at 174 bpm", "Reese"), ("acoustic folk song in G major", "")] {
        let mut d = Daw::new();
        let p = list(&call(&mut d, "daw_song_plan", &[s(desc), s(""), s(""), s(""), n(1.0), n(5.0)]));
        let line = st(&p[0]);
        let r = list(&call(&mut d, "daw_song_build", &[s(&line), s(desc), s(""), s(""), n(5.0), s(""), n(0.0), s(""), s(""), s(""), s(""), n(0.7), n(1.0)]));
        let summary = st(&r[4]);
        if want.is_empty() {
            assert!(!summary.contains("Sound recipes"), "{desc}: {summary}");
        } else {
            assert!(summary.contains("Sound recipes") && summary.contains(want), "{desc}: {summary}");
        }
        let nt = num(&call(&mut d, "daw_track_count", &[])) as usize;
        assert!(nt >= 3, "{desc}");
    }
}
