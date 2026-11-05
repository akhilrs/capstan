//! A stuck PM (`areas::pm_stuck`): the pure rules, the episodes the tick keeps, restarts, the card member, the feed events
//! and the push event. Every test passes `now_ms` itself; nothing sleeps.

use crate::event_feed::{check_event, events_of, project_of, Rig};
use capstan_config::NtfyEvent;
use capstan_kernel::areas::event_feed::{feed_page, recent_page, Position};
use capstan_kernel::areas::messaging::{
    AgentFacts, MessageClass, MessageFacts, StateObservation, WaitInterval,
};
use capstan_kernel::areas::pm_stuck::{
    evaluate_pm_stuck, Evaluation, Reason, RelayFact, CLEAR_MS, SUPPRESS_MS,
};
use capstan_kernel::areas::push_events::push_events_after;
use capstan_kernel::areas::snapshot_feed::snapshot_item;
use capstan_kernel::env::SEEDED_EPOCH_MS;
use serde_json::{json, Value};
use std::collections::HashMap;

const N: i64 = 15;
const MIN: i64 = 60_000;
/// Ten seconds after the epoch: every row the rig writes is older (the seeded clock moves one millisecond per reading).
const T0: i64 = SEEDED_EPOCH_MS + 10_000;

// ------------------------------------------------------------------------------------------------------ the pure rules

fn pm(state: &str, since: i64, last_activity: i64) -> AgentFacts {
    AgentFacts {
        agent_id: "pm-1".into(),
        kind: "PM".into(),
        last_activity_ms: last_activity,
        observations: vec![StateObservation {
            state: state.into(),
            at_ms: since,
        }],
        waits: Vec::new(),
        paused: false,
    }
}

fn worker(id: &str, kind: &str, state: &str) -> AgentFacts {
    AgentFacts {
        agent_id: id.into(),
        kind: kind.into(),
        ..pm(state, T0, T0)
    }
}

fn mail(to: &str, queued: i64, state: &str, class: MessageClass) -> MessageFacts {
    MessageFacts {
        message_id: format!("m-{queued}"),
        recipient_agent_id: to.into(),
        state: state.into(),
        class,
        sequence: queued,
        queued_ms: queued,
        sent_ms: None,
        deferred_ms: None,
        deferred_reason: None,
        input_clear_recorded: false,
        last_notified_ms: None,
        wake_count: 0,
        last_wake_ms: None,
    }
}

fn judge(
    agents: &[AgentFacts],
    messages: &[MessageFacts],
    run_paused: bool,
    relays: &[RelayFact],
    child: &HashMap<String, u64>,
    now: i64,
) -> Evaluation {
    // Watched since long before: the window of a start is its own test.
    evaluate_pm_stuck(
        agents,
        messages,
        run_paused,
        relays,
        child,
        now,
        N,
        T0 - 1000 * MIN,
    )
}

fn found(evaluation: &Evaluation) -> Option<Reason> {
    evaluation.found.map(|(reason, _)| reason)
}

#[test]
fn unread_mail_holds_at_n_minutes_and_not_at_n_minus_one() {
    let agents = [pm("idle", T0, T0)];
    let messages = [mail("pm-1", T0, "queued", MessageClass::Action)];
    let none = HashMap::new();
    let early = judge(&agents, &messages, false, &[], &none, T0 + (N - 1) * MIN);
    assert_eq!(found(&early), None);
    let late = judge(&agents, &messages, false, &[], &none, T0 + N * MIN);
    assert_eq!(late.found, Some((Reason::UnreadMail, T0)));
    // Pulled and not acknowledged counts; an info message, an acknowledged one and a working PM do not.
    for (state, class, pm_state, expect) in [
        ("sent", MessageClass::Action, "idle", true),
        ("unacked", MessageClass::Action, "done", true),
        ("queued", MessageClass::Info, "idle", false),
        ("acked", MessageClass::Action, "idle", false),
        ("queued", MessageClass::Action, "working", false),
    ] {
        let evaluation = judge(
            &[pm(pm_state, T0, T0 + 10 * N * MIN)],
            &[mail("pm-1", T0, state, class)],
            false,
            &[],
            &none,
            T0 + N * MIN,
        );
        assert_eq!(
            found(&evaluation) == Some(Reason::UnreadMail),
            expect,
            "{state} {class:?} {pm_state}"
        );
    }
}

#[test]
fn no_progress_holds_at_n_minutes_and_not_at_n_minus_one() {
    let agents = [pm("working", T0, T0)];
    let none = HashMap::new();
    assert_eq!(
        found(&judge(&agents, &[], false, &[], &none, T0 + (N - 1) * MIN)),
        None
    );
    assert_eq!(
        found(&judge(&agents, &[], false, &[], &none, T0 + N * MIN)),
        Some(Reason::NoProgress)
    );
}

#[test]
fn a_recorded_wait_or_tool_activity_blocks_no_progress() {
    let mut waiting = pm("working", T0, T0);
    waiting.waits.push(WaitInterval {
        start_ms: T0,
        end_ms: Some(T0 + 10 * MIN),
    });
    let none = HashMap::new();
    // 20 minutes since the activity, 10 of them a recorded wait: 10 counted.
    assert_eq!(
        found(&judge(
            &[waiting.clone()],
            &[],
            false,
            &[],
            &none,
            T0 + 20 * MIN
        )),
        None
    );
    assert_eq!(
        found(&judge(
            &[waiting],
            &[],
            false,
            &[],
            &none,
            T0 + (10 + N) * MIN
        )),
        Some(Reason::NoProgress)
    );
    let agents = [pm("working", T0, T0)];
    let now = T0 + 20 * MIN;
    let recent = HashMap::from([("pm-1".to_string(), (now - (N - 1) * MIN) as u64)]);
    assert_eq!(found(&judge(&agents, &[], false, &[], &recent, now)), None);
    let old = HashMap::from([("pm-1".to_string(), (now - N * MIN) as u64)]);
    assert_eq!(
        found(&judge(&agents, &[], false, &[], &old, now)),
        Some(Reason::NoProgress)
    );
    // Another agent's activity does not count.
    let other = HashMap::from([("dev-1".to_string(), now as u64)]);
    assert_eq!(
        found(&judge(&agents, &[], false, &[], &other, now)),
        Some(Reason::NoProgress)
    );
}

#[test]
fn a_prompt_holds_at_n_minutes_blocked_or_with_an_open_relay() {
    let none = HashMap::new();
    let blocked = [pm("blocked", T0, T0)];
    assert_eq!(
        found(&judge(&blocked, &[], false, &[], &none, T0 + (N - 1) * MIN)),
        None
    );
    assert_eq!(
        judge(&blocked, &[], false, &[], &none, T0 + N * MIN).found,
        Some((Reason::Prompt, T0))
    );
    let idle = [pm("idle", T0, T0)];
    let relay = [RelayFact {
        agent_id: "pm-1".into(),
        captured_ms: T0,
    }];
    assert_eq!(
        found(&judge(&idle, &[], false, &relay, &none, T0 + (N - 1) * MIN)),
        None
    );
    assert_eq!(
        judge(&idle, &[], false, &relay, &none, T0 + N * MIN).found,
        Some((Reason::Prompt, T0))
    );
    // A relay of another agent is not the PM's.
    let other = [RelayFact {
        agent_id: "dev-1".into(),
        captured_ms: T0,
    }];
    assert_eq!(
        found(&judge(&idle, &[], false, &other, &none, T0 + N * MIN)),
        None
    );
}

#[test]
fn a_paused_pm_is_never_stuck() {
    let mut paused = pm("idle", T0, T0);
    paused.paused = true;
    let evaluation = judge(
        &[paused],
        &[mail("pm-1", T0, "queued", MessageClass::Action)],
        false,
        &[],
        &HashMap::new(),
        T0 + 2 * N * MIN,
    );
    assert!(evaluation.pm_paused);
    assert_eq!(evaluation.found, None);
}

#[test]
fn a_paused_run_stops_no_progress_only() {
    let none = HashMap::new();
    let now = T0 + 2 * N * MIN;
    let working = [pm("working", T0, T0)];
    assert_eq!(
        found(&judge(&working, &[], false, &[], &none, now)),
        Some(Reason::NoProgress)
    );
    assert_eq!(found(&judge(&working, &[], true, &[], &none, now)), None);
    let idle = [pm("idle", T0, T0)];
    let messages = [mail("pm-1", T0, "queued", MessageClass::Action)];
    assert_eq!(
        found(&judge(&idle, &messages, true, &[], &none, now)),
        Some(Reason::UnreadMail)
    );
    let blocked = [pm("blocked", T0, T0)];
    assert_eq!(
        found(&judge(&blocked, &[], true, &[], &none, now)),
        Some(Reason::Prompt)
    );
}

#[test]
fn without_one_pm_or_for_other_kinds_nothing_is_stuck() {
    let none = HashMap::new();
    let now = T0 + 2 * N * MIN;
    let messages = [
        mail("sup-1", T0, "queued", MessageClass::Action),
        mail("dev-1", T0, "queued", MessageClass::Action),
    ];
    let others = [
        worker("sup-1", "Supervisor", "idle"),
        worker("dev-1", "Developer", "blocked"),
    ];
    let relay = [RelayFact {
        agent_id: "dev-1".into(),
        captured_ms: T0,
    }];
    let evaluation = judge(&others, &messages, false, &relay, &none, now);
    assert_eq!(evaluation.pm_id, None);
    assert_eq!(evaluation.found, None);
    // Two PMs are no PM.
    let two = [pm("idle", T0, T0), worker("pm-2", "PM", "idle")];
    let evaluation = judge(
        &two,
        &[mail("pm-1", T0, "queued", MessageClass::Action)],
        false,
        &[],
        &none,
        now,
    );
    assert_eq!(evaluation.pm_id, None);
    assert_eq!(evaluation.found, None);
}

#[test]
fn the_first_reason_in_order_wins() {
    let none = HashMap::new();
    let now = T0 + 2 * N * MIN;
    let relay = [RelayFact {
        agent_id: "pm-1".into(),
        captured_ms: T0,
    }];
    let messages = [mail("pm-1", T0, "queued", MessageClass::Action)];
    // Idle with mail and an open relay: unread_mail before prompt.
    assert_eq!(
        found(&judge(
            &[pm("idle", T0, T0)],
            &messages,
            false,
            &relay,
            &none,
            now
        )),
        Some(Reason::UnreadMail)
    );
    // Working with an open relay and no mail: no_progress before prompt.
    assert_eq!(
        found(&judge(
            &[pm("working", T0, T0)],
            &[],
            false,
            &relay,
            &none,
            now
        )),
        Some(Reason::NoProgress)
    );
    // Working with the run paused: only the relay is left.
    assert_eq!(
        found(&judge(
            &[pm("working", T0, T0)],
            &[],
            true,
            &relay,
            &none,
            now
        )),
        Some(Reason::Prompt)
    );
}

#[test]
fn no_progress_needs_n_minutes_of_watching() {
    let agents = [pm("working", T0, T0)];
    let none = HashMap::new();
    let now = T0 + 2 * N * MIN;
    let started = now - (N - 1) * MIN;
    let early = evaluate_pm_stuck(&agents, &[], false, &[], &none, now, N, started);
    assert!(!early.watching);
    assert_eq!(early.found, None);
    let late = evaluate_pm_stuck(&agents, &[], false, &[], &none, now, N, now - N * MIN);
    assert!(late.watching);
    assert_eq!(found(&late), Some(Reason::NoProgress));
}

// -------------------------------------------------------------------------------------------------------- the episodes

/// The one `pm.recovered` event of `events`, checked field by field (the golden fixture is the feed test's).
fn recovered_of(rig: &Rig, events: &[Value]) -> Value {
    for event in events {
        check_event(event, &project_of(rig));
    }
    let found = events_of(events, "pm.recovered");
    assert_eq!(found.len(), 1, "{events:#?}");
    found[0].clone()
}

fn tick(rig: &Rig, now: i64) -> Value {
    tick_with(rig, now, &HashMap::new())
}

fn tick_with(rig: &Rig, now: i64, child: &HashMap<String, u64>) -> Value {
    rig.core
        .pm_stuck_tick(&rig.ctx("owner"), child, now, N)
        .unwrap()
}

/// pm-1 idle with an action message that has waited since the epoch.
fn idle_with_mail(rig: &Rig) -> String {
    rig.observe("pm-1", "idle");
    rig.enqueue("pm-1", "please answer", true)
}

fn episodes(rig: &Rig) -> Vec<(String, String, Option<String>, i64)> {
    let db = rig.db();
    let mut statement = db
        .prepare("SELECT agent_id, reason, close_reason, alert FROM pm_stuck_episodes ORDER BY opened_ms, episode_id")
        .unwrap();
    statement
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn clear_since(rig: &Rig) -> Option<i64> {
    rig.db()
        .query_row(
            "SELECT clear_since_ms FROM pm_stuck_episodes WHERE closed_at IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

/// Acknowledges the mail of pm-1, which clears `unread_mail`.
fn read_the_mail(rig: &Rig, message_id: &str) {
    rig.core.pull_pending(&rig.cred("pm-1")).unwrap();
    rig.core.ack_message(&rig.ctx("pm-1"), message_id).unwrap();
}

#[test]
fn one_pm_stuck_event_is_written_across_many_ticks() {
    let rig = Rig::new();
    idle_with_mail(&rig);
    let head = rig.head();
    assert_eq!(tick(&rig, T0 + (N - 1) * MIN)["written"], 0);
    assert_eq!(rig.head(), head);
    assert_eq!(tick(&rig, T0 + N * MIN)["written"], 1);
    let opened = rig.head();
    assert_eq!(opened, head + 1);
    let version = rig.core.state_version().unwrap();
    for step in 1..=5 {
        tick(&rig, T0 + N * MIN + step * 10_000);
    }
    assert_eq!(rig.head(), opened, "an open episode writes nothing");
    assert_eq!(rig.core.state_version().unwrap(), version);
    let events = rig.since(head);
    let stuck = events_of(&events, "pm.stuck");
    assert_eq!(stuck.len(), 1, "{events:#?}");
    let body = &stuck[0]["body"];
    assert_eq!(body["agentId"], "pm-1");
    assert_eq!(body["reason"], "unread_mail");
    assert_eq!(body["minutes"], N);
    assert_eq!(body["alert"], true);
    assert!(body["episodeId"].as_str().unwrap().starts_with("stuck-"));
    assert_eq!(stuck[0]["entity"], json!({"type": "agent", "id": "pm-1"}));
}

#[test]
fn the_episode_recovers_after_sixty_seconds_clear() {
    let rig = Rig::new();
    let mail = idle_with_mail(&rig);
    let opened_at = T0 + N * MIN;
    tick(&rig, opened_at);
    read_the_mail(&rig, &mail);
    let head = rig.head();
    // The first clear tick starts the timer and writes no ledger row.
    let clear_at = opened_at + 10_000;
    tick(&rig, clear_at);
    assert_eq!(clear_since(&rig), Some(clear_at));
    tick(&rig, clear_at + CLEAR_MS - 1);
    assert_eq!(rig.head(), head);
    assert_eq!(tick(&rig, clear_at + CLEAR_MS)["written"], 1);
    let events = rig.since(head);
    let recovered = recovered_of(&rig, &events);
    assert_eq!(recovered["body"]["agentId"], "pm-1");
    assert_eq!(recovered["body"]["reason"], "recovered");
    assert_eq!(
        episodes(&rig),
        vec![(
            "pm-1".into(),
            "unread_mail".into(),
            Some("recovered".into()),
            1
        )]
    );
    // Nothing more once closed.
    let head = rig.head();
    tick(&rig, clear_at + 2 * CLEAR_MS);
    assert_eq!(rig.head(), head);
}

#[test]
fn a_condition_returning_inside_the_window_resets_the_timer() {
    let rig = Rig::new();
    rig.observe("pm-1", "blocked");
    let opened_at = T0 + N * MIN;
    tick(&rig, opened_at);
    assert_eq!(episodes(&rig).len(), 1);
    // The PM unblocks: the condition is clear.
    rig.observe("pm-1", "idle");
    tick(&rig, opened_at + 1000);
    assert_eq!(clear_since(&rig), Some(opened_at + 1000));
    // Blocked again for the same stretch (the observation is later, so the block is shorter than N): use an open relay.
    rig.db()
        .execute(
            "UPDATE pm_stuck_episodes SET clear_since_ms = ?1 WHERE closed_at IS NULL",
            [opened_at + 1000],
        )
        .unwrap();
    rig.observe("pm-1", "blocked");
    // The blocked state began after the epoch by a few ms: it has lasted N minutes by the time of this tick.
    tick(&rig, opened_at + 30_000);
    assert_eq!(clear_since(&rig), None, "the timer was reset");
    // Clear again: a full 60 seconds from the new start are needed.
    rig.observe("pm-1", "idle");
    tick(&rig, opened_at + 40_000);
    tick(&rig, opened_at + 40_000 + CLEAR_MS - 1);
    assert_eq!(episodes(&rig)[0].2, None);
    tick(&rig, opened_at + 40_000 + CLEAR_MS);
    assert_eq!(episodes(&rig)[0].2.as_deref(), Some("recovered"));
}

#[test]
fn a_tick_slower_than_sixty_seconds_clears_in_one_tick() {
    let rig = Rig::new();
    let mail = idle_with_mail(&rig);
    tick(&rig, T0 + N * MIN);
    read_the_mail(&rig, &mail);
    tick(&rig, T0 + N * MIN + 1000);
    // The next tick comes ten minutes later: one tick closes it.
    assert_eq!(tick(&rig, T0 + (N + 10) * MIN)["written"], 1);
    assert_eq!(episodes(&rig)[0].2.as_deref(), Some("recovered"));
}

#[test]
fn a_paused_pm_closes_its_episode_as_paused() {
    let rig = Rig::new();
    idle_with_mail(&rig);
    tick(&rig, T0 + N * MIN);
    rig.core
        .pause_agent(
            &rig.ctx("owner"),
            &json!({"agentId": "pm-1", "reason": "hold"}),
        )
        .unwrap();
    let head = rig.head();
    tick(&rig, T0 + N * MIN + 1000);
    let events = rig.since(head);
    let recovered = recovered_of(&rig, &events);
    assert_eq!(recovered["body"]["reason"], "paused");
    // A paused PM raises nothing.
    let head = rig.head();
    tick(&rig, T0 + 3 * N * MIN);
    assert_eq!(rig.head(), head);
}

#[test]
fn a_replaced_pm_closes_as_replaced_and_the_new_agent_alerts_after_its_own_minutes() {
    let mut rig = Rig::new();
    idle_with_mail(&rig);
    tick(&rig, T0 + N * MIN);
    rig.core.end_agent(&rig.ctx("owner"), "pm-1", None).unwrap();
    rig.add_agent("pm-2", "pm", "PM");
    rig.observe("pm-2", "idle");
    let head = rig.head();
    tick(&rig, T0 + N * MIN + 5000);
    let events = rig.since(head);
    let recovered = events_of(&events, "pm.recovered");
    assert_eq!(recovered.len(), 1, "{events:#?}");
    assert_eq!(recovered[0]["body"]["agentId"], "pm-1");
    assert_eq!(recovered[0]["body"]["reason"], "replaced");
    // pm-2 has no mail and is idle: nothing is raised for it.
    assert_eq!(episodes(&rig).len(), 1);
    // Mail to pm-2, queued after the epoch: its own N minutes start at its queue time.
    let queued_at: i64 = {
        let id = rig.enqueue("pm-2", "you are the PM now", true);
        rig.db()
            .query_row(
                "SELECT CAST(strftime('%s', queued_at) AS INTEGER) * 1000 FROM messages WHERE message_id = ?1",
                [id],
                |row| row.get(0),
            )
            .unwrap()
    };
    let base = queued_at.max(T0);
    assert_eq!(tick(&rig, base + (N - 1) * MIN)["written"], 0);
    assert_eq!(tick(&rig, base + N * MIN + 1000)["written"], 1);
    let rows = episodes(&rig);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].0, "pm-2");
    assert_eq!(rows[1].3, 1, "a new agent id alerts");
}

#[test]
fn an_episode_with_no_pm_left_closes_as_ended() {
    let rig = Rig::new();
    idle_with_mail(&rig);
    tick(&rig, T0 + N * MIN);
    rig.core.end_agent(&rig.ctx("owner"), "pm-1", None).unwrap();
    let head = rig.head();
    tick(&rig, T0 + N * MIN + 1000);
    let events = rig.since(head);
    let recovered = recovered_of(&rig, &events);
    assert_eq!(recovered["body"]["reason"], "ended");
}

#[test]
fn the_same_agent_id_does_not_alert_again_within_thirty_minutes() {
    let rig = Rig::new();
    let mail = idle_with_mail(&rig);
    let first = T0 + N * MIN;
    tick(&rig, first);
    read_the_mail(&rig, &mail);
    tick(&rig, first + 1000);
    tick(&rig, first + 1000 + CLEAR_MS);
    assert_eq!(episodes(&rig).len(), 1);
    // More mail waits: a second episode of the same agent id, 10 minutes after the first opened.
    let second_mail = rig.enqueue("pm-1", "again", true);
    let _ = second_mail;
    let head = rig.head();
    let second = first + 10 * MIN;
    // Its queue time is near the epoch, so it is old enough at once.
    tick(&rig, second);
    let rows = episodes(&rig);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[1].3, 0, "alert is false inside 30 minutes");
    let events = rig.since(head);
    let stuck = events_of(&events, "pm.stuck");
    assert_eq!(stuck[0]["body"]["alert"], false);
    // After 30 minutes the suppression is over.
    let mail = rig.enqueue("pm-1", "third", true);
    let _ = mail;
    let third_check = first + SUPPRESS_MS + MIN;
    // Close the second episode first (the mail is read), then raise a third.
    rig.core.pull_pending(&rig.cred("pm-1")).unwrap();
    for id in open_message_ids(&rig) {
        rig.core.ack_message(&rig.ctx("pm-1"), &id).unwrap();
    }
    tick(&rig, second + 1000);
    tick(&rig, second + 1000 + CLEAR_MS);
    assert_eq!(episodes(&rig)[1].2.as_deref(), Some("recovered"));
    rig.enqueue("pm-1", "fourth", true);
    tick(&rig, third_check);
    let rows = episodes(&rig);
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert_eq!(
        rows[2].3, 1,
        "the first alerting episode opened more than 30 minutes ago"
    );
}

fn open_message_ids(rig: &Rig) -> Vec<String> {
    let db = rig.db();
    let mut statement = db
        .prepare("SELECT message_id FROM messages WHERE recipient_agent_id = 'pm-1' AND state IN ('sent', 'unacked')")
        .unwrap();
    statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn a_prompt_relay_open_for_n_minutes_raises_a_prompt_episode() {
    let rig = Rig::new();
    rig.observe("pm-1", "idle");
    let (project, actor): (String, String) = rig
        .db()
        .query_row(
            "SELECT project_id, actor_id FROM agents WHERE agent_id = 'pm-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    rig.db()
        .execute(
            "INSERT INTO prompt_relays(project_id, relay_id, sequence, agent_id, pane_id, host_kind, prompt_text, options_json, prompt_sha,
               captured_by_actor_id, captured_at, expires_at, state)
             VALUES (?1, 'r-1', 1, 'pm-1', 'p1', 'claude', 'Allow?', '[]', ?2, ?3, '2026-01-01T00:00:00.000Z', '2030-01-01T00:00:00.000Z', 'captured')",
            rusqlite::params![project, "a".repeat(64), actor],
        )
        .unwrap();
    // The relay was captured at the epoch (2026-01-01T00:00:00Z).
    assert_eq!(tick(&rig, T0 + (N - 1) * MIN)["written"], 0);
    assert_eq!(tick(&rig, T0 + N * MIN)["written"], 1);
    assert_eq!(episodes(&rig)[0].1, "prompt");
}

// ------------------------------------------------------------------------------------------------------------ restarts

#[test]
fn a_restart_with_an_open_episode_writes_nothing_and_closes_it_sixty_seconds_after_clear_since() {
    let mut rig = Rig::new();
    let mail = idle_with_mail(&rig);
    tick(&rig, T0 + N * MIN);
    read_the_mail(&rig, &mail);
    let clear_at = T0 + N * MIN + 5000;
    tick(&rig, clear_at);
    rig.reopen("after-restart");
    let head = rig.head();
    // The new core has not watched yet; unread_mail and prompt do not need it, and the clear timer is in the row.
    assert_eq!(tick(&rig, clear_at + 1000)["written"], 0);
    assert_eq!(rig.head(), head);
    assert_eq!(clear_since(&rig), Some(clear_at));
    assert_eq!(tick(&rig, clear_at + CLEAR_MS)["written"], 1);
    assert_eq!(episodes(&rig)[0].2.as_deref(), Some("recovered"));
}

#[test]
fn a_restart_raises_unread_mail_and_prompt_from_ledger_times_and_no_progress_only_after_watching() {
    let mut rig = Rig::new();
    idle_with_mail(&rig);
    rig.reopen("after-restart");
    // Unread mail: the ledger says it has waited since the epoch.
    assert_eq!(tick(&rig, T0 + N * MIN)["written"], 1);
    assert_eq!(episodes(&rig)[0].1, "unread_mail");

    let mut rig = Rig::new();
    rig.observe("pm-1", "working");
    rig.reopen("after-restart-2");
    // The first tick of the new core starts the watching; N minutes later no_progress may be raised.
    let first = T0 + 3 * N * MIN;
    assert_eq!(tick(&rig, first)["written"], 0);
    assert_eq!(tick(&rig, first + (N - 1) * MIN)["written"], 0);
    assert_eq!(tick(&rig, first + N * MIN)["written"], 1);
    assert_eq!(episodes(&rig)[0].1, "no_progress");
}

#[test]
fn a_restart_during_the_watching_window_keeps_an_open_no_progress_episode_open() {
    let mut rig = Rig::new();
    rig.observe("pm-1", "working");
    // Watching since long before: the episode opens.
    tick(&rig, T0 - 100 * MIN);
    let opened = T0 + 3 * N * MIN;
    assert_eq!(tick(&rig, opened)["written"], 1);
    assert_eq!(episodes(&rig)[0].1, "no_progress");
    // The PM did something: the condition would be clear — but a restart begins a window in which it cannot be judged.
    rig.reopen("after-restart");
    rig.db()
        .execute(
            "UPDATE agents SET last_activity_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+1 day') WHERE agent_id = 'pm-1'",
            [],
        )
        .ok();
    let head = rig.head();
    let window = opened + MIN;
    for step in 0..(N - 1) {
        assert_eq!(tick(&rig, window + step * MIN)["written"], 0);
    }
    assert_eq!(rig.head(), head, "no row during the window");
    assert_eq!(episodes(&rig)[0].2, None);
    assert_eq!(clear_since(&rig), None, "the clear timer did not start");
    // No second episode either.
    assert_eq!(episodes(&rig).len(), 1);
}

// -------------------------------------------------------------------------------------------- migration, card, feed

#[test]
fn the_migration_applies_to_a_database_with_agents_and_messages() {
    let rig = Rig::new();
    rig.enqueue("dev-1", "hello", true);
    let db = rig.db();
    let tables: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'pm_stuck_episodes'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tables, 1);
    let indexes: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'pm_stuck_episodes_one_open_per_agent'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(indexes, 1);
    // The migration file is found by name, not by number.
    let found = std::fs::read_dir(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../migrations"),
    )
    .unwrap()
    .any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with("_pm_stuck_episodes.sql")
    });
    assert!(found);
}

#[test]
fn the_card_shows_the_open_episode_alert_false_included_and_clears_with_it() {
    let rig = Rig::new();
    let mail = idle_with_mail(&rig);
    let card =
        |rig: &Rig| snapshot_item(&rig.core, "run", "run").unwrap().unwrap()["pm"]["stuck"].clone();
    assert_eq!(card(&rig), Value::Null);
    let head = rig.head();
    tick(&rig, T0 + N * MIN);
    let events = rig.since(head);
    // The row's event carries the run effect last.
    let stuck = events_of(&events, "pm.stuck")[0];
    let effects = stuck["body"]["effects"].as_array().unwrap();
    assert_eq!(
        effects.last().unwrap(),
        &json!({"section": "run", "op": "invalidate", "key": "run", "fields": {}})
    );
    let shown = card(&rig);
    assert_eq!(shown["reason"], "unread_mail");
    assert_eq!(shown["alert"], true);
    assert_eq!(shown["episodeId"], stuck["body"]["episodeId"]);
    assert_eq!(shown["since"], stuck["body"]["since"]);
    read_the_mail(&rig, &mail);
    tick(&rig, T0 + N * MIN + 1000);
    let head = rig.head();
    tick(&rig, T0 + N * MIN + 1000 + CLEAR_MS);
    assert_eq!(card(&rig), Value::Null);
    let recovered = events_of(&rig.since(head), "pm.recovered").len();
    assert_eq!(recovered, 1);
    // An alert-false episode is shown too.
    rig.enqueue("pm-1", "again", true);
    tick(&rig, T0 + N * MIN + 2 * CLEAR_MS);
    let shown = card(&rig);
    assert_eq!(shown["alert"], false);
}

#[test]
fn recent_lists_pm_stuck_and_not_pm_recovered() {
    let rig = Rig::new();
    let mail = idle_with_mail(&rig);
    tick(&rig, T0 + N * MIN);
    read_the_mail(&rig, &mail);
    tick(&rig, T0 + N * MIN + 1000);
    tick(&rig, T0 + N * MIN + 1000 + CLEAR_MS);
    let head = rig.head();
    let epoch = capstan_kernel::areas::event_feed::feed_head(&rig.core)
        .unwrap()
        .epoch;
    let page = recent_page(&rig.core, &epoch, head, 20).unwrap();
    let events = match page {
        capstan_kernel::areas::event_feed::RecentPage::Page(p) => p.events,
        other => panic!("{other:?}"),
    };
    let types: Vec<&str> = events.iter().map(|e| e["type"].as_str().unwrap()).collect();
    assert!(types.contains(&"pm.stuck"), "{types:?}");
    assert!(!types.contains(&"pm.recovered"), "{types:?}");
    let _ = feed_page;
    let _ = Position::default();
}

// -------------------------------------------------------------------------------------------------------------- push

#[test]
fn only_an_alerting_pm_stuck_is_pushed_and_with_ids_only() {
    let rig = Rig::new();
    let mail = idle_with_mail(&rig);
    let before = rig.head();
    tick(&rig, T0 + N * MIN);
    let page = push_events_after(&rig.core, before, 200).unwrap();
    assert_eq!(page.events.len(), 1, "{:?}", page.events);
    let event = &page.events[0];
    assert_eq!(event.event, NtfyEvent::PmStuck);
    assert_eq!(event.agent_id.as_deref(), Some("pm-1"));
    assert_eq!(event.detail.as_deref(), Some("unread_mail:15"));
    read_the_mail(&rig, &mail);
    tick(&rig, T0 + N * MIN + 1000);
    let after_open = rig.head();
    tick(&rig, T0 + N * MIN + 1000 + CLEAR_MS);
    // pm.recovered is never pushed.
    let page = push_events_after(&rig.core, after_open, 200).unwrap();
    assert!(page.events.is_empty(), "{:?}", page.events);
    // An alert-false episode is not pushed either.
    rig.enqueue("pm-1", "again", true);
    let before = rig.head();
    tick(&rig, T0 + N * MIN + 2 * CLEAR_MS);
    assert_eq!(episodes(&rig)[1].3, 0);
    let page = push_events_after(&rig.core, before, 200).unwrap();
    assert!(page.events.is_empty(), "{:?}", page.events);
}
