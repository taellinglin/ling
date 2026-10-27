//! Where Ling Membrane's time goes: an idle kit (per-frame overhead) against one voice of each piece.
use ling_daw::*;

fn time(keys: &[u8], secs: f64) -> f64 {
    // as the audio thread runs: denormals flushed
    flush_denormals();
    let sr = 48000.0;
    let mut kit = dsp::make_instrument("drums", sr).unwrap();
    let mut buf = vec![0f32; 512];
    let blocks = (secs * sr as f64 / 256.0) as usize;
    let t0 = std::time::Instant::now();
    for b in 0..blocks {
        if b % 94 == 0 {
            for k in keys {
                kit.note_on(*k, 0.9);
            }
        }
        buf.fill(0.0);
        kit.render(&mut buf);
    }
    t0.elapsed().as_secs_f64() * 1e6 / secs
}

#[test]
#[ignore]
fn where_membrane_spends() {
    eprintln!("idle kit: {:.0} us/sec", time(&[], 4.0));
    for (name, k) in [("kick", 36u8), ("snare", 38), ("clap", 39), ("closed hat", 42), ("open hat", 46), ("crash", 49), ("ride", 51), ("tom", 45), ("cowbell", 56)] {
        eprintln!("{name:>10}: {:.0} us/sec", time(&[k], 4.0));
    }
}
