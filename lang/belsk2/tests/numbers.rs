//! Numeric types: literals, promotion, conversions, overflow, chars.

use belsk2::{ErrorKind, Interpreter, Value};

fn run(src: &str) -> String {
    let mut out = Vec::new();
    match Interpreter::new().run_source_with_writer(src, &mut out) {
        Ok(()) => String::from_utf8_lossy(&out).into_owned(),
        Err(e) => format!("ERROR: {e}"),
    }
}

fn err(src: &str) -> belsk2::Error {
    let mut out = Vec::new();
    match Interpreter::new().run_source_with_writer(src, &mut out) {
        Ok(()) => belsk2::Error::runtime("expected an error"),
        Err(e) => e,
    }
}

fn lines(items: &[&str]) -> String {
    items.iter().map(|s| format!("{s}\n")).collect()
}

#[test]
fn literal_types() {
    let src = r#"
        prinb(type(1)); prinb(type(1L)); prinb(type(1u)); prinb(type(1UL));
        prinb(type(3000000000)); prinb(type(10000000000));
        prinb(type(1.5)); prinb(type(1.5f)); prinb(type(2d)); prinb(type(1e3));
        prinb(0xFF); prinb(0b1010); prinb(1_000_000); prinb(1e3); prinb(2.5e-3);
        prinb(18446744073709551615);
    "#;
    assert_eq!(
        run(src),
        lines(&[
            "int",
            "long",
            "uint",
            "ulong",
            "uint",
            "long",
            "double",
            "float",
            "double",
            "double",
            "255",
            "10",
            "1000000",
            "1000",
            "0.0025",
            "18446744073709551615",
        ])
    );
}

#[test]
fn bad_literals() {
    for (src, needle) in [
        ("var x = 1abc;", "invalid number suffix"),
        ("var x = 99999999999999999999999;", "too large"),
        ("var x = 1.5u;", "invalid suffix"),
        ("var x = 0x;", "invalid number"),
        ("var x = 1_;", "misplaced '_'"),
    ] {
        let e = err(src);
        assert_eq!(e.kind, ErrorKind::Syntax, "{src}: {e}");
        assert!(e.message.contains(needle), "{src}: {e}");
    }
}

#[test]
fn integer_division_like_csharp() {
    let src = "prinb(7 / 2); prinb(-7 / 2); prinb(7 % 3); prinb(-7 % 3); \
               prinb(7.0 / 2); prinb(7 / 2.0); prinb(7 / 2 * 2.0);";
    assert_eq!(run(src), lines(&["3", "-3", "1", "-1", "3.5", "3.5", "6"]));
}

#[test]
fn overflow_wraps() {
    let src = r#"
        int big = 2147483647;
        prinb(big + 1);
        byte b = 250;
        b += 10;
        prinb(b);
        prinb(byte(300));
        prinb(sbyte(200));
        long l = 9223372036854775807L;
        prinb(l + 1);
        prinb(uint(-1));
    "#;
    assert_eq!(
        run(src),
        lines(&[
            "-2147483648",
            "4",
            "44",
            "-56",
            "-9223372036854775808",
            "4294967295"
        ])
    );
}

#[test]
fn implicit_conversions() {
    let src = r#"
        double d = 1;
        long l = 5;
        int i = 7;
        long l2 = i;
        double d2 = l2;
        byte b = 255;
        short s = b;
        float f = 0.1;
        char c = 'x';
        int code = c;
        prinb(d); prinb(l + l2); prinb(d2 / 2); prinb(s); prinb(f); prinb(code);
    "#;
    assert_eq!(run(src), lines(&["1", "12", "3.5", "255", "0.1", "120"]));
}

#[test]
fn narrowing_requires_explicit_conversion() {
    for (src, needle) in [
        ("int i = 2.5;", "cannot implicitly convert double to int"),
        (
            "long l = 1; int i = l;",
            "cannot implicitly convert long to int",
        ),
        ("byte b = 300;", "cannot implicitly convert int to byte"),
        ("byte b = -1;", "cannot implicitly convert int to byte"),
        (
            "int x = 1; byte b = x;",
            "cannot implicitly convert int to byte",
        ),
        (
            "double d = 1; float f = d;",
            "cannot implicitly convert double to float",
        ),
        ("fn f(x: int) { } f(1.5);", "argument 1 of 'f'"),
        ("fn f(): int { return 1L; }", "return value of 'f'"),
        ("int x = 1; x += 1.5;", "cannot store the double result"),
    ] {
        let e = err(src);
        assert_eq!(e.kind, ErrorKind::Type, "{src}: {e}");
        assert!(e.message.contains(needle), "{src}: {e}");
    }
    assert_eq!(
        run("prinb(int(2.9)); prinb(int(-2.9)); prinb(long(1e10)); prinb(int(\"42\"));"),
        lines(&["2", "-2", "10000000000", "42"])
    );
}

#[test]
fn promotion() {
    let src = r#"
        byte a = 200; byte b = 100;
        prinb(type(a + b)); prinb(a + b);
        uint u = 1; int i = -2;
        prinb(type(u + i)); prinb(u + i);
        prinb(type(1 + 2L)); prinb(type(1 + 2.5f)); prinb(type(1L + 2.5f)); prinb(type(2.5f + 1.0));
        prinb(type(-u));
    "#;
    assert_eq!(
        run(src),
        lines(&["int", "300", "long", "-1", "long", "float", "float", "double", "long"])
    );
    let e = err("ulong u = 1; int i = 1; prinb(u + i);");
    assert!(
        e.message.contains("cannot apply '+' to ulong and int"),
        "{e}"
    );
    let e = err("ulong u = 1; prinb(-u);");
    assert!(e.message.contains("cannot negate ulong"), "{e}");
}

#[test]
fn float_and_double() {
    let src = r#"
        float f = 0.1;
        prinb(f);
        prinb(double(f));
        prinb(0.1 + 0.2);
        prinb(1.0 / 0);
        prinb(-1.0 / 0);
        prinb(0.0 / 0.0);
        prinb(10.0 % 3);
        prinb(1.0 == 1);
        prinb(0.0 / 0.0 == 0.0 / 0.0);
    "#;
    assert_eq!(
        run(src),
        lines(&[
            "0.1",
            "0.10000000149011612",
            "0.30000000000000004",
            "Infinity",
            "-Infinity",
            "NaN",
            "1",
            "true",
            "false"
        ])
    );
}

#[test]
fn chars() {
    let src = r#"
        var s = "hey";
        var c = s[0];
        prinb(type(c));
        prinb(c);
        prinb(c == "h");
        prinb(s[0] + s[1]);
        char x = 'x';
        prinb(x + 1);
        prinb(char(x + 1));
        prinb(int(x));
        prinb(char(65));
        prinb('a' < 'b');
        string t = x;
        prinb(t + "!");
        for ch in "ab" { prinb(type(ch)); }
    "#;
    assert_eq!(
        run(src),
        lines(&["char", "h", "true", "he", "121", "y", "120", "A", "true", "x!", "char", "char"])
    );
    let e = err("char c = \"ab\";");
    assert!(e.message.contains("cannot use string as char"), "{e}");
    let e = err("prinb(char(\"ab\"));");
    assert!(e.message.contains("not a single character"), "{e}");
}

#[test]
fn bitwise_and_shifts() {
    let src = r#"
        prinb(6 & 3); prinb(6 | 3); prinb(6 ^ 3); prinb(~0);
        prinb(1 << 10); prinb(-16 >> 2); prinb(uint(4294967295) >> 28);
        prinb(1 << 33); prinb(1L << 33);
        prinb(true & false); prinb(true | false); prinb(true ^ true);
        var x = 5;
        x <<= 2; prinb(x);
        x >>= 1; prinb(x);
        x &= 6; prinb(x);
        x |= 1; prinb(x);
        x ^= 3; prinb(x);
        x *= 4; prinb(x);
        x /= 3; prinb(x);
        x %= 3; prinb(x);
        prinb(1 + 2 << 1);
        prinb((6 & 3) == 2);
    "#;
    assert_eq!(
        run(src),
        lines(&[
            "2",
            "7",
            "5",
            "-1",
            "1024",
            "-4",
            "15",
            "2",
            "8589934592",
            "false",
            "true",
            "false",
            "20",
            "10",
            "2",
            "3",
            "0",
            "0",
            "0",
            "0",
            "6",
            "true",
        ])
    );
    let e = err("prinb(6 & 3 == 2);");
    assert!(
        e.message.contains("cannot apply '&' to int and bool"),
        "{e}"
    );
    let e = err("prinb(1.5 & 1);");
    assert!(e.message.contains("cannot apply '&'"), "{e}");
    let e = err("prinb(1 << 1L);");
    assert!(e.message.contains("shift count must be an int"), "{e}");
    assert_eq!(err("prinb(1 > > 1);").kind, ErrorKind::Syntax);
}

#[test]
fn value_types_are_not_nullable() {
    for src in [
        "int x = null;",
        "double d = null;",
        "char c = null;",
        "bool b = null;",
    ] {
        let e = err(src);
        assert!(e.message.contains("cannot be null"), "{src}: {e}");
    }
    assert_eq!(run("string s = null; prinb(s);"), "null\n");
}

#[test]
fn every_path_must_return() {
    let e = err("fn f(x: int): int { if x > 0 { return 1; } }");
    assert!(e.message.contains("not all code paths"), "{e}");
    let ok = r#"
        fn a(x: int): int { if x > 0 { return 1; } else { return 2; } }
        fn b(x: int): int { while true { if x > 0 { return x; } x += 1; } }
        fn c(x: int): string { if x > 0 { return "pos"; } }
        prinb(a(1)); prinb(b(-3)); prinb(c(-1));
    "#;
    assert_eq!(run(ok), lines(&["1", "1", "null"]));
    let e = err("fn f(): int { return; }");
    assert!(e.message.contains("must return a value"), "{e}");
}

#[test]
fn any_values_are_checked_at_run_time() {
    assert_eq!(run("var a: any = 5L; int x = a; prinb(type(x));"), "int\n");
    let e = err("var a: any = 2.5; int x = a;");
    assert_eq!(e.kind, ErrorKind::Type);
    assert!(
        e.message
            .contains("cannot store double value in 'x' of type int"),
        "{e}"
    );
    let e = err("var a: any = 300; byte b = a;");
    assert!(e.message.contains("of type byte"), "{e}");
    let e = err("var a: any = null; int x = a;");
    assert!(e.message.contains("cannot be null"), "{e}");
    assert_eq!(
        run("var a: any = 1; var b: any = 2.5; prinb(a + b);"),
        "3.5\n"
    );
}

#[test]
fn host_values() {
    let mut interp = Interpreter::new();
    let mut out = Vec::new();
    let src =
        "fn half(x: double): double { return x / 2; } fn twice(x: long): long { return x * 2; }";
    assert!(interp.run_source_with_writer(src, &mut out).is_ok());
    let r = interp.call_function("half", &[Value::int(5)], &mut out);
    assert_eq!(r.ok(), Some(Value::double(2.5)));
    let r = interp.call_function("twice", &[Value::from(21.0)], &mut out);
    assert_eq!(r.ok().and_then(|v| v.as_i64()), Some(42));
    assert!(interp
        .call_function("twice", &[Value::from(1.5)], &mut out)
        .is_err());
}
