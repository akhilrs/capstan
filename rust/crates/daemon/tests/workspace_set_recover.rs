//! Recovery of integration sets after a crash, and the busy guards: a two-repo set is merged for real, then the state
//! a dead daemon would leave is written back and `recover_integrations` runs.
#![allow(dead_code)]

mod common;

use capstan_config::TrustStore;
use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::handlers::handle_frame;
use capstan_daemon::handlers::integrate_seams::{install_integrate_seams, IntegrateSeams};
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
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

static WORLDS: AtomicUsize = AtomicUsize::new(0);

fn git_env() -> Vec<(&'static str, String)> {
    vec![
        ("PATH", std::env::var("PATH").unwrap_or_default()),
        ("LC_ALL", "C".into()),
        ("GIT_CONFIG_NOSYSTEM", "1".into()),
        ("GIT_CONFIG_GLOBAL", "/dev/null".into()),
        ("GIT_TERMINAL_PROMPT", "0".into()),
        ("GIT_AUTHOR_NAME", "Test".into()),
        ("GIT_AUTHOR_EMAIL", "test@example.com".into()),
        ("GIT_COMMITTER_NAME", "Test".into()),
        ("GIT_COMMITTER_EMAIL", "test@example.com".into()),
    ]
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(git_env())
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} in {}: {}",
        cwd.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn make_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "--quiet", "-b", "main"]);
    std::fs::write(dir.join("f.txt"), "1\n2\n3\n").unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "--quiet", "-m", "chore: initial commit"]);
}

/// A commit off main on a throwaway branch writing `file`; main is checked out again.
fn commit_on(dir: &Path, file: &str, text: &str) -> String {
    let branch = format!("w-{}", git(dir, &["rev-parse", "--short=12", "HEAD"]));
    git(dir, &["checkout", "--quiet", "-B", &branch, "main"]);
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", "-A"]);
    git(
        dir,
        &["commit", "--quiet", "-m", &format!("feat: write {file}")],
    );
    let sha = git(dir, &["rev-parse", "HEAD"]);
    git(dir, &["checkout", "--quiet", "main"]);
    sha
}

fn has_branch(dir: &Path, branch: &str) -> bool {
    Command::new("git")
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .current_dir(dir)
        .env_clear()
        .envs(git_env())
        .output()
        .unwrap()
        .status
        .success()
}

struct World {
    _dir: common::PrivateDir,
    scratch: common::PrivateDir,
    deps: Deps,
    operator: String,
    database: PathBuf,
    repos: PathBuf,
    root: PathBuf,
    sequence: std::cell::Cell<i64>,
}

impl World {
    fn new() -> World {
        World::build(true)
    }

    /// `workspace` false writes a project with no `[repos]`: its one repository is the project root.
    fn build(workspace: bool) -> World {
        let dir = private_tempdir();
        let scratch = private_tempdir();
        let repos = scratch.path().join("repos");
        make_repo(&repos.join("core"));
        make_repo(&repos.join("web"));
        let identity = Identity {
            project_id: "pset".into(),
            name: "Set Merge".into(),
            credential: format!("operator-{}", "s".repeat(40)),
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
                    c = scratch.path().join("changes").display()
                )
            } else {
                String::new()
            },
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
                    seeded_env(&format!(
                        "set-merge-{}-{}",
                        std::process::id(),
                        WORLDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    )),
                )
            })
            .expect("the kernel opens")
        };
        let sink: Logger = Arc::new(|_entry: &LogEntry| {});
        let mut options = DaemonOptions::new(state.clone(), project, dir.path().to_path_buf());
        options.trust_store = trust.clone();
        let role_config = capstan_config::load_role_config_with(dir.path(), &trust).unwrap();
        assert_eq!(role_config.workspace.is_some(), workspace);
        options.capstan = Some(Arc::new(role_config));
        let mut deps = Deps::new(kernel, options, sink);
        deps.git = Arc::new(ProjectGit::new(dir.path()));
        let world = World {
            root: dir.path().to_path_buf(),
            _dir: dir,
            scratch,
            deps,
            operator: identity.credential.clone(),
            database,
            repos,
            sequence: std::cell::Cell::new(1),
        };
        world.step(
            "syncRoleDefinitions",
            json!([[
                {"name": "pm", "kind": "PM", "host": "claude", "configHash": "b".repeat(64)},
                {"name": "developer", "kind": "Developer", "host": "claude", "configHash": "a".repeat(64)},
            ]]),
        );
        for (agent, role, kind) in [
            ("pm-1", "pm", "PM"),
            ("dev-1", "developer", "Developer"),
            ("dev-2", "developer", "Developer"),
        ] {
            let seat = format!("seat-{agent}");
            world.step(
                "createSeat",
                json!([{"seatId": seat, "name": seat, "role": kind}]),
            );
            let actor = world.step(
                "createActor",
                json!([{"displayName": agent, "role": kind, "seatId": seat}]),
            );
            world.step(
                "registerAgent",
                json!([{"agentId": agent, "roleName": role, "seatId": seat, "actorId": actor["actorId"]}]),
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
        self.repos.join(name)
    }

    fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(&self.database).unwrap()
    }

    fn count(&self, sql: &str) -> i64 {
        self.db().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn call(&self, command: &str, args: &[&str]) -> Value {
        let frame = json!({"v": 1, "credential": self.operator, "command": command, "args": args});
        let outcome = handle_frame(
            &self.deps,
            frame.to_string().as_bytes(),
            &AbortSignal::new(),
            MAX_RESPONSE_BYTES,
        );
        serde_json::from_str(&outcome.response.expect("an answer")).expect("JSON")
    }

    /// An accepted, reviewed report of `agent` with one commit per repo (none: a report of before workspace mode).
    fn report(&self, agent: &str, commits: &[(&str, &str)], legacy_sha: Option<&str>) -> String {
        let conn = self.db();
        let actor = |who: &str| -> String {
            conn.query_row(
                "SELECT actor_id FROM agents WHERE agent_id = ?",
                [who],
                |r| r.get(0),
            )
            .unwrap()
        };
        let sequence = self.sequence.get();
        self.sequence.set(sequence + 1);
        let id = format!("report-{sequence}");
        let sha = legacy_sha.unwrap_or_else(|| commits[0].1);
        conn.execute(
            "INSERT INTO agent_reports(project_id, report_id, sequence, agent_id, generation, actor_id, commit_sha, branch, summary, state, evidence_json, created_at)
             VALUES ('pset', ?, ?, ?, 1, ?, ?, 'b', ?, 'accepted', '{}', '2026-01-01T00:00:00.000Z')",
            rusqlite::params![id, sequence, agent, actor(agent), sha, format!("summary {id}")],
        )
        .unwrap();
        for (repo, sha) in commits {
            conn.execute(
                "INSERT INTO report_commits(project_id, report_id, repo, commit_sha, branch, base_sha) VALUES ('pset', ?, ?, ?, 'b', ?)",
                rusqlite::params![id, repo, sha, "0".repeat(40)],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO reviews(project_id, review_id, sequence, round, subject_report_id, commit_sha, base_sha, requested_by_actor_id, reviewer_role, reviewer_agent_id, reviewer_actor_id, author_agent_id, author_actor_id, verdict_text, state, created_at)
             VALUES ('pset', ?, ?, 1, ?, ?, ?, ?, 'pm', 'pm-1', ?, ?, ?, 'ok', 'passed', '2026-01-01T00:00:00.000Z')",
            rusqlite::params![
                format!("rv-{sequence}"), sequence, id, sha, "0".repeat(40), actor(agent), actor("pm-1"), agent, actor(agent)
            ],
        )
        .unwrap();
        id
    }

    /// The reports of a typical change: one commit in each of core and web.
    fn two_repo_report(&self) -> (String, String, String) {
        let (a, b) = (
            commit_on(&self.repo("core"), "a.txt", "a\n"),
            commit_on(&self.repo("web"), "b.txt", "b\n"),
        );
        let report = self.report("dev-1", &[("core", &a), ("web", &b)], None);
        (report, a, b)
    }

    fn mains(&self) -> (String, String) {
        (
            git(&self.repo("core"), &["rev-parse", "main"]),
            git(&self.repo("web"), &["rev-parse", "main"]),
        )
    }

    fn install(&self, seams: IntegrateSeams) {
        install_integrate_seams(&self.deps, seams);
    }
}

fn result(answer: &Value) -> &Value {
    assert_eq!(answer["ok"], true, "{answer}");
    &answer["result"]
}

fn refusal(answer: &Value) -> String {
    assert_eq!(answer["ok"], false, "{answer}");
    answer["message"].as_str().unwrap_or_default().to_string()
}

fn repos_of(set: &Value) -> Vec<&str> {
    set["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["repo"].as_str().unwrap())
        .collect()
}

// ------------------------------------------------------------------------------------------------ merging
use capstan_daemon::loops::recover::recover_integrations;

impl World {
    fn set_id(&self) -> String {
        self.db()
            .query_row("SELECT set_id FROM integration_sets", [], |r| r.get(0))
            .unwrap()
    }

    fn sql(&self, sql: &str) {
        self.db().execute_batch(sql).unwrap();
    }

    fn recover(&self) {
        recover_integrations(&self.deps, &|| false);
    }

    fn set_state(&self) -> (String, Option<String>) {
        self.db()
            .query_row("SELECT state, reason FROM integration_sets", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap()
    }
}

#[test]
fn a_cut_off_after_the_first_member_recovers_to_failed_naming_the_repo() {
    let w = World::new();
    let (report, _, _) = w.two_repo_report();
    assert_eq!(result(&w.call("integrate", &[&report]))["state"], "merged");
    // Death between the members: web never started, the set still runs.
    w.sql(
        "UPDATE integration_set_members SET integration_id = NULL WHERE repo = 'web';
           UPDATE integration_sets SET state = 'running', reason = NULL;",
    );
    w.recover();
    let (state, reason) = w.set_state();
    assert_eq!(state, "failed");
    assert_eq!(reason.as_deref(), Some("cut off before repo web"));
    let members = w.count("SELECT count(*) FROM integration_set_members WHERE repo = 'web' AND integration_id IS NULL");
    assert_eq!(members, 1, "the unstarted member stays detached");
}

#[test]
fn death_after_the_last_member_merged_recovers_to_merged() {
    let w = World::new();
    let (report, _, _) = w.two_repo_report();
    assert_eq!(result(&w.call("integrate", &[&report]))["state"], "merged");
    w.sql("UPDATE integration_sets SET state = 'running', reason = NULL;");
    w.recover();
    assert_eq!(w.set_state(), ("merged".to_string(), None));
}

#[test]
fn a_running_member_row_is_settled_and_the_set_fails() {
    let w = World::new();
    let (report, _, _) = w.two_repo_report();
    assert_eq!(result(&w.call("integrate", &[&report]))["state"], "merged");
    // The ledger only moves an integration forward; the state a crash leaves is written past that guard.
    w.sql("DROP TRIGGER integrations_move_forward");
    w.sql("UPDATE integrations SET state = 'running', head_sha = NULL, completed_at = NULL WHERE integration_id =
             (SELECT integration_id FROM integration_set_members WHERE repo = 'web');
           UPDATE integration_sets SET state = 'running', reason = NULL;");
    w.recover();
    assert_eq!(
        w.count("SELECT count(*) FROM integrations WHERE state = 'running'"),
        0
    );
    let (state, reason) = w.set_state();
    assert_eq!(state, "failed");
    assert!(reason.unwrap().starts_with("repo web failed"));
}

#[test]
fn a_conflicted_member_makes_the_set_conflicted_with_its_files() {
    let w = World::new();
    let a = commit_on(&w.repo("core"), "a.txt", "a\n");
    let b = commit_on(&w.repo("web"), "f.txt", "two\n2\n3\n");
    let report = w.report("dev-1", &[("core", &a), ("web", &b)], None);
    std::fs::write(w.repo("web").join("f.txt"), "uno\n2\n3\n").unwrap();
    git(&w.repo("web"), &["commit", "--quiet", "-am", "feat: edit"]);
    assert_eq!(
        result(&w.call("integrate", &[&report]))["state"],
        "conflicted"
    );
    w.sql("UPDATE integration_sets SET state = 'running', reason = NULL;");
    w.recover();
    let (state, reason) = w.set_state();
    assert_eq!(state, "conflicted");
    assert_eq!(reason.as_deref(), Some("repo web conflicts in f.txt"));
}

#[test]
fn the_busy_guard_counts_a_running_set_and_frees_after_recovery() {
    let w = World::new();
    let (report, _, _) = w.two_repo_report();
    assert_eq!(result(&w.call("integrate", &[&report]))["state"], "merged");
    w.sql("UPDATE integration_sets SET state = 'running', reason = NULL;");
    let running = w
        .deps
        .kernel
        .run(|core| capstan_kernel::areas::cleanups::integration_running(core.kernel()))
        .unwrap();
    assert!(running);
    w.recover();
    let after = w
        .deps
        .kernel
        .run(|core| capstan_kernel::areas::cleanups::integration_running(core.kernel()))
        .unwrap();
    assert!(!after);
}

#[test]
fn a_settled_member_branch_is_swept_in_its_own_repo() {
    let w = World::new();
    let (report, _, _) = w.two_repo_report();
    assert_eq!(result(&w.call("integrate", &[&report]))["state"], "merged");
    let branch: String = w
        .db()
        .query_row(
            "SELECT i.branch FROM integrations i JOIN integration_set_members m ON m.integration_id = i.integration_id WHERE m.repo = 'web'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(has_branch(&w.repo("web"), &branch));
    // Plan-184's settle does not exist yet: the member is confirmed in the ledger directly.
    w.sql(&format!(
        "UPDATE integrations SET state = 'confirmed' WHERE branch = '{branch}'"
    ));
    w.recover();
    assert!(!has_branch(&w.repo("web"), &branch), "swept in web");
    // The merged core member's branch is not a settled one and stays.
    assert_eq!(w.set_state().0, "merged");
}

#[test]
fn a_set_without_members_recovers_to_failed() {
    let w = World::new();
    let (report, _, _) = w.two_repo_report();
    assert_eq!(result(&w.call("integrate", &[&report]))["state"], "merged");
    let triggers: Vec<String> = {
        let db = w.db();
        let mut q = db
            .prepare("SELECT name FROM sqlite_master WHERE type = 'trigger' AND tbl_name = 'integration_set_members'")
            .unwrap();
        q.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    for t in triggers {
        w.sql(&format!("DROP TRIGGER {t}"));
    }
    w.sql("DELETE FROM integration_set_members; UPDATE integration_sets SET state = 'running', reason = NULL;");
    w.recover();
    assert_eq!(
        w.set_state(),
        (
            "failed".to_string(),
            Some("cut off before any repo merged".to_string())
        )
    );
}

#[test]
fn a_member_still_running_after_the_settle_leaves_the_set_cut_off() {
    let w = World::new();
    let (report, _, _) = w.two_repo_report();
    assert_eq!(result(&w.call("integrate", &[&report]))["state"], "merged");
    w.sql("DROP TRIGGER integrations_move_forward");
    w.sql("UPDATE integrations SET state = 'running', head_sha = NULL, completed_at = NULL WHERE integration_id =
             (SELECT integration_id FROM integration_set_members WHERE repo = 'web');
           UPDATE integration_sets SET state = 'running', reason = NULL;
           CREATE TRIGGER refuse_failed BEFORE UPDATE OF state ON integrations WHEN NEW.state = 'failed'
             BEGIN SELECT RAISE(ABORT, 'refused for the test'); END;");
    w.recover();
    assert_eq!(
        w.count("SELECT count(*) FROM integrations WHERE state = 'running'"),
        1
    );
    assert_eq!(
        w.set_state(),
        (
            "failed".to_string(),
            Some("cut off before repo web".to_string())
        )
    );
}
