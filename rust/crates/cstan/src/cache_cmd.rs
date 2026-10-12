//! `cstan cache status|prune|clear`, and the `cache` member `cstan status` adds when no daemon answers.
//!
//! Everything runs locally from capstan.toml and `.capstan/project.json`, so the daemon need not be running. The one
//! exception is a caller with an agent environment, whose prune and clear go to the daemon's `cache` route (which lets
//! only the PM and the operator through). The guard prevents mistakes; it is not a security boundary.
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use capstan_cache::Runtime;
use capstan_config::{load_role_config, ConfigError, CONFIG_FILE_NAME};
use capstan_daemon::handlers::cache::{
    live_status, parse_request, run_clear, run_prune, runtime_of, status_member, CacheFailure,
    CacheRequest,
};
use capstan_daemon::run::load_project;
use serde_json::Value as Json;

use crate::agent::var;
use crate::{blocked, invalid, output, parse_options, render, runtime, to_js, Cli, Fail, Flow};

/// How long `cstan cache status` may walk the disk.
const STATUS_BUDGET: Duration = Duration::from_secs(30);

fn failure(failure: CacheFailure) -> Fail {
    match failure {
        CacheFailure::Invalid(message) => invalid(message),
        CacheFailure::Refused(message) => blocked(message),
        CacheFailure::Failed(message) => runtime(message),
    }
}

impl Cli<'_> {
    /// The project root: the working directory, or (in an agent's worktree, which has no `.capstan`) the project whose
    /// socket the agent environment names.
    fn cache_project_root(&self) -> PathBuf {
        let cwd = self.ctx.cwd.clone();
        if cwd.join(".capstan").is_dir() {
            return cwd;
        }
        var(&self.ctx.env, "CAPSTAN_SOCKET")
            .map(PathBuf::from)
            .and_then(|socket| {
                let state = socket.parent()?;
                let marker = state.parent()?;
                (state.file_name()? == "state" && marker.file_name()? == ".capstan")
                    .then(|| marker.parent().map(PathBuf::from))
                    .flatten()
            })
            .unwrap_or(cwd)
    }

    /// The runtime of the project's `[cache]` section; `Ok(None)` when it has none.
    fn cache_runtime(&self) -> Result<Option<Runtime>, Fail> {
        let root = self.cache_project_root();
        let config = load_role_config(&root).map_err(|error| match error {
            ConfigError::Invalid(message) => invalid(message),
            ConfigError::Parse(parse) => invalid(format!(
                "{CONFIG_FILE_NAME} is not valid TOML ({})",
                parse.detail
            )),
            ConfigError::Unreadable(message) => runtime(message),
        })?;
        if config.cache.is_none() {
            return Ok(None);
        }
        let (project, _) = load_project(&root).map_err(|error| invalid(error.message))?;
        Ok(runtime_of(
            &config,
            &root,
            &project.project_id,
            &self.base_env(),
        ))
    }

    fn base_env(&self) -> BTreeMap<String, String> {
        self.ctx
            .env
            .iter()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.to_string_lossy().into_owned(),
                )
            })
            .collect()
    }

    /// The `cache` member for `cstan status` when no daemon answered: read from the manifest, never from the disk.
    /// `None` without `[cache]` (or when capstan.toml cannot be read, which status does not need).
    pub fn cache_status_member(&self) -> Option<Json> {
        let runtime = self.cache_runtime().ok().flatten()?;
        Some(status_member(&runtime))
    }

    /// `cstan cache status|prune|clear`.
    pub fn cache(&mut self, rest: &[String]) -> Result<Flow, Fail> {
        let (positional, json) = parse_options(rest);
        let Some(verb) = positional.first().map(String::as_str) else {
            return Err(Fail::Usage);
        };
        match verb {
            "status" if positional.len() == 1 => self.cache_status(json),
            "prune" | "clear" => {
                let request = parse_request(&positional).map_err(failure)?;
                if self.agent_environment()?.is_some() {
                    return self.run_routed("cache", &positional, json);
                }
                self.cache_local(request, json)
            }
            _ => Err(Fail::Usage),
        }
    }

    fn cache_runtime_required(&self) -> Result<Runtime, Fail> {
        self.cache_runtime()?
            .ok_or_else(|| invalid(format!("{CONFIG_FILE_NAME} has no [cache] section")))
    }

    fn cache_status(&mut self, json: bool) -> Result<Flow, Fail> {
        let runtime = self.cache_runtime_required()?;
        let result = live_status(&runtime, STATUS_BUDGET);
        self.cache_print(&result, json);
        for warning in &runtime.warnings {
            self.io.err_line(&format!("warning: {warning}"));
        }
        Ok(Flow::Code(0))
    }

    fn cache_local(&mut self, request: CacheRequest, json: bool) -> Result<Flow, Fail> {
        let runtime = self.cache_runtime_required()?;
        let env = self.base_env();
        let result = match request {
            CacheRequest::Prune { names, dry_run } => run_prune(&runtime, &names, dry_run, &env),
            CacheRequest::Clear { selection, force } => {
                run_clear(&runtime, selection.as_deref(), force, &env)
            }
        }
        .map_err(failure)?;
        self.cache_print(&result, json);
        Ok(Flow::Code(0))
    }

    fn cache_print(&mut self, result: &Json, json: bool) {
        let value = to_js(result);
        if json {
            output(&mut self.io.out, Some(&value), true);
        } else {
            for line in render::cache_lines(Some(&value), self.ctx.now_ms) {
                self.io.out_line(&line);
            }
        }
    }
}
