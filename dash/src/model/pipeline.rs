//! The pipeline stages: reports, reviews and integrations with their per-state counts.
use indexmap::IndexMap;
use serde_json::Value;

use super::build::{
    field, get, is_objectish, is_plain_object, num_str, slice_units, text, text_of, text_or_null,
};
use super::helpers::PIPELINE_ITEMS;
use super::text::clean;
use super::types::{PipelineItem, PipelineStage};

/// `<agent> <short commit>`.
pub(super) fn label_report(row: &Value) -> String {
    let sha = text_or_null(get(row, "commitSha")).filter(|s| !s.is_empty());
    let short = sha.map_or_else(|| "-".to_string(), |s| slice_units(&s, 7));
    format!("{} {}", text_of(row, "agentId"), short)
}

/// `reviewer-4 -> developer-1 r1`: who reviews, whose work and the round; an integration review names the integration.
pub(super) fn label_review(row: &Value) -> String {
    let reviewer = Some(text_of(row, "reviewerAgentId"))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "no reviewer".to_string());
    let integration = text_of(row, "integrationId");
    let subject = Some(text_of(row, "authorAgentId"))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| slice_units(&integration, 8));
    let round = super::build::num(get(row, "round"));
    let arrow = if subject.is_empty() {
        String::new()
    } else {
        format!(" -> {subject}")
    };
    format!("{reviewer}{arrow} r{}", num_str(round))
}

pub(super) fn label_integration(row: &Value) -> String {
    slice_units(&text_of(row, "integrationId"), 8)
}

/// The all-time per-state counts of one stage from `status.pipelineCounts`, or `None` when absent or malformed.
pub(super) fn true_counts(
    pipeline_counts: Option<&Value>,
    key: &str,
) -> Option<IndexMap<String, i64>> {
    let all = pipeline_counts.filter(|v| is_objectish(v))?;
    let raw = field(all, key).filter(|v| is_plain_object(v))?;
    let mut counts = IndexMap::new();
    for (state, count) in raw.as_object()? {
        let n = count.as_f64().filter(|n| n.is_finite() && *n >= 0.0)?;
        if n > 0.0 {
            counts.insert(clean(state), n.floor() as i64);
        }
    }
    Some(counts)
}

pub(super) fn stage_of(
    rows: &[&Value],
    stage: &str,
    id_key: &str,
    label: fn(&Value) -> String,
    truth: Option<IndexMap<String, i64>>,
) -> PipelineStage {
    let mut counts: IndexMap<String, i64> = IndexMap::new();
    for row in rows {
        *counts.entry(text(get(row, "state"))).or_insert(0) += 1;
    }
    let mut items: Vec<PipelineItem> = rows
        .iter()
        .map(|row| {
            let state = text_of(row, "state");
            let label = label(row);
            PipelineItem {
                id: format!("{stage}:{}", text(get(row, id_key))),
                stage: stage.to_string(),
                fingerprint: format!("{state}|{label}"),
                state,
                label,
                created_at: text_of(row, "createdAt"),
            }
        })
        .collect();
    items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    items.truncate(PIPELINE_ITEMS);
    match truth {
        Some(counts) => PipelineStage {
            total: counts.values().sum(),
            counts,
            capped: false,
            items,
        },
        None => PipelineStage {
            counts,
            total: rows.len() as i64,
            capped: rows.len() >= PIPELINE_ITEMS,
            items,
        },
    }
}
