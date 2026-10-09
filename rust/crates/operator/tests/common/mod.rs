#![allow(dead_code)]
//! A scratch project for the service tests: a kernel opened on a thread of its own (the daemon's one-kernel-thread
//! rule), one PM and one developer agent, and the `OperatorLedger` port over that thread.

use capstan_config::Operator;
use capstan_kernel::types::{InitialProject, ProjectInput};
use capstan_kernel::{Core, KernelOptions, KernelResult, SystemEnv};
use capstan_operator::service::{LedgerStep, OperatorLedger};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

type Job = (LedgerStep, Sender<KernelResult<Value>>);

pub struct ThreadLedger {
    jobs: Mutex<Sender<Job>>,
}

impl OperatorLedger for ThreadLedger {
    fn run(&self, step: LedgerStep) -> KernelResult<Value> {
        let (reply, answer) = channel();
        self.jobs
            .lock()
            .unwrap()
            .send((step, reply))
            .expect("the kernel thread runs");
        answer.recv().expect("the kernel thread answers")
    }
}

pub const OWNER: &str = "owner-credential-0123456789-0123456789-abcdef";

pub struct World {
    pub dir: tempfile::TempDir,
    pub root: PathBuf,
    pub state: PathBuf,
    pub ledger: Arc<ThreadLedger>,
    /// Credential by agent id: `pm-1` and `dev-1`.
    pub agents: HashMap<String, String>,
}

fn project() -> InitialProject {
    let placeholder = |kind: &str| ProjectInput {
        kind: kind.into(),
        content: json!({"placeholder": true}),
    };
    InitialProject {
        project_id: "poperator".into(),
        name: "Operator test".into(),
        owner_credential: OWNER.into(),
        initial_inputs: vec![
            placeholder("project_config"),
            placeholder("task_brief"),
            ProjectInput {
                kind: "acceptance_criteria".into(),
                content: json!(["criterion"]),
            },
            placeholder("policy"),
            placeholder("plan"),
        ],
    }
}

fn step(core: &Core, op: &str, args: Value) -> Value {
    let context = json!({
        "credential": OWNER,
        "requestId": format!("req-{op}-{}", core.state_version().unwrap()),
        "idempotencyKey": format!("idem-{op}-{}", core.state_version().unwrap()),
        "expectedVersion": core.state_version().unwrap(),
        "inputRevision": core.input_revision().unwrap(),
    });
    let mut all = vec![context];
    all.extend(args.as_array().cloned().unwrap());
    core.dispatch(op, &all)
        .unwrap_or_else(|e| panic!("setup {op}: {e}"))
}

impl World {
    pub fn new() -> World {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let state = root.join("state");
        std::fs::create_dir(&state).unwrap();
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
        let (jobs, receiver) = channel::<Job>();
        let (ready, ready_receiver) = channel::<HashMap<String, String>>();
        let (state_dir, workspace) = (state.clone(), root.clone());
        std::thread::spawn(move || {
            let core = Core::open(
                &state_dir,
                &project(),
                &KernelOptions {
                    workspace_root: Some(workspace),
                    ..KernelOptions::default()
                },
                Box::new(SystemEnv),
            )
            .expect("the kernel opens");
            step(
                &core,
                "syncRoleDefinitions",
                json!([[
                    {"name": "developer", "kind": "Developer", "host": "claude", "configHash": "a".repeat(64)},
                    {"name": "pm", "kind": "PM", "host": "claude", "configHash": "b".repeat(64)},
                ]]),
            );
            let mut credentials = HashMap::new();
            for (agent, role_name, kind) in
                [("pm-1", "pm", "PM"), ("dev-1", "developer", "Developer")]
            {
                let seat = format!("seat-{agent}");
                step(
                    &core,
                    "createSeat",
                    json!([{"seatId": seat, "name": seat, "role": kind}]),
                );
                let actor = step(
                    &core,
                    "createActor",
                    json!([{"displayName": agent, "role": kind, "seatId": seat}]),
                );
                step(
                    &core,
                    "registerAgent",
                    json!([{"agentId": agent, "roleName": role_name, "seatId": seat, "actorId": actor["actorId"]}]),
                );
                credentials.insert(
                    agent.to_string(),
                    actor["credential"].as_str().unwrap().to_string(),
                );
            }
            ready.send(credentials).unwrap();
            while let Ok((job, reply)) = receiver.recv() {
                let _ = reply.send(job(&core));
            }
            core.close();
        });
        let agents = ready_receiver.recv().expect("the kernel thread is ready");
        World {
            dir,
            root,
            state,
            ledger: Arc::new(ThreadLedger {
                jobs: Mutex::new(jobs),
            }),
            agents,
        }
    }

    pub fn pm(&self) -> &str {
        &self.agents["pm-1"]
    }

    pub fn developer(&self) -> &str {
        &self.agents["dev-1"]
    }

    /// Polls `show` until the proposal is in `state`; panics after 15 seconds.
    pub fn wait_state(&self, show: &dyn Fn() -> Option<Value>, state: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(record) = show() {
                if record["state"] == state {
                    return record;
                }
            }
            assert!(
                Instant::now() < deadline,
                "the proposal never reached {state}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

pub fn operator_config(overrides: impl FnOnce(&mut Operator)) -> Operator {
    let mut config = Operator {
        configured: true,
        enabled: true,
        role: "developer".into(),
        auto_approve: vec![],
        auto_approve_prefix: vec![],
        timeout_seconds: 300,
        max_timeout_seconds: 1800,
        output_tail_bytes: 8192,
        proposal_ttl_minutes: 60,
        approval_ttl_minutes: 10,
        max_pending_proposals: 5,
        count_toward_worker_limit: false,
        restart_health_timeout_seconds: 60,
        restart_idle_wait_seconds: 120,
        session_grant_max_minutes: 60,
        full_auto_default_minutes: 30,
        full_auto_max_minutes: 120,
    };
    overrides(&mut config);
    config
}

pub fn path_str(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
