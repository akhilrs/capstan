//! Read-only discovery of the projects on this machine that have a controller: the roots of the running `cstan daemon`
//! processes (found as `cstan controllers` finds them), plus roots the caller already knows, such as a cursor's.
//!
//! Nothing is written and no daemon is started, stopped or signalled. A project whose daemon never ran, and that the
//! caller does not name, is not listed; a registry would be a separate decision. The environment (`CAPSTAN_SOCKET`,
//! `CAPSTAN_TOKEN`) is never read, and only the current user's processes are looked at.
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use capstan_client::{call_daemon, Response};
use capstan_daemon::deps::SOCKET_NAME;

use crate::controllers::{operator_key, Proc};

/// A root holding this file (any content) is skipped by discovery: it is never asked for its key nor polled. Test and
/// scratch daemons create it; a real project that does is hidden from `cstan events`.
pub const UNDISCOVERABLE_MARKER: &str = ".capstan/undiscoverable";

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

/// What discovery needs from outside the process list, so a test can supply it. Discovery asks every root from its
/// own thread, so a probe is `Sync`.
pub trait ProjectProbe: Sync {
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
        match call_within(root, credential, &["head".to_string()], ASK_TIMEOUT_MS) {
            Some(Ok(Response::Ok { .. })) => EndpointState::Ready,
            Some(Ok(Response::Refused { code, .. })) => classify(&code.to_utf8_lossy()),
            _ => EndpointState::Silent,
        }
    }
}

/// The state a refusal `code` of a `feed` call means: the key was not accepted, the daemon does not know `feed`, or
/// (any other code) no usable answer.
pub fn classify(code: &str) -> EndpointState {
    match code {
        "unauthorized" | "forbidden" => EndpointState::Refused,
        "unknown_command" => EndpointState::Unsupported,
        _ => EndpointState::Silent,
    }
}

/// Roots with a helper thread still running, so a root has at most one call in flight.
static IN_FLIGHT: std::sync::Mutex<BTreeSet<PathBuf>> = std::sync::Mutex::new(BTreeSet::new());
/// Helper threads running now by root, for tests.
static HELPERS: std::sync::Mutex<BTreeMap<PathBuf, usize>> = std::sync::Mutex::new(BTreeMap::new());

/// Removes the root from `IN_FLIGHT` when the helper thread ends, even by a panic.
struct Flight(PathBuf);

impl Drop for Flight {
    fn drop(&mut self) {
        if let Ok(mut set) = IN_FLIGHT.lock() {
            set.remove(&self.0);
        }
        if let Ok(mut map) = HELPERS.lock() {
            if let Some(count) = map.get_mut(&self.0) {
                *count = count.saturating_sub(1);
            }
        }
    }
}

/// The helper threads of `call_within` for `root` that are running now.
pub fn helper_threads(root: &Path) -> usize {
    HELPERS
        .lock()
        .ok()
        .and_then(|map| map.get(root).copied())
        .unwrap_or(0)
}

/// One `feed` call to the controller of `root` that gives up after `total_ms` in all. The client library's timeout is
/// idle-based, so a peer that sends a byte now and then could hold a call for ever; the call runs on a helper thread
/// and a call past the deadline is abandoned (its thread ends on the idle timeout). A root keeps at most one call in
/// flight: while an abandoned call still runs, a new one for the root is not made and gets no answer, so abandoned
/// threads are bounded by the number of roots. `None` is no answer. A slow but live peer can therefore show as
/// silent for one poll: the call that passed its deadline is still running when the next one is asked for.
pub fn call_within(
    root: &Path,
    credential: &Credential,
    args: &[String],
    total_ms: u64,
) -> Option<Result<Response, capstan_client::CallError>> {
    let socket = socket_of(root);
    let key = credential.expose().to_string();
    let args = args.to_vec();
    if !IN_FLIGHT.lock().ok()?.insert(root.to_path_buf()) {
        return None;
    }
    if let Ok(mut map) = HELPERS.lock() {
        *map.entry(root.to_path_buf()).or_insert(0) += 1;
    }
    let flight = Flight(root.to_path_buf());
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new().spawn(move || {
        let answer = call_daemon(&socket, &key, "feed", &args, total_ms);
        // The root is free again before the caller can see the answer, so a call right after never meets a stale entry.
        drop(flight);
        let _ = tx.send(answer);
    });
    spawned.ok()?;
    rx.recv_timeout(std::time::Duration::from_millis(total_ms))
        .ok()
}

/// The project id of `.capstan/project.json`, when the file holds one.
pub fn project_id(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join(".capstan/project.json")).ok()?;
    let config: serde_json::Value = serde_json::from_str(&text).ok()?;
    let id = config.get("projectId")?.as_str()?;
    (!id.is_empty()).then(|| id.to_string())
}

pub fn socket_of(root: &Path) -> PathBuf {
    root.join(".capstan/state").join(SOCKET_NAME)
}

/// One endpoint per canonical root among the roots of `procs` and `extra_roots`, sorted by root. A root holding
/// `UNDISCOVERABLE_MARKER` is left out.
pub fn discover(
    procs: &[Proc],
    extra_roots: &[PathBuf],
    probe: &dyn ProjectProbe,
) -> Vec<Endpoint> {
    discover_asked(procs, extra_roots, &[], probe)
}

/// `discover`, where the roots in `asked` are a caller's own choice and are kept even when they hold the marker. A
/// root of `asked` that is not also in `procs` or `extra_roots` is listed too.
pub fn discover_asked(
    procs: &[Proc],
    extra_roots: &[PathBuf],
    asked: &[PathBuf],
    probe: &dyn ProjectProbe,
) -> Vec<Endpoint> {
    let canonical = |root: &PathBuf| std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
    let asked: BTreeSet<PathBuf> = asked.iter().map(canonical).collect();
    let mut roots: BTreeMap<PathBuf, Option<u32>> = BTreeMap::new();
    let mut note = |root: PathBuf, pid: Option<u32>| {
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        if !asked.contains(&root) && root.join(UNDISCOVERABLE_MARKER).exists() {
            return;
        }
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
    for root in &asked {
        note(root.clone(), None);
    }
    std::thread::scope(|scope| {
        let handles: Vec<_> = roots
            .into_iter()
            .map(|(root, pid)| {
                let again = root.clone();
                (again, pid, scope.spawn(move || endpoint(root, pid, probe)))
            })
            .collect();
        handles
            .into_iter()
            .map(|(root, pid, handle)| {
                // A probe that panicked is a controller that did not answer.
                handle.join().unwrap_or_else(|_| Endpoint {
                    socket: socket_of(&root),
                    root,
                    pid,
                    state: EndpointState::Silent,
                })
            })
            .collect()
    })
}

fn endpoint(root: PathBuf, pid: Option<u32>, probe: &dyn ProjectProbe) -> Endpoint {
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
}
