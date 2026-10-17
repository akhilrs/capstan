//! The controller answers a worker's permission prompt while an approval window is open, and the PM's `prompt answer
//! --session auto=<v>` opens that window. The launcher is a fake that plays a pane: it shows a prompt until something
//! answers it, refuses to type when the screen is not the one it was asked about, and logs every answer it typed. The
//! clock is a cell the test moves.

mod common;

use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::handlers::handle_frame;
use capstan_daemon::handlers::shared::AbortSignal;
use capstan_daemon::handlers::MAX_RESPONSE_BYTES;
use capstan_daemon::loops::auto_answer::{AutoAnswer, AutoAnswerOptions};
use capstan_daemon::loops::driver::{Driver, DriverLog, DriverOptions};
use capstan_daemon::loops::notifier::Notifier;
use capstan_daemon::relay_answer::{attempts_of, Attempts, ATTEMPT_STALE_MS};
use capstan_herdr::api::{
    CaptureOutcome, CapturedPrompt, DeferralReason, HookError, InputBlocker, PaneEntry, PanePhase,
    PaneRole, PromptAnswer, RelayOption, RelayOutcome, RelayRefusal, SendOutcome, StubAdapter,
    INPUT_UNREADABLE_DETAIL,
};
use capstan_herdr::prompt_relay::{prompt_hash, HashInput};
use capstan_kernel::env::{Env, SystemEnv};
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::types::InitialProject;
use capstan_kernel::Core;
use capstan_launcher::api::{
    AnswerPromptRequest, BranchRenameResult, LaunchResult, LauncherError, LauncherResult,
    LauncherService, LauncherStatus, ObserveResult, ReleaseResult, ReplaceResult, SetupOutcome,
    SpawnOptions, SpawnResult, StubLauncher,
};
use common::private_tempdir;
use common::project::{write_project, Identity};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

const BASE_CONFIG: &str = r#"schema_version = 1

[hosts.claude]
kind = "claude"

[operator]
enabled = true
__OPERATOR__

[prompt_relay]
enabled = __RELAY__

[roles.pm]
kind = "PM"
host = "claude"

[roles.developer]
kind = "Developer"
host = "claude"

[roles.operator]
kind = "Developer"
host = "claude"
allow = ["Bash(cstan op *)"]
deny = ["Write", "Edit", "NotebookEdit", "Agent", "Task", "Read", "Glob", "Grep"]
"#;

/// The system's randomness with a clock the test sets.
struct ClockEnv(Arc<AtomicI64>);

impl Env for ClockEnv {
    fn now(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
    fn uuid(&self) -> String {
        SystemEnv.uuid()
    }
    fn random_bytes(&self, n: usize) -> Vec<u8> {
        SystemEnv.random_bytes(n)
    }
}

// ------------------------------------------------------------------------------------------------ the fake pane

/// A launcher whose workers sit at prompts the test puts on their screens.
struct Panes {
    stub: StubLauncher,
    screens: Mutex<HashMap<String, CaptureOutcome>>,
    /// Captures that answer before the screen does (a live pane that differs from the one captured).
    scripted: Mutex<VecDeque<CaptureOutcome>>,
    /// The screen changes after the capture and just before the send.
    swap_before_send: Mutex<Option<CaptureOutcome>>,
    refuse_next: Mutex<Option<RelayRefusal>>,
    typed: Mutex<Vec<(String, PromptAnswer)>>,
    answer_calls: Mutex<usize>,
    /// Run once while the hook has moved the row to typing.
    during_answer: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Panes {
    fn new() -> Arc<Panes> {
        Arc::new(Panes {
            stub: StubLauncher::new(),
            screens: Mutex::new(HashMap::new()),
            scripted: Mutex::new(VecDeque::new()),
            swap_before_send: Mutex::new(None),
            refuse_next: Mutex::new(None),
            typed: Mutex::new(Vec::new()),
            answer_calls: Mutex::new(0),
            during_answer: Mutex::new(None),
        })
    }

    fn show(&self, agent: &str, prompt: CapturedPrompt) {
        lock(&self.screens).insert(agent.into(), CaptureOutcome::Captured(prompt));
    }

    fn typed(&self) -> Vec<(String, PromptAnswer)> {
        lock(&self.typed).clone()
    }

    fn answer_calls(&self) -> usize {
        *lock(&self.answer_calls)
    }
}

impl LauncherService for Panes {
    fn launch_pm(&self) -> LauncherResult<LaunchResult> {
        self.stub.launch_pm()
    }
    fn restart_pm(&self) -> LauncherResult<LaunchResult> {
        self.stub.restart_pm()
    }
    fn spawn(&self, role_name: &str, options: &SpawnOptions) -> LauncherResult<SpawnResult> {
        self.stub.spawn(role_name, options)
    }
    fn rename_branch_for_task(
        &self,
        agent_id: &str,
        task: &str,
    ) -> LauncherResult<BranchRenameResult> {
        self.stub.rename_branch_for_task(agent_id, task)
    }
    fn release(&self, agent_id: &str) -> LauncherResult<ReleaseResult> {
        self.stub.release(agent_id)
    }
    fn replace(&self, agent_id: &str) -> LauncherResult<ReplaceResult> {
        self.stub.replace(agent_id)
    }
    fn observe(&self, agent_id: &str, lines: usize) -> LauncherResult<ObserveResult> {
        self.stub.observe(agent_id, lines)
    }
    fn capture_prompt(&self, agent_id: &str) -> LauncherResult<CaptureOutcome> {
        if let Some(scripted) = lock(&self.scripted).pop_front() {
            return Ok(scripted);
        }
        Ok(lock(&self.screens)
            .get(agent_id)
            .cloned()
            .unwrap_or(CaptureOutcome::Refused(RelayRefusal::NotBlocked)))
    }
    fn answer_prompt(
        &self,
        agent_id: &str,
        request: AnswerPromptRequest<'_>,
    ) -> LauncherResult<RelayOutcome> {
        *lock(&self.answer_calls) += 1;
        if let Some(swapped) = lock(&self.swap_before_send).take() {
            lock(&self.screens).insert(agent_id.into(), swapped);
        }
        let screen = lock(&self.screens).get(agent_id).cloned();
        match screen {
            Some(CaptureOutcome::Captured(prompt)) if prompt.prompt_sha == request.prompt_sha => {}
            Some(CaptureOutcome::Captured(_)) => {
                return Ok(RelayOutcome::Refused {
                    reason: RelayRefusal::PromptChanged,
                    keys: vec![],
                })
            }
            _ => {
                return Ok(RelayOutcome::Refused {
                    reason: RelayRefusal::NotBlocked,
                    keys: vec![],
                })
            }
        }
        if let Some(reason) = lock(&self.refuse_next).take() {
            return Ok(RelayOutcome::Refused {
                reason,
                keys: vec![],
            });
        }
        if let Err(error) = (request.before_type)() {
            let HookError { kind, message, .. } = error;
            return Err(LauncherError::new(kind, message));
        }
        if let Some(during) = lock(&self.during_answer).take() {
            during();
        }
        lock(&self.typed).push((agent_id.to_string(), request.answer.clone()));
        lock(&self.screens).insert(
            agent_id.into(),
            CaptureOutcome::Refused(RelayRefusal::NotBlocked),
        );
        Ok(RelayOutcome::Typed {
            keys: vec!["enter".into()],
            input_readable: None,
        })
    }
    fn interrupt(&self, agent_id: &str) -> LauncherResult<bool> {
        self.stub.interrupt(agent_id)
    }
    fn status(&self) -> LauncherStatus {
        self.stub.status()
    }
    fn operator_environment(&self) -> BTreeMap<String, String> {
        self.stub.operator_environment()
    }
    fn in_flight_operations(&self) -> usize {
        self.stub.in_flight_operations()
    }
    fn adopt_all(&self) -> LauncherResult<()> {
        self.stub.adopt_all()
    }
    fn teardown(&self, agent_id: &str, worktree_path: &str) -> LauncherResult<SetupOutcome> {
        self.stub.teardown(agent_id, worktree_path)
    }
}

// ------------------------------------------------------------------------------------------------ prompts

const DASH: &str = "╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌";

fn option(number: i64, text: &str, accepts_text: bool, widens: bool) -> RelayOption {
    RelayOption {
        number,
        text: text.into(),
        accepts_text,
        widens_permissions: widens,
    }
}

fn prompt(agent: &str, text: String, options: Vec<RelayOption>) -> CapturedPrompt {
    let pane = format!("pane-{agent}");
    let prompt_sha = prompt_hash(&HashInput {
        agent_id: agent,
        pane_id: &pane,
        host_kind: "claude",
        text: &text,
        options: &options,
        dialog: false,
    });
    CapturedPrompt {
        agent_id: agent.into(),
        pane_id: pane,
        host_kind: "claude".into(),
        text,
        options,
        prompt_sha,
        dialog: false,
    }
}

fn plain_options() -> Vec<RelayOption> {
    vec![
        option(1, "Yes", false, false),
        option(
            2,
            "Yes, and always allow access to /w from this project",
            false,
            true,
        ),
        option(3, "No", true, false),
    ]
}

/// A Bash prompt for `command` (a single line as the dialog shows it).
fn bash(agent: &str, command: &str) -> CapturedPrompt {
    prompt(
        agent,
        format!(" Bash command\n Run it\n{DASH}\n {command}\n{DASH}\n Do you want to proceed?"),
        plain_options(),
    )
}

/// A file prompt (an edit: its `Yes` takes no text).
fn edit(agent: &str, path: &str) -> CapturedPrompt {
    let name = path.rsplit('/').next().unwrap();
    prompt(
        agent,
        format!(" Edit file\n {path}\n{DASH}\n  1 hi\n{DASH}\n Do you want to make this edit to {name}?"),
        vec![
            option(1, "Yes", false, false),
            option(2, "Yes, and switch to accept edits for this session", false, true),
            option(3, "No", false, false),
        ],
    )
}

fn fetch(agent: &str) -> CapturedPrompt {
    prompt(
        agent,
        " Fetch\n https://example.com\n Do you want to allow Claude to fetch this content?".into(),
        vec![
            option(1, "Yes", false, false),
            option(2, "No", false, false),
        ],
    )
}

// ------------------------------------------------------------------------------------------------ the world

struct World {
    _dir: common::PrivateDir,
    deps: Deps,
    clock: Arc<AtomicI64>,
    panes: Arc<Panes>,
    pm: String,
    logs: Arc<Mutex<Vec<(String, Value)>>>,
    auto: Mutex<AutoAnswer>,
}

impl World {
    fn new() -> World {
        World::with("", true)
    }

    fn with(operator: &str, relay: bool) -> World {
        let dir = private_tempdir();
        let identity = Identity {
            project_id: "pautoans".into(),
            name: "AutoAnswer".into(),
            credential: format!("operator-{}", "d".repeat(40)),
        };
        let state = write_project(dir.path(), &identity);
        let config = BASE_CONFIG
            .replace("__OPERATOR__", operator)
            .replace("__RELAY__", if relay { "true" } else { "false" });
        let file = dir.path().join("capstan.toml");
        std::fs::write(&file, config).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        let database = capstan_ledger::resolve_database_path(&state).unwrap();
        capstan_ledger::open_database(&database, &capstan_ledger::OpenOptions::default()).unwrap();
        let project = identity.initial_project();
        let clock = Arc::new(AtomicI64::new(1_800_000_000_000));
        let kernel = {
            let (state, project, root, clock): (_, InitialProject, _, _) = (
                state.clone(),
                project.clone(),
                dir.path().to_path_buf(),
                Arc::clone(&clock),
            );
            KernelHandle::spawn(move || {
                Core::open(
                    &state,
                    &project,
                    &KernelOptions {
                        workspace_root: Some(root),
                        ..KernelOptions::default()
                    },
                    Box::new(ClockEnv(clock)),
                )
            })
            .expect("the kernel opens")
        };
        let logs: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
        let sink: Logger = {
            let logs = Arc::clone(&logs);
            Arc::new(move |entry: &LogEntry| {
                lock(&logs).push((
                    entry.command.clone(),
                    entry.detail.clone().unwrap_or(Value::Null),
                ))
            })
        };
        let mut options = DaemonOptions::new(state, project, dir.path().to_path_buf());
        options.capstan = Some(Arc::new(
            capstan_config::load_role_config(dir.path()).unwrap(),
        ));
        let panes = Panes::new();
        let mut deps = Deps::new(kernel, options, sink);
        deps.launcher = Some(panes.clone() as Arc<dyn LauncherService>);
        capstan_daemon::deps::startup_sequence(&deps);
        let log: DriverLog = {
            let logs = Arc::clone(&logs);
            Arc::new(move |event, details| lock(&logs).push((event.to_string(), details)))
        };
        let auto = Mutex::new(AutoAnswer::new(AutoAnswerOptions {
            deps: deps.clone(),
            log,
        }));
        let world = World {
            _dir: dir,
            deps,
            clock,
            panes,
            pm: String::new(),
            logs,
            auto,
        };
        world.populate(identity.credential)
    }

    fn ledger(&self, credential: &str, op: &str, args: Vec<Value>) -> Value {
        let (credential, name) = (credential.to_string(), op.to_string());
        self.deps
            .kernel
            .run(move |core| {
                let context = capstan_daemon::deps::new_context(core, &credential)?;
                let mut all = vec![serde_json::to_value(context).unwrap()];
                all.extend(args);
                core.dispatch(&name, &all)
            })
            .unwrap_or_else(|e| panic!("{op}: {e}"))
    }

    fn populate(mut self, owner: String) -> World {
        let roles: Vec<Value> = [("pm", "PM"), ("developer", "Developer"), ("operator", "Developer")]
            .iter()
            .map(|(name, kind)| {
                json!({"name": name, "kind": kind, "host": "claude", "configHash": "a".repeat(64)})
            })
            .collect();
        self.ledger(&owner, "syncRoleDefinitions", vec![json!(roles)]);
        for (agent, role, kind) in [
            ("pm-1", "pm", "PM"),
            ("dev-1", "developer", "Developer"),
            ("dev-2", "developer", "Developer"),
        ] {
            let seat = format!("seat-{agent}");
            self.ledger(
                &owner,
                "createSeat",
                vec![json!({"seatId": seat, "name": seat, "role": kind})],
            );
            let actor = self.ledger(
                &owner,
                "createActor",
                vec![json!({"displayName": agent, "role": kind, "seatId": seat})],
            );
            self.ledger(
                &owner,
                "registerAgent",
                vec![json!({"agentId": agent, "roleName": role, "seatId": seat, "actorId": actor["actorId"]})],
            );
            if agent == "pm-1" {
                self.pm = actor["credential"].as_str().unwrap().to_string();
            }
        }
        let internal = self.deps.credential().to_string();
        for agent in ["dev-1", "dev-2"] {
            self.ledger(
                &internal,
                "recordAgentPane",
                vec![json!({
                    "agentId": agent,
                    "workspaceId": "ws-1",
                    "paneId": format!("pane-{agent}"),
                    "worktreePath": format!("/tmp/wt/{agent}"),
                    "branch": format!("feat/{agent}"),
                    "baseSha": "a".repeat(40),
                })],
            );
        }
        self
    }

    fn pm_call(&self, args: &[&str]) -> Value {
        let frame = json!({"v": 1, "credential": self.pm, "command": "prompt", "args": args});
        let outcome = handle_frame(
            &self.deps,
            frame.to_string().as_bytes(),
            &AbortSignal::new(),
            MAX_RESPONSE_BYTES,
        );
        serde_json::from_str(&outcome.response.expect("an answer")).expect("JSON")
    }

    fn pm_ok(&self, args: &[&str]) -> Value {
        let answer = self.pm_call(args);
        assert_eq!(answer["ok"], true, "{args:?}: {answer}");
        answer["result"].clone()
    }

    fn pm_refused(&self, args: &[&str]) -> String {
        let answer = self.pm_call(args);
        assert_eq!(answer["ok"], false, "{args:?}: {answer}");
        answer["message"].as_str().unwrap().to_string()
    }

    /// The PM shows dev-2's prompt and answers `option 1 --session auto=<minutes>`: the window opens.
    fn open_window(&self, minutes: &str) -> Value {
        self.panes.show("dev-2", bash("dev-2", "ls"));
        let shown = self.pm_ok(&["show", "dev-2"]);
        let relay = shown["relayId"].as_str().unwrap().to_string();
        let hash = shown["hash"].as_str().unwrap().to_string();
        let session = format!("auto={minutes}");
        let answered = self.pm_ok(&[
            "answer",
            &relay,
            "--hash",
            &hash,
            "option",
            "1",
            "--session",
            &session,
        ]);
        answered["window"].clone()
    }

    fn tick(&self) {
        lock(&self.auto).tick();
    }

    fn advance_minutes(&self, minutes: i64) {
        self.clock.fetch_add(minutes * 60_000, Ordering::SeqCst);
    }

    fn relays(&self) -> Vec<Value> {
        self.deps
            .kernel
            .run(|core| core.list_prompt_relays(Some(50)))
            .unwrap()
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    fn relay_of(&self, agent: &str) -> Vec<Value> {
        self.relays()
            .into_iter()
            .filter(|r| r["agentId"] == agent)
            .collect()
    }

    fn pm_messages(&self) -> Vec<String> {
        self.deps
            .kernel
            .run(|core| core.messages_for("pm-1"))
            .unwrap()
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|m| m["body"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn pm_notices(&self, needle: &str) -> Vec<String> {
        self.pm_messages()
            .into_iter()
            .filter(|b| b.contains(needle))
            .collect()
    }

    fn logged(&self, event: &str) -> Vec<Value> {
        lock(&self.logs)
            .iter()
            .filter(|(name, _)| name == event || name == &format!("daemon:{event}"))
            .map(|(_, details)| details.clone())
            .collect()
    }

    fn active_window(&self) -> Value {
        self.deps
            .kernel
            .run(|core| core.active_approval_window())
            .unwrap()
    }
}

/// How many answers dev-1 got typed.
fn typed_for(world: &World, agent: &str) -> Vec<PromptAnswer> {
    world
        .panes
        .typed()
        .into_iter()
        .filter(|(a, _)| a == agent)
        .map(|(_, answer)| answer)
        .collect()
}

// ------------------------------------------------------------------------------------------------ answering

#[test]
fn a_clear_command_is_answered_by_the_controller_inside_the_window() {
    let w = World::new();
    w.open_window("60");
    let before = w.panes.typed().len();
    w.panes.show("dev-1", bash("dev-1", "cargo test"));
    w.tick();
    assert_eq!(
        typed_for(&w, "dev-1"),
        vec![PromptAnswer::Option { number: 1 }]
    );
    assert_eq!(w.panes.typed().len(), before + 1);
    let relay = &w.relay_of("dev-1")[0];
    assert_eq!(relay["state"], "answered");
    assert_eq!(relay["answer"]["option"], 1);
    let window = w.active_window()["windowId"].as_str().unwrap().to_string();
    let stored: String = {
        let row = rusqlite::Connection::open(
            capstan_ledger::resolve_database_path(&w.deps.options.state_directory).unwrap(),
        )
        .unwrap();
        row.query_row(
            "SELECT auto_window_id FROM prompt_relays WHERE agent_id = 'dev-1'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(stored, window);
    assert_eq!(
        w.pm_notices("of dev-1 was answered with option 1 by approval window")
            .len(),
        1
    );
    // The same prompt is never looked at twice.
    w.tick();
    assert_eq!(typed_for(&w, "dev-1").len(), 1);
}

#[test]
fn a_script_is_answered_with_the_unchecked_reason() {
    let w = World::new();
    w.open_window("60");
    w.panes.show("dev-1", bash("dev-1", "bash x.sh"));
    w.tick();
    assert_eq!(typed_for(&w, "dev-1").len(), 1);
    let answered = w.logged("auto_answered");
    assert_eq!(answered.len(), 1);
    assert!(answered[0]["unchecked"].is_string(), "{answered:?}");
    let note = &w.pm_notices("unchecked");
    assert_eq!(note.len(), 1, "{:?}", w.pm_messages());
}

#[test]
fn a_command_on_the_floor_gets_a_notice_and_no_answer() {
    for command in ["git push --force", "bash -c \"git push -f\""] {
        let w = World::new();
        w.open_window("60");
        w.panes.show("dev-1", bash("dev-1", command));
        w.tick();
        assert!(typed_for(&w, "dev-1").is_empty(), "{command}");
        let notices = w.pm_notices("needs the user even inside approval window");
        assert_eq!(notices.len(), 1, "{command}: {:?}", w.pm_messages());
        // The notice is told once per relay.
        w.tick();
        assert_eq!(
            w.pm_notices("needs the user even inside approval window")
                .len(),
            1
        );
        assert_eq!(w.relay_of("dev-1")[0]["state"], "captured");
    }
}

#[test]
fn a_prompt_that_cannot_be_read_whole_goes_to_the_pm() {
    let w = World::new();
    w.open_window("60");
    for command in ["cargo test -p … --all", "echo one\n echo two"] {
        w.panes.show("dev-1", bash("dev-1", command));
        w.tick();
    }
    let mut dialog = bash("dev-1", "ls");
    dialog.dialog = true;
    dialog.options.clear();
    w.panes.show("dev-1", dialog);
    w.tick();
    assert!(typed_for(&w, "dev-1").is_empty());
    assert!(w.relay_of("dev-1").is_empty(), "{:?}", w.relays());
    assert!(w.pm_notices("needs the user").is_empty());
}

#[test]
fn nothing_is_answered_without_an_open_window() {
    let w = World::new();
    w.panes.show("dev-1", bash("dev-1", "cargo test"));
    w.tick();
    assert!(typed_for(&w, "dev-1").is_empty());
    // An expired window answers nothing, and ends with a notice.
    w.open_window("5");
    w.advance_minutes(6);
    w.tick();
    assert!(typed_for(&w, "dev-1").is_empty());
    assert_eq!(w.active_window(), Value::Null);
    assert_eq!(w.pm_notices("reached its time cap").len(), 1);
    // A revoked one neither.
    w.open_window("30");
    let id = w.active_window()["windowId"].as_str().unwrap().to_string();
    let revoke = w.deps.credential().to_string();
    let ended = w.deps.kernel.run(move |core| {
        let context = capstan_daemon::deps::new_context(core, &revoke)?;
        core.end_approval_window(&context, &json!({"windowId": id}))
    });
    assert!(ended.is_ok(), "{ended:?}");
    w.tick();
    assert!(typed_for(&w, "dev-1").is_empty());
}

#[test]
fn a_refused_answer_is_tried_once() {
    let w = World::new();
    w.open_window("60");
    *lock(&w.panes.refuse_next) = Some(RelayRefusal::SelectionNotReached);
    w.panes.show("dev-1", bash("dev-1", "cargo test"));
    w.tick();
    w.tick();
    w.tick();
    assert!(typed_for(&w, "dev-1").is_empty());
    assert_eq!(
        w.panes.answer_calls(),
        2,
        "dev-2's open and one refused try"
    );
    assert_eq!(w.relay_of("dev-1")[0]["state"], "refused");
    assert_eq!(w.logged("auto_answer_refused").len(), 1);
}

#[test]
fn a_prompt_seen_again_after_the_agent_moved_on_is_a_new_attempt() {
    let w = World::new();
    w.open_window("60");
    for _ in 0..2 {
        w.panes.show("dev-1", bash("dev-1", "cargo test"));
        w.tick();
        w.tick();
    }
    assert_eq!(typed_for(&w, "dev-1").len(), 2);
}

#[test]
fn the_pm_is_never_answered_and_with_the_relay_off_only_the_expiry_runs() {
    let w = World::new();
    w.open_window("60");
    w.panes.show("pm-1", bash("pm-1", "cargo test"));
    w.tick();
    assert!(typed_for(&w, "pm-1").is_empty());

    let off = World::with("", false);
    off.panes.show("dev-1", bash("dev-1", "cargo test"));
    off.tick();
    assert!(typed_for(&off, "dev-1").is_empty());
}

// ------------------------------------------------------------------------------------------------ files and tools

#[test]
fn a_file_prompt_is_answered_inside_the_worktree_and_held_outside_it() {
    let w = World::new();
    w.open_window("60");
    w.panes.show("dev-1", edit("dev-1", "src/lib.rs"));
    w.tick();
    assert_eq!(typed_for(&w, "dev-1").len(), 1);
    for (index, path) in ["/tmp/wt/dev-2/src/lib.rs", "/etc/passwd", "../dev-2/x.rs"]
        .iter()
        .enumerate()
    {
        w.panes.show("dev-1", edit("dev-1", path));
        w.tick();
        assert_eq!(typed_for(&w, "dev-1").len(), 1, "{path}");
        assert_eq!(
            w.pm_notices("needs the user even inside approval window")
                .len(),
            index + 1,
            "{path}: {:?}",
            w.pm_messages()
        );
        // The agent moves on before the next one.
        w.panes.show(
            "dev-1",
            CapturedPrompt {
                dialog: true,
                ..bash("dev-1", "ls")
            },
        );
        lock(&w.panes.screens).remove("dev-1");
        w.tick();
    }
}

#[test]
fn a_destructive_command_outside_the_worktree_is_on_the_floor() {
    let w = World::new();
    w.open_window("60");
    w.panes.show("dev-1", bash("dev-1", "rm -rf ../x"));
    w.tick();
    assert!(typed_for(&w, "dev-1").is_empty());
    assert_eq!(
        w.pm_notices("needs the user even inside approval window")
            .len(),
        1
    );
}

#[test]
fn a_tool_prompt_is_answered_only_when_the_switch_is_on() {
    let on = World::new();
    on.open_window("60");
    on.panes.show("dev-1", fetch("dev-1"));
    on.tick();
    assert_eq!(typed_for(&on, "dev-1").len(), 1);
    assert_eq!(
        on.pm_notices("of dev-1 was answered with option 1 by approval window")
            .len(),
        1
    );

    let off = World::with("auto_window_tool_prompts = false", true);
    off.open_window("60");
    off.panes.show("dev-1", fetch("dev-1"));
    off.tick();
    assert!(typed_for(&off, "dev-1").is_empty());
    assert!(off.relay_of("dev-1").is_empty());
}

// ------------------------------------------------------------------------------------------------ the recheck

#[test]
fn a_live_option_that_widens_gets_no_keys_whatever_the_stored_options_say() {
    let w = World::new();
    w.open_window("60");
    let captured = bash("dev-1", "cargo test");
    let mut live = captured.clone();
    live.options[0].widens_permissions = true;
    live.prompt_sha = prompt_hash(&HashInput {
        agent_id: "dev-1",
        pane_id: "pane-dev-1",
        host_kind: "claude",
        text: &live.text,
        options: &live.options,
        dialog: false,
    });
    w.panes.show("dev-1", captured);
    lock(&w.panes.scripted).push_back(CaptureOutcome::Captured(bash("dev-1", "cargo test")));
    lock(&w.panes.scripted).push_back(CaptureOutcome::Captured(live));
    w.tick();
    assert!(typed_for(&w, "dev-1").is_empty());
    assert_eq!(w.relay_of("dev-1")[0]["state"], "refused");
    assert_eq!(w.logged("auto_answer_refused").len(), 1);
}

#[test]
fn a_screen_that_changes_before_the_send_gets_no_keys() {
    let w = World::new();
    w.open_window("60");
    w.panes.show("dev-1", bash("dev-1", "cargo test"));
    // A new command with the same option text: a new hash.
    *lock(&w.panes.swap_before_send) = Some(CaptureOutcome::Captured(bash("dev-1", "rm -rf /")));
    w.tick();
    assert!(typed_for(&w, "dev-1").is_empty());
    assert_eq!(w.relay_of("dev-1")[0]["state"], "refused");
}

// ------------------------------------------------------------------------------------------------ opening a window

#[test]
fn option_one_with_session_auto_types_it_and_opens_a_window_for_the_others() {
    let w = World::new();
    let window = w.open_window("60");
    assert_eq!(window["kind"], "hour");
    assert_eq!(window["minutes"], 60);
    assert_eq!(
        typed_for(&w, "dev-2"),
        vec![PromptAnswer::Option { number: 1 }]
    );
    assert_eq!(w.pm_notices("Approval window win-1 is open").len(), 1);
    let opened = &w.pm_notices("Approval window win-1 is open")[0];
    assert!(opened.contains("git push --force"), "{opened}");
    assert!(opened.contains("non-file tool prompts"), "{opened}");
    w.panes.show("dev-1", bash("dev-1", "cargo test"));
    w.tick();
    assert_eq!(typed_for(&w, "dev-1").len(), 1);
}

#[test]
fn a_session_auto_answer_with_anything_but_the_allow_once_option_types_nothing() {
    for (label, answer) in [
        ("widening", ["option", "2"].as_slice()),
        ("no", ["option", "3"].as_slice()),
        ("esc", ["esc"].as_slice()),
        ("text", ["text", "hello"].as_slice()),
    ] {
        let w = World::new();
        w.panes.show("dev-2", bash("dev-2", "ls"));
        let shown = w.pm_ok(&["show", "dev-2"]);
        let (relay, hash) = (
            shown["relayId"].as_str().unwrap().to_string(),
            shown["hash"].as_str().unwrap().to_string(),
        );
        let mut args = vec!["answer", relay.as_str(), "--hash", hash.as_str()];
        args.extend(answer);
        args.extend(["--session", "auto=60"]);
        w.pm_refused(&args);
        assert!(w.panes.typed().is_empty(), "{label}");
        assert_eq!(w.active_window(), Value::Null, "{label}");
    }
}

#[test]
fn a_bad_window_length_or_a_timed_session_is_refused_with_nothing_typed() {
    let w = World::new();
    w.panes.show("dev-2", bash("dev-2", "ls"));
    let shown = w.pm_ok(&["show", "dev-2"]);
    let (relay, hash) = (
        shown["relayId"].as_str().unwrap().to_string(),
        shown["hash"].as_str().unwrap().to_string(),
    );
    for value in [
        "auto=0", "auto=999", "auto=x", "timed=60", "timed", "exact", "auto=",
    ] {
        let message = w.pm_refused(&[
            "answer",
            &relay,
            "--hash",
            &hash,
            "option",
            "1",
            "--session",
            value,
        ]);
        if value.starts_with("timed") {
            assert!(message.contains("timed grants were replaced"), "{message}");
        }
        assert!(w.panes.typed().is_empty(), "{value}");
        assert_eq!(w.active_window(), Value::Null, "{value}");
    }
    // The plain answer still works.
    w.pm_ok(&["answer", &relay, "--hash", &hash, "option", "1"]);
    assert_eq!(w.panes.typed().len(), 1);
    assert_eq!(w.active_window(), Value::Null);
}

// ------------------------------------------------------------------------------------------------ the race

#[test]
fn a_pm_show_during_an_attempt_is_told_to_wait_and_a_second_begin_is_refused() {
    let w = World::new();
    w.open_window("60");
    w.panes.show("dev-1", bash("dev-1", "cargo test"));
    let (message, second_begin) = {
        let seen: Arc<Mutex<Option<(String, String)>>> = Arc::default();
        let (deps, pm, slot) = (w.deps.clone(), w.pm.clone(), Arc::clone(&seen));
        *lock(&w.panes.during_answer) = Some(Box::new(move || {
            let frame =
                json!({"v": 1, "credential": pm, "command": "prompt", "args": ["show", "dev-1"]});
            let outcome = handle_frame(
                &deps,
                frame.to_string().as_bytes(),
                &AbortSignal::new(),
                MAX_RESPONSE_BYTES,
            );
            let shown: Value = serde_json::from_str(&outcome.response.unwrap()).unwrap();
            // A second begin on the row that is typing.
            let credential = deps.credential().to_string();
            let begin = deps.kernel.run(move |core| {
                let relay = core.list_prompt_relays(Some(1))?;
                let relay = relay[0].clone();
                let context = capstan_daemon::deps::new_context(core, &credential)?;
                core.begin_prompt_answer(
                    &context,
                    &json!({"relayId": relay["relayId"], "hash": relay["hash12"], "answer": {"kind": "option", "number": 1}, "autoWindowId": "win-1"}),
                )
            });
            *lock(&slot) = Some((
                shown["message"].as_str().unwrap_or("").to_string(),
                begin.err().map(|e| e.to_string()).unwrap_or_default(),
            ));
        }));
        w.tick();
        let result = lock(&seen).clone();
        result.expect("the hook ran")
    };
    assert!(message.contains("auto_answer_in_progress"), "{message}");
    assert!(!second_begin.is_empty(), "a second begin is refused");
    assert_eq!(typed_for(&w, "dev-1").len(), 1);
    // After the attempt the PM may look again.
    assert!(!attempts_of(&w.deps).running("dev-1", w.deps.kernel.now_ms()));
}

#[test]
fn a_stuck_marker_clears_after_thirty_seconds() {
    let attempts = Attempts::default();
    assert!(attempts.start("dev-1", 1_000));
    assert!(!attempts.start("dev-1", 1_000 + ATTEMPT_STALE_MS - 1));
    assert!(attempts.running("dev-1", 1_000 + ATTEMPT_STALE_MS - 1));
    assert!(!attempts.running("dev-1", 1_000 + ATTEMPT_STALE_MS));
    assert!(attempts.start("dev-1", 1_000 + ATTEMPT_STALE_MS));
    attempts.finish("dev-1");
    assert!(!attempts.running("dev-1", 1_000 + ATTEMPT_STALE_MS));
}

// ------------------------------------------------------------------------------------------------ restart

#[test]
fn a_restart_ends_the_window_and_fails_the_row_that_was_typing() {
    let w = World::new();
    w.open_window("60");
    w.panes.show("dev-1", bash("dev-1", "cargo test"));
    // The controller reached typing and died before the keys.
    let credential = w.deps.credential().to_string();
    w.deps
        .kernel
        .run(move |core| {
            let context = capstan_daemon::deps::new_context(core, &credential)?;
            let relay = core.record_prompt_capture(
                &context,
                &json!({"autoWindowId": "win-1", "prompt": {
                    "agentId": "dev-1", "paneId": "pane-dev-1", "hostKind": "claude", "text": "x",
                    "options": [{"number": 1, "text": "Yes", "acceptsText": false, "widensPermissions": false}],
                    "promptSha": "ab".repeat(32),
                }}),
            )?;
            let context = capstan_daemon::deps::new_context(core, &credential)?;
            core.begin_prompt_answer(
                &context,
                &json!({"relayId": relay["relayId"], "hash": relay["hash12"], "answer": {"kind": "option", "number": 1}, "autoWindowId": "win-1"}),
            )
        })
        .unwrap();
    assert_eq!(w.relay_of("dev-1")[0]["state"], "typing");
    capstan_daemon::deps::startup_sequence(&w.deps);
    capstan_daemon::run::end_approval_windows_at_start(&w.deps);
    assert_eq!(w.relay_of("dev-1")[0]["state"], "failed");
    assert_eq!(w.active_window(), Value::Null);
    // The agent is still blocked, and nothing is answered until a new window opens.
    w.tick();
    w.tick();
    assert!(typed_for(&w, "dev-1").is_empty());
}

#[test]
fn a_controller_that_may_not_relay_never_answers_and_the_pm_is_told() {
    let w = World::new();
    w.open_window("60");
    let database = capstan_ledger::resolve_database_path(&w.deps.options.state_directory).unwrap();
    let db = rusqlite::Connection::open(&database).unwrap();
    db.execute(
        "UPDATE capability_grants SET revoked_at = '2030-01-01T00:00:00.000Z' WHERE capability = 'prompt:relay' AND actor_id IN (SELECT actor_id FROM actors WHERE is_internal = 1)",
        [],
    )
    .unwrap();
    drop(db);
    capstan_daemon::run::end_approval_windows_at_start(&w.deps);
    assert_eq!(w.logged("auto_answer_disabled").len(), 1);
    assert_eq!(w.pm_notices("will not answer worker prompts").len(), 1);
    assert!(attempts_of(&w.deps).disabled());
    w.open_window("60");
    w.panes.show("dev-1", bash("dev-1", "cargo test"));
    w.tick();
    assert!(typed_for(&w, "dev-1").is_empty());
}

// ------------------------------------------------------------------------------------------------ the driver

/// One driver tick over a message that waits for dev-1, whose input box a permission prompt covers; `answering` says an
/// auto-answer attempt is running on dev-1. Returns the PM's input-blocked notices.
fn blocked_notices_after_a_tick(answering: bool) -> usize {
    let w = World::new();
    let adapter = Arc::new(StubAdapter::new());
    adapter.register(
        "pane-dev-1",
        PaneEntry {
            role: PaneRole::Worker,
            phase: PanePhase::Started,
            kind: "claude".into(),
            agent: Some("dev-1".into()),
            worktree_path: None,
            workspace_id: Some("ws-1".into()),
        },
    );
    adapter.set_observation(capstan_herdr::api::HerdrState::Idle);
    adapter.set_send(SendOutcome::Deferred {
        reason: DeferralReason::InputNotEmpty,
        detail: Some(INPUT_UNREADABLE_DETAIL.to_string()),
        blocker: Some(InputBlocker::PermissionPrompt),
    });
    let owner = w.deps.credential().to_string();
    w.ledger(
        &owner,
        "enqueueMessage",
        vec![json!({"recipientAgentId": "dev-1", "body": "please look at this"})],
    );
    let clock = Arc::clone(&w.clock);
    let notifier_clock = Arc::clone(&w.clock);
    let notifier = Arc::new(Notifier::new(
        adapter.clone(),
        false,
        false,
        w.deps.options.state_directory.join("notifications.jsonl"),
        Arc::new(move || notifier_clock.load(Ordering::SeqCst)),
        Arc::new(|_: &str, _: Value| {}),
    ));
    let timers = capstan_daemon::loops::timers_value(w.deps.options.capstan.as_ref().unwrap());
    let mut driver = Driver::new(DriverOptions {
        kernel: w.deps.kernel.clone(),
        adapter: adapter.clone(),
        timers,
        notifier,
        credential: owner,
        now: Arc::new(move || clock.load(Ordering::SeqCst)),
        log: Arc::new(|_: &str, _: Value| {}),
        tick_ms: None,
        process_probe: None,
        pm_stale_seconds: 1200,
        liveness: None,
        stall_child_max_seconds: 3600,
        snapshot: None,
    });
    let attempts = attempts_of(&w.deps);
    driver.hold_blocked_notice_while(Arc::clone(&attempts));
    if answering {
        assert!(attempts.start("dev-1", w.clock.load(Ordering::SeqCst)));
    }
    driver.tick();
    w.pm_notices("cannot take messages").len()
}

#[test]
fn a_message_to_an_agent_being_auto_answered_gives_no_input_blocked_notice() {
    assert_eq!(blocked_notices_after_a_tick(false), 1, "the control");
    assert_eq!(blocked_notices_after_a_tick(true), 0);
}
