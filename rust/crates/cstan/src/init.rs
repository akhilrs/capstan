//! `cstan init`: the project directory, the operator key, the project record, the starter `capstan.toml` and the designer
//! prompt, with `--git` to set up the repository and its first commit.
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;

use capstan_config::starter::{DESIGNER_PROMPT, DESIGNER_PROMPT_PATH, STARTER_CONFIG};
use capstan_config::CONFIG_FILE_NAME;
use capstan_kernel::{Env, SystemEnv};
use capstan_wire::js;

use crate::git::{self, Check, Problem};
use crate::operator::{number, object, string};
use crate::{invalid, runtime, take_flag, Cli, Fail, Flow};

const CONFIG_NAME: &str = ".capstan/project.json";
const KEY_NAME: &str = ".capstan/operator.key";

/// The message of a Node `fs` error: `CODE: description, syscall 'path'`.
pub fn fs_error(error: &std::io::Error, syscall: &str, path: &Path) -> String {
    let (code, description) = match error.raw_os_error() {
        Some(libc::EACCES) => ("EACCES", "permission denied"),
        Some(libc::ENOENT) => ("ENOENT", "no such file or directory"),
        Some(libc::ENOTDIR) => ("ENOTDIR", "not a directory"),
        Some(libc::EEXIST) => ("EEXIST", "file already exists"),
        Some(libc::EISDIR) => ("EISDIR", "illegal operation on a directory"),
        Some(libc::EROFS) => ("EROFS", "read-only file system"),
        Some(libc::ENOSPC) => ("ENOSPC", "no space left on device"),
        Some(libc::EPERM) => ("EPERM", "operation not permitted"),
        Some(libc::ELOOP) => ("ELOOP", "too many symbolic links encountered"),
        Some(libc::ENAMETOOLONG) => ("ENAMETOOLONG", "name too long"),
        _ => return error.to_string(),
    };
    format!("{code}: {description}, {syscall} '{}'", path.display())
}

fn exists(path: &Path) -> bool {
    std::fs::metadata(path).is_ok()
}

fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..=chunk.len() {
            out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

/// `fs.writeFileSync(path, text, { flag: "wx", mode })`.
fn write_new(path: &Path, text: &str, mode: u32) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)?;
    file.write_all(text.as_bytes())
}

fn already_exists(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(libc::EEXIST)
}

impl Cli<'_> {
    /// `cstan init [--git]`.
    pub fn init(&mut self, rest: &[String]) -> Result<Flow, Fail> {
        let mut flags = rest.to_vec();
        let with_git = take_flag(&mut flags, "--git");
        if !flags.is_empty() {
            return Err(Fail::Usage);
        }
        let cwd = self.ctx.cwd.clone();
        let name = cwd
            .file_name()
            .map(|n| String::from_utf8_lossy(n.as_bytes()).into_owned())
            .unwrap_or_default();
        if capstan_kernel::helpers::js_trim(&name).is_empty() || name.encode_utf16().count() > 256 {
            return Err(invalid("project directory name is invalid"));
        }
        let directory = cwd.join(".capstan");
        if with_git {
            if let Some(refusal) = git::git_setup_refusal(&self.ctx.env, &cwd) {
                return Err(invalid(refusal));
            }
        }
        if with_git && exists(&directory) {
            // An initialized project: only the git part runs.
            self.setup_git(false)?;
            return Ok(Flow::Code(0));
        }
        if let Err(error) = std::fs::DirBuilder::new().mode(0o700).create(&directory) {
            return Err(if already_exists(&error) {
                invalid("Capstan project directory already exists; refusing to modify existing contents")
            } else {
                runtime(fs_error(&error, "mkdir", &directory))
            });
        }
        let config_path = cwd.join(CONFIG_NAME);
        if exists(&config_path) {
            return Err(runtime("Capstan project is already initialized"));
        }
        let env = SystemEnv;
        let project_id = format!("p{}", env.uuid().replace('-', ""));
        let state_directory = directory.join("state");
        let config = object(vec![
            ("schemaVersion", number(1.0)),
            ("projectId", string(&project_id)),
            ("name", string(&name)),
            ("stateDirectory", string(&state_directory.to_string_lossy())),
            ("maxSlices", number(4.0)),
            ("maxRunMs", number(3_600_000.0)),
            ("maxDispatches", number(16.0)),
        ]);
        let git_created = if with_git {
            git::git_init_if_needed(&self.ctx.env, &cwd).map_err(runtime)?
        } else {
            false
        };
        let credential_ignored = git::exclude_capstan_state(&self.ctx.env, &cwd)
            .map_err(|e| runtime(fs_error(&e, "open", &cwd)))?;
        let credential = base64url(&env.random_bytes(32));
        let key_path = cwd.join(KEY_NAME);
        write_new(&key_path, &format!("{credential}\n"), 0o600)
            .map_err(|e| runtime(fs_error(&e, "open", &key_path)))?;
        let json = js::stringify(&config, 2).to_utf8_lossy();
        write_new(&config_path, &format!("{json}\n"), 0o600)
            .map_err(|e| runtime(fs_error(&e, "open", &config_path)))?;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&state_directory)
            .map_err(|e| runtime(fs_error(&e, "mkdir", &state_directory)))?;
        let starter_path = cwd.join(CONFIG_FILE_NAME);
        let starter_written = match write_new(&starter_path, STARTER_CONFIG, 0o600) {
            Ok(()) => true,
            Err(error) if already_exists(&error) => false,
            Err(error) => return Err(runtime(fs_error(&error, "open", &starter_path))),
        };
        let mut designer_line = String::new();
        if starter_written {
            let designer_path = cwd.join(DESIGNER_PROMPT_PATH);
            if let Some(parent) = designer_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| runtime(fs_error(&e, "mkdir", parent)))?;
            }
            designer_line = match write_new(&designer_path, DESIGNER_PROMPT, 0o644) {
                Ok(()) => format!("Wrote {DESIGNER_PROMPT_PATH}\n"),
                Err(error) if already_exists(&error) => {
                    format!("Kept existing {DESIGNER_PROMPT_PATH}\n")
                }
                Err(error) => return Err(runtime(fs_error(&error, "open", &designer_path))),
            };
        }
        let starter_line = if starter_written {
            format!("Wrote starter {CONFIG_FILE_NAME}\n")
        } else {
            format!("Kept existing {CONFIG_FILE_NAME}\n")
        };
        let protect = if credential_ignored {
            "The repository-local Git exclude protects .capstan from ordinary staging."
        } else {
            "Add .capstan/ to .gitignore before staging project files."
        };
        let text = format!(
            "{starter_line}{designer_line}Initialized Capstan project {project_id}\nOperator credential: {} (0600)\n{protect}\n",
            key_path.display()
        );
        self.io.out.extend_from_slice(text.as_bytes());
        if with_git {
            self.setup_git(git_created)?;
        } else if let Check::Problem { message, .. } =
            git::check_git_requirement(&self.ctx.env, &cwd)
        {
            self.io.err_line(&format!("warning: {message}"));
        }
        Ok(Flow::Code(0))
    }

    /// `setupGit`: lists the files it will commit, then creates the initial commit when HEAD is unborn.
    fn setup_git(&mut self, already_created: bool) -> Result<(), Fail> {
        let cwd = self.ctx.cwd.clone();
        let created =
            git::git_init_if_needed(&self.ctx.env, &cwd).map_err(runtime)? || already_created;
        git::exclude_capstan_state(&self.ctx.env, &cwd)
            .map_err(|e| runtime(fs_error(&e, "open", &cwd)))?;
        if created {
            self.io.out_line("Ran git init");
        }
        match git::check_git_requirement(&self.ctx.env, &cwd) {
            Check::Ok => {
                self.io
                    .out_line("The repository already has a commit; nothing to commit");
                return Ok(());
            }
            Check::Problem { problem, message } if problem != Problem::NoCommit => {
                return Err(invalid(message));
            }
            Check::Problem { .. } => {}
        }
        let files = git::files_to_commit(&self.ctx.env, &cwd).map_err(runtime)?;
        let listing: Vec<String> = files.iter().map(|f| format!("  {f}")).collect();
        self.io.out_line(&format!(
            "Committing {} file{} as \"{}\":\n{}",
            files.len(),
            if files.len() == 1 { "" } else { "s" },
            git::INITIAL_COMMIT_MESSAGE,
            listing.join("\n")
        ));
        git::create_initial_commit(&self.ctx.env, &cwd).map_err(invalid)?;
        self.io.out_line("Created the initial commit");
        Ok(())
    }

    /// `requireGit`: refuses `cstan start` outside a usable git repository.
    pub fn require_git(&self) -> Result<(), Fail> {
        match git::check_git_requirement(&self.ctx.env, &self.ctx.cwd) {
            Check::Ok => Ok(()),
            Check::Problem { message, .. } => Err(invalid(message)),
        }
    }
}
