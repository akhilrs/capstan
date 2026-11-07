//! The trust record of a project's `capstan.toml`: which checked values the user has accepted, kept per checkout in a
//! private per-user store, and the status that compares the file with that record. Nothing here changes what loads.
use std::collections::BTreeMap;
use std::ffi::{CStr, OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::{Map, Number, Value};

use crate::error::{invalid, Result};
use crate::git_track::{self, Tracking};
use crate::json::sha256;
use crate::types::{CONFIG_FILE_NAME, LOCAL_CONFIG_FILE_NAME};
use crate::value::{Item, Table};
use crate::{primitives, trust_classes, trust_display};

const RECORD_VERSION: u64 = 1;
const MAX_RECORD_BYTES: u64 = 8 * 1024 * 1024;
const STORE_FIX: &str = "chmod 700 it, or set CAPSTAN_TRUST_DIR to another directory";

/// The two files of a project folder as read under the loader's checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPair {
    pub main: Vec<u8>,
    pub local: Option<Vec<u8>>,
    /// What git says about the local file; `Untracked` when there is no local file.
    pub tracking: Tracking,
}

/// Reads `capstan.toml` (and `capstan.local.toml`) of exactly `dir`; `None` without `capstan.toml`.
pub fn read_config_pair(dir: &Path) -> Result<Option<ConfigPair>> {
    let Some(main) = crate::read_config_file(dir, CONFIG_FILE_NAME)? else {
        return Ok(None);
    };
    let local = crate::read_config_file(dir, LOCAL_CONFIG_FILE_NAME)?;
    let tracking = if local.is_some() {
        git_track::local_file_tracking(dir)
    } else {
        Tracking::Untracked
    };
    Ok(Some(ConfigPair {
        main,
        local,
        tracking,
    }))
}

// ---------------------------------------------------------------------------------------------------------------
// Paths and classes

fn bare_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn encode_path(segments: &[String]) -> String {
    segments
        .iter()
        .map(|s| {
            if bare_segment(s) {
                s.clone()
            } else {
                format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// The segments of a real path: a quoted segment (which may hold dots) is one segment.
fn split_path(path: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut chars = path.chars().peekable();
    loop {
        let mut segment = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => {
                        if let Some(next) = chars.next() {
                            segment.push(next);
                        }
                    }
                    '"' => break,
                    other => segment.push(other),
                }
            }
        }
        while let Some(&c) = chars.peek() {
            if c == '.' {
                break;
            }
            segment.push(c);
            chars.next();
        }
        segments.push(segment);
        if chars.next().is_none() {
            return segments;
        }
    }
}

fn is_placeholder(segment: &str) -> bool {
    segment.len() > 2 && segment.starts_with('<') && segment.ends_with('>')
}

fn patterns() -> &'static Vec<(String, Vec<String>)> {
    static PATTERNS: OnceLock<Vec<(String, Vec<String>)>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        crate::keys::accepted_keys()
            .into_iter()
            .map(|key| {
                let segments = key.path.split('.').map(str::to_string).collect();
                (key.path, segments)
            })
            .collect()
    })
}

/// The accepted key path (placeholders such as `roles.<name>.model`) a real path belongs to; a value deeper than the
/// accepted path (a table inside `roles.<name>.hooks`) belongs to the path above it. `None` when no accepted path fits.
pub(crate) fn accepted_path_of(real_path: &str) -> Option<String> {
    let real = split_path(real_path);
    let mut best: Option<&(String, Vec<String>)> = None;
    for pattern in patterns() {
        let fits = pattern.1.len() <= real.len()
            && pattern
                .1
                .iter()
                .zip(&real)
                .all(|(p, r)| is_placeholder(p) || p == r);
        if fits && best.is_none_or(|b| pattern.1.len() > b.1.len()) {
            best = Some(pattern);
        }
    }
    best.map(|b| b.0.clone())
}

// ---------------------------------------------------------------------------------------------------------------
// The checked subset and its canonical form

fn canonical(value: &Value) -> String {
    fn sorted(value: &Value) -> Value {
        match value {
            Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
            Value::Object(members) => {
                let ordered: BTreeMap<&String, Value> =
                    members.iter().map(|(k, v)| (k, sorted(v))).collect();
                let mut out = Map::new();
                for (key, value) in ordered {
                    out.insert(key.clone(), value);
                }
                Value::Object(out)
            }
            other => other.clone(),
        }
    }
    serde_json::to_string(&sorted(value)).unwrap_or_default()
}

fn value_hash(value: &Value) -> String {
    sha256(&canonical(value))
}

fn digest_of(hashes: &BTreeMap<String, String>) -> String {
    let mut object = Map::new();
    for (path, hash) in hashes {
        object.insert(path.clone(), Value::String(hash.clone()));
    }
    sha256(&canonical(&Value::Object(object)))
}

fn item_to_value(item: &Item, path: &str, invalid: &mut Vec<String>) -> Value {
    match item {
        Item::Str(text) => Value::String(text.clone()),
        Item::Bool(flag) => Value::Bool(*flag),
        Item::Int(number) => Value::Number(Number::from(*number)),
        Item::Float(number) => match Number::from_f64(*number) {
            Some(number) => Value::Number(number),
            None => {
                invalid.push(path.to_string());
                invalid_marker("non-finite float")
            }
        },
        Item::BigInt(_) => {
            invalid.push(path.to_string());
            invalid_marker("big integer")
        }
        Item::Date => {
            invalid.push(path.to_string());
            invalid_marker("date")
        }
        Item::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| item_to_value(item, path, invalid))
                .collect(),
        ),
        Item::Table(table) => {
            let mut object = Map::new();
            for (key, item) in table.entries() {
                object.insert(key.to_string(), item_to_value(item, path, invalid));
            }
            Value::Object(object)
        }
    }
}

fn invalid_marker(kind: &str) -> Value {
    let mut object = Map::new();
    object.insert("$invalid".into(), Value::String(kind.into()));
    Value::Object(object)
}

fn decode_document(bytes: &[u8], name: &str) -> Result<Table> {
    if bytes.len() as u64 > primitives::MAX_FILE_BYTES {
        return invalid(format!(
            "{name} exceeds {} bytes",
            primitives::MAX_FILE_BYTES
        ));
    }
    let Ok(decoded) = std::str::from_utf8(bytes) else {
        return invalid(format!("{name} is not valid UTF-8"));
    };
    let source = decoded
        .strip_prefix('\u{feff}')
        .unwrap_or(decoded)
        .replace("\r\n", "\n");
    crate::smol::parse(&source).or_else(|position| {
        invalid(format!(
            "{name} is not valid TOML at line {}, column {}",
            position.line, position.column
        ))
    })
}

fn collect_leaves<'a>(prefix: &mut Vec<String>, item: &'a Item, out: &mut Vec<(String, &'a Item)>) {
    match item {
        Item::Table(table) if !table.is_empty() => {
            for (key, member) in table.entries() {
                prefix.push(key.to_string());
                collect_leaves(prefix, member, out);
                prefix.pop();
            }
        }
        _ => out.push((encode_path(prefix), item)),
    }
}

struct Subset {
    values: BTreeMap<String, Value>,
    /// Paths holding a value that cannot be trusted (a date, a big integer, a non-finite float).
    invalid: Vec<String>,
}

fn build_subset(pair: &ConfigPair) -> Result<Subset> {
    match pair.tracking {
        Tracking::Tracked => return invalid(crate::tracked_local_refusal()),
        Tracking::Untracked | Tracking::NoRepository | Tracking::Unknown => {}
    }
    let mut document = decode_document(&pair.main, CONFIG_FILE_NAME)?;
    if let (Tracking::Unknown, Some(local)) = (pair.tracking, &pair.local) {
        // git could not say the local file is untracked: its keys are checked like the tracked file's.
        document = document.merged_with(&decode_document(local, LOCAL_CONFIG_FILE_NAME)?);
    }
    let mut leaves = Vec::new();
    for (key, item) in document.entries() {
        let mut prefix = vec![key.to_string()];
        collect_leaves(&mut prefix, item, &mut leaves);
    }
    let mut values = BTreeMap::new();
    let mut invalid_paths = Vec::new();
    for (path, item) in leaves {
        let checked = match accepted_path_of(&path) {
            Some(accepted) => trust_classes::is_checked(&accepted),
            None => true,
        };
        if checked {
            let value = item_to_value(item, &path, &mut invalid_paths);
            values.insert(path, value);
        }
    }
    Ok(Subset {
        values,
        invalid: invalid_paths,
    })
}

/// The values of `capstan.toml` that need trust, by real path. An error when the files cannot be read as a trustable
/// subset (not UTF-8, not TOML, a tracked local file, a date or big integer in a checked path).
pub fn checked_subset(pair: &ConfigPair) -> Result<BTreeMap<String, Value>> {
    let subset = build_subset(pair)?;
    if let Some(path) = subset.invalid.first() {
        return invalid(format!(
            "{path} holds a date, a big integer or a non-finite float, which cannot be trusted"
        ));
    }
    Ok(subset.values)
}

fn hashes_of(values: &BTreeMap<String, Value>) -> BTreeMap<String, String> {
    values
        .iter()
        .map(|(path, value)| (path.clone(), value_hash(value)))
        .collect()
}

// ---------------------------------------------------------------------------------------------------------------
// Status types

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustChange {
    Added,
    Changed,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyChange {
    pub key: String,
    pub change: TrustChange,
    pub old: Option<String>,
    pub new: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustStatus {
    NotNeeded,
    Trusted,
    Needed {
        digest: String,
        changes: Vec<KeyChange>,
        invalid_record: bool,
        store_dir: PathBuf,
    },
    Unavailable {
        reason: String,
        store_dir: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustWritten {
    pub record_key_dir: PathBuf,
    pub linked_worktree: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustError {
    DigestChanged,
    Unavailable(String),
    Io(String),
}

impl fmt::Display for TrustError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TrustError::DigestChanged => {
                f.write_str("capstan.toml changed since the digest was shown")
            }
            TrustError::Unavailable(reason) => f.write_str(reason),
            TrustError::Io(error) => write!(f, "the trust record could not be written: {error}"),
        }
    }
}

impl std::error::Error for TrustError {}

// ---------------------------------------------------------------------------------------------------------------
// The store

#[derive(Debug, Clone, PartialEq, Eq)]
enum StoreKind {
    Dir(PathBuf),
    Unavailable {
        reason: String,
        dir: Option<PathBuf>,
    },
}

/// Where trust records live. Reads never create anything; only a write creates the directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustStore(StoreKind);

fn passwd_home(uid: libc::uid_t) -> Option<PathBuf> {
    let mut buffer = vec![0u8; 1024];
    loop {
        // SAFETY: an all-zero passwd is a valid out-parameter for getpwuid_r.
        let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: the buffer, its length and both out-pointers are valid for the call.
        let code = unsafe {
            libc::getpwuid_r(
                uid,
                &mut entry,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if code == libc::ERANGE && buffer.len() < (1 << 20) {
            buffer.resize(buffer.len() * 2, 0);
            continue;
        }
        if code != 0 || result.is_null() || entry.pw_dir.is_null() {
            return None;
        }
        // SAFETY: pw_dir points at a NUL-terminated string inside `buffer`.
        let home = unsafe { CStr::from_ptr(entry.pw_dir) };
        return Some(PathBuf::from(OsStr::from_bytes(home.to_bytes())));
    }
}

impl TrustStore {
    /// The store of the current user: `CAPSTAN_TRUST_DIR` when set, else the passwd home's `.local/state/capstan/trust`.
    pub fn for_user() -> TrustStore {
        TrustStore::for_user_dir(std::env::var_os("CAPSTAN_TRUST_DIR"))
    }

    /// `for_user` with the `CAPSTAN_TRUST_DIR` value given and the real passwd lookup.
    pub fn for_user_dir(trust_dir: Option<OsString>) -> TrustStore {
        TrustStore::for_user_with(passwd_home, trust_dir)
    }

    /// `for_user` with the passwd lookup and the `CAPSTAN_TRUST_DIR` value given. `HOME` and `XDG_*` are never read.
    pub fn for_user_with(
        lookup: impl Fn(libc::uid_t) -> Option<PathBuf>,
        trust_dir: Option<OsString>,
    ) -> TrustStore {
        if let Some(dir) = trust_dir {
            let dir = PathBuf::from(dir);
            if dir.is_absolute() {
                return TrustStore(StoreKind::Dir(dir));
            }
            return TrustStore(StoreKind::Unavailable {
                reason: format!(
                    "CAPSTAN_TRUST_DIR {} is not an absolute path; set it to one, or unset it",
                    dir.display()
                ),
                dir: Some(dir),
            });
        }
        // SAFETY: getuid has no preconditions and cannot fail.
        let uid = unsafe { libc::getuid() };
        match lookup(uid) {
            Some(home) if home.is_absolute() => TrustStore(StoreKind::Dir(
                home.join(".local/state/capstan/trust"),
            )),
            Some(home) => TrustStore(StoreKind::Unavailable {
                reason: format!(
                    "the home directory {} of this user is not an absolute path; set CAPSTAN_TRUST_DIR to an absolute directory",
                    home.display()
                ),
                dir: None,
            }),
            None => TrustStore(StoreKind::Unavailable {
                reason: "this user has no home directory; set CAPSTAN_TRUST_DIR to an absolute directory"
                    .to_string(),
                dir: None,
            }),
        }
    }

    /// A store in `dir` (tests).
    pub fn at(dir: impl Into<PathBuf>) -> TrustStore {
        TrustStore(StoreKind::Dir(dir.into()))
    }

    /// A store that is always unavailable and touches no directory.
    pub fn unset() -> TrustStore {
        TrustStore(StoreKind::Unavailable {
            reason: "no trust store set".to_string(),
            dir: None,
        })
    }

    fn dir(&self) -> Option<&Path> {
        match &self.0 {
            StoreKind::Dir(dir) => Some(dir),
            StoreKind::Unavailable { dir, .. } => dir.as_deref(),
        }
    }

    fn usable_dir(&self) -> std::result::Result<&Path, String> {
        match &self.0 {
            StoreKind::Dir(dir) => Ok(dir),
            StoreKind::Unavailable { reason, .. } => Err(reason.clone()),
        }
    }
}

fn my_uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

/// Checks an existing store directory; `Ok(false)` when it does not exist.
fn check_store_dir(dir: &Path) -> std::result::Result<bool, String> {
    let metadata = match fs::symlink_metadata(dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!(
                "the trust store {} cannot be read ({error}); {STORE_FIX}",
                dir.display()
            ))
        }
    };
    let name = dir.display();
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "the trust store {name} is a symbolic link; replace it with a directory, or set CAPSTAN_TRUST_DIR to another directory"
        ));
    }
    if !metadata.is_dir() {
        return Err(format!(
            "the trust store {name} is not a directory; remove it, or set CAPSTAN_TRUST_DIR to another directory"
        ));
    }
    if metadata.uid() != my_uid() {
        return Err(format!(
            "the trust store {name} is not owned by the current user; {STORE_FIX}"
        ));
    }
    if metadata.mode() & 0o777 != 0o700 {
        return Err(format!(
            "the trust store {name} has mode {:o}, not 700; {STORE_FIX}",
            metadata.mode() & 0o777
        ));
    }
    Ok(true)
}

// ---------------------------------------------------------------------------------------------------------------
// The record key: which folder's record applies

/// The folder whose record applies to `dir`, and whether that is the main checkout of a linked worktree.
fn record_key(dir: &Path) -> std::result::Result<(PathBuf, bool), String> {
    let real = fs::canonicalize(dir)
        .map_err(|error| format!("{} cannot be resolved ({error})", dir.display()))?;
    let dot_git = real.join(".git");
    let metadata = match fs::symlink_metadata(&dot_git) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((real, false)),
        Err(error) => return Err(format!("{} cannot be read ({error})", dot_git.display())),
    };
    if !metadata.is_file() {
        return Ok((real, false));
    }
    let malformed = || format!("{} is not a valid git link file", dot_git.display());
    let text = fs::read(&dot_git)
        .map_err(|error| format!("{} cannot be read ({error})", dot_git.display()))?;
    let text = String::from_utf8(text).map_err(|_| malformed())?;
    let line = text.lines().next().ok_or_else(malformed)?;
    let target = line.strip_prefix("gitdir:").ok_or_else(malformed)?.trim();
    if target.is_empty() {
        return Err(malformed());
    }
    let git_dir = real.join(target);
    let git_dir = fs::canonicalize(&git_dir).map_err(|error| {
        format!(
            "the git directory {} named by {} cannot be resolved ({error})",
            git_dir.display(),
            dot_git.display()
        )
    })?;
    let common_file = git_dir.join("commondir");
    let common_text = match fs::read(&common_file) {
        Ok(bytes) => bytes,
        // A submodule or a separate git directory: not a linked worktree.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((real, false)),
        Err(error) => {
            return Err(format!(
                "{} cannot be read ({error})",
                common_file.display()
            ))
        }
    };
    let common_text = String::from_utf8(common_text)
        .map_err(|_| format!("{} is not valid text", common_file.display()))?;
    let common = common_text.trim();
    if common.is_empty() {
        return Err(format!("{} is empty", common_file.display()));
    }
    let common = fs::canonicalize(git_dir.join(common)).map_err(|error| {
        format!(
            "the common git directory named by {} cannot be resolved ({error})",
            common_file.display()
        )
    })?;
    if common.file_name() == Some(OsStr::new(".git")) {
        if let Some(parent) = common.parent() {
            let parent = parent.to_path_buf();
            let linked = parent != real;
            return Ok((parent, linked));
        }
    }
    Ok((real, false))
}

// ---------------------------------------------------------------------------------------------------------------
// The record

struct Record {
    digest: String,
    keys: BTreeMap<String, (String, Value)>,
}

enum RecordState {
    Absent,
    Invalid,
    Valid(Record),
}

fn record_path(store_dir: &Path, key_dir: &Path) -> PathBuf {
    let name = sha256_bytes(key_dir.as_os_str().as_bytes());
    store_dir.join(format!("{name}.json"))
}

/// The SHA-256 of the raw bytes, as lower-case hex (a path is not always UTF-8, and a lossy decode would merge paths).
fn sha256_bytes(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn read_record(store_dir: &Path, key_dir: &Path) -> RecordState {
    let path = record_path(store_dir, key_dir);
    let mut file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) if error.raw_os_error() == Some(libc::ENOENT) => return RecordState::Absent,
        Err(_) => return RecordState::Invalid,
    };
    let Ok(metadata) = file.metadata() else {
        return RecordState::Invalid;
    };
    if !metadata.is_file()
        || metadata.uid() != my_uid()
        || metadata.mode() & 0o777 != 0o600
        || metadata.len() > MAX_RECORD_BYTES
    {
        return RecordState::Invalid;
    }
    let mut bytes = Vec::new();
    if (&mut file)
        .take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return RecordState::Invalid;
    }
    match parse_record(&bytes, key_dir) {
        Some(record) => RecordState::Valid(record),
        None => RecordState::Invalid,
    }
}

fn parse_record(bytes: &[u8], key_dir: &Path) -> Option<Record> {
    let document: Value = serde_json::from_slice(bytes).ok()?;
    let object = document.as_object()?;
    if object.get("version")?.as_u64()? != RECORD_VERSION {
        return None;
    }
    if object.get("root")?.as_str()? != key_dir.to_string_lossy() {
        return None;
    }
    object.get("trustedAt")?.as_str()?;
    let digest = object.get("digest")?.as_str()?.to_string();
    let mut keys = BTreeMap::new();
    let mut hashes = BTreeMap::new();
    for (path, entry) in object.get("keys")?.as_object()? {
        let entry = entry.as_object()?;
        let hash = entry.get("hash")?.as_str()?.to_string();
        let value = entry.get("value")?.clone();
        if value_hash(&value) != hash {
            return None;
        }
        hashes.insert(path.clone(), hash.clone());
        keys.insert(path.clone(), (hash, value));
    }
    if digest_of(&hashes) != digest {
        return None;
    }
    Some(Record { digest, keys })
}

fn format_time(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    // Civil date from a day count (days since 1970-01-01).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

fn changes_between(
    old: &BTreeMap<String, (String, Value)>,
    new: &BTreeMap<String, Value>,
    new_hashes: &BTreeMap<String, String>,
) -> Vec<KeyChange> {
    let mut changes = Vec::new();
    for (key, value) in new {
        match old.get(key) {
            None => changes.push(KeyChange {
                key: key.clone(),
                change: TrustChange::Added,
                old: None,
                new: Some(trust_display::display_value(value)),
            }),
            Some((hash, previous)) if Some(hash) != new_hashes.get(key) => {
                changes.push(KeyChange {
                    key: key.clone(),
                    change: TrustChange::Changed,
                    old: Some(trust_display::display_value(previous)),
                    new: Some(trust_display::display_value(value)),
                })
            }
            Some(_) => {}
        }
    }
    for (key, (_, previous)) in old {
        if !new.contains_key(key) {
            changes.push(KeyChange {
                key: key.clone(),
                change: TrustChange::Removed,
                old: Some(trust_display::display_value(previous)),
                new: None,
            });
        }
    }
    changes.sort_by(|a, b| a.key.cmp(&b.key));
    changes
}

// ---------------------------------------------------------------------------------------------------------------
// Status

/// `trust_status_with` for the files already read.
pub fn trust_status_of_bytes(dir: &Path, pair: &ConfigPair, store: &TrustStore) -> TrustStatus {
    let unavailable = |reason: String| TrustStatus::Unavailable {
        reason,
        store_dir: store.dir().map(Path::to_path_buf),
    };
    let subset = match build_subset(pair) {
        Ok(subset) => subset,
        Err(error) => return unavailable(error.to_string()),
    };
    if subset.values.is_empty() {
        return TrustStatus::NotNeeded;
    }
    let store_dir = match store.usable_dir() {
        Ok(dir) => dir.to_path_buf(),
        Err(reason) => return unavailable(reason),
    };
    let exists = match check_store_dir(&store_dir) {
        Ok(exists) => exists,
        Err(reason) => return unavailable(reason),
    };
    let (key_dir, _) = match record_key(dir) {
        Ok(key) => key,
        Err(reason) => return unavailable(reason),
    };
    let hashes = hashes_of(&subset.values);
    let digest = digest_of(&hashes);
    let (record, invalid_record) = if exists {
        match read_record(&store_dir, &key_dir) {
            RecordState::Absent => (None, false),
            RecordState::Invalid => (None, true),
            RecordState::Valid(record) => (Some(record), false),
        }
    } else {
        (None, false)
    };
    if subset.invalid.is_empty() {
        if let Some(record) = &record {
            if record.digest == digest {
                return TrustStatus::Trusted;
            }
        }
    }
    let empty = BTreeMap::new();
    let old = record.as_ref().map_or(&empty, |record| &record.keys);
    let mut changes = changes_between(old, &subset.values, &hashes);
    for path in &subset.invalid {
        if !changes.iter().any(|change| &change.key == path) {
            changes.push(KeyChange {
                key: path.clone(),
                change: TrustChange::Added,
                old: None,
                new: Some(trust_display::display_value(&invalid_marker("invalid"))),
            });
        }
    }
    TrustStatus::Needed {
        digest,
        changes,
        invalid_record,
        store_dir,
    }
}

/// The trust status of the folder `dir` against `store`. Never `Trusted` on an error.
pub fn trust_status_with(dir: &Path, store: &TrustStore) -> TrustStatus {
    match read_config_pair(dir) {
        Ok(Some(pair)) => trust_status_of_bytes(dir, &pair, store),
        Ok(None) => TrustStatus::NotNeeded,
        Err(error) => TrustStatus::Unavailable {
            reason: error.to_string(),
            store_dir: store.dir().map(Path::to_path_buf),
        },
    }
}

/// `trust_status_with` against the current user's store.
pub fn trust_status(dir: &Path) -> TrustStatus {
    trust_status_with(dir, &TrustStore::for_user())
}

// ---------------------------------------------------------------------------------------------------------------
// Recording trust

fn ensure_store_dir(dir: &Path) -> std::result::Result<(), TrustError> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    match builder.create(dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(TrustError::Io(format!("{}: {error}", dir.display()))),
    }
    match check_store_dir(dir) {
        Ok(true) => Ok(()),
        Ok(false) => Err(TrustError::Io(format!("{} was not created", dir.display()))),
        Err(reason) => Err(TrustError::Unavailable(reason)),
    }
}

fn write_record(
    store_dir: &Path,
    key_dir: &Path,
    subset: &BTreeMap<String, Value>,
    hashes: &BTreeMap<String, String>,
    digest: &str,
) -> std::result::Result<(), TrustError> {
    let io = |error: std::io::Error| TrustError::Io(error.to_string());
    let mut keys = Map::new();
    for (path, value) in subset {
        let mut entry = Map::new();
        entry.insert("hash".into(), Value::String(hashes[path].clone()));
        entry.insert("value".into(), value.clone());
        keys.insert(path.clone(), Value::Object(entry));
    }
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let mut document = Map::new();
    document.insert("version".into(), Value::Number(RECORD_VERSION.into()));
    document.insert(
        "root".into(),
        Value::String(key_dir.to_string_lossy().into_owned()),
    );
    document.insert("digest".into(), Value::String(digest.to_string()));
    document.insert("keys".into(), Value::Object(keys));
    document.insert("trustedAt".into(), Value::String(format_time(seconds)));
    let text = serde_json::to_string_pretty(&Value::Object(document))
        .map_err(|error| TrustError::Io(error.to_string()))?;

    let target = record_path(store_dir, key_dir);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos());
    let temp = store_dir.join(format!(".trust-{}-{nanos}.tmp", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temp)
        .map_err(io)?;
    let written = file
        .set_permissions(fs::Permissions::from_mode(0o600))
        .and_then(|()| file.write_all(text.as_bytes()))
        .and_then(|()| file.sync_all())
        .and_then(|()| fs::rename(&temp, &target));
    if let Err(error) = written {
        let _ = fs::remove_file(&temp);
        return Err(io(error));
    }
    Ok(())
}

/// Records the checked values of `dir` as trusted when they still hash to `expected_digest`. A record that already
/// matches is left as it is.
pub fn record_trust(
    dir: &Path,
    expected_digest: &str,
    store: &TrustStore,
) -> std::result::Result<TrustWritten, TrustError> {
    let pair = match read_config_pair(dir) {
        Ok(Some(pair)) => pair,
        Ok(None) => {
            return Err(TrustError::Unavailable(format!(
                "{CONFIG_FILE_NAME} does not exist in {}",
                dir.display()
            )))
        }
        Err(error) => return Err(TrustError::Unavailable(error.to_string())),
    };
    let subset =
        checked_subset(&pair).map_err(|error| TrustError::Unavailable(error.to_string()))?;
    let hashes = hashes_of(&subset);
    let digest = digest_of(&hashes);
    if digest != expected_digest {
        return Err(TrustError::DigestChanged);
    }
    let store_dir = store.usable_dir().map_err(TrustError::Unavailable)?;
    let (key_dir, linked) = record_key(dir).map_err(TrustError::Unavailable)?;
    let written = TrustWritten {
        record_key_dir: key_dir.clone(),
        linked_worktree: linked,
    };
    if subset.is_empty() {
        return Ok(written);
    }
    match check_store_dir(store_dir) {
        Ok(true) => {
            if let RecordState::Valid(record) = read_record(store_dir, &key_dir) {
                if record.digest == digest {
                    return Ok(written);
                }
            }
        }
        Ok(false) => {}
        Err(reason) => return Err(TrustError::Unavailable(reason)),
    }
    ensure_store_dir(store_dir)?;
    write_record(store_dir, &key_dir, &subset, &hashes, &digest)?;
    Ok(written)
}

/// Records the current checked values of `dir` as trusted (harness fixtures only in this plan).
pub fn record_current(
    dir: &Path,
    store: &TrustStore,
) -> std::result::Result<TrustWritten, TrustError> {
    match trust_status_with(dir, store) {
        TrustStatus::Needed { digest, .. } => record_trust(dir, &digest, store),
        TrustStatus::Trusted | TrustStatus::NotNeeded => {
            let (key_dir, linked) = record_key(dir).map_err(TrustError::Unavailable)?;
            Ok(TrustWritten {
                record_key_dir: key_dir,
                linked_worktree: linked,
            })
        }
        TrustStatus::Unavailable { reason, .. } => Err(TrustError::Unavailable(reason)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn for_user_dir_takes_an_absolute_dir_and_refuses_a_relative_one() {
        assert_eq!(
            TrustStore::for_user_dir(Some(OsString::from("/abs/trust"))),
            TrustStore::at("/abs/trust")
        );
        assert!(matches!(
            TrustStore::for_user_dir(Some(OsString::from("rel/trust"))).0,
            StoreKind::Unavailable { .. }
        ));
        let none = TrustStore::for_user_with(|_| Some(PathBuf::from("/home/x")), None);
        assert_eq!(none, TrustStore::at("/home/x/.local/state/capstan/trust"));
    }

    #[test]
    fn non_utf8_checkout_paths_get_different_records() {
        let one = Path::new(OsStr::from_bytes(b"/work/\xff"));
        let two = Path::new(OsStr::from_bytes(b"/work/\xfe"));
        let store = Path::new("/store");
        assert_ne!(record_path(store, one), record_path(store, two));
        assert_eq!(
            sha256_bytes(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn placeholders_match_one_segment() {
        assert_eq!(
            accepted_path_of("roles.foo.model").as_deref(),
            Some("roles.<name>.model")
        );
        assert_eq!(
            accepted_path_of("cache.max_size_gb").as_deref(),
            Some("cache.max_size_gb")
        );
        assert_eq!(accepted_path_of("nonsense.key"), None);
    }

    #[test]
    fn dotted_and_quoted_names_are_one_segment() {
        assert_eq!(
            accepted_path_of("hosts.\"my.host\".command").as_deref(),
            Some("hosts.<name>.command")
        );
        assert_eq!(
            accepted_path_of("roles.\"a \\\"b\\\".c\".model").as_deref(),
            Some("roles.<name>.model")
        );
        assert_eq!(
            split_path("a.\"b.c\".d"),
            vec!["a".to_string(), "b.c".to_string(), "d".to_string()]
        );
    }

    #[test]
    fn a_value_below_an_accepted_path_belongs_to_it() {
        assert_eq!(
            accepted_path_of("roles.x.hooks.PreToolUse").as_deref(),
            Some("roles.<name>.hooks")
        );
    }

    #[test]
    fn time_formats_as_utc() {
        assert_eq!(format_time(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_time(1_700_000_000), "2023-11-14T22:13:20Z");
    }
}
