//! `plan landed` through the command handlers: who may record or clear a landing, what is refused before git is asked, and
//! what each answer of the git port becomes. The git port counts its calls and answers `commit_on_head` as scripted.

mod common;

use capstan_daemon::deps::{
    CommitInspection, DaemonOptions, Deps, GitPort, GitResult, InspectCommitInput, KernelHandle,
    LogEntry, Logger, NewCommitMessages, NewCommitMessagesInput,
};
use capstan_daemon::handlers::handle_frame;
use capstan_daemon::handlers::shared::AbortSignal;
use capstan_daemon::handlers::MAX_RESPONSE_BYTES;
use capstan_daemon::ports::ProjectGit;
use capstan_daemon::verify::VerifyService;
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::Core;
use capstan_launcher::api::{LauncherService, StubLauncher};
use common::project::{build_repo, write_project, Identity};
use common::{private_tempdir, seeded_env};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicUsize, Ordering};
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
    git: Arc<CountingGit>,
}

impl World {
    /// `more` is appended to capstan.toml; `gate` adds a `[verify]` section whose commands pass.
    fn new(more: &str, gate: bool) -> World {
        let dir = private_tempdir();
        let scratch = private_tempdir();
        let identity = Identity {
            project_id: "plandlanded".into(),
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
                        "landed-{}-{}",
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
        let git = Arc::new(CountingGit {
            inner: ProjectGit::new(dir.path()),
            calls: AtomicUsize::new(0),
            answer: Mutex::new(Some(Some(true))),
        });
        deps.git = git.clone();
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
            git,
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

    /// A report of `agent` at its branch tip.
    fn report(&self, agent: &str) -> (String, String) {
        self.drain(agent);
        let sha = self.heads[&format!("feat/{agent}")].clone();
        let report = self.ok(agent, "report", &[&sha, "Done"]);
        (report["reportId"].as_str().unwrap().to_string(), sha)
    }
}

/// Delegates to the real port and counts every call; `commit_on_head` answers what the test scripted
/// (`None`: the port cannot check; `Some(Err)` is not scripted here, see `answer_error`).
struct CountingGit {
    inner: ProjectGit,
    calls: AtomicUsize,
    /// `Some(x)`: `Ok(x)`; `None`: `Err`.
    answer: Mutex<Option<Option<bool>>>,
}

impl CountingGit {
    fn tick(&self) {
        self.calls.fetch_add(1, Ordering::SeqCst);
    }
}

impl GitPort for CountingGit {
    fn commit_exists(&self, sha: &str) -> GitResult<bool> {
        self.tick();
        self.inner.commit_exists(sha)
    }
    fn inspect_commit(&self, input: &InspectCommitInput) -> GitResult<CommitInspection> {
        self.tick();
        self.inner.inspect_commit(input)
    }
    fn new_commit_messages(&self, input: &NewCommitMessagesInput) -> GitResult<NewCommitMessages> {
        self.tick();
        self.inner.new_commit_messages(input)
    }
    fn head_commit(&self) -> GitResult<String> {
        self.tick();
        self.inner.head_commit()
    }
    fn commit_subject(&self, sha: &str) -> GitResult<Option<String>> {
        self.tick();
        self.inner.commit_subject(sha)
    }
    fn merge(&self, input: &Value) -> GitResult<Value> {
        self.tick();
        self.inner.merge(input)
    }
    fn branch_tip(&self, branch: &str) -> GitResult<Option<String>> {
        self.tick();
        self.inner.branch_tip(branch)
    }
    fn is_in_head(&self, sha: &str) -> GitResult<bool> {
        self.tick();
        self.inner.is_in_head(sha)
    }
    fn delete_branch(&self, branch: &str, sha: &str) -> GitResult<bool> {
        self.tick();
        self.inner.delete_branch(branch, sha)
    }
    fn covered_reports(&self, head: &str, reports: &Value, options: &Value) -> GitResult<Value> {
        self.tick();
        self.inner.covered_reports(head, reports, options)
    }
    fn commit_on_head(&self, _sha: &str) -> GitResult<Option<bool>> {
        self.tick();
        match *self.answer.lock().unwrap() {
            Some(answer) => Ok(answer),
            None => Err(capstan_daemon::deps::GitError {
                message: "boom".into(),
            }),
        }
    }
}

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

impl World {
    /// `plan-1` with the package `api` assigned to dev-1, which has reported.
    fn reported(&self) {
        self.ok("pm-1", "plan", &["open", "normal", "Work"]);
        let body = json!({"summary": "one", "packages": [World::package("api")]});
        self.ok("arch-1", "plan", &["submit", "plan-1", &body.to_string()]);
        self.ok("pm-1", "plan", &["assign", "plan-1", "api", "dev-1"]);
        self.report("dev-1");
    }

    fn script(&self, answer: Option<Option<bool>>) {
        *self.git.answer.lock().unwrap() = answer;
    }

    fn calls(&self) -> usize {
        self.git.calls.load(Ordering::SeqCst)
    }
}

#[test]
fn a_non_operator_and_a_malformed_call_never_reach_git() {
    let w = World::new("", false);
    w.reported();
    let before = w.calls();
    for who in ["pm-1", "arch-1", "dev-1", "rev-1"] {
        let refused = w.refused(who, "plan", &["landed", "plan-1", "api", SHA]);
        assert!(
            refused.contains("only the operator may record a landed package"),
            "{who}: {refused}"
        );
        assert!(refused.to_lowercase().contains("forbidden"), "{refused}");
    }
    let refused = w.refused("pm-1", "plan", &["landed", "garbage"]);
    assert!(refused.contains("only the operator"), "{refused}");
    for args in [
        vec!["landed"],
        vec!["landed", "plan-1"],
        vec!["landed", "plan-1", "api"],
        vec!["landed", "plan-1", "api", &SHA[..39]],
        vec![
            "landed",
            "plan-1",
            "api",
            "zz23456789abcdef0123456789abcdef01234567",
        ],
        vec!["landed", "plan-1", "../x", SHA],
        vec!["landed", "plan-1", "api", SHA, "extra"],
        vec!["landed", "plan-1", "api", SHA, "--note"],
        vec!["landed", "plan-1", "api", "--clear", "x"],
    ] {
        let refused = w.refused("operator", "plan", &args);
        assert!(
            refused.to_lowercase().contains("invalid_request"),
            "{args:?}: {refused}"
        );
    }
    assert_eq!(w.calls(), before, "git was asked");
}

#[test]
fn each_answer_of_the_git_port_has_its_own_refusal() {
    let w = World::new("", false);
    w.reported();
    w.script(Some(Some(false)));
    let refused = w.refused("operator", "plan", &["landed", "plan-1", "api", SHA]);
    assert!(
        refused
            .contains("not_on_head: commit 0123456789ab is not in the history of the project HEAD"),
        "{refused}"
    );
    w.script(Some(None));
    let refused = w.refused("operator", "plan", &["landed", "plan-1", "api", SHA]);
    assert!(
        refused.contains("git_unavailable: the controller could not check the commit"),
        "{refused}"
    );
    w.script(None);
    let refused = w.refused("operator", "plan", &["landed", "plan-1", "api", SHA]);
    assert!(refused.contains("git_unavailable"), "{refused}");
}

#[test]
fn a_commit_on_head_is_recorded_and_cleared() {
    let w = World::new("", false);
    w.reported();
    let upper = SHA.to_uppercase();
    let done = w.ok(
        "operator",
        "plan",
        &[
            "landed",
            "plan-1",
            "api",
            &upper,
            "--note",
            "merged by hand",
        ],
    );
    assert_eq!(done["planId"], "plan-1");
    assert_eq!(done["packageId"], "api");
    assert_eq!(done["commitSha"], SHA);
    let refused = w.refused("operator", "plan", &["landed", "plan-1", "api", SHA]);
    assert!(
        refused.contains("plan_refused: already_landed"),
        "{refused}"
    );
    let cleared = w.ok("operator", "plan", &["landed", "plan-1", "api", "--clear"]);
    assert_eq!(cleared["packageId"], "api");
    let refused = w.refused("operator", "plan", &["landed", "plan-1", "api", "--clear"]);
    assert!(refused.contains("plan_refused: no_landing"), "{refused}");
    // The default note.
    w.ok("operator", "plan", &["landed", "plan-1", "api", SHA]);
    let notices = w.drain("pm-1").join("\n");
    assert!(notices.contains("recorded by the operator"), "{notices}");
}

#[test]
fn the_clear_form_is_operator_only_and_the_other_plan_forms_still_route() {
    let w = World::new("", false);
    w.reported();
    let refused = w.refused("pm-1", "plan", &["landed", "plan-1", "api", "--clear"]);
    assert!(
        refused.contains("only the operator may record a landed package"),
        "{refused}"
    );
    let opened = w.ok("pm-1", "plan", &["open", "normal", "Two", "--model", "m-1"]);
    assert_eq!(opened["planId"], "plan-2");
    let brief = w.ok("pm-1", "plan", &["show", "plan-1", "--brief"]);
    assert_eq!(brief["plan"]["planId"], "plan-1");
    let refused = w.refused(
        "pm-1",
        "plan",
        &["reassign", "plan-1", "arch-2", "--reason", "x"],
    );
    assert!(refused.contains("plan_refused: owner_active"), "{refused}");
    w.ok("pm-1", "plan", &["model", "plan-1", "--clear"]);
    let refused = w.refused("pm-1", "plan", &["bogus"]);
    assert!(refused.contains("archive or unarchive"), "{refused}");
}
