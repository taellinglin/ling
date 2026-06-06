use crate::core::{LingError, LingResult, OptimizationLevel};
use crate::parser;
use ling_ast::Span;
use ling_mir::ir::*;
use ling_mir::optimizer::{OptLevel, Optimizer};
use std::collections::HashMap;

/// Parse source, lower to MIR, run optimizer, return optimized MIR.
pub fn compile_and_optimize(source: &str, opt_level: OptimizationLevel) -> LingResult<MirProgram> {
    let ast = parser::parse(source).map_err(|e| LingError::Parse(e))?;

    // Run semantic analysis (type checking)
    let mut semantic = crate::semantic::SemanticAnalyzer::new();
    semantic.analyze(&ast).map_err(|e| {
        eprintln!("Type error: {}", e);
        e
    })?;

    let mut mir = lower_program(&ast);
    if !matches!(opt_level, OptimizationLevel::None) {
        let mir_opt = match opt_level {
            OptimizationLevel::None => OptLevel::None,
            OptimizationLevel::O1 => OptLevel::O1,
            OptimizationLevel::O2 => OptLevel::O2,
            OptimizationLevel::O3 => OptLevel::O3,
        };
        Optimizer::new(mir_opt).run(&mut mir.functions);
    }
    Ok(mir)
}

// ─── AST → MIR lowering ──────────────────────────────────────────────────────

fn lower_program(prog: &parser::ast::Program) -> MirProgram {
    let mut functions = Vec::new();
    for item in &prog.items {
        if let parser::ast::Item::Fn(fndef) = item {
            functions.push(lower_function(fndef));
        }
    }
    // closures are collected through LowerCtx during main body lowering

    // Lower top-level binds as the main function body
    let mut main_stmts: Vec<parser::ast::Stmt> = Vec::new();
    let has_main_bind = prog.items.iter().any(|item| {
        if let parser::ast::Item::Bind(name, _) = item {
            name == "start" || name == "เริ่ม" || name == "__main__"
        } else {
            false
        }
    });

    for item in &prog.items {
        if let parser::ast::Item::Bind(name, body) = item {
            if !has_main_bind || name == "start" || name == "เริ่ม" || name == "__main__"
            {
                main_stmts.push(parser::ast::Stmt::Bind(name.clone(), body.clone()));
            }
        }
    }

    if !main_stmts.is_empty() {
        let mut main = MirFunction::new("__main__", 0);
        let mut lctx = LowerCtx::new(&mut main, 0);
        for stmt in &main_stmts {
            lower_stmt(stmt, &mut lctx);
        }
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

fn lower_function(fndef: &parser::ast::FnDef) -> MirFunction {
    let arg_count = fndef.params.len();
    let param_names = fndef.params.clone();
    let mut func = MirFunction::new(&fndef.name, arg_count);
    func.param_names = param_names;

    let mut lctx = LowerCtx::new(&mut func, arg_count);

    for (i, pname) in fndef.params.iter().enumerate() {
        let local = Local(i + 1);
        lctx.locals.insert(pname.clone(), local);
    }

    lower_stmts(&fndef.body, &mut lctx);
    func
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
    closures: Vec<MirFunction>,
    closure_vars: HashMap<String, ClosureInfo>,
}

impl<'a> LowerCtx<'a> {
    fn new(func: &'a mut MirFunction, arg_count: usize) -> Self {
        let next = (arg_count + 1) as usize;
        Self {
            func,
            locals: HashMap::new(),
            next_local: next,
            closures: Vec::new(),
            closure_vars: HashMap::new(),
        }
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

    fn emit(&mut self, kind: StatementKind) {
        let bb = &mut self.func.basic_blocks[0];
        bb.statements.push(Statement { kind, span: Span::DUMMY });
    }

    fn set_term(&mut self, kind: TerminatorKind) {
        self.func.basic_blocks[0].terminator = Some(Terminator { kind, span: Span::DUMMY });
    }
}

fn lower_stmts(stmts: &[parser::ast::Stmt], ctx: &mut LowerCtx) {
    for stmt in stmts {
        lower_stmt(stmt, ctx);
    }
}

fn lower_stmt(stmt: &parser::ast::Stmt, ctx: &mut LowerCtx) {
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
                    let mut closure_ctx = LowerCtx::new(&mut closure_func, total_args);
                    for (i, pname) in params.iter().enumerate() {
                        closure_ctx.locals.insert(pname.clone(), Local(i + 1));
                    }
                    for (ci, fv) in free_vars.iter().enumerate() {
                        closure_ctx
                            .locals
                            .insert(fv.clone(), Local(arg_count + 1 + ci));
                    }
                    let body_val = lower_expr(body.as_ref(), &mut closure_ctx);
                    closure_ctx.emit(StatementKind::Assign(Local(0), Rvalue::Use(body_val)));
                    ctx.closures.push(closure_func);

                    ctx.closure_vars.insert(
                        name.clone(),
                        ClosureInfo {
                            func_name: closure_name.clone(),
                            captures: free_vars.clone(),
                        },
                    );

                    let local = ctx.alloc_local(Some(name.clone()), true);
                    ctx.locals.insert(name.clone(), local);
                    ctx.emit(StatementKind::Assign(
                        local,
                        Rvalue::Use(Operand::Constant(Constant::Function(closure_name))),
                    ));
                    return;
                }
            }
            let val = lower_expr(expr, ctx);
            let local = ctx.alloc_local(Some(name.clone()), true);
            ctx.locals.insert(name.clone(), local);
            ctx.emit(StatementKind::Assign(local, Rvalue::Use(val)));
        },
        parser::ast::Stmt::Expr(expr) => {
            let val = lower_expr(expr, ctx);
            let tmp = ctx.alloc_local(None, false);
            ctx.emit(StatementKind::Assign(tmp, Rvalue::Use(val)));
        },
        parser::ast::Stmt::Return(expr) => {
            let val = lower_expr(expr, ctx);
            ctx.emit(StatementKind::Assign(Local(0), Rvalue::Use(val)));
            ctx.set_term(TerminatorKind::Return);
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
                let mut closure_ctx = LowerCtx::new(&mut closure_func, arg_count + capture_count);
                for (i, pname) in params.iter().enumerate() {
                    let local = Local(i + 1);
                    closure_ctx.locals.insert(pname.clone(), local);
                }
                // Map captures to extra params
                for (ci, fv) in free_vars.iter().enumerate() {
                    let param_local = Local(arg_count + 1 + ci);
                    closure_ctx.locals.insert(fv.clone(), param_local);
                }
                let body_val = lower_expr(body, &mut closure_ctx);
                closure_ctx.emit(StatementKind::Assign(Local(0), Rvalue::Use(body_val)));
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
            let result = ctx.alloc_local(None, false);
            let cond_op = lower_expr(cond, ctx);

            let then_block = BasicBlockId(ctx.func.basic_blocks.len());
            ctx.func.basic_blocks.push(BasicBlock {
                statements: Vec::new(),
                terminator: Some(Terminator {
                    kind: TerminatorKind::Goto {
                        target: BasicBlockId(
                            ctx.func.basic_blocks.len()
                                + (if elseifs.is_empty() && else_body.is_none() {
                                    2
                                } else {
                                    4
                                }),
                        ),
                    },
                    span: Span::DUMMY,
                }),
            });

            if elseifs.is_empty() {
                let else_block = BasicBlockId(ctx.func.basic_blocks.len());
                ctx.func.basic_blocks.push(BasicBlock {
                    statements: Vec::new(),
                    terminator: Some(Terminator {
                        kind: TerminatorKind::Goto {
                            target: BasicBlockId(ctx.func.basic_blocks.len() + 1),
                        },
                        span: Span::DUMMY,
                    }),
                });

                let merge_block = BasicBlockId(ctx.func.basic_blocks.len());
                ctx.func.basic_blocks.push(BasicBlock {
                    statements: Vec::new(),
                    terminator: Some(Terminator {
                        kind: TerminatorKind::Return,
                        span: Span::DUMMY,
                    }),
                });

                // Save the current bb0 statements
                let current_stmts = std::mem::take(&mut ctx.func.basic_blocks[0].statements);

                // Set the SwitchInt terminator on bb0
                ctx.func.basic_blocks[0].terminator = Some(Terminator {
                    kind: TerminatorKind::SwitchInt {
                        discr: cond_op,
                        targets: vec![(1, then_block)],
                        otherwise: else_block,
                    },
                    span: Span::DUMMY,
                });

                // Lower then-body into a temporary ctx then move to then_block
                ctx.func.basic_blocks[0].statements = Vec::new();
                lower_stmts(then, ctx);
                ctx.func.basic_blocks[then_block.0].statements =
                    std::mem::take(&mut ctx.func.basic_blocks[0].statements);
                ctx.func.basic_blocks[then_block.0].terminator = Some(Terminator {
                    kind: TerminatorKind::Goto { target: merge_block },
                    span: Span::DUMMY,
                });

                if let Some(else_stmts) = else_body {
                    ctx.func.basic_blocks[0].statements = Vec::new();
                    lower_stmts(else_stmts, ctx);
                    ctx.func.basic_blocks[else_block.0].statements =
                        std::mem::take(&mut ctx.func.basic_blocks[0].statements);
                    ctx.func.basic_blocks[else_block.0].terminator = Some(Terminator {
                        kind: TerminatorKind::Goto { target: merge_block },
                        span: Span::DUMMY,
                    });
                }

                // Restore bb0: statements from before the if, terminator is the SwitchInt already
                ctx.func.basic_blocks[0].statements = current_stmts;
            } else {
                // Chain else-if: build nested if-else for each elif
                let mut all_elif_blocks = Vec::new();
                let mut all_elif_targets = Vec::new();

                for (elif_cond, elif_body) in elseifs {
                    let elif_block = BasicBlockId(ctx.func.basic_blocks.len());
                    let elif_merge = BasicBlockId(ctx.func.basic_blocks.len() + 1);
                    ctx.func.basic_blocks.push(BasicBlock {
                        statements: Vec::new(),
                        terminator: Some(Terminator {
                            kind: TerminatorKind::Goto {
                                target: BasicBlockId(ctx.func.basic_blocks.len() + 1),
                            },
                            span: Span::DUMMY,
                        }),
                    });
                    ctx.func.basic_blocks.push(BasicBlock {
                        statements: Vec::new(),
                        terminator: Some(Terminator {
                            kind: TerminatorKind::Return,
                            span: Span::DUMMY,
                        }),
                    });
                    all_elif_blocks.push((
                        elif_block,
                        elif_merge,
                        elif_cond.clone(),
                        elif_body.clone(),
                    ));
                    all_elif_targets.push(elif_block);
                }

                let final_else_block = BasicBlockId(ctx.func.basic_blocks.len());
                ctx.func.basic_blocks.push(BasicBlock {
                    statements: Vec::new(),
                    terminator: Some(Terminator {
                        kind: TerminatorKind::Goto {
                            target: BasicBlockId(ctx.func.basic_blocks.len() + 1),
                        },
                        span: Span::DUMMY,
                    }),
                });

                let merge_block = BasicBlockId(ctx.func.basic_blocks.len());
                ctx.func.basic_blocks.push(BasicBlock {
                    statements: Vec::new(),
                    terminator: Some(Terminator {
                        kind: TerminatorKind::Return,
                        span: Span::DUMMY,
                    }),
                });

                let current_stmts = std::mem::take(&mut ctx.func.basic_blocks[0].statements);

                // bb0: switch on first condition
                ctx.func.basic_blocks[0].terminator = Some(Terminator {
                    kind: TerminatorKind::SwitchInt {
                        discr: cond_op,
                        targets: vec![(1, then_block)],
                        otherwise: all_elif_targets[0],
                    },
                    span: Span::DUMMY,
                });

                // Lower then body
                ctx.func.basic_blocks[0].statements = Vec::new();
                lower_stmts(then, ctx);
                ctx.func.basic_blocks[then_block.0].statements =
                    std::mem::take(&mut ctx.func.basic_blocks[0].statements);
                ctx.func.basic_blocks[then_block.0].terminator = Some(Terminator {
                    kind: TerminatorKind::Goto { target: merge_block },
                    span: Span::DUMMY,
                });

                // Lower each elif
                for (idx, (elif_block, elif_merge, elif_cond, elif_body)) in
                    all_elif_blocks.iter().enumerate()
                {
                    let elif_cond_op = lower_expr(elif_cond, ctx);
                    let cond_local = ctx.alloc_local(None, false);
                    ctx.emit(StatementKind::Assign(cond_local, Rvalue::Use(elif_cond_op)));

                    let next_target = if idx + 1 < all_elif_blocks.len() {
                        all_elif_targets[idx + 1]
                    } else {
                        final_else_block
                    };

                    ctx.func.basic_blocks[elif_block.0].terminator = Some(Terminator {
                        kind: TerminatorKind::SwitchInt {
                            discr: Operand::Copy(cond_local),
                            targets: vec![(1, *elif_merge)],
                            otherwise: next_target,
                        },
                        span: Span::DUMMY,
                    });

                    ctx.func.basic_blocks[0].statements = Vec::new();
                    lower_stmts(elif_body, ctx);
                    ctx.func.basic_blocks[elif_merge.0].statements =
                        std::mem::take(&mut ctx.func.basic_blocks[0].statements);
                    ctx.func.basic_blocks[elif_merge.0].terminator = Some(Terminator {
                        kind: TerminatorKind::Goto { target: merge_block },
                        span: Span::DUMMY,
                    });
                }

                // Lower final else body
                if let Some(else_stmts) = else_body {
                    ctx.func.basic_blocks[0].statements = Vec::new();
                    lower_stmts(else_stmts, ctx);
                    ctx.func.basic_blocks[final_else_block.0].statements =
                        std::mem::take(&mut ctx.func.basic_blocks[0].statements);
                    ctx.func.basic_blocks[final_else_block.0].terminator = Some(Terminator {
                        kind: TerminatorKind::Goto { target: merge_block },
                        span: Span::DUMMY,
                    });
                } else {
                    ctx.func.basic_blocks[final_else_block.0].statements = Vec::new();
                    ctx.func.basic_blocks[final_else_block.0].terminator = Some(Terminator {
                        kind: TerminatorKind::Goto { target: merge_block },
                        span: Span::DUMMY,
                    });
                }

                ctx.func.basic_blocks[0].statements = current_stmts;
            }

            Operand::Copy(result)
        },
        parser::ast::Expr::While { cond, body } => {
            let header_block = BasicBlockId(ctx.func.basic_blocks.len());
            ctx.func.basic_blocks.push(BasicBlock {
                statements: Vec::new(),
                terminator: Some(Terminator { kind: TerminatorKind::Return, span: Span::DUMMY }),
            });

            let body_block = BasicBlockId(ctx.func.basic_blocks.len());
            ctx.func.basic_blocks.push(BasicBlock {
                statements: Vec::new(),
                terminator: Some(Terminator { kind: TerminatorKind::Return, span: Span::DUMMY }),
            });

            let exit_block = BasicBlockId(ctx.func.basic_blocks.len());
            ctx.func.basic_blocks.push(BasicBlock {
                statements: Vec::new(),
                terminator: Some(Terminator { kind: TerminatorKind::Return, span: Span::DUMMY }),
            });

            let prev_stmts = std::mem::take(&mut ctx.func.basic_blocks[0].statements);
            ctx.func.basic_blocks[0].terminator = Some(Terminator {
                kind: TerminatorKind::Goto { target: header_block },
                span: Span::DUMMY,
            });

            let cond_op = lower_expr(cond, ctx);
            ctx.func.basic_blocks[header_block.0].statements =
                std::mem::take(&mut ctx.func.basic_blocks[0].statements);
            let header_test = ctx.alloc_local(None, false);
            ctx.func.basic_blocks[header_block.0]
                .statements
                .push(Statement {
                    kind: StatementKind::Assign(header_test, Rvalue::Use(cond_op)),
                    span: Span::DUMMY,
                });
            ctx.func.basic_blocks[header_block.0].terminator = Some(Terminator {
                kind: TerminatorKind::SwitchInt {
                    discr: Operand::Copy(header_test),
                    targets: vec![(1, body_block)],
                    otherwise: exit_block,
                },
                span: Span::DUMMY,
            });

            ctx.func.basic_blocks[0].statements = Vec::new();
            lower_stmts(body, ctx);
            ctx.func.basic_blocks[body_block.0].statements =
                std::mem::take(&mut ctx.func.basic_blocks[0].statements);
            ctx.func.basic_blocks[body_block.0].terminator = Some(Terminator {
                kind: TerminatorKind::Goto { target: header_block },
                span: Span::DUMMY,
            });

            ctx.func.basic_blocks[0].statements = prev_stmts;
            ctx.func.basic_blocks[0].terminator = Some(Terminator {
                kind: TerminatorKind::Goto { target: exit_block },
                span: Span::DUMMY,
            });

            let result = ctx.alloc_local(None, false);
            ctx.emit(StatementKind::Assign(
                result,
                Rvalue::Use(Operand::Constant(Constant::None)),
            ));
            Operand::Copy(result)
        },
        parser::ast::Expr::For { var: _, iter, body } => {
            lower_expr(iter, ctx);
            lower_stmts(body, ctx);
            let result = ctx.alloc_local(None, false);
            ctx.emit(StatementKind::Assign(
                result,
                Rvalue::Use(Operand::Constant(Constant::None)),
            ));
            Operand::Copy(result)
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
            lower_stmts(stmts, ctx);
            Operand::Copy(Local(0))
        },
        parser::ast::Expr::Ref(inner) => {
            let op = lower_expr(inner, ctx);
            let l = match &op {
                Operand::Copy(l) | Operand::Move(l) => *l,
                _ => {
                    let t = ctx.alloc_local(None, false);
                    ctx.emit(StatementKind::Assign(t, Rvalue::Use(op)));
                    t
                },
            };
            let ref_local = ctx.alloc_local(None, false);
            ctx.emit(StatementKind::Assign(ref_local, Rvalue::Ref(l)));
            Operand::Copy(ref_local)
        },
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

            let merge_block = BasicBlockId(ctx.func.basic_blocks.len());
            ctx.func.basic_blocks.push(BasicBlock {
                statements: Vec::new(),
                terminator: Some(Terminator { kind: TerminatorKind::Return, span: Span::DUMMY }),
            });
            let result = ctx.alloc_local(None, false);

            let mut arm_blocks = Vec::new();
            for arm in arms {
                let body_block = BasicBlockId(ctx.func.basic_blocks.len());
                arm_blocks.push((arm, body_block, false));
            }

            // Emit each arm: check pattern, branch to body, fallthrough
            let current_stmts = std::mem::take(&mut ctx.func.basic_blocks[0].statements);
            let entry_term = ctx.func.basic_blocks[0].terminator.take();

            for (idx, (arm, body_block, _)) in arm_blocks.iter().enumerate() {
                match &arm.pattern {
                    parser::ast::Pattern::Wildcard => {
                        // Wildcard arm: always matches, store scrutinee if ident binding
                        ctx.func.basic_blocks[0].statements = Vec::new();
                        let arm_result = lower_expr(&arm.body, ctx);
                        ctx.emit(StatementKind::Assign(result, Rvalue::Use(arm_result)));
                        ctx.set_term(TerminatorKind::Goto { target: *body_block });
                        // body_block serves as goto-to-merge
                        let saved = std::mem::take(&mut ctx.func.basic_blocks[0].statements);
                        let saved_term = ctx.func.basic_blocks[0].terminator.take();
                        ctx.func.basic_blocks[body_block.0].statements = saved;
                        ctx.func.basic_blocks[body_block.0].terminator = saved_term;
                        ctx.func.basic_blocks[body_block.0].terminator = Some(Terminator {
                            kind: TerminatorKind::Goto { target: merge_block },
                            span: Span::DUMMY,
                        });
                    },
                    parser::ast::Pattern::Ident(name) => {
                        // Bind scrutinee to variable name, execute body
                        let bound_local = ctx.alloc_local(Some(name.clone()), true);
                        ctx.locals.insert(name.clone(), bound_local);
                        let prev_stmts = std::mem::take(&mut ctx.func.basic_blocks[0].statements);
                        ctx.emit(StatementKind::Assign(
                            bound_local,
                            Rvalue::Use(Operand::Copy(scrut_local)),
                        ));
                        let arm_result = lower_expr(&arm.body, ctx);
                        ctx.emit(StatementKind::Assign(result, Rvalue::Use(arm_result)));
                        ctx.set_term(TerminatorKind::Goto { target: *body_block });
                        let saved = std::mem::take(&mut ctx.func.basic_blocks[0].statements);
                        let saved_term = ctx.func.basic_blocks[0].terminator.take();
                        ctx.func.basic_blocks[body_block.0].statements = saved;
                        ctx.func.basic_blocks[body_block.0].terminator = saved_term;
                        ctx.func.basic_blocks[body_block.0].terminator = Some(Terminator {
                            kind: TerminatorKind::Goto { target: merge_block },
                            span: Span::DUMMY,
                        });
                        ctx.func.basic_blocks[0].statements = prev_stmts;
                    },
                    parser::ast::Pattern::Str(_)
                    | parser::ast::Pattern::Number(_)
                    | parser::ast::Pattern::Bool(_) => {
                        // Literal pattern: compare scrutinee to literal, branch on match
                        let lit_op = pattern_to_operand(&arm.pattern);
                        let cmp_local = ctx.alloc_local(None, false);
                        ctx.emit(StatementKind::Assign(
                            cmp_local,
                            Rvalue::BinaryOp(
                                ling_ast::ast::BinOp::Eq,
                                Operand::Copy(scrut_local),
                                lit_op,
                            ),
                        ));

                        let fallthrough = if idx + 1 < arm_blocks.len() {
                            arm_blocks[idx + 1].1
                        } else {
                            merge_block
                        };

                        let prev_stmts = std::mem::take(&mut ctx.func.basic_blocks[0].statements);
                        ctx.func.basic_blocks[0].terminator = Some(Terminator {
                            kind: TerminatorKind::SwitchInt {
                                discr: Operand::Copy(cmp_local),
                                targets: vec![(1, *body_block)],
                                otherwise: fallthrough,
                            },
                            span: Span::DUMMY,
                        });
                        ctx.func.basic_blocks[0].statements = prev_stmts;

                        // Body block
                        let arm_result = lower_expr(&arm.body, ctx);
                        ctx.emit(StatementKind::Assign(result, Rvalue::Use(arm_result)));
                        ctx.set_term(TerminatorKind::Goto { target: merge_block });
                        let saved = std::mem::take(&mut ctx.func.basic_blocks[0].statements);
                        let saved_term = ctx.func.basic_blocks[0].terminator.take();
                        ctx.func.basic_blocks[body_block.0].statements = saved;
                        ctx.func.basic_blocks[body_block.0].terminator = saved_term;
                    },
                    parser::ast::Pattern::Constructor(_, _) => {
                        // Constructor patterns: skip for now, fall through
                        let fallthrough = if idx + 1 < arm_blocks.len() {
                            arm_blocks[idx + 1].1
                        } else {
                            merge_block
                        };
                        ctx.set_term(TerminatorKind::Goto { target: fallthrough });
                    },
                }
            }

            ctx.func.basic_blocks[0].statements = current_stmts;
            ctx.func.basic_blocks[0].terminator = entry_term;

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
            let mut closure_ctx = LowerCtx::new(&mut closure_func, total_args);
            for (i, pname) in params.iter().enumerate() {
                let local = Local(i + 1);
                closure_ctx.locals.insert(pname.clone(), local);
            }
            for (ci, fv) in free_vars.iter().enumerate() {
                let param_local = Local(arg_count + 1 + ci);
                closure_ctx.locals.insert(fv.clone(), param_local);
            }
            let body_val = lower_expr(body, &mut closure_ctx);
            closure_ctx.emit(StatementKind::Assign(Local(0), Rvalue::Use(body_val)));
            ctx.closures.push(closure_func);
            Operand::Constant(Constant::Function(closure_name))
        },

        parser::ast::Expr::Await(_) => Operand::Constant(Constant::None),
    }
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
