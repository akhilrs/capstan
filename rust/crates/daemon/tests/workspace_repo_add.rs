//! `cstan repo add` through the command layer: a worker adds a repo to its change set. The kernel, the launcher, the
//! ledger and git are real; the host (Herdr) is the launcher tests' stub. The project root is itself a git repository
//! that no `[repos]` entry lists, so a boot needs no ports change.

mod common;

use capstan_config::TrustStore;
use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::handlers::handle_frame;
use capstan_daemon::handlers::shared::AbortSignal;
use capstan_daemon::handlers::{MAX_RESPONSE_BYTES, RUST_ONLY_ROUTES};
use capstan_daemon::ports::ProjectGit;
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
}

impl World {
    fn new() -> Arc<World> {
        World::build(true)
    }

    /// `workspace` false writes a configuration with no `[repos]` and no `[workspace]`.
    fn build(workspace: bool) -> Arc<World> {
        let dir = private_tempdir();
        let scratch = private_tempdir();
        let repos = scratch.path().join("repos");
        let changes = scratch.path().join("changes");
        let base = make_repo(&repos.join("core"), "main");
        make_repo(&repos.join("web"), "main");
        make_repo(&repos.join("extra"), "main");
        make_repo(&repos.join("bad"), "main");
        make_repo(&repos.join("nomain"), "trunk");
        let identity = Identity {
            project_id: "prepo".into(),
            name: "Repo Add".into(),
            credential: format!("operator-{}", "r".repeat(40)),
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

{ws}"#,
            ws = if workspace {
                format!(
                    r#"[repos.core]
path = "{r}/core"
main_branch = "main"

[repos.web]
path = "{r}/web"
main_branch = "main"

[repos.extra]
path = "{r}/extra"
main_branch = "main"

[repos.bad]
path = "{r}/bad"
main_branch = "main"
setup = "exit 1"

[repos.nomain]
path = "{r}/nomain"
main_branch = "main"

[workspace]
default_repo = "core"
merge_order = ["core", "web", "extra", "bad", "nomain"]
worktree_dir = "{c}"
"#,
                    r = repos.display(),
                    c = changes.display()
                )
            } else {
                String::new()
            },
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
                        "repo-add-{}-{}",
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
            project_id: "prepo".into(),
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

    fn add(&self, who: &str, repo: &str) -> Value {
        let answer = self.call(who, "repo", &["add", repo]);
        assert_eq!(answer["ok"], true, "repo add {repo}: {answer}");
        answer["result"].clone()
    }

    /// The launcher's code and message of a refusal (the wire code is `rejected`; the message is `<code>: <text>`).
    fn refused(&self, who: &str, repo: &str) -> (String, String) {
        let answer = self.call(who, "repo", &["add", repo]);
        assert_eq!(answer["ok"], false, "repo add {repo}: {answer}");
        refusal(&answer)
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

    fn repo_names(&self, agent: &str) -> Vec<String> {
        let mut names: Vec<String> = self
            .rows(agent)
            .iter()
            .map(|row| row["repo"].as_str().unwrap().to_string())
            .collect();
        names.sort();
        names
    }

    fn repo(&self, name: &str) -> PathBuf {
        self.repos.join(name)
    }

    fn branches(&self, repo: &str) -> Vec<String> {
        git(
            &self.repo(repo),
            &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
        )
        .lines()
        .map(str::to_string)
        .collect()
    }

    /// The worktrees git lists for a repo, besides the repo itself.
    fn worktrees(&self, repo: &str) -> usize {
        git(&self.repo(repo), &["worktree", "list", "--porcelain"])
            .matches("worktree ")
            .count()
            - 1
    }

    /// Every live row of `agent` has a worktree on disk and every repo folder of its change folder has a row.
    fn assert_rows_match_disk(&self, agent: &str) {
        let rows = self.rows(agent);
        for row in &rows {
            let path = Path::new(row["worktreePath"].as_str().unwrap());
            assert!(
                path.join(".git").exists(),
                "no worktree at {}",
                path.display()
            );
        }
        let folder = self.changes.join(agent);
        if let Ok(entries) = std::fs::read_dir(&folder) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                assert!(
                    rows.iter().any(|row| row["repo"] == name.as_str()),
                    "{} has no row",
                    entry.path().display()
                );
            }
        }
    }

    /// An approved plan whose packages name the given repos.
    fn plan(&self, packages: &[(&str, &[&str])]) {
        let list: Vec<Value> = packages
            .iter()
            .map(|(id, repos)| {
                let owns: Vec<String> = repos.iter().map(|r| format!("{r}:src/{id}")).collect();
                json!({"id": id, "title": id, "repos": repos, "owns": owns, "acceptance": ["x"], "estimate_hours": 1})
            })
            .collect();
        let order: Vec<&str> = packages.iter().map(|(id, _)| *id).collect();
        let body =
            json!({"summary": "s", "packages": list, "risks": [], "integration_order": order})
                .to_string();
        let stored =
            serde_json::to_string(&capstan_kernel::plan_body::parse_plan_body(&body, 8).unwrap())
                .unwrap();
        let base = self.base.clone();
        self.ledger(move |core| {
            use capstan_kernel::helpers::execute;
            let db = &core.kernel().database;
            execute(
                db,
                "INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, architect_agent_id, current_revision, approved_revision, created_at, updated_at)
                 VALUES ('prepo', 'plan-ws', (SELECT COALESCE(MAX(sequence), 0) + 1 FROM plans), 'T', 'normal', 'draft', 'x', 'pm-1', 0, NULL, '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')",
                (),
            )?;
            execute(
                db,
                "INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at)
                 VALUES ('prepo', 'plan-ws', 1, ?, ?, ?, 'pm-1', (SELECT actor_id FROM agents WHERE agent_id = 'pm-1'), '2026-01-01T00:00:00.000Z')",
                [base.as_str(), stored.as_str(), "c".repeat(64).as_str()],
            )?;
            execute(
                db,
                "UPDATE plans SET current_revision = 1, state = 'approved', approved_revision = 1 WHERE plan_id = 'plan-ws'",
                (),
            )
            .map(drop)
        });
    }

    fn package_worker(&self, package: &str) -> String {
        self.spawn(&SpawnOptions {
            task: Some(format!("plan-ws/{package}")),
            ..Default::default()
        })
    }
}

#[test]
fn repo_is_a_routed_agent_command_that_is_never_retried() {
    assert!(RUST_ONLY_ROUTES.contains(&"repo"));
    assert!(capstan_wire::NO_LOST_REPLY_RETRY.contains(&"repo"));
    let route = capstan_daemon::handlers::route_of("repo").expect("the route");
    assert_eq!(route.access, capstan_daemon::handlers::Access::Agent);
}

#[test]
fn an_ad_hoc_worker_adds_any_repo_on_its_branch_at_the_main_tip() {
    let w = World::new();
    let agent = w.spawn(&SpawnOptions::default());
    assert_eq!(w.repo_names(&agent), ["core"]);
    // A commit on web's main since the repo was made: the worktree starts at the tip.
    std::fs::write(w.repo("web").join("later.txt"), "x\n").unwrap();
    git(&w.repo("web"), &["add", "-A"]);
    git(&w.repo("web"), &["commit", "--quiet", "-m", "feat: later"]);
    let tip = git(&w.repo("web"), &["rev-parse", "main"]);
    let branch = w.rows(&agent)[0]["branch"].as_str().unwrap().to_string();
    let added = w.add(&agent, "web");
    assert_eq!(added["repo"], "web");
    assert_eq!(added["branch"], branch.as_str());
    assert_eq!(added["baseSha"], tip.as_str());
    let tree = w.changes.join(&agent).join("web");
    assert_eq!(added["worktreePath"], tree.to_string_lossy().as_ref());
    assert_eq!(git(&tree, &["rev-parse", "--abbrev-ref", "HEAD"]), branch);
    assert_eq!(git(&tree, &["rev-parse", "HEAD"]), tip);
    assert_eq!(w.repo_names(&agent), ["core", "web"]);
    assert!(w.branches("web").contains(&branch));
    // Any repo, listed in the configuration, may be added.
    w.add(&agent, "extra");
    assert_eq!(w.repo_names(&agent), ["core", "extra", "web"]);
    w.assert_rows_match_disk(&agent);
}

#[test]
fn each_refusal_has_its_code_and_text_and_leaves_nothing() {
    let w = World::new();
    let agent = w.spawn(&SpawnOptions::default());
    let branch = w.rows(&agent)[0]["branch"].as_str().unwrap().to_string();
    let (code, message) = w.refused(&agent, "ghost");
    assert_eq!(code, "unknown_repo", "{message}");
    assert!(message.contains("ghost"));
    let (code, _) = w.refused(&agent, "core");
    assert_eq!(code, "already_added");
    // The PM has no worktree.
    let (code, _) = w.refused("pm-1", "web");
    assert_eq!(code, "not_a_worker");
    // The branch name is taken in web.
    git(&w.repo("web"), &["branch", &branch]);
    let (code, message) = w.refused(&agent, "web");
    assert_eq!(code, "branch_taken");
    assert_eq!(
        message,
        format!("branch {branch} is taken in repo web; rename your branch in every repo (links rename without repos) or ask the PM")
    );
    // A repo without its main branch cannot be started from.
    let (code, _) = w.refused(&agent, "nomain");
    assert_eq!(code, "repo_unusable");
    // A setup that fails removes the worktree and the branch it made.
    let (code, message) = w.refused(&agent, "bad");
    assert_eq!(code, "setup_failed");
    assert!(message.contains("bad"), "{message}");
    assert_eq!(w.worktrees("bad"), 0);
    assert!(!w.branches("bad").contains(&branch));
    assert!(!w.changes.join(&agent).join("bad").exists());
    for name in ["ghost", "web", "nomain", "bad"] {
        assert!(!w.repo_names(&agent).contains(&name.to_string()), "{name}");
    }
    assert_eq!(w.repo_names(&agent), ["core"]);
    assert_eq!(w.worktrees("web"), 0);
    w.assert_rows_match_disk(&agent);
}

#[test]
fn a_configuration_without_repos_refuses_not_workspace() {
    let w = World::build(false);
    let agent = w.spawn(&SpawnOptions::default());
    let (code, message) = w.refused(&agent, "core");
    assert_eq!(code, "not_workspace");
    assert_eq!(message, "the configuration has no [repos]");
    assert!(w.repo_names(&agent).is_empty());
}

#[test]
fn a_package_worker_adds_only_repos_of_its_package() {
    let w = World::new();
    w.plan(&[("api", &["core", "web"]), ("solo", &["core"])]);
    let solo = w.package_worker("solo");
    assert_eq!(w.repo_names(&solo), ["core"]);
    let (code, message) = w.refused(&solo, "web");
    assert_eq!(code, "not_in_package");
    assert_eq!(message, "repo web is not in package plan-ws/solo");
    let (code, _) = w.refused(&solo, "extra");
    assert_eq!(code, "not_in_package");
    assert_eq!(w.worktrees("web"), 0);
    // A worker of the two-repo package re-adds a repo whose row it lost while it keeps the other.
    let api = w.package_worker("api");
    assert_eq!(w.repo_names(&api), ["core", "web"]);
    let (code, _) = w.refused(&api, "extra");
    assert_eq!(code, "not_in_package");
    let tree = w.changes.join(&api).join("web");
    let branch = w.rows(&api)[0]["branch"].as_str().unwrap().to_string();
    git(
        &w.repo("web"),
        &["worktree", "remove", "--force", &tree.to_string_lossy()],
    );
    git(&w.repo("web"), &["branch", "-D", &branch]);
    let owner = w.deps.credential().to_string();
    let id = api.clone();
    w.ledger(move |core| {
        let context = capstan_daemon::deps::new_context(core, &owner)?;
        core.remove_agent_worktree(&context, &json!({"agentId": id, "repo": "web"}))
    });
    assert_eq!(w.repo_names(&api), ["core"]);
    w.add(&api, "web");
    assert_eq!(w.repo_names(&api), ["core", "web"]);
    w.assert_rows_match_disk(&api);
}

#[test]
fn two_adds_of_one_repo_from_two_threads_give_one_worktree() {
    let w = World::new();
    let agent = w.spawn(&SpawnOptions::default());
    let answers: Vec<Value> = (0..2)
        .map(|_| {
            let (w, agent) = (Arc::clone(&w), agent.clone());
            std::thread::spawn(move || w.call(&agent, "repo", &["add", "web"]))
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    let done = answers.iter().filter(|a| a["ok"] == true).count();
    let again: Vec<&Value> = answers
        .iter()
        .filter(|a| a["ok"] == false && refusal(a).0 == "already_added")
        .collect();
    assert_eq!((done, again.len()), (1, 1), "{answers:?}");
    assert_eq!(w.worktrees("web"), 1);
    let branch = w.rows(&agent)[0]["branch"].as_str().unwrap().to_string();
    assert_eq!(
        w.branches("web").iter().filter(|b| **b == branch).count(),
        1
    );
    assert_eq!(w.repo_names(&agent), ["core", "web"]);
    w.assert_rows_match_disk(&agent);
}

#[test]
fn add_then_release_and_release_then_add_end_consistent() {
    let w = World::new();
    let agent = w.spawn(&SpawnOptions::default());
    w.add(&agent, "web");
    w.launcher.release(&agent).expect("the release");
    assert!(w.rows(&agent).is_empty());
    assert_eq!(w.worktrees("web"), 0);
    assert_eq!(w.worktrees("core"), 0);
    assert!(!w.changes.join(&agent).join("web").exists());
    // Added after the release: no live row.
    let error = w.launcher.repo_add(&agent, "extra").expect_err("refused");
    assert_eq!(error.code, "not_a_worker");
    let answer = w.call(&agent, "repo", &["add", "extra"]);
    assert_eq!(answer["ok"], false, "{answer}");
    let (code, _) = refusal(&answer);
    assert!(
        code == "not_a_worker" || code == "forbidden" || code == "unauthorized",
        "{code}"
    );
    assert_eq!(w.worktrees("extra"), 0);
    w.assert_rows_match_disk(&agent);
}

#[test]
fn add_then_replace_and_replace_then_add_end_consistent() {
    let w = World::new();
    let first = w.spawn(&SpawnOptions::default());
    w.add(&first, "web");
    let replaced = w.launcher.replace(&first).expect("the replace").to_value();
    let second = replaced["agentId"]
        .as_str()
        .expect("a successor")
        .to_string();
    w.note_token(&second);
    assert_ne!(first, second);
    assert_eq!(w.repo_names(&second), ["core", "web"]);
    assert!(w.rows(&first).is_empty());
    w.assert_rows_match_disk(&second);
    // The predecessor has no live row any more; the successor adds a repo.
    let error = w.launcher.repo_add(&first, "extra").expect_err("refused");
    assert_eq!(error.code, "not_a_worker");
    w.add(&second, "extra");
    assert_eq!(w.repo_names(&second), ["core", "extra", "web"]);
    w.assert_rows_match_disk(&second);
    assert_eq!(w.worktrees("extra"), 1);
}

#[test]
fn a_replace_that_failed_before_the_copy_leaves_no_worker() {
    let w = World::new();
    let agent = w.spawn(&SpawnOptions::default());
    w.stub.apply(&json!({
        "do": "fail", "method": "createTab",
        "error": {"kind": "unsupported_host", "message": "no"}
    }));
    assert!(w.launcher.replace(&agent).is_err());
    let error = w.launcher.repo_add(&agent, "web").expect_err("refused");
    assert_eq!(error.code, "not_a_worker");
    assert_eq!(w.worktrees("web"), 0);
    w.assert_rows_match_disk(&agent);
    let _ = &w.scratch;
}

#[test]
fn a_repo_add_may_run_as_long_as_a_spawn() {
    // The trial: `repo add` ran a repo's setup (about 21 s) and the daemon closed the connection first.
    let w = World::new();
    let who = capstan_kernel::types::Identity {
        actor_id: "x".into(),
        role: "developer".into(),
        capabilities: Vec::new(),
        agent: None,
    };
    let limit = capstan_daemon::handlers::command_limit_ms(&w.deps, "repo", &who);
    assert_eq!(
        limit,
        capstan_daemon::handlers::command_limit_ms(&w.deps, "spawn", &who)
    );
    assert!(limit.is_some_and(|ms| ms >= 600_000), "{limit:?}");
}

/// Commits one file in the worker's worktree of `repo` and returns the commit.
fn commit_in(w: &World, agent: &str, repo: &str, file: &str) -> String {
    let tree = w.changes.join(agent).join(repo);
    std::fs::write(tree.join(file), "x\n").unwrap();
    git(&tree, &["add", "-A"]);
    git(
        &tree,
        &["commit", "--quiet", "-m", &format!("feat: {file}")],
    );
    git(&tree, &["rev-parse", "HEAD"])
}

#[test]
fn repo_add_then_a_report_with_repo_equals_sha_names_both_repos() {
    let w = World::new();
    let agent = w.spawn(&SpawnOptions::default());
    w.add(&agent, "web");
    let core = commit_in(&w, &agent, "core", "a.txt");
    let web = commit_in(&w, &agent, "web", "b.txt");
    let answer = w.call(
        &agent,
        "report",
        &[&format!("core={core}"), &format!("web={web}"), "both repos"],
    );
    assert_eq!(answer["ok"], true, "{answer}");
    let report = answer["result"]["reportId"].as_str().unwrap().to_string();
    let commits = w
        .ledger(move |core| core.report_commits(&report))
        .as_array()
        .cloned()
        .unwrap();
    let repos: Vec<&str> = commits
        .iter()
        .map(|c| c["repo"].as_str().unwrap())
        .collect();
    assert_eq!(repos, ["core", "web"]);
    // The PM's notice lists every repo's commit and branch.
    let branch = w.rows(&agent)[0]["branch"].as_str().unwrap().to_string();
    let notice = w.pm_notice();
    assert!(notice.contains("Commits:"), "{notice}");
    assert!(
        notice.contains(&format!("- core: {core} on branch {branch}")),
        "{notice}"
    );
    assert!(
        notice.contains(&format!("- web: {web} on branch {branch}")),
        "{notice}"
    );
    assert!(!notice.contains("\nCommit: "), "{notice}");
}

#[test]
fn release_removes_every_worktree_and_keeps_the_branches_with_commits() {
    let w = World::new();
    let agent = w.spawn(&SpawnOptions::default());
    w.add(&agent, "web");
    commit_in(&w, &agent, "core", "a.txt");
    commit_in(&w, &agent, "web", "b.txt");
    let branch = w.rows(&agent)[0]["branch"].as_str().unwrap().to_string();
    let released = w.launcher.release(&agent).expect("the release");
    assert_eq!(released.outcome.worktree_removed, Some(true));
    assert_eq!(released.outcome.branch_kept, Some(true));
    assert_eq!(w.worktrees("core"), 0);
    assert_eq!(w.worktrees("web"), 0);
    assert!(!w.changes.join(&agent).exists());
    assert!(w.branches("core").contains(&branch));
    assert!(w.branches("web").contains(&branch));
    assert!(w.rows(&agent).is_empty());
}

impl World {
    /// The newest "Verified report" message queued for the PM.
    fn pm_notice(&self) -> String {
        self.ledger(|core| {
            let kernel = core.kernel();
            capstan_kernel::helpers::query_all(
                &kernel.database,
                "SELECT body FROM messages WHERE project_id = ? AND body LIKE 'Verified report%' ORDER BY sequence",
                [&kernel.project_id],
                |row| row.get::<_, String>(0),
            )
        })
        .pop()
        .expect("a verified-report notice")
    }
}
