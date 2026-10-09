//! The interfaces of the Herdr layer, as the rest of the daemon sees them: the runner (src/herdr/runner.ts), the adapter
//! (src/herdr/adapter*.ts) as the two parts the driver and the launcher use (`DriverAdapter` of src/driver.ts,
//! `LauncherAdapter` of src/launcher/shared.ts), the notifier's and process probe's narrow views, and the value types
//! they exchange (including the prompt-relay types of src/herdr/prompt-relay.ts). Recording stubs with scripted results
//! and a call log (test/launcher-stubs.ts, test/herdr-adapter-harness.ts) follow the interfaces.
//!
//! Everything blocks: there is no async runtime, a caller that must not block the kernel runs on its own thread. The
//! interface is a superset on purpose (a package that does not need a method ignores it); a missing method blocks a
//! parallel package.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt;
use std::sync::Mutex;

// ------------------------------------------------------------------------------------------------ names

/// `MAX_TEXT_BYTES`: the longest text typed into a pane.
pub const MAX_TEXT_BYTES: usize = 16 * 1024;

/// Herdr's own rule for an agent name (`HERDR_AGENT_NAME`).
pub const HERDR_AGENT_NAME_MAX: usize = 32;

/// `NAME_PATTERN` (`/^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/`) of src/herdr/adapter-validate.ts: whether `value` is a name
/// Herdr and the adapter accept for an agent.
pub fn is_agent_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..]
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// `shellQuote`: the value in single quotes for a POSIX shell.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

// ------------------------------------------------------------------------------------------------ errors

/// What a callback of the adapter (`beforeSend`, `beforeType`, `discard`) reports when it stops the physical action: the
/// `kind` names the error class of the caller (`StaleActionError` is `stale_action`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookError {
    pub kind: String,
    pub message: String,
}

impl HookError {
    pub fn new(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
        }
    }
}

pub type HookResult = Result<(), HookError>;

/// `HerdrError` of src/herdr/runner.ts: a failure Herdr (or the way it was run) reported, with a code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HerdrError {
    pub code: String,
    pub message: String,
}

impl HerdrError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for HerdrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for HerdrError {}

/// `InputBlocker` of src/herdr/screen.ts: what keeps an unreadable input line from being read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputBlocker {
    PermissionPrompt,
    Dialog,
    Unknown,
}

impl InputBlocker {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PermissionPrompt => "permission_prompt",
            Self::Dialog => "dialog",
            Self::Unknown => "unknown",
        }
    }
}

/// The error classes of src/herdr/adapter-errors.ts, plus the two ways an adapter call fails that are not its own: Herdr
/// itself (`HerdrError`) and a callback that refused. Each class has the `name` the Node class has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdapterError {
    /// A pane move failed and the pane cannot be found at its old id or at a new one.
    PaneLost(String),
    UnknownPane(String),
    PmPane(String),
    Phase(String),
    PaneGone(String),
    AgentPaneMismatch(String),
    DeferralNotElapsed(String),
    NotIdle(String),
    NotBlocked(String),
    InputUnreadable {
        message: String,
        blocker: InputBlocker,
    },
    ClearFailed(String),
    PromptUnrecognized(String),
    ShellNotReady(String),
    DialogStillOpen(String),
    UnsupportedHost(String),
    SendAfterRecord(String),
    InvalidArgument(String),
    Herdr(HerdrError),
    Hook(HookError),
}

impl AdapterError {
    /// The `name` of the Node error class (`HerdrError` for a Herdr failure, the hook's kind for a callback's).
    pub fn name(&self) -> &str {
        match self {
            Self::PaneLost(_) => "PaneLost",
            Self::UnknownPane(_) => "UnknownPaneError",
            Self::PmPane(_) => "PmPaneError",
            Self::Phase(_) => "PhaseError",
            Self::PaneGone(_) => "PaneGone",
            Self::AgentPaneMismatch(_) => "AgentPaneMismatch",
            Self::DeferralNotElapsed(_) => "DeferralNotElapsed",
            Self::NotIdle(_) => "NotIdle",
            Self::NotBlocked(_) => "NotBlocked",
            Self::InputUnreadable { .. } => "InputUnreadable",
            Self::ClearFailed(_) => "ClearFailed",
            Self::PromptUnrecognized(_) => "PromptUnrecognized",
            Self::ShellNotReady(_) => "ShellNotReady",
            Self::DialogStillOpen(_) => "DialogStillOpen",
            Self::UnsupportedHost(_) => "UnsupportedHostError",
            Self::SendAfterRecord(_) => "SendAfterRecordError",
            Self::InvalidArgument(_) => "InvalidArgumentError",
            Self::Herdr(_) => "HerdrError",
            Self::Hook(hook) => &hook.kind,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Self::PaneLost(m)
            | Self::UnknownPane(m)
            | Self::PmPane(m)
            | Self::Phase(m)
            | Self::PaneGone(m)
            | Self::AgentPaneMismatch(m)
            | Self::DeferralNotElapsed(m)
            | Self::NotIdle(m)
            | Self::NotBlocked(m)
            | Self::ClearFailed(m)
            | Self::PromptUnrecognized(m)
            | Self::ShellNotReady(m)
            | Self::DialogStillOpen(m)
            | Self::UnsupportedHost(m)
            | Self::SendAfterRecord(m)
            | Self::InvalidArgument(m) => m,
            Self::InputUnreadable { message, .. } => message,
            Self::Herdr(e) => &e.message,
            Self::Hook(e) => &e.message,
        }
    }

    /// The Herdr error code, when Herdr (and not the adapter) refused.
    pub fn herdr_code(&self) -> Option<&str> {
        match self {
            Self::Herdr(e) => Some(&e.code),
            _ => None,
        }
    }
}

impl fmt::Display for AdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for AdapterError {}

impl From<HerdrError> for AdapterError {
    fn from(error: HerdrError) -> Self {
        Self::Herdr(error)
    }
}

impl From<HookError> for AdapterError {
    fn from(error: HookError) -> Self {
        Self::Hook(error)
    }
}

pub type AdapterResult<T> = Result<T, AdapterError>;

// ------------------------------------------------------------------------------------------------ the runner

/// `HerdrResult`: what one run of the Herdr binary produced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HerdrOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// `RunOptions`, and the bytes handed to Herdr's standard input (Node's runner gives it none; the Rust one may).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunOptions {
    /// Overrides the runner's own limit for one call, for commands that wait on Herdr by design.
    pub timeout_ms: Option<u64>,
    pub stdin: Option<Vec<u8>>,
}

/// `HerdrRunner`: runs `herdr --session <session> <args...>` and returns its output, or the failure to run it
/// (`timeout`, `output_too_large`, `bad_output`).
pub trait HerdrRunner: Send + Sync {
    fn run(&self, args: &[String], options: &RunOptions) -> Result<HerdrOutput, HerdrError>;
}

// ------------------------------------------------------------------------------------------------ value types

/// `HerdrState` of src/controller/messaging.ts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HerdrState {
    Idle,
    Working,
    Blocked,
    Done,
    Unknown,
}

impl HerdrState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Unknown => "unknown",
        }
    }

    /// `herdrStateOf`: anything unrecognized is `unknown`.
    pub fn of(status: &str) -> Self {
        match status {
            "idle" => Self::Idle,
            "working" => Self::Working,
            "blocked" => Self::Blocked,
            "done" => Self::Done,
            _ => Self::Unknown,
        }
    }
}

/// `DeferralReason` of src/controller/messaging.ts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeferralReason {
    AgentBusy,
    AgentBlocked,
    InputNotEmpty,
}

impl DeferralReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentBusy => "agent_busy",
            Self::AgentBlocked => "agent_blocked",
            Self::InputNotEmpty => "input_not_empty",
        }
    }
}

/// `INPUT_UNREADABLE_DETAIL`.
pub const INPUT_UNREADABLE_DETAIL: &str = "the input line is unreadable";

/// `SendOutcome` of `guardedSend`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendOutcome {
    Sent,
    Deferred {
        reason: DeferralReason,
        detail: Option<String>,
        /// Set with `INPUT_UNREADABLE_DETAIL`: what is on the screen instead of the input line.
        blocker: Option<InputBlocker>,
    },
}

/// The answer of `wakePm`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeOutcome {
    Sent,
    PmNotIdle,
    InputNotEmpty,
}

/// `DialogOutcome` of `answerTrustDialog`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogOutcome {
    Handled { keys: Vec<String> },
    Unhandled { reason: String },
}

/// `KeyLogEntry`: one key the adapter sent, with why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyLogEntry {
    pub pane: String,
    pub key: String,
    pub reason: String,
}

/// `KeyLogger`.
pub type KeyLogger<'a> = &'a (dyn Fn(&KeyLogEntry) + Send + Sync);

/// What `clearAfterDeferral` did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClearOutcome {
    pub cleared: bool,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneRole {
    Pm,
    Worker,
}

impl PaneRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pm => "PM",
            Self::Worker => "worker",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanePhase {
    Fresh,
    Prepared,
    Started,
    Tainted,
}

/// `PaneEntry`: what the adapter's registry knows about a pane it created or adopted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneEntry {
    pub role: PaneRole,
    pub phase: PanePhase,
    pub kind: String,
    pub agent: Option<String>,
    pub worktree_path: Option<String>,
    pub workspace_id: Option<String>,
}

/// `PaneIdentity`: who a pane belongs to as Herdr shows it now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneIdentity {
    pub terminal_id: Option<String>,
    /// The `agent` token, when one was reported for the pane.
    pub agent: Option<String>,
    /// The `project` token, when one was reported for the pane.
    pub project: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutPane {
    pub pane_id: String,
    pub width: i64,
    pub height: i64,
}

/// `PaneLayoutView`: the panes of a tab with their sizes in terminal cells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneLayoutView {
    pub tab_id: String,
    pub workspace_id: String,
    pub zoomed: bool,
    pub panes: Vec<LayoutPane>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneAtPath {
    pub pane_id: String,
    pub workspace_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitDirection {
    Right,
    Down,
}

/// What `agentState` answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentState {
    pub status: String,
    pub pane_id: String,
    pub kind: String,
}

/// What `paneState` answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneState {
    pub status: String,
    pub agent: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceCreated {
    pub workspace_id: String,
    pub pane_id: String,
    pub tab_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabCreated {
    pub tab_id: String,
    pub pane_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeCreated {
    pub workspace_id: String,
    pub pane_id: String,
    pub path: String,
    pub branch: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartStatus {
    Started,
    BlockedAtStartup,
}

/// `HOST_KINDS` of src/config: the agent hosts the adapter starts.
pub const HOST_KINDS: [&str; 3] = ["claude", "codex", "omp"];

// The prompt-relay types of src/herdr/prompt-relay.ts.

pub const RELAY_TEXT_MAX_BYTES: usize = 1000;
pub const RELAY_PROMPT_MAX_BYTES: usize = 8192;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayOption {
    /// 1-based, in screen order.
    pub number: i64,
    pub text: String,
    pub accepts_text: bool,
    pub widens_permissions: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapturedPrompt {
    pub agent_id: String,
    pub pane_id: String,
    pub host_kind: String,
    pub text: String,
    pub options: Vec<RelayOption>,
    /// 64 lowercase hex characters.
    pub prompt_sha: String,
    /// True only for an Esc-only relay of an unrecognised blocking dialog; its options are empty.
    pub dialog: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptAnswer {
    Option { number: i64 },
    Esc,
    Text { number: i64, text: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayRefusal {
    NotBlocked,
    UnsupportedHost,
    PromptUnrecognized,
    PromptChanged,
    NoSuchOption,
    NoTextOption,
    TextRefused,
    SelectionNotReached,
    TextFieldNotOpen,
    DialogStillOpen,
}

impl RelayRefusal {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotBlocked => "not_blocked",
            Self::UnsupportedHost => "unsupported_host",
            Self::PromptUnrecognized => "prompt_unrecognized",
            Self::PromptChanged => "prompt_changed",
            Self::NoSuchOption => "no_such_option",
            Self::NoTextOption => "no_text_option",
            Self::TextRefused => "text_refused",
            Self::SelectionNotReached => "selection_not_reached",
            Self::TextFieldNotOpen => "text_field_not_open",
            Self::DialogStillOpen => "dialog_still_open",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureOutcome {
    Captured(CapturedPrompt),
    Refused(RelayRefusal),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelayOutcome {
    Typed {
        keys: Vec<String>,
        /// Dialog relays only: the input line reads again after the Esc.
        input_readable: Option<bool>,
    },
    Refused {
        reason: RelayRefusal,
        /// Keys already sent; empty when nothing was typed.
        keys: Vec<String>,
    },
}

// ------------------------------------------------------------------------------------------------ call inputs

pub struct GuardedSendInput<'a> {
    pub pane_id: &'a str,
    pub text: &'a str,
    pub before_send: &'a mut dyn FnMut() -> HookResult,
}

pub struct WakePmInput<'a> {
    pub pane_id: &'a str,
    pub text: &'a str,
    pub before_send: &'a mut dyn FnMut() -> HookResult,
}

pub struct ClearInput<'a> {
    pub pane_id: &'a str,
    pub deferred_for_ms: f64,
    pub max_deferral_ms: f64,
    pub discard: &'a mut dyn FnMut(&str) -> HookResult,
    pub log: KeyLogger<'a>,
}

pub struct PlacePaneInput<'a> {
    pub pane_id: &'a str,
    pub tab_id: &'a str,
    pub target_pane_id: &'a str,
    pub direction: SplitDirection,
    /// The fraction of the target pane the target keeps.
    pub keep: f64,
    pub worktree_path: &'a str,
}

pub struct CreateWorktreeInput<'a> {
    pub workspace_id: &'a str,
    pub branch: &'a str,
    pub label: &'a str,
    pub base: Option<&'a str>,
}

pub struct CreateWorkspaceInput<'a> {
    pub cwd: &'a str,
    pub label: &'a str,
    pub role: PaneRole,
}

pub struct CreateTabInput<'a> {
    pub workspace_id: &'a str,
    pub cwd: &'a str,
    pub label: &'a str,
    pub role: PaneRole,
}

pub struct PrepareShellInput<'a> {
    pub pane_id: &'a str,
    pub environment: &'a BTreeMap<String, String>,
    pub timeout_ms: Option<u64>,
}

pub struct StartAgentInput<'a> {
    pub name: &'a str,
    pub kind: &'a str,
    pub pane_id: &'a str,
    pub args: &'a [String],
    pub timeout_ms: Option<u64>,
    pub environment: Option<&'a BTreeMap<String, String>>,
}

pub struct AdoptPaneInput<'a> {
    pub pane_id: &'a str,
    pub role: PaneRole,
    pub agent: &'a str,
    pub workspace_id: Option<&'a str>,
    pub worktree_path: Option<&'a str>,
}

pub struct TrustDialogInput<'a> {
    pub pane_id: &'a str,
    pub log: KeyLogger<'a>,
    pub timeout_ms: Option<u64>,
}

pub struct AnswerPromptInput<'a> {
    pub pane_id: &'a str,
    pub prompt_sha: &'a str,
    pub answer: &'a PromptAnswer,
    pub before_type: &'a mut dyn FnMut() -> HookResult,
    pub log: KeyLogger<'a>,
}

/// Where a metadata report goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetadataTarget<'a> {
    Pane(&'a str),
    Workspace(&'a str),
}

// ------------------------------------------------------------------------------------------------ the adapter

/// The three methods the driver and the launcher both use (`paneForAgent`, `paneEntry`, `agentObservation`).
pub trait PaneRegistry: Send + Sync {
    /// The pane the adapter registered for an agent, if any.
    fn pane_for_agent(&self, agent_id: &str) -> Option<String>;
    fn pane_entry(&self, pane_id: &str) -> Option<PaneEntry>;
    /// The mapped Herdr state of an agent on its registered pane; fails when Herdr shows it elsewhere.
    fn agent_observation(&self, agent_id: &str) -> AdapterResult<HerdrState>;
}

/// `DriverAdapter` of src/driver.ts: the part of the Herdr adapter the delivery driver uses.
pub trait DriverAdapter: PaneRegistry {
    fn guarded_send(&self, input: GuardedSendInput<'_>) -> AdapterResult<SendOutcome>;
    fn wake_pm(&self, input: WakePmInput<'_>) -> AdapterResult<WakeOutcome>;
    fn clear_after_deferral(&self, input: ClearInput<'_>) -> AdapterResult<ClearOutcome>;
}

/// `LauncherAdapter` of src/launcher/shared.ts: the adapter methods the launcher uses.
pub trait LauncherAdapter: PaneRegistry {
    fn create_workspace(&self, input: CreateWorkspaceInput<'_>) -> AdapterResult<WorkspaceCreated>;
    fn create_worktree(&self, input: CreateWorktreeInput<'_>) -> AdapterResult<WorktreeCreated>;
    fn create_tab(&self, input: CreateTabInput<'_>) -> AdapterResult<TabCreated>;
    fn pane_layout(&self, pane_id: &str) -> AdapterResult<PaneLayoutView>;
    fn place_pane(&self, input: PlacePaneInput<'_>) -> AdapterResult<PaneAtPath>;
    /// The panes whose working directory is exactly `directory`, with their workspaces.
    fn panes_at_path(&self, directory: &str) -> AdapterResult<Vec<PaneAtPath>>;
    fn prepare_shell(&self, input: PrepareShellInput<'_>) -> AdapterResult<()>;
    fn start_agent(&self, input: StartAgentInput<'_>) -> AdapterResult<StartStatus>;
    fn answer_trust_dialog(&self, input: TrustDialogInput<'_>) -> AdapterResult<DialogOutcome>;
    fn close_pane(&self, pane_id: &str) -> AdapterResult<()>;
    /// None when Herdr has no such pane.
    fn pane_identity(&self, pane_id: &str) -> AdapterResult<Option<PaneIdentity>>;
    fn adopt_pane(&self, input: AdoptPaneInput<'_>) -> AdapterResult<()>;
    fn adopt_shell_pane(&self, pane_id: &str, workspace_id: Option<&str>) -> AdapterResult<()>;
    /// Display-only metadata for the operator's sidebar; the caller treats a failure as non-fatal.
    fn report_metadata(
        &self,
        target: MetadataTarget<'_>,
        tokens: &BTreeMap<String, String>,
    ) -> AdapterResult<()>;
    fn rename_workspace(&self, workspace_id: &str, label: &str) -> AdapterResult<()>;
    fn rename_tab(&self, tab_id: &str, label: &str) -> AdapterResult<()>;
    /// Drops a pane from the registry without touching Herdr.
    fn forget_pane(&self, pane_id: &str);
    fn run_in_pane(&self, pane_id: &str, command: &str) -> AdapterResult<()>;
    /// Writes a prompt file private to the adapter and returns its path.
    fn write_prompt_file(&self, text: &str) -> AdapterResult<String>;
    fn read_screen(&self, pane_id: &str, ansi: bool, lines: Option<usize>)
        -> AdapterResult<String>;
    fn capture_prompt(&self, pane_id: &str) -> AdapterResult<CaptureOutcome>;
    fn answer_prompt(&self, input: AnswerPromptInput<'_>) -> AdapterResult<RelayOutcome>;
    /// Sends one Esc to a worker Herdr shows working; true when sent.
    fn interrupt_working(&self, pane_id: &str, log: KeyLogger<'_>) -> AdapterResult<bool>;
}

/// `NotifierAdapter` of src/notifier.ts: shows a notification in Herdr.
pub trait NotifierAdapter: Send + Sync {
    fn notify(&self, title: &str, body: &str) -> AdapterResult<()>;
}

/// The rest of the adapter's public surface (`version`, `close`, `paneState`, `agentState`, `readInput`,
/// `removeWorktree`), used by the daemon's entry point and by tools.
pub trait AdapterAdmin: Send + Sync {
    fn version(&self) -> AdapterResult<String>;
    /// Removes the adapter's temporary files, prompt files included.
    fn close(&self);
    fn pane_state(&self, pane_id: &str) -> AdapterResult<PaneState>;
    fn agent_state(&self, name: &str) -> AdapterResult<AgentState>;
    fn read_input(&self, pane_id: &str) -> AdapterResult<Option<String>>;
    fn remove_worktree(&self, workspace_id: &str, force: bool) -> AdapterResult<()>;
}

/// The whole adapter: what `HerdrAdapter` is in Node, and what `DriverAdapter & LauncherAdapter` is in `DaemonOptions`.
pub trait HerdrAdapter: DriverAdapter + LauncherAdapter + NotifierAdapter + AdapterAdmin {}

impl<T: DriverAdapter + LauncherAdapter + NotifierAdapter + AdapterAdmin> HerdrAdapter for T {}

// ------------------------------------------------------------------------------------------------ process probe

/// One process of a pane's process tree.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessEntry {
    pub pid: i64,
    pub ppid: i64,
    pub comm: String,
    /// CPU time used so far, in clock ticks or seconds as the sampler reads it.
    pub cpu_seconds: f64,
}

/// `ProcessSample`: the processes under a pane's shell, as `ProcessActivityProbe.sample` reads them.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessSample {
    pub shell_pid: Option<i64>,
    pub processes: Vec<ProcessEntry>,
}

/// `ProcessActivityProbe` of src/herdr/process-activity.ts: samples the processes under a pane. Sampling reads /proc or
/// runs `ps`, so it never runs inside a kernel closure.
pub trait ProcessActivityProbe: Send + Sync {
    fn sample(&self, pane_id: &str) -> Result<ProcessSample, HerdrError>;
}

// ------------------------------------------------------------------------------------------------ stubs

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A scripted `HerdrRunner` with a call log: each call takes the next scripted result (a call after the script ends
/// answers `default`).
pub struct StubRunner {
    state: Mutex<StubRunnerState>,
}

struct StubRunnerState {
    script: VecDeque<Result<HerdrOutput, HerdrError>>,
    default: Result<HerdrOutput, HerdrError>,
    calls: Vec<(Vec<String>, RunOptions)>,
}

impl Default for StubRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl StubRunner {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(StubRunnerState {
                script: VecDeque::new(),
                default: Ok(HerdrOutput::default()),
                calls: Vec::new(),
            }),
        }
    }

    /// Queues the answer of the next call.
    pub fn push(&self, result: Result<HerdrOutput, HerdrError>) -> &Self {
        lock(&self.state).script.push_back(result);
        self
    }

    /// Queues a successful answer with this standard output.
    pub fn push_stdout(&self, stdout: &str) -> &Self {
        self.push(Ok(HerdrOutput {
            code: 0,
            stdout: stdout.to_string(),
            stderr: String::new(),
        }))
    }

    /// What a call answers once the script is empty.
    pub fn set_default(&self, result: Result<HerdrOutput, HerdrError>) {
        lock(&self.state).default = result;
    }

    /// The arguments of every call so far, in order.
    pub fn calls(&self) -> Vec<Vec<String>> {
        lock(&self.state)
            .calls
            .iter()
            .map(|(args, _)| args.clone())
            .collect()
    }

    /// Every call with its options.
    pub fn calls_with_options(&self) -> Vec<(Vec<String>, RunOptions)> {
        lock(&self.state).calls.clone()
    }
}

impl HerdrRunner for StubRunner {
    fn run(&self, args: &[String], options: &RunOptions) -> Result<HerdrOutput, HerdrError> {
        let mut state = lock(&self.state);
        state.calls.push((args.to_vec(), options.clone()));
        match state.script.pop_front() {
            Some(result) => result,
            None => state.default.clone(),
        }
    }
}

/// A recording adapter with scripted results, the Rust form of test/launcher-stubs.ts `StubAdapter`: every method
/// appends `method arg ...` to the call log, takes a scripted error first (`fail_next`), and otherwise answers the way
/// the Node stub does (counters for ids, the agent registry, `observation` for the agent state).
pub struct StubAdapter {
    state: Mutex<StubAdapterState>,
}

struct StubAdapterState {
    calls: Vec<String>,
    errors: HashMap<String, VecDeque<AdapterError>>,
    entries: HashMap<String, PaneEntry>,
    agent_panes: HashMap<String, String>,
    counter: u64,
    observation: HerdrState,
    start_status: StartStatus,
    dialog_handled: bool,
    prompts: Vec<String>,
    capture: CaptureOutcome,
    relay: RelayOutcome,
    send: SendOutcome,
    wake: WakeOutcome,
    interrupted: bool,
    layout: PaneLayoutView,
    panes_at: HashMap<String, Vec<PaneAtPath>>,
    worktree_base: String,
}

impl Default for StubAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl StubAdapter {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(StubAdapterState {
                calls: Vec::new(),
                errors: HashMap::new(),
                entries: HashMap::new(),
                agent_panes: HashMap::new(),
                counter: 0,
                observation: HerdrState::Idle,
                start_status: StartStatus::Started,
                dialog_handled: true,
                prompts: Vec::new(),
                capture: CaptureOutcome::Refused(RelayRefusal::NotBlocked),
                relay: RelayOutcome::Refused {
                    reason: RelayRefusal::NotBlocked,
                    keys: Vec::new(),
                },
                send: SendOutcome::Deferred {
                    reason: DeferralReason::AgentBusy,
                    detail: None,
                    blocker: None,
                },
                wake: WakeOutcome::PmNotIdle,
                interrupted: false,
                layout: PaneLayoutView {
                    tab_id: "w1:t1".into(),
                    workspace_id: "w1".into(),
                    zoomed: false,
                    panes: Vec::new(),
                },
                panes_at: HashMap::new(),
                worktree_base: "/tmp/work".into(),
            }),
        }
    }

    /// The call log, one `method arg ...` line per call.
    pub fn calls(&self) -> Vec<String> {
        lock(&self.state).calls.clone()
    }

    /// The next call of `method` (its snake_case name) fails with `error`; several calls queue.
    pub fn fail_next(&self, method: &str, error: AdapterError) {
        lock(&self.state)
            .errors
            .entry(method.to_string())
            .or_default()
            .push_back(error);
    }

    pub fn set_observation(&self, state: HerdrState) {
        lock(&self.state).observation = state;
    }

    pub fn set_start_status(&self, status: StartStatus) {
        lock(&self.state).start_status = status;
    }

    pub fn set_dialog_handled(&self, handled: bool) {
        lock(&self.state).dialog_handled = handled;
    }

    pub fn set_capture(&self, outcome: CaptureOutcome) {
        lock(&self.state).capture = outcome;
    }

    pub fn set_relay(&self, outcome: RelayOutcome) {
        lock(&self.state).relay = outcome;
    }

    pub fn set_send(&self, outcome: SendOutcome) {
        lock(&self.state).send = outcome;
    }

    pub fn set_wake(&self, outcome: WakeOutcome) {
        lock(&self.state).wake = outcome;
    }

    pub fn set_layout(&self, layout: PaneLayoutView) {
        lock(&self.state).layout = layout;
    }

    pub fn set_worktree_base(&self, base: &str) {
        lock(&self.state).worktree_base = base.to_string();
    }

    /// Panes `panes_at_path` reports for a directory.
    pub fn set_panes_at(&self, directory: &str, panes: Vec<PaneAtPath>) {
        lock(&self.state)
            .panes_at
            .insert(directory.to_string(), panes);
    }

    /// The prompt files written so far.
    pub fn prompts(&self) -> Vec<String> {
        lock(&self.state).prompts.clone()
    }

    /// Registers a pane as an earlier adapter instance would have.
    pub fn register(&self, pane_id: &str, entry: PaneEntry) {
        let mut state = lock(&self.state);
        if let Some(agent) = &entry.agent {
            state.agent_panes.insert(agent.clone(), pane_id.to_string());
        }
        state.entries.insert(pane_id.to_string(), entry);
    }

    fn record(&self, line: String, method: &str) -> AdapterResult<()> {
        self.enter(line, method).map(drop)
    }

    fn enter(
        &self,
        line: String,
        method: &str,
    ) -> AdapterResult<std::sync::MutexGuard<'_, StubAdapterState>> {
        let mut state = lock(&self.state);
        state.calls.push(line);
        if let Some(error) = state
            .errors
            .get_mut(method)
            .and_then(|queue| queue.pop_front())
        {
            return Err(error);
        }
        Ok(state)
    }

    fn fresh(state: &mut StubAdapterState, prefix: &str) -> String {
        state.counter += 1;
        format!("{prefix}{}", state.counter)
    }
}

impl PaneRegistry for StubAdapter {
    fn pane_for_agent(&self, agent_id: &str) -> Option<String> {
        lock(&self.state).agent_panes.get(agent_id).cloned()
    }

    fn pane_entry(&self, pane_id: &str) -> Option<PaneEntry> {
        lock(&self.state).entries.get(pane_id).cloned()
    }

    fn agent_observation(&self, agent_id: &str) -> AdapterResult<HerdrState> {
        let state = self.enter(format!("agent_observation {agent_id}"), "agent_observation")?;
        if !state.agent_panes.contains_key(agent_id) {
            return Err(AdapterError::UnknownPane(
                "no pane is registered for this agent".into(),
            ));
        }
        Ok(state.observation)
    }
}

impl DriverAdapter for StubAdapter {
    fn guarded_send(&self, input: GuardedSendInput<'_>) -> AdapterResult<SendOutcome> {
        let state = self.enter(
            format!("guarded_send {} {}", input.pane_id, input.text),
            "guarded_send",
        )?;
        let outcome = state.send.clone();
        drop(state);
        if outcome == SendOutcome::Sent {
            (input.before_send)()?;
        }
        Ok(outcome)
    }

    fn wake_pm(&self, input: WakePmInput<'_>) -> AdapterResult<WakeOutcome> {
        let state = self.enter(
            format!("wake_pm {} {}", input.pane_id, input.text),
            "wake_pm",
        )?;
        let outcome = state.wake;
        drop(state);
        if outcome == WakeOutcome::Sent {
            (input.before_send)()?;
        }
        Ok(outcome)
    }

    fn clear_after_deferral(&self, input: ClearInput<'_>) -> AdapterResult<ClearOutcome> {
        self.record(
            format!("clear_after_deferral {}", input.pane_id),
            "clear_after_deferral",
        )?;
        Ok(ClearOutcome {
            cleared: false,
            text: String::new(),
        })
    }
}

impl LauncherAdapter for StubAdapter {
    fn create_workspace(&self, input: CreateWorkspaceInput<'_>) -> AdapterResult<WorkspaceCreated> {
        let mut state = self.enter(
            format!(
                "create_workspace {} {} {}",
                input.cwd,
                input.label,
                input.role.as_str()
            ),
            "create_workspace",
        )?;
        let workspace_id = Self::fresh(&mut state, "w");
        let pane_id = format!("{workspace_id}:p1");
        let tab_id = format!("{workspace_id}:t1");
        state.entries.insert(
            pane_id.clone(),
            PaneEntry {
                role: input.role,
                phase: PanePhase::Fresh,
                kind: "shell".into(),
                agent: None,
                worktree_path: None,
                workspace_id: Some(workspace_id.clone()),
            },
        );
        Ok(WorkspaceCreated {
            workspace_id,
            pane_id,
            tab_id,
        })
    }

    fn create_worktree(&self, input: CreateWorktreeInput<'_>) -> AdapterResult<WorktreeCreated> {
        let mut state = self.enter(
            format!(
                "create_worktree {} {} {} {}",
                input.workspace_id,
                input.branch,
                input.label,
                input.base.unwrap_or("-")
            ),
            "create_worktree",
        )?;
        let workspace_id = Self::fresh(&mut state, "w");
        let pane_id = format!("{workspace_id}:p1");
        let path = format!("{}/{}", state.worktree_base, input.branch);
        state.entries.insert(
            pane_id.clone(),
            PaneEntry {
                role: PaneRole::Worker,
                phase: PanePhase::Fresh,
                kind: "shell".into(),
                agent: None,
                worktree_path: Some(path.clone()),
                workspace_id: Some(workspace_id.clone()),
            },
        );
        Ok(WorktreeCreated {
            workspace_id,
            pane_id,
            path,
            branch: input.branch.to_string(),
        })
    }

    fn create_tab(&self, input: CreateTabInput<'_>) -> AdapterResult<TabCreated> {
        let mut state = self.enter(
            format!(
                "create_tab {} {} {}",
                input.workspace_id, input.cwd, input.label
            ),
            "create_tab",
        )?;
        let tab = Self::fresh(&mut state, "t");
        let pane_id = format!("{}:p{}", input.workspace_id, state.counter);
        state.entries.insert(
            pane_id.clone(),
            PaneEntry {
                role: input.role,
                phase: PanePhase::Fresh,
                kind: "shell".into(),
                agent: None,
                worktree_path: None,
                workspace_id: Some(input.workspace_id.to_string()),
            },
        );
        Ok(TabCreated {
            tab_id: format!("{}:{tab}", input.workspace_id),
            pane_id,
        })
    }

    fn pane_layout(&self, pane_id: &str) -> AdapterResult<PaneLayoutView> {
        let state = self.enter(format!("pane_layout {pane_id}"), "pane_layout")?;
        Ok(state.layout.clone())
    }

    fn place_pane(&self, input: PlacePaneInput<'_>) -> AdapterResult<PaneAtPath> {
        let mut state = self.enter(
            format!(
                "place_pane {} {} {} {:?} {}",
                input.pane_id, input.tab_id, input.target_pane_id, input.direction, input.keep
            ),
            "place_pane",
        )?;
        state.counter += 1;
        let workspace_id = input.tab_id.split(':').next().unwrap_or("w1").to_string();
        let moved = format!("{workspace_id}:p{}", 100 + state.counter);
        if let Some(entry) = state.entries.remove(input.pane_id) {
            if let Some(agent) = &entry.agent {
                state.agent_panes.insert(agent.clone(), moved.clone());
            }
            state.entries.insert(moved.clone(), entry);
        }
        Ok(PaneAtPath {
            pane_id: moved,
            workspace_id,
        })
    }

    fn panes_at_path(&self, directory: &str) -> AdapterResult<Vec<PaneAtPath>> {
        let state = self.enter(format!("panes_at_path {directory}"), "panes_at_path")?;
        Ok(state.panes_at.get(directory).cloned().unwrap_or_default())
    }

    fn prepare_shell(&self, input: PrepareShellInput<'_>) -> AdapterResult<()> {
        let mut state = self.enter(format!("prepare_shell {}", input.pane_id), "prepare_shell")?;
        if let Some(entry) = state.entries.get_mut(input.pane_id) {
            entry.phase = PanePhase::Prepared;
        }
        Ok(())
    }

    fn start_agent(&self, input: StartAgentInput<'_>) -> AdapterResult<StartStatus> {
        let mut state = self.enter(
            format!(
                "start_agent {} {} {} {}",
                input.name,
                input.kind,
                input.pane_id,
                input.args.join(" ")
            ),
            "start_agent",
        )?;
        let status = state.start_status;
        if let Some(entry) = state.entries.get_mut(input.pane_id) {
            entry.phase = PanePhase::Started;
            entry.kind = input.kind.to_string();
            entry.agent = Some(input.name.to_string());
        }
        state
            .agent_panes
            .insert(input.name.to_string(), input.pane_id.to_string());
        Ok(status)
    }

    fn answer_trust_dialog(&self, input: TrustDialogInput<'_>) -> AdapterResult<DialogOutcome> {
        let state = self.enter(
            format!("answer_trust_dialog {}", input.pane_id),
            "answer_trust_dialog",
        )?;
        Ok(if state.dialog_handled {
            DialogOutcome::Handled { keys: Vec::new() }
        } else {
            DialogOutcome::Unhandled {
                reason: "no dialog".into(),
            }
        })
    }

    fn close_pane(&self, pane_id: &str) -> AdapterResult<()> {
        let mut state = self.enter(format!("close_pane {pane_id}"), "close_pane")?;
        if let Some(entry) = state.entries.remove(pane_id) {
            if let Some(agent) = entry.agent {
                state.agent_panes.remove(&agent);
            }
        }
        Ok(())
    }

    fn pane_identity(&self, pane_id: &str) -> AdapterResult<Option<PaneIdentity>> {
        let state = self.enter(format!("pane_identity {pane_id}"), "pane_identity")?;
        Ok(state.entries.get(pane_id).map(|entry| PaneIdentity {
            terminal_id: Some(format!("term:{pane_id}")),
            agent: entry.agent.clone(),
            project: None,
        }))
    }

    fn adopt_pane(&self, input: AdoptPaneInput<'_>) -> AdapterResult<()> {
        let mut state = self.enter(
            format!("adopt_pane {} {}", input.pane_id, input.agent),
            "adopt_pane",
        )?;
        state.entries.insert(
            input.pane_id.to_string(),
            PaneEntry {
                role: input.role,
                phase: PanePhase::Started,
                kind: "claude".into(),
                agent: Some(input.agent.to_string()),
                worktree_path: input.worktree_path.map(str::to_string),
                workspace_id: input.workspace_id.map(str::to_string),
            },
        );
        state
            .agent_panes
            .insert(input.agent.to_string(), input.pane_id.to_string());
        Ok(())
    }

    fn adopt_shell_pane(&self, pane_id: &str, workspace_id: Option<&str>) -> AdapterResult<()> {
        let mut state = self.enter(format!("adopt_shell_pane {pane_id}"), "adopt_shell_pane")?;
        state.entries.insert(
            pane_id.to_string(),
            PaneEntry {
                role: PaneRole::Worker,
                phase: PanePhase::Started,
                kind: "shell".into(),
                agent: None,
                worktree_path: None,
                workspace_id: workspace_id.map(str::to_string),
            },
        );
        Ok(())
    }

    fn report_metadata(
        &self,
        target: MetadataTarget<'_>,
        tokens: &BTreeMap<String, String>,
    ) -> AdapterResult<()> {
        let rendered: Vec<String> = tokens.iter().map(|(k, v)| format!("{k}={v}")).collect();
        self.record(
            format!("report_metadata {target:?} {}", rendered.join(",")),
            "report_metadata",
        )?;
        Ok(())
    }

    fn rename_workspace(&self, workspace_id: &str, label: &str) -> AdapterResult<()> {
        self.record(
            format!("rename_workspace {workspace_id} {label}"),
            "rename_workspace",
        )?;
        Ok(())
    }

    fn rename_tab(&self, tab_id: &str, label: &str) -> AdapterResult<()> {
        self.record(format!("rename_tab {tab_id} {label}"), "rename_tab")?;
        Ok(())
    }

    fn forget_pane(&self, pane_id: &str) {
        let mut state = lock(&self.state);
        state.calls.push(format!("forget_pane {pane_id}"));
        if let Some(entry) = state.entries.remove(pane_id) {
            if let Some(agent) = entry.agent {
                state.agent_panes.remove(&agent);
            }
        }
    }

    fn run_in_pane(&self, pane_id: &str, command: &str) -> AdapterResult<()> {
        let mut state = self.enter(format!("run_in_pane {pane_id} {command}"), "run_in_pane")?;
        if let Some(entry) = state.entries.get_mut(pane_id) {
            entry.phase = PanePhase::Started;
        }
        Ok(())
    }

    fn write_prompt_file(&self, text: &str) -> AdapterResult<String> {
        let mut state = self.enter("write_prompt_file".to_string(), "write_prompt_file")?;
        state.prompts.push(text.to_string());
        Ok(format!("/tmp/capstan-prompts/{}.md", state.prompts.len()))
    }

    fn read_screen(
        &self,
        pane_id: &str,
        ansi: bool,
        lines: Option<usize>,
    ) -> AdapterResult<String> {
        self.record(
            format!("read_screen {pane_id} {ansi} {lines:?}"),
            "read_screen",
        )?;
        Ok(String::new())
    }

    fn capture_prompt(&self, pane_id: &str) -> AdapterResult<CaptureOutcome> {
        let state = self.enter(format!("capture_prompt {pane_id}"), "capture_prompt")?;
        Ok(state.capture.clone())
    }

    fn answer_prompt(&self, input: AnswerPromptInput<'_>) -> AdapterResult<RelayOutcome> {
        let state = self.enter(
            format!(
                "answer_prompt {} {} {:?}",
                input.pane_id, input.prompt_sha, input.answer
            ),
            "answer_prompt",
        )?;
        let outcome = state.relay.clone();
        drop(state);
        if matches!(outcome, RelayOutcome::Typed { .. }) {
            (input.before_type)()?;
        }
        Ok(outcome)
    }

    fn interrupt_working(&self, pane_id: &str, _log: KeyLogger<'_>) -> AdapterResult<bool> {
        let mut state = self.enter(format!("interrupt_working {pane_id}"), "interrupt_working")?;
        state.interrupted = true;
        Ok(state.observation == HerdrState::Working)
    }
}

impl NotifierAdapter for StubAdapter {
    fn notify(&self, title: &str, body: &str) -> AdapterResult<()> {
        self.record(format!("notify {title} {body}"), "notify")?;
        Ok(())
    }
}

impl AdapterAdmin for StubAdapter {
    fn version(&self) -> AdapterResult<String> {
        self.record("version".to_string(), "version")?;
        Ok("0.9.1".into())
    }

    fn close(&self) {
        lock(&self.state).calls.push("close".to_string());
    }

    fn pane_state(&self, pane_id: &str) -> AdapterResult<PaneState> {
        let state = self.enter(format!("pane_state {pane_id}"), "pane_state")?;
        Ok(PaneState {
            status: state.observation.as_str().to_string(),
            agent: state.entries.get(pane_id).and_then(|e| e.agent.clone()),
        })
    }

    fn agent_state(&self, name: &str) -> AdapterResult<AgentState> {
        let state = self.enter(format!("agent_state {name}"), "agent_state")?;
        let pane_id = state.agent_panes.get(name).cloned().unwrap_or_default();
        Ok(AgentState {
            status: state.observation.as_str().to_string(),
            pane_id,
            kind: "claude".into(),
        })
    }

    fn read_input(&self, pane_id: &str) -> AdapterResult<Option<String>> {
        self.record(format!("read_input {pane_id}"), "read_input")?;
        Ok(Some(String::new()))
    }

    fn remove_worktree(&self, workspace_id: &str, force: bool) -> AdapterResult<()> {
        self.record(
            format!("remove_worktree {workspace_id} {force}"),
            "remove_worktree",
        )?;
        Ok(())
    }
}

/// A scripted `ProcessActivityProbe`: every pane answers the sample set for it, or an empty one.
#[derive(Default)]
pub struct StubProbe {
    samples: Mutex<HashMap<String, ProcessSample>>,
    calls: Mutex<Vec<String>>,
}

impl StubProbe {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, pane_id: &str, sample: ProcessSample) {
        lock(&self.samples).insert(pane_id.to_string(), sample);
    }

    pub fn calls(&self) -> Vec<String> {
        lock(&self.calls).clone()
    }
}

impl ProcessActivityProbe for StubProbe {
    fn sample(&self, pane_id: &str) -> Result<ProcessSample, HerdrError> {
        lock(&self.calls).push(pane_id.to_string());
        Ok(lock(&self.samples)
            .get(pane_id)
            .cloned()
            .unwrap_or(ProcessSample {
                shell_pid: None,
                processes: Vec::new(),
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_names_follow_the_adapter_pattern() {
        assert!(is_agent_name("dev-1"));
        assert!(is_agent_name("A.b_c-9"));
        assert!(!is_agent_name(""));
        assert!(!is_agent_name("-x"));
        assert!(!is_agent_name("a b"));
        assert!(is_agent_name(&"a".repeat(64)));
        assert!(!is_agent_name(&"a".repeat(65)));
    }

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn stub_runner_scripts_results_and_logs_calls() {
        let runner = StubRunner::new();
        runner.push_stdout("{}");
        runner.push(Err(HerdrError::new("timeout", "herdr x timed out")));
        let args = vec!["pane".to_string(), "get".to_string()];
        assert_eq!(
            runner.run(&args, &RunOptions::default()).unwrap().stdout,
            "{}"
        );
        assert_eq!(
            runner.run(&args, &RunOptions::default()).unwrap_err().code,
            "timeout"
        );
        assert_eq!(runner.run(&args, &RunOptions::default()).unwrap().code, 0);
        assert_eq!(runner.calls().len(), 3);
    }

    #[test]
    fn stub_adapter_registers_started_agents_and_scripts_failures() {
        let adapter = StubAdapter::new();
        let workspace = adapter
            .create_workspace(CreateWorkspaceInput {
                cwd: "/p",
                label: "l",
                role: PaneRole::Pm,
            })
            .unwrap();
        let env = BTreeMap::new();
        adapter
            .prepare_shell(PrepareShellInput {
                pane_id: &workspace.pane_id,
                environment: &env,
                timeout_ms: None,
            })
            .unwrap();
        let status = adapter
            .start_agent(StartAgentInput {
                name: "pm-1",
                kind: "claude",
                pane_id: &workspace.pane_id,
                args: &[],
                timeout_ms: None,
                environment: None,
            })
            .unwrap();
        assert_eq!(status, StartStatus::Started);
        assert_eq!(
            adapter.pane_for_agent("pm-1"),
            Some(workspace.pane_id.clone())
        );
        assert_eq!(adapter.agent_observation("pm-1").unwrap(), HerdrState::Idle);
        adapter.fail_next("close_pane", AdapterError::PaneGone("gone".into()));
        assert_eq!(
            adapter.close_pane(&workspace.pane_id).unwrap_err().name(),
            "PaneGone"
        );
        adapter.close_pane(&workspace.pane_id).unwrap();
        assert_eq!(adapter.pane_for_agent("pm-1"), None);
    }

    #[test]
    fn herdr_states_map_like_the_adapter() {
        assert_eq!(HerdrState::of("working"), HerdrState::Working);
        assert_eq!(HerdrState::of("strange"), HerdrState::Unknown);
    }
}
