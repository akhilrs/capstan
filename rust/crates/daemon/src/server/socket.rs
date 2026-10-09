//! The control socket's file: `removeStaleSocket` and the bind with its permissions (src/daemon.ts).

use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::Path;

fn uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

fn other(message: &str) -> io::Error {
    io::Error::other(message.to_string())
}

/// The caller holds the project lock, and only the lock holder binds the socket, so anything already at the path is stale
/// by construction. A path that is relative, a directory that is not private to the user, or something at the path that is
/// not a socket of this user is refused.
pub fn remove_stale_socket(socket_path: &Path) -> io::Result<()> {
    if !socket_path.is_absolute() {
        return Err(other("control socket path must be absolute"));
    }
    let directory = std::fs::symlink_metadata(socket_path.parent().unwrap_or(Path::new("/")))?;
    if !directory.is_dir()
        || directory.file_type().is_symlink()
        || directory.uid() != uid()
        || directory.mode() & 0o077 != 0
    {
        return Err(other(
            "control socket directory must be private and owned by the current user",
        ));
    }
    let stat = match std::fs::symlink_metadata(socket_path) {
        Ok(stat) => stat,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !stat.file_type().is_socket() {
        return Err(other("control socket path exists and is not a socket"));
    }
    if stat.uid() != uid() {
        return Err(other("control socket path is owned by another user"));
    }
    std::fs::remove_file(socket_path)
}

/// What identifies the socket file this process created, so cleanup removes only that one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SocketIdentity {
    pub dev: u64,
    pub ino: u64,
}

/// Binds the listening socket with mode 0600 (the umask is 0077 while it is created, and the mode is set again after).
pub fn bind(socket_path: &Path) -> io::Result<(UnixListener, SocketIdentity)> {
    // SAFETY: umask has no preconditions; the previous mask is restored below.
    let previous = unsafe { libc::umask(0o077) };
    let bound = UnixListener::bind(socket_path);
    // SAFETY: restores the mask read above.
    unsafe {
        libc::umask(previous);
    }
    let listener = bound?;
    let checked = (|| -> io::Result<SocketIdentity> {
        std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))?;
        let stat = std::fs::symlink_metadata(socket_path)?;
        if !stat.file_type().is_socket() {
            return Err(other("control socket was not created"));
        }
        Ok(SocketIdentity {
            dev: stat.dev(),
            ino: stat.ino(),
        })
    })();
    match checked {
        Ok(identity) => Ok((listener, identity)),
        Err(error) => {
            drop(listener);
            if std::fs::symlink_metadata(socket_path).is_ok_and(|stat| stat.file_type().is_socket())
            {
                let _ = std::fs::remove_file(socket_path);
            }
            Err(error)
        }
    }
}

/// `cleanup`: removes the socket file when it is still the one this process created.
pub fn remove_if_ours(socket_path: &Path, identity: SocketIdentity) -> io::Result<()> {
    match std::fs::symlink_metadata(socket_path) {
        Ok(current) => {
            if current.file_type().is_socket()
                && current.dev() == identity.dev
                && current.ino() == identity.ino
            {
                std::fs::remove_file(socket_path)?;
            }
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
