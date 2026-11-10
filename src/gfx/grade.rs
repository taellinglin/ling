//! Color grading: a 64x64x64 look-up table run over the finished 3-D frame (after the
//! toon / bloom / FXAA post chain, before the UI is drawn), the "LUT" pass of a film
//! pipeline that pulls every color in the scene toward one look.
//!
//! `set_color_grade(preset, strength)` builds the table from a built-in look
//! ("heavy", "warm", "cool", "vivid", "noir"; "off" / "" clears it);
//! `set_color_lut(path, strength)` loads a standard strip LUT image (N slices of NxN
//! side by side, e.g. 1024x32 or 4096x64: red across each slice, green down, blue
//! by slice). One table read per pixel (6 bits per channel, nearest), so a full
//! 1080p frame costs ~1-2 ms; the top byte of each pixel (the renderer's flags) is kept.
use std::sync::{Arc, Mutex, OnceLock};

const N: usize = 64;

fn slot() -> &'static Mutex<Option<Arc<Vec<u32>>>> {
    static S: OnceLock<Mutex<Option<Arc<Vec<u32>>>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

/// The active table (None = grading off), with the hue cycle applied when one is on.
pub fn current() -> Option<Arc<Vec<u32>>> {
    let base = slot().lock().ok().and_then(|g| g.clone())?;
    Some(cycled(base))
}

// ── hue cycling (set_color_cycle): the table's outputs are turned around the grey
// axis by an angle that oscillates on its own period in each brightness band
// (shadows / midtones / highlights), so the whole palette slowly flows ──
#[derive(Clone, Copy)]
struct Cycle {
    amount: f32,      // peak rotation, radians (0 = off)
    period: [f32; 3], // seconds per oscillation: shadows, mids, highlights
}
struct CycleState {
    cycle: Cycle,
    built_at: f32,
    base_ptr: usize,
    lut: Option<Arc<Vec<u32>>>,
}
fn cycle_slot() -> &'static Mutex<CycleState> {
    static S: OnceLock<Mutex<CycleState>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(CycleState { cycle: Cycle { amount: 0.0, period: [61.0, 37.0, 23.0] }, built_at: -99.0, base_ptr: 0, lut: None }))
}
fn clock() -> f32 {
    static T0: OnceLock<std::time::Instant> = OnceLock::new();
    T0.get_or_init(std::time::Instant::now).elapsed().as_secs_f32()
}
/// `set_color_cycle(degrees, shadow_s, mid_s, high_s)`: 0 degrees turns it off.
pub fn set_cycle(degrees: f32, periods: [f32; 3]) {
    if let Ok(mut c) = cycle_slot().lock() {
        c.cycle = Cycle { amount: degrees.clamp(0.0, 180.0).to_radians(), period: periods.map(|p| p.max(1.0)) };
        c.built_at = -99.0;
    }
}
/// rotate a packed rgb around the grey axis by `a` radians
fn hue_rotate(c: u32, a: f32) -> u32 {
    let (r, g, b) = (((c >> 16) & 255) as f32, ((c >> 8) & 255) as f32, (c & 255) as f32);
    let (s, co) = (a.sin(), a.cos());
    let k1 = (1.0 - co) / 3.0;
    let k2 = s * 0.577_350_3;
    let (h0, h1, h2) = (co + k1, k1 - k2, k1 + k2);
    let q = |v: f32| v.clamp(0.0, 255.0) as u32;
    (q(r * h0 + g * h1 + b * h2) << 16) | (q(r * h2 + g * h0 + b * h1) << 8) | q(r * h1 + g * h2 + b * h0)
}
fn cycled(base: Arc<Vec<u32>>) -> Arc<Vec<u32>> {
    let Ok(mut st) = cycle_slot().lock() else { return base };
    if st.cycle.amount <= 0.0 {
        return base;
    }
    let now = clock();
    let ptr = Arc::as_ptr(&base) as usize;
    if let Some(l) = &st.lut {
        if st.base_ptr == ptr && now - st.built_at < 0.16 {
            return l.clone();
        }
    }
    let cy = st.cycle;
    let tau = std::f32::consts::TAU;
    // each band's angle now; a color's angle is its band-weighted blend (by brightness)
    let ang = [0usize, 1, 2].map(|i| cy.amount * (now * tau / cy.period[i] + i as f32 * 2.1).sin());
    let f = |c: u32| -> u32 {
        let l = (0.2126 * ((c >> 16) & 255) as f32 + 0.7152 * ((c >> 8) & 255) as f32 + 0.0722 * (c & 255) as f32) / 255.0;
        let ws = (1.0 - l * 2.0).clamp(0.0, 1.0);
        let wh = (l * 2.0 - 1.0).clamp(0.0, 1.0);
        let wm = 1.0 - ws - wh;
        hue_rotate(c, ang[0] * ws + ang[1] * wm + ang[2] * wh)
    };
    #[cfg(not(target_arch = "wasm32"))]
    let out: Vec<u32> = {
        use rayon::prelude::*;
        base.par_iter().map(|&c| f(c)).collect()
    };
    #[cfg(target_arch = "wasm32")]
    let out: Vec<u32> = base.iter().map(|&c| f(c)).collect();
    let out = Arc::new(out);
    st.lut = Some(out.clone());
    st.base_ptr = ptr;
    st.built_at = now;
    out
}

pub fn clear() {
    if let Ok(mut g) = slot().lock() {
        *g = None;
    }
}

fn set(lut: Vec<u32>) {
    if let Ok(mut g) = slot().lock() {
        *g = Some(Arc::new(lut));
    }
}

fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// One color through a built-in look (inputs / outputs 0..1).
pub fn preset_color(name: &str, r: f32, g: f32, b: f32) -> [f32; 3] {
    let lum = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let sat = |c: [f32; 3], s: f32| {
        let l = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        [l + (c[0] - l) * s, l + (c[1] - l) * s, l + (c[2] - l) * s]
    };
    // a contrast S-curve around mid-grey: k blends a straight line toward smoothstep
    let curve = |x: f32, k: f32| x + (smooth(x) - x) * k;
    let c = match name {
        // the heavy filmic grade: punchy S-curve, teal shadows, warm highlights, rich color
        "heavy" => {
            let s = sat([r, g, b], 1.38);
            let mut c = [curve(s[0], 0.85), curve(s[1], 0.85), curve(s[2], 0.85)];
            let sh = (1.0 - lum).powi(2);
            let hi = lum * lum;
            c[0] += -0.05 * sh + 0.09 * hi;
            c[1] += 0.03 * sh + 0.035 * hi;
            c[2] += 0.08 * sh - 0.07 * hi;
            // lifted, slightly blue blacks
            [c[0] * 0.96 + 0.012, c[1] * 0.96 + 0.016, c[2] * 0.96 + 0.026]
        },
        "warm" => {
            let s = sat([r, g, b], 1.12);
            [curve(s[0], 0.35) * 1.06 + 0.02, curve(s[1], 0.35) * 1.01 + 0.008, curve(s[2], 0.35) * 0.88]
        },
        "cool" => {
            let s = sat([r, g, b], 1.05);
            [curve(s[0], 0.4) * 0.9, curve(s[1], 0.4) * 1.0 + 0.01, curve(s[2], 0.4) * 1.08 + 0.03]
        },
        "vivid" => {
            let s = sat([r, g, b], 1.55);
            [curve(s[0], 0.6), curve(s[1], 0.6), curve(s[2], 0.6)]
        },
        // night: the world sinks into deep navy and its saturated colors glow like a
        // bioluminescent forest -- foliage neon teal-green, blues electric cyan, warm
        // colors magenta; the brighter and more colorful, the stronger the glow
        "biolum" => {
            let mx = r.max(g).max(b);
            let mn = r.min(g).min(b);
            let chroma = mx - mn;
            // most of the night is dark: a deep navy, a quarter of the day's light
            let base = [r * 0.20 + 0.010, g * 0.25 + 0.022, b * 0.40 + 0.065];
            // only strongly colored, bright things glow; foliage most, warm ground least
            let (glow, gain) = if g >= r && g >= b {
                ([0.10, 1.0, 0.70], 1.0)
            } else if b >= r {
                ([0.18, 0.75, 1.0], 0.85)
            } else {
                ([0.62, 0.16, 0.78], 0.35)
            };
            let k = smooth((chroma - 0.12) * 2.6) * smooth((lum - 0.18) * 2.4) * 0.75 * gain;
            let lift = 0.35 + lum * 0.6;
            [base[0] + glow[0] * k * lift, base[1] + glow[1] * k * lift, base[2] + glow[2] * k * lift]
        },
        "noir" => {
            let l = curve(lum, 1.0);
            [l * 1.02, l, l * 0.97]
        },
        _ => [r, g, b],
    };
    [c[0].clamp(0.0, 1.0), c[1].clamp(0.0, 1.0), c[2].clamp(0.0, 1.0)]
}

fn pack(c: [f32; 3]) -> u32 {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    (q(c[0]) << 16) | (q(c[1]) << 8) | q(c[2])
}

/// Build the 64^3 table for `f`, blended toward identity by (1 - strength).
fn build(strength: f32, f: impl Fn(f32, f32, f32) -> [f32; 3]) -> Vec<u32> {
    let s = strength.clamp(0.0, 1.0);
    let mut lut = Vec::with_capacity(N * N * N);
    for ri in 0..N {
        for gi in 0..N {
            for bi in 0..N {
                let (r, g, b) = (ri as f32 / (N - 1) as f32, gi as f32 / (N - 1) as f32, bi as f32 / (N - 1) as f32);
                let o = f(r, g, b);
                lut.push(pack([r + (o[0] - r) * s, g + (o[1] - g) * s, b + (o[2] - b) * s]));
            }
        }
    }
    lut
}

/// `set_color_grade(name, strength)`: false for an unknown look (and grading is cleared).
pub fn set_preset(name: &str, strength: f32) -> bool {
    let n = name.trim().to_ascii_lowercase();
    if n.is_empty() || n == "off" || n == "none" || strength <= 0.0 {
        clear();
        return true;
    }
    if !matches!(n.as_str(), "heavy" | "warm" | "cool" | "vivid" | "noir" | "biolum") {
        clear();
        return false;
    }
    set(build(strength, |r, g, b| preset_color(&n, r, g, b)));
    true
}

/// `set_color_grade_mix(a, b, t, strength)`: look `a` blended toward look `b` by t (0 = all a,
/// 1 = all b; "off" = no grade) -- e.g. the day grade into "biolum" as night falls.
pub fn set_mix(a: &str, b: &str, t: f32, strength: f32) -> bool {
    let (a, b) = (a.trim().to_ascii_lowercase(), b.trim().to_ascii_lowercase());
    let t = t.clamp(0.0, 1.0);
    let known = |n: &str| matches!(n, "heavy" | "warm" | "cool" | "vivid" | "noir" | "biolum" | "off" | "none" | "");
    if !known(&a) || !known(&b) {
        return false;
    }
    if strength <= 0.0 {
        clear();
        return true;
    }
    set(build(strength, |r, g, bb| {
        let ca = preset_color(&a, r, g, bb);
        let cb = preset_color(&b, r, g, bb);
        [ca[0] + (cb[0] - ca[0]) * t, ca[1] + (cb[1] - ca[1]) * t, ca[2] + (cb[2] - ca[2]) * t]
    }));
    true
}

/// `set_color_lut(path, strength)` from decoded RGBA pixels of a strip LUT image.
pub fn set_from_strip(px: &[u8], w: usize, h: usize, strength: f32) -> bool {
    // a strip of `h` slices, each h x h: w == h * h
    if h < 2 || w != h * h || px.len() < w * h * 4 {
        return false;
    }
    let size = h;
    let at = |ri: usize, gi: usize, bi: usize| -> [f32; 3] {
        let x = bi * size + ri;
        let y = gi;
        let o = (y * w + x) * 4;
        [px[o] as f32 / 255.0, px[o + 1] as f32 / 255.0, px[o + 2] as f32 / 255.0]
    };
    let m = (size - 1) as f32;
    set(build(strength, |r, g, b| {
        // trilinear sample of the strip's own grid
        let (fr, fg, fb) = (r * m, g * m, b * m);
        let (r0, g0, b0) = (fr.floor() as usize, fg.floor() as usize, fb.floor() as usize);
        let (r1, g1, b1) = ((r0 + 1).min(size - 1), (g0 + 1).min(size - 1), (b0 + 1).min(size - 1));
        let (tr, tg, tb) = (fr - r0 as f32, fg - g0 as f32, fb - b0 as f32);
        let lerp = |a: [f32; 3], c: [f32; 3], t: f32| [a[0] + (c[0] - a[0]) * t, a[1] + (c[1] - a[1]) * t, a[2] + (c[2] - a[2]) * t];
        let c00 = lerp(at(r0, g0, b0), at(r1, g0, b0), tr);
        let c10 = lerp(at(r0, g1, b0), at(r1, g1, b0), tr);
        let c01 = lerp(at(r0, g0, b1), at(r1, g0, b1), tr);
        let c11 = lerp(at(r0, g1, b1), at(r1, g1, b1), tr);
        lerp(lerp(c00, c10, tg), lerp(c01, c11, tg), tb)
    }));
    true
}

#[inline]
fn grade_px(lut: &[u32], p: u32) -> u32 {
    // emissive: a near-white pixel (a star, a lamp core, a spell's hot centre) or one
    // with a channel at full drive (firework sparks, which push their hue to 255) keeps
    // its own colour, so night grading and fog tints never dim what should shine
    let (r, g, bl) = ((p >> 16) & 255, (p >> 8) & 255, p & 255);
    if (r >= 220 && g >= 220 && bl >= 220) || r.max(g).max(bl) >= 250 {
        return p;
    }
    let i = (((p >> 18) & 63) << 12) | (((p >> 10) & 63) << 6) | ((p >> 2) & 63);
    (p & 0xFF00_0000) | lut[i as usize]
}

/// Grade the frame in place (no-op while grading is off).
pub fn apply(buf: &mut [u32]) {
    let Some(lut) = current() else { return };
    #[cfg(not(target_arch = "wasm32"))]
    {
        use rayon::prelude::*;
        buf.par_chunks_mut(8192).for_each(|c| {
            for p in c.iter_mut() {
                *p = grade_px(&lut, *p);
            }
        });
    }
    #[cfg(target_arch = "wasm32")]
    for p in buf.iter_mut() {
        *p = grade_px(&lut, *p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_at_zero_strength_and_heavy_changes_color() {
        assert!(set_preset("heavy", 1.0));
        let mut px = vec![0x0080_4020u32, 0xFF20_6090];
        apply(&mut px);
        assert_ne!(px[0], 0x0080_4020);
        assert_eq!(px[1] >> 24, 0xFF, "flag byte kept");
        assert!(set_preset("off", 1.0));
        assert!(current().is_none());
        assert!(!set_preset("nonsense", 1.0));
    }
}
