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
mod wasi;

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
pub fn compile(program: &Program, target: Target) -> Result<Vec<u8>, Vec<Error>> {
    compile_units(
        &[Unit {
            name: "main".to_string(),
            interface: false,
            program,
            uses: Vec::new(),
        }],
        Some(0),
        None,
        target,
    )
}

/// What `belsk2 build` produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Target {
    /// A plain WebAssembly module (a component with `[component]`) with no
    /// input or output.
    #[default]
    Wasm32,
    /// A WASI 0.2 component: `prinb` and `input` use stdout and stdin, and
    /// `main` is exported as `wasi:cli/run`.
    Wasi02,
}

impl Target {
    pub const ALL: [Target; 2] = [Target::Wasm32, Target::Wasi02];

    pub fn name(self) -> &'static str {
        match self {
            Target::Wasm32 => "wasm32",
            Target::Wasi02 => "wasi0.2",
        }
    }

    pub fn parse(name: &str) -> Option<Target> {
        match name {
            "wasm32" => Some(Target::Wasm32),
            "wasi0.2" | "wasip2" => Some(Target::Wasi02),
            _ => None,
        }
    }
}

/// Compiles checked modules into one WebAssembly module that exports the
/// functions of unit `exported` (if any). Units must come after the units
/// they use.
///
/// With a WIT `world` or the wasi0.2 target, the result is a component
/// instead: it imports and exports exactly what the world says.
pub fn compile_units(
    units: &[Unit],
    exported: Option<usize>,
    world: Option<&World>,
    target: Target,
) -> Result<Vec<u8>, Vec<Error>> {
    let wasi = target == Target::Wasi02;
    // The id bank comes first: its globals are set before any `idb`.
    let bank_program = if wasi { Some(bank()?) } else { None };
    let mut all: Vec<Unit> = Vec::with_capacity(units.len() + 1);
    let shift = usize::from(bank_program.is_some());
    if let Some(program) = &bank_program {
        all.push(Unit {
            name: "#bank".to_string(),
            interface: false,
            program,
            uses: Vec::new(),
        });
    }
    all.extend(units.iter().map(|u| Unit {
        name: u.name.clone(),
        interface: u.interface,
        program: u.program,
        uses: u.uses.iter().map(|&i| i + shift).collect(),
    }));
    let units = all.as_slice();
    let exported = exported.map(|e| e + shift);
    let mut ctx = codegen::Ctx::default();
    if world.is_some() || wasi {
        component::imports(&mut ctx, world, wasi);
    }
    codegen::declare(&mut ctx, units);
    if bank_program.is_some() {
        if let (Some(set), Some(get)) = (ctx.unit_fn(0, "bankSet"), ctx.unit_fn(0, "bankGet")) {
            ctx.bank = Some((0, set, get));
        }
    }
    if let Some(world) = world {
        component::exports(&mut ctx, world, units, exported);
    }
    let main = exported.and_then(|u| ctx.unit_fn(u, "main").map(|d| (u, d)));
    if wasi {
        match &main {
            Some((unit, decl)) => {
                if let Err(e) = wasi::run_export(&mut ctx, decl, *unit) {
                    ctx.errors.push(e);
                }
            }
            None if world.is_none() => ctx.errors.push(Error::compile(
                "the wasi0.2 target runs `fn main()`, and there is none",
            )),
            None => {}
        }
    } else if world.is_none() {
        if let Some(unit) = exported {
            codegen::export_unit(&mut ctx, units, unit);
        }
    }
    let core = validate(codegen::finish(ctx)?)?;
    if wasi {
        let (resolve, id) = wasi::world(world, main.is_some())?;
        return component::encode(core, &resolve, id);
    }
    match world {
        Some(world) => component::encode(core, &world.resolve, world.id),
        None => Ok(core),
    }
}

/// Compiles a loaded and checked project. The functions of its entry
/// module (`main.belsk2`), or of `lib.belsk2` for a library, are exported;
/// a project with `[component]` becomes a component of its WIT world.
pub fn compile_project(loaded: &Loaded, target: Target) -> Result<Vec<u8>, Vec<Error>> {
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
    compile_units(&units, exported, loaded.world.as_deref(), target)
}

/// The checked id bank of the wasi0.2 target.
fn bank() -> Result<Program, Vec<Error>> {
    let mut program = belsk2_syntax::parse(wasi::BANK).map_err(|e| vec![e])?;
    let errors = belsk2_typeck::check(&mut program);
    if errors.is_empty() {
        Ok(program)
    } else {
        Err(errors)
    }
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
    compile(&program, Target::Wasm32)
}
