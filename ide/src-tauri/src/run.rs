//! Running programs. The IDE starts itself in runner mode
//! (`--belsk2-run <path>`), streams the output to the window as events and
//! forwards what the user types to the program's stdin.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::rc::Rc;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use belsk2::project::{self, Loaded, Mode as LoadMode};
use belsk2::{Error, Interpreter, SourceMap, Trace};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

#[derive(Clone, Copy)]
pub enum Mode {
    Run,
    Check,
}

impl Mode {
    pub fn from_flag(flag: &str) -> Option<Mode> {
        match flag {
            "--belsk2-run" => Some(Mode::Run),
            "--belsk2-check" => Some(Mode::Check),
            _ => None,
        }
    }

    fn flag(self) -> &'static str {
        match self {
            Mode::Run => "--belsk2-run",
            Mode::Check => "--belsk2-check",
        }
    }
}

fn report(errors: &[Error], sources: &SourceMap) {
    let _ = std::io::stdout().flush();
    for e in errors {
        eprintln!("{}", e.render_in(sources));
    }
}

/// Runner mode: checks and runs the file or project at `path`. With
/// `trace`, line hits, values and the id bank are written there as JSON
/// whenever the program waits for input, and when it ends.
pub fn runner(mode: Mode, path: &str, trace: Option<&str>) -> ExitCode {
    let path = Path::new(path);
    let loaded = match path.parent().and_then(project::find_root) {
        Some(root) => {
            let load_mode = match mode {
                Mode::Run => LoadMode::Run,
                Mode::Check => LoadMode::All,
            };
            project::load(&root, load_mode)
        }
        None => project::load_file(path),
    };
    match check_and_run(mode, loaded, trace) {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(()) => ExitCode::FAILURE,
    }
}

fn check_and_run(
    mode: Mode,
    loaded: Result<Loaded, (Vec<Error>, SourceMap)>,
    trace_file: Option<&str>,
) -> Result<i64, ()> {
    let mut loaded = loaded.map_err(|(errors, sources)| report(&errors, &sources))?;
    let errors = project::check(&mut loaded);
    if !errors.is_empty() {
        report(&errors, &loaded.sources);
        return Err(());
    }
    if let Mode::Check = mode {
        println!("No errors in {}", project::summary(&loaded));
        return Ok(0);
    }
    let mut interp = Interpreter::new();
    let mut dump: Option<Rc<dyn Fn()>> = None;
    if let Some(file) = trace_file {
        let trace = Rc::new(RefCell::new(Trace::default()));
        interp.set_trace(trace.clone());
        let paths: BTreeMap<u32, String> = loaded
            .modules
            .iter()
            .map(|m| (m.file, crate::files::plain(&m.path)))
            .collect();
        let file = file.to_string();
        let write: Rc<dyn Fn()> = Rc::new(move || {
            let _ = std::io::stdout().flush();
            if let Ok(t) = trace.try_borrow() {
                let _ = std::fs::write(&file, trace_json(&t, &paths));
            }
        });
        interp.set_input(TracingStdin {
            inner: BufReader::new(std::io::stdin()),
            dump: write.clone(),
        });
        dump = Some(write);
    }
    let result = project::run(&loaded, &mut interp, &[], &mut std::io::stdout());
    if let Some(dump) = &dump {
        dump();
    }
    let code = result.map_err(|e| report(&[e], &loaded.sources))?;
    Ok(code.unwrap_or(0))
}

fn trace_json(t: &Trace, paths: &BTreeMap<u32, String>) -> String {
    let mut files: BTreeMap<&str, BTreeMap<String, serde_json::Value>> = BTreeMap::new();
    for ((file, line), entry) in &t.lines {
        let Some(path) = paths.get(file) else {
            continue;
        };
        files.entry(path).or_default().insert(
            line.to_string(),
            serde_json::json!({ "hits": entry.hits, "values": entry.values }),
        );
    }
    let idb: Vec<(i64, &String)> = t.idb.iter().map(|(k, v)| (*k, v)).collect();
    serde_json::json!({ "files": files, "idb": idb }).to_string()
}

/// Stdin that saves the trace before the program blocks waiting for input,
/// so the editor can show the values so far.
struct TracingStdin {
    inner: BufReader<std::io::Stdin>,
    dump: Rc<dyn Fn()>,
}

impl Read for TracingStdin {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.inner.buffer().is_empty() {
            (self.dump)();
        }
        self.inner.read(buf)
    }
}

impl BufRead for TracingStdin {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        if self.inner.buffer().is_empty() {
            (self.dump)();
        }
        self.inner.fill_buf()
    }

    fn consume(&mut self, amt: usize) {
        self.inner.consume(amt);
    }
}

/// Reads the trace file and sends it to the window if it changed.
fn send_trace(app: &AppHandle, file: &Path, last: &mut Option<std::time::SystemTime>) {
    let Ok(modified) = std::fs::metadata(file).and_then(|m| m.modified()) else {
        return;
    };
    if *last == Some(modified) {
        return;
    }
    *last = Some(modified);
    if let Ok(text) = std::fs::read_to_string(file) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
            let _ = app.emit("run-trace", value);
        }
    }
}

/// The program that is running, if any.
#[derive(Default)]
pub struct Running {
    child: Arc<Mutex<Option<Child>>>,
    stdin: Mutex<Option<ChildStdin>>,
    /// Bumped by every start, so a waiter stops watching once replaced.
    generation: Arc<AtomicU64>,
}

#[derive(Serialize, Clone)]
struct Output {
    stream: &'static str,
    text: String,
}

fn pump(app: AppHandle, stream: &'static str, from: impl Read + Send + 'static) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(from);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let text = String::from_utf8_lossy(&buf).into_owned();
                    let _ = app.emit("run-output", Output { stream, text });
                }
            }
        }
    });
}

/// Starts running (or checking) `path`. Output arrives as `run-output`
/// events, the end as a `run-exit` event with the exit code.
#[tauri::command]
pub fn start(app: AppHandle, state: State<'_, Running>, path: String, check: bool) -> Result<(), String> {
    stop(state.clone());
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mode = if check { Mode::Check } else { Mode::Run };
    let dir = Path::new(&path)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let trace_file = std::env::temp_dir().join(format!("belsk2-ide-trace-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&trace_file);
    let mut cmd = Command::new(exe);
    cmd.arg(mode.flag())
        .arg(&path)
        .arg(&trace_file)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().map_err(|e| format!("cannot start the program: {e}"))?;
    if let Some(out) = child.stdout.take() {
        pump(app.clone(), "stdout", out);
    }
    if let Some(err) = child.stderr.take() {
        pump(app.clone(), "stderr", err);
    }
    if let Ok(mut stdin) = state.stdin.lock() {
        *stdin = child.stdin.take();
    }
    if let Ok(mut slot) = state.child.lock() {
        *slot = Some(child);
    }
    // Wait for the end without holding the lock, so `stop` can kill it.
    let child = state.child.clone();
    let generation = state.generation.clone();
    let mine = generation.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
    let mut last_trace = None;
    std::thread::spawn(move || loop {
        if generation.load(Ordering::SeqCst) != mine {
            break;
        }
        send_trace(&app, &trace_file, &mut last_trace);
        let status = match child.lock() {
            Ok(mut slot) => match slot.as_mut().map(Child::try_wait) {
                Some(Ok(Some(status))) => {
                    *slot = None;
                    Some(status.code())
                }
                Some(Ok(None)) => None,
                Some(Err(_)) | None => Some(None),
            },
            Err(_) => Some(None),
        };
        if let Some(code) = status {
            send_trace(&app, &trace_file, &mut last_trace);
            let _ = app.emit("run-exit", code);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    });
    Ok(())
}

/// Sends a line to the running program's stdin.
#[tauri::command]
pub fn send_input(state: State<'_, Running>, text: String) -> Result<(), String> {
    let mut stdin = state.stdin.lock().map_err(|e| e.to_string())?;
    match stdin.as_mut() {
        Some(s) => writeln!(s, "{text}")
            .and_then(|()| s.flush())
            .map_err(|e| e.to_string()),
        None => Err("no program is running".to_string()),
    }
}

#[tauri::command]
pub fn stop(state: State<'_, Running>) {
    if let Ok(mut stdin) = state.stdin.lock() {
        *stdin = None;
    }
    if let Ok(mut slot) = state.child.lock() {
        if let Some(child) = slot.as_mut() {
            let _ = child.kill();
        }
    }
}
