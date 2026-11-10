//! The knowledge base warnings at plan open and spawn and the Reuse section of a package assignment, through the command
//! handlers on a real temporary git repository. The launcher is the recording stub; the kernel and the ledger are real.

#![allow(dead_code)]

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
        World::with_files(more, gate, json!({"a.txt": "a\n"}))
    }

    fn with_files(more: &str, gate: bool, files: Value) -> World {
        let dir = private_tempdir();
        let scratch = private_tempdir();
        let identity = Identity {
            project_id: "pkbwarn".into(),
            name: "KbWarn".into(),
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
                "files": files,
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
                        "kbwarn-{}-{}",
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

const KB: &str = "\n[kb]\n";

impl World {
    fn root(&self) -> &std::path::Path {
        self._dir.path()
    }

    fn git(&self, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .arg("-C")
            .arg(self.root())
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn commit_file(&self, rel: &str, text: &str, message: &str) -> String {
        let path = self.root().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
        self.git(&["add", "--all"]);
        self.git(&["commit", "--quiet", "-m", message]);
        self.git(&["rev-parse", "HEAD"])
    }

    /// Commits a knowledge base with one helper entry for `path`, verified at the commit before it.
    fn commit_kb(&self, path: &str) -> String {
        let verified = self.git(&["rev-parse", "HEAD"]);
        let entry = capstan_config::KbEntry {
            id: "helper".into(),
            kind: capstan_config::KbKind::Helper,
            title: "A helper".into(),
            body: "does a thing".into(),
            path: Some(path.into()),
            area: None,
            tags: vec![],
            aliases: vec![],
            related: vec![],
            verified_commit: verified,
        };
        let line = capstan_config::kb::to_line(&entry);
        self.commit_file("docs/capstan/kb.jsonl", &format!("{line}\n"), "docs: kb")
    }

    fn plan(&self, packages: Vec<Value>) {
        self.ok("pm-1", "plan", &["open", "normal", "Work"]);
        let body = json!({"summary": "one", "packages": packages});
        self.ok("arch-1", "plan", &["submit", "plan-1", &body.to_string()]);
    }

    fn pkg(id: &str, owns: &str, design: bool) -> Value {
        let mut p = json!({"id": id, "title": id, "owns": [owns], "acceptance": ["works"], "estimate_hours": 1});
        if design {
            p["design"] = json!(true);
        }
        p
    }

    fn open_warning(&self) -> Option<String> {
        let result = self.ok("pm-1", "plan", &["open", "normal", "Next"]);
        result["warning"].as_str().map(str::to_string)
    }

    fn spawn_warning(&self, task: &str) -> Option<String> {
        self.script_spawn("dev-9");
        let result = self.ok("pm-1", "spawn", &["developer", "--task", task]);
        result["warning"].as_str().map(str::to_string)
    }
}

#[test]
fn plan_open_warns_about_a_missing_file_and_not_about_a_fresh_one() {
    let w = World::new(KB, false);
    let missing = w.open_warning().expect("a warning");
    assert!(
        missing.starts_with("kb: no knowledge base at docs/capstan/kb.jsonl"),
        "{missing}"
    );
    w.commit_kb("a.txt");
    assert_eq!(w.open_warning(), None);
}

#[test]
fn plan_open_warns_when_a_path_changed_after_the_entry_was_verified() {
    let w = World::new(KB, false);
    w.commit_kb("a.txt");
    assert_eq!(w.open_warning(), None);
    w.commit_file("a.txt", "changed\n", "fix: change a");
    let text = w.open_warning().expect("a warning");
    assert!(
        text.starts_with("kb: 1 stale and 0 missing entries in docs/capstan/kb.jsonl"),
        "{text}"
    );
}

#[test]
fn ui_paths_without_a_design_record_add_the_design_warning() {
    let w = World::with_files(
        "\n[kb]\nui_paths = [\"src/ui/**\"]\n",
        false,
        json!({"a.txt": "a\n", "src/ui/button.tsx": "x\n"}),
    );
    w.commit_kb("a.txt");
    let text = w.open_warning().expect("a warning");
    assert!(
        text.starts_with("design record: DESIGN.md, docs/design/rules.md missing"),
        "{text}"
    );
}

#[test]
fn spawn_warns_for_a_ui_package_on_the_record_and_not_for_another() {
    let w = World::with_files(
        "\n[kb]\nui_paths = [\"src/ui/**\"]\n",
        false,
        json!({"a.txt": "a\n", "src/ui/button.tsx": "x\n", "src/api/a.rs": "y\n"}),
    );
    w.commit_kb("a.txt");
    w.plan(vec![
        World::pkg("ui", "src/ui/", false),
        World::pkg("api", "src/api/", false),
        World::pkg("art", "docs/art/", true),
    ]);
    for task in ["plan-1/ui", "plan-1/art"] {
        let text = w.spawn_warning(task).expect(task);
        assert!(text.starts_with("design record:"), "{task}: {text}");
    }
    assert_eq!(w.spawn_warning("plan-1/api"), None);
    // A task that is not a package, and no task, give the knowledge base warnings only (here the design record is
    // checked for the project as a whole).
    let plain = w.spawn_warning("PM-5");
    assert!(plain.is_none_or(|t| t.starts_with("design record:")));
}

#[test]
fn a_spawn_warning_is_joined_with_an_existing_one() {
    let w = World::new(KB, false);
    w.launcher.script_spawn(SpawnResult {
        state: "started".into(),
        agent_id: "dev-9".into(),
        pane_id: "w1:p1".into(),
        worktree_path: "/tmp/work/dev-9".into(),
        branch: "capstan/dev-9".into(),
        placement: Placement::Tab,
        placement_note: None,
        hint: None,
        missing_env: None,
        warning: Some("the pane is slow".into()),
    });
    let result = w.ok("pm-1", "spawn", &["developer"]);
    let text = result["warning"].as_str().unwrap();
    assert!(
        text.starts_with("the pane is slow; kb: no knowledge base at docs/capstan/kb.jsonl"),
        "{text}"
    );
}

#[test]
fn without_kb_or_when_disabled_the_replies_have_no_warning() {
    for more in ["", "\n[kb]\nenabled = false\n"] {
        let w = World::new(more, false);
        let opened = w.ok("pm-1", "plan", &["open", "normal", "Work"]);
        assert!(opened.get("warning").is_none(), "{opened}");
        w.script_spawn("dev-9");
        let spawned = w.ok("pm-1", "spawn", &["developer"]);
        assert!(spawned.get("warning").is_none(), "{spawned}");
    }
}

#[test]
fn a_git_failure_gives_no_warning() {
    let w = World::new(KB, false);
    std::fs::rename(w.root().join(".git"), w.root().join("not-git")).unwrap();
    assert_eq!(w.open_warning(), None);
}

#[test]
fn assign_attaches_the_reuse_lines_of_an_owned_path_and_none_without_a_match() {
    let w = World::with_files(KB, false, json!({"a.txt": "a\n", "src/api/a.rs": "y\n"}));
    w.commit_kb("src/api/a.rs");
    w.plan(vec![
        World::pkg("api", "src/api/", false),
        World::pkg("web", "src/web/", false),
    ]);
    w.ok("pm-1", "plan", &["assign", "plan-1", "api", "dev-1"]);
    let sent = w.drain("dev-1").join("\n");
    let head = w.git(&["rev-parse", "HEAD"]);
    assert!(
        sent.contains(&format!(
            "\n\nReuse (knowledge base at {}; check these, cstan kb search",
            &head[..12]
        )),
        "{sent}"
    );
    assert!(
        sent.contains("- helper (helper) A helper: src/api/a.rs"),
        "{sent}"
    );
    w.ok("pm-1", "plan", &["assign", "plan-1", "web", "dev-2"]);
    let none = w.drain("dev-2").join("\n");
    assert!(!none.contains("Reuse (knowledge base"), "{none}");
}
