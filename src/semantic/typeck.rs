use crate::core::LingResult;
use crate::parser::ast;
use ling_core::types::Type;
use std::collections::HashMap;
use super::builtins::is_builtin;

type TypeEnv = HashMap<String, (Type, Vec<usize>)>;

#[derive(Clone, Debug)]
pub struct TypeChecker {
    var_counter: usize,
    substitutions: HashMap<usize, Type>,
    errors: Vec<String>,
}

impl TypeChecker {
    pub fn new() -> Self {
        Self {
            var_counter: 0,
            substitutions: HashMap::new(),
            errors: Vec::new(),
        }
    }

    pub fn check(&mut self, program: &ast::Program) -> LingResult<()> {
        let mut env = TypeEnv::new();
        // Built-in functions
        env.insert(
            "print".into(),
            (Type::Fn(vec![Type::Str], Box::new(Type::Unit)), vec![]),
        );
        env.insert(
            "now".into(),
            (Type::Fn(vec![], Box::new(Type::Float)), vec![]),
        );
        env.insert(
            "sleep".into(),
            (Type::Fn(vec![Type::Float], Box::new(Type::Unit)), vec![]),
        );
        env.insert(
            "len".into(),
            (
                Type::Fn(
                    vec![Type::List(Box::new(Type::Var(0)))],
                    Box::new(Type::Float),
                ),
                vec![0],
            ),
        );
        env.insert(
            "push".into(),
            (
                Type::Fn(
                    vec![Type::List(Box::new(Type::Var(0))), Type::Var(0)],
                    Box::new(Type::Unit),
                ),
                vec![0],
            ),
        );
        env.insert(
            "pop".into(),
            (
                Type::Fn(
                    vec![Type::List(Box::new(Type::Var(0)))],
                    Box::new(Type::Var(0)),
                ),
                vec![0],
            ),
        );
        env.insert(
            "type".into(),
            (Type::Fn(vec![Type::Var(0)], Box::new(Type::Str)), vec![0]),
        );
        env.insert(
            "assert".into(),
            (Type::Fn(vec![Type::Bool], Box::new(Type::Unit)), vec![]),
        );
        env.insert(
            "int".into(),
            (Type::Fn(vec![Type::Any], Box::new(Type::Float)), vec![]),
        );
        env.insert(
            "str".into(),
            (Type::Fn(vec![Type::Var(0)], Box::new(Type::Str)), vec![0]),
        );
        env.insert(
            "read_file".into(),
            (Type::Fn(vec![Type::Str], Box::new(Type::Str)), vec![]),
        );
        env.insert(
            "write_file".into(),
            (
                Type::Fn(vec![Type::Str, Type::Str], Box::new(Type::Unit)),
                vec![],
            ),
        );
        for item in &program.items {
            self.check_item(item, &mut env);
        }
        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(crate::core::LingError::Type(self.errors.join("\n")))
        }
    }

    fn new_var(&mut self) -> Type {
        let id = self.var_counter;
        self.var_counter += 1;
        Type::Var(id)
    }

    fn subst(&self, t: &Type) -> Type {
        match t {
            Type::Var(id) => {
                if let Some(resolved) = self.substitutions.get(id) {
                    self.subst(resolved)
                } else {
                    t.clone()
                }
            },
            Type::List(inner) => Type::List(Box::new(self.subst(inner))),
            Type::Tuple(elems) => Type::Tuple(elems.iter().map(|e| self.subst(e)).collect()),
            Type::Fn(params, ret) => Type::Fn(
                params.iter().map(|p| self.subst(p)).collect(),
                Box::new(self.subst(ret)),
            ),
            _ => t.clone(),
        }
    }

    fn free_vars(&self, t: &Type) -> Vec<usize> {
        match t {
            Type::Var(id) => {
                if self.substitutions.contains_key(id) {
                    self.free_vars(&self.substitutions[id])
                } else {
                    vec![*id]
                }
            },
            Type::List(inner) => self.free_vars(inner),
            Type::Tuple(elems) => {
                let mut v = Vec::new();
                for e in elems {
                    v.extend(self.free_vars(e));
                }
                v.sort();
                v.dedup();
                v
            },
            Type::Fn(params, ret) => {
                let mut v = Vec::new();
                for p in params {
                    v.extend(self.free_vars(p));
                }
                v.extend(self.free_vars(ret));
                v.sort();
                v.dedup();
                v
            },
            _ => vec![],
        }
    }

    fn env_free_vars(&self, env: &TypeEnv) -> Vec<usize> {
        let mut v = Vec::new();
        for (_, (t, _)) in env {
            v.extend(self.free_vars(t));
        }
        v.sort();
        v.dedup();
        v
    }

    fn generalize(&mut self, t: Type, env: &TypeEnv) -> (Type, Vec<usize>) {
        let t = self.subst(&t);
        let env_fv = self.env_free_vars(env);
        let t_fv = self.free_vars(&t);
        let generic: Vec<usize> = t_fv.into_iter().filter(|v| !env_fv.contains(v)).collect();
        (t, generic)
    }

    fn instantiate(&mut self, t: &Type, generic: &[usize]) -> Type {
        if generic.is_empty() {
            return t.clone();
        }
        let fresh_map: HashMap<usize, usize> = generic
            .iter()
            .map(|&v| (v, self.var_counter_usize()))
            .collect();
        self.replace_vars(t, &fresh_map)
    }

    fn var_counter_usize(&mut self) -> usize {
        let id = self.var_counter;
        self.var_counter += 1;
        id
    }

    fn replace_vars(&self, t: &Type, mapping: &HashMap<usize, usize>) -> Type {
        match t {
            Type::Var(id) => {
                if let Some(&new_id) = mapping.get(id) {
                    Type::Var(new_id)
                } else {
                    Type::Var(*id)
                }
            },
            Type::List(inner) => Type::List(Box::new(self.replace_vars(inner, mapping))),
            Type::Tuple(elems) => Type::Tuple(
                elems
                    .iter()
                    .map(|e| self.replace_vars(e, mapping))
                    .collect(),
            ),
            Type::Fn(params, ret) => Type::Fn(
                params
                    .iter()
                    .map(|p| self.replace_vars(p, mapping))
                    .collect(),
                Box::new(self.replace_vars(ret, mapping)),
            ),
            _ => t.clone(),
        }
    }

    fn lookup_env(&mut self, name: &str, env: &TypeEnv) -> Option<Type> {
        if let Some((t, generic)) = env.get(name) {
            if generic.is_empty() {
                Some(self.subst(t))
            } else {
                let substituted = self.subst(t);
                Some(self.instantiate(&substituted, generic))
            }
        } else if is_builtin(name) {
            Some(Type::Any)
        } else {
            None
        }
    }

    fn unify(&mut self, a: Type, b: Type) {
        let a = self.subst(&a);
        let b = self.subst(&b);
        if a == b || a == Type::Any || b == Type::Any {
            return;
        }
        match (&a, &b) {
            (Type::Var(id), _) => {
                if self.occurs_check(*id, &b) {
                    self.substitutions.insert(*id, b);
                } else {
                    self.errors.push("recursive type".into());
                }
            },
            (_, Type::Var(id)) => {
                if self.occurs_check(*id, &a) {
                    self.substitutions.insert(*id, a);
                } else {
                    self.errors.push("recursive type".into());
                }
            },
            (Type::List(ia), Type::List(ib)) => self.unify(*ia.clone(), *ib.clone()),
            (Type::Tuple(ea), Type::Tuple(eb)) if ea.len() == eb.len() => {
                for (ia, ib) in ea.iter().zip(eb.iter()) {
                    self.unify(ia.clone(), ib.clone());
                }
            },
            (Type::Tuple(ea), Type::Tuple(eb)) => {
                self.errors.push(format!(
                    "tuple length mismatch: {} vs {}",
                    ea.len(),
                    eb.len()
                ));
            },
            (Type::Fn(pa, ra), Type::Fn(pb, rb)) => {
                if pa.len() == pb.len() {
                    for (ia, ib) in pa.iter().zip(pb.iter()) {
                        self.unify(ia.clone(), ib.clone());
                    }
                    self.unify(*ra.clone(), *rb.clone());
                } else {
                    self.errors.push(format!(
                        "function parameter count mismatch: {} vs {}",
                        pa.len(),
                        pb.len()
                    ));
                }
            },
            _ => {
                self.errors
                    .push(format!("type mismatch: {:?} vs {:?}", a, b));
            },
        }
    }

    fn occurs_check(&self, id: usize, t: &Type) -> bool {
        match t {
            Type::Var(vid) => id != *vid,
            Type::List(inner) => self.occurs_check(id, inner),
            Type::Tuple(elems) => elems.iter().all(|e| self.occurs_check(id, e)),
            Type::Fn(params, ret) => {
                params.iter().all(|p| self.occurs_check(id, p)) && self.occurs_check(id, ret)
            },
            _ => true,
        }
    }

    fn check_item(&mut self, item: &ast::Item, env: &mut TypeEnv) {
        match item {
            ast::Item::Bind(name, expr) => {
                let t = self.infer_expr(expr, env);
                let (gt, generic) = self.generalize(t, env);
                env.insert(name.clone(), (gt, generic));
            },
            ast::Item::Fn(fndef) => {
                let mut fn_env = env.clone();
                let mut param_types = Vec::new();
                for pname in &fndef.params {
                    let tv = self.new_var();
                    fn_env.insert(pname.clone(), (tv.clone(), vec![]));
                    param_types.push(tv);
                }
                for stmt in &fndef.body {
                    self.check_stmt(stmt, &mut fn_env);
                }
                let ret_type = if fndef.body.is_empty() {
                    Type::Unit
                } else {
                    let last = fndef.body.last().unwrap();
                    match last {
                        ast::Stmt::Expr(e) => self.infer_expr(e, &fn_env),
                        ast::Stmt::Return(e) => self.infer_expr(e, &fn_env),
                        _ => Type::Unit,
                    }
                };
                let fn_type = Type::Fn(param_types, Box::new(ret_type));
                env.insert(fndef.name.clone(), (fn_type, vec![]));
            },
            ast::Item::Mod(_, items) => {
                let mut mod_env = env.clone();
                for sub in items {
                    self.check_item(sub, &mut mod_env);
                }
                env.extend(mod_env);
            },
            ast::Item::TypeAlias(_, _) => {},
            // Nominal data types: recorded structurally elsewhere; inference treats
            // their constructors as opaque for now (no field/variant typing yet).
            ast::Item::Struct(_, _) => {},
            ast::Item::Enum(_, _) => {},
            ast::Item::Use { .. } => {},
        }
    }

    fn check_stmt(&mut self, stmt: &ast::Stmt, env: &mut TypeEnv) {
        match stmt {
            ast::Stmt::Bind(name, expr) => {
                let t = self.infer_expr(expr, env);
                let (gt, generic) = self.generalize(t, env);
                env.insert(name.clone(), (gt, generic));
            },
            ast::Stmt::Expr(expr) => {
                self.infer_expr(expr, env);
            },
            ast::Stmt::Return(expr) => {
                self.infer_expr(expr, env);
            },
        }
    }

    fn infer_expr(&mut self, expr: &ast::Expr, env: &TypeEnv) -> Type {
        match expr {
            ast::Expr::Str(_) => Type::Str,
            ast::Expr::Number(_) => Type::Float,
            ast::Expr::Bool(_) => Type::Bool,
            ast::Expr::Unit => Type::Unit,
            ast::Expr::Ident(name) => self.lookup_env(name, env).unwrap_or_else(|| {
                self.errors.push(format!("undefined name `{}`", name));
                Type::Any
            }),
            ast::Expr::BinOp(op, lhs, rhs) => {
                let lt = self.infer_expr(lhs, env);
                let rt = self.infer_expr(rhs, env);
                match op {
                    ast::BinOp::Add
                    | ast::BinOp::Sub
                    | ast::BinOp::Mul
                    | ast::BinOp::Div
                    | ast::BinOp::Rem => {
                        self.unify(lt.clone(), rt.clone());
                        lt
                    },
                    ast::BinOp::Eq
                    | ast::BinOp::Ne
                    | ast::BinOp::Lt
                    | ast::BinOp::Le
                    | ast::BinOp::Gt
                    | ast::BinOp::Ge => {
                        self.unify(lt, rt);
                        Type::Bool
                    },
                    ast::BinOp::And | ast::BinOp::Or => {
                        self.unify(lt.clone(), Type::Bool);
                        self.unify(rt, Type::Bool);
                        Type::Bool
                    },
                }
            },
            ast::Expr::Call(callee, args) => {
                let callee_t = self.infer_expr(callee, env);
                let mut arg_types = Vec::new();
                for arg in args {
                    arg_types.push(self.infer_expr(arg, env));
                }
                let ret_t = self.new_var();
                let expected = Type::Fn(arg_types, Box::new(ret_t.clone()));
                self.unify(callee_t, expected);
                ret_t
            },
            ast::Expr::MethodCall { receiver, args, .. } => {
                let _recv_t = self.infer_expr(receiver, env);
                for arg in args {
                    self.infer_expr(arg, env);
                }
                Type::Float
            },
            ast::Expr::If { cond, then, elseifs, else_body } => {
                let cond_t = self.infer_expr(cond, env);
                self.unify(cond_t, Type::Bool);
                let then_t = self.infer_block(then, env);
                let mut result = then_t;
                for (_econd, ebody) in elseifs {
                    let et = self.infer_block(ebody, env);
                    self.unify(result.clone(), et);
                }
                if let Some(ebody) = else_body {
                    let et = self.infer_block(ebody, env);
                    self.unify(result.clone(), et);
                } else {
                    self.unify(result.clone(), Type::Unit);
                    result = Type::Unit;
                }
                result
            },
            ast::Expr::While { cond, body } => {
                let cond_t = self.infer_expr(cond, env);
                self.unify(cond_t, Type::Bool);
                let mut block_env = env.clone();
                for stmt in body {
                    self.check_stmt(stmt, &mut block_env);
                }
                Type::Unit
            },
            ast::Expr::For { var, iter, body } => {
                let iter_t = self.infer_expr(iter, env);
                let elem_t = self.new_var();
                self.unify(iter_t, Type::List(Box::new(elem_t.clone())));
                let mut for_env = env.clone();
                for_env.insert(var.clone(), (elem_t, vec![]));
                for stmt in body {
                    self.check_stmt(stmt, &mut for_env);
                }
                Type::Unit
            },
            ast::Expr::Match(scrutinee, arms) => {
                let scrut_t = self.infer_expr(scrutinee, env);
                let result_t = self.new_var();
                for arm in arms {
                    match &arm.pattern {
                        ast::Pattern::Ident(name) => {
                            let mut arm_env = env.clone();
                            arm_env.insert(name.clone(), (scrut_t.clone(), vec![]));
                            let arm_t = self.infer_expr(&arm.body, &arm_env);
                            self.unify(result_t.clone(), arm_t);
                        },
                        _ => {
                            let arm_t = self.infer_expr(&arm.body, env);
                            self.unify(result_t.clone(), arm_t);
                        },
                    }
                }
                result_t
            },
            ast::Expr::Array(elems) => {
                if elems.is_empty() {
                    Type::List(Box::new(self.new_var()))
                } else {
                    let elem_t = self.infer_expr(&elems[0], env);
                    for e in &elems[1..] {
                        let et = self.infer_expr(e, env);
                        self.unify(elem_t.clone(), et);
                    }
                    Type::List(Box::new(elem_t))
                }
            },
            ast::Expr::Range(lo, hi) => {
                let lo_t = self.infer_expr(lo, env);
                let hi_t = self.infer_expr(hi, env);
                self.unify(lo_t, hi_t);
                Type::List(Box::new(Type::Float))
            },
            ast::Expr::Index(base, idx) => {
                let base_t = self.infer_expr(base, env);
                let idx_t = self.infer_expr(idx, env);
                self.unify(idx_t, Type::Float);
                let elem_t = self.new_var();
                self.unify(base_t, Type::List(Box::new(elem_t.clone())));
                elem_t
            },
            ast::Expr::Ref(inner) => self.infer_expr(inner, env),
            ast::Expr::Closure(params, body) => {
                let mut closure_env = env.clone();
                let mut param_types = Vec::new();
                for pname in params {
                    let tv = self.new_var();
                    closure_env.insert(pname.clone(), (tv.clone(), vec![]));
                    param_types.push(tv);
                }
                let body_t = self.infer_expr(body, &closure_env);
                Type::Fn(param_types, Box::new(body_t))
            },
            ast::Expr::Do(stmts) => self.infer_block(stmts, env),
            ast::Expr::Path(parts) => env
                .get(&parts.join("::"))
                .map(|(t, _)| self.subst(t))
                .unwrap_or(Type::Any),
            ast::Expr::Await(_) => self.new_var(),
        }
    }

    fn infer_block(&mut self, stmts: &[ast::Stmt], env: &TypeEnv) -> Type {
        let mut block_env = env.clone();
        let mut last_type = Type::Unit;
        for stmt in stmts {
            match stmt {
                ast::Stmt::Bind(name, expr) => {
                    let t = self.infer_expr(expr, &block_env);
                    let (gt, generic) = self.generalize(t, &block_env);
                    block_env.insert(name.clone(), (gt, generic));
                },
                ast::Stmt::Expr(expr) => {
                    last_type = self.infer_expr(expr, &block_env);
                },
                ast::Stmt::Return(expr) => {
                    last_type = self.infer_expr(expr, &block_env);
                },
            }
        }
        last_type
    }
}

impl Default for TypeChecker {
    fn default() -> Self {
        Self::new()
    }
}
