//! Whether git tracks `capstan.local.toml`: a committed local file would let a repository ship its own machine settings.
use std::path::Path;
use std::process::{Command, Stdio};

use crate::types::LOCAL_CONFIG_FILE_NAME;

/// What git says about the local file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tracking {
    /// The index holds the file.
    Tracked,
    /// A work tree that does not hold the file.
    Untracked,
    /// `cwd` is not inside a git work tree, so nothing can be tracked.
    NoRepository,
    /// git is missing or failed in a way that does not say: callers fail closed.
    Unknown,
}

/// `git -C <cwd> ls-files --error-unmatch -- capstan.local.toml`, run without a shell and with a cleared environment
/// (PATH only) and the project's fsmonitor program switched off, so a variable such as `GIT_DIR` or `GIT_INDEX_FILE` cannot point git at another repository.
pub(crate) fn local_file_tracking(cwd: &Path) -> Tracking {
    local_file_tracking_with(cwd, Path::new("git"))
}

/// `local_file_tracking` with the git program named by the caller (tests pass a stub, not whatever is on `PATH`).
#[doc(hidden)]
pub fn local_file_tracking_with(cwd: &Path, git_program: &Path) -> Tracking {
    let mut command = Command::new(git_program);
    command
        .env_clear()
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(cwd)
        // The project's own `core.fsmonitor` program must not run from a config read.
        .args(["-c", "core.fsmonitor=false"])
        .args(["ls-files", "--error-unmatch", "--", LOCAL_CONFIG_FILE_NAME])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    let Ok(output) = command.output() else {
        return Tracking::Unknown;
    };
    match output.status.code() {
        Some(0) => Tracking::Tracked,
        Some(1) => Tracking::Untracked,
        Some(128) if String::from_utf8_lossy(&output.stderr).contains("not a git repository") => {
            Tracking::NoRepository
        }
        _ => Tracking::Unknown,
    }
}
