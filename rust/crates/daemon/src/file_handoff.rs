//! `send --file`: finds a file committed on an agent's branch so a message can name it by reference.
//!
//! Git runs in the project root with a cleared environment, like `ProjectGit::run`; the branches of all worktrees live
//! in that one object store, so any reader can `git show <commit>:<path>`.

use std::path::Path;
use std::process::{Command, Stdio};

/// The longest file path `send --file` accepts, in bytes.
pub const MAX_FILE_PATH_BYTES: usize = 512;
/// The longest summary `send --file` accepts, in bytes.
pub const MAX_SUMMARY_BYTES: usize = 1000;

/// The path rule message of `send --file`.
pub const PATH_RULE: &str =
    "the file path must be relative to the repository root, without .., and at most 512 bytes";

/// A file found on a branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRef {
    pub commit: String,
    pub size: u64,
    /// The worktree has changes to the file that the commit does not have.
    pub uncommitted: bool,
}

/// Whether `path` is acceptable: 1..=512 bytes, relative, no `..` segment, no leading `-` or `:`, no control character.
pub fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_FILE_PATH_BYTES
        && !path.starts_with(['/', '-', ':'])
        && !path.chars().any(char::is_control)
        && !path.split('/').any(|segment| segment == "..")
}

fn git(dir: &Path, args: &[&str]) -> Result<(i32, String), String> {
    let output = Command::new("git")
        .arg("--no-replace-objects")
        .arg("-C")
        .arg(dir)
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
        .output()
        .map_err(|e| format!("git could not be run ({e})"))?;
    let code = output.status.code().ok_or("git could not be run")?;
    Ok((code, String::from_utf8_lossy(&output.stdout).into_owned()))
}

/// Finds `path` at the tip of `branch`: its commit, size and whether the worktree changed it since.
pub fn locate(root: &Path, branch: &str, worktree: &str, path: &str) -> Result<FileRef, String> {
    let rev = format!("refs/heads/{branch}^{{commit}}");
    let (code, out) = git(root, &["rev-parse", "--verify", "--quiet", &rev])?;
    let commit = out.trim().to_string();
    if code != 0 || commit.len() != 40 {
        return Err(format!("the branch {branch} has no commit"));
    }
    let spec = format!("{commit}:{path}");
    let (code, kind) = git(root, &["cat-file", "-t", &spec])?;
    if code != 0 || kind.trim() != "blob" {
        return Err(format!(
            "{path} is not a file at {branch} ({}); commit it on your branch first",
            &commit[..12]
        ));
    }
    let (code, size) = git(root, &["cat-file", "-s", &spec])?;
    let size = size
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|_| code == 0)
        .ok_or("git could not read the size of the file")?;
    // A worktree that is gone or unreadable has nothing to compare: the committed version is what counts.
    let uncommitted = match git(
        Path::new(worktree),
        &["status", "--porcelain", "-z", "--", path],
    ) {
        Ok((0, out)) => !out.is_empty(),
        _ => false,
    };
    Ok(FileRef {
        commit,
        size,
        uncommitted,
    })
}

/// The message body naming the file.
pub fn body(path: &str, branch: &str, file: &FileRef, summary: Option<&str>) -> String {
    let mut text = format!(
        "File handed over: {path}\nBranch {branch} at {commit}, {size} bytes.\nRead it with: git show {target}",
        commit = file.commit,
        target = shell_word(&format!("{}:{path}", file.commit)),
        size = file.size
    );
    if let Some(summary) = summary {
        text.push_str("\nSummary: ");
        text.push_str(summary);
    }
    text
}

/// `word` as one shell word: unchanged when it has only safe characters, else in single quotes with `'` escaped.
fn shell_word(word: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "_./@%+=:,-".contains(c);
    if word.chars().all(safe) {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}
