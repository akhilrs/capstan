//! The Herdr adapter (src/herdr/adapter*.ts): the only module that types into, starts, stops or closes agents in
//! Herdr. It refuses to type into a pane it did not create, into a PM pane once its agent runs, and into any pane in the
//! wrong phase. Herdr's agent state is a hint; nothing here treats it as completion.
//!
//! The Node adapter is three classes (`HerdrAdapter`, `PaneOperations`, `PaneInput`) over a shared core; here the core
//! is the `Adapter` struct and the other two are `impl Adapter` blocks in `panes` and `input`. Everything blocks.

mod input;
mod panes;
pub mod validate;

use crate::api::*;
use crate::naming::herdr_agent_name;
use crate::runner::{failure_of, run_json};
use crate::screen::{classify_input_blocker, extract_input_line, fresh_prompt_ready, strip_ansi};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use validate::*;

/// `TESTED_HERDR_VERSION`.
pub const TESTED_HERDR_VERSION: &str = "0.9.1";

const AGENT_START_MARGIN_MS: u64 = 10_000;
const MAX_NOTIFICATION_TITLE_CHARS: usize = 100;
const MAX_NOTIFICATION_BODY_CHARS: usize = 500;

pub type SleepFn = Arc<dyn Fn(u64) + Send + Sync>;
pub type NowFn = Arc<dyn Fn() -> u64 + Send + Sync>;

pub struct AdapterOptions {
    pub run: Arc<dyn HerdrRunner>,
    pub temp_root: Option<PathBuf>,
    pub poll_ms: Option<u64>,
    pub sleep: Option<SleepFn>,
    pub now: Option<NowFn>,
    /// The project's slug: Herdr then sees every agent as `<slug>-<agent-id>`, while callers keep passing the ledger id.
    pub project_slug: Option<String>,
}

impl AdapterOptions {
    pub fn new(run: Arc<dyn HerdrRunner>) -> Self {
        Self {
            run,
            temp_root: None,
            poll_ms: None,
            sleep: None,
            now: None,
            project_slug: None,
        }
    }
}

/// The registry keeps insertion order, like the Node `Map`: setting a known pane keeps its place.
#[derive(Default)]
struct State {
    panes: Vec<(String, PaneEntry)>,
    prompt_directory: Option<PathBuf>,
}

impl State {
    fn get(&self, pane_id: &str) -> Option<&PaneEntry> {
        self.panes
            .iter()
            .find(|(id, _)| id == pane_id)
            .map(|(_, e)| e)
    }

    fn set(&mut self, pane_id: &str, entry: PaneEntry) {
        match self.panes.iter_mut().find(|(id, _)| id == pane_id) {
            Some(slot) => slot.1 = entry,
            None => self.panes.push((pane_id.to_string(), entry)),
        }
    }

    fn delete(&mut self, pane_id: &str) {
        self.panes.retain(|(id, _)| id != pane_id);
    }
}

pub struct Adapter {
    run: Arc<dyn HerdrRunner>,
    temp_root: PathBuf,
    poll_ms: u64,
    sleep: SleepFn,
    now: NowFn,
    slug: Option<String>,
    state: Mutex<State>,
}

pub(crate) fn herdr_failure(code: &str, message: &str) -> AdapterError {
    AdapterError::Herdr(HerdrError::new(code, message))
}

/// `core.record`: the value as an object, or the `bad_output` failure.
pub(crate) fn record<'a>(
    value: Option<&'a Value>,
    label: &str,
) -> Result<&'a Map<String, Value>, HerdrError> {
    match value {
        Some(Value::Object(map)) => Ok(map),
        _ => Err(HerdrError::new(
            "bad_output",
            format!("herdr did not report {label}"),
        )),
    }
}

/// `requireMatch` for a JSON value that must be a string.
pub(crate) fn require_match_value<'a>(
    value: Option<&'a Value>,
    pattern: &regex::Regex,
    label: &str,
) -> AdapterResult<&'a str> {
    match value.and_then(Value::as_str) {
        Some(text) => require_match(text, pattern, label),
        None => Err(invalid(format!("{label} is not acceptable"))),
    }
}

pub(crate) fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

/// A random lower-case hex string of `bytes` bytes.
pub(crate) fn random_hex(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    getrandom::fill(&mut buffer).expect("the system has randomness");
    buffer.iter().map(|b| format!("{b:02x}")).collect()
}

fn random_uuid() -> String {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("the system has randomness");
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: Vec<String> = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        h[0..4].concat(),
        h[4..6].concat(),
        h[6..8].concat(),
        h[8..10].concat(),
        h[10..16].concat()
    )
}

/// `mkdtemp(prefix + 6 random characters)` with mode 0700.
fn make_private_directory(root: &Path, prefix: &str) -> AdapterResult<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    for _ in 0..16 {
        let directory = root.join(format!("{prefix}{}", random_hex(3)));
        match std::fs::DirBuilder::new().mode(0o700).create(&directory) {
            Ok(()) => return Ok(directory),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(io_failure(error)),
        }
    }
    Err(invalid("a temporary directory could not be created"))
}

fn write_private_file(path: &Path, content: &str) -> AdapterResult<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(io_failure)?;
    file.write_all(content.as_bytes()).map_err(io_failure)
}

/// A filesystem failure of the adapter's own temporary files (a plain `Error` in Node).
fn io_failure(error: std::io::Error) -> AdapterError {
    AdapterError::Herdr(HerdrError::new("io", error.to_string()))
}

impl Adapter {
    pub fn new(options: AdapterOptions) -> Self {
        let sleep: SleepFn = options.sleep.unwrap_or_else(|| {
            Arc::new(|ms| std::thread::sleep(std::time::Duration::from_millis(ms)))
        });
        let now: NowFn = options.now.unwrap_or_else(|| {
            Arc::new(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as u64)
            })
        });
        Self {
            run: options.run,
            temp_root: options.temp_root.unwrap_or_else(std::env::temp_dir),
            poll_ms: options.poll_ms.unwrap_or(100),
            sleep,
            now,
            slug: options.project_slug,
            state: Mutex::new(State::default()),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub(crate) fn now(&self) -> u64 {
        (self.now)()
    }

    pub(crate) fn sleep(&self, ms: u64) {
        (self.sleep)(ms)
    }

    pub(crate) fn poll_ms(&self) -> u64 {
        self.poll_ms
    }

    pub(crate) fn entry(&self, pane_id: &str) -> Option<PaneEntry> {
        self.state().get(pane_id).cloned()
    }

    pub(crate) fn set_entry(&self, pane_id: &str, entry: PaneEntry) {
        self.state().set(pane_id, entry);
    }

    pub(crate) fn delete_entry(&self, pane_id: &str) {
        self.state().delete(pane_id);
    }

    pub(crate) fn entries(&self) -> Vec<(String, PaneEntry)> {
        self.state().panes.clone()
    }

    /// The name Herdr knows an agent by: the ledger id behind the project's slug.
    pub(crate) fn herdr_name(&self, agent_id: &str) -> AdapterResult<String> {
        match &self.slug {
            None => Ok(agent_id.to_string()),
            Some(slug) => herdr_agent_name(slug, agent_id).map_err(invalid),
        }
    }

    pub(crate) fn slug(&self) -> Option<&str> {
        self.slug.as_deref()
    }

    pub(crate) fn call(&self, args: &[String]) -> Result<HerdrOutput, HerdrError> {
        self.run.run(args, &RunOptions::default())
    }

    pub(crate) fn json(&self, args: &[String]) -> Result<Map<String, Value>, HerdrError> {
        run_json(self.run.as_ref(), args, &RunOptions::default())
    }

    pub(crate) fn json_with(
        &self,
        args: &[String],
        options: &RunOptions,
    ) -> Result<Map<String, Value>, HerdrError> {
        run_json(self.run.as_ref(), args, options)
    }

    /// For commands that print nothing on success: a non-zero exit or a JSON error is a failure.
    pub(crate) fn run_checked(&self, args: &[String]) -> AdapterResult<()> {
        let outcome = self.call(args)?;
        if outcome.code != 0 {
            return Err(failure_of(args, &outcome).into());
        }
        Ok(())
    }

    pub(crate) fn assert_typable(&self, pane_id: &str, action: Action) -> AdapterResult<PaneEntry> {
        let Some(entry) = self.entry(pane_id) else {
            return Err(AdapterError::UnknownPane(
                "the pane was not created by this adapter".into(),
            ));
        };
        if entry.phase == PanePhase::Tainted {
            return Err(AdapterError::Phase(
                "the pane is tainted and must be closed".into(),
            ));
        }
        let launching = matches!(action, Action::Prepare | Action::Start);
        if entry.role == PaneRole::Pm && (entry.phase == PanePhase::Started || !launching) {
            return Err(AdapterError::PmPane(
                "the PM pane accepts only the controller's launch sequence".into(),
            ));
        }
        if launching {
            if entry.phase == PanePhase::Started {
                return Err(AdapterError::Phase("the agent is already started".into()));
            }
            if action == Action::Prepare && entry.phase != PanePhase::Fresh {
                return Err(AdapterError::Phase("the pane is already prepared".into()));
            }
        } else if entry.phase != PanePhase::Started {
            return Err(AdapterError::Phase("the pane has no started agent".into()));
        }
        Ok(entry)
    }

    pub(crate) fn state_for(&self, agent: &str, pane_id: &str) -> AdapterResult<String> {
        let state = self.agent_state_of(agent)?;
        if state.pane_id != pane_id {
            return Err(AdapterError::AgentPaneMismatch(
                "the agent name no longer points at the registered pane".into(),
            ));
        }
        Ok(state.status)
    }

    pub(crate) fn agent_state_of(&self, name: &str) -> AdapterResult<AgentState> {
        require_match_name(name, "agent name")?;
        self.agent_state_by_herdr_name(&self.herdr_name(name)?)
    }

    pub(crate) fn agent_state_by_herdr_name(&self, herdr_name: &str) -> AdapterResult<AgentState> {
        let result = self.json(&strings(&["agent", "get", herdr_name]))?;
        let agent = record(result.get("agent"), "agent")?;
        let text = |key: &str, default: &str| {
            agent
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or(default)
                .to_string()
        };
        Ok(AgentState {
            status: text("agent_status", "unknown"),
            pane_id: text("pane_id", ""),
            kind: text("agent", "unknown"),
        })
    }

    pub(crate) fn screen(
        &self,
        pane_id: &str,
        ansi: bool,
        lines: Option<usize>,
    ) -> AdapterResult<String> {
        require_match(pane_id, &PANE_PATTERN, "pane id")?;
        let mut args = strings(&["pane", "read", pane_id, "--source", "visible", "--lines"]);
        args.push(lines.unwrap_or(80).to_string());
        if ansi {
            args.push("--ansi".into());
        }
        let outcome = self.call(&args)?;
        if outcome.code != 0 {
            return Err(failure_of(&strings(&["pane", "read"]), &outcome).into());
        }
        Ok(outcome.stdout)
    }

    /// The input line and, from the same screen read, what blocks it when it is unreadable.
    pub(crate) fn input_and_blocker(
        &self,
        pane_id: &str,
    ) -> AdapterResult<(Option<String>, InputBlocker)> {
        let Some(entry) = self.entry(pane_id) else {
            return Err(AdapterError::UnknownPane("pane is not registered".into()));
        };
        let screen = self.screen(pane_id, true, None)?;
        let text = extract_input_line(&entry.kind, &screen);
        let blocker = if text.is_none() {
            classify_input_blocker(&entry.kind, &screen)
        } else {
            InputBlocker::Unknown
        };
        Ok((text, blocker))
    }

    pub(crate) fn input_of(&self, pane_id: &str) -> AdapterResult<Option<String>> {
        Ok(self.input_and_blocker(pane_id)?.0)
    }

    fn wait_for_fresh_prompt(&self, pane_id: &str) -> AdapterResult<()> {
        let deadline = self.now() + 3_000;
        loop {
            if fresh_prompt_ready(&strip_ansi(&self.screen(pane_id, false, None)?)) {
                return Ok(());
            }
            self.sleep(self.poll_ms);
            if self.now() >= deadline {
                break;
            }
        }
        Err(AdapterError::PromptUnrecognized(
            "the pane does not end in a bare prompt symbol, so nothing was typed".into(),
        ))
    }

    fn set_phase(&self, pane_id: &str, entry: &PaneEntry, phase: PanePhase) {
        self.set_entry(
            pane_id,
            PaneEntry {
                phase,
                ..entry.clone()
            },
        );
    }

    // -------------------------------------------------------------------------------------- the adapter's own calls

    pub fn version(&self) -> AdapterResult<String> {
        let outcome = self.call(&strings(&["--version"]))?;
        if outcome.code != 0 {
            return Err(herdr_failure("version", "herdr --version failed"));
        }
        Ok(js_trim(&outcome.stdout).to_string())
    }

    pub fn run_in_pane(&self, pane_id: &str, command: &str) -> AdapterResult<()> {
        require_match(pane_id, &PANE_PATTERN, "pane id")?;
        let Some(entry) = self.entry(pane_id) else {
            return Err(AdapterError::UnknownPane(
                "the pane was not created by this adapter".into(),
            ));
        };
        if entry.role != PaneRole::Worker
            || entry.kind != "shell"
            || (entry.phase != PanePhase::Fresh && entry.phase != PanePhase::Prepared)
        {
            return Err(AdapterError::Phase(
                "only a fresh or prepared shell pane runs a command".into(),
            ));
        }
        if js_trim(command).is_empty() || utf16_len(command) > 2000 || has_control(command) {
            return Err(invalid("the command is not acceptable"));
        }
        self.wait_for_fresh_prompt(pane_id)?;
        self.run_checked(&strings(&["pane", "run", pane_id, command]))?;
        self.set_phase(pane_id, &entry, PanePhase::Started);
        Ok(())
    }

    /// Shows a notification in Herdr; the text is checked like any text that reaches the operator's screen.
    pub fn show_notification(&self, title: &str, body: &str) -> AdapterResult<()> {
        for (value, label, max) in [
            (title, "notification title", MAX_NOTIFICATION_TITLE_CHARS),
            (body, "notification body", MAX_NOTIFICATION_BODY_CHARS),
        ] {
            if js_trim(value).is_empty() || utf16_len(value) > max || has_unsafe(value) {
                return Err(invalid(format!("{label} is not acceptable")));
            }
        }
        let result = self.json(&strings(&[
            "notification",
            "show",
            title,
            "--body",
            body,
            "--sound",
            "request",
        ]))?;
        // Herdr answers exit 0 with shown:false when notifications are off, so an accepted command alone does not mean
        // the operator saw anything.
        if result.get("shown") != Some(&Value::Bool(true)) {
            let reason = match result.get("reason").and_then(Value::as_str) {
                Some(reason) => {
                    let kept: String = reason
                        .chars()
                        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '-'))
                        .collect();
                    format!(": {}", kept.chars().take(60).collect::<String>())
                }
                None => String::new(),
            };
            return Err(herdr_failure(
                "notification_not_shown",
                &format!("Herdr did not show the notification{reason}"),
            ));
        }
        Ok(())
    }

    pub fn pane_state_of(&self, pane_id: &str) -> AdapterResult<PaneState> {
        require_match(pane_id, &PANE_PATTERN, "pane id")?;
        let result = self.json(&strings(&["pane", "get", pane_id]))?;
        let pane = record(result.get("pane"), "pane")?;
        Ok(PaneState {
            status: pane
                .get("agent_status")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
            agent: pane
                .get("agent")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }

    pub fn prepare_shell_pane(&self, input: &PrepareShellInput<'_>) -> AdapterResult<()> {
        let entry = self.assert_typable(input.pane_id, Action::Prepare)?;
        if entry.phase != PanePhase::Fresh {
            return Err(AdapterError::Phase(
                "only a fresh pane can be prepared".into(),
            ));
        }
        let environment = input.environment;
        let home = require_quotable(environment.get("HOME").map(String::as_str), "HOME")?;
        if !home.starts_with('/') {
            return Err(invalid("HOME is not acceptable"));
        }
        let path_value = require_quotable(environment.get("PATH").map(String::as_str), "PATH")?;
        let term = require_quotable(environment.get("TERM").map(String::as_str), "TERM")?;
        for (name, value) in environment {
            if !ENVIRONMENT_KEY.is_match(name) {
                return Err(invalid(format!(
                    "environment name {name} is not acceptable"
                )));
            }
            if has_unsafe(value) {
                return Err(invalid(format!(
                    "environment value for {name} is not acceptable"
                )));
            }
        }
        self.wait_for_fresh_prompt(input.pane_id)?;

        let directory = make_private_directory(&self.temp_root, "capstan-shell-")?;
        let result = (|| -> AdapterResult<()> {
            let env_file = directory.join("env");
            let rc_file = directory.join("rc");
            for file in [&directory, &env_file, &rc_file] {
                require_match(&file.to_string_lossy(), &SIMPLE_VALUE, "temporary path")?;
            }
            let exports: String = environment
                .iter()
                .map(|(name, value)| format!("export {name}={}\n", shell_quote(value)))
                .collect();
            write_private_file(&env_file, &exports)?;
            write_private_file(
                &rc_file,
                &format!(
                    ". '{}'\nPS1='❯ '\nrm -rf '{}'\n",
                    env_file.display(),
                    directory.display()
                ),
            )?;
            self.set_entry(
                input.pane_id,
                PaneEntry {
                    phase: PanePhase::Prepared,
                    kind: "shell".into(),
                    ..entry.clone()
                },
            );
            let command = format!(
                "exec env -i HOME={} PATH={} TERM={} bash --noprofile --rcfile '{}' -i",
                shell_quote(home),
                shell_quote(path_value),
                shell_quote(term),
                rc_file.display()
            );
            if let Err(error) =
                self.run_checked(&strings(&["pane", "run", input.pane_id, &command]))
            {
                self.set_phase(input.pane_id, &entry, PanePhase::Tainted);
                return Err(error);
            }
            let deadline = self.now() + input.timeout_ms.unwrap_or(10_000);
            while self.now() < deadline {
                if !directory.exists() && self.input_of(input.pane_id)?.as_deref() == Some("") {
                    return Ok(());
                }
                self.sleep(self.poll_ms);
            }
            self.set_phase(input.pane_id, &entry, PanePhase::Tainted);
            Err(AdapterError::ShellNotReady(
                "the prepared shell did not show its prompt in time".into(),
            ))
        })();
        let _ = std::fs::remove_dir_all(&directory);
        result
    }

    pub fn write_prompt(&self, text: &str) -> AdapterResult<String> {
        if !is_safe_text(text) {
            return Err(invalid("prompt text is not acceptable"));
        }
        let directory = {
            let mut state = self.state();
            match &state.prompt_directory {
                Some(directory) => directory.clone(),
                None => {
                    let directory = make_private_directory(&self.temp_root, "capstan-prompts-")?;
                    state.prompt_directory = Some(directory.clone());
                    directory
                }
            }
        };
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
                .map_err(io_failure)?;
        }
        let file = directory.join(format!("{}.md", random_uuid()));
        write_private_file(&file, text)?;
        Ok(file.to_string_lossy().into_owned())
    }

    pub fn start_agent_in_pane(&self, input: &StartAgentInput<'_>) -> AdapterResult<StartStatus> {
        if !HOST_KINDS.contains(&input.kind) {
            return Err(AdapterError::UnsupportedHost(format!(
                "agent kind {} is not supported yet",
                input.kind
            )));
        }
        require_match_name(input.name, "agent name")?;
        let herdr_name = self.herdr_name(input.name)?;
        if self
            .entries()
            .iter()
            .any(|(_, other)| other.agent.as_deref() == Some(input.name))
        {
            return Err(invalid("an agent with this name is already registered"));
        }
        for arg in input.args {
            if arg.is_empty() || has_control(arg) {
                return Err(invalid(
                    "an agent argument is empty or has control characters",
                ));
            }
        }
        let timeout_ms = input.timeout_ms.unwrap_or(30_000).max(5_000);
        let mut entry = self.assert_typable(input.pane_id, Action::Start)?;
        match input.environment {
            None if entry.phase == PanePhase::Fresh => {
                return Err(invalid(
                    "a fresh pane needs an environment so its shell starts clean",
                ));
            }
            Some(environment) if entry.phase == PanePhase::Fresh => {
                self.prepare_shell_pane(&PrepareShellInput {
                    pane_id: input.pane_id,
                    environment,
                    timeout_ms: None,
                })?;
                entry = self.assert_typable(input.pane_id, Action::Start)?;
            }
            _ => {
                if self.input_of(input.pane_id)?.as_deref() != Some("") {
                    self.set_phase(input.pane_id, &entry, PanePhase::Tainted);
                    return Err(AdapterError::PromptUnrecognized(
                        "the prepared shell does not show an empty prompt".into(),
                    ));
                }
            }
        }
        let mut args = strings(&[
            "agent",
            "start",
            &herdr_name,
            "--kind",
            input.kind,
            "--pane",
            input.pane_id,
            "--timeout",
        ]);
        args.push(timeout_ms.to_string());
        args.push("--".into());
        args.extend(input.args.iter().cloned());
        let started = PaneEntry {
            phase: PanePhase::Started,
            kind: input.kind.to_string(),
            agent: Some(input.name.to_string()),
            ..entry.clone()
        };
        let options = RunOptions {
            timeout_ms: Some(timeout_ms + AGENT_START_MARGIN_MS),
            stdin: None,
        };
        match self.json_with(&args, &options) {
            Ok(_) => {
                self.set_entry(input.pane_id, started);
                Ok(StartStatus::Started)
            }
            Err(error) if error.code == "agent_not_ready" => {
                self.set_entry(input.pane_id, started);
                Ok(StartStatus::BlockedAtStartup)
            }
            Err(error) => {
                self.set_phase(input.pane_id, &entry, PanePhase::Tainted);
                Err(error.into())
            }
        }
    }
}

/// The five things the registry check is asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Prepare,
    Start,
    Send,
    Clear,
    Dialog,
}

pub(crate) fn require_match_name<'a>(value: &'a str, label: &str) -> AdapterResult<&'a str> {
    if is_name(value) {
        Ok(value)
    } else {
        Err(invalid(format!("{label} is not acceptable")))
    }
}

// ------------------------------------------------------------------------------------------------ the traits

impl PaneRegistry for Adapter {
    fn pane_for_agent(&self, agent_id: &str) -> Option<String> {
        self.entries()
            .into_iter()
            .find(|(_, entry)| entry.agent.as_deref() == Some(agent_id))
            .map(|(pane_id, _)| pane_id)
    }

    fn pane_entry(&self, pane_id: &str) -> Option<PaneEntry> {
        self.entry(pane_id)
    }

    fn agent_observation(&self, agent_id: &str) -> AdapterResult<HerdrState> {
        let Some(pane_id) = self.pane_for_agent(agent_id) else {
            return Err(AdapterError::UnknownPane(
                "no pane is registered for this agent".into(),
            ));
        };
        Ok(HerdrState::of(&self.state_for(agent_id, &pane_id)?))
    }
}

impl NotifierAdapter for Adapter {
    fn notify(&self, title: &str, body: &str) -> AdapterResult<()> {
        self.show_notification(title, body)
    }
}

impl AdapterAdmin for Adapter {
    fn version(&self) -> AdapterResult<String> {
        Adapter::version(self)
    }

    /// Removes the adapter's temporary files, prompt files included, so call it only after every agent that reads one
    /// has started.
    fn close(&self) {
        let directory = self.state().prompt_directory.take();
        if let Some(directory) = directory {
            let _ = std::fs::remove_dir_all(directory);
        }
    }

    fn pane_state(&self, pane_id: &str) -> AdapterResult<PaneState> {
        self.pane_state_of(pane_id)
    }

    fn agent_state(&self, name: &str) -> AdapterResult<AgentState> {
        self.agent_state_of(name)
    }

    fn read_input(&self, pane_id: &str) -> AdapterResult<Option<String>> {
        self.input_of(pane_id)
    }

    fn remove_worktree(&self, workspace_id: &str, force: bool) -> AdapterResult<()> {
        self.remove_worktree_of(workspace_id, force)
    }
}
