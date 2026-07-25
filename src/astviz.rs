// src/astviz.rs — project-wide AST → SVG in three styles.
//
// A whole Ling project is treated as one program (all `.ling` files merged) while
// preserving each definition's *file scope*. Three renderings are produced:
//
//   • Technical — colour-coded function cards with geometric call icons, grouped
//     into per-file bands, with caller→callee graph edges.
//   • Artwork   — text-free, esoteric composition of translucent stars/polygons
//     (3–8 sides) clustered per file; a cubist sigil of the program's shape.
//   • Ling      — Ling-centric tiles: named, colour-coded, laid out per file with
//     balance and harmony in mind.
//
// Every SVG auto-fits its content (tight viewBox) and declares a 300-DPI physical
// size so raster exports are crisp. Output is written to ./AST/ by `ling ast`.

use crate::parser::ast::*;
use crate::visualize::{categorize, icon, is_ai, is_audio, is_crypto, is_physics, is_vtex, Cat};
use std::collections::HashSet;
use std::fmt::Write;

// ── Public API ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AstStyle {
    Technical,
    Artwork,
    Ling,
}

impl AstStyle {
    pub fn slug(self) -> &'static str {
        match self {
            AstStyle::Technical => "technical",
            AstStyle::Artwork => "artwork",
            AstStyle::Ling => "ling",
        }
    }
}

/// Render the merged project (a list of `(file_label, Program)`) in `style`.
pub fn render(style: AstStyle, project: &str, files: &[(String, Program)]) -> String {
    let model = Project::analyze(project, files);
    match style {
        AstStyle::Technical => render_technical(&model),
        AstStyle::Artwork => render_artwork(&model),
        AstStyle::Ling => render_ling(&model),
    }
}

// ── Analysed model (file scope preserved) ────────────────────────────────────

struct Call {
    name: String,
    cat: Cat,
    count: usize,
}

struct Func {
    name: String,
    params: Vec<String>,
    calls: Vec<Call>,
    has_loop: bool,
    is_entry: bool,
    vtex: usize,
    audio: usize,
    crypto: usize,
    physics: usize,
    ai: usize,
}

struct FileScope {
    label: String,
    funcs: Vec<Func>,
    globals: Vec<(String, String)>,
    uses: Vec<String>,
}

struct Project {
    name: String,
    files: Vec<FileScope>,
    fn_names: HashSet<String>,
}

const ENTRY_NAMES: &[&str] = &[
    "start",
    "main",
    "启",
    "เริ่ม",
    "시작",
    "始め",
    "始",
    "начать",
    "начало",
    "inicio",
    "comenzar",
    "début",
    "commencer",
    "anfang",
    "starten",
    "início",
];
fn is_entry(name: &str) -> bool {
    ENTRY_NAMES.contains(&name)
}

impl Project {
    fn analyze(name: &str, files: &[(String, Program)]) -> Self {
        // First pass: every function/entry name across the whole project (for edges).
        let mut fn_names = HashSet::new();
        for (_, prog) in files {
            collect_names(&prog.items, &mut fn_names);
        }

        let mut scopes = Vec::new();
        for (label, prog) in files {
            let mut funcs = Vec::new();
            let mut globals = Vec::new();
            let mut uses = Vec::new();
            collect_scope(&prog.items, "", &mut funcs, &mut globals, &mut uses);
            // Skip empty files so the canvas stays meaningful.
            if funcs.is_empty() && globals.is_empty() && uses.is_empty() {
                continue;
            }
            scopes.push(FileScope { label: short_label(label), funcs, globals, uses });
        }
        Project { name: name.to_string(), files: scopes, fn_names }
    }

    fn total_funcs(&self) -> usize {
        self.files.iter().map(|f| f.funcs.len()).sum()
    }

    /// `files · fns` plus any present domain tallies (vtex/audio/crypto/physics/ai).
    fn subtitle(&self) -> String {
        let mut parts = vec![
            format!("{} files", self.files.len()),
            format!("{} fns", self.total_funcs()),
        ];
        let (mut v, mut a, mut c, mut ph, mut ai) = (0, 0, 0, 0, 0);
        for fs in &self.files {
            for fc in &fs.funcs {
                v += fc.vtex;
                a += fc.audio;
                c += fc.crypto;
                ph += fc.physics;
                ai += fc.ai;
            }
        }
        for (n, lbl) in [
            (v, "vtex"),
            (a, "audio"),
            (c, "crypto"),
            (ph, "physics"),
            (ai, "ai"),
        ] {
            if n > 0 {
                parts.push(format!("{n} {lbl}"));
            }
        }
        parts.join(" · ")
    }
}

fn short_label(path: &str) -> String {
    let p = path.replace('\\', "/");
    p.rsplit('/').next().unwrap_or(&p).to_string()
}

fn collect_names(items: &[Item], out: &mut HashSet<String>) {
    for it in items {
        match it {
            Item::Fn(f) => {
                out.insert(f.name.clone());
            },
            Item::Bind(name, Expr::Do(_)) => {
                out.insert(name.clone());
            },
            Item::Mod(_, body) => collect_names(body, out),
            _ => {},
        }
    }
}

fn collect_scope(
    items: &[Item],
    ns: &str,
    funcs: &mut Vec<Func>,
    globals: &mut Vec<(String, String)>,
    uses: &mut Vec<String>,
) {
    let q = |n: &str| {
        if ns.is_empty() {
            n.to_string()
        } else {
            format!("{ns}::{n}")
        }
    };
    for it in items {
        match it {
            Item::Fn(f) => funcs.push(build_func(
                q(&f.name),
                f.params.clone(),
                &f.body,
                is_entry(&f.name),
            )),
            Item::Bind(name, Expr::Do(body)) => {
                funcs.push(build_func(q(name), vec![], body, is_entry(name)));
            },
            Item::Bind(name, expr) => globals.push((q(name), value_repr(expr))),
            Item::Mod(name, body) => collect_scope(body, &q(name), funcs, globals, uses),
            Item::Use { path, alias } => {
                let a = alias
                    .as_deref()
                    .map(|x| format!(" as {x}"))
                    .unwrap_or_default();
                uses.push(format!("{path}{a}"));
            },
            Item::TypeAlias(n, t) => globals.push((q(n), format!("type {t}"))),
            Item::Struct(n, fields) => {
                globals.push((q(n), format!("form {{{}}}", fields.join(", "))))
            },
            Item::Enum(n, variants) => {
                let names: Vec<&str> = variants.iter().map(|v| v.name.as_str()).collect();
                globals.push((q(n), format!("choose {{{}}}", names.join(" | "))));
            },
        }
    }
}

fn value_repr(e: &Expr) -> String {
    match e {
        Expr::Number(n) => {
            if n.fract() == 0.0 {
                format!("{}", *n as i64)
            } else {
                format!("{n:.2}")
            }
        },
        Expr::Str(_) => "\"…\"".into(),
        Expr::Bool(b) => format!("{b}"),
        Expr::Array(_) => "[…]".into(),
        _ => "…".into(),
    }
}

fn build_func(name: String, params: Vec<String>, body: &[Stmt], is_entry: bool) -> Func {
    let mut raw = Vec::new();
    let mut has_loop = false;
    walk_stmts(body, &mut raw, &mut has_loop);
    let calls = aggregate(raw);
    let sum = |pred: fn(Cat) -> bool| calls.iter().filter(|c| pred(c.cat)).map(|c| c.count).sum();
    let vtex = sum(is_vtex);
    let audio = sum(is_audio);
    let crypto = sum(is_crypto);
    let physics = sum(is_physics);
    let ai = sum(is_ai);
    Func {
        name,
        params,
        calls,
        has_loop,
        is_entry,
        vtex,
        audio,
        crypto,
        physics,
        ai,
    }
}

fn aggregate(raw: Vec<(String, Cat)>) -> Vec<Call> {
    let mut out: Vec<Call> = Vec::new();
    for (name, cat) in raw {
        if let Some(l) = out.last_mut() {
            if l.name == name {
                l.count += 1;
                continue;
            }
        }
        out.push(Call { name, cat, count: 1 });
    }
    out
}

// ── AST walking (thorough: catches calls in every expression position) ────────

fn walk_stmts(stmts: &[Stmt], out: &mut Vec<(String, Cat)>, lp: &mut bool) {
    for s in stmts {
        match s {
            Stmt::Expr(e) | Stmt::Return(e) | Stmt::Bind(_, e) => walk_expr(e, out, lp),
        }
    }
}

fn walk_expr(e: &Expr, out: &mut Vec<(String, Cat)>, lp: &mut bool) {
    match e {
        Expr::Call(func, args) => {
            let name = match func.as_ref() {
                Expr::Ident(n) => Some(n.clone()),
                Expr::Path(segs) => segs.last().cloned(),
                _ => {
                    walk_expr(func, out, lp);
                    None
                },
            };
            if let Some(n) = name {
                let cat = categorize(&n);
                out.push((n, cat));
            }
            for a in args {
                walk_expr(a, out, lp);
            }
        },
        Expr::MethodCall { receiver, method, args } => {
            walk_expr(receiver, out, lp);
            out.push((method.clone(), categorize(method)));
            for a in args {
                walk_expr(a, out, lp);
            }
        },
        Expr::While { cond, body } => {
            *lp = true;
            walk_expr(cond, out, lp);
            walk_stmts(body, out, lp);
        },
        Expr::For { iter, body, .. } => {
            *lp = true;
            walk_expr(iter, out, lp);
            walk_stmts(body, out, lp);
        },
        Expr::Do(ss) => walk_stmts(ss, out, lp),
        Expr::If { cond, then, elseifs, else_body } => {
            walk_expr(cond, out, lp);
            walk_stmts(then, out, lp);
            for (c, b) in elseifs {
                walk_expr(c, out, lp);
                walk_stmts(b, out, lp);
            }
            if let Some(b) = else_body {
                walk_stmts(b, out, lp);
            }
        },
        Expr::Match(scrut, arms) => {
            walk_expr(scrut, out, lp);
            for a in arms {
                walk_expr(&a.body, out, lp);
            }
        },
        Expr::BinOp(_, a, b) | Expr::Range(a, b) | Expr::Index(a, b) => {
            walk_expr(a, out, lp);
            walk_expr(b, out, lp);
        },
        Expr::Ref(x) | Expr::Await(x) => walk_expr(x, out, lp),
        Expr::Closure(_, body) => walk_expr(body, out, lp),
        Expr::Array(es) => {
            for a in es {
                walk_expr(a, out, lp);
            }
        },
        _ => {},
    }
}

// ── Shared SVG helpers ────────────────────────────────────────────────────────

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
fn f(v: f32) -> String {
    format!("{v:.2}")
}

/// Wrap a body in an auto-fit, 300-DPI SVG document. `w`/`h` are the content
/// extents in user units (px); physical size is `w/300 × h/300` inches.
fn svg_doc(w: f32, h: f32, bg_body: &str, body: &str) -> String {
    format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="{win:.3}in" height="{hin:.3}in" viewBox="0 0 {w} {h}" preserveAspectRatio="xMidYMid meet" style="font-family:'JetBrains Mono','Fira Code',monospace,sans-serif">
{bg_body}{body}
</svg>"##,
        win = w / 300.0,
        hin = h / 300.0,
        w = f(w),
        h = f(h),
    )
}

/// Star polygon points: `sides` points alternating outer/inner radius.
fn star_points(cx: f32, cy: f32, ro: f32, ri: f32, sides: usize, rot: f32) -> String {
    let n = sides.max(2);
    (0..n * 2)
        .map(|i| {
            let a = rot + i as f32 * std::f32::consts::PI / n as f32;
            let r = if i % 2 == 0 { ro } else { ri };
            format!("{},{}", f(cx + r * a.cos()), f(cy + r * a.sin()))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Regular polygon points.
fn ngon_points(cx: f32, cy: f32, r: f32, sides: usize, rot: f32) -> String {
    let n = sides.max(3);
    (0..n)
        .map(|i| {
            let a = rot - std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::TAU / n as f32;
            format!("{},{}", f(cx + r * a.cos()), f(cy + r * a.sin()))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Tiny deterministic hash → used for reproducible "random" placement.
fn hash(s: &str) -> u64 {
    let mut h = 1469598103934665603u64;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    h
}
fn frand(seed: u64, i: u64) -> f32 {
    let mut x = seed ^ i.wrapping_mul(0x9E3779B97F4A7C15);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58476D1CE4E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D049BB133111EB);
    x ^= x >> 31;
    (x as f64 / u64::MAX as f64) as f32
}

/// Sides (3..=8) chosen deterministically from a call category + name.
fn shape_sides(cat: Cat, name: &str) -> usize {
    let base = match cat {
        Cat::Star | Cat::Yantra | Cat::Sign | Cat::Shard => 5,
        Cat::Flower | Cat::Chakra | Cat::Cog | Cat::Music | Cat::Hash | Cat::Trig => 6,
        Cat::Lotus | Cat::Holo | Cat::Spectrum => 8,
        Cat::Present | Cat::Force | Cat::Fractal | Cat::Torii | Cat::Draw3D | Cat::Pagoda => 3,
        Cat::Fill
        | Cat::Grid
        | Cat::Window
        | Cat::Widget
        | Cat::Rigid
        | Cat::Key
        | Cat::Cipher
        | Cat::File => 4,
        Cat::Rain | Cat::Net | Cat::Neural | Cat::Sfx => 7,
        _ => 0,
    };
    if base != 0 {
        base
    } else {
        3 + (hash(name) % 6) as usize
    }
}

// ── Background ────────────────────────────────────────────────────────────────

const BG: &str = "#0b0b1a";

fn bg_rect(w: f32, h: f32, fill: &str) -> String {
    format!(
        r##"<rect x="0" y="0" width="{}" height="{}" fill="{}"/>"##,
        f(w),
        f(h),
        fill
    )
}

// ══════════════════════════════════════════════════════════════════════════════
// 1. TECHNICAL — file-banded function cards + call icons + graph edges
// ══════════════════════════════════════════════════════════════════════════════

const T_MARGIN: f32 = 36.0;
const T_TITLE_H: f32 = 96.0;
const T_CARD_W: f32 = 360.0;
const T_CARD_GAP: f32 = 16.0;
const T_COLS: usize = 3;
const T_FILE_HDR: f32 = 40.0;
const T_ICON: f32 = 24.0;
const T_ICONS_ROW: usize = 10;

fn t_card_h(fc: &Func) -> f32 {
    let rows = (fc.calls.len() + T_ICONS_ROW - 1).max(1) / T_ICONS_ROW + 1;
    24.0 * 2.0 + 30.0 + 18.0 + rows as f32 * (T_ICON + 5.0) + 6.0
}

fn render_technical(p: &Project) -> String {
    let content_w = T_COLS as f32 * (T_CARD_W + T_CARD_GAP) - T_CARD_GAP;
    let w = content_w + T_MARGIN * 2.0;

    // Layout pass: per file, pack cards into T_COLS shortest-column order.
    // Records (function-name → card-centre) for edges.
    struct Placed<'a> {
        fc: &'a Func,
        x: f32,
        y: f32,
        h: f32,
    }
    let mut placed: Vec<Placed> = Vec::new();
    let mut centers: std::collections::HashMap<String, (f32, f32)> =
        std::collections::HashMap::new();
    let mut file_bands: Vec<(String, usize, f32, f32)> = Vec::new(); // (label, n_glob, y, height)

    let mut y = T_TITLE_H + 10.0;
    for fs in &p.files {
        let band_top = y;
        y += T_FILE_HDR;
        let mut col_y = [y; T_COLS];
        for fc in &fs.funcs {
            let h = t_card_h(fc);
            let (col, cy) = col_y
                .iter()
                .enumerate()
                .min_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                .map(|(i, &v)| (i, v))
                .unwrap();
            let x = T_MARGIN + col as f32 * (T_CARD_W + T_CARD_GAP);
            centers.insert(fc.name.clone(), (x + T_CARD_W / 2.0, cy + h / 2.0));
            placed.push(Placed { fc, x, y: cy, h });
            col_y[col] = cy + h + T_CARD_GAP;
        }
        let band_bottom = col_y.iter().cloned().fold(y, f32::max);
        // Reserve room for globals/uses summary under the band.
        let extra = if fs.globals.is_empty() && fs.uses.is_empty() {
            0.0
        } else {
            26.0
        };
        file_bands.push((
            fs.label.clone(),
            fs.globals.len() + fs.uses.len(),
            band_top,
            band_bottom - band_top + extra,
        ));
        y = band_bottom + extra + 24.0;
    }
    let h = y + T_MARGIN;

    let mut body = String::new();
    body.push_str(DEFS_T);

    // Subtle grid overlay
    let _ = write!(
        body,
        r##"<rect width="{}" height="{}" fill="url(#tgrid)" opacity="0.06"/>"##,
        f(w),
        f(h)
    );

    // Title
    let _ = write!(
        body,
        r##"<text x="{}" y="44" fill="#ffd700" font-size="11" font-weight="bold" letter-spacing="3" opacity="0.7">LING · AST · TECHNICAL</text>
            <text x="{}" y="78" fill="#d8d8ff" font-size="30" font-weight="bold">{}</text>
            <text x="{}" y="78" fill="#52528a" font-size="13" text-anchor="end">{}</text>"##,
        f(T_MARGIN),
        f(T_MARGIN),
        esc(&p.name),
        f(w - T_MARGIN),
        esc(&p.subtitle())
    );

    // Graph edges (behind cards): caller centre → callee centre, faint curves.
    for pl in &placed {
        let (x0, y0) = *centers.get(&pl.fc.name).unwrap();
        for c in &pl.fc.calls {
            if !p.fn_names.contains(&c.name) {
                continue;
            }
            if let Some(&(x1, y1)) = centers.get(&c.name) {
                if (x0 - x1).abs() < 0.5 && (y0 - y1).abs() < 0.5 {
                    continue;
                }
                let mx = (x0 + x1) / 2.0;
                let _ = write!(
                    body,
                    r##"<path d="M {},{} C {},{} {},{} {},{}" fill="none" stroke="{}" stroke-width="1.1" opacity="0.22"/>"##,
                    f(x0),
                    f(y0),
                    f(mx),
                    f(y0),
                    f(mx),
                    f(y1),
                    f(x1),
                    f(y1),
                    c.cat.color()
                );
            }
        }
    }

    // File band frames + headers
    for (label, _n, by, bh) in &file_bands {
        let _ = write!(
            body,
            r##"<rect x="{}" y="{}" width="{}" height="{}" rx="12" fill="#10102a" opacity="0.45" stroke="#22225a" stroke-width="1"/>
                <text x="{}" y="{}" fill="#8be9fd" font-size="14" font-weight="bold">◈ {}</text>"##,
            f(T_MARGIN - 12.0),
            f(*by),
            f(content_w + 24.0),
            f(*bh),
            f(T_MARGIN),
            f(by + 26.0),
            esc(label)
        );
    }

    // Cards
    for pl in &placed {
        body.push_str(&t_card(pl.fc, pl.x, pl.y, pl.h));
    }

    // Per-file globals/uses footnote
    for (fs, (_, _, by, bh)) in p.files.iter().zip(file_bands.iter()) {
        if fs.globals.is_empty() && fs.uses.is_empty() {
            continue;
        }
        let fy = by + bh - 8.0;
        let mut parts: Vec<String> = fs
            .globals
            .iter()
            .map(|(n, v)| format!("◇ {n}={v}"))
            .collect();
        parts.extend(fs.uses.iter().map(|u| format!("use {u}")));
        let _ = write!(
            body,
            r##"<text x="{}" y="{}" fill="#52528a" font-size="10">{}</text>"##,
            f(T_MARGIN),
            f(fy),
            esc(&parts.join("    "))
        );
    }

    svg_doc(w, h, &bg_rect(w, h, BG), &body)
}

const DEFS_T: &str = r##"<defs>
  <filter id="glow-g" x="-30%" y="-30%" width="160%" height="160%"><feGaussianBlur in="SourceGraphic" stdDeviation="3" result="b"/><feMerge><feMergeNode in="b"/><feMergeNode in="SourceGraphic"/></feMerge></filter>
  <pattern id="tgrid" width="40" height="40" patternUnits="userSpaceOnUse"><path d="M 40 0 L 0 0 0 40" fill="none" stroke="#ffffff" stroke-width="0.4"/></pattern>
</defs>"##;

fn t_card(fc: &Func, x: f32, y: f32, h: f32) -> String {
    let dominant = fc
        .calls
        .iter()
        .max_by_key(|c| c.count)
        .map(|c| c.cat.color())
        .unwrap_or("#6ab0f5");
    let border = if fc.is_entry { "#ffd700" } else { dominant };
    let bw = if fc.is_entry { 2.5 } else { 1.2 };
    let glow = if fc.is_entry {
        r##" filter="url(#glow-g)""##
    } else {
        ""
    };
    let mut s = String::new();
    let _ = write!(
        s,
        r##"<rect x="{}" y="{}" width="{}" height="{}" rx="8" fill="#13132e" stroke="{}" stroke-width="{}"{}/>
           <rect x="{}" y="{}" width="4" height="{}" rx="2" fill="{}" opacity="0.7"/>"##,
        f(x),
        f(y),
        f(T_CARD_W),
        f(h),
        border,
        bw,
        glow,
        f(x + 2.0),
        f(y + 2.0),
        f(h - 4.0),
        border
    );

    let name_y = y + 26.0;
    let badge = if fc.is_entry {
        format!(
            r##"<text x="{}" y="{}" fill="#ffd700" font-size="9" font-weight="bold" text-anchor="end" opacity="0.85">⬡ ENTRY</text>"##,
            f(x + T_CARD_W - 12.0),
            f(name_y)
        )
    } else {
        String::new()
    };
    let _ = write!(
        s,
        r##"<text x="{}" y="{}" fill="{}" font-size="14" font-weight="bold">{}</text>{}"##,
        f(x + 16.0),
        f(name_y),
        if fc.is_entry { "#ffd700" } else { "#d0d0f0" },
        esc(&fc.name),
        badge
    );

    let stats_y = name_y + 18.0;
    let params = if fc.params.is_empty() {
        String::new()
    } else {
        format!("({})", fc.params.join(", "))
    };
    let mut st = Vec::new();
    if fc.vtex > 0 {
        st.push(format!("{} vtex", fc.vtex));
    }
    if fc.audio > 0 {
        st.push(format!("{} audio", fc.audio));
    }
    if fc.crypto > 0 {
        st.push(format!("{} crypto", fc.crypto));
    }
    if fc.physics > 0 {
        st.push(format!("{} phys", fc.physics));
    }
    if fc.ai > 0 {
        st.push(format!("{} ai", fc.ai));
    }
    if fc.has_loop {
        st.push("↺ loop".into());
    }
    let _ = write!(
        s,
        r##"<text x="{}" y="{}" fill="#52528a" font-size="10">{}</text>
           <text x="{}" y="{}" fill="#52528a" font-size="10" text-anchor="end">{}</text>
           <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="#22225a" stroke-width="1"/>"##,
        f(x + 16.0),
        f(stats_y),
        esc(&params),
        f(x + T_CARD_W - 12.0),
        f(stats_y),
        esc(&st.join("  ·  ")),
        f(x + 12.0),
        f(stats_y + 6.0),
        f(x + T_CARD_W - 12.0),
        f(stats_y + 6.0)
    );

    let iy0 = stats_y + 6.0 + 5.0 + T_ICON / 2.0;
    let ix0 = x + 16.0 + T_ICON / 2.0;
    let ir = T_ICON / 2.0 * 0.82;
    for (i, c) in fc.calls.iter().enumerate() {
        let row = i / T_ICONS_ROW;
        let col = i % T_ICONS_ROW;
        let ix = ix0 + col as f32 * (T_ICON + 5.0);
        let iy = iy0 + row as f32 * (T_ICON + 5.0);
        let _ = write!(
            s,
            r##"<rect x="{}" y="{}" width="{}" height="{}" rx="3" fill="{}" opacity="0.12"/>"##,
            f(ix - T_ICON / 2.0),
            f(iy - T_ICON / 2.0),
            f(T_ICON),
            f(T_ICON),
            c.cat.color()
        );
        s.push_str(&icon(c.cat, ix, iy, ir));
        if c.count > 1 {
            let _ = write!(
                s,
                r##"<rect x="{}" y="{}" width="13" height="10" rx="3" fill="{}" opacity="0.9"/>
                                  <text x="{}" y="{}" fill="#0a0a1a" font-size="8" font-weight="bold" text-anchor="middle">{}</text>"##,
                f(ix + ir - 2.0),
                f(iy - ir - 1.0),
                c.cat.color(),
                f(ix + ir + 4.5),
                f(iy - ir + 7.0),
                c.count
            );
        }
    }
    s
}

// ══════════════════════════════════════════════════════════════════════════════
// 2. ARTWORK — text-free cubist sigil: translucent stars/polygons per file
// ══════════════════════════════════════════════════════════════════════════════

const A_CELL: f32 = 620.0;

fn render_artwork(p: &Project) -> String {
    let nf = p.files.len().max(1);
    let cols = (nf as f32).sqrt().ceil() as usize;
    let rows = nf.div_ceil(cols);
    let w = cols as f32 * A_CELL;
    let h = rows as f32 * A_CELL;

    let mut body = String::new();
    body.push_str(DEFS_A);

    // Deep gradient field + a few huge faint background facets (cubist ground).
    let _ = write!(
        body,
        r##"<rect width="{}" height="{}" fill="url(#agrad)"/>"##,
        f(w),
        f(h)
    );
    let seed = hash(&p.name);
    for i in 0..7u64 {
        let cx = frand(seed, i) * w;
        let cy = frand(seed, i + 100) * h;
        let r = 120.0 + frand(seed, i + 200) * 260.0;
        let sides = 3 + (i % 5) as usize;
        let hue = (i * 47) % 360;
        let _ = write!(
            body,
            r##"<polygon points="{}" fill="hsl({},55%,55%)" opacity="0.05"/>"##,
            ngon_points(cx, cy, r, sides, frand(seed, i + 300) * std::f32::consts::TAU),
            hue
        );
    }

    // Per-file constellations.
    let mut centroids: Vec<(f32, f32)> = Vec::new();
    for (fi, fs) in p.files.iter().enumerate() {
        let gx = (fi % cols) as f32 * A_CELL + A_CELL / 2.0;
        let gy = (fi / cols) as f32 * A_CELL + A_CELL / 2.0;
        centroids.push((gx, gy));
        let fseed = hash(&fs.label);

        // Each function = a node placed by golden-angle phyllotaxis around the cell
        // centre; each of its calls = a translucent star/polygon around that node.
        let ga = 2.399963f32; // golden angle
        for (qi, fc) in fs.funcs.iter().enumerate() {
            let rr = 36.0 + 150.0 * ((qi as f32 + 0.5) / fs.funcs.len().max(1) as f32).sqrt();
            let na = qi as f32 * ga;
            let fx = gx + rr * na.cos();
            let fy = gy + rr * na.sin();

            // faint tether to cell centre
            let _ = write!(
                body,
                r##"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="#ffffff" stroke-width="1" opacity="0.06"/>"##,
                f(gx),
                f(gy),
                f(fx),
                f(fy)
            );

            let shapes = fc
                .calls
                .iter()
                .map(|c| c.count)
                .sum::<usize>()
                .clamp(1, 14);
            let fnseed = hash(&fc.name) ^ fseed;
            let pal: Vec<&str> = if fc.calls.is_empty() {
                vec!["#6ab0f5"]
            } else {
                fc.calls.iter().map(|c| c.cat.color()).collect()
            };
            for si in 0..shapes {
                let ang = frand(fnseed, si as u64) * std::f32::consts::TAU;
                let dist = frand(fnseed, si as u64 + 7) * 30.0;
                let sx = fx + dist * ang.cos();
                let sy = fy + dist * ang.sin();
                let ro = 14.0
                    + frand(fnseed, si as u64 + 11) * 34.0
                    + if fc.is_entry { 16.0 } else { 0.0 };
                let col = pal[si % pal.len()];
                let cat = fc
                    .calls
                    .get(si % fc.calls.len().max(1))
                    .map(|c| c.cat)
                    .unwrap_or(Cat::User);
                let sides = shape_sides(cat, &fc.name);
                let rot = frand(fnseed, si as u64 + 23) * std::f32::consts::TAU;
                let op = 0.28 + frand(fnseed, si as u64 + 31) * 0.30;
                // alternate filled star vs polygon for rhythm
                let pts = if si % 2 == 0 {
                    star_points(sx, sy, ro, ro * 0.45, sides, rot)
                } else {
                    ngon_points(sx, sy, ro, sides, rot)
                };
                let _ = write!(
                    body,
                    r##"<polygon points="{}" fill="{}" opacity="{:.2}" stroke="{}" stroke-width="0.6" stroke-opacity="0.4"/>"##,
                    pts, col, op, col
                );
            }
            // bright core for entry points
            if fc.is_entry {
                let _ = write!(
                    body,
                    r##"<circle cx="{}" cy="{}" r="9" fill="#ffd700" opacity="0.9"/>"##,
                    f(fx),
                    f(fy)
                );
            }
        }
    }

    // Faint cross-file relationship arcs between cell centroids (program weave).
    for (i, &(x0, y0)) in centroids.iter().enumerate() {
        for &(x1, y1) in centroids.iter().skip(i + 1) {
            let mx = (x0 + x1) / 2.0;
            let my = (y0 + y1) / 2.0 - 60.0;
            let _ = write!(
                body,
                r##"<path d="M {},{} Q {},{} {},{}" fill="none" stroke="#ffffff" stroke-width="0.8" opacity="0.05"/>"##,
                f(x0),
                f(y0),
                f(mx),
                f(my),
                f(x1),
                f(y1)
            );
        }
    }

    svg_doc(w, h, "", &body)
}

const DEFS_A: &str = r##"<defs>
  <radialGradient id="agrad" cx="50%" cy="42%" r="75%">
    <stop offset="0%" stop-color="#171733"/><stop offset="55%" stop-color="#0c0c1e"/><stop offset="100%" stop-color="#050510"/>
  </radialGradient>
</defs>"##;

// ══════════════════════════════════════════════════════════════════════════════
// 3. LING — harmonious file panels of colour-coded named tiles
// ══════════════════════════════════════════════════════════════════════════════

const L_MARGIN: f32 = 40.0;
const L_TILE_W: f32 = 188.0;
const L_TILE_H: f32 = 84.0;
const L_TILE_GAP: f32 = 16.0;
const L_PANEL_PAD: f32 = 22.0;
const L_TITLE_H: f32 = 92.0;

fn render_ling(p: &Project) -> String {
    // Choose a tile column count that keeps panels balanced (square-ish).
    let max_funcs = p
        .files
        .iter()
        .map(|f| f.funcs.len())
        .max()
        .unwrap_or(1)
        .max(1);
    let tiles_per_row = ((max_funcs as f32).sqrt().ceil() as usize).clamp(2, 6);
    let panel_w = L_PANEL_PAD * 2.0 + tiles_per_row as f32 * (L_TILE_W + L_TILE_GAP) - L_TILE_GAP;
    let w = panel_w + L_MARGIN * 2.0;

    // Layout panels top-to-bottom, centred.
    let mut panels: Vec<(usize, f32, f32)> = Vec::new(); // (file index, y, height)
    let mut y = L_TITLE_H + 12.0;
    for (fi, fs) in p.files.iter().enumerate() {
        let n = fs.funcs.len().max(1);
        let rows = n.div_ceil(tiles_per_row);
        let body_h = rows as f32 * (L_TILE_H + L_TILE_GAP) - L_TILE_GAP;
        let glob_h = if fs.globals.is_empty() && fs.uses.is_empty() {
            0.0
        } else {
            30.0
        };
        let ph = 44.0 + body_h + glob_h + L_PANEL_PAD;
        panels.push((fi, y, ph));
        y += ph + 26.0;
    }
    let h = y + L_MARGIN;

    let mut body = String::new();
    body.push_str(DEFS_L);

    // Title
    let _ = write!(
        body,
        r##"<text x="{}" y="42" fill="#ffd700" font-size="11" font-weight="bold" letter-spacing="3" opacity="0.75">灵 · LING · AST</text>
            <text x="{}" y="78" fill="#e6e6ff" font-size="30" font-weight="bold">{}</text>
            <text x="{}" y="78" fill="#6a6aa0" font-size="13" text-anchor="end">{}</text>"##,
        f(L_MARGIN),
        f(L_MARGIN),
        esc(&p.name),
        f(w - L_MARGIN),
        esc(&p.subtitle())
    );

    for (fi, py, ph) in &panels {
        let fs = &p.files[*fi];
        let px = L_MARGIN;
        // Panel frame
        let _ = write!(
            body,
            r##"<rect x="{}" y="{}" width="{}" height="{}" rx="16" fill="#101028" stroke="#26264e" stroke-width="1.2"/>
                <rect x="{}" y="{}" width="{}" height="34" rx="16" fill="#16163a"/>
                <text x="{}" y="{}" fill="#9bd8ff" font-size="15" font-weight="bold">▦ {}</text>
                <text x="{}" y="{}" fill="#6a6aa0" font-size="11" text-anchor="end">{} fns</text>"##,
            f(px),
            f(*py),
            f(panel_w),
            f(*ph),
            f(px),
            f(*py),
            f(panel_w),
            f(px + 16.0),
            f(py + 23.0),
            esc(&fs.label),
            f(px + panel_w - 14.0),
            f(py + 23.0),
            fs.funcs.len()
        );

        // Tiles
        let t0y = py + 44.0;
        for (i, fc) in fs.funcs.iter().enumerate() {
            let row = i / tiles_per_row;
            let col = i % tiles_per_row;
            let tx = px + L_PANEL_PAD + col as f32 * (L_TILE_W + L_TILE_GAP);
            let ty = t0y + row as f32 * (L_TILE_H + L_TILE_GAP);
            body.push_str(&l_tile(fc, tx, ty));
        }

        // Globals / uses strip
        if !fs.globals.is_empty() || !fs.uses.is_empty() {
            let rows = fs.funcs.len().max(1).div_ceil(tiles_per_row);
            let gy = t0y + rows as f32 * (L_TILE_H + L_TILE_GAP) + 6.0;
            let mut parts: Vec<String> = fs
                .globals
                .iter()
                .map(|(n, v)| format!("◇ {n} = {v}"))
                .collect();
            parts.extend(fs.uses.iter().map(|u| format!("⇥ use {u}")));
            let _ = write!(
                body,
                r##"<text x="{}" y="{}" fill="#6a6aa0" font-size="11">{}</text>"##,
                f(px + L_PANEL_PAD),
                f(gy),
                esc(&parts.join("     "))
            );
        }
    }

    svg_doc(w, h, &bg_rect(w, h, "#08081a"), &body)
}

const DEFS_L: &str = r##"<defs>
  <filter id="tile-sh" x="-20%" y="-20%" width="140%" height="160%"><feDropShadow dx="0" dy="2" stdDeviation="2" flood-color="#000000" flood-opacity="0.45"/></filter>
</defs>"##;

fn l_tile(fc: &Func, x: f32, y: f32) -> String {
    let dom = fc
        .calls
        .iter()
        .max_by_key(|c| c.count)
        .map(|c| c.cat)
        .unwrap_or(Cat::User);
    let col = if fc.is_entry { "#ffd700" } else { dom.color() };
    let mut s = String::new();
    let _ = write!(
        s,
        r##"<rect x="{}" y="{}" width="{}" height="{}" rx="12" fill="#191940" stroke="{}" stroke-width="{}" filter="url(#tile-sh)"/>
           <rect x="{}" y="{}" width="{}" height="7" rx="3" fill="{}"/>"##,
        f(x),
        f(y),
        f(L_TILE_W),
        f(L_TILE_H),
        col,
        if fc.is_entry { 2.4 } else { 1.2 },
        f(x + 10.0),
        f(y + 12.0),
        f(L_TILE_W - 20.0),
        col
    );

    // Name (truncated to fit)
    let name = if fc.name.chars().count() > 20 {
        format!("{}…", fc.name.chars().take(19).collect::<String>())
    } else {
        fc.name.clone()
    };
    let _ = write!(
        s,
        r##"<text x="{}" y="{}" fill="{}" font-size="14" font-weight="bold">{}{}</text>"##,
        f(x + 12.0),
        f(y + 38.0),
        if fc.is_entry { "#ffd700" } else { "#e0e0ff" },
        if fc.is_entry { "⬡ " } else { "" },
        esc(&name)
    );

    // Category dots (one per distinct call category, up to 7)
    let mut seen = HashSet::new();
    let mut dots: Vec<Cat> = Vec::new();
    for c in &fc.calls {
        if seen.insert(c.cat) {
            dots.push(c.cat);
        }
    }
    for (i, c) in dots.iter().take(7).enumerate() {
        let _ = write!(
            s,
            r##"<circle cx="{}" cy="{}" r="4.5" fill="{}"/>"##,
            f(x + 16.0 + i as f32 * 13.0),
            f(y + 56.0),
            c.color()
        );
    }

    // Stats line
    let mut st = Vec::new();
    if !fc.params.is_empty() {
        st.push(format!("{}p", fc.params.len()));
    }
    if !fc.calls.is_empty() {
        st.push(format!(
            "{} calls",
            fc.calls.iter().map(|c| c.count).sum::<usize>()
        ));
    }
    if fc.has_loop {
        st.push("↺".into());
    }
    let _ = write!(
        s,
        r##"<text x="{}" y="{}" fill="#7a7ab0" font-size="10" text-anchor="end">{}</text>"##,
        f(x + L_TILE_W - 12.0),
        f(y + L_TILE_H - 10.0),
        esc(&st.join("  "))
    );
    s
}
