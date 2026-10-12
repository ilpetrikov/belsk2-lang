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
| `lang/analysis` | `belsk2-analysis`: diagnostics, hover, completion, navigation for editors |
| `lang/fmt` | `belsk2-fmt`: the code formatter (`belsk2 fmt`, `belsk2fmt.toml`) |
| `lang/wasm` | `belsk2-wasm`: the WebAssembly backend (`belsk2 build <file>`) |
| `lang/lsp` | `belsk2-lsp`: the language server (`belsk2 lsp`) |
| `editors/vscode` | the VS Code extension |
| `cli` | `belsk2` command-line tool |
| `ide` | the Belsk2 IDE (Tauri 2 + Monaco) |
| `examples` | example programs (`*.belsk2`) and projects (`examples/projects`) |
| `playground` | a project to try things in: open it in the IDE and press F5 |

## Building

Requires [Rust](https://rustup.rs) 1.98 or newer.

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

## Structs

```belsk2
struct Point {
    x: double;
    y: double;
    label: string = "";              // a field with a value

    fn len2(): double { return x * x + y * y; }
    fn moveBy(dx: double, dy: double) { x += dx; y += dy; }
}

fn main() {
    var p = Point(3, 4);             // the fields without a value, in order
    p.moveBy(1, 1);                  // methods may change the struct
    var q = p;                       // a copy: structs are values
    q.x = 0;
    prinb(p);                        // Point { x: 4, y: 5, label:  }
    prinb(p == Point(4, 5));         // true: fields are compared
}
```

Inside a method the fields and other methods can be used directly or
through `this` (`this.x`). A struct may declare a constructor instead of
the field-by-field one, named like the type as in C#:
`Point(x: double) { this.x = x; }` (fields not set
there keep their value or the default: `0`, `false`, `null`). Fields can
also be written C#-style (`double x;`) or with `var` (`var count = 0;`).

Structs are values, as in C#: assigning or passing one copies it, and
`a.b.x = 1` changes the copy stored in `a`. A field of a temporary value
(`make().x = 1`) cannot be assigned. A struct cannot contain itself.
Structs are not yet supported by the WASM backend.

## Enums

```belsk2
enum PerformanceMode {
    Low = 1,          // a case may stand for a number of your choice
    Balanced = 5,
    High,             // the previous one plus 1: 6

    fn fast(): bool { return this == PerformanceMode.High; }
}

var m = PerformanceMode.High;   // of type PerformanceMode, not int
prinb(m);                       // High
prinb(int(m));                  // 6
prinb(m.fast());                // true
```

Without numbers the cases count from 0, as in C#. Enum values can be
compared with `==`, used as dictionary keys and converted to numbers with
`int(...)`, `long(...)` and so on, but a number is not an enum value. A
field of an enum type starts as its first case. An enum may have methods.

## Classes

```belsk2
class Player {
    name: string;
    hp: int = 100;
    friend: Player = null;

    fn hit(n: int) { hp -= n; }
}

fn damage(p: Player) { p.hit(30); }

fn main() {
    var a = Player("Ann");
    var b = a;          // the same player: classes are references
    damage(b);
    prinb(a.hp);        // 70
}
```

A class has fields, methods, `this` and the constructor like a struct, but it is a
reference type, as in C#: copies share one object, changes through any of
them are seen by all, a variable of a class type may be `null`, and `==`
asks whether two values are the same object.

## Properties, static members and access

```belsk2
class Player {
    static count: int = 0;              // one for the whole type: Player.count

    Name: string { get; private set; }  // an auto-property: stored like a field
    Id: int { get; }                    // read-only: set by its value or the constructor
    private hp: int = 100;

    Hp: int {                           // a property with code
        get { return hp; }
        set { if value < 0 { hp = 0; } else { hp = value; } }
    }

    Player(name: string) { Name = name; count += 1; Id = count; }

    static fn create(name: string): Player { return Player(name); }
}

var p = Player.create("Ann");
p.Hp -= 30;                             // runs get, then set
prinb(Player.count);
```

Members are `public` unless marked `private` (only the type itself may use
them) or `protected` (the type and, once inheritance exists, the types
derived from it); a property's `set` (or `get` for a whole property) can
have its own modifier, and `a private constructor` keeps others from creating
values. `static` fields, properties and methods belong to the type and are
used through it (`Player.count`); inside the type they can be used by
name, but there is no `this` in them. Static fields get their values when
the declaration of the type is reached.

At the top level, `private fn`, `private var` and `private struct` (or
`enum`, `class`) are visible only in their own module; everything else is
`public`, as before. A WASM module exports only public functions.

## Inheritance and interfaces

```belsk2
interface INamed {
    Name: string { get; }               // implemented by a field or property
    fn describe(): string;
}

abstract class Animal : INamed {
    Name: string { get; }
    protected legs: int = 4;            // seen by Animal and derived classes

    Animal(name: string) { Name = name; }

    abstract fn speak(): string;        // no body: every class below overrides it
    virtual fn describe(): string { return Name + " says " + speak(); }
}

class Dog : Animal {
    Dog(name: string) : base(name) {}   // the base class's constructor runs first
    override fn speak(): string { return "woof"; }
    override fn describe(): string { return base.describe() + "!"; }
}

fn main() {
    Animal[] zoo = [Dog("Rex")];
    for a in zoo { prinb(a.describe()); }   // Rex says woof!
    var a: Animal = Dog("Ace");
    prinb(a is Dog);                    // true
    var d = a as Dog;                   // null if it is not a Dog
}
```

As in C#: a class has at most one base class (written first) and any
number of interfaces; structs and enums can implement interfaces too. A
method replaces an inherited one only if that one is `virtual` (or
`abstract`, or an `override` itself) and the new one says `override`, with
the same parameter and return types; calls go to the method of the
object's real class, and `base.m()` calls the base class's version. An
`abstract class` cannot be created. An interface lists methods and
properties (`Name: T { get; }` or `{ get; set; }`); a type implementing it
must have all of them, public. Interfaces can extend other interfaces.
Without `: base(...)`, the base class's the constructor is called without arguments
(an error if it needs some). `x is T` tests the type, and `x as T` gives
`x` as a `T` or `null`.

## Destructors and the garbage collector

```belsk2
class File {
    name: string;
    File(name: string) { this.name = name; prinb("open " + name); }
    ~File() { prinb("close " + name); }   // runs when the object is freed
}
```

Memory is managed automatically. An object is freed as soon as nothing
refers to it any more, and its destructor runs right after the statement
that dropped the last reference (destructors of base classes run after the
class's own). Objects that only refer to each other in a cycle are found by
a garbage collector, which runs every 10,000 new objects; `gc()` runs it
now and returns how many objects it freed. Only classes have destructors.

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
with a kind (`Syntax`, `Type`, `Runtime`, `Io`, `Project`, `Compile`), a message and a source location.

## Formatting

`belsk2 fmt` formats the project (or the files and folders you name);
`belsk2 fmt --check` only reports files that need it (useful in CI) and
`belsk2 fmt -` formats stdin. Comments and blank lines are kept. The IDE
(Shift+Alt+F, optional format on save) and editors using `belsk2 lsp` use
the same formatter.

Settings live in `belsk2fmt.toml` (or `.belsk2fmt.toml`) in the file's
folder or any folder above it. Every key is optional:

```toml
indent_style = "spaces"        # or "tabs"
indent_width = 4
brace_style = "same_line"      # or "next_line" (C# style)
max_blank_lines = 1
space_around_operators = true
line_ending = "auto"           # "lf", "crlf" or "auto" (keep the file's)
trailing_newline = true
```

## WebAssembly

`belsk2 build` compiles the current project to `bin/<package>.wasm`, and
`belsk2 build app.belsk2` compiles a single file to `app.wasm`; `-o` picks
another path. The functions of `main.belsk2` (or `lib.belsk2` for a library)
are exported under their own names, so the module can be used from any
WebAssembly host with the GC proposal:

```belsk2
fn fib(n: int): int {
    if n < 2 {
        return n;
    }
    return fib(n - 1) + fib(n - 2);
}
```

There are two targets, chosen with `--target` or in `belsk2.toml`:

```toml
[build]
target = "wasi0.2"   # or "wasm32" (the default)
```

- `wasm32`: a plain module (or a component, see below) with no input or
  output: `prinb`, `input`, `reab` and `idb` are compile errors.
- `wasi0.2`: a [WASI 0.2](https://wasi.dev) component. `prinb` and `input`
  use stdout and stdin, and `fn main()` is exported as `wasi:cli/run`, so
  the result runs with `wasmtime run app.wasm`. `belsk2 run --target
  wasi0.2` (or a project whose `[build] target` is `wasi0.2`) builds the
  program and runs it in wasmtime. `reab` and `idb` are not supported yet. Parameters and returned values need declared types. Strings,
arrays, generics and all statements are supported; dictionaries, `any` and
`str()` of floating-point numbers are not yet (see ROADMAP.md).

### Components

A project becomes a [WebAssembly component](https://component-model.bytecodealliance.org/)
when `belsk2.toml` names a WIT world:

```toml
[component]
world = "wit"        # a .wit file or a directory (with deps/)
```

```wit
package demo:app;

world app {
    import log: func(msg: string);
    export greet: func(name: string) -> string;
}
```

```belsk2
fn greet(name: string): string {
    log("greeting " + name);
    return "Hi, " + name;
}
```

Every export needs a function with the same name (`kebab-case` becomes
`snake_case`) and matching types; imports can be called from any module.
Interfaces work like modules: an imported interface (from the world's
package or from `wit/deps/`) is used with `using canvas;`, and an exported
interface is implemented in `src/<interface>.belsk2`. `belsk2 bindgen`
prints the whole world as Belsk2 declarations, and `belsk2 bindgen --stubs`
adds stubs for exports that are not implemented yet.
Supported WIT types: integers, `f32`, `f64`, `bool`, `char`, `string` and
`list<T>`.

`belsk2 run` builds a component project and runs it in wasmtime. It calls
the exported `run` (or `main`); `belsk2 run --call greet -- Ann` calls any
other export (`--call hooks.update` for one of an exported interface),
with arguments parsed by their WIT types (lists as `1,2,3`). There is no
real host, so imports are stubs that print the call
(`[host] canvas.draw("hi", 3)`) and return a default value. `belsk2 run
--interpreted` runs the same way in the interpreter (also with `--call`),
with the same stub imports.

## Editor support

`belsk2 lsp` is a language server: errors as you type, hover types,
completion, go to definition, references, rename and inline type hints, for
single files and whole projects. The VS Code extension in `editors/vscode`
uses it (see its README to build and install it); Zed, Neovim, Helix and
other LSP clients can run `belsk2 lsp` directly.

## IDE

`ide/` is the Belsk2 IDE, built with Tauri 2 and Monaco: explorer, tabs,
errors as you type, hover, completion, go to definition, rename, type hints,
command palette (Ctrl+Shift+P), quick open (Ctrl+P), Run (F5) with program
input, Stop, Check and projects.

```sh
cd ide
npm install
npm run app      # development build with hot reload
npm run bundle   # installer in ide/src-tauri/target/release/bundle
```

`npm run dev` alone opens the UI in a browser with a small demo backend.

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
```

## Supported Platforms

![Windows 11](https://img.shields.io/badge/Windows-11-0078D4?logo=windows&logoColor=blue)

## License

MIT
