//! WebAssembly backend for Belsk2.
//!
//! Compiles a checked program to a WebAssembly module (with the GC
//! proposal, for strings and arrays). WASM builds have no I/O: `prinb`,
//! `input`, `reab` and `idb` are compile errors. Every top-level function
//! is exported under its own name; top-level variables become globals,
//! initialized by the module's start function.
//!
//! ```
//! let wasm = belsk2_wasm::compile_source("fn add(a: int, b: int): int { return a + b; }");
//! assert!(wasm.is_ok());
//! ```
//!
//! Supported so far: numbers, `bool`, `char`, functions, local and global
//! variables, `if`, `while`, `break`, `continue`. Strings, arrays, generics
//! and dictionaries come in later stages (see ROADMAP.md).

mod codegen;
mod helpers;

use belsk2_syntax::ast::Program;
use belsk2_syntax::Error;

/// Compiles a program that has already been checked (types filled in).
/// Returns every error found.
pub fn compile(program: &Program) -> Result<Vec<u8>, Vec<Error>> {
    let bytes = codegen::compile(program)?;
    wasmparser::validate(&bytes).map_err(|e| {
        vec![Error::compile(format!(
            "internal compiler error: produced an invalid module: {e}"
        ))]
    })?;
    Ok(bytes)
}

/// Parses, checks and compiles source code.
pub fn compile_source(source: &str) -> Result<Vec<u8>, Vec<Error>> {
    let mut program = belsk2_syntax::parse(source).map_err(|e| vec![e])?;
    let errors = belsk2_typeck::check(&mut program);
    if !errors.is_empty() {
        return Err(errors);
    }
    compile(&program)
}
