#![allow(dead_code)]
//! Set verify and set review (plan-184): a set's members merged by the set loop are verified and reviewed as one subject.
//! The kernel, the launcher, the ledger, git, the integration loop and the verify service are real; the host (Herdr) is
//! the launcher tests' stub.

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
    /// A reviewer role and no `[verify]` section.
    NoVerify,
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
    service: Option<Arc<VerifyService>>,
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
        let require = if matches!(shape, Shape::Review) {
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
            reviewer = if matches!(shape, Shape::Review | Shape::NoVerify) {
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
        ) + &if shape == Shape::NoVerify {
            String::new()
        } else {
            format!(
            "\n[verify]\ncommands = [\"while [ ! -f {t} ]; do sleep 0.1; done; test ! -f fail.txt\"]\nscratch_dir = \"{s}\"\nmax_parallel = 4\n{req}",
            t = top_gate.display(),
            s = scratch.path().join("vscratch").display(),
            req = require,
        )
        };
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
        let role_config_verify = role_config.verify.clone();
        assert_eq!(role_config_verify.is_some(), shape != Shape::NoVerify);
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
            if let Some(config) = role_config_verify.as_mut() {
                config.commands = vec!["true".to_string(); 33];
                config.path_filters = vec![None; 33];
            }
        }
        let service = role_config_verify.map(|config| VerifyService::with_config(&deps, config));
        deps.verify.clone_from(&service);
        if open {
            std::fs::write(&top_gate, "").unwrap();
            std::fs::write(&web_gate, "").unwrap();
        }
        if let Some(service) = &service {
            service.start();
        }
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
        if let Some(service) = &self.service {
            service.stop();
        }
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

impl World {
    /// The review task the reviewer got, from the controller's queued message.
    fn review_task(&self, reviewer: &str) -> String {
        let reviewer = reviewer.to_string();
        self.ledger(move |core| {
            let kernel = core.kernel();
            capstan_kernel::helpers::query_all(
                &kernel.database,
                "SELECT body FROM messages WHERE project_id = ? AND recipient_agent_id = ? AND body LIKE 'Review request%' ORDER BY sequence",
                [&kernel.project_id, &reviewer],
                |row| row.get::<_, String>(0),
            )
        })
        .pop()
        .expect("the reviewer has a task")
    }
}

impl World {
    /// Verifies every repo of the report and waits for the runs.
    fn verified(&self, report: &str) {
        for run in self.queued(&[report]) {
            self.wait_run(run["runId"].as_str().unwrap());
        }
    }

    /// `integrate <reports>` as the operator.
    fn integrate(&self, reports: &[&str]) -> Value {
        let operator = format!("operator-{}", "v".repeat(40));
        self.call(&operator, "integrate", reports)
    }
}

struct Set {
    id: String,
    core: String,
    web: String,
    core_head: String,
    web_head: String,
}

impl World {
    /// A passed review row for a subject (`column` is the review column of its kind).
    fn pass_review(&self, column: &'static str, subject: &str) {
        let subject = subject.to_string();
        self.ledger(move |core| {
            let kernel = core.kernel();
            let reviewer_actor: String = capstan_kernel::helpers::query_opt(
                &kernel.database,
                "SELECT actor_id FROM agents WHERE project_id = ? AND agent_id = 'pm-1'",
                [&kernel.project_id],
                |r| r.get(0),
            )?
            .unwrap();
            // A report has an author; an integration none.
            let author: Option<(String, String)> = if column == "subject_report_id" {
                capstan_kernel::helpers::query_opt(
                    &kernel.database,
                    "SELECT r.agent_id, a.actor_id FROM agent_reports r JOIN agents a ON a.project_id = r.project_id AND a.agent_id = r.agent_id WHERE r.project_id = ? AND r.report_id = ?",
                    [&kernel.project_id, &subject],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?
            } else {
                None
            };
            let (author_agent, author_actor) = match author {
                Some((agent, actor)) => (Some(agent), Some(actor)),
                None => (None, None),
            };
            let sequence: i64 = capstan_kernel::helpers::query_opt(
                &kernel.database,
                "SELECT COALESCE(MAX(sequence), 0) + 1 FROM reviews WHERE project_id = ?",
                [&kernel.project_id],
                |r| r.get(0),
            )?
            .unwrap();
            capstan_kernel::helpers::execute(
                &kernel.database,
                &format!(
                    "INSERT INTO reviews(project_id, review_id, sequence, round, {column}, commit_sha, base_sha, requested_by_actor_id, reviewer_role, reviewer_agent_id, reviewer_actor_id, author_agent_id, author_actor_id, verdict_text, state, created_at)
                     VALUES (?, ?, ?, 1, ?, ?, ?, ?, 'reviewer', 'pm-1', ?, ?, ?, 'ok', 'passed', '2026-01-01T00:00:00.000Z')"
                ),
                rusqlite::params![
                    kernel.project_id,
                    format!("seed-{sequence}"),
                    sequence,
                    subject,
                    "1".repeat(40),
                    "0".repeat(40),
                    reviewer_actor,
                    reviewer_actor,
                    author_agent,
                    author_actor
                ],
            )?;
            Ok(())
        });
    }

    /// A merged two-member set (core, then web), made by the set loop from one verified, reviewed report.
    fn merged_set(&self) -> Set {
        let report = self.reported(&[("core", "a.txt"), ("web", "b.txt")]);
        self.verified(&report);
        self.pass_review("subject_report_id", &report);
        let answer = self.integrate(&[&report]);
        assert_eq!(answer["result"]["state"], "merged", "{answer}");
        let id = answer["result"]["setId"].as_str().unwrap().to_string();
        let set = {
            let id = id.clone();
            self.ledger(move |core| core.integration_set(&id))
        };
        let member = |repo: &str| -> String {
            set["members"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["repo"] == repo)
                .unwrap()["integrationId"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let (core, web) = (member("core"), member("web"));
        let head = |integration: &str| -> String {
            let integration = integration.to_string();
            self.ledger(move |core| core.integration(&integration))["headSha"]
                .as_str()
                .unwrap()
                .to_string()
        };
        Set {
            core_head: head(&core),
            web_head: head(&web),
            id,
            core,
            web,
        }
    }

    /// Settles a member as confirmed with the kernel-internal flag (a passed review is seeded first).
    fn confirm_member(&self, member: &str) {
        self.pass_review("subject_integration_id", member);
        let (member, credential) = (member.to_string(), self.deps.credential().to_string());
        self.ledger(move |core| {
            let context = capstan_daemon::deps::new_context(core, &credential)?;
            core.settle_integration(
                &context,
                &json!({"integrationId": member, "outcome": "confirmed", "member": true}),
            )
        });
    }

    fn reset(&self, set: &str) {
        let (set, credential) = (set.to_string(), self.deps.credential().to_string());
        self.ledger(move |core| {
            let context = capstan_daemon::deps::new_context(core, &credential)?;
            core.reset_set(&context, &json!({"setId": set}))
        });
    }

    fn set_gate(&self, set: &str) -> Value {
        let set = set.to_string();
        self.ledger(move |core| core.set_gate(&set, ""))
    }

    /// The code of one repo in the set gate.
    fn code(&self, set: &str, repo: &str) -> String {
        self.set_gate(set)["members"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["repo"] == repo)
            .unwrap()["code"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Waits for the runs `answer` queued.
    fn wait_runs(&self, answer: &Value) {
        for run in answer["result"]["runs"].as_array().unwrap() {
            self.wait_run(run["runId"].as_str().unwrap());
        }
    }

    fn set_messages(&self, set: &str) -> Vec<String> {
        let like = format!("Verify for set {set}:%");
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

    fn move_main(&self, repo: &str) {
        let dir = self.repo(repo);
        std::fs::write(dir.join("later.txt"), "later\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "--quiet", "-m", "chore: main moves on"]);
    }
}

fn repos_of(answer: &Value) -> Vec<String> {
    answer["result"]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["repo"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn verify_of_a_set_or_a_member_queues_one_run_per_unconfirmed_member() {
    let w = World::build_with(true, true, Shape::Normal);
    let set = w.merged_set();
    let by_set = w.verify(&[&set.id]);
    assert_eq!(by_set["ok"], true, "{by_set}");
    assert_eq!(repos_of(&by_set), ["core", "web"]);
    assert_eq!(by_set["result"]["setId"], set.id.as_str());
    w.wait_runs(&by_set);
    // A member id names the whole set.
    let by_member = w.verify(&[&set.web]);
    assert_eq!(repos_of(&by_member), ["core", "web"], "{by_member}");
    w.wait_runs(&by_member);
    // Each run is the member's own: its integration, its repo, its head.
    let runs = w.ledger({
        let id = set.id.clone();
        move |core| core.subject_runs(&capstan_kernel::records::GateSubject::Set(id))
    });
    assert_eq!(runs.len(), 4, "{runs:?}");
    assert!(
        runs.iter().all(|r| {
            (r["repo"] == "core"
                && r["subjectId"] == set.core.as_str()
                && r["commitSha"] == set.core_head.as_str())
                || (r["repo"] == "web"
                    && r["subjectId"] == set.web.as_str()
                    && r["commitSha"] == set.web_head.as_str())
        }),
        "{runs:?}"
    );
    assert_eq!(w.set_gate(&set.id)["ok"], true);
    // A confirmed member keeps its last passed run: only web is queued.
    w.confirm_member(&set.core);
    let again = w.verify(&[&set.id]);
    assert_eq!(repos_of(&again), ["web"], "{again}");
    w.wait_runs(&again);
    let confirmed = w.verify(&[&set.id, "--repo", "core"]);
    assert_eq!(confirmed["ok"], false, "{confirmed}");
    assert!(text_of(&confirmed).contains("is confirmed"), "{confirmed}");
}

#[test]
fn only_a_merged_set_is_verified() {
    let w = World::build_with(true, true, Shape::Normal);
    let set = w.merged_set();
    let (id, credential) = (set.id.clone(), w.deps.credential().to_string());
    w.ledger(move |core| {
        let context = capstan_daemon::deps::new_context(core, &credential)?;
        core.set_integration_state(
            &context,
            &json!({"setId": id, "state": "failed", "reason": "test"}),
        )
    });
    let refused = w.verify(&[&set.id]);
    assert_eq!(refused["ok"], false, "{refused}");
    assert!(
        text_of(&refused).contains("only a merged set is verified"),
        "{refused}"
    );
}

#[test]
fn a_cancelled_member_run_fails_the_set_gate_and_repo_reruns_it() {
    let w = World::held();
    let set = w.merged_set_held();
    let queued = w.verify(&[&set.id]);
    assert_eq!(queued["ok"], true, "{queued}");
    let web = queued["result"]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["repo"] == "web")
        .unwrap()["runId"]
        .as_str()
        .unwrap()
        .to_string();
    let cancelled = w.verify(&["cancel", &web]);
    assert_eq!(cancelled["ok"], true, "{cancelled}");
    w.release();
    w.wait_runs(&queued);
    assert_eq!(w.code(&set.id, "core"), "passed");
    assert_eq!(w.code(&set.id, "web"), "verify_failed");
    assert_eq!(w.set_gate(&set.id)["ok"], false);
    let rerun = w.verify(&[&set.id, "--repo", "web"]);
    assert_eq!(repos_of(&rerun), ["web"], "{rerun}");
    w.wait_runs(&rerun);
    assert_eq!(w.set_gate(&set.id)["ok"], true);
}

impl World {
    /// `merged_set` for a world whose gate commands wait: the report is verified with the gate open for a moment, then the
    /// gate is closed again for the set's runs.
    fn merged_set_held(&self) -> Set {
        self.release();
        let set = self.merged_set();
        // The report's runs are final; the set's own runs have not started. Close the gate files again.
        std::fs::remove_file(&self.top_gate).unwrap();
        std::fs::remove_file(&self.web_gate).unwrap();
        set
    }
}

#[test]
fn a_confirmed_member_counts_its_older_passed_run_after_a_reset_and_after_main_moved() {
    let w = World::build_with(true, true, Shape::Normal);
    let set = w.merged_set();
    w.wait_runs(&w.verify(&[&set.id]));
    assert_eq!(w.set_gate(&set.id)["ok"], true);
    w.confirm_member(&set.core);
    w.move_main("core");
    w.reset(&set.id);
    // The reset drops web's run, not the confirmed core member's.
    assert_eq!(w.code(&set.id, "core"), "passed");
    assert_eq!(w.code(&set.id, "web"), "verify_missing");
    assert_eq!(w.set_gate(&set.id)["ok"], false);
    let runs = w.ledger({
        let id = set.id.clone();
        move |core| core.subject_runs(&capstan_kernel::records::GateSubject::Set(id))
    });
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(runs[0]["repo"], "core");
    let again = w.verify(&[&set.id]);
    assert_eq!(repos_of(&again), ["web"]);
    w.wait_runs(&again);
    assert_eq!(w.set_gate(&set.id)["ok"], true);
}

#[test]
fn request_review_waits_for_merged_and_passed_and_records_every_member_head() {
    let w = World::build_with(true, true, Shape::Review);
    let set = w.merged_set();
    // A member run exists from the report only under the report; the set's members are not verified yet.
    let early = w.call("pm-1", "request-review", &[&set.id]);
    assert_eq!(early["ok"], false, "{early}");
    assert!(text_of(&early).contains("verify_required"), "{early}");
    w.wait_runs(&w.verify(&[&set.id]));
    let started = w.call("pm-1", "request-review", &[&set.web]);
    assert_eq!(started["ok"], true, "{started}");
    let reviewer = started["result"]["reviewerAgentId"].as_str().unwrap();
    assert_eq!(w.head(reviewer, "core"), set.core_head);
    assert_eq!(w.head(reviewer, "web"), set.web_head);
    let task = w.review_task(reviewer);
    assert!(task.contains(&format!("for set {}", set.id)), "{task}");
    assert!(task.contains("Repo core: "), "{task}");
    assert!(task.contains("Repo web: "), "{task}");
    // The review is of the first member by position.
    let id = started["result"]["reviewId"].as_str().unwrap().to_string();
    let subject = w.ledger(move |core| {
        let kernel = core.kernel();
        capstan_kernel::helpers::query_opt(
            &kernel.database,
            "SELECT subject_integration_id FROM reviews WHERE project_id = ? AND review_id = ?",
            [&kernel.project_id, &id],
            |r| r.get::<_, String>(0),
        )
    });
    assert_eq!(subject.as_deref(), Some(set.core.as_str()));
}

#[test]
fn a_failed_set_with_a_merged_member_cannot_be_reviewed() {
    let w = World::build_with(true, true, Shape::Review);
    let set = w.merged_set();
    w.wait_runs(&w.verify(&[&set.id]));
    let (id, credential) = (set.id.clone(), w.deps.credential().to_string());
    w.ledger(move |core| {
        let context = capstan_daemon::deps::new_context(core, &credential)?;
        core.set_integration_state(
            &context,
            &json!({"setId": id, "state": "failed", "reason": "test"}),
        )
    });
    let refused = w.call("pm-1", "request-review", &[&set.id]);
    assert_eq!(refused["ok"], false, "{refused}");
    assert!(
        text_of(&refused).contains("only a merged set can be reviewed"),
        "{refused}"
    );
}

#[test]
fn a_confirmed_first_member_stays_the_subject() {
    let w = World::build_with(true, true, Shape::Review);
    let set = w.merged_set();
    w.wait_runs(&w.verify(&[&set.id]));
    w.confirm_member(&set.core);
    let started = w.call("pm-1", "request-review", &[&set.id]);
    assert_eq!(started["ok"], true, "{started}");
    let reviewer = started["result"]["reviewerAgentId"].as_str().unwrap();
    assert_eq!(w.head(reviewer, "core"), set.core_head);
    assert_eq!(w.head(reviewer, "web"), set.web_head);
    let id = started["result"]["reviewId"].as_str().unwrap().to_string();
    let subject = w.ledger(move |core| {
        let kernel = core.kernel();
        capstan_kernel::helpers::query_opt(
            &kernel.database,
            "SELECT subject_integration_id FROM reviews WHERE project_id = ? AND review_id = ?",
            [&kernel.project_id, &id],
            |r| r.get::<_, String>(0),
        )
    });
    assert_eq!(subject.as_deref(), Some(set.core.as_str()));
}

#[test]
fn a_restart_mid_set_verify_gives_one_message() {
    let w = World::held();
    let set = w.merged_set_held();
    let queued = w.verify(&[&set.id]);
    assert_eq!(queued["ok"], true, "{queued}");
    // Both runs are claimed (they wait on the gate files).
    let end = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let running = queued["result"]["runs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| w.run(r["runId"].as_str().unwrap())["state"] == "running");
        if running {
            break;
        }
        assert!(std::time::Instant::now() < end, "runs did not start");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(w.set_messages(&set.id).is_empty());
    w.service.as_ref().unwrap().stop();
    for run in queued["result"]["runs"].as_array().unwrap() {
        assert_eq!(
            w.run(run["runId"].as_str().unwrap())["state"],
            "interrupted"
        );
    }
    let restarted =
        VerifyService::with_config(&w.deps, w.service.as_ref().unwrap().config().clone());
    restarted.recover();
    let messages = w.set_messages(&set.id);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("interrupted"), "{}", messages[0]);
}

#[test]
fn settling_a_member_needs_the_kernel_flag() {
    let w = World::build_with(true, true, Shape::Normal);
    let set = w.merged_set();
    w.pass_review("subject_integration_id", &set.core);
    let (member, credential) = (set.core.clone(), w.deps.credential().to_string());
    let err = w.deps.kernel.run(move |core| {
        let context = capstan_daemon::deps::new_context(core, &credential)?;
        core.settle_integration(
            &context,
            &json!({"integrationId": member, "outcome": "confirmed"}),
        )
    });
    assert!(err.unwrap_err().to_string().contains("belongs to set"));
    // No frame can set the flag: the extra argument is refused like any other and nothing is settled.
    let operator = format!("operator-{}", "v".repeat(40));
    let member_arg = w.call(&operator, "integrate", &["confirm", &set.core, "member"]);
    let other_arg = w.call(&operator, "integrate", &["confirm", &set.core, "bogus"]);
    assert_eq!(member_arg["ok"], false, "{member_arg}");
    assert_eq!(member_arg, other_arg);
    let integration = {
        let member = set.core.clone();
        w.ledger(move |core| core.integration(&member))
    };
    assert_eq!(integration["state"], "merged");
    // The flag on an integration that is not a set member is refused.
    let lone = {
        let base = w.base.clone();
        w.ledger(move |core| {
            let kernel = core.kernel();
            capstan_kernel::helpers::execute(
                &kernel.database,
                "INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, head_sha, created_at, completed_at)
                 VALUES (?, 'lone-1', 99, ?, 'int/lone', 'pm-1', 'merged', ?, '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:01.000Z')",
                [&kernel.project_id, &base, &"2".repeat(40)],
            )?;
            Ok("lone-1".to_string())
        })
    };
    w.pass_review("subject_integration_id", &lone);
    let credential = w.deps.credential().to_string();
    let err = w.deps.kernel.run(move |core| {
        let context = capstan_daemon::deps::new_context(core, &credential)?;
        core.settle_integration(
            &context,
            &json!({"integrationId": "lone-1", "outcome": "confirmed", "member": true}),
        )
    });
    assert!(err
        .unwrap_err()
        .to_string()
        .contains("integration lone-1 is not a set member"));
}
