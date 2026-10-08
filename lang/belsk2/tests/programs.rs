use belsk2::{ErrorKind, Interpreter, Value};

fn run_with_input(src: &str, input: &str) -> Result<String, belsk2::Error> {
    let mut interp = Interpreter::new();
    interp.set_input(std::io::Cursor::new(input.to_string()));
    let mut out = Vec::new();
    interp.run_source_with_writer(src, &mut out)?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

fn run(src: &str) -> String {
    match run_with_input(src, "") {
        Ok(s) => s,
        Err(e) => format!("ERROR: {e}"),
    }
}

fn run_err(src: &str) -> belsk2::Error {
    match run_with_input(src, "") {
        Ok(out) => belsk2::Error::runtime(format!("expected an error, got output {out:?}")),
        Err(e) => e,
    }
}

#[test]
fn assignment_in_block_is_visible_outside() {
    assert_eq!(run("var x = 1; if true { x = 2; } prinb(x);"), "2\n");
}

#[test]
fn while_loop_terminates() {
    let src = "var i = 0; while i < 5 { i = i + 1; } prinb(i);";
    assert_eq!(run(src), "5\n");
}

#[test]
fn break_and_continue() {
    let src = r#"
        var i = 0; var s = 0;
        while true {
            i += 1;
            if i > 10 { break; }
            if i % 2 == 0 { continue; }
            s += i;
        }
        prinb(s);
    "#;
    assert_eq!(run(src), "25\n");
}

#[test]
fn block_scoped_variables_do_not_leak() {
    let e = run_err("if true { var y = 1; } prinb(y);");
    assert!(e.message.contains("undefined variable 'y'"), "{e}");
}

#[test]
fn closures_share_state() {
    let src = r#"
        var count = 0;
        fn inc() { count += 1; }
        inc(); inc(); inc();
        prinb(count);
    "#;
    assert_eq!(run(src), "3\n");
}

#[test]
fn recursion_and_mutual_recursion() {
    let src = r#"
        fn fib(n: int): int { if n <= 1 { return n; } return fib(n - 1) + fib(n - 2); }
        fn even(n) { if n == 0 { return true; } return odd(n - 1); }
        fn odd(n) { if n == 0 { return false; } return even(n - 1); }
        prinb(fib(15));
        prinb(even(10));
    "#;
    assert_eq!(run(src), "610\ntrue\n");
}

#[test]
fn index_assignment_mutates_array() {
    assert_eq!(run("var a = [1, 2, 3]; a[0] = 9; prinb(a);"), "[9, 2, 3]\n");
    assert_eq!(run("var a = [1, 2]; a[1] += 5; prinb(a);"), "[1, 7]\n");
}

#[test]
fn arrays_are_shared_references() {
    assert_eq!(run("var a = [1]; var b = a; b[0] = 5; prinb(a);"), "[5]\n");
}

#[test]
fn push_and_pop_mutate_in_place() {
    let src = r#"
        var a = [];
        push(a, 1);
        a = push(a, 2);
        prinb(a);
        prinb(pop(a));
        prinb(a);
        prinb(pop([]));
    "#;
    assert_eq!(run(src), "[1, 2]\n2\n[1]\nnull\n");
}

#[test]
fn strings_are_counted_in_characters() {
    let src = r#"
        var s = "привет";
        prinb(len(s));
        prinb(s[1]);
        prinb(substr(s, 2, 3));
        prinb(substr(s, 4, 100));
        for c in "аб" { prinb(c); }
    "#;
    assert_eq!(run(src), "6\nр\nиве\nет\nа\nб\n");
}

#[test]
fn unary_operators_nest() {
    assert_eq!(
        run("prinb(- -3); prinb(!!true); prinb(-(2 + 3));"),
        "3\ntrue\n-5\n"
    );
}

#[test]
fn int_declarations_and_conversion_calls() {
    let src = r#"
        int x = 5;
        float(3);
        int y = int(7.9);
        prinb(x + y);
        prinb(int("42") + 1);
    "#;
    assert_eq!(run(src), "12\n43\n");
}

#[test]
fn type_reports_int_and_float() {
    assert_eq!(
        run(
            r#"prinb(type(1)); prinb(type(1.5)); prinb(type("a")); prinb(type(null)); prinb(type([]));"#
        ),
        "int\ndouble\nstring\nnull\narray\n"
    );
}

#[test]
fn precedence_of_equality_and_relational() {
    assert_eq!(run("prinb(1 < 2 == 3 < 4);"), "true\n");
    assert_eq!(run("prinb(2 + 3 * 4);"), "14\n");
    assert_eq!(run("prinb(true || false && false);"), "true\n");
}

#[test]
fn string_comparison_and_concat() {
    assert_eq!(
        run(r#"prinb("a" < "b"); prinb("x" + 1 + 2);"#),
        "true\nx12\n"
    );
}

#[test]
fn bel_and_ster_rules() {
    assert_eq!(run("bel b = 1000; prinb(b);"), "1000\n");
    let e = run_err("bel b = 1001;");
    assert_eq!(e.kind, ErrorKind::Type);
    let e = run_err("bel b = 5; b = 2000;");
    assert!(e.message.contains("exceeds"), "{e}");
    let e = run_err("ster s = 5;");
    assert_eq!(e.kind, ErrorKind::Type);
}

#[test]
fn declared_types_are_checked_on_assignment() {
    let e = run_err(r#"string s = "a"; s = 5;"#);
    assert_eq!(e.kind, ErrorKind::Type);
    let e = run_err("bool b = 1;");
    assert_eq!(e.kind, ErrorKind::Type);
    // `var` takes the type of its initializer, like C#.
    let e = run_err(r#"var x = 1; x = "now a string";"#);
    assert_eq!(e.kind, ErrorKind::Type);
    assert!(e.message.contains("cannot use string as int in 'x'"), "{e}");
    assert_eq!(
        run(r#"var x: any = 1; x = "now a string"; prinb(x);"#),
        "now a string\n"
    );
    assert_eq!(run("var x = null; x = 5; prinb(x);"), "5\n");
}

#[test]
fn repl_keeps_declarations_between_runs() {
    let mut interp = Interpreter::new();
    let mut out = Vec::new();
    let src = "var n = 2; fn sq(x) { return x * x; }";
    assert!(interp.run_source_with_writer(src, &mut out).is_ok());
    assert!(interp
        .run_source_with_writer("prinb(sq(n));", &mut out)
        .is_ok());
    let e = interp.run_source_with_writer("prinb(sq(1, 2));", &mut out);
    assert!(e.is_err_and(|e| e.message.contains("expects 1 argument")));
    let e = interp.run_source_with_writer(r#"n = "text";"#, &mut out);
    assert!(e.is_err_and(|e| e.kind == ErrorKind::Type));
    assert_eq!(String::from_utf8_lossy(&out), "4\n");
}

#[test]
fn idb_bank_and_reab() {
    let src = r#"
        idb 7 = "banked";
        prinb(7);
        prinb(8);
        reab(3);
        prinb(3);
        prinb(input("name? "));
    "#;
    assert_eq!(
        run_with_input(src, "hello\nworld\n").unwrap_or_default(),
        "banked\n8\nhello\nname? world\n"
    );
}

#[test]
fn calling_the_result_of_a_call() {
    let src = r#"
        fn add1(x) { return x + 1; }
        fn get() { return add1; }
        prinb(get()(41));
    "#;
    assert_eq!(run(src), "42\n");
}

#[test]
fn errors_have_locations() {
    let e = run_err("var x = 1;\nvar zero = 0;\nprinb(x / zero);");
    assert_eq!(e.kind, ErrorKind::Runtime);
    assert_eq!(e.span.map(|s| s.line), Some(3));
    let e = run_err("var x = 1;\nprinb(x / 0);");
    assert_eq!(e.kind, ErrorKind::Type);
    assert_eq!(e.span.map(|s| s.line), Some(2));
}

/// Mistakes that are caught before the program starts.
#[test]
fn compile_errors() {
    for (src, needle) in [
        ("prinb(y);", "undefined variable"),
        ("y = 1;", "undefined variable"),
        ("prinb(1 - \"a\");", "cannot apply '-' to int and string"),
        ("prinb(true + 1);", "cannot apply '+'"),
        ("prinb([1] < [2]);", "cannot apply '<'"),
        ("prinb(-\"a\");", "cannot negate"),
        ("for x in 5 { }", "cannot iterate"),
        ("var x = 5; x();", "not a function"),
        ("prinb(num(\"abc\"));", "is not a number"),
        ("prinb(int(\"12x\"));", "is not a number"),
        ("prinb(len(5));", "expects a string or array, got int"),
        ("prinb(1, 2);", "expects 0 to 1 arguments"),
        ("push([1]);", "expects 2 arguments"),
        ("substr(\"abc\", \"1\", 2);", "an integer for start"),
        ("var s = \"ab\"; s[0] = \"x\";", "immutable"),
        ("var a = [1]; prinb(a.length);", "no member"),
        ("var a = [1]; prinb(a[\"0\"]);", "index must be an integer"),
        ("fn f(a, b) { } f(1);", "expects 2 arguments, got 1"),
        (
            "fn f(a: int) { } f(\"x\");",
            "cannot use string as int in argument 1 of 'f'",
        ),
        ("fn f(): string { return 1; }", "return value of 'f'"),
        ("fn f() { } f = 5;", "cannot use int as fn"),
        ("var x: number = 1;", "unknown type 'number'"),
        ("var x = 1; var x = 2;", "already declared"),
        ("fn f() { } fn f() { }", "already declared"),
        ("var f = len;", "can only be called"),
        ("bel b = 5; b = 2000;", "exceeds"),
        ("ster s = null;", "cannot be null"),
        ("var x = 10 % 0;", "division by zero"),
    ] {
        let e = run_err(src);
        assert_eq!(e.kind, ErrorKind::Type, "{src}: {e}");
        assert!(e.message.contains(needle), "{src}: {e}");
    }
}

#[test]
fn compile_reports_every_error() {
    let src = "var a = 1 - \"x\";\nprinb(len(5));\nprinb(nope);";
    let errors = belsk2::compile(src).err().unwrap_or_default();
    let lines: Vec<usize> = errors
        .iter()
        .filter_map(|e| e.span)
        .map(|s| s.line)
        .collect();
    assert_eq!(lines, [1, 2, 3]);
}

#[test]
fn nothing_runs_when_compilation_fails() {
    let mut interp = Interpreter::new();
    let mut out = Vec::new();
    let r = interp.run_source_with_writer("prinb(1); prinb(1 - \"a\");", &mut out);
    assert!(r.is_err());
    assert!(out.is_empty());
}

/// Values of type `any` are checked when the program runs.
#[test]
fn runtime_errors() {
    for (src, needle) in [
        ("var a = [1]; prinb(a[1]);", "out of bounds"),
        ("var a = [1]; prinb(a[-1]);", "out of bounds"),
        (
            "var a = [1]; var i: any = 0.5; prinb(a[i]);",
            "index must be an integer",
        ),
        (
            "var a = [1, \"a\"]; prinb(a[0] - a[1]);",
            "cannot apply '-' to int and string",
        ),
        ("var a = [\"abc\"]; prinb(num(a[0]));", "cannot convert"),
        (
            "var a = [5]; prinb(len(a[0]));",
            "expects a string or array",
        ),
        ("fn f(x) { return -x; } f(\"s\");", "cannot negate"),
        ("fn f(g) { g(); } f(1);", "not a function"),
        ("fn f(a: int) { } var v: any = \"x\"; f(v);", "argument 'a'"),
        ("var zero = 0; prinb(1 / zero);", "division by zero"),
    ] {
        let e = run_err(src);
        assert_ne!(e.kind, ErrorKind::Syntax, "{src}: {e}");
        assert!(e.message.contains(needle), "{src}: {e}");
    }
}

#[test]
fn functions_are_hoisted() {
    let src = r#"
        prinb(twice(4));
        fn twice(n) { return n * 2; }
        if true {
            prinb(inner());
            fn inner() { return "inner"; }
        }
    "#;
    assert_eq!(run(src), "8\ninner\n");
}

#[test]
fn syntax_errors() {
    for (src, needle) in [
        ("var = 5;", "expected a variable name"),
        ("break;", "outside of a loop"),
        ("return 1;", "outside of a function"),
        ("while true { fn f() { break; } }", "outside of a loop"),
        ("1 = 2;", "invalid assignment target"),
        ("var x = \"abc", "unterminated string"),
        ("var x = @;", "unexpected character"),
        ("for x of [1] { }", "expected 'in'"),
        ("var if = 1;", "keyword"),
        ("fn f(a, a) { }", "duplicate parameter"),
        ("prinb(\"\\q\");", "unknown escape"),
    ] {
        let e = run_err(src);
        assert_eq!(e.kind, ErrorKind::Syntax, "{src}: {e}");
        assert!(e.message.contains(needle), "{src}: {e}");
    }
}

#[test]
fn incomplete_input_is_flagged() {
    for src in ["fn f() {", "var x = (1 +", "prinb(\"abc", "/* comment"] {
        let e = run_err(src);
        assert!(e.incomplete, "{src}: {e}");
    }
    assert!(!run_err("var x = )").incomplete);
}

#[test]
fn deep_recursion_is_an_error_not_a_crash() {
    let e = run_err("fn f(n) { return f(n + 1); } f(0);");
    assert!(e.message.contains("stack overflow"), "{e}");
}

#[test]
fn deep_but_legal_recursion_works() {
    let src = "fn sum(n) { if n == 0 { return 0; } return n + sum(n - 1); } prinb(sum(5000));";
    assert_eq!(run(src), "12502500\n");
}

#[test]
fn deeply_nested_source_is_rejected() {
    let src = format!("prinb({}1{});", "(".repeat(5000), ")".repeat(5000));
    let e = run_err(&src);
    assert!(e.message.contains("nested too deeply"), "{e}");
    let src = format!("prinb(1{});", " + 1".repeat(5000));
    let e = run_err(&src);
    assert!(e.message.contains("too deeply nested"), "{e}");
}

#[test]
fn self_containing_and_deeply_nested_arrays_are_safe() {
    let src = "var a = [1]; push(a, a); prinb(len(a)); prinb(a == a);";
    assert_eq!(run(src), "2\ntrue\n");
    let src = "var a = []; var i = 0; while i < 200000 { a = [a]; i += 1; } prinb(len(a));";
    assert_eq!(run(src), "1\n");
}

#[test]
fn host_api() {
    let mut interp = Interpreter::new();
    let mut out = Vec::new();
    interp.define_global("scale", Value::from(0.5));
    let src = "var total = 0; fn tick(n) { total += n * scale; return total; }";
    assert!(interp.run_source_with_writer(src, &mut out).is_ok());
    assert!(interp.has_function("tick"));
    assert!(!interp.has_function("total"));
    let r = interp.call_function("tick", &[Value::from(4.0)], &mut out);
    assert_eq!(r.ok(), Some(Value::from(2.0)));
    assert_eq!(interp.get_global("total"), Some(Value::from(2.0)));
    assert!(interp.call_function("nope", &[], &mut out).is_err());
}
