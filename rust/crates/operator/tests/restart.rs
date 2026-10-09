//! The restart at service level: the coordinator writes the plan file with the exact daemon argv and refuses with Node's
//! texts; the helper, run on a scratch project with a fake daemon command, stops the old controller, backs the ledger up,
//! starts the new build, rolls back when it does not answer, and writes result.json like Node. (The end-to-end restart
//! over the socket is in d2b-shadow.)

use capstan_operator::api::RestartCoordinator;
use capstan_operator::restart::{
    recover_restart_results, restart_directory, restart_helper_alive, skip_restarts_with_live_plan,
    KnownGoodBuild, ProcessRestartCoordinator, RestartCoordinatorOptions, RestartRecoveryDeps,
};
use capstan_operator::restart_helper::{run_helper, PartialTiming, RestartPlan, KNOWN_GOOD_BINARY};
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CREDENTIAL: &str = "restart-test-credential-0123456789abcdef";

struct Project {
    dir: tempfile::TempDir,
    root: PathBuf,
    state: PathBuf,
}

impl Project {
    fn new() -> Project {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let state = root.join(".capstan").join("state");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(
            root.join(".capstan").join("operator.key"),
            format!("{CREDENTIAL}\n"),
        )
        .unwrap();
        Project { dir, root, state }
    }

    fn key(&self) -> PathBuf {
        self.root.join(".capstan").join("operator.key")
    }

    fn script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.root.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }
}

fn dead_pid() -> i64 {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    i64::from(pid)
}

// ------------------------------------------------------------------------------------------------ the coordinator

struct Coordinator {
    coordinator: ProcessRestartCoordinator,
    stops: Arc<AtomicUsize>,
    notices: Arc<Mutex<Vec<String>>>,
    busy: Arc<Mutex<Vec<String>>>,
}

fn coordinator(project: &Project, binary: &Path, argv: Vec<String>) -> Coordinator {
    let stops = Arc::new(AtomicUsize::new(0));
    let notices = Arc::new(Mutex::new(Vec::new()));
    let busy: Arc<Mutex<Vec<String>>> = Arc::default();
    let (s, n, b) = (stops.clone(), notices.clone(), busy.clone());
    let coordinator = ProcessRestartCoordinator::new(RestartCoordinatorOptions {
        state_dir: project.state.clone(),
        project_root: project.root.clone(),
        binary_path: binary.to_path_buf(),
        node: binary.to_string_lossy().into_owned(),
        argv,
        socket_path: project.state.join("control.sock"),
        pid_path: project.state.join("daemon.pid"),
        log_path: project.state.join("daemon.log"),
        credential_file: project.key(),
        health_timeout_seconds: 7,
        idle_wait_seconds: 0,
        busy: Box::new(move || b.lock().unwrap().clone()),
        notify_pm: Box::new(move |body, _| n.lock().unwrap().push(body.to_string())),
        request_stop: Box::new(move || {
            s.fetch_add(1, Ordering::SeqCst);
        }),
        pid: Some(4242),
        timing: Some(PartialTiming {
            poll_ms: Some(20),
            ..PartialTiming::default()
        }),
        idle_poll_ms: None,
        now_ms: None,
        sleep: None,
        log: None,
    });
    Coordinator {
        coordinator,
        stops,
        notices,
        busy,
    }
}

fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn the_coordinator_writes_the_plan_with_the_exact_daemon_argv_and_starts_the_known_good_helper() {
    let project = Project::new();
    let binary = project.script("cstan-daemon", "exit 0");
    let argv = vec!["--serve".to_string(), "x y".to_string()];
    let c = coordinator(&project, &binary, argv.clone());
    let proposal = json!({"proposalId": "p-1", "forceRestart": false});

    // No known-good build yet: refused with Node's text, the PM told, nothing written.
    let refused = c.coordinator.run(&proposal).unwrap_err();
    assert_eq!(refused.code, "no_known_good");
    assert!(refused
        .message
        .starts_with("there is no known-good build to roll back to yet."));
    assert!(c.notices.lock().unwrap()[0].starts_with("Operator restart refused (no_known_good): "));
    assert!(!restart_directory(&project.state, "p-1").exists());
    assert_eq!(c.stops.load(Ordering::SeqCst), 0);

    // The known-good helper is a stand-in that records how it was started.
    let helper_args = project.root.join("helper-args.txt");
    let good = project.script(
        "known-good-source",
        &format!("echo \"$@\" > {}", helper_args.display()),
    );
    KnownGoodBuild::new(&project.state)
        .snapshot(&good, 7)
        .unwrap();
    assert_eq!(c.coordinator.preflight().unwrap().warning, None);

    // Busy and not forced: refused with Node's text; forced: goes ahead.
    *c.busy.lock().unwrap() = vec!["1 review(s) in state started".into()];
    let busy = c.coordinator.run(&proposal).unwrap_err();
    assert_eq!(busy.code, "busy");
    assert_eq!(
        busy.message,
        "the controller is not idle after 0 seconds: 1 review(s) in state started. Nothing was restarted. Propose the restart again with --force to restart anyway."
    );
    assert!(!restart_directory(&project.state, "p-1").exists());
    c.coordinator
        .run(&json!({"proposalId": "p-1", "forceRestart": true}))
        .unwrap();
    assert_eq!(c.stops.load(Ordering::SeqCst), 1);

    let directory = restart_directory(&project.state, "p-1");
    let plan: RestartPlan =
        serde_json::from_str(&std::fs::read_to_string(directory.join("plan.json")).unwrap())
            .unwrap();
    assert_eq!(plan.id, "p-1");
    assert_eq!(plan.node, binary.to_string_lossy());
    assert_eq!(plan.argv, argv);
    assert_eq!(plan.cwd, project.root.to_string_lossy());
    assert_eq!(plan.pid, 4242);
    assert_eq!(plan.binary, Some(true));
    assert_eq!(plan.dist_path, binary.to_string_lossy());
    assert_eq!(
        plan.ledger_path,
        project.state.join("controller.sqlite").to_string_lossy()
    );
    assert_eq!(plan.health_timeout_seconds, 7);
    assert_eq!(
        plan.known_good_path,
        project.state.join("known-good").to_string_lossy()
    );
    assert!(plan.deps_changed.is_empty());
    let mode = std::fs::metadata(directory.join("plan.json"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);

    // The helper is the saved binary run as `__restart-helper <plan>`.
    wait_for("the helper stand-in", || helper_args.exists());
    let args = std::fs::read_to_string(&helper_args).unwrap();
    assert_eq!(
        args.trim(),
        format!("__restart-helper {}", directory.join("plan.json").display())
    );
    assert!(directory.join("helper.pid").exists());
    assert!(project
        .state
        .join("known-good")
        .join(KNOWN_GOOD_BINARY)
        .exists());
    let _ = restart_helper_alive(&project.state, "p-1");
    assert!(!skip_restarts_with_live_plan(&project.state)(
        "no-such-proposal"
    ));
}

#[test]
fn a_missing_operator_key_refuses_before_anything_is_written() {
    let project = Project::new();
    let binary = project.script("cstan-daemon", "exit 0");
    let c = coordinator(&project, &binary, vec![]);
    KnownGoodBuild::new(&project.state)
        .snapshot(&binary, 1)
        .unwrap();
    std::fs::remove_file(project.key()).unwrap();
    let refused = c
        .coordinator
        .run(&json!({"proposalId": "p-2", "forceRestart": true}))
        .unwrap_err();
    assert_eq!(refused.code, "no_credential");
    assert!(refused.message.ends_with("Nothing was restarted."));
    assert!(!restart_directory(&project.state, "p-2").exists());
}

#[test]
fn a_finished_restart_is_ingested_once() {
    let project = Project::new();
    let directory = restart_directory(&project.state, "p-3");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("result.json"),
        "{\"outcome\":\"rolled_back\",\"reason\":\"no ping\"}\n",
    )
    .unwrap();
    let finished: Arc<Mutex<Vec<(String, String, i64)>>> = Arc::default();
    let notices: Arc<Mutex<Vec<(String, bool)>>> = Arc::default();
    let (f, n) = (finished.clone(), notices.clone());
    let deps = RestartRecoveryDeps {
        state_dir: project.state.clone(),
        running_restarts: Box::new(|| {
            vec![
                json!({"kind": "restart", "proposalId": "p-3", "run": {"startedAt": "2026-01-01T00:00:00.000Z"}}),
                json!({"kind": "restart", "proposalId": "p-4"}),
                json!({"kind": "command", "proposalId": "p-5"}),
            ]
        }),
        finish_run: Box::new(move |id, report, ms| {
            f.lock()
                .unwrap()
                .push((id.into(), report.status.as_str().into(), ms));
            Ok(())
        }),
        notify_pm: Box::new(move |body, action| n.lock().unwrap().push((body.into(), action))),
        now_ms: Some(Box::new(|| 1_767_225_600_000 + 5_000)),
        log: None,
    };
    assert_eq!(recover_restart_results(&deps), vec!["p-3"]);
    assert_eq!(
        finished.lock().unwrap().as_slice(),
        [("p-3".into(), "failed".into(), 5_000)]
    );
    let notices = notices.lock().unwrap();
    assert!(notices[0].0.starts_with("Operator restart p-3 ROLLED BACK"));
    assert!(notices[0].1);
    assert!(directory.join("result.ingested.json").exists());
    assert!(!directory.join("result.json").exists());
    assert!(recover_restart_results(&deps).is_empty());
}

// ------------------------------------------------------------------------------------------------ the helper

/// A stand-in for the daemon's control socket: once the fake build has written its pid, answers ping with it.
struct FakeSocket {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FakeSocket {
    fn serve(socket: PathBuf, marker: PathBuf) -> FakeSocket {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::spawn(move || {
            let pid = loop {
                if flag.load(Ordering::SeqCst) {
                    return;
                }
                if let Ok(text) = std::fs::read_to_string(&marker) {
                    if let Ok(pid) = text.trim().parse::<i64>() {
                        break pid;
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            let _ = std::fs::remove_file(&socket);
            let listener = UnixListener::bind(&socket).unwrap();
            listener.set_nonblocking(true).unwrap();
            while !flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        use std::io::{BufRead, BufReader, Write};
                        stream.set_nonblocking(false).unwrap();
                        let mut line = String::new();
                        BufReader::new(stream.try_clone().unwrap())
                            .read_line(&mut line)
                            .unwrap();
                        let frame: Value = serde_json::from_str(&line).unwrap();
                        assert_eq!(frame["command"], "ping");
                        let answer = if frame["credential"] == CREDENTIAL {
                            json!({"ok": true, "result": {"pid": pid}})
                        } else {
                            json!({"ok": false})
                        };
                        let _ = writeln!(stream, "{answer}");
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(10)),
                }
            }
        });
        FakeSocket {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for FakeSocket {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Kills the fake daemon a test started (its process group leader's pid is in the marker).
fn kill_marker(marker: &Path) {
    if let Ok(text) = std::fs::read_to_string(marker) {
        if let Ok(pid) = text.trim().parse::<i32>() {
            // SAFETY: the pid is the fake daemon this test started in its own process group.
            unsafe { libc::kill(-pid, libc::SIGKILL) };
        }
    }
}

struct HelperRun {
    project: Project,
    plan_path: PathBuf,
    restart_dir: PathBuf,
    marker: PathBuf,
    dist: PathBuf,
    ledger: PathBuf,
}

/// Lays out a restart: `dist_body` is the script now at the daemon path, `known_good_body` the saved one.
fn helper_run(dist_body: &str, known_good_body: &str, known_good_max: i64) -> HelperRun {
    let project = Project::new();
    let marker = project.root.join("started.pid");
    let good = format!("echo $$ > {}\nexec sleep 300", marker.display());
    let fill = |body: &str| body.replace("GOOD", &good);
    let dist = project.script("cstan-daemon", &fill(dist_body));
    let saved = project.script("saved", &fill(known_good_body));
    KnownGoodBuild::new(&project.state)
        .snapshot(&saved, known_good_max)
        .unwrap();
    let ledger = project.state.join("controller.sqlite");
    drop(capstan_ledger::open_database(&ledger, &capstan_ledger::OpenOptions::default()).unwrap());
    let restart_dir = restart_directory(&project.state, "p-9");
    std::fs::create_dir_all(&restart_dir).unwrap();
    let plan = RestartPlan {
        id: "p-9".into(),
        node: dist.to_string_lossy().into_owned(),
        argv: vec![],
        cwd: project.root.to_string_lossy().into_owned(),
        state_dir: project.state.to_string_lossy().into_owned(),
        socket_path: project
            .state
            .join("control.sock")
            .to_string_lossy()
            .into_owned(),
        pid_path: project
            .state
            .join("daemon.pid")
            .to_string_lossy()
            .into_owned(),
        pid: dead_pid(),
        dist_path: dist.to_string_lossy().into_owned(),
        known_good_path: project
            .state
            .join("known-good")
            .to_string_lossy()
            .into_owned(),
        binary: Some(true),
        ledger_path: ledger.to_string_lossy().into_owned(),
        log_path: project
            .state
            .join("daemon.log")
            .to_string_lossy()
            .into_owned(),
        health_timeout_seconds: 5,
        credential_file: project.key().to_string_lossy().into_owned(),
        deps_changed: vec![],
        timing: Some(PartialTiming {
            handoff_wait_ms: Some(2_000),
            term_wait_ms: Some(500),
            kill_wait_ms: Some(500),
            poll_ms: Some(20),
        }),
    };
    let plan_path = restart_dir.join("plan.json");
    std::fs::write(&plan_path, serde_json::to_string_pretty(&plan).unwrap()).unwrap();
    HelperRun {
        project,
        plan_path,
        restart_dir,
        marker,
        dist,
        ledger,
    }
}

fn result_of(run: &HelperRun) -> Value {
    serde_json::from_str(&std::fs::read_to_string(run.restart_dir.join("result.json")).unwrap())
        .unwrap()
}

#[test]
fn the_helper_backs_the_ledger_up_starts_the_new_build_and_reports_ok() {
    let run = helper_run("GOOD", "exit 9", 1);
    let socket = FakeSocket::serve(run.project.state.join("control.sock"), run.marker.clone());
    let original = std::fs::read(&run.ledger).unwrap();
    let code = run_helper(&run.plan_path, &|_| {});
    drop(socket);
    kill_marker(&run.marker);
    assert_eq!(code, 0);
    let result = result_of(&run);
    assert_eq!(result["outcome"], "ok");
    let pid: i64 = std::fs::read_to_string(&run.marker)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(result["pid"], pid);
    assert_eq!(
        std::fs::read(run.restart_dir.join("ledger.bak").join("controller.sqlite")).unwrap(),
        original
    );
    assert!(!run
        .project
        .dir
        .path()
        .join("cstan-daemon.failed-p-9")
        .exists());
    assert!(std::fs::read_to_string(run.project.state.join("daemon.log")).is_ok());
}

#[test]
fn a_new_build_that_fails_to_start_is_rolled_back_to_the_known_good_binary() {
    let run = helper_run(
        "echo boom-from-the-new-build\nexit 3",
        "GOOD",
        capstan_ledger::max_embedded_migration(),
    );
    let socket = FakeSocket::serve(run.project.state.join("control.sock"), run.marker.clone());
    let code = run_helper(&run.plan_path, &|_| {});
    drop(socket);
    kill_marker(&run.marker);
    assert_eq!(code, 1);
    let result = result_of(&run);
    assert_eq!(result["outcome"], "rolled_back");
    assert_eq!(
        result["reason"],
        "the controller exited right after it started (exit code 3)"
    );
    assert_eq!(result["ledgerRestored"], false);
    assert!(result["failedLogTail"]
        .as_str()
        .unwrap()
        .contains("boom-from-the-new-build"));
    assert!(result["pid"].as_i64().unwrap() > 1);
    // The failed binary is kept beside the restored one, which is the saved copy.
    let failed = PathBuf::from(format!("{}.failed-p-9", run.dist.display()));
    assert!(std::fs::read_to_string(failed)
        .unwrap()
        .contains("boom-from-the-new-build"));
    assert!(std::fs::read_to_string(&run.dist)
        .unwrap()
        .contains("exec sleep 300"));
    assert_eq!(
        std::fs::metadata(&run.dist).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

#[test]
fn a_ledger_migrated_past_the_known_good_build_is_restored_before_the_rollback() {
    // The saved build knows an older schema than the ledger on disk, and the failed build leaves a stray -wal behind.
    let max = capstan_ledger::max_embedded_migration();
    let run = helper_run("exit 3", "GOOD", max - 1);
    // Point the failing build at the real ledger path now that it is known.
    std::fs::write(
        &run.dist,
        format!(
            "#!/bin/sh\nprintf junk > {}-wal\nexit 3\n",
            run.ledger.display()
        ),
    )
    .unwrap();
    let socket = FakeSocket::serve(run.project.state.join("control.sock"), run.marker.clone());
    let code = run_helper(&run.plan_path, &|_| {});
    drop(socket);
    kill_marker(&run.marker);
    assert_eq!(code, 1);
    let result = result_of(&run);
    assert_eq!(result["outcome"], "rolled_back");
    assert_eq!(result["ledgerRestored"], true);
    assert!(!PathBuf::from(format!("{}-wal", run.ledger.display())).exists());
}

#[test]
fn when_neither_build_starts_the_helper_reports_down_with_manual_recovery() {
    let run = helper_run("exit 3", "exit 4", 1);
    let code = run_helper(&run.plan_path, &|_| {});
    assert_eq!(code, 2);
    let result = result_of(&run);
    assert_eq!(result["outcome"], "down");
    let reason = result["reason"].as_str().unwrap();
    assert!(reason.starts_with("the new build failed (the controller exited right after it started (exit code 3)) and the known-good build did not start either (the controller exited right after it started (exit code 4))."), "{reason}");
    assert!(result["manualRecovery"]
        .as_str()
        .unwrap()
        .contains("3. Start the controller with: cstan start"));
}

#[test]
fn an_unreadable_plan_is_a_down_result_not_a_hang() {
    let project = Project::new();
    let plan = project.root.join("plan.json");
    std::fs::write(&plan, "not json").unwrap();
    assert_eq!(run_helper(&plan, &|_| {}), 2);
    let result: Value =
        serde_json::from_str(&std::fs::read_to_string(project.root.join("result.json")).unwrap())
            .unwrap();
    assert_eq!(result["outcome"], "down");
    assert!(result["reason"]
        .as_str()
        .unwrap()
        .starts_with("the restart helper could not read its plan or the operator key ("));
}
