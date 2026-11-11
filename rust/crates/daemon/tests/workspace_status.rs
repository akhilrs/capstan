//! The per-repo members of `status` (plan-185): integration sets, set members on the integration rows, report commits and
//! agent worktrees; a project without workspace rows answers as it always did. The kernel, the ledger and the daemon's
//! command handlers are real; the launcher is a stub.

mod common;

use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::handlers::handle_frame;
use capstan_daemon::handlers::shared::AbortSignal;
use capstan_daemon::handlers::MAX_RESPONSE_BYTES;
use capstan_daemon::ports::ProjectGit;
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::Core;
use capstan_launcher::api::{LauncherService, StubLauncher};
use common::private_tempdir;
use common::project::{build_repo, write_project, Identity};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;

const CONFIG: &str = r#"schema_version = 1

[hosts.claude]
kind = "claude"

[roles.pm]
kind = "PM"
host = "claude"

[roles.developer]
kind = "Developer"
host = "claude"
"#;

const BASE: &str = "0123456789abcdef0123456789abcdef01234567";
const COMMIT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const COMMIT_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const PROJECT: &str = "wsstatus";

static WORLDS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

struct World {
    _dir: common::PrivateDir,
    state: PathBuf,
    deps: Deps,
    credentials: HashMap<String, String>,
}

impl World {
    fn new() -> World {
        World::with_config(CONFIG)
    }

    fn with_config(config: &str) -> World {
        let dir = private_tempdir();
        let identity = Identity {
            project_id: PROJECT.into(),
            name: "Workspace status".into(),
            credential: format!("operator-{}", "w".repeat(40)),
        };
        let state = write_project(dir.path(), &identity);
        let file = dir.path().join("capstan.toml");
        std::fs::write(&file, config).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        let trust = common::kept_trust_store();
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
                    common::seeded_env(&format!(
                        "wsstatus-{}-{}",
                        std::process::id(),
                        WORLDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    )),
                )
            })
            .expect("the kernel opens")
        };
        let sink: Logger = Arc::new(|_: &LogEntry| {});
        let mut options = DaemonOptions::new(state.clone(), project, dir.path().to_path_buf());
        options.trust_store = trust.clone();
        let role_config = capstan_config::load_role_config_with(dir.path(), &trust).unwrap();
        options.capstan = Some(Arc::new(role_config));
        let mut deps = Deps::new(kernel, options, sink);
        deps.git = Arc::new(ProjectGit::new(dir.path()));
        deps.launcher = Some(Arc::new(StubLauncher::new()) as Arc<dyn LauncherService>);
        capstan_daemon::deps::startup_sequence(&deps);
        let mut world = World {
            _dir: dir,
            state,
            deps,
            credentials: HashMap::new(),
        };
        world
            .credentials
            .insert("operator".into(), identity.credential);
        world.populate();
        world
    }

    fn ledger(&self, credential: &str, op: &str, args: Vec<Value>) -> Value {
        let credential = credential.to_string();
        let name = op.to_string();
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

    /// A workspace writer of `Core` (not a dispatch name), run as the operator.
    fn workspace(&self, op: &'static str, input: Value) -> Value {
        let credential = self.credentials["operator"].clone();
        self.deps
            .kernel
            .run(move |core| {
                let context = capstan_daemon::deps::new_context(core, &credential)?;
                match op {
                    "create_set" => core.create_integration_set(&context, &input),
                    "attach" => core.attach_set_member(&context, &input),
                    "worktree" => core.record_agent_worktree(&context, &input),
                    _ => unreachable!(),
                }
            })
            .unwrap_or_else(|e| panic!("{op}: {e}"))
    }

    fn populate(&mut self) {
        let owner = self.credentials["operator"].clone();
        let roles: Vec<Value> = [("pm", "PM"), ("developer", "Developer")]
            .iter()
            .map(|(name, kind)| {
                json!({"name": name, "kind": kind, "host": "claude", "configHash": "a".repeat(64)})
            })
            .collect();
        self.ledger(&owner, "syncRoleDefinitions", vec![json!(roles)]);
        for (agent, role, kind) in [("pm-1", "pm", "PM"), ("dev-1", "developer", "Developer")] {
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
    }

    fn db(&self) -> rusqlite::Connection {
        let path = capstan_ledger::resolve_database_path(&self.state).unwrap();
        rusqlite::Connection::open(path).unwrap()
    }

    fn status(&self) -> Value {
        let frame = json!({"v": 1, "credential": self.credentials["operator"], "command": "status", "args": []});
        let outcome = handle_frame(
            &self.deps,
            frame.to_string().as_bytes(),
            &AbortSignal::new(),
            MAX_RESPONSE_BYTES,
        );
        let answer: Value = serde_json::from_str(&outcome.response.expect("an answer")).unwrap();
        assert_eq!(answer["ok"], true, "{answer}");
        answer["result"].clone()
    }

    fn integration(&self, id: &str, sequence: i64, state: &str, head: Option<&str>) {
        let conn = self.db();
        if state == "conflicted" {
            let actor: String = conn
                .query_row(
                    "SELECT actor_id FROM agents WHERE agent_id = 'dev-1'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO agent_reports(project_id, report_id, sequence, agent_id, generation, actor_id, commit_sha, branch, summary, state, evidence_json, created_at)
                 VALUES (?, 'conflict-report', 900, 'dev-1', 1, ?, ?, 'b', 's', 'accepted', '{}', '2026-01-01T00:00:00.000Z')",
                rusqlite::params![PROJECT, actor, COMMIT_B],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, head_sha, created_at, completed_at,
                 failure_reason, conflict_report_id, conflict_files_json, conflict_files_omitted)
             VALUES (?, ?, ?, ?, ?, 'pm-1', ?, ?, '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z', NULL, ?, ?, ?)",
            rusqlite::params![
                PROJECT,
                id,
                sequence,
                BASE,
                format!("int/{id}"),
                state,
                head,
                (state == "conflicted").then_some("conflict-report"),
                (state == "conflicted").then_some("[\"f.txt\"]"),
                (state == "conflicted").then_some(0),
            ],
        )
        .unwrap();
    }
}

fn seed_workspace(world: &World) -> String {
    let set = world.workspace(
        "create_set",
        json!({"branch": "int/set", "repos": ["api", "web", "worker"]}),
    )["setId"]
        .as_str()
        .unwrap()
        .to_string();
    world.integration("i-api", 1, "confirmed", Some(COMMIT_A));
    world.integration("i-web", 2, "merged", Some(COMMIT_B));
    world.integration("i-worker", 3, "conflicted", None);
    for (repo, id) in [("api", "i-api"), ("web", "i-web"), ("worker", "i-worker")] {
        world.workspace(
            "attach",
            json!({"setId": set, "repo": repo, "integrationId": id}),
        );
    }
    // A report in two repos, and the worktrees of dev-1.
    let conn = world.db();
    let actor: String = conn
        .query_row(
            "SELECT actor_id FROM agents WHERE agent_id = 'dev-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO agent_reports(project_id, report_id, sequence, agent_id, generation, actor_id, commit_sha, branch, summary, state, evidence_json, created_at)
         VALUES (?, 'rep-1', 1, 'dev-1', 1, ?, ?, 'feat/x', 'both', 'accepted', '{}', '2026-01-01T00:00:00.000Z')",
        rusqlite::params![PROJECT, actor, COMMIT_A],
    )
    .unwrap();
    for (repo, sha) in [("api", COMMIT_A), ("web", COMMIT_B)] {
        conn.execute(
            "INSERT INTO report_commits(project_id, report_id, repo, commit_sha, branch, base_sha) VALUES (?, 'rep-1', ?, ?, 'feat/x', ?)",
            rusqlite::params![PROJECT, repo, sha, BASE],
        )
        .unwrap();
    }
    for repo in ["api", "web"] {
        world.workspace(
            "worktree",
            json!({"agentId": "dev-1", "generation": 1, "repo": repo, "worktreePath": format!("/cf/dev-1/{repo}"),
                "branch": "feat/x", "baseSha": BASE}),
        );
    }
    set
}

#[test]
fn status_lists_the_sets_the_members_of_the_rows_the_commits_of_a_report_and_the_worktrees_of_an_agent(
) {
    let world = World::new();
    let set = seed_workspace(&world);
    let status = world.status();

    let sets = status["integrationSets"]
        .as_array()
        .expect("integrationSets");
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0]["setId"], set.as_str());
    assert_eq!(sets[0]["state"], "running");
    assert_eq!(sets[0]["branch"], "int/set");
    assert_eq!(sets[0]["confirmed"], false);
    let members = sets[0]["members"].as_array().unwrap();
    let order: Vec<(&str, &str)> = members
        .iter()
        .map(|m| (m["repo"].as_str().unwrap(), m["state"].as_str().unwrap()))
        .collect();
    assert_eq!(
        order,
        [
            ("api", "confirmed"),
            ("web", "merged"),
            ("worker", "conflicted")
        ]
    );
    assert_eq!(members[1]["headSha"], COMMIT_B);
    // No verify run yet: every live member waits at the gate.
    assert_eq!(
        sets[0]["gate"],
        json!({"state": "waiting", "repos": ["api", "web", "worker"]})
    );
    assert_eq!(members[0]["verify"], Value::Null);

    // Each member keeps its integration row, which now carries its set.
    let rows = status["integrations"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    for row in rows {
        assert_eq!(row["set"]["setId"], set.as_str(), "{row}");
        assert_eq!(row["set"]["members"].as_array().unwrap().len(), 3);
    }
    let web = rows.iter().find(|r| r["integrationId"] == "i-web").unwrap();
    assert_eq!(web["set"]["repo"], "web");
    assert_eq!(web["set"]["position"], 1);

    // The report keeps its commitSha and lists a commit per repo.
    let reports = status["reports"].as_array().unwrap();
    let report = reports.iter().find(|r| r["reportId"] == "rep-1").unwrap();
    // The conflict report named no commits and has no `commits` member.
    let other = reports
        .iter()
        .find(|r| r["reportId"] == "conflict-report")
        .unwrap();
    assert!(other.get("commits").is_none());
    assert_eq!(report["commitSha"], COMMIT_A);
    assert_eq!(
        report["commits"],
        json!([
            {"repo": "api", "commitSha": COMMIT_A, "branch": "feat/x", "baseSha": BASE},
            {"repo": "web", "commitSha": COMMIT_B, "branch": "feat/x", "baseSha": BASE},
        ])
    );

    // The agent that holds worktrees lists them; the others do not.
    let agents = status["agents"].as_array().unwrap();
    let dev = agents.iter().find(|a| a["agentId"] == "dev-1").unwrap();
    assert_eq!(
        dev["worktrees"],
        json!([
            {"repo": "api", "path": "/cf/dev-1/api", "branch": "feat/x"},
            {"repo": "web", "path": "/cf/dev-1/web", "branch": "feat/x"},
        ])
    );
    assert_eq!(dev["changeFolder"], "/cf/dev-1");
    let pm = agents.iter().find(|a| a["agentId"] == "pm-1").unwrap();
    assert!(pm.get("worktrees").is_none() && pm.get("changeFolder").is_none());
}

#[test]
fn a_project_without_workspace_rows_answers_without_the_new_members() {
    let world = World::new();
    world.integration("i-1", 1, "merged", Some(COMMIT_A));
    let status = world.status();
    assert!(status.get("integrationSets").is_none());
    let row = &status["integrations"][0];
    assert_eq!(row["integrationId"], "i-1");
    assert!(row.get("set").is_none());
    let keys: Vec<&str> = row
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "integrationId",
            "state",
            "branch",
            "reports",
            "conflictReportId",
            "createdAt"
        ]
    );
    for agent in status["agents"].as_array().unwrap() {
        assert!(agent.get("worktrees").is_none());
        assert!(agent.get("changeFolder").is_none());
    }
}

#[test]
fn cstan_json_status_passes_the_members_through_and_the_doc_names_them() {
    // `cstan --json status` prints the daemon's result whole, so the members are the ones above; the document lists each.
    let doc = include_str!("../../../../docs/remote-protocol.md");
    for member in [
        "integrationSets",
        "worktrees",
        "changeFolder",
        "commits",
        "verify.byRepo",
        "reviewSubject",
        "dropped",
    ] {
        assert!(
            doc.contains(&format!("`{member}`")),
            "the doc lacks {member}"
        );
    }
}

#[test]
fn a_single_repo_status_has_none_of_the_per_repo_keys() {
    let world = World::new();
    world.integration("i-1", 1, "merged", Some(COMMIT_A));
    let text = world.status().to_string();
    for key in [
        "integrationSets",
        "commits",
        "worktrees",
        "changeFolder",
        "byRepo",
        "gate",
        "set",
    ] {
        assert!(
            !text.contains(&format!("\"{key}\":")),
            "a single-repo status shows `{key}`: {text}"
        );
    }
}

#[test]
fn a_set_or_member_change_shows_in_the_next_status_even_when_the_answer_is_cached() {
    let world = World::with_config(&format!("{CONFIG}\n[daemon]\nstatus_cache_ms = 60000\n"));
    let first = world.status();
    assert!(first.get("integrationSets").is_none());
    // A repeat call is the cached answer.
    assert_eq!(world.status(), first);
    let set = world.workspace(
        "create_set",
        json!({"branch": "int/set", "repos": ["api", "web"]}),
    )["setId"]
        .as_str()
        .unwrap()
        .to_string();
    let created = world.status();
    assert_eq!(
        created["integrationSets"][0]["setId"],
        set.as_str(),
        "a new set is seen at once"
    );
    world.integration("i-api", 1, "merged", Some(COMMIT_A));
    world.workspace(
        "attach",
        json!({"setId": set, "repo": "api", "integrationId": "i-api"}),
    );
    let attached = world.status();
    assert_eq!(
        attached["integrationSets"][0]["members"][0]["integrationId"],
        "i-api"
    );
    assert_eq!(
        attached["integrations"][0]["set"]["setId"],
        set.as_str(),
        "the row carries its set"
    );
    assert_ne!(attached["stateVersion"], created["stateVersion"]);
    world.workspace(
        "worktree",
        json!({"agentId": "dev-1", "generation": 1, "repo": "api", "worktreePath": "/cf/dev-1/api", "branch": "b", "baseSha": BASE}),
    );
    let tree = world.status();
    let dev = tree["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["agentId"] == "dev-1")
        .unwrap()
        .clone();
    assert_eq!(dev["worktrees"][0]["repo"], "api");
}

#[test]
fn a_merge_or_confirm_refreshes_the_set_row_together_with_its_integration_rows() {
    let world = World::with_config(&format!("{CONFIG}\n[daemon]\nstatus_cache_ms = 60000\n"));
    let set = world.workspace("create_set", json!({"branch": "int/set", "repos": ["api"]}))
        ["setId"]
        .as_str()
        .unwrap()
        .to_string();
    world.integration("i-api", 1, "merged", Some(COMMIT_A));
    world.workspace(
        "attach",
        json!({"setId": set, "repo": "api", "integrationId": "i-api"}),
    );
    let merged = world.status();
    assert_eq!(
        merged["integrationSets"][0]["members"][0]["state"],
        "merged"
    );
    assert_eq!(merged["integrationSets"][0]["confirmed"], false);

    // The confirm lands in the ledger and bumps the project's state version, as every mutation does.
    let conn = world.db();
    conn.execute(
        "UPDATE integrations SET state = 'confirmed' WHERE integration_id = 'i-api'",
        [],
    )
    .unwrap();
    // Not yet bumped: the cached answer is the one before the confirm, set and rows alike.
    assert_eq!(world.status(), merged);
    conn.execute(
        "UPDATE projects SET state_version = state_version + 1 WHERE project_id = ?",
        [PROJECT],
    )
    .unwrap();
    let confirmed = world.status();
    // One key moves the set row and the integration rows together.
    assert_eq!(
        confirmed["integrationSets"][0]["members"][0]["state"],
        "confirmed"
    );
    assert_eq!(confirmed["integrationSets"][0]["confirmed"], true);
    assert_eq!(confirmed["integrations"][0]["state"], "confirmed");
    assert_eq!(
        confirmed["integrations"][0]["set"]["members"][0]["state"],
        "confirmed"
    );
}
