//! The front end must never panic, whatever the input. These tests throw
//! large amounts of random and mutated input at the lexer and parser.

/// Small deterministic PRNG (xorshift) so the test needs no dependencies.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items.get(self.below(items.len())).copied().unwrap_or("")
    }
}

const PIECES: &[&str] = &[
    "var",
    "fn",
    "if",
    "else",
    "while",
    "for",
    "in",
    "return",
    "break",
    "continue",
    "idb",
    "int",
    "float",
    "string",
    "bool",
    "bel",
    "ster",
    "any",
    "true",
    "false",
    "null",
    "x",
    "y",
    "prinb",
    "0",
    "1",
    "3.14",
    "\"s\"",
    "'c'",
    "\"",
    "(",
    ")",
    "{",
    "}",
    "[",
    "]",
    ";",
    ",",
    ".",
    ":",
    "=",
    "==",
    "!=",
    "<",
    ">",
    "<=",
    ">=",
    "+",
    "-",
    "*",
    "/",
    "%",
    "+=",
    "-=",
    "&&",
    "||",
    "!",
    "->",
    "&",
    "|",
    "/*",
    "*/",
    "//",
    "\n",
    " ",
    "\\",
    "@",
    "é",
    "\u{0}",
    "^",
    "~",
    "<<",
    "<<=",
    ">>=",
    "*=",
    "1L",
    "2.5f",
    "0xFF",
    "0b1",
    "1e5",
    "1_0",
    "1e",
    "0x",
    "long",
    "double",
    "char",
    "byte",
    "int[]",
    "List<int>",
    "<",
    ">",
];

#[test]
fn random_token_soup() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..20_000 {
        let len = rng.below(40);
        let src: String = (0..len)
            .map(|_| rng.pick(PIECES))
            .collect::<Vec<_>>()
            .join(" ");
        let _ = belsk2_syntax::parse(&src);
        let joined: String = (0..len).map(|_| rng.pick(PIECES)).collect();
        let _ = belsk2_syntax::parse(&joined);
    }
}

#[test]
fn random_chars() {
    let mut rng = Rng(42);
    for _ in 0..20_000 {
        let len = rng.below(60);
        let src: String = (0..len)
            .map(|_| char::from_u32((rng.next() % 0x3000) as u32).unwrap_or('?'))
            .collect();
        let _ = belsk2_syntax::parse(&src);
    }
}

#[test]
fn truncated_valid_programs() {
    let program = r#"
        fn fib(n: int): int { if n <= 1 { return n; } else if n == 2 { return 1; } return fib(n - 1) + fib(n - 2); }
        var a = [1, 2, [3, 4]]; a[2][0] += 1; idb 3 = "x";
        for i in a { while i < 3 { i = i + 1; if i == 2 { break; } else { continue; } } }
        prinb(-a[0] * (2 + 3) / 4 % 5 >= 1 && !false || "s" != 'c');
    "#;
    let chars: Vec<char> = program.chars().collect();
    for end in 0..=chars.len() {
        let prefix: String = chars.iter().take(end).collect();
        let _ = belsk2_syntax::parse(&prefix);
    }
    assert!(belsk2_syntax::parse(program).is_ok());
}

#[test]
fn pathological_nesting() {
    for piece in [
        "(",
        "[",
        "{",
        "-",
        "!",
        "if x ",
        "fn f() {",
        "a(",
        "a[",
        "else if x {",
    ] {
        let src = piece.repeat(100_000);
        assert!(belsk2_syntax::parse(&src).is_err());
    }
}
