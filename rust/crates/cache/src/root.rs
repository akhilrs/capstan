use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use capstan_config::{CacheConfig, CacheScope};

use crate::fsutil::mkdir_private;

fn home_of(env: &BTreeMap<String, String>) -> Option<PathBuf> {
    env.get("HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
}

/// A project id as one safe path segment.
fn segment(project_id: &str) -> String {
    let cleaned: String = project_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_start_matches('.');
    if trimmed.is_empty() {
        "project".into()
    } else {
        trimmed.chars().take(128).collect()
    }
}

/// The store root: the configured root, else `$XDG_CACHE_HOME/capstan`, else `$HOME/.cache/capstan`; then
/// `projects/<projectId>` for scope project or `shared` for scope user. Missing directories are created 0700.
pub fn resolve_root(
    config: &CacheConfig,
    project_id: &str,
    env: &BTreeMap<String, String>,
) -> PathBuf {
    let base = match &config.base {
        Some(base) => match base.strip_prefix("~") {
            Ok(rest) => home_of(env).map_or_else(|| base.clone(), |home| home.join(rest)),
            Err(_) => base.clone(),
        },
        None => match env
            .get("XDG_CACHE_HOME")
            .filter(|x| Path::new(x).is_absolute())
        {
            Some(xdg) => PathBuf::from(xdg).join("capstan"),
            None => match home_of(env) {
                Some(home) => home.join(".cache").join("capstan"),
                // SAFETY: getuid has no preconditions.
                None => PathBuf::from(format!("/tmp/capstan-cache-{}", unsafe { libc::getuid() })),
            },
        },
    };
    let root = match config.scope {
        CacheScope::Project => base.join("projects").join(segment(project_id)),
        CacheScope::User => base.join("shared"),
    };
    // Best effort: `ensure` reports a directory that cannot be created.
    let _ = mkdir_private(&root);
    root
}
