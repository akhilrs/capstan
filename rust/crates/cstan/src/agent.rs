//! The agent environment and the project check on its socket: `agentEnvironment` and `socketVerdict` of the Node CLI.
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};

use crate::jsops::{has_space_or_control, JsError};
use crate::{invalid, Cli, Fail};

/// The environment variable `name`, `None` when unset.
pub fn var<'a>(env: &'a [(OsString, OsString)], name: &str) -> Option<&'a OsStr> {
    env.iter()
        .find(|(key, _)| key.as_os_str() == OsStr::new(name))
        .map(|(_, value)| value.as_os_str())
}

/// `path.resolve` for a path that is already absolute: drops `.` and empty parts and applies `..`.
fn resolve(path: &Path) -> PathBuf {
    let mut out = PathBuf::from("/");
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
            _ => {}
        }
    }
    out
}

fn real_or_resolved(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| resolve(path))
}

/// `socketVerdict`: how `socket` (the text of `CAPSTAN_SOCKET`) relates to the project `cwd` is in, the nearest ancestor
/// with a `.capstan` directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    None,
    Match,
    Foreign {
        project_root: PathBuf,
        expected_socket: PathBuf,
    },
}

pub fn socket_verdict(cwd: &Path, socket: &Path) -> Verdict {
    let mut dir = real_or_resolved(cwd);
    loop {
        let marker = dir.join(".capstan");
        if std::fs::metadata(&marker).is_ok_and(|m| m.is_dir()) {
            let expected = marker.join("state").join("control.sock");
            return if real_or_resolved(socket) == real_or_resolved(&expected) {
                Verdict::Match
            } else {
                Verdict::Foreign {
                    project_root: dir,
                    expected_socket: expected,
                }
            };
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => return Verdict::None,
        }
    }
}

/// A valid agent environment: the token and the absolute socket path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentEnv {
    pub token: String,
    pub socket: PathBuf,
}

impl Cli<'_> {
    /// `agentEnvironment()` of the Node CLI: `None` in an operator shell, the credentials of an agent, or the refusal. A
    /// socket of another project is refused (exit 2) unless `CAPSTAN_ALLOW_FOREIGN_SOCKET=1`, which warns once.
    pub fn agent_environment(&mut self) -> Result<Option<AgentEnv>, Fail> {
        let env = &self.ctx.env;
        let text = |name: &str| var(env, name).map(|v| v.to_string_lossy().into_owned());
        let token = text("CAPSTAN_TOKEN").filter(|v| !v.is_empty());
        let socket = text("CAPSTAN_SOCKET").filter(|v| !v.is_empty());
        let (token, socket) = match (token, socket) {
            (None, None) => return Ok(None),
            (Some(token), Some(socket)) if socket.starts_with('/') => (token, socket),
            _ => {
                return Err(invalid(
                    "CAPSTAN_TOKEN and CAPSTAN_SOCKET must both be set, and CAPSTAN_SOCKET must be an absolute path",
                ))
            }
        };
        if has_space_or_control(&token) {
            return Err(invalid(
                "CAPSTAN_TOKEN must not contain whitespace or control characters",
            ));
        }
        if has_space_or_control(&socket) {
            return Err(invalid(
                "CAPSTAN_SOCKET must not contain whitespace or control characters",
            ));
        }
        let socket_path = PathBuf::from(&socket);
        if let Verdict::Foreign {
            project_root,
            expected_socket,
        } = socket_verdict(&self.ctx.cwd, &socket_path)
        {
            let (root, expected) = (project_root.display(), expected_socket.display());
            if var(env, "CAPSTAN_ALLOW_FOREIGN_SOCKET").and_then(|v| v.to_str()) != Some("1") {
                return Err(Fail::Error(JsError {
                    exit: crate::EXIT_USAGE,
                    message: format!(
                        "CAPSTAN_SOCKET ({socket}) is not the socket of the project you are in ({root}, socket {expected}); nothing was sent. Unset CAPSTAN_SOCKET and CAPSTAN_TOKEN, or set CAPSTAN_ALLOW_FOREIGN_SOCKET=1 to use it anyway"
                    )
                    .into(),
                }));
            }
            if !self.foreign_warned {
                self.foreign_warned = true;
                self.io.err_line(&format!(
                    "warning: CAPSTAN_SOCKET ({socket}) is not the socket of the project you are in ({root}); continuing because CAPSTAN_ALLOW_FOREIGN_SOCKET=1"
                ));
            }
        }
        Ok(Some(AgentEnv {
            token,
            socket: socket_path,
        }))
    }
}
