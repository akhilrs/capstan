//! The user-mode git runner (`GitRepo::root_git`): the child environment, the pins, the refusals before spawning, the
//! caps and timeouts, the version probe and the source scans that keep the seams and the spawn in one place.
//!
//! Every test pins `forward_env` (a temporary HOME, no system config), so the host's configuration never changes a
//! result.

use capstan_kernel::integrate::{
    GitRepo, GitSeams, IntegrateError, RootCall, FIXED_CHILD_ENV, FORWARDED_USER_ENV,
    ROOT_CONFIG_OVERRIDES,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn env_pairs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn host_path() -> String {
    std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into())
}

fn real_git() -> PathBuf {
    std::env::split_paths(&host_path())
        .map(|dir| dir.join("git"))
        .find(|p| p.is_file())
        .expect("git on PATH")
}

/// Runs the host git with an empty environment apart from a temporary HOME.
fn plain_git(root: &Path, home: &Path, args: &[&str]) -> String {
    let out = Command::new(real_git())
        .arg("-C")
        .arg(root)
        .args(args)
        .env_clear()
        .env("PATH", host_path())
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .stdin(Stdio::null())
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

struct Fixture {
    _dir: TempDir,
    root: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("repo");
        let home = dir.path().join("home");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&home).unwrap();
        plain_git(&root, &home, &["init", "-q", "-b", "main"]);
        plain_git(
            &root,
            &home,
            &["commit", "-q", "--allow-empty", "-m", "first"],
        );
        Fixture {
            _dir: dir,
            root,
            home,
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self._dir.path().join(name)
    }

    fn forward(&self) -> Vec<(String, String)> {
        vec![
            ("HOME".into(), self.home.to_string_lossy().into_owned()),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
        ]
    }

    fn seams(&self) -> GitSeams {
        GitSeams {
            forward_env: Some(self.forward()),
            ..GitSeams::default()
        }
    }

    fn repo(&self) -> GitRepo {
        GitRepo::new(&self.root).with_seams(self.seams())
    }

    /// A script that stands for git: `body` runs in sh.
    fn script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.path(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// A fake git that logs each call to `log` and runs the real git; `version` answers `--version`.
    fn counting(&self, name: &str, log: &Path, version: &str) -> PathBuf {
        self.script(
            name,
            &format!(
                "echo \"$0 $1\" >> '{}'\nif [ \"$1\" = --version ]; then echo '{version}'; exit 0; fi\nexec '{}' \"$@\"",
                log.display(),
                real_git().display()
            ),
        )
    }
}

fn call(args: &[&str]) -> RootCall {
    RootCall {
        args: args.iter().map(|a| a.to_string()).collect(),
        timeout: Duration::from_secs(20),
        config_env: Vec::new(),
        network: false,
        user_config: true,
    }
}

fn code_of(error: IntegrateError) -> String {
    match error {
        IntegrateError::Integration { code, .. } => code,
        other => panic!("not an integration error: {}", other.display()),
    }
}

fn stdout_text(repo: &GitRepo, c: &RootCall) -> String {
    let out = repo
        .root_git(c)
        .unwrap_or_else(|e| panic!("{}", e.display()));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn alias_call(alias: &str) -> RootCall {
    let mut c = call(&["t"]);
    c.config_env = env_pairs(&[("alias.t", alias)]);
    c
}

// ------------------------------------------------------------------------------------------ environment

#[test]
fn the_child_environment_is_the_contract() {
    let fx = Fixture::new();
    let mut forward = fx.forward();
    forward.extend(env_pairs(&[
        ("GIT_SSH_VARIANT", "MARKER_variant"),
        ("GIT_CURL_VERBOSE", "MARKER_curl"),
        ("GIT_SSH", "MARKER_ssh"),
        ("GIT_SSH_COMMAND", "MARKER_sshc"),
        ("GIT_PROXY_COMMAND", "MARKER_proxy"),
        ("GIT_EXTERNAL_DIFF", "MARKER_diff"),
        ("GIT_ASKPASS", "MARKER_ask"),
        ("SSH_ASKPASS", "MARKER_sask"),
        ("GIT_SSL_NO_VERIFY", "MARKER_ssl"),
        ("GIT_SSL_CAINFO", "MARKER_cainfo"),
        ("GIT_CONFIG_PARAMETERS", "MARKER_params"),
        ("LANG", "de_DE.UTF-8"),
        ("LC_MESSAGES", "de_DE.UTF-8"),
    ]));
    let repo = GitRepo::new(&fx.root).with_seams(GitSeams {
        forward_env: Some(forward.clone()),
        ..GitSeams::default()
    });
    let text = stdout_text(&repo, &alias_call("!env"));
    let names: Vec<&str> = text.lines().filter_map(|l| l.split('=').next()).collect();

    // What git itself sets for an alias child, on this host's version.
    let mut baseline = Command::new(real_git());
    baseline.arg("--no-replace-objects").arg("-C").arg(&fx.root);
    for (key, value) in ROOT_CONFIG_OVERRIDES {
        baseline.arg("-c").arg(format!("{key}={value}"));
    }
    let base = baseline
        .arg("t")
        .env_clear()
        .env("PATH", host_path())
        .env("HOME", &fx.home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "alias.t")
        .env("GIT_CONFIG_VALUE_0", "!env")
        .stdin(Stdio::null())
        .output()
        .expect("baseline runs");
    let base_text = String::from_utf8_lossy(&base.stdout).into_owned();
    let base_names: Vec<&str> = base_text
        .lines()
        .filter_map(|l| l.split('=').next())
        .collect();

    let given: Vec<&str> = forward.iter().map(|(k, _)| k.as_str()).collect();
    for name in &names {
        let ok = FIXED_CHILD_ENV.contains(name)
            || (FORWARDED_USER_ENV.contains(name) && given.contains(name))
            || ["PWD", "OLDPWD", "SHLVL", "_"].contains(name)
            || base_names.contains(name);
        assert!(ok, "unexpected name {name} in the child environment");
    }
    assert!(
        !text.contains("MARKER_"),
        "a marker value reached the child:\n{text}"
    );
    assert!(text.lines().any(|l| l == "LC_ALL=C"));
    assert!(!names.contains(&"LANG") && !names.contains(&"LC_MESSAGES"));
}

#[test]
fn without_user_config_nothing_of_the_users_is_forwarded() {
    let fx = Fixture::new();
    let mut forward = fx.forward();
    forward.push(("SSH_AUTH_SOCK".into(), "MARKER_sock".into()));
    let repo = GitRepo::new(&fx.root).with_seams(GitSeams {
        forward_env: Some(forward),
        ..GitSeams::default()
    });
    let mut c = alias_call("!env");
    c.user_config = false;
    let text = stdout_text(&repo, &c);
    assert!(!text.contains("MARKER_"));
    assert!(!text.lines().any(|l| l.starts_with("HOME=")));
    assert!(text.lines().any(|l| l == "GIT_CONFIG_NOSYSTEM=1"));
    assert!(text.lines().any(|l| l == "GIT_CONFIG_GLOBAL=/dev/null"));
}

#[test]
fn user_config_false_ignores_global_and_system_config_and_true_reads_both() {
    let fx = Fixture::new();
    fs::write(fx.home.join(".gitconfig"), "[capstan]\n\tglobalmark = g\n").unwrap();
    let system = fx.path("system.cfg");
    fs::write(&system, "[capstan]\n\tsystemmark = s\n").unwrap();
    let repo = GitRepo::new(&fx.root).with_seams(GitSeams {
        forward_env: Some(env_pairs(&[
            ("HOME", &fx.home.to_string_lossy()),
            ("GIT_CONFIG_SYSTEM", &system.to_string_lossy()),
        ])),
        ..GitSeams::default()
    });
    for (key, value) in [("capstan.globalmark", "g"), ("capstan.systemmark", "s")] {
        let mut c = call(&["config", "--get", key]);
        assert_eq!(stdout_text(&repo, &c).trim(), value);
        c.user_config = false;
        let out = repo.root_git(&c).unwrap();
        assert_eq!(out.code, 1);
        assert!(out.stdout.is_empty());
    }
}

// ------------------------------------------------------------------------------------------ pins

#[test]
fn the_pins_win_over_local_and_global_config() {
    let fx = Fixture::new();
    let mut global = String::new();
    for (key, _) in ROOT_CONFIG_OVERRIDES {
        let (section, rest) = key.split_once('.').unwrap();
        let (sub, var) = match rest.rsplit_once('.') {
            Some((s, v)) => (format!(" \"{s}\""), v),
            None => (String::new(), rest),
        };
        global.push_str(&format!("[{section}{sub}]\n\t{var} = global-value\n"));
        plain_git(
            &fx.root,
            &fx.home,
            &["config", "--local", "--add", key, "local-value"],
        );
    }
    fs::write(fx.home.join(".gitconfig"), global).unwrap();
    let repo = fx.repo();
    for (key, pinned) in ROOT_CONFIG_OVERRIDES {
        let out = stdout_text(&repo, &call(&["config", "--get", key]));
        assert_eq!(out.trim_end_matches('\n'), *pinned, "{key}");
    }
}

#[test]
fn a_local_ext_remote_and_hooks_do_not_run() {
    let fx = Fixture::new();
    let marker = fx.path("ext-marker");
    plain_git(
        &fx.root,
        &fx.home,
        &["config", "protocol.ext.allow", "always"],
    );
    fs::write(
        fx.home.join(".gitconfig"),
        "[protocol \"ext\"]\n\tallow = always\n",
    )
    .unwrap();
    let repo = fx.repo();
    let mut c = call(&["ls-remote", &format!("ext::touch {}", marker.display())]);
    c.network = true;
    let _ = repo.root_git(&c);
    assert!(!marker.exists(), "the ext command ran");

    // Hooks: a pre-push hook in .git/hooks and one under a local core.hooksPath.
    let bare = fx.path("remote.git");
    plain_git(
        &fx.root,
        &fx.home,
        &["init", "-q", "--bare", bare.to_str().unwrap()],
    );
    let hook_marker = fx.path("hook-marker");
    let hook = format!("#!/bin/sh\ntouch '{}'\n", hook_marker.display());
    let hooks = fx.root.join(".git/hooks/pre-push");
    fs::write(&hooks, &hook).unwrap();
    fs::set_permissions(&hooks, fs::Permissions::from_mode(0o755)).unwrap();
    let other = fx.path("hooks");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("pre-push"), &hook).unwrap();
    fs::set_permissions(other.join("pre-push"), fs::Permissions::from_mode(0o755)).unwrap();
    plain_git(
        &fx.root,
        &fx.home,
        &["config", "core.hooksPath", other.to_str().unwrap()],
    );
    let mut push = call(&[
        "push",
        &format!("file://{}", bare.display()),
        "HEAD:refs/heads/main",
    ]);
    push.network = true;
    let out = repo.root_git(&push).unwrap();
    assert_eq!(out.code, 0, "{}", String::from_utf8_lossy(&out.stderr));
    assert!(!hook_marker.exists(), "a hook ran");
}

// ------------------------------------------------------------------------------------------ refusals

#[test]
fn refusals_happen_before_spawning() {
    let fx = Fixture::new();
    let log = fx.path("calls.log");
    let fake = fx.counting("fake-git", &log, "git version 2.40.0");
    let repo = GitRepo::new(&fx.root).with_seams(GitSeams {
        git_program: Some(fake),
        ..fx.seams()
    });

    let mut cases: Vec<RootCall> = Vec::new();
    for key in [
        "core.hooksPath",
        "CORE.HooksPath",
        "Core.SSHCommand",
        "protocol.ext.allow",
        "PROTOCOL.ext.ALLOW",
    ] {
        let mut c = call(&["config", "--list"]);
        c.config_env = env_pairs(&[(key, "x")]);
        cases.push(c);
    }
    for args in [
        &["config", "-c", "x=y"][..],
        &["config", "--config-env=a=b"],
        &["config", "--exec-path=/tmp"],
        &["config", "--upload-pack=x"],
        &["config", "-C", "/tmp"],
        &["ls-remote", "--upl=x", "."],
        &["ls-remote", "--rec=x", "."],
        &["ls-remote", "--exe=x", "."],
        &["ls-remote", "-u", "x", "."],
        &["ls-remote", "-ux", "."],
        &["config", "--git-dir=/tmp"],
        &["config", "a\0b"],
        &["config", "a\nb"],
        &[],
    ] {
        cases.push(call(args));
    }
    let mut newline = call(&["config", "--list"]);
    newline.config_env = env_pairs(&[("a.b", "x\ny")]);
    cases.push(newline);
    let mut no_user = call(&["push", "origin"]);
    no_user.network = true;
    no_user.user_config = false;
    cases.push(no_user);
    let mut push_offline = call(&["push", "origin"]);
    push_offline.network = false;
    cases.push(push_offline);
    let mut ls_offline = call(&["ls-remote", "origin"]);
    ls_offline.network = false;
    cases.push(ls_offline);
    let mut rev_network = call(&["rev-parse", "HEAD"]);
    rev_network.network = true;
    cases.push(rev_network);

    for c in &cases {
        let error = repo
            .root_git(c)
            .expect_err(&format!("{:?} was not refused", c.args));
        assert_eq!(code_of(error), "git_unavailable", "{:?}", c.args);
    }
    assert!(
        !log.exists(),
        "the fake git was run: {}",
        fs::read_to_string(&log).unwrap_or_default()
    );

    // Options that only look similar pass.
    let ok = repo
        .root_git(&call(&["rev-parse", "--git-common-dir"]))
        .unwrap();
    assert_eq!(ok.code, 0);
}

#[test]
fn a_missing_git_program_is_unavailable() {
    let fx = Fixture::new();
    let repo = GitRepo::new(&fx.root).with_seams(GitSeams {
        git_program: Some(fx.path("no-such-git")),
        ..fx.seams()
    });
    let error = repo.root_git(&call(&["rev-parse", "HEAD"])).unwrap_err();
    assert_eq!(code_of(error), "git_unavailable");
}

#[test]
fn the_version_probe_accepts_2_39_and_refuses_2_38() {
    let fx = Fixture::new();
    let log = fx.path("v.log");
    for (version, pass) in [
        ("git version 2.39.0", true),
        ("git version 2.39.5 (Apple Git-154)", true),
        ("git version 2.43.0.windows.1", true),
        ("git version 2.38.9", false),
    ] {
        let fake = fx.counting("vgit", &log, version);
        let repo = GitRepo::new(&fx.root).with_seams(GitSeams {
            git_program: Some(fake),
            ..fx.seams()
        });
        let result = repo.root_git(&call(&["rev-parse", "HEAD"]));
        if pass {
            result.unwrap_or_else(|e| panic!("{version}: {}", e.display()));
        } else {
            assert_eq!(code_of(result.unwrap_err()), "git_unavailable", "{version}");
        }
    }
}

#[test]
fn the_version_probe_runs_once_per_repository_and_its_clones() {
    let fx = Fixture::new();
    let log = fx.path("once.log");
    let fake = fx.counting("ogit", &log, "git version 2.40.0");
    let repo = GitRepo::new(&fx.root).with_seams(GitSeams {
        git_program: Some(fake),
        ..fx.seams()
    });
    let clone = repo.clone();
    for r in [&repo, &clone, &repo] {
        r.root_git(&call(&["rev-parse", "HEAD"])).unwrap();
    }
    let text = fs::read_to_string(&log).unwrap();
    assert_eq!(
        text.lines().filter(|l| l.ends_with(" --version")).count(),
        1,
        "{text}"
    );
}

#[test]
fn force_old_git_only_affects_the_repository_it_is_set_on() {
    let fx = Fixture::new();
    let repo = fx.repo();
    repo.root_git(&call(&["rev-parse", "HEAD"])).unwrap();
    let old = repo.clone().with_seams(GitSeams {
        force_old_git: true,
        ..fx.seams()
    });
    assert_eq!(
        code_of(old.root_git(&call(&["rev-parse", "HEAD"])).unwrap_err()),
        "git_unavailable"
    );
    repo.root_git(&call(&["rev-parse", "HEAD"])).unwrap();
}

#[test]
fn git_is_resolved_once_to_an_absolute_path() {
    let fx = Fixture::new();
    let bin = fx.path("bin");
    fs::create_dir_all(&bin).unwrap();
    let log = fx.path("paths.log");
    let fake = bin.join("git");
    fs::write(
        &fake,
        format!(
            "#!/bin/sh\necho \"$0\" >> '{}'\nexec '{}' \"$@\"\n",
            log.display(),
            real_git().display()
        ),
    )
    .unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    let repo = GitRepo::new(&fx.root).with_seams(GitSeams {
        child_path: Some(bin.to_string_lossy().into_owned()),
        skip_version_probe: true,
        ..fx.seams()
    });
    repo.root_git(&call(&["rev-parse", "HEAD"])).unwrap();
    repo.root_git(&call(&["rev-parse", "HEAD"])).unwrap();
    let text = fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0], lines[1]);
    assert!(Path::new(lines[0]).is_absolute());
}

// ------------------------------------------------------------------------------------------ caps

fn flood_repo(fx: &Fixture, body: &str) -> GitRepo {
    let fake = fx.script("flood", body);
    GitRepo::new(&fx.root).with_seams(GitSeams {
        git_program: Some(fake),
        skip_version_probe: true,
        reader_detach: Some(Duration::from_millis(200)),
        ..fx.seams()
    })
}

#[test]
fn the_stdout_cap_is_16_mib_inclusive() {
    let fx = Fixture::new();
    let cap = 16 * 1024 * 1024;
    let exact = flood_repo(&fx, &format!("head -c {cap} /dev/zero"));
    let out = exact.root_git(&call(&["rev-parse", "HEAD"])).unwrap();
    assert_eq!(out.stdout.len(), cap);
    let over = flood_repo(&fx, &format!("head -c {} /dev/zero", cap + 1));
    let error = over.root_git(&call(&["rev-parse", "HEAD"])).unwrap_err();
    assert_eq!(code_of(error), "output_too_large");
}

#[test]
fn the_stderr_is_cut_to_64_kib_without_blocking_the_child() {
    let fx = Fixture::new();
    let repo = flood_repo(&fx, "head -c 1048576 /dev/zero >&2");
    let out = repo.root_git(&call(&["rev-parse", "HEAD"])).unwrap();
    assert_eq!(out.stderr.len(), 64 * 1024);
}

#[test]
fn a_child_past_the_timeout_gives_timeout() {
    let fx = Fixture::new();
    let repo = flood_repo(&fx, "exec sleep 30");
    let mut c = call(&["rev-parse", "HEAD"]);
    c.timeout = Duration::from_secs(1);
    let started = Instant::now();
    let error = repo.root_git(&c).unwrap_err();
    assert_eq!(code_of(error), "timeout");
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn a_probe_that_hits_the_deadline_gives_timeout() {
    let fx = Fixture::new();
    let fake = fx.script(
        "slow-version",
        "if [ \"$1\" = --version ]; then exec sleep 30; fi\nexit 0",
    );
    let repo = GitRepo::new(&fx.root).with_seams(GitSeams {
        git_program: Some(fake),
        probe_timeout: Some(Duration::from_millis(300)),
        reader_detach: Some(Duration::from_millis(200)),
        ..fx.seams()
    });
    let started = Instant::now();
    assert_eq!(
        code_of(repo.root_git(&call(&["rev-parse", "HEAD"])).unwrap_err()),
        "timeout"
    );
    assert!(started.elapsed() < Duration::from_secs(10));
    let mut spent = call(&["rev-parse", "HEAD"]);
    spent.timeout = Duration::ZERO;
    assert_eq!(code_of(fx.repo().root_git(&spent).unwrap_err()), "timeout");
}

// ------------------------------------------------------------------------------------------ source scans

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap().flatten() {
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

/// The one file outside the tests directories that may call the seam setters: the decide-merge route's repository builder.
const SEAM_CALLER: &str = "daemon/src/handlers/decide_merge_seams.rs";

/// The 1-based lines of `text` in `file` that call a seam setter; the definitions do not count, and neither does the
/// one allowed file.
fn setter_call_lines(file: &Path, text: &str) -> Vec<usize> {
    if file.ends_with(SEAM_CALLER) {
        return Vec::new();
    }
    text.lines()
        .enumerate()
        .filter(|(_, line)| {
            let defines = line.contains("fn with_seams") || line.contains("fn with_merge_seams");
            !defines && (line.contains("with_seams(") || line.contains("with_merge_seams("))
        })
        .map(|(index, _)| index + 1)
        .collect()
}

#[test]
fn the_setter_matcher_allows_one_file_and_no_other() {
    let call = "let r = repo.with_seams(seams);\n";
    let allowed = Path::new("/x/rust/crates/daemon/src/handlers/decide_merge_seams.rs");
    let other = Path::new("/x/rust/crates/daemon/src/handlers/decide_merge.rs");
    assert!(setter_call_lines(allowed, call).is_empty());
    assert_eq!(setter_call_lines(other, call), vec![1]);
    assert_eq!(
        setter_call_lines(other, "x\n.with_merge_seams(s)\n"),
        vec![2]
    );
    assert!(setter_call_lines(other, "pub fn with_seams(mut self) -> Self {").is_empty());
    assert_eq!(
        setter_call_lines(Path::new("kernel/src/decide_merge_seams.rs"), call),
        vec![1]
    );
}

#[test]
fn seam_setters_are_called_only_from_tests() {
    let crates = crate_dir().join("..");
    let mut files = Vec::new();
    rust_files(&crates, &mut files);
    assert!(!files.is_empty());
    let mut hits = Vec::new();
    for file in files {
        let text = fs::read_to_string(&file).unwrap();
        for line in setter_call_lines(&file, &text) {
            hits.push(format!("{}:{line}", file.display()));
        }
    }
    assert!(
        hits.is_empty(),
        "seam setters called outside tests: {hits:?}"
    );
}

/// The text of the fn whose signature holds `fn <name>(`, up to the closing brace at the signature's indentation.
fn fn_body(source: &str, name: &str) -> Option<String> {
    let lines: Vec<&str> = source.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.contains(&format!("fn {name}(")) && !l.trim_start().starts_with("//"))?;
    let indent: String = lines[start].chars().take_while(|c| *c == ' ').collect();
    let close = format!("{indent}}}");
    let end = lines[start + 1..].iter().position(|l| *l == close)? + start + 1;
    Some(lines[start..=end].join("\n"))
}

#[test]
fn only_spawn_capped_spawns() {
    let source = fs::read_to_string(crate_dir().join("src/integrate.rs")).unwrap();
    let spawner = fn_body(&source, "spawn_capped").expect("spawn_capped exists");
    assert!(spawner.contains(".spawn()"));
    for name in [
        "run_root",
        "check_version",
        "ssh_probe",
        "ssh_probes",
        "probe_ssh",
    ] {
        if let Some(body) = fn_body(&source, name) {
            assert!(!body.contains(".spawn("), "{name} spawns");
        }
    }
    assert!(fn_body(&source, "run_root").is_some());
    assert!(fn_body(&source, "check_version").is_some());
}
