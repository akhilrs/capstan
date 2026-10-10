//! The `cstan` executable itself, as a process: the commands only a process does (the watch loop, the hand-over to
//! `cstan-dash`, the exit code and the streams) and a run from a clean environment with no Node anywhere.
mod support;

use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::json;
use support::*;

fn cstan(args: &[&str], cwd: &Path, env: &[(&str, &str)]) -> std::process::Output {
    Command::new(FRONT)
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .output()
        .expect("cstan runs")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn version_help_and_usage_errors_exit_as_the_node_cli_did() {
    let here = std::env::temp_dir();
    let version = cstan(&["--version"], &here, &[]);
    assert_eq!(
        text(&version.stdout),
        format!("cstan {}\n", cstan_front::VERSION)
    );
    assert_eq!(version.status.code(), Some(0));
    let help = cstan(&["help"], &here, &[]);
    assert_eq!(text(&help.stdout), format!("{}\n", cstan_front::USAGE));
    let usage = cstan(&["frobnicate"], &here, &[]);
    assert_eq!(usage.status.code(), Some(2));
    assert_eq!(
        text(&usage.stderr),
        format!("{0}\ncstan: {0}\n", cstan_front::USAGE)
    );
    let extra = cstan(&["daemon", "now"], &here, &[]);
    assert_eq!(extra.status.code(), Some(2));
    let helper = cstan(&["__restart-helper"], &here, &[]);
    assert_eq!(helper.status.code(), Some(64));
    assert_eq!(text(&helper.stderr), "usage: helper.mjs <plan.json>\n");
}

/// A scratch project with the operator files, as `cstan init` makes them.
fn project(label: &str) -> Scratch {
    let scratch = Scratch::new(label, 0);
    let root = scratch.text();
    let layout = json!({
        "dirs": ["proj/.capstan/state"],
        "files": [],
        "contents": {
            "proj/.capstan/operator.key": "fixture-operator-key-0123456789abcdefghijklmnopqrstuvwxyz\n",
            "proj/.capstan/project.json": format!("{{\n  \"schemaVersion\": 1,\n  \"projectId\": \"pfixture\",\n  \"name\": \"proj\",\n  \"stateDirectory\": \"{root}/proj/.capstan/state\",\n  \"maxSlices\": 4,\n  \"maxRunMs\": 3600000,\n  \"maxDispatches\": 16\n}}\n"),
        },
        "modes": {"proj/.capstan": "0700", "proj/.capstan/state": "0700"},
        "git": [{"dir": "proj", "commit": true}],
    });
    make_layout(&layout, &scratch.0, &root);
    scratch
}

fn stop_everything(scratch: &Scratch, env: &[(&str, &str)]) {
    let project = scratch.0.join("proj");
    let _ = cstan(&["stop"], &project, env);
    // A daemon that did not stop is the scratch project's own: found by its working directory and executable.
    support::kill_daemons_of(&project);
}

#[test]
fn start_watch_and_stop_run_against_a_real_daemon() {
    let scratch = project("binary-watch");
    let project = scratch.0.join("proj");
    let home = scratch.text();
    let env = [
        ("PATH", HOST_PATH),
        ("HOME", home.as_str()),
        ("CAPSTAN_LAUNCH", "off"),
    ];
    let started = cstan(&["start", "--json"], &project, &env);
    assert_eq!(started.status.code(), Some(0), "{}", text(&started.stderr));
    assert!(
        text(&started.stdout).contains("\"started\": true"),
        "{}",
        text(&started.stdout)
    );

    let mut watch = Command::new(FRONT)
        .args(["status", "--watch", "--interval", "1"])
        .current_dir(&project)
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = watch.stdout.take().unwrap();
    let mut seen = String::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut buffer = [0u8; 256];
    while !seen.contains("\n---\n") && Instant::now() < deadline {
        let n = stdout.read(&mut buffer).unwrap();
        if n == 0 {
            break;
        }
        seen.push_str(&String::from_utf8_lossy(&buffer[..n]));
    }
    let _ = watch.kill();
    let _ = watch.wait();
    assert_eq!(seen, "agents: none\nmessages: none unresolved\n---\n");

    let stopped = cstan(&["stop"], &project, &env);
    assert_eq!(text(&stopped.stdout), "running: false\nresult: stopped\n");
    stop_everything(&scratch, &env);
}

/// The pane the launcher's hub opens: `cd <project root> && exec cstan status --watch`, in a shell that has no agent
/// variables. The first frame comes, and a SIGTERM ends the watch as the signal ends it (no exit code, no output).
#[test]
fn the_hubs_status_watch_prints_its_first_frame_and_ends_on_sigterm() {
    use std::os::unix::process::ExitStatusExt;
    let scratch = project("binary-hub");
    let project = scratch.0.join("proj");
    let home = scratch.text();
    let env = [
        ("PATH", HOST_PATH),
        ("HOME", home.as_str()),
        ("CAPSTAN_LAUNCH", "off"),
    ];
    let started = cstan(&["start"], &project, &env);
    assert_eq!(started.status.code(), Some(0), "{}", text(&started.stderr));
    // With agent variables of another project in the environment the watch still uses the operator's own credential.
    let mut hub_env = env.to_vec();
    hub_env.push(("CAPSTAN_TOKEN", "tok-someone-else"));
    hub_env.push(("CAPSTAN_SOCKET", "/nonexistent/control.sock"));
    let mut watch = Command::new(FRONT)
        .args(["status", "--watch"])
        .current_dir(&project)
        .env_clear()
        .envs(hub_env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = watch.stdout.take().unwrap();
    let mut seen = String::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut buffer = [0u8; 256];
    while !seen.contains("\n---\n") && Instant::now() < deadline {
        let n = stdout.read(&mut buffer).unwrap();
        if n == 0 {
            break;
        }
        seen.push_str(&String::from_utf8_lossy(&buffer[..n]));
    }
    assert_eq!(seen, "agents: none\nmessages: none unresolved\n---\n");
    // SAFETY: the process is the child started above.
    unsafe {
        libc::kill(watch.id() as libc::pid_t, libc::SIGTERM);
    }
    let status = watch.wait().unwrap();
    let mut rest = String::new();
    let _ = stdout.read_to_string(&mut rest);
    let mut errors = String::new();
    let _ = watch.stderr.take().unwrap().read_to_string(&mut errors);
    assert_eq!(
        status.signal(),
        Some(libc::SIGTERM),
        "ended by the signal, not by an exit"
    );
    assert_eq!((rest.as_str(), errors.as_str()), ("", ""));
    stop_everything(&scratch, &env);
}

const PTY_RUNNER: &str = r#"
import os, pty, sys
pid, fd = pty.fork()
if pid == 0:
    os.execvpe(sys.argv[1], sys.argv[1:], os.environ)
out = b""
while True:
    try:
        data = os.read(fd, 65536)
    except OSError:
        break
    if not data:
        break
    out += data
_, status = os.waitpid(pid, 0)
sys.stdout.buffer.write(out)
sys.stdout.flush()
sys.exit(os.waitstatus_to_exitcode(status))
"#;

#[test]
fn dash_replaces_the_process_with_cstan_dash_on_a_terminal() {
    if !Path::new("/usr/bin/python3").exists() {
        eprintln!("no python3: the terminal run is skipped");
        return;
    }
    let scratch = project("binary-dash");
    let project = scratch.0.join("proj");
    let stub = scratch.0.join("bin/cstan-dash");
    std::fs::create_dir_all(stub.parent().unwrap()).unwrap();
    std::fs::write(
        &stub,
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'cstan-dash 9.9.9'; exit 0; fi\nprintf 'dash:%s\\n' \"$@\"\nprintf 'credential:%s\\n' \"$CSTAN_DASH_CREDENTIAL\"\nexit 7\n",
    )
    .unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let home = scratch.text();
    let dash_bin = stub.to_string_lossy().into_owned();
    let env = [
        ("PATH", HOST_PATH),
        ("HOME", home.as_str()),
        ("CAPSTAN_LAUNCH", "off"),
        ("CSTAN_DASH_BIN", dash_bin.as_str()),
    ];
    let run = Command::new("/usr/bin/python3")
        .args(["-c", PTY_RUNNER, FRONT, "dash", "--interval", "3"])
        .current_dir(&project)
        .env_clear()
        .envs(env)
        .output()
        .unwrap();
    let out = text(&run.stdout);
    assert!(out.contains("dash:--socket\r\n"), "{out}");
    assert!(out.contains("dash:--interval\r\ndash:3\r\n"), "{out}");
    assert!(
        out.contains("credential:fixture-operator-key-0123456789abcdefghijklmnopqrstuvwxyz\r\n"),
        "{out}"
    );
    assert_eq!(
        run.status.code(),
        Some(7),
        "the exit code of cstan-dash is the exit code of cstan dash"
    );
    stop_everything(&scratch, &env);
}
