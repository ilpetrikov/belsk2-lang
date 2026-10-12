use std::path::Path;

use belsk2_fmt::{format, BraceStyle, Config, IndentStyle};

fn fmt(src: &str) -> String {
    format(src, &Config::default()).unwrap_or_else(|e| format!("ERROR: {}", e.message))
}

#[test]
fn basics() {
    assert_eq!(
        fmt("fn add(a:int,b:int):int{return a+b;}"),
        "fn add(a: int, b: int): int {\n    return a + b;\n}\n"
    );
    assert_eq!(
        fmt("fn main(){\nif x>1{prinb(-x);}else{prinb(!ok);}\n}"),
        "fn main() {\n    if x > 1 {\n        prinb(-x);\n    } else {\n        prinb(!ok);\n    }\n}\n"
    );
}

#[test]
fn types_generics_and_literals() {
    assert_eq!(
        fmt("fn f<T>(items:T[]):T{Dictionary<string,int> d={};int[] a=[1,2];var m={\"a\":1};return items[0];}"),
        "fn f<T>(items: T[]): T {\n    Dictionary<string, int> d = {};\n    int[] a = [1, 2];\n    var m = { \"a\": 1 };\n    return items[0];\n}\n"
    );
    assert_eq!(
        fmt("fn main(){var x=a>>2;x>>=1;var y=empty<int>();}"),
        "fn main() {\n    var x = a >> 2;\n    x >>= 1;\n    var y = empty<int>();\n}\n"
    );
}

#[test]
fn comments_and_blank_lines_are_kept() {
    let src = "// header\n\n\n\nusing util; // why\n\n/* block\n   comment */\nfn main() { // start\n    prinb(1);\n\n\n    prinb(2); // two\n}\n";
    assert_eq!(
        fmt(src),
        "// header\n\nusing util; // why\n\n/* block\n   comment */\nfn main() { // start\n    prinb(1);\n\n    prinb(2); // two\n}\n"
    );
}

#[test]
fn multi_line_literals_and_calls() {
    let src = "fn main() {\nvar xs = [\n1,\n2,\n];\nprinb(add(1,\n2));\n}\n";
    assert_eq!(
        fmt(src),
        "fn main() {\n    var xs = [\n        1,\n        2,\n    ];\n    prinb(add(1,\n        2));\n}\n"
    );
}

#[test]
fn settings() {
    let tabs = Config {
        indent_style: IndentStyle::Tabs,
        brace_style: BraceStyle::NextLine,
        ..Config::default()
    };
    assert_eq!(
        format("fn main() { if a { b(); } else { c(); } }", &tabs).ok(),
        Some(
            "fn main()\n{\n\tif a\n\t{\n\t\tb();\n\t}\n\telse\n\t{\n\t\tc();\n\t}\n}\n".to_string()
        )
    );
    let crlf = format("fn main() {}\r\n", &Config::default()).ok();
    assert_eq!(crlf.as_deref(), Some("fn main() {}\r\n"));

    let parsed = Config::parse("indent_width = 2\nbrace_style = \"next_line\"\n");
    assert!(parsed.is_ok_and(|c| c.indent_width == 2 && c.brace_style == BraceStyle::NextLine));
    assert!(Config::parse("indent_wdth = 2").is_err());
}

#[test]
fn every_example_is_stable() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for dir in ["examples", "playground"] {
        collect(&repo.join(dir), &mut files);
    }
    assert!(files.len() > 5);
    for file in files {
        let src = std::fs::read_to_string(&file).unwrap_or_default();
        let once = format(&src, &Config::default());
        assert!(once.is_ok(), "{}: {:?}", file.display(), once.err());
        let once = once.unwrap_or_default();
        let twice = format(&once, &Config::default()).unwrap_or_default();
        assert_eq!(once, twice, "not idempotent: {}", file.display());
    }
}

fn collect(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "belsk2") {
            out.push(path);
        }
    }
}

#[test]
fn types_and_properties() {
    assert_eq!(
        fmt("class P{\nprivate static count:int=0;\nName:string{get;private set;}\nId : int { get; } = 5;\nArea:double{get{return 1;}}\n}\n"),
        "class P {\n    private static count: int = 0;\n    Name: string { get; private set; }\n    Id: int { get; } = 5;\n    Area: double {\n        get {\n            return 1;\n        }\n    }\n}\n"
    );
}

#[test]
fn inheritance() {
    assert_eq!(
        fmt("abstract class A:I{\nabstract fn f():int;\n}\nclass B:A,I{B():base(){}\n}\nfn g(x:A):bool{return x is B;}\n"),
        "abstract class A : I {\n    abstract fn f(): int;\n}\nclass B : A, I {\n    B() : base() {}\n}\nfn g(x: A): bool {\n    return x is B;\n}\n"
    );
}

#[test]
fn match_and_switch() {
    assert_eq!(
        fmt("fn main(){\nvar a=match s{Circle(r)=>r,_=>0};\nswitch x{\ncase 1,2:\nprinb(1);\ndefault:\nbreak;\n}\n}\n"),
        "fn main() {\n    var a = match s {\n        Circle(r) => r,\n        _ => 0\n    };\n    switch x {\n        case 1, 2:\n            prinb(1);\n        default:\n            break;\n    }\n}\n"
    );
}
