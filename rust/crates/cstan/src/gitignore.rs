//! The managed block of `.gitignore`: the paths Capstan keeps out of version control, and the check that they are ignored.
use std::io;
use std::path::Path;

use crate::git::{git, Env};

pub const BLOCK_BEGIN: &str = "# >>> capstan (managed by cstan init)";
pub const BLOCK_END: &str = "# <<< capstan";

/// What `write` did to `.gitignore`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Created,
    Added,
    Updated,
    Unchanged,
}

/// A Capstan path that git does not ignore.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unignored {
    CapstanDir,
    CapstanToml,
}

impl Unignored {
    pub fn warning(self) -> &'static str {
        match self {
            Unignored::CapstanDir => "warning: .capstan/ is not ignored by git; run cstan init --gitignore to add the capstan block to .gitignore",
            Unignored::CapstanToml => "warning: capstan.toml is not ignored by git; run cstan init --gitignore to ignore it, or commit it to track it (cstan init --gitignore --track-config keeps it out of the block)",
        }
    }
}

pub fn entries(track_config: bool) -> Vec<&'static str> {
    if track_config {
        vec![".capstan/"]
    } else {
        vec![".capstan/", "capstan.toml", "capstan.toml~"]
    }
}

fn content(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line).trim_end()
}

/// Puts the managed block into `existing`: appended when absent, its body replaced in place when present. Text outside the
/// block is untouched. A file with several blocks or an unterminated one is refused.
pub fn upsert(existing: &str, entries: &[&str]) -> Result<String, String> {
    let lines: Vec<&str> = existing.split_inclusive('\n').collect();
    let begins: Vec<usize> = (0..lines.len())
        .filter(|&i| content(lines[i]) == BLOCK_BEGIN)
        .collect();
    match begins.as_slice() {
        [] => {
            let mut out = existing.to_string();
            if !out.is_empty() {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push('\n');
            }
            out.push_str(BLOCK_BEGIN);
            out.push('\n');
            for entry in entries {
                out.push_str(entry);
                out.push('\n');
            }
            out.push_str(BLOCK_END);
            out.push('\n');
            Ok(out)
        }
        [begin] => {
            let Some(end) = (*begin + 1..lines.len()).find(|&i| content(lines[i]) == BLOCK_END)
            else {
                return Err(format!(
                    ".gitignore has \"{BLOCK_BEGIN}\" without \"{BLOCK_END}\"; fix it by hand"
                ));
            };
            let eol = if lines[*begin].ends_with("\r\n") {
                "\r\n"
            } else {
                "\n"
            };
            let mut out: String = lines[..=*begin].concat();
            for entry in entries {
                out.push_str(entry);
                out.push_str(eol);
            }
            out.push_str(&lines[end..].concat());
            Ok(out)
        }
        _ => Err(format!(
            ".gitignore has more than one \"{BLOCK_BEGIN}\" block; fix it by hand"
        )),
    }
}

/// Writes the managed block to `root/.gitignore`, only when the text changes.
pub fn write(root: &Path, track_config: bool) -> io::Result<Outcome> {
    let path = root.join(".gitignore");
    let existing = match std::fs::read(&path) {
        Ok(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let new = upsert(existing.as_deref().unwrap_or(""), &entries(track_config))
        .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?;
    let Some(old) = existing else {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(&path, &new)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))?;
        return Ok(Outcome::Created);
    };
    if new == old {
        return Ok(Outcome::Unchanged);
    }
    let had_block = old.lines().any(|line| content(line) == BLOCK_BEGIN);
    std::fs::write(&path, &new)?;
    Ok(if had_block {
        Outcome::Updated
    } else {
        Outcome::Added
    })
}

/// The Capstan paths in `root` that git would not ignore. Empty outside a git work tree, when git is missing, or when git
/// fails: an error is never a warning.
pub fn unignored(env: Env, root: &Path) -> Vec<Unignored> {
    let inside = git(env, root, &["rev-parse", "--is-inside-work-tree"]);
    if inside.error || inside.status != Some(0) || inside.stdout.trim() != "true" {
        return Vec::new();
    }
    let not_ignored =
        |path: &str| git(env, root, &["check-ignore", "-q", "--", path]).status == Some(1);
    let mut found = Vec::new();
    if not_ignored(".capstan/") {
        found.push(Unignored::CapstanDir);
    }
    if root.join("capstan.toml").exists()
        && not_ignored("capstan.toml")
        && git(
            env,
            root,
            &["ls-files", "--error-unmatch", "--", "capstan.toml"],
        )
        .status
            != Some(0)
    {
        found.push(Unignored::CapstanToml);
    }
    found
}
