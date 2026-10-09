#![allow(clippy::unwrap_used, clippy::panic)] // a failing test panics
//! Compiles programs to WebAssembly, runs them with wasmtime and checks
//! that every call gives the same result as the interpreter.

use belsk2_interp::{FloatKind, IntKind, Interpreter, Ty, Value};
use belsk2_syntax::ast::{FnDecl, StmtKind};
use wasmtime::{Engine, Instance, Module, Store, Val};

fn decl(source: &str, name: &str) -> Option<FnDecl> {
    let mut program = belsk2_syntax::parse(source).ok()?;
    belsk2_typeck::check(&mut program);
    program.stmts.into_iter().find_map(|s| match s.kind {
        StmtKind::FnDecl(d) if d.name == name => Some((*d).clone()),
        _ => None,
    })
}

fn to_val(v: &Value) -> Val {
    match v {
        Value::Int(i, k) if k.bits() == 64 => Val::I64(*i as i64),
        Value::Int(i, _) => Val::I32(*i as i32),
        Value::Float(f, FloatKind::F32) => Val::F32((*f as f32).to_bits()),
        Value::Float(f, _) => Val::F64(f.to_bits()),
        Value::Bool(b) => Val::I32(i32::from(*b)),
        Value::Char(c) => Val::I32(u32::from(*c) as i32),
        _ => Val::I32(0),
    }
}

fn from_val(v: &Val, ty: &Ty, store: &mut Store<()>) -> Value {
    match (v, ty) {
        (Val::AnyRef(None), _) => Value::Null,
        (Val::AnyRef(Some(r)), Ty::String) => {
            let array = r.as_array(&mut *store).unwrap().unwrap();
            let text: String = array
                .elems(&mut *store)
                .unwrap()
                .map(|v| char::from_u32(v.unwrap_i32() as u32).unwrap_or('?'))
                .collect();
            Value::String(text)
        }
        (Val::I32(i), Ty::Int(k)) if k.signed() => Value::Int((*i).into(), *k),
        (Val::I32(i), Ty::Int(k)) => Value::Int((*i as u32).into(), *k),
        (Val::I64(i), Ty::Int(k)) if k.signed() => Value::Int((*i).into(), *k),
        (Val::I64(i), Ty::Int(k)) => Value::Int((*i as u64).into(), *k),
        (Val::I32(i), Ty::Bool) => Value::Bool(*i != 0),
        (Val::I32(i), Ty::Char) => Value::Char(char::from_u32(*i as u32).unwrap_or('?')),
        (Val::F32(f), _) => Value::Float(f32::from_bits(*f).into(), FloatKind::F32),
        (Val::F64(f), _) => Value::Float(f64::from_bits(*f), FloatKind::F64),
        _ => Value::Null,
    }
}

/// Runs each call in the interpreter and in WebAssembly (one instance for
/// all calls, so globals carry over) and compares the results.
fn same(source: &str, calls: &[(&str, Vec<Value>)]) {
    let wasm = match belsk2_wasm::compile_source(source) {
        Ok(w) => w,
        Err(e) => panic!("compile failed: {e:?}"),
    };
    let mut config = wasmtime::Config::new();
    config.wasm_gc(true);
    let engine = Engine::new(&config).unwrap();
    let module = Module::new(&engine, &wasm).unwrap();
    let mut store = Store::new(&engine, ());
    let instance = Instance::new(&mut store, &module, &[]).unwrap();

    let mut interp = Interpreter::new();
    let mut sink = Vec::new();
    let program = interp.compile(source).unwrap();
    interp.run_program(&program, &mut sink).unwrap();

    for (name, args) in calls {
        let d = decl(source, name).unwrap();
        let expected = interp
            .call_function(name, args, &mut sink)
            .map(|v| v.to_string())
            .map_err(|_| ());

        let func = instance.get_func(&mut store, name).unwrap();
        let params: Vec<Val> = args.iter().map(to_val).collect();
        let mut results = vec![Val::I32(0); func.ty(&store).results().len()];
        let actual = func
            .call(&mut store, &params, &mut results)
            .map(|()| match results.first() {
                Some(v) => from_val(v, &d.ret_resolved, &mut store).to_string(),
                None => Value::Null.to_string(),
            })
            .map_err(|_| ());
        assert_eq!(actual, expected, "{name}({args:?})");
    }
}

fn int(i: i64) -> Value {
    Value::Int(i.into(), IntKind::I32)
}

fn typed(i: i128, k: IntKind) -> Value {
    Value::Int(i, k)
}

fn double(f: f64) -> Value {
    Value::Float(f, FloatKind::F64)
}

#[test]
fn functions_and_control_flow() {
    let src = r#"
fn fact(n: long): long { long r = 1; while n > 1 { r *= n; n -= 1; } return r; }
fn fib(n: int): int { if n < 2 { return n; } return fib(n - 1) + fib(n - 2); }
fn gcd(a: int, b: int): int { while b != 0 { var t = a % b; a = b; b = t; } return a; }
fn loops(n: int): int {
    var s = 0;
    var i = 0;
    while true {
        i += 1;
        if i > n { break; }
        if i % 3 == 0 { continue; } else if i % 5 == 0 { s -= 1; } else { s += i; }
    }
    return s;
}
fn main() { var x = fib(5); }
"#;
    same(
        src,
        &[
            ("fact", vec![typed(20, IntKind::I64)]),
            ("fact", vec![typed(25, IntKind::I64)]),
            ("fib", vec![int(15)]),
            ("gcd", vec![int(1071), int(462)]),
            ("loops", vec![int(20)]),
            ("main", vec![]),
        ],
    );
}

#[test]
fn integer_semantics() {
    let src = r#"
fn wrap(a: int, b: int): int { return a * b + 1; }
fn bytes(x: int): byte { byte b = 250; b += x; return b; }
fn small(x: int): sbyte { return sbyte(x) ; }
fn div(a: int, b: int): int { return a / b; }
fn rem(a: int, b: int): int { return a % b; }
fn ldiv(a: long, b: long): long { return a / b; }
fn u(a: uint, b: uint): uint { return a / b + (a >> 1); }
fn mix(a: int, b: uint): long { return a + b; }
fn sh(x: long, n: int): long { return (x << n) ^ (x >> 3); }
fn neg(x: uint): long { return -x; }
fn bn(x: byte): int { return ~x; }
fn ul(x: ulong): ulong { return x * ulong(3) + ulong(1); }
fn widen(x: short): ulong { return ulong(x); }
"#;
    same(
        src,
        &[
            ("wrap", vec![int(65536), int(65536)]),
            ("bytes", vec![int(10)]),
            ("bytes", vec![int(-300)]),
            ("small", vec![int(200)]),
            ("div", vec![int(-2147483648), int(-1)]),
            ("div", vec![int(-7), int(2)]),
            ("div", vec![int(1), int(0)]),
            ("rem", vec![int(-7), int(2)]),
            ("rem", vec![int(-2147483648), int(-1)]),
            (
                "ldiv",
                vec![
                    typed(i64::MIN.into(), IntKind::I64),
                    typed(-1, IntKind::I64),
                ],
            ),
            (
                "u",
                vec![typed(4_000_000_000, IntKind::U32), typed(7, IntKind::U32)],
            ),
            ("mix", vec![int(-5), typed(4_000_000_000, IntKind::U32)]),
            ("sh", vec![typed(-12345, IntKind::I64), int(70)]),
            ("neg", vec![typed(4_000_000_000, IntKind::U32)]),
            ("bn", vec![typed(5, IntKind::U8)]),
            ("ul", vec![typed(u64::MAX.into(), IntKind::U64)]),
            ("widen", vec![typed(-2, IntKind::I16)]),
        ],
    );
}

#[test]
fn floats_chars_and_conversions() {
    let src = r#"
fn trunc(x: double): int { return int(x * 2.5); }
fn fl(x: float, y: float): float { return x / y + x % y; }
fn md(x: double, y: double): double { return x % y; }
fn ch(x: int): char { return char(x); }
fn code(c: char): int { return c + 1; }
fn next(c: char): char { return char(c + 1); }
fn cmp(a: double, b: double): bool { return a < b || a == b; }
fn lt(a: char, b: char): bool { return a < b; }
fn half(x: long): float { return x / 2.0f; }
fn letter(): char { char c = "z"; return c; }
"#;
    let nan = double(f64::NAN);
    same(
        src,
        &[
            ("trunc", vec![double(3.3)]),
            ("trunc", vec![double(-3.3)]),
            ("trunc", vec![nan.clone()]),
            (
                "fl",
                vec![
                    Value::Float(7.5, FloatKind::F32),
                    Value::Float(2.0, FloatKind::F32),
                ],
            ),
            ("md", vec![double(7.5), double(2.0)]),
            ("md", vec![double(-7.5), double(2.0)]),
            ("md", vec![double(1.0), double(0.0)]),
            ("md", vec![double(5.0), double(f64::INFINITY)]),
            ("ch", vec![int(65)]),
            ("ch", vec![int(-1)]),
            ("ch", vec![int(0xD800)]),
            ("code", vec![Value::Char('a')]),
            ("next", vec![Value::Char('y')]),
            ("cmp", vec![double(1.0), double(2.0)]),
            ("cmp", vec![nan.clone(), nan]),
            ("lt", vec![Value::Char('a'), Value::Char('b')]),
            ("half", vec![typed(7, IntKind::I64)]),
            ("letter", vec![]),
        ],
    );
}

#[test]
fn globals_short_circuit_and_bel() {
    let src = r#"
var calls = 0;
long big = 1L << 40;
double ratio = big / 3.0;
fn touch(): bool { calls += 1; return true; }
fn sc(x: bool): int {
    if x && touch() { calls += 10; }
    if x || touch() { calls += 100; }
    return calls;
}
fn get_big(): long { return big; }
fn get_ratio(): double { return ratio; }
fn b(x: double): bel { bel y = x; return y; }
"#;
    same(
        src,
        &[
            ("sc", vec![Value::Bool(false)]),
            ("sc", vec![Value::Bool(true)]),
            ("get_big", vec![]),
            ("get_ratio", vec![]),
            ("b", vec![double(500.0)]),
            ("b", vec![double(2000.0)]),
        ],
    );
}

#[test]
fn strings_arrays_and_generics() {
    let src = r#"
fn greet(n: int): string { return "Hi " + str(n) + ", " + true + "!"; }
fn count(s: string, c: char): int { var k = 0; for x in s { if x == c { k += 1; } } return k; }
fn count_a(): int { return count("banana", "a"); }
fn sum(): long {
    long[] xs = [1L, 2L];
    push(xs, 40L);
    xs[0] += 10L;
    var t = 0L;
    for x in xs { if x == 2L { continue; } t += x; }
    return t + len(xs);
}
fn pops(): int {
    int[] a = [];
    var i = 0;
    while i < 100 { push(a, i * i); i += 1; }
    var p = pop(a);
    return p + len(a) + a[50];
}
fn first<T>(xs: T[]): T { return xs[0]; }
fn pair<T>(a: T, b: T): T[] { return [a, b]; }
fn generic(): string { return first(["x", "y"]) + str(first([7, 8])) + first(pair("p", "q")); }
fn cut(): string { return substr("hello world", 6, 100) + substr("abc", -5, 2) + substr("abc", 10, 1); }
fn cmp(): int {
    var r = 0;
    if "abc" < "abd" { r += 1; }
    if "b" > "abc" { r += 2; }
    if "x" == "x" { r += 4; }
    if [1, 2] == [1, 2] { r += 8; }
    if [[1], [2]] != [[1], [3]] { r += 16; }
    string s = null;
    if s == null { r += 32; }
    if "a" != "ab" { r += 64; }
    return r;
}
fn nulls(): string { string s = null; return s + "!" + str(s); }
fn ints(): string {
    ulong m = ulong(0) - ulong(1);
    return str(-9223372036854775807L - 1L) + " " + str(m) + " " + str(byte(255)) + str(-0);
}
fn idx(i: int): int { int[] a = [1, 2, 3]; return a[i]; }
fn chars(): string { var s = ""; for c in "héllo" { s += c; } return s + str(len("héllo")) + s[1]; }
fn nested(): int {
    int[][] g = [[1, 2], [3]];
    push(g[1], 4);
    g[0][1] = 9;
    return g[0][1] + g[1][1] + len(g);
}
fn empty(): int { int[] a = []; return pop(a); }
var names = ["ann", "bob"];
fn names_len(): int { push(names, "cy"); return len(names); }
"#;
    same(
        src,
        &[
            ("greet", vec![int(-42)]),
            ("count_a", vec![]),
            ("sum", vec![]),
            ("pops", vec![]),
            ("generic", vec![]),
            ("cut", vec![]),
            ("cmp", vec![]),
            ("nulls", vec![]),
            ("ints", vec![]),
            ("idx", vec![int(2)]),
            ("idx", vec![int(3)]),
            ("idx", vec![int(-1)]),
            ("chars", vec![]),
            ("nested", vec![]),
            ("names_len", vec![]),
            ("names_len", vec![]),
        ],
    );
}

fn error(source: &str) -> String {
    match belsk2_wasm::compile_source(source) {
        Ok(_) => String::from("compiled"),
        Err(errors) => errors
            .iter()
            .map(|e| e.message.clone())
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

#[test]
fn unsupported_code_is_a_compile_error() {
    assert!(error("fn main() { prinb(1); }").contains("no input or output"));
    assert!(error("fn main() { var s = input(); }").contains("no input or output"));
    assert!(error("idb 1 = 2;\nfn main() {}").contains("idb"));
    assert!(error("fn f(x) { }").contains("needs a type"));
    assert!(error("fn f() { return 1; }").contains("declare its return type"));
    assert!(
        error("fn f(): double { return 1.5; } fn g(): string { return str(f()); }")
            .contains("not supported")
    );
    assert!(error("fn f() { var d = {\"a\": 1}; }").contains("dictionaries"));
}

#[test]
fn output_is_valid_wasm() {
    let wasm = belsk2_wasm::compile_source("fn main() { var x = 1; }").unwrap_or_default();
    assert!(wasm.starts_with(b"\0asm"));
}
