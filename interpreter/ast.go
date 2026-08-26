package interpreter

type ASTNode interface{}

type Program struct{ Stmts []ASTNode }
type ExprStmt struct{ Expr ASTNode }
type CallExpr struct {
	Name string
	Args []ASTNode
}
type StringLit struct{ Value string }
type NumLit struct{ Value string }
type BoolLit struct{ Value bool }
type Ident struct{ Name string }
type BlockStmt struct{ Body []ASTNode }
type IfStmt struct {
	Cond ASTNode
	Body ASTNode
	Else ASTNode
}
type WhileStmt struct {
	Cond ASTNode
	Body ASTNode
}
type ForStmt struct {
	Var  string
	Iter ASTNode
	Body ASTNode
}
type VarDecl struct {
	Name    string
	VarType BType
	Value   ASTNode
	HasType bool
	IsBel   bool
	IsSter  bool
}
type IdbDecl struct {
	Id    int
	Value ASTNode
}
type Assign struct {
	Target ASTNode
	Value  ASTNode
}
type TypedParam struct {
	Name string
	Type BType
}
type FnDecl struct {
	Name       string
	Params     []TypedParam
	ReturnType BType
	HasRetType bool
	Body       ASTNode
}
type ReturnStmt struct{ Value ASTNode }
type BreakStmt struct{}
type ContinueStmt struct{}
type BinExpr struct {
	Op    string
	Left  ASTNode
	Right ASTNode
}
type UnaryExpr struct {
	Op   string
	Expr ASTNode
}
type DotExpr struct {
	Left  ASTNode
	Right string
}
type IndexExpr struct {
	Obj   ASTNode
	Index ASTNode
}
type ArrayLit struct {
	Elems []ASTNode
}
