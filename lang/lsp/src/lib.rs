//! Language server for Belsk2 (`belsk2 lsp`), speaking the Language Server
//! Protocol over stdin/stdout. Works with VS Code, Zed, Neovim, Helix and
//! any other LSP client. All the analysis comes from `belsk2-analysis`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use belsk2_analysis::{
    analyze_path, Analysis, CompletionKind, FileInfo, HintKind, Pos, Range, SymbolKind,
};
use lsp_server::{Connection, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument,
    Notification as _, PublishDiagnostics,
};
use lsp_types::request::{
    Completion, DocumentSymbolRequest, Formatting, GotoDefinition, HoverRequest, InlayHintRequest,
    PrepareRenameRequest, References, Rename, Request as _,
};
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionOptions, CompletionParams, CompletionResponse,
    Diagnostic, DiagnosticSeverity, DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse,
    Documentation, GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverContents, HoverParams,
    HoverProviderCapability, InlayHint, InlayHintKind, InlayHintLabel, InlayHintParams, Location,
    MarkupContent, MarkupKind, OneOf, Position, PrepareRenameResponse, PublishDiagnosticsParams,
    ReferenceParams, RenameOptions, RenameParams, ServerCapabilities, ServerInfo,
    TextDocumentPositionParams, TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit, Url,
    WorkspaceEdit,
};

type Result<T> = std::result::Result<T, String>;

/// Runs the server on stdin/stdout until the client shuts it down.
pub fn run() -> Result<()> {
    let (connection, io_threads) = Connection::stdio();
    serve(&connection)?;
    drop(connection);
    io_threads.join().map_err(|e| e.to_string())
}

/// Serves one client over `connection` until it shuts the server down.
pub fn serve(connection: &Connection) -> Result<()> {
    let capabilities = ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        completion_provider: Some(CompletionOptions::default()),
        definition_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        rename_provider: Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: Default::default(),
        })),
        document_symbol_provider: Some(OneOf::Left(true)),
        inlay_hint_provider: Some(OneOf::Left(true)),
        document_formatting_provider: Some(OneOf::Left(true)),
        ..ServerCapabilities::default()
    };
    let init = serde_json::json!({
        "capabilities": capabilities,
        "serverInfo": ServerInfo {
            name: "belsk2".to_string(),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        },
    });
    let (id, _params) = connection.initialize_start().map_err(|e| e.to_string())?;
    connection
        .initialize_finish(id, init)
        .map_err(|e| e.to_string())?;

    Server {
        connection,
        docs: HashMap::new(),
        cache: HashMap::new(),
    }
    .main_loop()
}

struct Server<'a> {
    connection: &'a Connection,
    /// Open documents: path -> (the client's URI, current text).
    docs: HashMap<PathBuf, (Url, String)>,
    /// Analyses by file path; cleared whenever a document changes.
    cache: HashMap<PathBuf, Rc<Analysis>>,
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn url_path(url: &Url) -> Option<PathBuf> {
    url.to_file_path().ok().map(|p| canonical(&p))
}

/// A file URL for a path, without the `\\?\` prefix of Windows canonical
/// paths.
fn path_url(path: &Path) -> Option<Url> {
    let text = path.to_string_lossy();
    let plain = text.strip_prefix(r"\\?\").unwrap_or(&text);
    Url::from_file_path(plain).ok()
}

fn line_text(info: &FileInfo, line: usize) -> &str {
    info.text.lines().nth(line.saturating_sub(1)).unwrap_or("")
}

fn to_lsp(info: &FileInfo, pos: Pos) -> Position {
    let character: usize = line_text(info, pos.line)
        .chars()
        .take(pos.col.saturating_sub(1))
        .map(char::len_utf16)
        .sum();
    Position {
        line: u32::try_from(pos.line.saturating_sub(1)).unwrap_or(u32::MAX),
        character: u32::try_from(character).unwrap_or(u32::MAX),
    }
}

fn from_lsp(info: &FileInfo, pos: Position) -> Pos {
    let line = usize::try_from(pos.line)
        .unwrap_or(usize::MAX)
        .saturating_add(1);
    let target = usize::try_from(pos.character).unwrap_or(usize::MAX);
    let mut units = 0;
    let mut col = 1;
    for c in line_text(info, line).chars() {
        if units >= target {
            break;
        }
        units += c.len_utf16();
        col += 1;
    }
    Pos::new(line, col)
}

fn range_to_lsp(info: &FileInfo, r: Range) -> lsp_types::Range {
    lsp_types::Range {
        start: to_lsp(info, r.start),
        end: to_lsp(info, r.end),
    }
}

fn markdown(value: String) -> MarkupContent {
    MarkupContent {
        kind: MarkupKind::Markdown,
        value,
    }
}

impl Server<'_> {
    fn main_loop(&mut self) -> Result<()> {
        for msg in &self.connection.receiver {
            match msg {
                Message::Request(req) => {
                    if self
                        .connection
                        .handle_shutdown(&req)
                        .map_err(|e| e.to_string())?
                    {
                        return Ok(());
                    }
                    let id = req.id.clone();
                    let response = match self.request(req) {
                        Ok(value) => Response::new_ok(id, value),
                        Err(message) => Response::new_err(
                            id,
                            lsp_server::ErrorCode::InvalidParams as i32,
                            message,
                        ),
                    };
                    self.send(Message::Response(response))?;
                }
                Message::Notification(note) => self.notification(note)?,
                Message::Response(_) => {}
            }
        }
        Ok(())
    }

    fn send(&self, msg: Message) -> Result<()> {
        self.connection.sender.send(msg).map_err(|e| e.to_string())
    }

    fn notification(&mut self, note: Notification) -> Result<()> {
        let changed = match note.method.as_str() {
            DidOpenTextDocument::METHOD => {
                let p: lsp_types::DidOpenTextDocumentParams = params(note.params)?;
                let doc = p.text_document;
                let path = url_path(&doc.uri);
                if let Some(path) = &path {
                    self.docs.insert(path.clone(), (doc.uri, doc.text));
                }
                path
            }
            DidChangeTextDocument::METHOD => {
                let p: lsp_types::DidChangeTextDocumentParams = params(note.params)?;
                let path = url_path(&p.text_document.uri);
                if let (Some(path), Some(change)) = (&path, p.content_changes.into_iter().last()) {
                    self.docs
                        .insert(path.clone(), (p.text_document.uri, change.text));
                }
                path
            }
            DidSaveTextDocument::METHOD => {
                let p: lsp_types::DidSaveTextDocumentParams = params(note.params)?;
                url_path(&p.text_document.uri)
            }
            DidCloseTextDocument::METHOD => {
                let p: lsp_types::DidCloseTextDocumentParams = params(note.params)?;
                let path = url_path(&p.text_document.uri);
                if let Some(path) = &path {
                    self.docs.remove(path);
                }
                path
            }
            _ => None,
        };
        if let Some(path) = changed {
            self.cache.clear();
            self.publish(&path)?;
        }
        Ok(())
    }

    /// The analysis containing `path`, computed on first use.
    fn analysis(&mut self, path: &Path) -> Rc<Analysis> {
        if let Some(a) = self.cache.get(path) {
            return a.clone();
        }
        let overlays = self
            .docs
            .iter()
            .map(|(p, (_, text))| (p.clone(), text.clone()))
            .collect();
        let analysis = Rc::new(analyze_path(path, &overlays));
        for f in &analysis.files {
            self.cache.insert(f.path.clone(), analysis.clone());
        }
        self.cache.insert(path.to_path_buf(), analysis.clone());
        analysis
    }

    fn uri_for(&self, path: &Path) -> Option<Url> {
        match self.docs.get(path) {
            Some((uri, _)) => Some(uri.clone()),
            None => path_url(path),
        }
    }

    /// Sends the errors of every file analyzed together with `path`.
    fn publish(&mut self, path: &Path) -> Result<()> {
        let analysis = self.analysis(path);
        let current = analysis.file_id(path);
        for info in &analysis.files {
            let Some(uri) = self.uri_for(&info.path) else {
                continue;
            };
            let mut diagnostics: Vec<Diagnostic> = analysis
                .diagnostics_for(info.id)
                .map(|d| Diagnostic {
                    range: range_to_lsp(info, d.range),
                    severity: Some(DiagnosticSeverity::ERROR),
                    source: Some("belsk2".to_string()),
                    message: d.message.clone(),
                    ..Diagnostic::default()
                })
                .collect();
            if Some(info.id) == current {
                diagnostics.extend(analysis.general_diagnostics().map(|d| Diagnostic {
                    severity: Some(DiagnosticSeverity::ERROR),
                    source: Some("belsk2".to_string()),
                    message: d.message.clone(),
                    ..Diagnostic::default()
                }));
            }
            let note = Notification::new(
                PublishDiagnostics::METHOD.to_string(),
                PublishDiagnosticsParams {
                    uri,
                    diagnostics,
                    version: None,
                },
            );
            self.send(Message::Notification(note))?;
        }
        Ok(())
    }

    /// Analysis, file and position for a request about a document.
    fn locate(
        &mut self,
        doc: &TextDocumentPositionParams,
    ) -> Option<(Rc<Analysis>, belsk2_analysis::FileId, Pos)> {
        let path = url_path(&doc.text_document.uri)?;
        let analysis = self.analysis(&path);
        let file = analysis.file_id(&path)?;
        let pos = from_lsp(analysis.file(file)?, doc.position);
        Some((analysis, file, pos))
    }

    fn location(&self, analysis: &Analysis, loc: belsk2_analysis::Location) -> Option<Location> {
        let info = analysis.file(loc.file)?;
        Some(Location {
            uri: self.uri_for(&info.path)?,
            range: range_to_lsp(info, loc.range),
        })
    }

    fn request(&mut self, req: Request) -> Result<serde_json::Value> {
        let value = match req.method.as_str() {
            HoverRequest::METHOD => {
                let p: HoverParams = params(req.params)?;
                let hover =
                    self.locate(&p.text_document_position_params)
                        .and_then(|(a, file, pos)| {
                            let (range, text) = a.hover(file, pos)?;
                            Some(Hover {
                                contents: HoverContents::Markup(markdown(text)),
                                range: a.file(file).map(|info| range_to_lsp(info, range)),
                            })
                        });
                serde_json::to_value(hover)
            }
            GotoDefinition::METHOD => {
                let p: GotoDefinitionParams = params(req.params)?;
                let loc = self
                    .locate(&p.text_document_position_params)
                    .and_then(|(a, file, pos)| self.location(&a, a.definition(file, pos)?));
                serde_json::to_value(loc.map(GotoDefinitionResponse::Scalar))
            }
            References::METHOD => {
                let p: ReferenceParams = params(req.params)?;
                let locs: Vec<Location> = match self.locate(&p.text_document_position) {
                    Some((a, file, pos)) => a
                        .references(file, pos)
                        .into_iter()
                        .filter_map(|l| self.location(&a, l))
                        .collect(),
                    None => Vec::new(),
                };
                serde_json::to_value(locs)
            }
            PrepareRenameRequest::METHOD => {
                let p: TextDocumentPositionParams = params(req.params)?;
                let range = self.locate(&p).and_then(|(a, file, pos)| {
                    if !a.can_rename(file, pos) {
                        return None;
                    }
                    let (_, range) = a.symbol_at(file, pos)?;
                    Some(PrepareRenameResponse::Range(range_to_lsp(
                        a.file(file)?,
                        range,
                    )))
                });
                serde_json::to_value(range)
            }
            Rename::METHOD => {
                let p: RenameParams = params(req.params)?;
                let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
                if let Some((a, file, pos)) = self.locate(&p.text_document_position) {
                    if !a.can_rename(file, pos) {
                        return Err("built-in functions cannot be renamed".to_string());
                    }
                    for loc in a.references(file, pos) {
                        if let Some(l) = self.location(&a, loc) {
                            changes.entry(l.uri).or_default().push(TextEdit {
                                range: l.range,
                                new_text: p.new_name.clone(),
                            });
                        }
                    }
                }
                serde_json::to_value(WorkspaceEdit {
                    changes: Some(changes),
                    ..WorkspaceEdit::default()
                })
            }
            Completion::METHOD => {
                let p: CompletionParams = params(req.params)?;
                let items: Vec<CompletionItem> = match self.locate(&p.text_document_position) {
                    Some((a, file, pos)) => a
                        .completions(file, pos)
                        .into_iter()
                        .map(|c| CompletionItem {
                            kind: Some(match c.kind {
                                CompletionKind::Function => CompletionItemKind::FUNCTION,
                                CompletionKind::Variable => CompletionItemKind::VARIABLE,
                                CompletionKind::Parameter => CompletionItemKind::VARIABLE,
                                CompletionKind::Keyword => CompletionItemKind::KEYWORD,
                                CompletionKind::Type => CompletionItemKind::CLASS,
                                CompletionKind::Module => CompletionItemKind::MODULE,
                                CompletionKind::Text => CompletionItemKind::TEXT,
                            }),
                            sort_text: Some(format!(
                                "{}{}",
                                match c.kind {
                                    CompletionKind::Variable | CompletionKind::Parameter => 0,
                                    CompletionKind::Function | CompletionKind::Module => 1,
                                    CompletionKind::Keyword | CompletionKind::Type => 2,
                                    CompletionKind::Text => 3,
                                },
                                c.label
                            )),
                            label: c.label,
                            detail: c.detail,
                            documentation: c.doc.map(|d| Documentation::MarkupContent(markdown(d))),
                            ..CompletionItem::default()
                        })
                        .collect(),
                    None => Vec::new(),
                };
                serde_json::to_value(CompletionResponse::Array(items))
            }
            InlayHintRequest::METHOD => {
                let p: InlayHintParams = params(req.params)?;
                let hints: Vec<InlayHint> = url_path(&p.text_document.uri)
                    .map(|path| {
                        let a = self.analysis(&path);
                        let Some(info) = a.file_id(&path).and_then(|id| a.file(id)) else {
                            return Vec::new();
                        };
                        a.hints_for(info.id)
                            .map(|h| InlayHint {
                                position: to_lsp(info, h.pos),
                                label: InlayHintLabel::String(h.label.clone()),
                                kind: Some(match h.kind {
                                    HintKind::Type => InlayHintKind::TYPE,
                                    HintKind::Parameter => InlayHintKind::PARAMETER,
                                }),
                                text_edits: None,
                                tooltip: None,
                                padding_left: None,
                                padding_right: Some(h.kind == HintKind::Parameter),
                                data: None,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                serde_json::to_value(hints)
            }
            DocumentSymbolRequest::METHOD => {
                let p: DocumentSymbolParams = params(req.params)?;
                let symbols: Vec<DocumentSymbol> = url_path(&p.text_document.uri)
                    .map(|path| {
                        let a = self.analysis(&path);
                        let Some(info) = a.file_id(&path).and_then(|id| a.file(id)) else {
                            return Vec::new();
                        };
                        a.document_symbols(info.id)
                            .into_iter()
                            .filter_map(|s| {
                                let range = range_to_lsp(info, s.def?.range);
                                #[allow(deprecated)]
                                Some(DocumentSymbol {
                                    name: s.name.clone(),
                                    detail: Some(s.signature.clone()),
                                    kind: match s.kind {
                                        SymbolKind::Function | SymbolKind::Builtin => {
                                            lsp_types::SymbolKind::FUNCTION
                                        }
                                        _ => lsp_types::SymbolKind::VARIABLE,
                                    },
                                    tags: None,
                                    deprecated: None,
                                    range,
                                    selection_range: range,
                                    children: None,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                serde_json::to_value(DocumentSymbolResponse::Nested(symbols))
            }
            Formatting::METHOD => {
                let p: lsp_types::DocumentFormattingParams = params(req.params)?;
                let path = url_path(&p.text_document.uri).ok_or("not a file")?;
                let text = match self.docs.get(&path) {
                    Some((_, text)) => text.clone(),
                    None => std::fs::read_to_string(&path).map_err(|e| e.to_string())?,
                };
                let config = belsk2_fmt::Config::for_file(&path).map_err(|e| e.message)?;
                let formatted = belsk2_fmt::format(&text, &config).map_err(|e| e.message)?;
                let edits = if formatted == text {
                    Vec::new()
                } else {
                    let lines = u32::try_from(text.lines().count()).unwrap_or(u32::MAX);
                    vec![TextEdit {
                        range: lsp_types::Range {
                            start: Position {
                                line: 0,
                                character: 0,
                            },
                            end: Position {
                                line: lines.saturating_add(1),
                                character: 0,
                            },
                        },
                        new_text: formatted,
                    }]
                };
                serde_json::to_value(edits)
            }
            _ => return Err(format!("unsupported request {}", req.method)),
        };
        value.map_err(|e| e.to_string())
    }
}

fn params<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> Result<T> {
    serde_json::from_value(value).map_err(|e| e.to_string())
}
