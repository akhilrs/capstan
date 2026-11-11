//! Workspace mode has no switch: the removed preview gate leaves no trace in the tree. A `git grep` over the checkout
//! (or a walk of the files when there is no git) finds neither name outside the dated investigation records, the
//! changelog and this file.
use std::path::{Path, PathBuf};
use std::process::Command;

const NAMES: [&str; 6] = [
    "CAPSTAN_WORKSPACE_PREVIEW",
    "WORKSPACE_MODE_READY",
    "WORKSPACE_PREVIEW_ENV",
    "WORKSPACE_NOT_COMPLETE",
    "WORKSPACE_ROOT_NOT_REPO",
    // The old config function, which the daemon's verify gate of the same shape is not.
    "capstan_config::workspace_gate",
];
const SELF: &str = "rust/crates/cstan/tests/no_preview_variable.rs";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn allowed(relative: &str) -> bool {
    relative.starts_with("docs/investigations/") || relative == "CHANGELOG.md" || relative == SELF
}

fn via_git(root: &Path) -> Option<Vec<String>> {
    let mut command = Command::new("git");
    command.arg("-C").arg(root).args(["grep", "-l", "-F"]);
    for name in NAMES {
        command.args(["-e", name]);
    }
    let output = command.arg("--").arg(".").output().ok()?;
    // 0: matches, 1: none; anything else (not a work tree, no git) falls back to the walk.
    match output.status.code() {
        Some(0) | Some(1) => Some(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(|l| l.trim_start_matches("./").to_string())
                .collect(),
        ),
        _ => None,
    }
}

fn walk(root: &Path, directory: &Path, found: &mut Vec<String>) {
    for entry in std::fs::read_dir(directory).unwrap().flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            if name == ".git" || name == "target" || name == "node_modules" || name == ".capstan" {
                continue;
            }
            walk(root, &path, found);
        } else if let Ok(text) = std::fs::read_to_string(&path) {
            if NAMES.iter().any(|n| text.contains(n)) {
                let relative = path.strip_prefix(root).unwrap();
                found.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
}

#[test]
fn no_file_names_the_removed_preview_gate() {
    let root = root();
    let found = via_git(&root).unwrap_or_else(|| {
        let mut found = Vec::new();
        walk(&root, &root, &mut found);
        found
    });
    let stray: Vec<&String> = found.iter().filter(|f| !allowed(f)).collect();
    assert!(
        stray.is_empty(),
        "the removed gate is still named in {stray:?}"
    );
}
