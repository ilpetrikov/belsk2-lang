//! Git dependencies and `belsk2.lock`. Uses local repositories; skipped
//! when `git` is not installed.

use std::path::{Path, PathBuf};
use std::process::Command;

use belsk2::project::{self, Lockfile, Mode, Options};
use belsk2::Interpreter;

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("belsk2-git-tests").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn write(root: &Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let path = root.join(path);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, text);
    }
}

fn git(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=test", "-c", "user.email=test@example.com"])
        .args(args)
        .output()
        .is_ok_and(|o| o.status.success())
}

/// A library repository whose `greet` returns `greeting`.
fn make_lib(dir: &Path, name: &str, greeting: &str) {
    write(
        dir,
        &[
            (
                "belsk2.toml",
                &format!("[package]\nname = \"{name}\"\nversion = \"1.0.0\"\n"),
            ),
            (
                "src/lib.belsk2",
                &format!("fn greet(): string {{ return \"{greeting}\"; }}\n"),
            ),
        ],
    );
    assert!(git(dir, &["init", "-q", "-b", "main"]));
    assert!(git(dir, &["add", "-A"]));
    assert!(git(dir, &["commit", "-qm", "first"]));
}

fn commit_greeting(dir: &Path, greeting: &str) {
    write(
        dir,
        &[(
            "src/lib.belsk2",
            &format!("fn greet(): string {{ return \"{greeting}\"; }}\n"),
        )],
    );
    assert!(git(dir, &["commit", "-qam", greeting]));
}

fn make_app(dir: &Path, deps: &str) {
    write(
        dir,
        &[
            (
                "belsk2.toml",
                &format!(
                    "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\n{deps}\n"
                ),
            ),
            (
                "src/main.belsk2",
                "using greeter;\nfn main() { prinb(greet()); }\n",
            ),
        ],
    );
}

fn options(home: &Path) -> Options {
    Options {
        home: home.to_path_buf(),
        ..Options::default()
    }
}

fn run(app: &Path, opts: &Options) -> Result<String, Vec<String>> {
    let mut loaded = project::load_with(app, Mode::Run, opts)
        .map_err(|(errors, _)| errors.iter().map(|e| e.message.clone()).collect::<Vec<_>>())?;
    let errors = project::check(&mut loaded);
    if !errors.is_empty() {
        return Err(errors.iter().map(|e| e.message.clone()).collect());
    }
    let mut out = Vec::new();
    project::run(&loaded, &mut Interpreter::new(), &[], &mut out).map_err(|e| vec![e.message])?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

fn assert_err(result: Result<String, Vec<String>>, needle: &str) {
    let output = result.as_ref().ok().cloned();
    let errors = result.err().unwrap_or_default();
    assert!(
        errors.iter().any(|e| e.contains(needle)),
        "expected {needle:?} in {errors:?} (output: {output:?})"
    );
}

#[test]
fn tag_dependency_and_lockfile() {
    if !git_available() {
        return;
    }
    let dir = temp_dir("tag");
    let lib = dir.join("greeter");
    make_lib(&lib, "greeter", "v1");
    assert!(git(&lib, &["tag", "v1.0"]));
    commit_greeting(&lib, "v2");

    let app = dir.join("app");
    make_app(&app, "greeter = { git = \"../greeter\", tag = \"v1.0\" }");
    let opts = options(&dir.join("home"));
    assert_eq!(run(&app, &opts), Ok("v1\n".to_string()));

    let lock = Lockfile::load(&app).unwrap_or_default();
    let entry = lock.packages.first();
    assert_eq!(lock.packages.len(), 1);
    assert!(entry.is_some_and(|p| p.name == "greeter"
        && p.version == "1.0.0"
        && p.reference == "tag=v1.0"
        && p.commit.len() == 40));
    // A local repository is written relative to the project.
    assert_eq!(entry.map(|p| p.git.as_str()), Some("../greeter"));
}

#[test]
fn branch_is_pinned_until_update() {
    if !git_available() {
        return;
    }
    let dir = temp_dir("branch");
    let lib = dir.join("greeter");
    make_lib(&lib, "greeter", "old");
    let app = dir.join("app");
    make_app(
        &app,
        "greeter = { git = \"../greeter\", branch = \"main\" }",
    );
    let opts = options(&dir.join("home"));
    assert_eq!(run(&app, &opts), Ok("old\n".to_string()));

    commit_greeting(&lib, "new");
    // belsk2.lock keeps the old commit...
    assert_eq!(run(&app, &opts), Ok("old\n".to_string()));
    // ...until an update.
    let update = Options {
        update: true,
        ..opts.clone()
    };
    assert_eq!(run(&app, &update), Ok("new\n".to_string()));
    assert_eq!(run(&app, &opts), Ok("new\n".to_string()));

    // Works without the network once downloaded.
    let offline = Options {
        offline: true,
        ..opts.clone()
    };
    assert_eq!(run(&app, &offline), Ok("new\n".to_string()));
}

#[test]
fn locked_refuses_to_change_the_lockfile() {
    if !git_available() {
        return;
    }
    let dir = temp_dir("locked");
    make_lib(&dir.join("greeter"), "greeter", "hi");
    let app = dir.join("app");
    make_app(&app, "greeter = { git = \"../greeter\" }");
    let locked = Options {
        locked: true,
        ..options(&dir.join("home"))
    };
    assert_err(
        run(&app, &locked),
        "needs to be updated, but --locked was given",
    );
    assert_eq!(
        run(&app, &options(&dir.join("home"))),
        Ok("hi\n".to_string())
    );
    assert_eq!(run(&app, &locked), Ok("hi\n".to_string()));
}

#[test]
fn offline_without_cache_fails() {
    if !git_available() {
        return;
    }
    let dir = temp_dir("offline");
    make_lib(&dir.join("greeter"), "greeter", "hi");
    let app = dir.join("app");
    make_app(&app, "greeter = { git = \"../greeter\" }");
    let offline = Options {
        offline: true,
        ..options(&dir.join("home"))
    };
    assert_err(run(&app, &offline), "--offline");
}

#[test]
fn git_errors() {
    if !git_available() {
        return;
    }
    let dir = temp_dir("errors");
    make_lib(&dir.join("greeter"), "greeter", "hi");
    let not_a_package = dir.join("plain");
    write(&not_a_package, &[("README.md", "hello")]);
    assert!(git(&not_a_package, &["init", "-q"]));
    assert!(git(&not_a_package, &["add", "-A"]));
    assert!(git(&not_a_package, &["commit", "-qm", "x"]));

    let opts = options(&dir.join("home"));
    let cases = [
        (
            "greeter = { git = \"../greeter\", tag = \"v9\" }",
            "tag 'v9' not found",
        ),
        (
            "greeter = { git = \"../greeter\", branch = \"dev\" }",
            "branch 'dev' not found",
        ),
        ("greeter = { git = \"../missing\" }", "failed"),
        ("greeter = { git = \"../plain\" }", "has no belsk2.toml"),
    ];
    for (i, (deps, needle)) in cases.iter().enumerate() {
        let app = dir.join(format!("app{i}"));
        make_app(&app, deps);
        assert_err(run(&app, &opts), needle);
    }
}

#[test]
fn transitive_dependencies_and_conflicts() {
    if !git_available() {
        return;
    }
    let dir = temp_dir("transitive");
    let lib = dir.join("greeter");
    make_lib(&lib, "greeter", "v1");
    assert!(git(&lib, &["tag", "v1"]));
    commit_greeting(&lib, "v2");

    // `wrapper` is a git dependency that itself depends on greeter v1.
    let wrapper = dir.join("wrapper");
    write(
        &wrapper,
        &[
            (
                "belsk2.toml",
                &format!(
                    "[package]\nname = \"wrapper\"\nversion = \"0.2.0\"\n[dependencies]\ngreeter = {{ git = \"{}\", tag = \"v1\" }}\n",
                    lib.display().to_string().replace('\\', "/")
                ),
            ),
            ("src/lib.belsk2", "using greeter;\nfn wrapped(): string { return \"[\" + greet() + \"]\"; }\n"),
        ],
    );
    assert!(git(&wrapper, &["init", "-q", "-b", "main"]));
    assert!(git(&wrapper, &["add", "-A"]));
    assert!(git(&wrapper, &["commit", "-qm", "w"]));

    let app = dir.join("app");
    write(
        &app,
        &[
            (
                "belsk2.toml",
                "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\nwrapper = { git = \"../wrapper\" }\n",
            ),
            ("src/main.belsk2", "using wrapper;\nfn main() { prinb(wrapped()); }\n"),
        ],
    );
    let opts = options(&dir.join("home"));
    assert_eq!(run(&app, &opts), Ok("[v1]\n".to_string()));
    let lock = Lockfile::load(&app).unwrap_or_default();
    let mut names: Vec<&str> = lock.packages.iter().map(|p| p.name.as_str()).collect();
    names.sort();
    assert_eq!(names, ["greeter", "wrapper"]);

    // The app also wants greeter, but from the main branch: a conflict.
    let conflict = dir.join("conflict");
    write(
        &conflict,
        &[
            (
                "belsk2.toml",
                "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\ngreeter = { git = \"../greeter\" }\nwrapper = { git = \"../wrapper\" }\n",
            ),
            ("src/main.belsk2", "using wrapper;\nfn main() { prinb(wrapped()); }\n"),
        ],
    );
    assert_err(run(&conflict, &opts), "is used from two different sources");
}
