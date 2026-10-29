//! Classification of git's config error output: fake input pinned to git 2.39.5's exact lines, the fallback for the
//! repository directories, and one real-git test that runs on git 2.39.x only.

use capstan_kernel::integrate::{classify_config_error, repo_dirs_fallback, ConfigError};
use std::path::{Path, PathBuf};
use std::process::Command;

const BAD_LINE: &str = "fatal: bad config line 6 in file .git/config\n";
const ACCESS_DENIED: &str = "warning: unable to access '.git/config': Permission denied\nfatal: unable to access '.git/config': Permission denied\n";
const DIR_IN_PLACE: &str = "warning: unable to access '.git/config': Is a directory\nwarning: unable to access '.git/config': Is a directory\nfatal: unknown error occurred while reading the configuration files\n";
const READ_FAILED: &str = "fatal: unable to read config file '.git/config': Is a directory\n";
const UNKNOWN: &str = "fatal: unknown error occurred while reading the configuration files\n";

fn unreadable(stderr: &str, root: &Path, git_dir: &Path, common: &Path) -> bool {
    classify_config_error(stderr.as_bytes(), root, git_dir, common) == ConfigError::Unreadable
}

fn message(stderr: &[u8], root: &Path) -> String {
    match classify_config_error(stderr, root, &root.join(".git"), &root.join(".git")) {
        ConfigError::Unavailable { message } => message,
        ConfigError::Unreadable => panic!("expected Unavailable"),
    }
}

fn scratch() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    let git = root.join(".git");
    std::fs::create_dir_all(&git).unwrap();
    (tmp, root, git)
}

#[test]
fn each_substring_naming_the_repo_config_is_unreadable() {
    let (_tmp, root, git) = scratch();
    let abs = git.join("config");
    for line in [
        format!("fatal: bad config line 1 in file {}\n", abs.display()),
        format!("warning: unable to access '{}': x\n", abs.display()),
        format!("error: '{}': Permission denied\n", abs.display()),
        format!("fatal: unable to read config file '{}': x\n", abs.display()),
        format!(
            "fatal: unknown error occurred while reading the configuration files\nwarning: unable to access '{}': x\n",
            abs.display()
        ),
    ] {
        assert!(unreadable(&line, &root, &git, &git), "{line}");
    }
    assert!(unreadable(BAD_LINE, &root, &git, &git));
    assert!(unreadable(ACCESS_DENIED, &root, &git, &git));
    assert!(unreadable(READ_FAILED, &root, &git, &git));
}

#[test]
fn a_global_file_is_unavailable() {
    let (_tmp, root, git) = scratch();
    for line in [
        "fatal: bad config line 1 in file /home/u/.gitconfig\n",
        "warning: unable to access '/home/u/.gitconfig': Permission denied\n",
        "fatal: unable to access '~/.gitconfig': Permission denied\n",
        "fatal: unable to read config file '/home/u/.gitconfig': x\n",
    ] {
        assert!(!unreadable(line, &root, &git, &git), "{line}");
    }
}

#[test]
fn dubious_ownership_wins() {
    let (_tmp, root, git) = scratch();
    let text = format!("{BAD_LINE}fatal: detected dubious ownership in repository at '/x'\n");
    let got = classify_config_error(text.as_bytes(), &root, &git, &git);
    match got {
        ConfigError::Unavailable { message } => assert!(message.contains("dubious ownership")),
        ConfigError::Unreadable => panic!("expected Unavailable"),
    }
}

#[test]
fn a_later_line_naming_the_repo_config_counts() {
    let (_tmp, root, git) = scratch();
    let text = "warning: unable to access '/home/u/.gitconfig': Permission denied\nfatal: bad config line 3 in file .git/config\n";
    assert!(unreadable(text, &root, &git, &git));
}

#[test]
fn config_worktree_in_a_linked_git_dir_counts() {
    let (_tmp, root, git) = scratch();
    let linked = git.join("worktrees/w");
    std::fs::create_dir_all(&linked).unwrap();
    let text = format!(
        "fatal: bad config line 2 in file {}\n",
        linked.join("config.worktree").display()
    );
    assert!(unreadable(&text, &root, &linked, &git));
    let common = format!(
        "fatal: bad config line 2 in file {}\n",
        git.join("config").display()
    );
    assert!(unreadable(&common, &root, &linked, &git));
}

#[test]
fn a_non_utf8_byte_does_not_change_the_result() {
    let (_tmp, root, git) = scratch();
    let mut bytes = b"warning: junk \xff\xfe here\n".to_vec();
    bytes.extend_from_slice(BAD_LINE.as_bytes());
    assert_eq!(
        classify_config_error(&bytes, &root, &git, &git),
        ConfigError::Unreadable
    );
}

#[test]
fn an_unknown_message_is_unavailable_with_the_line() {
    let (_tmp, root, _git) = scratch();
    assert_eq!(
        message(b"something odd happened\n", &root),
        "something odd happened"
    );
    assert_eq!(message(b"\n  \nwhat\n", &root), "what");
    assert_eq!(message(b"", &root), "");
}

#[test]
fn a_non_config_file_inside_git_is_unavailable() {
    let (_tmp, root, git) = scratch();
    let text = "fatal: unable to access '.git/packed-refs': Permission denied\n";
    assert!(!unreadable(text, &root, &git, &git));
}

#[test]
fn the_directory_in_place_stderr_is_unreadable_and_the_unknown_line_alone_is_not() {
    let (_tmp, root, git) = scratch();
    assert!(unreadable(DIR_IN_PLACE, &root, &git, &git));
    assert!(!unreadable(UNKNOWN, &root, &git, &git));
    assert_eq!(message(UNKNOWN.as_bytes(), &root), UNKNOWN.trim());
    let text = "warning: unable to access '.git/config': Is a directory\nfatal: x\n";
    assert!(unreadable(text, &root, &git, &git));
    assert_eq!(
        message(b"warning: something\nfatal: x\n", &root),
        "fatal: x"
    );
}

#[test]
fn the_unknown_line_needs_a_second_line_with_a_substring_naming_the_config() {
    let (_tmp, root, git) = scratch();
    let with = format!("{UNKNOWN}warning: unable to access '.git/config': x\n");
    assert!(unreadable(&with, &root, &git, &git));
    let without = format!("{UNKNOWN}warning: see .git/config\n");
    assert!(!unreadable(&without, &root, &git, &git));
    let other = format!("{UNKNOWN}warning: unable to access '/etc/gitconfig': x\n");
    assert!(!unreadable(&other, &root, &git, &git));
}

#[test]
fn the_fallback_follows_gitfiles_and_linked_worktrees() {
    let tmp = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(tmp.path()).unwrap();
    // A gitfile with a relative path.
    let root = base.join("sub");
    let real = base.join("store/modules/sub");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&real).unwrap();
    std::fs::write(root.join(".git"), "gitdir: ../store/modules/sub\n").unwrap();
    let (git_dir, common) = repo_dirs_fallback(&root);
    assert_eq!(std::fs::canonicalize(&git_dir).unwrap(), real);
    assert_eq!(common, git_dir);
    assert!(unreadable(
        "fatal: bad config line 1 in file ../store/modules/sub/config\n",
        &root,
        &git_dir,
        &common
    ));
    // A linked worktree: gitdir inside the main .git, with a commondir.
    let main = base.join("main");
    let linked = main.join(".git/worktrees/w");
    let wt = base.join("wt");
    std::fs::create_dir_all(&linked).unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(linked.join("commondir"), "../..\n").unwrap();
    std::fs::write(wt.join(".git"), format!("gitdir: {}\n", linked.display())).unwrap();
    let (git_dir, common) = repo_dirs_fallback(&wt);
    assert_eq!(git_dir, linked);
    assert_eq!(std::fs::canonicalize(&common).unwrap(), main.join(".git"));
    for file in [common.join("config"), git_dir.join("config.worktree")] {
        let text = format!("fatal: bad config line 1 in file {}\n", file.display());
        assert!(unreadable(&text, &wt, &git_dir, &common), "{text}");
    }
}

#[test]
fn a_missing_or_broken_gitfile_fails_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("r");
    std::fs::create_dir_all(&root).unwrap();
    let want = (root.join(".git"), root.join(".git"));
    assert_eq!(repo_dirs_fallback(&root), want);
    std::fs::write(root.join(".git"), "nothing here\n").unwrap();
    assert_eq!(repo_dirs_fallback(&root), want);
    std::fs::write(root.join(".git"), "gitdir: nowhere\n").unwrap();
    assert_eq!(repo_dirs_fallback(&root), want);
    let text = "fatal: bad config line 1 in file .git/config\n";
    assert!(!unreadable(text, &root, &want.0, &want.1));
    assert_eq!(message(text.as_bytes(), &root), text.trim());
}

#[cfg(unix)]
#[test]
fn a_symlinked_root_still_matches() {
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join("real");
    std::fs::create_dir_all(real.join(".git")).unwrap();
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let resolved = std::fs::canonicalize(&real).unwrap();
    let text = format!(
        "fatal: bad config line 1 in file {}\n",
        resolved.join(".git/config").display()
    );
    assert!(unreadable(
        &text,
        &link,
        &link.join(".git"),
        &link.join(".git")
    ));
    let (g, c) = repo_dirs_fallback(&link);
    assert!(unreadable(&text, &link, &g, &c));
}

fn git(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .unwrap()
}

#[test]
fn real_git_stderr_classification() {
    let version = Command::new("git").arg("--version").output().unwrap();
    let version = String::from_utf8_lossy(&version.stdout).to_string();
    if !version.trim().starts_with("git version 2.39.") {
        println!("SKIPPED real-git stderr: {}", version.trim());
        return;
    }
    let root_user = Command::new("id").arg("-u").output().unwrap().stdout == b"0\n";
    let mut cases: Vec<(&str, &str, Vec<&str>)> = vec![
        ("bad", BAD_LINE, vec!["bad config line"]),
        (
            "dir",
            DIR_IN_PLACE,
            vec!["unable to access", "unknown error"],
        ),
    ];
    if !root_user {
        cases.push(("denied", ACCESS_DENIED, vec!["Permission denied"]));
    }
    for (name, fixture, _) in &cases {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        assert!(git(&root, &["init", "-q", "."]).status.success(), "{name}");
        let config = root.join(".git/config");
        match *name {
            "bad" => std::fs::write(&config, "[core\nbroken line\n").unwrap(),
            "dir" => {
                std::fs::remove_file(&config).unwrap();
                std::fs::create_dir(&config).unwrap();
            }
            _ => {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o000)).unwrap();
            }
        }
        let git_dir = root.join(".git");
        for args in [
            &["rev-parse", "--git-dir"][..],
            &["config", "--local", "-z", "--list"][..],
        ] {
            let out = git(&root, args);
            assert!(!out.status.success(), "{name} {args:?} should fail");
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            for line in fixture.lines() {
                let marker = ["bad config line", "unable to access", "Permission denied"]
                    .into_iter()
                    .find(|m| line.contains(m));
                if let Some(marker) = marker {
                    if args[0] == "rev-parse" || *name != "dir" {
                        assert!(stderr.contains(marker), "{name} {args:?}: {stderr}");
                    }
                }
            }
            assert_eq!(
                classify_config_error(&out.stderr, &root, &git_dir, &git_dir),
                ConfigError::Unreadable,
                "{name} {args:?}: {stderr}"
            );
        }
        if *name == "denied" {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
}
