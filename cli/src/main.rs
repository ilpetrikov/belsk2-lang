use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use belsk2::{Error, Interpreter};
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

    /// Program to run (shorthand for `belsk2 run <FILE>`)
    file: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// Run a program
    Run {
        /// Source file; `-` reads from stdin
        file: PathBuf,
    },
    /// Check a program for errors without running it
    Check {
        /// Source files
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Start an interactive session
    Repl,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match (cli.command, cli.file) {
        (Some(Command::Run { file }), _) | (None, Some(file)) => run(&file),
        (Some(Command::Check { files }), _) => check(&files),
        (Some(Command::Repl), _) => repl(),
        (None, None) if io::stdin().is_terminal() => repl(),
        (None, None) => run(Path::new("-")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => ExitCode::FAILURE,
    }
}

/// Prints an error to stderr with a source snippet.
fn report(err: &Error, source: &str, file: Option<&str>) {
    let _ = io::stdout().flush();
    eprintln!("{}", err.render(source, file));
}

fn read_source(file: &Path) -> Result<(String, String), ()> {
    if file == Path::new("-") {
        let mut source = String::new();
        if let Err(e) = io::stdin().read_to_string(&mut source) {
            eprintln!("io error: cannot read stdin: {e}");
            return Err(());
        }
        return Ok((source, "<stdin>".to_string()));
    }
    match std::fs::read_to_string(file) {
        Ok(source) => Ok((source, file.display().to_string())),
        Err(e) => {
            eprintln!("io error: cannot read {}: {e}", file.display());
            Err(())
        }
    }
}

fn report_all(errors: &[Error], source: &str, file: Option<&str>) {
    for e in errors {
        report(e, source, file);
    }
    if errors.len() > 1 {
        eprintln!("{} errors", errors.len());
    }
}

fn run(file: &Path) -> Result<(), ()> {
    let (source, name) = read_source(file)?;
    let mut interp = Interpreter::new();
    if file == Path::new("-") {
        // stdin holds the program itself, so `reab`/`input` get no input.
        interp.set_input(io::empty());
    }
    let program = interp
        .compile(&source)
        .map_err(|errors| report_all(&errors, &source, Some(&name)))?;
    interp
        .run_program(&program, &mut io::stdout())
        .map_err(|e| report(&e, &source, Some(&name)))
}

fn check(files: &[PathBuf]) -> Result<(), ()> {
    let mut ok = true;
    for file in files {
        let (source, name) = read_source(file)?;
        if let Err(errors) = belsk2::compile(&source) {
            report_all(&errors, &source, Some(&name));
            ok = false;
        }
    }
    if ok {
        Ok(())
    } else {
        Err(())
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
        match interp.compile(&buffer) {
            Err(errors) if errors.iter().any(|e| e.incomplete) && !line.trim().is_empty() => {
                continue
            }
            Err(errors) => report_all(&errors, &buffer, None),
            Ok(program) => {
                if let Err(e) = interp.run_program(&program, &mut io::stdout()) {
                    report(&e, &buffer, None);
                }
            }
        }
        buffer.clear();
    }
}
