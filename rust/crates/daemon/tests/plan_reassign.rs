//! `plan reassign` through the command handlers: who may run it, what it refuses, and the plan carrying on under its new
//! architect, for a plan in review and for an approved plan with an assigned package. The launcher is the recording stub;
//! the kernel, the ledger and git are real.

mod common;

use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::handlers::handle_frame;
use capstan_daemon::handlers::shared::AbortSignal;
use capstan_daemon::handlers::MAX_RESPONSE_BYTES;
use capstan_daemon::ports::ProjectGit;
use capstan_daemon::verify::VerifyService;
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::Core;
use capstan_launcher::api::{LauncherService, Placement, SpawnResult, StubLauncher};
use common::project::{build_repo, write_project, Identity};
use common::{private_tempdir, seeded_env};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

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

const AGENTS: [(&str, &str, &str); 9] = [
    ("pm-1", "pm", "PM"),
    ("arch-1", "architect", "Developer"),
    ("arch-2", "architect", "Developer"),
    ("arch-3", "architect", "Developer"),
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
}

impl World {
    /// `more` is appended to capstan.toml; `gate` adds a `[verify]` section whose commands pass.
    fn new(more: &str, gate: bool) -> World {
        let dir = private_tempdir();
        let scratch = private_tempdir();
        let identity = Identity {
            project_id: "preassign".into(),
            name: "Reassign".into(),
            credential: format!("operator-{}", "w".repeat(40)),
        };
        let state = write_project(dir.path(), &identity);
        let mut config = CONFIG.to_string();
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
                        "reassign-{}-{}",
                        std::process::id(),
                        WORLDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    )),
                )
            })
            .expect("the kernel opens")
        };
        let sink: Logger = Arc::new(|_: &LogEntry| {});
        let mut options = DaemonOptions::new(state, project, dir.path().to_path_buf());
        let role_config = capstan_config::load_role_config(dir.path()).unwrap();
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
                vec![json!({"agentId": agent, "roleName": role, "seatId": seat, "actorId": actor["actorId"]})],
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

    /// Ends `agent` the way a lost pane does.
    fn end(&self, agent: &str) {
        let credential = self.credentials["operator"].clone();
        let agent = agent.to_string();
        self.deps
            .kernel
            .run(move |core| {
                let context = capstan_daemon::deps::new_context(core, &credential)?;
                core.end_agent(&context, &agent, None)
            })
            .unwrap();
    }

    fn package(id: &str) -> Value {
        json!({"id": id, "title": id, "owns": [format!("src/{id}/")], "acceptance": [format!("{id} works")], "estimate_hours": 2})
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
}

const USAGE: &str = "plan reassign needs a plan id and an architect agent id";

impl World {
    /// An approved plan `plan-1` with the package `api`, assigned to dev-1; arch-1 owns it.
    fn approved(&self) {
        self.ok("pm-1", "plan", &["open", "normal", "Work"]);
        let body = json!({"summary": "one", "packages": [World::package("api")]});
        self.ok("arch-1", "plan", &["submit", "plan-1", &body.to_string()]);
        self.ok("pm-1", "plan", &["assign", "plan-1", "api", "dev-1"]);
    }

    /// Reads `who`'s inbox, acknowledging what is there, and returns the bodies that contain `needle`.
    fn notices(&self, who: &str, needle: &str) -> Vec<String> {
        self.drain(who)
            .into_iter()
            .filter(|body| body.contains(needle))
            .collect()
    }
}

#[test]
fn the_pm_and_the_operator_reassign_a_plan() {
    let w = World::new("", false);
    w.approved();
    w.end("arch-1");
    let done = w.ok(
        "pm-1",
        "plan",
        &["reassign", "plan-1", "arch-2", "--reason", "pane lost"],
    );
    assert_eq!(done["planId"], "plan-1");
    assert_eq!(done["fromAgentId"], "arch-1");
    assert_eq!(done["toAgentId"], "arch-2");
    assert_eq!(done["state"], "approved");
    assert!(done["handoverMessageId"].is_string(), "{done}");
    assert_eq!(done["notifiedDevelopers"], json!(["dev-1"]));
    assert!(done["reassignedAt"].is_string(), "{done}");
    let shown = w.ok("pm-1", "plan", &["show", "plan-1"]);
    assert_eq!(shown["plan"]["architectAgentId"], "arch-2");
    assert_eq!(shown["plan"]["reassignments"][0]["reason"], "pane lost");
    let brief = w.ok("pm-1", "plan", &["show", "plan-1", "--brief"]);
    assert_eq!(brief["plan"]["lastReassignment"]["toAgentId"], "arch-2");
    let listed = w.ok("pm-1", "plan", &["show", "--brief"]);
    assert_eq!(
        listed["plans"][0]["lastReassignment"]["fromAgentId"],
        "arch-1"
    );

    // The operator does the same, from the new owner on to the next.
    w.end("arch-2");
    let again = w.ok("operator", "plan", &["reassign", "plan-1", "arch-3"]);
    assert_eq!(again["fromAgentId"], "arch-2");
}

#[test]
fn a_worker_and_an_architect_are_refused_before_the_kernel() {
    let w = World::new("", false);
    w.approved();
    w.end("arch-1");
    for who in ["dev-1", "dev-2", "arch-2", "rev-1"] {
        let refused = w.refused(who, "plan", &["reassign", "plan-1", "arch-2"]);
        assert!(
            refused.contains("Forbidden") || refused.contains("forbidden"),
            "{who}: {refused}"
        );
        assert!(
            refused.contains("not_manager: only the PM or the operator may reassign a plan"),
            "{who}: {refused}"
        );
    }
    let shown = w.ok("pm-1", "plan", &["show", "plan-1"]);
    assert_eq!(shown["plan"]["architectAgentId"], "arch-1");
}

#[test]
fn bad_arguments_give_the_usage_error() {
    let w = World::new("", false);
    w.approved();
    for args in [
        vec!["reassign"],
        vec!["reassign", "plan-1"],
        vec!["reassign", "plan-1", "arch-2", "extra"],
        vec!["reassign", "plan-1", "arch-2", "--reason"],
        vec!["reassign", "plan-1", "arch-2", "--why", "x"],
        vec!["reassign", "plan-1", "../x"],
        vec!["reassign", "plan 1", "arch-2"],
    ] {
        let refused = w.refused("pm-1", "plan", &args);
        assert!(refused.contains(USAGE), "{args:?}: {refused}");
        assert!(refused.contains("[--reason"), "{args:?}: {refused}");
    }
}

#[test]
fn a_kernel_refusal_surfaces_as_plan_refused() {
    let w = World::new("", false);
    w.approved();
    let refused = w.refused("pm-1", "plan", &["reassign", "plan-1", "arch-2"]);
    assert!(refused.contains("plan_refused: owner_active"), "{refused}");
    w.end("arch-1");
    let refused = w.refused("pm-1", "plan", &["reassign", "plan-1", "dev-2"]);
    assert!(refused.contains("plan_refused: not_architect"), "{refused}");
    let refused = w.refused("pm-1", "plan", &["reassign", "plan-9", "arch-2"]);
    assert!(
        refused.contains("plan_refused: plan_not_found"),
        "{refused}"
    );
}

#[test]
fn a_plan_in_review_carries_on_under_the_new_owner() {
    let w = World::new("", false);
    w.ok("pm-1", "plan", &["open", "high-risk", "Work"]);
    let body = json!({"summary": "one", "packages": [World::package("api")]});
    w.script_spawn("rev-1");
    let submitted = w.ok("arch-1", "plan", &["submit", "plan-1", &body.to_string()]);
    assert_eq!(submitted["state"], "in_review", "{submitted}");
    w.end("arch-1");
    let done = w.ok("pm-1", "plan", &["reassign", "plan-1", "arch-2"]);
    assert_eq!(done["state"], "in_review");

    w.drain("rev-1");
    w.ok(
        "rev-1",
        "review",
        &[
            "findings",
            "## api\n- [api] major owns: api also writes docs/api\n- [api] minor depends_on: none",
        ],
    );
    let verdict = w.notices("arch-2", "api also writes docs/api");
    assert!(!verdict.is_empty(), "the verdict did not reach arch-2");
    let amendment = r#"{"reason":"docs too","packages":[{"id":"api","add_owns":["docs/api"]}]}"#;
    w.script_spawn("rev-2");
    let amended = w.ok("arch-2", "plan", &["amend", "plan-1", amendment]);
    assert_eq!(amended["state"], "in_review", "{amended}");
    assert_eq!(amended["revision"], 2, "{amended}");
}

#[test]
fn an_approved_plan_is_carried_to_signoff_by_the_new_owner() {
    let w = World::new("", false);
    w.approved();
    w.drain("dev-1");
    w.end("arch-1");
    let done = w.ok("pm-1", "plan", &["reassign", "plan-1", "arch-2"]);
    assert_eq!(done["notifiedDevelopers"], json!(["dev-1"]));
    let told = w.notices("dev-1", "arch-2");
    assert!(!told.is_empty(), "dev-1 was not told");

    let (report, _) = w.report("dev-1");
    w.script_spawn("rev-1");
    w.ok("arch-2", "request-review", &[&report]);
    w.ok("rev-1", "review", &["pass", "fine"]);
    let merged = w.ok("arch-2", "integrate", &[&report]);
    let integration = merged["integrationId"].as_str().unwrap().to_string();
    w.script_spawn("rev-2");
    w.ok("arch-2", "request-review", &[&integration]);
    w.ok("rev-2", "review", &["pass", "all in"]);
    let signed = w.ok(
        "arch-2",
        "plan",
        &["signoff", "plan-1", &integration, "all in"],
    );
    assert_eq!(signed["planId"], "plan-1", "{signed}");
}
