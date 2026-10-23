//! The approval windows of migration 0047: how a window opens, is used by an Operator proposal and by the controller
//! answering a worker prompt, and every way it ends; the triggers that hold the controller to an open window; and the
//! texts of its notices. The clock is a cell the test moves.

use crate::fixture::{context, initial, step, Fixture, OWNER};
use capstan_kernel::canonical::sha256;
use capstan_kernel::env::{Env, SystemEnv};
use capstan_kernel::helpers::*;
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::{Core, KernelError};
use serde_json::{json, Value};
use std::cell::Cell;
use std::rc::Rc;

/// The system's uuids and randomness with a clock the test sets.
struct ClockEnv(Rc<Cell<i64>>);

impl Env for ClockEnv {
    fn now(&self) -> i64 {
        self.0.get()
    }
    fn uuid(&self) -> String {
        SystemEnv.uuid()
    }
    fn random_bytes(&self, n: usize) -> Vec<u8> {
        SystemEnv.random_bytes(n)
    }
}

const CONTROLLER: &str = "controller-credential-0123456789abcdef0123456789";
const OPTIONS: &str = r#"[{"number":1,"text":"Yes","acceptsText":false,"widensPermissions":false},{"number":2,"text":"Yes, always","acceptsText":false,"widensPermissions":true},{"number":3,"text":"No, tell it what to do","acceptsText":true,"widensPermissions":false}]"#;

struct World {
    f: Fixture,
    core: Core,
    clock: Rc<Cell<i64>>,
}

type Outcome = Result<Value, KernelError>;

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
        let db = rusqlite::Connection::open(f.database()).unwrap();
        db.execute(
            "UPDATE actors SET credential_hash = ? WHERE is_internal = 1",
            [sha256(CONTROLLER)],
        )
        .unwrap();
        drop(db);
        let clock = Rc::new(Cell::new(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64,
        ));
        let core = Core::open(
            &f.state,
            &initial(),
            &KernelOptions::default(),
            Box::new(ClockEnv(clock.clone())),
        )
        .unwrap();
        core.configure_prompt_relay(&json!({"enabled": true, "captureTtlSeconds": 600}))
            .unwrap();
        World { f, core, clock }
    }

    fn ctx(&self, credential: &str) -> capstan_kernel::types::MutationContext {
        context(&self.core, credential)
    }

    fn advance_minutes(&self, minutes: i64) {
        self.clock.set(self.clock.get() + minutes * 60_000);
    }

    fn sql<T>(&self, read: impl FnOnce(&rusqlite::Connection) -> T) -> T {
        read(&rusqlite::Connection::open(self.f.database()).unwrap())
    }

    fn propose(&self, extra: Value) -> Outcome {
        let mut input =
            json!({"kind": "command", "command": "ls -la", "reason": "look", "maxPending": 20});
        for (key, value) in extra.as_object().unwrap() {
            input[key] = value.clone();
        }
        self.core
            .propose_operator_action(&self.ctx(&self.f.developer_credential), &input)
    }

    fn decide(&self, proposal: &Value, session: Option<Value>) -> Outcome {
        let mut input = json!({
            "proposalId": proposal["proposalId"],
            "decision": "approve",
            "hash": proposal["commandSha"].as_str().unwrap()[..12],
            "proposalTtlMinutes": 100000,
        });
        if let Some(session) = session {
            input["session"] = session;
        }
        self.core
            .decide_operator_proposal(&self.ctx(&self.f.pm_credential), &input)
    }

    /// Opens a window through a decision.
    fn open(&self, kind: &str, minutes: i64) -> Value {
        let proposal = self.propose(json!({})).unwrap();
        self.decide(
            &proposal,
            Some(json!({"kind": "window", "windowKind": kind, "maxMinutes": minutes})),
        )
        .unwrap()["window"]
            .clone()
    }

    fn capture(&self, agent: &str, auto: Option<&str>, credential: &str) -> Outcome {
        let mut input = json!({"prompt": {
            "agentId": agent, "paneId": "pane-1", "hostKind": "herdr", "text": "Allow?",
            "options": serde_json::from_str::<Value>(OPTIONS).unwrap(),
            "promptSha": sha256(&format!("{agent}{}", self.clock.get())),
        }});
        if let Some(window) = auto {
            input["autoWindowId"] = json!(window);
        }
        self.core
            .record_prompt_capture(&self.ctx(credential), &input)
    }

    fn begin(&self, relay: &Value, number: i64, extra: Value) -> Outcome {
        self.begin_answer(relay, json!({"kind": "option", "number": number}), extra)
    }

    fn begin_answer(&self, relay: &Value, answer: Value, extra: Value) -> Outcome {
        let mut input =
            json!({"relayId": relay["relayId"], "hash": relay["promptSha"], "answer": answer});
        for (key, value) in extra.as_object().unwrap() {
            input[key] = value.clone();
        }
        let credential = if input.get("autoWindowId").is_some() {
            CONTROLLER
        } else {
            &self.f.pm_credential
        };
        self.core.begin_prompt_answer(&self.ctx(credential), &input)
    }

    fn active(&self) -> Value {
        self.core.active_approval_window().unwrap()
    }

    fn window_row(&self, id: &str) -> (Option<String>, Option<String>) {
        self.sql(|db| {
            db.query_row(
                "SELECT ended_reason, operator_ended_reason FROM approval_windows WHERE window_id = ?",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
        })
    }

    fn notices(&self, agent: &str, needle: &str) -> Vec<String> {
        self.sql(|db| {
            let mut statement = db
                .prepare("SELECT body FROM messages WHERE recipient_agent_id = ? AND body LIKE ? ORDER BY sequence")
                .unwrap();
            let rows = statement
                .query_map([agent, &format!("%{needle}%")], |r| r.get::<_, String>(0))
                .unwrap();
            rows.map(Result::unwrap).collect()
        })
    }
}

fn error(outcome: Outcome) -> String {
    outcome.expect_err("refused").to_string()
}

fn ms(t: &str) -> i64 {
    let day: i64 = t[8..10].parse().unwrap();
    let hour: i64 = t[11..13].parse().unwrap();
    let minute: i64 = t[14..16].parse().unwrap();
    let second: i64 = t[17..19].parse().unwrap();
    let milli: i64 = t[20..23].parse().unwrap();
    (((day * 24 + hour) * 60 + minute) * 60 + second) * 1000 + milli
}

// ------------------------------------------------------------------------------------------------ opening

#[test]
fn a_decision_opens_an_hour_a_session_and_a_minutes_window_with_the_right_expiry() {
    for (kind, minutes) in [("hour", 60), ("session", 240), ("minutes", 7)] {
        let w = World::new();
        let window = w.open(kind, minutes);
        assert_eq!(window["kind"], kind);
        assert_eq!(window["minutes"], minutes);
        assert_eq!(window["operatorActive"], true);
        assert_eq!(window["source"], json!({"proposalId": "op-1"}));
        assert_eq!(
            ms(window["expiresAt"].as_str().unwrap()) - ms(window["createdAt"].as_str().unwrap()),
            minutes * 60_000
        );
        assert_eq!(window["remainingSeconds"], minutes * 60);
        assert_eq!(w.active(), window);
        let opened = w.notices("pm-1", "Approval window win-1 is open");
        assert_eq!(opened.len(), 1);
    }
}

#[test]
fn the_window_kind_and_minutes_are_checked_and_a_restart_or_a_denial_opens_none() {
    let w = World::new();
    for session in [
        json!({"kind": "window", "windowKind": "day", "maxMinutes": 5}),
        json!({"kind": "window", "maxMinutes": 5}),
    ] {
        let proposal = w.propose(json!({})).unwrap();
        assert!(error(w.decide(&proposal, Some(session))).contains("window_kind_invalid"));
    }
    for (kind, minutes) in [
        ("minutes", json!(0)),
        ("minutes", json!(481)),
        ("minutes", json!(1.5)),
        ("hour", json!(61)),
        ("session", Value::Null),
    ] {
        let proposal = w.propose(json!({})).unwrap();
        let message = error(w.decide(
            &proposal,
            Some(json!({"kind": "window", "windowKind": kind, "maxMinutes": minutes})),
        ));
        assert!(
            message.contains("window_minutes_invalid"),
            "{kind} {minutes}: {message}"
        );
    }
    let restart = w
        .core
        .propose_operator_action(
            &w.ctx(&w.f.developer_credential),
            &json!({"kind": "restart", "reason": "r", "maxPending": 20}),
        )
        .unwrap();
    let session = json!({"kind": "window", "windowKind": "minutes", "maxMinutes": 5});
    assert!(error(w.decide(&restart, Some(session.clone()))).contains("session_not_for_restart"));
    let mut deny = json!({"proposalId": restart["proposalId"], "decision": "deny", "session": session, "proposalTtlMinutes": 60});
    deny["hash"] = json!("0".repeat(12));
    let message = error(
        w.core
            .decide_operator_proposal(&w.ctx(&w.f.pm_credential), &deny),
    );
    assert!(message.contains("only with an approval"), "{message}");
    assert_eq!(w.active(), Value::Null);
}

#[test]
fn a_second_window_replaces_the_first_and_one_opened_at_the_old_expiry_ends_it_expired() {
    let w = World::new();
    let first = w.open("minutes", 30);
    let second = w.open("minutes", 10);
    assert_eq!(w.active()["windowId"], second["windowId"]);
    assert_eq!(w.window_row("win-1").0.as_deref(), Some("replaced"));
    assert_eq!(
        w.notices("pm-1", "Approval window win-1 was replaced")
            .len(),
        1
    );

    let third_clock = World::new();
    third_clock.open("minutes", 30);
    third_clock.advance_minutes(30);
    assert_eq!(third_clock.active(), Value::Null);
    third_clock.open("minutes", 5);
    assert_eq!(
        third_clock.window_row("win-1").0.as_deref(),
        Some("expired")
    );
    assert_eq!(first["windowId"], "win-1");
}

#[test]
fn the_tick_expires_a_window_at_its_cap_and_a_restart_and_a_revoke_end_it() {
    let w = World::new();
    w.open("minutes", 5);
    w.advance_minutes(4);
    assert_eq!(
        w.core.expire_approval_windows(&w.ctx(OWNER)).unwrap(),
        json!([])
    );
    assert_eq!(w.active()["remainingSeconds"], 60);
    w.advance_minutes(1);
    assert_eq!(
        w.core.expire_approval_windows(&w.ctx(OWNER)).unwrap(),
        json!(["win-1"])
    );
    assert_eq!(w.window_row("win-1").0.as_deref(), Some("expired"));
    assert_eq!(w.notices("pm-1", "reached its time cap").len(), 1);

    let w = World::new();
    w.open("hour", 60);
    w.core
        .end_approval_windows_for_restart(&w.ctx(OWNER))
        .unwrap();
    assert_eq!(w.window_row("win-1").0.as_deref(), Some("restart"));
    assert_eq!(w.active(), Value::Null);

    let w = World::new();
    w.open("hour", 60);
    let revoked = w
        .core
        .end_approval_window(&w.ctx(&w.f.pm_credential), &json!({"windowId": "win-1"}))
        .unwrap();
    assert_eq!(revoked["windowId"], "win-1");
    assert_eq!(w.window_row("win-1").0.as_deref(), Some("revoked"));
    let again = error(
        w.core
            .end_approval_window(&w.ctx(&w.f.pm_credential), &json!({"windowId": "win-1"})),
    );
    assert!(again.contains("window_already_ended"), "{again}");
    let missing = error(
        w.core
            .end_approval_window(&w.ctx(&w.f.pm_credential), &json!({"windowId": "win-9"})),
    );
    assert!(missing.contains("not_found"), "{missing}");
    let listed = w.core.list_approval_windows(None).unwrap();
    assert_eq!(listed[0]["endedReason"], "revoked");
}

// ------------------------------------------------------------------------------------- Operator proposals

#[test]
fn a_proposal_under_the_window_is_approved_with_an_info_notice_naming_the_unchecked_reason() {
    let w = World::new();
    w.open("minutes", 30);
    let used = w.propose(json!({"autoRule": "window:win-1"})).unwrap();
    assert_eq!(used["state"], "approved");
    assert_eq!(used["autoRule"], "window:win-1");
    let plain = w.notices("pm-1", "was approved by approval window win-1");
    assert_eq!(plain.len(), 1);
    assert!(!plain[0].contains("without the floor check"));
    w.propose(json!({"autoRule": "window:win-1", "windowUnchecked": {"reason": "runs a script the floor cannot read"}}))
        .unwrap();
    let noted = w.notices(
        "pm-1",
        "without the floor check: runs a script the floor cannot read",
    );
    assert_eq!(noted.len(), 1);
    let class: String = w.sql(|db| {
        db.query_row("SELECT class FROM messages WHERE body LIKE '%approved by approval window win-1%' LIMIT 1", [], |r| r.get(0)).unwrap()
    });
    assert_eq!(class, "info");
    let uses: i64 = w.sql(|db| {
        db.query_row("SELECT count(*) FROM controller_events WHERE entity_type = 'approval_window' AND payload_json LIKE '%operator.window_used%'", [], |r| r.get(0)).unwrap()
    });
    assert_eq!(uses, 2);
}

#[test]
fn a_window_rule_is_refused_after_expiry_revoke_restart_replacement_and_release() {
    let refused = |w: &World, id: &str| {
        let message = error(w.propose(json!({"autoRule": format!("window:{id}")})));
        assert!(message.contains("window_not_active"), "{message}");
    };
    let w = World::new();
    w.open("minutes", 5);
    w.advance_minutes(5);
    refused(&w, "win-1");

    let w = World::new();
    w.open("minutes", 5);
    w.core
        .end_approval_window(&w.ctx(OWNER), &json!({"windowId": "win-1"}))
        .unwrap();
    refused(&w, "win-1");

    let w = World::new();
    w.open("minutes", 5);
    w.core
        .end_approval_windows_for_restart(&w.ctx(OWNER))
        .unwrap();
    refused(&w, "win-1");

    let w = World::new();
    w.open("minutes", 5);
    w.open("minutes", 5);
    refused(&w, "win-1");
    assert_eq!(
        w.propose(json!({"autoRule": "window:win-2"})).unwrap()["state"],
        "approved"
    );

    let w = World::new();
    w.open("minutes", 5);
    w.core.end_agent(&w.ctx(OWNER), "dev-1", None).unwrap();
    assert_eq!(w.window_row("win-1"), (None, Some("released".into())));
    assert_eq!(w.active()["operatorActive"], false);
    assert_eq!(
        w.notices("pm-1", "Operator part of approval window win-1 ended")
            .len(),
        1
    );
}

#[test]
fn a_window_floor_keeps_the_proposal_proposed_and_both_options_are_refused() {
    let w = World::new();
    w.open("minutes", 30);
    let held = w
        .propose(json!({"command": "git push --force", "windowFloor": {"windowId": "win-1", "entry": "git push --force", "detail": "force push"}}))
        .unwrap();
    assert_eq!(held["state"], "proposed");
    let note = &w.notices("pm-1", "needs the user even inside approval window win-1")[0];
    assert!(note.contains("git push --force"), "{note}");
    let both = error(w.propose(json!({
        "autoRule": "window:win-1",
        "windowUnchecked": {"reason": "x"},
        "windowFloor": {"windowId": "win-1", "entry": "e", "detail": "d"},
    })));
    assert!(both.contains("cannot both be given"), "{both}");
    let lone = error(w.propose(json!({"windowUnchecked": {"reason": "x"}})));
    assert!(lone.contains("autoRule of window"), "{lone}");
}

// ------------------------------------------------------------------------------------------ prompt relay

#[test]
fn the_controller_captures_and_answers_a_non_widening_option_inside_the_window() {
    let w = World::new();
    w.open("minutes", 30);
    let relay = w.capture("dev-2", Some("win-1"), CONTROLLER).unwrap();
    let begun = w
        .begin(
            &relay,
            1,
            json!({"autoWindowId": "win-1", "uncheckedReason": "a shell it cannot parse"}),
        )
        .unwrap();
    assert_eq!(begun["state"], "typing");
    let stored: (String, String) = w.sql(|db| {
        db.query_row(
            "SELECT auto_window_id, answered_by_actor_id FROM prompt_relays WHERE relay_id = ?",
            [relay["relayId"].as_str().unwrap()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    });
    assert_eq!(stored.0, "win-1");
    let notice = &w.notices(
        "pm-1",
        "was answered with option 1 by approval window win-1",
    )[0];
    assert!(notice.contains("a shell it cannot parse"), "{notice}");
}

#[test]
fn the_controller_is_refused_a_widening_esc_text_wrong_hash_closed_window_and_pm_prompts() {
    let w = World::new();
    w.open("minutes", 30);
    let capture = |agent: &str| w.capture(agent, Some("win-1"), CONTROLLER).unwrap();
    let auto = json!({"autoWindowId": "win-1"});

    let relay = capture("dev-2");
    assert!(error(w.begin(&relay, 2, auto.clone())).contains("window_option_refused"));
    assert!(error(w.begin(&relay, 3, auto.clone())).contains("window_option_refused"));
    assert!(
        error(w.begin_answer(&relay, json!({"kind": "esc"}), auto.clone()))
            .contains("window_option_refused")
    );
    assert!(error(w.begin_answer(
        &relay,
        json!({"kind": "text", "number": 3, "text": "no"}),
        auto.clone()
    ))
    .contains("window_option_refused"));
    let mut wrong = relay.clone();
    wrong["promptSha"] = json!("0".repeat(64));
    assert!(error(w.begin(&wrong, 1, auto.clone())).contains("hash_mismatch"));
    assert!(
        error(w.begin(&relay, 1, json!({"autoWindowId": "win-2"}))).contains("window_not_active")
    );

    // The PM's own prompt and a PM actor are refused as well.
    assert!(error(w.capture("pm-1", Some("win-1"), CONTROLLER)).contains("window_agent_refused"));
    assert!(error(w.capture("dev-2", Some("win-1"), &w.f.pm_credential))
        .contains("only the controller"));
    // A controller without a window is not a PM.
    assert!(error(w.capture("dev-2", None, CONTROLLER)).contains("only an active PM"));

    // A closed window refuses the capture and the answer.
    w.core
        .end_approval_window(&w.ctx(OWNER), &json!({"windowId": "win-1"}))
        .unwrap();
    assert!(error(w.capture("dev-2", Some("win-1"), CONTROLLER)).contains("window_not_active"));
    assert!(error(w.begin(&relay, 1, auto)).contains("window_not_active"));
}

#[test]
fn a_controller_without_prompt_relay_is_refused_a_capture_with_a_capability_error() {
    let w = World::new();
    w.open("minutes", 30);
    w.sql(|db| {
        db.execute("UPDATE capability_grants SET revoked_at = '2026-01-01T00:00:00.000Z' WHERE capability = 'prompt:relay' AND actor_id IN (SELECT actor_id FROM actors WHERE is_internal = 1)", []).unwrap();
    });
    let message = error(w.capture("dev-2", Some("win-1"), CONTROLLER));
    assert!(message.contains("actor lacks prompt:relay"), "{message}");
}

#[test]
fn the_pm_flows_are_unchanged() {
    let w = World::new();
    let relay = w.capture("dev-2", None, &w.f.pm_credential).unwrap();
    let begun = w.begin(&relay, 2, json!({})).unwrap();
    assert_eq!(begun["state"], "typing");
    assert_eq!(begun["answer"]["widensPermissions"], true);
    assert!(w.active().is_null());
}

#[test]
fn open_window_for_a_non_widening_option_opens_a_relay_window_that_covers_a_later_operator_proposal(
) {
    let w = World::new();
    let relay = w.capture("dev-2", None, &w.f.pm_credential).unwrap();
    for number in [2, 3] {
        let other = json!({"windowKind": "minutes", "maxMinutes": 20});
        let message = error(w.begin(&relay, number, json!({"openWindow": other})));
        assert!(
            message.contains("window_needs_allow_once"),
            "{number}: {message}"
        );
    }
    let begun = w
        .begin(
            &relay,
            1,
            json!({"openWindow": {"windowKind": "minutes", "maxMinutes": 20}}),
        )
        .unwrap();
    assert_eq!(
        begun["window"]["source"],
        json!({"relayId": relay["relayId"]})
    );
    assert_eq!(begun["window"]["operatorActive"], true);
    let opened = &w.notices("pm-1", "Approval window win-1 is open")[0];
    assert!(
        opened.contains("also covers later Operator proposals"),
        "{opened}"
    );
    let used = w.propose(json!({"autoRule": "window:win-1"})).unwrap();
    assert_eq!(used["state"], "approved");
    // Releasing the Operator ends only its part: the window stays open.
    w.core.end_agent(&w.ctx(OWNER), "dev-1", None).unwrap();
    assert_eq!(w.window_row("win-1"), (None, Some("released".into())));
    assert_eq!(w.active()["windowId"], "win-1");
}

#[test]
fn an_answer_cannot_both_run_inside_a_window_and_open_one() {
    let w = World::new();
    w.open("minutes", 30);
    let relay = w.capture("dev-2", Some("win-1"), CONTROLLER).unwrap();
    let message = error(w.begin(
        &relay,
        1,
        json!({"autoWindowId": "win-1", "openWindow": {"windowKind": "minutes", "maxMinutes": 5}}),
    ));
    assert!(message.contains("window_args_conflict"), "{message}");
}

#[test]
fn a_floor_hit_notice_is_queued_once_per_relay() {
    let w = World::new();
    w.open("minutes", 30);
    let relay = w.capture("dev-2", Some("win-1"), CONTROLLER).unwrap();
    let input = json!({"windowId": "win-1", "relayId": relay["relayId"], "agentId": "dev-2", "action": "git push --force", "entry": "git push --force", "detail": "force push"});
    let call = || {
        w.core
            .queue_floor_hit_notice(&w.ctx(CONTROLLER), &input)
            .unwrap()
    };
    assert_eq!(call(), json!({"queued": true}));
    assert_eq!(call(), json!({"queued": false}));
    assert_eq!(
        w.notices("pm-1", "needs the user even inside approval window win-1")
            .len(),
        1
    );
}

// ----------------------------------------------------------------------------------------------- triggers

fn raw(w: &World) -> rusqlite::Connection {
    let db = rusqlite::Connection::open(w.f.database()).unwrap();
    db.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    db
}

fn insert_window(
    db: &rusqlite::Connection,
    n: i64,
    proposal: Option<&str>,
    relay: Option<&str>,
) -> rusqlite::Result<usize> {
    db.execute(
        "INSERT INTO approval_windows(project_id, window_id, sequence, kind, minutes, created_by, source_proposal_id, source_relay_id, created_at, expires_at)
         VALUES ('prustonly', ?, ?, 'minutes', 5, (SELECT actor_id FROM actors LIMIT 1), ?, ?, '2026-01-01T00:00:00.000Z', '2026-01-01T00:05:00.000Z')",
        rusqlite::params![format!("win-{n}"), n, proposal, relay],
    )
}

#[test]
fn the_window_table_allows_one_open_window_and_exactly_one_source() {
    let w = World::new();
    w.propose(json!({})).unwrap();
    let db = raw(&w);
    insert_window(&db, 1, Some("op-1"), None).unwrap();
    assert!(
        insert_window(&db, 2, Some("op-1"), None).is_err(),
        "a second open window"
    );
    db.execute("UPDATE approval_windows SET ended_at = '2026-01-01T00:01:00.000Z', ended_reason = 'revoked'", []).unwrap();
    assert!(
        insert_window(&db, 3, Some("op-1"), Some("relay-1")).is_err(),
        "both sources"
    );
    assert!(insert_window(&db, 3, None, None).is_err(), "no source");
    insert_window(&db, 3, Some("op-1"), None).unwrap();
    assert!(db
        .execute(
            "UPDATE approval_windows SET minutes = 9 WHERE window_id = 'win-3'",
            []
        )
        .is_err());
    assert!(db.execute("UPDATE approval_windows SET ended_at = 'x', ended_reason = 'expired' WHERE window_id = 'win-1'", []).is_err());
    assert!(db.execute("DELETE FROM approval_windows", []).is_err());
}

/// A relay row written by hand as `captured_by` for `agent`, under `window`, captured at `at`.
fn insert_relay(
    db: &rusqlite::Connection,
    n: i64,
    agent: &str,
    actor_sql: &str,
    window: Option<&str>,
    at: &str,
) -> rusqlite::Result<usize> {
    db.execute(
        &format!(
            "INSERT INTO prompt_relays(project_id, relay_id, sequence, agent_id, pane_id, host_kind, prompt_text, options_json, prompt_sha, captured_by_actor_id, captured_at, expires_at, state, auto_window_id)
             VALUES ('prustonly', ?, ?, ?, 'p', 'herdr', 'Allow?', ?, ?, {actor_sql}, ?, '2099-01-01T00:00:00.000Z', 'captured', ?)"
        ),
        rusqlite::params![format!("relay-{n}"), n, agent, OPTIONS, sha256(&format!("{n}")), at, window],
    )
}

const CONTROLLER_SQL: &str = "(SELECT actor_id FROM actors WHERE is_internal = 1)";

#[test]
fn the_relay_triggers_hold_the_controller_to_an_open_window_and_a_non_pm_agent() {
    let w = World::new();
    w.propose(json!({})).unwrap();
    let db = raw(&w);
    insert_window(&db, 1, Some("op-1"), None).unwrap();
    let at = "2026-01-01T00:01:00.000Z";
    assert!(
        insert_relay(&db, 1, "dev-2", CONTROLLER_SQL, None, at).is_err(),
        "no window"
    );
    assert!(
        insert_relay(&db, 1, "pm-1", CONTROLLER_SQL, Some("win-1"), at).is_err(),
        "a PM agent"
    );
    assert!(
        insert_relay(&db, 1, "dev-2", CONTROLLER_SQL, Some("win-9"), at).is_err(),
        "unknown window"
    );
    insert_relay(&db, 1, "dev-2", CONTROLLER_SQL, Some("win-1"), at).unwrap();
    let pm_sql = format!("'{}'", w.f.pm_actor);
    assert!(
        insert_relay(&db, 2, "dev-1", &pm_sql, Some("win-1"), at).is_err(),
        "a PM capture carries no window"
    );
    insert_relay(&db, 2, "dev-1", &pm_sql, None, at).unwrap();

    let answer = |option: Option<i64>, kind: &str, begin: &str, relay: &str| {
        db.execute(
            "UPDATE prompt_relays SET state = 'typing', answer_kind = ?, answer_option = ?, answer_widens_permissions = 0,
               answered_by_actor_id = (SELECT actor_id FROM actors WHERE is_internal = 1), auto_begin_at = ? WHERE relay_id = ?",
            rusqlite::params![kind, option, begin, relay],
        )
    };
    assert!(
        answer(Some(2), "option", at, "relay-1").is_err(),
        "widening"
    );
    assert!(
        answer(Some(3), "option", at, "relay-1").is_err(),
        "accepts text"
    );
    assert!(answer(None, "esc", at, "relay-1").is_err(), "esc");
    assert!(answer(Some(1), "text", at, "relay-1").is_err(), "text");
    assert!(
        answer(Some(9), "option", at, "relay-1").is_err(),
        "no such option"
    );
    assert!(
        answer(Some(1), "option", "2026-01-01T00:05:00.000Z", "relay-1").is_err(),
        "at the expiry"
    );
    answer(Some(1), "option", "2026-01-01T00:04:59.999Z", "relay-1").unwrap();
    assert!(
        answer(Some(1), "option", at, "relay-2").is_err(),
        "the relay has no window"
    );
}

#[test]
fn a_capture_at_the_expiry_is_refused_and_one_millisecond_before_is_accepted() {
    let w = World::new();
    w.propose(json!({})).unwrap();
    let db = raw(&w);
    insert_window(&db, 1, Some("op-1"), None).unwrap();
    assert!(insert_relay(
        &db,
        1,
        "dev-2",
        CONTROLLER_SQL,
        Some("win-1"),
        "2026-01-01T00:05:00.000Z"
    )
    .is_err());
    insert_relay(
        &db,
        1,
        "dev-2",
        CONTROLLER_SQL,
        Some("win-1"),
        "2026-01-01T00:04:59.999Z",
    )
    .unwrap();
}

// -------------------------------------------------------------------------------------------------- texts

#[test]
fn the_window_notice_texts_are_the_fixture() {
    let fixture: Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../testdata/fixtures/kernel-text/approval_windows.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let strings = |value: &Value| -> Vec<String> {
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect()
    };
    for case in fixture["windowOpenedNotice"].as_array().unwrap() {
        let floor = case["floor"].as_array().map(|_| strings(&case["floor"]));
        assert_eq!(
            window_opened_notice(
                &case["window"],
                case["source"].as_str().unwrap(),
                floor.as_deref(),
                case["toolPrompts"].as_bool().unwrap(),
                case["relaySourced"].as_bool().unwrap()
            ),
            case["result"].as_str().unwrap()
        );
    }
    for case in fixture["windowEndedNotice"].as_array().unwrap() {
        assert_eq!(
            window_ended_notice(
                case["args"][0].as_str().unwrap(),
                case["args"][1].as_str().unwrap()
            ),
            case["result"].as_str().unwrap()
        );
    }
    for case in fixture["windowOperatorEndedNotice"].as_array().unwrap() {
        assert_eq!(
            window_operator_ended_notice(case["args"][0].as_str().unwrap()),
            case["result"].as_str().unwrap()
        );
    }
    for case in fixture["windowUsedNotice"].as_array().unwrap() {
        let a = &case["args"];
        assert_eq!(
            window_used_notice(
                a[0].as_str().unwrap(),
                a[1].as_str().unwrap(),
                a[2].as_str()
            ),
            case["result"].as_str().unwrap()
        );
    }
    for case in fixture["windowAnsweredNotice"].as_array().unwrap() {
        let a = &case["args"];
        assert_eq!(
            window_answered_notice(
                a[0].as_str().unwrap(),
                a[1].as_str().unwrap(),
                a[2].as_i64().unwrap(),
                a[3].as_str().unwrap(),
                a[4].as_str()
            ),
            case["result"].as_str().unwrap()
        );
    }
    for case in fixture["windowFloorProposalNote"].as_array().unwrap() {
        let a = &case["args"];
        assert_eq!(
            window_floor_proposal_note(
                a[0].as_str().unwrap(),
                a[1].as_str().unwrap(),
                a[2].as_str().unwrap()
            ),
            case["result"].as_str().unwrap()
        );
    }
    for case in fixture["windowFloorHitNotice"].as_array().unwrap() {
        let a = &case["args"];
        let s = |i: usize| a[i].as_str().unwrap();
        assert_eq!(
            window_floor_hit_notice(s(0), s(1), s(2), s(3), s(4), s(5)),
            case["result"].as_str().unwrap()
        );
    }
}

#[test]
fn a_fresh_ledger_gives_the_controller_prompt_relay() {
    let f = Fixture::new();
    let db = rusqlite::Connection::open(f.database()).unwrap();
    let granted: i64 = db
        .query_row(
            "SELECT count(*) FROM capability_grants WHERE capability = 'prompt:relay' AND revoked_at IS NULL AND actor_id IN (SELECT actor_id FROM actors WHERE role = 'controller')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(granted, 1);
}

#[test]
fn a_restart_is_approved_by_a_window_rule_and_by_no_other_rule_but_full_auto() {
    let w = World::new();
    w.open("minutes", 30);
    let approved = w
        .propose(json!({"kind": "restart", "autoRule": "window:win-1"}))
        .unwrap();
    assert_eq!(approved["state"], "approved");
    assert_eq!(approved["autoRule"], "window:win-1");
    for rule in ["ls", "session:grant-1", "exact"] {
        let message = error(w.propose(json!({"kind": "restart", "autoRule": rule})));
        assert!(
            message.contains("can be auto-approved"),
            "{rule}: {message}"
        );
    }
    w.core
        .end_approval_window(&w.ctx(OWNER), &json!({"windowId": "win-1"}))
        .unwrap();
    let message = error(w.propose(json!({"kind": "restart", "autoRule": "window:win-1"})));
    assert!(message.contains("window_not_active"), "{message}");
}

#[test]
fn the_due_read_matches_the_expiry_and_the_expiry_keeps_its_replay_and_refusals() {
    let w = World::new();
    assert_eq!(w.core.approval_window_due().unwrap(), json!(false));
    w.open("minutes", 5);
    w.advance_minutes(4);
    assert_eq!(w.core.approval_window_due().unwrap(), json!(false));
    w.advance_minutes(1);
    assert_eq!(w.core.approval_window_due().unwrap(), json!(true));

    let context = w.ctx(OWNER);
    assert_eq!(
        w.core.expire_approval_windows(&context).unwrap(),
        json!(["win-1"])
    );
    assert_eq!(w.core.approval_window_due().unwrap(), json!(false));
    // The identical request again replays the original result, though nothing is due any more.
    assert_eq!(
        w.core.expire_approval_windows(&context).unwrap(),
        json!(["win-1"])
    );
    // The same key on another operation is a conflict, and a stranger's credential is refused.
    assert!(w.core.end_approval_windows_for_restart(&context).is_err());
    let stranger = w.ctx("not-a-known-credential-0123456789-0123456789");
    assert!(w.core.expire_approval_windows(&stranger).is_err());
}
