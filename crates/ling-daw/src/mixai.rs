//! AI Mix & Master: measure every track and the master (loudness, peaks, dynamics, spectral balance, stereo width), then plan the moves a mix
//! engineer would make: levels by role, panning of crowded instruments, low cut and mud cut, a little compression where a track is too
//! dynamic, and the master level for a loudness target. The numbers decide; the language model (Qwen) only names the roles and picks the
//! style, so the plan is the same on every machine and every move comes with its reason.
use std::f32::consts::PI;

pub const BANDS: usize = 6;
pub const BAND_EDGES: [f32; BANDS + 1] = [20.0, 60.0, 250.0, 800.0, 2500.0, 6000.0, 20000.0];
pub const BAND_NAMES: [&str; BANDS] = ["sub", "bass", "low-mid", "mid", "presence", "air"];

#[derive(Clone, Debug, Default)]
pub struct Stat {
    pub peak_db: f32,
    /// RMS over the whole signal (dBFS)
    pub rms_db: f32,
    /// RMS over the moments the track is playing (blocks above -55 dB), the level the track has when it is there
    pub active_rms_db: f32,
    /// integrated loudness (LUFS, BS.1770: K-weighted, gated)
    pub lufs: f32,
    pub crest_db: f32,
    pub centroid_hz: f32,
    /// side energy relative to mid in dB (negative = narrow / mono)
    pub width_db: f32,
    /// share of 100 ms blocks that are not silent
    pub active: f32,
    /// energy per band relative to the track's total, dB
    pub bands: [f32; BANDS],
    /// L/R correlation -1..1
    pub corr: f32,
    pub clipped: usize,
}

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}
fn pdb(x: f32) -> f32 {
    10.0 * x.max(1e-12).log10()
}

// ── K-weighting (ITU-R BS.1770) ────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct Bq {
    b: [f32; 3],
    a: [f32; 2],
    z: [f32; 2],
}
impl Bq {
    fn run(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}
fn k_filters(sr: f32) -> [Bq; 2] {
    // stage 1: the head-related high shelf
    let (f0, g, q) = (1681.9745f32, 3.99984f32, 0.707175f32);
    let a = 10f32.powf(g / 40.0);
    let w0 = 2.0 * PI * f0 / sr;
    let (cw, sw) = (w0.cos(), w0.sin());
    let alpha = sw / (2.0 * q);
    let sa = a.sqrt();
    let b0 = a * ((a + 1.0) + (a - 1.0) * cw + 2.0 * sa * alpha);
    let b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cw);
    let b2 = a * ((a + 1.0) + (a - 1.0) * cw - 2.0 * sa * alpha);
    let a0 = (a + 1.0) - (a - 1.0) * cw + 2.0 * sa * alpha;
    let a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cw);
    let a2 = (a + 1.0) - (a - 1.0) * cw - 2.0 * sa * alpha;
    let s1 = Bq { b: [b0 / a0, b1 / a0, b2 / a0], a: [a1 / a0, a2 / a0], z: [0.0; 2] };
    // stage 2: the revised low-frequency B weighting (a 38 Hz high-pass)
    let (f0, q) = (38.13547f32, 0.500327f32);
    let w0 = 2.0 * PI * f0 / sr;
    let (cw, sw) = (w0.cos(), w0.sin());
    let alpha = sw / (2.0 * q);
    let a0 = 1.0 + alpha;
    let s2 = Bq { b: [(1.0 + cw) / 2.0 / a0, -(1.0 + cw) / a0, (1.0 + cw) / 2.0 / a0], a: [-2.0 * cw / a0, (1.0 - alpha) / a0], z: [0.0; 2] };
    [s1, s2]
}

/// integrated loudness in LUFS of interleaved stereo (absolute gate -70, relative gate -10)
pub fn lufs(stereo: &[f32], sr: f32) -> f32 {
    let n = stereo.len() / 2;
    if n < (sr * 0.4) as usize {
        return -70.0;
    }
    let mut kl = k_filters(sr);
    let mut kr = k_filters(sr);
    let mut e = Vec::with_capacity(n);
    for f in 0..n {
        let l0 = kl[0].run(stereo[f * 2]);
        let l = kl[1].run(l0);
        let r0 = kr[0].run(stereo[f * 2 + 1]);
        let r = kr[1].run(r0);
        e.push(l * l + r * r);
    }
    let block = (sr * 0.4) as usize;
    let step = (sr * 0.1) as usize;
    let mut cum = vec![0.0f64; n + 1];
    for i in 0..n {
        cum[i + 1] = cum[i] + e[i] as f64;
    }
    let mut blocks: Vec<f32> = vec![];
    let mut s = 0;
    while s + block <= n {
        blocks.push(((cum[s + block] - cum[s]) / block as f64) as f32);
        s += step;
    }
    let abs: Vec<f32> = blocks.iter().cloned().filter(|z| -0.691 + pdb(*z) > -70.0).collect();
    if abs.is_empty() {
        return -70.0;
    }
    let mean = abs.iter().sum::<f32>() / abs.len() as f32;
    let rel_gate = -0.691 + pdb(mean) - 10.0;
    let rel: Vec<f32> = abs.into_iter().filter(|z| -0.691 + pdb(*z) > rel_gate).collect();
    if rel.is_empty() {
        return -70.0;
    }
    -0.691 + pdb(rel.iter().sum::<f32>() / rel.len() as f32)
}

// ── spectrum ──────────────────────────────────────────────────────────────────

fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * PI / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        let mut i = 0;
        while i < n {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (ur, ui) = (re[i + k], im[i + k]);
                let (vr, vi) = (re[i + k + len / 2] * cr - im[i + k + len / 2] * ci, re[i + k + len / 2] * ci + im[i + k + len / 2] * cr);
                re[i + k] = ur + vr;
                im[i + k] = ui + vi;
                re[i + k + len / 2] = ur - vr;
                im[i + k + len / 2] = ui - vi;
                let t = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = t;
            }
            i += len;
        }
        len <<= 1;
    }
}

/// Measure interleaved stereo audio.
pub fn analyze(stereo: &[f32], sr: f32) -> Stat {
    let n = stereo.len() / 2;
    let mut st = Stat { peak_db: -120.0, rms_db: -120.0, active_rms_db: -120.0, lufs: -70.0, bands: [-120.0; BANDS], ..Stat::default() };
    if n < 2048 {
        return st;
    }
    let mut peak = 0f32;
    let (mut sq, mut sl, mut sr_, mut slr, mut mid, mut side) = (0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
    for f in 0..n {
        let (l, r) = (stereo[f * 2], stereo[f * 2 + 1]);
        peak = peak.max(l.abs()).max(r.abs());
        if l.abs() >= 0.999 || r.abs() >= 0.999 {
            st.clipped += 1;
        }
        sq += (l * l + r * r) as f64 * 0.5;
        sl += (l * l) as f64;
        sr_ += (r * r) as f64;
        slr += (l * r) as f64;
        let (m, s) = ((l + r) * 0.5, (l - r) * 0.5);
        mid += (m * m) as f64;
        side += (s * s) as f64;
    }
    st.peak_db = db(peak);
    st.rms_db = pdb((sq / n as f64) as f32);
    st.crest_db = st.peak_db - st.rms_db;
    st.width_db = pdb(side as f32) - pdb(mid as f32);
    st.corr = (slr / ((sl * sr_).sqrt() + 1e-12)) as f32;
    st.lufs = lufs(stereo, sr);
    // active blocks (100 ms): the level of the track where it plays
    let blk = (sr * 0.1) as usize;
    let mut act = 0usize;
    let mut total_blocks = 0usize;
    let mut act_sq = 0f64;
    let mut act_n = 0usize;
    let mut b = 0;
    while b + blk <= n {
        let e: f64 = (b..b + blk).map(|f| ((stereo[f * 2].powi(2) + stereo[f * 2 + 1].powi(2)) * 0.5) as f64).sum::<f64>() / blk as f64;
        total_blocks += 1;
        if pdb(e as f32) > -55.0 {
            act += 1;
            act_sq += e * blk as f64;
            act_n += blk;
        }
        b += blk;
    }
    st.active = act as f32 / total_blocks.max(1) as f32;
    st.active_rms_db = if act_n > 0 { pdb((act_sq / act_n as f64) as f32) } else { -120.0 };
    // average power spectrum over windows that are playing
    const N: usize = 2048;
    let win: Vec<f32> = (0..N).map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / N as f32).cos()).collect();
    let mut power = vec![0f64; N / 2];
    let hop = N;
    let mut count = 0;
    let mut p = 0;
    while p + N <= n {
        let mut re: Vec<f32> = (0..N).map(|i| (stereo[(p + i) * 2] + stereo[(p + i) * 2 + 1]) * 0.5 * win[i]).collect();
        let energy: f32 = re.iter().map(|x| x * x).sum::<f32>() / N as f32;
        if pdb(energy * 2.0) > -55.0 {
            let mut im = vec![0f32; N];
            fft(&mut re, &mut im);
            for k in 0..N / 2 {
                power[k] += (re[k] * re[k] + im[k] * im[k]) as f64;
            }
            count += 1;
        }
        p += hop;
    }
    if count > 0 {
        let binhz = sr / N as f32;
        let mut band = [0f64; BANDS];
        let (mut num, mut den) = (0f64, 0f64);
        for (k, pw) in power.iter().enumerate().skip(1) {
            let f = k as f32 * binhz;
            num += f as f64 * pw;
            den += pw;
            for bi in 0..BANDS {
                if f >= BAND_EDGES[bi] && f < BAND_EDGES[bi + 1] {
                    band[bi] += pw;
                }
            }
        }
        let tot: f64 = band.iter().sum::<f64>().max(1e-30);
        for bi in 0..BANDS {
            st.bands[bi] = pdb((band[bi] / tot) as f32);
        }
        st.centroid_hz = (num / den.max(1e-30)) as f32;
    }
    st
}

// ── roles and the plan ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Role {
    Drums,
    Bass,
    Vocal,
    Lead,
    Chords,
    Pad,
    Fx,
    Other,
}
impl Role {
    pub const ALL: [Role; 8] = [Role::Drums, Role::Bass, Role::Vocal, Role::Lead, Role::Chords, Role::Pad, Role::Fx, Role::Other];
    pub fn name(self) -> &'static str {
        match self {
            Role::Drums => "drums",
            Role::Bass => "bass",
            Role::Vocal => "vocal",
            Role::Lead => "lead",
            Role::Chords => "chords",
            Role::Pad => "pad",
            Role::Fx => "fx",
            Role::Other => "other",
        }
    }
    pub fn parse(s: &str) -> Option<Role> {
        let s = s.trim().to_lowercase();
        Role::ALL.iter().copied().find(|r| r.name() == s).or(match s.as_str() {
            "drum" | "percussion" | "perc" | "kick" | "hats" => Some(Role::Drums),
            "vocals" | "voice" | "vox" | "choir" => Some(Role::Vocal),
            "melody" | "solo" => Some(Role::Lead),
            "chord" | "keys" | "rhythm" | "guitar" | "piano" => Some(Role::Chords),
            "strings" | "atmosphere" | "ambient" => Some(Role::Pad),
            "sfx" | "effects" | "noise" => Some(Role::Fx),
            _ => None,
        })
    }
    /// the level (dBFS RMS where the track plays) this role sits at in a balanced mix
    pub fn target_rms(self) -> f32 {
        match self {
            Role::Drums => -19.0,
            Role::Bass => -19.0,
            Role::Vocal => -18.0,
            Role::Lead => -20.0,
            Role::Chords => -24.0,
            Role::Pad => -27.0,
            Role::Fx => -30.0,
            Role::Other => -24.0,
        }
    }
}

/// A first guess of what a track is: its name and instrument say most, the spectrum the rest.
pub fn guess_role(name: &str, inst_kind: &str, st: &Stat) -> Role {
    let n = name.to_lowercase();
    let has = |ws: &[&str]| ws.iter().any(|w| n.contains(w));
    if inst_kind == "drums" || has(&["drum", "kick", "snare", "hat", "perc", "kit", "cymbal", "tom ", "clap"]) {
        return Role::Drums;
    }
    if has(&["bass", "sub ", "808", "contrabass"]) {
        return Role::Bass;
    }
    if inst_kind == "voice" || has(&["vox", "vocal", "voice", "sing", "choir", "lyric"]) {
        return Role::Vocal;
    }
    if has(&["fx", "riser", "noise", "sweep", "impact", "foley"]) {
        return Role::Fx;
    }
    if has(&["pizz", "stacc", "pluck", "spiccato"]) {
        return Role::Chords;
    }
    if has(&["pad", "string", "swell", "atmos", "ambient", "drone", "sustain", "choir"]) {
        return Role::Pad;
    }
    if has(&["lead", "melody", "solo", "flute", "violin", "trumpet", "sax", "oboe", "clarinet", "synth lead", "whistle"]) {
        return Role::Lead;
    }
    if has(&["chord", "piano", "keys", "guitar", "pluck", "harp", "organ", "stab", "arp", "pizz", "rhodes"]) {
        return Role::Chords;
    }
    if st.centroid_hz > 0.0 && st.centroid_hz < 260.0 {
        return Role::Bass;
    }
    if st.active > 0.0 && st.centroid_hz > 2500.0 {
        return Role::Lead;
    }
    if st.active < 0.35 {
        return Role::Fx;
    }
    Role::Chords
}

#[derive(Clone, Debug)]
pub struct Style {
    /// integrated loudness the master is aimed at (LUFS)
    pub loudness: f32,
    /// -1 darker .. +1 brighter
    pub brightness: f32,
    /// 0..1: how much dynamic control the drums, bass and vocals get
    pub punch: f32,
    /// 0..1: how far crowded instruments are spread
    pub width: f32,
}
impl Default for Style {
    fn default() -> Style {
        Style { loudness: -14.0, brightness: 0.0, punch: 0.5, width: 0.5 }
    }
}

#[derive(Clone, Debug)]
pub struct Change {
    /// track index, -1 = master
    pub track: i32,
    /// gain | pan | eq | comp | master
    pub what: String,
    /// gain: the new fader (linear). pan: the new pan. eq: [low, mid, high] in dB. comp: the threshold in dBFS then ratio. master: the new master volume
    pub values: Vec<f32>,
    pub reason: String,
}

pub struct TrackInfo {
    pub name: String,
    pub volume: f32,
    pub pan: f32,
    pub has_eq: bool,
    pub has_comp: bool,
    pub stat: Stat,
    pub role: Role,
}

/// the moves for this mix
pub fn plan(tracks: &[TrackInfo], master: &Stat, master_volume: f32, style: &Style) -> Vec<Change> {
    let mut out: Vec<Change> = vec![];
    let mut power_before = 0f32;
    let mut power_after = 0f32;
    let mut crowd: Vec<(usize, Role)> = vec![];
    for (i, t) in tracks.iter().enumerate() {
        let s = &t.stat;
        if s.active_rms_db < -80.0 || s.active < 0.02 {
            continue;
        }
        // 1. the level of the track where it plays against the level its role wants
        let mut target = t.role.target_rms();
        if style.punch > 0.6 && matches!(t.role, Role::Drums | Role::Bass) {
            target += 1.0;
        }
        let delta = ((target - s.active_rms_db) * 0.8).clamp(-9.0, 9.0);
        let p = 10f32.powf(s.rms_db / 10.0);
        power_before += p;
        if delta.abs() >= 0.8 {
            let vol = (t.volume * 10f32.powf(delta / 20.0)).clamp(0.02, 1.5);
            out.push(Change { track: i as i32, what: "gain".into(), values: vec![vol], reason: format!("{} sits at {:.0} dB where it plays, a {} wants about {:.0}: fader {:+.1} dB", t.name, s.active_rms_db, t.role.name(), target, delta) });
            power_after += p * 10f32.powf(delta / 10.0);
        } else {
            power_after += p;
        }
        // 2. EQ: a low cut where the track has weight it does not need, a mud cut, brightness
        let (mut low, mut mid, mut high) = (0f32, 0f32, 0f32);
        let mut why: Vec<String> = vec![];
        let wants_bottom = matches!(t.role, Role::Bass | Role::Drums);
        if !wants_bottom && (s.bands[0] > -20.0 || s.bands[1] > -8.0) {
            low = -6.0;
            why.push(format!("low end ({:.0} dB below 250 Hz) that the bass and kick own: low shelf -6 dB", s.bands[1].max(s.bands[0])));
        }
        if matches!(t.role, Role::Chords | Role::Pad | Role::Other) && s.bands[2] > -7.0 {
            mid = -3.0;
            why.push(format!("a build-up at 250-800 Hz ({:.0} dB of its energy): mid -3 dB", s.bands[2]));
        }
        if s.bands[5] > -9.0 && style.brightness <= 0.0 && !matches!(t.role, Role::Drums) {
            high = -3.0;
            why.push(format!("a lot of air above 6 kHz ({:.0} dB): high shelf -3 dB", s.bands[5]));
        } else if style.brightness > 0.3 && matches!(t.role, Role::Lead | Role::Vocal) && s.bands[4] < -14.0 {
            high = 2.0 * style.brightness.min(1.0) + 1.0;
            why.push("dull presence: high shelf up a little for the brighter style".into());
        } else if style.brightness < -0.3 && matches!(t.role, Role::Lead | Role::Chords | Role::Pad) {
            high -= 1.5;
        }
        if low != 0.0 || mid != 0.0 || high != 0.0 {
            out.push(Change { track: i as i32, what: "eq".into(), values: vec![low, mid, high], reason: format!("{}: {}", t.name, why.join("; ")) });
        }
        // 3. dynamics
        if matches!(t.role, Role::Drums | Role::Bass | Role::Vocal) && !t.has_comp && s.crest_db > 15.0 - style.punch * 3.0 {
            let thr = (s.active_rms_db + 3.0).clamp(-40.0, -6.0);
            out.push(Change { track: i as i32, what: "comp".into(), values: vec![thr, 2.0 + style.punch * 2.0], reason: format!("{}: peaks {:.0} dB above its average, a {:.1}:1 compressor keeps it steady", t.name, s.crest_db, 2.0 + style.punch * 2.0) });
        }
        if matches!(t.role, Role::Chords | Role::Pad | Role::Other | Role::Lead) && t.pan.abs() < 0.05 {
            crowd.push((i, t.role));
        }
    }
    // 4. width: the instruments that compete for the middle are spread out, alternately left and right
    if style.width > 0.05 && crowd.len() >= 2 {
        let spread = [0.35f32, -0.35, 0.6, -0.6, 0.2, -0.2, 0.8, -0.8];
        for (k, (i, role)) in crowd.iter().enumerate() {
            // a lone lead stays in the middle
            if *role == Role::Lead && crowd.iter().filter(|(_, r)| *r == Role::Lead).count() == 1 {
                continue;
            }
            let p = spread[k % spread.len()] * style.width.clamp(0.0, 1.0) * 1.6;
            let p = p.clamp(-0.85, 0.85);
            out.push(Change { track: *i as i32, what: "pan".into(), values: vec![p], reason: format!("{} shares the middle with {} other instruments: pan {:+.0}%", tracks[*i].name, crowd.len() - 1, p * 100.0) });
        }
    }
    // 5. the master: the loudness the style wants
    let moved = if power_before > 0.0 { 10.0 * (power_after / power_before).log10() } else { 0.0 };
    let est = master.lufs + moved;
    let want = style.loudness - est;
    if master.lufs > -69.0 && want.abs() >= 0.7 {
        let mv = (master_volume * 10f32.powf(want.clamp(-12.0, 12.0) / 20.0)).clamp(0.2, 1.8);
        out.push(Change { track: -1, what: "master".into(), values: vec![mv], reason: format!("the mix is at about {:.1} LUFS after these moves; the master goes {:+.1} dB toward {:.0} LUFS (the limiter stays on)", est, want.clamp(-12.0, 12.0), style.loudness) });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, amp: f32, secs: f32, sr: f32) -> Vec<f32> {
        (0..(secs * sr) as usize).flat_map(|i| {
            let v = amp * (2.0 * PI * hz * i as f32 / sr).sin();
            [v, v]
        }).collect()
    }

    #[test]
    fn loudness_of_a_reference_sine() {
        // BS.1770: a 997 Hz sine at -20 dBFS peak on both channels reads about -20 LUFS... measured per the standard a full-scale
        // sine is -3.01 LUFS mono-equivalent per channel; stereo identical channels add 3 dB: -23.0 + ... compare relative steps instead
        let a = lufs(&sine(997.0, 0.1, 5.0, 48000.0), 48000.0);
        let b = lufs(&sine(997.0, 0.2, 5.0, 48000.0), 48000.0);
        assert!((b - a - 6.02).abs() < 0.1, "6 dB louder reads 6 dB louder: {a} {b}");
        assert!(a < -20.0 && a > -35.0, "{a}");
        // silence is silent
        assert!(lufs(&vec![0.0; 48000 * 2], 48000.0) <= -70.0);
    }

    #[test]
    fn bands_find_where_the_energy_is() {
        let sr = 44100.0;
        let low = analyze(&sine(100.0, 0.3, 3.0, sr), sr);
        assert!(low.bands[1] > -3.0, "100 Hz is in the bass band: {:?}", low.bands);
        assert!((low.centroid_hz - 100.0).abs() < 30.0, "{}", low.centroid_hz);
        let high = analyze(&sine(8000.0, 0.3, 3.0, sr), sr);
        assert!(high.bands[5] > -3.0, "{:?}", high.bands);
        assert!(low.width_db < -60.0, "identical channels are mono: {}", low.width_db);
        assert!(low.corr > 0.99);
    }

    #[test]
    fn plan_levels_by_role_and_leaves_a_balanced_mix_alone() {
        let quiet_pad = Stat { active_rms_db: -40.0, rms_db: -40.0, active: 0.9, crest_db: 8.0, bands: [-40.0, -20.0, -3.0, -10.0, -20.0, -30.0], lufs: -45.0, ..Stat::default() };
        let loud_bass = Stat { active_rms_db: -10.0, rms_db: -10.0, active: 0.9, crest_db: 18.0, bands: [-6.0, -1.0, -12.0, -25.0, -40.0, -60.0], lufs: -14.0, ..Stat::default() };
        let tracks = vec![
            TrackInfo { name: "Pad".into(), volume: 0.8, pan: 0.0, has_eq: false, has_comp: false, stat: quiet_pad, role: Role::Pad },
            TrackInfo { name: "Bass".into(), volume: 0.8, pan: 0.0, has_eq: false, has_comp: false, stat: loud_bass, role: Role::Bass },
        ];
        let master = Stat { lufs: -9.0, ..Stat::default() };
        let ch = plan(&tracks, &master, 1.0, &Style::default());
        let pad_gain = ch.iter().find(|c| c.track == 0 && c.what == "gain").expect("pad level");
        assert!(pad_gain.values[0] > 0.8, "the quiet pad comes up: {:?}", pad_gain);
        let bass_gain = ch.iter().find(|c| c.track == 1 && c.what == "gain").expect("bass level");
        assert!(bass_gain.values[0] < 0.8, "the loud bass comes down");
        assert!(ch.iter().any(|c| c.track == 0 && c.what == "eq" && c.values[1] < 0.0), "the pad mud is cut");
        assert!(ch.iter().any(|c| c.track == 1 && c.what == "comp"), "the dynamic bass is compressed");
        assert!(ch.iter().any(|c| c.track == -1 && c.what == "master"), "the master heads for the loudness target");
        // a mix already on target changes nothing
        let ok_stat = Stat { active_rms_db: -24.0, rms_db: -24.0, active: 0.9, crest_db: 10.0, bands: [-50.0, -30.0, -8.0, -12.0, -20.0, -30.0], lufs: -14.0, ..Stat::default() };
        let t2 = vec![TrackInfo { name: "Keys".into(), volume: 0.8, pan: 0.3, has_eq: false, has_comp: false, stat: ok_stat, role: Role::Chords }];
        assert!(plan(&t2, &Stat { lufs: -14.0, ..Stat::default() }, 1.0, &Style::default()).is_empty());
    }

    #[test]
    fn roles_come_from_names_and_kinds() {
        let s = Stat::default();
        assert_eq!(guess_role("Kick", "synth", &s), Role::Drums);
        assert_eq!(guess_role("Lead Violin", "synth", &s), Role::Lead);
        assert_eq!(guess_role("Sub Bass", "synth", &s), Role::Bass);
        assert_eq!(guess_role("x", "voice", &s), Role::Vocal);
        assert_eq!(Role::parse("Vocals"), Some(Role::Vocal));
    }
}
