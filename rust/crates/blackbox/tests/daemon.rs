//! `cstan daemon` as a process and over its socket (test/daemon.test.ts, daemon-launch, daemon-operator, daemon-background,
//! daemon-cost): who may send what, what the log holds, the socket and its files, the frame rules, the lock, shutdown, and
//! the commands that need a launcher or the Operator when there is none.

use capstan_blackbox::*;
use serde_json::json;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::time::Duration;

fn code(reply: &serde_json::Value) -> &str {
    reply["code"].as_str().unwrap_or("")
}

#[test]
fn the_operator_credential_is_for_operator_and_read_commands_and_an_agent_token_for_agent_commands()
{
    let w = World::standard("auth");
    // The operator reads and manages.
    assert_eq!(w.wire_op("ping", &[])["ok"], true);
    assert_eq!(w.wire_op("status", &[])["ok"], true);
    assert_eq!(
        w.wire_op("inbox", &["dev-1"])["ok"],
        true,
        "the operator may peek at an inbox"
    );
    // ...and is refused the commands only an agent runs.
    let refused = w.wire_op("report", &[&"a".repeat(40), "operator cannot report"]);
    assert_eq!(
        (refused["ok"].clone(), code(&refused)),
        (json!(false), "forbidden"),
        "{refused}"
    );
    assert!(
        refused["message"].as_str().unwrap().contains("agent token"),
        "{refused}"
    );
    let ack = w.wire_op("ack", &["no-such-message"]);
    assert_eq!(code(&ack), "forbidden", "{ack}");
    // An agent reads, sends, reports; it is refused the operator's commands.
    assert_eq!(w.wire_as("dev-1", "ping", &[])["ok"], true);
    assert_eq!(w.wire_as("dev-1", "inbox", &[])["ok"], true);
    for operator_only in ["shutdown", "launch", "pm-restart"] {
        let refused = w.wire_as("dev-1", operator_only, &[]);
        assert_eq!(
            (refused["ok"].clone(), code(&refused)),
            (json!(false), "forbidden"),
            "{operator_only}: {refused}"
        );
    }
    let spawn = w.wire_as("dev-1", "spawn", &["developer"]);
    assert_eq!(code(&spawn), "forbidden", "a worker may not spawn: {spawn}");
    // The same daemon still answers everyone.
    assert_eq!(w.wire_op("ping", &[])["ok"], true);
}

#[test]
fn a_missing_malformed_or_wrong_credential_is_unauthorized_for_every_command() {
    let w = World::standard("unauth");
    for command in [
        "ping", "status", "inbox", "send", "report", "plan", "op", "spawn", "shutdown",
    ] {
        for credential in ["", "nope", "operator-0000", &"x".repeat(500)] {
            let reply = w.wire(credential, command, &[]);
            assert_eq!(
                code(&reply),
                "unauthorized",
                "{command} with {credential:.10?}: {reply}"
            );
            assert_eq!(reply["message"], "credential not accepted");
        }
    }
    // A frame with no credential at all.
    let frame = json!({"v": 1, "command": "ping", "args": []});
    let reply = w.raw(format!("{frame}\n").as_bytes(), 10).unwrap();
    assert_eq!(code(&reply), "unauthorized", "{reply}");
}

#[test]
fn tokens_never_appear_in_status_the_log_or_any_file_and_the_log_holds_no_arguments() {
    let w = World::standard("secrets");
    let body = "BODY-SECRET-0123456789-never-logged";
    w.as_agent("pm-1", &["send", "dev-1", body]).ok();
    let status = w.op(&["status", "--json"]).ok();
    let inspected = w.as_agent("dev-1", &["inbox"]).ok();
    for agent in ["pm-1", "dev-1", "dev-2", "rev-1", "sup-1"] {
        let token = w.token(agent);
        assert!(
            !status.has(token) && !inspected.has(token),
            "{agent}'s token is not printed"
        );
        let holders = w.files_holding(token);
        assert!(holders.is_empty(), "{agent}'s token is in {holders:?}");
    }
    // The ledger holds the message (it is data), but the daemon's log does not hold it, nor the operator's key.
    assert!(
        !w.daemon_log().contains(body),
        "the log holds no argument text"
    );
    assert!(
        !w.daemon_log().contains(&w.operator),
        "the log holds no credential"
    );
    let sends: Vec<_> = w
        .command_log()
        .into_iter()
        .filter(|e| e["command"] == "send")
        .collect();
    assert_eq!(sends.len(), 1);
    assert_eq!(sends[0]["role"], "PM");
    assert_eq!(sends[0]["code"], "ok");
    assert_eq!(sends[0]["argCount"], 2);
    assert_eq!(sends[0]["argBytes"], "dev-1".len() + body.len());
    assert!(
        sends[0]["actorId"].is_string() && sends[0]["ms"].is_number() && sends[0]["ts"].is_string()
    );
    // Every line of the log is JSON.
    assert!(
        w.daemon_log()
            .lines()
            .all(|line| serde_json::from_str::<serde_json::Value>(line).is_ok()),
        "{}",
        w.daemon_log()
    );
}

#[test]
fn the_socket_is_private_and_the_state_directory_too() {
    let w = World::standard("socketmode");
    assert_eq!(
        std::fs::metadata(w.socket()).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(w.project.join(".capstan/state"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert!(std::os::unix::fs::FileTypeExt::is_socket(
        &std::fs::metadata(w.socket()).unwrap().file_type()
    ));
}

#[test]
fn a_stale_socket_of_a_dead_daemon_is_replaced_and_a_file_or_link_at_the_path_is_left_alone() {
    let mut w = World::standard("stale");
    let pid = w.daemon_pid().unwrap();
    // SAFETY: the pid is this world's own daemon.
    unsafe { kill(pid as i32, 9) };
    until("the daemon to die", 10, || {
        (!w.daemon_alive()).then_some(())
    });
    assert!(w.socket().exists(), "kill -9 leaves the socket file behind");
    w.restart();
    assert_eq!(
        w.wire_op("ping", &[])["ok"],
        true,
        "the new daemon answers on the old path"
    );
    w.stop();
    // A regular file at the path stops the daemon and is left as it was.
    std::fs::remove_file(w.socket()).ok();
    std::fs::write(w.socket(), "not a socket").unwrap();
    let start = w.start_expecting_failure();
    assert_ne!(start, 0, "the daemon refuses to start over a file");
    assert_eq!(std::fs::read_to_string(w.socket()).unwrap(), "not a socket");
    // So does a symlink.
    std::fs::remove_file(w.socket()).unwrap();
    let target = w.root.join("elsewhere");
    std::fs::write(&target, "x").unwrap();
    std::os::unix::fs::symlink(&target, w.socket()).unwrap();
    assert_ne!(w.start_expecting_failure(), 0);
    assert!(w.socket().is_symlink() && std::fs::read_to_string(&target).unwrap() == "x");
}

extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}

#[test]
fn frames_are_limited_to_64_kib_a_newline_ends_one_and_bad_input_is_answered() {
    let w = World::standard("frames");
    let ping = |credential: &str| {
        let frame = json!({"v": 1, "credential": credential, "command": "ping", "args": []});
        frame.to_string()
    };
    // A second frame after the newline is discarded; only the first is answered.
    let two = format!("{{not json\n{}", ping("x"));
    let reply = w.raw(two.as_bytes(), 10).unwrap();
    assert_eq!(
        (code(&reply), reply["message"].as_str().unwrap()),
        ("invalid_request", "request is not valid JSON"),
        "{reply}"
    );
    // A carriage return before the newline is whitespace.
    let crlf = format!("{}\r\n", ping("nope"));
    assert_eq!(code(&w.raw(crlf.as_bytes(), 10).unwrap()), "unauthorized");
    // A lone newline is an empty frame; a byte-order mark is not JSON.
    assert_eq!(code(&w.raw(b"\n", 10).unwrap()), "invalid_request");
    let bom = format!("\u{feff}{}\n", ping(&w.operator));
    assert_eq!(code(&w.raw(bom.as_bytes(), 10).unwrap()), "invalid_request");
    // Empty arguments are refused as invalid requests rather than run.
    let empty = json!({"v": 1, "credential": w.operator, "command": "status", "args": [""]});
    assert_eq!(
        code(&w.raw(format!("{empty}\n").as_bytes(), 10).unwrap()),
        "invalid_request"
    );
    // 65537 bytes without a newline drop the connection without an answer.
    let long = vec![b'a'; 65537];
    assert!(
        w.raw(&long, 10).is_none(),
        "an oversized frame gets no answer"
    );
    // The daemon is fine afterwards.
    assert_eq!(w.wire_op("ping", &[])["ok"], true);
}

#[test]
fn a_frame_may_arrive_in_pieces_and_a_silent_connection_is_dropped_after_five_seconds() {
    let w = World::standard("pieces");
    let frame = format!(
        "{}\n",
        json!({"v": 1, "credential": w.operator, "command": "ping", "args": []})
    );
    let (head, tail) = frame.as_bytes().split_at(10);
    let mut stream = UnixStream::connect(w.socket()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.write_all(head).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    stream.write_all(tail).unwrap();
    let mut answer = Vec::new();
    let mut buffer = [0u8; 1024];
    while !answer.contains(&b'\n') {
        let n = stream.read(&mut buffer).unwrap();
        assert!(n > 0, "closed before the answer");
        answer.extend_from_slice(&buffer[..n]);
    }
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&answer).unwrap()["ok"],
        true
    );
    // A connection that sends nothing is closed by the daemon (not left to the client).
    let mut silent = UnixStream::connect(w.socket()).unwrap();
    silent
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let mut nothing = [0u8; 16];
    assert_eq!(
        silent.read(&mut nothing).unwrap_or(0),
        0,
        "the daemon closes a silent connection"
    );
}

#[test]
fn shutdown_needs_the_operator_and_ends_the_daemon_leaving_no_socket_or_pid_file() {
    let mut w = World::standard("shutdown");
    let pid = w.daemon_pid().unwrap();
    assert_eq!(code(&w.wire_as("pm-1", "shutdown", &[])), "forbidden");
    assert_eq!(
        w.wire_op("ping", &[])["ok"],
        true,
        "the refused shutdown did nothing"
    );
    let reply = w.wire_op("shutdown", &[]);
    assert_eq!(reply["ok"], true, "{reply}");
    until("the daemon to exit", 20, || {
        (!w.daemon_alive()).then_some(())
    });
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    assert!(!w.socket().exists() && !w.project.join(".capstan/state/daemon.pid").exists());
}

#[test]
fn a_second_daemon_on_a_held_project_does_not_start() {
    let w = World::standard("lock");
    let second = w.cstan_with(&w.project, None, &["daemon"], &[], None);
    assert_ne!(second.code, 0, "{}", second.text());
    assert_eq!(
        second.code,
        4,
        "the project lock is held: {}",
        second.text()
    );
    assert_eq!(
        w.wire_op("ping", &[])["ok"],
        true,
        "the first daemon is untouched"
    );
}

#[test]
fn without_a_launcher_the_management_commands_say_not_configured() {
    let w = World::standard("nolaunch");
    for (who, args) in [
        (None, vec!["spawn", "developer"]),
        (None, vec!["release", "dev-1"]),
        (None, vec!["replace", "dev-1"]),
        (Some("pm-1"), vec!["observe", "dev-1"]),
        (Some("pm-1"), vec!["prompt", "show", "dev-1"]),
    ] {
        let out = match who {
            Some(a) => w.as_agent(a, &args),
            None => w.op(&args),
        };
        assert_eq!(out.code, 4, "{args:?}: {}", out.text());
        assert!(
            out.stderr.contains("not_configured"),
            "{args:?}: {}",
            out.text()
        );
    }
    let launch = w.wire_op("launch", &[]);
    assert_eq!(code(&launch), "not_configured", "{launch}");
    assert_eq!(code(&w.wire_op("pm-restart", &[])), "not_configured");
    // The commands that only use the ledger work.
    w.as_agent("pm-1", &["send", "dev-1", "hello"]).ok();
    w.op(&["status"]).ok();
}

#[test]
fn a_launching_daemon_without_herdr_keeps_its_pane_less_pm_and_stays_up() {
    let w = World::builder("launchfail")
        .config(TEAM_CONFIG)
        .launching()
        .agents(&[
            agent("pm-1", "pm", "PM"),
            agent("dev-1", "developer", "Developer"),
        ])
        .build();
    // The PM's pane row names a pane no Herdr knows: launch says so instead of starting a second PM.
    let launch = w.wire_op("launch", &[]);
    assert_eq!(launch["ok"], true, "{launch}");
    assert_eq!(launch["result"]["state"], "needs_restart", "{launch}");
    assert_eq!(launch["result"]["agentId"], "pm-1");
    assert!(
        launch["result"]["hint"]
            .as_str()
            .unwrap()
            .contains("cstan pm restart"),
        "{launch}"
    );
    // A restart needs Herdr, which is not there: it is refused with a reason, and the daemon goes on.
    let restart = w.wire_op("pm-restart", &[]);
    assert_eq!(restart["ok"], false, "no herdr is on the PATH: {restart}");
    assert!(
        restart["message"].as_str().is_some_and(|m| !m.is_empty()),
        "{restart}"
    );
    assert_eq!(w.wire_op("ping", &[])["ok"], true, "the daemon is still up");
    // The seats of the team are in the status.
    let status = w.op(&["status", "--json"]).ok().json();
    let seats: Vec<&str> = status["roles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["role"].as_str().unwrap())
        .collect();
    assert!(
        seats.contains(&"PM") && seats.contains(&"Developer"),
        "{seats:?}"
    );
    // A worker's token cannot spawn, release or launch; the operator is refused a spawn with the PM missing from Herdr.
    assert_eq!(
        code(&w.wire_as("dev-1", "spawn", &["developer"])),
        "forbidden"
    );
    assert_eq!(code(&w.wire_as("dev-1", "release", &["pm-1"])), "forbidden");
}

#[test]
fn without_operator_enabled_op_is_not_configured_and_no_restart_directory_exists() {
    let w = World::builder("noop")
        .config("schema_version = 1\n[hosts.claude]\nkind = \"claude\"\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n[roles.developer]\nkind = \"Developer\"\nhost = \"claude\"\n")
        .launching()
        .agents(&[agent("pm-1", "pm", "PM")])
        .build();
    let op = w.op(&["op", "grants"]);
    assert_eq!(op.code, 4, "{}", op.text());
    assert!(
        op.stderr.contains("not_configured") && op.stderr.contains("[operator]"),
        "{}",
        op.stderr
    );
    assert!(
        !w.project.join(".capstan/restart").exists()
            && !w.project.join(".capstan/state/restart").exists()
    );
    let leftovers: Vec<_> = std::fs::read_dir(w.project.join(".capstan"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("restart") || name.contains("snapshot"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn an_idle_daemon_writes_nothing_to_its_log_and_status_inbox_and_inspect_agree_on_a_populated_ledger(
) {
    let w = World::standard("idle");
    for round in 0..30 {
        w.as_agent(
            "pm-1",
            &[
                "send",
                if round % 2 == 0 { "dev-1" } else { "dev-2" },
                &format!("message {round}"),
            ],
        )
        .ok();
    }
    let before = w.command_log().len();
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(
        w.command_log().len(),
        before,
        "an idle daemon logs no command"
    );
    let status = w.op(&["status", "--json"]).ok().json();
    let again = w.op(&["status", "--json"]).ok().json();
    assert_eq!(
        status["stateVersion"], again["stateVersion"],
        "reading changes nothing"
    );
    let inbox = w.as_agent("dev-1", &["inbox", "--json"]).ok().json();
    assert_eq!(inbox["count"], 15);
    let peek = w.op(&["inbox", "dev-2"]).ok().json_or_text();
    assert!(peek.contains("message 1"), "{peek}");
    // inspect needs an id; the usage exit code otherwise.
    assert_eq!(w.op(&["inspect"]).code, 2);
}

trait JsonOrText {
    fn json_or_text(&self) -> String;
}

impl JsonOrText for Out {
    fn json_or_text(&self) -> String {
        self.stdout.clone()
    }
}
