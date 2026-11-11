//! Workspace mode of the launcher: a worker gets one change folder with one worktree per repo, all on one branch. Stub
//! host, real git repositories, a project root that is not a repository.
mod common;

use capstan_config::{RepoConfig, Workspace};
use capstan_launcher::api::{LauncherService, ReplaceResult, RepoBase, SetupOutcome, SpawnOptions};
use capstan_launcher::kernel::{Ledger, LedgerPort};
use capstan_launcher::launcher::Launcher;
use capstan_launcher::shared::{FrontEndSite, LauncherOptions, SetupRunner};
use common::engine::{git, git_args, Scenario, OWNER_CREDENTIAL};
use common::test_config;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

struct Ws {
    sc: Scenario,
    launcher: Launcher,
    changes: PathBuf,
    core: PathBuf,
    web: PathBuf,
    c1: String,
    w1: String,
    events: Arc<Mutex<Vec<Value>>>,
}

fn g(sc: &Scenario, cwd: &Path, args: &[&str]) -> String {
    git(&sc.world.real_git, cwd, &git_args(args))
        .unwrap_or_else(|e| panic!("git {args:?} in {}: {e}", cwd.display()))
        .trim()
        .to_string()
}

fn make_repo(sc: &Scenario, dir: &Path) -> String {
    std::fs::create_dir_all(dir).unwrap();
    g(sc, dir, &["init", "--quiet", "-b", "main"]);
    std::fs::write(dir.join("README.md"), "repo\n").unwrap();
    g(sc, dir, &["add", "-A"]);
    g(
        sc,
        dir,
        &["commit", "--quiet", "-m", "chore: initial commit"],
    );
    g(sc, dir, &["rev-parse", "HEAD"])
}

fn fixture(name: &str) -> Ws {
    fixture_with(name, None, |_| {})
}

fn fixture_with(
    name: &str,
    setup: Option<SetupRunner>,
    tweak: impl FnOnce(&mut capstan_config::RoleConfig),
) -> Ws {
    let sc = Scenario::new(name, json!({"maxWorkers": 4}));
    let scratch = PathBuf::from(&sc.world.scratch);
    let core = scratch.join("repos").join("core");
    let web = scratch.join("repos").join("web");
    let c1 = make_repo(&sc, &core);
    let w1 = make_repo(&sc, &web);
    // The project root is not a repository: only `.capstan` lives there.
    let root = scratch.join("wsroot");
    std::fs::create_dir_all(root.join(".capstan")).unwrap();
    let changes = scratch.join("changes");
    let repo = |name: &str, path: &Path| RepoConfig {
        name: name.into(),
        root: path.to_path_buf(),
        main_branch: "main".into(),
        verify: None,
        cache: None,
        setup: None,
    };
    let mut config = test_config(&sc.spec);
    config.workspace = Some(Workspace {
        root: root.clone(),
        repos: vec![repo("core", &core), repo("web", &web)],
        merge_order: vec!["core".into(), "web".into()],
        worktree_dir: changes.clone(),
        default_repo: "core".into(),
    });
    tweak(&mut config);
    let events: Arc<Mutex<Vec<Value>>> = Arc::default();
    let log_events = Arc::clone(&events);
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
        log: Some(Arc::new(move |event, details| {
            log_events
                .lock()
                .unwrap()
                .push(json!({"event": event, "details": details}));
        })),
        sync_roles: None,
        run_setup: setup,
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
    sc.take_calls();
    Ws {
        sc,
        launcher,
        changes,
        core,
        web,
        c1,
        w1,
        events,
    }
}

impl Ws {
    fn both(&self) -> SpawnOptions {
        SpawnOptions {
            repo_bases: Some(vec![
                RepoBase {
                    repo: "web".into(),
                    commit: self.w1.clone(),
                },
                RepoBase {
                    repo: "core".into(),
                    commit: self.c1.clone(),
                },
            ]),
            ..Default::default()
        }
    }

    fn spawn(&self, options: &SpawnOptions) -> Value {
        self.launcher
            .spawn("developer", options)
            .unwrap_or_else(|e| panic!("the spawn is refused: {e}"))
            .to_value()
    }

    fn rows(&self, agent: &str) -> Vec<Value> {
        let agent = agent.to_string();
        self.sc
            .ledger(move |core| core.list_agent_worktrees(&agent))
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    fn g(&self, cwd: &Path, args: &[&str]) -> String {
        g(&self.sc, cwd, args)
    }

    fn branch_exists(&self, repo: &Path, branch: &str) -> bool {
        git(
            &self.sc.world.real_git,
            repo,
            &git_args(&[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ]),
        )
        .is_ok()
    }

    fn events(&self, name: &str) -> Vec<Value> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e["event"] == name)
            .cloned()
            .collect()
    }
}

fn start_call(calls: &[Value]) -> Value {
    calls
        .iter()
        .find(|c| c["m"] == "startAgent")
        .cloned()
        .expect("an agent was started")
}

#[test]
fn two_repos_give_two_worktrees_on_one_branch_in_one_folder() {
    let ws = fixture("ws-two-repos");
    let spawned = ws.spawn(&ws.both());
    let agent = spawned["agentId"].as_str().unwrap();
    let folder = ws.changes.join(agent);
    assert_eq!(spawned["worktreePath"], folder.to_string_lossy().as_ref());
    let branch = spawned["branch"].as_str().unwrap();
    for (name, base) in [("core", &ws.c1), ("web", &ws.w1)] {
        let tree = folder.join(name);
        assert_eq!(ws.g(&tree, &["rev-parse", "--abbrev-ref", "HEAD"]), branch);
        assert_eq!(&ws.g(&tree, &["rev-parse", "HEAD"]), base);
    }
    // The folder itself is no git work tree.
    assert!(git(
        &ws.sc.world.real_git,
        &folder,
        &git_args(&["rev-parse", "--is-inside-work-tree"])
    )
    .is_err_and(|_| !folder.join(".git").exists()));
    let rows = ws.rows(agent);
    assert_eq!(
        rows.iter()
            .map(|r| r["repo"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["core", "web"]
    );
    assert!(rows.iter().all(|r| r["branch"] == branch));
    let calls = ws.sc.take_calls();
    let tab = calls.iter().find(|c| c["m"] == "createTab").expect("a tab");
    assert_eq!(
        tab["a"][0]["cwd"],
        folder
            .to_string_lossy()
            .replace(&ws.sc.world.scratch, "<tmp>")
            .as_str()
    );
    // One --add-dir per worktree, the pane environment holds the socket and no preview switch.
    let start = start_call(&calls);
    let args: Vec<&str> = start["a"][0]["args"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let added: Vec<&str> = args
        .windows(2)
        .filter(|w| w[0] == "--add-dir")
        .map(|w| w[1])
        .collect();
    assert_eq!(added.len(), 2);
    assert!(added[0].ends_with(&format!("{agent}/core")));
    assert!(added[1].ends_with(&format!("{agent}/web")));
    let environment = &start["a"][0]["environment"];
    assert!(environment["CAPSTAN_SOCKET"].is_string());
    // The seed names each repo and says to run git inside each.
    let prompt = ws.sc.prompts().pop().unwrap();
    assert!(prompt.contains(&format!("{agent}/core")) && prompt.contains(&format!("{agent}/web")));
    assert!(prompt.contains("Run git inside each repo") && prompt.contains("CLAUDE.md"));
}

#[test]
fn no_repo_bases_spawns_at_the_main_tips_of_the_default_repo() {
    let ws = fixture("ws-main-tips");
    std::fs::write(ws.core.join("later.txt"), "x\n").unwrap();
    ws.g(&ws.core, &["add", "-A"]);
    ws.g(&ws.core, &["commit", "--quiet", "-m", "feat: later"]);
    let tip = ws.g(&ws.core, &["rev-parse", "main"]);
    let spawned = ws.spawn(&SpawnOptions::default());
    let agent = spawned["agentId"].as_str().unwrap();
    let rows = ws.rows(agent);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["repo"], "core");
    assert_eq!(rows[0]["baseSha"], tip.as_str());
    assert_ne!(tip, ws.c1);
}

#[test]
fn a_name_taken_in_one_repo_gets_the_suffix_in_both_also_when_remote_only() {
    let ws = fixture("ws-taken");
    // The first spawn of the next agent id would be developer-1; the name it asks for is taken in web only, as a
    // remote-tracking ref.
    let wanted = "chore/developer-1-alpha";
    ws.g(
        &ws.web,
        &[
            "update-ref",
            &format!("refs/remotes/origin/{wanted}"),
            &ws.w1,
        ],
    );
    let first = ws.spawn(&SpawnOptions {
        title: Some("alpha".into()),
        ..ws.both()
    });
    let name = first["branch"].as_str().unwrap();
    assert_eq!(name, format!("{wanted}-2"));
    assert!(ws.branch_exists(&ws.core, name) && ws.branch_exists(&ws.web, name));
    assert!(!ws.branch_exists(&ws.core, wanted) && !ws.branch_exists(&ws.web, wanted));
}

#[test]
fn a_failing_add_in_the_second_repo_leaves_nothing() {
    let ws = fixture("ws-add-fails");
    // A directory named like the worktree makes the second add fail.
    let agent_guess = "developer-1";
    let blocker = ws.changes.join(agent_guess).join("web");
    std::fs::create_dir_all(&blocker).unwrap();
    std::fs::write(blocker.join("keep.txt"), "x").unwrap();
    let error = ws
        .launcher
        .spawn("developer", &ws.both())
        .expect_err("the spawn fails");
    assert!(
        error.message.contains("web") || error.message.contains("change folder"),
        "{error}"
    );
    assert!(ws.rows(agent_guess).is_empty());
    assert!(
        !ws.core.join(".git").join("worktrees").exists()
            || std::fs::read_dir(ws.core.join(".git").join("worktrees"))
                .unwrap()
                .count()
                == 0
    );
}

#[test]
fn a_failing_setup_in_the_second_repo_removes_every_worktree_and_new_branch() {
    let calls = Arc::new(Mutex::new(0usize));
    let counter = Arc::clone(&calls);
    let setup: SetupRunner = Arc::new(move |_, _, _| {
        let mut n = counter.lock().unwrap();
        *n += 1;
        if *n == 2 {
            SetupOutcome::Failed {
                exit_code: Some(3),
                output: "boom".into(),
            }
        } else {
            SetupOutcome::Ok
        }
    });
    let ws = fixture_with("ws-setup-fails", Some(setup), |config| {
        config.worktree = Some(capstan_config::Worktree {
            setup: Some("prepare".into()),
            setup_timeout_seconds: 30,
            teardown: None,
            teardown_timeout_seconds: None,
        });
    });
    let error = ws
        .launcher
        .spawn("developer", &ws.both())
        .expect_err("the setup fails");
    assert!(error.message.contains("repo web"), "{error}");
    let listed = |repo: &Path| ws.g(repo, &["worktree", "list", "--porcelain"]);
    assert_eq!(listed(&ws.core).matches("worktree ").count(), 1);
    assert_eq!(listed(&ws.web).matches("worktree ").count(), 1);
    assert_eq!(ws.g(&ws.core, &["branch", "--list"]).lines().count(), 1);
    assert_eq!(ws.g(&ws.web, &["branch", "--list"]).lines().count(), 1);
    assert!(std::fs::read_dir(&ws.changes)
        .map(|mut d| d.next().is_none())
        .unwrap_or(true));
}

#[test]
fn a_package_repo_and_a_repo_base_outside_the_config_are_refused_before_any_worktree() {
    let ws = fixture("ws-unknown");
    let error = ws
        .launcher
        .spawn(
            "developer",
            &SpawnOptions {
                repo_bases: Some(vec![RepoBase {
                    repo: "ghost".into(),
                    commit: ws.c1.clone(),
                }]),
                ..Default::default()
            },
        )
        .expect_err("refused");
    assert_eq!(
        error.message,
        "repo_bases names repo ghost, which is not in [repos]"
    );
    assert!(!ws.changes.exists() || std::fs::read_dir(&ws.changes).unwrap().next().is_none());
}

#[test]
fn a_package_naming_a_repo_outside_the_config_is_refused_before_any_worktree() {
    let ws = fixture("ws-package-ghost");
    let body = r#"{"summary":"s","packages":[{"id":"api","title":"Api","repos":["core","ghost"],"owns":["core:src/api","ghost:src/api"],"acceptance":["x"],"estimate_hours":1}],"risks":[],"integration_order":["api"]}"#;
    let stored =
        serde_json::to_string(&capstan_kernel::plan_body::parse_plan_body(body, 8).unwrap())
            .unwrap();
    let base = ws.c1.clone();
    // An approved plan, seeded through the Core handle of the ledger.
    ws.sc.ledger(move |core| {
        use capstan_kernel::helpers::execute;
        let db = &core.kernel().database;
        execute(
            db,
            "INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, architect_agent_id, current_revision, approved_revision, created_at, updated_at)
             VALUES ('proj1', 'plan-ws', (SELECT COALESCE(MAX(sequence), 0) + 1 FROM plans), 'T', 'normal', 'draft', 'x', 'pm-1', 0, NULL, '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')",
            (),
        )?;
        execute(
            db,
            "INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at)
             VALUES ('proj1', 'plan-ws', 1, ?, ?, ?, 'pm-1', (SELECT actor_id FROM agents WHERE agent_id = 'pm-1'), '2026-01-01T00:00:00.000Z')",
            [base.as_str(), stored.as_str(), "c".repeat(64).as_str()],
        )?;
        execute(
            db,
            "UPDATE plans SET current_revision = 1, state = 'approved', approved_revision = 1 WHERE plan_id = 'plan-ws'",
            (),
        )
        .map(drop)
    });
    let error = ws
        .launcher
        .spawn(
            "developer",
            &SpawnOptions {
                task: Some("plan-ws/api".into()),
                ..Default::default()
            },
        )
        .expect_err("refused");
    assert_eq!(error.code, "unknown_repo");
    assert!(error
        .message
        .contains("names repo ghost, which is not in [repos]"));
    assert!(!ws.changes.exists() || std::fs::read_dir(&ws.changes).unwrap().next().is_none());
}

#[test]
fn a_host_that_cannot_open_a_tab_at_a_folder_refuses_and_leaves_nothing() {
    let ws = fixture("ws-unsupported");
    ws.sc.world.stub.apply(&json!({
        "do": "fail", "method": "createTab",
        "error": {"kind": "unsupported_host", "message": "no"}
    }));
    let error = ws
        .launcher
        .spawn("developer", &ws.both())
        .expect_err("refused");
    assert_eq!(error.code, "host_unsupported");
    assert_eq!(
        error.message,
        "this host cannot open a worker at a folder; workspace mode needs it"
    );
    assert_eq!(
        ws.g(&ws.core, &["worktree", "list", "--porcelain"])
            .matches("worktree ")
            .count(),
        1
    );
}

#[test]
fn release_removes_unused_worktrees_and_keeps_those_with_commits() {
    let ws = fixture("ws-release");
    let spawned = ws.spawn(&ws.both());
    let agent = spawned["agentId"].as_str().unwrap().to_string();
    let folder = ws.changes.join(&agent);
    let branch = spawned["branch"].as_str().unwrap().to_string();
    // One commit in web only.
    std::fs::write(folder.join("web").join("w.txt"), "w\n").unwrap();
    ws.g(&folder.join("web"), &["add", "-A"]);
    ws.g(
        &folder.join("web"),
        &["commit", "--quiet", "-m", "feat: web"],
    );
    let released = ws.launcher.release(&agent).expect("released").to_value();
    assert_eq!(released["paneClosed"], true);
    assert_eq!(released["worktreeRemoved"], false);
    assert_eq!(released["branchKept"], true);
    assert!(!folder.join("core").exists(), "the unused worktree is gone");
    assert!(
        !ws.branch_exists(&ws.core, &branch),
        "its new branch is gone"
    );
    assert!(
        folder.join("web").exists(),
        "the worktree with a commit stays"
    );
    assert!(ws.branch_exists(&ws.web, &branch));
    assert!(ws.rows(&agent).is_empty());
    assert!(ws.sc.take_calls().iter().any(|c| c["m"] == "closePane"));
}

#[test]
fn release_of_an_unused_worker_removes_the_folder() {
    let ws = fixture("ws-release-all");
    let spawned = ws.spawn(&ws.both());
    let agent = spawned["agentId"].as_str().unwrap().to_string();
    ws.launcher.release(&agent).expect("released");
    assert!(!ws.changes.join(&agent).exists());
    assert!(ws.rows(&agent).is_empty());
}

#[test]
fn a_leftover_folder_is_reused_unless_a_worktree_has_commits() {
    let ws = fixture("ws-stale");
    // A crashed spawn of the next agent id left a worktree without commits.
    let id = "developer-1";
    let stale = ws.changes.join(id);
    std::fs::create_dir_all(&stale).unwrap();
    ws.g(
        &ws.core,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "left-over",
            stale.join("core").to_str().unwrap(),
            "main",
        ],
    );
    let spawned = ws.spawn(&ws.both());
    assert_eq!(spawned["agentId"], id);
    assert!(!ws.branch_exists(&ws.core, "left-over"));
    ws.launcher.release(id).expect("released");
    // Again, now with a commit in the leftover.
    ws.g(
        &ws.core,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "left-over",
            stale.join("core").to_str().unwrap(),
            "main",
        ],
    );
    std::fs::write(stale.join("core").join("c.txt"), "c\n").unwrap();
    ws.g(&stale.join("core"), &["add", "-A"]);
    ws.g(&stale.join("core"), &["commit", "--quiet", "-m", "feat: c"]);
    let _ = &ws.launcher; // the ids move on: the next agent is developer-2, so name the folder after it
    let next = ws.changes.join("developer-2");
    std::fs::create_dir_all(&next).unwrap();
    ws.g(
        &ws.core,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "left-two",
            next.join("core").to_str().unwrap(),
            "main",
        ],
    );
    std::fs::write(next.join("core").join("d.txt"), "d\n").unwrap();
    ws.g(&next.join("core"), &["add", "-A"]);
    ws.g(&next.join("core"), &["commit", "--quiet", "-m", "feat: d"]);
    let error = ws
        .launcher
        .spawn("developer", &ws.both())
        .expect_err("in use");
    assert_eq!(error.code, "change_folder_in_use");
}

#[test]
fn replace_keeps_the_folder_and_the_branches() {
    let ws = fixture("ws-replace");
    let spawned = ws.spawn(&ws.both());
    let agent = spawned["agentId"].as_str().unwrap().to_string();
    let branch = spawned["branch"].as_str().unwrap().to_string();
    let folder = ws.changes.join(&agent);
    std::fs::write(folder.join("core").join("c.txt"), "c\n").unwrap();
    ws.g(&folder.join("core"), &["add", "-A"]);
    ws.g(
        &folder.join("core"),
        &["commit", "--quiet", "-m", "feat: c"],
    );
    let tip = ws.g(&folder.join("core"), &["rev-parse", "HEAD"]);
    let replaced = ws.launcher.replace(&agent).expect("replaced");
    let ReplaceResult::Replaced(done) = replaced else {
        panic!("not replaced");
    };
    assert_eq!(done.spawn.branch, branch);
    assert_eq!(done.spawn.worktree_path, folder.to_string_lossy().as_ref());
    assert_eq!(ws.g(&folder.join("core"), &["rev-parse", "HEAD"]), tip);
    assert_eq!(ws.rows(&done.spawn.agent_id).len(), 2);
    assert!(ws.rows(&agent).is_empty());
    // Releasing the predecessor again must not take the successor's worktrees.
    let _ = ws.launcher.release(&agent);
    assert!(folder.join("core").exists() && folder.join("web").exists());
    assert_eq!(ws.rows(&done.spawn.agent_id).len(), 2);
}

#[test]
fn a_release_skips_a_tree_another_active_agent_holds() {
    let ws = fixture("ws-held");
    let spawned = ws.spawn(&ws.both());
    let agent = spawned["agentId"].as_str().unwrap().to_string();
    let folder = ws.changes.join(&agent);
    let other = ws.spawn(&SpawnOptions::default());
    let other_id = other["agentId"].as_str().unwrap().to_string();
    // The second agent's row for core points at the first agent's tree, as a half-finished replace would leave it.
    let (path, branch, base) = (
        folder.join("core").to_string_lossy().into_owned(),
        spawned["branch"].as_str().unwrap().to_string(),
        ws.c1.clone(),
    );
    let holder = other_id.clone();
    ws.sc.ledger(move |core| {
        use capstan_kernel::helpers::execute;
        execute(
            &core.kernel().database,
            "UPDATE agent_worktrees SET worktree_path = ?, branch = ?, base_sha = ? WHERE agent_id = ? AND repo = 'core' AND removed_at IS NULL",
            [path.as_str(), branch.as_str(), base.as_str(), holder.as_str()],
        )
        .map(drop)
    });
    ws.launcher.release(&agent).expect("released");
    assert!(folder.join("core").exists());
    assert!(!ws.events("worktree_held_by_other").is_empty());
}

#[test]
fn adopt_after_a_restart_keeps_the_rows_and_release_still_works() {
    let mut ws = fixture("ws-adopt");
    let spawned = ws.spawn(&ws.both());
    let agent = spawned["agentId"].as_str().unwrap().to_string();
    ws.sc.reopen();
    ws.launcher.adopt_all().expect("adopted");
    assert_eq!(ws.rows(&agent).len(), 2);
    ws.launcher.release(&agent).expect("released");
    assert!(ws.rows(&agent).is_empty());
    assert!(!ws.changes.join(&agent).exists());
}

#[test]
fn a_rename_that_fails_in_the_second_repo_restores_the_first_and_the_ledger() {
    let ws = fixture("ws-rename");
    let spawned = ws.spawn(&ws.both());
    let agent = spawned["agentId"].as_str().unwrap().to_string();
    let old = spawned["branch"].as_str().unwrap().to_string();
    let wanted = capstan_launcher::shared::worker_branch_name("feat", "PM-1", "PM-1");
    // A lock file makes git refuse the new name in web only.
    let lock = ws
        .web
        .join(".git")
        .join("refs")
        .join("heads")
        .join(format!("{wanted}.lock"));
    std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
    std::fs::write(&lock, "").unwrap();
    let result = ws
        .launcher
        .rename_branch_for_task(&agent, "PM-1")
        .expect("answers");
    assert!(!result.renamed);
    let note = result.note.unwrap();
    assert!(note.contains("repo web"), "{note}");
    assert!(!note.contains("could not rename back"), "{note}");
    assert!(ws.branch_exists(&ws.core, &old) && ws.branch_exists(&ws.web, &old));
    assert!(!ws.branch_exists(&ws.core, &wanted));
    assert!(ws
        .rows(&agent)
        .iter()
        .all(|row| row["branch"] == old.as_str()));
    // Without the lock the rename succeeds in both repos and in the ledger, once.
    std::fs::remove_file(&lock).unwrap();
    let result = ws
        .launcher
        .rename_branch_for_task(&agent, "PM-1")
        .expect("answers");
    assert!(result.renamed, "{:?}", result.note);
    let renamed = result.branch.unwrap();
    assert!(ws.branch_exists(&ws.core, &renamed) && ws.branch_exists(&ws.web, &renamed));
    assert!(!ws.branch_exists(&ws.core, &old));
    assert!(ws
        .rows(&agent)
        .iter()
        .all(|row| row["branch"] == renamed.as_str()));
}

#[test]
fn a_failing_removal_during_the_rollback_is_in_the_refusal() {
    let calls = Arc::new(Mutex::new(0usize));
    let counter = Arc::clone(&calls);
    let real = common::engine::git_environment();
    let setup: SetupRunner = Arc::new(move |_, cwd, _| {
        let mut n = counter.lock().unwrap();
        *n += 1;
        if *n == 1 {
            // Lock the first worktree so removing it fails later.
            let done = std::process::Command::new("git")
                .args(["worktree", "lock", cwd])
                .current_dir(cwd)
                .env_clear()
                .envs(real.clone())
                .status()
                .unwrap();
            assert!(done.success());
            SetupOutcome::Ok
        } else {
            SetupOutcome::Failed {
                exit_code: Some(1),
                output: "no".into(),
            }
        }
    });
    let ws = fixture_with("ws-removal-fails", Some(setup), |config| {
        config.worktree = Some(capstan_config::Worktree {
            setup: Some("prepare".into()),
            setup_timeout_seconds: 30,
            teardown: None,
            teardown_timeout_seconds: None,
        });
    });
    let error = ws
        .launcher
        .spawn("developer", &ws.both())
        .expect_err("fails");
    assert!(
        error.message.contains("a worktree could not be removed"),
        "{error}"
    );
    assert!(error.message.contains("repo core"), "{error}");
}

#[test]
fn an_agent_without_rows_is_released_through_its_pane_row() {
    let ws = fixture("ws-no-rows");
    let spawned = ws.spawn(&ws.both());
    let agent = spawned["agentId"].as_str().unwrap().to_string();
    for repo in ["core", "web"] {
        let (agent, repo) = (agent.clone(), repo.to_string());
        ws.sc.ledger(move |core| {
            let ctx = Scenario::owner(core);
            core.remove_agent_worktree(&ctx, &json!({"agentId": agent, "repo": repo}))
                .map(|_| ())
        });
    }
    let released = ws.launcher.release(&agent).expect("released").to_value();
    assert_eq!(released["paneClosed"], true);
    assert!(!ws.sc.ledger({
        let agent = agent.clone();
        move |core| {
            let panes = core.agent_panes(OWNER_CREDENTIAL)?;
            Ok(panes
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["agentId"] == agent.as_str()))
        }
    }));
}

#[test]
fn write_dirs_give_one_folder_per_repo_worktree() {
    let ws = fixture_with("ws-write-dirs", None, |config| {
        for role in config.roles.iter_mut().filter(|r| r.name == "developer") {
            role.write_dirs = vec!["docs".into()];
        }
    });
    ws.spawn(&ws.both());
    let start = start_call(&ws.sc.take_calls());
    let settings = start["a"][0]["args"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .find(|a| a.contains("guard-write"))
        .expect("the write guard")
        .to_string();
    assert!(
        settings.contains("/core/docs") && settings.contains("/web/docs"),
        "{settings}"
    );
}

#[test]
fn the_ledger_lists_each_worktree_of_an_agent() {
    let ws = fixture("ws-protected");
    let spawned = ws.spawn(&ws.both());
    let agent = spawned["agentId"].as_str().unwrap();
    let paths: Vec<String> = ws
        .rows(agent)
        .iter()
        .map(|row| row["worktreePath"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        paths,
        [
            ws.changes.join(agent).join("core").to_string_lossy(),
            ws.changes.join(agent).join("web").to_string_lossy()
        ]
    );
}

#[test]
fn idle_roots_are_false_while_a_change_folder_holds_a_worktree() {
    let ws = fixture("ws-idle");
    let roots = vec![ws.core.clone(), ws.web.clone()];
    assert!(capstan_cache::is_idle_roots(&roots).unwrap());
    let spawned = ws.spawn(&ws.both());
    assert!(!capstan_cache::is_idle_roots(&roots).unwrap());
    ws.launcher
        .release(spawned["agentId"].as_str().unwrap())
        .unwrap();
    assert!(capstan_cache::is_idle_roots(&roots).unwrap());
    let _ = ws.events("none");
}

#[test]
fn a_stray_file_in_a_leftover_folder_keeps_it_in_use() {
    let ws = fixture("ws-stray");
    let stale = ws.changes.join("developer-1");
    std::fs::create_dir_all(&stale).unwrap();
    std::fs::write(stale.join("notes.txt"), "mine\n").unwrap();
    let error = ws
        .launcher
        .spawn("developer", &ws.both())
        .expect_err("in use");
    assert_eq!(error.code, "change_folder_in_use");
    assert!(stale.join("notes.txt").exists());
}
