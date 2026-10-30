//! The preflight of a merge into the root checkout: real git in temporary repositories, a wrapper around git where a
//! step needs an exit code git does not give on demand.

use capstan_kernel::integrate::{
    GitRepo, GitSeams, IntegrateError, Preflight, ENTRY_DETACHED_HEAD, ENTRY_UNBORN_HEAD,
    ENTRY_UNRELATED_HISTORIES,
};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

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

fn code_of(error: IntegrateError) -> String {
    match error {
        IntegrateError::Integration { code, .. } => code,
        other => panic!("unexpected {other:?}"),
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

/// A git that logs `GIT_OPTIONAL_LOCKS|argv` and answers by `answers` (a shell `case` body), else runs the real one.
fn wrapper(tmp: &Path, answers: &str) -> (PathBuf, PathBuf) {
    let real = Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    let real = String::from_utf8_lossy(&real.stdout).trim().to_string();
    let log = tmp.join("wrapper.log");
    let script = tmp.join("wrapper-git");
    let body = format!(
        "#!/bin/sh\necho \"${{GIT_OPTIONAL_LOCKS-unset}}|$*\" >> '{log}'\ncase \" $* \" in\n{answers}\nesac\nexec '{real}' \"$@\"\n",
        log = log.display()
    );
    std::fs::write(&script, body).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    (script, log)
}

fn wrapped(fx: &Fx, answers: &str, skip_probe: bool) -> (GitRepo, PathBuf) {
    let (program, log) = wrapper(fx.tmp.path(), answers);
    let repo = fx.repo_with(GitSeams {
        git_program: Some(program),
        skip_version_probe: skip_probe,
        reader_detach: Some(Duration::from_millis(100)),
        ..GitSeams::default()
    });
    (repo, log)
}

// ------------------------------------------------------------------------------------------------ the refusals

#[test]
fn unsafe_config_names_the_key_and_runs_nothing() {
    let fx = Fx::new(true);
    let head = fx.side_branch("topic", "t.txt", "t\n");
    let marker = fx.tmp.path().join("ran");
    git(&fx.root, &["config", "--local", "merge.x.driver", "true"]);
    let refused = unchanged(&fx, || fx.run("main", &head));
    expect(refused, "unsafe_config", &["merge.x.driver"]);

    let fx = Fx::new(true);
    let head = fx.side_branch("topic", "t.txt", "t\n");
    write(&fx.root, ".gitattributes", "*.txt filter=x\n");
    commit_all(&fx.root, "attributes");
    let script = fx.tmp.path().join("smudge.sh");
    write(
        fx.tmp.path(),
        "smudge.sh",
        &format!("#!/bin/sh\ntouch '{}'\ncat\n", marker.display()),
    );
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    git(
        &fx.root,
        &[
            "config",
            "--local",
            "filter.x.smudge",
            &script.to_string_lossy(),
        ],
    );
    let refused = unchanged(&fx, || fx.run("main", &head));
    expect(refused, "unsafe_config", &["filter.x.smudge"]);
    assert!(!marker.exists());
}

#[test]
fn invalid_commit_for_a_short_name_a_non_commit_and_an_unknown_commit() {
    let fx = Fx::new(true);
    let head = fx.side_branch("topic", "t.txt", "t\n");
    let short = &head[..39];
    expect(
        unchanged(&fx, || fx.run("main", short)),
        "invalid_commit",
        &[short],
    );
    let tree = git_out(&fx.root, &["rev-parse", "HEAD^{tree}"]);
    expect(fx.run("main", &tree), "invalid_commit", &[&tree]);
    let upper = head.to_uppercase();
    expect(fx.run("main", &upper), "invalid_commit", &[&upper]);
    let unknown = "1".repeat(40);
    expect(fx.run("main", &unknown), "invalid_commit", &[&unknown]);
}

#[test]
fn invalid_branch_by_the_name_rules_by_git_and_for_the_target() {
    let fx = Fx::new(true);
    let head = fx.side_branch("topic", "t.txt", "t\n");
    expect(
        unchanged(&fx, || fx.run("-x", &head)),
        "invalid_branch",
        &["-x"],
    );
    let (repo, _) = wrapped(&fx, "  *\" check-ref-format \"*) exit 128;;", true);
    expect(
        repo.root_merge_preflight("main", &head, LONG).unwrap(),
        "invalid_branch",
        &["main"],
    );
    let error = fx.repo().merge_target(Some("-x")).unwrap_err();
    assert_eq!(code_of(error), "invalid_branch");
    assert_eq!(
        fx.repo().merge_target(Some("release/1")).unwrap(),
        Some("release/1".into())
    );
}

#[test]
fn wrong_branch_detached_other_remote_ref_and_unborn() {
    let fx = Fx::new(true);
    let head = fx.side_branch("topic", "t.txt", "t\n");
    git(&fx.root, &["checkout", "-q", "topic"]);
    expect(
        unchanged(&fx, || fx.run("main", &head)),
        "wrong_branch",
        &["topic"],
    );
    git(&fx.root, &["checkout", "-q", "main"]);
    git(&fx.root, &["checkout", "-q", "--detach"]);
    expect(
        unchanged(&fx, || fx.run("main", &head)),
        "wrong_branch",
        &[ENTRY_DETACHED_HEAD],
    );
    git(&fx.root, &["checkout", "-q", "main"]);
    git(&fx.root, &["update-ref", "refs/remotes/x", "HEAD"]);
    git(&fx.root, &["symbolic-ref", "HEAD", "refs/remotes/x"]);
    expect(fx.run("main", &head), "wrong_branch", &["refs/remotes/x"]);
    git(&fx.root, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    // The name is compared byte for byte, with case.
    expect(fx.run("Main", &head), "wrong_branch", &["main"]);
}

#[test]
fn an_unborn_head_is_wrong_branch() {
    let fx = Fx::new(true);
    let head = fx.sha("HEAD");
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    // The commit is fetched without a checkout, so step 2 passes and HEAD on main has no commit.
    git(
        &root,
        &["fetch", "-q", &fx.root.to_string_lossy(), "refs/heads/main"],
    );
    let repo = GitRepo::new(&root).with_seams(GitSeams {
        forward_env: Some(forward(tmp.path())),
        ..GitSeams::default()
    });
    let result = repo.root_merge_preflight("main", &head, LONG);
    let (code, files, _) = refusal(result.unwrap());
    assert_eq!(code, "wrong_branch");
    assert_eq!(files, [ENTRY_UNBORN_HEAD]);
}

#[test]
fn already_in_head() {
    let fx = Fx::new(true);
    let head = fx.sha("HEAD");
    assert_eq!(
        unchanged(&fx, || fx.run("main", &head)),
        Preflight::AlreadyInHead
    );
}

#[test]
fn no_identity_lists_the_missing_keys() {
    let fx = Fx::new(false);
    let head = fx.side_branch("topic", "t.txt", "t\n");
    expect(
        unchanged(&fx, || fx.run("main", &head)),
        "no_identity",
        &["user.email", "user.name"],
    );
    git(&fx.root, &["config", "--local", "user.email", "t@t"]);
    expect(fx.run("main", &head), "no_identity", &["user.name"]);
    git(&fx.root, &["config", "--local", "user.name", ""]);
    expect(fx.run("main", &head), "no_identity", &["user.name"]);
    git(&fx.root, &["config", "--local", "user.name", "T"]);
    git(&fx.root, &["config", "--local", "--unset", "user.email"]);
    expect(fx.run("main", &head), "no_identity", &["user.email"]);
}

#[test]
fn git_too_old_wins_over_the_config_and_names_nothing() {
    let fx = Fx::new(true);
    let head = fx.side_branch("topic", "t.txt", "t\n");
    git(&fx.root, &["config", "--local", "merge.x.driver", "true"]);
    let repo = fx.repo_with(GitSeams {
        force_old_git: true,
        ..GitSeams::default()
    });
    expect(
        repo.root_merge_preflight("main", &head, LONG).unwrap(),
        "git_too_old",
        &[],
    );
}

#[test]
fn merge_conflict_one_file_many_files_escapes_and_unrelated_histories() {
    let fx = Fx::new(true);
    git(&fx.root, &["checkout", "-q", "-b", "topic"]);
    write(&fx.root, "a.txt", "topic\n");
    commit_all(&fx.root, "topic");
    let head = fx.sha("HEAD");
    git(&fx.root, &["checkout", "-q", "main"]);
    write(&fx.root, "a.txt", "main\n");
    commit_all(&fx.root, "main");
    expect(
        unchanged(&fx, || fx.run("main", &head)),
        "merge_conflict",
        &["a.txt"],
    );

    // Over the limit: 60 files conflict, 50 are listed.
    let fx = Fx::new(true);
    for i in 0..60 {
        write(&fx.root, &format!("f{i:02}.txt"), "base\n");
    }
    commit_all(&fx.root, "files");
    git(&fx.root, &["checkout", "-q", "-b", "topic"]);
    for i in 0..60 {
        write(&fx.root, &format!("f{i:02}.txt"), "topic\n");
    }
    commit_all(&fx.root, "topic");
    let head = fx.sha("HEAD");
    git(&fx.root, &["checkout", "-q", "main"]);
    for i in 0..60 {
        write(&fx.root, &format!("f{i:02}.txt"), "main\n");
    }
    commit_all(&fx.root, "main");
    let (code, files, omitted) = refusal(fx.run("main", &head));
    assert_eq!(
        (code.as_str(), files.len(), omitted),
        ("merge_conflict", 50, 10)
    );

    // A space is kept, a newline is escaped.
    let fx = Fx::new(true);
    write(&fx.root, "a b", "base\n");
    write(&fx.root, "n\nl", "base\n");
    commit_all(&fx.root, "names");
    git(&fx.root, &["checkout", "-q", "-b", "topic"]);
    write(&fx.root, "a b", "topic\n");
    write(&fx.root, "n\nl", "topic\n");
    commit_all(&fx.root, "topic");
    let head = fx.sha("HEAD");
    git(&fx.root, &["checkout", "-q", "main"]);
    write(&fx.root, "a b", "main\n");
    write(&fx.root, "n\nl", "main\n");
    commit_all(&fx.root, "main");
    expect(fx.run("main", &head), "merge_conflict", &["a b", "n\\x0al"]);

    // No common ancestor.
    let fx = Fx::new(true);
    git(&fx.root, &["checkout", "-q", "--orphan", "other"]);
    git(&fx.root, &["rm", "-rfq", "."]);
    write(&fx.root, "o.txt", "o\n");
    commit_all(&fx.root, "other");
    let head = fx.sha("HEAD");
    git(&fx.root, &["checkout", "-q", "main"]);
    expect(
        unchanged(&fx, || fx.run("main", &head)),
        "merge_conflict",
        &[ENTRY_UNRELATED_HISTORIES],
    );
}

#[test]
fn a_merge_tree_failure_other_than_a_conflict_is_an_error() {
    let fx = Fx::new(true);
    git(&fx.root, &["config", "gc.auto", "0"]);
    write(&fx.root, "a.txt", "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n");
    commit_all(&fx.root, "lines");
    git(&fx.root, &["checkout", "-q", "-b", "topic"]);
    write(&fx.root, "a.txt", "one\n2\n3\n4\n5\n6\n7\n8\n9\n10\n");
    commit_all(&fx.root, "topic");
    let head = fx.sha("HEAD");
    git(&fx.root, &["checkout", "-q", "main"]);
    write(&fx.root, "a.txt", "1\n2\n3\n4\n5\n6\n7\n8\n9\nten\n");
    commit_all(&fx.root, "main");
    // Remove the loose blob of the topic version, so merge-tree cannot read it.
    let blob = git_out(&fx.root, &["rev-parse", &format!("{head}:a.txt")]);
    let loose = fx
        .root
        .join(".git/objects")
        .join(&blob[..2])
        .join(&blob[2..]);
    std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::remove_file(&loose).unwrap();
    let error = fx
        .repo()
        .root_merge_preflight("main", &head, LONG)
        .unwrap_err();
    assert_eq!(code_of(error), "git_unavailable");
}

// ------------------------------------------------------------------------------------------------ ready

#[test]
fn ready_names_the_head_and_leaves_the_checkout_alone() {
    let fx = Fx::new(true);
    let head = fx.side_branch("topic", "t.txt", "t\n");
    write(&fx.root, "m.txt", "m\n");
    commit_all(&fx.root, "main");
    let old = fx.sha("HEAD");
    let ready = unchanged(&fx, || fx.run("main", &head));
    assert_eq!(ready, Preflight::Ready { old_head: old });
}

#[test]
fn ready_with_an_identity_only_in_the_global_config() {
    let fx = Fx::new(false);
    let head = fx.side_branch("topic", "t.txt", "t\n");
    let home = fx.tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join(".gitconfig"),
        "[user]\n\tname = G\n\temail = g@g\n",
    )
    .unwrap();
    let old = fx.sha("HEAD");
    assert_eq!(fx.run("main", &head), Preflight::Ready { old_head: old });
}

// ------------------------------------------------------------------------------------------------ order

/// A repository that fails the steps `broken` names, for the adjacent pairs.
struct Order {
    fx: Fx,
    head: String,
}

impl Order {
    /// Fails every step 0 to 6 and 9 at once: an unsafe config, a bad commit and branch name, HEAD on another
    /// branch, no identity; a conflict is checked in its own test.
    fn new() -> Order {
        let fx = Fx::new(false);
        let head = fx.side_branch("topic", "t.txt", "t\n");
        Order { fx, head }
    }
}

#[test]
fn step_0_comes_before_step_1() {
    let o = Order::new();
    git(&o.fx.root, &["config", "--local", "merge.x.driver", "true"]);
    let repo = o.fx.repo_with(GitSeams {
        force_old_git: true,
        ..GitSeams::default()
    });
    let (code, _, _) = refusal(repo.root_merge_preflight("main", &o.head, LONG).unwrap());
    assert_eq!(code, "git_too_old");
}

#[test]
fn step_1_comes_before_step_2() {
    let o = Order::new();
    git(&o.fx.root, &["config", "--local", "merge.x.driver", "true"]);
    let (code, _, _) = refusal(o.fx.run("main", "nonsense"));
    assert_eq!(code, "unsafe_config");
}

#[test]
fn step_2_comes_before_step_3() {
    let o = Order::new();
    let (code, _, _) = refusal(o.fx.run("-x", "nonsense"));
    assert_eq!(code, "invalid_commit");
}

#[test]
fn step_3_comes_before_step_4() {
    let o = Order::new();
    git(&o.fx.root, &["checkout", "-q", "topic"]);
    let (code, _, _) = refusal(o.fx.run("-x", &o.head));
    assert_eq!(code, "invalid_branch");
}

#[test]
fn step_4_comes_before_step_5() {
    let o = Order::new();
    git(&o.fx.root, &["checkout", "-q", "topic"]);
    // topic is in the HEAD of topic, but main is asked for.
    let (code, _, _) = refusal(o.fx.run("main", &o.head));
    assert_eq!(code, "wrong_branch");
}

#[test]
fn step_5_comes_before_step_6() {
    let o = Order::new();
    let head = o.fx.sha("HEAD");
    assert_eq!(o.fx.run("main", &head), Preflight::AlreadyInHead);
}

#[test]
fn step_6_comes_before_step_9() {
    let fx = Fx::new(false);
    git(&fx.root, &["checkout", "-q", "--orphan", "other"]);
    git(&fx.root, &["rm", "-rfq", "."]);
    write(&fx.root, "o.txt", "o\n");
    commit_all(&fx.root, "other");
    let head = fx.sha("HEAD");
    git(&fx.root, &["checkout", "-q", "main"]);
    let (code, _, _) = refusal(fx.run("main", &head));
    assert_eq!(code, "no_identity");
}

// ------------------------------------------------------------------------------------------------ seams

#[test]
fn a_slow_config_read_ends_with_the_timeout_in_time() {
    let fx = Fx::new(true);
    let head = fx.side_branch("topic", "t.txt", "t\n");
    let (repo, _) = wrapped(&fx, "  *\" config \"*) sleep 5; exit 0;;", true);
    let started = Instant::now();
    let error = repo
        .root_merge_preflight("main", &head, Duration::from_secs(1))
        .unwrap_err();
    assert_eq!(code_of(error), "timeout");
    assert!(started.elapsed() < Duration::from_secs(4));
}

#[test]
fn a_spent_deadline_without_a_cached_version_is_the_timeout_at_step_0() {
    let fx = Fx::new(true);
    let head = fx.side_branch("topic", "t.txt", "t\n");
    let error = fx
        .repo()
        .root_merge_preflight("main", &head, Duration::ZERO)
        .unwrap_err();
    assert_eq!(code_of(error), "timeout");
}

#[test]
fn an_exit_of_128_is_git_unavailable() {
    let fx = Fx::new(true);
    let head = fx.side_branch("topic", "t.txt", "t\n");
    for answers in [
        "  *\" merge-base --is-ancestor \"*) exit 128;;",
        "  *\" symbolic-ref -q HEAD \"*) exit 128;;",
    ] {
        let (repo, _) = wrapped(&fx, answers, true);
        let error = repo.root_merge_preflight("main", &head, LONG).unwrap_err();
        assert_eq!(code_of(error), "git_unavailable", "{answers}");
    }
}

#[test]
fn every_repository_child_of_a_ready_run_is_read_only() {
    let fx = Fx::new(true);
    let head = fx.side_branch("topic", "t.txt", "t\n");
    let (repo, log) = wrapped(&fx, "", false);
    let ready = repo.root_merge_preflight("main", &head, LONG).unwrap();
    assert!(matches!(ready, Preflight::Ready { .. }), "{ready:?}");
    let text = std::fs::read_to_string(&log).unwrap();
    let mut seen_config = false;
    for line in text.lines() {
        let (locks, args) = line.split_once('|').unwrap();
        if args.contains("--version") || args.contains("config --get-all") {
            continue;
        }
        seen_config |= args.contains("config --local");
        assert_eq!(locks, "0", "{line}");
    }
    assert!(seen_config);
    assert!(text.lines().any(|l| l.contains("rev-parse --git-dir")));
}

#[test]
fn merge_target_reads_origin_head_through_the_pins() {
    let fx = Fx::new(true);
    assert_eq!(fx.repo().merge_target(None).unwrap(), None);
    git(&fx.root, &["update-ref", "refs/remotes/origin/-x", "HEAD"]);
    git(
        &fx.root,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/-x",
        ],
    );
    let (repo, log) = wrapped(&fx, "", true);
    assert_eq!(repo.merge_target(None).unwrap(), None);
    let text = std::fs::read_to_string(&log).unwrap();
    let line = text.lines().find(|l| l.contains("symbolic-ref")).unwrap();
    assert!(line.starts_with("0|"), "{line}");
    assert!(line.contains("--no-replace-objects"), "{line}");
    assert!(line.contains("protocol.ext.allow=never"), "{line}");

    git(&fx.root, &["update-ref", "refs/remotes/origin/dev", "HEAD"]);
    git(
        &fx.root,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/dev",
        ],
    );
    assert_eq!(fx.repo().merge_target(None).unwrap(), Some("dev".into()));
}
