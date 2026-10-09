//! Builds the symbol index: walks every checked module, links each use of
//! a name to its declaration and records scopes and type hints.

use std::collections::HashMap;

use belsk2::project::Loaded;
use belsk2::syntax::ast::{Block, Expr, ExprKind, FnDecl, Stmt, StmtKind};
use belsk2::syntax::token::TokenKind;
use belsk2::syntax::{FileId, Span};
use belsk2::Ty;

use crate::{
    builtins, Analysis, FileInfo, FnInfo, HintKind, InlayHint, Location, Pos, Range, Ref, Scope,
    Symbol, SymbolKind,
};

pub(crate) fn index(analysis: &mut Analysis, loaded: &Loaded) {
    let mut builtin_ids = HashMap::new();
    for name in belsk2::typeck::BUILTINS {
        let (signature, doc) =
            builtins::doc(name).unwrap_or_else(|| (format!("fn {name}(...)"), String::new()));
        builtin_ids.insert((*name).to_string(), analysis.symbols.len());
        analysis.symbols.push(Symbol {
            name: (*name).to_string(),
            kind: SymbolKind::Builtin,
            ty: Ty::Fn(None),
            signature,
            doc: Some(doc).filter(|d| !d.is_empty()),
            def: None,
            module: None,
            top_level: false,
            params: Vec::new(),
        });
    }

    let mut exports: Vec<Option<HashMap<String, usize>>> = vec![None; loaded.modules.len()];
    for &id in &loaded.order {
        let Some(module) = loaded.modules.get(id) else {
            continue;
        };
        let Some(file) = analysis.files.iter().position(|f| f.id == module.file) else {
            continue;
        };
        let info = analysis.files.get(file).cloned();
        let Some(info) = info else {
            continue;
        };
        let mut imports = HashMap::new();
        for &(used, _) in &module.uses {
            if let Some(Some(items)) = exports.get(used) {
                imports.extend(items.iter().map(|(k, v)| (k.clone(), *v)));
            }
        }
        let mut ix = Indexer {
            analysis,
            info: &info,
            module: module.name.clone(),
            builtins: &builtin_ids,
            frames: vec![imports.clone()],
            scope_stack: Vec::new(),
            fn_stack: Vec::new(),
        };
        let top = ix.open_scope(Pos::new(0, 0), Pos::MAX);
        for &symbol in imports.values() {
            ix.add_entry(top, symbol, Pos::new(0, 0));
        }
        ix.frames.push(HashMap::new());
        ix.stmts(&module.program.stmts);
        let own = ix.frames.pop().unwrap_or_default();
        ix.scope_stack.pop();
        if let Some(slot) = exports.get_mut(id) {
            *slot = Some(own);
        }
    }
}

struct Indexer<'a> {
    analysis: &'a mut Analysis,
    info: &'a FileInfo,
    module: String,
    builtins: &'a HashMap<String, usize>,
    /// Names in scope, innermost last.
    frames: Vec<HashMap<String, usize>>,
    /// Completion scopes being filled, innermost last.
    scope_stack: Vec<usize>,
    /// Functions being walked (indices into `Analysis::functions`).
    fn_stack: Vec<usize>,
}

fn is_literal(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Int(..)
        | ExprKind::Float(..)
        | ExprKind::String(_)
        | ExprKind::Bool(_)
        | ExprKind::Null => true,
        ExprKind::Convert { expr, .. } => is_literal(expr),
        _ => false,
    }
}

fn fn_signature(decl: &FnDecl) -> String {
    let mut s = format!("fn {}", decl.name);
    if !decl.type_params.is_empty() {
        s.push_str(&format!("<{}>", decl.type_params.join(", ")));
    }
    let params: Vec<String> = decl
        .params
        .iter()
        .map(|p| match p.ty {
            Some(_) => format!("{}: {}", p.name, p.resolved),
            None => p.name.clone(),
        })
        .collect();
    s.push_str(&format!("({})", params.join(", ")));
    if decl.ret.is_some() {
        s.push_str(&format!(": {}", decl.ret_resolved));
    }
    s
}

impl Indexer<'_> {
    fn file(&self) -> FileId {
        self.info.id
    }

    fn open_scope(&mut self, start: Pos, end: Pos) -> usize {
        let id = self.analysis.scopes.len();
        self.analysis.scopes.push(Scope {
            file: self.file(),
            start,
            end,
            entries: Vec::new(),
        });
        self.scope_stack.push(id);
        id
    }

    fn add_entry(&mut self, scope: usize, symbol: usize, from: Pos) {
        if let Some(s) = self.analysis.scopes.get_mut(scope) {
            s.entries.push((symbol, from));
        }
    }

    /// The range of the first `name` token at or after `span` (the name in
    /// `var x = ...`, `fn f(...)`, `for x in ...`).
    fn name_range(&self, span: Span, name: &str) -> Range {
        let at = Pos::of(span);
        let start = self
            .info
            .tokens
            .iter()
            .skip_while(|t| Pos::of(t.span) < at)
            .take(12)
            .find(|t| t.kind == TokenKind::Ident && t.text == name)
            .map(|t| Pos::of(t.span))
            .unwrap_or(at);
        Range {
            start,
            end: start.right(name.chars().count()),
        }
    }

    fn declare(&mut self, sym: Symbol) -> usize {
        let id = self.analysis.symbols.len();
        let name = sym.name.clone();
        let from = sym.def.map(|d| d.range.end).unwrap_or_default();
        let top = sym.top_level;
        let local = matches!(sym.kind, SymbolKind::Variable | SymbolKind::Parameter);
        self.analysis.symbols.push(sym);
        if let Some(f) = self
            .fn_stack
            .last()
            .and_then(|&f| self.analysis.functions.get_mut(f))
        {
            if local {
                f.locals.push(id);
            }
        }
        if let Some(frame) = self.frames.last_mut() {
            frame.insert(name, id);
        }
        if let Some(&scope) = self.scope_stack.last() {
            self.add_entry(scope, id, if top { Pos::new(0, 0) } else { from });
        }
        id
    }

    fn lookup(&self, name: &str) -> Option<usize> {
        self.frames
            .iter()
            .rev()
            .find_map(|f| f.get(name).copied())
            .or_else(|| self.builtins.get(name).copied())
    }

    fn symbol(
        &self,
        name: &str,
        kind: SymbolKind,
        ty: Ty,
        signature: String,
        def: Range,
    ) -> Symbol {
        let top_level = self.frames.len() == 2;
        Symbol {
            name: name.to_string(),
            kind,
            ty,
            signature,
            doc: None,
            def: Some(Location {
                file: self.file(),
                range: def,
            }),
            module: top_level.then(|| self.module.clone()),
            top_level,
            params: Vec::new(),
        }
    }

    fn stmts(&mut self, stmts: &[Stmt]) {
        // Functions can be used before they are declared.
        for stmt in stmts {
            if let StmtKind::FnDecl(decl) = &stmt.kind {
                let range = self.name_range(stmt.span, &decl.name);
                let mut sym = self.symbol(
                    &decl.name,
                    SymbolKind::Function,
                    belsk2::typeck::fn_type(decl),
                    fn_signature(decl),
                    range,
                );
                sym.params = decl.params.iter().map(|p| p.name.clone()).collect();
                self.declare(sym);
                if let Some(&scope) = self.scope_stack.last() {
                    // Visible in the whole block, not just after it.
                    if let Some(s) = self.analysis.scopes.get_mut(scope) {
                        if let Some(e) = s.entries.last_mut() {
                            e.1 = s.start;
                        }
                    }
                }
            }
        }
        for stmt in stmts {
            stacker::maybe_grow(64 * 1024, 1024 * 1024, || self.stmt(stmt));
        }
    }

    fn block(&mut self, block: &Block, before: impl FnOnce(&mut Self)) {
        let start = Pos::of(block.span);
        let end = self.info.braces.get(&start).copied().unwrap_or(Pos::MAX);
        self.open_scope(start, end);
        self.frames.push(HashMap::new());
        before(self);
        self.stmts(&block.stmts);
        self.frames.pop();
        self.scope_stack.pop();
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Using(_) | StmtKind::Break | StmtKind::Continue => {}
            StmtKind::Expr(e) => self.expr(e),
            StmtKind::VarDecl(decl) => {
                self.expr(&decl.value);
                let range = self.name_range(stmt.span, &decl.name);
                let sym = self.symbol(
                    &decl.name,
                    SymbolKind::Variable,
                    decl.resolved.clone(),
                    format!("var {}: {}", decl.name, decl.resolved),
                    range,
                );
                self.declare(sym);
                if decl.ty.is_none() && !is_literal(&decl.value) {
                    self.hint(range.end, format!(": {}", decl.resolved), HintKind::Type);
                }
            }
            StmtKind::Idb { value, .. } => self.expr(value),
            StmtKind::Assign { target, value } | StmtKind::CompoundAssign { target, value, .. } => {
                self.expr(target);
                self.expr(value);
            }
            StmtKind::FnDecl(decl) => {
                if let Some(symbol) = self.lookup(&decl.name) {
                    self.fn_stack.push(self.analysis.functions.len());
                    self.analysis.functions.push(FnInfo {
                        symbol,
                        file: self.file(),
                        locals: Vec::new(),
                        uses: Vec::new(),
                    });
                }
                let pushed = self.fn_stack.len();
                self.block(&decl.body, |ix| {
                    for p in &decl.params {
                        let range = ix.name_range(p.span, &p.name);
                        let sym = ix.symbol(
                            &p.name,
                            SymbolKind::Parameter,
                            p.resolved.clone(),
                            format!("(parameter) {}: {}", p.name, p.resolved),
                            range,
                        );
                        ix.declare(sym);
                    }
                    // Parameters are visible in the whole body.
                    if let Some(&scope) = ix.scope_stack.last() {
                        if let Some(s) = ix.analysis.scopes.get_mut(scope) {
                            let start = s.start;
                            for e in &mut s.entries {
                                e.1 = start;
                            }
                        }
                    }
                });
                if pushed > 0 && self.fn_stack.len() == pushed {
                    self.fn_stack.pop();
                }
            }
            StmtKind::Return(e) => {
                if let Some(e) = e {
                    self.expr(e);
                }
            }
            StmtKind::If {
                cond,
                then,
                otherwise,
            } => {
                self.expr(cond);
                self.block(then, |_| {});
                if let Some(other) = otherwise {
                    self.stmt(other);
                }
            }
            StmtKind::While { cond, body } => {
                self.expr(cond);
                self.block(body, |_| {});
            }
            StmtKind::For {
                var,
                var_ty,
                iter,
                body,
            } => {
                self.expr(iter);
                let range = self.name_range(stmt.span, var);
                self.block(body, |ix| {
                    let sym = ix.symbol(
                        var,
                        SymbolKind::Variable,
                        var_ty.clone(),
                        format!("var {var}: {var_ty}"),
                        range,
                    );
                    ix.declare(sym);
                    if let Some(&scope) = ix.scope_stack.last() {
                        if let Some(s) = ix.analysis.scopes.get_mut(scope) {
                            let start = s.start;
                            for e in &mut s.entries {
                                e.1 = start;
                            }
                        }
                    }
                });
                self.hint(range.end, format!(": {var_ty}"), HintKind::Type);
            }
            StmtKind::Block(b) => self.block(b, |_| {}),
        }
    }

    fn hint(&mut self, pos: Pos, label: String, kind: HintKind) {
        self.analysis.hints.push(InlayHint {
            file: self.file(),
            pos,
            label,
            kind,
        });
    }

    fn expr(&mut self, e: &Expr) {
        stacker::maybe_grow(64 * 1024, 1024 * 1024, || self.expr_inner(e));
    }

    fn expr_inner(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Int(..)
            | ExprKind::Float(..)
            | ExprKind::String(_)
            | ExprKind::Bool(_)
            | ExprKind::Null => {}
            ExprKind::Ident(name) => {
                if let Some(symbol) = self.lookup(name) {
                    if let Some(f) = self
                        .fn_stack
                        .last()
                        .and_then(|&f| self.analysis.functions.get_mut(f))
                    {
                        if !f.uses.contains(&symbol) {
                            f.uses.push(symbol);
                        }
                    }
                    let start = Pos::of(e.span);
                    self.analysis.refs.push(Ref {
                        loc: Location {
                            file: self.file(),
                            range: Range {
                                start,
                                end: start.right(name.chars().count()),
                            },
                        },
                        symbol,
                    });
                }
            }
            ExprKind::Array(items) => items.iter().for_each(|i| self.expr(i)),
            ExprKind::Map(entries) => {
                for (k, v) in entries {
                    self.expr(k);
                    self.expr(v);
                }
            }
            ExprKind::Unary { expr, .. } | ExprKind::Convert { expr, .. } => self.expr(expr),
            ExprKind::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            ExprKind::Call { callee, args, .. } => {
                self.expr(callee);
                let params = match &callee.kind {
                    ExprKind::Ident(name) => self
                        .lookup(name)
                        .and_then(|id| self.analysis.symbols.get(id))
                        .map(|s| s.params.clone())
                        .unwrap_or_default(),
                    _ => Vec::new(),
                };
                for (i, arg) in args.iter().enumerate() {
                    self.expr(arg);
                    if let Some(name) = params.get(i).filter(|_| is_literal(arg)) {
                        self.hint(Pos::of(arg.span), format!("{name}:"), HintKind::Parameter);
                    }
                }
            }
            ExprKind::Member { object, .. } => self.expr(object),
            ExprKind::Index { object, index } => {
                self.expr(object);
                self.expr(index);
            }
        }
    }
}
