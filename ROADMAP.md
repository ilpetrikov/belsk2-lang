# Roadmap

## Done

- [x] Workspace layout (`lang/`, `cli/`, `ide/`), Go version removed, panic-free toolchain
- [x] Interpreter bug fixes (scopes, arrays by reference, UTF-8 strings, ...)
- [x] Static checker: type and arity errors at compile time
- [x] C# numeric types: `sbyte`..`ulong`, `float`, `double`, `char`, bitwise operators
- [x] Typed arrays `T[]`, `List<T>`, `Dictionary<K, V>`, generic functions

## Next

### Projects and build
- [ ] `belsk2.toml` (name, version, ...), `belsk2 new`, `belsk2 run` for a project
- [ ] Dependencies from git, lock file
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
- [ ] Rewrite the IDE on Tauri: themes, VS Code-like layout with a distinctive
      feature of its own, compile errors shown in the editor

### Docs
- [ ] Documentation and the Belsk2 Book (English)

## Deferred (do not forget)

- [ ] `for k in dict` iterates over keys. In C# it yields `KeyValuePair<K, V>`
      with `.Key` and `.Value`. Switch to that once `struct` exists
      (and update `examples/collections.belsk2` and the tests).
- [ ] Explicit type arguments at call sites: `first<int>(xs)`. Today `T` is
      only inferred from the arguments. Needed when there is nothing to infer
      from, e.g. `fn empty<T>(): T[]`.
- [ ] Function types (`fn(int): bool`) instead of the untyped `fn`; comes with lambdas.
