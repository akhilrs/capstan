//! A project whose in-project socket path is over the system limit: `cstan` reaches the listener at the resolved runtime
//! path and its agent guard compares against that path. No daemon runs; fake listeners stand in for it.
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use cstan_front::controllers::{Live, Probe, Reply};
use cstan_front::projects::socket_of;
use serde_json::json;

const FRONT: &str = env!("CARGO_BIN_EXE_cstan");
const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef";

struct Long {
    root: PathBuf,
}

impl Long {
    /// A project root whose `<root>/.capstan/state/control.sock` is over the limit.
    fn new(tag: &str) -> Long {
        let mut root = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("cstan-long-{tag}-{}", std::process::id()));
        let mut part = 0;
        while root.join(".capstan/state/control.sock").as_os_str().len()
            <= capstan_wire::socket_path_limit()
        {
            part += 1;
            root.push(format!("segment-{part}-abcdefghijklmnopqrstuvwxyz"));
        }
        let capstan = root.join(".capstan");
        std::fs::create_dir_all(capstan.join("state")).unwrap();
        std::fs::set_permissions(&capstan, std::fs::Permissions::from_mode(0o700)).unwrap();
        let config = capstan.join("project.json");
        let state = capstan.join("state");
        std::fs::write(
            &config,
            json!({"schemaVersion": 1, "projectId": format!("long-{tag}"), "name": "long",
                "stateDirectory": state, "maxSlices": 2, "maxRunMs": 60_000, "maxDispatches": 2})
            .to_string(),
        )
        .unwrap();
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
        let key = capstan.join("operator.key");
        std::fs::write(&key, KEY).unwrap();
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
        Long { root }
    }

    fn state(&self) -> PathBuf {
        self.root.join(".capstan/state")
    }

    fn resolved(&self) -> PathBuf {
        capstan_wire::control_socket(&self.state())
    }

    /// Binds a listener at the resolved path (its private directory is made first) and counts the requests it answers.
    fn listen(&self) -> Arc<AtomicUsize> {
        let socket = self.resolved();
        capstan_wire::ensure_private_dir(socket.parent().unwrap()).unwrap();
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        let seen = Arc::new(AtomicUsize::new(0));
        let count = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let mut line = String::new();
                if BufReader::new(&stream).read_line(&mut line).is_ok() && !line.is_empty() {
                    count.fetch_add(1, Ordering::SeqCst);
                    let reply = json!({"ok": true, "result": {"projectId": "long", "pid": 1}});
                    let _ = (&stream).write_all(format!("{reply}\n").as_bytes());
                }
            }
        });
        seen
    }
}

impl Drop for Long {
    fn drop(&mut self) {
        // Only the socket file: the runtime directory is shared by every project of the user.
        let _ = std::fs::remove_file(self.resolved());
        let _ = std::fs::remove_dir_all(
            self.root
                .ancestors()
                .find(|p| {
                    p.file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with("cstan-long-"))
                })
                .unwrap(),
        );
    }
}

fn front(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(FRONT)
        .args(args)
        .current_dir(root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn the_project_socket_is_the_resolved_path() {
    let project = Long::new("path");
    let resolved = project.resolved();
    assert_ne!(resolved, project.state().join("control.sock"));
    assert_eq!(socket_of(&project.root), resolved);
}

#[test]
fn status_and_decide_reach_the_listener_at_the_resolved_path() {
    let project = Long::new("reach");
    let seen = project.listen();
    let status = front(&project.root, &["status", "--json"]);
    assert!(seen.load(Ordering::SeqCst) >= 1, "status: {status:?}");
    let before = seen.load(Ordering::SeqCst);
    let argv = [
        "decide",
        "--project",
        project.root.to_str().unwrap(),
        "op:abc",
        "--expect",
        "0123456789ab",
        "--nonce",
        "nonce-0001",
        "deny",
    ];
    let decide = front(&project.root, &argv);
    assert!(seen.load(Ordering::SeqCst) > before, "decide: {decide:?}");
}

#[test]
fn recent_reaches_the_listener_at_the_resolved_path() {
    let project = Long::new("recent");
    let seen = project.listen();
    let out = front(
        &project.root,
        &[
            "recent",
            "--project",
            project.root.to_str().unwrap(),
            "--epoch",
            "0123456789abcdef",
            "--head",
            "0",
            "--json",
        ],
    );
    assert!(seen.load(Ordering::SeqCst) >= 1, "recent: {out:?}");
}

#[test]
fn controllers_lists_the_project_as_answering() {
    let project = Long::new("controllers");
    let seen = project.listen();
    let reply = Live.ask(&project.root);
    assert!(seen.load(Ordering::SeqCst) >= 1);
    assert!(matches!(reply, Reply::Answered(_)), "{reply:?}");
}

fn guarded(project: &Long, socket: &Path, allow: bool) -> std::process::Output {
    let mut command = Command::new(FRONT);
    command
        .arg("status")
        .current_dir(&project.root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("CAPSTAN_TOKEN", "agent-token")
        .env("CAPSTAN_SOCKET", socket)
        .stdin(Stdio::null());
    if allow {
        command.env("CAPSTAN_ALLOW_FOREIGN_SOCKET", "1");
    }
    command.output().unwrap()
}

#[test]
fn the_agent_guard_accepts_the_resolved_path_and_refuses_the_rest() {
    let project = Long::new("guard");
    let other = Long::new("guard-other");
    let resolved = project.resolved();
    let accepted = guarded(&project, &resolved, false);
    assert_ne!(accepted.status.code(), Some(2), "{accepted:?}");
    for wrong in [project.state().join("control.sock"), other.resolved()] {
        let refused = guarded(&project, &wrong, false);
        assert_eq!(refused.status.code(), Some(2), "{wrong:?}: {refused:?}");
        let text = String::from_utf8_lossy(&refused.stderr);
        assert!(text.contains(resolved.to_str().unwrap()), "{text}");
        let allowed = guarded(&project, &wrong, true);
        assert_ne!(allowed.status.code(), Some(2), "{allowed:?}");
        assert!(String::from_utf8_lossy(&allowed.stderr).contains("CAPSTAN_ALLOW_FOREIGN_SOCKET=1"));
    }
}
