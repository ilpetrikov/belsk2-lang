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
use std::rc::Rc;

pub use belsk2_project::wit;
pub use belsk2_project::{
    create, display_path, find_root, load, load_file, load_file_lenient, load_lenient, load_with,
    module_name, ComponentInfo, Dependency, Loaded, Lockfile, Manifest, Mode, Module, ModuleId,
    Options, Package, PackageInfo, Template, WitFunc, WitInterface, WitTy, WitTypeKind, World,
    LOCKFILE,
    MANIFEST,
};
use belsk2_syntax::Ty;
use belsk2_typeck::Checker;

use crate::{Array, Error, Interpreter, Native, Result, StructValue, Value};

/// Type-checks every loaded module, each after the modules it uses, and
/// returns all errors.
pub fn check(loaded: &mut Loaded) -> Vec<Error> {
    let mut errors = merge_partial_files(loaded);
    let mut exports: Vec<Option<Vec<(String, Ty)>>> = vec![None; loaded.modules.len()];
    let names: Vec<String> = loaded.modules.iter().map(|m| m.name.clone()).collect();

    for &id in &loaded.order {
        let Some(module) = loaded.modules.get_mut(id) else {
            continue;
        };
        // An interface of the WIT world: its functions come from the host.
        if let (Some(world), Some(iface)) = (&loaded.world, &module.interface) {
            let items = world
                .interface_imports(&iface.module)
                .map(|f| (f.name.clone(), f.ty()))
                .collect();
            if let Some(slot) = exports.get_mut(id) {
                *slot = Some(items);
            }
            continue;
        }
        let mut checker = Checker::new();
        // Functions the WIT world imports are visible in the whole project.
        if let (Some(world), 0) = (&loaded.world, module.package) {
            for f in world.root_imports() {
                checker.declare_import(&f.name, f.ty(), "the component world");
            }
        }
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
        if let (Some(world), true) = (&loaded.world, Some(id) == loaded.wit_types) {
            world.bind_types(&module.program);
        }
        // A component starts in the functions its WIT world exports.
        let needs_main = loaded.world.is_none();
        if needs_main && Some(id) == loaded.entry && !items.iter().any(|(name, _)| name == "main") {
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

/// The parts of a `partial` type may be in several files of a package.
/// Such files are joined into one module (the first of them in the
/// checking order): the type gets the members of every part, and the
/// files see each other's declarations and the modules each one uses.
/// The other files stay as empty modules, and every `using` of them
/// means the joined module.
pub fn merge_partial_files(loaded: &mut Loaded) -> Vec<Error> {
    use belsk2_syntax::ast::StmtKind;
    let mut errors = Vec::new();
    // Partial types by (package, name): the modules that declare parts.
    let mut parts: HashMap<(usize, String), Vec<ModuleId>> = HashMap::new();
    for (id, m) in loaded.modules.iter().enumerate() {
        for s in &m.program.stmts {
            if let StmtKind::Type(d) = &s.kind {
                if d.partial {
                    let ids = parts.entry((m.package, d.name.clone())).or_default();
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
            }
        }
    }
    // Files that share partial types, joined into groups.
    let n = loaded.modules.len();
    let mut group: Vec<usize> = (0..n).collect();
    fn root(group: &[usize], mut x: usize) -> usize {
        let mut steps = 0;
        while let Some(&p) = group.get(x) {
            if p == x || steps > group.len() {
                break;
            }
            x = p;
            steps += 1;
        }
        x
    }
    let mut any = false;
    for ids in parts.values() {
        let Some((&first, rest)) = ids.split_first() else {
            continue;
        };
        for &other in rest {
            any = true;
            let (a, b) = (root(&group, first), root(&group, other));
            if let Some(slot) = group.get_mut(b) {
                *slot = a;
            }
        }
    }
    if !any {
        return errors;
    }
    let position = |id: ModuleId| {
        loaded
            .order
            .iter()
            .position(|&m| m == id)
            .unwrap_or(usize::MAX)
    };
    // Each group joins into its member that is checked first.
    let roots: Vec<usize> = (0..n).map(|m| root(&group, m)).collect();
    let mut owner: Vec<ModuleId> = (0..n).collect();
    for id in 0..n {
        let r = roots.get(id).copied();
        let members = (0..n).filter(|&m| roots.get(m).copied() == r);
        if let Some(first) = members.min_by_key(|&m| position(m)) {
            if let Some(slot) = owner.get_mut(id) {
                *slot = first;
            }
        }
    }
    for id in 0..n {
        let Some(&to) = owner.get(id) else { continue };
        if to == id {
            continue;
        }
        let (stmts, uses) = match loaded.modules.get_mut(id) {
            Some(m) => (
                std::mem::take(&mut m.program.stmts),
                std::mem::take(&mut m.uses),
            ),
            None => continue,
        };
        if let Some(target) = loaded.modules.get_mut(to) {
            target.program.stmts.extend(stmts);
            target.uses.extend(uses);
        }
    }
    // Every `using` of a joined file means its group's module.
    for (id, m) in loaded.modules.iter_mut().enumerate() {
        let mut seen = Vec::new();
        m.uses.retain_mut(|(u, _)| {
            *u = owner.get(*u).copied().unwrap_or(*u);
            let keep = *u != id && !seen.contains(u);
            seen.push(*u);
            keep
        });
    }
    if let Some(e) = loaded.entry {
        loaded.entry = owner.get(e).copied();
    }
    // Check the modules again in an order where each comes after the
    // modules it uses.
    let mut placed = vec![false; n];
    let mut order = Vec::with_capacity(n);
    while order.len() < n {
        let before = order.len();
        for &id in &loaded.order {
            let ready = !placed.get(id).copied().unwrap_or(true)
                && loaded.modules.get(id).is_some_and(|m| {
                    m.uses
                        .iter()
                        .all(|(u, _)| placed.get(*u).copied().unwrap_or(true))
                });
            if ready {
                if let Some(p) = placed.get_mut(id) {
                    *p = true;
                }
                order.push(id);
            }
        }
        if order.len() == before {
            let first = loaded
                .order
                .iter()
                .find(|&&id| !placed.get(id).copied().unwrap_or(true));
            let name = first
                .and_then(|&id| loaded.modules.get(id))
                .map(|m| m.name.clone())
                .unwrap_or_default();
            errors.push(Error::project(format!(
                "the files that share partial types use each other in a cycle (through module '{name}')"
            )));
            for &id in &loaded.order {
                if !placed.get(id).copied().unwrap_or(true) {
                    order.push(id);
                }
            }
            break;
        }
    }
    loaded.order = order;
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
    let modules = run_modules(loaded, interp, out)?;
    let main = loaded
        .entry
        .and_then(|e| modules.get(e))
        .and_then(|m| m.as_ref())
        .and_then(|items| items.iter().find(|(name, ..)| name == "main"))
        .map(|(_, v, _)| v.clone());
    match main {
        Some(main) => interp.call_main(&main, args, out),
        None => Err(Error::runtime("the program has no `fn main()`")),
    }
}

/// Runs a checked component project in the interpreter, as `belsk2 run`
/// runs it in wasmtime: calls `name` (default: `run`, else `main`) with
/// `args` parsed by the parameter types and returns its result. A
/// function of another module is named `module.function`. Functions the
/// WIT world imports are stubs (see [`run`]).
pub fn call(
    loaded: &Loaded,
    interp: &mut Interpreter,
    name: Option<&str>,
    args: &[String],
    out: &mut dyn Write,
) -> Result<Value> {
    let modules = run_modules(loaded, interp, out)?;
    let exports_of = |id: Option<ModuleId>| {
        id.and_then(|id| modules.get(id))
            .and_then(|m| m.clone())
            .unwrap_or_default()
    };
    let functions = |items: &[(String, Value, Ty)]| -> Vec<(String, Value, Ty)> {
        items
            .iter()
            .filter(|(_, v, _)| matches!(v, Value::Function(_) | Value::Native(_)))
            .cloned()
            .collect()
    };
    let entry = functions(&exports_of(loaded.entry));
    let found = match name {
        Some(name) => match name.rsplit_once('.') {
            Some((module, f)) => {
                let id = loaded.modules.iter().position(|m| m.name == module);
                functions(&exports_of(id))
                    .into_iter()
                    .find(|(n, ..)| n == f)
            }
            None => entry.iter().find(|(n, ..)| n == name).cloned(),
        },
        None => ["run", "main"]
            .iter()
            .find_map(|w| entry.iter().find(|(n, ..)| n == w).cloned()),
    };
    let Some((fname, f, ty)) = found else {
        let names: Vec<&str> = entry.iter().map(|(n, ..)| n.as_str()).collect();
        let what = match name {
            Some(n) => format!("there is no function '{n}'"),
            None => "the program has neither `fn run()` nor `fn main()`".to_string(),
        };
        return Err(Error::runtime(format!(
            "{what}; call one of [{}] with `belsk2 run --interpreted --call <name> -- <args>`",
            names.join(", ")
        )));
    };
    let params = match &ty {
        Ty::Fn(Some(sig)) => sig.params.clone(),
        _ => vec![Ty::Any; args.len()],
    };
    if params.len() != args.len() {
        return Err(Error::runtime(format!(
            "'{fname}' takes {} argument(s), got {}; pass them after `--`",
            params.len(),
            args.len()
        )));
    }
    let values = params
        .iter()
        .zip(args)
        .enumerate()
        .map(|(i, (t, a))| {
            parse_arg(a, t).map_err(|e| Error::runtime(format!("argument {}: {e}", i + 1)))
        })
        .collect::<Result<Vec<Value>>>()?;
    interp.call_value(&f, values, out)
}

/// Parses a command-line argument as a value of type `t` (lists as
/// `1,2,3`).
fn parse_arg(text: &str, t: &Ty) -> std::result::Result<Value, String> {
    let bad = || format!("\"{text}\" is not a valid {t}");
    Ok(match t.canonical() {
        Ty::Any | Ty::String => Value::String(text.to_string()),
        Ty::Bool => Value::Bool(text.parse().map_err(|_| bad())?),
        Ty::Int(k) => {
            let v: i128 = text.trim().parse().map_err(|_| bad())?;
            if !k.fits(v) {
                return Err(bad());
            }
            Value::Int(v, k)
        }
        Ty::Float(k) => Value::Float(k.round(text.trim().parse().map_err(|_| bad())?), k),
        Ty::Char => {
            let mut chars = text.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Value::Char(c),
                _ => return Err(bad()),
            }
        }
        Ty::Array(elem) => {
            let items = if text.is_empty() {
                Vec::new()
            } else {
                text.split(',')
                    .map(|item| parse_arg(item.trim(), &elem))
                    .collect::<std::result::Result<_, _>>()?
            };
            Value::Array(Array::typed((*elem).clone(), items))
        }
        other => return Err(format!("{other} arguments are not supported")),
    })
}

/// The top-level declarations of a module: name, value and type.
type Exports = Vec<(String, Value, Ty)>;

/// Runs every module the entry point needs, in order; returns the
/// declarations of each.
fn run_modules(
    loaded: &Loaded,
    interp: &mut Interpreter,
    out: &mut dyn Write,
) -> Result<Vec<Option<Exports>>> {
    let mut values: Vec<Option<Vec<(String, Value, Ty)>>> = vec![None; loaded.modules.len()];
    // There is no host in the interpreter: functions the WIT world imports
    // are stubs that print the call and return a default value. They are
    // made once the types of the world are defined.
    let mut world_imports: Option<Vec<(String, Value, Ty)>> = None;
    for id in loaded.run_order() {
        let Some(module) = loaded.modules.get(id) else {
            continue;
        };
        if let (Some(world), Some(iface)) = (&loaded.world, &module.interface) {
            let short = short_name(&iface.key);
            let stubs = world
                .interface_imports(&iface.module)
                .map(|f| {
                    let shown = format!("{short}.{}", f.wit_name);
                    (f.name.clone(), stub(f, shown, interp), f.ty())
                })
                .collect();
            if let Some(slot) = values.get_mut(id) {
                *slot = Some(stubs);
            }
            continue;
        }
        let mut imports = Vec::new();
        if module.package == 0 && Some(id) != loaded.wit_types {
            let roots = world_imports.get_or_insert_with(|| {
                loaded
                    .world
                    .iter()
                    .flat_map(|w| w.root_imports())
                    .map(|f| (f.name.clone(), stub(f, f.wit_name.clone(), interp), f.ty()))
                    .collect()
            });
            imports.extend(roots.iter().cloned());
        }
        for &(used, _) in &module.uses {
            if let Some(Some(items)) = values.get(used) {
                imports.extend(items.iter().cloned());
            }
        }
        let exports = interp.run_module(&module.program, &imports, out)?;
        if let Some(slot) = values.get_mut(id) {
            *slot = Some(exports);
        }
    }
    Ok(values)
}

/// `demo:host/canvas@0.1.0` is shown as `canvas`.
fn short_name(key: &str) -> &str {
    let name = key.rsplit('/').next().unwrap_or(key);
    name.split('@').next().unwrap_or(name)
}

/// A stand-in for a function the host provides: prints
/// `[host] name(args)` and returns the default value of its result type.
fn stub(f: &WitFunc, shown: String, interp: &Interpreter) -> Value {
    let result = f
        .result
        .as_ref()
        .map(|t| default(t, interp))
        .unwrap_or(Value::Null);
    Value::Native(Rc::new(Native {
        name: f.name.clone(),
        ty: f.ty(),
        func: Box::new(move |args, out| {
            let args: Vec<String> = args.iter().map(show).collect();
            writeln!(out, "[host] {shown}({})", args.join(", "))?;
            Ok(result.clone())
        }),
    }))
}

/// The value a stub returns: zeros, empty strings and lists, the first
/// case of an enum.
fn default(t: &WitTy, interp: &Interpreter) -> Value {
    match t {
        WitTy::Bool => Value::Bool(false),
        WitTy::Int(k) => Value::Int(0, *k),
        WitTy::Float(k) => Value::Float(0.0, *k),
        WitTy::Char => Value::Char(' '),
        WitTy::String => Value::String(String::new()),
        WitTy::List(elem) => Value::Array(Array::typed(elem.ty(), Vec::new())),
        WitTy::Named(named) => {
            let class = match t.ty() {
                Ty::Named(r) => interp.class_of(r.id()),
                _ => None,
            };
            let Some(class) = class else {
                return Value::Null;
            };
            match &named.kind {
                WitTypeKind::Record(fields) => Value::Struct(Rc::new(StructValue {
                    class,
                    fields: fields.iter().map(|(_, t)| default(t, interp)).collect(),
                })),
                WitTypeKind::Enum(_) => Value::Enum(class, 0, Rc::new(Vec::new())),
                WitTypeKind::Variant(cases) => {
                    let data = cases
                        .first()
                        .and_then(|(_, payload)| payload.as_ref())
                        .map(|(_, t)| vec![default(t, interp)])
                        .unwrap_or_default();
                    Value::Enum(class, 0, Rc::new(data))
                }
            }
        }
    }
}

/// An argument as the wasmtime runner prints it: strings and chars quoted.
fn show(v: &Value) -> String {
    match v {
        Value::String(s) => format!("{s:?}"),
        Value::Char(c) => format!("{c:?}"),
        Value::Array(a) => {
            let items: Vec<String> = a
                .borrow()
                .map(|items| items.iter().map(show).collect())
                .unwrap_or_default();
            format!("[{}]", items.join(", "))
        }
        other => other.to_string(),
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
