//! The details recorded once when an integration is merged: the plans and reports it brought and the impact of its
//! diff (files, areas, added migrations, next steps). Pure: the caller reads the commit info and the diff and passes
//! them in; nothing here touches git or the ledger.
//!
//! The shape (`v` is 1):
//! `{v, impactError, plans: [{planId, title}], plansOmitted, reports: [{reportId, packageId, summary}], reportsOmitted,
//! impact: null | {filesChanged, insertions, deletions, truncated, areas: [{path, files}], areasOmitted,
//! migrations: [path], migrationsOmitted, nextSteps: [text]}}`. A path, title or summary is cleaned and cut at
//! `TEXT_MAX_BYTES` on a character boundary; an entry that was cut carries `"truncated": true` (a migration path cut
//! sets `impact.migrationsTruncated`). The whole text stays within `DETAILS_MAX_BYTES`: the counts below hold it for
//! ordinary text, and `fit` drops the last entries (counting them as omitted) when escaping makes it longer.
use std::collections::{BTreeMap, HashMap};

use capstan_config::ImpactConfig;
use serde_json::{json, Map, Value};

use crate::helpers::{cap_text, clean_text};
use crate::integrate::{DiffStat, DiffStatError};

/// The most bytes of one path, title or summary.
pub const TEXT_MAX_BYTES: usize = 300;
/// The most bytes of the whole stored text.
pub const DETAILS_MAX_BYTES: usize = 16 * 1024;
pub const MAX_REPORTS: usize = 20;
const MAX_AREAS: usize = 12;
const MAX_MIGRATIONS: usize = 10;

fn capped(text: &str) -> (String, bool) {
    let cleaned = clean_text(text);
    let (cut, truncated) = cap_text(&cleaned, TEXT_MAX_BYTES);
    (cut.to_string(), truncated)
}

fn entry(members: Vec<(&str, Value)>, truncated: bool) -> Value {
    let mut map = Map::new();
    for (key, value) in members {
        map.insert(key.into(), value);
    }
    if truncated {
        map.insert("truncated".into(), Value::Bool(true));
    }
    Value::Object(map)
}

fn str_of<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// The details of a merge. `info` is the value of `integration_commit_info`; `packages` maps a report id to its package
/// (a report without one is not in it); `diff` is what `GitRepo::diff_stat` gave
/// for the integration's base and merged head. A failed diff gives `impact: null` and the reason in `impactError`.
pub fn details(
    info: &Value,
    packages: &HashMap<String, String>,
    diff: Result<&DiffStat, &DiffStatError>,
    config: &ImpactConfig,
) -> Value {
    let plans: Vec<Value> = match info.get("planId").and_then(Value::as_str) {
        Some(plan_id) => {
            let (id, id_cut) = capped(plan_id);
            let (title, title_cut) = capped(str_of(info, "planTitle"));
            vec![entry(
                vec![("planId", json!(id)), ("title", json!(title))],
                id_cut || title_cut,
            )]
        }
        None => Vec::new(),
    };
    let all_reports = info
        .get("reports")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let reports: Vec<Value> = all_reports
        .iter()
        .take(MAX_REPORTS)
        .map(|report| {
            let (id, id_cut) = capped(str_of(report, "reportId"));
            let (summary, summary_cut) = capped(str_of(report, "summary"));
            let package = packages
                .get(str_of(report, "reportId"))
                .map(|id| capped(id));
            let package_cut = package.as_ref().is_some_and(|(_, cut)| *cut);
            entry(
                vec![
                    ("reportId", json!(id)),
                    ("packageId", json!(package.map(|(text, _)| text))),
                    ("summary", json!(summary)),
                ],
                id_cut || summary_cut || package_cut,
            )
        })
        .collect();
    let mut value = json!({
        "v": 1,
        "impactError": Value::Null,
        "plans": plans,
        "plansOmitted": 0,
        "reports": reports,
        "reportsOmitted": all_reports.len().saturating_sub(MAX_REPORTS),
        "impact": Value::Null,
    });
    match diff {
        Ok(stat) => value["impact"] = impact(stat, config),
        Err(DiffStatError::Timeout) => value["impactError"] = json!("timeout"),
        Err(DiffStatError::Failed) => value["impactError"] = json!("diff_failed"),
    }
    fit(&mut value);
    value
}

fn impact(stat: &DiffStat, config: &ImpactConfig) -> Value {
    let mut areas: BTreeMap<String, usize> = BTreeMap::new();
    let mut migrations: Vec<&str> = Vec::new();
    for item in &stat.entries {
        *areas.entry(config.area_of(&item.path)).or_default() += 1;
        if item.status == 'A' && config.is_migration(&item.path) {
            migrations.push(&item.path);
        }
    }
    let mut ranked: Vec<(String, usize)> = areas.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let areas_omitted = ranked.len().saturating_sub(MAX_AREAS);
    let areas: Vec<Value> = ranked
        .iter()
        .take(MAX_AREAS)
        .map(|(path, files)| {
            let (path, cut) = capped(path);
            entry(vec![("path", json!(path)), ("files", json!(files))], cut)
        })
        .collect();
    let migrations_omitted = migrations.len().saturating_sub(MAX_MIGRATIONS);
    let mut migrations_cut = false;
    let listed: Vec<Value> = migrations
        .iter()
        .take(MAX_MIGRATIONS)
        .map(|path| {
            let (path, cut) = capped(path);
            migrations_cut |= cut;
            json!(path)
        })
        .collect();
    let mut steps: Vec<&str> = Vec::new();
    for step in &config.next_steps {
        let hit = stat.entries.iter().any(|item| {
            step.paths
                .iter()
                .any(|glob| capstan_config::impact::glob_matches(glob, &item.path))
        });
        if hit && !steps.contains(&step.step.as_str()) {
            steps.push(&step.step);
        }
    }
    let mut out = json!({
        "filesChanged": stat.files_changed,
        "insertions": stat.insertions,
        "deletions": stat.deletions,
        "truncated": stat.truncated,
        "areas": areas,
        "areasOmitted": areas_omitted,
        "migrations": listed,
        "migrationsOmitted": migrations_omitted,
        "nextSteps": steps,
    });
    if migrations_cut {
        out["migrationsTruncated"] = Value::Bool(true);
    }
    out
}

fn size(value: &Value) -> usize {
    crate::json::stringify(value).len()
}

/// Drops entries from the end of the longest lists, counting them as omitted, until the text fits.
fn fit(value: &mut Value) {
    while size(value) > DETAILS_MAX_BYTES {
        let dropped = pop(value, "reports", "reportsOmitted")
            || value.get_mut("impact").is_some_and(|impact| {
                pop(impact, "migrations", "migrationsOmitted")
                    || pop(impact, "areas", "areasOmitted")
                    || pop(impact, "nextSteps", "nextStepsOmitted")
            });
        if !dropped {
            break;
        }
    }
}

fn pop(holder: &mut Value, list: &str, omitted: &str) -> bool {
    let Some(items) = holder.get_mut(list).and_then(Value::as_array_mut) else {
        return false;
    };
    if items.pop().is_none() {
        return false;
    }
    let before = holder.get(omitted).and_then(Value::as_u64).unwrap_or(0);
    holder[omitted] = json!(before + 1);
    true
}
