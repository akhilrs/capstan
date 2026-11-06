//! The tool-cache store behind `[cache]`: where the caches live, the environment that points tools at them, and the
//! measure, prune and clear operations. Nothing here knows a language: presets and markers come from `capstan-config`.
mod error;
mod fsutil;
mod idle;
pub mod lease;
mod manifest;
mod measure;
mod prune;
mod root;
mod runtime;
mod setup;

pub use error::{CacheError, Result};
pub use idle::is_idle;
pub use lease::{GateLease, GateLeases, LeaseError};
pub use manifest::{read_manifest, write_manifest, write_manifest_hooked, Manifest, StoreLock};
pub use measure::{measure, setup_keys, EntrySize, SetupKey, SlotSize};
pub use prune::{
    clear, clear_hooked, clear_with_hooks, prune, prune_with_hooks, recover, ClearReport,
    CommandOutcome, CommandRun, Policy, PruneHooks, PruneReport, Runner, Skipped,
    GUARD_BUSY_REASON,
};
pub use root::resolve_root;
pub use runtime::{GateScope, Origin, ResolvedEntry, Runtime, SETUP_ENTRY};
pub use setup::setup_key;

/// The variable names copied from the base environment into every prune and stop command.
pub const BASE_ENV_ALLOWLIST: [&str; 8] = [
    "PATH", "HOME", "USER", "LOGNAME", "LANG", "LC_ALL", "TERM", "TMPDIR",
];
