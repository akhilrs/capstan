//! The controller answers inside an approval window as the internal principal even though the daemon asks with the
//! operator-role project credential (the internal principal's own credential is never kept). A PM or a worker that names a
//! window is refused, and the PM's flows without a window are as they were.

use crate::fixture::{context, initial, step, Fixture, OWNER};
use capstan_kernel::canonical::sha256;
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::{Core, KernelError, SystemEnv};
use serde_json::{json, Value};

const OPTIONS: &str = r#"[{"number":1,"text":"Yes","acceptsText":false,"widensPermissions":false},{"number":2,"text":"No","acceptsText":false,"widensPermissions":false}]"#;

struct World {
    f: Fixture,
    core: Core,
}

fn add_agent(f: &Fixture, agent: &str) {
    let core = f.core();
    let seat = format!("seat-{agent}");
    step(
        core,
        "createSeat",
        json!([{"seatId": seat, "name": seat, "role": "Developer"}]),
    );
    let actor = step(
        core,
        "createActor",
        json!([{"displayName": agent, "role": "Developer", "seatId": seat}]),
    );
    step(
        core,
        "registerAgent",
        json!([{"agentId": agent, "roleName": "developer", "seatId": seat, "actorId": actor["actorId"]}]),
    );
}

impl World {
    fn new() -> World {
        let mut f = Fixture::new();
        add_agent(&f, "dev-2");
        f.close();
        let core = Core::open(
            &f.state,
            &initial(),
            &KernelOptions::default(),
            Box::new(SystemEnv),
        )
        .unwrap();
        core.configure_prompt_relay(&json!({"enabled": true, "captureTtlSeconds": 600}))
            .unwrap();
        World { f, core }
    }

    fn sql<T>(&self, read: impl FnOnce(&rusqlite::Connection) -> T) -> T {
        read(&rusqlite::Connection::open(self.f.database()).unwrap())
    }

    fn ctx(&self, credential: &str) -> capstan_kernel::types::MutationContext {
        context(&self.core, credential)
    }

    /// Opens a window through a decision of the PM on a developer's proposal.
    fn open(&self) {
        let proposal = self
            .core
            .propose_operator_action(
                &self.ctx(&self.f.developer_credential),
                &json!({"kind": "command", "command": "ls -la", "reason": "look", "maxPending": 20}),
            )
            .unwrap();
        self.core
            .decide_operator_proposal(
                &self.ctx(&self.f.pm_credential),
                &json!({
                    "proposalId": proposal["proposalId"],
                    "decision": "approve",
                    "hash": proposal["commandSha"].as_str().unwrap()[..12],
                    "proposalTtlMinutes": 100000,
                    "session": {"kind": "window", "windowKind": "minutes", "maxMinutes": 30},
                }),
            )
            .unwrap();
    }

    fn capture(&self, credential: &str, auto: Option<&str>) -> Result<Value, KernelError> {
        let mut input = json!({"prompt": {
            "agentId": "dev-2", "paneId": "pane-1", "hostKind": "claude", "text": "Allow?",
            "options": serde_json::from_str::<Value>(OPTIONS).unwrap(),
            "promptSha": sha256(&format!("dev-2{}", self.core.state_version().unwrap())),
        }});
        if let Some(window) = auto {
            input["autoWindowId"] = json!(window);
        }
        self.core
            .record_prompt_capture(&self.ctx(credential), &input)
    }

    fn begin(&self, credential: &str, relay: &Value, extra: Value) -> Result<Value, KernelError> {
        let mut input = json!({"relayId": relay["relayId"], "hash": relay["promptSha"], "answer": {"kind": "option", "number": 1}});
        for (key, value) in extra.as_object().unwrap() {
            input[key] = value.clone();
        }
        self.core.begin_prompt_answer(&self.ctx(credential), &input)
    }

    fn internal_actor(&self) -> String {
        self.sql(|db| {
            db.query_row(
                "SELECT actor_id FROM actors WHERE is_internal = 1",
                [],
                |r| r.get(0),
            )
            .unwrap()
        })
    }
}

fn error(outcome: Result<Value, KernelError>) -> String {
    outcome.expect_err("refused").to_string()
}

#[test]
fn an_operator_caller_captures_and_answers_as_the_internal_controller() {
    let w = World::new();
    w.open();
    let relay = w.capture(OWNER, Some("win-1")).unwrap();
    assert_eq!(relay["state"], "captured");
    let begun = w
        .begin(
            OWNER,
            &relay,
            json!({"autoWindowId": "win-1", "uncheckedReason": "a shell it cannot parse"}),
        )
        .unwrap();
    assert_eq!(begun["state"], "typing");
    let stored: (String, String, String) = w.sql(|db| {
        db.query_row(
            "SELECT auto_window_id, answered_by_actor_id, captured_by_actor_id FROM prompt_relays WHERE relay_id = ?",
            [relay["relayId"].as_str().unwrap()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
    });
    assert_eq!(stored.0, "win-1");
    let internal = w.internal_actor();
    assert_eq!(stored.1, internal);
    assert_eq!(stored.2, internal);
}

#[test]
fn a_pm_or_a_worker_that_names_a_window_is_refused() {
    let w = World::new();
    w.open();
    for credential in [w.f.pm_credential.clone(), w.f.developer_credential.clone()] {
        let message = error(w.capture(&credential, Some("win-1")));
        assert!(message.contains("only the controller"), "{message}");
    }
    let relay = w.capture(OWNER, Some("win-1")).unwrap();
    for credential in [w.f.pm_credential.clone(), w.f.developer_credential.clone()] {
        let message = error(w.begin(&credential, &relay, json!({"autoWindowId": "win-1"})));
        assert!(message.contains("only the controller"), "{message}");
    }
    let still = w
        .core
        .prompt_relay(relay["relayId"].as_str().unwrap())
        .unwrap();
    assert_eq!(still["state"], "captured");
}

#[test]
fn an_internal_principal_without_prompt_relay_answers_nothing() {
    let mut w = World::new();
    w.open();
    w.core.close();
    let db = rusqlite::Connection::open(w.f.database()).unwrap();
    db.execute(
        "UPDATE capability_grants SET revoked_at = '2030-01-01T00:00:00.000Z' WHERE capability = 'prompt:relay' AND actor_id IN (SELECT actor_id FROM actors WHERE is_internal = 1)",
        [],
    )
    .unwrap();
    drop(db);
    w.core = Core::open(
        &w.f.state,
        &initial(),
        &KernelOptions::default(),
        Box::new(SystemEnv),
    )
    .unwrap();
    w.core
        .configure_prompt_relay(&json!({"enabled": true, "captureTtlSeconds": 600}))
        .unwrap();
    let message = error(w.capture(OWNER, Some("win-1")));
    assert!(message.contains("prompt:relay"), "{message}");
}

#[test]
fn the_pm_flow_without_a_window_is_unchanged() {
    let w = World::new();
    let pm = w.f.pm_credential.clone();
    let relay = w.capture(&pm, None).unwrap();
    let begun = w.begin(&pm, &relay, json!({})).unwrap();
    assert_eq!(begun["state"], "typing");
    let answered_by: String = w.sql(|db| {
        db.query_row("SELECT answered_by_actor_id FROM prompt_relays", [], |r| {
            r.get(0)
        })
        .unwrap()
    });
    assert_eq!(answered_by, w.f.pm_actor);
}
