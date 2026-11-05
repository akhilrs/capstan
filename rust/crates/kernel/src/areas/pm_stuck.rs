//! A stuck PM: the controller notices that the active PM has waited too long and records one episode for it.
//!
//! # Reasons
//! For the one active PM that is not itself paused, the first match in this order (`N` is the configured minutes):
//! * `unread_mail`: the PM is `idle` or `done` and its oldest pending action message (queued, sent or unacknowledged)
//!   has waited at least `N` minutes since it was queued.
//! * `no_progress`: the PM is `working`, its counted time since its last observed activity is at least `N` minutes
//!   (recorded waits do not count), none of its tool processes was active within `N` minutes, and the run is not paused.
//!   It needs `N` minutes of watching after the controller started; the activity it reads is the daemon's own.
//! * `prompt`: the PM is `blocked`, or has an open prompt relay, for `N` minutes.
//!
//! A run pause does not pause the PM, so `unread_mail` and `prompt` still apply under one. `unread_mail` and `prompt`
//! read ledger times and hold across a restart.
//!
//! # Episodes
//! A row of `pm_stuck_episodes` is open from the tick that finds a reason until the tick that closes it. The tick writes
//! to the ledger only on open (`pm.stuck`) or close (`pm.recovered`); the clear timer (`clear_since_ms`) is kept in the
//! row without a ledger row. An open episode survives a restart and is not raised again. It closes as `recovered` after the
//! condition has been clear for [`CLEAR_MS`] (timed by `clear_since_ms`, so a tick slower than that takes one tick), as
//! `paused` when the PM is paused, `replaced` when its agent was replaced and `ended` when it ended otherwise. While the
//! condition cannot be evaluated (a `no_progress` episode during the watching window of a start) it is neither cleared
//! nor raised again.
//!
//! `alert` is false when the same agent id has an episode with `alert` true that opened within [`SUPPRESS_MS`]; only an
//! alerting episode is pushed or notified by the apps. The rows are never pruned.

use super::messaging::{
    counted_millis, parse_iso_ms, state_at, state_since, AgentFacts, MessageClass, MessageFacts,
};
use super::{message_notices, pauses};
use crate::errors::KernelResult;
use crate::helpers::{execute, query_all, query_opt};
use crate::kernel::Kernel;
use crate::records::{MutationEvent, MutationOutput};
use crate::types::MutationContext;
use capstan_ledger::iso_from_millis;
use serde_json::{json, Value};
use std::collections::HashMap;

/// How long the condition must stay clear before an episode recovers.
pub const CLEAR_MS: i64 = 60_000;
/// How long after an alerting episode opened a new one of the same agent id does not alert.
pub const SUPPRESS_MS: i64 = 30 * 60_000;

/// Why the PM is stuck.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    UnreadMail,
    NoProgress,
    Prompt,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::UnreadMail => "unread_mail",
            Reason::NoProgress => "no_progress",
            Reason::Prompt => "prompt",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "unread_mail" => Some(Reason::UnreadMail),
            "no_progress" => Some(Reason::NoProgress),
            "prompt" => Some(Reason::Prompt),
            _ => None,
        }
    }
}

/// An open prompt relay of an agent.
#[derive(Clone, Debug)]
pub struct RelayFact {
    pub agent_id: String,
    pub captured_ms: i64,
}

/// What the pure evaluation finds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evaluation {
    /// The one active PM; none when there is no PM or more than one.
    pub pm_id: Option<String>,
    /// The PM itself is paused.
    pub pm_paused: bool,
    /// The first reason that holds, with the time it began.
    pub found: Option<(Reason, i64)>,
    /// `no_progress` can be evaluated (the controller has watched for `N` minutes).
    pub watching: bool,
}

/// `evaluatePmStuck`: pure. `watch_start_ms` is when this controller began watching.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_pm_stuck(
    agents: &[AgentFacts],
    messages: &[MessageFacts],
    run_paused: bool,
    relays: &[RelayFact],
    child_activity: &HashMap<String, u64>,
    now_ms: i64,
    minutes: i64,
    watch_start_ms: i64,
) -> Evaluation {
    let limit = minutes * 60_000;
    let watching = now_ms - watch_start_ms >= limit;
    let mut pms = agents.iter().filter(|a| a.kind == "PM");
    let (Some(pm), None) = (pms.next(), pms.next()) else {
        return Evaluation {
            pm_id: None,
            pm_paused: false,
            found: None,
            watching,
        };
    };
    let mut evaluation = Evaluation {
        pm_id: Some(pm.agent_id.clone()),
        pm_paused: pm.paused,
        found: None,
        watching,
    };
    if pm.paused {
        return evaluation;
    }
    let state = state_at(&pm.observations, now_ms);
    // unread_mail.
    if state == "idle" || state == "done" {
        let oldest = messages
            .iter()
            .filter(|m| {
                m.recipient_agent_id == pm.agent_id
                    && m.class == MessageClass::Action
                    && matches!(m.state.as_str(), "queued" | "sent" | "unacked")
            })
            .map(|m| m.queued_ms)
            .min();
        if let Some(queued) = oldest.filter(|queued| now_ms - queued >= limit) {
            evaluation.found = Some((Reason::UnreadMail, queued));
            return evaluation;
        }
    }
    // no_progress.
    if state == "working" && !run_paused && watching {
        let counted = counted_millis(
            "stall",
            pm.last_activity_ms,
            now_ms,
            &pm.observations,
            &pm.waits,
        );
        let tool_recent = child_activity
            .get(&pm.agent_id)
            .is_some_and(|at| now_ms - (*at as i64) < limit);
        if counted >= limit && !tool_recent {
            evaluation.found = Some((Reason::NoProgress, pm.last_activity_ms));
            return evaluation;
        }
    }
    // prompt.
    let blocked_since = if state == "blocked" {
        state_since(&pm.observations, now_ms)
    } else {
        None
    };
    let relay_since = relays
        .iter()
        .filter(|r| r.agent_id == pm.agent_id)
        .map(|r| r.captured_ms)
        .min();
    let since = [blocked_since, relay_since].into_iter().flatten().min();
    if let Some(since) = since.filter(|since| now_ms - since >= limit) {
        evaluation.found = Some((Reason::Prompt, since));
    }
    evaluation
}

struct OpenEpisode {
    episode_id: String,
    agent_id: String,
    reason: String,
    clear_since_ms: Option<i64>,
}

enum Change {
    Open {
        agent_id: String,
        reason: Reason,
        since_ms: i64,
    },
    Close {
        episode_id: String,
        agent_id: String,
        reason: String,
        close_reason: &'static str,
    },
}

fn open_episodes(kernel: &Kernel) -> KernelResult<Vec<OpenEpisode>> {
    query_all(
        &kernel.database,
        "SELECT episode_id, agent_id, reason, clear_since_ms FROM pm_stuck_episodes WHERE project_id = ? AND closed_at IS NULL ORDER BY opened_ms",
        [&kernel.project_id],
        |row| {
            Ok(OpenEpisode {
                episode_id: row.get(0)?,
                agent_id: row.get(1)?,
                reason: row.get(2)?,
                clear_since_ms: row.get(3)?,
            })
        },
    )
}

fn open_relays(kernel: &Kernel) -> KernelResult<Vec<RelayFact>> {
    query_all(
        &kernel.database,
        "SELECT agent_id, captured_at FROM prompt_relays WHERE project_id = ? AND state IN ('captured', 'typing')",
        [&kernel.project_id],
        |row| {
            Ok(RelayFact {
                agent_id: row.get(0)?,
                captured_ms: parse_iso_ms(&row.get::<_, String>(1)?),
            })
        },
    )
}

/// `pmStuckTick`: one pass of the controller. Writes one ledger row per call at most: the closes of the open episodes that
/// are due, else the open of a new one (a reason found while an episode just closed waits for the next tick).
pub fn pm_stuck_tick(
    kernel: &Kernel,
    context: &MutationContext,
    child_activity: &HashMap<String, u64>,
    now_ms: i64,
    minutes: i64,
    watch_start_ms: i64,
) -> KernelResult<Value> {
    kernel.authorize(&context.credential, "controller:reconcile")?;
    let (agents, messages) = message_notices::messaging_facts(kernel, now_ms)?;
    let run_paused = pauses::pause_state_of(kernel)?.run.is_some();
    let relays = open_relays(kernel)?;
    let evaluation = evaluate_pm_stuck(
        &agents,
        &messages,
        run_paused,
        &relays,
        child_activity,
        now_ms,
        minutes,
        watch_start_ms,
    );
    let open = open_episodes(kernel)?;
    let mut closes: Vec<Change> = Vec::new();
    let mut has_open_for_pm = false;
    for episode in &open {
        if evaluation.pm_id.as_deref() != Some(episode.agent_id.as_str()) {
            let state = kernel
                .agent_row(&episode.agent_id)?
                .map(|agent| agent.state)
                .unwrap_or_default();
            // Two active PMs make the PM unknown: the episode waits.
            if state == "active" {
                continue;
            }
            // Replaced: another PM took over, or the chain record of the replacement is in the ledger.
            let replaced = evaluation.pm_id.is_some()
                || query_opt(
                    &kernel.database,
                    "SELECT 1 FROM controller_events WHERE project_id = ?1 AND entity_type = 'agent' AND entity_id = ?2 AND to_state = 'replaced' LIMIT 1",
                    [&kernel.project_id, &episode.agent_id],
                    |_| Ok(()),
                )?
                .is_some();
            closes.push(Change::Close {
                episode_id: episode.episode_id.clone(),
                agent_id: episode.agent_id.clone(),
                reason: episode.reason.clone(),
                close_reason: if replaced { "replaced" } else { "ended" },
            });
            continue;
        }
        has_open_for_pm = true;
        let close = |close_reason: &'static str| Change::Close {
            episode_id: episode.episode_id.clone(),
            agent_id: episode.agent_id.clone(),
            reason: episode.reason.clone(),
            close_reason,
        };
        if evaluation.pm_paused {
            closes.push(close("paused"));
        } else if evaluation.found.is_some() {
            if episode.clear_since_ms.is_some() {
                set_clear_since(kernel, &episode.episode_id, None)?;
            }
        } else if episode.reason == Reason::NoProgress.as_str() && !evaluation.watching {
            // Not evaluable yet: neither cleared nor raised again.
        } else {
            match episode.clear_since_ms {
                None => set_clear_since(kernel, &episode.episode_id, Some(now_ms))?,
                Some(since) if now_ms - since >= CLEAR_MS => closes.push(close("recovered")),
                Some(_) => {}
            }
        }
    }
    let changes: Vec<Change> = if !closes.is_empty() {
        closes
    } else if let (Some(pm_id), Some((reason, since_ms)), false, false) = (
        &evaluation.pm_id,
        evaluation.found,
        has_open_for_pm,
        evaluation.pm_paused,
    ) {
        vec![Change::Open {
            agent_id: pm_id.clone(),
            reason,
            since_ms,
        }]
    } else {
        Vec::new()
    };
    if changes.is_empty() {
        return Ok(json!({"written": 0}));
    }
    write_changes(kernel, context, changes, now_ms, minutes)
}

fn set_clear_since(kernel: &Kernel, episode_id: &str, value: Option<i64>) -> KernelResult<()> {
    kernel.assert_open()?;
    execute(
        &kernel.database,
        "UPDATE pm_stuck_episodes SET clear_since_ms = ?1 WHERE project_id = ?2 AND episode_id = ?3 AND closed_at IS NULL",
        rusqlite::params![value, kernel.project_id, episode_id],
    )?;
    Ok(())
}

fn write_changes(
    kernel: &Kernel,
    context: &MutationContext,
    changes: Vec<Change>,
    now_ms: i64,
    minutes: i64,
) -> KernelResult<Value> {
    let opening = matches!(changes.first(), Some(Change::Open { .. }));
    let action = if opening { "pm.stuck" } else { "pm.recovered" };
    let payload = json!({
        "episodes": changes.iter().map(|change| match change {
            Change::Open { agent_id, reason, .. } => json!({"agentId": agent_id, "reason": reason.as_str()}),
            Change::Close { episode_id, close_reason, .. } => json!({"episodeId": episode_id, "closeReason": close_reason}),
        }).collect::<Vec<_>>(),
    });
    kernel.mutate(
        context,
        action,
        "controller:reconcile",
        &payload,
        |_| {
            let mut events: Vec<MutationEvent> = Vec::new();
            let mut written: Vec<Value> = Vec::new();
            for change in &changes {
                match change {
                    Change::Open {
                        agent_id,
                        reason,
                        since_ms,
                    } => {
                        let suppressed = query_opt(
                            &kernel.database,
                            "SELECT 1 FROM pm_stuck_episodes WHERE project_id = ?1 AND agent_id = ?2 AND alert = 1 AND opened_ms > ?3 LIMIT 1",
                            rusqlite::params![kernel.project_id, agent_id, now_ms - SUPPRESS_MS],
                            |_| Ok(()),
                        )?
                        .is_some();
                        let alert = !suppressed;
                        let episode_id = format!("stuck-{}", kernel.env.uuid());
                        execute(
                            &kernel.database,
                            "INSERT INTO pm_stuck_episodes(project_id, episode_id, agent_id, reason, since_ms, minutes, alert, opened_ms)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                            rusqlite::params![
                                kernel.project_id,
                                episode_id,
                                agent_id,
                                reason.as_str(),
                                since_ms,
                                minutes,
                                alert as i64,
                                now_ms
                            ],
                        )?;
                        written.push(json!({"opened": episode_id}));
                        let mut event = MutationEvent::new("pm_stuck", &episode_id, 0)
                            .with_details(json!({
                                "agentId": agent_id,
                                "episodeId": episode_id,
                                "reason": reason.as_str(),
                                "sinceMs": since_ms,
                                "minutes": minutes,
                                "alert": alert,
                            }));
                        event.to_state = Some("open".into());
                        events.push(event);
                    }
                    Change::Close {
                        episode_id,
                        agent_id,
                        reason,
                        close_reason,
                    } => {
                        execute(
                            &kernel.database,
                            "UPDATE pm_stuck_episodes SET closed_at = ?1, closed_ms = ?2, close_reason = ?3
                              WHERE project_id = ?4 AND episode_id = ?5 AND closed_at IS NULL",
                            rusqlite::params![
                                iso_from_millis(now_ms),
                                now_ms,
                                close_reason,
                                kernel.project_id,
                                episode_id
                            ],
                        )?;
                        written.push(json!({"closed": episode_id}));
                        events.push(
                            MutationEvent::new("pm_stuck", episode_id, 0)
                                .transition("open", close_reason)
                                .with_details(json!({
                                    "agentId": agent_id,
                                    "episodeId": episode_id,
                                    "reason": reason,
                                    "closeReason": close_reason,
                                })),
                        );
                    }
                }
            }
            let mut events = events.into_iter();
            let first = events.next().expect("a change was planned");
            let mut output = MutationOutput::new(json!({"written": written.len(), "changes": written}), first);
            output.extra_events = events.collect();
            Ok(output)
        },
    )
}

/// The open (not closed) episode of an agent, for the PM card: `{episodeId, reason, since, alert}` with `since` an ISO time.
pub fn open_episode_value(row: (String, String, i64, i64)) -> Value {
    let (episode_id, reason, since_ms, alert) = row;
    json!({
        "episodeId": episode_id,
        "reason": reason,
        "since": iso_from_millis(since_ms),
        "alert": alert == 1,
    })
}
