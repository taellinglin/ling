// src/runtime/mod.rs — tree-walking interpreter with graphics support
use std::cell::RefCell;
use std::collections::HashMap;
use crate::parser::ast::*;
use crate::gfx::{GfxState, Light};
use crate::gfx::raster::{fill_triangle, draw_line};
use ling_audio::{AudioEngine, ToneParams};

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

// ─── Interpreter ─────────────────────────────────────────────────────────────

pub struct Interpreter {
    globals:   HashMap<String, Expr>,
    functions: HashMap<String, FnDef>,
    modules:   HashMap<String, Vec<FnDef>>,
    gfx:       RefCell<GfxState>,
    /// Optional audio engine — `None` if no audio device is available.
    audio:     Option<AudioEngine>,
}

impl Interpreter {
    pub fn new() -> Self {
        let audio = AudioEngine::new()
            .map_err(|e| eprintln!("audio init failed (no sound): {e}"))
            .ok();
        Self {
            globals:   HashMap::new(),
            functions: HashMap::new(),
            modules:   HashMap::new(),
            gfx:       RefCell::new(GfxState::new()),
            audio,
        }
    }

    pub fn run_program(&mut self, program: &Program) -> Result<(), String> {
        for item in &program.items {
            self.register_item("", item)?;
        }
        let entry = self.find_entry()
            .ok_or("no entry point — need `bind start = do {...}` or `ผูก เริ่ม = ทำ {...}`")?;
        let mut env = Env::new();
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
        }
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
                    let mut local = env.clone();
                    return Ok(self.exec_block(then, &mut local)?.unwrap_or(Value::Unit));
                }
                for (ei_cond, ei_body) in elseifs {
                    if self.is_truthy(&self.eval_expr(ei_cond, env)?) {
                        let mut local = env.clone();
                        return Ok(self.exec_block(ei_body, &mut local)?.unwrap_or(Value::Unit));
                    }
                }
                if let Some(eb) = else_body {
                    let mut local = env.clone();
                    return Ok(self.exec_block(eb, &mut local)?.unwrap_or(Value::Unit));
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
                // Target 120 fps — software renderer is the bottleneck, not the timer.
                #[allow(deprecated)]
                win.limit_update_rate(Some(std::time::Duration::from_millis(8)));
                gfx.buffer = vec![0u32; w * h];
                gfx.width  = w;
                gfx.height = h;
                gfx.window = Some(win);
                gfx.sync_projection();   // auto-configure camera CX/CY/FOCAL
                return Ok(Value::Unit);
            }

            // ── เติม(r, g, b) — fill / clear screen with colour ──
            "เติม" | "fill" | "gfx_fill" | "clear" => {
                let r = self.arg_num(&args, 0, 0.0)? as u32;
                let g = self.arg_num(&args, 1, 0.0)? as u32;
                let b = self.arg_num(&args, 2, 0.0)? as u32;
                let c = (r << 16) | (g << 8) | b;
                let mut gfx = self.gfx.borrow_mut();
                gfx.buffer.fill(c);
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
                let w = gfx.width;
                let h = gfx.height;
                fill_triangle(&mut gfx.buffer, w, h, color, x0, y0, x1, y1, x2, y2);
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
                let w = gfx.width;
                let h = gfx.height;
                draw_line(&mut gfx.buffer, w, h, color, x0, y0, x1, y1);
                return Ok(Value::Unit);
            }

            // ── วาดจุด(x, y) — plot a single pixel ──
            "วาดจุด" | "draw_pixel" | "gfx_pixel" | "pixel" => {
                let px = self.arg_num(&args, 0, 0.0)? as i32;
                let py = self.arg_num(&args, 1, 0.0)? as i32;
                let mut gfx = self.gfx.borrow_mut();
                let color = gfx.color;
                let w = gfx.width;
                let h = gfx.height;
                if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                    gfx.buffer[py as usize * w + px as usize] = color;
                }
                return Ok(Value::Unit);
            }

            // ── แสดงผล() — flush depth queue, then present frame to screen ──
            "แสดงผล" | "present" | "gfx_present" | "show" => {
                let mut gfx = self.gfx.borrow_mut();
                // Flush deferred 3-D draw calls (depth-sorted, painter's algorithm).
                // Copy size to locals first — can't borrow gfx.width/height while
                // gfx.buffer is mutably borrowed through the same reference.
                if !gfx.depth_queue.is_empty() {
                    let w = gfx.width;
                    let h = gfx.height;
                    let queue = std::mem::take(&mut gfx.depth_queue);
                    queue.flush(&mut gfx.buffer, w, h);
                }
                // Clone buffer for the window update (window.as_mut() needs &mut gfx
                // which conflicts with &gfx.buffer — clone resolves that).
                let buf = gfx.buffer.clone();
                let w   = gfx.width;
                let h   = gfx.height;
                if let Some(win) = gfx.window.as_mut() {
                    win.update_with_buffer(&buf, w, h)
                        .map_err(|e| EvalErr::from(format!("present error: {e}")))?;
                }
                return Ok(Value::Unit);
            }

            // ── เปิดหน้าต่างเต็มจอ(title) — borderless fullscreen window ──
            "เปิดหน้าต่างเต็มจอ" | "open_fullscreen" | "fullscreen" => {
                let title = args.get(0).map(|v| v.to_string()).unwrap_or_else(|| "Ling".into());
                let w = args.get(1).map(|v| self.to_number(v).unwrap_or(1920.0) as usize).unwrap_or(1920);
                let h = args.get(2).map(|v| self.to_number(v).unwrap_or(1080.0) as usize).unwrap_or(1080);
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
                gfx.sync_projection();   // auto-configure camera CX/CY/FOCAL
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
                let gfx = self.gfx.borrow();
                let open = gfx.window.as_ref()
                    .map(|w| w.is_open() && !w.is_key_down(minifb::Key::Escape))
                    .unwrap_or(false);
                return Ok(Value::Bool(open));
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
            "vtex_grid" | "ลายตาราง" => {
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
            "vtex_rings" | "ลายวงซ้อน" => {
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
            "vtex_star" | "ลายดาว" => {
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
            "vtex_spiral" | "ลายเกลียว" => {
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
            "vtex_flower" | "ลายดอก" => {
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
            "vtex_letter_rain" | "ลายอักษรไหล" => {
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
            "vtex_hyperbolic_uv" | "ลายไฮเพอร์โบลิก" => {
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
            "vtex_halftone" | "ลายจุด" => {
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
            "vtex_tessellated" | "ลายตาข่าย" => {
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
            "vtex_lotus" | "ลายดอกบัว" => {
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
            "vtex_chakra" | "ลายจักร" => {
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
            "vtex_yantra" | "ลายยันต์" => {
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

            // ══════════════════════════════════════════════════════════════════
            // AUDIO BUILTINS
            // ══════════════════════════════════════════════════════════════════

            // audio_tone(idx, x, y, z, w, freq, amp, lfo_rate, lfo_depth)
            // Place / update a 4-D positional sine tone.
            // idx       — slot index (0-63)
            // x,y,z     — world-space position of the sound source
            // w         — 4th-dimension value driving cross-modulation
            // freq      — carrier frequency in Hz
            // amp       — amplitude 0..1
            // lfo_rate  — vibrato rate in Hz
            // lfo_depth — vibrato depth (fraction of freq, e.g. 0.03)
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

            // audio_listener(cry, sry, crx, srx)
            // Sync listener orientation with the graphics camera each frame.
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

            // audio_bgm(path)  — load a WAV file and loop it as BGM
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

            // audio_bgm_volume(vol)  — adjust BGM volume without reloading
            "audio_bgm_volume" | "ระดับเสียงพื้นหลัง" => {
                let vol = self.arg_num(&args, 0, 0.5)? as f32;
                if let Some(audio) = &self.audio {
                    audio.set_bgm_volume(vol);
                }
                return Ok(Value::Unit);
            }

            // audio_volume(vol)  — master volume (affects all tones + BGM)
            "audio_volume" | "ระดับเสียง" => {
                let vol = self.arg_num(&args, 0, 0.7)? as f32;
                if let Some(audio) = &self.audio {
                    audio.set_master_volume(vol);
                }
                return Ok(Value::Unit);
            }

            // ── รอหน้าต่าง() — block until window closed / Escape ──
            "รอหน้าต่าง" | "wait_window" | "gfx_wait" => {
                loop {
                    // Check open status (immutable borrow, dropped at end of block)
                    let still_open = {
                        let gfx = self.gfx.borrow();
                        gfx.window.as_ref()
                            .map(|w| w.is_open() && !w.is_key_down(minifb::Key::Escape))
                            .unwrap_or(false)
                    };
                    if !still_open { break; }

                    // Extract buffer while holding immutable borrow, then drop it
                    let (buf, w, h) = {
                        let gfx = self.gfx.borrow();
                        (gfx.buffer.clone(), gfx.width, gfx.height)
                    };

                    // Update window (mutable borrow) — now safe, immutable borrow gone
                    let mut gfx = self.gfx.borrow_mut();
                    if let Some(win) = gfx.window.as_mut() {
                        if win.update_with_buffer(&buf, w, h).is_err() { break; }
                    }
                }
                return Ok(Value::Unit);
            }

            _ => {}
        }

        // User-defined function
        if let Some(def) = self.functions.get(name).cloned() {
            let mut call_env = Env::new();
            // Seed env with non-Do globals (skip entry-point blocks to avoid infinite recursion)
            for (k, expr) in &self.globals {
                if matches!(expr, Expr::Do(_)) { continue; }
                let mut tmp = env.clone();
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
