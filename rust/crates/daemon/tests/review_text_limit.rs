//! A reviewer's text through the command handlers: stored whole up to the limit, refused above it with the review left
//! started, and a plan review's findings read in full. The launcher is the recording stub; the kernel, ledger and git are real.

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
    database: std::path::PathBuf,
}

impl World {
    /// `more` is appended to capstan.toml; `gate` adds a `[verify]` section whose commands pass.
    fn new(more: &str, gate: bool) -> World {
        let dir = private_tempdir();
        let scratch = private_tempdir();
        let identity = Identity {
            project_id: "preview".into(),
            name: "Warm".into(),
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
                        "rtext-{}-{}",
                        std::process::id(),
                        WORLDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    )),
                )
            })
            .expect("the kernel opens")
        };
        let sink: Logger = Arc::new(|_: &LogEntry| {});
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
            database,
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
}

#[test]
fn an_over_long_text_is_refused_and_the_review_stays_open_for_a_shorter_answer() {
    let w = World::new("", false);
    w.plan();
    let (report, _) = w.report("dev-1");
    w.script_spawn("rev-1");
    w.ok("pm-1", "request-review", &[&report]);
    w.drain("pm-1");

    let long = "x".repeat(16001);
    let message = w.refused("rev-1", "review", &["findings", &long]);
    assert!(
        message.contains("the review text is 16001 bytes; the most is 16000. Keep each finding to one line that says what is wrong and where"),
        "{message}"
    );
    let status = w.ok("operator", "status", &[]);
    assert_eq!(status["reviews"][0]["state"], "started", "{status}");

    let answer = "y".repeat(5000);
    let done = w.ok("rev-1", "review", &["findings", &answer]);
    assert_eq!(done["state"], "findings");
    let id = done["reviewId"].as_str().unwrap();
    let notice = w.drain("pm-1").join("\n");
    assert!(
        notice.contains(&format!("full text: cstan review show {id}]")),
        "{notice}"
    );
    assert!(notice.contains("showing 3600 of 5000 bytes"), "{notice}");
}

#[test]
fn plan_findings_over_the_notice_limit_are_read_in_full() {
    let w = World::new("", false);
    w.ok("pm-1", "plan", &["open", "high-risk", "Risky"]);
    w.script_spawn("rev-1");
    let package = |id: &str| json!({"id": id, "title": id, "owns": [format!("src/{id}/")], "acceptance": [format!("{id} works")], "estimate_hours": 2});
    let body = json!({"summary": "split", "packages": [package("core"), package("ui")]});
    let submitted = w.ok("arch-1", "plan", &["submit", "plan-1", &body.to_string()]);
    assert_eq!(submitted["state"], "in_review", "{submitted}");
    // Over 4000 bytes of mechanical findings, then a design finding on the last line.
    let mut text = String::new();
    while text.len() < 4500 {
        text.push_str("- [core] major owns: src/core overlaps src/ui in the shared module\n");
    }
    text.push_str("- [plan] minor risk: the rollout order is not stated\n");
    let done = w.ok("rev-1", "review", &["findings", &text]);
    assert_eq!(done["state"], "findings");
    let class: String = rusqlite::Connection::open(&w.database)
        .unwrap()
        .query_row(
            "SELECT findings_class FROM reviews WHERE review_id = ?",
            [done["reviewId"].as_str().unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(class, "design");
}

#[test]
fn review_show_gives_the_full_text_to_allowed_callers_and_refuses_the_rest() {
    let w = World::new("", false);
    w.plan();
    let (report, sha) = w.report("dev-1");
    w.script_spawn("rev-1");
    w.ok("pm-1", "request-review", &[&report]);
    w.drain("pm-1");
    let text = format!("{}\n{}", "a".repeat(4499), "b".repeat(4500));
    assert_eq!(text.len(), 9000);
    let done = w.ok("rev-1", "review", &["findings", &text]);
    let id = done["reviewId"].as_str().unwrap().to_string();

    for who in ["pm-1", "operator", "rev-1", "dev-1", "arch-1"] {
        let shown = w.ok(who, "review-show", &[&id]);
        assert_eq!(shown["text"], text, "{who}");
        assert_eq!(shown["reviewId"], id.as_str(), "{who}");
        assert_eq!(shown["subjectId"], report.as_str(), "{who}");
        assert_eq!(shown["state"], "findings", "{who}");
        assert_eq!(shown["reviewerAgentId"], "rev-1", "{who}");
        assert_eq!(shown["commitSha"], sha.as_str(), "{who}");
    }
    // A unique prefix of the id works as everywhere else.
    let shown = w.ok("dev-1", "review-show", &[&id[..8]]);
    assert_eq!(shown["text"], text);

    let refusal = w.refused("dev-2", "review-show", &[&id]);
    assert!(
        refusal.contains("only the PM, the operator, the reviewer"),
        "{refusal}"
    );
    let missing = w.refused("pm-1", "review-show", &["rev-nope-0000"]);
    assert!(missing.contains("there is no review"), "{missing}");
}
