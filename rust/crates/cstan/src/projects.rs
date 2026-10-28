//! Read-only discovery of the projects on this machine that have a controller: the roots of the running `cstan daemon`
//! processes (found as `cstan controllers` finds them), plus roots the caller already knows, such as a cursor's.
//!
//! Nothing is written and no daemon is started, stopped or signalled. A project whose daemon never ran, and that the
//! caller does not name, is not listed; a registry would be a separate decision. The environment (`CAPSTAN_SOCKET`,
//! `CAPSTAN_TOKEN`) is never read, and only the current user's processes are looked at.
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use capstan_client::{call_daemon, Response};
use capstan_daemon::deps::SOCKET_NAME;

use crate::controllers::{operator_key, Proc};

/// How long one controller may take to answer.
const ASK_TIMEOUT_MS: u64 = 2_000;

/// An operator key. It prints as `<redacted>` in `Debug` and `Display`.
#[derive(Clone, PartialEq, Eq)]
pub struct Credential(String);

impl Credential {
    pub fn new(text: impl Into<String>) -> Self {
        Credential(text.into())
    }

    /// The key text, for making a call.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl fmt::Display for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// The operator key of `root`, read after the daemon's own ownership and mode checks.
pub fn credential(root: &Path) -> Option<Credential> {
    operator_key(root).map(Credential)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndpointState {
    Ready,
    /// No usable operator key.
    NoKey,
    /// Nothing listens, or no answer came.
    Silent,
    /// The daemon did not accept the key.
    Refused,
    /// The daemon answers but does not know `feed`.
    Unsupported,
    /// The root or its `.capstan` is missing.
    Gone,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    /// The canonical project root.
    pub root: PathBuf,
    pub socket: PathBuf,
    /// A running controller process of the root, the lowest pid when several.
    pub pid: Option<u32>,
    pub state: EndpointState,
}

/// What discovery needs from outside the process list, so a test can supply it.
pub trait ProjectProbe {
    fn key(&self, root: &Path) -> Option<Credential>;
    fn ask(&self, root: &Path, credential: &Credential) -> EndpointState;
}

/// The real probe: the key file and the control socket of the project, asked `feed head`.
pub struct Live;

impl ProjectProbe for Live {
    fn key(&self, root: &Path) -> Option<Credential> {
        credential(root)
    }

    fn ask(&self, root: &Path, credential: &Credential) -> EndpointState {
        let socket = socket_of(root);
        let args = ["head".to_string()];
        match call_daemon(&socket, credential.expose(), "feed", &args, ASK_TIMEOUT_MS) {
            Ok(Response::Ok { .. }) => EndpointState::Ready,
            Ok(Response::Refused { code, .. }) => match code.to_utf8_lossy().as_str() {
                "unauthorized" | "forbidden" => EndpointState::Refused,
                "unknown_command" => EndpointState::Unsupported,
                _ => EndpointState::Silent,
            },
            _ => EndpointState::Silent,
        }
    }
}

fn socket_of(root: &Path) -> PathBuf {
    root.join(".capstan/state").join(SOCKET_NAME)
}

/// One endpoint per canonical root among the roots of `procs` and `extra_roots`, sorted by root.
pub fn discover(
    procs: &[Proc],
    extra_roots: &[PathBuf],
    probe: &dyn ProjectProbe,
) -> Vec<Endpoint> {
    let mut roots: BTreeMap<PathBuf, Option<u32>> = BTreeMap::new();
    let mut note = |root: PathBuf, pid: Option<u32>| {
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        let slot = roots.entry(root).or_insert(pid);
        *slot = match (*slot, pid) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
    };
    for proc in procs {
        if let Some(root) = &proc.project {
            note(root.clone(), Some(proc.pid));
        }
    }
    for root in extra_roots {
        note(root.clone(), None);
    }
    roots
        .into_iter()
        .map(|(root, pid)| {
            let socket = socket_of(&root);
            let state = if !root.join(".capstan").is_dir() {
                EndpointState::Gone
            } else {
                match probe.key(&root) {
                    None => EndpointState::NoKey,
                    Some(key) => probe.ask(&root, &key),
                }
            };
            Endpoint {
                root,
                socket,
                pid,
                state,
            }
        })
        .collect()
}
