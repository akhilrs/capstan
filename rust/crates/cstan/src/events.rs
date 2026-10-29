//! The engine of `cstan events`: follows the controllers of this host and writes the stream of `docs/remote-protocol.md`
//! (hello, project, snapshot, event, cursor, heartbeat, resync and error lines) to a sink.
//!
//! The engine only sees narrow traits: where the feed answers come from ([`FeedSource`]), how a project is probed, which
//! controller processes run ([`Processes`]), the clock, and where lines go ([`LineSink`]). [`Deps::live`] wires the real
//! ones; a test supplies fakes. The engine keeps no global state and installs no signal handler: the caller sets the
//! `stop` flag and [`run_stream`] returns.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use serde_json::Value;

use crate::controllers::Proc;
use crate::projects::{Credential, ProjectProbe};

mod engine;
mod lines;
mod live;
mod roots;
#[cfg(test)]
mod tests;

pub use capstan_client::FeedCall;
pub use roots::discover_roots;

/// What a run was asked for.
#[derive(Clone, Debug)]
pub struct EventsPlan {
    /// The cursor text to resume from.
    pub cursor: Option<String>,
    /// Roots to include besides the ones discovery finds; relative paths are taken from the working directory.
    pub roots: Vec<PathBuf>,
    /// The poll interval; below `Limits::min_poll_ms` it is raised to it.
    pub poll_ms: u64,
}

/// Where the lines go.
pub trait LineSink {
    /// Writes `lines` (each without its newline) with newlines and flushes once.
    fn write_batch(&mut self, lines: &[String]) -> std::io::Result<()>;
}

/// Why a feed call gave no answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FeedFailure {
    /// The controller answered with a refusal.
    Refused { code: String, message: String },
    /// Nothing answered in time, or the answer was no usable object.
    NoAnswer,
}

/// The `feed` route of the controller of a root. Discovery shares it between threads, so it is `Sync`.
pub trait FeedSource: Sync {
    fn call(&self, root: &Path, key: &Credential, call: &FeedCall) -> Result<Value, FeedFailure>;
}

/// The controller processes of this host.
pub trait Processes {
    fn list(&self) -> Vec<Proc>;
}

pub trait Clock: Sync {
    fn now_ms(&self) -> u64;
    fn sleep_ms(&self, ms: u64);
}

/// Sizes and times the stream keeps to.
#[derive(Clone, Debug)]
pub struct Limits {
    /// The longest line, newline included.
    pub line_bytes: usize,
    /// The most projects followed.
    pub projects: usize,
    pub min_poll_ms: u64,
    /// Silence after which a heartbeat is written.
    pub heartbeat_ms: u64,
    /// The most bytes of snapshot items held for one project.
    pub snapshot_buffer_bytes: usize,
    /// Consecutive missed polls after which a project last reported ready is reported silent.
    pub silent_after_misses: u32,
    /// Time since the first missed poll after which such a project is reported silent.
    pub silent_after_ms: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            line_bytes: 65_536,
            projects: 64,
            min_poll_ms: 1_000,
            heartbeat_ms: 15_000,
            snapshot_buffer_bytes: 64 * 1024 * 1024,
            silent_after_misses: 3,
            silent_after_ms: 10_000,
        }
    }
}

pub struct Deps {
    pub feed: Box<dyn FeedSource>,
    pub probe: Box<dyn ProjectProbe>,
    pub procs: Box<dyn Processes>,
    pub clock: Box<dyn Clock>,
    /// Notes for a person; never parsed.
    pub err: Box<dyn Write + Send>,
    pub limits: Limits,
    pub cwd: PathBuf,
}

impl Deps {
    /// The real controllers, process list and clock. Every call waits at most 2000 ms in all.
    pub fn live(cwd: PathBuf, err: Box<dyn Write + Send>) -> Deps {
        live::deps(cwd, err)
    }
}

/// Runs the stream until `stop` is set or the sink fails; the exit code of `cstan events`.
pub fn run_stream(
    plan: &EventsPlan,
    deps: &mut Deps,
    sink: &mut dyn LineSink,
    stop: &AtomicBool,
) -> i32 {
    engine::run(plan, deps, sink, stop)
}
