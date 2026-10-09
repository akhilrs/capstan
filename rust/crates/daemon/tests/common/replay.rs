//! The replay engine of the wire transcripts (tests/transcripts/*.json, written by test/daemon-transcript-export.ts from the
//! real Node daemon).
//!
//! A scenario is replayed in one of two ways:
//!
//!  - **handler mode** (the default): in process. The scratch project and repository are rebuilt, the `setup` steps are
//!    applied to the ledger by the kernel with the seeded clock of the Node export, then the daemon's kernel is opened with
//!    the seeded environment of the daemon run and every request frame of the `dispatch` layer goes through
//!    `handlers::handle_frame`: the same code a connection runs. The response line, the log entry (compared without its
//!    `ts` and `ms`; the clock readings they stand for are made all the same) and the ledger's table diffs are compared.
//!    A step of the `socket` layer (a frame beyond the limit, a partial frame, a held connection, a concurrent request) is
//!    pending: it needs the server of d2b-srv.
//!  - **socket mode** (`"mode": "socket"` in the scenario): the real `cstan-daemon` is started in the scratch project and
//!    spoken to over its socket; only the responses are compared (the binary reads the system clock). While the server is a
//!    stub (it logs `serve_not_implemented`) every step is pending.
//!
//! A step is pending too when its route answers with the stub of an unported handler (`shared::is_unported`). Strict mode
//! (`CAPSTAN_DAEMON_PARITY_STRICT=1`, `common::strict`) makes any pending step a failure.

use super::process::Daemon;
use super::project::{build_repo, write_project, Identity};
use super::tables::{describe_tables_difference, diff_tables, dump_tables};
use super::{private_tempdir, read_transcript, seeded_env};
use capstan_daemon::deps::{startup_sequence, DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::handlers::handle_frame;
use capstan_daemon::handlers::shared::{is_unported, AbortSignal, CommandResponse, ErrorCode};
use capstan_daemon::handlers::MAX_RESPONSE_BYTES;
use capstan_daemon::run::SERVE_STUB_EVENT;
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::Core;
use regex::Regex;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Passed,
    Pending(String),
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct StepReport {
    pub index: usize,
    pub label: String,
    pub layer: String,
    pub status: Status,
}

#[derive(Clone, Debug)]
pub struct Report {
    pub group: String,
    pub name: String,
    pub status: Status,
    pub steps: Vec<StepReport>,
}

impl Report {
    pub fn count(&self, wanted: fn(&Status) -> bool) -> usize {
        self.steps.iter().filter(|s| wanted(&s.status)).count()
    }

    pub fn line(&self) -> String {
        let steps = format!(
            "{} passed, {} pending, {} failed of {} steps",
            self.count(|s| *s == Status::Passed),
            self.count(|s| matches!(s, Status::Pending(_))),
            self.count(|s| matches!(s, Status::Failed(_))),
            self.steps.len()
        );
        match &self.status {
            Status::Passed => format!("{}/{}: passed ({steps})", self.group, self.name),
            Status::Pending(why) => {
                format!("{}/{}: pending ({steps}): {why}", self.group, self.name)
            }
            Status::Failed(why) => format!("{}/{}: FAILED ({steps}): {why}", self.group, self.name),
        }
    }
}

/// The report lines that fail the run: every failed scenario, and in strict mode every pending one as well.
pub fn failing(reports: &[Report], strict: bool) -> Vec<String> {
    reports
        .iter()
        .filter(|report| match report.status {
            Status::Passed => false,
            Status::Pending(_) => strict,
            Status::Failed(_) => true,
        })
        .map(Report::line)
        .collect()
}

// ------------------------------------------------------------------------------------------------ frames and responses

/// The bytes a frame spec stands for: a string is its UTF-8 text, `{base64}` raw bytes, `{repeat}` a generated frame.
pub fn frame_bytes(spec: &Value) -> Vec<u8> {
    if let Some(text) = spec.as_str() {
        return text.as_bytes().to_vec();
    }
    if let Some(encoded) = spec["base64"].as_str() {
        return decode_base64(encoded);
    }
    let repeat = &spec["repeat"];
    assert!(repeat.is_object(), "an unknown frame spec: {spec}");
    let text = |key: &str| repeat[key].as_str().unwrap_or("").to_string();
    let times = repeat["times"].as_u64().expect("repeat.times") as usize;
    format!(
        "{}{}{}",
        text("prefix"),
        text("text").repeat(times),
        text("suffix")
    )
    .into_bytes()
}

fn decode_base64(text: &str) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .expect("a base64 frame")
}

static PID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""pid":\d+"#).expect("a pattern"));

/// What the export redacts: the scratch project's path and the process id.
pub fn redact(text: &str, paths: &[String]) -> String {
    let mut out = text.to_string();
    for path in paths {
        out = out.replace(path.as_str(), "<project>");
    }
    PID.replace_all(&out, r#""pid":0"#).into_owned()
}

fn clip(text: &str) -> String {
    let flat: String = text.chars().take(400).collect();
    if text.chars().count() > 400 {
        format!("{flat}...")
    } else {
        flat
    }
}

/// Two response lines are the same when both are JSON and equal (the order of members is not compared: clients parse
/// them), or both are the same text.
fn same_response(expected: &str, actual: &str) -> bool {
    match (
        serde_json::from_str::<Value>(expected),
        serde_json::from_str::<Value>(actual),
    ) {
        (Ok(e), Ok(a)) => e == a,
        _ => expected == actual,
    }
}

/// A log line without the time it was written and the milliseconds it took.
fn log_entry(line: &str) -> Value {
    match serde_json::from_str::<Value>(line) {
        Ok(Value::Object(mut map)) => {
            map.remove("ts");
            map.remove("ms");
            Value::Object(map)
        }
        _ => Value::String(line.to_string()),
    }
}

fn unported_response(line: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return false;
    };
    let message = value["message"].as_str().unwrap_or("").to_string();
    is_unported(&CommandResponse::Err {
        code: if value["code"] == ErrorCode::NotImplemented.as_str() {
            ErrorCode::NotImplemented
        } else {
            ErrorCode::Error
        },
        message,
    })
}

fn paths_of(directory: &Path) -> Vec<String> {
    let mut paths = vec![directory.to_string_lossy().into_owned()];
    if let Ok(real) = std::fs::canonicalize(directory) {
        let real = real.to_string_lossy().into_owned();
        if !paths.contains(&real) {
            paths.push(real);
        }
    }
    paths
}

// ------------------------------------------------------------------------------------------------ setup

/// Applies the `setup` steps to the ledger as the Node export did (a controller opened on the project with the seed
/// `<seed>/setup`, each step run by its `ControllerCore` method name), comparing every result with the recorded one.
fn apply_setup(
    state: &Path,
    directory: &Path,
    identity: &Identity,
    seed: &str,
    steps: &Value,
) -> Result<(), String> {
    let core = Core::open(
        state,
        &identity.initial_project(),
        &KernelOptions {
            workspace_root: Some(directory.to_path_buf()),
            ..KernelOptions::default()
        },
        seeded_env(&format!("{seed}/setup")),
    )
    .map_err(|e| format!("setup: the controller does not open: {e}"))?;
    let outcome = (|| {
        for (index, step) in steps.as_array().into_iter().flatten().enumerate() {
            let op = step["op"].as_str().ok_or("a setup step has no op")?;
            let mut args: Vec<Value> = Vec::new();
            if step.get("context").is_some() {
                args.push(step["context"].clone());
            }
            args.extend(step["args"].as_array().cloned().unwrap_or_default());
            let got = core.dispatch(op, &args);
            match (&got, step.get("error")) {
                (Ok(value), None) => {
                    let expected = step.get("result").cloned().unwrap_or(Value::Null);
                    // The export keeps a result as JSON text; compare as JSON values.
                    let actual = serde_json::from_str::<Value>(&value.to_string()).unwrap();
                    if actual != expected {
                        return Err(format!(
                            "setup step {index} ({op}): expected {} but got {}",
                            clip(&expected.to_string()),
                            clip(&actual.to_string())
                        ));
                    }
                }
                (Err(error), Some(name)) => {
                    let message = step["message"].as_str().unwrap_or("");
                    if error.name() != name.as_str().unwrap_or("") || error.message() != message {
                        return Err(format!(
                            "setup step {index} ({op}): expected {name}: {message} but got {}: {}",
                            error.name(),
                            error.message()
                        ));
                    }
                }
                (Ok(_), Some(name)) => {
                    return Err(format!(
                        "setup step {index} ({op}): expected the error {name}"
                    ))
                }
                (Err(error), None) => {
                    return Err(format!("setup step {index} ({op}): unexpected {error}"))
                }
            }
        }
        Ok(())
    })();
    core.close();
    outcome
}

// ------------------------------------------------------------------------------------------------ the scenario

struct Scratch {
    dir: tempfile::TempDir,
    state: PathBuf,
    identity: Identity,
}

fn prepare(scenario: &Value) -> Result<Scratch, String> {
    let dir = private_tempdir();
    let identity = Identity::of(&scenario["project"]);
    let state = write_project(dir.path(), &identity);
    if scenario.get("repo").is_some() {
        let heads = build_repo(dir.path(), &scenario["repo"]);
        let expected = scenario["repo"]["heads"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        for (branch, sha) in &expected {
            if heads.get(branch).map(String::as_str) != sha.as_str() {
                return Err(format!(
                    "the repository does not rebuild to the same commits: {branch} is {:?}, the export has {sha}",
                    heads.get(branch)
                ));
            }
        }
    }
    // The ledger's migration timestamps come from the system clock in both implementations; migrating first keeps them out
    // of the seeded clock.
    let database = capstan_ledger::resolve_database_path(&state)
        .map_err(|e| format!("the ledger path: {e}"))?;
    capstan_ledger::open_database(&database, &capstan_ledger::OpenOptions::default())
        .map_err(|e| format!("the ledger does not migrate: {e}"))?;
    if scenario.get("setup").is_some() {
        apply_setup(
            &state,
            dir.path(),
            &identity,
            scenario["seed"].as_str().unwrap_or(""),
            &scenario["setup"],
        )?;
    }
    Ok(Scratch {
        dir,
        state,
        identity,
    })
}

fn label_of(step: &Value, index: usize) -> String {
    step["label"]
        .as_str()
        .map(str::to_string)
        .or_else(|| step["command"].as_str().map(str::to_string))
        .unwrap_or_else(|| format!("request {index}"))
}

/// Replays one scenario of a group file (`baseline` is the group's full baseline ledger).
pub fn replay_scenario(group: &str, baseline: &Value, scenario: &Value) -> Report {
    let name = scenario["name"].as_str().unwrap_or("?").to_string();
    let mut report = Report {
        group: group.to_string(),
        name,
        status: Status::Passed,
        steps: Vec::new(),
    };
    if scenario["mode"] == "socket" {
        let indices: Vec<usize> = (0..scenario["steps"].as_array().map_or(0, Vec::len)).collect();
        over_socket(&mut report, scenario, &indices, true);
        return report;
    }
    let scratch = match prepare(scenario) {
        Ok(scratch) => scratch,
        Err(message) => {
            report.status = Status::Failed(message);
            return report;
        }
    };
    handler_mode(&mut report, baseline, scenario, &scratch);
    drop(scratch);
    // The connection-level steps run against a real daemon of their own: they must not depend on what earlier steps did.
    let socket_steps: Vec<usize> = report
        .steps
        .iter()
        .filter(|s| matches!(s.status, Status::Pending(_)) && s.layer == "socket")
        .map(|s| s.index)
        .collect();
    if !socket_steps.is_empty() && !matches!(report.status, Status::Failed(_)) {
        over_socket(&mut report, scenario, &socket_steps, false);
    }
    report
}

fn finish(report: &mut Report, extra: Option<String>) {
    let failed = report
        .steps
        .iter()
        .find_map(|s| match &s.status {
            Status::Failed(why) => Some(format!("step {} ({}): {why}", s.index, s.label)),
            _ => None,
        })
        .or(extra);
    report.status = if let Some(why) = failed {
        Status::Failed(why)
    } else if let Some(pending) = report.steps.iter().find_map(|s| match &s.status {
        Status::Pending(why) => Some(why.clone()),
        _ => None,
    }) {
        Status::Pending(pending)
    } else {
        Status::Passed
    };
}

fn handler_mode(report: &mut Report, baseline: &Value, scenario: &Value, scratch: &Scratch) {
    let seed = scenario["seed"].as_str().unwrap_or("").to_string();
    let database = capstan_ledger::resolve_database_path(&scratch.state).expect("the ledger path");
    let kernel = {
        let (state, project, directory) = (
            scratch.state.clone(),
            scratch.identity.initial_project(),
            scratch.dir.path().to_path_buf(),
        );
        let env = seeded_env(&format!("{seed}/daemon"));
        match KernelHandle::spawn(move || {
            Core::open(
                &state,
                &project,
                &KernelOptions {
                    workspace_root: Some(directory),
                    ..KernelOptions::default()
                },
                env,
            )
        }) {
            Ok(kernel) => kernel,
            Err(error) => {
                report.status =
                    Status::Failed(format!("the daemon's kernel does not open: {error}"));
                return;
            }
        }
    };
    let lines: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink: Logger = {
        let (kernel, lines) = (kernel.clone(), Arc::clone(&lines));
        Arc::new(move |entry: &LogEntry| {
            let line = capstan_daemon::deps::render_log_line(&kernel, entry);
            lines.lock().unwrap().push(line);
        })
    };
    let deps = Deps::new(
        kernel.clone(),
        DaemonOptions::new(
            scratch.state.clone(),
            scratch.identity.initial_project(),
            scratch.dir.path().to_path_buf(),
        ),
        sink,
    );
    startup_sequence(&deps);
    let paths = paths_of(scratch.dir.path());
    let signal = AbortSignal::new();
    let mut pending_any = false;
    for (index, step) in scenario["steps"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let layer = step["layer"].as_str().unwrap_or("dispatch").to_string();
        let mut step_report = StepReport {
            index,
            label: label_of(step, index),
            layer: layer.clone(),
            status: Status::Passed,
        };
        if layer == "socket" || step["concurrent"] == true {
            step_report.status =
                Status::Pending("a connection-level step needs the server of d2b-srv".to_string());
            pending_any = true;
            report.steps.push(step_report);
            continue;
        }
        let bytes = frame_bytes(&step["frame"]);
        let outcome = handle_frame(&deps, &bytes, &signal, MAX_RESPONSE_BYTES);
        (deps.log)(&outcome.log);
        let expected = step["response"].as_str();
        step_report.status = match (expected, outcome.response.as_deref()) {
            (_, Some(actual)) if unported_response(actual) => {
                pending_any = true;
                Status::Pending(format!(
                    "{} is not ported yet",
                    step["command"].as_str().unwrap_or("the route")
                ))
            }
            (Some(want), Some(got)) => {
                let got = redact(got, &paths);
                if same_response(want, &got) {
                    Status::Passed
                } else {
                    Status::Failed(format!("expected {} but got {}", clip(want), clip(&got)))
                }
            }
            (None, None) => Status::Passed,
            (Some(want), None) => {
                Status::Failed(format!("expected {} but nothing was written", clip(want)))
            }
            (None, Some(got)) => Status::Failed(format!("expected nothing but got {}", clip(got))),
        };
        if step_report.status == Status::Passed && step.get("tablesDiff").is_some() {
            let now = diff_tables(baseline, &dump_tables(&database));
            if let Some(why) = describe_tables_difference(&step["tablesDiff"], &now) {
                step_report.status = Status::Failed(format!("the ledger differs: {why}"));
            }
        }
        report.steps.push(step_report);
    }
    // What the whole run wrote: only comparable when no step was left to another package.
    let mut extra = None;
    if !pending_any && report.steps.iter().all(|s| s.status == Status::Passed) {
        let expected: Vec<Value> = scenario["log"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|l| log_entry(&redact(l.as_str().unwrap_or(""), &paths)))
            .collect();
        let actual: Vec<Value> = lines
            .lock()
            .unwrap()
            .iter()
            .map(|l| log_entry(&redact(l, &paths)))
            .collect();
        if expected != actual {
            let at = expected
                .iter()
                .zip(&actual)
                .position(|(e, a)| e != a)
                .unwrap_or(expected.len().min(actual.len()));
            extra = Some(format!(
                "the daemon log differs at entry {at}: expected {} but got {} ({} entries expected, {} written)",
                expected.get(at).map_or("nothing".into(), |v| clip(&v.to_string())),
                actual.get(at).map_or("nothing".into(), |v| clip(&v.to_string())),
                expected.len(),
                actual.len()
            ));
        } else {
            let now = diff_tables(baseline, &dump_tables(&database));
            if let Some(why) = describe_tables_difference(&scenario["tablesDiff"], &now) {
                extra = Some(format!("the final ledger differs: {why}"));
            }
        }
    }
    kernel.close();
    finish(report, extra);
}

// ------------------------------------------------------------------------------------------------ socket mode

fn exchange(
    socket: &Path,
    bytes: &[u8],
    newline: bool,
    hold: Option<u64>,
) -> Result<(Option<String>, bool), String> {
    let mut stream = UnixStream::connect(socket).map_err(|e| format!("connect: {e}"))?;
    let wait = Duration::from_millis(hold.unwrap_or(20_000));
    stream.set_read_timeout(Some(wait)).ok();
    let _ = stream.write_all(bytes);
    if newline {
        let _ = stream.write_all(b"\n");
    }
    let mut data = Vec::new();
    let mut closed = true;
    let mut buffer = [0u8; 4096];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => data.extend_from_slice(&buffer[..n]),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                closed = false;
                break;
            }
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&data)
        .trim_end_matches('\n')
        .to_string();
    Ok(((!data.is_empty()).then_some(text), closed))
}

/// Runs the steps `indices` of a scenario against the real `cstan-daemon` in a fresh scratch project (the scenario's setup
/// applied), comparing the response and whether the connection closed. While the server is a stub every one is pending.
/// `whole` is a scenario that runs only this way: the exit code of the daemon is compared too.
fn over_socket(report: &mut Report, scenario: &Value, indices: &[usize], whole: bool) {
    let steps = scenario["steps"].as_array().cloned().unwrap_or_default();
    let step_report = |index: usize, status: Status| StepReport {
        index,
        label: label_of(&steps[index], index),
        layer: steps[index]["layer"]
            .as_str()
            .unwrap_or("dispatch")
            .to_string(),
        status,
    };
    let mut results: Vec<StepReport> = Vec::new();
    let mut extra = None;
    let scratch = match prepare(scenario) {
        Ok(scratch) => scratch,
        Err(message) => {
            report.status = Status::Failed(message);
            return;
        }
    };
    let mut daemon = Daemon::start(scratch.dir.path());
    let started = daemon.wait_for_line(Duration::from_secs(20), |line| {
        line.contains(SERVE_STUB_EVENT) || line.contains("\"event\":\"ready\"")
    });
    match started {
        None => {
            let exit = daemon.wait_exit(Duration::from_secs(1));
            report.status = Status::Failed(format!(
                "the daemon did not start (exit {exit:?}): {}",
                daemon.stderr()
            ));
            return;
        }
        Some(line) if line.contains(SERVE_STUB_EVENT) => {
            for &index in indices {
                results.push(step_report(
                    index,
                    Status::Pending("the server of d2b-srv is not ported yet".to_string()),
                ));
            }
            daemon.terminate();
        }
        Some(_) => {
            let socket = scratch.state.join("control.sock");
            let paths = paths_of(scratch.dir.path());
            for &index in indices {
                let step = &steps[index];
                let bytes = frame_bytes(&step["frame"]);
                let newline = step["newline"] != false;
                let hold = step["hold_ms"].as_u64();
                let status = match exchange(&socket, &bytes, newline, hold) {
                    Err(why) => Status::Failed(why),
                    Ok((response, closed)) => {
                        let expected = step["response"].as_str();
                        let got = response.as_deref().map(|text| redact(text, &paths));
                        let same = match (expected, got.as_deref()) {
                            (None, None) => true,
                            (Some(want), Some(got)) => same_response(want, got),
                            _ => false,
                        };
                        if same && closed == step["closed"].as_bool().unwrap_or(true) {
                            Status::Passed
                        } else {
                            Status::Failed(format!(
                                "expected {:?} (closed {}) but got {:?} (closed {closed})",
                                expected.map(clip),
                                step["closed"],
                                got.as_deref().map(clip)
                            ))
                        }
                    }
                };
                results.push(step_report(index, status));
            }
            let exit = daemon.terminate();
            if whole && exit != scenario["exit"].as_i64().map(|code| code as i32) {
                extra = Some(format!(
                    "the daemon exited with {exit:?}, the export has {}",
                    scenario["exit"]
                ));
            }
        }
    }
    for result in results {
        match report.steps.iter_mut().find(|s| s.index == result.index) {
            Some(slot) => *slot = result,
            None => report.steps.push(result),
        }
    }
    report.steps.sort_by_key(|s| s.index);
    finish(report, extra);
}

// ------------------------------------------------------------------------------------------------ groups

/// Replays every scenario of a transcript group (the document already unpacked).
pub fn replay_group(document: &Value) -> Vec<Report> {
    let group = document["group"].as_str().unwrap_or("?");
    document["scenarios"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|scenario| replay_scenario(group, &document["baseline"], scenario))
        .collect()
}

/// Replays every `*.json` file of a transcripts directory, in file order, found at run time.
pub fn replay_directory(directory: &Path) -> Vec<Report> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(directory)
        .unwrap_or_else(|e| panic!("{}: {e}", directory.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    files
        .iter()
        .flat_map(|file| replay_group(&read_transcript(file)))
        .collect()
}

/// A scenario of a hand-made group document, for tests of the engine itself.
pub fn scenario_document(group: &str, baseline: Value, scenarios: Vec<Value>) -> Value {
    json!({"format": 2, "group": group, "baseline": baseline, "scenarios": scenarios})
}
