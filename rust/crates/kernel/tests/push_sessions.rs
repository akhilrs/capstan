//! Sessions, the group kill and the output caps of the root runner (`GitRepo::root_git`): every child is a session
//! leader, its process group is killed on every exit path, and the leader is always reaped.
//!
//! The checks of `/proc` run on Linux only; the other tests run on every Unix host. Every test-started process has
//! a drop guard, and every "gone" check polls with a 10 s bound.

use capstan_kernel::integrate::{GitRepo, GitSeams, IntegrateError, RootCall};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tempfile::TempDir;

struct Fixture {
    dir: TempDir,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().join("home");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(dir.path().join("repo")).unwrap();
        Fixture { dir, home }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.path(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn seams(&self) -> GitSeams {
        GitSeams {
            forward_env: Some(vec![
                ("HOME".into(), self.home.to_string_lossy().into_owned()),
                ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ]),
            reader_detach: Some(Duration::from_millis(200)),
            ..GitSeams::default()
        }
    }

    /// A repository whose git is `program`, with the version probe skipped.
    fn repo_with(&self, program: &Path) -> GitRepo {
        let seams = GitSeams {
            git_program: Some(program.to_path_buf()),
            skip_version_probe: true,
            ..self.seams()
        };
        GitRepo::new(self.path("repo")).with_seams(seams)
    }

    /// Runs a script that stands for git, with a timeout of `secs` seconds.
    fn run(&self, body: &str, secs: u64) -> Result<capstan_kernel::integrate::RootOutput, String> {
        let program = self.script("fake-git", body);
        let mut c = call();
        c.timeout = Duration::from_secs(secs);
        self.repo_with(&program).root_git(&c).map_err(code_of)
    }
}

fn call() -> RootCall {
    RootCall {
        args: vec!["rev-parse".into(), "HEAD".into()],
        timeout: Duration::from_secs(20),
        config_env: Vec::new(),
        network: false,
        user_config: true,
    }
}

fn code_of(error: IntegrateError) -> String {
    match error {
        IntegrateError::Integration { code, .. } => code,
        other => panic!("not an integration error: {}", other.display()),
    }
}

/// The pid a script wrote to `file`, waited for up to 10 s.
fn read_pid(file: &Path) -> i32 {
    for _ in 0..1000 {
        if let Ok(text) = fs::read_to_string(file) {
            if let Ok(pid) = text.trim().parse() {
                return pid;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("no pid in {}", file.display());
}

/// Kills the process whose pid is in `file` when dropped.
struct PidFileGuard(PathBuf);

impl Drop for PidFileGuard {
    fn drop(&mut self) {
        if let Ok(text) = fs::read_to_string(&self.0) {
            if let Ok(pid) = text.trim().parse::<i32>() {
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
        }
    }
}

/// The fields of /proc/<pid>/stat after the command name: state, ppid, pgrp, session, tty_nr.
fn stat_fields(text: &str) -> Vec<String> {
    let rest = &text[text.rfind(')').expect("a stat line") + 1..];
    rest.split_whitespace().map(str::to_string).collect()
}

#[cfg(target_os = "linux")]
fn state_of(pid: i32) -> Option<char> {
    let text = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat_fields(&text).first().and_then(|s| s.chars().next())
}

/// Waits up to 10 s for `/proc/<pid>` to be absent or a zombie.
#[cfg(target_os = "linux")]
fn assert_gone(pid: i32) {
    for _ in 0..1000 {
        if matches!(state_of(pid), None | Some('Z')) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("process {pid} is still running");
}

/// Waits up to 10 s for `/proc/<pid>` to be absent: the leader was reaped.
#[cfg(target_os = "linux")]
fn assert_reaped(pid: i32) {
    for _ in 0..1000 {
        if !Path::new(&format!("/proc/{pid}")).exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("process {pid} was not reaped");
}

#[cfg(target_os = "linux")]
fn assert_session_leader_without_tty(stat_file: &Path, pid: i32) {
    let fields = stat_fields(&fs::read_to_string(stat_file).unwrap());
    assert_eq!(fields[3], pid.to_string(), "session id is the pid");
    assert_eq!(fields[4], "0", "no controlling terminal");
}

#[test]
fn a_sleeping_child_at_the_timeout_gives_timeout() {
    let fx = Fixture::new();
    let started = Instant::now();
    assert_eq!(fx.run("sleep 30", 1).unwrap_err(), "timeout");
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[cfg(target_os = "linux")]
#[test]
fn the_grandchild_holding_stdout_does_not_outlast_the_timeout() {
    let fx = Fixture::new();
    let pidf = fx.path("gpid");
    let _guard = PidFileGuard(pidf.clone());
    let started = Instant::now();
    let body = format!("sleep 30 &\necho $! > '{}'\nwait", pidf.display());
    assert_eq!(fx.run(&body, 1).unwrap_err(), "timeout");
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_gone(read_pid(&pidf));
}

#[cfg(target_os = "linux")]
#[test]
fn a_leader_that_exits_while_a_grandchild_holds_stdout_is_cleaned_up() {
    let fx = Fixture::new();
    let pidf = fx.path("gpid");
    let _guard = PidFileGuard(pidf.clone());
    let body = format!(
        "sleep 30 &\necho $! > '{}'\necho hi\nexit 0",
        pidf.display()
    );
    let out = fx.run(&body, 20).unwrap();
    assert_eq!(out.code, 0);
    assert_eq!(out.stdout, b"hi\n");
    assert_gone(read_pid(&pidf));
}

#[cfg(target_os = "linux")]
#[test]
fn a_setsid_grandchild_that_keeps_stdout_gives_output_incomplete() {
    let fx = Fixture::new();
    let pidf = fx.path("gpid");
    let _guard = PidFileGuard(pidf.clone());
    let body = format!(
        "setsid sh -c \"echo \\$\\$ > '{}'; exec sleep 30\" &\nwhile [ ! -s '{0}' ]; do sleep 0.05; done\nexit 0",
        pidf.display()
    );
    assert_eq!(fx.run(&body, 20).unwrap_err(), "output_incomplete");
}

#[cfg(target_os = "linux")]
#[test]
fn a_setsid_grandchild_that_keeps_only_stderr_gives_success() {
    let fx = Fixture::new();
    let pidf = fx.path("gpid");
    let _guard = PidFileGuard(pidf.clone());
    let body = format!(
        "setsid sh -c \"echo \\$\\$ > '{}'; exec sleep 30 >/dev/null\" &\nwhile [ ! -s '{0}' ]; do sleep 0.05; done\necho out\nexit 0",
        pidf.display()
    );
    let out = fx.run(&body, 20).unwrap();
    assert_eq!(out.code, 0);
    assert_eq!(out.stdout, b"out\n");
}

#[cfg(target_os = "linux")]
#[test]
fn a_grandchild_holding_no_pipe_is_killed_and_the_output_is_whole() {
    let fx = Fixture::new();
    let pidf = fx.path("gpid");
    let _guard = PidFileGuard(pidf.clone());
    let body = format!(
        "sleep 30 >/dev/null 2>&1 </dev/null &\necho $! > '{}'\necho full\nexit 0",
        pidf.display()
    );
    let out = fx.run(&body, 20).unwrap();
    assert_eq!(out.code, 0);
    assert_eq!(out.stdout, b"full\n");
    assert_gone(read_pid(&pidf));
}

#[test]
fn a_flood_past_the_cap_gives_output_too_large() {
    let fx = Fixture::new();
    let err = fx.run("exec head -c 16777217 /dev/zero", 20).unwrap_err();
    assert_eq!(err, "output_too_large");
}

#[cfg(target_os = "linux")]
#[test]
fn the_child_is_a_session_leader_without_a_terminal() {
    let fx = Fixture::new();
    let stat = fx.path("stat");
    let body = format!("cat /proc/$$/stat > '{}'\necho $$", stat.display());
    let out = fx.run(&body, 20).unwrap();
    let pid: i32 = String::from_utf8(out.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_session_leader_without_tty(&stat, pid);
}

#[cfg(target_os = "linux")]
#[test]
fn a_leader_killed_by_a_signal_gives_killed() {
    let fx = Fixture::new();
    let pidf = fx.path("lpid");
    let _guard = PidFileGuard(pidf.clone());
    let killer = {
        let pidf = pidf.clone();
        std::thread::spawn(move || {
            let pid = read_pid(&pidf);
            for _ in 0..1000 {
                let comm = fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
                if comm.trim() == "sleep" {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        })
    };
    let body = format!("echo $$ > '{}'\nexec sleep 30", pidf.display());
    assert_eq!(fx.run(&body, 20).unwrap_err(), "killed");
    killer.join().unwrap();
}

#[test]
fn an_exit_code_survives_the_wait_and_the_reap() {
    let fx = Fixture::new();
    assert_eq!(fx.run("exit 3", 20).unwrap().code, 3);
    assert_eq!(fx.run("sleep 0.3\nexit 3", 20).unwrap().code, 3);
    assert_eq!(fx.run("exit 0", 20).unwrap().code, 0);
}

#[test]
fn a_just_written_script_is_never_git_unavailable_under_concurrent_forks() {
    let workers: Vec<_> = (0..8)
        .map(|_| {
            std::thread::spawn(|| {
                for _ in 0..50 {
                    let fx = Fixture::new();
                    let out = fx.run("exit 0", 20);
                    assert!(out.is_ok(), "spawn failed: {:?}", out.err());
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
}

#[test]
fn our_own_group_kill_never_gives_killed() {
    let fx = Fixture::new();
    let out = fx.run("sleep 30 &\nexit 3", 20).unwrap();
    assert_eq!(out.code, 3);
    assert_eq!(fx.run("sleep 30", 1).unwrap_err(), "timeout");
    assert_eq!(
        fx.run("exec head -c 16777217 /dev/zero", 20).unwrap_err(),
        "output_too_large"
    );
}

#[test]
fn a_flood_to_stderr_keeps_64_kib_and_does_not_block() {
    let fx = Fixture::new();
    let out = fx
        .run("head -c 1048576 /dev/zero >&2 &\nwait\nexit 0", 20)
        .unwrap();
    assert_eq!(out.code, 0);
    assert_eq!(out.stderr.len(), 64 * 1024);
}

#[test]
fn a_missing_git_program_gives_git_unavailable() {
    let fx = Fixture::new();
    let repo = fx.repo_with(&fx.path("no-such-git"));
    let error = repo.root_git(&call()).unwrap_err();
    assert_eq!(code_of(error), "git_unavailable");
}

#[cfg(target_os = "linux")]
#[test]
fn no_leader_is_left_as_a_zombie() {
    let fx = Fixture::new();
    let pidf = fx.path("lpid");
    let _guard = PidFileGuard(pidf.clone());
    for (body, secs) in [
        ("exit 0", 20),
        ("exec sleep 30", 1),
        ("exec head -c 16777217 /dev/zero", 20),
    ] {
        let _ = fs::remove_file(&pidf);
        let script = format!("echo $$ > '{}'\n{body}", pidf.display());
        let _ = fx.run(&script, secs);
        assert_reaped(read_pid(&pidf));
    }
}

#[cfg(target_os = "linux")]
#[test]
fn a_version_probe_is_a_session_leader_and_its_grandchild_is_gone() {
    let fx = Fixture::new();
    let stat = fx.path("stat");
    let leader = fx.path("lpid");
    let grand = fx.path("gpid");
    let _guard = PidFileGuard(grand.clone());
    let body = format!(
        "if [ \"$1\" = --version ]; then\ncat /proc/$$/stat > '{}'\necho $$ > '{}'\nsleep 30 &\necho $! > '{}'\nwait\nfi\nexit 0",
        stat.display(),
        leader.display(),
        grand.display()
    );
    let program = fx.script("fake-git", &body);
    let seams = GitSeams {
        git_program: Some(program),
        probe_timeout: Some(Duration::from_secs(1)),
        ..fx.seams()
    };
    let repo = GitRepo::new(fx.path("repo")).with_seams(seams);
    let started = Instant::now();
    assert_eq!(code_of(repo.root_git(&call()).unwrap_err()), "timeout");
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_session_leader_without_tty(&stat, read_pid(&leader));
    assert_gone(read_pid(&grand));
}

#[cfg(target_os = "linux")]
#[test]
fn an_ssh_command_that_reads_the_terminal_fails_without_hanging() {
    let fx = Fixture::new();
    let stat = fx.path("stat");
    let ssh = fx.script(
        "ask-ssh",
        &format!(
            "cat /proc/$$/stat > '{}'\nhead -c1 /dev/tty || exit 9\nexit 0",
            stat.display()
        ),
    );
    fs::write(
        fx.home.join(".gitconfig"),
        format!("[core]\n\tsshCommand = {}\n", ssh.display()),
    )
    .unwrap();
    let repo = GitRepo::new(fx.path("repo")).with_seams(GitSeams {
        skip_version_probe: true,
        ..fx.seams()
    });
    let mut c = call();
    c.args = vec!["ls-remote".into(), "ssh://example.invalid/x".into()];
    c.network = true;
    c.timeout = Duration::from_secs(10);
    let started = Instant::now();
    let result = repo.root_git(&c);
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(!matches!(&result, Ok(out) if out.code == 0));
    let fields = stat_fields(&fs::read_to_string(&stat).expect("the ssh command ran"));
    assert_eq!(fields[4], "0", "no controlling terminal");
}

// ------------------------------------------------------------------------------------------ source scan

#[test]
fn spawn_capped_holds_the_session_and_the_wait_loop() {
    let source =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/integrate.rs")).unwrap();
    let lines: Vec<&str> = source.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.contains("fn spawn_capped(") && !l.trim_start().starts_with("//"))
        .expect("spawn_capped exists");
    let indent: String = lines[start].chars().take_while(|c| *c == ' ').collect();
    let close = format!("{indent}}}");
    let end = lines[start + 1..].iter().position(|l| *l == close).unwrap() + start + 1;
    let body = lines[start..=end].join("\n");
    for needle in ["pre_exec", "libc::setsid", "waitid", "libc::kill"] {
        assert!(body.contains(needle), "spawn_capped lacks {needle}");
    }
    assert!(!body.contains("child.kill()") || body.matches("child.kill()").count() == 1);
}
