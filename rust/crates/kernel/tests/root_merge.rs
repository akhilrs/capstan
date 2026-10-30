//! The merge commit and checkout update of a merge into the root checkout: real git in temporary repositories; every
//! timing case is driven by a seam, never by a sleep.

use capstan_kernel::integrate::{
    GitRepo, GitSeams, MergeSeams, Preflight, RootMerge, ENTRY_WOULD_OVERWRITE,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(60);

struct Fx {
    tmp: tempfile::TempDir,
    root: PathBuf,
}

fn forward(tmp: &Path) -> Vec<(String, String)> {
    let home = tmp.join("home");
    std::fs::create_dir_all(&home).unwrap();
    vec![
        ("HOME".into(), home.to_string_lossy().into_owned()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
    ]
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn git(dir: &Path, args: &[&str]) {
    git_out(dir, args);
}

fn write(dir: &Path, name: &str, text: &str) {
    if let Some(parent) = dir.join(name).parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(dir.join(name), text).unwrap();
}

fn commit_all(dir: &Path, message: &str) {
    git(dir, &["add", "-A"]);
    git(
        dir,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            message,
        ],
    );
}

/// HEAD, the tree id of the index, and the bytes of every file outside `.git`.
fn snapshot(root: &Path) -> (String, String, BTreeMap<String, Vec<u8>>) {
    fn walk(dir: &Path, base: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if entry.file_name() == ".git" {
                continue;
            }
            if path.is_dir() {
                walk(&path, base, out);
            } else {
                let name = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                out.insert(name, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(root, root, &mut files);
    (
        git_out(root, &["rev-parse", "HEAD"]),
        git_out(root, &["write-tree"]),
        files,
    )
}

impl Fx {
    /// A repository on `main` with one commit; a local identity unless `identity` is false.
    fn new(identity: bool) -> Fx {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        if identity {
            git(&root, &["config", "--local", "user.name", "T"]);
            git(&root, &["config", "--local", "user.email", "t@t"]);
        }
        write(&root, "a.txt", "base\n");
        write(&root, "b.txt", "keep\n");
        commit_all(&root, "base");
        Fx { tmp, root }
    }

    fn repo(&self) -> GitRepo {
        let seams = GitSeams {
            forward_env: Some(forward(self.tmp.path())),
            ..GitSeams::default()
        };
        GitRepo::new(&self.root).with_seams(seams)
    }

    fn seamed(&self, seams: MergeSeams) -> GitRepo {
        self.repo().with_merge_seams(seams)
    }

    fn sha(&self, rev: &str) -> String {
        git_out(&self.root, &["rev-parse", rev])
    }

    /// A branch `name` off HEAD with one more file; the starting branch is checked out again.
    fn side_branch(&self, name: &str, file: &str, text: &str) -> String {
        let start = git_out(&self.root, &["symbolic-ref", "--short", "HEAD"]);
        git(&self.root, &["checkout", "-q", "-b", name]);
        write(&self.root, file, text);
        commit_all(&self.root, name);
        let sha = self.sha("HEAD");
        git(&self.root, &["checkout", "-q", &start]);
        sha
    }

    /// A side branch adding `s.txt`, then one more commit on main adding `m.txt`: (side sha, old head).
    fn diverged(&self) -> (String, String) {
        let side = self.side_branch("side", "s.txt", "side\n");
        write(&self.root, "m.txt", "main\n");
        commit_all(&self.root, "main work");
        (side, self.sha("HEAD"))
    }

    fn merge(&self, repo: &GitRepo, side: &str, old: &str) -> RootMerge {
        repo.merge_into_root("main", "side", side, old, LONG)
            .unwrap()
    }
}

fn refusal(result: RootMerge) -> (String, Vec<String>) {
    match result {
        RootMerge::Refused(r) => (r.code().to_string(), r.files().to_vec()),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn failed(result: RootMerge) -> (&'static str, bool) {
    match result {
        RootMerge::Failed { reason, restored } => (reason, restored),
        other => panic!("expected a failure, got {other:?}"),
    }
}

fn merged(result: RootMerge) -> String {
    match result {
        RootMerge::Merged { commit } => commit,
        other => panic!("expected a merge, got {other:?}"),
    }
}

// ------------------------------------------------------------------------------------------------ the merge itself

#[test]
fn a_clean_merge_gives_the_commit_the_tree_and_the_identity() {
    let fx = Fx::new(true);
    let (side, old) = fx.diverged();
    let result = fx.merge(&fx.repo(), &side, &old);
    let commit = merged(result);

    assert_eq!(fx.sha("main"), commit);
    assert_eq!(
        git_out(&fx.root, &["rev-list", "--parents", "-n", "1", &commit]),
        format!("{commit} {old} {side}")
    );
    assert_eq!(
        git_out(&fx.root, &["log", "-1", "--format=%s", &commit]),
        "Merge branch 'side'"
    );
    assert_eq!(
        git_out(
            &fx.root,
            &["log", "-1", "--format=%an <%ae>|%cn <%ce>", &commit]
        ),
        "T <t@t>|T <t@t>"
    );
    // The working tree and the index equal the merged tree.
    let tree = git_out(&fx.root, &["rev-parse", &format!("{commit}^{{tree}}")]);
    assert_eq!(git_out(&fx.root, &["write-tree"]), tree);
    assert_eq!(git_out(&fx.root, &["status", "--porcelain"]), "");
    assert!(fx.root.join("s.txt").exists() && fx.root.join("m.txt").exists());

    // The same tree as `git merge --no-ff` by hand on a clone.
    let clone = fx.tmp.path().join("clone");
    git(
        fx.tmp.path(),
        &[
            "clone",
            "-q",
            fx.root.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    git(&clone, &["reset", "-q", "--hard", &old]);
    // Run with no user or system git config, so nothing but the arguments shapes the merge.
    let by_hand = Command::new("git")
        .arg("-C")
        .arg(&clone)
        .args([
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@t",
            "merge",
            "--no-ff",
            "-q",
            "-m",
            "x",
            &side,
        ])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        by_hand.status.success(),
        "git merge --no-ff: {}",
        String::from_utf8_lossy(&by_hand.stderr)
    );
    assert_eq!(
        git_out(&clone, &["rev-parse", "HEAD^{tree}"]),
        git_out(&fx.root, &["rev-parse", &format!("{commit}^{{tree}}")])
    );
}

#[test]
fn the_message_names_the_merged_branch_on_any_checkout_branch() {
    let fx = Fx::new(true);
    git(&fx.root, &["checkout", "-q", "-b", "dev"]);
    let side = fx.side_branch("feature/x", "s.txt", "side\n");
    write(&fx.root, "m.txt", "dev\n");
    commit_all(&fx.root, "dev work");
    let old = fx.sha("HEAD");
    let commit = merged(
        fx.repo()
            .merge_into_root("dev", "feature/x", &side, &old, LONG)
            .unwrap(),
    );
    assert_eq!(
        git_out(&fx.root, &["log", "-1", "--format=%B", &commit]),
        "Merge branch 'feature/x'"
    );
    assert_eq!(fx.sha("dev"), commit);
}

#[test]
fn a_touched_merged_file_with_the_same_content_still_merges() {
    let fx = Fx::new(true);
    let side = fx.side_branch("side", "a.txt", "side change\n");
    write(&fx.root, "m.txt", "main\n");
    commit_all(&fx.root, "main work");
    let old = fx.sha("HEAD");
    let pre = fx.repo().root_merge_preflight("main", &side, LONG).unwrap();
    assert!(matches!(pre, Preflight::Ready { .. }));
    let touched = Command::new("touch")
        .arg(fx.root.join("a.txt"))
        .status()
        .unwrap();
    assert!(touched.success());
    merged(fx.merge(&fx.repo(), &side, &old));
    assert_eq!(
        std::fs::read_to_string(fx.root.join("a.txt")).unwrap(),
        "side change\n"
    );
}

#[test]
fn the_identity_may_live_in_the_global_config_only() {
    let fx = Fx::new(false);
    forward(fx.tmp.path());
    std::fs::write(
        fx.tmp.path().join("home/.gitconfig"),
        "[user]\n\tname = Global Person\n\temail = g@example.com\n",
    )
    .unwrap();
    let (side, old) = fx.diverged();
    let commit = merged(fx.merge(&fx.repo(), &side, &old));
    assert_eq!(
        git_out(&fx.root, &["log", "-1", "--format=%an <%ae>", &commit]),
        "Global Person <g@example.com>"
    );
    let reflog = git_out(&fx.root, &["reflog", "-1", "--format=%gn|%ge|%gs", "main"]);
    assert_eq!(reflog, "Global Person|g@example.com|merge side (remote)");
}

#[test]
fn a_local_commit_encoding_leaves_no_encoding_header() {
    let fx = Fx::new(true);
    git(
        &fx.root,
        &["config", "--local", "i18n.commitEncoding", "ISO-8859-1"],
    );
    let (side, old) = fx.diverged();
    let commit = merged(fx.merge(&fx.repo(), &side, &old));
    let raw = git_out(&fx.root, &["cat-file", "-p", &commit]);
    assert!(!raw.contains("encoding"), "{raw}");
}

#[test]
fn no_hook_runs() {
    let fx = Fx::new(true);
    let (side, old) = fx.diverged();
    let marks = fx.tmp.path().join("marks");
    std::fs::create_dir_all(&marks).unwrap();
    let script = |name: &str| {
        format!(
            "#!/bin/sh\necho ran >> '{}'\n",
            marks.join(name).to_string_lossy()
        )
    };
    let global_hooks = fx.tmp.path().join("global-hooks");
    std::fs::create_dir_all(&global_hooks).unwrap();
    let local_hooks = fx.root.join(".git/hooks");
    std::fs::create_dir_all(&local_hooks).unwrap();
    for (dir, name, mark) in [
        (&global_hooks, "reference-transaction", "global"),
        (&global_hooks, "post-merge", "global-post"),
        (&local_hooks, "reference-transaction", "local"),
        (&local_hooks, "post-merge", "local-post"),
    ] {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, script(mark)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    forward(fx.tmp.path());
    std::fs::write(
        fx.tmp.path().join("home/.gitconfig"),
        format!("[core]\n\thooksPath = {}\n", global_hooks.to_string_lossy()),
    )
    .unwrap();
    merged(fx.merge(&fx.repo(), &side, &old));
    assert_eq!(std::fs::read_dir(&marks).unwrap().count(), 0);
}

// ------------------------------------------------------------------------------------------------------- refusals

#[test]
fn an_untracked_file_at_an_added_path_is_refused_and_nothing_changes() {
    let fx = Fx::new(true);
    let (side, old) = fx.diverged();
    write(&fx.root, "s.txt", "mine\n");
    let before = snapshot(&fx.root);
    let (code, files) = refusal(fx.merge(&fx.repo(), &side, &old));
    assert_eq!(code, "dirty_tree");
    assert!(
        files.contains(&ENTRY_WOULD_OVERWRITE.to_string()),
        "{files:?}"
    );
    assert_eq!(snapshot(&fx.root), before);
}

#[test]
fn an_ignored_file_at_an_added_path_is_refused_by_the_rerun() {
    let fx = Fx::new(true);
    write(&fx.root, ".gitignore", "*.log\n");
    commit_all(&fx.root, "ignore");
    git(&fx.root, &["checkout", "-q", "-b", "side"]);
    write(&fx.root, "x.log", "side\n");
    git(&fx.root, &["add", "-f", "x.log"]);
    commit_all(&fx.root, "side");
    let side = fx.sha("HEAD");
    git(&fx.root, &["checkout", "-q", "main"]);
    write(&fx.root, "m.txt", "main\n");
    commit_all(&fx.root, "main work");
    let old = fx.sha("HEAD");
    write(&fx.root, "x.log", "mine\n");
    let before = snapshot(&fx.root);
    let (code, files) = refusal(fx.merge(&fx.repo(), &side, &old));
    assert_eq!(code, "dirty_tree");
    assert!(files.contains(&"x.log".to_string()), "{files:?}");
    assert_eq!(snapshot(&fx.root), before);
}

#[test]
fn an_edit_of_a_merged_path_after_the_preflight_is_refused() {
    let fx = Fx::new(true);
    let side = fx.side_branch("side", "a.txt", "side change\n");
    write(&fx.root, "m.txt", "main\n");
    commit_all(&fx.root, "main work");
    let old = fx.sha("HEAD");
    let before = snapshot(&fx.root);
    let repo = fx.seamed(MergeSeams {
        edit_before_read_tree: Some((PathBuf::from("a.txt"), b"local edit\n".to_vec())),
        ..MergeSeams::default()
    });
    let (code, files) = refusal(fx.merge(&repo, &side, &old));
    assert_eq!(code, "dirty_tree");
    assert!(
        files.contains(&ENTRY_WOULD_OVERWRITE.to_string()),
        "{files:?}"
    );
    let after = snapshot(&fx.root);
    assert_eq!(after.0, before.0);
    assert_eq!(after.2.get("a.txt").unwrap(), b"local edit\n");
}

#[test]
fn an_edit_of_an_untouched_path_after_the_preflight_is_kept() {
    let fx = Fx::new(true);
    let (side, old) = fx.diverged();
    let repo = fx.seamed(MergeSeams {
        edit_before_read_tree: Some((PathBuf::from("b.txt"), b"local edit\n".to_vec())),
        ..MergeSeams::default()
    });
    merged(fx.merge(&repo, &side, &old));
    assert_eq!(
        std::fs::read_to_string(fx.root.join("b.txt")).unwrap(),
        "local edit\n"
    );
}

#[test]
fn a_head_sha_merged_by_hand_after_the_preflight_is_already_in_head() {
    let fx = Fx::new(true);
    let (side, old) = fx.diverged();
    let pre = fx.repo().root_merge_preflight("main", &side, LONG).unwrap();
    assert!(matches!(pre, Preflight::Ready { .. }));
    git(
        &fx.root,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "merge",
            "-q",
            "--no-edit",
            &side,
        ],
    );
    let head = fx.sha("HEAD");
    assert_eq!(fx.merge(&fx.repo(), &side, &old), RootMerge::AlreadyInHead);
    assert_eq!(fx.sha("HEAD"), head);
}

#[test]
fn a_conflicting_commit_after_the_preflight_is_a_conflict() {
    let fx = Fx::new(true);
    let side = fx.side_branch("side", "a.txt", "side change\n");
    write(&fx.root, "m.txt", "main\n");
    commit_all(&fx.root, "main work");
    let old = fx.sha("HEAD");
    let pre = fx.repo().root_merge_preflight("main", &side, LONG).unwrap();
    assert!(matches!(pre, Preflight::Ready { .. }));
    write(&fx.root, "a.txt", "conflicting\n");
    commit_all(&fx.root, "conflict");
    let before = snapshot(&fx.root);
    let (code, _) = refusal(fx.merge(&fx.repo(), &side, &old));
    assert_eq!(code, "merge_conflict");
    assert_eq!(snapshot(&fx.root), before);
}

#[test]
fn a_newline_in_the_merged_branch_is_invalid_and_an_invalid_old_head_comes_first() {
    let fx = Fx::new(true);
    let (side, old) = fx.diverged();
    let (code, _) = refusal(
        fx.repo()
            .merge_into_root("main", "bad\nname", &side, &old, LONG)
            .unwrap(),
    );
    assert_eq!(code, "invalid_branch");
    let (code, _) = refusal(
        fx.repo()
            .merge_into_root("main", "bad\nname", &side, "not-a-commit", LONG)
            .unwrap(),
    );
    assert_eq!(code, "invalid_commit");
}

#[test]
fn a_detached_head_or_another_branch_at_the_same_sha_is_the_wrong_branch() {
    let fx = Fx::new(true);
    let (side, old) = fx.diverged();
    let before = snapshot(&fx.root);
    git(&fx.root, &["checkout", "-q", "--detach", &old]);
    let (code, _) = refusal(fx.merge(&fx.repo(), &side, &old));
    assert_eq!(code, "wrong_branch");
    git(&fx.root, &["checkout", "-q", "-b", "other"]);
    let (code, files) = refusal(fx.merge(&fx.repo(), &side, &old));
    assert_eq!(code, "wrong_branch");
    assert_eq!(files, vec!["other".to_string()]);
    assert_eq!(fx.sha("main"), old);
    assert_eq!(snapshot(&fx.root).1, before.1);
}

// ----------------------------------------------------------------------------------------------- failures by seam

#[test]
fn a_lock_held_before_the_refresh_is_restored() {
    let fx = Fx::new(true);
    let (side, old) = fx.diverged();
    let repo = fx.seamed(MergeSeams {
        hold_lock_before_refresh: true,
        ..MergeSeams::default()
    });
    assert_eq!(failed(fx.merge(&repo, &side, &old)), ("merge_failed", true));
    assert_eq!(fx.sha("main"), old);
}

#[test]
fn a_lock_left_by_a_killed_refresh_is_never_deleted() {
    let fx = Fx::new(true);
    let (side, old) = fx.diverged();
    let repo = fx.seamed(MergeSeams {
        lock_after_refresh_killed: true,
        ..MergeSeams::default()
    });
    assert_eq!(
        failed(fx.merge(&repo, &side, &old)),
        ("index_lock_left", false)
    );
    assert!(fx.root.join(".git/index.lock").exists());
}

#[test]
fn a_lock_held_before_read_tree_fails_before_writing_and_is_restored() {
    let fx = Fx::new(true);
    let (side, old) = fx.diverged();
    let repo = fx.seamed(MergeSeams {
        hold_lock_before_read_tree: true,
        ..MergeSeams::default()
    });
    assert_eq!(failed(fx.merge(&repo, &side, &old)), ("merge_failed", true));
    assert_eq!(fx.sha("main"), old);
    assert!(!fx.root.join("s.txt").exists());
}

#[test]
fn a_lock_left_by_a_killed_read_tree_is_index_lock_left() {
    let fx = Fx::new(true);
    let (side, old) = fx.diverged();
    let repo = fx.seamed(MergeSeams {
        lock_after_read_tree_killed: true,
        ..MergeSeams::default()
    });
    assert_eq!(
        failed(fx.merge(&repo, &side, &old)),
        ("index_lock_left", false)
    );
    assert!(fx.root.join(".git/index.lock").exists());
}

#[test]
fn an_unknown_read_tree_failure_after_writing_is_rolled_back() {
    let fx = Fx::new(true);
    let (side, old) = fx.diverged();
    let before = snapshot(&fx.root);
    let repo = fx.seamed(MergeSeams {
        fail_read_tree_after_write: true,
        ..MergeSeams::default()
    });
    assert_eq!(failed(fx.merge(&repo, &side, &old)), ("merge_failed", true));
    assert_eq!(snapshot(&fx.root), before);
}

#[test]
fn an_expired_rollback_deadline_is_not_restored() {
    let fx = Fx::new(true);
    let (side, old) = fx.diverged();
    let repo = fx.seamed(MergeSeams {
        fail_read_tree_after_write: true,
        expire_rollback_deadline: true,
        ..MergeSeams::default()
    });
    assert_eq!(
        failed(fx.merge(&repo, &side, &old)),
        ("merge_failed", false)
    );
}

#[test]
fn a_file_created_at_a_merged_path_stops_the_rollback_and_is_kept() {
    let fx = Fx::new(true);
    let (side, old) = fx.diverged();
    let repo = fx.seamed(MergeSeams {
        fail_read_tree_after_write: true,
        create_file_after_read_tree: Some((PathBuf::from("new-in-window.txt"), b"mine\n".to_vec())),
        ..MergeSeams::default()
    });
    // The new file is not at a merged path, so the rollback runs; one at a merged path is created by the edit seam.
    assert_eq!(failed(fx.merge(&repo, &side, &old)), ("merge_failed", true));
    assert!(fx.root.join("new-in-window.txt").exists());

    let fx = Fx::new(true);
    let (side, old) = fx.diverged();
    let repo = fx.seamed(MergeSeams {
        fail_read_tree_after_write: true,
        edit_before_rollback: Some((PathBuf::from("s.txt"), b"mine\n".to_vec())),
        ..MergeSeams::default()
    });
    assert_eq!(
        failed(fx.merge(&repo, &side, &old)),
        ("merge_failed", false)
    );
    assert_eq!(
        std::fs::read_to_string(fx.root.join("s.txt")).unwrap(),
        "mine\n"
    );
}

#[test]
fn a_branch_moved_before_read_tree_is_not_rolled_back() {
    let fx = Fx::new(true);
    let (side, old) = fx.diverged();
    write(&fx.root, "n.txt", "n\n");
    commit_all(&fx.root, "moved");
    let moved = fx.sha("HEAD");
    git(&fx.root, &["reset", "-q", "--hard", &old]);
    let repo = fx.seamed(MergeSeams {
        move_branch_before_read_tree: Some(moved.clone()),
        ..MergeSeams::default()
    });
    let (_, restored) = failed_or_refused(fx.merge(&repo, &side, &old));
    assert!(!restored);
    assert_eq!(fx.sha("main"), moved);
}

/// A failure's flag, or false for a refusal (read-tree refusing the moved index is also fine).
fn failed_or_refused(result: RootMerge) -> (&'static str, bool) {
    match result {
        RootMerge::Failed { reason, restored } => (reason, restored),
        RootMerge::Refused(_) => ("refused", false),
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[test]
fn a_branch_moved_before_update_ref_is_not_restored() {
    let fx = Fx::new(true);
    let (side, old) = fx.diverged();
    let repo = fx.seamed(MergeSeams {
        move_branch_before_update_ref: true,
        ..MergeSeams::default()
    });
    assert_eq!(
        failed(fx.merge(&repo, &side, &old)),
        ("merge_failed", false)
    );
    assert_eq!(fx.sha("main"), side);
}

#[test]
fn a_held_ref_lock_rolls_back() {
    let fx = Fx::new(true);
    let (side, old) = fx.diverged();
    let before = snapshot(&fx.root);
    let repo = fx.seamed(MergeSeams {
        hold_ref_lock_before_update_ref: true,
        ..MergeSeams::default()
    });
    assert_eq!(failed(fx.merge(&repo, &side, &old)), ("merge_failed", true));
    let after = snapshot(&fx.root);
    assert_eq!(after.0, before.0);
    assert_eq!(after.1, before.1);
    assert_eq!(after.2, before.2);
}

#[test]
fn a_timeout_at_update_ref_rolls_back_with_the_timeout_reason() {
    for seams in [
        MergeSeams {
            force_update_ref_timeout: true,
            ..MergeSeams::default()
        },
        MergeSeams {
            expire_deadline_before_update_ref: true,
            ..MergeSeams::default()
        },
    ] {
        let fx = Fx::new(true);
        let (side, old) = fx.diverged();
        let repo = fx.seamed(seams);
        assert_eq!(failed(fx.merge(&repo, &side, &old)), ("timeout", true));
        assert_eq!(fx.sha("main"), old);
        assert!(!fx.root.join("s.txt").exists());
    }
}

// ------------------------------------------------------------------------------------------------ the seam source scan

/// The one file outside the tests directories that may call the seam setter: the decide-merge route's repository builder.
const SEAM_CALLER: &str = "daemon/src/handlers/decide_merge_seams.rs";

/// The 1-based lines of `text` that call the seam setter: the definition and mentions without a call do not count.
fn seam_call_lines(text: &str) -> Vec<usize> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| line.contains(".with_merge_seams("))
        .map(|(index, _)| index + 1)
        .collect()
}

#[test]
fn the_scan_matcher_finds_calls_and_not_the_definition() {
    assert_eq!(
        seam_call_lines("let r = repo.with_merge_seams(seams);"),
        vec![1]
    );
    assert_eq!(seam_call_lines("x\n    .with_merge_seams(s)\n"), vec![2]);
    assert!(
        seam_call_lines("pub fn with_merge_seams(mut self, s: MergeSeams) -> Self {").is_empty()
    );
    assert!(seam_call_lines("/// see with_merge_seams for tests").is_empty());
}

/// The hits of `seam_call_lines` in `file`, none in the one allowed file.
fn file_hits(file: &Path, text: &str) -> Vec<usize> {
    if file.ends_with(SEAM_CALLER) {
        return Vec::new();
    }
    seam_call_lines(text)
}

#[test]
fn the_allowed_file_may_call_the_setter_and_no_other_may() {
    let call = "repo = repo.with_merge_seams(m.clone());";
    let allowed = Path::new("/x/rust/crates/daemon/src/handlers/decide_merge_seams.rs");
    let other = Path::new("/x/rust/crates/daemon/src/handlers/decide_merge.rs");
    assert!(file_hits(allowed, call).is_empty());
    assert_eq!(file_hits(other, call), vec![1]);
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "tests" && name != "target" && !name.starts_with('.') {
                rust_files(&path, out);
            }
        } else if name.ends_with(".rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_source_outside_tests_calls_the_merge_seam_setter() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    rust_files(&crates, &mut files);
    assert!(!files.is_empty());
    let mut hits = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        for line in file_hits(&file, &text) {
            hits.push(format!("{}:{line}", file.display()));
        }
    }
    assert!(
        hits.is_empty(),
        "the merge seams are set outside tests: {hits:?}"
    );
}
