# belsk2

**belsk2** is an experimental programming language with a unique syntax.

```belsk2
fn fib(n: int): int {
    if n <= 1 { return n; }
    return fib(n - 1) + fib(n - 2);
}

for i in [0, 1, 2, 3, 4, 5] {
    prinb(fib(i));
}
```

## Repository layout

| Path | What it is |
|---|---|
| `lang/syntax` | `belsk2-syntax`: lexer, parser, AST, error type |
| `lang/interp` | `belsk2-interp`: the interpreter and built-in functions |
| `lang/belsk2` | `belsk2`: the main crate that re-exports everything (use this from Rust) |
| `cli` | `belsk2` command-line tool |
| `ide` | the belsk2 IDE |
| `examples` | example programs (`*.belsk2`) |

## Building

Requires [Rust](https://rustup.rs) 1.80 or newer.

```sh
cargo build --release
```

The `belsk2` binary ends up in `target/release/`.

## Usage

```sh
belsk2 program.belsk2        # run a file
belsk2 run program.belsk2    # same
belsk2 check program.belsk2  # syntax check only
belsk2 repl                  # interactive session (also: `belsk2` with no arguments)
```

Errors point at the exact place in the source:

```text
runtime error: division by zero
 --> main.belsk2:2:9
  |
2 | prinb(x / 0);
  |         ^
```

## Using belsk2 from Rust

```rust
use belsk2::{Interpreter, Value};

let mut interp = Interpreter::new();
let mut out = Vec::new();
interp.run_source_with_writer("fn sq(x: int): int { return x * x; }", &mut out)?;
let v = interp.call_function("sq", &[Value::from(7.0)], &mut out)?;
assert_eq!(v, Value::from(49.0));
```

Nothing in the toolchain panics: every failure is returned as a `belsk2::Error`
with a kind (`Syntax`, `Type`, `Runtime`, `Io`), a message and a source location.

## IDE

```sh
cd ide
npm install
npm run build:cli   # builds the CLI and copies it into ide/bin
npm start
```

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
```

## Supported Platforms

![Windows 11](https://img.shields.io/badge/Windows-11-0078D4?logo=windows&logoColor=blue)

## License

MIT
