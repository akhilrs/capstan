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
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
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

const SUBJECT_SHAPE: &str =
    "subject must look like 'type(scope)!: description' with a space after the colon";

/// `parseCommitSubject`: the parsed subject, or the reason it breaks `subject-format`.
pub fn parse_commit_subject_reason(subject: &str) -> Result<ParsedSubject, String> {
    let shape = || SUBJECT_SHAPE.to_string();
    // /^([a-z]+)(?:\(([^()\s]*)\))?(!)?: (.*)$/
    let kind_end = subject
        .find(|c: char| !c.is_ascii_lowercase())
        .unwrap_or(subject.len());
    if kind_end == 0 {
        return Err(shape());
    }
    let kind = &subject[..kind_end];
    let mut rest = &subject[kind_end..];
    let mut scope: Option<String> = None;
    if let Some(inner) = rest.strip_prefix('(') {
        let close = inner
            .find(|c: char| matches!(c, '(' | ')') || is_js_space(c))
            .ok_or_else(shape)?;
        if !inner[close..].starts_with(')') {
            return Err(shape());
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
    let description = rest.strip_prefix(": ").ok_or_else(shape)?;
    if description.chars().any(is_line_terminator) {
        return Err(shape());
    }
    if !COMMIT_TYPES.contains(&kind) {
        return Err(format!(
            "unknown type '{kind}'; use one of {}",
            COMMIT_TYPES.join(", ")
        ));
    }
    if scope.as_deref() == Some("") {
        return Err("scope must not be empty".to_string());
    }
    if js_trim(description).is_empty() || description.starts_with(is_js_space) {
        return Err("description must start right after one space and not be empty".to_string());
    }
    Ok(ParsedSubject {
        kind: kind.to_string(),
        scope,
        breaking,
        description: description.to_string(),
    })
}

/// `parseCommitSubject` (the `ok` case).
pub fn parse_commit_subject(subject: &str) -> Option<ParsedSubject> {
    parse_commit_subject_reason(subject).ok()
}

/// `checkCommitMessage`: the rules a message breaks, as `(rule, reason)`.
pub fn check_commit_message(message: &str, parents: usize) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    let normalized = message.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    if parents < 2 {
        if let Err(reason) = parse_commit_subject_reason(lines[0]) {
            out.push(("subject-format", reason));
        }
        if lines.len() > 1 && !js_trim(lines[1]).is_empty() {
            out.push((
                "body-separation",
                "leave a blank line between the subject and the body".to_string(),
            ));
        }
    }
    if lines.iter().any(|l| is_claude_co_author(l)) {
        out.push((
            "claude-co-author",
            "remove the Claude Co-Authored-By line".to_string(),
        ));
    }
    if lines.iter().any(|l| {
        l.trim_start_matches(is_js_space)
            .to_lowercase()
            .starts_with("claude-session:")
    }) {
        out.push((
            "claude-session",
            "remove the Claude-Session line".to_string(),
        ));
    }
    if lines.iter().any(|l| {
        let lower = l.to_lowercase();
        lower.contains("generated with claude code")
            || lower.contains("generated with [claude code")
            || l.contains('\u{1F916}')
    }) {
        out.push((
            "claude-code-footer",
            "remove the 'Generated with Claude Code' footer".to_string(),
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
    // A long title is cut at a word boundary in the subject; the whole of it goes first in the body.
    if !subject.ends_with(&description) {
        lines.insert(0, format!("Title: {description}"));
        lines.insert(1, String::new());
    }
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
const DEFAULT_NAME: &str = "capstan";
const DEFAULT_EMAIL: &str = "capstan@localhost";

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
    seams: GitSeams,
    /// The absolute path of git, resolved by the first `run_root` call of this repository (and its clones).
    git_path: Arc<OnceLock<PathBuf>>,
    /// The probed git version (major, minor), filled by the first successful probe.
    version: Arc<OnceLock<(u32, u32)>>,
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

// ------------------------------------------------------------------------------------------------------ root runner

/// The git version a `run_root` call needs at least.
const MIN_GIT: (u32, u32) = (2, 39);
const ROOT_STDOUT_CAP: usize = 16 * 1024 * 1024;
const PROBE_STDOUT_CAP: usize = 4 * 1024;
const ROOT_STDERR_CAP: usize = 64 * 1024;
const DEFAULT_READER_DETACH: Duration = Duration::from_secs(2);

/// The names every `run_root` child gets (PATH and the fixed values); nothing else is passed except
/// `FORWARDED_USER_ENV` with `user_config`, the `GIT_CONFIG_*` of `config_env` and the two pins of a call without it.
pub const FIXED_CHILD_ENV: &[&str] = &[
    "PATH",
    "LC_ALL",
    "GIT_TERMINAL_PROMPT",
    "GCM_INTERACTIVE",
    "GIT_EDITOR",
    "GIT_PAGER",
    "PAGER",
];

/// The user's environment a call with `user_config` forwards when it is set. The whole contract.
pub const FORWARDED_USER_ENV: &[&str] = &[
    "HOME",
    "XDG_CONFIG_HOME",
    "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_SYSTEM",
    "GIT_CONFIG_NOSYSTEM",
    "SSH_AUTH_SOCK",
    "TMPDIR",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "all_proxy",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
];

/// The settings passed as `-c` on every `run_root` call; nothing in the user's or the repository's config changes them.
pub const ROOT_CONFIG_OVERRIDES: &[(&str, &str)] = &[
    ("core.hooksPath", "/dev/null"),
    ("core.fsmonitor", "false"),
    ("commit.gpgSign", "false"),
    ("push.gpgSign", "false"),
    ("push.pushOption", ""),
    ("gc.auto", "0"),
    ("maintenance.auto", "false"),
    ("submodule.recurse", "false"),
    ("protocol.allow", "never"),
    ("protocol.ext.allow", "never"),
    ("protocol.fd.allow", "never"),
    ("protocol.git.allow", "never"),
    ("protocol.http.allow", "never"),
    ("protocol.file.allow", "user"),
    ("protocol.https.allow", "always"),
    ("protocol.ssh.allow", "always"),
];

/// Global and transport options that can override the pins or run a program.
const REFUSED_OPTIONS: &[&str] = &[
    "-c",
    "-C",
    "--config-env",
    "--exec-path",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--upload-pack",
    "--receive-pack",
    "--exec",
];

/// Short options refused with their attached form too (`-ufoo`): `-u` is the upload-pack program.
const REFUSED_SHORT_PREFIXES: &[&str] = &["-u"];

/// One `run_root` call.
#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct RootCall {
    pub args: Vec<String>,
    pub timeout: Duration,
    pub config_env: Vec<(String, String)>,
    pub network: bool,
    pub user_config: bool,
}

/// What a `run_root` child printed; `stderr` is cut to 64 KiB.
#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct RootOutput {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Switches for tests, all off by default. They travel with a clone and `with_env`, not with `new` or
/// `worktree_dirty`.
#[doc(hidden)]
#[derive(Clone, Debug, Default)]
pub struct GitSeams {
    pub force_old_git: bool,
    pub push_timeout: Option<Duration>,
    pub probe_timeout: Option<Duration>,
    pub allow_http: bool,
    pub forward_env: Option<Vec<(String, String)>>,
    pub git_program: Option<PathBuf>,
    pub child_path: Option<String>,
    pub reader_detach: Option<Duration>,
    pub skip_version_probe: bool,
    pub skip_ssh_probe: bool,
}

/// The time one call has in all.
pub(crate) struct Deadline {
    until: Instant,
}

impl Deadline {
    pub(crate) fn new(total: Duration) -> Deadline {
        Deadline {
            until: Instant::now() + total,
        }
    }

    /// The least of `per_child` and the time left; `None` once the deadline has passed.
    pub(crate) fn child_timeout(&self, per_child: Duration) -> Option<Duration> {
        let left = self.until.checked_duration_since(Instant::now())?;
        if left.is_zero() {
            return None;
        }
        Some(per_child.min(left))
    }
}

fn unavailable(message: impl Into<String>) -> IntegrateError {
    integration_error("git_unavailable", message.into())
}

/// Whether `key` is one a call may not set: a pin or `core.sshCommand`. Section and variable compare without case,
/// the subsection exactly, as git does.
fn is_pinned_key(key: &str) -> bool {
    fn same(a: &str, b: &str) -> bool {
        let split = |k: &str| -> (String, Option<String>, String) {
            let first = k.find('.');
            let last = k.rfind('.');
            match (first, last) {
                (Some(f), Some(l)) if f != l => (
                    k[..f].to_ascii_lowercase(),
                    Some(k[f + 1..l].to_string()),
                    k[l + 1..].to_ascii_lowercase(),
                ),
                (Some(f), _) => (
                    k[..f].to_ascii_lowercase(),
                    None,
                    k[f + 1..].to_ascii_lowercase(),
                ),
                _ => (k.to_ascii_lowercase(), None, String::new()),
            }
        };
        split(a) == split(b)
    }
    ROOT_CONFIG_OVERRIDES
        .iter()
        .map(|(name, _)| *name)
        .chain(std::iter::once("core.sshCommand"))
        .any(|pinned| same(pinned, key))
}

fn refused_option(arg: &str) -> bool {
    if REFUSED_SHORT_PREFIXES
        .iter()
        .any(|short| arg.starts_with(short) && !arg.starts_with("--"))
    {
        return true;
    }
    // Git takes any unambiguous abbreviation of a long option, so a name that is a prefix of a refused one is refused.
    let name = arg.split('=').next().unwrap_or(arg);
    REFUSED_OPTIONS.iter().any(|option| {
        arg == *option
            || (option.starts_with("--")
                && name.len() > 2
                && name.starts_with("--")
                && option.starts_with(name))
    })
}

/// A git boolean that is true: `true`, `yes`, `on` or a non-zero integer, without case.
fn git_true(value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    matches!(value.as_str(), "true" | "yes" | "on")
        || value.parse::<i64>().is_ok_and(|number| number != 0)
}

fn bad_text(text: &str) -> bool {
    text.contains('\0') || text.contains('\n')
}

/// The first x.y[.z] after "git version ": (major, minor).
fn parse_git_version(text: &str) -> Option<(u32, u32)> {
    let rest = text.trim_start().strip_prefix("git version ")?;
    let token: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let mut parts = token.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// Reads one pipe on a thread, keeping at most `keep` bytes; with `overflow_on_excess` a byte past `keep` stops the
/// read and raises the flag, else the rest is read and dropped.
struct Capture {
    buffer: Arc<Mutex<Vec<u8>>>,
    overflow: Arc<AtomicBool>,
    handle: std::thread::JoinHandle<()>,
}

fn capture<R: Read + Send + 'static>(
    mut pipe: R,
    keep: usize,
    overflow_on_excess: bool,
) -> Capture {
    let buffer = Arc::new(Mutex::new(Vec::<u8>::new()));
    let overflow = Arc::new(AtomicBool::new(false));
    let handle = {
        let buffer = buffer.clone();
        let overflow = overflow.clone();
        std::thread::spawn(move || {
            let mut chunk = [0u8; 16384];
            loop {
                match pipe.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let mut held = buffer.lock().expect("the buffer lock");
                        let room = keep - held.len();
                        held.extend_from_slice(&chunk[..n.min(room)]);
                        if n > room && overflow_on_excess {
                            overflow.store(true, Ordering::SeqCst);
                            break;
                        }
                    }
                }
            }
        })
    };
    Capture {
        buffer,
        overflow,
        handle,
    }
}

/// What the runner decided before the leader's own status was looked at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Decided {
    Timeout,
    TooLarge,
}

/// The negated process id that `kill` takes for the group led by `pid`; `None` when it does not fit.
fn group_target(pid: u32) -> Option<i32> {
    i32::try_from(pid)
        .ok()
        .filter(|pid| *pid > 0)
        .map(|pid| -pid)
}

/// The result of a call from the leader's status: a decided timeout or cap wins whatever the status says; a leader
/// killed by a signal is `killed`; stdout short of end of file is `output_incomplete`; else the exit code.
fn final_code(
    decided: Option<Decided>,
    status: std::process::ExitStatus,
    stdout_eof: bool,
) -> Result<i32, &'static str> {
    match decided {
        Some(Decided::Timeout) => return Err("timeout"),
        Some(Decided::TooLarge) => return Err("output_too_large"),
        None => {}
    }
    let Some(code) = status.code() else {
        return Err("killed");
    };
    if !stdout_eof {
        return Err("output_incomplete");
    }
    Ok(code)
}

impl Capture {
    /// Waits up to `detach` for the reader to end, then takes what it has and whether it ended; a reader still blocked
    /// is left behind.
    fn settle(&self, detach: Duration) -> (Vec<u8>, bool) {
        let until = Instant::now() + detach;
        while !self.handle.is_finished() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        let finished = self.handle.is_finished();
        let held = std::mem::take(&mut *self.buffer.lock().expect("the buffer lock"));
        (held, finished)
    }
}

impl GitRepo {
    fn detach(&self) -> Duration {
        self.seams.reader_detach.unwrap_or(DEFAULT_READER_DETACH)
    }

    /// The environment of the user that a call with `user_config` forwards, limited to `FORWARDED_USER_ENV`.
    fn user_env(&self) -> Vec<(String, String)> {
        let source: Vec<(String, String)> = match &self.seams.forward_env {
            Some(list) => list.clone(),
            None => std::env::vars().collect(),
        };
        source
            .into_iter()
            .filter(|(name, _)| FORWARDED_USER_ENV.contains(&name.as_str()))
            .collect()
    }

    fn fixed_env(&self) -> Vec<(String, String)> {
        let path = self
            .seams
            .child_path
            .clone()
            .or_else(|| std::env::var("PATH").ok())
            .unwrap_or_else(|| "/usr/bin:/bin".into());
        vec![
            ("PATH".into(), path),
            ("LC_ALL".into(), "C".into()),
            ("GIT_TERMINAL_PROMPT".into(), "0".into()),
            ("GCM_INTERACTIVE".into(), "never".into()),
            ("GIT_EDITOR".into(), "true".into()),
            ("GIT_PAGER".into(), "cat".into()),
            ("PAGER".into(), "cat".into()),
        ]
    }

    /// The absolute path of git: the `git_program` seam, else `git` found on the child PATH. Resolved once.
    fn git_program(&self) -> IntegrateResult<PathBuf> {
        if let Some(path) = self.git_path.get() {
            return Ok(path.clone());
        }
        let found = match &self.seams.git_program {
            Some(program) => program.clone(),
            None => {
                let path = self
                    .seams
                    .child_path
                    .clone()
                    .or_else(|| std::env::var("PATH").ok())
                    .unwrap_or_else(|| "/usr/bin:/bin".into());
                std::env::split_paths(&path)
                    .map(|dir| dir.join("git"))
                    .find(|candidate| candidate.is_file())
                    .ok_or_else(|| unavailable("git was not found on PATH"))?
            }
        };
        if !found.is_file() {
            return Err(unavailable(format!(
                "git could not be run ({} was not found)",
                found.display()
            )));
        }
        let absolute = std::path::absolute(&found)
            .map_err(|error| unavailable(format!("git could not be run ({error})")))?;
        Ok(self.git_path.get_or_init(|| absolute).clone())
    }

    /// The only place that spawns a child for `run_root`, the version probe and the ssh probes. The child starts in a
    /// new session and is its own process group; the group is killed on every exit path and the leader is always
    /// reaped before this returns.
    fn spawn_capped(
        &self,
        program: &Path,
        program_args: &[OsString],
        env: &[(String, String)],
        timeout: Duration,
        stdout_cap: usize,
    ) -> IntegrateResult<RootOutput> {
        let mut command = Command::new(program);
        command
            .args(program_args)
            .env_clear()
            .envs(env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // SAFETY: the closure runs between fork and exec and calls only setsid, which is async-signal-safe.
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        // A just-written executable can still be open for writing in another thread's forked child (ETXTBSY).
        let mut attempt = 1;
        let mut child = loop {
            match command.spawn() {
                Ok(child) => break child,
                Err(error) if attempt < 5 && error.raw_os_error() == Some(libc::ETXTBSY) => {
                    attempt += 1;
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => {
                    return Err(unavailable(format!("git could not be run ({error})")));
                }
            }
        };
        let pid = child.id();
        let out = capture(
            child.stdout.take().expect("stdout is piped"),
            stdout_cap,
            true,
        );
        let err = capture(
            child.stderr.take().expect("stderr is piped"),
            ROOT_STDERR_CAP,
            false,
        );
        let detach = self.detach();
        let Some(target) = group_target(pid) else {
            let _ = child.kill();
            let _ = child.wait();
            out.settle(detach);
            err.settle(detach);
            return Err(unavailable(
                "git could not be run (its process id is out of range)",
            ));
        };
        let started = Instant::now();
        let mut decided: Option<Decided> = None;
        let mut failure: Option<IntegrateError> = None;
        #[cfg_attr(target_os = "linux", allow(unused_mut))]
        let mut reaped: Option<std::process::ExitStatus> = None;
        loop {
            if out.overflow.load(Ordering::SeqCst) {
                decided = Some(Decided::TooLarge);
                break;
            }
            #[cfg(target_os = "linux")]
            {
                // SAFETY: an all-zero siginfo_t is a valid value, and waitid only writes to it.
                let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
                // SAFETY: `info` is a live siginfo_t; WNOWAIT leaves the leader unreaped, so its pgid stays ours.
                let rc = unsafe {
                    libc::waitid(
                        libc::P_PID,
                        pid as libc::id_t,
                        &mut info,
                        libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                    )
                };
                if rc == 0 {
                    // SAFETY: si_pid is read after a successful waitid.
                    if unsafe { info.si_pid() } != 0 {
                        break;
                    }
                } else {
                    let error = std::io::Error::last_os_error();
                    if error.kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    failure = Some(unavailable(format!("git could not be run ({error})")));
                    break;
                }
            }
            #[cfg(not(target_os = "linux"))]
            match child.try_wait() {
                Ok(Some(status)) => {
                    reaped = Some(status);
                    break;
                }
                Ok(None) => {}
                Err(error) => {
                    failure = Some(unavailable(format!("git could not be run ({error})")));
                    break;
                }
            }
            if started.elapsed() >= timeout {
                decided = Some(Decided::Timeout);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // The group dies on every path; ESRCH (nothing left) and EPERM are ignored.
        // SAFETY: kill takes no pointers; `target` is the negated pid of the child we started.
        unsafe {
            libc::kill(target, libc::SIGKILL);
        }
        let status = match reaped {
            Some(status) => Ok(status),
            None => child.wait(),
        };
        let (stdout, stdout_eof) = out.settle(detach);
        let flooded = out.overflow.load(Ordering::SeqCst);
        let (stderr, _) = err.settle(detach);
        if let Some(error) = failure {
            return Err(error);
        }
        let status =
            status.map_err(|error| unavailable(format!("git could not be run ({error})")))?;
        if decided.is_none() && flooded {
            decided = Some(Decided::TooLarge);
        }
        match final_code(decided, status, stdout_eof && !flooded) {
            Ok(code) => Ok(RootOutput {
                code,
                stdout,
                stderr,
            }),
            Err("timeout") => Err(integration_error("timeout", "git timed out".into())),
            Err("output_too_large") => Err(integration_error(
                "output_too_large",
                "git printed too much".into(),
            )),
            Err("killed") => Err(integration_error("killed", "git was killed".into())),
            Err(code) => Err(integration_error(
                code,
                "git did not finish its output".into(),
            )),
        }
    }

    /// The git version, probed once per repository and its clones; at least `MIN_GIT`, else `git_unavailable`.
    fn check_version(&self, program: &Path, deadline: &Deadline) -> IntegrateResult<()> {
        if self.seams.force_old_git {
            return Err(unavailable("git is older than 2.39"));
        }
        if self.seams.skip_version_probe {
            return Ok(());
        }
        if self.version.get().is_some() {
            return Ok(());
        }
        let per_child = self.seams.probe_timeout.unwrap_or(GIT_TIMEOUT);
        let timeout = deadline
            .child_timeout(per_child)
            .ok_or_else(|| integration_error("timeout", "git timed out".into()))?;
        let output = self.spawn_capped(
            program,
            &[OsString::from("--version")],
            &self.fixed_env(),
            timeout,
            PROBE_STDOUT_CAP,
        )?;
        let version = Some(output.code)
            .filter(|code| *code == 0)
            .and_then(|_| parse_git_version(&utf8(&output.stdout)))
            .ok_or_else(|| unavailable("the version of git could not be read"))?;
        if version < MIN_GIT {
            return Err(unavailable(format!(
                "git {}.{} is older than {}.{}",
                version.0, version.1, MIN_GIT.0, MIN_GIT.1
            )));
        }
        let _ = self.version.set(version);
        Ok(())
    }

    /// Refuses a call that must not reach git.
    fn check_call(call: &RootCall) -> IntegrateResult<()> {
        let Some(subcommand) = call.args.first() else {
            return Err(unavailable("a git call needs a subcommand"));
        };
        if call.args.iter().any(|arg| bad_text(arg)) {
            return Err(unavailable("a git argument holds a NUL or a newline"));
        }
        // `rev-parse --git-dir` and `--git-common-dir` only print a path; for every other call they select the repository.
        let query = |arg: &str| {
            subcommand == "rev-parse" && matches!(arg, "--git-dir" | "--git-common-dir")
        };
        if let Some(arg) = call
            .args
            .iter()
            .find(|arg| !query(arg) && refused_option(arg))
        {
            return Err(unavailable(format!("the git option {arg} is not allowed")));
        }
        for (key, value) in &call.config_env {
            if bad_text(key) || bad_text(value) {
                return Err(unavailable("a git setting holds a NUL or a newline"));
            }
            if is_pinned_key(key) {
                return Err(unavailable(format!("the git setting {key} is pinned")));
            }
        }
        let networked = matches!(subcommand.as_str(), "push" | "ls-remote");
        if call.network != networked {
            return Err(unavailable(format!(
                "git {subcommand} does not match network={}",
                call.network
            )));
        }
        if call.network && !call.user_config {
            return Err(unavailable("a network call needs the user's git config"));
        }
        Ok(())
    }

    /// Runs git in the project root with the pins of `ROOT_CONFIG_OVERRIDES` and the environment of `call`.
    /// `extra_env` is not applied.
    pub(crate) fn run_root(&self, call: &RootCall) -> IntegrateResult<RootOutput> {
        Self::check_call(call)?;
        let deadline = Deadline::new(call.timeout);
        let program = self.git_program()?;
        self.check_version(&program, &deadline)?;

        let mut env = self.fixed_env();
        if call.user_config {
            env.extend(self.user_env());
        } else {
            env.push(("GIT_CONFIG_NOSYSTEM".into(), "1".into()));
            env.push(("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()));
        }
        if !call.config_env.is_empty() {
            env.push(("GIT_CONFIG_COUNT".into(), call.config_env.len().to_string()));
            for (index, (key, value)) in call.config_env.iter().enumerate() {
                env.push((format!("GIT_CONFIG_KEY_{index}"), key.clone()));
                env.push((format!("GIT_CONFIG_VALUE_{index}"), value.clone()));
            }
        }
        let mut args = self.root_base_args();
        if call.network && !self.seams.skip_ssh_probe {
            let ssh_command = self.ssh_command(&program, &env, &deadline)?;
            args.push("-c".into());
            args.push(format!("core.sshCommand={ssh_command}").into());
        }
        args.extend(call.args.iter().map(OsString::from));
        let timeout = deadline
            .child_timeout(call.timeout)
            .ok_or_else(|| integration_error("timeout", "git timed out".into()))?;
        self.spawn_capped(&program, &args, &env, timeout, ROOT_STDOUT_CAP)
    }

    /// The arguments every `run_root` child starts with: the repository and the pins.
    fn root_base_args(&self) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec![
            "--no-replace-objects".into(),
            "-C".into(),
            self.root.clone().into(),
        ];
        for (key, value) in ROOT_CONFIG_OVERRIDES {
            let value = if self.seams.allow_http && *key == "protocol.http.allow" {
                "always"
            } else {
                value
            };
            args.push("-c".into());
            args.push(format!("{key}={value}").into());
        }
        args
    }

    /// The `core.sshCommand` a network call runs with: the user's global value, else the system one, else
    /// `ssh -o BatchMode=yes`. Read on every call, so a change in the user's config is seen at once.
    fn ssh_command(
        &self,
        program: &Path,
        env: &[(String, String)],
        deadline: &Deadline,
    ) -> IntegrateResult<String> {
        let nosystem = env
            .iter()
            .find(|(name, _)| name == "GIT_CONFIG_NOSYSTEM")
            .is_some_and(|(_, value)| git_true(value));
        let scopes: &[&str] = if nosystem {
            &["--global"]
        } else {
            &["--global", "--system"]
        };
        for scope in scopes {
            if let Some(value) = self.read_ssh_command(program, env, deadline, scope)? {
                return Ok(value);
            }
        }
        Ok("ssh -o BatchMode=yes".into())
    }

    fn read_ssh_command(
        &self,
        program: &Path,
        env: &[(String, String)],
        deadline: &Deadline,
        scope: &str,
    ) -> IntegrateResult<Option<String>> {
        let per_child = self.seams.probe_timeout.unwrap_or(GIT_TIMEOUT);
        let timeout = deadline
            .child_timeout(per_child)
            .filter(|timeout| !timeout.is_zero())
            .ok_or_else(|| integration_error("timeout", "git timed out".into()))?;
        let mut args = self.root_base_args();
        for word in [
            "config",
            "-z",
            "--includes",
            scope,
            "--get",
            "core.sshCommand",
        ] {
            args.push(word.into());
        }
        let output = self.spawn_capped(program, &args, env, timeout, PROBE_STDOUT_CAP)?;
        match output.code {
            1 => Ok(None),
            0 => {
                let mut bytes = output.stdout;
                if bytes.last() == Some(&0) {
                    bytes.pop();
                }
                let value = String::from_utf8(bytes)
                    .map_err(|_| unavailable("core.sshCommand is not valid text"))?;
                if bad_text(&value) {
                    return Err(unavailable("core.sshCommand holds a NUL or a newline"));
                }
                Ok(Some(value))
            }
            code => Err(unavailable(format!(
                "core.sshCommand could not be read (git exited {code})"
            ))),
        }
    }

    /// `run_root` for tests.
    #[doc(hidden)]
    pub fn root_git(&self, call: &RootCall) -> IntegrateResult<RootOutput> {
        self.run_root(call)
    }
}

/// One entry of `git worktree list`.
#[derive(Clone, Debug, PartialEq)]
pub struct Worktree {
    pub path: PathBuf,
    pub head: String,
    pub branch: Option<String>,
    pub is_main: bool,
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
            seams: GitSeams::default(),
            git_path: Arc::new(OnceLock::new()),
            version: Arc::new(OnceLock::new()),
        }
    }

    /// Sets the test seams; the path of git is resolved again under them.
    #[doc(hidden)]
    pub fn with_seams(mut self, seams: GitSeams) -> Self {
        self.seams = seams;
        self.git_path = Arc::new(OnceLock::new());
        self.version = Arc::new(OnceLock::new());
        self
    }

    pub fn with_env(mut self, extra_env: Vec<(String, String)>) -> Self {
        self.extra_env = extra_env;
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The author and committer of the commits the pipeline writes: the project's `user.name` and `user.email`, else
    /// the global ones, else `capstan` / `capstan@localhost`. The two resolve on their own, as git does for a commit.
    /// An empty value counts as unset, so it falls through to the next level.
    fn identity(&self) -> (String, String) {
        (
            self.config_value("user.name")
                .unwrap_or_else(|| DEFAULT_NAME.into()),
            self.config_value("user.email")
                .unwrap_or_else(|| DEFAULT_EMAIL.into()),
        )
    }

    /// One setting from the repository's config, then the global config (no system config). A failed lookup is an
    /// unset value, never an error or a prompt.
    fn config_value(&self, key: &str) -> Option<String> {
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(&self.root)
            .args(["config", "--get-all", key])
            .env_clear()
            .env("LC_ALL", "C")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for name in ["PATH", "HOME", "XDG_CONFIG_HOME", "GIT_CONFIG_GLOBAL"] {
            if let Ok(value) = std::env::var(name) {
                command.env(name, value);
            }
        }
        for (name, value) in &self.extra_env {
            command.env(name, value);
        }
        let mut child = command.spawn().ok()?;
        let mut pipe = child.stdout.take()?;
        let reader = std::thread::spawn(move || {
            let mut out = Vec::new();
            let _ = pipe.by_ref().take(4096).read_to_end(&mut out);
            out
        });
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if started.elapsed() < GIT_TIMEOUT => {
                    std::thread::sleep(Duration::from_millis(2));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
            }
        }
        let out = reader.join().ok()?;
        // The last value wins in git; skipping the empty ones lets an empty project value fall through.
        utf8(&out)
            .lines()
            .map(js_trim)
            .rfind(|line| !line.is_empty())
            .map(str::to_string)
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
            let (name, email) = self.identity();
            command
                .env("GIT_AUTHOR_NAME", &name)
                .env("GIT_AUTHOR_EMAIL", &email)
                .env("GIT_COMMITTER_NAME", &name)
                .env("GIT_COMMITTER_EMAIL", &email);
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

    /// The local branches (refs/heads only) and the commits they point at, sorted by name. Remote refs are never read.
    pub fn local_branches(&self) -> IntegrateResult<Vec<(String, String)>> {
        let (code, out) = self.text(
            &[
                "for-each-ref",
                "--format=%(objectname) %(refname)",
                "refs/heads",
            ],
            PLAIN,
        )?;
        if code != 0 {
            return Err(git_error("git could not list the branches"));
        }
        let mut branches: Vec<(String, String)> = out
            .lines()
            .filter_map(|line| {
                let (sha, name) = line.split_once(' ')?;
                let branch = name.strip_prefix("refs/heads/")?;
                is_full_sha(sha).then(|| (branch.to_string(), sha.to_string()))
            })
            .collect();
        branches.sort();
        Ok(branches)
    }

    /// The branch checked out in the project root, or `None` for a detached HEAD.
    pub fn root_branch(&self) -> IntegrateResult<Option<String>> {
        let (code, out) = self.text(&["symbolic-ref", "-q", "HEAD"], PLAIN)?;
        if code == 1 {
            return Ok(None);
        }
        if code != 0 {
            return Err(git_error("git could not read HEAD"));
        }
        Ok(js_trim(&out)
            .strip_prefix("refs/heads/")
            .map(str::to_string))
    }

    /// The branch refs/remotes/origin/HEAD points at (the remote's default branch), if the repository has one.
    pub fn origin_head_branch(&self) -> IntegrateResult<Option<String>> {
        let (code, out) = self.text(&["symbolic-ref", "-q", "refs/remotes/origin/HEAD"], PLAIN)?;
        if code != 0 {
            return Ok(None);
        }
        Ok(js_trim(&out)
            .strip_prefix("refs/remotes/origin/")
            .map(str::to_string))
    }

    /// The worktrees of the repository; the first one is the main checkout.
    pub fn worktrees(&self) -> IntegrateResult<Vec<Worktree>> {
        let (code, out) = self.text(&["worktree", "list", "--porcelain", "-z"], PLAIN)?;
        if code != 0 {
            return Err(git_error("git could not list the worktrees"));
        }
        let mut list: Vec<Worktree> = Vec::new();
        for field in out.split('\0') {
            if let Some(path) = field.strip_prefix("worktree ") {
                list.push(Worktree {
                    path: PathBuf::from(path),
                    head: String::new(),
                    branch: None,
                    is_main: list.is_empty(),
                });
            } else if let Some(current) = list.last_mut() {
                if let Some(head) = field.strip_prefix("HEAD ") {
                    current.head = head.to_string();
                } else if let Some(branch) = field.strip_prefix("branch refs/heads/") {
                    current.branch = Some(branch.to_string());
                }
            }
        }
        Ok(list)
    }

    /// Whether the worktree has changes to tracked files or untracked files.
    pub fn worktree_dirty(&self, path: &Path) -> IntegrateResult<bool> {
        let mut repo = GitRepo::new(path);
        repo.extra_env = self.extra_env.clone();
        let (code, out) = repo.text(&["status", "--porcelain"], PLAIN)?;
        if code != 0 {
            return Err(git_error("git could not read the worktree status"));
        }
        Ok(!out.trim().is_empty())
    }

    /// Removes a worktree without `--force`; false when git refuses.
    pub fn remove_worktree(&self, path: &Path) -> IntegrateResult<bool> {
        let path = path.to_string_lossy();
        Ok(self.run(&["worktree", "remove", "--", &path], WRITE)?.code == 0)
    }

    /// The best common ancestor of two commits, or `None` when they share none.
    pub fn merge_base(&self, a: &str, b: &str) -> IntegrateResult<Option<String>> {
        if !is_full_sha(a) || !is_full_sha(b) {
            return Ok(None);
        }
        let (code, out) = self.text(&["merge-base", a, b], PLAIN)?;
        let base = js_trim(&out).to_string();
        Ok((code == 0 && is_full_sha(&base)).then_some(base))
    }

    /// Whether every path the commit changed since its merge base with HEAD has the same content in HEAD (a commit HEAD
    /// already contains counts). `Err` when git cannot tell (output too large, no base); `Ok(false)` without a base.
    pub fn content_in_head(&self, sha: &str) -> IntegrateResult<bool> {
        let head = self.head_commit()?;
        let Some(base) = self.merge_base(sha, &head)? else {
            return Ok(false);
        };
        let own = self.changed_paths(&base, sha)?;
        if own.is_empty() {
            return Ok(true);
        }
        let drift = self.changed_paths(sha, &head)?;
        Ok(own.iter().all(|path| !drift.contains(path)))
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

    /// Whether an integration of this process is merging right now.
    pub fn busy(&self) -> bool {
        !self.in_flight.borrow().is_empty()
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
    if state == "conflicted" && outcome == "discarded" {
        // Merging stopped at the conflict: there is no branch to remove.
        let record = integrations::settle_integration(
            deps.kernel,
            &deps.ctx(),
            &json!({"integrationId": integration_id, "outcome": outcome}),
        )?;
        return Ok(json!({"record": record, "branchRemoved": false}));
    }
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

/// Why a merge request is refused: a code from `DOC_REFUSAL_CODES` and the names it concerns, escaped, sorted and cut.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Refusal {
    code: &'static str,
    files: Vec<String>,
    omitted: usize,
}

impl Refusal {
    /// Each name goes once through `printable_path`; the escaped strings are sorted by their bytes, made unique and cut
    /// at `MAX_CONFLICT_FILES`, and `omitted` counts the unique names cut.
    pub fn new(code: &'static str, names: Vec<Vec<u8>>) -> Refusal {
        debug_assert!(
            capstan_config::remote::DOC_REFUSAL_CODES.contains(&code),
            "unknown refusal code {code}"
        );
        let unique: BTreeSet<String> = names.iter().map(|name| printable_path(name)).collect();
        let omitted = unique.len().saturating_sub(MAX_CONFLICT_FILES);
        Refusal {
            code,
            files: unique.into_iter().take(MAX_CONFLICT_FILES).collect(),
            omitted,
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn files(&self) -> &[String] {
        &self.files
    }

    pub fn omitted(&self) -> usize {
        self.omitted
    }
}

/// The answer of the check before a merge into the root checkout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Preflight {
    AlreadyInHead,
    Ready { old_head: String },
    Refused(Refusal),
}

/// The result of a merge into the root checkout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RootMerge {
    Merged {
        commit: String,
    },
    AlreadyInHead,
    Refused(Refusal),
    Failed {
        reason: &'static str,
        restored: bool,
    },
}

impl RootMerge {
    pub fn failed(reason: &'static str, restored: bool) -> RootMerge {
        debug_assert!(
            capstan_config::remote::DOC_FAILURE_REASONS.contains(&reason),
            "unknown failure reason {reason}"
        );
        RootMerge::Failed { reason, restored }
    }
}

/// How far a push got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PushState {
    Pushed,
    Skipped,
    Failed,
}

/// The result of a push: the state and, unless it pushed, the reason from `DOC_PUSH_REASONS`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PushOutcome {
    pub state: PushState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
}

impl PushOutcome {
    pub fn pushed() -> PushOutcome {
        PushOutcome {
            state: PushState::Pushed,
            reason: None,
        }
    }

    pub fn skipped(reason: &'static str) -> PushOutcome {
        Self::with_reason(PushState::Skipped, reason)
    }

    pub fn failed(reason: &'static str) -> PushOutcome {
        Self::with_reason(PushState::Failed, reason)
    }

    fn with_reason(state: PushState, reason: &'static str) -> PushOutcome {
        debug_assert!(
            capstan_config::remote::DOC_PUSH_REASONS.contains(&reason),
            "unknown push reason {reason}"
        );
        PushOutcome {
            state,
            reason: Some(reason),
        }
    }
}

/// What git's config error output says about the repository's configuration.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// The repository's own config file cannot be read or parsed.
    Unreadable,
    /// Git is unavailable for any other reason; `message` is the line that tells why.
    Unavailable { message: String },
}

/// The substrings of the five git messages whose line names a file; the sixth ("unknown error occurred while reading
/// the configuration files") names none, so it only counts through the warning lines printed with it.
const CONFIG_ERROR_PATH_MARKERS: [&str; 4] = [
    "bad config line",
    "unable to access",
    "Permission denied",
    "unable to read config file",
];

fn config_error_message(lines: &[&str]) -> String {
    lines
        .iter()
        .find(|line| line.starts_with("fatal:") || line.starts_with("error:"))
        .or_else(|| lines.iter().find(|line| !line.trim().is_empty()))
        .map(|line| line.trim().to_string())
        .unwrap_or_default()
}

/// The path a config error line names: the rest after ` in file ` or the text inside the first pair of single quotes.
fn config_error_path(line: &str) -> Option<&str> {
    if line.contains("bad config line") {
        return line.split_once(" in file ").map(|(_, path)| path.trim());
    }
    let rest = line.split_once('\'')?.1;
    Some(rest.split_once('\'')?.0)
}

/// Canonical form of a path that may not exist: the path itself, else its parent plus the file name, else as given.
fn canonical_or_joined(path: &Path) -> PathBuf {
    if let Ok(real) = std::fs::canonicalize(path) {
        return real;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => match std::fs::canonicalize(parent) {
            Ok(real) => real.join(name),
            Err(_) => path.to_path_buf(),
        },
        _ => path.to_path_buf(),
    }
}

/// Classifies the stderr of a git call that failed while reading the configuration. `git_dir` and `common_dir` are
/// absolute; a relative path in a line is relative to `root`, the child's `-C` directory. Relies on `LC_ALL=C` stderr.
#[doc(hidden)]
pub fn classify_config_error(
    stderr: &[u8],
    root: &Path,
    git_dir: &Path,
    common_dir: &Path,
) -> ConfigError {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text.lines().collect();
    if let Some(line) = lines
        .iter()
        .find(|line| line.contains("detected dubious ownership"))
    {
        return ConfigError::Unavailable {
            message: line.trim().to_string(),
        };
    }
    let own: Vec<PathBuf> = [git_dir, common_dir]
        .iter()
        .filter(|dir| dir.is_dir())
        .flat_map(|dir| [dir.join("config"), dir.join("config.worktree")])
        .map(|path| canonical_or_joined(&path))
        .collect();
    let names_repo_config = lines.iter().any(|line| {
        CONFIG_ERROR_PATH_MARKERS
            .iter()
            .any(|marker| line.contains(marker))
            && config_error_path(line).is_some_and(|path| {
                !path.is_empty() && own.contains(&canonical_or_joined(&root.join(path)))
            })
    });
    if names_repo_config {
        return ConfigError::Unreadable;
    }
    ConfigError::Unavailable {
        message: config_error_message(&lines),
    }
}

/// The git dir and the common dir of `root` when `git rev-parse` failed: `root/.git` if it is a directory, else the
/// `gitdir:` path of a gitfile (relative to `root`); the common dir follows `commondir` when present. Anything
/// unreadable gives `(root/.git, root/.git)`, which matches nothing the classification would trust.
#[doc(hidden)]
pub fn repo_dirs_fallback(root: &Path) -> (PathBuf, PathBuf) {
    let dot_git = root.join(".git");
    let missing = || (dot_git.clone(), dot_git.clone());
    let git_dir = if dot_git.is_dir() {
        dot_git.clone()
    } else {
        let Ok(content) = std::fs::read_to_string(&dot_git) else {
            return missing();
        };
        let Some(path) = content
            .lines()
            .find_map(|line| line.strip_prefix("gitdir:"))
            .map(str::trim)
            .filter(|path| !path.is_empty())
        else {
            return missing();
        };
        let dir = root.join(path);
        if !dir.is_dir() {
            return missing();
        }
        dir
    };
    let common_dir = match std::fs::read_to_string(git_dir.join("commondir")) {
        Ok(content) if !content.trim().is_empty() => git_dir.join(content.trim()),
        _ => git_dir.clone(),
    };
    (git_dir, common_dir)
}

/// Which git call the repository-local configuration is checked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsafeScope {
    /// A merge into the root checkout: the deny-list `MERGE_UNSAFE_FAMILIES`.
    Merge,
    /// A push: every key of `DOC_UNSAFE_FAMILIES`.
    Push,
}

/// What the repository-local configuration holds for one scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnsafeConfig {
    Clean,
    /// The refused keys, as `git config --list` prints them.
    Keys(Vec<String>),
    /// The repository's own config file cannot be read or parsed.
    Unreadable,
}

impl UnsafeConfig {
    /// `unsafe_config` with the refused keys, or with `(config unreadable)`; `None` when clean.
    pub fn into_refusal(self) -> Option<Refusal> {
        match self {
            UnsafeConfig::Clean => None,
            UnsafeConfig::Keys(keys) => Some(Refusal::new(
                "unsafe_config",
                keys.iter().map(|key| key.as_bytes().to_vec()).collect(),
            )),
            UnsafeConfig::Unreadable => Some(Refusal::new(
                "unsafe_config",
                vec![b"(config unreadable)".to_vec()],
            )),
        }
    }
}

/// The local keys a merge into the root checkout refuses (patterns as in `DOC_UNSAFE_FAMILIES`). Hooks and fsmonitor are
/// pinned off by `run_root` and are not listed.
pub const MERGE_UNSAFE_FAMILIES: &[&str] = &[
    "core.alternateRefsCommand",
    "merge.*.driver",
    "filter.*.clean",
    "filter.*.smudge",
    "filter.*.process",
    "core.attributesFile",
    "core.worktree",
    "core.bare",
    "include.path",
    "includeIf.*",
    "extensions.partialClone",
    "remote.*.promisor",
    "remote.*.partialclonefilter",
];

/// The git subcommands a merge into the root checkout runs (plan-106: `check-ref-format --branch`, its steps 1 to 10 and
/// the merge commit). Each is audited to run no program of the repository's choosing; a subcommand not listed needs a
/// new audit.
pub const MERGE_AUDITED_COMMANDS: &[&str] = &[
    "rev-parse",
    "symbolic-ref",
    "check-ref-format",
    "merge-base",
    "status",
    "diff-tree",
    "diff-index",
    "diff-files",
    "ls-files",
    "merge-tree",
    "commit-tree",
    "read-tree",
    "update-index",
    "update-ref",
    "config",
];

/// The files of the common dir a merge refuses when they hold something, relative to the common dir.
const MERGE_REFUSED_FILES: [&str; 3] = ["objects/info/alternates", "info/grafts", "shallow"];

/// Whether a file holds a line that is neither blank (only whitespace or `\r`) nor a `#` comment.
fn has_content_line(bytes: &[u8]) -> bool {
    bytes.split(|byte| *byte == b'\n').any(|line| {
        !line.iter().all(|byte| byte.is_ascii_whitespace()) && line.first() != Some(&b'#')
    })
}

/// Whether `path` makes a merge refuse: a regular file (a symlink to one is followed) with a content line, or anything
/// else that is there but is not a readable regular file (a directory, a dangling link, an unreadable file). Absent is
/// fine.
fn repo_file_refuses(path: &Path) -> bool {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => match std::fs::read(path) {
            Ok(bytes) => has_content_line(&bytes),
            Err(_) => true,
        },
        Ok(_) => true,
        Err(err) => {
            err.kind() != std::io::ErrorKind::NotFound || std::fs::symlink_metadata(path).is_ok()
        }
    }
}

/// The directories of one repository, read once and used for every classification.
struct RepoDirs {
    root: PathBuf,
    git_dir: PathBuf,
    common_dir: PathBuf,
}

/// Splits a config key at its first and last dot: section, subsection (none when there is one dot), variable.
fn split_config_key(key: &str) -> Option<(&str, Option<&str>, &str)> {
    let first = key.find('.')?;
    let last = key.rfind('.')?;
    if first == last {
        Some((&key[..first], None, &key[first + 1..]))
    } else {
        Some((&key[..first], Some(&key[first + 1..last]), &key[last + 1..]))
    }
}

/// Section and variable match without case, the subsection exactly; `*` as a subsection matches any (also an empty)
/// one, `*` as the variable of a two-part pattern matches every key of the section.
fn config_key_matches(pattern: &str, key: &str) -> bool {
    let (Some((p_section, p_sub, p_var)), Some((k_section, k_sub, k_var))) =
        (split_config_key(pattern), split_config_key(key))
    else {
        return false;
    };
    if !p_section.eq_ignore_ascii_case(k_section) {
        return false;
    }
    if p_sub.is_none() && p_var == "*" {
        return true;
    }
    let sub_matches = match (p_sub, k_sub) {
        (None, None) => true,
        (Some("*"), Some(_)) => true,
        (Some(wanted), Some(found)) => wanted == found,
        _ => false,
    };
    sub_matches && p_var.eq_ignore_ascii_case(k_var)
}

/// Whether a value is a git false (`false`, `no`, `off`, `0`, empty); a key alone is true.
fn config_value_is_false(value: Option<&str>) -> bool {
    let Some(value) = value else {
        return false;
    };
    ["false", "no", "off", ""]
        .iter()
        .any(|word| value.eq_ignore_ascii_case(word))
        || value.parse::<i64>().is_ok_and(|number| number == 0)
}

/// Key and value of each entry; `None` is a key alone.
type ConfigEntries = Vec<(String, Option<String>)>;

/// The entries of `git config -z --list`: key, then a newline and the value; a key alone means true.
fn parse_config_entries(stdout: &[u8]) -> ConfigEntries {
    stdout
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| match entry.iter().position(|byte| *byte == b'\n') {
            Some(at) => (
                String::from_utf8_lossy(&entry[..at]).into_owned(),
                Some(String::from_utf8_lossy(&entry[at + 1..]).into_owned()),
            ),
            None => (String::from_utf8_lossy(entry).into_owned(), None),
        })
        .collect()
}

impl GitRepo {
    /// Reads the git dir and the common dir once each (a relative answer is relative to the root); when `rev-parse`
    /// fails the files decide, as `repo_dirs_fallback` does.
    fn repo_dirs(&self, call: &RootCall) -> IntegrateResult<RepoDirs> {
        let ask = |flag: &str| -> IntegrateResult<Option<PathBuf>> {
            let out = self.run_root(&RootCall {
                args: vec!["rev-parse".into(), flag.into()],
                ..call.clone()
            })?;
            if out.code != 0 {
                return Ok(None);
            }
            let text = out.stdout.strip_suffix(b"\n").unwrap_or(&out.stdout);
            if text.is_empty() {
                return Ok(None);
            }
            Ok(Some(self.root.join(Path::new(OsStr::from_bytes(text)))))
        };
        let git_dir = ask("--git-dir")?;
        let common_dir = ask("--git-common-dir")?;
        let (git_dir, common_dir) = match (git_dir, common_dir) {
            (Some(git_dir), Some(common_dir)) => (git_dir, common_dir),
            _ => repo_dirs_fallback(&self.root),
        };
        Ok(RepoDirs {
            root: self.root.clone(),
            git_dir,
            common_dir,
        })
    }

    /// One `git config -z --list` read of a scope flag; `Ok(None)` when the file cannot be read.
    fn read_config_entries(
        &self,
        call: &RootCall,
        dirs: &RepoDirs,
        flag: &str,
    ) -> IntegrateResult<Option<ConfigEntries>> {
        let out = self.run_root(&RootCall {
            args: vec!["config".into(), flag.into(), "-z".into(), "--list".into()],
            ..call.clone()
        })?;
        if out.code == 0 {
            return Ok(Some(parse_config_entries(&out.stdout)));
        }
        match classify_config_error(&out.stderr, &dirs.root, &dirs.git_dir, &dirs.common_dir) {
            ConfigError::Unreadable => Ok(None),
            ConfigError::Unavailable { message } => Err(unavailable(message)),
        }
    }

    /// Checks the repository-local configuration (`.git/config`, and `config.worktree` when
    /// `extensions.worktreeConfig` is on) against the keys refused for `scope`. The user's global and system
    /// configuration is not looked at.
    pub fn unsafe_repo_config(&self, scope: UnsafeScope) -> IntegrateResult<UnsafeConfig> {
        let call = RootCall {
            args: Vec::new(),
            timeout: GIT_TIMEOUT,
            config_env: Vec::new(),
            network: false,
            user_config: scope == UnsafeScope::Push,
        };
        let dirs = self.repo_dirs(&call)?;
        let Some(mut entries) = self.read_config_entries(&call, &dirs, "--local")? else {
            return Ok(UnsafeConfig::Unreadable);
        };
        let worktree_config = entries
            .iter()
            .rev()
            .find(|(key, _)| key.eq_ignore_ascii_case("extensions.worktreeconfig"))
            .is_some_and(|(_, value)| !config_value_is_false(value.as_deref()));
        let has_file = std::fs::symlink_metadata(dirs.git_dir.join("config.worktree")).is_ok();
        if worktree_config && has_file {
            let Some(more) = self.read_config_entries(&call, &dirs, "--worktree")? else {
                return Ok(UnsafeConfig::Unreadable);
            };
            entries.extend(more);
        }
        let families: &[&str] = match scope {
            UnsafeScope::Merge => MERGE_UNSAFE_FAMILIES,
            UnsafeScope::Push => capstan_config::remote::DOC_UNSAFE_FAMILIES,
        };
        let mut keys: Vec<String> = Vec::new();
        for (key, value) in &entries {
            let refused = families.iter().any(|pattern| {
                config_key_matches(pattern, key)
                    && (!key.eq_ignore_ascii_case("core.bare")
                        || !config_value_is_false(value.as_deref()))
            });
            if refused && !keys.contains(key) {
                keys.push(key.clone());
            }
        }
        if scope == UnsafeScope::Merge {
            for name in MERGE_REFUSED_FILES {
                let path = dirs.common_dir.join(name);
                if repo_file_refuses(&path) {
                    let shown = path.strip_prefix(&dirs.root).unwrap_or(&path);
                    keys.push(shown.to_string_lossy().into_owned());
                }
            }
        }
        Ok(if keys.is_empty() {
            UnsafeConfig::Clean
        } else {
            UnsafeConfig::Keys(keys)
        })
    }
}

#[cfg(test)]
mod root_runner_tests {
    use super::*;

    #[test]
    fn child_timeout_is_the_least_of_the_limit_and_the_time_left() {
        let deadline = Deadline::new(Duration::from_secs(60));
        let short = deadline
            .child_timeout(Duration::from_secs(2))
            .expect("time left");
        assert_eq!(short, Duration::from_secs(2));
        let long = deadline
            .child_timeout(Duration::from_secs(600))
            .expect("time left");
        assert!(long <= Duration::from_secs(60) && long > Duration::from_secs(50));
        let passed = Deadline::new(Duration::ZERO);
        assert!(passed.child_timeout(Duration::from_secs(1)).is_none());
    }

    fn exited(code: i32) -> std::process::ExitStatus {
        use std::os::unix::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(code << 8)
    }

    fn signalled(signal: i32) -> std::process::ExitStatus {
        use std::os::unix::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(signal)
    }

    #[test]
    fn a_decided_timeout_wins_whatever_the_leader_reports() {
        for status in [exited(0), exited(3), signalled(9)] {
            assert_eq!(
                final_code(Some(Decided::Timeout), status, true),
                Err("timeout")
            );
            assert_eq!(
                final_code(Some(Decided::Timeout), status, false),
                Err("timeout")
            );
        }
    }

    #[test]
    fn a_decided_cap_wins_whatever_the_leader_reports() {
        for status in [exited(0), exited(3), signalled(9)] {
            assert_eq!(
                final_code(Some(Decided::TooLarge), status, true),
                Err("output_too_large")
            );
        }
    }

    #[test]
    fn an_undecided_call_follows_the_leader() {
        assert_eq!(final_code(None, signalled(9), true), Err("killed"));
        assert_eq!(final_code(None, signalled(9), false), Err("killed"));
        assert_eq!(final_code(None, exited(0), true), Ok(0));
        assert_eq!(final_code(None, exited(3), true), Ok(3));
        assert_eq!(final_code(None, exited(0), false), Err("output_incomplete"));
        assert_eq!(final_code(None, exited(3), false), Err("output_incomplete"));
    }

    #[test]
    fn the_group_target_is_the_negated_pid_or_none() {
        assert_eq!(group_target(1234), Some(-1234));
        assert_eq!(group_target(u32::MAX), None);
        assert_eq!(group_target(i32::MAX as u32 + 1), None);
        assert_eq!(group_target(0), None);
    }

    #[test]
    fn the_version_is_the_first_x_y_after_git_version() {
        assert_eq!(
            parse_git_version("git version 2.39.5 (Apple Git-154)\n"),
            Some((2, 39))
        );
        assert_eq!(
            parse_git_version("git version 2.43.0.windows.1"),
            Some((2, 43))
        );
        assert_eq!(parse_git_version("something else"), None);
    }
}
