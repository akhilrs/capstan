//! `/proc`.
use std::path::PathBuf;

/// The fields of `/proc/<pid>/stat` after the command name, which is in parentheses and may contain spaces or
/// parentheses itself; the first field is the state.
fn stat_fields_after_name(pid: i64) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    Some(stat.get(stat.rfind(')')? + 2..)?.to_string())
}

pub fn start_time(pid: i64) -> Option<String> {
    let after = stat_fields_after_name(pid)?;
    let start = after.split(' ').nth(19)?;
    (!start.is_empty() && start.bytes().all(|b| b.is_ascii_digit())).then(|| start.to_string())
}

pub fn pids() -> Vec<i32> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<i32>().ok())
        .collect()
}

pub fn environment(pid: i32) -> Option<Vec<u8>> {
    std::fs::read(format!("/proc/{pid}/environ")).ok()
}

pub fn cwd(pid: i32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

pub fn executable(pid: i32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

pub fn is_zombie(pid: i64) -> bool {
    stat_fields_after_name(pid).is_some_and(|after| after.starts_with('Z'))
}

pub fn command_name(pid: i64) -> Option<String> {
    let name = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    Some(name.trim_end_matches('\n').to_string())
}
