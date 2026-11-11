//! Daemon git use from a workspace root that is not a repository (two repos under it): the plan base, the plan review
//! spawn, `plan landed`, the file hand-over, the knowledge base and the integration recovery never run git at the root.
//! The report, verify scratch, review, decide, decide-merge and cleanup paths are per repo and run on the same kind of
//! root in the workspace_report, workspace_verify, workspace_review, workspace_decide_merge and workspace_cleanup suites.

mod common;

use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::handlers::handle_frame;
use capstan_daemon::handlers::shared::AbortSignal;
use capstan_daemon::handlers::MAX_RESPONSE_BYTES;
use capstan_daemon::ports::ProjectGit;
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::Core;
use capstan_launcher::api::{LauncherService, Placement, SpawnResult, StubLauncher};
use common::private_tempdir;
use common::project::{build_repo, write_project, Identity};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

const BASE_CONFIG: &str = r#"schema_version = 1

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

/// A start far from zero, so no time is a default.
const BASE: i64 = 1_800_000_000_000;

/// The kernel clock is the test clock.
struct TestEnv {
    clock: Arc<AtomicI64>,
    inner: capstan_kernel::SeededEnv,
}

impl capstan_kernel::Env for TestEnv {
    fn now(&self) -> i64 {
        self.clock.load(Ordering::SeqCst)
    }
    fn uuid(&self) -> String {
        self.inner.uuid()
    }
    fn random_bytes(&self, n: usize) -> Vec<u8> {
        self.inner.random_bytes(n)
    }
}

static WORLDS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

struct World {
    _dir: common::PrivateDir,
    _scratch: common::PrivateDir,
    deps: Deps,
    credentials: HashMap<String, String>,
    heads: BTreeMap<String, String>,
    launcher: Arc<StubLauncher>,
    repos: PathBuf,
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs([
            ("PATH", std::env::var("PATH").unwrap_or_default()),
            ("LC_ALL", "C".into()),
            ("GIT_CONFIG_NOSYSTEM", "1".into()),
            ("GIT_CONFIG_GLOBAL", "/dev/null".into()),
            ("GIT_AUTHOR_NAME", "Test".into()),
            ("GIT_AUTHOR_EMAIL", "test@example.com".into()),
            ("GIT_COMMITTER_NAME", "Test".into()),
            ("GIT_COMMITTER_EMAIL", "test@example.com".into()),
        ])
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn make_repo(dir: &Path) -> String {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "--quiet", "-b", "main"]);
    std::fs::write(dir.join("README.md"), "repo\n").unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "--quiet", "-m", "chore: initial commit"]);
    git(dir, &["rev-parse", "HEAD"])
}

impl World {
    /// A workspace project (a non-repo root over repos `core` and `web`) or a single-repo project.
    fn build(workspace: bool) -> World {
        World::build_at(workspace, false)
    }

    /// `root_repo` makes the project root a git repository too, which a plan review needs for now.
    fn build_at(workspace: bool, root_repo: bool) -> World {
        let dir = private_tempdir();
        let scratch = private_tempdir();
        let repos = scratch.path().join("repos");
        let identity = Identity {
            project_id: "planws".into(),
            name: "Plan Ws".into(),
            credential: format!("operator-{}", "w".repeat(40)),
        };
        let state = write_project(dir.path(), &identity);
        let mut config = BASE_CONFIG.to_string();
        let mut heads = BTreeMap::new();
        if workspace {
            heads.insert("core".to_string(), make_repo(&repos.join("core")));
            heads.insert("web".to_string(), make_repo(&repos.join("web")));
            if root_repo {
                heads.insert("root".to_string(), make_repo(dir.path()));
                // The review still looks the base up in the root's git: give it the default repo's objects.
                git(
                    dir.path(),
                    &["fetch", "--quiet", &repos.join("core").to_string_lossy()],
                );
            }
            config.push_str(&format!(
                r#"
[repos.core]
path = "{r}/core"
main_branch = "main"

[repos.web]
path = "{r}/web"
main_branch = "main"

[workspace]
default_repo = "core"
merge_order = ["core", "web"]
worktree_dir = "{c}"
"#,
                r = repos.display(),
                c = scratch.path().join("changes").display()
            ));
        } else {
            heads.extend(build_repo(dir.path(), &json!({"files": {"a.txt": "a\n"}})));
        }
        let file = dir.path().join("capstan.toml");
        std::fs::write(&file, config).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        let trust = common::kept_trust_store();
        common::trusted_project(dir.path(), &trust);
        let database = capstan_ledger::resolve_database_path(&state).unwrap();
        capstan_ledger::open_database(&database, &capstan_ledger::OpenOptions::default()).unwrap();
        let project = identity.initial_project();
        let clock = Arc::new(AtomicI64::new(BASE));
        let kernel = {
            let clock = clock.clone();
            let (state, project, root) = (state.clone(), project.clone(), dir.path().to_path_buf());
            KernelHandle::spawn(move || {
                Core::open(
                    &state,
                    &project,
                    &KernelOptions {
                        workspace_root: Some(root),
                        ..KernelOptions::default()
                    },
                    Box::new(TestEnv {
                        clock,
                        inner: capstan_kernel::SeededEnv::new(&format!(
                            "planws-{}-{}",
                            std::process::id(),
                            WORLDS.fetch_add(1, Ordering::SeqCst)
                        )),
                    }),
                )
            })
            .expect("the kernel opens")
        };
        let sink: Logger = Arc::new(|_: &LogEntry| {});
        let mut options = DaemonOptions::new(state, project, dir.path().to_path_buf());
        options.trust_store = trust.clone();
        let role_config = capstan_config::load_role_config_with(dir.path(), &trust).unwrap();
        assert_eq!(role_config.workspace.is_some(), workspace);
        options.capstan = Some(Arc::new(role_config));
        let launcher = Arc::new(StubLauncher::new());
        let mut deps = Deps::new(kernel, options, sink);
        deps.git = Arc::new(ProjectGit::new(dir.path()));
        deps.launcher = Some(launcher.clone() as Arc<dyn LauncherService>);
        capstan_daemon::deps::startup_sequence(&deps);
        let mut world = World {
            _dir: dir,
            _scratch: scratch,
            deps,
            credentials: HashMap::new(),
            heads,
            launcher,
            repos,
        };
        world
            .credentials
            .insert("operator".into(), identity.credential);
        world.populate();
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

    fn open(&self, tier: &str) -> String {
        let opened = self.ok("pm-1", "plan", &["open", tier, "Ws plan"]);
        opened["planId"].as_str().unwrap().to_string()
    }

    fn spawn_reviewer(&self) {
        self.launcher.script_spawn(SpawnResult {
            state: "started".into(),
            agent_id: "rev-1".into(),
            pane_id: "w1:p1".into(),
            worktree_path: "/tmp/work/rev-1".into(),
            branch: "capstan/rev-1".into(),
            placement: Placement::Tab,
            placement_note: None,
            hint: None,
            missing_env: None,
            warning: None,
        });
    }
}

fn body(packages: &[Value]) -> String {
    json!({"summary": "split", "packages": packages}).to_string()
}

fn package(id: &str, repos: &[&str], owns: &str) -> Value {
    let mut package = json!({"id": id, "title": id, "owns": [owns], "acceptance": [format!("{id} works")], "estimate_hours": 2});
    package["repos"] = json!(repos);
    package
}

fn workspace_plan() -> String {
    body(&[
        package("api", &["core"], "core:src/api/"),
        package("ui", &["web"], "web:src/ui/"),
    ])
}

fn non_repo_root(world: &World) {
    assert!(
        !world._dir.path().join(".git").exists(),
        "the root is not a repository"
    );
}

#[test]
fn a_plan_submit_takes_the_default_repo_base() {
    let world = World::build(true);
    non_repo_root(&world);
    let plan = world.open("normal");
    let ok = world.ok("arch-1", "plan", &["submit", &plan, &workspace_plan()]);
    assert_eq!(ok["state"], "approved");
    let shown = world.ok("operator", "plan", &["show", &plan]).to_string();
    assert!(shown.contains(&world.heads["core"]), "{shown}");
}

#[test]
fn a_workspace_plan_reaches_plan_review_from_a_root_that_is_not_a_repo() {
    let world = World::build(true);
    non_repo_root(&world);
    let plan = world.open("high-risk");
    world.spawn_reviewer();
    let submitted = world.ok("arch-1", "plan", &["submit", &plan, &workspace_plan()]);
    assert_eq!(submitted["state"], "in_review", "{submitted}");
    let shown = world.ok("operator", "plan", &["show", &plan]).to_string();
    assert!(shown.contains(&world.heads["core"]), "{shown}");
}

#[test]
fn plan_landed_answers_no_repository_at_the_workspace_root() {
    let world = World::build(true);
    non_repo_root(&world);
    let plan = world.open("normal");
    world.ok("arch-1", "plan", &["submit", &plan, &workspace_plan()]);
    let sha = world.heads["core"].clone();
    let refused = world.refused("operator", "plan", &["landed", &plan, "api", &sha]);
    assert!(
        refused.contains("no repository at the workspace root"),
        "{refused}"
    );
}

#[test]
fn a_file_hand_over_runs_git_in_the_senders_worktree() {
    let world = World::build(true);
    let core = world.repos.join("core");
    std::fs::write(core.join("note.md"), "hello\n").unwrap();
    git(&core, &["add", "-A"]);
    git(&core, &["commit", "--quiet", "-m", "docs: note"]);
    let tip = git(&core, &["rev-parse", "HEAD"]);
    let root = world._dir.path();
    let found =
        capstan_daemon::file_handoff::locate(root, "main", &core.to_string_lossy(), "note.md")
            .expect("found through the worktree");
    assert_eq!(found.commit, tip);
    assert_eq!(found.size, 6);
    // Without a repository worktree the answer is the same text as before.
    let gone = capstan_daemon::file_handoff::locate(root, "main", "/nonexistent", "note.md");
    assert_eq!(gone.unwrap_err(), "the branch main has no commit");
}

#[test]
fn the_knowledge_base_answers_the_same_text_at_a_root_that_is_not_a_repo() {
    let world = World::build(true);
    let state = tempfile_state(&world);
    let cfg = capstan_config::kb::KbConfig::default();
    let error = capstan_daemon::kb::ensure(&state, world._dir.path(), &cfg).err();
    assert_eq!(
        error.map(|e| e.to_string()),
        Some("git: the project has no commit".to_string())
    );
}

fn tempfile_state(world: &World) -> PathBuf {
    world.deps.options.state_directory.clone()
}

#[test]
fn recovery_runs_without_a_root_repository() {
    let world = World::build(true);
    capstan_daemon::loops::recover::recover_integrations(&world.deps, &|| false);
}
