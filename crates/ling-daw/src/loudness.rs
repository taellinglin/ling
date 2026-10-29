//! Loudness targets for the master, and a model of the master through the true-peak Limiter.
//!
//! * `PRESETS` - the loudness presets of AI Mix & Master (streaming -14, club EDM -7, dubstep / riddim -5.5, lo-fi, ambient, film,
//!   broadcast ...), each with its true-peak ceiling.
//! * `genre_target` - the integrated loudness a genre's master is usually made at (club genres -5.5 to -8 LUFS, songs about -9,
//!   lo-fi -12, ambient and film -16): what the mix AI aims at when nobody chose a target.
//! * `LoudModel` - built once from the analysed master (before the limiter): per short chunk, the true peak and the K-weighted
//!   energy. `lufs_at(gain)` replays the Limiter's gain computer on those chunks (look-ahead hold, release, averaging) and returns the
//!   integrated loudness the limited master would have, so `solve` finds the limiter drive for a target by bisection without
//!   rendering the song again. The test renders through the real Limiter and lands within 0.5 LU.
use crate::dsp::edmfx::{Limiter, TruePeak};
use crate::dsp::fx::Effect;

pub struct Preset {
    pub name: &'static str,
    pub lufs: f32,
    pub ceiling: f32,
    pub note: &'static str,
}

/// index 0 is "Auto (genre)": the target comes from `genre_target`
pub const PRESETS: [Preset; 10] = [
    Preset { name: "Auto (genre)", lufs: f32::NAN, ceiling: -1.0, note: "the usual loudness of the song's genre" },
    Preset { name: "Streaming -14", lufs: -14.0, ceiling: -1.0, note: "what the streaming services normalise to: no level is lost to their turn-down" },
    Preset { name: "Loud streaming -11", lufs: -11.0, ceiling: -1.0, note: "a modern pop / electronic release that still breathes" },
    Preset { name: "Pop / rock -9", lufs: -9.0, ceiling: -1.0, note: "a loud song master" },
    Preset { name: "Club EDM -7", lufs: -7.0, ceiling: -1.0, note: "club masters of house, techno, trance and drum and bass (-6 to -8 LUFS)" },
    Preset { name: "Dubstep / riddim -5.5", lufs: -5.5, ceiling: -1.0, note: "bass music club masters (-5 to -6 LUFS): dense and heavily limited" },
    Preset { name: "Lo-fi / chill -12", lufs: -12.0, ceiling: -1.0, note: "soft and warm, room for the transients" },
    Preset { name: "Ambient -16", lufs: -16.0, ceiling: -1.0, note: "quiet and dynamic" },
    Preset { name: "Film / classical -18", lufs: -18.0, ceiling: -1.0, note: "wide dynamics for scores and orchestras" },
    Preset { name: "Broadcast -23", lufs: -23.0, ceiling: -1.0, note: "EBU R128 broadcast loudness" },
];

/// the master loudness of each song-generator genre (LUFS)
const GENRE_LUFS: [(&str, f32); 30] = [
    ("dubstep", -5.5),
    ("hardstyle", -6.0),
    ("drum and bass", -6.5),
    ("future bass", -7.0),
    ("trance", -7.0),
    ("techno", -7.0),
    ("acid techno", -7.0),
    ("hyperpop", -7.0),
    ("house", -7.5),
    ("garage", -7.5),
    ("breakbeat", -7.5),
    ("trap", -7.5),
    ("reggaeton", -8.0),
    ("deep house", -8.5),
    ("pop", -8.5),
    ("hip hop", -8.5),
    ("rock", -9.0),
    ("disco", -9.0),
    ("afrobeat", -9.0),
    ("synthwave", -9.5),
    ("chiptune", -10.0),
    ("witch house", -10.0),
    ("dub", -10.0),
    ("lofi", -12.0),
    ("folk", -12.0),
    ("ballad", -12.0),
    ("jazz", -14.0),
    ("cinematic", -14.0),
    ("orchestral", -16.0),
    ("ambient", -16.0),
];

/// The master loudness (LUFS) for a genre (a song-generator genre name or one of its words, "riddim" too); -14 (streaming) when unknown.
pub fn genre_target(genre: &str) -> f32 {
    let g = genre.trim().to_lowercase();
    if g.is_empty() {
        return -14.0;
    }
    if let Some((_, l)) = GENRE_LUFS.iter().find(|(n, _)| *n == g) {
        return *l;
    }
    if ["riddim", "brostep", "tearout", "colour bass", "color bass", "melodic dubstep"].iter().any(|w| g.contains(w)) {
        return -5.5;
    }
    // a word of a known genre ("edm", "dnb" ...)
    let canon = crate::songgen::genre(&g).name;
    if crate::songgen::GENRES.iter().any(|x| x.name == canon && (x.words.contains(&g.as_str()) || x.name == g)) {
        if let Some((_, l)) = GENRE_LUFS.iter().find(|(n, _)| *n == canon) {
            return *l;
        }
    }
    -14.0
}

/// samples per chunk of the model
const CHUNK: usize = 8;
/// the Limiter's internal margin under the ceiling (dB), mirrored here
const MARGIN_DB: f32 = 0.1;

/// The analysed master before the limiter, reduced to what the limiter and the loudness meter need.
pub struct LoudModel {
    sr: f32,
    /// the true peak of each chunk
    tp: Vec<f32>,
    /// the K-weighted energy (L^2 + R^2 summed) of each chunk
    ke: Vec<f32>,
    frames: usize,
}

impl LoudModel {
    pub fn build(stereo: &[f32], sr: f32) -> LoudModel {
        let frames = stereo.len() / 2;
        let nc = frames.div_ceil(CHUNK) + 1;
        let mut tp = vec![0.0f32; nc];
        let mut ke = vec![0.0f32; nc];
        let mut det = TruePeak::new();
        let mut k = [crate::mixai::k_filters(sr), crate::mixai::k_filters(sr)];
        for n in 0..frames + TruePeak::DELAY + 1 {
            let (l, r) = if n < frames { (stereo[n * 2], stereo[n * 2 + 1]) } else { (0.0, 0.0) };
            let p = det.push(l, r);
            // the stretch the peak finder reports is DELAY samples back; it touches that sample and the next
            if n >= TruePeak::DELAY {
                let m = n - TruePeak::DELAY;
                for s in [m, m + 1] {
                    if s < frames {
                        let c = s / CHUNK;
                        tp[c] = tp[c].max(p);
                    }
                }
            }
            if n < frames {
                let a0 = k[0][0].run(l);
                let a = k[0][1].run(a0);
                let b0 = k[1][0].run(r);
                let b = k[1][1].run(b0);
                ke[n / CHUNK] += a * a + b * b;
            }
        }
        LoudModel { sr, tp, ke, frames }
    }

    pub fn seconds(&self) -> f32 {
        self.frames as f32 / self.sr
    }

    /// integrated loudness (LUFS) of per-chunk energies `e`, BS.1770 gating (400 ms blocks, 100 ms hop)
    fn gated(&self, e: &[f64]) -> f32 {
        let sub = ((self.sr * 0.1) as usize / CHUNK).max(1);
        let ns = e.len() / sub;
        if ns < 4 {
            return -70.0;
        }
        let subs: Vec<f64> = (0..ns).map(|i| e[i * sub..(i + 1) * sub].iter().sum::<f64>()).collect();
        let blk = (4 * sub * CHUNK) as f64;
        let blocks: Vec<f64> = (0..=ns - 4).map(|i| subs[i..i + 4].iter().sum::<f64>() / blk).collect();
        let l = |p: f64| -0.691 + 10.0 * p.max(1e-12).log10();
        let abs: Vec<f64> = blocks.into_iter().filter(|p| l(*p) > -70.0).collect();
        if abs.is_empty() {
            return -70.0;
        }
        let gate = l(abs.iter().sum::<f64>() / abs.len() as f64) - 10.0;
        let rel: Vec<f64> = abs.into_iter().filter(|p| l(*p) > gate).collect();
        if rel.is_empty() {
            return -70.0;
        }
        l(rel.iter().sum::<f64>() / rel.len() as f64) as f32
    }

    /// the integrated loudness of the master pushed `gain_db` into the Limiter (ceiling dBTP, release s, look-ahead s)
    pub fn lufs_at(&self, gain_db: f32, ceiling_db: f32, release: f32, look: f32) -> f32 {
        let g = 10f32.powf(gain_db / 20.0);
        let ceil = 10f32.powf((ceiling_db - MARGIN_DB) / 20.0);
        let w = (((look * self.sr) as usize).max(1)).div_ceil(CHUNK).max(1);
        let kc = 1.0 - (-(CHUNK as f32) / (release.max(0.001) * self.sr)).exp();
        let n = self.tp.len();
        let mut q: std::collections::VecDeque<(usize, f32)> = std::collections::VecDeque::with_capacity(w + 2);
        let mut ring = vec![1.0f32; w];
        let (mut sum, mut ri) = (w as f64, 0usize);
        let mut rel = 1.0f32;
        let mut e = vec![0.0f64; n];
        // the gain for chunk c is the box average ending at chunk c + w - 1
        for i in 0..n + w {
            let pk = if i < n { self.tp[i] * g } else { 0.0 };
            let need = if pk > ceil { ceil / pk } else { 1.0 };
            while q.back().map(|b| b.1 >= need).unwrap_or(false) {
                q.pop_back();
            }
            q.push_back((i, need));
            while q.front().map(|f| f.0 + w <= i).unwrap_or(false) {
                q.pop_front();
            }
            let held = q.front().map(|x| x.1).unwrap_or(1.0);
            rel = if held < rel { held } else { rel + (held - rel) * kc };
            sum += rel as f64 - ring[ri] as f64;
            ring[ri] = rel;
            ri = (ri + 1) % w;
            if i + 1 >= w {
                let c = i + 1 - w;
                if c < n {
                    let gg = (sum / w as f64).min(1.0) * g as f64;
                    e[c] = self.ke[c] as f64 * gg * gg;
                }
            }
        }
        self.gated(&e)
    }

    /// The limiter drive (dB, -20..+24) that brings the master to `target` LUFS under `ceiling_db`, and the loudness it reaches.
    pub fn solve(&self, target: f32, ceiling_db: f32, release: f32, look: f32) -> (f32, f32) {
        let (mut lo, mut hi) = (-20.0f32, 24.0f32);
        if self.lufs_at(hi, ceiling_db, release, look) <= target {
            return (hi, self.lufs_at(hi, ceiling_db, release, look));
        }
        if self.lufs_at(lo, ceiling_db, release, look) >= target {
            return (lo, self.lufs_at(lo, ceiling_db, release, look));
        }
        for _ in 0..22 {
            let mid = 0.5 * (lo + hi);
            if self.lufs_at(mid, ceiling_db, release, look) < target {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let gdb = 0.5 * (lo + hi);
        (gdb, self.lufs_at(gdb, ceiling_db, release, look))
    }
}

/// The Limiter release (its 0..1 parameter) for a loudness target: loud club masters get a fast release (about 40 ms, the gain
/// recovers between kicks), the rest the default 100 ms.
pub fn release_for(target: f32) -> f32 {
    if target >= -9.0 {
        0.3
    } else {
        0.5
    }
}

/// The release (s) and look-ahead (s) a Limiter's parameters (0..1 by name) stand for; the defaults when missing.
pub fn limiter_timing(params: &[(&str, f32)]) -> (f32, f32) {
    let get = |n: &str, d: f32| params.iter().find(|p| p.0 == n).map(|p| p.1).unwrap_or(d);
    (0.01 * 100f32.powf(get("release", 0.5)), 0.0005 + get("lookahead", 0.3) * 0.0045)
}

/// What the end of the master chain does to a rendered mix: the Limiter (with `params`, when there is one), the master volume and the
/// engine's own safety limiter (instant attack, 60 ms release, 0.97), then the +-1 clip. Used to measure the master as it is now.
pub fn apply_tail(buf: &mut [f32], sr: f32, limiter: Option<&[(&str, f32)]>, master_volume: f32, engine_limiter: bool) {
    if let Some(ps) = limiter {
        let mut l = Limiter::new(sr);
        for (n, v) in ps {
            l.set_param(n, *v);
        }
        for c in buf.chunks_mut(2048) {
            l.process(c);
        }
    }
    let rel = 1.0 - (-1.0 / (0.06 * sr)).exp();
    let mut lg = 1.0f32;
    for f in buf.chunks_exact_mut(2) {
        let mut v = [f[0] * master_volume, f[1] * master_volume];
        if engine_limiter {
            let peak = v[0].abs().max(v[1].abs());
            if peak * lg > 0.97 {
                lg = 0.97 / peak.max(1e-9);
            } else {
                lg += (1.0 - lg) * rel;
            }
            v[0] *= lg;
            v[1] *= lg;
        }
        f[0] = if v[0].is_finite() { v[0].clamp(-1.0, 1.0) } else { 0.0 };
        f[1] = if v[1].is_finite() { v[1].clamp(-1.0, 1.0) } else { 0.0 };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::edmfx::LIMITER_PARAMS;

    const SR: f32 = 48000.0;

    /// 16 s of a club-like mix at 128 bpm: four-on-the-floor kick, off-beat hats, a bass, a chord stab, quieter intro / louder drop
    pub fn test_mix() -> Vec<f32> {
        let n = (SR * 16.0) as usize;
        let spb = (SR * 60.0 / 128.0) as usize;
        let mut out = vec![0.0f32; n * 2];
        let mut seed = 12345u32;
        for i in 0..n {
            let t = i as f32 / SR;
            let b = i % spb;
            let kt = b as f32 / SR;
            let kick = (std::f32::consts::TAU * (50.0 + 120.0 * (-kt * 30.0).exp()) * kt).sin() * (-kt * 9.0).exp() * 0.9;
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let nz = seed as f32 / u32::MAX as f32 * 2.0 - 1.0;
            let hb = (i + spb / 2) % spb;
            let hat = nz * (-(hb as f32 / SR) * 60.0).exp() * 0.25;
            let bass = (std::f32::consts::TAU * 55.0 * t).sin() * 0.35 * (1.0 - (-kt * 20.0).exp());
            let stab = [220.0f32, 261.6, 329.6].iter().map(|f| (std::f32::consts::TAU * f * t).sin()).sum::<f32>() * 0.08 * if (i / (spb / 2)) % 3 == 0 { 1.0 } else { 0.3 };
            let level = if t < 4.0 { 0.35 } else { 1.0 };
            let s = (kick + hat * 0.6 + bass + stab) * level;
            out[i * 2] = s + hat * 0.2;
            out[i * 2 + 1] = s - hat * 0.2;
        }
        out
    }

    #[test]
    fn genres_have_their_loudness() {
        assert_eq!(genre_target("dubstep"), -5.5);
        assert_eq!(genre_target("riddim"), -5.5);
        assert!((-8.0..=-6.0).contains(&genre_target("house")) && (-8.0..=-6.0).contains(&genre_target("techno")));
        assert!(genre_target("ambient") <= -14.0 && genre_target("lofi") < -10.0);
        assert_eq!(genre_target(""), -14.0);
        assert_eq!(genre_target("polka from mars"), -14.0);
        // every song-generator genre has its own entry
        for g in crate::songgen::GENRES {
            assert!(GENRE_LUFS.iter().any(|x| x.0 == g.name), "{} has no loudness", g.name);
        }
        assert!(PRESETS.iter().skip(1).all(|p| p.lufs.is_finite() && p.ceiling <= -0.9));
    }

    #[test]
    fn the_model_hits_the_target_through_the_real_limiter() {
        let mix = test_mix();
        let model = LoudModel::build(&mix, SR);
        for target in [-14.0f32, -9.0, -7.0, -5.5] {
            let rp = release_for(target);
            let (rel, look) = limiter_timing(&[("release", rp)]);
            let (gdb, est) = model.solve(target, -1.0, rel, look);
            // the club targets must be reachable for a mix like this; past what a limiter can do the model says how far it gets
            let reach = (est - target).abs() < 0.05;
            assert!(reach || (target > -6.0 && gdb >= 23.9), "the model reaches {target}: {est} at {gdb}");
            // render through the Limiter at that drive (a negative drive is the master volume after it)
            let mut l = Limiter::new(SR);
            for (n, v) in LIMITER_PARAMS {
                l.set_param(n, v);
            }
            l.set_param("release", rp);
            l.set_param("gain", (gdb / 24.0).clamp(0.0, 1.0));
            let mut b = mix.clone();
            for c in b.chunks_mut(1024) {
                l.process(c);
            }
            if gdb < 0.0 {
                let v = 10f32.powf(gdb / 20.0);
                b.iter_mut().for_each(|x| *x *= v);
            }
            let got = crate::mixai::lufs(&b, SR);
            eprintln!("target {target}: model {est:.2}, rendered {got:.2} LUFS at {gdb:+.2} dB drive");
            assert!((got - est).abs() < 0.5, "target {target}: rendered {got}, the model said {est} (drive {gdb:+.1} dB)");
            if reach {
                assert!((got - target).abs() < 0.5, "target {target}: rendered {got} (drive {gdb:+.1} dB)");
            }
            let tp = crate::dsp::edmfx::true_peak_db(&b);
            assert!(tp <= -0.99, "target {target}: true peak {tp}");
        }
    }

    #[test]
    fn the_tail_measures_the_master_as_it_plays() {
        let mut b = test_mix();
        apply_tail(&mut b, SR, None, 2.0, true);
        // the engine's safety limiter: instant attack at 0.97 (its release can overshoot it by a hair, as in the engine)
        assert!(b.iter().all(|x| x.abs() <= 0.975));
        let mut c = test_mix();
        apply_tail(&mut c, SR, Some(&[("gain", 0.25)]), 1.0, true);
        assert!(crate::dsp::edmfx::true_peak_db(&c) <= -0.99);
    }
}
