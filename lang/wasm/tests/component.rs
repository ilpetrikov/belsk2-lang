//! Builds component projects and runs them in wasmtime with a host that
//! implements the imports.

#![allow(clippy::unwrap_used, clippy::panic)] // a failing test panics

use std::path::{Path, PathBuf};

use belsk2::project::{self, Mode};
use wasmtime::component::{Component, Linker};
use wasmtime::{Engine, Store};

/// Writes a project with the given files into a fresh directory.
fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("belsk2-component-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

fn build(dir: &Path) -> Result<Vec<u8>, String> {
    let options = project::Options {
        offline: true,
        ..project::Options::default()
    };
    let mut loaded = project::load_with(dir, Mode::All, &options)
        .map_err(|(errors, _)| format!("{errors:?}"))?;
    let errors = project::check(&mut loaded);
    if !errors.is_empty() {
        return Err(format!("{errors:?}"));
    }
    belsk2_wasm::compile_project(&loaded).map_err(|errors| {
        errors
            .iter()
            .map(|e| e.message.clone())
            .collect::<Vec<_>>()
            .join("\n")
    })
}

fn engine() -> Engine {
    let mut config = wasmtime::Config::new();
    config.wasm_gc(true);
    config.wasm_component_model(true);
    Engine::new(&config).unwrap()
}

const MANIFEST: &str =
    "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[component]\nworld = \"wit\"\n";

#[test]
fn exports_and_imports() {
    let wit = r#"
package demo:app;

world app {
    import log: func(msg: string);
    import scale: func(x: s32) -> s32;
    import names: func() -> list<string>;
    export greet: func(name: string) -> string;
    export sum: func(xs: list<s32>) -> s64;
    export words: func(text: string) -> list<string>;
    export pick: func(xs: list<bool>, b: u8) -> list<u8>;
    export count: func() -> u32;
}
"#;
    let main = r#"
var calls = 0;

fn greet(name: string): string {
    calls += 1;
    log("greeting " + name);
    var all = "";
    for n in names() { all += n; }
    return "Hi, " + name + "! " + str(scale(21)) + " " + all;
}

fn sum(xs: int[]): long {
    calls += 1;
    long total = 0;
    for x in xs { total += x; }
    return total;
}

fn words(text: string): string[] {
    calls += 1;
    string[] out = [];
    var w = "";
    for c in text {
        if str(c) == " " { push(out, w); w = ""; } else { w += c; }
    }
    push(out, w);
    return out;
}

fn pick(xs: bool[], b: byte): byte[] {
    byte[] out = [];
    for x in xs { if x { push(out, b); } else { push(out, byte(0)); } }
    return out;
}

fn count(): uint { return uint(calls); }
"#;
    let dir = project(
        "basic",
        &[
            ("belsk2.toml", MANIFEST),
            ("wit/app.wit", wit),
            ("src/main.belsk2", main),
        ],
    );
    let wasm = build(&dir).unwrap();

    let engine = engine();
    let component = Component::new(&engine, &wasm).unwrap();
    let mut linker: Linker<Vec<String>> = Linker::new(&engine);
    let mut root = linker.root();
    root.func_wrap("log", |mut store, (msg,): (String,)| {
        store.data_mut().push(msg);
        Ok(())
    })
    .unwrap();
    root.func_wrap("scale", |_, (x,): (i32,)| Ok((x * 2,)))
        .unwrap();
    root.func_wrap("names", |_, (): ()| {
        Ok((vec!["\u{e9}".to_string(), "\u{1f600}".to_string()],))
    })
    .unwrap();
    let mut store = Store::new(&engine, Vec::new());
    let instance = linker.instantiate(&mut store, &component).unwrap();

    let greet = instance
        .get_typed_func::<(String,), (String,)>(&mut store, "greet")
        .unwrap();
    for _ in 0..3 {
        let (out,) = greet.call(&mut store, ("W\u{f6}rld".to_string(),)).unwrap();
        assert_eq!(out, "Hi, W\u{f6}rld! 42 \u{e9}\u{1f600}");
    }
    assert_eq!(
        store.data().first().map(String::as_str),
        Some("greeting W\u{f6}rld")
    );

    let sum = instance
        .get_typed_func::<(Vec<i32>,), (i64,)>(&mut store, "sum")
        .unwrap();
    let (total,) = sum
        .call(&mut store, (vec![i32::MAX, i32::MAX, -5],))
        .unwrap();
    assert_eq!(total, 2 * i64::from(i32::MAX) - 5);

    let words = instance
        .get_typed_func::<(String,), (Vec<String>,)>(&mut store, "words")
        .unwrap();
    let text = "a long text ".repeat(5000);
    let (list,) = words.call(&mut store, (text.clone(),)).unwrap();
    assert_eq!(list, text.split(' ').map(String::from).collect::<Vec<_>>());

    let pick = instance
        .get_typed_func::<(Vec<bool>, u8), (Vec<u8>,)>(&mut store, "pick")
        .unwrap();
    let (bytes,) = pick
        .call(&mut store, (vec![true, false, true], 200))
        .unwrap();
    assert_eq!(bytes, vec![200, 0, 200]);

    let count = instance
        .get_typed_func::<(), (u32,)>(&mut store, "count")
        .unwrap();
    let (n,) = count.call(&mut store, ()).unwrap();
    assert_eq!(n, 5);
}

#[test]
fn mismatches_are_reported() {
    let wit = "package demo:app;\nworld app {\n  export add: func(a: s32, b: s32) -> s32;\n  export missing: func();\n}\n";
    let main = "fn add(a: int, b: long): int { return a; }\nfn main() {}\n";
    let dir = project(
        "mismatch",
        &[
            ("belsk2.toml", MANIFEST),
            ("wit/app.wit", wit),
            ("src/main.belsk2", main),
        ],
    );
    let errors = build(&dir).unwrap_err();
    assert!(
        errors.contains("expects `fn add(a: int, b: int): int`"),
        "{errors}"
    );
    assert!(errors.contains("add `fn missing()`"), "{errors}");
}

#[test]
fn bad_wit_is_a_project_error() {
    let dir = project(
        "badwit",
        &[
            ("belsk2.toml", MANIFEST),
            (
                "wit/app.wit",
                "package demo:app;\nworld app { export f: func(; }\n",
            ),
            ("src/main.belsk2", "fn main() {}\n"),
        ],
    );
    assert!(build(&dir).is_err());
}

const HOST_WIT: &str = r#"
package demo:host@0.1.0;

interface canvas {
    /// Draws text at a column.
    draw: func(text: string, x: s32) -> bool;
    record point { x: s32, y: s32 }
    center: func() -> point;
}
"#;

const APP_WIT: &str = r#"
package demo:app;

interface hooks {
    update: func(frame: u32) -> string;
}

world app {
    import demo:host/canvas@0.1.0;
    export hooks;
    export version: func() -> string;
}
"#;

#[test]
fn interfaces_are_modules() {
    let hooks = r#"
using canvas;

fn update(frame: uint): string {
    if draw("frame " + str(frame), 3) {
        return "drawn " + str(frame);
    }
    return "skipped";
}
"#;
    let dir = project(
        "interfaces",
        &[
            ("belsk2.toml", MANIFEST),
            ("wit/app.wit", APP_WIT),
            ("wit/deps/host/host.wit", HOST_WIT),
            (
                "src/lib.belsk2",
                "fn version(): string { return \"1.0\"; }\n",
            ),
            ("src/hooks.belsk2", hooks),
        ],
    );
    let wasm = build(&dir).unwrap();

    let engine = engine();
    let component = Component::new(&engine, &wasm).unwrap();
    let mut linker: Linker<Vec<String>> = Linker::new(&engine);
    linker
        .instance("demo:host/canvas@0.1.0")
        .unwrap()
        .func_wrap("draw", |mut store, (text, x): (String, i32)| {
            store.data_mut().push(format!("{x}:{text}"));
            Ok((x > 0,))
        })
        .unwrap();
    let mut store = Store::new(&engine, Vec::new());
    let instance = linker.instantiate(&mut store, &component).unwrap();

    let version = instance
        .get_typed_func::<(), (String,)>(&mut store, "version")
        .unwrap();
    assert_eq!(version.call(&mut store, ()).unwrap().0, "1.0");

    let iface = instance
        .get_export_index(&mut store, None, "demo:app/hooks")
        .unwrap();
    let index = instance
        .get_export_index(&mut store, Some(&iface), "update")
        .unwrap();
    let update = instance
        .get_typed_func::<(u32,), (String,)>(&mut store, &index)
        .unwrap();
    assert_eq!(update.call(&mut store, (7,)).unwrap().0, "drawn 7");
    assert_eq!(store.data(), &vec!["3:frame 7".to_string()]);
}

#[test]
fn unsupported_wit_types_are_reported_when_used() {
    let hooks = "using canvas;\n\nfn update(frame: uint): string {\n    var p = center();\n    return \"\";\n}\n";
    let dir = project(
        "unsupported",
        &[
            ("belsk2.toml", MANIFEST),
            ("wit/app.wit", APP_WIT),
            ("wit/deps/host/host.wit", HOST_WIT),
            (
                "src/lib.belsk2",
                "fn version(): string { return \"1.0\"; }\n",
            ),
            ("src/hooks.belsk2", hooks),
        ],
    );
    let errors = build(&dir).unwrap_err();
    assert!(
        errors.contains("'center' cannot be used in Belsk2 yet: it uses record 'point'"),
        "{errors}"
    );
}
