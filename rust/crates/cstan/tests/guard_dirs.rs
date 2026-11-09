//! `cstan guard-write --dirs`: the built binary, JSON on stdin, exit 0 inside a folder and exit 2 everywhere else.

use std::fs;
use std::io::Write;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const CSTAN: &str = env!("CARGO_BIN_EXE_cstan");

fn base(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("cstan-guard-dirs-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(base.join("docs/architecture")).unwrap();
    fs::create_dir_all(base.join("docs/architecture-x")).unwrap();
    fs::create_dir_all(base.join("docs/research")).unwrap();
    fs::create_dir_all(base.join("src")).unwrap();
    fs::canonicalize(&base).unwrap()
}

fn run(args: &[&str], stdin: &str) -> (i32, String) {
    let mut child = Command::new(CSTAN)
        .arg("guard-write")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn call(tool: &str, path: &str) -> String {
    serde_json::json!({"tool_name": tool, "tool_input": {"file_path": path, "content": "x"}})
        .to_string()
}

#[test]
fn writes_inside_each_folder_pass() {
    let b = base("inside");
    let a = b.join("docs/architecture");
    let r = b.join("docs/research");
    let dirs = [a.to_str().unwrap(), r.to_str().unwrap()];
    let args = ["--dirs", dirs[0], dirs[1]];
    for dir in [&a, &r] {
        for tool in ["Write", "Edit", "MultiEdit"] {
            let path = dir.join("sub/new.md");
            assert_eq!(run(&args, &call(tool, path.to_str().unwrap())).0, 0);
        }
    }
}

#[test]
fn everything_else_exits_2_with_the_reason() {
    let b = base("outside");
    let a = b.join("docs/architecture");
    let a = a.to_str().unwrap();
    symlink(b.join("src"), b.join("docs/architecture/out")).unwrap();
    let args = ["--dirs", a];
    let outside = [
        b.join("docs/architecture-x/n.md"),
        b.join("src/main.rs"),
        b.join("docs/architecture"),
        b.join("docs/architecture/out/main.rs"),
    ];
    for path in &outside {
        let (code, err) = run(&args, &call("Write", path.to_str().unwrap()));
        assert_eq!(code, 2, "{path:?}");
        assert!(
            err.contains(&format!("this role may write only under {a}"))
                && err.contains("is outside them"),
            "{err}"
        );
    }
    let dotdot = format!("{a}/../../src/x.rs");
    assert_eq!(run(&args, &call("Edit", &dotdot)).0, 2);
    assert_eq!(run(&args, &call("Edit", "docs/architecture/x.md")).0, 2);
    assert_eq!(run(&args, "not json").0, 2);
    assert_eq!(run(&["--dirs"], &call("Write", a)).0, 2);
}
