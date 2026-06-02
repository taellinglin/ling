// src/runtime/mod.rs — tree-walking interpreter with graphics support
use std::cell::RefCell;
use std::collections::HashMap;
use crate::parser::ast::*;
use crate::gfx::{GfxState, Light};
#[cfg(not(target_arch = "wasm32"))]
use crate::gfx::raster::{fill_triangle, draw_line};
#[cfg(not(target_arch = "wasm32"))]
use ling_audio::{AudioEngine, ToneParams};

#[cfg(not(target_arch = "wasm32"))]
use ling_audio::FftAnalyzer;

// ─── Values ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Value {
    Str(String),
    Number(f64),
    Bool(bool),
    Unit,
    List(Vec<Value>),
    Ok(Box<Value>),
    Err(Box<Value>),
    Fn(Vec<String>, Vec<Stmt>, Env),
}

type Env = HashMap<String, Value>;

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Str(s)    => write!(f, "{s}"),
            Value::Number(n) => {
                if n.fract() == 0.0 && n.abs() < 1e15 { write!(f, "{}", *n as i64) }
                else { write!(f, "{n}") }
            }
            Value::Bool(b)   => write!(f, "{b}"),
            Value::Unit      => write!(f, "()"),
            Value::List(v)   => {
                write!(f, "[")?;
                for (i, x) in v.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{x}")?;
                }
                write!(f, "]")
            }
            Value::Ok(v)     => write!(f, "Ok({v})"),
            Value::Err(v)    => write!(f, "Err({v})"),
            Value::Fn(_, _, _) => write!(f, "<fn>"),
        }
    }
}

// ─── Control flow ────────────────────────────────────────────────────────────

#[derive(Debug)]
enum EvalErr {
    Runtime(String),
    Return(Value),
    #[allow(dead_code)] // reserved for future `break` statement support
    Break,
}

impl From<String> for EvalErr {
    fn from(s: String) -> Self { EvalErr::Runtime(s) }
}

type EvalResult = Result<Value, EvalErr>;

// GfxState is now defined in crate::gfx — see src/gfx/mod.rs.

// ─── SVG writer ───────────────────────────────────────────────────────────────

struct SvgWriter {
    path:     String,
    width:    f64,
    height:   f64,
    elements: Vec<String>,
}

impl SvgWriter {
    fn new(path: String, width: f64, height: f64) -> Self {
        let bg = format!(
            "<rect width=\"{width}\" height=\"{height}\" fill=\"#0a0a0a\"/>"
        );
        Self { path, width, height, elements: vec![bg] }
    }

    fn save(&self) -> std::io::Result<()> {
        let w = self.width;
        let h = self.height;
        let mut out = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <svg xmlns=\"http://www.w3.org/2000/svg\" \
             width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">\n"
        );
        for elem in &self.elements {
            out.push_str("  ");
            out.push_str(elem);
            out.push('\n');
        }
        out.push_str("</svg>\n");
        // Create parent directory if it doesn't exist.
        if let Some(parent) = std::path::Path::new(&self.path).parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        std::fs::write(&self.path, out.as_bytes())
    }
}

fn hsl_to_hex(h: f64, s: f64, l: f64) -> String {
    let s = s / 100.0;
    let l = l / 100.0;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r1, g1, b1) = if h < 60.0       { (c, x, 0.0) }
                       else if h < 120.0  { (x, c, 0.0) }
                       else if h < 180.0  { (0.0, c, x) }
                       else if h < 240.0  { (0.0, x, c) }
                       else if h < 300.0  { (x, 0.0, c) }
                       else               { (c, 0.0, x) };
    let r = ((r1 + m) * 255.0).round() as u8;
    let g = ((g1 + m) * 255.0).round() as u8;
    let b = ((b1 + m) * 255.0).round() as u8;
    format!("#{r:02x}{g:02x}{b:02x}")
}

// ─── Procedural texture helpers ───────────────────────────────────────────────

fn tex_hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_add((y as u32).wrapping_mul(2654435769)).wrapping_add(seed.wrapping_mul(1234567891));
    h ^= h >> 16; h = h.wrapping_mul(0x45d9f3b); h ^= h >> 16;
    h as f32 / u32::MAX as f32
}

fn tex_vnoise(x: f32, y: f32, seed: u32) -> f32 {
    let xi = x.floor() as i32; let yi = y.floor() as i32;
    let sm = |t: f32| t * t * (3.0 - 2.0 * t);
    let xf = sm(x - xi as f32); let yf = sm(y - yi as f32);
    let a = tex_hash(xi, yi, seed); let b = tex_hash(xi+1, yi, seed);
    let c = tex_hash(xi, yi+1, seed); let d = tex_hash(xi+1, yi+1, seed);
    a + (b-a)*xf + (c-a)*yf + (a-b-c+d)*xf*yf
}

fn tex_fbm(x: f32, y: f32, octaves: u32, seed: u32) -> f32 {
    let mut v = 0.0f32; let mut amp = 0.5f32; let mut f = 1.0f32;
    for i in 0..octaves {
        v += tex_vnoise(x * f, y * f, seed.wrapping_add(i * 7919)) * amp;
        amp *= 0.5; f *= 2.0;
    }
    v
}

fn tex_palette(name: &str, t: f32) -> [f32; 3] {
    let (a, b, c, d): ([f32;3],[f32;3],[f32;3],[f32;3]) = match name {
        "fire"        => ([0.8,0.4,0.1],[0.7,0.3,0.1],[1.0,0.5,0.3],[0.0,0.5,0.8]),
        "ocean"       => ([0.1,0.4,0.7],[0.3,0.3,0.4],[0.8,1.0,0.5],[0.3,0.0,0.6]),
        "psychedelic" => ([0.5,0.5,0.5],[0.8,0.8,0.8],[1.0,1.3,0.7],[0.0,0.15,0.3]),
        "neon"        => ([0.5,0.5,0.5],[0.5,0.5,0.5],[2.0,1.0,0.0],[0.5,0.2,0.25]),
        "forest"      => ([0.3,0.5,0.2],[0.2,0.3,0.1],[1.0,0.5,0.8],[0.1,0.3,0.6]),
        _             => ([0.5,0.5,0.5],[0.5,0.5,0.5],[1.0,1.0,1.0],[0.0,0.333,0.667]),
    };
    [0,1,2].map(|i| (a[i] + b[i] * (std::f32::consts::TAU * (c[i] * t + d[i])).cos()).clamp(0.0, 1.0))
}

fn tex_rgb(r: f32, g: f32, b: f32) -> u32 {
    ((r * 255.0) as u32) << 16 | ((g * 255.0) as u32) << 8 | (b * 255.0) as u32
}

// ─── Interpreter ─────────────────────────────────────────────────────────────

pub struct Interpreter {
    globals:   HashMap<String, Expr>,
    functions: HashMap<String, FnDef>,
    _modules:  HashMap<String, Vec<FnDef>>,
    gfx:       RefCell<GfxState>,
    svg:       RefCell<Option<SvgWriter>>,
    /// Directory of the primary source file, for relative `use` resolution.
    pub source_dir: Option<std::path::PathBuf>,
    /// Files already loaded — prevents circular imports.
    loaded_files: std::collections::HashSet<String>,
    /// Optional audio engine — `None` if no audio device is available.
    #[cfg(not(target_arch = "wasm32"))]
    audio:     Option<AudioEngine>,
    #[cfg(not(target_arch = "wasm32"))]
    fft:       RefCell<FftAnalyzer>,
    fft_bands_cache: RefCell<Vec<f32>>,
}

impl Interpreter {
    pub fn new() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let audio = AudioEngine::new()
            .map_err(|e| eprintln!("audio init failed (no sound): {e}"))
            .ok();
        Self {
            globals:   HashMap::new(),
            functions: HashMap::new(),
            _modules:  HashMap::new(),
            gfx:       RefCell::new(GfxState::new()),
            svg:       RefCell::new(None),
            source_dir: None,
            loaded_files: std::collections::HashSet::new(),
            #[cfg(not(target_arch = "wasm32"))]
            audio,
            #[cfg(not(target_arch = "wasm32"))]
            fft: RefCell::new(FftAnalyzer::new(2048, 44100)),
            fft_bands_cache: RefCell::new(vec![]),
        }
    }

    pub fn run_program(&mut self, program: &Program) -> Result<(), String> {
        for item in &program.items {
            self.register_item("", item)?;
        }
        let entry = self.find_entry()
            .ok_or("no entry point — need `bind start = do {...}` or `ผูก เริ่ม = ทำ {...}`")?;
        // Seed the entry env with non-Do globals so top-level `令` bindings
        // are visible in the main Do block (same two-pass logic as call_named).
        let mut env = Env::new();
        let non_do: Vec<_> = self.globals.iter()
            .filter(|(_, e)| !matches!(e, Expr::Do(_)))
            .map(|(k, e)| (k.clone(), e.clone()))
            .collect();
        let mut pending: Vec<(String, Expr)> = Vec::new();
        for (k, expr) in &non_do {
            let mut tmp = Env::new();
            if let Ok(v) = self.eval_expr(expr, &mut tmp) {
                env.insert(k.clone(), v);
            } else {
                pending.push((k.clone(), expr.clone()));
            }
        }
        for (k, expr) in &pending {
            let mut tmp = env.clone();
            if let Ok(v) = self.eval_expr(expr, &mut tmp) {
                env.insert(k.clone(), v);
            }
        }
        self.eval_expr(&entry, &mut env).map(|_| ()).map_err(|e| match e {
            EvalErr::Runtime(s) => s,
            EvalErr::Return(_)  => "unexpected top-level return".to_string(),
            EvalErr::Break      => "unexpected break at top level".to_string(),
        })
    }

    fn register_item(&mut self, ns: &str, item: &Item) -> Result<(), String> {
        match item {
            Item::Bind(name, expr) => {
                let key = if ns.is_empty() { name.clone() } else { format!("{ns}::{name}") };
                self.globals.insert(key, expr.clone());
            }
            Item::Fn(def) => {
                let key = if ns.is_empty() { def.name.clone() } else { format!("{ns}::{}", def.name) };
                self.functions.insert(key, def.clone());
            }
            Item::Mod(name, body) => {
                let child_ns = if ns.is_empty() { name.clone() } else { format!("{ns}::{name}") };
                for child in body {
                    self.register_item(&child_ns, child)?;
                }
            }
            Item::TypeAlias(_, _) => {}
            Item::Use { path, alias } => {
                self.load_module(path, alias.as_deref(), ns)?;
            }
        }
        Ok(())
    }

    /// Resolve `path` relative to `source_dir`, load and parse it, then
    /// register all its definitions.  If `alias` is given, every name is
    /// prefixed with `<parent_ns>::<alias>`.  Circular imports are silently
    /// skipped.
    fn load_module(&mut self, path: &str, alias: Option<&str>, parent_ns: &str) -> Result<(), String> {
        // Build candidate file paths (.ling extension variants)
        let base_dir = self.source_dir.clone().unwrap_or_else(|| std::path::PathBuf::from("."));
        let raw = std::path::Path::new(path);
        let candidates: Vec<std::path::PathBuf> = vec![
            base_dir.join(format!("{}.ling", path)),
            base_dir.join(format!("{}.灵", path)),
            base_dir.join(format!("{}.령", path)),
            base_dir.join(format!("{}.霊", path)),
            base_dir.join(format!("{}.ลิง", path)),
            // exact path if already has extension
            base_dir.join(raw),
            std::path::PathBuf::from(format!("{}.ling", path)),
            std::path::PathBuf::from(path),
        ];

        let resolved = candidates.into_iter().find(|p| p.exists())
            .ok_or_else(|| format!("use: cannot find module '{path}'"))?;

        let canonical = resolved.canonicalize()
            .unwrap_or_else(|_| resolved.clone())
            .to_string_lossy()
            .to_string();

        // Skip if already loaded (circular import guard)
        if self.loaded_files.contains(&canonical) {
            return Ok(());
        }
        self.loaded_files.insert(canonical.clone());

        let source = std::fs::read_to_string(&resolved)
            .map_err(|e| format!("use: failed to read '{path}': {e}"))?;

        // Save/restore source_dir for nested relative imports
        let prev_dir = self.source_dir.clone();
        self.source_dir = resolved.parent().map(|p| p.to_path_buf());

        let program = crate::parser::parse(&source)
            .map_err(|e| format!("use: parse error in '{path}': {e}"))?;

        // Compute target namespace: parent_ns :: alias (or just alias, or just parent_ns)
        let target_ns = match (parent_ns.is_empty(), alias) {
            (_, Some(a)) if !parent_ns.is_empty() => format!("{parent_ns}::{a}"),
            (_, Some(a)) => a.to_string(),
            (false, None) => parent_ns.to_string(),
            (true,  None) => String::new(),
        };

        for item in &program.items {
            self.register_item(&target_ns, item)?;
        }

        self.source_dir = prev_dir;
        Ok(())
    }

    fn find_entry(&self) -> Option<Expr> {
        // Try all known entry-point names in multiple human languages
        for key in &[
            "start", "main",
            "启",
            "เริ่ม",           // Thai
            "시작",
            "начать", "начало",
            "inicio", "comenzar",
            "début", "commencer",
            "anfang", "starten",
            "início",
            "शुरू",
            "ابدأ",
        ] {
            if let Some(e) = self.globals.get(*key) { return Some(e.clone()); }
        }
        self.globals.values().find(|e| matches!(e, Expr::Do(_))).cloned()
    }

    // ─── Expression evaluation ────────────────────────────────────────────────

    fn eval_expr(&self, expr: &Expr, env: &mut Env) -> EvalResult {
        match expr {
            Expr::Str(s)    => Ok(Value::Str(s.clone())),
            Expr::Number(n) => Ok(Value::Number(*n)),
            Expr::Bool(b)   => Ok(Value::Bool(*b)),
            Expr::Unit      => Ok(Value::Unit),
            Expr::Array(elems) => {
                let vs: Vec<_> = elems.iter()
                    .map(|e| self.eval_expr(e, env))
                    .collect::<Result<_,_>>()?;
                Ok(Value::List(vs))
            }

            Expr::Ident(name) => self.lookup(name, env),

            Expr::Path(segs) => {
                if segs.len() == 1 { return self.lookup(&segs[0], env); }
                Ok(Value::Str(segs.join("::")))
            }

            Expr::Ref(inner) => self.eval_expr(inner, env),
            Expr::Await(inner) => self.eval_expr(inner, env),

            Expr::Do(stmts) => {
                let mut local = env.clone();
                Ok(self.exec_block(stmts, &mut local)?.unwrap_or(Value::Unit))
            }

            Expr::BinOp(op, lhs, rhs) => {
                let l = self.eval_expr(lhs, env)?;
                let r = self.eval_expr(rhs, env)?;
                self.apply_binop(op, l, r)
            }

            Expr::If { cond, then, elseifs, else_body } => {
                if self.is_truthy(&self.eval_expr(cond, env)?) {
                    return Ok(self.exec_block(then, env)?.unwrap_or(Value::Unit));
                }
                for (ei_cond, ei_body) in elseifs {
                    if self.is_truthy(&self.eval_expr(ei_cond, env)?) {
                        return Ok(self.exec_block(ei_body, env)?.unwrap_or(Value::Unit));
                    }
                }
                if let Some(eb) = else_body {
                    return Ok(self.exec_block(eb, env)?.unwrap_or(Value::Unit));
                }
                Ok(Value::Unit)
            }

            Expr::While { cond, body } => {
                // Run the body directly in the *outer* env so that
                // `bind counter = counter + 1` persists across iterations,
                // which is the expected behaviour in a scripting language.
                loop {
                    let cv = self.eval_expr(cond, env)?;
                    if !self.is_truthy(&cv) { break; }
                    match self.exec_block(body, env) {
                        Ok(_) => {}
                        Err(EvalErr::Break) => break,
                        Err(e) => return Err(e),
                    }
                }
                Ok(Value::Unit)
            }

            Expr::For { var, iter, body } => {
                let iter_val = self.eval_expr(iter, env)?;
                let items = self.value_to_iter(iter_val)?;
                for item in items {
                    let mut local = env.clone();
                    local.insert(var.clone(), item);
                    match self.exec_block(body, &mut local) {
                        Ok(_) => {}
                        Err(EvalErr::Break) => break,
                        Err(e) => return Err(e),
                    }
                }
                Ok(Value::Unit)
            }

            Expr::Match(subject, arms) => {
                let subj = self.eval_expr(subject, env)?;
                for arm in arms {
                    if let Some(bindings) = self.match_pattern(&arm.pattern, &subj) {
                        let mut local = env.clone();
                        local.extend(bindings);
                        return self.eval_expr(&arm.body, &mut local);
                    }
                }
                Ok(Value::Unit)
            }

            Expr::Range(lo, hi) => {
                let lo_v = self.eval_expr(lo, env)?;
                let hi_v = self.eval_expr(hi, env)?;
                let lo_n = self.to_number(&lo_v)? as i64;
                let hi_n = self.to_number(&hi_v)? as i64;
                Ok(Value::List((lo_n..hi_n).map(|i| Value::Number(i as f64)).collect()))
            }

            Expr::Index(base, idx) => {
                let b = self.eval_expr(base, env)?;
                let i = self.eval_expr(idx, env)?;
                let n = self.to_number(&i)? as usize;
                match b {
                    Value::List(v) => v.get(n).cloned()
                        .ok_or_else(|| EvalErr::from(format!("index {n} out of bounds"))),
                    Value::Str(s)  => s.chars().nth(n)
                        .map(|c| Value::Str(c.to_string()))
                        .ok_or_else(|| EvalErr::from(format!("index {n} out of bounds"))),
                    other => Err(EvalErr::from(format!("cannot index {:?}", other))),
                }
            }

            Expr::Call(callee, args) => {
                let arg_vals: Vec<Value> = args.iter()
                    .map(|a| self.eval_expr(a, env))
                    .collect::<Result<_,_>>()?;
                match callee.as_ref() {
                    Expr::Ident(name) => self.call_named(name, arg_vals, env),
                    Expr::Path(segs)  => self.call_named(&segs.join("::"), arg_vals, env),
                    _ => {
                        let v = self.eval_expr(callee, env)?;
                        self.call_value(v, arg_vals)
                    }
                }
            }

            Expr::MethodCall { receiver, method, args } => {
                let recv = self.eval_expr(receiver, env)?;
                let arg_vals: Vec<Value> = args.iter()
                    .map(|a| self.eval_expr(a, env))
                    .collect::<Result<_,_>>()?;
                self.call_method(recv, method, arg_vals)
            }

            Expr::Closure(params, body) => {
                Ok(Value::Fn(params.clone(), vec![Stmt::Expr(*body.clone())], env.clone()))
            }
        }
    }

    // ─── Block execution ─────────────────────────────────────────────────────

    fn exec_block(&self, stmts: &[Stmt], env: &mut Env) -> Result<Option<Value>, EvalErr> {
        let mut last: Option<Value> = None;
        for stmt in stmts {
            match stmt {
                Stmt::Bind(name, expr) => {
                    let v = self.eval_expr(expr, env)?;
                    env.insert(name.clone(), v);
                    last = None;
                }
                Stmt::Return(expr) => {
                    let v = self.eval_expr(expr, env)?;
                    return Err(EvalErr::Return(v));
                }
                Stmt::Expr(expr) => {
                    last = Some(self.eval_expr(expr, env)?);
                }
            }
        }
        Ok(last)
    }

    // ─── Dispatch helpers ─────────────────────────────────────────────────────

    fn lookup(&self, name: &str, env: &Env) -> EvalResult {
        if let Some(v) = env.get(name) { return Ok(v.clone()); }
        if self.functions.contains_key(name) {
            let def = &self.functions[name];
            return Ok(Value::Fn(def.params.clone(), def.body.clone(), Env::new()));
        }
        // Math constants usable as plain identifiers (e.g. `sin(pi)`)
        match name {
            "pi" | "π" | "พาย" => return Ok(Value::Number(std::f64::consts::PI)),
            "tau" | "τ"        => return Ok(Value::Number(std::f64::consts::TAU)),
            _ => {}
        }
        Err(EvalErr::from(format!("undefined: '{name}'")))
    }

    fn call_named(&self, name: &str, args: Vec<Value>, env: &Env) -> EvalResult {
        match name {
            // ── Print ──
            "print" | "println" | "印" | "พิมพ์" | "출력" | "вывести" | "imprimir" | "afficher" => {
                let s = args.iter().map(|v| v.to_string()).collect::<Vec<_>>().join("");
                println!("{s}");
                return Ok(Value::Unit);
            }
            // ── Format ──
            "format" | "格式" | "รูปแบบ" | "форматировать" | "formatear" | "formater" => {
                return Ok(Value::Str(self.builtin_format(&args)?));
            }
            // ── String join / concatenation ──
            "格式::拼接" | "format::join" => {
                match args.first() {
                    Some(Value::List(items)) => {
                        return Ok(Value::Str(items.iter().map(|v| v.to_string()).collect()));
                    }
                    _ => return Ok(Value::Str(self.builtin_format(&args)?)),
                }
            }
            // ── Result constructors ──
            "ok" | "好" => {
                let val = args.into_iter().next().unwrap_or(Value::Unit);
                return Ok(Value::Ok(Box::new(val)));
            }
            "bad" | "坏" | "err" => {
                let val = args.into_iter().next().unwrap_or(Value::Unit);
                return Ok(Value::Err(Box::new(val)));
            }
            // ── Vec constructors ──
            "向量::从" | "Vec::from" => {
                if let Some(Value::List(v)) = args.first() {
                    return Ok(Value::List(v.clone()));
                }
                return Ok(Value::List(args));
            }
            "向量::有容量" | "Vec::with_capacity" => return Ok(Value::List(Vec::new())),
            // ── Timer stubs ──
            "计时::获取当前小时" | "Timer::hour" => return Ok(Value::Number(14.0)),
            "计时::现在" | "Timer::now"          => return Ok(Value::Number(1000.0)),
            // ── Sleep ──
            "sleep" | "หยุด" | "sleep_ms" | "流水::睡眠" | "Flow::sleep" => {
                if let Some(ms_val) = args.first() {
                    if let Ok(ms) = self.to_number(ms_val) {
                        std::thread::sleep(std::time::Duration::from_millis(ms as u64));
                    }
                }
                return Ok(Value::Unit);
            }
            // ── Flow::parallel stub ──
            "流水::并行" | "Flow::parallel" => {
                if let Some(Value::Fn(params, body, mut cap)) = args.first().cloned() {
                    let _ = params;
                    match self.exec_block(&body, &mut cap) {
                        Ok(Some(v)) => return Ok(v),
                        Ok(None) => return Ok(Value::Unit),
                        Err(EvalErr::Return(v)) => return Ok(v),
                        Err(e) => return Err(e),
                    }
                }
                return Ok(Value::Unit);
            }

            // ══════════════════════════════════════════════════════════════════
            // MATH BUILTINS  (all args and results are f64)
            // Thai aliases: ไซน์ โคไซน์ แทนเจนต์ รากที่สอง ค่าสัมบูรณ์
            //               ปัดลง ปัดขึ้น ปัดเศษ ตัดทศนิยม ต่ำสุด สูงสุด
            //               จำกัด ยกกำลัง ลอการิทึม พาย
            // ══════════════════════════════════════════════════════════════════

            // ── Trigonometry (input in radians) ──
            "sin" | "ไซน์" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.sin()));
            }
            "cos" | "โคไซน์" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.cos()));
            }

            // ── Hyperbolic functions ──
            // Hyperbolic tangent
            "tanh" | "tanhf" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.tanh()));
            }


            "tan" | "แทนเจนต์" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.tan()));
            }
            "asin" | "arcsin" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.asin()));
            }
            "acos" | "arccos" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.acos()));
            }
            "atan" | "arctan" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.atan()));
            }
            "atan2" | "arctan2" => {
                let y = self.arg_num(&args, 0, 0.0)?;
                let x = self.arg_num(&args, 1, 1.0)?;
                return Ok(Value::Number(y.atan2(x)));
            }

            // ── Roots / powers ──
            "sqrt" | "รากที่สอง" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.sqrt()));
            }
            "cbrt" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.cbrt()));
            }
            "pow" | "ยกกำลัง" => {
                let base = self.arg_num(&args, 0, 0.0)?;
                let exp  = self.arg_num(&args, 1, 1.0)?;
                return Ok(Value::Number(base.powf(exp)));
            }
            "exp" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.exp()));
            }
            "hypot" => {
                let x = self.arg_num(&args, 0, 0.0)?;
                let y = self.arg_num(&args, 1, 0.0)?;
                return Ok(Value::Number(x.hypot(y)));
            }

            // ── Logarithms ──
            "ln" | "log" | "ลอการิทึม" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 1.0)?.ln()));
            }
            "log2" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 1.0)?.log2()));
            }
            "log10" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 1.0)?.log10()));
            }

            // ── Rounding / truncation ──
            "abs" | "ค่าสัมบูรณ์" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.abs()));
            }
            "floor" | "ปัดลง" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.floor()));
            }
            "ceil" | "ปัดขึ้น" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.ceil()));
            }
            "round" | "ปัดเศษ" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.round()));
            }
            "trunc" | "int" | "ตัดทศนิยม" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.trunc()));
            }
            "fract" => {
                return Ok(Value::Number(self.arg_num(&args, 0, 0.0)?.fract()));
            }

            // ── min / max / clamp ──
            "min" | "ต่ำสุด" => {
                let a = self.arg_num(&args, 0, 0.0)?;
                let b = self.arg_num(&args, 1, 0.0)?;
                return Ok(Value::Number(a.min(b)));
            }
            "max" | "สูงสุด" => {
                let a = self.arg_num(&args, 0, 0.0)?;
                let b = self.arg_num(&args, 1, 0.0)?;
                return Ok(Value::Number(a.max(b)));
            }
            "clamp" | "จำกัด" => {
                let x  = self.arg_num(&args, 0, 0.0)?;
                let lo = self.arg_num(&args, 1, 0.0)?;
                let hi = self.arg_num(&args, 2, 1.0)?;
                return Ok(Value::Number(x.clamp(lo, hi)));
            }

            // ── Constants (also accessible as plain identifiers via lookup) ──
            "pi" | "π" | "พาย" => return Ok(Value::Number(std::f64::consts::PI)),
            "tau" | "τ"        => return Ok(Value::Number(std::f64::consts::TAU)),

            // ══════════════════════════════════════════════════════════════════
            // GRAPHICS BUILTINS
            // Thai names first, then English aliases.
            // ══════════════════════════════════════════════════════════════════

            // ── เปิดหน้าต่าง(width, height, title) — open_window ──
            "เปิดหน้าต่าง" | "open_window" | "gfx_window" => {
                let w = self.arg_num(&args, 0, 800.0)? as usize;
                let h = self.arg_num(&args, 1, 600.0)? as usize;
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let title = args.get(2).map(|v| v.to_string()).unwrap_or_else(|| "Ling".into());
                    let mut gfx = self.gfx.borrow_mut();
                    let mut win = minifb::Window::new(
                        &title, w, h,
                        minifb::WindowOptions {
                            resize: false,
                            scale: minifb::Scale::X1,
                            ..Default::default()
                        },
                    ).map_err(|e| EvalErr::from(format!("cannot open window: {e}")))?;
                    #[allow(deprecated)]
                    win.limit_update_rate(Some(std::time::Duration::from_millis(8)));
                    gfx.buffer = vec![0u32; w * h];
                    gfx.width  = w;
                    gfx.height = h;
                    gfx.window = Some(win);
                    gfx.sync_projection();
                    hide_console_window();
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let mut gfx = self.gfx.borrow_mut();
                    gfx.width  = w;
                    gfx.height = h;
                    gfx.sync_projection();
                    crate::gfx::webgl::resize(w as u32, h as u32);
                }
                return Ok(Value::Unit);
            }

            // ── เติม(r, g, b) — fill / clear screen with colour ──
            "เติม" | "fill" | "gfx_fill" | "clear" => {
                let r = self.arg_num(&args, 0, 0.0)? as u32;
                let g = self.arg_num(&args, 1, 0.0)? as u32;
                let b = self.arg_num(&args, 2, 0.0)? as u32;
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let c = (r << 16) | (g << 8) | b;
                    self.gfx.borrow_mut().buffer.fill(c);
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let mut gfx = self.gfx.borrow_mut();
                    gfx.fill_r = r as f32 / 255.0;
                    gfx.fill_g = g as f32 / 255.0;
                    gfx.fill_b = b as f32 / 255.0;
                }
                return Ok(Value::Unit);
            }

            // ── set_color_hsl(h, s, l) — set drawing colour from HSL ──
            // h: 0–360 degrees, s: 0–100 saturation, l: 0–100 lightness
            "set_color_hsl" | "颜色HSL" | "สีHSLวาด" => {
                let h = self.arg_num(&args, 0, 0.0)?;
                let s = self.arg_num(&args, 1, 70.0)?;
                let l = self.arg_num(&args, 2, 50.0)?;
                let hex = hsl_to_hex(h, s, l);
                let r = u32::from_str_radix(&hex[1..3], 16).unwrap_or(255);
                let g = u32::from_str_radix(&hex[3..5], 16).unwrap_or(255);
                let b = u32::from_str_radix(&hex[5..7], 16).unwrap_or(255);
                self.gfx.borrow_mut().color = (r << 16) | (g << 8) | b;
                return Ok(Value::Unit);
            }

            // ── สีดินสอ(r, g, b) — set drawing colour ──
            "สีดินสอ" | "set_color" | "gfx_color" | "color" => {
                let r = self.arg_num(&args, 0, 255.0)? as u32;
                let g = self.arg_num(&args, 1, 255.0)? as u32;
                let b = self.arg_num(&args, 2, 255.0)? as u32;
                self.gfx.borrow_mut().color = (r << 16) | (g << 8) | b;
                return Ok(Value::Unit);
            }

            // ── วาดสามเหลี่ยม(x1,y1, x2,y2, x3,y3) — draw filled triangle ──
            "วาดสามเหลี่ยม" | "draw_triangle" | "gfx_triangle" | "triangle" => {
                let x0 = self.arg_num(&args, 0, 0.0)? as f32;
                let y0 = self.arg_num(&args, 1, 0.0)? as f32;
                let x1 = self.arg_num(&args, 2, 0.0)? as f32;
                let y1 = self.arg_num(&args, 3, 0.0)? as f32;
                let x2 = self.arg_num(&args, 4, 0.0)? as f32;
                let y2 = self.arg_num(&args, 5, 0.0)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                let color = gfx.color;
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let w = gfx.width;
                    let h = gfx.height;
                    fill_triangle(&mut gfx.buffer, w, h, color, x0, y0, x1, y1, x2, y2);
                }
                #[cfg(target_arch = "wasm32")]
                gfx.depth_queue.push_triangle(0.0, color, x0, y0, x1, y1, x2, y2);
                return Ok(Value::Unit);
            }

            // ── วาดเส้น(x1,y1, x2,y2) — draw line ──
            "วาดเส้น" | "draw_line" | "gfx_line" | "line" => {
                let x0 = self.arg_num(&args, 0, 0.0)? as f32;
                let y0 = self.arg_num(&args, 1, 0.0)? as f32;
                let x1 = self.arg_num(&args, 2, 0.0)? as f32;
                let y1 = self.arg_num(&args, 3, 0.0)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                let color = gfx.color;
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let w = gfx.width;
                    let h = gfx.height;
                    draw_line(&mut gfx.buffer, w, h, color, x0, y0, x1, y1);
                }
                #[cfg(target_arch = "wasm32")]
                gfx.depth_queue.push_line(0.0, color, x0, y0, x1, y1);
                return Ok(Value::Unit);
            }

            // ── วาดจุด(x, y) — plot a single pixel ──
            "วาดจุด" | "draw_pixel" | "gfx_pixel" | "pixel" => {
                let px = self.arg_num(&args, 0, 0.0)? as i32;
                let py = self.arg_num(&args, 1, 0.0)? as i32;
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let mut gfx = self.gfx.borrow_mut();
                    let color = gfx.color;
                    let w = gfx.width;
                    let h = gfx.height;
                    if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                        gfx.buffer[py as usize * w + px as usize] = color;
                    }
                }
                #[cfg(target_arch = "wasm32")]
                {
                    // Render pixel as a 1×1 square via two triangles.
                    let mut gfx = self.gfx.borrow_mut();
                    let color = gfx.color;
                    let x = px as f32; let y = py as f32;
                    gfx.depth_queue.push_triangle(0.0, color, x, y, x+1.0, y, x+1.0, y+1.0);
                    gfx.depth_queue.push_triangle(0.0, color, x, y, x+1.0, y+1.0, x, y+1.0);
                }
                return Ok(Value::Unit);
            }

            // ── แสดงผล() — flush depth queue, then present frame to screen ──
            "แสดงผล" | "present" | "gfx_present" | "show" => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    // Flush depth queue and present — release borrow before reading mouse.
                    {
                        let mut gfx = self.gfx.borrow_mut();
                        if !gfx.depth_queue.is_empty() {
                            let w = gfx.width;
                            let h = gfx.height;
                            let queue = std::mem::take(&mut gfx.depth_queue);
                            queue.flush(&mut gfx.buffer, w, h);
                        }
                        let buf = gfx.buffer.clone();
                        let w   = gfx.width;
                        let h   = gfx.height;
                        if let Some(win) = gfx.window.as_mut() {
                            win.update_with_buffer(&buf, w, h)
                                .map_err(|e| EvalErr::from(format!("present error: {e}")))?;
                        }
                    }
                    // Read mouse AFTER update_with_buffer so events are processed.
                    let mouse_pos = {
                        let gfx = self.gfx.borrow();
                        gfx.window.as_ref()
                            .and_then(|w| w.get_mouse_pos(minifb::MouseMode::Clamp))
                    };
                    let mut gfx = self.gfx.borrow_mut();
                    if gfx.mouse_captured {
                        if let Some((mx, my)) = mouse_pos {
                            if gfx.last_mx.is_nan() {
                                gfx.mouse_dx = 0.0; gfx.mouse_dy = 0.0;
                            } else {
                                gfx.mouse_dx = mx - gfx.last_mx;
                                gfx.mouse_dy = my - gfx.last_my;
                            }
                            gfx.last_mx = mx; gfx.last_my = my;
                        } else {
                            gfx.mouse_dx = 0.0; gfx.mouse_dy = 0.0;
                        }
                        // Clip cursor to window bounds so it can't escape.
                        #[cfg(windows)]
                        unsafe {
                            #[repr(C)]
                            struct RECT { left: i32, top: i32, right: i32, bottom: i32 }
                            extern "system" {
                                fn ClipCursor(lpRect: *const std::ffi::c_void) -> i32;
                                fn GetForegroundWindow() -> isize;
                                fn GetWindowRect(hwnd: isize, lpRect: *mut RECT) -> i32;
                            }
                            let hwnd = GetForegroundWindow();
                            let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                            if GetWindowRect(hwnd, &mut rect) != 0 {
                                ClipCursor(&rect as *const RECT as *const std::ffi::c_void);
                            }
                        }
                    } else if let Some((mx, my)) = mouse_pos {
                        if gfx.last_mx.is_nan() {
                            gfx.mouse_dx = 0.0; gfx.mouse_dy = 0.0;
                        } else {
                            gfx.mouse_dx = mx - gfx.last_mx;
                            gfx.mouse_dy = my - gfx.last_my;
                        }
                        gfx.last_mx = mx; gfx.last_my = my;
                    } else {
                        gfx.mouse_dx = 0.0; gfx.mouse_dy = 0.0;
                    }
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let mut gfx = self.gfx.borrow_mut();
                    let w  = gfx.width;
                    let h  = gfx.height;
                    let fr = gfx.fill_r;
                    let fg = gfx.fill_g;
                    let fb = gfx.fill_b;
                    let queue = std::mem::take(&mut gfx.depth_queue);
                    queue.flush_to_webgl(fr, fg, fb, w, h);
                }
                return Ok(Value::Unit);
            }

            // ── เปิดหน้าต่างเต็มจอ(title) — true native-res fullscreen window ──
            "เปิดหน้าต่างเต็มจอ" | "open_fullscreen" | "fullscreen" => {
                // In WASM the canvas defines the viewport; use its current size
                // as the default so the projection matches what's actually visible.
                #[cfg(target_arch = "wasm32")]
                let (default_w, default_h) = {
                    let (cw, ch) = crate::gfx::webgl::canvas_size();
                    (cw as f64, ch as f64)
                };
                // On native: query the actual primary monitor resolution.
                #[cfg(all(not(target_arch = "wasm32"), windows))]
                let (default_w, default_h) = unsafe {
                    extern "system" { fn GetSystemMetrics(nIndex: i32) -> i32; }
                    (GetSystemMetrics(0) as f64, GetSystemMetrics(1) as f64)
                };
                #[cfg(all(not(target_arch = "wasm32"), not(windows)))]
                let (default_w, default_h) = native_screen_size();

                let w = args.get(1).map(|v| self.to_number(v).unwrap_or(default_w) as usize).unwrap_or(default_w as usize);
                let h = args.get(2).map(|v| self.to_number(v).unwrap_or(default_h) as usize).unwrap_or(default_h as usize);
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let title = args.get(0).map(|v| v.to_string()).unwrap_or_else(|| "Ling".into());
                    let mut gfx = self.gfx.borrow_mut();
                    let mut win = minifb::Window::new(
                        &title, w, h,
                        minifb::WindowOptions {
                            borderless: true,
                            title:      false,
                            resize:     false,
                            scale:      minifb::Scale::X1,
                            ..Default::default()
                        },
                    ).map_err(|e| EvalErr::from(format!("cannot open fullscreen: {e}")))?;
                    #[allow(deprecated)]
                    win.limit_update_rate(Some(std::time::Duration::from_millis(8)));
                    gfx.buffer = vec![0u32; w * h];
                    gfx.width  = w;
                    gfx.height = h;
                    gfx.window = Some(win);
                    gfx.sync_projection();
                    // Snap to top-left, cover full screen, bring above taskbar.
                    #[cfg(windows)]
                    reposition_fullscreen(w as i32, h as i32);
                    hide_console_window();
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let mut gfx = self.gfx.borrow_mut();
                    gfx.width  = w;
                    gfx.height = h;
                    gfx.sync_projection();
                    crate::gfx::webgl::resize(w as u32, h as u32);
                }
                return Ok(Value::Unit);
            }

            // ── ความกว้าง() / ความสูง() — current framebuffer size ──
            "get_width" | "ความกว้าง" => {
                return Ok(Value::Number(self.gfx.borrow().width as f64));
            }
            "get_height" | "ความสูง" => {
                return Ok(Value::Number(self.gfx.borrow().height as f64));
            }

            // ── หน้าต่างเปิดอยู่() → bool — is the window still open? ──
            "หน้าต่างเปิดอยู่" | "window_is_open" | "gfx_is_open" | "is_open" => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let gfx = self.gfx.borrow();
                    let open = gfx.window.as_ref()
                        .map(|w| w.is_open() && !w.is_key_down(minifb::Key::Escape))
                        .unwrap_or(false);
                    return Ok(Value::Bool(open));
                }
                #[cfg(target_arch = "wasm32")]
                return Ok(Value::Bool(true));
            }

            // ── key_down(name) → bool — is a key held? ──
            "key_down" | "กดค้าง" => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let name = self.arg_str(&args, 0, "");
                    let gfx  = self.gfx.borrow();
                    let down = gfx.window.as_ref()
                        .and_then(|w| str_to_minifb_key(&name).map(|k| w.is_key_down(k)))
                        .unwrap_or(false);
                    return Ok(Value::Bool(down));
                }
                #[cfg(target_arch = "wasm32")]
                return Ok(Value::Bool(false));
            }

            // ── key_pressed(name) → bool — was a key pressed this frame? ──
            "key_pressed" | "กดปุ่ม" => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let name = self.arg_str(&args, 0, "");
                    let gfx  = self.gfx.borrow();
                    let pressed = gfx.window.as_ref()
                        .and_then(|w| str_to_minifb_key(&name)
                            .map(|k| w.is_key_pressed(k, minifb::KeyRepeat::No)))
                        .unwrap_or(false);
                    return Ok(Value::Bool(pressed));
                }
                #[cfg(target_arch = "wasm32")]
                return Ok(Value::Bool(false));
            }

            // ── mouse_dx() / mouse_dy() → f64 — delta since last frame ──
            "mouse_dx" | "เมาส์X" => {
                #[cfg(not(target_arch = "wasm32"))]
                return Ok(Value::Number(self.gfx.borrow().mouse_dx as f64));
                #[cfg(target_arch = "wasm32")]
                return Ok(Value::Number(0.0));
            }
            "mouse_dy" | "เมาส์Y" => {
                #[cfg(not(target_arch = "wasm32"))]
                return Ok(Value::Number(self.gfx.borrow().mouse_dy as f64));
                #[cfg(target_arch = "wasm32")]
                return Ok(Value::Number(0.0));
            }

            // ── set_camera_pos(x, y, z) — move camera to world position ──
            "set_camera_pos" | "ตั้งตำแหน่งกล้อง" => {
                let x = self.arg_num(&args, 0, 0.0)? as f32;
                let y = self.arg_num(&args, 1, 0.0)? as f32;
                let z = self.arg_num(&args, 2, 0.0)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                gfx.camera.tx = x; gfx.camera.ty = y; gfx.camera.tz = z;
                return Ok(Value::Unit);
            }

            // ── move_camera(dx, dy, dz) — translate camera by delta ──
            "move_camera" => {
                let dx = self.arg_num(&args, 0, 0.0)? as f32;
                let dy = self.arg_num(&args, 1, 0.0)? as f32;
                let dz = self.arg_num(&args, 2, 0.0)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                gfx.camera.tx += dx; gfx.camera.ty += dy; gfx.camera.tz += dz;
                return Ok(Value::Unit);
            }

            // ── set_zdist(d) — set perspective z-offset (field-of-view taper) ──
            "set_zdist" | "ตั้งระยะห่าง" => {
                let d = self.arg_num(&args, 0, 5.0)? as f32;
                self.gfx.borrow_mut().camera.zdist = d;
                return Ok(Value::Unit);
            }

            // ── capture_mouse() — hide cursor and warp to centre each frame ──
            "capture_mouse" | "จับเมาส์" => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let mut gfx = self.gfx.borrow_mut();
                    gfx.mouse_captured = true;
                    gfx.last_mx = f32::NAN;
                    if let Some(win) = gfx.window.as_mut() {
                        win.set_cursor_visibility(false);
                    }
                }
                return Ok(Value::Unit);
            }

            // ── release_mouse() — restore cursor and remove clip region ──
            "release_mouse" => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let mut gfx = self.gfx.borrow_mut();
                    gfx.mouse_captured = false;
                    gfx.last_mx = f32::NAN;
                    if let Some(win) = gfx.window.as_mut() {
                        win.set_cursor_visibility(true);
                    }
                    #[cfg(windows)]
                    unsafe {
                        // Null releases the clip; reuse the RECT-typed declaration above.
                        extern "system" { fn ClipCursor(lpRect: *const std::ffi::c_void) -> i32; }
                        ClipCursor(std::ptr::null());
                    }
                }
                return Ok(Value::Unit);
            }

            // ══════════════════════════════════════════════════════════════════
            // 3-D / 4-D DRAWING — camera, lights, depth-sorted geometry
            // ══════════════════════════════════════════════════════════════════

            // ── set_camera(cry, sry, crx, srx) — store precomputed camera trig ──
            // Call once per frame after computing cos/sin of your rotation angles.
            "set_camera" | "ตั้งกล้อง" => {
                let cry = self.arg_num(&args, 0, 1.0)? as f32;
                let sry = self.arg_num(&args, 1, 0.0)? as f32;
                let crx = self.arg_num(&args, 2, 1.0)? as f32;
                let srx = self.arg_num(&args, 3, 0.0)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                gfx.camera.cry = cry; gfx.camera.sry = sry;
                gfx.camera.crx = crx; gfx.camera.srx = srx;
                return Ok(Value::Unit);
            }

            // ── set_projection(cx, cy, focal, zdist) — override projection params ──
            // Automatically set when the window opens; override only if needed.
            "set_projection" | "ตั้งโปรเจกชัน" => {
                let cx    = self.arg_num(&args, 0, 960.0)? as f32;
                let cy    = self.arg_num(&args, 1, 540.0)? as f32;
                let focal = self.arg_num(&args, 2, 1080.0)? as f32;
                let zdist = self.arg_num(&args, 3, 5.0)? as f32;
                let mut gfx = self.gfx.borrow_mut();
                gfx.camera.cx    = cx;
                gfx.camera.cy    = cy;
                gfx.camera.focal = focal;
                gfx.camera.zdist = zdist;
                return Ok(Value::Unit);
            }

            // ── add_light(x, y, z, r, g, b, intensity, radius) ──
            // Adds a point light in world space.  r/g/b in [0..1].
            // radius == 0 → no distance falloff.
            "add_light" | "เพิ่มแสง" => {
                let x   = self.arg_num(&args, 0, 0.0)? as f32;
                let y   = self.arg_num(&args, 1, -3.0)? as f32;
                let z   = self.arg_num(&args, 2, 3.0)? as f32;
                let r   = self.arg_num(&args, 3, 1.0)? as f32;
                let g   = self.arg_num(&args, 4, 1.0)? as f32;
                let b   = self.arg_num(&args, 5, 1.0)? as f32;
                let intensity = self.arg_num(&args, 6, 1.0)? as f32;
                let radius    = self.arg_num(&args, 7, 0.0)? as f32;
                self.gfx.borrow_mut().lights.push(Light { x, y, z, r, g, b, intensity, radius });
                return Ok(Value::Unit);
            }

            // ── clear_lights() — remove all lights ──
            "clear_lights" | "ล้างแสง" => {
                self.gfx.borrow_mut().lights.clear();
                return Ok(Value::Unit);
            }

            // ── set_ambient(v) — ambient light level [0..1] ──
            "set_ambient" | "ตั้งแสงรอบข้าง" => {
                let v = self.arg_num(&args, 0, 0.15)? as f32;
                self.gfx.borrow_mut().ambient = v;
                return Ok(Value::Unit);
            }

            // ── วาดสามเหลี่ยม3มิติ(ax,ay,az, bx,by,bz, cx,cy,cz) ──
            // Computes lighting from world-space normal + active lights (cel shading),
            // projects via the stored camera, and pushes to the depth queue.
            "วาดสามเหลี่ยม3มิติ" | "draw_triangle_3d" | "triangle3d" => {
                let ax = self.arg_num(&args, 0, 0.0)? as f32;
                let ay = self.arg_num(&args, 1, 0.0)? as f32;
                let az = self.arg_num(&args, 2, 0.0)? as f32;
                let bx = self.arg_num(&args, 3, 0.0)? as f32;
                let by = self.arg_num(&args, 4, 0.0)? as f32;
                let bz = self.arg_num(&args, 5, 0.0)? as f32;
                let cx = self.arg_num(&args, 6, 0.0)? as f32;
                let cy = self.arg_num(&args, 7, 0.0)? as f32;
                let cz = self.arg_num(&args, 8, 0.0)? as f32;

                let mut gfx = self.gfx.borrow_mut();

                // World-space face normal  N = (B−A) × (C−A)
                let ux = bx-ax; let uy = by-ay; let uz = bz-az;
                let vx = cx-ax; let vy = cy-ay; let vz = cz-az;
                let normal = [
                    uy*vz - uz*vy,
                    uz*vx - ux*vz,
                    ux*vy - uy*vx,
                ];
                // World-space centroid
                let centroid = [
                    (ax+bx+cx)/3.0,
                    (ay+by+cy)/3.0,
                    (az+bz+cz)/3.0,
                ];

                // Cel-shaded colour
                let lit_color = crate::gfx::light::compute_lit_color(
                    gfx.color, normal, centroid, &gfx.lights, gfx.ambient,
                );

                // Near-plane cull — skip any triangle that has a vertex
                // behind or at the camera near plane (avoids projected-to-infinity blowup).
                let near = -gfx.camera.zdist + 0.05;
                let da_raw = gfx.camera.depth(ax, ay, az);
                let db_raw = gfx.camera.depth(bx, by, bz);
                let dc_raw = gfx.camera.depth(cx, cy, cz);
                if da_raw <= near || db_raw <= near || dc_raw <= near {
                    return Ok(Value::Unit);
                }

                // Project to screen
                let (sax, say, da) = gfx.camera.project(ax, ay, az);
                let (sbx, sby, db) = gfx.camera.project(bx, by, bz);
                let (scx, scy, dc) = gfx.camera.project(cx, cy, cz);

                // Average camera depth (used for painter's sort)
                let depth = (da + db + dc) / 3.0;

                gfx.depth_queue.push_triangle(
                    depth, lit_color,
                    sax, say, sbx, sby, scx, scy,
                );
                return Ok(Value::Unit);
            }

            // ── วาดเส้น3มิติ(ax,ay,az, bx,by,bz) ──
            // Projects two world-space points via the stored camera and pushes
            // a line to the depth queue.
            "วาดเส้น3มิติ" | "draw_line_3d" | "line3d" => {
                let ax = self.arg_num(&args, 0, 0.0)? as f32;
                let ay = self.arg_num(&args, 1, 0.0)? as f32;
                let az = self.arg_num(&args, 2, 0.0)? as f32;
                let bx = self.arg_num(&args, 3, 0.0)? as f32;
                let by = self.arg_num(&args, 4, 0.0)? as f32;
                let bz = self.arg_num(&args, 5, 0.0)? as f32;

                let mut gfx = self.gfx.borrow_mut();
                let color = gfx.color;
                // Near-plane clip in 3-D before perspective divide
                let near = -gfx.camera.zdist + 0.05;
                let mut lax = ax; let mut lay = ay; let mut laz = az;
                let mut lbx = bx; let mut lby = by; let mut lbz = bz;
                let da_raw = gfx.camera.depth(lax, lay, laz);
                let db_raw = gfx.camera.depth(lbx, lby, lbz);
                if da_raw <= near && db_raw <= near {
                    return Ok(Value::Unit);
                }
                if da_raw <= near {
                    let t = (near - da_raw) / (db_raw - da_raw);
                    lax += t * (lbx - lax);
                    lay += t * (lby - lay);
                    laz += t * (lbz - laz);
                } else if db_raw <= near {
                    let t = (near - da_raw) / (db_raw - da_raw);
                    lbx = lax + t * (lbx - lax);
                    lby = lay + t * (lby - lay);
                    lbz = laz + t * (lbz - laz);
                }
                let (sax, say, da) = gfx.camera.project(lax, lay, laz);
                let (sbx, sby, db) = gfx.camera.project(lbx, lby, lbz);
                let depth = (da + db) / 2.0;
                gfx.depth_queue.push_line(depth, color, sax, say, sbx, sby);
                return Ok(Value::Unit);
            }

            // ══════════════════════════════════════════════════════════════════
            // VECTOR TEXTURE BUILTINS  (src/gfx/vtex.rs)
            // All patterns are depth-biased so they appear on top of surfaces.
            // Plane defined by: centre (cx,cy,cz) + U tangent + V tangent.
            // Last two args always: fr (frame f32), hue (phase offset f32).
            // ══════════════════════════════════════════════════════════════════

            // vtex_grid(cx,cy,cz, ux,uy,uz, vx,vy,vz, cols,rows, cw,ch, fr,hue)
            "vtex_grid" | "ลายตาราง" | "纹格" | "格子模様" | "격자무늬" => {
                let cx=self.arg_num(&args,0,0.)?as f32; let cy=self.arg_num(&args,1,0.)?as f32; let cz=self.arg_num(&args,2,0.)?as f32;
                let ux=self.arg_num(&args,3,1.)?as f32; let uy=self.arg_num(&args,4,0.)?as f32; let uz=self.arg_num(&args,5,0.)?as f32;
                let vx=self.arg_num(&args,6,0.)?as f32; let vy=self.arg_num(&args,7,0.)?as f32; let vz=self.arg_num(&args,8,1.)?as f32;
                let cols=self.arg_num(&args,9,10.)?as usize; let rows=self.arg_num(&args,10,10.)?as usize;
                let cw=self.arg_num(&args,11,1.)?as f32;  let ch=self.arg_num(&args,12,1.)?as f32;
                let fr=self.arg_num(&args,13,0.)?as f32;  let hue=self.arg_num(&args,14,0.)?as f32;
                let mut gfx = self.gfx.borrow_mut();
                let cam = gfx.camera.clone();
                crate::gfx::vtex::draw_grid(&mut gfx.depth_queue,&cam, cx,cy,cz, ux,uy,uz, vx,vy,vz, cols,rows,cw,ch, fr,hue);
                return Ok(Value::Unit);
            }

            // vtex_rings(cx,cy,cz, ux,uy,uz, vx,vy,vz, n_rings,n_sides, max_r,twist, fr,hue)
            "vtex_rings" | "ลายวงซ้อน" | "纹环" | "同心円" | "동심원" => {
                let cx=self.arg_num(&args,0,0.)?as f32; let cy=self.arg_num(&args,1,0.)?as f32; let cz=self.arg_num(&args,2,0.)?as f32;
                let ux=self.arg_num(&args,3,1.)?as f32; let uy=self.arg_num(&args,4,0.)?as f32; let uz=self.arg_num(&args,5,0.)?as f32;
                let vx=self.arg_num(&args,6,0.)?as f32; let vy=self.arg_num(&args,7,0.)?as f32; let vz=self.arg_num(&args,8,1.)?as f32;
                let nr=self.arg_num(&args,9,6.)?as usize; let ns=self.arg_num(&args,10,6.)?as usize;
                let mr=self.arg_num(&args,11,3.)?as f32;  let tw=self.arg_num(&args,12,0.)?as f32;
                let fr=self.arg_num(&args,13,0.)?as f32;  let hue=self.arg_num(&args,14,0.)?as f32;
                let mut gfx = self.gfx.borrow_mut();
                let cam = gfx.camera.clone();
                crate::gfx::vtex::draw_rings(&mut gfx.depth_queue,&cam, cx,cy,cz, ux,uy,uz, vx,vy,vz, nr,ns,mr,tw, fr,hue);
                return Ok(Value::Unit);
            }

            // vtex_star(cx,cy,cz, ux,uy,uz, vx,vy,vz, n_pts,r_out,r_in, rot_speed, fr,hue)
            "vtex_star" | "ลายดาว" | "纹星" | "星模様" | "별무늬" => {
                let cx=self.arg_num(&args,0,0.)?as f32; let cy=self.arg_num(&args,1,0.)?as f32; let cz=self.arg_num(&args,2,0.)?as f32;
                let ux=self.arg_num(&args,3,1.)?as f32; let uy=self.arg_num(&args,4,0.)?as f32; let uz=self.arg_num(&args,5,0.)?as f32;
                let vx=self.arg_num(&args,6,0.)?as f32; let vy=self.arg_num(&args,7,0.)?as f32; let vz=self.arg_num(&args,8,1.)?as f32;
                let np=self.arg_num(&args,9,6.)?as usize;
                let ro=self.arg_num(&args,10,2.)?as f32; let ri=self.arg_num(&args,11,1.)?as f32;
                let rs=self.arg_num(&args,12,0.01)?as f32;
                let fr=self.arg_num(&args,13,0.)?as f32; let hue=self.arg_num(&args,14,0.)?as f32;
                let mut gfx = self.gfx.borrow_mut();
                let cam = gfx.camera.clone();
                crate::gfx::vtex::draw_star(&mut gfx.depth_queue,&cam, cx,cy,cz, ux,uy,uz, vx,vy,vz, np,ro,ri,rs, fr,hue);
                return Ok(Value::Unit);
            }

            // vtex_spiral(cx,cy,cz, ux,uy,uz, vx,vy,vz, n_turns,max_r,steps, fr,hue)
            "vtex_spiral" | "ลายเกลียว" | "纹螺" | "螺旋" | "나선" => {
                let cx=self.arg_num(&args,0,0.)?as f32; let cy=self.arg_num(&args,1,0.)?as f32; let cz=self.arg_num(&args,2,0.)?as f32;
                let ux=self.arg_num(&args,3,1.)?as f32; let uy=self.arg_num(&args,4,0.)?as f32; let uz=self.arg_num(&args,5,0.)?as f32;
                let vx=self.arg_num(&args,6,0.)?as f32; let vy=self.arg_num(&args,7,0.)?as f32; let vz=self.arg_num(&args,8,1.)?as f32;
                let nt=self.arg_num(&args,9,3.)?as f32; let mr=self.arg_num(&args,10,3.)?as f32;
                let st=self.arg_num(&args,11,120.)?as usize;
                let fr=self.arg_num(&args,12,0.)?as f32; let hue=self.arg_num(&args,13,0.)?as f32;
                let mut gfx = self.gfx.borrow_mut();
                let cam = gfx.camera.clone();
                crate::gfx::vtex::draw_spiral(&mut gfx.depth_queue,&cam, cx,cy,cz, ux,uy,uz, vx,vy,vz, nt,mr,st, fr,hue);
                return Ok(Value::Unit);
            }

            // vtex_flower(cx,cy,cz, ux,uy,uz, vx,vy,vz, radius,n_sides, fr,hue)
            "vtex_flower" | "ลายดอก" | "纹花" | "花模様" | "꽃무늬" => {
                let cx=self.arg_num(&args,0,0.)?as f32; let cy=self.arg_num(&args,1,0.)?as f32; let cz=self.arg_num(&args,2,0.)?as f32;
                let ux=self.arg_num(&args,3,1.)?as f32; let uy=self.arg_num(&args,4,0.)?as f32; let uz=self.arg_num(&args,5,0.)?as f32;
                let vx=self.arg_num(&args,6,0.)?as f32; let vy=self.arg_num(&args,7,0.)?as f32; let vz=self.arg_num(&args,8,1.)?as f32;
                let r=self.arg_num(&args,9,1.)?as f32; let ns=self.arg_num(&args,10,24.)?as usize;
                let fr=self.arg_num(&args,11,0.)?as f32; let hue=self.arg_num(&args,12,0.)?as f32;
                let mut gfx = self.gfx.borrow_mut();
                let cam = gfx.camera.clone();
                crate::gfx::vtex::draw_flower(&mut gfx.depth_queue,&cam, cx,cy,cz, ux,uy,uz, vx,vy,vz, r,ns, fr,hue);
                return Ok(Value::Unit);
            }

            // vtex_letter_rain(cx,cy,cz, ux,uy,uz, vx,vy,vz, n_cols,n_vis, col_w,row_h, speed, fr,hue)
            "vtex_letter_rain" | "ลายอักษรไหล" | "纹字雨" | "文字雨" | "글자비" => {
                let cx=self.arg_num(&args,0,0.)?as f32; let cy=self.arg_num(&args,1,0.)?as f32; let cz=self.arg_num(&args,2,0.)?as f32;
                let ux=self.arg_num(&args,3,1.)?as f32; let uy=self.arg_num(&args,4,0.)?as f32; let uz=self.arg_num(&args,5,0.)?as f32;
                let vx=self.arg_num(&args,6,0.)?as f32; let vy=self.arg_num(&args,7,0.)?as f32; let vz=self.arg_num(&args,8,1.)?as f32;
                let nc=self.arg_num(&args,9,16.)?as usize; let nv=self.arg_num(&args,10,14.)?as usize;
                let cw=self.arg_num(&args,11,0.65)?as f32; let rh=self.arg_num(&args,12,0.60)?as f32;
                let sp=self.arg_num(&args,13,0.025)?as f32;
                let fr=self.arg_num(&args,14,0.)?as f32; let hue=self.arg_num(&args,15,0.)?as f32;
                let mut gfx = self.gfx.borrow_mut();
                let cam = gfx.camera.clone();
                crate::gfx::vtex::draw_letter_rain(&mut gfx.depth_queue,&cam, cx,cy,cz, ux,uy,uz, vx,vy,vz, nc,nv,cw,rh,sp, fr,hue);
                return Ok(Value::Unit);
            }

            // vtex_hyperbolic_uv(cx,cy,cz, ux,uy,uz, vx,vy,vz, max_r,n_circles,n_rays, fr,hue)
            "vtex_hyperbolic_uv" | "ลายไฮเพอร์โบลิก" | "纹曲面" | "双曲線" | "쌍곡선" => {
                let cx=self.arg_num(&args,0,0.)?as f32; let cy=self.arg_num(&args,1,0.)?as f32; let cz=self.arg_num(&args,2,0.)?as f32;
                let ux=self.arg_num(&args,3,1.)?as f32; let uy=self.arg_num(&args,4,0.)?as f32; let uz=self.arg_num(&args,5,0.)?as f32;
                let vx=self.arg_num(&args,6,0.)?as f32; let vy=self.arg_num(&args,7,0.)?as f32; let vz=self.arg_num(&args,8,1.)?as f32;
                let mr=self.arg_num(&args,9,5.)?as f32;
                let nc=self.arg_num(&args,10,12.)?as usize; let nr=self.arg_num(&args,11,18.)?as usize;
                let fr=self.arg_num(&args,12,0.)?as f32; let hue=self.arg_num(&args,13,0.)?as f32;
                let mut gfx = self.gfx.borrow_mut();
                let cam = gfx.camera.clone();
                crate::gfx::vtex::draw_hyperbolic_uv(&mut gfx.depth_queue,&cam, cx,cy,cz, ux,uy,uz, vx,vy,vz, mr,nc,nr, fr,hue);
                return Ok(Value::Unit);
            }

            // vtex_halftone(cx,cy,cz, ux,uy,uz, vx,vy,vz, cols,rows, cell_w,cell_h, density, fr,hue)
            "vtex_halftone" | "ลายจุด" | "纹半调" | "網点模様" | "망점" => {
                let cx=self.arg_num(&args,0,0.)?as f32; let cy=self.arg_num(&args,1,0.)?as f32; let cz=self.arg_num(&args,2,0.)?as f32;
                let ux=self.arg_num(&args,3,1.)?as f32; let uy=self.arg_num(&args,4,0.)?as f32; let uz=self.arg_num(&args,5,0.)?as f32;
                let vx=self.arg_num(&args,6,0.)?as f32; let vy=self.arg_num(&args,7,0.)?as f32; let vz=self.arg_num(&args,8,1.)?as f32;
                let cols=self.arg_num(&args,9,16.)?as usize; let rows=self.arg_num(&args,10,12.)?as usize;
                let cw=self.arg_num(&args,11,0.5)?as f32; let ch=self.arg_num(&args,12,0.5)?as f32;
                let dens=self.arg_num(&args,13,0.4)?as f32;
                let fr=self.arg_num(&args,14,0.)?as f32; let hue=self.arg_num(&args,15,0.)?as f32;
                let mut gfx = self.gfx.borrow_mut();
                let cam = gfx.camera.clone();
                crate::gfx::vtex::draw_halftone(&mut gfx.depth_queue,&cam, cx,cy,cz, ux,uy,uz, vx,vy,vz, cols,rows,cw,ch,dens, fr,hue);
                return Ok(Value::Unit);
            }

            // vtex_tessellated(cx,cy,cz, ux,uy,uz, vx,vy,vz, cols,rows, cell, amplitude,freq, fr,hue)
            "vtex_tessellated" | "ลายตาข่าย" | "纹镶嵌" | "網目模様" | "격자망" => {
                let cx=self.arg_num(&args,0,0.)?as f32; let cy=self.arg_num(&args,1,0.)?as f32; let cz=self.arg_num(&args,2,0.)?as f32;
                let ux=self.arg_num(&args,3,1.)?as f32; let uy=self.arg_num(&args,4,0.)?as f32; let uz=self.arg_num(&args,5,0.)?as f32;
                let vx=self.arg_num(&args,6,0.)?as f32; let vy=self.arg_num(&args,7,0.)?as f32; let vz=self.arg_num(&args,8,1.)?as f32;
                let cols=self.arg_num(&args,9,14.)?as usize; let rows=self.arg_num(&args,10,10.)?as usize;
                let cell=self.arg_num(&args,11,0.6)?as f32;
                let amp=self.arg_num(&args,12,0.25)?as f32; let freq=self.arg_num(&args,13,4.)?as f32;
                let fr=self.arg_num(&args,14,0.)?as f32; let hue=self.arg_num(&args,15,0.)?as f32;
                let mut gfx = self.gfx.borrow_mut();
                let cam = gfx.camera.clone();
                crate::gfx::vtex::draw_tessellated(&mut gfx.depth_queue,&cam, cx,cy,cz, ux,uy,uz, vx,vy,vz, cols,rows,cell,amp,freq, fr,hue);
                return Ok(Value::Unit);
            }

            // vtex_lotus(cx,cy,cz, ux,uy,uz, vx,vy,vz, r_inner,r_outer,n_petals, fr,hue)
            "vtex_lotus" | "ลายดอกบัว" | "纹莲" | "蓮模様" | "연꽃무늬" => {
                let cx=self.arg_num(&args,0,0.)?as f32; let cy=self.arg_num(&args,1,0.)?as f32; let cz=self.arg_num(&args,2,0.)?as f32;
                let ux=self.arg_num(&args,3,1.)?as f32; let uy=self.arg_num(&args,4,0.)?as f32; let uz=self.arg_num(&args,5,0.)?as f32;
                let vx=self.arg_num(&args,6,0.)?as f32; let vy=self.arg_num(&args,7,0.)?as f32; let vz=self.arg_num(&args,8,1.)?as f32;
                let ri=self.arg_num(&args,9,1.)?as f32; let ro=self.arg_num(&args,10,2.)?as f32;
                let np=self.arg_num(&args,11,12.)?as usize;
                let fr=self.arg_num(&args,12,0.)?as f32; let hue=self.arg_num(&args,13,0.)?as f32;
                let mut gfx = self.gfx.borrow_mut();
                let cam = gfx.camera.clone();
                crate::gfx::vtex::draw_lotus(&mut gfx.depth_queue,&cam, cx,cy,cz, ux,uy,uz, vx,vy,vz, ri,ro,np, fr,hue);
                return Ok(Value::Unit);
            }

            // vtex_chakra(cx,cy,cz, ux,uy,uz, vx,vy,vz, r,n_spokes, fr,hue)
            "vtex_chakra" | "ลายจักร" | "纹轮" | "輪模様" | "바퀴무늬" => {
                let cx=self.arg_num(&args,0,0.)?as f32; let cy=self.arg_num(&args,1,0.)?as f32; let cz=self.arg_num(&args,2,0.)?as f32;
                let ux=self.arg_num(&args,3,1.)?as f32; let uy=self.arg_num(&args,4,0.)?as f32; let uz=self.arg_num(&args,5,0.)?as f32;
                let vx=self.arg_num(&args,6,0.)?as f32; let vy=self.arg_num(&args,7,0.)?as f32; let vz=self.arg_num(&args,8,1.)?as f32;
                let r=self.arg_num(&args,9,2.)?as f32; let ns=self.arg_num(&args,10,8.)?as usize;
                let fr=self.arg_num(&args,11,0.)?as f32; let hue=self.arg_num(&args,12,0.)?as f32;
                let mut gfx = self.gfx.borrow_mut();
                let cam = gfx.camera.clone();
                crate::gfx::vtex::draw_chakra(&mut gfx.depth_queue,&cam, cx,cy,cz, ux,uy,uz, vx,vy,vz, r,ns, fr,hue);
                return Ok(Value::Unit);
            }

            // vtex_yantra(cx,cy,cz, ux,uy,uz, vx,vy,vz, n_layers,max_r, fr,hue)
            "vtex_yantra" | "ลายยันต์" | "纹咒" | "護符模様" | "부적무늬" => {
                let cx=self.arg_num(&args,0,0.)?as f32; let cy=self.arg_num(&args,1,0.)?as f32; let cz=self.arg_num(&args,2,0.)?as f32;
                let ux=self.arg_num(&args,3,1.)?as f32; let uy=self.arg_num(&args,4,0.)?as f32; let uz=self.arg_num(&args,5,0.)?as f32;
                let vx=self.arg_num(&args,6,0.)?as f32; let vy=self.arg_num(&args,7,0.)?as f32; let vz=self.arg_num(&args,8,1.)?as f32;
                let nl=self.arg_num(&args,9,4.)?as usize; let mr=self.arg_num(&args,10,3.)?as f32;
                let fr=self.arg_num(&args,11,0.)?as f32; let hue=self.arg_num(&args,12,0.)?as f32;
                let mut gfx = self.gfx.borrow_mut();
                let cam = gfx.camera.clone();
                crate::gfx::vtex::draw_yantra(&mut gfx.depth_queue,&cam, cx,cy,cz, ux,uy,uz, vx,vy,vz, nl,mr, fr,hue);
                return Ok(Value::Unit);
            }

            // vtex_spiked_cog(cx,cy,cz, ux,uy,uz, vx,vy,vz, n_teeth,r_body,r_spike,r_hub,n_spokes, fr,hue)
            "vtex_spiked_cog" | "ฟันเฟืองหนาม" | "纹棘轮" | "歯車模様" | "톱니바퀴" => {
                let cx=self.arg_num(&args,0,0.)?as f32; let cy=self.arg_num(&args,1,0.)?as f32; let cz=self.arg_num(&args,2,0.)?as f32;
                let ux=self.arg_num(&args,3,1.)?as f32; let uy=self.arg_num(&args,4,0.)?as f32; let uz=self.arg_num(&args,5,0.)?as f32;
                let vx=self.arg_num(&args,6,0.)?as f32; let vy=self.arg_num(&args,7,0.)?as f32; let vz=self.arg_num(&args,8,1.)?as f32;
                let nt=self.arg_num(&args,9,12.)?as usize; let rb=self.arg_num(&args,10,1.)?as f32;
                let rs=self.arg_num(&args,11,1.3)?as f32; let rh=self.arg_num(&args,12,0.2)?as f32;
                let ns=self.arg_num(&args,13,6.)?as usize;
                let fr=self.arg_num(&args,14,0.)?as f32; let hue=self.arg_num(&args,15,0.)?as f32;
                let mut gfx = self.gfx.borrow_mut();
                let cam = gfx.camera.clone();
                crate::gfx::vtex::draw_spiked_cog(&mut gfx.depth_queue,&cam, cx,cy,cz, ux,uy,uz, vx,vy,vz, nt,rb,rs,rh,ns, fr,hue);
                return Ok(Value::Unit);
            }

            // vtex_torii(cx,cy,cz, ux,uy,uz, vx,vy,vz, width,height, fr,hue)
            "vtex_torii" | "ประตูโทริอิ" | "纹鸟居" | "鳥居" | "도리이" => {
                let cx=self.arg_num(&args,0,0.)?as f32; let cy=self.arg_num(&args,1,0.)?as f32; let cz=self.arg_num(&args,2,0.)?as f32;
                let ux=self.arg_num(&args,3,1.)?as f32; let uy=self.arg_num(&args,4,0.)?as f32; let uz=self.arg_num(&args,5,0.)?as f32;
                let vx=self.arg_num(&args,6,0.)?as f32; let vy=self.arg_num(&args,7,0.)?as f32; let vz=self.arg_num(&args,8,1.)?as f32;
                let w=self.arg_num(&args,9,4.)?as f32; let h=self.arg_num(&args,10,5.)?as f32;
                let fr=self.arg_num(&args,11,0.)?as f32; let hue=self.arg_num(&args,12,0.)?as f32;
                let mut gfx = self.gfx.borrow_mut();
                let cam = gfx.camera.clone();
                crate::gfx::vtex::draw_torii(&mut gfx.depth_queue,&cam, cx,cy,cz, ux,uy,uz, vx,vy,vz, w,h, fr,hue);
                return Ok(Value::Unit);
            }

            // vtex_pagoda(cx,cy,cz, ux,uy,uz, vx,vy,vz, n_tiers,base_w,tier_h,taper,eave_out, fr,hue)
            "vtex_pagoda" | "เจดีย์" | "纹塔" | "塔" | "탑" => {
                let cx=self.arg_num(&args,0,0.)?as f32; let cy=self.arg_num(&args,1,0.)?as f32; let cz=self.arg_num(&args,2,0.)?as f32;
                let ux=self.arg_num(&args,3,1.)?as f32; let uy=self.arg_num(&args,4,0.)?as f32; let uz=self.arg_num(&args,5,0.)?as f32;
                let vx=self.arg_num(&args,6,0.)?as f32; let vy=self.arg_num(&args,7,0.)?as f32; let vz=self.arg_num(&args,8,1.)?as f32;
                let nt=self.arg_num(&args,9,5.)?as usize; let bw=self.arg_num(&args,10,2.)?as f32;
                let th=self.arg_num(&args,11,1.)?as f32; let tp=self.arg_num(&args,12,0.72)?as f32;
                let eo=self.arg_num(&args,13,0.28)?as f32;
                let fr=self.arg_num(&args,14,0.)?as f32; let hue=self.arg_num(&args,15,0.)?as f32;
                let mut gfx = self.gfx.borrow_mut();
                let cam = gfx.camera.clone();
                crate::gfx::vtex::draw_pagoda(&mut gfx.depth_queue,&cam, cx,cy,cz, ux,uy,uz, vx,vy,vz, nt,bw,th,tp,eo, fr,hue);
                return Ok(Value::Unit);
            }

            // ══════════════════════════════════════════════════════════════════
            // AUDIO BUILTINS
            // ══════════════════════════════════════════════════════════════════

            // audio_tone(idx, x, y, z, w, freq, amp, lfo_rate, lfo_depth)
            #[cfg(not(target_arch = "wasm32"))]
            "audio_tone" | "เสียงโทน" => {
                let idx  = self.arg_num(&args, 0, 0.0)? as usize;
                let x    = self.arg_num(&args, 1, 0.0)? as f32;
                let y    = self.arg_num(&args, 2, 0.0)? as f32;
                let z    = self.arg_num(&args, 3, 0.0)? as f32;
                let w    = self.arg_num(&args, 4, 1.0)? as f32;
                let freq = self.arg_num(&args, 5, 220.0)? as f32;
                let amp  = self.arg_num(&args, 6, 0.15)? as f32;
                let lfo_rate  = self.arg_num(&args, 7, 0.5)? as f32;
                let lfo_depth = self.arg_num(&args, 8, 0.02)? as f32;
                if let Some(audio) = &self.audio {
                    audio.set_tone(idx, ToneParams { x, y, z, w, freq, amp, lfo_rate, lfo_depth });
                }
                return Ok(Value::Unit);
            }

            #[cfg(not(target_arch = "wasm32"))]
            "audio_listener" | "ผู้ฟัง" => {
                let cry = self.arg_num(&args, 0, 1.0)? as f32;
                let sry = self.arg_num(&args, 1, 0.0)? as f32;
                let crx = self.arg_num(&args, 2, 1.0)? as f32;
                let srx = self.arg_num(&args, 3, 0.0)? as f32;
                if let Some(audio) = &self.audio {
                    audio.set_listener(cry, sry, crx, srx);
                }
                return Ok(Value::Unit);
            }

            #[cfg(not(target_arch = "wasm32"))]
            "audio_bgm" | "เพลงพื้นหลัง" => {
                let path = match args.first() {
                    Some(Value::Str(s)) => s.clone(),
                    _ => return Ok(Value::Unit),
                };
                let vol = self.arg_num(&args, 1, 0.5)? as f32;
                if let Some(audio) = &self.audio {
                    audio.load_bgm(&path, vol);
                }
                return Ok(Value::Unit);
            }

            #[cfg(not(target_arch = "wasm32"))]
            "audio_bgm_volume" | "ระดับเสียงพื้นหลัง" => {
                let vol = self.arg_num(&args, 0, 0.5)? as f32;
                if let Some(audio) = &self.audio {
                    audio.set_bgm_volume(vol);
                }
                return Ok(Value::Unit);
            }

            #[cfg(not(target_arch = "wasm32"))]
            "audio_volume" | "ระดับเสียง" => {
                let vol = self.arg_num(&args, 0, 0.7)? as f32;
                if let Some(audio) = &self.audio {
                    audio.set_master_volume(vol);
                }
                return Ok(Value::Unit);
            }

            // WASM audio builtins — delegate to Web Audio API
            #[cfg(target_arch = "wasm32")]
            "audio_tone" | "เสียงโทน" => {
                let idx  = self.arg_num(&args, 0, 0.0)? as usize;
                let x    = self.arg_num(&args, 1, 0.0)? as f32;
                let y    = self.arg_num(&args, 2, 0.0)? as f32;
                let z    = self.arg_num(&args, 3, 0.0)? as f32;
                let w    = self.arg_num(&args, 4, 1.0)? as f32;
                let freq = self.arg_num(&args, 5, 220.0)? as f32;
                let amp  = self.arg_num(&args, 6, 0.15)? as f32;
                let lfo_rate  = self.arg_num(&args, 7, 0.5)? as f32;
                let lfo_depth = self.arg_num(&args, 8, 0.02)? as f32;
                crate::gfx::audio_web::set_tone(idx, x, y, z, w, freq, amp, lfo_rate, lfo_depth);
                return Ok(Value::Unit);
            }

            #[cfg(target_arch = "wasm32")]
            "audio_listener" | "ผู้ฟัง" => {
                let cry = self.arg_num(&args, 0, 1.0)? as f32;
                let sry = self.arg_num(&args, 1, 0.0)? as f32;
                let crx = self.arg_num(&args, 2, 1.0)? as f32;
                let srx = self.arg_num(&args, 3, 0.0)? as f32;
                crate::gfx::audio_web::set_listener(cry, sry, crx, srx);
                return Ok(Value::Unit);
            }

            #[cfg(target_arch = "wasm32")]
            "audio_bgm" | "เพลงพื้นหลัง" => {
                let path = self.arg_str(&args, 0, "");
                let vol  = self.arg_num(&args, 1, 0.5)? as f32;
                crate::gfx::audio_web::load_bgm(&path, vol);
                return Ok(Value::Unit);
            }

            #[cfg(target_arch = "wasm32")]
            "audio_bgm_volume" | "ระดับเสียงพื้นหลัง" => {
                let vol = self.arg_num(&args, 0, 0.5)? as f32;
                crate::gfx::audio_web::set_bgm_volume(vol);
                return Ok(Value::Unit);
            }

            #[cfg(target_arch = "wasm32")]
            "audio_volume" | "ระดับเสียง" => {
                let vol = self.arg_num(&args, 0, 0.7)? as f32;
                crate::gfx::audio_web::set_master_volume(vol);
                return Ok(Value::Unit);
            }

            // ── รอหน้าต่าง() — block until window closed / Escape ──
            "รอหน้าต่าง" | "wait_window" | "gfx_wait" => {
                #[cfg(not(target_arch = "wasm32"))]
                loop {
                    let still_open = {
                        let gfx = self.gfx.borrow();
                        gfx.window.as_ref()
                            .map(|w| w.is_open() && !w.is_key_down(minifb::Key::Escape))
                            .unwrap_or(false)
                    };
                    if !still_open { break; }
                    let (buf, w, h) = {
                        let gfx = self.gfx.borrow();
                        (gfx.buffer.clone(), gfx.width, gfx.height)
                    };
                    let mut gfx = self.gfx.borrow_mut();
                    if let Some(win) = gfx.window.as_mut() {
                        if win.update_with_buffer(&buf, w, h).is_err() { break; }
                    }
                }
                return Ok(Value::Unit);
            }

            // ── File I/O ──────────────────────────────────────────────────────
            "read_file" | "อ่านไฟล์" => {
                let path = self.arg_str(&args, 0, "");
                return std::fs::read_to_string(&path)
                    .map(Value::Str)
                    .map_err(|e| EvalErr::from(format!("read_file '{path}': {e}")));
            }
            "write_file" | "เขียนไฟล์" => {
                let path    = self.arg_str(&args, 0, "");
                let content = self.arg_str(&args, 1, "");
                std::fs::write(&path, content.as_bytes())
                    .map_err(|e| EvalErr::from(format!("write_file '{path}': {e}")))?;
                return Ok(Value::Unit);
            }
            "print_file" | "พิมพ์ไฟล์" => {
                let content = self.arg_str(&args, 0, "");
                print!("{content}");
                return Ok(Value::Unit);
            }

            // ── CLI arguments ─────────────────────────────────────────────────
            "get_args" | "รับอาร์กิวเมนต์" => {
                let v: Vec<Value> = std::env::args().map(Value::Str).collect();
                return Ok(Value::List(v));
            }

            // ── String utilities ──────────────────────────────────────────────
            "split" | "str_split" | "แยก" => {
                let s   = self.arg_str(&args, 0, "");
                let sep = self.arg_str(&args, 1, "\n");
                let sep = if sep.is_empty() { "\n".into() } else { sep };
                let parts: Vec<Value> = s.split(sep.as_str())
                    .map(|p| Value::Str(p.to_string())).collect();
                return Ok(Value::List(parts));
            }
            "trim" | "str_trim" | "ตัดช่องว่าง" => {
                let s = self.arg_str(&args, 0, "");
                return Ok(Value::Str(s.trim().to_string()));
            }
            "starts_with" | "str_starts_with" | "เริ่มด้วย" => {
                let s      = self.arg_str(&args, 0, "");
                let prefix = self.arg_str(&args, 1, "");
                return Ok(Value::Bool(s.starts_with(prefix.as_str())));
            }
            "ends_with" | "str_ends_with" | "ลงท้ายด้วย" => {
                let s      = self.arg_str(&args, 0, "");
                let suffix = self.arg_str(&args, 1, "");
                return Ok(Value::Bool(s.ends_with(suffix.as_str())));
            }
            "str_replace" | "แทนสตริง" => {
                let s    = self.arg_str(&args, 0, "");
                let from = self.arg_str(&args, 1, "");
                let to   = self.arg_str(&args, 2, "");
                return Ok(Value::Str(s.replace(from.as_str(), to.as_str())));
            }
            "str_find" | "หาในสตริง" => {
                let s      = self.arg_str(&args, 0, "");
                let needle = self.arg_str(&args, 1, "");
                // Return char index (not byte index) for consistency with substr
                let pos = s.find(needle.as_str())
                    .map(|byte_i| s[..byte_i].chars().count() as f64)
                    .unwrap_or(-1.0);
                return Ok(Value::Number(pos));
            }
            "substr" | "str_slice" | "ส่วนสตริง" => {
                let s     = self.arg_str(&args, 0, "");
                let start = self.arg_num(&args, 1, 0.0)? as usize;
                let len   = args.get(2)
                    .map(|v| self.to_number(v).unwrap_or(999999.0) as usize)
                    .unwrap_or_else(|| s.chars().count().saturating_sub(start));
                let chars: Vec<char> = s.chars().collect();
                let end   = (start + len).min(chars.len());
                let slice: String = chars.get(start..end).unwrap_or(&[]).iter().collect();
                return Ok(Value::Str(slice));
            }
            "to_str" | "str" | "num_str" | "แปลงสตริง" => {
                let v = args.into_iter().next().unwrap_or(Value::Unit);
                return Ok(Value::Str(v.to_string()));
            }
            "str_repeat" | "ทำซ้ำสตริง" => {
                let s = self.arg_str(&args, 0, "");
                let n = self.arg_num(&args, 1, 1.0)? as usize;
                return Ok(Value::Str(s.repeat(n)));
            }
            "str_upper" => {
                let s = self.arg_str(&args, 0, "");
                return Ok(Value::Str(s.to_uppercase()));
            }
            "str_lower" => {
                let s = self.arg_str(&args, 0, "");
                return Ok(Value::Str(s.to_lowercase()));
            }
            "str_len" | "len" | "ความยาว" => {
                match args.first() {
                    Some(Value::Str(s))  => return Ok(Value::Number(s.chars().count() as f64)),
                    Some(Value::List(v)) => return Ok(Value::Number(v.len() as f64)),
                    _ => return Ok(Value::Number(0.0)),
                }
            }

            // ── FNV-1a hash (deterministic, normalized 0.0–1.0) ──────────────
            "hash_str" | "แฮช" => {
                let s = self.arg_str(&args, 0, "");
                let mut h: u64 = 14695981039346656037_u64;
                for b in s.bytes() { h ^= b as u64; h = h.wrapping_mul(1099511628211); }
                return Ok(Value::Number((h & 0xFFFFFF) as f64 / 16777215.0));
            }
            "hash_int" | "แฮชจำนวน" => {
                let s = self.arg_str(&args, 0, "");
                let n = self.arg_num(&args, 1, 100.0)? as u64;
                let mut h: u64 = 14695981039346656037_u64;
                for b in s.bytes() { h ^= b as u64; h = h.wrapping_mul(1099511628211); }
                return Ok(Value::Number((h % n.max(1)) as f64));
            }

            // ── List utilities ────────────────────────────────────────────────
            "list_new" | "รายการใหม่" => {
                return Ok(Value::List(Vec::new()));
            }
            "list_push" | "เพิ่มรายการ" => {
                let lst = args.first().cloned().unwrap_or(Value::List(vec![]));
                let val = args.get(1).cloned().unwrap_or(Value::Unit);
                if let Value::List(mut v) = lst { v.push(val); return Ok(Value::List(v)); }
                return Ok(Value::List(vec![val]));
            }
            "list_get" | "รับรายการ" => {
                let lst = args.first().cloned().unwrap_or(Value::List(vec![]));
                let i   = self.arg_num(&args, 1, 0.0)? as usize;
                if let Value::List(v) = lst {
                    return Ok(v.get(i).cloned().unwrap_or(Value::Str(String::new())));
                }
                return Ok(Value::Str(String::new()));
            }
            "list_join" | "join" | "รวมรายการ" => {
                let lst = args.first().cloned().unwrap_or(Value::List(vec![]));
                let sep = args.get(1).map(|v| v.to_string()).unwrap_or_default();
                if let Value::List(v) = lst {
                    return Ok(Value::Str(v.iter().map(|x| x.to_string())
                        .collect::<Vec<_>>().join(&sep)));
                }
                return Ok(Value::Str(String::new()));
            }

            // ══════════════════════════════════════════════════════════════════
            // SVG EXPORT  (svg_begin / svg_rect / svg_circle / svg_line /
            //              svg_polyline / svg_text / svg_end / hsl_color)
            // Chinese aliases: 开始SVG 结束SVG SVG矩形 SVG圆形 SVG线段 SVG折线 SVG文本 HSL颜色
            // Thai aliases:    เริ่มSVG จบSVG SVGสี่เหลี่ยม SVGวงกลม SVGเส้น SVGเส้นหัก SVGข้อความ สีHSL
            // ══════════════════════════════════════════════════════════════════

            "svg_begin" | "开始SVG" | "เริ่มSVG" => {
                let path   = self.arg_str(&args, 0, "output.svg");
                let width  = self.arg_num(&args, 1, 800.0)?;
                let height = self.arg_num(&args, 2, 600.0)?;
                *self.svg.borrow_mut() = Some(SvgWriter::new(path, width, height));
                return Ok(Value::Unit);
            }

            "svg_rect" | "SVG矩形" | "SVGสี่เหลี่ยม" => {
                let x    = self.arg_num(&args, 0, 0.0)?;
                let y    = self.arg_num(&args, 1, 0.0)?;
                let w    = self.arg_num(&args, 2, 10.0)?;
                let h    = self.arg_num(&args, 3, 10.0)?;
                let fill = self.arg_str(&args, 4, "#ffffff");
                if let Some(svg) = self.svg.borrow_mut().as_mut() {
                    svg.elements.push(format!(
                        "<rect x=\"{x:.1}\" y=\"{y:.1}\" width=\"{w:.1}\" \
                         height=\"{h:.1}\" fill=\"{fill}\"/>"));
                }
                return Ok(Value::Unit);
            }

            "svg_circle" | "SVG圆形" | "SVGวงกลม" => {
                let cx   = self.arg_num(&args, 0, 0.0)?;
                let cy   = self.arg_num(&args, 1, 0.0)?;
                let r    = self.arg_num(&args, 2, 5.0)?;
                let fill = self.arg_str(&args, 3, "#ffffff");
                if let Some(svg) = self.svg.borrow_mut().as_mut() {
                    svg.elements.push(format!(
                        "<circle cx=\"{cx:.1}\" cy=\"{cy:.1}\" r=\"{r:.1}\" fill=\"{fill}\"/>"));
                }
                return Ok(Value::Unit);
            }

            "svg_line" | "SVG线段" | "SVGเส้น" => {
                let x1     = self.arg_num(&args, 0, 0.0)?;
                let y1     = self.arg_num(&args, 1, 0.0)?;
                let x2     = self.arg_num(&args, 2, 0.0)?;
                let y2     = self.arg_num(&args, 3, 0.0)?;
                let stroke = self.arg_str(&args, 4, "#ffffff");
                let sw     = self.arg_num(&args, 5, 1.0)?;
                if let Some(svg) = self.svg.borrow_mut().as_mut() {
                    svg.elements.push(format!(
                        "<line x1=\"{x1:.1}\" y1=\"{y1:.1}\" x2=\"{x2:.1}\" y2=\"{y2:.1}\" \
                         stroke=\"{stroke}\" stroke-width=\"{sw:.1}\"/>"));
                }
                return Ok(Value::Unit);
            }

            "svg_polyline" | "SVG折线" | "SVGเส้นหัก" => {
                let pts    = self.arg_str(&args, 0, "");
                let stroke = self.arg_str(&args, 1, "#ffffff");
                let sw     = self.arg_num(&args, 2, 1.0)?;
                if let Some(svg) = self.svg.borrow_mut().as_mut() {
                    svg.elements.push(format!(
                        "<polyline points=\"{pts}\" fill=\"none\" \
                         stroke=\"{stroke}\" stroke-width=\"{sw:.1}\"/>"));
                }
                return Ok(Value::Unit);
            }

            "svg_text" | "SVG文本" | "SVGข้อความ" => {
                let x    = self.arg_num(&args, 0, 0.0)?;
                let y    = self.arg_num(&args, 1, 0.0)?;
                let text = self.arg_str(&args, 2, "");
                let fill = self.arg_str(&args, 3, "#ffffff");
                let size = self.arg_num(&args, 4, 12.0)?;
                if let Some(svg) = self.svg.borrow_mut().as_mut() {
                    let safe = text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
                    svg.elements.push(format!(
                        "<text x=\"{x:.1}\" y=\"{y:.1}\" fill=\"{fill}\" \
                         font-family=\"monospace\" font-size=\"{size:.0}\">{safe}</text>"));
                }
                return Ok(Value::Unit);
            }

            "svg_end" | "结束SVG" | "จบSVG" => {
                {
                    let borrow = self.svg.borrow();
                    if let Some(svg) = borrow.as_ref() {
                        svg.save().map_err(|e| EvalErr::from(format!("svg_end: {e}")))?;
                    }
                }
                *self.svg.borrow_mut() = None;
                return Ok(Value::Unit);
            }

            "hsl_color" | "HSL颜色" | "สีHSL" => {
                let h = self.arg_num(&args, 0, 0.0)?;
                let s = self.arg_num(&args, 1, 70.0)?;
                let l = self.arg_num(&args, 2, 50.0)?;
                return Ok(Value::Str(hsl_to_hex(h, s, l)));
            }

            // ══════════════════════════════════════════════════════════════════
            // FFT / AUDIO ANALYSIS BUILTINS  (native only)
            // ══════════════════════════════════════════════════════════════════

            // fft_push(samples_list) — feed raw audio samples and run FFT
            #[cfg(not(target_arch = "wasm32"))]
            "fft_push" | "วิเคราะห์เสียง" => {
                if let Some(Value::List(v)) = args.first() {
                    let samples: Vec<f32> = v.iter()
                        .filter_map(|x| if let Value::Number(n) = x { Some(*n as f32) } else { None })
                        .collect();
                    self.fft.borrow_mut().push_samples(&samples);
                }
                return Ok(Value::Unit);
            }

            // fft_bands(n) → list of n log-spaced magnitude bands (0..1)
            #[cfg(not(target_arch = "wasm32"))]
            "fft_bands" | "แถบความถี่" => {
                let n = self.arg_num(&args, 0, 32.0)? as usize;
                let bands = self.fft.borrow().freq_bands(n);
                *self.fft_bands_cache.borrow_mut() = bands.clone();
                return Ok(Value::List(bands.into_iter().map(|v| Value::Number(v as f64)).collect()));
            }

            // fft_beat() → bool
            #[cfg(not(target_arch = "wasm32"))]
            "fft_beat" | "จังหวะเสียง" => {
                return Ok(Value::Bool(self.fft.borrow().is_beat()));
            }

            // fft_beat_ratio() → f64  (1.0 = at threshold, >1 = strong beat)
            #[cfg(not(target_arch = "wasm32"))]
            "fft_beat_ratio" | "อัตราจังหวะ" => {
                return Ok(Value::Number(self.fft.borrow().beat_ratio() as f64));
            }

            // fft_rms() → f64
            #[cfg(not(target_arch = "wasm32"))]
            "fft_rms" | "ระดับRMS" => {
                return Ok(Value::Number(self.fft.borrow().rms() as f64));
            }

            // fft_dominant_freq() → f64  in Hz
            #[cfg(not(target_arch = "wasm32"))]
            "fft_dominant_freq" | "ความถี่หลัก" => {
                return Ok(Value::Number(self.fft.borrow().dominant_freq() as f64));
            }

            // ── wasm32 stubs: fft builtins are no-ops on web ───────────────
            #[cfg(target_arch = "wasm32")]
            "fft_push" | "วิเคราะห์เสียง" => { return Ok(Value::Unit); }
            #[cfg(target_arch = "wasm32")]
            "fft_bands" | "แถบความถี่" => {
                let n = self.arg_num(&args, 0, 32.0)? as usize;
                return Ok(Value::List(vec![Value::Number(0.0); n]));
            }
            #[cfg(target_arch = "wasm32")]
            "fft_beat" | "จังหวะเสียง" => { return Ok(Value::Bool(false)); }
            #[cfg(target_arch = "wasm32")]
            "fft_beat_ratio" | "อัตราจังหวะ" => { return Ok(Value::Number(1.0)); }
            #[cfg(target_arch = "wasm32")]
            "fft_rms" | "ระดับRMS" => { return Ok(Value::Number(0.0)); }
            #[cfg(target_arch = "wasm32")]
            "fft_dominant_freq" | "ความถี่หลัก" => { return Ok(Value::Number(0.0)); }

            // ══════════════════════════════════════════════════════════════════
            // PROCEDURAL TEXTURE BLIT BUILTINS  (screen-space)
            // All: name(dst_x, dst_y, width, height, ...params, palette)
            // palette: "rainbow" | "fire" | "ocean" | "psychedelic" | "neon" | "forest"
            // ══════════════════════════════════════════════════════════════════

            // tex_checkerboard(x, y, w, h, tiles, r1,g1,b1, r2,g2,b2)
            "tex_checkerboard" | "ลายตารางหมากรุก" => {
                let (tx,ty,tw,th) = self.tex_rect(&args)?;
                let tiles = self.arg_num(&args, 4, 8.0)? as u32;
                let (r1,g1,b1) = (self.arg_num(&args,5,255.)? as u32, self.arg_num(&args,6,255.)? as u32, self.arg_num(&args,7,255.)? as u32);
                let (r2,g2,b2) = (self.arg_num(&args,8,0.)? as u32,   self.arg_num(&args,9,0.)? as u32,   self.arg_num(&args,10,0.)? as u32);
                let c1 = (r1<<16)|(g1<<8)|b1; let c2 = (r2<<16)|(g2<<8)|b2;
                let mut gfx = self.gfx.borrow_mut();
                let (bw, bh) = (gfx.width, gfx.height);
                for row in 0..th { for col in 0..tw {
                    let cx = col as u32 * tiles / tw as u32;
                    let cy = row as u32 * tiles / th as u32;
                    let (dx, dy) = (tx+col, ty+row);
                    if dx < bw && dy < bh { gfx.buffer[dy*bw+dx] = if (cx+cy)%2==0 { c1 } else { c2 }; }
                }}
                return Ok(Value::Unit);
            }

            // tex_gradient(x, y, w, h, angle_deg, r1,g1,b1, r2,g2,b2)
            "tex_gradient" | "ลายไล่สี" => {
                let (tx,ty,tw,th) = self.tex_rect(&args)?;
                let angle = self.arg_num(&args, 4, 0.0)? as f32;
                let (r1,g1,b1) = (self.arg_num(&args,5,0.)? as f32/255., self.arg_num(&args,6,0.)? as f32/255., self.arg_num(&args,7,0.)? as f32/255.);
                let (r2,g2,b2) = (self.arg_num(&args,8,255.)? as f32/255., self.arg_num(&args,9,255.)? as f32/255., self.arg_num(&args,10,255.)? as f32/255.);
                let (ca, sa) = (angle.to_radians().cos(), angle.to_radians().sin());
                let mut gfx = self.gfx.borrow_mut();
                let (bw, bh) = (gfx.width, gfx.height);
                for row in 0..th { for col in 0..tw {
                    let nx = col as f32/tw as f32 - 0.5; let ny = row as f32/th as f32 - 0.5;
                    let t = ((nx*ca + ny*sa + 0.707)/1.414).clamp(0.,1.);
                    let (dx, dy) = (tx+col, ty+row);
                    if dx < bw && dy < bh { gfx.buffer[dy*bw+dx] = tex_rgb(r1+(r2-r1)*t, g1+(g2-g1)*t, b1+(b2-b1)*t); }
                }}
                return Ok(Value::Unit);
            }

            // tex_noise(x, y, w, h, scale, octaves, seed, palette)
            "tex_noise" | "ลายนอยส์" => {
                let (tx,ty,tw,th) = self.tex_rect(&args)?;
                let scale   = self.arg_num(&args, 4, 4.0)? as f32;
                let octaves = self.arg_num(&args, 5, 4.0)? as u32;
                let seed    = self.arg_num(&args, 6, 0.0)? as u32;
                let palette = self.arg_str(&args, 7, "rainbow");
                let mut gfx = self.gfx.borrow_mut();
                let (bw, bh) = (gfx.width, gfx.height);
                for row in 0..th { for col in 0..tw {
                    let v = tex_fbm(col as f32*scale/tw as f32, row as f32*scale/th as f32, octaves, seed);
                    let [r,g,b] = tex_palette(&palette, v);
                    let (dx, dy) = (tx+col, ty+row);
                    if dx < bw && dy < bh { gfx.buffer[dy*bw+dx] = tex_rgb(r, g, b); }
                }}
                return Ok(Value::Unit);
            }

            // tex_freq_map(x, y, w, h, time, speed, palette)
            // Uses bands written by the last fft_bands() call.
            "tex_freq_map" | "ลายความถี่" => {
                let (tx,ty,tw,th) = self.tex_rect(&args)?;
                let time    = self.arg_num(&args, 4, 0.0)? as f32;
                let speed   = self.arg_num(&args, 5, 0.3)? as f32;
                let palette = self.arg_str(&args, 6, "rainbow");
                let bands: Vec<f32> = {
                    let c = self.fft_bands_cache.borrow();
                    if c.is_empty() { vec![0.0; 32] } else { c.clone() }
                };
                let n = bands.len().max(1);
                let mut gfx = self.gfx.borrow_mut();
                let (bw, bh) = (gfx.width, gfx.height);
                for row in 0..th { for col in 0..tw {
                    let band_idx = (col * n / tw.max(1)).min(n-1);
                    let mag = bands[band_idx].clamp(0.,1.);
                    let fill_y = (mag * th as f32) as usize;
                    if row >= th.saturating_sub(fill_y) {
                        let t = (col as f32/tw as f32 + time*speed) % 1.0;
                        let [r,g,b] = tex_palette(&palette, t);
                        let bright = mag * (1.0 - row as f32/th as f32 * 0.5);
                        let (dx, dy) = (tx+col, ty+row);
                        if dx < bw && dy < bh { gfx.buffer[dy*bw+dx] = tex_rgb(r*bright, g*bright, b*bright); }
                    }
                }}
                return Ok(Value::Unit);
            }

            // tex_spiral(x, y, w, h, freq, bands, time, palette)
            "tex_spiral" | "ลายเกลียวหมุน" => {
                let (tx,ty,tw,th) = self.tex_rect(&args)?;
                let freq    = self.arg_num(&args, 4, 5.0)? as f32;
                let n_bands = self.arg_num(&args, 5, 8.0)? as f32;
                let time    = self.arg_num(&args, 6, 0.0)? as f32;
                let palette = self.arg_str(&args, 7, "rainbow");
                let mut gfx = self.gfx.borrow_mut();
                let (bw, bh) = (gfx.width, gfx.height);
                for row in 0..th { for col in 0..tw {
                    let nx = col as f32/tw as f32 - 0.5; let ny = row as f32/th as f32 - 0.5;
                    let r  = (nx*nx + ny*ny).sqrt();
                    let theta = ny.atan2(nx);
                    let t = ((r*freq - theta/std::f32::consts::TAU + time*0.5) * n_bands % 1.0).abs();
                    let [cr,cg,cb] = tex_palette(&palette, t);
                    let (dx, dy) = (tx+col, ty+row);
                    if dx < bw && dy < bh { gfx.buffer[dy*bw+dx] = tex_rgb(cr, cg, cb); }
                }}
                return Ok(Value::Unit);
            }

            // tex_ripple(x, y, w, h, freq, cx, cy, time, palette)
            "tex_ripple" | "ลายระลอก" => {
                let (tx,ty,tw,th) = self.tex_rect(&args)?;
                let freq    = self.arg_num(&args, 4, 10.0)? as f32;
                let rcx     = self.arg_num(&args, 5, 0.5)? as f32;
                let rcy     = self.arg_num(&args, 6, 0.5)? as f32;
                let time    = self.arg_num(&args, 7, 0.0)? as f32;
                let palette = self.arg_str(&args, 8, "ocean");
                let mut gfx = self.gfx.borrow_mut();
                let (bw, bh) = (gfx.width, gfx.height);
                for row in 0..th { for col in 0..tw {
                    let nx = col as f32/tw as f32 - rcx; let ny = row as f32/th as f32 - rcy;
                    let r = (nx*nx + ny*ny).sqrt();
                    let t = ((r*freq - time) % 1.0).abs();
                    let [cr,cg,cb] = tex_palette(&palette, t);
                    let (dx, dy) = (tx+col, ty+row);
                    if dx < bw && dy < bh { gfx.buffer[dy*bw+dx] = tex_rgb(cr, cg, cb); }
                }}
                return Ok(Value::Unit);
            }

            // tex_mandelbrot(x, y, w, h, zoom, cx, cy, max_iter, palette)
            "tex_mandelbrot" | "ลายแมนเดลบรอต" => {
                let (tx,ty,tw,th) = self.tex_rect(&args)?;
                let zoom     = self.arg_num(&args, 4, 1.0)?;
                let mcx      = self.arg_num(&args, 5, -0.5)?;
                let mcy      = self.arg_num(&args, 6, 0.0)?;
                let max_iter = self.arg_num(&args, 7, 64.0)? as u32;
                let palette  = self.arg_str(&args, 8, "psychedelic");
                let mut gfx = self.gfx.borrow_mut();
                let (bw, bh) = (gfx.width, gfx.height);
                for row in 0..th { for col in 0..tw {
                    let zx0 = (col as f64/tw as f64 - 0.5)/zoom + mcx;
                    let zy0 = (row as f64/th as f64 - 0.5)/zoom + mcy;
                    let mut x = 0.0f64; let mut y = 0.0f64; let mut i = 0u32;
                    while i < max_iter && x*x+y*y < 4.0 { let t=x*x-y*y+zx0; y=2.0*x*y+zy0; x=t; i+=1; }
                    let t = if i==max_iter { 0.0f32 } else {
                        (i as f32 - (x as f32*x as f32+y as f32*y as f32).ln().ln()/2.0f32.ln()) / max_iter as f32
                    };
                    let [cr,cg,cb] = tex_palette(&palette, t.clamp(0.,1.));
                    let (dx, dy) = (tx+col, ty+row);
                    if dx < bw && dy < bh { gfx.buffer[dy*bw+dx] = tex_rgb(cr, cg, cb); }
                }}
                return Ok(Value::Unit);
            }

            // tex_julia(x, y, w, h, c_re, c_im, max_iter, palette)
            "tex_julia" | "ลายจูเลีย" => {
                let (tx,ty,tw,th) = self.tex_rect(&args)?;
                let c_re     = self.arg_num(&args, 4, -0.7)?;
                let c_im     = self.arg_num(&args, 5, 0.27)?;
                let max_iter = self.arg_num(&args, 6, 64.0)? as u32;
                let palette  = self.arg_str(&args, 7, "neon");
                let mut gfx = self.gfx.borrow_mut();
                let (bw, bh) = (gfx.width, gfx.height);
                for row in 0..th { for col in 0..tw {
                    let mut zx = (col as f64/tw as f64 - 0.5)*3.5;
                    let mut zy = (row as f64/th as f64 - 0.5)*3.5;
                    let mut i = 0u32;
                    while i < max_iter && zx*zx+zy*zy < 4.0 { let t=zx*zx-zy*zy+c_re; zy=2.0*zx*zy+c_im; zx=t; i+=1; }
                    let t = i as f32 / max_iter as f32;
                    let [cr,cg,cb] = tex_palette(&palette, t);
                    let (dx, dy) = (tx+col, ty+row);
                    if dx < bw && dy < bh { gfx.buffer[dy*bw+dx] = tex_rgb(cr, cg, cb); }
                }}
                return Ok(Value::Unit);
            }

            // tex_voronoi(x, y, w, h, cells, seed, palette)
            "tex_voronoi" | "ลายโวโรนอย" => {
                let (tx,ty,tw,th) = self.tex_rect(&args)?;
                let cells   = self.arg_num(&args, 4, 16.0)? as u32;
                let seed    = self.arg_num(&args, 5, 42.0)? as u32;
                let palette = self.arg_str(&args, 6, "rainbow");
                let pts: Vec<[f32;2]> = (0..cells).map(|i| [tex_hash(i as i32,0,seed), tex_hash(i as i32,1,seed+999)]).collect();
                let mut gfx = self.gfx.borrow_mut();
                let (bw, bh) = (gfx.width, gfx.height);
                for row in 0..th { for col in 0..tw {
                    let (fx, fy) = (col as f32/tw as f32, row as f32/th as f32);
                    let (min_d, nearest) = pts.iter().enumerate().fold((f32::MAX,0usize), |(d,idx),(i,&[cx,cy])| {
                        let dd = (fx-cx).powi(2)+(fy-cy).powi(2);
                        if dd < d { (dd,i) } else { (d,idx) }
                    });
                    let t = (nearest as f32/cells as f32 + min_d*4.0) % 1.0;
                    let [cr,cg,cb] = tex_palette(&palette, t);
                    let (dx, dy) = (tx+col, ty+row);
                    if dx < bw && dy < bh { gfx.buffer[dy*bw+dx] = tex_rgb(cr, cg, cb); }
                }}
                return Ok(Value::Unit);
            }

            // tex_halftone(x, y, w, h, dot_size, time, palette)
            "tex_halftone" | "ลายฮาล์ฟโทน" => {
                let (tx,ty,tw,th) = self.tex_rect(&args)?;
                let dot_size = self.arg_num(&args, 4, 0.05)? as f32;
                let time     = self.arg_num(&args, 5, 0.0)? as f32;
                let palette  = self.arg_str(&args, 6, "rainbow");
                let mut gfx = self.gfx.borrow_mut();
                let (bw, bh) = (gfx.width, gfx.height);
                for row in 0..th { for col in 0..tw {
                    let (fx, fy) = (col as f32/tw as f32, row as f32/th as f32);
                    let gx = (fx/dot_size).floor(); let gy = (fy/dot_size).floor();
                    let lx = (fx/dot_size - gx - 0.5)*2.0; let ly = (fy/dot_size - gy - 0.5)*2.0;
                    let r = (lx*lx + ly*ly).sqrt();
                    let t = (gx/(1.0/dot_size) + time*0.1) % 1.0;
                    let a = if r < 0.7 { ((0.7-r)/0.7).clamp(0.,1.) } else { 0.0 };
                    if a > 0.0 {
                        let [cr,cg,cb] = tex_palette(&palette, t);
                        let (dx, dy) = (tx+col, ty+row);
                        if dx < bw && dy < bh { gfx.buffer[dy*bw+dx] = tex_rgb(cr, cg, cb); }
                    }
                }}
                return Ok(Value::Unit);
            }

            // ══════════════════════════════════════════════════════════════════
            // RENDER / LIGHTING MODES  (holographic cel shading)
            // ══════════════════════════════════════════════════════════════════
            // set_shade_mode(m) — 0 flat · 1 cel · 2 holo (default)
            "set_shade_mode" | "设置着色" | "シェード設定" | "셰이드모드" | "ตั้งการแรเงา" => {
                let m = self.arg_num(&args, 0, 2.0)? as u8;
                self.gfx.borrow_mut().shade_mode = m;
                return Ok(Value::Unit);
            }
            // set_cel_bands(n) — number of posterisation bands (>=2)
            "set_cel_bands" | "设置色阶" | "セル段数" | "셀밴드" | "ตั้งระดับสี" => {
                let n = (self.arg_num(&args, 0, 4.0)? as u32).max(2);
                self.gfx.borrow_mut().shade.bands = n;
                return Ok(Value::Unit);
            }
            // set_shadow_color(r,g,b) — coloured-shadow tint, 0-255
            "set_shadow_color" | "设置阴影色" | "影の色" | "그림자색" | "ตั้งสีเงา" => {
                let r=self.arg_num(&args,0,26.)? as f32/255.0;
                let g=self.arg_num(&args,1,33.)? as f32/255.0;
                let b=self.arg_num(&args,2,77.)? as f32/255.0;
                self.gfx.borrow_mut().shade.shadow = [r,g,b];
                return Ok(Value::Unit);
            }
            // set_rim(strength, r,g,b) — holographic fresnel edge glow
            "set_rim" | "设置边缘光" | "リム設定" | "림라이트" | "ตั้งขอบเรือง" => {
                let s=self.arg_num(&args,0,0.6)? as f32;
                let r=self.arg_num(&args,1,115.)? as f32/255.0;
                let g=self.arg_num(&args,2,217.)? as f32/255.0;
                let b=self.arg_num(&args,3,255.)? as f32/255.0;
                let mut gfx=self.gfx.borrow_mut();
                gfx.shade.rim = s; gfx.shade.rim_color = [r,g,b];
                return Ok(Value::Unit);
            }

            // ══════════════════════════════════════════════════════════════════
            // 3-D PRIMITIVES  (src/gfx/shapes.rs)  — "Inkscape for 3-D"
            //   shape(cx,cy,cz,  sx,sy,sz,  rx,ry,rz,  mode,  e0,e1,e2)
            //     centre (cx,cy,cz), per-axis scale, Euler rotation (radians),
            //     mode: 0 filled · 1 wireframe · 2 both,
            //     e0..e2: shape-specific (segments / sides / ratio …).
            //   Pen colour (set_color) drives fill lighting and wireframe colour.
            // ══════════════════════════════════════════════════════════════════
            n if crate::gfx::shapes::canon(n).is_some() => {
                let kind = crate::gfx::shapes::canon(n).unwrap();
                let cx=self.arg_num(&args,0,0.)? as f32; let cy=self.arg_num(&args,1,0.)? as f32; let cz=self.arg_num(&args,2,0.)? as f32;
                let sx=self.arg_num(&args,3,1.)? as f32; let sy=self.arg_num(&args,4,1.)? as f32; let sz=self.arg_num(&args,5,1.)? as f32;
                let rx=self.arg_num(&args,6,0.)? as f32; let ry=self.arg_num(&args,7,0.)? as f32; let rz=self.arg_num(&args,8,0.)? as f32;
                let mode=self.arg_num(&args,9,0.)? as i32;
                let e0=self.arg_num(&args,10,0.)? as f32; let e1=self.arg_num(&args,11,0.)? as f32; let e2=self.arg_num(&args,12,0.)? as f32;
                if let Some(mesh)=crate::gfx::shapes::build(kind,[cx,cy,cz,sx,sy,sz,rx,ry,rz],e0,e1,e2){
                    let mut gfx=self.gfx.borrow_mut();
                    gfx.emit_mesh(&mesh,mode);
                }
                return Ok(Value::Unit);
            }

            _ => {}
        }

        // User-defined function
        if let Some(def) = self.functions.get(name).cloned() {
            let mut call_env = Env::new();
            // Seed env with non-Do globals (skip entry-point blocks to avoid infinite recursion).
            // Pass 1: evaluate each global with the call-site env — simple literals succeed.
            // Pass 2: retry failed globals with the partially-built call_env so that compound
            //         globals (e.g. `FM = (NZ + FZ) / 2.0`) can resolve their dependencies.
            let non_do_globals: Vec<_> = self.globals.iter()
                .filter(|(_, expr)| !matches!(expr, Expr::Do(_)))
                .map(|(k, e)| (k.clone(), e.clone()))
                .collect();
            let mut pending: Vec<(String, Expr)> = Vec::new();
            for (k, expr) in &non_do_globals {
                let mut tmp = env.clone();
                if let Ok(v) = self.eval_expr(expr, &mut tmp) {
                    call_env.insert(k.clone(), v);
                } else {
                    pending.push((k.clone(), expr.clone()));
                }
            }
            // Second pass: retry compound globals now that literals are in call_env.
            for (k, expr) in &pending {
                let mut tmp = env.clone();
                tmp.extend(call_env.clone());
                if let Ok(v) = self.eval_expr(expr, &mut tmp) {
                    call_env.insert(k.clone(), v);
                }
            }
            for (param, arg) in def.params.iter().zip(args) {
                call_env.insert(param.clone(), arg);
            }
            return match self.exec_block(&def.body, &mut call_env) {
                Ok(v) => Ok(v.unwrap_or(Value::Unit)),
                Err(EvalErr::Return(v)) => Ok(v),
                Err(e) => Err(e),
            };
        }

        Err(EvalErr::from(format!("unknown function '{name}'")))
    }

    fn call_value(&self, v: Value, args: Vec<Value>) -> EvalResult {
        match v {
            Value::Fn(params, body, mut captured) => {
                for (p, a) in params.iter().zip(args) {
                    captured.insert(p.clone(), a);
                }
                match self.exec_block(&body, &mut captured) {
                    Ok(v) => Ok(v.unwrap_or(Value::Unit)),
                    Err(EvalErr::Return(v)) => Ok(v),
                    Err(e) => Err(e),
                }
            }
            other => Err(EvalErr::from(format!("cannot call {:?}", other))),
        }
    }

    fn call_method(&self, recv: Value, method: &str, args: Vec<Value>) -> EvalResult {
        match (&recv, method) {
            (Value::Str(s), "is_empty" | "是空") => Ok(Value::Bool(s.is_empty())),
            (Value::Str(s), "len" | "长")        => Ok(Value::Number(s.len() as f64)),
            (Value::Str(s), "to_string" | "转文") => Ok(Value::Str(s.clone())),
            (Value::Str(s), "contains" | "包含") => {
                if let Some(Value::Str(sub)) = args.first() {
                    Ok(Value::Bool(s.contains(sub.as_str())))
                } else { Ok(Value::Bool(false)) }
            }
            (Value::Str(s), "push_str" | "推_文") => {
                let mut s2 = s.clone();
                if let Some(Value::Str(a)) = args.first() { s2.push_str(a); }
                Ok(Value::Str(s2))
            }
            (Value::List(v), "len" | "长") => Ok(Value::Number(v.len() as f64)),
            (Value::List(v), "push" | "推") => {
                let mut v2 = v.clone();
                if let Some(a) = args.first() { v2.push(a.clone()); }
                Ok(Value::List(v2))
            }
            (Value::Ok(inner), _) | (Value::Err(inner), _) => Ok(*inner.clone()),
            _ => Err(EvalErr::from(format!("no method '{method}' on {recv}"))),
        }
    }

    // ─── Pattern matching ─────────────────────────────────────────────────────

    fn match_pattern(&self, pat: &Pattern, val: &Value) -> Option<Env> {
        match (pat, val) {
            (Pattern::Wildcard, _) => Some(Env::new()),
            (Pattern::Str(s), Value::Str(v)) if s == v => Some(Env::new()),
            (Pattern::Number(n), Value::Number(v)) if (n - v).abs() < 1e-12 => Some(Env::new()),
            (Pattern::Bool(b), Value::Bool(v)) if b == v => Some(Env::new()),
            (Pattern::Ident(name), _) => {
                let mut e = Env::new();
                e.insert(name.clone(), val.clone());
                Some(e)
            }
            (Pattern::Constructor(ctor, inner_pat), _) => {
                let (matches, inner_val) = match (ctor.as_str(), val) {
                    ("ok"  | "好", Value::Ok(v))  => (true, Some(v.as_ref().clone())),
                    ("bad" | "坏", Value::Err(v)) => (true, Some(v.as_ref().clone())),
                    ("ok"  | "好", v) if !matches!(v, Value::Err(_)) => (true, Some(v.clone())),
                    _ => (false, None),
                };
                if !matches { return None; }
                match (inner_pat, inner_val) {
                    (Some(p), Some(v)) => self.match_pattern(p, &v),
                    (None, _)          => Some(Env::new()),
                    (Some(p), None)    => self.match_pattern(p, &Value::Unit),
                }
            }
            _ => None,
        }
    }

    // ─── Utilities ───────────────────────────────────────────────────────────

    fn value_to_iter(&self, val: Value) -> Result<Vec<Value>, EvalErr> {
        match val {
            Value::List(v)   => Ok(v),
            Value::Str(s)    => Ok(s.chars().map(|c| Value::Str(c.to_string())).collect()),
            Value::Number(n) => Ok((0..n as i64).map(|i| Value::Number(i as f64)).collect()),
            other => Err(EvalErr::from(format!("cannot iterate over {:?}", other))),
        }
    }

    fn is_truthy(&self, val: &Value) -> bool {
        match val {
            Value::Bool(b)     => *b,
            Value::Unit        => false,
            Value::Number(n)   => *n != 0.0,
            Value::Str(s)      => !s.is_empty(),
            Value::List(v)     => !v.is_empty(),
            Value::Ok(_)       => true,
            Value::Err(_)      => false,
            Value::Fn(_, _, _) => true,
        }
    }

    fn to_number(&self, val: &Value) -> Result<f64, EvalErr> {
        match val {
            Value::Number(n) => Ok(*n),
            Value::Str(s)    => s.parse().map_err(|_| EvalErr::from(format!("cannot convert '{s}' to number"))),
            other => Err(EvalErr::from(format!("expected number, got {:?}", other))),
        }
    }

    /// Get the n-th argument as f64, falling back to `default` if missing.
    fn arg_num(&self, args: &[Value], n: usize, default: f64) -> Result<f64, EvalErr> {
        match args.get(n) {
            Some(v) => self.to_number(v),
            None    => Ok(default),
        }
    }

    fn arg_str(&self, args: &[Value], n: usize, default: &str) -> String {
        args.get(n).map(|v| v.to_string()).unwrap_or_else(|| default.to_string())
    }

    /// Parse (dst_x, dst_y, width, height) from the first four args of a tex_* builtin.
    fn tex_rect(&self, args: &[Value]) -> Result<(usize, usize, usize, usize), EvalErr> {
        let tx = self.arg_num(args, 0, 0.0)? as usize;
        let ty = self.arg_num(args, 1, 0.0)? as usize;
        let tw = self.arg_num(args, 2, 256.0)? as usize;
        let th = self.arg_num(args, 3, 256.0)? as usize;
        Ok((tx, ty, tw.max(1), th.max(1)))
    }

    fn apply_binop(&self, op: &BinOp, l: Value, r: Value) -> EvalResult {
        match op {
            BinOp::Add => match (l, r) {
                (Value::Number(a), Value::Number(b)) => Ok(Value::Number(a + b)),
                (Value::Str(a), Value::Str(b))       => Ok(Value::Str(a + &b)),
                (Value::Str(a), b)                   => Ok(Value::Str(a + &b.to_string())),
                (a, Value::Str(b))                   => Ok(Value::Str(a.to_string() + &b)),
                (a, b) => Err(EvalErr::from(format!("cannot add {:?} and {:?}", a, b))),
            },
            BinOp::Sub => Ok(Value::Number(self.to_number(&l)? - self.to_number(&r)?)),
            BinOp::Mul => Ok(Value::Number(self.to_number(&l)? * self.to_number(&r)?)),
            BinOp::Div => Ok(Value::Number(self.to_number(&l)? / self.to_number(&r)?)),
            BinOp::Rem => Ok(Value::Number(self.to_number(&l)? % self.to_number(&r)?)),
            BinOp::Eq  => Ok(Value::Bool(values_equal(&l, &r))),
            BinOp::Ne  => Ok(Value::Bool(!values_equal(&l, &r))),
            BinOp::Lt  => Ok(Value::Bool(self.to_number(&l)? < self.to_number(&r)?)),
            BinOp::Gt  => Ok(Value::Bool(self.to_number(&l)? > self.to_number(&r)?)),
            BinOp::Le  => Ok(Value::Bool(self.to_number(&l)? <= self.to_number(&r)?)),
            BinOp::Ge  => Ok(Value::Bool(self.to_number(&l)? >= self.to_number(&r)?)),
            BinOp::And => Ok(Value::Bool(self.is_truthy(&l) && self.is_truthy(&r))),
            BinOp::Or  => Ok(Value::Bool(self.is_truthy(&l) || self.is_truthy(&r))),
        }
    }

    fn builtin_format(&self, args: &[Value]) -> Result<String, EvalErr> {
        if args.is_empty() { return Ok(String::new()); }
        let fmt = match &args[0] {
            Value::Str(s) => s.clone(),
            other => return Ok(other.to_string()),
        };

        let mut result = String::new();
        let mut arg_idx = 1usize;
        let mut chars = fmt.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '{' {
                if chars.peek() == Some(&'}') {
                    chars.next();
                    if arg_idx < args.len() {
                        result.push_str(&args[arg_idx].to_string());
                        arg_idx += 1;
                    }
                } else {
                    let mut spec = String::new();
                    for ch in chars.by_ref() {
                        if ch == '}' { break; }
                        spec.push(ch);
                    }
                    if arg_idx < args.len() {
                        if spec.starts_with(":.") {
                            if let Value::Number(n) = &args[arg_idx] {
                                let prec: usize = spec[2..].trim_end_matches('f')
                                    .parse().unwrap_or(2);
                                result.push_str(&format!("{:.prec$}", n));
                                arg_idx += 1;
                                continue;
                            }
                        }
                        result.push_str(&args[arg_idx].to_string());
                        arg_idx += 1;
                    }
                }
            } else {
                result.push(c);
            }
        }
        Ok(result)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn str_to_minifb_key(name: &str) -> Option<minifb::Key> {
    use minifb::Key;
    Some(match name {
        "numpad0" | "kp0" => Key::NumPad0,
        "numpad1" | "kp1" => Key::NumPad1,
        "numpad2" | "kp2" => Key::NumPad2,
        "numpad3" | "kp3" => Key::NumPad3,
        "numpad4" | "kp4" => Key::NumPad4,
        "numpad5" | "kp5" => Key::NumPad5,
        "numpad6" | "kp6" => Key::NumPad6,
        "numpad7" | "kp7" => Key::NumPad7,
        "numpad8" | "kp8" => Key::NumPad8,
        "numpad9" | "kp9" => Key::NumPad9,
        "numpad+" | "kp+" => Key::NumPadPlus,
        "numpad-" | "kp-" => Key::NumPadMinus,
        "numpad*" | "kp*" => Key::NumPadAsterisk,
        "numpad/" | "kp/" => Key::NumPadSlash,
        "left"   => Key::Left,
        "right"  => Key::Right,
        "up"     => Key::Up,
        "down"   => Key::Down,
        "space"  => Key::Space,
        "enter"  => Key::Enter,
        "escape" => Key::Escape,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "lshift" | "leftshift"  => Key::LeftShift,
        "rshift" | "rightshift" => Key::RightShift,
        "lctrl"  | "leftctrl"   => Key::LeftCtrl,
        "rctrl"  | "rightctrl"  => Key::RightCtrl,
        "tab"    => Key::Tab,
        "backspace" => Key::Backspace,
        "delete" => Key::Delete,
        "insert" => Key::Insert,
        "home"   => Key::Home,
        "end"    => Key::End,
        "a" => Key::A, "b" => Key::B, "c" => Key::C, "d" => Key::D,
        "e" => Key::E, "f" => Key::F, "g" => Key::G, "h" => Key::H,
        "i" => Key::I, "j" => Key::J, "k" => Key::K, "l" => Key::L,
        "m" => Key::M, "n" => Key::N, "o" => Key::O, "p" => Key::P,
        "q" => Key::Q, "r" => Key::R, "s" => Key::S, "t" => Key::T,
        "u" => Key::U, "v" => Key::V, "w" => Key::W, "x" => Key::X,
        "y" => Key::Y, "z" => Key::Z,
        "0" => Key::Key0, "1" => Key::Key1, "2" => Key::Key2,
        "3" => Key::Key3, "4" => Key::Key4, "5" => Key::Key5,
        "6" => Key::Key6, "7" => Key::Key7, "8" => Key::Key8,
        "9" => Key::Key9,
        _ => return None,
    })
}

fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => (x - y).abs() < 1e-12,
        (Value::Str(x), Value::Str(y))       => x == y,
        (Value::Bool(x), Value::Bool(y))     => x == y,
        (Value::Unit, Value::Unit)            => true,
        _ => false,
    }
}

// Rasteriser functions live in crate::gfx::raster — imported at top of file.

// ── Window platform helpers ────────────────────────────────────────────────────

/// Hide the console window that the OS auto-attaches to console-subsystem
/// processes. No-op on non-Windows and when no console is present.
#[cfg(not(target_arch = "wasm32"))]
fn hide_console_window() {
    #[cfg(windows)]
    unsafe {
        extern "system" {
            fn GetConsoleWindow() -> isize;
            fn ShowWindow(hwnd: isize, nCmdShow: i32) -> i32;
        }
        let hwnd = GetConsoleWindow();
        if hwnd != 0 {
            ShowWindow(hwnd, 0); // SW_HIDE = 0
        }
    }
}

/// Move and resize the foreground window so it fills the primary monitor
/// (0,0 → screen_w × screen_h) and sits above the taskbar (HWND_TOPMOST).
#[cfg(all(not(target_arch = "wasm32"), windows))]
fn reposition_fullscreen(screen_w: i32, screen_h: i32) {
    unsafe {
        extern "system" {
            fn GetForegroundWindow() -> isize;
            fn SetWindowPos(hwnd: isize, insert_after: isize,
                            x: i32, y: i32, cx: i32, cy: i32,
                            flags: u32) -> i32;
        }
        let hwnd = GetForegroundWindow();
        if hwnd != 0 {
            // HWND_TOPMOST = -1, SWP_SHOWWINDOW = 0x0040
            SetWindowPos(hwnd, -1isize, 0, 0, screen_w, screen_h, 0x0040);
        }
    }
}

/// Query the primary display resolution on non-Windows platforms.
/// Falls back to 1920×1080 if the size cannot be determined.
#[cfg(all(not(target_arch = "wasm32"), not(windows)))]
fn native_screen_size() -> (f64, f64) {
    // On Linux/macOS we don't have an easy dependency-free call; return a
    // sensible default. Callers can always pass explicit dimensions.
    (1920.0, 1080.0)
}
