//! The `[cache]` environment of the launcher: the runtime is resolved once when the kernel is built, and every agent
//! and worktree hook gets the same variables from it.

use capstan_cache::{GateScope, Runtime};
use capstan_config::RoleConfig;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

/// Resolves `[cache]` against the agent PATH; `None` without the section. Each warning is passed to `log` once, as
/// `cache_tool_missing`.
pub fn build_runtime(
    config: &RoleConfig,
    project_root: &str,
    project_id: &str,
    base_environment: &HashMap<String, String>,
    log: &dyn Fn(&str, Value),
) -> Option<Runtime> {
    let cache = config.cache.as_ref()?;
    let env: BTreeMap<String, String> = base_environment
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let path = base_environment.get("PATH").map_or("", String::as_str);
    let runtime = Runtime::from_config(cache, Path::new(project_root), project_id, &env, path);
    for warning in &runtime.warnings {
        log("cache_tool_missing", json!({"message": warning}));
    }
    Some(runtime)
}

/// The variables of the enabled entries for `scope` plus `CAPSTAN_CACHE_DIR`, after creating the directories. An
/// error is the message of the failure; the caller spawns without cache variables. `Bare` gives no variables at all.
pub fn cache_extras(runtime: &Runtime, scope: GateScope) -> Result<Vec<(String, String)>, String> {
    cache_extras_for(runtime, scope, None)
}

/// `cache_extras` for the entries named in `names` (`None`: all); the setup entry is internal and always stays. A
/// workspace worker gets the entries of the repos it holds.
pub fn cache_extras_for(
    runtime: &Runtime,
    scope: GateScope,
    names: Option<&[String]>,
) -> Result<Vec<(String, String)>, String> {
    let filtered;
    let runtime = match names {
        None => runtime,
        Some(names) => {
            let mut limited = runtime.clone();
            limited.entries.retain(|entry| {
                entry.origin == capstan_cache::Origin::Internal
                    || names.iter().any(|name| name == &entry.name)
            });
            filtered = limited;
            &filtered
        }
    };
    let mut extras = runtime.ensure_scoped(scope).map_err(|e| e.to_string())?;
    if scope != GateScope::Bare {
        extras.push((
            "CAPSTAN_CACHE_DIR".to_string(),
            runtime.root.to_string_lossy().into_owned(),
        ));
    }
    Ok(extras)
}
