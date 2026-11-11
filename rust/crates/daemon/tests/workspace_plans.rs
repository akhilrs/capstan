//! `plan submit` and `plan amend` in a workspace project (a non-repo root over two repos): the repo checks of a body and
//! the revision base, which is the default repo's HEAD. The single-repo project refuses repos.

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
    if !repos.is_empty() {
        package["repos"] = json!(repos);
    }
    package
}

fn workspace_plan() -> String {
    body(&[
        package("api", &["core"], "core:src/api/"),
        package("ui", &["web"], "web:src/ui/"),
    ])
}

#[test]
fn a_workspace_plan_needs_repos_known_to_the_workspace() {
    let world = World::build(true);
    let plan = world.open("normal");
    let plain = body(&[package("api", &[], "src/api/")]);
    let refused = world.refused("arch-1", "plan", &["submit", &plan, &plain]);
    assert!(
        refused.contains("package api needs repos in a workspace project"),
        "{refused}"
    );
    let unknown = body(&[package("api", &["nope"], "nope:src/api/")]);
    let refused = world.refused("arch-1", "plan", &["submit", &plan, &unknown]);
    assert!(
        refused.contains("package api names unknown repo nope"),
        "{refused}"
    );
}

#[test]
fn a_single_repo_plan_refuses_repos() {
    let world = World::build(false);
    let plan = world.open("normal");
    let refused = world.refused("arch-1", "plan", &["submit", &plan, &workspace_plan()]);
    assert!(
        refused.contains("repos are only for workspace projects"),
        "{refused}"
    );
    // The single-repo baseline still submits, with the project HEAD as the base.
    let ok = world.ok(
        "arch-1",
        "plan",
        &["submit", &plan, &body(&[package("api", &[], "src/api/")])],
    );
    assert_eq!(ok["state"], "approved");
    let shown = world.ok("operator", "plan", &["show", &plan]).to_string();
    assert!(shown.contains(&world.heads["main"]), "{shown}");
}

#[test]
fn a_workspace_plan_takes_the_default_repo_head_as_its_base() {
    let world = World::build(true);
    let plan = world.open("normal");
    let ok = world.ok("arch-1", "plan", &["submit", &plan, &workspace_plan()]);
    assert_eq!(ok["state"], "approved");
    let shown = world.ok("operator", "plan", &["show", &plan]).to_string();
    assert!(shown.contains(&world.heads["core"]), "{shown}");
    assert!(!shown.contains(&world.heads["web"]) || world.heads["web"] == world.heads["core"]);
}

#[test]
fn a_reviewed_workspace_plan_keeps_repos_through_an_amendment() {
    let world = World::build_at(true, true);
    let plan = world.open("high-risk");
    world.spawn_reviewer();
    let submitted = world.ok("arch-1", "plan", &["submit", &plan, &workspace_plan()]);
    assert_eq!(submitted["state"], "in_review");
    world.ok(
        "rev-1",
        "review",
        &["findings", "- [ui] major owns: ui also edits docs"],
    );
    let changed = r#"{"reason":"repos","packages":[{"id":"ui","repos":["core"]}]}"#;
    let refused = world.refused("arch-1", "plan", &["amend", &plan, changed]);
    assert!(refused.contains("repos cannot be amended"), "{refused}");
    let amendment =
        r#"{"reason":"ui edits docs","packages":[{"id":"ui","add_owns":["web:docs/ui"]}]}"#;
    world.spawn_reviewer();
    let amended = world.ok("arch-1", "plan", &["amend", &plan, amendment]);
    assert_eq!(amended["state"], "in_review");
    let shown = world.ok("operator", "plan", &["show", &plan]).to_string();
    assert!(shown.contains("web:docs/ui"), "{shown}");
    assert!(shown.contains("\"repos\":[\"web\"]"), "{shown}");
    assert!(shown.contains(&world.heads["core"]), "{shown}");
    assert!(!shown.contains(&world.heads["root"]), "{shown}");
}

#[test]
fn an_unreadable_default_repo_refuses_the_submit() {
    let text = "default_repo core has no readable HEAD";
    for case in ["missing", "detached", "not-git", "unborn"] {
        let world = World::build(true);
        let core = world.repos.join("core");
        match case {
            "missing" => std::fs::remove_dir_all(&core).unwrap(),
            "detached" => {
                let head = git(&core, &["rev-parse", "HEAD"]);
                git(&core, &["checkout", "--quiet", "--detach", &head]);
            }
            "not-git" => std::fs::remove_dir_all(core.join(".git")).unwrap(),
            _ => {
                std::fs::remove_dir_all(core.join(".git")).unwrap();
                git(&core, &["init", "--quiet", "-b", "main"]);
            }
        }
        let plan = world.open("normal");
        let refused = world.refused("arch-1", "plan", &["submit", &plan, &workspace_plan()]);
        assert!(refused.contains(text), "{case}: {refused}");
    }
}
