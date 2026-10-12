//! The cache command (`cstan cache prune` and `cstan cache clear` of an agent environment), and the operations behind
//! it, which `cstan cache` also runs locally when there is no agent environment and the daemon's own loop uses.
//!
//! The guard is advisory: only the PM and the operator reach the operations through the route. An agent can drop its
//! environment and run the local path; the worst case is that a rebuildable cache is cleared.

use super::shared::{fail, ok, CommandCall, CommandEnv, CommandResponse, ErrorCode};
use super::HandlerMap;
use capstan_cache::{
    clear, is_idle, measure, prune, read_manifest, setup_keys, write_manifest, CacheError,
    CommandOutcome, Manifest, Policy, Runtime, SETUP_ENTRY,
};
use capstan_config::{CacheScope, RoleConfig};
use capstan_launcher::api::SetupOutcome;
use capstan_launcher::setup::run_setup_command;
use capstan_ledger::{iso_from_millis, now_millis};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

/// How long one measurement may walk the disk.
pub const MEASURE_BUDGET: Duration = Duration::from_secs(60);

/// Runs `operation` again while the store is busy, for about a second: another prune or clear holds the store lock, or a
/// process forked at this moment still shares its descriptor for the instant before it executes. `Busy` is raised
/// before anything is changed, so a retry is safe.
pub fn retrying<T>(
    mut operation: impl FnMut() -> capstan_cache::Result<T>,
) -> capstan_cache::Result<T> {
    let mut attempt = 0;
    loop {
        match operation() {
            Err(CacheError::Busy) if attempt < 20 => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
            other => return other,
        }
    }
}

/// Registers the handler of the route this module serves (cache).
pub fn register(map: &mut HandlerMap) {
    map.insert("cache", cache_command);
}

/// Why a cache operation did not run: the caller's request is wrong, the store refuses it for now, or it failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CacheFailure {
    Invalid(String),
    Refused(String),
    Failed(String),
}

impl CacheFailure {
    pub fn message(&self) -> &str {
        match self {
            Self::Invalid(m) | Self::Refused(m) | Self::Failed(m) => m,
        }
    }
}

impl From<CacheError> for CacheFailure {
    fn from(error: CacheError) -> Self {
        let message = error.to_string();
        match error {
            CacheError::Busy | CacheError::NotIdle => Self::Refused(message),
            CacheError::Invalid(_) => Self::Invalid(message),
            CacheError::Io(_) => Self::Failed(message),
        }
    }
}

/// What a `cache` request asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CacheRequest {
    Prune {
        names: Vec<String>,
        dry_run: bool,
    },
    Clear {
        /// `None` is `--all`.
        selection: Option<Vec<String>>,
        force: bool,
    },
}

/// Parses `prune [<entry>...] [--dry-run]` and `clear <entry>...|--all [--force]` (the words after `cache`).
pub fn parse_request(args: &[String]) -> Result<CacheRequest, CacheFailure> {
    let invalid = |m: &str| Err(CacheFailure::Invalid(m.to_string()));
    let Some((verb, rest)) = args.split_first() else {
        return invalid("cache takes prune or clear");
    };
    let (mut names, mut dry_run, mut force, mut all) = (Vec::new(), false, false, false);
    for arg in rest {
        match arg.as_str() {
            "--dry-run" => dry_run = true,
            "--force" => force = true,
            "--all" => all = true,
            flag if flag.starts_with("--") => {
                return Err(CacheFailure::Invalid(format!("unknown option {flag}")))
            }
            name => names.push(name.to_string()),
        }
    }
    match verb.as_str() {
        "prune" if !all && !force => Ok(CacheRequest::Prune { names, dry_run }),
        "prune" => invalid("cache prune takes entry names and --dry-run"),
        "clear" if dry_run => invalid("cache clear takes entry names or --all, and --force"),
        "clear" if all && !names.is_empty() => {
            invalid("cache clear takes entry names or --all, not both")
        }
        "clear" if !all && names.is_empty() => invalid("cache clear needs an entry name or --all"),
        "clear" => Ok(CacheRequest::Clear {
            selection: (!all).then_some(names),
            force,
        }),
        _ => invalid("cache takes prune or clear"),
    }
}

/// The runtime of the project's `[cache]` section; `None` when it has none. Nothing is created beyond the root.
pub fn runtime_of(
    config: &RoleConfig,
    project_root: &Path,
    project_id: &str,
    env: &BTreeMap<String, String>,
) -> Option<Runtime> {
    let cache = config.cache.as_ref()?;
    let path = env.get("PATH").map(String::as_str).unwrap_or_default();
    Some(Runtime::from_config(
        cache,
        project_root,
        project_id,
        env,
        path,
    ))
}

/// The process environment as the map the runtime reads.
pub fn process_env() -> BTreeMap<String, String> {
    std::env::vars().collect()
}

/// The runtime of the daemon's project, from its own configuration and environment.
pub fn runtime_of_deps(deps: &crate::deps::Deps) -> Option<Runtime> {
    let config = deps.options.capstan.as_deref()?;
    runtime_of(
        config,
        &deps.options.workspace_root,
        &deps.options.project.project_id,
        &process_env(),
    )
}

/// Runs a prune or stop command through the launcher's sanitising command runner.
pub fn run_command(
    command: &str,
    cwd: &Path,
    env: &BTreeMap<String, String>,
    timeout_ms: u64,
) -> CommandOutcome {
    match run_setup_command(command, &cwd.to_string_lossy(), timeout_ms, env) {
        SetupOutcome::Ok => CommandOutcome {
            ok: true,
            message: String::new(),
        },
        SetupOutcome::Failed { exit_code, output } => CommandOutcome {
            ok: false,
            message: match exit_code {
                Some(code) => format!("exit {code}: {}", output.trim()),
                None => output.trim().to_string(),
            },
        },
        SetupOutcome::Timeout => CommandOutcome {
            ok: false,
            message: "timed out".to_string(),
        },
    }
}

/// Measures every entry and writes the manifest. `last_prune_at_ms` is what the manifest records as the last prune.
pub fn refresh_manifest(
    runtime: &Runtime,
    last_prune_at_ms: Option<u64>,
) -> Result<Manifest, CacheFailure> {
    let sizes = measure(runtime, Instant::now() + MEASURE_BUDGET);
    let mut manifest = Manifest::new(runtime, sizes, now_millis().max(0) as u64);
    manifest.last_prune_at_ms = last_prune_at_ms;
    retrying(|| write_manifest(runtime, &manifest))?;
    Ok(manifest)
}

fn last_prune_of(runtime: &Runtime) -> Option<u64> {
    read_manifest(&runtime.root)
        .ok()
        .flatten()
        .and_then(|m| m.last_prune_at_ms)
}

/// `runtime` with only the named entries (all when `names` is empty).
fn scoped(runtime: &Runtime, names: &[String]) -> Result<Runtime, CacheFailure> {
    let mut scoped = runtime.clone();
    if names.is_empty() {
        return Ok(scoped);
    }
    for name in names {
        if runtime.entry(name).is_none() {
            return Err(CacheFailure::Invalid(format!(
                "no cache entry named {name}"
            )));
        }
    }
    scoped.entries.retain(|e| names.contains(&e.name));
    Ok(scoped)
}

/// Runs the tool prune commands and the age and size policy. Entries are removed only when no linked worktree exists.
/// With `dry_run` nothing runs and nothing is removed: the answer lists the commands that would run and the entries
/// that would go.
pub fn run_prune(
    runtime: &Runtime,
    names: &[String],
    dry_run: bool,
    base_env: &BTreeMap<String, String>,
) -> Result<Value, CacheFailure> {
    let mut target = scoped(runtime, names)?;
    let idle = is_idle(&runtime.project_root)?;
    let before = last_prune_of(runtime);
    let mut would_run = Vec::new();
    if dry_run {
        for entry in target.entries.iter_mut() {
            if entry.enabled && runtime.entry_dir(&entry.name).is_dir() {
                would_run.extend(
                    entry
                        .prune
                        .iter()
                        .map(|c| json!({"entry": entry.name, "command": c})),
                );
            }
            entry.prune.clear();
        }
    }
    let policy = Policy::for_runtime(&target, idle && !dry_run);
    let report = retrying(|| prune(&target, policy, base_env, &run_command))?;
    // The prune wrote a manifest of only the selected entries: measure them all again.
    let last = if dry_run {
        before
    } else {
        Some(now_millis().max(0) as u64)
    };
    let manifest = refresh_manifest(runtime, last)?;
    let total_after = manifest.entries.iter().map(|e| e.bytes).sum::<u64>();
    Ok(json!({
        "dryRun": dry_run,
        "idle": idle,
        "commands": report.commands.iter().map(|c| json!({
            "entry": c.entry, "command": c.command, "ok": c.ok, "message": c.message,
        })).collect::<Vec<_>>(),
        "wouldRun": would_run,
        "cleared": report.cleared,
        "wouldClear": report.would_clear,
        "totalBefore": report.total_before,
        "totalAfter": if dry_run { report.total_before } else { total_after },
        "partial": report.partial,
        "errors": report.errors,
    }))
}

/// Clears the named entries (`None`: all). The store refuses while a linked worktree exists unless `force`, and a
/// store shared between projects (`scope = "user"`) is never cleared without it.
pub fn run_clear(
    runtime: &Runtime,
    selection: Option<&[String]>,
    force: bool,
    base_env: &BTreeMap<String, String>,
) -> Result<Value, CacheFailure> {
    if runtime.scope == CacheScope::User && !force {
        return Err(CacheFailure::Refused(
            "the cache is shared between projects (scope user); use --force to clear it".into(),
        ));
    }
    let before = last_prune_of(runtime);
    let report = retrying(|| clear(runtime, selection, force, base_env, &run_command))?;
    // A cleared setup snapshot drops the whole setup entry from the manifest: measure again before anything is shown.
    let manifest = refresh_manifest(runtime, before)?;
    Ok(json!({
        "cleared": report.cleared,
        "stopCommands": report.stop_commands.iter().map(|c| json!({
            "entry": c.entry, "command": c.command, "ok": c.ok, "message": c.message,
        })).collect::<Vec<_>>(),
        "errors": report.errors,
        "totalBytes": manifest.entries.iter().map(|e| e.bytes).sum::<u64>(),
    }))
}

fn iso(ms: u64) -> Value {
    json!(iso_from_millis(ms as i64))
}

/// The `cache` member of `status`: the entries as the runtime resolves them with the sizes of the manifest. It never
/// walks the disk.
pub fn status_member(runtime: &Runtime) -> Value {
    let manifest = read_manifest(&runtime.root).ok().flatten();
    let size_of = |name: &str| {
        manifest
            .as_ref()
            .and_then(|m| m.entries.iter().find(|e| e.name == name))
    };
    let entries: Vec<Value> = runtime
        .entries
        .iter()
        .map(|entry| {
            let size = size_of(&entry.name);
            json!({
                "name": entry.name,
                "enabled": entry.enabled,
                "reason": entry.reason,
                "bytes": size.map(|s| s.bytes),
                "lastUsedAt": size.filter(|s| s.last_used_ms > 0).map(|s| iso(s.last_used_ms)),
            })
        })
        .collect();
    let total = manifest.as_ref().map(|_| {
        runtime
            .entries
            .iter()
            .filter_map(|e| size_of(&e.name))
            .map(|s| s.bytes)
            .sum::<u64>()
    });
    json!({
        "root": runtime.root.to_string_lossy(),
        "scope": runtime.scope.as_str(),
        "entries": entries,
        "totalBytes": total,
        "maxBytes": runtime.max_bytes,
        "measuredAt": manifest.as_ref().map(|m| iso(m.measured_at_ms)),
        "warnings": runtime.warnings,
    })
}

/// The live view `cstan cache status` prints: bounded walk of every entry.
pub fn live_status(runtime: &Runtime, budget: Duration) -> Value {
    let deadline = Instant::now() + budget;
    let sizes = measure(runtime, deadline);
    let (keys, keys_partial) = if runtime.entry(SETUP_ENTRY).is_some() {
        setup_keys(runtime, deadline)
    } else {
        (Vec::new(), false)
    };
    let mut partial = keys_partial;
    let entries: Vec<Value> = runtime
        .entries
        .iter()
        .map(|entry| {
            let size = sizes.iter().find(|s| s.name == entry.name);
            partial |= size.is_some_and(|s| s.partial);
            json!({
                "name": entry.name,
                "origin": entry.origin.as_str(),
                "enabled": entry.enabled,
                "reason": entry.reason,
                "mode": entry.mode,
                "bytes": size.map_or(0, |s| s.bytes),
                "files": size.map_or(0, |s| s.files),
                "lastUsedAt": size.filter(|s| s.last_used_ms > 0).map(|s| iso(s.last_used_ms)),
                "partial": size.is_some_and(|s| s.partial),
                "dir": runtime.entry_dir(&entry.name).to_string_lossy(),
            })
        })
        .collect();
    json!({
        "schemaVersion": 1,
        "root": runtime.root.to_string_lossy(),
        "scope": runtime.scope.as_str(),
        "maxBytes": runtime.max_bytes,
        "maxAgeDays": runtime.max_age_days,
        "totalBytes": sizes.iter().map(|s| s.bytes).sum::<u64>(),
        "partial": partial,
        "measuredAt": iso(now_millis().max(0) as u64),
        "entries": entries,
        "setupSnapshots": keys.iter().map(|k| json!({
            "key": k.key, "bytes": k.bytes, "files": k.files,
            "lastUsedAt": (k.last_used_ms > 0).then(|| iso(k.last_used_ms)),
        })).collect::<Vec<_>>(),
        "warnings": runtime.warnings,
    })
}

fn failure_response(failure: &CacheFailure) -> CommandResponse {
    let code = match failure {
        CacheFailure::Invalid(_) => ErrorCode::InvalidRequest,
        CacheFailure::Refused(_) => ErrorCode::Rejected,
        CacheFailure::Failed(_) => ErrorCode::Error,
    };
    fail(code, failure.message().to_string())
}

fn cache_command(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    if env.worker_manager(call.identity).is_none() {
        return Some(fail(
            ErrorCode::Forbidden,
            "only the PM or the operator may prune or clear the cache",
        ));
    }
    let request = match parse_request(call.args) {
        Ok(request) => request,
        Err(failure) => return Some(failure_response(&failure)),
    };
    let Some(runtime) = runtime_of_deps(env.deps) else {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "capstan.toml has no [cache] section",
        ));
    };
    let base_env = process_env();
    let result = match request {
        CacheRequest::Prune { names, dry_run } => run_prune(&runtime, &names, dry_run, &base_env),
        CacheRequest::Clear { selection, force } => {
            run_clear(&runtime, selection.as_deref(), force, &base_env)
        }
    };
    Some(match result {
        Ok(value) => ok(value),
        Err(failure) => failure_response(&failure),
    })
}
