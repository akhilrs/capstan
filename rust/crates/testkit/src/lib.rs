//! Test-only helpers shared by the suites.
//!
//! A test that starts a daemon (or anything that outlives a command) in a scratch directory must not leave it running when the
//! test ends, however it ends. [`Reaper`] is the one guard for that: its `Drop` kills every process whose working directory is
//! inside the scratch directory, so it runs on success, on an assertion failure and on a panic, and it does not depend on the
//! directory's deletion (a daemon whose directory is gone keeps running). A run killed from outside (a timeout's SIGKILL) runs no
//! `Drop`; `scripts/daemon-survivors.sh` reports what such a run leaves.
use std::path::{Path, PathBuf};

/// The longest temp directory a suite may build scratch projects under: a project's control socket
/// (`<root>/<scratch name>/proj/.capstan/state/control.sock`) must stay within the 103-byte socket path limit of macOS.
const SHORT_ROOT_BYTES: usize = 20;

/// A canonical base directory for scratch projects: the system temp directory when its path is short, else `/tmp`
/// (macOS gives each user a long one under `/var/folders`, too long for a socket path inside it).
pub fn short_temp_root() -> PathBuf {
    let temp = std::env::temp_dir();
    let temp = std::fs::canonicalize(&temp).unwrap_or(temp);
    if temp.as_os_str().len() <= SHORT_ROOT_BYTES {
        return temp;
    }
    std::fs::canonicalize("/tmp").unwrap_or_else(|_| PathBuf::from("/tmp"))
}

/// Whether `pid` exists and has not ended (a zombie waiting for its parent has ended).
pub fn is_running(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the pid exists.
    let exists = unsafe { libc::kill(pid, 0) } == 0
        || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
    exists && !capstan_process::is_zombie(i64::from(pid))
}

/// Kills (SIGKILL) every process other than this one whose working directory is `dir` or inside it, and returns how many.
/// Nothing outside `dir` is ever signalled.
pub fn kill_processes_in(dir: &Path) -> usize {
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let me = std::process::id() as i32;
    let mut killed = 0;
    for pid in capstan_process::pids() {
        if pid == me {
            continue;
        }
        if capstan_process::cwd(pid).is_some_and(|cwd| cwd.starts_with(&dir)) {
            // SAFETY: the process runs inside a scratch directory this test created, so this test started it.
            unsafe { libc::kill(pid, libc::SIGKILL) };
            killed += 1;
        }
    }
    killed
}

/// Kills what runs inside a scratch directory when dropped (or when [`Reaper::reap`] is called).
pub struct Reaper(PathBuf);

impl Reaper {
    pub fn new(dir: impl Into<PathBuf>) -> Reaper {
        Reaper(dir.into())
    }

    pub fn reap(&self) -> usize {
        kill_processes_in(&self.0)
    }
}

impl Drop for Reaper {
    fn drop(&mut self) {
        self.reap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    #[test]
    fn a_process_inside_the_directory_is_killed_and_one_outside_is_not() {
        let dir = std::env::temp_dir().join(format!("capstan-testkit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let spawn = |cwd: &Path| {
            Command::new("sleep")
                .arg("60")
                .current_dir(cwd)
                .stdin(Stdio::null())
                .spawn()
                .unwrap()
        };
        let mut inside = spawn(&dir);
        let mut outside = spawn(&std::env::temp_dir());
        {
            let _guard = Reaper::new(&dir);
        }
        assert!(!inside.wait().unwrap().success());
        assert!(outside.try_wait().unwrap().is_none());
        outside.kill().unwrap();
        outside.wait().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
