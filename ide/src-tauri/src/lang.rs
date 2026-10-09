//! Code intelligence for the editor, from `belsk2-analysis`.
//!
//! Every request carries the unsaved contents of open files and a
//! `version` that the editor bumps on each change; the last analysis is
//! reused while the version stays the same (hover, completion, ...).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use belsk2_analysis::{analyze_path, Analysis, CompletionKind, HintKind, Location, Pos, Range};
use serde::{Deserialize, Serialize};

use crate::files::plain;

thread_local! {
    static CACHE: RefCell<Option<(u64, Rc<Analysis>)>> = const { RefCell::new(None) };
}

#[derive(Deserialize)]
pub struct Doc {
    path: String,
    version: u64,
    /// Unsaved contents by path.
    overlays: HashMap<String, String>,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
pub struct P {
    line: usize,
    col: usize,
}

#[derive(Serialize)]
pub struct R {
    start: P,
    end: P,
}

#[derive(Serialize)]
pub struct Loc {
    path: String,
    range: R,
}

fn p(pos: Pos) -> P {
    P {
        line: pos.line,
        col: pos.col,
    }
}

fn r(range: Range) -> R {
    R {
        start: p(range.start),
        end: p(range.end),
    }
}

fn analysis(doc: &Doc) -> Rc<Analysis> {
    let path = Path::new(&doc.path);
    let cached = CACHE.with(|c| {
        c.borrow()
            .as_ref()
            .filter(|(v, a)| *v == doc.version && a.file_id(path).is_some())
            .map(|(_, a)| a.clone())
    });
    if let Some(a) = cached {
        return a;
    }
    let overlays: HashMap<PathBuf, String> = doc
        .overlays
        .iter()
        .map(|(k, v)| (PathBuf::from(k), v.clone()))
        .collect();
    let a = Rc::new(analyze_path(path, &overlays));
    CACHE.with(|c| *c.borrow_mut() = Some((doc.version, a.clone())));
    a
}

/// The analysis, the file id and the position for a request.
fn at(doc: &Doc, pos: P) -> Option<(Rc<Analysis>, belsk2_analysis::FileId, Pos)> {
    let a = analysis(doc);
    let file = a.file_id(Path::new(&doc.path))?;
    Some((a, file, Pos::new(pos.line, pos.col)))
}

fn loc(a: &Analysis, l: Location) -> Option<Loc> {
    Some(Loc {
        path: plain(&a.file(l.file)?.path),
        range: r(l.range),
    })
}

#[derive(Serialize)]
pub struct Diag {
    path: Option<String>,
    range: R,
    message: String,
}

#[derive(Serialize)]
pub struct Hint {
    pos: P,
    label: String,
    kind: &'static str,
}

#[derive(Serialize)]
pub struct ModuleDto {
    name: String,
    package: String,
    path: String,
    uses: Vec<usize>,
    errors: usize,
    entry: bool,
    dependency: bool,
}

#[derive(Serialize)]
pub struct Report {
    /// Errors in every analyzed file (and errors that belong to none).
    diagnostics: Vec<Diag>,
    /// Hints for the requested file.
    hints: Vec<Hint>,
    modules: Vec<ModuleDto>,
    root: Option<String>,
}

#[tauri::command]
pub fn analyze(doc: Doc) -> Report {
    let a = analysis(&doc);
    let file = a.file_id(Path::new(&doc.path));
    let diagnostics = a
        .diagnostics
        .iter()
        .map(|d| Diag {
            path: d.file.and_then(|f| a.file(f)).map(|f| plain(&f.path)),
            range: r(d.range),
            message: d.message.clone(),
        })
        .collect();
    let hints = match file {
        Some(f) => a
            .hints_for(f)
            .map(|h| Hint {
                pos: p(h.pos),
                label: h.label.clone(),
                kind: match h.kind {
                    HintKind::Type => "type",
                    HintKind::Parameter => "parameter",
                },
            })
            .collect(),
        None => Vec::new(),
    };
    let modules = a
        .modules
        .iter()
        .map(|m| ModuleDto {
            name: m.name.clone(),
            package: m.package.clone(),
            path: plain(&m.path),
            uses: m.uses.clone(),
            errors: m.errors,
            entry: m.entry,
            dependency: m.dependency,
        })
        .collect();
    Report {
        diagnostics,
        hints,
        modules,
        root: a.root.as_deref().map(plain),
    }
}

#[derive(Serialize)]
pub struct HoverDto {
    range: R,
    text: String,
}

#[tauri::command]
pub fn hover(doc: Doc, pos: P) -> Option<HoverDto> {
    let (a, file, pos) = at(&doc, pos)?;
    let (range, text) = a.hover(file, pos)?;
    Some(HoverDto {
        range: r(range),
        text,
    })
}

#[derive(Serialize)]
pub struct CompletionDto {
    label: String,
    kind: &'static str,
    detail: Option<String>,
    doc: Option<String>,
}

#[tauri::command]
pub fn complete(doc: Doc, pos: P) -> Vec<CompletionDto> {
    let Some((a, file, pos)) = at(&doc, pos) else {
        return Vec::new();
    };
    a.completions(file, pos)
        .into_iter()
        .map(|c| CompletionDto {
            kind: match c.kind {
                CompletionKind::Function => "function",
                CompletionKind::Variable => "variable",
                CompletionKind::Parameter => "parameter",
                CompletionKind::Keyword => "keyword",
                CompletionKind::Type => "type",
                CompletionKind::Module => "module",
                CompletionKind::Text => "text",
            },
            label: c.label,
            detail: c.detail,
            doc: c.doc,
        })
        .collect()
}

#[tauri::command]
pub fn definition(doc: Doc, pos: P) -> Option<Loc> {
    let (a, file, pos) = at(&doc, pos)?;
    loc(&a, a.definition(file, pos)?)
}

#[tauri::command]
pub fn references(doc: Doc, pos: P) -> Vec<Loc> {
    let Some((a, file, pos)) = at(&doc, pos) else {
        return Vec::new();
    };
    a.references(file, pos)
        .into_iter()
        .filter_map(|l| loc(&a, l))
        .collect()
}

/// The places to replace when renaming the name at `pos`.
#[tauri::command]
pub fn rename(doc: Doc, pos: P) -> Result<Vec<Loc>, String> {
    let (a, file, pos) = at(&doc, pos).ok_or("nothing to rename here")?;
    if !a.can_rename(file, pos) {
        return Err("only names declared in the project can be renamed".to_string());
    }
    Ok(a.references(file, pos)
        .into_iter()
        .filter_map(|l| loc(&a, l))
        .collect())
}
