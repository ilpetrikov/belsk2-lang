use std::rc::Rc;

use crate::ast::*;
use crate::error::{Error, Result};
use crate::lexer::tokenize;
use crate::span::Span;
use crate::token::{Token, TokenKind};
use crate::types::{FloatKind, IntKind, Ty, TypeExpr, TypeExprKind};

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
            // `int x = 5`, `string[] names = ...`
            if let Some(ty) = self.try_decl_type() {
                return self.parse_typed_decl(ty, span);
            }
        }

        let expr = self.parse_expr()?;
        let (assign_op, op_tokens) = match self.kind() {
            TokenKind::Eq => (Some(None), 1),
            TokenKind::PlusEq => (Some(Some(BinOp::Add)), 1),
            TokenKind::MinusEq => (Some(Some(BinOp::Sub)), 1),
            TokenKind::StarEq => (Some(Some(BinOp::Mul)), 1),
            TokenKind::SlashEq => (Some(Some(BinOp::Div)), 1),
            TokenKind::PercentEq => (Some(Some(BinOp::Rem)), 1),
            TokenKind::AmpEq => (Some(Some(BinOp::BitAnd)), 1),
            TokenKind::PipeEq => (Some(Some(BinOp::BitOr)), 1),
            TokenKind::CaretEq => (Some(Some(BinOp::BitXor)), 1),
            TokenKind::ShlEq => (Some(Some(BinOp::Shl)), 1),
            TokenKind::Gt if self.adjacent(TokenKind::Gte) => (Some(Some(BinOp::Shr)), 2),
            _ => (None, 0),
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
        for _ in 0..op_tokens {
            self.bump();
        }
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
                cast: Ty::Any,
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

    /// Whether the next token is `kind` and directly touches the current
    /// one (no space between), as the second `>` of `>>`.
    fn adjacent(&self, kind: TokenKind) -> bool {
        let (a, b) = (self.peek(), self.peek_at(1));
        b.kind == kind && a.span.line == b.span.line && a.span.col + 1 == b.span.col
    }

    /// Parses a type: `int`, `string[]`, `List<int>`, `Dictionary<string, int[]>`.
    fn parse_type(&mut self) -> Result<TypeExpr> {
        self.enter()?;
        let r = with_stack(|| self.parse_type_inner());
        self.leave();
        r
    }

    fn parse_type_inner(&mut self) -> Result<TypeExpr> {
        let t = self.peek().clone();
        if t.kind != TokenKind::Ident || (KEYWORDS.contains(&t.text.as_str()) && t.text != "fn") {
            return Err(self.unexpected("a type"));
        }
        self.bump();
        let mut args = Vec::new();
        if self.kind() == TokenKind::Lt {
            self.bump();
            loop {
                args.push(self.parse_type()?);
                if !self.eat(TokenKind::Comma) {
                    break;
                }
            }
            self.expect(TokenKind::Gt)?;
        }
        let mut ty = TypeExpr {
            kind: TypeExprKind::Named { name: t.text, args },
            span: t.span,
        };
        while self.kind() == TokenKind::LBracket && self.peek_at(1).kind == TokenKind::RBracket {
            self.bump();
            self.bump();
            ty = TypeExpr {
                kind: TypeExprKind::Array(Box::new(ty)),
                span: t.span,
            };
        }
        Ok(ty)
    }

    /// At `<` after an expression: parses `<T, U>` if it is directly
    /// followed by `(`, leaving the position at `(`. Otherwise restores the
    /// position and returns `None`.
    fn try_type_args(&mut self) -> Option<Vec<TypeExpr>> {
        let (pos, nesting) = (self.pos, self.nesting);
        self.bump();
        let mut args = Vec::new();
        let ok = loop {
            match self.parse_type() {
                Ok(t) => args.push(t),
                Err(_) => break false,
            }
            if !self.eat(TokenKind::Comma) {
                break self.eat(TokenKind::Gt) && self.kind() == TokenKind::LParen;
            }
        };
        if ok {
            return Some(args);
        }
        self.pos = pos;
        self.nesting = nesting;
        None
    }

    /// At the start of a statement: if it is a declaration like
    /// `int x = ...` or `List<int> xs = ...`, consumes and returns the type.
    /// Otherwise leaves the position unchanged.
    fn try_decl_type(&mut self) -> Option<TypeExpr> {
        let (pos, nesting) = (self.pos, self.nesting);
        if let Ok(ty) = self.parse_type() {
            if self.kind() == TokenKind::Ident && self.peek_at(1).kind == TokenKind::Eq {
                return Some(ty);
            }
        }
        self.pos = pos;
        self.nesting = nesting;
        None
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
            kind: StmtKind::VarDecl(VarDecl {
                name,
                ty,
                resolved: Ty::Any,
                value,
            }),
            span,
        })
    }

    fn parse_typed_decl(&mut self, ty: TypeExpr, span: Span) -> Result<Stmt> {
        let name = self.expect_name("a variable name")?;
        self.expect(TokenKind::Eq)?;
        let value = self.parse_expr()?;
        self.end_stmt();
        Ok(Stmt {
            kind: StmtKind::VarDecl(VarDecl {
                name,
                ty: Some(ty),
                resolved: Ty::Any,
                value,
            }),
            span,
        })
    }

    fn parse_idb(&mut self) -> Result<Stmt> {
        let span = self.bump().span;
        let num = self.expect(TokenKind::Number)?;
        let id = num.text.replace('_', "").parse::<i64>().map_err(|_| {
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
        let mut type_params: Vec<String> = Vec::new();
        if self.eat(TokenKind::Lt) {
            loop {
                let tspan = self.span();
                let t = self.expect_name("a type parameter")?;
                if type_params.contains(&t) {
                    return Err(Error::syntax(
                        format!("duplicate type parameter '{t}'"),
                        tspan,
                    ));
                }
                type_params.push(t);
                if !self.eat(TokenKind::Comma) {
                    break;
                }
            }
            self.expect(TokenKind::Gt)?;
        }
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
                    resolved: Ty::Any,
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
                type_params,
                params,
                ret,
                ret_resolved: Ty::Any,
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
            kind: StmtKind::For {
                var,
                var_ty: Ty::Any,
                iter,
                body,
            },
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
        self.binary_level(&[(TokenKind::And, BinOp::And)], Self::parse_bit_or)
    }

    fn parse_bit_or(&mut self) -> Result<Expr> {
        self.binary_level(&[(TokenKind::Pipe, BinOp::BitOr)], Self::parse_bit_xor)
    }

    fn parse_bit_xor(&mut self) -> Result<Expr> {
        self.binary_level(&[(TokenKind::Caret, BinOp::BitXor)], Self::parse_bit_and)
    }

    fn parse_bit_and(&mut self) -> Result<Expr> {
        self.binary_level(&[(TokenKind::Amp, BinOp::BitAnd)], Self::parse_equality)
    }

    fn parse_equality(&mut self) -> Result<Expr> {
        self.binary_level(
            &[(TokenKind::EqEq, BinOp::Eq), (TokenKind::Neq, BinOp::Ne)],
            Self::parse_relational,
        )
    }

    fn parse_relational(&mut self) -> Result<Expr> {
        let mut lhs = self.parse_shift()?;
        loop {
            let op = match self.kind() {
                TokenKind::Lt => BinOp::Lt,
                TokenKind::Lte => BinOp::Le,
                TokenKind::Gte => BinOp::Ge,
                // `>` directly followed by `>=` is the `>>=` operator.
                TokenKind::Gt if !self.adjacent(TokenKind::Gte) => BinOp::Gt,
                _ => return Ok(lhs),
            };
            let span = self.bump().span;
            let rhs = self.parse_shift()?;
            lhs = self.mk(
                ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            )?;
        }
    }

    /// `<<` and `>>`. `>>` is two adjacent `>` tokens (see [`TokenKind::Shl`]).
    fn parse_shift(&mut self) -> Result<Expr> {
        let mut lhs = self.parse_additive()?;
        loop {
            let op = match self.kind() {
                TokenKind::Shl => BinOp::Shl,
                TokenKind::Gt if self.adjacent(TokenKind::Gt) => BinOp::Shr,
                _ => return Ok(lhs),
            };
            let span = self.bump().span;
            if op == BinOp::Shr {
                self.bump();
            }
            let rhs = self.parse_additive()?;
            lhs = self.mk(
                ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            )?;
        }
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
            TokenKind::Tilde => UnaryOp::BitNot,
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
                TokenKind::LParen | TokenKind::Lt => {
                    // `f<int>(x)`: type arguments, but only if what follows
                    // `<` parses as types closed by `>` and then `(` (the
                    // same rule as C#). Otherwise `<` is a comparison.
                    let type_args = if self.kind() == TokenKind::Lt {
                        match self.try_type_args() {
                            Some(t) => t,
                            None => return Ok(node),
                        }
                    } else {
                        Vec::new()
                    };
                    self.bump();
                    let args = self.parse_list(TokenKind::RParen)?;
                    // Errors about a call point at the callee, not at '('.
                    let call_span = node.span;
                    node = self.mk(
                        ExprKind::Call {
                            callee: Box::new(node),
                            type_args,
                            args,
                            inst: Vec::new(),
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
                let kind = parse_number(&t.text)
                    .map_err(|msg| Error::syntax(format!("{msg}: '{}'", t.text), span))?;
                self.mk(kind, span)
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
            TokenKind::LBrace => {
                self.bump();
                let mut entries = Vec::new();
                while self.kind() != TokenKind::RBrace {
                    let key = self.parse_expr()?;
                    self.expect(TokenKind::Colon)?;
                    let value = self.parse_expr()?;
                    entries.push((key, value));
                    if !self.eat(TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(TokenKind::RBrace)?;
                self.mk(ExprKind::Map(entries), span)
            }
            _ => Err(self.unexpected("an expression")),
        }
    }
}

/// Interprets a number literal as C# does: an integer without suffix is the
/// first of `int`, `uint`, `long`, `ulong` that can hold it; `u`, `l`, `ul`
/// select unsigned/long types; a fraction or exponent makes a `double`, and
/// `f`/`d` select `float`/`double`.
fn parse_number(text: &str) -> std::result::Result<ExprKind, &'static str> {
    let clean: String = text.chars().filter(|&c| c != '_').collect();
    if text.ends_with('_') || text.contains("_.") || text.contains("._") {
        return Err("misplaced '_' in number");
    }
    let lower = clean.to_ascii_lowercase();
    let (radix, body) = if let Some(rest) = lower.strip_prefix("0x") {
        (16, rest)
    } else if let Some(rest) = lower.strip_prefix("0b") {
        (2, rest)
    } else {
        (10, lower.as_str())
    };
    let is_digit =
        |c: char| c.is_digit(radix) || (radix == 10 && matches!(c, '.' | 'e' | '+' | '-'));
    let split = body.find(|c: char| !is_digit(c)).unwrap_or(body.len());
    let (digits, suffix) = body.split_at(split);
    if digits.is_empty() {
        return Err("invalid number");
    }

    let is_float = radix == 10 && (digits.contains('.') || digits.contains('e'));
    let float_kind = match suffix {
        "f" if radix == 10 => Some(FloatKind::F32),
        "d" if radix == 10 => Some(FloatKind::F64),
        "" if is_float => Some(FloatKind::F64),
        _ if is_float => return Err("invalid suffix for a floating-point number"),
        _ => None,
    };
    if let Some(kind) = float_kind {
        let v: f64 = digits.parse().map_err(|_| "invalid number")?;
        return Ok(ExprKind::Float(kind.round(v), kind));
    }

    let v = u128::from_str_radix(digits, radix).map_err(|_| "integer literal is too large")?;
    let v = i128::try_from(v).map_err(|_| "integer literal is too large")?;
    let candidates: &[IntKind] = match suffix {
        "" => &[IntKind::I32, IntKind::U32, IntKind::I64, IntKind::U64],
        "u" => &[IntKind::U32, IntKind::U64],
        "l" => &[IntKind::I64, IntKind::U64],
        "ul" | "lu" => &[IntKind::U64],
        _ => return Err("invalid number suffix"),
    };
    candidates
        .iter()
        .find(|k| k.fits(v))
        .map(|&k| ExprKind::Int(v, k))
        .ok_or("integer literal is too large")
}
