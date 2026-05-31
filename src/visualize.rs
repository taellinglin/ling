// src/visualize.rs — AST-driven SVG visualiser for .ling source files.
//
// Usage: ling visualize examples/foo.ling > foo.svg
//
// Each function becomes a card showing colour-coded geometric icons for every
// vtex_*/audio_*/control-flow call it makes.  Global constants appear in a
// sidebar.  The entry-point card is highlighted in gold.

use crate::parser::ast::*;
use std::collections::HashSet;
use std::fmt::Write;

// ── Layout constants ─────────────────────────────────────────────────────────

const SVG_W:      f32 = 1640.0;
const SIDEBAR_W:  f32 = 200.0;
const CARD_W:     f32 = 370.0;
const CARD_GAP:   f32 = 14.0;
const GRID_X:     f32 = SIDEBAR_W + 14.0;
const COLS:       usize = 3;
const HEADER_H:   f32 = 74.0;
const LEGEND_H:   f32 = 40.0;
const CONTENT_Y:  f32 = HEADER_H + LEGEND_H;
const ICON_SZ:    f32 = 22.0;   // icon bounding box (icon radius = ICON_SZ/2)
const ICON_GAP:   f32 = 4.0;
const ICONS_ROW:  usize = 11;
const CARD_PAD:   f32 = 12.0;
const CARD_ROUNDING: f32 = 7.0;

// ── Colour palette ───────────────────────────────────────────────────────────

const BG:        &str = "#0b0b1a";
const CARD_BG:   &str = "#11112a";
const CARD_BD:   &str = "#22225a";
const TEXT:      &str = "#c0c0e0";
const TEXT_DIM:  &str = "#52528a";
const GOLD:      &str = "#ffd700";
const SIDEBAR_BG:&str = "#0d0d22";

// ── Call category ────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[allow(dead_code)]
enum Cat {
    Rings, Spiral, Star, Flower, Lotus, Chakra, Yantra,
    Hyper, Tess, Rain, Grid, Halftone,
    Tone, Vol, Listen,
    Camera, Light, Fill, Present,
    User, Loop, Const,
}

impl Cat {
    fn color(self) -> &'static str {
        match self {
            Cat::Rings    => "#00e5ff",
            Cat::Spiral   => "#00ffb3",
            Cat::Star     => "#ffd700",
            Cat::Flower   => "#ff79c6",
            Cat::Lotus    => "#ff5e8a",
            Cat::Chakra   => "#bd93f9",
            Cat::Yantra   => "#ffb86c",
            Cat::Hyper    => "#5575c8",
            Cat::Tess     => "#50fa7b",
            Cat::Rain     => "#f1fa8c",
            Cat::Grid     => "#8be9fd",
            Cat::Halftone => "#888899",
            Cat::Tone     => "#ff1493",
            Cat::Vol      => "#ff69b4",
            Cat::Listen   => "#db77d9",
            Cat::Camera   => "#ffe066",
            Cat::Light    => "#ffe4b5",
            Cat::Fill     => "#3a3a6a",
            Cat::Present  => "#555580",
            Cat::User     => "#6ab0f5",
            Cat::Loop     => "#ff7f50",
            Cat::Const    => "#50fa7b",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Cat::Rings    => "rings",
            Cat::Spiral   => "spiral",
            Cat::Star     => "star",
            Cat::Flower   => "flower",
            Cat::Lotus    => "lotus",
            Cat::Chakra   => "chakra",
            Cat::Yantra   => "yantra",
            Cat::Hyper    => "hyperbolic",
            Cat::Tess     => "tessellated",
            Cat::Rain     => "letter rain",
            Cat::Grid     => "grid",
            Cat::Halftone => "halftone",
            Cat::Tone     => "audio tone",
            Cat::Vol      => "audio vol",
            Cat::Listen   => "listener",
            Cat::Camera   => "camera",
            Cat::Light    => "light",
            Cat::Fill     => "fill",
            Cat::Present  => "render",
            Cat::User     => "fn call",
            Cat::Loop     => "loop",
            Cat::Const    => "const",
        }
    }
}

fn categorize(name: &str) -> Cat {
    if name.starts_with("vtex_rings")       || name == "ลายวงซ้อน"         { return Cat::Rings; }
    if name.starts_with("vtex_spiral")      || name == "ลายก้นหอย"         { return Cat::Spiral; }
    if name.starts_with("vtex_star")        || name == "ลายดาว"             { return Cat::Star; }
    if name.starts_with("vtex_flower")      || name == "ลายดอกไม้"          { return Cat::Flower; }
    if name.starts_with("vtex_lotus")       || name == "ลายบัว"             { return Cat::Lotus; }
    if name.starts_with("vtex_chakra")      || name == "ลายจักร"            { return Cat::Chakra; }
    if name.starts_with("vtex_yantra")      || name == "ลายยันต์"           { return Cat::Yantra; }
    if name.starts_with("vtex_hyperbolic")  || name == "ลายไฮเพอร์โบลิก"   { return Cat::Hyper; }
    if name.starts_with("vtex_tessellated") || name == "ลายตาข่าย"          { return Cat::Tess; }
    if name.starts_with("vtex_letter_rain") || name == "ลายอักษรไหล"        { return Cat::Rain; }
    if name.starts_with("vtex_grid")        || name == "ลายตาราง"           { return Cat::Grid; }
    if name.starts_with("vtex_halftone")    || name == "ลายจุด"             { return Cat::Halftone; }
    match name {
        "audio_tone"                            => Cat::Tone,
        "audio_volume"                          => Cat::Vol,
        "audio_listener"                        => Cat::Listen,
        "set_camera"                            => Cat::Camera,
        "add_light" | "clear_lights"            => Cat::Light,
        "เติม"  | "fill"                        => Cat::Fill,
        "แสดงผล" | "present"                    => Cat::Present,
        _                                       => Cat::User,
    }
}

fn is_vtex(c: Cat) -> bool {
    matches!(c, Cat::Rings|Cat::Spiral|Cat::Star|Cat::Flower|Cat::Lotus|
               Cat::Chakra|Cat::Yantra|Cat::Hyper|Cat::Tess|Cat::Rain|
               Cat::Grid|Cat::Halftone)
}
fn is_audio(c: Cat) -> bool { matches!(c, Cat::Tone|Cat::Vol|Cat::Listen) }

const ENTRY_NAMES: &[&str] = &[
    "start","main","启","เริ่ม","시작","начать","начало",
    "inicio","comenzar","début","commencer","anfang","starten","início",
];
fn is_entry(name: &str) -> bool { ENTRY_NAMES.contains(&name) }

// ── Data model ────────────────────────────────────────────────────────────────

struct GlobalConst { name: String, value: String }

#[derive(Clone)]
struct CallItem { name: String, cat: Cat, count: usize }

struct FuncCard {
    name:      String,
    params:    Vec<String>,
    calls:     Vec<CallItem>,
    has_loop:  bool,
    is_entry:  bool,
    vtex_count:  usize,
    audio_count: usize,
}

struct Document {
    filename: String,
    globals:  Vec<GlobalConst>,
    funcs:    Vec<FuncCard>,
    #[allow(dead_code)]
    fn_names: HashSet<String>,
}

// ── AST walking ───────────────────────────────────────────────────────────────

struct RawCall { name: String, cat: Cat }

fn walk_stmts(stmts: &[Stmt], fns: &HashSet<String>, out: &mut Vec<RawCall>, loop_: &mut bool) {
    for s in stmts {
        let e = match s { Stmt::Expr(e)|Stmt::Return(e) => e, Stmt::Bind(_,e) => e };
        walk_expr(e, fns, out, loop_);
    }
}

fn walk_expr(e: &Expr, fns: &HashSet<String>, out: &mut Vec<RawCall>, loop_: &mut bool) {
    match e {
        Expr::Call(func, args) => {
            if let Expr::Ident(name) = func.as_ref() {
                let cat = if fns.contains(name.as_str()) {
                    let base = categorize(name);
                    if base == Cat::User { Cat::User } else { base }
                } else {
                    categorize(name)
                };
                out.push(RawCall { name: name.clone(), cat });
            }
            for a in args { walk_expr(a, fns, out, loop_); }
        }
        Expr::While { cond, body } => {
            *loop_ = true;
            walk_expr(cond, fns, out, loop_);
            walk_stmts(body, fns, out, loop_);
        }
        Expr::Do(ss) => walk_stmts(ss, fns, out, loop_),
        Expr::If { cond, then, elseifs, else_body } => {
            walk_expr(cond, fns, out, loop_);
            walk_stmts(then, fns, out, loop_);
            for (c,b) in elseifs { walk_expr(c,fns,out,loop_); walk_stmts(b,fns,out,loop_); }
            if let Some(b) = else_body { walk_stmts(b, fns, out, loop_); }
        }
        Expr::For { iter, body, .. } => {
            walk_expr(iter, fns, out, loop_);
            walk_stmts(body, fns, out, loop_);
        }
        Expr::BinOp(_, a, b) => { walk_expr(a, fns, out, loop_); walk_expr(b, fns, out, loop_); }
        Expr::Array(es) => { for a in es { walk_expr(a, fns, out, loop_); } }
        _ => {}
    }
}

fn aggregate(raw: Vec<RawCall>) -> Vec<CallItem> {
    let mut out: Vec<CallItem> = Vec::new();
    for r in raw {
        if let Some(last) = out.last_mut() {
            if last.name == r.name { last.count += 1; continue; }
        }
        out.push(CallItem { name: r.name, cat: r.cat, count: 1 });
    }
    out
}

fn make_card(name: String, params: Vec<String>, raw: Vec<RawCall>,
             has_loop: bool, is_entry: bool) -> FuncCard {
    let calls = aggregate(raw);
    let vtex_count  = calls.iter().filter(|c| is_vtex(c.cat)).map(|c| c.count).sum();
    let audio_count = calls.iter().filter(|c| is_audio(c.cat)).map(|c| c.count).sum();
    FuncCard { name, params, calls, has_loop, is_entry, vtex_count, audio_count }
}

impl Document {
    fn build(filename: &str, prog: &Program) -> Self {
        let fn_names: HashSet<String> = prog.items.iter().filter_map(|i| {
            if let Item::Fn(f) = i { Some(f.name.clone()) } else { None }
        }).collect();

        let mut globals = Vec::new();
        let mut entries = Vec::new();
        let mut funcs   = Vec::new();

        for item in &prog.items {
            match item {
                Item::Bind(name, expr) => match expr {
                    Expr::Number(n) => globals.push(GlobalConst {
                        name: name.clone(),
                        value: if n.fract() == 0.0 { format!("{}", *n as i64) }
                               else { format!("{:.2}", n) },
                    }),
                    Expr::Do(body) => {
                        let mut raw = Vec::new();
                        let mut lp = false;
                        // Scan inside the do block for a while loop too
                        for s in body {
                            if let Stmt::Expr(Expr::While { body: wb, .. }) = s {
                                lp = true;
                                walk_stmts(wb, &fn_names, &mut raw, &mut lp);
                            }
                        }
                        walk_stmts(body, &fn_names, &mut raw, &mut lp);
                        entries.push(make_card(name.clone(), vec![], raw, lp, is_entry(name)));
                    }
                    _ => {}
                },
                Item::Fn(f) => {
                    let mut raw = Vec::new();
                    let mut lp  = false;
                    walk_stmts(&f.body, &fn_names, &mut raw, &mut lp);
                    funcs.push(make_card(f.name.clone(), f.params.clone(), raw, lp, false));
                }
                _ => {}
            }
        }

        entries.extend(funcs);
        Document { filename: filename.to_string(), globals, funcs: entries, fn_names }
    }
}

// ── SVG icons ─────────────────────────────────────────────────────────────────
// Each icon is drawn centred at (cx, cy) within a radius of r≈10.

fn xe(s: &str) -> String { s.replace('&',"&amp;").replace('<',"&lt;").replace('>',"&gt;") }
fn p(v: f32) -> String { format!("{:.2}", v) }

fn icon(cat: Cat, cx: f32, cy: f32, r: f32) -> String {
    let c = cat.color();
    match cat {
        Cat::Rings => {
            format!(
                r#"<circle cx="{}" cy="{}" r="{}" fill="none" stroke="{c}" stroke-width="1.2" opacity="0.9"/>
                   <circle cx="{}" cy="{}" r="{}" fill="none" stroke="{c}" stroke-width="1.2" opacity="0.65"/>
                   <circle cx="{}" cy="{}" r="{}" fill="none" stroke="{c}" stroke-width="1.2" opacity="0.4"/>"#,
                p(cx),p(cy),p(r), p(cx),p(cy),p(r*0.63), p(cx),p(cy),p(r*0.33)
            )
        }
        Cat::Spiral => {
            // Approximate spiral with two arcs
            let (x0,y0) = (cx, cy - r*0.1);
            let (x1,y1) = (cx + r*0.85, cy);
            let (x2,y2) = (cx, cy + r*0.9);
            let (x3,y3) = (cx - r*0.85, cy + r*0.1);
            let (x4,y4) = (cx, cy - r*0.9);
            format!(
                r#"<path d="M {},{} C {},{} {},{} {},{} S {},{} {},{} S {},{} {},{} S {},{} {},{}"
                        fill="none" stroke="{c}" stroke-width="1.4" opacity="0.9" stroke-linecap="round"/>"#,
                p(x0),p(y0),
                p(cx+r),p(y0), p(x1),p(cy-r*0.5), p(x1),p(y1),
                p(cx+r*0.3),p(y2), p(x2),p(y2),
                p(x3),p(y3-r*0.4), p(x3),p(y3),
                p(cx),p(y4), p(x4),p(y4)
            )
        }
        Cat::Star => {
            // 5-point star
            let pts: String = (0..5).flat_map(|i| {
                let ao = (i as f32 * 72.0 - 90.0).to_radians();
                let ai = ao + 36.0_f32.to_radians();
                let ri = r * 0.42;
                vec![
                    format!("{},{}", p(cx + r*ao.cos()), p(cy + r*ao.sin())),
                    format!("{},{}", p(cx + ri*ai.cos()), p(cy + ri*ai.sin())),
                ]
            }).collect::<Vec<_>>().join(" ");
            format!(r#"<polygon points="{pts}" fill="{c}" opacity="0.85"/>"#)
        }
        Cat::Flower => {
            // 6 petals as ellipses
            (0..6).map(|i| {
                let angle = i as f32 * 60.0;
                format!(
                    r#"<ellipse cx="{}" cy="{}" rx="{}" ry="{}"
                           fill="{c}" opacity="0.5"
                           transform="rotate({angle},{},{})"/>"#,
                    p(cx), p(cy - r*0.45), p(r*0.28), p(r*0.52),
                    p(cx), p(cy)
                )
            }).collect::<String>()
        }
        Cat::Lotus => {
            // 8 petals, more elongated
            (0..8).map(|i| {
                let angle = i as f32 * 45.0;
                format!(
                    r#"<ellipse cx="{}" cy="{}" rx="{}" ry="{}"
                           fill="{c}" opacity="0.45"
                           transform="rotate({angle},{},{})"/>"#,
                    p(cx), p(cy - r*0.50), p(r*0.22), p(r*0.55),
                    p(cx), p(cy)
                )
            }).collect::<String>()
                + &format!(r#"<circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.8"/>"#,
                    p(cx), p(cy), p(r*0.22))
        }
        Cat::Chakra => {
            // Central circle + 8 spokes
            let spokes: String = (0..8).map(|i| {
                let a = (i as f32 * 45.0).to_radians();
                format!(r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.2" opacity="0.8"/>"#,
                    p(cx + r*0.28*a.cos()), p(cy + r*0.28*a.sin()),
                    p(cx + r*0.88*a.cos()), p(cy + r*0.88*a.sin()))
            }).collect();
            spokes + &format!(
                r#"<circle cx="{}" cy="{}" r="{}" fill="none" stroke="{c}" stroke-width="1.2" opacity="0.7"/>
                   <circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.8"/>"#,
                p(cx), p(cy), p(r*0.88),
                p(cx), p(cy), p(r*0.2)
            )
        }
        Cat::Yantra => {
            // Hexagram (Star of David) from two triangles
            let h = r * 0.866;
            let t1 = format!("{},{} {},{} {},{}",
                p(cx), p(cy - r),
                p(cx + h), p(cy + r*0.5),
                p(cx - h), p(cy + r*0.5));
            let t2 = format!("{},{} {},{} {},{}",
                p(cx), p(cy + r),
                p(cx - h), p(cy - r*0.5),
                p(cx + h), p(cy - r*0.5));
            format!(
                r#"<polygon points="{t1}" fill="none" stroke="{c}" stroke-width="1.3" opacity="0.85"/>
                   <polygon points="{t2}" fill="none" stroke="{c}" stroke-width="1.3" opacity="0.85"/>
                   <circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.25"/>"#,
                p(cx), p(cy), p(r*0.38)
            )
        }
        Cat::Hyper => {
            // Radial + concentric = Poincaré disc
            let rays: String = (0..6).map(|i| {
                let a = (i as f32 * 30.0).to_radians();
                format!(r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.0" opacity="0.55"/>"#,
                    p(cx - r*a.cos()), p(cy - r*a.sin()),
                    p(cx + r*a.cos()), p(cy + r*a.sin()))
            }).collect();
            rays + &format!(
                r#"<circle cx="{}" cy="{}" r="{}" fill="none" stroke="{c}" stroke-width="1.0" opacity="0.8"/>
                   <circle cx="{}" cy="{}" r="{}" fill="none" stroke="{c}" stroke-width="1.0" opacity="0.5"/>
                   <circle cx="{}" cy="{}" r="{}" fill="none" stroke="{c}" stroke-width="1.0" opacity="0.3"/>"#,
                p(cx),p(cy),p(r),   p(cx),p(cy),p(r*0.6),   p(cx),p(cy),p(r*0.3)
            )
        }
        Cat::Tess => {
            // Wavy horizontal lines
            (0..4).map(|i| {
                let y = cy - r*0.7 + i as f32 * r*0.46;
                let amp = r * 0.15;
                format!(
                    r#"<path d="M {},{} C {},{} {},{} {},{} S {},{} {},{}"
                            fill="none" stroke="{c}" stroke-width="1.1" opacity="0.7"/>"#,
                    p(cx-r), p(y),
                    p(cx-r+r*0.4), p(y-amp),  p(cx-r*0.1), p(y-amp),  p(cx), p(y),
                    p(cx+r*0.5), p(y+amp),  p(cx+r), p(y)
                )
            }).collect::<String>()
        }
        Cat::Rain => {
            // Falling dashes (letter rain columns)
            (0..5).flat_map(|col| {
                let x = cx - r*0.9 + col as f32 * r*0.45;
                (0..3).map(move |row| {
                    let y = cy - r*0.8 + row as f32 * r*0.55;
                    let opa = 0.9 - row as f32 * 0.22;
                    format!(r#"<rect x="{}" y="{}" width="{}" height="{}" rx="1" fill="{c}" opacity="{:.2}"/>"#,
                        p(x - r*0.06), p(y), p(r*0.12), p(r*0.30), opa)
                })
            }).collect::<String>()
        }
        Cat::Grid => {
            // # hash grid
            let n = 3;
            let step = r * 2.0 / n as f32;
            let mut s = String::new();
            for i in 0..=n {
                let off = -r + i as f32 * step;
                write!(s, r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.0" opacity="0.65"/>
                             <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.0" opacity="0.65"/>"#,
                    p(cx+off),p(cy-r), p(cx+off),p(cy+r),
                    p(cx-r),p(cy+off), p(cx+r),p(cy+off)).ok();
            }
            s
        }
        Cat::Halftone => {
            // 4×4 dot grid
            (0..4).flat_map(|row| (0..4).map(move |col| {
                let x = cx - r*0.75 + col as f32 * r*0.5;
                let y = cy - r*0.75 + row as f32 * r*0.5;
                format!(r#"<circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.6"/>"#,
                    p(x), p(y), p(r*0.14))
            })).collect::<String>()
        }
        Cat::Tone => {
            // Sine wave
            let x0 = cx - r; let x3 = cx + r; let xm = cx;
            let amp = r * 0.65;
            format!(
                r#"<path d="M {},{} C {},{} {},{} {},{} S {},{} {},{}"
                        fill="none" stroke="{c}" stroke-width="1.6" opacity="0.9"/>"#,
                p(x0), p(cy),
                p(x0 + r*0.5), p(cy - amp), p(xm - r*0.1), p(cy - amp), p(xm), p(cy),
                p(xm + r*0.6), p(cy + amp), p(x3), p(cy)
            )
        }
        Cat::Vol | Cat::Listen => {
            // Concentric arcs (speaker/ear)
            let arcs: String = (1..=3).map(|i| {
                let ri = r * 0.3 * i as f32;
                format!(
                    r#"<path d="M {},{} A {ri},{ri} 0 0,1 {},{}"
                            fill="none" stroke="{c}" stroke-width="1.3" opacity="{:.2}"/>"#,
                    p(cx), p(cy - ri),
                    p(cx), p(cy + ri),
                    1.0 - i as f32 * 0.22
                )
            }).collect();
            arcs + &format!(r#"<circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.7"/>"#,
                p(cx - r*0.15), p(cy), p(r*0.22))
        }
        Cat::Camera => {
            // Lens: outer circle + inner circle + crosshair dot
            format!(
                r#"<circle cx="{}" cy="{}" r="{}" fill="none" stroke="{c}" stroke-width="1.5" opacity="0.8"/>
                   <circle cx="{}" cy="{}" r="{}" fill="none" stroke="{c}" stroke-width="1.2" opacity="0.55"/>
                   <circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.9"/>"#,
                p(cx), p(cy), p(r),
                p(cx), p(cy), p(r*0.6),
                p(cx), p(cy), p(r*0.18)
            )
        }
        Cat::Light => {
            // Sunburst: central circle + 8 rays
            let rays: String = (0..8).map(|i| {
                let a = (i as f32 * 45.0).to_radians();
                let r1 = r * 0.38;
                let r2 = r * 0.90;
                format!(r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.2" opacity="0.75"/>"#,
                    p(cx + r1*a.cos()), p(cy + r1*a.sin()),
                    p(cx + r2*a.cos()), p(cy + r2*a.sin()))
            }).collect();
            rays + &format!(r#"<circle cx="{}" cy="{}" r="{}" fill="{c}" opacity="0.85"/>"#,
                p(cx), p(cy), p(r*0.32))
        }
        Cat::Fill => {
            format!(r#"<rect x="{}" y="{}" width="{}" height="{}" rx="2" fill="{c}" opacity="0.5"/>"#,
                p(cx - r*0.9), p(cy - r*0.7), p(r*1.8), p(r*1.4))
        }
        Cat::Present => {
            // Triangle (play/render button)
            let pts = format!("{},{} {},{} {},{}",
                p(cx - r*0.7), p(cy - r*0.8),
                p(cx + r*0.8), p(cy),
                p(cx - r*0.7), p(cy + r*0.8));
            format!(r#"<polygon points="{pts}" fill="{c}" opacity="0.7"/>"#)
        }
        Cat::User => {
            // Right-pointing chevron (function call arrow)
            format!(
                r#"<path d="M {},{} L {},{} L {},{}" fill="none" stroke="{c}" stroke-width="1.8"
                         stroke-linecap="round" stroke-linejoin="round" opacity="0.85"/>
                   <path d="M {},{} L {},{} L {},{}" fill="none" stroke="{c}" stroke-width="1.8"
                         stroke-linecap="round" stroke-linejoin="round" opacity="0.5"/>"#,
                p(cx-r*0.6), p(cy-r*0.7), p(cx+r*0.5), p(cy), p(cx-r*0.6), p(cy+r*0.7),
                p(cx), p(cy-r*0.7), p(cx+r*0.95), p(cy), p(cx), p(cy+r*0.7)
            )
        }
        Cat::Loop => {
            // Circular arrow
            format!(
                r#"<path d="M {},{} A {},{} 0 1,1 {},{}"
                        fill="none" stroke="{c}" stroke-width="1.6" opacity="0.9"/>
                   <polygon points="{},{} {},{} {},{}" fill="{c}" opacity="0.9"/>"#,
                p(cx), p(cy - r*0.9),
                p(r*0.9), p(r*0.9),
                p(cx + r*0.4), p(cy - r*0.9),
                p(cx+r*0.4), p(cy-r*0.9),
                p(cx+r*0.1), p(cy-r*0.55),
                p(cx+r*0.8), p(cy-r*0.62)
            )
        }
        Cat::Const => {
            format!(r#"<rect x="{}" y="{}" width="{}" height="{}" rx="2"
                           fill="none" stroke="{c}" stroke-width="1.2" opacity="0.7"/>
                       <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{c}" stroke-width="1.1" opacity="0.5"/>"#,
                p(cx-r*0.7), p(cy-r*0.55), p(r*1.4), p(r*1.1),
                p(cx-r*0.4), p(cy), p(cx+r*0.4), p(cy))
        }
    }
}

// ── Card rendering ────────────────────────────────────────────────────────────

fn card_height(card: &FuncCard) -> f32 {
    let icon_rows = (card.calls.len() + ICONS_ROW - 1).max(1) / ICONS_ROW + 1;
    let base = CARD_PAD * 2.0          // top+bottom padding
        + 32.0                          // name header
        + 20.0;                         // params + stats row
    base + icon_rows as f32 * (ICON_SZ + ICON_GAP) + ICON_GAP
}

fn render_card(card: &FuncCard, x: f32, y: f32) -> String {
    let h = card_height(card);
    let w = CARD_W;
    let r = CARD_ROUNDING;

    // Dominant colour from most-common call category (by raw count)
    let dominant = card.calls.iter()
        .max_by_key(|c| c.count)
        .map(|c| c.cat.color())
        .unwrap_or(Cat::User.color());

    let border_color = if card.is_entry { GOLD } else { dominant };
    let border_w     = if card.is_entry { 2.5  } else { 1.2 };
    let glow_filter  = if card.is_entry { r#" filter="url(#glow-gold)""# } else { "" };

    let mut s = String::new();

    // Card background
    write!(s, r#"<rect x="{}" y="{}" width="{w}" height="{h}" rx="{r}"
                      fill="{CARD_BG}" stroke="{border_color}" stroke-width="{border_w}"{glow_filter}/>
                 <line x1="{}" y1="{}" x2="{}" y2="{}"
                      stroke="{border_color}" stroke-width="3" opacity="0.6"/>
"#,
        p(x), p(y),
        p(x+r), p(y+h), p(x+r), p(y)   // left accent stripe
    ).ok();

    // Function name
    let name_y = y + CARD_PAD + 18.0;
    let entry_badge = if card.is_entry {
        format!(r#" <text x="{}" y="{}" fill="{GOLD}" font-size="9" font-weight="bold" opacity="0.8">⬡ ENTRY</text>"#,
            p(x + w - CARD_PAD - 48.0), p(name_y))
    } else { String::new() };

    write!(s, r#"<text x="{}" y="{}" fill="{}" font-size="13" font-weight="bold">{}</text>{}"#,
        p(x + CARD_PAD + 10.0), p(name_y),
        if card.is_entry { GOLD } else { TEXT },
        xe(&card.name),
        entry_badge
    ).ok();

    // Params + stats row
    let stats_y = name_y + 18.0;
    let params_str = if card.params.is_empty() { String::new() }
        else { format!("({})", card.params.join(", ")) };
    let stats_str = {
        let mut parts = Vec::new();
        if card.vtex_count > 0  { parts.push(format!("{} vtex",  card.vtex_count)); }
        if card.audio_count > 0 { parts.push(format!("{} audio", card.audio_count)); }
        if card.has_loop        { parts.push("↺ loop".into()); }
        parts.join("  ·  ")
    };
    write!(s, r#"<text x="{}" y="{}" fill="{TEXT_DIM}" font-size="10">{}</text>
                 <text x="{}" y="{}" fill="{TEXT_DIM}" font-size="10" text-anchor="end">{}</text>
"#,
        p(x + CARD_PAD + 10.0), p(stats_y), xe(&params_str),
        p(x + w - CARD_PAD), p(stats_y), xe(&stats_str)
    ).ok();

    // Divider
    let div_y = stats_y + 6.0;
    write!(s, r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{CARD_BD}" stroke-width="1"/>"#,
        p(x + CARD_PAD), p(div_y), p(x + w - CARD_PAD), p(div_y)).ok();

    // Icons
    let icon_y0 = div_y + ICON_GAP + ICON_SZ / 2.0;
    let icon_x0 = x + CARD_PAD + ICON_SZ / 2.0 + 6.0;
    let ir = ICON_SZ / 2.0 * 0.82; // icon radius slightly smaller than half-cell

    for (i, call) in card.calls.iter().enumerate() {
        let row = i / ICONS_ROW;
        let col = i % ICONS_ROW;
        let ix = icon_x0 + col as f32 * (ICON_SZ + ICON_GAP);
        let iy = icon_y0 + row as f32 * (ICON_SZ + ICON_GAP);

        // Icon cell background
        write!(s, r#"<rect x="{}" y="{}" width="{ICON_SZ}" height="{ICON_SZ}" rx="3"
                          fill="{}" opacity="0.12"/>
"#,
            p(ix - ICON_SZ/2.0), p(iy - ICON_SZ/2.0),
            call.cat.color()
        ).ok();

        // Icon shape
        s.push_str(&icon(call.cat, ix, iy, ir));

        // Count badge (if > 1)
        if call.count > 1 {
            write!(s, r##"<rect x="{}" y="{}" width="13" height="10" rx="3" fill="{}" opacity="0.9"/>
                         <text x="{}" y="{}" fill="#0a0a1a" font-size="8" font-weight="bold" text-anchor="middle">{}</text>"##,
                p(ix + ir - 2.0), p(iy - ir - 1.0), call.cat.color(),
                p(ix + ir + 4.5), p(iy - ir + 7.0), call.count
            ).ok();
        }
    }

    s
}

// ── Legend, header, sidebar ───────────────────────────────────────────────────

fn render_header(filename: &str, funcs: &[FuncCard]) -> String {
    let name = std::path::Path::new(filename)
        .file_name().map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| filename.to_string());
    let n_fns   = funcs.len();
    let n_vtex: usize  = funcs.iter().flat_map(|f| &f.calls).filter(|c| is_vtex(c.cat)).map(|c| c.count).sum();
    let n_audio: usize = funcs.iter().flat_map(|f| &f.calls).filter(|c| is_audio(c.cat)).map(|c| c.count).sum();
    let n_calls: usize = funcs.iter().map(|f| f.calls.len()).sum();

    format!(
        r##"<rect x="0" y="0" width="{SVG_W}" height="{HEADER_H}" fill="#080814"/>
           <line x1="0" y1="{HEADER_H}" x2="{SVG_W}" y2="{HEADER_H}" stroke="#1a1a3a" stroke-width="1"/>
           <text x="20" y="22" fill="{GOLD}" font-size="9" font-weight="bold" opacity="0.6" letter-spacing="2">LING VISUALIZER</text>
           <text x="20" y="56" fill="{TEXT}" font-size="26" font-weight="bold">{}</text>
           <text x="{}" y="56" fill="{TEXT_DIM}" font-size="12" text-anchor="end">{n_fns} fn  ·  {n_calls} call types  ·  {n_vtex} vtex  ·  {n_audio} audio</text>"##,
        xe(&name),
        SVG_W - 20.0
    )
}

fn render_legend(funcs: &[FuncCard]) -> String {
    // Collect present categories from all cards
    let mut present: Vec<Cat> = Vec::new();
    let mut seen = HashSet::new();
    for f in funcs {
        for c in &f.calls {
            if seen.insert(c.cat) { present.push(c.cat); }
        }
    }

    let ly = HEADER_H + LEGEND_H / 2.0 + 4.0;
    let mut s = format!(
        r##"<rect x="0" y="{HEADER_H}" width="{SVG_W}" height="{LEGEND_H}" fill="#0a0a18"/>
           <line x1="0" y1="{}" x2="{SVG_W}" y2="{}" stroke="#171730" stroke-width="1"/>"##,
        HEADER_H + LEGEND_H, HEADER_H + LEGEND_H
    );

    let mut lx = GRID_X + 8.0;
    for cat in present {
        let c   = cat.color();
        let lbl = cat.label();
        let icon_svg = icon(cat, lx + 7.0, ly - 2.0, 6.0);
        s.push_str(&format!(
            r#"<rect x="{}" y="{}" width="14" height="14" rx="3" fill="{c}" opacity="0.12"/>
               {}
               <text x="{}" y="{}" fill="{TEXT_DIM}" font-size="10">{lbl}</text>"#,
            p(lx), p(ly - 9.0),
            icon_svg,
            p(lx + 18.0), p(ly + 3.0)
        ));
        lx += 20.0 + lbl.len() as f32 * 6.2 + 10.0;
        if lx > SVG_W - 120.0 { break; }
    }
    s
}

fn render_sidebar(globals: &[GlobalConst], total_h: f32) -> String {
    let h = total_h - CONTENT_Y;
    let mut s = format!(
        r##"<rect x="0" y="{CONTENT_Y}" width="{SIDEBAR_W}" height="{h}" fill="{SIDEBAR_BG}"/>
           <line x1="{SIDEBAR_W}" y1="{CONTENT_Y}" x2="{SIDEBAR_W}" y2="{total_h}" stroke="#1a1a3a" stroke-width="1"/>
           <text x="14" y="{}" fill="{TEXT_DIM}" font-size="9" font-weight="bold" letter-spacing="2">CONSTANTS</text>"##,
        CONTENT_Y + 20.0
    );

    let mut gy = CONTENT_Y + 38.0;
    for g in globals {
        // Small colored diamond
        write!(s,
            r#"<rect x="{}" y="{}" width="8" height="8" rx="1" fill="{}" opacity="0.7"
                    transform="rotate(45,{},{})"/>
               <text x="{}" y="{}" fill="{}" font-size="12" font-weight="bold">{}</text>
               <text x="{}" y="{}" fill="{TEXT_DIM}" font-size="12" text-anchor="end">{}</text>"#,
            p(14.0), p(gy - 7.0), Cat::Star.color(),
            p(18.0), p(gy - 3.0),
            p(28.0), p(gy), Cat::Grid.color(), xe(&g.name),
            p(SIDEBAR_W - 10.0), p(gy), xe(&g.value)
        ).ok();
        gy += 22.0;
    }
    s
}

// ── SVG defs (filters, patterns) ─────────────────────────────────────────────

const DEFS: &str = r##"<defs>
  <filter id="glow-gold" x="-30%" y="-30%" width="160%" height="160%">
    <feGaussianBlur in="SourceGraphic" stdDeviation="3" result="blur"/>
    <feMerge><feMergeNode in="blur"/><feMergeNode in="SourceGraphic"/></feMerge>
  </filter>
  <pattern id="grid-pat" x="0" y="0" width="40" height="40" patternUnits="userSpaceOnUse">
    <path d="M 40 0 L 0 0 0 40" fill="none" stroke="#ffffff" stroke-width="0.4"/>
  </pattern>
</defs>"##;

// ── Public entry point ────────────────────────────────────────────────────────

pub fn render(filename: &str, program: &Program) -> String {
    let doc = Document::build(filename, program);

    // Layout: pack cards into COLS columns using shortest-column strategy
    let heights: Vec<f32> = doc.funcs.iter().map(|f| card_height(f)).collect();
    let mut col_y = vec![CONTENT_Y; COLS];
    let mut positions: Vec<(f32, f32)> = Vec::with_capacity(doc.funcs.len());

    for &h in &heights {
        let (col, &cy) = col_y.iter().enumerate()
            .min_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap();
        let cx = GRID_X + col as f32 * (CARD_W + CARD_GAP);
        positions.push((cx, cy));
        col_y[col] += h + CARD_GAP;
    }

    let total_h = col_y.iter().cloned().fold(0.0f32, f32::max) + 40.0;

    let mut svg = String::new();
    write!(svg,
        r#"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="{SVG_W}" height="{}"
     style="font-family:'JetBrains Mono','Fira Code',monospace,sans-serif;background:{BG}">"#,
        total_h
    ).ok();

    svg.push_str(DEFS);

    // Background
    write!(svg, r#"<rect width="{SVG_W}" height="{total_h}" fill="{BG}"/>
                   <rect width="{SVG_W}" height="{total_h}" fill="url(#grid-pat)" opacity="0.05"/>"#,
        total_h = total_h).ok();

    svg.push_str(&render_header(&doc.filename, &doc.funcs));
    svg.push_str(&render_legend(&doc.funcs));
    svg.push_str(&render_sidebar(&doc.globals, total_h));

    for (card, &(cx, cy)) in doc.funcs.iter().zip(positions.iter()) {
        svg.push_str(&render_card(card, cx, cy));
    }

    svg.push_str("\n</svg>");
    svg
}
