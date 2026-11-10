//! The controller's knowledge base index: a SQLite cache (FTS5 search, path lookup, links, staleness) built from the
//! entries file as committed at the project root's HEAD. The working tree is never read. Deleting the index only costs a
//! rebuild. `text` holds the lines and briefs the controller shows, `stale` the git checks and the warnings.
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use capstan_config::KbConfig;

mod index;
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

/// Opens the index in `<state_dir>/kb`, rebuilding it when HEAD or the entries file's blob changed since the last build.
pub fn ensure(state_dir: &Path, root: &Path, cfg: &KbConfig) -> Result<KbIndex, KbError> {
    if !cfg.enabled {
        return Err(KbError::NotConfigured);
    }
    let head = match git(root, &["rev-parse", "--verify", "HEAD"]).map_err(KbError::Git)? {
        (0, out) => out.trim().to_string(),
        _ => return Err(KbError::Git("the project has no commit".to_string())),
    };
    let spec = format!("HEAD:{}", cfg.path);
    let blob = match git(root, &["rev-parse", "--verify", &spec]).map_err(KbError::Git)? {
        (0, out) => out.trim().to_string(),
        _ => String::new(),
    };
    let _guard = BUILD.lock().unwrap_or_else(|p| p.into_inner());
    let mut index = KbIndex::open(state_dir, root)?;
    if index.meta("head").as_deref() == Some(head.as_str())
        && index.meta("blob").as_deref() == Some(blob.as_str())
    {
        return Ok(index);
    }
    let file = if blob.is_empty() {
        None
    } else {
        match git(root, &["show", &spec]).map_err(KbError::Git)? {
            (0, out) => Some(out),
            _ => {
                return Err(KbError::Git(
                    "the entries file could not be read".to_string(),
                ))
            }
        }
    };
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
