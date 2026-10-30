//! Step 10 of the preflight of a merge into the root checkout: ignored files the merge would overwrite, with real git
//! in temporary repositories.

use capstan_kernel::integrate::{GitRepo, GitSeams, Preflight, ENTRY_WOULD_OVERWRITE};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(120);

struct Fx {
    tmp: tempfile::TempDir,
    root: PathBuf,
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

fn commit_all(dir: &Path, message: &str) {
    git(dir, &["add", "-f", "-A"]);
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

fn put(root: &Path, name: &str, text: &str) {
    let path = root.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

impl Fx {
    /// A repository on `main` with a.txt, src/a.rs and a .gitignore of `ignore`, one commit, a local identity.
    fn new(ignore: &str) -> Fx {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["config", "--local", "user.name", "T"]);
        git(&root, &["config", "--local", "user.email", "t@t"]);
        put(&root, "a.txt", "base\n");
        put(&root, "src/a.rs", "base\n");
        put(&root, ".gitignore", ignore);
        commit_all(&root, "base");
        Fx { tmp, root }
    }

    fn repo(&self) -> GitRepo {
        let home = self.tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        GitRepo::new(&self.root).with_seams(GitSeams {
            forward_env: Some(vec![
                ("HOME".into(), home.to_string_lossy().into_owned()),
                ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ]),
            ..GitSeams::default()
        })
    }

    /// A branch `name` that writes `files` (forced past the ignore rules); main is checked out again.
    fn side(&self, name: &str, files: &[(&str, &str)]) -> String {
        git(&self.root, &["checkout", "-q", "-b", name]);
        for (file, text) in files {
            put(&self.root, file, text);
        }
        commit_all(&self.root, name);
        let sha = git_out(&self.root, &["rev-parse", "HEAD"]);
        git(&self.root, &["checkout", "-q", "main"]);
        sha
    }

    fn run(&self, sha: &str) -> Preflight {
        self.repo().root_merge_preflight("main", sha, LONG).unwrap()
    }

    fn index(&self) -> Vec<u8> {
        std::fs::read(self.root.join(".git/index")).unwrap()
    }

    /// The preflight must refuse with dirty_tree naming `files` and leave the index bytes alone.
    fn refused(&self, sha: &str, files: &[&str]) {
        let before = self.index();
        match self.run(sha) {
            Preflight::Refused(r) => {
                assert_eq!(r.code(), "dirty_tree");
                assert_eq!(r.files(), files);
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert_eq!(self.index(), before);
    }

    fn ready(&self, sha: &str) {
        let p = self.run(sha);
        assert!(
            matches!(p, Preflight::Ready { .. }),
            "expected ready, got {p:?}"
        );
    }
}

#[test]
fn an_ignored_file_at_a_path_the_merge_adds_is_dirty_tree() {
    let fx = Fx::new("*.o\n");
    let sha = fx.side("topic", &[("x.o", "side\n")]);
    put(&fx.root, "x.o", "local\n");
    fx.refused(&sha, &["x.o"]);
}

#[test]
fn an_ignored_file_under_a_directory_the_merge_adds_is_dirty_tree() {
    let fx = Fx::new("gen/\n");
    let sha = fx.side("topic", &[("gen/deep/a.txt", "side\n")]);
    put(&fx.root, "gen/deep/a.txt", "local\n");
    fx.refused(&sha, &["gen/deep/a.txt"]);
}

#[test]
fn an_ignored_file_where_the_merge_adds_a_directory_is_dirty_tree() {
    let fx = Fx::new("d\n");
    let sha = fx.side("topic", &[("d/x", "side\n")]);
    put(&fx.root, "d", "local\n");
    fx.refused(&sha, &["d"]);
}

#[test]
fn a_case_variant_is_dirty_tree_with_ignorecase_in_the_local_config() {
    let fx = Fx::new("*.o\n");
    git(&fx.root, &["config", "--local", "core.ignorecase", "true"]);
    let sha = fx.side("topic", &[("d/x.o", "side\n")]);
    put(&fx.root, "D/x.o", "local\n");
    fx.refused(&sha, &["D/x.o"]);
}

#[test]
fn a_case_variant_of_a_file_parent_is_dirty_tree_with_ignorecase() {
    let fx = Fx::new("D\n");
    git(&fx.root, &["config", "--local", "core.ignorecase", "true"]);
    let sha = fx.side("topic", &[("d/x", "side\n")]);
    put(&fx.root, "D", "local\n");
    fx.refused(&sha, &["D"]);
}

#[test]
fn ignorecase_only_in_the_global_config_compares_exactly() {
    let fx = Fx::new("*.o\n");
    let sha = fx.side("topic", &[("d/x.o", "side\n")]);
    put(&fx.root, "D/x.o", "local\n");
    let home = fx.tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join(".gitconfig"), "[core]\n\tignorecase = true\n").unwrap();
    fx.ready(&sha);
}

#[test]
fn an_excludes_file_only_in_the_global_config_does_not_apply() {
    let fx = Fx::new("");
    let sha = fx.side("topic", &[("x.o", "side\n")]);
    put(&fx.root, "x.o", "local\n");
    let home = fx.tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("excludes"), "*.o\n").unwrap();
    std::fs::write(
        home.join(".gitconfig"),
        format!(
            "[core]\n\texcludesFile = {}\n",
            home.join("excludes").display()
        ),
    )
    .unwrap();
    fx.ready(&sha);
}

#[test]
fn ignored_files_the_merge_does_not_touch_are_ready() {
    let fx = Fx::new("target/\n*.bak\n");
    let sha = fx.side("topic", &[("src/b.rs", "side\n")]);
    put(&fx.root, "src/target/x", "local\n");
    put(&fx.root, "src/a.rs.bak", "local\n");
    fx.ready(&sha);
    let modify = fx.side("edit", &[("src/a.rs", "side\n")]);
    fx.ready(&modify);
}

#[test]
fn a_tracked_changed_path_that_matches_an_ignore_pattern_is_ready() {
    let fx = Fx::new("*.o\n");
    put(&fx.root, "k.o", "base\n");
    commit_all(&fx.root, "track k.o");
    let sha = fx.side("topic", &[("k.o", "side\n")]);
    fx.ready(&sha);
}

#[test]
fn a_non_ascii_case_variant_is_compared_exactly() {
    let fx = Fx::new("*.o\n");
    git(&fx.root, &["config", "--local", "core.ignorecase", "true"]);
    let sha = fx.side("topic", &[("\u{c9}/x.o", "side\n")]);
    put(&fx.root, "\u{e9}/x.o", "local\n");
    fx.ready(&sha);
}

#[test]
fn an_ignored_file_in_a_later_pathspec_chunk_is_dirty_tree() {
    let fx = Fx::new("*.o\n");
    let files: Vec<(String, String)> = (0..1100)
        .map(|n| (format!("gen/f{n:04}.o"), "side\n".to_string()))
        .collect();
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let sha = fx.side("topic", &refs);
    put(&fx.root, "gen/f1099.o", "local\n");
    fx.refused(&sha, &["gen/f1099.o"]);
}

#[test]
fn a_conflict_is_reported_before_an_ignored_file() {
    let fx = Fx::new("*.o\n");
    let sha = fx.side("topic", &[("a.txt", "side\n"), ("x.o", "side\n")]);
    put(&fx.root, "a.txt", "main\n");
    commit_all(&fx.root, "main edit");
    put(&fx.root, "x.o", "local\n");
    match fx.run(&sha) {
        Preflight::Refused(r) => assert_eq!(r.code(), "merge_conflict"),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn a_non_utf8_top_level_path_through_the_dot_listing_is_dirty_tree() {
    let fx = Fx::new("*.o\n");
    let name = std::ffi::OsStr::from_bytes(b"caf\xe9.o");
    git(&fx.root, &["checkout", "-q", "-b", "topic"]);
    std::fs::write(fx.root.join(name), "side\n").unwrap();
    commit_all(&fx.root, "topic");
    let sha = git_out(&fx.root, &["rev-parse", "HEAD"]);
    git(&fx.root, &["checkout", "-q", "main"]);
    std::fs::write(fx.root.join(name), "local\n").unwrap();
    let before = fx.index();
    match fx.run(&sha) {
        Preflight::Refused(r) => {
            assert_eq!(r.code(), "dirty_tree");
            assert_ne!(r.files(), [ENTRY_WOULD_OVERWRITE]);
            assert_eq!(r.files().len(), 1);
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert_eq!(fx.index(), before);
}
