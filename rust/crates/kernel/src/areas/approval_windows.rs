//! The approval windows (migration 0047): a time box in which the controller approves safe Operator proposals and answers
//! worker prompts that ask once. One window is open at a time; it ends only at its cap, on revoke, replacement or restart.
//! Releasing the Operator agent ends the Operator part of the window and nothing else.

use crate::auth::AuthenticatedActor;
use crate::dispatch::{arg, call, Args};
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{
    execute, query_all, query_opt, safe_id, window_ended_notice, window_opened_notice,
    window_operator_ended_notice,
};
use crate::kernel::Kernel;
use crate::records::{MutationEvent, MutationOutput};
use crate::types::MutationContext;
use capstan_ledger::iso_from_millis;
use rusqlite::Row;
use serde_json::{json, Value};

use super::message_notices;
use super::messaging::parse_iso_ms;

/// The hard cap of a window, in minutes.
pub(crate) const WINDOW_HARD_MAX_MINUTES: i64 = 480;

struct WindowRow {
    window_id: String,
    kind: String,
    minutes: i64,
    source_proposal_id: Option<String>,
    source_relay_id: Option<String>,
    created_at: String,
    expires_at: String,
    operator_ended_at: Option<String>,
    ended_at: Option<String>,
}

impl WindowRow {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            window_id: row.get("window_id")?,
            kind: row.get("kind")?,
            minutes: row.get("minutes")?,
            source_proposal_id: row.get("source_proposal_id")?,
            source_relay_id: row.get("source_relay_id")?,
            created_at: row.get("created_at")?,
            expires_at: row.get("expires_at")?,
            operator_ended_at: row.get("operator_ended_at")?,
            ended_at: row.get("ended_at")?,
        })
    }

    fn due(&self, now_ms: i64) -> bool {
        parse_iso_ms(&self.expires_at) <= now_ms
    }

    fn record(&self, now_ms: i64) -> Value {
        let source = match (&self.source_proposal_id, &self.source_relay_id) {
            (Some(proposal), _) => json!({"proposalId": proposal}),
            (_, relay) => json!({"relayId": relay}),
        };
        let remaining_ms = (parse_iso_ms(&self.expires_at) - now_ms).max(0);
        json!({
            "windowId": self.window_id,
            "kind": self.kind,
            "minutes": self.minutes,
            "operatorActive": self.ended_at.is_none() && self.operator_ended_at.is_none(),
            "source": source,
            "createdAt": self.created_at,
            "expiresAt": self.expires_at,
            "remainingSeconds": (remaining_ms + 999) / 1000,
        })
    }
}

fn window_row(kernel: &Kernel, window_id: &str) -> KernelResult<Option<WindowRow>> {
    query_opt(
        &kernel.database,
        "SELECT * FROM approval_windows WHERE project_id = ? AND window_id = ?",
        [&kernel.project_id, window_id],
        WindowRow::from_row,
    )
}

fn open_row(kernel: &Kernel) -> KernelResult<Option<WindowRow>> {
    query_opt(
        &kernel.database,
        "SELECT * FROM approval_windows WHERE project_id = ? AND ended_at IS NULL",
        [&kernel.project_id],
        WindowRow::from_row,
    )
}

/// The window a request may use now: open, inside its cap, and (when `operator`) with the Operator part still open.
pub(crate) fn usable_window(
    kernel: &Kernel,
    window_id: &str,
    operator: bool,
) -> KernelResult<Value> {
    let now = parse_iso_ms(&kernel.now());
    match window_row(kernel, window_id)? {
        Some(row) if row.ended_at.is_none() && !row.due(now) => {
            if operator && row.operator_ended_at.is_some() {
                return Err(KernelError::controller(format!(
                    "window_not_active: the Operator part of window {window_id} has ended"
                )));
            }
            Ok(row.record(now))
        }
        Some(_) => Err(KernelError::controller(format!(
            "window_not_active: window {window_id} has ended"
        ))),
        None => Err(KernelError::controller(format!(
            "window_not_active: window {window_id} does not exist"
        ))),
    }
}

/// `activeApprovalWindow`: the open window, or null; one past its cap is closed.
pub fn active_approval_window(kernel: &Kernel) -> KernelResult<Value> {
    kernel.assert_open()?;
    let now = parse_iso_ms(&kernel.now());
    Ok(match open_row(kernel)? {
        Some(row) if !row.due(now) => row.record(now),
        _ => Value::Null,
    })
}

/// `listApprovalWindows`: newest first, ended ones included, with the way each ended.
pub fn list_approval_windows(kernel: &Kernel, limit: Option<i64>) -> KernelResult<Value> {
    kernel.assert_open()?;
    let now = parse_iso_ms(&kernel.now());
    let rows = query_all(
        &kernel.database,
        "SELECT * FROM approval_windows WHERE project_id = ? ORDER BY sequence DESC LIMIT ?",
        rusqlite::params![kernel.project_id, limit.unwrap_or(20)],
        |row| {
            let window = WindowRow::from_row(row)?;
            let reason: Option<String> = row.get("ended_reason")?;
            Ok((window, reason))
        },
    )?;
    Ok(Value::Array(
        rows.into_iter()
            .map(|(window, reason)| {
                let mut record = window.record(now);
                record["endedAt"] = json!(window.ended_at);
                record["endedReason"] = json!(reason);
                record
            })
            .collect(),
    ))
}

fn end_event(window_id: &str, reason: &str) -> MutationEvent {
    MutationEvent {
        from_state: Some("open".into()),
        to_state: Some("ended".into()),
        details: Some(json!({"event": "operator.window_ended", "reason": reason})),
        ..MutationEvent::new("approval_window", window_id, 0)
    }
}

/// Ends one open window and tells the PM. The caller owns the transaction.
fn end_row(
    kernel: &Kernel,
    row: &WindowRow,
    reason: &str,
    now: &str,
) -> KernelResult<MutationEvent> {
    execute(
        &kernel.database,
        "UPDATE approval_windows SET ended_at = ?, ended_reason = ? WHERE project_id = ? AND window_id = ? AND ended_at IS NULL",
        [now, reason, &kernel.project_id, &row.window_id],
    )?;
    message_notices::notice_to_pm(
        kernel,
        "approval_window",
        &window_ended_notice(&row.window_id, reason),
        now,
        false,
    )?;
    Ok(end_event(&row.window_id, reason))
}

/// How a window was asked for: `windowKind` and the minutes it lasts (`maxMinutes`, already cut to the configured cap).
pub(crate) struct WindowSpec {
    pub kind: String,
    pub minutes: i64,
}

impl WindowSpec {
    pub(crate) fn parse(spec: &Value) -> KernelResult<Self> {
        let kind = match spec.get("windowKind").and_then(Value::as_str) {
            Some(kind @ ("hour" | "session" | "minutes")) => kind.to_string(),
            _ => {
                return Err(KernelError::controller(
                    "window_kind_invalid: a window is hour, session or minutes",
                ))
            }
        };
        let minutes = spec
            .get("maxMinutes")
            .and_then(Value::as_f64)
            .unwrap_or(f64::NAN);
        let limit = if kind == "hour" {
            60
        } else {
            WINDOW_HARD_MAX_MINUTES
        };
        if !(minutes.fract() == 0.0 && (1.0..=limit as f64).contains(&minutes)) {
            return Err(KernelError::controller(format!(
                "window_minutes_invalid: a {kind} window lasts a whole number of minutes from 1 to {limit}"
            )));
        }
        Ok(Self {
            kind,
            minutes: minutes as i64,
        })
    }
}

/// Where a window comes from.
pub(crate) enum WindowSource<'a> {
    Proposal(&'a str),
    Relay(&'a str),
}

/// What the opening notice says besides the window itself; the caller knows the configured floor.
pub(crate) struct OpenNoticeInput {
    pub floor: Option<Vec<String>>,
    pub tool_prompts: bool,
}

impl OpenNoticeInput {
    pub(crate) fn of(input: &Value) -> Self {
        Self {
            floor: input
                .get("approvalFloor")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_string))
                        .collect()
                }),
            tool_prompts: input.get("windowToolPrompts").and_then(Value::as_bool) == Some(true),
        }
    }
}

/// Opens a window: a window past its cap ends 'expired' and an open one 'replaced', then the new one is written, in
/// the caller's transaction. Returns the window record and the events (the ends first, the opening last).
pub(crate) fn open_window(
    kernel: &Kernel,
    actor: &AuthenticatedActor,
    spec: &WindowSpec,
    source: WindowSource<'_>,
    notice: &OpenNoticeInput,
) -> KernelResult<(Value, Vec<MutationEvent>)> {
    let now = kernel.now();
    let now_ms = parse_iso_ms(&now);
    let mut events = Vec::new();
    if let Some(open) = open_row(kernel)? {
        let reason = if open.due(now_ms) {
            "expired"
        } else {
            "replaced"
        };
        events.push(end_row(kernel, &open, reason, &now)?);
    }
    let sequence = query_opt(
        &kernel.database,
        "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM approval_windows WHERE project_id = ?",
        [&kernel.project_id],
        |row| row.get::<_, i64>(0),
    )?
    .unwrap_or(1);
    let window_id = format!("win-{sequence}");
    let expires_at = iso_from_millis(now_ms + spec.minutes * 60_000);
    let (proposal, relay, label, relay_sourced) = match source {
        WindowSource::Proposal(id) => (Some(id), None, format!("Operator proposal {id}"), false),
        WindowSource::Relay(id) => (None, Some(id), format!("prompt relay {id}"), true),
    };
    execute(
        &kernel.database,
        "INSERT INTO approval_windows(project_id, window_id, sequence, kind, minutes, created_by, source_proposal_id, source_relay_id, created_at, expires_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        rusqlite::params![
            kernel.project_id,
            window_id,
            sequence,
            spec.kind,
            spec.minutes,
            actor.actor_id,
            proposal,
            relay,
            now,
            expires_at
        ],
    )?;
    let row = window_row(kernel, &window_id)?
        .ok_or_else(|| KernelError::controller("the window vanished"))?;
    let record = row.record(now_ms);
    message_notices::notice_to_pm(
        kernel,
        "approval_window",
        &window_opened_notice(
            &record,
            &label,
            notice.floor.as_deref(),
            notice.tool_prompts,
            relay_sourced,
        ),
        &now,
        false,
    )?;
    events.push(MutationEvent {
        to_state: Some("open".into()),
        details: Some(json!({
            "event": "operator.window_opened",
            "kind": spec.kind,
            "minutes": spec.minutes,
            "expiresAt": expires_at,
            "source": record["source"],
        })),
        ..MutationEvent::new("approval_window", &window_id, 0)
    });
    Ok((record, events))
}

/// `endOperatorGrantsOf` for windows: the Operator part of the open window ends when the Operator agent that opened it
/// (or, for a window opened from a worker prompt, any agent that proposes operator actions) is released. The window
/// itself stays open. The caller owns the transaction.
pub(crate) fn end_operator_part_of(kernel: &Kernel, agent_id: &str) -> KernelResult<()> {
    let Some(row) = open_row(kernel)? else {
        return Ok(());
    };
    if row.operator_ended_at.is_some() {
        return Ok(());
    }
    let proposed_here = query_opt(
        &kernel.database,
        "SELECT 1 FROM operator_proposals WHERE project_id = ? AND proposer_agent_id = ? AND (? IS NULL OR proposal_id = ?)",
        rusqlite::params![
            kernel.project_id,
            agent_id,
            row.source_proposal_id,
            row.source_proposal_id
        ],
        |_| Ok(()),
    )?;
    if proposed_here.is_none() {
        return Ok(());
    }
    let now = kernel.now();
    execute(
        &kernel.database,
        "UPDATE approval_windows SET operator_ended_at = ?, operator_ended_reason = 'released' WHERE project_id = ? AND window_id = ? AND operator_ended_at IS NULL",
        [&now, &kernel.project_id, &row.window_id],
    )?;
    message_notices::notice_to_pm(
        kernel,
        "approval_window",
        &window_operator_ended_notice(&row.window_id),
        &now,
        false,
    )?;
    Ok(())
}

fn many_events(ended: Vec<String>, mut events: Vec<MutationEvent>, none: &str) -> MutationOutput {
    let first = if events.is_empty() {
        MutationEvent::new("approval_window", "none", 0).with_details(json!({ none: 0 }))
    } else {
        events.remove(0)
    };
    let mut output = MutationOutput::new(json!(ended), first);
    output.extra_events = events;
    output
}

/// `expireApprovalWindows`: ends the open window when it is past its cap; the operator tick calls it.
pub fn expire_approval_windows(kernel: &Kernel, context: &MutationContext) -> KernelResult<Value> {
    kernel.mutate(
        context,
        "operator.window_expire",
        "controller:reconcile",
        &json!({}),
        |_| {
            let now = kernel.now();
            let mut ended = Vec::new();
            let mut events = Vec::new();
            if let Some(row) = open_row(kernel)? {
                if row.due(parse_iso_ms(&now)) {
                    events.push(end_row(kernel, &row, "expired", &now)?);
                    ended.push(row.window_id);
                }
            }
            Ok(many_events(ended, events, "expired"))
        },
    )
}

/// `endApprovalWindowsForRestart`: at startup no window survives.
pub fn end_approval_windows_for_restart(
    kernel: &Kernel,
    context: &MutationContext,
) -> KernelResult<Value> {
    kernel.mutate(
        context,
        "operator.window_restart",
        "controller:reconcile",
        &json!({}),
        |_| {
            let now = kernel.now();
            let mut ended = Vec::new();
            let mut events = Vec::new();
            if let Some(row) = open_row(kernel)? {
                events.push(end_row(kernel, &row, "restart", &now)?);
                ended.push(row.window_id);
            }
            Ok(many_events(ended, events, "ended"))
        },
    )
}

/// `endApprovalWindow`: the PM or the operator ends the open window at once.
pub fn end_approval_window(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let window_id =
        safe_id(input.get("windowId").unwrap_or(&Value::Null), "window id")?.to_string();
    kernel.mutate(
        context,
        "operator.window_revoke",
        "operator:decide",
        &json!({"windowId": window_id}),
        |actor| {
            let agent = kernel.agent_by_actor(&actor.actor_id)?;
            if actor.role != "operator" && !matches!(&agent, Some(a) if a.kind == "PM") {
                return Err(KernelError::controller(
                    "only the PM or the operator revokes a window",
                ));
            }
            let Some(row) = window_row(kernel, &window_id)? else {
                return Err(KernelError::controller(format!(
                    "not_found: window {window_id} does not exist"
                )));
            };
            let now = kernel.now();
            if row.ended_at.is_some() || row.due(parse_iso_ms(&now)) {
                return Err(KernelError::controller(format!(
                    "window_already_ended: window {window_id} has already ended"
                )));
            }
            let event = end_row(kernel, &row, "revoked", &now)?;
            let after = window_row(kernel, &window_id)?
                .ok_or_else(|| KernelError::controller("the window vanished"))?;
            let mut record = after.record(parse_iso_ms(&now));
            record["endedReason"] = json!("revoked");
            Ok(MutationOutput::new(record, event))
        },
    )
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "activeApprovalWindow" => Some(call(active_approval_window(kernel))),
        "listApprovalWindows" => {
            let limit = arg!(a.opt_i64(0, "limit"));
            Some(call(list_approval_windows(kernel, limit)))
        }
        "expireApprovalWindows" => {
            let context = arg!(a.ctx(0));
            Some(call(expire_approval_windows(kernel, &context)))
        }
        "endApprovalWindowsForRestart" => {
            let context = arg!(a.ctx(0));
            Some(call(end_approval_windows_for_restart(kernel, &context)))
        }
        "endApprovalWindow" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(end_approval_window(kernel, &context, input)))
        }
        _ => None,
    }
}
