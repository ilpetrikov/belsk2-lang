//! `enum` (cases with numbers) and `class` (reference types).

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

const MODE: &str = "enum PerformanceMode {
    Low = 1,
    Balanced = 5,
    High,

    fn fast(): bool { return this == PerformanceMode.High; }
}
";

#[test]
fn enum_cases_are_typed_values_with_numbers() {
    let src = format!(
        "{MODE}fn main() {{
            var m = PerformanceMode.High;
            prinb(m);
            prinb(int(m));
            prinb(long(PerformanceMode.Low) + 1);
            prinb(m == PerformanceMode.High);
            prinb(m != PerformanceMode.Low);
            prinb(m.fast());
            prinb(type(m));
            var names = {{PerformanceMode.Low: \"eco\"}};
            names[PerformanceMode.High] = \"turbo\";
            prinb(names);
            PerformanceMode[] all = [PerformanceMode.Low, PerformanceMode.Balanced];
            prinb(all);
        }}"
    );
    assert_eq!(
        run(&src),
        "High\n6\n2\ntrue\ntrue\ntrue\nPerformanceMode\n{Low: eco, High: turbo}\n[Low, Balanced]\n"
    );
}

#[test]
fn enum_fields_default_to_the_first_case() {
    let src = format!(
        "{MODE}struct Settings {{ mode: PerformanceMode; level: int; init() {{ level = 1; }} }}
        fn main() {{ prinb(Settings()); }}"
    );
    assert_eq!(run(&src), "Settings { mode: Low, level: 1 }\n");
}

const PLAYER: &str = "class Player {
    name: string;
    hp: int = 100;
    friend: Player = null;

    fn hit(n: int) { hp -= n; }
}
";

#[test]
fn classes_are_shared_references() {
    let src = format!(
        "{PLAYER}fn damage(p: Player) {{ p.hit(30); }}
        fn main() {{
            var a = Player(\"Ann\");
            var b = a;
            damage(b);
            prinb(a.hp);
            a.friend = Player(\"Bob\");
            a.friend.hp = 7;
            prinb(a);
            prinb(a == b);
            prinb(Player(\"Ann\") == Player(\"Ann\"));
            Player nobody = null;
            prinb(nobody);
            Player[] team = [a, Player(\"Cid\")];
            team[0].hit(1);
            prinb(a.hp);
        }}"
    );
    assert_eq!(
        run(&src),
        "70\nPlayer { name: Ann, hp: 70, friend: Player { name: Bob, hp: 7, friend: null } }\ntrue\nfalse\nnull\n69\n"
    );
}

#[test]
fn class_init_and_this() {
    let src = "class Counter {
        count: int;
        init(start: int) { count = start; }
        fn inc(): Counter { count += 1; return this; }
    }
    fn main() {
        var c = Counter(10);
        c.inc().inc();
        prinb(c.count);
    }";
    assert_eq!(run(src), "12\n");
}

#[test]
fn cyclic_objects_print() {
    let src = format!(
        "{PLAYER}fn main() {{ var a = Player(\"A\"); a.friend = a; prinb(len(str(a)) < 1000); }}"
    );
    assert_eq!(run(&src), "true\n");
}

#[test]
fn enum_and_class_errors() {
    assert_error(
        &format!("{MODE}fn main() {{ var m = PerformanceMode(1); }}"),
        "has no constructor",
    );
    assert_error(
        &format!("{MODE}fn main() {{ var m = PerformanceMode.Turbo; }}"),
        "has no member 'Turbo'",
    );
    assert_error(
        &format!("{MODE}fn main() {{ var m = PerformanceMode.Low; var n = m.High; }}"),
        "used through the type",
    );
    assert_error(
        &format!("{MODE}fn main() {{ PerformanceMode m = null; }}"),
        "cannot be null",
    );
    assert_error(
        &format!("{MODE}fn main() {{ int x = PerformanceMode.Low; }}"),
        "cannot use PerformanceMode as int",
    );
    assert_error("enum E { A = \"x\" }", "whole number");
    assert_error("enum E { A, x: int }", "only cases and methods");
    assert_error(
        "class C { x: int; fn f() { this = C(1); } }",
        "cannot be replaced in a class",
    );
    assert_error(
        &format!("{PLAYER}fn main() {{ var p = Player(); }}"),
        "expects 1 argument",
    );
}
