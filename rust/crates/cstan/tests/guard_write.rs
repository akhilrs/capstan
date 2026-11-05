//! The PM's Write/Edit guard: only paths whose real location is inside the memory folder pass.

use cstan_front::guard::check;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

struct Tree {
    base: PathBuf,
    memory: PathBuf,
}

fn tree(name: &str) -> Tree {
    let base = std::env::temp_dir().join(format!("cstan-guard-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let base = {
        fs::create_dir_all(&base).unwrap();
        fs::canonicalize(&base).unwrap()
    };
    let memory = base.join("home/.claude/projects/-proj/memory");
    fs::create_dir_all(&memory).unwrap();
    fs::create_dir_all(base.join("proj")).unwrap();
    fs::create_dir_all(base.join("worktrees/w1")).unwrap();
    fs::create_dir_all(base.join("home/.claude/projects/-other/memory")).unwrap();
    Tree { base, memory }
}

fn call(tool: &str, path: &str) -> String {
    serde_json::json!({"tool_name": tool, "tool_input": {"file_path": path, "content": "x"}})
        .to_string()
}

fn verdict(tree: &Tree, tool: &str, path: &Path) -> Result<(), String> {
    check(
        tree.memory.to_str().unwrap(),
        &call(tool, path.to_str().unwrap()),
    )
}

#[test]
fn memory_paths_are_allowed() {
    let t = tree("allowed");
    let multi = call("MultiEdit", t.memory.join("m.md").to_str().unwrap());
    assert_eq!(check(t.memory.to_str().unwrap(), &multi), Ok(()));
    let outside = call("MultiEdit", t.base.join("proj/x").to_str().unwrap());
    assert!(check(t.memory.to_str().unwrap(), &outside).is_err());
    for file in ["MEMORY.md", "note.md", "sub/dir/deep.md"] {
        assert_eq!(verdict(&t, "Write", &t.memory.join(file)), Ok(()), "{file}");
        assert_eq!(verdict(&t, "Edit", &t.memory.join(file)), Ok(()), "{file}");
    }
    fs::write(t.memory.join("existing.md"), "a").unwrap();
    assert_eq!(verdict(&t, "Edit", &t.memory.join("existing.md")), Ok(()));
}

#[test]
fn everything_else_is_refused() {
    let t = tree("refused");
    let home = t.base.join("home");
    let cases = [
        t.base.join("proj/src/main.rs"),
        t.base.join("proj"),
        t.base.join("worktrees/w1/file.rs"),
        home.join(".bashrc"),
        home.join(".claude/settings.json"),
        home.join(".claude/projects/-other/memory/MEMORY.md"),
        t.memory.clone(),
        t.memory.parent().unwrap().join("memory-evil/x.md"),
        PathBuf::from("/tmp/anything"),
        PathBuf::from("/etc/passwd"),
    ];
    for path in cases {
        assert!(verdict(&t, "Write", &path).is_err(), "{}", path.display());
        assert!(verdict(&t, "Edit", &path).is_err(), "{}", path.display());
    }
}

#[test]
fn dotdot_relative_and_odd_input_are_refused() {
    let t = tree("odd");
    let memory = t.memory.to_str().unwrap();
    let escape = format!("{memory}/../../../../proj/src/x.rs");
    let inside_dotdot = format!("{memory}/sub/../ok.md");
    for bad in [
        escape.as_str(),
        inside_dotdot.as_str(),
        "MEMORY.md",
        "./MEMORY.md",
        "",
        "~/x",
    ] {
        assert!(check(memory, &call("Write", bad)).is_err(), "{bad:?}");
    }
    for input in [
        "",
        "not json",
        "{}",
        r#"{"tool_name":"Write"}"#,
        r#"{"tool_name":"Write","tool_input":{"file_path":5}}"#,
        &call("Bash", &format!("{memory}/a")),
        &call("NotebookEdit", &format!("{memory}/a")),
    ] {
        assert!(check(memory, input).is_err(), "{input:?}");
    }
    assert!(check(
        &format!("{memory}/../x"),
        &call("Write", &format!("{memory}/a"))
    )
    .is_err());
    assert!(check("relative/dir", &call("Write", "relative/dir/a")).is_err());
}

#[test]
fn symlinks_cannot_escape() {
    let t = tree("symlink");
    let outside = t.base.join("proj");
    symlink(&outside, t.memory.join("dir-link")).unwrap();
    symlink(outside.join("file.rs"), t.memory.join("dangling")).unwrap();
    fs::write(outside.join("real.rs"), "a").unwrap();
    symlink(outside.join("real.rs"), t.memory.join("file-link")).unwrap();
    for name in ["dir-link/new.rs", "dir-link", "dangling", "file-link"] {
        assert!(
            verdict(&t, "Write", &t.memory.join(name)).is_err(),
            "{name}"
        );
    }
    // The folder itself reached through a symlink still counts as the folder.
    let alias = t.base.join("alias");
    symlink(&t.memory, &alias).unwrap();
    assert_eq!(verdict(&t, "Write", &alias.join("ok.md")), Ok(()));
    // A symlink pointing from outside into the folder resolves inside, so it is fine.
    symlink(&t.memory, t.base.join("proj/to-mem")).unwrap();
    assert_eq!(
        verdict(&t, "Write", &t.base.join("proj/to-mem/ok.md")),
        Ok(())
    );
    // A symlink inside the folder to another folder inside it is fine too.
    fs::create_dir_all(t.memory.join("a")).unwrap();
    symlink(t.memory.join("a"), t.memory.join("a-link")).unwrap();
    assert_eq!(verdict(&t, "Write", &t.memory.join("a-link/x.md")), Ok(()));
}

#[test]
fn missing_memory_folder_still_guards() {
    let t = tree("missing");
    let gone = t.base.join("home/.claude/projects/-new/memory");
    let memory = gone.to_str().unwrap();
    assert_eq!(
        check(memory, &call("Write", &format!("{memory}/MEMORY.md"))),
        Ok(())
    );
    assert!(check(
        memory,
        &call("Write", t.base.join("proj/x").to_str().unwrap())
    )
    .is_err());
}
