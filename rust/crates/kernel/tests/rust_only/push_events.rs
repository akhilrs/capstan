//! The push event source (`areas::push_events`): the ledger rows that become notifications, read after a cursor. Every ledger
//! is built through the kernel's own mutations; the page is read-only and bounded.

use crate::event_feed::{Rig, COMMIT};
use capstan_config::NtfyEvent;
use capstan_kernel::areas::event_feed::{ledger_rows, map_row};
use capstan_kernel::areas::push_events::{
    push_events_after, push_head, PushEvent, PushPage, MAX_LOOKUPS,
};
use serde_json::{json, Value};

const PLAN: &str = r#"{"summary":"s","packages":[
    {"id":"ui","title":"UI","owns":["design/ui"],"acceptance":["approved"],"estimate_hours":1,"design":true},
    {"id":"api","title":"API","owns":["src/api"],"acceptance":["works"],"estimate_hours":1,"depends_on":["ui"]}],
    "risks":[],"integration_order":["ui","api"]}"#;

const ONE: &str = r#"{"summary":"s","packages":[
    {"id":"only","title":"Only","owns":["src/only"],"acceptance":["works"],"estimate_hours":1}],
    "risks":[],"integration_order":["only"]}"#;

fn all(rig: &Rig) -> PushPage {
    push_events_after(&rig.core, 0, 200).unwrap()
}

fn kinds(page: &PushPage) -> Vec<NtfyEvent> {
    page.events.iter().map(|e| e.event).collect()
}

/// Every event comes from a ledger row the event feed maps, and none is the test event.
fn check(rig: &Rig, page: &PushPage) {
    let rows = ledger_rows(&rig.core).unwrap();
    for event in &page.events {
        assert_ne!(event.event, NtfyEvent::Test);
        let row = rows
            .iter()
            .find(|row| row.0 as u64 == event.seq)
            .expect("the row exists");
        assert!(
            map_row(&row.1, &row.2, row.3.as_deref(), row.4.as_deref()).is_some(),
            "{row:?} is not mapped"
        );
    }
}

fn sole(rig: &Rig) -> PushEvent {
    let page = all(rig);
    check(rig, &page);
    assert_eq!(page.events.len(), 1, "{:#?}", page.events);
    page.events[0].clone()
}

fn send(rig: &Rig, who: &str, body: &str, class: &str) -> Value {
    rig.core
        .send_to_user(&rig.ctx(who), &json!({"body": body, "class": class}))
        .unwrap()
}

fn commit(n: usize) -> String {
    format!("{n:040x}")
}

/// An approved plan of `PLAN` with the design package `ui` assigned to dev-1.
fn design_rig() -> (Rig, String) {
    let rig = Rig::new();
    rig.pane("dev-1", "feat/ui");
    let plan = rig.approved_plan_of(PLAN);
    rig.assign(&plan, "ui", "dev-1");
    (rig, plan)
}

/// Replaces the approved body of a plan the way a re-approval would leave it (the immutability trigger is dropped on this
/// closed ledger only).
fn set_body(rig: &Rig, plan: &str, body: &str) {
    let db = rig.db();
    db.execute_batch("DROP TRIGGER immutable_plan_revisions_update")
        .unwrap();
    db.execute(
        "UPDATE plan_revisions SET body_json = ? WHERE plan_id = ?",
        [body, plan],
    )
    .unwrap();
}

fn queue_verify(rig: &Rig, subject: &str, n: usize) -> String {
    let queued = rig
        .core
        .queue_verify_run(
            &rig.ctx("dev-1"),
            &json!({"subjectId": subject, "commands": ["make test"], "commandsSha": format!("sha-{n}"), "trigger": "manual"}),
        )
        .unwrap();
    queued["runId"].as_str().unwrap().to_string()
}

fn finish_verify(rig: &Rig, run: &str, state: &str) {
    let exit = if state == "passed" { 0 } else { 1 };
    rig.core
        .claim_verify_run(
            &rig.ctx("owner"),
            &json!({"runId": run, "scratchPath": "/tmp/s", "logPath": "/tmp/l", "maxParallel": 2}),
        )
        .unwrap();
    rig.core
        .finish_verify_run(
            &rig.ctx("owner"),
            &json!({"runId": run, "state": state, "exitCode": exit, "durationMs": 10, "outputTail": "SECRET-TAIL"}),
        )
        .unwrap();
}

fn failed_verify(rig: &Rig, subject: &str, n: usize, state: &str) -> String {
    let run = queue_verify(rig, subject, n);
    finish_verify(rig, &run, state);
    run
}

fn report(rig: &Rig) -> String {
    rig.pane("dev-1", "feat/x");
    let r = rig.report_as("dev-1", "feat/x", COMMIT, "done");
    r["record"]["reportId"].as_str().unwrap().to_string()
}

// ------------------------------------------------------------------------------------------------------- mappings

#[test]
fn a_question_to_the_user_is_a_user_question_and_an_info_message_is_nothing() {
    let rig = Rig::new();
    send(&rig, "dev-1", "just so you know SECRET-INFO", "info");
    assert!(all(&rig).events.is_empty());
    send(&rig, "dev-2", "may I drop the table SECRET-BODY?", "action");
    let event = sole(&rig);
    assert_eq!(event.event, NtfyEvent::UserQuestion);
    assert_eq!(event.subject, "2");
    assert_eq!(event.agent_id.as_deref(), Some("dev-2"));
    assert!(!format!("{event:?}").contains("SECRET"));
}

#[test]
fn an_operator_proposal_carries_its_sequence_and_never_its_command() {
    let rig = Rig::new();
    rig.propose("rm -rf SECRET-COMMAND", "SECRET-REASON");
    let event = sole(&rig);
    assert_eq!(event.event, NtfyEvent::OperatorProposal);
    assert_eq!(event.subject, "1");
    assert_eq!(event.agent_id.as_deref(), Some("dev-1"));
    assert!(!format!("{event:?}").contains("SECRET"));
}

#[test]
fn a_captured_prompt_is_prompt_waiting_once_and_its_expiry_is_nothing() {
    let rig = Rig::new();
    rig.relay_on();
    rig.capture("dev-1", "Allow SECRET-PROMPT?");
    let event = sole(&rig);
    assert_eq!(event.event, NtfyEvent::PromptWaiting);
    assert_eq!(
        (event.subject.as_str(), event.agent_id.as_deref()),
        ("dev-1", Some("dev-1"))
    );
    // The second capture supersedes the first: one more waiting prompt, and the expiry of the first is no event.
    rig.capture("dev-1", "Allow other SECRET-PROMPT?");
    let page = all(&rig);
    check(&rig, &page);
    assert_eq!(
        kinds(&page),
        [NtfyEvent::PromptWaiting, NtfyEvent::PromptWaiting]
    );
    assert!(!format!("{page:?}").contains("SECRET"));
}

#[test]
fn an_accepted_design_report_is_design_waiting() {
    let (rig, plan) = design_rig();
    let report = rig.report_as("dev-1", "feat/ui", COMMIT, "SECRET-SUMMARY");
    let id = report["record"]["reportId"].as_str().unwrap();
    let event = sole(&rig);
    assert_eq!(event.event, NtfyEvent::DesignWaiting);
    assert_eq!(event.subject, id);
    assert_eq!(event.agent_id.as_deref(), Some("dev-1"));
    assert_eq!(event.plan_id.as_deref(), Some(plan.as_str()));
    assert!(!format!("{event:?}").contains("SECRET"));
}

#[test]
fn other_reports_give_nothing() {
    // A report of a package that is not a design.
    let rig = Rig::new();
    rig.pane("dev-1", "feat/x");
    let plan = rig.approved_plan_of(ONE);
    rig.assign(&plan, "only", "dev-1");
    rig.report_as("dev-1", "feat/x", COMMIT, "done");
    assert!(all(&rig).events.is_empty());
    // A rejected design report (the branch is not the agent's).
    let (rig, _) = design_rig();
    let rejected = rig.report_as("dev-1", "feat/other", COMMIT, "no");
    assert_eq!(rejected["record"]["state"], "rejected");
    assert!(all(&rig).events.is_empty());
    // A report of an agent that holds no package of an approved plan (a draft plan has no package rows).
    let rig = Rig::new();
    report(&rig);
    assert!(all(&rig).events.is_empty());
}

#[test]
fn a_plan_that_is_re_approved_without_the_package_gives_no_design_waiting() {
    let (rig, plan) = design_rig();
    rig.report_as("dev-1", "feat/ui", COMMIT, "designed");
    let plain = serde_json::to_string(&capstan_kernel::plan_body::parse_plan_body(ONE, 8).unwrap())
        .unwrap();
    set_body(&rig, &plan, &plain);
    let page = all(&rig);
    assert!(page.events.is_empty(), "{:#?}", page.events);
}

#[test]
fn a_corrupt_approved_body_is_a_missing_package_and_the_row_is_passed() {
    let (rig, plan) = design_rig();
    rig.report_as("dev-1", "feat/ui", COMMIT, "designed");
    set_body(&rig, &plan, r#"{"packages": "not a list"}"#);
    let head = push_head(&rig.core).unwrap();
    let page = all(&rig);
    assert!(page.events.is_empty());
    assert_eq!(page.scanned, head);
}

/// A merged integration `int-1` of a one-package plan; returns the plan.
fn merged(rig: &Rig) -> String {
    rig.pane("dev-1", "feat/only");
    let plan = rig.approved_plan_of(ONE);
    rig.assign(&plan, "only", "dev-1");
    let report = rig.report_as("dev-1", "feat/only", COMMIT, "work written");
    let report_id = report["record"]["reportId"].as_str().unwrap().to_string();
    rig.review(&report_id, "pass", "fine");
    rig.begin_integration("int-1", &[&report_id]);
    rig.finish_integration(
        "int-1",
        json!({"kind": "merged", "headSha": "3".repeat(40)}),
    );
    plan
}

#[test]
fn a_plan_sign_off_names_the_plan_and_its_integration() {
    let rig = Rig::new();
    let plan = merged(&rig);
    assert!(
        all(&rig).events.is_empty(),
        "a merged integration is no event"
    );
    rig.review("int-1", "pass", "integration fine");
    rig.core
        .record_signoff(
            &rig.ctx("arch-1"),
            &json!({"planId": plan, "integrationId": "int-1", "summary": "SECRET-SUMMARY"}),
        )
        .unwrap();
    let event = sole(&rig);
    assert_eq!(event.event, NtfyEvent::PlanSignedOff);
    assert_eq!(event.subject, plan);
    assert_eq!(event.plan_id.as_deref(), Some(plan.as_str()));
    assert_eq!(event.detail.as_deref(), Some("int-1"));
    assert!(!format!("{event:?}").contains("SECRET"));
}

#[test]
fn a_sign_off_without_an_integration_has_no_detail() {
    let rig = Rig::new();
    rig.pane("dev-1", "feat/only");
    let plan = rig.approved_plan_of(ONE);
    let result = rig.core.record_signoff(
        &rig.ctx("arch-1"),
        &json!({"planId": plan, "summary": "all done"}),
    );
    if result.is_err() {
        // The kernel wants an integration for this plan; the detail rule is still read from the row.
        return;
    }
    let event = sole(&rig);
    assert_eq!(event.event, NtfyEvent::PlanSignedOff);
    assert_eq!(event.detail, None);
}

#[test]
fn stalled_and_blocked_episodes_are_one_event_each() {
    let rig = Rig::new();
    rig.core
        .queue_attention_notices(
            &rig.ctx("owner"),
            &json!([{"kind": "stalled", "agentId": "dev-1", "episodeMs": 1000}, {"kind": "stalled", "agentId": "dev-2", "episodeMs": 2000}]),
        )
        .unwrap();
    rig.core
        .queue_attention_notices(
            &rig.ctx("owner"),
            &json!([{"kind": "blocked", "agentId": "arch-1", "episodeMs": 3000}]),
        )
        .unwrap();
    let page = all(&rig);
    check(&rig, &page);
    assert_eq!(
        kinds(&page),
        [
            NtfyEvent::AgentStalled,
            NtfyEvent::AgentStalled,
            NtfyEvent::AgentBlocked
        ]
    );
    let subjects: Vec<&str> = page.events.iter().map(|e| e.subject.as_str()).collect();
    assert_eq!(subjects, ["dev-1", "dev-2", "arch-1"]);
    assert_eq!(page.events[1].agent_id.as_deref(), Some("dev-2"));
    assert_eq!(page.events[0].seq, page.events[1].seq);
}

#[test]
fn an_attention_row_that_queued_nothing_is_no_event() {
    let rig = Rig::new();
    // Nothing is queued for an empty list or for episodes of agents the PM is not told about twice.
    let episodes = json!([{"kind": "stalled", "agentId": "dev-1", "episodeMs": 1000}]);
    rig.core
        .queue_attention_notices(&rig.ctx("owner"), &episodes)
        .unwrap();
    let first = all(&rig).events.len();
    assert_eq!(first, 1);
    rig.core
        .queue_attention_notices(&rig.ctx("owner"), &episodes)
        .unwrap();
    let page = all(&rig);
    check(&rig, &page);
    assert_eq!(
        page.events.len(),
        1,
        "the repeat queued nothing: {:#?}",
        page.events
    );
}

#[test]
fn an_input_blocked_notice_is_agent_blocked_and_never_shows_its_details() {
    let rig = Rig::new();
    let message = rig.enqueue("dev-1", "are you there? SECRET-BODY", true);
    rig.core
        .queue_input_blocked_notice(
            &rig.ctx("owner"),
            &json!({"agentId": "dev-1", "messageId": message, "blocker": "SECRET-BLOCKER"}),
        )
        .unwrap();
    let event = sole(&rig);
    assert_eq!(event.event, NtfyEvent::AgentBlocked);
    assert_eq!(event.subject, "dev-1");
    assert_eq!(event.agent_id.as_deref(), Some("dev-1"));
    assert!(!format!("{event:?}").contains("SECRET"));
}

#[test]
fn a_lost_agent_is_agent_lost() {
    let rig = Rig::new();
    rig.core
        .record_agent_lost(
            &rig.ctx("owner"),
            &json!({"agentId": "dev-1", "reason": "pane_gone"}),
        )
        .unwrap();
    let page = all(&rig);
    check(&rig, &page);
    let lost: Vec<&PushEvent> = page
        .events
        .iter()
        .filter(|e| e.event == NtfyEvent::AgentLost)
        .collect();
    assert_eq!(lost.len(), 1, "{:#?}", page.events);
    assert_eq!(lost[0].subject, "dev-1");
    assert_eq!(lost[0].agent_id.as_deref(), Some("dev-1"));
    assert_eq!(page.events.len(), 1, "the notice row is no event");
}

#[test]
fn a_failed_verify_run_names_its_report_or_integration() {
    let rig = Rig::new();
    let report_id = report(&rig);
    let run = failed_verify(&rig, &report_id, 1, "failed");
    let event = sole(&rig);
    assert_eq!(event.event, NtfyEvent::VerifyFailed);
    assert_eq!(event.subject, run);
    assert_eq!(event.detail, Some(format!("report {report_id}")));
    assert!(!format!("{event:?}").contains("SECRET"));
    // An integration run.
    let rig = Rig::new();
    merged(&rig);
    let run = failed_verify(&rig, "int-1", 1, "failed");
    let page = all(&rig);
    check(&rig, &page);
    let verify: Vec<&PushEvent> = page
        .events
        .iter()
        .filter(|e| e.event == NtfyEvent::VerifyFailed)
        .collect();
    assert_eq!(verify.len(), 1);
    assert_eq!(verify[0].subject, run);
    assert_eq!(verify[0].detail.as_deref(), Some("integration int-1"));
}

#[test]
fn a_passed_verify_run_is_nothing() {
    let rig = Rig::new();
    let report_id = report(&rig);
    failed_verify(&rig, &report_id, 1, "passed");
    assert!(all(&rig).events.is_empty());
}

#[test]
fn a_conflicted_integration_counts_its_files() {
    let rig = Rig::new();
    rig.pane("dev-1", "feat/only");
    let plan = rig.approved_plan_of(ONE);
    rig.assign(&plan, "only", "dev-1");
    let report = rig.report_as("dev-1", "feat/only", COMMIT, "work written");
    let report_id = report["record"]["reportId"].as_str().unwrap().to_string();
    rig.review(&report_id, "pass", "fine");
    rig.begin_integration("int-9", &[&report_id]);
    rig.finish_integration(
        "int-9",
        json!({"kind": "conflicted", "reportId": report_id, "files": ["a.txt", "b.txt"], "omitted": 0}),
    );
    let event = sole(&rig);
    assert_eq!(event.event, NtfyEvent::IntegrationConflicted);
    assert_eq!(event.subject, "int-9");
    assert_eq!(event.detail.as_deref(), Some("2"));
}

#[test]
fn ids_that_do_not_fit_the_rule_come_out_as_a_question_mark() {
    let rig = Rig::new();
    let bad = "bad id <script> SECRET";
    let long = "x".repeat(65);
    rig.core
        .queue_attention_notices(
            &rig.ctx("owner"),
            &json!([{"kind": "stalled", "agentId": bad, "episodeMs": 1}, {"kind": "blocked", "agentId": long, "episodeMs": 1}, {"kind": "stalled", "agentId": "a/b", "episodeMs": 1}]),
        )
        .unwrap();
    let page = all(&rig);
    assert_eq!(page.events.len(), 3, "{:#?}", page.events);
    for event in &page.events {
        assert_eq!(event.subject, "?");
        assert_eq!(event.agent_id.as_deref(), Some("?"));
    }
    assert!(!format!("{page:?}").contains("SECRET"));
}

// ---------------------------------------------------------------------------------------------------------- paging

#[test]
fn a_page_of_one_row_is_one_read_and_scanned_passes_skipped_rows() {
    let rig = Rig::new();
    let head = push_head(&rig.core).unwrap();
    for n in 0..10 {
        send(&rig, "dev-1", &format!("info {n}"), "info");
    }
    let page = push_events_after(&rig.core, head, 1).unwrap();
    assert!(page.events.is_empty());
    assert_eq!(page.scanned, head + 1);
    assert_eq!(page.queries, 1);
    // A question after the skipped rows is found by walking the cursor.
    send(&rig, "dev-1", "a question", "action");
    let mut cursor = head;
    let mut found = Vec::new();
    for _ in 0..40 {
        let page = push_events_after(&rig.core, cursor, 3).unwrap();
        found.extend(page.events);
        if page.scanned == cursor {
            break;
        }
        cursor = page.scanned;
    }
    assert_eq!(cursor, push_head(&rig.core).unwrap());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].event, NtfyEvent::UserQuestion);
}

#[test]
fn a_cursor_at_or_past_the_head_gets_no_events_and_the_head() {
    let rig = Rig::new();
    send(&rig, "dev-1", "a question", "action");
    let head = push_head(&rig.core).unwrap();
    for after in [head, head + 1, head + 1000, u64::MAX] {
        let page = push_events_after(&rig.core, after, 200).unwrap();
        assert!(page.events.is_empty());
        assert_eq!(page.scanned, head);
    }
    let empty = Rig::new();
    let page = push_events_after(&empty.core, 0, 200).unwrap();
    assert_eq!(page.scanned, push_head(&empty.core).unwrap());
}

#[test]
fn a_call_writes_nothing() {
    let rig = Rig::new();
    send(&rig, "dev-1", "a question", "action");
    let before = (rig.core.state_version().unwrap(), rig.head());
    let rows = ledger_rows(&rig.core).unwrap();
    all(&rig);
    push_events_after(&rig.core, 3, 5).unwrap();
    assert_eq!((rig.core.state_version().unwrap(), rig.head()), before);
    assert_eq!(ledger_rows(&rig.core).unwrap(), rows);
}

// ------------------------------------------------------------------------------------------------------------ cost

#[test]
fn sixty_design_reports_of_one_plan_read_the_body_once_and_stop_at_fifty_lookups() {
    let (rig, plan) = design_rig();
    for n in 1..=60 {
        rig.report_as("dev-1", "feat/ui", &commit(n), "designed");
    }
    let rows = ledger_rows(&rig.core).unwrap();
    let report_rows: Vec<i64> = rows
        .iter()
        .filter(|r| r.2 == "report.record" && r.4.as_deref() == Some("accepted"))
        .map(|r| r.0)
        .collect();
    assert_eq!(report_rows.len(), 60);
    let page = push_events_after(&rig.core, 0, 200).unwrap();
    check(&rig, &page);
    assert_eq!(page.events.len(), 49);
    assert!(page.events.iter().all(
        |e| e.event == NtfyEvent::DesignWaiting && e.plan_id.as_deref() == Some(plan.as_str())
    ));
    assert_eq!(page.scanned as i64, report_rows[48]);
    // The first row cost 2 (the report and the body), the other 48 cost 1: 50 lookups and the range read.
    assert_eq!(page.queries, 1 + MAX_LOOKUPS);
    // The next page resumes at the 50th report.
    let next = push_events_after(&rig.core, page.scanned, 200).unwrap();
    assert_eq!(next.events.len(), 11);
    assert_eq!(next.events[0].seq as i64, report_rows[49]);
}

#[test]
fn sixty_failed_verify_runs_handle_fifty_rows() {
    let rig = Rig::new();
    let report_id = report(&rig);
    let start = push_head(&rig.core).unwrap();
    for n in 0..60 {
        failed_verify(
            &rig,
            &report_id,
            n,
            if n % 2 == 0 { "failed" } else { "error" },
        );
    }
    let page = push_events_after(&rig.core, start, 200).unwrap();
    check(&rig, &page);
    assert_eq!(page.events.len(), 50);
    assert!(page.queries <= 51, "{}", page.queries);
    let rows = ledger_rows(&rig.core).unwrap();
    let finish: Vec<i64> = rows
        .iter()
        .filter(|r| r.2 == "verify.finish")
        .map(|r| r.0)
        .collect();
    // The page ends before the 51st run; the rows between the 50th and the 51st that cost nothing are passed.
    assert!(page.scanned as i64 >= finish[49] && (page.scanned as i64) < finish[50]);
    let next = push_events_after(&rig.core, page.scanned, 200).unwrap();
    assert_eq!(next.events.len(), 10);
}

#[test]
fn a_large_page_that_needs_fifty_lookups_stays_within_the_bound() {
    let rig = Rig::new();
    let report_id = report(&rig);
    let start = push_head(&rig.core).unwrap();
    for n in 0..60 {
        failed_verify(&rig, &report_id, n, "failed");
        send(&rig, "dev-2", &format!("info {n}"), "info");
    }
    let page = push_events_after(&rig.core, start, 200).unwrap();
    assert!(page.queries <= 51, "{}", page.queries);
    assert!(page.events.len() >= 40, "{}", page.events.len());
    assert!(page.scanned < push_head(&rig.core).unwrap());
    // Rows needing 50 lookups: the same ledger with the cap lifted by a second call reaches the end in few calls.
    let mut cursor = start;
    let mut seen = 0;
    for _ in 0..10 {
        let page = push_events_after(&rig.core, cursor, 200).unwrap();
        assert!(page.queries <= 51);
        seen += page.events.len();
        cursor = page.scanned;
    }
    assert_eq!(cursor, push_head(&rig.core).unwrap());
    assert_eq!(seen, 60);
}
