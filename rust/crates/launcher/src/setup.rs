//! Runs the worktree setup and teardown commands (src/launcher/setup.ts), and the command runner under them
//! (`runCommand` and `sanitiseOutput` of src/command-runner.ts): `sh -c` in its own process group, a timeout that ends
//! the whole group, and a capped, sanitised tail of the output.

use crate::api::SetupOutcome;
use crate::kernel::LauncherKernel;
use crate::shared::{Budget, OpError, OpResult, SETUP_OUTPUT_CHARS};
use crate::text::one_line;
use capstan_config::{Worktree, DEFAULT_WORKTREE_TEARDOWN_TIMEOUT_SECONDS};
use capstan_kernel::helpers::strip_terminal_sequences;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};
use unicode_segmentation::UnicodeSegmentation;

const RUN_KILL_GRACE: Duration = Duration::from_millis(2000);
/// The raw window kept while a command runs; sanitising only shrinks it, so the final tail still has the requested size.
const RAW_WINDOW_FACTOR: usize = 4;
const MIN_RAW_WINDOW_BYTES: usize = 16384;
const MIN_SECRET_VALUE_CHARS: usize = 8;
const REDACTED: &str = "[redacted]";

fn pattern(source: &str) -> Regex {
    Regex::new(source).expect("the pattern is valid")
}

static SECRET_NAME: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"(?i)TOKEN|SECRET|KEY|PASSWORD|PASSWD|CREDENTIAL"));

/// Shapes that look like a credential: provider key prefixes, JWTs, bearer headers and long mixed letter-digit runs (a 40
/// or 64 hex commit or digest is left alone).
static CREDENTIAL_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        pattern(r"(?-u:\b)(?:Bearer|Basic)\s+[A-Za-z0-9._~+/=-]{8,}"),
        pattern(r"(?-u:\b)(?:sk|pk|rk)-[A-Za-z0-9_-]{16,}"),
        pattern(r"(?-u:\b)gh[pousr]_[A-Za-z0-9]{20,}"),
        pattern(r"(?-u:\b)github_pat_[A-Za-z0-9_]{20,}"),
        pattern(r"(?-u:\b)xox[abprs]-[A-Za-z0-9-]{10,}"),
        pattern(r"(?-u:\b)(?:AKIA|ASIA)[A-Z0-9]{16}(?-u:\b)"),
        pattern(r"(?-u:\b)eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}"),
        pattern(
            r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?(?:-----END [A-Z ]*PRIVATE KEY-----|$)",
        ),
    ]
});
static LONG_RUN: LazyLock<Regex> = LazyLock::new(|| pattern(r"[A-Za-z0-9_-]{32,}"));
static CONTROLS: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"[\x{00}-\x{08}\x{0b}-\x{1f}\x{7f}-\x{9f}]"));
static FORMATS: LazyLock<Regex> = LazyLock::new(|| pattern(r"[\p{Cf}\p{Zl}\p{Zp}]"));

fn looks_like_secret_run(run: &str) -> bool {
    run.bytes().any(|b| b.is_ascii_alphabetic())
        && run.bytes().any(|b| b.is_ascii_digit())
        && !run.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The values of the environment variables whose names say they are secrets, longest first.
fn secret_values<'a>(environments: impl IntoIterator<Item = (&'a str, &'a str)>) -> Vec<String> {
    let mut values: Vec<String> = Vec::new();
    for (name, value) in environments {
        if SECRET_NAME.is_match(name)
            && value.encode_utf16().count() >= MIN_SECRET_VALUE_CHARS
            && !values.iter().any(|v| v == value)
        {
            values.push(value.to_string());
        }
    }
    values.sort_by_key(|v| std::cmp::Reverse(v.encode_utf16().count()));
    values
}

/// The last `max_bytes` bytes of `text`, cut on a user-perceived character boundary.
fn tail_of(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let segments: Vec<&str> = text.graphemes(true).collect();
    let mut bytes = 0;
    let mut start = segments.len();
    while start > 0 {
        let size = segments[start - 1].len();
        if bytes + size > max_bytes {
            break;
        }
        bytes += size;
        start -= 1;
    }
    segments[start..].concat()
}

/// `sanitiseOutput`: terminal escapes and C0 controls (other than newline and tab) are removed, credential shapes and the
/// given secret values are replaced by a marker, and the last `max_bytes` bytes are kept.
pub fn sanitise_output(raw: &str, max_bytes: usize, secrets: &[String]) -> String {
    let stripped = strip_terminal_sequences(raw);
    let mut text = stripped.replace("\r\n", "\n").replace('\r', "\n");
    text = CONTROLS.replace_all(&text, "").into_owned();
    text = FORMATS.replace_all(&text, "").into_owned();
    for value in secrets {
        if !value.is_empty() {
            text = text
                .split(value.as_str())
                .collect::<Vec<_>>()
                .join(REDACTED);
        }
    }
    for pattern in CREDENTIAL_PATTERNS.iter() {
        text = pattern.replace_all(&text, REDACTED).into_owned();
    }
    text = LONG_RUN
        .replace_all(&text, |found: &regex::Captures<'_>| {
            let run = &found[0];
            if looks_like_secret_run(run) {
                REDACTED.to_string()
            } else {
                run.to_string()
            }
        })
        .into_owned();
    tail_of(&text, max_bytes)
}

fn signal_group(pid: u32, signal: i32) {
    // SAFETY: kill with a negative pid signals the process group of a child this function started; a group that is gone
    // answers ESRCH, which is ignored.
    unsafe {
        libc::kill(-(pid as i32), signal);
    }
}

fn read_into(
    mut pipe: impl Read + Send + 'static,
    window: usize,
    sink: Arc<Mutex<Vec<u8>>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) | Err(_) => return,
                Ok(n) => {
                    let mut raw = sink.lock().unwrap_or_else(|p| p.into_inner());
                    raw.extend_from_slice(&chunk[..n]);
                    if raw.len() > window {
                        let excess = raw.len() - window;
                        raw.drain(..excess);
                    }
                }
            }
        }
    })
}

/// Runs `command` through `sh -c` in its own process group; the whole group is killed on timeout. Its output is kept only
/// as a capped tail.
pub fn run_setup_command(
    command: &str,
    cwd: &str,
    timeout_ms: u64,
    environment: &BTreeMap<String, String>,
) -> SetupOutcome {
    let tail_bytes = SETUP_OUTPUT_CHARS * 2;
    let window = MIN_RAW_WINDOW_BYTES.max(tail_bytes * RAW_WINDOW_FACTOR);
    let process_env: Vec<(String, String)> = std::env::vars().collect();
    let secrets = secret_values(
        environment
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .chain(process_env.iter().map(|(k, v)| (k.as_str(), v.as_str()))),
    );
    let mut child = match Command::new("sh")
        .args(["-c", command])
        .current_dir(cwd)
        .env_clear()
        .envs(environment)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return SetupOutcome::Failed {
                exit_code: None,
                output: sanitise_output(&format!("\n{error}"), tail_bytes, &secrets),
            }
        }
    };
    let pid = child.id();
    let raw = Arc::new(Mutex::new(Vec::new()));
    let readers: Vec<_> = [
        child
            .stdout
            .take()
            .map(|p| read_into(p, window, Arc::clone(&raw))),
        child
            .stderr
            .take()
            .map(|p| read_into(p, window, Arc::clone(&raw))),
    ]
    .into_iter()
    .flatten()
    .collect();
    let started = Instant::now();
    let timeout = Duration::from_millis(timeout_ms);
    let mut code: Option<Option<i32>> = None;
    let mut timed_out = false;
    let mut hard_kill_at: Option<Instant> = None;
    loop {
        if code.is_none() {
            if let Ok(Some(status)) = child.try_wait() {
                code = Some(status.code());
            }
        }
        let closed = readers.iter().all(|r| r.is_finished());
        if code.is_some() && closed {
            break;
        }
        if !timed_out && started.elapsed() >= timeout {
            timed_out = true;
            signal_group(pid, libc::SIGTERM);
            hard_kill_at = Some(Instant::now() + RUN_KILL_GRACE);
        }
        if let Some(at) = hard_kill_at {
            if Instant::now() >= at {
                signal_group(pid, libc::SIGKILL);
                hard_kill_at = None;
                // The group is dead; a leader that is somehow still around is reaped below.
                if code.is_none() {
                    let _ = child.kill();
                    code = Some(child.wait().ok().and_then(|s| s.code()));
                }
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    // A group member that ignored SIGTERM must still die.
    if timed_out {
        signal_group(pid, libc::SIGKILL);
    }
    for reader in readers {
        let _ = reader.join();
    }
    if timed_out {
        return SetupOutcome::Timeout;
    }
    if code == Some(Some(0)) {
        return SetupOutcome::Ok;
    }
    let bytes = raw.lock().unwrap_or_else(|p| p.into_inner()).clone();
    SetupOutcome::Failed {
        exit_code: code.flatten(),
        output: sanitise_output(&String::from_utf8_lossy(&bytes), tail_bytes, &secrets),
    }
}

impl LauncherKernel {
    /// Runs the configured setup command in a new worktree. Its time is not taken from the operation's budget; a failure or
    /// timeout is an error, so the caller's cleanup removes the agent.
    pub fn setup_worktree(
        &self,
        config: &Worktree,
        agent_id: &str,
        worktree_path: &str,
        budget: &Budget,
    ) -> OpResult<()> {
        let Some(setup) = &config.setup else {
            return Ok(());
        };
        budget.check("running the worktree setup")?;
        let started = self.now_ms();
        let outcome = self.run_setup(
            setup,
            worktree_path,
            (config.setup_timeout_seconds * 1000) as u64,
        );
        budget.extend((self.now_ms() - started).max(0));
        let outcome = outcome?;
        let command = one_line(setup, 200);
        match outcome {
            SetupOutcome::Ok => Ok(()),
            SetupOutcome::Timeout => Err(OpError::launcher(
                "worktree_setup_failed",
                format!(
                    "the setup of {agent_id} ({command}) timed out after {}s",
                    config.setup_timeout_seconds
                ),
            )),
            SetupOutcome::Failed { exit_code, output } => {
                let tail = one_line(&output, SETUP_OUTPUT_CHARS);
                Err(OpError::launcher(
                    "worktree_setup_failed",
                    format!(
                        "the setup of {agent_id} ({command}) failed with exit code {}{}",
                        exit_code.map_or("none".to_string(), |c| c.to_string()),
                        if tail.is_empty() {
                            String::new()
                        } else {
                            format!(": {tail}")
                        }
                    ),
                ))
            }
        }
    }

    /// Runs the configured teardown command in the project root just before a worktree is removed. A failure or timeout is
    /// logged and never stops the removal. Returns how the command ended (`Ok` when none is configured).
    pub fn teardown_worktree(
        &self,
        agent_id: &str,
        worktree_path: &str,
        budget: &Budget,
    ) -> SetupOutcome {
        let Some(config) = &self.config.worktree else {
            return SetupOutcome::Ok;
        };
        let Some(teardown) = &config.teardown else {
            return SetupOutcome::Ok;
        };
        let timeout_seconds = config
            .teardown_timeout_seconds
            .unwrap_or(DEFAULT_WORKTREE_TEARDOWN_TIMEOUT_SECONDS);
        self.log("teardown_started", json!({"agentId": agent_id}));
        let started = self.now_ms();
        let outcome = match self.environment(None, Some(true)) {
            Ok(mut environment) => {
                environment.insert("CAPSTAN_WORKTREE_PATH".into(), worktree_path.to_string());
                environment.insert("CAPSTAN_AGENT_ID".into(), agent_id.to_string());
                self.run_teardown(
                    teardown,
                    &self.root,
                    (timeout_seconds * 1000) as u64,
                    &environment,
                )
            }
            Err(error) => SetupOutcome::Failed {
                exit_code: None,
                output: error.message(),
            },
        };
        budget.extend((self.now_ms() - started).max(0));
        match &outcome {
            SetupOutcome::Ok => {}
            SetupOutcome::Timeout => self.log(
                "teardown_failed",
                json!({"agentId": agent_id, "exit": "timeout", "output": ""}),
            ),
            SetupOutcome::Failed { exit_code, output } => self.log(
                "teardown_failed",
                json!({
                    "agentId": agent_id,
                    "exit": exit_code.map_or(Value::Null, |c| json!(c)),
                    "output": one_line(output, SETUP_OUTPUT_CHARS),
                }),
            ),
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> BTreeMap<String, String> {
        BTreeMap::from([("PATH".to_string(), "/usr/bin:/bin".to_string())])
    }

    #[test]
    fn a_command_that_succeeds_is_ok() {
        assert_eq!(
            run_setup_command("echo hi", "/", 5_000, &env()),
            SetupOutcome::Ok
        );
    }

    #[test]
    fn a_failing_command_reports_its_exit_code_and_output() {
        match run_setup_command("echo oops >&2; exit 3", "/", 5_000, &env()) {
            SetupOutcome::Failed { exit_code, output } => {
                assert_eq!(exit_code, Some(3));
                assert_eq!(output.trim(), "oops");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_command_past_its_time_ends_with_its_group() {
        let started = Instant::now();
        assert_eq!(
            run_setup_command("sleep 30 & sleep 30", "/", 200, &env()),
            SetupOutcome::Timeout
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn credentials_never_reach_the_tail() {
        let secrets = vec!["hunter2hunter2".to_string()];
        let out = sanitise_output(
            "token hunter2hunter2 and Bearer abcdefghijkl and ghp_abcdefghijklmnopqrstuv\u{1b}[31m done",
            4000,
            &secrets,
        );
        assert!(!out.contains("hunter2"));
        assert!(!out.contains("abcdefghijkl"));
        assert!(!out.contains("ghp_"));
        assert!(out.contains("done"));
        // A commit id stays.
        let sha = "a".repeat(40);
        assert_eq!(sanitise_output(&sha, 4000, &[]), sha);
    }

    #[test]
    fn the_tail_keeps_the_end() {
        assert_eq!(sanitise_output("abcdefghij", 4, &[]), "ghij");
    }
}
