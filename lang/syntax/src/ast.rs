use std::rc::Rc;

use crate::span::Span;
use crate::types::{Access, FloatKind, IntKind, Ty, TypeExpr, TypeInfo, TypeKind, Virt};

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
    /// `using a.b;` — makes the declarations of module `a.b` visible.
    Using(Vec<String>),
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
        /// The type of `target`; the result is converted back to it
        /// (filled in by the checker).
        cast: Ty,
    },
    FnDecl(Rc<FnDecl>),
    /// `struct Point { ... }`, `enum Mode { ... }`, `class Player { ... }`
    Type(Rc<TypeDecl>),
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
        /// Type of the loop variable, filled in by the checker.
        var_ty: Ty,
        iter: Expr,
        body: Block,
    },
    Block(Block),
    /// `switch x { case 1, 2: ... default: ... }`
    Switch {
        subject: Expr,
        cases: Vec<SwitchCase>,
        default: Option<Block>,
    },
    /// `match x { pattern => ..., }` as a statement: arms may be blocks.
    Match(Box<Match>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SwitchCase {
    pub values: Vec<Expr>,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub subject: Expr,
    pub arms: Vec<Arm>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Arm {
    pub pattern: Pattern,
    /// `pattern if cond => ...`
    pub guard: Option<Expr>,
    pub body: ArmBody,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ArmBody {
    Expr(Expr),
    Block(Block),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    pub kind: PatternKind,
    pub span: Span,
    /// What the pattern turned out to be, filled in by the checker.
    pub resolved: PatternRes,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PatternKind {
    /// `_`
    Wildcard,
    /// `n`: binds the value, unless it names a case of the enum being
    /// matched (`Red`) or a type (`Dog`).
    Name(String),
    /// `1`, `-2.5`, `"text"`, `true`, `null`
    Literal(Expr),
    /// `Circle(r)`, `Shape.Circle(r)`, `Color.Red`
    Variant {
        path: Vec<String>,
        args: Vec<Pattern>,
    },
    /// `Dog d`: the value is a `Dog`; binds it as one.
    Type { ty: TypeExpr, bind: Option<String> },
    /// `1 | 2 | 3`
    Or(Vec<Pattern>),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum PatternRes {
    #[default]
    Unresolved,
    /// Binds the value to a new variable of this type.
    Bind(Ty),
    /// The case of an enum with this index.
    Variant(usize),
    /// A type test.
    Type(Ty),
}

/// `struct Point { x: double; fn len(): double { ... } }`
#[derive(Debug, Clone, PartialEq)]
pub struct TypeDecl {
    pub name: String,
    pub kind: TypeKind,
    /// `private struct ...` is visible only in its module.
    pub access: Access,
    /// `abstract class`: cannot be created, may have abstract methods.
    pub is_abstract: bool,
    /// `partial class`: the type may be declared in several parts (in one
    /// file), which the checker joins into the first.
    pub partial: bool,
    /// The base class and the implemented interfaces, after `:`.
    pub bases: Vec<TypeExpr>,
    pub members: Vec<Member>,
    pub span: Span,
    /// The type, created by the checker. The declaration owns it.
    pub info: Option<Rc<TypeInfo>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Member {
    pub kind: MemberKind,
    pub mods: Modifiers,
    pub span: Span,
}

/// `public`, `private`, `protected`, `static`, `virtual`, `override`,
/// `abstract`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Modifiers {
    /// `None` when not written: public.
    pub access: Option<Access>,
    pub is_static: bool,
    pub virt: Virt,
}

impl Modifiers {
    pub fn access(&self) -> Access {
        self.access.unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MemberKind {
    /// `x: int;`, `x: int = 1;`, `var x = 1;`, `int x;`
    Field(Box<FieldDecl>),
    /// `fn name(...) { ... }`; abstract and interface methods have no body.
    Method(Rc<FnDecl>),
    /// `Point(...) { ... }`, the constructor (named like the type, as in
    /// C#). `Dog(...) : base(args)` becomes a call `base(args)` at the
    /// start of the body.
    Init(Rc<FnDecl>),
    /// `~Player() { ... }`, the destructor: runs when the garbage
    /// collector frees the object.
    Drop(Rc<FnDecl>),
    /// A property with `get`/`set` bodies.
    Prop(Box<PropDecl>),
    /// A case of an enum: `High`, `High = 10` or `Circle(radius: double)`.
    Case {
        name: String,
        value: Option<Box<Expr>>,
        fields: Vec<Param>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldDecl {
    pub name: String,
    pub ty: Option<TypeExpr>,
    pub value: Option<Expr>,
    /// Filled in by the checker.
    pub resolved: Ty,
    /// Declared as an auto-property: `Name: string { get; set; }`.
    pub property: bool,
    /// `{ get; }`: assigned only by its value and `init`.
    pub readonly: bool,
    /// `{ get; private set; }`
    pub set_access: Option<Access>,
}

/// `Area: double { get { ... } set { ... } }`
#[derive(Debug, Clone, PartialEq)]
pub struct PropDecl {
    pub name: String,
    pub ty: TypeExpr,
    /// Filled in by the checker.
    pub resolved: Ty,
    pub get: Option<Accessor>,
    pub set: Option<Accessor>,
}

/// `get { ... }` (a function returning the property's type) or
/// `set { ... }` (a function taking `value`).
#[derive(Debug, Clone, PartialEq)]
pub struct Accessor {
    pub access: Option<Access>,
    pub decl: Rc<FnDecl>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VarDecl {
    pub name: String,
    pub ty: Option<TypeExpr>,
    /// The variable's type, filled in by the checker (inferred for `var`).
    pub resolved: Ty,
    pub value: Expr,
    /// `private var x` (at the top level) is visible only in its module.
    pub access: Access,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub ty: Option<TypeExpr>,
    /// Filled in by the checker; `any` when no type is written.
    pub resolved: Ty,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FnDecl {
    pub name: String,
    /// `T` and `U` in `fn f<T, U>(...)`.
    pub type_params: Vec<String>,
    pub params: Vec<Param>,
    pub ret: Option<TypeExpr>,
    /// Filled in by the checker; `any` when no type is written.
    pub ret_resolved: Ty,
    pub body: Block,
    pub span: Span,
    /// `private fn f` (at the top level) is visible only in its module.
    pub access: Access,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
    /// The static type, filled in by the checker (`any` before that).
    pub ty: Ty,
    /// Height of this expression tree. The parser bounds it so that every
    /// recursive pass over the AST (including `Drop`) stays within the stack.
    pub depth: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    /// An integer literal and its type (`1` is `int`, `1L` is `long`, ...).
    Int(i128, IntKind),
    /// A floating-point literal (`1.5` is `double`, `1.5f` is `float`).
    Float(f64, FloatKind),
    String(String),
    Bool(bool),
    Null,
    Ident(String),
    Array(Vec<Expr>),
    /// `{ key: value, ... }`, a dictionary literal.
    Map(Vec<(Expr, Expr)>),
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
        /// Explicit type arguments: `int` in `empty<int>()`.
        type_args: Vec<TypeExpr>,
        args: Vec<Expr>,
        /// The type arguments of a generic call, written or inferred,
        /// filled in by the checker. Generics are reified: the interpreter
        /// uses these so that `empty<int>()` really creates an `int[]`.
        inst: Vec<Ty>,
    },
    Member {
        object: Box<Expr>,
        name: String,
    },
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
    },
    /// An implicit conversion inserted by the checker (`int` to `double`,
    /// one-character string to `char`, ...).
    Convert {
        expr: Box<Expr>,
        to: Ty,
    },
    /// `this`, the value a method runs on.
    This,
    /// `base` in `base.method()` (and `base(...)` at the start of `init`).
    Base,
    /// `x is Dog`
    Is {
        expr: Box<Expr>,
        ty: TypeExpr,
        resolved: Ty,
    },
    /// `x as Dog`: `x` if it is a `Dog`, else `null`.
    As {
        expr: Box<Expr>,
        ty: TypeExpr,
        resolved: Ty,
    },
    /// `match x { pattern => value, ... }`
    Match(Box<Match>),
    /// The type whose method is running, inserted by the checker for a
    /// static member used without its type name (`count` for
    /// `Counter.count`).
    SelfType,
}

impl Expr {
    pub fn new(kind: ExprKind, span: Span) -> Self {
        let child_depth = match &kind {
            ExprKind::Int(..)
            | ExprKind::Float(..)
            | ExprKind::String(_)
            | ExprKind::Bool(_)
            | ExprKind::Null
            | ExprKind::Ident(_)
            | ExprKind::This
            | ExprKind::Base
            | ExprKind::SelfType => 0,
            ExprKind::Array(items) => max_depth(items.iter()),
            ExprKind::Map(entries) => max_depth(entries.iter().flat_map(|(k, v)| [k, v])),
            ExprKind::Unary { expr, .. } => expr.depth,
            ExprKind::Binary { lhs, rhs, .. } => lhs.depth.max(rhs.depth),
            ExprKind::Call { callee, args, .. } => callee.depth.max(max_depth(args.iter())),
            ExprKind::Member { object, .. } => object.depth,
            ExprKind::Index { object, index } => object.depth.max(index.depth),
            ExprKind::Convert { expr, .. }
            | ExprKind::Is { expr, .. }
            | ExprKind::As { expr, .. } => expr.depth,
            ExprKind::Match(m) => {
                m.subject
                    .depth
                    .max(max_depth(m.arms.iter().filter_map(|a| match &a.body {
                        ArmBody::Expr(e) => Some(e),
                        ArmBody::Block(_) => None,
                    })))
            }
        };
        Expr {
            kind,
            span,
            ty: Ty::Any,
            depth: child_depth.saturating_add(1),
        }
    }

    /// Whether this expression may appear on the left of `=`.
    pub fn is_place(&self) -> bool {
        matches!(
            self.kind,
            ExprKind::Ident(_) | ExprKind::Index { .. } | ExprKind::Member { .. } | ExprKind::This
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
    /// `~`
    BitNot,
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
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
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
            BinOp::BitAnd => "&",
            BinOp::BitOr => "|",
            BinOp::BitXor => "^",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
        }
    }
}

impl UnaryOp {
    pub fn symbol(self) -> &'static str {
        match self {
            UnaryOp::Neg => "-",
            UnaryOp::Not => "!",
            UnaryOp::BitNot => "~",
        }
    }
}
