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
mod component;
mod helpers;
mod types;

use std::collections::HashMap;

use belsk2_project::{Loaded, ModuleId, World};
use belsk2_syntax::ast::Program;
use belsk2_syntax::Error;

/// One module of a multi-file program.
pub struct Unit<'a> {
    /// The module name (`main`, `utils`, `net.http`).
    pub name: String,
    /// Whether this is an interface imported by the WIT world (no source;
    /// the host provides its functions).
    pub interface: bool,
    pub program: &'a Program,
    /// The units this one imports with `using` (indices into the list).
    pub uses: Vec<usize>,
}

/// Compiles a program that has already been checked (types filled in).
/// Returns every error found.
pub fn compile(program: &Program) -> Result<Vec<u8>, Vec<Error>> {
    compile_units(
        &[Unit {
            name: "main".to_string(),
            interface: false,
            program,
            uses: Vec::new(),
        }],
        Some(0),
        None,
    )
}

/// Compiles checked modules into one WebAssembly module that exports the
/// functions of unit `exported` (if any). Units must come after the units
/// they use.
///
/// With a WIT `world`, the result is a component instead: it imports and
/// exports exactly what the world says.
pub fn compile_units(
    units: &[Unit],
    exported: Option<usize>,
    world: Option<&World>,
) -> Result<Vec<u8>, Vec<Error>> {
    let mut ctx = codegen::Ctx::default();
    if let Some(world) = world {
        component::imports(&mut ctx, world);
    }
    codegen::declare(&mut ctx, units);
    match world {
        Some(world) => component::exports(&mut ctx, world, units, exported),
        None => {
            if let Some(unit) = exported {
                codegen::export_unit(&mut ctx, units, unit);
            }
        }
    }
    let core = validate(codegen::finish(ctx)?)?;
    match world {
        Some(world) => component::encode(core, world),
        None => Ok(core),
    }
}

/// Compiles a loaded and checked project. The functions of its entry
/// module (`main.belsk2`), or of `lib.belsk2` for a library, are exported;
/// a project with `[component]` becomes a component of its WIT world.
pub fn compile_project(loaded: &Loaded) -> Result<Vec<u8>, Vec<Error>> {
    // A component also exports interfaces from modules `main` does not use.
    let order = if loaded.entry.is_some() && loaded.world.is_none() {
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
    let exported = exported.and_then(|m| positions.get(&m)).copied();
    if exported.is_none() && loaded.world.is_none() {
        return Err(vec![Error::project(
            "nothing to build: the project has neither src/main.belsk2 nor src/lib.belsk2",
        )]);
    }
    let units: Vec<Unit> = order
        .iter()
        .filter_map(|&m| loaded.modules.get(m))
        .map(|m| Unit {
            name: m.name.clone(),
            interface: m.interface.is_some(),
            program: &m.program,
            uses: m
                .uses
                .iter()
                .filter_map(|(u, _)| positions.get(u).copied())
                .collect(),
        })
        .collect();
    compile_units(&units, exported, loaded.world.as_deref())
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
