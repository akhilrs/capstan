//! What the parity sequences cannot show: the one-operation-at-a-time queue, the teardown the daemon asks for, the `cstan`
//! wrapper an agent finds first on its PATH, and the rule that a reused pane id is never closed.
mod common;

use capstan_herdr::api::{
    CaptureOutcome, HerdrState, PromptAnswer, RelayOutcome, RelayRefusal, StubAdapter,
};
use capstan_kernel::Core;
use capstan_launcher::api::{AnswerPromptRequest, LauncherService, SetupOutcome};
use capstan_launcher::kernel::{KernelGone, Ledger, LedgerPort};
use capstan_launcher::launcher::Launcher;
use capstan_launcher::shared::{FrontEndSite, LauncherOptions, TeardownRunner};
use common::ledger::KernelThread;
use common::{parity_dir, read_json, test_config};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};

const OWNER: &str = "owner-credential-0123456789-abcdefghijklmnopqrstuvwxyz";

fn project() -> Value {
    json!({
        "projectId": "proj1",
        "name": "Operations",
        "ownerCredential": OWNER,
        "initialInputs": [
            {"kind": "project_config", "content": {"name": "operations"}},
            {"kind": "task_brief", "content": {"objective": "prove it"}},
            {"kind": "acceptance_criteria", "content": {"criteria": ["it works"]}},
            {"kind": "policy", "content": {"review": "required"}},
            {"kind": "plan", "content": {"steps": ["one"]}},
        ],
    })
}

fn private(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

fn options(
    port: Arc<dyn LedgerPort>,
    root: &Path,
    spec: &Value,
    teardown: Option<TeardownRunner>,
) -> LauncherOptions {
    LauncherOptions {
        ledger: Ledger::new(port, OWNER),
        adapter: Arc::new(StubAdapter::new()),
        config: test_config(spec),
        project_root: root.to_string_lossy().into_owned(),
        cli_path: "/opt/capstan/cli.js".into(),
        socket_path: root.join("control.sock").to_string_lossy().into_owned(),
        credential: OWNER.into(),
        node_path: Some("/usr/bin/node".into()),
        base_environment: HashMap::from([("PATH".to_string(), "/usr/bin:/bin".to_string())]),
        git: None,
        now: None,
        log: None,
        sync_roles: None,
        run_setup: None,
        sleep: None,
        run_teardown: teardown,
        site: Some(FrontEndSite {
            env: HashMap::new(),
            exec_path: "/usr/bin/node".into(),
            sea: false,
            cstan: "/usr/bin/cstan".into(),
        }),
    }
}

/// Syncs the roles of `test_config` into the ledger, as the daemon does at start.
fn sync_roles(kernel: &KernelThread) {
    let desired: Vec<Value> = [("pm", "PM"), ("developer", "Developer")]
        .iter()
        .map(|(name, kind)| {
            json!({"name": name, "kind": kind, "host": "claude", "configHash": common::hash_of(name)})
        })
        .collect();
    kernel
        .call(move |core| {
            let context = common::ledger::context(core, OWNER)?;
            core.sync_role_definitions(&context, &Value::Array(desired))
        })
        .unwrap()
        .unwrap();
}

/// A ledger port that holds every job until it is opened, and then says the kernel is gone.
struct GatedPort {
    open: Mutex<bool>,
    changed: Condvar,
}

impl LedgerPort for GatedPort {
    fn run_job(&self, _job: Box<dyn FnOnce(&Core) + Send>) -> Result<(), KernelGone> {
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.changed.wait(open).unwrap();
        }
        Err(KernelGone)
    }
}

fn wait_for(what: &str, condition: impl Fn() -> bool) {
    for _ in 0..500 {
        if condition() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn one_operation_runs_one_waits_and_the_next_is_refused_as_busy() {
    let root = tempfile::tempdir().unwrap();
    let port = Arc::new(GatedPort {
        open: Mutex::new(false),
        changed: Condvar::new(),
    });
    let launcher = Arc::new(Launcher::new(options(
        port.clone(),
        root.path(),
        &json!({}),
        None,
    )));
    let running = {
        let launcher = Arc::clone(&launcher);
        std::thread::spawn(move || launcher.launch_pm())
    };
    wait_for("the first operation", || {
        launcher.in_flight_operations() == 1
    });
    let waiting = {
        let launcher = Arc::clone(&launcher);
        std::thread::spawn(move || launcher.restart_pm())
    };
    wait_for("the second operation", || {
        launcher.in_flight_operations() == 2
    });
    let refused = launcher.release("developer-1").unwrap_err();
    assert_eq!(refused.code, "busy");
    assert_eq!(
        refused.message,
        "another launcher operation is already waiting"
    );
    assert_eq!(launcher.in_flight_operations(), 2);
    *port.open.lock().unwrap() = true;
    port.changed.notify_all();
    // The kernel is gone: both operations end with the controller's own message, and the queue is empty again.
    let first = running.join().unwrap().unwrap_err();
    let second = waiting.join().unwrap().unwrap_err();
    assert_eq!(first.message, "controller is closed");
    assert_eq!(second.message, "controller is closed");
    assert_eq!(launcher.in_flight_operations(), 0);
}

#[test]
fn teardown_runs_the_configured_command_in_the_project_root() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join(".capstan").join("state");
    private(&root.path().join(".capstan"));
    private(&state);
    let kernel = Arc::new(KernelThread::spawn(&state, &project(), "operations").unwrap());
    let queue = Arc::new(Mutex::new(VecDeque::from([
        SetupOutcome::Failed {
            exit_code: Some(3),
            output: "boom".into(),
        },
        SetupOutcome::Timeout,
        SetupOutcome::Ok,
    ])));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let runner: TeardownRunner = {
        let queue = Arc::clone(&queue);
        let calls = Arc::clone(&calls);
        Arc::new(move |command, cwd, timeout_ms, environment| {
            calls.lock().unwrap().push((
                command.to_string(),
                cwd.to_string(),
                timeout_ms,
                environment.get("CAPSTAN_WORKTREE_PATH").cloned(),
                environment.get("CAPSTAN_AGENT_ID").cloned(),
                environment.contains_key("CAPSTAN_TOKEN"),
            ));
            queue.lock().unwrap().pop_front().unwrap()
        })
    };
    let spec = json!({"worktree": {"teardown": "./cleanup.sh", "teardownTimeoutSeconds": 5}});
    let launcher = Launcher::new(options(
        kernel.clone(),
        root.path(),
        &spec,
        Some(runner.clone()),
    ));
    assert_eq!(
        launcher.teardown("developer-1", "/tmp/wt").unwrap(),
        SetupOutcome::Failed {
            exit_code: Some(3),
            output: "boom".into()
        }
    );
    assert_eq!(
        launcher.teardown("developer-1", "/tmp/wt").unwrap(),
        SetupOutcome::Timeout
    );
    assert_eq!(
        launcher.teardown("developer-2", "/tmp/other").unwrap(),
        SetupOutcome::Ok
    );
    let root_text = root.path().to_string_lossy().into_owned();
    assert_eq!(
        *calls.lock().unwrap(),
        vec![
            (
                "./cleanup.sh".to_string(),
                root_text.clone(),
                5_000,
                Some("/tmp/wt".to_string()),
                Some("developer-1".to_string()),
                false
            ),
            (
                "./cleanup.sh".to_string(),
                root_text.clone(),
                5_000,
                Some("/tmp/wt".to_string()),
                Some("developer-1".to_string()),
                false
            ),
            (
                "./cleanup.sh".to_string(),
                root_text,
                5_000,
                Some("/tmp/other".to_string()),
                Some("developer-2".to_string()),
                false
            ),
        ]
    );
    // Nothing configured: nothing runs.
    let unconfigured = Launcher::new(options(kernel, root.path(), &json!({}), Some(runner)));
    assert_eq!(
        unconfigured.teardown("developer-1", "/tmp/wt").unwrap(),
        SetupOutcome::Ok
    );
    assert_eq!(calls.lock().unwrap().len(), 3);
}

#[test]
fn the_cstan_wrapper_is_private_and_runs_the_daemons_cstan() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join(".capstan").join("state");
    private(&root.path().join(".capstan"));
    private(&state);
    let kernel = Arc::new(KernelThread::spawn(&state, &project(), "wrapper").unwrap());
    let launcher = Launcher::new(options(kernel, root.path(), &json!({}), None));
    let environment = launcher
        .kernel()
        .environment(Some("token-1"), None)
        .unwrap();
    let bin = root.path().join(".capstan").join("bin");
    assert_eq!(
        environment["PATH"],
        format!("{}:/usr/bin:/bin", bin.display())
    );
    assert_eq!(environment["CAPSTAN_TOKEN"], "token-1");
    let wrapper = bin.join("cstan");
    assert_eq!(
        std::fs::read_to_string(&wrapper).unwrap(),
        "#!/bin/sh\nexec '/usr/bin/cstan' \"$@\"\n"
    );
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&wrapper), 0o700);
    assert_eq!(mode(&bin), 0o700);
    // Only the wrapper is left in the directory: the temporary file was renamed over it.
    assert_eq!(std::fs::read_dir(&bin).unwrap().count(), 1);
    // A project path with a colon cannot be put on PATH.
    let colon = tempfile::Builder::new().prefix("a:b").tempdir().unwrap();
    let odd_state = colon.path().join(".capstan").join("state");
    private(&colon.path().join(".capstan"));
    private(&odd_state);
    let odd = Launcher::new(options(
        Arc::new(KernelThread::spawn(&odd_state, &project(), "colon").unwrap()),
        colon.path(),
        &json!({}),
        None,
    ));
    let refused = odd.kernel().environment(Some("t"), None).unwrap_err();
    assert_eq!(
        refused.message(),
        "a project path with a colon cannot be put on PATH, so cstan would not be found"
    );
}

/// The pane-reuse incident (migration 0037, commit 0b0007a): Herdr hands a closed pane's short id to another agent's pane,
/// and closing the id then kills a live agent. The sequences below put another terminal behind the id the launcher has
/// recorded for the agent; whatever Node did, the launcher must not close it. (The parity test shows the Rust launcher
/// does what the fixtures say; this shows the fixtures say not to close.)
#[test]
fn a_reused_pane_is_never_closed() {
    let guarded = [
        ("teardown", "teardown-of-a-reused-pane-id"),
        (
            "teardown",
            "teardown-of-a-pane-whose-tokens-name-another-agent",
        ),
        ("teardown", "teardown-of-a-pane-of-another-project"),
        ("teardown", "teardown-after-a-pane-move"),
        ("adopt", "adopt-a-lost-pane-whose-id-was-reused"),
        ("pm", "restart-pm-old-pane-reused"),
    ];
    for (group, name) in guarded {
        let file = read_json(&parity_dir().join(format!("{group}.json")));
        let sequence = file["sequences"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == name)
            .unwrap_or_else(|| panic!("{group}/{name} is missing"));
        let steps = sequence["steps"].as_array().unwrap();
        let released = steps
            .iter()
            .filter(|s| s["op"] == "release" || s["op"] == "adopt_all" || s["op"] == "restart_pm")
            .count();
        assert!(released > 0, "{name} never reaches the cleanup it guards");
        for step in steps {
            let closed: Vec<_> = step["calls"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|c| c["m"] == "closePane")
                .collect();
            assert!(
                closed.is_empty(),
                "{name}: {} closed a pane: {closed:?}",
                step["op"]
            );
            let logged = step["events"].as_array().unwrap();
            if step["op"] == "release" || step["op"] == "adopt_all" || step["op"] == "restart_pm" {
                assert!(
                    logged.iter().any(|e| e["event"] == "pane_not_owned"),
                    "{name}: {} left the pane without logging pane_not_owned",
                    step["op"]
                );
            }
        }
    }
}

#[test]
fn a_prompt_is_captured_and_answered_in_the_agents_own_pane() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join(".capstan").join("state");
    private(&root.path().join(".capstan"));
    private(&state);
    let kernel = Arc::new(KernelThread::spawn(&state, &project(), "prompts").unwrap());
    sync_roles(&kernel);
    let adapter = Arc::new(StubAdapter::new());
    let mut launcher_options = options(kernel, root.path(), &json!({"promptRelay": true}), None);
    launcher_options.adapter = adapter.clone();
    let launcher = Launcher::new(launcher_options);
    assert_eq!(
        launcher.launch_pm().unwrap().state.unwrap().as_str(),
        "started"
    );

    // Nothing is blocked: the adapter refuses and nothing is typed.
    assert_eq!(
        launcher.capture_prompt("pm-1").unwrap(),
        CaptureOutcome::Refused(RelayRefusal::NotBlocked)
    );
    let typed = Arc::new(Mutex::new(0));
    let answer = PromptAnswer::Option { number: 1 };
    let mut count = {
        let typed = Arc::clone(&typed);
        move || {
            *typed.lock().unwrap() += 1;
            Ok(())
        }
    };
    adapter.set_relay(RelayOutcome::Typed {
        keys: vec!["enter".into()],
        input_readable: None,
    });
    let outcome = launcher
        .answer_prompt(
            "pm-1",
            AnswerPromptRequest {
                prompt_sha: &"a".repeat(64),
                answer: &answer,
                before_type: &mut count,
            },
        )
        .unwrap();
    assert!(matches!(outcome, RelayOutcome::Typed { .. }));
    assert_eq!(*typed.lock().unwrap(), 1);
    // An agent that is not active has no pane to read or type into.
    let refused = launcher.capture_prompt("developer-1").unwrap_err();
    assert_eq!(refused.code, "agent_not_active");

    // An interrupt sends one Esc, and only to an agent Herdr shows working.
    assert!(!launcher.interrupt("pm-1").unwrap());
    adapter.set_observation(HerdrState::Working);
    assert!(launcher.interrupt("pm-1").unwrap());
}

#[test]
fn an_interrupt_needs_the_prompt_relay() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join(".capstan").join("state");
    private(&root.path().join(".capstan"));
    private(&state);
    let kernel = Arc::new(KernelThread::spawn(&state, &project(), "interrupt").unwrap());
    sync_roles(&kernel);
    let launcher = Launcher::new(options(kernel, root.path(), &json!({}), None));
    launcher.launch_pm().unwrap();
    let refused = launcher.interrupt("pm-1").unwrap_err();
    assert_eq!(refused.code, "not_configured");
    assert_eq!(
        refused.message,
        "--interrupt needs [prompt_relay] enabled = true in capstan.toml"
    );
}
