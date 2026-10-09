//! The Belsk2 IDE: a Tauri app whose Rust side reads and writes files,
//! analyzes code with `belsk2-analysis` and runs programs.

// No console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod files;
mod lang;
mod run;

use std::process::ExitCode;

fn main() -> ExitCode {
    // The IDE starts itself with `--belsk2-run <path>` to run a program in
    // a separate process that can be stopped.
    let args: Vec<String> = std::env::args().collect();
    if let Some(mode) = args.get(1).and_then(|a| run::Mode::from_flag(a)) {
        return run::runner(mode, args.get(2).map(String::as_str).unwrap_or(""));
    }

    let result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(run::Running::default())
        .invoke_handler(tauri::generate_handler![
            files::open_folder,
            files::read_file,
            files::write_file,
            files::create_file,
            files::create_dir,
            files::rename_path,
            files::new_project,
            lang::analyze,
            lang::hover,
            lang::complete,
            lang::definition,
            lang::references,
            lang::rename,
            run::start,
            run::send_input,
            run::stop,
        ])
        .run(tauri::generate_context!());
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("belsk2 ide: {e}");
            ExitCode::FAILURE
        }
    }
}
