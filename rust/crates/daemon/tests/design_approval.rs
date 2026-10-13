//! The design approval gate through the command handlers: `design approve|changes|show`, who may run them, what they refuse,
//! and the `plan assign` refusal for a package that depends on a design package. The launcher is the recording stub; the
//! kernel, the ledger and git are real.

mod common;

use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::handlers::handle_frame;
use capstan_daemon::handlers::shared::AbortSignal;
use capstan_daemon::handlers::MAX_RESPONSE_BYTES;
use capstan_daemon::ports::ProjectGit;
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::Core;
use capstan_launcher::api::{LauncherService, StubLauncher};
use common::project::{build_repo, write_project, Identity};
use common::{private_tempdir, seeded_env};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
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
"#;

const AGENTS: [(&str, &str, &str); 5] = [
    ("pm-1", "pm", "PM"),
    ("arch-1", "architect", "Developer"),
    ("dev-1", "developer", "Developer"),
    ("dev-2", "developer", "Developer"),
    ("dev-3", "developer", "Developer"),
];

static WORLDS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

struct World {
    dir: common::PrivateDir,
    deps: Deps,
    credentials: HashMap<String, String>,
    heads: BTreeMap<String, String>,
}

impl World {
    /// `more` is appended to capstan.toml.
    fn new(more: &str) -> World {
        let dir = private_tempdir();
        let identity = Identity {
            project_id: "pdesign".into(),
            name: "Design".into(),
            credential: format!("operator-{}", "w".repeat(40)),
        };
        let state = write_project(dir.path(), &identity);
        let mut config = CONFIG.to_string();
        config.push_str(more);
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
                        "design-{}-{}",
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
        options.capstan = Some(Arc::new(role_config));
        let launcher = Arc::new(StubLauncher::new());
        let mut deps = Deps::new(kernel, options, sink);
        deps.git = Arc::new(ProjectGit::new(dir.path()));
        deps.launcher = Some(launcher.clone() as Arc<dyn LauncherService>);
        capstan_daemon::deps::startup_sequence(&deps);
        let mut world = World {
            dir,
            deps,
            credentials: HashMap::new(),
            heads,
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

    fn populate(&mut self) {
        let owner = self.credentials["operator"].clone();
        let roles: Vec<Value> = [
            ("pm", "PM"),
            ("developer", "Developer"),
            ("architect", "Developer"),
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

    /// An approved plan `plan-1`: `ui` (a design package, held by dev-1), `api` (depends on `ui`) and `docs`.
    fn plan(&self) {
        self.ok("pm-1", "plan", &["open", "normal", "Work"]);
        let package = |id: &str| json!({"id": id, "title": id, "owns": [format!("src/{id}/")], "acceptance": [format!("{id} works")], "estimate_hours": 2});
        let mut ui = package("ui");
        ui["design"] = json!(true);
        let mut api = package("api");
        api["depends_on"] = json!(["ui"]);
        let body = json!({"summary": "split", "packages": [ui, api, package("docs")]});
        self.ok("arch-1", "plan", &["submit", "plan-1", &body.to_string()]);
        self.ok("pm-1", "plan", &["assign", "plan-1", "ui", "dev-1"]);
    }

    /// A report of `agent` at its branch tip.
    fn report(&self, agent: &str) -> String {
        self.drain(agent);
        let sha = self.heads[&format!("feat/{agent}")].clone();
        let report = self.ok(agent, "report", &[&sha, "Done"]);
        report["reportId"].as_str().unwrap().to_string()
    }

    /// A new commit on the agent's branch (same tree), reported again.
    fn rework(&mut self, agent: &str) -> String {
        let branch = format!("feat/{agent}");
        let root = self.dir.path();
        let tip = self.heads[&branch].clone();
        let tree = git(root, &["rev-parse", &format!("{tip}^{{tree}}")]);
        let sha = git(
            root,
            &["commit-tree", &tree, "-p", &tip, "-m", "fix: rework"],
        );
        git(root, &["update-ref", &format!("refs/heads/{branch}"), &sha]);
        self.heads.insert(branch, sha);
        self.report(agent)
    }

    fn approve(&self) -> Value {
        self.ok(
            "pm-1",
            "design",
            &[
                "approve",
                "plan-1/ui",
                "--link",
                "https://design/1",
                "--asked-user",
                "looks good",
            ],
        )
    }
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
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
    assert!(output.status.success(), "git {args:?}");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

const MISSING: &str = "design_approval_missing: package api depends on design package ui";

#[test]
fn a_dependent_waits_for_the_decision_and_early_does_not_bypass() {
    let w = World::new("");
    w.plan();
    w.report("dev-1");
    // The design is reported but not reviewed, so the dependency itself is unmet unless the PM goes early; early must not
    // get past the approval.
    let refused = w.refused("pm-1", "plan", &["assign", "plan-1", "api", "dev-2"]);
    assert!(refused.contains("dependencies_unmet"), "{refused}");
    let refused = w.refused(
        "pm-1",
        "plan",
        &["assign", "plan-1", "api", "dev-2", "--early", "soon"],
    );
    assert!(
        refused.contains(MISSING) && refused.contains("no decision"),
        "{refused}"
    );
    assert!(
        refused.contains("cstan design approve plan-1/ui"),
        "{refused}"
    );
    let approved = w.approve();
    assert_eq!(approved["decision"], "approved");
    w.ok(
        "pm-1",
        "plan",
        &["assign", "plan-1", "api", "dev-2", "--early", "soon"],
    );
    let shown = w.ok("pm-1", "plan", &["show", "plan-1"]);
    let ui = shown["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["packageId"] == "ui")
        .unwrap()
        .clone();
    assert_eq!(ui["design"], true);
    assert_eq!(ui["designDecision"]["decision"], "approved");
    let decisions = w.ok("pm-1", "design", &["show", "plan-1"]);
    assert_eq!(decisions["packages"][0]["gate"], "approved");
}

#[test]
fn changes_tell_the_designer_and_an_old_approval_does_not_carry_over() {
    let mut w = World::new("");
    w.plan();
    w.report("dev-1");
    let changes = w.ok(
        "pm-1",
        "design",
        &[
            "changes",
            "plan-1/ui",
            "--asked-user",
            "make the header smaller",
        ],
    );
    assert!(changes["messageId"].is_string());
    let bodies = w.drain("dev-1");
    assert!(
        bodies
            .iter()
            .any(|b| b.contains("make the header smaller") && b.contains("plan-1")),
        "{bodies:?}"
    );
    let refused = w.refused(
        "pm-1",
        "plan",
        &["assign", "plan-1", "api", "dev-2", "--early", "x"],
    );
    assert!(refused.contains("changes requested"), "{refused}");
    w.rework("dev-1");
    w.approve();
    w.rework("dev-1");
    let refused = w.refused(
        "pm-1",
        "plan",
        &["assign", "plan-1", "api", "dev-2", "--early", "x"],
    );
    assert!(
        refused.contains("approval is for an earlier report"),
        "{refused}"
    );
    w.approve();
    w.ok(
        "pm-1",
        "plan",
        &["assign", "plan-1", "api", "dev-2", "--early", "x"],
    );
}

#[test]
fn without_the_requirement_assignment_is_unchanged() {
    let w = World::new("\n[design]\nrequire_approval = false\n");
    w.plan();
    w.report("dev-1");
    w.ok(
        "pm-1",
        "plan",
        &["assign", "plan-1", "api", "dev-2", "--early", "x"],
    );
}

#[test]
fn a_plan_without_design_packages_assigns_as_before() {
    let w = World::new("");
    w.ok("pm-1", "plan", &["open", "normal", "Work"]);
    let package = |id: &str| json!({"id": id, "title": id, "owns": [format!("src/{id}/")], "acceptance": ["x"], "estimate_hours": 2});
    let body = json!({"summary": "split", "packages": [package("a"), package("b")]});
    w.ok("arch-1", "plan", &["submit", "plan-1", &body.to_string()]);
    w.ok("pm-1", "plan", &["assign", "plan-1", "a", "dev-1"]);
    w.ok("pm-1", "plan", &["assign", "plan-1", "b", "dev-2"]);
}

#[test]
fn only_the_pm_or_operator_decides_and_bad_requests_are_refused() {
    let w = World::new("");
    w.plan();
    let ok_args = ["approve", "plan-1/ui", "--link", "l", "--asked-user", "yes"];
    let refused = w.refused("pm-1", "design", &ok_args);
    assert!(refused.contains("no report yet"), "{refused}");
    w.report("dev-1");
    for who in ["dev-2", "arch-1", "dev-1"] {
        let refused = w.refused(who, "design", &ok_args);
        assert!(
            refused.contains("only the PM or the operator"),
            "{who}: {refused}"
        );
    }
    let refused = w.refused("pm-1", "design", &["approve", "plan-1/ui", "--link", "l"]);
    assert!(refused.contains("--asked-user"), "{refused}");
    let refused = w.refused(
        "pm-1",
        "design",
        &["approve", "plan-1/ui", "--asked-user", "yes"],
    );
    assert!(refused.contains("--link"), "{refused}");
    let refused = w.refused(
        "pm-1",
        "design",
        &["changes", "plan-1/ui", "--asked-user", "  "],
    );
    assert!(refused.contains("--asked-user"), "{refused}");
    let refused = w.refused(
        "pm-1",
        "design",
        &[
            "approve",
            "plan-1/docs",
            "--link",
            "l",
            "--asked-user",
            "yes",
        ],
    );
    assert!(refused.contains("not a design package"), "{refused}");
    w.ok("operator", "design", &ok_args);
    w.ok("operator", "plan", &["cancel", "plan-1", "ui"]);
    let refused = w.refused("pm-1", "design", &ok_args);
    assert!(refused.contains("cancelled"), "{refused}");
}
