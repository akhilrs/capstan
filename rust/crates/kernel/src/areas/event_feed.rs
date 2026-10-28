//! The kernel event feed: the public events a client reads from the ledger, in pages, and the snapshot effects each one
//! carries.
//!
//! A ledger row (`controller_events`) is internal. This module maps the rows that mean something to a client to public
//! events, one per row (or one per item of a row that lists several), in ledger order. Nothing is written: every function
//! takes `&Core` and reads, so a caller that needs `feed_page` to agree with the epoch and head runs it inside one job.
//!
//! # Positions
//! A row `seq` with `n` public sub-events has the positions `(seq, 0)` .. `(seq, n-1)`; a row with none has the single
//! position `(seq, 0)`. `(0, 0)` is before the ledger. `after` names the last position the caller consumed; reading starts
//! at the next one. `scanned` is the last position examined (an event returned or a row skipped).
//!
//! # The result types (serde, camelCase)
//! | type | fields |
//! |---|---|
//! | `FeedHead` | `projectId`, `epoch`, `head` |
//! | `FeedPage` | `projectId`, `epoch`, `head`, `anchorOk`, `ahead`, `events`, `scanned {seq, k}`, `hasMore`, `rowsRead` |
//!
//! # Events
//! Every event is `{id "<projectId>:<epoch>:<seq>[:<k>]", seq, k, ts, type, actorId, entity {type, id}, from, to, body}`.
//! The id has `:<k>` only for the rows that list several items (`prompt_relay.expire`, `operator.expire`, ...). Fields that
//! are fixed when the row is written (seq, ts, actor, entity id, states, hashes, write-once text) never change on replay;
//! values that can move on live only under `body.current`, which carries `readAt` (the head the read was made at), and
//! never decide the type or the effects of an event.
//!
//! Text goes through `textVerified`: the text of the entity row is hashed again the way its writer hashed it and compared
//! with the hash the ledger row of the writer holds; on a mismatch (or no ledger hash) the text is left out and
//! `textVerified` is false. Text that is shown is cleaned (`helpers::clean_text`), capped at 4096 bytes (options at 32
//! entries of 256 bytes), flagged by `body.truncated` and its paths are listed in `body.untrusted`.
//!
//! `plan.needs_attention` comes from `pm.attention`, which lists agent episodes (stalled or blocked), not plans: its
//! `planId` is always null and its `state` is the episode kind. `details.queued` counts the episodes actually queued while
//! the row lists every fresh one, so an event may cover an episode whose notice was not queued.
//!
//! `snapshot.invalidated` carries `{section, key}`; `key` is the full item key of the snapshot (`integration:3`, an agent
//! id), which is what `snapshot_item` takes, not the raw ledger entity id.
//!
//! # Effects
//! `body.effects` is a list of `{section, op, key, fields}` for the snapshot (`waiting`, `agents`, `run`, `plans`).
//! `upsert` is used only for facts fixed when the row is written (a registered agent); every other change is an
//! `invalidate`, which asks the client to re-read the item (`snapshot_item`), so an effect never claims a mutable value.
//! An `invalidate` whose key is a bare kind name (`design`) asks for every item of that kind (`kind_key`).
//!
//! The keys are built by [`item_key`] from [`KEY_KINDS`]; the snapshot code uses the same function.

use super::feed_cursor;
use crate::canonical::sha256;
use crate::core::Core;
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{cap_text, clean_text, query_all, query_opt, MAX_EVENT_BYTES};
use crate::json::stringify;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::HashMap;

// ---------------------------------------------------------------------------------------------------------- limits

/// The most row reads (ledger rows and entity lookups) one `feed_page` makes.
pub const MAX_SCAN_ROWS: usize = 2000;
/// The most serialized bytes of the events of a page (escapes included).
pub const MAX_PAGE_BYTES: usize = 512 * 1024;
/// The most events one page returns.
pub const MAX_LIMIT: u32 = 500;
/// The most bytes of one text field of an event.
pub const TEXT_CAP: usize = 4096;
/// The most bytes of one reason or note of an event.
pub const REASON_CAP: usize = 1024;
/// The most options of a prompt an event carries.
pub const OPTIONS_CAP: usize = 32;
/// The most bytes of the text of one option of a prompt.
pub const OPTION_TEXT_CAP: usize = 256;
/// Room kept in a page for what is not an event.
const PAGE_ENVELOPE_BYTES: usize = 512;

// ---------------------------------------------------------------------------------------------------------- results

/// Where a client stands in the feed: the row and the sub-event of the row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Position {
    pub seq: u64,
    pub k: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedHead {
    pub project_id: String,
    pub epoch: String,
    pub head: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedPage {
    pub project_id: String,
    pub epoch: String,
    pub head: u64,
    pub anchor_ok: bool,
    pub ahead: bool,
    pub events: Vec<Value>,
    pub scanned: Position,
    pub has_more: bool,
    /// The row reads this call made (diagnostic).
    #[serde(rename = "rowsRead")]
    pub rows_read: usize,
}

// ------------------------------------------------------------------------------------------------------------- keys

/// One kind of item key of the snapshot: the section it lives in, its name in the key and where its number comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyKind {
    pub section: &'static str,
    pub kind: &'static str,
    pub source: &'static str,
}

const MESSAGE_KIND: &str = "message";

/// Every kind of item key, in sort order: the waiting section sorts by kind in this order, then by number.
pub const KEY_KINDS: &[KeyKind] = &[
    KeyKind {
        section: "waiting",
        kind: "op",
        source: "operator_proposals.sequence",
    },
    KeyKind {
        section: "waiting",
        kind: "prompt",
        source: "prompt_relays.sequence",
    },
    KeyKind {
        section: "waiting",
        kind: "design",
        source: "plans.sequence/plan_packages.package_id",
    },
    KeyKind {
        section: "waiting",
        kind: "message",
        source: "messages.sequence",
    },
    KeyKind {
        section: "waiting",
        kind: "integration",
        source: "integrations.sequence",
    },
    KeyKind {
        section: "agents",
        kind: "agent",
        source: "agents.agent_id",
    },
    KeyKind {
        section: "run",
        kind: "run",
        source: "none",
    },
    KeyKind {
        section: "plans",
        kind: "plan",
        source: "plans.sequence",
    },
];

/// The key of an item: `<kind>:<id>` in the waiting section (the id is a decimal number without padding, or
/// `<plan sequence>/<packageId>` for a design), the agent id in `agents`, `run` in `run`, the plan sequence in `plans`.
pub fn item_key(section: &str, kind: &str, id: &str) -> String {
    match section {
        "waiting" => format!("{kind}:{id}"),
        "run" => "run".to_string(),
        _ => id.to_string(),
    }
}

/// The key that asks for every item of a kind of a section (the bare kind name; no item key is one).
pub fn kind_key(kind: &str) -> String {
    kind.to_string()
}

/// The kind and id an item key of `section` was built from; `None` for a key `item_key` cannot have made.
pub fn parse_item_key(section: &str, key: &str) -> Option<(&'static str, String)> {
    let kinds = KEY_KINDS.iter().filter(|k| k.section == section);
    for kind in kinds {
        let id = match section {
            "waiting" => key
                .strip_prefix(kind.kind)
                .and_then(|r| r.strip_prefix(':')),
            "run" => (key == "run").then_some(""),
            _ => Some(key).filter(|k| !k.is_empty()),
        };
        if let Some(id) = id {
            if item_key(section, kind.kind, id) == key {
                return Some((kind.kind, id.to_string()));
            }
        }
    }
    None
}

// ------------------------------------------------------------------------------------------------------- the mapping

/// What a ledger row becomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// One public event of the type, or (with a non-empty `each`) one per item of that list in the row's details.
    Event {
        ty: &'static str,
        each: &'static str,
    },
    /// A `snapshot.invalidated` event for the item of the entity, in the section.
    Invalidate(&'static str),
    /// A `snapshot.invalidated` event that asks for every item of a kind of the waiting section: the writer queues
    /// messages (notices) without a row of its own for them.
    Refresh(&'static str),
}

/// One line of the mapping. `entity` is the ledger entity type, `actions` the payload actions it applies to (`a|b`), `from`
/// and `to` the states: `*` any, `+` any but none, `-` none, or `a|b`.
#[derive(Clone, Copy, Debug)]
pub struct Rule {
    pub entity: &'static str,
    pub actions: &'static str,
    pub from: &'static str,
    pub to: &'static str,
    pub kind: Kind,
}

const fn ev(ty: &'static str) -> Kind {
    Kind::Event { ty, each: "" }
}

const fn each(ty: &'static str, list: &'static str) -> Kind {
    Kind::Event { ty, each: list }
}

const fn rule(
    entity: &'static str,
    actions: &'static str,
    from: &'static str,
    to: &'static str,
    kind: Kind,
) -> Rule {
    Rule {
        entity,
        actions,
        from,
        to,
        kind,
    }
}

const MESSAGE_ACTIONS: &str = "message.enqueue|message.pull|message.input_clear|message.ack|message.resolve|message.transition|message.advance|message.defer|message.fail|message.sent|message.notified";

/// The mapping of a row to a public type; the first line that matches wins and a row no line matches is skipped. Details and
/// write-once columns may refine a type; mutable state never does.
pub const MAPPING: &[Rule] = &[
    rule(
        "message",
        "message.enqueue",
        "-",
        "queued",
        ev("message.action_needed"),
    ),
    rule("message", MESSAGE_ACTIONS, "*", "+", ev("message.state")),
    rule(
        "prompt_relay",
        "prompt_relay.capture",
        "-",
        "captured",
        ev("prompt.waiting"),
    ),
    rule(
        "prompt_relay",
        "prompt_relay.capture",
        "captured",
        "expired",
        ev("prompt.expired"),
    ),
    rule(
        "prompt_relay",
        "prompt_relay.begin",
        "captured",
        "typing",
        ev("prompt.answered"),
    ),
    rule(
        "prompt_relay",
        "prompt_relay.refuse",
        "captured",
        "refused",
        ev("prompt.answered"),
    ),
    rule(
        "prompt_relay",
        "prompt_relay.finish",
        "typing",
        "+",
        ev("prompt.answered"),
    ),
    rule(
        "prompt_relay",
        "prompt_relay.expire",
        "*",
        "*",
        each("prompt.expired", "expired"),
    ),
    rule(
        "prompt_relay",
        "prompt_relay.interrupted",
        "*",
        "*",
        each("prompt.answered", "failed"),
    ),
    rule(
        "operator_proposal",
        "operator.propose",
        "-",
        "+",
        ev("operator.proposed"),
    ),
    rule(
        "operator_proposal",
        "operator.decide",
        "proposed",
        "+",
        ev("operator.decided"),
    ),
    rule(
        "operator_proposal",
        "operator.cancel",
        "+",
        "cancelled",
        ev("operator.decided"),
    ),
    rule(
        "operator_proposal",
        "operator.expire",
        "*",
        "*",
        each("operator.decided", "expired"),
    ),
    rule(
        "operator_proposal",
        "operator.abandon",
        "*",
        "*",
        each("operator.decided", "abandoned"),
    ),
    rule(
        "operator_proposal",
        "operator.finish",
        "running",
        "+",
        ev("operator.finished"),
    ),
    rule(
        "operator_proposal",
        "operator.claim",
        "approved",
        "cancelled",
        ev("operator.decided"),
    ),
    rule(
        "operator_proposal",
        "operator.claim",
        "approved",
        "running|expired",
        Kind::Invalidate("waiting"),
    ),
    rule(
        "agent_report",
        "report.record",
        "*",
        "accepted|rejected",
        ev("report.recorded"),
    ),
    rule("plan", "design.decide", "*", "*", ev("design.decided")),
    rule("plan", "plan.open", "-", "draft", ev("plan.changed")),
    rule(
        "plan",
        "plan.submit|plan.amend",
        "draft",
        "in_review",
        ev("plan.submitted"),
    ),
    rule(
        "plan",
        "plan.submit|plan.amend",
        "draft",
        "approved",
        ev("plan.approved"),
    ),
    rule("plan", "plan.signoff", "*", "*", ev("plan.signed_off")),
    rule(
        "plan",
        "plan.assign|plan.reassign|plan.cancel|plan.review_abandon",
        "*",
        "*",
        ev("plan.changed"),
    ),
    rule(
        "review",
        "review.complete",
        "started",
        "passed|findings",
        ev("review.verdict"),
    ),
    rule(
        "verify_run",
        "verify.finish",
        "running",
        "+",
        ev("verify.result"),
    ),
    rule(
        "agent_finding",
        "finding.raise",
        "-",
        "open",
        ev("finding.raised"),
    ),
    rule(
        "agent_finding",
        "finding.check|finding.deadline",
        "open",
        "resolved|escalated|cancelled",
        ev("finding.resolved"),
    ),
    // An unresolved check queues another intervention for the target: no state moves, the message waits.
    rule(
        "agent_finding",
        "finding.check|finding.deadline",
        "open",
        "open",
        Kind::Refresh("message"),
    ),
    rule(
        "agent",
        "agent.register",
        "-",
        "active",
        ev("agent.registered"),
    ),
    rule("agent", "agent.lost", "active", "lost", ev("agent.lost")),
    rule(
        "agent",
        "agent.pause",
        "active",
        "paused",
        ev("agent.paused"),
    ),
    rule(
        "agent",
        "agent.resume",
        "paused",
        "active",
        ev("agent.resumed"),
    ),
    rule("agent", "agent.end", "active", "ended", ev("agent.ended")),
    rule(
        "agent",
        "agent.replace|agent.restart|agent.replaced",
        "*",
        "*",
        Kind::Invalidate("agents"),
    ),
    rule(
        "agent_pane",
        "agent_pane.record|agent_pane.clear|agent_pane.rename_branch",
        "*",
        "*",
        Kind::Invalidate("agents"),
    ),
    rule(
        "pm_notice",
        "pm.input_blocked",
        "*",
        "*",
        ev("agent.blocked"),
    ),
    rule(
        "pm_notice",
        "pm.attention",
        "*",
        "*",
        each("plan.needs_attention", "episodes"),
    ),
    rule("run_control", "run.transition", "*", "+", ev("run.changed")),
    rule(
        "integration",
        "integration.finish",
        "running",
        "+",
        ev("integration.finished"),
    ),
    rule(
        "integration",
        "integration.begin|integration.settle",
        "*",
        "*",
        Kind::Invalidate("waiting"),
    ),
    // Writers that queue notices for the PM or an agent (a cleanup offer, a delivery problem, a stall or a lost agent,
    // a finding, plan, report or review announcement, a wake): the message has no row of its own, so the row that
    // belongs to the writer asks for the waiting messages again.
    rule("*", NOTICE_ACTIONS, "*", "*", Kind::Refresh("message")),
];

/// Actions whose writer queues messages in the same mutation.
const NOTICE_ACTIONS: &str = "cleanup.branch|cleanup.complete|cleanup.offer|finding.announce|plan.announce|report.announce|review.announce|pm.delivery_notices|pm.floor_hit|pm.wake|supervision.check|agent.observe|verify.cancel|verify.interrupt|verify.orphan|verify.process|operator.process|operator.orphan_clear|pm_restart.consume";

/// Actions the code writes that mean nothing to a client, by name: nothing in a snapshot or a type of the feed changes with
/// them.
pub const SKIPPED_ACTIONS: &[&str] = &[
    "actor.create",
    "actor.revoke",
    "fallback_pane.clear",
    "fallback_pane.record",
    "link.bind",
    "link.external",
    "operator.full_auto_refuse",
    "operator.grant_expire",
    "operator.grant_restart",
    "operator.grant_revoke",
    "operator.window_expire",
    "operator.window_restart",
    "operator.window_revoke",
    "orphan_pane.clear",
    "orphan_pane.record",
    "review.begin",
    "review.warm_release",
    "role.sync",
    "seat.create",
    "verify.claim",
    "verify.queue",
    "wait.begin",
    "wait.end",
    "wait.end_controller",
];

/// Spellings of actions an older kernel or the Node controller wrote that the current code no longer does; rows with them
/// are skipped. (The history of `kernel/src` and `testdata/node/daemon.ts` show none that no current action replaced.)
pub const LEGACY_ACTIONS: &[&str] = &[];

/// The types of the first version of the feed, fixed by the plan.
pub const V1_TYPES: &[&str] = &[
    "message.action_needed",
    "message.state",
    "prompt.waiting",
    "prompt.answered",
    "prompt.expired",
    "operator.proposed",
    "operator.decided",
    "operator.finished",
    "report.recorded",
    "design.decided",
    "plan.submitted",
    "plan.approved",
    "plan.signed_off",
    "plan.changed",
    "plan.needs_attention",
    "review.verdict",
    "verify.result",
    "finding.raised",
    "finding.resolved",
    "agent.registered",
    "agent.lost",
    "agent.paused",
    "agent.resumed",
    "agent.ended",
    "agent.blocked",
    "run.changed",
    "integration.finished",
    "snapshot.invalidated",
];

/// V1 types that cannot be produced, each with its reason; empty when every type has a writer that moves `state_version`
/// (searched: every `MutationEvent` in `kernel/src` and the actions of `MAPPING`).
pub const DROPPED: &[(&str, &str)] = &[];

fn pattern_matches(pattern: &str, value: Option<&str>) -> bool {
    match pattern {
        "*" => true,
        "+" => value.is_some(),
        "-" => value.is_none(),
        _ => value.is_some_and(|v| pattern.split('|').any(|p| p == v)),
    }
}

impl Rule {
    fn matches(&self, entity: &str, action: &str, from: Option<&str>, to: Option<&str>) -> bool {
        (self.entity == "*" || self.entity == entity)
            && pattern_matches(self.actions, Some(action))
            && pattern_matches(self.from, from)
            && pattern_matches(self.to, to)
    }
}

/// The kind a row maps to, or `None` when it is skipped.
pub fn map_row(entity: &str, action: &str, from: Option<&str>, to: Option<&str>) -> Option<Kind> {
    MAPPING
        .iter()
        .find(|rule| rule.matches(entity, action, from, to))
        .map(|rule| rule.kind)
}

/// Whether `action` appears in a line of `MAPPING`.
pub fn action_is_mapped(action: &str) -> bool {
    MAPPING
        .iter()
        .any(|rule| rule.actions.split('|').any(|a| a == action))
}

// ------------------------------------------------------------------------------------------------------------ rows

struct Row {
    seq: i64,
    entity_type: String,
    entity_id: String,
    from: Option<String>,
    to: Option<String>,
    actor_id: String,
    created_at: String,
    action: String,
    /// `payload.payload`: what the caller sent.
    input: Value,
    details: Value,
    entity_version: i64,
}

fn row_of(seq: i64, record: RawRow) -> Row {
    let payload: Value = serde_json::from_str(&record.payload_json).unwrap_or(Value::Null);
    Row {
        seq,
        entity_type: record.entity_type,
        entity_id: record.entity_id,
        from: record.from_state,
        to: record.to_state,
        actor_id: record.actor_id,
        created_at: record.created_at,
        action: payload["action"].as_str().unwrap_or("").to_string(),
        input: payload["payload"].clone(),
        details: payload["details"].clone(),
        entity_version: payload["entityVersion"].as_i64().unwrap_or(0),
    }
}

struct RawRow {
    entity_type: String,
    entity_id: String,
    from_state: Option<String>,
    to_state: Option<String>,
    actor_id: String,
    payload_json: String,
    created_at: String,
}

/// Why a step of the page ended early.
enum Flow {
    /// The row-read budget is spent.
    Stop,
    Error(KernelError),
}

impl From<KernelError> for Flow {
    fn from(error: KernelError) -> Self {
        Flow::Error(error)
    }
}

type Step<T> = Result<T, Flow>;

/// A parameter list of mixed types.
macro_rules! sql_params {
    ($($value:expr),* $(,)?) => {
        &[$(&$value as &dyn rusqlite::ToSql),*]
    };
}

/// The state of one `feed_page` call: the reads made, and the entity rows already read.
struct Ctx<'a> {
    core: &'a Core,
    project_id: String,
    epoch: String,
    head: u64,
    reads: usize,
    max_rows: usize,
    max_bytes: usize,
    cache: HashMap<String, Option<Value>>,
}

impl Ctx<'_> {
    /// Spends one row read, or says the budget is spent.
    fn charge(&mut self) -> Step<()> {
        if self.reads >= self.max_rows {
            return Err(Flow::Stop);
        }
        self.reads += 1;
        Ok(())
    }

    /// The first row of `sql` as a JSON object of the named columns; one read, shared by repeats of the same `key`.
    fn lookup(
        &mut self,
        key: String,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
        columns: &[&str],
    ) -> Step<Option<Value>> {
        if let Some(found) = self.cache.get(&key) {
            return Ok(found.clone());
        }
        self.charge()?;
        let kernel = self.core.kernel();
        let found = query_opt(&kernel.database, sql, params, |row| {
            let mut object = Map::new();
            for (index, name) in columns.iter().enumerate() {
                object.insert((*name).to_string(), sql_value(row.get_ref(index)?));
            }
            Ok(Value::Object(object))
        })?;
        self.cache.insert(key, found.clone());
        Ok(found)
    }
}

fn sql_value(value: rusqlite::types::ValueRef<'_>) -> Value {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(n) => Value::from(n),
        ValueRef::Real(x) => Value::from(x),
        ValueRef::Text(bytes) => Value::String(String::from_utf8_lossy(bytes).into_owned()),
        ValueRef::Blob(_) => Value::Null,
    }
}

// --------------------------------------------------------------------------------------------------- entity lookups

fn load_message(ctx: &mut Ctx<'_>, id: &str) -> Step<Option<Value>> {
    let project = ctx.project_id.clone();
    ctx.lookup(
        format!("message:{id}"),
        "SELECT sequence, body, recipient_agent_id, action_needed FROM messages WHERE project_id = ? AND message_id = ?",
        sql_params![project, id],
        &["sequence", "body", "recipient", "actionNeeded"],
    )
}

fn load_relay(ctx: &mut Ctx<'_>, id: &str) -> Step<Option<Value>> {
    let project = ctx.project_id.clone();
    ctx.lookup(
        format!("relay:{id}"),
        "SELECT sequence, agent_id, pane_id, host_kind, prompt_text, options_json FROM prompt_relays WHERE project_id = ? AND relay_id = ?",
        sql_params![project, id],
        &["sequence", "agentId", "paneId", "hostKind", "text", "options"],
    )
}

fn load_proposal(ctx: &mut Ctx<'_>, id: &str) -> Step<Option<Value>> {
    let project = ctx.project_id.clone();
    ctx.lookup(
        format!("proposal:{id}"),
        "SELECT sequence, kind, command, reason, force_restart FROM operator_proposals WHERE project_id = ? AND proposal_id = ?",
        sql_params![project, id],
        &["sequence", "kind", "command", "reason", "forceRestart"],
    )
}

fn load_report(ctx: &mut Ctx<'_>, id: &str) -> Step<Option<Value>> {
    let project = ctx.project_id.clone();
    ctx.lookup(
        format!("report:{id}"),
        "SELECT agent_id, summary, commit_sha FROM agent_reports WHERE project_id = ? AND report_id = ?",
        sql_params![project, id],
        &["agentId", "summary", "commitSha"],
    )
}

fn load_agent(ctx: &mut Ctx<'_>, id: &str) -> Step<Option<Value>> {
    let project = ctx.project_id.clone();
    ctx.lookup(
        format!("agent:{id}"),
        "SELECT kind, role_name, generation FROM agents WHERE project_id = ? AND agent_id = ?",
        sql_params![project, id],
        &["kind", "role", "generation"],
    )
}

fn load_plan(ctx: &mut Ctx<'_>, id: &str) -> Step<Option<Value>> {
    let project = ctx.project_id.clone();
    ctx.lookup(
        format!("plan:{id}"),
        "SELECT sequence FROM plans WHERE project_id = ? AND plan_id = ?",
        sql_params![project, id],
        &["sequence"],
    )
}

fn load_integration(ctx: &mut Ctx<'_>, id: &str) -> Step<Option<Value>> {
    let project = ctx.project_id.clone();
    ctx.lookup(
        format!("integration:{id}"),
        "SELECT sequence FROM integrations WHERE project_id = ? AND integration_id = ?",
        sql_params![project, id],
        &["sequence"],
    )
}

fn load_review(ctx: &mut Ctx<'_>, id: &str) -> Step<Option<Value>> {
    let project = ctx.project_id.clone();
    ctx.lookup(
        format!("review:{id}"),
        "SELECT subject_plan_id FROM reviews WHERE project_id = ? AND review_id = ?",
        sql_params![project, id],
        &["planId"],
    )
}

/// The `payload_json` of the first row of the entity that has no from-state (the row that created it), parsed; the hashes
/// of the writer's input sit there.
fn creation_payload(ctx: &mut Ctx<'_>, entity_type: &str, id: &str) -> Step<Option<Value>> {
    let project = ctx.project_id.clone();
    let found = ctx.lookup(
        format!("created:{entity_type}:{id}"),
        "SELECT payload_json FROM controller_events WHERE project_id = ? AND entity_type = ? AND entity_id = ?
           AND from_state IS NULL AND to_state IS NOT NULL ORDER BY sequence LIMIT 1",
        sql_params![project, entity_type, id],
        &["payload"],
    )?;
    Ok(found.and_then(|row| {
        row["payload"]
            .as_str()
            .and_then(|text| serde_json::from_str(text).ok())
    }))
}

// ------------------------------------------------------------------------------------------------------- text parts

/// What a body says about the text it carries.
struct Text {
    verified: bool,
    untrusted: Vec<String>,
    truncated: bool,
}

impl Text {
    fn new() -> Self {
        Text {
            verified: true,
            untrusted: Vec::new(),
            truncated: false,
        }
    }

    fn fail(&mut self) {
        self.verified = false;
    }

    /// `raw`, cleaned and capped, as `body[key]`.
    fn put(&mut self, body: &mut Map<String, Value>, key: &str, raw: &str, cap: usize) {
        let cleaned = clean_text(raw);
        let (shown, cut) = cap_text(&cleaned, cap);
        self.truncated |= cut;
        self.untrusted.push(format!("body.{key}"));
        body.insert(key.to_string(), Value::from(shown));
    }

    /// A free text that no hash covers (a reason): cleaned and capped, never part of `textVerified`.
    fn put_free(&mut self, body: &mut Map<String, Value>, key: &str, raw: Option<&str>) {
        match raw {
            Some(raw) => self.put(body, key, raw, REASON_CAP),
            None => {
                body.insert(key.to_string(), Value::Null);
            }
        }
    }

    fn finish(self, body: &mut Map<String, Value>) {
        body.insert("textVerified".into(), Value::Bool(self.verified));
        body.insert("untrusted".into(), json!(self.untrusted));
        body.insert("truncated".into(), Value::Bool(self.truncated));
    }
}

/// What the event of a row is about, before its body is read.
struct Draft {
    ty: &'static str,
    /// The item of a row that lists several (a relay id, a proposal id, an episode).
    item: Option<Value>,
    /// Whether the id of the event has `:<k>`.
    listed: bool,
}

struct Built {
    entity_type: String,
    entity_id: String,
    from: Option<String>,
    to: Option<String>,
    body: Map<String, Value>,
}

fn effect(section: &str, op: &str, key: String, fields: Value) -> Value {
    json!({"section": section, "op": op, "key": key, "fields": fields})
}

/// The effect of a writer that may queue a notice for an agent or the PM: a notice is inserted with the mutation that
/// causes it and has no ledger row of its own, so every message that waits is asked for again.
fn notices_effect() -> Value {
    effect("waiting", "invalidate", kind_key(MESSAGE_KIND), json!({}))
}

fn text_of<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn num_text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_i64)
        .map(|n| n.to_string())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------------------- classify

fn classify(ctx: &mut Ctx<'_>, row: &Row) -> Step<Vec<Draft>> {
    let Some(kind) = map_row(
        &row.entity_type,
        &row.action,
        row.from.as_deref(),
        row.to.as_deref(),
    ) else {
        return Ok(Vec::new());
    };
    match kind {
        Kind::Invalidate(_) | Kind::Refresh(_) => Ok(vec![Draft {
            ty: "snapshot.invalidated",
            item: None,
            listed: false,
        }]),
        Kind::Event { ty, each: "" } => {
            if row.entity_type == "message" {
                // Only an action-needed message is a public event.
                let Some(message) = load_message(ctx, &row.entity_id)? else {
                    return Ok(Vec::new());
                };
                if message["actionNeeded"].as_i64() != Some(1) {
                    // An ordinary message that fails, expires or goes unacknowledged makes the controller queue a
                    // delivery notice, which waits for the PM.
                    let problem =
                        matches!(row.to.as_deref(), Some("failed" | "expired" | "unacked"));
                    return Ok(if problem {
                        vec![Draft {
                            ty: "snapshot.invalidated",
                            item: None,
                            listed: false,
                        }]
                    } else {
                        Vec::new()
                    });
                }
            }
            if ty == "agent.blocked" && !queued(row) {
                return Ok(Vec::new());
            }
            Ok(vec![Draft {
                ty,
                item: None,
                listed: false,
            }])
        }
        Kind::Event { ty, each } => {
            let items: Vec<Value> = if each == "episodes" {
                if queued(row) {
                    row.input["episodes"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                } else {
                    Vec::new()
                }
            } else {
                row.details[each]
                    .as_array()
                    .map(|ids| ids.iter().filter(|id| id.is_string()).cloned().collect())
                    .unwrap_or_default()
            };
            Ok(items
                .into_iter()
                .map(|item| Draft {
                    ty,
                    item: Some(item),
                    listed: true,
                })
                .collect())
        }
    }
}

/// Whether a notice row queued anything (`details.queued` is a flag or a count).
fn queued(row: &Row) -> bool {
    match &row.details["queued"] {
        Value::Bool(flag) => *flag,
        Value::Number(n) => n.as_i64().unwrap_or(0) > 0,
        _ => false,
    }
}

// ------------------------------------------------------------------------------------------------------------ build

fn build(ctx: &mut Ctx<'_>, row: &Row, draft: &Draft) -> Step<Option<Built>> {
    let mut body = Map::new();
    let mut text = Text::new();
    let mut base = Built {
        entity_type: row.entity_type.clone(),
        entity_id: row.entity_id.clone(),
        from: row.from.clone(),
        to: row.to.clone(),
        body: Map::new(),
    };
    #[allow(clippy::needless_late_init)]
    let effects: Vec<Value>;
    match draft.ty {
        "message.action_needed" | "message.state" => {
            let Some(message) = load_message(ctx, &row.entity_id)? else {
                return Ok(None);
            };
            let created = if row.from.is_none() {
                Some(json!({"details": row.details.clone()}))
            } else {
                creation_payload(ctx, "message", &row.entity_id)?
            };
            let ledger = created
                .as_ref()
                .and_then(|payload| text_of(&payload["details"], "bodyHash"))
                .map(str::to_string);
            let content = text_of(&message, "body").unwrap_or("");
            body.insert("messageId".into(), json!(row.entity_id));
            body.insert("recipientAgentId".into(), message["recipient"].clone());
            body.insert("state".into(), json!(row.to));
            if ledger.as_deref() == Some(sha256(content).as_str()) {
                text.put(&mut body, "body", content, TEXT_CAP);
            } else {
                text.fail();
                body.insert("body".into(), Value::Null);
            }
            text.put_free(&mut body, "reason", text_of(&row.details, "reason"));
            if let Some(decision) = text_of(&row.details, "decision") {
                body.insert("decision".into(), json!(decision));
            }
            let mut list = vec![effect(
                "waiting",
                "invalidate",
                item_key("waiting", "message", &num_text(&message, "sequence")),
                json!({}),
            )];
            if draft.ty == "message.state" {
                // An expired, failed or unacknowledged message makes the controller queue notices.
                list.push(notices_effect());
            }
            effects = list;
        }
        "prompt.waiting" | "prompt.answered" | "prompt.expired" => {
            let id = draft
                .item
                .as_ref()
                .and_then(Value::as_str)
                .unwrap_or(&row.entity_id)
                .to_string();
            let Some(relay) = load_relay(ctx, &id)? else {
                return Ok(None);
            };
            let created = if row.from.is_none() && draft.item.is_none() {
                Some(json!({"payload": row.input.clone()}))
            } else {
                creation_payload(ctx, "prompt_relay", &id)?
            };
            let ledger = created
                .as_ref()
                .and_then(|payload| text_of(&payload["payload"], "promptSha"))
                .map(str::to_string);
            base.entity_id = id.clone();
            if draft.item.is_some() {
                base.from = Some(
                    if row.action == "prompt_relay.interrupted" {
                        "typing"
                    } else {
                        "captured"
                    }
                    .to_string(),
                );
                base.to = Some(
                    if row.action == "prompt_relay.interrupted" {
                        "failed"
                    } else {
                        "expired"
                    }
                    .to_string(),
                );
            }
            body.insert("relayId".into(), json!(id));
            body.insert("agentId".into(), relay["agentId"].clone());
            body.insert("state".into(), json!(base.to));
            let reason = match row.action.as_str() {
                "prompt_relay.expire" => Some("expired"),
                "prompt_relay.interrupted" => Some("interrupted"),
                _ => text_of(&row.details, "reason"),
            };
            text.put_free(&mut body, "reason", reason);
            if prompt_verified(&relay, ledger.as_deref()) {
                let raw = text_of(&relay, "text").unwrap_or("");
                text.put(&mut body, "text", raw, TEXT_CAP);
                let options = prompt_options(&relay, &mut text);
                body.insert("options".into(), Value::Array(options));
                text.untrusted.push("body.options".into());
            } else {
                text.fail();
                body.insert("text".into(), Value::Null);
                body.insert("options".into(), Value::Null);
            }
            effects = vec![effect(
                "waiting",
                "invalidate",
                item_key("waiting", "prompt", &num_text(&relay, "sequence")),
                json!({}),
            )];
        }
        "operator.proposed" | "operator.decided" | "operator.finished" => {
            let id = draft
                .item
                .as_ref()
                .and_then(Value::as_str)
                .unwrap_or(&row.entity_id)
                .to_string();
            let Some(proposal) = load_proposal(ctx, &id)? else {
                return Ok(None);
            };
            let created = if row.from.is_none() && draft.item.is_none() {
                Some(json!({"payload": row.input.clone()}))
            } else {
                creation_payload(ctx, "operator_proposal", &id)?
            };
            let ledger = created.as_ref().map(|payload| payload["payload"].clone());
            base.entity_id = id.clone();
            if draft.item.is_some() {
                base.from = None;
                base.to = Some(
                    if row.action == "operator.abandon" {
                        "abandoned"
                    } else {
                        "expired"
                    }
                    .to_string(),
                );
            }
            body.insert("proposalId".into(), json!(id));
            body.insert("kind".into(), proposal["kind"].clone());
            body.insert("state".into(), json!(base.to));
            body.insert(
                "decision".into(),
                if draft.ty == "operator.decided" {
                    json!(base.to)
                } else {
                    Value::Null
                },
            );
            let command = text_of(&proposal, "command").unwrap_or("");
            let kind = text_of(&proposal, "kind").unwrap_or("");
            let force = proposal["forceRestart"].as_i64() == Some(1);
            let command_hash = sha256(&stringify(&json!([kind, command, force])));
            if ledger.as_ref().and_then(|l| text_of(l, "commandSha")) == Some(command_hash.as_str())
            {
                text.put(&mut body, "command", command, TEXT_CAP);
            } else {
                text.fail();
                body.insert("command".into(), Value::Null);
            }
            let reason = text_of(&proposal, "reason").unwrap_or("");
            if ledger.as_ref().and_then(|l| text_of(l, "reasonSha"))
                == Some(sha256(reason).as_str())
            {
                text.put(&mut body, "reason", reason, REASON_CAP);
            } else {
                text.fail();
                body.insert("reason".into(), Value::Null);
            }
            let mut list = vec![effect(
                "waiting",
                "invalidate",
                item_key("waiting", "op", &num_text(&proposal, "sequence")),
                json!({}),
            )];
            if draft.ty == "operator.proposed" {
                list.push(notices_effect());
            }
            effects = list;
        }
        "report.recorded" => {
            let Some(report) = load_report(ctx, &row.entity_id)? else {
                return Ok(None);
            };
            let summary = text_of(&report, "summary").unwrap_or("");
            body.insert("reportId".into(), json!(row.entity_id));
            body.insert("agentId".into(), report["agentId"].clone());
            body.insert("state".into(), json!(row.to));
            body.insert("commitSha".into(), report["commitSha"].clone());
            if text_of(&row.input, "summaryHash") == Some(sha256(summary).as_str()) {
                text.put(&mut body, "summary", summary, TEXT_CAP);
            } else {
                text.fail();
                body.insert("summary".into(), Value::Null);
            }
            let package = current_package(ctx, text_of(&report, "agentId").unwrap_or(""))?;
            body.insert(
                "current".into(),
                json!({"readAt": ctx.head, "package": package}),
            );
            // A report moves the reported count of its plan; which plan is not read (it would move on replay).
            effects = vec![
                effect("waiting", "invalidate", kind_key("design"), json!({})),
                effect("plans", "invalidate", kind_key("plan"), json!({})),
                notices_effect(),
            ];
        }
        "design.decided" => {
            let Some(plan) = load_plan(ctx, &row.entity_id)? else {
                return Ok(None);
            };
            let package_id = text_of(&row.details, "packageId").unwrap_or("");
            body.insert("planId".into(), json!(row.entity_id));
            body.insert("packageId".into(), json!(package_id));
            body.insert("decision".into(), row.details["designDecision"].clone());
            body.insert("reportId".into(), row.details["reportId"].clone());
            body.insert("link".into(), row.input["link"].clone());
            let sequence = num_text(&plan, "sequence");
            effects = vec![
                effect(
                    "waiting",
                    "invalidate",
                    item_key("waiting", "design", &format!("{sequence}/{package_id}")),
                    json!({}),
                ),
                effect(
                    "plans",
                    "invalidate",
                    item_key("plans", "plan", &sequence),
                    json!({}),
                ),
                notices_effect(),
            ];
        }
        "plan.submitted" | "plan.approved" | "plan.signed_off" | "plan.changed" => {
            let Some(plan) = load_plan(ctx, &row.entity_id)? else {
                return Ok(None);
            };
            body.insert("planId".into(), json!(row.entity_id));
            body.insert("state".into(), json!(row.to));
            for key in [
                "packageId",
                "agentId",
                "revision",
                "integrationId",
                "fromAgentId",
                "toAgentId",
            ] {
                if let Some(value) = row.details.get(key) {
                    body.insert(key.to_string(), value.clone());
                }
            }
            let mut list = vec![effect(
                "plans",
                "invalidate",
                item_key("plans", "plan", &num_text(&plan, "sequence")),
                json!({}),
            )];
            if draft.ty == "plan.approved" {
                // The approval retires the plan this one supersedes, without a row of its own: the plans and the
                // designs that waited on the old one are asked for again.
                list.push(effect("plans", "invalidate", kind_key("plan"), json!({})));
                list.push(effect(
                    "waiting",
                    "invalidate",
                    kind_key("design"),
                    json!({}),
                ));
            }
            if matches!(
                row.action.as_str(),
                "plan.assign" | "plan.reassign" | "plan.cancel"
            ) {
                // A package that changes hands or is cancelled no longer waits on the report it had (an ended agent's package is
                // handed over by a new assign).
                list.push(effect(
                    "waiting",
                    "invalidate",
                    kind_key("design"),
                    json!({}),
                ));
            }
            list.push(notices_effect());
            effects = list;
        }
        "plan.needs_attention" => {
            let episode = draft.item.clone().unwrap_or(Value::Null);
            let agent_id = text_of(&episode, "agentId").unwrap_or("").to_string();
            body.insert("planId".into(), Value::Null);
            body.insert("state".into(), episode["kind"].clone());
            body.insert("agentId".into(), json!(agent_id));
            body.insert("kind".into(), episode["kind"].clone());
            body.insert("episodeMs".into(), episode["episodeMs"].clone());
            base.entity_type = "agent".into();
            base.entity_id = agent_id.clone();
            effects = vec![notices_effect()];
        }
        "review.verdict" => {
            body.insert("reviewId".into(), json!(row.entity_id));
            body.insert(
                "verdict".into(),
                json!(if row.to.as_deref() == Some("passed") {
                    "pass"
                } else {
                    "findings"
                }),
            );
            let plan_id = load_review(ctx, &row.entity_id)?
                .and_then(|review| text_of(&review, "planId").map(str::to_string));
            let mut list = match plan_id {
                Some(plan_id) => match load_plan(ctx, &plan_id)? {
                    Some(plan) => vec![effect(
                        "plans",
                        "invalidate",
                        item_key("plans", "plan", &num_text(&plan, "sequence")),
                        json!({}),
                    )],
                    None => Vec::new(),
                },
                None => Vec::new(),
            };
            if row.to.as_deref() == Some("passed") {
                // A passed plan review approves the plan, which retires the plan it supersedes (no row of its own).
                list.push(effect("plans", "invalidate", kind_key("plan"), json!({})));
                list.push(effect(
                    "waiting",
                    "invalidate",
                    kind_key("design"),
                    json!({}),
                ));
            }
            list.push(notices_effect());
            effects = list;
        }
        "verify.result" => {
            body.insert("runId".into(), json!(row.entity_id));
            body.insert("result".into(), json!(row.to));
            effects = vec![notices_effect()];
        }
        "finding.raised" | "finding.resolved" => {
            body.insert("findingId".into(), json!(row.entity_id));
            body.insert("state".into(), json!(row.to));
            for key in [
                "targetAgentId",
                "raisedByAgentId",
                "severity",
                "result",
                "reason",
            ] {
                if let Some(value) = row.details.get(key) {
                    body.insert(key.to_string(), value.clone());
                }
            }
            effects = vec![notices_effect()];
        }
        "agent.registered" | "agent.lost" | "agent.paused" | "agent.resumed" | "agent.ended"
        | "agent.blocked" => {
            let agent_id = if draft.ty == "agent.blocked" {
                text_of(&row.input, "agentId").unwrap_or("").to_string()
            } else {
                row.entity_id.clone()
            };
            let Some(agent) = load_agent(ctx, &agent_id)? else {
                return Ok(None);
            };
            base.entity_type = "agent".into();
            base.entity_id = agent_id.clone();
            let state = match draft.ty {
                "agent.blocked" => "blocked".to_string(),
                _ => row.to.clone().unwrap_or_default(),
            };
            let paused = match draft.ty {
                "agent.paused" => json!(true),
                "agent.registered" | "agent.resumed" | "agent.ended" => json!(false),
                _ => Value::Null,
            };
            let generation = match row.details.get("generation").and_then(Value::as_i64) {
                Some(generation) => Some(generation),
                None if row.entity_version > 0 && draft.ty != "agent.blocked" => {
                    Some(row.entity_version)
                }
                None => None,
            };
            body.insert("agentId".into(), json!(agent_id));
            body.insert("kind".into(), agent["kind"].clone());
            body.insert("role".into(), agent["role"].clone());
            body.insert("state".into(), json!(state));
            body.insert("paused".into(), paused.clone());
            body.insert("generation".into(), json!(generation));
            let mut current = Map::new();
            current.insert("readAt".into(), json!(ctx.head));
            if generation.is_none() {
                current.insert("generation".into(), agent["generation"].clone());
            }
            body.insert("current".into(), Value::Object(current));
            for key in ["reason", "branch", "scope"] {
                if let Some(raw) = text_of(&row.details, key) {
                    text.put(&mut body, key, raw, REASON_CAP);
                }
            }
            if draft.ty == "agent.blocked" {
                body.insert("blocker".into(), row.details["blocker"].clone());
            }
            effects = if draft.ty == "agent.registered" {
                vec![effect(
                    "agents",
                    "upsert",
                    item_key("agents", "agent", &agent_id),
                    json!({
                        "agentId": agent_id,
                        "kind": agent["kind"],
                        "role": agent["role"],
                        "state": state,
                        "paused": false,
                        "generation": generation,
                    }),
                )]
            } else {
                let mut effects = vec![effect(
                    "agents",
                    "invalidate",
                    item_key("agents", "agent", &agent_id),
                    json!({}),
                )];
                if matches!(draft.ty, "agent.lost" | "agent.blocked") {
                    effects.push(notices_effect());
                }
                if draft.ty == "agent.ended" {
                    // Ending an agent cancels its unstarted operator proposals without a ledger row of their own.
                    effects.push(effect("waiting", "invalidate", kind_key("op"), json!({})));
                }
                effects
            };
        }
        "run.changed" => {
            body.insert("state".into(), json!(row.to));
            body.insert("paused".into(), json!(row.to.as_deref() == Some("paused")));
            if let Some(raw) = text_of(&row.details, "reason") {
                text.put(&mut body, "reason", raw, REASON_CAP);
            }
            effects = vec![effect(
                "run",
                "invalidate",
                item_key("run", "run", ""),
                json!({}),
            )];
        }
        "integration.finished" => {
            body.insert("integrationId".into(), json!(row.entity_id));
            body.insert("state".into(), json!(row.to));
            let sequence = load_integration(ctx, &row.entity_id)?.map(|i| num_text(&i, "sequence"));
            // A merged integration counts its packages as integrated in their plans.
            let mut list = vec![
                effect("plans", "invalidate", kind_key("plan"), json!({})),
                notices_effect(),
            ];
            if let Some(sequence) = sequence {
                list.push(effect(
                    "waiting",
                    "invalidate",
                    item_key("waiting", "integration", &sequence),
                    json!({}),
                ));
            }
            effects = list;
        }
        "snapshot.invalidated" => {
            let kind = if row.entity_type == "message" {
                Some(Kind::Refresh(MESSAGE_KIND))
            } else {
                map_row(
                    &row.entity_type,
                    &row.action,
                    row.from.as_deref(),
                    row.to.as_deref(),
                )
            };
            let Some(kind) = kind else {
                return Ok(None);
            };
            if let Kind::Refresh(name) = kind {
                body.insert("section".into(), json!("waiting"));
                body.insert("key".into(), json!(kind_key(name)));
                base.body = Map::new();
                effects = vec![effect("waiting", "invalidate", kind_key(name), json!({}))];
                body.insert("effects".into(), Value::Array(effects));
                base.body = body;
                return Ok(Some(base));
            }
            let Kind::Invalidate(section) = kind else {
                return Ok(None);
            };
            let key = match section {
                "agents" => Some(item_key("agents", "agent", &row.entity_id)),
                "waiting" => match row.entity_type.as_str() {
                    "operator_proposal" => load_proposal(ctx, &row.entity_id)?
                        .map(|p| item_key("waiting", "op", &num_text(&p, "sequence"))),
                    "integration" => load_integration(ctx, &row.entity_id)?
                        .map(|i| item_key("waiting", "integration", &num_text(&i, "sequence"))),
                    _ => None,
                },
                _ => None,
            };
            let Some(key) = key else {
                return Ok(None);
            };
            body.insert("section".into(), json!(section));
            body.insert("key".into(), json!(key));
            let mut list = vec![effect(section, "invalidate", key, json!({}))];
            if matches!(
                row.action.as_str(),
                "agent.replace" | "agent.restart" | "agent.replaced"
            ) {
                // A replaced or restarted agent's unstarted operator proposals are cancelled without a row of their own.
                list.push(effect("waiting", "invalidate", kind_key("op"), json!({})));
                // The reports of the earlier generation no longer count for a design package.
                list.push(effect(
                    "waiting",
                    "invalidate",
                    kind_key("design"),
                    json!({}),
                ));
            }
            if matches!(
                row.action.as_str(),
                "agent.replace" | "agent.restart" | "agent.replaced"
            ) {
                // The packages count only the reports of the current generation (or of the successor they were
                // rebound to), so the reported counts of the plans move.
                list.push(effect("plans", "invalidate", kind_key("plan"), json!({})));
            }
            if row.action == "integration.settle" {
                // A discarded integration stops counting its packages as integrated.
                list.push(effect("plans", "invalidate", kind_key("plan"), json!({})));
            }
            effects = list;
        }
        _ => return Ok(None),
    }
    body.insert("effects".into(), Value::Array(effects));
    if has_text_fields(draft.ty) || !text.untrusted.is_empty() {
        text.finish(&mut body);
    }
    base.body = body;
    Ok(Some(base))
}

/// Whether the body of a type carries text, so it states `textVerified`.
fn has_text_fields(ty: &str) -> bool {
    matches!(
        ty,
        "message.action_needed"
            | "message.state"
            | "prompt.waiting"
            | "prompt.answered"
            | "prompt.expired"
            | "operator.proposed"
            | "operator.decided"
            | "operator.finished"
            | "report.recorded"
    )
}

/// The reporting agent's package now: `{planId, packageId, design}` of its latest assignment in an approved plan, or null.
fn current_package(ctx: &mut Ctx<'_>, agent_id: &str) -> Step<Value> {
    let project = ctx.project_id.clone();
    let route = ctx.lookup(
        format!("package-of:{agent_id}"),
        "SELECT p.plan_id, p.package_id, r.body_json FROM plan_packages p
           JOIN plans pl ON pl.project_id = p.project_id AND pl.plan_id = p.plan_id
           LEFT JOIN plan_revisions r ON r.project_id = pl.project_id AND r.plan_id = pl.plan_id AND r.revision = pl.approved_revision
          WHERE p.project_id = ? AND p.assignee_agent_id = ? AND pl.state = 'approved' AND p.cancelled_at IS NULL
          ORDER BY p.assigned_at DESC, pl.sequence DESC LIMIT 1",
        sql_params![project, agent_id],
        &["planId", "packageId", "body"],
    )?;
    Ok(match route {
        None => Value::Null,
        Some(route) => {
            let package_id = text_of(&route, "packageId").unwrap_or("");
            let design = text_of(&route, "body")
                .is_some_and(|body| crate::plan_body::package_is_design(body, package_id));
            json!({"planId": route["planId"], "packageId": package_id, "design": design})
        }
    })
}

/// The full prompt hash of the writer (herdr `prompt_hash`), recomputed from the entity row, compared with the ledger's.
fn prompt_verified(relay: &Value, ledger: Option<&str>) -> bool {
    let Some(ledger) = ledger else {
        return false;
    };
    let options: Vec<Value> = relay["options"]
        .as_str()
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|option| {
            json!({
                "number": option["number"],
                "text": option["text"],
                "acceptsText": option.get("acceptsText").cloned().unwrap_or(Value::Bool(false)),
                "widensPermissions": option.get("widensPermissions").cloned().unwrap_or(Value::Bool(false)),
            })
        })
        .collect();
    [false, true].into_iter().any(|dialog| {
        let mut canonical = Map::new();
        canonical.insert("agentId".into(), relay["agentId"].clone());
        canonical.insert(
            "paneId".into(),
            json!(text_of(relay, "paneId").unwrap_or("")),
        );
        canonical.insert(
            "hostKind".into(),
            json!(text_of(relay, "hostKind").unwrap_or("")),
        );
        canonical.insert("text".into(), relay["text"].clone());
        canonical.insert("options".into(), Value::Array(options.clone()));
        if dialog {
            canonical.insert("dialog".into(), Value::Bool(true));
        }
        sha256(&Value::Object(canonical).to_string()) == ledger
    })
}

/// The options of a prompt as shown: at most `OPTIONS_CAP`, each text cleaned and cut to `OPTION_TEXT_CAP` bytes.
fn prompt_options(relay: &Value, text: &mut Text) -> Vec<Value> {
    let all = relay["options"]
        .as_str()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    if all.len() > OPTIONS_CAP {
        text.truncated = true;
    }
    all.iter()
        .take(OPTIONS_CAP)
        .map(|option| {
            let cleaned = clean_text(text_of(option, "text").unwrap_or(""));
            let (shown, cut) = cap_text(&cleaned, OPTION_TEXT_CAP);
            text.truncated |= cut;
            json!({
                "number": option["number"],
                "text": shown,
                "acceptsText": option.get("acceptsText").cloned().unwrap_or(Value::Bool(false)),
                "widensPermissions": option.get("widensPermissions").cloned().unwrap_or(Value::Bool(false)),
            })
        })
        .collect()
}

// ------------------------------------------------------------------------------------------------------- the reads

fn head_of(core: &Core) -> KernelResult<u64> {
    Ok(feed_cursor::head(core)?.max(0) as u64)
}

/// The project, the ledger epoch and the head sequence.
pub fn feed_head(core: &Core) -> KernelResult<FeedHead> {
    Ok(FeedHead {
        project_id: core.project_id().to_string(),
        epoch: feed_cursor::epoch(core)?,
        head: head_of(core)?,
    })
}

fn read_row(core: &Core, seq: i64) -> KernelResult<Option<Row>> {
    let kernel = core.kernel();
    let raw = query_opt(
        &kernel.database,
        "SELECT entity_type, entity_id, from_state, to_state, actor_id, payload_json, created_at
           FROM controller_events WHERE project_id = ? AND sequence = ?",
        rusqlite::params![kernel.project_id, seq],
        |row| {
            Ok(RawRow {
                entity_type: row.get(0)?,
                entity_id: row.get(1)?,
                from_state: row.get(2)?,
                to_state: row.get(3)?,
                actor_id: row.get(4)?,
                payload_json: row.get(5)?,
                created_at: row.get(6)?,
            })
        },
    )?;
    Ok(raw.map(|raw| row_of(seq, raw)))
}

fn event_of(ctx: &Ctx<'_>, row: &Row, k: usize, draft: &Draft, built: Built) -> Value {
    let id = if draft.listed {
        format!("{}:{}:{}:{}", ctx.project_id, ctx.epoch, row.seq, k)
    } else {
        format!("{}:{}:{}", ctx.project_id, ctx.epoch, row.seq)
    };
    json!({
        "id": id,
        "seq": row.seq,
        "k": k,
        "ts": row.created_at,
        "type": draft.ty,
        "actorId": row.actor_id,
        "entity": {"type": built.entity_type, "id": built.entity_id},
        "from": built.from,
        "to": built.to,
        "body": Value::Object(built.body),
    })
}

/// A page of the feed: the events after `after`, at most `limit` (1 to 500) of them, within `MAX_SCAN_ROWS` row reads and
/// `MAX_PAGE_BYTES`; the first event is always returned. `anchor` is the history anchor the caller holds for `after.seq`
/// (see `feed_cursor::anchor`); `None` leaves it unchecked.
pub fn feed_page(
    core: &Core,
    after: Position,
    limit: u32,
    anchor: Option<&str>,
) -> KernelResult<FeedPage> {
    feed_page_with(core, after, limit, anchor, MAX_SCAN_ROWS, MAX_PAGE_BYTES)
}

/// `feed_page` with other budgets (tests use small ones to reach the limits).
#[doc(hidden)]
pub fn feed_page_with(
    core: &Core,
    after: Position,
    limit: u32,
    anchor: Option<&str>,
    max_rows: usize,
    max_bytes: usize,
) -> KernelResult<FeedPage> {
    if limit == 0 || limit > MAX_LIMIT {
        return Err(KernelError::type_error(format!(
            "limit must be between 1 and {MAX_LIMIT}"
        )));
    }
    if after.seq == 0 && anchor.is_some() {
        return Err(KernelError::type_error(
            "a position before the ledger takes no anchor",
        ));
    }
    let mut ctx = Ctx {
        core,
        project_id: core.project_id().to_string(),
        epoch: feed_cursor::epoch(core)?,
        head: head_of(core)?,
        reads: 0,
        max_rows,
        max_bytes,
        cache: HashMap::new(),
    };
    let mut page = FeedPage {
        project_id: ctx.project_id.clone(),
        epoch: ctx.epoch.clone(),
        head: ctx.head,
        anchor_ok: true,
        ahead: false,
        events: Vec::new(),
        scanned: after,
        has_more: false,
        rows_read: 0,
    };
    if after.seq > ctx.head {
        page.ahead = true;
        page.anchor_ok = false;
        return Ok(page);
    }
    if let (true, Some(expected)) = (after.seq > 0, anchor) {
        if feed_cursor::anchor(core, after.seq as i64)?.as_deref() != Some(expected) {
            page.anchor_ok = false;
            return Ok(page);
        }
    }
    match fill(&mut ctx, &mut page, after, limit) {
        Ok(()) | Err(Flow::Stop) => {}
        Err(Flow::Error(error)) => return Err(error),
    }
    page.rows_read = ctx.reads;
    Ok(page)
}

fn fill(ctx: &mut Ctx<'_>, page: &mut FeedPage, after: Position, limit: u32) -> Step<()> {
    // Where reading starts: after `(seq, k)` comes `(seq, k+1)` while that row has more sub-events, else `(seq+1, 0)`.
    let mut seq = after.seq as i64;
    let mut k = 0usize;
    if after.seq > 0 {
        ctx.charge()?;
        let more = match read_row(ctx.core, after.seq as i64)? {
            Some(row) => classify(ctx, &row)?.len() > after.k as usize + 1,
            None => false,
        };
        if more {
            k = after.k as usize + 1;
        } else {
            seq += 1;
        }
    } else {
        seq = 1;
    }
    let mut bytes = PAGE_ENVELOPE_BYTES;
    let head = ctx.head as i64;
    let mut done_in_row_more = false;
    while seq <= head {
        if let Err(stop) = ctx.charge() {
            page.has_more = true;
            return Err(stop);
        }
        let row = read_row(ctx.core, seq)?;
        let drafts = match &row {
            Some(row) => match classify(ctx, row) {
                Ok(drafts) => drafts,
                Err(Flow::Stop) => {
                    page.has_more = true;
                    return Err(Flow::Stop);
                }
                Err(error) => return Err(error),
            },
            None => Vec::new(),
        };
        if drafts.is_empty() {
            page.scanned = Position {
                seq: seq as u64,
                k: 0,
            };
            seq += 1;
            k = 0;
            continue;
        }
        let row = row.expect("a row with drafts exists");
        for index in k..drafts.len() {
            let built = match build(ctx, &row, &drafts[index]) {
                Ok(built) => built,
                Err(Flow::Stop) => {
                    page.has_more = true;
                    return Err(Flow::Stop);
                }
                Err(error) => return Err(error),
            };
            let Some(built) = built else {
                // The entity row is gone: nothing to show; the position is still examined.
                page.scanned = Position {
                    seq: seq as u64,
                    k: index as u32,
                };
                continue;
            };
            let event = event_of(ctx, &row, index, &drafts[index], built);
            let size = serde_json::to_string(&event)
                .map_err(|e| Flow::Error(KernelError::from(e)))?
                .len();
            debug_assert!(size <= MAX_EVENT_BYTES);
            if !page.events.is_empty() && bytes + size + 1 > ctx.max_bytes {
                page.has_more = true;
                return Ok(());
            }
            bytes += size + 1;
            page.events.push(event);
            page.scanned = Position {
                seq: seq as u64,
                k: index as u32,
            };
            if page.events.len() as u32 == limit {
                done_in_row_more = index + 1 < drafts.len();
                page.has_more = (seq < head) || done_in_row_more;
                return Ok(());
            }
        }
        seq += 1;
        k = 0;
    }
    page.has_more = false;
    let _ = done_in_row_more;
    Ok(())
}

// ------------------------------------------------------------------------------------------------- helpers for tests

/// `(sequence, entity_type, action, from, to)` of a ledger row.
#[doc(hidden)]
pub type LedgerRow = (i64, String, String, Option<String>, Option<String>);

/// Every ledger row of the project, for tests and diagnostics.
#[doc(hidden)]
pub fn ledger_rows(core: &Core) -> KernelResult<Vec<LedgerRow>> {
    let kernel = core.kernel();
    query_all(
        &kernel.database,
        "SELECT sequence, entity_type, COALESCE(json_extract(payload_json, '$.action'), ''), from_state, to_state
           FROM controller_events WHERE project_id = ? ORDER BY sequence",
        [&kernel.project_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
    )
}
