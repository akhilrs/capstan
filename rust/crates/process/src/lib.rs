//! Facts about other processes, read from the operating system: `/proc` on Linux, libproc and sysctl on macOS.
//!
//! Every read is racy by nature (the process may end between two reads), so a process that cannot be read is reported as
//! absent, never as an error. A start time is an opaque string: it is only ever compared with another start time read by
//! this crate on the same machine (equal means the same process; a greater value started later).
use std::path::PathBuf;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as os;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as os;

/// When the process started: clock ticks since boot on Linux (field 22 of `/proc/<pid>/stat`), microseconds since the
/// epoch on macOS. `None` when the process does not exist or cannot be read.
pub fn start_time(pid: i64) -> Option<String> {
    os::start_time(pid)
}

/// The pids of every process this user can see.
pub fn pids() -> Vec<i32> {
    os::pids()
}

/// Whether the environment of `pid` holds exactly `entry` (`NAME=value`). False when it cannot be read, which includes
/// every process of another user.
pub fn has_environment_entry(pid: i32, entry: &str) -> bool {
    os::environment(pid).is_some_and(|environment| {
        environment
            .split(|byte| *byte == 0)
            .any(|item| item == entry.as_bytes())
    })
}

/// The value of the environment variable `name` of `pid`, or `None` when it is unset or cannot be read (see
/// `has_environment_entry`).
pub fn environment_value(pid: i32, name: &str) -> Option<String> {
    let environment = os::environment(pid)?;
    let prefix = format!("{name}=");
    environment
        .split(|byte| *byte == 0)
        .find_map(|item| item.strip_prefix(prefix.as_bytes()))
        .map(|value| String::from_utf8_lossy(value).into_owned())
}

/// The working directory of `pid`, or `None` when it cannot be read.
pub fn cwd(pid: i32) -> Option<PathBuf> {
    os::cwd(pid)
}

/// The executable file `pid` runs, or `None` when it cannot be read.
pub fn executable(pid: i32) -> Option<PathBuf> {
    os::executable(pid)
}

/// Whether `pid` has ended and waits for its parent to collect it.
pub fn is_zombie(pid: i64) -> bool {
    os::is_zombie(pid)
}

/// The short command name of `pid` (what `ps -o comm` shows, at most 15 bytes on Linux and 16 on macOS).
pub fn command_name(pid: i64) -> Option<String> {
    os::command_name(pid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Command, Stdio};

    fn sleeper(cwd: &std::path::Path, env: (&str, &str)) -> Child {
        Command::new("sleep")
            .arg("30")
            .current_dir(cwd)
            .env(env.0, env.1)
            .stdin(Stdio::null())
            .spawn()
            .unwrap()
    }

    /// Waits (bounded) until `ready` holds. On Linux a child that `spawn` returned can still show its parent's name and
    /// environment for a moment, until its exec has replaced them.
    fn eventually(what: &str, ready: impl Fn() -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !ready() {
            assert!(std::time::Instant::now() < deadline, "never: {what}");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn stop(mut child: Child) {
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn this_process_has_a_stable_start_time_and_is_listed() {
        let me = i64::from(std::process::id());
        let first = start_time(me).expect("own start time");
        assert!(first.bytes().all(|b| b.is_ascii_digit()), "{first}");
        assert_eq!(start_time(me), Some(first));
        assert!(pids().contains(&(me as i32)));
        assert!(!is_zombie(me));
    }

    #[test]
    fn a_child_started_later_has_a_later_or_equal_start_time() {
        let dir = std::env::temp_dir();
        let me = start_time(i64::from(std::process::id()))
            .unwrap()
            .parse::<u64>()
            .unwrap();
        let child = sleeper(&dir, ("CAPSTAN_PROCESS_TEST", "1"));
        let theirs = start_time(i64::from(child.id()))
            .unwrap()
            .parse::<u64>()
            .unwrap();
        assert!(theirs >= me, "child {theirs} before parent {me}");
        stop(child);
    }

    #[test]
    fn a_pid_with_no_process_has_no_facts() {
        // Above the largest pid either system hands out.
        let none = 0x7fff_fff0;
        assert_eq!(start_time(i64::from(none)), None);
        assert_eq!(cwd(none), None);
        assert_eq!(executable(none), None);
        assert!(!has_environment_entry(none, "PATH=/bin"));
        assert_eq!(command_name(i64::from(none)), None);
        assert!(!is_zombie(i64::from(none)));
    }

    /// Not a test: the child the environment test reads, this test binary itself, because macOS hides the environment
    /// of Apple's own binaries (`/bin/sleep`) from other processes.
    #[test]
    #[ignore = "a helper process, started by a_child_shows_its_working_directory_executable_and_environment"]
    fn helper_sleeps() {
        std::thread::sleep(std::time::Duration::from_secs(30));
    }

    #[test]
    fn a_child_shows_its_working_directory_executable_and_environment() {
        let dir = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let marker = format!("CAPSTAN_PROCESS_MARKER={}", std::process::id());
        let (name, value) = marker.split_once('=').unwrap();
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "tests::helper_sleeps"])
            .current_dir(&dir)
            .env(name, value)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        eventually("the child carries the marker", || {
            has_environment_entry(pid, &marker)
        });
        assert_eq!(cwd(pid), Some(dir));
        assert_eq!(
            executable(pid).map(|exe| std::fs::canonicalize(exe).unwrap()),
            Some(std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap())
        );
        assert!(!has_environment_entry(pid, &format!("{marker}0")));
        assert_eq!(environment_value(pid, name).as_deref(), Some(value));
        assert_eq!(environment_value(pid, "CAPSTAN_PROCESS_UNSET"), None);
        assert!(!has_environment_entry(pid, "CAPSTAN_PROCESS_UNSET=1"));
        stop(child);
    }

    #[test]
    fn a_child_shows_its_command_name() {
        let child = sleeper(&std::env::temp_dir(), ("CAPSTAN_PROCESS_TEST", "1"));
        let pid = i64::from(child.id());
        eventually("the child runs sleep", || {
            command_name(pid).as_deref() == Some("sleep")
        });
        stop(child);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_hides_the_environment_of_an_apple_binary() {
        let child = sleeper(&std::env::temp_dir(), ("CAPSTAN_PROCESS_HIDDEN", "1"));
        let pid = i64::from(child.id());
        eventually("the child runs sleep", || {
            command_name(pid).as_deref() == Some("sleep")
        });
        assert!(!has_environment_entry(
            child.id() as i32,
            "CAPSTAN_PROCESS_HIDDEN=1"
        ));
        stop(child);
    }

    #[test]
    fn an_ended_child_that_was_not_collected_is_a_zombie() {
        let mut child = Command::new("true").spawn().unwrap();
        let pid = i64::from(child.id());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !is_zombie(pid) {
            assert!(
                std::time::Instant::now() < deadline,
                "never became a zombie"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        child.wait().unwrap();
        assert!(!is_zombie(pid));
    }
}
