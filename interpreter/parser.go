package interpreter

import (
	"fmt"
	"os"
	"strconv"
)

type Parser struct {
	tokens []Token
	pos    int
}

func NewParser(tokens []Token) *Parser {
	return &Parser{tokens: tokens}
}

func (p *Parser) peek() Token {
	if p.pos >= len(p.tokens) {
		return Token{TOKEN_EOF, "", 0, 0}
	}
	return p.tokens[p.pos]
}

func (p *Parser) advance() Token {
	t := p.tokens[p.pos]
	p.pos++
	return t
}

func (p *Parser) expect(tt TokenType) Token {
	t := p.peek()
	if t.Type != tt {
		fmt.Fprintf(os.Stderr, "line %d:%d: expected %s, got %s (%q)\n",
			t.Line, t.Col, tokenNames[tt], tokenNames[t.Type], t.Value)
		os.Exit(1)
	}
	return p.advance()
}

func (p *Parser) match(tt TokenType) bool {
	if p.peek().Type == tt {
		p.advance()
		return true
	}
	return false
}

func (p *Parser) atIdent(v string) bool {
	return p.peek().Type == TOKEN_IDENT && p.peek().Value == v
}

func (p *Parser) parseType() BType {
	t := p.expect(TOKEN_IDENT)
	switch t.Value {
	case "float":
		return TYPE_FLOAT
	case "string":
		return TYPE_STRING
	case "bool":
		return TYPE_BOOL
	case "bel":
		return TYPE_BEL
	case "ster":
		return TYPE_STER
	case "any":
		return TYPE_ANY
	default:
		fmt.Fprintf(os.Stderr, "line %d:%d: unknown type '%s'\n", t.Line, t.Col, t.Value)
		os.Exit(1)
	}
	return TYPE_ANY
}

func (p *Parser) Parse() *Program {
	prog := &Program{}
	for p.peek().Type != TOKEN_EOF {
		prog.Stmts = append(prog.Stmts, p.parseStmt())
	}
	return prog
}

func (p *Parser) parseStmt() ASTNode {
	if p.atIdent("var") {
		return p.parseVarDecl()
	}
	if p.atIdent("fn") {
		return p.parseFnDecl()
	}
	if p.atIdent("if") {
		return p.parseIf()
	}
	if p.atIdent("while") {
		return p.parseWhile()
	}
	if p.atIdent("for") {
		return p.parseFor()
	}
	if p.atIdent("return") {
		return p.parseReturn()
	}
	if p.atIdent("break") {
		p.advance()
		p.match(TOKEN_SEMICOLON)
		return &BreakStmt{}
	}
	if p.atIdent("continue") {
		p.advance()
		p.match(TOKEN_SEMICOLON)
		return &ContinueStmt{}
	}
	if p.peek().Type == TOKEN_LBRACE {
		return p.parseBlock()
	}

	if p.atIdent("idb") {
		return p.parseIdbDecl()
	}

	if p.peek().Type == TOKEN_IDENT && isTypeName(p.peek().Value) {
		return p.parseTypedDecl()
	}

	expr := p.parseExpr()

	if p.peek().Type == TOKEN_EQ {
		p.advance()
		val := p.parseExpr()
		p.match(TOKEN_SEMICOLON)
		return &Assign{Target: expr, Value: val}
	}
	if p.peek().Type == TOKEN_PLUS_EQ {
		p.advance()
		val := p.parseExpr()
		p.match(TOKEN_SEMICOLON)
		return &Assign{Target: expr, Value: &BinExpr{Op: "+", Left: expr, Right: val}}
	}
	if p.peek().Type == TOKEN_MINUS_EQ {
		p.advance()
		val := p.parseExpr()
		p.match(TOKEN_SEMICOLON)
		return &Assign{Target: expr, Value: &BinExpr{Op: "-", Left: expr, Right: val}}
	}

	p.match(TOKEN_SEMICOLON)
	return &ExprStmt{Expr: expr}
}

func (p *Parser) parseBlock() ASTNode {
	p.expect(TOKEN_LBRACE)
	b := &BlockStmt{}
	for p.peek().Type != TOKEN_RBRACE && p.peek().Type != TOKEN_EOF {
		b.Body = append(b.Body, p.parseStmt())
	}
	p.expect(TOKEN_RBRACE)
	return b
}

func (p *Parser) parseIf() ASTNode {
	p.advance()
	cond := p.parseExpr()
	body := p.parseBlock()
	var els ASTNode
	if p.atIdent("else") {
		p.advance()
		if p.atIdent("if") {
			els = p.parseIf()
		} else {
			els = p.parseBlock()
		}
	}
	return &IfStmt{Cond: cond, Body: body, Else: els}
}

func (p *Parser) parseWhile() ASTNode {
	p.advance()
	cond := p.parseExpr()
	body := p.parseBlock()
	return &WhileStmt{Cond: cond, Body: body}
}

func (p *Parser) parseFor() ASTNode {
	p.advance()
	varName := p.expect(TOKEN_IDENT).Value
	p.expect(TOKEN_IDENT) // "in"
	iter := p.parseExpr()
	body := p.parseBlock()
	return &ForStmt{Var: varName, Iter: iter, Body: body}
}

func (p *Parser) parseVarDecl() ASTNode {
	p.advance()
	name := p.expect(TOKEN_IDENT).Value

	var varType BType
	hasType := false
	if p.match(TOKEN_COLON) {
		varType = p.parseType()
		hasType = true
	}

	p.expect(TOKEN_EQ)
	val := p.parseExpr()
	p.match(TOKEN_SEMICOLON)
	return &VarDecl{Name: name, VarType: varType, Value: val, HasType: hasType}
}

func (p *Parser) parseIdbDecl() ASTNode {
	p.advance()
	num := p.expect(TOKEN_NUMBER)
	id, err := strconv.Atoi(num.Value)
	if err != nil {
		fmt.Fprintf(os.Stderr, "line %d:%d: invalid idb id '%s'\n", num.Line, num.Col, num.Value)
		os.Exit(1)
	}
	p.expect(TOKEN_EQ)
	val := p.parseExpr()
	p.match(TOKEN_SEMICOLON)
	return &IdbDecl{Id: id, Value: val}
}

func isTypeName(name string) bool {
	switch name {
	case "float", "string", "bool", "bel", "ster", "any":
		return true
	}
	return false
}

func (p *Parser) parseTypedDecl() ASTNode {
	typeName := p.advance().Value
	name := p.expect(TOKEN_IDENT).Value
	p.expect(TOKEN_EQ)
	val := p.parseExpr()
	p.match(TOKEN_SEMICOLON)

	var btype BType
	isBel := false
	isSter := false
	switch typeName {
	case "float":
		btype = TYPE_FLOAT
	case "string":
		btype = TYPE_STRING
	case "bool":
		btype = TYPE_BOOL
	case "bel":
		btype = TYPE_BEL
		isBel = true
	case "ster":
		btype = TYPE_STER
		isSter = true
	case "any":
		btype = TYPE_ANY
	}
	return &VarDecl{Name: name, VarType: btype, Value: val, HasType: true, IsBel: isBel, IsSter: isSter}
}

func (p *Parser) parseFnDecl() ASTNode {
	p.advance()
	name := p.expect(TOKEN_IDENT).Value
	p.expect(TOKEN_LPAREN)

	var params []TypedParam
	if p.peek().Type != TOKEN_RPAREN {
		pName := p.expect(TOKEN_IDENT).Value
		pType := TYPE_ANY
		if p.match(TOKEN_COLON) {
			pType = p.parseType()
		}
		params = append(params, TypedParam{Name: pName, Type: pType})
		for p.match(TOKEN_COMMA) {
			pName = p.expect(TOKEN_IDENT).Value
			pType = TYPE_ANY
			if p.match(TOKEN_COLON) {
				pType = p.parseType()
			}
			params = append(params, TypedParam{Name: pName, Type: pType})
		}
	}
	p.expect(TOKEN_RPAREN)

	var retType BType
	hasRetType := false
	if p.match(TOKEN_COLON) {
		retType = p.parseType()
		hasRetType = true
	}

	body := p.parseBlock()
	return &FnDecl{Name: name, Params: params, ReturnType: retType, HasRetType: hasRetType, Body: body}
}

func (p *Parser) parseReturn() ASTNode {
	p.advance()
	var val ASTNode
	if p.peek().Type != TOKEN_SEMICOLON && p.peek().Type != TOKEN_RBRACE && p.peek().Type != TOKEN_EOF {
		val = p.parseExpr()
	}
	p.match(TOKEN_SEMICOLON)
	return &ReturnStmt{Value: val}
}

func (p *Parser) parseExpr() ASTNode { return p.parseOr() }

func (p *Parser) parseOr() ASTNode {
	left := p.parseAnd()
	for p.peek().Type == TOKEN_OR {
		p.advance()
		right := p.parseAnd()
		left = &BinExpr{Op: "||", Left: left, Right: right}
	}
	return left
}

func (p *Parser) parseAnd() ASTNode {
	left := p.parseComparison()
	for p.peek().Type == TOKEN_AND {
		p.advance()
		right := p.parseComparison()
		left = &BinExpr{Op: "&&", Left: left, Right: right}
	}
	return left
}

func (p *Parser) parseComparison() ASTNode {
	left := p.parseAddSub()
	for {
		t := p.peek()
		if t.Type == TOKEN_EQEQ || t.Type == TOKEN_NEQ ||
			t.Type == TOKEN_LT || t.Type == TOKEN_GT ||
			t.Type == TOKEN_LTE || t.Type == TOKEN_GTE {
			p.advance()
			right := p.parseAddSub()
			left = &BinExpr{Op: t.Value, Left: left, Right: right}
		} else {
			break
		}
	}
	return left
}

func (p *Parser) parseAddSub() ASTNode {
	left := p.parseMulDiv()
	for {
		t := p.peek()
		if t.Type == TOKEN_PLUS || t.Type == TOKEN_MINUS {
			p.advance()
			right := p.parseMulDiv()
			left = &BinExpr{Op: t.Value, Left: left, Right: right}
		} else {
			break
		}
	}
	return left
}

func (p *Parser) parseMulDiv() ASTNode {
	left := p.parseUnary()
	for {
		t := p.peek()
		if t.Type == TOKEN_STAR || t.Type == TOKEN_SLASH || t.Type == TOKEN_PERCENT {
			p.advance()
			right := p.parseUnary()
			left = &BinExpr{Op: t.Value, Left: left, Right: right}
		} else {
			break
		}
	}
	return left
}

func (p *Parser) parseUnary() ASTNode {
	if p.peek().Type == TOKEN_MINUS {
		p.advance()
		return &UnaryExpr{Op: "-", Expr: p.parsePostfix()}
	}
	if p.peek().Type == TOKEN_NOT {
		p.advance()
		return &UnaryExpr{Op: "!", Expr: p.parsePostfix()}
	}
	return p.parsePostfix()
}

func (p *Parser) parsePostfix() ASTNode {
	node := p.parsePrimary()
	for {
		if p.peek().Type == TOKEN_LPAREN {
			p.advance()
			var call *CallExpr
			switch n := node.(type) {
			case *CallExpr:
				call = n
			case *Ident:
				call = &CallExpr{Name: n.Name}
			default:
				fmt.Fprintf(os.Stderr, "line %d: not callable\n", p.peek().Line)
				os.Exit(1)
			}
			if p.peek().Type != TOKEN_RPAREN {
				call.Args = append(call.Args, p.parseExpr())
				for p.match(TOKEN_COMMA) {
					call.Args = append(call.Args, p.parseExpr())
				}
			}
			p.expect(TOKEN_RPAREN)
			node = call
		} else if p.peek().Type == TOKEN_DOT {
			p.advance()
			right := p.expect(TOKEN_IDENT).Value
			node = &DotExpr{Left: node, Right: right}
		} else if p.peek().Type == TOKEN_LBRACKET {
			p.advance()
			idx := p.parseExpr()
			p.expect(TOKEN_RBRACKET)
			node = &IndexExpr{Obj: node, Index: idx}
		} else {
			break
		}
	}
	return node
}

func (p *Parser) parsePrimary() ASTNode {
	t := p.peek()
	switch t.Type {
	case TOKEN_STRING:
		p.advance()
		return &StringLit{Value: t.Value}
	case TOKEN_NUMBER:
		p.advance()
		return &NumLit{Value: t.Value}
	case TOKEN_IDENT:
		p.advance()
		switch t.Value {
		case "true":
			return &BoolLit{Value: true}
		case "false":
			return &BoolLit{Value: false}
		case "null":
			return &Ident{Name: "null"}
		default:
			return &Ident{Name: t.Value}
		}
	case TOKEN_LPAREN:
		p.advance()
		expr := p.parseExpr()
		p.expect(TOKEN_RPAREN)
		return expr
	case TOKEN_LBRACKET:
		return p.parseArrayLit()
	default:
		fmt.Fprintf(os.Stderr, "line %d:%d: unexpected token '%s' (%q)\n",
			t.Line, t.Col, tokenNames[t.Type], t.Value)
		os.Exit(1)
	}
	return nil
}

func (p *Parser) parseArrayLit() ASTNode {
	p.expect(TOKEN_LBRACKET)
	al := &ArrayLit{}
	if p.peek().Type != TOKEN_RBRACKET {
		al.Elems = append(al.Elems, p.parseExpr())
		for p.match(TOKEN_COMMA) {
			al.Elems = append(al.Elems, p.parseExpr())
		}
	}
	p.expect(TOKEN_RBRACKET)
	return al
}
