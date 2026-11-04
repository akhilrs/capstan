//! The push event source: the ledger rows that should reach the user as a notification, read in order after a cursor.
//!
//! Read-only. A caller runs [`push_events_after`] inside the kernel's lock, sends what it returns, and moves its cursor to
//! [`PushPage::scanned`] (never to the sequence of an event). The events carry ids and counts only: no message body, prompt,
//! command, reason or report summary is ever read into one.
//!
//! The cost of a call is bounded: one range read of the ledger plus at most [`MAX_LOOKUPS`] lookups (an entity row or a plan
//! body). A row is handled whole or not at all; when the next row does not fit in what is left, the page ends and `scanned`
//! is the row before it. The cheapest row costs 0 and the dearest 2, so a call always makes progress.

use super::design_approvals::approved_body;
use super::event_feed::{self, map_row, RawRow, Row};
use super::feed_cursor;
use crate::core::Core;
use crate::errors::KernelResult;
use crate::helpers::{query_all, query_opt};
use crate::plan_body::package_is_design;
use capstan_config::NtfyEvent;
use serde_json::Value;
use std::collections::HashMap;

/// The most rows one call reads.
pub const MAX_PUSH_ROWS: usize = 200;
/// The most lookups (entity rows and plan bodies) one call makes.
pub const MAX_LOOKUPS: u32 = 50;

/// One thing to tell the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushEvent {
    /// The ledger sequence of the row it comes from.
    pub seq: u64,
    /// The time of that row.
    pub ts: String,
    pub event: NtfyEvent,
    /// What the event is about: a message sequence, a proposal sequence, a plan, report, run or integration id, or an agent.
    pub subject: String,
    pub agent_id: Option<String>,
    pub plan_id: Option<String>,
    pub detail: Option<String>,
}

/// The events of a range of rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushPage {
    pub events: Vec<PushEvent>,
    /// The sequence of the last row handled (the head when the caller was already at or past it): the caller's next cursor.
    pub scanned: u64,
    /// The reads made (the range read and each lookup); a diagnostic, like the feed's `rowsRead`.
    pub queries: u32,
}

/// The sequence of the latest ledger row; 0 for an empty ledger.
pub fn push_head(core: &Core) -> KernelResult<u64> {
    Ok(feed_cursor::head(core)?.max(0) as u64)
}

/// An id is kept only when it is 1 to 64 characters of letters, digits and `._:-`; any other is shown as `?`.
fn id(raw: &str) -> String {
    let ok = !raw.is_empty()
        && raw.len() <= 64
        && raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'));
    if ok {
        raw.to_string()
    } else {
        "?".to_string()
    }
}

fn id_of(value: &Value) -> String {
    id(value.as_str().unwrap_or(""))
}

struct Ctx<'a> {
    core: &'a Core,
    lookups: u32,
    queries: u32,
    /// The approved plan bodies already read in this call.
    bodies: HashMap<String, Option<String>>,
}

enum Outcome {
    Events(Vec<PushEvent>),
    /// The row does not fit in the lookups left.
    Defer,
}

impl Ctx<'_> {
    fn spend(&mut self) -> bool {
        if self.lookups >= MAX_LOOKUPS {
            return false;
        }
        self.lookups += 1;
        self.queries += 1;
        true
    }
}

fn event(row: &Row, event: NtfyEvent, subject: String) -> PushEvent {
    PushEvent {
        seq: row.seq.max(0) as u64,
        ts: row.created_at.clone(),
        event,
        subject,
        agent_id: None,
        plan_id: None,
        detail: None,
    }
}

fn agent_event(row: &Row, kind: NtfyEvent, agent: String) -> PushEvent {
    PushEvent {
        agent_id: Some(agent.clone()),
        ..event(row, kind, agent)
    }
}

/// How many lookups the row needs at least; the plan body of a report is charged when it turns out to be needed.
fn base_cost(row: &Row) -> u32 {
    match (row.action.as_str(), row.to.as_deref()) {
        ("operator.propose", _) | ("prompt_relay.capture", _) => 1,
        ("report.record", Some("accepted")) => 1,
        ("verify.finish", Some("failed" | "error")) => 1,
        _ => 0,
    }
}

fn handle(ctx: &mut Ctx<'_>, row: &Row) -> KernelResult<Outcome> {
    if map_row(
        &row.entity_type,
        &row.action,
        row.from.as_deref(),
        row.to.as_deref(),
    )
    .is_none()
    {
        return Ok(Outcome::Events(Vec::new()));
    }
    let kernel = ctx.core.kernel();
    let project = kernel.project_id.as_str();
    let mut events = Vec::new();
    match (row.action.as_str(), row.from.as_deref(), row.to.as_deref()) {
        ("user_message.send", _, _) => {
            if row.details["class"].as_str() == Some("action") {
                if let Some(sequence) = row.details["sequence"].as_i64() {
                    let mut e = event(row, NtfyEvent::UserQuestion, sequence.to_string());
                    e.agent_id = Some(id_of(&row.details["senderAgentId"]));
                    events.push(e);
                }
            }
        }
        ("operator.propose", _, _) => {
            if !ctx.spend() {
                return Ok(Outcome::Defer);
            }
            let found = query_opt(
                &kernel.database,
                "SELECT sequence, proposer_agent_id FROM operator_proposals WHERE project_id = ? AND proposal_id = ?",
                rusqlite::params![project, row.entity_id],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)),
            )?;
            if let Some((sequence, proposer)) = found {
                let mut e = event(row, NtfyEvent::OperatorProposal, sequence.to_string());
                e.agent_id = proposer.as_deref().map(id);
                events.push(e);
            }
        }
        ("prompt_relay.capture", None, Some("captured")) => {
            if !ctx.spend() {
                return Ok(Outcome::Defer);
            }
            let agent = query_opt(
                &kernel.database,
                "SELECT agent_id FROM prompt_relays WHERE project_id = ? AND relay_id = ?",
                rusqlite::params![project, row.entity_id],
                |r| r.get::<_, String>(0),
            )?;
            if let Some(agent) = agent {
                events.push(agent_event(row, NtfyEvent::PromptWaiting, id(&agent)));
            }
        }
        ("report.record", _, Some("accepted")) => {
            if !ctx.spend() {
                return Ok(Outcome::Defer);
            }
            // The reporter and the package it holds now in an approved plan, in one read.
            let found = query_opt(
                &kernel.database,
                "SELECT r.agent_id, p.plan_id, p.package_id
                   FROM agent_reports r
                   LEFT JOIN (plan_packages p JOIN plans pl ON pl.project_id = p.project_id AND pl.plan_id = p.plan_id)
                     ON p.project_id = r.project_id AND p.assignee_agent_id = r.agent_id
                    AND pl.state = 'approved' AND p.cancelled_at IS NULL
                  WHERE r.project_id = ? AND r.report_id = ?
                  ORDER BY p.assigned_at DESC, pl.sequence DESC LIMIT 1",
                rusqlite::params![project, row.entity_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ))
                },
            )?;
            if let Some((agent, Some(plan), Some(package))) = found {
                if !ctx.bodies.contains_key(&plan) {
                    if !ctx.spend() {
                        return Ok(Outcome::Defer);
                    }
                    let body = approved_body(ctx.core.kernel(), &plan)?;
                    ctx.bodies.insert(plan.clone(), body);
                }
                let design = ctx.bodies[&plan]
                    .as_deref()
                    .is_some_and(|body| package_is_design(body, &package));
                if design {
                    let mut e = event(row, NtfyEvent::DesignWaiting, id(&row.entity_id));
                    e.agent_id = Some(id(&agent));
                    e.plan_id = Some(id(&plan));
                    events.push(e);
                }
            }
        }
        ("plan.signoff", _, _) => {
            let mut e = event(row, NtfyEvent::PlanSignedOff, id(&row.entity_id));
            e.plan_id = Some(id(&row.entity_id));
            e.detail = row.details["integrationId"].as_str().map(id);
            events.push(e);
        }
        ("pm.attention", _, _) => {
            if event_feed::queued(row) {
                for episode in event_feed::attention_episodes(&row.input) {
                    let kind = match episode["kind"].as_str() {
                        Some("stalled") => NtfyEvent::AgentStalled,
                        Some("blocked") => NtfyEvent::AgentBlocked,
                        _ => continue,
                    };
                    events.push(agent_event(row, kind, id_of(&episode["agentId"])));
                }
            }
        }
        ("pm.input_blocked", _, _) => {
            if event_feed::queued(row) {
                events.push(agent_event(
                    row,
                    NtfyEvent::AgentBlocked,
                    id_of(&row.input["agentId"]),
                ));
            }
        }
        ("agent.lost", Some("active"), Some("lost")) => {
            events.push(agent_event(row, NtfyEvent::AgentLost, id(&row.entity_id)));
        }
        ("verify.finish", Some("running"), Some("failed" | "error")) => {
            if !ctx.spend() {
                return Ok(Outcome::Defer);
            }
            let subject = query_opt(
                &kernel.database,
                "SELECT subject_integration_id, subject_report_id FROM verify_runs WHERE project_id = ? AND run_id = ?",
                rusqlite::params![project, row.entity_id],
                |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?,
                        r.get::<_, Option<String>>(1)?,
                    ))
                },
            )?;
            let mut e = event(row, NtfyEvent::VerifyFailed, id(&row.entity_id));
            e.detail = match subject {
                Some((Some(integration), _)) => Some(format!("integration {}", id(&integration))),
                Some((None, Some(report))) => Some(format!("report {}", id(&report))),
                _ => None,
            };
            events.push(e);
        }
        ("integration.finish", Some("running"), Some("conflicted")) => {
            let listed = row.details["files"].as_array().map_or(0, Vec::len) as i64;
            let omitted = row.details["omitted"].as_i64().unwrap_or(0).max(0);
            let mut e = event(row, NtfyEvent::IntegrationConflicted, id(&row.entity_id));
            e.detail = Some((listed + omitted).to_string());
            events.push(e);
        }
        _ => {}
    }
    Ok(Outcome::Events(events))
}

/// The events of the ledger rows after `after`, at most `max_rows` rows (capped at [`MAX_PUSH_ROWS`]) and
/// [`MAX_LOOKUPS`] lookups. Read-only. Any error ends the call with no page: the caller's cursor stays where it was.
pub fn push_events_after(core: &Core, after: u64, max_rows: usize) -> KernelResult<PushPage> {
    let limit = max_rows.clamp(1, MAX_PUSH_ROWS);
    let kernel = core.kernel();
    let raw = query_all(
        &kernel.database,
        "SELECT sequence, entity_type, entity_id, from_state, to_state, actor_id, payload_json, created_at
           FROM controller_events WHERE project_id = ? AND sequence > ? ORDER BY sequence LIMIT ?",
        rusqlite::params![
            kernel.project_id,
            i64::try_from(after).unwrap_or(i64::MAX),
            limit as i64
        ],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                RawRow {
                    entity_type: r.get(1)?,
                    entity_id: r.get(2)?,
                    from_state: r.get(3)?,
                    to_state: r.get(4)?,
                    actor_id: r.get(5)?,
                    payload_json: r.get(6)?,
                    created_at: r.get(7)?,
                },
            ))
        },
    )?;
    let mut ctx = Ctx {
        core,
        lookups: 0,
        queries: 1,
        bodies: HashMap::new(),
    };
    if raw.is_empty() {
        ctx.queries += 1;
        return Ok(PushPage {
            events: Vec::new(),
            scanned: push_head(core)?,
            queries: ctx.queries,
        });
    }
    let mut events = Vec::new();
    let mut scanned = after;
    for (seq, record) in raw {
        let row = event_feed::row_of(seq, record);
        if ctx.lookups + base_cost(&row) > MAX_LOOKUPS {
            break;
        }
        match handle(&mut ctx, &row)? {
            Outcome::Events(found) => events.extend(found),
            Outcome::Defer => break,
        }
        scanned = seq.max(0) as u64;
    }
    Ok(PushPage {
        events,
        scanned,
        queries: ctx.queries,
    })
}
