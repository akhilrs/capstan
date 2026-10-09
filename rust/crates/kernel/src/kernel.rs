//! The state every area of the controller shares: the database, the project identity, the clock, the lock and the one
//! transaction wrapper (`mutate`) through which every state change is written and ledgered. src/controller/kernel.ts,
//! and the open/close of src/controller/core.ts.

use crate::auth::{authenticate_actor, require_capability, AuthenticatedActor};
use crate::bootstrap::initialize_project;
use crate::canonical::{canonical_json, digest_json};
use crate::env::Env;
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{
    decrypt_actor_result, encrypt_actor_result, execute, exists, is_safe_id, query_all, query_opt,
    returns_credential,
};
use crate::records::{AgentRow, MutationEvent, MutationOutput};
use crate::types::{AgentRecord, InitialProject, MutationContext};
use capstan_ledger::{
    iso_from_millis, open_database, open_database_read_only, resolve_database_path, Database,
    LedgerError, OpenOptions as LedgerOpenOptions, ProjectLock,
};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::fs;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

/// What `ControllerCore.open` takes besides the state directory and the project.
#[derive(Default)]
pub struct KernelOptions {
    pub workspace_root: Option<PathBuf>,
    pub runtime_workspace_path: Option<PathBuf>,
    /// Pre-migration backups to keep after a migration; default 3.
    pub keep_migration_backups: Option<usize>,
}

pub struct Kernel {
    pub database: Database,
    lock: RefCell<Option<ProjectLock>>,
    pub project_id: String,
    pub internal_actor_id: String,
    pub read_only: bool,
    pub env: Box<dyn Env>,
    closed: Cell<bool>,
}

/// `path.resolve` for an absolute path: `.` and `..` folded, no trailing separator.
fn resolve_absolute(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn private_directory_error() -> KernelError {
    LedgerError::Ownership(
        "controller state directory must be a private directory owned by the current user".into(),
    )
    .into()
}

fn check_project_id(project: &InitialProject) -> KernelResult<()> {
    let id = project.project_id.as_bytes();
    let ok = (1..=64).contains(&id.len())
        && id[0].is_ascii_alphanumeric()
        && id[1..]
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(KernelError::type_error(
            "project id must be 1-64 safe ASCII characters",
        ))
    }
}

fn check_state_directory(state_dir: &Path, create: bool) -> KernelResult<PathBuf> {
    if !state_dir.is_absolute() {
        return Err(KernelError::type_error("state directory must be absolute"));
    }
    let resolved = resolve_absolute(state_dir);
    if create {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&resolved)?;
    }
    let stat = fs::symlink_metadata(&resolved)?;
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    if !stat.is_dir()
        || stat.file_type().is_symlink()
        || stat.uid() != uid
        || stat.mode() & 0o077 != 0
    {
        return Err(private_directory_error());
    }
    Ok(resolved)
}

fn check_workspace(options: &KernelOptions) -> KernelResult<()> {
    let root = match &options.workspace_root {
        Some(root) => resolve_absolute(&if root.is_absolute() {
            root.clone()
        } else {
            std::env::current_dir()?.join(root)
        }),
        None => std::env::current_dir()?,
    };
    let stat = fs::symlink_metadata(root)?;
    if !stat.is_dir() || stat.file_type().is_symlink() {
        return Err(KernelError::controller(
            "workspace root must be an existing non-symlink directory",
        ));
    }
    if let Some(runtime) = &options.runtime_workspace_path {
        if !runtime.is_absolute() {
            return Err(KernelError::type_error(
                "runtime workspace path must be absolute",
            ));
        }
    }
    Ok(())
}

fn open_existing_project(database: &Database, project: &InitialProject) -> KernelResult<()> {
    let name = query_opt(
        database,
        "SELECT name FROM projects WHERE project_id = ?",
        [&project.project_id],
        |row| row.get::<_, String>(0),
    )?;
    let Some(name) = name else {
        return Err(KernelError::controller(
            "durable project state does not exist",
        ));
    };
    if name != project.name {
        return Err(KernelError::controller(
            "project name does not match durable project state",
        ));
    }
    let actor = authenticate_actor(database, &project.project_id, &project.owner_credential)?;
    if actor.role != "operator" {
        return Err(KernelError::controller(
            "opening an existing project requires its operator credential",
        ));
    }
    Ok(())
}

fn internal_actor_id(database: &Database, project_id: &str) -> KernelResult<String> {
    query_opt(
        database,
        "SELECT actor_id FROM actors WHERE project_id = ? AND is_internal = 1 AND role = 'controller' AND active = 1 AND revoked_at IS NULL",
        [project_id],
        |row| row.get::<_, String>(0),
    )?
    .ok_or_else(|| KernelError::controller("durable internal controller principal is missing"))
}

impl Kernel {
    /// `ControllerCore.open`: private state directory checks, the project lock, migrations, the first-time bootstrap of
    /// the project (or its owner check), the internal actor, and the reconciliation of assignments left uncertain by a
    /// restart.
    pub fn open(
        state_dir: &Path,
        project: &InitialProject,
        options: &KernelOptions,
        env: Box<dyn Env>,
    ) -> KernelResult<Kernel> {
        check_project_id(project)?;
        let state_directory = check_state_directory(state_dir, true)?;
        let mut lock = ProjectLock::acquire(&state_directory.join("controller.lock"))?;
        let attempt = (|| -> KernelResult<Kernel> {
            let database_path = resolve_database_path(&state_directory)?;
            match fs::symlink_metadata(&database_path) {
                Ok(existing) => {
                    if !existing.is_file() || existing.file_type().is_symlink() {
                        return Err(LedgerError::Ownership(
                            "controller database path must be a regular file".into(),
                        )
                        .into());
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            let mut ledger_options = LedgerOpenOptions::default();
            if let Some(keep) = options.keep_migration_backups {
                ledger_options.keep_migration_backups = keep;
            }
            let database = open_database(&database_path, &ledger_options)?;
            fs::set_permissions(&database_path, fs::Permissions::from_mode(0o600))?;
            let known = query_opt(
                &database,
                "SELECT name FROM projects WHERE project_id = ?",
                [&project.project_id],
                |row| row.get::<_, String>(0),
            )?
            .is_some();
            if known {
                open_existing_project(&database, project)?;
            } else {
                initialize_project(&database, project, env.as_ref())?;
            }
            let internal = internal_actor_id(&database, &project.project_id)?;
            check_workspace(options)?;
            Ok(Kernel {
                database,
                lock: RefCell::new(None),
                project_id: project.project_id.clone(),
                internal_actor_id: internal,
                read_only: false,
                env,
                closed: Cell::new(false),
            })
        })();
        match attempt {
            Ok(kernel) => {
                *kernel.lock.borrow_mut() = Some(lock);
                if let Err(error) =
                    crate::areas::reconcile::reconcile_uncertain_assignments(&kernel)
                {
                    kernel.close();
                    return Err(error);
                }
                Ok(kernel)
            }
            Err(error) => {
                lock.close();
                Err(error)
            }
        }
    }

    /// `ControllerCore.openReadOnly`: an existing ledger, no lock, no bootstrap, every mutation refused.
    pub fn open_read_only(
        state_dir: &Path,
        project: &InitialProject,
        options: &KernelOptions,
        env: Box<dyn Env>,
    ) -> KernelResult<Kernel> {
        check_project_id(project)?;
        let state_directory = check_state_directory(state_dir, false)?;
        let database_path = resolve_database_path(&state_directory)?;
        let stat = fs::symlink_metadata(&database_path)?;
        if !stat.is_file() || stat.file_type().is_symlink() {
            return Err(LedgerError::Ownership(
                "controller database path must be a regular file".into(),
            )
            .into());
        }
        let database = open_database_read_only(&database_path)?;
        open_existing_project(&database, project)?;
        let internal = internal_actor_id(&database, &project.project_id)?;
        check_workspace(options)?;
        Ok(Kernel {
            database,
            lock: RefCell::new(None),
            project_id: project.project_id.clone(),
            internal_actor_id: internal,
            read_only: true,
            env,
            closed: Cell::new(false),
        })
    }

    /// `ControllerCore.close`: the lock is released at once; the connection closes when the kernel is dropped.
    pub fn close(&self) {
        if self.closed.replace(true) {
            return;
        }
        if let Some(mut lock) = self.lock.borrow_mut().take() {
            lock.close();
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed.get()
    }

    /// The current time as `new Date().toISOString()`; every call reads the clock once.
    pub fn now(&self) -> String {
        iso_from_millis(self.env.now())
    }

    pub fn assert_open(&self) -> KernelResult<()> {
        if self.closed.get() {
            return Err(KernelError::controller("controller is closed"));
        }
        if let Some(lock) = self.lock.borrow().as_ref() {
            lock.assert_held()?;
        }
        Ok(())
    }

    pub fn assert_writable(&self) -> KernelResult<()> {
        if self.read_only {
            Err(KernelError::controller("controller is open read-only"))
        } else {
            Ok(())
        }
    }

    pub fn agent_row(&self, agent_id: &str) -> KernelResult<Option<AgentRow>> {
        query_opt(
            &self.database,
            "SELECT * FROM agents WHERE project_id = ? AND agent_id = ?",
            [&self.project_id, agent_id],
            AgentRow::from_row,
        )
    }

    pub fn agent_by_actor(&self, actor_id: &str) -> KernelResult<Option<AgentRow>> {
        query_opt(
            &self.database,
            "SELECT * FROM agents WHERE project_id = ? AND actor_id = ? AND state = 'active'",
            [&self.project_id, actor_id],
            AgentRow::from_row,
        )
    }

    /// The one transaction wrapper: authenticates, authorizes, replays or refuses a repeated request, checks the project
    /// version and input revision, runs `apply`, bumps the version and writes the events and the stored result.
    pub fn mutate<F>(
        &self,
        context: &MutationContext,
        action: &str,
        capability: &str,
        payload: &Value,
        apply: F,
    ) -> KernelResult<Value>
    where
        F: FnOnce(&AuthenticatedActor) -> KernelResult<MutationOutput>,
    {
        self.assert_open()?;
        self.assert_writable()?;
        if !is_safe_id(&context.request_id) || !is_safe_id(&context.idempotency_key) {
            return Err(KernelError::type_error(
                "request and idempotency identifiers must be 1-128 safe ASCII characters",
            ));
        }
        if context.expected_version < 1 || context.input_revision < 1 {
            return Err(KernelError::type_error(
                "expected state version and input revision must be positive integers",
            ));
        }
        self.database.exec("BEGIN IMMEDIATE")?;
        let outcome = self.mutate_in_transaction(context, action, capability, payload, apply);
        match outcome {
            Ok(value) => Ok(value),
            Err(error) => {
                let _ = self.database.exec("ROLLBACK");
                Err(error)
            }
        }
    }

    fn mutate_in_transaction<F>(
        &self,
        context: &MutationContext,
        action: &str,
        capability: &str,
        payload: &Value,
        apply: F,
    ) -> KernelResult<Value>
    where
        F: FnOnce(&AuthenticatedActor) -> KernelResult<MutationOutput>,
    {
        let actor = authenticate_actor(&self.database, &self.project_id, &context.credential)?;
        require_capability(&actor, capability)?;
        let request_hash = digest_json(&json!({
            "action": action,
            "actorId": actor.actor_id,
            "projectId": self.project_id,
            "requestId": context.request_id,
            "expectedVersion": context.expected_version,
            "inputRevision": context.input_revision,
            "payload": payload,
        }));
        let existing = query_opt(
            &self.database,
            "SELECT actor_id, request_hash, result_json FROM mutation_requests WHERE project_id = ? AND idempotency_key = ?",
            [&self.project_id, &context.idempotency_key],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )?;
        if let Some((actor_id, stored_hash, result_json)) = existing {
            if actor_id != actor.actor_id || stored_hash != request_hash {
                return Err(KernelError::IdempotencyConflict(
                    "idempotency key was reused with different request content or actor".into(),
                ));
            }
            let value = if returns_credential(action) {
                decrypt_actor_result(
                    &result_json,
                    &context.credential,
                    &self.project_id,
                    &request_hash,
                )?
            } else {
                serde_json::from_str(&result_json)?
            };
            self.database.exec("COMMIT")?;
            return Ok(value);
        }
        if exists(
            &self.database,
            "SELECT idempotency_key FROM mutation_requests WHERE project_id = ? AND request_id = ?",
            [&self.project_id, &context.request_id],
        )? {
            return Err(KernelError::IdempotencyConflict(
                "request id was already used by another mutation".into(),
            ));
        }
        let (state_version, input_revision) = query_opt(
            &self.database,
            "SELECT state_version, current_input_revision FROM projects WHERE project_id = ?",
            [&self.project_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )?
        .ok_or_else(|| KernelError::controller("durable project state does not exist"))?;
        if state_version != context.expected_version {
            return Err(KernelError::StateVersionConflict(format!(
                "expected project version {}, found {}",
                context.expected_version, state_version
            )));
        }
        if input_revision != context.input_revision {
            return Err(KernelError::InputRevisionConflict(format!(
                "expected input revision {}, found {}",
                context.input_revision, input_revision
            )));
        }
        let output = apply(&actor)?;
        let next_version = state_version + 1;
        execute(
            &self.database,
            "UPDATE projects SET state_version = ? WHERE project_id = ? AND state_version = ?",
            rusqlite::params![next_version, self.project_id, state_version],
        )?;
        let now = self.now();
        let first_sequence = self.next_event_sequence()?;
        let events = std::iter::once(&output.event).chain(output.extra_events.iter());
        for (sequence, event) in (first_sequence..).zip(events) {
            self.insert_event(
                sequence,
                event,
                next_version,
                &actor.actor_id,
                &context.request_id,
                context.input_revision,
                action,
                payload,
                &now,
            )?;
        }
        let result_json = if returns_credential(action) {
            encrypt_actor_result(
                &output.value,
                &context.credential,
                &self.project_id,
                &request_hash,
                &self.env.random_bytes(12),
            )?
        } else {
            canonical_json(&output.value)
        };
        execute(
            &self.database,
            "INSERT INTO mutation_requests(project_id, idempotency_key, request_id, actor_id, request_hash, result_json, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                self.project_id,
                context.idempotency_key,
                context.request_id,
                actor.actor_id,
                request_hash,
                result_json,
                now
            ],
        )?;
        self.database.exec("COMMIT")?;
        Ok(output.value)
    }

    fn next_event_sequence(&self) -> KernelResult<i64> {
        Ok(query_opt(
            &self.database,
            "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM controller_events WHERE project_id = ?",
            [&self.project_id],
            |row| row.get::<_, i64>(0),
        )?
        .unwrap_or(1))
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_event(
        &self,
        sequence: i64,
        event: &MutationEvent,
        state_version: i64,
        actor_id: &str,
        request_id: &str,
        input_revision: i64,
        action: &str,
        payload: &Value,
        now: &str,
    ) -> KernelResult<()> {
        let event_id = self.env.uuid();
        let payload_json = canonical_json(&json!({
            "action": action,
            "payload": payload,
            "entityVersion": event.state_version,
            "details": event.details.clone().unwrap_or(Value::Null),
        }));
        execute(
            &self.database,
            "INSERT INTO controller_events(project_id, sequence, event_id, entity_type, entity_id, from_state, to_state,
               state_version, actor_id, request_id, input_revision, payload_json, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                self.project_id,
                sequence,
                event_id,
                event.entity_type,
                event.entity_id,
                event.from_state,
                event.to_state,
                state_version,
                actor_id,
                request_id,
                input_revision,
                payload_json,
                now
            ],
        )?;
        Ok(())
    }

    pub fn state_version(&self) -> KernelResult<i64> {
        self.assert_open()?;
        Ok(query_opt(
            &self.database,
            "SELECT state_version FROM projects WHERE project_id = ?",
            [&self.project_id],
            |row| row.get::<_, i64>(0),
        )?
        .unwrap_or(0))
    }

    pub fn input_revision(&self) -> KernelResult<i64> {
        self.assert_open()?;
        Ok(query_opt(
            &self.database,
            "SELECT current_input_revision FROM projects WHERE project_id = ?",
            [&self.project_id],
            |row| row.get::<_, i64>(0),
        )?
        .unwrap_or(0))
    }

    pub fn authorize(
        &self,
        credential: &str,
        capability: &str,
    ) -> KernelResult<AuthenticatedActor> {
        self.assert_open()?;
        self.assert_writable()?;
        let actor = authenticate_actor(&self.database, &self.project_id, credential)?;
        require_capability(&actor, capability)?;
        Ok(actor)
    }

    pub fn has_stored_request(&self, context: &MutationContext) -> KernelResult<bool> {
        exists(
            &self.database,
            "SELECT 1 FROM mutation_requests WHERE project_id = ? AND idempotency_key = ?",
            [&self.project_id, &context.idempotency_key],
        )
    }

    pub fn internal_principal(&self) -> KernelResult<AuthenticatedActor> {
        let rows = query_all(
            &self.database,
            "SELECT a.actor_id, a.project_id, a.role, cg.capability
             FROM actors a JOIN capability_grants cg
               ON cg.project_id = a.project_id AND cg.actor_id = a.actor_id AND cg.revoked_at IS NULL
             WHERE a.project_id = ? AND a.actor_id = ? AND a.active = 1 AND a.revoked_at IS NULL
             ORDER BY cg.capability",
            [&self.project_id, &self.internal_actor_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )?;
        match rows.first() {
            Some(first) if first.2 == "controller" => Ok(AuthenticatedActor {
                actor_id: first.0.clone(),
                project_id: first.1.clone(),
                role: "controller".into(),
                seat_id: None,
                capabilities: rows.iter().map(|r| r.3.clone()).collect(),
            }),
            _ => Err(KernelError::controller(
                "internal controller principal is not active",
            )),
        }
    }

    /// A mutation an operator asks the controller to make: `apply` gets the internal controller principal and the caller.
    pub fn mutate_as_controller<F>(
        &self,
        context: &MutationContext,
        action: &str,
        capability: &str,
        payload: &Value,
        apply: F,
    ) -> KernelResult<Value>
    where
        F: FnOnce(&AuthenticatedActor, &AuthenticatedActor) -> KernelResult<MutationOutput>,
    {
        self.mutate(context, action, capability, payload, |caller| {
            if caller.role != "operator" {
                return Err(KernelError::TransitionAuthorization(
                    "controller operations require an authenticated operator request".into(),
                ));
            }
            apply(&self.internal_principal()?, caller)
        })
    }

    /// An event row written inside a mutation that already returns its own event.
    #[allow(clippy::too_many_arguments)]
    pub fn append_event(
        &self,
        actor: &AuthenticatedActor,
        context: &MutationContext,
        entity_type: &str,
        entity_id: &str,
        from: &str,
        to: &str,
        action: &str,
        state_version: i64,
        details: &Value,
    ) -> KernelResult<()> {
        let project_version = query_opt(
            &self.database,
            "SELECT state_version FROM projects WHERE project_id = ?",
            [&self.project_id],
            |row| row.get::<_, i64>(0),
        )?
        .unwrap_or(0);
        let sequence = self.next_event_sequence()?;
        let event_id = self.env.uuid();
        let payload_json = canonical_json(&json!({
            "action": action,
            "payload": {"batch": true},
            "entityVersion": state_version,
            "details": details,
        }));
        let now = self.now();
        execute(
            &self.database,
            "INSERT INTO controller_events(project_id, sequence, event_id, entity_type, entity_id, from_state, to_state,
               state_version, actor_id, request_id, input_revision, payload_json, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                self.project_id,
                sequence,
                event_id,
                entity_type,
                entity_id,
                from,
                to,
                project_version + 1,
                actor.actor_id,
                context.request_id,
                context.input_revision,
                payload_json,
                now
            ],
        )?;
        Ok(())
    }

    pub fn controller_actor_id(&self) -> KernelResult<String> {
        query_opt(
            &self.database,
            "SELECT actor_id FROM actors WHERE project_id = ? AND is_internal = 1 AND role = 'controller' AND active = 1 AND revoked_at IS NULL",
            [&self.project_id],
            |row| row.get::<_, String>(0),
        )?
        .ok_or_else(|| KernelError::controller("the controller actor is missing"))
    }

    pub fn is_transition_allowed(
        &self,
        entity_type: &str,
        from_state: &str,
        to_state: &str,
        actor: &AuthenticatedActor,
    ) -> KernelResult<bool> {
        let rules = query_all(
            &self.database,
            "SELECT capability FROM transition_rules
             WHERE entity_type = ? AND from_state = ? AND to_state = ? AND role = ?",
            [entity_type, from_state, to_state, actor.role.as_str()],
            |row| row.get::<_, String>(0),
        )?;
        Ok(rules.iter().any(|rule| actor.capabilities.contains(rule)))
    }

    pub fn agent_record(&self, agent_id: &str) -> KernelResult<Option<AgentRecord>> {
        Ok(self.agent_row(agent_id)?.map(|row| row.record()))
    }

    /// Every agent of the project in one query, ordered by agent id; with `active_only`, only those whose state is active.
    pub fn agent_records(&self, active_only: bool) -> KernelResult<Vec<AgentRecord>> {
        let sql = format!(
            "SELECT agent_id, role_name, kind, seat_id, actor_id, generation, state, last_activity_at
             FROM agents WHERE project_id = ?{} ORDER BY agent_id",
            if active_only { " AND state = 'active'" } else { "" }
        );
        Ok(
            query_all(&self.database, &sql, [&self.project_id], AgentRow::from_row)?
                .iter()
                .map(AgentRow::record)
                .collect(),
        )
    }

    pub fn agent_by_actor_record(&self, actor_id: &str) -> KernelResult<Option<AgentRecord>> {
        match self.agent_by_actor(actor_id)? {
            Some(row) => self.agent_record(&row.agent_id),
            None => Ok(None),
        }
    }

    pub fn touch_agent(&self, agent_id: &str, now: &str) -> KernelResult<()> {
        execute(
            &self.database,
            "UPDATE agents SET last_activity_at = ? WHERE project_id = ? AND agent_id = ?",
            [now, &self.project_id, agent_id],
        )?;
        Ok(())
    }
}
