//! Properties, `static` members and access modifiers.

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

/// A program using every kind of member (also in `members.belsk2`).
const PROGRAM: &str = include_str!("members.belsk2");

#[test]
fn properties_and_static_members() {
    assert_eq!(
        run(PROGRAM),
        "2\n2\n0\nfalse\n70\nAnna\n10\nBox { w: 4, h: 3 }\n1\n"
    );
}

#[test]
fn static_members_of_enums_and_structs() {
    let src = "enum Mode {
        Low, High;
        static fn parse(s: string): Mode { if s == \"high\" { return Mode.High; } return Mode.Low; }
        Label: string { get { if this == Mode.High { return \"fast\"; } return \"slow\"; } }
    }
    struct Vec2 {
        x: double; y: double;
        static zero: Vec2 = Vec2(0, 0);
        static fn of(v: double): Vec2 { return Vec2(v, v); }
    }
    fn main() {
        prinb(Mode.parse(\"high\").Label);
        prinb(Vec2.zero);
        prinb(Vec2.of(2).x);
    }";
    assert_eq!(run(src), "fast\nVec2 { x: 0, y: 0 }\n2\n");
}

const P: &str = "class P {
    private secret: int = 1;
    Name: string { get; private set; }
    Id: int { get; }
    Total: int { get { return secret; } }
    static count: int = 0;
    private init(id: int) { Id = id; }
    fn rename(n: string) { Name = n; Id = 2; }
    static fn make(): P { return P(1); }
    static fn bad(): int { return secret; }
}
";

#[test]
fn access_errors() {
    let main = |body: &str| format!("{P}fn main() {{ var p = P.make(); {body} }}");
    assert_error(&main("prinb(p.secret);"), "'secret' is private in P");
    assert_error(&main("p.Name = \"x\";"), "setting 'Name' is private in P");
    assert_error(&main("p.Id = 3;"), "'Id' is read-only");
    assert_error(&main(""), "'Id' is read-only");
    assert_error(&main("p.Total = 3;"), "property 'Total' has no set");
    assert_error(
        &main("prinb(p.count);"),
        "'count' is static; use it through the type: P.count",
    );
    assert_error(&main("prinb(P.Name);"), "belongs to a value of P");
    assert_error(&main("var q = P(2);"), "the init of P is private in P");
    assert_error(&main(""), "cannot be used in a static member");
    assert_error(
        "protected fn f() {}",
        "'protected' is for members of a type",
    );
    assert_error(
        "fn main() { private var x = 1; }",
        "access modifiers are for top-level",
    );
    assert_error(
        "class C { X: int { get { return 1; } set; } }",
        "both 'get' and 'set' bodies",
    );
}
