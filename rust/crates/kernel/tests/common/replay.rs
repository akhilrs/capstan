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

/// The first place two JSON values differ, with both sides.
pub fn first_difference(expected: &Value, actual: &Value, path: &str) -> Option<String> {
    if expected == actual {
        return None;
    }
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => {
            for key in e.keys().chain(a.keys()) {
                let here = format!("{path}.{key}");
                match (e.get(key), a.get(key)) {
                    (Some(x), Some(y)) => {
                        if let Some(diff) = first_difference(x, y, &here) {
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
                        if let Some(diff) = first_difference(x, y, &here) {
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

/// Every table of the ledger as the Node exporter dumps it.
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
            .collect();
        let order: Vec<String> = (1..=columns.len()).map(|i| i.to_string()).collect();
        let sql = format!(
            "SELECT {} FROM {name} ORDER BY {}",
            columns.join(", "),
            order.join(", ")
        );
        let mut statement = conn.prepare(&sql).unwrap();
        let rows: Vec<Value> = statement
            .query_map([], |row| {
                let mut values = Vec::new();
                for i in 0..columns.len() {
                    values.push(match row.get_ref(i)? {
                        ValueRef::Null => Value::Null,
                        ValueRef::Integer(n) => Value::from(n),
                        ValueRef::Real(f) => capstan_kernel::plan_body::number_value(f),
                        ValueRef::Text(t) => Value::String(String::from_utf8_lossy(t).into_owned()),
                        ValueRef::Blob(b) => json!({"$blob": capstan_kernel::canonical::hex(b)}),
                    });
                }
                Ok(Value::Array(values))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        out.insert(name, json!({"columns": columns, "rows": rows}));
    }
    Value::Object(out)
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
        self.handles.insert(on, core);
        Ok(Value::Null)
    }

    fn step(&mut self, step: &Value) -> Result<(), StepFault> {
        let on = step["on"].as_str().unwrap_or("main").to_string();
        let op = step["op"].as_str().unwrap_or("");
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
        if step.get("stateVersion").is_some() {
            let core = self.handles.get(&on).ok_or_else(|| {
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
        if let Some(tables) = step.get("tables") {
            let actual = dump_tables(&self.directory.join("controller.sqlite"));
            if let Some(diff) = first_difference(tables, &actual, "tables") {
                return Err(StepFault::Failed(format!("{}: {diff}", step_label(step))));
            }
        }
        Ok(())
    }
}

fn step_label(step: &Value) -> String {
    step["op"].as_str().unwrap_or("step").to_string()
}

/// Replays one sequence of a group file.
pub fn replay_sequence(sequence: &Value) -> Status {
    let seed = sequence["seed"].as_str().unwrap_or("");
    let directory = super::private_tempdir();
    let mut run = Run {
        directory: directory.path().to_path_buf(),
        handles: BTreeMap::new(),
        env: Rc::new(SeededEnv::new(seed)),
        requires: sequence["requires"]
            .as_array()
            .is_some_and(|r| !r.is_empty()),
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
        let actual = dump_tables(&directory.path().join("controller.sqlite"));
        if let Some(diff) = first_difference(&sequence["tables"], &actual, "tables") {
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
    let mut reports = Vec::new();
    for file in files {
        let group = super::read_json(&file);
        let name = group["group"].as_str().unwrap_or("?").to_string();
        for sequence in group["sequences"].as_array().cloned().unwrap_or_default() {
            reports.push(Report {
                group: name.clone(),
                sequence: sequence["name"].as_str().unwrap_or("?").to_string(),
                status: replay_sequence(&sequence),
            });
        }
    }
    reports
}
