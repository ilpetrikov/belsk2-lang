//! Projects (`belsk2.toml`), modules (`using`) and path dependencies.

use std::path::{Path, PathBuf};

use belsk2::project::{self, Mode, Template};
use belsk2::{ErrorKind, Interpreter};

/// A fresh directory for one test.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("belsk2-tests").join(name);
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

const MANIFEST: &str = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n";

/// Loads, checks and runs; returns the output or the error messages.
fn run_loaded(
    loaded: Result<project::Loaded, (Vec<belsk2::Error>, belsk2::SourceMap)>,
) -> Result<String, Vec<String>> {
    let mut loaded = loaded
        .map_err(|(errors, _)| errors.iter().map(|e| e.message.clone()).collect::<Vec<_>>())?;
    let errors = project::check(&mut loaded);
    if !errors.is_empty() {
        return Err(errors.iter().map(|e| e.message.clone()).collect());
    }
    let mut out = Vec::new();
    project::run(&loaded, &mut Interpreter::new(), &[], &mut out).map_err(|e| vec![e.message])?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

fn run_project(root: &Path) -> Result<String, Vec<String>> {
    run_loaded(project::load(root, Mode::Run))
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
fn new_program_runs() {
    let dir = temp_dir("new_program");
    let files = project::create(&dir, "hello-world", Template::Bin);
    assert!(files.is_ok_and(|f| f.len() == 3));
    assert_eq!(
        run_project(&dir),
        Ok("Hello from hello-world!\n".to_string())
    );
    let manifest = project::Manifest::load(&dir.join("belsk2.toml"));
    assert!(manifest.is_ok_and(|m| m.package.name == "hello-world" && m.package.version == "0.1.0"));
    // A second `create` in the same place is refused.
    assert!(project::create(&dir, "hello-world", Template::Bin).is_err());
}

#[test]
fn new_library_cannot_be_run() {
    let dir = temp_dir("new_library");
    assert!(project::create(&dir, "mylib", Template::Lib).is_ok());
    assert_err(run_project(&dir), "this is a library");
    let mut loaded = project::load(&dir, Mode::All).ok();
    assert!(loaded
        .as_mut()
        .is_some_and(|l| project::check(l).is_empty()));
}

#[test]
fn invalid_names_are_rejected() {
    let dir = temp_dir("invalid_names");
    for name in ["Hello", "1abc", "a b", "", "using"] {
        assert!(
            project::create(&dir, name, Template::Bin).is_err(),
            "{name}"
        );
    }
}

#[test]
fn modules_and_path_dependencies() {
    let dir = temp_dir("modules");
    let app = dir.join("app");
    write(
        &app,
        &[
            (
                "belsk2.toml",
                "[package]\nname = \"app\"\nversion = \"1.2.3\"\n\n[dependencies]\nmath-utils = { path = \"../math\" }\n",
            ),
            (
                "src/main.belsk2",
                "using text.fmt;\nusing math_utils;\nusing math_utils.stats;\n\
                 fn main() { prinb(shout(str(square(4)))); prinb(mean([1.0, 2, 6])); prinb(greeting); }\n",
            ),
            (
                "src/text/fmt.belsk2",
                "var greeting = \"hi\";\nfn shout(s: string): string { return s + \"!\"; }\n",
            ),
        ],
    );
    write(
        &dir.join("math"),
        &[
            ("belsk2.toml", "[package]\nname = \"math-utils\"\nversion = \"0.1.0\"\n"),
            ("src/lib.belsk2", "fn square(x: int): int { return x * x; }\n"),
            (
                "src/stats.belsk2",
                "using sum;\nfn mean(xs: double[]): double { return total(xs) / len(xs); }\n",
            ),
            (
                "src/sum.belsk2",
                "fn total(xs: double[]): double { double t = 0; for x in xs { t += x; } return t; }\n",
            ),
        ],
    );
    assert_eq!(run_project(&app), Ok("16!\n3\nhi\n".to_string()));
    let loaded = project::load(&app, Mode::All);
    assert!(loaded.is_ok_and(|l| project::summary(&l) == "app v1.2.3 (5 modules)"));
}

#[test]
fn modules_run_once_in_dependency_order() {
    let dir = temp_dir("init_order");
    write(
        &dir,
        &[
            ("belsk2.toml", MANIFEST),
            (
                "src/main.belsk2",
                "using a;\nusing b;\nusing c;\nfn main() { note(\"main\"); }\n",
            ),
            ("src/a.belsk2", "using c;\nvar a_ready = note(\"a\");\n"),
            ("src/b.belsk2", "using c;\nvar b_ready = note(\"b\");\n"),
            (
                "src/c.belsk2",
                "fn note(s: string): bool { prinb(s); return true; }\nvar c_ready = note(\"c\");\n",
            ),
            ("src/unused.belsk2", "var never = prinb(\"never\");\n"),
        ],
    );
    assert_eq!(run_project(&dir), Ok("c\na\nb\nmain\n".to_string()));
}

#[test]
fn module_errors() {
    type Case<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a str);
    let cases: &[Case] = &[
        (
            "missing",
            &[("src/main.belsk2", "using nope;\n")],
            "module 'nope' not found",
        ),
        (
            "cycle",
            &[
                ("src/main.belsk2", "using a;\n"),
                ("src/a.belsk2", "using b;\n"),
                ("src/b.belsk2", "using a;\n"),
            ],
            "cycle: a -> b -> a",
        ),
        (
            "readonly",
            &[
                ("src/main.belsk2", "using a;\nx = 2;\n"),
                ("src/a.belsk2", "var x = 1;\n"),
            ],
            "'x' is imported from module 'a' and cannot be assigned",
        ),
        (
            "conflict",
            &[
                ("src/main.belsk2", "using a;\nusing b;\n"),
                ("src/a.belsk2", "fn f() { }\n"),
                ("src/b.belsk2", "fn f() { }\n"),
            ],
            "'f' is declared in both 'a' and 'b'",
        ),
        (
            "types",
            &[
                ("src/main.belsk2", "using a;\nf(\"s\");\n"),
                ("src/a.belsk2", "fn f(x: int) { }\n"),
            ],
            "cannot use string as int in argument 1 of 'f'",
        ),
        (
            "main",
            &[("src/main.belsk2", "using main;\n")],
            "'main' cannot be used",
        ),
        (
            "twice",
            &[
                ("src/main.belsk2", "using a;\nusing a;\n"),
                ("src/a.belsk2", ""),
            ],
            "already used",
        ),
        (
            "nested",
            &[
                ("src/main.belsk2", "if true { using a; }\n"),
                ("src/a.belsk2", ""),
            ],
            "must be at the top level",
        ),
        (
            "no main",
            &[("src/other.belsk2", "")],
            "main.belsk2 not found",
        ),
    ];
    for (name, files, needle) in cases {
        let dir = temp_dir(&format!("module_errors_{}", name.replace(' ', "_")));
        write(&dir, &[("belsk2.toml", MANIFEST)]);
        write(&dir, files);
        assert_err(run_project(&dir), needle);
    }
}

#[test]
fn errors_point_at_the_right_file() {
    let dir = temp_dir("error_files");
    write(
        &dir,
        &[
            ("belsk2.toml", MANIFEST),
            (
                "src/main.belsk2",
                "using calc;\nfn main() { prinb(div(1, 0)); }\n",
            ),
            (
                "src/calc.belsk2",
                "fn div(a: int, b: int): int {\n    return a / b;\n}\n",
            ),
        ],
    );
    let mut loaded = project::load(&dir, Mode::Run).ok();
    assert!(loaded.is_some());
    let Some(loaded) = loaded.as_mut() else {
        return;
    };
    assert!(project::check(loaded).is_empty());
    let mut out = Vec::new();
    let err = project::run(loaded, &mut Interpreter::new(), &[], &mut out).err();
    let rendered = err
        .map(|e| e.render_in(&loaded.sources))
        .unwrap_or_default();
    assert!(rendered.contains("calc.belsk2:2:14"), "{rendered}");
    assert!(rendered.contains("return a / b;"), "{rendered}");
}

#[test]
fn check_all_finds_errors_in_unused_files() {
    let dir = temp_dir("check_all");
    write(
        &dir,
        &[
            ("belsk2.toml", MANIFEST),
            ("src/main.belsk2", "fn main() { prinb(1); }\n"),
            ("src/broken.belsk2", "var x: int = \"s\";\n"),
        ],
    );
    assert_eq!(run_project(&dir), Ok("1\n".to_string()));
    let mut loaded = project::load(&dir, Mode::All).ok();
    let errors = loaded.as_mut().map(project::check).unwrap_or_default();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors.first().map(|e| e.kind), Some(ErrorKind::Type));
}

#[test]
fn single_files_can_use_their_neighbours() {
    let dir = temp_dir("single_file");
    write(
        &dir,
        &[
            (
                "script.belsk2",
                "using helpers;\nfn main() { prinb(twice(21)); }\n",
            ),
            (
                "helpers.belsk2",
                "fn twice(x: int): int { return x * 2; }\n",
            ),
        ],
    );
    assert_eq!(
        run_loaded(project::load_file(&dir.join("script.belsk2"))),
        Ok("42\n".to_string())
    );
    let mut out = Vec::new();
    let e = Interpreter::new().run_source_with_writer("using helpers;", &mut out);
    assert!(e.is_err_and(|e| e.message.contains("needs a file or a project")));
}

#[test]
fn manifest_errors() {
    let cases = [
        ("[package]\nname = \"Bad\"\nversion = \"0.1.0\"\n", "invalid package name"),
        ("[package]\nname = \"ok\"\nversion = \"1\"\n", "invalid version"),
        ("[package]\nname = \"ok\"\n", "missing field `version`"),
        ("[package]\nname = \"ok\"\nversion = \"0.1.0\"\nedition = 1\n", "unknown field `edition`"),
        (
            "[package]\nname = \"ok\"\nversion = \"0.1.0\"\n[dependencies]\nx = { path = \"a\", git = \"b\" }\n",
            "both `path` and `git`",
        ),
        (
            "[package]\nname = \"ok\"\nversion = \"0.1.0\"\n[dependencies]\nx = { }\n",
            "needs `path",
        ),
        (
            "[package]\nname = \"ok\"\nversion = \"0.1.0\"\n[dependencies]\nx = { path = \"a\", tag = \"v1\" }\n",
            "only apply to git",
        ),
        (
            "[package]\nname = \"ok\"\nversion = \"0.1.0\"\n[dependencies]\nx = { path = \"../nowhere\" }\n",
            "no belsk2.toml in",
        ),
    ];
    for (i, (manifest, needle)) in cases.iter().enumerate() {
        let dir = temp_dir(&format!("manifest_{i}"));
        write(&dir, &[("belsk2.toml", manifest), ("src/main.belsk2", "")]);
        assert_err(run_project(&dir), needle);
    }
}

#[test]
fn dependency_name_must_match_package() {
    let dir = temp_dir("dep_name");
    write(
        &dir.join("app"),
        &[
            ("belsk2.toml", "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\nfoo = { path = \"../bar\" }\n"),
            ("src/main.belsk2", ""),
        ],
    );
    write(
        &dir.join("bar"),
        &[
            (
                "belsk2.toml",
                "[package]\nname = \"bar\"\nversion = \"0.1.0\"\n",
            ),
            ("src/lib.belsk2", ""),
        ],
    );
    assert_err(run_project(&dir.join("app")), "points to package \"bar\"");
}

#[test]
fn find_root_walks_up() {
    let dir = temp_dir("find_root");
    write(
        &dir,
        &[("belsk2.toml", MANIFEST), ("src/deep/x.belsk2", "")],
    );
    let found = project::find_root(&dir.join("src").join("deep"));
    assert_eq!(found.map(|p| p.ends_with("find_root")), Some(true));
}

#[test]
fn programs_start_in_main() {
    let dir = temp_dir("main_rules");
    let cases: [(&str, Result<&str, &str>); 7] = [
        ("prinb(1);\n", Err("code must be inside a function")),
        ("var x = 1;\nx = 2;\nfn main() { }\n", Err("code must be inside a function")),
        ("fn helper() { }\n", Err("has no `fn main()`")),
        ("fn main(x: int) { }\n", Err("`main` must be declared as")),
        ("fn main(): string { return \"\"; }\n", Err("`main` must be declared as")),
        (
            "var greeting = \"hi\";\nidb 1 = \"banked\";\nfn main() { prinb(greeting); prinb(1); }\n",
            Ok("hi\nbanked\n"),
        ),
        ("fn main(): int { prinb(\"bye\"); return 0; }\n", Ok("bye\n")),
    ];
    for (i, (src, expected)) in cases.iter().enumerate() {
        let file = dir.join(format!("case{i}.belsk2"));
        write(&dir, &[(&format!("case{i}.belsk2"), src)]);
        let result = run_loaded(project::load_file(&file));
        match expected {
            Ok(out) => assert_eq!(result, Ok(out.to_string()), "{src}"),
            Err(needle) => assert_err(result, needle),
        }
    }
}

#[test]
fn main_gets_arguments_and_returns_an_exit_code() {
    let dir = temp_dir("main_args");
    write(
        &dir,
        &[(
            "app.belsk2",
            "fn main(args: string[]): int { for a in args { prinb(a); } return len(args); }\n",
        )],
    );
    let mut loaded = project::load_file(&dir.join("app.belsk2")).ok();
    assert!(loaded
        .as_mut()
        .is_some_and(|l| project::check(l).is_empty()));
    let Some(loaded) = loaded else {
        return;
    };
    let mut out = Vec::new();
    let args = ["x".to_string(), "y".to_string()];
    let code = project::run(&loaded, &mut Interpreter::new(), &args, &mut out);
    assert_eq!(code.ok(), Some(Some(2)));
    assert_eq!(String::from_utf8_lossy(&out), "x\ny\n");
}

#[test]
fn repl_input_may_have_statements() {
    let interp = Interpreter::new();
    assert!(interp.compile_repl("var x = 1; prinb(x);").is_ok());
    assert!(interp.compile("var x = 1; prinb(x);").is_err());
}
