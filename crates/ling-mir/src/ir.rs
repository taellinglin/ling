use ling_ast::ast::{BinOp, UnOp};
use ling_ast::Span;
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Local(pub usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BasicBlockId(pub usize);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operand {
    Copy(Local),
    Move(Local),
    Constant(Constant),
}

impl Hash for Operand {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Operand::Copy(l) => {
                0u8.hash(state);
                l.hash(state);
            },
            Operand::Move(l) => {
                1u8.hash(state);
                l.hash(state);
            },
            Operand::Constant(c) => {
                2u8.hash(state);
                c.hash(state);
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Constant {
    I64(i64),
    F64(u64),
    Str(String),
    Bool(bool),
    Function(String),
    GlobalData(String),
    None,
}

impl Hash for Constant {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Constant::I64(v) => {
                0u8.hash(state);
                v.hash(state);
            },
            Constant::F64(v) => {
                1u8.hash(state);
                v.hash(state);
            },
            Constant::Str(v) => {
                2u8.hash(state);
                v.hash(state);
            },
            Constant::Bool(v) => {
                3u8.hash(state);
                v.hash(state);
            },
            Constant::Function(v) => {
                4u8.hash(state);
                v.hash(state);
            },
            Constant::GlobalData(v) => {
                5u8.hash(state);
                v.hash(state);
            },
            Constant::None => {
                6u8.hash(state);
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AggregateKind {
    Tuple,
    List,
    Set,
    Dict,
    Map,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rvalue {
    Use(Operand),
    BinaryOp(BinOp, Operand, Operand),
    UnaryOp(UnOp, Operand),
    Call { func: Operand, args: Vec<Operand> },
    Aggregate(AggregateKind, Vec<Operand>),
    GetAttr(Operand, String),
    GetIndex(Operand, Operand),
    Ref(Local),
    MutRef(Local),
    VectorSplat(Operand, usize),
    VectorLoad(Operand, Operand, usize),
    VectorFMA(Operand, Operand, Operand),
}

impl Hash for Rvalue {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Rvalue::Use(op) => {
                0u8.hash(state);
                op.hash(state);
            },
            Rvalue::BinaryOp(op, l, r) => {
                1u8.hash(state);
                (*op as u8).hash(state);
                l.hash(state);
                r.hash(state);
            },
            Rvalue::UnaryOp(op, o) => {
                2u8.hash(state);
                (*op as u8).hash(state);
                o.hash(state);
            },
            Rvalue::Call { func, args } => {
                3u8.hash(state);
                func.hash(state);
                args.hash(state);
            },
            Rvalue::Aggregate(kind, ops) => {
                4u8.hash(state);
                kind.hash(state);
                ops.hash(state);
            },
            Rvalue::GetAttr(op, name) => {
                5u8.hash(state);
                op.hash(state);
                name.hash(state);
            },
            Rvalue::GetIndex(l, r) => {
                6u8.hash(state);
                l.hash(state);
                r.hash(state);
            },
            Rvalue::Ref(l) => {
                7u8.hash(state);
                l.hash(state);
            },
            Rvalue::MutRef(l) => {
                8u8.hash(state);
                l.hash(state);
            },
            Rvalue::VectorSplat(op, w) => {
                9u8.hash(state);
                op.hash(state);
                w.hash(state);
            },
            Rvalue::VectorLoad(obj, idx, w) => {
                10u8.hash(state);
                obj.hash(state);
                idx.hash(state);
                w.hash(state);
            },
            Rvalue::VectorFMA(a, b, c) => {
                11u8.hash(state);
                a.hash(state);
                b.hash(state);
                c.hash(state);
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    pub kind: StatementKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatementKind {
    Assign(Local, Rvalue),
    SetAttr(Operand, String, Operand),
    SetIndex(Operand, Operand, Operand),
    StorageLive(Local),
    StorageDead(Local),
    Drop(Local),
    VectorStore(Operand, Operand, Operand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminatorKind {
    Goto {
        target: BasicBlockId,
    },
    SwitchInt {
        discr: Operand,
        targets: Vec<(i64, BasicBlockId)>,
        otherwise: BasicBlockId,
    },
    Return,
    Unreachable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Terminator {
    pub kind: TerminatorKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasicBlock {
    pub statements: Vec<Statement>,
    pub terminator: Option<Terminator>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDecl {
    pub ty: MirType,
    pub name: Option<String>,
    pub span: Span,
    pub is_mut: bool,
    pub is_owning: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MirType {
    Int,
    Float,
    Bool,
    String,
    Unit,
    Ptr(Box<MirType>),
    Ref(Box<MirType>),
    Tuple(Vec<MirType>),
    List(Box<MirType>),
    Dict,
    Map,
    Function,
    Any,
}

impl MirType {
    pub fn is_move_type(&self) -> bool {
        matches!(
            self,
            MirType::String
                | MirType::List(_)
                | MirType::Dict
                | MirType::Map
                | MirType::Tuple(_)
                | MirType::Any
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirFunction {
    pub name: String,
    pub locals: Vec<LocalDecl>,
    pub basic_blocks: Vec<BasicBlock>,
    pub arg_count: usize,
    pub param_names: Vec<String>,
}

impl MirFunction {
    pub fn new(name: impl Into<String>, arg_count: usize) -> Self {
        Self {
            name: name.into(),
            locals: Vec::new(),
            basic_blocks: vec![BasicBlock {
                statements: Vec::new(),
                terminator: Some(Terminator { kind: TerminatorKind::Return, span: Span::DUMMY }),
            }],
            arg_count,
            param_names: Vec::new(),
        }
    }

    /// Flat `Local` index that the next freshly allocated temporary will take.
    ///
    /// The index space is `Local(0)` for the return slot, `Local(1..=arg_count)`
    /// for parameters, and `Local(arg_count + 1 + k)` for the temporary stored at
    /// `locals[k]`. Parameters and the return slot have no entry in `locals`.
    pub fn next_local(&self) -> usize {
        self.arg_count + 1 + self.locals.len()
    }

    /// Allocate a fresh temporary, append its declaration, and return its `Local`.
    pub fn push_local(&mut self, decl: LocalDecl) -> Local {
        let id = Local(self.next_local());
        self.locals.push(decl);
        id
    }

    /// Declaration backing `local`, or `None` for the return slot and parameters
    /// (which are not represented in `locals`).
    pub fn local_decl(&self, local: Local) -> Option<&LocalDecl> {
        local
            .0
            .checked_sub(self.arg_count + 1)
            .and_then(|k| self.locals.get(k))
    }
}

#[derive(Debug, Clone, Default)]
pub struct MirProgram {
    pub functions: Vec<MirFunction>,
}
