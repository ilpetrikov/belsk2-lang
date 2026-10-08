//! Typed arrays, `List<T>`, `Dictionary<K, V>` and generic functions.

use belsk2::{ErrorKind, Interpreter};

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

fn compile_error(src: &str, needle: &str) {
    let e = err(src);
    assert_eq!(e.kind, ErrorKind::Type, "{src}: {e}");
    assert!(e.message.contains(needle), "{src}: {e}");
}

#[test]
fn array_literal_types() {
    let src = r#"
        prinb(type([1, 2])); prinb(type([1, 2.5])); prinb(type([1, 2L]));
        prinb(type(["a"])); prinb(type([1, "a"])); prinb(type([]));
        prinb(type([[1], [2, 3]])); prinb(type([null, "a"]));
        prinb(type(['a', 'b']));
    "#;
    assert_eq!(
        run(src),
        lines(&[
            "int[]", "double[]", "long[]", "string[]", "any[]", "any[]", "int[][]", "string[]",
            "string[]",
        ])
    );
}

#[test]
fn declared_array_types() {
    let src = r#"
        int[] xs = [];
        push(xs, 5);
        push(xs, 7);
        double[] ds = [1, 2];
        List<string> names = ["ann"];
        push(names, "bob");
        char[] cs = ['a', 'b'];
        int[][] grid = [[1, 2], [3]];
        grid[1][0] += 10;
        prinb(xs); prinb(ds); prinb(type(ds[0])); prinb(type(names)); prinb(names);
        prinb(type(cs)); prinb(grid);
        int last = pop(xs);
        prinb(last);
        for d in ds { prinb(type(d)); }
    "#;
    assert_eq!(
        run(src),
        lines(&[
            "[5, 7]",
            "[1, 2]",
            "double",
            "string[]",
            "[ann, bob]",
            "char[]",
            "[[1, 2], [13]]",
            "7",
            "double",
            "double",
        ])
    );
}

#[test]
fn array_type_errors() {
    compile_error(
        "int[] xs = []; push(xs, \"s\");",
        "second argument of push()",
    );
    compile_error("int[] xs = [1]; xs[0] = \"s\";", "cannot use string as int");
    compile_error("int[] xs = [\"x\"];", "an element of 'xs'");
    compile_error("var a = [1]; a = [1.5];", "an element of 'a'");
    compile_error(
        "var a = [1]; var b = [1.5]; a = b;",
        "cannot use double[] as int[]",
    );
    compile_error("string[] s = [1];", "cannot use int as string");
    compile_error(
        "int[] a = [1]; string s = a[0];",
        "cannot use int as string",
    );
    compile_error("List<int, int> l = [];", "needs 1 type argument");
    compile_error("List l = [];", "needs 1 type argument");
    compile_error("int<string> x = 1;", "does not take type arguments");
}

#[test]
fn arrays_are_checked_at_run_time() {
    let e = err("var x: any = \"s\"; int[] xs = []; push(xs, x);");
    assert!(e.message.contains("element of int[]"), "{e}");
    let e = err("int[] xs = [1]; any[] a = xs; push(a, \"s\");");
    assert!(e.message.contains("element of int[]"), "{e}");
    let e = err("int[] xs = [1]; any[] a = xs; a[0] = 2.5;");
    assert!(e.message.contains("element of int[]"), "{e}");
    assert_eq!(
        run("var a: any = [1, 2]; int[] b = a; prinb(b);"),
        "[1, 2]\n"
    );
    let e = err("var a: any = [1, \"x\"]; int[] b = a;");
    assert!(
        e.message
            .contains("cannot store any[] value in 'b' of type int[]"),
        "{e}"
    );
    assert_eq!(
        run("any[] a = [1]; push(a, \"two\"); push(a, a); prinb(len(a));"),
        "3\n"
    );
}

#[test]
fn dictionaries() {
    let src = r#"
        var ages = {"ann": 30, "bob": 25};
        prinb(type(ages));
        prinb(ages["ann"]);
        ages["cid"] = 40;
        ages["ann"] += 1;
        prinb(len(ages));
        prinb(has(ages, "bob"));
        prinb(remove(ages, "bob"));
        prinb(remove(ages, "bob"));
        prinb(keys(ages));
        prinb(values(ages));
        for k in ages { prinb(k + "=" + str(ages[k])); }
        prinb(ages);
        Dictionary<string, double> d = {};
        d["x"] = 1;
        prinb(type(d["x"]));
        Dictionary<long, string> byId = {1: "one"};
        prinb(byId[1]);
        prinb({"a": 1} == {"a": 1});
        Map<char, int> counts = {};
        for c in "hello" {
            if has(counts, c) { counts[c] += 1; } else { counts[c] = 1; }
        }
        prinb(counts);
    "#;
    assert_eq!(
        run(src),
        lines(&[
            "Dictionary<string, int>",
            "30",
            "3",
            "true",
            "true",
            "false",
            "[ann, cid]",
            "[31, 40]",
            "ann=31",
            "cid=40",
            "{ann: 31, cid: 40}",
            "double",
            "one",
            "true",
            "{h: 1, e: 1, l: 2, o: 1}",
        ])
    );
}

#[test]
fn dictionary_errors() {
    compile_error(
        "var d = {\"a\": 1}; prinb(d[1]);",
        "cannot use int as string",
    );
    compile_error(
        "var d = {\"a\": 1}; d[\"b\"] = \"x\";",
        "cannot use string as int",
    );
    compile_error(
        "Dictionary<double, int> d = {};",
        "cannot be a dictionary key",
    );
    compile_error("var d = {1.5: 1};", "cannot be a dictionary key");
    compile_error("Dictionary<int> d = {};", "needs 2 type arguments");
    compile_error("var d = {\"a\": 1}; has(d, 1);", "second argument of has()");
    compile_error("prinb(keys([1]));", "expects a dictionary");
    let e = err("var d = {\"a\": 1}; prinb(d[\"zed\"]);");
    assert_eq!(e.kind, ErrorKind::Runtime);
    assert!(e.message.contains("key not found: zed"), "{e}");
}

#[test]
fn generic_functions() {
    let src = r#"
        fn first<T>(items: T[]): T { return items[0]; }
        fn pair<T>(a: T, b: T): T[] { return [a, b]; }
        fn show<T>(x: T): string { return "<" + str(x) + ">"; }
        fn get_or<K, V>(m: Dictionary<K, V>, k: K, fallback: V): V {
            if has(m, k) { return m[k]; }
            return fallback;
        }
        fn count<T>(items: T[]): int {
            if len(items) == 0 { return 0; }
            pop(items);
            return 1 + count(items);
        }

        var x = first([10, 20]);
        prinb(type(x));
        prinb(x + 1);
        prinb(first(["a", "b"]) + "!");
        prinb(pair(1, 2.5));
        double[] p = pair(1.0, 2);
        prinb(type(p[0]));
        prinb(show(3));
        prinb(show([1, 2]));
        prinb(get_or({"a": 1}, "b", 0));
        prinb(get_or({"a": 1}, "a", 0) * 10);
        prinb(count([1, 2, 3]));
    "#;
    assert_eq!(
        run(src),
        lines(&["int", "11", "a!", "[1, 2.5]", "double", "<3>", "<[1, 2]>", "0", "10", "3"])
    );
}

#[test]
fn generic_errors() {
    // As in C#, the result type comes from the arguments, not the target.
    compile_error(
        "fn pair<T>(a: T, b: T): T[] { return [a, b]; } double[] p = pair(1, 2);",
        "cannot use int[] as double[]",
    );
    compile_error(
        "fn pair<T>(a: T, b: T): T[] { return [a, b]; } pair(1, \"s\");",
        "cannot use string as int in argument 2 of 'pair'",
    );
    compile_error(
        "fn bad<T>(x: T): T { return x + 1; }",
        "cannot apply '+' to T and int",
    );
    compile_error(
        "fn first<T>(items: T[]): T { return items[0]; } string s = first([1]);",
        "cannot use int as string",
    );
    compile_error("fn f(x: T) { }", "unknown type 'T'");
    compile_error("fn f<T>(x: T): int { return x; }", "cannot use T as int");
    compile_error("fn len(x) { }", "built-in function and cannot be redefined");
    assert_eq!(err("fn f<T, T>() { }").kind, ErrorKind::Syntax);
}

#[test]
fn nested_collections_drop_without_recursion() {
    let src = r#"
        var d: any = {"k": 1};
        var i = 0;
        while i < 100000 {
            Dictionary<string, any> next = {"k": d};
            d = next;
            any[] wrapper = [d];
            d = wrapper;
            i += 1;
        }
        prinb("ok");
    "#;
    assert_eq!(run(src), "ok\n");
}
