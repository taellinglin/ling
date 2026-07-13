use crate::core::{LingError, LingResult, OptimizationLevel};
use crate::parser;
use ling_ast::Span;
use ling_mir::ir::*;
use ling_mir::optimizer::{OptLevel, Optimizer};
use std::collections::HashMap;
use std::path::Path;

mod modules;

/// Parse source, lower to MIR, run optimizer, return optimized MIR.
pub fn compile_and_optimize(source: &str, opt_level: OptimizationLevel) -> LingResult<MirProgram> {
    let ast = parser::parse(source).map_err(|e| LingError::Parse(e))?;

    // Run semantic analysis (type checking)
    // let mut semantic = crate::semantic::SemanticAnalyzer::new();
    // semantic.analyze(&ast).map_err(|e| {
    //     eprintln!("Type error: {}", e);
    //     e
    // })?;

    let mir = lower_program(&ast);
    Ok(optimize(mir, opt_level))
}

/// Like [`compile_and_optimize`], but resolves `use` imports and `mod` blocks
/// relative to the entry file's directory before lowering. This is the
/// multi-file entry point for the AOT/JIT backends.
pub fn compile_path(entry: &Path, opt_level: OptimizationLevel) -> LingResult<MirProgram> {
    let flat = flatten_path(entry)?;
    let mir = lower_program(&flat);
    Ok(optimize(mir, opt_level))
}

/// Resolve `use`/`mod` imports for `entry` and return the flattened AST. Used by
/// the JIT to prime its fallback interpreter with the same program it compiles.
pub fn flatten_path(entry: &Path) -> LingResult<parser::ast::Program> {
    let source = std::fs::read_to_string(entry)
        .map_err(|e| LingError::Io(format!("error reading '{}': {e}", entry.display())))?;
    let ast = parser::parse(&source).map_err(LingError::Parse)?;
    let entry_dir = entry.parent().unwrap_or(Path::new("."));
    let flat = modules::flatten(&ast, entry_dir)?;
    Ok(dedup_items(flat))
}

/// Flatten a concatenated source string against `source_dir`, resolving its
/// `use` imports and deduplicating definitions. A host that pastes several files
/// together (e.g. the game launcher) and whose entry also `use`s some of them
/// would otherwise produce duplicate top-level functions; the JIT requires each
/// symbol once, so the last definition of each name wins (matching the
/// tree-walker, which registers into a map).
pub fn flatten_source(source: &str, source_dir: &Path) -> LingResult<parser::ast::Program> {
    let ast = parser::parse(source).map_err(LingError::Parse)?;
    let flat = modules::flatten(&ast, source_dir)?;
    Ok(dedup_items(flat))
}

/// Keep the last definition of every top-level `fn`/`bind` name, preserving its
/// position; drop earlier shadowed copies. Other items pass through untouched.
fn dedup_items(prog: parser::ast::Program) -> parser::ast::Program {
    use parser::ast::Item;
    let mut last: HashMap<(u8, String), usize> = HashMap::new();
    for (i, item) in prog.items.iter().enumerate() {
        let key = match item {
            Item::Fn(def) => Some((0u8, def.name.clone())),
            Item::Bind(name, _) => Some((1u8, name.clone())),
            _ => None,
        };
        if let Some(k) = key {
            last.insert(k, i);
        }
    }
    let items = prog
        .items
        .into_iter()
        .enumerate()
        .filter(|(i, item)| {
            let key = match item {
                Item::Fn(def) => Some((0u8, def.name.clone())),
                Item::Bind(name, _) => Some((1u8, name.clone())),
                _ => None,
            };
            match key {
                Some(k) => last.get(&k) == Some(i),
                None => true,
            }
        })
        .map(|(_, item)| item)
        .collect();
    parser::ast::Program { items }
}

/// Lower an already-flattened AST to optimized MIR. Shared by the path- and
/// source-based JIT entry points so both compile exactly the program their
/// fallback interpreter is primed with.
pub fn lower_and_optimize(prog: &parser::ast::Program, opt_level: OptimizationLevel) -> MirProgram {
    optimize(lower_program(prog), opt_level)
}

fn optimize(mut mir: MirProgram, opt_level: OptimizationLevel) -> MirProgram {
    if !matches!(opt_level, OptimizationLevel::None) {
        let mir_opt = match opt_level {
            OptimizationLevel::None => OptLevel::None,
            OptimizationLevel::O1 => OptLevel::O1,
            OptimizationLevel::O2 => OptLevel::O2,
            OptimizationLevel::O3 => OptLevel::O3,
        };
        Optimizer::new(mir_opt).run(&mut mir.functions);
    }
    mir
}

// ─── AST → MIR lowering ──────────────────────────────────────────────────────

fn lower_program(prog: &parser::ast::Program) -> MirProgram {
    let entry = crate::entry::entry_name(&prog.items);

    // Module globals: every top-level bind that is not the entry. Functions read
    // these via `__ling_global` against the runtime's evaluated snapshot.
    let globals: std::rc::Rc<std::collections::HashSet<String>> = std::rc::Rc::new(
        prog.items
            .iter()
            .filter_map(|item| match item {
                parser::ast::Item::Bind(name, _) => Some(name.clone()),
                _ => None,
            })
            .filter(|name| entry.as_deref() != Some(name.as_str()))
            .collect(),
    );

    let mut functions = Vec::new();
    for item in &prog.items {
        if let parser::ast::Item::Fn(fndef) = item {
            let (func, closures) = lower_function(fndef, globals.clone());
            functions.push(func);
            functions.extend(closures);
        }
    }

    // Lower the entry binding as the main function body. When an entry exists
    // (a known entry name or a `do { }` bind), only it becomes `__main__`; the
    // other top-level binds are globals (resolved at runtime, see above).
    // With no entry, every top-level bind is treated as the script body.
    let mut main_stmts: Vec<parser::ast::Stmt> = Vec::new();
    for item in &prog.items {
        if let parser::ast::Item::Bind(name, body) = item {
            let is_main = match &entry {
                Some(e) => name == e || name == "__main__",
                None => true,
            };
            if is_main {
                main_stmts.push(parser::ast::Stmt::Bind(name.clone(), body.clone()));
            }
        }
    }

    if !main_stmts.is_empty() {
        let mut main = MirFunction::new("__main__", 0);
        let mut lctx = LowerCtx::new(&mut main, 0, globals.clone());
        let last = lower_stmts(&main_stmts, &mut lctx);
        lctx.finish(last);
        let closure_fns = std::mem::take(&mut lctx.closures);
        functions.push(main);
        functions.extend(closure_fns);
    } else if functions.is_empty() {
        let mut main = MirFunction::new("__main__", 0);
        main.basic_blocks[0].statements.push(Statement {
            kind: StatementKind::Assign(Local(0), Rvalue::Use(Operand::Constant(Constant::None))),
            span: Span::DUMMY,
        });
        functions.push(main);
    }
    MirProgram { functions }
}

fn lower_function(
    fndef: &parser::ast::FnDef,
    globals: std::rc::Rc<std::collections::HashSet<String>>,
) -> (MirFunction, Vec<MirFunction>) {
    let arg_count = fndef.params.len();
    let param_names = fndef.params.clone();
    let mut func = MirFunction::new(&fndef.name, arg_count);
    func.param_names = param_names;

    let mut lctx = LowerCtx::new(&mut func, arg_count, globals);

    for (i, pname) in fndef.params.iter().enumerate() {
        lctx.declare_in_scope(pname, Local(i + 1));
    }

    let last = lower_stmts(&fndef.body, &mut lctx);
    lctx.finish(last);
    let closures = std::mem::take(&mut lctx.closures);
    (func, closures)
}

#[derive(Clone)]
struct ClosureInfo {
    func_name: String,
    captures: Vec<String>,
}

struct LowerCtx<'a> {
    func: &'a mut MirFunction,
    locals: HashMap<String, Local>,
    next_local: usize,
    /// Block currently being filled. All `emit`/`set_term` target this block,
    /// and control-flow lowering advances it so constructs compose cleanly.
    current: BasicBlockId,
    /// Names bound in each active lexical scope (innermost last). Re-binding a
    /// name in the same scope mutates it; binding a name owned by an outer scope
    /// shadows it. Only `for` loops open a fresh scope (matching the tree-walker,
    /// where `while`/`if` bodies share the surrounding scope).
    scope_names: Vec<std::collections::HashSet<String>>,
    /// Per-scope undo log of bindings shadowed on entry, restored on scope exit.
    shadowed: Vec<Vec<(String, Option<Local>)>>,
    closures: Vec<MirFunction>,
    closure_vars: HashMap<String, ClosureInfo>,
    /// Module-level global names. A read of one of these (when not a local) is
    /// lowered to a `__ling_global` call resolved against the runtime's evaluated
    /// global snapshot, matching the tree-walker's read-only global semantics.
    globals: std::rc::Rc<std::collections::HashSet<String>>,
}

impl<'a> LowerCtx<'a> {
    fn new(
        func: &'a mut MirFunction,
        arg_count: usize,
        globals: std::rc::Rc<std::collections::HashSet<String>>,
    ) -> Self {
        let next = (arg_count + 1) as usize;
        // bb0 already exists; clear its terminator — lowering fills it in.
        func.basic_blocks[0].terminator = None;
        Self {
            func,
            locals: HashMap::new(),
            next_local: next,
            current: BasicBlockId(0),
            scope_names: vec![std::collections::HashSet::new()],
            shadowed: vec![Vec::new()],
            closures: Vec::new(),
            closure_vars: HashMap::new(),
            globals,
        }
    }

    /// Register a name already mapped to `local` as owned by the current scope
    /// (used for parameters and loop counters).
    fn declare_in_scope(&mut self, name: &str, local: Local) {
        self.locals.insert(name.to_string(), local);
        self.scope_names
            .last_mut()
            .unwrap()
            .insert(name.to_string());
    }

    /// Resolve the local a `bind name = …` should target. Re-binding within the
    /// same scope reuses the local (mutation); otherwise a fresh local shadows the
    /// outer one until the scope exits.
    fn bind_local(&mut self, name: &str) -> Local {
        if self.scope_names.last().unwrap().contains(name) {
            return self.locals[name];
        }
        let prev = self.locals.get(name).copied();
        let l = self.alloc_local(Some(name.to_string()), true);
        self.locals.insert(name.to_string(), l);
        self.scope_names
            .last_mut()
            .unwrap()
            .insert(name.to_string());
        self.shadowed
            .last_mut()
            .unwrap()
            .push((name.to_string(), prev));
        l
    }

    fn enter_scope(&mut self) {
        self.scope_names.push(std::collections::HashSet::new());
        self.shadowed.push(Vec::new());
    }

    fn exit_scope(&mut self) {
        for (name, prev) in self.shadowed.pop().unwrap().into_iter().rev() {
            match prev {
                Some(l) => {
                    self.locals.insert(name, l);
                },
                None => {
                    self.locals.remove(&name);
                },
            }
        }
        self.scope_names.pop();
    }

    fn alloc_local(&mut self, name: Option<String>, is_mut: bool) -> Local {
        let l = Local(self.next_local);
        self.next_local += 1;
        self.func.locals.push(LocalDecl {
            ty: MirType::Any,
            name,
            span: Span::DUMMY,
            is_mut,
            is_owning: true,
        });
        l
    }

    /// Append a fresh, empty (unterminated) block and return its id.
    fn new_block(&mut self) -> BasicBlockId {
        let id = BasicBlockId(self.func.basic_blocks.len());
        self.func
            .basic_blocks
            .push(BasicBlock { statements: Vec::new(), terminator: None });
        id
    }

    fn switch(&mut self, bb: BasicBlockId) {
        self.current = bb;
    }

    fn emit(&mut self, kind: StatementKind) {
        self.func.basic_blocks[self.current.0]
            .statements
            .push(Statement { kind, span: Span::DUMMY });
    }

    fn set_term(&mut self, kind: TerminatorKind) {
        self.func.basic_blocks[self.current.0].terminator =
            Some(Terminator { kind, span: Span::DUMMY });
    }

    fn term_is_set(&self) -> bool {
        self.func.basic_blocks[self.current.0].terminator.is_some()
    }

    /// Close the function: store the trailing value into the return slot (Local 0)
    /// and return, unless the current block already ended (e.g. an explicit return).
    fn finish(&mut self, last: Option<Operand>) {
        if self.term_is_set() {
            return;
        }
        if let Some(v) = last {
            self.emit(StatementKind::Assign(Local(0), Rvalue::Use(v)));
        }
        self.set_term(TerminatorKind::Return);
    }
}

/// Lower a statement sequence, returning the value of the trailing expression
/// statement (used when a block is in expression position).
fn lower_stmts(stmts: &[parser::ast::Stmt], ctx: &mut LowerCtx) -> Option<Operand> {
    let mut last = None;
    for stmt in stmts {
        last = lower_stmt(stmt, ctx);
    }
    last
}

fn lower_stmt(stmt: &parser::ast::Stmt, ctx: &mut LowerCtx) -> Option<Operand> {
    match stmt {
        parser::ast::Stmt::Bind(name, expr) => {
            // Detect closure binding with captures
            if let parser::ast::Expr::Closure(params, body) = expr {
                let free_vars = collect_free_vars(body.as_ref(), params.as_ref());
                if !free_vars.is_empty() {
                    let closure_id = ctx.closures.len();
                    let closure_name = format!("__closure_{}", closure_id);
                    let capture_count = free_vars.len();
                    let arg_count = params.len();
                    let total_args = arg_count + capture_count;
                    let mut closure_func = MirFunction::new(&closure_name, total_args);
                    closure_func.param_names = params.clone();
                    let mut closure_ctx =
                        LowerCtx::new(&mut closure_func, total_args, ctx.globals.clone());
                    for (i, pname) in params.iter().enumerate() {
                        closure_ctx.declare_in_scope(pname, Local(i + 1));
                    }
                    for (ci, fv) in free_vars.iter().enumerate() {
                        closure_ctx
                            .locals
                            .insert(fv.clone(), Local(arg_count + 1 + ci));
                    }
                    let body_val = lower_expr(body.as_ref(), &mut closure_ctx);
                    closure_ctx.emit(StatementKind::Assign(Local(0), Rvalue::Use(body_val)));
                    closure_ctx.set_term(TerminatorKind::Return);
                    ctx.closures.push(closure_func);

                    ctx.closure_vars.insert(
                        name.clone(),
                        ClosureInfo {
                            func_name: closure_name.clone(),
                            captures: free_vars.clone(),
                        },
                    );

                    let local = ctx.bind_local(name);
                    ctx.emit(StatementKind::Assign(
                        local,
                        Rvalue::Use(Operand::Constant(Constant::Function(closure_name))),
                    ));
                    return None;
                }
            }
            let val = lower_expr(expr, ctx);
            // Re-binding within the same scope mutates the same local (so loop
            // bodies feed accumulators across the back-edge); binding a name owned
            // by an outer scope shadows it for the duration of this scope.
            let local = ctx.bind_local(name);
            ctx.emit(StatementKind::Assign(local, Rvalue::Use(val)));
            None
        },
        parser::ast::Stmt::Expr(expr) => Some(lower_expr(expr, ctx)),
        parser::ast::Stmt::Return(expr) => {
            let val = lower_expr(expr, ctx);
            ctx.emit(StatementKind::Assign(Local(0), Rvalue::Use(val)));
            ctx.set_term(TerminatorKind::Return);
            // Anything after a return is dead; continue in a fresh block so later
            // lowering has a valid cursor.
            let dead = ctx.new_block();
            ctx.switch(dead);
            None
        },
    }
}

fn lower_expr(expr: &parser::ast::Expr, ctx: &mut LowerCtx) -> Operand {
    match expr {
        parser::ast::Expr::Number(n) => Operand::Constant(Constant::F64(n.to_bits())),
        parser::ast::Expr::Str(s) => {
            let local = ctx.alloc_local(None, false);
            ctx.emit(StatementKind::StorageLive(local));
            ctx.emit(StatementKind::Assign(
                local,
                Rvalue::Use(Operand::Constant(Constant::Str(s.clone()))),
            ));
            Operand::Copy(local)
        },
        parser::ast::Expr::Bool(b) => Operand::Constant(Constant::Bool(*b)),
        parser::ast::Expr::Unit => Operand::Constant(Constant::None),
        parser::ast::Expr::Ident(name) => {
            if let Some(&local) = ctx.locals.get(name) {
                Operand::Copy(local)
            } else if ctx.globals.contains(name) {
                // Module global read → resolve against the runtime snapshot.
                let local = ctx.alloc_local(None, false);
                ctx.emit(StatementKind::Assign(
                    local,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function("__ling_global".to_string())),
                        args: vec![Operand::Constant(Constant::Str(name.clone()))],
                    },
                ));
                Operand::Copy(local)
            } else {
                // Unknown names in expression context: treat as function constant
                Operand::Constant(Constant::Function(name.clone()))
            }
        },
        parser::ast::Expr::BinOp(op, lhs, rhs) => {
            let l = lower_expr(lhs, ctx);
            let r = lower_expr(rhs, ctx);
            let local = ctx.alloc_local(None, false);
            ctx.emit(StatementKind::Assign(
                local,
                Rvalue::BinaryOp(lower_binop(op.clone()), l, r),
            ));
            Operand::Copy(local)
        },
        parser::ast::Expr::Call(callee, args) => {
            // Detect inline closure call with captures
            if let parser::ast::Expr::Closure(params, body) = callee.as_ref() {
                let free_vars = collect_free_vars(body, params);
                let capture_count = free_vars.len();
                let arg_count = params.len();
                let closure_id = ctx.closures.len();
                let closure_name = format!("__closure_{}", closure_id);
                let mut closure_func = MirFunction::new(&closure_name, arg_count + capture_count);
                closure_func.param_names = params.clone();
                let mut closure_ctx = LowerCtx::new(
                    &mut closure_func,
                    arg_count + capture_count,
                    ctx.globals.clone(),
                );
                for (i, pname) in params.iter().enumerate() {
                    let local = Local(i + 1);
                    closure_ctx.declare_in_scope(pname, local);
                }
                // Map captures to extra params
                for (ci, fv) in free_vars.iter().enumerate() {
                    let param_local = Local(arg_count + 1 + ci);
                    closure_ctx.declare_in_scope(fv, param_local);
                }
                let body_val = lower_expr(body, &mut closure_ctx);
                closure_ctx.emit(StatementKind::Assign(Local(0), Rvalue::Use(body_val)));
                closure_ctx.set_term(TerminatorKind::Return);
                ctx.closures.push(closure_func);

                let mut mir_args = Vec::new();
                for arg in args {
                    mir_args.push(lower_expr(arg, ctx));
                }
                // Pass captured values from enclosing scope
                for fv in &free_vars {
                    if let Some(&local) = ctx.locals.get(fv) {
                        mir_args.push(Operand::Copy(local));
                    } else {
                        mir_args.push(Operand::Constant(Constant::None));
                    }
                }
                let local = ctx.alloc_local(None, false);
                ctx.emit(StatementKind::Assign(
                    local,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(closure_name)),
                        args: mir_args,
                    },
                ));
                Operand::Copy(local)
            } else {
                // Check for two-step closure call: f(5) where f is a closure binding
                if let parser::ast::Expr::Ident(name) = callee.as_ref() {
                    if let Some(ci) = ctx.closure_vars.get(name).cloned() {
                        let mut mir_args = Vec::new();
                        for arg in args {
                            mir_args.push(lower_expr(arg, ctx));
                        }
                        // Append captured values from enclosing scope
                        for fv in &ci.captures {
                            if let Some(&local) = ctx.locals.get(fv) {
                                mir_args.push(Operand::Copy(local));
                            } else {
                                mir_args.push(Operand::Constant(Constant::None));
                            }
                        }
                        let local = ctx.alloc_local(None, false);
                        ctx.emit(StatementKind::Assign(
                            local,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(ci.func_name.clone())),
                                args: mir_args,
                            },
                        ));
                        return Operand::Copy(local);
                    }
                }

                let callee_op = lower_expr(callee, ctx);
                let mut mir_args = Vec::new();
                for arg in args {
                    mir_args.push(lower_expr(arg, ctx));
                }
                let local = ctx.alloc_local(None, false);
                ctx.emit(StatementKind::Assign(
                    local,
                    Rvalue::Call { func: callee_op, args: mir_args },
                ));
                Operand::Copy(local)
            }
        },
        parser::ast::Expr::If { cond, then, elseifs, else_body } => {
            let result = ctx.alloc_local(None, true);
            let merge = ctx.new_block();
            lower_if_chain(cond, then, elseifs, else_body, result, merge, ctx);
            ctx.switch(merge);
            Operand::Copy(result)
        },
        parser::ast::Expr::While { cond, body } => {
            let header = ctx.new_block();
            let body_block = ctx.new_block();
            let exit = ctx.new_block();
            ctx.set_term(TerminatorKind::Goto { target: header });

            // Header re-evaluates the condition on every iteration (including after
            // the back-edge), so re-bound loop variables are observed each time.
            ctx.switch(header);
            let cond_op = lower_expr(cond, ctx);
            let test = ctx.alloc_local(None, false);
            ctx.emit(StatementKind::Assign(test, Rvalue::Use(cond_op)));
            ctx.set_term(TerminatorKind::SwitchInt {
                discr: Operand::Copy(test),
                targets: vec![(1, body_block)],
                otherwise: exit,
            });

            ctx.switch(body_block);
            lower_stmts(body, ctx);
            if !ctx.term_is_set() {
                ctx.set_term(TerminatorKind::Goto { target: header });
            }

            ctx.switch(exit);
            Operand::Constant(Constant::None)
        },
        parser::ast::Expr::For { var, iter, body } => {
            lower_for(var, iter, body, ctx);
            Operand::Constant(Constant::None)
        },
        parser::ast::Expr::Array(elems) => {
            let mut ops = Vec::new();
            for e in elems {
                ops.push(lower_expr(e, ctx));
            }
            let local = ctx.alloc_local(None, false);
            ctx.emit(StatementKind::Assign(
                local,
                Rvalue::Aggregate(AggregateKind::List, ops),
            ));
            Operand::Copy(local)
        },
        parser::ast::Expr::Index(base, idx) => {
            let b = lower_expr(base, ctx);
            let i = lower_expr(idx, ctx);
            let local = ctx.alloc_local(None, false);
            ctx.emit(StatementKind::Assign(local, Rvalue::GetIndex(b, i)));
            Operand::Copy(local)
        },
        parser::ast::Expr::Do(stmts) => {
            lower_stmts(stmts, ctx).unwrap_or(Operand::Constant(Constant::None))
        },
        // `&x` is value-identity in Ling (the tree-walker evaluates the inner
        // expression and returns it unchanged); values are NaN-boxed and passed
        // by value, so the reference lowers to the inner operand directly.
        parser::ast::Expr::Ref(inner) => lower_expr(inner, ctx),
        parser::ast::Expr::MethodCall { receiver, method, args } => {
            let recv = lower_expr(receiver, ctx);
            let mut mir_args = vec![recv];
            for arg in args {
                mir_args.push(lower_expr(arg, ctx));
            }
            let fn_name = format!("{}.{}", method_name_from_expr(receiver), method);
            let local = ctx.alloc_local(None, false);
            ctx.emit(StatementKind::Assign(
                local,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(fn_name)),
                    args: mir_args,
                },
            ));
            Operand::Copy(local)
        },
        parser::ast::Expr::Path(parts) => Operand::Constant(Constant::Function(parts.join("::"))),
        parser::ast::Expr::Range(lo, hi) => {
            let l = lower_expr(lo, ctx);
            let h = lower_expr(hi, ctx);
            let local = ctx.alloc_local(None, false);
            ctx.emit(StatementKind::Assign(
                local,
                Rvalue::Aggregate(AggregateKind::List, vec![l, h]),
            ));
            Operand::Copy(local)
        },
        parser::ast::Expr::Match(scrutinee, arms) => {
            let scrut_op = lower_expr(scrutinee, ctx);
            let scrut_local = ctx.alloc_local(None, false);
            ctx.emit(StatementKind::Assign(scrut_local, Rvalue::Use(scrut_op)));

            let result = ctx.alloc_local(None, true);
            ctx.emit(StatementKind::Assign(
                result,
                Rvalue::Use(Operand::Constant(Constant::None)),
            ));
            let merge = ctx.new_block();

            let mut matched_all = false;
            for arm in arms {
                match &arm.pattern {
                    parser::ast::Pattern::Wildcard => {
                        let v = lower_expr(&arm.body, ctx);
                        ctx.emit(StatementKind::Assign(result, Rvalue::Use(v)));
                        ctx.set_term(TerminatorKind::Goto { target: merge });
                        matched_all = true;
                        break;
                    },
                    parser::ast::Pattern::Ident(name) => {
                        let bound = ctx.bind_local(name);
                        ctx.emit(StatementKind::Assign(
                            bound,
                            Rvalue::Use(Operand::Copy(scrut_local)),
                        ));
                        let v = lower_expr(&arm.body, ctx);
                        ctx.emit(StatementKind::Assign(result, Rvalue::Use(v)));
                        ctx.set_term(TerminatorKind::Goto { target: merge });
                        matched_all = true;
                        break;
                    },
                    parser::ast::Pattern::Str(_)
                    | parser::ast::Pattern::Number(_)
                    | parser::ast::Pattern::Bool(_) => {
                        let lit_op = pattern_to_operand(&arm.pattern);
                        let cmp = ctx.alloc_local(None, false);
                        ctx.emit(StatementKind::Assign(
                            cmp,
                            Rvalue::BinaryOp(
                                ling_ast::ast::BinOp::Eq,
                                Operand::Copy(scrut_local),
                                lit_op,
                            ),
                        ));
                        let arm_bb = ctx.new_block();
                        let next = ctx.new_block();
                        ctx.set_term(TerminatorKind::SwitchInt {
                            discr: Operand::Copy(cmp),
                            targets: vec![(1, arm_bb)],
                            otherwise: next,
                        });
                        ctx.switch(arm_bb);
                        let v = lower_expr(&arm.body, ctx);
                        ctx.emit(StatementKind::Assign(result, Rvalue::Use(v)));
                        if !ctx.term_is_set() {
                            ctx.set_term(TerminatorKind::Goto { target: merge });
                        }
                        ctx.switch(next);
                    },
                    parser::ast::Pattern::Constructor(_, _)
                    | parser::ast::Pattern::Variant(_, _) => {
                        // Constructor / variant patterns are handled by the tree-walker;
                        // in MIR they fall through to the next arm.
                    },
                }
            }
            if !matched_all && !ctx.term_is_set() {
                ctx.set_term(TerminatorKind::Goto { target: merge });
            }
            ctx.switch(merge);
            Operand::Copy(result)
        },

        parser::ast::Expr::Closure(params, body) => {
            let free_vars = collect_free_vars(body, params);
            let capture_count = free_vars.len();
            let arg_count = params.len();
            let closure_id = ctx.closures.len();
            let closure_name = format!("__closure_{}", closure_id);
            let total_args = arg_count + capture_count;
            let mut closure_func = MirFunction::new(&closure_name, total_args);
            closure_func.param_names = params.clone();
            let mut closure_ctx = LowerCtx::new(&mut closure_func, total_args, ctx.globals.clone());
            for (i, pname) in params.iter().enumerate() {
                let local = Local(i + 1);
                closure_ctx.declare_in_scope(pname, local);
            }
            for (ci, fv) in free_vars.iter().enumerate() {
                let param_local = Local(arg_count + 1 + ci);
                closure_ctx.declare_in_scope(fv, param_local);
            }
            let body_val = lower_expr(body, &mut closure_ctx);
            closure_ctx.emit(StatementKind::Assign(Local(0), Rvalue::Use(body_val)));
            closure_ctx.set_term(TerminatorKind::Return);
            ctx.closures.push(closure_func);
            Operand::Constant(Constant::Function(closure_name))
        },

        parser::ast::Expr::Await(_) => Operand::Constant(Constant::None),
    }
}

/// One-bit float constant used for loop counters (`0.0` / `1.0`).
fn f64_const(v: f64) -> Operand {
    Operand::Constant(Constant::F64(v.to_bits()))
}

/// Lower an `if`/`else if`/`else` chain. Each branch writes its trailing value to
/// `result` and jumps to `merge`. The condition tree is built recursively so any
/// number of `else if` arms compose without bespoke index arithmetic.
fn lower_if_chain(
    cond: &parser::ast::Expr,
    then: &[parser::ast::Stmt],
    elseifs: &[(parser::ast::Expr, Vec<parser::ast::Stmt>)],
    else_body: &Option<Vec<parser::ast::Stmt>>,
    result: Local,
    merge: BasicBlockId,
    ctx: &mut LowerCtx,
) {
    let cond_op = lower_expr(cond, ctx);
    let then_bb = ctx.new_block();
    let else_bb = ctx.new_block();
    ctx.set_term(TerminatorKind::SwitchInt {
        discr: cond_op,
        targets: vec![(1, then_bb)],
        otherwise: else_bb,
    });

    ctx.switch(then_bb);
    let then_val = lower_stmts(then, ctx).unwrap_or(Operand::Constant(Constant::None));
    if !ctx.term_is_set() {
        ctx.emit(StatementKind::Assign(result, Rvalue::Use(then_val)));
        ctx.set_term(TerminatorKind::Goto { target: merge });
    }

    ctx.switch(else_bb);
    if let Some(((next_cond, next_then), rest)) = elseifs.split_first() {
        lower_if_chain(next_cond, next_then, rest, else_body, result, merge, ctx);
    } else {
        let else_val = match else_body {
            Some(stmts) => lower_stmts(stmts, ctx).unwrap_or(Operand::Constant(Constant::None)),
            None => Operand::Constant(Constant::None),
        };
        if !ctx.term_is_set() {
            ctx.emit(StatementKind::Assign(result, Rvalue::Use(else_val)));
            ctx.set_term(TerminatorKind::Goto { target: merge });
        }
    }
}

/// Lower a `for var in iter { body }` loop. Integer ranges (`lo..hi`) iterate a
/// counter directly; any other iterable is treated as a list and indexed via the
/// list runtime so the same lowering serves both.
fn lower_for(var: &str, iter: &parser::ast::Expr, body: &[parser::ast::Stmt], ctx: &mut LowerCtx) {
    use ling_ast::ast::BinOp as B;
    if let parser::ast::Expr::Range(lo, hi) = iter {
        let lo_op = lower_expr(lo, ctx);
        let hi_op = lower_expr(hi, ctx);
        // The loop body runs in a fresh scope: the counter lives here, and
        // re-binding an outer name shadows it rather than mutating it.
        ctx.enter_scope();
        let counter = ctx.alloc_local(Some(var.to_string()), true);
        ctx.declare_in_scope(var, counter);
        ctx.emit(StatementKind::Assign(counter, Rvalue::Use(lo_op)));
        let limit = ctx.alloc_local(None, false);
        ctx.emit(StatementKind::Assign(limit, Rvalue::Use(hi_op)));

        let header = ctx.new_block();
        let body_bb = ctx.new_block();
        let exit = ctx.new_block();
        ctx.set_term(TerminatorKind::Goto { target: header });

        ctx.switch(header);
        let test = ctx.alloc_local(None, false);
        ctx.emit(StatementKind::Assign(
            test,
            Rvalue::BinaryOp(B::Lt, Operand::Copy(counter), Operand::Copy(limit)),
        ));
        ctx.set_term(TerminatorKind::SwitchInt {
            discr: Operand::Copy(test),
            targets: vec![(1, body_bb)],
            otherwise: exit,
        });

        ctx.switch(body_bb);
        lower_stmts(body, ctx);
        if !ctx.term_is_set() {
            ctx.emit(StatementKind::Assign(
                counter,
                Rvalue::BinaryOp(B::Add, Operand::Copy(counter), f64_const(1.0)),
            ));
            ctx.set_term(TerminatorKind::Goto { target: header });
        }
        ctx.switch(exit);
        ctx.exit_scope();
        return;
    }

    // General iterable: index into it as a list.
    let list_op = lower_expr(iter, ctx);
    let list = ctx.alloc_local(None, false);
    ctx.emit(StatementKind::Assign(list, Rvalue::Use(list_op)));
    let len = ctx.alloc_local(None, false);
    ctx.emit(StatementKind::Assign(
        len,
        Rvalue::Call {
            func: Operand::Constant(Constant::Function("len".to_string())),
            args: vec![Operand::Copy(list)],
        },
    ));
    ctx.enter_scope();
    let idx = ctx.alloc_local(None, true);
    ctx.emit(StatementKind::Assign(idx, Rvalue::Use(f64_const(0.0))));
    let item = ctx.alloc_local(Some(var.to_string()), true);
    ctx.declare_in_scope(var, item);

    let header = ctx.new_block();
    let body_bb = ctx.new_block();
    let exit = ctx.new_block();
    ctx.set_term(TerminatorKind::Goto { target: header });

    ctx.switch(header);
    let test = ctx.alloc_local(None, false);
    ctx.emit(StatementKind::Assign(
        test,
        Rvalue::BinaryOp(B::Lt, Operand::Copy(idx), Operand::Copy(len)),
    ));
    ctx.set_term(TerminatorKind::SwitchInt {
        discr: Operand::Copy(test),
        targets: vec![(1, body_bb)],
        otherwise: exit,
    });

    ctx.switch(body_bb);
    ctx.emit(StatementKind::Assign(
        item,
        Rvalue::GetIndex(Operand::Copy(list), Operand::Copy(idx)),
    ));
    lower_stmts(body, ctx);
    if !ctx.term_is_set() {
        ctx.emit(StatementKind::Assign(
            idx,
            Rvalue::BinaryOp(B::Add, Operand::Copy(idx), f64_const(1.0)),
        ));
        ctx.set_term(TerminatorKind::Goto { target: header });
    }
    ctx.switch(exit);
    ctx.exit_scope();
}

fn method_name_from_expr(expr: &parser::ast::Expr) -> String {
    match expr {
        parser::ast::Expr::Ident(name) => name.clone(),
        parser::ast::Expr::Path(parts) => parts.join("::"),
        _ => "value".to_string(),
    }
}

fn collect_free_vars(body: &parser::ast::Expr, params: &[String]) -> Vec<String> {
    let param_set: std::collections::HashSet<&str> = params.iter().map(|s| s.as_str()).collect();
    let mut free = std::collections::HashSet::new();
    free_vars_in_expr(body, &param_set, &mut free);
    let mut result: Vec<String> = free.into_iter().collect();
    result.sort();
    result
}

#[allow(unused)]
fn free_vars_in_expr(
    expr: &parser::ast::Expr,
    params: &std::collections::HashSet<&str>,
    free: &mut std::collections::HashSet<String>,
) {
    match expr {
        parser::ast::Expr::Ident(name) => {
            if !params.contains(name.as_str()) {
                free.insert(name.clone());
            }
        },
        parser::ast::Expr::BinOp(_, lhs, rhs) => {
            free_vars_in_expr(lhs, params, free);
            free_vars_in_expr(rhs, params, free);
        },
        parser::ast::Expr::Call(callee, args) => {
            free_vars_in_expr(callee, params, free);
            for a in args {
                free_vars_in_expr(a, params, free);
            }
        },
        parser::ast::Expr::MethodCall { receiver, args, .. } => {
            free_vars_in_expr(receiver, params, free);
            for a in args {
                free_vars_in_expr(a, params, free);
            }
        },
        parser::ast::Expr::If { cond, then, elseifs, else_body, .. } => {
            free_vars_in_expr(cond, params, free);
            for s in then {
                free_vars_in_stmt(s, params, free);
            }
            for (ec, eb) in elseifs {
                free_vars_in_expr(ec, params, free);
                for s in eb {
                    free_vars_in_stmt(s, params, free);
                }
            }
            if let Some(eb) = else_body {
                for s in eb {
                    free_vars_in_stmt(s, params, free);
                }
            }
        },
        parser::ast::Expr::While { cond, body } => {
            free_vars_in_expr(cond, params, free);
            for s in body {
                free_vars_in_stmt(s, params, free);
            }
        },
        parser::ast::Expr::For { var: _, iter, body } => {
            free_vars_in_expr(iter, params, free);
            for s in body {
                free_vars_in_stmt(s, params, free);
            }
        },
        parser::ast::Expr::Match(scrutinee, arms) => {
            free_vars_in_expr(scrutinee, params, free);
            for arm in arms {
                // Pattern bindings introduce new variables
                let mut arm_params = params.clone();
                if let parser::ast::Pattern::Ident(name) = &arm.pattern {
                    arm_params.insert(name);
                }
                free_vars_in_expr(&arm.body, &arm_params, free);
            }
        },
        parser::ast::Expr::Do(stmts) => {
            for s in stmts {
                free_vars_in_stmt(s, params, free);
            }
        },
        parser::ast::Expr::Array(elems) => {
            for e in elems {
                free_vars_in_expr(e, params, free);
            }
        },
        parser::ast::Expr::Range(lo, hi) => {
            free_vars_in_expr(lo, params, free);
            free_vars_in_expr(hi, params, free);
        },
        parser::ast::Expr::Index(base, idx) => {
            free_vars_in_expr(base, params, free);
            free_vars_in_expr(idx, params, free);
        },
        parser::ast::Expr::Ref(inner) => free_vars_in_expr(inner, params, free),
        parser::ast::Expr::Closure(inner_params, body) => {
            let mut closure_params = params.clone();
            for p in inner_params {
                closure_params.insert(p);
            }
            free_vars_in_expr(body, &closure_params, free);
        },
        parser::ast::Expr::Await(inner) => free_vars_in_expr(inner, params, free),
        parser::ast::Expr::Path(parts) => {
            for p in parts {
                if !params.contains(p.as_str()) {
                    free.insert(p.clone());
                }
            }
        },
        _ => {},
    }
}

#[allow(unused)]
fn free_vars_in_stmt(
    stmt: &parser::ast::Stmt,
    params: &std::collections::HashSet<&str>,
    free: &mut std::collections::HashSet<String>,
) {
    match stmt {
        parser::ast::Stmt::Bind(name, expr) => {
            free_vars_in_expr(expr, params, free);
            // name is defined here, so it's not free
        },
        parser::ast::Stmt::Expr(expr) => free_vars_in_expr(expr, params, free),
        parser::ast::Stmt::Return(expr) => free_vars_in_expr(expr, params, free),
    }
}

fn pattern_to_operand(p: &parser::ast::Pattern) -> Operand {
    match p {
        parser::ast::Pattern::Str(s) => Operand::Constant(Constant::Str(s.clone())),
        parser::ast::Pattern::Number(n) => Operand::Constant(Constant::F64(n.to_bits())),
        parser::ast::Pattern::Bool(b) => Operand::Constant(Constant::Bool(*b)),
        _ => Operand::Constant(Constant::None),
    }
}

fn lower_binop(op: parser::ast::BinOp) -> ling_ast::ast::BinOp {
    use parser::ast::BinOp as A;
    match op {
        A::Add => ling_ast::ast::BinOp::Add,
        A::Sub => ling_ast::ast::BinOp::Sub,
        A::Mul => ling_ast::ast::BinOp::Mul,
        A::Div => ling_ast::ast::BinOp::Div,
        A::Rem => ling_ast::ast::BinOp::Rem,
        A::Eq => ling_ast::ast::BinOp::Eq,
        A::Ne => ling_ast::ast::BinOp::Ne,
        A::Lt => ling_ast::ast::BinOp::Lt,
        A::Gt => ling_ast::ast::BinOp::Gt,
        A::Le => ling_ast::ast::BinOp::Le,
        A::Ge => ling_ast::ast::BinOp::Ge,
        A::And => ling_ast::ast::BinOp::And,
        A::Or => ling_ast::ast::BinOp::Or,
    }
}
