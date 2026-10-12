//! `struct`: fields, methods, `init`, `this` and value semantics.

use belsk2::Interpreter;

fn run(src: &str) -> String {
    let mut out = Vec::new();
    match Interpreter::new().run_source_with_writer(src, &mut out) {
        Ok(()) => String::from_utf8_lossy(&out).into_owned(),
        Err(e) => format!("ERROR: {e}"),
    }
}

/// All errors the checker reports for `src`.
fn errors(src: &str) -> Vec<String> {
    match belsk2::compile(src) {
        Ok(_) => Vec::new(),
        Err(errors) => errors.into_iter().map(|e| e.message).collect(),
    }
}

fn assert_error(src: &str, needle: &str) {
    let errors = errors(src);
    assert!(
        errors.iter().any(|e| e.contains(needle)),
        "expected {needle:?} in {errors:?}"
    );
}

const POINT: &str = "struct Point {
    x: double;
    y: double;

    fn len2(): double { return x * x + y * y; }
    fn moveBy(dx: double, dy: double) { x += dx; this.y = y + dy; }
}
";

#[test]
fn fields_methods_and_printing() {
    let src = format!(
        "{POINT}fn main() {{ var p = Point(3, 4); prinb(p); prinb(p.len2()); prinb(p.x + 1); prinb(type(p)); }}"
    );
    assert_eq!(run(&src), "Point { x: 3, y: 4 }\n25\n4\nPoint\n");
}

#[test]
fn structs_are_copied() {
    let src = format!(
        "{POINT}fn main() {{
            var p = Point(3, 4);
            var q = p;
            q.x = 10;
            Point[] ps = [p, q];
            ps[0].y = 9;
            prinb(p);
            prinb(ps);
        }}"
    );
    assert_eq!(
        run(&src),
        "Point { x: 3, y: 4 }\n[Point { x: 3, y: 9 }, Point { x: 10, y: 4 }]\n"
    );
}

#[test]
fn methods_change_the_variable_they_are_called_on() {
    let src = format!(
        "{POINT}struct Line {{ a: Point; b: Point; }}
        fn main() {{
            var p = Point(3, 4);
            p.moveBy(1, 1);
            var l = Line(Point(0, 0), p);
            l.b.moveBy(10, 0);
            l.a.x = -1;
            Point[] ps = [p];
            ps[0].moveBy(1, 1);
            prinb(p);
            prinb(l);
            prinb(ps[0]);
        }}"
    );
    assert_eq!(
        run(&src),
        "Point { x: 4, y: 5 }\nLine { a: Point { x: -1, y: 0 }, b: Point { x: 14, y: 5 } }\nPoint { x: 5, y: 6 }\n"
    );
}

#[test]
fn init_and_field_values() {
    let src = "struct Counter {
        var count = 0;
        name: string = \"c\";
        step: int;
        init(start: int) { count = start; step = 2; }
        fn inc() { count += step; }
    }
    struct Opts { verbose: bool = true; level: int; }
    fn main() {
        var c = Counter(5);
        c.inc();
        prinb(c);
        prinb(Opts(3));
    }";
    assert_eq!(
        run(src),
        "Counter { count: 7, name: c, step: 2 }\nOpts { verbose: true, level: 3 }\n"
    );
}

#[test]
fn equality_generics_and_dictionaries() {
    let src = format!(
        "{POINT}fn first<T>(xs: T[]): T {{ return xs[0]; }}
        fn main() {{
            prinb(Point(1, 2) == Point(1, 2));
            prinb(Point(1, 2) != Point(1, 3));
            var p: Point = first([Point(5, 6)]);
            prinb(p.y);
            var named = {{\"origin\": Point(0, 0)}};
            prinb(named[\"origin\"].len2());
        }}"
    );
    assert_eq!(run(&src), "true\ntrue\n6\n0\n");
}

#[test]
fn struct_errors() {
    let p = "struct P { x: int; y: int; fn sum(): int { return x + y; } }\nfn make(): P { return P(1, 2); }\n";
    assert_error(
        &format!("{p}fn main() {{ var a = P(1); }}"),
        "expects 2 arguments",
    );
    assert_error(
        &format!("{p}fn main() {{ var a = P(1, \"a\"); }}"),
        "cannot use string as int",
    );
    assert_error(
        &format!("{p}fn main() {{ var a = P(1, 2); prinb(a.z); }}"),
        "P has no member 'z'",
    );
    assert_error(
        &format!("{p}fn main() {{ make().x = 5; }}"),
        "temporary P value",
    );
    assert_error(
        &format!("{p}fn main() {{ var a = P(1, 2); var f = a.sum; }}"),
        "can only be called",
    );
    assert_error(
        &format!("{p}fn main() {{ prinb(this); }}"),
        "'this' can only be used",
    );
    assert_error(
        &format!("{p}fn main() {{ var t = P; }}"),
        "'P' is a type, not a value",
    );
    assert_error(&format!("{p}fn main() {{ P r = null; }}"), "cannot be null");
    assert_error("struct L { inner: L; }", "contains itself");
    assert_error("struct D { x: int; x: int; }", "already has a member 'x'");
    assert_error("fn main() { struct S { } }", "top level");
}
