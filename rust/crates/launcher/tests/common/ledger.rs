//! The ledger side of a replay: the kernel thread the launcher reaches through a `LedgerPort`, and the table dumps and
//! differences the fixtures record (the Rust half of `dumpTables` and `diffTables` of test/kernel-parity-export.ts).

use capstan_kernel::types::{InitialProject, MutationContext};
use capstan_kernel::{Core, KernelOptions, KernelResult, SeededEnv};
use capstan_launcher::kernel::{KernelGone, LedgerPort};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::mpsc::{channel, Sender};
use std::sync::Mutex;
use std::thread::JoinHandle;

type Job = Box<dyn FnOnce(&Core) + Send>;

enum Message {
    Run(Job),
    Close,
}

/// A thread that owns the `Core` (it opens it there, with a seeded clock and randomness) and runs the closures it is sent.
pub struct KernelThread {
    sender: Mutex<Option<Sender<Message>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl KernelThread {
    pub fn spawn(state_dir: &Path, project: &Value, seed: &str) -> Result<KernelThread, String> {
        let state_dir = state_dir.to_path_buf();
        let project: InitialProject =
            serde_json::from_value(project.clone()).map_err(|e| format!("project: {e}"))?;
        let seed = seed.to_string();
        let (sender, receiver) = channel::<Message>();
        let (opened_tx, opened_rx) = channel::<Result<(), String>>();
        let thread = std::thread::Builder::new()
            .name("parity-kernel".into())
            .spawn(move || {
                let opened = Core::open(
                    &state_dir,
                    &project,
                    &KernelOptions::default(),
                    Box::new(SeededEnv::new(&seed)),
                );
                let core = match opened {
                    Ok(core) => {
                        let _ = opened_tx.send(Ok(()));
                        core
                    }
                    Err(error) => {
                        let _ = opened_tx.send(Err(error.to_string()));
                        return;
                    }
                };
                while let Ok(message) = receiver.recv() {
                    match message {
                        Message::Run(job) => job(&core),
                        Message::Close => break,
                    }
                }
                core.close();
            })
            .map_err(|e| e.to_string())?;
        match opened_rx.recv() {
            Ok(Ok(())) => Ok(KernelThread {
                sender: Mutex::new(Some(sender)),
                thread: Mutex::new(Some(thread)),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => Err("the kernel thread ended while opening".into()),
        }
    }

    /// Runs `f` on the kernel thread and waits for its answer.
    pub fn call<R: Send + 'static>(
        &self,
        f: impl FnOnce(&Core) -> R + Send + 'static,
    ) -> Result<R, KernelGone> {
        let (answer_tx, answer_rx) = channel();
        self.run_job(Box::new(move |core| {
            let _ = answer_tx.send(f(core));
        }))?;
        answer_rx.recv().map_err(|_| KernelGone)
    }

    pub fn close(&self) {
        if let Some(sender) = self.sender.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = sender.send(Message::Close);
        }
        if let Some(thread) = self.thread.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = thread.join();
        }
    }
}

impl LedgerPort for KernelThread {
    fn run_job(&self, job: Box<dyn FnOnce(&Core) + Send>) -> Result<(), KernelGone> {
        let sender = self.sender.lock().unwrap_or_else(|p| p.into_inner());
        match sender.as_ref() {
            Some(sender) => sender.send(Message::Run(job)).map_err(|_| KernelGone),
            None => Err(KernelGone),
        }
    }
}

impl Drop for KernelThread {
    fn drop(&mut self) {
        self.close();
    }
}

/// `newContext` of src/context.ts for a credential: one uuid of the kernel's randomness, the ledger's counters.
pub fn context(core: &Core, credential: &str) -> KernelResult<MutationContext> {
    let id = core.kernel().env.uuid();
    Ok(MutationContext {
        credential: credential.to_string(),
        request_id: format!("req-{id}"),
        idempotency_key: format!("idem-{id}"),
        expected_version: core.state_version()?,
        input_revision: core.input_revision()?,
    })
}

/// A number the way JavaScript prints it: an integral float is an integer.
fn js_number(value: Value) -> Value {
    match value {
        Value::Number(n) => match n.as_f64() {
            Some(f) if n.is_f64() && f.fract() == 0.0 && f.abs() < 9.007_199_254_740_992e15 => {
                Value::from(f as i64)
            }
            _ => Value::Number(n),
        },
        Value::Array(items) => Value::Array(items.into_iter().map(js_number).collect()),
        other => other,
    }
}

/// Every table of the ledger that has rows (empty tables are left out), each row's values in column order, rows ordered by
/// all columns.
pub fn dump_tables(core: &Core) -> Value {
    let connection = core.kernel().database.connection();
    let names: Vec<String> = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut out = Map::new();
    for name in names {
        let columns: Vec<String> = connection
            .prepare(&format!("PRAGMA table_info({name})"))
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .map(Result::unwrap)
            .filter(|c| !(name == "schema_migrations" && c == "applied_at"))
            .collect();
        let sql = format!(
            "SELECT json_array({0}) FROM {name} ORDER BY {0}",
            columns.join(", ")
        );
        let rows: Vec<Value> = connection
            .prepare(&sql)
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|text| js_number(serde_json::from_str(&text.unwrap()).unwrap()))
            .collect();
        // A request hash covers the arguments of a mutation, and those name the scratch directory, which differs from
        // one run to the next.
        let rows: Vec<Value> = match (
            name.as_str(),
            columns.iter().position(|c| c == "request_hash"),
        ) {
            ("mutation_requests", Some(column)) => rows
                .into_iter()
                .map(|mut row| {
                    row[column] = json!("<hash>");
                    row
                })
                .collect(),
            _ => rows,
        };
        if !rows.is_empty() {
            out.insert(name, json!({"columns": columns, "rows": rows}));
        }
    }
    Value::Object(out)
}

/// What changed in `current` against `baseline` (both as `dump_tables` returns them), the way the exporter records it: per
/// table the rows only in `current` (`added`) and the rows only in the baseline (`removed`), each in dump order; `columns`
/// where the baseline has no such table or other columns; unchanged tables are left out.
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
