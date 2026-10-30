//! Step 7 of the preflight of a merge into the root checkout: an operation in progress or a tracked change, with real
//! git in temporary repositories.

use capstan_kernel::integrate::{GitRepo, GitSeams, Preflight, ENTRY_UNRELATED_HISTORIES};
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
        commit_all(&root, "base");
        Fx { tmp, root }
    }

    fn repo(&self) -> GitRepo {
        self.repo_with(GitSeams::default())
    }

    fn repo_with(&self, mut seams: GitSeams) -> GitRepo {
        seams.forward_env = Some(forward(self.tmp.path()));
        GitRepo::new(&self.root).with_seams(seams)
    }

    fn sha(&self, rev: &str) -> String {
        git_out(&self.root, &["rev-parse", rev])
    }

    /// A branch `name` with one more file, main checked out again.
    fn side_branch(&self, name: &str, file: &str, text: &str) -> String {
        git(&self.root, &["checkout", "-q", "-b", name]);
        write(&self.root, file, text);
        commit_all(&self.root, name);
        let sha = self.sha("HEAD");
        git(&self.root, &["checkout", "-q", "main"]);
        sha
    }

    fn run(&self, branch: &str, sha: &str) -> Preflight {
        self.repo().root_merge_preflight(branch, sha, LONG).unwrap()
    }
}

fn refusal(p: Preflight) -> (String, Vec<String>, usize) {
    match p {
        Preflight::Refused(r) => (r.code().to_string(), r.files().to_vec(), r.omitted()),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn expect(p: Preflight, code: &str, files: &[&str]) {
    let (got, entries, omitted) = refusal(p);
    assert_eq!(got, code);
    assert_eq!(entries, files);
    assert_eq!(omitted, 0);
}

fn tree_files(dir: &Path, base: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().is_some_and(|name| name == ".git") {
            continue;
        }
        if path.is_dir() {
            tree_files(&path, base, out);
        } else {
            let name = path.strip_prefix(base).unwrap().to_string_lossy().into();
            out.push((name, std::fs::read(&path).unwrap()));
        }
    }
}

/// HEAD, the index bytes and every working file.
type Snap = (String, Option<Vec<u8>>, Vec<(String, Vec<u8>)>);

fn snap(root: &Path) -> Snap {
    let head = std::fs::read_to_string(root.join(".git/HEAD")).unwrap();
    let mut files = Vec::new();
    tree_files(root, root, &mut files);
    files.sort();
    (head, std::fs::read(root.join(".git/index")).ok(), files)
}

/// Runs `check`, which must leave HEAD, the index and the files alone, also when the stat data of a file is stale.
fn unchanged<T>(fx: &Fx, check: impl Fn() -> T) -> T {
    let before = snap(&fx.root);
    let first = check();
    assert_eq!(snap(&fx.root), before);
    // A rewritten file has a new mtime, so the index stat is stale; a refresh would rewrite .git/index.
    std::thread::sleep(Duration::from_millis(1100));
    let text = std::fs::read(fx.root.join("a.txt")).unwrap();
    std::fs::write(fx.root.join("a.txt"), text).unwrap();
    let stale = snap(&fx.root);
    let second = check();
    assert_eq!(snap(&fx.root), stale);
    drop(first);
    second
}

fn ready(p: Preflight) {
    assert!(
        matches!(p, Preflight::Ready { .. }),
        "expected ready, got {p:?}"
    );
}

fn index_bytes(root: &Path) -> Vec<u8> {
    std::fs::read(root.join(".git/index")).unwrap()
}

#[test]
fn each_operation_marker_is_dirty_tree_with_its_name() {
    let markers: [(&str, bool); 7] = [
        ("MERGE_HEAD", false),
        ("CHERRY_PICK_HEAD", false),
        ("REVERT_HEAD", false),
        ("rebase-merge", true),
        ("rebase-apply", true),
        ("sequencer", true),
        ("BISECT_LOG", false),
    ];
    for (name, dir) in markers {
        let fx = Fx::new(true);
        let head = fx.side_branch("topic", "t.txt", "t\n");
        let path = fx.root.join(".git").join(name);
        if dir {
            std::fs::create_dir(&path).unwrap();
        } else {
            std::fs::write(&path, format!("{head}\n")).unwrap();
        }
        let before = index_bytes(&fx.root);
        expect(fx.run("main", &head), "dirty_tree", &[name]);
        assert_eq!(index_bytes(&fx.root), before, "{name}");
    }
}

#[test]
fn a_dangling_marker_symlink_still_counts() {
    let fx = Fx::new(true);
    let head = fx.side_branch("topic", "t.txt", "t\n");
    std::os::unix::fs::symlink("nowhere", fx.root.join(".git/MERGE_HEAD")).unwrap();
    expect(fx.run("main", &head), "dirty_tree", &["MERGE_HEAD"]);
}

#[test]
fn merge_head_in_a_linked_worktree_is_found_by_its_absolute_path() {
    let fx = Fx::new(true);
    let head = fx.side_branch("topic", "t.txt", "t\n");
    let linked = fx.tmp.path().join("linked");
    git(
        &fx.root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "wt",
            &linked.to_string_lossy(),
        ],
    );
    let repo = || {
        let seams = GitSeams {
            forward_env: Some(forward(fx.tmp.path())),
            ..GitSeams::default()
        };
        GitRepo::new(&linked).with_seams(seams)
    };
    ready(repo().root_merge_preflight("wt", &head, LONG).unwrap());
    let marker = git_out(&linked, &["rev-parse", "--git-path", "MERGE_HEAD"]);
    assert!(Path::new(&marker).is_absolute() || !marker.starts_with(".git/"));
    std::fs::write(
        if Path::new(&marker).is_absolute() {
            PathBuf::from(&marker)
        } else {
            linked.join(&marker)
        },
        format!("{head}\n"),
    )
    .unwrap();
    expect(
        repo().root_merge_preflight("wt", &head, LONG).unwrap(),
        "dirty_tree",
        &["MERGE_HEAD"],
    );
}

#[test]
fn merge_head_in_the_main_worktree_is_found_by_its_relative_path() {
    let fx = Fx::new(true);
    let head = fx.side_branch("topic", "t.txt", "t\n");
    assert_eq!(
        git_out(&fx.root, &["rev-parse", "--git-path", "MERGE_HEAD"]),
        ".git/MERGE_HEAD"
    );
    std::fs::write(fx.root.join(".git/MERGE_HEAD"), format!("{head}\n")).unwrap();
    expect(fx.run("main", &head), "dirty_tree", &["MERGE_HEAD"]);
}

#[test]
fn a_staged_edit_and_a_tracked_edit_are_dirty_tree() {
    let fx = Fx::new(true);
    let head = fx.side_branch("topic", "t.txt", "t\n");
    write(&fx.root, "a.txt", "staged\n");
    git(&fx.root, &["add", "a.txt"]);
    expect(
        unchanged(&fx, || fx.run("main", &head)),
        "dirty_tree",
        &["a.txt"],
    );

    let fx = Fx::new(true);
    let head = fx.side_branch("topic", "t.txt", "t\n");
    write(&fx.root, "a.txt", "edited\n");
    expect(
        unchanged(&fx, || fx.run("main", &head)),
        "dirty_tree",
        &["a.txt"],
    );
}

#[test]
fn untracked_files_do_not_count() {
    let fx = Fx::new(true);
    let head = fx.side_branch("topic", "t.txt", "t\n");
    write(&fx.root, "loose.txt", "x\n");
    ready(unchanged(&fx, || fx.run("main", &head)));
}

#[test]
fn a_staged_rename_names_both_paths_whatever_status_renames_says() {
    let fx = Fx::new(true);
    let head = fx.side_branch("topic", "t.txt", "t\n");
    git(&fx.root, &["config", "--local", "status.renames", "true"]);
    git(&fx.root, &["config", "--local", "diff.renames", "copies"]);
    git(&fx.root, &["mv", "a.txt", "b.txt"]);
    let before = index_bytes(&fx.root);
    expect(fx.run("main", &head), "dirty_tree", &["a.txt", "b.txt"]);
    assert_eq!(index_bytes(&fx.root), before);
}

/// A root with a submodule `sub`, committed; the submodule's own commits are made in `fx.tmp/subsrc` clones.
fn with_submodule() -> (Fx, String) {
    let fx = Fx::new(true);
    let src = fx.tmp.path().join("subsrc");
    std::fs::create_dir_all(&src).unwrap();
    git(&src, &["init", "-q", "-b", "main"]);
    write(&src, "s.txt", "s\n");
    commit_all(&src, "s1");
    git(
        &fx.root,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            &src.to_string_lossy(),
            "sub",
        ],
    );
    commit_all(&fx.root, "add sub");
    let head = fx.side_branch("topic", "t.txt", "t\n");
    (fx, head)
}

#[test]
fn a_staged_submodule_pointer_change_is_dirty_tree() {
    let (fx, head) = with_submodule();
    let sub = fx.root.join("sub");
    write(&sub, "s.txt", "s2\n");
    commit_all(&sub, "s2");
    git(&fx.root, &["add", "sub"]);
    expect(
        unchanged(&fx, || fx.run("main", &head)),
        "dirty_tree",
        &["sub"],
    );
}

#[test]
fn a_submodule_whose_head_moved_is_dirty_tree_but_a_dirty_worktree_is_not() {
    let (fx, head) = with_submodule();
    let sub = fx.root.join("sub");
    write(&sub, "s.txt", "dirty\n");
    ready(unchanged(&fx, || fx.run("main", &head)));
    git(&sub, &["checkout", "-q", "--", "s.txt"]);
    write(&sub, "s.txt", "s2\n");
    commit_all(&sub, "s2");
    expect(
        unchanged(&fx, || fx.run("main", &head)),
        "dirty_tree",
        &["sub"],
    );
}

#[test]
fn step_6_comes_before_step_7_and_step_7_before_step_9() {
    let fx = Fx::new(false);
    let head = fx.side_branch("topic", "t.txt", "t\n");
    write(&fx.root, "a.txt", "edited\n");
    let (code, _, _) = refusal(fx.run("main", &head));
    assert_eq!(code, "no_identity");

    let tmp = tempfile::tempdir().unwrap();
    let other = tmp.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    git(&other, &["init", "-q", "-b", "side"]);
    write(&other, "o.txt", "o\n");
    commit_all(&other, "other");
    let unrelated = git_out(&other, &["rev-parse", "HEAD"]);
    let fx = Fx::new(true);
    git(
        &fx.root,
        &["fetch", "-q", &other.to_string_lossy(), "refs/heads/side"],
    );
    expect(
        fx.run("main", &unrelated),
        "merge_conflict",
        &[ENTRY_UNRELATED_HISTORIES],
    );
    write(&fx.root, "a.txt", "edited\n");
    expect(fx.run("main", &unrelated), "dirty_tree", &["a.txt"]);
}
