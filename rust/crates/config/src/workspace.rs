//! The `[repos]` and `[workspace]` sections: a project whose root holds the Capstan state (`capstan.toml`, `.capstan/`)
//! while the code lives in one or more repositories listed by name. Loading (`resolve_workspace`) reads no git: it checks
//! names, counts, required keys and the cross references and keeps a path that does not exist as it was joined, so
//! `status`, `inbox` and `repair` still run with a broken repository. `validate_workspace` is the git half: `start`,
//! `init --workspace` and the daemon boot call it and refuse what it finds.
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use capstan_wire::js::Value;

use crate::cache::CacheConfig;
use crate::error::{invalid, Result};
use crate::json::{number, object, optional, strings, text};
use crate::primitives::*;
use crate::types::MAX_WORKTREE_SETUP_CHARS;
use crate::value::Item;
use crate::verify::{resolve_env, MAX_VERIFY_COMMANDS, MAX_VERIFY_COMMAND_CHARS};

/// The most repositories a project may list.
pub const MAX_REPOS: usize = 32;
/// The pattern a repository name must match.
pub const REPO_NAME_PATTERN: &str = "^[a-z0-9][a-z0-9_-]{0,31}$";
pub const REPO_KEYS: &[&str] = &["path", "main_branch", "verify", "cache", "setup"];
pub const REPO_VERIFY_KEYS: &[&str] = &["commands", "env", "timeout_seconds"];
pub const WORKSPACE_KEYS: &[&str] = &["merge_order", "worktree_dir", "default_repo"];
const MAX_PATH_CHARS: usize = 1024;
const MAX_BRANCH_CHARS: usize = 200;

/// Workspace mode is built but not finished: while this is `false`, a project with `[repos]` is refused by the daemon
/// boot, `cstan start` and `cstan init --workspace` unless `CAPSTAN_WORKSPACE_PREVIEW=1` is set. The package that
/// completes the mode sets it to `true`.
pub const WORKSPACE_MODE_READY: bool = false;
/// The variable that switches the gate off for a trial of the unfinished mode. Worker panes never get it.
pub const WORKSPACE_PREVIEW_ENV: &str = "CAPSTAN_WORKSPACE_PREVIEW";
pub const WORKSPACE_NOT_COMPLETE: &str = "workspace mode is not complete in this build";
pub const WORKSPACE_ROOT_NOT_REPO: &str = "this build boots the daemon only from a git repository root; a non-repo workspace root needs a later build";

/// The gate: `Err` with the not-complete text while the mode is unfinished and `preview` (the value of
/// `CAPSTAN_WORKSPACE_PREVIEW`) is not `1`. Callers apply it only to a project that has `[repos]`, before any git.
pub fn workspace_gate(preview: Option<&str>) -> std::result::Result<(), String> {
    if WORKSPACE_MODE_READY || preview == Some("1") {
        Ok(())
    } else {
        Err(WORKSPACE_NOT_COMPLETE.to_string())
    }
}

/// `[repos.<name>.verify]`: the gate of one repository. A repository without it uses the top-level `[verify]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoVerify {
    pub commands: Vec<String>,
    pub env: Vec<(String, String)>,
    /// `None`: the top-level `[verify]` value.
    pub timeout_seconds: Option<i64>,
}

/// One `[repos.<name>]` entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoConfig {
    pub name: String,
    /// The repository's top-level folder: canonical when it exists, else the configured path as it was joined.
    pub root: PathBuf,
    pub main_branch: String,
    pub verify: Option<RepoVerify>,
    /// The `[cache]` entry names that apply to the repository; `None`: all.
    pub cache: Option<Vec<String>>,
    /// The repository's worktree setup command; `None`: the `[worktree]` setup.
    pub setup: Option<String>,
}

/// The resolved `[repos]` and `[workspace]` sections; `RoleConfig.workspace` is `None` without a `[repos.<name>]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    /// The project root (canonical when it exists): where `capstan.toml` and `.capstan/` live.
    pub root: PathBuf,
    /// By name.
    pub repos: Vec<RepoConfig>,
    /// Every repository: the listed ones in the listed order, then the rest by name.
    pub merge_order: Vec<String>,
    pub worktree_dir: PathBuf,
    pub default_repo: String,
}

impl Workspace {
    pub fn repo(&self, name: &str) -> Option<&RepoConfig> {
        self.repos.iter().find(|repo| repo.name == name)
    }

    /// The configured main branch of `repo`, the base for its work and the target of its integrations whatever its
    /// checkout holds. An unknown name gives an empty string.
    pub fn main_branch_of(&self, repo: &str) -> String {
        self.repo(repo)
            .map(|repo| repo.main_branch.clone())
            .unwrap_or_default()
    }

    pub(crate) fn repos_value(&self) -> Value {
        Value::Array(self.repos.iter().map(repo_value).collect())
    }

    pub(crate) fn workspace_value(&self) -> Value {
        object(vec![
            ("mergeOrder", strings(&self.merge_order)),
            ("worktreeDir", text(&self.worktree_dir.to_string_lossy())),
            ("defaultRepo", text(&self.default_repo)),
        ])
    }
}

fn repo_value(repo: &RepoConfig) -> Value {
    let verify = repo.verify.as_ref().map_or(Value::Null, |verify| {
        object(vec![
            ("commands", strings(&verify.commands)),
            (
                "env",
                object(
                    verify
                        .env
                        .iter()
                        .map(|(name, value)| (name.as_str(), text(value)))
                        .collect(),
                ),
            ),
            (
                "timeoutSeconds",
                verify.timeout_seconds.map_or(Value::Null, number),
            ),
        ])
    });
    object(vec![
        ("name", text(&repo.name)),
        ("path", text(&repo.root.to_string_lossy())),
        ("mainBranch", text(&repo.main_branch)),
        ("verify", verify),
        (
            "cache",
            repo.cache
                .as_ref()
                .map_or(Value::Null, |names| strings(names)),
        ),
        ("setup", optional(&repo.setup)),
    ])
}

/// Whether `name` matches `REPO_NAME_PATTERN`.
pub fn repo_name_ok(name: &str) -> bool {
    let bytes = name.as_bytes();
    (1..=32).contains(&bytes.len())
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes[1..]
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_' || *b == b'-')
}

/// A branch name git would accept, kept strict: no space, no ref syntax, no leading dash.
fn branch_ok(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(['-', '/'])
        && !name.ends_with(['/', '.'])
        && !name.ends_with(".lock")
        && !name.contains("..")
        && !name.contains("//")
        && !name.contains("@{")
        && name != "@"
        && !name
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\".contains(c))
}

/// `path` without `.` and with `..` resolved by the text alone.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The canonical form of `path` when it exists; otherwise its deepest existing ancestor canonicalised with the rest
/// appended by the text (a symlinked temp folder must not change a comparison).
fn canonical_lenient(path: &Path) -> PathBuf {
    let path = lexical(path);
    if let Ok(real) = std::fs::canonicalize(&path) {
        return real;
    }
    let mut rest: Vec<&std::ffi::OsStr> = Vec::new();
    let mut current = path.as_path();
    while let Some(parent) = current.parent() {
        if let Some(name) = current.file_name() {
            rest.push(name);
        }
        if let Ok(mut real) = std::fs::canonicalize(parent) {
            for name in rest.iter().rev() {
                real.push(name);
            }
            return real;
        }
        current = parent;
    }
    path
}

fn repo_path(root: &Path, configured: &str) -> PathBuf {
    let path = Path::new(configured);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    // A path that exists is canonical; one that does not is kept as it was joined.
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn resolve_verify(table: &crate::value::Table, name: &str) -> Result<Option<RepoVerify>> {
    let at = format!("repos.{name}.verify");
    let Some(item) = table.get("verify") else {
        return Ok(None);
    };
    let verify = required_table(Some(item), &at)?;
    reject_unknown_keys(verify, REPO_VERIFY_KEYS, &at)?;
    let Some(Item::Array(entries)) = verify.get("commands") else {
        return invalid(format!(
            "{at}.commands must be an array of 1 to {MAX_VERIFY_COMMANDS} command strings"
        ));
    };
    if entries.is_empty() || entries.len() > MAX_VERIFY_COMMANDS {
        return invalid(format!(
            "{at}.commands must have between 1 and {MAX_VERIFY_COMMANDS} entries"
        ));
    }
    let mut commands = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let command = required_string(
            Some(entry),
            &format!("{at}.commands[{index}]"),
            MAX_VERIFY_COMMAND_CHARS,
            false,
        )?;
        commands.push(command.to_string());
    }
    let timeout_seconds = match verify.get("timeout_seconds") {
        None => None,
        Some(_) => Some(optional_integer(
            verify.get("timeout_seconds"),
            &format!("{at}.timeout_seconds"),
            60,
            14_400,
            3600,
        )?),
    };
    Ok(Some(RepoVerify {
        commands,
        env: resolve_env(verify.get("env"), &format!("{at}.env"))?,
        timeout_seconds,
    }))
}

/// Resolves `[repos]` and `[workspace]` against the project root. No git runs. `cache` is the resolved `[cache]`
/// section, whose entry names a repository's `cache` list may use.
pub fn resolve_workspace(
    repos: Option<&Item>,
    workspace: Option<&Item>,
    project_root: &Path,
    cache: Option<&CacheConfig>,
) -> Result<Option<Workspace>> {
    let repo_table = optional_table(repos, "repos")?;
    let workspace_table = optional_table(workspace, "workspace")?;
    if repo_table.is_empty() {
        if workspace.is_some() {
            return invalid("[workspace] needs at least one [repos.<name>]");
        }
        return Ok(None);
    }
    reject_unknown_keys(workspace_table, WORKSPACE_KEYS, "workspace")?;
    if repo_table.len() > MAX_REPOS {
        return invalid(format!("at most {MAX_REPOS} repos may be listed"));
    }
    let root = std::fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let cache_names: Vec<&str> = cache
        .map(|cache| {
            cache
                .entries
                .iter()
                .chain(&cache.detectable)
                .map(|entry| entry.name.as_str())
                .collect()
        })
        .unwrap_or_default();

    let mut list: Vec<RepoConfig> = Vec::new();
    for (name, item) in repo_table.entries() {
        if !repo_name_ok(name) {
            return invalid(format!("repo name {name} must match {REPO_NAME_PATTERN}"));
        }
        let table = required_table(Some(item), &format!("repos.{name}"))?;
        reject_unknown_keys(table, REPO_KEYS, &format!("repos.{name}"))?;
        if table.get("path").is_none() {
            return invalid(format!("[repos.{name}] needs path"));
        }
        let configured = required_string(
            table.get("path"),
            &format!("repos.{name}.path"),
            MAX_PATH_CHARS,
            false,
        )?;
        if table.get("main_branch").is_none() {
            return invalid(format!("[repos.{name}] needs main_branch"));
        }
        let main_branch = required_string(
            table.get("main_branch"),
            &format!("repos.{name}.main_branch"),
            MAX_BRANCH_CHARS,
            false,
        )?;
        if !branch_ok(main_branch) {
            return invalid(format!(
                "repos.{name}.main_branch must be a plain branch name"
            ));
        }
        let repo_root = repo_path(&root, configured);
        if canonical_lenient(&repo_root) == root {
            return invalid("the workspace root cannot be a listed repo");
        }
        let verify = resolve_verify(table, name)?;
        let cache_list = match table.get("cache") {
            None => None,
            Some(_) => {
                let names = string_list(
                    table.get("cache"),
                    &format!("repos.{name}.cache"),
                    MAX_LIST_ENTRIES,
                )?;
                for entry in &names {
                    if !cache_names.contains(&entry.as_str()) {
                        return invalid(format!("repo {name} names unknown cache entry {entry}"));
                    }
                }
                Some(names)
            }
        };
        let setup = match table.get("setup") {
            None => None,
            Some(_) => {
                let at = format!("repos.{name}.setup");
                let command =
                    required_string(table.get("setup"), &at, MAX_WORKTREE_SETUP_CHARS, false)?;
                guard_credential_shape(command, &at)?;
                Some(command.to_string())
            }
        };
        list.push(RepoConfig {
            name: name.to_string(),
            root: repo_root,
            main_branch: main_branch.to_string(),
            verify,
            cache: cache_list,
            setup,
        });
    }
    list.sort_by(|a, b| a.name.cmp(&b.name));
    let names: Vec<String> = list.iter().map(|repo| repo.name.clone()).collect();

    let mut merge_order: Vec<String> = Vec::new();
    if workspace_table.get("merge_order").is_some() {
        let listed = string_list(
            workspace_table.get("merge_order"),
            "workspace.merge_order",
            MAX_REPOS,
        )?;
        for name in listed {
            if !names.contains(&name) {
                return invalid(format!("merge_order names unknown repo {name}"));
            }
            if merge_order.contains(&name) {
                return invalid(format!("merge_order names {name} twice"));
            }
            merge_order.push(name);
        }
    }
    for name in &names {
        if !merge_order.contains(name) {
            merge_order.push(name.clone());
        }
    }

    let default_repo = match workspace_table.get("default_repo") {
        None => names[0].clone(),
        Some(_) => {
            let name = required_string(
                workspace_table.get("default_repo"),
                "workspace.default_repo",
                32,
                false,
            )?;
            if !names.iter().any(|known| known == name) {
                return invalid(format!("default_repo names unknown repo {name}"));
            }
            name.to_string()
        }
    };

    let worktree_dir = match workspace_table.get("worktree_dir") {
        None => root.join(".capstan").join("worktrees"),
        Some(_) => {
            let configured = required_string(
                workspace_table.get("worktree_dir"),
                "workspace.worktree_dir",
                MAX_PATH_CHARS,
                false,
            )?;
            lexical(&root.join(configured))
        }
    };

    Ok(Some(Workspace {
        root,
        repos: list,
        merge_order,
        worktree_dir,
        default_repo,
    }))
}

// ---------------------------------------------------------------------------------------------------------------
// The git half

/// `git -C <dir> <args>` with a cleared environment (PATH only) and the project's fsmonitor off; `None` when git did
/// not run or exited non-zero, else its trimmed stdout.
fn git_out(dir: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command
        .env_clear()
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.fsmonitor=false"])
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    // PATH finds git; HOME and XDG_CONFIG_HOME keep the user's global git configuration (safe.directory), so this
    // check agrees with the cstan checks that inherit the environment.
    for name in ["PATH", "HOME", "XDG_CONFIG_HOME"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Runs `check` over each listed repository in order and returns the first problem, its message prefixed with the
/// repository's name. One loop for every per-repository git check (cstan, launcher).
pub fn first_repo_problem<P>(
    workspace: &Workspace,
    mut check: impl FnMut(&Path) -> Option<(P, String)>,
) -> Option<(P, String)> {
    workspace.repos.iter().find_map(|repo| {
        check(&repo.root)
            .map(|(problem, message)| (problem, format!("repo {}: {message}", repo.name)))
    })
}

/// Whether `dir` is the top level of a git work tree.
pub fn is_work_tree_root(dir: &Path) -> bool {
    let Ok(real) = std::fs::canonicalize(dir) else {
        return false;
    };
    git_out(dir, &["rev-parse", "--show-toplevel"])
        .and_then(|top| std::fs::canonicalize(top).ok())
        .is_some_and(|top| top == real)
}

fn common_dir(repo: &Path) -> Option<PathBuf> {
    let text = git_out(repo, &["rev-parse", "--git-common-dir"])?;
    std::fs::canonicalize(repo.join(text)).ok()
}

/// The git half of the checks, in the order a user fixes them: each repository is the top level of a work tree with a
/// commit and has its main branch; then no two are the same repository, none lies inside another, and the worktree
/// folder lies inside no repository and inside no other project.
pub fn validate_workspace(workspace: &Workspace) -> std::result::Result<(), String> {
    let mut canonical: Vec<(&RepoConfig, PathBuf)> = Vec::new();
    for repo in &workspace.repos {
        let real = std::fs::canonicalize(&repo.root);
        let top_ok = real.as_ref().is_ok_and(|real| {
            git_out(real, &["rev-parse", "--show-toplevel"])
                .and_then(|top| std::fs::canonicalize(top).ok())
                .is_some_and(|top| &top == real)
                && git_out(real, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"]).is_some()
        });
        let Ok(real) = real else {
            return Err(not_top_level(repo));
        };
        if !top_ok {
            return Err(not_top_level(repo));
        }
        let branch = format!("refs/heads/{}^{{commit}}", repo.main_branch);
        if git_out(&real, &["rev-parse", "--verify", "--quiet", &branch]).is_none() {
            return Err(format!(
                "repo {} has no branch {}",
                repo.name, repo.main_branch
            ));
        }
        canonical.push((repo, real));
    }
    for (index, (first, first_root)) in canonical.iter().enumerate() {
        for (second, second_root) in &canonical[index + 1..] {
            let (a, b) = (common_dir(first_root), common_dir(second_root));
            if a.is_some() && a == b {
                return Err(format!(
                    "repos {} and {} are the same repository",
                    first.name, second.name
                ));
            }
            if first_root.starts_with(second_root) {
                return Err(format!(
                    "repo {} lies inside repo {}",
                    first.name, second.name
                ));
            }
            if second_root.starts_with(first_root) {
                return Err(format!(
                    "repo {} lies inside repo {}",
                    second.name, first.name
                ));
            }
        }
    }
    let worktrees = canonical_lenient(&workspace.worktree_dir);
    for (repo, real) in &canonical {
        if worktrees.starts_with(real) {
            return Err(format!("worktree_dir lies inside repo {}", repo.name));
        }
    }
    let root = canonical_lenient(&workspace.root);
    for ancestor in worktrees.ancestors().skip(1) {
        if ancestor == root {
            break;
        }
        if ancestor.join(".capstan").is_dir() {
            return Err(format!(
                "worktree_dir lies inside another project {}",
                ancestor.display()
            ));
        }
    }
    Ok(())
}

fn not_top_level(repo: &RepoConfig) -> String {
    format!(
        "repo {}: {} is not the top level of a git work tree with a commit",
        repo.name,
        repo.root.display()
    )
}
