//! The `[remote]` section: whether a project's integrated work may be merged into, and pushed from, a remote branch. This
//! module holds the shape, the rules for a branch name and a push URL, and the fixed lists the documentation repeats;
//! nothing here runs git.
use capstan_wire::js::Value;

use crate::error::{invalid, Result};
use crate::json::{number, object};
use crate::keys::REMOTE_KEYS;
use crate::primitives::*;
use crate::value::{Item, Table};

/// The most bytes a `push_url` may hold.
pub const PUSH_URL_MAX_BYTES: usize = 1024;

/// The resolved `[remote]` section; an absent section is the default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    /// Whether integrated work may be merged into the remote branch.
    pub merge: bool,
    /// The branch to merge into; `None` is the remote's default branch at merge time.
    pub merge_branch: Option<String>,
    /// Whether the merged branch may be pushed.
    pub push: bool,
    /// Where a push goes.
    pub push_url: Option<String>,
    /// A hard cap on messages per project and per direction over the trailing 60 minutes.
    pub messages_per_hour: u32,
}

impl Default for Remote {
    fn default() -> Self {
        Remote {
            merge: false,
            merge_branch: None,
            push: false,
            push_url: None,
            messages_per_hour: 30,
        }
    }
}

impl Remote {
    pub(crate) fn to_value(&self) -> Value {
        let mut members = vec![
            ("merge", Value::Bool(self.merge)),
            ("mergeBranch", crate::json::optional(&self.merge_branch)),
            ("push", Value::Bool(self.push)),
            ("pushUrl", crate::json::optional(&self.push_url)),
        ];
        members.push(("messagesPerHour", number(i64::from(self.messages_per_hour))));
        object(members)
    }
}

/// Reads `[remote]`; every refusal names its key and none echoes a rejected value.
pub(crate) fn resolve_remote(table: &Table) -> Result<Remote> {
    reject_unknown_keys(table, REMOTE_KEYS, "remote")?;
    let merge = optional_boolean(table.get("merge"), "remote.merge", false)?;
    let push = optional_boolean(table.get("push"), "remote.push", false)?;
    let merge_branch = optional_text(table.get("merge_branch"), "remote.merge_branch")?;
    if let Some(branch) = merge_branch {
        if let Err(reason) = validate_branch_name(branch) {
            return invalid(format!("remote.merge_branch {reason}"));
        }
    }
    let push_url = optional_text(table.get("push_url"), "remote.push_url")?;
    if let Some(url) = push_url {
        if let Err(reason) = validate_push_url(url) {
            return invalid(format!("remote.push_url {reason}"));
        }
    }
    let messages_per_hour = optional_integer(
        table.get("messages_per_hour"),
        "remote.messages_per_hour",
        1,
        600,
        30,
    )?;
    if push && !merge {
        return invalid("remote.push requires remote.merge = true");
    }
    if push && push_url.is_none() {
        return invalid("remote.push requires remote.push_url");
    }
    Ok(Remote {
        merge,
        merge_branch: merge_branch.map(str::to_string),
        push,
        push_url: push_url.map(str::to_string),
        messages_per_hour: messages_per_hour as u32,
    })
}

fn optional_text<'a>(value: Option<&'a Item>, at: &str) -> Result<Option<&'a str>> {
    match value {
        None => Ok(None),
        Some(Item::Str(text)) => Ok(Some(text)),
        Some(_) => invalid(format!("{at} must be a string")),
    }
}

/// Git's branch-name rules, stricter or equal: what `validate_branch_name` refuses, `git check-ref-format --branch`
/// refuses too, except `HEAD`, a leading `-` and the empty name, which git also refuses as branches.
pub fn validate_branch_name(name: &str) -> std::result::Result<(), &'static str> {
    const REFUSED: &str = "is not a valid branch name";
    if name.is_empty() || name.starts_with('-') || name.starts_with('/') {
        return Err(REFUSED);
    }
    if name.contains("..") || name.contains("//") || name.contains("@{") {
        return Err(REFUSED);
    }
    if name
        .chars()
        .any(|c| c.is_control() || matches!(c, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\'))
    {
        return Err(REFUSED);
    }
    if name.ends_with('/') || name.ends_with('.') || name == "@" || name == "HEAD" {
        return Err(REFUSED);
    }
    if name
        .split('/')
        .any(|part| part.starts_with('.') || part.ends_with(".lock"))
    {
        return Err(REFUSED);
    }
    Ok(())
}

fn host_chars(host: &str) -> bool {
    host.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
}

fn user_chars(user: &str) -> bool {
    !user.is_empty()
        && user
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn scheme_name(host: &str) -> bool {
    ["ssh", "https", "http", "file", "git"]
        .iter()
        .any(|name| host.eq_ignore_ascii_case(name))
}

/// A push destination: `ssh://`, `https://` or `file://`, or the scp-like `[user@]host:path`. Refusals are fixed texts
/// that never repeat the value.
pub fn validate_push_url(url: &str) -> std::result::Result<(), &'static str> {
    const REFUSED: &str = "is not an accepted push URL";
    if url.len() > PUSH_URL_MAX_BYTES {
        return Err("is longer than 1024 bytes");
    }
    if url.is_empty() || url.starts_with('-') {
        return Err(REFUSED);
    }
    if url
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '?' | '#' | '%'))
    {
        return Err(REFUSED);
    }
    // A remote helper (`ext::`, `fd::`, `hg::`) runs a program.
    let helper_end = url
        .bytes()
        .position(|b| !(b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-')));
    if let Some(end) = helper_end {
        if end > 0 && url[end..].starts_with("::") {
            return Err(REFUSED);
        }
    }
    let Some(colon) = url.find(':') else {
        return Err(REFUSED);
    };
    let (before, after) = (&url[..colon], &url[colon + 1..]);
    if let Some(rest) = after.strip_prefix("//") {
        return validate_url_form(&before.to_ascii_lowercase(), rest);
    }
    // scp-like: [user@]host:path
    let host = match before.split_once('@') {
        Some((user, host)) => {
            if !user_chars(user) {
                return Err(REFUSED);
            }
            host
        }
        None => before,
    };
    if host.len() < 2 || !host_chars(host) || host.starts_with('-') || scheme_name(host) {
        return Err(REFUSED);
    }
    if after.is_empty() || after.contains('@') {
        return Err(REFUSED);
    }
    Ok(())
}

fn validate_url_form(scheme: &str, rest: &str) -> std::result::Result<(), &'static str> {
    const REFUSED: &str = "is not an accepted push URL";
    if !matches!(scheme, "ssh" | "https" | "file") {
        return Err(REFUSED);
    }
    let (authority, path) = match rest.find('/') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, ""),
    };
    if scheme == "file" {
        if path.is_empty() {
            return Err(REFUSED);
        }
        if authority.is_empty() {
            return Ok(());
        }
    }
    let host_port = match authority.rfind('@') {
        Some(at) => {
            if !user_chars(&authority[..at]) {
                // Covers a password (`user:pw@h`), an empty user and a second `@`.
                return Err(REFUSED);
            }
            &authority[at + 1..]
        }
        None => authority,
    };
    let (host, port) = if let Some(inner) = host_port.strip_prefix('[') {
        let Some(close) = inner.find(']') else {
            return Err(REFUSED);
        };
        let literal = &inner[..close];
        if literal.is_empty()
            || !literal.contains(':')
            || !literal.bytes().all(|b| b.is_ascii_hexdigit() || b == b':')
        {
            return Err(REFUSED);
        }
        let tail = &inner[close + 1..];
        match tail.strip_prefix(':') {
            Some(port) => (literal, Some(port)),
            None if tail.is_empty() => (literal, None),
            None => return Err(REFUSED),
        }
    } else {
        match host_port.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (host_port, None),
        }
    };
    let bracketed = host_port.starts_with('[');
    if host.is_empty() || host.starts_with('-') || (!bracketed && !host_chars(host)) {
        return Err(REFUSED);
    }
    if let Some(port) = port {
        if scheme == "file" {
            return Err(REFUSED);
        }
        let digits = !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit());
        if !digits || port.len() > 5 || !matches!(port.parse::<u32>(), Ok(1..=65535)) {
            return Err(REFUSED);
        }
    }
    Ok(())
}

/// The local git config keys (families) a root git call refuses to run under, one string per pattern.
pub const DOC_UNSAFE_FAMILIES: &[&str] = &[
    "merge.*.driver",
    "filter.*.clean",
    "filter.*.smudge",
    "filter.*.process",
    "diff.external",
    "diff.*.textconv",
    "diff.*.command",
    "core.sshCommand",
    "core.askPass",
    "core.gitProxy",
    "core.pager",
    "core.editor",
    "core.attributesFile",
    "core.worktree",
    "core.bare",
    "core.alternateRefsCommand",
    "sequence.editor",
    "submodule.*.update",
    "mergetool.*.cmd",
    "push.recurseSubmodules",
    "http.proxy",
    "http.proxyAuthMethod",
    "http.sslVerify",
    "http.sslCert",
    "http.sslKey",
    "http.sslCAInfo",
    "http.sslCAPath",
    "http.sslBackend",
    "http.pinnedPubkey",
    "http.followRedirects",
    "http.extraHeader",
    "http.cookieFile",
    "http.curloptResolve",
    "http.*.proxy",
    "http.*.proxyAuthMethod",
    "http.*.sslVerify",
    "http.*.sslCert",
    "http.*.sslKey",
    "http.*.sslCAInfo",
    "http.*.sslCAPath",
    "http.*.sslBackend",
    "http.*.pinnedPubkey",
    "http.*.followRedirects",
    "http.*.extraHeader",
    "http.*.cookieFile",
    "http.*.curloptResolve",
    "ssh.variant",
    "remote.*.proxy",
    "remote.*.proxyAuthMethod",
    "credential.*",
    "gpg.program",
    "gpg.*.program",
    "remote.*.receivepack",
    "remote.*.uploadpack",
    "remote.*.vcs",
    "url.*.insteadOf",
    "url.*.pushInsteadOf",
    "protocol.*.allow",
    "include.path",
    "includeIf.*",
];

/// The codes a merge request is refused with.
pub const DOC_REFUSAL_CODES: &[&str] = &[
    "unsafe_config",
    "invalid_branch",
    "invalid_commit",
    "wrong_branch",
    "no_identity",
    "dirty_tree",
    "merge_conflict",
    "git_too_old",
];

/// The reasons a merge that started then failed.
pub const DOC_FAILURE_REASONS: &[&str] = &[
    "merge_failed",
    "index_lock_left",
    "timeout",
    "tree_mismatch",
    "no_merge_head",
];

/// The reasons a push is not made or fails.
pub const DOC_PUSH_REASONS: &[&str] = &[
    "invalid_branch",
    "unsafe_config",
    "no_upstream",
    "mirror",
    "url_mismatch",
    "push_url_rewritten",
    "not_fast_forward",
    "rejected",
    "timeout",
    "unreachable",
];

/// One row of `tests/fixtures/branch_names.txt`: the class (`accept`, `git-refuses` or `config-refuses`) and the name.
/// In a name `\\` is a backslash, `\t` a tab and `\xNN` the ASCII character with that code; a line is split at its first
/// tab, and empty lines and lines starting with `#` are skipped. Panics on a malformed row: the fixture is fixed text.
#[doc(hidden)]
pub fn parse_branch_table(table: &str) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    for line in table.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (class, raw) = line.split_once('\t').expect("a row has a tab");
        assert!(
            matches!(class, "accept" | "git-refuses" | "config-refuses"),
            "unknown class {class}"
        );
        let mut name = String::new();
        let mut chars = raw.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                name.push(c);
                continue;
            }
            match chars.next() {
                Some('\\') => name.push('\\'),
                Some('t') => name.push('\t'),
                Some('x') => {
                    let hex: String = chars.by_ref().take(2).collect();
                    let code = u8::from_str_radix(&hex, 16).expect("two hex digits");
                    assert!(code < 0x80, "\\x escapes are ASCII");
                    name.push(char::from(code));
                }
                other => panic!("bad escape {other:?}"),
            }
        }
        rows.push((class.to_string(), name));
    }
    rows
}
