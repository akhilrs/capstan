//! The launcher of the capstan daemon (src/launcher.ts and src/launcher/): starts, restarts and re-adopts the agents that
//! live in Herdr panes, one operation at a time. `api` holds the service interface and a stub; the modules below it are
//! filled in by the package that owns each.

pub mod api;
pub mod git;
pub mod hub;
pub mod kernel;
pub mod launcher;
pub mod observe;
pub mod release;
pub mod setup;
pub mod shared;
pub mod spawn;
pub mod text;
