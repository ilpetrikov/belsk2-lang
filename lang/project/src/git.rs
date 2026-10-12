//! Git dependencies, fetched with the `git` command into a cache:
//!
//! ```text
//! <home>/git/db/<name>-<hash>/                bare clone of the repository
//! <home>/git/checkouts/<name>-<hash>/<commit>/ the files at one commit
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;

use belsk2_syntax::Error;

use crate::manifest::Dependency;

/// Which commit of a repository a dependency wants.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GitRef {
    /// The repository's default branch.
    Default,
    Branch(String),
    Tag(String),
    Rev(String),
}

impl GitRef {
    pub fn of(spec: &Dependency) -> GitRef {
        if let Some(b) = &spec.branch {
            GitRef::Branch(b.clone())
        } else if let Some(t) = &spec.tag {
            GitRef::Tag(t.clone())
        } else if let Some(r) = &spec.rev {
            GitRef::Rev(r.clone())
        } else {
            GitRef::Default
        }
    }

    /// The form stored in `belsk2.lock`: `branch=main`, `tag=v1.0`, ...
    pub fn to_lock(&self) -> String {
        match self {
            GitRef::Default => "default".to_string(),
            GitRef::Branch(b) => format!("branch={b}"),
            GitRef::Tag(t) => format!("tag={t}"),
            GitRef::Rev(r) => format!("rev={r}"),
        }
    }

    fn rev_spec(&self) -> String {
        match self {
            GitRef::Default => "HEAD".to_string(),
            GitRef::Branch(b) => format!("refs/heads/{b}"),
            GitRef::Tag(t) => format!("refs/tags/{t}"),
            GitRef::Rev(r) => r.clone(),
        }
    }
}

impl std::fmt::Display for GitRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitRef::Default => f.write_str("default branch"),
            GitRef::Branch(b) => write!(f, "branch '{b}'"),
            GitRef::Tag(t) => write!(f, "tag '{t}'"),
            GitRef::Rev(r) => write!(f, "rev '{r}'"),
        }
    }
}

/// The directory belsk2 keeps downloaded dependencies in: `$BELSK2_HOME`,
/// or `.belsk2` in the user's home directory.
pub fn default_home() -> PathBuf {
    if let Some(home) = std::env::var_os("BELSK2_HOME") {
        return PathBuf::from(home);
    }
    let user = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    user.join(".belsk2")
}

/// FNV-1a: a hash that stays the same across Rust versions, so cache
/// directory names do not change.
fn stable_hash(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// `json-1a2b3c4d` for `https://example.com/someone/json.git`. Kept short
/// because Windows limits path lengths.
fn cache_name(url: &str) -> String {
    let last = url
        .trim_end_matches('/')
        .rsplit(['/', '\\', ':'])
        .next()
        .unwrap_or("repo")
        .trim_end_matches(".git");
    let clean: String = last
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let clean: String = clean.chars().take(24).collect();
    format!("{clean}-{:08x}", stable_hash(&source_key(url)) >> 32)
}

/// What identifies a repository, whichever way its URL is written:
/// `https://GitHub.com/a/b.git/`, `https://github.com/a/b` and
/// `git@github.com:a/b` are the same. Local paths are compared as they
/// are (they are absolute and canonical already).
pub fn source_key(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    let url = url
        .strip_suffix(".git")
        .unwrap_or(url)
        .trim_end_matches('/');
    let (host, path) = if let Some((_, rest)) = url.split_once("://") {
        match rest.split_once('/') {
            Some((host, path)) => (host, path),
            None => (rest, ""),
        }
    } else if let Some((user_host, path)) = url
        .split_once(':')
        .filter(|(h, _)| h.contains('@') && !h.contains('/'))
    {
        // `git@github.com:a/b`
        (user_host, path)
    } else {
        return url.to_string();
    };
    // Users and ports do not change the repository.
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    format!(
        "{}/{}",
        host.to_ascii_lowercase(),
        path.trim_start_matches('/')
    )
}

/// Whether two git URLs name the same repository.
pub fn same_source(a: &str, b: &str) -> bool {
    source_key(a) == source_key(b)
}

/// How `belsk2.lock` (in `project_root`) writes a URL: a local path
/// relative to the project, so the file works on every machine; other URLs
/// as they are.
pub fn lock_url(url: &str, project_root: &Path) -> String {
    let path = Path::new(url);
    if url.contains("://") || url.starts_with("git@") || !path.is_absolute() {
        return url.to_string();
    }
    let root = std::fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let root = strip_verbatim(&root);
    let target = strip_verbatim(path);
    let (root_parts, target_parts): (Vec<_>, Vec<_>) =
        (root.components().collect(), target.components().collect());
    let common = root_parts
        .iter()
        .zip(&target_parts)
        .take_while(|(a, b)| a == b)
        .count();
    if common == 0 {
        // Another drive: only an absolute path works.
        return url.to_string();
    }
    let mut parts: Vec<String> = vec!["..".to_string(); root_parts.len() - common];
    parts.extend(
        target_parts
            .iter()
            .skip(common)
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    if parts.is_empty() {
        return ".".to_string();
    }
    parts.join("/")
}

fn strip_verbatim(p: &Path) -> PathBuf {
    PathBuf::from(p.to_string_lossy().trim_start_matches(r"\\?\").to_string())
}

fn git(args: &[&str], dir: Option<&Path>) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.args(["-c", "core.longpaths=true"]);
    if let Some(dir) = dir {
        cmd.arg("-C").arg(dir);
    }
    cmd.args(args).env("GIT_TERMINAL_PROMPT", "0");
    let output = cmd.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "git is not installed or not on PATH; it is needed for git dependencies".to_string()
        } else {
            format!("cannot run git: {e}")
        }
    })?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "`git {}` failed: {}",
            args.join(" "),
            stderr.trim()
        ))
    }
}

pub struct GitCache {
    root: PathBuf,
    offline: bool,
}

impl GitCache {
    pub fn new(home: &Path, offline: bool) -> Self {
        GitCache {
            root: home.join("git"),
            offline,
        }
    }

    fn db(&self, url: &str) -> PathBuf {
        self.root.join("db").join(cache_name(url))
    }

    /// Makes sure the bare clone exists, fetching new commits if `fetch`.
    fn update_db(&self, url: &str, fetch: bool) -> Result<PathBuf, String> {
        let db = self.db(url);
        if !db.join("HEAD").is_file() {
            if self.offline {
                return Err(format!(
                    "{url} is not downloaded yet and --offline was given"
                ));
            }
            if let Some(parent) = db.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
            }
            let db_str = db.to_string_lossy().into_owned();
            git(
                &["clone", "--bare", "--quiet", "--template=", url, &db_str],
                None,
            )?;
        } else if fetch && !self.offline {
            git(
                &[
                    "fetch",
                    "--quiet",
                    "--force",
                    "--tags",
                    "origin",
                    "+refs/heads/*:refs/heads/*",
                ],
                Some(&db),
            )?;
        }
        Ok(db)
    }

    /// Finds the commit `reference` points to, fetching the latest state
    /// of the repository first.
    pub fn resolve(&self, url: &str, reference: &GitRef) -> Result<String, String> {
        let db = self.update_db(url, true)?;
        let spec = format!("{}^{{commit}}", reference.rev_spec());
        git(&["rev-parse", "--verify", "--quiet", &spec], Some(&db))
            .map_err(|_| format!("{reference} not found in {url}"))
    }

    /// The files of `url` at `commit`, checked out once and then reused.
    pub fn checkout(&self, url: &str, commit: &str) -> Result<PathBuf, String> {
        let short = commit.get(..12).unwrap_or(commit);
        let dir = self
            .root
            .join("checkouts")
            .join(cache_name(url))
            .join(short);
        let done = dir.join(".belsk2-ok");
        if std::fs::read_to_string(&done).is_ok_and(|c| c == commit) {
            return Ok(dir);
        }
        let mut db = self.update_db(url, false)?;
        let has_commit = |db: &Path| {
            git(
                &["cat-file", "-e", &format!("{commit}^{{commit}}")],
                Some(db),
            )
            .is_ok()
        };
        if !has_commit(&db) {
            // A locked commit we do not have yet: fetch and look again.
            db = self.update_db(url, true)?;
            if !has_commit(&db) {
                return Err(format!("commit {commit} not found in {url}"));
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
        if let Some(parent) = dir.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        let (db_str, dir_str) = (
            db.to_string_lossy().into_owned(),
            dir.to_string_lossy().into_owned(),
        );
        git(
            &[
                "clone",
                "--quiet",
                "--no-checkout",
                "--template=",
                &db_str,
                &dir_str,
            ],
            None,
        )?;
        git(&["checkout", "--quiet", "--detach", commit], Some(&dir))?;
        std::fs::write(dir.join(".belsk2-ok"), commit)
            .map_err(|e| format!("cannot write to {}: {e}", dir.display()))?;
        Ok(dir)
    }
}

/// Turns a relative git URL (a local path) into an absolute one, so that
/// it means the same from every package.
pub fn normalize_url(url: &str, package_root: &Path) -> String {
    let is_remote = url.contains("://") || url.starts_with("git@") || Path::new(url).is_absolute();
    if is_remote {
        return url.to_string();
    }
    let full = package_root.join(url);
    std::fs::canonicalize(&full)
        .unwrap_or(full)
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_string()
}

pub(crate) fn error(manifest: &Path, dep: &str, msg: String) -> Error {
    Error::project(format!(
        "{}: dependency \"{dep}\": {msg}",
        manifest.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spellings_of_one_repository_are_the_same_source() {
        let same = [
            "https://github.com/someone/json.belsk2",
            "https://github.com/someone/json.belsk2/",
            "https://github.com/someone/json.belsk2.git",
            "https://GitHub.com/someone/json.belsk2",
            "git@github.com:someone/json.belsk2.git",
            "ssh://git@github.com:22/someone/json.belsk2",
        ];
        for url in same {
            assert!(
                same_source(url, "https://github.com/someone/json.belsk2"),
                "{url}"
            );
        }
        assert!(!same_source(
            "https://github.com/someone/json.belsk2",
            "https://github.com/other/json.belsk2"
        ));
        assert!(!same_source("/repos/a", "/repos/b"));
    }

    #[test]
    fn local_paths_are_written_relative_to_the_project() {
        let base = std::env::temp_dir().join("belsk2-lock-url");
        let app = base.join("app");
        let lib = base.join("libs").join("greeter");
        let _ = std::fs::create_dir_all(&app);
        let _ = std::fs::create_dir_all(&lib);
        let url = normalize_url("../libs/greeter", &app);
        assert_eq!(lock_url(&url, &app), "../libs/greeter");
        assert_eq!(normalize_url(&lock_url(&url, &app), &app), url);
        let remote = "https://example.com/a/b";
        assert_eq!(lock_url(remote, &app), remote);
    }
}
