//! The capstan configuration reader (`src/config/capstan-config.ts`) and the role prompt texts, in Rust. A configuration
//! loads to the same `RoleConfig` JSON, warnings or `ConfigError` text as the Node loader, and a text that is not TOML is
//! refused with the same line and column: the TOML is read by a port of `smol-toml` (`smol.rs`), not by another parser.
//! Nothing here runs until a caller asks for it.
use std::fs;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

pub mod cache;
pub mod design;
pub mod design_guidance;
mod error;
mod git_track;
pub mod impact;
mod json;
pub mod keys;
pub mod model_fit;
pub mod ntfy;
mod operator_policy;
pub mod operator_texts;
pub mod primitives;
pub mod prompts;
pub mod remote;
mod researcher_policy;
mod resolve;
pub mod review;
mod smol;
mod smol_date;
pub mod starter;
mod text;
mod trust;
mod trust_classes;
mod trust_display;
mod types;
mod value;
pub mod verify;

pub use cache::{
    CacheConfig, CacheEntry, CacheOrigin, CacheScope, SetupReuse, DETECT_MARKERS, DETECT_MAX_DEPTH,
    DETECT_SKIP_DIRS,
};
pub use design::{Design, GuidanceMode, GuidanceRules, RecordMode};
pub use error::{ConfigError, ParseError, Result};
#[doc(hidden)]
pub use git_track::local_file_tracking_with;
pub use git_track::Tracking as LocalTracking;
pub use impact::{ImpactConfig, NextStep};
pub use json::{digest_json, sha256};
pub use keys::{accepted_keys, KeyPath};
pub use model_fit::{
    check_model_value, fit_for_role, model_fits_host, MODEL_FAMILIES, MULTI_PROVIDER_HOSTS,
};
pub use ntfy::{
    check_secret_file, effective_prefix, read_secret, Ntfy, NtfyAuth, NtfyEvent, NtfyEventStyle,
    Secret, SecretError, NTFY_EVENTS,
};
pub use operator_policy::{
    approval_floor_entry_problem, auto_approve_rule_problem, parse_auto_window,
    timed_safe_command_problem, AutoWindowKind,
};
pub use remote::Remote;
pub use resolve::{
    parse_config, parse_config_with_home, parse_layered_config, parse_layered_config_with,
    tracked_local_only_refusal, tracked_remote_refusal, DAEMON_NODE_REMOVED,
    DAEMON_NODE_REMOVED_ENV, DAEMON_RUST_ENV_WARNING, DAEMON_RUST_WARNING,
};
pub use review::Review;
pub use smol::{refusal as toml_refusal, Position as TomlPosition};
pub use trust::{
    checked_subset, read_config_pair, record_current, record_trust, trust_status,
    trust_status_of_bytes, trust_status_with, ConfigPair, KeyChange, TrustChange, TrustError,
    TrustStatus, TrustStore, TrustWritten,
};
pub use types::*;
pub use verify::{path_matches, step_runs, Verify};

/// `loadCapstanConfig` / `loadRoleConfig`: reads `capstan.toml` in `cwd`, lays `capstan.local.toml` over it when that
/// file exists, and resolves the result. Every reader of the project configuration comes through here.
pub fn load_role_config(cwd: &Path) -> Result<RoleConfig> {
    load_role_config_with(cwd, &TrustStore::for_user())
}

/// `load_role_config` against the given trust store: the one loader with an explicit store, for product seams and
/// tests. The trust gate is always on. The pair is read once; a refusal of the text itself comes back unchanged, and
/// trust is judged on the very bytes that were parsed.
pub fn load_role_config_with(cwd: &Path, store: &TrustStore) -> Result<RoleConfig> {
    let pair = pair_of(cwd)?;
    let config = parse_pair(&pair, cwd)?;
    check_trusted(cwd, &pair, store)?;
    Ok(config)
}

fn pair_of(cwd: &Path) -> Result<ConfigPair> {
    read_config_pair(cwd)?
        .ok_or_else(|| ConfigError::Invalid(format!("{CONFIG_FILE_NAME} does not exist")))
}

fn parse_pair(pair: &ConfigPair, cwd: &Path) -> Result<RoleConfig> {
    let mut confirmed_untracked = true;
    if pair.local.is_some() {
        match pair.tracking {
            git_track::Tracking::Tracked => {
                return Err(ConfigError::Invalid(tracked_local_refusal()))
            }
            git_track::Tracking::Untracked | git_track::Tracking::NoRepository => {}
            // Fail closed: the file's [remote] merge/push/push_url/merge_branch are refused (see `parse_layered_config_with`).
            git_track::Tracking::Unknown => confirmed_untracked = false,
        }
    }
    let home = std::env::var("HOME").ok();
    parse_layered_config_with(
        &pair.main,
        pair.local.as_deref(),
        cwd,
        home.as_deref(),
        confirmed_untracked,
    )
}

fn check_trusted(cwd: &Path, pair: &ConfigPair, store: &TrustStore) -> Result<()> {
    let (keys, dir): (Vec<String>, Option<PathBuf>) = match trust_status_of_bytes(cwd, pair, store)
    {
        TrustStatus::NotNeeded | TrustStatus::Trusted => return Ok(()),
        TrustStatus::Needed {
            changes, store_dir, ..
        } => (
            changes.into_iter().map(|change| change.key).collect(),
            Some(store_dir),
        ),
        TrustStatus::Unavailable { reason, store_dir } => {
            let keys = checked_subset(pair)
                .map(|subset| subset.into_keys().collect())
                .unwrap_or_default();
            return Err(ConfigError::Untrusted(untrusted_text(
                keys,
                store_dir,
                Some(&reason),
            )));
        }
    };
    Err(ConfigError::Untrusted(untrusted_text(keys, dir, None)))
}

const MAX_NAMED_KEYS: usize = 20;

fn untrusted_text(keys: Vec<String>, dir: Option<PathBuf>, reason: Option<&str>) -> String {
    let mut keys = keys;
    keys.sort();
    let more = keys.len().saturating_sub(MAX_NAMED_KEYS);
    let total = keys.len();
    keys.truncate(MAX_NAMED_KEYS);
    let mut list = keys.join(", ");
    if more > 0 {
        list.push_str(&format!(", and {more} more"));
    }
    let head = format!("{CONFIG_FILE_NAME} has {total} key(s) not trusted on this machine: {list}");
    match (dir, reason) {
        (Some(dir), None) => format!(
            "{head} (trust store {}). Review the change and trust it with: cstan trust",
            dir.display()
        ),
        (_, reason) => format!(
            "{head} (no trust store: {}). Set CAPSTAN_TRUST_DIR to an absolute directory, then run: cstan trust",
            reason.unwrap_or("none is set")
        ),
    }
}

/// The refusal for a committed `capstan.local.toml`.
pub(crate) fn tracked_local_refusal() -> String {
    format!(
        "{LOCAL_CONFIG_FILE_NAME} is tracked in git and is not honoured: it holds machine settings and must never be committed; untrack it with `git rm --cached {LOCAL_CONFIG_FILE_NAME}`"
    )
}

/// A config file of `cwd` read under the loader's checks; `None` when it does not exist.
pub(crate) fn read_config_file(cwd: &Path, name: &str) -> Result<Option<Vec<u8>>> {
    let file = cwd.join(name);
    let mut descriptor = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&file)
    {
        Ok(descriptor) => descriptor,
        Err(error) => {
            return match error.raw_os_error() {
                Some(libc::ENOENT) => Ok(None),
                Some(libc::ELOOP) => Err(ConfigError::Invalid(format!(
                    "{name} must be a regular file"
                ))),
                _ => Err(ConfigError::Unreadable(error.to_string())),
            }
        }
    };
    let metadata = descriptor
        .metadata()
        .map_err(|error| ConfigError::Unreadable(error.to_string()))?;
    if !metadata.is_file() {
        return Err(ConfigError::Invalid(format!(
            "{name} must be a regular file"
        )));
    }
    // SAFETY: getuid has no preconditions and cannot fail.
    if metadata.uid() != unsafe { libc::getuid() } {
        return Err(ConfigError::Invalid(format!(
            "{name} must be owned by the current user"
        )));
    }
    if metadata.mode() & 0o022 != 0 {
        return Err(ConfigError::Invalid(format!(
            "{name} must not be writable by group or others"
        )));
    }
    if metadata.len() > primitives::MAX_FILE_BYTES {
        return Err(ConfigError::Invalid(format!(
            "{name} exceeds {} bytes",
            primitives::MAX_FILE_BYTES
        )));
    }
    let mut bytes = Vec::new();
    (&mut descriptor)
        .take(primitives::MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| ConfigError::Unreadable(error.to_string()))?;
    Ok(Some(bytes))
}

#[cfg(test)]
mod gate_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn text_is_sorted_and_capped() {
        let keys: Vec<String> = (0..25).rev().map(|n| format!("k{n:02}")).collect();
        let text = untrusted_text(keys, Some(PathBuf::from("/s")), None);
        assert!(text.contains("25 key(s)"), "{text}");
        assert!(text.contains("k00, k01"), "{text}");
        assert!(text.contains("k19, and 5 more (trust store /s)"), "{text}");
        assert!(!text.contains("k20"), "{text}");
        assert!(text.ends_with("cstan trust"), "{text}");
        let none = untrusted_text(vec!["b".into(), "a".into()], None, Some("why"));
        assert_eq!(
            none,
            "capstan.toml has 2 key(s) not trusted on this machine: a, b (no trust store: why). Set CAPSTAN_TRUST_DIR to an absolute directory, then run: cstan trust"
        );
    }

    #[test]
    fn the_verdict_is_for_the_bytes_read() {
        let tmp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap();
        let project = root.join("project");
        fs::create_dir(&project).unwrap();
        let write = |text: &str| {
            let file = project.join("capstan.toml");
            fs::write(&file, text).unwrap();
            fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        };
        let base = "schema_version = 1\n[nexora]\ntrack = \"never\"\n[hosts.claude]\nkind = \"claude\"\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n";
        write(&format!("{base}[cache]\nmax_size_gb = 1\n"));
        let store = TrustStore::at(root.join("store"));
        record_current(&project, &store).unwrap();
        let pair = pair_of(&project).unwrap();
        write(&format!("{base}[cache]\nmax_size_gb = 9\n"));
        parse_pair(&pair, &project).unwrap();
        check_trusted(&project, &pair, &store).unwrap();
        assert!(matches!(
            load_role_config_with(&project, &store),
            Err(ConfigError::Untrusted(_))
        ));
    }
}
