//! The ssh probe of a network `run_root` call: which `core.sshCommand` reaches the transport. A fake ssh first on the
//! child's PATH records its argv and exits 255, so no connection is made.

use capstan_kernel::integrate::{GitRepo, GitSeams, IntegrateError, RootCall};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use tempfile::TempDir;

fn host_path() -> String {
    std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into())
}

fn real_git() -> PathBuf {
    std::env::split_paths(&host_path())
        .map(|dir| dir.join("git"))
        .find(|p| p.is_file())
        .expect("git on PATH")
}

fn plain_git(root: &Path, home: &Path, args: &[&str]) {
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
}

struct Fixture {
    dir: TempDir,
    root: PathBuf,
    home: PathBuf,
    system: PathBuf,
    ssh_log: PathBuf,
    bin: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("repo");
        let home = dir.path().join("home");
        let bin = dir.path().join("bin");
        for d in [&root, &home, &bin] {
            fs::create_dir_all(d).unwrap();
        }
        plain_git(&root, &home, &["init", "-q", "-b", "main"]);
        let system = dir.path().join("system.conf");
        fs::write(&system, "").unwrap();
        let ssh_log = dir.path().join("ssh.log");
        let fx = Fixture {
            dir,
            root,
            home,
            system,
            ssh_log,
            bin,
        };
        fx.script(
            "ssh",
            &format!(
                "for a in \"$@\"; do echo \"$a\"; done > '{}'\nexit 255",
                fx.ssh_log.display()
            ),
        );
        fx
    }

    fn script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.bin.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn global(&self, text: &str) {
        fs::write(self.home.join(".gitconfig"), text).unwrap();
    }

    fn forward(&self) -> Vec<(String, String)> {
        vec![
            ("HOME".into(), self.home.to_string_lossy().into_owned()),
            (
                "GIT_CONFIG_SYSTEM".into(),
                self.system.to_string_lossy().into_owned(),
            ),
        ]
    }

    fn seams(&self) -> GitSeams {
        GitSeams {
            forward_env: Some(self.forward()),
            child_path: Some(format!("{}:{}", self.bin.display(), host_path())),
            ..GitSeams::default()
        }
    }

    fn repo(&self) -> GitRepo {
        GitRepo::new(&self.root).with_seams(self.seams())
    }

    /// Runs the network call and returns what the fake ssh was given (empty when it never ran).
    fn argv(&self, repo: &GitRepo) -> Vec<String> {
        let _ = fs::remove_file(&self.ssh_log);
        let _ = repo.root_git(&ls_remote());
        fs::read_to_string(&self.ssh_log)
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }
}

fn ls_remote() -> RootCall {
    RootCall {
        args: vec!["ls-remote".into(), "ssh://capstan.invalid/r.git".into()],
        timeout: Duration::from_secs(20),
        config_env: Vec::new(),
        network: true,
        user_config: true,
    }
}

fn code_of(error: IntegrateError) -> String {
    match error {
        IntegrateError::Integration { code, .. } => code,
        other => panic!("not an integration error: {}", other.display()),
    }
}

fn has(argv: &[String], item: &str) -> bool {
    argv.iter().any(|a| a == item)
}

#[test]
fn batch_mode_is_added_when_nothing_is_set() {
    let fx = Fixture::new();
    let argv = fx.argv(&fx.repo());
    assert!(has(&argv, "BatchMode=yes"), "{argv:?}");
}

#[test]
fn a_global_value_is_used_verbatim() {
    let fx = Fixture::new();
    fx.global("[core]\n\tsshCommand = ssh -o Marker=global\n");
    let argv = fx.argv(&fx.repo());
    assert!(has(&argv, "Marker=global"), "{argv:?}");
    assert!(!has(&argv, "BatchMode=yes"), "{argv:?}");
}

#[test]
fn a_system_only_value_is_used() {
    let fx = Fixture::new();
    fs::write(&fx.system, "[core]\n\tsshCommand = ssh -o Marker=system\n").unwrap();
    let argv = fx.argv(&fx.repo());
    assert!(has(&argv, "Marker=system"), "{argv:?}");
}

#[test]
fn the_global_value_wins_over_the_system_one() {
    let fx = Fixture::new();
    fs::write(&fx.system, "[core]\n\tsshCommand = ssh -o Marker=system\n").unwrap();
    fx.global("[core]\n\tsshCommand = ssh -o Marker=global\n");
    let argv = fx.argv(&fx.repo());
    assert!(has(&argv, "Marker=global"), "{argv:?}");
    assert!(!has(&argv, "Marker=system"), "{argv:?}");
}

#[test]
fn the_last_of_several_global_values_is_used() {
    let fx = Fixture::new();
    fx.global("[core]\n\tsshCommand = ssh -o Marker=first\n\tsshCommand = ssh -o Marker=second\n");
    let argv = fx.argv(&fx.repo());
    assert!(has(&argv, "Marker=second"), "{argv:?}");
}

#[test]
fn an_included_value_is_found() {
    let fx = Fixture::new();
    fs::write(
        fx.home.join("extra.conf"),
        "[core]\n\tsshCommand = ssh -o Marker=included\n",
    )
    .unwrap();
    fx.global("[include]\n\tpath = extra.conf\n");
    let argv = fx.argv(&fx.repo());
    assert!(has(&argv, "Marker=included"), "{argv:?}");
}

#[test]
fn an_include_if_gitdir_value_is_found() {
    let fx = Fixture::new();
    fs::write(
        fx.home.join("extra.conf"),
        "[core]\n\tsshCommand = ssh -o Marker=gitdir\n",
    )
    .unwrap();
    fx.global(&format!(
        "[includeIf \"gitdir:{}/\"]\n\tpath = extra.conf\n",
        fx.dir.path().display()
    ));
    let argv = fx.argv(&fx.repo());
    assert!(has(&argv, "Marker=gitdir"), "{argv:?}");
}

#[test]
fn an_unborn_repository_matches_include_if_gitdir() {
    let fx = Fixture::new();
    // `new` has no commit already; the include rule names this very repository.
    fs::write(
        fx.home.join("extra.conf"),
        "[core]\n\tsshCommand = ssh -o Marker=unborn\n",
    )
    .unwrap();
    fx.global(&format!(
        "[includeIf \"gitdir:{}/.git\"]\n\tpath = extra.conf\n",
        fx.root.display()
    ));
    let argv = fx.argv(&fx.repo());
    assert!(has(&argv, "Marker=unborn"), "{argv:?}");
}

#[test]
fn a_local_value_never_runs() {
    let fx = Fixture::new();
    let marker = fx.dir.path().join("local-ran");
    let local = format!("touch {}; ssh", marker.display());
    plain_git(&fx.root, &fx.home, &["config", "core.sshCommand", &local]);
    let argv = fx.argv(&fx.repo());
    assert!(has(&argv, "BatchMode=yes"), "{argv:?}");
    assert!(!marker.exists());

    fx.global("[core]\n\tsshCommand = ssh -o Marker=global\n");
    let argv = fx.argv(&fx.repo());
    assert!(has(&argv, "Marker=global"), "{argv:?}");
    assert!(!marker.exists());
}

#[test]
fn a_value_with_a_newline_is_unavailable() {
    let fx = Fixture::new();
    fx.global("[core]\n\tsshCommand = \"ssh \\n -o X\"\n");
    let error = fx.repo().root_git(&ls_remote()).unwrap_err();
    assert_eq!(code_of(error), "git_unavailable");
    assert!(!fx.ssh_log.exists());
}

#[test]
fn a_changed_global_value_is_seen_by_the_next_call() {
    let fx = Fixture::new();
    let repo = fx.repo();
    fx.global("[core]\n\tsshCommand = ssh -o Marker=one\n");
    assert!(has(&fx.argv(&repo), "Marker=one"));
    fx.global("[core]\n\tsshCommand = ssh -o Marker=two\n");
    let argv = fx.argv(&repo);
    assert!(has(&argv, "Marker=two"), "{argv:?}");
}

#[test]
fn without_a_global_location_the_call_is_unavailable() {
    let fx = Fixture::new();
    let mut seams = fx.seams();
    seams.forward_env = Some(vec![(
        "GIT_CONFIG_SYSTEM".into(),
        fx.system.to_string_lossy().into_owned(),
    )]);
    let repo = GitRepo::new(&fx.root).with_seams(seams);
    let error = repo.root_git(&ls_remote()).unwrap_err();
    assert_eq!(code_of(error), "git_unavailable");
}

#[test]
fn only_xdg_config_home_is_enough() {
    let fx = Fixture::new();
    let xdg = fx.dir.path().join("xdg");
    fs::create_dir_all(xdg.join("git")).unwrap();
    fs::write(
        xdg.join("git/config"),
        "[core]\n\tsshCommand = ssh -o Marker=xdg\n",
    )
    .unwrap();
    let mut seams = fx.seams();
    // git 2.39 stops with "$HOME not set" for --global even when XDG_CONFIG_HOME is set, so HOME is forwarded too;
    // the empty home has no .gitconfig, so the value can only come from the XDG file.
    seams.forward_env = Some(vec![
        ("HOME".into(), fx.home.to_string_lossy().into_owned()),
        ("XDG_CONFIG_HOME".into(), xdg.to_string_lossy().into_owned()),
        (
            "GIT_CONFIG_SYSTEM".into(),
            fx.system.to_string_lossy().into_owned(),
        ),
    ]);
    let argv = fx.argv(&GitRepo::new(&fx.root).with_seams(seams));
    assert!(has(&argv, "Marker=xdg"), "{argv:?}");
}

#[test]
fn a_value_with_quotes_reaches_ssh_as_a_manual_push_gives_it() {
    let fx = Fixture::new();
    fx.global("[core]\n\tsshCommand = ssh -o \\\"ProxyCommand=none\\\" -i 'my key'\n");
    let ours = fx.argv(&fx.repo());
    assert!(has(&ours, "my key"), "{ours:?}");

    let _ = fs::remove_file(&fx.ssh_log);
    let _ = Command::new(real_git())
        .args(["-C"])
        .arg(&fx.root)
        .args(["ls-remote", "ssh://capstan.invalid/r.git"])
        .env_clear()
        .env("PATH", format!("{}:{}", fx.bin.display(), host_path()))
        .env("HOME", &fx.home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let manual: Vec<String> = fs::read_to_string(&fx.ssh_log)
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    assert_eq!(ours, manual);
}

#[test]
fn a_root_that_is_not_a_repository_fails_in_git() {
    let fx = Fixture::new();
    let plain = fx.dir.path().join("plain");
    fs::create_dir_all(&plain).unwrap();
    let repo = GitRepo::new(&plain).with_seams(fx.seams());
    let out = repo.root_git(&ls_remote()).expect("git ran");
    assert_ne!(out.code, 0);
}

// ------------------------------------------------------------------------------------------------ probe counting

/// A fake git that logs `config` calls and runs the real git; `--version` answers at once (after `delay`).
fn counting(fx: &Fixture, log: &Path, delay: &str) -> PathBuf {
    fx.script(
        "git",
        &format!(
            "if [ \"$1\" = --version ]; then sleep {delay}; echo 'git version 2.45.0'; exit 0; fi\n\
             for a in \"$@\"; do if [ \"$a\" = config ]; then echo config >> '{}'; fi; done\n\
             exec '{}' \"$@\"",
            log.display(),
            real_git().display()
        ),
    )
}

fn config_calls(log: &Path) -> usize {
    fs::read_to_string(log).map_or(0, |text| text.lines().count())
}

fn counted(fx: &Fixture, extra: &[(&str, &str)]) -> (GitRepo, PathBuf) {
    let log = fx.dir.path().join("config.log");
    let _ = fs::remove_file(&log);
    let mut seams = fx.seams();
    seams.git_program = Some(counting(fx, &log, "0"));
    for (k, v) in extra {
        seams
            .forward_env
            .as_mut()
            .unwrap()
            .push((k.to_string(), v.to_string()));
    }
    (GitRepo::new(&fx.root).with_seams(seams), log)
}

#[test]
fn probes_run_for_network_calls_only() {
    let fx = Fixture::new();
    let (repo, log) = counted(&fx, &[]);
    let mut local = ls_remote();
    local.args = vec!["rev-parse".into(), "HEAD".into()];
    local.network = false;
    let _ = repo.root_git(&local);
    assert_eq!(config_calls(&log), 0);
    let _ = repo.root_git(&ls_remote());
    assert_eq!(config_calls(&log), 2);
}

#[test]
fn git_config_nosystem_skips_the_system_probe() {
    for (value, expected) in [("1", 1), ("true", 1), ("0", 2)] {
        let fx = Fixture::new();
        let (repo, log) = counted(&fx, &[("GIT_CONFIG_NOSYSTEM", value)]);
        let _ = repo.root_git(&ls_remote());
        assert_eq!(config_calls(&log), expected, "NOSYSTEM={value}");
    }
    let fx = Fixture::new();
    fs::write(&fx.system, "[core]\n\tsshCommand = ssh -o Marker=system\n").unwrap();
    let (repo, _) = counted(&fx, &[("GIT_CONFIG_NOSYSTEM", "1")]);
    let argv = fx.argv(&repo);
    assert!(!has(&argv, "Marker=system"), "{argv:?}");
}

#[test]
fn an_expired_probe_timeout_spawns_nothing() {
    let fx = Fixture::new();
    let (_, log) = counted(&fx, &[]);
    let mut seams = fx.seams();
    seams.git_program = Some(counting(&fx, &log, "0"));
    seams.skip_version_probe = true;
    seams.probe_timeout = Some(Duration::ZERO);
    let repo = GitRepo::new(&fx.root).with_seams(seams);
    let error = repo.root_git(&ls_remote()).unwrap_err();
    assert_eq!(code_of(error), "timeout");
    assert_eq!(config_calls(&log), 0);
}

#[test]
fn a_call_deadline_spent_by_the_version_probe_gives_timeout() {
    let fx = Fixture::new();
    let log = fx.dir.path().join("config.log");
    let mut seams = fx.seams();
    seams.git_program = Some(counting(&fx, &log, "1"));
    let repo = GitRepo::new(&fx.root).with_seams(seams);
    let mut call = ls_remote();
    call.timeout = Duration::from_millis(700);
    let error = repo.root_git(&call).unwrap_err();
    assert_eq!(code_of(error), "timeout");
    assert_eq!(config_calls(&log), 0);
    assert!(!fx.ssh_log.exists());
}
