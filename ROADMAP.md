# Roadmap

## Done

- [x] Workspace layout (`lang/`, `cli/`, `ide/`), Go version removed, panic-free toolchain
- [x] Interpreter bug fixes (scopes, arrays by reference, UTF-8 strings, ...)
- [x] Static checker: type and arity errors at compile time
- [x] C# numeric types: `sbyte`..`ulong`, `float`, `double`, `char`, bitwise operators
- [x] Typed arrays `T[]`, `List<T>`, `Dictionary<K, V>`, generic functions
- [x] Explicit type arguments `f<int>(x)`; generics are reified at run time

## Next

### Projects and build
- [x] `belsk2.toml` (name, version, ...), `belsk2 new`/`init`/`run`/`check`/`build`, modules (`using`), path dependencies
- [x] All code in functions; programs start in `fn main()` (args, exit code)
- [x] Dependencies from git (`branch`/`tag`/`rev`), `belsk2.lock`, `belsk2 update`, `--locked`, `--offline`
- [x] Formatter: `belsk2 fmt` (`--check`, stdin), `belsk2fmt.toml`, LSP and IDE formatting
- [ ] WASM compiler. Decided: `wasm-encoder` + `wasmparser`; **WasmGC** for strings,
      arrays, dictionaries and later classes; generics by **monomorphization**;
      **no I/O** in WASM builds (`prinb`/`input`/`reab`/`idb` are compile errors);
      tests run exported functions in `wasmtime` and compare with the interpreter.
      Stages: W1 numbers/functions/control flow/globals; W2 strings, arrays, generics;
      W3 components; later dictionaries, `any`, `str(double)`.
  - [x] W1: `lang/wasm`, `belsk2 build <file>` writes a `.wasm`. Numbers with C#
        semantics, `bool`, `char`, functions, locals, globals (set by the start
        function), `if`/`while`/`break`/`continue`, conversions, `bel` checks (trap).
        Every top-level function is exported.
  - [x] W2: strings (GC arrays of code points), arrays (`{data, len}` structs that
        grow on `push`), `for`, `len`/`push`/`pop`/`substr`/`str` (not of floats),
        string and array equality, generics by monomorphization. Project builds:
        `belsk2 build` compiles every module the entry point needs into
        `bin/<package>.wasm` (`-o` picks another path); libraries export `lib.belsk2`.
  - [x] W3: components. `[component] world = "wit"` (a `.wit` file or a directory with
        `deps/`, optional `name` for the world) in `belsk2.toml`; `belsk2 build` writes a
        component. Exports are plain functions with matching names (`kebab-case` becomes
        `snake_case`) and signatures; world-level imports are callable everywhere.
        Canonical ABI glue for numbers, bool, char, string and `list<T>` (nested too):
        linear memory with a bump allocator reset after every export call.
  - [x] W4: interfaces. An imported interface is a module without source
        (`using canvas;`); an exported one is implemented in `src/<interface>.belsk2`.
        WIT packages come from `wit/deps/`. Functions whose WIT types Belsk2 lacks stay
        visible but are a compile error when used. `belsk2 bindgen` prints the world as
        Belsk2 declarations (`-o` to a file); `--stubs` adds missing exports.
  - [x] `belsk2 run` runs component projects in wasmtime (exported `run`/`main`, or
        `--call name -- args`) with printing stub imports; `--interpreted` for the
        interpreter. wasmtime is a default feature of the CLI (`--no-default-features`
        builds without it). MSRV is 1.98; raise it to 1.99 once that is released.
  - [ ] W5: records, enums, variants, option/result, resources (with struct/enum/class).
  - Known differences from the interpreter: runtime errors are traps without a message;
    float-to-integer conversions saturate at the `long` range (the interpreter wraps
    huge values); `%` on doubles uses `a - b * trunc(a / b)`, inexact for huge quotients;
    `pop` of an empty array traps (the interpreter returns `null`).
- [ ] Components (our own "wit-bindgen backend"): `[component] world = "wit/app.wit"` in
      `belsk2.toml`; exports are plain `fn`s with matching names, imports become functions
      usable via `using`; canonical ABI glue for numbers, bool, char, string, list<T>;
      packaged with `wit-component`. Records/variants come with struct/enum.
- [ ] WASI 0.2 / 0.3: not now (not needed yet; 0.3 is barely supported)

### Rust integration
- [ ] Calling Belsk2 from Rust (API, macros, wasmtime)
- [ ] IDE: hover and completion for WIT functions
- [ ] `belsk2 run --interpreted` for components: stub imports in the interpreter too
- [ ] Rust dependencies (crates.io / git) in `belsk2.toml`

### Language
- [ ] `struct`
- [ ] `enum` (including enums with data)
- [ ] `class`: fields, methods, constructors, `this`
- [ ] Inheritance, `virtual`/`override`, `base`
- [ ] `interface`
- [ ] Properties `get`/`set`, access modifiers, `static`
- [ ] `switch`/`case`
- [ ] `match` with patterns
- [ ] Lambdas and closures, function types like `fn(int): bool`
- [ ] LINQ

### IDE
- [x] `lang/analysis` (diagnostics, hover, completion, definition, references, rename, hints, module graph)
- [x] Language server `belsk2 lsp`
- [x] VS Code extension (`editors/vscode`): highlighting, snippets, LSP, Run/Check
- [x] Tauri 2 IDE skeleton: VS Code-like layout, Monaco, analysis in the editor,
      palette, quick open, projects, Run/Stop/Check with program input
- [x] IDE: delete to the trash, search in all files (Ctrl+Shift+F), emoji-safe columns
- [x] Example projects (`examples/projects`) and `playground/`
- [x] Themes (Belsk2 Night, Dark, Light, High Contrast), "islands" layout, nesting stripe, settings view
- [x] Module map (as a tab; modules / functions / variables views; drag and drop), live values, state inspector (module variables + idb)
- [ ] Module map: a "Classes" view once classes exist
- [x] Sessions per project, persistent output, recent projects, shortcuts

### Docs
- [ ] Documentation and the Belsk2 Book (English)

## Deferred (do not forget)

- [ ] `for k in dict` iterates over keys. In C# it yields `KeyValuePair<K, V>`
      with `.Key` and `.Value`. Switch to that once `struct` exists
      (and update `examples/collections.belsk2` and the tests).
- [ ] Function types (`fn(int): bool`) instead of the untyped `fn`; comes with lambdas.
- [ ] Access modifiers for modules: today every top-level function and variable is exported.
      Add `public`/`private` together with the class access modifiers.
- [ ] `belsk2.lock` stores a relative git URL (a local path) as an absolute path, so such a
      lock file is machine-specific. Fine for remote URLs.
- [ ] Two git URLs that are spelled differently but mean the same repository count as
      different sources.
- [ ] Analysis: completion after `.` is empty (no members yet; add with structs/classes);
      a file that does not parse only gets keyword/builtin/word completions.
- [ ] Publish the VS Code extension to the Marketplace / Open VSX.
