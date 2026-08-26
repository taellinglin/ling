//! A small `no_std` tree-walking interpreter for a useful *subset* of Ling,
//! running inside the kernel -- this is what backs `ling run <file>` in the
//! Terminal. It's built on the kernel's global allocator (see `mm::heap`'s
//! GlobalAlloc adapter), which is the whole reason this can exist in-kernel.
//!
//! Honest scope, stated plainly rather than oversold: this handles the
//! language core -- `bind` variables, i64/f64 arithmetic with promotion,
//! comparisons, booleans, string literals and `+` concatenation, `if`/`else`,
//! `while`, function definitions and calls, `do { }` blocks, `return`, and
//! `print(...)`. It is NOT the full language: no pattern matching, structs/
//! enums, modules/imports, closures-over-outer-scope, or the huge native
//! builtin library (audio/gfx/net/...). Those live in the hosted compiler.
//! The plan (see the in-OS-ling notes) is to grow this toward the shared
//! reference core; for now it runs real Ling programs for the supported
//! subset, entirely inside LingOS.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

// ── Values ──────────────────────────────────────────────────────────────────
#[derive(Clone)]
enum Value {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Unit,
}

impl Value {
    fn display(&self) -> String {
        match self {
            Value::Int(i) => i.to_string(),
            Value::Float(f) => float_str(*f),
            Value::Str(s) => s.clone(),
            Value::Bool(b) => if *b { "true".into() } else { "false".into() },
            Value::Unit => "()".into(),
        }
    }
    fn truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Int(i) => *i != 0,
            Value::Float(f) => *f != 0.0,
            Value::Str(s) => !s.is_empty(),
            Value::Unit => false,
        }
    }
    fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }
}

/// Minimal float formatting (no std). Prints an integer-valued float without a
/// trailing ".0"? No -- keep ".0" so floats are distinguishable, then up to a
/// few fractional digits.
fn float_str(f: f64) -> String {
    if f.is_nan() {
        return "nan".into();
    }
    let neg = f < 0.0;
    let mut x = if neg { -f } else { f };
    let int_part = x as i64;
    x -= int_part as f64;
    let mut s = String::new();
    if neg {
        s.push('-');
    }
    s.push_str(&int_part.to_string());
    s.push('.');
    // 6 fractional digits, trimmed of trailing zeros (but keep one).
    let mut frac = [0u8; 6];
    for d in frac.iter_mut() {
        x *= 10.0;
        let digit = x as i64;
        *d = b'0' + (digit as u8 % 10);
        x -= digit as f64;
    }
    let mut end = frac.len();
    while end > 1 && frac[end - 1] == b'0' {
        end -= 1;
    }
    for &d in &frac[..end] {
        s.push(d as char);
    }
    s
}

// ── Tokens ──────────────────────────────────────────────────────────────────
#[derive(Clone, PartialEq)]
enum Tok {
    Int(i64),
    Float(f64),
    Str(String),
    Ident(String),
    Bind,
    Fn,
    If,
    Else,
    While,
    Do,
    Return,
    True,
    False,
    LParen,
    RParen,
    LBrace,
    RBrace,
    Comma,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Assign,
    EqEq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Not,
    Newline,
    Eof,
}

fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let b = src.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i];
        match c {
            b' ' | b'\t' | b'\r' => i += 1,
            b'\n' => {
                out.push(Tok::Newline);
                i += 1;
            },
            b'/' if i + 1 < b.len() && b[i + 1] == b'/' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            },
            b'"' => {
                i += 1;
                let mut s = String::new();
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\\' && i + 1 < b.len() {
                        i += 1;
                        s.push(match b[i] {
                            b'n' => '\n',
                            b't' => '\t',
                            b'"' => '"',
                            b'\\' => '\\',
                            other => other as char,
                        });
                    } else {
                        s.push(b[i] as char);
                    }
                    i += 1;
                }
                if i >= b.len() {
                    return Err("unterminated string".into());
                }
                i += 1; // closing quote
                out.push(Tok::Str(s));
            },
            b'0'..=b'9' => {
                let start = i;
                let mut is_float = false;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                    if b[i] == b'.' {
                        // a lone '.' not followed by a digit isn't part of the number
                        if i + 1 >= b.len() || !b[i + 1].is_ascii_digit() {
                            break;
                        }
                        is_float = true;
                    }
                    i += 1;
                }
                let text = core::str::from_utf8(&b[start..i]).unwrap_or("0");
                if is_float {
                    out.push(Tok::Float(parse_f64(text)));
                } else {
                    out.push(Tok::Int(text.parse::<i64>().unwrap_or(0)));
                }
            },
            _ if c == b'_' || c.is_ascii_alphabetic() => {
                let start = i;
                while i < b.len() && (b[i] == b'_' || b[i].is_ascii_alphanumeric()) {
                    i += 1;
                }
                let w = core::str::from_utf8(&b[start..i]).unwrap_or("");
                out.push(match w {
                    "bind" | "let" => Tok::Bind,
                    "fn" => Tok::Fn,
                    "if" => Tok::If,
                    "else" => Tok::Else,
                    "while" => Tok::While,
                    "do" => Tok::Do,
                    "return" => Tok::Return,
                    "true" => Tok::True,
                    "false" => Tok::False,
                    _ => Tok::Ident(w.to_string()),
                });
            },
            _ => {
                // Operators (two-char forms first).
                let two = if i + 1 < b.len() { &b[i..i + 2] } else { &b[i..i] };
                let (tok, len): (Tok, usize) = match two {
                    b"==" => (Tok::EqEq, 2),
                    b"!=" => (Tok::Ne, 2),
                    b"<=" => (Tok::Le, 2),
                    b">=" => (Tok::Ge, 2),
                    b"&&" => (Tok::And, 2),
                    b"||" => (Tok::Or, 2),
                    _ => {
                        let t = match c {
                            b'+' => Tok::Plus,
                            b'-' => Tok::Minus,
                            b'*' => Tok::Star,
                            b'/' => Tok::Slash,
                            b'%' => Tok::Percent,
                            b'=' => Tok::Assign,
                            b'<' => Tok::Lt,
                            b'>' => Tok::Gt,
                            b'!' => Tok::Not,
                            b'(' => Tok::LParen,
                            b')' => Tok::RParen,
                            b'{' => Tok::LBrace,
                            b'}' => Tok::RBrace,
                            b',' => Tok::Comma,
                            _ => return Err(format!("unexpected character '{}'", c as char)),
                        };
                        (t, 1)
                    },
                };
                out.push(tok);
                i += len;
            },
        }
    }
    out.push(Tok::Eof);
    Ok(out)
}

fn parse_f64(s: &str) -> f64 {
    // no_std has no str::parse::<f64>; do it by hand.
    let mut neg = false;
    let bytes = s.as_bytes();
    let mut i = 0;
    if i < bytes.len() && (bytes[i] == b'-' || bytes[i] == b'+') {
        neg = bytes[i] == b'-';
        i += 1;
    }
    let mut int_part: f64 = 0.0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        int_part = int_part * 10.0 + (bytes[i] - b'0') as f64;
        i += 1;
    }
    let mut frac: f64 = 0.0;
    let mut scale = 1.0;
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            frac = frac * 10.0 + (bytes[i] - b'0') as f64;
            scale *= 10.0;
            i += 1;
        }
    }
    let v = int_part + frac / scale;
    if neg { -v } else { v }
}

// ── AST ─────────────────────────────────────────────────────────────────────
#[derive(Clone)]
enum Expr {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Ident(String),
    Unary(u8, Box<Expr>),
    Bin(u8, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
    Do(Vec<Stmt>),
}

#[derive(Clone)]
enum Stmt {
    Bind(String, Expr),
    Expr(Expr),
    Return(Expr),
    If(Expr, Vec<Stmt>, Vec<Stmt>),
    While(Expr, Vec<Stmt>),
    Fn(String, Vec<String>, Vec<Stmt>),
}

// Binary-op codes (u8 keeps the AST small).
const OP_ADD: u8 = 0;
const OP_SUB: u8 = 1;
const OP_MUL: u8 = 2;
const OP_DIV: u8 = 3;
const OP_MOD: u8 = 4;
const OP_EQ: u8 = 5;
const OP_NE: u8 = 6;
const OP_LT: u8 = 7;
const OP_LE: u8 = 8;
const OP_GT: u8 = 9;
const OP_GE: u8 = 10;
const OP_AND: u8 = 11;
const OP_OR: u8 = 12;

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        self.toks.get(self.pos).unwrap_or(&Tok::Eof)
    }
    fn next(&mut self) -> Tok {
        let t = self.toks.get(self.pos).cloned().unwrap_or(Tok::Eof);
        self.pos += 1;
        t
    }
    fn skip_newlines(&mut self) {
        while matches!(self.peek(), Tok::Newline) {
            self.pos += 1;
        }
    }
    fn expect(&mut self, t: Tok, what: &str) -> Result<(), String> {
        if core::mem::discriminant(self.peek()) == core::mem::discriminant(&t) {
            self.pos += 1;
            Ok(())
        } else {
            Err(format!("expected {}", what))
        }
    }

    fn parse_program(&mut self) -> Result<Vec<Stmt>, String> {
        let mut stmts = Vec::new();
        self.skip_newlines();
        while !matches!(self.peek(), Tok::Eof) {
            stmts.push(self.parse_stmt()?);
            self.skip_newlines();
        }
        Ok(stmts)
    }

    fn parse_block(&mut self) -> Result<Vec<Stmt>, String> {
        self.expect(Tok::LBrace, "'{'")?;
        let mut stmts = Vec::new();
        self.skip_newlines();
        while !matches!(self.peek(), Tok::RBrace | Tok::Eof) {
            stmts.push(self.parse_stmt()?);
            self.skip_newlines();
        }
        self.expect(Tok::RBrace, "'}'")?;
        Ok(stmts)
    }

    fn parse_stmt(&mut self) -> Result<Stmt, String> {
        match self.peek() {
            Tok::Bind => {
                self.pos += 1;
                let name = match self.next() {
                    Tok::Ident(n) => n,
                    _ => return Err("expected name after 'bind'".into()),
                };
                self.expect(Tok::Assign, "'='")?;
                let e = self.parse_expr(0)?;
                Ok(Stmt::Bind(name, e))
            },
            Tok::Fn => {
                self.pos += 1;
                let name = match self.next() {
                    Tok::Ident(n) => n,
                    _ => return Err("expected function name".into()),
                };
                self.expect(Tok::LParen, "'('")?;
                let mut params = Vec::new();
                while !matches!(self.peek(), Tok::RParen) {
                    match self.next() {
                        Tok::Ident(p) => params.push(p),
                        _ => return Err("expected parameter name".into()),
                    }
                    if matches!(self.peek(), Tok::Comma) {
                        self.pos += 1;
                    }
                }
                self.expect(Tok::RParen, "')'")?;
                let body = self.parse_block()?;
                Ok(Stmt::Fn(name, params, body))
            },
            Tok::If => {
                self.pos += 1;
                let cond = self.parse_expr(0)?;
                let then_b = self.parse_block()?;
                let mut else_b = Vec::new();
                self.skip_newlines();
                if matches!(self.peek(), Tok::Else) {
                    self.pos += 1;
                    if matches!(self.peek(), Tok::If) {
                        else_b = alloc::vec![self.parse_stmt()?];
                    } else {
                        else_b = self.parse_block()?;
                    }
                }
                Ok(Stmt::If(cond, then_b, else_b))
            },
            Tok::While => {
                self.pos += 1;
                let cond = self.parse_expr(0)?;
                let body = self.parse_block()?;
                Ok(Stmt::While(cond, body))
            },
            Tok::Return => {
                self.pos += 1;
                let e = if matches!(self.peek(), Tok::Newline | Tok::RBrace | Tok::Eof) {
                    Expr::Int(0)
                } else {
                    self.parse_expr(0)?
                };
                Ok(Stmt::Return(e))
            },
            _ => Ok(Stmt::Expr(self.parse_expr(0)?)),
        }
    }

    // Precedence-climbing expression parser.
    fn parse_expr(&mut self, min_bp: u8) -> Result<Expr, String> {
        let mut lhs = self.parse_unary()?;
        loop {
            let (op, bp) = match self.peek() {
                Tok::Or => (OP_OR, 1),
                Tok::And => (OP_AND, 2),
                Tok::EqEq => (OP_EQ, 3),
                Tok::Ne => (OP_NE, 3),
                Tok::Lt => (OP_LT, 4),
                Tok::Le => (OP_LE, 4),
                Tok::Gt => (OP_GT, 4),
                Tok::Ge => (OP_GE, 4),
                Tok::Plus => (OP_ADD, 5),
                Tok::Minus => (OP_SUB, 5),
                Tok::Star => (OP_MUL, 6),
                Tok::Slash => (OP_DIV, 6),
                Tok::Percent => (OP_MOD, 6),
                _ => break,
            };
            if bp < min_bp {
                break;
            }
            self.pos += 1;
            let rhs = self.parse_expr(bp + 1)?;
            lhs = Expr::Bin(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        match self.peek() {
            Tok::Minus => {
                self.pos += 1;
                Ok(Expr::Unary(b'-', Box::new(self.parse_unary()?)))
            },
            Tok::Not => {
                self.pos += 1;
                Ok(Expr::Unary(b'!', Box::new(self.parse_unary()?)))
            },
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        match self.next() {
            Tok::Int(i) => Ok(Expr::Int(i)),
            Tok::Float(f) => Ok(Expr::Float(f)),
            Tok::Str(s) => Ok(Expr::Str(s)),
            Tok::True => Ok(Expr::Bool(true)),
            Tok::False => Ok(Expr::Bool(false)),
            Tok::LParen => {
                let e = self.parse_expr(0)?;
                self.expect(Tok::RParen, "')'")?;
                Ok(e)
            },
            Tok::Do => {
                let body = self.parse_block()?;
                Ok(Expr::Do(body))
            },
            Tok::Ident(name) => {
                if matches!(self.peek(), Tok::LParen) {
                    self.pos += 1;
                    let mut args = Vec::new();
                    self.skip_newlines();
                    while !matches!(self.peek(), Tok::RParen) {
                        args.push(self.parse_expr(0)?);
                        self.skip_newlines();
                        if matches!(self.peek(), Tok::Comma) {
                            self.pos += 1;
                            self.skip_newlines();
                        }
                    }
                    self.expect(Tok::RParen, "')'")?;
                    Ok(Expr::Call(name, args))
                } else {
                    Ok(Expr::Ident(name))
                }
            },
            other => Err(format!("unexpected token in expression: {}", tok_name(&other))),
        }
    }
}

fn tok_name(t: &Tok) -> &'static str {
    match t {
        Tok::Eof => "end of input",
        Tok::RBrace => "'}'",
        Tok::RParen => "')'",
        Tok::Newline => "newline",
        _ => "token",
    }
}

// ── Evaluator ────────────────────────────────────────────────────────────────
type Env = BTreeMap<String, Value>;

enum Flow {
    /// A block ran to the end; the value is that of its last expression
    /// statement (Ling is expression-oriented -- the last expression is the
    /// block's/function's value), or Unit if it ended on a non-expression.
    Normal(Value),
    Return(Value),
}

struct Interp {
    funcs: BTreeMap<String, (Vec<String>, Vec<Stmt>)>,
    out: String,
    depth: u32,
}

const MAX_DEPTH: u32 = 256;

impl Interp {
    fn exec_block(&mut self, stmts: &[Stmt], env: &mut Env) -> Result<Flow, String> {
        let mut last = Value::Unit;
        for s in stmts {
            match self.exec_stmt(s, env)? {
                Flow::Return(v) => return Ok(Flow::Return(v)),
                Flow::Normal(v) => last = v,
            }
        }
        Ok(Flow::Normal(last))
    }

    fn exec_stmt(&mut self, s: &Stmt, env: &mut Env) -> Result<Flow, String> {
        match s {
            Stmt::Bind(name, e) => {
                let v = self.eval(e, env)?;
                env.insert(name.clone(), v);
                Ok(Flow::Normal(Value::Unit))
            },
            Stmt::Expr(e) => {
                let v = self.eval(e, env)?;
                Ok(Flow::Normal(v))
            },
            Stmt::Return(e) => {
                let v = self.eval(e, env)?;
                Ok(Flow::Return(v))
            },
            Stmt::If(cond, then_b, else_b) => {
                if self.eval(cond, env)?.truthy() {
                    self.exec_block(then_b, env)
                } else {
                    self.exec_block(else_b, env)
                }
            },
            Stmt::While(cond, body) => {
                let mut guard = 0u64;
                while self.eval(cond, env)?.truthy() {
                    if let Flow::Return(v) = self.exec_block(body, env)? {
                        return Ok(Flow::Return(v));
                    }
                    guard += 1;
                    if guard > 10_000_000 {
                        return Err("while loop exceeded iteration limit".into());
                    }
                }
                Ok(Flow::Normal(Value::Unit))
            },
            Stmt::Fn(name, params, body) => {
                self.funcs.insert(name.clone(), (params.clone(), body.clone()));
                Ok(Flow::Normal(Value::Unit))
            },
        }
    }

    fn eval(&mut self, e: &Expr, env: &mut Env) -> Result<Value, String> {
        match e {
            Expr::Int(i) => Ok(Value::Int(*i)),
            Expr::Float(f) => Ok(Value::Float(*f)),
            Expr::Str(s) => Ok(Value::Str(s.clone())),
            Expr::Bool(b) => Ok(Value::Bool(*b)),
            Expr::Ident(n) => env
                .get(n)
                .cloned()
                .ok_or_else(|| format!("undefined variable '{}'", n)),
            Expr::Unary(op, inner) => {
                let v = self.eval(inner, env)?;
                match op {
                    b'-' => match v {
                        Value::Int(i) => Ok(Value::Int(-i)),
                        Value::Float(f) => Ok(Value::Float(-f)),
                        _ => Err("cannot negate a non-number".into()),
                    },
                    _ => Ok(Value::Bool(!v.truthy())),
                }
            },
            Expr::Bin(op, l, r) => {
                // Short-circuit boolean ops.
                if *op == OP_AND {
                    let lv = self.eval(l, env)?;
                    if !lv.truthy() {
                        return Ok(Value::Bool(false));
                    }
                    return Ok(Value::Bool(self.eval(r, env)?.truthy()));
                }
                if *op == OP_OR {
                    let lv = self.eval(l, env)?;
                    if lv.truthy() {
                        return Ok(Value::Bool(true));
                    }
                    return Ok(Value::Bool(self.eval(r, env)?.truthy()));
                }
                let lv = self.eval(l, env)?;
                let rv = self.eval(r, env)?;
                self.binop(*op, lv, rv)
            },
            Expr::Do(body) => {
                let mut inner = env.clone();
                match self.exec_block(body, &mut inner)? {
                    Flow::Return(v) | Flow::Normal(v) => Ok(v),
                }
            },
            Expr::Call(name, args) => self.call(name, args, env),
        }
    }

    fn binop(&self, op: u8, l: Value, r: Value) -> Result<Value, String> {
        // String concatenation with '+'.
        if op == OP_ADD {
            if let (Value::Str(a), b) = (&l, &r) {
                return Ok(Value::Str(format!("{}{}", a, b.display())));
            }
            if let (a, Value::Str(b)) = (&l, &r) {
                return Ok(Value::Str(format!("{}{}", a.display(), b)));
            }
        }
        // Equality works for any same-ish types.
        if op == OP_EQ || op == OP_NE {
            let eq = values_equal(&l, &r);
            return Ok(Value::Bool(if op == OP_EQ { eq } else { !eq }));
        }
        // Numeric ops: integer-preserving when both are ints, else float.
        if let (Value::Int(a), Value::Int(b)) = (&l, &r) {
            let (a, b) = (*a, *b);
            return Ok(match op {
                OP_ADD => Value::Int(a.wrapping_add(b)),
                OP_SUB => Value::Int(a.wrapping_sub(b)),
                OP_MUL => Value::Int(a.wrapping_mul(b)),
                OP_DIV => {
                    if b == 0 {
                        return Err("division by zero".into());
                    }
                    Value::Int(a / b)
                },
                OP_MOD => {
                    if b == 0 {
                        return Err("modulo by zero".into());
                    }
                    Value::Int(a % b)
                },
                OP_LT => Value::Bool(a < b),
                OP_LE => Value::Bool(a <= b),
                OP_GT => Value::Bool(a > b),
                OP_GE => Value::Bool(a >= b),
                _ => Value::Unit,
            });
        }
        let (a, b) = match (l.as_f64(), r.as_f64()) {
            (Some(a), Some(b)) => (a, b),
            _ => return Err("arithmetic on non-numbers".into()),
        };
        Ok(match op {
            OP_ADD => Value::Float(a + b),
            OP_SUB => Value::Float(a - b),
            OP_MUL => Value::Float(a * b),
            OP_DIV => Value::Float(a / b),
            OP_MOD => Value::Float(a % b),
            OP_LT => Value::Bool(a < b),
            OP_LE => Value::Bool(a <= b),
            OP_GT => Value::Bool(a > b),
            OP_GE => Value::Bool(a >= b),
            _ => Value::Unit,
        })
    }

    fn call(&mut self, name: &str, args: &[Expr], env: &mut Env) -> Result<Value, String> {
        // Builtins first.
        match name {
            "print" | "println" => {
                let mut first = true;
                for a in args {
                    let v = self.eval(a, env)?;
                    if !first {
                        self.out.push(' ');
                    }
                    self.out.push_str(&v.display());
                    first = false;
                }
                self.out.push('\n');
                return Ok(Value::Unit);
            },
            "str" => {
                let v = if args.is_empty() {
                    Value::Unit
                } else {
                    self.eval(&args[0], env)?
                };
                return Ok(Value::Str(v.display()));
            },
            "len" => {
                let v = if args.is_empty() {
                    Value::Unit
                } else {
                    self.eval(&args[0], env)?
                };
                return Ok(Value::Int(match v {
                    Value::Str(s) => s.len() as i64,
                    _ => 0,
                }));
            },
            _ => {},
        }
        // User function.
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            self.depth -= 1;
            return Err("recursion too deep".into());
        }
        let (params, body) = match self.funcs.get(name) {
            Some(f) => (f.0.clone(), f.1.clone()),
            None => {
                self.depth -= 1;
                return Err(format!("call to undefined function '{}'", name));
            },
        };
        if args.len() != params.len() {
            self.depth -= 1;
            return Err(format!(
                "'{}' expects {} args, got {}",
                name,
                params.len(),
                args.len()
            ));
        }
        let mut local: Env = BTreeMap::new();
        for (p, a) in params.iter().zip(args.iter()) {
            let v = self.eval(a, env)?;
            local.insert(p.clone(), v);
        }
        let result = match self.exec_block(&body, &mut local)? {
            Flow::Return(v) | Flow::Normal(v) => v,
        };
        self.depth -= 1;
        Ok(result)
    }
}

/// Structural equality for the value types the subset supports.
fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Unit, Value::Unit) => true,
        _ => match (a.as_f64(), b.as_f64()) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        },
    }
}

/// Run a Ling program from source. Returns the program's printed output and,
/// if it stopped early, an error message. Top-level statements run in order;
/// `fn` definitions are registered first so forward references work.
pub fn run_source(src: &str) -> (String, Option<String>) {
    let toks = match lex(src) {
        Ok(t) => t,
        Err(e) => return (String::new(), Some(e)),
    };
    let mut p = Parser { toks, pos: 0 };
    let stmts = match p.parse_program() {
        Ok(s) => s,
        Err(e) => return (String::new(), Some(e)),
    };
    let mut it = Interp { funcs: BTreeMap::new(), out: String::new(), depth: 0 };
    for s in &stmts {
        if let Stmt::Fn(n, params, body) = s {
            it.funcs.insert(n.clone(), (params.clone(), body.clone()));
        }
    }
    let mut env: Env = BTreeMap::new();
    for s in &stmts {
        if matches!(s, Stmt::Fn(..)) {
            continue;
        }
        match it.exec_stmt(s, &mut env) {
            Ok(Flow::Return(_)) => break,
            Ok(Flow::Normal(_)) => {},
            Err(e) => return (it.out, Some(e)),
        }
    }
    (it.out, None)
}
