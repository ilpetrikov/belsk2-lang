use std::rc::Rc;

use crate::span::Span;
use crate::types::BType;

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub stmts: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    Expr(Expr),
    /// `var x = e`, `var x: T = e`, `T x = e`
    VarDecl(VarDecl),
    /// `idb 5 = e` — stores a value in the id bank.
    Idb {
        id: i64,
        value: Expr,
    },
    /// `target = value`
    Assign {
        target: Expr,
        value: Expr,
    },
    /// `target += value`, `target -= value`
    CompoundAssign {
        op: BinOp,
        target: Expr,
        value: Expr,
    },
    FnDecl(Rc<FnDecl>),
    Return(Option<Expr>),
    Break,
    Continue,
    If {
        cond: Expr,
        then: Block,
        /// Either a [`StmtKind::Block`] or another [`StmtKind::If`].
        otherwise: Option<Box<Stmt>>,
    },
    While {
        cond: Expr,
        body: Block,
    },
    For {
        var: String,
        iter: Expr,
        body: Block,
    },
    Block(Block),
}

#[derive(Debug, Clone, PartialEq)]
pub struct VarDecl {
    pub name: String,
    pub ty: Option<BType>,
    pub value: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub ty: Option<BType>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FnDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub ret: Option<BType>,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
    /// Height of this expression tree. The parser bounds it so that every
    /// recursive pass over the AST (including `Drop`) stays within the stack.
    pub depth: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Number(f64),
    String(String),
    Bool(bool),
    Null,
    Ident(String),
    Array(Vec<Expr>),
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    Member {
        object: Box<Expr>,
        name: String,
    },
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
    },
}

impl Expr {
    pub fn new(kind: ExprKind, span: Span) -> Self {
        let child_depth = match &kind {
            ExprKind::Number(_)
            | ExprKind::String(_)
            | ExprKind::Bool(_)
            | ExprKind::Null
            | ExprKind::Ident(_) => 0,
            ExprKind::Array(items) => max_depth(items.iter()),
            ExprKind::Unary { expr, .. } => expr.depth,
            ExprKind::Binary { lhs, rhs, .. } => lhs.depth.max(rhs.depth),
            ExprKind::Call { callee, args } => callee.depth.max(max_depth(args.iter())),
            ExprKind::Member { object, .. } => object.depth,
            ExprKind::Index { object, index } => object.depth.max(index.depth),
        };
        Expr {
            kind,
            span,
            depth: child_depth.saturating_add(1),
        }
    }

    /// Whether this expression may appear on the left of `=`.
    pub fn is_place(&self) -> bool {
        matches!(
            self.kind,
            ExprKind::Ident(_) | ExprKind::Index { .. } | ExprKind::Member { .. }
        )
    }
}

fn max_depth<'a>(items: impl Iterator<Item = &'a Expr>) -> u32 {
    items.map(|e| e.depth).max().unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
}

impl BinOp {
    pub fn symbol(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Gt => ">",
            BinOp::Le => "<=",
            BinOp::Ge => ">=",
            BinOp::And => "&&",
            BinOp::Or => "||",
        }
    }
}

impl UnaryOp {
    pub fn symbol(self) -> &'static str {
        match self {
            UnaryOp::Neg => "-",
            UnaryOp::Not => "!",
        }
    }
}
