//! Name, label, shell-quoting and safe-text checks the adapter applies before anything reaches Herdr
//! (src/herdr/adapter-validate.ts). Patterns that use `\s`, `.` or `\d` in Node are written out by hand or with ASCII
//! classes, because those mean something different in the `regex` crate.

use crate::api::{AdapterError, AdapterResult};
use crate::naming::is_js_space;
use regex::Regex;
use std::sync::LazyLock;

pub use crate::api::{is_agent_name, shell_quote, MAX_TEXT_BYTES};

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("the pattern is valid")
}

pub static BRANCH_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^[A-Za-z0-9][A-Za-z0-9._/-]{0,99}$"));
pub static WORKSPACE_PATTERN: LazyLock<Regex> = LazyLock::new(|| regex(r"^w[0-9A-Za-z]+$"));
pub static PANE_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^w[0-9A-Za-z]+:p[0-9A-Za-z]+$"));
pub static TAB_PATTERN: LazyLock<Regex> = LazyLock::new(|| regex(r"^w[0-9A-Za-z]+:t[0-9A-Za-z]+$"));
pub static SIMPLE_VALUE: LazyLock<Regex> = LazyLock::new(|| regex(r"^[A-Za-z0-9_@%+=:,./-]*$"));
pub static TOKEN_NAME: LazyLock<Regex> = LazyLock::new(|| regex(r"^[a-z][a-z0-9_]{0,31}$"));
pub static ENVIRONMENT_KEY: LazyLock<Regex> = LazyLock::new(|| regex(r"^[A-Z_][A-Z0-9_]*$"));
/// `NAME_PATTERN`; `is_agent_name` is the same check without the regex.
pub fn is_name(value: &str) -> bool {
    is_agent_name(value)
}
/// The same characters the controller refuses in a message body.
pub static UNSAFE_TEXT: LazyLock<Regex> =
    LazyLock::new(|| regex(r"[\p{Cc}\p{Cf}\p{Zl}\p{Zp}\p{Noncharacter_Code_Point}]"));

pub fn js_trim(text: &str) -> &str {
    text.trim_matches(is_js_space)
}

pub fn js_trim_end(text: &str) -> &str {
    text.trim_end_matches(is_js_space)
}

/// `value.length`: UTF-16 code units.
pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

pub fn has_unsafe(text: &str) -> bool {
    UNSAFE_TEXT.is_match(text)
}

/// `CONTROL_CHARACTERS = /\p{Cc}/u`.
pub fn has_control(text: &str) -> bool {
    text.chars().any(char::is_control)
}

/// Whether a value HOME, PATH or TERM can be shell-quoted: not blank, nothing unsafe to show.
fn is_quotable(value: &str) -> bool {
    !js_trim(value).is_empty() && !has_unsafe(value)
}

/// In Claude Code a first character of / ! # ? or @ (or a tab) acts on the input box instead of adding text:
/// `/^(?:\t|\s*[/!#?@])/`.
pub fn command_start(text: &str) -> bool {
    if text.starts_with('\t') {
        return true;
    }
    matches!(
        text.trim_start_matches(is_js_space).chars().next(),
        Some('/' | '!' | '#' | '?' | '@')
    )
}

/// `isSafeText`: not blank, nothing unsafe except newlines, tabs and the two joiners.
pub fn is_safe_text(text: &str) -> bool {
    if js_trim(text).is_empty() {
        return false;
    }
    let kept: String = text
        .chars()
        .filter(|c| !matches!(c, '\n' | '\t' | '\u{200c}' | '\u{200d}'))
        .collect();
    !has_unsafe(&kept)
}

pub fn invalid(message: impl Into<String>) -> AdapterError {
    AdapterError::InvalidArgument(message.into())
}

pub fn require_match<'a>(value: &'a str, pattern: &Regex, label: &str) -> AdapterResult<&'a str> {
    if pattern.is_match(value) {
        Ok(value)
    } else {
        Err(invalid(format!("{label} is not acceptable")))
    }
}

/// A display label: printable text on one line, at most 64 characters.
pub fn require_label(value: &str) -> AdapterResult<&str> {
    if js_trim(value).is_empty()
        || utf16_len(value) > 128
        || value.chars().count() > 64
        || has_unsafe(value)
    {
        return Err(invalid("label is not acceptable"));
    }
    Ok(value)
}

pub fn require_quotable<'a>(value: Option<&'a str>, label: &str) -> AdapterResult<&'a str> {
    match value {
        Some(text) if is_quotable(text) => Ok(text),
        _ => Err(invalid(format!("{label} is not acceptable"))),
    }
}
