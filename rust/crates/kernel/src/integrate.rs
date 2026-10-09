//! The integration pipeline (src/integration.ts and the git half of src/git.ts): merging reports into an integration
//! branch and settling it. Git runs as a subprocess with the same clean environment, argument lists and exit-code
//! handling as the Node code; the kernel is reached through the integrations area.

use crate::areas::integrations;
use crate::errors::KernelError;
use crate::helpers::{fold_whitespace, is_js_space, js_trim};
use crate::kernel::Kernel;
use crate::plan_body::{cut_at_word, cut_hard, format_subject, COMMIT_TYPES};
use crate::records::{MAX_CONFLICT_FILES, MAX_CONFLICT_PATH_CHARS};
use crate::types::MutationContext;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

// ------------------------------------------------------------------------------------------------------ errors

/// What `integrate`, `settle_integration` and `recover_integrations` can fail with: `IntegrationError`, `GitCheckError`
/// or an error of the kernel.
#[derive(Debug)]
pub enum IntegrateError {
    Integration { code: String, message: String },
    Git(String),
    Kernel(KernelError),
}

pub type IntegrateResult<T> = Result<T, IntegrateError>;

impl IntegrateError {
    /// The `name` property of the Node error.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Integration { .. } => "IntegrationError",
            Self::Git(_) => "GitCheckError",
            Self::Kernel(e) => e.name(),
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::Integration { message, .. } => message.clone(),
            Self::Git(message) => message.clone(),
            Self::Kernel(e) => e.message(),
        }
    }

    /// `String(error)`.
    pub fn display(&self) -> String {
        format!("{}: {}", self.name(), self.message())
    }
}

impl From<KernelError> for IntegrateError {
    fn from(error: KernelError) -> Self {
        Self::Kernel(error)
    }
}

fn git_error(message: impl Into<String>) -> IntegrateError {
    IntegrateError::Git(message.into())
}

// ------------------------------------------------------------------------------------------------------ conventions

const MAX_BRANCH: usize = 100;
const SUBJECT_MAX: usize = 72;
const SUMMARY_MAX: usize = 400;
const TYPE_RANK: [&str; 2] = ["feat", "fix"];

fn js_len(text: &str) -> usize {
    text.encode_utf16().count()
}

fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// `ParsedSubject` when it parsed.
pub struct ParsedSubject {
    pub kind: String,
    pub scope: Option<String>,
    pub breaking: bool,
    pub description: String,
}

/// `parseCommitSubject` (the `ok` case; the reason texts are not needed here).
pub fn parse_commit_subject(subject: &str) -> Option<ParsedSubject> {
    // /^([a-z]+)(?:\(([^()\s]*)\))?(!)?: (.*)$/
    let kind_end = subject
        .find(|c: char| !c.is_ascii_lowercase())
        .unwrap_or(subject.len());
    if kind_end == 0 {
        return None;
    }
    let kind = &subject[..kind_end];
    let mut rest = &subject[kind_end..];
    let mut scope: Option<String> = None;
    if let Some(inner) = rest.strip_prefix('(') {
        let close = inner.find(|c: char| matches!(c, '(' | ')') || is_js_space(c))?;
        if !inner[close..].starts_with(')') {
            return None;
        }
        scope = Some(inner[..close].to_string());
        rest = &inner[close + 1..];
    }
    let breaking = match rest.strip_prefix('!') {
        Some(after) => {
            rest = after;
            true
        }
        None => false,
    };
    let description = rest.strip_prefix(": ")?;
    if description.chars().any(is_line_terminator) {
        return None;
    }
    if !COMMIT_TYPES.contains(&kind) {
        return None;
    }
    if scope.as_deref() == Some("") {
        return None;
    }
    if js_trim(description).is_empty() || description.starts_with(is_js_space) {
        return None;
    }
    Some(ParsedSubject {
        kind: kind.to_string(),
        scope,
        breaking,
        description: description.to_string(),
    })
}

/// `checkCommitMessage`: the rules a message breaks, as `(rule, reason)`.
pub fn check_commit_message(message: &str, parents: usize) -> Vec<(&'static str, &'static str)> {
    let mut out = Vec::new();
    let normalized = message.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    if parents < 2 {
        if parse_commit_subject(lines[0]).is_none() {
            out.push(("subject-format", "subject format"));
        }
        if lines.len() > 1 && !js_trim(lines[1]).is_empty() {
            out.push((
                "body-separation",
                "leave a blank line between the subject and the body",
            ));
        }
    }
    if lines.iter().any(|l| is_claude_co_author(l)) {
        out.push(("claude-co-author", "remove the Claude Co-Authored-By line"));
    }
    if lines.iter().any(|l| {
        l.trim_start_matches(is_js_space)
            .to_lowercase()
            .starts_with("claude-session:")
    }) {
        out.push(("claude-session", "remove the Claude-Session line"));
    }
    if lines.iter().any(|l| {
        let lower = l.to_lowercase();
        lower.contains("generated with claude code")
            || lower.contains("generated with [claude code")
            || l.contains('\u{1F916}')
    }) {
        out.push((
            "claude-code-footer",
            "remove the 'Generated with Claude Code' footer",
        ));
    }
    out
}

/// /^\s*co-authored-by:.*(\bclaude\b|noreply@anthropic\.com)/i
fn is_claude_co_author(line: &str) -> bool {
    let lower = line.trim_start_matches(is_js_space).to_lowercase();
    let Some(rest) = lower.strip_prefix("co-authored-by:") else {
        return false;
    };
    let rest: &str = match rest.find(is_line_terminator) {
        Some(end) => &rest[..end],
        None => rest,
    };
    if rest.contains("noreply@anthropic.com") {
        return true;
    }
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    rest.match_indices("claude").any(|(at, word)| {
        let before = rest[..at].chars().next_back();
        let after = rest[at + word.len()..].chars().next();
        !before.is_some_and(is_word) && !after.is_some_and(is_word)
    })
}

/// `slugify` of src/conventions.ts: the title in NFKD with the combining marks dropped, lowercased, the letters that
/// do not decompose folded to ASCII, every run of other characters one hyphen, cut to `max` characters.
pub fn slugify(title: &str, max: usize) -> String {
    let mut folded = String::new();
    for c in title.nfkd().filter(|c| !is_combining_mark(*c)) {
        for lower in c.to_lowercase() {
            match lower {
                'ß' => folded.push_str("ss"),
                'ø' => folded.push('o'),
                'æ' => folded.push_str("ae"),
                'œ' => folded.push_str("oe"),
                'đ' | 'ð' => folded.push('d'),
                'ł' => folded.push('l'),
                'þ' => folded.push_str("th"),
                other => folded.push(other),
            }
        }
    }
    let mut slug = String::new();
    let mut in_run = false;
    for c in folded.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
            in_run = false;
        } else if !in_run {
            slug.push('-');
            in_run = true;
        }
    }
    let slug = slug.trim_matches('-');
    let cut: String = slug.chars().take(max.max(1)).collect();
    let cut = cut.trim_end_matches('-');
    if cut.is_empty() {
        "work".into()
    } else {
        cut.to_string()
    }
}

/// `cleanId`.
fn clean_id(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut in_run = false;
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            in_run = false;
        } else if !in_run {
            out.push('-');
            in_run = true;
        }
    }
    let trimmed = out.trim_matches('-');
    let cut: String = trimmed.chars().take(max).collect();
    let cut = cut.trim_end_matches('-');
    if cut.is_empty() {
        "x".into()
    } else {
        cut.to_string()
    }
}

/// `clip`.
fn clip(name: &str) -> String {
    if name.len() <= MAX_BRANCH {
        name.to_string()
    } else {
        name[..MAX_BRANCH]
            .trim_end_matches(['-', '.', '/'])
            .to_string()
    }
}

/// `integrationBranchName` with the parts the pipeline passes.
pub fn integration_branch_name(integration_id: &str, plan_title: Option<&str>) -> String {
    let mut parts = vec![clean_id(integration_id, 40)];
    if let Some(title) = plan_title.filter(|t| !t.is_empty()) {
        parts.push(slugify(title, 30));
    }
    clip(&format!("integration/{}", parts.join("-")))
}

/// `withSuffix`.
pub fn with_suffix(branch: &str, n: usize) -> String {
    if n < 2 {
        return branch.to_string();
    }
    let suffix = format!("-{n}");
    if branch.len() + suffix.len() <= MAX_BRANCH {
        return format!("{branch}{suffix}");
    }
    format!(
        "{}{suffix}",
        branch[..MAX_BRANCH - suffix.len()].trim_end_matches(['-', '.', '/'])
    )
}

/// The end of /^[a-z]+(?:\([^)]*\))?!?:\s*/ at the start of `text`.
fn leading_type_end(text: &str) -> Option<usize> {
    let letters = text
        .find(|c: char| !c.is_ascii_lowercase())
        .unwrap_or(text.len());
    if letters == 0 {
        return None;
    }
    let mut at = letters;
    if text[at..].starts_with('(') {
        let close = text[at..].find(')')?;
        at += close + 1;
    }
    if text[at..].starts_with('!') {
        at += 1;
    }
    if !text[at..].starts_with(':') {
        return None;
    }
    at += 1;
    let spaces = text[at..]
        .find(|c: char| !is_js_space(c))
        .unwrap_or(text[at..].len());
    Some(at + spaces)
}

fn strip_leading_type(text: &str) -> &str {
    match leading_type_end(text) {
        Some(end) => &text[end..],
        None => text,
    }
}

/// The type word of /^([a-z]+)(?:\([^)]*\))?!?:/ when it matches.
fn leading_type_word(text: &str) -> Option<&str> {
    let end = leading_type_end(text)?;
    let letters = text.find(|c: char| !c.is_ascii_lowercase())?;
    let _ = end;
    Some(&text[..letters])
}

fn first_line(text: &str) -> String {
    text.split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .find(|l| !js_trim(l).is_empty())
        .map(|l| js_trim(l).to_string())
        .unwrap_or_default()
}

fn text_of<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

// ------------------------------------------------------------------------------------------------------ squash message

struct FromSubjects {
    kind: String,
    scope: Option<String>,
    breaking: bool,
    description: String,
}

fn from_commit_subjects(
    subjects: &HashMap<String, String>,
    report_ids: &[&str],
) -> Option<FromSubjects> {
    let parsed: Vec<ParsedSubject> = report_ids
        .iter()
        .filter_map(|id| subjects.get(*id).and_then(|s| parse_commit_subject(s)))
        .collect();
    if parsed.is_empty() {
        return None;
    }
    let mut lead = &parsed[0];
    for wanted in TYPE_RANK {
        if let Some(hit) = parsed.iter().find(|p| p.kind == wanted) {
            lead = hit;
            break;
        }
    }
    let scopes: BTreeSet<&str> = parsed
        .iter()
        .filter_map(|p| p.scope.as_deref().filter(|s| !s.is_empty()))
        .collect();
    Some(FromSubjects {
        kind: lead.kind.clone(),
        scope: if scopes.len() == 1 {
            scopes.iter().next().map(|s| s.to_string())
        } else {
            None
        },
        breaking: parsed.iter().any(|p| p.breaking),
        description: lead.description.clone(),
    })
}

fn squash_type(info: &Value, source: &str) -> String {
    let packages = info
        .get("packages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let types: Vec<&str> = packages
        .iter()
        .filter_map(|p| p.get("type").and_then(Value::as_str))
        .collect();
    // `types` borrows from `packages`, which is owned here.
    for wanted in TYPE_RANK {
        if types.contains(&wanted) {
            return wanted.to_string();
        }
    }
    if let Some(first) = types.first() {
        return first.to_string();
    }
    match leading_type_word(source) {
        Some(word) if COMMIT_TYPES.contains(&word) => word.to_string(),
        _ => "feat".into(),
    }
}

/// `squashMessage`: the squash commit message of an integration, as `(subject, body)`. `info` is the record of
/// `integrationCommitInfo` and `subjects` the commit subject of each report.
pub fn squash_message(info: &Value, subjects: &HashMap<String, String>) -> (String, String) {
    let reports: Vec<Value> = info
        .get("reports")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let packages: Vec<Value> = info
        .get("packages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let plan_title = info.get("planTitle").and_then(Value::as_str);
    let plan_id = info.get("planId").and_then(Value::as_str);
    let first = first_line(reports.first().map_or("", |r| text_of(r, "summary")));
    let own = if plan_title.is_none() {
        let ids: Vec<&str> = reports.iter().map(|r| text_of(r, "reportId")).collect();
        from_commit_subjects(subjects, &ids)
    } else {
        None
    };
    let source: String = plan_title.map_or(first, str::to_string);
    let folded_source = fold_whitespace(&source);
    let from_source = js_trim(strip_leading_type(js_trim(&folded_source)));
    let description = match own.as_ref().map(|o| o.description.as_str()) {
        Some(d) if !d.is_empty() => d.to_string(),
        _ if !from_source.is_empty() => from_source.to_string(),
        _ => "integrate reports".to_string(),
    };
    let scopes: BTreeSet<&str> = packages
        .iter()
        .filter_map(|p| {
            p.get("scope")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        })
        .collect();
    let lines: Vec<String> = reports
        .iter()
        .map(|r| {
            let text = js_trim(&fold_whitespace(text_of(r, "summary"))).to_string();
            let clipped = if js_len(&text) > SUMMARY_MAX {
                let cut = cut_at_word(&text, SUMMARY_MAX);
                let cut = if cut.is_empty() {
                    cut_hard(&text, SUMMARY_MAX)
                } else {
                    cut
                };
                format!("{cut} ...")
            } else {
                text
            };
            let id = text_of(r, "reportId");
            let agent = text_of(r, "agentId");
            let line = format!("Report {id} ({agent}): {clipped}");
            // A worker's free text must not carry an attribution line into the commit.
            if check_commit_message(&format!("chore: x\n\n{line}"), 1).is_empty() {
                line
            } else {
                format!("Report {id} ({agent}): summary left out")
            }
        })
        .collect();
    let scope: Option<String> = if scopes.len() == 1 {
        scopes.iter().next().map(|s| s.to_string())
    } else {
        own.as_ref().and_then(|o| o.scope.clone())
    };
    let breaking = own.as_ref().is_some_and(|o| o.breaking)
        || packages
            .iter()
            .any(|p| p.get("breaking").and_then(Value::as_bool) == Some(true));
    let kind = match &own {
        Some(o) => o.kind.clone(),
        None => squash_type(info, &source),
    };
    let subject = format_subject(&kind, scope.as_deref(), breaking, &description, SUBJECT_MAX);
    let mut lines = lines;
    if let Some(plan) = plan_id {
        lines.push(String::new());
        lines.push(format!("Refs: {plan}"));
    }
    (subject, lines.join("\n"))
}

// ------------------------------------------------------------------------------------------------------ git

const GIT_TIMEOUT: Duration = Duration::from_secs(10);
const INTEGRATION_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_BUFFER: usize = 64 * 1024;
const INTEGRATION_BUFFER: usize = 8 * 1024 * 1024;
const NO_COMMIT: &str = "0000000000000000000000000000000000000000";
const IDENTITY: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "capstan"),
    ("GIT_AUTHOR_EMAIL", "capstan@localhost"),
    ("GIT_COMMITTER_NAME", "capstan"),
    ("GIT_COMMITTER_EMAIL", "capstan@localhost"),
];

fn is_full_sha(text: &str) -> bool {
    text.len() == 40 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// One git run.
struct Outcome {
    code: i32,
    stdout: Vec<u8>,
}

/// The bytes git printed as the text `encoding: "latin1"` gives: one character per byte.
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|b| char::from(*b)).collect()
}

fn utf8(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[derive(Clone, Copy)]
struct RunOptions {
    timeout: Duration,
    max_buffer: usize,
    identity: bool,
}

const PLAIN: RunOptions = RunOptions {
    timeout: GIT_TIMEOUT,
    max_buffer: DEFAULT_BUFFER,
    identity: false,
};
const WRITE: RunOptions = RunOptions {
    timeout: INTEGRATION_TIMEOUT,
    max_buffer: INTEGRATION_BUFFER,
    identity: true,
};
const BIG: RunOptions = RunOptions {
    timeout: INTEGRATION_TIMEOUT,
    max_buffer: INTEGRATION_BUFFER,
    identity: false,
};

/// The repository the integration pipeline works on. `extra_env` is added to every git run (a test pins the commit
/// dates with it); the daemon leaves it empty.
#[derive(Clone, Debug)]
pub struct GitRepo {
    root: PathBuf,
    extra_env: Vec<(String, String)>,
}

/// What `mergeIntoBranch` found.
#[derive(Clone, Debug, PartialEq)]
pub enum MergeResult {
    Merged {
        head_sha: String,
    },
    Conflicted {
        report_id: String,
        files: Vec<String>,
        omitted: usize,
    },
    Failed {
        reason: String,
    },
}

impl MergeResult {
    /// The outcome the kernel records.
    pub fn outcome(&self) -> Value {
        match self {
            Self::Merged { head_sha } => json!({"kind": "merged", "headSha": head_sha}),
            Self::Conflicted {
                report_id,
                files,
                omitted,
            } => {
                json!({"kind": "conflicted", "reportId": report_id, "files": files, "omitted": omitted})
            }
            Self::Failed { reason } => json!({"kind": "failed", "reason": reason}),
        }
    }
}

/// A report the integration head already holds.
#[derive(Clone, Debug, PartialEq)]
pub struct CoveredReport {
    pub report_id: String,
    pub how: &'static str,
}

/// A report to judge for coverage.
#[derive(Clone, Debug)]
pub struct CoverageReport {
    pub report_id: String,
    pub commit_sha: String,
    pub integration_heads: Vec<String>,
}

/// `printablePath`: a path as text that is safe to show and to store.
pub fn printable_path(raw: &[u8]) -> String {
    let mut text = String::new();
    for &byte in raw {
        if !(0x20..=0x7e).contains(&byte) || matches!(byte, b'"' | b',' | b'#' | b'\\') {
            text.push_str(&format!("\\x{byte:02x}"));
        } else {
            text.push(char::from(byte));
        }
    }
    if text.len() <= MAX_CONFLICT_PATH_CHARS {
        return text;
    }
    // Cut between escapes, and add a digest of the whole name so two long paths with one beginning stay different.
    let mut cut = text[..MAX_CONFLICT_PATH_CHARS].to_string();
    let partial = |s: &str| -> Option<usize> {
        let bytes = s.as_bytes();
        for back in 1..=3usize.min(bytes.len()) {
            let at = bytes.len() - back;
            if bytes[at] != b'\\' {
                continue;
            }
            let tail = &bytes[at + 1..];
            let ok = match tail {
                [] => true,
                [b'x'] => true,
                [b'x', h] => matches!(h, b'0'..=b'9' | b'a'..=b'f'),
                _ => false,
            };
            if ok {
                return Some(at);
            }
        }
        None
    };
    if let Some(at) = partial(&cut) {
        cut.truncate(at);
    }
    let digest = Sha256::digest(raw);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("{cut}...#{}", &hex[..12])
}

impl GitRepo {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            extra_env: Vec::new(),
        }
    }

    pub fn with_env(mut self, extra_env: Vec<(String, String)>) -> Self {
        self.extra_env = extra_env;
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `runGit`: `Err` when git itself could not run (spawn failure, timeout, output too large, killed by a signal).
    fn run(&self, args: &[&str], options: RunOptions) -> IntegrateResult<Outcome> {
        let mut command = Command::new("git");
        command
            .arg("--no-replace-objects")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .env_clear()
            .env(
                "PATH",
                std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()),
            )
            .env("LC_ALL", "C")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if options.identity {
            for (key, value) in IDENTITY {
                command.env(key, value);
            }
        }
        for (key, value) in &self.extra_env {
            command.env(key, value);
        }
        let mut child = command.spawn().map_err(|error| {
            let code = match error.kind() {
                std::io::ErrorKind::NotFound => "ENOENT".to_string(),
                _ => error.to_string(),
            };
            git_error(format!("git could not be run ({code})"))
        })?;
        let mut pipe = child.stdout.take().expect("stdout is piped");
        let collected = Arc::new(Mutex::new(Vec::<u8>::new()));
        let overflow = Arc::new(AtomicBool::new(false));
        let reader = {
            let collected = collected.clone();
            let overflow = overflow.clone();
            let limit = options.max_buffer;
            std::thread::spawn(move || {
                let mut chunk = [0u8; 8192];
                loop {
                    match pipe.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let mut buffer = collected.lock().expect("the buffer lock");
                            buffer.extend_from_slice(&chunk[..n]);
                            if buffer.len() > limit {
                                overflow.store(true, Ordering::SeqCst);
                                break;
                            }
                        }
                    }
                }
            })
        };
        let started = Instant::now();
        let status = loop {
            if overflow.load(Ordering::SeqCst) {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(git_error(
                    "git could not be run (ERR_CHILD_PROCESS_STDIO_MAXBUFFER)",
                ));
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = reader.join();
                    return Err(git_error(format!("git could not be run ({error})")));
                }
            }
            if started.elapsed() >= options.timeout {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(git_error("git could not be run (timed out)"));
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        let _ = reader.join();
        if overflow.load(Ordering::SeqCst) {
            return Err(git_error(
                "git could not be run (ERR_CHILD_PROCESS_STDIO_MAXBUFFER)",
            ));
        }
        let Some(code) = status.code() else {
            return Err(git_error("git could not be run (Command failed: git)"));
        };
        let stdout = collected.lock().expect("the buffer lock").clone();
        Ok(Outcome { code, stdout })
    }

    fn text(&self, args: &[&str], options: RunOptions) -> IntegrateResult<(i32, String)> {
        let outcome = self.run(args, options)?;
        Ok((outcome.code, utf8(&outcome.stdout)))
    }

    /// Exit 0 true, exit 1 false, anything else is a failure of git itself and an error.
    fn is_ancestor(&self, ancestor: &str, descendant: &str) -> IntegrateResult<bool> {
        let outcome = self.run(
            &["merge-base", "--is-ancestor", ancestor, descendant],
            PLAIN,
        )?;
        match outcome.code {
            0 => Ok(true),
            1 => Ok(false),
            _ => Err(git_error("git could not compare the commits")),
        }
    }

    /// Whether a commit with this id exists; `Err` for an id that is not a full sha1.
    pub fn commit_exists(&self, sha: &str) -> IntegrateResult<bool> {
        if !is_full_sha(sha) {
            return Err(git_error(
                "the commit id must be 40 lowercase hex characters",
            ));
        }
        let outcome = self.run(
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{sha}^{{commit}}"),
            ],
            PLAIN,
        )?;
        match outcome.code {
            0 => Ok(true),
            1 => Ok(false),
            _ => Err(git_error("git could not look the commit up")),
        }
    }

    /// The first line of a commit's message, or `None` when git cannot read it.
    pub fn commit_subject(&self, sha: &str) -> IntegrateResult<Option<String>> {
        if !is_full_sha(sha) {
            return Ok(None);
        }
        let (code, stdout) = self.text(
            &[
                "log",
                "-1",
                "--format=%s",
                &format!("{sha}^{{commit}}"),
                "--",
            ],
            PLAIN,
        )?;
        Ok(if code == 0 {
            Some(js_trim(stdout.split('\n').next().unwrap_or_default()).to_string())
        } else {
            None
        })
    }

    /// The commit HEAD points at, as a full sha1.
    pub fn head_commit(&self) -> IntegrateResult<String> {
        let (code, format) = self.text(&["rev-parse", "--show-object-format"], PLAIN)?;
        if code != 0 || js_trim(&format) != "sha1" {
            return Err(git_error("only sha1 repositories are supported"));
        }
        let (code, out) = self.text(
            &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"],
            PLAIN,
        )?;
        let sha = js_trim(&out).to_string();
        if code != 0 || !is_full_sha(&sha) {
            return Err(git_error("the project has no commit to integrate onto"));
        }
        Ok(sha)
    }

    /// `mergeIntoBranch`: merges the commits in order onto the base and creates the branch at one squash commit whose
    /// only parent is the base and whose tree is the result of the merges. Nothing is checked out.
    pub fn merge_into_branch(
        &self,
        base_sha: &str,
        branch: &str,
        subject: &str,
        body: &str,
        merges: &[(String, String)],
    ) -> IntegrateResult<MergeResult> {
        let failed = |reason: &str| {
            Ok(MergeResult::Failed {
                reason: reason.into(),
            })
        };
        let reference = format!("refs/heads/{branch}");
        if !is_full_sha(base_sha) || merges.iter().any(|(_, sha)| !is_full_sha(sha)) {
            return failed("a commit id is not a full sha1");
        }
        if self.run(&["check-ref-format", &reference], PLAIN)?.code != 0 {
            return failed("the branch name is not a valid ref");
        }
        for (report_id, sha) in merges {
            if !self.commit_exists(sha)? {
                return failed(&format!("the commit of report {report_id} does not exist"));
            }
        }
        let mut head = base_sha.to_string();
        let mut tree: Option<String> = None;
        for (report_id, sha) in merges {
            if self.is_ancestor(sha, &head)? {
                continue;
            }
            let merged = self.run(
                &[
                    "merge-tree",
                    "--write-tree",
                    "--name-only",
                    "--no-messages",
                    "-z",
                    &head,
                    sha,
                ],
                WRITE,
            )?;
            let fields: Vec<&[u8]> = merged.stdout.split(|b| *b == 0).collect();
            if merged.code == 1 {
                let end = fields
                    .iter()
                    .enumerate()
                    .skip(1)
                    .find(|(_, f)| f.is_empty())
                    .map(|(i, _)| i);
                let slice = &fields[1.min(fields.len())..end.unwrap_or(fields.len())];
                let mut names: Vec<&[u8]> = Vec::new();
                for name in slice {
                    if !name.is_empty() && !names.contains(name) {
                        names.push(name);
                    }
                }
                let files: Vec<String> = names
                    .iter()
                    .take(MAX_CONFLICT_FILES)
                    .map(|n| printable_path(n))
                    .collect();
                if !files.is_empty() {
                    return Ok(MergeResult::Conflicted {
                        report_id: report_id.clone(),
                        files,
                        omitted: names.len().saturating_sub(MAX_CONFLICT_FILES),
                    });
                }
            }
            let tree_field = fields.first().map(|f| utf8(f)).unwrap_or_default();
            if merged.code != 0 || !is_full_sha(&tree_field) {
                return Ok(MergeResult::Failed {
                    reason: if merged.code == 129 {
                        "git 2.38 or newer is needed to merge".into()
                    } else {
                        format!(
                            "git could not merge report {report_id} (exit {})",
                            merged.code
                        )
                    },
                });
            }
            let message = format!("Merge report {report_id}");
            let commit = self.run(
                &[
                    "-c",
                    "commit.gpgSign=false",
                    "commit-tree",
                    &tree_field,
                    "-p",
                    &head,
                    "-p",
                    sha,
                    "-m",
                    &message,
                ],
                WRITE,
            )?;
            let next = js_trim(&utf8(&commit.stdout)).to_string();
            if commit.code != 0 || !is_full_sha(&next) {
                return failed(&format!(
                    "git could not commit the merge of report {report_id}"
                ));
            }
            head = next;
            tree = Some(tree_field);
        }
        let Some(tree) = tree.filter(|_| head != base_sha) else {
            return failed("every report is already contained in the base commit");
        };
        let message = format!("{subject}\n\n{body}");
        let squash = self.run(
            &[
                "-c",
                "commit.gpgSign=false",
                "commit-tree",
                &tree,
                "-p",
                base_sha,
                "-m",
                &message,
            ],
            WRITE,
        )?;
        let squashed = js_trim(&utf8(&squash.stdout)).to_string();
        if squash.code != 0 || !is_full_sha(&squashed) {
            return failed("git could not commit the squashed integration");
        }
        let created = self.run(&["update-ref", &reference, &squashed, NO_COMMIT], WRITE)?;
        if created.code != 0 {
            return failed("git could not create the integration branch");
        }
        Ok(MergeResult::Merged { head_sha: squashed })
    }

    fn branch_checked_out(&self, branch: &str) -> IntegrateResult<bool> {
        let (code, out) = self.text(&["worktree", "list", "--porcelain", "-z"], PLAIN)?;
        if code != 0 {
            return Err(git_error("git could not list the worktrees"));
        }
        let wanted = format!("branch refs/heads/{branch}");
        Ok(out.split('\0').any(|field| field == wanted))
    }

    /// Deletes a branch only while it still points at `sha` and no worktree has it checked out.
    pub fn delete_branch_at(&self, branch: &str, sha: &str) -> IntegrateResult<bool> {
        let reference = format!("refs/heads/{branch}");
        if self.run(&["check-ref-format", &reference], PLAIN)?.code != 0 {
            return Ok(false);
        }
        if !is_full_sha(sha) {
            return Ok(false);
        }
        if self.branch_checked_out(branch)? {
            return Ok(false);
        }
        Ok(self
            .run(&["update-ref", "-d", &reference, sha], WRITE)?
            .code
            == 0)
    }

    /// The commit a branch points at, or `None` when the branch does not exist.
    pub fn branch_tip(&self, branch: &str) -> IntegrateResult<Option<String>> {
        let reference = format!("refs/heads/{branch}");
        if self.run(&["check-ref-format", &reference], PLAIN)?.code != 0 {
            return Ok(None);
        }
        let (code, out) = self.text(
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{reference}^{{commit}}"),
            ],
            PLAIN,
        )?;
        if code == 1 {
            return Ok(None);
        }
        let tip = js_trim(&out).to_string();
        if code != 0 || !is_full_sha(&tip) {
            return Err(git_error("git could not read the branch"));
        }
        Ok(Some(tip))
    }

    /// Whether the commit is already part of the project's HEAD.
    pub fn is_in_head(&self, sha: &str) -> IntegrateResult<bool> {
        if !is_full_sha(sha) {
            return Err(git_error(
                "the commit id must be 40 lowercase hex characters",
            ));
        }
        self.is_ancestor(sha, "HEAD")
    }

    /// Paths whose entry differs between two commits.
    fn changed_paths(&self, from: &str, to: &str) -> IntegrateResult<BTreeSet<Vec<u8>>> {
        let outcome = self.run(
            &[
                "diff-tree",
                "-r",
                "-z",
                "--no-renames",
                "--name-only",
                from,
                to,
            ],
            BIG,
        )?;
        if outcome.code != 0 {
            return Err(git_error("git could not compare the trees"));
        }
        Ok(outcome
            .stdout
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(<[u8]>::to_vec)
            .collect())
    }

    /// `git merge-tree --write-tree` needs git 2.38.
    fn supports_merge_tree(&self) -> bool {
        static SUPPORTED: OnceLock<bool> = OnceLock::new();
        *SUPPORTED.get_or_init(|| {
            let Ok((code, out)) = self.text(&["--version"], PLAIN) else {
                return false;
            };
            let Some(numbers) = out
                .split(|c: char| !c.is_ascii_digit() && c != '.')
                .find_map(|part| {
                    let mut it = part.split('.');
                    let major = it.next()?.parse::<u32>().ok()?;
                    let minor = it.next()?.parse::<u32>().ok()?;
                    Some((major, minor))
                })
            else {
                return false;
            };
            code == 0 && (numbers.0 > 2 || (numbers.0 == 2 && numbers.1 >= 38))
        })
    }

    /// Merging the report into the head: `same` means the clean merge equals the head, so the report adds nothing.
    fn merge_outcome(
        &self,
        head: &str,
        report_commit: &str,
        skipped: &mut dyn FnMut(&str, &str),
    ) -> IntegrateResult<&'static str> {
        if !self.supports_merge_tree() {
            skipped(
                report_commit,
                "git is older than 2.38, so the merge rule is off",
            );
            return Ok("off");
        }
        let (code, out) = self.text(&["merge-tree", "--write-tree", head, report_commit], BIG)?;
        if code == 1 {
            return Ok("conflict");
        }
        if code != 0 {
            return Ok("off");
        }
        let (tree_code, tree) = self.text(&["rev-parse", &format!("{head}^{{tree}}")], BIG)?;
        Ok(
            if tree_code == 0
                && js_trim(out.split('\n').next().unwrap_or_default()) == js_trim(&tree)
            {
                "same"
            } else {
                "different"
            },
        )
    }

    /// Fallback for a merge that conflicts only because the head changed next to the report's edit.
    fn report_contained_line_wise(&self, head: &str, report_commit: &str) -> IntegrateResult<bool> {
        let (code, base_out) = self.text(&["merge-base", report_commit, head], BIG)?;
        let base = js_trim(&base_out).to_string();
        if code != 0 || !is_full_sha(&base) {
            return Ok(false);
        }
        let own = self.changed_paths(&base, report_commit)?;
        if own.is_empty() {
            return Ok(false);
        }
        for path in own {
            // The path was read one character per byte, and goes back into an argument as UTF-8 text.
            let path = latin1(&path);
            let (in_report_code, in_report) =
                self.text(&["rev-parse", &format!("{report_commit}:{path}")], BIG)?;
            let (in_head_code, in_head) =
                self.text(&["rev-parse", &format!("{head}:{path}")], BIG)?;
            if in_report_code == 0 && in_report == in_head {
                continue;
            }
            if in_head_code != 0 || in_report_code != 0 {
                return Ok(false);
            }
            let (diff_code, diff) = self.text(
                &[
                    "diff",
                    "--unified=0",
                    "--no-color",
                    "--no-ext-diff",
                    &base,
                    report_commit,
                    "--",
                    &path,
                ],
                BIG,
            )?;
            let (head_code, head_text) = self.text(&["show", &format!("{head}:{path}")], BIG)?;
            if diff_code != 0 || head_code != 0 {
                return Ok(false);
            }
            let head_lines: BTreeSet<String> = meaningful_lines(&head_text).into_iter().collect();
            let lines: Vec<&str> = diff.split('\n').collect();
            let added = meaningful_lines(
                &lines
                    .iter()
                    .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
                    .map(|l| &l[1..])
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            let deleted = meaningful_lines(
                &lines
                    .iter()
                    .filter(|l| l.starts_with('-') && !l.starts_with("---"))
                    .map(|l| &l[1..])
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            if !added.iter().all(|l| head_holds(&head_lines, l)) {
                return Ok(false);
            }
            if deleted.iter().any(|l| head_lines.contains(l)) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// `coveredReports`: which reports an integration head already holds without having merged them. `skipped` hears of
    /// a report that could not be judged.
    pub fn covered_reports(
        &self,
        head: &str,
        reports: &[CoverageReport],
        member_commits: &[String],
        skipped: &mut dyn FnMut(&str, &str),
    ) -> IntegrateResult<Vec<CoveredReport>> {
        if !self.commit_exists(head)? {
            return Err(git_error("the integration head does not exist"));
        }
        let mut covered = Vec::new();
        for report in reports {
            let judged = (|| -> IntegrateResult<Option<&'static str>> {
                if !is_full_sha(&report.commit_sha) || !self.commit_exists(&report.commit_sha)? {
                    skipped(&report.report_id, "its commit does not exist");
                    return Ok(None);
                }
                let tips = std::iter::once(head).chain(member_commits.iter().map(String::as_str));
                let mut how: Option<&'static str> = None;
                for tip in tips {
                    if self.is_ancestor(&report.commit_sha, tip)? {
                        how = Some("ancestor");
                        break;
                    }
                }
                if how.is_none() {
                    let (code, out) =
                        self.text(&["merge-base", &report.commit_sha, head], PLAIN)?;
                    if code == 0 && is_full_sha(js_trim(&out)) {
                        let own = self.changed_paths(js_trim(&out), &report.commit_sha)?;
                        let drift = self.changed_paths(&report.commit_sha, head)?;
                        if own.iter().all(|path| !drift.contains(path)) {
                            how = Some("tree");
                        }
                    }
                }
                if how.is_none() {
                    let merge = self.merge_outcome(head, &report.commit_sha, skipped)?;
                    if merge == "same"
                        || (merge == "conflict"
                            && self.report_contained_line_wise(head, &report.commit_sha)?)
                    {
                        how = Some("merge");
                    }
                }
                // Chain: an earlier integration that held the report has a head that a member commit of this one
                // builds on, so the member carries the report's content forward.
                for earlier in &report.integration_heads {
                    if how.is_some() {
                        break;
                    }
                    if !self.commit_exists(earlier)? {
                        continue;
                    }
                    for tip in member_commits {
                        if self.is_ancestor(earlier, tip)? {
                            how = Some("integration");
                            break;
                        }
                    }
                }
                Ok(how)
            })();
            match judged {
                Ok(Some(how)) => covered.push(CoveredReport {
                    report_id: report.report_id.clone(),
                    how,
                }),
                Ok(None) => {}
                Err(error) => skipped(&report.report_id, &error.display()),
            }
        }
        Ok(covered)
    }
}

/// Lines that say something: blank and one- or two-character lines (braces) are ignored.
fn meaningful_lines(text: &str) -> Vec<String> {
    text.split('\n')
        .map(|line| js_trim(line).to_string())
        .filter(|line| js_len(line) >= 3)
        .collect()
}

fn line_tokens(line: &str) -> BTreeSet<&str> {
    line.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .filter(|t| !t.is_empty())
        .collect()
}

/// The head has the line, or a line that extends it: a later commit edited the same line and kept every word of it.
fn head_holds(head_lines: &BTreeSet<String>, line: &str) -> bool {
    if head_lines.contains(line) {
        return true;
    }
    let wanted = line_tokens(line);
    if wanted.len() < 4 {
        return false;
    }
    head_lines.iter().any(|head_line| {
        if js_len(head_line) <= js_len(line) {
            return false;
        }
        let have = line_tokens(head_line);
        wanted.iter().all(|t| have.contains(t))
    })
}

// ------------------------------------------------------------------------------------------------------ the pipeline

/// What the pipeline of one process remembers: the integrations being merged right now (a `running` row outside this
/// set belongs to a cut-off run) and the settled ones whose branch could not be deleted.
#[derive(Default)]
pub struct IntegrationState {
    in_flight: RefCell<BTreeSet<String>>,
    pending_sweep: RefCell<Vec<String>>,
}

impl IntegrationState {
    pub fn new() -> Self {
        Self::default()
    }

    fn forget(&self, integration_id: &str) {
        self.pending_sweep
            .borrow_mut()
            .retain(|id| id != integration_id);
    }

    fn remember(&self, integration_id: &str) {
        let mut pending = self.pending_sweep.borrow_mut();
        if !pending.iter().any(|id| id == integration_id) {
            pending.push(integration_id.to_string());
        }
    }
}

/// `IntegrationDeps`.
pub struct IntegrationDeps<'a> {
    pub kernel: &'a Kernel,
    pub git: &'a GitRepo,
    pub state: &'a IntegrationState,
    pub credential: &'a str,
    pub context: &'a dyn Fn(&str) -> MutationContext,
    pub log: &'a dyn Fn(&str, Value),
}

impl IntegrationDeps<'_> {
    fn ctx(&self) -> MutationContext {
        (self.context)(self.credential)
    }
}

fn integration_error(code: &str, message: String) -> IntegrateError {
    IntegrateError::Integration {
        code: code.into(),
        message,
    }
}

/// integration/<plan-id>-<slug>, or integration/<integration-id> without a common plan; -2, -3 ... while the name is a
/// live branch or an earlier integration's.
fn choose_branch(
    deps: &IntegrationDeps,
    integration_id: &str,
    info: &Value,
) -> IntegrateResult<String> {
    let base = match info.get("planId").and_then(Value::as_str) {
        None => integration_branch_name(integration_id, None),
        Some(plan_id) => {
            let title = text_of(info, "planTitle");
            let title = js_trim(strip_leading_type(title));
            integration_branch_name(plan_id, Some(title))
        }
    };
    for n in 1..1000 {
        let name = with_suffix(&base, n);
        if integrations::integration_branch_recorded(deps.kernel, &name)? == Value::Bool(false)
            && deps.git.branch_tip(&name)?.is_none()
        {
            return Ok(name);
        }
    }
    Ok(integration_branch_name(integration_id, None))
}

/// `integrate`: merges the reports in order and returns the recorded outcome. A refusal fails before anything is created.
pub fn integrate(
    deps: &IntegrationDeps,
    report_ids: &[String],
    requested_by: &str,
) -> IntegrateResult<Value> {
    recover_integrations(deps, Scope::Pending)?;
    let base_sha = deps.git.head_commit()?;
    let integration_id = deps.kernel.env.uuid();
    let planned = integrations::planned_commit_info(deps.kernel, &json!(report_ids))?;
    let branch = choose_branch(deps, &integration_id, &planned)?;
    let begun = integrations::begin_integration(
        deps.kernel,
        &deps.ctx(),
        &json!({
            "integrationId": integration_id,
            "reportIds": report_ids,
            "baseSha": base_sha,
            "branch": branch,
            "requestedBy": requested_by,
        }),
    )?;
    deps.state
        .in_flight
        .borrow_mut()
        .insert(integration_id.clone());
    let result = finish_merge(deps, &integration_id, &base_sha, &branch, &begun);
    deps.state.in_flight.borrow_mut().remove(&integration_id);
    result
}

fn finish_merge(
    deps: &IntegrationDeps,
    integration_id: &str,
    base_sha: &str,
    branch: &str,
    begun: &Value,
) -> IntegrateResult<Value> {
    let finish = |outcome: Value| -> Result<Value, KernelError> {
        integrations::finish_integration(
            deps.kernel,
            &deps.ctx(),
            &json!({"integrationId": integration_id, "outcome": outcome}),
        )
    };
    let members: Vec<(String, String)> = begun
        .get("reports")
        .and_then(Value::as_array)
        .map(|reports| {
            reports
                .iter()
                .map(|r| {
                    (
                        text_of(r, "reportId").to_string(),
                        text_of(r, "commitSha").to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let merge = || -> IntegrateResult<MergeResult> {
        let mut missing: Option<&str> = None;
        for (report_id, sha) in &members {
            if !deps.git.commit_exists(sha)? {
                missing = Some(report_id);
                break;
            }
        }
        if let Some(report_id) = missing {
            return Ok(MergeResult::Failed {
                reason: format!("the commit of report {report_id} no longer exists"),
            });
        }
        let mut subjects: HashMap<String, String> = HashMap::new();
        for (report_id, sha) in &members {
            if let Ok(Some(subject)) = deps.git.commit_subject(sha) {
                subjects.insert(report_id.clone(), subject);
            }
        }
        let info = integrations::integration_commit_info(deps.kernel, integration_id)?;
        let (subject, body) = squash_message(&info, &subjects);
        deps.git
            .merge_into_branch(base_sha, branch, &subject, &body, &members)
    };
    let result = match merge() {
        Ok(result) => result,
        Err(error) => {
            (deps.log)(
                "integration_merge_error",
                json!({"integrationId": integration_id, "error": error.display()}),
            );
            MergeResult::Failed {
                reason: "git could not run the merge".into(),
            }
        }
    };
    match finish(result.outcome()) {
        Ok(record) => Ok(record),
        Err(error) => {
            // The branch must not outlive a record that does not say it exists, and the row must not stay running.
            if let MergeResult::Merged { head_sha } = &result {
                let _ = deps.git.delete_branch_at(branch, head_sha);
            }
            let _ =
                finish(json!({"kind": "failed", "reason": "the outcome could not be recorded"}));
            Err(error.into())
        }
    }
}

/// `settleIntegration`: confirming needs the integrated commit to be part of the project's HEAD already (the PM or
/// operator merged the branch); the branch is then removed. Discarding removes the branch at once. Returns
/// `{record, branchRemoved}`.
pub fn settle_integration(
    deps: &IntegrationDeps,
    integration_id: &str,
    outcome: &str,
) -> IntegrateResult<Value> {
    let before = integrations::integration(deps.kernel, integration_id)?;
    let state = text_of(&before, "state").to_string();
    let head_sha = before
        .get("headSha")
        .and_then(Value::as_str)
        .map(str::to_string);
    let branch = text_of(&before, "branch").to_string();
    let Some(head_sha) = head_sha.filter(|_| state == "merged") else {
        return Err(integration_error(
            "not_merged_state",
            format!("the integration is {state}; only a merged one can be settled"),
        ));
    };
    if outcome == "confirmed" && !deps.git.is_in_head(&head_sha)? {
        return Err(integration_error(
            "not_in_head",
            format!("merge branch {branch} into the project's HEAD first, then confirm"),
        ));
    }
    let record = integrations::settle_integration(
        deps.kernel,
        &deps.ctx(),
        &json!({"integrationId": integration_id, "outcome": outcome}),
    )?;
    if outcome == "confirmed" {
        record_coverage(deps, integration_id);
    }
    let removed = deps
        .git
        .delete_branch_at(&branch, &head_sha)
        .unwrap_or(false);
    if !removed {
        deps.state.remember(integration_id);
        (deps.log)(
            "integration_branch_not_removed",
            json!({"integrationId": integration_id, "branch": branch}),
        );
    }
    Ok(json!({"record": record, "branchRemoved": removed}))
}

/// Stores the reports a confirmed integration covers without having merged them. A failure is logged and retried at
/// the next daemon start; it never blocks the confirm.
fn record_coverage(deps: &IntegrationDeps, integration_id: &str) {
    let attempt = || -> IntegrateResult<Option<Vec<CoveredReport>>> {
        let candidates =
            integrations::coverage_candidates(deps.kernel, deps.credential, integration_id)?;
        let reports: Vec<CoverageReport> = candidates
            .get("reports")
            .and_then(Value::as_array)
            .map(|reports| {
                reports
                    .iter()
                    .map(|r| CoverageReport {
                        report_id: text_of(r, "reportId").to_string(),
                        commit_sha: text_of(r, "commitSha").to_string(),
                        integration_heads: r
                            .get("integrationHeads")
                            .and_then(Value::as_array)
                            .map(|h| {
                                h.iter()
                                    .filter_map(Value::as_str)
                                    .map(str::to_string)
                                    .collect()
                            })
                            .unwrap_or_default(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        if candidates.is_null() || reports.is_empty() {
            return Ok(None);
        }
        let members: Vec<String> = candidates
            .get("memberCommits")
            .and_then(Value::as_array)
            .map(|c| {
                c.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let mut skipped = |report_id: &str, reason: &str| {
            (deps.log)(
                "integration_coverage_report_skipped",
                json!({"integrationId": integration_id, "reportId": report_id, "reason": reason}),
            );
        };
        let covered = deps.git.covered_reports(
            text_of(&candidates, "headSha"),
            &reports,
            &members,
            &mut skipped,
        )?;
        if covered.is_empty() {
            return Ok(None);
        }
        let rows: Vec<Value> = covered
            .iter()
            .map(|c| json!({"reportId": c.report_id, "how": c.how}))
            .collect();
        integrations::record_covered_reports(
            deps.kernel,
            deps.credential,
            integration_id,
            &json!(rows),
        )?;
        Ok(Some(covered))
    };
    match attempt() {
        Ok(Some(covered)) => (deps.log)(
            "integration_coverage_recorded",
            json!({
                "integrationId": integration_id,
                "covered": covered.iter().map(|c| c.report_id.as_str()).collect::<Vec<_>>(),
            }),
        ),
        Ok(None) => {}
        Err(error) => (deps.log)(
            "integration_coverage_failed",
            json!({"integrationId": integration_id, "error": error.display()}),
        ),
    }
}

/// Which settled rows `recover_integrations` looks at.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    Pending,
    All,
}

fn rows_of(value: Value) -> Vec<Value> {
    value.as_array().cloned().unwrap_or_default()
}

/// Removes the branch of a settled integration whose deletion failed earlier, while it still points at the recorded commit.
fn sweep_settled_branches(deps: &IntegrationDeps, scope: Scope) -> IntegrateResult<()> {
    let rows = match scope {
        Scope::All => rows_of(integrations::settled_integrations(
            deps.kernel,
            deps.credential,
            None,
        )?),
        Scope::Pending => {
            let ids = deps.state.pending_sweep.borrow().clone();
            let mut rows = Vec::new();
            for id in ids {
                rows.push(integrations::integration(deps.kernel, &id)?);
            }
            rows
        }
    };
    for row in rows {
        let Some(head_sha) = row.get("headSha").and_then(Value::as_str) else {
            continue;
        };
        let integration_id = text_of(&row, "integrationId");
        let branch = text_of(&row, "branch");
        let swept = (|| -> IntegrateResult<bool> {
            let tip = deps.git.branch_tip(branch)?;
            if tip.as_deref() != Some(head_sha) {
                deps.state.forget(integration_id);
                return Ok(false);
            }
            if deps.git.delete_branch_at(branch, head_sha)? {
                deps.state.forget(integration_id);
                return Ok(true);
            }
            Ok(false)
        })();
        match swept {
            Ok(true) => (deps.log)(
                "integration_branch_swept",
                json!({"integrationId": integration_id}),
            ),
            Ok(false) => {}
            Err(error) => (deps.log)(
                "integration_branch_sweep_failed",
                json!({"integrationId": integration_id, "error": error.display()}),
            ),
        }
    }
    Ok(())
}

/// `recoverIntegrations`: a `running` integration that this process is not merging was cut off: its branch is removed
/// and it is marked failed. Branches of settled integrations that could not be deleted earlier are swept first.
pub fn recover_integrations(deps: &IntegrationDeps, scope: Scope) -> IntegrateResult<()> {
    sweep_settled_branches(deps, scope)?;
    if scope == Scope::All {
        for row in rows_of(integrations::settled_integrations(
            deps.kernel,
            deps.credential,
            None,
        )?) {
            if text_of(&row, "state") == "confirmed" {
                record_coverage(deps, text_of(&row, "integrationId"));
            }
        }
    }
    for row in rows_of(integrations::running_integrations(
        deps.kernel,
        deps.credential,
    )?) {
        let integration_id = text_of(&row, "integrationId").to_string();
        if deps.state.in_flight.borrow().contains(&integration_id) {
            continue;
        }
        let branch = text_of(&row, "branch").to_string();
        let recovered = (|| -> IntegrateResult<()> {
            if let Some(tip) = deps.git.branch_tip(&branch)? {
                if !deps.git.delete_branch_at(&branch, &tip)? {
                    (deps.log)(
                        "integration_branch_not_removed",
                        json!({"integrationId": integration_id, "branch": branch}),
                    );
                }
            }
            integrations::finish_integration(
                deps.kernel,
                &deps.ctx(),
                &json!({
                    "integrationId": integration_id,
                    "outcome": {
                        "kind": "failed",
                        "reason": "the daemon stopped while the integration was merging",
                    },
                }),
            )?;
            (deps.log)(
                "integration_recovered",
                json!({"integrationId": integration_id}),
            );
            Ok(())
        })();
        if let Err(error) = recovered {
            (deps.log)(
                "integration_not_recovered",
                json!({"integrationId": integration_id, "error": error.display()}),
            );
        }
    }
    Ok(())
}
