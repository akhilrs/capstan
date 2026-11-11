#![allow(dead_code)]
//! `cstan request-review` in a workspace project: the per-repo gate, the reviewer placed at each repo's commit, the
//! brief with each repo's range and worktree, the set refusal and the pre-workspace refusal. The kernel, the launcher,
//! the ledger, git and the verify service are real; the host (Herdr) is the launcher tests' stub.
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

// ------------------------------------------------------------------------------------------------ one run per repo

// ------------------------------------------------------------------------------------------------ review

impl World {
    /// Verifies every repo of the report and waits for the runs.
    fn verified(&self, report: &str) {
        for run in self.queued(&[report]) {
            self.wait_run(run["runId"].as_str().unwrap());
        }
    }

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

#[test]
fn request_review_waits_for_every_repos_verify_gate() {
    let w = World::build_with(true, true, Shape::Review);
    let report = w.reported(&[("core", "a.txt"), ("web", "b.txt")]);
    let refused = w.call("pm-1", "request-review", &[&report]);
    assert!(text_of(&refused).contains("verify_required"), "{refused}");
    // One repo verified is not enough.
    let runs = w.queued(&[&report, "--repo", "core"]);
    w.wait_run(runs[0]["runId"].as_str().unwrap());
    let half = w.call("pm-1", "request-review", &[&report]);
    assert_eq!(half["ok"], false, "{half}");
    assert!(text_of(&half).contains("verify_required"), "{half}");
    let web_commit = &w.commits_of(&json!(report))[1].1;
    assert!(text_of(&half).contains(web_commit.as_str()), "{half}");
    w.verified(&report);
    let started = w.call("pm-1", "request-review", &[&report]);
    assert_eq!(started["ok"], true, "{started}");
}

#[test]
fn the_reviewer_starts_at_each_repos_commit_and_the_brief_lists_each_repo() {
    let w = World::build_with(true, true, Shape::Review);
    let report = w.reported(&[("core", "a.txt"), ("web", "b.txt")]);
    w.verified(&report);
    let commits = w.commits_of(&json!(report));
    assert_eq!(commits.len(), 2);
    let started = w.call("pm-1", "request-review", &[&report]);
    assert_eq!(started["ok"], true, "{started}");
    let reviewer = started["result"]["reviewerAgentId"].as_str().unwrap();
    assert_eq!(started["result"]["reviewerReused"], false);
    let task = w.review_task(reviewer);
    let rows = w.rows(reviewer);
    for (repo, commit) in &commits {
        // The reviewer's worktree of the repo is at the repo's commit.
        assert_eq!(&w.head(reviewer, repo), commit, "{repo} worktree");
        let base = w.base(&report_author(&w, &report), repo);
        let tree = w.tree(reviewer, repo);
        assert!(
            task.contains(&format!(
                "Repo {repo}: {base}..{commit}, worktree {}",
                tree.display()
            )),
            "{task}"
        );
        assert!(
            task.contains(&format!("Repo {repo}: Gate passed at {commit}")),
            "{task}"
        );
    }
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(!task.contains("Commit to review:"), "{task}");
}

/// The agent that wrote the report (its worktrees carry the bases).
fn report_author(w: &World, report: &str) -> String {
    let id = report.to_string();
    let controller = w.deps.credential().to_string();
    w.ledger(move |core| core.agent_reports(&controller, Some(1000)))
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["reportId"] == id.as_str())
        .and_then(|r| r["agentId"].as_str())
        .unwrap()
        .to_string()
}

#[test]
fn a_set_that_is_not_merged_is_refused() {
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
    let refused = w.call("pm-1", "request-review", &[&set_id]);
    assert_eq!(refused["ok"], false, "{refused}");
    assert!(
        text_of(&refused).contains("only a merged set can be reviewed"),
        "{refused}"
    );
    assert!(w
        .ledger({
            let set_id = set_id.clone();
            move |core| Ok(core.review_gate(&capstan_kernel::records::GateSubject::Set(set_id)))
        })
        .unwrap_err()
        .contains("only a merged set can be reviewed"));
}

#[test]
fn a_report_from_before_workspace_mode_is_refused() {
    let w = World::build_with(true, true, Shape::Review);
    let agent = w.worker(&["core"]);
    let sha = w.commit(&agent, "core", "a.txt");
    let report = w.accepted(&agent, &[&pair("core", &sha), "done"])["reportId"]
        .as_str()
        .unwrap()
        .to_string();
    // A copy of the accepted report without its report_commits rows, as a ledger from before workspace mode holds it.
    let (from, report) = (report, "r-old".to_string());
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
        Ok(())
    });
    let refused = w.call("pm-1", "request-review", &[&report]);
    assert_eq!(refused["ok"], false, "{refused}");
    assert!(
        text_of(&refused).contains("names no commit per repo"),
        "{refused}"
    );
}

#[test]
fn a_workspace_without_verify_is_not_verify_gated() {
    let w = World::build_with(true, true, Shape::NoVerify);
    let report = w.reported(&[("core", "a.txt"), ("web", "b.txt")]);
    let started = w.call("pm-1", "request-review", &[&report]);
    assert_eq!(started["ok"], true, "{started}");
    let reviewer = started["result"]["reviewerAgentId"].as_str().unwrap();
    // The reviewer still starts at each repo's commit.
    for (repo, commit) in &w.commits_of(&json!(report)) {
        assert_eq!(&w.head(reviewer, repo), commit, "{repo} worktree");
    }
}

#[test]
fn a_workspace_report_skips_warm_reuse_and_gets_a_per_repo_scope() {
    let w = World::build_with(true, true, Shape::Review);
    let first = w.reported(&[("core", "a.txt"), ("web", "b.txt")]);
    w.verified(&first);
    let one = w.call("pm-1", "request-review", &[&first]);
    assert_eq!(one["ok"], true, "{one}");
    let reviewer = one["result"]["reviewerAgentId"]
        .as_str()
        .unwrap()
        .to_string();
    let task = w.review_task(&reviewer);
    assert!(
        task.contains("Scope: all repos, paths written <repo>:<path>: 2 files changed"),
        "{task}"
    );
    assert!(task.contains("\"core:a.txt\""), "{task}");
    assert!(task.contains("\"web:b.txt\""), "{task}");
    // A workspace report is never moved onto a warm reviewer (one moves to one commit of one repo): it gets a fresh one.
    assert_eq!(one["result"]["reviewerReused"], false, "{one}");
}
