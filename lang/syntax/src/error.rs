use std::fmt;

use crate::span::Span;

/// Which stage of the toolchain produced an [`Error`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// Invalid characters, malformed literals, unexpected tokens.
    Syntax,
    /// A value does not match a declared type.
    Type,
    /// Any other failure while running a program.
    Runtime,
    /// Reading or writing failed (files, stdin, stdout).
    Io,
}

impl ErrorKind {
    pub fn name(self) -> &'static str {
        match self {
            ErrorKind::Syntax => "syntax error",
            ErrorKind::Type => "type error",
            ErrorKind::Runtime => "runtime error",
            ErrorKind::Io => "io error",
        }
    }
}

/// The single error type used across Belsk2. Nothing in the toolchain
/// panics; every failure ends up as one of these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
    pub span: Option<Span>,
    /// Set when the parser ran out of input. A REPL uses this to ask for
    /// another line instead of reporting the error.
    pub incomplete: bool,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Error {
            kind,
            message: message.into(),
            span: None,
            incomplete: false,
        }
    }

    pub fn syntax(message: impl Into<String>, span: Span) -> Self {
        Error::new(ErrorKind::Syntax, message).at(span)
    }

    pub fn type_error(message: impl Into<String>) -> Self {
        Error::new(ErrorKind::Type, message)
    }

    pub fn runtime(message: impl Into<String>) -> Self {
        Error::new(ErrorKind::Runtime, message)
    }

    pub fn io(message: impl Into<String>) -> Self {
        Error::new(ErrorKind::Io, message)
    }

    /// Sets the location of the error.
    pub fn at(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }

    /// Sets the location only if the error does not have one yet, so the
    /// innermost (most precise) location wins.
    pub fn or_at(mut self, span: Span) -> Self {
        if self.span.is_none() {
            self.span = Some(span);
        }
        self
    }

    /// Renders the error with the offending source line and a caret:
    ///
    /// ```text
    /// error[runtime]: division by zero
    ///  --> main.belsk2:3:12
    ///   |
    /// 3 | var x = 10 / 0;
    ///   |            ^
    /// ```
    pub fn render(&self, source: &str, file: Option<&str>) -> String {
        let mut out = format!("{}: {}", self.kind.name(), self.message);
        let Some(span) = self.span else {
            if let Some(file) = file {
                out.push_str(&format!("\n --> {file}"));
            }
            return out;
        };
        let file = file.unwrap_or("<input>");
        out.push_str(&format!("\n --> {file}:{span}"));
        if let Some(line) = source.lines().nth(span.line.saturating_sub(1)) {
            let num = span.line.to_string();
            let pad = " ".repeat(num.len());
            let caret_pad: String = line
                .chars()
                .take(span.col.saturating_sub(1))
                .map(|c| if c == '\t' { '\t' } else { ' ' })
                .collect();
            out.push_str(&format!("\n{pad} |\n{num} | {line}\n{pad} | {caret_pad}^"));
        }
        out
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.span {
            Some(span) => write!(f, "line {span}: {}: {}", self.kind.name(), self.message),
            None => write!(f, "{}: {}", self.kind.name(), self.message),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::io(e.to_string())
    }
}
