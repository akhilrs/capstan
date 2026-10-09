//! The default git runner and the checks that guard worktree removal (src/launcher/git.ts), and the git requirement
//! (src/git-requirement.ts) `head_sha` starts with.
//!
//! Every call is an argument array (no shell) with the clean environment of src/git.ts: only what git needs, and the
//! daemon's own `GIT_*` variables and the user's and the system's git configuration left out.

use crate::api::LauncherError;
use crate::shared::{GitRunner, WorktreeRemoval, MAX_NOTE_LENGTH};
use crate::text::one_line;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

const GIT_TIMEOUT: Duration = Duration::from_secs(30);

/// `cleanGitEnvironment`: only what git needs; every inherited `GIT_*` variable, the user's and the system's git
/// configuration are left out.
pub fn clean_git_environment() -> Vec<(String, String)> {
    vec![
        (
            "PATH".into(),
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()),
        ),
        ("LC_ALL".into(), "C".into()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
        ("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()),
        ("GIT_TERMINAL_PROMPT".into(), "0".into()),
    ]
}

/// What one run of git produced; `status` is None when git did not start, was killed or ran out of time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitOutput {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    /// Git could not be run at all (it is not installed).
    pub missing: bool,
}

impl GitOutput {
    pub fn ok(&self) -> bool {
        self.status == Some(0)
    }
}

fn drain(mut pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe.read_to_end(&mut bytes);
        bytes
    })
}

/// Runs `git args` in `cwd` (or the current directory) with the clean environment and the 30 s limit.
pub fn run_git(args: &[&str], cwd: Option<&Path>) -> GitOutput {
    let mut command = Command::new("git");
    command
        .args(args)
        .env_clear()
        .envs(clean_git_environment())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return GitOutput {
                status: None,
                stdout: String::new(),
                stderr: String::new(),
                missing: error.kind() == std::io::ErrorKind::NotFound,
            }
        }
    };
    let stdout = child.stdout.take().map(drain);
    let stderr = child.stderr.take().map(drain);
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) if started.elapsed() >= GIT_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(2)),
            Err(_) => break None,
        }
    };
    let collect = |handle: Option<std::thread::JoinHandle<Vec<u8>>>| {
        handle
            .and_then(|h| h.join().ok())
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default()
    };
    GitOutput {
        status,
        stdout: collect(stdout),
        stderr: collect(stderr),
        missing: false,
    }
}

fn real(value: &str) -> PathBuf {
    std::fs::canonicalize(value).unwrap_or_else(|_| {
        let path = Path::new(value);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir().unwrap_or_default().join(path)
        }
    })
}

fn is_hex(text: &str, length: usize) -> bool {
    text.len() == length && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

// ------------------------------------------------------------------------------------------------ the git requirement

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitProblem {
    GitMissing,
    NotARepository,
    BareRepository,
    Subdirectory,
    NoCommit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitCheck {
    Ok {
        detached: bool,
    },
    Problem {
        problem: GitProblem,
        message: String,
    },
}

const INITIAL_COMMIT_MESSAGE: &str = "chore: initial commit";
const INIT_FLAG_HINT: &str =
    "or let Capstan do it: cstan init --git (it lists the files it will commit first)";

fn problem(root: &str, kind: GitProblem, what: &str) -> GitCheck {
    let fix_steps =
        format!("  git init\n  git add -A\n  git commit -m \"{INITIAL_COMMIT_MESSAGE}\"");
    let fix = match kind {
        GitProblem::GitMissing => "install git, then run cstan again".to_string(),
        GitProblem::BareRepository => "run Capstan in a normal checkout (git clone <this repository> <folder>), not in the bare repository".to_string(),
        GitProblem::Subdirectory => "run cstan from the repository's top-level folder (git rev-parse --show-toplevel prints it); a project root inside a larger repository is not supported".to_string(),
        _ => format!("run in {root}:\n{fix_steps}"),
    };
    let hint = if matches!(kind, GitProblem::NotARepository | GitProblem::NoCommit) {
        format!("\n{INIT_FLAG_HINT}")
    } else {
        String::new()
    };
    GitCheck::Problem {
        problem: kind,
        message: format!(
            "Capstan needs a git repository with at least one commit (workers get their own git worktree and branch from HEAD), but {what}.\nFix: {fix}{hint}"
        ),
    }
}

fn git_at(root: &str, args: &[&str]) -> GitOutput {
    let mut full = vec!["-C", root];
    full.extend_from_slice(args);
    run_git(&full, None)
}

/// `checkGitRequirement`: checks `root` against the git requirement; never changes anything.
pub fn check_git_requirement(root: &str) -> GitCheck {
    let inside = git_at(root, &["rev-parse", "--is-inside-work-tree"]);
    if inside.missing {
        return problem(root, GitProblem::GitMissing, "git could not be run");
    }
    if !inside.ok() || inside.stdout.trim() != "true" {
        let bare = git_at(root, &["rev-parse", "--is-bare-repository"]);
        if bare.ok() && bare.stdout.trim() == "true" {
            return problem(
                root,
                GitProblem::BareRepository,
                &format!("{root} is a bare repository"),
            );
        }
        return problem(
            root,
            GitProblem::NotARepository,
            &format!("{root} is not inside a git work tree"),
        );
    }
    let top = git_at(root, &["rev-parse", "--show-toplevel"]);
    if top.ok() && real(top.stdout.trim()) != real(root) {
        return problem(
            root,
            GitProblem::Subdirectory,
            &format!(
                "{root} is a subdirectory of the repository at {}",
                top.stdout.trim()
            ),
        );
    }
    let head = git_at(root, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"]);
    if !head.ok() {
        return problem(
            root,
            GitProblem::NoCommit,
            &format!("the repository at {root} has no commit yet (HEAD is an unborn branch)"),
        );
    }
    let symbolic = git_at(root, &["symbolic-ref", "--quiet", "HEAD"]);
    GitCheck::Ok {
        detached: !symbolic.ok(),
    }
}

// ------------------------------------------------------------------------------------------------ the default runner

/// Whether git lists `worktree_path` as a worktree checked out on a legacy `capstan/` branch or on `recorded_branch`, the
/// branch the ledger records for its agent.
fn is_capstan_worktree(
    git: &DefaultGit,
    worktree_path: &str,
    recorded_branch: Option<&str>,
) -> bool {
    let result = git.run(&["worktree", "list", "--porcelain", "-z"]);
    if !result.ok() {
        return false;
    }
    let wanted = real(worktree_path);
    let mut current: Option<&str> = None;
    for line in result.stdout.split('\0') {
        if let Some(path) = line.strip_prefix("worktree ") {
            current = Some(path);
        }
        let capstan_branch = line.starts_with("branch refs/heads/capstan/")
            || recorded_branch.is_some_and(|b| line == format!("branch refs/heads/{b}"));
        if capstan_branch && current.is_some_and(|path| real(path) == wanted) {
            return true;
        }
    }
    false
}

/// `defaultGit`.
pub struct DefaultGit {
    project_root: String,
}

impl DefaultGit {
    pub fn new(project_root: &str) -> Self {
        Self {
            project_root: project_root.to_string(),
        }
    }

    fn run(&self, args: &[&str]) -> GitOutput {
        run_git(args, Some(Path::new(&self.project_root)))
    }
}

static VERSION: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"git version (\d+)\.(\d+)").expect("the pattern is valid"));

impl GitRunner for DefaultGit {
    fn head_sha(&self) -> Result<String, LauncherError> {
        if let GitCheck::Problem { message, .. } = check_git_requirement(&self.project_root) {
            return Err(LauncherError::new("git_requirement", message));
        }
        let result = self.run(&["rev-parse", "HEAD"]);
        let sha = result.stdout.trim();
        if is_hex(sha, 64) {
            return Err(LauncherError::new(
                "git_error",
                "this repository uses SHA-256 object names; only SHA-1 repositories are supported",
            ));
        }
        if !result.ok() || !is_hex(sha, 40) {
            return Err(LauncherError::new(
                "git_error",
                "the project has no commit to branch from",
            ));
        }
        Ok(sha.to_string())
    }

    fn worktree_remove(
        &self,
        worktree_path: &str,
        recorded_branch: Option<&str>,
    ) -> WorktreeRemoval {
        let mut args = vec!["worktree", "remove"];
        if is_capstan_worktree(self, worktree_path, recorded_branch) {
            args.push("--force");
        }
        args.push(worktree_path);
        let result = self.run(&args);
        WorktreeRemoval {
            removed: result.ok(),
            stderr: one_line(&result.stderr, MAX_NOTE_LENGTH),
        }
    }

    fn worktree_present(&self, worktree_path: &str) -> bool {
        Path::new(worktree_path).exists()
    }

    fn prune_worktrees(&self) {
        self.run(&["worktree", "prune"]);
    }

    fn worktree_dirty_count(&self, worktree_path: &str) -> Option<usize> {
        let result = run_git(&["-C", worktree_path, "status", "--porcelain", "-z"], None);
        if !result.ok() {
            return None;
        }
        Some(
            result
                .stdout
                .split('\0')
                .filter(|entry| !entry.is_empty())
                .count(),
        )
    }

    fn delete_branch_if(&self, branch: &str, sha: &str) -> bool {
        self.run(&["update-ref", "-d", &format!("refs/heads/{branch}"), sha])
            .ok()
    }

    fn worktree_by_branch(&self, branch: &str) -> Result<Option<String>, LauncherError> {
        let result = self.run(&["worktree", "list", "--porcelain", "-z"]);
        // A failing git (or one too old for -z) must never read as "no worktree".
        if !result.ok() {
            return Err(LauncherError::new(
                "git_error",
                "git could not list the worktrees",
            ));
        }
        let wanted = format!("branch refs/heads/{branch}");
        let mut current: Option<&str> = None;
        for line in result.stdout.split('\0') {
            if let Some(path) = line.strip_prefix("worktree ") {
                current = Some(path);
            }
            if line == wanted {
                return Ok(current.map(str::to_string));
            }
        }
        Ok(None)
    }

    fn branch_name_valid(&self, branch: &str) -> bool {
        self.run(&["check-ref-format", &format!("refs/heads/{branch}")])
            .ok()
    }

    fn rename_branch(&self, from: &str, to: &str) -> WorktreeRemoval {
        let result = self.run(&["branch", "-m", from, to]);
        WorktreeRemoval {
            removed: result.ok(),
            stderr: one_line(&result.stderr, MAX_NOTE_LENGTH),
        }
    }

    fn save_ref(&self, reference: &str, sha: &str) -> bool {
        self.run(&["update-ref", reference, sha]).ok()
    }

    fn branch_tip(&self, branch: &str) -> Option<String> {
        let result = self.run(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}^{{commit}}"),
        ]);
        let sha = result.stdout.trim();
        (result.ok() && is_hex(sha, 40)).then(|| sha.to_string())
    }

    fn reachable_commit(&self, sha: &str, from: &[String]) -> Result<bool, LauncherError> {
        if !is_hex(sha, 40) {
            return Ok(false);
        }
        // `--end-of-options` came with git 2.24; an older git must not be mistaken for "no such commit".
        let version = self.run(&["--version"]);
        let too_old = match VERSION.captures(&version.stdout) {
            None => true,
            Some(found) => {
                let major: u64 = found[1].parse().unwrap_or(0);
                let minor: u64 = found[2].parse().unwrap_or(0);
                major < 2 || (major == 2 && minor < 24)
            }
        };
        if too_old {
            return Err(LauncherError::new(
                "old_git",
                "git 2.24 or newer is needed to check a commit",
            ));
        }
        let verify = self.run(&[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{sha}^{{commit}}"),
        ]);
        if !verify.ok() {
            return Ok(false);
        }
        Ok(from.iter().any(|reference| {
            self.run(&["merge-base", "--is-ancestor", sha, reference])
                .ok()
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet", "-b", "main"]);
        std::fs::write(dir.path().join("a"), "a").unwrap();
        git(dir.path(), &["add", "-A"]);
        git(
            dir.path(),
            &["commit", "--quiet", "-m", "chore: initial commit"],
        );
        dir
    }

    #[test]
    fn the_requirement_names_what_is_missing() {
        let empty = tempfile::tempdir().unwrap();
        let root = empty.path().to_string_lossy().into_owned();
        match check_git_requirement(&root) {
            GitCheck::Problem { problem, message } => {
                assert_eq!(problem, GitProblem::NotARepository);
                assert!(
                    message.contains("is not inside a git work tree"),
                    "{message}"
                );
            }
            other => panic!("{other:?}"),
        }
        let repo = repo();
        let root = repo.path().to_string_lossy().into_owned();
        assert_eq!(
            check_git_requirement(&root),
            GitCheck::Ok { detached: false }
        );
    }

    #[test]
    fn worktrees_and_branches_round_trip() {
        let repo = repo();
        let root = repo.path().to_string_lossy().into_owned();
        let runner = DefaultGit::new(&root);
        let head = runner.head_sha().unwrap();
        assert_eq!(head.len(), 40);
        assert_eq!(runner.branch_tip("main").as_deref(), Some(head.as_str()));
        assert_eq!(runner.branch_tip("nope"), None);
        assert!(runner.branch_name_valid("feat/x-1"));
        assert!(!runner.branch_name_valid("bad..name"));
        let tree = repo.path().join("..").join(format!(
            "tree-{}",
            repo.path().file_name().unwrap().to_string_lossy()
        ));
        let tree_text = tree.to_string_lossy().into_owned();
        git(
            repo.path(),
            &["worktree", "add", "-b", "feat/x-1", &tree_text, &head],
        );
        assert_eq!(
            runner
                .worktree_by_branch("feat/x-1")
                .unwrap()
                .map(|p| real(&p)),
            Some(real(&tree_text))
        );
        assert_eq!(
            runner.worktree_by_branch("main").unwrap().map(|p| real(&p)),
            Some(real(&root))
        );
        assert!(runner.worktree_present(&tree_text));
        std::fs::write(tree.join("untracked"), "x").unwrap();
        assert_eq!(runner.worktree_dirty_count(&tree_text), Some(1));
        assert!(runner
            .reachable_commit(&head, &["feat/x-1".to_string()])
            .unwrap());
        assert!(!runner
            .reachable_commit(&"0".repeat(40), &["main".to_string()])
            .unwrap());
        // A worktree of an unrelated branch is not forced: the untracked file refuses the removal.
        let refused = runner.worktree_remove(&tree_text, None);
        assert!(!refused.removed, "{refused:?}");
        // The ledger's recorded branch makes it a Capstan worktree, which is forced.
        let removed = runner.worktree_remove(&tree_text, Some("feat/x-1"));
        assert!(removed.removed, "{removed:?}");
        assert!(runner.delete_branch_if("feat/x-1", &head));
        assert_eq!(runner.branch_tip("feat/x-1"), None);
        assert!(runner.save_ref("refs/capstan/kept/a-1", &head));
        let renamed = runner.rename_branch("main", "trunk");
        assert!(renamed.removed, "{renamed:?}");
    }
}
