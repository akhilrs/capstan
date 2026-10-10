//! The git requirement of `src/git-requirement.ts`: the project root is the top of a work tree whose HEAD is a commit.
use std::ffi::OsString;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const INITIAL_COMMIT_MESSAGE: &str = "chore: initial commit";
const INIT_FLAG_HINT: &str =
    "or let Capstan do it: cstan init --git (it lists the files it will commit first)";
const TIMEOUT: Duration = Duration::from_secs(30);

/// What `spawnSync` returns for one git call.
pub struct GitRun {
    /// `None` when git could not be run, or was killed.
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    /// `result.error` is set: git did not start (not installed) or timed out.
    pub error: bool,
}

pub type Env<'a> = &'a [(OsString, OsString)];

pub fn git(env: Env, root: &Path, args: &[&str]) -> GitRun {
    let spawned = Command::new("git")
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k, v)))
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(_) => {
            return GitRun {
                status: None,
                stdout: String::new(),
                stderr: String::new(),
                error: true,
            }
        }
    };
    let drain = |mut pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            String::from_utf8_lossy(&bytes).into_owned()
        })
    };
    let out = drain(Box::new(child.stdout.take().expect("piped")));
    let err = drain(Box::new(child.stderr.take().expect("piped")));
    let deadline = Instant::now() + TIMEOUT;
    let mut error = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                error = true;
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(2)),
            Err(_) => {
                error = true;
                break None;
            }
        }
    };
    GitRun {
        status,
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
        error,
    }
}

fn ok(run: &GitRun) -> bool {
    run.status == Some(0)
}

/// `path.resolve` for a path that may be relative to `cwd` or contain `..`.
pub fn resolve(path: &Path, cwd: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    for component in absolute.components() {
        match component {
            Component::RootDir => out.push("/"),
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
            _ => {}
        }
    }
    out
}

fn real(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| {
        resolve(
            path,
            &std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
        )
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    GitMissing,
    NotARepository,
    BareRepository,
    Subdirectory,
    NoCommit,
}

pub enum Check {
    Ok,
    Problem { problem: Problem, message: String },
}

fn problem(root: &Path, kind: Problem, what: &str) -> Check {
    let root_text = root.display();
    let fix = match kind {
        Problem::GitMissing => "install git, then run cstan again".to_string(),
        Problem::BareRepository => "run Capstan in a normal checkout (git clone <this repository> <folder>), not in the bare repository".to_string(),
        Problem::Subdirectory => "run cstan from the repository's top-level folder (git rev-parse --show-toplevel prints it); a project root inside a larger repository is not supported".to_string(),
        _ => format!(
            "run in {root_text}:\n  git init\n  git add -A\n  git commit -m \"{INITIAL_COMMIT_MESSAGE}\""
        ),
    };
    let hint = if matches!(kind, Problem::NotARepository | Problem::NoCommit) {
        format!("\n{INIT_FLAG_HINT}")
    } else {
        String::new()
    };
    Check::Problem {
        problem: kind,
        message: format!(
            "Capstan needs a git repository with at least one commit (workers get their own git worktree and branch from HEAD), but {what}.\nFix: {fix}{hint}"
        ),
    }
}

/// `checkGitRequirement`: checks `root` against the git requirement; never changes anything.
pub fn check_git_requirement(env: Env, root: &Path) -> Check {
    let inside = git(env, root, &["rev-parse", "--is-inside-work-tree"]);
    if inside.error {
        return problem(root, Problem::GitMissing, "git could not be run");
    }
    if !ok(&inside) || inside.stdout.trim() != "true" {
        let bare = git(env, root, &["rev-parse", "--is-bare-repository"]);
        if ok(&bare) && bare.stdout.trim() == "true" {
            return problem(
                root,
                Problem::BareRepository,
                &format!("{} is a bare repository", root.display()),
            );
        }
        return problem(
            root,
            Problem::NotARepository,
            &format!("{} is not inside a git work tree", root.display()),
        );
    }
    let top = git(env, root, &["rev-parse", "--show-toplevel"]);
    if ok(&top) && real(Path::new(top.stdout.trim())) != real(root) {
        return problem(
            root,
            Problem::Subdirectory,
            &format!(
                "{} is a subdirectory of the repository at {}",
                root.display(),
                top.stdout.trim()
            ),
        );
    }
    let head = git(
        env,
        root,
        &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"],
    );
    if !ok(&head) {
        return problem(
            root,
            Problem::NoCommit,
            &format!(
                "the repository at {} has no commit yet (HEAD is an unborn branch)",
                root.display()
            ),
        );
    }
    Check::Ok
}

/// `text.replace(/\r?\n$/, "")`.
fn chomp(text: &str) -> &str {
    match text.strip_suffix('\n') {
        Some(rest) => rest.strip_suffix('\r').unwrap_or(rest),
        None => text,
    }
}

/// `excludeCapstanState`: adds `/.capstan/` to the repository's local exclude file so project state is never staged; true
/// when the repository root is `root` and the file protects it.
pub fn exclude_capstan_state(env: Env, root: &Path) -> std::io::Result<bool> {
    let top = git(env, root, &["rev-parse", "--show-toplevel"]);
    if !ok(&top) || resolve(Path::new(chomp(&top.stdout)), root) != root {
        return Ok(false);
    }
    let exclude = git(env, root, &["rev-parse", "--git-path", "info/exclude"]);
    if !ok(&exclude) {
        return Ok(false);
    }
    let exclude_path = resolve(Path::new(exclude.stdout.trim()), root);
    let existing = if exclude_path.exists() {
        String::from_utf8_lossy(&std::fs::read(&exclude_path)?).into_owned()
    } else {
        String::new()
    };
    if !existing
        .split('\n')
        .any(|line| line.strip_suffix('\r').unwrap_or(line) == "/.capstan/")
    {
        if let Some(parent) = exclude_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .open(&exclude_path)?;
        let lead = if !existing.is_empty() && !existing.ends_with('\n') {
            "\n"
        } else {
            ""
        };
        file.write_all(format!("{lead}/.capstan/\n").as_bytes())?;
    }
    Ok(true)
}

/// `gitSetupRefusal`: whether `--git` may run in `root`: not a bare repository and not inside another repository.
pub fn git_setup_refusal(env: Env, root: &Path) -> Option<String> {
    match check_git_requirement(env, root) {
        Check::Ok => None,
        Check::Problem { problem, message } => match problem {
            Problem::BareRepository | Problem::Subdirectory | Problem::GitMissing => Some(message),
            _ => None,
        },
    }
}

/// `gitInitIfNeeded`: runs `git init` unless `root` is already a work tree. Returns whether it ran.
pub fn git_init_if_needed(env: Env, root: &Path) -> Result<bool, String> {
    let inside = git(env, root, &["rev-parse", "--is-inside-work-tree"]);
    if ok(&inside) && inside.stdout.trim() == "true" {
        return Ok(false);
    }
    let result = git(env, root, &["init", "--quiet"]);
    if !ok(&result) {
        return Err(format!("git init failed: {}", result.stderr.trim()));
    }
    Ok(true)
}

/// `filesToCommit`: the files `git add -A` would stage.
pub fn files_to_commit(env: Env, root: &Path) -> Result<Vec<String>, String> {
    let result = git(
        env,
        root,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    );
    if !ok(&result) {
        return Err(format!("git ls-files failed: {}", result.stderr.trim()));
    }
    Ok(result
        .stdout
        .split('\0')
        .filter(|file| !file.is_empty())
        .map(str::to_string)
        .collect())
}

/// `createInitialCommit`: stages everything not ignored and creates the initial commit.
pub fn create_initial_commit(env: Env, root: &Path) -> Result<(), String> {
    let add = git(env, root, &["add", "-A"]);
    if !ok(&add) {
        return Err(format!("git add failed: {}", add.stderr.trim()));
    }
    let commit = git(
        env,
        root,
        &["commit", "--quiet", "-m", INITIAL_COMMIT_MESSAGE],
    );
    if !ok(&commit) {
        let detail = if commit.stderr.is_empty() {
            &commit.stdout
        } else {
            &commit.stderr
        };
        return Err(format!(
            "git commit failed: {}\nIf git asks who you are, set it (git config user.name / user.email) and run: git commit -m \"{INITIAL_COMMIT_MESSAGE}\"",
            detail.trim()
        ));
    }
    Ok(())
}
