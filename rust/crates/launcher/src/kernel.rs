//! The state and helpers every launcher operation shares (src/launcher/kernel.ts), and the launcher's access to the
//! controller's ledger.
//!
//! # The ledger port
//!
//! `capstan_kernel::Core` lives on one thread and is reached by posting closures to it (the daemon's `KernelHandle`).
//! The daemon crate depends on this one, so the launcher cannot name the handle: it takes a `LedgerPort`, which the daemon
//! implements for its handle, and `Ledger` wraps it in the typed calls of `ControllerCore` the launcher uses. Every call
//! is one closure on the kernel thread, so Herdr, git and setup commands never run inside one.

use crate::api::{MAX_WAITING_OPERATIONS, STEP_BUDGET_MS};
use crate::git::DefaultGit;
use crate::setup::run_setup_command;
use crate::shared::*;
use capstan_config::prompts::{
    ArchitectInput, NexoraInput, OperatorInput, ResearcherInput, CSTAN_ALLOW_RULE,
};
use capstan_config::{Role, RoleConfig, DEFAULT_WAIT_TIMEOUT_SECONDS};
use capstan_herdr::api::{AdapterError, LauncherAdapter, MetadataTarget};
use capstan_herdr::claude_args::McpServer as HerdrMcpServer;
use capstan_herdr::claude_args::{build_agent_environment, claude_arguments, ClaudeRoleSettings};
use capstan_herdr::hosts::{codex_arguments, omp_arguments, HostRoleSettings};
use capstan_herdr::naming::project_display_name;
use capstan_kernel::types::MutationContext;
use capstan_kernel::{Core, KernelError, KernelResult};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex, MutexGuard};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ------------------------------------------------------------------------------------------------ the ledger port

/// The kernel thread is gone (closed, or it failed to open).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KernelGone;

/// Runs a closure on the thread that owns the `Core`. Each call is one ledger step: one `KernelHandle::call` on the kernel
/// thread, so no Herdr, git or worktree I/O happens while the kernel is held (the one-kernel-thread rule of the daemon).
/// The implementation for the daemon's `KernelHandle` lives in `capstan-daemon` (the orphan rule: that crate depends on this
/// one), written when the launcher is wired in; tests use their own port over a seeded `Core`.
pub trait LedgerPort: Send + Sync {
    fn run_job(&self, job: Box<dyn FnOnce(&Core) + Send>) -> Result<(), KernelGone>;
}

fn closed() -> KernelError {
    KernelError::controller("controller is closed")
}

/// `newContext`: a context for exactly one mutation, built at the moment of the call; the id comes from the kernel's
/// `Env` (one uuid), the counters from the ledger.
fn new_context(core: &Core, credential: &str) -> KernelResult<MutationContext> {
    let id = core.kernel().env.uuid();
    Ok(MutationContext {
        credential: credential.to_string(),
        request_id: format!("req-{id}"),
        idempotency_key: format!("idem-{id}"),
        expected_version: core.state_version()?,
        input_revision: core.input_revision()?,
    })
}

fn objects(value: Value) -> Vec<Value> {
    match value {
        Value::Array(items) => items,
        _ => Vec::new(),
    }
}

/// The calls of `ControllerCore` the launcher makes, with the project credential it makes them under.
#[derive(Clone)]
pub struct Ledger {
    port: Arc<dyn LedgerPort>,
    credential: String,
}

impl Ledger {
    pub fn new(port: Arc<dyn LedgerPort>, credential: impl Into<String>) -> Self {
        Self {
            port,
            credential: credential.into(),
        }
    }

    /// Runs `f` on the kernel thread and waits for its answer.
    pub fn call<R, F>(&self, f: F) -> KernelResult<R>
    where
        R: Send + 'static,
        F: FnOnce(&Core) -> KernelResult<R> + Send + 'static,
    {
        let (answer_tx, answer_rx) = channel();
        self.port
            .run_job(Box::new(move |core| {
                let _ = answer_tx.send(f(core));
            }))
            .map_err(|_| closed())?;
        answer_rx.recv().map_err(|_| closed())?
    }

    /// `call` for a mutation: the context is made on the kernel thread right before `f` runs.
    pub fn mutate<R, F>(&self, f: F) -> KernelResult<R>
    where
        R: Send + 'static,
        F: FnOnce(&Core, &MutationContext) -> KernelResult<R> + Send + 'static,
    {
        let credential = self.credential.clone();
        self.call(move |core| {
            let context = new_context(core, &credential)?;
            f(core, &context)
        })
    }

    /// One uuid from the kernel's `Env` (what `randomUUID()` is in Node).
    pub fn uuid(&self) -> KernelResult<String> {
        self.call(|core| Ok(core.kernel().env.uuid()))
    }

    pub fn active_agents(&self) -> KernelResult<Vec<AgentRec>> {
        let value = self.call(|core| core.active_agents())?;
        Ok(objects(value)
            .iter()
            .filter_map(AgentRec::from_value)
            .collect())
    }

    pub fn list_agents(&self) -> KernelResult<Vec<AgentRec>> {
        let value = self.call(|core| core.list_agents())?;
        Ok(objects(value)
            .iter()
            .filter_map(AgentRec::from_value)
            .collect())
    }

    pub fn agent_record(&self, agent_id: &str) -> KernelResult<Option<AgentRec>> {
        let id = agent_id.to_string();
        let value = self.call(move |core| core.agent_record(&id))?;
        Ok(AgentRec::from_value(&value))
    }

    /// `statusSnapshot().roles`: the seats.
    pub fn seats(&self) -> KernelResult<Vec<Value>> {
        let snapshot = self.call(|core| core.status_snapshot())?;
        Ok(objects(
            snapshot.get("roles").cloned().unwrap_or(Value::Null),
        ))
    }

    pub fn create_seat(&self, seat_id: &str, name: &str, role: &str) -> KernelResult<()> {
        let input = json!({"seatId": seat_id, "name": name, "role": role});
        self.mutate(move |core, context| core.create_seat(context, &input))
            .map(drop)
    }

    pub fn role_definitions(&self) -> KernelResult<Vec<Value>> {
        Ok(objects(self.call(|core| core.role_definitions())?))
    }

    pub fn seat_actor_ids(&self, seat_id: &str) -> KernelResult<Vec<String>> {
        let credential = self.credential.clone();
        let seat = seat_id.to_string();
        let value = self.call(move |core| core.seat_actor_ids(&credential, &seat))?;
        Ok(objects(value)
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect())
    }

    pub fn revoke_actor(&self, actor_id: &str) -> KernelResult<()> {
        let actor = actor_id.to_string();
        self.mutate(move |core, context| core.revoke_actor(context, &actor))
            .map(drop)
    }

    /// `createActor`: the new actor's id and credential.
    pub fn create_actor(
        &self,
        display_name: &str,
        role: &str,
        seat_id: &str,
    ) -> KernelResult<(String, String)> {
        let input = json!({"displayName": display_name, "role": role, "seatId": seat_id});
        let value = self.mutate(move |core, context| core.create_actor(context, &input))?;
        Ok((
            text(&value, "actorId").unwrap_or_default(),
            text(&value, "credential").unwrap_or_default(),
        ))
    }

    pub fn register_agent(
        &self,
        agent_id: &str,
        role_name: &str,
        seat_id: &str,
        actor_id: &str,
    ) -> KernelResult<()> {
        let input = json!({"agentId": agent_id, "roleName": role_name, "seatId": seat_id, "actorId": actor_id});
        self.mutate(move |core, context| core.register_agent(context, &input))
            .map(drop)
    }

    pub fn agent_panes(&self) -> KernelResult<Vec<PaneRow>> {
        let credential = self.credential.clone();
        let value = self.call(move |core| core.agent_panes(&credential))?;
        Ok(objects(value)
            .iter()
            .filter_map(PaneRow::from_value)
            .collect())
    }

    pub fn pane_terminal_id(&self, agent_id: &str) -> KernelResult<Option<String>> {
        let credential = self.credential.clone();
        let agent = agent_id.to_string();
        let value = self.call(move |core| core.pane_terminal_id(&credential, &agent))?;
        Ok(value.as_str().map(str::to_string))
    }

    pub fn record_agent_pane(&self, input: Value) -> KernelResult<()> {
        self.mutate(move |core, context| core.record_agent_pane(context, &input))
            .map(drop)
    }

    pub fn clear_agent_pane(&self, agent_id: &str) -> KernelResult<()> {
        let agent = agent_id.to_string();
        self.mutate(move |core, context| core.clear_agent_pane(context, &agent))
            .map(drop)
    }

    /// The watch pane recorded for the hub: (workspace id, pane id).
    pub fn fallback_pane(&self) -> KernelResult<Option<(Option<String>, String)>> {
        let credential = self.credential.clone();
        let value = self.call(move |core| core.fallback_pane(&credential))?;
        Ok(text(&value, "paneId").map(|pane| (text(&value, "workspaceId"), pane)))
    }

    pub fn record_fallback_pane(&self, workspace_id: &str, pane_id: &str) -> KernelResult<()> {
        let input = json!({"workspaceId": workspace_id, "paneId": pane_id});
        self.mutate(move |core, context| core.record_fallback_pane(context, &input))
            .map(drop)
    }

    pub fn clear_fallback_pane(&self) -> KernelResult<()> {
        self.mutate(|core, context| core.clear_fallback_pane(context))
            .map(drop)
    }

    pub fn record_orphan_pane(
        &self,
        agent_id: &str,
        pane_id: &str,
        terminal_id: Option<&str>,
    ) -> KernelResult<()> {
        let mut input = Map::new();
        input.insert("agentId".into(), json!(agent_id));
        input.insert("paneId".into(), json!(pane_id));
        if let Some(terminal) = terminal_id {
            input.insert("terminalId".into(), json!(terminal));
        }
        let input = Value::Object(input);
        self.mutate(move |core, context| core.record_orphan_pane(context, &input))
            .map(drop)
    }

    pub fn clear_orphan_pane(&self, pane_id: &str) -> KernelResult<()> {
        let pane = pane_id.to_string();
        self.mutate(move |core, context| core.clear_orphan_pane(context, &pane))
            .map(drop)
    }

    /// (agent id, pane id)
    pub fn orphan_panes(&self) -> KernelResult<Vec<(String, String)>> {
        let credential = self.credential.clone();
        let value = self.call(move |core| core.orphan_panes(&credential))?;
        Ok(objects(value)
            .iter()
            .filter_map(|v| Some((text(v, "agentId")?, text(v, "paneId")?)))
            .collect())
    }

    /// (agent id, pane id, terminal id)
    pub fn orphan_pane_terminals(&self) -> KernelResult<Vec<(String, String, Option<String>)>> {
        let credential = self.credential.clone();
        let value = self.call(move |core| core.orphan_pane_terminals(&credential))?;
        Ok(objects(value)
            .iter()
            .filter_map(|v| {
                Some((
                    text(v, "agentId")?,
                    text(v, "paneId")?,
                    text(v, "terminalId"),
                ))
            })
            .collect())
    }

    /// `endAgent`: the ids of the messages it cancelled. `lost` and `branch` are the options as the caller wrote them.
    pub fn end_agent(
        &self,
        agent_id: &str,
        lost: Option<&str>,
        branch: Option<Option<&str>>,
    ) -> KernelResult<Vec<String>> {
        let agent = agent_id.to_string();
        let mut options = Map::new();
        if let Some(lost) = lost {
            options.insert("lost".into(), json!(lost));
        }
        if let Some(branch) = branch {
            options.insert("branch".into(), json!(branch));
        }
        let options = Value::Object(options);
        let value =
            self.mutate(move |core, context| core.end_agent(context, &agent, Some(&options)))?;
        Ok(strings(value.get("cancelledMessageIds")))
    }

    pub fn restart_agent_generation(&self, agent_id: &str) -> KernelResult<Value> {
        let agent = agent_id.to_string();
        self.mutate(move |core, context| core.restart_agent_generation(context, &agent))
    }

    /// The restarts of an agent's PM: (sequence).
    pub fn pm_restart_sequences(&self, agent_id: &str) -> KernelResult<Vec<i64>> {
        let credential = self.credential.clone();
        let agent = agent_id.to_string();
        let value = self.call(move |core| core.pm_restarts(&credential, &agent))?;
        Ok(objects(value)
            .iter()
            .filter_map(|v| v.get("sequence").and_then(Value::as_i64))
            .collect())
    }

    pub fn mark_pm_restarts_consumed(&self, agent_id: &str, sequence: i64) -> KernelResult<()> {
        let agent = agent_id.to_string();
        self.mutate(move |core, context| core.mark_pm_restarts_consumed(context, &agent, sequence))
            .map(drop)
    }

    pub fn assert_run_not_paused(&self, action: &str) -> KernelResult<()> {
        let action = action.to_string();
        self.call(move |core| core.assert_run_not_paused(&action))
            .map(drop)
    }

    pub fn active_branch_holder(
        &self,
        branch: &str,
        except_agent_id: &str,
    ) -> KernelResult<Option<String>> {
        let branch = branch.to_string();
        let except = except_agent_id.to_string();
        let value = self.call(move |core| core.active_branch_holder(&branch, Some(&except)))?;
        Ok(match &value {
            Value::Null => None,
            Value::String(agent) => Some(agent.clone()),
            other => text(other, "agentId").or(Some(String::new())),
        })
    }

    /// `taskNaming`: (task id, type, title).
    pub fn task_naming(&self, reference: &str) -> KernelResult<(String, Option<String>, String)> {
        let credential = self.credential.clone();
        let reference = reference.to_string();
        let value = self.call(move |core| core.task_naming(&credential, &reference))?;
        Ok((
            text(&value, "taskId").unwrap_or_default(),
            text(&value, "type"),
            text(&value, "title").unwrap_or_default(),
        ))
    }

    pub fn agent_has_reports(&self, agent_id: &str) -> KernelResult<bool> {
        let agent = agent_id.to_string();
        let value = self.call(move |core| core.agent_has_reports(&agent))?;
        Ok(value.as_bool().unwrap_or(false))
    }

    pub fn rename_agent_branch(&self, agent_id: &str, from: &str, to: &str) -> KernelResult<()> {
        let input = json!({"agentId": agent_id, "from": from, "to": to});
        self.mutate(move |core, context| core.rename_agent_branch(context, &input))
            .map(drop)
    }

    pub fn is_agent_replaced(&self, agent_id: &str) -> KernelResult<bool> {
        let agent = agent_id.to_string();
        let value = self.call(move |core| core.is_agent_replaced(&agent))?;
        Ok(value.as_bool().unwrap_or(false))
    }

    pub fn running_integrations(&self) -> KernelResult<usize> {
        let credential = self.credential.clone();
        let value = self.call(move |core| core.running_integrations(&credential))?;
        Ok(objects(value).len())
    }

    pub fn agent_seed(&self, agent_id: &str) -> KernelResult<Value> {
        let agent = agent_id.to_string();
        self.call(move |core| core.agent_seed(&agent))
    }

    pub fn record_agent_replaced(
        &self,
        predecessor_id: &str,
        successor_id: &str,
        branch: &str,
        kept: Option<(&str, &str)>,
    ) -> KernelResult<()> {
        let mut input = Map::new();
        input.insert("predecessorId".into(), json!(predecessor_id));
        input.insert("successorId".into(), json!(successor_id));
        input.insert("branch".into(), json!(branch));
        if let Some((kept_ref, kept_tip)) = kept {
            input.insert("keptRef".into(), json!(kept_ref));
            input.insert("keptTip".into(), json!(kept_tip));
        }
        let input = Value::Object(input);
        self.mutate(move |core, context| core.record_agent_replaced(context, &input))
            .map(drop)
    }
}

/// The strings of a JSON array; anything else is an empty list.
pub fn strings(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

// ------------------------------------------------------------------------------------------------ the kernel

/// `PromptInput["architect"]` etc. are built here from the configuration.
pub struct LauncherKernel {
    pub ledger: Ledger,
    pub adapter: Arc<dyn LauncherAdapter>,
    pub config: RoleConfig,
    pub root: String,
    pub project: String,
    pub cli_path: String,
    pub socket_path: String,
    pub credential: String,
    pub node: String,
    pub base_environment: HashMap<String, String>,
    pub git: Arc<dyn GitRunner>,
    pub now: Clock,
    log_fn: LogFn,
    sync_roles: Option<Arc<dyn Fn() -> Result<(), String> + Send + Sync>>,
    run_setup_override: Option<SetupRunner>,
    run_teardown_override: Option<TeardownRunner>,
    sleep_fn: Arc<dyn Fn(u64) + Send + Sync>,
    pub site: FrontEndSite,
    /// Predecessors whose replacement is running now.
    pub replacing: Mutex<HashSet<String>>,
    /// How many agents this launcher has started; an operation that raised it started one.
    pub agents_started: AtomicUsize,
    pub cleanup_failed: Mutex<Vec<CleanupFailure>>,
    active: Mutex<usize>,
    serial: Mutex<()>,
}

/// The names of the `[env] pass` variables that are not set, and the warning that names them.
pub type MissingEnv = (Vec<String>, String);

/// What `closeOwned` found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Closed {
    Closed,
    Gone,
    NotOwned,
}

fn system_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn is_safe_terminal_id(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
}

impl LauncherKernel {
    pub fn new(options: LauncherOptions) -> Self {
        let site = options.site.unwrap_or_else(FrontEndSite::current);
        let project = project_display_name(
            options.config.project_name.as_deref(),
            &options.project_root,
        );
        let git: Arc<dyn GitRunner> = options
            .git
            .unwrap_or_else(|| Arc::new(DefaultGit::new(&options.project_root)));
        Self {
            ledger: options.ledger,
            adapter: options.adapter,
            config: options.config,
            node: options.node_path.unwrap_or_else(|| site.exec_path.clone()),
            root: options.project_root,
            project,
            cli_path: options.cli_path,
            socket_path: options.socket_path,
            credential: options.credential,
            base_environment: options.base_environment,
            git,
            now: options.now.unwrap_or_else(|| Arc::new(system_millis)),
            log_fn: options.log.unwrap_or_else(|| Arc::new(|_, _| {})),
            sync_roles: options.sync_roles,
            run_setup_override: options.run_setup,
            run_teardown_override: options.run_teardown,
            sleep_fn: options.sleep.unwrap_or_else(|| {
                Arc::new(|ms| std::thread::sleep(std::time::Duration::from_millis(ms)))
            }),
            site,
            replacing: Mutex::new(HashSet::new()),
            agents_started: AtomicUsize::new(0),
            cleanup_failed: Mutex::new(Vec::new()),
            active: Mutex::new(0),
            serial: Mutex::new(()),
        }
    }

    pub fn log(&self, event: &str, details: Value) {
        (self.log_fn)(event, details);
    }

    pub fn sleep(&self, milliseconds: u64) {
        (self.sleep_fn)(milliseconds);
    }

    pub fn now_ms(&self) -> i64 {
        (self.now)()
    }

    /// One operation runs, up to `MAX_WAITING_OPERATIONS` wait behind it.
    pub fn run<T>(&self, operation: impl FnOnce(&Budget) -> OpResult<T>) -> OpResult<T> {
        {
            let mut active = lock(&self.active);
            if *active > MAX_WAITING_OPERATIONS {
                return Err(OpError::launcher(
                    "busy",
                    "another launcher operation is already waiting",
                ));
            }
            *active += 1;
        }
        let result = {
            let _turn = lock(&self.serial);
            operation(&self.budget(STEP_BUDGET_MS))
        };
        *lock(&self.active) -= 1;
        result
    }

    pub fn in_flight(&self) -> usize {
        *lock(&self.active)
    }

    pub fn budget(&self, ms: u64) -> Budget {
        Budget::new(Arc::clone(&self.now), ms)
    }

    pub fn pm_role(&self) -> OpResult<&Role> {
        self.config
            .roles
            .iter()
            .find(|role| role.kind == "PM")
            .ok_or_else(|| OpError::launcher("not_configured", "the configuration has no PM role"))
    }

    pub fn role_named(&self, name: &str) -> Option<&Role> {
        self.config.roles.iter().find(|role| role.name == name)
    }

    pub fn wait_seconds(&self, role: &Role) -> i64 {
        self.config
            .hosts
            .iter()
            .find(|host| host.name == role.host)
            .map(|host| host.wait_timeout_seconds)
            .unwrap_or(DEFAULT_WAIT_TIMEOUT_SECONDS)
    }

    pub fn worker_roles(&self) -> Vec<(String, String)> {
        self.config
            .roles
            .iter()
            .filter(|role| role.kind != "PM")
            .map(|role| (role.name.clone(), role.kind.clone()))
            .collect()
    }

    /// The architect settings a prompt needs; None while the Architect is disabled, so no prompt changes.
    pub fn architect_prompt(&self) -> Option<ArchitectInput> {
        let architect = &self.config.architect;
        architect.enabled.then(|| ArchitectInput {
            role: architect.role.clone(),
            high_risk_triggers: architect.high_risk_triggers.clone(),
        })
    }

    /// The researcher settings a prompt needs; None while the Researcher is disabled, so no prompt changes.
    pub fn researcher_prompt(&self) -> Option<ResearcherInput> {
        let researcher = &self.config.researcher;
        researcher.enabled.then(|| ResearcherInput {
            role: researcher.role.clone(),
            output_dir: researcher.output_dir.clone(),
            user_agent: researcher.user_agent.clone(),
        })
    }

    /// The operator settings a prompt needs; None while the Operator is disabled, so no prompt changes.
    pub fn operator_prompt(&self) -> Option<OperatorInput> {
        let operator = &self.config.operator;
        operator.enabled.then(|| OperatorInput {
            role: operator.role.clone(),
            auto_approve: operator.auto_approve.clone(),
        })
    }

    pub fn is_operator_role(&self, name: &str, kind: &str) -> bool {
        let operator = &self.config.operator;
        operator.configured && kind == "Developer" && name == operator.role
    }

    pub fn nexora_prompt(&self) -> Option<NexoraInput> {
        let nexora = &self.config.nexora;
        (nexora.track != "never").then_some(NexoraInput {
            track: nexora.track,
            default_action: nexora.default_action,
        })
    }

    pub fn is_architect_role(&self, name: &str, kind: &str) -> bool {
        let architect = &self.config.architect;
        architect.enabled && kind == "Developer" && name == architect.role
    }

    pub fn is_researcher_role(&self, name: &str, kind: &str) -> bool {
        let researcher = &self.config.researcher;
        researcher.enabled && kind == "Developer" && name == researcher.role
    }

    pub fn active_agents(&self) -> OpResult<Vec<AgentRec>> {
        Ok(self.ledger.active_agents()?)
    }

    pub fn agent_record(&self, agent_id: &str) -> OpResult<Option<AgentRec>> {
        Ok(self.ledger.agent_record(agent_id)?)
    }

    /// The first of the role's seats that no active agent holds; an extra seat has the id `<role>-seat-<n>` and the display
    /// name `<role>.<n>`, which no role name can equal (role names allow no dot), so it never collides with another role's
    /// seat; the core allows one active agent per seat, so each concurrent worker needs its own. A disabled seat, or one
    /// made for another kind, is an operator-visible error and stops the walk even when a later seat is free.
    pub fn seat(&self, role: &Role) -> OpResult<String> {
        let held: HashSet<String> = self
            .active_agents()?
            .into_iter()
            .map(|a| a.seat_id)
            .collect();
        let seats = self.ledger.seats()?;
        let attempts = if role.kind == "PM" {
            1
        } else {
            self.config.limits.max_workers
        };
        for number in 1..=attempts {
            let seat_id = if number == 1 {
                format!("{}-seat", role.name)
            } else {
                format!("{}-seat-{number}", role.name)
            };
            let existing = seats
                .iter()
                .find(|entry| text(entry, "seatId").as_deref() == Some(seat_id.as_str()));
            let Some(existing) = existing else {
                let name = if number == 1 {
                    role.name.clone()
                } else {
                    format!("{}.{number}", role.name)
                };
                self.ledger.create_seat(&seat_id, &name, &role.kind)?;
                return Ok(seat_id);
            };
            let seat_kind = text(existing, "role").unwrap_or_default();
            if seat_kind != role.kind {
                return Err(OpError::launcher(
                    "seat_kind_mismatch",
                    format!(
                        "the seat {seat_id} was created for kind {seat_kind} but the configuration says {}",
                        role.kind
                    ),
                ));
            }
            if text(existing, "seatState").as_deref() == Some("disabled") {
                return Err(OpError::launcher(
                    "seat_disabled",
                    format!("the seat {seat_id} is disabled"),
                ));
            }
            if !held.contains(&seat_id) {
                return Ok(seat_id);
            }
        }
        Err(OpError::launcher(
            "role_active",
            format!(
                "every seat of the role {} is held by an active agent",
                role.name
            ),
        ))
    }

    fn role_is_synced(&self, role: &Role) -> OpResult<bool> {
        let definitions = self.ledger.role_definitions()?;
        Ok(definitions
            .iter()
            .find(|d| text(d, "name").as_deref() == Some(role.name.as_str()))
            .is_some_and(|d| {
                text(d, "state").as_deref() == Some("active")
                    && text(d, "kind").as_deref() == Some(role.kind.as_str())
                    && text(d, "host").as_deref() == Some(role.host.as_str())
                    && text(d, "configHash").as_deref() == Some(role.config_hash.as_str())
            }))
    }

    pub fn assert_role_synced(&self, role: &Role) -> OpResult<()> {
        if self.role_is_synced(role)? {
            return Ok(());
        }
        let mut reason = String::new();
        if let Some(sync) = &self.sync_roles {
            if let Err(message) = sync() {
                let folded = crate::text::fold_blanks(&message);
                let cut: String = folded.chars().take(MAX_SYNC_REASON_CHARS).collect();
                reason = format!(" ({cut})");
            }
        }
        if self.role_is_synced(role)? {
            return Ok(());
        }
        Err(OpError::launcher(
            "role_not_synced",
            format!(
                "the role {} is not synced into the controller{reason}; run cstan stop, then cstan start, and look for role_sync_failed in the daemon log if it persists",
                role.name
            ),
        ))
    }

    /// A new agent of `role`: its id and credential.
    pub fn create_agent(&self, role: &Role) -> OpResult<(String, String)> {
        self.assert_role_synced(role)?;
        let seat_id = self.seat(role)?;
        let used = self
            .ledger
            .list_agents()?
            .iter()
            .filter(|a| a.role_name == role.name)
            .count() as u32;
        let mut last_error: Option<OpError> = None;
        for attempt in 1..=ID_ATTEMPTS {
            let agent_id = format!("{}-{}", role.name, used + attempt);
            if self.agent_record(&agent_id)?.is_some() {
                continue;
            }
            // Only the crash window between createActor and registerAgent can leave an active actor on the seat that no
            // active agent owns.
            let owned: HashSet<String> = self
                .active_agents()?
                .into_iter()
                .map(|a| a.actor_id)
                .collect();
            for actor_id in self.ledger.seat_actor_ids(&seat_id)? {
                if !owned.contains(&actor_id) {
                    self.ledger.revoke_actor(&actor_id)?;
                }
            }
            let (actor_id, credential) =
                self.ledger.create_actor(&agent_id, &role.kind, &seat_id)?;
            match self
                .ledger
                .register_agent(&agent_id, &role.name, &seat_id, &actor_id)
            {
                Ok(()) => return Ok((agent_id, credential)),
                Err(error) => {
                    last_error = Some(error.into());
                    if let Err(revoke_error) = self.ledger.revoke_actor(&actor_id) {
                        self.log(
                            "actor_not_revoked",
                            json!({"error": OpError::from(revoke_error).js_string()}),
                        );
                    }
                }
            }
        }
        Err(last_error
            .unwrap_or_else(|| OpError::launcher("id_conflict", "no free agent id was found")))
    }

    /// Writes the per-agent `cstan` wrapper into `<root>/.capstan/bin` and returns that directory.
    pub fn ensure_wrapper(&self) -> OpResult<String> {
        let directory = Path::new(&self.root).join(".capstan").join("bin");
        let io = |error: std::io::Error| {
            OpError::launcher(
                "wrapper_failed",
                format!("the cstan wrapper could not be written: {error}"),
            )
        };
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&directory)
            .map_err(io)?;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).map_err(io)?;
        let temporary = directory.join(format!("cstan.tmp-{}", self.ledger.uuid()?));
        {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o700)
                .open(&temporary)
                .map_err(io)?;
            file.write_all(cstan_wrapper_script(&self.site).as_bytes())
                .map_err(io)?;
        }
        std::fs::rename(&temporary, directory.join("cstan")).map_err(io)?;
        Ok(directory.to_string_lossy().into_owned())
    }

    /// The pane of an active agent, or a LauncherError.
    pub fn active_pane(&self, agent_id: &str) -> OpResult<String> {
        let agent = self.agent_record(agent_id)?;
        if !agent.is_some_and(|a| a.active()) {
            return Err(OpError::launcher(
                "agent_not_active",
                "the agent is not active",
            ));
        }
        let pane = match self.adapter.pane_for_agent(agent_id) {
            Some(pane) => Some(pane),
            None => self
                .ledger
                .agent_panes()?
                .into_iter()
                .find(|row| row.agent_id == agent_id)
                .and_then(|row| row.pane_id),
        };
        pane.ok_or_else(|| OpError::launcher("no_pane", "the agent has no pane recorded"))
    }

    /// Names from `[env] pass` that are not set (or are empty) in the daemon's environment: an agent started now would not
    /// have them.
    pub fn missing_pass_environment(&self) -> Vec<String> {
        self.config
            .env_pass
            .iter()
            .filter(|name| {
                self.base_environment
                    .get(*name)
                    .is_none_or(|value| value.is_empty())
            })
            .cloned()
            .collect()
    }

    /// Runs an operation like `run`. If it started an agent while a listed variable was unset, the answer carries the names
    /// and a warning and the log gets one entry. The count is read inside the operation, so overlapping operations cannot
    /// see each other's starts.
    pub fn run_starting<T>(
        &self,
        operation: impl FnOnce(&Budget) -> OpResult<T>,
    ) -> OpResult<(T, Option<MissingEnv>)> {
        self.run(|budget| {
            let before = self.agents_started.load(Ordering::SeqCst);
            let result = operation(budget)?;
            let missing = if self.agents_started.load(Ordering::SeqCst) > before {
                self.missing_environment()
            } else {
                None
            };
            Ok((result, missing))
        })
    }

    /// The missing variables and the warning that names them; None when nothing is missing. One log entry per call.
    pub fn missing_environment(&self) -> Option<MissingEnv> {
        let missing = self.missing_pass_environment();
        if missing.is_empty() {
            return None;
        }
        self.log("env_pass_missing", json!({"names": missing}));
        let one = missing.len() == 1;
        let warning = format!(
            "{} {} listed in [env] pass but not set where the daemon was started, so this agent does not have {}; set {} in the shell that runs cstan start (or its profile file), then restart the daemon",
            missing.join(", "),
            if one { "is" } else { "are" },
            if one { "it" } else { "them" },
            if one { "it" } else { "them" },
        );
        Some((missing, warning))
    }

    /// The one environment every agent starts with: the allowlist, its token and socket, and `cstan` first on PATH.
    /// `include_pass` defaults to whether there is a token.
    pub fn environment(
        &self,
        token: Option<&str>,
        include_pass: Option<bool>,
    ) -> OpResult<BTreeMap<String, String>> {
        if self.root.contains(':') {
            return Err(OpError::launcher(
                "unsupported_root",
                "a project path with a colon cannot be put on PATH, so cstan would not be found",
            ));
        }
        let bin = self.ensure_wrapper()?;
        let path = match self.base_environment.get("PATH") {
            None => bin,
            Some(base) if base.is_empty() => bin,
            Some(base) => format!("{bin}:{base}"),
        };
        let mut extras = vec![("PATH".to_string(), path)];
        if let Some(token) = token {
            extras.push(("CAPSTAN_TOKEN".into(), token.to_string()));
            extras.push(("CAPSTAN_SOCKET".into(), self.socket_path.clone()));
        }
        let include_pass = include_pass.unwrap_or(token.is_some());
        let pass: &[String] = if include_pass {
            &self.config.env_pass
        } else {
            &[]
        };
        Ok(build_agent_environment(
            &self.base_environment,
            &extras,
            pass,
        )?)
    }

    pub fn host_kind(&self, role: &Role) -> OpResult<String> {
        self.config
            .hosts
            .iter()
            .find(|host| host.name == role.host)
            .map(|host| host.kind.clone())
            .ok_or_else(|| {
                OpError::launcher(
                    "unknown_host",
                    format!("the configuration has no host {}", role.host),
                )
            })
    }

    /// The command-line arguments of the agent's host.
    pub fn arguments(
        &self,
        role: &Role,
        prompt_text: &str,
        prompt_file: &str,
        worktree_path: Option<&str>,
    ) -> OpResult<Vec<String>> {
        let kind = self.host_kind(role)?;
        let host = HostRoleSettings {
            model: role.model.clone(),
        };
        Ok(match kind.as_str() {
            "codex" => codex_arguments(&host, prompt_text, worktree_path.map(Path::new))?,
            "omp" => omp_arguments(&host, prompt_file)?,
            _ => {
                let mut allow = role.allow.clone();
                if !allow.iter().any(|rule| rule == CSTAN_ALLOW_RULE) {
                    allow.push(CSTAN_ALLOW_RULE.to_string());
                }
                claude_arguments(
                    &ClaudeRoleSettings {
                        model: role.model.clone(),
                        permission_mode: role.permission_mode.clone(),
                        allow,
                        deny: role.deny.clone(),
                        hooks: role.hooks.to_string(),
                        mcp: role
                            .mcp
                            .iter()
                            .map(|server| HerdrMcpServer {
                                name: server.name.clone(),
                                command: server.command.clone(),
                                args: server.args.clone(),
                            })
                            .collect(),
                    },
                    Some(prompt_file),
                )?
            }
        })
    }

    /// Tells Herdr which project, role and agent a pane belongs to, for the operator's sidebar; display only, so a failure
    /// is logged and never stops a start.
    pub fn describe(
        &self,
        pane_id: &str,
        workspace_id: Option<&str>,
        agent_id: &str,
        role_name: &str,
        label: Option<&str>,
    ) {
        let attempt = || -> Result<(), AdapterError> {
            if let (Some(label), Some(workspace)) = (label, workspace_id) {
                self.adapter.rename_workspace(workspace, label)?;
            }
            let pane_tokens = BTreeMap::from([
                ("project".to_string(), self.project.clone()),
                ("role".to_string(), role_name.to_string()),
                ("agent".to_string(), agent_id.to_string()),
            ]);
            self.adapter
                .report_metadata(MetadataTarget::Pane(pane_id), &pane_tokens)?;
            if let Some(workspace) = workspace_id {
                let tokens = BTreeMap::from([("project".to_string(), self.project.clone())]);
                self.adapter
                    .report_metadata(MetadataTarget::Workspace(workspace), &tokens)?;
            }
            Ok(())
        };
        if let Err(error) = attempt() {
            self.log(
                "describe_failed",
                json!({"agentId": agent_id, "error": OpError::from(error).js_string()}),
            );
        }
    }

    /// Closes a pane; a pane Herdr no longer knows counts as closed. Returns false when the pane was already gone.
    pub fn close(&self, pane_id: &str) -> OpResult<bool> {
        match self.adapter.close_pane(pane_id) {
            Ok(()) => Ok(true),
            Err(AdapterError::Herdr(error))
                if error.code.contains("not_found") || error.code.contains("no_such") =>
            {
                self.adapter.forget_pane(pane_id);
                Ok(false)
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Closes the pane recorded for `owner` only while Herdr still shows it as that agent's: the terminal id recorded at
    /// spawn matches, and the pane's `agent` and `project` tokens, when set, name this agent and project. A row recorded
    /// without a terminal id needs both tokens to match. Herdr reuses short pane ids, so a pane that fails the check
    /// belongs to someone else (often a newer agent) and is left alone. "gone" and "not owned" both mean the agent's pane
    /// is no longer open.
    pub fn close_owned(
        &self,
        pane_id: &str,
        agent_id: &str,
        terminal_id: Option<&str>,
    ) -> OpResult<Closed> {
        let Some(identity) = self.adapter.pane_identity(pane_id)? else {
            self.adapter.forget_pane(pane_id);
            return Ok(Closed::Gone);
        };
        let tokens_agree = identity.agent.as_deref().is_none_or(|a| a == agent_id)
            && identity
                .project
                .as_deref()
                .is_none_or(|p| p == self.project);
        let owned = match terminal_id {
            Some(recorded) => identity.terminal_id.as_deref() == Some(recorded) && tokens_agree,
            None => {
                identity.agent.as_deref() == Some(agent_id)
                    && identity.project.as_deref() == Some(self.project.as_str())
            }
        };
        if !owned {
            self.log(
                "pane_not_owned",
                json!({
                    "agentId": agent_id,
                    "paneId": pane_id,
                    "terminalId": identity.terminal_id,
                    "recordedTerminalId": terminal_id,
                    "paneAgent": identity.agent,
                    "paneProject": identity.project,
                }),
            );
            self.forget_stale(pane_id, agent_id);
            return Ok(Closed::NotOwned);
        }
        Ok(if self.close(pane_id)? {
            Closed::Closed
        } else {
            Closed::Gone
        })
    }

    /// Drops a registry entry for `pane_id` only when it is this agent's; an entry another agent holds under a reused id
    /// stays.
    fn forget_stale(&self, pane_id: &str, agent_id: &str) {
        if let Some(entry) = self.adapter.pane_entry(pane_id) {
            if entry.agent.as_deref() == Some(agent_id) {
                self.adapter.forget_pane(pane_id);
            }
        }
    }

    /// Herdr's terminal id of a pane this operation just made; None when Herdr does not report one, so the pane is
    /// recorded without it.
    pub fn terminal_of(&self, pane_id: &str, agent_id: &str) -> Option<String> {
        match self.adapter.pane_identity(pane_id) {
            Ok(identity) => {
                if let Some(terminal) = identity.and_then(|i| i.terminal_id) {
                    if is_safe_terminal_id(&terminal) {
                        return Some(terminal);
                    }
                }
                self.log(
                    "terminal_id_unknown",
                    json!({"agentId": agent_id, "paneId": pane_id}),
                );
            }
            Err(error) => self.log(
                "terminal_id_unknown",
                json!({"agentId": agent_id, "paneId": pane_id, "error": OpError::from(error).js_string()}),
            ),
        }
        None
    }

    pub fn within(&self, budget: &Budget) -> bool {
        budget.within()
    }

    /// Runs the worktree setup command: the injected runner, or `sh -c` with the agent environment.
    pub fn run_setup(
        &self,
        command: &str,
        cwd: &str,
        timeout_ms: u64,
    ) -> OpResult<crate::api::SetupOutcome> {
        if let Some(runner) = &self.run_setup_override {
            return Ok(runner(command, cwd, timeout_ms));
        }
        let environment = self.environment(None, Some(true))?;
        Ok(run_setup_command(command, cwd, timeout_ms, &environment))
    }

    /// Runs the worktree teardown command with the environment it is given.
    pub fn run_teardown(
        &self,
        command: &str,
        cwd: &str,
        timeout_ms: u64,
        environment: &BTreeMap<String, String>,
    ) -> crate::api::SetupOutcome {
        match &self.run_teardown_override {
            Some(runner) => runner(command, cwd, timeout_ms, environment),
            None => run_setup_command(command, cwd, timeout_ms, environment),
        }
    }

    pub fn cleanup_failures(&self) -> Vec<CleanupFailure> {
        lock(&self.cleanup_failed).clone()
    }

    pub fn forget_cleanup_failure(&self, agent_id: &str) {
        lock(&self.cleanup_failed).retain(|entry| entry.agent_id != agent_id);
    }

    pub fn note_cleanup_failure(&self, agent_id: &str, reason: &str) {
        let mut failed = lock(&self.cleanup_failed);
        failed.retain(|entry| entry.agent_id != agent_id);
        failed.push(CleanupFailure {
            agent_id: agent_id.to_string(),
            reason: reason.to_string(),
        });
    }
}
