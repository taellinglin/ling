// src/runtime/mod.rs — tree-walking interpreter with 2-D graphics support
use std::cell::RefCell;
use std::collections::HashMap;
use crate::parser::ast::*;

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

// ─── Graphics state ───────────────────────────────────────────────────────────

struct GfxState {
    window:  Option<minifb::Window>,
    buffer:  Vec<u32>,
    width:   usize,
    height:  usize,
    /// Current drawing colour as 0x00RRGGBB.
    color:   u32,
}

impl GfxState {
    fn new() -> Self {
        Self {
            window: None,
            buffer: Vec::new(),
            width:  0,
            height: 0,
            color:  0x00FFFFFF,
        }
    }
}

// ─── Interpreter ─────────────────────────────────────────────────────────────

pub struct Interpreter {
    globals:   HashMap<String, Expr>,
    functions: HashMap<String, FnDef>,
    modules:   HashMap<String, Vec<FnDef>>,
    gfx:       RefCell<GfxState>,
}

impl Interpreter {
    pub fn new() -> Self {
        Self {
            globals:   HashMap::new(),
            functions: HashMap::new(),
            modules:   HashMap::new(),
            gfx:       RefCell::new(GfxState::new()),
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
                // Cap update rate ~60 fps so the loop doesn't burn 100% CPU.
                #[allow(deprecated)]
                win.limit_update_rate(Some(std::time::Duration::from_millis(16)));
                gfx.buffer = vec![0u32; w * h];
                gfx.width  = w;
                gfx.height = h;
                gfx.window = Some(win);
                return Ok(Value::Unit);
            }

            // ── เติม(r, g, b) — fill / clear screen with colour ──
            "เติม" | "fill" | "gfx_fill" | "clear" => {
                let r = self.arg_num(&args, 0, 0.0)? as u32;
                let g = self.arg_num(&args, 1, 0.0)? as u32;
                let b = self.arg_num(&args, 2, 0.0)? as u32;
                let c = (r << 16) | (g << 8) | b;
                let mut gfx = self.gfx.borrow_mut();
                for px in gfx.buffer.iter_mut() { *px = c; }
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

            // ── แสดงผล() — present frame to screen ──
            "แสดงผล" | "present" | "gfx_present" | "show" => {
                // Clone buffer data while holding *immutable* borrow, then drop
                // it before taking the *mutable* borrow needed for the window.
                let (buf, w, h) = {
                    let gfx = self.gfx.borrow();
                    (gfx.buffer.clone(), gfx.width, gfx.height)
                };
                let mut gfx = self.gfx.borrow_mut();
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
                win.limit_update_rate(Some(std::time::Duration::from_millis(16)));
                gfx.buffer = vec![0u32; w * h];
                gfx.width  = w;
                gfx.height = h;
                gfx.window = Some(win);
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

// ─── Software 2-D rasterizer (no external deps) ──────────────────────────────

/// Fill a triangle using barycentric edge-function rasterisation.
/// `color` is 0x00RRGGBB.  `buf` is row-major, top-left origin.
fn fill_triangle(
    buf: &mut Vec<u32>,
    width: usize, height: usize,
    color: u32,
    x0: f32, y0: f32,
    x1: f32, y1: f32,
    x2: f32, y2: f32,
) {
    if width == 0 || height == 0 { return; }

    let min_x = x0.min(x1).min(x2).max(0.0) as i32;
    let max_x = x0.max(x1).max(x2).min(width  as f32 - 1.0) as i32;
    let min_y = y0.min(y1).min(y2).max(0.0) as i32;
    let max_y = y0.max(y1).max(y2).min(height as f32 - 1.0) as i32;
    if min_x > max_x || min_y > max_y { return; }

    for py in min_y..=max_y {
        for px in min_x..=max_x {
            let fx = px as f32 + 0.5;
            let fy = py as f32 + 0.5;
            // Edge functions — positive on the same side as the interior
            let e0 = (x1 - x0) * (fy - y0) - (y1 - y0) * (fx - x0);
            let e1 = (x2 - x1) * (fy - y1) - (y2 - y1) * (fx - x1);
            let e2 = (x0 - x2) * (fy - y2) - (y0 - y2) * (fx - x2);
            if (e0 >= 0.0 && e1 >= 0.0 && e2 >= 0.0)
            || (e0 <= 0.0 && e1 <= 0.0 && e2 <= 0.0)
            {
                buf[py as usize * width + px as usize] = color;
            }
        }
    }
}

/// Bresenham line drawing into a `0x00RRGGBB` pixel buffer.
fn draw_line(
    buf: &mut Vec<u32>,
    width: usize, height: usize,
    color: u32,
    x0: f32, y0: f32,
    x1: f32, y1: f32,
) {
    if width == 0 || height == 0 { return; }
    let mut x = x0 as i32;
    let mut y = y0 as i32;
    let x2 = x1 as i32;
    let y2 = y1 as i32;
    let dx = (x2 - x).abs();
    let dy = -((y2 - y).abs());
    let sx: i32 = if x < x2 { 1 } else { -1 };
    let sy: i32 = if y < y2 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        if x >= 0 && y >= 0 && (x as usize) < width && (y as usize) < height {
            buf[y as usize * width + x as usize] = color;
        }
        if x == x2 && y == y2 { break; }
        let e2 = 2 * err;
        if e2 >= dy { err += dy; x += sx; }
        if e2 <= dx { err += dx; y += sy; }
    }
}
