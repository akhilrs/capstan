//! The stream loop: discovery, snapshots, catching up from a cursor, polling, heartbeats and stopping.
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use capstan_kernel::areas::feed_cursor::{decode_cursor, CursorEntry};
use capstan_kernel::helpers::clean_text;
use capstan_ledger::iso_from_millis;
use serde_json::Value;

use super::{lines, roots, Deps, EventsPlan, FeedCall, FeedFailure, LineSink};
use crate::projects::{classify, discover, project_id, Credential, EndpointState};
use crate::VERSION;

/// The anchor of sequence 0, and of a position the ledger has no anchor for.
const ZERO_ANCHOR: &str = "0000000000000000";
/// Events asked for in one page.
const PAGE_LIMIT: u32 = 500;
/// Items asked for in one snapshot page.
const ITEM_LIMIT: u32 = 100;
/// The longest sleep between two looks at the stop flag.
const SLICE_MS: u64 = 100;
const SECTIONS: [&str; 3] = ["waiting", "agents", "plans"];

/// Why the loop ends.
enum Halt {
    /// `stop` was set: write the final cursor line and return 0.
    Stop,
    /// The error line was written (or could not be): return this code.
    Exit(i32),
    /// The sink failed: return 0 at once.
    Sink,
}

/// What ends the work on one project.
enum Fail {
    Halt(Halt),
    /// The project cannot be followed now, in this state.
    Down(EndpointState),
}

impl From<Halt> for Fail {
    fn from(halt: Halt) -> Self {
        Fail::Halt(halt)
    }
}

/// How a catch-up ended.
enum Caught {
    Done,
    Resync(&'static str),
}

/// What one poll knows of one project.
struct Info {
    root: PathBuf,
    id: String,
    state: EndpointState,
    /// The ledger epoch and head of a ready project.
    at: Option<(String, u64)>,
}

struct Engine<'a> {
    plan: &'a EventsPlan,
    deps: &'a mut Deps,
    sink: &'a mut dyn LineSink,
    stop: &'a AtomicBool,
    cur: BTreeMap<String, CursorEntry>,
    /// The project line last written for each root: (project id, state).
    known: BTreeMap<PathBuf, (String, EndpointState)>,
    last_line_ms: u64,
    noted: BTreeSet<String>,
}

pub(super) fn run(
    plan: &EventsPlan,
    deps: &mut Deps,
    sink: &mut dyn LineSink,
    stop: &AtomicBool,
) -> i32 {
    let last_line_ms = deps.clock.now_ms();
    let mut engine = Engine {
        plan,
        deps,
        sink,
        stop,
        cur: BTreeMap::new(),
        known: BTreeMap::new(),
        last_line_ms,
        noted: BTreeSet::new(),
    };
    match engine.main() {
        Ok(()) | Err(Halt::Sink) => 0,
        Err(Halt::Exit(code)) => code,
        Err(Halt::Stop) => {
            let line = lines::cursor(&engine.cur);
            match engine.emit(vec![line]) {
                Err(Halt::Exit(code)) => code,
                _ => 0,
            }
        }
    }
}

fn host_name() -> String {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer is valid for its length and gethostname writes at most that many bytes.
    let rc = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len() - 1) };
    if rc != 0 {
        return "unknown".into();
    }
    let end = buffer.iter().position(|b| *b == 0).unwrap_or(buffer.len());
    String::from_utf8_lossy(&buffer[..end]).into_owned()
}

fn root_text(root: &Path) -> String {
    root.to_string_lossy().into_owned()
}

fn as_u64(value: &Value, name: &str) -> Option<u64> {
    value.get(name)?.as_u64()
}

fn as_str<'v>(value: &'v Value, name: &str) -> Option<&'v str> {
    value.get(name)?.as_str()
}

fn bad() -> Fail {
    Fail::Down(EndpointState::Silent)
}

impl Engine<'_> {
    fn main(&mut self) -> Result<(), Halt> {
        let mut bad_cursor = false;
        if let Some(text) = &self.plan.cursor {
            match decode_cursor(text) {
                Ok(entries) => self.cur = entries,
                Err(_) => bad_cursor = true,
            }
        }
        let poll_ms = self.plan.poll_ms.max(self.deps.limits.min_poll_ms);
        let mut first = true;
        loop {
            self.check_stop()?;
            let started = self.deps.clock.now_ms();
            self.cycle(first, bad_cursor && first)?;
            first = false;
            self.wait_until(started.saturating_add(poll_ms))?;
        }
    }

    fn check_stop(&self) -> Result<(), Halt> {
        if self.stop.load(Ordering::SeqCst) {
            Err(Halt::Stop)
        } else {
            Ok(())
        }
    }

    fn note(&mut self, text: &str) {
        let _ = writeln!(self.deps.err, "cstan events: {}", clean_text(text));
    }

    /// Writes `lines` as one batch, after checking every line; a line over the limit stops the stream.
    fn emit(&mut self, lines: Vec<String>) -> Result<(), Halt> {
        if lines.is_empty() {
            return Ok(());
        }
        if lines
            .iter()
            .any(|l| l.len() + 1 > self.deps.limits.line_bytes)
        {
            return Err(self.fatal("internal", "a line is longer than the stream allows"));
        }
        self.write(&lines)
    }

    fn write(&mut self, lines: &[String]) -> Result<(), Halt> {
        match self.sink.write_batch(lines) {
            Ok(()) => {
                self.last_line_ms = self.deps.clock.now_ms();
                Ok(())
            }
            Err(_) => Err(Halt::Sink),
        }
    }

    /// Writes the error line and gives the halt for exit code 1.
    fn fatal(&mut self, code: &str, message: &str) -> Halt {
        match self.write(&[lines::error(code, message)]) {
            Err(Halt::Sink) => Halt::Sink,
            _ => Halt::Exit(1),
        }
    }

    fn wait_until(&mut self, due: u64) -> Result<(), Halt> {
        loop {
            self.check_stop()?;
            let now = self.beat()?;
            if now >= due {
                return Ok(());
            }
            self.deps.clock.sleep_ms((due - now).min(SLICE_MS));
        }
    }

    /// Writes a heartbeat when nothing was written for the heartbeat interval; the time now.
    fn beat(&mut self) -> Result<u64, Halt> {
        let now = self.deps.clock.now_ms();
        if now.saturating_sub(self.last_line_ms) >= self.deps.limits.heartbeat_ms {
            let line = lines::heartbeat(&iso_from_millis(now.min(i64::MAX as u64) as i64));
            self.emit(vec![line])?;
        }
        Ok(now)
    }

    /// One feed call. A slow but live controller can show as silent for one poll: its abandoned call still runs and
    /// the next call to the root is not made until it ends (see `projects::call_within`).
    fn call(&mut self, root: &Path, key: &Credential, call: &FeedCall) -> Result<Value, Fail> {
        self.check_stop()?;
        self.beat()?;
        let answer = self.deps.feed.call(root, key, call);
        // A call that returned after the stop flag was set counts for nothing.
        self.check_stop()?;
        match answer {
            Ok(value) => Ok(value),
            Err(FeedFailure::Refused { code, .. }) => Err(Fail::Down(classify(&code))),
            Err(FeedFailure::NoAnswer) => Err(Fail::Down(EndpointState::Silent)),
        }
    }

    // ------------------------------------------------------------------------------------------- one poll

    fn project_id_of(&self, root: &Path) -> Option<String> {
        let cwd = &self.deps.cwd;
        self.cur
            .iter()
            .find(|(_, e)| roots::normalise(cwd, Path::new(&e.r)) == root)
            .map(|(id, _)| id.clone())
            .or_else(|| project_id(root))
    }

    fn cycle(&mut self, first: bool, bad_cursor: bool) -> Result<(), Halt> {
        let procs = self.deps.procs.list();
        let cursor_roots: Vec<String> = self.cur.values().map(|e| e.r.clone()).collect();
        let mut found =
            roots::discover_roots(&procs, &self.deps.cwd, &self.plan.roots, &cursor_roots);
        let max = self.deps.limits.projects;
        if found.len() > max {
            if first {
                return Err(
                    self.fatal("invalid_request", &format!("more than {max} project roots"))
                );
            }
            found.truncate(max);
        }
        self.check_stop()?;
        self.beat()?;
        let endpoints = discover(&[], &found, &*self.deps.probe);
        self.check_stop()?;

        let mut infos: Vec<Info> = Vec::new();
        let mut taken: HashMap<String, PathBuf> = HashMap::new();
        for endpoint in endpoints {
            let root = endpoint.root;
            let (id, state, at) = if endpoint.state == EndpointState::Ready {
                match self.head(&root) {
                    Ok((id, epoch, head)) => (Some(id), EndpointState::Ready, Some((epoch, head))),
                    Err(Fail::Halt(halt)) => return Err(halt),
                    Err(Fail::Down(state)) => {
                        self.note(&format!(
                            "{} cannot be followed now ({})",
                            root_text(&root),
                            lines::state_name(state)
                        ));
                        (self.project_id_of(&root), state, None)
                    }
                }
            } else {
                (self.project_id_of(&root), endpoint.state, None)
            };
            let Some(id) = id else { continue };
            if let Some(other) = taken.get(&id) {
                let text = format!(
                    "{} has the project id of {}; not followed",
                    root_text(&root),
                    root_text(other)
                );
                if self.noted.insert(text.clone()) {
                    self.note(&text);
                }
                continue;
            }
            taken.insert(id.clone(), root.clone());
            infos.push(Info {
                root,
                id,
                state,
                at,
            });
        }

        let mut batch: Vec<String> = Vec::new();
        if first {
            batch.push(lines::hello(VERSION, &host_name(), infos.len()));
        }
        let mut dirty = false;
        for info in &infos {
            let now = (info, self.known.get(&info.root));
            let unchanged =
                matches!(now.1, Some((id, state)) if *id == info.id && *state == info.state);
            if !unchanged {
                batch.push(project_line(info));
                self.known
                    .insert(info.root.clone(), (info.id.clone(), info.state));
            }
            dirty |= self.mark_availability(&info.id, info.state);
        }
        if bad_cursor {
            batch.push(lines::resync(None, "cursor"));
        }
        if dirty {
            batch.push(lines::cursor(&self.cur));
        }
        self.emit(batch)?;

        for info in &infos {
            if info.state != EndpointState::Ready {
                continue;
            }
            self.check_stop()?;
            match self.sync(info) {
                Ok(()) => {}
                Err(Fail::Halt(halt)) => return Err(halt),
                Err(Fail::Down(state)) => self.down(info, state)?,
            }
        }
        Ok(())
    }

    /// Sets or clears the entry's `d` for a project in `state`; true when the entry changed.
    fn mark_availability(&mut self, id: &str, state: EndpointState) -> bool {
        let now_s = (self.deps.clock.now_ms() / 1000).min(i64::MAX as u64) as i64;
        let Some(entry) = self.cur.get_mut(id) else {
            return false;
        };
        match state {
            EndpointState::Ready if entry.d.is_some() => {
                entry.d = None;
                true
            }
            EndpointState::Silent | EndpointState::Unsupported | EndpointState::Gone
                if entry.d.is_none() =>
            {
                entry.d = Some(now_s);
                true
            }
            _ => false,
        }
    }

    /// A project that answered `head` and then failed: its new state, as a project line and in the entry.
    fn down(&mut self, info: &Info, state: EndpointState) -> Result<(), Halt> {
        self.note(&format!(
            "{} cannot be followed now ({})",
            root_text(&info.root),
            lines::state_name(state)
        ));
        let mut batch = Vec::new();
        if self.known.get(&info.root) != Some(&(info.id.clone(), state)) {
            batch.push(lines::project(
                &info.id,
                &root_text(&info.root),
                state,
                None,
            ));
            self.known
                .insert(info.root.clone(), (info.id.clone(), state));
        }
        if self.mark_availability(&info.id, state) {
            batch.push(lines::cursor(&self.cur));
        }
        self.emit(batch)
    }

    fn head(&mut self, root: &Path) -> Result<(String, String, u64), Fail> {
        let key = self
            .deps
            .probe
            .key(root)
            .ok_or(Fail::Down(EndpointState::NoKey))?;
        let answer = self.call(root, &key, &FeedCall::Head)?;
        let id = as_str(&answer, "projectId").ok_or_else(bad)?;
        let epoch = as_str(&answer, "epoch").ok_or_else(bad)?;
        let head = as_u64(&answer, "head").ok_or_else(bad)?;
        Ok((id.to_string(), epoch.to_string(), head))
    }

    // --------------------------------------------------------------------------------------- one project

    fn sync(&mut self, info: &Info) -> Result<(), Fail> {
        let key = self
            .deps
            .probe
            .key(&info.root)
            .ok_or(Fail::Down(EndpointState::NoKey))?;
        let epoch = info.at.as_ref().map(|(e, _)| e.as_str()).unwrap_or("");
        let entry = self.cur.get(&info.id).cloned();
        let reason = match entry {
            None => None,
            Some(entry) if entry.e != epoch => Some("epoch"),
            Some(_) => match self.catch_up(info, &key)? {
                Caught::Done => return Ok(()),
                Caught::Resync(reason) => Some(reason),
            },
        };
        if let Some(reason) = reason {
            self.cur.remove(&info.id);
            let batch = vec![
                lines::resync(Some(&info.id), reason),
                lines::cursor(&self.cur),
            ];
            self.emit(batch)?;
        }
        self.snapshot(info, &key)?;
        match self.catch_up(info, &key)? {
            Caught::Done => Ok(()),
            Caught::Resync(_) => Err(bad()),
        }
    }

    /// Reads the pages after the entry of the project until none is left.
    fn catch_up(&mut self, info: &Info, key: &Credential) -> Result<Caught, Fail> {
        loop {
            let Some(entry) = self.cur.get(&info.id).cloned() else {
                return Err(bad());
            };
            let call = FeedCall::Page {
                after_seq: entry.s,
                after_k: entry.k,
                limit: PAGE_LIMIT,
                anchor: (entry.s > 0).then(|| entry.h.clone()),
            };
            let page = self.call(&info.root, key, &call)?;
            if page.get("ahead").and_then(Value::as_bool) == Some(true) {
                return Ok(Caught::Resync("ahead"));
            }
            match page.get("anchorOk").and_then(Value::as_bool) {
                Some(true) => {}
                Some(false) => return Ok(Caught::Resync("anchor")),
                None => return Err(bad()),
            }
            let events = page
                .get("events")
                .and_then(Value::as_array)
                .ok_or_else(bad)?;
            let scanned = page.get("scanned").ok_or_else(bad)?;
            let seq = as_u64(scanned, "seq").ok_or_else(bad)?;
            let k = as_u64(scanned, "k")
                .and_then(|k| u32::try_from(k).ok())
                .ok_or_else(bad)?;
            let anchor = match page.get("anchor") {
                Some(Value::String(text)) => text.clone(),
                Some(Value::Null) | None => ZERO_ANCHOR.to_string(),
                _ => return Err(bad()),
            };
            let has_more = page
                .get("hasMore")
                .and_then(Value::as_bool)
                .ok_or_else(bad)?;
            let next = CursorEntry {
                e: entry.e.clone(),
                s: seq,
                k,
                h: anchor,
                r: root_text(&info.root),
                d: None,
            };
            let moved = (next.s, next.k, &next.h) != (entry.s, entry.k, &entry.h);
            let mut batch: Vec<String> = events
                .iter()
                .map(|event| lines::event(&info.id, event))
                .collect();
            let mut after = self.cur.clone();
            if moved || !batch.is_empty() || entry.d.is_some() {
                after.insert(info.id.clone(), next);
                batch.push(lines::cursor(&after));
            }
            self.emit(batch)?;
            self.cur = after;
            if !has_more || !moved {
                return Ok(Caught::Done);
            }
        }
    }

    /// Reads the snapshot of the project and writes it with the entry at its head.
    fn snapshot(&mut self, info: &Info, key: &Credential) -> Result<(), Fail> {
        let head = self.call(&info.root, key, &FeedCall::Snapshot)?;
        let epoch = as_str(&head, "epoch").ok_or_else(bad)?.to_string();
        let at = as_u64(&head, "head").ok_or_else(bad)?;
        let anchor = match head.get("anchor") {
            Some(Value::String(text)) => text.clone(),
            Some(Value::Null) | None => ZERO_ANCHOR.to_string(),
            _ => return Err(bad()),
        };
        let run = head
            .get("run")
            .filter(|run| run.is_object())
            .ok_or_else(bad)?;

        // Items by (section, key): a later read replaces an earlier one, the order is the order first seen.
        let mut items: Vec<(&'static str, String, Value)> = Vec::new();
        let mut index: HashMap<(&'static str, String), usize> = HashMap::new();
        let mut bytes = 0usize;
        for section in SECTIONS {
            let mut after: Option<String> = None;
            loop {
                let call = match section {
                    "waiting" => FeedCall::Waiting {
                        after: after.clone(),
                        limit: ITEM_LIMIT,
                        kind: None,
                    },
                    "agents" => FeedCall::Agents {
                        after: after.clone(),
                        limit: ITEM_LIMIT,
                    },
                    _ => FeedCall::Plans {
                        after: after.clone(),
                        limit: ITEM_LIMIT,
                    },
                };
                let page = self.call(&info.root, key, &call)?;
                for item in page
                    .get("items")
                    .and_then(Value::as_array)
                    .ok_or_else(bad)?
                {
                    let item_key = as_str(item, "key").ok_or_else(bad)?.to_string();
                    bytes = bytes.saturating_add(item.to_string().len());
                    if bytes > self.deps.limits.snapshot_buffer_bytes {
                        return Err(Fail::Halt(
                            self.fatal("internal", "a snapshot is larger than the buffer allows"),
                        ));
                    }
                    match index.get(&(section, item_key.clone())) {
                        Some(at) => items[*at].2 = item.clone(),
                        None => {
                            index.insert((section, item_key.clone()), items.len());
                            items.push((section, item_key, item.clone()));
                        }
                    }
                }
                match page.get("next") {
                    Some(Value::String(next)) if after.as_deref() != Some(next.as_str()) => {
                        after = Some(next.clone())
                    }
                    Some(Value::String(_)) => return Err(bad()),
                    _ => break,
                }
            }
        }

        let mut batch: Vec<String> = Vec::with_capacity(items.len() + 3);
        for section in SECTIONS {
            for (item_section, item_key, item) in &items {
                if *item_section == section {
                    batch.push(lines::snapshot(&info.id, section, item_key, item));
                }
            }
        }
        batch.push(lines::snapshot(&info.id, "run", "run", run));
        batch.push(lines::snapshot_end(&info.id, at));
        let mut after = self.cur.clone();
        after.insert(
            info.id.clone(),
            CursorEntry {
                e: epoch,
                s: at,
                k: 0,
                h: anchor,
                r: root_text(&info.root),
                d: None,
            },
        );
        batch.push(lines::cursor(&after));
        self.emit(batch)?;
        self.cur = after;
        Ok(())
    }
}

fn project_line(info: &Info) -> String {
    let position = info
        .at
        .as_ref()
        .map(|(epoch, head)| (epoch.as_str(), *head));
    lines::project(&info.id, &root_text(&info.root), info.state, position)
}
