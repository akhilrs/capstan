//! `cstan controllers [--json]`: every controller daemon running on this machine, found by scanning the user's processes
//! for `cstan daemon` (not a registry, so old builds and leftover processes show up too).
//!
//! The command is read-only. It sends no signal and writes nothing. Each controller is asked one question, `about`
//! (`ping` for a build that does not know it), on the control socket of the project the process runs in. The call
//! carries that project's operator key, read from `.capstan/operator.key` after the same ownership and mode checks the
//! daemon makes. Both files belong to the current user, and the route only reads, so this is the access `cstan status` run
//! in that project would have. The command never looks at `CAPSTAN_SOCKET` or `CAPSTAN_TOKEN`, so it also runs inside an
//! agent's worktree.
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use capstan_client::{call_daemon, Response};
use capstan_daemon::deps::SOCKET_NAME;
use capstan_wire::js::Value;
use serde_json::{json, Value as Json};

use crate::agent::var;
use crate::{output, parse_options, to_js, Cli, Fail, Flow};

/// How long CPU use is sampled.
const SAMPLE: Duration = Duration::from_millis(200);
/// How long one controller may take to answer.
const ASK_TIMEOUT_MS: u64 = 2_000;

/// One running controller process, as the operating system shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Proc {
    pub pid: u32,
    /// The project root: the daemon's working directory.
    pub project: Option<PathBuf>,
    /// The working directory was deleted.
    pub project_gone: bool,
    /// Process start, in seconds since the epoch.
    pub started_at: Option<u64>,
    pub rss_bytes: Option<u64>,
    pub cpu_percent: Option<f64>,
}

/// What a controller said to the question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct About {
    /// The process that answered.
    pub pid: Option<u32>,
    pub version: Option<String>,
    pub active_agents: Option<u64>,
}

/// The outcome of asking the controller of a project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Answered(About),
    /// Nothing listens on the socket, or the answer did not come.
    Silent,
    /// It answered, but did not accept the key.
    Refused,
    /// There is no usable operator key to ask with.
    NoKey,
}

/// The two things the rows need from outside the process list, so a test can supply them.
pub trait Probe {
    fn ledger_exists(&self, project: &Path) -> bool;
    fn ask(&self, project: &Path) -> Reply;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Serving,
    NotAnswering,
    Orphaned,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            State::Serving => "serving",
            State::NotAnswering => "not answering",
            State::Orphaned => "orphaned",
        }
    }
}

/// One line of the table.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub pid: u32,
    pub state: State,
    pub note: Option<String>,
    pub version: Option<String>,
    pub uptime_seconds: Option<u64>,
    pub cpu_percent: Option<f64>,
    pub memory_bytes: Option<u64>,
    pub active_agents: Option<u64>,
    pub project: Option<PathBuf>,
}

/// True when `words` (a command line) starts the controller: `<path>/cstan daemon` or `<path>/cstan-daemon`.
pub fn is_controller_command<S: AsRef<str>>(words: &[S]) -> bool {
    let base = |word: &S| {
        word.as_ref()
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_string()
    };
    match words {
        [program] => base(program) == "cstan-daemon",
        [program, word] => base(program) == "cstan" && word.as_ref() == "daemon",
        _ => false,
    }
}

/// The rows for `procs`, ordered by pid.
pub fn rows(procs: &[Proc], now: u64, probe: &dyn Probe) -> Vec<Row> {
    let mut rows: Vec<Row> = procs.iter().map(|proc| row(proc, now, probe)).collect();
    rows.sort_by_key(|row| row.pid);
    rows
}

fn row(proc: &Proc, now: u64, probe: &dyn Probe) -> Row {
    let mut row = Row {
        pid: proc.pid,
        state: State::NotAnswering,
        note: None,
        version: None,
        uptime_seconds: proc.started_at.map(|start| now.saturating_sub(start)),
        cpu_percent: proc.cpu_percent,
        memory_bytes: proc.rss_bytes,
        active_agents: None,
        project: proc.project.clone(),
    };
    let Some(project) = &proc.project else {
        row.note = Some("(project unknown)".into());
        return row;
    };
    let ledger_gone = proc.project_gone || !probe.ledger_exists(project);
    let reply = if proc.project_gone {
        Reply::Silent
    } else {
        probe.ask(project)
    };
    let answered = match &reply {
        Reply::Answered(about) => Some(about),
        _ => None,
    };
    if ledger_gone {
        row.state = State::Orphaned;
        row.note = Some(if proc.project_gone {
            "(project deleted)".into()
        } else {
            "(ledger deleted)".into()
        });
    } else if let Some(other) = answered
        .and_then(|about| about.pid)
        .filter(|pid| *pid != proc.pid)
    {
        row.state = State::Orphaned;
        row.note = Some(format!("(superseded by {other})"));
        return row;
    } else if answered.is_some() {
        row.state = State::Serving;
    } else {
        row.note = match reply {
            Reply::Refused => Some("(key refused)".into()),
            Reply::NoKey => Some("(no operator key)".into()),
            _ => None,
        };
    }
    if let Some(about) = answered {
        row.version = about.version.clone();
        row.active_agents = about.active_agents;
    }
    row
}

/// CPU use over a sample: the CPU seconds spent while `wall_seconds` passed.
pub fn cpu_percent(before: f64, after: f64, wall_seconds: f64) -> Option<f64> {
    (wall_seconds > 0.0 && after >= before).then(|| (after - before) / wall_seconds * 100.0)
}

/// `3d04h`, `5h12m`, `7m03s`, `42s`.
pub fn format_uptime(seconds: u64) -> String {
    let (days, hours, minutes) = (seconds / 86_400, seconds / 3_600 % 24, seconds / 60 % 60);
    if days > 0 {
        format!("{days}d{hours:02}h")
    } else if hours > 0 {
        format!("{hours}h{minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m{:02}s", seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

/// `812 KB`, `48.2 MB`, `1.3 GB` (powers of 1024).
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 || value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// The path with the home directory written as `~`.
pub fn shorten_home(path: &Path, home: Option<&Path>) -> String {
    match home.map(|home| path.strip_prefix(home)) {
        Some(Ok(rest)) if rest.as_os_str().is_empty() => "~".into(),
        Some(Ok(rest)) => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

/// The table: a header and one line per controller, columns padded with two spaces.
pub fn render_table(rows: &[Row], home: Option<&Path>) -> String {
    if rows.is_empty() {
        return "No controllers are running.\n".into();
    }
    let header = [
        "PID", "STATE", "VERSION", "UPTIME", "CPU", "MEM", "AGENTS", "PROJECT",
    ];
    let mut cells: Vec<[String; 8]> = vec![header.map(String::from)];
    for row in rows {
        let state = match &row.note {
            Some(note) => format!("{} {note}", row.state.label()),
            None => row.state.label().to_string(),
        };
        cells.push([
            row.pid.to_string(),
            state,
            row.version.clone().unwrap_or_else(|| "?".into()),
            row.uptime_seconds.map_or_else(|| "-".into(), format_uptime),
            row.cpu_percent
                .map_or_else(|| "-".into(), |cpu| format!("{cpu:.1}%")),
            row.memory_bytes.map_or_else(|| "-".into(), format_bytes),
            row.active_agents
                .map_or_else(|| "-".into(), |count| count.to_string()),
            row.project
                .as_deref()
                .map_or_else(|| "?".into(), |path| shorten_home(path, home)),
        ]);
    }
    let widths: Vec<usize> = (0..7)
        .map(|at| {
            cells
                .iter()
                .map(|line| line[at].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut text = String::new();
    for line in &cells {
        for (at, width) in widths.iter().enumerate() {
            let _ = write!(text, "{:<width$}  ", line[at]);
        }
        text.push_str(&line[7]);
        text.push('\n');
    }
    text
}

/// The stable JSON of the rows: an array, one object per controller, with the full project path. Unknown values are null.
pub fn rows_json(rows: &[Row]) -> Json {
    Json::Array(
        rows.iter()
            .map(|row| {
                json!({
                    "pid": row.pid,
                    "state": row.state.label(),
                    "note": row.note,
                    "version": row.version,
                    "uptimeSeconds": row.uptime_seconds,
                    "cpuPercent": row.cpu_percent.map(|cpu| (cpu * 10.0).round() / 10.0),
                    "memoryBytes": row.memory_bytes,
                    "activeAgents": row.active_agents,
                    "project": row.project.as_ref().map(|path| path.display().to_string()),
                })
            })
            .collect(),
    )
}

// ---------------------------------------------------------------------------------------------- asking a real controller

/// The operator key of `project`, with the checks the daemon makes of its own files: a private `.capstan` and a
/// regular key file of mode 0600, both owned by the current user.
pub(crate) fn operator_key(project: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    let directory = std::fs::symlink_metadata(project.join(".capstan")).ok()?;
    if !directory.is_dir() || directory.uid() != uid || directory.mode() & 0o077 != 0 {
        return None;
    }
    let path = project.join(".capstan/operator.key");
    let file = std::fs::symlink_metadata(&path).ok()?;
    if !file.is_file() || file.uid() != uid || file.mode() & 0o077 != 0 {
        return None;
    }
    let key = String::from_utf8_lossy(&std::fs::read(&path).ok()?)
        .trim()
        .to_string();
    (key.encode_utf16().count() >= 32).then_some(key)
}

fn number(value: &Value, name: &str) -> Option<u64> {
    match value.get(name) {
        Some(Value::Number(n)) if *n >= 0.0 && n.fract() == 0.0 => Some(*n as u64),
        _ => None,
    }
}

/// The real `Probe`: the ledger file and the control socket of the project.
pub struct Live;

impl Probe for Live {
    fn ledger_exists(&self, project: &Path) -> bool {
        capstan_ledger::resolve_database_path(&project.join(".capstan/state"))
            .is_ok_and(|path| path.exists())
    }

    fn ask(&self, project: &Path) -> Reply {
        let Some(key) = operator_key(project) else {
            return Reply::NoKey;
        };
        let socket = project.join(".capstan/state").join(SOCKET_NAME);
        let call = |command: &str| call_daemon(&socket, &key, command, &[], ASK_TIMEOUT_MS);
        match call("about") {
            Ok(Response::Ok {
                result: Some(result),
            }) => Reply::Answered(About {
                pid: number(&result, "pid").and_then(|pid| u32::try_from(pid).ok()),
                version: match result.get("version") {
                    Some(Value::String(text)) => Some(text.to_utf8_lossy()),
                    _ => None,
                },
                active_agents: number(&result, "activeAgents"),
            }),
            // A build from before `about` refuses the command; `ping` still names its process.
            Ok(Response::Refused { code, .. }) if code.to_utf8_lossy() != "unauthorized" => {
                match call("ping") {
                    Ok(Response::Ok {
                        result: Some(result),
                    }) => Reply::Answered(About {
                        pid: number(&result, "pid").and_then(|pid| u32::try_from(pid).ok()),
                        version: None,
                        active_agents: None,
                    }),
                    _ => Reply::Silent,
                }
            }
            Ok(Response::Refused { .. }) => Reply::Refused,
            _ => Reply::Silent,
        }
    }
}

// ---------------------------------------------------------------------------------------------- finding the processes

/// What the process table gives for one controller.
struct Seen {
    proc: Proc,
    cpu_seconds: Option<f64>,
}

/// The controllers of the current user as the process table shows them now, without the CPU sample.
pub fn processes() -> Vec<Proc> {
    scan_once().into_iter().map(|seen| seen.proc).collect()
}

/// The controllers of the current user, with CPU use measured over `SAMPLE`.
pub fn scan() -> Vec<Proc> {
    let first = scan_once();
    let began = Instant::now();
    std::thread::sleep(SAMPLE);
    let second = scan_once();
    let wall = began.elapsed().as_secs_f64();
    first
        .into_iter()
        .map(|mut seen| {
            let later = second
                .iter()
                .find(|other| other.proc.pid == seen.proc.pid)
                .and_then(|other| other.cpu_seconds);
            seen.proc.cpu_percent = match (seen.cpu_seconds, later) {
                (Some(before), Some(after)) => cpu_percent(before, after, wall),
                _ => None,
            };
            seen.proc
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn scan_once() -> Vec<Seen> {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: getuid and sysconf have no preconditions.
    let (uid, ticks, page) = unsafe {
        (
            libc::getuid(),
            libc::sysconf(libc::_SC_CLK_TCK) as f64,
            libc::sysconf(libc::_SC_PAGESIZE) as u64,
        )
    };
    let boot = std::fs::read_to_string("/proc/stat").ok().and_then(|text| {
        text.lines()
            .find_map(|line| line.strip_prefix("btime ")?.trim().parse::<u64>().ok())
    });
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut seen = Vec::new();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        let dir = entry.path();
        if std::fs::metadata(&dir).map_or(true, |meta| meta.uid() != uid) {
            continue;
        }
        let Ok(raw) = std::fs::read(dir.join("cmdline")) else {
            continue;
        };
        let words: Vec<String> = raw
            .split(|byte| *byte == 0)
            .filter(|word| !word.is_empty())
            .map(|word| String::from_utf8_lossy(word).into_owned())
            .collect();
        if !is_controller_command(&words) {
            continue;
        }
        let (project, project_gone) = match std::fs::read_link(dir.join("cwd")) {
            Ok(path) => match path.to_str().and_then(|p| p.strip_suffix(" (deleted)")) {
                Some(live) => (Some(PathBuf::from(live)), true),
                None => (Some(path), false),
            },
            Err(_) => (None, false),
        };
        let stat = std::fs::read_to_string(dir.join("stat")).unwrap_or_default();
        // Fields after the command name, which is in parentheses and may hold spaces: state is field 3.
        let fields: Vec<&str> = stat
            .rsplit_once(')')
            .map(|(_, rest)| rest.split_whitespace().collect())
            .unwrap_or_default();
        let field = |number: usize| fields.get(number - 3)?.parse::<u64>().ok();
        let cpu_seconds = field(14)
            .zip(field(15))
            .map(|(user, system)| (user + system) as f64 / ticks);
        let started_at = field(22)
            .zip(boot)
            .map(|(start, boot)| boot + (start as f64 / ticks) as u64);
        let rss_bytes = std::fs::read_to_string(dir.join("statm"))
            .ok()
            .and_then(|text| text.split_whitespace().nth(1)?.parse::<u64>().ok())
            .map(|pages| pages * page);
        seen.push(Seen {
            proc: Proc {
                pid,
                project,
                project_gone,
                started_at,
                rss_bytes,
                cpu_percent: None,
            },
            cpu_seconds,
        });
    }
    seen
}

/// `[[dd-]hh:]mm:ss` (ps `etime`) in seconds.
pub fn parse_elapsed(text: &str) -> Option<u64> {
    let (days, clock) = match text.split_once('-') {
        Some((days, clock)) => (days.parse::<u64>().ok()?, clock),
        None => (0, text),
    };
    let mut seconds = 0;
    let parts: Vec<&str> = clock.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    for part in parts {
        seconds = seconds * 60 + part.parse::<u64>().ok()?;
    }
    Some(days * 86_400 + seconds)
}

/// `[hh:]mm:ss[.cc]` (ps `time`) in seconds.
pub fn parse_cpu_time(text: &str) -> Option<f64> {
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut seconds = 0.0;
    for part in parts {
        seconds = seconds * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(seconds)
}

/// One line of `ps -axo pid=,uid=,etime=,rss=,time=,command=`: the pid, owner, process start (from `now`), resident bytes,
/// CPU seconds and the command words.
pub fn parse_ps_line(line: &str, now: u64) -> Option<(u32, u32, u64, u64, f64, Vec<String>)> {
    let mut fields = line.split_whitespace();
    let pid = fields.next()?.parse().ok()?;
    let uid = fields.next()?.parse().ok()?;
    let started = now.saturating_sub(parse_elapsed(fields.next()?)?);
    let rss = fields.next()?.parse::<u64>().ok()? * 1024;
    let cpu = parse_cpu_time(fields.next()?)?;
    let words: Vec<String> = fields.map(String::from).collect();
    Some((pid, uid, started, rss, cpu, words))
}

/// The working directory of `pid`, from `lsof -Fn`'s `n<path>` lines.
pub fn parse_lsof_cwd(text: &str) -> Option<PathBuf> {
    text.lines()
        .find_map(|line| line.strip_prefix('n'))
        .map(PathBuf::from)
}

#[cfg(not(target_os = "linux"))]
fn scan_once() -> Vec<Seen> {
    use std::process::Command;
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let Ok(listing) = Command::new("ps")
        .args(["-axo", "pid=,uid=,etime=,rss=,time=,command="])
        .output()
    else {
        return Vec::new();
    };
    let mut seen = Vec::new();
    for line in String::from_utf8_lossy(&listing.stdout).lines() {
        let Some((pid, owner, started, rss, cpu, words)) = parse_ps_line(line, now) else {
            continue;
        };
        if owner != uid || !is_controller_command(&words) {
            continue;
        }
        let project = Command::new("lsof")
            .args(["-a", "-d", "cwd", "-Fn", "-p", &pid.to_string()])
            .output()
            .ok()
            .and_then(|out| parse_lsof_cwd(&String::from_utf8_lossy(&out.stdout)));
        seen.push(Seen {
            proc: Proc {
                pid,
                project,
                project_gone: false,
                started_at: Some(started),
                rss_bytes: Some(rss),
                cpu_percent: None,
            },
            cpu_seconds: Some(cpu),
        });
    }
    seen
}

impl Cli<'_> {
    /// `cstan controllers [--json]`: handled before any socket guard, so it needs no project and ignores the agent
    /// environment.
    pub fn controllers(&mut self, args: &[String]) -> Result<Flow, Fail> {
        let (positional, json) = parse_options(args);
        if !positional.is_empty() {
            return Err(Fail::Usage);
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let found = rows(&scan(), now, &Live);
        if json {
            output(&mut self.io.out, Some(&to_js(&rows_json(&found))), true);
        } else {
            let home = var(&self.ctx.env, "HOME").map(PathBuf::from);
            self.io
                .out
                .extend_from_slice(render_table(&found, home.as_deref()).as_bytes());
        }
        Ok(Flow::Code(0))
    }
}
