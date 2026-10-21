//! The MIR interpreter: runs a lowered [`MirProgram`] on the same NaN-boxed
//! values and runtime (`lingrt`, `runtime`, `lingapp`, `lingdaw`) that
//! AOT-compiled apps use, with the AOT translator's semantics
//! (`ling-codegen/src/cranelift/translate.rs`): numbers are f64 bits, every
//! builtin the AOT doesn't inline goes through `ling_builtin` by name, and the
//! inlined ones (print, maths, time, lists) call the same runtime symbols. So
//! `ling run file.ling` and a bundled app built from that file behave alike.
//! Beyond the AOT it has first-class function values: a function name used as
//! a value (a MIR function or a builtin) can be passed around and called.

use super::ast::{BinOp, UnOp};
use super::ir::*;
use crate::lingrt::{self, boolean, is_num, mk_str, num, str_of, truthy, UNIT};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Function values: a NaN-boxed kind of their own (unit 0x7F00, false 0x7F01,
/// true 0x7F02, strings 0x7F03, lists 0x7F04). The payload is a MIR function
/// index, or with `BUILTIN_BIT` an index into the interned builtin names.
const FUNC_TAG: u64 = 0x7F0F;
const BUILTIN_BIT: u64 = 1 << 31;

fn func_value(payload: u64) -> u64 {
    (FUNC_TAG << 48) | (payload & 0xFFFF_FFFF)
}

fn func_payload(v: u64) -> Option<u64> {
    if v >> 48 == FUNC_TAG {
        Some(v & 0xFFFF_FFFF)
    } else {
        None
    }
}

/// Ling call depth before the interpreter gives up (each level is a few
/// hundred bytes of kernel stack; tasks have 2 MiB).
const MAX_DEPTH: usize = 400;

pub struct Interp<'p> {
    prog: &'p MirProgram,
    by_name: BTreeMap<&'p str, usize>,
    /// Each distinct string literal boxed once (strings are immutable).
    strs: BTreeMap<&'p str, u64>,
    /// Builtin names that were used as values.
    names: Vec<&'p str>,
    depth: usize,
    /// Set by a runtime error; execution unwinds and `run` reports it.
    pub error: Option<String>,
}

impl<'p> Interp<'p> {
    pub fn new(prog: &'p MirProgram) -> Self {
        let mut by_name = BTreeMap::new();
        for (i, f) in prog.functions.iter().enumerate() {
            by_name.insert(f.name.as_str(), i);
        }
        Interp { prog, by_name, strs: BTreeMap::new(), names: Vec::new(), depth: 0, error: None }
    }

    /// Run `__main__` (the program body + global initialisers). Returns its value.
    pub fn run(&mut self) -> u64 {
        let Some(&main) = self.by_name.get("__main__") else {
            self.error = Some(String::from("no __main__ (nothing to run)"));
            return UNIT;
        };
        self.call(main, &[])
    }

    fn fail(&mut self, msg: String) -> u64 {
        if self.error.is_none() {
            self.error = Some(msg);
        }
        UNIT
    }

    fn str_const(&mut self, s: &'p str) -> u64 {
        if let Some(&v) = self.strs.get(s) {
            return v;
        }
        let v = mk_str(s);
        self.strs.insert(s, v);
        v
    }

    fn func_const(&mut self, name: &'p str) -> u64 {
        if let Some(&i) = self.by_name.get(name) {
            return func_value(i as u64);
        }
        let idx = match self.names.iter().position(|n| *n == name) {
            Some(i) => i,
            None => {
                self.names.push(name);
                self.names.len() - 1
            },
        };
        func_value(BUILTIN_BIT | idx as u64)
    }

    fn operand(&mut self, op: &'p Operand, locals: &[u64]) -> u64 {
        match op {
            Operand::Copy(l) | Operand::Move(l) => locals.get(l.0).copied().unwrap_or(UNIT),
            Operand::Constant(c) => match c {
                Constant::I64(v) => num(*v as f64),
                Constant::F64(bits) => *bits,
                Constant::Bool(b) => boolean(*b),
                Constant::Str(s) => self.str_const(s),
                Constant::Function(name) => self.func_const(name),
                Constant::GlobalData(_) | Constant::None => UNIT,
            },
        }
    }

    /// Call MIR function `idx` with boxed `args` (missing ones are unit).
    pub fn call(&mut self, idx: usize, args: &[u64]) -> u64 {
        if self.error.is_some() {
            return UNIT;
        }
        let f: &'p MirFunction = &self.prog.functions[idx];
        if self.depth >= MAX_DEPTH {
            return self.fail(format!("call depth over {MAX_DEPTH} in {}()", f.name));
        }
        self.depth += 1;
        let mut locals = vec![UNIT; (f.arg_count + 1 + f.locals.len()).max(max_local(f) + 1)];
        for i in 0..f.arg_count {
            locals[i + 1] = args.get(i).copied().unwrap_or(UNIT);
        }
        let mut bb = 0usize;
        'blocks: loop {
            let Some(block) = f.basic_blocks.get(bb) else { break };
            for stmt in &block.statements {
                match &stmt.kind {
                    StatementKind::Assign(dst, rv) => {
                        let v = self.rvalue(rv, &locals);
                        if dst.0 < locals.len() {
                            locals[dst.0] = v;
                        }
                    },
                    StatementKind::SetIndex(obj, idx, val) => {
                        let o = self.operand(obj, &locals);
                        let i = self.operand(idx, &locals);
                        let v = self.operand(val, &locals);
                        lingrt::ling_list_set(o, i, v);
                    },
                    _ => {},
                }
                if self.error.is_some() {
                    break 'blocks;
                }
            }
            match block.terminator.as_ref().map(|t| &t.kind) {
                Some(TerminatorKind::Goto { target }) => bb = target.0,
                Some(TerminatorKind::SwitchInt { discr, targets, otherwise }) => {
                    // As the AOT branches: the `1` target when the discriminant
                    // is truthy, the `0` target (else `otherwise`) when not.
                    let v = self.operand(discr, &locals);
                    let t = truthy(v);
                    let mut next = otherwise.0;
                    for (c, target) in targets {
                        if (*c != 0) == t {
                            next = target.0;
                            break;
                        }
                    }
                    bb = next;
                },
                Some(TerminatorKind::Return) | Some(TerminatorKind::Unreachable) | None => break,
            }
        }
        self.depth -= 1;
        locals[0]
    }

    fn rvalue(&mut self, rv: &'p Rvalue, locals: &[u64]) -> u64 {
        match rv {
            Rvalue::Use(op) => self.operand(op, locals),
            Rvalue::BinaryOp(op, l, r) => {
                let a = self.operand(l, locals);
                let b = self.operand(r, locals);
                binop(*op, a, b)
            },
            Rvalue::UnaryOp(op, o) => {
                let v = self.operand(o, locals);
                match op {
                    UnOp::Neg => {
                        if is_num(v) {
                            num(-f64::from_bits(v))
                        } else {
                            crate::runtime::ling_neg(v)
                        }
                    },
                    UnOp::Not => boolean(!truthy(v)),
                    UnOp::Ref | UnOp::Deref => v,
                }
            },
            Rvalue::Call { func, args } => {
                let mut vals: Vec<u64> = Vec::with_capacity(args.len());
                for a in args {
                    vals.push(self.operand(a, locals));
                }
                match func {
                    Operand::Constant(Constant::Function(name)) => self.call_named(name.as_str(), &vals),
                    other => {
                        let f = self.operand(other, locals);
                        self.call_value(f, &vals)
                    },
                }
            },
            Rvalue::Aggregate(_, ops) => {
                let mut list = lingrt::ling_list_new();
                for op in ops {
                    let v = self.operand(op, locals);
                    list = lingrt::ling_list_push(list, v);
                }
                list
            },
            Rvalue::GetIndex(obj, idx) => {
                let o = self.operand(obj, locals);
                let i = self.operand(idx, locals);
                lingrt::ling_list_get(o, i)
            },
            Rvalue::Ref(l) | Rvalue::MutRef(l) => locals.get(l.0).copied().unwrap_or(UNIT),
            Rvalue::InlineAsm(_) => {
                lingrt::report_missing(b"asm (inline assembly is compiled, not interpreted)");
                UNIT
            },
            // Attribute access and vector ops have no boxed-value form (the AOT
            // yields unit for them too).
            Rvalue::GetAttr(..) | Rvalue::VectorSplat(..) | Rvalue::VectorLoad(..) | Rvalue::VectorFMA(..) => UNIT,
        }
    }

    /// Call a function value (see `func_value`).
    fn call_value(&mut self, f: u64, args: &[u64]) -> u64 {
        match func_payload(f) {
            Some(p) if p & BUILTIN_BIT != 0 => {
                let name = self.names[(p & !BUILTIN_BIT) as usize];
                self.call_named(name, args)
            },
            Some(p) => self.call(p as usize, args),
            None => self.fail(format!("call of a value that is not a function ({})", lingrt::to_string(f))),
        }
    }

    /// A call by name: a MIR function, a kernel intrinsic, one of the builtins
    /// the AOT inlines, or the generic `ling_builtin` dispatcher.
    fn call_named(&mut self, name: &'p str, args: &[u64]) -> u64 {
        if let Some(&i) = self.by_name.get(name) {
            return self.call(i, args);
        }
        if name.starts_with("ling_kernel_") || name.starts_with("ling_sys_") {
            return kernel_call(name, args);
        }
        let f1 = |f: fn(f64) -> f64| -> u64 {
            match args.first() {
                Some(&v) if is_num(v) => num(f(f64::from_bits(v))),
                Some(&v) => v,
                None => UNIT,
            }
        };
        match name {
            "print" | "println" | "พิมพ์" | "印" | "打印" | "印刷" | "출력" => {
                for &a in args {
                    lingrt::ling_print_val(a);
                }
                lingrt::ling_print_newline()
            },
            "sin" => f1(libm::sin),
            "cos" => f1(libm::cos),
            "sqrt" => f1(libm::sqrt),
            "abs" => f1(libm::fabs),
            "floor" => f1(libm::floor),
            "ceil" => f1(libm::ceil),
            "round" => f1(libm::round),
            "time_now" | "เวลาปัจจุบัน" | "当前时间" | "経過時間" | "현재시간" => lingrt::ling_time_now(),
            "list_new" | "รายการใหม่" | "新建列表" | "新規リスト" | "새목록" => lingrt::ling_list_new(),
            "list_push" | "เพิ่มรายการ" | "列表添加" | "リスト追加" | "목록추가" if args.len() >= 2 => {
                lingrt::ling_list_push(args[0], args[1])
            },
            "list_get" | "รับรายการ" | "取元素" | "要素取得" | "요소가져오기" if args.len() >= 2 => {
                lingrt::ling_list_get(args[0], args[1])
            },
            "list_set" | "ตั้งรายการ" | "设元素" | "要素設定" | "요소설정" if args.len() >= 3 => {
                lingrt::ling_list_set(args[0], args[1], args[2])
            },
            _ => unsafe { lingrt::ling_builtin(name.as_ptr(), name.len(), args.as_ptr(), args.len()) },
        }
    }
}

/// The highest local index a function touches (the lowering's temporaries can
/// outnumber its `locals` declarations).
fn max_local(f: &MirFunction) -> usize {
    let mut m = f.arg_count;
    let see = |op: &Operand, m: &mut usize| {
        if let Operand::Copy(l) | Operand::Move(l) = op {
            *m = (*m).max(l.0);
        }
    };
    for bb in &f.basic_blocks {
        for s in &bb.statements {
            match &s.kind {
                StatementKind::Assign(l, rv) => {
                    m = m.max(l.0);
                    match rv {
                        Rvalue::Use(o) | Rvalue::UnaryOp(_, o) | Rvalue::GetAttr(o, _) => see(o, &mut m),
                        Rvalue::BinaryOp(_, a, b) | Rvalue::GetIndex(a, b) => {
                            see(a, &mut m);
                            see(b, &mut m);
                        },
                        Rvalue::Call { func, args } => {
                            see(func, &mut m);
                            for a in args {
                                see(a, &mut m);
                            }
                        },
                        Rvalue::Aggregate(_, ops) => {
                            for o in ops {
                                see(o, &mut m);
                            }
                        },
                        Rvalue::Ref(l) | Rvalue::MutRef(l) => m = m.max(l.0),
                        _ => {},
                    }
                },
                StatementKind::SetIndex(a, b, c) => {
                    see(a, &mut m);
                    see(b, &mut m);
                    see(c, &mut m);
                },
                _ => {},
            }
        }
        if let Some(t) = &bb.terminator {
            if let TerminatorKind::SwitchInt { discr, .. } = &t.kind {
                see(discr, &mut m);
            }
        }
    }
    m
}

/// A binary operator with the AOT's semantics: plain f64 maths when both sides
/// are numbers, else the runtime's typed fallbacks (string concatenation, list
/// and string comparison...).
fn binop(op: BinOp, a: u64, b: u64) -> u64 {
    use crate::runtime as rt;
    if is_num(a) && is_num(b) {
        let (x, y) = (f64::from_bits(a), f64::from_bits(b));
        return match op {
            BinOp::Add => num(x + y),
            BinOp::Sub => num(x - y),
            BinOp::Mul => num(x * y),
            BinOp::Div => num(x / y),
            BinOp::Rem => num(libm::fmod(x, y)),
            BinOp::Eq => boolean(x == y),
            BinOp::Ne => boolean(x != y),
            BinOp::Lt => boolean(x < y),
            BinOp::Le => boolean(x <= y),
            BinOp::Gt => boolean(x > y),
            BinOp::Ge => boolean(x >= y),
            BinOp::And => boolean(x != 0.0 && y != 0.0),
            BinOp::Or => boolean(x != 0.0 || y != 0.0),
        };
    }
    {
        match op {
            BinOp::Add => rt::ling_add(a, b),
            BinOp::Sub => rt::ling_sub(a, b),
            BinOp::Mul => rt::ling_mul(a, b),
            BinOp::Div => rt::ling_div(a, b),
            BinOp::Rem => rt::ling_rem(a, b),
            BinOp::Eq => rt::ling_eq(a, b),
            BinOp::Ne => rt::ling_ne(a, b),
            BinOp::Lt => rt::ling_lt(a, b),
            BinOp::Le => rt::ling_le(a, b),
            BinOp::Gt => rt::ling_gt(a, b),
            BinOp::Ge => rt::ling_ge(a, b),
            BinOp::And => boolean(truthy(a) && truthy(b)),
            BinOp::Or => boolean(truthy(a) || truthy(b)),
        }
    }
}

/// The kernel intrinsics a script may reasonably call (the rest of the
/// `ling_kernel_*` surface is for the kernel's own entry programs); unknown
/// ones report once and yield unit.
fn kernel_call(name: &str, args: &[u64]) -> u64 {
    match name {
        "ling_kernel_vga_write_str" | "ling_kernel_serial_write" | "ling_kernel_console_write" => {
            if let Some(&v) = args.first() {
                crate::console_write(str_of(v).as_bytes());
            }
            num(0.0)
        },
        "ling_kernel_uptime_ms" => num(crate::arch::timer::now_ms() as f64),
        "ling_kernel_init" | "ling_kernel_halt" => num(0.0),
        _ => {
            lingrt::report_missing(name.as_bytes());
            UNIT
        },
    }
}
