//! The capstan configuration reader (`src/config/capstan-config.ts`) and the role prompt texts, in Rust. A configuration
//! loads to the same `RoleConfig` JSON, warnings or `ConfigError` text as the Node loader; a text the TOML parser
//! refuses is a distinct `ConfigError::Parse`, so a caller can hand the command to Node, which words that error with a
//! line and column. Nothing here runs until a caller asks for it.
use std::fs;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

mod disagreements;
mod error;
mod json;
mod operator_policy;
pub mod primitives;
pub mod prompts;
mod researcher_policy;
mod resolve;
pub mod starter;
mod text;
mod types;
mod value;

pub use disagreements::node_refuses;
pub use error::{ConfigError, ParseError, Result};
pub use json::{digest_json, sha256};
pub use operator_policy::auto_approve_rule_problem;
pub use resolve::parse_config;
pub use types::*;

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
