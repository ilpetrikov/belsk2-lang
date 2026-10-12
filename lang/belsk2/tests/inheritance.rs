//! Inheritance (`virtual`/`override`/`abstract`, `base`), interfaces and
//! `is`/`as`.

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

/// Animals, pets and tags (also in `inheritance.belsk2`).
const PROGRAM: &str = include_str!("inheritance.belsk2");

#[test]
fn virtual_methods_base_and_interfaces() {
    assert_eq!(
        run(PROGRAM),
        "Rex says woof (3 tricks)\nTweety says tweet\ntag blue\nAce says woof (1 tricks)\ntrue\nfalse\ntrue\ntrue\n2\nMax fetches\n"
    );
}

#[test]
fn inits_run_from_the_base_class_down() {
    let src = "class A { log: string[] = []; A() { push(log, \"A\"); } }
    class B : A { B() { push(log, \"B\"); } }
    class C : B { x: int; }
    class D : A { n: int; D(n: int) : base() { this.n = n; push(log, \"D\"); } }
    fn main() {
        prinb(C(5).log);
        prinb(C(5).x);
        prinb(D(2).log);
    }";
    assert_eq!(run(src), "[A, B]\n5\n[A, D]\n");
}

#[test]
fn inherited_fields_print_and_compare() {
    let src = "class A { a: int = 1; }
    class B : A { b: int = 2; }
    fn main() { var b = B(); prinb(b); var a: A = b; prinb(a == b); prinb(type(a)); }";
    assert_eq!(run(src), "B { a: 1, b: 2 }\ntrue\nB\n");
}

const BASE: &str = "abstract class Shape {
    protected name: string = \"shape\";
    abstract fn area(): double;
    virtual fn label(): string { return name; }
    fn fixed(): int { return 1; }
}
interface IDraw { fn draw(): string; Size: int { get; } }
";

#[test]
fn inheritance_errors() {
    assert!(belsk2::compile(BASE).is_ok());
    assert_error(
        &format!("{BASE}class Sq : Shape {{ fn area(): double {{ return 1; }} }}"),
        "already a member of Shape",
    );
    assert_error(&format!("{BASE}class Sq : Shape {{ override fn fixed(): int {{ return 2; }} override fn area(): double {{ return 1; }} }}"), "no virtual method 'fixed'");
    assert_error(
        &format!("{BASE}class Sq : Shape {{ }}"),
        "must override the abstract method 'area' of Shape",
    );
    assert_error(
        &format!("{BASE}class Sq : Shape {{ override fn area(): int {{ return 1; }} }}"),
        "must take and return the same types",
    );
    assert_error(
        &format!("{BASE}fn main() {{ var s = Shape(); }}"),
        "Shape is an abstract class",
    );
    assert_error(
        &format!("{BASE}fn main() {{ var s = IDraw(); }}"),
        "IDraw is an interface",
    );
    assert_error(
        &format!("{BASE}class P : IDraw {{ fn draw(): string {{ return \"\"; }} }}"),
        "does not implement `Size: int { get; }`",
    );
    assert_error(&format!("{BASE}class P : IDraw {{ Size: int {{ get; }} private fn draw(): string {{ return \"\"; }} }}"), "does not implement `fn draw(): string`");
    assert_error(
        &format!(
            "{BASE}class Sq : Shape {{ override fn area(): double {{ return base.area(); }} }}"
        ),
        "'area' is abstract in the base class",
    );
    assert_error(
        &format!("{BASE}fn main(s: Shape) {{ }} fn f(s: Shape): string {{ return s.name; }}"),
        "'name' is protected in Shape",
    );
    assert_error(&format!("{BASE}class Sq : Shape {{ override fn area(): double {{ prinb(name); return 1; }} }} fn main() {{ var s: Shape = Sq(); prinb(s.area()); var x = s as int; }}"), "'as' needs a type that can be null");
    assert_error("class A { abstract fn f(); }", "must be `abstract class`");
    assert_error(
        "struct S { virtual fn f() {} }",
        "are for members of classes",
    );
    assert_error("class A : B {} class B : A {}", "inherits from itself");
    assert_error(
        "struct P { x: int; } class C : P {}",
        "cannot inherit from struct P",
    );
    assert_error("interface I { x: int; }", "an interface has no fields");
    assert_error(
        "class A { A(x: int) {} } class B : A {}",
        "needs `B(...) : base(...)`",
    );
    assert_error(
        "class A { A(x: int) {} } class B : A { B() : base(\"s\") {} }",
        "argument 1 of the constructor of A",
    );
    assert_error("class A { } class B : A, A {}", "must come first");
    assert_error("interface I { fn f() { } }", "has no body");
}
