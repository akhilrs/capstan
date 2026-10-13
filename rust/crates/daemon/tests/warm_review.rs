//! Warm reviewers through the command handlers: a plan's reviewer is reused for its next review request, kept after a
//! verdict, evicted for room and released on signoff, cancel, idle time, its review limit and a restart that lost it. The
//! launcher is the recording stub; the kernel, the ledger and git are real.

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
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

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
    dir: common::PrivateDir,
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
            project_id: "pwarm".into(),
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
                        "warm-{}-{}",
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
            dir,
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

    /// A new commit on the agent's branch (same tree), reported again.
    fn rework(&mut self, agent: &str) -> (String, String) {
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

    /// Waits for the delayed release of `agent`.
    fn wait_released(&self, agent: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.released(agent) {
            assert!(
                Instant::now() < deadline,
                "{agent} not released: {:?}",
                self.calls()
            );
            std::thread::sleep(Duration::from_millis(25));
        }
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

#[test]
fn the_second_package_and_a_fix_round_reuse_the_first_reviewer() {
    let mut w = World::new("", false);
    w.plan();
    let (first, first_sha) = w.report("dev-1");
    let (second, second_sha) = w.report("dev-2");
    w.script_spawn("rev-1");
    let one = w.ok("pm-1", "request-review", &[&first]);
    assert_eq!(one["reviewerAgentId"], "rev-1");
    assert_eq!(one["reviewerReused"], false);
    // The brief names the scope of the diff, the package's paths and its acceptance.
    let brief = w.drain("rev-1").join("\n");
    assert!(brief.contains("Scope: "), "{brief}");
    assert!(brief.contains("one.txt"), "{brief}");
    assert!(brief.contains("src/core"), "{brief}");
    assert!(brief.contains("core works"), "{brief}");
    w.ok("rev-1", "review", &["pass", "Core is fine."]);
    std::thread::sleep(Duration::from_millis(1200));
    assert!(!w.released("rev-1"), "{:?}", w.calls());
    assert_eq!(w.warm().len(), 1);

    // The next package of the plan goes to the same reviewer, moved to its commit; nothing is spawned.
    let spawns = w.calls().iter().filter(|c| c.starts_with("spawn ")).count();
    let two = w.ok("pm-1", "request-review", &[&second]);
    assert_eq!(two["reviewerAgentId"], "rev-1");
    assert_eq!(two["reviewerReused"], true);
    assert_eq!(
        w.calls().iter().filter(|c| c.starts_with("spawn ")).count(),
        spawns
    );
    assert!(
        w.calls()
            .contains(&format!("retarget_review rev-1 {second_sha} {second}")),
        "{:?}",
        w.calls()
    );
    w.ok("rev-1", "review", &["findings", "- [ui] major ui is wrong"]);

    // The fix round of the first package is reviewed by the same reviewer at the new commit.
    let (fixed, fixed_sha) = w.rework("dev-2");
    w.drain("rev-1");
    let three = w.ok("pm-1", "request-review", &[&fixed]);
    assert_eq!(three["reviewerAgentId"], "rev-1");
    assert_eq!(three["reviewerReused"], true);
    assert!(w
        .calls()
        .contains(&format!("retarget_review rev-1 {fixed_sha} {fixed}")));
    let brief = w.drain("rev-1").join("\n");
    assert!(brief.contains("Judge this commit on its own"), "{brief}");
    assert!(
        brief.contains(&first_sha[..12]) || brief.contains(&first_sha),
        "{brief}"
    );
    assert_eq!(w.warm()[0]["reviewsCount"], 3);
}

#[test]
fn a_reviewer_who_authored_the_report_never_reviews_it() {
    // A Verifier cannot own a package (assign refuses it), so the author case that remains is a report outside a plan:
    // the review goes to another reviewer and the kernel refuses the author as reviewer outright.
    let w = World::new("", false);
    w.plan();
    let sha = w.heads["feat/dev-1"].clone();
    let internal = w.deps.credential().to_string();
    w.ledger(
        &internal,
        "recordAgentPane",
        vec![json!({
            "agentId": "rev-1", "workspaceId": "ws-1", "paneId": "pane-rev-1", "worktreePath": "/tmp/wt/rev-1",
            "branch": "feat/dev-1", "baseSha": w.heads["main"],
        })],
    );
    w.drain("rev-1");
    let report = w.ok("rev-1", "report", &[&sha, "Mine"])["reportId"]
        .as_str()
        .unwrap()
        .to_string();
    let credential = w.credentials["pm-1"].clone();
    let input =
        json!({"subjectId": report, "reviewerRole": "reviewer", "reviewerAgentId": "rev-1"});
    let begun = w.deps.kernel.run(move |core| {
        let context = capstan_daemon::deps::new_context(core, &credential)?;
        core.begin_review(&context, &input)
    });
    let message = begun.unwrap_err().message();
    assert!(message.contains("author"), "{message}");
    w.script_spawn("rev-2");
    let asked = w.ok("pm-1", "request-review", &[&report]);
    assert_eq!(asked["reviewerAgentId"], "rev-2");
    assert_eq!(asked["reviewerReused"], false);
    assert!(!w.calls().iter().any(|c| c.starts_with("retarget_review")));
}

#[test]
fn signoff_and_cancel_release_the_warm_reviewer() {
    let w = World::new("", false);
    w.plan();
    let (first, _) = w.report("dev-1");
    w.script_spawn("rev-1");
    w.ok("pm-1", "request-review", &[&first]);
    w.ok("rev-1", "review", &["pass", "fine"]);
    assert_eq!(w.warm().len(), 1);
    w.ok("operator", "plan", &["cancel", "plan-1"]);
    w.wait_released("rev-1");
    assert!(w.warm().is_empty());
}

#[test]
fn the_idle_loop_releases_a_reviewer_only_after_idle_minutes() {
    let w = World::new("\n[review]\nidle_minutes = 5\n", false);
    w.plan();
    let (first, _) = w.report("dev-1");
    w.script_spawn("rev-1");
    w.ok("pm-1", "request-review", &[&first]);
    // Busy: never idle-released.
    let used = w.warm()[0]["lastUsedAt"].as_str().unwrap().to_string();
    let used_ms = capstan_kernel::areas::messaging::parse_iso_ms(&used);
    capstan_daemon::warm::release_idle(&w.deps, used_ms + 10 * 60_000, 5);
    assert!(!w.released("rev-1"));
    w.ok("rev-1", "review", &["pass", "fine"]);
    capstan_daemon::warm::release_idle(&w.deps, used_ms + 5 * 60_000 - 1000, 5);
    assert!(!w.released("rev-1"));
    assert_eq!(w.warm().len(), 1);
    capstan_daemon::warm::release_idle(&w.deps, used_ms + 5 * 60_000 + 1000, 5);
    assert!(w.released("rev-1"), "{:?}", w.calls());
    assert!(w.warm().is_empty());
}

#[test]
fn the_max_reviews_th_verdict_releases_the_reviewer() {
    let w = World::new("\n[review]\nmax_reviews = 1\n", false);
    w.plan();
    let (first, _) = w.report("dev-1");
    w.script_spawn("rev-1");
    w.ok("pm-1", "request-review", &[&first]);
    w.ok("rev-1", "review", &["pass", "fine"]);
    w.wait_released("rev-1");
    assert!(w.warm().is_empty());
}

#[test]
fn recovery_keeps_a_held_warm_reviewer_and_marks_a_lost_one() {
    let w = World::new("", false);
    w.plan();
    let (first, _) = w.report("dev-1");
    w.script_spawn("rev-1");
    w.ok("pm-1", "request-review", &[&first]);
    w.ok("rev-1", "review", &["pass", "fine"]);
    capstan_daemon::reviews::recover_reviews(&w.deps, w.deps.credential());
    assert_eq!(w.warm().len(), 1);
    assert!(!w.released("rev-1"));
    // The agent ends: the next recovery marks the row lost.
    let internal = w.deps.credential().to_string();
    w.ledger(&internal, "endAgent", vec![json!("rev-1")]);
    capstan_daemon::reviews::recover_reviews(&w.deps, w.deps.credential());
    assert!(w.warm().is_empty());
}

#[test]
fn a_spawn_at_its_limit_evicts_an_idle_warm_reviewer_but_not_a_busy_one() {
    let w = World::new("", false);
    w.plan();
    let (first, _) = w.report("dev-1");
    w.script_spawn("rev-1");
    w.ok("pm-1", "request-review", &[&first]);
    // Busy (a review started): the original refusal stands.
    w.launcher.fail_next(
        "spawn",
        LauncherError::new("worker_limit", "2 of 2 workers are active"),
    );
    let refusal = w.refused("pm-1", "spawn", &["developer"]);
    assert!(refusal.contains("worker_limit"), "{refusal}");
    assert!(!w.released("rev-1"));
    // Idle: it is evicted and the spawn is tried again.
    w.ok("rev-1", "review", &["pass", "fine"]);
    w.launcher.fail_next(
        "spawn",
        LauncherError::new("worker_limit", "2 of 2 workers are active"),
    );
    w.script_spawn("dev-9");
    let spawned = w.ok("pm-1", "spawn", &["developer"]);
    assert_eq!(spawned["agentId"], "dev-9");
    assert!(w.released("rev-1"), "{:?}", w.calls());
    assert!(w.warm().is_empty());
}

#[test]
fn with_warm_reviewer_off_every_review_spawns_fresh_and_releases_after_the_verdict() {
    let w = World::new("\n[review]\nwarm_reviewer = false\n", false);
    w.plan();
    let (first, _) = w.report("dev-1");
    let (second, _) = w.report("dev-2");
    w.script_spawn("rev-1");
    let one = w.ok("pm-1", "request-review", &[&first]);
    assert_eq!(one["reviewerReused"], false);
    w.ok("rev-1", "review", &["pass", "fine"]);
    w.wait_released("rev-1");
    w.script_spawn("rev-2");
    let two = w.ok("pm-1", "request-review", &[&second]);
    assert_eq!(two["reviewerAgentId"], "rev-2");
    assert_eq!(two["reviewerReused"], false);
    assert!(!w.calls().iter().any(|c| c.starts_with("retarget_review")));
    assert!(w.warm().is_empty());
}

#[test]
fn a_failed_retarget_releases_the_warm_reviewer_and_spawns_fresh() {
    let w = World::new("", false);
    w.plan();
    let (first, _) = w.report("dev-1");
    let (second, _) = w.report("dev-2");
    w.script_spawn("rev-1");
    w.ok("pm-1", "request-review", &[&first]);
    w.ok("rev-1", "review", &["pass", "fine"]);
    w.launcher.fail_next(
        "retarget_review",
        LauncherError::new("worktree_dirty", "dirty"),
    );
    w.script_spawn("rev-2");
    let two = w.ok("pm-1", "request-review", &[&second]);
    assert_eq!(two["reviewerAgentId"], "rev-2");
    assert_eq!(two["reviewerReused"], false);
    assert!(w.released("rev-1"));
    // The fresh reviewer is now the plan's warm reviewer.
    assert_eq!(w.warm().len(), 1);
    assert_eq!(w.warm()[0]["agentId"], "rev-2");
}

#[test]
fn the_brief_names_the_gate_that_passed() {
    let w = World::new("", true);
    w.plan();
    let (first, sha) = w.report("dev-1");
    let queued = w.ok("dev-1", "verify", &[&first]);
    let id = queued["runId"].as_str().unwrap().to_string();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let run = w.ok("operator", "verify", &["show", &id]);
        if run["state"] == "passed" {
            break;
        }
        assert!(Instant::now() < deadline, "{run}");
        std::thread::sleep(Duration::from_millis(40));
    }
    w.script_spawn("rev-1");
    w.ok("pm-1", "request-review", &[&first]);
    let brief = w.drain("rev-1").join("\n");
    assert!(
        brief.contains(&format!("Gate passed at {sha} (run {id}, ")),
        "{brief}"
    );
}
