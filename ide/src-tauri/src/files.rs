//! Files and folders: the explorer tree, reading, saving, creating.

use std::path::{Path, PathBuf};

use belsk2::project::{self, Template};
use serde::Serialize;

/// Folders the explorer does not show.
const SKIPPED: &[&str] = &[".git", "target", "node_modules", "dist", ".belsk2"];
/// Upper bound on the number of entries, so huge folders stay responsive.
const MAX_ENTRIES: usize = 5000;

#[derive(Serialize)]
pub struct Entry {
    name: String,
    path: String,
    dir: bool,
    children: Vec<Entry>,
}

#[derive(Serialize)]
pub struct Folder {
    root: String,
    name: String,
    /// The package name from `belsk2.toml`, when the folder is a project.
    project: Option<String>,
    entries: Vec<Entry>,
}

/// A path without the `\\?\` prefix of Windows canonical paths, with the
/// same spelling that `belsk2-analysis` uses.
pub fn plain(path: &Path) -> String {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = canonical.display().to_string();
    text.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(text)
}

fn list(dir: &Path, count: &mut usize) -> Vec<Entry> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut items: Vec<(String, PathBuf, bool)> = read
        .flatten()
        .map(|e| {
            let path = e.path();
            let dir = path.is_dir();
            (e.file_name().to_string_lossy().into_owned(), path, dir)
        })
        .filter(|(name, _, dir)| !(*dir && SKIPPED.contains(&name.as_str())))
        .collect();
    // Folders first, then by name.
    items.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
    let mut out = Vec::new();
    for (name, path, dir) in items {
        if *count >= MAX_ENTRIES {
            break;
        }
        *count += 1;
        let children = if dir { list(&path, count) } else { Vec::new() };
        out.push(Entry {
            name,
            path: plain(&path),
            dir,
            children,
        });
    }
    out
}

#[tauri::command]
pub fn open_folder(path: String) -> Result<Folder, String> {
    let root = PathBuf::from(plain(Path::new(&path)));
    if !root.is_dir() {
        return Err(format!("{} is not a folder", root.display()));
    }
    let project = project::Manifest::load(&root.join(project::MANIFEST))
        .ok()
        .map(|m| m.package.name);
    let mut count = 0;
    Ok(Folder {
        name: root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string()),
        root: root.display().to_string(),
        project,
        entries: list(&root, &mut count),
    })
}

#[tauri::command]
pub fn read_file(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| format!("cannot read {path}: {e}"))
}

#[tauri::command]
pub fn write_file(path: String, text: String) -> Result<(), String> {
    std::fs::write(&path, text).map_err(|e| format!("cannot save {path}: {e}"))
}

#[tauri::command]
pub fn create_file(path: String) -> Result<String, String> {
    let p = Path::new(&path);
    if p.exists() {
        return Err(format!("{path} already exists"));
    }
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(p, "").map_err(|e| format!("cannot create {path}: {e}"))?;
    Ok(plain(p))
}

#[tauri::command]
pub fn create_dir(path: String) -> Result<String, String> {
    std::fs::create_dir_all(&path).map_err(|e| format!("cannot create {path}: {e}"))?;
    Ok(plain(Path::new(&path)))
}

#[tauri::command]
pub fn rename_path(from: String, to: String) -> Result<String, String> {
    if Path::new(&to).exists() {
        return Err(format!("{to} already exists"));
    }
    std::fs::rename(&from, &to).map_err(|e| format!("cannot rename {from}: {e}"))?;
    Ok(plain(Path::new(&to)))
}

/// Creates a project `name` inside `parent` and returns its folder.
#[tauri::command]
pub fn new_project(parent: String, name: String, lib: bool) -> Result<String, String> {
    let dir = Path::new(&parent).join(&name);
    let template = if lib { Template::Lib } else { Template::Bin };
    project::create(&dir, &name, template).map_err(|e| e.message)?;
    Ok(plain(&dir))
}
