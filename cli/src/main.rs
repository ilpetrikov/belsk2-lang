#[cfg(feature = "wasmtime")]
mod host;

use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use belsk2::project::{self, Loaded, Mode, Template};
use belsk2::{Error, Interpreter, SourceMap};
use belsk2_wasm::Target;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "belsk2",
    version = belsk2::VERSION,
    about = "The Belsk2 programming language",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Do not use the network: only dependencies that are already downloaded
    #[arg(long, global = true)]
    offline: bool,

    /// Fail if belsk2.lock would change (useful in CI)
    #[arg(long, global = true)]
    locked: bool,

    /// Program to run (shorthand for `belsk2 run <FILE>`)
    file: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new project in a new directory
    New {
        /// Project name (also the directory name)
        name: String,
        /// Create a library instead of a program
        #[arg(long)]
        lib: bool,
    },
    /// Create a new project in the current directory
    Init {
        /// Project name (defaults to the directory name)
        #[arg(long)]
        name: Option<String>,
        /// Create a library instead of a program
        #[arg(long)]
        lib: bool,
    },
    /// Run a file, or the current project when no file is given
    Run {
        /// Source file; `-` reads from stdin
        file: Option<PathBuf>,
        /// Arguments passed to `main(args: string[])` (or to the function
        /// given with --call)
        #[arg(last = true)]
        args: Vec<String>,
        /// Run a component project in the interpreter instead of wasmtime
        #[arg(long)]
        interpreted: bool,
        /// Component projects: the exported function to call (default: `run`
        /// or `main`); `interface.name` for functions of exported interfaces
        #[arg(long)]
        call: Option<String>,
        /// Build for this target and run it in wasmtime (wasi0.2 runs any
        /// program with `fn main()`)
        #[arg(long)]
        target: Option<String>,
    },
    /// Check the current project (or the given files) without running it
    Check {
        /// Source files
        files: Vec<PathBuf>,
    },
    /// Compile the current project (or a file) to WebAssembly
    Build {
        /// Source file to compile instead of the project
        file: Option<PathBuf>,
        /// Where to write the module (default: bin/<package>.wasm in the
        /// project, or next to the file)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// What to build: wasm32 (default) or wasi0.2. Default: `[build]
        /// target` in belsk2.toml
        #[arg(long)]
        target: Option<String>,
    },
    /// Move git dependencies to the newest commit of their branch or tag
    /// and rewrite belsk2.lock
    Update,
    /// Start an interactive session
    Repl,
    /// Format files (the whole project when none are given). Settings come
    /// from the nearest belsk2fmt.toml
    Fmt {
        /// Files or folders to format; `-` formats stdin to stdout
        files: Vec<PathBuf>,
        /// Only check: list unformatted files and fail if there are any
        #[arg(long)]
        check: bool,
    },
    /// Start the language server (for editors), talking LSP over stdin/stdout
    Lsp,
    /// Show the project's WIT world as Belsk2 declarations
    Bindgen {
        /// Add stubs for exported functions that are not implemented yet
        #[arg(long)]
        stubs: bool,
        /// Write the declarations to a file instead of printing them
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let options = project::Options {
        offline: cli.offline,
        locked: cli.locked,
        ..project::Options::default()
    };
    let result = match (cli.command, cli.file) {
        (
            Some(Command::Run {
                file,
                args,
                interpreted,
                call,
                target,
            }),
            _,
        ) => run(
            file,
            &args,
            RunMode {
                interpreted,
                call,
                target,
            },
            &options,
        ),
        (None, Some(file)) => run_file(&file, &[], &options),
        (Some(Command::Check { files }), _) if files.is_empty() => check_project(&options),
        (Some(Command::Check { files }), _) => check_files(&files),
        (
            Some(Command::Build {
                file: None,
                output,
                target,
            }),
            _,
        ) => build_project(output, target, &options),
        (
            Some(Command::Build {
                file: Some(file),
                output,
                target,
            }),
            _,
        ) => build_wasm(&file, output, target),
        (Some(Command::Update), _) => update(&options),
        (Some(Command::New { name, lib }), _) => new_project(&PathBuf::from(&name), &name, lib),
        (Some(Command::Init { name, lib }), _) => init(name, lib),
        (Some(Command::Repl), _) => repl(),
        (Some(Command::Fmt { files, check }), _) => fmt(&files, check),
        (Some(Command::Bindgen { stubs, output }), _) => bindgen(stubs, output, &options),
        (Some(Command::Lsp), _) => belsk2_lsp::run().map_err(|e| eprintln!("belsk2 lsp: {e}")),
        (None, None) if io::stdin().is_terminal() => repl(),
        (None, None) => run_file(Path::new("-"), &[], &options).map(|_| ()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => ExitCode::FAILURE,
    }
}

/// The process exit code for what `main` returned.
fn exit_code(code: Option<i64>) -> Result<(), ()> {
    match code {
        None | Some(0) => Ok(()),
        Some(code) => {
            let _ = io::stdout().flush();
            std::process::exit(i32::try_from(code).unwrap_or(1));
        }
    }
}

/// Prints errors to stderr with source snippets.
fn report(errors: &[Error], sources: &SourceMap) {
    let _ = io::stdout().flush();
    for e in errors {
        eprintln!("{}", e.render_in(sources));
    }
    if errors.len() > 1 {
        eprintln!("{} errors", errors.len());
    }
}

/// Checks and runs a loaded program.
fn check_and_run(
    loaded: Result<Loaded, (Vec<Error>, SourceMap)>,
    args: &[String],
) -> Result<(), ()> {
    let mut loaded = loaded.map_err(|(errors, sources)| report(&errors, &sources))?;
    let errors = project::check(&mut loaded);
    if !errors.is_empty() {
        report(&errors, &loaded.sources);
        return Err(());
    }
    let mut interp = Interpreter::new();
    let code = project::run(&loaded, &mut interp, args, &mut io::stdout())
        .map_err(|e| report(&[e], &loaded.sources))?;
    exit_code(code)
}

fn run_file(file: &Path, args: &[String], options: &project::Options) -> Result<(), ()> {
    if file.is_dir() {
        if !file.join(project::MANIFEST).is_file() {
            eprintln!(
                "project error: no {} in {}",
                project::MANIFEST,
                file.display()
            );
            return Err(());
        }
        return check_and_run(project::load_with(file, Mode::Run, options), args);
    }
    if file != Path::new("-") {
        return check_and_run(project::load_file(file), args);
    }
    let mut source = String::new();
    if let Err(e) = io::stdin().read_to_string(&mut source) {
        eprintln!("io error: cannot read stdin: {e}");
        return Err(());
    }
    let mut interp = Interpreter::new();
    // stdin holds the program itself, so `reab`/`input` get no input.
    interp.set_input(io::empty());
    let program = interp
        .compile(&source)
        .map_err(|errors| report_text(&errors, &source, "<stdin>"))?;
    let mut out = io::stdout();
    interp
        .run_program(&program, &mut out)
        .map_err(|e| report_text(&[e], &source, "<stdin>"))?;
    let Some(main) = interp.get_global("main") else {
        eprintln!("type error: the program has no `fn main()`");
        return Err(());
    };
    let code = interp
        .call_main(&main, args, &mut out)
        .map_err(|e| report_text(&[e], &source, "<stdin>"))?;
    exit_code(code)
}

/// Reports errors for source that is not in a file.
fn report_text(errors: &[Error], source: &str, name: &str) {
    let _ = io::stdout().flush();
    for e in errors {
        eprintln!("{}", e.render(source, Some(name)));
    }
}

/// The root of the project containing the current directory.
fn project_root() -> Result<PathBuf, ()> {
    let cwd = std::env::current_dir().map_err(|e| eprintln!("io error: {e}"))?;
    project::find_root(&cwd).ok_or_else(|| {
        eprintln!(
            "project error: no {} found in {} or any parent directory\n\
             hint: run a single file with `belsk2 run <file>`, or create a project with `belsk2 new <name>`",
            project::MANIFEST,
            cwd.display()
        );
    })
}

/// Runs a project: in the interpreter, or in wasmtime for a component
/// (unless `interpreted`).
/// How `belsk2 run` runs a program.
struct RunMode {
    interpreted: bool,
    call: Option<String>,
    target: Option<String>,
}

/// The build target: `flag`, else `[build] target` of the manifest, else
/// wasm32.
fn target(flag: Option<&str>, manifest: Option<&project::Manifest>) -> Result<Target, ()> {
    let name = flag.or_else(|| {
        manifest
            .and_then(|m| m.build.as_ref())
            .and_then(|b| b.target.as_deref())
    });
    let Some(name) = name else {
        return Ok(Target::default());
    };
    Target::parse(name).ok_or_else(|| {
        let names: Vec<&str> = Target::ALL.iter().map(|t| t.name()).collect();
        eprintln!(
            "error: unknown target '{name}'; use one of: {}",
            names.join(", ")
        );
    })
}

fn load_manifest(root: &Path) -> Result<project::Manifest, ()> {
    let path = root.join(project::MANIFEST);
    if !path.is_file() {
        eprintln!(
            "project error: no {} in {}",
            project::MANIFEST,
            root.display()
        );
        return Err(());
    }
    project::Manifest::load(&path).map_err(|e| eprintln!("{}", e.render_in(&SourceMap::new())))
}

fn run(
    file: Option<PathBuf>,
    args: &[String],
    mode: RunMode,
    options: &project::Options,
) -> Result<(), ()> {
    match file {
        Some(file) if file.is_dir() => run_project(&file, args, mode, options),
        Some(file) => {
            let target = target(mode.target.as_deref(), None)?;
            if target == Target::Wasi02 && !mode.interpreted {
                let wasm = compile_file(&file, target)?;
                return run_wasm(&wasm, mode.call.as_deref(), args);
            }
            if mode.call.is_some() {
                eprintln!(
                    "error: --call needs a component (a [component] project or --target wasi0.2)"
                );
                return Err(());
            }
            run_file(&file, args, options)
        }
        None => project_root().and_then(|root| run_project(&root, args, mode, options)),
    }
}

/// Runs a project: in wasmtime for a component or the wasi0.2 target
/// (unless `interpreted`), else in the interpreter.
fn run_project(
    root: &Path,
    args: &[String],
    mode: RunMode,
    options: &project::Options,
) -> Result<(), ()> {
    let manifest = load_manifest(root)?;
    let target = target(mode.target.as_deref(), Some(&manifest))?;
    let wasm = manifest.component.is_some() || target == Target::Wasi02;
    if wasm && !mode.interpreted {
        let mut loaded = project::load_with(root, Mode::All, options)
            .map_err(|(errors, sources)| report(&errors, &sources))?;
        let errors = project::check(&mut loaded);
        if !errors.is_empty() {
            report(&errors, &loaded.sources);
            return Err(());
        }
        let wasm = belsk2_wasm::compile_project(&loaded, target)
            .map_err(|errors| report(&errors, &loaded.sources))?;
        return run_wasm(&wasm, mode.call.as_deref(), args);
    }
    if mode.call.is_some() {
        eprintln!("error: --call needs a component (a [component] project or --target wasi0.2)");
        return Err(());
    }
    check_and_run(project::load_with(root, Mode::Run, options), args)
}

#[cfg(feature = "wasmtime")]
fn run_wasm(wasm: &[u8], call: Option<&str>, args: &[String]) -> Result<(), ()> {
    host::run(wasm, call, args).map_err(|e| eprintln!("error: {e}"))
}

#[cfg(not(feature = "wasmtime"))]
fn run_wasm(_: &[u8], _: Option<&str>, _: &[String]) -> Result<(), ()> {
    eprintln!("error: this belsk2 was built without wasmtime; use `belsk2 run --interpreted`");
    Err(())
}

fn update(options: &project::Options) -> Result<(), ()> {
    let root = project_root()?;
    let options = project::Options {
        update: true,
        ..options.clone()
    };
    let before = project::Lockfile::load(&root).unwrap_or_default();
    project::load_with(&root, Mode::All, &options)
        .map_err(|(errors, sources)| report(&errors, &sources))?;
    let after = project::Lockfile::load(&root).unwrap_or_default();
    let mut changed = 0;
    for p in &after.packages {
        match before.find(&p.git, &p.reference) {
            Some(old) if old.commit == p.commit => {}
            Some(old) => {
                changed += 1;
                println!(
                    "Updated {} {} -> {}",
                    p.name,
                    short(&old.commit),
                    short(&p.commit)
                );
            }
            None => {
                changed += 1;
                println!("Added {} at {}", p.name, short(&p.commit));
            }
        }
    }
    if changed == 0 {
        println!("All git dependencies are up to date");
    }
    Ok(())
}

fn short(commit: &str) -> &str {
    commit.get(..8).unwrap_or(commit)
}

fn collect_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if !name.starts_with('.') && name != "target" && name != "node_modules" {
                collect_sources(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "belsk2") {
            out.push(path);
        }
    }
}

/// `belsk2 fmt`: formats files in place (or checks them with `--check`).
fn fmt(files: &[PathBuf], check: bool) -> Result<(), ()> {
    if files.len() == 1 && files.first().is_some_and(|f| f == Path::new("-")) {
        let mut source = String::new();
        io::stdin()
            .read_to_string(&mut source)
            .map_err(|e| eprintln!("io error: cannot read stdin: {e}"))?;
        let cwd = std::env::current_dir().unwrap_or_default();
        let config = belsk2_fmt::Config::find(&cwd)
            .map(|p| belsk2_fmt::Config::load(&p))
            .unwrap_or_else(|| Ok(belsk2_fmt::Config::default()))
            .map_err(|e| eprintln!("{}", e.message))?;
        let out = belsk2_fmt::format(&source, &config)
            .map_err(|e| eprintln!("{}", e.render(&source, Some("<stdin>"))))?;
        print!("{out}");
        return Ok(());
    }
    let mut paths = Vec::new();
    if files.is_empty() {
        let root = project_root()?;
        collect_sources(&root, &mut paths);
    }
    for f in files {
        if f.is_dir() {
            collect_sources(f, &mut paths);
        } else {
            paths.push(f.clone());
        }
    }
    paths.sort();
    let mut failed = false;
    let mut changed = 0;
    for path in &paths {
        let shown = project::display_path(path);
        let result = std::fs::read_to_string(path)
            .map_err(|e| format!("io error: cannot read {shown}: {e}"))
            .and_then(|source| {
                let config = belsk2_fmt::Config::for_file(path).map_err(|e| e.message)?;
                let out = belsk2_fmt::format(&source, &config)
                    .map_err(|e| e.render(&source, Some(&shown)))?;
                Ok((source, out))
            });
        match result {
            Err(message) => {
                eprintln!("{message}");
                failed = true;
            }
            Ok((source, out)) if source != out => {
                changed += 1;
                if check {
                    println!("would reformat {shown}");
                } else if let Err(e) = std::fs::write(path, out) {
                    eprintln!("io error: cannot write {shown}: {e}");
                    failed = true;
                } else {
                    println!("formatted {shown}");
                }
            }
            Ok(_) => {}
        }
    }
    if check && changed > 0 {
        eprintln!("{changed} file(s) need formatting; run `belsk2 fmt`");
        return Err(());
    }
    if !check && changed == 0 && !failed {
        println!("{} file(s) already formatted", paths.len());
    }
    if failed {
        Err(())
    } else {
        Ok(())
    }
}

fn check_files(files: &[PathBuf]) -> Result<(), ()> {
    let mut ok = true;
    for file in files {
        let result =
            project::load_file(file).map_err(|(errors, sources)| report(&errors, &sources));
        let Ok(mut loaded) = result else {
            ok = false;
            continue;
        };
        let errors = project::check(&mut loaded);
        if !errors.is_empty() {
            report(&errors, &loaded.sources);
            ok = false;
        }
    }
    if ok {
        Ok(())
    } else {
        Err(())
    }
}

/// Checks every module of the current project.
fn check_project(options: &project::Options) -> Result<(), ()> {
    let root = project_root()?;
    let mut loaded = project::load_with(&root, Mode::All, options)
        .map_err(|(errors, sources)| report(&errors, &sources))?;
    let errors = project::check(&mut loaded);
    if !errors.is_empty() {
        report(&errors, &loaded.sources);
        return Err(());
    }
    println!("Checked {}", project::summary(&loaded));
    Ok(())
}

/// Compiles the current project to `bin/<package>.wasm` (or `output`).
fn build_project(
    output: Option<PathBuf>,
    target_flag: Option<String>,
    options: &project::Options,
) -> Result<(), ()> {
    let root = project_root()?;
    let target = target(target_flag.as_deref(), Some(&load_manifest(&root)?))?;
    let mut loaded = project::load_with(&root, Mode::All, options)
        .map_err(|(errors, sources)| report(&errors, &sources))?;
    let errors = project::check(&mut loaded);
    if !errors.is_empty() {
        report(&errors, &loaded.sources);
        return Err(());
    }
    let wasm = belsk2_wasm::compile_project(&loaded, target)
        .map_err(|errors| report(&errors, &loaded.sources))?;
    let name = loaded
        .root_package()
        .map(|p| p.name.clone())
        .unwrap_or_default();
    let output = output.unwrap_or_else(|| root.join("bin").join(format!("{name}.wasm")));
    write_output(&output, &wasm)
}

fn write_output(output: &Path, wasm: &[u8]) -> Result<(), ()> {
    if let Some(dir) = output.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .map_err(|e| eprintln!("io error: cannot create {}: {e}", dir.display()))?;
    }
    std::fs::write(output, wasm)
        .map_err(|e| eprintln!("io error: cannot write {}: {e}", output.display()))?;
    println!("Wrote {} ({} bytes)", output.display(), wasm.len());
    Ok(())
}

/// Compiles one file to a WebAssembly module.
fn build_wasm(file: &Path, output: Option<PathBuf>, target_flag: Option<String>) -> Result<(), ()> {
    let wasm = compile_file(file, target(target_flag.as_deref(), None)?)?;
    let output = output.unwrap_or_else(|| file.with_extension("wasm"));
    write_output(&output, &wasm)
}

fn compile_file(file: &Path, target: Target) -> Result<Vec<u8>, ()> {
    let mut loaded =
        project::load_file(file).map_err(|(errors, sources)| report(&errors, &sources))?;
    let errors = project::check(&mut loaded);
    if !errors.is_empty() {
        report(&errors, &loaded.sources);
        return Err(());
    }
    let Some(module) = loaded.entry.and_then(|m| loaded.modules.get(m)) else {
        eprintln!("project error: nothing to build in {}", file.display());
        return Err(());
    };
    belsk2_wasm::compile(&module.program, target).map_err(|errors| report(&errors, &loaded.sources))
}

/// `belsk2 bindgen`: shows the WIT world as Belsk2 declarations, or with
/// `stubs` adds missing exported functions to the project's sources.
fn bindgen(stubs: bool, output: Option<PathBuf>, options: &project::Options) -> Result<(), ()> {
    let root = project_root()?;
    let (loaded, errors) = project::load_lenient(&root, Mode::All, options);
    let Some(world) = loaded.world.clone() else {
        if errors.is_empty() {
            eprintln!(
                "project error: {} has no [component] section naming a WIT world",
                root.join(project::MANIFEST).display()
            );
        } else {
            report(&errors, &loaded.sources);
        }
        return Err(());
    };
    if !stubs {
        let text = world.describe();
        return match output {
            Some(path) => std::fs::write(&path, text)
                .map_err(|e| eprintln!("io error: cannot write {}: {e}", path.display())),
            None => {
                print!("{text}");
                Ok(())
            }
        };
    }
    let root_module = if root.join("src/main.belsk2").is_file() || loaded.entry.is_some() {
        "main.belsk2"
    } else {
        "lib.belsk2"
    };
    let mut added = 0;
    for f in &world.exports {
        if let Some(why) = &f.unsupported {
            eprintln!("skipped `{}`: {why}", f.wit_name);
            continue;
        }
        let path = root.join(project::wit::export_file(f, root_module));
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let declared = belsk2::syntax::parse(&text).is_ok_and(|p| {
            p.stmts.iter().any(
                |s| matches!(&s.kind, belsk2::syntax::ast::StmtKind::FnDecl(d) if d.name == f.name),
            )
        });
        if declared {
            continue;
        }
        let mut new_text = text.clone();
        if !new_text.is_empty() {
            if !new_text.ends_with('\n') {
                new_text.push('\n');
            }
            new_text.push('\n');
        }
        new_text.push_str(&project::wit::stub(f));
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| eprintln!("io error: cannot create {}: {e}", dir.display()))?;
        }
        std::fs::write(&path, new_text)
            .map_err(|e| eprintln!("io error: cannot write {}: {e}", path.display()))?;
        println!(
            "Added `{}` to {}",
            f.signature(),
            project::display_path(&path)
        );
        added += 1;
    }
    if added == 0 {
        println!("Every export of the WIT world is already implemented");
    }
    Ok(())
}

fn new_project(dir: &Path, name: &str, lib: bool) -> Result<(), ()> {
    if dir.exists() {
        eprintln!("project error: {} already exists", dir.display());
        return Err(());
    }
    create(dir, name, lib)
}

fn init(name: Option<String>, lib: bool) -> Result<(), ()> {
    let cwd = std::env::current_dir().map_err(|e| eprintln!("io error: {e}"))?;
    let name = match name {
        Some(n) => n,
        None => cwd
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase().replace(' ', "-"))
            .unwrap_or_default(),
    };
    create(&cwd, &name, lib)
}

fn create(dir: &Path, name: &str, lib: bool) -> Result<(), ()> {
    let template = if lib { Template::Lib } else { Template::Bin };
    match project::create(dir, name, template) {
        Ok(files) => {
            let kind = if lib { "library" } else { "program" };
            println!("Created {kind} `{name}` in {}", dir.display());
            for f in files {
                println!("  {}", f.display());
            }
            Ok(())
        }
        Err(e) => {
            eprintln!("{}", e.render("", None));
            Err(())
        }
    }
}

fn repl() -> Result<(), ()> {
    println!("belsk2 {} — type 'exit' to quit", belsk2::VERSION);
    let mut interp = Interpreter::new();
    let stdin = io::stdin();
    let mut buffer = String::new();

    loop {
        print!(
            "{}",
            if buffer.is_empty() {
                "belsk2> "
            } else {
                "   ...> "
            }
        );
        let _ = io::stdout().flush();

        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => {
                println!();
                return Ok(());
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("io error: {e}");
                return Err(());
            }
        }
        if buffer.is_empty() {
            match line.trim() {
                "" => continue,
                "exit" | "quit" => return Ok(()),
                _ => {}
            }
        }
        buffer.push_str(&line);

        // Keep reading while the input is an unfinished statement.
        let errors = match interp.compile_repl(&buffer) {
            Err(errors) if errors.iter().any(|e| e.incomplete) && !line.trim().is_empty() => {
                continue
            }
            Err(errors) => errors,
            Ok(program) => match interp.run_program(&program, &mut io::stdout()) {
                Ok(()) => Vec::new(),
                Err(e) => vec![e],
            },
        };
        report_text(&errors, &buffer, "<input>");
        buffer.clear();
    }
}
