# belsk2

**belsk2** is an experimental programming language with a unique syntax.

```belsk2
fn fib(n: int): int {
    if n <= 1 { return n; }
    return fib(n - 1) + fib(n - 2);
}

fn main() {
    for i in [0, 1, 2, 3, 4, 5] {
        prinb(fib(i));
    }
}
```

All code lives in functions: a file holds declarations (`using`, `fn`,
variables, `idb`), and a program starts in `fn main()`. `main` may take the
command-line arguments (`fn main(args: string[])`, run with
`belsk2 run app.belsk2 -- one two`) and return an `int` exit code. Only the
REPL accepts statements outside functions.

## Repository layout

| Path | What it is |
|---|---|
| `lang/syntax` | `belsk2-syntax`: lexer, parser, AST, error type |
| `lang/typeck` | `belsk2-typeck`: static checker that runs before the program starts |
| `lang/project` | `belsk2-project`: `belsk2.toml`, modules and dependencies |
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
belsk2 new hello             # create a project (--lib for a library)
cd hello
belsk2 run                   # run the project
belsk2 check                 # check every file of the project, nothing runs
belsk2 build                 # check the project and its dependencies
belsk2 update                # move git dependencies to their newest commits

belsk2 run program.belsk2    # run a single file (also: `belsk2 program.belsk2`)
belsk2 check a.belsk2        # check single files
belsk2 repl                  # interactive session (also: `belsk2` with no arguments)
```

## Projects and modules

A project is a directory with a `belsk2.toml`:

```toml
[package]
name = "hello"
version = "0.1.0"

[dependencies]
utils = { path = "../utils" }
json = { git = "https://github.com/someone/json.belsk2", tag = "v1.0.0" }
```

Dependencies come from a local `path` or from `git` (with an optional
`branch`, `tag` or `rev`; the default branch otherwise). Git dependencies are
downloaded with the `git` command into `~/.belsk2/git` (set `BELSK2_HOME` to
change that), and the exact commit of each is recorded in `belsk2.lock`.
Commit that file: the project then builds the same way everywhere until
`belsk2 update` moves the dependencies forward. `--locked` makes any command
fail instead of changing `belsk2.lock` (useful in CI), and `--offline` uses
only what is already downloaded.

`src/main.belsk2` is where a program starts; a library has `src/lib.belsk2`.
Every other file under `src` is a module that other files import with
`using`:

```belsk2
using text.format;   // src/text/format.belsk2
using utils;         // the `utils` dependency (its src/lib.belsk2)
using utils.math;    // the dependency's src/math.belsk2

fn main() {
    prinb(shout("hi"));
}
```

A module's top-level functions and variables are visible to the files that
use it (imported variables are read-only). A module's top-level variables
are initialized once, before the modules that use it. A single file run with `belsk2 run`
can also use the files next to it.

Programs are checked before they run: wrong argument counts, type
mismatches (`1 - "a"`, `len(5)`, `num("abc")`), undefined names and so on are
compile errors, all reported at once. Errors point at the exact place in the
source:

```text
type error: len() expects a string or array, got number
 --> main.belsk2:2:11
  |
2 | prinb(len(5));
  |           ^
```

## Numbers

Belsk2 has the numeric types of C#, with the same rules:

| Type | Size | Literal |
|---|---|---|
| `sbyte` / `byte` | 8-bit | `byte b = 255;` |
| `short` / `ushort` | 16-bit | `short s = -5;` |
| `int` / `uint` | 32-bit | `42`, `42u`, `0xFF`, `0b1010`, `1_000_000` |
| `long` / `ulong` | 64-bit | `42L`, `42UL` |
| `float` | 32-bit floating point | `1.5f` |
| `double` | 64-bit floating point | `1.5`, `1e-9`, `2d` |
| `char` | a Unicode character | `s[0]`, `char c = 'x';` |

- Integer division truncates: `7 / 2 == 3`, `7 / 2.0 == 3.5`.
- Mixing types widens automatically (`int + long` is `long`, `int + double` is
  `double`). Narrowing needs an explicit conversion: `int(2.9)`, `byte(300)`,
  `long(x)`, `char(65)`.
- Integers wrap around on overflow, as in C# by default.
- Bitwise operators `& | ^ ~ << >>` and compound assignments
  (`+= -= *= /= %= &= |= ^= <<= >>=`) are available.
- Numbers, `bool` and `char` can never be `null`.
- Single-quoted literals are strings, as before; a one-character one can be
  used where a `char` is expected (`char c = 'x';`).

## Collections and generics

```belsk2
int[] xs = [1, 2, 3];          // the type of [1, 2, 3] is inferred as int[]
List<string> names = [];       // List<T> is another name for T[]
push(names, "ann");
double[] ds = [1, 2.5];        // ints convert to double

var ages = {"ann": 30};        // Dictionary<string, int>
ages["bob"] = 25;
if has(ages, "ann") { remove(ages, "ann"); }
prinb(keys(ages));             // [bob]

fn first<T>(items: T[]): T { return items[0]; }
int n = first([10, 20]);       // T is inferred from the argument
```

Arrays and dictionaries know their element types, so adding a wrong value
is a compile error (or, for values of type `any`, a run-time error). A
literal with mixed element types, such as `[1, "two"]`, is an `any[]`.

Variables declared with `var` take the type of their initial value, as in C#.
Use `any` (`var x: any = 1;`) for a variable that may hold different types;
such values are checked at run time.

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
