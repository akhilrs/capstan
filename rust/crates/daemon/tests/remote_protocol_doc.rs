//! Drift test for `docs/remote-protocol.md`: the document names what the code has (event types, key kinds, project
//! states, error codes, line kinds, caps), lists the `feed` subcommands, its JSON examples parse and its cursors
//! round-trip through the codec. The required body fields per type are pinned by this file's own table (which matches
//! the kernel's golden fixtures when written), not read from the kernel: the example events and the doc's table rows
//! are checked against that table.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use capstan_kernel::areas::event_feed::{self, KEY_KINDS, V1_TYPES};
use capstan_kernel::areas::feed_cursor::{decode_cursor, encode_cursor};
use capstan_kernel::areas::snapshot_feed;
use serde_json::Value;
use std::collections::BTreeSet;

const DOC: &str = include_str!("../../../../docs/remote-protocol.md");

const SUBCOMMANDS: &[&str] = &[
    "head", "page", "snapshot", "waiting", "agents", "plans", "item", "hash",
];
const PROJECT_STATES: &[&str] = &[
    "ready",
    "no_key",
    "silent",
    "refused",
    "unsupported",
    "gone",
];
const ERROR_CODES: &[&str] = &[
    "unauthorized",
    "forbidden",
    "unknown_command",
    "invalid_request",
    "error",
    "bad_cursor",
    "internal",
];
const LINE_KINDS: &[&str] = &[
    "hello",
    "project",
    "snapshot",
    "snapshotEnd",
    "event",
    "cursor",
    "heartbeat",
    "resync",
    "error",
];
const ENVELOPE: &[&str] = &[
    "id", "seq", "k", "ts", "type", "actorId", "entity", "from", "to", "body",
];

const AGENT: &[&str] = &["agentId", "kind", "role", "generation", "paused", "state"];
const TEXT: &[&str] = &["textVerified", "truncated", "untrusted"];
const PROMPT: &[&str] = &["relayId", "agentId", "state", "text", "options", "reason"];
const OPERATOR: &[&str] = &[
    "proposalId",
    "kind",
    "state",
    "command",
    "decision",
    "reason",
];
const MESSAGE: &[&str] = &["messageId", "recipientAgentId", "state", "body", "reason"];

/// The required body fields of a type, besides `effects`: the groups it is made of and the fields of its own.
fn required(ty: &str) -> Vec<&'static str> {
    let (groups, own): (&[&[&str]], &[&'static str]) = match ty {
        "message.action_needed" | "message.state" => (&[MESSAGE, TEXT], &[]),
        "prompt.waiting" | "prompt.answered" | "prompt.expired" => (&[PROMPT, TEXT], &[]),
        "operator.proposed" | "operator.decided" | "operator.finished" => (&[OPERATOR, TEXT], &[]),
        "report.recorded" => (
            &[TEXT],
            &["reportId", "agentId", "commitSha", "state", "summary"],
        ),
        "design.decided" => (
            &[],
            &["planId", "packageId", "reportId", "decision", "link"],
        ),
        "plan.submitted" | "plan.approved" => (&[], &["planId", "agentId", "revision", "state"]),
        "plan.signed_off" => (&[], &["planId", "integrationId", "state"]),
        "plan.changed" => (&[], &["planId", "state"]),
        "plan.needs_attention" => (&[], &["planId", "agentId", "kind", "state", "episodeMs"]),
        "review.verdict" => (&[], &["reviewId", "verdict"]),
        "verify.result" => (&[], &["runId", "result"]),
        "finding.raised" => (
            &[],
            &[
                "findingId",
                "raisedByAgentId",
                "targetAgentId",
                "severity",
                "state",
            ],
        ),
        "finding.resolved" => (&[], &["findingId", "result", "state"]),
        "agent.registered" | "agent.ended" => (&[AGENT], &[]),
        "agent.lost" => (&[AGENT, TEXT], &["reason"]),
        "agent.paused" | "agent.resumed" => (&[AGENT, TEXT], &["scope", "reason"]),
        "agent.blocked" => (&[AGENT], &["blocker"]),
        "run.changed" => (&[TEXT], &["state", "paused", "reason"]),
        "integration.finished" => (&[], &["integrationId", "state"]),
        "snapshot.invalidated" => (&[], &["section", "key"]),
        other => panic!("no required fields known for {other}"),
    };
    let mut fields: Vec<&'static str> = groups.iter().flat_map(|g| g.iter().copied()).collect();
    fields.extend_from_slice(own);
    fields
}

/// The bodies of the fenced blocks tagged `json`.
fn json_blocks() -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current: Option<String> = None;
    for line in DOC.lines() {
        match (&mut current, line.trim_end()) {
            (None, "```json") => current = Some(String::new()),
            (Some(_), "```") => blocks.push(current.take().expect("a block is open")),
            (Some(text), _) => {
                text.push_str(line);
                text.push('\n');
            }
            (None, _) => {}
        }
    }
    assert!(current.is_none(), "a json block is not closed");
    blocks
}

/// Every JSON value of the blocks (a block may hold several lines, each a value).
fn json_values() -> Vec<Value> {
    json_blocks()
        .iter()
        .flat_map(|block| {
            let whole = serde_json::from_str::<Value>(block);
            match whole {
                Ok(value) => vec![value],
                Err(_) => block
                    .lines()
                    .map(|line| {
                        serde_json::from_str::<Value>(line)
                            .unwrap_or_else(|e| panic!("json example does not parse ({e}): {line}"))
                    })
                    .collect(),
            }
        })
        .collect()
}

fn section_names() -> Vec<String> {
    DOC.lines()
        .filter_map(|l| l.strip_prefix("## "))
        .map(str::to_string)
        .collect()
}

#[test]
fn the_document_is_a_draft_with_its_ten_sections() {
    let mut lines = DOC.lines();
    assert_eq!(lines.next(), Some("# Remote protocol v1"));
    assert_eq!(lines.next(), Some(""));
    assert_eq!(
        lines.next(),
        Some("Status: draft (v1, awaiting approval)"),
        "the status line comes right after the title"
    );
    let sections = section_names();
    assert_eq!(sections.len(), 10, "{sections:?}");
    for (index, section) in sections.iter().enumerate() {
        assert!(
            section.starts_with(&format!("{}. ", index + 1)),
            "section {index}: {section}"
        );
    }
    assert!(DOC.contains("frozen API"));
    assert!(DOC.contains("needs a follow-up on the feed core and the feed cursor"));
}

#[test]
fn it_names_what_the_code_has() {
    for ty in V1_TYPES {
        assert!(DOC.contains(&format!("`{ty}`")), "event type {ty}");
    }
    for kind in KEY_KINDS {
        assert!(
            DOC.contains(&format!("`{}`", kind.kind)),
            "key kind {}",
            kind.kind
        );
        assert!(DOC.contains(kind.source), "key source {}", kind.source);
    }
    let paragraph = DOC
        .lines()
        .skip_while(|l| !l.starts_with("The daemon route `feed` takes these subcommands:"))
        .take_while(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let listed: Vec<&str> = paragraph
        .split('`')
        .skip(1)
        .step_by(2)
        .skip(1)
        .map(|c| c.split_whitespace().next().unwrap())
        .collect();
    assert_eq!(
        listed, SUBCOMMANDS,
        "the subcommands of the route paragraph"
    );
    for state in PROJECT_STATES {
        assert!(DOC.contains(&format!("`{state}`")), "project state {state}");
    }
    for code in ERROR_CODES {
        assert!(DOC.contains(&format!("`{code}`")), "error code {code}");
    }
    for kind in LINE_KINDS {
        assert!(DOC.contains(&format!("`{kind}`")), "line kind {kind}");
    }
    for op in ["upsert", "remove", "invalidate"] {
        assert!(DOC.contains(&format!("`{op}`")), "effect op {op}");
    }
}

#[test]
fn every_type_has_a_table_row_with_its_required_fields() {
    for ty in V1_TYPES {
        let prefix = format!("| `{ty}` | ");
        let row = DOC
            .lines()
            .find(|l| l.starts_with(&prefix))
            .unwrap_or_else(|| panic!("no table row for {ty}"));
        let listed: BTreeSet<&str> = row[prefix.len()..].split('`').skip(1).step_by(2).collect();
        let expected: BTreeSet<&str> = required(ty).into_iter().collect();
        assert_eq!(listed, expected, "required fields of {ty}");
    }
}

#[test]
fn the_caps_equal_the_code() {
    let rows = [
        ("event_feed::TEXT_CAP", event_feed::TEXT_CAP),
        ("event_feed::REASON_CAP", event_feed::REASON_CAP),
        ("event_feed::OPTIONS_CAP", event_feed::OPTIONS_CAP),
        ("event_feed::OPTION_TEXT_CAP", event_feed::OPTION_TEXT_CAP),
        ("snapshot_feed::TEXT_CAP", snapshot_feed::TEXT_CAP),
        ("event_feed::MAX_LIMIT", event_feed::MAX_LIMIT as usize),
        (
            "snapshot_feed::MAX_LIMIT",
            snapshot_feed::MAX_LIMIT as usize,
        ),
        ("event_feed::MAX_SCAN_ROWS", event_feed::MAX_SCAN_ROWS),
        ("snapshot_feed::MAX_READS", snapshot_feed::MAX_READS),
        ("serialized bytes per page", event_feed::MAX_PAGE_BYTES),
        ("serialized bytes per page", snapshot_feed::MAX_PAGE_BYTES),
    ];
    for (name, value) in rows {
        let row = DOC
            .lines()
            .find(|l| l.starts_with('|') && l.contains(name))
            .unwrap_or_else(|| panic!("no table row for {name}"));
        let cell = row.trim_end_matches('|').rsplit('|').next().unwrap().trim();
        assert_eq!(cell, value.to_string(), "{name}");
    }
}

#[test]
fn every_json_example_parses_and_the_cursors_round_trip() {
    let values = json_values();
    assert!(values.len() >= 10, "{} examples", values.len());
    let mut cursors = 0;
    let mut decoded_texts = Vec::new();
    for value in &values {
        if value["kind"] == "cursor" {
            let text = value["cursor"].as_str().expect("cursor text");
            let entries = decode_cursor(text).expect("the example cursor decodes");
            assert_eq!(encode_cursor(&entries), text, "re-encode");
            let bytes = URL_SAFE_NO_PAD.decode(&text[3..]).unwrap();
            decoded_texts.push(serde_json::from_slice::<Value>(&bytes).unwrap());
            assert!(entries.len() >= 2, "a multi-project cursor");
            cursors += 1;
        }
    }
    assert_eq!(cursors, 1);
    let shown = values
        .iter()
        .find(|v| v.get("p").is_some())
        .expect("the decoded cursor is shown");
    assert_eq!(shown, &decoded_texts[0], "the decoded cursor matches");
}

#[test]
fn example_events_have_the_envelope_and_their_required_fields() {
    let mut seen = 0;
    for line in json_values().iter().filter(|v| v["kind"] == "event") {
        assert_eq!(line["v"], 1);
        assert!(line["projectId"].is_string());
        let event = line["event"].as_object().expect("event envelope");
        for field in ENVELOPE {
            assert!(event.contains_key(*field), "envelope field {field}");
        }
        let ty = event["type"].as_str().unwrap();
        assert!(V1_TYPES.contains(&ty), "{ty}");
        let body = event["body"].as_object().unwrap();
        assert!(body["effects"].is_array());
        for field in required(ty) {
            assert!(body.contains_key(field), "{ty} body field {field}");
        }
        seen += 1;
    }
    assert!(seen >= 1);
}

#[test]
fn example_pages_have_the_shape_of_each_section() {
    let items: Vec<Value> = json_values()
        .into_iter()
        .filter(|v| v.get("items").is_some() || v.get("requestId") == Some(&Value::from("run")))
        .collect();
    let waiting = [
        "key",
        "requestId",
        "kind",
        "since",
        "summary",
        "contentHash",
        "needsTerminal",
    ];
    let agents = [
        "key",
        "agentId",
        "kind",
        "role",
        "generation",
        "state",
        "paused",
        "task",
        "requestId",
        "contentHash",
    ];
    let plans = ["key", "planId", "sequence", "title", "state", "packages"];
    let run = ["requestId", "state", "paused", "contentHash"];
    let expected: [&[&str]; 4] = [&waiting, &agents, &plans, &run];
    assert_eq!(items.len(), 4, "one example per section");
    for (value, fields) in items.iter().zip(expected) {
        let item = if value.get("items").is_some() {
            assert!(value["head"].is_u64() && value.get("next").is_some());
            &value["items"][0]
        } else {
            value
        };
        let mut have: Vec<&str> = item
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let mut want = fields.to_vec();
        have.sort_unstable();
        want.sort_unstable();
        assert_eq!(have, want);
    }
}

#[test]
fn it_names_no_product_or_vendor() {
    let lower = DOC.to_lowercase();
    for word in ["herdr", "claude", "anthropic", "capstan", "openai"] {
        assert!(!lower.contains(word), "the document names {word}");
    }
}
