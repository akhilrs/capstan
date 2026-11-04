//! The `[ntfy]` section: push notifications to an ntfy server. This module holds the shape, the rules for every key,
//! the per-event defaults and the secret reader; it sends nothing. Syntax rules apply always; checks that look at the
//! host (the CA bundle, a token or password file) run only while `enabled = true`, so a committed capstan.toml with
//! `enabled = false` resolves on any host. A refusal names its key and never repeats a rejected value.
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use capstan_wire::js::Value;

use crate::error::{invalid, Result};
use crate::json::{number, object, strings, text};
use crate::keys::{NTFY_EVENT_KEYS, NTFY_KEYS};
use crate::primitives::*;
use crate::value::{Item, Table};

/// The longest `title_prefix`, and the longest prefix derived from a project name.
pub const TITLE_PREFIX_MAX_CHARS: usize = 40;
/// The longest secret (token or password) read from the environment or a file, in bytes.
pub const SECRET_MAX_BYTES: usize = 4096;
/// The most bytes a `server` or `click` URL may hold.
pub const URL_MAX_BYTES: usize = 512;
/// The prefix when neither `title_prefix` nor a project name gives one.
pub const FALLBACK_PREFIX: &str = "Capstan";

/// An event a notification can announce.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NtfyEvent {
    UserQuestion,
    OperatorProposal,
    PromptWaiting,
    DesignWaiting,
    PlanSignedOff,
    AgentBlocked,
    AgentStalled,
    AgentLost,
    VerifyFailed,
    IntegrationConflicted,
    /// The test notification; never selectable in a configuration.
    Test,
}

/// The selectable event names, in the order the documentation lists them.
pub const NTFY_EVENTS: &[&str] = &[
    "user_question",
    "operator_proposal",
    "prompt_waiting",
    "design_waiting",
    "plan_signed_off",
    "agent_blocked",
    "agent_stalled",
    "agent_lost",
    "verify_failed",
    "integration_conflicted",
];

impl NtfyEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            NtfyEvent::UserQuestion => "user_question",
            NtfyEvent::OperatorProposal => "operator_proposal",
            NtfyEvent::PromptWaiting => "prompt_waiting",
            NtfyEvent::DesignWaiting => "design_waiting",
            NtfyEvent::PlanSignedOff => "plan_signed_off",
            NtfyEvent::AgentBlocked => "agent_blocked",
            NtfyEvent::AgentStalled => "agent_stalled",
            NtfyEvent::AgentLost => "agent_lost",
            NtfyEvent::VerifyFailed => "verify_failed",
            NtfyEvent::IntegrationConflicted => "integration_conflicted",
            NtfyEvent::Test => "test",
        }
    }

    fn lookup(name: &str) -> Option<NtfyEvent> {
        const SELECTABLE: [NtfyEvent; 10] = [
            NtfyEvent::UserQuestion,
            NtfyEvent::OperatorProposal,
            NtfyEvent::PromptWaiting,
            NtfyEvent::DesignWaiting,
            NtfyEvent::PlanSignedOff,
            NtfyEvent::AgentBlocked,
            NtfyEvent::AgentStalled,
            NtfyEvent::AgentLost,
            NtfyEvent::VerifyFailed,
            NtfyEvent::IntegrationConflicted,
        ];
        if name == "test" {
            return Some(NtfyEvent::Test);
        }
        SELECTABLE.into_iter().find(|event| event.as_str() == name)
    }

    fn selectable() -> Vec<NtfyEvent> {
        NTFY_EVENTS
            .iter()
            .filter_map(|name| name.parse().ok())
            .collect()
    }
}

/// The event named `name`; `test` is known here but is not one of `NTFY_EVENTS`.
impl std::str::FromStr for NtfyEvent {
    type Err = ();

    fn from_str(name: &str) -> std::result::Result<NtfyEvent, ()> {
        NtfyEvent::lookup(name).ok_or(())
    }
}

/// How a notification for one event looks: priority 1 (min) to 5 (urgent), tags and an optional click URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NtfyEventStyle {
    pub priority: u8,
    pub tags: Vec<String>,
    pub click: Option<String>,
}

impl NtfyEventStyle {
    /// The style of `event` when no `[ntfy.event.<name>]` table sets it.
    pub fn default_for(event: NtfyEvent) -> NtfyEventStyle {
        let (priority, tag) = match event {
            NtfyEvent::UserQuestion | NtfyEvent::OperatorProposal | NtfyEvent::PromptWaiting => {
                (4, "question")
            }
            NtfyEvent::AgentBlocked | NtfyEvent::AgentLost | NtfyEvent::IntegrationConflicted => {
                (4, "warning")
            }
            NtfyEvent::DesignWaiting => (3, "art"),
            NtfyEvent::PlanSignedOff => (3, "white_check_mark"),
            NtfyEvent::AgentStalled => (3, "hourglass"),
            NtfyEvent::VerifyFailed => (3, "x"),
            NtfyEvent::Test => (2, "test"),
        };
        NtfyEventStyle {
            priority,
            tags: vec![tag.to_string()],
            click: None,
        }
    }
}

/// How the sender authenticates; the secret is named here, never held.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NtfyAuth {
    None,
    TokenEnv(String),
    TokenFile(PathBuf),
    BasicEnv {
        user: String,
        password_env: String,
    },
    BasicFile {
        user: String,
        password_file: PathBuf,
    },
}

/// The resolved `[ntfy]` section; an absent section is the default (disabled).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ntfy {
    pub enabled: bool,
    /// `scheme://host[:port][/path]` with trailing slashes trimmed; empty while unset.
    pub server: String,
    pub topic: String,
    pub auth: NtfyAuth,
    pub events: Vec<NtfyEvent>,
    pub timeout_seconds: u32,
    pub retries: u32,
    pub ca_file: Option<PathBuf>,
    pub title_prefix: Option<String>,
    /// The merged style of each event that has a table; other events use `NtfyEventStyle::default_for`.
    pub per_event: BTreeMap<NtfyEvent, NtfyEventStyle>,
}

impl Default for Ntfy {
    fn default() -> Self {
        Ntfy {
            enabled: false,
            server: String::new(),
            topic: String::new(),
            auth: NtfyAuth::None,
            events: NtfyEvent::selectable(),
            timeout_seconds: 10,
            retries: 2,
            ca_file: None,
            title_prefix: None,
            per_event: BTreeMap::new(),
        }
    }
}

impl Ntfy {
    /// The style a notification for `event` uses.
    pub fn style(&self, event: NtfyEvent) -> NtfyEventStyle {
        self.per_event
            .get(&event)
            .cloned()
            .unwrap_or_else(|| NtfyEventStyle::default_for(event))
    }

    pub(crate) fn to_value(&self) -> Value {
        let auth = match &self.auth {
            NtfyAuth::None => object(vec![("method", text("none"))]),
            NtfyAuth::TokenEnv(name) => {
                object(vec![("method", text("token_env")), ("env", text(name))])
            }
            NtfyAuth::TokenFile(path) => object(vec![
                ("method", text("token_file")),
                ("file", text(&path.to_string_lossy())),
            ]),
            NtfyAuth::BasicEnv { user, password_env } => object(vec![
                ("method", text("user_password_env")),
                ("user", text(user)),
                ("env", text(password_env)),
            ]),
            NtfyAuth::BasicFile {
                user,
                password_file,
            } => object(vec![
                ("method", text("user_password_file")),
                ("user", text(user)),
                ("file", text(&password_file.to_string_lossy())),
            ]),
        };
        let events: Vec<String> = self.events.iter().map(|e| e.as_str().to_string()).collect();
        let per_event = self
            .per_event
            .iter()
            .map(|(event, style)| {
                let mut members = vec![
                    ("priority", number(i64::from(style.priority))),
                    ("tags", strings(&style.tags)),
                ];
                if let Some(click) = &style.click {
                    members.push(("click", text(click)));
                }
                (event.as_str(), object(members))
            })
            .collect();
        let mut members = vec![
            ("enabled", Value::Bool(self.enabled)),
            ("server", text(&self.server)),
            ("topic", text(&self.topic)),
            ("auth", auth),
            ("events", strings(&events)),
            ("timeoutSeconds", number(i64::from(self.timeout_seconds))),
            ("retries", number(i64::from(self.retries))),
        ];
        if let Some(path) = &self.ca_file {
            members.push(("caFile", text(&path.to_string_lossy())));
        }
        if let Some(prefix) = &self.title_prefix {
            members.push(("titlePrefix", text(prefix)));
        }
        members.push(("event", object(per_event)));
        object(members)
    }
}

/// The title prefix a sender uses: `title_prefix` when set, else the project name with control characters removed and
/// cut to 40 characters, else `Capstan`.
pub fn effective_prefix(ntfy: &Ntfy, project_name: Option<&str>) -> String {
    if let Some(prefix) = &ntfy.title_prefix {
        return prefix.clone();
    }
    let cleaned: String = project_name
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .take(TITLE_PREFIX_MAX_CHARS)
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        FALLBACK_PREFIX.to_string()
    } else {
        cleaned.to_string()
    }
}

// --- secrets ---------------------------------------------------------------------------------------------------------

/// A token or password. It prints as `[redacted]`; `expose` is the only way to read it.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

/// Why a secret could not be read. The text names the variable or the path and never a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretError {
    MissingEnv(String),
    MissingFile(PathBuf),
    BadMode(PathBuf),
    NotOwner(PathBuf),
    Symlink(PathBuf),
    /// Empty after trimming, longer than 4096 bytes, or not UTF-8.
    Empty,
}

impl fmt::Display for SecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecretError::MissingEnv(name) => write!(f, "environment variable {name} is not set"),
            SecretError::MissingFile(path) => write!(f, "{} does not exist", path.display()),
            SecretError::BadMode(path) => write!(
                f,
                "{} must be a regular file with mode 0600",
                path.display()
            ),
            SecretError::NotOwner(path) => {
                write!(f, "{} must be owned by the current user", path.display())
            }
            SecretError::Symlink(path) => {
                write!(f, "{} must not be a symbolic link", path.display())
            }
            SecretError::Empty => write!(
                f,
                "the secret is empty, not text, or longer than {SECRET_MAX_BYTES} bytes"
            ),
        }
    }
}

impl std::error::Error for SecretError {}

/// What `check_secret_file` needs to know about a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretFileFacts {
    pub path: PathBuf,
    pub is_symlink: bool,
    pub is_regular: bool,
    pub uid: u32,
    /// The permission bits (`st_mode & 0o7777`).
    pub mode: u32,
}

/// Whether a secret file is acceptable: not a symlink, a regular file, owned by `my_uid`, mode 0600.
pub fn check_secret_file(
    facts: &SecretFileFacts,
    my_uid: u32,
) -> std::result::Result<(), SecretError> {
    if facts.is_symlink {
        return Err(SecretError::Symlink(facts.path.clone()));
    }
    if !facts.is_regular {
        return Err(SecretError::BadMode(facts.path.clone()));
    }
    if facts.uid != my_uid {
        return Err(SecretError::NotOwner(facts.path.clone()));
    }
    if facts.mode & 0o7777 != 0o600 {
        return Err(SecretError::BadMode(facts.path.clone()));
    }
    Ok(())
}

fn finish_secret(raw: &[u8]) -> std::result::Result<Secret, SecretError> {
    let text = std::str::from_utf8(raw)
        .map_err(|_| SecretError::Empty)?
        .trim();
    if text.is_empty() || text.len() > SECRET_MAX_BYTES {
        return Err(SecretError::Empty);
    }
    Ok(Secret(text.to_string()))
}

fn secret_from_env(
    name: &str,
    env: &dyn Fn(&str) -> Option<String>,
) -> std::result::Result<Secret, SecretError> {
    match env(name) {
        Some(value) => finish_secret(value.as_bytes()),
        None => Err(SecretError::MissingEnv(name.to_string())),
    }
}

fn secret_from_file(path: &Path) -> std::result::Result<Secret, SecretError> {
    let missing = |error: std::io::Error| match error.kind() {
        std::io::ErrorKind::NotFound => SecretError::MissingFile(path.to_path_buf()),
        _ => SecretError::BadMode(path.to_path_buf()),
    };
    let link = fs::symlink_metadata(path).map_err(missing)?;
    if link.file_type().is_symlink() {
        return Err(SecretError::Symlink(path.to_path_buf()));
    }
    if !link.file_type().is_file() {
        return Err(SecretError::BadMode(path.to_path_buf()));
    }
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| match error.raw_os_error() {
            Some(libc::ELOOP) => SecretError::Symlink(path.to_path_buf()),
            _ => missing(error),
        })?;
    let meta = file.metadata().map_err(missing)?;
    let facts = SecretFileFacts {
        path: path.to_path_buf(),
        is_symlink: false,
        is_regular: meta.file_type().is_file(),
        uid: meta.uid(),
        mode: meta.mode() & 0o7777,
    };
    // SAFETY: geteuid has no preconditions and cannot fail.
    check_secret_file(&facts, unsafe { libc::geteuid() })?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(SECRET_MAX_BYTES as u64 + 2)
        .read_to_end(&mut bytes)
        .map_err(missing)?;
    finish_secret(&bytes)
}

/// Reads the token or password `auth` names, anew on every call. `None` for an anonymous server.
pub fn read_secret(
    auth: &NtfyAuth,
    env: &dyn Fn(&str) -> Option<String>,
) -> std::result::Result<Option<Secret>, SecretError> {
    match auth {
        NtfyAuth::None => Ok(None),
        NtfyAuth::TokenEnv(name)
        | NtfyAuth::BasicEnv {
            password_env: name, ..
        } => secret_from_env(name, env).map(Some),
        NtfyAuth::TokenFile(path)
        | NtfyAuth::BasicFile {
            password_file: path,
            ..
        } => secret_from_file(path).map(Some),
    }
}

// --- parsing ---------------------------------------------------------------------------------------------------------

const SECRET_KEYS: &[&str] = &["token", "password", "secret"];

fn refuse_secret_keys(table: &Table, at: &str) -> Result<()> {
    for key in table.keys() {
        if SECRET_KEYS.contains(&key) {
            return invalid(format!(
                "{at}.{key} is not accepted: capstan.toml must not hold secrets; name an environment variable with [ntfy] token_env or a file with [ntfy] token_file"
            ));
        }
    }
    Ok(())
}

fn text_value<'a>(table: &'a Table, key: &str, at: &str, max: usize) -> Result<Option<&'a str>> {
    optional_string(table.get(key), &format!("{at}.{key}"), max)
}

fn topic_ok(text: &str) -> bool {
    (1..=64).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn user_ok(text: &str) -> bool {
    (1..=64).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'@' | b'-'))
}

fn tag_ok(text: &str) -> bool {
    (1..=32).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'+' | b'-'))
}

fn plain_url_chars(url: &str) -> bool {
    url.bytes()
        .all(|b| (0x21..=0x7e).contains(&b) && b != b'\\')
}

/// Validates a `server` URL and returns it with trailing slashes trimmed; the error is a fixed reason.
pub fn normalize_server(url: &str) -> std::result::Result<String, &'static str> {
    if url.len() > URL_MAX_BYTES || !plain_url_chars(url) {
        return Err("is not a valid URL");
    }
    let (secure, rest) = if let Some(rest) = url.strip_prefix("https://") {
        (true, rest)
    } else if let Some(rest) = url.strip_prefix("http://") {
        (false, rest)
    } else {
        return Err("must start with https:// (or http:// for localhost)");
    };
    if rest.contains(['?', '#']) {
        return Err("must not have a query or a fragment");
    }
    let (authority, path) = match rest.find('/') {
        Some(at) => rest.split_at(at),
        None => (rest, ""),
    };
    if authority.contains('@') {
        return Err("must not have user information");
    }
    let (host, port) = if authority.starts_with('[') {
        match authority.split_once(']') {
            Some(("[::1", "")) => ("[::1]", None),
            Some(("[::1", tail)) if tail.starts_with(':') => ("[::1]", Some(&tail[1..])),
            _ => return Err("accepts only [::1] as an IPv6 host"),
        }
    } else {
        match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    if host.is_empty() {
        return Err("has no valid host");
    }
    if host != "[::1]"
        && !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    {
        return Err("has no valid host");
    }
    if let Some(port) = port {
        let digits =
            !port.is_empty() && port.len() <= 5 && port.bytes().all(|b| b.is_ascii_digit());
        if !digits || !(1..=65535).contains(&port.parse::<u32>().unwrap_or(0)) {
            return Err("has an invalid port");
        }
    }
    if !secure && !matches!(host, "localhost" | "127.0.0.1" | "[::1]") {
        return Err("may use http:// only for localhost, 127.0.0.1 or [::1]");
    }
    let path = path.trim_end_matches('/');
    Ok(format!(
        "{}://{authority}{path}",
        if secure { "https" } else { "http" }
    ))
}

fn click_problem(url: &str) -> Option<&'static str> {
    if url.len() > URL_MAX_BYTES || !plain_url_chars(url) {
        return Some("is not a valid URL");
    }
    let Some(rest) = url.strip_prefix("https://") else {
        return Some("must start with https://");
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return Some("has no valid host or has user information");
    }
    None
}

fn absolute_path(table: &Table, key: &str, at: &str) -> Result<Option<PathBuf>> {
    let Some(text) = text_value(table, key, at, 4096)? else {
        return Ok(None);
    };
    if !Path::new(text).is_absolute() {
        return invalid(format!("{at}.{key} must be an absolute path"));
    }
    Ok(Some(PathBuf::from(text)))
}

fn env_name(table: &Table, key: &str, at: &str, env_pass: &[String]) -> Result<Option<String>> {
    let Some(name) = text_value(table, key, at, 64)? else {
        return Ok(None);
    };
    if !env_name_ok(name) {
        return invalid(format!(
            "{at}.{key} must be an upper-case variable name matching ^[A-Z_][A-Z0-9_]*$"
        ));
    }
    if env_pass.iter().any(|passed| passed == name) {
        return invalid(format!(
            "{at}.{key} names {name}, which is listed in [env] pass; a notification secret must not be passed to agents, so remove it from [env] pass"
        ));
    }
    Ok(Some(name.to_string()))
}

fn priority_value(item: &Item, at: &str) -> Result<u8> {
    const NAMES: &[(&str, u8)] = &[
        ("min", 1),
        ("low", 2),
        ("default", 3),
        ("high", 4),
        ("urgent", 5),
    ];
    match item {
        Item::Str(name) => match NAMES.iter().find(|(known, _)| known == name) {
            Some((_, n)) => Ok(*n),
            None => invalid(format!(
                "{at} must be one of min, low, default, high, urgent or an integer from 1 to 5"
            )),
        },
        Item::Int(n) if (1..=5).contains(n) => Ok(*n as u8),
        _ => invalid(format!(
            "{at} must be one of min, low, default, high, urgent or an integer from 1 to 5"
        )),
    }
}

fn resolve_event_style(event: NtfyEvent, table: &Table, at: &str) -> Result<NtfyEventStyle> {
    refuse_secret_keys(table, at)?;
    reject_unknown_keys(table, NTFY_EVENT_KEYS, at)?;
    let mut style = NtfyEventStyle::default_for(event);
    if let Some(item) = table.get("priority") {
        style.priority = priority_value(item, &format!("{at}.priority"))?;
    }
    if let Some(item) = table.get("tags") {
        let key = format!("{at}.tags");
        let Item::Array(entries) = item else {
            return invalid(format!("{key} must be an array of strings"));
        };
        if entries.len() > 5 {
            return invalid(format!("{key} exceeds 5 entries"));
        }
        let mut tags = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            match entry {
                Item::Str(tag) if tag_ok(tag) => tags.push(tag.clone()),
                _ => {
                    return invalid(format!(
                        "{key}[{index}] must match ^[A-Za-z0-9_+-]{{1,32}}$"
                    ))
                }
            }
        }
        style.tags = tags;
    }
    if let Some(click) = text_value(table, "click", at, URL_MAX_BYTES)? {
        if let Some(reason) = click_problem(click) {
            return invalid(format!("{at}.click {reason}"));
        }
        style.click = Some(click.to_string());
    }
    Ok(style)
}

fn int_in(table: &Table, key: &str, min: i64, max: i64, fallback: i64) -> Result<u32> {
    Ok(optional_integer(table.get(key), &format!("ntfy.{key}"), min, max, fallback)? as u32)
}

/// Reads `[ntfy]`. `env_pass` is the `[env] pass` list, which may not name a secret variable.
pub(crate) fn parse_ntfy(value: Option<&Item>, env_pass: &[String]) -> Result<Ntfy> {
    let table = optional_table(value, "ntfy")?;
    refuse_secret_keys(table, "ntfy")?;
    reject_unknown_keys(table, NTFY_KEYS, "ntfy")?;
    let enabled = optional_boolean(table.get("enabled"), "ntfy.enabled", false)?;

    let server = match text_value(table, "server", "ntfy", URL_MAX_BYTES)? {
        Some(url) => match normalize_server(url) {
            Ok(server) => server,
            Err(reason) => return invalid(format!("ntfy.server {reason}")),
        },
        None => String::new(),
    };
    let topic = match text_value(table, "topic", "ntfy", 64)? {
        Some(topic) if topic_ok(topic) => topic.to_string(),
        Some(_) => return invalid("ntfy.topic must match ^[A-Za-z0-9_-]{1,64}$"),
        None => String::new(),
    };
    if enabled && server.is_empty() {
        return invalid("ntfy.server is required when ntfy.enabled = true");
    }
    if enabled && topic.is_empty() {
        return invalid("ntfy.topic is required when ntfy.enabled = true");
    }

    let token_env = env_name(table, "token_env", "ntfy", env_pass)?;
    let token_file = absolute_path(table, "token_file", "ntfy")?;
    let password_env = env_name(table, "password_env", "ntfy", env_pass)?;
    let password_file = absolute_path(table, "password_file", "ntfy")?;
    let user = match text_value(table, "user", "ntfy", 64)? {
        Some(user) if user_ok(user) => Some(user.to_string()),
        Some(_) => return invalid("ntfy.user must match ^[A-Za-z0-9._@-]{1,64}$"),
        None => None,
    };
    let methods = usize::from(token_env.is_some())
        + usize::from(token_file.is_some())
        + usize::from(password_env.is_some() || password_file.is_some());
    if user.is_some() && (token_env.is_some() || token_file.is_some()) {
        return invalid(
            "ntfy.user cannot be combined with token_env or token_file: use one authentication method",
        );
    }
    if methods > 1 || (password_env.is_some() && password_file.is_some()) {
        return invalid(
            "ntfy accepts one authentication method: token_env, token_file, user with password_env, or user with password_file",
        );
    }
    let auth = match (token_env, token_file, password_env, password_file, user) {
        (Some(name), ..) => NtfyAuth::TokenEnv(name),
        (_, Some(path), ..) => NtfyAuth::TokenFile(path),
        (_, _, Some(password_env), _, Some(user)) => NtfyAuth::BasicEnv { user, password_env },
        (_, _, _, Some(password_file), Some(user)) => NtfyAuth::BasicFile {
            user,
            password_file,
        },
        (_, _, Some(_), _, None) | (_, _, _, Some(_), None) => {
            return invalid("ntfy.password_env and ntfy.password_file need ntfy.user")
        }
        (_, _, None, None, Some(_)) => {
            return invalid("ntfy.user needs ntfy.password_env or ntfy.password_file")
        }
        _ => NtfyAuth::None,
    };

    let events = match table.get("events") {
        None => NtfyEvent::selectable(),
        Some(Item::Array(entries)) => {
            let mut events = Vec::new();
            for (index, entry) in entries.iter().enumerate() {
                let event = match entry {
                    Item::Str(name) if name != "test" => name.parse::<NtfyEvent>().ok(),
                    _ => None,
                };
                let Some(event) = event else {
                    return invalid(format!(
                        "ntfy.events[{index}] must be one of {}",
                        NTFY_EVENTS.join(", ")
                    ));
                };
                if events.contains(&event) {
                    return invalid(format!("ntfy.events[{index}] repeats {}", event.as_str()));
                }
                events.push(event);
            }
            events
        }
        Some(_) => return invalid("ntfy.events must be an array of event names"),
    };
    let timeout_seconds = int_in(table, "timeout_seconds", 1, 60, 10)?;
    let retries = int_in(table, "retries", 0, 5, 2)?;
    let ca_file = absolute_path(table, "ca_file", "ntfy")?;
    let title_prefix = match table.get("title_prefix") {
        None => None,
        Some(Item::Str(prefix)) => {
            if prefix.is_empty() || prefix.chars().count() > TITLE_PREFIX_MAX_CHARS {
                return invalid("ntfy.title_prefix must be 1 to 40 characters");
            }
            if prefix.chars().any(char::is_control) {
                return invalid("ntfy.title_prefix must not contain control characters");
            }
            Some(prefix.clone())
        }
        Some(_) => return invalid("ntfy.title_prefix must be a string"),
    };

    let mut per_event = BTreeMap::new();
    if let Some(item) = table.get("event") {
        let tables = required_table(Some(item), "ntfy.event")?;
        for (name, item) in tables.entries() {
            let event = match name.parse::<NtfyEvent>() {
                Ok(event) if event != NtfyEvent::Test => event,
                _ => {
                    return invalid(format!(
                        "ntfy.event has an unknown event name; allowed: {}",
                        NTFY_EVENTS.join(", ")
                    ))
                }
            };
            let at = format!("ntfy.event.{name}");
            let style = resolve_event_style(event, required_table(Some(item), &at)?, &at)?;
            per_event.insert(event, style);
        }
    }

    if enabled {
        if let Some(path) = &ca_file {
            if !fs::metadata(path).map(|m| m.is_file()).unwrap_or(false) {
                return invalid("ntfy.ca_file does not name an existing file");
            }
        }
        let file = match &auth {
            NtfyAuth::TokenFile(path) => Some(("token_file", path)),
            NtfyAuth::BasicFile { password_file, .. } => Some(("password_file", password_file)),
            _ => None,
        };
        if let Some((key, path)) = file {
            if fs::symlink_metadata(path).is_err() {
                return invalid(format!("ntfy.{key} does not name an existing file"));
            }
        }
    }

    Ok(Ntfy {
        enabled,
        server,
        topic,
        auth,
        events,
        timeout_seconds,
        retries,
        ca_file,
        title_prefix,
        per_event,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_project_name_with_a_newline_gives_a_clean_prefix() {
        let name = format!("ab\ncd{}", "x".repeat(60));
        let prefix = effective_prefix(&Ntfy::default(), Some(&name));
        assert_eq!(prefix.chars().count(), 40);
        assert!(!prefix.chars().any(char::is_control));
        assert_eq!(effective_prefix(&Ntfy::default(), None), "Capstan");
        assert_eq!(effective_prefix(&Ntfy::default(), Some("\n")), "Capstan");
    }
}
