#![allow(dead_code)]
//! `cstan report` in a workspace project: one commit per repo, `--heads`, coverage by the earlier report. The kernel, the
//! launcher, the ledger and git are real; the host (Herdr) is the launcher tests' stub.

mod common;

use capstan_config::TrustStore;
use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::handlers::handle_frame;
use capstan_daemon::handlers::shared::AbortSignal;
use capstan_daemon::handlers::MAX_RESPONSE_BYTES;
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
        let identity = Identity {
            project_id: "prep".into(),
            name: "Report Ws".into(),
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

[workspace]
default_repo = "core"
merge_order = ["core", "web", "extra"]
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
                        "report-ws-{}-{}",
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
            project_id: "prep".into(),
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

fn pair(repo: &str, sha: &str) -> String {
    format!("{repo}={sha}")
}

fn try_git(cwd: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(engine::git_environment())
        .output()
        .expect("git runs")
        .status
        .success()
}

const ONE_FORM: &str = "use one form: <repo>=<sha>... or --heads";

// ------------------------------------------------------------------------------------------------ grammar

#[test]
fn the_grammar_of_a_workspace_report_and_its_check_order() {
    let w = World::new();
    let agent = w.worker(&["core", "web"]);
    let sha = w.commit(&agent, "core", "a.txt");
    let other = w.commit(&agent, "web", "b.txt");
    let table: Vec<(Vec<String>, &str)> = vec![
        (vec![], "report needs commits and a summary"),
        (vec!["summary".into()], "report needs commits and a summary"),
        (
            vec![sha.clone(), "summary".into()],
            "name each repo: <repo>=<sha>",
        ),
        (vec![sha.clone(), pair("core", &sha)], ONE_FORM),
        (vec![pair("core", &sha), pair("web", &other)], ONE_FORM),
        (vec![pair("core", &sha), sha.clone()], ONE_FORM),
        (
            vec!["--heads".into(), pair("core", &sha), "s".into()],
            ONE_FORM,
        ),
        (
            vec!["--heads".into(), "--heads".into(), "s".into()],
            ONE_FORM,
        ),
        (vec!["core".into(), "s".into()], ONE_FORM),
        (
            vec![pair("core", &sha), pair("core", &sha), "s".into()],
            ONE_FORM,
        ),
        (
            vec![pair("ghost", &sha), "s".into()],
            "repo ghost is not a repo of this project",
        ),
        (
            vec!["core=abc".into(), "s".into()],
            "repo core: the commit must be a full 40-character id (git rev-parse HEAD)",
        ),
        (
            vec![pair("core", &sha), "  ".into()],
            "the summary must not be empty",
        ),
    ];
    for (args, message) in table {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        assert_eq!(w.refused(&agent, &args), message, "{args:?}");
    }
    // Nothing above reached git or the ledger.
    assert_eq!(w.report_count(), 0);
    // A non-worker still gets the old refusal.
    let answer = w.report("pm-1", &["--heads", "s"]);
    assert_eq!(answer["message"], "only a worker agent can report");
}

// ------------------------------------------------------------------------------------------------ pairs

#[test]
fn pairs_record_one_report_with_a_row_per_repo_sorted() {
    let w = World::new();
    let agent = w.worker(&["core", "web"]);
    let (core, web) = (
        w.commit(&agent, "core", "a.txt"),
        w.commit(&agent, "web", "b.txt"),
    );
    // Named in either order: the rows are sorted by repo.
    let done = w.accepted(&agent, &[&pair("web", &web), &pair("core", &core), "both"]);
    assert_eq!(done["carried"], json!([]));
    assert_eq!(
        w.commits_of(&done["reportId"]),
        [
            ("core".to_string(), core.clone()),
            ("web".to_string(), web.clone())
        ]
    );
    // The same commits again are a duplicate and write nothing.
    let count = w.report_count();
    let again = w.accepted(&agent, &[&pair("core", &core), &pair("web", &web), "both"]);
    assert_eq!(again["duplicate"], true);
    assert_eq!(again["reportId"], done["reportId"]);
    assert_eq!(w.report_count(), count);
}

#[test]
fn a_bad_commit_in_one_repo_refuses_the_whole_report() {
    let w = World::new();
    let agent = w.worker(&["core", "web"]);
    let core = w.commit(&agent, "core", "a.txt");
    let web = w.commit(&agent, "web", "b.txt");
    let wrong = "1".repeat(40);
    let message = w.refused(&agent, &[&pair("core", &core), &pair("web", &wrong), "s"]);
    assert_eq!(
        message,
        "report_rejected: repo web: that commit does not exist in the repository (nothing was recorded)"
    );
    // A base commit in the second repo is not new.
    let base = w.base(&agent, "web");
    // (naming the base drops the repo, so use a commit of core in web's place)
    let message = w.refused(&agent, &[&pair("core", &core), &pair("web", &core), "s"]);
    assert!(
        message.starts_with("report_rejected: repo web: "),
        "{message}"
    );
    assert_ne!(base, web);
    // A commit message that breaks the rules in web names web.
    let tree = w.tree(&agent, "web");
    std::fs::write(tree.join("c.txt"), "c\n").unwrap();
    git(&tree, &["add", "-A"]);
    git(&tree, &["commit", "--quiet", "-m", "no type here"]);
    let bad = git(&tree, &["rev-parse", "HEAD"]);
    let message = w.refused(&agent, &[&pair("core", &core), &pair("web", &bad), "s"]);
    assert!(
        message.starts_with("commit_message: repo web: "),
        "{message}"
    );
    assert_eq!(w.report_count(), 0, "nothing was written");
    // A bad commit in the first repo is recorded as rejected, naming the repo.
    let message = w.refused(&agent, &[&pair("core", &wrong), &pair("web", &web), "s"]);
    assert!(
        message.starts_with("report_rejected: repo core: that commit does not exist"),
        "{message}"
    );
}

#[test]
fn naming_the_base_drops_a_repo_and_nothing_new_is_refused() {
    let w = World::new();
    let agent = w.worker(&["core", "web"]);
    let core = w.commit(&agent, "core", "a.txt");
    let web_base = w.base(&agent, "web");
    let done = w.accepted(
        &agent,
        &[&pair("core", &core), &pair("web", &web_base), "core only"],
    );
    assert_eq!(
        w.commits_of(&done["reportId"]),
        [("core".to_string(), core.clone())]
    );
    let both_bases = [
        pair("core", &w.base(&agent, "core")),
        pair("web", &web_base),
    ];
    assert_eq!(
        w.refused(&agent, &[&both_bases[0], &both_bases[1], "none"]),
        "no repo has new commits"
    );
}

// ------------------------------------------------------------------------------------------------ --heads

#[test]
fn heads_takes_what_moved_and_leaves_out_what_did_not() {
    let w = World::new();
    let agent = w.worker(&["core", "web"]);
    assert_eq!(
        w.refused(&agent, &["--heads", "s"]),
        "no repo has new commits"
    );
    let web = w.commit(&agent, "web", "b.txt");
    let done = w.accepted(&agent, &["--heads", "web only"]);
    // The recorded id is the one read.
    assert_eq!(
        w.commits_of(&done["reportId"]),
        [("web".to_string(), web.clone())]
    );
    assert_eq!(done["carried"], json!([]));
    // Both moved: both are taken.
    let (core2, web2) = (
        w.commit(&agent, "core", "c.txt"),
        w.commit(&agent, "web", "d.txt"),
    );
    let done = w.accepted(&agent, &["--heads", "both"]);
    assert_eq!(
        w.commits_of(&done["reportId"]),
        [("core".to_string(), core2), ("web".to_string(), web2)]
    );
}

#[test]
fn heads_refuses_each_unreportable_state_naming_the_repo() {
    let w = World::new();
    let agent = w.worker(&["core", "web"]);
    w.commit(&agent, "web", "b.txt");
    let tree = w.tree(&agent, "web");
    let branch = git(&tree, &["rev-parse", "--abbrev-ref", "HEAD"]);
    // Another branch.
    git(&tree, &["checkout", "--quiet", "-b", "elsewhere"]);
    assert_eq!(
        w.refused(&agent, &["--heads", "s"]),
        format!(
            "repo web is on branch elsewhere, not {branch}; check out {branch} and report again"
        )
    );
    // Detached.
    git(&tree, &["checkout", "--quiet", "--detach"]);
    assert_eq!(
        w.refused(&agent, &["--heads", "s"]),
        format!("repo web has a detached HEAD; check out {branch} and report again")
    );
    git(&tree, &["checkout", "--quiet", &branch]);
    // A merge stopped on a conflict.
    std::fs::write(tree.join("README.md"), "mine\n").unwrap();
    git(&tree, &["commit", "--quiet", "-am", "feat: mine"]);
    git(&tree, &["checkout", "--quiet", "-b", "theirs", "HEAD~2"]);
    std::fs::write(tree.join("README.md"), "theirs\n").unwrap();
    git(&tree, &["commit", "--quiet", "-am", "feat: theirs"]);
    git(&tree, &["checkout", "--quiet", &branch]);
    assert!(!try_git(&tree, &["merge", "--no-edit", "theirs"]));
    assert_eq!(
        w.refused(&agent, &["--heads", "s"]),
        "repo web has a merge in progress; finish or abort it and report again"
    );
    git(&tree, &["merge", "--abort"]);
    // A rebase stopped on a conflict.
    assert!(!try_git(&tree, &["rebase", "theirs"]));
    assert_eq!(
        w.refused(&agent, &["--heads", "s"]),
        "repo web has a rebase in progress; finish or abort it and report again"
    );
    git(&tree, &["rebase", "--abort"]);
    assert_eq!(w.report_count(), 0);
}

#[test]
fn a_dirty_tree_is_taken_with_a_warning() {
    let w = World::new();
    let agent = w.worker(&["core", "web"]);
    let core = w.commit(&agent, "core", "a.txt");
    std::fs::write(w.tree(&agent, "web").join("README.md"), "dirty\n").unwrap();
    let done = w.accepted(&agent, &["--heads", "s"]);
    assert_eq!(
        done["warnings"],
        json!(["repo web has uncommitted changes; they are not in this report"])
    );
    assert_eq!(
        w.commits_of(&done["reportId"]),
        [("core".to_string(), core)]
    );
}

// ------------------------------------------------------------------------------------------------ coverage

#[test]
fn heads_covers_by_the_earlier_report_across_every_row_of_the_table() {
    let w = World::new();
    let agent = w.worker(&["core", "web", "extra"]);
    let core = w.commit(&agent, "core", "a.txt");
    let web = w.commit(&agent, "web", "b.txt");
    let first = w.accepted(&agent, &["--heads", "first"]);
    assert_eq!(first["carried"], json!([]));
    // H = E in both repos and nothing new.
    assert_eq!(
        w.refused(&agent, &["--heads", "again"]),
        format!(
            "no repo has new commits since report {}",
            first["reportId"].as_str().unwrap()
        )
    );
    // core moves; web is carried (H = E); extra never moved and is left out.
    let core2 = w.commit(&agent, "core", "c.txt");
    let second = w.accepted(&agent, &["--heads", "second"]);
    assert_eq!(second["carried"], json!(["web"]));
    assert_eq!(
        w.commits_of(&second["reportId"]),
        [
            ("core".to_string(), core2.clone()),
            ("web".to_string(), web.clone())
        ]
    );
    // web goes back to its base with E set: refused, and dropping it by name works.
    let web_base = w.base(&agent, "web");
    git(
        &w.tree(&agent, "web"),
        &["reset", "--quiet", "--hard", &web_base],
    );
    assert_eq!(
        w.refused(&agent, &["--heads", "back"]),
        format!("repo web is back at its base; name it web={web_base} to drop it, or commit")
    );
    let core3 = w.commit(&agent, "core", "e.txt");
    let dropped = w.accepted(
        &agent,
        &[&pair("core", &core3), &pair("web", &web_base), "drop web"],
    );
    assert_eq!(
        w.commits_of(&dropped["reportId"]),
        [("core".to_string(), core3)]
    );
    let _ = (core, core2);
}

#[test]
fn pairs_carry_an_omitted_repo_the_report_covers_and_refuse_a_moved_one() {
    let w = World::new();
    let agent = w.worker(&["core", "web"]);
    let core = w.commit(&agent, "core", "a.txt");
    let web = w.commit(&agent, "web", "b.txt");
    let first = w.accepted(&agent, &[&pair("core", &core), &pair("web", &web), "first"]);
    // web moves on, core is omitted: core is carried and sorts first.
    let web2 = w.commit(&agent, "web", "c.txt");
    let second = w.accepted(&agent, &[&pair("web", &web2), "second"]);
    assert_eq!(second["carried"], json!(["core"]));
    assert_eq!(
        w.commits_of(&second["reportId"]),
        [
            ("core".to_string(), core.clone()),
            ("web".to_string(), web2.clone())
        ]
    );
    assert_ne!(
        first["reportId"], second["reportId"],
        "a carried first commit is still a new report"
    );
    // The same report again is a duplicate.
    let again = w.accepted(&agent, &[&pair("web", &web2), "second"]);
    assert_eq!(again["duplicate"], true);
    assert_eq!(again["reportId"], second["reportId"]);
    // core moves past what the report covers and is not named: refused.
    w.commit(&agent, "core", "d.txt");
    let web3 = w.commit(&agent, "web", "e.txt");
    let message = w.refused(&agent, &[&pair("web", &web3), "third"]);
    assert_eq!(
        message,
        format!(
            "repo core has commits after report {}; name it",
            second["reportId"].as_str().unwrap()
        )
    );
    // core back at its base is refused as well.
    let base = w.base(&agent, "core");
    git(
        &w.tree(&agent, "core"),
        &["reset", "--quiet", "--hard", &base],
    );
    assert_eq!(
        w.refused(&agent, &[&pair("web", &web3), "third"]),
        format!("repo core is back at its base; name it core={base} to drop it, or commit")
    );
}

#[test]
fn a_replacement_has_no_earlier_report_so_heads_goes_by_the_base() {
    let w = World::new();
    let agent = w.worker(&["core", "web"]);
    let core = w.commit(&agent, "core", "a.txt");
    w.accepted(&agent, &["--heads", "mine"]);
    let replaced = w.launcher.replace(&agent).expect("the replace runs");
    let successor = replaced.to_value()["agentId"]
        .as_str()
        .unwrap_or_else(|| panic!("no successor: {replaced:?}"))
        .to_string();
    w.note_token(&successor);
    assert_ne!(successor, agent);
    // The moved rows keep the base; the successor's commit in core is the head the predecessor left, which is past the
    // base, and web is at its base: core is taken, web is left out.
    let done = w.accepted(&successor, &["--heads", "continued"]);
    assert_eq!(
        w.commits_of(&done["reportId"]),
        [("core".to_string(), core)]
    );
    assert_eq!(done["carried"], json!([]));
}

// ------------------------------------------------------------------------------------------------ ports

#[test]
fn worktree_state_reports_each_state_and_repo_git_of_the_root_is_the_root() {
    let w = World::new();
    let agent = w.worker(&["core"]);
    let tree = w.tree(&agent, "core");
    let git_port = w.deps.repo_git("core");
    let state = git_port.worktree_state(&tree).expect("a state");
    assert_eq!(state.head, w.head(&agent, "core"));
    assert!(state.branch.is_some() && state.in_progress.is_none() && !state.dirty);
    std::fs::write(tree.join("README.md"), "x\n").unwrap();
    assert!(git_port.worktree_state(&tree).unwrap().dirty);
    git(&tree, &["checkout", "--quiet", "--", "README.md"]);
    git(&tree, &["checkout", "--quiet", "--detach"]);
    assert_eq!(git_port.worktree_state(&tree).unwrap().branch, None);
    assert_eq!(
        w.deps.repo_root("core"),
        w.repo("core").canonicalize().unwrap()
    );
    assert_eq!(w.deps.repo_root(""), w.deps.options.workspace_root);
    assert!(Arc::ptr_eq(&w.deps.repo_git(""), &w.deps.git));
}

// ------------------------------------------------------------------------------------------------ single repo

/// Every report call of the existing daemon report tests at base c50db6d1, with the answers that base gave. The table is
/// frozen: it is never regenerated.
#[test]
fn a_project_without_repos_answers_exactly_as_it_did() {
    let w = World::build(false);
    let agent = w.spawn(&SpawnOptions::default());
    let sha = "a".repeat(40);
    let table: Vec<(Vec<&str>, &str)> = vec![
        (vec![], "report needs a commit id and a summary"),
        (vec!["x"], "report needs a commit id and a summary"),
        (
            vec!["x", "y", "z"],
            "report needs a commit id and a summary",
        ),
        (
            vec!["--heads", "s"],
            "the commit must be a full 40-character id (git rev-parse HEAD)",
        ),
        (
            vec!["core=abc", "s"],
            "the commit must be a full 40-character id (git rev-parse HEAD)",
        ),
        (
            vec!["abc", "s"],
            "the commit must be a full 40-character id (git rev-parse HEAD)",
        ),
        (vec![&sha, "   "], "the summary must not be empty"),
    ];
    for (args, message) in table {
        assert_eq!(w.refused(&agent, &args), message, "{args:?}");
    }
    assert_eq!(
        w.refused("pm-1", &[&sha, "s"]),
        "only a worker agent can report"
    );
    // The old-client frame: a bare `v: 1` request with an upper-case sha; a commit that does not exist is recorded as rejected.
    let credential = w.tokens.lock().unwrap().get(&agent).cloned().unwrap();
    let frame = json!({"v": 1, "credential": credential, "command": "report", "args": [sha.to_uppercase(), "One"]});
    let outcome = handle_frame(
        &w.deps,
        frame.to_string().as_bytes(),
        &AbortSignal::new(),
        MAX_RESPONSE_BYTES,
    );
    let answer: Value = serde_json::from_str(&outcome.response.expect("an answer")).unwrap();
    assert_eq!(answer["ok"], false, "{answer}");
    assert!(
        answer["message"]
            .as_str()
            .unwrap()
            .starts_with("report_rejected: that commit does not exist in the repository (report "),
        "{answer}"
    );
}
