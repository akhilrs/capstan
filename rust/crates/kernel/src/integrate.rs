//! The integration pipeline (src/integration.ts and the git half of src/git.ts): merging reports into an integration
//! branch and settling it. Git runs as a subprocess with the same clean environment, argument lists and exit-code
//! handling as the Node code; the kernel is reached through the integrations area.

use crate::areas::integrations;
use crate::errors::KernelError;
use crate::helpers::{fold_whitespace, is_js_space, js_trim};
use crate::integration_impact;
use crate::kernel::Kernel;
use crate::plan_body::{cut_at_word, cut_hard, format_subject, COMMIT_TYPES};
use crate::records::{MAX_CONFLICT_FILES, MAX_CONFLICT_PATH_CHARS};
use crate::types::MutationContext;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
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
    merge_seams: MergeSeams,
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
/// `FORWARDED_USER_ENV` with `user_config`, the `GIT_CONFIG_*` of `config_env`, the two pins of a call without it and
/// the names of `MERGE_CHILD_ENV` a merge call sets.
pub const FIXED_CHILD_ENV: &[&str] = &[
    "PATH",
    "LC_ALL",
    "GIT_TERMINAL_PROMPT",
    "GCM_INTERACTIVE",
    "GIT_EDITOR",
    "GIT_PAGER",
    "PAGER",
];

/// The only names a merge call may add to the environment of its child (`run_merge`'s `extra_env` and the lock pin).
pub const MERGE_CHILD_ENV: &[&str] = &[
    "GIT_OPTIONAL_LOCKS",
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
    "TZ",
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

/// The subcommands `run_merge` runs: the audited list of a merge, one list.
pub const RUN_MERGE_SUBCOMMANDS: &[&str] = MERGE_AUDITED_COMMANDS;

/// The limit of one merge child that walks the tree or the index.
#[allow(dead_code)]
const MERGE_TREE_TIMEOUT: Duration = Duration::from_secs(120);

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

/// The call `run_merge` makes: the tree-walking subcommands get `MERGE_TREE_TIMEOUT`, the rest `GIT_TIMEOUT`; the
/// commit encoding is pinned and the user's config is not read.
#[allow(dead_code)]
fn merge_call(args: &[&str]) -> RootCall {
    let walks = args.first().is_some_and(|sub| {
        matches!(
            *sub,
            "merge-tree"
                | "read-tree"
                | "status"
                | "diff-tree"
                | "ls-files"
                | "update-index"
                | "diff-index"
                | "diff-files"
                | "merge-base"
        )
    });
    RootCall {
        args: args.iter().map(|arg| arg.to_string()).collect(),
        timeout: if walks {
            MERGE_TREE_TIMEOUT
        } else {
            GIT_TIMEOUT
        },
        config_env: vec![("i18n.commitEncoding".into(), "UTF-8".into())],
        network: false,
        user_config: false,
    }
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

    /// Spawns `git --version` and reads it; a version at or above `MIN_GIT` is cached, a too-old one never is.
    fn probe_version(&self, program: &Path, deadline: &Deadline) -> IntegrateResult<(u32, u32)> {
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
        if version >= MIN_GIT {
            let _ = self.version.set(version);
        }
        Ok(version)
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
        let version = self.probe_version(program, deadline)?;
        if version < MIN_GIT {
            return Err(unavailable(format!(
                "git {}.{} is older than {}.{}",
                version.0, version.1, MIN_GIT.0, MIN_GIT.1
            )));
        }
        Ok(())
    }

    /// Whether git is older than `MIN_GIT`; `force_old_git` wins over `skip_version_probe`. A spawn failure or an
    /// unreadable version is `git_unavailable`.
    #[allow(dead_code)]
    fn git_too_old(&self, deadline: &Deadline) -> IntegrateResult<bool> {
        if self.seams.force_old_git {
            return Ok(true);
        }
        if self.seams.skip_version_probe || self.version.get().is_some() {
            return Ok(false);
        }
        let program = self.git_program()?;
        Ok(self.probe_version(&program, deadline)? < MIN_GIT)
    }

    /// Refuses a call that must not reach git.
    fn check_call(call: &RootCall, merge: bool) -> IntegrateResult<()> {
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
        // `read-tree -m -u` and `--reset -u` have no long form: the exact `-u` is allowed there for a merge call.
        let update = |arg: &str| merge && subcommand == "read-tree" && arg == "-u";
        if let Some(arg) = call
            .args
            .iter()
            .find(|arg| !query(arg) && !update(arg) && refused_option(arg))
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
        self.run_root_in(call, &Deadline::new(call.timeout), &[], false)
    }

    /// `run_root` under a shared `deadline` (each child gets the least of `call.timeout` and the time left), with
    /// `child_env` added after the fixed environment: each name must be in `MERGE_CHILD_ENV` and each value free of
    /// NUL and newline, else `git_unavailable` before any spawn. `merge` is passed to `check_call`.
    fn run_root_in(
        &self,
        call: &RootCall,
        deadline: &Deadline,
        child_env: &[(&str, &str)],
        merge: bool,
    ) -> IntegrateResult<RootOutput> {
        Self::check_call(call, merge)?;
        for (name, value) in child_env {
            if !MERGE_CHILD_ENV.contains(name) {
                return Err(unavailable(format!(
                    "the git environment name {name} is not allowed"
                )));
            }
            if bad_text(value) {
                return Err(unavailable(format!(
                    "the git environment value of {name} holds a NUL or a newline"
                )));
            }
        }
        let program = self.git_program()?;
        self.check_version(&program, deadline)?;

        let mut env = self.fixed_env();
        env.extend(
            child_env
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string())),
        );
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
            let ssh_command = self.ssh_command(&program, &env, deadline)?;
            args.push("-c".into());
            args.push(format!("core.sshCommand={ssh_command}").into());
        }
        args.extend(call.args.iter().map(OsString::from));
        let timeout = deadline
            .child_timeout(call.timeout)
            .ok_or_else(|| integration_error("timeout", "git timed out".into()))?;
        self.spawn_capped(&program, &args, &env, timeout, ROOT_STDOUT_CAP)
    }

    /// Runs one git command of a merge into the root checkout (`RUN_MERGE_SUBCOMMANDS` only) under the shared
    /// `deadline`; `read_only` adds `GIT_OPTIONAL_LOCKS=0`. A name given twice is `git_unavailable` before any spawn.
    #[allow(dead_code)]
    fn run_merge(
        &self,
        args: &[&str],
        deadline: &Deadline,
        read_only: bool,
        extra_env: &[(&str, &str)],
    ) -> IntegrateResult<RootOutput> {
        if !args
            .first()
            .is_some_and(|sub| RUN_MERGE_SUBCOMMANDS.contains(sub))
        {
            return Err(unavailable("this git subcommand is not a merge command"));
        }
        let mut child_env: Vec<(&str, &str)> = Vec::new();
        if read_only {
            child_env.push(("GIT_OPTIONAL_LOCKS", "0"));
        }
        child_env.extend_from_slice(extra_env);
        for (index, (name, _)) in child_env.iter().enumerate() {
            if child_env[..index]
                .iter()
                .any(|(earlier, _)| earlier == name)
            {
                return Err(unavailable(format!(
                    "the git environment name {name} is given twice"
                )));
            }
        }
        self.run_root_in(&merge_call(args), deadline, &child_env, true)
    }

    /// One value of `key` as `git commit` reads it (all scopes, the last value wins), trimmed; `None` when it is
    /// unset, empty, holds `<`, `>` or a newline, or has nothing git keeps in an ident.
    #[allow(dead_code)]
    fn identity_value(&self, key: &str, deadline: &Deadline) -> IntegrateResult<Option<String>> {
        let call = RootCall {
            args: ["config", "--get-all", "-z", key]
                .iter()
                .map(|arg| arg.to_string())
                .collect(),
            timeout: GIT_TIMEOUT,
            config_env: Vec::new(),
            network: false,
            user_config: true,
        };
        let output = self.run_root_in(&call, deadline, &[], false)?;
        if output.stdout.len() > PROBE_STDOUT_CAP {
            return Err(integration_error(
                "output_too_large",
                "git printed too much".into(),
            ));
        }
        match output.code {
            0 => {}
            1 => return Ok(None),
            code => {
                return Err(unavailable(format!(
                    "{key} could not be read (git exited {code})"
                )))
            }
        }
        let mut items: Vec<&[u8]> = output.stdout.split(|byte| *byte == 0).collect();
        if items.last().is_some_and(|last| last.is_empty()) {
            items.pop();
        }
        let Some(last) = items.last() else {
            return Ok(None);
        };
        let Ok(text) = std::str::from_utf8(last) else {
            return Ok(None);
        };
        let value = text.trim();
        let stripped = |c: char| c.is_whitespace() || ".,:;\"\\'".contains(c);
        if value.is_empty() || value.contains(['<', '>', '\n']) || value.chars().all(stripped) {
            return Ok(None);
        }
        Ok(Some(value.to_string()))
    }

    /// The commit identity from git config alone: `Some` only when `user.name` and `user.email` both give a value.
    #[allow(dead_code)]
    fn strict_identity(&self, deadline: &Deadline) -> IntegrateResult<Option<(String, String)>> {
        let Some(name) = self.identity_value("user.name", deadline)? else {
            return Ok(None);
        };
        let Some(email) = self.identity_value("user.email", deadline)? else {
            return Ok(None);
        };
        Ok(Some((name, email)))
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
            merge_seams: MergeSeams::default(),
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
            .arg(&self.root);
        // The repository's own config must not start a program (`core.fsmonitor`, hooks) during any helper.
        for (key, value) in ROOT_CONFIG_OVERRIDES {
            command.arg("-c").arg(format!("{key}={value}"));
        }
        command
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

    /// What changed between `base` and `head`, per path: status, insertions and deletions (a binary file counts 0 and
    /// 0). Renames are not followed. At most `DIFF_STAT_MAX_PATHS` paths are listed; beyond that only the counts are
    /// kept and `truncated` is set. The whole call has `DIFF_STAT_TIMEOUT`.
    pub fn diff_stat(&self, base: &str, head: &str) -> Result<DiffStat, DiffStatError> {
        if !is_full_sha(base) || !is_full_sha(head) {
            return Err(DiffStatError::Failed);
        }
        let started = Instant::now();
        let run = |format: &str| -> Result<Vec<u8>, DiffStatError> {
            let left = DIFF_STAT_TIMEOUT.saturating_sub(started.elapsed());
            if left.is_zero() {
                return Err(DiffStatError::Timeout);
            }
            let options = RunOptions {
                timeout: left,
                ..BIG
            };
            let args = ["diff", "--no-renames", "-z", format, base, head, "--"];
            match self.run(&args, options) {
                Ok(outcome) if outcome.code == 0 => Ok(outcome.stdout),
                Ok(_) => Err(DiffStatError::Failed),
                Err(error) if error.display().contains("timed out") => Err(DiffStatError::Timeout),
                Err(_) => Err(DiffStatError::Failed),
            }
        };
        let raw = run("--raw")?;
        let numstat = run("--numstat")?;
        let mut statuses = raw.split(|b| *b == 0).filter(|part| !part.is_empty());
        let mut stat = DiffStat::default();
        let mut counts = numstat.split(|b| *b == 0).filter(|part| !part.is_empty());
        while let (Some(meta), Some(path), Some(line)) =
            (statuses.next(), statuses.next(), counts.next())
        {
            let status = meta.last().map_or('M', |b| char::from(*b));
            let text = utf8(line);
            let mut fields = text.splitn(3, '\t');
            let added = fields.next().and_then(|n| n.parse::<u64>().ok());
            let removed = fields.next().and_then(|n| n.parse::<u64>().ok());
            let (added, removed) = (added.unwrap_or(0), removed.unwrap_or(0));
            stat.files_changed += 1;
            stat.insertions += added;
            stat.deletions += removed;
            if stat.entries.len() < DIFF_STAT_MAX_PATHS {
                stat.entries.push(DiffEntry {
                    path: utf8(path),
                    status,
                    insertions: added,
                    deletions: removed,
                });
            } else {
                stat.truncated = true;
            }
        }
        Ok(stat)
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

/// The most paths `GitRepo::diff_stat` lists.
pub const DIFF_STAT_MAX_PATHS: usize = 5000;
/// The time the whole of `GitRepo::diff_stat` may take.
pub const DIFF_STAT_TIMEOUT: Duration = Duration::from_secs(10);

/// One changed path of a `DiffStat`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffEntry {
    pub path: String,
    /// git's one-letter status: `A`, `M`, `D`, `T` ...
    pub status: char,
    pub insertions: u64,
    pub deletions: u64,
}

/// What changed between two commits.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiffStat {
    /// Every changed path, also those beyond the listed ones.
    pub files_changed: usize,
    pub insertions: u64,
    pub deletions: u64,
    /// More than `DIFF_STAT_MAX_PATHS` paths changed; `entries` holds the first ones only.
    pub truncated: bool,
    pub entries: Vec<DiffEntry>,
}

/// Why `GitRepo::diff_stat` gave nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffStatError {
    Timeout,
    Failed,
}

/// `IntegrationDeps`.
pub struct IntegrationDeps<'a> {
    pub kernel: &'a Kernel,
    pub git: &'a GitRepo,
    pub state: &'a IntegrationState,
    pub credential: &'a str,
    pub context: &'a dyn Fn(&str) -> MutationContext,
    pub log: &'a dyn Fn(&str, Value),
    /// The project's resolved `[impact]`, applied to the details recorded when a merge finishes.
    pub impact: &'a capstan_config::ImpactConfig,
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
    let finish = |outcome: Value, details: Option<&Value>| -> Result<Value, KernelError> {
        integrations::finish_integration_with_details(
            deps.kernel,
            &deps.ctx(),
            &json!({"integrationId": integration_id, "outcome": outcome}),
            details,
        )
    };
    let info: RefCell<Option<Value>> = RefCell::new(None);
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
        let read = integrations::integration_commit_info(deps.kernel, integration_id)?;
        let (subject, body) = squash_message(&read, &subjects);
        *info.borrow_mut() = Some(read);
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
    let details = match (&result, info.borrow().as_ref()) {
        (MergeResult::Merged { head_sha }, Some(info)) => {
            let diff = deps.git.diff_stat(base_sha, head_sha);
            if let Err(error) = &diff {
                (deps.log)(
                    "integration_impact_failed",
                    json!({"integrationId": integration_id, "reason": match error {
                        DiffStatError::Timeout => "timeout",
                        DiffStatError::Failed => "diff_failed",
                    }}),
                );
            }
            let listed: Vec<&str> = info
                .get("reports")
                .and_then(Value::as_array)
                .map(|reports| reports.iter().map(|r| text_of(r, "reportId")).collect())
                .unwrap_or_default();
            let listed = &listed[..listed.len().min(integration_impact::MAX_REPORTS)];
            let plan = info.get("planId").and_then(Value::as_str);
            let packages =
                integrations::report_package_ids(deps.kernel, plan, listed).unwrap_or_default();
            Some(integration_impact::details(
                info,
                &packages,
                diff.as_ref(),
                deps.impact,
            ))
        }
        _ => None,
    };
    match finish(result.outcome(), details.as_ref()) {
        Ok(record) => Ok(record),
        Err(error) => {
            // The branch must not outlive a record that does not say it exists, and the row must not stay running.
            if let MergeResult::Merged { head_sha } = &result {
                let _ = deps.git.delete_branch_at(branch, head_sha);
            }
            let _ = finish(
                json!({"kind": "failed", "reason": "the outcome could not be recorded"}),
                None,
            );
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

/// The branches a remote merge may go into (`GitRepo::merge_targets`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeTargets {
    pub checked_out: Option<String>,
    pub branches: Vec<String>,
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
    fn repo_dirs(&self, call: &RootCall, deadline: Option<&Deadline>) -> IntegrateResult<RepoDirs> {
        let ask = |flag: &str| -> IntegrateResult<Option<PathBuf>> {
            let out = self.run_config_call(
                &RootCall {
                    args: vec!["rev-parse".into(), flag.into()],
                    ..call.clone()
                },
                deadline,
            )?;
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

    /// `run_root`, or with a `deadline` the shared deadline and `GIT_OPTIONAL_LOCKS=0` so the read refreshes nothing.
    fn run_config_call(
        &self,
        call: &RootCall,
        deadline: Option<&Deadline>,
    ) -> IntegrateResult<RootOutput> {
        match deadline {
            None => self.run_root(call),
            Some(deadline) => {
                self.run_root_in(call, deadline, &[("GIT_OPTIONAL_LOCKS", "0")], false)
            }
        }
    }

    /// One `git config -z --list` read of a scope flag; `Ok(None)` when the file cannot be read.
    fn read_config_entries(
        &self,
        call: &RootCall,
        dirs: &RepoDirs,
        flag: &str,
        deadline: Option<&Deadline>,
    ) -> IntegrateResult<Option<ConfigEntries>> {
        let out = self.run_config_call(
            &RootCall {
                args: vec!["config".into(), flag.into(), "-z".into(), "--list".into()],
                ..call.clone()
            },
            deadline,
        )?;
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
        self.unsafe_repo_config_in(scope, None)
    }

    /// `unsafe_repo_config`; with a `deadline` every child shares it and runs read-only.
    fn unsafe_repo_config_in(
        &self,
        scope: UnsafeScope,
        deadline: Option<&Deadline>,
    ) -> IntegrateResult<UnsafeConfig> {
        let call = RootCall {
            args: Vec::new(),
            timeout: GIT_TIMEOUT,
            config_env: Vec::new(),
            network: false,
            user_config: scope == UnsafeScope::Push,
        };
        let dirs = self.repo_dirs(&call, deadline)?;
        let Some(mut entries) = self.read_config_entries(&call, &dirs, "--local", deadline)? else {
            return Ok(UnsafeConfig::Unreadable);
        };
        let worktree_config = entries
            .iter()
            .rev()
            .find(|(key, _)| key.eq_ignore_ascii_case("extensions.worktreeconfig"))
            .is_some_and(|(_, value)| !config_value_is_false(value.as_deref()));
        let has_file = std::fs::symlink_metadata(dirs.git_dir.join("config.worktree")).is_ok();
        if worktree_config && has_file {
            let Some(more) = self.read_config_entries(&call, &dirs, "--worktree", deadline)? else {
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

/// Entry of `wrong_branch` for a detached HEAD.
pub const ENTRY_DETACHED_HEAD: &str = "(detached HEAD)";
/// Entry of `wrong_branch` for a branch without a commit, or a HEAD whose object is broken.
pub const ENTRY_UNBORN_HEAD: &str = "(unborn HEAD)";
/// Entry of `merge_conflict` for two histories with no common ancestor.
pub const ENTRY_UNRELATED_HISTORIES: &str = "(unrelated histories)";
/// Entry of `dirty_tree` for local files the merge would overwrite.
pub const ENTRY_WOULD_OVERWRITE: &str = "(would overwrite local files)";

/// The files of the git dir that show an operation in progress, for `git rev-parse --git-path`.
const OPERATION_MARKERS: [&str; 7] = [
    "MERGE_HEAD",
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
    "rebase-merge",
    "rebase-apply",
    "sequencer",
    "BISECT_LOG",
];

/// The paths of `git status --porcelain=v1 -z`: records are `XY <path>` NUL, and an R or C record has the original
/// path as one more field, which is skipped (`--no-renames` makes git print none).
fn parse_status_z(out: &[u8]) -> Vec<Vec<u8>> {
    let mut fields = out.split(|byte| *byte == 0);
    let mut paths = Vec::new();
    while let Some(record) = fields.next() {
        if record.len() < 4 || record[2] != b' ' {
            continue;
        }
        if matches!(record[0], b'R' | b'C') || matches!(record[1], b'R' | b'C') {
            fields.next();
        }
        paths.push(record[3..].to_vec());
    }
    paths
}

/// What a ready preflight computed for the merge itself (plan-123 reads it).
#[allow(dead_code)]
#[derive(Debug, Clone)]
struct MergePlan {
    tree: String,
    identity: (String, String),
}

/// The tree id and the conflicted paths of `git merge-tree --write-tree --name-only --no-messages -z` that exited 1;
/// trailing empty items are ignored, and `None` when the tree id or every path is missing.
fn parse_merge_tree(out: &[u8]) -> Option<(String, Vec<Vec<u8>>)> {
    let mut items: Vec<&[u8]> = out.split(|byte| *byte == 0).collect();
    while items.last().is_some_and(|last| last.is_empty()) {
        items.pop();
    }
    let (tree, paths) = items.split_first()?;
    let tree = tree_id(tree)?;
    let paths: Vec<Vec<u8>> = paths
        .iter()
        .filter(|path| !path.is_empty())
        .map(|path| path.to_vec())
        .collect();
    if paths.is_empty() {
        return None;
    }
    Some((tree, paths))
}

/// An object id: hexadecimal digits of one of git's lengths.
fn tree_id(raw: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(raw).ok()?;
    let ok = matches!(text.len(), 40 | 64) && text.bytes().all(|b| b.is_ascii_hexdigit());
    ok.then(|| text.to_string())
}

fn is_commit_id(text: &str) -> bool {
    text.len() == 40
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn refused(code: &'static str, names: &[&str]) -> Preflight {
    Preflight::Refused(Refusal::new(
        code,
        names.iter().map(|name| name.as_bytes().to_vec()).collect(),
    ))
}

impl GitRepo {
    /// The branch a merge goes into: `configured` when given (a bad name is `invalid_branch`), else the branch
    /// `refs/remotes/origin/HEAD` names, else `None`.
    pub fn merge_target(&self, configured: Option<&str>) -> IntegrateResult<Option<String>> {
        if let Some(name) = configured {
            capstan_config::remote::validate_branch_name(name).map_err(|reason| {
                integration_error("invalid_branch", format!("the branch {name} {reason}"))
            })?;
            return Ok(Some(name.to_string()));
        }
        let deadline = Deadline::new(GIT_TIMEOUT);
        let out = self.run_merge(
            &["symbolic-ref", "-q", "refs/remotes/origin/HEAD"],
            &deadline,
            true,
            &[],
        )?;
        match out.code {
            0 => {}
            1 => return Ok(None),
            code => {
                return Err(unavailable(format!(
                    "origin/HEAD could not be read (git exited {code})"
                )))
            }
        }
        let text = out.stdout.strip_suffix(b"\n").unwrap_or(&out.stdout);
        let name = std::str::from_utf8(text)
            .ok()
            .and_then(|text| text.strip_prefix("refs/remotes/origin/"));
        Ok(name
            .filter(|name| capstan_config::remote::validate_branch_name(name).is_ok())
            .map(str::to_string))
    }

    /// The check before a merge into the root checkout: nothing is written. Steps 1 to 10 of the preflight.
    pub fn root_merge_preflight(
        &self,
        checkout_branch: &str,
        head_sha: &str,
        timeout: Duration,
    ) -> IntegrateResult<Preflight> {
        Ok(self
            .preflight_inner(checkout_branch, head_sha, &Deadline::new(timeout))?
            .0)
    }

    /// One git exit code of a merge check; any exit but `allowed` is `git_unavailable`.
    fn check_code(
        &self,
        args: &[&str],
        deadline: &Deadline,
        allowed: &[i32],
    ) -> IntegrateResult<RootOutput> {
        let out = self.run_merge(args, deadline, true, &[])?;
        if allowed.contains(&out.code) {
            return Ok(out);
        }
        Err(unavailable(format!(
            "git {} exited {}",
            args.first().copied().unwrap_or(""),
            out.code
        )))
    }

    fn preflight_inner(
        &self,
        checkout_branch: &str,
        head_sha: &str,
        deadline: &Deadline,
    ) -> IntegrateResult<(Preflight, Option<MergePlan>)> {
        let refuse = |preflight: Preflight| Ok((preflight, None));
        // 0
        if self.git_too_old(deadline)? {
            return refuse(refused("git_too_old", &[]));
        }
        // 1
        if let Some(refusal) = self
            .unsafe_repo_config_in(UnsafeScope::Merge, Some(deadline))?
            .into_refusal()
        {
            return refuse(Preflight::Refused(refusal));
        }
        // 2
        let commit = format!("{head_sha}^{{commit}}");
        if !is_commit_id(head_sha)
            || self
                .check_code(
                    &["rev-parse", "--verify", "--quiet", &commit],
                    deadline,
                    &[0, 1],
                )?
                .code
                == 1
        {
            return refuse(refused("invalid_commit", &[head_sha]));
        }
        // 3
        if capstan_config::remote::validate_branch_name(checkout_branch).is_err()
            || matches!(
                self.check_code(
                    &["check-ref-format", "--branch", checkout_branch],
                    deadline,
                    &[0, 1, 128],
                )?
                .code,
                1 | 128
            )
        {
            return refuse(refused("invalid_branch", &[checkout_branch]));
        }
        // 4
        let head = self.check_code(&["symbolic-ref", "-q", "HEAD"], deadline, &[0, 1])?;
        if head.code == 1 {
            return refuse(refused("wrong_branch", &[ENTRY_DETACHED_HEAD]));
        }
        let full = head.stdout.strip_suffix(b"\n").unwrap_or(&head.stdout);
        if full != format!("refs/heads/{checkout_branch}").as_bytes() {
            let text = String::from_utf8_lossy(full);
            let shown = text.strip_prefix("refs/heads/").unwrap_or(&text);
            return refuse(refused("wrong_branch", &[shown]));
        }
        let old = self.check_code(
            &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"],
            deadline,
            &[0, 1],
        )?;
        if old.code == 1 {
            return refuse(refused("wrong_branch", &[ENTRY_UNBORN_HEAD]));
        }
        let old_head = String::from_utf8_lossy(&old.stdout).trim().to_string();
        if old_head.is_empty() || !old_head.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(unavailable("git printed no commit for HEAD"));
        }
        // 5
        let ancestor = self.check_code(
            &["merge-base", "--is-ancestor", head_sha, &old_head],
            deadline,
            &[0, 1],
        )?;
        if ancestor.code == 0 {
            return Ok((Preflight::AlreadyInHead, None));
        }
        // 6
        let Some(identity) = self.strict_identity(deadline)? else {
            let mut missing: Vec<&str> = Vec::new();
            for key in ["user.name", "user.email"] {
                if self.identity_value(key, deadline)?.is_none() {
                    missing.push(key);
                }
            }
            return refuse(refused("no_identity", &missing));
        };
        // 7
        if let Some(refusal) = self.dirty_refusal(deadline)? {
            return refuse(Preflight::Refused(refusal));
        }
        // 9
        let base = self.check_code(&["merge-base", &old_head, head_sha], deadline, &[0, 1])?;
        if base.code == 1 {
            return refuse(refused("merge_conflict", &[ENTRY_UNRELATED_HISTORIES]));
        }
        let merged = self.check_code(
            &[
                "merge-tree",
                "--write-tree",
                "--name-only",
                "--no-messages",
                "-z",
                &old_head,
                head_sha,
            ],
            deadline,
            &[0, 1],
        )?;
        let tree = if merged.code == 0 {
            let first = merged.stdout.split(|byte| *byte == 0).next().unwrap_or(&[]);
            tree_id(first).ok_or_else(|| unavailable("git merge-tree printed no tree"))?
        } else {
            let Some((_, paths)) = parse_merge_tree(&merged.stdout) else {
                return Err(unavailable("git merge-tree printed no conflict"));
            };
            return refuse(Preflight::Refused(Refusal::new("merge_conflict", paths)));
        };
        // 10
        if let Some(refusal) = self.ignored_refusal(&old_head, &tree, deadline)? {
            return refuse(Preflight::Refused(refusal));
        }
        Ok((
            Preflight::Ready {
                old_head: old_head.clone(),
            },
            Some(MergePlan { tree, identity }),
        ))
    }

    /// Step 7: an operation in progress (a marker in the git dir) or a tracked change (staged, in the worktree, or a
    /// submodule at another commit than its gitlink) is `dirty_tree`.
    fn dirty_refusal(&self, deadline: &Deadline) -> IntegrateResult<Option<Refusal>> {
        let mut args = vec!["rev-parse"];
        for marker in OPERATION_MARKERS {
            args.push("--git-path");
            args.push(marker);
        }
        let paths = self.check_code(&args, deadline, &[0])?;
        let text = String::from_utf8_lossy(&paths.stdout);
        let lines: Vec<&str> = text.lines().collect();
        if lines.len() != OPERATION_MARKERS.len() {
            return Err(unavailable("git printed the wrong number of marker paths"));
        }
        let mut found: Vec<Vec<u8>> = Vec::new();
        for (marker, line) in OPERATION_MARKERS.iter().zip(lines) {
            let path = self.root.join(Path::new(line));
            if std::fs::symlink_metadata(&path).is_ok() {
                found.push(marker.as_bytes().to_vec());
            }
        }
        if !found.is_empty() {
            return Ok(Some(Refusal::new("dirty_tree", found)));
        }
        let status = self.check_code(
            &[
                "status",
                "--porcelain=v1",
                "-z",
                "--no-renames",
                "--untracked-files=no",
                "--ignore-submodules=dirty",
            ],
            deadline,
            &[0],
        )?;
        let changed = parse_status_z(&status.stdout);
        if changed.is_empty() {
            return Ok(None);
        }
        Ok(Some(Refusal::new("dirty_tree", changed)))
    }

    /// Step 10: an ignored file the merge would overwrite (`read-tree -m -u` replaces ignored files without a word,
    /// where an untracked file that is not ignored stops it). The changed paths of `old_head..tree` are looked up as
    /// literal pathspecs, with every parent of one that is a file in the working tree; only entries at one of them,
    /// or under a changed path, count. Writes nothing.
    fn ignored_refusal(
        &self,
        old_head: &str,
        tree: &str,
        deadline: &Deadline,
    ) -> IntegrateResult<Option<Refusal>> {
        let icase = self.ignore_case(deadline)?;
        let diff = self.check_code(
            &[
                "diff-tree",
                "-r",
                "-z",
                "--name-only",
                "--no-renames",
                old_head,
                tree,
            ],
            deadline,
            &[0],
        )?;
        let mut changed: Vec<Vec<u8>> = diff
            .stdout
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(<[u8]>::to_vec)
            .collect();
        changed.sort();
        changed.dedup();
        let parents = self.non_dir_parents(&changed, icase);
        let chunks = ignored_check_pathspecs(&changed, &parents, icase);
        let fold = |bytes: &[u8]| -> Vec<u8> {
            if icase {
                bytes.to_ascii_lowercase()
            } else {
                bytes.to_vec()
            }
        };
        let changed_set: HashSet<Vec<u8>> = changed.iter().map(|path| fold(path)).collect();
        let exact_set: HashSet<Vec<u8>> = changed_set
            .iter()
            .cloned()
            .chain(parents.iter().map(|path| fold(path)))
            .collect();
        let mut refused: Vec<Vec<u8>> = Vec::new();
        for chunk in chunks {
            let mut args = vec![
                "ls-files",
                "-z",
                "--others",
                "--ignored",
                "--exclude-standard",
                "--",
            ];
            args.extend(chunk.specs.iter().map(String::as_str));
            let listing = match self.run_merge(&args, deadline, true, &[]) {
                Ok(out) => out,
                Err(IntegrateError::Integration { code, .. })
                    if code == "output_too_large" && !chunk.fallback =>
                {
                    refused.push(ENTRY_WOULD_OVERWRITE.as_bytes().to_vec());
                    return Ok(Some(Refusal::new("dirty_tree", refused)));
                }
                Err(error) => return Err(error),
            };
            if listing.code != 0 {
                return Err(unavailable(format!("git ls-files exited {}", listing.code)));
            }
            for entry in listing.stdout.split(|byte| *byte == 0) {
                if entry.is_empty() {
                    continue;
                }
                let folded = fold(entry);
                let under_changed = folded
                    .iter()
                    .enumerate()
                    .filter(|(_, byte)| **byte == b'/')
                    .any(|(at, _)| changed_set.contains(&folded[..at]));
                if exact_set.contains(&folded) || under_changed {
                    refused.push(entry.to_vec());
                }
            }
            if refused.len() >= MAX_CONFLICT_FILES * 2 {
                break;
            }
        }
        if refused.is_empty() {
            return Ok(None);
        }
        Ok(Some(Refusal::new("dirty_tree", refused)))
    }

    /// `core.ignorecase` as the local and worktree config give it; unset is false.
    fn ignore_case(&self, deadline: &Deadline) -> IntegrateResult<bool> {
        let out = self.check_code(
            &["config", "--get", "--bool", "core.ignorecase"],
            deadline,
            &[0, 1],
        )?;
        Ok(out.code == 0 && out.stdout.trim_ascii() == b"true")
    }

    /// The parents of `changed` paths that exist in the working tree as something other than a directory (a
    /// symlink to a directory counts, it is not one to git). With `icase` a parent is also found under any ASCII case
    /// variant of its name, by reading the directories; the names returned are the ones on disk.
    fn non_dir_parents(&self, changed: &[Vec<u8>], icase: bool) -> Vec<Vec<u8>> {
        use std::os::unix::ffi::OsStringExt;
        let at = |relative: &[u8]| self.root.join(std::ffi::OsStr::from_bytes(relative));
        let mut found: BTreeSet<Vec<u8>> = BTreeSet::new();
        // The on-disk directories each folded parent prefix stands for (`icase` only).
        let mut resolved: HashMap<Vec<u8>, Vec<Vec<u8>>> = HashMap::new();
        resolved.insert(Vec::new(), vec![Vec::new()]);
        let mut seen: HashSet<Vec<u8>> = HashSet::new();
        for path in changed {
            let parts: Vec<&[u8]> = path.split(|byte| *byte == b'/').collect();
            let mut prefix: Vec<u8> = Vec::new();
            for part in &parts[..parts.len().saturating_sub(1)] {
                let parent = prefix.clone();
                if !prefix.is_empty() {
                    prefix.push(b'/');
                }
                prefix.extend_from_slice(part);
                if !icase {
                    if !seen.insert(prefix.clone()) {
                        continue;
                    }
                    match std::fs::symlink_metadata(at(&prefix)) {
                        Ok(meta) if meta.is_dir() => {}
                        Ok(_) => {
                            found.insert(prefix.clone());
                        }
                        Err(_) => break,
                    }
                    continue;
                }
                let key = prefix.to_ascii_lowercase();
                if !resolved.contains_key(&key) {
                    let parent_key = parent.to_ascii_lowercase();
                    let mut actual: Vec<Vec<u8>> = Vec::new();
                    for dir in resolved.get(&parent_key).cloned().unwrap_or_default() {
                        let Ok(entries) = std::fs::read_dir(at(&dir)) else {
                            continue;
                        };
                        for entry in entries.flatten() {
                            let name = entry.file_name().into_vec();
                            if !name.eq_ignore_ascii_case(part) {
                                continue;
                            }
                            let mut full = dir.clone();
                            if !full.is_empty() {
                                full.push(b'/');
                            }
                            full.extend_from_slice(&name);
                            match std::fs::symlink_metadata(at(&full)) {
                                Ok(meta) if meta.is_dir() => actual.push(full),
                                Ok(_) => {
                                    found.insert(full);
                                }
                                Err(_) => {}
                            }
                        }
                    }
                    resolved.insert(key.clone(), actual);
                }
                if resolved.get(&key).is_none_or(|dirs| dirs.is_empty()) {
                    break;
                }
            }
        }
        found.into_iter().collect()
    }
}

/// One `ls-files` call of step 10: the pathspecs, and whether any of them is an ancestor standing in for a path
/// git cannot be given (so the listing may hold ignored files the merge does not touch).
#[derive(Debug, PartialEq, Eq)]
struct PathspecChunk {
    specs: Vec<String>,
    fallback: bool,
}

const PATHSPEC_CHUNK_COUNT: usize = 1000;
const PATHSPEC_CHUNK_BYTES: usize = 64 * 1024;

/// The pathspecs of step 10 in chunks: `:(literal)<path>` per changed path and non-directory parent
/// (`:(icase,literal)` with `icase`: git applies no `core.ignorecase` to a literal pathspec). A path that is not
/// UTF-8 or holds a newline cannot be passed as an argument and becomes its nearest ancestor directory that can
/// (same magic), else the plain `.`; those go in chunks of their own with `fallback` set. A chunk holds at most 1000
/// pathspecs and 64 KiB of arguments.
fn ignored_check_pathspecs(
    changed: &[Vec<u8>],
    non_dir_parents: &[Vec<u8>],
    icase: bool,
) -> Vec<PathspecChunk> {
    let magic = if icase {
        ":(icase,literal)"
    } else {
        ":(literal)"
    };
    let mut exact: BTreeSet<String> = BTreeSet::new();
    let mut stand_in: BTreeSet<String> = BTreeSet::new();
    for path in changed.iter().chain(non_dir_parents) {
        if path.is_empty() {
            continue;
        }
        match std::str::from_utf8(path) {
            Ok(text) if !text.contains('\n') => {
                exact.insert(format!("{magic}{text}"));
            }
            _ => {
                let mut good = 0;
                let mut at = 0;
                for part in path.split(|byte| *byte == b'/') {
                    if std::str::from_utf8(part).is_err() || part.contains(&b'\n') {
                        break;
                    }
                    at += part.len();
                    good = at;
                    at += 1;
                }
                let ancestor = &path[..good.min(path.len())];
                if ancestor.is_empty() || good == 0 {
                    stand_in.insert(".".to_string());
                } else {
                    let text = String::from_utf8_lossy(ancestor);
                    stand_in.insert(format!("{magic}{text}"));
                }
            }
        }
    }
    let mut chunks = Vec::new();
    for (specs, fallback) in [(exact, false), (stand_in, true)] {
        let mut current: Vec<String> = Vec::new();
        let mut bytes = 0;
        for spec in specs {
            if !current.is_empty()
                && (current.len() >= PATHSPEC_CHUNK_COUNT
                    || bytes + spec.len() > PATHSPEC_CHUNK_BYTES)
            {
                chunks.push(PathspecChunk {
                    specs: std::mem::take(&mut current),
                    fallback,
                });
                bytes = 0;
            }
            bytes += spec.len();
            current.push(spec);
        }
        if !current.is_empty() {
            chunks.push(PathspecChunk {
                specs: current,
                fallback,
            });
        }
    }
    chunks
}

// ----------------------------------------------------------------------------------------------- merge commit

/// The time the rollback and the re-reads after a failed write have in all.
const MERGE_RECOVER_TIME: Duration = Duration::from_secs(20);

/// The reasons `classify_failure` returns; equal to `DOC_FAILURE_REASONS` (a unit test compares them).
pub(crate) const MERGE_FAILURE_REASONS: &[&str] = &["merge_failed", "timeout", "index_lock_left"];

/// Switches for tests of `merge_into_root`, all off by default; nothing but a test sets them. They travel with a
/// clone, apart from `GitSeams`.
#[doc(hidden)]
#[derive(Debug, Clone, Default)]
pub struct MergeSeams {
    pub lock_after_refresh_killed: bool,
    pub expire_rollback_deadline: bool,
    pub move_branch_before_update_ref: bool,
    pub hold_lock_before_read_tree: bool,
    pub hold_lock_before_refresh: bool,
    pub hold_ref_lock_before_update_ref: bool,
    pub force_update_ref_timeout: bool,
    /// A commit to move the branch to (and check out) before read-tree.
    pub move_branch_before_read_tree: Option<String>,
    /// Read-tree runs, then its result is treated as an unknown failure.
    pub fail_read_tree_after_write: bool,
    /// After read-tree returns, index.lock is created and the child is treated as killed at the deadline.
    pub lock_after_read_tree_killed: bool,
    /// The main deadline is treated as passed: update-ref is not run and the ref is read.
    pub expire_deadline_before_update_ref: bool,
    /// Applied after the preflight re-run, before the refresh.
    pub edit_before_read_tree: Option<(PathBuf, Vec<u8>)>,
    pub create_file_after_read_tree: Option<(PathBuf, Vec<u8>)>,
    pub edit_before_rollback: Option<(PathBuf, Vec<u8>)>,
}

/// The step of the merge that failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailStep {
    Refresh,
    ReadTree,
    UpdateRef,
}

/// Where the checkout branch points after a failed step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefAfter {
    New,
    OldHead,
    Other,
    Unknown,
}

/// What is known of the tree after a failure; `Rollback` is resolved by running the rollback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum After {
    Restored,
    NotRestored,
    Rollback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailDecision {
    Merged,
    RefuseDirty,
    Fail { reason: &'static str, after: After },
}

/// What a failed step of the merge means, first matching row wins (rows F1 to U3 of the plan).
fn classify_failure(
    step: FailStep,
    lock_before: bool,
    lock_after: bool,
    killed_at_deadline: bool,
    stderr: &str,
    ref_after: RefAfter,
) -> FailDecision {
    let fail = |reason, after| FailDecision::Fail { reason, after };
    let reason = if killed_at_deadline {
        "timeout"
    } else {
        "merge_failed"
    };
    let rollback = if ref_after == RefAfter::OldHead {
        After::Rollback
    } else {
        After::NotRestored
    };
    match step {
        FailStep::Refresh => {
            if killed_at_deadline && lock_after && !lock_before {
                // F1
                fail("index_lock_left", After::NotRestored)
            } else {
                // F2
                fail(reason, After::Restored)
            }
        }
        FailStep::ReadTree => {
            if lock_before {
                // R1
                fail("merge_failed", After::Restored)
            } else if lock_after {
                // R2 and R2b
                fail("index_lock_left", After::NotRestored)
            } else if killed_at_deadline {
                // R3
                fail("timeout", rollback)
            } else if stderr.contains("index.lock") {
                // R4
                fail("merge_failed", After::Restored)
            } else if stderr.contains("would be overwritten") || stderr.contains("not uptodate") {
                // R5
                FailDecision::RefuseDirty
            } else {
                // R6
                fail("merge_failed", rollback)
            }
        }
        FailStep::UpdateRef => match ref_after {
            // U1
            RefAfter::New => FailDecision::Merged,
            // U2
            RefAfter::OldHead => fail(reason, After::Rollback),
            // U3
            RefAfter::Other | RefAfter::Unknown => fail(reason, After::NotRestored),
        },
    }
}

/// The failure of a decision whose rollback has run (a `Rollback` counts as not restored).
fn failed_from(decision: FailDecision) -> RootMerge {
    match decision {
        FailDecision::Fail { reason, after } => {
            debug_assert!(MERGE_FAILURE_REASONS.contains(&reason));
            RootMerge::failed(reason, after == After::Restored)
        }
        FailDecision::Merged | FailDecision::RefuseDirty => {
            RootMerge::failed("merge_failed", false)
        }
    }
}

/// The paths read-tree names in its `Entry '<path>' ...` lines.
fn read_tree_entries(stderr: &str) -> Vec<Vec<u8>> {
    let mut names = Vec::new();
    for line in stderr.lines() {
        let Some(at) = line.find("Entry '") else {
            continue;
        };
        let rest = &line[at + "Entry '".len()..];
        let end = ["' would be", "' not uptodate", "' "]
            .iter()
            .filter_map(|marker| rest.find(marker))
            .min();
        if let Some(end) = end {
            names.push(rest.as_bytes()[..end].to_vec());
        }
    }
    names
}

fn refused_merge(code: &'static str, names: &[&str]) -> RootMerge {
    RootMerge::Refused(Refusal::new(
        code,
        names.iter().map(|name| name.as_bytes().to_vec()).collect(),
    ))
}

/// One git child of the merge, with a timeout and any other failure to run it folded into the code (-1).
struct Step {
    code: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    killed: bool,
}

fn lock_present(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

impl GitRepo {
    /// Test-only switches of `merge_into_root`; plan-121 must not use them.
    #[doc(hidden)]
    pub fn with_merge_seams(mut self, seams: MergeSeams) -> Self {
        self.merge_seams = seams;
        self
    }

    /// Writes the merge of `head_sha` into the checked-out `checkout_branch` of the root as a commit with parents
    /// `old_head` and `head_sha`, updating index and working tree. The preflight runs again first, so a refusal it
    /// gives is returned as is.
    pub fn merge_into_root(
        &self,
        checkout_branch: &str,
        merged_branch: &str,
        head_sha: &str,
        old_head: &str,
        timeout: Duration,
    ) -> IntegrateResult<RootMerge> {
        let deadline = Deadline::new(timeout);
        let commit = format!("{old_head}^{{commit}}");
        if !is_commit_id(old_head)
            || self
                .check_code(
                    &["rev-parse", "--verify", "--quiet", &commit],
                    &deadline,
                    &[0, 1],
                )?
                .code
                == 1
        {
            return Ok(refused_merge("invalid_commit", &[old_head]));
        }
        if capstan_config::remote::validate_branch_name(merged_branch).is_err() {
            return Ok(refused_merge("invalid_branch", &[merged_branch]));
        }
        let (preflight, plan) = self.preflight_inner(checkout_branch, head_sha, &deadline)?;
        match preflight {
            Preflight::AlreadyInHead => return Ok(RootMerge::AlreadyInHead),
            Preflight::Refused(refusal) => return Ok(RootMerge::Refused(refusal)),
            Preflight::Ready { old_head: current } => {
                if current != old_head {
                    return Ok(RootMerge::failed("merge_failed", true));
                }
            }
        }
        let Some(plan) = plan else {
            return Err(unavailable("the preflight printed no merge plan"));
        };
        self.write_merge(
            checkout_branch,
            merged_branch,
            head_sha,
            old_head,
            &plan,
            &deadline,
        )
    }

    /// The branches a remote merge may go into: the one checked out in the project's root (`checked_out`, none on a
    /// detached or unborn HEAD) and every other local branch that no worktree has checked out (a worker's branch is
    /// checked out in its worktree) and that is not an integration branch. Sorted, read-only.
    pub fn merge_targets(&self, timeout: Duration) -> IntegrateResult<MergeTargets> {
        let deadline = Deadline::new(timeout);
        let head = self.check_code(&["symbolic-ref", "-q", "HEAD"], &deadline, &[0, 1])?;
        let checked_out = (head.code == 0)
            .then(|| String::from_utf8_lossy(head.stdout.strip_suffix(b"\n").unwrap_or(&head.stdout)).to_string())
            .and_then(|full| full.strip_prefix("refs/heads/").map(str::to_string));
        let busy = self.worktree_branches(&deadline)?;
        let listed = self.check_code(
            &["rev-parse", "--symbolic-full-name", "--branches"],
            &deadline,
            &[0],
        )?;
        let mut branches: Vec<String> = String::from_utf8_lossy(&listed.stdout)
            .lines()
            .filter_map(|line| line.strip_prefix("refs/heads/"))
            .filter(|name| !name.starts_with("integration/"))
            .filter(|name| capstan_config::remote::validate_branch_name(name).is_ok())
            .filter(|name| Some(*name) == checked_out.as_deref() || !busy.contains(*name))
            .map(str::to_string)
            .collect();
        branches.sort();
        branches.dedup();
        Ok(MergeTargets {
            checked_out,
            branches,
        })
    }

    /// The branches checked out in any worktree of the repository, the root's included: the root's `HEAD`, and the
    /// `HEAD` file of each linked worktree under the common dir (read as files; no git command lists them).
    fn worktree_branches(&self, deadline: &Deadline) -> IntegrateResult<HashSet<String>> {
        let mut found = HashSet::new();
        let head = self.check_code(&["symbolic-ref", "-q", "HEAD"], deadline, &[0, 1])?;
        if head.code == 0 {
            let text = String::from_utf8_lossy(&head.stdout).trim().to_string();
            if let Some(name) = text.strip_prefix("refs/heads/") {
                found.insert(name.to_string());
            }
        }
        let common = self.check_code(&["rev-parse", "--git-common-dir"], deadline, &[0])?;
        let text = String::from_utf8_lossy(&common.stdout).trim().to_string();
        let dir = self.root.join(&text).join("worktrees");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Ok(found);
        };
        for entry in entries.flatten() {
            let Ok(head) = std::fs::read_to_string(entry.path().join("HEAD")) else {
                continue;
            };
            if let Some(name) = head.trim().strip_prefix("ref: refs/heads/") {
                found.insert(name.to_string());
            }
        }
        Ok(found)
    }

    /// Merges `head_sha` (the tip of `merged_branch`) into `target`, a local branch that no worktree has checked out,
    /// without touching any working tree: `merge-tree` writes the merged tree, `commit-tree` the merge commit, and
    /// `update-ref` moves the branch only if it still points where it did (a branch that moved meanwhile fails with
    /// nothing changed). The same refusals as the merge into the root, except `dirty_tree`, which no working tree can
    /// cause here; a target checked out in a worktree is `wrong_branch`.
    pub fn merge_into_ref(
        &self,
        target: &str,
        merged_branch: &str,
        head_sha: &str,
        timeout: Duration,
    ) -> IntegrateResult<RootMerge> {
        let deadline = Deadline::new(timeout);
        if self.git_too_old(&deadline)? {
            return Ok(refused_merge("git_too_old", &[]));
        }
        if let Some(refusal) = self
            .unsafe_repo_config_in(UnsafeScope::Merge, Some(&deadline))?
            .into_refusal()
        {
            return Ok(RootMerge::Refused(refusal));
        }
        let commit = format!("{head_sha}^{{commit}}");
        if !is_commit_id(head_sha)
            || self
                .check_code(&["rev-parse", "--verify", "--quiet", &commit], &deadline, &[0, 1])?
                .code
                == 1
        {
            return Ok(refused_merge("invalid_commit", &[head_sha]));
        }
        if capstan_config::remote::validate_branch_name(merged_branch).is_err() {
            return Ok(refused_merge("invalid_branch", &[merged_branch]));
        }
        if capstan_config::remote::validate_branch_name(target).is_err()
            || target.starts_with("integration/")
            || self
                .check_code(&["check-ref-format", "--branch", target], &deadline, &[0, 1, 128])?
                .code
                != 0
        {
            return Ok(refused_merge("invalid_branch", &[target]));
        }
        let reference = format!("refs/heads/{target}");
        let tip = self.check_code(
            &["rev-parse", "--verify", "--quiet", &format!("{reference}^{{commit}}")],
            &deadline,
            &[0, 1],
        )?;
        if tip.code == 1 {
            return Ok(refused_merge("invalid_branch", &[target]));
        }
        let old = String::from_utf8_lossy(&tip.stdout).trim().to_string();
        if !is_commit_id(&old) {
            return Err(unavailable("git printed no commit for the target branch"));
        }
        if self.worktree_branches(&deadline)?.contains(target) {
            return Ok(refused_merge("wrong_branch", &[target]));
        }
        let ancestor = self.check_code(
            &["merge-base", "--is-ancestor", head_sha, &old],
            &deadline,
            &[0, 1],
        )?;
        if ancestor.code == 0 {
            return Ok(RootMerge::AlreadyInHead);
        }
        let Some((name, email)) = self.strict_identity(&deadline)? else {
            let mut missing: Vec<&str> = Vec::new();
            for key in ["user.name", "user.email"] {
                if self.identity_value(key, &deadline)?.is_none() {
                    missing.push(key);
                }
            }
            return Ok(refused_merge("no_identity", &missing));
        };
        let base = self.check_code(&["merge-base", &old, head_sha], &deadline, &[0, 1])?;
        if base.code == 1 {
            return Ok(refused_merge("merge_conflict", &[ENTRY_UNRELATED_HISTORIES]));
        }
        let merged = self.check_code(
            &[
                "merge-tree",
                "--write-tree",
                "--name-only",
                "--no-messages",
                "-z",
                &old,
                head_sha,
            ],
            &deadline,
            &[0, 1],
        )?;
        let tree = if merged.code == 0 {
            let first = merged.stdout.split(|byte| *byte == 0).next().unwrap_or(&[]);
            tree_id(first).ok_or_else(|| unavailable("git merge-tree printed no tree"))?
        } else {
            let Some((_, paths)) = parse_merge_tree(&merged.stdout) else {
                return Err(unavailable("git merge-tree printed no conflict"));
            };
            return Ok(RootMerge::Refused(Refusal::new("merge_conflict", paths)));
        };
        let message = format!("Merge branch '{merged_branch}' into {target}");
        let created = self.step(
            &["commit-tree", &tree, "-p", &old, "-p", head_sha, "-m", &message],
            &deadline,
            false,
            &[
                ("GIT_AUTHOR_NAME", name.as_str()),
                ("GIT_AUTHOR_EMAIL", email.as_str()),
                ("GIT_COMMITTER_NAME", name.as_str()),
                ("GIT_COMMITTER_EMAIL", email.as_str()),
                ("TZ", "UTC"),
            ],
        );
        let new = String::from_utf8_lossy(&created.stdout).trim().to_string();
        if created.code != 0 || !is_commit_id(&new) {
            return Ok(if created.killed {
                RootMerge::failed("timeout", true)
            } else {
                RootMerge::failed("merge_failed", true)
            });
        }
        let moved = self.step(
            &["update-ref", "-m", &message, &reference, &new, &old],
            &deadline,
            false,
            &[
                ("GIT_COMMITTER_NAME", name.as_str()),
                ("GIT_COMMITTER_EMAIL", email.as_str()),
            ],
        );
        if moved.code != 0 {
            // The branch still points where it did (update-ref is all or nothing), unless it moved meanwhile.
            return Ok(if moved.killed {
                RootMerge::failed("timeout", true)
            } else {
                RootMerge::failed("merge_failed", true)
            });
        }
        Ok(RootMerge::Merged { commit: new })
    }

    /// Whether `sha` is in the history of the local branch `branch`.
    pub fn is_in_branch(&self, sha: &str, branch: &str, timeout: Duration) -> IntegrateResult<bool> {
        let deadline = Deadline::new(timeout);
        if !is_commit_id(sha) || capstan_config::remote::validate_branch_name(branch).is_err() {
            return Ok(false);
        }
        let reference = format!("refs/heads/{branch}");
        Ok(self
            .check_code(&["merge-base", "--is-ancestor", sha, &reference], &deadline, &[0, 1, 128])?
            .code
            == 0)
    }

    /// Runs one git child and folds a timeout (`killed`) or any other failure to run it into the step.
    fn step(
        &self,
        args: &[&str],
        deadline: &Deadline,
        read_only: bool,
        extra_env: &[(&str, &str)],
    ) -> Step {
        match self.run_merge(args, deadline, read_only, extra_env) {
            Ok(out) => Step {
                code: out.code,
                stdout: out.stdout,
                stderr: out.stderr,
                killed: false,
            },
            Err(error) => Step {
                code: -1,
                stdout: Vec::new(),
                stderr: Vec::new(),
                killed: matches!(
                    &error,
                    IntegrateError::Integration { code, .. } if code == "timeout" || code == "killed"
                ),
            },
        }
    }

    /// `git rev-parse --git-path <name>` as a path in the file system.
    fn git_path_of(&self, name: &str, deadline: &Deadline) -> IntegrateResult<PathBuf> {
        let out = self.check_code(&["rev-parse", "--git-path", name], deadline, &[0])?;
        let text = out.stdout.strip_suffix(b"\n").unwrap_or(&out.stdout);
        if text.is_empty() {
            return Err(unavailable("git printed no path"));
        }
        Ok(self.root.join(OsStr::from_bytes(text)))
    }

    fn touch_file(&self, path: &Path) {
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path);
    }

    fn put_file(&self, edit: &Option<(PathBuf, Vec<u8>)>) {
        if let Some((path, bytes)) = edit {
            let _ = std::fs::write(self.root.join(path), bytes);
        }
    }

    /// Where `refs/heads/<branch>` points now.
    fn ref_after(&self, branch: &str, new: &str, old_head: &str, deadline: &Deadline) -> RefAfter {
        let name = format!("refs/heads/{branch}");
        let step = self.step(
            &["rev-parse", "--verify", "--quiet", &name],
            deadline,
            true,
            &[],
        );
        match step.code {
            0 => {
                let text = String::from_utf8_lossy(&step.stdout).trim().to_string();
                if text == new {
                    RefAfter::New
                } else if text == old_head {
                    RefAfter::OldHead
                } else {
                    RefAfter::Other
                }
            }
            1 => RefAfter::Other,
            _ => RefAfter::Unknown,
        }
    }

    /// Steps 1 to 4 of the merge commit.
    fn write_merge(
        &self,
        checkout_branch: &str,
        merged_branch: &str,
        head_sha: &str,
        old_head: &str,
        plan: &MergePlan,
        deadline: &Deadline,
    ) -> IntegrateResult<RootMerge> {
        let seams = &self.merge_seams;
        let (name, email) = (plan.identity.0.as_str(), plan.identity.1.as_str());
        let lock = self.git_path_of("index.lock", deadline)?;
        let branch_ref = format!("refs/heads/{checkout_branch}");
        let committer = [("GIT_COMMITTER_NAME", name), ("GIT_COMMITTER_EMAIL", email)];

        // 1
        self.put_file(&seams.edit_before_read_tree);
        if seams.hold_lock_before_refresh {
            self.touch_file(&lock);
        }
        let lock_before = lock_present(&lock);
        let mut refresh = self.step(&["update-index", "-q", "--refresh"], deadline, false, &[]);
        if seams.lock_after_refresh_killed {
            self.touch_file(&lock);
            refresh.code = -1;
            refresh.killed = true;
        }
        if !matches!(refresh.code, 0 | 1) {
            let stderr = String::from_utf8_lossy(&refresh.stderr);
            let decision = classify_failure(
                FailStep::Refresh,
                lock_before,
                lock_present(&lock),
                refresh.killed,
                &stderr,
                RefAfter::Unknown,
            );
            return Ok(failed_from(decision));
        }

        // 2
        let message = format!("Merge branch '{merged_branch}'");
        let created = self.step(
            &[
                "commit-tree",
                &plan.tree,
                "-p",
                old_head,
                "-p",
                head_sha,
                "-m",
                &message,
            ],
            deadline,
            false,
            &[
                ("GIT_AUTHOR_NAME", name),
                ("GIT_AUTHOR_EMAIL", email),
                committer[0],
                committer[1],
                ("TZ", "UTC"),
            ],
        );
        let new = String::from_utf8_lossy(&created.stdout).trim().to_string();
        if created.code != 0 || !is_commit_id(&new) {
            return Ok(if created.killed {
                RootMerge::failed("timeout", true)
            } else {
                RootMerge::failed("merge_failed", true)
            });
        }

        // 3
        if let Some(commit) = &seams.move_branch_before_read_tree {
            self.step(
                &["update-ref", &branch_ref, commit],
                deadline,
                false,
                &committer,
            );
            self.step(
                &["read-tree", "--reset", "-u", commit],
                deadline,
                false,
                &[],
            );
        }
        if seams.hold_lock_before_read_tree {
            self.touch_file(&lock);
        }
        let lock_before = lock_present(&lock);
        let mut read = self.step(
            &["read-tree", "-m", "-u", old_head, &new],
            deadline,
            false,
            &[],
        );
        if seams.fail_read_tree_after_write {
            read.code = -1;
            read.killed = false;
            read.stderr.clear();
        }
        if let Some((path, bytes)) = &seams.create_file_after_read_tree {
            let _ = std::fs::write(self.root.join(path), bytes);
        }
        if seams.lock_after_read_tree_killed {
            self.touch_file(&lock);
            read.code = -1;
            read.killed = true;
        }
        if read.code != 0 {
            let recover = Deadline::new(MERGE_RECOVER_TIME);
            let ref_after = self.ref_after(checkout_branch, &new, old_head, &recover);
            let stderr = String::from_utf8_lossy(&read.stderr).into_owned();
            let decision = classify_failure(
                FailStep::ReadTree,
                lock_before,
                lock_present(&lock),
                read.killed,
                &stderr,
                ref_after,
            );
            if decision == FailDecision::RefuseDirty {
                let mut names = vec![ENTRY_WOULD_OVERWRITE.as_bytes().to_vec()];
                names.extend(read_tree_entries(&stderr));
                return Ok(RootMerge::Refused(Refusal::new("dirty_tree", names)));
            }
            return Ok(self.settle(decision, old_head, &new));
        }

        // 4
        if seams.move_branch_before_update_ref {
            self.step(
                &["update-ref", &branch_ref, head_sha],
                deadline,
                false,
                &committer,
            );
        }
        if seams.hold_ref_lock_before_update_ref {
            if let Ok(path) = self.git_path_of(&format!("{branch_ref}.lock"), deadline) {
                self.touch_file(&path);
            }
        }
        let log = format!("merge {merged_branch} (remote)");
        let update = if seams.force_update_ref_timeout || seams.expire_deadline_before_update_ref {
            Step {
                code: -1,
                stdout: Vec::new(),
                stderr: Vec::new(),
                killed: true,
            }
        } else {
            self.step(
                &["update-ref", "-m", &log, &branch_ref, &new, old_head],
                deadline,
                false,
                &committer,
            )
        };
        if update.code == 0 {
            return Ok(RootMerge::Merged { commit: new });
        }
        let recover = Deadline::new(MERGE_RECOVER_TIME);
        let ref_after = self.ref_after(checkout_branch, &new, old_head, &recover);
        let stderr = String::from_utf8_lossy(&update.stderr).into_owned();
        let decision = classify_failure(
            FailStep::UpdateRef,
            false,
            false,
            update.killed,
            &stderr,
            ref_after,
        );
        if decision == FailDecision::Merged {
            return Ok(RootMerge::Merged { commit: new });
        }
        Ok(self.settle(decision, old_head, &new))
    }

    /// The failure of `decision`, after the rollback when it asks for one.
    fn settle(&self, decision: FailDecision, old_head: &str, new: &str) -> RootMerge {
        match decision {
            FailDecision::Fail {
                reason,
                after: After::Rollback,
            } => {
                let restored = self.rollback(old_head, new);
                failed_from(FailDecision::Fail {
                    reason,
                    after: if restored {
                        After::Restored
                    } else {
                        After::NotRestored
                    },
                })
            }
            other => failed_from(other),
        }
    }

    /// Puts index and working tree back from `new` to `old_head`; whether they equal `old_head` over the merge's paths
    /// afterwards. Any failure, an untracked or ignored file at a merged path included, is `false`.
    fn rollback(&self, old_head: &str, new: &str) -> bool {
        let seams = &self.merge_seams;
        let deadline = Deadline::new(if seams.expire_rollback_deadline {
            Duration::ZERO
        } else {
            MERGE_RECOVER_TIME
        });
        self.put_file(&seams.edit_before_rollback);
        self.rollback_inner(old_head, new, &deadline)
            .unwrap_or(false)
    }

    fn rollback_inner(
        &self,
        old_head: &str,
        new: &str,
        deadline: &Deadline,
    ) -> IntegrateResult<bool> {
        let diff = self.check_code(
            &[
                "diff-tree",
                "-r",
                "-z",
                "--name-only",
                "--no-renames",
                old_head,
                new,
            ],
            deadline,
            &[0],
        )?;
        let mut changed: Vec<Vec<u8>> = diff
            .stdout
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(<[u8]>::to_vec)
            .collect();
        changed.sort();
        changed.dedup();
        let chunks = ignored_check_pathspecs(&changed, &[], false);
        for chunk in &chunks {
            let mut args = vec!["ls-files", "-z", "--others", "--"];
            args.extend(chunk.specs.iter().map(String::as_str));
            let listing = self.check_code(&args, deadline, &[0])?;
            if listing.stdout.iter().any(|byte| *byte != 0) {
                return Ok(false);
            }
        }
        let back = self.run_merge(
            &["read-tree", "-m", "-u", new, old_head],
            deadline,
            false,
            &[],
        )?;
        if back.code != 0 {
            return Ok(false);
        }
        // The refresh makes the stat data of a touched file match, so `diff-files` judges content.
        self.run_merge(&["update-index", "-q", "--refresh"], deadline, false, &[])?;
        for chunk in &chunks {
            let specs: Vec<&str> = chunk.specs.iter().map(String::as_str).collect();
            let mut cached = vec!["diff-index", "--cached", "--quiet", old_head, "--"];
            cached.extend(&specs);
            let mut files = vec!["diff-files", "--quiet", "--"];
            files.extend(&specs);
            for args in [cached, files] {
                if self.run_merge(&args, deadline, true, &[])?.code != 0 {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
}

#[cfg(test)]
mod merge_commit_tests {
    use super::*;
    use FailStep::{ReadTree, Refresh, UpdateRef};

    fn fail(reason: &'static str, after: After) -> FailDecision {
        FailDecision::Fail { reason, after }
    }

    #[test]
    fn f1_a_killed_refresh_that_left_the_lock_is_index_lock_left() {
        let got = classify_failure(Refresh, false, true, true, "", RefAfter::Unknown);
        assert_eq!(got, fail("index_lock_left", After::NotRestored));
    }

    #[test]
    fn f2_any_other_refresh_failure_is_restored() {
        let killed = classify_failure(Refresh, true, true, true, "", RefAfter::Unknown);
        assert_eq!(killed, fail("timeout", After::Restored));
        let lock = classify_failure(Refresh, true, true, false, "index.lock", RefAfter::Unknown);
        assert_eq!(lock, fail("merge_failed", After::Restored));
        let plain = classify_failure(Refresh, false, false, true, "", RefAfter::Unknown);
        assert_eq!(plain, fail("timeout", After::Restored));
    }

    #[test]
    fn r1_a_lock_before_wins_over_the_killed_rows() {
        let got = classify_failure(ReadTree, true, true, true, "", RefAfter::OldHead);
        assert_eq!(got, fail("merge_failed", After::Restored));
    }

    #[test]
    fn r2_killed_with_a_lock_after_is_index_lock_left() {
        let got = classify_failure(ReadTree, false, true, true, "", RefAfter::OldHead);
        assert_eq!(got, fail("index_lock_left", After::NotRestored));
    }

    #[test]
    fn r2b_not_killed_with_a_lock_after_is_index_lock_left() {
        let got = classify_failure(ReadTree, false, true, false, "", RefAfter::OldHead);
        assert_eq!(got, fail("index_lock_left", After::NotRestored));
    }

    #[test]
    fn r3_killed_rolls_back_only_when_the_ref_is_old_head() {
        let old = classify_failure(ReadTree, false, false, true, "", RefAfter::OldHead);
        assert_eq!(old, fail("timeout", After::Rollback));
        for other in [RefAfter::New, RefAfter::Other, RefAfter::Unknown] {
            let got = classify_failure(ReadTree, false, false, true, "", other);
            assert_eq!(got, fail("timeout", After::NotRestored));
        }
    }

    #[test]
    fn r4_a_stderr_naming_the_lock_is_restored() {
        let text = "fatal: Unable to create '/x/.git/index.lock': File exists.";
        let got = classify_failure(ReadTree, false, false, false, text, RefAfter::OldHead);
        assert_eq!(got, fail("merge_failed", After::Restored));
    }

    #[test]
    fn r5_a_dirty_path_is_refused() {
        for text in [
            "error: Entry 'a' would be overwritten by merge. Cannot merge.",
            "error: Entry 'a' not uptodate. Cannot merge.",
        ] {
            let got = classify_failure(ReadTree, false, false, false, text, RefAfter::OldHead);
            assert_eq!(got, FailDecision::RefuseDirty);
        }
    }

    #[test]
    fn r6_anything_else_rolls_back_only_when_the_ref_is_old_head() {
        let old = classify_failure(ReadTree, false, false, false, "boom", RefAfter::OldHead);
        assert_eq!(old, fail("merge_failed", After::Rollback));
        for other in [RefAfter::New, RefAfter::Other, RefAfter::Unknown] {
            let got = classify_failure(ReadTree, false, false, false, "boom", other);
            assert_eq!(got, fail("merge_failed", After::NotRestored));
        }
    }

    #[test]
    fn u1_a_moved_ref_is_merged_killed_or_not() {
        for killed in [false, true] {
            let got = classify_failure(UpdateRef, false, false, killed, "", RefAfter::New);
            assert_eq!(got, FailDecision::Merged);
        }
    }

    #[test]
    fn u2_an_unmoved_ref_rolls_back() {
        let plain = classify_failure(UpdateRef, false, false, false, "", RefAfter::OldHead);
        assert_eq!(plain, fail("merge_failed", After::Rollback));
        let killed = classify_failure(UpdateRef, false, false, true, "", RefAfter::OldHead);
        assert_eq!(killed, fail("timeout", After::Rollback));
    }

    #[test]
    fn u3_another_or_unknown_ref_is_not_restored() {
        for other in [RefAfter::Other, RefAfter::Unknown] {
            let plain = classify_failure(UpdateRef, false, false, false, "", other);
            assert_eq!(plain, fail("merge_failed", After::NotRestored));
            let killed = classify_failure(UpdateRef, false, false, true, "", other);
            assert_eq!(killed, fail("timeout", After::NotRestored));
        }
    }

    #[test]
    fn the_failure_reasons_equal_the_documented_ones() {
        let mut ours: Vec<&str> = MERGE_FAILURE_REASONS.to_vec();
        let mut documented: Vec<&str> = capstan_config::remote::DOC_FAILURE_REASONS.to_vec();
        ours.sort_unstable();
        documented.sort_unstable();
        assert_eq!(ours, documented);
    }

    #[test]
    fn read_tree_entries_are_taken_from_its_lines() {
        let text = "error: Entry 'a b.txt' would be overwritten by merge. Cannot merge.
                    error: Entry 'dir/c' not uptodate. Cannot merge.
error: other
";
        assert_eq!(
            read_tree_entries(text),
            vec![b"a b.txt".to_vec(), b"dir/c".to_vec()]
        );
    }

    #[test]
    fn failed_from_keeps_the_reason_and_the_restored_flag() {
        assert_eq!(
            failed_from(fail("timeout", After::Restored)),
            RootMerge::Failed {
                reason: "timeout",
                restored: true
            }
        );
        assert_eq!(
            failed_from(fail("index_lock_left", After::NotRestored)),
            RootMerge::Failed {
                reason: "index_lock_left",
                restored: false
            }
        );
    }
}

#[cfg(test)]
mod status_parse_tests {
    use super::parse_status_z;

    #[test]
    fn a_rename_record_loses_its_original_path_field() {
        let out = b"R  new name\0old\0 M plain\0C  copy\0src\0";
        assert_eq!(
            parse_status_z(out),
            vec![b"new name".to_vec(), b"plain".to_vec(), b"copy".to_vec()]
        );
    }

    #[test]
    fn paths_with_a_space_or_a_newline_stay_whole() {
        let out = b"M  a b\0 M x\ny\0";
        assert_eq!(parse_status_z(out), vec![b"a b".to_vec(), b"x\ny".to_vec()]);
    }

    #[test]
    fn empty_output_has_no_records() {
        assert!(parse_status_z(b"").is_empty());
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

    // ------------------------------------------------------------------------------ merge helpers

    use std::process::Command as Cmd;

    fn code_of(error: IntegrateError) -> String {
        match error {
            IntegrateError::Integration { code, .. } => code,
            other => panic!("expected an integration error, got {other:?}"),
        }
    }

    /// A fake git that logs `CALL <args>` and the environment of each call to the returned log, answers
    /// `--version` with `version`, sleeps 12 s for merge-tree and otherwise runs `other`.
    fn fake_git(dir: &Path, version: &str, other: &str) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let log = dir.join("fake.log");
        let script = dir.join("fake-git");
        let body = format!(
            "#!/bin/sh\necho \"CALL $*\" >> '{log}'\nenv | sort | sed 's/^/ENV /' >> '{log}'\n\
             case \" $* \" in\n  *\" --version \"*) echo '{version}'; exit 0;;\n  *\" merge-tree \"*) sleep 12; exit 0;;\nesac\n{other}\n",
            log = log.display()
        );
        std::fs::write(&script, body).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        (script, log)
    }

    /// A repository with the seams set directly on the field.
    fn seamed(dir: &Path, seams: GitSeams) -> GitRepo {
        let mut repo = GitRepo::new(dir);
        repo.seams = seams;
        repo
    }

    fn fake_repo(dir: &Path, program: PathBuf, skip: bool) -> GitRepo {
        seamed(
            dir,
            GitSeams {
                git_program: Some(program),
                skip_version_probe: skip,
                ..GitSeams::default()
            },
        )
    }

    fn deadline() -> Deadline {
        Deadline::new(Duration::from_secs(20))
    }

    fn rev_parse() -> [&'static str; 2] {
        ["rev-parse", "--git-dir"]
    }

    fn root_call(args: &[&str], network: bool) -> RootCall {
        RootCall {
            args: args.iter().map(|arg| arg.to_string()).collect(),
            timeout: Duration::from_secs(10),
            config_env: Vec::new(),
            network,
            user_config: network,
        }
    }

    fn log_of(log: &Path) -> String {
        std::fs::read_to_string(log).unwrap_or_default()
    }

    #[test]
    fn a_merge_child_carries_the_pins_and_only_the_named_environment() {
        let tmp = tempfile::tempdir().unwrap();
        let (program, log) = fake_git(tmp.path(), "git version 2.39.5", "echo .git");
        let repo =
            fake_repo(tmp.path(), program, true).with_env(vec![("GIT_DIR".into(), "/x".into())]);
        repo.run_merge(&rev_parse(), &deadline(), true, &[])
            .unwrap();
        let text = log_of(&log);
        for (key, value) in ROOT_CONFIG_OVERRIDES {
            assert!(text.contains(&format!("-c {key}={value}")), "{key}");
        }
        for line in [
            "ENV GIT_CONFIG_GLOBAL=/dev/null",
            "ENV GIT_CONFIG_NOSYSTEM=1",
            "ENV GIT_CONFIG_COUNT=1",
            "ENV GIT_CONFIG_KEY_0=i18n.commitEncoding",
            "ENV GIT_CONFIG_VALUE_0=UTF-8",
            "ENV GIT_OPTIONAL_LOCKS=0",
            "ENV LC_ALL=C",
        ] {
            assert!(text.lines().any(|l| l == line), "{line}");
        }
        assert!(text.contains("--no-replace-objects -C "));
        assert!(!text.contains("GIT_DIR"));

        std::fs::remove_file(&log).unwrap();
        repo.run_merge(
            &rev_parse(),
            &deadline(),
            false,
            &[("TZ", "UTC"), ("GIT_AUTHOR_NAME", "x")],
        )
        .unwrap();
        let text = log_of(&log);
        assert!(!text.contains("GIT_OPTIONAL_LOCKS"));
        assert!(text.lines().any(|l| l == "ENV TZ=UTC"));
        assert!(text.lines().any(|l| l == "ENV GIT_AUTHOR_NAME=x"));
    }

    #[test]
    fn a_bad_merge_environment_is_refused_before_any_spawn() {
        let tmp = tempfile::tempdir().unwrap();
        let (program, log) = fake_git(tmp.path(), "git version 2.39.5", "echo .git");
        let repo = fake_repo(tmp.path(), program, true);
        let d = deadline();
        let cases: Vec<(bool, Vec<(&str, &str)>)> = vec![
            (false, vec![("TZ", "UTC"), ("TZ", "UTC")]),
            (true, vec![("GIT_OPTIONAL_LOCKS", "1")]),
            (false, vec![("GIT_DIR", "/x")]),
            (false, vec![("TZ", "U\nTC")]),
        ];
        for (read_only, extra) in cases {
            let error = repo
                .run_merge(&rev_parse(), &d, read_only, &extra)
                .unwrap_err();
            assert_eq!(code_of(error), "git_unavailable");
        }
        let error = repo.run_merge(&["push"], &d, false, &[]).unwrap_err();
        assert_eq!(code_of(error), "git_unavailable");
        assert!(!log.exists());
    }

    #[test]
    fn only_a_merge_call_may_pass_the_exact_u_to_read_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let (program, log) = fake_git(tmp.path(), "git version 2.39.5", "exit 0");
        let repo = fake_repo(tmp.path(), program, true);
        repo.run_merge(
            &["read-tree", "-m", "-u", "a", "b"],
            &deadline(),
            false,
            &[],
        )
        .unwrap();
        assert!(log_of(&log).contains("read-tree -m -u a b"));
        std::fs::remove_file(&log).unwrap();
        assert!(repo
            .run_root(&root_call(&["read-tree", "-m", "-u", "a", "b"], false))
            .is_err());
        assert!(repo
            .run_root(&root_call(&["ls-remote", "-u", "x"], true))
            .is_err());
        assert!(repo
            .run_merge(&["read-tree", "-ux"], &deadline(), false, &[])
            .is_err());
        assert!(repo
            .run_merge(&["update-index", "-u"], &deadline(), false, &[])
            .is_err());
        assert!(!log.exists());
    }

    #[test]
    fn merge_call_sets_the_limit_by_subcommand() {
        for sub in ["merge-tree", "read-tree", "status", "ls-files"] {
            assert_eq!(merge_call(&[sub]).timeout, Duration::from_secs(120));
        }
        let call = merge_call(&["rev-parse", "--git-dir"]);
        assert_eq!(call.timeout, GIT_TIMEOUT);
        assert_eq!(
            call.config_env,
            vec![("i18n.commitEncoding".to_string(), "UTF-8".to_string())]
        );
        assert!(!call.user_config && !call.network);
    }

    #[test]
    fn run_merge_gives_a_tree_walk_more_than_the_ordinary_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let (program, _) = fake_git(tmp.path(), "git version 2.39.5", "exit 0");
        let repo = fake_repo(tmp.path(), program, true);
        let started = Instant::now();
        let out = repo
            .run_merge(&["merge-tree", "a", "b"], &deadline(), true, &[])
            .unwrap();
        assert_eq!(out.code, 0);
        assert!(started.elapsed() > GIT_TIMEOUT);
    }

    #[test]
    fn git_too_old_follows_the_seams_and_the_probe() {
        let tmp = tempfile::tempdir().unwrap();
        let (program, log) = fake_git(tmp.path(), "git version 2.39.5", "echo .git");
        let d = deadline();
        let seams = |force, skip| GitSeams {
            git_program: Some(program.clone()),
            force_old_git: force,
            skip_version_probe: skip,
            ..GitSeams::default()
        };
        let repo = seamed(tmp.path(), seams(true, true));
        assert!(repo.git_too_old(&d).unwrap());
        let repo = seamed(tmp.path(), seams(false, true));
        assert!(!repo.git_too_old(&d).unwrap());
        assert!(!log.exists());
        // Cached by a run_root call: no further --version.
        let repo = seamed(tmp.path(), seams(false, false));
        repo.run_root(&root_call(&rev_parse(), false)).unwrap();
        let versions = |log: &Path| log_of(log).matches("CALL --version").count();
        assert_eq!(versions(&log), 1);
        assert!(!repo.git_too_old(&d).unwrap());
        assert_eq!(versions(&log), 1);
        // A fresh repository probes: 2.39.5 is new enough.
        let repo = seamed(tmp.path(), seams(false, false));
        assert!(!repo.git_too_old(&d).unwrap());
        // A zero deadline is the timeout.
        let repo = seamed(tmp.path(), seams(false, false));
        let error = repo
            .git_too_old(&Deadline::new(Duration::ZERO))
            .unwrap_err();
        assert_eq!(code_of(error), "timeout");
    }

    #[test]
    fn a_too_old_git_is_never_cached_as_good() {
        let tmp = tempfile::tempdir().unwrap();
        let (program, _) = fake_git(tmp.path(), "git version 2.38.1", "echo .git");
        let repo = fake_repo(tmp.path(), program, false);
        assert!(repo.git_too_old(&deadline()).unwrap());
        let error = repo.run_root(&root_call(&rev_parse(), false)).unwrap_err();
        assert_eq!(code_of(error), "git_unavailable");
        assert!(repo.git_too_old(&deadline()).unwrap());
    }

    #[test]
    fn an_unreadable_version_is_git_unavailable() {
        let tmp = tempfile::tempdir().unwrap();
        let (program, _) = fake_git(tmp.path(), "x", "exit 1");
        let repo = fake_repo(tmp.path(), program, false);
        // `--version` prints junk with exit 0 here; a failing probe is covered by the exit-1 script below.
        assert_eq!(
            code_of(repo.git_too_old(&deadline()).unwrap_err()),
            "git_unavailable"
        );
        let failing = tmp.path().join("failing-git");
        std::fs::write(&failing, "#!/bin/sh\nexit 1\n").unwrap();
        Cmd::new("chmod").arg("755").arg(&failing).status().unwrap();
        let repo = fake_repo(tmp.path(), failing, false);
        assert_eq!(
            code_of(repo.git_too_old(&deadline()).unwrap_err()),
            "git_unavailable"
        );
    }

    /// Real git for the identity tests; older than 2.39 fails unless `CAPSTAN_ALLOW_OLD_GIT=1`, which skips them.
    fn real_git_is_new_enough() -> bool {
        let out = Cmd::new("git").arg("--version").output().expect("git runs");
        let version = parse_git_version(&utf8(&out.stdout));
        if version.is_some_and(|version| version >= MIN_GIT) {
            return true;
        }
        assert!(
            std::env::var("CAPSTAN_ALLOW_OLD_GIT").is_ok_and(|value| value == "1"),
            "git is older than 2.39; set CAPSTAN_ALLOW_OLD_GIT=1 to skip these tests"
        );
        false
    }

    struct RealFx {
        tmp: tempfile::TempDir,
        root: PathBuf,
        global: PathBuf,
    }

    fn real_fx() -> RealFx {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let fx = RealFx {
            global: home.join("gitconfig"),
            tmp,
            root,
        };
        fx.git(&["init", "-q"]);
        fx
    }

    impl RealFx {
        fn git(&self, args: &[&str]) {
            let out = Cmd::new("git")
                .arg("-C")
                .arg(&self.root)
                .args(args)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {}", utf8(&out.stderr));
        }
        fn global(&self, args: &[&str]) {
            let out = Cmd::new("git")
                .args(["config", "--file"])
                .arg(&self.global)
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "{}", utf8(&out.stderr));
        }
        fn repo(&self) -> GitRepo {
            let home = self.tmp.path().join("home").to_string_lossy().into_owned();
            seamed(
                &self.root,
                GitSeams {
                    forward_env: Some(vec![
                        ("HOME".into(), home),
                        (
                            "GIT_CONFIG_GLOBAL".into(),
                            self.global.to_string_lossy().into_owned(),
                        ),
                        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
                    ]),
                    ..GitSeams::default()
                },
            )
        }
        fn value(&self, key: &str) -> Option<String> {
            self.repo().identity_value(key, &deadline()).unwrap()
        }
    }

    #[test]
    fn identity_is_read_from_the_scopes_git_commit_reads() {
        if !real_git_is_new_enough() {
            return;
        }
        let fx = real_fx();
        assert_eq!(fx.value("user.name"), None);
        fx.global(&["user.name", "Global Name"]);
        fx.global(&["user.email", "g@example.com"]);
        assert_eq!(fx.value("user.name").as_deref(), Some("Global Name"));
        assert_eq!(
            fx.repo().strict_identity(&deadline()).unwrap(),
            Some(("Global Name".into(), "g@example.com".into()))
        );
        fx.git(&["config", "extensions.worktreeConfig", "true"]);
        fx.git(&["config", "--worktree", "user.name", "Tree Name"]);
        assert_eq!(fx.value("user.name").as_deref(), Some("Tree Name"));
    }

    #[test]
    fn a_value_git_cannot_use_as_an_ident_is_no_identity() {
        if !real_git_is_new_enough() {
            return;
        }
        let fx = real_fx();
        for bad in ["A <b", "a>b", "A\nB", ".", " ,; ", ""] {
            fx.global(&["--replace-all", "user.name", bad]);
            assert_eq!(fx.value("user.name"), None, "{bad:?}");
        }
        fx.global(&["--replace-all", "user.name", "A"]);
        fx.global(&["--add", "user.name", ""]);
        assert_eq!(fx.value("user.name"), None, "the last value wins");
        fx.global(&["--add", "user.name", "B"]);
        assert_eq!(fx.value("user.name").as_deref(), Some("B"));
        assert_eq!(fx.repo().strict_identity(&deadline()).unwrap(), None);
    }

    #[test]
    fn identity_reads_fail_loudly() {
        let tmp = tempfile::tempdir().unwrap();
        let (program, _) = fake_git(
            tmp.path(),
            "git version 2.39.5",
            "head -c 5000 /dev/zero | tr '\\0' a",
        );
        let repo = fake_repo(tmp.path(), program, true);
        let error = repo.identity_value("user.name", &deadline()).unwrap_err();
        assert_eq!(code_of(error), "output_too_large");
        let (program, _) = fake_git(tmp.path(), "git version 2.39.5", "exit 2");
        let repo = fake_repo(tmp.path(), program, true);
        let error = repo.identity_value("user.name", &deadline()).unwrap_err();
        assert_eq!(code_of(error), "git_unavailable");
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

    #[test]
    fn parse_merge_tree_reads_the_tree_and_the_paths() {
        let tree = "0123456789abcdef0123456789abcdef01234567";
        let mut out = format!("{tree}\0a b\0n\nl\0").into_bytes();
        let want = (tree.to_string(), vec![b"a b".to_vec(), b"n\nl".to_vec()]);
        assert_eq!(parse_merge_tree(&out), Some(want.clone()));
        out.pop();
        assert_eq!(parse_merge_tree(&out), Some(want.clone()));
        out.extend_from_slice(b"\0\0");
        assert_eq!(parse_merge_tree(&out), Some(want));
        assert_eq!(parse_merge_tree(format!("{tree}\0").as_bytes()), None);
        assert_eq!(parse_merge_tree(tree.as_bytes()), None);
        assert_eq!(parse_merge_tree(b"\0a\0"), None);
        assert_eq!(parse_merge_tree(b"nothex\0a\0"), None);
        assert_eq!(parse_merge_tree(b""), None);
    }

    #[test]
    fn a_commit_id_is_forty_lowercase_hex_digits() {
        assert!(is_commit_id(&"a1".repeat(20)));
        assert!(!is_commit_id(&"A1".repeat(20)));
        assert!(!is_commit_id(&"a".repeat(39)));
        assert!(!is_commit_id(&"g".repeat(40)));
    }
}

#[cfg(test)]
mod ignored_check_tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command as Cmd;

    fn paths(items: &[&str]) -> Vec<Vec<u8>> {
        items.iter().map(|item| item.as_bytes().to_vec()).collect()
    }

    fn specs_of(chunks: &[PathspecChunk]) -> Vec<&str> {
        chunks
            .iter()
            .flat_map(|chunk| chunk.specs.iter().map(String::as_str))
            .collect()
    }

    #[test]
    fn every_entry_is_a_literal_pathspec_of_a_path_or_parent() {
        let chunks = ignored_check_pathspecs(&paths(&["a/b.rs", "c"]), &paths(&["d"]), false);
        assert_eq!(
            specs_of(&chunks),
            vec![":(literal)a/b.rs", ":(literal)c", ":(literal)d"]
        );
        assert!(chunks.iter().all(|chunk| !chunk.fallback));
        let chunks = ignored_check_pathspecs(&paths(&["a/b.rs"]), &paths(&["D"]), true);
        assert_eq!(
            specs_of(&chunks),
            vec![":(icase,literal)D", ":(icase,literal)a/b.rs"]
        );
    }

    #[test]
    fn many_paths_make_chunks_of_at_most_1000() {
        let names: Vec<Vec<u8>> = (0..2500).map(|n| format!("f{n:05}").into_bytes()).collect();
        let chunks = ignored_check_pathspecs(&names, &[], false);
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|chunk| chunk.specs.len() <= 1000));
        assert_eq!(
            chunks.iter().map(|chunk| chunk.specs.len()).sum::<usize>(),
            2500
        );
    }

    #[test]
    fn long_paths_make_chunks_of_at_most_64_kib() {
        let names: Vec<Vec<u8>> = (0..1000)
            .map(|n| format!("{n:04}{}", "x".repeat(3996)).into_bytes())
            .collect();
        let chunks = ignored_check_pathspecs(&names, &[], false);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            let bytes: usize = chunk.specs.iter().map(String::len).sum();
            assert!(bytes <= 64 * 1024, "{bytes}");
        }
        assert_eq!(
            chunks.iter().map(|chunk| chunk.specs.len()).sum::<usize>(),
            1000
        );
    }

    #[test]
    fn empty_inputs_make_no_chunk() {
        assert!(ignored_check_pathspecs(&[], &[], false).is_empty());
        assert!(ignored_check_pathspecs(&[], &[], true).is_empty());
    }

    #[test]
    fn a_newline_gives_the_parent_and_fallback() {
        let chunks = ignored_check_pathspecs(&paths(&["dir/sub/a\nb"]), &[], false);
        assert_eq!(
            chunks,
            vec![PathspecChunk {
                specs: vec![":(literal)dir/sub".to_string()],
                fallback: true
            }]
        );
        let chunks = ignored_check_pathspecs(&paths(&["Dir/a\nb"]), &[], true);
        assert_eq!(specs_of(&chunks), vec![":(icase,literal)Dir"]);
        assert!(chunks[0].fallback);
    }

    #[test]
    fn a_top_level_non_utf8_path_gives_the_plain_dot() {
        let chunks = ignored_check_pathspecs(&[b"caf\xe9".to_vec()], &[], true);
        assert_eq!(
            chunks,
            vec![PathspecChunk {
                specs: vec![".".to_string()],
                fallback: true
            }]
        );
        let chunks =
            ignored_check_pathspecs(&[b"ok/caf\xe9".to_vec(), b"fine".to_vec()], &[], false);
        assert_eq!(chunks.len(), 2);
        assert!(!chunks[0].fallback && chunks[1].fallback);
        assert_eq!(chunks[1].specs, vec![":(literal)ok".to_string()]);
    }

    // ------------------------------------------------------------------ fake git

    fn fake(dir: &Path, diff: &str, config: &str, listing: &str) -> GitRepo {
        use std::os::unix::fs::PermissionsExt;
        let script = dir.join("fake-git-ignored");
        let body = format!(
            "#!/bin/sh\ncase \" $* \" in\n  *\" --version \"*) echo 'git version 2.39.5';;\n  \
             *\" config \"*) {config};;\n  *\" diff-tree \"*) {diff};;\n  *\" ls-files \"*) {listing};;\n  *) exit 0;;\nesac\n"
        );
        std::fs::write(&script, body).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut repo = GitRepo::new(dir);
        repo.seams = GitSeams {
            git_program: Some(script),
            skip_version_probe: true,
            ..GitSeams::default()
        };
        repo
    }

    const OK_CONFIG: &str = "echo false";
    const HUGE: &str = "head -c 17825792 /dev/zero | tr '\\0' 'a'";

    fn deadline() -> Deadline {
        Deadline::new(Duration::from_secs(60))
    }

    fn code_of(error: IntegrateError) -> String {
        match error {
            IntegrateError::Integration { code, .. } => code,
            other => panic!("expected an integration error, got {other:?}"),
        }
    }

    #[test]
    fn a_listing_over_the_cap_is_dirty_tree_for_exact_pathspecs() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = fake(tmp.path(), "printf 'a.txt\\0'", OK_CONFIG, HUGE);
        let refusal = repo
            .ignored_refusal("old", "new", &deadline())
            .unwrap()
            .expect("refused");
        assert_eq!(refusal.code(), "dirty_tree");
        assert_eq!(refusal.files(), [ENTRY_WOULD_OVERWRITE]);
    }

    #[test]
    fn a_listing_over_the_cap_for_a_fallback_pathspec_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = fake(tmp.path(), "printf 'caf\\351\\0'", OK_CONFIG, HUGE);
        let error = repo
            .ignored_refusal("old", "new", &deadline())
            .expect_err("too large");
        assert_eq!(code_of(error), "output_too_large");
    }

    #[test]
    fn a_failing_config_or_diff_tree_is_git_unavailable() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = fake(tmp.path(), "exit 0", "exit 2", "exit 0");
        let error = repo
            .ignored_refusal("o", "n", &deadline())
            .expect_err("config");
        assert_eq!(code_of(error), "git_unavailable");
        let repo = fake(tmp.path(), "exit 128", OK_CONFIG, "exit 0");
        let error = repo
            .ignored_refusal("o", "n", &deadline())
            .expect_err("diff");
        assert_eq!(code_of(error), "git_unavailable");
    }

    // ------------------------------------------------------------------ real git

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Cmd::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn git_repo_run_never_starts_a_program_named_by_the_repository_config() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["config", "--local", "user.name", "T"]);
        git(&root, &["config", "--local", "user.email", "t@t"]);
        std::fs::write(root.join("a.txt"), "base\n").unwrap();
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "base"]);
        let head = git(&root, &["rev-parse", "HEAD"]);
        let ran = tmp.path().join("ran");
        let script = tmp.path().join("sentinel.sh");
        std::fs::write(
            &script,
            format!("#!/bin/sh\necho ran >> '{}'\nexit 0\n", ran.display()),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let hooks = tmp.path().join("hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        for hook in ["reference-transaction", "post-checkout"] {
            std::fs::copy(&script, hooks.join(hook)).unwrap();
        }
        git(
            &root,
            &[
                "config",
                "--local",
                "core.fsmonitor",
                &script.to_string_lossy(),
            ],
        );
        git(
            &root,
            &[
                "config",
                "--local",
                "core.hooksPath",
                &hooks.to_string_lossy(),
            ],
        );
        let repo = GitRepo::new(&root);
        repo.run(&["status", "--porcelain"], PLAIN).unwrap();
        repo.run(&["diff", "--quiet"], PLAIN).unwrap();
        repo.run(&["diff-tree", "-r", &head, &head], BIG).unwrap();
        repo.run(&["update-ref", "refs/heads/sentinel", &head], WRITE)
            .unwrap();
        repo.content_in_head(&head).unwrap();
        repo.delete_branch_at("sentinel", &head).unwrap();
        assert!(!ran.exists(), "a repository-configured program ran");
    }

    #[test]
    fn a_top_level_non_utf8_changed_path_with_an_ignored_file_is_dirty_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["config", "--local", "user.name", "T"]);
        git(&root, &["config", "--local", "user.email", "t@t"]);
        std::fs::write(root.join("a.txt"), "base\n").unwrap();
        std::fs::write(root.join(".gitignore"), "*.o\n").unwrap();
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "base"]);
        git(&root, &["checkout", "-q", "-b", "topic"]);
        let name = std::ffi::OsStr::from_bytes(b"caf\xe9.o");
        std::fs::write(root.join(name), "side\n").unwrap();
        git(&root, &["add", "-f", "-A"]);
        git(&root, &["commit", "-q", "-m", "topic"]);
        let head = git(&root, &["rev-parse", "HEAD"]);
        git(&root, &["checkout", "-q", "main"]);
        std::fs::write(root.join(name), "local\n").unwrap();
        let mut repo = GitRepo::new(&root);
        repo.seams = GitSeams {
            forward_env: Some(vec![
                ("HOME".into(), home.to_string_lossy().into_owned()),
                ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ]),
            ..GitSeams::default()
        };
        match repo
            .root_merge_preflight("main", &head, Duration::from_secs(60))
            .unwrap()
        {
            Preflight::Refused(refusal) => {
                assert_eq!(refusal.code(), "dirty_tree");
                assert_eq!(refusal.files().len(), 1);
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }
}
