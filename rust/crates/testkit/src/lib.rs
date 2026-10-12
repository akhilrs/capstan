//! Test-only helpers shared by the suites.
//!
//! A test that starts a daemon (or anything that outlives a command) in a scratch directory must not leave it running when the
//! test ends, however it ends. [`Reaper`] is the one guard for that: its `Drop` kills every process whose working directory is
//! inside the scratch directory, so it runs on success, on an assertion failure and on a panic, and it does not depend on the
//! directory's deletion (a daemon whose directory is gone keeps running). A run killed from outside (a timeout's SIGKILL) runs no
//! `Drop`; `scripts/daemon-survivors.sh` reports what such a run leaves.
use std::path::{Path, PathBuf};

/// Kills (SIGKILL) every process other than this one whose working directory is `dir` or inside it, and returns how many.
/// Linux only (/proc); elsewhere it does nothing. Nothing outside `dir` is ever signalled.
pub fn kill_processes_in(dir: &Path) -> usize {
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return 0;
    };
    let me = std::process::id() as i32;
    let mut killed = 0;
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<i32>().ok())
        else {
            continue;
        };
        if pid == me {
            continue;
        }
        if std::fs::read_link(format!("/proc/{pid}/cwd")).is_ok_and(|cwd| cwd.starts_with(&dir)) {
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
