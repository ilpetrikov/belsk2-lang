use crate::ast::*;
use crate::token::{Token, TokenType};
use crate::types::BType;

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

fn is_type_name(name: &str) -> bool {
    matches!(name, "float" | "string" | "bool" | "bel" | "ster" | "any")
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Parser { tokens, pos: 0 }
    }

    fn peek(&self) -> &Token {
        if self.pos >= self.tokens.len() {
            static EOF: Token = Token {
                tt: TokenType::Eof,
                value: String::new(),
                line: 0,
                col: 0,
            };
            &EOF
        } else {
            &self.tokens[self.pos]
        }
    }

    fn advance(&mut self) -> Token {
        let t = self.tokens[self.pos].clone();
        self.pos += 1;
        t
    }

    fn expect(&mut self, tt: TokenType) -> Token {
        let t = self.peek().clone();
        if t.tt != tt {
            panic!(
                "line {}:{}: expected {}, got {} ({})",
                t.line,
                t.col,
                tt.name(),
                t.tt.name(),
                t.value
            );
        }
        self.advance()
    }

    fn match_token(&mut self, tt: TokenType) -> bool {
        if self.peek().tt == tt {
            self.advance();
            true
        } else {
            false
        }
    }

    fn at_ident(&self, v: &str) -> bool {
        self.peek().tt == TokenType::Ident && self.peek().value == v
    }

    fn parse_type(&mut self) -> BType {
        let t = self.expect(TokenType::Ident);
        match t.value.as_str() {
            "float" => BType::Float,
            "string" => BType::String,
            "bool" => BType::Bool,
            "bel" => BType::Bel,
            "ster" => BType::Ster,
            "any" => BType::Any,
            _ => {
                panic!("line {}:{}: unknown type '{}'", t.line, t.col, t.value);
            }
        }
    }

    pub fn parse(&mut self) -> Program {
        let mut prog = Program { stmts: Vec::new() };
        while self.peek().tt != TokenType::Eof {
            prog.stmts.push(self.parse_stmt());
        }
        prog
    }

    fn parse_stmt(&mut self) -> ASTNode {
        if self.at_ident("var") {
            return self.parse_var_decl();
        }
        if self.at_ident("fn") {
            return self.parse_fn_decl();
        }
        if self.at_ident("if") {
            return self.parse_if();
        }
        if self.at_ident("while") {
            return self.parse_while();
        }
        if self.at_ident("for") {
            return self.parse_for();
        }
        if self.at_ident("return") {
            return self.parse_return();
        }
        if self.at_ident("break") {
            self.advance();
            self.match_token(TokenType::Semicolon);
            return ASTNode::BreakStmt;
        }
        if self.at_ident("continue") {
            self.advance();
            self.match_token(TokenType::Semicolon);
            return ASTNode::ContinueStmt;
        }
        if self.peek().tt == TokenType::LBrace {
            return self.parse_block();
        }
        if self.at_ident("idb") {
            return self.parse_idb_decl();
        }
        if self.peek().tt == TokenType::Ident && is_type_name(&self.peek().value) {
            return self.parse_typed_decl();
        }

        let expr = self.parse_expr();

        if self.peek().tt == TokenType::Eq {
            self.advance();
            let val = self.parse_expr();
            self.match_token(TokenType::Semicolon);
            return ASTNode::Assign(Box::new(Assign {
                target: Box::new(expr),
                value: Box::new(val),
            }));
        }
        if self.peek().tt == TokenType::PlusEq {
            self.advance();
            let val = self.parse_expr();
            self.match_token(TokenType::Semicolon);
            return ASTNode::Assign(Box::new(Assign {
                target: Box::new(expr.clone()),
                value: Box::new(ASTNode::BinExpr(Box::new(BinExpr {
                    op: "+".to_string(),
                    left: Box::new(expr),
                    right: Box::new(val),
                }))),
            }));
        }
        if self.peek().tt == TokenType::MinusEq {
            self.advance();
            let val = self.parse_expr();
            self.match_token(TokenType::Semicolon);
            return ASTNode::Assign(Box::new(Assign {
                target: Box::new(expr.clone()),
                value: Box::new(ASTNode::BinExpr(Box::new(BinExpr {
                    op: "-".to_string(),
                    left: Box::new(expr),
                    right: Box::new(val),
                }))),
            }));
        }

        self.match_token(TokenType::Semicolon);
        ASTNode::ExprStmt(Box::new(ExprStmt {
            expr: Box::new(expr),
        }))
    }

    fn parse_block(&mut self) -> ASTNode {
        self.expect(TokenType::LBrace);
        let mut body = Vec::new();
        while self.peek().tt != TokenType::RBrace && self.peek().tt != TokenType::Eof {
            body.push(self.parse_stmt());
        }
        self.expect(TokenType::RBrace);
        ASTNode::BlockStmt(Box::new(BlockStmt { body }))
    }

    fn parse_if(&mut self) -> ASTNode {
        self.advance();
        let cond = self.parse_expr();
        let body = self.parse_block();
        let mut else_body = None;
        if self.at_ident("else") {
            self.advance();
            if self.at_ident("if") {
                else_body = Some(Box::new(self.parse_if()));
            } else {
                else_body = Some(Box::new(self.parse_block()));
            }
        }
        ASTNode::IfStmt(Box::new(IfStmt {
            cond: Box::new(cond),
            body: Box::new(body),
            else_body,
        }))
    }

    fn parse_while(&mut self) -> ASTNode {
        self.advance();
        let cond = self.parse_expr();
        let body = self.parse_block();
        ASTNode::WhileStmt(Box::new(WhileStmt {
            cond: Box::new(cond),
            body: Box::new(body),
        }))
    }

    fn parse_for(&mut self) -> ASTNode {
        self.advance();
        let var_name = self.expect(TokenType::Ident).value;
        self.expect(TokenType::Ident); // "in"
        let iter = self.parse_expr();
        let body = self.parse_block();
        ASTNode::ForStmt(Box::new(ForStmt {
            var: var_name,
            iter: Box::new(iter),
            body: Box::new(body),
        }))
    }

    fn parse_var_decl(&mut self) -> ASTNode {
        self.advance();
        let name = self.expect(TokenType::Ident).value;

        let mut var_type = BType::Any;
        let mut has_type = false;
        if self.match_token(TokenType::Colon) {
            var_type = self.parse_type();
            has_type = true;
        }

        self.expect(TokenType::Eq);
        let val = self.parse_expr();
        self.match_token(TokenType::Semicolon);
        ASTNode::VarDecl(Box::new(VarDecl {
            name,
            var_type,
            value: Box::new(val),
            has_type,
            is_bel: false,
            is_ster: false,
        }))
    }

    fn parse_idb_decl(&mut self) -> ASTNode {
        self.advance();
        let num = self.expect(TokenType::Number);
        let id = num.value.parse::<i64>().unwrap_or(0);
        self.expect(TokenType::Eq);
        let val = self.parse_expr();
        self.match_token(TokenType::Semicolon);
        ASTNode::IdbDecl(Box::new(IdbDecl {
            id,
            value: Box::new(val),
        }))
    }

    fn parse_typed_decl(&mut self) -> ASTNode {
        let type_name = self.advance().value;
        let name = self.expect(TokenType::Ident).value;
        self.expect(TokenType::Eq);
        let val = self.parse_expr();
        self.match_token(TokenType::Semicolon);

        let mut btype = BType::Any;
        let mut is_bel = false;
        let mut is_ster = false;
        match type_name.as_str() {
            "float" => btype = BType::Float,
            "string" => btype = BType::String,
            "bool" => btype = BType::Bool,
            "bel" => {
                btype = BType::Bel;
                is_bel = true;
            }
            "ster" => {
                btype = BType::Ster;
                is_ster = true;
            }
            "any" => btype = BType::Any,
            _ => {}
        }
        ASTNode::VarDecl(Box::new(VarDecl {
            name,
            var_type: btype,
            value: Box::new(val),
            has_type: true,
            is_bel,
            is_ster,
        }))
    }

    fn parse_fn_decl(&mut self) -> ASTNode {
        self.advance();
        let name = self.expect(TokenType::Ident).value;
        self.expect(TokenType::LParen);

        let mut params = Vec::new();
        if self.peek().tt != TokenType::RParen {
            let p_name = self.expect(TokenType::Ident).value;
            let mut p_type = BType::Any;
            if self.match_token(TokenType::Colon) {
                p_type = self.parse_type();
            }
            params.push(TypedParam {
                name: p_name,
                param_type: p_type,
            });
            while self.match_token(TokenType::Comma) {
                let p_name = self.expect(TokenType::Ident).value;
                let mut p_type = BType::Any;
                if self.match_token(TokenType::Colon) {
                    p_type = self.parse_type();
                }
                params.push(TypedParam {
                    name: p_name,
                    param_type: p_type,
                });
            }
        }
        self.expect(TokenType::RParen);

        let mut ret_type = BType::Any;
        let mut has_ret_type = false;
        if self.match_token(TokenType::Colon) {
            ret_type = self.parse_type();
            has_ret_type = true;
        }

        let body = self.parse_block();
        ASTNode::FnDecl(Box::new(FnDecl {
            name,
            params,
            return_type: ret_type,
            has_ret_type,
            body: Box::new(body),
        }))
    }

    fn parse_return(&mut self) -> ASTNode {
        self.advance();
        let mut val = None;
        if self.peek().tt != TokenType::Semicolon
            && self.peek().tt != TokenType::RBrace
            && self.peek().tt != TokenType::Eof
        {
            val = Some(Box::new(self.parse_expr()));
        }
        self.match_token(TokenType::Semicolon);
        ASTNode::ReturnStmt(Box::new(ReturnStmt { value: val }))
    }

    fn parse_expr(&mut self) -> ASTNode {
        self.parse_or()
    }

    fn parse_or(&mut self) -> ASTNode {
        let mut left = self.parse_and();
        while self.peek().tt == TokenType::Or {
            self.advance();
            let right = self.parse_and();
            left = ASTNode::BinExpr(Box::new(BinExpr {
                op: "||".to_string(),
                left: Box::new(left),
                right: Box::new(right),
            }));
        }
        left
    }

    fn parse_and(&mut self) -> ASTNode {
        let mut left = self.parse_comparison();
        while self.peek().tt == TokenType::And {
            self.advance();
            let right = self.parse_comparison();
            left = ASTNode::BinExpr(Box::new(BinExpr {
                op: "&&".to_string(),
                left: Box::new(left),
                right: Box::new(right),
            }));
        }
        left
    }

    fn parse_comparison(&mut self) -> ASTNode {
        let mut left = self.parse_add_sub();
        loop {
            let tt = self.peek().tt;
            if tt == TokenType::EqEq
                || tt == TokenType::Neq
                || tt == TokenType::Lt
                || tt == TokenType::Gt
                || tt == TokenType::Lte
                || tt == TokenType::Gte
            {
                let op = self.advance().value;
                let right = self.parse_add_sub();
                left = ASTNode::BinExpr(Box::new(BinExpr {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                }));
            } else {
                break;
            }
        }
        left
    }

    fn parse_add_sub(&mut self) -> ASTNode {
        let mut left = self.parse_mul_div();
        loop {
            let tt = self.peek().tt;
            if tt == TokenType::Plus || tt == TokenType::Minus {
                let op = self.advance().value;
                let right = self.parse_mul_div();
                left = ASTNode::BinExpr(Box::new(BinExpr {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                }));
            } else {
                break;
            }
        }
        left
    }

    fn parse_mul_div(&mut self) -> ASTNode {
        let mut left = self.parse_unary();
        loop {
            let tt = self.peek().tt;
            if tt == TokenType::Star || tt == TokenType::Slash || tt == TokenType::Percent {
                let op = self.advance().value;
                let right = self.parse_unary();
                left = ASTNode::BinExpr(Box::new(BinExpr {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                }));
            } else {
                break;
            }
        }
        left
    }

    fn parse_unary(&mut self) -> ASTNode {
        if self.peek().tt == TokenType::Minus {
            self.advance();
            return ASTNode::UnaryExpr(Box::new(UnaryExpr {
                op: "-".to_string(),
                expr: Box::new(self.parse_postfix()),
            }));
        }
        if self.peek().tt == TokenType::Not {
            self.advance();
            return ASTNode::UnaryExpr(Box::new(UnaryExpr {
                op: "!".to_string(),
                expr: Box::new(self.parse_postfix()),
            }));
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> ASTNode {
        let mut node = self.parse_primary();
        loop {
            if self.peek().tt == TokenType::LParen {
                self.advance();
                let name = match &node {
                    ASTNode::Ident(i) => i.name.clone(),
                    ASTNode::CallExpr(c) => c.name.clone(),
                    _ => {
                        panic!("line {}: not callable", self.peek().line);
                    }
                };
                let mut args = Vec::new();
                if self.peek().tt != TokenType::RParen {
                    args.push(self.parse_expr());
                    while self.match_token(TokenType::Comma) {
                        args.push(self.parse_expr());
                    }
                }
                self.expect(TokenType::RParen);
                node = ASTNode::CallExpr(Box::new(CallExpr { name, args }));
            } else if self.peek().tt == TokenType::Dot {
                self.advance();
                let right = self.expect(TokenType::Ident).value;
                node = ASTNode::DotExpr(Box::new(DotExpr {
                    left: Box::new(node),
                    right,
                }));
            } else if self.peek().tt == TokenType::LBracket {
                self.advance();
                let idx = self.parse_expr();
                self.expect(TokenType::RBracket);
                node = ASTNode::IndexExpr(Box::new(IndexExpr {
                    obj: Box::new(node),
                    index: Box::new(idx),
                }));
            } else {
                break;
            }
        }
        node
    }

    fn parse_primary(&mut self) -> ASTNode {
        let t = self.peek().clone();
        match t.tt {
            TokenType::String => {
                self.advance();
                ASTNode::StringLit(Box::new(StringLit { value: t.value }))
            }
            TokenType::Number => {
                self.advance();
                ASTNode::NumLit(Box::new(NumLit { value: t.value }))
            }
            TokenType::Ident => {
                self.advance();
                match t.value.as_str() {
                    "true" => ASTNode::BoolLit(Box::new(BoolLit { value: true })),
                    "false" => ASTNode::BoolLit(Box::new(BoolLit { value: false })),
                    "null" => ASTNode::Ident(Box::new(Ident {
                        name: "null".to_string(),
                    })),
                    _ => ASTNode::Ident(Box::new(Ident { name: t.value })),
                }
            }
            TokenType::LParen => {
                self.advance();
                let expr = self.parse_expr();
                self.expect(TokenType::RParen);
                expr
            }
            TokenType::LBracket => self.parse_array_lit(),
            _ => {
                panic!(
                    "line {}:{}: unexpected token '{}' ({})",
                    t.line,
                    t.col,
                    t.tt.name(),
                    t.value
                );
            }
        }
    }

    fn parse_array_lit(&mut self) -> ASTNode {
        self.expect(TokenType::LBracket);
        let mut elems = Vec::new();
        if self.peek().tt != TokenType::RBracket {
            elems.push(self.parse_expr());
            while self.match_token(TokenType::Comma) {
                elems.push(self.parse_expr());
            }
        }
        self.expect(TokenType::RBracket);
        ASTNode::ArrayLit(Box::new(ArrayLit { elems }))
    }
}
