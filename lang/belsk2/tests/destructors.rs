//! Constructors named like their type, destructors (`~Name()`) and the
//! garbage collector.

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

#[test]
fn destructors_run_when_objects_are_freed() {
    let src = include_str!("destructors.belsk2");
    assert_eq!(
        run(src),
        "open a\nusing a\nclose a\nafter use\nopen b\nclose b\nafter null\nopen c1\nopen c2\ncycle dropped\nclose c1\nclose c2\n2\nopen l\nlogged l\nclose l\n6\nend\n"
    );
}

#[test]
fn the_collector_frees_many_cycles() {
    // Every node points at itself; the collector runs on its own after
    // enough objects and also when gc() is called.
    let src = "var freed = 0;
    class Node { next: Node = null; ~Node() { freed += 1; } }
    fn main() {
        var i = 0;
        while i < 25000 { var n = Node(); n.next = n; i += 1; }
        gc();
        prinb(freed);
    }";
    assert_eq!(run(src), "25000\n");
}

#[test]
fn live_objects_survive_a_collection() {
    let src = "class Node { next: Node = null; value: int = 0; }
    fn main() {
        var a = Node();
        a.next = Node();
        a.next.next = a;
        a.value = 7;
        Node[] list = [Node()];
        list[0].next = list[0];
        prinb(gc());
        prinb(a.next.next.value);
        prinb(list[0].next == list[0]);
    }";
    assert_eq!(run(src), "0\n7\ntrue\n");
}

#[test]
fn constructor_and_destructor_errors() {
    let e = belsk2::compile("class A { init() {} }")
        .err()
        .unwrap_or_default();
    assert!(
        e.iter().any(|e| e.message.contains("named like its type")),
        "{e:?}"
    );
    assert_error("struct S { ~S() {} }", "only classes have a destructor");
    assert_error("class A { ~A() {} ~A() {} }", "already has a destructor");
    assert_error(
        "class A { A() {} A(x: int) {} }",
        "already has a constructor",
    );
}
