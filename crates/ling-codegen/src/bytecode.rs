use crate::CodegenBackend;
use ling_ast::ast::BinOp;
use ling_ast::ast::UnOp;
use ling_mir::ir::*;
use std::collections::HashMap;

// ─── Opcodes ─────────────────────────────────────────────────────────────────

const OP_NOP: u8 = 0;
const OP_PUSHI64: u8 = 1;
const OP_PUSHF64: u8 = 2;
const OP_PUSHSTR: u8 = 3;
const OP_PUSHBOOL: u8 = 4;
const OP_PUSHNONE: u8 = 5;
const OP_POP: u8 = 6;
const OP_LOADLOCAL: u8 = 7;
const OP_STORELOCAL: u8 = 8;
const OP_ADD: u8 = 9;
const OP_SUB: u8 = 10;
const OP_MUL: u8 = 11;
const OP_DIV: u8 = 12;
const OP_REM: u8 = 13;
const OP_EQ: u8 = 14;
const OP_NE: u8 = 15;
const OP_LT: u8 = 16;
const OP_LE: u8 = 17;
const OP_GT: u8 = 18;
const OP_GE: u8 = 19;
const OP_AND: u8 = 20;
const OP_OR: u8 = 21;
const OP_NOT: u8 = 22;
const OP_NEG: u8 = 23;
const OP_CALL: u8 = 24;
const OP_CALLBUILTIN: u8 = 25;
const OP_RET: u8 = 26;
const OP_JUMP: u8 = 27;
const OP_JUMPIFFALSE: u8 = 28;
const OP_MAKELIST: u8 = 29;
const OP_GETINDEX: u8 = 30;
const OP_HALT: u8 = 0xFF;

// ─── Values ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Value {
    Number(f64),
    Bool(bool),
    Str(String),
    None,
    List(Vec<Value>),
}

impl Value {
    fn is_truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Number(n) => *n != 0.0,
            Value::Str(s) => !s.is_empty(),
            Value::List(l) => !l.is_empty(),
            Value::None => false,
        }
    }

    fn display(&self) -> String {
        match self {
            Value::Number(n) => {
                if n.fract() == 0.0 && n.is_finite() && n.abs() < 1e15 {
                    format!("{:.0}", n)
                } else {
                    format!("{}", n)
                }
            },
            Value::Bool(b) => b.to_string(),
            Value::Str(s) => s.clone(),
            Value::None => "()".to_string(),
            Value::List(l) => {
                let items: Vec<String> = l.iter().map(|v| v.display()).collect();
                format!("[{}]", items.join(", "))
            },
        }
    }
}

fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => a.to_bits() == b.to_bits(),
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Str(a), Value::Str(b)) => a == b,
        (Value::None, Value::None) => true,
        (Value::List(a), Value::List(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| values_equal(x, y))
        },
        _ => false,
    }
}

fn num_cmp<F: Fn(f64, f64) -> bool>(a: &Value, b: &Value, cmp: F) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => cmp(*a, *b),
        _ => false,
    }
}

fn resolve_builtin(name: &str) -> Result<(&'static str, usize), String> {
    match name {
        "print" | "println" | "พิมพ์" | "印" | "打印" | "印刷" => Ok(("print", 1)),
        "format" | "รูปแบบ" | "格式" | "フォーマット" | "서식" => {
            Ok(("format", 0))
        },
        "len" | "str_len" | "ความยาว" | "长度" | "長さ" | "길이" => {
            Ok(("len", 1))
        },
        "to_str" | "str" | "แปลงสตริง" => Ok(("to_str", 1)),
        "sin" => Ok(("sin", 1)),
        "cos" => Ok(("cos", 1)),
        _ => Err(format!("unknown builtin: {}", name)),
    }
}

// ─── Chunk ───────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct Chunk {
    pub code: Vec<u8>,
    pub floats: Vec<f64>,
    pub strings: Vec<String>,
    pub local_count: u16,
}

impl Chunk {
    fn w(&mut self, b: u8) {
        self.code.push(b);
    }

    fn w2(&mut self, v: u16) {
        self.code.extend_from_slice(&v.to_le_bytes());
    }

    fn w8(&mut self, v: i64) {
        self.code.extend_from_slice(&v.to_le_bytes());
    }

    fn add_float(&mut self, v: f64) -> u16 {
        let i = self.floats.len();
        self.floats.push(v);
        i as u16
    }

    fn add_str(&mut self, s: &str) -> u16 {
        let i = self.strings.len();
        self.strings.push(s.to_string());
        i as u16
    }
}

// ─── Compiled program ────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct CompiledFunction {
    pub name: String,
    pub chunk: Chunk,
    pub arg_count: u16,
}

#[derive(Debug)]
pub struct VmProgram {
    pub functions: Vec<CompiledFunction>,
    pub main_index: usize,
}

// ─── Compiler ────────────────────────────────────────────────────────────────

struct Ctx {
    chunk: Chunk,
    local_map: HashMap<Local, u16>,
    local_next: u16,
    bb_start: HashMap<usize, usize>,
    patches: Vec<(usize, usize)>,
}

pub fn compile_mir_program(mir: &MirProgram) -> VmProgram {
    let fn_names: Vec<String> = mir.functions.iter().map(|f| f.name.clone()).collect();
    let mut functions = Vec::new();
    let mut main_index = 0;

    for (i, mir_fn) in mir.functions.iter().enumerate() {
        let chunk = compile_fn(mir_fn, &fn_names);
        if mir_fn.name == "__main__" || mir_fn.name == "start" || mir_fn.name == "เริ่ม" {
            main_index = i;
        }
        functions.push(CompiledFunction {
            name: mir_fn.name.clone(),
            chunk,
            arg_count: mir_fn.arg_count as u16,
        });
    }

    if functions.is_empty() {
        functions.push(CompiledFunction {
            name: "__main__".into(),
            chunk: Chunk { code: vec![OP_PUSHNONE, OP_RET], ..Default::default() },
            arg_count: 0,
        });
    }

    VmProgram { functions, main_index }
}

fn compile_fn(mir_fn: &MirFunction, fn_names: &[String]) -> Chunk {
    let mut ctx = Ctx {
        chunk: Chunk::default(),
        local_map: HashMap::new(),
        local_next: 0,
        bb_start: HashMap::new(),
        patches: Vec::new(),
    };

    // Local(0) = return slot at slot 0
    ctx.local_map.insert(Local(0), 0);

    // Args: Local(1)..Local(arg_count) get sequential slots
    for i in 0..mir_fn.arg_count {
        let local = Local(i + 1);
        ctx.local_map.entry(local).or_insert_with(|| {
            let s = ctx.local_next;
            ctx.local_next += 1;
            s
        });
    }

    // Explicit locals get slots after args
    for li in 0..mir_fn.locals.len() {
        let local = Local(mir_fn.arg_count + 1 + li);
        ctx.local_map.entry(local).or_insert_with(|| {
            let s = ctx.local_next;
            ctx.local_next += 1;
            s
        });
    }

    ctx.chunk.local_count = ctx.local_next;

    for bb_idx in 0..mir_fn.basic_blocks.len() {
        ctx.bb_start.insert(bb_idx, ctx.chunk.code.len());
        let bb = &mir_fn.basic_blocks[bb_idx];
        for stmt in &bb.statements {
            compile_stmt(stmt, fn_names, &mut ctx);
        }
        if let Some(term) = &bb.terminator {
            compile_term(term, fn_names, &mut ctx);
        }
    }

    for &(pos, target) in &ctx.patches {
        let target_offset = ctx.bb_start.get(&target).copied().unwrap_or(0);
        ctx.chunk.code[pos..pos + 2].copy_from_slice(&(target_offset as u16).to_le_bytes());
    }

    ctx.chunk
}

fn compile_stmt(stmt: &Statement, fn_names: &[String], ctx: &mut Ctx) {
    match &stmt.kind {
        StatementKind::Assign(local, rvalue) => {
            compile_rval(rvalue, fn_names, ctx);
            let slot = *ctx.local_map.entry(*local).or_insert_with(|| {
                let s = ctx.local_next;
                ctx.local_next += 1;
                s
            });
            ctx.chunk.w(OP_STORELOCAL);
            ctx.chunk.w2(slot);
        },
        _ => {},
    }
}

fn compile_rval(rv: &Rvalue, fn_names: &[String], ctx: &mut Ctx) {
    match rv {
        Rvalue::Use(op) => compile_op(op, fn_names, ctx),
        Rvalue::BinaryOp(op, l, r) => {
            compile_op(l, fn_names, ctx);
            compile_op(r, fn_names, ctx);
            ctx.chunk.w(match op {
                BinOp::Add => OP_ADD,
                BinOp::Sub => OP_SUB,
                BinOp::Mul => OP_MUL,
                BinOp::Div => OP_DIV,
                BinOp::Rem => OP_REM,
                BinOp::Eq => OP_EQ,
                BinOp::Ne => OP_NE,
                BinOp::Lt => OP_LT,
                BinOp::Le => OP_LE,
                BinOp::Gt => OP_GT,
                BinOp::Ge => OP_GE,
                BinOp::And => OP_AND,
                BinOp::Or => OP_OR,
                _ => OP_NOP, // unsupported ops become nop
            });
        },
        Rvalue::UnaryOp(op, o) => {
            compile_op(o, fn_names, ctx);
            ctx.chunk.w(match op {
                UnOp::Not => OP_NOT,
                UnOp::Neg => OP_NEG,
                _ => OP_NOP,
            });
        },
        Rvalue::Call { func, args } => {
            for arg in args {
                compile_op(arg, fn_names, ctx);
            }
            match func {
                Operand::Constant(Constant::Function(name)) => {
                    if let Some(idx) = fn_names.iter().position(|n| n == name) {
                        ctx.chunk.w(OP_CALL);
                        ctx.chunk.w2(idx as u16);
                    } else {
                        let sidx = ctx.chunk.add_str(name);
                        ctx.chunk.w(OP_CALLBUILTIN);
                        ctx.chunk.w2(sidx);
                        ctx.chunk.w2(args.len() as u16);
                    }
                },
                _ => {
                    compile_op(func, fn_names, ctx);
                    let sidx = ctx.chunk.add_str("__indirect");
                    ctx.chunk.w(OP_CALLBUILTIN);
                    ctx.chunk.w2(sidx);
                    ctx.chunk.w2(args.len() as u16);
                },
            }
        },
        Rvalue::Aggregate(_kind, ops) => {
            for op in ops {
                compile_op(op, fn_names, ctx);
            }
            ctx.chunk.w(OP_MAKELIST);
            ctx.chunk.w2(ops.len() as u16);
        },
        Rvalue::GetIndex(obj, idx) => {
            compile_op(obj, fn_names, ctx);
            compile_op(idx, fn_names, ctx);
            ctx.chunk.w(OP_GETINDEX);
        },
        _ => {
            ctx.chunk.w(OP_PUSHNONE);
        },
    }
}

fn compile_op(op: &Operand, _fn_names: &[String], ctx: &mut Ctx) {
    match op {
        Operand::Copy(l) | Operand::Move(l) => {
            let slot = ctx.local_map.get(l).copied().unwrap_or(0);
            ctx.chunk.w(OP_LOADLOCAL);
            ctx.chunk.w2(slot);
        },
        Operand::Constant(c) => match c {
            Constant::I64(v) => {
                ctx.chunk.w(OP_PUSHI64);
                ctx.chunk.w8(*v);
            },
            Constant::F64(bits) => {
                let idx = ctx.chunk.add_float(f64::from_bits(*bits));
                ctx.chunk.w(OP_PUSHF64);
                ctx.chunk.w2(idx);
            },
            Constant::Str(s) => {
                let idx = ctx.chunk.add_str(s);
                ctx.chunk.w(OP_PUSHSTR);
                ctx.chunk.w2(idx);
            },
            Constant::Bool(b) => {
                ctx.chunk.w(OP_PUSHBOOL);
                ctx.chunk.w(if *b { 1 } else { 0 });
            },
            _ => {
                ctx.chunk.w(OP_PUSHNONE);
            },
        },
    }
}

fn compile_term(term: &Terminator, fn_names: &[String], ctx: &mut Ctx) {
    match &term.kind {
        TerminatorKind::Return => {
            ctx.chunk.w(OP_LOADLOCAL);
            ctx.chunk.w2(0);
            ctx.chunk.w(OP_RET);
        },
        TerminatorKind::Goto { target } => {
            ctx.chunk.w(OP_JUMP);
            let pos = ctx.chunk.code.len();
            ctx.chunk.w2(0);
            ctx.patches.push((pos, target.0));
        },
        TerminatorKind::SwitchInt { discr, targets, otherwise } => {
            compile_op(discr, fn_names, ctx);
            if let Some((val, target)) = targets.first() {
                ctx.chunk.w(OP_PUSHI64);
                ctx.chunk.w8(*val);
                ctx.chunk.w(OP_EQ);
                ctx.chunk.w(OP_JUMPIFFALSE);
                let pos = ctx.chunk.code.len();
                ctx.chunk.w2(0);
                ctx.patches.push((pos, target.0));
            }
            ctx.chunk.w(OP_JUMP);
            let pos = ctx.chunk.code.len();
            ctx.chunk.w2(0);
            ctx.patches.push((pos, otherwise.0));
        },
        TerminatorKind::Unreachable => {
            ctx.chunk.w(OP_HALT);
        },
    }
}

fn read_u8(code: &[u8], ip: usize) -> u8 {
    code[ip]
}

fn read_u16(code: &[u8], ip: usize) -> u16 {
    let mut bytes = [0u8; 2];
    bytes.copy_from_slice(&code[ip..ip + 2]);
    u16::from_le_bytes(bytes)
}

fn read_i64(code: &[u8], ip: usize) -> i64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&code[ip..ip + 8]);
    i64::from_le_bytes(bytes)
}

// ─── VM ──────────────────────────────────────────────────────────────────────

struct Frame {
    fn_index: usize,
    ip: usize,
    base: usize,
    local_count: usize,
}

pub struct Vm {
    stack: Vec<Value>,
    program: Option<VmProgram>,
    call_stack: Vec<Frame>,
}

impl Vm {
    pub fn new() -> Self {
        Self { stack: Vec::new(), program: None, call_stack: Vec::new() }
    }

    pub fn load(&mut self, program: VmProgram) {
        self.program = Some(program);
    }

    pub fn run_main(&mut self) -> Result<(), String> {
        let p = self.program.as_ref().ok_or("no program loaded")?;
        if p.functions.is_empty() {
            return Ok(());
        }
        let main_idx = p.main_index;
        let func = &p.functions[main_idx];
        let lc = func.chunk.local_count as usize;
        for _ in 0..lc {
            self.stack.push(Value::None);
        }
        self.call_stack
            .push(Frame { fn_index: main_idx, ip: 0, base: 0, local_count: lc });
        self.exec()
    }

    fn exec(&mut self) -> Result<(), String> {
        loop {
            if self.call_stack.is_empty() {
                return Ok(());
            }

            let frame_ip = self.call_stack.last().unwrap().ip;

            // Pre-extract chunk data through an index to avoid holding cross-references
            let fn_index = self.call_stack.last().unwrap().fn_index;
            let (code, floats, strings) = {
                let p = self.program.as_ref().unwrap();
                let chunk = &p.functions[fn_index].chunk;
                // Clone small headers to avoid holding borrow across mutable ops
                (
                    chunk.code.clone(),
                    chunk.floats.clone(),
                    chunk.strings.clone(),
                )
            };
            let code_len = code.len();

            if frame_ip >= code_len {
                self.call_stack.pop();
                return Err("unexpected end of code".into());
            }

            let op = read_u8(&code, frame_ip);
            let mut ip = frame_ip + 1;

            match op {
                OP_PUSHI64 => {
                    let v = read_i64(&code, ip);
                    ip += 8;
                    self.stack.push(Value::Number(v as f64));
                },
                OP_PUSHF64 => {
                    let idx = read_u16(&code, ip) as usize;
                    ip += 2;
                    self.stack.push(Value::Number(floats[idx]));
                },
                OP_PUSHSTR => {
                    let idx = read_u16(&code, ip) as usize;
                    ip += 2;
                    self.stack.push(Value::Str(strings[idx].clone()));
                },
                OP_PUSHBOOL => {
                    let b = read_u8(&code, ip) != 0;
                    ip += 1;
                    self.stack.push(Value::Bool(b));
                },
                OP_PUSHNONE => self.stack.push(Value::None),
                OP_POP => {
                    self.stack.pop();
                },
                OP_LOADLOCAL => {
                    let idx = read_u16(&code, ip) as usize;
                    ip += 2;
                    let base = self.call_stack.last().unwrap().base;
                    self.stack.push(self.stack[base + idx].clone());
                },
                OP_STORELOCAL => {
                    let idx = read_u16(&code, ip) as usize;
                    ip += 2;
                    let val = self.stack.pop().unwrap();
                    let base = self.call_stack.last().unwrap().base;
                    self.stack[base + idx] = val;
                },
                OP_ADD | OP_SUB | OP_MUL | OP_DIV | OP_REM => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.pop().unwrap();
                    let r = match (op, &a, &b) {
                        (OP_ADD, Value::Str(s), v) => Value::Str(format!("{}{}", s, v.display())),
                        (OP_ADD, v, Value::Str(s)) => Value::Str(format!("{}{}", v.display(), s)),
                        (OP_ADD, Value::Number(a), Value::Number(b)) => Value::Number(a + b),
                        (OP_SUB, Value::Number(a), Value::Number(b)) => Value::Number(a - b),
                        (OP_MUL, Value::Number(a), Value::Number(b)) => Value::Number(a * b),
                        (OP_DIV, Value::Number(a), Value::Number(b)) => Value::Number(a / b),
                        (OP_REM, Value::Number(a), Value::Number(b)) => Value::Number(a % b),
                        _ => return Err("type error in arithmetic".into()),
                    };
                    self.stack.push(r);
                },
                OP_EQ | OP_NE | OP_LT | OP_LE | OP_GT | OP_GE => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.pop().unwrap();
                    let r = match op {
                        OP_EQ => values_equal(&a, &b),
                        OP_NE => !values_equal(&a, &b),
                        OP_LT => num_cmp(&a, &b, |a, b| a < b),
                        OP_LE => num_cmp(&a, &b, |a, b| a <= b),
                        OP_GT => num_cmp(&a, &b, |a, b| a > b),
                        OP_GE => num_cmp(&a, &b, |a, b| a >= b),
                        _ => false,
                    };
                    self.stack.push(Value::Bool(r));
                },
                OP_AND => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.pop().unwrap();
                    self.stack.push(Value::Bool(a.is_truthy() && b.is_truthy()));
                },
                OP_OR => {
                    let b = self.stack.pop().unwrap();
                    let a = self.stack.pop().unwrap();
                    self.stack.push(Value::Bool(a.is_truthy() || b.is_truthy()));
                },
                OP_NOT => {
                    let v = self.stack.pop().unwrap();
                    self.stack.push(Value::Bool(!v.is_truthy()));
                },
                OP_NEG => {
                    let v = self.stack.pop().unwrap();
                    if let Value::Number(n) = v {
                        self.stack.push(Value::Number(-n));
                    } else {
                        return Err("type error: negate non-number".into());
                    }
                },
                OP_CALL => {
                    let idx = read_u16(&code, ip) as usize;
                    ip += 2;
                    let (ac, lc) = {
                        let p = self.program.as_ref().unwrap();
                        (
                            p.functions[idx].arg_count as usize,
                            p.functions[idx].chunk.local_count as usize,
                        )
                    };
                    let extra = lc.saturating_sub(ac);
                    for _ in 0..extra {
                        self.stack.push(Value::None);
                    }
                    let new_base = self.stack.len() - lc;
                    self.call_stack.push(Frame {
                        fn_index: idx,
                        ip: 0,
                        base: new_base,
                        local_count: lc,
                    });
                },
                OP_CALLBUILTIN => {
                    let si = read_u16(&code, ip) as usize;
                    ip += 2;
                    let count = read_u16(&code, ip) as usize;
                    ip += 2;
                    let name = strings[si].clone();
                    let (canon, _) =
                        resolve_builtin(&name).map_err(|e| format!("{} in {}", e, name))?;
                    self.call_builtin(canon, count)?;
                },
                OP_RET => {
                    let val = self.stack.pop().unwrap_or(Value::None);
                    let _frame = self.call_stack.pop().unwrap();
                    if let Some(caller) = self.call_stack.last() {
                        self.stack.truncate(caller.base + caller.local_count);
                        self.stack.push(val);
                    } else {
                        self.stack.push(val);
                        return Ok(());
                    }
                },
                OP_JUMP => {
                    ip = read_u16(&code, ip) as usize;
                },
                OP_JUMPIFFALSE => {
                    let offset = read_u16(&code, ip) as usize;
                    ip += 2;
                    let cond = self.stack.pop().unwrap();
                    if !cond.is_truthy() {
                        ip = offset;
                    }
                },
                OP_MAKELIST => {
                    let count = read_u16(&code, ip) as usize;
                    ip += 2;
                    let mut items = Vec::with_capacity(count);
                    for _ in 0..count {
                        items.push(self.stack.pop().unwrap());
                    }
                    items.reverse();
                    self.stack.push(Value::List(items));
                },
                OP_GETINDEX => {
                    let idx_v = self.stack.pop().unwrap();
                    let obj = self.stack.pop().unwrap();
                    match (&obj, &idx_v) {
                        (Value::List(list), Value::Number(n)) => {
                            let i = *n as usize;
                            self.stack.push(if i < list.len() {
                                list[i].clone()
                            } else {
                                Value::None
                            });
                        },
                        _ => self.stack.push(Value::None),
                    }
                },
                OP_HALT => return Ok(()),
                _ => return Err(format!("unknown opcode {} at ip {}", op, ip - 1)),
            }

            self.call_stack.last_mut().unwrap().ip = ip;
        }
    }

    fn call_builtin(&mut self, name: &str, arg_count: usize) -> Result<(), String> {
        match name {
            "print" => {
                if let Some(val) = self.stack.pop() {
                    println!("{}", val.display());
                }
                self.stack.push(Value::None);
            },
            "format" => {
                let mut parts: Vec<String> = Vec::with_capacity(arg_count);
                for _ in 0..arg_count {
                    if let Some(val) = self.stack.pop() {
                        parts.push(val.display());
                    }
                }
                parts.reverse();
                if parts.is_empty() {
                    self.stack.push(Value::Str(String::new()));
                    return Ok(());
                }
                let fmt_str = parts.remove(0);
                let mut result = fmt_str;
                for part in parts {
                    result = result.replacen("{}", &part, 1);
                }
                self.stack.push(Value::Str(result));
            },
            "len" => {
                let val = self.stack.pop().unwrap_or(Value::None);
                let n = match &val {
                    Value::Str(s) => s.len() as f64,
                    Value::List(l) => l.len() as f64,
                    _ => 0.0,
                };
                self.stack.push(Value::Number(n));
            },
            "to_str" => {
                let val = self.stack.pop().unwrap_or(Value::None);
                self.stack.push(Value::Str(val.display()));
            },
            "sin" => {
                let val = self.stack.pop().unwrap_or(Value::None);
                if let Value::Number(n) = val {
                    self.stack.push(Value::Number(n.sin()));
                } else {
                    self.stack.push(Value::Number(0.0));
                }
            },
            "cos" => {
                let val = self.stack.pop().unwrap_or(Value::None);
                if let Value::Number(n) = val {
                    self.stack.push(Value::Number(n.cos()));
                } else {
                    self.stack.push(Value::Number(0.0));
                }
            },
            _ => return Err(format!("unimplemented builtin: {}", name)),
        }
        Ok(())
    }
}

// ─── BytecodeBackend ─────────────────────────────────────────────────────────

pub struct BytecodeBackend;

impl CodegenBackend for BytecodeBackend {
    fn emit(&mut self, program: &crate::MirProgram, out: &std::path::Path) -> anyhow::Result<()> {
        let vm_prog = compile_mir_program(&program.mir);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"LINGBC");
        let fn_count = vm_prog.functions.len() as u32;
        bytes.extend_from_slice(&fn_count.to_le_bytes());
        for func in &vm_prog.functions {
            let name_bytes = func.name.as_bytes();
            bytes.extend_from_slice(&(name_bytes.len() as u32).to_le_bytes());
            bytes.extend_from_slice(name_bytes);
            bytes.extend_from_slice(&func.arg_count.to_le_bytes());
            bytes.extend_from_slice(&func.chunk.local_count.to_le_bytes());
            bytes.extend_from_slice(&(func.chunk.code.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&func.chunk.code);
            bytes.extend_from_slice(&(func.chunk.floats.len() as u32).to_le_bytes());
            for &f in &func.chunk.floats {
                bytes.extend_from_slice(&f.to_bits().to_le_bytes());
            }
            bytes.extend_from_slice(&(func.chunk.strings.len() as u32).to_le_bytes());
            for s in &func.chunk.strings {
                let sb = s.as_bytes();
                bytes.extend_from_slice(&(sb.len() as u32).to_le_bytes());
                bytes.extend_from_slice(sb);
            }
        }
        std::fs::write(out, &bytes)?;
        Ok(())
    }
}
