//! The Belsk2 programming language.
//!
//! This crate is the main entry point. It re-exports the front end
//! ([`syntax`]), the static checker ([`typeck`]) and the interpreter
//! ([`Interpreter`], [`Value`]).
//!
//! ```
//! let mut out = Vec::new();
//! belsk2::run_source_with_writer("prinb(1 + 2);", &mut out).unwrap();
//! assert_eq!(String::from_utf8(out).unwrap(), "3\n");
//! ```

use std::io::Write;
use std::path::Path;

pub use belsk2_interp::{
    format_number, Array, Error, ErrorKind, Function, Input, Interpreter, Result, Span, Ty, Value,
    BUILTINS, DEFAULT_MAX_CALL_DEPTH,
};
pub use belsk2_syntax as syntax;
pub use belsk2_typeck as typeck;

/// The language version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Runs a program, printing to stdout.
pub fn run_source(source: &str) -> Result<()> {
    Interpreter::new().run_source(source)
}

/// Runs a program, printing to `out`.
pub fn run_source_with_writer(source: &str, out: &mut dyn Write) -> Result<()> {
    Interpreter::new().run_source_with_writer(source, out)
}

/// Reads and runs a file, printing to stdout.
pub fn run_file(path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    let source = std::fs::read_to_string(path)
        .map_err(|e| Error::io(format!("cannot read {}: {e}", path.display())))?;
    run_source(&source)
}

/// Parses and statically checks a program without running it.
/// Returns every error found.
pub fn compile(source: &str) -> std::result::Result<syntax::ast::Program, Vec<Error>> {
    Interpreter::new().compile(source)
}
