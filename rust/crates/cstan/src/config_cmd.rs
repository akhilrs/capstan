//! `cstan config check` and `cstan config sync`.
use capstan_config::{load_role_config, ConfigError, RoleConfig, CONFIG_FILE_NAME};
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::types::{InitialProject, MutationContext};
use capstan_kernel::{Core, KernelError, SystemEnv};
use capstan_ledger::LedgerError;
use capstan_wire::js::Value;
use unicode_normalization::UnicodeNormalization;

use crate::operator::{number, object};
use crate::{blocked, invalid, output, runtime, to_js, Cli, Fail, Flow};

impl Cli<'_> {
    /// `loadRoleConfig`: a refusal of the configuration is invalid input (exit 3); a file that cannot be read is the
    /// operating system's error (exit 5).
    pub fn load_role_config(&self) -> Result<RoleConfig, Fail> {
        load_role_config(&self.ctx.cwd).map_err(|error| match error {
            ConfigError::Invalid(message) => invalid(message),
            ConfigError::Parse(parse) => invalid(format!(
                "{CONFIG_FILE_NAME} is not valid TOML ({})",
                parse.detail
            )),
            ConfigError::Unreadable(message) => runtime(message),
        })
    }

    pub fn config(&mut self, rest: &[String]) -> Result<Flow, Fail> {
        let (subcommand, extra) = match rest.split_first() {
            Some((subcommand, extra)) => (subcommand.as_str(), extra),
            None => return Err(Fail::Usage),
        };
        if (subcommand != "check" && subcommand != "sync") || !extra.is_empty() {
            return Err(Fail::Usage);
        }
        let role_config = self.load_role_config()?;
        if subcommand == "check" {
            self.io.out_line(&role_config.to_json());
            for warning in &role_config.warnings {
                self.io.err_line(&format!("warning: {warning}"));
            }
            for role in &role_config.roles {
                let host = role_config.hosts.iter().find(|host| host.name == role.host);
                if let Some(host) = host.filter(|host| host.kind != "claude") {
                    self.io.err_line(&format!(
                        "warning: role {} runs on {} with full access and no approval prompts; nothing blocks it from editing outside its worktree or from pushing",
                        role.name, host.kind
                    ));
                }
            }
            return Ok(Flow::Code(0));
        }
        let (config, credential) = self.load_config()?;
        if let Some(name) = &role_config.project_name {
            let nfc = |text: &str| text.nfc().collect::<String>();
            if nfc(name) != nfc(&config.name) {
                return Err(invalid(format!(
                    "{CONFIG_FILE_NAME} project.name does not match the initialized project"
                )));
            }
        }
        if !config.state_directory.join("controller.sqlite").exists() {
            return Err(blocked(
                "controller record does not exist; create it before syncing roles",
            ));
        }
        let project = InitialProject {
            project_id: config.project_id.clone(),
            name: config.name.clone(),
            owner_credential: credential.clone(),
            initial_inputs: Vec::new(),
        };
        let options = KernelOptions {
            workspace_root: Some(self.ctx.cwd.clone()),
            ..KernelOptions::default()
        };
        let core = Core::open(
            &config.state_directory,
            &project,
            &options,
            Box::new(SystemEnv),
        )
        .map_err(|error| match error {
            KernelError::Ledger(LedgerError::ProjectLockHeld) => blocked(format!(
                "{}; stop the daemon with cstan stop, then run cstan config sync again",
                error.message()
            )),
            other => kernel_failure(&other),
        })?;
        let result = sync_configured_roles(&core, &role_config, &credential);
        core.close();
        let result = result?;
        let mut merged = object(vec![("schemaVersion", number(1.0))]);
        if let (Value::Object(into), Value::Object(from)) = (&mut merged, to_js(&result)) {
            into.extend(from);
        }
        output(&mut self.io.out, Some(&merged), true);
        Ok(Flow::Code(0))
    }
}

/// How the catch-all words a kernel failure: a value the controller refuses is a `TypeError` (exit 3), a lock or ownership
/// failure that nothing wrapped is a plain error (exit 5).
pub fn kernel_failure(error: &KernelError) -> Fail {
    match error {
        KernelError::Type(_) | KernelError::Ledger(LedgerError::InvalidArgument(_)) => {
            invalid(error.message())
        }
        _ => runtime(error.message()),
    }
}

/// `syncConfiguredRoles`: the configured roles are the desired role definitions; a state version conflict is retried once,
/// a second one asks for the command to be run again.
fn sync_configured_roles(
    core: &Core,
    config: &RoleConfig,
    credential: &str,
) -> Result<serde_json::Value, Fail> {
    let desired = serde_json::Value::Array(
        config
            .roles
            .iter()
            .map(|role| {
                serde_json::json!({
                    "name": role.name,
                    "kind": role.kind,
                    "host": role.host,
                    "configHash": role.config_hash,
                })
            })
            .collect(),
    );
    for attempt in 0.. {
        // The context of the Node CLI (`context` of src/cli.ts): one bare uuid is both the request id and the idempotency
        // key, and the ledger rows record them.
        let id = core.kernel().env.uuid();
        let context = MutationContext {
            credential: credential.to_string(),
            request_id: id.clone(),
            idempotency_key: id,
            expected_version: core.state_version().map_err(|e| kernel_failure(&e))?,
            input_revision: core.input_revision().map_err(|e| kernel_failure(&e))?,
        };
        match core.sync_role_definitions(&context, &desired) {
            Ok(result) => return Ok(result),
            Err(KernelError::StateVersionConflict(_)) if attempt == 0 => {}
            Err(KernelError::StateVersionConflict(_)) => {
                return Err(blocked(
                    "role sync conflicted with another change twice; run the command again",
                ))
            }
            Err(other) => return Err(kernel_failure(&other)),
        }
    }
    unreachable!("the loop returns")
}
