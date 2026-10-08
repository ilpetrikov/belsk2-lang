use std::rc::Rc;

use crate::ast::*;
use crate::error::{Error, Result};
use crate::lexer::tokenize;
use crate::span::Span;
use crate::token::{Token, TokenKind};
use crate::types::BType;

/// Maximum height of a single expression tree.
pub const MAX_EXPR_DEPTH: u32 = 512;
/// Maximum nesting of blocks, parentheses and other recursive constructs.
pub const MAX_NESTING: usize = 256;

/// Words that cannot be used as names.
pub const KEYWORDS: &[&str] = &[
    "var", "fn", "if", "else", "while", "for", "in", "return", "break", "continue", "idb", "true",
    "false", "null",
];

/// Parses a whole program.
pub fn parse(source: &str) -> Result<Program> {
    let tokens = tokenize(source)?;
    Parser::new(tokens).parse_program()
}

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    eof: Token,
    nesting: usize,
    loop_depth: usize,
    fn_depth: usize,
}

/// Grows the stack on demand so deep (but bounded) recursion never overflows.
fn with_stack<T>(f: impl FnOnce() -> T) -> T {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, f)
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        let end = tokens.last().map(|t| t.span).unwrap_or_default();
        Parser {
            tokens,
            pos: 0,
            eof: Token::new(TokenKind::Eof, "", end),
            nesting: 0,
            loop_depth: 0,
            fn_depth: 0,
        }
    }

    // ---- token helpers -------------------------------------------------

    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&self.eof)
    }

    fn peek_at(&self, offset: usize) -> &Token {
        self.tokens.get(self.pos + offset).unwrap_or(&self.eof)
    }

    fn kind(&self) -> TokenKind {
        self.peek().kind
    }

    fn span(&self) -> Span {
        self.peek().span
    }

    fn bump(&mut self) -> Token {
        let t = self.peek().clone();
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, kind: TokenKind) -> bool {
        if self.kind() == kind {
            self.bump();
            true
        } else {
            false
        }
    }

    fn at_word(&self, word: &str) -> bool {
        let t = self.peek();
        t.kind == TokenKind::Ident && t.text == word
    }

    fn unexpected(&self, expected: &str) -> Error {
        let t = self.peek();
        let mut e = Error::syntax(
            format!("expected {expected}, found {}", t.describe()),
            t.span,
        );
        e.incomplete = t.kind == TokenKind::Eof;
        e
    }

    fn expect(&mut self, kind: TokenKind) -> Result<Token> {
        if self.kind() == kind {
            Ok(self.bump())
        } else {
            Err(self.unexpected(kind.name()))
        }
    }

    fn expect_word(&mut self, word: &str) -> Result<()> {
        if self.at_word(word) {
            self.bump();
            Ok(())
        } else {
            Err(self.unexpected(&format!("'{word}'")))
        }
    }

    /// Expects an identifier that is not a keyword.
    fn expect_name(&mut self, what: &str) -> Result<String> {
        let t = self.peek().clone();
        if t.kind != TokenKind::Ident {
            return Err(self.unexpected(what));
        }
        if KEYWORDS.contains(&t.text.as_str()) {
            return Err(Error::syntax(
                format!("'{}' is a keyword and cannot be used as {what}", t.text),
                t.span,
            ));
        }
        self.bump();
        Ok(t.text)
    }

    fn end_stmt(&mut self) {
        self.eat(TokenKind::Semicolon);
    }

    fn enter(&mut self) -> Result<()> {
        if self.nesting >= MAX_NESTING {
            return Err(Error::syntax("code is nested too deeply", self.span()));
        }
        self.nesting += 1;
        Ok(())
    }

    fn leave(&mut self) {
        self.nesting = self.nesting.saturating_sub(1);
    }

    fn mk(&self, kind: ExprKind, span: Span) -> Result<Expr> {
        let e = Expr::new(kind, span);
        if e.depth > MAX_EXPR_DEPTH {
            return Err(Error::syntax("expression is too deeply nested", span));
        }
        Ok(e)
    }

    // ---- statements ----------------------------------------------------

    pub fn parse_program(&mut self) -> Result<Program> {
        let mut stmts = Vec::new();
        while self.kind() != TokenKind::Eof {
            stmts.push(self.parse_stmt()?);
        }
        Ok(Program { stmts })
    }

    fn parse_stmt(&mut self) -> Result<Stmt> {
        self.enter()?;
        let r = with_stack(|| self.parse_stmt_inner());
        self.leave();
        r
    }

    fn parse_stmt_inner(&mut self) -> Result<Stmt> {
        let span = self.span();
        let t = self.peek();

        if t.kind == TokenKind::LBrace {
            let block = self.parse_block()?;
            return Ok(Stmt {
                kind: StmtKind::Block(block),
                span,
            });
        }

        if t.kind == TokenKind::Ident {
            match t.text.as_str() {
                "var" => return self.parse_var_decl(),
                "fn" => return self.parse_fn_decl(),
                "if" => return self.parse_if(),
                "while" => return self.parse_while(),
                "for" => return self.parse_for(),
                "return" => return self.parse_return(),
                "idb" => return self.parse_idb(),
                "break" | "continue" => {
                    let is_break = t.text == "break";
                    if self.loop_depth == 0 {
                        return Err(Error::syntax(
                            format!("'{}' outside of a loop", t.text),
                            span,
                        ));
                    }
                    self.bump();
                    self.end_stmt();
                    let kind = if is_break {
                        StmtKind::Break
                    } else {
                        StmtKind::Continue
                    };
                    return Ok(Stmt { kind, span });
                }
                _ => {}
            }
            // `int x = 5` — a type name directly followed by a name.
            if let Some(ty) = BType::from_name(&t.text) {
                if ty != BType::Fn && self.peek_at(1).kind == TokenKind::Ident {
                    self.bump();
                    return self.parse_typed_decl(ty, span);
                }
            }
        }

        let expr = self.parse_expr()?;
        let assign_op = match self.kind() {
            TokenKind::Eq => Some(None),
            TokenKind::PlusEq => Some(Some(BinOp::Add)),
            TokenKind::MinusEq => Some(Some(BinOp::Sub)),
            _ => None,
        };
        let Some(op) = assign_op else {
            self.end_stmt();
            return Ok(Stmt {
                kind: StmtKind::Expr(expr),
                span,
            });
        };
        if !expr.is_place() {
            return Err(Error::syntax("invalid assignment target", expr.span));
        }
        self.bump();
        let value = self.parse_expr()?;
        self.end_stmt();
        let kind = match op {
            None => StmtKind::Assign {
                target: expr,
                value,
            },
            Some(op) => StmtKind::CompoundAssign {
                op,
                target: expr,
                value,
            },
        };
        Ok(Stmt { kind, span })
    }

    fn parse_block(&mut self) -> Result<Block> {
        let span = self.expect(TokenKind::LBrace)?.span;
        let mut stmts = Vec::new();
        while self.kind() != TokenKind::RBrace {
            if self.kind() == TokenKind::Eof {
                return Err(self.unexpected("'}'"));
            }
            stmts.push(self.parse_stmt()?);
        }
        self.bump();
        Ok(Block { stmts, span })
    }

    fn parse_type(&mut self) -> Result<BType> {
        let t = self.peek().clone();
        if t.kind != TokenKind::Ident {
            return Err(self.unexpected("a type"));
        }
        let ty = BType::from_name(&t.text)
            .ok_or_else(|| Error::syntax(format!("unknown type '{}'", t.text), t.span))?;
        self.bump();
        Ok(ty)
    }

    fn parse_var_decl(&mut self) -> Result<Stmt> {
        let span = self.bump().span;
        let name = self.expect_name("a variable name")?;
        let ty = if self.eat(TokenKind::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(TokenKind::Eq)?;
        let value = self.parse_expr()?;
        self.end_stmt();
        Ok(Stmt {
            kind: StmtKind::VarDecl(VarDecl { name, ty, value }),
            span,
        })
    }

    fn parse_typed_decl(&mut self, ty: BType, span: Span) -> Result<Stmt> {
        let name = self.expect_name("a variable name")?;
        self.expect(TokenKind::Eq)?;
        let value = self.parse_expr()?;
        self.end_stmt();
        Ok(Stmt {
            kind: StmtKind::VarDecl(VarDecl {
                name,
                ty: Some(ty),
                value,
            }),
            span,
        })
    }

    fn parse_idb(&mut self) -> Result<Stmt> {
        let span = self.bump().span;
        let num = self.expect(TokenKind::Number)?;
        let id = num.text.parse::<i64>().map_err(|_| {
            Error::syntax(
                format!("idb slot must be a whole number, found {}", num.text),
                num.span,
            )
        })?;
        self.expect(TokenKind::Eq)?;
        let value = self.parse_expr()?;
        self.end_stmt();
        Ok(Stmt {
            kind: StmtKind::Idb { id, value },
            span,
        })
    }

    fn parse_fn_decl(&mut self) -> Result<Stmt> {
        let span = self.bump().span;
        let name = self.expect_name("a function name")?;
        self.expect(TokenKind::LParen)?;
        let mut params: Vec<Param> = Vec::new();
        if self.kind() != TokenKind::RParen {
            loop {
                let pspan = self.span();
                let pname = self.expect_name("a parameter name")?;
                if params.iter().any(|p| p.name == pname) {
                    return Err(Error::syntax(
                        format!("duplicate parameter '{pname}'"),
                        pspan,
                    ));
                }
                let ty = if self.eat(TokenKind::Colon) {
                    Some(self.parse_type()?)
                } else {
                    None
                };
                params.push(Param {
                    name: pname,
                    ty,
                    span: pspan,
                });
                if !self.eat(TokenKind::Comma) {
                    break;
                }
            }
        }
        self.expect(TokenKind::RParen)?;
        let ret = if self.eat(TokenKind::Colon) || self.eat(TokenKind::Arrow) {
            Some(self.parse_type()?)
        } else {
            None
        };

        let saved_loops = std::mem::replace(&mut self.loop_depth, 0);
        self.fn_depth += 1;
        let body = self.parse_block();
        self.fn_depth -= 1;
        self.loop_depth = saved_loops;
        let body = body?;

        Ok(Stmt {
            kind: StmtKind::FnDecl(Rc::new(FnDecl {
                name,
                params,
                ret,
                body,
                span,
            })),
            span,
        })
    }

    fn parse_if(&mut self) -> Result<Stmt> {
        let span = self.bump().span;
        let cond = self.parse_expr()?;
        let then = self.parse_block()?;
        let otherwise = if self.at_word("else") {
            self.bump();
            let else_span = self.span();
            let stmt = if self.at_word("if") {
                self.enter()?;
                let r = with_stack(|| self.parse_if());
                self.leave();
                r?
            } else {
                Stmt {
                    kind: StmtKind::Block(self.parse_block()?),
                    span: else_span,
                }
            };
            Some(Box::new(stmt))
        } else {
            None
        };
        Ok(Stmt {
            kind: StmtKind::If {
                cond,
                then,
                otherwise,
            },
            span,
        })
    }

    fn parse_loop_body(&mut self) -> Result<Block> {
        self.loop_depth += 1;
        let body = self.parse_block();
        self.loop_depth -= 1;
        body
    }

    fn parse_while(&mut self) -> Result<Stmt> {
        let span = self.bump().span;
        let cond = self.parse_expr()?;
        let body = self.parse_loop_body()?;
        Ok(Stmt {
            kind: StmtKind::While { cond, body },
            span,
        })
    }

    fn parse_for(&mut self) -> Result<Stmt> {
        let span = self.bump().span;
        let var = self.expect_name("a loop variable")?;
        self.expect_word("in")?;
        let iter = self.parse_expr()?;
        let body = self.parse_loop_body()?;
        Ok(Stmt {
            kind: StmtKind::For { var, iter, body },
            span,
        })
    }

    fn parse_return(&mut self) -> Result<Stmt> {
        let span = self.bump().span;
        if self.fn_depth == 0 {
            return Err(Error::syntax("'return' outside of a function", span));
        }
        let value = match self.kind() {
            TokenKind::Semicolon | TokenKind::RBrace | TokenKind::Eof => None,
            _ => Some(self.parse_expr()?),
        };
        self.end_stmt();
        Ok(Stmt {
            kind: StmtKind::Return(value),
            span,
        })
    }

    // ---- expressions ---------------------------------------------------

    pub fn parse_expr(&mut self) -> Result<Expr> {
        self.enter()?;
        let r = with_stack(|| self.parse_or());
        self.leave();
        r
    }

    fn binary_level(
        &mut self,
        ops: &[(TokenKind, BinOp)],
        next: fn(&mut Self) -> Result<Expr>,
    ) -> Result<Expr> {
        let mut lhs = next(self)?;
        while let Some(&(_, op)) = ops.iter().find(|(k, _)| *k == self.kind()) {
            let span = self.bump().span;
            let rhs = next(self)?;
            lhs = self.mk(
                ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            )?;
        }
        Ok(lhs)
    }

    fn parse_or(&mut self) -> Result<Expr> {
        self.binary_level(&[(TokenKind::Or, BinOp::Or)], Self::parse_and)
    }

    fn parse_and(&mut self) -> Result<Expr> {
        self.binary_level(&[(TokenKind::And, BinOp::And)], Self::parse_equality)
    }

    fn parse_equality(&mut self) -> Result<Expr> {
        self.binary_level(
            &[(TokenKind::EqEq, BinOp::Eq), (TokenKind::Neq, BinOp::Ne)],
            Self::parse_relational,
        )
    }

    fn parse_relational(&mut self) -> Result<Expr> {
        self.binary_level(
            &[
                (TokenKind::Lt, BinOp::Lt),
                (TokenKind::Gt, BinOp::Gt),
                (TokenKind::Lte, BinOp::Le),
                (TokenKind::Gte, BinOp::Ge),
            ],
            Self::parse_additive,
        )
    }

    fn parse_additive(&mut self) -> Result<Expr> {
        self.binary_level(
            &[
                (TokenKind::Plus, BinOp::Add),
                (TokenKind::Minus, BinOp::Sub),
            ],
            Self::parse_multiplicative,
        )
    }

    fn parse_multiplicative(&mut self) -> Result<Expr> {
        self.binary_level(
            &[
                (TokenKind::Star, BinOp::Mul),
                (TokenKind::Slash, BinOp::Div),
                (TokenKind::Percent, BinOp::Rem),
            ],
            Self::parse_unary,
        )
    }

    fn parse_unary(&mut self) -> Result<Expr> {
        let op = match self.kind() {
            TokenKind::Minus => UnaryOp::Neg,
            TokenKind::Not => UnaryOp::Not,
            _ => return self.parse_postfix(),
        };
        let span = self.bump().span;
        self.enter()?;
        let inner = with_stack(|| self.parse_unary());
        self.leave();
        let expr = inner?;
        self.mk(
            ExprKind::Unary {
                op,
                expr: Box::new(expr),
            },
            span,
        )
    }

    fn parse_postfix(&mut self) -> Result<Expr> {
        let mut node = self.parse_primary()?;
        loop {
            let span = self.span();
            match self.kind() {
                TokenKind::LParen => {
                    self.bump();
                    let args = self.parse_list(TokenKind::RParen)?;
                    // Errors about a call point at the callee, not at '('.
                    let call_span = node.span;
                    node = self.mk(
                        ExprKind::Call {
                            callee: Box::new(node),
                            args,
                        },
                        call_span,
                    )?;
                }
                TokenKind::Dot => {
                    self.bump();
                    let name = self.expect(TokenKind::Ident)?.text;
                    node = self.mk(
                        ExprKind::Member {
                            object: Box::new(node),
                            name,
                        },
                        span,
                    )?;
                }
                TokenKind::LBracket => {
                    self.bump();
                    let index = self.parse_expr()?;
                    self.expect(TokenKind::RBracket)?;
                    node = self.mk(
                        ExprKind::Index {
                            object: Box::new(node),
                            index: Box::new(index),
                        },
                        span,
                    )?;
                }
                _ => return Ok(node),
            }
        }
    }

    /// Parses comma-separated expressions up to and including `close`.
    /// A trailing comma is allowed.
    fn parse_list(&mut self, close: TokenKind) -> Result<Vec<Expr>> {
        let mut items = Vec::new();
        while self.kind() != close {
            items.push(self.parse_expr()?);
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }
        self.expect(close)?;
        Ok(items)
    }

    fn parse_primary(&mut self) -> Result<Expr> {
        let t = self.peek().clone();
        let span = t.span;
        match t.kind {
            TokenKind::String => {
                self.bump();
                self.mk(ExprKind::String(t.text), span)
            }
            TokenKind::Number => {
                self.bump();
                let n = t.text.parse::<f64>().map_err(|_| {
                    Error::syntax(format!("invalid number literal '{}'", t.text), span)
                })?;
                self.mk(ExprKind::Number(n), span)
            }
            TokenKind::Ident => {
                let kind = match t.text.as_str() {
                    "true" => ExprKind::Bool(true),
                    "false" => ExprKind::Bool(false),
                    "null" => ExprKind::Null,
                    word if KEYWORDS.contains(&word) => {
                        return Err(self.unexpected("an expression"))
                    }
                    _ => ExprKind::Ident(t.text),
                };
                self.bump();
                self.mk(kind, span)
            }
            TokenKind::LParen => {
                self.bump();
                let e = self.parse_expr()?;
                self.expect(TokenKind::RParen)?;
                Ok(e)
            }
            TokenKind::LBracket => {
                self.bump();
                let items = self.parse_list(TokenKind::RBracket)?;
                self.mk(ExprKind::Array(items), span)
            }
            _ => Err(self.unexpected("an expression")),
        }
    }
}
