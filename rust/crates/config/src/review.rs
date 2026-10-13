//! The `[review]` section: whether a plan's reviewer is kept warm between review requests. This module holds the shape
//! and resolves and validates the section; nothing here starts or ends a session.
use capstan_wire::js::Value;

use crate::error::Result;
use crate::json::{number, object};
use crate::primitives::*;
use crate::value::Item;

/// The resolved `[review]` section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Review {
    /// True when the section is present in capstan.toml (the JSON output prints it only then).
    pub configured: bool,
    /// Keep a plan's reviewer after its verdict so the next review request of the plan reuses it.
    pub warm_reviewer: bool,
    /// Minutes a kept reviewer may sit idle before the controller ends it.
    pub idle_minutes: i64,
    /// Review requests one kept reviewer answers before it is replaced by a fresh one.
    pub max_reviews: i64,
    /// Whether integration reviews may also reuse the plan's reviewer.
    pub integration: bool,
}

impl Default for Review {
    fn default() -> Self {
        Review {
            configured: false,
            warm_reviewer: true,
            idle_minutes: 30,
            max_reviews: 6,
            integration: false,
        }
    }
}

impl Review {
    pub(crate) fn to_value(&self) -> Value {
        object(vec![
            ("warmReviewer", Value::Bool(self.warm_reviewer)),
            ("idleMinutes", number(self.idle_minutes)),
            ("maxReviews", number(self.max_reviews)),
            ("integration", Value::Bool(self.integration)),
        ])
    }
}

/// Resolves `[review]`; an absent table is `Review::default()` with `configured` false.
pub fn resolve_review(value: Option<&Item>) -> Result<Review> {
    if value.is_none() {
        return Ok(Review::default());
    }
    let table = required_table(value, "review")?;
    reject_unknown_keys(
        table,
        &[
            "warm_reviewer",
            "idle_minutes",
            "max_reviews",
            "integration",
        ],
        "review",
    )?;
    let defaults = Review::default();
    Ok(Review {
        configured: true,
        warm_reviewer: optional_boolean(
            table.get("warm_reviewer"),
            "review.warm_reviewer",
            defaults.warm_reviewer,
        )?,
        idle_minutes: optional_integer(
            table.get("idle_minutes"),
            "review.idle_minutes",
            5,
            240,
            defaults.idle_minutes,
        )?,
        max_reviews: optional_integer(
            table.get("max_reviews"),
            "review.max_reviews",
            1,
            20,
            defaults.max_reviews,
        )?,
        integration: optional_boolean(
            table.get("integration"),
            "review.integration",
            defaults.integration,
        )?,
    })
}
