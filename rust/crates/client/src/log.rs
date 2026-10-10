//! The daemon log: opened the way `openDaemonLog` opens it, and its tail for a start failure.

use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

const NOT_PRIVATE: &str =
    "daemon log must be a regular file owned by the current user with mode 0600";

/// Opens `daemon.log` for appending, creating it with mode 0600. An existing file is used only if it is a regular file
/// of the current user with mode 0600, because someone else may have made it; a symlink is refused (`O_NOFOLLOW`).
pub fn open_daemon_log(path: &Path) -> Result<File, String> {
    let base = || {
        let mut options = OpenOptions::new();
        options
            .append(true)
            .custom_flags(libc::O_NOFOLLOW)
            .mode(0o600);
        options
    };
    let (file, created) = match base().create_new(true).open(path) {
        Ok(file) => (file, true),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => (
            base().open(path).map_err(|e| io_text("open", path, &e))?,
            false,
        ),
        Err(error) => return Err(io_text("open", path, &error)),
    };
    // A file just created gets its mode from the umask; an existing file with a wrong mode is refused.
    if created {
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    let meta = file.metadata().map_err(|e| e.to_string())?;
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    if !meta.is_file() || meta.mode() & 0o777 != 0o600 || meta.uid() != uid {
        return Err(NOT_PRIVATE.to_string());
    }
    Ok(file)
}

fn io_text(operation: &str, path: &Path, error: &std::io::Error) -> String {
    let code = match error.raw_os_error() {
        Some(2) => "ENOENT",
        Some(13) => "EACCES",
        Some(20) => "ENOTDIR",
        Some(40) => "ELOOP",
        Some(21) => "EISDIR",
        _ => return error.to_string(),
    };
    format!("{code}: {operation} '{}'", path.display())
}

fn is_format_char(c: char) -> bool {
    matches!(c as u32,
        0xAD | 0x600..=0x605 | 0x61C | 0x6DD | 0x70F | 0x180E | 0x200B..=0x200F | 0x202A..=0x202E
        | 0x2060..=0x2064 | 0x2066..=0x206F | 0xFEFF | 0xFFF9..=0xFFFB | 0xE0001 | 0xE0020..=0xE007F)
}

/// The last three lines of what the daemon wrote to its log since `from_offset` (at most the last 4096 bytes), control and
/// format characters shown as spaces, joined with ` | ` and cut to 400 characters.
pub fn log_tail(path: &Path, from_offset: u64) -> String {
    let read = || -> std::io::Result<String> {
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        let size = file.metadata()?.len();
        let start = from_offset.max(size.saturating_sub(4096));
        let mut bytes = vec![0u8; size.saturating_sub(start) as usize];
        file.seek(SeekFrom::Start(start))?;
        let mut filled = 0;
        while filled < bytes.len() {
            let n = file.read(&mut bytes[filled..])?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        bytes.truncate(filled);
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    };
    let Ok(text) = read() else {
        return "(log unreadable)".to_string();
    };
    let cleaned: String = text
        .chars()
        .map(|c| {
            if c == '\n' {
                '\n'
            } else if c.is_control() || is_format_char(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let lines: Vec<&str> = cleaned.trim_end().split('\n').collect();
    let tail = lines[lines.len().saturating_sub(3)..].join(" | ");
    // JavaScript's slice(-400) counts UTF-16 units.
    let mut units = 0;
    let mut cut = tail.len();
    for (index, c) in tail.char_indices().rev() {
        units += c.len_utf16();
        if units > 400 {
            break;
        }
        cut = index;
    }
    let tail = &tail[cut..];
    if tail.is_empty() {
        "(the daemon wrote nothing)".to_string()
    } else {
        tail.to_string()
    }
}
