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
//! Supported: numbers, `bool`, `char`, strings, arrays, functions (generic
//! ones by monomorphization), local and global variables, `if`, `while`,
//! `for`, `break`, `continue`. Dictionaries, `any` and formatting of
//! floating-point numbers come later (see ROADMAP.md).

mod codegen;
mod helpers;
mod types;

use std::collections::HashMap;

use belsk2_project::{Loaded, ModuleId};
use belsk2_syntax::ast::Program;
use belsk2_syntax::Error;

/// One module of a multi-file program.
pub struct Unit<'a> {
    pub program: &'a Program,
    /// The units this one imports with `using` (indices into the list).
    pub uses: Vec<usize>,
}

/// Compiles a program that has already been checked (types filled in).
/// Returns every error found.
pub fn compile(program: &Program) -> Result<Vec<u8>, Vec<Error>> {
    compile_units(
        &[Unit {
            program,
            uses: Vec::new(),
        }],
        0,
    )
}

/// Compiles checked modules into one WebAssembly module that exports the
/// functions of unit `exported`. Units must come after the units they use.
pub fn compile_units(units: &[Unit], exported: usize) -> Result<Vec<u8>, Vec<Error>> {
    let mut ctx = codegen::Ctx::default();
    codegen::declare(&mut ctx, units);
    codegen::export_unit(&mut ctx, units, exported);
    validate(codegen::finish(ctx)?)
}

/// Compiles a loaded and checked project. The functions of its entry
/// module (`main.belsk2`), or of `lib.belsk2` for a library, are exported.
pub fn compile_project(loaded: &Loaded) -> Result<Vec<u8>, Vec<Error>> {
    let order = if loaded.entry.is_some() {
        loaded.run_order()
    } else {
        loaded.order.clone()
    };
    let exported = loaded.entry.or_else(|| {
        loaded
            .modules
            .iter()
            .position(|m| m.package == 0 && m.name == "lib")
    });
    let positions: HashMap<ModuleId, usize> =
        order.iter().enumerate().map(|(i, &m)| (m, i)).collect();
    let Some(&exported) = exported.and_then(|m| positions.get(&m)) else {
        return Err(vec![Error::project(
            "nothing to build: the project has neither src/main.belsk2 nor src/lib.belsk2",
        )]);
    };
    let units: Vec<Unit> = order
        .iter()
        .filter_map(|&m| loaded.modules.get(m))
        .map(|m| Unit {
            program: &m.program,
            uses: m
                .uses
                .iter()
                .filter_map(|(u, _)| positions.get(u).copied())
                .collect(),
        })
        .collect();
    compile_units(&units, exported)
}

fn validate(bytes: Vec<u8>) -> Result<Vec<u8>, Vec<Error>> {
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
