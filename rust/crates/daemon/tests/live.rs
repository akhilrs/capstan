//! `cstan daemon` against a real Herdr server in a throwaway session (`capstan-test-*`), driven the way a person drives it:
//! a PM is launched, workers are spawned, replaced and released, mail is delivered, the PM is woken, the daemon is
//! restarted through the Operator, and stopped. A stand-in for Claude Code (test/fixtures/fake-claude.py) draws the real
//! screens, so no model runs. Nothing starts Node: the daemon is `cstan daemon` and the commands are the ones the Rust
//! front end serves (see the harness, rust/crates/herdr/tests/common/live_env.rs).
//!
//! Run by hand, one at a time, after `cargo build -p cstan-front -p capstan-daemon`:
//! `CSTAN_LIVE=1 cargo test -p capstan-daemon --test live -- --ignored --test-threads=1`
//! It needs herdr, python3 and git; without CSTAN_LIVE=1 (or without them, or a built `cstan`) a test says
//! `SKIPPED LOUDLY` and passes. It never touches the operator's own Herdr sessions.

#[path = "../../herdr/tests/common/live_env.rs"]
mod live_env;

use live_env::{process_alive, text, until, Live, Options};
use serde_json::Value;
use std::path::{Path, PathBuf};

fn real(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap()
}

#[test]
#[ignore = "drives a real Herdr server; run by hand: CSTAN_LIVE=1 cargo test -p capstan-daemon --test live -- --ignored"]
fn the_rust_daemon_runs_a_project_in_a_real_herdr_session_and_leaves_nothing_behind() {
    let Some(mut live) = Live::start(&Options::default()) else {
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
    let unrelated = live.pane_ids();
    assert_eq!(unrelated.len(), 1);

    // cstan daemon: the Rust daemon, and a PM launched in a pane that waits at the trust dialog.
    let pid = live.start_daemon();
    assert_eq!(
        real(Path::new(&format!("/proc/{pid}/exe"))),
        real(&live.cstan),
        "the daemon is cstan daemon"
    );
    let pong = live.cstan_ok(&["ping"]);
    assert_eq!(pong["pid"].as_u64().unwrap() as u32, pid, "{pong}");
    let launched = live.launch_pm();
    assert_eq!(launched["state"], "blocked", "{launched}");
    assert_eq!(launched["agentId"], "pm-1");
    let pm_pane = launched["paneId"].as_str().unwrap().to_string();
    live.answer_trust_dialog(&pm_pane);

    // spawn: a worktree on its own branch at the project's HEAD, a pane, an agent.
    let head = live.git_ok(&["rev-parse", "HEAD"]);
    let spawned = live.cstan_ok(&["spawn", "developer"]);
    assert_eq!(spawned["state"], "started", "{spawned}");
    assert_eq!(spawned["agentId"], "developer-1");
    assert_eq!(spawned["branch"], "chore/developer-1-developer");
    let worktree = PathBuf::from(spawned["worktreePath"].as_str().unwrap());
    assert!(worktree.is_dir());
    assert_eq!(
        live.git_ok(&["rev-parse", "chore/developer-1-developer"]),
        head
    );
    // The worker limit refuses the next spawn with the launcher's own code.
    let refused = live.cstan(&["spawn", "developer"]);
    assert!(!refused.status.success());
    assert!(
        text(&refused).contains("worker_limit"),
        "{}",
        text(&refused)
    );
    let worker_pane = spawned["paneId"].as_str().unwrap().to_string();
    live.wait_status(&worker_pane, "idle", 30);

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
    let remaining = live.pane_ids();
    assert!(remaining.contains(&pm_pane), "{remaining:?}");
    for pane in &unrelated {
        assert!(remaining.contains(pane), "{pane} survived: {remaining:?}");
    }
    assert!(live.pane_of("developer-1").is_none() && live.pane_of("developer-2").is_none());

    // Stopping the daemon: it is gone and so are its socket and pid file.
    live.stop_daemon();
    assert!(!process_alive(pid));
    assert!(!live.socket().exists());
    assert!(!live.state().join("daemon.pid").exists());
    let listing = std::process::Command::new("herdr")
        .args(["session", "list"])
        .env_clear()
        .envs(&live.environment)
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&listing.stdout).contains(&live.session));
    // The suite leaves no session, no pane and no directory under /tmp.
    live.close();
}

#[test]
#[ignore = "drives a real Herdr server; run by hand: CSTAN_LIVE=1 cargo test -p capstan-daemon --test live -- --ignored"]
fn an_operator_restart_replaces_the_daemon_and_the_clients_reconnect() {
    let Some(mut live) = Live::start(&Options {
        max_workers: 2,
        operator: true,
        ..Options::default()
    }) else {
        return;
    };
    let first = live.start_daemon();
    let launched = live.launch_pm();
    let pm_pane = launched["paneId"].as_str().unwrap().to_string();
    live.answer_trust_dialog(&pm_pane);
    let spawned = live.cstan_ok(&["spawn", "operator"]);
    assert_eq!(spawned["state"], "started", "{spawned}");
    let operator_token = until("the operator's token", 20, || {
        live.token_in(Path::new(spawned["worktreePath"].as_str().unwrap()))
    });
    let pm_token = until("the PM's token", 20, || live.token_in(&live.repo));

    // The restart needs a known-good copy of the daemon's binary (here `cstan`, which runs `cstan daemon`); the daemon takes
    // one after a settle time, so take it now.
    capstan_operator::restart::KnownGoodBuild::new(&live.state())
        .snapshot(&live.cstan, capstan_ledger::max_embedded_migration())
        .expect("the known-good snapshot");

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
        live.pid_file_pid()
            .filter(|pid| *pid != first && process_alive(*pid))
    });
    assert_eq!(
        real(Path::new(&format!("/proc/{second}/exe"))),
        real(&live.cstan),
        "the new daemon is cstan daemon"
    );
    until("the restart proposal to finish", 60, || {
        let shown = live.cstan_ok(&["op", "show", &id]);
        (shown["state"] == "finished").then_some(shown)
    });
    // The front end reconnects, and the ledger is valid.
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
    drop(connection);

    // The front end runs unchanged against the new daemon.
    let status = live.cstan_ok(&["status"]);
    assert_eq!(
        status["controller"]["pid"].as_u64().unwrap() as u32,
        second,
        "{status}"
    );
    let agents: Vec<&str> = status["agents"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["agentId"].as_str())
        .collect();
    assert!(
        agents.contains(&"pm-1") && agents.contains(&"operator-1"),
        "{agents:?}"
    );

    // cstan-dash is drawn into a terminal for a moment: one frame shows the agents. It lives in dash/ and is built apart.
    let target = live.cstan.parent().unwrap().to_path_buf();
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
        let output = std::process::Command::new("python3")
            .args(["-c", script])
            .arg(&dash)
            .arg(live.socket())
            .arg(credential.trim())
            .env_clear()
            .envs(&live.environment)
            .output()
            .unwrap();
        assert!(output.status.success(), "cstan-dash: {}", text(&output));
        let frame = String::from_utf8_lossy(&output.stdout);
        assert!(frame.contains("pm-1"), "one frame shows the PM");
        assert!(frame.contains("operator-1"), "one frame shows the operator");
    } else {
        eprintln!("cstan-dash is not built beside cstan; that step did not run");
    }
    live.stop_daemon();
    live.close();
}
