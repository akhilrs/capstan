//! The repository-local config check: real git for the key families and the scopes, a fake git for the error paths.

use capstan_config::remote::DOC_UNSAFE_FAMILIES;
use capstan_kernel::integrate::{
    GitRepo, GitSeams, IntegrateError, UnsafeConfig, UnsafeScope, MERGE_UNSAFE_FAMILIES,
};
use std::path::{Path, PathBuf};
use std::process::Command;

struct Fx {
    tmp: tempfile::TempDir,
    root: PathBuf,
}

fn forward(tmp: &Path) -> Vec<(String, String)> {
    let home = tmp.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let home = home.to_string_lossy().into_owned();
    vec![
        ("HOME".into(), home.clone()),
        ("XDG_CONFIG_HOME".into(), home.clone()),
        ("GIT_CONFIG_GLOBAL".into(), format!("{home}/gitconfig")),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
    ]
}

fn git(dir: &Path, args: &[&str]) {
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
}

fn fixture() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["-c", "init.defaultBranch=main", "init", "-q"]);
    Fx { tmp, root }
}

impl Fx {
    fn repo(&self) -> GitRepo {
        self.repo_at(&self.root)
    }
    fn repo_at(&self, root: &Path) -> GitRepo {
        GitRepo::new(root).with_seams(GitSeams {
            forward_env: Some(forward(self.tmp.path())),
            ..GitSeams::default()
        })
    }
    fn set(&self, key: &str, value: &str) {
        git(&self.root, &["config", "--local", key, value]);
    }
    fn check(&self, scope: UnsafeScope) -> UnsafeConfig {
        self.repo().unsafe_repo_config(scope).unwrap()
    }
}

fn keys(config: UnsafeConfig) -> Vec<String> {
    match config {
        UnsafeConfig::Keys(keys) => keys,
        other => panic!("expected keys, got {other:?}"),
    }
}

/// A sample key for a pattern: `x` for a `*` subsection, `y` / `x.y` for a trailing `*`.
fn samples(pattern: &str) -> Vec<String> {
    if let Some(section) = pattern.strip_suffix(".*") {
        if !section.contains('.') {
            return vec![format!("{section}.y"), format!("{section}.x.y")];
        }
    }
    vec![pattern.replacen('*', "x", 1)]
}

fn value_for(key: &str) -> &'static str {
    if key.eq_ignore_ascii_case("core.bare") {
        "true"
    } else if key == "core.worktree" {
        ".."
    } else {
        "v"
    }
}

#[test]
fn every_merge_family_refuses_merge_scope() {
    for pattern in MERGE_UNSAFE_FAMILIES {
        for key in samples(pattern) {
            let fx = fixture();
            fx.set(&key, value_for(&key));
            let found = keys(fx.check(UnsafeScope::Merge));
            assert!(
                found.iter().any(|k| k.eq_ignore_ascii_case(&key)),
                "{pattern}: {key} not in {found:?}"
            );
        }
    }
}

#[test]
fn every_doc_family_refuses_push_scope() {
    for pattern in DOC_UNSAFE_FAMILIES {
        for key in samples(pattern) {
            let fx = fixture();
            fx.set(&key, value_for(&key));
            let found = keys(fx.check(UnsafeScope::Push));
            assert!(
                found.iter().any(|k| k.eq_ignore_ascii_case(&key)),
                "{pattern}: {key} not in {found:?}"
            );
        }
    }
}

#[test]
fn push_only_keys_do_not_refuse_merge_and_hooks_refuse_neither() {
    for key in [
        "credential.helper",
        "http.proxy",
        "url.x.insteadOf",
        "core.pager",
    ] {
        let fx = fixture();
        fx.set(key, "v");
        assert!(matches!(fx.check(UnsafeScope::Push), UnsafeConfig::Keys(_)));
        assert_eq!(fx.check(UnsafeScope::Merge), UnsafeConfig::Clean, "{key}");
    }
    for key in ["core.hooksPath", "core.fsmonitor"] {
        let fx = fixture();
        fx.set(key, "v");
        assert_eq!(fx.check(UnsafeScope::Push), UnsafeConfig::Clean, "{key}");
        assert_eq!(fx.check(UnsafeScope::Merge), UnsafeConfig::Clean, "{key}");
    }
}

#[test]
fn case_rules_of_a_key() {
    let fx = fixture();
    fx.set("Core.SshCommand", "v");
    assert_eq!(keys(fx.check(UnsafeScope::Push)), ["core.sshcommand"]);
    let fx = fixture();
    fx.set("Filter.X.smudge", "v");
    assert_eq!(keys(fx.check(UnsafeScope::Merge)), ["filter.X.smudge"]);
    let fx = fixture();
    fx.set("Merge.X.Driver", "v");
    git(
        &fx.root,
        &["config", "--local", "--unset", "merge.X.driver"],
    );
    fx.set("url.X.insteadof", "v");
    assert_eq!(keys(fx.check(UnsafeScope::Push)), ["url.X.insteadof"]);
}

#[test]
fn ordinary_repositories_are_clean_in_both_scopes() {
    let fx = fixture();
    let std_ = [UnsafeScope::Merge, UnsafeScope::Push];
    for scope in std_ {
        assert_eq!(fx.check(scope), UnsafeConfig::Clean);
    }
    let clone = fx.tmp.path().join("clone");
    git(
        fx.tmp.path(),
        &[
            "-c",
            "init.defaultBranch=main",
            "clone",
            "-q",
            fx.root.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    for scope in std_ {
        assert_eq!(
            fx.repo_at(&clone).unsafe_repo_config(scope).unwrap(),
            UnsafeConfig::Clean
        );
    }
    git(&clone, &["config", "--local", "user.name", "a"]);
    git(&clone, &["config", "--local", "core.autocrlf", "true"]);
    for scope in std_ {
        assert_eq!(
            fx.repo_at(&clone).unsafe_repo_config(scope).unwrap(),
            UnsafeConfig::Clean
        );
    }
}

#[test]
fn core_bare_follows_the_git_boolean() {
    for scope in [UnsafeScope::Merge, UnsafeScope::Push] {
        for value in ["true", "yes", "1"] {
            let fx = fixture();
            fx.set("core.bare", value);
            assert_eq!(keys(fx.check(scope)), ["core.bare"], "{value}");
        }
        for value in ["false", "", "no", "off", "0"] {
            let fx = fixture();
            fx.set("core.bare", value);
            assert_eq!(fx.check(scope), UnsafeConfig::Clean, "{value:?}");
        }
        let fx = fixture();
        let path = fx.root.join(".git/config");
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("[core]\n\tbare\n");
        std::fs::write(&path, text).unwrap();
        assert_eq!(keys(fx.check(scope)), ["core.bare"]);
    }
}

#[test]
fn includes_refuse_by_presence() {
    let fx = fixture();
    let harmless = fx.tmp.path().join("harmless");
    std::fs::write(&harmless, "[user]\n\tname = a\n").unwrap();
    fx.set("include.path", harmless.to_str().unwrap());
    for scope in [UnsafeScope::Merge, UnsafeScope::Push] {
        assert_eq!(keys(fx.check(scope)), ["include.path"]);
    }
    let fx = fixture();
    fx.set("includeIf.gitdir:/x/.path", harmless.to_str().unwrap());
    for scope in [UnsafeScope::Merge, UnsafeScope::Push] {
        assert_eq!(keys(fx.check(scope)), ["includeif.gitdir:/x/.path"]);
    }
}

#[test]
fn merge_scope_is_a_deny_list() {
    let fx = fixture();
    fx.set("core.untrackedCache", "true");
    fx.set("merge.x.name", "n");
    fx.set("filter.x.required", "true");
    assert_eq!(fx.check(UnsafeScope::Merge), UnsafeConfig::Clean);
}

#[test]
fn key_splitting_with_dots_and_empty_subsections() {
    let fx = fixture();
    fx.set("remote.a.b.pushurl", "v");
    assert_eq!(keys(fx.check(UnsafeScope::Push)), ["remote.a.b.pushurl"]);
    let fx = fixture();
    fx.set("filter..smudge", "v");
    assert_eq!(keys(fx.check(UnsafeScope::Merge)), ["filter..smudge"]);
    let fx = fixture();
    fx.set("alias.x", "line1\nfilter.y.smudge=z");
    assert_eq!(fx.check(UnsafeScope::Merge), UnsafeConfig::Clean);
}

#[test]
fn worktree_config_is_read_only_when_enabled() {
    let fx = fixture();
    let file = fx.root.join(".git/config.worktree");
    std::fs::write(
        &file,
        "[filter \"x\"]\n\tsmudge = v\n[core]\n\tsshCommand = v\n",
    )
    .unwrap();
    assert_eq!(fx.check(UnsafeScope::Merge), UnsafeConfig::Clean);
    fx.set("extensions.worktreeConfig", "true");
    assert_eq!(keys(fx.check(UnsafeScope::Merge)), ["filter.x.smudge"]);
    assert!(keys(fx.check(UnsafeScope::Push)).contains(&"core.sshcommand".to_string()));
    fx.set("extensions.worktreeConfig", "false");
    assert_eq!(fx.check(UnsafeScope::Push), UnsafeConfig::Clean);
}

#[test]
fn worktree_config_flag_without_a_file_or_with_a_directory() {
    let fx = fixture();
    fx.set("extensions.worktreeConfig", "true");
    assert_eq!(fx.check(UnsafeScope::Push), UnsafeConfig::Clean);
    std::fs::create_dir(fx.root.join(".git/config.worktree")).unwrap();
    assert_eq!(fx.check(UnsafeScope::Push), UnsafeConfig::Unreadable);
}

#[test]
fn a_linked_worktree_root() {
    let fx = fixture();
    git(&fx.root, &["config", "user.email", "a@b"]);
    git(&fx.root, &["config", "user.name", "a"]);
    git(&fx.root, &["commit", "-q", "--allow-empty", "-m", "x"]);
    let linked = fx.tmp.path().join("linked");
    git(
        &fx.root,
        &["worktree", "add", "-q", linked.to_str().unwrap()],
    );
    let repo = fx.repo_at(&linked);
    assert_eq!(
        repo.unsafe_repo_config(UnsafeScope::Merge).unwrap(),
        UnsafeConfig::Clean
    );
    fx.set("core.sshCommand", "v");
    assert_eq!(
        keys(repo.unsafe_repo_config(UnsafeScope::Push).unwrap()),
        ["core.sshcommand"]
    );
    assert_eq!(
        repo.unsafe_repo_config(UnsafeScope::Merge).unwrap(),
        UnsafeConfig::Clean
    );
}

#[test]
fn into_refusal_codes() {
    assert!(UnsafeConfig::Clean.into_refusal().is_none());
    let refusal = UnsafeConfig::Unreadable.into_refusal().unwrap();
    assert_eq!(refusal.code(), "unsafe_config");
    assert_eq!(refusal.files(), ["(config unreadable)"]);
    let refusal = UnsafeConfig::Keys(vec!["core.pager".into()])
        .into_refusal()
        .unwrap();
    assert_eq!(refusal.files(), ["core.pager"]);
}

/// A fake git that logs argv and the environment of every call to `log` and answers by the subcommand.
fn fake(tmp: &Path, config_failure: &str, worktree_failure: &str) -> (PathBuf, PathBuf) {
    let log = tmp.join("fake.log");
    let script = tmp.join("fake-git");
    let body = format!(
        r#"#!/bin/sh
{{ echo "ARGS $*"; echo "LC_ALL=$LC_ALL"; echo "LANG=${{LANG-unset}}"; }} >> '{log}'
case " $* " in
  *" rev-parse --git-dir "*) echo .git; exit 0;;
  *" rev-parse --git-common-dir "*) echo .git; exit 0;;
  *" --worktree "*) {worktree_failure};;
  *" --local "*) {config_failure};;
esac
"#,
        log = log.display(),
    );
    std::fs::write(&script, body).unwrap();
    Command::new("chmod")
        .arg("755")
        .arg(&script)
        .status()
        .unwrap();
    (script, log)
}

fn fake_repo(fx: &Fx, program: PathBuf, extra: Vec<(String, String)>) -> GitRepo {
    let mut env = forward(fx.tmp.path());
    env.extend(extra);
    GitRepo::new(&fx.root).with_seams(GitSeams {
        forward_env: Some(env),
        git_program: Some(program),
        skip_version_probe: true,
        ..GitSeams::default()
    })
}

#[test]
fn fake_git_error_classification_end_to_end() {
    let fx = fixture();
    let (program, _) = fake(
        fx.tmp.path(),
        "echo 'fatal: bad config line 6 in file .git/config' >&2; exit 128",
        "exit 0",
    );
    let repo = fake_repo(&fx, program, vec![]);
    for scope in [UnsafeScope::Merge, UnsafeScope::Push] {
        assert_eq!(
            repo.unsafe_repo_config(scope).unwrap(),
            UnsafeConfig::Unreadable
        );
    }
    let (program, _) = fake(
        fx.tmp.path(),
        "echo 'fatal: detected dubious ownership in repository at /r' >&2; exit 128",
        "exit 0",
    );
    let err = fake_repo(&fx, program, vec![])
        .unsafe_repo_config(UnsafeScope::Merge)
        .unwrap_err();
    match err {
        IntegrateError::Integration { code, message } => {
            assert_eq!(code, "git_unavailable");
            assert!(message.contains("dubious ownership"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    let (program, _) = fake(
        fx.tmp.path(),
        "echo 'fatal: something odd' >&2; exit 128",
        "exit 0",
    );
    match fake_repo(&fx, program, vec![])
        .unsafe_repo_config(UnsafeScope::Push)
        .unwrap_err()
    {
        IntegrateError::Integration { code, .. } => assert_eq!(code, "git_unavailable"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn fake_git_worktree_read_failure_and_logged_environment() {
    let fx = fixture();
    // rev-parse prints a relative .git, so the read looks at <root>/.git/config.worktree.
    std::fs::write(fx.root.join(".git/config.worktree"), "").unwrap();
    let (program, log) = fake(
        fx.tmp.path(),
        "printf 'extensions.worktreeconfig\\ntrue\\0'; exit 0",
        "echo \"fatal: bad config line 1 in file .git/config.worktree\" >&2; exit 128",
    );
    let repo = fake_repo(&fx, program, vec![("LANG".into(), "de_DE.UTF-8".into())]);
    assert_eq!(
        repo.unsafe_repo_config(UnsafeScope::Push).unwrap(),
        UnsafeConfig::Unreadable
    );
    let text = std::fs::read_to_string(log).unwrap();
    assert!(text.contains("--no-replace-objects"), "{text}");
    assert!(text.contains("LC_ALL=C"), "{text}");
    assert!(text.contains("LANG=unset"), "{text}");
    assert!(text.contains("rev-parse --git-dir"), "{text}");
    assert!(text.contains("config --worktree -z --list"), "{text}");
}

#[test]
fn an_invalid_core_bare_value_refuses() {
    // Real git refuses to start on such a value, so the entry comes from a fake git.
    let fx = fixture();
    let (program, _) = fake(
        fx.tmp.path(),
        "printf 'core.bare\\nmaybe\\0core.bare\\0'; exit 0",
        "exit 0",
    );
    let repo = fake_repo(&fx, program, vec![]);
    for scope in [UnsafeScope::Merge, UnsafeScope::Push] {
        assert_eq!(
            repo.unsafe_repo_config(scope).unwrap(),
            UnsafeConfig::Keys(vec!["core.bare".into()])
        );
    }
}
