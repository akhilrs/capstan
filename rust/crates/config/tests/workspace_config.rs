//! The `[repos]` and `[workspace]` sections: loading without git, every refusal text, the git half
//! (`validate_workspace`), the JSON members and the trust of a workspace root that may not be a repository.
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use capstan_config::starter::{STARTER_CONFIG, WORKSPACE_STARTER_CONFIG};
use capstan_config::{
    load_role_config_with, parse_config, record_current, trust_status_with, validate_workspace,
    workspace_gate, ConfigError, RoleConfig, TrustStatus, TrustStore, WORKSPACE_NOT_COMPLETE,
};

const BASE: &str = r#"schema_version = 1
[nexora]
track = "never"
[hosts.claude]
kind = "claude"
[roles.pm]
kind = "PM"
host = "claude"
"#;

struct Project {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

fn project() -> Project {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap().join("ws");
    fs::create_dir(&root).unwrap();
    Project { _tmp: tmp, root }
}

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(["-c", "init.defaultBranch=main"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("git must be installed");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A repository with one commit on `main`.
fn make_repo(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "--quiet"]);
    fs::write(dir.join("a"), "a").unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "--quiet", "-m", "chore: initial commit"]);
}

fn load(project: &Project, extra: &str) -> Result<RoleConfig, String> {
    let text = format!("{BASE}{extra}");
    parse_config(text.as_bytes(), &project.root).map_err(|error| match error {
        ConfigError::Invalid(message) => message,
        other => format!("{other:?}"),
    })
}

fn refusal(project: &Project, extra: &str) -> String {
    load(project, extra).expect_err("refused")
}

fn two_repos() -> &'static str {
    "[repos.api]\npath = \"api\"\nmain_branch = \"main\"\n[repos.web]\npath = \"web\"\nmain_branch = \"main\"\n"
}

#[test]
fn a_non_repo_root_with_relative_and_symlinked_repos_resolves() {
    let p = project();
    make_repo(&p.root.join("api"));
    make_repo(&p.root.join("web"));
    let outside = p.root.parent().unwrap().join("elsewhere");
    make_repo(&outside);
    symlink(&outside, p.root.parent().unwrap().join("link")).unwrap();
    let extra = format!(
        "{}[repos.ext]\npath = \"{}\"\nmain_branch = \"main\"\n[repos.up]\npath = \"../elsewhere\"\nmain_branch = \"main\"\n",
        two_repos(),
        p.root.parent().unwrap().join("link").display()
    );
    let config = load(&p, &extra).unwrap();
    let ws = config.workspace.expect("workspace mode");
    let names: Vec<&str> = ws.repos.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["api", "ext", "up", "web"]);
    assert_eq!(ws.repo("api").unwrap().root, p.root.join("api"));
    assert_eq!(ws.repo("ext").unwrap().root, outside);
    assert_eq!(ws.repo("up").unwrap().root, outside);
    assert_eq!(ws.main_branch_of("web"), "main");
    // Two names for one repository load; validation refuses them.
    assert_eq!(
        validate_workspace(&ws).unwrap_err(),
        "repos ext and up are the same repository"
    );
}

#[test]
fn defaults_are_by_name_and_paths_may_be_absolute_under_the_root() {
    let p = project();
    make_repo(&p.root.join("api"));
    make_repo(&p.root.join("web"));
    let extra = format!(
        "[repos.web]\npath = \"{}\"\nmain_branch = \"trunk\"\n[repos.api]\npath = \"api\"\nmain_branch = \"main\"\n",
        p.root.join("web").display()
    );
    let ws = load(&p, &extra).unwrap().workspace.unwrap();
    assert_eq!(ws.merge_order, ["api", "web"]);
    assert_eq!(ws.default_repo, "api");
    assert_eq!(ws.worktree_dir, p.root.join(".capstan").join("worktrees"));
    assert_eq!(ws.repo("web").unwrap().root, p.root.join("web"));
    assert_eq!(ws.main_branch_of("web"), "trunk");
    let listed = format!("{extra}[workspace]\nmerge_order = [\"web\"]\ndefault_repo = \"web\"\n");
    let ws = load(&p, &listed).unwrap().workspace.unwrap();
    assert_eq!(ws.merge_order, ["web", "api"]);
    assert_eq!(ws.default_repo, "web");
}

#[test]
fn every_refusal_has_its_text() {
    let p = project();
    make_repo(&p.root.join("api"));
    let ok = "path = \"api\"\nmain_branch = \"main\"\n";
    let cases: Vec<(String, &str)> = vec![
        (
            format!("[repos.Api]\n{ok}"),
            "repo name Api must match ^[a-z0-9][a-z0-9_-]{0,31}$",
        ),
        (
            "[repos.api]\nmain_branch = \"main\"\n".into(),
            "[repos.api] needs path",
        ),
        (
            "[repos.api]\npath = \"api\"\n".into(),
            "[repos.api] needs main_branch",
        ),
        (
            "[repos.me]\npath = \".\"\nmain_branch = \"main\"\n".into(),
            "the workspace root cannot be a listed repo",
        ),
        (
            format!("[repos.api]\n{ok}[workspace]\nmerge_order = [\"zzz\"]\n"),
            "merge_order names unknown repo zzz",
        ),
        (
            format!("[repos.api]\n{ok}[workspace]\nmerge_order = [\"api\", \"api\"]\n"),
            "merge_order names api twice",
        ),
        (
            format!("[repos.api]\n{ok}[workspace]\ndefault_repo = \"zzz\"\n"),
            "default_repo names unknown repo zzz",
        ),
        (
            format!("[repos.api]\n{ok}cache = [\"rust\"]\n"),
            "repo api names unknown cache entry rust",
        ),
    ];
    for (extra, text) in cases {
        let got = refusal(&p, &extra);
        assert!(got.contains(text), "{extra}\n=> {got}");
    }
    // The root named by an absolute path or a symlink is refused too.
    symlink(&p.root, p.root.parent().unwrap().join("self")).unwrap();
    let got = refusal(
        &p,
        &format!(
            "[repos.me]\npath = \"{}\"\nmain_branch = \"main\"\n",
            p.root.parent().unwrap().join("self").display()
        ),
    );
    assert!(
        got.contains("the workspace root cannot be a listed repo"),
        "{got}"
    );
    // A known cache entry loads.
    let extra = format!("[cache]\npresets = [\"rust\"]\n[repos.api]\n{ok}cache = [\"rust\"]\n");
    assert!(load(&p, &extra).is_ok());
}

#[test]
fn the_boundaries_are_32_repos_and_32_characters() {
    let p = project();
    let many = |count: usize| -> String {
        (0..count)
            .map(|n| format!("[repos.r{n:02}]\npath = \"r{n:02}\"\nmain_branch = \"main\"\n"))
            .collect()
    };
    assert_eq!(
        load(&p, &many(32)).unwrap().workspace.unwrap().repos.len(),
        32
    );
    assert!(refusal(&p, &many(33)).contains("at most 32 repos may be listed"));
    let name = |text: &str| format!("[repos.{text}]\npath = \"x\"\nmain_branch = \"main\"\n");
    assert!(load(&p, &name(&format!("a{}", "b".repeat(31)))).is_ok());
    assert!(refusal(&p, &name(&format!("a{}", "b".repeat(32)))).contains("repo name a"));
    assert!(load(&p, &name("1_a-b")).is_ok());
    assert!(refusal(&p, &name("_a")).contains("must match"));
}

#[test]
fn json_has_the_members_only_in_workspace_mode() {
    let p = project();
    let plain = load(&p, "").unwrap();
    assert!(plain.workspace.is_none());
    let json = plain.to_json();
    assert!(!json.contains("\"repos\"") && !json.contains("\"workspace\""));
    let json = load(&p, two_repos()).unwrap().to_json();
    assert!(
        json.contains("\"repos\"") && json.contains("\"workspace\""),
        "{json}"
    );
    assert!(json.contains("\"mergeOrder\"") && json.contains("\"defaultRepo\""));
    assert_eq!(
        refusal(&p, "[workspace]\ndefault_repo = \"a\"\n"),
        "[workspace] needs at least one [repos.<name>]"
    );
}

#[test]
fn loading_keeps_what_git_will_refuse() {
    let p = project();
    make_repo(&p.root.join("api"));
    fs::create_dir_all(p.root.join("api").join("sub")).unwrap();
    make_repo(&p.root.join("nested"));
    make_repo(&p.root.join("nested").join("inner"));
    git(&p.root.join("nested"), &["init", "--quiet"]);
    // missing path, a nested repository, a subdirectory of a repository, a worktree_dir inside a repository
    let extra = "[repos.api]\npath = \"api\"\nmain_branch = \"main\"\n[repos.gone]\npath = \"missing\"\nmain_branch = \"main\"\n";
    let ws = load(&p, extra).unwrap().workspace.unwrap();
    assert_eq!(ws.repo("gone").unwrap().root, p.root.join("missing"));
    assert_eq!(
        validate_workspace(&ws).unwrap_err(),
        format!(
            "repo gone: {} is not the top level of a git work tree with a commit",
            p.root.join("missing").display()
        )
    );

    let sub = "[repos.sub]\npath = \"api/sub\"\nmain_branch = \"main\"\n";
    let ws = load(&p, sub).unwrap().workspace.unwrap();
    assert!(validate_workspace(&ws).unwrap_err().contains("repo sub: "));
    assert!(validate_workspace(&ws)
        .unwrap_err()
        .ends_with("is not the top level of a git work tree with a commit"));

    let nested = "[repos.outer]\npath = \"nested\"\nmain_branch = \"main\"\n[repos.inner]\npath = \"nested/inner\"\nmain_branch = \"main\"\n";
    let ws = load(&p, nested).unwrap().workspace.unwrap();
    assert_eq!(
        validate_workspace(&ws).unwrap_err(),
        "repo inner lies inside repo outer"
    );

    let wt = "[repos.api]\npath = \"api\"\nmain_branch = \"main\"\n[workspace]\nworktree_dir = \"api/trees\"\n";
    let ws = load(&p, wt).unwrap().workspace.unwrap();
    assert_eq!(
        validate_workspace(&ws).unwrap_err(),
        "worktree_dir lies inside repo api"
    );

    let branch = "[repos.api]\npath = \"api\"\nmain_branch = \"nope\"\n";
    let ws = load(&p, branch).unwrap().workspace.unwrap();
    assert_eq!(
        validate_workspace(&ws).unwrap_err(),
        "repo api has no branch nope"
    );
}

#[test]
fn a_repo_without_a_commit_is_refused_and_a_good_workspace_passes() {
    let p = project();
    make_repo(&p.root.join("api"));
    fs::create_dir(p.root.join("empty")).unwrap();
    git(&p.root.join("empty"), &["init", "--quiet"]);
    let extra = "[repos.empty]\npath = \"empty\"\nmain_branch = \"main\"\n";
    let ws = load(&p, extra).unwrap().workspace.unwrap();
    assert!(validate_workspace(&ws)
        .unwrap_err()
        .starts_with("repo empty: "));
    let ws = load(&p, "[repos.api]\npath = \"api\"\nmain_branch = \"main\"\n")
        .unwrap()
        .workspace
        .unwrap();
    assert_eq!(validate_workspace(&ws), Ok(()));
}

#[test]
fn a_worktree_dir_inside_another_project_is_refused_and_one_outside_any_is_fine() {
    let p = project();
    make_repo(&p.root.join("api"));
    let other = p.root.parent().unwrap().join("other");
    fs::create_dir_all(other.join(".capstan")).unwrap();
    let repo = "[repos.api]\npath = \"api\"\nmain_branch = \"main\"\n";
    let inside = format!(
        "{repo}[workspace]\nworktree_dir = \"{}\"\n",
        other.join("trees").display()
    );
    let ws = load(&p, &inside).unwrap().workspace.unwrap();
    assert_eq!(
        validate_workspace(&ws).unwrap_err(),
        format!(
            "worktree_dir lies inside another project {}",
            other.display()
        )
    );
    let free = p.root.parent().unwrap().join("free").join("trees");
    let outside = format!("{repo}[workspace]\nworktree_dir = \"{}\"\n", free.display());
    let ws = load(&p, &outside).unwrap().workspace.unwrap();
    assert_eq!(validate_workspace(&ws), Ok(()));
}

#[test]
fn the_gate_names_the_unfinished_mode() {
    assert_eq!(
        workspace_gate(None),
        Err(WORKSPACE_NOT_COMPLETE.to_string())
    );
    assert_eq!(
        workspace_gate(Some("0")),
        Err(WORKSPACE_NOT_COMPLETE.to_string())
    );
    assert_eq!(workspace_gate(Some("1")), Ok(()));
}

#[test]
fn the_workspace_starter_extends_the_starter_with_a_commented_example() {
    assert!(WORKSPACE_STARTER_CONFIG.starts_with(STARTER_CONFIG));
    let tail = &WORKSPACE_STARTER_CONFIG[STARTER_CONFIG.len()..];
    assert!(tail.contains("# [repos.api]") && tail.contains("# [workspace]"));
    assert!(tail.lines().all(|l| l.is_empty() || l.starts_with('#')));
    let p = project();
    let config = parse_config(WORKSPACE_STARTER_CONFIG.as_bytes(), &p.root).unwrap();
    assert!(config.workspace.is_none());
}

fn write_config(dir: &Path, text: &str) {
    let file = dir.join("capstan.toml");
    fs::write(&file, text).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
}

fn changed(status: &TrustStatus) -> Vec<String> {
    match status {
        TrustStatus::Needed { changes, .. } => changes.iter().map(|c| c.key.clone()).collect(),
        other => panic!("expected Needed, got {other:?}"),
    }
}

#[test]
fn trust_covers_every_repo_key_and_follows_the_root_folder() {
    let p = project();
    let store = TrustStore::at(p.root.parent().unwrap().join("store"));
    let text = format!(
        "{BASE}[repos.api]\npath = \"api\"\nmain_branch = \"main\"\nverify = {{ commands = [\"make test\"] }}\n"
    );
    write_config(&p.root, &text);
    // A root that is not a repository: its first record is keyed by the folder.
    assert!(matches!(
        trust_status_with(&p.root, &store),
        TrustStatus::Needed { .. }
    ));
    record_current(&p.root, &store).unwrap();
    assert_eq!(trust_status_with(&p.root, &store), TrustStatus::Trusted);
    assert!(load_role_config_with(&p.root, &store)
        .unwrap()
        .workspace
        .is_some());

    for (from, to, key) in [
        ("path = \"api\"", "path = \"api2\"", "repos.api.path"),
        (
            "\"main\"\nverify",
            "\"trunk\"\nverify",
            "repos.api.main_branch",
        ),
        ("make test", "make evil", "repos.api.verify.commands"),
    ] {
        write_config(&p.root, &text.replace(from, to));
        let keys = changed(&trust_status_with(&p.root, &store));
        assert!(keys.iter().any(|k| k.starts_with(key)), "{key}: {keys:?}");
    }
    write_config(
        &p.root,
        &format!("{text}[workspace]\nmerge_order = [\"api\"]\n"),
    );
    assert!(changed(&trust_status_with(&p.root, &store))
        .iter()
        .any(|k| k.starts_with("workspace.merge_order")));

    // The root later becomes a repository: one record, and the local file is untracked until committed.
    write_config(&p.root, &text);
    git(&p.root, &["init", "--quiet"]);
    assert_eq!(trust_status_with(&p.root, &store), TrustStatus::Trusted);
    let local = p.root.join("capstan.local.toml");
    fs::write(&local, "# machine settings\n").unwrap();
    fs::set_permissions(&local, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(load_role_config_with(&p.root, &store).is_ok());
    git(&p.root, &["add", "capstan.local.toml"]);
    assert!(matches!(
        load_role_config_with(&p.root, &store),
        Err(ConfigError::Invalid(m)) if m.contains("is tracked in git")
    ));
}

#[test]
fn a_local_file_in_a_non_repo_root_counts_as_untracked() {
    let p = project();
    let store = TrustStore::at(p.root.parent().unwrap().join("store"));
    write_config(&p.root, &format!("{BASE}{}", two_repos()));
    let local = p.root.join("capstan.local.toml");
    fs::write(&local, "# machine settings\n").unwrap();
    fs::set_permissions(&local, fs::Permissions::from_mode(0o644)).unwrap();
    record_current(&p.root, &store).unwrap();
    let config = load_role_config_with(&p.root, &store).unwrap();
    assert_eq!(config.workspace.unwrap().repos.len(), 2);
}

#[test]
fn a_broken_repo_loads_and_only_validation_refuses() {
    let p = project();
    make_repo(&p.root.join("api"));
    let store = TrustStore::at(p.root.parent().unwrap().join("store"));
    write_config(
        &p.root,
        &format!("{BASE}[repos.api]\npath = \"api\"\nmain_branch = \"main\"\n"),
    );
    record_current(&p.root, &store).unwrap();
    git(&p.root.join("api"), &["branch", "-m", "main", "other"]);
    let ws = load_role_config_with(&p.root, &store)
        .unwrap()
        .workspace
        .unwrap();
    assert_eq!(
        validate_workspace(&ws).unwrap_err(),
        "repo api has no branch main"
    );
}
