//! LingDSP: custom effects and instruments written as text in a Ling script and run sample by sample at audio rate.
//!
//! A Ling script (scripts/*.ling) defines `fn <name>_dsp() { return "..." }` that returns a program in this small language:
//!
//! ```text
//! # comment
//! param drive 0.3 0 1          knob: name, default, optional range lo hi (the panel shows 0..1, the program sees lo..hi)
//! state z = 0                  a variable that keeps its value from one sample to the next
//! y = tanh(l * (1 + drive * 30))
//! z = z + (y - z) * 0.2
//! out_l = z
//! out_r = z
//! ```
//! Effects read `l` and `r` (the input) and write `out_l` / `out_r` (or `out` for both; unwritten = the input passes). Instruments read `freq`
//! (Hz), `note` (MIDI key), `vel` (0..1), `gate` (1 while the key is down) and `t` (seconds since the note began) and write `out` (or
//! out_l / out_r). Everywhere: `sr`, `pi`, `tau`. Operators `+ - * / % ^ < > <= >= == != && ||` and unary `-` / `!`.
//! Functions: sin cos tan tanh atan abs sign floor fract sqrt exp log min max clamp pow mix(a,b,t) fold(x) if(c,a,b)
//! and the stateful ones, each call site keeping its own memory: lp(x,hz) hp(x,hz) bp(x,hz,q) delay(x,secs) osc(hz) saw(hz) sqr(hz) tri(hz)
//! noise() env(x,atk,rel) slew(x,secs) adsr(gate,a,d,s,r) lfo(hz) sah(x,trig).
use super::fx::Effect;
use super::Instrument;
use std::collections::HashMap;

const STACK: usize = 48;
const MAX_DELAY_S: f32 = 2.0;
const VOICES: usize = 8;

#[derive(Clone, Debug)]
pub struct ParamSpec {
    pub name: String,
    pub def: f32,
    pub lo: f32,
    pub hi: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum F {
    Sin,
    Cos,
    Tan,
    Tanh,
    Atan,
    Abs,
    Sign,
    Floor,
    Fract,
    Sqrt,
    Exp,
    Log,
    Min,
    Max,
    Clamp,
    Pow,
    Mix,
    Fold,
    If,
    Lp,
    Hp,
    Bp,
    Delay,
    Osc,
    Saw,
    Sqr,
    Tri,
    Noise,
    Env,
    Slew,
    Adsr,
    Lfo,
    Sah,
}

fn func(name: &str) -> Option<(F, usize)> {
    Some(match name {
        "sin" => (F::Sin, 1),
        "cos" => (F::Cos, 1),
        "tan" => (F::Tan, 1),
        "tanh" => (F::Tanh, 1),
        "atan" => (F::Atan, 1),
        "abs" => (F::Abs, 1),
        "sign" => (F::Sign, 1),
        "floor" => (F::Floor, 1),
        "fract" => (F::Fract, 1),
        "sqrt" => (F::Sqrt, 1),
        "exp" => (F::Exp, 1),
        "log" => (F::Log, 1),
        "min" => (F::Min, 2),
        "max" => (F::Max, 2),
        "clamp" => (F::Clamp, 3),
        "pow" => (F::Pow, 2),
        "mix" => (F::Mix, 3),
        "fold" => (F::Fold, 1),
        "if" => (F::If, 3),
        "lp" => (F::Lp, 2),
        "hp" => (F::Hp, 2),
        "bp" => (F::Bp, 3),
        "delay" => (F::Delay, 2),
        "osc" => (F::Osc, 1),
        "saw" => (F::Saw, 1),
        "sqr" => (F::Sqr, 1),
        "tri" => (F::Tri, 1),
        "noise" => (F::Noise, 0),
        "env" => (F::Env, 3),
        "slew" => (F::Slew, 2),
        "adsr" => (F::Adsr, 5),
        "lfo" => (F::Lfo, 1),
        "sah" => (F::Sah, 2),
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug)]
enum Op {
    Const(f32),
    Load(usize),
    Neg,
    Not,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
    Lt,
    Gt,
    Le,
    Ge,
    Eq,
    Ne,
    And,
    Or,
    Call(F, usize), // function, call-site state index
}

#[derive(Clone, Debug)]
struct Stmt {
    target: usize,
    code: Vec<Op>,
}

/// a compiled program (shared by every voice / instance)
#[derive(Clone, Debug)]
pub struct Program {
    pub params: Vec<ParamSpec>,
    /// variable table: params, inputs, states, temps, outputs
    names: Vec<String>,
    stmts: Vec<Stmt>,
    /// initial values of the state variables
    init: Vec<f32>,
    n_sites: usize,
    // well-known slots
    pub s_l: usize,
    pub s_r: usize,
    s_freq: usize,
    s_note: usize,
    s_vel: usize,
    s_gate: usize,
    s_t: usize,
    s_out: usize,
    s_out_l: usize,
    s_out_r: usize,
    first_param: usize,
    wrote_out: bool,
    wrote_out_l: bool,
    wrote_out_r: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f32),
    Id(String),
    Sym(&'static str),
}

fn lex(s: &str) -> Result<Vec<Tok>, String> {
    let cs: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut out = vec![];
    while i < cs.len() {
        let c = cs[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || (c == '.' && cs.get(i + 1).map(|d| d.is_ascii_digit()).unwrap_or(false)) {
            let st = i;
            while i < cs.len() && (cs[i].is_ascii_digit() || cs[i] == '.') {
                i += 1;
            }
            if i < cs.len() && (cs[i] == 'e' || cs[i] == 'E') && cs.get(i + 1).map(|d| d.is_ascii_digit() || *d == '-' || *d == '+').unwrap_or(false) {
                i += 2;
                while i < cs.len() && cs[i].is_ascii_digit() {
                    i += 1;
                }
            }
            let t: String = cs[st..i].iter().collect();
            out.push(Tok::Num(t.parse().map_err(|_| format!("bad number '{t}'"))?));
        } else if c.is_alphabetic() || c == '_' {
            let st = i;
            while i < cs.len() && (cs[i].is_alphanumeric() || cs[i] == '_') {
                i += 1;
            }
            out.push(Tok::Id(cs[st..i].iter().collect()));
        } else {
            let two: String = cs[i..(i + 2).min(cs.len())].iter().collect();
            let sym: &'static str = match two.as_str() {
                "<=" => "<=",
                ">=" => ">=",
                "==" => "==",
                "!=" => "!=",
                "&&" => "&&",
                "||" => "||",
                _ => match c {
                    '+' => "+",
                    '-' => "-",
                    '*' => "*",
                    '/' => "/",
                    '%' => "%",
                    '^' => "^",
                    '<' => "<",
                    '>' => ">",
                    '(' => "(",
                    ')' => ")",
                    ',' => ",",
                    '=' => "=",
                    '!' => "!",
                    _ => return Err(format!("unexpected character '{c}'")),
                },
            };
            i += sym.len();
            out.push(Tok::Sym(sym));
        }
    }
    Ok(out)
}

struct Parser<'a> {
    t: &'a [Tok],
    p: usize,
    vars: &'a HashMap<String, usize>,
    code: Vec<Op>,
    sites: &'a mut usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.p)
    }
    fn eat(&mut self, s: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Sym(x)) if *x == s) {
            self.p += 1;
            true
        } else {
            false
        }
    }
    fn expr(&mut self) -> Result<(), String> {
        self.or()
    }
    fn or(&mut self) -> Result<(), String> {
        self.and()?;
        while self.eat("||") {
            self.and()?;
            self.code.push(Op::Or);
        }
        Ok(())
    }
    fn and(&mut self) -> Result<(), String> {
        self.cmp()?;
        while self.eat("&&") {
            self.cmp()?;
            self.code.push(Op::And);
        }
        Ok(())
    }
    fn cmp(&mut self) -> Result<(), String> {
        self.add()?;
        loop {
            let op = if self.eat("<=") {
                Op::Le
            } else if self.eat(">=") {
                Op::Ge
            } else if self.eat("==") {
                Op::Eq
            } else if self.eat("!=") {
                Op::Ne
            } else if self.eat("<") {
                Op::Lt
            } else if self.eat(">") {
                Op::Gt
            } else {
                break;
            };
            self.add()?;
            self.code.push(op);
        }
        Ok(())
    }
    fn add(&mut self) -> Result<(), String> {
        self.mul()?;
        loop {
            if self.eat("+") {
                self.mul()?;
                self.code.push(Op::Add);
            } else if self.eat("-") {
                self.mul()?;
                self.code.push(Op::Sub);
            } else {
                break;
            }
        }
        Ok(())
    }
    fn mul(&mut self) -> Result<(), String> {
        self.unary()?;
        loop {
            if self.eat("*") {
                self.unary()?;
                self.code.push(Op::Mul);
            } else if self.eat("/") {
                self.unary()?;
                self.code.push(Op::Div);
            } else if self.eat("%") {
                self.unary()?;
                self.code.push(Op::Rem);
            } else {
                break;
            }
        }
        Ok(())
    }
    fn unary(&mut self) -> Result<(), String> {
        if self.eat("-") {
            self.unary()?;
            self.code.push(Op::Neg);
            return Ok(());
        }
        if self.eat("!") {
            self.unary()?;
            self.code.push(Op::Not);
            return Ok(());
        }
        self.power()
    }
    fn power(&mut self) -> Result<(), String> {
        self.atom()?;
        if self.eat("^") {
            self.unary()?; // right associative
            self.code.push(Op::Pow);
        }
        Ok(())
    }
    fn atom(&mut self) -> Result<(), String> {
        match self.peek().cloned() {
            Some(Tok::Num(n)) => {
                self.p += 1;
                self.code.push(Op::Const(n));
                Ok(())
            }
            Some(Tok::Sym("(")) => {
                self.p += 1;
                self.expr()?;
                if !self.eat(")") {
                    return Err("missing )".into());
                }
                Ok(())
            }
            Some(Tok::Id(name)) => {
                self.p += 1;
                if self.eat("(") {
                    let Some((f, argc)) = func(&name) else { return Err(format!("unknown function '{name}'")) };
                    let mut n = 0;
                    if !self.eat(")") {
                        loop {
                            self.expr()?;
                            n += 1;
                            if self.eat(")") {
                                break;
                            }
                            if !self.eat(",") {
                                return Err(format!("expected , or ) in {name}(...)"));
                            }
                        }
                    }
                    if n != argc {
                        return Err(format!("{name}() takes {argc} argument(s), not {n}"));
                    }
                    let site = *self.sites;
                    *self.sites += 1;
                    self.code.push(Op::Call(f, site));
                    Ok(())
                } else {
                    match name.as_str() {
                        "pi" => self.code.push(Op::Const(std::f32::consts::PI)),
                        "tau" => self.code.push(Op::Const(std::f32::consts::TAU)),
                        _ => {
                            let Some(i) = self.vars.get(&name) else { return Err(format!("unknown name '{name}' (assign it first)")) };
                            self.code.push(Op::Load(*i));
                        }
                    }
                    Ok(())
                }
            }
            other => Err(format!("unexpected {other:?}")),
        }
    }
}

pub fn compile(src: &str, instrument: bool) -> Result<Program, String> {
    let mut names: Vec<String> = vec![];
    let mut vars: HashMap<String, usize> = HashMap::new();
    let mut init: Vec<f32> = vec![];
    let mut declare = |n: &str, v: f32, names: &mut Vec<String>, init: &mut Vec<f32>, vars: &mut HashMap<String, usize>| -> usize {
        if let Some(i) = vars.get(n) {
            return *i;
        }
        names.push(n.to_string());
        init.push(v);
        vars.insert(n.to_string(), names.len() - 1);
        names.len() - 1
    };
    let s_l = declare("l", 0.0, &mut names, &mut init, &mut vars);
    let s_r = declare("r", 0.0, &mut names, &mut init, &mut vars);
    let s_freq = declare("freq", 440.0, &mut names, &mut init, &mut vars);
    let s_note = declare("note", 69.0, &mut names, &mut init, &mut vars);
    let s_vel = declare("vel", 1.0, &mut names, &mut init, &mut vars);
    let s_gate = declare("gate", 0.0, &mut names, &mut init, &mut vars);
    let s_t = declare("t", 0.0, &mut names, &mut init, &mut vars);
    declare("sr", 44100.0, &mut names, &mut init, &mut vars);
    let s_out = declare("out", 0.0, &mut names, &mut init, &mut vars);
    let s_out_l = declare("out_l", 0.0, &mut names, &mut init, &mut vars);
    let s_out_r = declare("out_r", 0.0, &mut names, &mut init, &mut vars);
    let first_param = names.len();
    // pass 1: params, so they are known everywhere
    let mut params: Vec<ParamSpec> = vec![];
    let lines: Vec<&str> = src.lines().collect();
    for (ln, raw) in lines.iter().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        let toks = lex(line).map_err(|e| format!("line {}: {e}", ln + 1))?;
        if let Some(Tok::Id(k)) = toks.first() {
            if k == "param" {
                let Some(Tok::Id(name)) = toks.get(1) else { return Err(format!("line {}: param needs a name", ln + 1)) };
                let num = |i: usize| -> Option<f32> {
                    match toks.get(i) {
                        Some(Tok::Num(n)) => Some(*n),
                        Some(Tok::Sym("-")) => match toks.get(i + 1) {
                            Some(Tok::Num(n)) => Some(-*n),
                            _ => None,
                        },
                        _ => None,
                    }
                };
                // numbers may carry a leading minus: collect them in order
                let mut nums: Vec<f32> = vec![];
                let mut i = 2;
                while i < toks.len() {
                    match (&toks[i], toks.get(i + 1)) {
                        (Tok::Num(n), _) => {
                            nums.push(*n);
                            i += 1;
                        }
                        (Tok::Sym("-"), Some(Tok::Num(n))) => {
                            nums.push(-*n);
                            i += 2;
                        }
                        _ => return Err(format!("line {}: param {name} wants numbers: param name default [lo hi]", ln + 1)),
                    }
                }
                let _ = num;
                if nums.is_empty() {
                    return Err(format!("line {}: param {name} needs a default value", ln + 1));
                }
                let (lo, hi) = if nums.len() >= 3 { (nums[1], nums[2]) } else { (0.0, 1.0) };
                if hi <= lo {
                    return Err(format!("line {}: param {name}: the range must go up", ln + 1));
                }
                if vars.contains_key(name) {
                    return Err(format!("line {}: '{name}' is already a name", ln + 1));
                }
                let def = nums[0].clamp(lo, hi);
                declare(name, def, &mut names, &mut init, &mut vars);
                params.push(ParamSpec { name: name.clone(), def, lo, hi });
            }
        }
    }
    // pass 2: states and statements
    let mut stmts: Vec<Stmt> = vec![];
    let mut sites = 0usize;
    let (mut wrote_out, mut wrote_l, mut wrote_r) = (false, false, false);
    for (ln, raw) in lines.iter().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let toks = lex(line).map_err(|e| format!("line {}: {e}", ln + 1))?;
        let at = |e: String| format!("line {}: {e}", ln + 1);
        match toks.first() {
            Some(Tok::Id(k)) if k == "param" => continue,
            Some(Tok::Id(k)) if k == "state" => {
                let Some(Tok::Id(name)) = toks.get(1) else { return Err(at("state needs a name".into())) };
                if !matches!(toks.get(2), Some(Tok::Sym("="))) {
                    return Err(at(format!("state {name} = <number>")));
                }
                let v = match (toks.get(3), toks.get(4)) {
                    (Some(Tok::Num(n)), None) => *n,
                    (Some(Tok::Sym("-")), Some(Tok::Num(n))) => -*n,
                    _ => return Err(at(format!("state {name} = <number>"))),
                };
                if vars.contains_key(name) {
                    return Err(at(format!("'{name}' is already a name")));
                }
                declare(name, v, &mut names, &mut init, &mut vars);
            }
            Some(Tok::Id(name)) if matches!(toks.get(1), Some(Tok::Sym("="))) => {
                // the target exists before its expression is read, so `z = z + 1` on a state works and a temp may feed later lines
                let existed = vars.contains_key(name);
                let mut sites_now = sites;
                let mut p = Parser { t: &toks[2..], p: 0, vars: &vars, code: vec![], sites: &mut sites_now };
                p.expr().map_err(at)?;
                if p.p != p.t.len() {
                    return Err(at("unexpected text after the expression".into()));
                }
                let code = std::mem::take(&mut p.code);
                sites = sites_now;
                let target = if existed { vars[name] } else { declare(name, 0.0, &mut names, &mut init, &mut vars) };
                if target < first_param && !matches!(name.as_str(), "out" | "out_l" | "out_r") {
                    return Err(at(format!("'{name}' is an input: it cannot be assigned")));
                }
                match name.as_str() {
                    "out" => wrote_out = true,
                    "out_l" => wrote_l = true,
                    "out_r" => wrote_r = true,
                    _ => {}
                }
                if params.iter().any(|p| p.name == *name) {
                    return Err(at(format!("'{name}' is a knob: it cannot be assigned")));
                }
                stmts.push(Stmt { target, code });
            }
            _ => return Err(at("expected  name = expression,  param ...  or  state ...".into())),
        }
    }
    if !(wrote_out || wrote_l || wrote_r) && instrument {
        return Err("an instrument must write out (or out_l / out_r)".into());
    }
    Ok(Program { params, names, stmts, init, n_sites: sites, s_l, s_r, s_freq, s_note, s_vel, s_gate, s_t, s_out, s_out_l, s_out_r, first_param, wrote_out, wrote_out_l: wrote_l, wrote_out_r: wrote_r })
}

/// the memory of one call site
#[derive(Clone, Default)]
struct Site {
    a: f32,
    b: f32,
    c: f32,
    buf: Vec<f32>,
    pos: usize,
}

struct Run {
    vars: Vec<f32>,
    sites: Vec<Site>,
    noise: u32,
}

impl Run {
    fn new(p: &Program, sr: f32) -> Run {
        let mut sites = vec![Site::default(); p.n_sites];
        // delay lines are allocated up front, only where a delay() is called
        for st in &p.stmts {
            for op in &st.code {
                if let Op::Call(F::Delay, i) = op {
                    sites[*i].buf = vec![0.0; (MAX_DELAY_S * sr) as usize + 4];
                }
            }
        }
        let mut vars = p.init.clone();
        vars[p.names.iter().position(|n| n == "sr").unwrap_or(0)] = sr;
        Run { vars, sites, noise: 0x1234_5678 }
    }
}

#[inline]
fn call(f: F, st: &mut Site, a: &[f32], sr: f32, noise: &mut u32) -> f32 {
    let dt = 1.0 / sr;
    match f {
        F::Sin => a[0].sin(),
        F::Cos => a[0].cos(),
        F::Tan => a[0].tan().clamp(-1e4, 1e4),
        F::Tanh => a[0].tanh(),
        F::Atan => a[0].atan(),
        F::Abs => a[0].abs(),
        F::Sign => {
            if a[0] > 0.0 {
                1.0
            } else if a[0] < 0.0 {
                -1.0
            } else {
                0.0
            }
        }
        F::Floor => a[0].floor(),
        F::Fract => a[0] - a[0].floor(),
        F::Sqrt => a[0].max(0.0).sqrt(),
        F::Exp => a[0].clamp(-60.0, 60.0).exp(),
        F::Log => a[0].max(1e-20).ln(),
        F::Min => a[0].min(a[1]),
        F::Max => a[0].max(a[1]),
        F::Clamp => a[0].clamp(a[1].min(a[2]), a[1].max(a[2])),
        F::Pow => {
            let r = a[0].powf(a[1]);
            if r.is_finite() { r } else { 0.0 }
        }
        F::Mix => a[0] + (a[1] - a[0]) * a[2],
        F::Fold => {
            // triangle fold into -1..1
            let x = a[0] * 0.25 + 0.25;
            let f = x - x.floor();
            ((f - 0.5).abs() * 4.0 - 1.0).clamp(-1.0, 1.0) * -1.0
        }
        F::If => {
            if a[0] != 0.0 {
                a[1]
            } else {
                a[2]
            }
        }
        F::Lp => {
            let g = 1.0 - (-std::f32::consts::TAU * a[1].clamp(1.0, sr * 0.45) * dt).exp();
            st.a += (a[0] - st.a) * g;
            st.a
        }
        F::Hp => {
            let g = 1.0 - (-std::f32::consts::TAU * a[1].clamp(1.0, sr * 0.45) * dt).exp();
            st.a += (a[0] - st.a) * g;
            a[0] - st.a
        }
        F::Bp => {
            // state variable filter (Chamberlin, two steps for stability)
            let f = (std::f32::consts::PI * a[1].clamp(20.0, sr * 0.3) * dt).sin() * 2.0;
            let q = 1.0 / a[2].clamp(0.5, 30.0);
            for _ in 0..2 {
                st.a += f * st.b; // low-pass
                let high = a[0] - st.a - q * st.b;
                st.b += f * high; // band-pass
            }
            st.b
        }
        F::Delay => {
            let n = st.buf.len();
            if n < 8 {
                return a[0];
            }
            let d = (a[1].clamp(0.0, MAX_DELAY_S) * sr).clamp(1.0, (n - 3) as f32);
            let r = (st.pos as f32 - d).rem_euclid(n as f32);
            let i0 = r.floor() as usize % n;
            let fr = r - r.floor();
            let (x0, x1) = (st.buf[i0], st.buf[(i0 + 1) % n]);
            let out = x0 + (x1 - x0) * fr;
            st.buf[st.pos] = a[0];
            st.pos = (st.pos + 1) % n;
            out
        }
        F::Osc | F::Saw | F::Sqr | F::Tri | F::Lfo => {
            st.a = (st.a + a[0].clamp(0.0, sr * 0.5) * dt).fract();
            let p = st.a;
            match f {
                F::Osc => (p * std::f32::consts::TAU).sin(),
                F::Saw => 2.0 * p - 1.0,
                F::Sqr => {
                    if p < 0.5 {
                        1.0
                    } else {
                        -1.0
                    }
                }
                F::Tri => 4.0 * (p - 0.5).abs() - 1.0,
                _ => 0.5 + 0.5 * (p * std::f32::consts::TAU).sin(),
            }
        }
        F::Noise => {
            let mut x = *noise;
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            *noise = x;
            (x as f32 / u32::MAX as f32) * 2.0 - 1.0
        }
        F::Env => {
            let x = a[0].abs();
            let c = if x > st.a { 1.0 - (-dt / a[1].max(1e-4)).exp() } else { 1.0 - (-dt / a[2].max(1e-4)).exp() };
            st.a += (x - st.a) * c;
            st.a
        }
        F::Slew => {
            let c = 1.0 - (-dt / a[1].max(1e-5)).exp();
            st.a += (a[0] - st.a) * c;
            st.a
        }
        F::Adsr => {
            // st.b: stage 0 idle 1 attack 2 decay 3 sustain 4 release; st.a the level
            let gate = a[0] > 0.5;
            if gate && st.b == 0.0 {
                st.b = 1.0;
            }
            if !gate && st.b > 0.5 && st.b < 3.5 {
                st.b = 4.0;
            }
            if gate && st.b == 4.0 {
                st.b = 1.0;
            }
            match st.b as i32 {
                1 => {
                    st.a += dt / a[1].max(1e-4);
                    if st.a >= 1.0 {
                        st.a = 1.0;
                        st.b = 2.0;
                    }
                }
                2 => {
                    st.a -= dt * (1.0 - a[3].clamp(0.0, 1.0)) / a[2].max(1e-4);
                    if st.a <= a[3] {
                        st.a = a[3].clamp(0.0, 1.0);
                        st.b = 3.0;
                    }
                }
                3 => st.a = a[3].clamp(0.0, 1.0),
                4 => {
                    st.a -= dt / a[4].max(1e-4) * st.a.max(0.02);
                    if st.a <= 1e-4 {
                        st.a = 0.0;
                        st.b = 0.0;
                    }
                }
                _ => st.a = 0.0,
            }
            st.a
        }
        F::Sah => {
            if a[1] > 0.5 && st.b <= 0.5 {
                st.a = a[0];
            }
            st.b = a[1];
            st.a
        }
    }
}

#[inline]
fn eval(p: &Program, r: &mut Run, sr: f32) {
    let mut stack = [0.0f32; STACK];
    for st in &p.stmts {
        let mut sp = 0usize;
        for op in &st.code {
            match *op {
                Op::Const(c) => {
                    stack[sp] = c;
                    sp += 1;
                }
                Op::Load(i) => {
                    stack[sp] = r.vars[i];
                    sp += 1;
                }
                Op::Neg => stack[sp - 1] = -stack[sp - 1],
                Op::Not => stack[sp - 1] = if stack[sp - 1] == 0.0 { 1.0 } else { 0.0 },
                Op::Call(f, site) => {
                    let argc = func_argc(f);
                    let base = sp - argc;
                    let mut args = [0.0f32; 5];
                    args[..argc].copy_from_slice(&stack[base..sp]);
                    let v = call(f, &mut r.sites[site], &args[..argc], sr, &mut r.noise);
                    sp = base;
                    stack[sp] = v;
                    sp += 1;
                }
                bin => {
                    sp -= 1;
                    let (a, b) = (stack[sp - 1], stack[sp]);
                    stack[sp - 1] = match bin {
                        Op::Add => a + b,
                        Op::Sub => a - b,
                        Op::Mul => a * b,
                        Op::Div => {
                            if b.abs() < 1e-20 {
                                0.0
                            } else {
                                a / b
                            }
                        }
                        Op::Rem => {
                            if b.abs() < 1e-20 {
                                0.0
                            } else {
                                a % b
                            }
                        }
                        Op::Pow => {
                            let v = a.powf(b);
                            if v.is_finite() { v } else { 0.0 }
                        }
                        Op::Lt => (a < b) as u8 as f32,
                        Op::Gt => (a > b) as u8 as f32,
                        Op::Le => (a <= b) as u8 as f32,
                        Op::Ge => (a >= b) as u8 as f32,
                        Op::Eq => (a == b) as u8 as f32,
                        Op::Ne => (a != b) as u8 as f32,
                        Op::And => ((a != 0.0) && (b != 0.0)) as u8 as f32,
                        Op::Or => ((a != 0.0) || (b != 0.0)) as u8 as f32,
                        _ => 0.0,
                    };
                }
            }
        }
        let v = if sp > 0 { stack[sp - 1] } else { 0.0 };
        r.vars[st.target] = if v.is_finite() { v.clamp(-1e6, 1e6) } else { 0.0 };
    }
}

fn func_argc(f: F) -> usize {
    match f {
        F::Noise => 0,
        F::Sin | F::Cos | F::Tan | F::Tanh | F::Atan | F::Abs | F::Sign | F::Floor | F::Fract | F::Sqrt | F::Exp | F::Log | F::Fold | F::Osc | F::Saw | F::Sqr | F::Tri | F::Lfo => 1,
        F::Min | F::Max | F::Pow | F::Lp | F::Hp | F::Delay | F::Slew | F::Sah => 2,
        F::Clamp | F::Mix | F::If | F::Bp | F::Env => 3,
        F::Adsr => 5,
    }
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

fn norm_to_value(spec: &ParamSpec, n: f32) -> f32 {
    spec.lo + (spec.hi - spec.lo) * n.clamp(0.0, 1.0)
}
fn value_to_norm(spec: &ParamSpec, v: f32) -> f32 {
    ((v - spec.lo) / (spec.hi - spec.lo)).clamp(0.0, 1.0)
}

/// the knob list of a program as (name, default 0..1), with the names leaked so the rest of the DAW can treat them as static
pub fn param_defaults(p: &Program) -> Vec<(&'static str, f32)> {
    p.params.iter().map(|s| (leak(&s.name), value_to_norm(s, s.def))).collect()
}

// ── the effect ────────────────────────────────────────────────────────────────

pub struct ScriptFx {
    sr: f32,
    prog: Option<Program>,
    run: Option<Run>,
    /// knob values 0..1 in program order
    knobs: Vec<f32>,
    src: String,
    path: String,
    pub error: String,
    label: &'static str,
}

impl ScriptFx {
    pub fn new(sr: f32) -> ScriptFx {
        ScriptFx { sr, prog: None, run: None, knobs: vec![], src: String::new(), path: String::new(), error: String::new(), label: "Script FX" }
    }
    pub fn from_source(sr: f32, src: &str, path: &str) -> Result<ScriptFx, String> {
        let mut f = ScriptFx::new(sr);
        f.path = path.to_string();
        f.load(src)?;
        Ok(f)
    }
    /// (re)compile; the knobs keep their values by name when the new program has the same ones
    pub fn load(&mut self, src: &str) -> Result<(), String> {
        match compile(src, false) {
            Ok(p) => {
                let old: HashMap<String, f32> = self.prog.as_ref().map(|o| o.params.iter().zip(&self.knobs).map(|(s, v)| (s.name.clone(), *v)).collect()).unwrap_or_default();
                self.knobs = p.params.iter().map(|s| old.get(&s.name).copied().unwrap_or(value_to_norm(s, s.def))).collect();
                self.run = Some(Run::new(&p, self.sr));
                self.prog = Some(p);
                self.src = src.to_string();
                self.error.clear();
                Ok(())
            }
            Err(e) => {
                self.error = e.clone();
                Err(e)
            }
        }
    }
    pub fn defaults(&self) -> Vec<(&'static str, f32)> {
        self.prog.as_ref().map(param_defaults).unwrap_or_default()
    }
    pub fn set_label(&mut self, name: &str) {
        self.label = leak(name);
    }
}

impl Effect for ScriptFx {
    fn process(&mut self, buf: &mut [f32]) {
        let (Some(p), Some(r)) = (self.prog.as_ref(), self.run.as_mut()) else { return };
        for (i, spec) in p.params.iter().enumerate() {
            r.vars[p.first_param + i] = norm_to_value(spec, self.knobs.get(i).copied().unwrap_or(0.0));
        }
        let sr = self.sr;
        for fr in buf.chunks_exact_mut(2) {
            r.vars[p.s_l] = fr[0];
            r.vars[p.s_r] = fr[1];
            // unwritten outputs pass the input through
            r.vars[p.s_out] = 0.0;
            r.vars[p.s_out_l] = fr[0];
            r.vars[p.s_out_r] = fr[1];
            eval(p, r, sr);
            let (ol, or) = if p.wrote_out && !p.wrote_out_l && !p.wrote_out_r {
                (r.vars[p.s_out], r.vars[p.s_out])
            } else {
                (r.vars[p.s_out_l], r.vars[p.s_out_r])
            };
            fr[0] = ol;
            fr[1] = or;
        }
    }
    fn set_param(&mut self, name: &str, v: f32) {
        if let Some(p) = &self.prog {
            if let Some(i) = p.params.iter().position(|s| s.name == name) {
                self.knobs[i] = v.clamp(0.0, 1.0);
            }
        }
    }
    fn name(&self) -> &'static str {
        self.label
    }
    fn command(&mut self, cmd: &str, _args: &[f64], text: &str) -> Vec<f64> {
        match cmd {
            "load_source" | "load_config" => {
                let (src, path) = if cmd == "load_config" {
                    match serde_json::from_str::<serde_json::Value>(text) {
                        Ok(v) => (v["source"].as_str().unwrap_or("").to_string(), v["path"].as_str().unwrap_or("").to_string()),
                        Err(_) => return vec![],
                    }
                } else {
                    (text.to_string(), self.path.clone())
                };
                self.path = path;
                match self.load(&src) {
                    Ok(()) => vec![1.0],
                    Err(_) => vec![],
                }
            }
            _ => vec![],
        }
    }
    fn info(&self, what: &str) -> String {
        match what {
            "config" => serde_json::json!({ "path": self.path, "source": self.src }).to_string(),
            "error" => self.error.clone(),
            "path" => self.path.clone(),
            "params" => {
                let v: Vec<serde_json::Value> = self.prog.as_ref().map(|p| p.params.iter().zip(&self.knobs).map(|(s, k)| serde_json::json!([s.name, k])).collect()).unwrap_or_default();
                serde_json::Value::Array(v).to_string()
            }
            _ => String::new(),
        }
    }
}

// ── the instrument ────────────────────────────────────────────────────────────

struct Voice {
    run: Run,
    key: u8,
    vel: f32,
    gate: bool,
    t: f32,
    since_off: f32,
    active: bool,
    quiet: f32,
    age: u64,
}

pub struct ScriptSynth {
    sr: f32,
    prog: Option<Program>,
    voices: Vec<Voice>,
    knobs: Vec<f32>,
    src: String,
    path: String,
    pub error: String,
    clock: u64,
}

impl ScriptSynth {
    pub fn new(sr: f32) -> ScriptSynth {
        ScriptSynth { sr, prog: None, voices: vec![], knobs: vec![], src: String::new(), path: String::new(), error: String::new(), clock: 0 }
    }
    pub fn from_config(sr: f32, config: &str) -> Result<ScriptSynth, String> {
        let v: serde_json::Value = serde_json::from_str(config).map_err(|e| e.to_string())?;
        let mut s = ScriptSynth::new(sr);
        s.path = v["path"].as_str().unwrap_or("").to_string();
        s.load(v["source"].as_str().unwrap_or(""))?;
        if let Some(obj) = v["knobs"].as_object() {
            for (k, x) in obj {
                if let Some(x) = x.as_f64() {
                    s.set_param(k, x as f32);
                }
            }
        }
        Ok(s)
    }
    pub fn load(&mut self, src: &str) -> Result<(), String> {
        match compile(src, true) {
            Ok(p) => {
                let old: HashMap<String, f32> = self.prog.as_ref().map(|o| o.params.iter().zip(&self.knobs).map(|(s, v)| (s.name.clone(), *v)).collect()).unwrap_or_default();
                self.knobs = p.params.iter().map(|s| old.get(&s.name).copied().unwrap_or(value_to_norm(s, s.def))).collect();
                self.voices = (0..VOICES).map(|_| Voice { run: Run::new(&p, self.sr), key: 0, vel: 0.0, gate: false, t: 0.0, since_off: 0.0, active: false, quiet: 0.0, age: 0 }).collect();
                self.prog = Some(p);
                self.src = src.to_string();
                self.error.clear();
                Ok(())
            }
            Err(e) => {
                self.error = e.clone();
                Err(e)
            }
        }
    }
    pub fn defaults(&self) -> Vec<(&'static str, f32)> {
        self.prog.as_ref().map(param_defaults).unwrap_or_default()
    }
}

impl Instrument for ScriptSynth {
    fn note_on(&mut self, key: u8, vel: f32) {
        let Some(p) = self.prog.as_ref() else { return };
        self.clock += 1;
        // a free voice, else the quietest / oldest
        let vi = self.voices.iter().position(|v| !v.active).unwrap_or_else(|| self.voices.iter().enumerate().min_by_key(|(_, v)| v.age).map(|(i, _)| i).unwrap_or(0));
        let sr = self.sr;
        let v = &mut self.voices[vi];
        v.run = Run::new(p, sr);
        v.key = key;
        v.vel = vel;
        v.gate = true;
        v.t = 0.0;
        v.since_off = 0.0;
        v.active = true;
        v.quiet = 0.0;
        v.age = self.clock;
    }
    fn note_off(&mut self, key: u8) {
        for v in self.voices.iter_mut().filter(|v| v.active && v.gate && v.key == key) {
            v.gate = false;
        }
    }
    fn all_off(&mut self) {
        for v in self.voices.iter_mut() {
            v.active = false;
            v.gate = false;
        }
    }
    fn render(&mut self, out: &mut [f32]) {
        let Some(p) = self.prog.as_ref() else { return };
        let sr = self.sr;
        let dt = 1.0 / sr;
        for v in self.voices.iter_mut().filter(|v| v.active) {
            for (i, spec) in p.params.iter().enumerate() {
                v.run.vars[p.first_param + i] = norm_to_value(spec, self.knobs.get(i).copied().unwrap_or(0.0));
            }
            let freq = 440.0 * 2f32.powf((v.key as f32 - 69.0) / 12.0);
            for fr in out.chunks_exact_mut(2) {
                v.run.vars[p.s_freq] = freq;
                v.run.vars[p.s_note] = v.key as f32;
                v.run.vars[p.s_vel] = v.vel;
                v.run.vars[p.s_gate] = if v.gate { 1.0 } else { 0.0 };
                v.run.vars[p.s_t] = v.t;
                v.run.vars[p.s_out] = 0.0;
                v.run.vars[p.s_out_l] = 0.0;
                v.run.vars[p.s_out_r] = 0.0;
                eval(p, &mut v.run, sr);
                let (l, r) = if p.wrote_out && !p.wrote_out_l && !p.wrote_out_r { (v.run.vars[p.s_out], v.run.vars[p.s_out]) } else { (v.run.vars[p.s_out_l] + v.run.vars[p.s_out], v.run.vars[p.s_out_r] + v.run.vars[p.s_out]) };
                let (l, r) = (l * 0.5, r * 0.5);
                fr[0] += l;
                fr[1] += r;
                v.t += dt;
                if !v.gate {
                    v.since_off += dt;
                    if l.abs().max(r.abs()) < 1e-5 {
                        v.quiet += dt;
                    } else {
                        v.quiet = 0.0;
                    }
                }
            }
            // a voice ends when it has been silent for a while after the key came up, or after 12 s of release
            if !v.gate && (v.quiet > 0.25 || v.since_off > 12.0) {
                v.active = false;
            }
        }
    }
    fn params(&self) -> Vec<(&'static str, f32)> {
        self.prog.as_ref().map(|p| p.params.iter().zip(&self.knobs).map(|(s, v)| (leak(&s.name), *v)).collect()).unwrap_or_default()
    }
    fn set_param(&mut self, name: &str, v: f32) {
        if let Some(p) = &self.prog {
            if let Some(i) = p.params.iter().position(|s| s.name == name) {
                self.knobs[i] = v.clamp(0.0, 1.0);
            }
        }
    }
    fn name(&self) -> &'static str {
        "Script Synth"
    }
    fn command(&mut self, cmd: &str, _args: &[f64], text: &str) -> Vec<f64> {
        match cmd {
            "load_source" => {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
                    self.path = v["path"].as_str().unwrap_or(&self.path).to_string();
                    return match self.load(v["source"].as_str().unwrap_or("")) {
                        Ok(()) => vec![1.0],
                        Err(_) => vec![],
                    };
                }
                match self.load(text) {
                    Ok(()) => vec![1.0],
                    Err(_) => vec![],
                }
            }
            _ => vec![],
        }
    }
    fn info(&self, what: &str) -> String {
        match what {
            "config" => {
                let knobs: serde_json::Map<String, serde_json::Value> = self.params().into_iter().map(|(n, v)| (n.to_string(), serde_json::json!(v))).collect();
                serde_json::json!({ "path": self.path, "source": self.src, "knobs": knobs }).to_string()
            }
            "error" => self.error.clone(),
            "path" => self.path.clone(),
            _ => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_fx(src: &str, input: &[f32]) -> Vec<f32> {
        let mut f = ScriptFx::from_source(48000.0, src, "").expect("compiles");
        let mut buf: Vec<f32> = input.iter().flat_map(|x| [*x, *x]).collect();
        f.process(&mut buf);
        buf.chunks(2).map(|c| c[0]).collect()
    }

    #[test]
    fn passthrough_and_gain() {
        let y = run_fx("param g 1 0 4\nout = l * g", &[0.5, -0.25, 1.0]);
        assert_eq!(y, vec![0.5, -0.25, 1.0]);
        let y = run_fx("param g 2 0 4\nout_l = l * g\nout_r = r * g", &[0.5]);
        assert_eq!(y, vec![1.0]);
        // nothing written: the input passes
        assert_eq!(run_fx("x = l * 2", &[0.3]), vec![0.3]);
    }

    #[test]
    fn precedence_functions_and_state() {
        let y = run_fx("out = 1 + 2 * 3 - 4 / 2 + 2 ^ 3 ^ 0", &[0.0]);
        assert!((y[0] - (1.0 + 6.0 - 2.0 + 2.0)).abs() < 1e-5, "{y:?}");
        let y = run_fx("state n = 0\nn = n + 1\nout = n", &[0.0, 0.0, 0.0]);
        assert_eq!(y, vec![1.0, 2.0, 3.0]);
        let y = run_fx("out = if(l > 0.5, 1, -1) + clamp(l * 10, 0, 2) + mix(0, 10, 0.25)", &[0.7]);
        assert!((y[0] - (1.0 + 2.0 + 2.5)).abs() < 1e-5);
    }

    #[test]
    fn filters_oscillators_delay() {
        // a low-pass at 100 Hz keeps DC and removes a 10 kHz tone
        let sr = 48000.0;
        let tone: Vec<f32> = (0..4800).map(|i| (std::f32::consts::TAU * 10000.0 * i as f32 / sr).sin()).collect();
        let y = run_fx("out = lp(l, 100)", &tone);
        let peak = y[2400..].iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(peak < 0.05, "high tone through a low-pass: {peak}");
        // delay of 0.01 s = 480 samples
        let mut imp = vec![0.0f32; 1000];
        imp[0] = 1.0;
        let y = run_fx("out = delay(l, 0.01)", &imp);
        let k = y.iter().position(|v| v.abs() > 0.5).unwrap();
        assert!((k as i32 - 480).abs() <= 1, "delay at {k}");
        // an oscillator at 440 Hz crosses zero ~880 times a second
        let y = run_fx("out = osc(440)", &vec![0.0; 48000]);
        let zc = y.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        assert!((zc as i32 - 440).abs() <= 2, "zero crossings {zc}");
    }

    #[test]
    fn errors_are_reported_with_a_line() {
        assert!(compile("out = l *\n", false).unwrap_err().contains("line 1"));
        assert!(compile("a = 1\nb = nosuch(2)", false).unwrap_err().contains("line 2"));
        assert!(compile("l = 3", false).unwrap_err().contains("input"));
        assert!(compile("out = foo", false).unwrap_err().contains("unknown name"));
        assert!(compile("x = 1", true).unwrap_err().contains("write out"));
        assert!(compile("out = sin(1, 2)", false).unwrap_err().contains("argument"));
    }

    #[test]
    fn knobs_map_to_ranges_and_keep_values_on_reload() {
        let mut f = ScriptFx::from_source(48000.0, "param g 1 0 4\nout = l * g", "").unwrap();
        f.set_param("g", 0.5); // 2.0
        let mut buf = vec![0.5f32, 0.5];
        f.process(&mut buf);
        assert!((buf[0] - 1.0).abs() < 1e-6);
        f.load("param g 1 0 4\nparam h 0 0 1\nout = l * g").unwrap();
        let mut buf = vec![0.5f32, 0.5];
        f.process(&mut buf);
        assert!((buf[0] - 1.0).abs() < 1e-6, "the knob kept its value across a reload");
        // the config round-trips
        let cfg = f.info("config");
        let mut g = ScriptFx::new(48000.0);
        assert_eq!(g.command("load_config", &[], &cfg), vec![1.0]);
    }

    #[test]
    fn synth_plays_notes_and_dies_after_release() {
        let src = "param rel 0.1 0.01 1\nenv_ = adsr(gate, 0.005, 0.1, 0.7, rel)\nout = saw(freq) * env_ * vel";
        let mut s = ScriptSynth::new(48000.0);
        s.load(src).unwrap();
        s.note_on(57, 1.0);
        let mut buf = vec![0.0f32; 2 * 4800];
        s.render(&mut buf);
        let peak = buf.iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(peak > 0.05 && peak.is_finite(), "sound {peak}");
        // pitch: a saw at 220 Hz wraps 220 times a second
        let mut s2 = ScriptSynth::new(48000.0);
        s2.load("out = saw(freq) * 0.5").unwrap();
        s2.note_on(57, 1.0);
        let mut b2 = vec![0.0f32; 2 * 48000];
        s2.render(&mut b2);
        let wraps = b2.chunks(2).map(|c| c[0]).collect::<Vec<_>>().windows(2).filter(|w| w[1] < w[0] - 0.3).count();
        assert!((wraps as i32 - 220).abs() <= 3, "saw wraps {wraps}");
        s.note_off(57);
        for _ in 0..40 {
            let mut b = vec![0.0f32; 2 * 4800];
            s.render(&mut b);
        }
        assert!(!s.voices.iter().any(|v| v.active), "the voice ended");
        // polyphony beyond the voice count steals
        for k in 40..60 {
            s.note_on(k, 0.8);
        }
        let mut b = vec![0.0f32; 2 * 512];
        s.render(&mut b);
        assert!(b.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn a_bad_script_never_breaks_sound() {
        let mut f = ScriptFx::new(48000.0);
        let mut buf = vec![0.3f32, 0.3];
        f.process(&mut buf); // nothing loaded: untouched
        assert_eq!(buf, vec![0.3, 0.3]);
        assert!(f.load("out = 1 / 0").is_ok());
        let mut buf = vec![0.3f32, 0.3];
        f.process(&mut buf);
        assert!(buf[0].is_finite());
    }
}
