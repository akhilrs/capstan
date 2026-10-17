//! The replay engine of the parity sequences: runs the steps of a group file (exported from Node by
//! test/kernel-parity-export.ts) on the Rust kernel and compares every outcome, counter and table dump.

use capstan_kernel::env::Env;
use capstan_kernel::types::InitialProject;
use capstan_kernel::{Core, KernelError, KernelOptions, SeededEnv};
use capstan_ledger::{open_database, OpenOptions};
use rusqlite::types::ValueRef;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Passed,
    /// A step reached code of an area that is not ported; the text says which.
    Pending(String),
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct Report {
    pub group: String,
    pub sequence: String,
    pub status: Status,
}

impl Report {
    pub fn line(&self) -> String {
        let (word, detail) = match &self.status {
            Status::Passed => ("passed", String::new()),
            Status::Pending(d) => ("pending", format!(" ({d})")),
            Status::Failed(d) => ("FAILED", format!("\n{d}")),
        };
        format!("{}/{}: {word}{detail}", self.group, self.sequence)
    }
}

/// One clock and one random stream for every controller handle of a sequence, like the hooks on the Node side.
struct SharedEnv(Rc<SeededEnv>);

impl Env for SharedEnv {
    fn now(&self) -> i64 {
        self.0.now()
    }
    fn uuid(&self) -> String {
        self.0.uuid()
    }
    fn random_bytes(&self, n: usize) -> Vec<u8> {
        self.0.random_bytes(n)
    }
}

fn clip(value: &Value) -> String {
    let text = serde_json::to_string(value).unwrap_or_default();
    if text.chars().count() > 600 {
        format!("{}...", text.chars().take(600).collect::<String>())
    } else {
        text
    }
}

/// Fields and columns only the Rust build has (the frozen Node corpus predates them): message records carry `class`,
/// plan records `maxReviewRounds`, package records `design` and `designDecision` (false and null unless a design package), the shown plan revision `amendment` and `amendsReviewId` (null unless amended), and
/// the unread summary `wakeCount` and `infoCount`. A comparison leaves them out of the actual side, so the corpus
/// replays unchanged; the Rust-only tests check them directly.
fn without_rust_only(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let message_record = map.contains_key("messageId") && map.contains_key("actionNeeded");
            let summary = map.contains_key("oldestQueuedAt") && map.contains_key("messageIds");
            let plan_record = map.contains_key("planId") && map.contains_key("approvedRevision");
            let plan_revision = map.contains_key("bodySha") && map.contains_key("amendsReviewId");
            let package_record = map.contains_key("packageId") && map.contains_key("progress");
            // The controller's capabilities include prompt:relay (migration 0047), which the Node corpus never had.
            let controller = map.get("role").and_then(Value::as_str) == Some("controller");
            let mut kept: Map<String, Value> = map
                .iter()
                .filter(|(key, v)| {
                    !(matches!(key.as_str(), "auto_window_id" | "auto_begin_at") && v.is_null()
                        || message_record && key.as_str() == "class"
                        || summary && matches!(key.as_str(), "wakeCount" | "infoCount")
                        || plan_record && key.as_str() == "maxReviewRounds"
                        || package_record
                            && matches!(key.as_str(), "design" | "designDecision")
                            && (v.is_null() || v.as_bool() == Some(false))
                        || plan_revision
                            && matches!(key.as_str(), "amendment" | "amendsReviewId")
                            && v.is_null())
                })
                .map(|(key, v)| (key.clone(), without_rust_only(v)))
                .collect();
            if let (true, Some(Value::Array(capabilities))) =
                (controller, kept.get_mut("capabilities"))
            {
                capabilities.retain(|capability| capability != "prompt:relay");
            }
            Value::Object(kept)
        }
        Value::Array(items) => Value::Array(items.iter().map(without_rust_only).collect()),
        other => other.clone(),
    }
}

/// The first place two JSON values differ, with both sides (Rust-only fields of the actual side are ignored).
pub fn first_difference(expected: &Value, actual: &Value, path: &str) -> Option<String> {
    difference(expected, &without_rust_only(actual), path)
}

fn difference(expected: &Value, actual: &Value, path: &str) -> Option<String> {
    if expected == actual {
        return None;
    }
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => {
            for key in e.keys().chain(a.keys()) {
                let here = format!("{path}.{key}");
                match (e.get(key), a.get(key)) {
                    (Some(x), Some(y)) => {
                        if let Some(diff) = difference(x, y, &here) {
                            return Some(diff);
                        }
                    }
                    (x, y) => {
                        return Some(format!(
                            "{here}: expected {} but got {}",
                            x.map_or("nothing".into(), clip),
                            y.map_or("nothing".into(), clip)
                        ))
                    }
                }
            }
            None
        }
        (Value::Array(e), Value::Array(a)) => {
            for i in 0..e.len().max(a.len()) {
                let here = format!("{path}[{i}]");
                match (e.get(i), a.get(i)) {
                    (Some(x), Some(y)) => {
                        if let Some(diff) = difference(x, y, &here) {
                            return Some(diff);
                        }
                    }
                    (x, y) => {
                        return Some(format!(
                            "{here}: expected {} but got {}",
                            x.map_or("nothing".into(), clip),
                            y.map_or("nothing".into(), clip)
                        ))
                    }
                }
            }
            None
        }
        _ => Some(format!(
            "{path}: expected {} but got {}",
            clip(expected),
            clip(actual)
        )),
    }
}

/// DELIBERATE PIN (frozen Node ledger format; never follows the newest migration). The last migration Node knows. Ledgers the Rust build opens carry later migrations that no frozen Node export has, so
/// their `schema_migrations` rows are left out of a dump.
pub const NODE_LAST_MIGRATION: i64 = 37;

/// Tables only the Rust build has. A dump leaves one out while it is empty; a non-empty one stays in, so a replay that
/// writes rows there fails the comparison.
pub const RUST_ONLY_TABLES: [&str; 2] = ["cleanups", "cleanup_branches"];

/// The review request of a plan and the needs-attention notice, as the frozen Node corpus has them: the brief this build writes adds an exhaustive-list
/// instruction, the finding line format and a closing sentence, and the corpus predates them. The dump of the actual side
/// gets the Node text (and its hash) back, so the corpus replays unchanged; the Rust-only tests check the new brief.
fn legacy_plan_brief(columns: &[String], values: &mut [Value]) {
    let (Some(body), Some(hash)) = (
        columns.iter().position(|c| c == "body"),
        columns.iter().position(|c| c == "body_hash"),
    ) else {
        return;
    };
    let Some(text) = values[body].as_str() else {
        return;
    };
    let first = text.lines().next().unwrap_or_default();
    if let Some((plan, rest)) = first.split_once(" needs attention: it used ") {
        if plan.starts_with("Plan ") && rest.contains(" design review rounds (limit ") {
            let legacy = format!("{plan} needs attention: it used 5 review rounds without a pass. It is a draft; decide whether to replace the architect or open a new plan.");
            values[hash] = Value::String(capstan_kernel::canonical::sha256(&legacy));
            values[body] = Value::String(legacy);
        }
        return;
    }
    if !first.starts_with("Review request ") || !first.contains(" for plan ") {
        return;
    }
    let mut lines: Vec<&str> = text
        .lines()
        .filter(|line| {
            !line.starts_with("Give one exhaustive findings list")
                && !line.starts_with("Write one finding per line")
                && !line.starts_with("A finding about owns, depends_on")
        })
        .collect();
    let Some(last) = lines.last_mut() else {
        return;
    };
    *last = "Review only that plan. Do not edit any file. Answer exactly once with cstan review pass \"<text>\" or cstan review findings \"<text>\". Findings must say what is wrong and in which package.";
    let legacy = lines.join("\n");
    values[hash] = Value::String(capstan_kernel::canonical::sha256(&legacy));
    values[body] = Value::String(legacy);
}

/// Every table of the ledger that has rows, as the Node exporter dumps it (empty tables are left out).
pub fn dump_tables(file: &Path) -> Value {
    let conn =
        rusqlite::Connection::open_with_flags(file, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("the ledger opens read-only");
    let names: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut out = Map::new();
    for name in names {
        let columns: Vec<String> = conn
            .prepare(&format!("PRAGMA table_info({name})"))
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .map(Result::unwrap)
            .filter(|c| !(name == "schema_migrations" && c == "applied_at"))
            .filter(|c| !(name == "messages" && c == "class"))
            .filter(|c| !(name == "plans" && c == "max_review_rounds"))
            .filter(|c| {
                !(name == "plan_revisions"
                    && matches!(c.as_str(), "amendment_json" | "amends_review_id"))
            })
            .filter(|c| !(name == "reviews" && c == "findings_class"))
            .filter(|c| {
                !(name == "prompt_relays"
                    && matches!(c.as_str(), "auto_window_id" | "auto_begin_at"))
            })
            .collect();
        let order: Vec<String> = (1..=columns.len()).map(|i| i.to_string()).collect();
        // The controller's prompt:relay grant (migration 0047) is Rust-only, like the later migrations.
        let only_node = match name.as_str() {
            "schema_migrations" => format!(" WHERE version <= {NODE_LAST_MIGRATION}"),
            "role_capabilities" => " WHERE NOT (role = 'controller' AND capability = 'prompt:relay')".to_string(),
            "capability_grants" => " WHERE NOT (capability = 'prompt:relay' AND actor_id IN (SELECT actor_id FROM actors WHERE role = 'controller'))".to_string(),
            _ => String::new(),
        };
        let sql = format!(
            "SELECT {} FROM {name}{only_node} ORDER BY {}",
            columns.join(", "),
            order.join(", ")
        );
        let mut statement = conn.prepare(&sql).unwrap();
        let rows: Vec<Value> = statement
            .query_map([], |row| {
                let mut values = Vec::new();
                for (i, column) in columns.iter().enumerate() {
                    values.push(match row.get_ref(i)? {
                        ValueRef::Null => Value::Null,
                        ValueRef::Integer(n) => Value::from(n),
                        ValueRef::Real(f) => capstan_kernel::plan_body::number_value(f),
                        ValueRef::Text(t) => {
                            let text = String::from_utf8_lossy(t).into_owned();
                            // A stored result holds message records, which carry the Rust-only `class`.
                            Value::String(if column == "result_json" {
                                text.replace("\"class\":\"action\",", "")
                                    .replace("\"class\":\"info\",", "")
                                    .replace("\"maxReviewRounds\":null,", "")
                                    .replace("\"design\":false,\"designDecision\":null,", "")
                                    .replace("\"amendment\":null,", "")
                                    .replace("\"amendsReviewId\":null,", "")
                            } else {
                                text
                            })
                        }
                        ValueRef::Blob(b) => json!({"$blob": capstan_kernel::canonical::hex(b)}),
                    });
                }
                if name == "messages" {
                    legacy_plan_brief(&columns, &mut values);
                }
                Ok(Value::Array(values))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        // Empty tables are left out, which is all it takes for a Rust-only table to vanish from a dump until it has rows.
        if !rows.is_empty() {
            out.insert(name, json!({"columns": columns, "rows": rows}));
        }
    }
    Value::Object(out)
}

/// What changed in `current` against `baseline` (both as `dump_tables` returns them), the way the exporter records it:
/// per table the rows only in `current` (`added`) and the rows only in the baseline (`removed`), each in dump order;
/// `columns` where the baseline has no such table or other columns; unchanged tables are left out.
pub fn diff_tables(baseline: &Value, current: &Value) -> Value {
    let mut names: Vec<&String> = baseline
        .as_object()
        .into_iter()
        .chain(current.as_object())
        .flat_map(|tables| tables.keys())
        .collect();
    names.sort();
    names.dedup();
    let rows_of = |tables: &Value, name: &str| -> Vec<Value> {
        tables[name]["rows"].as_array().cloned().unwrap_or_default()
    };
    let mut out = Map::new();
    for name in names {
        let was = rows_of(baseline, name);
        let now = rows_of(current, name);
        let mut count: BTreeMap<String, usize> = BTreeMap::new();
        for row in &was {
            *count.entry(row.to_string()).or_default() += 1;
        }
        let mut added = Vec::new();
        for row in &now {
            match count.get_mut(&row.to_string()) {
                Some(left) if *left > 0 => *left -= 1,
                _ => added.push(row.clone()),
            }
        }
        let mut removed = Vec::new();
        for row in &was {
            if let Some(left) = count.get_mut(&row.to_string()) {
                if *left > 0 {
                    *left -= 1;
                    removed.push(row.clone());
                }
            }
        }
        let columns_changed =
            current.get(name).is_some() && baseline[name]["columns"] != current[name]["columns"];
        if added.is_empty() && removed.is_empty() && !columns_changed {
            continue;
        }
        let mut entry = Map::new();
        if columns_changed {
            entry.insert("columns".into(), current[name]["columns"].clone());
        }
        if !added.is_empty() {
            entry.insert("added".into(), Value::Array(added));
        }
        if !removed.is_empty() {
            entry.insert("removed".into(), Value::Array(removed));
        }
        out.insert(name.clone(), Value::Object(entry));
    }
    Value::Object(out)
}

/// The first place two table diffs (see `diff_tables`) differ, in the terms of the ledger: the table, the list, the row
/// (its first column) and the column, with both values decoded; the fixture's dictionary is long expanded by then.
pub fn first_tables_difference(
    expected: &Value,
    actual: &Value,
    baseline: &Value,
    label: &str,
) -> Option<String> {
    if expected == actual {
        return None;
    }
    let tables = |v: &Value| -> Vec<String> {
        v.as_object()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    };
    let mut names = tables(expected);
    names.extend(tables(actual));
    names.sort();
    names.dedup();
    let empty = Vec::new();
    for name in names {
        let (e, a) = (&expected[&name], &actual[&name]);
        if e == a {
            continue;
        }
        let columns_of = |v: &Value| -> Option<Vec<String>> {
            v["columns"]
                .as_array()
                .or(baseline[&name]["columns"].as_array())
                .map(|c| {
                    c.iter()
                        .map(|x| x.as_str().unwrap_or("?").to_string())
                        .collect()
                })
        };
        let (expected_columns, actual_columns) = (columns_of(e), columns_of(a));
        if expected_columns != actual_columns {
            return Some(format!(
                "{label}: table {name}: expected columns {expected_columns:?} but got {actual_columns:?}"
            ));
        }
        let columns = expected_columns.unwrap_or_default();
        for list in ["added", "removed"] {
            let (x, y) = (
                e[list].as_array().unwrap_or(&empty),
                a[list].as_array().unwrap_or(&empty),
            );
            for i in 0..x.len().max(y.len()) {
                let here = format!("{label}: table {name}, {list} row {i}");
                match (x.get(i), y.get(i)) {
                    (Some(p), Some(q)) if p == q => {}
                    (Some(p), Some(q)) if p[0] == q[0] => {
                        let column = (0..p.as_array().map_or(0, Vec::len))
                            .find(|&j| p[j] != q[j])
                            .unwrap_or(0);
                        let key = columns.first().map_or("?", String::as_str);
                        return Some(format!(
                            "{here} ({key} = {}): column {}: Node has {} but Rust has {}",
                            clip(&p[0]),
                            columns.get(column).map_or("?", String::as_str),
                            clip(&p[column]),
                            clip(&q[column])
                        ));
                    }
                    (Some(p), Some(q)) => {
                        return Some(format!(
                            "{here}: Node has the row {} but Rust has the row {} (a row is missing or extra)",
                            clip(p),
                            clip(q)
                        ))
                    }
                    (Some(p), None) => {
                        return Some(format!("{here}: the row {} is missing in Rust", clip(p)))
                    }
                    (None, Some(q)) => {
                        return Some(format!("{here}: Rust has the extra row {}", clip(q)))
                    }
                    (None, None) => {}
                }
            }
        }
    }
    first_difference(expected, actual, label)
}

/// The full baseline ledger of the parity export: `baseline` of the group file `core.json` in `dir`, else of the
/// committed parity directory.
pub fn baseline_of(dir: &Path) -> Value {
    let own = dir.join("core.json");
    let file = if own.exists() {
        own
    } else {
        super::parity_dir().join("core.json")
    };
    let core = super::read_json(&file);
    assert!(
        core["baseline"].is_object(),
        "{} has no baseline",
        file.display()
    );
    core["baseline"].clone()
}

/// What a step did, in the shape the exporter records it: `result`, or `error` and `message`.
fn outcome_of(result: &Result<Value, KernelError>) -> Value {
    match result {
        Ok(value) => json!({"result": value}),
        Err(error) => json!({"error": error.name(), "message": error.message()}),
    }
}

/// The outcome fields of a recorded step.
fn recorded_outcome(step: &Value) -> Value {
    match step.get("error") {
        Some(error) => json!({"error": error, "message": step["message"]}),
        None => json!({"result": step["result"]}),
    }
}

enum StepFault {
    Pending(String),
    Failed(String),
}

struct Run {
    directory: PathBuf,
    handles: BTreeMap<String, Core>,
    env: Rc<SeededEnv>,
    requires: bool,
    /// The full ledger every recorded table diff is against.
    baseline: Value,
}

impl Run {
    /// The outcome of a step against what Node recorded: equal, pending on an unported area, or a failure.
    fn compare(&self, step: &Value, actual: Result<Value, KernelError>) -> Result<(), StepFault> {
        if let Err(error) = &actual {
            if error.is_unported() {
                return if self.requires {
                    Err(StepFault::Pending(format!(
                        "{} reached {}",
                        step_label(step),
                        error.message()
                    )))
                } else {
                    Err(StepFault::Failed(format!(
                        "{} reached unported code ({}) but the sequence does not require it",
                        step_label(step),
                        error.message()
                    )))
                };
            }
        }
        let actual = outcome_of(&actual);
        match first_difference(&recorded_outcome(step), &actual, "outcome") {
            None => Ok(()),
            Some(diff) => Err(StepFault::Failed(format!(
                "{} of step {}: {diff}",
                step_label(step),
                clip(&step["args"])
            ))),
        }
    }

    fn open(&mut self, step: &Value, read_only: bool) -> Result<Value, KernelError> {
        let on = step["on"].as_str().unwrap_or("main").to_string();
        let spec = &step["args"][0];
        let state_dir = spec["stateDir"].as_str().unwrap_or("dir");
        let project: InitialProject = serde_json::from_value(spec["project"].clone())
            .map_err(|e| KernelError::Other(format!("project: {e}")))?;
        let path = if state_dir == "dir" {
            self.directory.clone()
        } else {
            PathBuf::from(state_dir)
        };
        if state_dir == "dir" && !read_only {
            // The ledger's migration timestamps come from the system clock; migrating first keeps them out of the seeded clock.
            open_database(&path.join("controller.sqlite"), &OpenOptions::default())?.close()?;
        }
        let options = KernelOptions {
            workspace_root: spec["options"]["workspaceRoot"].as_str().map(PathBuf::from),
            runtime_workspace_path: spec["options"]["runtimeWorkspacePath"]
                .as_str()
                .map(PathBuf::from),
            keep_migration_backups: None,
        };
        let env = Box::new(SharedEnv(self.env.clone()));
        let core = if read_only {
            Core::open_read_only(&path, &project, &options, env)?
        } else {
            Core::open(&path, &project, &options, env)?
        };
        // The corpus predates the finding line format of plan reviews.
        core.allow_free_text_plan_findings(true);
        self.handles.insert(on, core);
        Ok(Value::Null)
    }

    fn step(&mut self, step: &Value) -> Result<(), StepFault> {
        let on = step["on"].as_str().unwrap_or("main").to_string();
        let op = step["op"].as_str().unwrap_or("");
        if is_unresolved_reference(step) {
            // The exporter could not resolve a `$name.path` of the step (the earlier step it names failed or has no such
            // field): the step never reached the controller, so there is nothing to run, only the counters to check.
            self.check_counters(step, &on)?;
            return self.check_tables(step);
        }
        let result = match op {
            "open" => self.open(step, false),
            "openReadOnly" => self.open(step, true),
            "dump" => Ok(Value::Null),
            _ => match self.handles.get(&on) {
                Some(core) => {
                    let mut args: Vec<Value> = Vec::new();
                    if let Some(context) = step.get("context") {
                        args.push(context.clone());
                    }
                    args.extend(step["args"].as_array().cloned().unwrap_or_default());
                    core.dispatch(op, &args)
                }
                None => Err(KernelError::Other(format!("no handle {on}"))),
            },
        };
        self.compare(step, result)?;
        self.check_counters(step, &on)?;
        self.check_tables(step)
    }

    fn check_counters(&self, step: &Value, on: &str) -> Result<(), StepFault> {
        if step.get("stateVersion").is_some() {
            let core = self.handles.get(on).ok_or_else(|| {
                StepFault::Failed(format!(
                    "{}: no handle {on} for the counters",
                    step_label(step)
                ))
            })?;
            let actual = json!({
                "stateVersion": core.state_version().map_err(|e| StepFault::Failed(e.to_string()))?,
                "inputRevision": core.input_revision().map_err(|e| StepFault::Failed(e.to_string()))?,
            });
            let expected = json!({"stateVersion": step["stateVersion"], "inputRevision": step["inputRevision"]});
            if let Some(diff) = first_difference(&expected, &actual, "counters") {
                return Err(StepFault::Failed(format!("{}: {diff}", step_label(step))));
            }
        }
        Ok(())
    }

    fn check_tables(&self, step: &Value) -> Result<(), StepFault> {
        if let Some(expected) = step.get("tablesDiff") {
            let actual = diff_tables(
                &self.baseline,
                &dump_tables(&self.directory.join("controller.sqlite")),
            );
            if let Some(diff) = first_tables_difference(expected, &actual, &self.baseline, "tables")
            {
                return Err(StepFault::Failed(format!("{}: {diff}", step_label(step))));
            }
        }
        Ok(())
    }
}

/// A step the exporter failed before it called the controller, because a reference of its arguments did not resolve
/// (`unknown reference $name` or `$name.path does not resolve`): it records no `args`.
fn is_unresolved_reference(step: &Value) -> bool {
    step.get("args").is_none()
        && step["error"] == "Error"
        && step["message"].as_str().is_some_and(|m| {
            m.starts_with("unknown reference ") || m.ends_with(" does not resolve")
        })
}

fn step_label(step: &Value) -> String {
    step["op"].as_str().unwrap_or("step").to_string()
}

/// Replays one sequence of a group file.
pub fn replay_sequence(sequence: &Value, baseline: &Value) -> Status {
    let seed = sequence["seed"].as_str().unwrap_or("");
    let directory = super::private_tempdir();
    let mut run = Run {
        directory: directory.path().to_path_buf(),
        handles: BTreeMap::new(),
        env: Rc::new(SeededEnv::new(seed)),
        requires: sequence["requires"]
            .as_array()
            .is_some_and(|r| !r.is_empty()),
        baseline: baseline.clone(),
    };
    let mut status = Status::Passed;
    for (index, step) in sequence["steps"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .enumerate()
    {
        match run.step(step) {
            Ok(()) => {}
            Err(StepFault::Pending(detail)) => {
                status = Status::Pending(format!("step {index}: {detail}"));
                break;
            }
            Err(StepFault::Failed(detail)) => {
                status = Status::Failed(format!("step {index}: {detail}"));
                break;
            }
        }
    }
    if status == Status::Passed {
        let actual = diff_tables(
            baseline,
            &dump_tables(&directory.path().join("controller.sqlite")),
        );
        if let Some(diff) =
            first_tables_difference(&sequence["tablesDiff"], &actual, baseline, "tables")
        {
            status = Status::Failed(format!("final dump: {diff}"));
        }
    }
    for core in run.handles.values() {
        core.close();
    }
    run.handles.clear();
    status
}

/// Replays every `*.json` group file of `dir`, in name order.
pub fn replay_directory(dir: &Path) -> Vec<Report> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    let baseline = baseline_of(dir);
    let mut reports = Vec::new();
    for file in files {
        let group = super::read_json(&file);
        let name = group["group"].as_str().unwrap_or("?").to_string();
        for sequence in group["sequences"].as_array().cloned().unwrap_or_default() {
            reports.push(Report {
                group: name.clone(),
                sequence: sequence["name"].as_str().unwrap_or("?").to_string(),
                status: replay_sequence(&sequence, &baseline),
            });
        }
    }
    reports
}
