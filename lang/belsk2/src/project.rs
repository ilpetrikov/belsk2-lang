//! Checking and running multi-file programs and projects.
//!
//! ```no_run
//! use belsk2::project;
//!
//! let mut loaded = project::load(std::path::Path::new("my-app"), project::Mode::Run)
//!     .map_err(|(errors, _)| format!("{errors:?}"))?;
//! let errors = project::check(&mut loaded);
//! assert!(errors.is_empty());
//! project::run(&loaded, &mut belsk2::Interpreter::new(), &[], &mut std::io::stdout())?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::collections::HashMap;
use std::io::Write;

pub use belsk2_project::{
    create, find_root, load, load_file, load_with, module_name, Dependency, Loaded, Lockfile,
    Manifest, Mode, Module, ModuleId, Options, Package, PackageInfo, Template, LOCKFILE, MANIFEST,
};
use belsk2_syntax::Ty;
use belsk2_typeck::Checker;

use crate::{Error, Interpreter, Result, Value};

/// Type-checks every loaded module, each after the modules it uses, and
/// returns all errors.
pub fn check(loaded: &mut Loaded) -> Vec<Error> {
    let mut errors = Vec::new();
    let mut exports: Vec<Option<Vec<(String, Ty)>>> = vec![None; loaded.modules.len()];
    let names: Vec<String> = loaded.modules.iter().map(|m| m.name.clone()).collect();

    for &id in &loaded.order {
        let Some(module) = loaded.modules.get_mut(id) else {
            continue;
        };
        let mut checker = Checker::new();
        let mut imported: HashMap<String, ModuleId> = HashMap::new();
        for &(used, span) in &module.uses {
            let Some(Some(items)) = exports.get(used) else {
                continue;
            };
            let from = names.get(used).cloned().unwrap_or_default();
            for (name, ty) in items {
                if let Some(&other) = imported.get(name) {
                    let other = names.get(other).cloned().unwrap_or_default();
                    errors.push(
                        Error::type_error(format!(
                            "'{name}' is declared in both '{other}' and '{from}'"
                        ))
                        .at(span),
                    );
                    continue;
                }
                imported.insert(name.clone(), used);
                checker.declare_import(name, ty.clone(), &from);
            }
        }
        let (errs, items) = checker.check_module(&mut module.program);
        errors.extend(errs);
        if Some(id) == loaded.entry && !items.iter().any(|(name, _)| name == "main") {
            errors.push(
                Error::type_error(format!(
                    "{} has no `fn main()`; a program starts in `main`",
                    loaded
                        .sources
                        .get(module.file)
                        .map(|f| f.name.as_str())
                        .unwrap_or("the entry file")
                ))
                .at(belsk2_syntax::Span::in_file(module.file, 1, 1)),
            );
        }
        if let Some(slot) = exports.get_mut(id) {
            *slot = Some(items);
        }
    }
    errors
}

/// Runs a checked program: every module the entry point needs, in order,
/// then the entry point, then its `main` with `args`. Returns the exit
/// code if `main` returns an `int`.
pub fn run(
    loaded: &Loaded,
    interp: &mut Interpreter,
    args: &[String],
    out: &mut dyn Write,
) -> Result<Option<i64>> {
    let mut main = None;
    let mut values: Vec<Option<Vec<(String, Value, Ty)>>> = vec![None; loaded.modules.len()];
    for id in loaded.run_order() {
        let Some(module) = loaded.modules.get(id) else {
            continue;
        };
        let mut imports = Vec::new();
        for &(used, _) in &module.uses {
            if let Some(Some(items)) = values.get(used) {
                imports.extend(items.iter().cloned());
            }
        }
        let exports = interp.run_module(&module.program, &imports, out)?;
        if Some(id) == loaded.entry {
            main = exports
                .iter()
                .find(|(name, ..)| name == "main")
                .map(|(_, v, _)| v.clone());
        }
        if let Some(slot) = values.get_mut(id) {
            *slot = Some(exports);
        }
    }
    match main {
        Some(main) => interp.call_main(&main, args, out),
        None => Err(Error::runtime("the program has no `fn main()`")),
    }
}

/// Number of modules and a summary line, for `belsk2 build`.
pub fn summary(loaded: &Loaded) -> String {
    match loaded.root_package() {
        Some(p) => format!(
            "{} v{} ({} module{})",
            p.name,
            p.version,
            loaded.modules.len(),
            if loaded.modules.len() == 1 { "" } else { "s" }
        ),
        None => String::new(),
    }
}
