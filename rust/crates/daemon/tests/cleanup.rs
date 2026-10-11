//! The `cleanup` command and the cleanup offer at integrate confirm, through `handlers::handle_frame` like a connection,
//! on a scratch repository and a real ledger. The branches that are done with are the reported branches of two developers
//! whose agents have ended; the integration pipeline is the real one.

mod common;

use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::handlers::cleanup::FAIL_AFTER_CONFIRM;
use capstan_daemon::handlers::handle_frame;
use capstan_daemon::handlers::shared::AbortSignal;
use capstan_daemon::handlers::MAX_RESPONSE_BYTES;
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::Core;
use common::project::{build_repo, write_project, Identity};
use common::replay::ReplayGit;
use common::{private_tempdir, seeded_env};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

/// Tests that confirm an integration share the failure switch, so they run one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|p| p.into_inner())
}

const BASE_CONFIG: &str = r#"schema_version = 1

[hosts.claude]
kind = "claude"

[roles.pm]
kind = "PM"
host = "claude"

[roles.developer]
kind = "Developer"
host = "claude"

[roles.architect]
kind = "Developer"
host = "claude"

[roles.reviewer]
kind = "Verifier"
host = "claude"

[roles.supervisor]
kind = "Supervisor"
host = "claude"

[architect]
enabled = true
"#;

const AGENTS: [(&str, &str, &str); 5] = [
    ("pm-1", "pm", "PM"),
    ("arch-1", "architect", "Developer"),
    ("dev-1", "developer", "Developer"),
    ("dev-2", "developer", "Developer"),
    ("rev-1", "reviewer", "Verifier"),
];

struct World {
    dir: tempfile::TempDir,
    deps: Deps,
    credentials: HashMap<String, String>,
    heads: BTreeMap<String, String>,
    logs: Arc<Mutex<Vec<LogEntry>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

impl World {
    /// `cleanup` is the text of the `[cleanup]` section.
    fn new(cleanup: &str) -> World {
        let dir = private_tempdir();
        let identity = Identity {
            project_id: "pcleanup".into(),
            name: "Cleanup".into(),
            credential: format!("operator-{}", "c".repeat(40)),
        };
        let state = write_project(dir.path(), &identity);
        let file = dir.path().join("capstan.toml");
        std::fs::write(&file, format!("{BASE_CONFIG}\n{cleanup}\n")).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        let heads = build_repo(
            dir.path(),
            &json!({
                "files": {"a.txt": "a\n"},
                "branches": {
                    "feat/dev-1": {"files": {"one.txt": "one\n"}, "message": "feat: one"},
                    "feat/dev-2": {"files": {"two.txt": "two\n"}, "message": "feat: two"},
                },
            }),
        );
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
                    seeded_env("cleanup"),
                )
            })
            .expect("the kernel opens")
        };
        let logs: Arc<Mutex<Vec<LogEntry>>> = Arc::default();
        let sink: Logger = {
            let logs = logs.clone();
            Arc::new(move |entry: &LogEntry| lock(&logs).push(entry.clone()))
        };
        let mut options = DaemonOptions::new(state, project, dir.path().to_path_buf());
        options.capstan = Some(Arc::new(
            capstan_config::load_role_config(dir.path()).unwrap(),
        ));
        let mut deps = Deps::new(kernel, options, sink);
        deps.git = Arc::new(ReplayGit::new(dir.path()));
        capstan_daemon::deps::startup_sequence(&deps);
        let mut world = World {
            dir,
            deps,
            credentials: HashMap::new(),
            heads,
            logs,
        };
        world
            .credentials
            .insert("operator".into(), identity.credential);
        world.populate();
        world
    }

    fn ledger(&self, credential: &str, op: &str, args: Vec<Value>) -> Value {
        let (credential, name) = (credential.to_string(), op.to_string());
        self.deps
            .kernel
            .run(move |core| {
                let context = capstan_daemon::deps::new_context(core, &credential)?;
                let mut all = vec![serde_json::to_value(context).unwrap()];
                all.extend(args);
                core.dispatch(&name, &all)
            })
            .unwrap_or_else(|e| panic!("{op}: {e}"))
    }

    fn populate(&mut self) {
        let owner = self.credentials["operator"].clone();
        let roles: Vec<Value> = [
            ("pm", "PM"),
            ("developer", "Developer"),
            ("architect", "Developer"),
            ("reviewer", "Verifier"),
            ("supervisor", "Supervisor"),
        ]
        .iter()
        .map(|(name, kind)| {
            json!({"name": name, "kind": kind, "host": "claude", "configHash": "a".repeat(64)})
        })
        .collect();
        self.ledger(&owner, "syncRoleDefinitions", vec![json!(roles)]);
        for (agent, role, kind) in AGENTS {
            let seat = format!("seat-{agent}");
            self.ledger(
                &owner,
                "createSeat",
                vec![json!({"seatId": seat, "name": seat, "role": kind})],
            );
            let actor = self.ledger(
                &owner,
                "createActor",
                vec![json!({"displayName": agent, "role": kind, "seatId": seat})],
            );
            self.ledger(
                &owner,
                "registerAgent",
                vec![json!({"agentId": agent, "roleName": role, "seatId": seat, "actorId": actor["actorId"]})],
            );
            self.credentials
                .insert(agent.into(), actor["credential"].as_str().unwrap().into());
        }
        let internal = self.deps.credential().to_string();
        for (agent, branch) in [("dev-1", "feat/dev-1"), ("dev-2", "feat/dev-2")] {
            self.ledger(
                &internal,
                "recordAgentPane",
                vec![json!({
                    "agentId": agent,
                    "workspaceId": "ws-1",
                    "paneId": format!("pane-{agent}"),
                    "worktreePath": format!("/tmp/wt/{agent}"),
                    "branch": branch,
                    "baseSha": self.heads["main"],
                })],
            );
        }
    }

    fn call(&self, who: &str, command: &str, args: &[&str]) -> Value {
        let credential = self
            .credentials
            .get(who)
            .cloned()
            .unwrap_or_else(|| who.into());
        let frame = json!({"v": 1, "credential": credential, "command": command, "args": args});
        let outcome = handle_frame(
            &self.deps,
            frame.to_string().as_bytes(),
            &AbortSignal::new(),
            MAX_RESPONSE_BYTES,
        );
        serde_json::from_str(&outcome.response.expect("an answer")).expect("JSON")
    }

    fn ok(&self, who: &str, command: &str, args: &[&str]) -> Value {
        let answer = self.call(who, command, args);
        assert_eq!(answer["ok"], true, "{command} {args:?}: {answer}");
        answer["result"].clone()
    }

    fn refused(&self, who: &str, command: &str, args: &[&str], code: &str) -> String {
        let answer = self.call(who, command, args);
        assert_eq!(answer["ok"], false, "{command} {args:?}: {answer}");
        assert_eq!(answer["code"], code, "{command} {args:?}: {answer}");
        answer["message"].as_str().unwrap().to_string()
    }

    fn git(&self, args: &[&str]) -> String {
        git_in(self.dir.path(), args)
    }

    fn branches(&self) -> Vec<String> {
        self.git(&["for-each-ref", "--format=%(refname:short)", "refs/heads"])
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Reports feat/dev-1, has it reviewed and integrated, and merges the integration branch into main. Returns the
    /// integration id; the confirm is the test's.
    fn integrated(&self) -> String {
        let sha = self.heads["feat/dev-1"].clone();
        let report = self.ok("dev-1", "report", &[&sha, "One"]);
        let report_id = report["reportId"].as_str().unwrap().to_string();
        let pm = self.credentials["pm-1"].clone();
        let reviewer = self.credentials["rev-1"].clone();
        let review = |subject: &str| {
            self.ledger(
                &pm,
                "beginReview",
                vec![json!({"subjectId": subject, "reviewerRole": "reviewer", "reviewerAgentId": "rev-1"})],
            );
            self.ledger(
                &reviewer,
                "completeReview",
                vec![json!({"verdict": "pass", "text": "Fine."})],
            );
        };
        review(&report_id);
        let merged = self.ok("pm-1", "integrate", &[&report_id]);
        let branch = merged["branch"].as_str().unwrap().to_string();
        let integration_id = merged["integrationId"].as_str().unwrap().to_string();
        review(&integration_id);
        self.git(&["merge", "--ff-only", "--quiet", &branch]);
        integration_id
    }

    /// Ends the developers' agents, so their branches are no longer an active agent's.
    fn end_developers(&self) {
        let owner = self.credentials["operator"].clone();
        for agent in ["dev-1", "dev-2"] {
            let owner = owner.clone();
            self.deps
                .kernel
                .run(move |core| {
                    let context = capstan_daemon::deps::new_context(core, &owner)?;
                    core.end_agent(&context, agent, None)
                })
                .unwrap();
        }
    }

    fn cleanup_rows(&self) -> Vec<Value> {
        let owner = self.credentials["operator"].clone();
        self.deps
            .kernel
            .run(move |core| {
                capstan_kernel::areas::cleanups::cleanups(core.kernel(), &owner, Some(50))
            })
            .unwrap()
            .as_array()
            .unwrap()
            .clone()
    }

    /// The bodies of the PM's queued messages that mention a cleanup, with their action-needed flag.
    fn cleanup_messages(&self) -> Vec<(String, bool)> {
        let inbox = self.ok("pm-1", "inbox", &[]);
        inbox["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|m| {
                m["body"]
                    .as_str()
                    .is_some_and(|b| b.starts_with("Cleanup "))
            })
            .map(|m| {
                (
                    m["body"].as_str().unwrap().to_string(),
                    m["actionNeeded"] == true,
                )
            })
            .collect()
    }

    fn logged(&self, event: &str) -> Vec<Value> {
        lock(&self.logs)
            .iter()
            .filter(|e| e.command == format!("daemon:{event}"))
            .filter_map(|e| e.detail.clone())
            .collect()
    }

    /// Lays down a running integration, the way the pipeline leaves one while it works.
    fn running_integration(&self) {
        self.deps
            .kernel
            .run(|core| {
                core.kernel()
                    .database
                    .connection()
                    .execute(
                        "INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, created_at)
                         VALUES ('pcleanup', 'integration-running', 99, ?, 'capstan/integration/running', 'operator', 'running', '2026-01-01T00:00:00.000Z')",
                        [&"a".repeat(40)],
                    )
                    .unwrap();
                Ok(())
            })
            .unwrap();
    }
}

fn git_in(directory: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn confirm(world: &World, integration_id: &str) -> Value {
    world.ok("pm-1", "integrate", &["confirm", integration_id])
}

// ---------------------------------------------------------------------------------------------- confirm

#[test]
fn confirm_with_ask_offers_the_landed_branch_in_one_action_needed_message_and_deletes_nothing() {
    let _guard = serial();
    let world = World::new("[cleanup]\nbranches = \"ask\"");
    let integration = world.integrated();
    world.end_developers();
    let answer = confirm(&world, &integration);
    assert_eq!(answer["state"], "confirmed");
    let cleanup_id = answer["cleanup"]["cleanupId"].as_str().unwrap().to_string();
    assert_eq!(answer["cleanup"]["offered"], 1);
    assert_eq!(answer["cleanup"]["deleted"], 0);
    assert_eq!(
        world.branches(),
        ["feat/dev-1", "feat/dev-2", "main"],
        "nothing is deleted"
    );
    let messages = world.cleanup_messages();
    assert_eq!(messages.len(), 1, "{messages:?}");
    let (body, action_needed) = &messages[0];
    assert!(*action_needed);
    assert!(body.contains("feat/dev-1"), "{body}");
    assert!(body.contains(&world.heads["feat/dev-1"]), "{body}");
    assert!(body.contains(&cleanup_id), "{body}");
    assert!(
        !body.contains("feat/dev-2"),
        "an unlanded branch is not offered: {body}"
    );
    let rows = world.cleanup_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["state"], "offered");
}

#[test]
fn confirm_with_auto_deletes_the_landed_branch_records_it_and_tells_the_pm() {
    let _guard = serial();
    let world = World::new("[cleanup]\nbranches = \"auto\"");
    let integration = world.integrated();
    world.end_developers();
    let answer = confirm(&world, &integration);
    assert_eq!(answer["state"], "confirmed");
    assert_eq!(answer["cleanup"]["deleted"], 1);
    assert!(!world.branches().contains(&"feat/dev-1".to_string()));
    assert!(
        world.branches().contains(&"feat/dev-2".to_string()),
        "an unlanded branch stays"
    );
    let rows = world.cleanup_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["mode"], "auto");
    assert_eq!(rows[0]["state"], "applied");
    let messages = world.cleanup_messages();
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(!messages[0].1, "an informational message");
    assert!(
        messages[0].0.contains("feat/dev-1") && messages[0].0.contains(&world.heads["feat/dev-1"]),
        "{}",
        messages[0].0
    );
}

#[test]
fn confirm_with_off_creates_no_cleanup_and_no_message() {
    let _guard = serial();
    let world = World::new("[cleanup]\nbranches = \"off\"");
    let integration = world.integrated();
    world.end_developers();
    let answer = confirm(&world, &integration);
    assert_eq!(answer["state"], "confirmed");
    assert!(answer.get("cleanup").is_none(), "{answer}");
    assert!(world.cleanup_rows().is_empty());
    assert!(world.cleanup_messages().is_empty());
    assert!(world.branches().contains(&"feat/dev-1".to_string()));
}

#[test]
fn confirm_with_nothing_to_offer_sends_no_message() {
    let _guard = serial();
    let world = World::new("[cleanup]\nbranches = \"ask\"");
    let integration = world.integrated();
    // The developers are still active, so their branches are protected.
    let answer = confirm(&world, &integration);
    assert_eq!(answer["state"], "confirmed");
    assert!(answer.get("cleanup").is_none(), "{answer}");
    assert!(world.cleanup_rows().is_empty());
    assert!(world.cleanup_messages().is_empty());
}

#[test]
fn a_failing_cleanup_leaves_the_confirm_confirmed_and_is_logged() {
    let _guard = serial();
    let with = World::new("[cleanup]\nbranches = \"ask\"");
    let plain = World::new("[cleanup]\nbranches = \"off\"");
    let (with_id, plain_id) = (with.integrated(), plain.integrated());
    with.end_developers();
    FAIL_AFTER_CONFIRM.store(true, Ordering::SeqCst);
    let failed = confirm(&with, &with_id);
    FAIL_AFTER_CONFIRM.store(false, Ordering::SeqCst);
    let normal = confirm(&plain, &plain_id);
    assert_eq!(failed["state"], "confirmed");
    for key in ["state", "branch", "branchRemoved"] {
        assert_eq!(failed[key], normal[key], "{key}");
    }
    assert!(failed["cleanup"]["error"]
        .as_str()
        .unwrap()
        .contains("injected"));
    assert_eq!(world_state(&with, &with_id), "confirmed");
    let logged = with.logged("cleanup_after_confirm_failed");
    assert_eq!(logged.len(), 1, "{logged:?}");
    assert_eq!(logged[0]["integrationId"], json!(with_id));
    assert!(with.cleanup_rows().is_empty());
}

fn world_state(world: &World, integration_id: &str) -> String {
    let id = integration_id.to_string();
    world
        .deps
        .kernel
        .run(move |core| {
            Ok(core
                .kernel()
                .database
                .connection()
                .query_row(
                    "SELECT state FROM integrations WHERE integration_id = ?",
                    [&id],
                    |r| r.get::<_, String>(0),
                )
                .unwrap())
        })
        .unwrap()
}

// ---------------------------------------------------------------------------------------------- the command

/// A world where feat/dev-1 has landed and feat/dev-2 has not, both with ended agents, and no automatic cleanup.
fn landed_and_unlanded(section: &str) -> World {
    let world = World::new(section);
    let integration = world.integrated();
    world.end_developers();
    confirm(&world, &integration);
    world
}

#[test]
fn only_the_pm_and_the_operator_may_call_cleanup() {
    let world = World::new("[cleanup]\nbranches = \"off\"");
    for who in ["dev-1", "rev-1", "arch-1"] {
        for args in [
            vec![],
            vec!["--dry-run"],
            vec!["apply", "x"],
            vec!["decline", "x"],
        ] {
            world.refused(who, "cleanup", &args, "forbidden");
        }
    }
    world.ok("pm-1", "cleanup", &["--dry-run"]);
    world.ok("operator", "cleanup", &["--dry-run"]);
    // The developers are active, so there is nothing to offer yet; the call itself is allowed.
    for who in ["pm-1", "operator"] {
        assert!(world.ok(who, "cleanup", &[])["cleanupId"].is_null());
    }
    let world = landed_and_unlanded("[cleanup]\nbranches = \"off\"");
    for who in ["pm-1", "operator"] {
        assert!(world.ok(who, "cleanup", &[])["cleanupId"].is_string());
    }
}

#[test]
fn dry_run_groups_the_branches_and_changes_nothing() {
    let world = landed_and_unlanded("[cleanup]\nbranches = \"off\"");
    let before = world.git(&["for-each-ref"]);
    let result = world.ok("pm-1", "cleanup", &["--dry-run"]);
    assert_eq!(result["dryRun"], true);
    let landed = result["landed"].as_array().unwrap();
    assert_eq!(landed.len(), 1);
    assert_eq!(landed[0]["branch"], "feat/dev-1");
    assert_eq!(landed[0]["sha"], world.heads["feat/dev-1"]);
    assert!(landed[0]["reason"].as_str().unwrap().contains("confirmed"));
    let unlanded = result["unlanded"].as_array().unwrap();
    assert_eq!(unlanded.len(), 1);
    assert_eq!(unlanded[0]["branch"], "feat/dev-2");
    let protected: Vec<&str> = result["protected"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["branch"].as_str().unwrap())
        .collect();
    assert_eq!(protected, ["main"]);
    assert_eq!(world.git(&["for-each-ref"]), before);
    assert!(world.cleanup_rows().is_empty(), "a dry run records nothing");
}

#[test]
fn apply_deletes_the_landed_entries_only_and_unlanded_ones_only_when_named() {
    let world = landed_and_unlanded("[cleanup]\nbranches = \"off\"\ninclude_unlanded = true");
    let offer = world.ok("pm-1", "cleanup", &[]);
    assert_eq!(offer["offered"], 2);
    let id = offer["cleanupId"].as_str().unwrap().to_string();
    let applied = world.ok("pm-1", "cleanup", &["apply", &id]);
    let deleted = applied["deleted"].as_array().unwrap();
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0]["branch"], "feat/dev-1");
    assert_eq!(deleted[0]["sha"], world.heads["feat/dev-1"]);
    assert_eq!(
        applied["restore"],
        json!([format!(
            "git branch feat/dev-1 {}",
            world.heads["feat/dev-1"]
        )])
    );
    assert!(world.branches().contains(&"feat/dev-2".to_string()));
    world.refused("pm-1", "cleanup", &["apply", &id], "rejected");

    let offer = world.ok("pm-1", "cleanup", &[]);
    let id = offer["cleanupId"].as_str().unwrap().to_string();
    let applied = world.ok("pm-1", "cleanup", &["apply", &id, "feat/dev-2"]);
    assert_eq!(applied["deleted"][0]["branch"], "feat/dev-2");
    assert!(!world.branches().contains(&"feat/dev-2".to_string()));
}

#[test]
fn decline_deletes_nothing_and_a_settled_offer_cannot_be_applied() {
    let world = landed_and_unlanded("[cleanup]\nbranches = \"off\"");
    let id = world.ok("pm-1", "cleanup", &[])["cleanupId"]
        .as_str()
        .unwrap()
        .to_string();
    let before = world.branches();
    let declined = world.ok("operator", "cleanup", &["decline", &id]);
    assert_eq!(declined["state"], "declined");
    assert_eq!(world.branches(), before);
    for args in [vec!["apply", &id], vec!["decline", &id]] {
        let message = world.refused("pm-1", "cleanup", &args, "rejected");
        assert!(message.contains("not_open"), "{message}");
    }
    // A newer offer supersedes an older one.
    let first = world.ok("pm-1", "cleanup", &[])["cleanupId"]
        .as_str()
        .unwrap()
        .to_string();
    world.ok("pm-1", "cleanup", &[]);
    let message = world.refused("pm-1", "cleanup", &["apply", &first], "rejected");
    assert!(message.contains("superseded"), "{message}");
    let message = world.refused("pm-1", "cleanup", &["apply", "nope"], "rejected");
    assert!(message.contains("unknown_cleanup"), "{message}");
}

#[test]
fn cleanup_is_refused_while_the_run_is_paused_and_while_an_integration_is_running() {
    let world = landed_and_unlanded("[cleanup]\nbranches = \"off\"");
    let id = world.ok("pm-1", "cleanup", &[])["cleanupId"]
        .as_str()
        .unwrap()
        .to_string();
    world.running_integration();
    for args in [vec![], vec!["apply", &id]] {
        let message = world.refused("pm-1", "cleanup", &args, "rejected");
        assert!(message.contains("integration_running"), "{message}");
    }
    let world = landed_and_unlanded("[cleanup]\nbranches = \"off\"");
    let id = world.ok("pm-1", "cleanup", &[])["cleanupId"]
        .as_str()
        .unwrap()
        .to_string();
    let owner = world.credentials["operator"].clone();
    world
        .deps
        .kernel
        .run(move |core| {
            let context = capstan_daemon::deps::new_context(core, &owner)?;
            core.transition_run(&context, &json!("paused"), Some("test"))
        })
        .unwrap();
    for args in [vec![], vec!["apply", &id]] {
        let message = world.refused("operator", "cleanup", &args, "rejected");
        assert!(message.contains("paused"), "{message}");
    }
    assert!(world.branches().contains(&"feat/dev-1".to_string()));
}
