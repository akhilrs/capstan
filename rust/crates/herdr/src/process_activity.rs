//! Tells whether a tool command started by an agent (a test run, a build) is using CPU, so a long silent command is not
//! reported as a stalled agent (src/herdr/process-activity.ts). The probe reads the pane's shell pid from Herdr and the
//! process table from the operating system; the tracker is pure and keeps the last CPU activity.

use crate::adapter::validate::js_trim;
use crate::api::{self, HerdrError, HerdrRunner, RunOptions};
use crate::naming::is_js_space;
use crate::runner::run_json;
use crate::screen::js_number;
use std::collections::{HashMap, HashSet};
use std::io;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The least summed CPU increase between two samples that counts as activity.
pub const MIN_CHILD_CPU_MS: f64 = 200.0;

/// How long after a failed read the walker lets the caller read the whole table before it tries `children` lists again.
pub const CHILDREN_RETRY_MS: u64 = 60_000;

/// One process, as the sampler reads it (`ProcessEntry` of Node; `api::ProcessEntry` is the narrow view of it).
#[derive(Clone, Debug, PartialEq)]
pub struct ProcEntry {
    pub pid: i64,
    pub ppid: i64,
    pub comm: String,
    pub cpu_ms: f64,
    /// Tells a reused pid from the same process: the start time on Linux, the command name elsewhere.
    pub start_key: String,
}

pub type ProcessTable = Vec<ProcEntry>;

const SHELLS: [&str; 6] = ["sh", "bash", "zsh", "dash", "fish", "ksh"];

fn shell_name(comm: &str) -> &str {
    let base = comm.rsplit('/').next().unwrap_or(comm);
    base.strip_prefix('-').unwrap_or(base)
}

fn is_shell(comm: &str) -> bool {
    SHELLS.contains(&shell_name(comm))
}

/// Processes counted under `shell_pid`: a depth-2 descendant (a child of the host process) that is a shell, plus all of
/// its descendants. Other depth-2 children (MCP servers) are not counted.
pub fn tool_processes(table: &[ProcEntry], shell_pid: i64) -> Vec<ProcEntry> {
    let mut children: HashMap<i64, Vec<&ProcEntry>> = HashMap::new();
    for entry in table {
        children.entry(entry.ppid).or_default().push(entry);
    }
    let mut counted: Vec<ProcEntry> = Vec::new();
    let mut seen: HashSet<i64> = HashSet::from([shell_pid]);
    fn walk<'a>(
        entry: &'a ProcEntry,
        children: &HashMap<i64, Vec<&'a ProcEntry>>,
        seen: &mut HashSet<i64>,
        counted: &mut Vec<ProcEntry>,
    ) {
        if !seen.insert(entry.pid) {
            return;
        }
        counted.push(entry.clone());
        if let Some(list) = children.get(&entry.pid) {
            for child in list {
                walk(child, children, seen, counted);
            }
        }
    }
    if let Some(hosts) = children.get(&shell_pid) {
        for host in hosts {
            if let Some(tools) = children.get(&host.pid) {
                for tool in tools {
                    if is_shell(&tool.comm) {
                        walk(tool, &children, &mut seen, &mut counted);
                    }
                }
            }
        }
    }
    counted
}

/// `Math.round`: halves go up.
fn js_round(value: f64) -> f64 {
    if (value - value.floor()) == 0.5 {
        value.ceil()
    } else {
        value.round()
    }
}

/// `text.split(/\s+/)` (after a trim): the fields of the rest of a stat line.
fn split_fields(text: &str) -> Vec<&str> {
    js_trim(text)
        .split(is_js_space)
        .collect::<Vec<_>>()
        .into_iter()
        .fold(Vec::new(), |mut fields, piece| {
            // Runs of white space are one separator; an empty piece only remains when the whole text is empty.
            if !piece.is_empty() || fields.is_empty() {
                fields.push(piece);
            }
            fields
        })
}

/// Parses the content of /proc/<pid>/stat; the command name sits in parentheses and may contain spaces and
/// parentheses.
pub fn parse_proc_stat(pid: i64, text: &str, ticks_per_second: f64) -> Option<ProcEntry> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    if close < open {
        return None;
    }
    let comm = &text[open + 1..close];
    let fields = split_fields(&text[close + 1..]);
    // fields[0] is the state (field 3 of stat): ppid = 1, utime = 11, stime = 12, starttime = 19.
    let field = |index: usize| fields.get(index).map_or(f64::NAN, |f| js_number(f));
    let ppid = field(1);
    let utime = field(11);
    let stime = field(12);
    // Children that already exited and were waited for: a test runner's short-lived test processes.
    let cutime = field(13);
    let cstime = field(14);
    let start = fields.get(19)?;
    let integer = ppid.is_finite() && ppid.fract() == 0.0;
    if !integer
        || !utime.is_finite()
        || !stime.is_finite()
        || !cutime.is_finite()
        || !cstime.is_finite()
    {
        return None;
    }
    Some(ProcEntry {
        pid,
        ppid: ppid as i64,
        comm: comm.to_string(),
        cpu_ms: js_round(((utime + stime + cutime + cstime) * 1000.0) / ticks_per_second),
        start_key: (*start).to_string(),
    })
}

/// Reads /proc once.
pub fn read_proc_table() -> io::Result<ProcessTable> {
    let mut entries = Vec::new();
    for item in std::fs::read_dir("/proc")? {
        let Ok(item) = item else { continue };
        let name = item.file_name().to_string_lossy().into_owned();
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(pid) = name.parse::<i64>() else {
            continue;
        };
        // The process may exit between the listing and the read.
        let Ok(bytes) = std::fs::read(format!("/proc/{name}/stat")) else {
            continue;
        };
        if let Some(entry) = parse_proc_stat(pid, &String::from_utf8_lossy(&bytes), 100.0) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

/// The reads the walker makes of `/proc`; tests replace them.
pub trait ProcIo: Send + Sync {
    /// Thread ids of a process; fails with ENOENT when it is gone.
    fn threads(&self, pid: i64) -> io::Result<Vec<String>>;
    /// The `children` list of one thread; fails with ENOENT when the thread is gone or the kernel has no such file.
    fn children(&self, pid: i64, thread: &str) -> io::Result<String>;
    /// The content of `/proc/<pid>/stat`; fails with ENOENT or ESRCH when the process is gone.
    fn stat(&self, pid: i64) -> io::Result<String>;
}

pub struct RealProcIo;

impl ProcIo for RealProcIo {
    fn threads(&self, pid: i64) -> io::Result<Vec<String>> {
        let mut names = Vec::new();
        for item in std::fs::read_dir(format!("/proc/{pid}/task"))? {
            names.push(item?.file_name().to_string_lossy().into_owned());
        }
        Ok(names)
    }

    fn children(&self, pid: i64, thread: &str) -> io::Result<String> {
        std::fs::read_to_string(format!("/proc/{pid}/task/{thread}/children"))
    }

    fn stat(&self, pid: i64) -> io::Result<String> {
        let bytes = std::fs::read(format!("/proc/{pid}/stat"))?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}

fn is_gone(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(libc::ENOENT) | Some(libc::ESRCH))
        || error.kind() == io::ErrorKind::NotFound
}

/// Finds the processes `tool_processes(read_proc_table(), shell_pid)` counts by following `children` lists from the
/// shell pid instead of reading the stat of every process on the machine. `read` returns None when the caller has to
/// read the whole table: for good when the kernel has no `children` lists (no CONFIG_PROC_CHILDREN), and for
/// `retry_ms` after any other failure (a transient EMFILE, a permission error), after which it tries again.
pub struct ToolProcessWalker {
    io: Arc<dyn ProcIo>,
    now: Arc<dyn Fn() -> u64 + Send + Sync>,
    retry_ms: u64,
    own_pid: i64,
    state: Mutex<WalkerState>,
}

struct WalkerState {
    supported: Option<bool>,
    retry_at: u64,
}

impl ToolProcessWalker {
    pub fn new(
        io: Arc<dyn ProcIo>,
        now: Arc<dyn Fn() -> u64 + Send + Sync>,
        retry_ms: u64,
        own_pid: i64,
    ) -> Self {
        Self {
            io,
            now,
            retry_ms,
            own_pid,
            state: Mutex::new(WalkerState {
                supported: None,
                retry_at: 0,
            }),
        }
    }

    /// The production walker: real `/proc`, the wall clock, this process's pid.
    pub fn system() -> Self {
        Self::new(
            Arc::new(RealProcIo),
            Arc::new(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as u64)
            }),
            CHILDREN_RETRY_MS,
            i64::from(std::process::id()),
        )
    }

    pub fn read(&self, shell_pid: i64) -> Option<Vec<ProcEntry>> {
        {
            let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if state.supported == Some(false) || (self.now)() < state.retry_at {
                return None;
            }
        }
        let outcome = (|| -> io::Result<Option<Vec<ProcEntry>>> {
            let supported = {
                let known = self
                    .state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .supported;
                match known {
                    Some(value) => value,
                    None => {
                        let value = self.probe()?;
                        self.state
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .supported = Some(value);
                        value
                    }
                }
            };
            if !supported {
                return Ok(None);
            }
            self.walk(shell_pid).map(Some)
        })();
        match outcome {
            Ok(result) => result,
            Err(_) => {
                self.state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .retry_at = (self.now)().saturating_add(self.retry_ms);
                None
            }
        }
    }

    /// The kernel has `children` lists when this process's own list can be read: a missing file is the answer, any other
    /// failure is an error.
    fn probe(&self) -> io::Result<bool> {
        match self.io.children(self.own_pid, &self.own_pid.to_string()) {
            Ok(_) => Ok(true),
            Err(error)
                if error.raw_os_error() == Some(libc::ENOENT)
                    || error.kind() == io::ErrorKind::NotFound =>
            {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    fn child_pids(&self, pid: i64) -> io::Result<Vec<i64>> {
        let threads = match self.io.threads(pid) {
            Ok(threads) => threads,
            Err(error) if is_gone(&error) => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut pids = Vec::new();
        for thread in threads {
            let list = match self.io.children(pid, &thread) {
                Ok(list) => list,
                // The thread exited.
                Err(error) if is_gone(&error) => String::new(),
                Err(error) => return Err(error),
            };
            for id in list.split(' ').filter(|id| !id.is_empty()) {
                pids.push(js_number(id) as i64);
            }
        }
        Ok(pids)
    }

    fn entries(&self, pid: i64) -> io::Result<Vec<ProcEntry>> {
        let mut found = Vec::new();
        for id in self.child_pids(pid)? {
            match self.io.stat(id) {
                Ok(text) => {
                    if let Some(entry) = parse_proc_stat(id, &text, 100.0) {
                        found.push(entry);
                    }
                }
                // The process exited.
                Err(error) if is_gone(&error) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(found)
    }

    fn walk(&self, shell_pid: i64) -> io::Result<Vec<ProcEntry>> {
        let mut counted = Vec::new();
        let mut seen: HashSet<i64> = HashSet::from([shell_pid]);
        fn add(
            walker: &ToolProcessWalker,
            entry: ProcEntry,
            seen: &mut HashSet<i64>,
            counted: &mut Vec<ProcEntry>,
        ) -> io::Result<()> {
            if !seen.insert(entry.pid) {
                return Ok(());
            }
            let pid = entry.pid;
            counted.push(entry);
            for child in walker.entries(pid)? {
                add(walker, child, seen, counted)?;
            }
            Ok(())
        }
        for host in self.entries(shell_pid)? {
            for tool in self.entries(host.pid)? {
                if is_shell(&tool.comm) {
                    add(self, tool, &mut seen, &mut counted)?;
                }
            }
        }
        Ok(counted)
    }
}

/// `[[dd-]hh:]mm:ss[.cc]` in milliseconds: `/^(?:(\d+)-)?(?:(\d+):)?(\d+):(\d+(?:\.\d+)?)$/`.
pub fn parse_ps_time(text: &str) -> Option<f64> {
    let regex = &*PS_TIME;
    let captures = regex.captures(text)?;
    let number = |index: usize| captures.get(index).map_or(0.0, |m| js_number(m.as_str()));
    let days = number(1);
    let hours = number(2);
    let minutes = number(3);
    let seconds = number(4);
    Some(js_round(
        (days * 86_400.0 + hours * 3_600.0 + minutes * 60.0 + seconds) * 1000.0,
    ))
}

static PS_TIME: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"^(?:([0-9]+)-)?(?:([0-9]+):)?([0-9]+):([0-9]+(?:\.[0-9]+)?)$")
        .expect("the pattern is valid")
});

/// One line of `ps -A -o pid=,ppid=,time=,comm=`: `/^\s*(\d+)\s+(\d+)\s+(\S+)\s+(.+?)\s*$/` with JS meanings of `\s`
/// and `.`, as (pid, ppid, time, command).
fn split_ps_line(line: &str) -> Option<(&str, &str, &str, &str)> {
    fn digits(text: &str) -> (&str, &str) {
        let n = text.bytes().take_while(u8::is_ascii_digit).count();
        text.split_at(n)
    }
    fn spaces(text: &str) -> (&str, &str) {
        let n = text.len() - text.trim_start_matches(is_js_space).len();
        text.split_at(n)
    }
    let rest = line.trim_start_matches(is_js_space);
    let (pid, rest) = digits(rest);
    let (gap, rest) = spaces(rest);
    if pid.is_empty() || gap.is_empty() {
        return None;
    }
    let (ppid, rest) = digits(rest);
    let (gap, rest) = spaces(rest);
    if ppid.is_empty() || gap.is_empty() {
        return None;
    }
    let time_len = rest.len() - rest.trim_start_matches(|c: char| !is_js_space(c)).len();
    let (time, rest) = rest.split_at(time_len);
    let (gap, after_gap) = spaces(rest);
    if time.is_empty() || gap.is_empty() {
        return None;
    }
    // `\s+(.+?)\s*$`: the white space gives characters back to the command when the rest cannot match.
    let gap_chars: Vec<(usize, char)> = gap.char_indices().collect();
    for taken in (1..=gap_chars.len()).rev() {
        let start = if taken == gap_chars.len() {
            gap.len()
        } else {
            gap_chars[taken].0
        };
        let tail = &rest[start..];
        let mut end = 0;
        for c in tail.chars() {
            if is_line_terminator(c) {
                break;
            }
            end += c.len_utf8();
            if tail[end..].chars().all(is_js_space) {
                return Some((pid, ppid, time, &tail[..end]));
            }
        }
        let _ = after_gap;
    }
    None
}

fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// Parses `ps -A -o pid=,ppid=,time=,comm=` output (a header line is skipped); the command name is the rest of the line
/// and may hold spaces.
pub fn parse_ps_table(text: &str) -> ProcessTable {
    let mut entries = Vec::new();
    for line in text.split('\n') {
        let Some((pid, ppid, time, comm)) = split_ps_line(line) else {
            continue;
        };
        let Some(cpu_ms) = parse_ps_time(time) else {
            continue;
        };
        entries.push(ProcEntry {
            pid: js_number(pid) as i64,
            ppid: js_number(ppid) as i64,
            comm: comm.to_string(),
            cpu_ms,
            start_key: comm.to_string(),
        });
    }
    entries
}

const PS_MAX_BYTES: usize = 16 * 1024 * 1024;

fn read_ps_table() -> io::Result<ProcessTable> {
    let mut child = Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,time=,comm="])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdout = child.stdout.take().expect("piped");
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = io::Read::take(&mut stdout, PS_MAX_BYTES as u64 + 1).read_to_end(&mut bytes);
        bytes
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait()? {
            let bytes = reader.join().unwrap_or_default();
            if !status.success() || bytes.len() > PS_MAX_BYTES {
                return Err(io::Error::other("ps failed"));
            }
            return Ok(parse_ps_table(&String::from_utf8_lossy(&bytes)));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(io::ErrorKind::TimedOut, "ps timed out"));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

use std::io::Read as _;

/// Reads the whole process table: /proc on Linux, `ps` elsewhere.
pub type ProcessTableReader = Arc<dyn Fn() -> io::Result<ProcessTable> + Send + Sync>;

pub fn default_process_table_reader() -> ProcessTableReader {
    Arc::new(|| {
        if cfg!(target_os = "linux") {
            read_proc_table()
        } else {
            read_ps_table()
        }
    })
}

/// Finds the tool processes under a shell pid, or None when the caller has to read the whole table.
pub type ToolProcessReader = Arc<dyn Fn(i64) -> Option<Vec<ProcEntry>> + Send + Sync>;

/// What `sample` reads: the shell pid of the pane and the counted processes under it.
#[derive(Clone, Debug, PartialEq)]
pub struct DetailedSample {
    pub shell_pid: i64,
    pub processes: Vec<ProcEntry>,
}

pub struct HerdrProcessProbe {
    runner: Arc<dyn HerdrRunner>,
    read_table: ProcessTableReader,
    read_tools: Option<ToolProcessReader>,
}

impl HerdrProcessProbe {
    /// The production probe: follows `children` lists on Linux and reads the whole table when it cannot.
    pub fn new(runner: Arc<dyn HerdrRunner>) -> Self {
        let walker = Arc::new(ToolProcessWalker::system());
        let read_tools: Option<ToolProcessReader> = if cfg!(target_os = "linux") {
            Some(Arc::new(move |pid| walker.read(pid)))
        } else {
            None
        };
        Self {
            runner,
            read_table: default_process_table_reader(),
            read_tools,
        }
    }

    /// A probe with its own table reader (a test); it reads that table and never follows `children` lists.
    pub fn with_table_reader(runner: Arc<dyn HerdrRunner>, read_table: ProcessTableReader) -> Self {
        Self {
            runner,
            read_table,
            read_tools: None,
        }
    }

    pub fn sample_detailed(&self, pane_id: &str) -> Result<DetailedSample, HerdrError> {
        let args: Vec<String> = ["pane", "process-info", "--pane", pane_id]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let result = run_json(self.runner.as_ref(), &args, &RunOptions::default())?;
        let shell_pid = result
            .get("process_info")
            .and_then(|info| info.as_object())
            .and_then(|info| info.get("shell_pid"))
            .and_then(|pid| {
                pid.as_i64()
                    .or_else(|| pid.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64))
            })
            .filter(|pid| *pid > 0)
            .ok_or_else(|| HerdrError::new("bad_output", "herdr process info has no shell_pid"))?;
        let direct = self.read_tools.as_ref().and_then(|read| read(shell_pid));
        let processes = match direct {
            Some(processes) => processes,
            None => {
                let table = (self.read_table)().map_err(|error| {
                    HerdrError::new(
                        "process_table",
                        format!("the process table cannot be read: {error}"),
                    )
                })?;
                tool_processes(&table, shell_pid)
            }
        };
        Ok(DetailedSample {
            shell_pid,
            processes,
        })
    }
}

impl api::ProcessActivityProbe for HerdrProcessProbe {
    fn sample(&self, pane_id: &str) -> Result<api::ProcessSample, HerdrError> {
        let sample = self.sample_detailed(pane_id)?;
        Ok(api::ProcessSample {
            shell_pid: Some(sample.shell_pid),
            processes: sample
                .processes
                .iter()
                .map(|entry| api::ProcessEntry {
                    pid: entry.pid,
                    ppid: entry.ppid,
                    comm: entry.comm.clone(),
                    cpu_seconds: entry.cpu_ms / 1000.0,
                })
                .collect(),
        })
    }
}

/// Keeps each agent's last CPU activity of its tool processes; pure, in memory.
#[derive(Default)]
pub struct ProcessActivityTracker {
    previous: HashMap<String, HashMap<String, f64>>,
    last_activity: HashMap<String, u64>,
}

impl ProcessActivityTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, agent_id: &str, processes: &[ProcEntry], now_ms: u64) {
        let before = self.previous.get(agent_id);
        let mut next: HashMap<String, f64> = HashMap::new();
        let mut increase = 0.0;
        for entry in processes {
            let key = format!("{}|{}", entry.pid, entry.start_key);
            next.insert(key.clone(), entry.cpu_ms);
            let Some(before) = before else { continue };
            // The first sample of an agent only sets the baseline; later, a process not seen before counts its whole
            // CPU time as new work.
            let was = before.get(&key).copied().unwrap_or(0.0);
            increase += (entry.cpu_ms - was).max(0.0);
        }
        self.previous.insert(agent_id.to_string(), next);
        if increase >= MIN_CHILD_CPU_MS {
            self.last_activity.insert(agent_id.to_string(), now_ms);
        }
    }

    pub fn last_child_activity(&self, agent_id: &str) -> Option<u64> {
        self.last_activity.get(agent_id).copied()
    }

    pub fn forget(&mut self, agent_id: &str) {
        self.previous.remove(agent_id);
        self.last_activity.remove(agent_id);
    }

    /// Every agent with a last activity, for the messaging evaluation.
    pub fn activity(&self) -> &HashMap<String, u64> {
        &self.last_activity
    }
}
