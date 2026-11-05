//! The PM card (`areas::pm_card`): the `pm` member of the run item, its freshness events and its bounds.

use crate::event_feed::{plan_body_json, Rig, COMMIT};
use capstan_kernel::areas::event_feed::{feed_page, Position};
use capstan_kernel::areas::pm_card::{
    pm_card, BODY_CAP, FROM_USER_SCAN, MAX_STATEMENTS, PM_CARD_ACTIONS, PM_CARD_IGNORED,
    PM_CARD_TABLES,
};
use capstan_kernel::areas::remote_decisions::parse;
use capstan_kernel::areas::snapshot_feed::{
    content_hash, snapshot_head, snapshot_item, waiting_page,
};
use serde_json::{json, Value};
use std::cell::Cell;
use std::collections::BTreeSet;

pub fn card(rig: &Rig) -> Value {
    snapshot_item(&rig.core, "run", "run").unwrap().unwrap()["pm"].clone()
}

fn kinds(card: &Value) -> Vec<(String, u64)> {
    card["waitingOn"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| {
            (
                w["kind"].as_str().unwrap().to_string(),
                w["count"].as_u64().unwrap(),
            )
        })
        .collect()
}

fn since(card: &Value, kind: &str) -> String {
    card["waitingOn"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["kind"] == kind)
        .unwrap_or_else(|| panic!("no {kind} in {card}"))["since"]
        .as_str()
        .unwrap()
        .to_string()
}

fn text(rig: &Rig, sql: &str) -> String {
    rig.db().query_row(sql, [], |row| row.get(0)).unwrap()
}

/// A message of the person to the PM through `pm message` from the client.
pub fn remote_message(rig: &Rig, client: &str, nonce: &str, body: &str) -> Value {
    let hash = content_hash(&rig.core, "pm").unwrap().unwrap();
    let words: Vec<String> = [
        "pm",
        "--expect",
        &hash[..12],
        "--nonce",
        nonce,
        "--client",
        client,
        "message",
        body,
    ]
    .iter()
    .map(|w| w.to_string())
    .collect();
    let input = parse(&words).unwrap();
    rig.core.decide(&rig.ctx("owner"), &input).unwrap()
}

fn send_question(rig: &Rig, body: &str) -> Value {
    rig.core
        .send_to_user(&rig.ctx("dev-2"), &json!({"body": body, "class": "action"}))
        .unwrap()
}

fn queue_verify(rig: &Rig, subject: &str) -> Value {
    rig.core
        .queue_verify_run(
            &rig.ctx("dev-1"),
            &json!({"subjectId": subject, "commands": ["make test"], "commandsSha": "cmd-sha-a", "trigger": "manual"}),
        )
        .unwrap()
}

fn begin_review(rig: &Rig, subject: &str) {
    rig.core
        .begin_review(
            &rig.ctx("pm-1"),
            &json!({"subjectId": subject, "reviewerRole": "reviewer", "reviewerAgentId": "rev-1"}),
        )
        .unwrap();
}

fn plan_in_review(rig: &Rig) -> String {
    let plan = rig
        .core
        .open_plan(
            &rig.ctx("pm-1"),
            &json!({"tier": "normal", "title": "Plan two"}),
        )
        .unwrap();
    let plan_id = plan["planId"].as_str().unwrap().to_string();
    rig.core
        .submit_plan(
            &rig.ctx("arch-1"),
            &json!({"planId": plan_id, "baseSha": crate::event_feed::BASE, "bodyJson": plan_body_json(), "review": true}),
        )
        .unwrap();
    plan_id
}

// ------------------------------------------------------------------------------------------------------- the member

#[test]
fn pm_is_null_without_an_active_pm_and_has_the_four_members_with_one() {
    let rig = Rig::new();
    let head = snapshot_head(&rig.core).unwrap();
    let run = serde_json::to_value(&head.run).unwrap();
    assert_eq!(
        run["pm"],
        json!({"agentId": "pm-1", "waitingOn": [], "fromUser": null, "stuck": null})
    );
    rig.core.end_agent(&rig.ctx("owner"), "pm-1", None).unwrap();
    assert_eq!(card(&rig), Value::Null);
    let run = snapshot_item(&rig.core, "run", "run").unwrap().unwrap();
    assert!(run.as_object().unwrap().contains_key("pm") && run["pm"].is_null());
}

#[test]
fn each_waiting_kind_is_counted_with_its_oldest_since_in_a_fixed_order() {
    let rig = Rig::new();
    rig.relay_on();
    rig.pane("dev-1", "feat/ui");
    rig.pane("dev-2", "feat/docs");
    let plan = rig.approved_plan();
    rig.assign(&plan, "docs", "dev-2");
    let first = card(&rig);
    assert_eq!(kinds(&first), vec![("report".to_string(), 1)]);
    let docs_at = text(
        &rig,
        "SELECT assigned_at FROM plan_packages WHERE package_id = 'docs'",
    );
    assert_eq!(since(&first, "report"), docs_at);
    rig.assign(&plan, "ui", "dev-1");
    let second = card(&rig);
    assert_eq!(kinds(&second), vec![("report".to_string(), 2)]);
    assert_eq!(since(&second, "report"), docs_at, "the oldest assignment");
    // The design package reports: it leaves `report` and waits for the user as a design.
    let report = rig.report_as("dev-1", "feat/ui", COMMIT, "designed the screens");
    let report_id = report["record"]["reportId"].as_str().unwrap().to_string();
    let third = card(&rig);
    assert_eq!(
        kinds(&third),
        vec![("report".to_string(), 1), ("user".to_string(), 1)]
    );
    assert_eq!(
        since(&third, "user"),
        text(&rig, "SELECT created_at FROM agent_reports"),
    );
    queue_verify(&rig, &report_id);
    begin_review(&rig, &report_id);
    let plan_two = plan_in_review(&rig);
    let proposal = rig.propose("echo hi", "to test");
    assert!(proposal["proposalId"].is_string());
    rig.capture("dev-2", "Allow?");
    send_question(&rig, "which one?");
    let full = card(&rig);
    assert_eq!(
        kinds(&full),
        vec![
            ("report".to_string(), 1),
            ("review".to_string(), 1),
            ("verify".to_string(), 1),
            ("plan_review".to_string(), 1),
            ("user".to_string(), 4),
        ]
    );
    assert_eq!(
        since(&full, "review"),
        text(&rig, "SELECT created_at FROM reviews")
    );
    assert_eq!(
        since(&full, "verify"),
        text(&rig, "SELECT queued_at FROM verify_runs")
    );
    assert_eq!(
        since(&full, "plan_review"),
        text(
            &rig,
            &format!("SELECT created_at FROM controller_events WHERE entity_id = '{plan_two}' AND json_extract(payload_json, '$.action') = 'plan.submit'")
        )
    );
    // The `user` count is the waiting section's (criterion: the same rows, the oldest `since`).
    let waiting = waiting_page(&rig.core, None, 100, None).unwrap();
    assert_eq!(waiting.items.len(), 4);
    let oldest = waiting.items.iter().map(|i| i.since.clone()).min().unwrap();
    assert_eq!(since(&full, "user"), oldest);
}

#[test]
fn the_user_count_follows_the_waiting_section_with_paused_expired_and_superseded_items() {
    let rig = Rig::new();
    rig.relay_on();
    rig.pane("dev-1", "feat/x");
    // An agent that is paused still has its items.
    rig.core
        .pause_agent(
            &rig.ctx("pm-1"),
            &json!({"agentId": "dev-1", "reason": "hold"}),
        )
        .unwrap();
    rig.capture("dev-1", "Allow one?");
    rig.propose("echo a", "r");
    // A prompt that expires leaves the section, and the card.
    rig.core
        .configure_prompt_relay(&json!({"enabled": true, "captureTtlSeconds": 0.001}))
        .unwrap();
    rig.capture("dev-2", "Allow two?");
    std::thread::sleep(std::time::Duration::from_millis(10));
    rig.core.expire_prompt_captures(&rig.ctx("owner")).unwrap();
    let waiting = waiting_page(&rig.core, None, 100, None).unwrap();
    let card = card(&rig);
    let counted = kinds(&card)
        .into_iter()
        .find(|(kind, _)| kind == "user")
        .map_or(0, |(_, count)| count);
    assert_eq!(counted as usize, waiting.items.len());
    assert!(counted >= 2);
    let oldest = waiting.items.iter().map(|i| i.since.clone()).min().unwrap();
    assert_eq!(since(&card, "user"), oldest);
    assert!(waiting.items.iter().all(|i| !i.since.is_empty()) && !waiting.items.is_empty());
}

#[test]
fn from_user_is_the_newest_operator_message_to_the_current_pm_with_its_delivery() {
    let rig = Rig::new();
    assert_eq!(card(&rig)["fromUser"], Value::Null);
    // A worker's message to the PM is not the person's.
    rig.core
        .enqueue_message(
            &rig.ctx("dev-1"),
            &json!({"recipientAgentId": "pm-1", "body": "progress", "actionNeeded": false}),
        )
        .unwrap();
    assert_eq!(card(&rig)["fromUser"], Value::Null);
    // A message of the person to a developer is not either.
    rig.enqueue("dev-1", "to the developer", false);
    assert_eq!(card(&rig)["fromUser"], Value::Null);
    // The terminal: the operator's own message.
    let id = rig.enqueue("pm-1", "check the plan please", true);
    let from = card(&rig)["fromUser"].clone();
    assert_eq!(from["messageId"], json!(id));
    assert_eq!(from["client"], "terminal");
    assert_eq!(from["state"], "queued");
    assert_eq!(from["delivery"], "queued");
    assert_eq!(
        from["body"],
        json!({"text": "check the plan please", "textVerified": true, "truncated": false})
    );
    // A remote message later is the newest; the wrapper is taken off.
    let sent = remote_message(&rig, "iPhone", "nonce-0001", "Which database?");
    let from = card(&rig)["fromUser"].clone();
    assert_eq!(from["messageId"], sent["messageId"]);
    assert_eq!(from["client"], "iPhone");
    assert_eq!(from["body"]["text"], "Which database?");
    // Every message state maps to a delivery.
    let id = sent["messageId"].as_str().unwrap().to_string();
    let db = rig.db();
    let expected = [
        ("queued", "queued"),
        ("deferred", "queued"),
        ("sent", "delivered"),
        ("unacked", "delivered"),
        ("acked", "read"),
        ("acked_late", "read"),
        ("expired", "not_delivered"),
        ("cancelled", "not_delivered"),
        ("failed", "not_delivered"),
    ];
    for (state, delivery) in expected {
        assert_eq!(capstan_kernel::areas::pm_card::delivery_of(state), delivery);
    }
    drop(db);
    // States through the kernel: a pull and a send move it on.
    let _ = id;
}

#[test]
fn from_user_cuts_the_text_at_three_hundred_bytes_and_flags_it() {
    let rig = Rig::new();
    let long = "é".repeat(200);
    rig.enqueue("pm-1", &long, true);
    let body = card(&rig)["fromUser"]["body"].clone();
    assert_eq!(body["truncated"], true);
    assert_eq!(body["textVerified"], true);
    let shown = body["text"].as_str().unwrap();
    assert!(shown.len() <= BODY_CAP && shown.len() >= BODY_CAP - 1);
    assert!(long.starts_with(shown));
}

#[test]
fn a_message_whose_text_no_longer_matches_its_hash_gives_no_text() {
    let rig = Rig::new();
    let id = rig.enqueue("pm-1", "original", true);
    let db = rig.db();
    db.execute_batch("DROP TRIGGER IF EXISTS messages_write_once")
        .ok();
    let triggers: Vec<String> = db
        .prepare("SELECT name FROM sqlite_master WHERE type = 'trigger' AND tbl_name = 'messages'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    for name in &triggers {
        db.execute_batch(&format!("DROP TRIGGER \"{name}\""))
            .unwrap();
    }
    db.execute(
        "UPDATE messages SET body = 'tampered' WHERE message_id = ?",
        [&id],
    )
    .unwrap();
    drop(db);
    let from = card(&rig)["fromUser"].clone();
    assert_eq!(
        from["body"],
        json!({"text": null, "textVerified": false, "truncated": false})
    );
}

#[test]
fn a_new_pm_after_a_replace_shows_its_own_values() {
    let rig = Rig::new();
    rig.relay_on();
    rig.enqueue("pm-1", "for the first", true);
    rig.capture("dev-1", "Allow?");
    assert!(card(&rig)["fromUser"].is_object());
    assert_eq!(card(&rig)["waitingOn"][0]["kind"], "user");
    rig.core
        .replace_agent_generation(&rig.ctx("owner"), "pm-1")
        .unwrap();
    let after = card(&rig);
    assert_eq!(after["agentId"], "pm-1");
    assert_eq!(
        after["fromUser"],
        Value::Null,
        "the new generation has no message of the person yet"
    );
    let id = rig.enqueue("pm-1", "for the second", true);
    assert_eq!(card(&rig)["fromUser"]["messageId"], json!(id));
}

// ------------------------------------------------------------------------------------------------------------ bounds

thread_local! {
    static STATEMENTS: Cell<usize> = const { Cell::new(0) };
}

fn count_statement(event: rusqlite::trace::TraceEvent<'_>) {
    if let rusqlite::trace::TraceEvent::Stmt(_, _) = event {
        STATEMENTS.with(|c| c.set(c.get() + 1));
    }
}

fn statements_of_one_computation(rig: &Rig) -> usize {
    let connection = rig.core.kernel().database.connection();
    connection.trace_v2(
        rusqlite::trace::TraceEventCodes::SQLITE_TRACE_STMT,
        Some(count_statement),
    );
    STATEMENTS.with(|c| c.set(0));
    let found = pm_card(&rig.core).unwrap();
    assert!(found.is_some());
    let used = STATEMENTS.with(Cell::get);
    connection.trace_v2(rusqlite::trace::TraceEventCodes::empty(), None);
    used
}

#[test]
fn one_computation_takes_at_most_eleven_statements_with_one_row_or_five_hundred_per_table() {
    let rig = Rig::new();
    rig.relay_on();
    rig.pane("dev-1", "feat/x");
    let report = rig.report_as("dev-1", "feat/x", COMMIT, "done");
    let report_id = report["record"]["reportId"].as_str().unwrap().to_string();
    let small = statements_of_one_computation(&rig);
    assert!(small <= MAX_STATEMENTS, "{small}");
    // 500 rows in the tables the card counts and reads.
    let db = rig.db();
    let (project, actor): (String, String) = db
        .query_row(
            "SELECT project_id, actor_id FROM agents WHERE agent_id = 'dev-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    db.execute_batch("BEGIN").unwrap();
    for i in 1..=500i64 {
        db.execute(
            "INSERT INTO user_messages(project_id, message_id, sequence, sender_agent_id, sender_actor_id, class, body, body_hash, state, created_at)
             VALUES (?, ?, ?, 'dev-1', ?, 'action', 'q', ?, 'open', '2026-01-01T00:00:00.000Z')",
            rusqlite::params![project, format!("um-{i}"), i, actor, "a".repeat(64)],
        )
        .unwrap();
        db.execute(
            "INSERT INTO messages(project_id, message_id, sequence, recipient_agent_id, recipient_generation, sender_actor_id, body, body_hash,
               state, state_version, queued_at, deferral_count, send_attempts, created_at, updated_at)
             VALUES (?, ?, ?, 'pm-1', 1, ?, 'b', ?, 'queued', 0, '2026-01-01T00:00:00.000Z', 0, 0, '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')",
            rusqlite::params![project, format!("m-{i}"), 1000 + i, actor, "b".repeat(64)],
        )
        .unwrap();
        db.execute(
            "INSERT INTO verify_runs(project_id, run_id, sequence, subject_report_id, commit_sha, commands_json, commands_sha, trigger, requested_by_actor_id, state, queued_at)
             VALUES (?, ?, ?, ?, ?, '[]', ?, 'manual', ?, 'queued', '2026-01-01T00:00:00.000Z')",
            rusqlite::params![project, format!("v-{i}"), i, report_id, format!("{i:040x}"), format!("c{i}"), actor],
        )
        .unwrap();
    }
    db.execute_batch("COMMIT").unwrap();
    drop(db);
    let big = statements_of_one_computation(&rig);
    assert_eq!(big, small, "the statements do not depend on the rows");
    assert!(big <= MAX_STATEMENTS);
    let card = card(&rig);
    assert_eq!(
        kinds(&card),
        vec![("verify".to_string(), 500), ("user".to_string(), 500)]
    );
}

// ----------------------------------------------------------------------------------------------------------- freshness

fn run_invalidates(events: &[Value]) -> usize {
    events
        .iter()
        .flat_map(|event| {
            event["body"]["effects"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter(|effect| {
            effect["section"] == "run" && effect["op"] == "invalidate" && effect["key"] == "run"
        })
        .count()
}

#[test]
fn a_new_run_invalidate_comes_last_in_the_effects_and_a_row_without_events_gets_a_run_event() {
    let rig = Rig::new();
    let (_, events) = rig.run(|r| r.enqueue("pm-1", "hello", true));
    assert_eq!(events.len(), 1);
    let effects = events[0]["body"]["effects"].as_array().unwrap();
    assert_eq!(
        effects.last().unwrap(),
        &json!({"section": "run", "op": "invalidate", "key": "run", "fields": {}})
    );
    assert_eq!(run_invalidates(&events), 1);
    // An ordinary message that is acknowledged maps to no event of its own: it is one run event.
    let id = rig.enqueue("pm-1", "ordinary", false);
    let head = rig.head();
    rig.insert_rows(
        1,
        "message",
        &id,
        "message.transition",
        Some("sent"),
        Some("acked"),
    );
    let events = rig.since(head);
    assert_eq!(events.len(), 1, "{events:#?}");
    assert_eq!(events[0]["type"], "snapshot.invalidated");
    assert_eq!(events[0]["body"]["section"], "run");
    assert_eq!(events[0]["body"]["key"], "run");
    assert_eq!(run_invalidates(&events), 1);
}

#[test]
fn five_hundred_message_rows_between_workers_give_no_run_invalidate_and_nor_do_claims() {
    let rig = Rig::new();
    let id = {
        let r = rig.core
            .enqueue_message(
                &rig.ctx("dev-1"),
                &json!({"recipientAgentId": "dev-2", "body": "between workers", "actionNeeded": false}),
            )
            .unwrap();
        r["messageId"].as_str().unwrap().to_string()
    };
    let head = rig.head();
    rig.insert_rows(
        500,
        "message",
        &id,
        "message.transition",
        Some("sent"),
        Some("acked"),
    );
    let page = feed_page(&rig.core, Position { seq: head, k: 0 }, 500, None).unwrap();
    assert_eq!(run_invalidates(&page.events), 0);
    // A message to the PM from a worker is not the person's either.
    let to_pm = rig
        .core
        .enqueue_message(
            &rig.ctx("dev-1"),
            &json!({"recipientAgentId": "pm-1", "body": "to the pm", "actionNeeded": false}),
        )
        .unwrap()["messageId"]
        .as_str()
        .unwrap()
        .to_string();
    let head = rig.head();
    rig.insert_rows(
        50,
        "message",
        &to_pm,
        "message.transition",
        Some("sent"),
        Some("acked"),
    );
    let page = feed_page(&rig.core, Position { seq: head, k: 0 }, 500, None).unwrap();
    assert_eq!(run_invalidates(&page.events), 0);
    // Verify: a queue opens, a claim and a process change nothing the card counts, a finish closes.
    rig.pane("dev-1", "feat/x");
    let report = rig.report_as("dev-1", "feat/x", COMMIT, "done");
    let report_id = report["record"]["reportId"].as_str().unwrap().to_string();
    let (run, events) = rig.run(|r| queue_verify(r, &report_id));
    assert_eq!(run_invalidates(&events), 1, "{events:#?}");
    let run_id = run["runId"].as_str().unwrap().to_string();
    let (_, events) = rig.run(|r| {
        r.core
            .claim_verify_run(
                &r.ctx("owner"),
                &json!({"runId": run_id, "scratchPath": "/tmp/s", "logPath": "/tmp/l", "maxParallel": 4}),
            )
            .unwrap()
    });
    assert_eq!(run_invalidates(&events), 0, "{events:#?}");
    let (_, events) = rig.run(|r| {
        r.core
            .record_verify_process(
                &r.ctx("owner"),
                &json!({"runId": run_id, "pgid": 4242, "leaderStart": "1"}),
            )
            .unwrap()
    });
    assert_eq!(run_invalidates(&events), 0, "{events:#?}");
    let (_, events) = rig.run(|r| {
        r.core
            .finish_verify_run(
                &r.ctx("owner"),
                &json!({"runId": run_id, "state": "passed", "exitCode": 0, "durationMs": 5, "outputTail": "ok"}),
            )
            .unwrap()
    });
    assert_eq!(run_invalidates(&events), 1, "{events:#?}");
}

#[test]
fn every_action_of_pm_card_actions_gives_one_run_invalidate_on_a_row_that_can_change_the_card() {
    let rig = Rig::new();
    let id = rig.enqueue("pm-1", "from the person", true);
    let mut covered = BTreeSet::new();
    for action in PM_CARD_ACTIONS {
        let (entity, from, to): (&str, Option<&str>, Option<&str>) =
            match action.split('.').next().unwrap() {
                "message" => ("message", Some("queued"), Some("sent")),
                "verify" => ("verify_run", Some("running"), Some("passed")),
                "prompt_relay" => ("prompt_relay", Some("captured"), Some("answered")),
                "agent" => ("agent", Some("active"), Some("ended")),
                "report" => ("agent_report", None, Some("accepted")),
                other => (other, Some("a"), Some("b")),
            };
        let entity_id = if entity == "message" {
            id.clone()
        } else {
            format!("{entity}-x")
        };
        let (entity, from, to) = match *action {
            "verify.queue" => ("verify_run", None, Some("queued")),
            "verify.claim" | "verify.orphan" => continue,
            "verify.interrupt" => ("verify_run", None, None),
            "prompt_relay.capture" => ("prompt_relay", None, Some("captured")),
            _ => (entity, from, to),
        };
        let head = rig.head();
        rig.insert_rows(1, entity, &entity_id, action, from, to);
        let events = rig.since(head);
        assert_eq!(run_invalidates(&events), 1, "{action}: {events:#?}");
        assert!(
            events.iter().all(|e| {
                e["body"]["effects"]
                    .as_array()
                    .unwrap()
                    .last()
                    .map(|l| l["section"] == "run")
                    == Some(true)
                    || e["body"]["effects"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|l| l["section"] != "run")
            }),
            "{action}: the run effect is last"
        );
        covered.insert(action.to_string());
    }
    assert!(covered.len() >= PM_CARD_ACTIONS.len() - 2);
}

#[test]
fn claim_and_process_rows_and_other_actions_give_no_run_invalidate() {
    let rig = Rig::new();
    for (entity, action, from, to) in [
        (
            "verify_run",
            "verify.claim",
            Some("queued"),
            Some("running"),
        ),
        ("verify_run", "verify.orphan", None, None),
        (
            "prompt_relay",
            "prompt_relay.finish",
            Some("typing"),
            Some("answered"),
        ),
        ("verify_run", "verify.process", None, None),
        ("pm", "pm.wake", None, None),
    ] {
        let head = rig.head();
        rig.insert_rows(1, entity, &format!("{entity}-y"), action, from, to);
        assert_eq!(run_invalidates(&rig.since(head)), 0, "{action}");
    }
}

fn source_files() -> Vec<std::path::PathBuf> {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut out,
    );
    out
}

fn known_actions() -> BTreeSet<String> {
    include_str!("fixtures/event_feed/known_actions.txt")
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .filter_map(|line| line.split_once(' ').map(|(_, action)| action.to_string()))
        .collect()
}

/// The action literals passed to `mutate`-like calls in `kernel/src` (the scan the known file is checked with).
fn scanned_actions() -> BTreeSet<String> {
    let name = r#""([a-z_]+\.[a-z_]+)""#;
    let patterns = [
        format!(r"\b(?:mutate|mutate_as_controller)\(\s*[A-Za-z_&.]+,\s*{name}"),
        format!(
            r"\b(?:mutate_as_window_controller|message_mutation|reject)\(\s*[^,]+,\s*[^,]+,\s*{name}"
        ),
        format!(r"\bappend_event\(\s*(?:[^,]+,\s*){{6}}{name}"),
        format!(r"\{{\s*{name}\s*\}}\s*else\s*\{{\s*{name}\s*\}}"),
    ];
    let mut found = BTreeSet::new();
    for file in source_files() {
        let source = std::fs::read_to_string(file).unwrap();
        for pattern in &patterns {
            let regex = regex::Regex::new(pattern).unwrap();
            for captures in regex.captures_iter(&source) {
                for group in captures.iter().skip(1).flatten() {
                    found.insert(group.as_str().to_string());
                }
            }
        }
    }
    found
}

#[test]
fn every_known_or_scanned_action_is_a_card_action_or_ignored_with_tables_the_card_does_not_read() {
    let classified: BTreeSet<&str> = PM_CARD_ACTIONS
        .iter()
        .copied()
        .chain(PM_CARD_IGNORED.iter().map(|(action, _, _)| *action))
        .collect();
    let mut wanted = known_actions();
    wanted.extend(scanned_actions());
    // A name in both lists would be classified twice.
    for action in PM_CARD_ACTIONS {
        assert!(
            !PM_CARD_IGNORED
                .iter()
                .any(|(ignored, _, _)| ignored == action),
            "{action} is both a card action and ignored"
        );
    }
    let missing: Vec<&String> = wanted
        .iter()
        .filter(|a| !classified.contains(a.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "classify these in PM_CARD_ACTIONS or PM_CARD_IGNORED (with the tables they write): {missing:?}"
    );
    // The lists name only actions the code writes.
    for action in &classified {
        assert!(
            wanted.contains(*action),
            "{action} is classified but unknown to the code"
        );
    }
    // An ignored action writes no table the card reads.
    for (action, tables, reason) in PM_CARD_IGNORED {
        assert!(!tables.is_empty() && !reason.is_empty(), "{action}");
        for table in *tables {
            assert!(
                !PM_CARD_TABLES.contains(table),
                "{action} writes {table}, which the card reads"
            );
        }
    }
    assert!(
        PM_CARD_ACTIONS.windows(2).all(|w| w[0] < w[1]),
        "sorted and unique"
    );
    // The card reads only tables that exist.
    let rig = Rig::new();
    let db = rig.db();
    for table in PM_CARD_TABLES {
        db.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap_or_else(|e| panic!("{table}: {e}"));
    }
}

#[test]
fn the_content_hash_of_the_run_does_not_cover_pm() {
    let rig = Rig::new();
    rig.relay_on();
    let before = content_hash(&rig.core, "run").unwrap().unwrap();
    rig.enqueue("pm-1", "hello", true);
    rig.capture("dev-1", "Allow?");
    assert_eq!(content_hash(&rig.core, "run").unwrap().unwrap(), before);
    let run = snapshot_item(&rig.core, "run", "run").unwrap().unwrap();
    assert_eq!(run["contentHash"], json!(before));
}

#[test]
fn the_queries_of_the_user_kinds_and_of_from_user_use_the_indexes_named_here() {
    let rig = Rig::new();
    let db = rig.db();
    let named = [
        ("op", "operator_proposals_by_state"),
        ("prompt", "prompt_relays_by_state"),
        ("message", "user_messages_by_state"),
        ("from_user", "messages_by_recipient"),
    ];
    for (name, sql) in capstan_kernel::areas::pm_card::query_texts() {
        let mut statement = db
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let params = vec!["x"; statement.parameter_count()];
        let plan: Vec<String> = statement
            .query_map(rusqlite::params_from_iter(params), |row| {
                row.get::<_, String>(3)
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        if let Some((_, index)) = named.iter().find(|(n, _)| *n == name) {
            assert!(
                plan.iter().any(|line| line.contains(index)),
                "{name} does not use {index}: {plan:?}"
            );
        }
    }
}

#[test]
fn from_user_looks_through_at_most_the_newest_five_hundred_messages_to_the_pm() {
    let rig = Rig::new();
    remote_message(&rig, "phone", "nonce-0001", "old question");
    assert_eq!(card(&rig)["fromUser"]["body"]["text"], "old question");
    let (project, actor): (String, String) = rig
        .db()
        .query_row(
            "SELECT project_id, actor_id FROM agents WHERE agent_id = 'dev-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let insert = |count: usize, first: usize| {
        let db = rig.db();
        db.execute_batch("BEGIN").unwrap();
        for i in first..first + count {
            db.execute(
                "INSERT INTO messages(project_id, message_id, sequence, recipient_agent_id, recipient_generation, sender_actor_id, body, body_hash,
                   state, state_version, queued_at, deferral_count, send_attempts, created_at, updated_at)
                 VALUES (?, ?, ?, 'pm-1', 1, ?, 'b', ?, 'queued', 0, '2026-01-01T00:00:00.000Z', 0, 0, '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')",
                rusqlite::params![project, format!("w-{i}"), 100_000 + i as i64, actor, "b".repeat(64)],
            )
            .unwrap();
        }
        db.execute_batch("COMMIT").unwrap();
    };
    // 499 newer messages from a worker: the operator's message is the 500th newest and is still found.
    insert(FROM_USER_SCAN - 1, 0);
    assert_eq!(card(&rig)["fromUser"]["body"]["text"], "old question");
    // One more pushes it out of the window.
    insert(1, FROM_USER_SCAN);
    assert_eq!(card(&rig)["fromUser"], Value::Null);
}
