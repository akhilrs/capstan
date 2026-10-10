//! The restart helper (src/restart-helper.ts), started detached by the restart coordinator as `<known-good binary>
//! __restart-helper <plan>`. It restarts exactly the daemon command recorded in its plan file (`node` is the daemon
//! binary's path for cstan-daemon, `argv` its arguments), so no selection logic lives here.
//!
//! Order: wait for the old controller to be gone, back the ledger up, start the new build, ping it directly over the
//! control socket. A new build that does not answer is stopped, the binary is renamed to `<binary>.failed-<id>` and the
//! known-good copy takes its place; the ledger backup is restored first when the stored schema is newer than the
//! known-good build knows. At most two start attempts per build, and nothing the helper did not create is deleted.
//!
//! The helper outlives the controller it replaces, so it is single threaded and blocking: it sleeps between polls.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

pub const EXIT_OK: i32 = 0;
pub const EXIT_ROLLED_BACK: i32 = 1;
pub const EXIT_DOWN: i32 = 2;
pub const EXIT_USAGE: i32 = 64;
/// The file name of the saved binary inside the known-good directory (Node's standalone build saves `cstan`).
pub const KNOWN_GOOD_BINARY: &str = "cstan";
/// What a snapshot taken before the upgrade to a single `cstan` holds: the daemon binary. The next snapshot replaces the
/// directory and holds `cstan`.
pub const KNOWN_GOOD_LEGACY_BINARY: &str = "cstan-daemon";

/// The saved binary inside the known-good directory `dir`: `cstan`, else a `cstan-daemon` from before the upgrade.
pub fn known_good_binary(dir: &Path) -> PathBuf {
    let current = dir.join(KNOWN_GOOD_BINARY);
    let legacy = dir.join(KNOWN_GOOD_LEGACY_BINARY);
    if !current.is_file() && legacy.is_file() {
        legacy
    } else {
        current
    }
}
const START_ATTEMPTS: usize = 2;
const LOG_TAIL_CHARS: usize = 600;
const PING_TIMEOUT_MS: u64 = 3_000;
const SETTLE_RECHECK_MS: u64 = 2_000;
const MAX_PING_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RestartTiming {
    /// How long to wait for the old controller to be gone before it is signalled.
    pub handoff_wait_ms: u64,
    /// How long the old controller gets after SIGTERM before SIGKILL.
    pub term_wait_ms: u64,
    /// How long a process group gets to end after a signal.
    pub kill_wait_ms: u64,
    pub poll_ms: u64,
}

pub const DEFAULT_RESTART_TIMING: RestartTiming = RestartTiming {
    handoff_wait_ms: 30_000,
    term_wait_ms: 10_000,
    kill_wait_ms: 5_000,
    poll_ms: 1_000,
};

/// `Partial<RestartTiming>` as the plan file carries it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartialTiming {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff_wait_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub term_wait_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kill_wait_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll_ms: Option<u64>,
}

impl PartialTiming {
    pub fn resolve(&self) -> RestartTiming {
        RestartTiming {
            handoff_wait_ms: self
                .handoff_wait_ms
                .unwrap_or(DEFAULT_RESTART_TIMING.handoff_wait_ms),
            term_wait_ms: self
                .term_wait_ms
                .unwrap_or(DEFAULT_RESTART_TIMING.term_wait_ms),
            kill_wait_ms: self
                .kill_wait_ms
                .unwrap_or(DEFAULT_RESTART_TIMING.kill_wait_ms),
            poll_ms: self.poll_ms.unwrap_or(DEFAULT_RESTART_TIMING.poll_ms),
        }
    }
}

/// `RestartPlan`: the plan file `<state>/restart/<id>/plan.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestartPlan {
    pub id: String,
    /// The program that starts the controller (for cstan-daemon its own executable path).
    pub node: String,
    /// The arguments after it.
    pub argv: Vec<String>,
    pub cwd: String,
    pub state_dir: String,
    pub socket_path: String,
    pub pid_path: String,
    /// The pid of the controller that asked for the restart.
    pub pid: i64,
    /// The binary file the controller runs from.
    pub dist_path: String,
    pub known_good_path: String,
    /// True when `dist_path` is a standalone binary and the known-good copy is the file `cstan-daemon`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary: Option<bool>,
    pub ledger_path: String,
    pub log_path: String,
    pub health_timeout_seconds: u64,
    pub credential_file: String,
    /// Dependency files that differ from the known-good manifest at the time of the proposal.
    #[serde(default)]
    pub deps_changed: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timing: Option<PartialTiming>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestartOutcome {
    Ok,
    RolledBack,
    Down,
}

impl RestartOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::RolledBack => "rolled_back",
            Self::Down => "down",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "ok" => Some(Self::Ok),
            "rolled_back" => Some(Self::RolledBack),
            "down" => Some(Self::Down),
            _ => None,
        }
    }
}

/// `RestartResult`: `result.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestartResult {
    pub outcome: RestartOutcome,
    pub reason: Option<String>,
    pub failed_log_tail: Option<String>,
    pub ledger_restored: Option<bool>,
    pub manual_recovery: Option<String>,
    pub deps_changed: Option<Vec<String>>,
    pub pid: Option<i64>,
}

impl RestartResult {
    pub fn new(outcome: RestartOutcome) -> Self {
        Self {
            outcome,
            reason: None,
            failed_log_tail: None,
            ledger_restored: None,
            manual_recovery: None,
            deps_changed: None,
            pid: None,
        }
    }

    /// The members `JSON.stringify` writes, in the order the helper sets them.
    pub fn to_json(&self) -> Value {
        let mut map = serde_json::Map::new();
        map.insert("outcome".into(), Value::from(self.outcome.as_str()));
        if let Some(reason) = &self.reason {
            map.insert("reason".into(), Value::from(reason.clone()));
        }
        if let Some(tail) = &self.failed_log_tail {
            map.insert("failedLogTail".into(), Value::from(tail.clone()));
        }
        if let Some(restored) = self.ledger_restored {
            map.insert("ledgerRestored".into(), Value::from(restored));
        }
        if let Some(manual) = &self.manual_recovery {
            map.insert("manualRecovery".into(), Value::from(manual.clone()));
        }
        if let Some(deps) = &self.deps_changed {
            map.insert("depsChanged".into(), Value::from(deps.clone()));
        }
        if let Some(pid) = self.pid {
            map.insert("pid".into(), Value::from(pid));
        }
        Value::Object(map)
    }

    /// Reads a `result.json`; `None` when it is not an object with a known outcome.
    pub fn from_json(value: &Value) -> Option<Self> {
        let outcome = RestartOutcome::parse(value.get("outcome")?.as_str()?)?;
        let text = |name: &str| value.get(name).and_then(Value::as_str).map(str::to_string);
        Some(Self {
            outcome,
            reason: text("reason"),
            failed_log_tail: text("failedLogTail"),
            ledger_restored: value.get("ledgerRestored").and_then(Value::as_bool),
            manual_recovery: text("manualRecovery"),
            deps_changed: value.get("depsChanged").and_then(Value::as_array).map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            }),
            pid: value.get("pid").and_then(Value::as_i64),
        })
    }
}

// ------------------------------------------------------------------------------------------------ ping

/// The frame `callDaemon` in src/client.ts sends.
pub fn ping_frame(credential: &str) -> String {
    format!(
        "{}\n",
        serde_json::json!({"v": 1, "credential": credential, "command": "ping", "args": []})
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PingProbe {
    Ok {
        pid: Option<i64>,
    },
    /// Nothing listens (no socket file or a dead listener).
    Refused,
    Failed,
}

/// One direct ping over the control socket.
pub fn direct_ping(socket_path: &Path, credential: &str, timeout_ms: u64) -> PingProbe {
    let timeout = Some(Duration::from_millis(timeout_ms.max(1)));
    let mut stream = match UnixStream::connect(socket_path) {
        Ok(stream) => stream,
        Err(error) => {
            return match error.raw_os_error() {
                Some(libc::ENOENT) | Some(libc::ECONNREFUSED) => PingProbe::Refused,
                _ => PingProbe::Failed,
            }
        }
    };
    let _ = stream.set_read_timeout(timeout);
    let _ = stream.set_write_timeout(timeout);
    if stream.write_all(ping_frame(credential).as_bytes()).is_err() {
        return PingProbe::Failed;
    }
    let started = Instant::now();
    let mut bytes: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(newline) = bytes.iter().position(|b| *b == b'\n') {
            return match serde_json::from_slice::<Value>(&bytes[..newline]) {
                Ok(body) if body.get("ok") == Some(&Value::Bool(true)) => PingProbe::Ok {
                    pid: body
                        .get("result")
                        .and_then(|result| result.get("pid"))
                        .and_then(Value::as_i64),
                },
                _ => PingProbe::Failed,
            };
        }
        if bytes.len() > MAX_PING_BYTES || started.elapsed() > Duration::from_millis(timeout_ms) {
            return PingProbe::Failed;
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return PingProbe::Failed,
            Ok(read) => bytes.extend_from_slice(&chunk[..read]),
        }
    }
}

// ------------------------------------------------------------------------------------------------ processes and files

fn process_alive(pid: i64) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: kill with signal 0 only checks that the process exists.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn group_alive(pgid: i64) -> bool {
    if pgid <= 1 {
        return false;
    }
    // SAFETY: kill with signal 0 only checks that the group exists.
    let result = unsafe { libc::kill(-(pgid as libc::pid_t), 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn signal_process(pid: i64, signal: libc::c_int) {
    if pid > 1 {
        // SAFETY: a plain signal to a pid this helper was told about; a stale pid is the caller's check.
        unsafe { libc::kill(pid as libc::pid_t, signal) };
    }
}

fn signal_group(pgid: i64, signal: libc::c_int) {
    if pgid > 1 {
        // SAFETY: a plain signal to the process group of a child this helper started.
        unsafe { libc::kill(-(pgid as libc::pid_t), signal) };
    }
}

fn wait_until(mut condition: impl FnMut() -> bool, limit_ms: u64, poll_ms: u64) -> bool {
    let deadline = Instant::now() + Duration::from_millis(limit_ms);
    loop {
        if condition() {
            return true;
        }
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        let left = (deadline - now).as_millis().max(1) as u64;
        std::thread::sleep(Duration::from_millis(poll_ms.min(left)));
    }
}

fn read_pid_file(pid_path: &Path) -> Option<i64> {
    let text = fs::read_to_string(pid_path).ok()?;
    let digits: String = text
        .trim()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse::<i64>().ok().filter(|pid| *pid > 0)
}

/// A pid file that is gone, or names a dead process, no longer blocks a start.
fn pid_file_free(pid_path: &Path) -> bool {
    if !pid_path.exists() {
        return true;
    }
    match read_pid_file(pid_path) {
        None => true,
        Some(pid) => !process_alive(pid),
    }
}

fn write_file_atomic(target: &Path, text: &str) -> std::io::Result<()> {
    let mut temporary = target.as_os_str().to_os_string();
    temporary.push(format!(".tmp-{}", std::process::id()));
    let temporary = PathBuf::from(temporary);
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(text.as_bytes())?;
    drop(file);
    fs::rename(&temporary, target)
}

/// `/[\p{Cc}\p{Cf}]/gu` replaced by a space (newline kept).
fn is_format_char(c: char) -> bool {
    matches!(c as u32,
        0xad | 0x600..=0x605 | 0x61c | 0x6dd | 0x70f | 0x890..=0x891 | 0x8e2 | 0x180e
        | 0x200b..=0x200f | 0x202a..=0x202e | 0x2060..=0x2064 | 0x2066..=0x206f | 0xfeff
        | 0xfff9..=0xfffb | 0x110bd | 0x110cd | 0x13430..=0x1343f | 0x1bca0..=0x1bca3
        | 0x1d173..=0x1d17a | 0xe0001 | 0xe0020..=0xe007f)
}

fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c == '\n' {
                '\n'
            } else if c.is_control() || is_format_char(c) {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// The last `max` UTF-16 units of `text`, as `text.slice(-max)`.
pub fn tail_units(text: &str, max: usize) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.len() <= max {
        return text.to_string();
    }
    String::from_utf16_lossy(&units[units.len() - max..])
}

fn log_tail_from(log_path: &Path, from_offset: u64) -> String {
    let read = || -> std::io::Result<String> {
        let mut file = File::open(log_path)?;
        let size = file.metadata()?.len();
        let start = from_offset.max(size.saturating_sub((LOG_TAIL_CHARS * 2) as u64));
        file.seek(SeekFrom::Start(start))?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)?;
        let text = sanitize(&String::from_utf8_lossy(&buffer));
        Ok(tail_units(text.trim_end(), LOG_TAIL_CHARS))
    };
    read().unwrap_or_default()
}

const LEDGER_SUFFIXES: [&str; 3] = ["", "-wal", "-shm"];

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut text = path.as_os_str().to_os_string();
    text.push(suffix);
    PathBuf::from(text)
}

/// Copies the database and its -wal and -shm files; the old controller has exited, so they are stable.
pub fn backup_ledger(ledger_path: &Path, backup_dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(backup_dir)?;
    fs::set_permissions(backup_dir, fs::Permissions::from_mode(0o700)).ok();
    for suffix in LEDGER_SUFFIXES {
        let source = with_suffix(ledger_path, suffix);
        if source.exists() {
            let name = source.file_name().expect("a ledger file has a name");
            fs::copy(&source, backup_dir.join(name))?;
        }
    }
    Ok(())
}

/// Puts the backup back, replacing the live files, so no -wal of a newer schema stays beside an older database.
pub fn restore_ledger(ledger_path: &Path, backup_dir: &Path) -> std::io::Result<()> {
    for suffix in LEDGER_SUFFIXES {
        let target = with_suffix(ledger_path, suffix);
        let name = target.file_name().expect("a ledger file has a name");
        let backup = backup_dir.join(name);
        if backup.exists() {
            let temporary = with_suffix(&target, ".restoring");
            fs::copy(&backup, &temporary)?;
            fs::rename(&temporary, &target)?;
        } else if let Err(error) = fs::remove_file(&target) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(error);
            }
        }
    }
    Ok(())
}

/// The highest migration the stored ledger records, or `None` when it cannot be read.
pub fn stored_schema_version(ledger_path: &Path) -> Option<i64> {
    let database = capstan_ledger::open_database_read_only(ledger_path).ok()?;
    let version: Option<i64> = database
        .connection()
        .query_row(
            "SELECT MAX(version) AS version FROM schema_migrations",
            [],
            |row| row.get(0),
        )
        .ok()?;
    Some(version.unwrap_or(0))
}

fn read_manifest_max_migration(known_good_path: &Path) -> Option<i64> {
    let text = fs::read_to_string(known_good_path.join("manifest.json")).ok()?;
    let parsed: Value = serde_json::from_str(&text).ok()?;
    let number = parsed.get("maxMigration")?;
    number
        .as_i64()
        .or_else(|| number.as_f64().map(|n| n as i64))
}

fn signal_name(signal: i32) -> String {
    match signal {
        libc::SIGHUP => "SIGHUP".into(),
        libc::SIGINT => "SIGINT".into(),
        libc::SIGQUIT => "SIGQUIT".into(),
        libc::SIGILL => "SIGILL".into(),
        libc::SIGABRT => "SIGABRT".into(),
        libc::SIGBUS => "SIGBUS".into(),
        libc::SIGFPE => "SIGFPE".into(),
        libc::SIGKILL => "SIGKILL".into(),
        libc::SIGSEGV => "SIGSEGV".into(),
        libc::SIGPIPE => "SIGPIPE".into(),
        libc::SIGALRM => "SIGALRM".into(),
        libc::SIGTERM => "SIGTERM".into(),
        other => format!("SIG{other}"),
    }
}

fn exit_text(status: &ExitStatus) -> String {
    match status.signal() {
        Some(signal) => signal_name(signal),
        None => format!("exit code {}", status.code().unwrap_or(0)),
    }
}

// ------------------------------------------------------------------------------------------------ the helper

struct Started {
    child: Option<Child>,
    spawn_error: Option<String>,
    log_offset: u64,
}

struct Helper<'a> {
    plan: &'a RestartPlan,
    timing: RestartTiming,
    credential: String,
    backup_dir: PathBuf,
    failed_dist: PathBuf,
    log: &'a dyn Fn(&str),
}

impl<'a> Helper<'a> {
    fn new(
        plan: &'a RestartPlan,
        restart_dir: &Path,
        log: &'a dyn Fn(&str),
    ) -> std::io::Result<Self> {
        let timing = plan.timing.unwrap_or_default().resolve();
        let credential = fs::read_to_string(&plan.credential_file)?
            .trim()
            .to_string();
        let failed_dist = if plan.binary == Some(true) {
            PathBuf::from(format!("{}.failed-{}", plan.dist_path, plan.id))
        } else {
            let dist = Path::new(&plan.dist_path);
            dist.parent()
                .unwrap_or_else(|| Path::new("."))
                .join(format!("dist.failed-{}", plan.id))
        };
        Ok(Self {
            plan,
            timing,
            credential,
            backup_dir: restart_dir.join("ledger.bak"),
            failed_dist,
            log,
        })
    }

    fn socket_refuses(&self) -> bool {
        direct_ping(
            Path::new(&self.plan.socket_path),
            &self.credential,
            PING_TIMEOUT_MS,
        ) == PingProbe::Refused
    }

    /// The old controller is gone when its pid, its socket and its pid file are. Returns a reason on failure.
    fn handoff(&self) -> Option<String> {
        let plan = self.plan;
        let timing = self.timing;
        let pid_path = Path::new(&plan.pid_path);
        let settled =
            || !process_alive(plan.pid) && self.socket_refuses() && pid_file_free(pid_path);
        if wait_until(settled, timing.handoff_wait_ms, timing.poll_ms) {
            self.clear_own_pid_file();
            return None;
        }
        if process_alive(plan.pid) {
            (self.log)(&format!("old controller {} still alive: SIGTERM", plan.pid));
            signal_process(plan.pid, libc::SIGTERM);
            if !wait_until(
                || !process_alive(plan.pid),
                timing.term_wait_ms,
                timing.poll_ms.min(100),
            ) {
                (self.log)(&format!(
                    "old controller {} ignored SIGTERM: SIGKILL",
                    plan.pid
                ));
                signal_process(plan.pid, libc::SIGKILL);
                wait_until(
                    || !process_alive(plan.pid),
                    timing.kill_wait_ms,
                    timing.poll_ms.min(100),
                );
            }
        }
        if process_alive(plan.pid) {
            return Some(format!(
                "the old controller (pid {}) did not exit",
                plan.pid
            ));
        }
        self.clear_own_pid_file();
        if !wait_until(settled, SETTLE_RECHECK_MS, 100) {
            return Some("another process still holds the control socket or the pid file".into());
        }
        None
    }

    /// Removes the pid file only when it names the old, dead pid; another live pid's file is never touched.
    fn clear_own_pid_file(&self) {
        let pid_path = Path::new(&self.plan.pid_path);
        if read_pid_file(pid_path) == Some(self.plan.pid) && !process_alive(self.plan.pid) {
            let _ = fs::remove_file(pid_path);
        }
    }

    fn start(&self) -> Started {
        let plan = self.plan;
        let log_path = Path::new(&plan.log_path);
        let log_offset = fs::metadata(log_path).map(|m| m.len()).unwrap_or(0);
        let open = || -> std::io::Result<(File, File)> {
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(log_path)?;
            let second = file.try_clone()?;
            Ok((file, second))
        };
        let (out, err) = match open() {
            Ok(files) => files,
            Err(error) => {
                return Started {
                    child: None,
                    spawn_error: Some(format!("spawn failed: {error}")),
                    log_offset,
                }
            }
        };
        let mut command = Command::new(&plan.node);
        command
            .args(&plan.argv)
            .current_dir(&plan.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err))
            .env_remove("CAPSTAN_TOKEN")
            .env_remove("CAPSTAN_SOCKET")
            .process_group(0);
        match command.spawn() {
            Ok(child) => Started {
                child: Some(child),
                spawn_error: None,
                log_offset,
            },
            Err(error) => Started {
                child: None,
                spawn_error: Some(format!("spawn failed: {error}")),
                log_offset,
            },
        }
    }

    /// Polls until the controller answers ping, exits, or the timeout passes. Returns a reason when unhealthy.
    fn health(&self, started: &mut Started) -> Result<i64, String> {
        let plan = self.plan;
        let deadline = Instant::now() + Duration::from_secs(plan.health_timeout_seconds);
        loop {
            if let Some(error) = &started.spawn_error {
                return Err(format!(
                    "the controller exited right after it started ({error})"
                ));
            }
            let child = started.child.as_mut().expect("a started child");
            if let Ok(Some(status)) = child.try_wait() {
                return Err(format!(
                    "the controller exited right after it started ({})",
                    exit_text(&status)
                ));
            }
            let child_pid = i64::from(child.id());
            match direct_ping(
                Path::new(&plan.socket_path),
                &self.credential,
                PING_TIMEOUT_MS,
            ) {
                PingProbe::Ok { pid } => {
                    if let Some(answered) = pid {
                        if answered != child_pid {
                            return Err(format!(
                                "a different process (pid {answered}) answered ping instead of the new controller (pid {child_pid})"
                            ));
                        }
                    }
                    return Ok(child_pid);
                }
                PingProbe::Refused | PingProbe::Failed => {}
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(format!(
                    "the controller did not answer ping within {} seconds",
                    plan.health_timeout_seconds
                ));
            }
            let left = (deadline - now).as_millis().max(1) as u64;
            std::thread::sleep(Duration::from_millis(self.timing.poll_ms.min(left)));
        }
    }

    /// Starts the controller and waits for it to be healthy. A failed attempt stops the process group it started.
    fn start_and_check(&self) -> Result<i64, (String, String)> {
        let mut started = self.start();
        match self.health(&mut started) {
            Ok(pid) => Ok(pid),
            Err(reason) => {
                let log_tail = log_tail_from(Path::new(&self.plan.log_path), started.log_offset);
                if let Some(child) = started.child.as_mut() {
                    self.stop_group(child);
                }
                Err((reason, log_tail))
            }
        }
    }

    fn stop_group(&self, child: &mut Child) {
        let pgid = i64::from(child.id());
        let timing = self.timing;
        for (signal, limit) in [
            (libc::SIGTERM, timing.term_wait_ms),
            (libc::SIGKILL, timing.kill_wait_ms),
        ] {
            // The leader is this process's child: reap it, or a zombie keeps the group "alive".
            let gone = |child: &mut Child| {
                let _ = child.try_wait();
                !group_alive(pgid)
            };
            if gone(child) {
                return;
            }
            signal_group(pgid, signal);
            if wait_until(|| gone(child), limit, timing.poll_ms.min(100)) {
                return;
            }
        }
    }

    /// The binary -> `<binary>.failed-<id>`, then a complete copy of the known-good binary renamed into place: a mode
    /// 0755 file next to the binary, so the rename is atomic and a binary that is running is never written to.
    fn restore_known_good(&self) -> Option<String> {
        let plan = self.plan;
        if plan.binary != Some(true) {
            return Some(
                "restoring the known-good build failed: this helper restarts a standalone binary only"
                    .into(),
            );
        }
        let source = known_good_binary(Path::new(&plan.known_good_path));
        if !source.exists() {
            return Some(format!(
                "the known-good build {} is missing",
                source.display()
            ));
        }
        let dist = PathBuf::from(&plan.dist_path);
        let staging = PathBuf::from(format!("{}.restoring-{}", plan.dist_path, plan.id));
        let mut renamed_failed = false;
        let attempt = || -> std::io::Result<bool> {
            let mut renamed = false;
            if dist.exists() {
                fs::rename(&dist, &self.failed_dist)?;
                renamed = true;
            }
            let _ = fs::remove_file(&staging);
            fs::copy(&source, &staging)?;
            fs::set_permissions(&staging, fs::Permissions::from_mode(0o755))?;
            fs::rename(&staging, &dist)?;
            Ok(renamed)
        };
        // `attempt` cannot report how far it got when it fails, so look at the files afterwards.
        match attempt() {
            Ok(_) => None,
            Err(error) => {
                let _ = fs::remove_file(&staging);
                if self.failed_dist.exists() {
                    renamed_failed = true;
                }
                if renamed_failed && !dist.exists() {
                    let _ = fs::rename(&self.failed_dist, &dist);
                }
                Some(format!("restoring the known-good build failed: {error}"))
            }
        }
    }

    fn manual_recovery(&self) -> String {
        let plan = self.plan;
        let saved = known_good_binary(Path::new(&plan.known_good_path));
        [
            format!("Run these from the project root ({}):", plan.cwd),
            format!(
                "1. Put the saved binary back: cp {saved} {dist}.new && chmod 755 {dist}.new && mv {dist}.new {dist} (the failed binary is kept as {failed}).",
                saved = saved.display(),
                dist = plan.dist_path,
                failed = self.failed_dist.display()
            ),
            format!(
                "2. If the controller still refuses the ledger, restore the backup in {} over {} (with its -wal and -shm files).",
                self.backup_dir.display(),
                plan.ledger_path
            ),
            "3. Start the controller with: cstan start".to_string(),
        ]
        .join("\n")
    }
}

fn write_result(restart_dir: &Path, result: &RestartResult) -> i32 {
    let text = format!("{}\n", result.to_json());
    if let Err(error) = write_file_atomic(&restart_dir.join("result.json"), &text) {
        eprintln!("restart helper failed: {error}");
        return EXIT_DOWN;
    }
    match result.outcome {
        RestartOutcome::Ok => EXIT_OK,
        RestartOutcome::RolledBack => EXIT_ROLLED_BACK,
        RestartOutcome::Down => EXIT_DOWN,
    }
}

/// Runs one restart from its plan file and returns the exit code: 0 ok, 1 rolled back, 2 down.
pub fn run_helper(plan_path: &Path, log: &dyn Fn(&str)) -> i32 {
    let restart_dir = plan_path.parent().unwrap_or_else(|| Path::new("."));
    let unreadable = |error: String| {
        // The controller is already stopping: say so in a result instead of leaving the run open.
        let mut result = RestartResult::new(RestartOutcome::Down);
        result.reason = Some(format!(
            "the restart helper could not read its plan or the operator key ({error}); the controller was stopped and is NOT running"
        ));
        result.manual_recovery = Some(
            [
                "Run these from the project root:",
                "1. Check the daemon binary (the restart changed nothing before this failure) and, if needed, restore it from .capstan/state/known-good/cstan.",
                "2. Start the controller with: cstan start",
            ]
            .join("\n"),
        );
        write_result(restart_dir, &result)
    };
    let plan = match fs::read_to_string(plan_path)
        .map_err(|error| error.to_string())
        .and_then(|text| {
            serde_json::from_str::<RestartPlan>(&text).map_err(|error| error.to_string())
        }) {
        Ok(plan) => plan,
        Err(error) => return unreadable(error),
    };
    let helper = match Helper::new(&plan, restart_dir, log) {
        Ok(helper) => helper,
        Err(error) => return unreadable(error.to_string()),
    };
    run_steps(&plan, &helper, restart_dir, log)
}

fn run_steps(
    plan: &RestartPlan,
    helper: &Helper<'_>,
    restart_dir: &Path,
    log: &dyn Fn(&str),
) -> i32 {
    let down = |reason: String, extra: &dyn Fn(&mut RestartResult)| -> i32 {
        let mut result = RestartResult::new(RestartOutcome::Down);
        result.reason = Some(reason);
        result.manual_recovery = Some(helper.manual_recovery());
        if !plan.deps_changed.is_empty() {
            result.deps_changed = Some(plan.deps_changed.clone());
        }
        extra(&mut result);
        write_result(restart_dir, &result)
    };
    let nothing = |_: &mut RestartResult| {};

    if let Some(refused) = helper.handoff() {
        log(&format!("handoff failed: {refused}"));
        // The handoff may already have signalled the old controller: only a controller that still answers ping counts
        // as "nothing changed".
        let serving = direct_ping(
            Path::new(&plan.socket_path),
            &helper.credential,
            PING_TIMEOUT_MS,
        );
        return match serving {
            PingProbe::Ok { pid } if pid == Some(plan.pid) => {
                let mut result = RestartResult::new(RestartOutcome::RolledBack);
                result.reason = Some(format!(
                    "the restart did not start the new build and the old controller (pid {}) still answers ping: {refused}",
                    plan.pid
                ));
                write_result(restart_dir, &result)
            }
            PingProbe::Ok { pid } => down(
                format!(
                    "the restart stopped before the new build started ({refused}), but a controller other than the old one (pid {}) answers ping{}: check which build it runs",
                    plan.pid,
                    match pid {
                        Some(pid) => format!(" (pid {pid})"),
                        None => String::new(),
                    }
                ),
                &nothing,
            ),
            _ => down(
                format!(
                    "the restart stopped before the new build started ({refused}) and no controller answers ping: the controller is NOT running"
                ),
                &nothing,
            ),
        };
    }

    let failure: (String, String);
    match backup_ledger(Path::new(&plan.ledger_path), &helper.backup_dir) {
        Err(error) => {
            failure = (
                format!(
                    "the ledger could not be backed up ({error}); the new build was not started"
                ),
                String::new(),
            );
        }
        Ok(()) => {
            log("starting the new build");
            match helper.start_and_check() {
                Ok(pid) => {
                    log(&format!("the new controller answers ping (pid {pid})"));
                    let mut result = RestartResult::new(RestartOutcome::Ok);
                    result.pid = Some(pid);
                    return write_result(restart_dir, &result);
                }
                Err(first) => failure = first,
            }
        }
    }
    log(&format!("new build failed: {}", failure.0));

    if let Some(restored) = helper.restore_known_good() {
        return down(restored, &nothing);
    }

    let mut ledger_restored = false;
    if let Some(max_migration) = read_manifest_max_migration(Path::new(&plan.known_good_path)) {
        if let Some(stored) = stored_schema_version(Path::new(&plan.ledger_path)) {
            if stored > max_migration {
                match restore_ledger(Path::new(&plan.ledger_path), &helper.backup_dir) {
                    Ok(()) => {
                        ledger_restored = true;
                        log(&format!(
                            "ledger schema {stored} is newer than the known-good build ({max_migration}): ledger restored from backup"
                        ));
                    }
                    Err(error) => {
                        return down(
                            format!(
                                "the ledger schema {stored} is newer than the known-good build and restoring the backup failed: {error}"
                            ),
                            &nothing,
                        )
                    }
                }
            }
        }
    }

    let mut last = "no start attempt was made".to_string();
    for attempt in 1..=START_ATTEMPTS {
        log(&format!(
            "starting the known-good build (attempt {attempt})"
        ));
        match helper.start_and_check() {
            Ok(pid) => {
                let mut result = RestartResult::new(RestartOutcome::RolledBack);
                result.reason = Some(failure.0.clone());
                result.failed_log_tail = Some(failure.1.clone());
                result.ledger_restored = Some(ledger_restored);
                if !plan.deps_changed.is_empty() {
                    result.deps_changed = Some(plan.deps_changed.clone());
                }
                result.pid = Some(pid);
                return write_result(restart_dir, &result);
            }
            Err((reason, _)) => last = reason,
        }
    }
    let dependency_note = if plan.deps_changed.is_empty() {
        String::new()
    } else {
        format!(
            " Dependencies changed since the known-good build ({}), which can stop the old build from starting.",
            plan.deps_changed.join(", ")
        )
    };
    down(
        format!(
            "the new build failed ({}) and the known-good build did not start either ({last}).{dependency_note}",
            failure.0
        ),
        &|result: &mut RestartResult| {
            result.failed_log_tail = Some(failure.1.clone());
            result.ledger_restored = Some(ledger_restored);
        },
    )
}

/// Runs the helper for the plan file in `args` (the arguments after `__restart-helper`) and returns its exit code: 0 the
/// new build answered, 1 rolled back, 2 down.
pub fn run(args: &[String]) -> i32 {
    let Some(plan_path) = args.first() else {
        eprintln!("usage: cstan __restart-helper <plan.json>");
        return EXIT_USAGE;
    };
    let log = |message: &str| {
        let line = format!("{} {message}\n", capstan_ledger::now_iso());
        let _ = std::io::stdout().write_all(line.as_bytes());
        let _ = std::io::stdout().flush();
    };
    run_helper(Path::new(plan_path), &log)
}
