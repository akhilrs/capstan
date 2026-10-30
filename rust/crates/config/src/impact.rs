//! The `[impact]` section: how a merged integration's changed paths are grouped into areas, which added paths count as
//! migrations and which next steps a change calls for. This module holds the shape, the rules for a pattern and the
//! matchers the kernel applies to a path; nothing here runs git or reads a file.
use capstan_wire::js::Value;

use crate::error::{invalid, Result};
use crate::json::{object, strings, text};
use crate::keys::IMPACT_KEYS;
use crate::primitives::*;
use crate::value::{Item, Table};

/// The most entries `areas`, `migrations` and one step's `paths` may hold.
pub const IMPACT_MAX_LIST_ENTRIES: usize = 64;
/// The most `next_steps` entries.
pub const IMPACT_MAX_NEXT_STEPS: usize = 10;
/// The most bytes one pattern may hold.
pub const IMPACT_MAX_PATTERN_BYTES: usize = 200;
/// The most bytes one step text may hold.
pub const IMPACT_MAX_STEP_BYTES: usize = 200;

/// The keys of one `next_steps` entry.
pub const IMPACT_NEXT_STEP_KEYS: &[&str] = &["paths", "step"];

/// One `next_steps` entry: a step to take when a changed path matches one of the globs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NextStep {
    pub paths: Vec<String>,
    pub step: String,
}

/// The resolved `[impact]` section; an absent section is the default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImpactConfig {
    /// Area patterns: `a/b/*` maps a path under `a/b` to `a/b/<child>`, a plain prefix maps to itself. Empty means the
    /// first path component.
    pub areas: Vec<String>,
    /// Globs of the added paths that are migrations.
    pub migrations: Vec<String>,
    pub next_steps: Vec<NextStep>,
}

impl ImpactConfig {
    pub(crate) fn to_value(&self) -> Value {
        let steps = self
            .next_steps
            .iter()
            .map(|entry| {
                object(vec![
                    ("paths", strings(&entry.paths)),
                    ("step", text(&entry.step)),
                ])
            })
            .collect();
        object(vec![
            ("areas", strings(&self.areas)),
            ("migrations", strings(&self.migrations)),
            ("nextSteps", Value::Array(steps)),
        ])
    }

    /// The area a changed path belongs to: the longest matching area pattern wins; with none, the first path component.
    pub fn area_of(&self, path: &str) -> String {
        let mut best: Option<(usize, String)> = None;
        for pattern in &self.areas {
            let found = match pattern.strip_suffix("/*") {
                Some(prefix) => path
                    .strip_prefix(prefix)
                    .and_then(|rest| rest.strip_prefix('/'))
                    .filter(|rest| !rest.is_empty())
                    .map(|rest| {
                        let child = rest.split('/').next().unwrap_or(rest);
                        format!("{prefix}/{child}")
                    }),
                None => (path == pattern
                    || path
                        .strip_prefix(pattern.as_str())
                        .is_some_and(|rest| rest.starts_with('/')))
                .then(|| pattern.clone()),
            };
            if let Some(area) = found {
                let rank = pattern.len();
                if best.as_ref().is_none_or(|(len, _)| rank > *len) {
                    best = Some((rank, area));
                }
            }
        }
        match best {
            Some((_, area)) => area,
            None => path.split('/').next().unwrap_or(path).to_string(),
        }
    }

    /// Whether `path` matches one of the `migrations` globs.
    pub fn is_migration(&self, path: &str) -> bool {
        self.migrations.iter().any(|glob| glob_matches(glob, path))
    }
}

/// Matches `path` against `glob`: `*` is any run of characters but `/`, `?` is one character but `/`, and `**` is any
/// run of characters including `/`.
pub fn glob_matches(glob: &str, path: &str) -> bool {
    fn go(glob: &[u8], path: &[u8]) -> bool {
        match glob.split_first() {
            None => path.is_empty(),
            Some((b'*', rest)) => {
                let (deep, rest) = match rest.split_first() {
                    Some((b'*', after)) => (true, after),
                    _ => (false, rest),
                };
                let mut taken = 0;
                loop {
                    if go(rest, &path[taken..]) {
                        return true;
                    }
                    if taken == path.len() || (!deep && path[taken] == b'/') {
                        return false;
                    }
                    taken += 1;
                }
            }
            Some((b'?', rest)) => match path.split_first() {
                Some((byte, tail)) if *byte != b'/' => go(rest, tail),
                _ => false,
            },
            Some((byte, rest)) => match path.split_first() {
                Some((head, tail)) if head == byte => go(rest, tail),
                _ => false,
            },
        }
    }
    go(glob.as_bytes(), path.as_bytes())
}

fn pattern_problem(pattern: &str, area: bool) -> Option<&'static str> {
    if pattern.is_empty() || pattern.len() > IMPACT_MAX_PATTERN_BYTES {
        return Some("must be 1 to 200 bytes");
    }
    if pattern.starts_with('/') || pattern.ends_with('/') || pattern.contains("//") {
        return Some("must be a relative path without empty segments");
    }
    if pattern.contains('\\') || pattern.chars().any(char::is_control) {
        return Some("must not contain a backslash or a control character");
    }
    if pattern
        .split('/')
        .any(|segment| segment == "." || segment == "..")
    {
        return Some("must not contain . or .. segments");
    }
    if pattern.contains(['[', ']', '{', '}', '!']) || pattern.contains("***") {
        return Some("is not a valid glob");
    }
    if area {
        let body = pattern.strip_suffix("/*").unwrap_or(pattern);
        if body.contains(['*', '?']) {
            return Some("may use a wildcard only as a final /*");
        }
        if body.is_empty() {
            return Some("must name a directory before /*");
        }
    }
    None
}

fn pattern_list(value: Option<&Item>, at: &str, area: bool) -> Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let Item::Array(entries) = value else {
        return invalid(format!("{at} must be an array of strings"));
    };
    if entries.len() > IMPACT_MAX_LIST_ENTRIES {
        return invalid(format!("{at} exceeds {IMPACT_MAX_LIST_ENTRIES} entries"));
    }
    let mut list = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let at = format!("{at}[{index}]");
        let Item::Str(pattern) = entry else {
            return invalid(format!("{at} must be a string"));
        };
        if let Some(reason) = pattern_problem(pattern, area) {
            return invalid(format!("{at} {reason}"));
        }
        list.push(pattern.clone());
    }
    Ok(list)
}

/// Reads `[impact]`; every refusal names its key and none echoes a rejected value.
pub(crate) fn resolve_impact(table: &Table) -> Result<ImpactConfig> {
    reject_unknown_keys(table, IMPACT_KEYS, "impact")?;
    let areas = pattern_list(table.get("areas"), "impact.areas", true)?;
    let migrations = pattern_list(table.get("migrations"), "impact.migrations", false)?;
    let mut next_steps = Vec::new();
    match table.get("next_steps") {
        None => {}
        Some(Item::Array(entries)) => {
            if entries.len() > IMPACT_MAX_NEXT_STEPS {
                return invalid(format!(
                    "impact.next_steps exceeds {IMPACT_MAX_NEXT_STEPS} entries"
                ));
            }
            for (index, entry) in entries.iter().enumerate() {
                let at = format!("impact.next_steps[{index}]");
                let Item::Table(entry) = entry else {
                    return invalid(format!("{at} must be a table"));
                };
                reject_unknown_keys(entry, IMPACT_NEXT_STEP_KEYS, &at)?;
                let paths = pattern_list(entry.get("paths"), &format!("{at}.paths"), false)?;
                if paths.is_empty() {
                    return invalid(format!("{at}.paths must name at least one glob"));
                }
                let Some(Item::Str(step)) = entry.get("step") else {
                    return invalid(format!("{at}.step must be a string of 1 to 200 bytes"));
                };
                if step.is_empty() || step.len() > IMPACT_MAX_STEP_BYTES {
                    return invalid(format!("{at}.step must be a string of 1 to 200 bytes"));
                }
                assert_safe_text(step, &format!("{at}.step"), false)?;
                next_steps.push(NextStep {
                    paths,
                    step: step.clone(),
                });
            }
        }
        Some(_) => return invalid("impact.next_steps must be an array of tables"),
    }
    Ok(ImpactConfig {
        areas,
        migrations,
        next_steps,
    })
}
