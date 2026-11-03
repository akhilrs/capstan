//! A project whose in-project socket path is over the system limit, through the real `cstan`: the daemon started by `cstan start`
//! listens in the private runtime directory, the operator and agent commands reach it there, the agent guard refuses the
//! in-project path and another project's socket, `cstan stop` removes the runtime socket, and a world leaves none behind.

use capstan_blackbox::*;
use std::io::Read;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn long_world(label: &str) -> World {
    World::builder(label)
        .agents(&[
            agent("pm-1", "pm", "PM"),
            agent("dev-1", "developer", "Developer"),
        ])
        .long_path()
        .stopped()
        .build()
}

fn runtime_dir() -> PathBuf {
    // SAFETY: getuid has no preconditions and cannot fail.
    Path::new("/tmp").join(format!("capstan-{}", unsafe { libc::getuid() }))
}

fn in_project(w: &World) -> PathBuf {
    w.project.join(".capstan/state/control.sock")
}

fn pending(w: &World, agent: &str) -> i64 {
    w.op(&["inbox", agent, "--json"]).ok().json()["messages"]
        .as_array()
        .map_or(0, |m| m.len() as i64)
}

#[test]
fn the_daemon_started_by_cstan_start_answers_at_the_runtime_socket() {
    let w = long_world("longstart");
    let limit = in_project(&w).as_os_str().len();
    assert!(limit > 100, "the project path is long: {limit}");
    let start = w.op(&["start"]);
    assert_eq!(start.code, 0, "{}", start.text());
    let socket = w.socket();
    assert!(
        socket.parent() == Some(runtime_dir().as_path()),
        "{socket:?}"
    );
    assert!(socket.exists(), "the runtime socket exists");
    assert!(!in_project(&w).exists(), "nothing is bound in the project");
    assert_eq!(w.op(&["status"]).code, 0);
    assert_eq!(w.op(&["ping"]).code, 0);
    assert_eq!(w.op(&["inbox", "pm-1", "--json"]).code, 0);
}

#[test]
fn an_agent_sends_reads_waits_and_hooks_through_the_runtime_socket() {
    let mut w = long_world("longagent");
    w.start();
    w.as_agent("dev-1", &["send", "@pm", "from the long path"])
        .ok();
    let inbox = w.as_agent("pm-1", &["inbox"]).ok();
    assert!(
        inbox.stdout.contains("from the long path"),
        "{}",
        inbox.stdout
    );

    let mut waiting = Command::new(cstan_binary())
        .arg("wait")
        .current_dir(&w.project)
        .env_clear()
        .envs(w.environment())
        .env("CAPSTAN_TOKEN", w.token("dev-1"))
        .env("CAPSTAN_SOCKET", w.socket())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    until("the wait to register", 15, || {
        (w.open_waits("dev-1") == 1).then_some(())
    });
    w.as_agent("pm-1", &["send", "dev-1", "wake up"]).ok();
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(status) = waiting.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = waiting.kill();
            let _ = waiting.wait();
            panic!("the wait did not end");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(0));
    let mut out = String::new();
    waiting
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    assert!(out.contains("wake up"), "{out}");

    w.as_agent("pm-1", &["send", "dev-1", "hook me"]).ok();
    let hook = w.as_agent("dev-1", &["inbox", "--hook"]).ok();
    assert!(hook.stdout.contains("PostToolUse"), "{}", hook.stdout);
}

#[test]
fn the_guard_refuses_the_in_project_path_and_another_projects_socket() {
    let mut a = long_world("longguarda");
    a.start();
    let mut b = long_world("longguardb");
    b.start();
    assert_ne!(a.socket(), b.socket());

    let before = pending(&a, "pm-1");
    let inside = in_project(&a);
    let refused = a.cstan_with(
        &a.project,
        Some("dev-1"),
        &["send", "@pm", "never sent"],
        &[("CAPSTAN_SOCKET", inside.to_str().unwrap())],
        None,
    );
    assert_eq!(refused.code, 2, "{}", refused.text());
    assert_eq!(pending(&a, "pm-1"), before, "nothing was sent");

    let foreign = b.socket();
    let refused = a.cstan_with(
        &a.project,
        Some("dev-1"),
        &["send", "@pm", "never sent"],
        &[("CAPSTAN_SOCKET", foreign.to_str().unwrap())],
        None,
    );
    assert_eq!(refused.code, 2, "{}", refused.text());
    assert_eq!(
        pending(&a, "pm-1"),
        before,
        "nothing reached the first project"
    );
    assert_eq!(pending(&b, "pm-1"), 0, "nothing reached the other project");
}

#[test]
fn stop_removes_the_runtime_socket_and_the_next_start_works() {
    let w = long_world("longstop");
    w.op(&["start"]).ok();
    let socket = w.socket();
    assert!(socket.exists());
    w.op(&["stop"]).ok();
    until("the runtime socket to go", 20, || {
        (!socket.exists()).then_some(())
    });
    let again = w.op(&["start"]);
    assert_eq!(again.code, 0, "{}", again.text());
    assert!(socket.exists());
    assert_eq!(w.op(&["status"]).code, 0);
}

#[test]
fn a_short_world_keeps_its_socket_in_the_project() {
    let w = World::standard("shortsock");
    assert_eq!(w.socket(), in_project(&w));
    assert!(w.socket().exists());
}

#[test]
fn a_world_that_panics_leaves_no_socket_in_the_runtime_directory() {
    let seen = std::sync::Mutex::new(None::<PathBuf>);
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut w = long_world("longpanic");
        w.start();
        *seen.lock().unwrap() = Some(w.socket());
        assert!(w.socket().exists());
        panic!("a test failing midway");
    }));
    assert!(result.is_err());
    let socket = seen.lock().unwrap().take().expect("the world started");
    assert_eq!(socket.parent(), Some(runtime_dir().as_path()));
    assert!(!socket.exists(), "the socket is gone: {socket:?}");
    if let Ok(meta) = std::fs::symlink_metadata(runtime_dir()) {
        assert!(meta.is_dir(), "the runtime directory itself is kept");
    }
}
