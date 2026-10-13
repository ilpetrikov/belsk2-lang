//! Builds component projects and runs them in wasmtime with a host that
//! implements the imports.

#![allow(clippy::unwrap_used, clippy::panic)] // a failing test panics

use std::path::{Path, PathBuf};

use belsk2::project::{self, Mode};
use wasmtime::component::{Component, Linker, Val};
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
    belsk2_wasm::compile_project(&loaded, belsk2_wasm::Target::Wasm32).map_err(|errors| {
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
    corner: func() -> tuple<s32, s32>;
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
    let mut canvas = linker.instance("demo:host/canvas@0.1.0").unwrap();
    canvas
        .func_wrap("draw", |mut store, (text, x): (String, i32)| {
            store.data_mut().push(format!("{x}:{text}"));
            Ok((x > 0,))
        })
        .unwrap();
    canvas
        .func_new("center", |_, _, _, results: &mut [Val]| {
            if let Some(slot) = results.first_mut() {
                *slot = Val::Record(vec![("x".into(), Val::S32(1)), ("y".into(), Val::S32(2))]);
            }
            Ok(())
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
    let hooks = "using canvas;\n\nfn update(frame: uint): string {\n    var p = corner();\n    return \"\";\n}\n";
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
        errors.contains("'corner' cannot be used in Belsk2 yet: it uses a tuple"),
        "{errors}"
    );
}

const TYPES_WIT: &str = r#"
package demo:app;

interface geo {
    record point { x: s32, y: s32 }
    enum color { red, green, dark-blue }
    variant shape { circle(f64), rect(point), tagged(string), empty }
    nearest: func(p: point) -> option<point>;
    paint: func(s: shape, c: color) -> result<u32, string>;
}

world app {
    import geo;
    use geo.{point, color, shape};
    record stats { count: u32, total: f64, name: string, best: option<point>, tint: color, small: s8 }
    export area: func(s: shape) -> f64;
    export summary: func(shapes: list<shape>) -> stats;
    export mirror: func(p: point) -> point;
    export classify: func(n: s64) -> result<string, u8>;
    export roundtrip: func(s: shape) -> shape;
    export first-color: func(cs: list<color>) -> option<color>;
    export ask: func(p: point, s: shape) -> string;
}
"#;

const TYPES_MAIN: &str = r#"
using geo;

fn area(s: Shape): double {
    return match s {
        Circle(r) => 3.0 * r * r,
        Rect(p) => double(p.x * p.y),
        Tagged(t) => double(len(t)),
        Empty => 0.0,
    };
}

fn summary(shapes: Shape[]): Stats {
    var total = 0.0;
    var best = OptionPoint.None;
    for s in shapes {
        total += area(s);
        match s {
            Rect(p) => { best = OptionPoint.Some(p); }
            _ => {}
        }
    }
    return Stats(uint(len(shapes)), total, "shapes", best, Color.DarkBlue, sbyte(-5));
}

fn mirror(p: Point): Point { return Point(-p.x, p.y * 2); }

fn classify(n: long): ResultStringOrByte {
    if n < 0L { return ResultStringOrByte.Err(byte(n * -1L)); }
    return ResultStringOrByte.Ok("n=" + str(n));
}

fn roundtrip(s: Shape): Shape { return s; }

fn first_color(cs: Color[]): OptionColor {
    if len(cs) == 0 { return OptionColor.None; }
    return OptionColor.Some(cs[0]);
}

fn ask(p: Point, s: Shape): string {
    var out = "";
    match nearest(p) {
        Some(q) => { out += "near " + str(q.x) + "," + str(q.y); }
        None => { out += "none"; }
    }
    match paint(s, Color.Green) {
        Ok(n) => { out += " ok " + str(n); }
        Err(e) => { out += " err " + e; }
    }
    return out;
}

fn main() {}
"#;

#[test]
fn records_enums_and_variants() {
    let dir = project(
        "types",
        &[
            ("belsk2.toml", MANIFEST),
            ("wit/app.wit", TYPES_WIT),
            ("src/main.belsk2", TYPES_MAIN),
        ],
    );
    let wasm = build(&dir).unwrap();
    let engine = engine();
    let component = Component::new(&engine, &wasm).unwrap();
    let mut linker: Linker<Vec<String>> = Linker::new(&engine);
    let mut geo = linker.instance("demo:app/geo").unwrap();
    geo.func_new("nearest", |_, _, params: &[Val], results: &mut [Val]| {
        let out = match params.first() {
            Some(Val::Record(fields)) if matches!(fields.first(), Some((_, Val::S32(x))) if *x > 0) => {
                Val::Option(Some(Box::new(Val::Record(vec![
                    ("x".into(), Val::S32(10)),
                    ("y".into(), Val::S32(-20)),
                ]))))
            }
            _ => Val::Option(None),
        };
        if let Some(slot) = results.first_mut() {
            *slot = out;
        }
        Ok(())
    })
    .unwrap();
    geo.func_new("paint", |mut store, _, params: &[Val], results: &mut [Val]| {
        store.data_mut().push(format!("{params:?}"));
        let out = match params.first() {
            Some(Val::Variant(case, _)) if case == "tagged" => {
                Val::Result(Err(Some(Box::new(Val::String("no tags".into())))))
            }
            _ => Val::Result(Ok(Some(Box::new(Val::U32(7))))),
        };
        if let Some(slot) = results.first_mut() {
            *slot = out;
        }
        Ok(())
    })
    .unwrap();
    let mut store = Store::new(&engine, Vec::new());
    let instance = linker.instantiate(&mut store, &component).unwrap();
    let mut call = |name: &str, args: &[Val]| -> Val {
        let f = instance.get_func(&mut store, name).unwrap();
        let mut results = vec![Val::Bool(false)];
        f.call(&mut store, args, &mut results).unwrap();
        results.into_iter().next().unwrap()
    };
    let point = |x: i32, y: i32| Val::Record(vec![("x".into(), Val::S32(x)), ("y".into(), Val::S32(y))]);
    let rect = |x, y| Val::Variant("rect".into(), Some(Box::new(point(x, y))));
    let circle = Val::Variant("circle".into(), Some(Box::new(Val::Float64(2.0))));
    let tagged = Val::Variant("tagged".into(), Some(Box::new(Val::String("hello".into()))));
    let empty = Val::Variant("empty".into(), None);

    assert_eq!(call("area", std::slice::from_ref(&circle)), Val::Float64(12.0));
    assert_eq!(call("area", &[rect(3, 4)]), Val::Float64(12.0));
    assert_eq!(call("area", std::slice::from_ref(&tagged)), Val::Float64(5.0));
    assert_eq!(call("mirror", &[point(3, -4)]), point(-3, -8));
    for s in [circle.clone(), rect(1, 2), tagged.clone(), empty.clone()] {
        assert_eq!(call("roundtrip", std::slice::from_ref(&s)), s);
    }
    assert_eq!(
        call("summary", &[Val::List(vec![circle.clone(), rect(2, 5), empty.clone(), tagged.clone()])]),
        Val::Record(vec![
            ("count".into(), Val::U32(4)),
            ("total".into(), Val::Float64(27.0)),
            ("name".into(), Val::String("shapes".into())),
            ("best".into(), Val::Option(Some(Box::new(point(2, 5))))),
            ("tint".into(), Val::Enum("dark-blue".into())),
            ("small".into(), Val::S8(-5)),
        ])
    );
    assert_eq!(
        call("summary", &[Val::List(Vec::new())]),
        Val::Record(vec![
            ("count".into(), Val::U32(0)),
            ("total".into(), Val::Float64(0.0)),
            ("name".into(), Val::String("shapes".into())),
            ("best".into(), Val::Option(None)),
            ("tint".into(), Val::Enum("dark-blue".into())),
            ("small".into(), Val::S8(-5)),
        ])
    );
    assert_eq!(
        call("classify", &[Val::S64(42)]),
        Val::Result(Ok(Some(Box::new(Val::String("n=42".into())))))
    );
    assert_eq!(
        call("classify", &[Val::S64(-9)]),
        Val::Result(Err(Some(Box::new(Val::U8(9)))))
    );
    assert_eq!(
        call(
            "first-color",
            &[Val::List(vec![Val::Enum("green".into()), Val::Enum("red".into())])]
        ),
        Val::Option(Some(Box::new(Val::Enum("green".into()))))
    );
    assert_eq!(call("first-color", &[Val::List(Vec::new())]), Val::Option(None));
    assert_eq!(
        call("ask", &[point(1, 1), circle.clone()]),
        Val::String("near 10,-20 ok 7".into())
    );
    assert_eq!(
        call("ask", &[point(-1, 1), tagged.clone()]),
        Val::String("none err no tags".into())
    );
    assert_eq!(store.data().len(), 2, "{:?}", store.data());
}
