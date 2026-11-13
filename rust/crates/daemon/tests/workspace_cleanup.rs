//! Cleanup per repo (plan-184): one cleanup per repo with candidates, and the safe deletion of the worktrees of a repo
//! that left the configuration. Two real repos, real worktrees, a real ledger; the commands go through `handle_frame`.
#![allow(dead_code)]

mod common;

use capstan_config::TrustStore;
use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::handlers::cleanup::BEFORE_DELETE;
use capstan_daemon::handlers::handle_frame;
use capstan_daemon::handlers::shared::AbortSignal;
use capstan_daemon::handlers::MAX_RESPONSE_BYTES;
use capstan_daemon::ports::ProjectGit;
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::Core;
use common::project::{build_repo, write_project, Identity};
use common::{private_tempdir, seeded_env};
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

/// The tests share the process-wide hook, so they run one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

fn git(cwd: &Path, args: &[&str]) -> String {
    let path = std::env::var("PATH").unwrap_or_default();
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs([
            ("PATH", path.as_str()),
            ("LC_ALL", "C"),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_AUTHOR_NAME", "Test"),
            ("GIT_AUTHOR_EMAIL", "test@example.com"),
            ("GIT_COMMITTER_NAME", "Test"),
            ("GIT_COMMITTER_EMAIL", "test@example.com"),
        ])
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn make_repo(dir: &Path) -> String {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "--quiet", "-b", "main"]);
    std::fs::write(dir.join("f.txt"), "1\n").unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "--quiet", "-m", "chore: initial commit"]);
    git(dir, &["rev-parse", "HEAD"])
}

struct World {
    _dir: common::PrivateDir,
    scratch: common::PrivateDir,
    deps: Deps,
    operator: String,
    /// The real worktree folder; the configuration names it through a symlink.
    changes: PathBuf,
    link: PathBuf,
    database: PathBuf,
}

impl World {
    fn new(agents: &[&str]) -> World {
        World::build(agents, true)
    }

    /// `workspace` false writes a project with no `[repos]`.
    fn build(agents: &[&str], workspace: bool) -> World {
        let dir = private_tempdir();
        let scratch = private_tempdir();
        let repos = scratch.path().join("repos");
        for name in ["core", "web", "old"] {
            make_repo(&repos.join(name));
        }
        let changes = scratch.path().join("changes");
        std::fs::create_dir_all(&changes).unwrap();
        let link = scratch.path().join("changes-link");
        std::os::unix::fs::symlink(&changes, &link).unwrap();
        let identity = Identity {
            project_id: "pwsclean".into(),
            name: "Workspace Cleanup".into(),
            credential: format!("operator-{}", "w".repeat(40)),
        };
        let state = write_project(dir.path(), &identity);
        let config = format!(
            r#"schema_version = 1

[hosts.claude]
kind = "claude"

[architect]
enabled = false

[roles.pm]
kind = "PM"
host = "claude"

[roles.developer]
kind = "Developer"
host = "claude"

{ws}"#,
            ws = if workspace {
                format!(
                    r#"[repos.core]
path = "{r}/core"
main_branch = "main"

[repos.web]
path = "{r}/web"
main_branch = "main"

[workspace]
default_repo = "core"
merge_order = ["core", "web"]
worktree_dir = "{c}"
"#,
                    r = repos.display(),
                    c = link.display()
                )
            } else {
                String::new()
            }
        );
        let file = dir.path().join("capstan.toml");
        std::fs::write(&file, config).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        let trust: TrustStore = common::kept_trust_store();
        common::trusted_project(dir.path(), &trust);
        build_repo(dir.path(), &json!({"files": {"a.txt": "a\n"}}));
        let database = capstan_ledger::resolve_database_path(&state).unwrap();
        capstan_ledger::open_database(&database, &capstan_ledger::OpenOptions::default()).unwrap();
        let project = identity.initial_project();
        let kernel = {
            let (state, project, root) = (state.clone(), project.clone(), dir.path().to_path_buf());
            KernelHandle::spawn(move || {
                Core::open(
                    &state,
                    &project,
                    &KernelOptions {
                        workspace_root: Some(root),
                        ..KernelOptions::default()
                    },
                    seeded_env("wscleanup"),
                )
            })
            .expect("the kernel opens")
        };
        let sink: Logger = Arc::new(|_entry: &LogEntry| {});
        let mut options = DaemonOptions::new(state, project, dir.path().to_path_buf());
        options.trust_store = trust.clone();
        options.capstan = Some(Arc::new(
            capstan_config::load_role_config_with(dir.path(), &trust).unwrap(),
        ));
        let mut deps = Deps::new(kernel, options, sink);
        deps.git = Arc::new(ProjectGit::new(dir.path()));
        let world = World {
            _dir: dir,
            scratch,
            deps,
            operator: identity.credential.clone(),
            changes,
            link,
            database,
        };
        world.step(
            "syncRoleDefinitions",
            json!([[{"name": "developer", "kind": "Developer", "host": "claude", "configHash": "a".repeat(64)}]]),
        );
        for agent in agents {
            let seat = format!("seat-{agent}");
            world.step(
                "createSeat",
                json!([{"seatId": seat, "name": seat, "role": "Developer"}]),
            );
            let actor = world.step(
                "createActor",
                json!([{"displayName": agent, "role": "Developer", "seatId": seat}]),
            );
            world.step(
                "registerAgent",
                json!([{"agentId": agent, "roleName": "developer", "seatId": seat, "actorId": actor["actorId"]}]),
            );
        }
        world
    }

    fn step(&self, op: &str, args: Value) -> Value {
        let (credential, name) = (self.operator.clone(), op.to_string());
        self.deps
            .kernel
            .run(move |core| {
                let context = capstan_daemon::deps::new_context(core, &credential)?;
                let mut all = vec![serde_json::to_value(context).unwrap()];
                all.extend(args.as_array().cloned().unwrap());
                core.dispatch(&name, &all)
            })
            .unwrap_or_else(|e| panic!("{op}: {e}"))
    }

    fn repo(&self, name: &str) -> PathBuf {
        self.scratch.path().join("repos").join(name)
    }

    /// Records `path` as the live worktree `agent` holds in `repo`, on `branch`.
    fn record(&self, agent: &str, repo: &str, path: &Path, branch: &str) {
        let (operator, agent, repo) = (self.operator.clone(), agent.to_string(), repo.to_string());
        let (path, branch) = (path.to_string_lossy().into_owned(), branch.to_string());
        let base = git(&self.repo("core"), &["rev-parse", "main"]);
        self.deps
            .kernel
            .run(move |core| {
                let context = capstan_daemon::deps::new_context(core, &operator)?;
                core.record_agent_worktree(
                    &context,
                    &json!({"agentId": agent, "generation": 1, "repo": repo, "worktreePath": path, "branch": branch, "baseSha": base}),
                )
            })
            .unwrap();
    }

    fn end(&self, agent: &str) {
        let (operator, agent) = (self.operator.clone(), agent.to_string());
        self.deps
            .kernel
            .run(move |core| {
                let context = capstan_daemon::deps::new_context(core, &operator)?;
                core.end_agent(&context, &agent, None)
            })
            .unwrap();
    }

    /// A real worktree of `repo` at `path` on a new `branch`, recorded for `agent`.
    fn worktree(&self, agent: &str, repo: &str, path: &Path, branch: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        git(
            &self.repo(repo),
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                branch,
                &path.to_string_lossy(),
                "main",
            ],
        );
        self.record(agent, repo, path, branch);
    }

    fn call(&self, args: &[&str]) -> Value {
        let frame =
            json!({"v": 1, "credential": self.operator, "command": "cleanup", "args": args});
        let outcome = handle_frame(
            &self.deps,
            frame.to_string().as_bytes(),
            &AbortSignal::new(),
            MAX_RESPONSE_BYTES,
        );
        serde_json::from_str(&outcome.response.expect("an answer")).expect("JSON")
    }

    /// `feed snapshot` and `feed item run run`: the `run` item each answers.
    fn feed_runs(&self) -> (Value, Value) {
        let one = |args: &[&str]| {
            let frame =
                json!({"v": 1, "credential": self.operator, "command": "feed", "args": args});
            let outcome = handle_frame(
                &self.deps,
                frame.to_string().as_bytes(),
                &AbortSignal::new(),
                MAX_RESPONSE_BYTES,
            );
            let answer: Value =
                serde_json::from_str(&outcome.response.expect("an answer")).unwrap();
            assert_eq!(answer["ok"], true, "feed {args:?}: {answer}");
            answer["result"].clone()
        };
        (
            one(&["snapshot"])["run"].clone(),
            one(&["item", "run", "run"])["item"].clone(),
        )
    }

    fn ok(&self, args: &[&str]) -> Value {
        let answer = self.call(args);
        assert_eq!(answer["ok"], true, "cleanup {args:?}: {answer}");
        answer["result"].clone()
    }
}

fn cleanup_of<'a>(answer: &'a Value, repo: &str) -> &'a Value {
    answer["cleanups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["repo"] == repo)
        .unwrap_or_else(|| panic!("no cleanup for {repo}: {answer}"))
}

fn skipped_reason(applied: &Value, branch: &str) -> String {
    applied["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["branch"] == branch)
        .unwrap_or_else(|| panic!("{branch} not skipped: {applied}"))["reason"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn one_cleanup_per_repo_with_its_own_answer() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let world = World::new(&["dev-1", "dev-2"]);
    // dev-1 holds a worktree in both configured repos; dev-2's repo left the configuration.
    for repo in ["core", "web"] {
        world.worktree(
            "dev-1",
            repo,
            &world.link.join("dev-1").join(repo),
            "feat/dev-1",
        );
    }
    world.worktree(
        "dev-2",
        "old",
        &world.link.join("dev-2").join("old"),
        "feat/dev-2",
    );
    world.end("dev-1");
    world.end("dev-2");

    let answer = world.ok(&[]);
    assert_eq!(answer["cleanups"].as_array().unwrap().len(), 3, "{answer}");
    for repo in ["core", "web"] {
        let cleanup = cleanup_of(&answer, repo);
        assert_eq!(cleanup["candidates"], 1, "{answer}");
        assert_eq!(cleanup["worktrees"], 1, "{answer}");
        assert!(cleanup["cleanupId"].as_str().is_some());
        assert!(cleanup.get("note").is_none() && cleanup.get("left").is_none());
    }
    let old = cleanup_of(&answer, "old");
    assert_eq!(old["candidates"], 1);
    assert_eq!(old["worktrees"], 1);
    assert_eq!(
        old["left"], true,
        "a repo that left the configuration says so"
    );
    assert!(old["note"]
        .as_str()
        .unwrap()
        .contains("left the config; landed and clean cannot be checked; name it to delete"));
    assert!(old["note"].as_str().unwrap().contains("worktree prune"));

    // Each id answers with its repo, applied or declined by id.
    let core_id = cleanup_of(&answer, "core")["cleanupId"].as_str().unwrap();
    let web_id = cleanup_of(&answer, "web")["cleanupId"].as_str().unwrap();
    let declined = world.ok(&["decline", web_id]);
    assert_eq!(declined["repo"], "web");
    assert_eq!(declined["state"], "declined");
    let applied = world.ok(&["apply", core_id, "feat/dev-1"]);
    assert_eq!(applied["repo"], "core");
    assert_eq!(applied["deleted"][0]["branch"], "feat/dev-1");
    assert!(!world.link.join("dev-1/core").exists());
    assert!(world.link.join("dev-1/web").exists(), "web was declined");
    assert!(
        git(&world.repo("core"), &["branch", "--list", "feat/dev-1"]).is_empty(),
        "the branch of the applied repo is deleted"
    );

    // A second run supersedes only an older offer of the same repo; the removed repo's offer is still open.
    let again = world.ok(&[]);
    let old_id = cleanup_of(&again, "old")["cleanupId"].as_str().unwrap();
    assert_ne!(old_id, old["cleanupId"].as_str().unwrap());
}

/// A world with a removed repo `old` and the agents named; each ended.
fn old_world(agents: &[&str]) -> World {
    let world = World::new(agents);
    std::fs::create_dir_all(world.scratch.path().join("outside")).unwrap();
    world
}

fn outside(world: &World) -> PathBuf {
    world.scratch.path().join("outside")
}

fn at(world: &World, agent: &str) -> PathBuf {
    world.link.join(agent).join("old")
}

/// Offers the removed repo's worktrees, applies them all by name (with `hook` set) and returns the answer.
fn apply_named(
    world: &World,
    agents: &[&str],
    hook: Option<capstan_daemon::handlers::cleanup::PathHook>,
) -> Value {
    for agent in agents {
        world.end(agent);
    }
    let answer = world.ok(&[]);
    let old = cleanup_of(&answer, "old");
    assert_eq!(old["candidates"], agents.len(), "{answer}");
    let id = old["cleanupId"].as_str().unwrap().to_string();
    let branches: Vec<String> = agents.iter().map(|a| format!("b/{a}")).collect();
    let mut args = vec!["apply", id.as_str()];
    args.extend(branches.iter().map(String::as_str));
    *BEFORE_DELETE.lock().unwrap() = hook;
    let applied = world.ok(&args);
    *BEFORE_DELETE.lock().unwrap() = None;
    applied
}

fn assert_by_hand(applied: &Value, branch: &str) {
    let reason = skipped_reason(applied, branch);
    assert!(
        reason.starts_with("remove it by hand: git -C ") && reason.contains("worktree remove"),
        "{branch}: {reason}"
    );
}

/// `good` is deleted and `bad` is skipped by hand; the caller checks what `bad` left behind.
fn check_good_and_bad(world: &World, applied: &Value, bad: &str) {
    let deleted: Vec<&str> = applied["deleted"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["branch"].as_str().unwrap())
        .collect();
    assert_eq!(deleted, ["b/good"], "{applied}");
    assert!(!at(world, "good").exists());
    assert_by_hand(applied, &format!("b/{bad}"));
    assert!(!git(&world.repo("old"), &["branch", "--list", "b/good"]).is_empty());
}

#[test]
fn a_removed_repos_worktree_is_offered_unlanded_left_by_plain_apply_and_deleted_only_when_named() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let world = old_world(&["good"]);
    world.worktree("good", "old", &at(&world, "good"), "b/good");
    world.end("good");
    let answer = world.ok(&[]);
    let id = cleanup_of(&answer, "old")["cleanupId"].as_str().unwrap();
    let rows = capstan_kernel_cleanup_rows(&world);
    let entry = &rows[0]["entries"][0];
    assert_eq!(entry["class"], "unlanded", "{rows:?}");
    assert_eq!(entry["branch"], "b/good");
    let plain = world.ok(&["apply", id]);
    assert!(plain["deleted"].as_array().unwrap().is_empty(), "{plain}");
    assert!(at(&world, "good").exists());
    let answer = world.ok(&[]);
    let id = cleanup_of(&answer, "old")["cleanupId"].as_str().unwrap();
    let named = world.ok(&["apply", id, "b/good"]);
    assert_eq!(named["deleted"][0]["branch"], "b/good", "{named}");
    assert!(!at(&world, "good").exists());
    assert!(!git(&world.repo("old"), &["branch", "--list", "b/good"]).is_empty());
}

fn capstan_kernel_cleanup_rows(world: &World) -> Vec<Value> {
    let operator = world.operator.clone();
    world
        .deps
        .kernel
        .run(move |core| {
            capstan_kernel::areas::cleanups::cleanups(core.kernel(), &operator, Some(50))
        })
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn a_symlink_out_of_worktree_dir_is_skipped() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let world = old_world(&["good", "sym"]);
    world.worktree("good", "old", &at(&world, "good"), "b/good");
    let real = outside(&world).join("wt");
    git(
        &world.repo("old"),
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "b/sym",
            &real.to_string_lossy(),
            "main",
        ],
    );
    std::fs::create_dir_all(at(&world, "sym").parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&real, at(&world, "sym")).unwrap();
    world.record("sym", "old", &at(&world, "sym"), "b/sym");
    let applied = apply_named(&world, &["good", "sym"], None);
    check_good_and_bad(&world, &applied, "sym");
    assert!(real.join("f.txt").exists());
}

#[test]
fn a_git_file_naming_a_gitdir_outside_git_worktrees_is_skipped() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let world = old_world(&["good", "foreign"]);
    world.worktree("good", "old", &at(&world, "good"), "b/good");
    let fake = outside(&world).join("fakegit");
    std::fs::create_dir_all(&fake).unwrap();
    std::fs::create_dir_all(at(&world, "foreign")).unwrap();
    std::fs::write(
        at(&world, "foreign").join(".git"),
        format!("gitdir: {}\n", fake.display()),
    )
    .unwrap();
    world.record("foreign", "old", &at(&world, "foreign"), "b/foreign");
    let applied = apply_named(&world, &["good", "foreign"], None);
    check_good_and_bad(&world, &applied, "foreign");
    assert!(at(&world, "foreign").join(".git").exists());
}

#[test]
fn a_non_directory_path_is_skipped() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let world = old_world(&["good", "file"]);
    world.worktree("good", "old", &at(&world, "good"), "b/good");
    std::fs::create_dir_all(at(&world, "file").parent().unwrap()).unwrap();
    std::fs::write(at(&world, "file"), "not a worktree").unwrap();
    world.record("file", "old", &at(&world, "file"), "b/file");
    let applied = apply_named(&world, &["good", "file"], None);
    check_good_and_bad(&world, &applied, "file");
    assert!(at(&world, "file").is_file());
}

#[test]
fn an_unrelated_worktree_whose_back_link_names_another_path_is_skipped() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let world = old_world(&["good", "unrel"]);
    world.worktree("good", "old", &at(&world, "good"), "b/good");
    world.worktree("unrel", "old", &at(&world, "unrel"), "b/unrel");
    let admin = git(&at(&world, "unrel"), &["rev-parse", "--git-dir"]);
    std::fs::write(
        Path::new(&admin).join("gitdir"),
        format!("{}\n", world.changes.join("elsewhere/.git").display()),
    )
    .unwrap();
    let applied = apply_named(&world, &["good", "unrel"], None);
    check_good_and_bad(&world, &applied, "unrel");
    assert!(at(&world, "unrel").join("f.txt").exists());
}

#[test]
fn a_path_swapped_in_before_delete_survives() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let world = old_world(&["good", "swap"]);
    world.worktree("good", "old", &at(&world, "good"), "b/good");
    world.worktree("swap", "old", &at(&world, "swap"), "b/swap");
    let victim = outside(&world).join("victim");
    std::fs::create_dir_all(&victim).unwrap();
    std::fs::write(victim.join("keep.txt"), "mine").unwrap();
    let (target, link_to) = (at(&world, "swap"), victim.clone());
    let hook: capstan_daemon::handlers::cleanup::PathHook = Arc::new(move |path: &Path| {
        if path.ends_with("swap/old") {
            std::fs::remove_dir_all(&target).unwrap();
            std::os::unix::fs::symlink(&link_to, &target).unwrap();
        }
    });
    let applied = apply_named(&world, &["good", "swap"], Some(hook));
    check_good_and_bad(&world, &applied, "swap");
    assert_eq!(
        std::fs::read_to_string(victim.join("keep.txt")).unwrap(),
        "mine"
    );
}

#[test]
fn a_symlinked_worktree_dir_is_compared_by_canonical_path() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    // The configuration names the link; one worktree is recorded by the link path, one by the real path.
    let world = old_world(&["good", "real"]);
    world.worktree("good", "old", &at(&world, "good"), "b/good");
    world.worktree(
        "real",
        "old",
        &world.changes.join("real").join("old"),
        "b/real",
    );
    for agent in ["good", "real"] {
        world.end(agent);
    }
    let answer = world.ok(&[]);
    let id = cleanup_of(&answer, "old")["cleanupId"].as_str().unwrap();
    let applied = world.ok(&["apply", id, "b/good", "b/real"]);
    assert_eq!(applied["deleted"].as_array().unwrap().len(), 2, "{applied}");
    assert!(!at(&world, "good").exists());
    assert!(!world.changes.join("real/old").exists());
}

#[test]
fn a_running_set_blocks_cleanup() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let world = old_world(&["good"]);
    world.worktree("good", "old", &at(&world, "good"), "b/good");
    world.end("good");
    let connection = rusqlite::Connection::open(&world.database).unwrap();
    connection
        .execute(
            "INSERT INTO integration_sets(project_id, set_id, sequence, branch, state, created_by, created_at)
             VALUES ('pwsclean', 'set-1', 1, 'set/1', 'running', (SELECT actor_id FROM agents LIMIT 1), '2026-01-01T00:00:00.000Z')",
            [],
        )
        .unwrap();
    let answer = world.call(&[]);
    assert_eq!(answer["ok"], false, "{answer}");
    assert!(
        answer["message"]
            .as_str()
            .unwrap()
            .contains("integration_running"),
        "{answer}"
    );
    assert!(at(&world, "good").exists());
}

#[test]
fn without_repos_the_answer_keeps_its_single_repo_shape() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let world = World::build(&[], false);
    let answer = world.ok(&[]);
    let mut keys: Vec<&str> = answer
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        ["cleanupId", "entries", "offered", "protected"],
        "{answer}"
    );
    assert!(answer["cleanupId"].is_null());
    assert_eq!(answer["offered"], 0);
}

#[test]
fn the_feed_run_item_carries_the_repos_main_branches_and_the_open_cleanups_of_a_workspace() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let world = old_world(&["good", "gone"]);
    // No offer yet: the repos show, the cleanups do not.
    let (snapshot, item) = world.feed_runs();
    let repos = json!([
        {"repo": "core", "mainBranch": "main"},
        {"repo": "web", "mainBranch": "main"},
    ]);
    assert_eq!(snapshot["repos"], repos, "{snapshot}");
    assert_eq!(item["repos"], repos, "{item}");
    assert!(snapshot.get("cleanups").is_none(), "{snapshot}");

    world.worktree("good", "core", &at(&world, "good"), "b/good");
    world.worktree("gone", "old", &at(&world, "gone"), "b/gone");
    world.end("good");
    world.end("gone");
    let answer = world.ok(&[]);
    let core_id = cleanup_of(&answer, "core")["cleanupId"]
        .as_str()
        .unwrap()
        .to_string();
    let old_id = cleanup_of(&answer, "old")["cleanupId"]
        .as_str()
        .unwrap()
        .to_string();
    let (snapshot, item) = world.feed_runs();
    assert_eq!(snapshot["cleanups"], item["cleanups"]);
    let offers = snapshot["cleanups"].as_array().unwrap();
    assert_eq!(offers.len(), 2, "{snapshot}");
    let core = offers.iter().find(|o| o["repo"] == "core").unwrap();
    assert_eq!(core["cleanupId"], core_id.as_str());
    assert_eq!(core["branches"], 1);
    assert_eq!(core["worktrees"], 1);
    assert!(core.get("left").is_none(), "{core}");
    let old = offers.iter().find(|o| o["repo"] == "old").unwrap();
    assert_eq!(old["cleanupId"], old_id.as_str());
    assert_eq!(old["left"], true);

    // A declined offer is no longer listed.
    world.ok(&["decline", &core_id]);
    let (snapshot, _) = world.feed_runs();
    assert_eq!(
        snapshot["cleanups"].as_array().unwrap().len(),
        1,
        "{snapshot}"
    );
}

#[test]
fn the_feed_run_item_of_a_single_repo_project_has_neither() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let world = World::build(&[], false);
    let (snapshot, item) = world.feed_runs();
    for run in [&snapshot, &item] {
        assert!(
            run.get("repos").is_none() && run.get("cleanups").is_none(),
            "{run}"
        );
    }
}
