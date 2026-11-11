//! The local knowledge base store (`[kb] storage = local`): `<state dir>/kb/entries.jsonl` is the source of truth, in the
//! same JSON Lines format as the repository file, with `entries.jsonl.prev` (the version before the last write) and
//! `changes.jsonl` (the change log) beside it. Nothing under the project tree is created or changed.
//!
//! Two daemons never share a state directory (the state lock), so one process-wide mutex held for the whole
//! read-modify-write is enough to keep concurrent writers from losing an update.
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use capstan_config::kb::{parse_file, remove, upsert_sorted, validate, KbStorage};
use capstan_config::{KbConfig, KbEntry};
use serde::{Deserialize, Serialize};

use super::{git, local_file, location, source};

static WRITE: Mutex<()> = Mutex::new(());

/// One line of the change log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub at: String,
    pub agent: String,
    pub role: String,
    pub verb: String,
    pub id: String,
    pub title: String,
    pub count: u32,
}

/// One change to the entries file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Write {
    Add {
        entry: KbEntry,
        force: bool,
    },
    /// `expect` is the sha (see `line_sha`) of the entry line the caller read; a different current line is refused.
    Update {
        entry: KbEntry,
        expect: Option<String>,
    },
    Remove {
        id: String,
        expect: Option<String>,
    },
}

/// The sha256 hex of an entry's line, which `kb get` returns and `update` and `remove` send back as `--expect`.
pub fn line_sha(entry: &KbEntry) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(capstan_config::kb::to_line(entry).as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn check_expect(entry: &KbEntry, expect: &Option<String>) -> Result<(), String> {
    match expect {
        Some(sha) if *sha != line_sha(entry) => Err(format!(
            "kb: {} changed since it was read; run the command again.",
            entry.id
        )),
        _ => Ok(()),
    }
}

/// What an import did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportResult {
    pub added: usize,
    pub replaced: usize,
    pub skipped: usize,
    pub invalid: Vec<(usize, String)>,
}

fn kb_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("kb")
}

fn prev_file(state_dir: &Path) -> PathBuf {
    kb_dir(state_dir).join("entries.jsonl.prev")
}

fn changes_file(state_dir: &Path) -> PathBuf {
    kb_dir(state_dir).join("changes.jsonl")
}

fn io(e: std::io::Error) -> String {
    format!("kb: local store: {e}")
}

/// Writes `text` to `path` through a temp file in the same directory: fsync, then rename; mode 0600.
fn atomic_write(path: &Path, text: &str) -> Result<(), String> {
    let dir = path.parent().ok_or("kb: local store has no directory")?;
    std::fs::create_dir_all(dir).map_err(io)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("kb");
    let tmp = dir.join(format!("{name}.tmp"));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp).map_err(io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(io)?;
    }
    file.write_all(text.as_bytes()).map_err(io)?;
    file.sync_all().map_err(io)?;
    drop(file);
    std::fs::rename(&tmp, path).map_err(io)?;
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Saves `text` as the entries file; the current file becomes `.prev`.
fn save(state_dir: &Path, text: &str) -> Result<(), String> {
    let file = local_file(state_dir);
    if let Ok(current) = std::fs::read(&file) {
        atomic_write(&prev_file(state_dir), &String::from_utf8_lossy(&current))?;
    }
    atomic_write(&file, text)
}

fn iso_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

fn append_change(state_dir: &Path, cfg: &KbConfig, change: &Change) -> Result<(), String> {
    let path = changes_file(state_dir);
    let mut text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("the change log could not be read: {e}")),
    };
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&serde_json::to_string(change).map_err(|e| e.to_string())?);
    text.push('\n');
    let keep = cfg.log_max_entries.max(1) as usize;
    let lines: Vec<&str> = text.lines().collect();
    let text = if lines.len() > keep {
        let mut out = lines[lines.len() - keep..].join("\n");
        out.push('\n');
        out
    } else {
        text
    };
    atomic_write(&path, &text)
}

/// Logs the change; a log failure never undoes the write already saved.
fn log_change(state_dir: &Path, cfg: &KbConfig, change: &Change) {
    if let Err(why) = append_change(state_dir, cfg, change) {
        eprintln!("kb_log_failed: {why}");
    }
}

fn require_local(cfg: &KbConfig) -> Result<(), String> {
    if cfg.storage == KbStorage::Local {
        Ok(())
    } else {
        Err("kb: [kb] storage is repo; the local store is not in use".to_string())
    }
}

fn current(state_dir: &Path, root: &Path, cfg: &KbConfig) -> Result<String, String> {
    Ok(source(state_dir, root, cfg)
        .map_err(|e| e.to_string())?
        .unwrap_or_default())
}

fn commit_known(root: &Path, sha: &str) -> bool {
    let spec = format!("{sha}^{{commit}}");
    sha.bytes().all(|b| b.is_ascii_hexdigit())
        && !sha.is_empty()
        && matches!(git(root, &["cat-file", "-e", &spec]), Ok((0, _)))
}

fn check_commit(root: &Path, entry: &KbEntry) -> Result<(), String> {
    if commit_known(root, &entry.verified_commit) {
        Ok(())
    } else {
        let sha = &entry.verified_commit;
        Err(format!(
            "kb: verified commit {} is not in the project repository",
            &sha[..sha.len().min(12)]
        ))
    }
}

/// Applies one change to the local entries file and logs it. `who` is (agent, role). Err is the refusal text.
pub fn write(
    state_dir: &Path,
    root: &Path,
    cfg: &KbConfig,
    w: Write,
    who: (&str, &str),
) -> Result<Change, String> {
    require_local(cfg)?;
    let _guard = WRITE.lock().unwrap_or_else(|p| p.into_inner());
    let place = location(state_dir, root, cfg);
    let max_bytes = cfg.max_entry_bytes.max(0) as usize;
    let text = current(state_dir, root, cfg)?;
    let parsed = parse_file(&text, max_bytes);
    let find = |id: &str| parsed.entries.iter().find(|e| e.id == id);
    let (verb, entry_id, title, next) = match w {
        Write::Add { entry, force } => {
            validate(&entry, max_bytes)?;
            if find(&entry.id).is_some() {
                return Err(format!(
                    "kb: id {id} exists; use cstan kb update {id}",
                    id = entry.id
                ));
            }
            if !force {
                // Checked against the file as read under the write lock, not the caller's earlier index.
                let fresh = crate::kb::ensure(state_dir, root, cfg).map_err(|e| e.to_string())?;
                let similar = fresh.similar(&entry).map_err(|e| e.to_string())?;
                if !similar.is_empty() {
                    return Err(format!(
                        "kb: similar entries exist: {}; reuse or update one, or add --force",
                        similar.join(", ")
                    ));
                }
            }
            if parsed.entries.len() as i64 >= cfg.max_entries {
                return Err(format!(
                    "kb: {place} already has {} entries, the limit of {}; remove one first",
                    parsed.entries.len(),
                    cfg.max_entries
                ));
            }
            check_commit(root, &entry)?;
            let next = upsert_sorted(&text, &entry);
            ("add", entry.id, entry.title, next)
        }
        Write::Update { entry, expect } => {
            validate(&entry, max_bytes)?;
            let Some(old) = find(&entry.id) else {
                return Err(format!("kb: no entry {} in {place}", entry.id));
            };
            check_expect(old, &expect)?;
            check_commit(root, &entry)?;
            let next = upsert_sorted(&text, &entry);
            ("update", entry.id, entry.title, next)
        }
        Write::Remove { id, expect } => {
            let Some(old) = find(&id) else {
                return Err(format!("kb: no entry {id} in {place}"));
            };
            check_expect(old, &expect)?;
            let title = old.title.clone();
            let next = remove(&text, &id).ok_or_else(|| format!("kb: no entry {id} in {place}"))?;
            ("remove", id, title, next)
        }
    };
    save(state_dir, &next)?;
    let change = Change {
        at: iso_now(),
        agent: who.0.to_string(),
        role: who.1.to_string(),
        verb: verb.to_string(),
        id: entry_id,
        title,
        count: 1,
    };
    log_change(state_dir, cfg, &change);
    Ok(change)
}

/// Imports the entries of `text` (a repository entries file) into the local file. Ids already present are skipped, or
/// overwritten when `replace`. Invalid lines are returned and never written. Refused (nothing written) when the result
/// would exceed `max_entries`.
pub fn import(
    state_dir: &Path,
    root: &Path,
    cfg: &KbConfig,
    text: &str,
    replace: bool,
    who: (&str, &str),
) -> Result<ImportResult, String> {
    require_local(cfg)?;
    let _guard = WRITE.lock().unwrap_or_else(|p| p.into_inner());
    let place = location(state_dir, root, cfg);
    let max_bytes = cfg.max_entry_bytes.max(0) as usize;
    let mut file = current(state_dir, root, cfg)?;
    let have = parse_file(&file, max_bytes);
    let incoming = parse_file(text, max_bytes);
    let mut result = ImportResult {
        added: 0,
        replaced: 0,
        skipped: 0,
        invalid: incoming.invalid.clone(),
    };
    let mut todo: Vec<&KbEntry> = Vec::new();
    for entry in &incoming.entries {
        if have.entries.iter().any(|e| e.id == entry.id) {
            if replace {
                result.replaced += 1;
                todo.push(entry);
            } else {
                result.skipped += 1;
            }
        } else {
            result.added += 1;
            todo.push(entry);
        }
    }
    let total = have.entries.len() + result.added;
    if total as i64 > cfg.max_entries {
        return Err(format!(
            "kb: importing would give {place} {total} entries, over the limit of {}; remove some first",
            cfg.max_entries
        ));
    }
    if todo.is_empty() {
        return Ok(result);
    }
    for entry in todo {
        file = upsert_sorted(&file, entry);
    }
    save(state_dir, &file)?;
    let change = Change {
        at: iso_now(),
        agent: who.0.to_string(),
        role: who.1.to_string(),
        verb: "import".to_string(),
        id: String::new(),
        title: String::new(),
        count: (result.added + result.replaced) as u32,
    };
    log_change(state_dir, cfg, &change);
    Ok(result)
}

/// Up to `limit` change log lines, newest first.
pub fn log(state_dir: &Path, limit: usize) -> Vec<Change> {
    let text = std::fs::read_to_string(changes_file(state_dir)).unwrap_or_default();
    text.lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<Change>(line).ok())
        .take(limit)
        .collect()
}

/// One entry by id from the entries file of either storage.
pub fn entry(state_dir: &Path, root: &Path, cfg: &KbConfig, id: &str) -> Option<KbEntry> {
    let text = source(state_dir, root, cfg).ok()??;
    let max_bytes = cfg.max_entry_bytes.max(0) as usize;
    parse_file(&text, max_bytes)
        .entries
        .into_iter()
        .find(|e| e.id == id)
}
