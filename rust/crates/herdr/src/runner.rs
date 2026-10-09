//! The runner that drives the `herdr` binary (src/herdr/runner.ts), and the helpers that read its answers.

use crate::api::{HerdrError, HerdrOutput, HerdrRunner, RunOptions};
use regex::Regex;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

pub const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;

/// `/^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/`: a session name.
pub fn is_session_name(value: &str) -> bool {
    crate::api::is_agent_name(value)
}

/// The environment with every HERDR_ variable removed, so a command can never follow the caller's own pane into
/// another session.
pub fn herdr_environment(base: &HashMap<String, String>) -> HashMap<String, String> {
    base.iter()
        .filter(|(name, _)| !name.starts_with("HERDR_"))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

pub struct RunnerOptions {
    /// Required: the session is always named, so the operator's session cannot be reached by omission or inheritance.
    pub session: String,
    pub binary: Option<String>,
    pub timeout_ms: Option<u64>,
    /// The environment the binary gets (HERDR_ variables are removed); the process's own when `None`.
    pub env: Option<HashMap<String, String>>,
}

/// Runs `herdr --session <session> <args...>`.
pub struct ProcessRunner {
    session: String,
    binary: String,
    timeout_ms: u64,
    environment: HashMap<String, String>,
}

impl ProcessRunner {
    /// `createHerdrRunner`. Fails (like Node's `TypeError`) without a valid session name.
    pub fn new(options: RunnerOptions) -> Result<Self, String> {
        if !is_session_name(&options.session) {
            return Err("a Herdr session name is required".to_string());
        }
        let base = options
            .env
            .unwrap_or_else(|| std::env::vars().collect::<HashMap<_, _>>());
        Ok(Self {
            session: options.session,
            binary: options.binary.unwrap_or_else(|| "herdr".to_string()),
            timeout_ms: options.timeout_ms.unwrap_or(30_000),
            environment: herdr_environment(&base),
        })
    }
}

enum Chunk {
    Out(Vec<u8>),
    Err(Vec<u8>),
    Closed,
}

fn pump<R: Read + Send + 'static>(
    mut reader: R,
    wrap: fn(Vec<u8>) -> Chunk,
    sender: mpsc::Sender<Chunk>,
) {
    std::thread::spawn(move || {
        let mut buffer = [0u8; 16 * 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if sender.send(wrap(buffer[..n].to_vec())).is_err() {
                        return;
                    }
                }
            }
        }
        let _ = sender.send(Chunk::Closed);
    });
}

impl HerdrRunner for ProcessRunner {
    fn run(&self, args: &[String], options: &RunOptions) -> Result<HerdrOutput, HerdrError> {
        let first = args.first().map(String::as_str).unwrap_or("");
        let mut command = Command::new(&self.binary);
        command
            .arg("--session")
            .arg(&self.session)
            .args(args)
            .env_clear()
            .envs(&self.environment)
            .stdin(if options.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|error| {
            HerdrError::new("spawn", format!("herdr could not be started: {error}"))
        })?;
        if let (Some(bytes), Some(mut stdin)) = (options.stdin.clone(), child.stdin.take()) {
            std::thread::spawn(move || {
                let _ = stdin.write_all(&bytes);
            });
        }
        let (sender, receiver) = mpsc::channel();
        pump(
            child.stdout.take().expect("piped"),
            Chunk::Out,
            sender.clone(),
        );
        pump(child.stderr.take().expect("piped"), Chunk::Err, sender);
        let deadline =
            Instant::now() + Duration::from_millis(options.timeout_ms.unwrap_or(self.timeout_ms));
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let mut size = 0usize;
        let mut failure: Option<HerdrError> = None;
        let mut open = 2;
        while open > 0 {
            let wait = deadline.saturating_duration_since(Instant::now());
            match receiver.recv_timeout(wait) {
                Ok(Chunk::Closed) => open -= 1,
                Ok(Chunk::Out(bytes)) | Ok(Chunk::Err(bytes)) if failure.is_some() => drop(bytes),
                Ok(Chunk::Out(bytes)) => {
                    size += bytes.len();
                    if size > MAX_OUTPUT_BYTES {
                        failure = Some(HerdrError::new(
                            "output_too_large",
                            "herdr output exceeded the limit",
                        ));
                        let _ = child.kill();
                    } else {
                        stdout.extend_from_slice(&bytes);
                    }
                }
                Ok(Chunk::Err(bytes)) => {
                    size += bytes.len();
                    if size > MAX_OUTPUT_BYTES {
                        failure = Some(HerdrError::new(
                            "output_too_large",
                            "herdr output exceeded the limit",
                        ));
                        let _ = child.kill();
                    } else {
                        stderr.extend_from_slice(&bytes);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    failure.get_or_insert_with(|| {
                        HerdrError::new("timeout", format!("herdr {first} timed out"))
                    });
                    let _ = child.kill();
                    break;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        // The pipes can close while the child keeps running: the same deadline still applies to the exit.
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if failure.is_some() => break child.wait(),
                Ok(None) if Instant::now() >= deadline => {
                    failure = Some(HerdrError::new(
                        "timeout",
                        format!("herdr {first} timed out"),
                    ));
                    let _ = child.kill();
                    break child.wait();
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                Err(error) => break Err(error),
            }
        };
        if let Some(error) = failure {
            return Err(error);
        }
        let bad = || HerdrError::new("bad_output", "herdr wrote text that is not UTF-8");
        let code = status.ok().and_then(|s| s.code()).unwrap_or(1);
        Ok(HerdrOutput {
            code,
            stdout: String::from_utf8(stdout).map_err(|_| bad())?,
            stderr: String::from_utf8(stderr).map_err(|_| bad())?,
        })
    }
}

/// Control characters and length are removed so Herdr output cannot inject escape sequences into logs.
pub fn describe_output(text: &str) -> String {
    let replaced: String = text
        .chars()
        .map(|c| if is_cc_or_cf(c) { ' ' } else { c })
        .collect();
    let cut: String = crate::adapter::validate::js_trim(&replaced)
        .chars()
        .take(200)
        .collect();
    if cut.is_empty() {
        "no output".to_string()
    } else {
        cut
    }
}

static CC_CF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\p{Cc}\p{Cf}]$").expect("the pattern is valid"));

fn is_cc_or_cf(c: char) -> bool {
    c.is_control() || CC_CF.is_match(c.encode_utf8(&mut [0u8; 4]))
}

static ERROR_CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_.-]{1,64}$").expect("the pattern is valid"));

fn parse_json(text: &str) -> Option<Value> {
    serde_json::from_str(text.strip_prefix('\u{feff}').unwrap_or(text)).ok()
}

fn error_from(text: &str) -> Option<HerdrError> {
    let body = parse_json(text)?;
    let error = body.as_object()?.get("error")?.as_object()?;
    let code = match error.get("code").and_then(Value::as_str) {
        Some(code) if ERROR_CODE.is_match(code) => code.to_string(),
        _ => "error".to_string(),
    };
    let message = match error.get("message").and_then(Value::as_str) {
        Some(message) => describe_output(message),
        None => "herdr failed".to_string(),
    };
    Some(HerdrError::new(code, message))
}

/// The failure a non-zero exit stands for. Herdr writes it as JSON on stderr, and older shapes used stdout.
pub fn failure_of(args: &[String], outcome: &HerdrOutput) -> HerdrError {
    error_from(&outcome.stderr)
        .or_else(|| error_from(&outcome.stdout))
        .unwrap_or_else(|| {
            HerdrError::new(
                "exit",
                format!(
                    "herdr {} failed (exit {}): {}",
                    args.first().map(String::as_str).unwrap_or(""),
                    outcome.code,
                    describe_output(&outcome.stderr)
                ),
            )
        })
}

/// Runs a Herdr command that answers with `{id, result}` JSON and returns the result, or the failure as a HerdrError.
pub fn run_json(
    runner: &dyn HerdrRunner,
    args: &[String],
    options: &RunOptions,
) -> Result<Map<String, Value>, HerdrError> {
    let first = args.first().map(String::as_str).unwrap_or("");
    let outcome = runner.run(args, options)?;
    if outcome.code != 0 {
        return Err(failure_of(args, &outcome));
    }
    let Some(body) = parse_json(&outcome.stdout) else {
        return Err(HerdrError::new(
            "bad_output",
            format!(
                "herdr {first} did not answer with JSON: {}",
                describe_output(&outcome.stdout)
            ),
        ));
    };
    let Value::Object(mut record) = body else {
        return Err(HerdrError::new(
            "bad_output",
            "herdr answered with an unexpected shape",
        ));
    };
    if let Some(error) = error_from(&outcome.stdout) {
        return Err(error);
    }
    match record.remove("result") {
        Some(Value::Object(result)) => Ok(result),
        _ => Err(HerdrError::new(
            "bad_output",
            "herdr answered without a result",
        )),
    }
}
