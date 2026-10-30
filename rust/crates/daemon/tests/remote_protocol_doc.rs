//! Drift test for `docs/remote-protocol.md`: the document names what the code has (event types, key kinds, project
//! states, error codes, line kinds, caps), lists the `feed` subcommands, its JSON examples parse and its cursors
//! round-trip through the codec. The required body fields per type are pinned by this file's own table (which matches
//! the kernel's golden fixtures when written), not read from the kernel: the example events and the doc's table rows
//! are checked against that table.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use capstan_kernel::areas::event_feed::{self, FeedHead, FeedPage, Position, KEY_KINDS, V1_TYPES};
use capstan_kernel::areas::feed_cursor::{decode_cursor, encode_cursor};
use capstan_kernel::areas::remote_decisions::{
    ERROR_CODES as DECIDE_ERROR_CODES, RESULT_FIELDS, VERBS,
};
use capstan_kernel::areas::snapshot_feed::{self, RunItem, SnapshotHead};
use serde_json::Value;
use std::collections::BTreeSet;

const DOC: &str = include_str!("../../../../docs/remote-protocol.md");

const SUBCOMMANDS: &[&str] = &[
    "head", "page", "snapshot", "waiting", "agents", "plans", "item", "hash", "recent",
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
        "agent.activity" => (
            &[],
            &["agentId", "activity", "activitySince", "activitySeq"],
        ),
        "run.changed" => (&[TEXT], &["state", "paused", "reason"]),
        "integration.finished" => (&[], &["integrationId", "state"]),
        "decision.remote" => (
            &[],
            &[
                "requestId",
                "outcome",
                "verb",
                "client",
                "contentHash",
                "messageId",
                "replaces",
            ],
        ),
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
fn the_document_is_approved_with_its_eleven_sections() {
    let mut lines = DOC.lines();
    assert_eq!(lines.next(), Some("# Remote protocol v1"));
    assert_eq!(lines.next(), Some(""));
    assert_eq!(
        lines.next(),
        Some("Status: approved (v1)"),
        "the status line comes right after the title"
    );
    let sections = section_names();
    assert_eq!(sections.len(), 11, "{sections:?}");
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
fn the_doc_states_recent_history_signed_off_plans_and_the_agent_branch() {
    assert!(DOC.contains("### Recent history"));
    for word in [
        "oldestScanned",
        "rowsScanned",
        "rate_limited: ",
        "unknown_command",
        "display-only",
    ] {
        assert!(DOC.contains(word), "recent history names {word}");
    }
    for ty in event_feed::OUTCOME_TYPES {
        let section = DOC.split("### Recent history").nth(1).unwrap();
        assert!(section.contains(&format!("`{ty}`")), "outcome type {ty}");
    }
    assert!(DOC.contains("`signed_off`"));
    let agent = DOC
        .split("`AgentItem`: ")
        .nth(1)
        .and_then(|rest| rest.split("`PlanItem`").next())
        .unwrap();
    assert!(agent.contains("`branch`"), "AgentItem lists branch");
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
        "integrationState",
        "branch",
        "headSha",
        "integrationDetails",
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
        "activity",
        "activitySince",
        "activitySeq",
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

/// The members of `integrationDetails` when recorded: the frozen list of the merge inspector's design.
const DETAILS: &[&str] = &[
    "recorded",
    "impactError",
    "plans",
    "plansOmitted",
    "reports",
    "reportsOmitted",
    "signoffs",
    "signoffsOmitted",
    "impact",
    "review",
    "verify",
];

fn names(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    keys
}

fn sorted(list: &[&str]) -> Vec<String> {
    let mut keys: Vec<String> = list.iter().map(|k| k.to_string()).collect();
    keys.sort();
    keys
}

#[test]
fn the_integration_example_shows_every_member_of_the_details() {
    let page = json_values()
        .into_iter()
        .find(|v| v["items"][1]["kind"] == "integration")
        .expect("an integration example item");
    let details = &page["items"][1]["integrationDetails"];
    assert_eq!(names(details), sorted(DETAILS));
    let flags = ["textVerified", "truncated"];
    let with = |own: &[&str]| sorted(&[own, &flags].concat());
    assert_eq!(names(&details["plans"][0]), with(&["planId", "title"]));
    assert_eq!(
        names(&details["reports"][0]),
        with(&["reportId", "packageId", "summary"])
    );
    assert_eq!(
        names(&details["signoffs"][0]),
        with(&["planId", "summary", "createdAt"])
    );
    let impact = &details["impact"];
    assert_eq!(
        names(impact),
        sorted(&[
            "filesChanged",
            "insertions",
            "deletions",
            "truncated",
            "areas",
            "areasOmitted",
            "migrations",
            "migrationsOmitted",
            "nextSteps"
        ])
    );
    assert_eq!(names(&impact["areas"][0]), with(&["path", "files"]));
    assert_eq!(names(&impact["migrations"][0]), with(&["path"]));
    assert_eq!(names(&impact["nextSteps"][0]), with(&["text"]));
    assert_eq!(
        names(&details["review"]),
        sorted(&["verdict", "reviewerAgentId", "round"])
    );
    assert_eq!(names(&details["verify"]), sorted(&["latest", "lastPassed"]));
    assert_eq!(
        names(&details["verify"]["latest"]),
        sorted(&["state", "commitSha", "finishedAt"])
    );
    for member in DETAILS.iter().chain(&["integrationDetails", "contentHash"]) {
        assert!(
            DOC.contains(&format!("`{member}`")),
            "the doc lacks {member}"
        );
    }
}

#[test]
fn it_names_no_product_or_vendor() {
    let lower = DOC.to_lowercase();
    for word in ["herdr", "claude", "anthropic", "capstan", "openai"] {
        assert!(!lower.contains(word), "the document names {word}");
    }
}

/// The top-level field names of the `{...}` span that follows `marker` in the document.
fn listed_fields(marker: &str) -> BTreeSet<String> {
    let start = DOC
        .find(marker)
        .unwrap_or_else(|| panic!("the document lacks {marker}"))
        + marker.len();
    let span = &DOC[start..];
    let span = &span[..span.find('`').expect("the field list is closed")];
    let span = span
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .unwrap_or_else(|| panic!("not a braced list: {span}"));
    let mut fields = BTreeSet::new();
    let (mut depth, mut current) = (0, String::new());
    for c in span.chars().chain([',']) {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                let name = current.split_whitespace().next().unwrap_or("");
                assert!(!name.is_empty(), "an empty field in {span}");
                fields.insert(name.to_string());
                current.clear();
                continue;
            }
            _ => {}
        }
        if depth == 0 || c != '{' {
            current.push(c);
        }
    }
    fields
}

fn keys_of<T: serde::Serialize>(value: &T) -> BTreeSet<String> {
    serde_json::to_value(value)
        .unwrap()
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect()
}

#[test]
fn the_head_snapshot_and_page_answers_list_the_fields_the_code_serializes() {
    let head = FeedHead {
        project_id: "p".into(),
        epoch: "e".into(),
        head: 1,
        anchor: None,
    };
    assert_eq!(listed_fields("A `feed head` answer is `"), keys_of(&head));
    let snapshot = SnapshotHead {
        epoch: "e".into(),
        head: 1,
        anchor: None,
        run: RunItem {
            request_id: "run".into(),
            state: "active".into(),
            paused: false,
            content_hash: "h".into(),
        },
    };
    assert_eq!(
        listed_fields("`feed snapshot` answers `"),
        keys_of(&snapshot)
    );
    let page = FeedPage {
        project_id: "p".into(),
        epoch: "e".into(),
        head: 1,
        anchor_ok: true,
        ahead: false,
        events: Vec::new(),
        scanned: Position { seq: 0, k: 0 },
        anchor: None,
        has_more: false,
        rows_read: 0,
    };
    assert_eq!(listed_fields("A `feed page` answer is `"), keys_of(&page));
    assert!(DOC.contains("`anchor`"));
}

// ------------------------------------------------------------------------------------------------------ section 11

/// The text of section 11 of `doc`.
fn decisions_section(doc: &str) -> Result<&str, String> {
    let start = doc
        .find("\n## 11. Decisions\n")
        .ok_or("the document has no section 11")?;
    Ok(&doc[start..])
}

/// The `(kind, verb, outcome)` rows of the verbs table of section 11.
fn verb_rows(doc: &str) -> Result<Vec<(String, String, String)>, String> {
    let section = decisions_section(doc)?;
    let rows: Vec<(String, String, String)> = section
        .lines()
        .skip_while(|line| *line != "| kind | verb | outcome |")
        .skip(2)
        .take_while(|line| line.starts_with('|'))
        .map(|line| {
            let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
            match cells[..] {
                [kind, verb, outcome] => Ok((kind.into(), verb.into(), outcome.into())),
                _ => Err(format!("a verbs row without three cells: {line}")),
            }
        })
        .collect::<Result<_, _>>()?;
    if rows.is_empty() {
        return Err("the verbs table of section 11 is missing".into());
    }
    Ok(rows)
}

/// Whether the verbs table of `doc` lists exactly the verbs of the kernel.
fn check_verbs(doc: &str) -> Result<(), String> {
    let mut listed = verb_rows(doc)?;
    let mut expected: Vec<(String, String, String)> = VERBS
        .iter()
        .map(|row| {
            (
                row.kind.as_str().to_string(),
                row.verb.to_string(),
                row.outcome.as_str().to_string(),
            )
        })
        .collect();
    listed.sort();
    expected.sort();
    if listed == expected {
        Ok(())
    } else {
        Err(format!(
            "verbs: the document has {listed:?}, the code {expected:?}"
        ))
    }
}

/// Whether section 11 of `doc` names every refusal code of the route.
fn check_codes(doc: &str) -> Result<(), String> {
    let section = decisions_section(doc)?;
    for code in DECIDE_ERROR_CODES
        .iter()
        .chain(&["forbidden", "unauthorized"])
    {
        if !section.contains(&format!("`{code}`")) {
            return Err(format!("section 11 lacks the code {code}"));
        }
    }
    Ok(())
}

/// The JSON objects of the fenced blocks of section 11.
fn decision_examples(doc: &str) -> Result<Vec<Value>, String> {
    let mut values = Vec::new();
    let mut open: Option<String> = None;
    for line in decisions_section(doc)?.lines() {
        match (&mut open, line.trim_end()) {
            (None, "```json") => open = Some(String::new()),
            (Some(_), "```") => {
                let text = open.take().expect("a block is open");
                values.push(
                    serde_json::from_str::<Value>(&text).map_err(|e| format!("{e}: {text}"))?,
                );
            }
            (Some(text), _) => text.push_str(line),
            (None, _) => {}
        }
    }
    Ok(values)
}

fn field_names(value: &Value) -> BTreeSet<String> {
    value
        .as_object()
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default()
}

/// Whether section 11 has one result with the fields of the kernel and one refusal of the form `{v, error {code, message}}`.
fn check_examples(doc: &str) -> Result<(), String> {
    let examples = decision_examples(doc)?;
    let results: Vec<&Value> = examples
        .iter()
        .filter(|v| v.get("decisionId").is_some())
        .collect();
    let refusals: Vec<&Value> = examples
        .iter()
        .filter(|v| v.get("error").is_some())
        .collect();
    let [result] = results[..] else {
        return Err(format!("{} result examples, one wanted", results.len()));
    };
    let [refusal] = refusals[..] else {
        return Err(format!("{} refusal examples, one wanted", refusals.len()));
    };
    let want: BTreeSet<String> = RESULT_FIELDS.iter().map(|f| f.to_string()).collect();
    if field_names(result) != want {
        return Err(format!(
            "result fields {:?}, the code has {want:?}",
            field_names(result)
        ));
    }
    let top: BTreeSet<String> = ["v", "error"].map(String::from).into();
    let inner: BTreeSet<String> = ["code", "message"].map(String::from).into();
    if field_names(refusal) != top || field_names(&refusal["error"]) != inner {
        return Err(format!("refusal fields {:?}", field_names(refusal)));
    }
    if refusal["error"]["code"].as_str().is_none() || result["v"] != 1 || refusal["v"] != 1 {
        return Err("an example without its code or with another version".into());
    }
    Ok(())
}

#[test]
fn section_11_lists_the_verbs_the_codes_and_the_examples_of_the_code() {
    check_verbs(DOC).unwrap();
    check_codes(DOC).unwrap();
    check_examples(DOC).unwrap();
}

#[test]
fn the_verbs_check_fails_on_a_row_less_and_on_an_extra_row() {
    let row = "| op | deny | applied |\n";
    assert!(DOC.contains(row));
    let fewer = DOC.replacen(row, "", 1);
    assert!(check_verbs(&fewer).is_err());
    let extra = DOC.replacen(row, &format!("{row}| op | cancel | applied |\n"), 1);
    assert!(check_verbs(&extra).is_err());
    let other = DOC.replacen(row, "| op | deny | forwarded |\n", 1);
    assert!(check_verbs(&other).is_err());
}

#[test]
fn the_code_and_example_checks_fail_on_a_changed_copy() {
    let without_code = DOC.replace("`limit_reached`", "`limit`");
    assert!(check_codes(&without_code).is_err());
    let without_field = DOC.replacen(",\"eventSeq\":57", "", 1);
    assert_ne!(without_field, DOC);
    assert!(check_examples(&without_field).is_err());
    let renamed = DOC.replacen(
        "{\"code\":\"hash_mismatch\"",
        "{\"kind\":\"hash_mismatch\"",
        1,
    );
    assert!(check_examples(&renamed).is_err());
}
