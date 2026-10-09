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
- [ ] WASM compiler (codegen, memory model, WASI 0.2)
- [ ] wit-bindgen backend: export/import WIT interfaces
- [ ] WASI 0.3, if the toolchain supports it

### Rust integration
- [ ] Calling Belsk2 from Rust (API, macros, wasmtime)
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
- [ ] Themes (dark, light, high contrast, Belsk2) and the "islands" look
- [ ] Module map, live values, idb inspector

### Docs
- [ ] Documentation and the Belsk2 Book (English)

## Deferred (do not forget)

- [ ] `for k in dict` iterates over keys. In C# it yields `KeyValuePair<K, V>`
      with `.Key` and `.Value`. Switch to that once `struct` exists
      (and update `examples/collections.belsk2` and the tests).
- [ ] Function types (`fn(int): bool`) instead of the untyped `fn`; comes with lambdas.
- [ ] `belsk2 build` only checks the project; it should produce output once the WASM backend exists.
- [ ] Access modifiers for modules: today every top-level function and variable is exported.
      Add `public`/`private` together with the class access modifiers.
- [ ] `belsk2.lock` stores a relative git URL (a local path) as an absolute path, so such a
      lock file is machine-specific. Fine for remote URLs.
- [ ] Two git URLs that are spelled differently but mean the same repository count as
      different sources.
- [ ] Analysis: completion after `.` is empty (no members yet; add with structs/classes);
      a file that does not parse only gets keyword/builtin/word completions.
- [ ] Publish the VS Code extension to the Marketplace / Open VSX.
- [ ] IDE: positions are sent as character columns; Monaco counts UTF-16 units, so lines with
      emoji (characters outside the BMP) get slightly shifted squiggles.
- [ ] IDE: no delete in the explorer yet (needs a trash/undo story); no search across files.
