//! The timed grant kind of migration 0046: its decision, its fixed text and expiry, its refusals, its use by a proposal
//! and every way it ends.

use crate::fixture::{context, Fixture, OWNER};
use serde_json::{json, Value};

fn propose(f: &Fixture, command: &str, auto_rule: Option<&str>) -> Value {
    let core = f.core();
    let mut input =
        json!({"kind": "command", "command": command, "reason": "look", "maxPending": 10});
    if let Some(rule) = auto_rule {
        input["autoRule"] = json!(rule);
    }
    core.propose_operator_action(&context(core, &f.developer_credential), &input)
        .unwrap()
}

fn decide(
    f: &Fixture,
    proposal: &Value,
    decision: &str,
    session: Option<Value>,
) -> Result<Value, capstan_kernel::KernelError> {
    let core = f.core();
    let mut input = json!({
        "proposalId": proposal["proposalId"],
        "decision": decision,
        "hash": proposal["commandSha"].as_str().unwrap()[..12],
        "proposalTtlMinutes": 60,
    });
    if let Some(session) = session {
        input["session"] = session;
    }
    core.decide_operator_proposal(&context(core, &f.pm_credential), &input)
}

fn timed(f: &Fixture, minutes: Value) -> Result<Value, capstan_kernel::KernelError> {
    let proposal = propose(f, "ls -la", None);
    decide(
        f,
        &proposal,
        "approve",
        Some(json!({"kind": "timed", "maxMinutes": minutes})),
    )
}

fn grant_of(decided: &Value) -> Value {
    decided["sessionGrant"].clone()
}

fn error(result: Result<Value, capstan_kernel::KernelError>) -> String {
    result.expect_err("refused").to_string()
}

fn read<T>(f: &Fixture, read: impl FnOnce(&rusqlite::Connection) -> T) -> T {
    read(&rusqlite::Connection::open(f.database()).unwrap())
}

fn used_events(f: &Fixture) -> i64 {
    read(f, |db| {
        db.query_row(
            "SELECT count(*) FROM controller_events WHERE entity_type = 'operator_grant' AND payload_json LIKE '%operator.grant_used%'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    })
}

fn notices(f: &Fixture, needle: &str) -> Vec<String> {
    read(f, |db| {
        let mut statement = db
            .prepare("SELECT body FROM messages WHERE recipient_agent_id = 'dev-1' AND body LIKE ? ORDER BY sequence")
            .unwrap();
        let rows = statement
            .query_map([format!("%{needle}%")], |r| r.get::<_, String>(0))
            .unwrap();
        rows.map(Result::unwrap).collect()
    })
}

#[test]
fn approving_with_a_timed_session_creates_a_timed_grant() {
    let f = Fixture::new();
    let decided = timed(&f, json!(30)).unwrap();
    let grant = grant_of(&decided);
    assert_eq!(grant["kind"], "timed");
    assert_eq!(grant["text"], "non-destructive commands");
    let created = grant["createdAt"].as_str().unwrap();
    let expires = grant["expiresAt"].as_str().unwrap();
    let ms = |t: &str| chrono_free_ms(t);
    assert_eq!(ms(expires) - ms(created), 30 * 60_000);
    let listed = f.core().list_operator_grants(None).unwrap();
    assert_eq!(listed[0]["kind"], "timed");
    assert_eq!(
        f.core()
            .active_operator_grants()
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let notice = &notices(&f, "A session grant")[0];
    assert!(notice.contains("a timed grant that auto-approves commands the controller classifies as non-destructive"), "{notice}");
}

/// Milliseconds of an ISO time `YYYY-MM-DDTHH:MM:SS.mmmZ` within the same day range (enough for a difference).
fn chrono_free_ms(t: &str) -> i64 {
    let day: i64 = t[8..10].parse().unwrap();
    let hour: i64 = t[11..13].parse().unwrap();
    let minute: i64 = t[14..16].parse().unwrap();
    let second: i64 = t[17..19].parse().unwrap();
    let milli: i64 = t[20..23].parse().unwrap();
    (((day * 24 + hour) * 60 + minute) * 60 + second) * 1000 + milli
}

#[test]
fn the_minutes_are_a_whole_number_from_one_to_the_hard_cap() {
    let f = Fixture::new();
    for minutes in [
        json!(0),
        json!(481),
        json!(1.5),
        json!(-3),
        Value::Null,
        json!("30"),
    ] {
        let message = error(timed(&f, minutes.clone()));
        assert!(
            message.contains("session_minutes_invalid"),
            "{minutes}: {message}"
        );
    }
    timed(&f, json!(1)).unwrap();
    timed(&f, json!(480)).unwrap();
}

#[test]
fn an_unknown_session_kind_is_refused_and_never_a_prefix() {
    let f = Fixture::new();
    for session in [
        json!({"kind": "other", "text": "ls", "maxMinutes": 5}),
        json!({"kind": "Prefix", "text": "ls", "maxMinutes": 5}),
        json!({"text": "ls", "maxMinutes": 5}),
    ] {
        let proposal = propose(&f, "ls -la", None);
        let message = error(decide(&f, &proposal, "approve", Some(session)));
        assert!(message.contains("session_kind_invalid"), "{message}");
    }
    assert!(f
        .core()
        .list_operator_grants(None)
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn a_timed_session_is_refused_on_a_restart_and_on_a_denial() {
    let f = Fixture::new();
    let core = f.core();
    let restart = core
        .propose_operator_action(
            &context(core, &f.developer_credential),
            &json!({"kind": "restart", "reason": "r", "maxPending": 10}),
        )
        .unwrap();
    let message = error(decide(
        &f,
        &restart,
        "approve",
        Some(json!({"kind": "timed", "maxMinutes": 5})),
    ));
    assert!(message.contains("session_not_for_restart"), "{message}");
    let proposal = propose(&f, "ls", None);
    let message = error(decide(
        &f,
        &proposal,
        "deny",
        Some(json!({"kind": "timed", "maxMinutes": 5})),
    ));
    assert!(message.contains("only with an approval"), "{message}");
}

fn use_grant(f: &Fixture, grant: &Value) -> Value {
    let rule = format!("session:{}", grant["grantId"].as_str().unwrap());
    propose(f, "ls /tmp", Some(&rule))
}

fn setup() -> (Fixture, Value) {
    let f = Fixture::new();
    let grant = grant_of(&timed(&f, json!(10)).unwrap());
    (f, grant)
}

#[test]
fn a_proposal_under_the_timed_grant_is_approved_and_recorded_as_a_use() {
    let (f, grant) = setup();
    let used = use_grant(&f, &grant);
    assert_eq!(used["state"], "approved");
    assert_eq!(used_events(&f), 1);
    let pm_notices: i64 = read(&f, |db| {
        db.query_row("SELECT count(*) FROM messages WHERE recipient_agent_id = 'pm-1' AND body LIKE '%Operator proposal op-2%'", [], |r| r.get(0)).unwrap()
    });
    assert!(pm_notices >= 1);
}

fn assert_ended(f: &Fixture, grant: &Value) {
    assert!(f
        .core()
        .active_operator_grants()
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
    let after = use_grant(f, grant);
    assert_eq!(after["state"], "proposed");
}

#[test]
fn the_timed_grant_ends_by_expiry_revocation_restart_and_release() {
    let (f, grant) = setup();
    let id = grant["grantId"].as_str().unwrap();
    let core = f.core();
    core.revoke_operator_grant(&context(core, OWNER), id)
        .unwrap();
    assert_ended(&f, &grant);
    let notice = &notices(&f, "was revoked")[0];
    assert!(
        notice.contains("a timed grant that auto-approves"),
        "{notice}"
    );

    let (f, grant) = setup();
    let core = f.core();
    core.end_operator_grants_for_restart(&context(core, OWNER))
        .unwrap();
    assert_ended(&f, &grant);

    let (f, grant) = setup();
    let core = f.core();
    core.end_agent(&context(core, OWNER), "dev-1", None)
        .unwrap();
    assert!(f
        .core()
        .active_operator_grants()
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        f.core().list_operator_grants(None).unwrap()[0]["endedReason"],
        "released"
    );
    assert_eq!(grant["kind"], "timed");
}

#[test]
fn the_timed_grant_expires_at_its_cap() {
    let mut f = Fixture::new();
    let grant = grant_of(&timed(&f, json!(10)).unwrap());
    f.close();
    let db = rusqlite::Connection::open(f.database()).unwrap();
    // The grant is immutable, so the cap passes by moving the clock of the rows: create it already past.
    db.execute_batch("DROP TRIGGER operator_grants_identity_is_immutable")
        .unwrap();
    db.execute(
        "UPDATE operator_grants SET expires_at = '2000-01-01T00:00:00.000Z'",
        [],
    )
    .unwrap();
    drop(db);
    f.reopen();
    let core = f.core();
    let expired = core
        .expire_operator_grants(&context(core, OWNER), None)
        .unwrap();
    assert_eq!(expired, json!(1));
    assert_ended(&f, &grant);
    let notice = &notices(&f, "reached its time cap")[0];
    assert!(
        notice.contains("a timed grant that auto-approves"),
        "{notice}"
    );
}
