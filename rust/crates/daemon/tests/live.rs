//! `cstan-daemon` against a real Herdr server in a throwaway session (`capstan-test-*`), driven the way a person drives it:
//! the Node `cstan` starts the Rust daemon (`[daemon] implementation = "rust"`), launches a PM, spawns, replaces and
//! releases workers, delivers mail, wakes the PM, restarts the daemon through the Operator, and stops it. A stand-in for
//! Claude Code (test/fixtures/fake-claude.py) draws the real screens, so no model runs.
//!
//! Run by hand: `npm run build && cargo test -p capstan-daemon --test live -- --ignored --test-threads=1`. It needs herdr,
//! python3, git and node; without them (or without dist/) a test says so and passes. It never touches the operator's own
//! Herdr sessions: the server runs on a scratch HOME, every command names the session, and HERDR_ variables are removed.

use serde_json::Value;
use std::collections::HashMap;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const DAEMON: &str = env!("CARGO_BIN_EXE_cstan-daemon");

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn suffix() -> String {
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

fn until<T>(what: &str, seconds: u64, mut action: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        if let Some(value) = action() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

struct Live {
    root: PathBuf,
    repo: PathBuf,
    session: String,
    environment: HashMap<String, String>,
    cli: PathBuf,
    server: Option<Child>,
}

impl Live {
    /// The scratch project and its isolated Herdr server, or `None` (with the reason printed) when this machine cannot run it.
    fn start(max_workers: u32) -> Option<Live> {
        let cli = repository().join("dist/src/cli.js");
        let fixtures = repository().join("test/fixtures");
        for (program, present) in [
            ("herdr", available("herdr")),
            ("python3", available("python3")),
            ("git", available("git")),
            ("node", available("node")),
        ] {
            if !present {
                eprintln!(
                    "SKIPPED LOUDLY: live daemon test did not run: {program} is not available"
                );
                return None;
            }
        }
        if !cli.exists() {
            eprintln!("SKIPPED LOUDLY: live daemon test did not run: run npm run build first ({} is missing)", cli.display());
            return None;
        }
        let root = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("cph-d2b-{}", suffix()));
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

        let mut environment: HashMap<String, String> = std::env::vars()
            .filter(|(name, _)| {
                !name.starts_with("HERDR_")
                    && !name.starts_with("XDG_")
                    && !name.starts_with("CAPSTAN_")
                    && !name.starts_with("CSTAN_")
            })
            .collect();
        environment.insert("HOME".into(), home.to_string_lossy().into_owned());
        environment.insert(
            "PATH".into(),
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        );
        environment.insert("TERM".into(), "xterm-256color".into());
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
        environment.insert("CSTAN_DAEMON_BIN".into(), DAEMON.into());

        let session = format!("capstan-test-{}", suffix());
        let server = Command::new("herdr")
            .args(["--session", &session, "server"])
            .env_clear()
            .envs(&environment)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .expect("herdr starts");
        let live = Live {
            root,
            repo,
            session,
            environment,
            cli,
            server: Some(server),
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
            let status = Command::new("git")
                .args(args)
                .current_dir(&live.repo)
                .status()
                .unwrap();
            assert!(status.success());
        }
        let init = live.cstan(&["init"]);
        assert!(init.status.success(), "{}", text(&init));
        let config = format!(
            r#"schema_version = 1
herdr_session = "{session}"

[daemon]
implementation = "rust"

[notifications]
herdr = false
fallback = true

[timers]
pm_wake_after_seconds = 2
pm_wake_interval_seconds = 10

[limits]
max_workers = {max_workers}

[layout]
spawn = "pane"

[nexora]
track = "never"

[operator]
enabled = true

[hosts.claude]
kind = "claude"

[roles.pm]
kind = "PM"
host = "claude"

[roles.developer]
kind = "Developer"
host = "claude"

[roles.operator]
kind = "Developer"
host = "claude"
deny = ["Write", "Edit", "NotebookEdit", "Agent", "Task", "Read", "Glob", "Grep"]
"#,
            session = live.session,
            max_workers = max_workers
        );
        std::fs::write(live.repo.join("capstan.toml"), config).unwrap();
        Some(live)
    }

    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command
            .env_clear()
            .envs(&self.environment)
            .current_dir(&self.repo)
            .stdin(Stdio::null());
        command
    }

    /// `herdr --session <session> <args...>`: the parsed JSON result, or the failure text.
    fn herdr(&self, args: &[&str]) -> Result<Value, String> {
        let output = self
            .command("herdr")
            .args(["--session", &self.session])
            .args(args)
            .output()
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(text(&output));
        }
        Ok(serde_json::from_slice(&output.stdout).unwrap_or(Value::Null))
    }

    fn cstan(&self, args: &[&str]) -> Output {
        self.command("node")
            .arg(&self.cli)
            .args(args)
            .output()
            .unwrap()
    }

    fn cstan_ok(&self, args: &[&str]) -> Value {
        let mut all = args.to_vec();
        all.push("--json");
        let output = self.cstan(&all);
        assert!(output.status.success(), "cstan {args:?}: {}", text(&output));
        serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
    }

    /// `cstan <args>` as an agent: its token and the project socket.
    fn cstan_as(&self, token: &str, args: &[&str]) -> Output {
        self.command("node")
            .arg(&self.cli)
            .args(args)
            .env("CAPSTAN_TOKEN", token)
            .env("CAPSTAN_SOCKET", self.state().join("control.sock"))
            .output()
            .unwrap()
    }

    fn state(&self) -> PathBuf {
        self.repo.join(".capstan/state")
    }

    fn panes(&self) -> Vec<Value> {
        self.herdr(&["pane", "list"])
            .ok()
            .and_then(|v| v["result"]["panes"].as_array().cloned())
            .unwrap_or_default()
    }

    /// The pane that carries the agent token `agent`.
    fn pane_of(&self, agent: &str) -> Option<String> {
        self.panes()
            .iter()
            .find(|p| p["tokens"]["agent"] == agent)
            .and_then(|p| p["pane_id"].as_str().map(str::to_string))
    }

    /// What the pane shows now (`herdr pane read` prints the text itself).
    fn screen(&self, pane: &str) -> String {
        self.command("herdr")
            .args([
                "--session",
                &self.session,
                "pane",
                "read",
                pane,
                "--source",
                "visible",
            ])
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
            .unwrap_or_default()
    }

    fn answer_trust_dialog(&self, pane: &str) {
        self.herdr(&["pane", "send-keys", pane, "down"]).unwrap();
        self.herdr(&["pane", "send-keys", pane, "enter"]).unwrap();
        until("the agent to reach its idle screen", 30, || {
            self.panes()
                .iter()
                .any(|p| p["pane_id"] == pane && p["agent_status"] == "idle")
                .then_some(())
        });
    }

    fn daemon_pid(&self) -> Option<u32> {
        std::fs::read_to_string(self.state().join("daemon.pid"))
            .ok()
            .and_then(|text| text.trim().parse().ok())
    }

    /// The token of the agent whose process works in `directory`: it lives in the environment of the stand-in agent.
    fn token_in(&self, directory: &Path) -> Option<String> {
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
}

impl Drop for Live {
    fn drop(&mut self) {
        // The daemon this test started, found through its pid file and checked to be the binary under test.
        if let Some(pid) = self.daemon_pid() {
            let ours = std::fs::read_link(format!("/proc/{pid}/exe"))
                .is_ok_and(|exe| exe == Path::new(DAEMON));
            if ours {
                let _ = self.cstan(&["stop"]);
            }
        }
        let _ = self.herdr(&["server", "stop"]);
        if let Some(mut server) = self.server.take() {
            std::thread::sleep(Duration::from_millis(500));
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
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn process_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

#[test]
#[ignore = "drives a real Herdr server; run by hand: cargo test -p capstan-daemon --test live -- --ignored"]
fn the_rust_daemon_runs_a_project_in_a_real_herdr_session_and_leaves_nothing_behind() {
    let Some(live) = Live::start(1) else {
        return;
    };
    // A workspace that is not the project's: no teardown of the daemon may touch it.
    live.herdr(&[
        "workspace",
        "create",
        "--label",
        "unrelated",
        "--cwd",
        "/tmp",
    ])
    .unwrap();
    let unrelated: Vec<String> = live
        .panes()
        .iter()
        .filter_map(|p| p["pane_id"].as_str().map(str::to_string))
        .collect();
    assert_eq!(unrelated.len(), 1);

    // cstan start: the Rust daemon, and a PM launched in a pane that waits at the trust dialog.
    let started = live.cstan_ok(&["start"]);
    assert_eq!(started["running"], true, "{started}");
    assert_eq!(started["launch"]["state"], "blocked", "{started}");
    assert_eq!(started["launch"]["agentId"], "pm-1");
    let pid = started["pid"].as_u64().unwrap() as u32;
    assert_eq!(
        std::fs::read_link(format!("/proc/{pid}/exe")).unwrap(),
        Path::new(DAEMON),
        "the daemon is cstan-daemon"
    );
    let pm_pane = started["launch"]["paneId"].as_str().unwrap().to_string();
    live.answer_trust_dialog(&pm_pane);

    // spawn: a worktree on its own branch at the project's HEAD, a pane, an agent.
    let head = String::from_utf8(
        live.command("git")
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let spawned = live.cstan_ok(&["spawn", "developer"]);
    assert_eq!(spawned["state"], "started", "{spawned}");
    assert_eq!(spawned["agentId"], "developer-1");
    assert_eq!(spawned["branch"], "chore/developer-1-developer");
    let worktree = PathBuf::from(spawned["worktreePath"].as_str().unwrap());
    assert!(worktree.is_dir());
    let branch_tip = String::from_utf8(
        live.command("git")
            .args(["rev-parse", "chore/developer-1-developer"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(branch_tip, head);
    // The worker limit refuses the next spawn with the launcher's own code.
    let refused = live.cstan(&["spawn", "developer"]);
    assert!(!refused.status.success());
    assert!(
        text(&refused).contains("worker_limit"),
        "{}",
        text(&refused)
    );
    let worker_pane = spawned["paneId"].as_str().unwrap().to_string();
    until("the worker to be idle", 30, || {
        live.panes()
            .iter()
            .any(|p| p["pane_id"] == worker_pane.as_str() && p["agent_status"] == "idle")
            .then_some(())
    });

    // Mail delivery to the worker, and the PM wake for mail the PM has not read.
    let sent = live.cstan_ok(&["send", "developer-1", "build slice one"]);
    let message_id = sent["messageId"].as_str().unwrap().to_string();
    until("the worker to show the message", 30, || {
        let screen = live.screen(&worker_pane);
        (screen.contains("build slice one") && screen.contains(&message_id)).then_some(())
    });
    live.cstan_ok(&["send", "pm-1", "report to the PM"]);
    until("the PM wake line", 30, || {
        live.screen(&pm_pane)
            .contains("Run cstan inbox")
            .then_some(())
    });

    // replace: a successor on the same branch, the predecessor's worktree gone.
    let replaced = live.cstan_ok(&["replace", "developer-1"]);
    assert_eq!(replaced["state"], "started", "{replaced}");
    assert_eq!(replaced["agentId"], "developer-2");
    assert_eq!(replaced["predecessor"], "developer-1");
    assert_eq!(replaced["predecessorWorktreeRemoved"], true);
    // release: the pane closes and the worktree and the branch go.
    let released = live.cstan_ok(&["release", "developer-2"]);
    assert_eq!(released["state"], "released", "{released}");
    assert_eq!(released["paneClosed"], true);
    assert_eq!(released["worktreeRemoved"], true);
    assert!(!worktree.exists());

    // Teardown closed only the panes of the released workers: the PM and the unrelated workspace are there.
    let remaining: Vec<String> = live
        .panes()
        .iter()
        .filter_map(|p| p["pane_id"].as_str().map(str::to_string))
        .collect();
    assert!(remaining.contains(&pm_pane), "{remaining:?}");
    for pane in &unrelated {
        assert!(remaining.contains(pane), "{pane} survived: {remaining:?}");
    }
    assert!(live.pane_of("developer-1").is_none() && live.pane_of("developer-2").is_none());

    // cstan stop: the daemon is gone and so are its socket and pid file.
    let stopped = live.cstan_ok(&["stop"]);
    assert_eq!(stopped["running"], false, "{stopped}");
    until("the daemon to exit", 20, || {
        (!process_alive(pid)).then_some(())
    });
    assert!(!live.state().join("control.sock").exists());
    assert!(!live.state().join("daemon.pid").exists());
    let listing = String::from_utf8(
        live.command("herdr")
            .args(["session", "list"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(listing.contains(&live.session));
    let session = live.session.clone();
    let environment = live.environment.clone();
    drop(live);
    let after = String::from_utf8(
        Command::new("herdr")
            .args(["session", "list"])
            .env_clear()
            .envs(&environment)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(!after.contains(&session), "the session is removed: {after}");
}

#[test]
#[ignore = "drives a real Herdr server; run by hand: cargo test -p capstan-daemon --test live -- --ignored"]
fn an_operator_restart_replaces_the_daemon_and_the_clients_reconnect() {
    let Some(live) = Live::start(2) else {
        return;
    };
    let started = live.cstan_ok(&["start"]);
    let first = started["pid"].as_u64().unwrap() as u32;
    let pm_pane = started["launch"]["paneId"].as_str().unwrap().to_string();
    live.answer_trust_dialog(&pm_pane);
    let spawned = live.cstan_ok(&["spawn", "operator"]);
    assert_eq!(spawned["state"], "started", "{spawned}");
    let operator_token = until("the operator's token", 20, || {
        live.token_in(Path::new(spawned["worktreePath"].as_str().unwrap()))
    });
    let pm_token = until("the PM's token", 20, || live.token_in(&live.repo));

    // The restart needs a known-good copy of this binary; the daemon takes one after a settle time, so take it now.
    let snapshot = capstan_operator::restart::KnownGoodBuild::new(&live.state())
        .snapshot(Path::new(DAEMON), capstan_ledger::max_embedded_migration());
    snapshot.expect("the known-good snapshot");

    let proposed = live.cstan_as(
        &operator_token,
        &["op", "propose", "--restart", "--json", "end to end"],
    );
    assert!(proposed.status.success(), "{}", text(&proposed));
    let proposal: Value = serde_json::from_slice(&proposed.stdout).unwrap();
    let (id, hash) = (
        proposal["proposalId"].as_str().unwrap().to_string(),
        proposal["hash"].as_str().unwrap().to_string(),
    );
    let decided = live.cstan_as(
        &pm_token,
        &["op", "decide", &id, "approve", "--hash", &hash],
    );
    assert!(decided.status.success(), "{}", text(&decided));

    let second = until("the restarted daemon", 120, || {
        live.daemon_pid()
            .filter(|pid| *pid != first && process_alive(*pid))
    });
    assert_eq!(
        std::fs::read_link(format!("/proc/{second}/exe")).unwrap(),
        Path::new(DAEMON)
    );
    until("the restart proposal to finish", 60, || {
        let shown = live.cstan_ok(&["op", "show", &id]);
        (shown["state"] == "finished").then_some(shown)
    });
    // The Node client reconnects, and the ledger is valid.
    let pong = live.cstan_ok(&["ping"]);
    assert_eq!(pong["pid"].as_u64().unwrap() as u32, second, "{pong}");
    let connection = rusqlite::Connection::open_with_flags(
        live.state().join("controller.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let check: String = connection
        .query_row("pragma integrity_check", [], |row| row.get(0))
        .unwrap();
    assert_eq!(check, "ok");

    // The Rust front end and cstan-dash run unchanged against the new daemon. cstan-dash is drawn into a terminal for
    // a moment: one frame shows the agents.
    let target = Path::new(DAEMON).parent().unwrap().to_path_buf();
    let front = target.join("cstan");
    if front.exists() {
        let output = live
            .command(front.to_str().unwrap())
            .env("CSTAN_NODE_CLI", &live.cli)
            .args(["status", "--json"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", text(&output));
        let status: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(status["schemaVersion"], 1);
    } else {
        eprintln!(
            "the Rust cstan front end is not built beside cstan-daemon; that step did not run"
        );
    }
    let dash = target.join("cstan-dash");
    if dash.exists() {
        let credential = std::fs::read_to_string(live.repo.join(".capstan/operator.key")).unwrap();
        let script = r#"
import fcntl, os, pty, select, struct, sys, termios, time
dash, socket, credential = sys.argv[1:4]
pid, fd = pty.fork()
if pid == 0:
    os.environ["CSTAN_DASH_CREDENTIAL"] = credential
    os.execv(dash, ["cstan-dash", "--socket", socket, "--interval", "1", "--no-color"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
out = b""
end = time.time() + 4
while time.time() < end:
    ready, _, _ = select.select([fd], [], [], 0.3)
    if ready:
        try:
            out += os.read(fd, 65536)
        except OSError:
            break
os.write(fd, b"q")
time.sleep(0.5)
try:
    out += os.read(fd, 65536)
except OSError:
    pass
_, status = os.waitpid(pid, 0)
sys.stdout.write(out.decode("utf-8", "replace"))
sys.exit(os.waitstatus_to_exitcode(status))
"#;
        let output = live
            .command("python3")
            .args(["-c", script])
            .arg(&dash)
            .arg(live.state().join("control.sock"))
            .arg(credential.trim())
            .output()
            .unwrap();
        assert!(output.status.success(), "cstan-dash: {}", text(&output));
        let frame = String::from_utf8_lossy(&output.stdout);
        assert!(frame.contains("pm-1"), "one frame shows the PM");
        assert!(frame.contains("operator-1"), "one frame shows the operator");
    } else {
        eprintln!("cstan-dash is not built beside cstan-daemon; that step did not run");
    }
}
