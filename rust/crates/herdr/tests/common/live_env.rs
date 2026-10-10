#![allow(dead_code)]
//! The live-suite harness, shared by the live suites of the herdr, launcher, operator and daemon crates (each includes
//! this file with `#[path]`): a throwaway Herdr server in a `capstan-test-*` session on a scratch HOME, a scratch project,
//! the stand-in for Claude Code (test/fixtures/fake-claude.py) and `cstan daemon` running the project.
//!
//! The rules the suites rely on:
//! - Everything runs only with `CSTAN_LIVE=1`; without it (or without herdr, python3, git or a built `cstan`) a test says
//!   `SKIPPED LOUDLY: ...` and passes, so `cargo test` never touches Herdr by accident. The tests are `#[ignore]` as well.
//! - The operator's own Herdr sessions are never touched: the server runs on the scratch HOME, every Herdr command names
//!   the scratch session, and no HERDR_ or XDG_ variable is passed on.
//! - The daemon is started with `cstan daemon`; the suites use only the agent and operator commands the Rust front end
//!   serves itself (`spawn`, `send`, `release`, `replace`, `op ...`, `prompt ...`, `ping`, `status`, ...) and the three
//!   operator commands that go straight to the daemon over the socket (`launch`, `pm-restart`, `shutdown`). Nothing here
//!   starts Node.
//! - Waiting is on state (Herdr's pane list and screens, the daemon's answers, files), polled until a deadline that only a
//!   failing run reaches; no step sleeps for a fixed time.
//! - Everything lives under `/tmp/capstan-suites-live-*`, which `close` removes; `close` also checks that the session is
//!   gone from Herdr's list and that the directory is gone.
//!
//! By hand (one suite at a time; the build slot rules of the machine apply):
//! `cargo build -p cstan-front -p capstan-daemon && CSTAN_LIVE=1 cargo test -p capstan-daemon --test live -- --ignored --test-threads=1`

use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

pub fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

pub fn suffix() -> String {
    let mut bytes = [0u8; 4];
    getrandom::fill(&mut bytes).unwrap();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn available(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// The `cstan` front end: `CSTAN_BIN`, else the one beside the test executables (`target/<profile>/cstan`).
pub fn cstan_binary() -> PathBuf {
    if let Some(named) = std::env::var_os("CSTAN_BIN") {
        return PathBuf::from(named);
    }
    // target/<profile>/deps/<this test> -> target/<profile>/cstan
    std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("cstan")
}

pub fn until<T>(what: &str, seconds: u64, mut action: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        if let Some(value) = action() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

pub fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// `command.output()` with a deadline: a command that does not end in time is killed (it is a process this harness started)
/// and the test fails, so a hung command can never hold the run.
pub fn output_within(command: &mut Command, seconds: u64) -> Output {
    use std::io::Read;
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the command starts");
    let (mut out, mut err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let out_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = out.read_to_end(&mut bytes);
        bytes
    });
    let err_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = err.read_to_end(&mut bytes);
        bytes
    });
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let status = loop {
        if let Some(status) = child.try_wait().expect("the command can be waited for") {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("a command did not end within {seconds} seconds: {command:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    Output {
        status,
        stdout: out_reader.join().unwrap(),
        stderr: err_reader.join().unwrap(),
    }
}

pub fn process_alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map(|stat| !stat.contains(") Z "))
        .unwrap_or(false)
}

/// What a suite asks of the project.
pub struct Options {
    pub max_workers: u32,
    /// `[layout] spawn`: `"tab"` or `"pane"`.
    pub layout: &'static str,
    /// Whether `[operator]` is on, with the operator role.
    pub operator: bool,
    /// `[prompt_relay]` on.
    pub prompt_relay: bool,
    /// The agents run the real `claude` on the scratch HOME, to which the operator's Claude sign-in and the Playwright
    /// browsers are linked (never copied).
    pub linked_login: bool,
    /// A `capstan.toml` of the suite's own; `{session}` is replaced by the Herdr session. The other options then only
    /// decide the environment.
    pub config: Option<String>,
    /// Whether the suite runs the daemon, which needs the built `cstan`; a suite of the Herdr adapter alone does not.
    pub daemon: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            max_workers: 1,
            layout: "tab",
            operator: false,
            prompt_relay: false,
            linked_login: false,
            config: None,
            daemon: true,
        }
    }
}

/// The daemon this harness started. A thread reaps it the moment it exits: a daemon that is a zombie still answers
/// `kill(pid, 0)`, which would look like a daemon that did not stop (a real client leaves its daemon to init).
struct Daemon {
    child: std::sync::Arc<std::sync::Mutex<Child>>,
    exited: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pid: u32,
}

impl Daemon {
    fn start(child: Child) -> Daemon {
        use std::sync::atomic::Ordering;
        let pid = child.id();
        let child = std::sync::Arc::new(std::sync::Mutex::new(child));
        let exited = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (watched, flag) = (child.clone(), exited.clone());
        std::thread::spawn(move || loop {
            if watched
                .lock()
                .map(|mut c| c.try_wait().ok().flatten().is_some())
                .unwrap_or(true)
            {
                flag.store(true, Ordering::SeqCst);
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        });
        Daemon { child, exited, pid }
    }

    fn exited(&self) -> bool {
        self.exited.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// A process this harness started.
    fn kill(&self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
    }
}

pub struct Live {
    pub root: PathBuf,
    pub repo: PathBuf,
    pub session: String,
    pub environment: HashMap<String, String>,
    pub cstan: PathBuf,
    pub credential: String,
    server: Option<Child>,
    daemon: Option<Daemon>,
    /// What the daemon (and so every agent) gets as its environment: the scratch one, with the operator's PATH when the real claude is used.
    agents_env: Option<HashMap<String, String>>,
    closed: bool,
}

impl Live {
    /// The scratch project and its isolated Herdr server, or `None` (with the reason printed) when this machine or this
    /// run cannot do it.
    pub fn start(options: &Options) -> Option<Live> {
        let skip = |reason: &str| {
            eprintln!("SKIPPED LOUDLY: live suite did not run: {reason}");
            None
        };
        if std::env::var("CSTAN_LIVE").as_deref() != Ok("1") {
            return skip("set CSTAN_LIVE=1 to run it");
        }
        for program in ["herdr", "python3", "git"] {
            if !available(program) {
                return skip(&format!("{program} is not available"));
            }
        }
        let cstan = cstan_binary();
        if options.daemon && !cstan.is_file() {
            return skip(&format!(
                "no cstan at {}: run cargo build -p cstan-front (or set CSTAN_BIN)",
                cstan.display()
            ));
        }
        if options.linked_login {
            if !available("claude") {
                return skip("claude is not available");
            }
            let home = std::env::var("HOME").unwrap_or_default();
            if !Path::new(&home).join(".claude").exists() {
                return skip("no Claude Code login in the operator's home");
            }
            if !Path::new(&home).join(".claude.json").exists() {
                return skip(".claude.json is missing, so Claude Code is not signed in");
            }
        }
        let fixtures = repository().join("test/fixtures");
        let root = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("capstan-suites-live-{}", suffix()));
        assert!(
            root.to_string_lossy().len() <= 50,
            "the scratch path is short enough for the project's socket"
        );
        let (home, bin, repo) = (root.join("home"), root.join("bin"), root.join("repo"));
        for directory in [&home, &bin, &repo] {
            std::fs::create_dir_all(directory).unwrap();
        }
        for file in [".bashrc", ".bash_profile"] {
            std::fs::write(home.join(file), "PS1='❯ '\n").unwrap();
        }
        let python = String::from_utf8(
            Command::new("python3")
                .args(["-c", "import sys;print(sys.executable)"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        let fake = std::fs::read_to_string(fixtures.join("fake-claude.py")).unwrap();
        let body = fake.split_once('\n').map_or("", |(_, rest)| rest);
        let claude = bin.join("claude");
        std::fs::write(&claude, format!("#!{}\n{body}", python.trim())).unwrap();
        std::fs::set_permissions(&claude, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        std::fs::copy(
            fixtures.join("claude-trust-dialog.txt"),
            bin.join("claude-trust-dialog.txt"),
        )
        .unwrap();

        // The watch pane runs `cstan status --watch`, the one interactive command the front end does not serve itself yet.
        // A stand-in that stays up takes its place (as test/launcher-live's watch-cli.js did), so the suites need no Node.
        let watch = root.join("watch-stub");
        // Any other command that reaches it was handed to Node, which these suites never do: it fails at once.
        std::fs::write(
            &watch,
            "#!/bin/sh\nif [ \"$1\" = status ] && [ \"$2\" = --watch ]; then echo WATCH-PANE-RUNNING; exec sleep 3600; fi\necho \"cstan: handed to Node: $*\" >&2\nexit 5\n",
        )
        .unwrap();
        std::fs::set_permissions(&watch, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let mut environment: HashMap<String, String> = std::env::vars()
            .filter(|(name, _)| {
                !name.starts_with("HERDR_")
                    && !name.starts_with("XDG_")
                    && !name.starts_with("CAPSTAN_")
                    && !name.starts_with("CSTAN_")
            })
            .collect();
        let operators_home = environment.get("HOME").cloned().unwrap_or_default();
        let operators_path = environment.get("PATH").cloned().unwrap_or_default();
        environment.insert("HOME".into(), home.to_string_lossy().into_owned());
        // The stand-in is first on PATH, unless the real claude is asked for.
        let path = if options.linked_login {
            operators_path.clone()
        } else {
            format!("{}:{operators_path}", bin.display())
        };
        environment.insert("PATH".into(), path);
        environment.insert("TERM".into(), "xterm-256color".into());
        environment.insert(
            "CSTAN_NODE_CLI".into(),
            watch.to_string_lossy().into_owned(),
        );
        // The scratch home's rc files set the prompt the adapter waits for, so panes must run bash.
        environment.insert("SHELL".into(), "/bin/bash".into());
        environment.insert(
            "FAKE_CLAUDE_FIXTURES".into(),
            fixtures.to_string_lossy().into_owned(),
        );
        environment.insert(
            "FAKE_CLAUDE_ARGS".into(),
            root.join("args.log").to_string_lossy().into_owned(),
        );
        environment.insert(
            "FAKE_CLAUDE_LOG".into(),
            root.join("messages.log").to_string_lossy().into_owned(),
        );
        // Herdr's own server runs on the scratch home, and so does the daemon: it finds the session where the server made it.
        let server_environment = environment.clone();
        if options.linked_login {
            environment.insert("PATH".into(), operators_path);
        }
        if options.linked_login {
            use std::os::unix::fs::symlink;
            for name in [".claude", ".claude.json"] {
                symlink(Path::new(&operators_home).join(name), home.join(name)).unwrap();
            }
            let browsers = Path::new(&operators_home).join(".cache/ms-playwright");
            if browsers.exists() {
                std::fs::create_dir_all(home.join(".cache")).unwrap();
                symlink(browsers, home.join(".cache/ms-playwright")).unwrap();
            }
        }

        let session = format!("capstan-test-{}", suffix());
        let server = Command::new("herdr")
            .args(["--session", &session, "server"])
            .env_clear()
            .envs(&server_environment)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .expect("herdr starts");
        let mut credential_bytes = [0u8; 20];
        getrandom::fill(&mut credential_bytes).unwrap();
        let credential = format!(
            "operator-{}",
            credential_bytes
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        let live = Live {
            root,
            repo,
            session,
            environment: server_environment,
            cstan,
            credential,
            server: Some(server),
            daemon: None,
            agents_env: Some(environment),
            closed: false,
        };
        until("the isolated Herdr server", 20, || {
            live.herdr(&["workspace", "list"]).ok()
        });
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec![
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        ] {
            let status = live.git(&args);
            assert!(status.status.success(), "{}", text(&status));
        }
        live.write_project(options);
        Some(live)
    }

    fn write_project(&self, options: &Options) {
        use std::os::unix::fs::PermissionsExt;
        let capstan = self.repo.join(".capstan");
        let state = capstan.join("state");
        std::fs::create_dir(&capstan).unwrap();
        std::fs::set_permissions(&capstan, std::fs::Permissions::from_mode(0o700)).unwrap();
        let project = serde_json::json!({
            "schemaVersion": 1,
            "projectId": format!("plive{}", suffix()),
            "name": "Live suite",
            "stateDirectory": state.to_string_lossy(),
            "maxSlices": 4,
            "maxRunMs": 3_600_000,
            "maxDispatches": 16,
        });
        for (name, body) in [
            ("project.json", format!("{project:#}\n")),
            ("operator.key", format!("{}\n", self.credential)),
        ] {
            let file = capstan.join(name);
            std::fs::write(&file, body).unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        std::fs::create_dir(&state).unwrap();
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
        // The project directory is not part of the repository's content.
        std::fs::write(self.repo.join(".git/info/exclude"), ".capstan/\n").unwrap();
        if let Some(config) = &options.config {
            std::fs::write(
                self.repo.join("capstan.toml"),
                config.replace("{session}", &self.session),
            )
            .unwrap();
            return;
        }
        let mut config = format!(
            r#"schema_version = 1
herdr_session = "{session}"

[notifications]
herdr = false
fallback = true

[timers]
pm_wake_after_seconds = 2
pm_wake_interval_seconds = 10

[limits]
max_workers = {max_workers}

[layout]
spawn = "{layout}"
min_pane_columns = 40
min_pane_rows = 12

[nexora]
track = "never"

[hosts.claude]
kind = "claude"

[roles.pm]
kind = "PM"
host = "claude"

[roles.developer]
kind = "Developer"
host = "claude"
"#,
            session = self.session,
            max_workers = options.max_workers,
            layout = options.layout
        );
        if options.operator {
            config.push_str(
                r#"
[operator]
enabled = true

[roles.operator]
kind = "Developer"
host = "claude"
deny = ["Write", "Edit", "NotebookEdit", "Agent", "Task", "Read", "Glob", "Grep"]
"#,
            );
        }
        if options.prompt_relay {
            config.push_str("\n[prompt_relay]\nenabled = true\n");
        }
        std::fs::write(self.repo.join("capstan.toml"), config).unwrap();
    }

    /// The name Herdr knows an agent by: the project's slug (its directory name) and the agent id.
    pub fn herdr_agent_name(&self, agent: &str) -> String {
        format!(
            "{}-{agent}",
            self.repo.file_name().unwrap().to_string_lossy()
        )
    }

    pub fn state(&self) -> PathBuf {
        self.repo.join(".capstan/state")
    }

    pub fn socket(&self) -> PathBuf {
        self.state().join("control.sock")
    }

    fn git(&self, args: &[&str]) -> Output {
        Command::new("git")
            .args(args)
            .current_dir(&self.repo)
            .env_clear()
            .envs(&self.environment)
            .stdin(Stdio::null())
            .output()
            .expect("git runs")
    }

    pub fn git_ok(&self, args: &[&str]) -> String {
        let output = self.git(args);
        assert!(output.status.success(), "git {args:?}: {}", text(&output));
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn command(&self, program: &Path) -> Command {
        let mut command = Command::new(program);
        command
            .env_clear()
            .envs(&self.environment)
            .current_dir(&self.repo)
            .stdin(Stdio::null());
        command
    }

    /// `herdr --session <session> <args...>`: the parsed JSON result, or the failure text.
    pub fn herdr(&self, args: &[&str]) -> Result<Value, String> {
        let output = output_within(
            self.command(Path::new("herdr"))
                .args(["--session", &self.session])
                .args(args),
            120,
        );
        if !output.status.success() {
            return Err(text(&output));
        }
        Ok(serde_json::from_slice(&output.stdout).unwrap_or(Value::Null))
    }

    pub fn panes(&self) -> Vec<Value> {
        self.herdr(&["pane", "list"])
            .ok()
            .and_then(|v| v["result"]["panes"].as_array().cloned())
            .unwrap_or_default()
    }

    pub fn pane_ids(&self) -> Vec<String> {
        self.panes()
            .iter()
            .filter_map(|p| p["pane_id"].as_str().map(str::to_string))
            .collect()
    }

    /// The pane that carries the agent token `agent`.
    pub fn pane_of(&self, agent: &str) -> Option<String> {
        self.panes()
            .iter()
            .find(|p| p["tokens"]["agent"] == agent)
            .and_then(|p| p["pane_id"].as_str().map(str::to_string))
    }

    /// The panes that carry an agent token, as (agent, pane) pairs in agent order: the PM and the workers, not the watch tab.
    pub fn agent_panes(&self) -> Vec<(String, String)> {
        let mut found: Vec<(String, String)> = self
            .panes()
            .iter()
            .filter_map(|p| {
                Some((
                    p["tokens"]["agent"].as_str()?.to_string(),
                    p["pane_id"].as_str()?.to_string(),
                ))
            })
            .collect();
        found.sort();
        found
    }

    pub fn agent_status(&self, pane: &str) -> Option<String> {
        self.panes()
            .iter()
            .find(|p| p["pane_id"] == pane)
            .and_then(|p| p["agent_status"].as_str().map(str::to_string))
    }

    /// What the pane shows now (`herdr pane read` prints the text itself).
    pub fn screen(&self, pane: &str) -> String {
        let output = output_within(
            self.command(Path::new("herdr")).args([
                "--session",
                &self.session,
                "pane",
                "read",
                pane,
                "--source",
                "visible",
            ]),
            120,
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    pub fn wait_status(&self, pane: &str, status: &str, seconds: u64) {
        until(&format!("pane {pane} to be {status}"), seconds, || {
            (self.agent_status(pane).as_deref() == Some(status)).then_some(())
        });
    }

    pub fn answer_trust_dialog(&self, pane: &str) {
        self.herdr(&["pane", "send-keys", pane, "down"]).unwrap();
        self.herdr(&["pane", "send-keys", pane, "enter"]).unwrap();
        self.wait_status(pane, "idle", 30);
    }

    // -------------------------------------------------------------------------------------------- the daemon

    fn agent_environment(&self) -> &HashMap<String, String> {
        self.agents_env.as_ref().unwrap_or(&self.environment)
    }

    /// `cstan daemon` for the project, detached, with its output in `.capstan/daemon.log`; returns its pid once it answers.
    pub fn start_daemon(&mut self) -> u32 {
        assert!(self.daemon.is_none(), "the daemon is already started");
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.repo.join(".capstan/daemon.log"))
            .unwrap();
        let child = Command::new(&self.cstan)
            .arg("daemon")
            .env_clear()
            .envs(self.agent_environment())
            .current_dir(&self.repo)
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .process_group(0)
            .spawn()
            .expect("cstan daemon starts");
        let pid = child.id();
        self.daemon = Some(Daemon::start(child));
        until("the daemon to answer", 60, || {
            if self.daemon.as_ref().is_some_and(Daemon::exited) {
                panic!("the daemon exited: {}", self.daemon_log());
            }
            // Before the socket exists the front end would hand `ping` to Node; nothing is asked until it does.
            if !self.socket().exists() {
                return None;
            }
            self.cstan_as(&self.credential.clone(), &["ping", "--json"])
                .status
                .success()
                .then_some(())
        });
        pid
    }

    pub fn daemon_log(&self) -> String {
        std::fs::read_to_string(self.repo.join(".capstan/daemon.log")).unwrap_or_default()
    }

    pub fn daemon_pid(&self) -> Option<u32> {
        self.daemon.as_ref().map(|d| d.pid)
    }

    /// One operator command straight to the daemon over the socket: the commands the front end does not serve (`launch`,
    /// `pm-restart`, `shutdown`). The answer is the whole response object.
    pub fn wire(&self, command: &str, args: &[&str]) -> Value {
        let request = serde_json::json!({
            "v": 1,
            "credential": self.credential,
            "command": command,
            "args": args,
        });
        let mut stream = UnixStream::connect(self.socket()).expect("the daemon's socket");
        stream
            .set_read_timeout(Some(Duration::from_secs(300)))
            .unwrap();
        writeln!(stream, "{request}").unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("{command}: {e}: {line:?}"))
    }

    /// The result of an operator command that must be accepted.
    pub fn wire_ok(&self, command: &str, args: &[&str]) -> Value {
        let response = self.wire(command, args);
        assert_eq!(response["ok"], true, "{command}: {response}");
        response["result"].clone()
    }

    /// `launch`: the PM, in a pane that waits at the trust dialog.
    pub fn launch_pm(&self) -> Value {
        self.wire_ok("launch", &[])
    }

    /// The pid in the project's `daemon.pid`: the daemon that serves the socket now, which after an Operator restart is
    /// not the process this harness started.
    pub fn pid_file_pid(&self) -> Option<u32> {
        std::fs::read_to_string(self.state().join("daemon.pid"))
            .ok()
            .and_then(|text| text.trim().parse().ok())
    }

    /// Asks the daemon to stop and waits for its process (the one that serves the socket, and the one this harness
    /// started); a daemon that does not stop is a failure.
    pub fn stop_daemon(&mut self) {
        let serving = self.pid_file_pid();
        let response = self.wire("shutdown", &[]);
        assert_eq!(response["ok"], true, "shutdown: {response}");
        if let Some(daemon) = self.daemon.take() {
            until("the daemon to exit", 60, || daemon.exited().then_some(()));
        }
        if let Some(pid) = serving {
            until("the serving daemon to exit", 60, || {
                (!process_alive(pid)).then_some(())
            });
        }
    }

    /// `cstan <args>` as the token's holder, with the project socket; the working directory is the project's.
    pub fn cstan_as(&self, token: &str, args: &[&str]) -> Output {
        output_within(
            self.command(&self.cstan)
                .args(args)
                .envs(self.agent_environment())
                .env("CAPSTAN_TOKEN", token)
                .env("CAPSTAN_SOCKET", self.socket()),
            240,
        )
    }

    /// `cstan <args> --json` as the operator; the parsed result, and a failure if the command is refused.
    pub fn cstan_ok(&self, args: &[&str]) -> Value {
        self.cstan_as_ok(&self.credential.clone(), args)
    }

    pub fn cstan_as_ok(&self, token: &str, args: &[&str]) -> Value {
        let mut all = args.to_vec();
        all.push("--json");
        let output = self.cstan_as(token, &all);
        assert!(output.status.success(), "cstan {args:?}: {}", text(&output));
        serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
    }

    /// `cstan <args>` as the operator, as it is (exit status and both streams).
    pub fn cstan(&self, args: &[&str]) -> Output {
        self.cstan_as(&self.credential.clone(), args)
    }

    /// The token of the agent whose process works in `directory`: it lives in the environment of the agent.
    pub fn token_in(&self, directory: &Path) -> Option<String> {
        for entry in std::fs::read_dir("/proc").ok()?.flatten() {
            let path = entry.path();
            if std::fs::read_link(path.join("cwd")).ok().as_deref() != Some(directory) {
                continue;
            }
            let Ok(environment) = std::fs::read(path.join("environ")) else {
                continue;
            };
            for variable in environment.split(|b| *b == 0) {
                if let Some(token) = variable.strip_prefix(b"CAPSTAN_TOKEN=") {
                    return Some(String::from_utf8_lossy(token).into_owned());
                }
            }
        }
        None
    }

    // -------------------------------------------------------------------------------------------- the end

    /// What a failed run leaves to look at before the scratch directory goes: the daemon's log and every restart result.
    fn explain_failure(&self) {
        let tail = |text: String| {
            let start = text.len().saturating_sub(16000);
            text[text
                .char_indices()
                .map(|(i, _)| i)
                .find(|i| *i >= start)
                .unwrap_or(0)..]
                .to_string()
        };
        eprintln!("---- daemon.log (tail) ----\n{}", tail(self.daemon_log()));
        if let Ok(entries) = std::fs::read_dir(self.state().join("restart")) {
            for entry in entries.flatten() {
                for name in ["plan.json", "result.json", "helper.log"] {
                    if let Ok(body) = std::fs::read_to_string(entry.path().join(name)) {
                        eprintln!(
                            "---- restart/{}/{name} ----\n{}",
                            entry.file_name().to_string_lossy(),
                            tail(body)
                        );
                    }
                }
            }
        }
        eprintln!("---- panes ----\n{:?}", self.pane_ids());
    }

    fn teardown(&mut self) {
        if let Some(daemon) = self.daemon.take() {
            // A daemon this harness started; ask it to stop, then make sure.
            if self.socket().exists() {
                let _ = UnixStream::connect(self.socket()).map(|mut stream| {
                    let line = serde_json::json!({"v": 1, "credential": self.credential, "command": "shutdown", "args": []});
                    let _ = writeln!(stream, "{line}");
                    let _ = BufReader::new(stream).read_line(&mut String::new());
                });
            }
            let deadline = Instant::now() + Duration::from_secs(30);
            while !daemon.exited() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(100));
            }
            daemon.kill();
        }
        // After an Operator restart the daemon that serves the socket is the helper's, not this harness's: it was asked to
        // stop above and gets a moment to do so before the server goes.
        if let Some(pid) = self.pid_file_pid() {
            let deadline = Instant::now() + Duration::from_secs(30);
            while process_alive(pid) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        let _ = self.herdr(&["server", "stop"]);
        if let Some(mut server) = self.server.take() {
            let deadline = Instant::now() + Duration::from_secs(10);
            while server.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(100));
            }
            // The server is a process this harness started.
            let _ = server.kill();
            let _ = server.wait();
        }
        let _ = Command::new("herdr")
            .args(["session", "delete", &self.session])
            .env_clear()
            .envs(&self.environment)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    /// Ends the run and checks that it left nothing behind: no session in Herdr's list, no Herdr server, no pane, no
    /// directory under /tmp.
    pub fn close(mut self) {
        self.teardown();
        let listing = Command::new("herdr")
            .args(["session", "list"])
            .env_clear()
            .envs(&self.environment)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        assert!(
            !listing.contains(&self.session),
            "the session is removed: {listing}"
        );
        let root = self.root.clone();
        std::fs::remove_dir_all(&root).unwrap();
        self.closed = true;
        assert!(!root.exists(), "the scratch directory is removed");
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        if !self.closed {
            if std::thread::panicking() {
                self.explain_failure();
            }
            self.teardown();
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}
