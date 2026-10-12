//! Builds the symbol index: walks every checked module, links each use of
//! a name to its declaration and records scopes and type hints.

use std::collections::HashMap;

use belsk2::project::Loaded;
use belsk2::syntax::ast::{Block, Expr, ExprKind, FnDecl, MemberKind, Stmt, StmtKind, TypeDecl};
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

    // Functions of the WIT world: those it imports directly are visible
    // in every module of the project; those of an imported interface are
    // the exports of its (source-less) module.
    let mut world_items = HashMap::new();
    if let Some(world) = &loaded.world {
        for f in world.root_imports() {
            let id = wit_symbol(analysis, f, "the component world");
            world_items.insert(f.name.clone(), id);
        }
    }

    let mut exports: Vec<Option<HashMap<String, usize>>> = vec![None; loaded.modules.len()];
    for &id in &loaded.order {
        let Some(module) = loaded.modules.get(id) else {
            continue;
        };
        if let (Some(world), Some(iface)) = (&loaded.world, &module.interface) {
            let mut items = HashMap::new();
            for f in world.interface_imports(&iface.module) {
                let from = format!("WIT interface {}", iface.key);
                items.insert(f.name.clone(), wit_symbol(analysis, f, &from));
            }
            if let Some(slot) = exports.get_mut(id) {
                *slot = Some(items);
            }
            continue;
        }
        let Some(file) = analysis.files.iter().position(|f| f.id == module.file) else {
            continue;
        };
        let info = analysis.files.get(file).cloned();
        let Some(info) = info else {
            continue;
        };
        let mut imports = HashMap::new();
        if module.package == 0 {
            imports.extend(world_items.iter().map(|(k, v)| (k.clone(), *v)));
        }
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
        let mut own = ix.frames.pop().unwrap_or_default();
        // `private` declarations are not seen by other modules.
        for stmt in &module.program.stmts {
            let private = match &stmt.kind {
                StmtKind::FnDecl(d) if d.access == belsk2::syntax::Access::Private => Some(&d.name),
                StmtKind::VarDecl(d) if d.access == belsk2::syntax::Access::Private => {
                    Some(&d.name)
                }
                StmtKind::Type(d) if d.access == belsk2::syntax::Access::Private => Some(&d.name),
                _ => None,
            };
            if let Some(name) = private {
                own.remove(name);
            }
        }
        ix.scope_stack.pop();
        if let Some(slot) = exports.get_mut(id) {
            *slot = Some(own);
        }
    }
}

/// A symbol for a function the host provides through WIT.
fn wit_symbol(analysis: &mut Analysis, f: &belsk2::project::WitFunc, from: &str) -> usize {
    let mut doc = f.docs.clone().unwrap_or_default();
    if let Some(why) = &f.unsupported {
        if !doc.is_empty() {
            doc.push_str("\n\n");
        }
        doc.push_str(&format!("Cannot be used in Belsk2 yet: {why}."));
    }
    if !doc.is_empty() {
        doc.push_str("\n\n");
    }
    doc.push_str(&format!(
        "Provided by the host (`{}` in {from}).",
        f.wit_name
    ));
    analysis.symbols.push(Symbol {
        name: f.name.clone(),
        kind: SymbolKind::Function,
        ty: f.ty(),
        signature: f.signature(),
        doc: Some(doc),
        def: None,
        module: None,
        top_level: true,
        params: f
            .params
            .iter()
            .map(|(n, _)| belsk2::project::wit::snake(n))
            .collect(),
    });
    analysis.symbols.len() - 1
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
        // Functions and types can be used before they are declared.
        for stmt in stmts {
            if let StmtKind::Type(decl) = &stmt.kind {
                self.declare_type(decl, stmt.span);
            }
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

    /// Makes the newest entry of the current scope visible in the whole
    /// scope.
    fn visible_in_whole_scope(&mut self) {
        if let Some(&scope) = self.scope_stack.last() {
            if let Some(s) = self.analysis.scopes.get_mut(scope) {
                if let Some(e) = s.entries.last_mut() {
                    e.1 = s.start;
                }
            }
        }
    }

    /// The symbol of a struct and of its fields and methods.
    fn declare_type(&mut self, decl: &TypeDecl, span: Span) {
        let range = self.name_range(span, &decl.name);
        let ty = decl
            .info
            .as_ref()
            .map(|i| Ty::Type(i.reference()))
            .unwrap_or(Ty::Any);
        let sym = self.symbol(
            &decl.name,
            SymbolKind::Type,
            ty,
            {
                let mut sig = format!("{} {}", decl.kind.keyword(), decl.name);
                if decl.is_abstract {
                    sig = format!("abstract {sig}");
                }
                if !decl.bases.is_empty() {
                    let bases: Vec<String> = decl.bases.iter().map(|b| b.to_string()).collect();
                    sig.push_str(&format!(" : {}", bases.join(", ")));
                }
                sig
            },
            range,
        );
        self.declare(sym);
        self.visible_in_whole_scope();
        let Some(info) = &decl.info else {
            return;
        };
        self.analysis.types.push(std::rc::Rc::clone(info));
        for m in &decl.members {
            let mut mods = String::new();
            if let Some(access) = m
                .mods
                .access
                .filter(|a| *a != belsk2::syntax::Access::Public)
            {
                mods.push_str(access.keyword());
                mods.push(' ');
            }
            if m.mods.is_static {
                mods.push_str("static ");
            }
            match m.mods.virt {
                belsk2::syntax::Virt::Virtual => mods.push_str("virtual "),
                belsk2::syntax::Virt::Override => mods.push_str("override "),
                belsk2::syntax::Virt::Abstract => mods.push_str("abstract "),
                belsk2::syntax::Virt::None => {}
            }
            let (name, kind, ty, signature) = match &m.kind {
                MemberKind::Field(f) => {
                    let accessors = match (f.property, f.readonly, f.set_access) {
                        (false, ..) => String::new(),
                        (true, true, _) => " { get; }".to_string(),
                        (true, false, Some(a)) => format!(" {{ get; {} set; }}", a.keyword()),
                        (true, false, None) => " { get; set; }".to_string(),
                    };
                    (
                        f.name.clone(),
                        SymbolKind::Field,
                        f.resolved.clone(),
                        format!("{mods}{}.{}: {}{accessors}", decl.name, f.name, f.resolved),
                    )
                }
                MemberKind::Prop(p) => {
                    let mut accessors = vec!["get;".to_string()];
                    if let Some(set) = &p.set {
                        accessors.push(match set.access {
                            Some(a) => format!("{} set;", a.keyword()),
                            None => "set;".to_string(),
                        });
                    }
                    (
                        p.name.clone(),
                        SymbolKind::Field,
                        p.resolved.clone(),
                        format!(
                            "{mods}{}.{}: {} {{ {} }}",
                            decl.name,
                            p.name,
                            p.resolved,
                            accessors.join(" ")
                        ),
                    )
                }
                MemberKind::Method(d) => {
                    let sig = fn_signature(d);
                    let sig = sig.strip_prefix("fn ").unwrap_or(&sig);
                    (
                        d.name.clone(),
                        SymbolKind::Method,
                        belsk2::typeck::fn_type(d),
                        format!("{mods}fn {}.{sig}", decl.name),
                    )
                }
                MemberKind::Init(_) | MemberKind::Drop(_) => continue,
                MemberKind::Case { name, .. } => {
                    let value = info
                        .reference()
                        .variants()
                        .into_iter()
                        .find(|v| v.name == *name)
                        .map(|v| v.value)
                        .unwrap_or(0);
                    (
                        name.clone(),
                        SymbolKind::Field,
                        Ty::Named(info.reference()),
                        format!("{}.{name} = {value}", decl.name),
                    )
                }
            };
            let range = self.name_range(m.span, &name);
            let mut sym = self.symbol(&name, kind, ty, signature, range);
            sym.top_level = false;
            sym.module = None;
            if let MemberKind::Method(d) = &m.kind {
                sym.params = d.params.iter().map(|p| p.name.clone()).collect();
            }
            let id = self.analysis.symbols.len();
            self.analysis.symbols.push(sym);
            self.analysis.members.insert((info.id(), name), id);
        }
    }

    /// Walks a function body with its parameters (and `this` for a
    /// method) in scope.
    fn walk_fn(&mut self, decl: &FnDecl, symbol: Option<usize>, this: Option<Ty>) {
        if let Some(symbol) = symbol {
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
            if let Some(ty) = this {
                let range = ix.name_range(decl.span, &decl.name);
                let mut sym = ix.symbol(
                    "this",
                    SymbolKind::Parameter,
                    ty.clone(),
                    format!("this: {ty}"),
                    range,
                );
                sym.def = None;
                ix.declare(sym);
            }
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
        if pushed > 0 && symbol.is_some() && self.fn_stack.len() == pushed {
            self.fn_stack.pop();
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
                let symbol = self.lookup(&decl.name);
                self.walk_fn(decl, symbol, None);
            }
            StmtKind::Type(decl) => {
                let this = decl.info.as_ref().map(|i| Ty::Named(i.reference()));
                for m in &decl.members {
                    match &m.kind {
                        MemberKind::Field(f) => {
                            if let Some(v) = &f.value {
                                self.expr(v);
                            }
                        }
                        MemberKind::Method(d) => {
                            let symbol = decl.info.as_ref().and_then(|i| {
                                self.analysis
                                    .members
                                    .get(&(i.id(), d.name.clone()))
                                    .copied()
                            });
                            let this = this.clone().filter(|_| !m.mods.is_static);
                            self.walk_fn(d, symbol, this);
                        }
                        MemberKind::Prop(p) => {
                            for a in p.get.iter().chain(p.set.iter()) {
                                let this = this.clone().filter(|_| !m.mods.is_static);
                                self.walk_fn(&a.decl, None, this);
                            }
                        }
                        MemberKind::Init(d) | MemberKind::Drop(d) => {
                            self.walk_fn(d, None, this.clone())
                        }
                        MemberKind::Case { value, .. } => {
                            if let Some(v) = value {
                                self.expr(v);
                            }
                        }
                    }
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

    /// Records a use of `name` at `span`.
    fn reference(&mut self, name: &str, span: Span) {
        if let Some(symbol) = self.lookup(name) {
            let start = Pos::of(span);
            let range = Range {
                start,
                end: start.right(name.chars().count()),
            };
            self.use_symbol(symbol, range);
        }
    }

    fn use_symbol(&mut self, symbol: usize, range: Range) {
        if let Some(f) = self
            .fn_stack
            .last()
            .and_then(|&f| self.analysis.functions.get_mut(f))
        {
            if !f.uses.contains(&symbol) {
                f.uses.push(symbol);
            }
        }
        self.analysis.refs.push(Ref {
            loc: Location {
                file: self.file(),
                range,
            },
            symbol,
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
            ExprKind::This => self.reference("this", e.span),
            ExprKind::SelfType | ExprKind::Base => {}
            ExprKind::Is { expr, .. } | ExprKind::As { expr, .. } => self.expr(expr),
            ExprKind::Ident(name) => self.reference(name, e.span),
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
            ExprKind::Member { object, name } => {
                // `x` inside a method is `this.x` with no `this` written.
                let implicit = matches!(object.kind, ExprKind::This | ExprKind::SelfType)
                    && object.span == e.span;
                if !implicit {
                    self.expr(object);
                }
                if let Ty::Named(t) | Ty::Type(t) = &object.ty {
                    let symbol = t.member(name).and_then(|(owner, _)| {
                        self.analysis
                            .members
                            .get(&(owner.id(), name.clone()))
                            .copied()
                    });
                    if let Some(symbol) = symbol {
                        let range = self.name_range(e.span, name);
                        self.use_symbol(symbol, range);
                    }
                }
            }
            ExprKind::Index { object, index } => {
                self.expr(object);
                self.expr(index);
            }
        }
    }
}
