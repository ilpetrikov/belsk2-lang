//! `switch`/`case`, `match` with patterns, enums with data, and the check
//! that every value is covered.

use belsk2::Interpreter;

fn run(src: &str) -> String {
    let mut out = Vec::new();
    match Interpreter::new().run_source_with_writer(src, &mut out) {
        Ok(()) => String::from_utf8_lossy(&out).into_owned(),
        Err(e) => format!("ERROR: {e}"),
    }
}

fn assert_error(src: &str, needle: &str) {
    let errors: Vec<String> = match belsk2::compile(src) {
        Ok(_) => Vec::new(),
        Err(errors) => errors.into_iter().map(|e| e.message).collect(),
    };
    assert!(
        errors.iter().any(|e| e.contains(needle)),
        "expected {needle:?} in {errors:?}"
    );
}

/// Shapes, modes, expressions and animals (also in `matching.belsk2`).
const PROGRAM: &str = include_str!("matching.belsk2");

#[test]
fn switch_match_and_enums_with_data() {
    assert_eq!(
        run(PROGRAM),
        "Circle(1)\n3\nRect(2, 3)\n6\nEmpty\n0\nnot low\nbig\nodd 7\n-3\nwoof\na cat\nsomething\nyes\ntrue\n"
    );
}

#[test]
fn match_returns_from_every_arm() {
    let src = "enum Mode { Low, High }
    fn f(m: Mode): int { match m { Mode.Low => { return 1; } Mode.High => { return 2; } } }
    fn g(n: int): string { switch n { case 1: return \"one\"; default: return \"many\"; } }
    fn main() { prinb(f(Mode.High)); prinb(g(1)); prinb(g(5)); }";
    assert_eq!(run(src), "2\none\nmany\n");
}

#[test]
fn literals_strings_and_or_patterns() {
    let src = "fn kind(s: string): string {
        return match s { \"a\" | \"e\" | \"o\" => \"vowel\", \"\" => \"empty\", _ => \"other\" };
    }
    fn sign(n: long): int { return match n { 0 => 0, x if x < 0 => -1, _ => 1 }; }
    fn main() { prinb(kind(\"e\")); prinb(kind(\"\")); prinb(kind(\"z\")); prinb(sign(-5L)); }";
    assert_eq!(run(src), "vowel\nempty\nother\n-1\n");
}

const TYPES: &str = "enum Shape { Circle(radius: double), Rect(w: double, h: double), Empty }
enum Mode { Low, Medium, High }
enum Opt { Some(v: bool), None }
";

#[test]
fn coverage_errors() {
    let e = |body: &str, needle: &str| assert_error(&format!("{TYPES}{body}"), needle);
    e(
        "fn a(s: Shape): int { return match s { Shape.Circle(r) => 1, Empty => 0 }; }",
        "match does not cover Shape.Rect",
    );
    e(
        "fn b(m: Mode) { switch m { case Mode.Low: prinb(1); } }",
        "switch does not cover Mode.Medium, Mode.High",
    );
    e(
        "fn c(n: int) { switch n { case 1: prinb(1); } }",
        "a switch on int needs `default:`",
    );
    e(
        "fn d(n: int): int { return match n { 1 => 1, 2 => 2 }; }",
        "every value of int",
    );
    e(
        "fn e(o: Opt): int { return match o { Opt.Some(true) => 1, Opt.None => 0 }; }",
        "Opt.Some (only partly)",
    );
    e(
        "fn g(n: int): int { return match n { x if x > 0 => 1 }; }",
        "every value of int",
    );
    e(
        "fn h(s: Shape): int { return match s { Circle(r, q) => 1, _ => 0 }; }",
        "has 1 field, the pattern gives 2",
    );
    e(
        "fn i(s: Shape): int { return match s { Circle => 1, _ => 0 }; }",
        "case Circle has data",
    );
    e(
        "fn j(m: Mode) { switch m { case Mode.Low, Mode.Low: prinb(1); default: prinb(2); } }",
        "duplicate case Low",
    );
    e(
        "fn l(s: Shape): int { return match s { Circle(r) | Empty => 1, _ => 0 }; }",
        "cannot bind names",
    );
    e(
        "fn m(s: string): int { return match s { \"a\" => 1, 5 => 2, _ => 3 }; }",
        "can never match",
    );
    e(
        "fn n(s: Shape) { switch s { case Shape.Empty: prinb(1); default: prinb(2); } }",
        "use `match`",
    );
    e(
        "fn o(): Shape { return Shape.Circle; }",
        "case Circle has data",
    );
    e(
        "enum Bad { A(x: int) = 1, B }",
        "cannot give the cases numbers",
    );
    assert!(belsk2::compile(&format!(
        "{TYPES}fn f(o: Opt): int {{ return match o {{ Opt.Some(true) => 1, Opt.Some(false) => 2, Opt.None => 0 }}; }}
        fn g(b: bool): int {{ return match b {{ true => 1, false => 0 }}; }}
        fn h(s: Shape): int {{ return match s {{ Circle(_) | Empty => 1, Rect(w, _) => 2 }}; }}"
    ))
    .is_ok());
}
