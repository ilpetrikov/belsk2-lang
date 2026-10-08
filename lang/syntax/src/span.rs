/// Identifies a source file within a [`SourceMap`]. `0` is used for code
/// that does not come from a file (REPL input, strings passed by a host).
pub type FileId = u32;

/// A position in source code (1-based line and column, counted in chars).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Span {
    pub file: FileId,
    pub line: usize,
    pub col: usize,
}

impl Span {
    pub const fn new(line: usize, col: usize) -> Self {
        Span { file: 0, line, col }
    }

    pub const fn in_file(file: FileId, line: usize, col: usize) -> Self {
        Span { file, line, col }
    }
}

impl std::fmt::Display for Span {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.line, self.col)
    }
}

/// A source file known to the toolchain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    /// The path shown in error messages.
    pub name: String,
    pub text: String,
}

/// All source files of a program, so errors can show the right file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

impl SourceMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a file and returns its id. Ids start at 1.
    pub fn add(&mut self, name: impl Into<String>, text: impl Into<String>) -> FileId {
        self.files.push(SourceFile {
            name: name.into(),
            text: text.into(),
        });
        FileId::try_from(self.files.len()).unwrap_or(FileId::MAX)
    }

    pub fn get(&self, id: FileId) -> Option<&SourceFile> {
        let index = usize::try_from(id).ok()?.checked_sub(1)?;
        self.files.get(index)
    }
}
