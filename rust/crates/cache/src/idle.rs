use std::path::Path;
use std::process::{Command, Stdio};

use crate::error::Result;

/// False when any linked worktree (not the main checkout) exists on disk: building agents and running setup or
/// teardown hooks all work inside one. A git failure counts as not idle.
pub fn is_idle(project_root: &Path) -> Result<bool> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["worktree", "list", "--porcelain"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let Ok(output) = output else { return Ok(false) };
    if !output.status.success() {
        return Ok(false);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let linked = text
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .skip(1)
        .any(|path| Path::new(path).exists());
    Ok(!linked)
}
