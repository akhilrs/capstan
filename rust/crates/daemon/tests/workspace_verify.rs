#![allow(dead_code)]
//! `cstan verify` in a workspace project: one run per repo a report names a commit for, the per-repo gate, `--repo`, the
//! one message per report, restart, the set refusal and the integrate gate. The kernel, the launcher, the ledger, git and
//! the verify service are real; the host (Herdr) is the launcher tests' stub.

mod common;

use capstan_config::TrustStore;
use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::handlers::handle_frame;
use capstan_daemon::handlers::shared::AbortSignal;
use capstan_daemon::handlers::MAX_RESPONSE_BYTES;
use capstan_daemon::ports::ProjectGit;
use capstan_daemon::verify::VerifyService;
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::Core;
use capstan_launcher::api::{LauncherService, SpawnOptions};
use capstan_launcher::kernel::Ledger;
use capstan_launcher::launcher::Launcher;
use capstan_launcher::shared::{FrontEndSite, LauncherOptions};
use common::project::{build_repo, write_project, Identity};
use common::{private_tempdir, seeded_env};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

// The launcher tests' stub host, with the two things it takes from its parent module.
fn normal(value: &Value, _scratch: &str) -> Value {
    value.clone()
}

mod engine {
    pub fn git_environment() -> Vec<(String, String)> {
        let path = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into());
        [
            ("PATH", path.as_str()),
            ("LC_ALL", "C"),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_TERMINAL_PROMPT", "0"),
            ("GIT_AUTHOR_NAME", "Test"),
            ("GIT_AUTHOR_EMAIL", "test@example.com"),
            ("GIT_COMMITTER_NAME", "Test"),
            ("GIT_COMMITTER_EMAIL", "test@example.com"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }
}

#[allow(dead_code)]
#[path = "../../launcher/tests/common/stub.rs"]
mod stub;
use stub::StubHerdr;

static WORLDS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(engine::git_environment())
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} in {}: {}",
        cwd.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn make_repo(dir: &Path, branch: &str) -> String {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "--quiet", "-b", branch]);
    std::fs::write(dir.join("README.md"), "repo\n").unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "--quiet", "-m", "chore: initial commit"]);
    git(dir, &["rev-parse", "HEAD"])
}

/// `(code, text)` of a refused answer.
fn refusal(answer: &Value) -> (String, String) {
    let message = answer["message"].as_str().unwrap_or_default();
    match message.split_once(": ") {
        Some((code, text)) if answer["code"] == "rejected" => (code.to_string(), text.to_string()),
        _ => (
            answer["code"].as_str().unwrap_or_default().to_string(),
            message.to_string(),
        ),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Shape {
    Normal,
    /// core has its own commands; the other repos use top-level commands the ledger refuses.
    Partial,
    /// `require = ["review"]`.
    Review,
}

struct World {
    _dir: common::PrivateDir,
    scratch: common::PrivateDir,
    deps: Deps,
    launcher: Arc<Launcher>,
    stub: Arc<StubHerdr>,
    changes: PathBuf,
    repos: PathBuf,
    base: String,
    tokens: Mutex<HashMap<String, String>>,
    service: Arc<VerifyService>,
    /// The files the gate commands wait for: `top` for the top-level commands, `web` for the web repo's own.
    top_gate: PathBuf,
    web_gate: PathBuf,
}

impl World {
    fn new() -> Arc<World> {
        World::build_with(true, true, Shape::Normal)
    }

    /// A world whose gate commands wait until `release` is called.
    fn held() -> Arc<World> {
        World::build_with(true, false, Shape::Normal)
    }

    /// `workspace` false writes a configuration with no `[repos]` and no `[workspace]`.
    fn build(workspace: bool) -> Arc<World> {
        World::build_with(workspace, true, Shape::Normal)
    }

    /// `open` writes the gate files first, so the commands do not wait.
    fn build_with(workspace: bool, open: bool, shape: Shape) -> Arc<World> {
        let require = if shape == Shape::Review {
            "require = [\"review\"]\n"
        } else {
            "require = []\n"
        };
        let dir = private_tempdir();
        let scratch = private_tempdir();
        let repos = scratch.path().join("repos");
        let (top_gate, web_gate) = (
            scratch.path().join("gate-top"),
            scratch.path().join("gate-web"),
        );
        let overrides = if shape == Shape::Partial {
            "[repos.core.verify]\ncommands = [\"true\"]\n".to_string()
        } else {
            format!(
                "[repos.web.verify]\ncommands = [\"while [ ! -f {w} ]; do sleep 0.1; done; test ! -f web-fail.txt\"]\n\n[repos.extra.verify]\ncommands = [\"true\"]\n",
                w = web_gate.display()
            )
        };
        let changes = scratch.path().join("changes");
        let base = make_repo(&repos.join("core"), "main");
        make_repo(&repos.join("web"), "main");
        make_repo(&repos.join("extra"), "main");
        let identity = Identity {
            project_id: "pvws".into(),
            name: "Verify Ws".into(),
            credential: format!("operator-{}", "v".repeat(40)),
        };
        let state = write_project(dir.path(), &identity);
        let config = format!(
            r#"schema_version = 1

[hosts.claude]
kind = "claude"

[architect]
enabled = false

[roles.pm]
kind = "PM"
host = "claude"

[roles.developer]
kind = "Developer"
host = "claude"

{reviewer}{ws}"#,
            reviewer = if shape == Shape::Review {
                "[roles.reviewer]\nkind = \"Verifier\"\nhost = \"claude\"\n\n"
            } else {
                ""
            },
            ws = if workspace {
                format!(
                    r#"[repos.core]
path = "{r}/core"
main_branch = "main"

[repos.web]
path = "{r}/web"
main_branch = "main"

{overrides}
[repos.extra]
path = "{r}/extra"
main_branch = "main"

[workspace]
default_repo = "core"
merge_order = ["core", "web", "extra"]
worktree_dir = "{c}"
"#,
                    r = repos.display(),
                    c = changes.display(),
                    overrides = overrides
                )
            } else {
                String::new()
            },
        ) + &format!(
            "\n[verify]\ncommands = [\"while [ ! -f {t} ]; do sleep 0.1; done; test ! -f fail.txt\"]\nscratch_dir = \"{s}\"\nmax_parallel = 4\n{req}",
            t = top_gate.display(),
            s = scratch.path().join("vscratch").display(),
            req = require,
        );
        let file = dir.path().join("capstan.toml");
        std::fs::write(&file, config).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        let trust: TrustStore = common::kept_trust_store();
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
                    seeded_env(&format!(
                        "verify-ws-{}-{}",
                        std::process::id(),
                        WORLDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    )),
                )
            })
            .expect("the kernel opens")
        };
        let sink: Logger = Arc::new(|_entry: &LogEntry| {});
        let mut options = DaemonOptions::new(state.clone(), project, dir.path().to_path_buf());
        options.trust_store = trust.clone();
        let role_config = capstan_config::load_role_config_with(dir.path(), &trust).unwrap();
        assert_eq!(role_config.workspace.is_some(), workspace);
        let role_config_verify = role_config.verify.clone().expect("[verify] is configured");
        options.capstan = Some(Arc::new(role_config.clone()));
        let mut deps = Deps::new(kernel, options, sink);
        deps.git = Arc::new(ProjectGit::new(dir.path()));
        let real_git = String::from_utf8_lossy(
            &Command::new("sh")
                .args(["-c", "command -v git"])
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .to_string();
        let prompts = scratch.path().join("prompts");
        std::fs::create_dir_all(&prompts).unwrap();
        let root_text = dir.path().to_string_lossy().into_owned();
        let stub = Arc::new(StubHerdr::new(
            &scratch.path().to_string_lossy(),
            prompts,
            &changes.to_string_lossy(),
            &real_git,
            &root_text,
        ));
        let sync_deps = deps.clone();
        let launcher = Arc::new(Launcher::new(LauncherOptions {
            ledger: Ledger::new(Arc::new(deps.kernel.clone()), deps.credential()),
            adapter: stub.clone(),
            config: role_config,
            project_root: root_text,
            project_id: "pvws".into(),
            cli_path: "/opt/capstan/cli.js".into(),
            socket_path: state.join("control.sock").to_string_lossy().into_owned(),
            credential: deps.credential().to_string(),
            node_path: Some("/usr/bin/node".into()),
            base_environment: HashMap::from([("PATH".to_string(), std::env::var("PATH").unwrap())]),
            git: None,
            now: None,
            log: None,
            sync_roles: Some(Arc::new(move || {
                capstan_daemon::run::sync_configured_roles(&sync_deps)
            })),
            run_setup: None,
            sleep: None,
            run_teardown: None,
            site: Some(FrontEndSite {
                env: HashMap::new(),
                exec_path: "/usr/bin/node".into(),
                sea: false,
                cstan: "/usr/bin/cstan".into(),
            }),
        }));
        deps.launcher = Some(launcher.clone() as Arc<dyn LauncherService>);
        capstan_daemon::run::sync_configured_roles(&deps).expect("the roles sync");
        let mut role_config_verify = role_config_verify;
        if shape == Shape::Partial {
            // Repos without their own commands use these: too many for the ledger, so their runs cannot be queued.
            role_config_verify.commands = vec!["true".to_string(); 33];
            role_config_verify.path_filters = vec![None; 33];
        }
        let service = VerifyService::with_config(&deps, role_config_verify);
        deps.verify = Some(service.clone());
        if open {
            std::fs::write(&top_gate, "").unwrap();
            std::fs::write(&web_gate, "").unwrap();
        }
        service.start();
        let world = Arc::new(World {
            _dir: dir,
            scratch,
            deps,
            launcher,
            stub,
            changes,
            repos,
            base,
            tokens: Mutex::default(),
            service,
            top_gate,
            web_gate,
        });
        let pm = world.launcher.launch_pm().expect("the PM launches");
        world.note_token(&pm.agent_id.expect("the PM has an id"));
        world
    }

    /// Remembers the credential the stub's last agent start carried, for `agent`.
    fn note_token(&self, agent: &str) {
        for call in self.stub.take_calls() {
            if call["m"] != "startAgent" {
                continue;
            }
            if let Some(token) = call["a"][0]["environment"]["CAPSTAN_TOKEN"].as_str() {
                self.tokens
                    .lock()
                    .unwrap()
                    .insert(agent.to_string(), token.to_string());
            }
        }
    }

    fn spawn(&self, options: &SpawnOptions) -> String {
        let spawned = self
            .launcher
            .spawn("developer", options)
            .unwrap_or_else(|e| panic!("the spawn is refused: {e}"));
        self.note_token(&spawned.agent_id);
        spawned.agent_id
    }

    fn call(&self, who: &str, command: &str, args: &[&str]) -> Value {
        let credential = self
            .tokens
            .lock()
            .unwrap()
            .get(who)
            .cloned()
            .unwrap_or_else(|| who.to_string());
        let frame = json!({"v": 1, "credential": credential, "command": command, "args": args});
        let outcome = handle_frame(
            &self.deps,
            frame.to_string().as_bytes(),
            &AbortSignal::new(),
            MAX_RESPONSE_BYTES,
        );
        serde_json::from_str(&outcome.response.expect("an answer")).expect("JSON")
    }

    fn repo(&self, name: &str) -> PathBuf {
        self.repos.join(name)
    }

    fn ledger<R: Send + 'static>(
        &self,
        f: impl FnOnce(&Core) -> capstan_kernel::KernelResult<R> + Send + 'static,
    ) -> R {
        self.deps.kernel.run(f).expect("the ledger answers")
    }

    fn rows(&self, agent: &str) -> Vec<Value> {
        let agent = agent.to_string();
        self.ledger(move |core| core.list_agent_worktrees(&agent))
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    /// The worktree of `agent` in `repo`.
    fn tree(&self, agent: &str, repo: &str) -> PathBuf {
        let row = self
            .rows(agent)
            .into_iter()
            .find(|row| row["repo"] == repo)
            .unwrap_or_else(|| panic!("{agent} has no {repo} row"));
        PathBuf::from(row["worktreePath"].as_str().unwrap())
    }

    /// A worker with a worktree in each of `repos`.
    fn worker(&self, repos: &[&str]) -> String {
        let agent = self.spawn(&SpawnOptions::default());
        for repo in repos.iter().filter(|repo| **repo != "core") {
            let answer = self.call(&agent, "repo", &["add", repo]);
            assert_eq!(answer["ok"], true, "repo add {repo}: {answer}");
        }
        agent
    }

    /// A commit in the agent's worktree of `repo`; its id.
    fn commit(&self, agent: &str, repo: &str, file: &str) -> String {
        let tree = self.tree(agent, repo);
        std::fs::write(tree.join(file), format!("{file}\n")).unwrap();
        git(&tree, &["add", "-A"]);
        git(
            &tree,
            &["commit", "--quiet", "-m", &format!("feat: add {file}")],
        );
        git(&tree, &["rev-parse", "HEAD"])
    }

    /// The worktree of `agent` in a project without `[repos]`: the folder under the worktree directory named for it.
    fn single_tree(&self, agent: &str) -> PathBuf {
        std::fs::read_dir(&self.changes)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .find(|p| p.join(".git").exists() && p.to_string_lossy().contains(agent))
            .unwrap_or_else(|| panic!("no worktree for {agent} in {}", self.changes.display()))
    }

    fn head(&self, agent: &str, repo: &str) -> String {
        git(&self.tree(agent, repo), &["rev-parse", "HEAD"])
    }

    fn base(&self, agent: &str, repo: &str) -> String {
        self.rows(agent)
            .into_iter()
            .find(|row| row["repo"] == repo)
            .unwrap()["baseSha"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn report(&self, who: &str, args: &[&str]) -> Value {
        self.call(who, "report", args)
    }

    fn accepted(&self, who: &str, args: &[&str]) -> Value {
        let answer = self.report(who, args);
        assert_eq!(answer["ok"], true, "report {args:?}: {answer}");
        answer["result"].clone()
    }

    fn refused(&self, who: &str, args: &[&str]) -> String {
        let answer = self.report(who, args);
        assert_eq!(answer["ok"], false, "report {args:?}: {answer}");
        answer["message"].as_str().unwrap_or_default().to_string()
    }

    /// The `(repo, commit)` rows of a report.
    fn commits_of(&self, report_id: &Value) -> Vec<(String, String)> {
        let id = report_id.as_str().unwrap().to_string();
        self.ledger(move |core| core.report_commits(&id))
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    row["repo"].as_str().unwrap().to_string(),
                    row["commitSha"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    }

    fn report_count(&self) -> usize {
        let controller = self.deps.credential().to_string();
        self.ledger(move |core| core.agent_reports(&controller, Some(1000)))
            .as_array()
            .unwrap()
            .len()
    }
}

impl Drop for World {
    fn drop(&mut self) {
        self.service.stop();
    }
}

fn pair(repo: &str, sha: &str) -> String {
    format!("{repo}={sha}")
}

/// What `verify` answered, with its refusal text when it refused.
fn text_of(answer: &Value) -> String {
    answer["message"].as_str().unwrap_or_default().to_string()
}

impl World {
    fn verify(&self, args: &[&str]) -> Value {
        self.call("pm-1", "verify", args)
    }

    fn queued(&self, args: &[&str]) -> Vec<Value> {
        let answer = self.verify(args);
        assert_eq!(answer["ok"], true, "verify {args:?}: {answer}");
        answer["result"]["runs"].as_array().cloned().unwrap()
    }

    fn run(&self, id: &str) -> Value {
        let answer = self.verify(&["show", id]);
        assert_eq!(answer["ok"], true, "{answer}");
        answer["result"].clone()
    }

    /// Waits until every run of the subject is final.
    fn settle(&self, report: &str) {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            let id = report.to_string();
            let runs = self.ledger(move |core| core.verify_runs(Some(&id), 50));
            let all_final = runs["runs"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| !matches!(r["state"].as_str(), Some("queued" | "running")));
            if all_final {
                return;
            }
            assert!(
                std::time::Instant::now() < end,
                "runs did not settle: {runs}"
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    /// Waits until `run_id` is in a final state; its record.
    fn wait_run(&self, run_id: &str) -> Value {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            let run = self.run(run_id);
            if !matches!(run["state"].as_str(), Some("queued" | "running")) {
                return run;
            }
            assert!(std::time::Instant::now() < end, "run did not end: {run}");
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    /// The controller messages the PM got for the report, oldest first.
    fn messages(&self, report: &str) -> Vec<String> {
        let like = format!("Verify for report {report}:%");
        self.ledger(move |core| {
            let kernel = core.kernel();
            capstan_kernel::helpers::query_all(
                &kernel.database,
                "SELECT body FROM messages WHERE project_id = ? AND recipient_agent_id = 'pm-1' AND body LIKE ? ORDER BY sequence",
                [&kernel.project_id, &like],
                |row| row.get::<_, String>(0),
            )
        })
    }

    fn release(&self) {
        std::fs::write(&self.top_gate, "").unwrap();
        std::fs::write(&self.web_gate, "").unwrap();
    }

    /// A report naming one commit in each of `repos` (each with the files given, `fail.txt` making the top-level
    /// commands fail and `web-fail.txt` the web repo's own).
    fn reported(&self, repos: &[(&str, &str)]) -> String {
        let names: Vec<&str> = repos.iter().map(|(r, _)| *r).collect();
        let agent = self.worker(&names);
        let mut pairs = Vec::new();
        for (repo, file) in repos {
            let sha = self.commit(&agent, repo, file);
            pairs.push(pair(repo, &sha));
        }
        let mut args: Vec<&str> = pairs.iter().map(String::as_str).collect();
        args.push("done");
        self.accepted(&agent, &args)["reportId"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn gate_repo(&self, repo: &str, commit: &str, sha: &str) -> Value {
        let (repo, commit, sha) = (repo.to_string(), commit.to_string(), sha.to_string());
        self.ledger(move |core| core.verify_gate_in_repo(&repo, &commit, &sha))
    }
}

// ------------------------------------------------------------------------------------------------ one run per repo

#[test]
fn verify_runs_once_per_repo_with_its_commands_and_the_report_hears_once() {
    let w = World::new();
    let report = w.reported(&[("core", "a.txt"), ("extra", "c.txt"), ("web", "b.txt")]);
    let runs = w.queued(&[&report]);
    let repos: Vec<&str> = runs.iter().map(|r| r["repo"].as_str().unwrap()).collect();
    assert_eq!(repos, ["core", "extra", "web"]);
    assert!(runs.iter().all(|r| r["existing"] == false));
    for run in &runs {
        let done = w.wait_run(run["runId"].as_str().unwrap());
        assert_eq!(done["state"], "passed", "{done}");
        assert_eq!(done["repo"], run["repo"]);
    }
    // Each repo ran its own commands: extra's own, the others' the top-level or their own.
    let extra = w.run(runs[1]["runId"].as_str().unwrap());
    assert_eq!(extra["commands"], json!(["true"]));
    let core = w.run(runs[0]["runId"].as_str().unwrap());
    assert!(core["commands"][0].as_str().unwrap().contains("fail.txt"));
    assert_ne!(core["commandsSha"], extra["commandsSha"]);
    // The ledger names the runs that decide the gate.
    let id = report.clone();
    let decided =
        w.ledger(move |core| core.subject_runs(&capstan_kernel::records::GateSubject::Report(id)));
    assert_eq!(decided.len(), 3);
    // One message, once all are final, listing each repo.
    let messages = w.messages(&report);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].contains("passed in every repo"),
        "{}",
        messages[0]
    );
    for repo in ["core", "extra", "web"] {
        assert!(
            messages[0].contains(&format!("repo {repo} at")),
            "{}",
            messages[0]
        );
    }
    // The integrate gate passes: integrate is not refused for verification.
    let integrated = w.call("pm-1", "integrate", &[&report]);
    assert!(
        !text_of(&integrated).contains("verify_required"),
        "{integrated}"
    );
}

#[test]
fn a_failing_repo_fails_the_gate_and_a_rerun_after_the_message_gives_one_more() {
    let w = World::new();
    // The second repo's commit holds fail.txt: the top-level commands fail there.
    let report = w.reported(&[("core", "a.txt"), ("web", "web-fail.txt")]);
    let runs = w.queued(&[&report]);
    for run in &runs {
        w.wait_run(run["runId"].as_str().unwrap());
    }
    let web = runs.iter().find(|r| r["repo"] == "web").unwrap();
    assert_eq!(w.run(web["runId"].as_str().unwrap())["state"], "failed");
    let messages = w.messages(&report);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("did not pass"), "{}", messages[0]);
    // The gate does not pass: integrate is refused for verification.
    let integrated = w.call("pm-1", "integrate", &[&report]);
    assert!(
        text_of(&integrated).starts_with("verify_required"),
        "{integrated}"
    );
    assert!(w.messages("none").is_empty());
    // --repo reruns one repo; its message is one more.
    let again = w.queued(&[&report, "--repo", "web"]);
    assert_eq!(again.len(), 1);
    assert_eq!(again[0]["repo"], "web");
    assert_eq!(again[0]["existing"], false);
    assert_ne!(again[0]["runId"], web["runId"]);
    w.wait_run(again[0]["runId"].as_str().unwrap());
    w.settle(&report);
    assert_eq!(w.messages(&report).len(), 2);
}

#[test]
fn a_two_repo_report_whose_second_repo_fails_does_not_pass() {
    let w = World::new();
    let report = w.reported(&[("core", "a.txt"), ("extra", "fail.txt")]);
    let runs = w.queued(&[&report]);
    for run in &runs {
        w.wait_run(run["runId"].as_str().unwrap());
    }
    // core (top-level commands) passed; extra's own commands (`true`) passed too: both pass.
    assert!(w.call("pm-1", "integrate", &[&report])["message"]
        .as_str()
        .is_none_or(|m| !m.contains("verify_required")));
    // The same shape with the failing file in a repo that uses the top-level commands fails.
    let report = w.reported(&[("extra", "e.txt"), ("core", "fail.txt")]);
    for run in w.queued(&[&report]) {
        w.wait_run(run["runId"].as_str().unwrap());
    }
    let refused = w.call("pm-1", "integrate", &[&report]);
    let text = text_of(&refused);
    assert!(text.starts_with("verify_required"), "{refused}");
}

#[test]
fn a_cancelled_repo_fails_the_gate() {
    let w = World::held();
    let report = w.reported(&[("core", "a.txt"), ("web", "b.txt")]);
    let runs = w.queued(&[&report]);
    let web = runs.iter().find(|r| r["repo"] == "web").unwrap()["runId"]
        .as_str()
        .unwrap()
        .to_string();
    // The run is held by its gate file; cancel it, let the other one go.
    let cancelled = w.call("pm-1", "verify", &["cancel", &web]);
    assert_eq!(cancelled["ok"], true, "{cancelled}");
    w.release();
    w.settle(&report);
    assert_eq!(w.run(&web)["state"], "interrupted");
    let refused = w.call("pm-1", "integrate", &[&report]);
    assert!(
        text_of(&refused).starts_with("verify_required"),
        "{refused}"
    );
}

// ------------------------------------------------------------------------------------------------ dedupe and --repo

#[test]
fn a_second_verify_while_runs_are_active_returns_them_and_a_repo_run_never_refuses() {
    let w = World::held();
    let report = w.reported(&[("core", "a.txt"), ("web", "b.txt")]);
    let first = w.queued(&[&report]);
    let second = w.queued(&[&report]);
    assert_eq!(first.len(), 2);
    for (a, b) in first.iter().zip(&second) {
        assert_eq!(a["runId"], b["runId"]);
        assert_eq!(b["existing"], true);
    }
    let raced = w.queued(&[&report, "--repo", "core"]);
    assert_eq!(raced[0]["existing"], true);
    assert_eq!(raced[0]["runId"], first[0]["runId"]);
    w.release();
    w.settle(&report);
}

#[test]
fn a_replaced_failed_run_gives_one_message() {
    let w = World::held();
    // core uses the top-level commands (gate-top), web its own (gate-web, held).
    let report = w.reported(&[("core", "fail.txt"), ("web", "b.txt")]);
    let first = w.queued(&[&report]);
    std::fs::write(&w.top_gate, "").unwrap();
    let core_first = first.iter().find(|r| r["repo"] == "core").unwrap();
    assert_eq!(
        w.wait_run(core_first["runId"].as_str().unwrap())["state"],
        "failed"
    );
    assert!(w.messages(&report).is_empty(), "web is still running");
    // A newer core run replaces the failed one.
    let again = w.queued(&[&report, "--repo", "core"]);
    assert_ne!(again[0]["runId"], core_first["runId"]);
    w.wait_run(again[0]["runId"].as_str().unwrap());
    assert!(w.messages(&report).is_empty(), "web is still running");
    std::fs::write(&w.web_gate, "").unwrap();
    w.settle(&report);
    let messages = w.messages(&report);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].contains(again[0]["runId"].as_str().unwrap()),
        "{}",
        messages[0]
    );
    assert!(
        !messages[0].contains(core_first["runId"].as_str().unwrap()),
        "{}",
        messages[0]
    );
}

#[test]
fn both_repo_refusals() {
    let w = World::new();
    let report = w.reported(&[("core", "a.txt")]);
    let answer = w.verify(&[&report, "--repo", "web"]);
    assert_eq!(answer["ok"], false);
    assert!(
        text_of(&answer).ends_with(&format!("repo web is not in report {report}")),
        "{answer}"
    );
    let single = World::build(false);
    let agent = single.spawn(&SpawnOptions::default());
    let tree = single.single_tree(&agent);
    std::fs::write(tree.join("n.txt"), "n\n").unwrap();
    git(&tree, &["add", "-A"]);
    git(&tree, &["commit", "--quiet", "-m", "feat: a"]);
    let sha = git(&tree, &["rev-parse", "HEAD"]);
    let id = single.accepted(&agent, &[&sha, "one"])["reportId"]
        .as_str()
        .unwrap()
        .to_string();
    let answer = single.verify(&[&id, "--repo", "core"]);
    assert!(
        text_of(&answer).ends_with("--repo is only for workspace projects"),
        "{answer}"
    );
    // A single-repo verify is as it was: one run, no repo member, no list.
    let plain = single.verify(&[&id]);
    assert_eq!(plain["ok"], true, "{plain}");
    assert!(plain["result"].get("runs").is_none());
    assert!(plain["result"].get("repo").is_none());
}

// ------------------------------------------------------------------------------------------------ recover

#[test]
fn a_restart_mid_run_gives_one_message() {
    let w = World::held();
    // core waits on gate-top (held); extra's own commands pass at once.
    let report = w.reported(&[("core", "a.txt"), ("extra", "c.txt")]);
    let runs = w.queued(&[&report]);
    let extra = runs.iter().find(|r| r["repo"] == "extra").unwrap()["runId"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(w.wait_run(&extra)["state"], "passed");
    assert!(w.messages(&report).is_empty());
    // The daemon stops: the running run is interrupted; every repo is final, so the report hears once.
    w.service.stop();
    let core = runs.iter().find(|r| r["repo"] == "core").unwrap()["runId"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(w.run(&core)["state"], "interrupted");
    // The new daemon recovers: nothing more is said.
    let restarted = VerifyService::with_config(&w.deps, w.service.config().clone());
    restarted.recover();
    let messages = w.messages(&report);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].contains("interrupted") && messages[0].contains("passed"),
        "{}",
        messages[0]
    );
}

// ------------------------------------------------------------------------------------------------ sets and old reports

#[test]
fn a_set_that_is_not_merged_has_no_runs_and_is_not_verified() {
    let w = World::new();
    let credential = w.deps.credential().to_string();
    let set_id = w.ledger(move |core| {
        let context = capstan_daemon::deps::new_context(core, &credential)?;
        Ok(core.create_integration_set(
            &context,
            &json!({"branch": "int/set", "repos": ["core", "web"]}),
        )?["setId"]
            .as_str()
            .unwrap()
            .to_string())
    });
    let ids = set_id.clone();
    let err = w
        .deps
        .kernel
        .run(move |core| core.subject_runs(&capstan_kernel::records::GateSubject::Set(ids)));
    assert!(err.unwrap().is_empty());
    // Seed an integration and make it the member of core.
    let credential = w.deps.credential().to_string();
    let (set_for_member, base) = (set_id.clone(), w.base.clone());
    w.ledger(move |core| {
        let kernel = core.kernel();
        capstan_kernel::helpers::execute(
            &kernel.database,
            "INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, created_at)
             VALUES (?, 'i-1', '1', ?, 'int/i-1', 'pm-1', 'running', '2026-01-01T00:00:00.000Z')",
            [&kernel.project_id, &base],
        )?;
        let context = capstan_daemon::deps::new_context(core, &credential)?;
        core.attach_set_member(
            &context,
            &json!({"setId": set_for_member, "repo": "core", "integrationId": "i-1"}),
        )
    });
    let member = w.verify(&["i-1"]);
    assert!(
        text_of(&member).ends_with(&format!(
            "set {set_id} is running; only a merged set is verified"
        )),
        "{member}"
    );
    let whole = w.verify(&[&set_id]);
    assert!(
        text_of(&whole).contains("only a merged set is verified"),
        "{whole}"
    );
}

#[test]
fn a_report_from_before_workspace_mode_is_refused_and_still_listed() {
    let w = World::new();
    // A report with no commits per repo: a copy of an accepted report without its report_commits rows, as a ledger from
    // before workspace mode holds it.
    let agent = w.worker(&["core"]);
    let sha = w.commit(&agent, "core", "a.txt");
    let made = w.accepted(&agent, &[&pair("core", &sha), "one"])["reportId"]
        .as_str()
        .unwrap()
        .to_string();
    let old = "r-old".to_string();
    let (from, to) = (made.clone(), old.clone());
    w.ledger(move |core| {
        let kernel = core.kernel();
        let columns: Vec<String> = capstan_kernel::helpers::query_all(
            &kernel.database,
            "SELECT name FROM pragma_table_info('agent_reports')",
            rusqlite::params![],
            |row| row.get::<_, String>(0),
        )?;
        let select: Vec<String> = columns
            .iter()
            .map(|c| match c.as_str() {
                "report_id" => "'r-old'".to_string(),
                "sequence" => "(SELECT MAX(sequence) + 1 FROM agent_reports)".to_string(),
                other => other.to_string(),
            })
            .collect();
        capstan_kernel::helpers::execute(
            &kernel.database,
            &format!(
                "INSERT INTO agent_reports({}) SELECT {} FROM agent_reports WHERE project_id = ? AND report_id = ?",
                columns.join(", "),
                select.join(", ")
            ),
            [&kernel.project_id, &from],
        )?;
        let _ = &to;
        Ok(())
    });
    let report = old;
    let answer = w.verify(&[&report]);
    assert_eq!(answer["ok"], false, "{answer}");
    assert!(
        text_of(&answer).contains("before workspace mode"),
        "{answer}"
    );
    assert_eq!(w.report_count(), 2);
}

// ------------------------------------------------------------------------------------------------ the integrate gate

#[test]
fn with_verify_configured_and_require_empty_a_workspace_integrate_needs_every_repo_verified() {
    let w = World::new();
    let report = w.reported(&[("core", "a.txt"), ("web", "b.txt")]);
    // Only core is verified.
    let runs = w.queued(&[&report, "--repo", "core"]);
    w.wait_run(runs[0]["runId"].as_str().unwrap());
    let refused = w.call("pm-1", "integrate", &[&report]);
    assert!(
        text_of(&refused).starts_with("verify_required"),
        "{refused}"
    );
    // The other repo too: the gate lets it through (other refusals may still apply).
    let runs = w.queued(&[&report, "--repo", "web"]);
    w.wait_run(runs[0]["runId"].as_str().unwrap());
    let next = w.call("pm-1", "integrate", &[&report]);
    assert!(!text_of(&next).contains("verify_required"), "{next}");
}

#[test]
fn a_single_repo_project_with_require_empty_integrates_as_today() {
    let w = World::build(false);
    let agent = w.spawn(&SpawnOptions::default());
    let tree = w.single_tree(&agent);
    std::fs::write(tree.join("n.txt"), "n\n").unwrap();
    git(&tree, &["add", "-A"]);
    git(&tree, &["commit", "--quiet", "-m", "feat: a"]);
    let sha = git(&tree, &["rev-parse", "HEAD"]);
    let id = w.accepted(&agent, &[&sha, "one"])["reportId"]
        .as_str()
        .unwrap()
        .to_string();
    let answer = w.call("pm-1", "integrate", &[&id]);
    assert!(!text_of(&answer).contains("verify_required"), "{answer}");
}

#[test]
fn a_queue_that_fails_partway_keeps_the_runs_it_queued_and_says_so() {
    let w = World::build_with(true, true, Shape::Partial);
    let report = w.reported(&[("core", "a.txt"), ("extra", "c.txt")]);
    let answer = w.verify(&[&report]);
    assert_eq!(answer["ok"], false, "{answer}");
    let text = text_of(&answer);
    // core was queued before extra was refused; its run is named and runs without waiting for the idle poll.
    let id = report.clone();
    let runs = w.ledger(move |core| core.verify_runs(Some(&id), 10));
    let runs = runs["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert!(text.contains(runs[0]["runId"].as_str().unwrap()), "{text}");
    assert_eq!(
        w.wait_run(runs[0]["runId"].as_str().unwrap())["state"],
        "passed"
    );
}

#[test]
fn the_review_gate_of_a_workspace_uses_each_repos_own_commands() {
    let w = World::build_with(true, true, Shape::Review);
    // extra has its own commands: its runs carry another commands sha than the top-level one.
    let report = w.reported(&[("core", "a.txt"), ("extra", "c.txt")]);
    let refused = w.call("pm-1", "request-review", &[&report]);
    assert!(text_of(&refused).contains("verify_required"), "{refused}");
    for run in w.queued(&[&report]) {
        w.wait_run(run["runId"].as_str().unwrap());
    }
    let next = w.call("pm-1", "request-review", &[&report]);
    assert!(!text_of(&next).contains("verify_required"), "{next}");
}

#[test]
fn the_review_gate_of_a_workspace_refuses_a_set_member_and_gates_a_report_per_repo() {
    let w = World::build_with(true, true, Shape::Review);
    let credential = w.deps.credential().to_string();
    let set_id = w.ledger(move |core| {
        let context = capstan_daemon::deps::new_context(core, &credential)?;
        Ok(core.create_integration_set(
            &context,
            &json!({"branch": "int/set", "repos": ["core", "web"]}),
        )?["setId"]
            .as_str()
            .unwrap()
            .to_string())
    });
    let credential = w.deps.credential().to_string();
    let base = w.base.clone();
    w.ledger(move |core| {
        let kernel = core.kernel();
        capstan_kernel::helpers::execute(
            &kernel.database,
            "INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, created_at)
             VALUES (?, 'i-1', '1', ?, 'int/i-1', 'pm-1', 'running', '2026-01-01T00:00:00.000Z')",
            [&kernel.project_id, &base],
        )?;
        let context = capstan_daemon::deps::new_context(core, &credential)?;
        core.attach_set_member(
            &context,
            &json!({"setId": set_id, "repo": "core", "integrationId": "i-1"}),
        )?;
        capstan_kernel::helpers::execute(
            &kernel.database,
            "UPDATE integrations SET state = 'merged', head_sha = '1111111111111111111111111111111111111111',
             completed_at = '2026-01-01T00:00:01.000Z' WHERE project_id = ? AND integration_id = 'i-1'",
            [&kernel.project_id],
        )?;
        Ok(())
    });
    // (a) the set refusal answers, not verify_missing.
    let refused = w.call("pm-1", "request-review", &["i-1"]);
    let text = text_of(&refused);
    assert!(text.contains("only a merged set can be reviewed"), "{text}");
    assert!(!text.contains("verify_missing"), "{text}");
    // (b) a report is gated per repo.
    let report = w.reported(&[("core", "a.txt"), ("extra", "c.txt")]);
    let gated = w.call("pm-1", "request-review", &[&report]);
    assert!(text_of(&gated).contains("verify_required"), "{gated}");
}
