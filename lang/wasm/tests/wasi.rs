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
    let (out, code) = run_with(source, input, &[]);
    (out, code == 0)
}

/// Runs a program with command-line `args`; returns stdout and the exit
/// code.
fn run_with(source: &str, input: &str, args: &[&str]) -> (String, i32) {
    let wasm = compile(source).unwrap();
    let mut config = Config::new();
    config.wasm_gc(true);
    config.wasm_component_model(true);
    let engine = Engine::new(&config).unwrap();
    let component = Component::new(&engine, &wasm).unwrap();
    let mut linker: Linker<Host> = Linker::new(&engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker).unwrap();
    let stdout = MemoryOutputPipe::new(1 << 20);
    let mut argv = vec!["main"];
    argv.extend(args);
    let wasi = WasiCtx::builder()
        .stdin(MemoryInputPipe::new(input.as_bytes().to_vec()))
        .stdout(stdout.clone())
        .args(&argv)
        .build();
    let mut store = Store::new(
        &engine,
        Host {
            wasi,
            table: ResourceTable::new(),
        },
    );
    let command = Command::instantiate(&mut store, &component, &linker).unwrap();
    let code = match command.wasi_cli_run().call_run(&mut store) {
        Ok(Ok(())) => 0,
        Ok(Err(())) => 1,
        Err(e) => match e.downcast_ref::<wasmtime_wasi::I32Exit>() {
            Some(exit) => exit.0,
            None => panic!("{e:#}"),
        },
    };
    let out = String::from_utf8(stdout.contents().to_vec()).unwrap();
    (out, code)
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

#[test]
fn arguments_id_bank_and_exit_codes() {
    let src = r#"
idb 1 = "top";

fn main(args: string[]): int {
    prinb(len(args));
    for a in args { prinb(a); }
    idb 7 = 42;
    prinb(7);
    prinb(1);
    prinb(8);
    var name = reab(3);
    prinb(3);
    return 300;
}
"#;
    let (out, code) = run_with(src, "Ann\nrest", &["one", "two words"]);
    assert_eq!(out, "2\none\ntwo words\n42\ntop\n8\nAnn\n");
    // Exit codes are bytes, as on Unix.
    assert_eq!(code, 300 % 256);
    let (_, code) = run_with("fn main(): long { return 0L; }", "", &[]);
    assert_eq!(code, 0);
}

#[test]
fn long_input_is_read_in_blocks() {
    let src = r#"
fn main() {
    var total = 0;
    var lines = 0;
    var line = input();
    while line != "" {
        total += len(line);
        lines += 1;
        line = input();
    }
    prinb(str(lines) + " " + str(total));
}
"#;
    let mut input = String::new();
    for i in 0..3000 {
        input.push_str(&"x".repeat(i % 50 + 1));
        input.push_str(if i % 2 == 0 { "\n" } else { "\r\n" });
    }
    let total: usize = (0..3000).map(|i| i % 50 + 1).sum();
    let (out, _) = run_with(src, &input, &[]);
    assert_eq!(out, format!("3000 {total}\n"));
}
