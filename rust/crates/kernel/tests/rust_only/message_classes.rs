//! Message classes: the notice table, `enqueue_message` resolution, the wake policy of `evaluate_messaging`, `deliver_inbox`,
//! `wake_pending`, `ack_messages` / `ack_all_read`, and the schema-39 fixture generator.

use crate::fixture::{context, step, Fixture, OWNER};
use capstan_kernel::areas::messaging::{
    evaluate_messaging, notice_kind, AgentFacts, MessageClass, MessageFacts, MessagePolicy,
    MessagingTimers, StateObservation, NOTICE_KINDS,
};
use serde_json::{json, Value};

fn enqueue(f: &Fixture, recipient: &str, body: &str, extra: Value) -> Result<String, String> {
    let mut input = json!({"recipientAgentId": recipient, "body": body});
    for (key, value) in extra.as_object().unwrap() {
        input[key] = value.clone();
    }
    f.core()
        .enqueue_message(&context(f.core(), OWNER), &input)
        .map(|result| result["messageId"].as_str().unwrap().to_string())
        .map_err(|e| e.message())
}

fn class_of(f: &Fixture, id: &str) -> String {
    f.core().message(id).unwrap()["class"]
        .as_str()
        .unwrap()
        .to_string()
}

fn set_state(f: &Fixture, id: &str, state: &str) {
    let db = rusqlite::Connection::open(f.database()).unwrap();
    db.execute(
        "UPDATE messages SET state = ? WHERE message_id = ?",
        rusqlite::params![state, id],
    )
    .unwrap();
}

fn state_of(f: &Fixture, id: &str) -> String {
    f.core().message(id).unwrap()["state"]
        .as_str()
        .unwrap()
        .to_string()
}

fn ack_context(f: &Fixture, credential: &str) -> capstan_kernel::types::MutationContext {
    context(f.core(), credential)
}

#[test]
fn the_notice_table_names_every_kind_once_and_a_non_overridable_kind_is_never_info_by_policy() {
    let mut names: Vec<&str> = NOTICE_KINDS.iter().map(|(name, _, _)| *name).collect();
    let total = names.len();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), total, "a kind is listed twice");
    for kind in [
        "finding_resolved",
        "finding_cancelled",
        "cleanup_done",
        "verify_finished",
        "restart_result",
    ] {
        assert_eq!(notice_kind(kind).unwrap().1, MessageClass::Info, "{kind}");
    }
    for kind in [
        "finding_raised",
        "finding_escalated",
        "report_verified",
        "review_verdict",
        "plan_notice",
        "delivery_problem",
        "agent_stalled",
        "agent_blocked",
        "agent_lost",
    ] {
        assert_eq!(notice_kind(kind).unwrap().1, MessageClass::Action, "{kind}");
    }
    let policy = MessagePolicy::from_value(&json!({"noticeClass": {
        "delivery_problem": "info", "finding_resolved": "action", "report_verified": "info"}}));
    assert_eq!(
        policy.class_of("delivery_problem"),
        Some(MessageClass::Action)
    );
    assert_eq!(
        policy.class_of("finding_resolved"),
        Some(MessageClass::Action)
    );
    assert_eq!(
        MessagePolicy::default().class_of("finding_resolved"),
        Some(MessageClass::Info)
    );
    assert_eq!(policy.class_of("no_such_kind"), None);
    assert_eq!(MessageClass::parse("info").unwrap().as_str(), "info");
    assert_eq!(MessageClass::parse("urgent"), None);
}

#[test]
fn enqueue_message_resolves_the_class_from_action_needed_class_and_kind() {
    let f = Fixture::new();
    let by_kind = enqueue(&f, "pm-1", "a", json!({"kind": "finding_resolved"})).unwrap();
    assert_eq!(class_of(&f, &by_kind), "info");
    let delivery = enqueue(&f, "pm-1", "b", json!({"kind": "delivery_problem"})).unwrap();
    assert_eq!(class_of(&f, &delivery), "action");
    let plain = enqueue(&f, "pm-1", "c", json!({})).unwrap();
    assert_eq!(class_of(&f, &plain), "action");
    let explicit = enqueue(&f, "pm-1", "d", json!({"class": "info"})).unwrap();
    assert_eq!(class_of(&f, &explicit), "info");
    let forced = enqueue(
        &f,
        "pm-1",
        "e",
        json!({"class": "info", "actionNeeded": true}),
    )
    .unwrap();
    assert_eq!(class_of(&f, &forced), "action");
    assert!(f.core().message(&forced).unwrap()["actionNeeded"]
        .as_bool()
        .unwrap());
    let explicit_wins = enqueue(
        &f,
        "pm-1",
        "g",
        json!({"class": "action", "kind": "finding_resolved"}),
    )
    .unwrap();
    assert_eq!(class_of(&f, &explicit_wins), "action");
    let error = enqueue(&f, "pm-1", "f", json!({"kind": "no_such_kind"})).unwrap_err();
    assert!(error.contains("unknown notice kind"), "{error}");
    assert!(enqueue(&f, "pm-1", "h", json!({"class": "loud"})).is_err());
}

#[test]
fn the_policy_overrides_an_overridable_kind() {
    let f = Fixture::new();
    f.core().set_message_policy(
        &json!({"noticeClass": {"report_verified": "info", "delivery_problem": "info"}}),
    );
    let report = enqueue(&f, "pm-1", "a", json!({"kind": "report_verified"})).unwrap();
    let delivery = enqueue(&f, "pm-1", "b", json!({"kind": "delivery_problem"})).unwrap();
    // `report_verified` is not overridable either; only kinds the table marks overridable follow the policy.
    assert_eq!(class_of(&f, &report), "action");
    assert_eq!(class_of(&f, &delivery), "action");
    f.core()
        .set_message_policy(&json!({"noticeClass": {"finding_resolved": "action"}}));
    let resolved = enqueue(&f, "pm-1", "c", json!({"kind": "finding_resolved"})).unwrap();
    assert_eq!(class_of(&f, &resolved), "action");
}

fn pm_facts(state: &str) -> AgentFacts {
    AgentFacts {
        agent_id: "pm".into(),
        kind: "PM".into(),
        last_activity_ms: 0,
        observations: vec![StateObservation {
            state: state.into(),
            at_ms: 0,
        }],
        waits: vec![],
        paused: false,
    }
}

fn queued(id: &str, sequence: i64, class: MessageClass, queued_ms: i64) -> MessageFacts {
    MessageFacts {
        message_id: id.into(),
        recipient_agent_id: "pm".into(),
        state: "queued".into(),
        class,
        sequence,
        queued_ms,
        sent_ms: None,
        deferred_ms: None,
        deferred_reason: None,
        input_clear_recorded: false,
        last_notified_ms: None,
        wake_count: 0,
        last_wake_ms: None,
    }
}

fn timers(policy: Value) -> MessagingTimers {
    let mut value = json!({
        "maxDeferralSeconds": 60, "maxBusyDeferralSeconds": 600, "pmAckTimeoutSeconds": 600,
        "pmNotifyAfterSeconds": 100, "notifyIntervalSeconds": 100, "stallAfterSeconds": 100000,
        "workerAckTimeoutSeconds": 600, "pmWakeAfterSeconds": 10, "pmWakeIntervalSeconds": 10,
    });
    for (key, v) in policy.as_object().unwrap() {
        value[key] = v.clone();
    }
    MessagingTimers::from_value(&value)
}

fn kinds(actions: &[Value]) -> Vec<(String, String)> {
    actions
        .iter()
        .map(|a| {
            (
                a["kind"].as_str().unwrap().into(),
                a["messageId"].as_str().unwrap().into(),
            )
        })
        .collect()
}

#[test]
fn an_idle_pm_with_only_queued_info_wakes_by_the_policy() {
    let now = 1_000_000;
    let info = [queued("i1", 1, MessageClass::Info, 0)];
    let wake = |policy: Value, now: i64| {
        kinds(&evaluate_messaging(&[pm_facts("idle")], &info, now, &timers(policy)).actions)
            .into_iter()
            .filter(|(kind, _)| kind == "wake_pm")
            .count()
    };
    // Absent fields: the legacy behaviour wakes for every message.
    assert_eq!(wake(json!({}), now), 1);
    assert_eq!(wake(json!({"pmWakeClass": "all"}), now), 1);
    // Under 'action' info does not wake while it is young ...
    assert_eq!(wake(json!({"pmWakeClass": "action"}), now), 0);
    assert_eq!(
        wake(
            json!({"pmWakeClass": "action", "infoWakeAfterSeconds": 600}),
            500_000
        ),
        0
    );
    // ... and wakes once it is older than infoWakeAfterSeconds.
    assert_eq!(
        wake(
            json!({"pmWakeClass": "action", "infoWakeAfterSeconds": 600}),
            700_000
        ),
        1
    );
    // infoWakeAfterSeconds = 0 means never.
    assert_eq!(
        wake(
            json!({"pmWakeClass": "action", "infoWakeAfterSeconds": 0}),
            10_000_000
        ),
        0
    );
}

#[test]
fn queued_action_wakes_the_pm_and_the_wake_target_skips_info() {
    let messages = [
        queued("i1", 1, MessageClass::Info, 0),
        queued("a2", 2, MessageClass::Action, 0),
    ];
    let result = evaluate_messaging(
        &[pm_facts("idle")],
        &messages,
        1_000_000,
        &timers(json!({"pmWakeClass": "action"})),
    );
    assert!(
        kinds(&result.actions).contains(&("wake_pm".into(), "a2".into())),
        "{:?}",
        result.actions
    );
}

#[test]
fn an_info_only_queue_does_not_notify_the_operator() {
    let info = [queued("i1", 1, MessageClass::Info, 0)];
    let result = evaluate_messaging(&[pm_facts("idle")], &info, 1_000_000, &timers(json!({})));
    assert!(
        !kinds(&result.actions)
            .iter()
            .any(|(kind, _)| kind == "notify_operator"),
        "{:?}",
        result.actions
    );
    let action = [queued("a1", 1, MessageClass::Action, 0)];
    let result = evaluate_messaging(&[pm_facts("idle")], &action, 1_000_000, &timers(json!({})));
    assert!(kinds(&result.actions).contains(&("notify_operator".into(), "a1".into())));
}

#[test]
fn deliver_inbox_with_digest_acks_info_and_keeps_action_sent() {
    let f = Fixture::new();
    let a1 = enqueue(&f, "pm-1", "do this", json!({"actionNeeded": true})).unwrap();
    let i1 = enqueue(&f, "pm-1", "fyi", json!({"class": "info"})).unwrap();
    let a2 = enqueue(&f, "pm-1", "and this", json!({})).unwrap();
    let result = f.core().deliver_inbox(&f.pm_credential, true).unwrap();
    assert_eq!(result["pulled"], true);
    let ids = |key: &str| -> Vec<String> {
        result[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["messageId"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(ids("messages"), vec![a1.clone(), a2.clone()]);
    assert_eq!(ids("info"), vec![i1.clone()]);
    assert_eq!(result["info"][0]["state"], "acked");
    assert_eq!(state_of(&f, &a1), "sent");
    assert_eq!(state_of(&f, &a2), "sent");
    assert_eq!(state_of(&f, &i1), "acked");
    let db = rusqlite::Connection::open(f.database()).unwrap();
    let payload: String = db
        .query_row(
            "SELECT payload_json FROM controller_events WHERE entity_id = ? AND to_state = 'acked'",
            [&i1],
            |r| r.get(0),
        )
        .unwrap();
    let payload: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(payload["details"]["auto"], "digest", "{payload}");
    // Nothing pending now: a second call pulls nothing and lists the unacked actions.
    let again = f.core().deliver_inbox(&f.pm_credential, true).unwrap();
    assert_eq!(again["pulled"], false);
    assert_eq!(again["messages"].as_array().unwrap().len(), 2);
    assert!(again["info"].as_array().unwrap().is_empty());
}

#[test]
fn deliver_inbox_without_digest_leaves_info_sent_in_messages() {
    let f = Fixture::new();
    let a1 = enqueue(&f, "pm-1", "do this", json!({"actionNeeded": true})).unwrap();
    let i1 = enqueue(&f, "pm-1", "fyi", json!({"class": "info"})).unwrap();
    let result = f.core().deliver_inbox(&f.pm_credential, false).unwrap();
    let ids: Vec<&str> = result["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["messageId"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![a1.as_str(), i1.as_str()]);
    assert!(result["info"].as_array().unwrap().is_empty());
    assert_eq!(state_of(&f, &i1), "sent");
}

#[test]
fn wake_pending_is_read_only_and_follows_the_policy() {
    let f = Fixture::new();
    enqueue(&f, "pm-1", "fyi", json!({"class": "info"})).unwrap();
    let before = f.core().state_version().unwrap();
    f.core()
        .set_message_policy(&json!({"pmWakeClass": "action"}));
    let quiet = f.core().wake_pending(&f.pm_credential).unwrap();
    assert_eq!(quiet, json!({"wake": false, "count": 0, "infoCount": 1}));
    f.core().set_message_policy(&json!({}));
    let legacy = f.core().wake_pending(&f.pm_credential).unwrap();
    assert_eq!(legacy, json!({"wake": true, "count": 1, "infoCount": 1}));
    f.core()
        .set_message_policy(&json!({"pmWakeClass": "action"}));
    enqueue(&f, "pm-1", "do", json!({"actionNeeded": true})).unwrap();
    let loud = f.core().wake_pending(&f.pm_credential).unwrap();
    assert_eq!(loud, json!({"wake": true, "count": 1, "infoCount": 1}));
    let summary = f.core().unread_summary(&f.pm_credential).unwrap();
    assert_eq!(summary["wakeCount"], 1);
    assert_eq!(summary["infoCount"], 1);
    assert_eq!(summary["count"], 2);
    // Nothing above wrote.
    assert_eq!(
        f.core().state_version().unwrap(),
        before + 1,
        "only the second enqueue wrote"
    );
    let stable = f.core().state_version().unwrap();
    f.core().wake_pending(&f.pm_credential).unwrap();
    f.core().unread_summary(&f.pm_credential).unwrap();
    assert_eq!(f.core().state_version().unwrap(), stable);
}

#[test]
fn ack_all_read_acks_only_sent_and_unacked_rows_of_the_caller() {
    let f = Fixture::new();
    let queued = enqueue(&f, "pm-1", "queued", json!({})).unwrap();
    let deferred = enqueue(&f, "pm-1", "deferred", json!({})).unwrap();
    let sent = enqueue(&f, "pm-1", "sent", json!({})).unwrap();
    let unacked = enqueue(&f, "pm-1", "unacked", json!({})).unwrap();
    let other = enqueue(&f, "dev-1", "theirs", json!({})).unwrap();
    set_state(&f, &deferred, "deferred");
    set_state(&f, &sent, "sent");
    set_state(&f, &unacked, "unacked");
    set_state(&f, &other, "sent");
    let acked = f
        .core()
        .ack_all_read(&ack_context(&f, &f.pm_credential))
        .unwrap();
    assert_eq!(acked, json!([sent.clone(), unacked.clone()]));
    assert_eq!(state_of(&f, &sent), "acked");
    assert_eq!(state_of(&f, &unacked), "acked_late");
    assert_eq!(state_of(&f, &queued), "queued");
    assert_eq!(state_of(&f, &deferred), "deferred");
    assert_eq!(state_of(&f, &other), "sent");
}

#[test]
fn ack_messages_validates_every_id_and_refuses_all_on_one_bad_id() {
    let f = Fixture::new();
    let a = enqueue(&f, "pm-1", "a", json!({})).unwrap();
    let b = enqueue(&f, "pm-1", "b", json!({})).unwrap();
    let theirs = enqueue(&f, "dev-1", "c", json!({})).unwrap();
    for id in [&a, &b, &theirs] {
        set_state(&f, id, "sent");
    }
    for bad in [theirs.clone(), "no-such-message".to_string()] {
        let error = f
            .core()
            .ack_messages(&ack_context(&f, &f.pm_credential), &[a.clone(), bad])
            .unwrap_err();
        assert!(
            matches!(error, capstan_kernel::KernelError::MessageTransition { .. }),
            "{error}"
        );
        assert_eq!(state_of(&f, &a), "sent", "nothing was acked");
    }
    assert_eq!(state_of(&f, &theirs), "sent");
    let results = f
        .core()
        .ack_messages(&ack_context(&f, &f.pm_credential), &[a.clone(), b.clone()])
        .unwrap();
    assert_eq!(results.as_array().unwrap().len(), 2);
    assert_eq!(state_of(&f, &a), "acked");
    assert_eq!(state_of(&f, &b), "acked");
    let db = rusqlite::Connection::open(f.database()).unwrap();
    for id in [&a, &b] {
        let events: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM controller_events WHERE entity_id = ? AND to_state = 'acked'",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(events, 1);
    }
}

#[test]
fn ack_messages_with_duplicate_or_unknown_ids_never_acks_partially() {
    let f = Fixture::new();
    let a = enqueue(&f, "pm-1", "a", json!({})).unwrap();
    let b = enqueue(&f, "pm-1", "b", json!({})).unwrap();
    set_state(&f, &a, "sent");
    set_state(&f, &b, "sent");
    let mixed = f.core().ack_messages(
        &ack_context(&f, &f.pm_credential),
        &[a.clone(), "no-such-message".into()],
    );
    assert!(mixed.is_err());
    assert_eq!(state_of(&f, &a), "sent");
    let results = f
        .core()
        .ack_messages(
            &ack_context(&f, &f.pm_credential),
            &[a.clone(), b.clone(), a.clone()],
        )
        .unwrap();
    assert_eq!(results.as_array().unwrap().len(), 2);
    assert_eq!(state_of(&f, &a), "acked");
    assert_eq!(state_of(&f, &b), "acked");
}

/// Regenerates `testdata/ledger/schema-39-messages.sqlite`: a ledger at schema 39 with a PM and a message in every state.
/// Run with `cargo test -p capstan-kernel --test rust_only regenerate_schema_39_fixture -- --ignored`.
#[test]
#[ignore = "regenerates a checked-in fixture"]
fn regenerate_schema_39_fixture() {
    let mut f = Fixture::new();
    let states = [
        "queued",
        "deferred",
        "sent",
        "acked",
        "unacked",
        "acked_late",
        "expired",
        "failed",
        "cancelled",
    ];
    for (n, state) in states.iter().enumerate() {
        let id = enqueue(
            &f,
            "pm-1",
            &format!("a {state} message"),
            json!({"actionNeeded": n % 2 == 0}),
        )
        .unwrap();
        set_state(&f, &id, state);
    }
    enqueue(&f, "dev-1", "a queued message to the developer", json!({})).unwrap();
    step(f.core(), "syncRoleDefinitions", json!([[]]));
    f.close();
    let db = rusqlite::Connection::open(f.database()).unwrap();
    db.execute_batch(
        "PRAGMA foreign_keys = OFF;
         DROP TRIGGER messages_info_needs_no_action_insert;
         DROP TRIGGER messages_info_needs_no_action_update;
         ALTER TABLE messages DROP COLUMN class;
         DELETE FROM schema_migrations WHERE version >= 40;
         PRAGMA wal_checkpoint(TRUNCATE);",
    )
    .unwrap();
    drop(db);
    let target = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/ledger/schema-39-messages.sqlite");
    std::fs::copy(f.database(), &target).unwrap();
    let _ = std::fs::remove_file(target.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(target.with_extension("sqlite-shm"));
}

#[test]
fn the_wake_policy_applies_to_the_pm_only() {
    let f = Fixture::new();
    f.core()
        .set_message_policy(&json!({"pmWakeClass": "action"}));
    enqueue(&f, "dev-1", "fyi", json!({"class": "info"})).unwrap();
    enqueue(&f, "pm-1", "fyi", json!({"class": "info"})).unwrap();
    let worker = f.core().wake_pending(&f.developer_credential).unwrap();
    assert_eq!(worker["wake"], true, "{worker}");
    let pm = f.core().wake_pending(&f.pm_credential).unwrap();
    assert_eq!(pm["wake"], false, "{pm}");
    let summary = f.core().unread_summary(&f.developer_credential).unwrap();
    assert_eq!(summary["wakeCount"], 1);
}
