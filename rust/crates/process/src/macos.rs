//! libproc (`proc_pidinfo`, `proc_listallpids`) and `sysctl(KERN_PROCARGS2)`.
use std::ffi::CStr;
use std::path::PathBuf;

fn bsd_info(pid: i64) -> Option<libc::proc_bsdinfo> {
    let pid = libc::c_int::try_from(pid).ok().filter(|pid| *pid > 0)?;
    // SAFETY: proc_bsdinfo is plain data; an all-zero value is valid.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: the buffer is a proc_bsdinfo of exactly `size` bytes, the layout PROC_PIDTBSDINFO writes.
    let written =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    (written == size).then_some(info)
}

pub fn start_time(pid: i64) -> Option<String> {
    let info = bsd_info(pid)?;
    Some((info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec).to_string())
}

pub fn pids() -> Vec<i32> {
    // SAFETY: a null buffer asks only for the number of pids.
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if count <= 0 {
        return Vec::new();
    }
    // Room for processes started between the two calls.
    let mut buffer = vec![0 as libc::pid_t; count as usize + 64];
    let bytes = (buffer.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
    // SAFETY: the buffer holds `bytes` bytes of pid_t values.
    let listed = unsafe { libc::proc_listallpids(buffer.as_mut_ptr().cast(), bytes) };
    if listed <= 0 {
        return Vec::new();
    }
    buffer.truncate((listed as usize).min(buffer.len()));
    buffer.retain(|pid| *pid > 0);
    buffer
}

/// The environment block of `pid` as NUL-separated `NAME=value` entries, from the `KERN_PROCARGS2` layout: the argument
/// count, the executable path, NUL padding, the arguments, then the environment.
pub fn environment(pid: i32) -> Option<Vec<u8>> {
    if pid <= 0 {
        return None;
    }
    let mut argmax: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    // SAFETY: reads one c_int into `argmax`, whose size is passed in `size`.
    let read = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            2,
            (&raw mut argmax).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if read != 0 || argmax <= 0 {
        return None;
    }
    let mut buffer = vec![0u8; argmax as usize];
    let mut size = buffer.len();
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    // SAFETY: the kernel writes at most `size` bytes into `buffer` and stores the written length in `size`.
    let read = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if read != 0 {
        return None;
    }
    buffer.truncate(size);
    environment_of_procargs(&buffer)
}

fn environment_of_procargs(block: &[u8]) -> Option<Vec<u8>> {
    let argc = i32::from_ne_bytes(block.get(..4)?.try_into().ok()?);
    let mut rest = &block[4..];
    let skip_string =
        |bytes: &[u8]| -> Option<usize> { Some(bytes.iter().position(|b| *b == 0)? + 1) };
    rest = &rest[skip_string(rest)?..];
    let padding = rest.iter().take_while(|b| **b == 0).count();
    rest = &rest[padding..];
    for _ in 0..argc.max(0) {
        rest = &rest[skip_string(rest)?..];
    }
    let end = rest
        .windows(2)
        .position(|pair| pair == [0, 0])
        .map_or(rest.len(), |index| index + 1);
    Some(rest[..end].to_vec())
}

pub fn cwd(pid: i32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    if pid <= 0 {
        return None;
    }
    // SAFETY: proc_vnodepathinfo is plain data; an all-zero value is valid.
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    // SAFETY: the buffer is a proc_vnodepathinfo of exactly `size` bytes, the layout PROC_PIDVNODEPATHINFO writes.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            (&raw mut info).cast(),
            size,
        )
    };
    if written != size {
        return None;
    }
    // vip_path is a MAXPATHLEN byte buffer, declared as nested arrays; read it as one NUL-terminated string.
    let path = &info.pvi_cdir.vip_path;
    // SAFETY: the nested arrays are one contiguous block of MAXPATHLEN c_chars.
    let bytes = unsafe {
        std::slice::from_raw_parts(path.as_ptr().cast::<u8>(), std::mem::size_of_val(path))
    };
    let path = CStr::from_bytes_until_nul(bytes).ok()?.to_bytes();
    (!path.is_empty()).then(|| PathBuf::from(std::ffi::OsStr::from_bytes(path)))
}

pub fn executable(pid: i32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    if pid <= 0 {
        return None;
    }
    let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer holds PROC_PIDPATHINFO_MAXSIZE bytes; proc_pidpath writes at most that and returns the length.
    let length =
        unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    if length <= 0 {
        return None;
    }
    buffer.truncate(length as usize);
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(&buffer)))
}

/// libproc answers ESRCH for a zombie, while the pid still exists for `kill`; a live process of this user reads.
pub fn is_zombie(pid: i64) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    if let Some(info) = bsd_info(i64::from(pid)) {
        return info.pbi_status == libc::SZOMB;
    }
    let gone = std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
    // SAFETY: signal 0 only checks that the pid exists.
    gone && unsafe { libc::kill(pid, 0) } == 0
}

pub fn command_name(pid: i64) -> Option<String> {
    let info = bsd_info(pid)?;
    // SAFETY: pbi_comm is a fixed c_char array; read it as bytes up to its NUL or its end.
    let bytes = unsafe {
        std::slice::from_raw_parts(info.pbi_comm.as_ptr().cast::<u8>(), info.pbi_comm.len())
    };
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    Some(String::from_utf8_lossy(&bytes[..end]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::environment_of_procargs;

    fn block(argc: i32, parts: &[&[u8]]) -> Vec<u8> {
        let mut bytes = argc.to_ne_bytes().to_vec();
        for part in parts {
            bytes.extend_from_slice(part);
        }
        bytes
    }

    #[test]
    fn the_environment_follows_the_path_padding_and_arguments() {
        let bytes = block(
            2,
            &[
                b"/bin/sleep\0",
                b"\0\0\0",
                b"sleep\0",
                b"30\0",
                b"A=1\0",
                b"B=x y\0",
                b"\0\0",
            ],
        );
        assert_eq!(environment_of_procargs(&bytes).unwrap(), b"A=1\0B=x y\0");
    }

    #[test]
    fn a_truncated_block_has_no_environment() {
        assert_eq!(
            environment_of_procargs(&block(3, &[b"/bin/x\0", b"a\0"])),
            None
        );
        assert_eq!(environment_of_procargs(&[1, 0]), None);
    }

    #[test]
    fn an_empty_environment_is_empty() {
        let bytes = block(1, &[b"/bin/x\0", b"x\0"]);
        assert_eq!(environment_of_procargs(&bytes).unwrap(), b"");
    }
}
