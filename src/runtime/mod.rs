// src/runtime/mod.rs — tree-walking interpreter
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
    // Callable user-defined function (params, body, captured env)
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

// ─── Return signal ───────────────────────────────────────────────────────────

/// Used to unwind the call stack on `return`.
struct ReturnSignal(Value);

// ─── Interpreter ─────────────────────────────────────────────────────────────

pub struct Interpreter {
    globals: HashMap<String, Expr>,
    functions: HashMap<String, FnDef>,  // qualified name → def
    modules: HashMap<String, Vec<FnDef>>,
}

impl Interpreter {
    pub fn new() -> Self {
        Self {
            globals: HashMap::new(),
            functions: HashMap::new(),
            modules: HashMap::new(),
        }
    }

    pub fn run_program(&mut self, program: &Program) -> Result<(), String> {
        // First pass: register everything
        for item in &program.items {
            self.register_item("", item)?;
        }

        // Find and run the entry point
        let entry = self.find_entry()
            .ok_or("no entry point — need `bind start = do {...}` or `灵符 启 = 执 {...}`")?;

        let mut env = Env::new();
        self.eval_expr(&entry, &mut env).map(|_| ())
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
            Item::TypeAlias(_, _) => {} // ignored at runtime
        }
        Ok(())
    }

    fn find_entry(&self) -> Option<Expr> {
        // Try all common "start" words across supported languages
        for key in &[
            "start", "启",      // English, Chinese
            "เริ่ม",             // Thai
            "시작",              // Korean
            "начать", "начало",  // Russian
            "inicio", "comenzar", // Spanish
            "début", "commencer", // French
            "anfang", "starten", // German
            "início",            // Portuguese
            "शुरू",              // Hindi
            "ابدأ",              // Arabic
            "main",              // universal alias
        ] {
            if let Some(e) = self.globals.get(*key) { return Some(e.clone()); }
        }
        self.globals.values().find(|e| matches!(e, Expr::Do(_))).cloned()
    }

    // ─── Expression evaluation ────────────────────────────────────────────────

    fn eval_expr(&self, expr: &Expr, env: &mut Env) -> Result<Value, String> {
        match expr {
            Expr::Str(s)    => Ok(Value::Str(s.clone())),
            Expr::Number(n) => Ok(Value::Number(*n)),
            Expr::Bool(b)   => Ok(Value::Bool(*b)),
            Expr::Unit      => Ok(Value::Unit),
            Expr::Array(elems) => {
                let vs: Vec<_> = elems.iter().map(|e| self.eval_expr(e, env)).collect::<Result<_,_>>()?;
                Ok(Value::List(vs))
            }

            Expr::Ident(name) => self.lookup(name, env),

            Expr::Path(segs) => {
                // Last segment might be a function ref; for now resolve as an identifier
                if segs.len() == 1 { return self.lookup(&segs[0], env); }
                // Path resolves to a callable — return a dummy
                Ok(Value::Str(segs.join("::")))
            }

            Expr::Ref(inner) => self.eval_expr(inner, env),

            Expr::Await(inner) => self.eval_expr(inner, env),

            Expr::Do(stmts) => {
                let mut local = env.clone();
                match self.exec_block(stmts, &mut local)? {
                    Some(v) => Ok(v),
                    None    => Ok(Value::Unit),
                }
            }

            Expr::BinOp(op, lhs, rhs) => {
                let l = self.eval_expr(lhs, env)?;
                let r = self.eval_expr(rhs, env)?;
                self.apply_binop(op, l, r)
            }

            Expr::If { cond, then, elseifs, else_body } => {
                if self.is_truthy(&self.eval_expr(cond, env)?) {
                    let mut local = env.clone();
                    return match self.exec_block(then, &mut local)? {
                        Some(v) => Ok(v),
                        None    => Ok(Value::Unit),
                    };
                }
                for (ei_cond, ei_body) in elseifs {
                    if self.is_truthy(&self.eval_expr(ei_cond, env)?) {
                        let mut local = env.clone();
                        return match self.exec_block(ei_body, &mut local)? {
                            Some(v) => Ok(v),
                            None    => Ok(Value::Unit),
                        };
                    }
                }
                if let Some(eb) = else_body {
                    let mut local = env.clone();
                    return match self.exec_block(eb, &mut local)? {
                        Some(v) => Ok(v),
                        None    => Ok(Value::Unit),
                    };
                }
                Ok(Value::Unit)
            }

            Expr::For { var, iter, body } => {
                let iter_val = self.eval_expr(iter, env)?;
                let items = self.value_to_iter(iter_val)?;
                for item in items {
                    let mut local = env.clone();
                    local.insert(var.clone(), item);
                    self.exec_block(body, &mut local)?;
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
                let list: Vec<Value> = (lo_n..hi_n).map(|i| Value::Number(i as f64)).collect();
                Ok(Value::List(list))
            }

            Expr::Index(base, idx) => {
                let b = self.eval_expr(base, env)?;
                let i = self.eval_expr(idx, env)?;
                let n = self.to_number(&i)? as usize;
                match b {
                    Value::List(v) => v.get(n).cloned().ok_or_else(|| format!("index {n} out of bounds")),
                    Value::Str(s)  => s.chars().nth(n).map(|c| Value::Str(c.to_string()))
                                        .ok_or_else(|| format!("index {n} out of bounds")),
                    other => Err(format!("cannot index {:?}", other)),
                }
            }

            Expr::Call(callee, args) => {
                let arg_vals: Vec<Value> = args.iter()
                    .map(|a| self.eval_expr(a, env))
                    .collect::<Result<_,_>>()?;

                match callee.as_ref() {
                    Expr::Ident(name) => self.call_named(name, arg_vals, env),
                    Expr::Path(segs)  => {
                        let key = segs.join("::");
                        self.call_named(&key, arg_vals, env)
                    }
                    _ => {
                        let v = self.eval_expr(callee, env)?;
                        match v {
                            Value::Fn(params, body, mut captured) => {
                                for (p, a) in params.iter().zip(arg_vals) {
                                    captured.insert(p.clone(), a);
                                }
                                match self.exec_block(&body, &mut captured)? {
                                    Some(ret) => Ok(ret),
                                    None => Ok(Value::Unit),
                                }
                            }
                            other => Err(format!("cannot call {:?}", other)),
                        }
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

    /// Execute a block; returns `Some(v)` if a return was hit, `None` otherwise.
    fn exec_block(&self, stmts: &[Stmt], env: &mut Env) -> Result<Option<Value>, String> {
        for stmt in stmts {
            match stmt {
                Stmt::Bind(name, expr) => {
                    let v = self.eval_expr(expr, env)?;
                    env.insert(name.clone(), v);
                }
                Stmt::Return(expr) => {
                    return Ok(Some(self.eval_expr(expr, env)?));
                }
                Stmt::Expr(expr) => {
                    self.eval_expr(expr, env)?;
                }
            }
        }
        Ok(None)
    }

    // ─── Dispatch helpers ─────────────────────────────────────────────────────

    fn lookup(&self, name: &str, env: &Env) -> Result<Value, String> {
        if let Some(v) = env.get(name) { return Ok(v.clone()); }
        // Functions as values
        if self.functions.contains_key(name) {
            let def = &self.functions[name];
            return Ok(Value::Fn(def.params.clone(), def.body.clone(), Env::new()));
        }
        Err(format!("undefined: '{name}'"))
    }

    fn call_named(&self, name: &str, args: Vec<Value>, env: &Env) -> Result<Value, String> {
        // Built-in functions
        match name {
            // Print — Chinese: 印 | Thai: พิมพ์ | Korean: 출력 | Russian: вывести
            "print" | "println" | "印" | "พิมพ์" | "출력" | "вывести" | "imprimir" | "afficher" => {
                let s = args.iter().map(|v| v.to_string()).collect::<Vec<_>>().join("");
                println!("{s}");
                return Ok(Value::Unit);
            }
            // Format — Chinese: 格式 | Thai: รูปแบบ
            "format" | "格式" | "รูปแบบ" | "форматировать" | "formatear" | "formater" => {
                return Ok(Value::Str(self.builtin_format(&args)?));
            }
            // String join / concatenation
            "格式::拼接" | "format::join" => {
                match args.first() {
                    Some(Value::List(items)) => {
                        return Ok(Value::Str(items.iter().map(|v| v.to_string()).collect()));
                    }
                    _ => return Ok(Value::Str(self.builtin_format(&args)?)),
                }
            }
            // Result constructors — ok(val) / bad(val)
            "ok" | "好" => {
                let val = args.into_iter().next().unwrap_or(Value::Unit);
                return Ok(Value::Ok(Box::new(val)));
            }
            "bad" | "坏" | "err" => {
                let val = args.into_iter().next().unwrap_or(Value::Unit);
                return Ok(Value::Err(Box::new(val)));
            }
            // Vec constructors
            "向量::从" | "Vec::from" => {
                if let Some(Value::List(v)) = args.first() {
                    return Ok(Value::List(v.clone()));
                }
                return Ok(Value::List(args));
            }
            "向量::有容量" | "Vec::with_capacity" => return Ok(Value::List(Vec::new())),
            // Timer stubs — return demo values
            "计时::获取当前小时" | "Timer::hour" => return Ok(Value::Number(14.0)),
            "计时::现在" | "Timer::now"          => return Ok(Value::Number(1000.0)),
            // Flow/concurrency stubs — execute synchronously
            "流水::睡眠" | "Flow::sleep"         => return Ok(Value::Unit),
            "流水::并行" | "Flow::parallel"       => {
                // Run the closure immediately
                if let Some(Value::Fn(params, body, mut cap)) = args.first().cloned() {
                    let _ = params; // no params for `||` closures
                    match self.exec_block(&body, &mut cap)? {
                        Some(v) => return Ok(v),
                        None    => return Ok(Value::Unit),
                    }
                }
                return Ok(Value::Unit);
            }
            // format convenience shim for `格式("...", args)`
            _ => {}
        }

        // User-defined function
        if let Some(def) = self.functions.get(name).cloned() {
            let mut call_env = Env::new();
            // Seed with globals, skipping Do-blocks (entry points) to avoid
            // re-running the whole program and causing infinite recursion.
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
            return match self.exec_block(&def.body, &mut call_env)? {
                Some(v) => Ok(v),
                None    => Ok(Value::Unit),
            };
        }

        Err(format!("unknown function '{name}'"))
    }

    fn call_method(&self, recv: Value, method: &str, args: Vec<Value>) -> Result<Value, String> {
        match (&recv, method) {
            // String methods (Chinese aliases)
            (Value::Str(s), "是空" | "is_empty") => Ok(Value::Bool(s.is_empty())),
            (Value::Str(s), "长"   | "len")      => Ok(Value::Number(s.len() as f64)),
            (Value::Str(s), "转文" | "to_string") => Ok(Value::Str(s.clone())),
            (Value::Str(s), "包含" | "contains")  => {
                if let Some(Value::Str(sub)) = args.first() {
                    Ok(Value::Bool(s.contains(sub.as_str())))
                } else { Ok(Value::Bool(false)) }
            }
            (Value::Str(s), "推_文" | "push_str") => {
                let mut s2 = s.clone();
                if let Some(Value::Str(a)) = args.first() { s2.push_str(a); }
                Ok(Value::Str(s2))
            }
            // List methods
            (Value::List(v), "长" | "len") => Ok(Value::Number(v.len() as f64)),
            (Value::List(v), "推" | "push") => {
                let mut v2 = v.clone();
                if let Some(a) = args.first() { v2.push(a.clone()); }
                Ok(Value::List(v2))
            }
            // Ok/Err extraction
            (Value::Ok(inner), _) | (Value::Err(inner), _) => Ok(*inner.clone()),
            // Fallback: try calling as a global function name(recv, args)
            _ => {
                let mut all_args = vec![recv];
                all_args.extend(args);
                Err(format!("no method '{method}' on value"))
            }
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

    fn value_to_iter(&self, val: Value) -> Result<Vec<Value>, String> {
        match val {
            Value::List(v)  => Ok(v),
            Value::Str(s)   => Ok(s.chars().map(|c| Value::Str(c.to_string())).collect()),
            Value::Number(n) => Ok((0..n as i64).map(|i| Value::Number(i as f64)).collect()),
            other => Err(format!("cannot iterate over {:?}", other)),
        }
    }

    fn is_truthy(&self, val: &Value) -> bool {
        match val {
            Value::Bool(b)   => *b,
            Value::Unit      => false,
            Value::Number(n) => *n != 0.0,
            Value::Str(s)    => !s.is_empty(),
            Value::List(v)   => !v.is_empty(),
            Value::Ok(_)     => true,
            Value::Err(_)    => false,
            Value::Fn(_, _, _) => true,
        }
    }

    fn to_number(&self, val: &Value) -> Result<f64, String> {
        match val {
            Value::Number(n) => Ok(*n),
            Value::Str(s)    => s.parse().map_err(|_| format!("cannot convert '{s}' to number")),
            other => Err(format!("expected number, got {:?}", other)),
        }
    }

    fn apply_binop(&self, op: &BinOp, l: Value, r: Value) -> Result<Value, String> {
        match op {
            BinOp::Add => match (l, r) {
                (Value::Number(a), Value::Number(b)) => Ok(Value::Number(a + b)),
                (Value::Str(a), Value::Str(b))       => Ok(Value::Str(a + &b)),
                (Value::Str(a), b)                   => Ok(Value::Str(a + &b.to_string())),
                (a, Value::Str(b))                   => Ok(Value::Str(a.to_string() + &b)),
                (a, b) => Err(format!("cannot add {:?} and {:?}", a, b)),
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

    /// Minimal `{}` format string implementation.
    fn builtin_format(&self, args: &[Value]) -> Result<String, String> {
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
                    // Named/formatted placeholder — consume until `}`
                    let mut spec = String::new();
                    for ch in chars.by_ref() {
                        if ch == '}' { break; }
                        spec.push(ch);
                    }
                    if arg_idx < args.len() {
                        // Basic float format: {:.1}
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
            } else if c == ':' && chars.peek() == Some(&'?') {
                chars.next(); // debug fmt — treat same as Display
                if arg_idx < args.len() {
                    result.push_str(&args[arg_idx].to_string());
                    arg_idx += 1;
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
