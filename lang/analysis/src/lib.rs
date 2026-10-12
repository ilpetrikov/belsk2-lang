//! Code analysis for Belsk2 editors and IDEs.
//!
//! Loads a file or a whole project (using unsaved editor contents when
//! given), checks it and builds an index of every declaration and every use
//! of a name. Editors query the result: errors, the type under the cursor,
//! completions, go to definition, references, type hints and the module
//! graph. The language server (`belsk2 lsp`) and the IDE both use this
//! crate.
//!
//! ```
//! let a = belsk2_analysis::analyze_source("demo.belsk2", "fn main() {\n    var x = 1 + 2;\n    prinb(x);\n}\n");
//! assert!(a.diagnostics.is_empty());
//! let file = a.files[0].id;
//! // Hovering `x` in `prinb(x)` shows its type.
//! let (_, text) = a.hover(file, belsk2_analysis::Pos::new(3, 11)).unwrap();
//! assert!(text.contains("var x: int"));
//! ```

mod builtins;
mod index;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use belsk2::project::{self, Loaded, Mode, Module, Options, Package};
use belsk2::syntax::ast::Program;
use belsk2::syntax::token::{Token, TokenKind};
pub use belsk2::syntax::FileId;
use belsk2::syntax::{ErrorKind, SourceMap, Span};
use belsk2::Error;
pub use belsk2::Ty;

/// A position in a file: 1-based line and column, counted in characters
/// (the same as [`Span`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

impl Pos {
    pub const fn new(line: usize, col: usize) -> Self {
        Pos { line, col }
    }

    const MAX: Pos = Pos::new(usize::MAX, usize::MAX);

    fn of(span: Span) -> Self {
        Pos::new(span.line, span.col)
    }

    fn right(self, n: usize) -> Self {
        Pos::new(self.line, self.col.saturating_add(n))
    }
}

/// `start` up to (not including) `end`, on one or more lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Range {
    pub start: Pos,
    pub end: Pos,
}

impl Range {
    /// Whether `pos` is inside, or right at the end (where a cursor
    /// after the last character of a name is).
    pub fn touches(&self, pos: Pos) -> bool {
        self.start <= pos && pos <= self.end
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Location {
    pub file: FileId,
    pub range: Range,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Function,
    Variable,
    Parameter,
    Builtin,
    /// A `struct`, `enum` or `class`.
    Type,
    /// A field of a struct or class, or a case of an enum.
    Field,
    /// A method.
    Method,
}

/// A declared name.
#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub ty: Ty,
    /// What hover shows, e.g. `fn add(a: int, b: int): int`.
    pub signature: String,
    pub doc: Option<String>,
    /// Where the name is declared (none for built-ins).
    pub def: Option<Location>,
    /// The module it was declared in, for top-level declarations.
    pub module: Option<String>,
    /// Declared at the top level of a module.
    pub top_level: bool,
    /// Parameter names of a function, for parameter hints.
    pub params: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    /// None for errors that belong to no file (a broken `belsk2.toml`, a
    /// missing dependency, ...).
    pub file: Option<FileId>,
    pub range: Range,
    pub message: String,
    pub severity: Severity,
    pub kind: ErrorKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintKind {
    /// `: int` after a `var` name.
    Type,
    /// `count: ` before a literal argument.
    Parameter,
}

#[derive(Debug, Clone)]
pub struct InlayHint {
    pub file: FileId,
    pub pos: Pos,
    pub label: String,
    pub kind: HintKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    Function,
    Variable,
    Parameter,
    Field,
    Method,
    Keyword,
    Type,
    Module,
    /// A word found elsewhere in the file.
    Text,
}

#[derive(Debug, Clone)]
pub struct Completion {
    pub label: String,
    pub kind: CompletionKind,
    pub detail: Option<String>,
    pub doc: Option<String>,
}

/// A module in the module graph.
#[derive(Debug, Clone)]
pub struct ModuleNode {
    /// Dotted module name (`utils`, `net.http`).
    pub name: String,
    pub package: String,
    pub path: PathBuf,
    pub file: FileId,
    /// Indices of the modules it uses.
    pub uses: Vec<usize>,
    /// Number of errors in the module's file.
    pub errors: usize,
    /// Whether the program starts here.
    pub entry: bool,
    /// Whether it belongs to a dependency rather than the project itself.
    pub dependency: bool,
}

/// A function: what it declares and what it uses.
#[derive(Debug, Clone)]
pub struct FnInfo {
    /// The function's symbol.
    pub symbol: usize,
    pub file: FileId,
    /// Parameters and variables declared in it (symbols).
    pub locals: Vec<usize>,
    /// Symbols it refers to (functions it calls, globals it reads, ...).
    pub uses: Vec<usize>,
}

/// A source file that was analyzed.
#[derive(Debug, Clone)]
pub struct FileInfo {
    pub id: FileId,
    pub path: PathBuf,
    /// The name shown in messages.
    pub name: String,
    pub text: String,
    tokens: Vec<Token>,
    /// Position of each `{` -> position just after its `}`.
    braces: HashMap<Pos, Pos>,
}

/// Names visible in a part of a file.
#[derive(Debug, Clone)]
struct Scope {
    file: FileId,
    start: Pos,
    end: Pos,
    /// Symbol and the position from which it is visible.
    entries: Vec<(usize, Pos)>,
}

#[derive(Debug, Clone, Copy)]
struct Ref {
    loc: Location,
    symbol: usize,
}

/// The result of analyzing a file or project.
#[derive(Debug, Clone, Default)]
pub struct Analysis {
    pub files: Vec<FileInfo>,
    pub diagnostics: Vec<Diagnostic>,
    pub symbols: Vec<Symbol>,
    pub hints: Vec<InlayHint>,
    pub modules: Vec<ModuleNode>,
    /// Every function with its parameters, variables and the names it uses.
    pub functions: Vec<FnInfo>,
    /// Root directory of the project, if the file belongs to one.
    pub root: Option<PathBuf>,
    refs: Vec<Ref>,
    scopes: Vec<Scope>,
    /// The symbols of fields and methods, by type id and name.
    members: HashMap<(usize, String), usize>,
    /// Keeps the user-defined types alive once the syntax trees that own
    /// them are gone (types in symbols only refer to them).
    types: Vec<std::rc::Rc<belsk2::syntax::TypeInfo>>,
}

/// Analyzes source text that is not on disk.
pub fn analyze_source(name: &str, text: &str) -> Analysis {
    let mut errors = Vec::new();
    let repaired = repair(text);
    let text = repaired.as_deref().unwrap_or(text);
    let mut sources = SourceMap::new();
    let file = sources.add(name, text);
    let program = belsk2::syntax::parse_file(text, file).unwrap_or_else(|e| {
        errors.push(e);
        Program { stmts: Vec::new() }
    });
    let loaded = Loaded {
        packages: vec![Package {
            name: "main".to_string(),
            version: "0.0.0".to_string(),
            root: PathBuf::new(),
            src: PathBuf::new(),
            deps: BTreeMap::new(),
        }],
        modules: vec![Module {
            package: 0,
            name: "main".to_string(),
            path: PathBuf::from(name),
            file,
            program,
            uses: Vec::new(),
            interface: None,
        }],
        sources,
        entry: Some(0),
        order: vec![0],
        world: None,
    };
    build(loaded, errors, None)
}

/// Analyzes the file at `path`: the whole project when the file belongs to
/// one (a `belsk2.toml` above it), otherwise the file and the modules it
/// uses. `overlays` holds unsaved editor contents by path. Never touches
/// the network or `belsk2.lock`.
pub fn analyze_path(path: &Path, overlays: &HashMap<PathBuf, String>) -> Analysis {
    let overlays = overlays
        .iter()
        .map(|(p, text)| (p.clone(), repair(text).unwrap_or_else(|| text.clone())))
        .collect();
    let options = Options {
        offline: true,
        write_lock: false,
        overlays,
        ..Options::default()
    };
    if let Some(root) = path.parent().and_then(project::find_root) {
        let (loaded, errors) = project::load_lenient(&root, Mode::All, &options);
        let analysis = build(loaded, errors, Some(root));
        if analysis.file_id(path).is_some() {
            return analysis;
        }
    }
    let (loaded, errors) = project::load_file_lenient(path, &options);
    build(loaded, errors, None)
}

/// Stands in for the member name being typed after a `.`.
const PLACEHOLDER: &str = "__member__";

/// While a member name is being typed (`p.` at the end of a line), the
/// file does not parse, and nothing would be known about `p`. If inserting
/// a placeholder name after such dots makes it parse, that text is
/// analyzed instead (errors about the placeholder become "expected a
/// member name").
fn repair(text: &str) -> Option<String> {
    if belsk2::syntax::parse(text).is_ok() {
        return None;
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 16);
    let mut changed = false;
    for (i, &c) in chars.iter().enumerate() {
        out.push(c);
        if c != '.' {
            continue;
        }
        let prev = i.checked_sub(1).and_then(|j| chars.get(j)).copied();
        let next = chars.get(i + 1).copied();
        let after_value =
            prev.is_some_and(|p| p.is_alphanumeric() || p == '_' || p == ')' || p == ']');
        let dangling = !next.is_some_and(|n| n.is_alphanumeric() || n == '_');
        if after_value && dangling && !prev.is_some_and(|p| p.is_ascii_digit()) {
            out.push_str(PLACEHOLDER);
            changed = true;
        }
    }
    (changed && belsk2::syntax::parse(&out).is_ok()).then_some(out)
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn build(mut loaded: Loaded, mut errors: Vec<Error>, root: Option<PathBuf>) -> Analysis {
    errors.extend(project::check(&mut loaded));
    let mut analysis = Analysis {
        root,
        ..Analysis::default()
    };
    for module in &loaded.modules {
        let Some(source) = loaded.sources.get(module.file) else {
            continue;
        };
        let tokens = belsk2::syntax::tokenize_file(&source.text, module.file).unwrap_or_default();
        let braces = match_braces(&tokens);
        analysis.files.push(FileInfo {
            id: module.file,
            path: canonical(&module.path),
            name: source.name.clone(),
            text: source.text.clone(),
            tokens,
            braces,
        });
    }

    // A file that does not parse also "has no main"; only show the first.
    let broken: Vec<FileId> = errors
        .iter()
        .filter(|e| e.kind == ErrorKind::Syntax)
        .filter_map(|e| e.span.map(|s| s.file))
        .collect();
    errors.retain(|e| {
        !(e.message.contains("has no `fn main()`")
            && e.span.is_some_and(|s| broken.contains(&s.file)))
    });
    for mut e in errors {
        if e.message.contains(PLACEHOLDER) {
            e.message = "expected a member name after '.'".to_string();
        }
        let range = match e.span {
            Some(span) => analysis.token_range(span),
            None => Range::default(),
        };
        analysis.diagnostics.push(Diagnostic {
            file: e.span.map(|s| s.file),
            range,
            message: e.message,
            severity: Severity::Error,
            kind: e.kind,
        });
    }

    index::index(&mut analysis, &loaded);

    for (id, module) in loaded.modules.iter().enumerate() {
        let errors = analysis
            .diagnostics
            .iter()
            .filter(|d| d.file == Some(module.file))
            .count();
        analysis.modules.push(ModuleNode {
            name: module.name.clone(),
            package: loaded
                .packages
                .get(module.package)
                .map(|p| p.name.clone())
                .unwrap_or_default(),
            path: module.path.clone(),
            file: module.file,
            uses: module.uses.iter().map(|(u, _)| *u).collect(),
            errors,
            entry: loaded.entry == Some(id),
            dependency: module.package != 0,
        });
    }
    analysis
}

/// Maps each `{` to the position just after its matching `}`.
fn match_braces(tokens: &[Token]) -> HashMap<Pos, Pos> {
    let mut open = Vec::new();
    let mut map = HashMap::new();
    for t in tokens {
        match t.kind {
            TokenKind::LBrace => open.push(Pos::of(t.span)),
            TokenKind::RBrace => {
                if let Some(start) = open.pop() {
                    map.insert(start, Pos::of(t.span).right(1));
                }
            }
            _ => {}
        }
    }
    map
}

fn token_len(t: &Token) -> usize {
    match t.kind {
        TokenKind::Ident | TokenKind::Number => t.text.chars().count().max(1),
        _ => 1,
    }
}

impl Analysis {
    pub fn file(&self, id: FileId) -> Option<&FileInfo> {
        self.files.iter().find(|f| f.id == id)
    }

    /// The id of the file at `path`, if it was analyzed.
    pub fn file_id(&self, path: &Path) -> Option<FileId> {
        let path = canonical(path);
        self.files.iter().find(|f| f.path == path).map(|f| f.id)
    }

    /// The range of the token starting at `span` (one character if none).
    fn token_range(&self, span: Span) -> Range {
        let start = Pos::of(span);
        let len = self
            .file(span.file)
            .and_then(|f| f.tokens.iter().find(|t| t.span == span))
            .map(token_len)
            .unwrap_or(1);
        Range {
            start,
            end: start.right(len),
        }
    }

    /// Errors in `file`.
    pub fn diagnostics_for(&self, file: FileId) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics
            .iter()
            .filter(move |d| d.file == Some(file))
    }

    /// Errors that belong to no file.
    pub fn general_diagnostics(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics.iter().filter(|d| d.file.is_none())
    }

    /// The symbol whose name (a use or the declaration) is at `pos`, and
    /// the range of that name.
    pub fn symbol_at(&self, file: FileId, pos: Pos) -> Option<(usize, Range)> {
        let used = self
            .refs
            .iter()
            .find(|r| r.loc.file == file && r.loc.range.touches(pos))
            .map(|r| (r.symbol, r.loc.range));
        used.or_else(|| {
            self.symbols.iter().enumerate().find_map(|(i, s)| {
                let def = s.def?;
                (def.file == file && def.range.touches(pos)).then_some((i, def.range))
            })
        })
    }

    /// Markdown describing the name at `pos`.
    pub fn hover(&self, file: FileId, pos: Pos) -> Option<(Range, String)> {
        let (id, range) = self.symbol_at(file, pos)?;
        let symbol = self.symbols.get(id)?;
        let mut text = format!("```belsk2\n{}\n```", symbol.signature);
        let elsewhere = symbol.def.is_some_and(|d| d.file != file);
        if let Some(module) = symbol.module.as_ref().filter(|_| elsewhere) {
            text.push_str(&format!("\n\nfrom module `{module}`"));
        }
        if let Some(doc) = &symbol.doc {
            text.push_str("\n\n");
            text.push_str(doc);
        }
        Some((range, text))
    }

    /// Where the name at `pos` is declared.
    pub fn definition(&self, file: FileId, pos: Pos) -> Option<Location> {
        let (id, _) = self.symbol_at(file, pos)?;
        self.symbols.get(id)?.def
    }

    /// The declaration and every use of the name at `pos`.
    pub fn references(&self, file: FileId, pos: Pos) -> Vec<Location> {
        let Some((id, _)) = self.symbol_at(file, pos) else {
            return Vec::new();
        };
        let mut locs: Vec<Location> = self
            .symbols
            .get(id)
            .and_then(|s| s.def)
            .into_iter()
            .collect();
        locs.extend(self.refs.iter().filter(|r| r.symbol == id).map(|r| r.loc));
        locs
    }

    /// Whether the name at `pos` can be renamed (it is declared in the
    /// project, not built in).
    pub fn can_rename(&self, file: FileId, pos: Pos) -> bool {
        self.symbol_at(file, pos)
            .and_then(|(id, _)| self.symbols.get(id))
            .is_some_and(|s| s.def.is_some())
    }

    /// Type and parameter hints in `file`.
    pub fn hints_for(&self, file: FileId) -> impl Iterator<Item = &InlayHint> {
        self.hints.iter().filter(move |h| h.file == file)
    }

    /// Top-level declarations of `file`.
    pub fn document_symbols(&self, file: FileId) -> Vec<&Symbol> {
        self.symbols
            .iter()
            .filter(|s| s.top_level && s.def.is_some_and(|d| d.file == file))
            .collect()
    }

    /// Completions at `pos`: names in scope, built-ins, keywords and types.
    pub fn completions(&self, file: FileId, pos: Pos) -> Vec<Completion> {
        let Some(info) = self.file(file) else {
            return Vec::new();
        };
        let line = info
            .text
            .lines()
            .nth(pos.line.saturating_sub(1))
            .unwrap_or("");
        let before: String = line.chars().take(pos.col.saturating_sub(1)).collect();
        let word_start = before.trim_end_matches(|c: char| c.is_alphanumeric() || c == '_');
        if let Some(receiver) = word_start.strip_suffix('.') {
            return self.member_completions(file, pos, receiver);
        }
        let mut out: Vec<Completion> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut add = |out: &mut Vec<Completion>, c: Completion| {
            if seen.insert(c.label.clone()) {
                out.push(c);
            }
        };

        if word_start.trim_start().starts_with("using") {
            for m in &self.modules {
                add(
                    &mut out,
                    Completion {
                        label: m.name.clone(),
                        kind: CompletionKind::Module,
                        detail: Some(format!("module of {}", m.package)),
                        doc: None,
                    },
                );
            }
            return out;
        }

        let mut scopes: Vec<&Scope> = self
            .scopes
            .iter()
            .filter(|s| s.file == file && s.start <= pos && pos <= s.end)
            .collect();
        scopes.sort_by_key(|s| std::cmp::Reverse(s.start));
        for scope in scopes {
            for &(id, from) in scope.entries.iter().rev() {
                if from > pos {
                    continue;
                }
                if let Some(s) = self.symbols.get(id) {
                    add(&mut out, symbol_completion(s));
                }
            }
        }
        for s in self
            .symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Builtin)
        {
            add(&mut out, symbol_completion(s));
        }
        for kw in belsk2::syntax::parser::KEYWORDS {
            add(
                &mut out,
                Completion {
                    label: (*kw).to_string(),
                    kind: CompletionKind::Keyword,
                    detail: None,
                    doc: None,
                },
            );
        }
        for ty in builtins::TYPE_NAMES {
            add(
                &mut out,
                Completion {
                    label: (*ty).to_string(),
                    kind: CompletionKind::Type,
                    detail: Some("type".to_string()),
                    doc: None,
                },
            );
        }
        // Words used elsewhere in the file help while it does not parse.
        let current: String = before.chars().skip(word_start.chars().count()).collect();
        for t in &info.tokens {
            if t.kind == TokenKind::Ident && t.text != current {
                add(
                    &mut out,
                    Completion {
                        label: t.text.clone(),
                        kind: CompletionKind::Text,
                        detail: None,
                        doc: None,
                    },
                );
            }
        }
        out
    }
}

impl Analysis {
    /// Completions after `receiver.`: the fields and methods of the type
    /// of the variable `receiver`.
    fn member_completions(&self, file: FileId, pos: Pos, receiver: &str) -> Vec<Completion> {
        let name: String = receiver
            .chars()
            .rev()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        if name.is_empty() {
            return Vec::new();
        }
        let mut scopes: Vec<&Scope> = self
            .scopes
            .iter()
            .filter(|s| s.file == file && s.start <= pos && pos <= s.end)
            .collect();
        scopes.sort_by_key(|s| std::cmp::Reverse(s.start));
        let ty = scopes.iter().find_map(|scope| {
            scope
                .entries
                .iter()
                .rev()
                .filter(|(_, from)| *from <= pos)
                .filter_map(|(id, _)| self.symbols.get(*id))
                .find(|s| s.name == name)
                .map(|s| s.ty.clone())
        });
        // `Mode.` lists the cases, `p.` the fields and methods.
        let (t, of_type) = match ty {
            Some(Ty::Named(t)) => (t, false),
            Some(Ty::Type(t)) => (t, true),
            _ => return Vec::new(),
        };
        t.members()
            .into_iter()
            .filter(|(_, _, m)| m.is_static() == of_type)
            .map(|(owner, name, _)| {
                let symbol = self
                    .members
                    .get(&(owner.id(), name.clone()))
                    .and_then(|&id| self.symbols.get(id));
                match symbol {
                    Some(s) => symbol_completion(s),
                    None => Completion {
                        label: name,
                        kind: CompletionKind::Field,
                        detail: None,
                        doc: None,
                    },
                }
            })
            .collect()
    }
}

fn symbol_completion(s: &Symbol) -> Completion {
    Completion {
        label: s.name.clone(),
        kind: match s.kind {
            SymbolKind::Function | SymbolKind::Builtin => CompletionKind::Function,
            SymbolKind::Variable => CompletionKind::Variable,
            SymbolKind::Parameter => CompletionKind::Parameter,
            SymbolKind::Type => CompletionKind::Type,
            SymbolKind::Field => CompletionKind::Field,
            SymbolKind::Method => CompletionKind::Method,
        },
        detail: Some(s.signature.clone()),
        doc: s.doc.clone(),
    }
}
