//! The decide-merge route, one case for each refusal the project's git state or configuration gives, and the documents
//! that name the verb: the merge commit has the right parents and runs no hook of the project; every refusal leaves
//! HEAD and the tree as they were.

mod common;

use capstan_daemon::handlers::decide_merge_seams::{install_seams, DecideMergeSeams};
use capstan_kernel::areas::remote_decisions::MERGE_ERROR_CODES;
use capstan_kernel::integrate::{GitSeams, MergeSeams};
use common::merge::MergeProject;
use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

/// The most a case may take.
const BOUND: Duration = Duration::from_secs(60);

fn git(project: &MergeProject, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(project.dir.path())
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("LC_ALL", "C")
        .env("HOME", "/nonexistent")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Merge Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Merge Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn git_dir(project: &MergeProject) -> PathBuf {
    project.dir.path().join(".git")
}

/// A merge project whose remote names `main` as its default branch, so the merge has a target.
fn new_project() -> MergeProject {
    let project = MergeProject::new();
    git(
        &project,
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    );
    git(
        &project,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    project
}

/// The answer of the merge, which must come within `BOUND`.
fn merge(project: &MergeProject, nonce: &str) -> Value {
    let started = Instant::now();
    let answer = project.decide_merge(nonce);
    assert!(
        started.elapsed() < BOUND,
        "the merge took {:?}",
        started.elapsed()
    );
    answer
}

fn code(answer: &Value) -> String {
    assert_eq!(answer["ok"], false, "{answer}");
    answer["code"].as_str().unwrap().to_string()
}

fn message(answer: &Value) -> String {
    answer["message"].as_str().unwrap_or_default().to_string()
}

fn status(project: &MergeProject) -> String {
    git(project, &["status", "--porcelain", "--untracked-files=all"])
}

fn script(path: &PathBuf, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn a_hook_and_an_fsmonitor_program_of_the_project_do_not_run() {
    let project = new_project();
    let git_dir = git_dir(&project);
    let (hook_ran, fsmonitor_ran) = (git_dir.join("hook-ran"), git_dir.join("fsmonitor-ran"));
    let touch = |file: &PathBuf| format!("#!/bin/sh\ntouch '{}'\n", file.display());
    for hook in [
        "pre-merge-commit",
        "post-merge",
        "pre-commit",
        "commit-msg",
        "post-commit",
        "reference-transaction",
    ] {
        script(&git_dir.join("hooks").join(hook), &touch(&hook_ran));
    }
    let monitor = git_dir.join("monitor.sh");
    // The comparison of commits in the cleanup after the merge (`diff-tree`) is not part of the merge and does not pin
    // the program; any other call of git that runs it is a failure.
    script(
        &monitor,
        &format!(
            "#!/bin/sh\ncase \"$(tr '\\0' ' ' < /proc/$PPID/cmdline)\" in *' diff-tree '*) exit 0;; esac\ntouch '{}'\n",
            fsmonitor_ran.display()
        ),
    );
    git(
        &project,
        &["config", "core.fsmonitor", monitor.to_str().unwrap()],
    );
    let before = project.rev("HEAD");
    let head = project.integration()["headSha"]
        .as_str()
        .unwrap()
        .to_string();
    let answer = merge(&project, "nonce-hooks");
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["result"]["outcome"], "applied", "{answer}");
    let parents = git(&project, &["rev-list", "--parents", "-n1", "HEAD"]);
    let parents: Vec<&str> = parents.split(' ').skip(1).collect();
    assert_eq!(parents, [before.as_str(), head.as_str()]);
    project.wait_handoff();
    assert!(!hook_ran.exists(), "a hook of the project ran");
    assert!(!fsmonitor_ran.exists(), "the fsmonitor program ran");
}

#[test]
fn a_merge_in_progress_is_a_dirty_tree() {
    let project = new_project();
    let head = project.rev("HEAD");
    std::fs::write(git_dir(&project).join("MERGE_HEAD"), format!("{head}\n")).unwrap();
    let answer = merge(&project, "nonce-merge-head");
    assert_eq!(code(&answer), "dirty_tree", "{answer}");
    assert_eq!(project.rev("HEAD"), head);
}

#[test]
fn a_detached_head_is_the_wrong_branch() {
    let project = new_project();
    git(&project, &["checkout", "--detach", "--quiet"]);
    let head = project.rev("HEAD");
    let answer = merge(&project, "nonce-detached");
    assert_eq!(code(&answer), "wrong_branch", "{answer}");
    assert_eq!(project.rev("HEAD"), head);
}

#[test]
fn an_untracked_file_the_merge_would_create_is_a_dirty_tree_before_any_write() {
    let project = new_project();
    let file = project.dir.path().join("one.txt");
    std::fs::write(&file, "mine\n").unwrap();
    let (head, before) = (project.rev("HEAD"), status(&project));
    let answer = merge(&project, "nonce-untracked");
    assert_eq!(code(&answer), "dirty_tree", "{answer}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "mine\n");
    assert_eq!(project.rev("HEAD"), head);
    assert_eq!(status(&project), before);
    assert!(!git_dir(&project).join("capstan-remote-merge").exists());
}

#[test]
fn an_old_git_is_git_too_old() {
    let project = new_project();
    install_seams(
        &project.deps,
        DecideMergeSeams {
            git_seams: Some(GitSeams {
                force_old_git: true,
                ..GitSeams::default()
            }),
            ..DecideMergeSeams::default()
        },
    );
    let head = project.rev("HEAD");
    let answer = merge(&project, "nonce-old-git");
    // The version check of the first call of git answers `git_unavailable`; `git_too_old` is the preflight's own refusal.
    assert!(
        ["git_too_old", "git_unavailable"].contains(&code(&answer).as_str()),
        "{answer}"
    );
    assert_eq!(project.rev("HEAD"), head);
}

#[test]
fn a_failed_read_tree_is_restored() {
    let project = new_project();
    install_seams(
        &project.deps,
        DecideMergeSeams {
            merge_seams: Some(MergeSeams {
                fail_read_tree_after_write: true,
                ..MergeSeams::default()
            }),
            ..DecideMergeSeams::default()
        },
    );
    let (head, before) = (project.rev("HEAD"), status(&project));
    let answer = merge(&project, "nonce-read-tree");
    assert_eq!(code(&answer), "merge_failed", "{answer}");
    assert!(message(&answer).contains("restored true"), "{answer}");
    assert_eq!(project.rev("HEAD"), head);
    assert_eq!(status(&project), before);
}

#[test]
fn a_local_lfs_filter_is_unsafe_config() {
    let project = new_project();
    git(
        &project,
        &["config", "filter.lfs.smudge", "git-lfs smudge -- %f"],
    );
    let head = project.rev("HEAD");
    let answer = merge(&project, "nonce-filter");
    assert_eq!(code(&answer), "unsafe_config", "{answer}");
    assert_eq!(project.rev("HEAD"), head);
}

// ------------------------------------------------------------------------------------------------ the documents

fn document(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../docs/reference")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The text from the line `start` to the next line of the same or a higher level (or the end).
fn section(text: &str, start: &str) -> String {
    let mut lines = text.lines().skip_while(|l| l.trim() != start);
    let first = lines.next().unwrap_or_else(|| panic!("no {start:?}"));
    let level = start.chars().take_while(|c| *c == '#').count();
    let mut found = vec![first];
    for line in lines {
        let hashes = line.chars().take_while(|c| *c == '#').count();
        if hashes > 0 && hashes <= level && line.chars().nth(hashes) == Some(' ') {
            break;
        }
        found.push(line);
    }
    found.join("\n")
}

#[test]
fn the_remote_merge_section_of_the_operator_guide_names_every_code() {
    let remote = section(&document("operator.md"), "## Remote merge");
    for code in MERGE_ERROR_CODES {
        assert!(
            remote.contains(code),
            "the Remote merge section lacks {code}"
        );
    }
    for word in [
        "cstan integrate confirm",
        "capstan-remote-merge",
        "git revert",
        "cstan cleanup",
        "merge = false",
        "capstan.toml",
    ] {
        assert!(
            remote.contains(word),
            "the Remote merge section lacks {word}"
        );
    }
    assert!(
        remote.contains("repeats") && remote.contains("nonce"),
        "a stored nonce still repeats its answer"
    );
}

#[test]
fn the_decide_row_of_the_command_reference_names_the_merge() {
    let commands = document("commands.md");
    let row = commands
        .lines()
        .find(|l| l.starts_with("| `cstan decide "))
        .expect("the cstan decide row");
    assert!(row.contains("integration:<id>"), "{row}");
    assert!(row.contains("300"), "{row}");
}
