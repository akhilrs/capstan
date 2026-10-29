//! Hot kernel paths read only the open mail of an agent, never its history; `pm_mail_digest` reads no body. Each hot path
//! is compared with the old filter applied to the whole history (`messages_for`, which still reads every row).

#[path = "rust_only/fixture.rs"]
mod fixture;

use capstan_kernel::types::MutationContext;
use fixture::{context, Fixture, OWNER};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const HISTORY: usize = 5000;
const AGENTS: [&str; 2] = ["pm-1", "dev-1"];

fn enqueue(f: &Fixture, agent: &str, body: &str, extra: Value) -> String {
    let mut input = json!({"recipientAgentId": agent, "body": body});
    for (key, value) in extra.as_object().unwrap() {
        input[key] = value.clone();
    }
    let core = f.core();
    core.enqueue_message(&context(core, OWNER), &input).unwrap()["messageId"]
        .as_str()
        .unwrap()
        .to_string()
}

fn sql(f: &Fixture, edit: impl FnOnce(&rusqlite::Connection)) {
    let db = rusqlite::Connection::open(f.database()).unwrap();
    edit(&db);
}

/// A ledger where each agent has `acked` acked messages and then 2 queued (one action, one info), 1 sent and 1 unacked.
fn ledger(acked: usize) -> Fixture {
    let mut f = Fixture::new();
    if acked > 0 {
        let templates: Vec<String> = AGENTS
            .iter()
            .map(|agent| enqueue(&f, agent, "history", json!({})))
            .collect();
        f.close();
        sql(&f, |db| {
            db.execute_batch("BEGIN").unwrap();
            for template in &templates {
                let base: i64 = db
                    .query_row("SELECT MAX(sequence) FROM messages", [], |r| r.get(0))
                    .unwrap();
                db.execute(
                    "UPDATE messages SET state = 'acked', sent_at = queued_at, acked_at = queued_at WHERE project_id = 'prustonly' AND message_id = ?",
                    [template],
                )
                .unwrap();
                let mut insert = db
                    .prepare(
                        "INSERT INTO messages(project_id, message_id, sequence, recipient_agent_id, recipient_generation, sender_actor_id, body, body_hash, state, state_version, queued_at, deferral_count, sent_at, acked_at, send_attempts, created_at, updated_at)
                         SELECT project_id, ?1 || '-' || ?2, ?3, recipient_agent_id, recipient_generation, sender_actor_id, body, body_hash, 'acked', state_version, queued_at, 0, sent_at, acked_at, 1, created_at, updated_at FROM messages WHERE project_id = 'prustonly' AND message_id = ?1",
                    )
                    .unwrap();
                for i in 1..acked {
                    insert
                        .execute(rusqlite::params![template, i as i64, base + i as i64])
                        .unwrap();
                }
            }
            db.execute_batch("COMMIT").unwrap();
        });
        f.reopen();
    }
    let mut states = Vec::new();
    for agent in AGENTS {
        let ids = [
            enqueue(&f, agent, "do this", json!({"actionNeeded": true})),
            enqueue(&f, agent, "fyi", json!({"class": "info"})),
            enqueue(&f, agent, "sent one", json!({})),
            enqueue(&f, agent, "unacked one", json!({})),
        ];
        states.push(ids);
    }
    f.close();
    sql(&f, |db| {
        for ids in &states {
            db.execute(
                "UPDATE messages SET state = 'sent', sent_at = queued_at WHERE message_id = ?",
                [&ids[2]],
            )
            .unwrap();
            db.execute(
                "UPDATE messages SET state = 'unacked', sent_at = queued_at WHERE message_id = ?",
                [&ids[3]],
            )
            .unwrap();
        }
    });
    f.reopen();
    f
}

fn credential(f: &Fixture, agent: &str) -> String {
    if agent == "pm-1" {
        f.pm_credential.clone()
    } else {
        f.developer_credential.clone()
    }
}

fn history(f: &Fixture, agent: &str) -> Vec<Value> {
    f.core()
        .messages_for(agent)
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

fn state(m: &Value) -> &str {
    m["state"].as_str().unwrap()
}

fn is_final(m: &Value) -> bool {
    matches!(state(m), "acked" | "acked_late" | "cancelled")
}

fn ids_where(rows: &[Value], keep: impl Fn(&Value) -> bool) -> Vec<Value> {
    rows.iter()
        .filter(|m| keep(m))
        .map(|m| m["messageId"].clone())
        .collect()
}

fn ctx(f: &Fixture, credential: &str) -> MutationContext {
    context(f.core(), credential)
}

#[test]
fn read_paths_return_what_the_old_filter_gives_on_the_whole_history() {
    let f = ledger(HISTORY);
    for agent in AGENTS {
        let rows = history(&f, agent);
        assert_eq!(rows.len(), HISTORY + 4);
        let cred = credential(&f, agent);
        let queued = |m: &Value| matches!(state(m), "queued" | "deferred");
        let waiting = |m: &Value| matches!(state(m), "queued" | "deferred" | "sent" | "unacked");
        let wake = f.core().wake_pending(&cred).unwrap();
        assert_eq!(
            wake,
            json!({"wake": true, "count": 2, "infoCount": 1}),
            "{agent}"
        );
        let summary = f.core().unread_summary(&cred).unwrap();
        assert_eq!(summary["count"], 4);
        assert_eq!(
            summary["messageIds"],
            Value::Array(ids_where(&rows, waiting))
        );
        let oldest = rows
            .iter()
            .filter(|m| waiting(m))
            .map(|m| m["queuedAt"].as_str().unwrap())
            .min();
        assert_eq!(summary["oldestQueuedAt"], json!(oldest));
        assert_eq!(
            summary["actionNeeded"],
            rows.iter()
                .filter(|m| waiting(m) && m["actionNeeded"] == true)
                .count()
        );
        let head = rows.iter().find(|m| !is_final(m)).unwrap()["messageId"].clone();
        let inbox = f.core().agent_inbox(&cred, Some(agent)).unwrap();
        let expected: Vec<Value> = rows
            .iter()
            .filter(|m| m["messageId"] == head || matches!(state(m), "sent" | "unacked"))
            .cloned()
            .collect();
        assert_eq!(inbox, Value::Array(expected), "{agent}");
        assert!(rows.iter().any(queued));
    }
}

#[test]
fn agent_seed_reads_the_last_messages_and_counts_the_rest() {
    let f = ledger(HISTORY);
    let rows = history(&f, "dev-1");
    let seed = f.core().agent_seed("dev-1").unwrap();
    let shown = seed["messages"].as_array().unwrap();
    let ids: Vec<&Value> = shown.iter().map(|m| &m["messageId"]).collect();
    let expected: Vec<&Value> = rows[rows.len() - shown.len()..]
        .iter()
        .map(|m| &m["messageId"])
        .collect();
    assert_eq!(ids, expected);
    assert!(!shown.is_empty());
    assert_eq!(
        seed["messagesOmitted"].as_u64().unwrap() as usize,
        rows.len() - shown.len()
    );
}

#[test]
fn pull_pending_and_deliver_inbox_move_exactly_the_open_mail() {
    for digest in [None, Some(false), Some(true)] {
        let f = ledger(HISTORY);
        let cred = f.pm_credential.clone();
        let rows = history(&f, "pm-1");
        let open = ids_where(&rows, |m| {
            matches!(state(m), "queued" | "deferred" | "sent" | "unacked")
        });
        match digest {
            None => {
                let pulled = f.core().pull_pending(&cred).unwrap();
                let pulled: Vec<Value> = pulled
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|m| m["messageId"].clone())
                    .collect();
                assert_eq!(pulled, open[..2].to_vec());
            }
            Some(digest) => {
                let out = f.core().deliver_inbox(&cred, digest).unwrap();
                assert_eq!(out["pulled"], true);
                let shown: Vec<Value> = out["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|m| m["messageId"].clone())
                    .collect();
                let info: Vec<Value> = out["info"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|m| m["messageId"].clone())
                    .collect();
                if digest {
                    assert_eq!(info, vec![rows[HISTORY + 1]["messageId"].clone()]);
                    assert_eq!(shown.len(), 3);
                } else {
                    assert!(info.is_empty());
                    assert_eq!(shown, open);
                }
            }
        }
        let after = history(&f, "pm-1");
        assert_eq!(
            after.iter().filter(|m| state(m) == "acked").count(),
            HISTORY + usize::from(digest == Some(true))
        );
        assert_eq!(after.iter().filter(|m| state(m) == "queued").count(), 0);
    }
}

#[test]
fn ack_all_read_and_pull_message_act_on_the_open_mail() {
    let f = ledger(HISTORY);
    let rows = history(&f, "dev-1");
    let acked = f
        .core()
        .ack_all_read(&ctx(&f, &f.developer_credential))
        .unwrap();
    assert_eq!(
        acked,
        Value::Array(ids_where(&rows, |m| matches!(state(m), "sent" | "unacked")))
    );

    let rows = history(&f, "pm-1");
    let head = rows.iter().find(|m| !is_final(m)).unwrap()["messageId"].clone();
    let pulled = f.core().pull_message(&ctx(&f, &f.pm_credential)).unwrap();
    assert_eq!(pulled["message"]["messageId"], head);
    assert_eq!(pulled["message"]["state"], "sent");
}

#[test]
fn end_agent_record_agent_lost_and_restart_summary_name_the_open_mail() {
    let f = ledger(HISTORY);
    let open = ids_where(&history(&f, "dev-1"), |m| !is_final(m));
    assert_eq!(open.len(), 4);
    f.core()
        .record_agent_lost(&ctx(&f, OWNER), &json!({"agentId": "dev-1"}))
        .unwrap();
    let lost: String = rusqlite::Connection::open(f.database())
        .unwrap()
        .query_row(
            "SELECT payload_json FROM controller_events WHERE entity_id = 'dev-1' AND to_state = 'lost'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let lost: Value = serde_json::from_str(&lost).unwrap();
    assert_eq!(
        lost["details"]["unacknowledgedMessageIds"],
        Value::Array(open.clone())
    );
    assert_eq!(lost["details"]["unacknowledgedCount"], 4);

    let expected_carried = ids_where(&history(&f, "pm-1"), |m| !is_final(m));
    let summary = f.restart_pm();
    let carried: Vec<Value> = summary["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["messageId"].clone())
        .collect();
    assert_eq!(carried, expected_carried);

    let ended = f.core().end_agent(&ctx(&f, OWNER), "dev-1", None).unwrap();
    let cancelled: Vec<Value> = ended["cancelledMessageIds"].as_array().unwrap().clone();
    let expected: Vec<Value> = open
        .iter()
        .filter(|id| *id != &Value::Null)
        .cloned()
        .collect();
    assert!(expected.iter().all(|id| cancelled.contains(id)));
    let after = history(&f, "dev-1");
    assert_eq!(
        after.iter().filter(|m| state(m) == "acked").count(),
        HISTORY
    );
    assert!(after[..HISTORY].iter().all(|m| state(m) == "acked"));
}

/// The VM steps SQLite runs while `work` runs on the core's own connection.
fn steps(f: &Fixture, work: impl FnOnce()) -> u64 {
    let counter = Arc::new(AtomicU64::new(0));
    let seen = Arc::clone(&counter);
    let conn = f.core().kernel().database.connection();
    let _ = conn.progress_handler(
        1,
        Some(move || {
            seen.fetch_add(1, Ordering::Relaxed);
            false
        }),
    );
    work();
    let _ = conn.progress_handler(0, None::<fn() -> bool>);
    counter.load(Ordering::Relaxed)
}

#[test]
fn the_work_is_bounded_by_the_open_rows_not_the_history() {
    let small = ledger(0);
    let big = ledger(HISTORY);
    let cost = |f: &Fixture| {
        let cred = f.pm_credential.clone();
        steps(f, || {
            f.core().wake_pending(&cred).unwrap();
        })
    };
    let (a, b) = (cost(&small), cost(&big));
    assert!(a > 0, "the counter saw nothing");
    assert!(
        b <= 2 * a,
        "wake_pending cost {b} steps with history, {a} without"
    );
}

#[test]
fn pm_mail_digest_gives_states_of_the_given_ids_and_the_latest_pull() {
    let f = ledger(HISTORY);
    let rows = history(&f, "pm-1");
    let id = |n: usize| rows[HISTORY + n]["messageId"].as_str().unwrap().to_string();
    let ask = vec![
        id(0),
        id(2),
        "unknown".to_string(),
        rows[0]["messageId"].as_str().unwrap().to_string(),
    ];
    let digest = f.core().pm_mail_digest("pm-1", &ask).unwrap();
    assert_eq!(
        digest["states"],
        json!({id(0): "queued", id(2): "sent", rows[0]["messageId"].as_str().unwrap(): "acked"})
    );
    let latest = rows
        .iter()
        .flat_map(|m| [m["sentAt"].as_str(), m["ackedAt"].as_str()])
        .flatten()
        .max()
        .unwrap();
    let expected = capstan_kernel::areas::messaging::parse_iso_ms(latest);
    assert_eq!(digest["latestPullMs"], json!(expected));
    // Another agent's message is not reported under this agent.
    let other = history(&f, "dev-1")[HISTORY]["messageId"]
        .as_str()
        .unwrap()
        .to_string();
    let digest = f.core().pm_mail_digest("pm-1", &[other]).unwrap();
    assert_eq!(digest["states"], json!({}));

    let empty = Fixture::new();
    let digest = empty.core().pm_mail_digest("pm-1", &[]).unwrap();
    assert_eq!(digest, json!({"states": {}, "latestPullMs": null}));
}

#[test]
fn pm_mail_digest_reads_no_body() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    let f = ledger(0);
    let conn = f.core().kernel().database.connection();
    let reads = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen = Arc::clone(&reads);
    let _ = conn.authorizer(Some(move |context: AuthContext<'_>| {
        if let AuthAction::Read {
            table_name,
            column_name,
        } = context.action
        {
            seen.lock()
                .unwrap()
                .push(format!("{table_name}.{column_name}"));
        }
        Authorization::Allow
    }));
    f.core().pm_mail_digest("pm-1", &["x".to_string()]).unwrap();
    let _ = conn.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    let reads = reads.lock().unwrap().clone();
    assert!(reads.iter().any(|r| r == "messages.state"), "{reads:?}");
    assert!(reads.iter().all(|r| !r.ends_with(".body")), "{reads:?}");
}
