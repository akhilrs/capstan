//! `ProjectGit::commit_on_head` on a real repository: an ancestor of HEAD is on it; a commit only on another branch, a
//! missing id and a blob id are not.

mod common;

use capstan_daemon::deps::GitPort;
use capstan_daemon::ports::ProjectGit;
use common::private_tempdir;
use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

#[test]
fn only_a_commit_in_the_history_of_head_is_on_head() {
    let dir = private_tempdir();
    let root = dir.path();
    git(root, &["init", "-q"]);
    std::fs::write(root.join("a.txt"), "a\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "feat: a"]);
    let first = git(root, &["rev-parse", "HEAD"]);
    std::fs::write(root.join("b.txt"), "b\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "feat: b"]);
    let head = git(root, &["rev-parse", "HEAD"]);
    git(root, &["checkout", "-q", "-b", "other", &first]);
    std::fs::write(root.join("c.txt"), "c\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "feat: c"]);
    let other = git(root, &["rev-parse", "HEAD"]);
    git(root, &["checkout", "-q", "main"]);
    let blob = git(root, &["rev-parse", "HEAD:a.txt"]);

    let port = ProjectGit::new(root);
    assert_eq!(port.commit_on_head(&first).unwrap(), Some(true));
    assert_eq!(port.commit_on_head(&head).unwrap(), Some(true));
    assert_eq!(port.commit_on_head(&other).unwrap(), Some(false));
    assert_eq!(port.commit_on_head(&"0".repeat(40)).unwrap(), Some(false));
    assert_eq!(port.commit_on_head(&blob).unwrap(), Some(false));
}
