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
pub use manifest::{
    read_manifest, write_manifest, write_manifest_hooked, Manifest, OccupantRecord, StoreLock,
};
pub use measure::{
    measure, measure_dir, setup_keys, DirSize, EntrySize, SetupKey, SlotSize, UnitSize,
};
pub use prune::{
    clear, clear_hooked, clear_with_hooks, over_cap, prune, prune_with_hooks, prune_with_occupants,
    recover, set_after_lock_open_hook, ClearReport, CommandOutcome, CommandRun, Occupant, OverCap,
    Policy, PruneHooks, PruneReport, Runner, Skipped, BUILDING_REASON, GUARD_BUSY_REASON,
    IN_PROCESS_REASON, NOT_IDLE_REASON, NO_UNITS_REASON, OCCUPANT_REASON, RECENT_REASON,
    TOOL_TRIMMED_REASON,
};
pub use root::resolve_root;
pub use runtime::{GateScope, Origin, ResolvedEntry, Runtime, SETUP_ENTRY};
pub use setup::setup_key;

/// The variable names copied from the base environment into every prune and stop command.
pub const BASE_ENV_ALLOWLIST: [&str; 8] = [
    "PATH", "HOME", "USER", "LOGNAME", "LANG", "LC_ALL", "TERM", "TMPDIR",
];
