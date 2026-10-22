//! Tracks render side by side; the audio must come out exactly the same as when they render one after the other.
use ling_daw::*;

fn build(sr: f32) -> Engine {
    let mut e = Engine::new(sr);
    let kinds = ["growl", "synth", "voice", "growl", "synth", "drums", "synth", "growl"];
    for (i, k) in kinds.iter().enumerate() {
        let inst = dsp::make_instrument(k, sr).unwrap();
        let mut t = Track::new(&format!("t{i}"), TrackKind::Instrument, 0xff0000, Some(inst));
        t.inst_kind = k.to_string();
        let mut c = Clip::midi(0.0, 8.0, "c");
        if let ClipData::Midi(n) = &mut c.data {
            for b in 0..16 {
                n.push(Note::new(b as f64 * 0.5, 0.4, 36 + (i as u8 * 3 + b as u8 * 2) % 36, 0.8));
            }
        }
        t.clips.push(c);
        t.effects = vec![];
        e.project.tracks.push(t);
    }
    // a send and a sidechain, so the dependency levels are not all the same
    e.project.routes.push(Route { kind: RouteKind::Send, from: 0, to: 4, amount: 0.5, keys: vec![] });
    e.project.routes.push(Route { kind: RouteKind::Sidechain, from: 1, to: 2, amount: 1.0, keys: vec![] });
    e.project.routes.push(Route { kind: RouteKind::Send, from: 4, to: 6, amount: 0.3, keys: vec![] });
    e
}

#[test]
fn parallel_and_sequential_rendering_are_identical() {
    let sr = 48000.0;
    set_parallel_render(false);
    let t0 = std::time::Instant::now();
    let a = build(sr).bounce(0.5);
    let seq = t0.elapsed();
    set_parallel_render(true);
    let t1 = std::time::Instant::now();
    let b = build(sr).bounce(0.5);
    let par = t1.elapsed();
    eprintln!("sequential {:?}  parallel {:?}  ({} cores)", seq, par, std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1));
    assert_eq!(a.len(), b.len());
    assert!(a.iter().any(|x| x.abs() > 0.01), "the project must make sound");
    let diff = a.iter().zip(b.iter()).filter(|(x, y)| x.to_bits() != y.to_bits()).count();
    assert_eq!(diff, 0, "{diff} samples differ between parallel and sequential rendering");
}
