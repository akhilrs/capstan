//! The agent environment and the project check on its socket: `agentEnvironment` and `socketVerdict` of the Node CLI.
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use crate::jsops::has_space_or_control;

/// How the socket in the environment relates to the project the working directory is in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// No `.capstan` above the working directory.
    None,
    Match,
    Foreign,
}

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

/// Whether `socket` is the socket of the project `cwd` is in: the nearest ancestor with a `.capstan` directory.
pub fn socket_verdict(cwd: &Path, socket: &Path) -> Verdict {
    let mut dir = real_or_resolved(cwd);
    loop {
        let marker = dir.join(".capstan");
        if std::fs::metadata(&marker).is_ok_and(|m| m.is_dir()) {
            let expected = marker.join("state").join("control.sock");
            return if real_or_resolved(socket) == real_or_resolved(&expected) {
                Verdict::Match
            } else {
                Verdict::Foreign
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

/// Why the environment is not an agent environment the front end serves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotAgent {
    /// Neither variable is set (an operator shell).
    Unset,
    /// Half set, a relative socket, whitespace or control characters, or text that is not UTF-8.
    Invalid,
    /// The socket belongs to another project.
    Foreign,
}

/// `agentEnvironment()` of the Node CLI without its messages: anything but a usable environment is a reason to hand the
/// command to Node, which words the refusal.
pub fn agent_environment(env: &[(OsString, OsString)], cwd: &Path) -> Result<AgentEnv, NotAgent> {
    let token = var(env, "CAPSTAN_TOKEN").filter(|v| !v.is_empty());
    let socket = var(env, "CAPSTAN_SOCKET").filter(|v| !v.is_empty());
    let (token, socket) = match (token, socket) {
        (None, None) => return Err(NotAgent::Unset),
        (Some(token), Some(socket)) => (token, socket),
        _ => return Err(NotAgent::Invalid),
    };
    let (Some(token), Some(socket_text)) = (token.to_str(), socket.to_str()) else {
        return Err(NotAgent::Invalid);
    };
    if !socket_text.starts_with('/')
        || has_space_or_control(token)
        || has_space_or_control(socket_text)
    {
        return Err(NotAgent::Invalid);
    }
    let socket = Path::new(OsStr::from_bytes(socket_text.as_bytes())).to_path_buf();
    if socket_verdict(cwd, &socket) == Verdict::Foreign {
        return Err(NotAgent::Foreign);
    }
    Ok(AgentEnv {
        token: token.to_string(),
        socket,
    })
}
