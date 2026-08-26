use crate::types::BType;

#[derive(Debug, Clone)]
pub enum ASTNode {
    Program(Program),
    ExprStmt(Box<ExprStmt>),
    CallExpr(Box<CallExpr>),
    StringLit(Box<StringLit>),
    NumLit(Box<NumLit>),
    BoolLit(Box<BoolLit>),
    Ident(Box<Ident>),
    BlockStmt(Box<BlockStmt>),
    IfStmt(Box<IfStmt>),
    WhileStmt(Box<WhileStmt>),
    ForStmt(Box<ForStmt>),
    VarDecl(Box<VarDecl>),
    IdbDecl(Box<IdbDecl>),
    Assign(Box<Assign>),
    FnDecl(Box<FnDecl>),
    ReturnStmt(Box<ReturnStmt>),
    BreakStmt,
    ContinueStmt,
    BinExpr(Box<BinExpr>),
    UnaryExpr(Box<UnaryExpr>),
    DotExpr(Box<DotExpr>),
    IndexExpr(Box<IndexExpr>),
    ArrayLit(Box<ArrayLit>),
}

#[derive(Debug, Clone)]
pub struct Program {
    pub stmts: Vec<ASTNode>,
}

#[derive(Debug, Clone)]
pub struct ExprStmt {
    pub expr: Box<ASTNode>,
}

#[derive(Debug, Clone)]
pub struct CallExpr {
    pub name: String,
    pub args: Vec<ASTNode>,
}

#[derive(Debug, Clone)]
pub struct StringLit {
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct NumLit {
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct BoolLit {
    pub value: bool,
}

#[derive(Debug, Clone)]
pub struct Ident {
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct BlockStmt {
    pub body: Vec<ASTNode>,
}

#[derive(Debug, Clone)]
pub struct IfStmt {
    pub cond: Box<ASTNode>,
    pub body: Box<ASTNode>,
    pub else_body: Option<Box<ASTNode>>,
}

#[derive(Debug, Clone)]
pub struct WhileStmt {
    pub cond: Box<ASTNode>,
    pub body: Box<ASTNode>,
}

#[derive(Debug, Clone)]
pub struct ForStmt {
    pub var: String,
    pub iter: Box<ASTNode>,
    pub body: Box<ASTNode>,
}

#[derive(Debug, Clone)]
pub struct VarDecl {
    pub name: String,
    pub var_type: BType,
    pub value: Box<ASTNode>,
    pub has_type: bool,
    pub is_bel: bool,
    pub is_ster: bool,
}

#[derive(Debug, Clone)]
pub struct IdbDecl {
    pub id: i64,
    pub value: Box<ASTNode>,
}

#[derive(Debug, Clone)]
pub struct Assign {
    pub target: Box<ASTNode>,
    pub value: Box<ASTNode>,
}

#[derive(Debug, Clone)]
pub struct TypedParam {
    pub name: String,
    pub param_type: BType,
}

#[derive(Debug, Clone)]
pub struct FnDecl {
    pub name: String,
    pub params: Vec<TypedParam>,
    pub return_type: BType,
    pub has_ret_type: bool,
    pub body: Box<ASTNode>,
}

#[derive(Debug, Clone)]
pub struct ReturnStmt {
    pub value: Option<Box<ASTNode>>,
}

#[derive(Debug, Clone)]
pub struct BinExpr {
    pub op: String,
    pub left: Box<ASTNode>,
    pub right: Box<ASTNode>,
}

#[derive(Debug, Clone)]
pub struct UnaryExpr {
    pub op: String,
    pub expr: Box<ASTNode>,
}

#[derive(Debug, Clone)]
pub struct DotExpr {
    pub left: Box<ASTNode>,
    pub right: String,
}

#[derive(Debug, Clone)]
pub struct IndexExpr {
    pub obj: Box<ASTNode>,
    pub index: Box<ASTNode>,
}

#[derive(Debug, Clone)]
pub struct ArrayLit {
    pub elems: Vec<ASTNode>,
}
