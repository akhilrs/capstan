//! The capstan configuration reader (`src/config/capstan-config.ts`) and the role prompt texts, in Rust. A configuration
//! loads to the same `RoleConfig` JSON, warnings or `ConfigError` text as the Node loader, and a text that is not TOML is
//! refused with the same line and column: the TOML is read by a port of `smol-toml` (`smol.rs`), not by another parser.
//! Nothing here runs until a caller asks for it.
use std::fs;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

pub mod cache;
pub mod design;
pub mod design_guidance;
mod error;
pub mod impact;
mod json;
pub mod keys;
mod operator_policy;
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
mod types;
mod value;
pub mod verify;

pub use cache::{
    CacheConfig, CacheEntry, CacheOrigin, CacheScope, SetupReuse, DETECT_MARKERS, DETECT_MAX_DEPTH,
    DETECT_SKIP_DIRS,
};
pub use design::{Design, GuidanceMode, GuidanceRules, RecordMode};
pub use error::{ConfigError, ParseError, Result};
pub use impact::{ImpactConfig, NextStep};
pub use json::{digest_json, sha256};
pub use keys::{accepted_keys, KeyPath};
pub use operator_policy::{
    approval_floor_entry_problem, auto_approve_rule_problem, parse_auto_window,
    timed_safe_command_problem, AutoWindowKind,
};
pub use remote::Remote;
pub use resolve::{
    parse_config, parse_config_with_home, DAEMON_NODE_REMOVED, DAEMON_NODE_REMOVED_ENV,
    DAEMON_RUST_ENV_WARNING, DAEMON_RUST_WARNING,
};
pub use review::Review;
pub use smol::{refusal as toml_refusal, Position as TomlPosition};
pub use types::*;
pub use verify::Verify;

/// `loadCapstanConfig` / `loadRoleConfig`: reads `capstan.toml` in `cwd` and resolves it.
pub fn load_role_config(cwd: &Path) -> Result<RoleConfig> {
    let file = cwd.join(CONFIG_FILE_NAME);
    let mut descriptor = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&file)
    {
        Ok(descriptor) => descriptor,
        Err(error) => {
            return Err(match error.raw_os_error() {
                Some(libc::ENOENT) => {
                    ConfigError::Invalid(format!("{CONFIG_FILE_NAME} does not exist"))
                }
                Some(libc::ELOOP) => {
                    ConfigError::Invalid(format!("{CONFIG_FILE_NAME} must be a regular file"))
                }
                _ => ConfigError::Unreadable(error.to_string()),
            })
        }
    };
    let metadata = descriptor
        .metadata()
        .map_err(|error| ConfigError::Unreadable(error.to_string()))?;
    if !metadata.is_file() {
        return Err(ConfigError::Invalid(format!(
            "{CONFIG_FILE_NAME} must be a regular file"
        )));
    }
    // SAFETY: getuid has no preconditions and cannot fail.
    if metadata.uid() != unsafe { libc::getuid() } {
        return Err(ConfigError::Invalid(format!(
            "{CONFIG_FILE_NAME} must be owned by the current user"
        )));
    }
    if metadata.mode() & 0o022 != 0 {
        return Err(ConfigError::Invalid(format!(
            "{CONFIG_FILE_NAME} must not be writable by group or others"
        )));
    }
    if metadata.len() > primitives::MAX_FILE_BYTES {
        return Err(ConfigError::Invalid(format!(
            "{CONFIG_FILE_NAME} exceeds {} bytes",
            primitives::MAX_FILE_BYTES
        )));
    }
    let mut bytes = Vec::new();
    (&mut descriptor)
        .take(primitives::MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| ConfigError::Unreadable(error.to_string()))?;
    parse_config(&bytes, cwd)
}
