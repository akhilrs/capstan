//! A project with an owner, a PM and a developer, on a real ledger in a private temporary directory.
#![allow(dead_code)]

use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::types::{InitialProject, MutationContext, ProjectInput};
use capstan_kernel::{Core, SystemEnv};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const OWNER: &str = "operator-0123456789abcdef0123456789abcdef01234567";

pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub state: PathBuf,
    pub core: Option<Core>,
    pub pm_credential: String,
    pub pm_actor: String,
    pub developer_credential: String,
}

pub fn initial() -> InitialProject {
    InitialProject {
        project_id: "prustonly".into(),
        name: "rust-only".into(),
        owner_credential: OWNER.into(),
        initial_inputs: vec![
            ProjectInput {
                kind: "project_config".into(),
                content: json!({"placeholder": true}),
            },
            ProjectInput {
                kind: "task_brief".into(),
                content: json!({"placeholder": true}),
            },
            ProjectInput {
                kind: "acceptance_criteria".into(),
                content: json!(["(none yet)"]),
            },
            ProjectInput {
                kind: "policy".into(),
                content: json!({"placeholder": true}),
            },
            ProjectInput {
                kind: "plan".into(),
                content: json!({"placeholder": true}),
            },
        ],
    }
}

pub fn open(state: &Path) -> Core {
    Core::open(
        state,
        &initial(),
        &KernelOptions::default(),
        Box::new(SystemEnv),
    )
    .expect("the ledger opens")
}

pub fn context(core: &Core, credential: &str) -> MutationContext {
    MutationContext {
        credential: credential.to_string(),
        request_id: format!("req-{}", core.state_version().unwrap()),
        idempotency_key: format!("idem-{}", core.state_version().unwrap()),
        expected_version: core.state_version().unwrap(),
        input_revision: core.input_revision().unwrap(),
    }
}

fn step(core: &Core, op: &str, args: Value) -> Value {
    let context = serde_json::to_value(context(core, OWNER)).unwrap();
    let mut all = vec![context];
    all.extend(args.as_array().cloned().unwrap());
    core.dispatch(op, &all)
        .unwrap_or_else(|e| panic!("{op}: {e}"))
}

impl Fixture {
    pub fn new() -> Fixture {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let state = dir.path().join("state");
        std::fs::create_dir(&state).unwrap();
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
        let core = open(&state);
        step(
            &core,
            "syncRoleDefinitions",
            json!([[
                {"name": "pm", "kind": "PM", "host": "claude", "configHash": "b".repeat(64)},
                {"name": "developer", "kind": "Developer", "host": "claude", "configHash": "a".repeat(64)},
            ]]),
        );
        let mut credentials = Vec::new();
        for (agent, role_name, kind) in [("pm-1", "pm", "PM"), ("dev-1", "developer", "Developer")]
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
            credentials.push((
                actor["credential"].as_str().unwrap().to_string(),
                actor["actorId"].as_str().unwrap().to_string(),
            ));
        }
        Fixture {
            dir,
            state,
            pm_credential: credentials[0].0.clone(),
            pm_actor: credentials[0].1.clone(),
            developer_credential: credentials[1].0.clone(),
            core: Some(core),
        }
    }

    pub fn core(&self) -> &Core {
        self.core.as_ref().expect("the ledger is open")
    }

    /// Message `body` for the PM, sent by the owner.
    pub fn tell_pm(&self, body: &str) {
        let core = self.core();
        core.enqueue_message(
            &context(core, OWNER),
            &json!({"recipientAgentId": "pm-1", "body": body}),
        )
        .unwrap();
    }

    /// The summary of a PM generation restart.
    pub fn restart_pm(&self) -> Value {
        let core = self.core();
        core.restart_agent_generation(&context(core, OWNER), "pm-1")
            .unwrap()["summary"]
            .clone()
    }

    /// Closes the ledger so raw SQL can edit the file; `reopen` opens it again.
    pub fn close(&mut self) {
        if let Some(core) = self.core.take() {
            core.close();
        }
    }

    pub fn reopen(&mut self) {
        self.core = Some(open(&self.state));
    }

    pub fn database(&self) -> PathBuf {
        self.state.join("controller.sqlite")
    }
}
