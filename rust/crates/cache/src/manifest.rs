use std::fs::{self, File};
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::error::{CacheError, Result};
use crate::fsutil::{mkdir_private, nanos};
use crate::measure::EntrySize;
use crate::prune::Skipped;
use crate::runtime::Runtime;

pub const MANIFEST_VERSION: u64 = 1;

/// A non-blocking exclusive lock on `<root>/.lock`, held for prune, clear and manifest writes.
#[derive(Debug)]
pub struct StoreLock {
    _file: File,
}

impl StoreLock {
    /// `Busy` when another holder (any process) has it.
    pub fn try_acquire(root: &Path) -> Result<StoreLock> {
        mkdir_private(root)?;
        let path = root.join(".lock");
        let file = File::options()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| CacheError::io(format!("open {}", path.display()), e))?;
        // SAFETY: the descriptor stays open for the lifetime of `file`.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = std::io::Error::last_os_error();
            return if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
                Err(CacheError::Busy)
            } else {
                Err(CacheError::io(format!("lock {}", path.display()), error))
            };
        }
        Ok(StoreLock { _file: file })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    pub version: u64,
    pub project_id: String,
    pub scope: String,
    pub entries: Vec<EntrySize>,
    pub measured_at_ms: u64,
    pub last_prune_at_ms: Option<u64>,
    /// What the last prune that knew the occupants left alone (`lastSkipped`).
    pub last_skipped: Vec<Skipped>,
    /// The scratch the last such prune counted against the cap but never removes (`lastOccupants`).
    pub last_occupants: Vec<OccupantRecord>,
}

/// One occupant as the last prune saw it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OccupantRecord {
    pub label: String,
    pub bytes: u64,
    pub at_ms: u64,
}

impl Manifest {
    pub fn new(runtime: &Runtime, entries: Vec<EntrySize>, measured_at_ms: u64) -> Manifest {
        Manifest {
            version: MANIFEST_VERSION,
            project_id: runtime.project_id.clone(),
            scope: runtime.scope.as_str().into(),
            entries,
            measured_at_ms,
            last_prune_at_ms: None,
            last_skipped: Vec::new(),
            last_occupants: Vec::new(),
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "version": self.version,
            "projectId": self.project_id,
            "scope": self.scope,
            "measuredAt": self.measured_at_ms,
            "lastPruneAt": self.last_prune_at_ms,
            "lastSkipped": self.last_skipped.iter().map(|s| json!({
                "label": s.label, "reason": s.reason,
            })).collect::<Vec<_>>(),
            "lastOccupants": self.last_occupants.iter().map(|o| json!({
                "label": o.label, "bytes": o.bytes, "at": o.at_ms,
            })).collect::<Vec<_>>(),
            "entries": self.entries.iter().map(|e| json!({
                "name": e.name, "bytes": e.bytes, "files": e.files,
                "lastUsedMs": e.last_used_ms, "partial": e.partial, "fixedBytes": e.fixed_bytes,
            })).collect::<Vec<_>>(),
        })
    }

    fn from_json(value: &Value) -> Option<Manifest> {
        let entries = value
            .get("entries")?
            .as_array()?
            .iter()
            .map(|e| {
                Some(EntrySize {
                    name: e.get("name")?.as_str()?.to_string(),
                    bytes: e.get("bytes")?.as_u64()?,
                    files: e.get("files")?.as_u64()?,
                    last_used_ms: e.get("lastUsedMs")?.as_u64()?,
                    partial: e.get("partial")?.as_bool()?,
                    slots: Vec::new(),
                    units: Vec::new(),
                    fixed_bytes: e.get("fixedBytes").and_then(Value::as_u64).unwrap_or(0),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Manifest {
            version: value.get("version")?.as_u64()?,
            project_id: value.get("projectId")?.as_str()?.to_string(),
            scope: value.get("scope")?.as_str()?.to_string(),
            entries,
            measured_at_ms: value.get("measuredAt")?.as_u64()?,
            last_prune_at_ms: value.get("lastPruneAt").and_then(Value::as_u64),
            last_skipped: value
                .get("lastSkipped")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(|s| {
                            Some(Skipped {
                                label: s.get("label")?.as_str()?.to_string(),
                                reason: s.get("reason")?.as_str()?.to_string(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
            last_occupants: value
                .get("lastOccupants")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(|o| {
                            Some(OccupantRecord {
                                label: o.get("label")?.as_str()?.to_string(),
                                bytes: o.get("bytes")?.as_u64()?,
                                at_ms: o.get("at")?.as_u64()?,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
    }
}

pub(crate) fn manifest_path(root: &Path) -> PathBuf {
    root.join("manifest.json")
}

/// The manifest, or `None` when there is none or it cannot be read (it is only a cache of measurements).
pub fn read_manifest(root: &Path) -> Result<Option<Manifest>> {
    match fs::read_to_string(manifest_path(root)) {
        Ok(text) => Ok(serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| Manifest::from_json(&v))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(CacheError::io("read manifest", e)),
    }
}

/// Writes the manifest under the store lock: temporary file, flush, rename.
pub fn write_manifest(runtime: &Runtime, manifest: &Manifest) -> Result<()> {
    let _lock = StoreLock::try_acquire(&runtime.root)?;
    write_locked(&runtime.root, manifest, &|| {})
}

/// `write_manifest` with a hook between writing the temporary file and renaming it; for kill-safety tests.
#[doc(hidden)]
pub fn write_manifest_hooked(
    runtime: &Runtime,
    manifest: &Manifest,
    between: &dyn Fn(),
) -> Result<()> {
    let _lock = StoreLock::try_acquire(&runtime.root)?;
    write_locked(&runtime.root, manifest, between)
}

/// The caller holds the store lock.
pub(crate) fn write_locked(root: &Path, manifest: &Manifest, between: &dyn Fn()) -> Result<()> {
    let tmp = root.join(format!(
        "manifest.json.tmp-{}-{}",
        std::process::id(),
        nanos()
    ));
    let mut text = serde_json::to_string_pretty(&manifest.to_json())
        .map_err(|e| CacheError::Io(format!("encode manifest: {e}")))?;
    text.push('\n');
    let write = || -> std::io::Result<()> {
        let mut file = File::create(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()
    };
    if let Err(e) = write() {
        let _ = fs::remove_file(&tmp);
        return Err(CacheError::io("write manifest", e));
    }
    between();
    fs::rename(&tmp, manifest_path(root)).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        CacheError::io("replace manifest", e)
    })
}
