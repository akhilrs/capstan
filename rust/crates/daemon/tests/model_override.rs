//! Model overrides through the command handlers: who may set one, how plan open, plan model, link model and spawn
//! check and answer, what plan show and status carry, and what a review request does when the model of its reviewer
//! changed or does not fit. The launcher is the recording stub; the kernel, the ledger and git are real.

mod common;

use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::handlers::handle_frame;
use capstan_daemon::handlers::shared::AbortSignal;
use capstan_daemon::handlers::MAX_RESPONSE_BYTES;
use capstan_daemon::ports::ProjectGit;
use capstan_daemon::verify::VerifyService;
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::Core;
use capstan_launcher::api::{LauncherError, LauncherService, Placement, SpawnResult, StubLauncher};
use common::project::{build_repo, write_project, Identity};
use common::{private_tempdir, seeded_env};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};

const CONFIG: &str = r#"schema_version = 1

[hosts.claude]
kind = "claude"

[architect]
enabled = true

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
"#;

const AGENTS: [(&str, &str, &str); 7] = [
    ("pm-1", "pm", "PM"),
    ("arch-1", "architect", "Developer"),
    ("dev-1", "developer", "Developer"),
    ("dev-2", "developer", "Developer"),
    ("dev-3", "developer", "Developer"),
    ("rev-1", "reviewer", "Verifier"),
    ("rev-2", "reviewer", "Verifier"),
];

static WORLDS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

struct World {
    _dir: common::PrivateDir,
    _scratch: common::PrivateDir,
    deps: Deps,
    credentials: HashMap<String, String>,
    heads: BTreeMap<String, String>,
    launcher: Arc<StubLauncher>,
    logs: Arc<Mutex<Vec<(String, Value)>>>,
}

impl World {
    /// `more` is appended to capstan.toml; `gate` adds a `[verify]` section whose commands pass.
    fn new(more: &str, gate: bool) -> World {
        Self::with_base(CONFIG, more, gate)
    }

    /// `base` replaces the standard capstan.toml.
    fn with_base(base: &str, more: &str, gate: bool) -> World {
        let dir = private_tempdir();
        let scratch = private_tempdir();
        let identity = Identity {
            project_id: "pmodel".into(),
            name: "Model".into(),
            credential: format!("operator-{}", "w".repeat(40)),
        };
        let state = write_project(dir.path(), &identity);
        let mut config = base.to_string();
        config.push_str(more);
        if gate {
            config.push_str(&format!(
                "\n[verify]\ncommands = [\"echo ok\"]\nscratch_dir = \"{}\"\n",
                scratch.path().display()
            ));
        }
        let file = dir.path().join("capstan.toml");
        std::fs::write(&file, config).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        let trust = common::kept_trust_store();
        common::trusted_project(dir.path(), &trust);
        let heads = build_repo(
            dir.path(),
            &json!({
                "files": {"a.txt": "a\n"},
                "branches": {
                    "feat/dev-1": {"files": {"one.txt": "one\n"}, "message": "feat: one"},
                    "feat/dev-2": {"files": {"two.txt": "two\n"}, "message": "feat: two"},
                    "feat/dev-3": {"files": {"three.txt": "three\n"}, "message": "feat: three"},
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
                    seeded_env(&format!(
                        "model-{}-{}",
                        std::process::id(),
                        WORLDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    )),
                )
            })
            .expect("the kernel opens")
        };
        let logs: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
        let sink: Logger = {
            let logs = logs.clone();
            Arc::new(move |entry: &LogEntry| {
                logs.lock().unwrap().push((
                    entry.command.clone(),
                    entry.detail.clone().unwrap_or(Value::Null),
                ));
            })
        };
        let mut options = DaemonOptions::new(state, project, dir.path().to_path_buf());
        options.trust_store = trust.clone();
        let role_config = capstan_config::load_role_config_with(dir.path(), &trust).unwrap();
        let verify = role_config.verify.clone();
        options.capstan = Some(Arc::new(role_config));
        let launcher = Arc::new(StubLauncher::new());
        let mut deps = Deps::new(kernel, options, sink);
        deps.git = Arc::new(ProjectGit::new(dir.path()));
        deps.launcher = Some(launcher.clone() as Arc<dyn LauncherService>);
        capstan_daemon::deps::startup_sequence(&deps);
        let mut service = None;
        if let Some(verify) = verify {
            let made = VerifyService::with_config(&deps, verify);
            deps.verify = Some(made.clone());
            service = Some(made);
        }
        let mut world = World {
            _dir: dir,
            _scratch: scratch,
            deps,
            credentials: HashMap::new(),
            heads,
            launcher,
            logs,
        };
        world
            .credentials
            .insert("operator".into(), identity.credential);
        world.populate();
        if let Some(service) = service {
            service.start();
        }
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

    fn populate(&mut self) {
        let owner = self.credentials["operator"].clone();
        let roles: Vec<Value> = [
            ("pm", "PM"),
            ("developer", "Developer"),
            ("architect", "Developer"),
            ("reviewer", "Verifier"),
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
                vec![if agent == "dev-1" {
                    json!({"agentId": agent, "roleName": role, "seatId": seat, "actorId": actor["actorId"],
                           "model": "claude-opus-4-8", "modelSource": "plan", "modelScope": "plan-1"})
                } else {
                    json!({"agentId": agent, "roleName": role, "seatId": seat, "actorId": actor["actorId"]})
                }],
            );
            self.credentials
                .insert(agent.into(), actor["credential"].as_str().unwrap().into());
        }
        let internal = self.deps.credential().to_string();
        for agent in ["dev-1", "dev-2", "dev-3"] {
            self.ledger(
                &internal,
                "recordAgentPane",
                vec![json!({
                    "agentId": agent,
                    "workspaceId": "ws-1",
                    "paneId": format!("pane-{agent}"),
                    "worktreePath": format!("/tmp/wt/{agent}"),
                    "branch": format!("feat/{agent}"),
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

    fn refused(&self, who: &str, command: &str, args: &[&str]) -> String {
        let answer = self.call(who, command, args);
        assert_eq!(answer["ok"], false, "{command} {args:?}: {answer}");
        format!("{}: {}", answer["code"], answer["message"])
    }

    /// Reads and acknowledges what waits for `who`; returns the bodies.
    fn drain(&self, who: &str) -> Vec<String> {
        let inbox = self.ok(who, "inbox", &[]);
        let mut bodies = Vec::new();
        for message in inbox["messages"].as_array().into_iter().flatten() {
            bodies.push(message["body"].as_str().unwrap_or("").to_string());
            self.ok(who, "ack", &[message["messageId"].as_str().unwrap()]);
        }
        bodies
    }

    /// An approved plan `plan-1` of `core`, `ui` and `docs`, assigned to dev-1, dev-2 and dev-3.
    fn plan(&self) -> &'static str {
        self.ok("pm-1", "plan", &["open", "normal", "Work"]);
        let package = |id: &str| json!({"id": id, "title": id, "owns": [format!("src/{id}/")], "acceptance": [format!("{id} works")], "estimate_hours": 2});
        let body = json!({"summary": "split", "packages": [package("core"), package("ui"), package("docs")]});
        self.ok("arch-1", "plan", &["submit", "plan-1", &body.to_string()]);
        for (package, agent) in [("core", "dev-1"), ("ui", "dev-2"), ("docs", "dev-3")] {
            self.ok("pm-1", "plan", &["assign", "plan-1", package, agent]);
        }
        "plan-1"
    }

    /// A report of `agent` at its branch tip.
    fn report(&self, agent: &str) -> (String, String) {
        self.drain(agent);
        let sha = self.heads[&format!("feat/{agent}")].clone();
        let report = self.ok(agent, "report", &[&sha, "Done"]);
        (report["reportId"].as_str().unwrap().to_string(), sha)
    }

    fn script_spawn(&self, agent: &str) {
        self.launcher.script_spawn(SpawnResult {
            state: "started".into(),
            agent_id: agent.into(),
            pane_id: "w1:p1".into(),
            worktree_path: format!("/tmp/work/{agent}"),
            branch: format!("capstan/{agent}"),
            placement: Placement::Tab,
            placement_note: None,
            hint: None,
            missing_env: None,
            warning: None,
        });
    }

    fn calls(&self) -> Vec<String> {
        self.launcher.calls()
    }

    fn released(&self, agent: &str) -> bool {
        self.calls()
            .iter()
            .any(|c| *c == format!("release {agent}"))
    }

    fn logged(&self, event: &str) -> Vec<Value> {
        self.logs
            .lock()
            .unwrap()
            .iter()
            .filter(|(command, _)| command == &format!("daemon:{event}"))
            .map(|(_, detail)| detail.clone())
            .collect()
    }

    fn spawns(&self) -> usize {
        self.calls()
            .iter()
            .filter(|c| c.starts_with("spawn "))
            .count()
    }

    fn warm(&self) -> Vec<Value> {
        let credential = self.deps.credential().to_string();
        match self
            .deps
            .kernel
            .run(move |core| core.warm_reviewers(&credential))
            .unwrap()
        {
            Value::Array(rows) => rows,
            other => panic!("{other}"),
        }
    }
}

const CODEX_DEVELOPERS: &str = r#"schema_version = 1

[hosts.claude]
kind = "claude"

[hosts.codex]
kind = "codex"

[architect]
enabled = true

[roles.pm]
kind = "PM"
host = "claude"

[roles.developer]
kind = "Developer"
host = "codex"

[roles.architect]
kind = "Developer"
host = "claude"

[roles.reviewer]
kind = "Verifier"
host = "claude"
"#;

const MIXED: &str = r#"schema_version = 1

[hosts.claude]
kind = "claude"

[hosts.codex]
kind = "codex"

[architect]
enabled = true

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
host = "codex"
"#;

const CLAUDE_MODEL: &str = "claude-opus-4-8";

#[test]
fn only_the_pm_and_the_operator_set_a_model_override() {
    let w = World::new("", false);
    w.plan();
    for who in ["dev-1", "rev-1", "arch-1"] {
        for (command, args) in [
            ("spawn", vec!["developer", "--model", CLAUDE_MODEL]),
            ("plan", vec!["model", "plan-1", CLAUDE_MODEL]),
            ("link", vec!["model", "req-1", CLAUDE_MODEL]),
        ] {
            let refusal = w.refused(who, command, &args);
            assert!(
                refusal.contains("Forbidden") || refusal.contains("forbidden"),
                "{who} {command}: {refusal}"
            );
        }
    }
    assert!(w
        .refused("dev-1", "plan", &["model", "plan-1", CLAUDE_MODEL])
        .contains("may set a model override"));
    w.ok("pm-1", "plan", &["model", "plan-1", CLAUDE_MODEL]);
    w.ok(
        "operator",
        "plan",
        &["model", "plan-1", "Developer", "claude-sonnet-5-5"],
    );
    assert_eq!(w.spawns(), 0);
}

#[test]
fn set_change_and_clear_round_trip_in_plan_show() {
    let w = World::new("", false);
    w.plan();
    assert!(w
        .ok("pm-1", "plan", &["show", "plan-1"])
        .get("modelOverrides")
        .is_none());
    let set = w.ok("pm-1", "plan", &["model", "plan-1", CLAUDE_MODEL]);
    assert_eq!(set["previous"], Value::Null);
    assert!(set["running"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["agentId"] == "dev-1"));
    let shown = w.ok("pm-1", "plan", &["show", "plan-1"]);
    assert_eq!(shown["modelOverrides"][0]["model"], CLAUDE_MODEL);
    assert_eq!(shown["modelOverrides"][0]["roleKind"], Value::Null);
    let changed = w.ok("pm-1", "plan", &["model", "plan-1", "haiku"]);
    assert_eq!(changed["previous"], CLAUDE_MODEL);
    w.ok("pm-1", "plan", &["model", "plan-1", "Verifier", "sonnet"]);
    assert_eq!(
        w.ok("pm-1", "plan", &["model", "plan-1"])["modelOverrides"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let cleared = w.ok("pm-1", "plan", &["model", "plan-1", "--clear"]);
    assert_eq!(cleared["cleared"], true);
    w.ok("pm-1", "plan", &["model", "plan-1", "Verifier", "--clear"]);
    assert!(w
        .ok("pm-1", "plan", &["show", "plan-1"])
        .get("modelOverrides")
        .is_none());
    // One argument that is a kind name or a flag names no kind.
    for bad in ["Developer", "--other"] {
        assert!(w
            .refused("pm-1", "plan", &["model", "plan-1", bad])
            .contains("name the kind before the model"));
    }
    assert!(w
        .refused("pm-1", "plan", &["model", "plan-1", "developer", "haiku"])
        .contains("name the kind"));
}

#[test]
fn plan_show_names_the_models_of_the_architect_and_assignees() {
    let w = World::new("", false);
    w.plan();
    let shown = w.ok("pm-1", "plan", &["show", "plan-1"]);
    let models = shown["agentModels"].as_array().unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0]["agentId"], "dev-1");
    assert_eq!(models[0]["role"], "developer");
    assert_eq!(models[0]["model"], "claude-opus-4-8");
    assert_eq!(models[0]["modelSource"], "plan");
    // The members of the status answer reach the client.
    let status = w.ok("pm-1", "status", &[]);
    let dev = status["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["agentId"] == "dev-1")
        .unwrap()
        .clone();
    assert_eq!(dev["model"], "claude-opus-4-8");
    assert_eq!(dev["modelSource"], "plan");
    let other = status["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["agentId"] == "dev-2")
        .unwrap()
        .clone();
    assert!(other.get("model").is_none());
}

#[test]
fn plan_open_writes_its_overrides_in_one_step_or_none() {
    let w = World::new("", false);
    let opened = w.ok(
        "pm-1",
        "plan",
        &[
            "open",
            "normal",
            "Work",
            "--model",
            CLAUDE_MODEL,
            "--model",
            "Verifier=sonnet",
        ],
    );
    assert_eq!(opened["modelOverrides"].as_array().unwrap().len(), 2);
    let id = opened["planId"].as_str().unwrap();
    assert_eq!(
        w.ok("pm-1", "plan", &["model", id])["modelOverrides"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let before = w.ok("pm-1", "plan", &["show"])["plans"]
        .as_array()
        .unwrap()
        .len();
    for args in [
        vec!["open", "normal", "Two", "--model", "PM=sonnet"],
        vec![
            "open",
            "normal",
            "Two",
            "--model",
            "Verifier=sonnet",
            "--model",
            "Verifier=haiku",
        ],
        vec![
            "open", "normal", "Two", "--model", "sonnet", "--model", "haiku",
        ],
        vec!["open", "normal", "Two", "--model", "developer=sonnet"],
        vec!["open", "normal", "Two", "--model", "-bad"],
    ] {
        let refusal = w.refused("pm-1", "plan", &args);
        assert!(refusal.contains("invalid_request"), "{args:?}: {refusal}");
    }
    assert_eq!(
        w.ok("pm-1", "plan", &["show"])["plans"]
            .as_array()
            .unwrap()
            .len(),
        before
    );
    assert!(w
        .ok("pm-1", "plan", &["open", "normal", "Plain"])
        .get("modelOverrides")
        .is_none());
}

#[test]
fn a_model_that_no_role_of_the_kind_runs_is_refused_and_a_partial_fit_warns() {
    // Every Developer role runs claude: a gpt model fits none of them.
    let w = World::new("", false);
    w.plan();
    let refusal = w.refused("pm-1", "plan", &["model", "plan-1", "Developer", "gpt-5"]);
    assert!(refusal.contains("model_host_mismatch"), "{refusal}");
    assert!(w
        .ok("pm-1", "plan", &["show", "plan-1"])
        .get("modelOverrides")
        .is_none());
    let plans = w.ok("pm-1", "plan", &["show"])["plans"]
        .as_array()
        .unwrap()
        .len();
    assert!(w
        .refused(
            "pm-1",
            "plan",
            &["open", "normal", "X", "--model", "Developer=gpt-5"]
        )
        .contains("model_host_mismatch"));
    assert_eq!(
        w.ok("pm-1", "plan", &["show"])["plans"]
            .as_array()
            .unwrap()
            .len(),
        plans
    );
    // The developer role runs codex, the architect role claude: a claude model is set with a warning.
    let w = World::with_base(CODEX_DEVELOPERS, "", false);
    w.plan();
    let all = w.ok("pm-1", "plan", &["model", "plan-1", CLAUDE_MODEL]);
    let warnings = all["warnings"].as_array().unwrap();
    assert!(warnings.iter().any(|w| w
        .as_str()
        .unwrap()
        .contains("developer on codex (codex) will refuse to spawn with this model")));
    // The spawn flag is checked against the role's host and spawns nothing.
    let refused = w.refused("pm-1", "spawn", &["developer", "--model", CLAUDE_MODEL]);
    assert!(refused.contains("invalid_request"), "{refused}");
    assert_eq!(w.spawns(), 0);
    w.script_spawn("dev-9");
    w.ok("pm-1", "spawn", &["developer", "--model", "gpt-5"]);
    assert_eq!(w.spawns(), 1);
}

#[test]
fn spawn_takes_a_plan_or_a_task_not_both() {
    let w = World::new("", false);
    w.plan();
    assert!(w
        .refused(
            "pm-1",
            "spawn",
            &["developer", "--task", "plan-1/core", "--plan", "plan-1"]
        )
        .contains("use --task or --plan, not both"));
    assert!(w
        .refused("pm-1", "spawn", &["developer", "--plan", "plan-nope"])
        .contains("no plan"));
    assert_eq!(w.spawns(), 0);
    w.script_spawn("dev-9");
    w.ok(
        "pm-1",
        "spawn",
        &["developer", "--plan", "plan-1", "--model", "haiku"],
    );
    let logged = w.logged("spawn_requested");
    assert_eq!(logged[0]["model"], "haiku");
    assert_eq!(logged[0]["plan"], "plan-1");
}

#[test]
fn a_refused_requirement_model_creates_no_link() {
    let w = World::new("", false);
    let refusal = w.refused(
        "pm-1",
        "link",
        &["requirement", "req-1", "NX-1", "--model", "bad model\u{1}"],
    );
    assert!(refusal.contains("invalid_request"), "{refusal}");
    // Nothing was linked: model has no requirement to attach to.
    assert!(w
        .refused("pm-1", "link", &["model", "req-1", "haiku"])
        .contains("not linked"));
    assert!(w.ok("pm-1", "link", &["model", "req-1"])["modelOverrides"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn a_requirement_override_is_set_with_the_link_and_shown_by_link_model() {
    let w = World::new("", false);
    let linked = w.ok(
        "pm-1",
        "link",
        &["requirement", "req-1", "NX-1", "--model", CLAUDE_MODEL],
    );
    assert_eq!(linked["modelOverrides"][0]["model"], CLAUDE_MODEL);
    let shown = w.ok("pm-1", "link", &["model", "req-1"]);
    assert_eq!(shown["modelOverrides"][0]["model"], CLAUDE_MODEL);
    w.ok("pm-1", "link", &["model", "req-1", "Developer", "haiku"]);
    w.ok("pm-1", "link", &["model", "req-1", "--clear"]);
    assert_eq!(
        w.ok("pm-1", "link", &["model", "req-1"])["modelOverrides"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(w
        .refused(
            "pm-1",
            "link",
            &["plan", "plan-1", "NX-2", "--model", "haiku"]
        )
        .contains("requirement"));
}

#[test]
fn a_review_whose_reviewer_cannot_run_the_override_is_refused_once() {
    let w = World::with_base(MIXED, "", false);
    w.plan();
    let (report, _) = w.report("dev-1");
    // Developers run claude, the reviewer role runs codex: set with a warning, refused at the fresh spawn.
    let set = w.ok("pm-1", "plan", &["model", "plan-1", CLAUDE_MODEL]);
    assert!(set["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w.as_str().unwrap().contains("reviewer on codex")));
    w.launcher.fail_next(
        "spawn",
        LauncherError::new(
            "model_host_mismatch",
            "model is a claude model and host codex is codex",
        ),
    );
    let refusal = w.refused("pm-1", "request-review", &[&report]);
    assert!(refusal.contains("model_host_mismatch"), "{refusal}");
    assert_eq!(w.spawns(), 1);
    assert!(w.warm().is_empty());
    // No review was begun: clearing the override lets the same report be asked again.
    w.ok("pm-1", "plan", &["model", "plan-1", "--clear"]);
    w.script_spawn("rev-1");
    let again = w.ok("pm-1", "request-review", &[&report]);
    assert_eq!(again["reviewerAgentId"], "rev-1");
    assert_eq!(w.spawns(), 2);
}

#[test]
fn a_warm_reviewer_whose_model_changed_is_replaced() {
    let w = World::new("", false);
    w.plan();
    let (first, _) = w.report("dev-1");
    let (second, _) = w.report("dev-2");
    w.script_spawn("rev-1");
    w.ok("pm-1", "request-review", &[&first]);
    w.ok("rev-1", "review", &["pass", "fine"]);
    w.ok("pm-1", "plan", &["model", "plan-1", "Verifier", "sonnet"]);
    w.launcher.fail_next(
        "retarget_review",
        LauncherError::new(
            "model_changed",
            "rev-1 runs no model and the review wants sonnet",
        ),
    );
    w.script_spawn("rev-2");
    let two = w.ok("pm-1", "request-review", &[&second]);
    assert_eq!(two["reviewerAgentId"], "rev-2");
    assert_eq!(two["reviewerReused"], false);
    let logged = w.logged("review_reviewer_model_changed");
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0]["agentId"], "rev-1");
    assert!(w.logged("review_retarget_failed").is_empty());
    assert!(w.released("rev-1"));
    assert!(w
        .calls()
        .iter()
        .any(|c| c.starts_with("spawn reviewer ") && c.ends_with(&second)));
    assert_eq!(w.warm().len(), 1);
    assert_eq!(w.warm()[0]["agentId"], "rev-2");
}
