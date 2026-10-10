//! The `cstan` executable as an operator or an agent uses it (test/cli.test.ts, commands.test.ts): usage and exit codes, `init`,
//! `config check` and `config sync`, `start`/`stop`/`ping`, the agent environment and the daemon's life around a command.
//! Every case runs the built binary in a scratch project with no Node on PATH.

use capstan_blackbox::*;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn pid_of(out: &Out) -> u64 {
    out.json()["pid"].as_u64().expect("a pid")
}

fn alive(pid: u64) -> bool {
    // SAFETY: signal 0 only asks whether the process exists.
    unsafe { libc_kill(pid as i32, 0) == 0 }
}

extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, signal: i32) -> i32;
}

fn gone(what: &str, pid: u64) {
    until(what, 20, || (!alive(pid)).then_some(()));
}

#[test]
fn version_help_and_usage_errors_exit_with_their_codes() {
    let w = World::builder("usage").bare().build();
    let version = w.op(&["--version"]).ok();
    assert!(version.stdout.starts_with("cstan "), "{}", version.stdout);
    assert_eq!(w.op(&["version"]).ok().stdout, version.stdout);
    for flag in ["help", "--help", "-h"] {
        let help = w.op(&[flag]).ok();
        assert!(
            help.stdout.starts_with("usage: cstan init | cstan start"),
            "{}",
            help.stdout
        );
    }
    let unknown = w.op(&["frobnicate"]);
    assert_eq!(unknown.code, 2, "{}", unknown.text());
    assert!(unknown.stderr.contains("usage: cstan init"));
    assert_eq!(w.op(&[]).code, 2, "no command is a usage error");
    assert_eq!(
        w.op(&["daemon", "now"]).code,
        2,
        "daemon takes no arguments"
    );
    // The usage line names the plan subcommands.
    assert!(w
        .op(&["help"])
        .stdout
        .contains("cstan plan open normal|high-risk <title>"));
}

#[test]
fn the_scratch_environment_has_no_node_and_the_native_cli_needs_none() {
    let w = World::builder("nonode").bare().build();
    let env = w.environment();
    assert!(
        env.iter().all(|(name, _)| !name.starts_with("CSTAN_NODE")),
        "{env:?}"
    );
    // `node` does not resolve in the PATH every child gets.
    let probe = std::process::Command::new("/bin/sh")
        .args(["-c", "command -v node || command -v nodejs"])
        .env_clear()
        .envs(env.iter().cloned())
        .output()
        .unwrap();
    assert!(
        !probe.status.success(),
        "node resolved: {}",
        String::from_utf8_lossy(&probe.stdout)
    );
    // The commands an operator runs first all work.
    w.op(&["init"]).ok();
    w.op(&["config", "check"]).ok();
    w.op(&["herdr-config"]).ok();
    w.op(&["status", "--json"]).ok();
}

#[test]
fn init_creates_a_private_project_a_starter_config_and_refuses_to_change_it() {
    let w = World::builder("init").bare().build();
    let created = w.op(&["init"]).ok();
    assert!(
        created.stdout.contains("Wrote starter capstan.toml"),
        "{}",
        created.stdout
    );
    assert!(
        created.stdout.contains("Initialized Capstan project"),
        "{}",
        created.stdout
    );
    assert_eq!(mode(&w.project.join(".capstan")), 0o700);
    assert_eq!(mode(&w.project.join(".capstan/operator.key")), 0o600);
    assert_eq!(mode(&w.project.join(".capstan/project.json")), 0o600);
    assert!(w.project.join("capstan.toml").is_file());
    assert!(w.project.join("roles/designer.md").is_file());
    // The key is the one `init` printed a path for, and git does not stage it.
    let key = std::fs::read_to_string(w.project.join(".capstan/operator.key")).unwrap();
    assert!(
        key.trim().len() >= 32 && !created.has(key.trim()),
        "the credential is not printed"
    );
    assert!(!w.git(&["status", "--short"]).contains(".capstan"));
    // A second init changes nothing and exits 3.
    let config = std::fs::read(w.project.join("capstan.toml")).unwrap();
    w.op(&["init"])
        .refused(3, "refusing to modify existing contents");
    assert_eq!(
        std::fs::read(w.project.join("capstan.toml")).unwrap(),
        config
    );
    // status answers from the new project (the daemon it starts syncs the starter roles).
    let status = w.op(&["status", "--json"]).ok().json();
    assert_eq!(status["schemaVersion"], 1);
    assert!(status["projectId"]
        .as_str()
        .is_some_and(|id| id.starts_with('p')));
}

#[test]
fn init_keeps_an_existing_capstan_toml_and_a_dangling_symlink() {
    let w = World::builder("init-keep").bare().build();
    std::fs::write(
        w.project.join("capstan.toml"),
        "# mine\nschema_version = 1\n",
    )
    .unwrap();
    let created = w.op(&["init"]).ok();
    assert!(
        !created.stdout.contains("Wrote starter capstan.toml"),
        "{}",
        created.stdout
    );
    assert_eq!(
        std::fs::read_to_string(w.project.join("capstan.toml")).unwrap(),
        "# mine\nschema_version = 1\n"
    );
    assert!(
        !w.project.join("roles").exists(),
        "no role file is written next to a kept capstan.toml"
    );

    let w = World::builder("init-dangling").bare().build();
    std::os::unix::fs::symlink("/nonexistent/capstan.toml", w.project.join("capstan.toml"))
        .unwrap();
    w.op(&["init"]).ok();
    assert!(
        w.project.join("capstan.toml").is_symlink(),
        "the dangling link is kept"
    );
    assert!(
        w.project.join(".capstan/operator.key").is_file(),
        "init finished"
    );
}

#[test]
fn config_check_exits_3_for_a_missing_or_invalid_file_and_never_echoes_a_secret() {
    let w = World::builder("check").bare().build();
    w.op(&["config", "check"])
        .refused(3, "capstan.toml does not exist");
    let secret = "sk-SECRET-VALUE-0123456789";
    std::fs::write(
        w.project.join("capstan.toml"),
        format!("schema_version = 1\napi_token = \"{secret}\"\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n"),
    )
    .unwrap();
    let invalid = w.op(&["config", "check"]);
    assert_eq!(invalid.code, 3, "{}", invalid.text());
    assert!(
        !invalid.has(secret),
        "the secret is not echoed: {}",
        invalid.text()
    );
    std::fs::write(w.project.join("capstan.toml"), "schema_version = [\n").unwrap();
    assert_eq!(w.op(&["config", "check"]).code, 3);
}

#[test]
fn config_check_warns_about_roles_that_run_unattended_on_another_host() {
    let w = World::builder("check-host").bare().build();
    std::fs::write(
        w.project.join("capstan.toml"),
        "schema_version = 1\n[hosts.claude]\nkind = \"claude\"\n[hosts.codex]\nkind = \"codex\"\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n[roles.developer]\nkind = \"Developer\"\nhost = \"codex\"\npermission_mode = \"acceptEdits\"\n",
    )
    .unwrap();
    let check = w.op(&["config", "check"]).ok();
    let resolved = check.json();
    assert_eq!(resolved["roles"].as_array().unwrap().len(), 2);
    assert!(
        check.stderr.contains("developer"),
        "a warning names the role: {}",
        check.text()
    );
}

#[test]
fn config_sync_writes_the_roles_once_and_a_second_run_writes_nothing() {
    let w = World::builder("sync").bare().build();
    w.op(&["init"]).ok();
    // The daemon creates the controller record and syncs the starter roles; the ledger is the CLI's again once it stopped.
    w.op(&["start"]).ok();
    w.op(&["stop"]).ok();
    let quiet = w.op(&["config", "sync"]).ok().json();
    assert_eq!(
        quiet["changed"], false,
        "the starter roles are already synced"
    );
    let mut config = std::fs::read_to_string(w.project.join("capstan.toml")).unwrap();
    config.push_str("\n[roles.extra]\nkind = \"Developer\"\nhost = \"claude\"\n");
    std::fs::write(w.project.join("capstan.toml"), config).unwrap();
    let first = w.op(&["config", "sync"]).ok().json();
    assert_eq!(first["changed"], true, "{first}");
    assert_eq!(
        first["inserted"].as_array().map(Vec::len),
        Some(1),
        "{first}"
    );
    let second = w.op(&["config", "sync"]).ok().json();
    assert_eq!(second["changed"], false, "{second}");
    assert_eq!(second["inserted"].as_array().map(Vec::len), Some(0));
    // With the daemon running the ledger belongs to it.
    w.op(&["start"]).ok();
    w.op(&["config", "sync"])
        .refused(4, "another cooperating controller owns this project");
    w.op(&["stop"]).ok();
}

#[test]
fn start_ping_and_stop_manage_one_daemon_per_project() {
    let w = World::builder("startstop").bare().build();
    w.op(&["init"]).ok();
    let started = w.op(&["start", "--json"]).ok();
    assert_eq!(started.json()["started"], true);
    let pid = pid_of(&started);
    let repeat = w.op(&["start", "--json"]).ok();
    assert_eq!(
        repeat.json()["started"],
        false,
        "a repeat finds the running daemon"
    );
    assert_eq!(pid_of(&repeat), pid);
    let ping = w.op(&["ping", "--json"]).ok();
    assert_eq!(pid_of(&ping), pid);
    assert!(w.socket().exists() && w.project.join(".capstan/state/daemon.pid").exists());
    assert_eq!(mode(&w.project.join(".capstan/state")), 0o700);
    // The daemon's files are private.
    assert_eq!(mode(&w.socket()), 0o600);
    let stopped = w.op(&["stop"]).ok();
    assert_eq!(stopped.stdout, "running: false\nresult: stopped\n");
    gone("the daemon to exit", pid);
    assert!(!w.socket().exists() && !w.project.join(".capstan/state/daemon.pid").exists());
    assert_eq!(
        w.op(&["stop"]).ok().stdout,
        "running: false\nresult: not_running\n"
    );
}

/// Fails about three runs in ten: both daemons lose the project lock at once (SQLite's `BEGIN EXCLUSIVE` with a zero busy
/// timeout in `ledger/src/lock.rs` returns BUSY to both when two processes arrive together), no daemon is left and both
/// clients end with exit 5. Node has the same protocol. Reproduction: `/tmp/capstan-suites-core-race.sh` of the report, or
/// run this test with `--ignored` several times. A bounded retry on BUSY in the lock would fix it; un-ignore this test as the
/// proof when it lands (plan-30/suites-core stop-and-ask: no change to any src/).
#[test]
#[ignore = "known defect: concurrent start, follow-up fix pending"]
fn two_starts_at_once_end_with_one_daemon_and_both_succeed() {
    let w = World::builder("racing").bare().build();
    w.op(&["init"]).ok();
    let (a, b) = std::thread::scope(|scope| {
        let a = scope.spawn(|| w.op(&["start", "--json"]));
        let b = scope.spawn(|| w.op(&["start", "--json"]));
        (a.join().unwrap(), b.join().unwrap())
    });
    let (a, b) = (a.ok(), b.ok());
    assert_eq!(pid_of(&a), pid_of(&b), "both found the same daemon");
    let started = [
        a.json()["started"].as_bool().unwrap(),
        b.json()["started"].as_bool().unwrap(),
    ];
    assert!(
        started.iter().any(|s| *s),
        "one of them started it: {started:?}"
    );
    w.op(&["stop"]).ok();
}

#[test]
fn sigterm_stops_the_daemon_cleanly_and_the_next_start_needs_no_recovery() {
    let w = World::builder("sigterm").bare().build();
    w.op(&["init"]).ok();
    let pid = pid_of(&w.op(&["start", "--json"]).ok());
    // SAFETY: the pid is the scratch daemon this test started.
    unsafe { libc_kill(pid as i32, 15) };
    gone("the daemon to exit on SIGTERM", pid);
    assert!(!w.socket().exists(), "no socket is left");
    assert!(
        !w.project.join(".capstan/state/daemon.pid").exists(),
        "no pid file is left"
    );
    let next = w.op(&["start", "--json"]).ok();
    assert_eq!(next.json()["started"], true);
    assert_ne!(pid_of(&next), pid);
    w.op(&["stop"]).ok();
}

#[test]
fn after_kill_9_status_still_reads_the_ledger_and_start_replaces_the_dead_daemons_files() {
    let w = World::builder("kill9").bare().build();
    w.op(&["init"]).ok();
    let pid = pid_of(&w.op(&["start", "--json"]).ok());
    let before = w.op(&["status", "--json"]).ok().json();
    // SAFETY: the pid is the scratch daemon the start command started.
    unsafe { libc_kill(pid as i32, 9) };
    gone("the daemon to die", pid);
    assert!(w.socket().exists(), "kill -9 leaves the socket behind");
    // The operator's status is a read of the ledger and works with the daemon down.
    let down = w.op(&["status", "--json"]).ok().json();
    assert_eq!(down["projectId"], before["projectId"]);
    assert_eq!(
        down["stateVersion"], before["stateVersion"],
        "nothing was lost"
    );
    // start finds nothing answering, replaces the stale socket and pid file, and a new daemon runs.
    let restarted = w.op(&["start", "--json"]).ok();
    assert_eq!(restarted.json()["started"], true);
    assert_ne!(pid_of(&restarted), pid);
    let after = w.op(&["status", "--json"]).ok().json();
    assert_eq!(after["projectId"], before["projectId"]);
    assert_eq!(
        after["stateVersion"], before["stateVersion"],
        "nothing was duplicated"
    );
    w.op(&["stop"]).ok();
}

#[test]
fn a_broken_capstan_toml_stops_the_daemon_at_start_with_the_loaders_message() {
    let w = World::builder("brokenstart").bare().build();
    w.op(&["init"]).ok();
    std::fs::write(
        w.project.join("capstan.toml"),
        "schema_version = 1\n[roles.pm]\nkind = \"Nonsense\"\nhost = \"claude\"\n",
    )
    .unwrap();
    let start = w.op(&["start"]);
    assert_ne!(start.code, 0, "{}", start.text());
    assert!(
        !start.stderr.is_empty(),
        "the failure is explained: {}",
        start.text()
    );
    assert!(!w.socket().exists(), "no daemon is left behind");
}

#[test]
fn a_restrictive_umask_does_not_stop_a_start_and_the_log_stays_private() {
    let w = World::builder("umask").bare().build();
    w.op(&["init"]).ok();
    let cstan = cstan_binary();
    let out = std::process::Command::new("/bin/sh")
        .args(["-c", "umask 0277; exec \"$0\" start --json"])
        .arg(&cstan)
        .current_dir(&w.project)
        .env_clear()
        .envs(w.environment())
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let log = w.project.join(".capstan/daemon.log");
    assert!(log.is_file(), "the start log exists");
    assert_eq!(mode(&log), 0o600);
    w.op(&["stop"]).ok();
}

#[test]
fn agent_mode_uses_the_token_and_the_socket_from_the_environment() {
    let w = World::standard("agentenv");
    // An agent's token and socket make it that agent.
    let who = w.as_agent("dev-1", &["inbox"]).ok();
    assert!(who.stdout.contains("no messages"), "{}", who.stdout);
    // Operator mode never uses a token: without the agent environment the same command is the operator's and needs an agent id.
    w.op(&["inbox"])
        .refused(3, "inbox needs exactly one agent id");
    // Half an agent environment is named, and a relative socket path too.
    let half = w.cstan_with(
        &w.project,
        None,
        &["inbox"],
        &[("CAPSTAN_TOKEN", w.token("dev-1"))],
        None,
    );
    assert_eq!(half.code, 3, "{}", half.text());
    assert!(
        half.stderr
            .contains("CAPSTAN_TOKEN and CAPSTAN_SOCKET must both be set"),
        "{}",
        half.stderr
    );
    let relative = w.cstan_with(
        &w.project,
        None,
        &["inbox"],
        &[
            ("CAPSTAN_TOKEN", w.token("dev-1")),
            ("CAPSTAN_SOCKET", "control.sock"),
        ],
        None,
    );
    assert_eq!(relative.code, 3, "{}", relative.text());
    assert!(
        relative.stderr.contains("absolute path"),
        "{}",
        relative.stderr
    );
    // A socket of another project is refused unless the operator says otherwise.
    let other = w.root.join("other");
    std::fs::create_dir_all(other.join(".capstan/state")).unwrap();
    let foreign = w.cstan_with(
        &other,
        None,
        &["inbox"],
        &[
            ("CAPSTAN_TOKEN", w.token("dev-1")),
            ("CAPSTAN_SOCKET", w.socket().to_str().unwrap()),
        ],
        None,
    );
    assert_eq!(foreign.code, 2, "{}", foreign.text());
    assert!(
        foreign
            .stderr
            .contains("is not the socket of the project you are in"),
        "{}",
        foreign.stderr
    );
    let allowed = w.cstan_with(
        &other,
        None,
        &["inbox"],
        &[
            ("CAPSTAN_TOKEN", w.token("dev-1")),
            ("CAPSTAN_SOCKET", w.socket().to_str().unwrap()),
            ("CAPSTAN_ALLOW_FOREIGN_SOCKET", "1"),
        ],
        None,
    );
    assert_eq!(allowed.code, 0, "{}", allowed.text());
    assert!(
        allowed
            .stderr
            .contains("continuing because CAPSTAN_ALLOW_FOREIGN_SOCKET=1"),
        "{}",
        allowed.stderr
    );
}

#[test]
fn a_bad_token_is_named_and_routed_arguments_keep_a_literal_json_after_the_separator() {
    let w = World::standard("routed");
    let bad = w.cstan_with(
        &w.project,
        None,
        &["inbox"],
        &[
            ("CAPSTAN_TOKEN", "bad token"),
            ("CAPSTAN_SOCKET", w.socket().to_str().unwrap()),
        ],
        None,
    );
    assert_eq!(bad.code, 3, "{}", bad.text());
    assert!(
        bad.stderr
            .contains("CAPSTAN_TOKEN must not contain whitespace"),
        "{}",
        bad.stderr
    );
    let sent = w.as_agent("pm-1", &["send", "dev-1", "--", "--json"]).ok();
    assert!(sent.stdout.contains("messageId"), "{}", sent.stdout);
    let inbox = w.as_agent("dev-1", &["inbox", "--json"]).ok().json();
    assert_eq!(
        inbox["messages"][0]["body"], "--json",
        "the text after the separator is the message"
    );
    // An empty argument is refused rather than sent.
    let empty = w.as_agent("pm-1", &["send", "dev-1", ""]);
    assert_ne!(empty.code, 0, "{}", empty.text());
}

#[test]
fn a_folder_that_is_not_a_repository_or_has_no_commit_is_named_with_the_exact_fix() {
    // No repository at all.
    let none = World::builder("nogit").no_git().bare().build();
    let init = none.op(&["init"]);
    let text = init.text();
    assert!(
        text.contains("git")
            && (text.contains("not a git repository") || text.contains("git init")),
        "{text}"
    );
    // A repository whose HEAD is unborn.
    let unborn = World::builder("unborn").no_git().bare().build();
    unborn.git(&["init", "--quiet", "--initial-branch=main"]);
    let out = unborn.op(&["init"]);
    let text = out.text();
    assert!(
        text.contains("no commit yet") || text.contains("unborn"),
        "{text}"
    );
    assert!(
        text.contains("git commit -m \"chore: initial commit\"")
            || text.contains("cstan init --git"),
        "{text}"
    );
}
