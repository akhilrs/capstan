#![allow(dead_code)]
//! Confirm per repo and signoff over sets (plan-184): `cstan integrate confirm <set> --repo <repo>`, `plan signoff`.
//! The kernel, the launcher, the ledger, git, the integration loop and the verify service are real; the host (Herdr) is
//! the launcher tests' stub.

mod common;

use capstan_config::TrustStore;
use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::handlers::handle_frame;
use capstan_daemon::handlers::integrate_seams::{install_integrate_seams, IntegrateSeams};
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

    /// `merged_set` without the verification and review of the report (for a world whose gates are held).
    fn merged_set_unverified(&self) -> Set {
        let report = self.reported(&[("core", "a.txt"), ("web", "b.txt")]);
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

// ------------------------------------------------------------------------------------------------- redo

impl World {
    fn operator(&self) -> String {
        format!("operator-{}", "v".repeat(40))
    }

    fn redo(&self, set: &str, repo: &str) -> Value {
        self.call(
            &self.operator(),
            "integrate",
            &["redo", set, "--repo", repo],
        )
    }

    fn redone(&self, set: &str, repo: &str) -> Value {
        let answer = self.redo(set, repo);
        assert_eq!(answer["ok"], true, "redo {repo}: {answer}");
        answer["result"].clone()
    }

    fn set_of(&self, set: &str) -> Value {
        let set = set.to_string();
        self.ledger(move |core| core.integration_set(&set))
    }

    fn member(&self, set: &str, repo: &str) -> Option<Value> {
        self.set_of(set)["members"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["repo"] == repo)
            .cloned()
    }

    fn integration(&self, id: &str) -> Value {
        let id = id.to_string();
        self.ledger(move |core| core.integration(&id))
    }

    fn member_integration(&self, set: &str, repo: &str) -> Value {
        let id = self.member(set, repo).unwrap()["integrationId"]
            .as_str()
            .unwrap()
            .to_string();
        self.integration(&id)
    }

    fn main_tip(&self, repo: &str) -> String {
        git(&self.repo(repo), &["rev-parse", "main"])
    }

    /// Puts `sha` on the main branch of `repo` as a user's merge would.
    fn land(&self, repo: &str, sha: &str) {
        git(&self.repo(repo), &["update-ref", "refs/heads/main", sha]);
    }

    fn install(&self, seams: IntegrateSeams) {
        install_integrate_seams(&self.deps, seams);
    }

    /// A commit of `agent` in `repo` writing `content` into `file`; its id.
    fn commit_text(&self, agent: &str, repo: &str, file: &str, content: &str) -> String {
        let tree = self.tree(agent, repo);
        std::fs::write(tree.join(file), content).unwrap();
        git(&tree, &["add", "-A"]);
        git(
            &tree,
            &["commit", "--quiet", "-m", &format!("feat: write {file}")],
        );
        git(&tree, &["rev-parse", "HEAD"])
    }

    fn report_of(&self, agent: &str, pairs: &[(&str, String)]) -> String {
        let pairs: Vec<String> = pairs.iter().map(|(r, s)| pair(r, s)).collect();
        let mut args: Vec<&str> = pairs.iter().map(String::as_str).collect();
        args.push("done");
        self.accepted(agent, &args)["reportId"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// A review row in state `started` for an integration (a review under way).
    fn start_review(&self, subject: &str) {
        let subject = subject.to_string();
        self.ledger(move |core| {
            let kernel = core.kernel();
            let actor: String = capstan_kernel::helpers::query_opt(
                &kernel.database,
                "SELECT actor_id FROM agents WHERE project_id = ? AND agent_id = 'pm-1'",
                [&kernel.project_id],
                |r| r.get(0),
            )?
            .unwrap();
            capstan_kernel::helpers::execute(
                &kernel.database,
                "INSERT INTO reviews(project_id, review_id, sequence, round, subject_integration_id, commit_sha, base_sha, requested_by_actor_id, reviewer_role, reviewer_agent_id, reviewer_actor_id, state, created_at)
                 VALUES (?, 'open-review', (SELECT COALESCE(MAX(sequence), 0) + 1 FROM reviews WHERE project_id = ?), 2, ?, ?, ?, ?, 'reviewer', 'pm-1', ?, 'started', '2026-01-01T00:00:00.000Z')",
                rusqlite::params![kernel.project_id, kernel.project_id, subject, "1".repeat(40), "0".repeat(40), actor, actor],
            )?;
            Ok(())
        });
    }

    /// A set whose web member is conflicted: agents A and D both write `w.txt` in web; B (core only) also reported. Core
    /// merges. Returns `(set id, A, D, B)`.
    fn conflicted_set(&self) -> (String, String, String, String) {
        let a = self.worker(&["core", "web"]);
        let d = self.worker(&["core", "web"]);
        let b = self.worker(&["core"]);
        let a_core = self.commit_text(&a, "core", "a.txt", "A\n");
        let a_web = self.commit_text(&a, "web", "w.txt", "A\n");
        let ra = self.report_of(&a, &[("core", a_core), ("web", a_web)]);
        let d_core = self.commit_text(&d, "core", "d.txt", "D\n");
        let d_web = self.commit_text(&d, "web", "w.txt", "D\n");
        let rd = self.report_of(&d, &[("core", d_core), ("web", d_web)]);
        let b_core = self.commit_text(&b, "core", "b.txt", "B\n");
        let rb = self.report_of(&b, &[("core", b_core)]);
        for report in [&ra, &rd, &rb] {
            self.verified(report);
            self.pass_review("subject_report_id", report);
        }
        let answer = self.integrate(&[&ra, &rd, &rb]);
        assert_eq!(answer["ok"], true, "{answer}");
        assert_eq!(answer["result"]["state"], "conflicted", "{answer}");
        (
            answer["result"]["setId"].as_str().unwrap().to_string(),
            a,
            d,
            b,
        )
    }
}

impl World {
    fn discard(&self, args: &[&str]) -> Value {
        let mut all = vec!["discard"];
        all.extend_from_slice(args);
        self.call(&self.operator(), "integrate", &all)
    }

    fn discarded(&self, args: &[&str]) -> Value {
        let answer = self.discard(args);
        assert_eq!(answer["ok"], true, "discard {args:?}: {answer}");
        answer["result"].clone()
    }

    fn branch_exists(&self, repo: &str, branch: &str) -> bool {
        !git(&self.repo(repo), &["branch", "--list", branch]).is_empty()
    }
}
// ------------------------------------------------------------------------------------------------- confirm

impl World {
    fn confirm(&self, args: &[&str]) -> Value {
        let mut all = vec!["confirm"];
        all.extend_from_slice(args);
        self.call(&self.operator(), "integrate", &all)
    }

    /// `integrate confirm <set> --repo <repo>` that must be accepted.
    fn confirmed(&self, set: &str, repo: &str) -> Value {
        let answer = self.confirm(&[set, "--repo", repo]);
        assert_eq!(answer["ok"], true, "confirm {repo}: {answer}");
        answer["result"].clone()
    }

    /// A passed review of the set (its subject is the first live member), then `confirmed`.
    fn reviewed(&self, set: &Set) {
        self.pass_review("subject_integration_id", &set.core);
    }

    fn set_confirmed(&self, set: &str) -> bool {
        let set = set.to_string();
        self.ledger(move |core| core.set_confirmed(&set))
    }

    /// Lands the member head as the user would with `--squash`: one new commit with its changes.
    fn land_squash(&self, repo: &str, sha: &str) {
        let dir = self.repo(repo);
        git(&dir, &["merge", "--squash", sha]);
        git(&dir, &["commit", "--quiet", "-m", "feat: squashed"]);
    }

    /// Lands the member head as a rebase merge would: a new commit with the same change.
    fn land_rebased(&self, repo: &str, sha: &str) {
        git(&self.repo(repo), &["cherry-pick", sha]);
    }
}

#[test]
fn confirm_goes_per_repo_and_the_set_is_confirmed_when_every_member_is() {
    let world = World::new();
    let set = world.merged_set();
    world.reviewed(&set);
    let core_branch = world.integration(&set.core)["branch"]
        .as_str()
        .unwrap()
        .to_string();
    world.land("core", &set.core_head);
    let done = world.confirmed(&set.id, "core");
    assert_eq!(done["state"], "confirmed", "{done}");
    assert_eq!(done["repo"], "core", "{done}");
    assert_eq!(done["warning"], Value::Null, "{done}");
    assert_eq!(done["branchRemoved"], true, "{done}");
    assert!(!world.branch_exists("core", &core_branch));
    assert_eq!(world.integration(&set.web)["state"], "merged");
    assert!(!world.set_confirmed(&set.id));
    world.land("web", &set.web_head);
    world.confirmed(&set.id, "web");
    assert_eq!(world.integration(&set.web)["state"], "confirmed");
    assert!(world.set_confirmed(&set.id));
    let (code, text) = refusal(&world.confirm(&[&set.id, "--repo", "web"]));
    assert_eq!(code, "workspace_integrate");
    assert!(text.contains("already confirmed"), "{text}");
}

#[test]
fn a_member_merged_by_hand_out_of_order_is_confirmed_with_a_warning() {
    let world = World::new();
    let set = world.merged_set();
    world.reviewed(&set);
    world.land("web", &set.web_head);
    let done = world.confirmed(&set.id, "web");
    assert_eq!(
        done["warning"], "repo web confirmed before core (merge_order)",
        "{done}"
    );
    assert_eq!(world.integration(&set.web)["state"], "confirmed");
    assert_eq!(world.integration(&set.core)["state"], "merged");
}

#[test]
fn confirm_without_the_repo_asks_for_it_and_unknown_things_are_refused() {
    let world = World::new();
    let set = world.merged_set();
    world.reviewed(&set);
    let (code, text) = refusal(&world.confirm(&[&set.id]));
    assert_eq!(code, "workspace_integrate");
    assert!(text.contains("name the repo"), "{text}");
    assert!(text.contains("core") && text.contains("web"), "{text}");
    let (_, text) = refusal(&world.confirm(&[&set.id, "--repo", "extra"]));
    assert!(text.contains("has no member for repo extra"), "{text}");
    // An integration id still goes the old way, which a member refuses.
    let (_, text) = refusal(&world.confirm(&[&set.core]));
    assert!(text.contains("belongs to set"), "{text}");
    assert_eq!(world.integration(&set.core)["state"], "merged");
}

#[test]
fn confirm_is_refused_while_main_lacks_the_member_or_changed_one_of_its_paths() {
    let world = World::new();
    let set = world.merged_set();
    world.reviewed(&set);
    let branch = world.integration(&set.core)["branch"]
        .as_str()
        .unwrap()
        .to_string();
    let (code, text) = refusal(&world.confirm(&[&set.id, "--repo", "core"]));
    assert_eq!(code, "workspace_integrate");
    assert_eq!(
        text,
        format!("repo core main does not contain {branch}"),
        "{text}"
    );
    // It landed, then main changed one of its files.
    world.land_squash("core", &set.core_head);
    let dir = world.repo("core");
    std::fs::write(dir.join("a.txt"), "changed later\n").unwrap();
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "--quiet", "-m", "fix: change a.txt"]);
    let (_, text) = refusal(&world.confirm(&[&set.id, "--repo", "core"]));
    assert!(text.contains("main does not contain"), "{text}");
    assert_eq!(world.integration(&set.core)["state"], "merged");
}

#[test]
fn confirm_accepts_a_squash_a_rebase_and_a_main_that_moved_on_in_other_paths() {
    let world = World::new();
    let set = world.merged_set();
    world.reviewed(&set);
    world.land_squash("core", &set.core_head);
    world.move_main("core");
    world.confirmed(&set.id, "core");
    world.land_rebased("web", &set.web_head);
    world.move_main("web");
    world.confirmed(&set.id, "web");
    assert!(world.set_confirmed(&set.id));
}

#[test]
fn confirm_needs_the_set_review_passed_and_none_open() {
    let world = World::new();
    let set = world.merged_set();
    world.land("core", &set.core_head);
    let (_, text) = refusal(&world.confirm(&[&set.id, "--repo", "core"]));
    assert!(text.contains("latest review passed"), "{text}");
    world.reviewed(&set);
    world.start_review(&set.web);
    let (_, text) = refusal(&world.confirm(&[&set.id, "--repo", "core"]));
    assert!(
        text.contains("a review of set") && text.contains("is open"),
        "{text}"
    );
}

#[test]
fn confirm_of_a_member_not_merged_is_refused() {
    let world = World::new();
    let set = world.merged_set();
    world.reviewed(&set);
    world.discarded(&[&set.id, "--repo", "web"]);
    let (_, text) = refusal(&world.confirm(&[&set.id, "--repo", "web"]));
    assert!(text.contains("discarded"), "{text}");
}

#[test]
fn a_dropped_member_does_not_block_the_set_and_a_row_only_member_does() {
    let world = World::new();
    let set = world.merged_set();
    world.reviewed(&set);
    world.discarded(&[&set.id, "--repo", "web"]);
    assert!(!world.set_confirmed(&set.id));
    world.land("core", &set.core_head);
    world.confirmed(&set.id, "core");
    assert!(world.set_confirmed(&set.id));
    // A member with a row and no integration blocks.
    let id = set.id.clone();
    world.ledger(move |core| {
        let kernel = core.kernel();
        capstan_kernel::helpers::execute(
            &kernel.database,
            "INSERT INTO integration_set_members(project_id, set_id, repo, position, integration_id, attempt) VALUES (?, ?, 'extra', 9, NULL, 1)",
            [&kernel.project_id, &id],
        )?;
        Ok(())
    });
    assert!(!world.set_confirmed(&set.id));
}

#[test]
fn a_single_member_set_is_confirmed_by_its_one_member() {
    let world = World::new();
    let report = world.reported(&[("core", "a.txt")]);
    world.verified(&report);
    world.pass_review("subject_report_id", &report);
    let answer = world.integrate(&[&report]);
    assert_eq!(answer["result"]["state"], "merged", "{answer}");
    let id = answer["result"]["setId"].as_str().unwrap().to_string();
    let member = world.member(&id, "core").unwrap()["integrationId"]
        .as_str()
        .unwrap()
        .to_string();
    world.pass_review("subject_integration_id", &member);
    let head = world.integration(&member)["headSha"]
        .as_str()
        .unwrap()
        .to_string();
    world.land("core", &head);
    world.confirmed(&id, "core");
    assert!(world.set_confirmed(&id));
}

// ------------------------------------------------------------------------------------------------- signoff

const LONG_AGO: &str = "2000-01-01T00:00:00.000Z";

/// A plan with one package `a` assigned to `reporter`, and its architect.
struct Plan {
    id: String,
    architect: String,
    reporter: String,
}

impl World {
    /// An approved plan inserted into the ledger: package `a` of `reporter`, architect a second worker.
    fn plan_for(&self, reporter: &str) -> Plan {
        let architect = self.worker(&[]);
        let (reporter_id, arch) = (reporter.to_string(), architect.clone());
        let body = serde_json::to_string(
            &capstan_kernel::plan_body::parse_plan_body(
                r#"{"summary":"s","packages":[{"id":"a","title":"T a","owns":["src/a"],"acceptance":["x"],"estimate_hours":1}],"risks":[],"integration_order":["a"]}"#,
                8,
            )
            .unwrap(),
        )
        .unwrap();
        self.ledger(move |core| {
            let kernel = core.kernel();
            let p = &kernel.project_id;
            capstan_kernel::helpers::execute(
                &kernel.database,
                "INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, architect_agent_id, current_revision, created_at, updated_at)
                 VALUES (?, 'plan-900', 900, 'T', 'normal', 'draft', 'x', ?, 0, ?, ?)",
                rusqlite::params![p, arch, LONG_AGO, LONG_AGO],
            )?;
            capstan_kernel::helpers::execute(
                &kernel.database,
                "INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at)
                 VALUES (?, 'plan-900', 1, ?, ?, ?, ?, (SELECT actor_id FROM agents WHERE agent_id = ?), ?)",
                rusqlite::params![p, "0".repeat(40), body, "c".repeat(64), arch, arch, LONG_AGO],
            )?;
            capstan_kernel::helpers::execute(
                &kernel.database,
                "UPDATE plans SET current_revision = 1, state = 'approved', approved_revision = 1 WHERE plan_id = 'plan-900'",
                rusqlite::params![],
            )?;
            capstan_kernel::helpers::execute(
                &kernel.database,
                "INSERT INTO plan_packages(project_id, plan_id, package_id, assignee_agent_id, assigned_at) VALUES (?, 'plan-900', 'a', ?, ?)",
                rusqlite::params![p, reporter_id, LONG_AGO],
            )?;
            Ok(())
        });
        Plan {
            id: "plan-900".into(),
            architect,
            reporter: reporter.to_string(),
        }
    }

    /// The architect signs the plan off against `target` (an integration or a set id).
    fn signoff(&self, plan: &Plan, target: &str) -> Result<Value, String> {
        let credential = self.tokens.lock().unwrap()[&plan.architect].clone();
        let (plan_id, target) = (plan.id.clone(), target.to_string());
        self.deps
            .kernel
            .run(move |core| {
                let context = capstan_daemon::deps::new_context(core, &credential)?;
                Ok(core.record_signoff(
                    &context,
                    &json!({"planId": plan_id, "integrationId": target, "summary": "done"}),
                ))
            })
            .expect("the ledger answers")
            .map_err(|error| error.message().to_string())
    }

    /// A merged set of one report of a worker `a` (core and web), reviewed; the worker is the plan's package assignee.
    fn planned_set(&self) -> (Set, Plan) {
        let agent = self.worker(&["core", "web"]);
        let core = self.commit(&agent, "core", "a.txt");
        let web = self.commit(&agent, "web", "b.txt");
        let args = [pair("core", &core), pair("web", &web), "done".to_string()];
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let report = self.accepted(&agent, &refs)["reportId"]
            .as_str()
            .unwrap()
            .to_string();
        self.verified(&report);
        self.pass_review("subject_report_id", &report);
        let answer = self.integrate(&[&report]);
        assert_eq!(answer["result"]["state"], "merged", "{answer}");
        let id = answer["result"]["setId"].as_str().unwrap().to_string();
        let set = self.set_of(&id);
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
        let (c, w) = (member("core"), member("web"));
        let set = Set {
            core_head: self.integration(&c)["headSha"]
                .as_str()
                .unwrap()
                .to_string(),
            web_head: self.integration(&w)["headSha"]
                .as_str()
                .unwrap()
                .to_string(),
            id,
            core: c,
            web: w,
        };
        self.pass_review("subject_integration_id", &set.core);
        let plan = self.plan_for(&agent);
        (set, plan)
    }
}

#[test]
fn signoff_waits_for_every_member_confirmed() {
    let world = World::new();
    let (set, plan) = world.planned_set();
    let text = world.signoff(&plan, &set.core).unwrap_err();
    assert!(
        text.contains("is not confirmed") && text.contains("core"),
        "{text}"
    );
    world.land("core", &set.core_head);
    world.confirmed(&set.id, "core");
    let text = world.signoff(&plan, &set.web).unwrap_err();
    assert!(text.contains("repo web is not confirmed yet"), "{text}");
    world.land("web", &set.web_head);
    world.confirmed(&set.id, "web");
    let signed = world
        .signoff(&plan, &set.web)
        .expect("every member is confirmed");
    assert_eq!(signed["integrationId"], set.web.as_str(), "{signed}");
}

#[test]
fn signoff_by_the_set_id_names_the_review_subject() {
    let world = World::new();
    let (set, plan) = world.planned_set();
    for (repo, head) in [("core", &set.core_head), ("web", &set.web_head)] {
        world.land(repo, head);
        world.confirmed(&set.id, repo);
    }
    let signed = world.signoff(&plan, &set.id).expect("the set signs off");
    assert_eq!(signed["integrationId"], set.core.as_str(), "{signed}");
    assert!(world.signoff(&plan, &set.id).is_err());
}

#[test]
fn signoff_skips_a_dropped_member() {
    let world = World::new();
    let (set, plan) = world.planned_set();
    world.discarded(&[&set.id, "--repo", "web"]);
    world.land("core", &set.core_head);
    world.confirmed(&set.id, "core");
    world
        .signoff(&plan, &set.core)
        .expect("the dropped member does not block");
}

#[test]
fn signoff_is_blocked_by_a_row_only_member() {
    let world = World::new();
    let (set, plan) = world.planned_set();
    world.discarded(&[&set.id, "--repo", "web"]);
    world.land("core", &set.core_head);
    world.confirmed(&set.id, "core");
    let id = set.id.clone();
    world.ledger(move |core| {
        let kernel = core.kernel();
        capstan_kernel::helpers::execute(
            &kernel.database,
            "INSERT INTO integration_set_members(project_id, set_id, repo, position, integration_id, attempt) VALUES (?, ?, 'extra', 9, NULL, 1)",
            [&kernel.project_id, &id],
        )?;
        Ok(())
    });
    let text = world.signoff(&plan, &set.core).unwrap_err();
    assert!(text.contains("repo extra is not confirmed yet"), "{text}");
}

#[test]
fn a_package_repo_without_a_report_does_not_block_signoff() {
    // The plan's package names repos core and web; only core has a report, so the set has one member.
    let world = World::new();
    let agent = world.worker(&[]);
    let sha = world.commit(&agent, "core", "a.txt");
    let report = world.accepted(&agent, &[&pair("core", &sha), "done"])["reportId"]
        .as_str()
        .unwrap()
        .to_string();
    world.verified(&report);
    world.pass_review("subject_report_id", &report);
    let answer = world.integrate(&[&report]);
    assert_eq!(answer["result"]["state"], "merged", "{answer}");
    let id = answer["result"]["setId"].as_str().unwrap().to_string();
    let member = world.member(&id, "core").unwrap()["integrationId"]
        .as_str()
        .unwrap()
        .to_string();
    world.pass_review("subject_integration_id", &member);
    let head = world.integration(&member)["headSha"]
        .as_str()
        .unwrap()
        .to_string();
    let plan = world.plan_for(&agent);
    world.land("core", &head);
    world.confirmed(&id, "core");
    world
        .signoff(&plan, &member)
        .expect("a single-member set signs off");
}

// ------------------------------------------------------------------------------------------------- cleanup

#[test]
fn the_confirm_time_offer_of_a_member_names_its_repo() {
    let world = World::new();
    let set = world.merged_set();
    world.reviewed(&set);
    world.land("web", &set.web_head);
    // A spare branch whose change main holds, in web only.
    git(&world.repo("web"), &["branch", "spare-web", &set.web_head]);
    let done = world.confirmed(&set.id, "web");
    let cleanup = done["cleanup"].clone();
    let id = cleanup["cleanupId"]
        .as_str()
        .expect("an offer was made")
        .to_string();
    let repo = world.ledger(move |core| core.cleanup_repo(&id));
    assert_eq!(repo, "web");
    assert_eq!(cleanup["offered"], 1, "{cleanup}");
}

#[test]
fn a_repo_whose_change_is_on_main_already_has_no_member_to_confirm() {
    let world = World::new();
    let agent = world.worker(&["core", "web"]);
    let core = world.commit(&agent, "core", "a.txt");
    let web = world.commit(&agent, "web", "b.txt");
    let report = world.accepted(&agent, &[&pair("core", &core), &pair("web", &web), "done"])
        ["reportId"]
        .as_str()
        .unwrap()
        .to_string();
    world.verified(&report);
    world.pass_review("subject_report_id", &report);
    // Core landed by hand before the integration.
    world.land("core", &core);
    let answer = world.integrate(&[&report]);
    assert_eq!(answer["result"]["landed"], json!(["core"]), "{answer}");
    let id = answer["result"]["setId"].as_str().unwrap().to_string();
    assert!(world.member(&id, "core").is_none());
    let (_, text) = refusal(&world.confirm(&[&id, "--repo", "core"]));
    assert!(text.contains("has no member for repo core"), "{text}");
}

#[test]
fn confirm_takes_the_repo_flag_before_the_id() {
    let world = World::new();
    let set = world.merged_set();
    world.reviewed(&set);
    world.land("core", &set.core_head);
    let answer = world.confirm(&["--repo", "core", &set.id]);
    assert_eq!(answer["ok"], true, "{answer}");
}

#[test]
fn signoff_of_a_set_with_no_live_member_is_refused() {
    let world = World::new();
    let (set, plan) = world.planned_set();
    world.discarded(&[&set.id]);
    for target in [set.id.as_str(), set.core.as_str()] {
        let text = world.signoff(&plan, target).unwrap_err();
        assert!(text.contains("has no live member; discard it"), "{text}");
    }
}

#[test]
fn the_warning_names_an_earlier_member_that_has_no_integration() {
    let world = World::new();
    let set = world.merged_set();
    world.reviewed(&set);
    let id = set.id.clone();
    world.ledger(move |core| {
        let kernel = core.kernel();
        capstan_kernel::helpers::execute(
            &kernel.database,
            "INSERT INTO integration_set_members(project_id, set_id, repo, position, integration_id, attempt) VALUES (?, ?, 'extra', 5, NULL, 1)",
            [&kernel.project_id, &id],
        )?;
        capstan_kernel::helpers::execute(
            &kernel.database,
            "UPDATE integration_set_members SET position = 9 WHERE project_id = ? AND set_id = ? AND repo = 'web'",
            [&kernel.project_id, &id],
        )?;
        Ok(())
    });
    world.land("core", &set.core_head);
    world.confirmed(&set.id, "core");
    world.land("web", &set.web_head);
    let done = world.confirmed(&set.id, "web");
    assert_eq!(
        done["warning"], "repo web confirmed before extra (merge_order)",
        "{done}"
    );
}
