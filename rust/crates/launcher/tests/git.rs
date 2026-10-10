//! `DefaultGit` and the git requirement against real repositories (replaces the git halves of test/launcher-git.test.ts and
//! test/git-requirement.test.ts that need no ledger): SHA-256 repositories, which worktrees are forced and which are not,
//! locked and unknown worktrees, branch renames and kept refs. Nothing here needs Node.
use capstan_launcher::git::{check_git_requirement, DefaultGit, GitCheck, GitProblem};
use capstan_launcher::shared::GitRunner;
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .expect("git runs")
}

fn git_ok(dir: &Path, args: &[&str]) -> String {
    let out = git(dir, args);
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn repository() -> (tempfile::TempDir, PathBuf) {
    let scratch = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(scratch.path()).unwrap();
    git_ok(&root, &["init", "-q", "-b", "main"]);
    git_ok(&root, &["commit", "-q", "--allow-empty", "-m", "i"]);
    (scratch, root)
}

fn tree(root: &Path, branch: &str, name: &str) -> PathBuf {
    let dir = root.join("trees").join(name);
    git_ok(
        root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            branch,
            &dir.to_string_lossy(),
        ],
    );
    dir
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[test]
fn a_sha256_repository_is_named_as_unsupported_instead_of_reported_as_having_no_commit() {
    let scratch = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(scratch.path()).unwrap();
    let init = git(&root, &["init", "-q", "--object-format=sha256", "."]);
    if !init.status.success() {
        eprintln!("this git cannot make a SHA-256 repository; nothing to check");
        return;
    }
    git_ok(&root, &["commit", "-q", "--allow-empty", "-m", "x"]);
    let error = DefaultGit::new(&text(&root)).head_sha().unwrap_err();
    assert_eq!(error.code, "git_error");
    assert!(error.message.contains("SHA-256"), "{}", error.message);
}

#[test]
fn the_git_requirement_names_each_thing_that_is_missing() {
    let empty = tempfile::tempdir().unwrap();
    let root = text(&std::fs::canonicalize(empty.path()).unwrap());
    let GitCheck::Problem { problem, message } = check_git_requirement(&root) else {
        panic!("an empty directory is not a repository");
    };
    assert_eq!(problem, GitProblem::NotARepository);
    assert!(
        message.contains("is not inside a git work tree"),
        "{message}"
    );
    assert!(message.contains("cstan init --git"), "{message}");

    let unborn = tempfile::tempdir().unwrap();
    let unborn_root = std::fs::canonicalize(unborn.path()).unwrap();
    git_ok(&unborn_root, &["init", "-q", "-b", "main"]);
    let GitCheck::Problem { problem, message } = check_git_requirement(&text(&unborn_root)) else {
        panic!("no commit yet");
    };
    assert_eq!(problem, GitProblem::NoCommit);
    assert!(message.contains("has no commit yet"), "{message}");

    let bare = tempfile::tempdir().unwrap();
    let bare_root = std::fs::canonicalize(bare.path()).unwrap();
    git_ok(&bare_root, &["init", "-q", "--bare"]);
    let GitCheck::Problem { problem, .. } = check_git_requirement(&text(&bare_root)) else {
        panic!("a bare repository is refused");
    };
    assert_eq!(problem, GitProblem::BareRepository);

    let (_keep, repo) = repository();
    let inner = repo.join("inner");
    std::fs::create_dir(&inner).unwrap();
    let GitCheck::Problem { problem, .. } = check_git_requirement(&text(&inner)) else {
        panic!("a subdirectory is refused");
    };
    assert_eq!(problem, GitProblem::Subdirectory);
    assert_eq!(
        check_git_requirement(&text(&repo)),
        GitCheck::Ok { detached: false }
    );
    git_ok(&repo, &["checkout", "-q", "--detach"]);
    assert_eq!(
        check_git_requirement(&text(&repo)),
        GitCheck::Ok { detached: true }
    );
}

#[test]
fn a_capstan_worktree_with_untracked_files_is_removed_and_a_locked_or_unknown_one_is_reported() {
    let (_keep, root) = repository();
    let runner = DefaultGit::new(&text(&root));

    let dirty = tree(&root, "capstan/dev-1-g1", "dirty");
    std::fs::write(dirty.join("untracked.txt"), "x").unwrap();
    assert_eq!(runner.worktree_dirty_count(&text(&dirty)), Some(1));
    assert!(runner.worktree_remove(&text(&dirty), None).removed);
    assert!(!dirty.exists());

    let locked = tree(&root, "capstan/dev-2-g1", "locked");
    git_ok(&root, &["worktree", "lock", &text(&locked)]);
    let refused = runner.worktree_remove(&text(&locked), None);
    assert!(!refused.removed);
    assert!(refused.stderr.contains("locked"), "{}", refused.stderr);
    assert_eq!(runner.worktree_dirty_count(&text(&locked)), Some(0));

    let foreign = tree(&root, "feature/x", "foreign");
    std::fs::write(foreign.join("untracked.txt"), "x").unwrap();
    let kept = runner.worktree_remove(&text(&foreign), None);
    assert!(
        !kept.removed,
        "a worktree of another branch is never forced"
    );
    assert!(kept.stderr.contains("untracked"), "{}", kept.stderr);

    let unknown = root.join("trees").join("nowhere");
    let missing = runner.worktree_remove(&text(&unknown), None);
    assert!(!missing.removed);
    assert!(!missing.stderr.is_empty());
    assert_eq!(runner.worktree_dirty_count(&text(&unknown)), None);
}

#[test]
fn only_the_recorded_branch_and_a_legacy_capstan_branch_are_forced() {
    let (_keep, root) = repository();
    let runner = DefaultGit::new(&text(&root));
    let make = |branch: &str, name: &str| {
        let dir = tree(&root, branch, name);
        std::fs::write(dir.join("node_modules"), "x").unwrap();
        dir
    };
    let recorded = make("feat/NX-1-parser", "recorded");
    assert!(
        runner
            .worktree_remove(&text(&recorded), Some("feat/NX-1-parser"))
            .removed,
        "the branch recorded for the agent is forced"
    );
    assert!(!recorded.exists());

    let unrecorded = make("feat/NX-2-other", "unrecorded");
    assert!(
        !runner
            .worktree_remove(&text(&unrecorded), Some("feat/NX-9-else"))
            .removed
    );
    assert!(!runner.worktree_remove(&text(&unrecorded), None).removed);

    let legacy = make("capstan/dev-1-g1", "legacy");
    assert!(runner.worktree_remove(&text(&legacy), None).removed);

    let renamed = runner.rename_branch("feat/NX-2-other", "feat/NX-2-renamed");
    assert!(renamed.removed, "{renamed:?}");
    assert!(
        runner
            .worktree_remove(&text(&unrecorded), Some("feat/NX-2-renamed"))
            .removed
    );
    assert_eq!(runner.branch_tip("feat/NX-2-other"), None);
    let head = runner.head_sha().unwrap();
    assert!(runner.save_ref("refs/capstan/kept/x", &head));
    assert_eq!(git_ok(&root, &["rev-parse", "refs/capstan/kept/x"]), head);
}

#[test]
fn a_branch_is_deleted_only_while_it_is_at_the_recorded_commit() {
    let (_keep, root) = repository();
    let runner = DefaultGit::new(&text(&root));
    let head = runner.head_sha().unwrap();
    git_ok(&root, &["branch", "feat/a-1"]);
    git_ok(&root, &["checkout", "-q", "feat/a-1"]);
    git_ok(&root, &["commit", "-q", "--allow-empty", "-m", "more"]);
    git_ok(&root, &["checkout", "-q", "main"]);
    assert!(
        !runner.delete_branch_if("feat/a-1", &head),
        "a branch that moved on is kept"
    );
    let tip = runner.branch_tip("feat/a-1").unwrap();
    assert_ne!(tip, head);
    assert!(runner.delete_branch_if("feat/a-1", &tip));
    assert_eq!(runner.branch_tip("feat/a-1"), None);
}

#[test]
fn a_failing_git_never_reads_as_no_worktree() {
    let scratch = tempfile::tempdir().unwrap();
    let not_a_repository = text(&std::fs::canonicalize(scratch.path()).unwrap());
    let error = DefaultGit::new(&not_a_repository)
        .worktree_by_branch("feat/x-1")
        .unwrap_err();
    assert_eq!(error.code, "git_error");
    assert_eq!(error.message, "git could not list the worktrees");
}
