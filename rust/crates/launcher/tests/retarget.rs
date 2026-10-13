//! `retarget_review`: an active reviewer's worktree moves to another commit, over real git.
mod common;

use capstan_launcher::api::{LauncherService, SetupOutcome, SpawnOptions, StubLauncher};
use capstan_launcher::kernel::{Ledger, LedgerPort};
use capstan_launcher::launcher::Launcher;
use capstan_launcher::shared::{FrontEndSite, LauncherOptions, SetupRunner};
use common::engine::{git, git_args, Scenario, OWNER_CREDENTIAL, ROLES};
use common::ledger::context;
use common::{hash_of, test_config};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

struct Fixture {
    sc: Scenario,
    launcher: Launcher,
    /// The worktree and agent of the reviewer spawned at `a`.
    agent: String,
    path: String,
    branch: String,
    a: String,
    b: String,
}

fn run(sc: &Scenario, cwd: &Path, args: &[&str]) -> String {
    git(&sc.world.real_git, cwd, &git_args(args))
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"))
        .trim()
        .to_string()
}

/// A project with commits A (README, .gitignore) and B (adds `b.txt`), a Verifier role beside the standard ones, and a
/// reviewer spawned at A with a `[worktree].setup` step that writes untracked files.
fn fixture(name: &str) -> Fixture {
    let sc = Scenario::new(
        name,
        json!({"maxWorkers": 3, "worktree": {"setup": "prepare"}}),
    );
    let root = sc.project_root.clone();
    std::fs::write(root.join(".gitignore"), "build/\n").unwrap();
    run(&sc, &root, &["add", "-A"]);
    run(
        &sc,
        &root,
        &["commit", "--quiet", "-m", "chore: ignore build"],
    );
    let a = run(&sc, &root, &["rev-parse", "HEAD"]);
    std::fs::write(root.join("b.txt"), "b\n").unwrap();
    run(&sc, &root, &["add", "-A"]);
    run(&sc, &root, &["commit", "--quiet", "-m", "feat: add b"]);
    let b = run(&sc, &root, &["rev-parse", "HEAD"]);

    let mut desired: Vec<Value> = ROLES
        .iter()
        .map(|(role, kind)| json!({"name": role, "kind": kind, "host": "claude", "configHash": hash_of(role)}))
        .collect();
    desired.push(json!({"name": "reviewer", "kind": "Verifier", "host": "claude", "configHash": hash_of("reviewer")}));
    sc.ledger(move |core| {
        let ctx = context(core, OWNER_CREDENTIAL)?;
        core.sync_role_definitions(&ctx, &Value::Array(desired))
    });

    let mut config = test_config(&sc.spec);
    let mut reviewer = config
        .roles
        .iter()
        .find(|r| r.name == "developer")
        .cloned()
        .expect("a developer role");
    reviewer.name = "reviewer".into();
    reviewer.kind = "Verifier".into();
    reviewer.config_hash = hash_of("reviewer");
    config.roles.push(reviewer);
    // The setup writes files the way a project's setup does: an untracked host file and an ignored-or-not dependency tree.
    let setup: SetupRunner = Arc::new(|_, cwd, _| {
        let cwd = Path::new(cwd);
        std::fs::create_dir_all(cwd.join("node_modules/dep")).unwrap();
        std::fs::write(cwd.join("node_modules/dep/index.js"), "x").unwrap();
        std::fs::write(cwd.join(".env.local"), "SECRET=1").unwrap();
        SetupOutcome::Ok
    });
    let launcher = Launcher::new(LauncherOptions {
        ledger: Ledger::new(
            Arc::clone(&sc.world.kernel) as Arc<dyn LedgerPort>,
            OWNER_CREDENTIAL,
        ),
        adapter: sc.world.stub.clone(),
        config,
        project_root: root.to_string_lossy().into_owned(),
        project_id: "proj1".into(),
        cli_path: "/opt/capstan/cli.js".into(),
        socket_path: sc
            .state_dir
            .join("control.sock")
            .to_string_lossy()
            .into_owned(),
        credential: OWNER_CREDENTIAL.into(),
        node_path: Some("/usr/bin/node".into()),
        base_environment: HashMap::from([("PATH".to_string(), std::env::var("PATH").unwrap())]),
        git: None,
        now: None,
        log: None,
        sync_roles: None,
        run_setup: Some(setup),
        sleep: None,
        run_teardown: None,
        site: Some(FrontEndSite {
            env: HashMap::new(),
            exec_path: "/usr/bin/node".into(),
            sea: false,
            cstan: "/usr/bin/cstan".into(),
        }),
    });
    launcher.launch_pm().expect("the PM launches");
    let spawned = launcher
        .spawn(
            "reviewer",
            &SpawnOptions {
                base_sha: Some(a.clone()),
                review_target: Some("report-1".into()),
                ..Default::default()
            },
        )
        .expect("the reviewer spawns")
        .to_value();
    Fixture {
        launcher,
        agent: spawned["agentId"].as_str().unwrap().to_string(),
        path: spawned["worktreePath"].as_str().unwrap().to_string(),
        branch: spawned["branch"].as_str().unwrap().to_string(),
        sc,
        a,
        b,
    }
}

fn head(f: &Fixture) -> String {
    run(&f.sc, Path::new(&f.path), &["rev-parse", "HEAD"])
}

fn branch_exists(f: &Fixture) -> bool {
    git(
        &f.sc.world.real_git,
        &f.sc.project_root,
        &git_args(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{}", f.branch),
        ]),
    )
    .is_ok()
}

#[test]
fn a_reviewer_worktree_moves_to_the_new_commit() {
    let f = fixture("retarget-moves");
    assert_eq!(head(&f), f.a);
    let result = f
        .launcher
        .retarget_review(&f.agent, &f.b, "report-2")
        .expect("retargets");
    assert_eq!(
        result.to_value(),
        json!({
            "agentId": f.agent, "worktreePath": f.path, "branch": f.branch,
            "fromSha": f.a, "toSha": f.b,
        })
    );
    assert_eq!(head(&f), f.b);
    assert_eq!(
        run(
            &f.sc,
            Path::new(&f.path),
            &["symbolic-ref", "--short", "HEAD"]
        ),
        f.branch
    );
    assert_eq!(
        run(
            &f.sc,
            Path::new(&f.path),
            &["status", "--porcelain", "--untracked-files=no"]
        ),
        ""
    );
    assert!(Path::new(&f.path).join("b.txt").exists());
    // The setup output (untracked) survived: a worktree prepared by the spawn path is retargetable.
    assert!(Path::new(&f.path).join(".env.local").exists());
    assert!(Path::new(&f.path)
        .join("node_modules/dep/index.js")
        .exists());
}

#[test]
fn tracked_changes_refuse_and_untracked_and_ignored_files_do_not() {
    let f = fixture("retarget-dirty");
    let readme = Path::new(&f.path).join("README.md");
    std::fs::write(&readme, "edited\n").unwrap();
    let error = f.launcher.retarget_review(&f.agent, &f.b, "t").unwrap_err();
    assert_eq!(error.code, "worktree_dirty");
    assert_eq!(head(&f), f.a);
    assert_eq!(std::fs::read_to_string(&readme).unwrap(), "edited\n");
    // Staged only.
    run(&f.sc, Path::new(&f.path), &["add", "README.md"]);
    std::fs::write(&readme, "edited\n").unwrap();
    let error = f.launcher.retarget_review(&f.agent, &f.b, "t").unwrap_err();
    assert_eq!(error.code, "worktree_dirty");
    assert_eq!(head(&f), f.a);
    run(
        &f.sc,
        Path::new(&f.path),
        &["checkout", "HEAD", "--", "README.md"],
    );
    // Untracked and ignored files do not block and stay.
    std::fs::write(Path::new(&f.path).join("untracked.txt"), "u").unwrap();
    std::fs::create_dir_all(Path::new(&f.path).join("build")).unwrap();
    std::fs::write(Path::new(&f.path).join("build/out.o"), "o").unwrap();
    f.launcher
        .retarget_review(&f.agent, &f.b, "t")
        .expect("retargets");
    assert_eq!(head(&f), f.b);
    assert!(Path::new(&f.path).join("untracked.txt").exists());
    assert!(Path::new(&f.path).join("build/out.o").exists());
}

#[test]
fn refusals_carry_their_codes() {
    let f = fixture("retarget-refusals");
    let code = |agent: &str, sha: &str| {
        f.launcher
            .retarget_review(agent, sha, "t")
            .unwrap_err()
            .code
    };
    assert_eq!(code("nobody-9", &f.b), "unknown_agent");
    let developer = f
        .launcher
        .spawn("developer", &SpawnOptions::default())
        .expect("a developer spawns")
        .to_value();
    assert_eq!(
        code(developer["agentId"].as_str().unwrap(), &f.b),
        "not_reviewer"
    );
    assert_eq!(code(&f.agent, &"1".repeat(40)), "commit_missing");
    assert_eq!(code(&f.agent, "not-a-sha"), "commit_missing");
    assert_eq!(head(&f), f.a);
    std::fs::remove_dir_all(&f.path).unwrap();
    assert_eq!(code(&f.agent, &f.b), "no_worktree");
}

#[test]
fn release_after_a_retarget_removes_the_worktree_and_branch() {
    let f = fixture("retarget-release");
    f.launcher
        .retarget_review(&f.agent, &f.b, "t")
        .expect("retargets");
    assert!(branch_exists(&f));
    let released = f.launcher.release(&f.agent).expect("releases").to_value();
    assert_eq!(released["worktreeRemoved"], true, "{released}");
    assert_eq!(released["branchKept"], false, "{released}");
    assert!(!Path::new(&f.path).exists());
    assert!(!branch_exists(&f));
}

#[test]
fn the_stub_records_the_call_and_fails_on_request() {
    let stub = StubLauncher::new();
    let sha = "a".repeat(40);
    let result = stub
        .retarget_review("reviewer-1", &sha, "report-3")
        .unwrap();
    assert_eq!(result.to_sha, sha);
    assert_eq!(
        stub.calls(),
        vec![format!("retarget_review reviewer-1 {sha} report-3")]
    );
    stub.fail_next(
        "retarget_review",
        capstan_launcher::api::LauncherError::new("worktree_dirty", "dirty"),
    );
    assert_eq!(
        stub.retarget_review("reviewer-1", &sha, "report-3")
            .unwrap_err()
            .code,
        "worktree_dirty"
    );
}
