use std::collections::HashMap;
use std::path::Path;

use belsk2_analysis::{analyze_path, analyze_source, CompletionKind, HintKind, Pos};

const SRC: &str = "fn add(a: int, b: int): int {
    return a + b;
}

fn main() {
    var total = add(1, 2);
    for x in [1, 2] {
        total += x;
    }
    prinb(total);
}
";

#[test]
fn hover_definition_references() {
    let a = analyze_source("t.belsk2", SRC);
    assert!(a.diagnostics.is_empty(), "{:?}", a.diagnostics);
    let f = a.files.first().map(|f| f.id).unwrap_or_default();

    let hover = a.hover(f, Pos::new(6, 17)).map(|h| h.1).unwrap_or_default();
    assert!(hover.contains("fn add(a: int, b: int): int"), "{hover}");
    let hover = a
        .hover(f, Pos::new(10, 12))
        .map(|h| h.1)
        .unwrap_or_default();
    assert!(hover.contains("var total: int"), "{hover}");
    let hover = a.hover(f, Pos::new(10, 5)).map(|h| h.1).unwrap_or_default();
    assert!(hover.contains("Prints"), "{hover}");
    let hover = a.hover(f, Pos::new(2, 12)).map(|h| h.1).unwrap_or_default();
    assert!(hover.contains("(parameter) a: int"), "{hover}");

    let def = a.definition(f, Pos::new(6, 17));
    assert_eq!(def.map(|d| d.range.start), Some(Pos::new(1, 4)));
    let def = a.definition(f, Pos::new(8, 18));
    assert_eq!(def.map(|d| d.range.start), Some(Pos::new(7, 9)));

    // Declaration, `+=` target and `prinb(total)`.
    assert_eq!(a.references(f, Pos::new(6, 9)).len(), 3);
    assert!(a.can_rename(f, Pos::new(6, 9)));
    assert!(!a.can_rename(f, Pos::new(10, 5)));
}

#[test]
fn hints_and_symbols() {
    let a = analyze_source("t.belsk2", SRC);
    let f = a.files.first().map(|f| f.id).unwrap_or_default();
    let hints: Vec<(Pos, &str, HintKind)> = a
        .hints_for(f)
        .map(|h| (h.pos, h.label.as_str(), h.kind))
        .collect();
    assert!(
        hints.contains(&(Pos::new(6, 14), ": int", HintKind::Type)),
        "{hints:?}"
    );
    assert!(
        hints.contains(&(Pos::new(6, 21), "a:", HintKind::Parameter)),
        "{hints:?}"
    );
    assert!(
        hints.contains(&(Pos::new(7, 10), ": int", HintKind::Type)),
        "{hints:?}"
    );
    let names: Vec<&str> = a
        .document_symbols(f)
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(names, ["add", "main"]);
}

#[test]
fn completions_respect_scopes() {
    let a = analyze_source("t.belsk2", SRC);
    let f = a.files.first().map(|f| f.id).unwrap_or_default();
    let at = |line, col| -> Vec<(String, CompletionKind)> {
        a.completions(f, Pos::new(line, col))
            .into_iter()
            .map(|c| (c.label, c.kind))
            .collect()
    };
    let inside_loop = at(8, 9);
    assert!(inside_loop.contains(&("x".into(), CompletionKind::Variable)));
    assert!(inside_loop.contains(&("total".into(), CompletionKind::Variable)));
    assert!(inside_loop.contains(&("add".into(), CompletionKind::Function)));
    assert!(inside_loop.contains(&("prinb".into(), CompletionKind::Function)));
    assert!(inside_loop.contains(&("while".into(), CompletionKind::Keyword)));
    // `x` is not visible after the loop, `a` not outside `add`.
    let after = at(10, 5);
    assert!(!after.contains(&("x".into(), CompletionKind::Variable)));
    assert!(!after.contains(&("a".into(), CompletionKind::Parameter)));
    assert!(at(2, 5).contains(&("a".into(), CompletionKind::Parameter)));
}

#[test]
fn errors_have_ranges() {
    let a = analyze_source("t.belsk2", "fn main() {\n    var y = len(5);\n}\n");
    let d = a.diagnostics.first();
    assert!(d.is_some_and(|d| d.message.contains("len()") && d.range.start.line == 2));

    // A syntax error does not also report a missing `main`.
    let a = analyze_source("t.belsk2", "fn main( {\n");
    assert_eq!(a.diagnostics.len(), 1, "{:?}", a.diagnostics);
}

#[test]
fn project_with_unsaved_changes() {
    let dir = std::env::temp_dir()
        .join("belsk2-analysis-tests")
        .join("proj");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(dir.join("src"));
    let _ = std::fs::write(
        dir.join("belsk2.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
    );
    let _ = std::fs::write(
        dir.join("src/util.belsk2"),
        "fn twice(n: int): int { return n * 2; }\n",
    );
    let main = dir.join("src/main.belsk2");
    let _ = std::fs::write(&main, "fn main() {}\n");

    let mut overlays = HashMap::new();
    overlays.insert(
        main.clone(),
        "using util;\nfn main() {\n    prinb(twice(4));\n}\n".to_string(),
    );
    let a = analyze_path(&main, &overlays);
    assert!(a.diagnostics.is_empty(), "{:?}", a.diagnostics);
    let f = a.file_id(&main).unwrap_or_default();
    let def = a.definition(f, Pos::new(3, 12));
    let util = a.file_id(&dir.join("src/util.belsk2"));
    assert_eq!(def.map(|d| Some(d.file)), Some(util));
    let hover = a.hover(f, Pos::new(3, 12)).map(|h| h.1).unwrap_or_default();
    assert!(hover.contains("from module `util`"), "{hover}");

    let graph: Vec<(&str, usize)> = a
        .modules
        .iter()
        .map(|m| (m.name.as_str(), m.uses.len()))
        .collect();
    assert!(
        graph.contains(&("main", 1)) && graph.contains(&("util", 0)),
        "{graph:?}"
    );
    assert!(a.modules.iter().any(|m| m.entry && m.name == "main"));
    assert!(a
        .completions(f, Pos::new(1, 7))
        .iter()
        .any(|c| c.label == "util"));
    assert!(!Path::new(&dir.join("belsk2.lock")).exists());
}
