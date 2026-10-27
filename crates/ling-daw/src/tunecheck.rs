//! Tune check: is an instrument in tune with 12-tone equal temperament (A = 440)?
//!
//! A few notes across the range (C3, G3, C4, G4) are played and their pitch measured with the DAW's pitch tracker, past the attack.
//! Each note's deviation is folded to the nearest octave (a tracker that locks onto the octave is not an error) and to the nearest
//! fifth / fourth (a sound with a strong third harmonic can lock there), then:
//! * `cents` - the median signed deviation of the notes that had a pitch;
//! * `worst` - the largest absolute deviation;
//! * `pitched` - the share of analysed frames that had a pitch (a noise, a drum or a slow pad can have none: such a sound is not
//!   judged, see [`Tuning::in_tune`]).
//!
//! A microtonal patch (19-EDO glass, a slendro pad, Bohlen-Pierce drones), a detuned preset or a broken one fails; the AI song
//! build swaps it for the next candidate, and Edit > Tune Check Tracks reports it.
use crate::dsp::Instrument;

#[derive(Clone, Copy, Debug, Default)]
pub struct Tuning {
    pub cents: f32,
    pub worst: f32,
    pub pitched: f32,
}

impl Tuning {
    /// in tune, or not judgeable (too little pitch to tell)
    pub fn in_tune(&self) -> bool {
        self.pitched < 0.25 || (self.cents.abs() <= 30.0 && self.worst <= 55.0)
    }
}

/// the deviation folded to the nearest octave / fifth / fourth
fn fold(c: f32) -> f32 {
    let o = c - (c / 1200.0).round() * 1200.0;
    [o, o - 702.0, o + 702.0, o - 498.0, o + 498.0].into_iter().min_by(|a, b| a.abs().total_cmp(&b.abs())).unwrap_or(o)
}

pub fn check(inst: &mut dyn Instrument, sr: f32) -> Tuning {
    let keys = [48u8, 55, 60, 67];
    let mut devs: Vec<f32> = vec![];
    let (mut voiced, mut frames) = (0usize, 0usize);
    let hold = (sr * 0.8) as usize;
    let mut buf = vec![0f32; 512];
    for k in keys {
        inst.all_off();
        for _ in 0..8 {
            buf.fill(0.0);
            inst.render(&mut buf);
        }
        inst.note_on(k, 0.85);
        let mut st: Vec<f32> = Vec::with_capacity(hold * 2);
        while st.len() / 2 < hold {
            buf.fill(0.0);
            inst.render(&mut buf);
            st.extend_from_slice(&buf);
        }
        inst.note_off(k);
        let mono: Vec<f32> = st.chunks_exact(2).map(|f| (f[0] + f[1]) * 0.5).collect();
        // past the attack: the last 80 %
        let tail = &mono[mono.len() / 5..];
        let p = crate::vocal::track_pitch(tail, sr);
        let f: Vec<f32> = p.f0.iter().copied().filter(|x| *x > 20.0).collect();
        frames += p.f0.len();
        voiced += f.len();
        if f.len() >= 5 {
            let mut s = f.clone();
            s.sort_by(f32::total_cmp);
            let med = s[s.len() / 2];
            let want = 440.0 * 2f32.powf((k as f32 - 69.0) / 12.0);
            devs.push(fold(1200.0 * (med / want).log2()));
        }
    }
    inst.all_off();
    let pitched = voiced as f32 / frames.max(1) as f32;
    if devs.is_empty() {
        return Tuning { cents: 0.0, worst: 0.0, pitched };
    }
    let worst = devs.iter().map(|d| d.abs()).fold(0.0, f32::max);
    devs.sort_by(f32::total_cmp);
    Tuning { cents: devs[devs.len() / 2], worst, pitched }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_synth_is_in_tune_and_folding_forgives_octaves() {
        let mut s = crate::dsp::make_instrument("synth", 44100.0).unwrap();
        let t = check(s.as_mut(), 44100.0);
        assert!(t.in_tune(), "{t:?}");
        assert!(fold(1203.0).abs() < 5.0 && fold(-1195.0).abs() < 6.0 && fold(705.0).abs() < 4.0);
        assert!(fold(40.0) == 40.0);
    }

    #[test]
    fn microtonal_presets_fail_and_twelve_tone_ones_pass() {
        let sr = 44100.0;
        let check_preset = |name: &str| {
            let mut p = crate::dsp::make_instrument("prism", sr).unwrap();
            p.load_preset(name);
            check(p.as_mut(), sr)
        };
        for good in ["Uplift Pad", "Rolling Bass", "Trance Pluck"] {
            let t = check_preset(good);
            assert!(t.in_tune(), "{good}: {t:?}");
        }
        let bad: Vec<(&str, Tuning)> = ["19 EDO Glass", "Slendro Pad", "Bohlen Drone"].iter().map(|n| (*n, check_preset(n))).collect();
        assert!(bad.iter().filter(|(_, t)| !t.in_tune()).count() >= 2, "{bad:?}");
    }

    #[test]
    fn every_instrument_kind_survives_the_check() {
        for kind in ["synth", "growl", "voice", "prism", "sampler", "pluck", "chip", "script"] {
            let Some(mut i) = crate::dsp::make_instrument(kind, 44100.0) else { continue };
            let st = i.state();
            i.load_state(&st);
            let _ = check(i.as_mut(), 44100.0);
            // a state with nothing in it, as an empty track has
            let Some(mut j) = crate::dsp::make_instrument(kind, 44100.0) else { continue };
            j.load_state(&[]);
            let _ = check(j.as_mut(), 44100.0);
        }
    }
}
