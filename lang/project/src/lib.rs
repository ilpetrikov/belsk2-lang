//! Projects, modules and dependencies.
//!
//! A project is a directory with a [`MANIFEST`] (`belsk2.toml`) and a `src`
//! directory. `src/main.belsk2` is the entry point of a program,
//! `src/lib.belsk2` the root of a library. Every other `.belsk2` file under
//! `src` is a module: `src/utils.belsk2` is `utils`, `src/net/http.belsk2`
//! is `net.http`. A file imports a module with `using utils;`; the first
//! part of the path may also name a dependency (`using json;` for its
//! `lib.belsk2`, `using json.parser;` for its `src/parser.belsk2`).
//!
//! [`load`] reads a project and everything it uses and returns the parsed
//! modules in dependency order.

pub mod git;
mod loader;
pub mod lockfile;
pub mod manifest;

use std::path::{Path, PathBuf};

pub use loader::{
    display_path, load, load_file, load_file_lenient, load_lenient, load_with, Loaded, Mode,
    Module, ModuleId, Options, Package,
};
pub use lockfile::{Lockfile, LOCKFILE};
pub use manifest::{module_name, valid_package_name, Dependency, Manifest, PackageInfo, MANIFEST};

/// Looks for `belsk2.toml` in `start` and its parent directories.
pub fn find_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join(MANIFEST).is_file())
        .map(Path::to_path_buf)
}

/// The kind of project `belsk2 new` creates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Template {
    /// A program with `src/main.belsk2`.
    Bin,
    /// A library with `src/lib.belsk2`.
    Lib,
}

/// Creates a new project in `dir` (which may exist but must not contain a
/// `belsk2.toml`). Returns the files written.
pub fn create(
    dir: &Path,
    name: &str,
    template: Template,
) -> Result<Vec<PathBuf>, belsk2_syntax::Error> {
    use belsk2_syntax::Error;
    if !valid_package_name(name) {
        return Err(Error::project(format!(
            "invalid package name \"{name}\": use lowercase letters, digits, '-' and '_', starting with a letter"
        )));
    }
    let manifest_path = dir.join(MANIFEST);
    if manifest_path.exists() {
        return Err(Error::project(format!(
            "{} already exists",
            manifest_path.display()
        )));
    }
    let io =
        |path: &Path, e: std::io::Error| Error::io(format!("cannot write {}: {e}", path.display()));
    let src = dir.join("src");
    std::fs::create_dir_all(&src).map_err(|e| io(&src, e))?;

    let mut files = Vec::new();
    let mut write = |path: PathBuf, text: String| -> Result<(), Error> {
        std::fs::write(&path, text).map_err(|e| io(&path, e))?;
        files.push(path);
        Ok(())
    };
    write(manifest_path, Manifest::new(name).to_toml())?;
    match template {
        Template::Bin => write(
            src.join("main.belsk2"),
            format!("// Entry point of {name}.\n\nfn main() {{\n    prinb(\"Hello from {name}!\");\n}}\n"),
        )?,
        Template::Lib => write(
            src.join("lib.belsk2"),
            format!(
                "// The {name} library. Other projects use it with `using {};`.\n\n\
                 fn greet(who: string): string {{\n    return \"Hello, \" + who + \"!\";\n}}\n",
                module_name(name)
            ),
        )?,
    }
    let gitignore = dir.join(".gitignore");
    if !gitignore.exists() {
        write(gitignore, "/target\n".to_string())?;
    }
    Ok(files)
}
