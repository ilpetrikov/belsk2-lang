use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use belsk2::project::{self, Loaded, Mode, Template};
use belsk2::{Error, Interpreter, SourceMap};
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
        /// Arguments passed to `main(args: string[])`
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Check the current project (or the given files) without running it
    Check {
        /// Source files
        files: Vec<PathBuf>,
    },
    /// Build the current project
    Build,
    /// Move git dependencies to the newest commit of their branch or tag
    /// and rewrite belsk2.lock
    Update,
    /// Start an interactive session
    Repl,
    /// Start the language server (for editors), talking LSP over stdin/stdout
    Lsp,
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
                file: Some(file),
                args,
            }),
            _,
        ) => run_file(&file, &args, &options),
        (None, Some(file)) => run_file(&file, &[], &options),
        (Some(Command::Run { file: None, args }), _) => run_project(&args, &options),
        (Some(Command::Check { files }), _) if files.is_empty() => build(false, &options),
        (Some(Command::Check { files }), _) => check_files(&files),
        (Some(Command::Build), _) => build(true, &options),
        (Some(Command::Update), _) => update(&options),
        (Some(Command::New { name, lib }), _) => new_project(&PathBuf::from(&name), &name, lib),
        (Some(Command::Init { name, lib }), _) => init(name, lib),
        (Some(Command::Repl), _) => repl(),
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

fn run_project(args: &[String], options: &project::Options) -> Result<(), ()> {
    let root = project_root()?;
    check_and_run(project::load_with(&root, Mode::Run, options), args)
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
fn build(is_build: bool, options: &project::Options) -> Result<(), ()> {
    let root = project_root()?;
    let mut loaded = project::load_with(&root, Mode::All, options)
        .map_err(|(errors, sources)| report(&errors, &sources))?;
    let errors = project::check(&mut loaded);
    if !errors.is_empty() {
        report(&errors, &loaded.sources);
        return Err(());
    }
    let summary = project::summary(&loaded);
    if is_build {
        // Code generation arrives with the WASM backend; until then a build
        // is a full check of the project and its dependencies.
        println!("Checked {summary}; no compiled output yet (the WASM backend is not ready)");
    } else {
        println!("Checked {summary}");
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
