//! Professional vector UI toolkit — HUD overlays, meters/gauges, interface
//! controls and game UI, all emitted as resolved vector primitives so a single
//! widget can be multi-colour and the host can rasterize (AA fill + AA stroke)
//! however it likes. Pure, allocation-light, immediate-mode: call a function
//! each frame with the live params and draw the returned [`Draw`].
//!
//! Conventions: screen space, x→right, **y→down**, origin top-left. Colours are
//! packed `0x00RRGGBB` (the Ling framebuffer format). Interactive state
//! (hover/active/value) is passed in by the host; these functions are pure
//! geometry and never read input themselves.

use crate::holo;
use core::f32::consts::PI;

/// Packed `0x00RRGGBB` colour.
pub type Rgba = u32;

/// Resolved vector output: filled polygons + stroked polylines, each with its
/// own colour so one widget can mix theme slots (track vs fill vs accent).
#[derive(Default, Clone)]
pub struct Draw {
    pub fills: Vec<(Rgba, Vec<[f32; 2]>)>,
    pub strokes: Vec<(Rgba, Vec<[f32; 2]>)>,
}

impl Draw {
    fn new() -> Self {
        Self::default()
    }

    fn fill(&mut self, c: Rgba, poly: Vec<[f32; 2]>) {
        self.fills.push((c, poly));
    }

    fn stroke(&mut self, c: Rgba, pl: Vec<[f32; 2]>) {
        self.strokes.push((c, pl));
    }

    /// Stroke a list of disjoint segments `[x0,y0,x1,y1]` (e.g. from `holo`).
    #[allow(dead_code)] // helper kept for holo/segment widgets (not yet called)
    fn segs(&mut self, c: Rgba, segs: &[[f32; 4]]) {
        for s in segs {
            self.strokes.push((c, vec![[s[0], s[1]], [s[2], s[3]]]));
        }
    }

    fn rect_outline(&mut self, c: Rgba, x: f32, y: f32, w: f32, h: f32) {
        self.stroke(
            c,
            vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h], [x, y]],
        );
    }

    fn rect_fill(&mut self, c: Rgba, x: f32, y: f32, w: f32, h: f32) {
        self.fill(
            c,
            vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h], [x, y]],
        );
    }
}

// ── small helpers ─────────────────────────────────────────────────────────────

#[inline]
fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

/// Mix two packed colours by `t∈[0,1]`.
pub fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = clamp01(t);
    let lerp = |x: u32, y: u32| (x as f32 + (y as f32 - x as f32) * t) as u32 & 0xFF;
    let (ar, ag, ab) = (a >> 16 & 0xFF, a >> 8 & 0xFF, a & 0xFF);
    let (br, bg, bb) = (b >> 16 & 0xFF, b >> 8 & 0xFF, b & 0xFF);
    (lerp(ar, br) << 16) | (lerp(ag, bg) << 8) | lerp(ab, bb)
}

/// Scale a colour's brightness by `k` (clamped).
pub fn shade(c: Rgba, k: f32) -> Rgba {
    let f = |x: u32| ((x as f32 * k).clamp(0.0, 255.0)) as u32;
    (f(c >> 16 & 0xFF) << 16) | (f(c >> 8 & 0xFF) << 8) | f(c & 0xFF)
}

/// Arc polyline from `a0` to `a1` radians (y-down screen space), `n` segments.
fn arc(cx: f32, cy: f32, r: f32, a0: f32, a1: f32, n: usize) -> Vec<[f32; 2]> {
    let n = n.max(1);
    (0..=n)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / n as f32;
            [cx + a.cos() * r, cy + a.sin() * r]
        })
        .collect()
}

/// Beveled (cut-corner) panel polygon — the holographic panel silhouette.
fn bevel_poly(x: f32, y: f32, w: f32, h: f32, b: f32) -> Vec<[f32; 2]> {
    let b = b.min(w * 0.5).min(h * 0.5);
    let (x1, y1) = (x + w, y + h);
    vec![
        [x + b, y],
        [x1 - b, y],
        [x1, y + b],
        [x1, y1 - b],
        [x1 - b, y1],
        [x + b, y1],
        [x, y1 - b],
        [x, y + b],
        [x + b, y],
    ]
}

// ════════════════════════════════════════════════════════════════════════════
// HUD / sci-fi overlays
// ════════════════════════════════════════════════════════════════════════════

/// Circular radar with grid rings, cross axes and a sweeping wedge at `sweep`
/// radians. `blip` ∈ [0,1] fraction along the sweep where a contact pings.
pub fn radar(
    cx: f32,
    cy: f32,
    r: f32,
    sweep: f32,
    primary: Rgba,
    accent: Rgba,
    track: Rgba,
) -> Draw {
    let mut d = Draw::new();
    for k in 1..=3 {
        d.stroke(track, arc(cx, cy, r * k as f32 / 3.0, 0.0, PI * 2.0, 48));
    }
    d.stroke(track, vec![[cx - r, cy], [cx + r, cy]]);
    d.stroke(track, vec![[cx, cy - r], [cx, cy + r]]);
    // sweep wedge
    let sw = 0.32;
    let mut wedge = vec![[cx, cy]];
    wedge.extend(arc(cx, cy, r, sweep - sw, sweep, 10));
    wedge.push([cx, cy]);
    d.fill(shade(accent, 0.5), wedge);
    d.stroke(
        accent,
        vec![[cx, cy], [cx + sweep.cos() * r, cy + sweep.sin() * r]],
    );
    // contact ping
    let pr = r * 0.62;
    let pa = sweep - 0.15;
    let (bx, by) = (cx + pa.cos() * pr, cy + pa.sin() * pr);
    d.fill(primary, arc(bx, by, 3.5, 0.0, PI * 2.0, 10));
    d
}

/// Horizontal compass strip centred on `heading` (degrees). Width `w`, ticks
/// every 15°, cardinal marks taller.
pub fn compass(x: f32, y: f32, w: f32, h: f32, heading: f32, primary: Rgba, track: Rgba) -> Draw {
    let mut d = Draw::new();
    d.rect_outline(track, x, y, w, h);
    let cx = x + w * 0.5;
    let deg_per_px = 90.0 / w; // show ~90° across
    let start = (heading - w * 0.5 * deg_per_px).floor() as i32;
    let end = (heading + w * 0.5 * deg_per_px).ceil() as i32;
    let mut deg = start - (start.rem_euclid(15));
    while deg <= end {
        let px = cx + (deg as f32 - heading) / deg_per_px;
        if px >= x && px <= x + w {
            let card = deg.rem_euclid(90) == 0;
            let th = if card { h * 0.6 } else { h * 0.3 };
            d.stroke(
                if card { primary } else { track },
                vec![[px, y + h], [px, y + h - th]],
            );
        }
        deg += 15;
    }
    // centre marker
    d.stroke(
        primary,
        vec![
            [cx, y - 4.0],
            [cx - 5.0, y - 12.0],
            [cx + 5.0, y - 12.0],
            [cx, y - 4.0],
        ],
    );
    d
}

/// Aiming reticle: ring + tick gaps + centre dot. `spread` ∈ [0,1] opens the gaps.
pub fn reticle(cx: f32, cy: f32, r: f32, spread: f32, primary: Rgba) -> Draw {
    let mut d = Draw::new();
    let g = 0.25 + spread * 0.5;
    for q in 0..4 {
        let base = q as f32 * PI * 0.5 + PI * 0.25;
        d.stroke(
            primary,
            arc(cx, cy, r, base + g * 0.5, base + PI * 0.5 - g * 0.5, 10),
        );
    }
    let inner = r * 0.45 + spread * r * 0.4;
    for k in 0..4 {
        let a = k as f32 * PI * 0.5;
        d.stroke(
            primary,
            vec![
                [cx + a.cos() * inner, cy + a.sin() * inner],
                [cx + a.cos() * (inner + 8.0), cy + a.sin() * (inner + 8.0)],
            ],
        );
    }
    d.fill(primary, arc(cx, cy, 1.8, 0.0, PI * 2.0, 8));
    d
}

/// Animated target brackets that close in as `lock` → 1 (0 = wide, 1 = snug).
pub fn target(x: f32, y: f32, w: f32, h: f32, lock: f32, primary: Rgba, accent: Rgba) -> Draw {
    let mut d = Draw::new();
    let off = (1.0 - clamp01(lock)) * 14.0;
    let col = mix(primary, accent, clamp01(lock));
    let l = (w.min(h)) * 0.28;
    let (xx, yy, ww, hh) = (x - off, y - off, w + off * 2.0, h + off * 2.0);
    for seg in holo::corner_brackets(xx, yy, ww, hh, l) {
        d.stroke(col, vec![[seg[0], seg[1]], [seg[2], seg[3]]]);
    }
    if lock > 0.5 {
        d.stroke(col, vec![[x + w * 0.5, y - off - 6.0], [x + w * 0.5, y]]);
    }
    d
}

/// Sci-fi panel: beveled outline + faint fill + a header tab notch.
pub fn panel(x: f32, y: f32, w: f32, h: f32, bevel: f32, primary: Rgba, bg: Rgba) -> Draw {
    let mut d = Draw::new();
    d.fill(bg, bevel_poly(x, y, w, h, bevel));
    d.stroke(primary, bevel_poly(x, y, w, h, bevel));
    // header bar
    let hb = 16.0_f32.min(h * 0.25);
    d.stroke(primary, vec![[x + bevel, y + hb], [x + w - bevel, y + hb]]);
    d.fill(
        shade(primary, 0.35),
        vec![
            [x + bevel, y],
            [x + w * 0.4, y],
            [x + w * 0.4 - 6.0, y + hb],
            [x + bevel, y + hb],
        ],
    );
    d
}

/// Horizontal scanline overlay inside a rect (`density` lines).
pub fn scanlines(x: f32, y: f32, w: f32, h: f32, density: usize, line: Rgba) -> Draw {
    let mut d = Draw::new();
    let n = density.max(1);
    for i in 0..n {
        let py = y + h * i as f32 / n as f32;
        d.stroke(line, vec![[x, py], [x + w, py]]);
    }
    d
}

// ════════════════════════════════════════════════════════════════════════════
// Meters & gauges
// ════════════════════════════════════════════════════════════════════════════

/// Linear bar: track outline + proportional fill (`value/max`).
pub fn bar(x: f32, y: f32, w: f32, h: f32, frac: f32, fill: Rgba, track: Rgba) -> Draw {
    let mut d = Draw::new();
    d.rect_outline(track, x, y, w, h);
    let fw = w * clamp01(frac);
    if fw > 0.5 {
        d.rect_fill(fill, x + 1.0, y + 1.0, (fw - 2.0).max(0.0), h - 2.0);
    }
    d
}

/// Segmented/notched bar — `segs` cells, `frac` lit proportionally.
#[allow(clippy::too_many_arguments)]
pub fn segbar(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    frac: f32,
    segs: usize,
    fill: Rgba,
    track: Rgba,
) -> Draw {
    let mut d = Draw::new();
    let n = segs.max(1);
    let gap = 2.0;
    let cw = (w - gap * (n as f32 - 1.0)) / n as f32;
    let lit = (clamp01(frac) * n as f32).round() as usize;
    for i in 0..n {
        let cx = x + i as f32 * (cw + gap);
        if i < lit {
            d.rect_fill(fill, cx, y, cw, h);
        } else {
            d.rect_outline(track, cx, y, cw, h);
        }
    }
    d
}

/// Radial dial gauge with tick marks and a needle. Sweeps 225°→ -45° (270° arc).
pub fn gauge(cx: f32, cy: f32, r: f32, frac: f32, needle: Rgba, accent: Rgba, track: Rgba) -> Draw {
    let mut d = Draw::new();
    let a0 = PI * 0.75; // 135° (down-left)
    let a1 = PI * 0.25 + PI * 2.0; // wrap to up-right (270° sweep clockwise)
    let span = a1 - a0;
    d.stroke(track, arc(cx, cy, r, a0, a1, 60));
    // colored progress arc
    d.stroke(accent, arc(cx, cy, r, a0, a0 + span * clamp01(frac), 60));
    // ticks
    for i in 0..=10 {
        let a = a0 + span * i as f32 / 10.0;
        let (c, s) = (a.cos(), a.sin());
        d.stroke(
            track,
            vec![
                [cx + c * r, cy + s * r],
                [cx + c * (r - 7.0), cy + s * (r - 7.0)],
            ],
        );
    }
    // needle
    let a = a0 + span * clamp01(frac);
    d.stroke(
        needle,
        vec![
            [cx, cy],
            [cx + a.cos() * (r - 4.0), cy + a.sin() * (r - 4.0)],
        ],
    );
    d.fill(needle, arc(cx, cy, 3.0, 0.0, PI * 2.0, 8));
    d
}

/// Ring / arc progress meter (full circle minus a gap at the bottom).
pub fn ring(cx: f32, cy: f32, r: f32, frac: f32, fill: Rgba, track: Rgba) -> Draw {
    let mut d = Draw::new();
    let a0 = PI * 0.5 + 0.4;
    let a1 = PI * 0.5 - 0.4 + PI * 2.0;
    let span = a1 - a0;
    d.stroke(track, arc(cx, cy, r, a0, a1, 64));
    d.stroke(fill, arc(cx, cy, r, a0, a0 + span * clamp01(frac), 64));
    d
}

/// VU / spectrum bars from a list of levels (each 0..1).
pub fn vu(x: f32, y: f32, w: f32, h: f32, levels: &[f32], fill: Rgba, peak: Rgba) -> Draw {
    let mut d = Draw::new();
    let n = levels.len().max(1);
    let gap = 2.0;
    let bw = (w - gap * (n as f32 - 1.0)) / n as f32;
    for (i, &lv) in levels.iter().enumerate() {
        let bx = x + i as f32 * (bw + gap);
        let bh = h * clamp01(lv);
        let c = mix(fill, peak, clamp01(lv));
        d.rect_fill(c, bx, y + h - bh, bw, bh);
    }
    d
}

/// Sparkline polyline from a list of values (auto-scaled to [min,max]).
pub fn spark(x: f32, y: f32, w: f32, h: f32, vals: &[f32], line: Rgba) -> Draw {
    let mut d = Draw::new();
    if vals.len() < 2 {
        return d;
    }
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for &v in vals {
        lo = lo.min(v);
        hi = hi.max(v);
    }
    let rng = (hi - lo).max(1e-6);
    let pl: Vec<[f32; 2]> = vals
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            [
                x + w * i as f32 / (vals.len() as f32 - 1.0),
                y + h - (v - lo) / rng * h,
            ]
        })
        .collect();
    d.stroke(line, pl);
    d
}

/// Battery cell with bezel, terminal nub and proportional charge fill.
#[allow(clippy::too_many_arguments)]
pub fn battery(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    frac: f32,
    fill: Rgba,
    track: Rgba,
    warn: Rgba,
) -> Draw {
    let mut d = Draw::new();
    let bw = w - 4.0;
    d.rect_outline(track, x, y, bw, h);
    d.rect_fill(track, x + bw, y + h * 0.3, 4.0, h * 0.4); // terminal
    let f = clamp01(frac);
    let c = if f < 0.25 { warn } else { fill };
    if f > 0.0 {
        d.rect_fill(c, x + 2.0, y + 2.0, (bw - 4.0) * f, h - 4.0);
    }
    d
}

// ════════════════════════════════════════════════════════════════════════════
// Interface controls  (host supplies hover/active/value; widget = geometry)
// ════════════════════════════════════════════════════════════════════════════

/// Beveled button background (label drawn by the host). `hover`/`active` brighten.
#[allow(clippy::too_many_arguments)]
pub fn button(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    hover: bool,
    active: bool,
    primary: Rgba,
    bg: Rgba,
) -> Draw {
    let mut d = Draw::new();
    let glow = if active {
        0.55
    } else if hover {
        0.3
    } else {
        0.12
    };
    d.fill(shade(primary, glow), bevel_poly(x, y, w, h, 8.0));
    d.stroke(
        if hover {
            mix(primary, 0xFFFFFF, 0.4)
        } else {
            primary
        },
        bevel_poly(x, y, w, h, 8.0),
    );
    let _ = bg;
    d
}

/// Toggle switch; `on` slides the knob and recolours the rail.
pub fn toggle(x: f32, y: f32, w: f32, h: f32, on: bool, on_col: Rgba, track: Rgba) -> Draw {
    let mut d = Draw::new();
    let r = h * 0.5;
    let rail = if on { on_col } else { track };
    // rounded rail
    let mut p = arc(x + r, y + r, r, PI * 0.5, PI * 1.5, 12);
    p.extend(arc(x + w - r, y + r, r, PI * 1.5, PI * 2.5, 12));
    p.push(p[0]);
    d.stroke(rail, p);
    let kx = if on { x + w - r } else { x + r };
    d.fill(rail, arc(kx, y + r, r - 2.0, 0.0, PI * 2.0, 16));
    d
}

/// Horizontal slider: track + filled portion + draggable knob at `frac`.
pub fn slider(x: f32, y: f32, w: f32, frac: f32, hover: bool, fill: Rgba, track: Rgba) -> Draw {
    let mut d = Draw::new();
    d.stroke(track, vec![[x, y], [x + w, y]]);
    let kx = x + w * clamp01(frac);
    d.stroke(fill, vec![[x, y], [kx, y]]);
    let kr = if hover { 8.0 } else { 6.0 };
    d.fill(fill, arc(kx, y, kr, 0.0, PI * 2.0, 16));
    d.stroke(mix(fill, 0xFFFFFF, 0.5), arc(kx, y, kr, 0.0, PI * 2.0, 16));
    d
}

/// Checkbox; `checked` draws a tick.
pub fn checkbox(
    x: f32,
    y: f32,
    s: f32,
    checked: bool,
    hover: bool,
    primary: Rgba,
    track: Rgba,
) -> Draw {
    let mut d = Draw::new();
    d.rect_outline(
        if hover {
            mix(primary, 0xFFFFFF, 0.4)
        } else {
            track
        },
        x,
        y,
        s,
        s,
    );
    if checked {
        d.stroke(
            primary,
            vec![
                [x + s * 0.22, y + s * 0.55],
                [x + s * 0.42, y + s * 0.75],
                [x + s * 0.8, y + s * 0.25],
            ],
        );
    }
    d
}

/// Tab strip: `count` tabs, `active` highlighted. Returns per-tab rects via geometry.
#[allow(clippy::too_many_arguments)]
pub fn tabs(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    count: usize,
    active: usize,
    hover: i32,
    primary: Rgba,
    track: Rgba,
) -> Draw {
    let mut d = Draw::new();
    let n = count.max(1);
    let tw = w / n as f32;
    for i in 0..n {
        let tx = x + i as f32 * tw;
        if i == active {
            d.fill(
                shade(primary, 0.3),
                vec![
                    [tx, y],
                    [tx + tw, y],
                    [tx + tw, y + h],
                    [tx, y + h],
                    [tx, y],
                ],
            );
            d.stroke(
                primary,
                vec![[tx, y + h], [tx, y], [tx + tw, y], [tx + tw, y + h]],
            );
        } else {
            let c = if hover == i as i32 {
                mix(track, primary, 0.5)
            } else {
                track
            };
            d.stroke(c, vec![[tx, y + h], [tx + tw, y + h]]);
        }
    }
    d
}

/// Progress bar with subtle internal chevrons.
pub fn progress(x: f32, y: f32, w: f32, h: f32, frac: f32, fill: Rgba, track: Rgba) -> Draw {
    let mut d = Draw::new();
    d.rect_outline(track, x, y, w, h);
    let fw = (w - 2.0) * clamp01(frac);
    if fw > 0.5 {
        d.rect_fill(fill, x + 1.0, y + 1.0, fw, h - 2.0);
    }
    d
}

/// Tooltip / callout bubble with a pointer (label drawn by host).
pub fn tooltip(x: f32, y: f32, w: f32, h: f32, primary: Rgba, bg: Rgba) -> Draw {
    let mut d = Draw::new();
    d.fill(bg, bevel_poly(x, y, w, h, 6.0));
    d.stroke(primary, bevel_poly(x, y, w, h, 6.0));
    d.fill(
        bg,
        vec![
            [x + 12.0, y + h],
            [x + 22.0, y + h],
            [x + 14.0, y + h + 8.0],
        ],
    );
    d.stroke(
        primary,
        vec![
            [x + 12.0, y + h],
            [x + 14.0, y + h + 8.0],
            [x + 22.0, y + h],
        ],
    );
    d
}

/// Stepper: [ - value + ] frame (host draws value). Returns the two button rects' geometry.
#[allow(clippy::too_many_arguments)]
pub fn stepper(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    hover_minus: bool,
    hover_plus: bool,
    primary: Rgba,
    track: Rgba,
) -> Draw {
    let mut d = Draw::new();
    let bw = h;
    d.fill(
        shade(primary, if hover_minus { 0.4 } else { 0.12 }),
        bevel_poly(x, y, bw, h, 5.0),
    );
    d.stroke(primary, bevel_poly(x, y, bw, h, 5.0));
    d.stroke(
        primary,
        vec![[x + bw * 0.3, y + h * 0.5], [x + bw * 0.7, y + h * 0.5]],
    );
    let px = x + w - bw;
    d.fill(
        shade(primary, if hover_plus { 0.4 } else { 0.12 }),
        bevel_poly(px, y, bw, h, 5.0),
    );
    d.stroke(primary, bevel_poly(px, y, bw, h, 5.0));
    d.stroke(
        primary,
        vec![[px + bw * 0.3, y + h * 0.5], [px + bw * 0.7, y + h * 0.5]],
    );
    d.stroke(
        primary,
        vec![[px + bw * 0.5, y + h * 0.3], [px + bw * 0.5, y + h * 0.7]],
    );
    d.rect_outline(track, x + bw + 2.0, y, w - bw * 2.0 - 4.0, h);
    d
}

// ════════════════════════════════════════════════════════════════════════════
// Game UI
// ════════════════════════════════════════════════════════════════════════════

/// Health bar: notched fill that shifts colour low→high and can pulse (host
/// passes a 0..1 `pulse`).
#[allow(clippy::too_many_arguments)]
pub fn healthbar(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    frac: f32,
    pulse: f32,
    full: Rgba,
    low: Rgba,
    track: Rgba,
) -> Draw {
    let mut d = Draw::new();
    d.rect_outline(track, x, y, w, h);
    let f = clamp01(frac);
    let mut c = mix(low, full, f);
    if f < 0.3 {
        c = mix(c, 0xFFFFFF, pulse * 0.6);
    }
    if f > 0.0 {
        d.rect_fill(c, x + 1.0, y + 1.0, (w - 2.0) * f, h - 2.0);
    }
    for i in 1..10 {
        let nx = x + w * i as f32 / 10.0;
        d.stroke(track, vec![[nx, y], [nx, y + h]]);
    }
    d
}

/// Radial cooldown wipe (clockwise from top). `frac` = remaining 1→0.
pub fn cooldown(cx: f32, cy: f32, r: f32, frac: f32, fill: Rgba, track: Rgba) -> Draw {
    let mut d = Draw::new();
    d.stroke(track, arc(cx, cy, r, 0.0, PI * 2.0, 48));
    let a0 = -PI * 0.5;
    let mut wedge = vec![[cx, cy]];
    wedge.extend(arc(cx, cy, r, a0, a0 + PI * 2.0 * clamp01(frac), 48));
    wedge.push([cx, cy]);
    d.fill(shade(fill, 0.55), wedge);
    d
}

/// 7-segment style vector number for `value` with `digits` places.
#[allow(clippy::too_many_arguments)]
pub fn counter(
    x: f32,
    y: f32,
    dw: f32,
    dh: f32,
    value: i64,
    digits: usize,
    on: Rgba,
    off: Rgba,
) -> Draw {
    let mut d = Draw::new();
    let segmap: [u8; 10] = [
        0b1111110, 0b0110000, 0b1101101, 0b1111001, 0b0110011, 0b1011011, 0b1011111, 0b1110000,
        0b1111111, 0b1111011,
    ];
    let gap = dw * 0.35;
    let v = value.unsigned_abs();
    for i in 0..digits {
        let digit = ((v / 10u64.pow((digits - 1 - i) as u32)) % 10) as usize;
        let mask = segmap[digit];
        let dx = x + i as f32 * (dw + gap);
        seven_seg(&mut d, dx, y, dw, dh, mask, on, off);
    }
    d
}

#[allow(clippy::too_many_arguments)]
fn seven_seg(d: &mut Draw, x: f32, y: f32, w: f32, h: f32, mask: u8, on: Rgba, off: Rgba) {
    let t = w * 0.12; // segment thickness
    let mid = y + h * 0.5;
    // segments: a(top) b(tr) c(br) d(bottom) e(bl) f(tl) g(mid) — bit6..bit0
    let bars = [
        (6, [x + t, y, w - 2.0 * t, t]),                       // a
        (5, [x + w - t, y + t, t, h * 0.5 - 1.5 * t]),         // b
        (4, [x + w - t, mid + 0.5 * t, t, h * 0.5 - 1.5 * t]), // c
        (3, [x + t, y + h - t, w - 2.0 * t, t]),               // d
        (2, [x, mid + 0.5 * t, t, h * 0.5 - 1.5 * t]),         // e
        (1, [x, y + t, t, h * 0.5 - 1.5 * t]),                 // f
        (0, [x + t, mid - 0.5 * t, w - 2.0 * t, t]),           // g
    ];
    for (bit, r) in bars {
        let c = if mask & (1 << bit) != 0 { on } else { off };
        d.rect_fill(c, r[0], r[1], r[2], r[3]);
    }
}

/// Minimap frame with corner brackets (host plots blips separately).
pub fn minimap(x: f32, y: f32, w: f32, h: f32, primary: Rgba, bg: Rgba) -> Draw {
    let mut d = Draw::new();
    d.fill(
        bg,
        vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h], [x, y]],
    );
    for seg in holo::corner_brackets(x, y, w, h, 14.0) {
        d.stroke(primary, vec![[seg[0], seg[1]], [seg[2], seg[3]]]);
    }
    d
}

/// Virtual D-pad / joystick; `dir` 0=none,1=up,2=right,3=down,4=left highlights.
pub fn dpad(cx: f32, cy: f32, r: f32, dir: i32, primary: Rgba, track: Rgba) -> Draw {
    let mut d = Draw::new();
    let arm = r * 0.45;
    let dirs = [(0.0, -1.0), (1.0, 0.0), (0.0, 1.0), (-1.0, 0.0)];
    d.stroke(track, arc(cx, cy, r, 0.0, PI * 2.0, 32));
    for (i, (dx, dy)) in dirs.iter().enumerate() {
        let on = dir == i as i32 + 1;
        let c = if on { primary } else { track };
        let bx = cx + dx * r * 0.55;
        let by = cy + dy * r * 0.55;
        let tri = vec![
            [
                bx + dx * arm + dy * arm * 0.4,
                by + dy * arm + dx * arm * 0.4,
            ],
            [
                bx + dx * arm - dy * arm * 0.4,
                by + dy * arm - dx * arm * 0.4,
            ],
            [bx - dx * arm * 0.3, by - dy * arm * 0.3],
            [
                bx + dx * arm + dy * arm * 0.4,
                by + dy * arm + dx * arm * 0.4,
            ],
        ];
        if on {
            d.fill(c, tri.clone());
        }
        d.stroke(c, tri);
    }
    d
}

/// Item slot grid (`cols`×`rows`), `sel` index highlighted.
#[allow(clippy::too_many_arguments)]
pub fn slotgrid(
    x: f32,
    y: f32,
    cols: usize,
    rows: usize,
    cell: f32,
    sel: i32,
    primary: Rgba,
    track: Rgba,
) -> Draw {
    let mut d = Draw::new();
    let gap = 4.0;
    for r in 0..rows {
        for c in 0..cols {
            let idx = (r * cols + c) as i32;
            let cx = x + c as f32 * (cell + gap);
            let cy = y + r as f32 * (cell + gap);
            if idx == sel {
                d.fill(
                    shade(primary, 0.3),
                    vec![
                        [cx, cy],
                        [cx + cell, cy],
                        [cx + cell, cy + cell],
                        [cx, cy + cell],
                        [cx, cy],
                    ],
                );
                d.rect_outline(primary, cx, cy, cell, cell);
            } else {
                d.rect_outline(track, cx, cy, cell, cell);
            }
        }
    }
    d
}

/// Full-screen damage / alert vignette — `intensity` ∈ [0,1] thickens the border glow.
pub fn vignette(w: f32, h: f32, intensity: f32, col: Rgba) -> Draw {
    let mut d = Draw::new();
    let t = clamp01(intensity);
    let layers = (1.0 + t * 10.0) as usize;
    for i in 0..layers {
        let inset = i as f32 * 3.0;
        let c = shade(col, t * (1.0 - i as f32 / layers as f32));
        d.rect_outline(
            c,
            inset,
            inset,
            (w - inset * 2.0).max(0.0),
            (h - inset * 2.0).max(0.0),
        );
    }
    d
}

// ════════════════════════════════════════════════════════════════════════════
// Faux-3D in 2D space  (screen-space rotate + perspective divide, no camera)
// ════════════════════════════════════════════════════════════════════════════

#[inline]
fn proj3(p: [f32; 3], spin: f32, tilt: f32, cx: f32, cy: f32, scale: f32) -> [f32; 2] {
    // rotate around Y (spin) then X (tilt), simple perspective divide
    let (cs, sn) = (spin.cos(), spin.sin());
    let (ct, st) = (tilt.cos(), tilt.sin());
    let x1 = p[0] * cs + p[2] * sn;
    let z1 = -p[0] * sn + p[2] * cs;
    let y2 = p[1] * ct - z1 * st;
    let z2 = p[1] * st + z1 * ct;
    let f = 3.0 / (3.0 + z2);
    [cx + x1 * scale * f, cy + y2 * scale * f]
}

/// Spinning wireframe ring gauge — a torus-ish band that rotates in 2D space,
/// with `frac` lighting a leading arc.
pub fn gauge3d(cx: f32, cy: f32, r: f32, frac: f32, spin: f32, fill: Rgba, track: Rgba) -> Draw {
    let mut d = Draw::new();
    let n = 48;
    let lit = (clamp01(frac) * n as f32) as usize;
    let mut prev = proj3([1.0, 0.0, 0.0], spin, 0.9, cx, cy, r);
    for i in 1..=n {
        let a = i as f32 / n as f32 * PI * 2.0;
        let p = proj3([a.cos(), 0.0, a.sin()], spin, 0.9, cx, cy, r);
        let c = if i <= lit { fill } else { track };
        d.stroke(c, vec![prev, p]);
        prev = p;
    }
    // spokes for depth cue
    for k in 0..8 {
        let a = k as f32 / 8.0 * PI * 2.0;
        let o = proj3([a.cos(), 0.0, a.sin()], spin, 0.9, cx, cy, r);
        let inn = proj3([a.cos() * 0.7, 0.0, a.sin() * 0.7], spin, 0.9, cx, cy, r);
        d.stroke(shade(track, 0.7), vec![inn, o]);
    }
    d
}

/// Extruded / isometric panel — a beveled face lifted off the surface by `depth`.
pub fn panel3d(x: f32, y: f32, w: f32, h: f32, depth: f32, primary: Rgba, bg: Rgba) -> Draw {
    let mut d = Draw::new();
    let dx = depth * 0.7;
    let dy = -depth * 0.7;
    let front = [[x, y], [x + w, y], [x + w, y + h], [x, y + h]];
    let back: Vec<[f32; 2]> = front.iter().map(|p| [p[0] + dx, p[1] + dy]).collect();
    // side faces
    d.fill(
        shade(primary, 0.18),
        vec![front[1], back[1], back[2], front[2], front[1]],
    );
    d.fill(
        shade(primary, 0.28),
        vec![front[0], back[0], back[1], front[1], front[0]],
    );
    for i in 0..4 {
        d.stroke(shade(primary, 0.6), vec![front[i], back[i]]);
    }
    d.stroke(
        primary,
        back.iter()
            .cloned()
            .chain(std::iter::once(back[0]))
            .collect(),
    );
    d.fill(
        bg,
        front
            .iter()
            .cloned()
            .chain(std::iter::once(front[0]))
            .collect(),
    );
    d.stroke(
        primary,
        front
            .iter()
            .cloned()
            .chain(std::iter::once(front[0]))
            .collect(),
    );
    d
}

/// Perspective radar dish — concentric rings tilted into the screen, with a sweep.
pub fn radar3d(
    cx: f32,
    cy: f32,
    r: f32,
    tilt: f32,
    sweep: f32,
    primary: Rgba,
    track: Rgba,
) -> Draw {
    let mut d = Draw::new();
    for k in 1..=3 {
        let rr = r * k as f32 / 3.0;
        let mut pl = Vec::new();
        for i in 0..=40 {
            let a = i as f32 / 40.0 * PI * 2.0;
            pl.push(proj3(
                [a.cos() * rr / r, 0.0, a.sin() * rr / r],
                0.0,
                tilt,
                cx,
                cy,
                r,
            ));
        }
        d.stroke(track, pl);
    }
    let o = proj3([sweep.cos(), 0.0, sweep.sin()], 0.0, tilt, cx, cy, r);
    let c = proj3([0.0, 0.0, 0.0], 0.0, tilt, cx, cy, r);
    d.stroke(primary, vec![c, o]);
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_fill_is_proportional() {
        let empty = bar(0.0, 0.0, 100.0, 10.0, 0.0, 0xFFFFFF, 0x222222);
        let half = bar(0.0, 0.0, 100.0, 10.0, 0.5, 0xFFFFFF, 0x222222);
        assert!(empty.fills.is_empty());
        assert_eq!(half.fills.len(), 1);
        // ~half width (minus the 2px inset)
        let poly = &half.fills[0].1;
        let wpx = poly[1][0] - poly[0][0];
        assert!((wpx - 48.0).abs() < 2.0, "got {wpx}");
    }

    #[test]
    fn segbar_lights_cells() {
        let d = segbar(0.0, 0.0, 100.0, 10.0, 0.5, 10, 0xFFFFFF, 0x222222);
        assert_eq!(d.fills.len(), 5); // 5 of 10 lit
    }

    #[test]
    fn counter_renders_digits() {
        let d = counter(0.0, 0.0, 12.0, 20.0, 42, 3, 0xFFFFFF, 0x111111);
        // 3 digits × 7 segments each
        assert_eq!(d.fills.len(), 21);
    }

    #[test]
    fn mix_and_shade() {
        assert_eq!(mix(0x000000, 0xFFFFFF, 0.5) & 0xFF, 0x7F);
        assert_eq!(shade(0x808080, 2.0), 0xFFFFFF);
    }
}
