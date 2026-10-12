//! User-defined types in WASM builds: every program prints the same in
//! wasmtime (as a wasi0.2 command) as in the interpreter.

#![allow(clippy::unwrap_used, clippy::panic)] // a failing test panics

use belsk2_wasm::Target;
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::p2::bindings::sync::Command;
use wasmtime_wasi::p2::pipe::MemoryOutputPipe;
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

fn wasm_output(source: &str) -> String {
    let mut program = belsk2_syntax::parse(source).unwrap();
    let errors = belsk2_typeck::check(&mut program);
    assert!(errors.is_empty(), "{errors:?}");
    let wasm = belsk2_wasm::compile(&program, Target::Wasi02).unwrap_or_else(|errors| {
        let messages: Vec<String> = errors.iter().map(|e| format!("{e}")).collect();
        panic!("{}", messages.join("\n"))
    });
    let mut config = Config::new();
    config.wasm_gc(true);
    config.wasm_component_model(true);
    let engine = Engine::new(&config).unwrap();
    let component = Component::new(&engine, &wasm).unwrap();
    let mut linker: Linker<Host> = Linker::new(&engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker).unwrap();
    let stdout = MemoryOutputPipe::new(1 << 20);
    let wasi = WasiCtx::builder().stdout(stdout.clone()).build();
    let mut store = Store::new(
        &engine,
        Host {
            wasi,
            table: ResourceTable::new(),
        },
    );
    let command = Command::instantiate(&mut store, &component, &linker).unwrap();
    if let Err(e) = command.wasi_cli_run().call_run(&mut store) {
        panic!("{e:#}");
    }
    String::from_utf8(stdout.contents().to_vec()).unwrap()
}

fn interpreter_output(source: &str) -> String {
    let mut out = Vec::new();
    belsk2::run_source_with_writer(source, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

fn same(source: &str) {
    let expected = interpreter_output(source);
    assert!(!expected.is_empty());
    assert_eq!(wasm_output(source), expected);
}

#[test]
fn programs_of_the_interpreter_tests() {
    same(include_str!("../../belsk2/tests/inheritance.belsk2"));
    same(include_str!("../../belsk2/tests/members.belsk2"));
    same(include_str!("../../belsk2/tests/matching.belsk2"));
}

#[test]
fn structs() {
    same(
        r#"struct Point {
            x: double;
            y: double;
            label: string = "p";
            fn len2(): double { return x * x + y * y; }
            fn moveBy(dx: double, dy: double) { x += dx; this.y = y + dy; }
        }
        struct Line { a: Point; b: Point; }
        struct Counter { count: int; step: int = 2; Counter(start: int) { count = start; } fn inc() { count += step; } }
        fn first<T>(xs: T[]): T { return xs[0]; }
        fn main() {
            var p = Point(3, 4);
            var q = p;
            q.x = 10;
            p.moveBy(1, 1);
            var l = Line(Point(0, 0), p);
            l.b.x = 100;
            l.a.moveBy(0.5, 0.25);
            Point[] ps = [p, q];
            ps[0].y = 9;
            ps[1].moveBy(1, 1);
            prinb(p); prinb(q); prinb(l); prinb(ps);
            prinb(p.len2());
            prinb(p == Point(4, 5));
            prinb(p != q);
            var c = Counter(5);
            c.inc(); c.inc();
            prinb(c);
            prinb(first([Point(7, 8)]).x);
        }"#,
    );
}

#[test]
fn enums() {
    same(
        r#"enum Mode {
            Low = 1, Balanced = 5, High;
            fn fast(): bool { return this == Mode.High; }
            Label: string { get { if this == Mode.Low { return "eco"; } return "fast"; } }
            static fn parse(s: string): Mode { if s == "high" { return Mode.High; } return Mode.Low; }
        }
        struct Settings { mode: Mode; level: int; Settings() { level = 1; } }
        fn main() {
            var m = Mode.High;
            prinb(m);
            prinb(int(m));
            prinb(long(Mode.Low) + 1);
            prinb(m == Mode.High);
            prinb(m.fast());
            prinb(Mode.Low.Label);
            prinb(Mode.parse("high"));
            Mode[] all = [Mode.Low, Mode.Balanced, m];
            prinb(all);
            prinb(Settings());
            switch m {
                case Mode.Low: prinb("low");
                case Mode.Balanced, Mode.High: prinb("not low");
            }
        }"#,
    );
}

#[test]
fn classes_statics_and_properties() {
    same(
        r#"class Player {
            static count: int = 0;
            Name: string { get; private set; }
            Id: int { get; }
            private hp: int = 100;
            friend: Player = null;
            Hp: int { get { return hp; } set { if value < 0 { hp = 0; } else { hp = value; } } }
            Player(name: string) { Name = name; count += 1; Id = count; }
            fn hit(n: int) { Hp = Hp - n; }
            static fn create(name: string): Player { return Player(name); }
        }
        fn damage(p: Player) { p.hit(30); }
        fn main() {
            var a = Player.create("Ann");
            var b = a;
            damage(b);
            prinb(a.Hp);
            a.Hp -= 100;
            prinb(a.Hp);
            a.friend = Player("Bob");
            a.friend.Hp = 7;
            prinb(a);
            prinb(a == b);
            prinb(Player("x") == Player("x"));
            Player nobody = null;
            prinb(nobody);
            prinb(nobody == null);
            prinb(Player.count);
            a.friend.friend = a;
            prinb(len(str(a)) > 10);
        }"#,
    );
}

#[test]
fn inheritance_is_as_and_interfaces() {
    same(
        r#"interface IShape { fn area(): double; Name: string { get; } }
        abstract class Shape : IShape {
            Name: string { get; }
            Shape(name: string) { Name = name; }
            abstract fn area(): double;
            virtual fn describe(): string { return Name + " " + str(area()); }
        }
        class Circle : Shape {
            r: double;
            Circle(r: double) : base("circle") { this.r = r; }
            override fn area(): double { return 3 * r * r; }
        }
        class Square : Shape {
            s: double;
            Square(s: double) : base("square") { this.s = s; }
            override fn area(): double { return s * s; }
            override fn describe(): string { return "[" + base.describe() + "]"; }
        }
        struct Tag : IShape {
            Name: string { get; }
            fn area(): double { return 0; }
        }
        fn total(shapes: IShape[]): double {
            var t = 0.0;
            for s in shapes { t += s.area(); }
            return t;
        }
        fn main() {
            Shape[] shapes = [Circle(1), Square(2)];
            for s in shapes { prinb(s.describe()); }
            IShape[] all = [Circle(2), Square(3), Tag("t")];
            prinb(total(all));
            for s in all { prinb(s.Name); }
            Shape x = Square(5);
            prinb(x is Square);
            prinb(x is Circle);
            prinb(x is IShape);
            var c = x as Circle;
            prinb(c == null);
            var sq = x as Square;
            prinb(sq.s);
            prinb(x);
        }"#,
    );
}

#[test]
fn match_and_switch() {
    same(
        r#"enum Opt { Some(v: int), None }
        enum Tree { Leaf(v: int), Node(l: Tree, r: Tree) }
        fn sum(t: Tree): int {
            return match t { Leaf(v) => v, Node(l, r) => sum(l) + sum(r) };
        }
        fn kind(s: string): string {
            return match s { "a" | "e" => "vowel", "" => "empty", _ => "other" };
        }
        fn main() {
            prinb(sum(Tree.Node(Tree.Leaf(1), Tree.Node(Tree.Leaf(2), Tree.Leaf(3)))));
            Opt[] xs = [Opt.Some(4), Opt.None, Opt.Some(-1)];
            for o in xs {
                match o {
                    Opt.Some(n) if n > 0 => prinb("positive " + str(n)),
                    Opt.Some(n) => { prinb("other " + str(n)); }
                    Opt.None => prinb("none"),
                }
            }
            prinb(xs);
            prinb(Opt.Some(1) == Opt.Some(1));
            prinb(Opt.Some(1) == Opt.Some(2));
            prinb(kind("e")); prinb(kind("")); prinb(kind("z"));
            var i = 0;
            while i < 5 {
                switch i {
                    case 0: prinb("zero");
                    case 1, 2:
                        prinb("small");
                        if i == 2 { break; }
                        prinb("one");
                    default:
                        i += 1;
                        continue;
                }
                i += 1;
            }
            var b = match i > 3 { true => 1, false => 0 };
            prinb(b);
        }"#,
    );
}

#[test]
fn numbers_print_as_in_the_interpreter() {
    same(
        r#"fn main() {
            double[] ds = [0.1, 0.2 + 0.1, 3.14, -2.5, 1.0 / 3.0, 100.0, 1e15, 1e16, 123456789.125, 1e21, 1e-5, 1e-7, 1.234e-10, 0.0, 9007199254740993.0, 123.456, 0.000123];
            for d in ds { prinb(d); }
            float[] fs = [0.1f, 3.14f, 1.0f / 3.0f, 16777217.0f, 1e-7f, 2.5f];
            for f in fs { prinb(f); }
            prinb(0.0 / 0.0);
            prinb(1.0 / 0.0);
            prinb("x = " + str(2.75));
        }"#,
    );
}

#[test]
fn destructors_are_an_error() {
    let mut program = belsk2_syntax::parse("class A { ~A() {} } fn main() {}").unwrap();
    assert!(belsk2_typeck::check(&mut program).is_empty());
    let errors = belsk2_wasm::compile(&program, Target::Wasi02).unwrap_err();
    assert!(
        errors.iter().any(|e| e.message.contains("destructors")),
        "{errors:?}"
    );
}
