//! The wasi0.2 target: programs run as WASI commands in wasmtime, with
//! stdin and stdout in memory.

#![allow(clippy::unwrap_used, clippy::panic)] // a failing test panics

use belsk2_wasm::Target;
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::p2::bindings::sync::Command;
use wasmtime_wasi::p2::pipe::{MemoryInputPipe, MemoryOutputPipe};
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

fn compile(source: &str) -> Result<Vec<u8>, String> {
    let mut program = belsk2_syntax::parse(source).map_err(|e| e.message)?;
    let errors = belsk2_typeck::check(&mut program);
    if !errors.is_empty() {
        return Err(format!("{errors:?}"));
    }
    belsk2_wasm::compile(&program, Target::Wasi02).map_err(|errors| {
        errors
            .iter()
            .map(|e| e.message.clone())
            .collect::<Vec<_>>()
            .join("\n")
    })
}

/// Runs a program with `input` on stdin; returns stdout and whether
/// `main` succeeded.
fn run(source: &str, input: &str) -> (String, bool) {
    let wasm = compile(source).unwrap();
    let mut config = Config::new();
    config.wasm_gc(true);
    config.wasm_component_model(true);
    let engine = Engine::new(&config).unwrap();
    let component = Component::new(&engine, &wasm).unwrap();
    let mut linker: Linker<Host> = Linker::new(&engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker).unwrap();
    let stdout = MemoryOutputPipe::new(1 << 20);
    let wasi = WasiCtx::builder()
        .stdin(MemoryInputPipe::new(input.as_bytes().to_vec()))
        .stdout(stdout.clone())
        .build();
    let mut store = Store::new(
        &engine,
        Host {
            wasi,
            table: ResourceTable::new(),
        },
    );
    let command = Command::instantiate(&mut store, &component, &linker).unwrap();
    let ok = command.wasi_cli_run().call_run(&mut store).unwrap().is_ok();
    let out = String::from_utf8(stdout.contents().to_vec()).unwrap();
    (out, ok)
}

#[test]
fn hello_world_with_input() {
    let src = r#"
fn main() {
    prinb("Hello, " + str(6 * 7));
    var name = input("name? ");
    var other = input();
    var eof = input();
    prinb("[" + name + "|" + other + "|" + eof + "]");
    prinb(true);
    prinb();
    prinb(len(name));
}
"#;
    let (out, ok) = run(src, "W\u{f6}rld \u{1f600}\r\nsecond");
    assert!(ok);
    assert_eq!(
        out,
        "Hello, 42\nname? [W\u{f6}rld \u{1f600}|second|]\ntrue\n\n7\n"
    );
}

#[test]
fn long_output_and_exit_code() {
    let src = r#"
var lines = 0;
fn main(): int {
    var s = "";
    var i = 0;
    while i < 3000 { s += "ab"; i += 1; }
    prinb(s);
    lines += 1;
    return lines;
}
"#;
    let (out, ok) = run(src, "");
    assert!(!ok);
    assert_eq!(out.len(), 6001);
    assert!(out.starts_with("abab") && out.ends_with("ab\n"));
}

#[test]
fn wasm32_has_no_io_and_wasi_needs_main() {
    let err = belsk2_wasm::compile_source("fn main() { prinb(1); }").unwrap_err();
    let message = err.first().map(|e| e.message.clone()).unwrap_or_default();
    assert!(message.contains("--target wasi0.2"), "{message}");
    let err = compile("fn helper(): int { return 1; }").unwrap_err();
    assert!(err.contains("runs `fn main()`"), "{err}");
}
