//! The merge commit of a merge into the root checkout under the seams of `MergeSeams`: branch moves, locks, deadlines
//! and the rollback, in real git repositories. Every timing case is driven by a seam, never by a sleep.

use capstan_kernel::integrate::{GitRepo, GitSeams, MergeSeams, RootMerge};
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
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

fn git_cmd(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null");
    cmd
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = git_cmd(dir, args).output().unwrap();
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

/// Whether git exits 0 for `args`.
fn git_ok(dir: &Path, args: &[&str]) -> bool {
    git_cmd(dir, args).output().unwrap().status.success()
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

/// "tree equal to `old`'s": the index equals the tree of `old` and no tracked file differs from it.
fn tree_equals(root: &Path, old: &str) -> bool {
    git_ok(root, &["diff-index", "--cached", "--quiet", old])
        && git_out(root, &["status", "--porcelain", "--untracked-files=no"]).is_empty()
}

/// "the index is the merge over the moved HEAD": the index equals `git merge-tree --write-tree <moved> <side>`.
fn index_is_merge_over(root: &Path, moved: &str, side: &str) -> bool {
    let tree = git_out(root, &["merge-tree", "--write-tree", moved, side]);
    git_ok(root, &["diff-index", "--cached", "--quiet", &tree])
}

impl Fx {
    /// A repository on `main` with files `a.txt`, `b.txt`, `del.txt` and `old.txt`.
    fn new() -> Fx {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["config", "--local", "user.name", "T"]);
        git(&root, &["config", "--local", "user.email", "t@t"]);
        write(&root, "a.txt", "base\n");
        write(&root, "b.txt", "keep\n");
        write(&root, "del.txt", "to be deleted\n");
        let body: String = (0..20)
            .map(|n| format!("line {n} of the old file\n"))
            .collect();
        write(&root, "old.txt", &body);
        commit_all(&root, "base");
        Fx { tmp, root }
    }

    fn git_seams(&self) -> GitSeams {
        GitSeams {
            forward_env: Some(forward(self.tmp.path())),
            ..GitSeams::default()
        }
    }

    fn seamed(&self, seams: MergeSeams) -> GitRepo {
        GitRepo::new(&self.root)
            .with_seams(self.git_seams())
            .with_merge_seams(seams)
    }

    fn sha(&self, rev: &str) -> String {
        git_out(&self.root, &["rev-parse", rev])
    }

    /// A side branch adding `s.txt`, then one more commit on main adding `m.txt`: (side sha, old head).
    fn diverged(&self) -> (String, String) {
        git(&self.root, &["checkout", "-q", "-b", "side"]);
        write(&self.root, "s.txt", "side\n");
        commit_all(&self.root, "side");
        let side = self.sha("HEAD");
        git(&self.root, &["checkout", "-q", "main"]);
        write(&self.root, "m.txt", "main\n");
        commit_all(&self.root, "main work");
        (side, self.sha("HEAD"))
    }

    /// Like `diverged`, but the side branch deletes `del.txt`, renames `old.txt` and adds a directory.
    fn rich(&self) -> (String, String) {
        git(&self.root, &["checkout", "-q", "-b", "side"]);
        git(&self.root, &["rm", "-q", "del.txt"]);
        git(&self.root, &["mv", "old.txt", "renamed.txt"]);
        write(&self.root, "dir/sub/new.txt", "new\n");
        commit_all(&self.root, "side");
        let side = self.sha("HEAD");
        git(&self.root, &["checkout", "-q", "main"]);
        write(&self.root, "m.txt", "main\n");
        commit_all(&self.root, "main work");
        (side, self.sha("HEAD"))
    }

    /// A commit on top of `old` that adds `file`; the checkout is put back on `old`.
    fn moved_commit(&self, old: &str, file: &str, text: &str) -> String {
        write(&self.root, file, text);
        commit_all(&self.root, "moved");
        let moved = self.sha("HEAD");
        git(&self.root, &["reset", "-q", "--hard", old]);
        moved
    }

    fn merge(&self, repo: &GitRepo, side: &str, old: &str) -> RootMerge {
        repo.merge_into_root("main", "side", side, old, LONG)
            .unwrap()
    }

    fn lock(&self) -> PathBuf {
        self.root.join(".git/index.lock")
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

// ------------------------------------------------------------------------------------------------- branch moves

#[test]
fn a_branch_moved_to_a_commit_changing_a_merged_path_is_refused_dirty() {
    let fx = Fx::new();
    let (side, old) = fx.diverged();
    let moved = fx.moved_commit(&old, "s.txt", "moved\n");
    let repo = fx.seamed(MergeSeams {
        move_branch_before_read_tree: Some(moved.clone()),
        ..MergeSeams::default()
    });
    match fx.merge(&repo, &side, &old) {
        RootMerge::Refused(refusal) => assert_eq!(refusal.code(), "dirty_tree"),
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert_eq!(fx.sha("main"), moved);
    assert_eq!(fx.sha("HEAD"), moved);
    assert!(tree_equals(&fx.root, &moved));
    assert_eq!(
        std::fs::read_to_string(fx.root.join("s.txt")).unwrap(),
        "moved\n"
    );
}

#[test]
fn a_branch_moved_to_an_unrelated_commit_is_failed_and_not_restored() {
    let fx = Fx::new();
    let (side, old) = fx.diverged();
    let moved = fx.moved_commit(&old, "n.txt", "n\n");
    let repo = fx.seamed(MergeSeams {
        move_branch_before_read_tree: Some(moved.clone()),
        ..MergeSeams::default()
    });
    assert_eq!(
        failed(fx.merge(&repo, &side, &old)),
        ("merge_failed", false)
    );
    assert_eq!(fx.sha("main"), moved);
    assert!(index_is_merge_over(&fx.root, &moved, &side));
}

#[test]
fn a_branch_moved_before_update_ref_is_failed_without_a_rollback() {
    let fx = Fx::new();
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
    assert_eq!(
        git_out(&fx.root, &["rev-list", "--merges", &format!("{old}..main")]),
        ""
    );
    // The tree was not rewritten toward the moved commit: the merge's files are all still there.
    assert!(fx.root.join("s.txt").exists() && fx.root.join("m.txt").exists());
    assert!(index_is_merge_over(&fx.root, &old, &side));
}

#[test]
fn a_head_that_is_not_old_head_at_the_call_writes_nothing() {
    let fx = Fx::new();
    let (side, old) = fx.diverged();
    let stale = fx.sha("HEAD~1");
    let before = snapshot(&fx.root);
    let repo = fx.seamed(MergeSeams::default());
    assert_eq!(
        failed(fx.merge(&repo, &side, &stale)),
        ("merge_failed", true)
    );
    assert_eq!(snapshot(&fx.root), before);
    assert_eq!(fx.sha("main"), old);
}

// ------------------------------------------------------------------------------------------- locks and rollback

#[test]
fn a_lock_held_before_the_refresh_is_restored_and_writes_no_commit() {
    let fx = Fx::new();
    let (side, old) = fx.diverged();
    let repo = fx.seamed(MergeSeams {
        hold_lock_before_refresh: true,
        ..MergeSeams::default()
    });
    assert_eq!(failed(fx.merge(&repo, &side, &old)), ("merge_failed", true));
    assert_eq!(fx.sha("main"), old);
    assert!(fx.lock().exists());
    assert!(fx.root.join("m.txt").exists() && !fx.root.join("s.txt").exists());
}

#[test]
fn a_lock_held_before_read_tree_is_restored_and_left_untouched() {
    let fx = Fx::new();
    let (side, old) = fx.diverged();
    let repo = fx.seamed(MergeSeams {
        hold_lock_before_read_tree: true,
        ..MergeSeams::default()
    });
    assert_eq!(failed(fx.merge(&repo, &side, &old)), ("merge_failed", true));
    assert_eq!(fx.sha("main"), old);
    assert_eq!(std::fs::metadata(fx.lock()).unwrap().len(), 0);
    assert!(!fx.root.join("s.txt").exists());
}

#[test]
fn a_lock_held_in_a_linked_worktree_is_its_own_index_lock() {
    let fx = Fx::new();
    let (side, old) = fx.diverged();
    let linked = fx.tmp.path().join("linked");
    git(
        &fx.root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "wt",
            linked.to_str().unwrap(),
            &old,
        ],
    );
    let lock = PathBuf::from(git_out(&linked, &["rev-parse", "--git-path", "index.lock"]));
    let lock = if lock.is_absolute() {
        lock
    } else {
        linked.join(lock)
    };
    assert_ne!(lock, fx.lock());
    let repo = GitRepo::new(&linked)
        .with_seams(fx.git_seams())
        .with_merge_seams(MergeSeams {
            hold_lock_before_read_tree: true,
            ..MergeSeams::default()
        });
    let result = repo
        .merge_into_root("wt", "side", &side, &old, LONG)
        .unwrap();
    assert_eq!(failed(result), ("merge_failed", true));
    assert!(lock.exists());
    assert!(!fx.lock().exists());
    assert_eq!(git_out(&linked, &["rev-parse", "wt"]), old);
    assert!(!linked.join("s.txt").exists());
}

#[test]
fn a_held_ref_lock_rolls_back_a_delete_a_rename_and_an_added_directory() {
    let fx = Fx::new();
    let (side, old) = fx.rich();
    let before = snapshot(&fx.root);
    let repo = fx.seamed(MergeSeams {
        hold_ref_lock_before_update_ref: true,
        ..MergeSeams::default()
    });
    assert_eq!(failed(fx.merge(&repo, &side, &old)), ("merge_failed", true));
    assert_eq!(fx.sha("main"), old);
    assert_eq!(snapshot(&fx.root), before);
    assert!(fx.root.join("del.txt").exists());
    assert!(fx.root.join("old.txt").exists());
    assert!(!fx.root.join("renamed.txt").exists());
    assert!(!fx.root.join("dir").exists());
    assert!(tree_equals(&fx.root, &old));
}

#[test]
fn a_lock_left_by_a_killed_read_tree_is_index_lock_left() {
    let fx = Fx::new();
    let (side, old) = fx.diverged();
    let repo = fx.seamed(MergeSeams {
        lock_after_read_tree_killed: true,
        ..MergeSeams::default()
    });
    assert_eq!(
        failed(fx.merge(&repo, &side, &old)),
        ("index_lock_left", false)
    );
    assert!(fx.lock().exists());
}

#[test]
fn a_killed_read_tree_with_a_lock_held_before_is_restored() {
    let fx = Fx::new();
    let (side, old) = fx.diverged();
    let repo = fx.seamed(MergeSeams {
        lock_after_read_tree_killed: true,
        hold_lock_before_read_tree: true,
        ..MergeSeams::default()
    });
    assert_eq!(failed(fx.merge(&repo, &side, &old)), ("merge_failed", true));
    assert!(fx.lock().exists());
}

#[test]
fn a_lock_left_by_a_killed_refresh_is_index_lock_left() {
    let fx = Fx::new();
    let (side, old) = fx.diverged();
    let repo = fx.seamed(MergeSeams {
        lock_after_refresh_killed: true,
        ..MergeSeams::default()
    });
    assert_eq!(
        failed(fx.merge(&repo, &side, &old)),
        ("index_lock_left", false)
    );
    assert!(fx.lock().exists());
    assert_eq!(fx.sha("main"), old);
}

#[test]
fn a_read_tree_failing_after_it_wrote_is_rolled_back() {
    let fx = Fx::new();
    let (side, old) = fx.diverged();
    let repo = fx.seamed(MergeSeams {
        fail_read_tree_after_write: true,
        ..MergeSeams::default()
    });
    assert_eq!(failed(fx.merge(&repo, &side, &old)), ("merge_failed", true));
    assert!(tree_equals(&fx.root, &old));
    assert!(!fx.root.join("s.txt").exists());
}

#[test]
fn a_failing_read_tree_over_a_moved_branch_runs_no_rollback() {
    let fx = Fx::new();
    let (side, old) = fx.diverged();
    let moved = fx.moved_commit(&old, "n.txt", "n\n");
    let repo = fx.seamed(MergeSeams {
        fail_read_tree_after_write: true,
        move_branch_before_read_tree: Some(moved.clone()),
        ..MergeSeams::default()
    });
    assert_eq!(
        failed(fx.merge(&repo, &side, &old)),
        ("merge_failed", false)
    );
    assert_eq!(fx.sha("main"), moved);
    // Nothing was rolled back: read-tree's merge is still in the index and the working tree.
    assert!(index_is_merge_over(&fx.root, &moved, &side));
    assert!(fx.root.join("s.txt").exists());
}

// --------------------------------------------------------------------------------------------------- deadlines

#[test]
fn a_forced_update_ref_timeout_rolls_back_and_keeps_an_unrelated_untracked_file() {
    let fx = Fx::new();
    let (side, old) = fx.diverged();
    write(&fx.root, "scratch.txt", "mine\n");
    let repo = fx.seamed(MergeSeams {
        force_update_ref_timeout: true,
        ..MergeSeams::default()
    });
    assert_eq!(failed(fx.merge(&repo, &side, &old)), ("timeout", true));
    assert_eq!(fx.sha("main"), old);
    assert!(tree_equals(&fx.root, &old));
    assert_eq!(
        std::fs::read_to_string(fx.root.join("scratch.txt")).unwrap(),
        "mine\n"
    );
}

#[test]
fn an_expired_deadline_before_update_ref_rolls_back_and_leaves_the_branch() {
    let fx = Fx::new();
    let (side, old) = fx.diverged();
    let repo = fx.seamed(MergeSeams {
        expire_deadline_before_update_ref: true,
        ..MergeSeams::default()
    });
    assert_eq!(failed(fx.merge(&repo, &side, &old)), ("timeout", true));
    assert_eq!(fx.sha("main"), old);
    assert!(tree_equals(&fx.root, &old));
}

#[test]
fn an_expired_rollback_deadline_over_a_held_ref_lock_is_not_restored() {
    let fx = Fx::new();
    let (side, old) = fx.diverged();
    let repo = fx.seamed(MergeSeams {
        hold_ref_lock_before_update_ref: true,
        expire_rollback_deadline: true,
        ..MergeSeams::default()
    });
    assert_eq!(
        failed(fx.merge(&repo, &side, &old)),
        ("merge_failed", false)
    );
    assert_eq!(fx.sha("main"), old);
}

// ----------------------------------------------------------------------------------- the rollback's own window

#[test]
fn an_untracked_file_at_a_merged_path_stops_the_rollback_and_is_kept() {
    let fx = Fx::new();
    let (side, old) = fx.rich();
    // The merge deletes `del.txt`, so a file created there after read-tree is untracked.
    let repo = fx.seamed(MergeSeams {
        hold_ref_lock_before_update_ref: true,
        create_file_after_read_tree: Some((PathBuf::from("del.txt"), b"mine\n".to_vec())),
        ..MergeSeams::default()
    });
    assert_eq!(
        failed(fx.merge(&repo, &side, &old)),
        ("merge_failed", false)
    );
    assert_eq!(
        std::fs::read_to_string(fx.root.join("del.txt")).unwrap(),
        "mine\n"
    );
    // No rollback ran: the merge's other changes are still in the working tree.
    assert!(fx.root.join("dir/sub/new.txt").exists());
    assert!(fx.root.join("renamed.txt").exists());
    assert_eq!(fx.sha("main"), old);
}

#[test]
fn an_edit_of_a_merged_path_before_the_rollback_makes_it_fail_and_is_kept() {
    let fx = Fx::new();
    let (side, old) = fx.diverged();
    let repo = fx.seamed(MergeSeams {
        hold_ref_lock_before_update_ref: true,
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
    assert_eq!(fx.sha("main"), old);
}

// ---------------------------------------------------------------------------------------- the default seams

/// A git that logs the words (letters and dashes only, so no path or id) of each child's arguments, then runs the
/// real one.
fn counting_git(dir: &Path, log: &Path) -> PathBuf {
    let real = std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .map(|dir| dir.join("git"))
        .find(|candidate| candidate.is_file())
        .expect("git on PATH");
    let path = dir.join("counting-git");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\nfor a in \"$@\"; do case \"$a\" in ''|*[!a-z-]*) ;; *) printf '%s ' \"$a\" >> '{}';; esac; done\necho >> '{}'\nexec '{}' \"$@\"\n",
            log.display(),
            log.display(),
            real.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn default_merge_seams_run_the_same_children_as_none() {
    let mut logs = Vec::new();
    for with_default_seams in [false, true] {
        let fx = Fx::new();
        let (side, old) = fx.diverged();
        let log = fx.tmp.path().join("calls.log");
        let fake = counting_git(fx.tmp.path(), &log);
        let seams = GitSeams {
            git_program: Some(fake),
            ..fx.git_seams()
        };
        let mut repo = GitRepo::new(&fx.root).with_seams(seams);
        if with_default_seams {
            repo = repo.with_merge_seams(MergeSeams::default());
        }
        merged(fx.merge(&repo, &side, &old));
        logs.push(std::fs::read_to_string(&log).unwrap());
    }
    assert!(logs[0].contains("commit-tree") && logs[0].contains("update-ref"));
    assert_eq!(logs[0], logs[1]);
}
