//! The project the decide-merge tests run in: a real repository, `[remote] merge = true` and a one-command `[verify]`
//! section, and a merged integration whose latest review passed and which has a passed verify run at its head with the
//! current commands. No agent is live (the developer's has ended); the PM is active. The daemon is in-process, with a
//! stub launcher, and also answers on a real socket.

use super::project::{build_repo, write_project, Identity};
use super::{private_tempdir, seeded_env, PrivateDir};
use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::handlers::handle_frame;
use capstan_daemon::handlers::shared::AbortSignal;
use capstan_daemon::handlers::MAX_RESPONSE_BYTES;
use capstan_daemon::server::{DaemonServer, ServerOptions};
use capstan_kernel::areas::snapshot_feed::content_hash;
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::Core;
use capstan_launcher::api::{LauncherService, StubLauncher};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

const BASE_CONFIG: &str = r#"schema_version = 1

[hosts.claude]
kind = "claude"

[roles.pm]
kind = "PM"
host = "claude"

[roles.developer]
kind = "Developer"
host = "claude"

[roles.reviewer]
kind = "Verifier"
host = "claude"
"#;

const AGENTS: [(&str, &str, &str); 3] = [
    ("pm-1", "pm", "PM"),
    ("dev-1", "developer", "Developer"),
    ("rev-1", "reviewer", "Verifier"),
];

/// The most a wait for the hand-off thread takes.
const HANDOFF_WAIT: Duration = Duration::from_secs(60);

pub struct MergeProject {
    pub dir: PrivateDir,
    pub deps: Deps,
    pub launcher: Arc<StubLauncher>,
    pub socket: PathBuf,
    /// The operator credential and the credential of each agent.
    pub credentials: HashMap<String, String>,
    pub report_id: String,
    pub integration_id: String,
    /// The integration branch.
    pub branch: String,
    scratch: tempfile::TempDir,
    remote_merge: bool,
    verify_commands: Vec<String>,
    /// How many messages the PM had when the last merge began; `wait_handoff` waits for one more.
    pm_seen: usize,
    _server: DaemonServer,
}

fn git(directory: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(directory)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("LC_ALL", "C")
        .env("HOME", "/nonexistent")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Merge Project")
        .env("GIT_AUTHOR_EMAIL", "merge@example.com")
        .env("GIT_COMMITTER_NAME", "Merge Project")
        .env("GIT_COMMITTER_EMAIL", "merge@example.com")
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

impl MergeProject {
    pub fn new() -> MergeProject {
        let dir = private_tempdir();
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let identity = Identity {
            project_id: "pmerge".into(),
            name: "Merge".into(),
            credential: format!("operator-{}", "m".repeat(40)),
        };
        let state = write_project(dir.path(), &identity);
        let heads = build_repo(
            dir.path(),
            &json!({
                "files": {"a.txt": "a\n"},
                "branches": {"feat/dev-1": {"files": {"one.txt": "one\n"}, "message": "feat: one"}},
            }),
        );
        // `identity_value` reads the identity from the repository's own config; no test changes HOME.
        git(dir.path(), &["config", "user.name", "Merge Project"]);
        git(dir.path(), &["config", "user.email", "merge@example.com"]);
        let (remote_merge, verify_commands) = (true, vec!["true".to_string()]);
        write_config(dir.path(), scratch.path(), remote_merge, &verify_commands);
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
                    seeded_env("merge-project"),
                )
            })
            .expect("the kernel opens")
        };
        let sink: Logger = Arc::new(|_: &LogEntry| {});
        let mut options = DaemonOptions::new(state.clone(), project, dir.path().to_path_buf());
        options.capstan = Some(Arc::new(
            capstan_config::load_role_config(dir.path()).unwrap(),
        ));
        let launcher = Arc::new(StubLauncher::new());
        let mut deps = Deps::new(kernel, options, sink);
        deps.launcher = Some(launcher.clone() as Arc<dyn LauncherService>);
        capstan_daemon::deps::startup_sequence(&deps);
        let socket = state.join("control.sock");
        let server = DaemonServer::start(deps.clone(), ServerOptions::new(socket.clone()))
            .expect("the server starts");
        let mut project = MergeProject {
            dir,
            deps,
            launcher,
            socket,
            credentials: HashMap::from([("operator".to_string(), identity.credential)]),
            report_id: String::new(),
            integration_id: String::new(),
            branch: String::new(),
            scratch,
            remote_merge,
            verify_commands,
            pm_seen: 0,
            _server: server,
        };
        project.populate(&heads);
        project.integrate_and_verify(&heads);
        project.pm_seen = project.pm_messages().len();
        project
    }

    /// Roles, agents (the PM active, the developer ended after its report) and the developer's pane.
    fn populate(&mut self, heads: &std::collections::BTreeMap<String, String>) {
        let owner = self.credentials["operator"].clone();
        let roles: Vec<Value> = [("pm", "PM"), ("developer", "Developer"), ("reviewer", "Verifier")]
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
        self.ledger(
            &internal,
            "recordAgentPane",
            vec![json!({
                "agentId": "dev-1",
                "workspaceId": "ws-1",
                "paneId": "pane-dev-1",
                "worktreePath": "/tmp/wt/dev-1",
                "branch": "feat/dev-1",
                "baseSha": heads["main"],
            })],
        );
    }

    /// Reports the developer's branch, reviews the report, integrates it, reviews the integration and verifies its head.
    fn integrate_and_verify(&mut self, heads: &std::collections::BTreeMap<String, String>) {
        let sha = heads["feat/dev-1"].clone();
        let evidence = json!({
            "generation": 1,
            "branch": "feat/dev-1",
            "baseSha": heads["main"],
            "commitExists": true,
            "branchTip": sha,
            "isAncestorOfTip": true,
            "isAncestorOfBase": false,
            "checkedAt": "2026-01-01T00:00:00.000Z",
        });
        let dev = self.credentials["dev-1"].clone();
        let reported = self.ledger(
            &dev,
            "recordAgentReport",
            vec![json!({"commitSha": sha, "summary": "One", "evidence": evidence})],
        );
        self.report_id = reported["record"]["reportId"].as_str().unwrap().to_string();
        let report = self.report_id.clone();
        self.review(&report, "pass");
        let merged = self.ok("pm-1", "integrate", &[&report]);
        assert_eq!(merged["state"], "merged", "{merged}");
        self.integration_id = merged["integrationId"].as_str().unwrap().to_string();
        self.branch = merged["branch"].as_str().unwrap().to_string();
        let integration = self.integration_id.clone();
        self.review(&integration, "pass");
        self.verify_passed(&integration);
        let owner = self.credentials["operator"].clone();
        self.deps
            .kernel
            .run(move |core| {
                let context = capstan_daemon::deps::new_context(core, &owner)?;
                core.end_agent(&context, "dev-1", None)
            })
            .expect("the developer's agent ends");
    }

    /// A review round on `subject` by the reviewer, ended with `verdict` (`pass` or `findings`).
    fn review(&self, subject: &str, verdict: &str) {
        let (pm, reviewer) = (
            self.credentials["pm-1"].clone(),
            self.credentials["rev-1"].clone(),
        );
        self.ledger(
            &pm,
            "beginReview",
            vec![json!({"subjectId": subject, "reviewerRole": "reviewer", "reviewerAgentId": "rev-1"})],
        );
        self.ledger(
            &reviewer,
            "completeReview",
            vec![json!({"verdict": verdict, "text": "Reviewed."})],
        );
    }

    /// A passed verify run for `subject` with the commands in `capstan.toml` now.
    fn verify_passed(&self, subject: &str) {
        let owner = self.credentials["operator"].clone();
        let internal = self.deps.credential().to_string();
        let config = capstan_config::load_role_config(self.dir.path()).unwrap();
        let verify = config.verify.expect("the project has a [verify] section");
        let queued = self.ledger(
            &owner,
            "queueVerifyRun",
            vec![json!({
                "subjectId": subject,
                "commands": verify.commands,
                "commandsSha": verify.commands_sha(),
                "trigger": "manual",
            })],
        );
        let run = queued["runId"].as_str().unwrap().to_string();
        let claimed = self.ledger(
            &internal,
            "claimVerifyRun",
            vec![json!({
                "runId": run,
                "maxParallel": 1,
                "scratchPath": self.scratch.path().join(&run).to_string_lossy(),
                "logPath": self.scratch.path().join(format!("{run}.log")).to_string_lossy(),
            })],
        );
        assert_eq!(claimed["claimed"], true, "{claimed}");
        self.ledger(
            &internal,
            "finishVerifyRun",
            vec![json!({"runId": run, "state": "passed", "exitCode": 0, "durationMs": 1})],
        );
    }

    // ---------------------------------------------------------------------------------------- changing one fact

    /// Rewrites `[remote] merge` in `capstan.toml`.
    pub fn set_remote_merge(&mut self, merge: bool) {
        self.remote_merge = merge;
        write_config(
            self.dir.path(),
            self.scratch.path(),
            self.remote_merge,
            &self.verify_commands,
        );
    }

    /// The integration's latest review has findings (a second round); nothing else changes.
    pub fn set_review_findings(&self) {
        self.review(&self.integration_id, "findings");
    }

    /// Rewrites the `[verify]` commands, so the passed run no longer has the current commands.
    pub fn set_verify_commands(&mut self, commands: &[&str]) {
        self.verify_commands = commands.iter().map(|c| c.to_string()).collect();
        write_config(
            self.dir.path(),
            self.scratch.path(),
            self.remote_merge,
            &self.verify_commands,
        );
    }

    /// Moves the integration branch one commit past its head (the tree is the same); returns the new tip.
    pub fn move_branch_tip(&self) -> String {
        let root = self.dir.path();
        let tip = git(root, &["rev-parse", &self.branch]);
        let moved = git(
            root,
            &[
                "commit-tree",
                &format!("{tip}^{{tree}}"),
                "-p",
                &tip,
                "-m",
                "chore: move the tip",
            ],
        );
        git(
            root,
            &["update-ref", &format!("refs/heads/{}", self.branch), &moved],
        );
        moved
    }

    /// The tip of `refname` in the project's repository.
    pub fn rev(&self, refname: &str) -> String {
        git(self.dir.path(), &["rev-parse", refname])
    }

    // ---------------------------------------------------------------------------------------- what happened

    /// Every launcher call so far.
    pub fn launcher_calls(&self) -> Vec<String> {
        self.launcher.calls()
    }

    /// The messages addressed to the PM, oldest first, from the ledger.
    pub fn pm_messages(&self) -> Vec<Value> {
        let messages = self
            .deps
            .kernel
            .run(|core| core.messages_for("pm-1"))
            .expect("the PM's messages");
        messages.as_array().cloned().unwrap_or_default()
    }

    /// Waits until the hand-off thread of the last applied merge is done: its cleanup is finished and its PM message is
    /// in the ledger.
    pub fn wait_handoff(&self) {
        let deadline = Instant::now() + HANDOFF_WAIT;
        while self.pm_messages().len() <= self.pm_seen {
            assert!(
                Instant::now() < deadline,
                "the hand-off thread did not finish"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    // ---------------------------------------------------------------------------------------- calls

    /// `ledger` ops on the kernel thread with a fresh context.
    pub fn ledger(&self, credential: &str, op: &str, args: Vec<Value>) -> Value {
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

    /// One request through `handle_frame`, as a connection would run it. `who` is an agent id, `operator` or a credential.
    pub fn call(&self, who: &str, command: &str, args: &[&str]) -> Value {
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

    pub fn ok(&self, who: &str, command: &str, args: &[&str]) -> Value {
        let answer = self.call(who, command, args);
        assert_eq!(answer["ok"], true, "{command} {args:?}: {answer}");
        answer["result"].clone()
    }

    /// The argv `decide_merge` sends for `nonce`, with the hash of the waiting item as it is now (twelve zeros when the
    /// integration does not wait).
    pub fn decide_merge_argv(&self, nonce: &str) -> Vec<String> {
        let request = format!("integration:{}", self.integration_id);
        let hash = self
            .deps
            .kernel
            .run({
                let request = request.clone();
                move |core| content_hash(core, &request)
            })
            .expect("the feed is read")
            .map_or_else(|| "0".repeat(12), |hash| hash[..12].to_string());
        [
            request.as_str(),
            "--expect",
            &hash,
            "--nonce",
            nonce,
            "merge",
        ]
        .map(String::from)
        .to_vec()
    }

    /// The decide-merge route for the project's integration as the operator, with `nonce`.
    pub fn decide_merge(&self, nonce: &str) -> Value {
        let argv = self.decide_merge_argv(nonce);
        self.decide_merge_args(&argv.iter().map(String::as_str).collect::<Vec<_>>())
    }

    /// The decide-merge route with a raw argv, as the operator.
    pub fn decide_merge_args(&self, args: &[&str]) -> Value {
        self.call("operator", "decide-merge", args)
    }

    /// One request over a fresh connection to the real socket.
    pub fn socket_call(&self, who: &str, command: &str, args: &[&str]) -> Value {
        let credential = self
            .credentials
            .get(who)
            .cloned()
            .unwrap_or_else(|| who.into());
        let mut stream = UnixStream::connect(&self.socket).expect("the socket accepts");
        let frame = json!({"v": 1, "credential": credential, "command": command, "args": args});
        stream.write_all(format!("{frame}\n").as_bytes()).unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("{line:?}: {e}"))
    }

    /// The state of the project's integration.
    pub fn integration(&self) -> Value {
        let id = self.integration_id.clone();
        self.deps
            .kernel
            .run(move |core| core.integration(&id))
            .expect("the integration")
    }

    /// The latest review of the project's integration, newest round last in the ledger.
    pub fn latest_review(&self) -> Value {
        let owner = self.credentials["operator"].clone();
        let reviews = self
            .deps
            .kernel
            .run(move |core| core.reviews(&owner, Some(200)))
            .expect("the reviews");
        reviews
            .as_array()
            .into_iter()
            .flatten()
            .filter(|r| r["integrationId"] == self.integration_id.as_str())
            .max_by_key(|r| r["round"].as_i64().unwrap_or(0))
            .cloned()
            .expect("the integration has a review")
    }

    /// The verify runs of the project's integration.
    pub fn verify_runs(&self) -> Vec<Value> {
        let id = self.integration_id.clone();
        let runs = self
            .deps
            .kernel
            .run(move |core| core.verify_runs(Some(&id), 50))
            .expect("the verify runs");
        runs["runs"].as_array().cloned().unwrap_or_default()
    }

    /// The commands sha `capstan.toml` has now.
    pub fn current_commands_sha(&self) -> String {
        capstan_config::load_role_config(self.dir.path())
            .unwrap()
            .verify
            .expect("a [verify] section")
            .commands_sha()
    }
}

/// `capstan.toml` with `[remote]` and a `[verify]` section of `commands`.
fn write_config(dir: &Path, scratch: &Path, merge: bool, commands: &[String]) {
    let commands: Vec<String> = commands
        .iter()
        .map(|c| serde_json::to_string(c).unwrap())
        .collect();
    let text = format!(
        "{BASE_CONFIG}\n[remote]\nmerge = {merge}\n\n[verify]\ncommands = [{}]\nscratch_dir = \"{}\"\n",
        commands.join(", "),
        scratch.display()
    );
    let file = dir.join("capstan.toml");
    std::fs::write(&file, text).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
}
