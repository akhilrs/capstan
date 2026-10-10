//! The harness of the black-box suites (`tests/*.rs`): the native `cstan`, run as a process the way an operator or an agent
//! runs it, against `cstan daemon` in a scratch project under `/tmp/capstan-suites-core-*`.
//!
//! Rules:
//! - The binary is `CSTAN_BIN`, else the workspace target's (`release/cstan` if it is built, else the profile of the test
//!   run); it is built with cargo when neither exists. `cstan-dash` is `CSTAN_DASH_BIN` (it builds apart from the
//!   workspace, in `dash/`); the one frame test says so when it is not named.
//! - No Node: every child gets `PATH=<scratch>/bin`, which holds links to `git`, `sh` and a few coreutils and nothing else, and no `CSTAN_NODE*`
//!   variable. A test asserts that `node` does not resolve there.
//! - One scratch daemon per `World`, started as `cstan daemon` with `CAPSTAN_LAUNCH=off` (no Herdr), owned by the `World`
//!   and stopped, waited on and removed in `Drop`, also when the test fails.
//! - The ledger is seeded through the kernel before the daemon starts (seats, actors, agents, panes), which is how a spawn
//!   leaves it; the agents' tokens are then used with `CAPSTAN_TOKEN` and `CAPSTAN_SOCKET` as a real agent shell has them.
//!   The seeding is the only place a test knows about the ledger; every assertion is on what `cstan` prints, exits with and
//!   leaves in git.
#![allow(clippy::too_many_arguments)]

use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::{Core, SystemEnv};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Once;
use std::time::{Duration, Instant};

pub use serde_json;

// ------------------------------------------------------------------------------------------------------- binaries

/// The `cstan` under test.
pub fn cstan_binary() -> PathBuf {
    static BUILD: Once = Once::new();
    if let Some(named) = std::env::var_os("CSTAN_BIN") {
        let named = PathBuf::from(named);
        assert!(
            named.is_file(),
            "CSTAN_BIN names no file: {}",
            named.display()
        );
        return named;
    }
    // target/<profile>/deps/<this test> -> target/<profile>/cstan, or target/release/cstan
    let exe = std::env::current_exe().expect("the test executable");
    let profile = exe
        .parent()
        .and_then(Path::parent)
        .expect("target/<profile>");
    let release = profile.with_file_name("release").join("cstan");
    let own = profile.join("cstan");
    if release.is_file() {
        return release;
    }
    BUILD.call_once(|| {
        if own.is_file() {
            return;
        }
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let status = Command::new(cargo)
            .args(["build", "-j", "2", "--locked", "-p", "cstan-front"])
            .current_dir(workspace())
            .status()
            .expect("cargo runs");
        assert!(status.success(), "cstan did not build");
    });
    assert!(
        own.is_file(),
        "no cstan at {}: build cstan-front or set CSTAN_BIN",
        own.display()
    );
    own
}

/// `cstan-dash`, when `CSTAN_DASH_BIN` names it.
pub fn dash_binary() -> Option<PathBuf> {
    std::env::var_os("CSTAN_DASH_BIN")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
}

/// Writes an executable file the race-free way: under a temporary name, closed, then renamed into place (a file that is open
/// for writing cannot be executed, and a parallel test's fork can hold a copy of the descriptor).
pub fn write_executable(path: &Path, text: &str) {
    let temp = path.with_file_name(format!(
        ".{}.tmp{}",
        path.file_name().unwrap().to_string_lossy(),
        std::process::id()
    ));
    {
        let mut file = std::fs::File::create(&temp).unwrap();
        file.write_all(text.as_bytes()).unwrap();
        file.sync_all().unwrap();
    }
    std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::rename(&temp, path).unwrap();
}

/// `rust/` of this checkout.
pub fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The repository root.
pub fn repository() -> PathBuf {
    workspace().join("..")
}

// ------------------------------------------------------------------------------------------------------- results

/// What one `cstan` run did.
#[derive(Debug, Clone)]
pub struct Out {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
    pub args: Vec<String>,
}

impl Out {
    /// Stdout and stderr, for failure messages and loose matches.
    pub fn text(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }

    /// Asserts exit 0 and returns self.
    pub fn ok(self) -> Out {
        assert_eq!(
            self.code,
            0,
            "cstan {:?} failed: {}",
            self.args,
            self.text()
        );
        self
    }

    /// Asserts a non-zero exit with `code` and that the error text contains `fragment`.
    pub fn refused(self, code: i32, fragment: &str) -> Out {
        assert_eq!(self.code, code, "cstan {:?}: {}", self.args, self.text());
        assert!(
            self.stderr.contains(fragment),
            "cstan {:?} did not say {fragment:?}: {}",
            self.args,
            self.text()
        );
        self
    }

    /// Stdout as JSON (every command that prints JSON prints one document).
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|e| {
            panic!(
                "cstan {:?} printed no JSON ({e}): {}",
                self.args,
                self.text()
            )
        })
    }

    pub fn has(&self, fragment: &str) -> bool {
        self.text().contains(fragment)
    }
}

pub fn until<T>(what: &str, seconds: u64, mut action: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        if let Some(found) = action() {
            return found;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A child that is killed (if still alive) when the test ends or fails. A thread reaps it the moment it exits: a daemon that is
/// a zombie still answers `kill(pid, 0)`, so `cstan stop` would wait for it, as it would not for a daemon whose parent is init.
pub struct Run {
    pid: u32,
    status: std::sync::Arc<std::sync::Mutex<Option<std::process::ExitStatus>>>,
}

impl Run {
    fn new(mut child: Child) -> Run {
        let pid = child.id();
        let status = std::sync::Arc::new(std::sync::Mutex::new(None));
        let shared = status.clone();
        std::thread::spawn(move || {
            let ended = child.wait().ok();
            *shared.lock().unwrap() = ended;
        });
        Run { pid, status }
    }
    pub fn pid(&self) -> u32 {
        self.pid
    }
    pub fn try_wait(&mut self) -> Option<std::process::ExitStatus> {
        *self.status.lock().unwrap()
    }
    fn signal(&self, signal: i32) {
        if self.status.lock().unwrap().is_none() {
            // SAFETY: the pid is a child this struct started; it has not been reaped (the status is empty).
            unsafe { libc::kill(self.pid as libc::pid_t, signal) };
        }
    }
    fn wait(&mut self, seconds: u64) -> Option<std::process::ExitStatus> {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        loop {
            if let Some(status) = self.try_wait() {
                return Some(status);
            }
            if Instant::now() > deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    /// SIGTERM, then the exit status (killed after the deadline).
    pub fn terminate(&mut self, seconds: u64) -> std::process::ExitStatus {
        self.signal(libc::SIGTERM);
        if let Some(status) = self.wait(seconds) {
            return status;
        }
        self.signal(libc::SIGKILL);
        self.wait(10).expect("the child ends after SIGKILL")
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        self.signal(libc::SIGKILL);
        let _ = self.wait(10);
    }
}

// ------------------------------------------------------------------------------------------------------- the world

/// An agent the ledger is seeded with.
#[derive(Clone, Debug)]
pub struct AgentSpec {
    pub id: String,
    pub role: String,
    pub kind: String,
}

/// The roles every world is seeded with: name, kind.
pub const ROLES: [(&str, &str); 6] = [
    ("pm", "PM"),
    ("developer", "Developer"),
    ("architect", "Developer"),
    ("reviewer", "Verifier"),
    ("supervisor", "Supervisor"),
    ("researcher", "Developer"),
];

/// A team configuration: the PM, developers, an architect, a reviewer and a supervisor; plans, the operator and the prompt relay
/// on. With launching off (`CAPSTAN_LAUNCH=off`, as every world runs) nothing in it reaches Herdr.
pub const TEAM_CONFIG: &str = r#"schema_version = 1

[hosts.claude]
kind = "claude"

[architect]
enabled = true

[operator]
enabled = true

[prompt_relay]
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

[roles.operator]
kind = "Developer"
host = "claude"
allow = ["Bash(cstan op *)"]
deny = ["Write", "Edit", "NotebookEdit", "Agent", "Task", "Read", "Glob", "Grep"]

[roles.reviewer]
kind = "Verifier"
host = "claude"

[roles.supervisor]
kind = "Supervisor"
host = "claude"
"#;

/// Builds a [`World`].
pub struct Builder {
    label: String,
    agents: Vec<AgentSpec>,
    config: Option<String>,
    env: Vec<(String, String)>,
    git: bool,
    start: bool,
    launch: bool,
    bare: bool,
    panes: bool,
}

pub fn agent(id: &str, role: &str, kind: &str) -> AgentSpec {
    AgentSpec {
        id: id.into(),
        role: role.into(),
        kind: kind.into(),
    }
}

impl Builder {
    pub fn agents(mut self, agents: &[AgentSpec]) -> Self {
        self.agents.extend_from_slice(agents);
        self
    }
    /// A `capstan.toml` for the project (mode 0600).
    pub fn config(mut self, toml: &str) -> Self {
        self.config = Some(toml.to_string());
        self
    }
    /// An environment variable the daemon and every client get.
    pub fn env(mut self, name: &str, value: &str) -> Self {
        self.env.push((name.into(), value.into()));
        self
    }
    /// A launching daemon is given a pane row for every agent by default (an agent without one is a crashed spawn and is ended at
    /// start); this leaves the rows out, for a test of exactly that.
    pub fn without_pane_rows(mut self) -> Self {
        self.panes = false;
        self
    }
    /// A scratch directory with a git repository and nothing else: no `.capstan`, no ledger, no daemon (for `cstan init`).
    pub fn bare(mut self) -> Self {
        self.bare = true;
        self.start = false;
        self
    }
    /// Lets the daemon wire the launcher, the Operator and the loops as a real run does (there is no `herdr` on the PATH of the
    /// scratch run, so nothing can reach one: a spawn or a launch fails, the commands that only use the ledger work).
    pub fn launching(mut self) -> Self {
        self.launch = true;
        self
    }
    /// No git repository in the project.
    pub fn no_git(mut self) -> Self {
        self.git = false;
        self
    }
    /// Leaves the daemon stopped; the test calls [`World::start`].
    pub fn stopped(mut self) -> Self {
        self.start = false;
        self
    }
    pub fn build(self) -> World {
        World::make(self)
    }
}

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A scratch project with its daemon.
pub struct World {
    pub root: PathBuf,
    pub project: PathBuf,
    pub operator: String,
    pub tokens: BTreeMap<String, String>,
    env: Vec<(String, String)>,
    launch: bool,
    cstan: PathBuf,
    daemon: Option<Run>,
    log: PathBuf,
}

impl World {
    pub fn builder(label: &str) -> Builder {
        Builder {
            label: label.into(),
            agents: Vec::new(),
            config: None,
            env: Vec::new(),
            git: true,
            start: true,
            launch: false,
            bare: false,
            panes: true,
        }
    }

    /// A project with a PM, two developers, a reviewer and a supervisor, and a running daemon.
    pub fn standard(label: &str) -> World {
        World::builder(label)
            .agents(&[
                agent("pm-1", "pm", "PM"),
                agent("dev-1", "developer", "Developer"),
                agent("dev-2", "developer", "Developer"),
                agent("rev-1", "reviewer", "Verifier"),
                agent("sup-1", "supervisor", "Supervisor"),
            ])
            .build()
    }

    fn make(builder: Builder) -> World {
        let root = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!(
                "capstan-suites-core-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::SeqCst)
            ));
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("proj");
        let bin = root.join("bin");
        std::fs::create_dir_all(project.join(".capstan/state")).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        assert!(
            root.to_string_lossy().len() <= 45,
            "the scratch path is short enough for the project's socket: {}",
            root.display()
        );
        // PATH holds git, a shell and a few coreutils (commands an operator proposal runs) and nothing else: no node, no python.
        for tool in [
            "git", "sh", "touch", "cat", "ls", "sleep", "pwd", "mkdir", "rm", "printf", "env",
            "true", "false", "head", "tr", "wc",
        ] {
            if let Some(found) = which(tool) {
                std::os::unix::fs::symlink(found, bin.join(tool)).unwrap();
            }
        }
        let mode = |path: &Path, bits: u32| {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(bits)).unwrap()
        };
        mode(&project.join(".capstan"), 0o700);
        mode(&project.join(".capstan/state"), 0o700);
        let bare = builder.bare;
        let mut random = [0u8; 20];
        getrandom::fill(&mut random).unwrap();
        let operator = format!(
            "operator-{}",
            random
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        let state = project.join(".capstan/state");
        let config = json!({
            "schemaVersion": 1,
            "projectId": format!("p{}", COUNTER.load(Ordering::SeqCst)),
            "name": builder.label,
            "stateDirectory": state.to_string_lossy(),
            "maxSlices": 4,
            "maxRunMs": 3_600_000,
            "maxDispatches": 16,
        });
        for (name, text) in [
            ("project.json", format!("{config:#}\n")),
            ("operator.key", format!("{operator}\n")),
        ] {
            let file = project.join(".capstan").join(name);
            std::fs::write(&file, text).unwrap();
            mode(&file, 0o600);
        }
        if let Some(toml) = &builder.config {
            let file = project.join("capstan.toml");
            std::fs::write(&file, toml).unwrap();
            mode(&file, 0o600);
        }
        if builder.bare {
            std::fs::remove_dir_all(project.join(".capstan")).unwrap();
        }
        let mut world = World {
            log: root.join("daemon.log"),
            cstan: cstan_binary(),
            root,
            project,
            operator,
            tokens: BTreeMap::new(),
            env: builder.env,
            launch: builder.launch,
            daemon: None,
        };
        if builder.git {
            world.git_init();
        }
        if !bare {
            world.seed(&builder.agents, &config);
            if builder.launch && builder.git && builder.panes {
                // A launching daemon ends an agent that has no pane row (a crashed spawn), as it should.
                let head = world.git(&["rev-parse", "HEAD"]);
                for spec in &builder.agents {
                    let worktree = world.root.join("wt").join(&spec.id);
                    world.record_pane(&spec.id, &format!("feat/{}", spec.id), &worktree, &head);
                }
            }
        }
        if builder.start {
            world.start();
        }
        world
    }

    /// Seeds roles, seats, actors and agents through the kernel, with real time and randomness.
    fn seed(&mut self, agents: &[AgentSpec], config: &Value) {
        if agents.is_empty() {
            return;
        }
        let state = self.project.join(".capstan/state");
        let initial = capstan_kernel::types::InitialProject {
            project_id: config["projectId"].as_str().unwrap().to_string(),
            name: config["name"].as_str().unwrap().to_string(),
            owner_credential: self.operator.clone(),
            initial_inputs: capstan_daemon::run::placeholder_inputs(),
        };
        if self.project.join("capstan.toml").is_file() {
            // The roles come from the file, as `cstan config sync` writes them; the project record must exist first.
            Core::open(
                &state,
                &initial,
                &KernelOptions {
                    workspace_root: Some(self.project.clone()),
                    ..KernelOptions::default()
                },
                Box::new(SystemEnv),
            )
            .expect("the project is created")
            .close();
            self.op(&["config", "sync"]).ok();
        }
        let core = Core::open(
            &state,
            &initial,
            &KernelOptions {
                workspace_root: Some(self.project.clone()),
                ..KernelOptions::default()
            },
            Box::new(SystemEnv),
        )
        .expect("the seeding controller opens");
        let step = |credential: &str, op: &str, args: Value| -> Value {
            let version = core.state_version().unwrap();
            let context = json!({
                "credential": credential,
                "requestId": format!("req-{op}-{version}"),
                "idempotencyKey": format!("idem-{op}-{version}"),
                "expectedVersion": version,
                "inputRevision": core.input_revision().unwrap(),
            });
            let mut all = vec![context];
            all.extend(args.as_array().cloned().unwrap());
            core.dispatch(op, &all)
                .unwrap_or_else(|e| panic!("seeding {op}: {e}"))
        };
        let configured = self.project.join("capstan.toml").is_file();
        let roles: Vec<Value> = ROLES
            .iter()
            .enumerate()
            .map(|(i, (name, kind))| {
                json!({"name": name, "kind": kind, "host": "claude", "configHash": format!("{:x}", i + 1).repeat(64)})
            })
            .collect();
        if !configured {
            step(&self.operator, "syncRoleDefinitions", json!([roles]));
        }
        for spec in agents {
            let seat = format!("seat-{}", spec.id);
            step(
                &self.operator,
                "createSeat",
                json!([{"seatId": seat, "name": seat, "role": spec.kind}]),
            );
            let actor = step(
                &self.operator,
                "createActor",
                json!([{"displayName": spec.id, "role": spec.kind, "seatId": seat}]),
            );
            step(
                &self.operator,
                "registerAgent",
                json!([{"agentId": spec.id, "roleName": spec.role, "seatId": seat, "actorId": actor["actorId"]}]),
            );
            self.tokens.insert(
                spec.id.clone(),
                actor["credential"].as_str().unwrap().to_string(),
            );
        }
        core.close();
    }

    /// Records the pane, worktree path, branch and base of an agent the way a spawn does (for `report` and the commit checks).
    pub fn record_pane(&self, agent: &str, branch: &str, worktree: &Path, base_sha: &str) {
        self.with_core(|core, owner| {
            let token = owner.to_string();
            let version = core.state_version().unwrap();
            let context = json!({
                "credential": token,
                "requestId": format!("req-pane-{version}"),
                "idempotencyKey": format!("idem-pane-{version}"),
                "expectedVersion": version,
                "inputRevision": core.input_revision().unwrap(),
            });
            core.dispatch(
                "recordAgentPane",
                &[
                    context,
                    json!({"agentId": agent, "workspaceId": "ws-1", "paneId": format!("pane-{agent}"),
                           "worktreePath": worktree.to_string_lossy(), "branch": branch, "baseSha": base_sha}),
                ],
            )
            .unwrap_or_else(|e| panic!("recording the pane of {agent}: {e}"));
        });
    }

    /// Runs `f` on a controller opened on the ledger of a world whose daemon is stopped.
    pub fn with_core<T>(&self, f: impl FnOnce(&Core, &str) -> T) -> T {
        assert!(
            self.daemon.is_none(),
            "the ledger is seeded only while the daemon is stopped"
        );
        let config: Value = serde_json::from_str(
            &std::fs::read_to_string(self.project.join(".capstan/project.json")).unwrap(),
        )
        .unwrap();
        let initial = capstan_kernel::types::InitialProject {
            project_id: config["projectId"].as_str().unwrap().to_string(),
            name: config["name"].as_str().unwrap().to_string(),
            owner_credential: self.operator.clone(),
            initial_inputs: capstan_daemon::run::placeholder_inputs(),
        };
        let core = Core::open(
            &self.project.join(".capstan/state"),
            &initial,
            &KernelOptions {
                workspace_root: Some(self.project.clone()),
                ..KernelOptions::default()
            },
            Box::new(SystemEnv),
        )
        .expect("the seeding controller opens");
        let result = f(&core, &self.operator);
        core.close();
        result
    }

    /// Stops the daemon, runs `f` on the ledger with a controller (the operator's context maker is `ctx`), and starts the
    /// daemon again: how a test puts a ledger state in place that only a launcher or a Herdr could make.
    pub fn while_stopped<T>(
        &mut self,
        f: impl FnOnce(&Core, &dyn Fn(&str) -> capstan_kernel::types::MutationContext, &str) -> T,
    ) -> T {
        let was_running = self.daemon.is_some();
        self.stop();
        let result = self.with_core(|core, owner| {
            let context = |credential: &str| {
                capstan_daemon::deps::new_context(core, credential).expect("a context")
            };
            f(core, &context, owner)
        });
        if was_running {
            self.start();
        }
        result
    }

    /// Gives `agent` a branch `feat/<agent>` in a worktree of its own, with one commit changing `file`, and records them as its
    /// pane row (what a spawn leaves); returns the commit and the worktree. The daemon is restarted around the ledger write.
    pub fn give_branch(
        &mut self,
        agent: &str,
        file: &str,
        content: &str,
        message: &str,
    ) -> (String, PathBuf) {
        let base = self.git(&["rev-parse", "main"]);
        let branch = format!("feat/{agent}");
        let (commit, worktree) = self.commit_on_branch(&branch, file, content, message);
        self.add_commit(agent, &branch, &worktree, &base);
        (commit, worktree)
    }

    fn add_commit(&mut self, agent: &str, branch: &str, worktree: &Path, base: &str) {
        let was_running = self.daemon.is_some();
        self.stop();
        self.record_pane(agent, branch, worktree, base);
        if was_running {
            self.start();
        }
    }

    /// What `cstan request-review` records once its reviewer is running: a review of `subject` by `reviewer`, begun by
    /// `requester` (the part a launcher cannot do here is the spawn itself).
    pub fn begin_review(
        &mut self,
        requester: &str,
        subject: &str,
        reviewer_role: &str,
        reviewer: &str,
    ) -> Value {
        let credential = self.token(requester).to_string();
        self.while_stopped(|core, ctx, _| {
            core.begin_review(&ctx(&credential), &json!({"subjectId": subject, "reviewerRole": reviewer_role, "reviewerAgentId": reviewer}))
                .unwrap_or_else(|e| panic!("beginning the review of {subject}: {e}"))
        })
    }

    /// Replaces the agent's generation in the ledger (what a launcher `replace` does first): the old token stops working and
    /// the new one is kept in `tokens`; returns the old token.
    pub fn replace_generation(&mut self, agent: &str) -> String {
        let old = self.token(agent).to_string();
        let fresh = self.while_stopped(|core, ctx, owner| {
            core.replace_agent_generation(&ctx(owner), agent)
                .expect("the generation is replaced")["credential"]
                .as_str()
                .expect("a new credential")
                .to_string()
        });
        self.tokens.insert(agent.to_string(), fresh);
        old
    }

    /// Points the agent's pane row at another branch and base (a replacement's new branch).
    pub fn move_pane(&mut self, agent: &str, branch: &str, worktree: &Path, base: &str) {
        self.add_commit(agent, branch, worktree, base);
    }

    // ------------------------------------------------------------------------------------------------- git

    fn git_env(command: &mut Command, root: &Path) {
        command
            .env_clear()
            .env("PATH", root.join("bin"))
            .env("HOME", root)
            .env("LC_ALL", "C")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Black Box")
            .env("GIT_AUTHOR_EMAIL", "blackbox@example.invalid")
            .env("GIT_COMMITTER_NAME", "Black Box")
            .env("GIT_COMMITTER_EMAIL", "blackbox@example.invalid");
    }

    /// `git <args>` in `dir` (the project by default); panics on failure; returns trimmed stdout.
    pub fn git_in(&self, dir: &Path, args: &[&str]) -> String {
        let mut command = Command::new(self.root.join("bin/git"));
        command.args(args).current_dir(dir);
        Self::git_env(&mut command, &self.root);
        let output = command.output().expect("git runs");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    pub fn git(&self, args: &[&str]) -> String {
        self.git_in(&self.project, args)
    }

    fn git_init(&self) {
        self.git(&["init", "--quiet", "--initial-branch=main"]);
        std::fs::write(self.project.join(".git/info/exclude"), ".capstan/\n").unwrap();
        std::fs::write(self.project.join("a.txt"), "a\n").unwrap();
        self.git(&["add", "--all"]);
        self.git(&["commit", "--quiet", "-m", "chore: initial commit"]);
    }

    /// A branch with one commit changing `file` to `content`, made in a worktree of its own at `<root>/wt/<name>`; returns
    /// (the commit id, the worktree path). The project stays on main.
    pub fn commit_on_branch(
        &self,
        branch: &str,
        file: &str,
        content: &str,
        message: &str,
    ) -> (String, PathBuf) {
        let worktree = self.root.join("wt").join(branch.replace('/', "-"));
        std::fs::create_dir_all(worktree.parent().unwrap()).unwrap();
        self.git(&[
            "worktree",
            "add",
            "--quiet",
            "-b",
            branch,
            worktree.to_str().unwrap(),
            "main",
        ]);
        std::fs::write(worktree.join(file), content).unwrap();
        self.git_in(&worktree, &["add", "--all"]);
        self.git_in(&worktree, &["commit", "--quiet", "-m", message]);
        (self.git_in(&worktree, &["rev-parse", "HEAD"]), worktree)
    }

    // ------------------------------------------------------------------------------------------------- the daemon

    pub fn socket(&self) -> PathBuf {
        self.project.join(".capstan/state/control.sock")
    }

    /// The environment of every child: scratch HOME and PATH, no Node, nothing of the operator's.
    pub fn environment(&self) -> Vec<(String, String)> {
        let mut env = vec![
            (
                "PATH".to_string(),
                self.root.join("bin").to_string_lossy().into_owned(),
            ),
            ("HOME".to_string(), self.root.to_string_lossy().into_owned()),
            ("LC_ALL".to_string(), "C".to_string()),
            ("TERM".to_string(), "dumb".to_string()),
        ];
        if !self.launch {
            env.push(("CAPSTAN_LAUNCH".to_string(), "off".to_string()));
        }
        env.extend(self.env.iter().cloned());
        env
    }

    /// Starts `cstan daemon` and waits until it answers `ping`.
    pub fn start(&mut self) {
        if self.daemon.as_mut().is_some_and(|d| d.try_wait().is_some()) {
            self.daemon = None; // it died (kill -9 in a test); its socket and pid file are what a crash leaves
        }
        assert!(self.daemon.is_none(), "the daemon is already running");
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log)
            .unwrap();
        let child = Command::new(&self.cstan)
            .arg("daemon")
            .current_dir(&self.project)
            .env_clear()
            .envs(self.environment())
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .process_group(0)
            .spawn()
            .expect("cstan daemon starts");
        self.daemon = Some(Run::new(child));
        until("the daemon to answer", 30, || {
            if let Some(status) = self.daemon.as_mut().and_then(Run::try_wait) {
                panic!("the daemon exited ({status}): {}", self.daemon_log());
            }
            (self.socket().exists() && self.cstan_at(&self.project, None, &["ping"]).code == 0)
                .then_some(())
        });
    }

    /// Starts `cstan daemon` where it is expected to refuse; returns its exit code (the daemon is not kept).
    pub fn start_expecting_failure(&mut self) -> i32 {
        let out = self.cstan_with(&self.project, None, &["daemon"], &[], None);
        out.code
    }

    /// Stops the daemon with `cstan stop` and waits for it to end.
    pub fn stop(&mut self) {
        let Some(mut daemon) = self.daemon.take() else {
            return;
        };
        let _ = self.cstan_at(&self.project, None, &["stop"]);
        let deadline = Instant::now() + Duration::from_secs(20);
        while daemon.try_wait().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        if daemon.try_wait().is_none() {
            daemon.terminate(5);
        }
    }

    pub fn restart(&mut self) {
        self.stop();
        self.start();
    }

    pub fn daemon_pid(&self) -> Option<u32> {
        self.daemon.as_ref().map(Run::pid)
    }

    pub fn daemon_alive(&mut self) -> bool {
        self.daemon.as_mut().is_some_and(|d| d.try_wait().is_none())
    }

    pub fn daemon_log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// The daemon's log (its standard output: one JSON object per line), parsed.
    pub fn command_log(&self) -> Vec<Value> {
        let mut entries = Vec::new();
        entries.extend(
            self.daemon_log()
                .lines()
                .filter_map(|l| serde_json::from_str::<Value>(l).ok()),
        );
        entries
    }

    // ------------------------------------------------------------------------------------------------- running cstan

    fn cstan_at(&self, dir: &Path, agent: Option<&str>, args: &[&str]) -> Out {
        self.cstan_with(dir, agent, args, &[], None)
    }

    pub fn cstan_with(
        &self,
        dir: &Path,
        agent: Option<&str>,
        args: &[&str],
        extra: &[(&str, &str)],
        stdin: Option<&str>,
    ) -> Out {
        let mut command = Command::new(&self.cstan);
        command
            .args(args)
            .current_dir(dir)
            .env_clear()
            .envs(self.environment());
        if let Some(agent) = agent {
            // The variables an agent's shell has (the hook needs the agent id as well).
            command
                .env("CAPSTAN_TOKEN", self.token(agent))
                .env("CAPSTAN_SOCKET", self.socket())
                .env("CAPSTAN_AGENT_ID", agent);
        }
        for (name, value) in extra {
            command.env(name, value);
        }
        command.stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command.spawn().expect("cstan runs");
        if let Some(input) = stdin {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
        }
        let output = wait_with_deadline(child, 120);
        Out {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            args: args.iter().map(|a| a.to_string()).collect(),
        }
    }

    /// `cstan <args>` as the operator, in the project directory.
    pub fn op(&self, args: &[&str]) -> Out {
        self.cstan_at(&self.project, None, args)
    }

    /// `cstan <args>` as `agent`, in the project directory.
    pub fn as_agent(&self, agent: &str, args: &[&str]) -> Out {
        self.cstan_at(&self.project, Some(agent), args)
    }

    /// `cstan <args>` as `agent` in `dir` (its worktree).
    pub fn as_agent_in(&self, agent: &str, dir: &Path, args: &[&str]) -> Out {
        self.cstan_at(dir, Some(agent), args)
    }

    pub fn token(&self, agent: &str) -> &str {
        self.tokens
            .get(agent)
            .unwrap_or_else(|| panic!("no seeded agent {agent}"))
    }

    // ------------------------------------------------------------------------------------------------- raw socket

    /// Sends one frame (`bytes`, a newline is not added) and reads the reply until the daemon closes the connection or
    /// `seconds` pass; the reply as JSON when it is.
    pub fn raw(&self, bytes: &[u8], seconds: u64) -> Option<Value> {
        let mut stream = UnixStream::connect(self.socket()).expect("the socket accepts");
        stream
            .set_read_timeout(Some(Duration::from_secs(seconds)))
            .unwrap();
        stream.write_all(bytes).unwrap();
        let mut data = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    data.extend_from_slice(&buffer[..n]);
                    if data.contains(&b'\n') {
                        break;
                    }
                }
            }
        }
        serde_json::from_slice(&data).ok()
    }

    /// One wire command with a credential: the daemon's JSON answer.
    pub fn wire(&self, credential: &str, command: &str, args: &[&str]) -> Value {
        let frame = json!({"v": 1, "credential": credential, "command": command, "args": args});
        self.raw(format!("{frame}\n").as_bytes(), 30)
            .unwrap_or_else(|| panic!("{command} got no answer"))
    }

    /// A wire command as the operator.
    pub fn wire_op(&self, command: &str, args: &[&str]) -> Value {
        self.wire(&self.operator.clone(), command, args)
    }

    /// A wire command as `agent`.
    pub fn wire_as(&self, agent: &str, command: &str, args: &[&str]) -> Value {
        self.wire(self.token(agent), command, args)
    }

    /// How many waits of `agent` are open right now: a read-only look at the ledger, the only way to see that a `cstan wait`
    /// process has registered with the daemon (nothing else shows an open wait).
    pub fn open_waits(&self, agent: &str) -> i64 {
        let database = self.project.join(".capstan/state/controller.sqlite");
        let Ok(connection) = rusqlite::Connection::open_with_flags(
            &database,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) else {
            return 0;
        };
        connection
            .query_row(
                "SELECT count(*) FROM agent_waits WHERE agent_id = ?1 AND ended_at IS NULL",
                [agent],
                |row| row.get(0),
            )
            .unwrap_or(0)
    }

    /// Every file under the scratch root that holds `needle` (to prove a secret is not left in a log or file).
    pub fn files_holding(&self, needle: &str) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let mut pending = vec![self.root.clone()];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_dir() && !path.ends_with(".git") {
                    pending.push(path);
                } else if kind.is_file()
                    && std::fs::read(&path).is_ok_and(|bytes| contains(&bytes, needle.as_bytes()))
                {
                    found.push(path);
                }
            }
        }
        found
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

impl Drop for World {
    fn drop(&mut self) {
        self.stop();
        // A daemon that `cstan start` detached from this world is stopped by its own command.
        if self.project.join(".capstan/state").is_dir() {
            let _ = self.cstan_at(&self.project, None, &["stop"]);
        }
        // A directory made private by a test must be writable again before it can go.
        let _ = Command::new("chmod")
            .args(["-R", "u+rwx"])
            .arg(&self.root)
            .status();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn wait_with_deadline(mut child: Child, seconds: u64) -> std::process::Output {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out = std::thread::spawn(move || {
        let mut data = Vec::new();
        let _ = stdout.read_to_end(&mut data);
        data
    });
    let err = std::thread::spawn(move || {
        let mut data = Vec::new();
        let _ = stderr.read_to_end(&mut data);
        data
    });
    let status = loop {
        if let Some(status) = child.try_wait().expect("cstan can be waited on") {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let status = child.wait().unwrap();
            panic!("cstan did not end within {seconds} s (killed, {status})");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    std::process::Output {
        status,
        stdout: out.join().unwrap(),
        stderr: err.join().unwrap(),
    }
}

fn which(program: &str) -> Option<PathBuf> {
    for dir in ["/usr/bin", "/bin", "/usr/local/bin"] {
        let candidate = Path::new(dir).join(program);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|d| d.join(program))
            .find(|c| c.is_file())
    })
}
