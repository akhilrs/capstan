//! The controller's knowledge base index: a SQLite cache (FTS5 search, path lookup, links, staleness) built from the
//! entries file as committed at the project root's HEAD (storage = repo; the working tree is never read) or from the
//! local entries file in the state directory (storage = local). Deleting the index only costs a
//! rebuild. `text` holds the lines and briefs the controller shows, `stale` the git checks and the warnings.
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use capstan_config::KbConfig;

mod index;
pub mod local;
mod stale;
mod text;

pub use index::{Hit, KbIndex, Status};
pub use stale::warnings;
pub use text::{brief, owned_paths, reuse_lines, BriefKind};

const GIT_LIMIT: Duration = Duration::from_secs(10);

/// Why the index could not be opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KbError {
    NotConfigured,
    Git(String),
    Store(String),
}

impl std::fmt::Display for KbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KbError::NotConfigured => write!(f, "the knowledge base is not configured"),
            KbError::Git(why) => write!(f, "git: {why}"),
            KbError::Store(why) => write!(f, "kb index: {why}"),
        }
    }
}

impl std::error::Error for KbError {}

impl From<rusqlite::Error> for KbError {
    fn from(error: rusqlite::Error) -> Self {
        KbError::Store(error.to_string())
    }
}

/// One git call in `root` with a cleared environment and a 10 second limit: the exit code and standard output.
pub(crate) fn git(root: &Path, args: &[&str]) -> Result<(i32, String), String> {
    let mut child = Command::new("git")
        .arg("--no-replace-objects")
        .arg("-C")
        .arg(root)
        .args(args)
        .env_clear()
        .env(
            "PATH",
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()),
        )
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("git could not be run ({e})"))?;
    let mut stdout = child.stdout.take().ok_or("git has no output")?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < GIT_LIMIT => std::thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err("git took longer than 10 seconds".to_string());
            }
            Err(e) => return Err(format!("git could not be waited for ({e})")),
        }
    };
    let bytes = reader.join().unwrap_or_default();
    let code = status.code().ok_or("git was stopped by a signal")?;
    Ok((code, String::from_utf8_lossy(&bytes).into_owned()))
}

static BUILD: Mutex<()> = Mutex::new(());

/// The local entries file: `<state dir>/kb/entries.jsonl`.
pub(crate) fn local_file(state_dir: &Path) -> std::path::PathBuf {
    state_dir.join("kb").join("entries.jsonl")
}

/// Where the entries live, as the warnings, briefs and refusals name it: `cfg.path` (repo), or the local entries file
/// relative to `root` when it is under it (normally `.capstan/state/kb/entries.jsonl`), else absolute.
pub fn location(state_dir: &Path, root: &Path, cfg: &KbConfig) -> String {
    match cfg.storage {
        capstan_config::kb::KbStorage::Repo => cfg.path.clone(),
        capstan_config::kb::KbStorage::Local => {
            let file = local_file(state_dir);
            match file.strip_prefix(root) {
                Ok(rel) => rel.to_string_lossy().into_owned(),
                Err(_) => file.to_string_lossy().into_owned(),
            }
        }
    }
}

/// The entries file text: `git show HEAD:<path>` (repo; None when absent) or the local file (local; None when absent).
pub fn source(state_dir: &Path, root: &Path, cfg: &KbConfig) -> Result<Option<String>, KbError> {
    match cfg.storage {
        capstan_config::kb::KbStorage::Repo => {
            let spec = format!("HEAD:{}", cfg.path);
            match git(root, &["rev-parse", "--verify", &spec]).map_err(KbError::Git)? {
                (0, _) => {}
                _ => return Ok(None),
            }
            match git(root, &["show", &spec]).map_err(KbError::Git)? {
                (0, out) => Ok(Some(out)),
                _ => Err(KbError::Git(
                    "the entries file could not be read".to_string(),
                )),
            }
        }
        capstan_config::kb::KbStorage::Local => read_local(state_dir),
    }
}

fn read_local(state_dir: &Path) -> Result<Option<String>, KbError> {
    match std::fs::read(local_file(state_dir)) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            // A file with no entry and no invalid line counts as missing, the same as an absent file.
            let parsed = capstan_config::kb::parse_file(&text, usize::MAX);
            if parsed.entries.is_empty() && parsed.invalid.is_empty() {
                Ok(None)
            } else {
                Ok(Some(text))
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(KbError::Store(format!("the entries file: {e}"))),
    }
}

/// Opens the index in `<state_dir>/kb`, rebuilding it when HEAD or the entries file changed since the last build (the
/// blob id at HEAD for repo storage, a hash of the local file for local storage).
pub fn ensure(state_dir: &Path, root: &Path, cfg: &KbConfig) -> Result<KbIndex, KbError> {
    if !cfg.enabled {
        return Err(KbError::NotConfigured);
    }
    let head = match git(root, &["rev-parse", "--verify", "HEAD"]).map_err(KbError::Git)? {
        (0, out) => out.trim().to_string(),
        _ => return Err(KbError::Git("the project has no commit".to_string())),
    };
    let local = cfg.storage == capstan_config::kb::KbStorage::Local;
    let mut file: Option<String> = None;
    let blob = if local {
        file = read_local(state_dir)?;
        match &file {
            Some(text) => format!("local:{}", capstan_config::sha256(text)),
            None => String::new(),
        }
    } else {
        let spec = format!("HEAD:{}", cfg.path);
        match git(root, &["rev-parse", "--verify", &spec]).map_err(KbError::Git)? {
            (0, out) => out.trim().to_string(),
            _ => String::new(),
        }
    };
    let _guard = BUILD.lock().unwrap_or_else(|p| p.into_inner());
    let mut index = KbIndex::open(state_dir, root)?;
    if index.meta("head").as_deref() == Some(head.as_str())
        && index.meta("blob").as_deref() == Some(blob.as_str())
    {
        return Ok(index);
    }
    if !local && !blob.is_empty() {
        file = source(state_dir, root, cfg)?;
    }
    let parsed = file
        .as_deref()
        .map(|text| capstan_config::kb::parse_file(text, cfg.max_entry_bytes.max(0) as usize));
    let checks = parsed
        .as_ref()
        .map(|p| stale::check(root, &head, &p.entries))
        .unwrap_or_default();
    index.rebuild(&head, &blob, parsed.as_ref(), &checks)?;
    Ok(index)
}
