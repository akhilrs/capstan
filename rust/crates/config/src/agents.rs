//! The `[agents]` section: when an agent that has sat idle is released automatically, and how long before that it is
//! warned. This module holds the shape and resolves and validates the section; nothing here ends a session.
use crate::error::{invalid, Result};
use crate::keys::AGENTS_KEYS;
use crate::primitives::*;
use crate::value::Item;

/// The resolved `[agents]` section. It is never printed in the config JSON.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Agents {
    /// Minutes an agent may sit idle before it is released; 0 turns automatic release off.
    pub idle_release_minutes: i64,
    /// Minutes before the release at which the agent is warned.
    pub idle_release_warn_minutes: i64,
}

impl Default for Agents {
    fn default() -> Self {
        Agents {
            idle_release_minutes: 120,
            idle_release_warn_minutes: 10,
        }
    }
}

/// Resolves `[agents]`; an absent table is `Agents::default()`.
pub fn resolve_agents(value: Option<&Item>) -> Result<Agents> {
    if value.is_none() {
        return Ok(Agents::default());
    }
    let table = required_table(value, "agents")?;
    reject_unknown_keys(table, AGENTS_KEYS, "agents")?;
    let defaults = Agents::default();
    // 0 turns release off; any other value is held to 15..10080.
    let idle_release_minutes = match table.get("idle_release_minutes") {
        Some(Item::Int(0)) => 0,
        Some(Item::Int(n)) if !(15..=10080).contains(n) => {
            return invalid(
                "agents.idle_release_minutes must be 0 (off) or between 15 and 10080".to_string(),
            )
        }
        Some(Item::BigInt(_)) => {
            return invalid(
                "agents.idle_release_minutes must be 0 (off) or between 15 and 10080".to_string(),
            )
        }
        other => optional_integer(
            other,
            "agents.idle_release_minutes",
            15,
            10080,
            defaults.idle_release_minutes,
        )?,
    };
    let idle_release_warn_minutes = optional_integer(
        table.get("idle_release_warn_minutes"),
        "agents.idle_release_warn_minutes",
        1,
        60,
        defaults.idle_release_warn_minutes,
    )?;
    if idle_release_minutes != 0 && idle_release_warn_minutes >= idle_release_minutes {
        return invalid(format!(
            "agents.idle_release_warn_minutes ({idle_release_warn_minutes}) must be less than agents.idle_release_minutes ({idle_release_minutes})"
        ));
    }
    Ok(Agents {
        idle_release_minutes,
        idle_release_warn_minutes,
    })
}
