//! The checker must never panic on any program the parser accepts.

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        let i = (self.next() % items.len() as u64) as usize;
        items.get(i).copied().unwrap_or("")
    }
}

const STMTS: &[&str] = &[
    "var a = 1;",
    "var s = \"x\";",
    "var arr = [1, \"two\", [3]];",
    "int i = 2;",
    "bel b = 999;",
    "ster t = \"t\";",
    "a = s;",
    "a += 1;",
    "s -= 1;",
    "arr[0] = arr;",
    "arr[s] = 1;",
    "prinb(a + s);",
    "prinb(len(a));",
    "prinb(num(\"x\"), 1);",
    "push(arr);",
    "fn f(x: int, y) { return x - y; }",
    "fn g(): string { return f(1); }",
    "f(s, s);",
    "g()(1);",
    "for c in s { prinb(c); }",
    "for c in a { }",
    "while a < s { a = a + 1; }",
    "if arr { var a = 2; } else { s = 3; }",
    "{ var z = -s; z = !z; }",
    "idb 1 = f;",
    "prinb(arr.length);",
    "prinb(missing);",
    "var x = null;",
    "x = len;",
    "a = 1 / 0;",
];

#[test]
fn random_programs() {
    let mut rng = Rng(7);
    for _ in 0..20_000 {
        let n = (rng.next() % 12) as usize;
        let src: Vec<&str> = (0..n).map(|_| rng.pick(STMTS)).collect();
        if let Ok(program) = belsk2_syntax::parse(&src.join("\n")) {
            let _ = belsk2_typeck::check(&program);
        }
    }
}

#[test]
fn deeply_nested_program() {
    let depth = 200;
    let src = format!(
        "{}prinb(1 - \"a\");{}",
        "if true { ".repeat(depth),
        " }".repeat(depth)
    );
    let program = belsk2_syntax::parse(&src);
    assert!(program.is_ok_and(|p| belsk2_typeck::check(&p).len() == 1));
}
