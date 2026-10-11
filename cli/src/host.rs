//! `belsk2 run` for component projects: runs the component in wasmtime.
//!
//! WASI 0.2 imports (stdout, stdin, ...) are provided by wasmtime-wasi.
//! There is no real host for the other imports (the application that would
//! embed the component), so each is a stub that prints the call and
//! returns a default value (`0`, `false`, `""`, an empty list, ...).

use wasmtime::component::types::{ComponentFunc, ComponentItem};
use wasmtime::component::{Component, Linker, LinkerInstance, ResourceTable, Type, Val};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

struct Host {
    wasi: WasiCtx,
    table: ResourceTable,
}

impl WasiView for Host {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

/// The export of a WASI command.
const WASI_RUN: &str = "wasi:cli/run@0.2.0";

/// Runs export `call` (default: the WASI `run`, or `run` or `main`) with
/// `args` parsed by its parameter types, and prints what it returns.
pub fn run(wasm: &[u8], call: Option<&str>, args: &[String]) -> Result<(), String> {
    let err = |e: wasmtime::Error| format!("{e:#}");
    let mut config = Config::new();
    config.wasm_gc(true);
    config.wasm_component_model(true);
    let engine = Engine::new(&config).map_err(err)?;
    let component = Component::new(&engine, wasm).map_err(err)?;
    let ty = component.component_type();

    let mut linker: Linker<Host> = Linker::new(&engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker).map_err(err)?;
    for (name, item) in ty.imports(&engine) {
        if name.starts_with("wasi:") {
            continue;
        }
        match item.ty {
            ComponentItem::ComponentFunc(_) => stub(&mut linker.root(), name, name.to_string())?,
            ComponentItem::ComponentInstance(instance) => {
                let short = short_name(name);
                let mut li = linker.instance(name).map_err(err)?;
                for (fname, fitem) in instance.exports(&engine) {
                    if let ComponentItem::ComponentFunc(_) = fitem.ty {
                        stub(&mut li, fname, format!("{short}.{fname}"))?;
                    }
                }
            }
            _ => {
                return Err(format!(
                    "the component imports '{name}', which `belsk2 run` cannot provide"
                ))
            }
        }
    }

    // Exported functions: `name` for the world's own, `interface.name`
    // for those of exported interfaces.
    let mut exports: Vec<(String, String, Option<String>)> = Vec::new();
    for (name, item) in ty.exports(&engine) {
        match item.ty {
            ComponentItem::ComponentFunc(_) => {
                exports.push((name.to_string(), name.to_string(), None));
            }
            ComponentItem::ComponentInstance(instance) => {
                for (fname, fitem) in instance.exports(&engine) {
                    if let ComponentItem::ComponentFunc(_) = fitem.ty {
                        let shown = format!("{}.{fname}", short_name(name));
                        exports.push((shown, fname.to_string(), Some(name.to_string())));
                    }
                }
            }
            _ => {}
        }
    }
    let wanted: Vec<&str> = match call {
        Some(c) => vec![c],
        None => vec!["run.run", "run", "main"],
    };
    let chosen = wanted.iter().find_map(|w| {
        exports
            .iter()
            .find(|(shown, ..)| shown == w || shown.replace('-', "_") == *w)
    });
    let Some((shown, fname, iface)) = chosen.cloned() else {
        let names: Vec<&str> = exports.iter().map(|(s, ..)| s.as_str()).collect();
        let what = match call {
            Some(c) => format!("the component exports no function '{c}'"),
            None => "the component exports neither `run` nor `main`".to_string(),
        };
        return Err(format!(
            "{what}; call one of [{}] with `belsk2 run --call <name> -- <args>`",
            names.join(", ")
        ));
    };

    let wasi_run = iface.as_deref() == Some(WASI_RUN);
    // A WASI command gets the arguments; for other exports they are the
    // function's parameters.
    let mut wasi = WasiCtx::builder();
    wasi.inherit_stdio();
    if wasi_run {
        let mut argv = vec!["main".to_string()];
        argv.extend(args.iter().cloned());
        wasi.args(&argv);
    }
    let host = Host {
        wasi: wasi.build(),
        table: ResourceTable::new(),
    };
    let mut store = Store::new(&engine, host);
    let instance = linker.instantiate(&mut store, &component).map_err(err)?;
    let parent = match &iface {
        Some(i) => Some(
            instance
                .get_export_index(&mut store, None, i)
                .ok_or_else(|| format!("export '{i}' not found"))?,
        ),
        None => None,
    };
    let index = instance
        .get_export_index(&mut store, parent.as_ref(), &fname)
        .ok_or_else(|| format!("export '{shown}' not found"))?;
    let func = instance
        .get_func(&mut store, index)
        .ok_or_else(|| format!("'{shown}' is not a function"))?;
    let fty = func.ty(&store);
    let params: Vec<(String, Type)> = fty.params().map(|(n, t)| (n.to_string(), t)).collect();
    let args = if wasi_run { &[] } else { args };
    if params.len() != args.len() {
        let names: Vec<&str> = params.iter().map(|(n, _)| n.as_str()).collect();
        return Err(format!(
            "'{shown}' takes {} argument(s) ({}), got {}; pass them after `--`",
            params.len(),
            names.join(", "),
            args.len()
        ));
    }
    let values = params
        .iter()
        .zip(args)
        .map(|((name, t), a)| parse(a, t).map_err(|e| format!("argument '{name}': {e}")))
        .collect::<Result<Vec<Val>, String>>()?;
    let mut results: Vec<Val> = fty.results().map(|_| Val::Bool(false)).collect();
    func.call(&mut store, &values, &mut results)
        .map_err(|e| format!("runtime error in '{shown}': {e}"))?;
    if wasi_run {
        return match results.first() {
            Some(Val::Result(Err(_))) => Err("`main` returned a non-zero exit code".to_string()),
            _ => Ok(()),
        };
    }
    for r in &results {
        println!("{}", show(r, true));
    }
    Ok(())
}

/// `demo:host/canvas@0.1.0` is shown as `canvas`.
fn short_name(name: &str) -> &str {
    let name = name.rsplit('/').next().unwrap_or(name);
    name.split('@').next().unwrap_or(name)
}

fn stub(li: &mut LinkerInstance<'_, Host>, name: &str, shown: String) -> Result<(), String> {
    li.func_new(
        name,
        move |_, ty: ComponentFunc, params: &[Val], results: &mut [Val]| {
            let args: Vec<String> = params.iter().map(|v| show(v, false)).collect();
            println!("[host] {shown}({})", args.join(", "));
            for (slot, t) in results.iter_mut().zip(ty.results()) {
                *slot = default(&t).map_err(wasmtime::Error::msg)?;
            }
            Ok(())
        },
    )
    .map_err(|e| format!("{e:#}"))
}

/// The value a stub import returns.
fn default(t: &Type) -> Result<Val, String> {
    Ok(match t {
        Type::Bool => Val::Bool(false),
        Type::S8 => Val::S8(0),
        Type::U8 => Val::U8(0),
        Type::S16 => Val::S16(0),
        Type::U16 => Val::U16(0),
        Type::S32 => Val::S32(0),
        Type::U32 => Val::U32(0),
        Type::S64 => Val::S64(0),
        Type::U64 => Val::U64(0),
        Type::Float32 => Val::Float32(0.0),
        Type::Float64 => Val::Float64(0.0),
        Type::Char => Val::Char(' '),
        Type::String => Val::String(String::new()),
        Type::List(_) => Val::List(Vec::new()),
        Type::Option(_) => Val::Option(None),
        Type::Flags(_) => Val::Flags(Vec::new()),
        Type::Tuple(t) => Val::Tuple(t.types().map(|t| default(&t)).collect::<Result<_, _>>()?),
        Type::Record(r) => Val::Record(
            r.fields()
                .map(|f| default(&f.ty).map(|v| (f.name.to_string(), v)))
                .collect::<Result<_, _>>()?,
        ),
        Type::Enum(e) => Val::Enum(e.names().next().unwrap_or_default().to_string()),
        other => return Err(format!("a stub cannot return {other:?}")),
    })
}

/// Parses a command-line argument as a value of type `t`.
fn parse(text: &str, t: &Type) -> Result<Val, String> {
    let bad = || format!("\"{text}\" is not a valid {}", type_name(t));
    Ok(match t {
        Type::Bool => Val::Bool(text.parse().map_err(|_| bad())?),
        Type::S8 => Val::S8(text.parse().map_err(|_| bad())?),
        Type::U8 => Val::U8(text.parse().map_err(|_| bad())?),
        Type::S16 => Val::S16(text.parse().map_err(|_| bad())?),
        Type::U16 => Val::U16(text.parse().map_err(|_| bad())?),
        Type::S32 => Val::S32(text.parse().map_err(|_| bad())?),
        Type::U32 => Val::U32(text.parse().map_err(|_| bad())?),
        Type::S64 => Val::S64(text.parse().map_err(|_| bad())?),
        Type::U64 => Val::U64(text.parse().map_err(|_| bad())?),
        Type::Float32 => Val::Float32(text.parse().map_err(|_| bad())?),
        Type::Float64 => Val::Float64(text.parse().map_err(|_| bad())?),
        Type::Char => {
            let mut chars = text.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Val::Char(c),
                _ => return Err(bad()),
            }
        }
        Type::String => Val::String(text.to_string()),
        // `1,2,3`; an empty argument is an empty list.
        Type::List(list) => {
            let elem = list.ty();
            if text.is_empty() {
                Val::List(Vec::new())
            } else {
                Val::List(
                    text.split(',')
                        .map(|item| parse(item.trim(), &elem))
                        .collect::<Result<_, _>>()?,
                )
            }
        }
        _ => return Err(format!("{} arguments are not supported", type_name(t))),
    })
}

fn type_name(t: &Type) -> String {
    match t {
        Type::List(l) => format!("list<{}>", type_name(&l.ty())),
        other => format!("{other:?}").to_lowercase(),
    }
}

/// Formats a value; strings are quoted unless `top` (a printed result).
fn show(v: &Val, top: bool) -> String {
    let list = |items: &[Val]| {
        let items: Vec<String> = items.iter().map(|v| show(v, false)).collect();
        format!("[{}]", items.join(", "))
    };
    match v {
        Val::Bool(b) => b.to_string(),
        Val::S8(n) => n.to_string(),
        Val::U8(n) => n.to_string(),
        Val::S16(n) => n.to_string(),
        Val::U16(n) => n.to_string(),
        Val::S32(n) => n.to_string(),
        Val::U32(n) => n.to_string(),
        Val::S64(n) => n.to_string(),
        Val::U64(n) => n.to_string(),
        Val::Float32(f) => f.to_string(),
        Val::Float64(f) => f.to_string(),
        Val::Char(c) if top => c.to_string(),
        Val::Char(c) => format!("{c:?}"),
        Val::String(s) if top => s.clone(),
        Val::String(s) => format!("{s:?}"),
        Val::List(items) | Val::Tuple(items) | Val::FixedLengthList(items) => list(items),
        other => format!("{other:?}"),
    }
}
