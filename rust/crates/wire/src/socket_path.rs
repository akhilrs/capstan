//! Where the control socket of a project lives, and the private per-user directory used when the project path is too
//! long for a Unix socket address.
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::transport::socket_path_limit;

/// The file name of the control socket inside a state directory.
pub const SOCKET_NAME: &str = "control.sock";

fn my_uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

/// The name of the per-user directory that holds sockets of projects with long paths.
pub fn runtime_dir_name() -> String {
    format!("capstan-{}", my_uid())
}

/// The path made absolute and normalised without touching the file system: `.` is dropped, `..` pops one component.
fn lexical_absolute(path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(path)
    };
    let mut out = PathBuf::from("/");
    for part in joined.components() {
        match part {
            Component::Normal(name) => out.push(name),
            Component::ParentDir => {
                out.pop();
            }
            _ => {}
        }
    }
    out
}

/// The canonical form of a state directory that may not exist yet: its parent (the `.capstan` directory) is
/// canonicalized, so the answer is the same before and after the daemon creates it.
fn canonical_state(state_directory: &Path) -> PathBuf {
    let absolute = lexical_absolute(state_directory);
    if let (Some(parent), Some(name)) = (absolute.parent(), absolute.file_name()) {
        if let Ok(real) = fs::canonicalize(parent) {
            return real.join(name);
        }
    }
    absolute
}

/// The control socket of the project whose state directory is `state_directory`.
pub fn control_socket(state_directory: &Path) -> PathBuf {
    control_socket_in(state_directory, Path::new("/tmp"))
}

/// As `control_socket`, with the base of the fallback directory given (tests only).
pub fn control_socket_in(state_directory: &Path, base: &Path) -> PathBuf {
    let canonical = canonical_state(state_directory);
    let inside = canonical.join(SOCKET_NAME);
    if inside.as_os_str().as_bytes().len() <= socket_path_limit() {
        return inside;
    }
    let digest = Sha256::digest(canonical.as_os_str().as_bytes());
    let hex: String = digest.iter().take(16).map(|b| format!("{b:02x}")).collect();
    base.join(runtime_dir_name()).join(format!("{hex}.sock"))
}

/// The pure verdict on a directory; the reason on refusal.
fn private_dir_verdict(
    is_dir: bool,
    is_symlink: bool,
    uid: u32,
    mode: u32,
    my_uid: u32,
) -> Result<(), String> {
    if is_symlink {
        return Err("it is a symbolic link".to_string());
    }
    if !is_dir {
        return Err("it is not a directory".to_string());
    }
    if uid != my_uid {
        return Err(format!("it is owned by user {uid}, not by you ({my_uid})"));
    }
    if mode & 0o077 != 0 {
        return Err(format!("its mode is {:03o}, not 700", mode & 0o777));
    }
    Ok(())
}

/// Refuses a directory that is not a real directory of this user with no group or other access. Changes nothing.
pub fn check_private_dir(dir: &Path) -> Result<(), String> {
    let meta =
        fs::symlink_metadata(dir).map_err(|e| format!("cannot inspect {}: {e}", dir.display()))?;
    private_dir_verdict(
        meta.is_dir(),
        meta.file_type().is_symlink(),
        meta.uid(),
        meta.mode(),
        my_uid(),
    )
    .map_err(|reason| {
        format!(
            "{} is not safe for the control socket: {reason}; remove it or run chmod 700 on it",
            dir.display()
        )
    })
}

/// Creates `dir` with mode 0700 when missing (its parent must exist), then checks it.
pub fn ensure_private_dir(dir: &Path) -> Result<(), String> {
    match fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("cannot create {}: {e}", dir.display())),
    }
    check_private_dir(dir)
}

fn real_or_resolved(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| lexical_absolute(path))
}

/// Whether `socket` is the control socket of the project at `state_directory`, in any path form.
pub fn socket_matches(state_directory: &Path, socket: &Path) -> bool {
    real_or_resolved(socket) == real_or_resolved(&control_socket(state_directory))
}

/// Whether the socket file, if present, belongs to the user.
pub(crate) fn socket_owned_by_user(socket: &Path) -> bool {
    match fs::symlink_metadata(socket) {
        Ok(meta) => meta.uid() == my_uid(),
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::private_dir_verdict as verdict;

    #[test]
    fn the_verdict_refuses_other_owners_and_group_or_other_bits() {
        assert!(verdict(true, false, 1000, 0o700, 1000).is_ok());
        assert!(verdict(true, false, 0, 0o700, 1000).is_err());
        assert!(verdict(true, false, 1000, 0o750, 1000).is_err());
        assert!(verdict(true, false, 1000, 0o705, 1000).is_err());
        assert!(verdict(true, true, 1000, 0o700, 1000).is_err());
        assert!(verdict(false, false, 1000, 0o600, 1000).is_err());
    }
}
