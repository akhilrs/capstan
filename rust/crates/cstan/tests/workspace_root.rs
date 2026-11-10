//! Workspace mode at the front door: the unfinished-mode gate of `cstan start`, `cstan init --workspace` and the daemon
//! boot, `init --workspace` over a root that is not a repository, and the socket verdict from a worktree folder.
mod support;

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use capstan_config::starter::WORKSPACE_STARTER_CONFIG;
use capstan_config::{
    load_role_config_with, record_current, trust_status_with, TrustStatus, WORKSPACE_NOT_COMPLETE,
    WORKSPACE_ROOT_NOT_REPO,
};
use support::*;

const NOT_A_REPO_TEXT: &str = "this build boots the daemon only from a git repository root; a non-repo workspace root needs a later build";

fn git(directory: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(["-c", "init.defaultBranch=main"])
        .args(args)
        .env_clear()
        .env("PATH", HOST_PATH)
        .env("HOME", "/nonexistent")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn repo_with_commit(directory: &Path) {
    std::fs::create_dir_all(directory).unwrap();
    git(directory, &["init", "--quiet"]);
    std::fs::write(directory.join("a"), "a").unwrap();
    git(directory, &["add", "-A"]);
    git(
        directory,
        &["commit", "--quiet", "-m", "chore: initial commit"],
    );
}

fn preview(bed: &TrustBed) -> Vec<(OsString, OsString)> {
    let mut env = bed.env();
    env.push((
        OsString::from("CAPSTAN_WORKSPACE_PREVIEW"),
        OsString::from("1"),
    ));
    env
}

const BASE: &str = "schema_version = 1\n[nexora]\ntrack = \"never\"\n[hosts.claude]\nkind = \"claude\"\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n";

fn workspace_toml(extra: &str) -> String {
    format!("{BASE}{extra}")
}

const API: &str = "[repos.api]\npath = \"api\"\nmain_branch = \"main\"\n";

/// A project initialised with the preview, then given `toml` and trusted.
fn initialised(label: &str, toml: &str) -> TrustBed {
    let bed = TrustBed::new(label);
    let (exit, out, err) = bed.run(&["init", "--workspace"], preview(&bed));
    assert_eq!(exit, 0, "{out}{err}");
    bed.write(toml);
    record_current(&bed.project, &bed.store()).unwrap();
    bed
}

#[test]
fn init_workspace_and_start_are_refused_without_the_preview() {
    let bed = TrustBed::new("ws-gate-init");
    let (exit, out, err) = bed.run(&["init", "--workspace"], bed.env());
    assert_ne!(exit, 0, "{out}");
    assert!(err.contains(WORKSPACE_NOT_COMPLETE), "{err}");
    assert!(!bed.project.join(".capstan").exists());
    assert!(!bed.project.join("capstan.toml").exists());

    // The gate comes before validation: a broken repository gives the not-complete text, not the git text.
    let bed = initialised("ws-gate-start", &workspace_toml(API));
    let (exit, out, err) = bed.run(&["start"], bed.env());
    assert_ne!(exit, 0, "{out}");
    assert!(err.contains(WORKSPACE_NOT_COMPLETE), "{err}");
    assert!(!err.contains("top level"), "{err}");
    // The gate reads the file without judging trust.
    bed.write(&workspace_toml(&format!("{API}# changed\n")));
    let (_, _, err) = bed.run(&["start"], bed.env());
    assert!(err.contains(WORKSPACE_NOT_COMPLETE), "{err}");
}

#[test]
fn start_with_the_preview_names_a_repo_without_a_commit() {
    let bed = initialised("ws-start-nocommit", &workspace_toml(API));
    std::fs::create_dir_all(bed.project.join("api")).unwrap();
    git(&bed.project.join("api"), &["init", "--quiet"]);
    let (exit, out, err) = bed.run(&["start"], preview(&bed));
    assert_ne!(exit, 0, "{out}");
    assert!(
        err.contains("repo api: ")
            && err.contains("is not the top level of a git work tree with a commit"),
        "{err}"
    );
}

#[test]
fn init_workspace_works_in_an_empty_folder_and_writes_the_starter_byte_for_byte() {
    let bed = TrustBed::new("ws-init");
    let (exit, out, err) = bed.run(&["init", "--workspace"], preview(&bed));
    assert_eq!(exit, 0, "{out}{err}");
    assert_eq!(
        std::fs::read_to_string(bed.project.join("capstan.toml")).unwrap(),
        WORKSPACE_STARTER_CONFIG
    );
    assert!(bed.project.join(".capstan").is_dir());
    // The folder is not a repository: no .gitignore, no git run, no git warning.
    assert!(!bed.project.join(".gitignore").exists());
    assert!(!bed.project.join(".git").exists());
    assert!(!err.contains("git repository"), "{err}");
    assert_eq!(
        trust_status_with(&bed.project, &bed.store()),
        TrustStatus::Trusted
    );
    assert!(load_role_config_with(&bed.project, &bed.store())
        .unwrap()
        .workspace
        .is_none());
    // The flag stands alone.
    for args in [
        &["init", "--workspace", "--git"][..],
        &["init", "--workspace", "--gitignore"][..],
    ] {
        let (exit, _, _) = bed.run(args, preview(&bed));
        assert_eq!(exit, 2, "{args:?}");
    }
}

#[test]
fn init_workspace_in_a_repository_root_writes_the_block() {
    let bed = TrustBed::new("ws-init-repo");
    repo_with_commit(&bed.project);
    let (exit, out, err) = bed.run(&["init", "--workspace"], preview(&bed));
    assert_eq!(exit, 0, "{out}{err}");
    let ignore = std::fs::read_to_string(bed.project.join(".gitignore")).unwrap();
    assert!(ignore.contains(".capstan/"), "{ignore}");
}

#[test]
fn editing_main_branch_turns_trust_to_changed() {
    let toml = workspace_toml(API);
    let bed = initialised("ws-trust", &toml);
    assert_eq!(
        trust_status_with(&bed.project, &bed.store()),
        TrustStatus::Trusted
    );
    bed.write(&toml.replace("\"main\"", "\"trunk\""));
    match trust_status_with(&bed.project, &bed.store()) {
        TrustStatus::Needed { changes, .. } => assert!(
            changes.iter().any(|c| c.key == "repos.api.main_branch"),
            "{changes:?}"
        ),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_daemon_boot_gates_then_validates_then_wants_a_repository_root() {
    let bed = initialised("ws-boot", &workspace_toml(API));
    repo_with_commit(&bed.project.join("api"));
    let config = load_role_config_with(&bed.project, &bed.store()).unwrap();
    let check = |preview: Option<&str>| {
        capstan_daemon::run::check_workspace_boot(&config, &bed.project, preview)
            .map_err(|error| error.message)
    };
    // The root is not a repository here: without the variable the gate answers first.
    assert_eq!(check(None).unwrap_err(), WORKSPACE_NOT_COMPLETE);
    assert_eq!(check(Some("1")).unwrap_err(), NOT_A_REPO_TEXT);
    assert_eq!(WORKSPACE_ROOT_NOT_REPO, NOT_A_REPO_TEXT);
    // A root that is a git repository listed nowhere boots with the variable only.
    repo_with_commit(&bed.project);
    git(&bed.project, &["rm", "-r", "--cached", "--quiet", "api"]);
    assert_eq!(check(None).unwrap_err(), WORKSPACE_NOT_COMPLETE);
    assert!(check(Some("1")).is_ok());
    // The options come through the same gate (the variable is not set in this process).
    let error = capstan_daemon::run::daemon_options_with(&bed.project, bed.store())
        .err()
        .expect("refused");
    assert_eq!(error.message, WORKSPACE_NOT_COMPLETE);
    // A project without [repos] passes whatever its root is.
    bed.write(BASE);
    record_current(&bed.project, &bed.store()).unwrap();
    let plain = load_role_config_with(&bed.project, &bed.store()).unwrap();
    assert!(capstan_daemon::run::check_workspace_boot(&plain, &bed.project, None).is_ok());
}

#[test]
fn the_socket_verdict_matches_below_the_default_worktree_dir_and_not_outside_a_project() {
    let bed = initialised("ws-socket", &workspace_toml(API));
    let state = bed.project.join(".capstan").join("state");
    let socket = capstan_wire::control_socket(&state);
    let config = load_role_config_with(&bed.project, &bed.store()).unwrap();
    let workspace = config.workspace.unwrap();
    let inside = workspace.worktree_dir.join("api-feature");
    std::fs::create_dir_all(&inside).unwrap();
    assert_eq!(
        cstan_front::agent::socket_verdict(&inside, &socket),
        cstan_front::agent::Verdict::Match
    );
    let outside = bed.temp.join("trees").join("api-feature");
    std::fs::create_dir_all(&outside).unwrap();
    assert_eq!(
        cstan_front::agent::socket_verdict(&outside, &socket),
        cstan_front::agent::Verdict::None
    );
}
