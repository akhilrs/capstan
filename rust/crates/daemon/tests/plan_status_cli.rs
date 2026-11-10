//! `plan show` filters, `plan archive` with its preview and confirmation, and `plan unarchive` through the command handlers.
//! The kernel clock is a test clock that moves, so a plan can cross the idle threshold between the preview and the confirm.

mod common;

use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::handlers::handle_frame;
use capstan_daemon::handlers::shared::AbortSignal;
use capstan_daemon::handlers::MAX_RESPONSE_BYTES;
use capstan_daemon::ports::ProjectGit;
use capstan_daemon::verify::VerifyService;
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::Core;
use capstan_launcher::api::{LauncherService, StubLauncher};
use common::private_tempdir;
use common::project::{build_repo, write_project, Identity};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicI64, Ordering};
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

/// A start far from zero, so no time is a default.
const BASE: i64 = 1_800_000_000_000;
const DAY: i64 = 86_400_000;

/// The kernel clock is the test clock.
struct TestEnv {
    clock: Arc<AtomicI64>,
    inner: capstan_kernel::SeededEnv,
}

impl capstan_kernel::Env for TestEnv {
    fn now(&self) -> i64 {
        self.clock.load(Ordering::SeqCst)
    }
    fn uuid(&self) -> String {
        self.inner.uuid()
    }
    fn random_bytes(&self, n: usize) -> Vec<u8> {
        self.inner.random_bytes(n)
    }
}

static WORLDS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

struct World {
    _dir: common::PrivateDir,
    _scratch: common::PrivateDir,
    deps: Deps,
    credentials: HashMap<String, String>,
    heads: BTreeMap<String, String>,
    clock: Arc<AtomicI64>,
}

impl World {
    /// `more` is appended to capstan.toml; `gate` adds a `[verify]` section whose commands pass.
    fn new(more: &str, gate: bool) -> World {
        World::build(more, gate, None)
    }

    /// `days` sets `[architect] stale_plan_days`.
    fn build(more: &str, gate: bool, days: Option<i64>) -> World {
        let dir = private_tempdir();
        let scratch = private_tempdir();
        let identity = Identity {
            project_id: "planstatus".into(),
            name: "Status".into(),
            credential: format!("operator-{}", "w".repeat(40)),
        };
        let state = write_project(dir.path(), &identity);
        let mut config = CONFIG.to_string();
        if let Some(days) = days {
            config = config.replace(
                "[architect]\nenabled = true\n",
                &format!("[architect]\nenabled = true\nstale_plan_days = {days}\n"),
            );
        }
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
        let clock = Arc::new(AtomicI64::new(BASE));
        let kernel = {
            let clock = clock.clone();
            let (state, project, root) = (state.clone(), project.clone(), dir.path().to_path_buf());
            KernelHandle::spawn(move || {
                Core::open(
                    &state,
                    &project,
                    &KernelOptions {
                        workspace_root: Some(root),
                        ..KernelOptions::default()
                    },
                    Box::new(TestEnv {
                        clock,
                        inner: capstan_kernel::SeededEnv::new(&format!(
                            "status-{}-{}",
                            std::process::id(),
                            WORLDS.fetch_add(1, Ordering::SeqCst)
                        )),
                    }),
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
            clock,
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

    fn package(id: &str) -> Value {
        json!({"id": id, "title": id, "owns": [format!("src/{id}/")], "acceptance": [format!("{id} works")], "estimate_hours": 2})
    }
}

fn ids(result: &Value) -> Vec<String> {
    result["plans"]
        .as_array()
        .expect("plans")
        .iter()
        .map(|p| p["planId"].as_str().unwrap().to_string())
        .collect()
}

impl World {
    fn open(&self, title: &str) -> String {
        let opened = self.ok("pm-1", "plan", &["open", "normal", title]);
        opened["planId"].as_str().unwrap().to_string()
    }

    /// Submits one package; a normal plan is approved by it.
    fn approve(&self, plan: &str, package: &str) {
        let body = json!({"summary": "one", "packages": [World::package(package)]});
        self.ok("arch-1", "plan", &["submit", plan, &body.to_string()]);
    }

    fn days(&self, days: i64) {
        self.clock.fetch_add(days * DAY, Ordering::SeqCst);
    }

    fn hours(&self, hours: i64) {
        self.clock.fetch_add(hours * DAY / 24, Ordering::SeqCst);
    }

    fn list(&self, args: &[&str]) -> Vec<String> {
        let mut all = vec!["show"];
        all.extend(args);
        ids(&self.ok("operator", "plan", &all))
    }

    /// plan-1 draft, plan-2 approved, plan-3 cancelled, plan-4 archived, plan-5 draft.
    fn mixed(&self) {
        for title in ["One", "Two", "Three", "Four", "Five"] {
            self.open(title);
        }
        self.approve("plan-2", "api");
        self.ok("operator", "plan", &["cancel", "plan-3"]);
        let preview = self.ok("operator", "plan", &["archive", "plan-4"]);
        self.ok(
            "operator",
            "plan",
            &[
                "archive",
                "plan-4",
                "--confirm",
                preview["token"].as_str().unwrap(),
            ],
        );
    }
}

#[test]
fn each_status_filter_lists_its_plans_and_two_make_a_union() {
    let w = World::new("", false);
    w.mixed();
    assert_eq!(w.list(&["--draft"]), ["plan-1", "plan-5"]);
    assert_eq!(w.list(&["--approved"]), ["plan-2"]);
    assert_eq!(w.list(&["--cancelled"]), ["plan-3"]);
    assert_eq!(w.list(&["--archived"]), ["plan-4"]);
    for none in ["--in-review", "--signed-off", "--superseded"] {
        assert!(w.list(&[none]).is_empty(), "{none}");
    }
    assert_eq!(
        w.list(&["--draft", "--cancelled"]),
        ["plan-1", "plan-3", "plan-5"]
    );
    assert_eq!(
        w.list(&["--all"]),
        ["plan-1", "plan-2", "plan-3", "plan-4", "plan-5"]
    );
    // A filtered row carries its status; nothing is stale yet.
    let rows = w.ok("operator", "plan", &["show", "--approved"]);
    assert_eq!(rows["plans"][0]["status"], "approved");
    assert!(rows["plans"][0].get("stale").is_none());
}

#[test]
fn the_default_list_leaves_archived_plans_out_and_keeps_its_shape() {
    let w = World::new("", false);
    w.mixed();
    let shown = w.ok("operator", "plan", &["show"]);
    assert_eq!(ids(&shown), ["plan-1", "plan-2", "plan-3", "plan-5"]);
    let row = &shown["plans"][0];
    assert_eq!(
        row.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["planId", "tier", "state", "title"]
    );
    let brief = w.ok("operator", "plan", &["show", "--brief"]);
    assert_eq!(ids(&brief), ["plan-1", "plan-2", "plan-3", "plan-5"]);
    assert!(brief["plans"][0].get("status").is_none());
}

#[test]
fn the_bare_form_is_plan_show() {
    let w = World::new("", false);
    w.mixed();
    w.days(20);
    assert_eq!(
        w.ok("operator", "plan", &[]),
        w.ok("operator", "plan", &["show"])
    );
    assert_eq!(
        w.ok("operator", "plan", &["--brief"]),
        w.ok("operator", "plan", &["show", "--brief"])
    );
    assert_eq!(
        w.ok("operator", "plan", &["--stale", "--older-than", "5"]),
        w.ok(
            "operator",
            "plan",
            &["show", "--stale", "--older-than", "5"]
        )
    );
    assert_eq!(
        w.ok("operator", "plan", &["--draft", "--brief"]),
        w.ok("operator", "plan", &["show", "--draft", "--brief"])
    );
}

#[test]
fn stale_plans_show_why_and_older_than_overrides_the_config() {
    let w = World::new("", false);
    w.mixed();
    assert!(w.list(&["--stale"]).is_empty());
    w.days(10);
    // The default is 14 days: not yet idle.
    assert!(w.list(&["--stale"]).is_empty());
    assert_eq!(
        w.list(&["--stale", "--older-than", "5"]),
        ["plan-1", "plan-2", "plan-5"]
    );
    w.days(5);
    let stale = w.ok("operator", "plan", &["show", "--stale"]);
    assert_eq!(ids(&stale), ["plan-1", "plan-2", "plan-5"]);
    assert_eq!(stale["plans"][0]["stale"], json!(["idle"]));
    assert_eq!(stale["plans"][0]["status"], "draft");
    // --stale narrows the status filters.
    assert_eq!(w.list(&["--approved", "--stale"]), ["plan-2"]);
    assert_eq!(
        w.list(&["--all", "--stale"]),
        ["plan-1", "plan-2", "plan-5"]
    );
    // The brief line shows the status and ends with the reasons.
    let brief = w.ok("operator", "plan", &["--stale", "--brief"]);
    assert_eq!(brief["plans"][1]["status"], "approved");
    assert_eq!(brief["plans"][1]["stale"], json!(["idle"]));
}

#[test]
fn the_config_sets_the_idle_days() {
    let w = World::build("", false, Some(3));
    w.open("One");
    w.days(2);
    assert!(w.list(&["--stale"]).is_empty());
    w.days(2);
    assert_eq!(w.list(&["--stale"]), ["plan-1"]);
    // --older-than wins over the config either way.
    assert!(w.list(&["--stale", "--older-than", "10"]).is_empty());
    let w = World::build("", false, Some(0));
    w.open("One");
    w.days(400);
    assert!(
        w.list(&["--stale"]).is_empty(),
        "0 turns the idle reason off"
    );
}

#[test]
fn bad_filters_are_refused_with_their_reasons() {
    let w = World::new("", false);
    w.mixed();
    let refused = w.refused("operator", "plan", &["show", "plan-1", "--stale"]);
    assert!(
        refused.contains("status filters list plans; plan show <plan-id> shows one"),
        "{refused}"
    );
    let refused = w.refused("operator", "plan", &["show", "plan-1", "--draft"]);
    assert!(refused.contains("status filters list plans"), "{refused}");
    for args in [
        vec!["show", "--delete"],
        vec!["--delete"],
        vec!["archive", "plan-1", "--delete"],
        vec!["show", "plan-1", "--delete"],
    ] {
        let refused = w.refused("operator", "plan", &args);
        assert!(
            refused
                .contains("plans are never deleted; --archive hides them and keeps their history"),
            "{args:?}: {refused}"
        );
        assert!(refused.contains("invalid_request"), "{refused}");
    }
    let refused = w.refused("operator", "plan", &["--foo"]);
    assert!(
        refused.contains("unknown filter --foo; filters: --draft --in-review --approved --signed-off --cancelled --superseded --archived --stale --all --older-than --brief --archive"),
        "{refused}"
    );
    let refused = w.refused("operator", "plan", &["show", "--reason", "x"]);
    assert!(refused.contains("--reason needs --archive"), "{refused}");
    let refused = w.refused("operator", "plan", &["show", "--older-than", "3"]);
    assert!(refused.contains("--older-than needs --stale"), "{refused}");
    for bad in ["-1", "3651", "x"] {
        let refused = w.refused("operator", "plan", &["--stale", "--older-than", bad]);
        assert!(
            refused.contains("--older-than must be an integer from 0 to 3650"),
            "{refused}"
        );
    }
    let refused = w.refused("operator", "plan", &["--stale", "--yes"]);
    assert!(refused.contains("--yes"), "{refused}");
    let refused = w.refused("operator", "plan", &["archive", "--stale", "--yes"]);
    assert!(refused.contains("--yes"), "{refused}");
    let refused = w.refused("operator", "plan", &["show", "--confirm", "abc"]);
    assert!(refused.contains("--confirm needs --archive"), "{refused}");
    let refused = w.refused("pm-1", "plan", &["bogus"]);
    assert!(
        refused.contains("open, submit, amend, show, model, assign, reassign, signoff, cancel, landed, archive or unarchive"),
        "{refused}"
    );
}

#[test]
fn only_the_operator_archives_and_unarchives() {
    let w = World::new("", false);
    w.mixed();
    for who in ["pm-1", "arch-1", "dev-1"] {
        for args in [
            vec!["archive", "plan-1"],
            vec!["show", "--draft", "--archive"],
            vec!["unarchive", "plan-4"],
        ] {
            let refused = w.refused(who, "plan", &args);
            assert!(
                refused.to_lowercase().contains("forbidden"),
                "{who}: {refused}"
            );
            assert!(refused.contains("only the operator may"), "{refused}");
        }
    }
}

#[test]
fn a_preview_writes_nothing_and_a_confirm_archives_exactly_the_shown_plans() {
    let w = World::new("", false);
    w.mixed();
    let before = w.ok("operator", "plan", &["show", "--all"]);
    let preview = w.ok("operator", "plan", &["archive", "--draft"]);
    assert_eq!(preview["preview"], true);
    assert_eq!(ids(&preview), ["plan-1", "plan-5"]);
    assert_eq!(preview["plans"][0]["status"], "draft");
    assert_eq!(preview["plans"][0]["title"], "One");
    let token = preview["token"].as_str().unwrap().to_string();
    assert_eq!(token.len(), 16);
    // The same selection by the show form gives the same preview, and nothing is written.
    let again = w.ok("operator", "plan", &["show", "--draft", "--archive"]);
    assert_eq!(again["token"], token.as_str());
    assert_eq!(w.ok("operator", "plan", &["show", "--all"]), before);
    // The token is the hash of the sorted entries, a client may compute it.
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(b"plan-1:draft:,plan-5:draft:");
    let expected: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    assert_eq!(token, expected);
    let done = w.ok(
        "operator",
        "plan",
        &["archive", "--draft", "--confirm", &token],
    );
    assert_eq!(done["archived"], json!(["plan-1", "plan-5"]));
    assert_eq!(w.list(&["--archived"]), ["plan-1", "plan-4", "plan-5"]);
    let notices = w.drain("pm-1").join("\n");
    assert!(
        notices.contains("Plans archived by the operator (2): plan-1, plan-5"),
        "{notices}"
    );
    assert!(
        notices.contains("archived by the operator: --draft"),
        "{notices}"
    );
    // Ids work the same way, with the reason given.
    let preview = w.ok(
        "operator",
        "plan",
        &["archive", "plan-2", "--reason", "done"],
    );
    let token = preview["token"].as_str().unwrap();
    w.ok(
        "operator",
        "plan",
        &["archive", "plan-2", "--reason", "done", "--confirm", token],
    );
    assert!(w.drain("pm-1").join("\n").contains("Reason: done"));
    // Naming an archived or unknown plan, or mixing ids and filters, is refused.
    let refused = w.refused("operator", "plan", &["archive", "plan-2"]);
    assert!(refused.contains("already_archived"), "{refused}");
    let refused = w.refused("operator", "plan", &["archive", "plan-404"]);
    assert!(refused.contains("unknown_plan"), "{refused}");
    let refused = w.refused("operator", "plan", &["archive", "plan-3", "--draft"]);
    assert!(refused.contains("ids or filters, not both"), "{refused}");
    let refused = w.refused("operator", "plan", &["archive"]);
    assert!(refused.contains("needs plan ids or filters"), "{refused}");
    let refused = w.refused(
        "operator",
        "plan",
        &["archive", "--draft", "--confirm", "x"],
    );
    assert!(refused.contains("selection_changed"), "{refused}");
    // An empty selection answers a note, and its preview an empty list.
    let empty = w.ok("operator", "plan", &["archive", "--in-review"]);
    assert_eq!(empty["plans"], json!([]));
    let done = w.ok(
        "operator",
        "plan",
        &[
            "archive",
            "--in-review",
            "--confirm",
            empty["token"].as_str().unwrap(),
        ],
    );
    assert_eq!(done, json!({"archived": [], "note": "no plans match"}));
}

#[test]
fn unarchive_restores_a_plan() {
    let w = World::new("", false);
    w.mixed();
    let restored = w.ok("operator", "plan", &["unarchive", "plan-4"]);
    assert_eq!(restored["planId"], "plan-4");
    assert_eq!(w.list(&["--archived"]), Vec::<String>::new());
    assert!(w.list(&[]).contains(&"plan-4".to_string()));
    let refused = w.refused("operator", "plan", &["unarchive", "plan-4"]);
    assert!(refused.contains("not_archived"), "{refused}");
    let refused = w.refused("operator", "plan", &["unarchive"]);
    assert!(refused.contains("needs one plan id"), "{refused}");
}

#[test]
fn more_than_500_matches_are_refused_at_the_preview() {
    let w = World::new("", false);
    for n in 0..501 {
        w.open(&format!("Plan {n}"));
    }
    let refused = w.refused("operator", "plan", &["archive", "--draft"]);
    assert!(
        refused.contains("too_many: 501 plans match; archive at most 500 per call (narrow the filters or name ids)"),
        "{refused}"
    );
}

#[test]
fn a_changed_selection_is_refused_at_the_confirm() {
    // An id was added.
    let w = World::new("", false);
    w.open("One");
    let preview = w.ok("operator", "plan", &["archive", "--draft"]);
    w.open("Two");
    let refused = w.refused(
        "operator",
        "plan",
        &[
            "archive",
            "--draft",
            "--confirm",
            preview["token"].as_str().unwrap(),
        ],
    );
    assert!(refused.contains("selection_changed: the plans to archive changed since the list was shown; run the command again"), "{refused}");
    assert!(w.list(&["--archived"]).is_empty());

    // A status changed.
    let w = World::new("", false);
    w.open("One");
    w.open("Two");
    let preview = w.ok("operator", "plan", &["archive", "--draft"]);
    w.ok("operator", "plan", &["cancel", "plan-2"]);
    let refused = w.refused(
        "operator",
        "plan",
        &[
            "archive",
            "plan-1",
            "plan-2",
            "--confirm",
            preview["token"].as_str().unwrap(),
        ],
    );
    assert!(refused.contains("selection_changed"), "{refused}");
    assert!(w.list(&["--archived"]).is_empty());

    // A plan left stale: new activity on it.
    let w = World::new("", false);
    w.open("One");
    w.open("Two");
    w.days(20);
    let preview = w.ok("operator", "plan", &["archive", "--stale"]);
    assert_eq!(ids(&preview), ["plan-1", "plan-2"]);
    w.approve("plan-2", "api");
    let refused = w.refused(
        "operator",
        "plan",
        &[
            "archive",
            "--stale",
            "--confirm",
            preview["token"].as_str().unwrap(),
        ],
    );
    assert!(refused.contains("selection_changed"), "{refused}");
    assert!(w.list(&["--archived"]).is_empty());
}

#[test]
fn a_plan_crossing_the_idle_threshold_between_preview_and_confirm_is_a_changed_selection() {
    let w = World::new("", false);
    w.open("One");
    w.days(14);
    // Exactly at the threshold: not yet idle.
    let preview = w.ok("operator", "plan", &["archive", "--stale"]);
    assert_eq!(ids(&preview), Vec::<String>::new());
    w.hours(1);
    let refused = w.refused(
        "operator",
        "plan",
        &[
            "archive",
            "--stale",
            "--confirm",
            preview["token"].as_str().unwrap(),
        ],
    );
    assert!(refused.contains("selection_changed"), "{refused}");
    // Run the command again: the plan is listed and archived.
    let preview = w.ok("operator", "plan", &["archive", "--stale"]);
    assert_eq!(ids(&preview), ["plan-1"]);
    assert_eq!(preview["plans"][0]["stale"], json!(["idle"]));
    let done = w.ok(
        "operator",
        "plan",
        &[
            "archive",
            "--stale",
            "--confirm",
            preview["token"].as_str().unwrap(),
        ],
    );
    assert_eq!(done["archived"], json!(["plan-1"]));
}

#[test]
fn a_plan_made_busy_between_preview_and_confirm_is_refused_and_nothing_is_archived() {
    let w = World::new("", false);
    w.open("One");
    w.open("Two");
    w.approve("plan-1", "api");
    w.approve("plan-2", "web");
    let preview = w.ok("operator", "plan", &["archive", "--approved"]);
    assert_eq!(ids(&preview), ["plan-1", "plan-2"]);
    // A package assigned: the status is the same, so the token still matches, and the archive itself refuses.
    w.ok("pm-1", "plan", &["assign", "plan-2", "web", "dev-1"]);
    let refused = w.refused(
        "operator",
        "plan",
        &[
            "archive",
            "--approved",
            "--confirm",
            preview["token"].as_str().unwrap(),
        ],
    );
    assert!(refused.contains("plan_busy"), "{refused}");
    assert!(refused.contains("plan-2"), "{refused}");
    assert!(w.list(&["--archived"]).is_empty());
}

#[test]
fn a_changed_stale_reason_is_a_changed_selection() {
    let w = World::new("", false);
    w.open("One");
    w.approve("plan-1", "api");
    w.days(20);
    let preview = w.ok("operator", "plan", &["archive", "--stale"]);
    assert_eq!(preview["plans"][0]["status"], "approved");
    assert_eq!(preview["plans"][0]["stale"], json!(["idle"]));
    // Cancelling the only package is activity (idle goes) and finishes the plan's work (done_unsigned arrives):
    // the status is the same, the reasons are not.
    w.ok("operator", "plan", &["cancel", "plan-1", "api"]);
    let now = w.ok("operator", "plan", &["show", "--stale"]);
    assert_eq!(now["plans"][0]["status"], "approved");
    assert_eq!(now["plans"][0]["stale"], json!(["done_unsigned"]));
    let refused = w.refused(
        "operator",
        "plan",
        &[
            "archive",
            "--stale",
            "--confirm",
            preview["token"].as_str().unwrap(),
        ],
    );
    assert!(refused.contains("selection_changed"), "{refused}");
    assert!(w.list(&["--archived"]).is_empty());
}
