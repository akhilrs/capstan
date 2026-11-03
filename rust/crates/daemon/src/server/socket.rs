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

/// Whether the socket lives in the per-user runtime fallback directory (`/tmp/capstan-<uid>`), not in the project.
pub fn is_runtime_socket(socket_path: &Path) -> bool {
    socket_path
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == capstan_wire::runtime_dir_name().as_str())
}

/// For a runtime-fallback socket: creates the private runtime directory (or checks it) before anything else touches it.
/// An in-project socket is left to the rules of `remove_stale_socket`.
pub fn prepare_directory(socket_path: &Path) -> io::Result<()> {
    if !is_runtime_socket(socket_path) {
        return Ok(());
    }
    match socket_path.parent() {
        Some(parent) => capstan_wire::ensure_private_dir(parent).map_err(io::Error::other),
        None => Ok(()),
    }
}

/// Fails when something accepts a connection at `socket_path`. Only a refused connection or a missing file counts as
/// nobody listening. The project lock already excludes a second daemon of the same project, so this only guards against
/// a foreign listener (a hash collision or another program); the window between this probe and the unlink that follows
/// stays open against such a listener, and is accepted.
fn refuse_live_listener(socket_path: &Path) -> io::Result<()> {
    match std::os::unix::net::UnixStream::connect(socket_path) {
        Ok(_) => Err(other(&format!(
            "another controller listens on {}",
            socket_path.display()
        ))),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(other(&format!(
            "cannot tell whether another controller listens on {}: {error}",
            socket_path.display()
        ))),
    }
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
    if is_runtime_socket(socket_path) {
        refuse_live_listener(socket_path)?;
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
    capstan_wire::check_socket_path(socket_path).map_err(io::Error::other)?;
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

/// Removes a stale socket and binds a fresh listener: the start of the server, and the recovery after the socket was lost.
pub fn rebind(socket_path: &Path) -> io::Result<(UnixListener, SocketIdentity)> {
    prepare_directory(socket_path)?;
    remove_stale_socket(socket_path)?;
    bind(socket_path)
}

/// Whether the file at `socket_path` is the socket this process created.
pub fn is_ours(socket_path: &Path, identity: SocketIdentity) -> bool {
    std::fs::symlink_metadata(socket_path).is_ok_and(|stat| {
        stat.file_type().is_socket() && stat.dev() == identity.dev && stat.ino() == identity.ino
    })
}

/// Sets the access and modification time of a runtime-fallback socket and of its directory to now, so age-based cleaners
/// of `/tmp` leave them alone. An in-project socket is never touched.
pub fn touch_runtime_socket(socket_path: &Path) -> io::Result<()> {
    if !is_runtime_socket(socket_path) {
        return Ok(());
    }
    let mut result = Ok(());
    for path in [Some(socket_path), socket_path.parent()]
        .into_iter()
        .flatten()
    {
        let c_path =
            std::ffi::CString::new(std::os::unix::ffi::OsStrExt::as_bytes(path.as_os_str()))
                .map_err(|_| other("path contains a NUL byte"))?;
        let now = libc::timespec {
            tv_sec: 0,
            tv_nsec: libc::UTIME_NOW,
        };
        let times = [now, now];
        // SAFETY: `c_path` is a NUL terminated string and `times` points at two valid timespec values.
        let done = unsafe {
            libc::utimensat(
                libc::AT_FDCWD,
                c_path.as_ptr(),
                times.as_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if done != 0 && result.is_ok() {
            result = Err(io::Error::last_os_error());
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    fn runtime_base() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("cs")
            .tempdir_in("/tmp")
            .unwrap()
    }

    fn set_old(path: &Path) {
        let c = std::ffi::CString::new(std::os::unix::ffi::OsStrExt::as_bytes(path.as_os_str()))
            .unwrap();
        let old = libc::timespec {
            tv_sec: 1_000_000,
            tv_nsec: 0,
        };
        let times = [old, old];
        // SAFETY: valid C string and two timespec values.
        assert_eq!(
            unsafe {
                libc::utimensat(
                    libc::AT_FDCWD,
                    c.as_ptr(),
                    times.as_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            },
            0
        );
    }

    #[test]
    fn the_freshness_touch_moves_runtime_times_forward_and_skips_project_sockets() {
        let base = runtime_base();
        let dir = base.path().join(capstan_wire::runtime_dir_name());
        prepare_directory(&dir.join("a.sock")).unwrap();
        let path = dir.join("a.sock");
        let (_listener, _) = bind(&path).unwrap();
        set_old(&path);
        set_old(&dir);
        touch_runtime_socket(&path).unwrap();
        assert!(std::fs::symlink_metadata(&path).unwrap().mtime() > 1_000_000);
        assert!(std::fs::symlink_metadata(&dir).unwrap().mtime() > 1_000_000);
        assert!(std::fs::symlink_metadata(&path).unwrap().atime() > 1_000_000);

        let project = base.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let inside = project.join("control.sock");
        let (_other, _) = bind(&inside).unwrap();
        set_old(&inside);
        set_old(&project);
        touch_runtime_socket(&inside).unwrap();
        assert_eq!(
            std::fs::symlink_metadata(&inside).unwrap().mtime(),
            1_000_000
        );
        assert_eq!(
            std::fs::symlink_metadata(&project).unwrap().mtime(),
            1_000_000
        );
    }

    #[test]
    fn a_live_listener_at_the_runtime_path_is_not_removed_and_a_stale_one_is() {
        // The project lock excludes a second daemon of the same project, so only a foreign listener is probed.
        let base = runtime_base();
        let dir = base.path().join(capstan_wire::runtime_dir_name());
        let path = dir.join("b.sock");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        prepare_directory(&path).unwrap();
        let (listener, _) = bind(&path).unwrap();
        let error = rebind(&path).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("another controller listens on"),
            "{error}"
        );
        assert!(path.exists(), "the live listener's socket is untouched");
        drop(listener);
        let (fresh, identity) = rebind(&path).expect("a stale socket is replaced");
        assert!(is_ours(&path, identity));
        drop(fresh);
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "x").unwrap();
        assert!(rebind(&path)
            .unwrap_err()
            .to_string()
            .contains("not a socket"));
        std::fs::remove_file(&path).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let error = prepare_directory(&path).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("not safe for the control socket"),
            "{error}"
        );
    }

    use std::os::unix::fs::PermissionsExt;
}
