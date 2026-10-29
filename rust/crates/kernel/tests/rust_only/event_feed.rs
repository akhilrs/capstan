//! The kernel event feed (`areas::event_feed`): the public events of the ledger rows the kernel's own mutations write, the
//! golden fixtures of each type, the bounds and positions of a page, and the text rules. Every ledger is built through the
//! kernel's mutation functions on a `SeededEnv` kernel; the fixtures are normalised (timestamps and uuids) and compared byte
//! for byte (`CAPSTAN_BLESS=1` writes them, locally only).

use crate::fixture::{context, initial, OWNER};
use capstan_kernel::areas::event_feed::{
    action_is_mapped, feed_head, feed_page, feed_page_with, item_key, parse_item_key, Position,
    DROPPED, KEY_KINDS, LEGACY_ACTIONS, MAX_PAGE_BYTES, MAX_SCAN_ROWS, OPTIONS_CAP,
    OPTION_TEXT_CAP, SKIPPED_ACTIONS, TEXT_CAP, V1_TYPES,
};
use capstan_kernel::areas::feed_cursor::anchor;
use capstan_kernel::areas::write_once::write_once_snapshot;
use capstan_kernel::helpers::{clean_text, CLEAN_CASES, MAX_EVENT_BYTES, UNSAFE_TEXT_PATTERN};
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::types::MutationContext;
use capstan_kernel::{Core, SeededEnv};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub const BASE: &str = "0123456789abcdef0123456789abcdef01234567";
pub const COMMIT: &str = "1111111111111111111111111111111111111111";

// -------------------------------------------------------------------------------------------------------------- rig

pub struct Rig {
    pub _dir: tempfile::TempDir,
    pub state: PathBuf,
    pub core: Core,
    pub creds: BTreeMap<String, String>,
}

fn private(path: &Path) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

pub fn open_seeded(state: &Path, seed: &str) -> Core {
    Core::open(
        state,
        &initial(),
        &KernelOptions::default(),
        Box::new(SeededEnv::new(seed)),
    )
    .expect("the ledger opens")
}

impl Rig {
    /// pm-1 (PM), dev-1 and dev-2 (Developer), arch-1 (architect, Developer kind), rev-1 (Verifier).
    pub fn new() -> Rig {
        let dir = tempfile::tempdir().unwrap();
        private(dir.path());
        let state = dir.path().join("state");
        std::fs::create_dir(&state).unwrap();
        private(&state);
        let core = open_seeded(&state, "event-feed");
        let mut rig = Rig {
            _dir: dir,
            state,
            core,
            creds: BTreeMap::new(),
        };
        rig.owner(
            "syncRoleDefinitions",
            json!([[
                {"name": "pm", "kind": "PM", "host": "claude", "configHash": "b".repeat(64)},
                {"name": "developer", "kind": "Developer", "host": "claude", "configHash": "a".repeat(64)},
                {"name": "architect", "kind": "Developer", "host": "claude", "configHash": "e".repeat(64)},
                {"name": "reviewer", "kind": "Verifier", "host": "claude", "configHash": "f".repeat(64)},
            ]]),
        );
        for (agent, role, kind) in [
            ("pm-1", "pm", "PM"),
            ("dev-1", "developer", "Developer"),
            ("dev-2", "developer", "Developer"),
            ("arch-1", "architect", "Developer"),
            ("rev-1", "reviewer", "Verifier"),
        ] {
            rig.add_agent(agent, role, kind);
        }
        rig
    }

    /// An operation with the owner's credential.
    pub fn owner(&self, op: &str, args: Value) -> Value {
        let mut all = vec![serde_json::to_value(self.ctx("owner")).unwrap()];
        all.extend(args.as_array().cloned().unwrap());
        self.core
            .dispatch(op, &all)
            .unwrap_or_else(|e| panic!("{op}: {e}"))
    }

    pub fn add_agent(&mut self, agent: &str, role: &str, kind: &str) {
        let seat = format!("seat-{agent}");
        self.owner(
            "createSeat",
            json!([{"seatId": seat, "name": seat, "role": kind}]),
        );
        let actor = self.owner(
            "createActor",
            json!([{"displayName": agent, "role": kind, "seatId": seat}]),
        );
        self.owner(
            "registerAgent",
            json!([{"agentId": agent, "roleName": role, "seatId": seat, "actorId": actor["actorId"]}]),
        );
        self.creds.insert(
            agent.to_string(),
            actor["credential"].as_str().unwrap().to_string(),
        );
    }

    pub fn cred(&self, who: &str) -> String {
        if who == "owner" {
            OWNER.to_string()
        } else {
            self.creds[who].clone()
        }
    }

    pub fn ctx(&self, who: &str) -> MutationContext {
        context(&self.core, &self.cred(who))
    }

    pub fn head(&self) -> u64 {
        feed_head(&self.core).unwrap().head
    }

    /// Every event after ledger row `head`.
    pub fn since(&self, head: u64) -> Vec<Value> {
        feed_page(
            &self.core,
            Position {
                seq: head,
                k: u32::MAX - 1,
            },
            500,
            None,
        )
        .unwrap()
        .events
    }

    /// The guarded (write-once) columns of every row of every guarded table, by table and id.
    pub fn write_once_state(&self) -> BTreeMap<(String, String), Vec<(String, Value)>> {
        let db = self.db();
        let mut state = BTreeMap::new();
        for (table, id_column) in [
            ("messages", "message_id"),
            ("prompt_relays", "relay_id"),
            ("operator_proposals", "proposal_id"),
            ("agent_reports", "report_id"),
            ("design_approvals", "decision_id"),
            ("agents", "agent_id"),
        ] {
            let mut statement = db
                .prepare(&format!("SELECT {id_column} FROM {table}"))
                .unwrap();
            let ids: Vec<String> = statement
                .query_map([], |row| row.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            for id in ids {
                let columns = write_once_snapshot(&self.core, table, &id).unwrap();
                state.insert((table.to_string(), id), columns);
            }
        }
        state
    }

    /// Runs one mutation, checks that it raised the state version and left every write-once column as it was, and returns
    /// the events it wrote.
    pub fn run<R>(&self, mutation: impl FnOnce(&Rig) -> R) -> (R, Vec<Value>) {
        let head = self.head();
        let before = self.core.state_version().unwrap();
        let guarded = self.write_once_state();
        let result = mutation(self);
        assert!(
            self.core.state_version().unwrap() > before,
            "the mutation did not raise state_version"
        );
        let after = self.write_once_state();
        for (key, columns) in &guarded {
            assert_eq!(
                after.get(key),
                Some(columns),
                "a writer changed write-once columns of {key:?}"
            );
        }
        (result, self.since(head))
    }

    pub fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.state.join("controller.sqlite")).unwrap()
    }

    pub fn reopen(&mut self, seed: &str) {
        self.core.close();
        self.core = open_seeded(&self.state, seed);
    }
}

// --------------------------------------------------------------------------------------------------------- goldens

fn replace_uuids(text: &str, seen: &mut Vec<String>) -> String {
    let bytes: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let is_uuid_at = |i: usize| -> bool {
        if i + 36 > bytes.len() {
            return false;
        }
        bytes[i..i + 36].iter().enumerate().all(|(k, c)| match k {
            8 | 13 | 18 | 23 => *c == '-',
            _ => c.is_ascii_hexdigit() && !c.is_ascii_uppercase(),
        })
    };
    while i < bytes.len() {
        if is_uuid_at(i) {
            let id: String = bytes[i..i + 36].iter().collect();
            let n = match seen.iter().position(|s| *s == id) {
                Some(n) => n,
                None => {
                    seen.push(id);
                    seen.len() - 1
                }
            };
            out.push_str(&format!("<id:{}>", n + 1));
            i += 36;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

fn normal(value: &Value, seen: &mut Vec<String>) -> Value {
    match value {
        Value::String(text) => Value::String(replace_uuids(text, seen)),
        Value::Array(items) => Value::Array(items.iter().map(|v| normal(v, seen)).collect()),
        Value::Object(map) => {
            let mut sorted = BTreeMap::new();
            for (key, item) in map {
                let shown = if key == "ts" {
                    Value::String("<ts>".into())
                } else {
                    normal(item, seen)
                };
                sorted.insert(replace_uuids(key, seen), shown);
            }
            Value::Object(sorted.into_iter().collect::<Map<_, _>>())
        }
        other => other.clone(),
    }
}

/// The text of a golden fixture: ts and uuids replaced, keys sorted, 2-space indent and a final newline.
pub fn golden_text(event: &Value) -> String {
    let mut seen = Vec::new();
    let mut text = serde_json::to_string_pretty(&normal(event, &mut seen)).unwrap();
    text.push('\n');
    text
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/rust_only/fixtures/event_feed")
}

/// Compares the event with the fixture of `name` (written with `CAPSTAN_BLESS=1`; a missing fixture fails).
pub fn golden(name: &str, event: &Value) {
    let path = fixture_dir().join(format!("{name}.json"));
    let text = golden_text(event);
    if std::env::var("CAPSTAN_BLESS").as_deref() == Ok("1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &text).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing fixture {}", path.display()));
    assert_eq!(text, expected, "fixture {name} differs");
}

// ----------------------------------------------------------------------------------------------- field-by-field checks

/// The body fields each family must carry, fixed by the plan.
fn required_fields(ty: &str) -> &'static [&'static str] {
    match ty.split('.').next().unwrap() {
        "message" => &["messageId", "recipientAgentId", "state", "body"],
        "prompt" => &["relayId", "agentId", "state", "text", "options", "reason"],
        "operator" => &[
            "proposalId",
            "kind",
            "state",
            "command",
            "reason",
            "decision",
        ],
        "report" => &["reportId", "agentId", "commitSha", "summary", "current"],
        "design" => &["planId", "packageId", "decision", "reportId", "link"],
        "plan" => &["planId", "state"],
        "review" => &["reviewId", "verdict"],
        "verify" => &["runId", "result"],
        "finding" => &["findingId", "state"],
        "agent" => &["agentId", "kind", "role", "state", "paused", "generation"],
        "run" => &["state", "paused"],
        "integration" => &["integrationId", "state"],
        "snapshot" => &["section", "key"],
        other => panic!("no family {other}"),
    }
}

const TEXT_FAMILIES: &[&str] = &["message", "prompt", "operator", "report"];

/// Checks the envelope, the required body fields and the effects of one event, field by field.
pub fn check_event(event: &Value, project: &str) {
    let object = event.as_object().expect("an event is an object");
    let keys: BTreeSet<&str> = object.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        BTreeSet::from(["id", "seq", "k", "ts", "type", "actorId", "entity", "from", "to", "body"]),
        "envelope keys of {event}"
    );
    let ty = event["type"].as_str().unwrap();
    assert!(V1_TYPES.contains(&ty), "{ty} is not a v1 type");
    let seq = event["seq"].as_u64().unwrap();
    let id = event["id"].as_str().unwrap();
    let prefix = format!("{project}:");
    assert!(id.starts_with(&prefix), "{id}");
    let parts: Vec<&str> = id[prefix.len()..].split(':').collect();
    assert_eq!(parts[0].len(), 16, "epoch of {id}");
    assert_eq!(parts[1].parse::<u64>().unwrap(), seq, "{id}");
    match parts.len() {
        2 => assert_eq!(event["k"], 0, "{id}"),
        3 => assert_eq!(
            parts[2].parse::<u64>().unwrap(),
            event["k"].as_u64().unwrap(),
            "{id}"
        ),
        _ => panic!("bad id {id}"),
    }
    assert!(event["ts"].as_str().is_some_and(|ts| ts.ends_with('Z')));
    assert!(event["actorId"].as_str().is_some_and(|a| !a.is_empty()));
    assert!(event["entity"]["type"].is_string() && event["entity"]["id"].is_string());
    assert!(object["from"].is_string() || object["from"].is_null());
    assert!(object["to"].is_string() || object["to"].is_null());
    let body = event["body"].as_object().expect("body");
    for field in required_fields(ty) {
        assert!(
            body.contains_key(*field),
            "{ty} lacks body.{field}: {event}"
        );
    }
    let family = ty.split('.').next().unwrap();
    if TEXT_FAMILIES.contains(&family) {
        assert!(body["textVerified"].is_boolean(), "{ty}");
        assert!(body["untrusted"].is_array(), "{ty}");
        assert!(body["truncated"].is_boolean(), "{ty}");
    }
    let effects = body["effects"].as_array().expect("effects");
    for effect in effects {
        let section = effect["section"].as_str().unwrap();
        assert!(["waiting", "agents", "run", "plans"].contains(&section));
        assert!(["upsert", "remove", "invalidate"].contains(&effect["op"].as_str().unwrap()));
        let key = effect["key"].as_str().unwrap();
        assert!(effect["fields"].is_object());
        let kinds: Vec<&str> = KEY_KINDS
            .iter()
            .filter(|k| k.section == section)
            .map(|k| k.kind)
            .collect();
        match parse_item_key(section, key) {
            Some((kind, rest)) => {
                assert!(kinds.contains(&kind), "{key}");
                assert_eq!(item_key(section, kind, &rest), key);
            }
            None => {
                // A kind refresh names the kind alone.
                assert!(
                    effect["op"] == "invalidate" && kinds.contains(&key),
                    "effect key {key} of {ty} does not parse through KEY_KINDS"
                );
            }
        }
    }
    assert!(event.to_string().len() <= MAX_EVENT_BYTES);
}

pub fn project_of(rig: &Rig) -> String {
    rig.core.project_id().to_string()
}

pub fn events_of<'a>(events: &'a [Value], ty: &str) -> Vec<&'a Value> {
    events.iter().filter(|e| e["type"] == ty).collect()
}

/// Checks every event and the first of `ty` against its golden; returns that first event.
pub fn expect_type<'a>(rig: &Rig, events: &'a [Value], ty: &str) -> &'a Value {
    let project = project_of(rig);
    for event in events {
        check_event(event, &project);
    }
    let found = events_of(events, ty);
    assert!(!found.is_empty(), "no {ty} in {events:#?}");
    golden(ty, found[0]);
    found[0]
}

// ------------------------------------------------------------------------------------------------------ messages

impl Rig {
    pub fn enqueue(&self, to: &str, body: &str, action_needed: bool) -> String {
        self.core
            .enqueue_message(
                &self.ctx("owner"),
                &json!({"recipientAgentId": to, "body": body, "actionNeeded": action_needed}),
            )
            .unwrap()["messageId"]
            .as_str()
            .unwrap()
            .to_string()
    }
}

#[test]
fn feed_message_action_needed() {
    let rig = Rig::new();
    let (id, events) = rig.run(|r| r.enqueue("dev-1", "please look at the failing test", true));
    let event = expect_type(&rig, &events, "message.action_needed");
    assert_eq!(events.len(), 1);
    assert_eq!(event["entity"], json!({"type": "message", "id": id}));
    assert_eq!(event["from"], Value::Null);
    assert_eq!(event["to"], "queued");
    let body = &event["body"];
    assert_eq!(body["messageId"], json!(id));
    assert_eq!(body["recipientAgentId"], "dev-1");
    assert_eq!(body["state"], "queued");
    assert_eq!(body["body"], "please look at the failing test");
    assert_eq!(body["textVerified"], true);
    assert_eq!(body["untrusted"], json!(["body.body"]));
    let effect = &body["effects"][0];
    assert_eq!(
        (effect["section"].as_str(), effect["op"].as_str()),
        (Some("waiting"), Some("invalidate"))
    );
    assert!(effect["key"].as_str().unwrap().starts_with("message:"));
    // An ordinary message is skipped.
    let (_, ordinary) = rig.run(|r| r.enqueue("dev-1", "just so you know", false));
    assert!(ordinary.is_empty());
}

#[test]
fn feed_message_state() {
    let rig = Rig::new();
    let (id, _) = rig.run(|r| r.enqueue("dev-1", "needs you", true));
    let owner = rig.ctx("owner");
    let (_, sent) = rig.run(|r| r.core.record_sent(&owner, &id).unwrap());
    let event = expect_type(&rig, &sent, "message.state");
    assert_eq!(
        (event["from"].as_str(), event["to"].as_str()),
        (Some("queued"), Some("sent"))
    );
    assert_eq!(event["body"]["state"], "sent");
    assert_eq!(event["body"]["body"], "needs you");
    assert_eq!(event["body"]["textVerified"], true);
    let (_, acked) = rig.run(|r| r.core.ack_message(&r.ctx("dev-1"), &id).unwrap());
    assert_eq!(acked[0]["body"]["state"], "acked");
    // resolve cancel, fail
    let second = rig.enqueue("dev-1", "second", true);
    let (_, cancelled) = rig.run(|r| {
        r.core
            .resolve_message(
                &r.ctx("owner"),
                &second,
                &json!("cancel"),
                Some("no longer needed"),
            )
            .unwrap()
    });
    assert_eq!(cancelled.len(), 1);
    assert_eq!(cancelled[0]["type"], "message.state");
    assert_eq!(cancelled[0]["body"]["state"], "cancelled");
    assert_eq!(cancelled[0]["body"]["decision"], "cancel");
    let third = rig.enqueue("dev-1", "third", true);
    let (_, failed) = rig.run(|r| {
        r.core
            .record_failure(&r.ctx("owner"), &third, "pane gone")
            .unwrap()
    });
    assert_eq!(failed[0]["body"]["state"], "failed");
    assert_eq!(failed[0]["body"]["reason"], "pane gone");
    assert_eq!(
        failed[0]["body"]["untrusted"],
        json!(["body.body", "body.reason"])
    );
    // An ordinary message's state changes are skipped too.
    let plain = rig.enqueue("dev-2", "plain", false);
    let (_, none) = rig.run(|r| r.core.record_sent(&r.ctx("owner"), &plain).unwrap());
    assert!(none.is_empty());
    for event in sent.iter().chain(&acked).chain(&cancelled).chain(&failed) {
        check_event(event, &project_of(&rig));
    }
}

#[test]
fn feed_message_state_expire() {
    let rig = Rig::new();
    let id = rig.enqueue("dev-1", "waits", true);
    rig.run(|r| {
        r.core
            .record_deferral(&r.ctx("owner"), &id, &json!("agent_busy"))
            .unwrap()
    });
    let timers = json!({
        "maxDeferralSeconds": 0.001, "maxBusyDeferralSeconds": 0.001, "pmAckTimeoutSeconds": 0.001,
        "pmNotifyAfterSeconds": 0.001, "notifyIntervalSeconds": 0.001, "stallAfterSeconds": 0.001,
        "workerAckTimeoutSeconds": 0.001, "pmWakeAfterSeconds": 0.001, "pmWakeIntervalSeconds": 0.001,
    });
    let (_, events) = rig.run(|r| r.core.advance_messaging(&r.ctx("owner"), &timers).unwrap());
    let states = events_of(&events, "message.state");
    assert!(!states.is_empty(), "{events:#?}");
    assert_eq!(states[0]["body"]["state"], "expired");
}

// -------------------------------------------------------------------------------------------------------- prompts

/// The hash of the prompt as the Herdr layer writes it (`prompt_hash`): canonical JSON with a fixed key order.
pub fn prompt_sha(agent: &str, pane: &str, host: &str, text: &str, options: &Value) -> String {
    let mut canonical = Map::new();
    canonical.insert("agentId".into(), json!(agent));
    canonical.insert("paneId".into(), json!(pane));
    canonical.insert("hostKind".into(), json!(host));
    canonical.insert("text".into(), json!(text));
    canonical.insert("options".into(), options.clone());
    capstan_kernel::canonical::sha256(&Value::Object(canonical).to_string())
}

pub fn yes_no() -> Value {
    json!([
        {"number": 1, "text": "Yes", "acceptsText": false, "widensPermissions": false},
        {"number": 2, "text": "No, tell me what to do instead", "acceptsText": true, "widensPermissions": false},
    ])
}

impl Rig {
    pub fn relay_on(&self) {
        self.core
            .configure_prompt_relay(&json!({"enabled": true, "captureTtlSeconds": 600}))
            .unwrap();
    }

    /// The PM captures a prompt of `agent`; returns the relay id.
    pub fn capture_with(&self, agent: &str, text: &str, options: &Value) -> Value {
        let sha = prompt_sha(agent, "pane-1", "claude", text, options);
        self.core
            .record_prompt_capture(
                &self.ctx("pm-1"),
                &json!({"prompt": {
                    "agentId": agent, "paneId": "pane-1", "hostKind": "claude", "text": text,
                    "options": options, "promptSha": sha,
                }}),
            )
            .unwrap()
    }

    pub fn capture(&self, agent: &str, text: &str) -> Value {
        self.capture_with(agent, text, &yes_no())
    }
}

#[test]
fn feed_prompt_waiting() {
    let rig = Rig::new();
    rig.relay_on();
    let (relay, events) = rig.run(|r| r.capture("dev-1", "Allow Bash(ls)?"));
    let event = expect_type(&rig, &events, "prompt.waiting");
    assert_eq!(events.len(), 1);
    assert_eq!(event["entity"]["type"], "prompt_relay");
    assert_eq!(event["to"], "captured");
    let body = &event["body"];
    assert_eq!(body["relayId"], relay["relayId"]);
    assert_eq!(body["agentId"], "dev-1");
    assert_eq!(body["state"], "captured");
    assert_eq!(body["text"], "Allow Bash(ls)?");
    assert_eq!(body["options"], yes_no());
    assert_eq!(body["textVerified"], true);
    assert_eq!(body["untrusted"], json!(["body.text", "body.options"]));
    assert_eq!(body["effects"][0]["key"], "prompt:1");
    // A second capture of the same agent supersedes the first, in ledger order: the capture, then the expiry.
    let (second, events) = rig.run(|r| r.capture("dev-1", "Allow Bash(pwd)?"));
    let types: Vec<&str> = events.iter().map(|e| e["type"].as_str().unwrap()).collect();
    assert_eq!(types, ["prompt.waiting", "prompt.expired"]);
    assert_eq!(events[0]["body"]["relayId"], second["relayId"]);
    assert_eq!(events[1]["body"]["relayId"], relay["relayId"]);
    assert_eq!(events[1]["body"]["reason"], "superseded");
    assert_eq!(events[1]["body"]["state"], "expired");
    assert_eq!(events[1]["body"]["text"], "Allow Bash(ls)?");
    assert_eq!(events[1]["body"]["textVerified"], true);
    assert_eq!(events[1]["from"], "captured");
    for event in &events {
        check_event(event, &project_of(&rig));
    }
    golden("prompt.expired.superseded", &events[1]);
}

#[test]
fn feed_prompt_answered() {
    let rig = Rig::new();
    rig.relay_on();
    let relay = rig.capture("dev-1", "Allow Bash(ls)?");
    let (_, begun) = rig.run(|r| {
        r.core
            .begin_prompt_answer(
                &r.ctx("pm-1"),
                &json!({"relayId": relay["relayId"], "hash": relay["promptSha"], "answer": {"kind": "option", "number": 1}}),
            )
            .unwrap()
    });
    let event = expect_type(&rig, &begun, "prompt.answered");
    assert_eq!(event["body"]["state"], "typing");
    assert_eq!(event["body"]["text"], "Allow Bash(ls)?");
    assert_eq!(event["body"]["textVerified"], true);
    let (_, finished) = rig.run(|r| {
        r.core
            .finish_prompt_answer(
                &r.ctx("owner"),
                &json!({"relayId": relay["relayId"], "outcome": {"typed": true, "keys": ["1"]}}),
            )
            .unwrap()
    });
    assert_eq!(finished[0]["type"], "prompt.answered");
    assert_eq!(finished[0]["body"]["state"], "answered");
    assert_eq!(finished[0]["from"], "typing");
    // A refusal before any key was typed.
    let other = rig.capture("dev-2", "Allow Read(x)?");
    let (_, refused) = rig.run(|r| {
        r.core
            .refuse_prompt_answer(
                &r.ctx("pm-1"),
                &json!({"relayId": other["relayId"], "answer": {"kind": "esc"}, "reason": "not now"}),
            )
            .unwrap()
    });
    assert_eq!(refused[0]["body"]["state"], "refused");
    assert_eq!(refused[0]["body"]["reason"], "not now");
}

#[test]
fn feed_prompt_expired() {
    let mut rig = Rig::new();
    rig.add_agent("dev-3", "developer", "Developer");
    rig.core
        .configure_prompt_relay(&json!({"enabled": true, "captureTtlSeconds": 0.001}))
        .unwrap();
    let ids: Vec<Value> = ["dev-1", "dev-2", "dev-3"]
        .iter()
        .map(|agent| rig.capture(agent, &format!("Allow {agent}?"))["relayId"].clone())
        .collect();
    let (expired, events) = rig.run(|r| r.core.expire_prompt_captures(&r.ctx("owner")).unwrap());
    assert_eq!(expired, json!(ids));
    assert_eq!(events.len(), 3);
    let first = expect_type(&rig, &events, "prompt.expired");
    assert_eq!(first["body"]["reason"], "expired");
    for (k, event) in events.iter().enumerate() {
        assert_eq!(event["type"], "prompt.expired");
        assert_eq!(event["k"], k);
        assert_eq!(event["seq"], events[0]["seq"]);
        assert_eq!(event["body"]["relayId"], ids[k]);
        assert_eq!(event["body"]["textVerified"], true);
        assert!(event["id"].as_str().unwrap().ends_with(&format!(":{k}")));
    }
    // limit 1 stops inside the row and resumes at (seq, k) with no loss or duplicate.
    let seq = events[0]["seq"].as_u64().unwrap();
    let mut after = Position {
        seq: seq - 1,
        k: u32::MAX - 1,
    };
    let mut seen = Vec::new();
    loop {
        let page = feed_page(&rig.core, after, 1, None).unwrap();
        if page.events.is_empty() {
            break;
        }
        seen.push(page.events[0]["id"].as_str().unwrap().to_string());
        assert_eq!(
            page.scanned,
            Position {
                seq,
                k: (seen.len() - 1) as u32
            }
        );
        after = page.scanned;
        if after.seq > seq {
            break;
        }
        if !page.has_more {
            break;
        }
    }
    let want: Vec<String> = events
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(seen, want);
    rig.reopen("again");
}

// ------------------------------------------------------------------------------------------------------ operator

impl Rig {
    pub fn propose(&self, command: &str, reason: &str) -> Value {
        self.core
            .propose_operator_action(
                &self.ctx("dev-1"),
                &json!({"kind": "command", "command": command, "reason": reason, "maxPending": 20}),
            )
            .unwrap()
    }

    pub fn decide(&self, proposal: &Value, decision: &str) -> Value {
        self.core
            .decide_operator_proposal(
                &self.ctx("pm-1"),
                &json!({
                    "proposalId": proposal["proposalId"],
                    "decision": decision,
                    "hash": proposal["commandSha"].as_str().unwrap()[..12],
                    "proposalTtlMinutes": 100000,
                }),
            )
            .unwrap()
    }
}

const LIMITS: fn() -> Value =
    || json!({"proposalTtlMinutes": 100000, "approvalTtlMinutes": 100000});

#[test]
fn feed_operator_proposed() {
    let rig = Rig::new();
    let (proposal, events) = rig.run(|r| r.propose("ls -la", "look around"));
    let event = expect_type(&rig, &events, "operator.proposed");
    assert_eq!(events.len(), 1);
    let body = &event["body"];
    assert_eq!(body["proposalId"], proposal["proposalId"]);
    assert_eq!(body["kind"], "command");
    assert_eq!(body["state"], "proposed");
    assert_eq!(body["command"], "ls -la");
    assert_eq!(body["reason"], "look around");
    assert_eq!(body["decision"], Value::Null);
    assert_eq!(body["textVerified"], true);
    assert_eq!(body["effects"][0]["key"], "op:1");
    assert_eq!(event["entity"]["type"], "operator_proposal");
}

#[test]
fn feed_operator_decided() {
    let rig = Rig::new();
    let approved = rig.propose("ls -la", "look");
    let (_, events) = rig.run(|r| r.decide(&approved, "approve"));
    let event = expect_type(&rig, &events, "operator.decided");
    assert_eq!(event["body"]["state"], "approved");
    assert_eq!(event["body"]["decision"], "approved");
    assert_eq!(event["body"]["command"], "ls -la");
    assert_eq!(event["from"], "proposed");
    let denied = rig.propose("rm -rf build", "clean");
    let (_, events) = rig.run(|r| r.decide(&denied, "deny"));
    assert_eq!(events[0]["body"]["state"], "denied");
    // cancel
    let cancelled = rig.propose("echo cancel", "no");
    let (_, events) = rig.run(|r| {
        r.core
            .cancel_operator_proposal(&r.ctx("dev-1"), cancelled["proposalId"].as_str().unwrap())
            .unwrap()
    });
    assert_eq!(events[0]["type"], "operator.decided");
    assert_eq!(events[0]["body"]["state"], "cancelled");
    // expire: one proposal-level sub-event per id, in row order
    let a = rig.propose("echo a", "x");
    let b = rig.propose("echo b", "y");
    let (expired, events) = rig.run(|r| {
        r.core
            .expire_operator_proposals(
                &r.ctx("owner"),
                &json!({"proposalTtlMinutes": 0.00001, "approvalTtlMinutes": 0.00001}),
            )
            .unwrap()
    });
    // The approved one is still open and ages out with the two new ones.
    assert_eq!(
        expired,
        json!([approved["proposalId"], a["proposalId"], b["proposalId"]])
    );
    assert_eq!(events.len(), 3);
    for (k, event) in events.iter().enumerate() {
        assert_eq!(event["type"], "operator.decided");
        assert_eq!(event["k"], k);
        assert_eq!(event["body"]["state"], "expired");
    }
    assert_eq!(events[2]["body"]["command"], "echo b");
}

#[test]
fn feed_operator_finished() {
    let rig = Rig::new();
    let proposal = rig.propose("ls -la", "look");
    rig.decide(&proposal, "approve");
    let id = proposal["proposalId"].clone();
    let (_, claimed) = rig.run(|r| {
        let mut input = LIMITS();
        input["proposalId"] = id.clone();
        r.core.claim_operator_run(&r.ctx("owner"), &input).unwrap()
    });
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0]["type"], "snapshot.invalidated");
    assert_eq!(claimed[0]["body"]["section"], "waiting");
    assert_eq!(claimed[0]["body"]["key"], "op:1");
    let (_, events) = rig.run(|r| {
        r.core
            .finish_operator_run(
                &r.ctx("owner"),
                &json!({"proposalId": id, "status": "ok", "exitCode": 0, "durationMs": 12, "outputTail": "done"}),
            )
            .unwrap()
    });
    let event = expect_type(&rig, &events, "operator.finished");
    assert_eq!(event["body"]["state"], "finished");
    assert_eq!(event["from"], "running");
    assert_eq!(event["body"]["command"], "ls -la");
    // abandon: a run left running when the controller stopped
    let second = rig.propose("echo again", "twice");
    rig.decide(&second, "approve");
    rig.run(|r| {
        let mut input = LIMITS();
        input["proposalId"] = second["proposalId"].clone();
        r.core.claim_operator_run(&r.ctx("owner"), &input).unwrap()
    });
    let (_, events) = rig.run(|r| {
        r.core
            .abandon_running_operator_runs(&r.ctx("owner"), None)
            .unwrap()
    });
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["type"], "operator.decided");
    assert_eq!(events[0]["body"]["state"], "abandoned");
    assert_eq!(events[0]["k"], 0);
}

// ---------------------------------------------------------------------------------------- reports, plans, designs

const ONE_PACKAGE: &str = r#"{"summary":"s","packages":[
    {"id":"only","title":"Only","owns":["src/only"],"acceptance":["works"],"estimate_hours":1}],
    "risks":[],"integration_order":["only"]}"#;

const BODY: &str = r#"{"summary":"s","packages":[
    {"id":"ui","title":"UI","owns":["design/ui"],"acceptance":["approved"],"estimate_hours":1,"design":true},
    {"id":"api","title":"API","owns":["src/api"],"acceptance":["works"],"estimate_hours":1,"depends_on":["ui"]},
    {"id":"docs","title":"Docs","owns":["docs/x"],"acceptance":["written"],"estimate_hours":1}],
    "risks":[],"integration_order":["ui","api","docs"]}"#;

impl Rig {
    pub fn pane(&self, agent: &str, branch: &str) {
        self.owner(
            "recordAgentPane",
            json!([{"agentId": agent, "workspaceId": "ws-1", "paneId": format!("pane-{agent}"),
                "worktreePath": format!("/tmp/wt-{agent}"), "branch": branch, "baseSha": BASE}]),
        );
    }

    pub fn report_as(&self, agent: &str, branch: &str, commit: &str, summary: &str) -> Value {
        self.core
            .record_agent_report(
                &self.ctx(agent),
                &json!({"commitSha": commit, "summary": summary, "evidence": {
                    "generation": 1, "branch": branch, "baseSha": BASE, "commitExists": true,
                    "isAncestorOfTip": true, "isAncestorOfBase": false, "checkedAt": "2026-01-01T00:00:00.000Z",
                    "branchTip": commit,
                }}),
            )
            .unwrap()
    }

    /// An approved plan of the architect arch-1 with the packages of `BODY`.
    pub fn approved_plan(&self) -> String {
        self.approved_plan_of(BODY)
    }

    pub fn approved_plan_of(&self, plan_body: &str) -> String {
        let plan = self
            .core
            .open_plan(
                &self.ctx("pm-1"),
                &json!({"tier": "normal", "title": "Plan one"}),
            )
            .unwrap();
        let plan_id = plan["planId"].as_str().unwrap().to_string();
        let body = serde_json::to_string(
            &capstan_kernel::plan_body::parse_plan_body(plan_body, 8).unwrap(),
        )
        .unwrap();
        self.core
            .submit_plan(
                &self.ctx("arch-1"),
                &json!({"planId": plan_id, "baseSha": BASE, "bodyJson": body, "review": false}),
            )
            .unwrap();
        plan_id
    }

    pub fn assign(&self, plan: &str, package: &str, agent: &str) -> Value {
        self.core
            .assign_package(
                &self.ctx("pm-1"),
                &json!({"planId": plan, "packageId": package, "agentId": agent}),
            )
            .unwrap()
    }
}

#[test]
fn feed_report_recorded() {
    let rig = Rig::new();
    rig.pane("dev-1", "feat/ui");
    let plan = rig.approved_plan();
    rig.assign(&plan, "ui", "dev-1");
    let (report, events) =
        rig.run(|r| r.report_as("dev-1", "feat/ui", COMMIT, "designed the screens"));
    let event = expect_type(&rig, &events, "report.recorded");
    assert_eq!(events.len(), 1);
    let body = &event["body"];
    assert_eq!(body["reportId"], report["record"]["reportId"]);
    assert_eq!(body["agentId"], "dev-1");
    assert_eq!(body["commitSha"], COMMIT);
    assert_eq!(body["summary"], "designed the screens");
    assert_eq!(body["textVerified"], true);
    assert_eq!(body["state"], "accepted");
    // The package is the design package, now; and the waiting design items are asked to refresh.
    assert_eq!(
        body["current"]["package"],
        json!({"planId": plan, "packageId": "ui", "design": true})
    );
    assert_eq!(body["current"]["readAt"], rig.head());
    assert_eq!(
        body["effects"],
        json!([
            {"section": "waiting", "op": "invalidate", "key": "design", "fields": {}},
            {"section": "plans", "op": "invalidate", "key": "plan", "fields": {}},
            {"section": "waiting", "op": "invalidate", "key": "message", "fields": {}},
        ])
    );
    // A non-design report: the package is not a design.
    let other_rig = Rig::new();
    other_rig.pane("dev-2", "feat/docs");
    let other_plan = other_rig.approved_plan();
    other_rig.assign(&other_plan, "docs", "dev-2");
    let (_, events) =
        other_rig.run(|r| r.report_as("dev-2", "feat/docs", COMMIT, "wrote the docs"));
    assert_eq!(events[0]["body"]["current"]["package"]["design"], false);
    // A report of an agent with no package has current.package null.
    let third = Rig::new();
    third.pane("dev-1", "feat/x");
    let (_, events) = third.run(|r| r.report_as("dev-1", "feat/x", COMMIT, "loose work"));
    assert_eq!(events[0]["body"]["current"]["package"], Value::Null);
    golden("report.recorded.no_assignment", &events[0]);
    // The duplicate of an accepted report writes a row with no state: skipped.
    let (_, duplicate) = third.run(|r| r.report_as("dev-1", "feat/x", COMMIT, "loose work"));
    assert!(duplicate.is_empty());
}

#[test]
fn a_report_event_replays_with_the_same_effects_after_its_agent_moves_to_another_package() {
    let rig = Rig::new();
    rig.pane("dev-1", "feat/ui");
    let plan = rig.approved_plan();
    rig.assign(&plan, "ui", "dev-1");
    let (_, before) = rig.run(|r| r.report_as("dev-1", "feat/ui", COMMIT, "designed"));
    let seq = before[0]["seq"].as_u64().unwrap();
    let replay = |rig: &Rig| {
        feed_page(
            &rig.core,
            Position {
                seq: seq - 1,
                k: u32::MAX - 1,
            },
            1,
            None,
        )
        .unwrap()
        .events[0]
            .clone()
    };
    assert_eq!(replay(&rig), before[0]);
    // The agent is moved to a package of another plan: only body.current moves.
    let other = rig.approved_plan();
    rig.assign(&other, "docs", "dev-1");
    let after = replay(&rig);
    assert_eq!(after["body"]["effects"], before[0]["body"]["effects"]);
    assert_eq!(after["type"], before[0]["type"]);
    assert_ne!(after["body"]["current"], before[0]["body"]["current"]);
    let mut stripped_before = before[0].clone();
    let mut stripped_after = after.clone();
    stripped_before["body"]
        .as_object_mut()
        .unwrap()
        .remove("current");
    stripped_after["body"]
        .as_object_mut()
        .unwrap()
        .remove("current");
    assert_eq!(stripped_before, stripped_after);
}

#[test]
fn feed_design_decided() {
    let rig = Rig::new();
    rig.pane("dev-1", "feat/ui");
    let plan = rig.approved_plan();
    rig.assign(&plan, "ui", "dev-1");
    let report = rig.report_as("dev-1", "feat/ui", COMMIT, "designed");
    let (_, events) = rig.run(|r| {
        r.core
            .record_design_decision(
                &r.ctx("pm-1"),
                &json!({"planId": plan, "packageId": "ui", "decision": "approved", "link": "https://example.test/ui", "askedUser": "ok?"}),
            )
            .unwrap()
    });
    let event = expect_type(&rig, &events, "design.decided");
    let body = &event["body"];
    assert_eq!(body["planId"], json!(plan));
    assert_eq!(body["packageId"], "ui");
    assert_eq!(body["decision"], "approved");
    assert_eq!(body["reportId"], report["record"]["reportId"]);
    assert_eq!(body["link"], "https://example.test/ui");
    assert_eq!(body["effects"][0]["key"], "design:1/ui");
    assert_eq!(body["effects"][1]["key"], "1");
    assert_eq!(body["effects"][1]["section"], "plans");
}

fn plan_body_json() -> String {
    serde_json::to_string(&capstan_kernel::plan_body::parse_plan_body(BODY, 8).unwrap()).unwrap()
}

#[test]
fn feed_plan_changed() {
    let rig = Rig::new();
    let (plan, events) = rig.run(|r| {
        r.core
            .open_plan(
                &r.ctx("pm-1"),
                &json!({"tier": "normal", "title": "Plan one"}),
            )
            .unwrap()
    });
    let plan_id = plan["planId"].as_str().unwrap().to_string();
    let event = expect_type(&rig, &events, "plan.changed");
    assert_eq!(event["body"]["state"], "draft");
    assert_eq!(event["body"]["planId"], json!(plan_id));
    let approved = rig.approved_plan();
    let (_, assigned) = rig.run(|r| r.assign(&approved, "docs", "dev-2"));
    assert_eq!(assigned[0]["type"], "plan.changed");
    assert_eq!(assigned[0]["body"]["packageId"], "docs");
    let (_, cancelled) = rig.run(|r| {
        r.core
            .cancel_plan(&r.ctx("owner"), &json!({"planId": approved}))
            .unwrap()
    });
    assert_eq!(cancelled[0]["type"], "plan.changed");
    assert!(
        has_kind_invalidate(&cancelled[0], "integration"),
        "a cancelled plan supersedes integrations"
    );
    assert!(!has_kind_invalidate(&assigned[0], "integration"));
}

/// Whether the event asks for the waiting items of `kind` again (the kind-level key).
fn has_kind_invalidate(event: &Value, kind: &str) -> bool {
    event["body"]["effects"].as_array().is_some_and(|effects| {
        effects
            .iter()
            .any(|e| e["section"] == "waiting" && e["op"] == "invalidate" && e["key"] == kind)
    })
}

#[test]
fn feed_plan_submitted() {
    let rig = Rig::new();
    let plan = rig
        .core
        .open_plan(
            &rig.ctx("pm-1"),
            &json!({"tier": "normal", "title": "Plan one"}),
        )
        .unwrap();
    let plan_id = plan["planId"].as_str().unwrap().to_string();
    let (_, events) = rig.run(|r| {
        r.core
            .submit_plan(
                &r.ctx("arch-1"),
                &json!({"planId": plan_id, "baseSha": BASE, "bodyJson": plan_body_json(), "review": true}),
            )
            .unwrap()
    });
    let event = expect_type(&rig, &events, "plan.submitted");
    assert_eq!(event["body"]["state"], "in_review");
    assert_eq!(event["body"]["planId"], json!(plan_id));
    assert_eq!(
        event["body"]["effects"],
        json!([
            {"section": "plans", "op": "invalidate", "key": "1", "fields": {}},
            {"section": "waiting", "op": "invalidate", "key": "message", "fields": {}},
        ])
    );
}

#[test]
fn feed_plan_approved() {
    let rig = Rig::new();
    let plan = rig
        .core
        .open_plan(
            &rig.ctx("pm-1"),
            &json!({"tier": "normal", "title": "Plan one"}),
        )
        .unwrap();
    let plan_id = plan["planId"].as_str().unwrap().to_string();
    let (_, events) = rig.run(|r| {
        r.core
            .submit_plan(
                &r.ctx("arch-1"),
                &json!({"planId": plan_id, "baseSha": BASE, "bodyJson": plan_body_json(), "review": false}),
            )
            .unwrap()
    });
    let event = expect_type(&rig, &events, "plan.approved");
    assert_eq!(event["body"]["state"], "approved");
    assert_eq!(event["body"]["effects"][0]["key"], "1");
    assert_eq!(event["from"], "draft");
}

// ---------------------------------------------------------------------- reviews, integrations, sign-off, verification

impl Rig {
    pub fn review(&self, subject: &str, verdict: &str, text: &str) -> (Value, Vec<Value>) {
        let head = self.head();
        self.core
            .begin_review(
                &self.ctx("pm-1"),
                &json!({"subjectId": subject, "reviewerRole": "reviewer", "reviewerAgentId": "rev-1"}),
            )
            .unwrap();
        let done = self
            .core
            .complete_review(
                &self.ctx("rev-1"),
                &json!({"verdict": verdict, "text": text}),
            )
            .unwrap();
        (done, self.since(head))
    }

    pub fn begin_integration(&self, id: &str, reports: &[&str]) {
        self.owner(
            "beginIntegration",
            json!([{"integrationId": id, "requestedBy": "pm-1", "baseSha": BASE, "reportIds": reports,
                "branch": format!("capstan/integration/{id}")}]),
        );
    }

    pub fn finish_integration(&self, id: &str, outcome: Value) {
        self.owner(
            "finishIntegration",
            json!([{"integrationId": id, "outcome": outcome}]),
        );
    }
}

#[test]
fn feed_review_verdict() {
    let rig = Rig::new();
    rig.pane("dev-1", "feat/x");
    let report = rig.report_as("dev-1", "feat/x", COMMIT, "done");
    let report_id = report["record"]["reportId"].as_str().unwrap().to_string();
    let (_, events) = rig.review(&report_id, "findings", "fix the thing");
    let event = expect_type(&rig, &events, "review.verdict");
    assert_eq!(event["body"]["verdict"], "findings");
    assert_eq!(event["from"], "started");
    assert_eq!(
        event["body"]["effects"],
        json!([{"section": "waiting", "op": "invalidate", "key": "message", "fields": {}}])
    );
    let (_, again) = rig.review(&report_id, "pass", "fine now");
    let verdicts = events_of(&again, "review.verdict");
    assert_eq!(verdicts[0]["body"]["verdict"], "pass");
}

/// A one-package plan whose report passed review and is in a merged integration `int-1`.
fn merged_integration(rig: &Rig) -> (String, String, Vec<Value>, Vec<Value>) {
    rig.pane("dev-1", "feat/only");
    let plan = rig.approved_plan_of(ONE_PACKAGE);
    rig.assign(&plan, "only", "dev-1");
    let report = rig.report_as("dev-1", "feat/only", COMMIT, "work written");
    let report_id = report["record"]["reportId"].as_str().unwrap().to_string();
    rig.review(&report_id, "pass", "fine");
    let (_, begun) = rig.run(|r| r.begin_integration("int-1", &[&report_id]));
    let (_, finished) = rig.run(|r| {
        r.finish_integration(
            "int-1",
            json!({"kind": "merged", "headSha": "3".repeat(40)}),
        )
    });
    (plan, report_id, begun, finished)
}

#[test]
fn feed_integration_finished() {
    let rig = Rig::new();
    let (_, _, begun, finished) = merged_integration(&rig);
    assert_eq!(begun.len(), 1);
    assert_eq!(begun[0]["type"], "snapshot.invalidated");
    assert_eq!(begun[0]["body"]["section"], "waiting");
    assert_eq!(begun[0]["body"]["key"], "integration:1");
    assert_eq!(
        begun[0]["body"]["effects"],
        json!([{"section": "waiting", "op": "invalidate", "key": "integration:1", "fields": {}}])
    );
    let event = expect_type(&rig, &finished, "integration.finished");
    assert_eq!(event["body"]["integrationId"], "int-1");
    assert_eq!(event["body"]["state"], "merged");
    // The plans are asked again (a merged package counts as integrated), then the notices, then the integration.
    assert_eq!(event["body"]["effects"][0]["key"], "plan");
    assert_eq!(event["body"]["effects"][1]["key"], "message");
    assert_eq!(event["body"]["effects"][2]["key"], "integration:1");
    assert!(
        has_kind_invalidate(event, "integration"),
        "a merge supersedes the earlier integrations of its plans"
    );
    let (_, settled) = rig.run(|r| {
        r.core
            .settle_integration(
                &r.ctx("owner"),
                &json!({"integrationId": "int-1", "outcome": "discarded"}),
            )
            .unwrap()
    });
    assert_eq!(settled[0]["type"], "snapshot.invalidated");
    assert_eq!(settled[0]["body"]["key"], "integration:1");
    assert!(has_kind_invalidate(&settled[0], "integration"));
}

#[test]
fn feed_plan_signed_off() {
    let rig = Rig::new();
    let (plan, _, _, _) = merged_integration(&rig);
    rig.review("int-1", "pass", "integration fine");
    let (_, signed) = rig.run(|r| {
        r.core
            .record_signoff(
                &r.ctx("arch-1"),
                &json!({"planId": plan, "integrationId": "int-1", "summary": "all merged"}),
            )
            .unwrap()
    });
    let event = expect_type(&rig, &signed, "plan.signed_off");
    assert_eq!(event["body"]["planId"], json!(plan));
    assert_eq!(event["body"]["integrationId"], "int-1");
    assert!(has_kind_invalidate(event, "integration"));
}

#[test]
fn feed_verify_result() {
    let rig = Rig::new();
    rig.pane("dev-1", "feat/x");
    let report = rig.report_as("dev-1", "feat/x", COMMIT, "done");
    let report_id = report["record"]["reportId"].as_str().unwrap().to_string();
    let queued = rig
        .core
        .queue_verify_run(
            &rig.ctx("dev-1"),
            &json!({"subjectId": report_id, "commands": ["make test"], "commandsSha": "cmd-sha", "trigger": "manual"}),
        )
        .unwrap();
    let run_id = queued["runId"].as_str().unwrap().to_string();
    let (_, claimed) = rig.run(|r| {
        r.core
            .claim_verify_run(
                &r.ctx("owner"),
                &json!({"runId": run_id, "scratchPath": "/tmp/s", "logPath": "/tmp/l", "maxParallel": 2}),
            )
            .unwrap()
    });
    assert!(claimed.is_empty(), "queue and claim are not public");
    let (_, events) = rig.run(|r| {
        r.core
            .finish_verify_run(
                &r.ctx("owner"),
                &json!({"runId": run_id, "state": "passed", "exitCode": 0, "durationMs": 1000, "outputTail": "ok"}),
            )
            .unwrap()
    });
    let event = expect_type(&rig, &events, "verify.result");
    assert_eq!(event["body"]["runId"], json!(run_id));
    assert_eq!(event["body"]["result"], "passed");
    assert_eq!(event["from"], "running");
}

fn supervised() -> (Rig, Value, Vec<Value>) {
    let mut rig = Rig::new();
    rig.owner(
        "syncRoleDefinitions",
        json!([[
            {"name": "pm", "kind": "PM", "host": "claude", "configHash": "b".repeat(64)},
            {"name": "developer", "kind": "Developer", "host": "claude", "configHash": "a".repeat(64)},
            {"name": "architect", "kind": "Developer", "host": "claude", "configHash": "e".repeat(64)},
            {"name": "reviewer", "kind": "Verifier", "host": "claude", "configHash": "f".repeat(64)},
            {"name": "supervisor", "kind": "Supervisor", "host": "claude", "configHash": "c".repeat(64)},
        ]]),
    );
    rig.add_agent("sup-1", "supervisor", "Supervisor");
    let (finding, events) = rig.run(|r| {
        r.core
            .raise_finding(
                &r.ctx("sup-1"),
                &json!({"targetAgentId": "dev-1", "severity": "high", "evidence": "it skipped the tests",
                    "correction": "run them", "doneWhen": "tests ran"}),
            )
            .unwrap()
    });
    (rig, finding, events)
}

#[test]
fn feed_finding_raised() {
    let (rig, finding, events) = supervised();
    let event = expect_type(&rig, &events, "finding.raised");
    assert_eq!(event["body"]["findingId"], finding["findingId"]);
    assert_eq!(event["body"]["state"], "open");
    assert_eq!(event["body"]["severity"], "high");
}

#[test]
fn feed_finding_resolved() {
    let (rig, finding, _) = supervised();
    // The delivery to the target must be acknowledged before the Supervisor checks.
    let messages = rig.core.messages_for("dev-1").unwrap();
    let delivery = messages.as_array().unwrap().last().unwrap()["messageId"]
        .as_str()
        .unwrap()
        .to_string();
    rig.core.record_sent(&rig.ctx("owner"), &delivery).unwrap();
    rig.core.ack_message(&rig.ctx("dev-1"), &delivery).unwrap();
    let (_, checked) = rig.run(|r| {
        r.core
            .check_finding(
                &r.ctx("sup-1"),
                &json!({"findingId": finding["findingId"], "result": "resolved", "evidence": "tests ran now"}),
            )
            .unwrap()
    });
    let event = expect_type(&rig, &checked, "finding.resolved");
    assert_eq!(event["body"]["state"], "resolved");
    assert_eq!(event["from"], "open");
}

// ---------------------------------------------------------------------------------------------- agents and the run

#[test]
fn feed_agent_registered() {
    let mut rig = Rig::new();
    let head = rig.head();
    let before = rig.core.state_version().unwrap();
    rig.add_agent("dev-3", "developer", "Developer");
    assert!(rig.core.state_version().unwrap() > before);
    let events = rig.since(head);
    // The seat and actor rows are skipped; the registration is public.
    assert_eq!(events.len(), 1, "{events:#?}");
    let event = expect_type(&rig, &events, "agent.registered");
    let body = &event["body"];
    assert_eq!(body["agentId"], "dev-3");
    assert_eq!(body["kind"], "Developer");
    assert_eq!(body["role"], "developer");
    assert_eq!(body["state"], "active");
    assert_eq!(body["paused"], false);
    assert_eq!(body["generation"], 1);
    assert_eq!(body["effects"][0]["op"], "upsert");
    assert_eq!(body["effects"][0]["key"], "dev-3");
    assert_eq!(body["effects"][0]["fields"]["kind"], "Developer");
    assert_eq!(body["effects"][0]["fields"]["role"], "developer");
}

#[test]
fn feed_agent_paused() {
    let rig = Rig::new();
    let (_, paused) = rig.run(|r| {
        r.core
            .pause_agent(
                &r.ctx("pm-1"),
                &json!({"agentId": "dev-1", "reason": "hold on"}),
            )
            .unwrap()
    });
    let event = expect_type(&rig, &paused, "agent.paused");
    assert_eq!(event["body"]["paused"], true);
    assert_eq!(event["body"]["reason"], "hold on");
    assert_eq!(
        event["body"]["effects"],
        json!([{"section": "agents", "op": "invalidate", "key": "dev-1", "fields": {}}])
    );
}

#[test]
fn feed_agent_resumed() {
    let rig = Rig::new();
    rig.core
        .pause_agent(
            &rig.ctx("pm-1"),
            &json!({"agentId": "dev-1", "reason": "hold on"}),
        )
        .unwrap();
    let (_, resumed) = rig.run(|r| {
        r.core
            .resume_agent(
                &r.ctx("pm-1"),
                &json!({"agentId": "dev-1", "reason": "go on"}),
            )
            .unwrap()
    });
    let event = expect_type(&rig, &resumed, "agent.resumed");
    assert_eq!(event["body"]["paused"], false);
    assert_eq!(event["from"], "paused");
}

#[test]
fn feed_agent_lost() {
    let rig = Rig::new();
    let (_, lost) = rig.run(|r| {
        r.core
            .record_agent_lost(
                &r.ctx("owner"),
                &json!({"agentId": "dev-1", "reason": "pane_gone"}),
            )
            .unwrap()
    });
    // The batch row is the loss; the main row (no states) and the notice message are not.
    assert_eq!(events_of(&lost, "agent.lost").len(), 1, "{lost:#?}");
    let event = expect_type(&rig, &lost, "agent.lost");
    assert_eq!(event["body"]["agentId"], "dev-1");
    assert_eq!(event["body"]["generation"], 1);
    assert_eq!(event["body"]["reason"], "pane_gone");
}

#[test]
fn feed_agent_ended() {
    let rig = Rig::new();
    let (_, ended) = rig.run(|r| r.core.end_agent(&r.ctx("owner"), "dev-1", None).unwrap());
    let event = expect_type(&rig, &ended, "agent.ended");
    assert_eq!(event["body"]["state"], "ended");
    assert_eq!(event["from"], "active");
}

#[test]
fn feed_agent_ended_closes_a_pause_with_a_resume_row() {
    let rig = Rig::new();
    rig.core
        .pause_agent(
            &rig.ctx("pm-1"),
            &json!({"agentId": "dev-2", "reason": "hold"}),
        )
        .unwrap();
    let (_, events) = rig.run(|r| r.core.end_agent(&r.ctx("owner"), "dev-2", None).unwrap());
    let types: Vec<&str> = events.iter().map(|e| e["type"].as_str().unwrap()).collect();
    assert_eq!(types, ["agent.resumed", "agent.ended"], "{events:#?}");
}

#[test]
fn feed_agent_replacement() {
    let mut rig = Rig::new();
    rig.add_agent("dev-3", "developer", "Developer");
    let write_once_before = write_once_snapshot(&rig.core, "agents", "dev-1").unwrap();
    let (_, events) = rig.run(|r| {
        r.core
            .replace_agent_generation(&r.ctx("owner"), "dev-1")
            .unwrap()
    });
    assert_eq!(events.len(), 1, "{events:#?}");
    assert_eq!(events[0]["type"], "snapshot.invalidated");
    assert_eq!(events[0]["body"]["section"], "agents");
    assert_eq!(events[0]["body"]["key"], "dev-1");
    assert_eq!(events[0]["from"], "1");
    assert_eq!(events[0]["to"], "2");
    check_event(&events[0], &project_of(&rig));
    assert_eq!(
        write_once_snapshot(&rig.core, "agents", "dev-1").unwrap(),
        write_once_before
    );
    // The chain record of a replaced agent: end, then record.
    rig.core
        .end_agent(&rig.ctx("owner"), "dev-2", None)
        .unwrap();
    let (_, replaced) = rig.run(|r| {
        r.core
            .record_agent_replaced(
                &r.ctx("owner"),
                &json!({"predecessorId": "dev-2", "successorId": "dev-3"}),
            )
            .unwrap()
    });
    assert_eq!(replaced[0]["type"], "snapshot.invalidated");
    assert_eq!(replaced[0]["body"]["key"], "dev-2");
}

#[test]
fn feed_agent_blocked() {
    let rig = Rig::new();
    let message = rig.enqueue("dev-1", "are you there?", true);
    let (_, events) = rig.run(|r| {
        r.core
            .queue_input_blocked_notice(
                &r.ctx("owner"),
                &json!({"agentId": "dev-1", "messageId": message, "blocker": "permission_prompt"}),
            )
            .unwrap()
    });
    let event = expect_type(&rig, &events, "agent.blocked");
    assert_eq!(event["body"]["agentId"], "dev-1");
    assert_eq!(event["body"]["state"], "blocked");
    assert_eq!(event["body"]["blocker"], "permission_prompt");
}

#[test]
fn feed_plan_needs_attention() {
    let rig = Rig::new();
    let (_, events) = rig.run(|r| {
        r.core
            .queue_attention_notices(
                &r.ctx("owner"),
                &json!([{"kind": "stalled", "agentId": "dev-1", "episodeMs": 1000}, {"kind": "blocked", "agentId": "dev-2", "episodeMs": 2000}]),
            )
            .unwrap()
    });
    let event = expect_type(&rig, &events, "plan.needs_attention");
    assert_eq!(events.len(), 2);
    assert_eq!(event["body"]["planId"], Value::Null);
    assert_eq!(event["body"]["state"], "stalled");
    assert_eq!(event["body"]["agentId"], "dev-1");
    assert_eq!(
        event["body"]["effects"],
        json!([{"section": "waiting", "op": "invalidate", "key": "message", "fields": {}}])
    );
    assert_eq!(events[1]["k"], 1);
    assert!(events[1]["id"].as_str().unwrap().ends_with(":1"));
    assert_eq!(events[1]["body"]["state"], "blocked");
}

#[test]
fn feed_run_changed() {
    let rig = Rig::new();
    let (_, paused) = rig.run(|r| {
        r.core
            .transition_run(&r.ctx("owner"), &json!("paused"), Some("maintenance"))
            .unwrap()
    });
    let event = expect_type(&rig, &paused, "run.changed");
    assert_eq!(event["body"]["state"], "paused");
    assert_eq!(event["body"]["paused"], true);
    assert_eq!(
        event["body"]["effects"],
        json!([{"section": "run", "op": "invalidate", "key": "run", "fields": {}}])
    );
    let (_, resumed) = rig.run(|r| {
        r.core
            .transition_run(&r.ctx("owner"), &json!("active"), Some("back"))
            .unwrap()
    });
    assert_eq!(resumed[0]["body"]["paused"], false);
    assert_eq!(resumed[0]["from"], "paused");
}

#[test]
fn feed_snapshot_invalidated() {
    let rig = Rig::new();
    rig.pane("dev-1", "feat/x");
    let (_, events) = rig.run(|r| {
        r.owner(
            "recordAgentPane",
            json!([{"agentId": "dev-1", "workspaceId": "ws-1", "paneId": "pane-dev-1", "worktreePath": "/tmp/wt-dev-1",
                "branch": "feat/x", "baseSha": BASE, "taskRef": "PM-1", "taskTitle": "Do the thing"}]),
        )
    });
    let event = expect_type(&rig, &events, "snapshot.invalidated");
    assert_eq!(event["body"]["section"], "agents");
    assert_eq!(event["body"]["key"], "dev-1");
    assert_eq!(event["entity"]["type"], "agent_pane");
}

// ------------------------------------------------------------------------- text that does not match its hash

impl Rig {
    /// Edits the closed ledger with `sql` (the guard triggers named in `drop_triggers` dropped first) and opens it again.
    pub fn tamper(&mut self, drop_triggers: &[&str], sql: &str, seed: &str) {
        self.core.close();
        let db = self.db();
        for trigger in drop_triggers {
            db.execute(&format!("DROP TRIGGER {trigger}"), []).unwrap();
        }
        db.execute(sql, []).unwrap();
        drop(db);
        self.core = open_seeded(&self.state, seed);
    }

    pub fn events_from_start(&self) -> Vec<Value> {
        self.since(0)
    }
}

#[test]
fn an_entity_row_whose_text_no_longer_matches_its_hash_gives_no_text() {
    let mut rig = Rig::new();
    let id = rig.enqueue("dev-1", "the original words", true);
    let before = rig.events_from_start();
    let original = events_of(&before, "message.action_needed")[0].clone();
    assert_eq!(original["body"]["textVerified"], true);
    rig.tamper(
        &[],
        "UPDATE messages SET body = 'someone else wrote this'",
        "tamper-message",
    );
    let events = rig.events_from_start();
    let event = events_of(&events, "message.action_needed")[0];
    assert_eq!(event["body"]["textVerified"], false);
    assert_eq!(event["body"]["body"], Value::Null);
    assert_eq!(event["body"]["messageId"], json!(id));
    assert_eq!(event["body"]["recipientAgentId"], "dev-1");
    golden("message.action_needed.corrupt_body", event);
    // Event-time fields and effects are what they were.
    assert_eq!(event["body"]["effects"], original["body"]["effects"]);
    assert_eq!(event["seq"], original["seq"]);
    // A later state change of the same message is unverified too: the ledger hash is the creation row's.
    rig.core.record_sent(&rig.ctx("owner"), &id).unwrap();
    let sent = events_of(&rig.since(event["seq"].as_u64().unwrap()), "message.state")[0].clone();
    assert_eq!(sent["body"]["textVerified"], false);
    assert_eq!(sent["body"]["body"], Value::Null);
}

const PROMPT_TRIGGER: &str = "prompt_relays_identity_is_immutable";

#[test]
fn a_corrupted_prompt_text_is_left_out_with_its_stored_hash_unchanged() {
    let mut rig = Rig::new();
    rig.relay_on();
    let relay = rig.capture("dev-1", "Allow Bash(ls)?");
    rig.tamper(
        &[PROMPT_TRIGGER],
        "UPDATE prompt_relays SET prompt_text = 'Allow Bash(rm -rf /)?'",
        "tamper-text",
    );
    let sha: String = rig
        .db()
        .query_row("SELECT prompt_sha FROM prompt_relays", [], |r| r.get(0))
        .unwrap();
    assert_eq!(json!(sha), relay["promptSha"]);
    let events = rig.events_from_start();
    let event = events_of(&events, "prompt.waiting")[0];
    assert_eq!(event["body"]["textVerified"], false);
    assert_eq!(event["body"]["text"], Value::Null);
    assert_eq!(event["body"]["options"], Value::Null);
    golden("prompt.waiting.corrupt_text", event);
}

#[test]
fn corrupted_prompt_options_are_left_out_with_their_stored_hash_unchanged() {
    let mut rig = Rig::new();
    rig.relay_on();
    rig.capture("dev-1", "Allow Bash(ls)?");
    rig.tamper(
        &[PROMPT_TRIGGER],
        r#"UPDATE prompt_relays SET options_json = '[{"number":1,"text":"Yes, always","acceptsText":false,"widensPermissions":true}]'"#,
        "tamper-options",
    );
    let events = rig.events_from_start();
    let event = events_of(&events, "prompt.waiting")[0];
    assert_eq!(event["body"]["textVerified"], false);
    assert_eq!(event["body"]["text"], Value::Null);
    assert_eq!(event["body"]["options"], Value::Null);
    golden("prompt.waiting.corrupt_options", event);
}

#[test]
fn a_prompt_hashed_over_the_pane_and_host_verifies_in_full() {
    let rig = Rig::new();
    rig.relay_on();
    let options =
        json!([{"number": 1, "text": "Yes", "acceptsText": false, "widensPermissions": true}]);
    let sha = prompt_sha("dev-1", "pane-77", "codex", "Allow it?", &options);
    rig.core
        .record_prompt_capture(
            &rig.ctx("pm-1"),
            &json!({"prompt": {"agentId": "dev-1", "paneId": "pane-77", "hostKind": "codex", "text": "Allow it?",
                "options": options, "promptSha": sha}}),
        )
        .unwrap();
    let events = rig.events_from_start();
    let event = events_of(&events, "prompt.waiting")[0];
    assert_eq!(event["body"]["textVerified"], true);
    assert_eq!(event["body"]["options"], options);
    golden("prompt.waiting.full_hash", event);
    // A hash that leaves the pane or the host out does not verify.
    let wrong = prompt_sha("dev-2", "", "", "Allow it?", &options);
    rig.core
        .record_prompt_capture(
            &rig.ctx("pm-1"),
            &json!({"prompt": {"agentId": "dev-2", "paneId": "pane-1", "hostKind": "claude", "text": "Allow it?",
                "options": options, "promptSha": wrong}}),
        )
        .unwrap();
    let events = rig.events_from_start();
    let waiting = events_of(&events, "prompt.waiting");
    assert_eq!(waiting[1]["body"]["textVerified"], false);
}

#[test]
fn a_corrupted_operator_command_is_left_out_and_a_restart_command_verifies() {
    let mut rig = Rig::new();
    rig.propose("ls -la", "look around");
    let restart = rig
        .core
        .propose_operator_action(
            &rig.ctx("dev-1"),
            &json!({"kind": "restart", "forceRestart": true, "reason": "stuck", "maxPending": 20}),
        )
        .unwrap();
    let events = rig.events_from_start();
    let proposed = events_of(&events, "operator.proposed");
    assert_eq!(proposed[1]["body"]["kind"], "restart");
    assert_eq!(proposed[1]["body"]["textVerified"], true);
    assert_eq!(proposed[1]["body"]["command"], restart["command"]);
    golden("operator.proposed.restart", proposed[1]);
    rig.tamper(
        &["operator_proposals_identity_is_immutable"],
        "UPDATE operator_proposals SET command = 'rm -rf /' WHERE proposal_id = 'op-1'",
        "tamper-command",
    );
    let events = rig.events_from_start();
    let proposed = events_of(&events, "operator.proposed");
    assert_eq!(proposed[0]["body"]["textVerified"], false);
    assert_eq!(proposed[0]["body"]["command"], Value::Null);
    assert_eq!(proposed[0]["body"]["reason"], "look around");
    golden("operator.proposed.corrupt_command", proposed[0]);
    // The restart proposal is untouched.
    assert_eq!(proposed[1]["body"]["textVerified"], true);
}

#[test]
fn replaying_a_row_after_later_transitions_gives_the_same_event() {
    let rig = Rig::new();
    rig.relay_on();
    let relay = rig.capture("dev-1", "Allow Bash(ls)?");
    let first = rig.events_from_start();
    let waiting = events_of(&first, "prompt.waiting")[0].clone();
    // The relay is answered, then captured again, then expires.
    rig.core
        .begin_prompt_answer(
            &rig.ctx("pm-1"),
            &json!({"relayId": relay["relayId"], "hash": relay["promptSha"], "answer": {"kind": "option", "number": 1}}),
        )
        .unwrap();
    rig.core
        .finish_prompt_answer(
            &rig.ctx("owner"),
            &json!({"relayId": relay["relayId"], "outcome": {"typed": true, "keys": ["1"]}}),
        )
        .unwrap();
    rig.capture("dev-1", "Allow Bash(pwd)?");
    let seq = waiting["seq"].as_u64().unwrap();
    let again = feed_page(
        &rig.core,
        Position {
            seq: seq - 1,
            k: u32::MAX - 1,
        },
        1,
        None,
    )
    .unwrap()
    .events[0]
        .clone();
    assert_eq!(again, waiting);
    // A message row replays the same too.
    let id = rig.enqueue("dev-2", "hello", true);
    let created = events_of(&rig.events_from_start(), "message.action_needed")[0].clone();
    rig.core.record_sent(&rig.ctx("owner"), &id).unwrap();
    rig.core.ack_message(&rig.ctx("dev-2"), &id).unwrap();
    let seq = created["seq"].as_u64().unwrap();
    let replay = feed_page(
        &rig.core,
        Position {
            seq: seq - 1,
            k: u32::MAX - 1,
        },
        1,
        None,
    )
    .unwrap()
    .events[0]
        .clone();
    assert_eq!(replay, created);
}

// ------------------------------------------------------------------------------------- positions, anchors, bounds

impl Rig {
    /// `count` synthetic ledger rows after the head, written to the closed-over file directly (rows of a kind the feed
    /// skips or has to look up).
    pub fn insert_rows(
        &self,
        count: usize,
        entity_type: &str,
        entity_id: &str,
        action: &str,
        from: Option<&str>,
        to: Option<&str>,
    ) {
        let mut db = self.db();
        let (project, actor): (String, String) = db
            .query_row("SELECT project_id, actor_id FROM agents LIMIT 1", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        let first: i64 = db
            .query_row(
                "SELECT COALESCE(MAX(sequence), 0) + 1 FROM controller_events",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let payload = json!({"action": action, "payload": {}, "entityVersion": 0, "details": null})
            .to_string();
        let transaction = db.transaction().unwrap();
        {
            let mut insert = transaction
                .prepare(
                    "INSERT INTO controller_events(project_id, sequence, event_id, entity_type, entity_id, from_state, to_state,
                       state_version, actor_id, request_id, input_revision, payload_json, created_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, 0, ?, ?, 1, ?, '2026-01-01T00:00:00.000Z')",
                )
                .unwrap();
            for i in 0..count as i64 {
                insert
                    .execute(rusqlite::params![
                        project,
                        first + i,
                        format!("synthetic-{}", first + i),
                        entity_type,
                        entity_id,
                        from,
                        to,
                        actor,
                        format!("synthetic-request-{}", first + i),
                        payload,
                    ])
                    .unwrap();
            }
        }
        transaction.commit().unwrap();
    }
}

#[test]
fn twenty_thousand_internal_rows_are_scanned_within_the_row_budget() {
    let rig = Rig::new();
    let start = rig.head();
    rig.insert_rows(
        20_000,
        "actor",
        "someone",
        "actor.create",
        None,
        Some("active"),
    );
    let head = rig.head();
    assert_eq!(head, start + 20_000);
    let mut after = Position { seq: start, k: 0 };
    let mut pages = 0;
    loop {
        let page = feed_page(&rig.core, after, 500, None).unwrap();
        assert!(
            page.rows_read <= MAX_SCAN_ROWS,
            "{} rows read",
            page.rows_read
        );
        assert!(page.events.is_empty());
        assert!(page.scanned.seq > after.seq);
        pages += 1;
        after = page.scanned;
        if after.seq == head {
            assert!(!page.has_more);
            break;
        }
        assert!(page.has_more, "hasMore until the head");
    }
    assert!(pages >= 10);
}

#[test]
fn three_thousand_ordinary_message_rows_cost_at_most_the_row_budget_per_call() {
    let rig = Rig::new();
    let id = rig.enqueue("dev-1", "ordinary", false);
    let start = rig.head();
    rig.insert_rows(
        3000,
        "message",
        &id,
        "message.transition",
        Some("sent"),
        Some("acked"),
    );
    let page = feed_page(&rig.core, Position { seq: start, k: 0 }, 500, None).unwrap();
    assert!(page.rows_read <= MAX_SCAN_ROWS);
    assert!(page.events.is_empty());
    assert!(page.has_more);
    assert!(page.scanned.seq > start);
}

#[test]
fn feed_page_positions() {
    let rig = Rig::new();
    let start = rig.head();
    let ids: Vec<String> = (0..5)
        .map(|n| rig.enqueue("dev-1", &format!("message {n}"), true))
        .collect();
    let after = Position { seq: start, k: 0 };
    let first = feed_page(&rig.core, after, 2, None).unwrap();
    assert_eq!(first.events.len(), 2);
    assert!(first.has_more);
    assert_eq!(first.scanned.seq, start + 2);
    let second = feed_page(&rig.core, first.scanned, 2, None).unwrap();
    assert_eq!(second.events[0]["body"]["messageId"], json!(ids[2]));
    assert_eq!(second.events[0]["seq"], start + 3);
    let third = feed_page(&rig.core, second.scanned, 2, None).unwrap();
    assert_eq!(third.events.len(), 1);
    assert!(!third.has_more);
    // At the head nothing is examined: scanned stays, hasMore is false.
    let head = feed_page(&rig.core, third.scanned, 5, None).unwrap();
    assert!(head.events.is_empty());
    assert_eq!(head.scanned, third.scanned);
    assert!(!head.has_more);
    assert!(head.rows_read <= 2, "the row at `after` and its lookup");
    // limit 0 and 501 are invalid input
    for bad in [0, 501] {
        let error = feed_page(&rig.core, after, bad, None).unwrap_err();
        assert_eq!(error.name(), "TypeError");
    }
    // A row with 3 sub-events read one at a time: (seq,0), (seq,1), (seq,2), then the next row at (seq+1,0).
    let mut rig = rig;
    rig.add_agent("dev-3", "developer", "Developer");
    rig.core
        .configure_prompt_relay(&json!({"enabled": true, "captureTtlSeconds": 0.001}))
        .unwrap();
    for agent in ["dev-1", "dev-2", "dev-3"] {
        rig.capture(agent, "Allow?");
    }
    let row_start = rig.head();
    rig.core.expire_prompt_captures(&rig.ctx("owner")).unwrap();
    let expiry = row_start + 1;
    rig.enqueue("dev-2", "after the row", true);
    let mut at = Position {
        seq: row_start,
        k: 0,
    };
    for k in 0..3u32 {
        let page = feed_page(&rig.core, at, 1, None).unwrap();
        assert_eq!(page.events.len(), 1);
        assert_eq!(page.scanned, Position { seq: expiry, k });
        assert!(page.has_more);
        at = page.scanned;
    }
    let page = feed_page(&rig.core, at, 1, None).unwrap();
    assert_eq!(page.events[0]["type"], "message.action_needed");
    assert_eq!(
        page.scanned,
        Position {
            seq: expiry + 1,
            k: 0
        }
    );
    assert!(!page.has_more);
}

#[test]
fn feed_page_anchor_rules() {
    let rig = Rig::new();
    rig.enqueue("dev-1", "one", true);
    let head = rig.head();
    // after 0 takes no anchor
    let page = feed_page(&rig.core, Position { seq: 0, k: 0 }, 1, None).unwrap();
    assert!(page.anchor_ok && !page.ahead);
    assert_eq!(
        feed_page(
            &rig.core,
            Position { seq: 0, k: 0 },
            1,
            Some("0123456789abcdef")
        )
        .unwrap_err()
        .name(),
        "TypeError"
    );
    // after past the head
    let ahead = feed_page(
        &rig.core,
        Position {
            seq: head + 5,
            k: 0,
        },
        5,
        None,
    )
    .unwrap();
    assert!(ahead.ahead && !ahead.anchor_ok && ahead.events.is_empty());
    assert!(!ahead.has_more);
    // no anchor with after > 0 is unchecked
    let unchecked = feed_page(
        &rig.core,
        Position {
            seq: head - 1,
            k: 0,
        },
        5,
        None,
    )
    .unwrap();
    assert!(unchecked.anchor_ok && !unchecked.ahead);
    assert_eq!(unchecked.events.len(), 1);
    // the right anchor passes, a wrong one fails with no events
    let right = anchor(&rig.core, (head - 1) as i64).unwrap().unwrap();
    assert!(
        feed_page(
            &rig.core,
            Position {
                seq: head - 1,
                k: 0
            },
            5,
            Some(&right)
        )
        .unwrap()
        .anchor_ok
    );
    let wrong = feed_page(
        &rig.core,
        Position {
            seq: head - 1,
            k: 0,
        },
        5,
        Some("ffffffffffffffff"),
    )
    .unwrap();
    assert!(!wrong.anchor_ok && !wrong.ahead && wrong.events.is_empty());
}

fn anchor_at(rig: &Rig, seq: u64) -> Option<String> {
    anchor(&rig.core, seq as i64).unwrap()
}

#[test]
fn the_head_answer_carries_the_anchor_of_head() {
    let rig = Rig::new();
    rig.enqueue("dev-1", "one", true);
    let answer = feed_head(&rig.core).unwrap();
    assert_eq!(answer.anchor, anchor_at(&rig, answer.head));
    assert!(answer.anchor.is_some());
    assert_eq!(
        serde_json::to_value(&answer).unwrap()["anchor"],
        json!(answer.anchor)
    );
}

#[test]
fn a_page_carries_the_anchor_of_where_it_ends() {
    let mut rig = Rig::new();
    let start = rig.head();
    // Ends on an event.
    rig.enqueue("dev-1", "one", true);
    rig.enqueue("dev-1", "two", true);
    let page = feed_page(&rig.core, Position { seq: start, k: 0 }, 1, None).unwrap();
    assert_eq!(page.events.len(), 1);
    assert_eq!(page.anchor, anchor_at(&rig, page.scanned.seq));
    assert!(page.anchor.is_some());
    // Ends on a skipped row: rows of an action that is not public.
    let top = rig.head();
    rig.insert_rows(2, "agent", "dev-1", "agent.not_public_action", None, None);
    let page = feed_page(&rig.core, Position { seq: top, k: 0 }, 5, None).unwrap();
    assert!(page.events.is_empty());
    assert_eq!(page.scanned.seq, top + 2);
    assert_eq!(page.anchor, anchor_at(&rig, top + 2));
    assert!(page.anchor.is_some());
    // Ends in the middle of a row (k > 0).
    rig.add_agent("dev-3", "developer", "Developer");
    rig.core
        .configure_prompt_relay(&json!({"enabled": true, "captureTtlSeconds": 0.001}))
        .unwrap();
    for agent in ["dev-1", "dev-2", "dev-3"] {
        rig.capture(agent, "Allow?");
    }
    let row_start = rig.head();
    rig.core.expire_prompt_captures(&rig.ctx("owner")).unwrap();
    let page = feed_page(
        &rig.core,
        Position {
            seq: row_start,
            k: 0,
        },
        1,
        None,
    )
    .unwrap();
    assert_eq!(
        page.scanned,
        Position {
            seq: row_start + 1,
            k: 0
        }
    );
    let page = feed_page(&rig.core, page.scanned, 1, None).unwrap();
    assert_eq!(page.scanned.k, 1);
    assert_eq!(page.anchor, anchor_at(&rig, page.scanned.seq));
    // An ahead page and a page whose anchor does not match.
    let head = rig.head();
    let ahead = feed_page(
        &rig.core,
        Position {
            seq: head + 5,
            k: 0,
        },
        5,
        None,
    )
    .unwrap();
    assert!(ahead.ahead && ahead.anchor.is_none());
    let wrong = feed_page(
        &rig.core,
        Position {
            seq: head - 1,
            k: 0,
        },
        5,
        Some("ffffffffffffffff"),
    )
    .unwrap();
    assert!(!wrong.anchor_ok && !wrong.ahead);
    assert_eq!(wrong.anchor, anchor_at(&rig, head - 1));
    assert_eq!(
        serde_json::to_value(&wrong).unwrap()["anchor"],
        json!(wrong.anchor)
    );
    // At the head nothing is read and the anchor is the head's.
    let at_head = feed_page(&rig.core, Position { seq: head, k: 9 }, 5, None).unwrap();
    assert_eq!(at_head.anchor, anchor_at(&rig, head));
}

#[test]
fn from_the_start_the_first_page_has_no_anchor_to_check_and_gives_one() {
    let rig = Rig::new();
    rig.enqueue("dev-1", "one", true);
    let page = feed_page(&rig.core, Position { seq: 0, k: 0 }, 500, None).unwrap();
    assert!(page.anchor_ok && !page.ahead && !page.events.is_empty());
    assert_eq!(page.anchor, anchor_at(&rig, page.scanned.seq));
    assert!(feed_page(
        &rig.core,
        Position { seq: 0, k: 0 },
        1,
        Some("0000000000000000")
    )
    .is_err());
}

#[test]
fn six_hundred_events_resumed_with_the_anchor_of_the_previous_answer_pass_every_check() {
    let rig = Rig::new();
    let start = rig.head();
    for n in 0..600 {
        rig.enqueue("dev-1", &format!("message {n}"), true);
    }
    let mut after = Position { seq: 0, k: 0 };
    let mut carried: Option<String> = None;
    let mut seqs = Vec::new();
    let mut pages = 0;
    loop {
        let page = feed_page(&rig.core, after, 100, carried.as_deref()).unwrap();
        assert!(page.anchor_ok && !page.ahead, "page {pages}");
        seqs.extend(page.events.iter().map(|e| e["seq"].as_u64().unwrap()));
        pages += 1;
        after = page.scanned;
        carried = page.anchor.clone();
        if !page.has_more {
            break;
        }
    }
    assert!(pages > 1);
    let public: Vec<u64> = seqs.iter().copied().filter(|seq| *seq > start).collect();
    assert_eq!(public, (start + 1..=start + 600).collect::<Vec<u64>>());
    assert!(seqs.windows(2).all(|w| w[0] < w[1]), "in order, no repeats");
}

#[test]
fn a_ledger_restored_below_the_cursor_is_ahead_and_a_regrown_one_has_another_anchor() {
    let mut rig = Rig::new();
    for n in 0..5 {
        rig.enqueue("dev-1", &format!("before {n}"), true);
    }
    rig.core.close();
    rig.db()
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .unwrap();
    let copy_dir = tempfile::tempdir().unwrap();
    private(copy_dir.path());
    let copy = copy_dir.path().join("state");
    std::fs::create_dir(&copy).unwrap();
    private(&copy);
    std::fs::copy(
        rig.state.join("controller.sqlite"),
        copy.join("controller.sqlite"),
    )
    .unwrap();
    let taken = {
        rig.reopen("run-a2");
        rig.head()
    };
    for n in 0..5 {
        rig.enqueue("dev-1", &format!("after {n}"), true);
    }
    let top = rig.head();
    let cursor_anchor = anchor_at(&rig, top).unwrap();
    rig.core.close();

    let restored = open_seeded(&copy, "restored-a");
    assert_eq!(feed_head(&restored).unwrap().head, taken);
    let ahead = feed_page(
        &restored,
        Position { seq: top, k: 0 },
        5,
        Some(&cursor_anchor),
    )
    .unwrap();
    assert!(ahead.ahead && !ahead.anchor_ok && ahead.anchor.is_none());
    restored.close();
    let restored = open_seeded(&copy, "restored-b");
    for n in 0..8 {
        restored
            .enqueue_message(
                &context(&restored, &rig.cred("owner")),
                &json!({"recipientAgentId": "dev-1", "body": format!("other {n}"), "actionNeeded": true}),
            )
            .unwrap();
    }
    assert!(feed_head(&restored).unwrap().head >= top);
    let regrown = feed_page(
        &restored,
        Position { seq: top, k: 0 },
        5,
        Some(&cursor_anchor),
    )
    .unwrap();
    assert!(!regrown.anchor_ok && !regrown.ahead);
    let new_anchor = regrown.anchor.expect("the regrown ledger has the sequence");
    assert_ne!(new_anchor, cursor_anchor);
    restored.close();
}

#[test]
fn the_first_event_of_a_page_is_returned_even_when_it_alone_exceeds_the_byte_budget() {
    let rig = Rig::new();
    let start = rig.head();
    rig.enqueue("dev-1", &"x".repeat(3000), true);
    rig.enqueue("dev-1", &"y".repeat(3000), true);
    let page = feed_page_with(
        &rig.core,
        Position { seq: start, k: 0 },
        10,
        None,
        MAX_SCAN_ROWS,
        100,
    )
    .unwrap();
    assert_eq!(page.events.len(), 1);
    assert!(page.has_more);
    assert_eq!(page.scanned.seq, start + 1);
    // and the next page resumes at the second
    let next = feed_page_with(&rig.core, page.scanned, 10, None, MAX_SCAN_ROWS, 100).unwrap();
    assert_eq!(next.events[0]["body"]["body"], "y".repeat(3000));
    // a small row budget stops before the limit and says there is more
    let few = feed_page_with(
        &rig.core,
        Position { seq: 0, k: 0 },
        500,
        None,
        3,
        MAX_PAGE_BYTES,
    )
    .unwrap();
    assert!(few.rows_read <= 3);
    assert!(few.has_more);
}

#[test]
fn six_hundred_events_of_quotes_and_backslashes_stay_within_the_page_budget_and_resume() {
    let rig = Rig::new();
    let start = rig.head();
    for n in 0..600 {
        let ch = if n % 2 == 0 { "\"" } else { "\\" };
        rig.enqueue("dev-1", &ch.repeat(TEXT_CAP + 100), true);
    }
    let mut after = Position { seq: start, k: 0 };
    let mut ids = Vec::new();
    let mut pages = 0;
    loop {
        let page = feed_page(&rig.core, after, 500, None).unwrap();
        let bytes = serde_json::to_string(&page).unwrap().len();
        assert!(bytes <= MAX_PAGE_BYTES, "page of {bytes} bytes");
        for event in &page.events {
            assert!(event.to_string().len() <= MAX_EVENT_BYTES);
            assert_eq!(event["body"]["truncated"], true);
            ids.push(event["seq"].as_u64().unwrap());
        }
        pages += 1;
        after = page.scanned;
        if !page.has_more {
            break;
        }
    }
    assert!(pages > 1);
    let want: Vec<u64> = (start + 1..=start + 600).collect();
    assert_eq!(ids, want, "no loss, no duplicate, in order");
}

// ------------------------------------------------------------------------------------------------------- text rules

#[test]
fn clean_text_strips_control_bidi_zero_width_and_separator_characters() {
    for (input, expected) in CLEAN_CASES {
        assert_eq!(clean_text(input), *expected, "{input:?}");
    }
    let pattern = regex::Regex::new(UNSAFE_TEXT_PATTERN).unwrap();
    for c in [
        '\u{0}', '\u{1f}', '\u{7f}', '\u{9f}', '\u{200b}', '\u{200f}', '\u{202a}', '\u{202e}',
        '\u{2066}', '\u{2069}', '\u{feff}', '\u{2028}', '\u{2029}',
    ] {
        assert!(pattern.is_match(&c.to_string()), "{:?}", c);
        assert_eq!(clean_text(&format!("a{c}b")), "a b");
    }
    for c in ['a', ' ', '\u{a0}', '\u{e9}', '\u{4e2d}', '\u{1f600}'] {
        assert!(!pattern.is_match(&c.to_string()), "{:?}", c);
    }
}

#[test]
fn the_unsafe_text_pattern_is_the_one_cstan_watch_uses() {
    let watch = include_str!("../../../cstan/src/watch.rs");
    assert!(
        watch.contains("Regex::new(UNSAFE_TEXT_PATTERN)"),
        "cstan watch.rs does not build its regex from UNSAFE_TEXT_PATTERN"
    );
    assert!(
        !watch.contains(UNSAFE_TEXT_PATTERN),
        "cstan watch.rs holds a literal of the pattern"
    );
}

#[test]
fn agent_text_with_control_and_bidi_characters_over_the_cap_is_stripped_and_truncated() {
    let rig = Rig::new();
    rig.relay_on();
    let unit = "ab\u{1b}[31m\u{202e}\u{200b}\n\u{85}x";
    let mut text = String::new();
    while text.len() < 6000 {
        text.push_str(unit);
    }
    assert!(text.len() > TEXT_CAP && text.len() <= 8192);
    rig.capture("dev-1", &text);
    let events = rig.events_from_start();
    let event = events_of(&events, "prompt.waiting")[0];
    let shown = event["body"]["text"].as_str().unwrap();
    assert!(shown.len() <= TEXT_CAP);
    assert!(!shown
        .chars()
        .any(|c| c.is_control() || ('\u{202a}'..='\u{202e}').contains(&c) || c == '\u{200b}'));
    assert_eq!(event["body"]["truncated"], true);
    assert_eq!(event["body"]["textVerified"], true);
    assert!(event["body"]["untrusted"]
        .as_array()
        .unwrap()
        .contains(&json!("body.text")));
    golden("prompt.waiting.cleaned_and_truncated", event);
}

#[test]
fn prompt_options_over_the_count_or_the_length_are_capped_and_flagged() {
    let rig = Rig::new();
    rig.relay_on();
    let options: Vec<Value> = (1..=OPTIONS_CAP + 3)
        .map(|n| json!({"number": n, "text": "o".repeat(OPTION_TEXT_CAP + 50), "acceptsText": false, "widensPermissions": false}))
        .collect();
    rig.capture_with("dev-1", "Pick one", &Value::Array(options));
    let events = rig.events_from_start();
    let event = events_of(&events, "prompt.waiting")[0];
    let shown = event["body"]["options"].as_array().unwrap();
    assert_eq!(shown.len(), OPTIONS_CAP);
    assert!(shown
        .iter()
        .all(|o| o["text"].as_str().unwrap().len() == OPTION_TEXT_CAP));
    assert_eq!(event["body"]["truncated"], true);
    assert_eq!(event["body"]["textVerified"], true);
    // Few short options are not flagged.
    rig.capture_with("dev-2", "Pick", &yes_no());
    let events = rig.events_from_start();
    assert_eq!(
        events_of(&events, "prompt.waiting")[1]["body"]["truncated"],
        false
    );
}

#[test]
fn the_worst_event_of_each_text_type_stays_within_max_event_bytes() {
    let rig = Rig::new();
    rig.relay_on();
    let q = |n: usize| "\"".repeat(n);
    // message: the longest body, then the longest failure reason
    let id = rig.enqueue("dev-1", &q(16 * 1024), true);
    rig.core
        .record_failure(&rig.ctx("owner"), &id, &q(500))
        .unwrap();
    // prompt: the longest text, 32 options of long texts, then a refusal with the longest reason
    let options: Vec<Value> = (1..=OPTIONS_CAP)
        .map(|n| json!({"number": n, "text": q(400), "acceptsText": true, "widensPermissions": true}))
        .collect();
    let relay = rig.capture_with("dev-1", &q(8192), &Value::Array(options));
    rig.core
        .refuse_prompt_answer(
            &rig.ctx("pm-1"),
            &json!({"relayId": relay["relayId"], "answer": {"kind": "esc"}, "reason": q(1024)}),
        )
        .unwrap();
    // operator: the longest command and reason
    rig.core
        .propose_operator_action(
            &rig.ctx("dev-1"),
            &json!({"kind": "command", "command": q(8192), "reason": q(1024), "maxPending": 20}),
        )
        .unwrap();
    // agent and run: a pause reason
    rig.core
        .pause_agent(
            &rig.ctx("pm-1"),
            &json!({"agentId": "dev-1", "reason": q(500)}),
        )
        .unwrap();
    rig.core
        .transition_run(&rig.ctx("owner"), &json!("paused"), Some(&q(500)))
        .unwrap();
    // report: the longest summary
    rig.pane("dev-2", "feat/x");
    rig.report_as("dev-2", "feat/x", COMMIT, &q(1000));
    let events = rig.events_from_start();
    let mut biggest = BTreeMap::new();
    for event in &events {
        let size = event.to_string().len();
        assert!(size <= MAX_EVENT_BYTES, "{} is {size} bytes", event["type"]);
        let entry = biggest
            .entry(event["type"].as_str().unwrap().to_string())
            .or_insert(0);
        *entry = (*entry).max(size);
    }
    assert!(biggest["prompt.waiting"] > 20_000, "{biggest:?}");
    assert!(biggest["message.state"] > 8000 || biggest["message.action_needed"] > 8000);
    assert!(biggest["operator.proposed"] > 8000);
    assert!(biggest["report.recorded"] > 2000);
}

// -------------------------------------------------------------------------------- pauses, reconciliation on reopen

fn without_current(event: &Value) -> Value {
    let mut event = event.clone();
    event["body"].as_object_mut().unwrap().remove("current");
    event
}

#[test]
fn pause_resume_pause_gives_stable_invalidate_effects_for_the_agent_and_the_run() {
    let rig = Rig::new();
    let head = rig.head();
    let agent = |reason: &str, pause: bool| {
        let input = json!({"agentId": "dev-1", "reason": reason});
        if pause {
            rig.core.pause_agent(&rig.ctx("pm-1"), &input).unwrap();
        } else {
            rig.core.resume_agent(&rig.ctx("pm-1"), &input).unwrap();
        }
    };
    agent("first", true);
    agent("back", false);
    agent("second", true);
    for to in ["paused", "active", "paused"] {
        rig.core
            .transition_run(&rig.ctx("owner"), &json!(to), Some("run reason"))
            .unwrap();
    }
    let first_read = rig.since(head);
    let types: Vec<&str> = first_read
        .iter()
        .map(|e| e["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        types,
        [
            "agent.paused",
            "agent.resumed",
            "agent.paused",
            "run.changed",
            "run.changed",
            "run.changed"
        ]
    );
    // More history, then replay everything: type, effects and every event-time field are what they were.
    rig.enqueue("dev-2", "later", true);
    rig.core
        .resume_agent(
            &rig.ctx("pm-1"),
            &json!({"agentId": "dev-1", "reason": "done"}),
        )
        .unwrap();
    let replay = rig.since(head);
    for (old, new) in first_read.iter().zip(&replay) {
        assert_eq!(without_current(old), without_current(new));
        assert_eq!(old["body"]["effects"], new["body"]["effects"]);
        for effect in old["body"]["effects"].as_array().unwrap() {
            assert_eq!(effect["op"], "invalidate");
            assert_eq!(
                effect["fields"],
                json!({}),
                "no pause id or other mutable value in an effect"
            );
        }
        assert!(!old.to_string().contains("openPauseId"));
    }
    // Both pauses of the agent invalidate the same item; the run's rows the same run item.
    assert_eq!(
        first_read[0]["body"]["effects"],
        first_read[2]["body"]["effects"]
    );
    assert_eq!(
        first_read[3]["body"]["effects"],
        first_read[5]["body"]["effects"]
    );
    assert_eq!(first_read[2]["body"]["paused"], true);
    assert_eq!(first_read[4]["body"]["paused"], false);
}

fn seed_uncertain_assignment(rig: &Rig) {
    let db = rig.db();
    let (project, actor): (String, String) = db
        .query_row(
            "SELECT project_id, actor_id FROM agents WHERE agent_id = 'dev-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let at = "2026-01-01T00:00:00.000Z";
    db.execute(
        "INSERT INTO work_items(project_id, work_item_id, title, description, required_role, state, state_version, input_revision, created_by, created_at)
         VALUES (?, 'w-1', 't', 'd', 'Developer', 'running', 1, 1, ?, ?)",
        rusqlite::params![project, actor, at],
    )
    .unwrap();
    db.execute(
        "INSERT INTO assignments(project_id, assignment_id, work_item_id, seat_id, state, state_version, input_revision, active_generation, authority_state, created_by, created_at)
         VALUES (?, 'a-1', 'w-1', 'seat-dev-1', 'running', 1, 1, 1, 'active', ?, ?)",
        rusqlite::params![project, actor, at],
    )
    .unwrap();
    db.execute(
        "INSERT INTO assignment_attempts(project_id, assignment_id, attempt, generation, state, state_version, authority_state, created_at)
         VALUES (?, 'a-1', 1, 1, 'running', 1, 'active', ?)",
        rusqlite::params![project, at],
    )
    .unwrap();
    db.execute(
        "INSERT INTO commands(project_id, command_id, assignment_id, attempt, generation, command_type, payload_json, payload_hash, state, state_version, created_at, updated_at)
         VALUES (?, 'c-1', 'a-1', 1, 1, 'dispatch', '{}', ?, 'started', 1, ?, ?)",
        rusqlite::params![project, "c".repeat(64), at, at],
    )
    .unwrap();
}

#[test]
fn a_reopen_that_reconciles_raises_state_version_once_and_feed_reads_raise_it_no_further() {
    let mut rig = Rig::new();
    rig.core.close();
    seed_uncertain_assignment(&rig);
    let before_version = {
        let db = rig.db();
        db.query_row("SELECT state_version FROM projects", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap()
    };
    let head_before = {
        let db = rig.db();
        db.query_row("SELECT MAX(sequence) FROM controller_events", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap()
    };
    rig.core = open_seeded(&rig.state, "reopen-1");
    let version = rig.core.state_version().unwrap();
    assert_eq!(
        version,
        before_version + 1,
        "one reconciled assignment, one version"
    );
    assert_eq!(rig.head() as i64, head_before + 1);
    // The reconcile row is not a public event, and reading the feed writes nothing.
    assert!(rig.since(head_before as u64).is_empty());
    let page = feed_page(&rig.core, Position { seq: 0, k: 0 }, 500, None).unwrap();
    assert!(page
        .events
        .iter()
        .all(|e| e["seq"].as_i64().unwrap() <= head_before));
    assert_eq!(rig.core.state_version().unwrap(), version);
    for round in 0..2 {
        rig.reopen(&format!("reopen-more-{round}"));
        assert_eq!(
            rig.core.state_version().unwrap(),
            version,
            "a reopen with nothing to reconcile writes nothing"
        );
        feed_page(&rig.core, Position { seq: 0, k: 0 }, 500, None).unwrap();
        feed_head(&rig.core).unwrap();
        assert_eq!(rig.core.state_version().unwrap(), version);
    }
}

// ------------------------------------------------------------------------------------- the known actions of the code

const KNOWN_ACTIONS: &str = include_str!("fixtures/event_feed/known_actions.txt");

fn source_actions(source: &str) -> BTreeSet<String> {
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
    for pattern in &patterns {
        let regex = regex::Regex::new(pattern).unwrap();
        for captures in regex.captures_iter(source) {
            for group in captures.iter().skip(1).flatten() {
                found.insert(group.as_str().to_string());
            }
        }
    }
    found
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn known(source: &str) -> BTreeSet<String> {
    KNOWN_ACTIONS
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .filter_map(|line| line.split_once(' '))
        .filter(|(from, _)| *from == source)
        .map(|(_, action)| action.to_string())
        .collect()
}

#[test]
fn every_action_the_code_writes_is_in_the_known_file_and_every_known_action_is_classified() {
    let mut files = Vec::new();
    rust_sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    let mut in_code = BTreeSet::new();
    for file in files {
        in_code.extend(source_actions(&std::fs::read_to_string(file).unwrap()));
    }
    assert!(in_code.len() > 80, "the scan found only {}", in_code.len());
    let listed = known("code");
    let missing: Vec<&String> = in_code.difference(&listed).collect();
    assert!(
        missing.is_empty(),
        "actions in kernel/src missing from known_actions.txt (add them, then map or skip them by name): {missing:?}"
    );
    let all: BTreeSet<String> = KNOWN_ACTIONS
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .filter_map(|line| line.split_once(' ').map(|(_, action)| action.to_string()))
        .collect();
    for action in &all {
        assert!(
            action_is_mapped(action)
                || SKIPPED_ACTIONS.contains(&action.as_str())
                || LEGACY_ACTIONS.contains(&action.as_str()),
            "{action} is neither mapped, skipped by name nor legacy"
        );
    }
    // The lists do not carry names the file does not know.
    for action in SKIPPED_ACTIONS.iter().chain(LEGACY_ACTIONS) {
        assert!(
            all.contains(*action),
            "{action} is skipped or legacy but not in known_actions.txt"
        );
    }
    // Every rule of the mapping names only known actions.
    for rule in capstan_kernel::areas::event_feed::MAPPING {
        for action in rule.actions.split('|') {
            assert!(
                all.contains(action),
                "{action} of the mapping is not in known_actions.txt"
            );
        }
    }
}

#[test]
fn rows_of_unknown_and_legacy_actions_are_skipped_and_the_real_ledger_writes_only_known_actions() {
    let rig = Rig::new();
    let start = rig.head();
    rig.insert_rows(
        1,
        "message",
        "m-x",
        "message.unheard_of",
        Some("queued"),
        Some("sent"),
    );
    rig.insert_rows(
        1,
        "plan",
        "plan-x",
        "plan.old_spelling",
        None,
        Some("draft"),
    );
    rig.insert_rows(1, "agent", "dev-1", "", Some("active"), Some("ended"));
    assert!(rig.since(start).is_empty());
    // Every action the setup wrote is one the file knows.
    let all: BTreeSet<String> = KNOWN_ACTIONS
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .filter_map(|line| line.split_once(' ').map(|(_, action)| action.to_string()))
        .collect();
    for (_, _, action, _, _) in capstan_kernel::areas::event_feed::ledger_rows(&rig.core).unwrap() {
        if action.is_empty()
            || action.starts_with("message.unheard")
            || action.starts_with("plan.old")
        {
            continue;
        }
        assert!(
            all.contains(&action),
            "the ledger holds {action}, which known_actions.txt does not"
        );
    }
}

#[test]
fn every_v1_type_is_tested_by_name_or_dropped() {
    let source = include_str!("event_feed.rs");
    for ty in V1_TYPES {
        let name = format!("fn feed_{}(", ty.replace('.', "_"));
        let dropped = DROPPED
            .iter()
            .any(|(dropped, reason)| dropped == ty && !reason.is_empty());
        assert!(
            source.contains(&name) || dropped,
            "{ty} has no test {name} and is not in DROPPED"
        );
    }
    for (ty, _) in DROPPED {
        assert!(V1_TYPES.contains(ty));
    }
}
