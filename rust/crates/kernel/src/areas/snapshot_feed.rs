//! The kernel snapshot, read in pages: what is waiting for the user, the agents, the in-progress plans and the run, each
//! item with a request id and a content hash. Nothing is written: every function takes `&Core` and reads.
//!
//! # Handoff
//! A client takes [`snapshot_head`] (the head `H` of the ledger and the run), reads every page of every list (each page is
//! read at its own head, possibly newer than `H`) and then applies the events after `H` (`event_feed::feed_page`), using
//! their `body.effects`. An item that changed after `H` may appear in a page and also as an event; the client de-duplicates
//! by item key. The head of a page is read before its rows, so a page is never older than the head it states.
//!
//! # Items (serde, camelCase)
//! | type | fields |
//! |---|---|
//! | `WaitingItem` | `key`, `requestId`, `kind`, `since`, `summary`, `contentHash`, `needsTerminal`, `integrationState`, `branch`, `headSha` |
//! | `AgentItem` | `key`, `agentId`, `kind`, `role`, `generation`, `state`, `paused`, `task`, `branch`, `requestId`, `contentHash` |
//! | `PlanItem` | `key`, `planId`, `sequence`, `title`, `state`, `packages {total, assigned, reported, integrated}`, `packageDetails`, `packageDetailsOmitted` |
//! | `RunItem` | `requestId` ("run"), `state`, `paused`, `contentHash` |
//! | `Page<T>` | `head`, `items`, `next` |
//!
//! An agent item has no `stalled` field. Text (`summary`, `task`, `branch`, `title`) is cleaned and cut to [`TEXT_CAP`]
//! bytes. `branch` is the branch of the agent's pane (null without a pane). The `contentHash` of an agent is a decision
//! binding, not a change detector: a branch rename changes no hash and arrives as an invalidate, and a client re-reads
//! an item by key on every invalidate (and plans, which have no hash, are always re-read).
//!
//! `packageDetails` lists at most [`MAX_PACKAGE_DETAILS`] packages of a plan (`packageDetailsOmitted` counts the rest):
//! live packages first (in the order of the approved revision, then rows the body does not name), cancelled ones after.
//! Each is `id`, `title`, `role`, `assigneeAgentId`, `state` (`unassigned`, `assigned`, `reported`, `integrated`,
//! `confirmed` or `cancelled`), `dependsOn`, `review` (`passed`, `findings` or null), `integrationId` and
//! `integrationState` (null serializes as null). A plan without a readable approved revision lists its rows by package id
//! with empty title and role. `packages.integrated` equals the details in state `integrated` plus `confirmed`. The new
//! fields (`branch`, `packageDetails`, `packageDetailsOmitted`) are not yet in the protocol doc.
//! A plan with a sign-off is listed with the state `signed_off` while one of its sign-offs is on an integration that is
//! neither confirmed nor discarded, and leaves when that integration is confirmed or discarded. A cancelled plan is never
//! listed. An integration confirm or discard raises the bare plans invalidate.
//! A review verdict raises the bare plans invalidate but no invalidate of the plan's item key, and an integration start
//! raises no plans invalidate yet, so a client holding the item can keep a stale `review` or a running
//! `integrationState` until it re-reads plans.
//!
//! # Waiting items
//! `integrationState` (`merged` or `conflicted`), `branch` and `headSha` are filled for kind `integration` and serialize as
//! null for every other kind. They are not yet in the protocol doc, nor is the rule that follows (plan-74 documents both).
//!
//! The key of an item numbers the row (`event_feed::item_key`, kinds from `KEY_KINDS`); the request id names the row by its
//! id. Both come from the one row. Items sort by kind in `KEY_KINDS` order, then by number (a design by plan sequence, then
//! package id).
//!
//! | kind | key | requestId | waits while | contentHash |
//! |---|---|---|---|---|
//! | `op` | `op:<sequence>` | `op:<proposalId>` | the proposal is `proposed` | `command_sha` |
//! | `prompt` | `prompt:<sequence>` | `prompt:<relayId>` | the relay is `captured` | `prompt_sha` |
//! | `design` | `design:<plan sequence>/<packageId>` | `design:<planId>/<packageId>` | an approved plan's assigned design package has a report and no decision on that report | sha256 of the canonical JSON of `{reportId, commit, link}` |
//! | `message` | `message:<sequence>` | `message:<messageId>` (display only) | an open, action-needed message is addressed to the user (none is today: see below) | sha256 of the canonical JSON of `{id, body}` |
//! | `integration` | `integration:<sequence>` | `integration:<integrationId>` | the integration is `merged` or `conflicted` and is not superseded | sha256 of the canonical JSON of `{id, state, branch, headSha}` |
//!
//! Messages wait only when addressed to the user. A message's recipient is an agent (a foreign key to `agents`), so no
//! message waits today: the kind stays in protocol v1 for a later user recipient, its fetch returns no candidate and
//! reads nothing. The backlog of messages to an agent stays visible on that agent.
//!
//! A `merged` or `conflicted` integration `I` is superseded, and does not wait, when a later integration `J` (higher
//! sequence) shares a plan with `I` and is `merged` or `confirmed`, or when every plan of `I` has a sign-off on another
//! integration or is cancelled. The plans of an integration are those of the plan packages of its reports; an
//! integration with no plan package (a follow-up report only) is never superseded. One aggregated statement per built
//! integration decides this, charged as a statement and its row (two reads).
//!
//! The canonical JSON has its keys sorted, no whitespace and `serde_json`'s string escaping. `link` is the link of the
//! current decision of the package, or null. `needsTerminal` is true for a prompt whose options all widen permissions or take
//! text, or that lists no usable option (an unknown dialog), and false for every other kind.
//!
//! An agent's request id is `agent:<agentId>` and the run's is `run`. The hash of an agent is of `{agentId, generation,
//! state, paused, openPauseId}` (one `pauses_one_open_per_scope` lookup), the hash of the run of `{state, paused,
//! openPauseId}`. Only visible fields are in them, so a resumed agent or run may get a hash it had before: a decision on a
//! pause or a resume must bind to a request nonce as well as to the hash. [`content_hash`] takes every request id above
//! and returns `None` once the item is not waiting.
//!
//! # Bounds
//! A call reads at most [`MAX_READS`] rows (a skipped candidate counts), returns at most [`MAX_LIMIT`] items and about
//! [`MAX_PAGE_BYTES`] serialized bytes. `next` is the item key of the last row examined (an item or a skipped candidate),
//! to be passed back as `after`; it is `None` only when no candidate is left. A page may therefore hold no item and still
//! have a `next`.

use super::event_feed::{item_key, parse_item_key, KEY_KINDS};
use super::{design_approvals, feed_cursor, plan_packages, user_messages};
use crate::canonical::sha256;
use crate::core::Core;
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{cap_text, clean_text, query_all, query_opt as raw_query_opt};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::cell::Cell;
use std::collections::HashMap;

/// The most row reads one call makes.
pub const MAX_READS: usize = 2000;
/// The most items one page returns.
pub const MAX_LIMIT: u32 = 100;
/// The most serialized bytes of the items of a page.
pub const MAX_PAGE_BYTES: usize = 512 * 1024;
/// The most bytes of a text field of an item.
pub const TEXT_CAP: usize = 1024;
/// The most package details of a plan item.
pub const MAX_PACKAGE_DETAILS: usize = 64;
/// The most bytes of the title of a package detail.
const DETAIL_TITLE_CAP: usize = 256;
/// The most bytes of the role of a package detail.
const DETAIL_ROLE_CAP: usize = 64;
/// Rows fetched with one query.
const CHUNK: usize = 256;

// ---------------------------------------------------------------------------------------------------------- results

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitingItem {
    pub key: String,
    pub request_id: String,
    pub kind: String,
    pub since: String,
    pub summary: String,
    pub content_hash: String,
    pub needs_terminal: bool,
    pub integration_state: Option<String>,
    pub branch: Option<String>,
    pub head_sha: Option<String>,
    /// What an integration brought and how it fared (see [`integration_details`]); null for every other kind.
    pub integration_details: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentItem {
    pub key: String,
    pub agent_id: String,
    pub kind: String,
    pub role: String,
    pub generation: i64,
    pub state: String,
    pub paused: bool,
    pub task: Option<String>,
    pub branch: Option<String>,
    pub request_id: String,
    pub content_hash: String,
    pub activity: Option<String>,
    pub activity_since: Option<String>,
    pub activity_seq: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct PackageCounts {
    pub total: u32,
    pub assigned: u32,
    pub reported: u32,
    pub integrated: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageDetail {
    pub id: String,
    pub title: String,
    pub role: String,
    pub assignee_agent_id: Option<String>,
    pub state: String,
    pub depends_on: Vec<String>,
    pub review: Option<String>,
    pub integration_id: Option<String>,
    pub integration_state: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanItem {
    pub key: String,
    pub plan_id: String,
    pub sequence: i64,
    pub title: String,
    pub state: String,
    pub packages: PackageCounts,
    pub package_details: Vec<PackageDetail>,
    pub package_details_omitted: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunItem {
    pub request_id: String,
    pub state: String,
    pub paused: bool,
    pub content_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotHead {
    pub epoch: String,
    pub head: u64,
    /// The `feed_cursor::anchor` of `head` (null for an empty ledger).
    pub anchor: Option<String>,
    pub run: RunItem,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub head: u64,
    pub items: Vec<T>,
    pub next: Option<String>,
}

// ------------------------------------------------------------------------------------------------------------ helpers

/// The canonical JSON of `value`: keys sorted, no whitespace, `serde_json` string escaping.
pub fn canonical(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .into_iter()
                .map(|key| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(key).expect("a string serializes"),
                        canonical(&map[key])
                    )
                })
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(canonical).collect::<Vec<_>>().join(",")
        ),
        other => serde_json::to_string(other).expect("a value serializes"),
    }
}

/// The text a design item is hashed over (canonical JSON of what the user sees).
pub fn design_hash_input(report_id: &str, commit: &str, link: Option<&str>) -> String {
    canonical(&json!({"reportId": report_id, "commit": commit, "link": link}))
}

/// The text a message item is hashed over.
pub fn message_hash_input(id: &str, body: &str) -> String {
    canonical(&json!({"id": id, "body": body}))
}

fn shown(text: &str) -> String {
    let cleaned = clean_text(text);
    cap_text(&cleaned, TEXT_CAP).0.to_string()
}

/// Cleaned text cut to `cap` bytes on a char boundary.
fn shown_to(text: &str, cap: usize) -> String {
    let cleaned = clean_text(text);
    cap_text(&cleaned, cap).0.to_string()
}

fn invalid(message: impl Into<String>) -> KernelError {
    KernelError::type_error(message)
}

/// The ledger head: a statement and its row.
fn head_of(core: &Core) -> KernelResult<u64> {
    tick(2);
    Ok(feed_cursor::head(core)?.max(0) as u64)
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

/// Rows of `sql` as objects of the named columns.
fn rows(
    core: &Core,
    sql: &str,
    params: Vec<rusqlite::types::Value>,
    columns: &[&str],
) -> KernelResult<Vec<Map<String, Value>>> {
    let found = query_all(
        &core.kernel().database,
        sql,
        rusqlite::params_from_iter(params),
        |row| {
            let mut object = Map::new();
            for (index, name) in columns.iter().enumerate() {
                object.insert((*name).to_string(), sql_value(row.get_ref(index)?));
            }
            Ok(object)
        },
    )?;
    tick(1 + found.len());
    Ok(found)
}

thread_local! {
    /// The statements and rows this thread's snapshot reads actually ran (what a test compares with the budget).
    static ACTUAL: Cell<usize> = const { Cell::new(0) };
}

fn tick(n: usize) {
    ACTUAL.with(|cell| cell.set(cell.get() + n));
}

/// Resets the count of reads this thread has made (tests).
#[doc(hidden)]
pub fn actual_reads_reset() {
    ACTUAL.with(|cell| cell.set(0));
}

/// The reads (each statement, and each row it returned) this thread made since the reset (tests).
#[doc(hidden)]
pub fn actual_reads() -> usize {
    ACTUAL.with(Cell::get)
}

/// `helpers::query_opt`, counted.
fn query_opt<T>(
    database: &capstan_ledger::Database,
    sql: &str,
    params: impl rusqlite::Params,
    map: impl FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> KernelResult<Option<T>> {
    let found = raw_query_opt(database, sql, params, map)?;
    tick(1 + usize::from(found.is_some()));
    Ok(found)
}

/// Counts a lookup made by another module: its statement, and its row when it found one.
fn counted<T>(found: KernelResult<Option<T>>) -> KernelResult<Option<T>> {
    if let Ok(found) = &found {
        tick(1 + usize::from(found.is_some()));
    }
    found
}

fn text(row: &Map<String, Value>, key: &str) -> String {
    row.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn int(row: &Map<String, Value>, key: &str) -> i64 {
    row.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn project(core: &Core) -> rusqlite::types::Value {
    rusqlite::types::Value::Text(core.project_id().to_string())
}

fn sql_text(value: &str) -> rusqlite::types::Value {
    rusqlite::types::Value::Text(value.to_string())
}

fn sql_int(value: i64) -> rusqlite::types::Value {
    rusqlite::types::Value::Integer(value)
}

/// The row reads of one call.
struct Reads {
    used: usize,
    max: usize,
}

impl Reads {
    /// Charges a lookup that has run: its statement, and its row when it found one.
    fn looked_up<T>(&mut self, found: &Option<T>) {
        self.used += 1 + usize::from(found.is_some());
    }

    fn room(&self) -> usize {
        self.max.saturating_sub(self.used)
    }
}

fn check_limit(limit: u32) -> KernelResult<usize> {
    if limit == 0 || limit > MAX_LIMIT {
        return Err(invalid(format!("limit must be between 1 and {MAX_LIMIT}")));
    }
    Ok(limit as usize)
}

/// A number as the key writes it: decimal, positive, no padding.
fn key_number(text: &str) -> Option<i64> {
    let number: i64 = text.parse().ok()?;
    (number > 0 && number.to_string() == text).then_some(number)
}

// ----------------------------------------------------------------------------------------------------------- the run

fn open_pause(core: &Core, scope: &str, agent_id: &str) -> KernelResult<Option<String>> {
    let kernel = core.kernel();
    query_opt(
        &kernel.database,
        "SELECT pause_id FROM pauses WHERE project_id = ? AND scope = ? AND COALESCE(agent_id, '') = ? AND resumed_at IS NULL",
        [kernel.project_id.as_str(), scope, agent_id],
        |row| row.get::<_, String>(0),
    )
}

fn run_hash(state: &str, paused: bool, open_pause_id: Option<&str>) -> String {
    sha256(&canonical(&json!({
        "state": state,
        "paused": paused,
        "openPauseId": open_pause_id,
    })))
}

fn run_item(core: &Core) -> KernelResult<RunItem> {
    let kernel = core.kernel();
    let state = query_opt(
        &kernel.database,
        "SELECT state FROM run_controls WHERE project_id = ?",
        [&kernel.project_id],
        |row| row.get::<_, String>(0),
    )?
    .ok_or_else(|| KernelError::controller("run control record is missing"))?;
    let pause = open_pause(core, "run", "")?;
    let paused = state == "paused";
    Ok(RunItem {
        request_id: "run".to_string(),
        content_hash: run_hash(&state, paused, pause.as_deref()),
        state,
        paused,
    })
}

/// The epoch and head of the ledger and the run: where a client starts reading pages.
pub fn snapshot_head(core: &Core) -> KernelResult<SnapshotHead> {
    let head = head_of(core)?;
    // The epoch is a statement and, in a ledger that has an event, its row.
    tick(2);
    let epoch = feed_cursor::epoch(core)?;
    // The anchor is a statement and, in a ledger that has an event, its row.
    tick(2);
    Ok(SnapshotHead {
        epoch,
        head,
        anchor: feed_cursor::anchor(core, head as i64)?,
        run: run_item(core)?,
    })
}

// --------------------------------------------------------------------------------------------------------- the agents

/// Which agent rows a read wants.
enum AgentSel<'a> {
    After(Option<&'a str>),
    One(&'a str),
}

fn agent_hash(
    agent_id: &str,
    generation: i64,
    state: &str,
    paused: bool,
    pause: Option<&str>,
) -> String {
    sha256(&canonical(&json!({
        "agentId": agent_id,
        "generation": generation,
        "state": state,
        "paused": paused,
        "openPauseId": pause,
    })))
}

/// The hash of an agent from its visible fields (a new agent has no open pause).
pub fn agent_content_hash(
    agent_id: &str,
    generation: i64,
    state: &str,
    paused: bool,
    open_pause_id: Option<&str>,
) -> String {
    agent_hash(agent_id, generation, state, paused, open_pause_id)
}

fn agent_query(
    core: &Core,
    sel: AgentSel<'_>,
    limit: usize,
) -> (String, Vec<rusqlite::types::Value>) {
    let base = "SELECT a.agent_id, a.kind, a.role_name, a.generation, a.state,
                       COALESCE(pn.task_title, pn.task_ref), pn.branch,
                       (SELECT pause_id FROM pauses WHERE project_id = a.project_id AND scope = 'agent'
                          AND COALESCE(agent_id, '') = a.agent_id AND resumed_at IS NULL),
                       (SELECT h.herdr_state FROM agent_state_history h WHERE h.project_id = a.project_id AND h.agent_id = a.agent_id ORDER BY h.sequence DESC LIMIT 1),
                       (SELECT h.observed_at FROM agent_state_history h WHERE h.project_id = a.project_id AND h.agent_id = a.agent_id ORDER BY h.sequence DESC LIMIT 1),
                       (SELECT h.sequence FROM agent_state_history h WHERE h.project_id = a.project_id AND h.agent_id = a.agent_id ORDER BY h.sequence DESC LIMIT 1)
                  FROM agents a
                  LEFT JOIN agent_panes pn ON pn.project_id = a.project_id AND pn.agent_id = a.agent_id
                 WHERE a.project_id = ? AND a.state = 'active'";
    let (sql, params) = match sel {
        AgentSel::After(after) => (
            format!("{base} AND a.agent_id > ? ORDER BY a.agent_id LIMIT ?"),
            vec![
                project(core),
                sql_text(after.unwrap_or("")),
                sql_int(limit as i64),
            ],
        ),
        AgentSel::One(id) => (
            format!("{base} AND a.agent_id = ?"),
            vec![project(core), sql_text(id)],
        ),
    };
    (sql, params)
}

/// The two statements `agent_rows` runs, for the page and for one item (tests: their query plans).
#[doc(hidden)]
pub fn agent_sql_texts(core: &Core) -> [String; 2] {
    [
        agent_query(core, AgentSel::After(None), 1).0,
        agent_query(core, AgentSel::One("x"), 1).0,
    ]
}

fn agent_rows(core: &Core, sel: AgentSel<'_>, limit: usize) -> KernelResult<Vec<AgentItem>> {
    let (sql, params) = agent_query(core, sel, limit);
    let found = rows(
        core,
        &sql,
        params,
        &[
            "agentId",
            "kind",
            "role",
            "generation",
            "state",
            "task",
            "branch",
            "pauseId",
            "activity",
            "activitySince",
            "activitySeq",
        ],
    )?;
    Ok(found
        .into_iter()
        .map(|row| {
            let agent_id = text(&row, "agentId");
            let generation = int(&row, "generation");
            let state = text(&row, "state");
            let pause = row
                .get("pauseId")
                .and_then(Value::as_str)
                .map(str::to_string);
            let paused = pause.is_some();
            AgentItem {
                key: item_key("agents", "agent", &agent_id),
                request_id: format!("agent:{agent_id}"),
                content_hash: agent_hash(&agent_id, generation, &state, paused, pause.as_deref()),
                kind: text(&row, "kind"),
                role: text(&row, "role"),
                task: row.get("task").and_then(Value::as_str).map(shown),
                branch: row.get("branch").and_then(Value::as_str).map(shown),
                activity: row
                    .get("activity")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                activity_since: row
                    .get("activitySince")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                activity_seq: row.get("activitySeq").and_then(Value::as_i64),
                agent_id,
                generation,
                state,
                paused,
            }
        })
        .collect())
}

/// The active agents after the agent id `after`, in agent id order.
pub fn agents_page(core: &Core, after: Option<&str>, limit: u32) -> KernelResult<Page<AgentItem>> {
    Ok(agents_page_counted(core, after, limit, MAX_READS)?.0)
}

/// `agents_page` with another read budget; also returns the reads made (tests).
#[doc(hidden)]
pub fn agents_page_counted(
    core: &Core,
    after: Option<&str>,
    limit: u32,
    max_reads: usize,
) -> KernelResult<(Page<AgentItem>, usize)> {
    let limit = check_limit(limit)?;
    if let Some(after) = after {
        if parse_item_key("agents", after).is_none() {
            return Err(invalid("after is not an agent key"));
        }
    }
    let head = head_of(core)?;
    // The fetch is a statement and a row for each agent; an agent is charged two (its row and its pause lookup), and
    // the page's head and fetch statement three. No more rows are fetched than the budget can pay for.
    let want = (limit + 1).min(max_reads.saturating_sub(3) / 2 + 1);
    let found = agent_rows(core, AgentSel::After(after), want)?;
    let costs = vec![2; found.len()];
    paged_costed(head, found, &costs, limit, max_reads, |item| {
        item.key.clone()
    })
}

// ---------------------------------------------------------------------------------------------------------- the plans

/// The plans the plans section lists, before the paging condition. A plan with a sign-off is listed, as `signed_off`,
/// while one of its sign-offs is on an integration that is neither confirmed nor discarded; a plan with none is listed in
/// its own state. The sign-off tests are in-row subselects read through the primary keys of `plan_signoffs` and
/// `integrations`, so they are not charged as reads of their own.
#[doc(hidden)]
pub const PLAN_ROWS_BASE: &str = "SELECT plan_id, sequence, title,
                       CASE WHEN EXISTS (SELECT 1 FROM plan_signoffs s WHERE s.project_id = plans.project_id AND s.plan_id = plans.plan_id)
                            THEN 'signed_off' ELSE state END, approved_revision FROM plans
                 WHERE project_id = ? AND state IN ('draft', 'in_review', 'approved') AND cancelled_at IS NULL
                   AND (NOT EXISTS (SELECT 1 FROM plan_signoffs s WHERE s.project_id = plans.project_id AND s.plan_id = plans.plan_id)
                        OR EXISTS (SELECT 1 FROM plan_signoffs s JOIN integrations i
                                     ON i.project_id = s.project_id AND i.integration_id = s.integration_id
                                    WHERE s.project_id = plans.project_id AND s.plan_id = plans.plan_id
                                      AND i.state NOT IN ('confirmed', 'discarded')))";

fn plan_rows(
    core: &Core,
    after: i64,
    one: Option<i64>,
    limit: usize,
) -> KernelResult<Vec<Map<String, Value>>> {
    let base = PLAN_ROWS_BASE;
    let (sql, params) = match one {
        Some(sequence) => (
            format!("{base} AND sequence = ?"),
            vec![project(core), sql_int(sequence)],
        ),
        None => (
            format!("{base} AND sequence > ? ORDER BY sequence LIMIT ?"),
            vec![project(core), sql_int(after), sql_int(limit as i64)],
        ),
    };
    rows(
        core,
        &sql,
        params,
        &["planId", "sequence", "title", "state", "approvedRevision"],
    )
}

/// One row of `plan_packages` as the details read it.
struct PackageRow {
    id: String,
    assignee: Option<String>,
    assigned_at: Option<String>,
    cancelled: bool,
}

/// What the approved revision says about a package.
struct BodyPackage {
    id: String,
    title: String,
    role: String,
    depends_on: Vec<String>,
}

/// The packages of the approved revision of a plan, or `None` when there is none or it cannot be read (the plan then
/// lists its rows alone).
fn approved_packages(
    core: &Core,
    plan_id: &str,
    revision: Option<i64>,
) -> KernelResult<Option<Vec<BodyPackage>>> {
    let Some(revision) = revision else {
        return Ok(None);
    };
    let kernel = core.kernel();
    let Some(body) = query_opt(
        &kernel.database,
        "SELECT body_json FROM plan_revisions WHERE project_id = ? AND plan_id = ? AND revision = ?",
        rusqlite::params![kernel.project_id.as_str(), plan_id, revision],
        |row| row.get::<_, String>(0),
    )?
    else {
        eprintln!("snapshot_feed: plan {plan_id} has no revision {revision}; listing its package rows only");
        return Ok(None);
    };
    // The stored body is the camelCase `PlanBody` JSON, which `parse_plan_body` (snake_case input) does not read.
    match stored_packages(&body) {
        Some(packages) => Ok(Some(packages)),
        None => {
            eprintln!("snapshot_feed: the approved body of plan {plan_id} cannot be read; listing its package rows only");
            Ok(None)
        }
    }
}

/// The `id`, `title`, `role` and `dependsOn` of each package of a stored plan body, or `None` when it has no such list.
fn stored_packages(body: &str) -> Option<Vec<BodyPackage>> {
    let body: Value = serde_json::from_str(body).ok()?;
    let list = body.get("packages")?.as_array()?;
    let text_of = |package: &Value, key: &str| {
        package
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    list.iter()
        .map(|package| {
            Some(BodyPackage {
                id: package.get("id")?.as_str()?.to_string(),
                title: text_of(package, "title"),
                role: text_of(package, "role"),
                depends_on: package
                    .get("dependsOn")
                    .and_then(Value::as_array)
                    .map(|ids| {
                        ids.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// `?, ?, ...` for `n` parameters.
fn placeholders(n: usize) -> String {
    vec!["?"; n].join(", ")
}

/// The plan item of a row. Its package counts take the list of packages (a statement and a row each, cancelled ones
/// included) and, for each live package that is assigned, a lookup of its report and, when it has one, a lookup of the
/// integration that holds it (each a statement and, when it finds one, a row). The details of the packages shown take
/// the approved revision and two batched lookups (the reviews and the integrations of the reports shown).
fn plan_item(core: &Core, row: &Map<String, Value>) -> KernelResult<PlanItem> {
    let kernel = core.kernel();
    let plan_id = text(row, "planId");
    let package_rows = rows(
        core,
        "SELECT assignee_agent_id, assigned_at, package_id, cancelled_at FROM plan_packages
          WHERE project_id = ? AND plan_id = ? ORDER BY package_id",
        vec![project(core), sql_text(&plan_id)],
        &["assignee", "assignedAt", "packageId", "cancelledAt"],
    )?
    .iter()
    .map(|package| PackageRow {
        id: text(package, "packageId"),
        assignee: package
            .get("assignee")
            .and_then(Value::as_str)
            .map(str::to_string),
        assigned_at: package
            .get("assignedAt")
            .and_then(Value::as_str)
            .map(str::to_string),
        cancelled: !package.get("cancelledAt").is_none_or(Value::is_null),
    })
    .collect::<Vec<_>>();
    let body = approved_packages(
        core,
        &plan_id,
        row.get("approvedRevision").and_then(Value::as_i64),
    )?;
    let mut counts = PackageCounts {
        total: 0,
        assigned: 0,
        reported: 0,
        integrated: 0,
    };
    // The report of each live assigned package (None when it has none).
    let mut reports: HashMap<&str, Option<String>> = HashMap::new();
    for package in package_rows.iter().filter(|package| !package.cancelled) {
        counts.total += 1;
        let (Some(assignee), Some(assigned_at)) =
            (package.assignee.as_deref(), package.assigned_at.as_deref())
        else {
            continue;
        };
        counts.assigned += 1;
        let report = counted(plan_packages::package_report(
            kernel,
            &plan_id,
            &package.id,
            Some(assignee),
            Some(assigned_at),
        ))?;
        if let Some(report) = &report {
            counts.reported += 1;
            let integrated = query_opt(
                &kernel.database,
                "SELECT 1 FROM integration_reports ir
                   JOIN integrations i ON i.project_id = ir.project_id AND i.integration_id = ir.integration_id
                  WHERE ir.project_id = ? AND ir.report_id = ? AND i.state IN ('merged', 'confirmed')",
                [kernel.project_id.as_str(), report.as_str()],
                |_| Ok(()),
            )?;
            if integrated.is_some() {
                counts.integrated += 1;
            }
        }
        reports.insert(package.id.as_str(), report);
    }

    // The packages in the order they are listed: live first (body order, then rows the body does not name), cancelled after.
    let by_id: HashMap<&str, &PackageRow> = package_rows
        .iter()
        .map(|package| (package.id.as_str(), package))
        .collect();
    struct Slot<'a> {
        id: &'a str,
        title: String,
        role: String,
        depends_on: Vec<String>,
        row: Option<&'a PackageRow>,
    }
    let mut slots: Vec<Slot<'_>> = Vec::new();
    match &body {
        Some(body) => {
            for package in body {
                slots.push(Slot {
                    id: &package.id,
                    title: package.title.clone(),
                    role: package.role.clone(),
                    depends_on: package.depends_on.clone(),
                    row: by_id.get(package.id.as_str()).copied(),
                });
            }
            let named: std::collections::HashSet<&str> =
                body.iter().map(|package| package.id.as_str()).collect();
            for package in package_rows
                .iter()
                .filter(|package| !named.contains(package.id.as_str()))
            {
                slots.push(Slot {
                    id: &package.id,
                    title: String::new(),
                    role: String::new(),
                    depends_on: Vec::new(),
                    row: Some(package),
                });
            }
        }
        None => {
            for package in &package_rows {
                slots.push(Slot {
                    id: &package.id,
                    title: String::new(),
                    role: String::new(),
                    depends_on: Vec::new(),
                    row: Some(package),
                });
            }
        }
    }
    let is_cancelled = |slot: &Slot<'_>| slot.row.is_some_and(|package| package.cancelled);
    let (cancelled, live): (Vec<Slot<'_>>, Vec<Slot<'_>>) =
        slots.into_iter().partition(|slot| is_cancelled(slot));
    let listed: Vec<Slot<'_>> = live.into_iter().chain(cancelled).collect();
    let omitted = listed.len().saturating_sub(MAX_PACKAGE_DETAILS);
    let shown_slots = &listed[..listed.len() - omitted];

    // The reports of the shown live assigned packages, and their reviews and integrations (one statement each).
    let report_ids: Vec<String> = shown_slots
        .iter()
        .filter(|slot| !is_cancelled(slot))
        .filter_map(|slot| reports.get(slot.id).cloned().flatten())
        .collect();
    let mut reviews: HashMap<String, String> = HashMap::new();
    let mut integrations: HashMap<String, (String, String)> = HashMap::new();
    if !report_ids.is_empty() {
        let marks = placeholders(report_ids.len());
        let with_ids = |extra: Vec<rusqlite::types::Value>| {
            let mut params = extra;
            params.extend(report_ids.iter().map(|id| sql_text(id)));
            params
        };
        for found in rows(
            core,
            &format!(
                "SELECT r.subject_report_id, r.state FROM reviews r
                  WHERE r.project_id = ? AND r.subject_report_id IN ({marks}) AND r.state IN ('passed', 'findings')
                    AND r.sequence = (SELECT MAX(r2.sequence) FROM reviews r2
                                       WHERE r2.project_id = r.project_id AND r2.subject_report_id = r.subject_report_id
                                         AND r2.state IN ('passed', 'findings'))"
            ),
            with_ids(vec![project(core)]),
            &["reportId", "state"],
        )? {
            reviews.insert(text(&found, "reportId"), text(&found, "state"));
        }
        for found in rows(
            core,
            &format!(
                "SELECT ir.report_id, i.integration_id, i.state FROM integration_reports ir
                   JOIN integrations i ON i.project_id = ir.project_id AND i.integration_id = ir.integration_id
                  WHERE ir.project_id = ? AND ir.report_id IN ({marks})
                    AND i.integration_id = (SELECT i2.integration_id FROM integration_reports ir2
                          JOIN integrations i2 ON i2.project_id = ir2.project_id AND i2.integration_id = ir2.integration_id
                         WHERE ir2.project_id = ir.project_id AND ir2.report_id = ir.report_id
                         ORDER BY CASE i2.state WHEN 'confirmed' THEN 0 WHEN 'merged' THEN 1 ELSE 2 END, i2.sequence DESC
                         LIMIT 1)"
            ),
            with_ids(vec![project(core)]),
            &["reportId", "integrationId", "state"],
        )? {
            integrations.insert(
                text(&found, "reportId"),
                (text(&found, "integrationId"), text(&found, "state")),
            );
        }
    }

    let package_details: Vec<PackageDetail> = shown_slots
        .iter()
        .map(|slot| {
            let mut detail = PackageDetail {
                id: slot.id.to_string(),
                title: shown_to(&slot.title, DETAIL_TITLE_CAP),
                role: shown_to(&slot.role, DETAIL_ROLE_CAP),
                assignee_agent_id: None,
                state: "unassigned".to_string(),
                depends_on: slot.depends_on.clone(),
                review: None,
                integration_id: None,
                integration_state: None,
            };
            let Some(package) = slot.row else {
                return detail;
            };
            if package.cancelled {
                detail.state = "cancelled".to_string();
                return detail;
            }
            let (Some(assignee), Some(_)) = (&package.assignee, &package.assigned_at) else {
                return detail;
            };
            detail.assignee_agent_id = Some(assignee.clone());
            detail.state = "assigned".to_string();
            let Some(report) = reports.get(slot.id).cloned().flatten() else {
                return detail;
            };
            detail.state = "reported".to_string();
            detail.review = reviews.get(&report).cloned();
            if let Some((id, state)) = integrations.get(&report) {
                detail.state = match state.as_str() {
                    "merged" => "integrated",
                    "confirmed" => "confirmed",
                    _ => "reported",
                }
                .to_string();
                detail.integration_id = Some(id.clone());
                detail.integration_state = Some(state.clone());
            }
            detail
        })
        .collect();

    let sequence = int(row, "sequence");
    Ok(PlanItem {
        key: item_key("plans", "plan", &sequence.to_string()),
        title: shown(&text(row, "title")),
        state: text(row, "state"),
        plan_id,
        sequence,
        packages: counts,
        package_details,
        package_details_omitted: omitted as u32,
    })
}

/// The in-progress plans after the plan sequence `after`: not superseded or cancelled, and not signed off unless a sign-off's
/// integration is still unconfirmed (such a plan has the state `signed_off`).
pub fn plans_page(core: &Core, after: Option<&str>, limit: u32) -> KernelResult<Page<PlanItem>> {
    Ok(plans_page_counted(core, after, limit, MAX_READS)?.0)
}

/// `plans_page` with another read budget; also returns the reads made (tests).
#[doc(hidden)]
pub fn plans_page_counted(
    core: &Core,
    after: Option<&str>,
    limit: u32,
    max_reads: usize,
) -> KernelResult<(Page<PlanItem>, usize)> {
    plans_page_sized(core, after, limit, max_reads, MAX_PAGE_BYTES)
}

/// `plans_page_counted` with another byte cap (tests).
#[doc(hidden)]
pub fn plans_page_sized(
    core: &Core,
    after: Option<&str>,
    limit: u32,
    max_reads: usize,
    max_bytes: usize,
) -> KernelResult<(Page<PlanItem>, usize)> {
    let limit = check_limit(limit)?;
    let after = match after {
        None => 0,
        Some(key) => match parse_item_key("plans", key).and_then(|(_, id)| key_number(&id)) {
            Some(sequence) => sequence,
            None => return Err(invalid("after is not a plan key")),
        },
    };
    let head = head_of(core)?;
    // The unit is a statement and a row. The head has been read.
    let mut reads = Reads {
        used: 2,
        max: max_reads,
    };
    // A plan takes at least six (its row, the count's statement and row, the list's statement and the two lookups of the
    // reports), so no more rows than the budget pays for at that rate (and one to see whether more follow) are asked for.
    let found = plan_rows(
        core,
        after,
        None,
        (limit + 1).min(max_reads.saturating_sub(3) / 6 + 1),
    )?;
    reads.used += 1 + found.len();
    let kernel = core.kernel();
    let mut items: Vec<PlanItem> = Vec::new();
    let mut bytes = 0usize;
    let mut more = false;
    for (index, row) in found.iter().enumerate() {
        if index == limit {
            more = true;
            break;
        }
        // The packages are counted first (a statement and a row, checked before they run), then read with the plan.
        if !items.is_empty() && reads.used + 2 > reads.max {
            more = true;
            break;
        }
        reads.used += 2;
        let (rows_all, live, assigned) = query_opt(
            &kernel.database,
            "SELECT COUNT(*), COUNT(CASE WHEN cancelled_at IS NULL THEN 1 END),
                    COUNT(CASE WHEN cancelled_at IS NULL AND assignee_agent_id IS NOT NULL AND assigned_at IS NOT NULL THEN 1 END)
               FROM plan_packages WHERE project_id = ? AND plan_id = ?",
            [kernel.project_id.as_str(), text(row, "planId").as_str()],
            |r| {
                Ok((
                    r.get::<_, i64>(0)? as usize,
                    r.get::<_, i64>(1)? as usize,
                    r.get::<_, i64>(2)? as usize,
                ))
            },
        )?
        .unwrap_or((0, 0, 0));
        // The list (a statement and every row), the report lookups and integration counts of the live assigned packages
        // (four each), the approved revision (two) and the two batched lookups (a statement each and at most a row for
        // each package shown).
        let revision = if row.get("approvedRevision").is_some_and(|v| !v.is_null()) {
            2
        } else {
            0
        };
        let cost = 1 + rows_all + 4 * assigned + revision + 2 + 2 * live.min(MAX_PACKAGE_DETAILS);
        if !items.is_empty() && reads.used + cost > reads.max {
            more = true;
            break;
        }
        reads.used += cost;
        let item = plan_item(core, row)?;
        let size = serde_json::to_string(&item)?.len() + 1;
        if !items.is_empty() && bytes + size > max_bytes {
            more = true;
            break;
        }
        bytes += size;
        items.push(item);
    }
    let next = if more {
        items.last().map(|item| item.key.clone())
    } else {
        None
    };
    Ok((Page { head, items, next }, reads.used))
}

// ------------------------------------------------------------------------------------------------------------- paging

/// Cuts `found` (fetched with a limit of one more than `limit`) to a page, charging `costs[i]` reads for row `i`.
fn paged_costed<T: Serialize>(
    head: u64,
    found: Vec<T>,
    costs: &[usize],
    limit: usize,
    max_reads: usize,
    key: impl Fn(&T) -> String,
) -> KernelResult<(Page<T>, usize)> {
    let mut reads = Reads {
        used: 3,
        max: max_reads,
    };
    let mut bytes = 0usize;
    let mut items: Vec<T> = Vec::new();
    let mut more = false;
    for (index, item) in found.into_iter().enumerate() {
        if index == limit || (!items.is_empty() && reads.used + costs[index] > reads.max) {
            more = true;
            break;
        }
        let size = serde_json::to_string(&item)?.len() + 1;
        if !items.is_empty() && bytes + size > MAX_PAGE_BYTES {
            more = true;
            break;
        }
        bytes += size;
        reads.used += costs[index];
        items.push(item);
    }
    let next = if more { items.last().map(&key) } else { None };
    Ok((Page { head, items, next }, reads.used))
}

// -------------------------------------------------------------------------------------------------------- the waiting

const MESSAGE_KIND: &str = "message";

/// The kinds of the waiting section, in key order.
fn waiting_kinds() -> Vec<&'static str> {
    KEY_KINDS
        .iter()
        .filter(|kind| kind.section == "waiting")
        .map(|kind| kind.kind)
        .collect()
}

/// Where a candidate sorts in its kind: the row sequence, and the package id of a design.
type Sort = (i64, String);

/// A row that may be a waiting item.
struct Candidate {
    sort: Sort,
    /// The id of the request id (`<planId>/<packageId>` for a design).
    id: String,
    cols: Map<String, Value>,
}

enum Sel {
    After(Option<Sort>),
    /// By the number of the key.
    Key(Sort),
    /// By the id of the request id.
    Id(String),
}

fn fetch(core: &Core, kind: &str, sel: &Sel, limit: usize) -> KernelResult<Vec<Candidate>> {
    if kind == "design" {
        return fetch_design(core, sel, limit);
    }
    let (columns, id_column, from, state): (&[&str], &str, &str, String) = match kind {
        "op" => (
            &[
                "sequence",
                "proposal_id",
                "kind",
                "command",
                "command_sha",
                "created_at",
            ],
            "proposal_id",
            "operator_proposals",
            "state = 'proposed'".to_string(),
        ),
        "prompt" => (
            &[
                "sequence",
                "relay_id",
                "prompt_text",
                "options_json",
                "prompt_sha",
                "captured_at",
            ],
            "relay_id",
            "prompt_relays",
            "state = 'captured'".to_string(),
        ),
        MESSAGE_KIND => (
            &["sequence", "message_id", "body", "created_at"],
            "message_id",
            "user_messages",
            "class = 'action' AND state = 'open'".to_string(),
        ),
        "integration" => (
            &[
                "sequence",
                "integration_id",
                "state",
                "branch",
                "head_sha",
                "created_at",
            ],
            "integration_id",
            "integrations",
            "state IN ('merged', 'conflicted')".to_string(),
        ),
        other => return Err(invalid(format!("unknown kind {other}"))),
    };
    let select = columns.join(", ");
    let base = format!("SELECT {select} FROM {from} WHERE project_id = ? AND {state}");
    let (sql, params) = match sel {
        Sel::After(after) => (
            format!("{base} AND sequence > ? ORDER BY sequence LIMIT ?"),
            vec![
                project(core),
                sql_int(after.as_ref().map_or(0, |sort| sort.0)),
                sql_int(limit as i64),
            ],
        ),
        Sel::Key(sort) => (
            format!("{base} AND sequence = ?"),
            vec![project(core), sql_int(sort.0)],
        ),
        Sel::Id(id) => (
            format!("{base} AND {id_column} = ?"),
            vec![project(core), sql_text(id)],
        ),
    };
    let found = rows(core, &sql, params, columns)?;
    Ok(found
        .into_iter()
        .map(|cols| Candidate {
            sort: (int(&cols, "sequence"), String::new()),
            id: text(&cols, id_column),
            cols,
        })
        .collect())
}

fn fetch_design(core: &Core, sel: &Sel, limit: usize) -> KernelResult<Vec<Candidate>> {
    let base = "SELECT pl.sequence, pl.plan_id, p.package_id, p.assignee_agent_id, p.assigned_at, pl.approved_revision
                  FROM plan_packages p
                  JOIN plans pl ON pl.project_id = p.project_id AND pl.plan_id = p.plan_id
                 WHERE p.project_id = ? AND pl.state = 'approved' AND pl.cancelled_at IS NULL
                   AND p.cancelled_at IS NULL AND p.assignee_agent_id IS NOT NULL";
    let (sql, params) = match sel {
        Sel::After(None) => (
            format!("{base} ORDER BY pl.sequence, p.package_id LIMIT ?"),
            vec![project(core), sql_int(limit as i64)],
        ),
        Sel::After(Some((sequence, package))) => (
            format!(
                "{base} AND (pl.sequence > ? OR (pl.sequence = ? AND p.package_id > ?))
                 ORDER BY pl.sequence, p.package_id LIMIT ?"
            ),
            vec![
                project(core),
                sql_int(*sequence),
                sql_int(*sequence),
                sql_text(package),
                sql_int(limit as i64),
            ],
        ),
        Sel::Key((sequence, package)) => (
            format!("{base} AND pl.sequence = ? AND p.package_id = ?"),
            vec![project(core), sql_int(*sequence), sql_text(package)],
        ),
        Sel::Id(id) => {
            let Some((plan_id, package)) = id.rsplit_once('/') else {
                return Ok(Vec::new());
            };
            (
                format!("{base} AND pl.plan_id = ? AND p.package_id = ?"),
                vec![project(core), sql_text(plan_id), sql_text(package)],
            )
        }
    };
    let found = rows(
        core,
        &sql,
        params,
        &[
            "sequence",
            "planId",
            "packageId",
            "assignee",
            "assignedAt",
            "revision",
        ],
    )?;
    Ok(found
        .into_iter()
        .map(|cols| Candidate {
            sort: (int(&cols, "sequence"), text(&cols, "packageId")),
            id: format!("{}/{}", text(&cols, "planId"), text(&cols, "packageId")),
            cols,
        })
        .collect())
}

/// What a waiting scan remembers between candidates.
struct Scan {
    /// Plan bodies by plan id (one read each).
    bodies: HashMap<String, Option<String>>,
    /// Whether an integration item reads its details (a content hash does not need them).
    details: bool,
}

fn key_of(kind: &str, candidate: &Candidate) -> String {
    if kind == "design" {
        item_key(
            "waiting",
            kind,
            &format!("{}/{}", candidate.sort.0, candidate.sort.1),
        )
    } else {
        item_key("waiting", kind, &candidate.sort.0.to_string())
    }
}

fn needs_terminal(options_json: &str) -> bool {
    let options = serde_json::from_str::<Value>(options_json)
        .ok()
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    options.is_empty()
        || options.iter().all(|option| {
            option["widensPermissions"] == Value::Bool(true)
                || option["acceptsText"] == Value::Bool(true)
        })
}

/// The waiting item of a candidate, or `None` when it does not wait (a skipped candidate). `reads` is charged for the
/// lookups beyond the candidate row itself.
fn build(
    core: &Core,
    kind: &str,
    candidate: &Candidate,
    scan: &mut Scan,
    reads: &mut Reads,
    force: bool,
) -> KernelResult<Built> {
    let cols = &candidate.cols;
    let item = |since: String, summary: String, hash: String, terminal: bool| WaitingItem {
        key: key_of(kind, candidate),
        request_id: format!("{kind}:{}", candidate.id),
        kind: kind.to_string(),
        since,
        summary,
        content_hash: hash,
        needs_terminal: terminal,
        integration_state: None,
        branch: None,
        head_sha: None,
        integration_details: None,
    };
    Ok(Built::Done(
        match kind {
            "op" => Some(item(
                text(cols, "created_at"),
                shown(&text(cols, "command")),
                text(cols, "command_sha"),
                false,
            )),
            "prompt" => Some(item(
                text(cols, "captured_at"),
                shown(&text(cols, "prompt_text")),
                text(cols, "prompt_sha"),
                needs_terminal(&text(cols, "options_json")),
            )),
            "integration" => {
                let (state, branch) = (text(cols, "state"), text(cols, "branch"));
                let head_sha = cols.get("head_sha").and_then(Value::as_str);
                // The statement and its row, reserved before it runs (a call always examines one candidate).
                if !force && reads.used + 2 > reads.max {
                    return Ok(Built::Defer);
                }
                let superseded =
                    integration_superseded(core, &candidate.id, int(cols, "sequence"))?;
                reads.used += 2;
                let details = if superseded || !scan.details {
                    None
                } else if !force && reads.used + DETAILS_RESERVE > reads.max {
                    return Ok(Built::Defer);
                } else {
                    Some(integration_details(core, &candidate.id, reads)?)
                };
                (!superseded).then(|| {
                    let mut found = item(
                        text(cols, "created_at"),
                        shown(&format!("{state} {branch}")),
                        sha256(&canonical(&json!({
                            "id": candidate.id,
                            "state": state,
                            "branch": branch,
                            "headSha": head_sha,
                        }))),
                        false,
                    );
                    found.integration_state = Some(state.clone());
                    found.branch = Some(branch.clone());
                    found.head_sha = head_sha.map(str::to_string);
                    found.integration_details = details;
                    found
                })
            }
            MESSAGE_KIND => Some(item(
                text(cols, "created_at"),
                shown(&text(cols, "body")),
                sha256(&message_hash_input(&candidate.id, &text(cols, "body"))),
                false,
            )),
            "design" => match build_design(core, candidate, scan, reads, force)? {
                Designed::Defer => return Ok(Built::Defer),
                Designed::Skip => None,
                Designed::Item(since, summary, hash) => Some(item(since, summary, hash, false)),
            },
            _ => None,
        }
        .map(Box::new),
    ))
}

/// The packages of `pp` whose accepted report `r` is in an integration, as in `plan_packages` (`PACKAGE_REPORT_PREDICATE`).
const PACKAGE_REPORT: &str = "pp.assignee_agent_id = r.agent_id AND r.state = 'accepted' AND pp.assigned_at <= r.created_at
  AND EXISTS (SELECT 1 FROM agents a WHERE a.project_id = r.project_id AND a.agent_id = r.agent_id AND a.generation = r.generation)
  AND NOT EXISTS (
    SELECT 1 FROM plan_packages other
    WHERE other.project_id = r.project_id AND other.assignee_agent_id = r.agent_id AND other.assigned_at <= r.created_at
      AND (other.assigned_at, other.plan_id, other.package_id) > (pp.assigned_at, pp.plan_id, pp.package_id))";

/// Whether a `merged` or `conflicted` integration is superseded (see the module doc): one statement, one row.
fn integration_superseded(core: &Core, integration_id: &str, sequence: i64) -> KernelResult<bool> {
    let kernel = core.kernel();
    let sql = format!(
        "WITH mine(plan_id) AS (
           SELECT DISTINCT pp.plan_id FROM integration_reports ir
             JOIN agent_reports r ON r.project_id = ir.project_id AND r.report_id = ir.report_id
             JOIN plan_packages pp ON pp.project_id = ir.project_id AND {PACKAGE_REPORT}
            WHERE ir.project_id = ?1 AND ir.integration_id = ?2)
         SELECT
           EXISTS (SELECT 1 FROM integrations j
                    WHERE j.project_id = ?1 AND j.sequence > ?3 AND j.state IN ('merged', 'confirmed')
                      AND EXISTS (SELECT 1 FROM integration_reports jr
                                    JOIN agent_reports r ON r.project_id = jr.project_id AND r.report_id = jr.report_id
                                    JOIN plan_packages pp ON pp.project_id = jr.project_id AND {PACKAGE_REPORT}
                                   WHERE jr.project_id = j.project_id AND jr.integration_id = j.integration_id
                                     AND pp.plan_id IN (SELECT plan_id FROM mine))),
           EXISTS (SELECT 1 FROM mine)
             AND NOT EXISTS (SELECT 1 FROM mine m
                               JOIN plans pl ON pl.project_id = ?1 AND pl.plan_id = m.plan_id
                              WHERE pl.cancelled_at IS NULL
                                AND NOT EXISTS (SELECT 1 FROM plan_signoffs s
                                                 WHERE s.project_id = ?1 AND s.plan_id = m.plan_id AND s.integration_id <> ?2))"
    );
    let found = query_opt(
        &kernel.database,
        &sql,
        rusqlite::params![kernel.project_id, integration_id, sequence],
        |row| Ok(row.get::<_, i64>(0)? == 1 || row.get::<_, i64>(1)? == 1),
    )?;
    Ok(found.unwrap_or(false))
}

/// The most reads one integration's details can cost: five statements and the rows they return (four single rows
/// and ten sign-offs). An item that is not the first of a call waits for this much room.
const DETAILS_RESERVE: usize = 5 + 4 + SIGNOFF_ROWS;
/// The most sign-offs of an item (the `LIMIT` of [`SIGNOFFS`]).
const SIGNOFF_ROWS: usize = 10;
/// The statements of the details of an integration, each through an index (`feed_query_plans` pins their plans).
const DETAILS_ROW: &str =
    "SELECT details_json FROM integration_details WHERE project_id = ?1 AND integration_id = ?2";
const FINISHED_REVIEW: &str = "SELECT state, reviewer_agent_id, round FROM reviews
          WHERE project_id = ?1 AND subject_integration_id = ?2 AND state IN ('passed', 'findings', 'failed', 'cancelled')
          ORDER BY round DESC LIMIT 1";
const LATEST_RESULT: &str = "SELECT state, commit_sha, finished_at FROM verify_runs
          WHERE project_id = ?1 AND subject_integration_id = ?2 AND state IN ('passed', 'failed', 'timeout', 'error')
          ORDER BY sequence DESC LIMIT 1";
const LAST_PASSED: &str = "SELECT state, commit_sha, finished_at FROM verify_runs
          WHERE project_id = ?1 AND subject_integration_id = ?2 AND state = 'passed'
          ORDER BY sequence DESC LIMIT 1";
const SIGNOFFS: &str = "SELECT plan_id, summary, created_at,
                    (SELECT COUNT(*) FROM plan_signoffs WHERE project_id = ?1 AND integration_id = ?2)
               FROM plan_signoffs WHERE project_id = ?1 AND integration_id = ?2
              ORDER BY created_at LIMIT 10";
/// The most serialized bytes of `integrationDetails`: the stored text (16 KiB at most) plus the flags and the three
/// lookups. Entries are dropped from the end of the longest lists (counted as omitted) until it fits.
pub const DETAILS_WIRE_MAX: usize = 17 * 1024;
/// The most bytes of one text of the details.
const DETAILS_TEXT_CAP: usize = 300;

/// A text of the details with its flags. The stored texts sit in write-once rows, so none is re-hashed: `textVerified`
/// is true for every one of them.
fn details_text(mut members: Vec<(&str, String)>, was_cut: bool) -> Map<String, Value> {
    let mut out = Map::new();
    let mut cut = was_cut;
    for (key, value) in members.drain(..) {
        let cleaned = clean_text(&value);
        let (text, over) = cap_text(&cleaned, DETAILS_TEXT_CAP);
        cut |= over;
        out.insert(key.into(), json!(text));
    }
    out.insert("textVerified".into(), json!(true));
    out.insert("truncated".into(), json!(cut));
    out
}

fn field<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn list<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn number(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn flagged(value: &Value) -> bool {
    value.get("truncated").and_then(Value::as_bool) == Some(true)
}

/// Drops the last entry of `holder[list]` and counts it in `holder[omitted]`; false when the list is empty.
fn drop_last(holder: &mut Value, list: &str, omitted: &str) -> bool {
    let Some(items) = holder.get_mut(list).and_then(Value::as_array_mut) else {
        return false;
    };
    if items.pop().is_none() {
        return false;
    }
    holder[omitted] = json!(number(holder, omitted) + 1);
    true
}

fn fit_details(value: &mut Value) {
    while serde_json::to_string(value).map_or(0, |text| text.len()) > DETAILS_WIRE_MAX {
        let dropped = drop_last(value, "reports", "reportsOmitted")
            || value.get_mut("impact").is_some_and(|impact| {
                drop_last(impact, "migrations", "migrationsOmitted")
                    || drop_last(impact, "areas", "areasOmitted")
            })
            || drop_last(value, "signoffs", "signoffsOmitted")
            || drop_last(value, "plans", "plansOmitted");
        if !dropped {
            break;
        }
    }
}

/// The wire form of the stored details of an integration, without the results.
fn wire_stored(stored: &Value) -> Value {
    let plans: Vec<Value> = list(stored, "plans")
        .iter()
        .map(|plan| {
            let members = vec![
                ("planId", field(plan, "planId").to_string()),
                ("title", field(plan, "title").to_string()),
            ];
            Value::Object(details_text(members, flagged(plan)))
        })
        .collect();
    let reports: Vec<Value> = list(stored, "reports")
        .iter()
        .map(|report| {
            let mut entry = details_text(
                vec![
                    ("reportId", field(report, "reportId").to_string()),
                    ("summary", field(report, "summary").to_string()),
                ],
                flagged(report),
            );
            let package = report.get("packageId").and_then(Value::as_str);
            entry.insert("packageId".into(), json!(package));
            Value::Object(entry)
        })
        .collect();
    let impact = match stored.get("impact") {
        Some(impact) if impact.is_object() => {
            let areas: Vec<Value> = list(impact, "areas")
                .iter()
                .map(|area| {
                    let mut entry = details_text(
                        vec![("path", field(area, "path").to_string())],
                        flagged(area),
                    );
                    entry.insert("files".into(), json!(number(area, "files")));
                    Value::Object(entry)
                })
                .collect();
            // The stored text says only that some migration path was cut: a path at the cap is the one that was.
            let some_cut = impact.get("migrationsTruncated").and_then(Value::as_bool) == Some(true);
            let migrations: Vec<Value> = list(impact, "migrations")
                .iter()
                .map(|path| {
                    let path = path.as_str().unwrap_or_default();
                    let at_cap = path.len() > DETAILS_TEXT_CAP - 4;
                    Value::Object(details_text(
                        vec![("path", path.to_string())],
                        some_cut && at_cap,
                    ))
                })
                .collect();
            let steps: Vec<Value> = list(impact, "nextSteps")
                .iter()
                .map(|step| {
                    let text = step.as_str().unwrap_or_default().to_string();
                    Value::Object(details_text(vec![("text", text)], false))
                })
                .collect();
            json!({
                "filesChanged": number(impact, "filesChanged"),
                "insertions": number(impact, "insertions"),
                "deletions": number(impact, "deletions"),
                "truncated": flagged(impact),
                "areas": areas,
                "areasOmitted": number(impact, "areasOmitted"),
                "migrations": migrations,
                "migrationsOmitted": number(impact, "migrationsOmitted"),
                "nextSteps": steps,
            })
        }
        _ => Value::Null,
    };
    json!({
        "recorded": true,
        "impactError": stored.get("impactError").filter(|e| e.is_string()).cloned().unwrap_or(Value::Null),
        "plans": plans,
        "plansOmitted": number(stored, "plansOmitted"),
        "reports": reports,
        "reportsOmitted": number(stored, "reportsOmitted"),
        "impact": impact,
    })
}

/// The `integrationDetails` of a waiting integration, and the reads it made charged to `reads`. The members are listed in
/// `docs/remote-protocol.md`. One statement and its row read the stored details (primary key); the finished review with
/// the greatest round, the latest finished verify run, the last passed verify run (each one statement and at most one
/// row, all backwards over an index) and the sign-offs (one statement, at most [`SIGNOFF_ROWS`] rows) complete them, so
/// an item costs 5 statements plus 0 to 14 rows. An integration with no stored row reads that one statement.
fn integration_details(
    core: &Core,
    integration_id: &str,
    reads: &mut Reads,
) -> KernelResult<Value> {
    let before = actual_reads();
    let found = read_details(core, integration_id);
    reads.used += actual_reads().saturating_sub(before);
    found
}

fn read_details(core: &Core, integration_id: &str) -> KernelResult<Value> {
    let kernel = core.kernel();
    let stored = query_opt(
        &kernel.database,
        DETAILS_ROW,
        rusqlite::params![kernel.project_id, integration_id],
        |row| row.get::<_, String>(0),
    )?;
    let Some(stored) = stored else {
        return Ok(json!({"recorded": false}));
    };
    let stored: Value = serde_json::from_str(&stored).unwrap_or(Value::Null);
    let mut details = wire_stored(&stored);

    details["review"] = query_opt(
        &kernel.database,
        FINISHED_REVIEW,
        rusqlite::params![kernel.project_id, integration_id],
        |row| {
            Ok(json!({
                "verdict": row.get::<_, String>(0)?,
                "reviewerAgentId": row.get::<_, String>(1)?,
                "round": row.get::<_, i64>(2)?,
            }))
        },
    )?
    .unwrap_or(Value::Null);

    let run = |sql: &str| {
        query_opt(
            &kernel.database,
            sql,
            rusqlite::params![kernel.project_id, integration_id],
            |row| {
                Ok(json!({
                    "state": row.get::<_, String>(0)?,
                    "commitSha": row.get::<_, String>(1)?,
                    "finishedAt": row.get::<_, Option<String>>(2)?,
                }))
            },
        )
    };
    let latest = run(LATEST_RESULT)?;
    let last_passed = run(LAST_PASSED)?.map(|mut passed| {
        passed.as_object_mut().map(|map| map.remove("state"));
        passed
    });
    details["verify"] = match latest {
        Some(latest) => json!({"latest": latest, "lastPassed": last_passed}),
        None => Value::Null,
    };

    let signoffs = rows(
        core,
        SIGNOFFS,
        vec![project(core), sql_text(integration_id)],
        &["plan_id", "summary", "created_at", "total"],
    )?;
    let total = signoffs.first().map_or(0, |row| int(row, "total"));
    details["signoffsOmitted"] = json!((total as usize).saturating_sub(signoffs.len()));
    details["signoffs"] = Value::Array(
        signoffs
            .iter()
            .map(|row| {
                let mut entry = details_text(
                    vec![
                        ("planId", text(row, "plan_id")),
                        ("summary", text(row, "summary")),
                    ],
                    false,
                );
                entry.insert("createdAt".into(), json!(text(row, "created_at")));
                Value::Object(entry)
            })
            .collect(),
    );
    fit_details(&mut details);
    Ok(details)
}

/// What building a candidate gave.
enum Built {
    /// The item, or `None` for a candidate that does not wait.
    Done(Option<Box<WaitingItem>>),
    /// The lookups it needs do not fit in the reads left: the candidate is not examined and a later call starts at it.
    Defer,
}

enum Designed {
    Skip,
    Defer,
    Item(String, String, String),
}

/// `(since, summary, hash)` of a design package that has a report with no decision on it.
fn build_design(
    core: &Core,
    candidate: &Candidate,
    scan: &mut Scan,
    reads: &mut Reads,
    force: bool,
) -> KernelResult<Designed> {
    let kernel = core.kernel();
    let cols = &candidate.cols;
    let (plan_id, package_id) = (text(cols, "planId"), text(cols, "packageId"));
    // The most it can take: the plan body (once per plan), the report, the decision and the report row, each a statement
    // and a row. They are reserved before the first one runs; a candidate that does not fit waits for the next call,
    // unless it is the first of the call (a call always examines one).
    let need = 2 * usize::from(!scan.bodies.contains_key(&plan_id)) + 6;
    if !force && reads.used + need > reads.max {
        return Ok(Designed::Defer);
    }
    if !scan.bodies.contains_key(&plan_id) {
        let body = query_opt(
            &kernel.database,
            "SELECT body_json FROM plan_revisions WHERE project_id = ? AND plan_id = ? AND revision = ?",
            rusqlite::params![kernel.project_id, plan_id, int(cols, "revision")],
            |row| row.get::<_, String>(0),
        )?;
        reads.looked_up(&body);
        scan.bodies.insert(plan_id.clone(), body);
    }
    let is_design = scan.bodies[&plan_id]
        .as_deref()
        .is_some_and(|body| crate::plan_body::package_is_design(body, &package_id));
    if !is_design {
        return Ok(Designed::Skip);
    }
    let assignee = text(cols, "assignee");
    let assigned_at = text(cols, "assignedAt");
    let report = counted(plan_packages::package_report(
        kernel,
        &plan_id,
        &package_id,
        Some(&assignee),
        Some(&assigned_at),
    ))?;
    reads.looked_up(&report);
    let Some(report_id) = report else {
        return Ok(Designed::Skip);
    };
    let decision = counted(design_approvals::current_decision(
        kernel,
        &plan_id,
        &package_id,
    ))?;
    reads.looked_up(&decision);
    if decision.as_ref().is_some_and(|d| d.report_id == report_id) {
        return Ok(Designed::Skip);
    }
    let row = query_opt(
        &kernel.database,
        "SELECT summary, commit_sha, created_at FROM agent_reports WHERE project_id = ? AND report_id = ?",
        [kernel.project_id.as_str(), report_id.as_str()],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)),
    )?;
    reads.looked_up(&row);
    let Some((summary, commit, created_at)) = row else {
        return Ok(Designed::Skip);
    };
    let link = decision.as_ref().and_then(|d| d.link.as_deref());
    Ok(Designed::Item(
        created_at,
        shown(&summary),
        sha256(&design_hash_input(&report_id, &commit, link)),
    ))
}

/// Whether a candidate comes after `(kind index, sort)` in the kinds a scan covers.
fn more_after(
    core: &Core,
    kinds: &[&str],
    filter: Option<usize>,
    last: &(usize, Sort),
    reads: &mut Reads,
) -> KernelResult<bool> {
    // A probe is a statement and, when something follows, its row.
    let mut probe = |kind: &str, sort: Option<Sort>| -> KernelResult<bool> {
        let found = !fetch(core, kind, &Sel::After(sort), 1)?.is_empty();
        reads.used += 1 + usize::from(found);
        Ok(found)
    };
    if probe(kinds[last.0], Some(last.1.clone()))? {
        return Ok(true);
    }
    for (index, kind) in kinds.iter().enumerate().skip(last.0 + 1) {
        if filter.is_some_and(|f| f != index) {
            continue;
        }
        if probe(kind, None)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn parse_waiting_key(key: &str) -> KernelResult<(usize, Sort)> {
    let bad = || invalid(format!("{key} is not a waiting key"));
    let (kind, id) = parse_item_key("waiting", key).ok_or_else(bad)?;
    let index = waiting_kinds()
        .iter()
        .position(|k| *k == kind)
        .ok_or_else(bad)?;
    let sort = if kind == "design" {
        let (sequence, package) = id.split_once('/').ok_or_else(bad)?;
        (key_number(sequence).ok_or_else(bad)?, package.to_string())
    } else {
        (key_number(&id).ok_or_else(bad)?, String::new())
    };
    Ok((index, sort))
}

/// The waiting items after the item key `after`, optionally of one kind (`op`, `prompt`, `design`, `message` or
/// `integration`).
pub fn waiting_page(
    core: &Core,
    after: Option<&str>,
    limit: u32,
    kind: Option<&str>,
) -> KernelResult<Page<WaitingItem>> {
    Ok(waiting_page_counted(core, after, limit, kind, MAX_READS)?.0)
}

/// `waiting_page` with another read budget; also returns the reads made (tests).
#[doc(hidden)]
pub fn waiting_page_counted(
    core: &Core,
    after: Option<&str>,
    limit: u32,
    kind: Option<&str>,
    max_reads: usize,
) -> KernelResult<(Page<WaitingItem>, usize)> {
    let limit = check_limit(limit)?;
    let kinds = waiting_kinds();
    let filter = match kind {
        None => None,
        Some(name) => Some(
            kinds
                .iter()
                .position(|k| *k == name)
                .ok_or_else(|| invalid(format!("unknown kind {name}")))?,
        ),
    };
    let start = match after {
        None => None,
        Some(key) => Some(parse_waiting_key(key)?),
    };
    let head = head_of(core)?;
    // One probe for the rest of the current kind and one for each later kind (two each) are kept back for the end of
    // the call. The head has been read.
    let mut reads = Reads {
        used: 2,
        max: max_reads.saturating_sub(2 * (kinds.len() + 1)).max(3),
    };
    let mut scan = Scan {
        bodies: HashMap::new(),
        details: true,
    };
    let mut items: Vec<WaitingItem> = Vec::new();
    let mut bytes = 0usize;
    let mut last: Option<(usize, Sort)> = None;
    let mut stopped = false;
    'kinds: for (index, name) in kinds.iter().enumerate() {
        if filter.is_some_and(|f| f != index) || start.as_ref().is_some_and(|s| index < s.0) {
            continue;
        }
        let mut cursor = match &start {
            Some((first, sort)) if *first == index => Some(sort.clone()),
            _ => None,
        };
        loop {
            // A fetch is a statement and a row for each it returns, and asks for no more than the budget holds. A call
            // that has examined nothing still asks for one.
            let room = reads.room();
            if room < 2 && (!items.is_empty() || last.is_some()) {
                stopped = true;
                break 'kinds;
            }
            let ask = room.saturating_sub(1).clamp(1, CHUNK);
            let chunk = fetch(core, name, &Sel::After(cursor.clone()), ask)?;
            let count = chunk.len();
            reads.used += 1 + count;
            for candidate in chunk {
                cursor = Some(candidate.sort.clone());
                let force = items.is_empty() && last.is_none();
                let built = match build(core, name, &candidate, &mut scan, &mut reads, force)? {
                    Built::Defer => {
                        stopped = true;
                        break 'kinds;
                    }
                    Built::Done(built) => built.map(|item| *item),
                };
                if let Some(item) = &built {
                    let size = serde_json::to_string(item)?.len() + 1;
                    if !items.is_empty() && bytes + size > MAX_PAGE_BYTES {
                        stopped = true;
                        break 'kinds;
                    }
                    bytes += size;
                }
                last = Some((index, candidate.sort.clone()));
                if let Some(item) = built {
                    items.push(item);
                    if items.len() == limit {
                        stopped = true;
                        break 'kinds;
                    }
                }
            }
            if count < ask {
                break;
            }
        }
    }
    let next = match (&last, stopped) {
        (Some(last), true) if more_after(core, &kinds, filter, last, &mut reads)? => {
            Some(item_key_of(&kinds, last))
        }
        _ => None,
    };
    Ok((Page { head, items, next }, reads.used))
}

fn item_key_of(kinds: &[&str], last: &(usize, Sort)) -> String {
    let kind = kinds[last.0];
    if kind == "design" {
        item_key("waiting", kind, &format!("{}/{}", last.1 .0, last.1 .1))
    } else {
        item_key("waiting", kind, &last.1 .0.to_string())
    }
}

/// The one waiting item selected by `sel` of `kind`, or `None` when the row does not wait.
fn waiting_one(
    core: &Core,
    kind: &str,
    sel: Sel,
    details: bool,
) -> KernelResult<Option<WaitingItem>> {
    let mut reads = Reads {
        used: 0,
        max: MAX_READS,
    };
    let mut scan = Scan {
        bodies: HashMap::new(),
        details,
    };
    match fetch(core, kind, &sel, 1)?.into_iter().next() {
        Some(candidate) => match build(core, kind, &candidate, &mut scan, &mut reads, true)? {
            Built::Done(item) => Ok(item.map(|item| *item)),
            Built::Defer => Ok(None),
        },
        None => Ok(None),
    }
}

// --------------------------------------------------------------------------------------------------- the messages

/// A message from an agent to the user, as the `messages` section lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageItem {
    /// The sequence number of the message, in decimal.
    pub key: String,
    pub message_id: String,
    pub sequence: i64,
    pub sender_agent_id: String,
    /// `action` (asks the user) or `info`.
    pub class: String,
    /// `open` (an action not answered yet), `answered` or `info`.
    pub state: String,
    pub body: String,
    pub truncated: bool,
    pub created_at: String,
    pub answered_at: Option<String>,
    /// `message:<messageId>`: the request id of the waiting item while the message is open.
    pub request_id: String,
}

const MESSAGE_COLUMNS: &[&str] = &[
    "sequence",
    "message_id",
    "sender_agent_id",
    "class",
    "state",
    "body",
    "created_at",
    "answered_at",
];

/// Messages to the user among the latest [`user_messages::LISTED_MESSAGES`]: the one of sequence `only`, or those after
/// `after`, oldest first.
fn message_rows(
    core: &Core,
    only: Option<i64>,
    after: i64,
    limit: usize,
) -> KernelResult<Vec<MessageItem>> {
    let select = MESSAGE_COLUMNS.join(", ");
    let floor = "sequence > (SELECT COALESCE(MAX(sequence), 0) FROM user_messages WHERE project_id = ?1) - ?2";
    let (sql, params) = match only {
        Some(sequence) => (
            format!("SELECT {select} FROM user_messages WHERE project_id = ?1 AND {floor} AND sequence = ?3"),
            vec![project(core), sql_int(user_messages::LISTED_MESSAGES), sql_int(sequence)],
        ),
        None => (
            format!(
                "SELECT {select} FROM user_messages WHERE project_id = ?1 AND {floor} AND sequence > ?3
                 ORDER BY sequence LIMIT ?4"
            ),
            vec![
                project(core),
                sql_int(user_messages::LISTED_MESSAGES),
                sql_int(after),
                sql_int(limit as i64),
            ],
        ),
    };
    Ok(rows(core, &sql, params, MESSAGE_COLUMNS)?
        .into_iter()
        .map(|cols| {
            let cleaned = clean_text(&text(&cols, "body"));
            let (body, truncated) = cap_text(&cleaned, TEXT_CAP);
            let sequence = int(&cols, "sequence");
            MessageItem {
                key: item_key("messages", "message", &sequence.to_string()),
                message_id: text(&cols, "message_id"),
                sequence,
                sender_agent_id: text(&cols, "sender_agent_id"),
                class: text(&cols, "class"),
                state: text(&cols, "state"),
                body: body.to_string(),
                truncated,
                created_at: text(&cols, "created_at"),
                answered_at: cols.get("answered_at").and_then(Value::as_str).map(str::to_string),
                request_id: format!("{MESSAGE_KIND}:{}", text(&cols, "message_id")),
            }
        })
        .collect())
}

/// The messages to the user after the key `after` (a sequence), oldest first, among the latest
/// [`user_messages::LISTED_MESSAGES`].
pub fn messages_page(core: &Core, after: Option<&str>, limit: u32) -> KernelResult<Page<MessageItem>> {
    let limit = check_limit(limit)?;
    let after = match after {
        None => 0,
        Some(key) => parse_item_key("messages", key)
            .and_then(|(_, id)| key_number(&id))
            .ok_or_else(|| invalid("after is not a message key"))?,
    };
    let head = head_of(core)?;
    // One more row than asked tells whether there is a next page.
    let mut items = message_rows(core, None, after, limit + 1)?;
    let more = items.len() > limit;
    items.truncate(limit);
    let next = if more { items.last().map(|item| item.key.clone()) } else { None };
    Ok(Page { head, items, next })
}

// ------------------------------------------------------------------------------------------------------ single reads

/// One item of a section by its key, as the page of that section lists it; `None` when the row is gone or no longer
/// listed (the client removes it). Agents are keyed by agent id.
pub fn snapshot_item(core: &Core, section: &str, key: &str) -> KernelResult<Option<Value>> {
    match section {
        "waiting" => {
            let (index, sort) = parse_waiting_key(key)?;
            let kind = waiting_kinds()[index];
            Ok(waiting_one(core, kind, Sel::Key(sort), true)?.map(|item| to_value(&item)))
        }
        "agents" => {
            if parse_item_key("agents", key).is_none() {
                return Err(invalid("not an agent key"));
            }
            Ok(agent_rows(core, AgentSel::One(key), 1)?
                .pop()
                .map(|item| to_value(&item)))
        }
        "run" => {
            if parse_item_key("run", key).is_none() {
                return Err(invalid("not the run key"));
            }
            Ok(Some(to_value(&run_item(core)?)))
        }
        "plans" => {
            let sequence = parse_item_key("plans", key)
                .and_then(|(_, id)| key_number(&id))
                .ok_or_else(|| invalid("not a plan key"))?;
            let found = plan_rows(core, 0, Some(sequence), 1)?;
            match found.first() {
                Some(row) => Ok(Some(to_value(&plan_item(core, row)?))),
                None => Ok(None),
            }
        }
        "messages" => {
            let sequence = parse_item_key("messages", key)
                .and_then(|(_, id)| key_number(&id))
                .ok_or_else(|| invalid("not a message key"))?;
            Ok(message_rows(core, Some(sequence), 0, 1)?
                .pop()
                .map(|item| to_value(&item)))
        }
        other => Err(invalid(format!("unknown section {other}"))),
    }
}

fn to_value<T: Serialize>(item: &T) -> Value {
    serde_json::to_value(item).expect("an item serializes")
}

/// The content hash of what a request id names, or `None` once it is not waiting. The forms are `op:<proposalId>`,
/// `prompt:<relayId>`, `design:<planId>/<packageId>`, `message:<messageId>`, `integration:<integrationId>`,
/// `agent:<agentId>` and `run`.
pub fn content_hash(core: &Core, request_id: &str) -> KernelResult<Option<String>> {
    Ok(content_hash_counted(core, request_id)?.0)
}

/// `content_hash` that also returns the lookups it made (tests).
#[doc(hidden)]
pub fn content_hash_counted(
    core: &Core,
    request_id: &str,
) -> KernelResult<(Option<String>, usize)> {
    if request_id == "run" {
        return Ok((Some(run_item(core)?.content_hash), 2));
    }
    let (kind, id) = request_id
        .split_once(':')
        .filter(|(_, id)| !id.is_empty())
        .ok_or_else(|| invalid(format!("{request_id} is not a request id")))?;
    if kind == "agent" {
        let item = agent_rows(core, AgentSel::One(id), 1)?.pop();
        return Ok((item.map(|item| item.content_hash), 2));
    }
    if !waiting_kinds().contains(&kind) {
        return Err(invalid(format!("{request_id} is not a request id")));
    }
    let item = waiting_one(core, kind, Sel::Id(id.to_string()), false)?;
    Ok((item.map(|item| item.content_hash), 1))
}
