//! The small readers and validators every capstan.toml section uses (`src/config/primitives.ts`).
use crate::error::{invalid, Result};
use crate::text::{is_unsafe_char, len16, looks_like_credential, trim};
use crate::types::{
    CONFIG_FILE_NAME, MAX_PASSED_ENV_NAMES, RESERVED_ENV_NAMES, RESERVED_ENV_PREFIXES,
};
use crate::value::{Item, Table};

pub const MAX_FILE_BYTES: u64 = 64 * 1024;
pub const MAX_PROMPT_CHARS: usize = MAX_FILE_BYTES as usize;
pub const MAX_LIST_ENTRIES: usize = 64;
pub const MAX_DENY_ENTRIES: usize = 128;
pub const MAX_ENTRY_CHARS: usize = 200;
pub const NAME_PATTERN: &str = "^[a-z][a-z0-9-]{0,31}$";
pub const SESSION_PATTERN: &str = "^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$";
pub const MCP_SERVER_NAME_PATTERN: &str = "^[a-z][a-z0-9_-]{0,31}$";

static EMPTY: Table = Table::empty();

fn chars_in(
    text: &str,
    first: impl Fn(u8) -> bool,
    rest: impl Fn(u8) -> bool,
    max_rest: usize,
) -> bool {
    let bytes = text.as_bytes();
    match bytes.split_first() {
        Some((head, tail)) => {
            first(*head) && tail.len() <= max_rest && tail.iter().all(|b| rest(*b))
        }
        None => false,
    }
}

pub fn name_ok(text: &str) -> bool {
    chars_in(
        text,
        |b| b.is_ascii_lowercase(),
        |b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-',
        31,
    )
}

pub fn session_ok(text: &str) -> bool {
    chars_in(
        text,
        |b| b.is_ascii_alphanumeric(),
        |b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'),
        63,
    )
}

pub fn mcp_server_name_ok(text: &str) -> bool {
    chars_in(
        text,
        |b| b.is_ascii_lowercase(),
        |b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-',
        31,
    )
}

pub fn env_name_ok(text: &str) -> bool {
    chars_in(
        text,
        |b| b.is_ascii_uppercase() || b == b'_',
        |b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_',
        63,
    )
}

/// `/^[\x20-\x7e]*$/`.
pub fn printable_line(text: &str) -> bool {
    text.bytes().all(|b| (0x20..=0x7e).contains(&b))
}

/// `COMMAND_PATTERN` and the checks of `isExecutablePath`.
pub fn is_executable_path(command: &str) -> bool {
    let body = command.strip_prefix("./").unwrap_or(command);
    let shaped = chars_in(
        body,
        |b| b.is_ascii_alphanumeric() || b == b'_' || b == b'/',
        |b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'-'),
        199,
    );
    shaped && !command.ends_with('/') && !matches!(command.rsplit('/').next(), Some("." | ".."))
}

pub fn table_or_empty() -> &'static Table {
    &EMPTY
}

pub fn validated_names<'a>(table: &'a Table, at: &str, noun: &str) -> Result<Vec<&'a str>> {
    let names: Vec<&str> = table.keys().collect();
    for name in &names {
        if !name_ok(name) {
            return invalid(format!(
                "{at} has a {noun} name that does not match {NAME_PATTERN}"
            ));
        }
        guard_credential_shape(name, &format!("{at} {noun} name"))?;
    }
    Ok(names)
}

pub fn reject_unknown_keys(table: &Table, allowed: &[&str], at: &str) -> Result<()> {
    let unknown = table.keys().filter(|key| !allowed.contains(key)).count();
    if unknown > 0 {
        return invalid(format!(
            "{at} has {unknown} unknown key(s); allowed: {}",
            allowed.join(", ")
        ));
    }
    Ok(())
}

pub fn optional_table<'a>(value: Option<&'a Item>, at: &str) -> Result<&'a Table> {
    match value {
        None => Ok(&EMPTY),
        Some(item) => required_table(Some(item), at),
    }
}

pub fn required_table<'a>(value: Option<&'a Item>, at: &str) -> Result<&'a Table> {
    match value {
        Some(Item::Table(table)) => Ok(table),
        _ => invalid(format!("{at} must be a table")),
    }
}

pub fn required_string<'a>(
    value: Option<&'a Item>,
    at: &str,
    max_chars: usize,
    multiline: bool,
) -> Result<&'a str> {
    let Some(Item::Str(text)) = value else {
        return invalid(format!("{at} must be a non-empty string"));
    };
    if trim(text).is_empty() {
        return invalid(format!("{at} must be a non-empty string"));
    }
    if len16(text) > max_chars {
        return invalid(format!("{at} exceeds {max_chars} characters"));
    }
    if !multiline && text != trim(text) {
        return invalid(format!("{at} must not have leading or trailing whitespace"));
    }
    assert_safe_text(text, at, multiline)?;
    Ok(text)
}

pub fn assert_safe_text(value: &str, at: &str, multiline: bool) -> Result<()> {
    let unsafe_found = value.chars().any(|c| {
        if multiline && matches!(c, '\n' | '\t' | '\u{200c}' | '\u{200d}') {
            return false;
        }
        is_unsafe_char(c)
    });
    if unsafe_found {
        return invalid(format!(
            "{at} contains control, format or line-separator characters"
        ));
    }
    Ok(())
}

pub fn optional_string<'a>(
    value: Option<&'a Item>,
    at: &str,
    max_chars: usize,
) -> Result<Option<&'a str>> {
    match value {
        None => Ok(None),
        Some(_) => required_string(value, at, max_chars, false).map(Some),
    }
}

pub fn optional_boolean(value: Option<&Item>, at: &str, fallback: bool) -> Result<bool> {
    match value {
        None => Ok(fallback),
        Some(Item::Bool(b)) => Ok(*b),
        Some(_) => invalid(format!("{at} must be true or false")),
    }
}

pub fn optional_integer(
    value: Option<&Item>,
    at: &str,
    min: i64,
    max: i64,
    fallback: i64,
) -> Result<i64> {
    match value {
        None => Ok(fallback),
        Some(Item::Int(n)) => {
            if *n < min || *n > max {
                invalid(format!("{at} must be between {min} and {max}"))
            } else {
                Ok(*n)
            }
        }
        // Beyond 64 bits is beyond every range asked for.
        Some(Item::BigInt(_)) => invalid(format!("{at} must be between {min} and {max}")),
        Some(_) => invalid(format!("{at} must be an integer")),
    }
}

pub fn enum_value<'a>(value: Option<&Item>, at: &str, allowed: &[&'a str]) -> Result<&'a str> {
    if let Some(Item::Str(text)) = value {
        if let Some(found) = allowed.iter().find(|candidate| **candidate == text) {
            return Ok(found);
        }
    }
    invalid(format!("{at} must be one of {}", allowed.join(", ")))
}

/// The names are written by the operator; values never appear in the file.
pub fn passed_environment_names(value: Option<&Item>) -> Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let Item::Array(entries) = value else {
        return invalid("env.pass must be an array of variable names");
    };
    if entries.len() > MAX_PASSED_ENV_NAMES {
        return invalid(format!("env.pass exceeds {MAX_PASSED_ENV_NAMES} names"));
    }
    let mut seen: Vec<&str> = Vec::new();
    let mut names = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let at = format!("env.pass[{index}]");
        let name = match entry {
            Item::Str(text) if env_name_ok(text) => text.as_str(),
            _ => {
                return invalid(format!(
                    "{at} must be an upper-case variable name (letters, digits and underscore, at most 64 characters)"
                ))
            }
        };
        if name.starts_with("CAPSTAN_") {
            return invalid(format!(
                "{at} must not start with CAPSTAN_: those variables belong to Capstan"
            ));
        }
        if RESERVED_ENV_NAMES.contains(&name)
            || RESERVED_ENV_PREFIXES
                .iter()
                .any(|prefix| name.starts_with(prefix))
        {
            return invalid(format!(
                "{at} must not be {name}: Capstan already sets it or it changes how an agent's shell, loader or git behaves"
            ));
        }
        if seen.contains(&name) {
            return invalid(format!("{at} repeats {name}"));
        }
        seen.push(name);
        names.push(name.to_string());
    }
    Ok(names)
}

pub fn string_list(value: Option<&Item>, at: &str, max_entries: usize) -> Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let Item::Array(entries) = value else {
        return invalid(format!("{at} must be an array of strings"));
    };
    if entries.len() > max_entries {
        return invalid(format!("{at} exceeds {max_entries} entries"));
    }
    let mut list = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let at = format!("{at}[{index}]");
        let text = required_string(Some(entry), &at, MAX_ENTRY_CHARS, false)?;
        guard_credential_shape(text, &at)?;
        if text.starts_with('-') {
            return invalid(format!("{at} must not start with a dash"));
        }
        list.push(text.to_string());
    }
    Ok(list)
}

pub fn guard_credential_shape(text: &str, at: &str) -> Result<()> {
    if looks_like_credential(text) {
        return invalid(format!(
            "{at} looks like a credential; {CONFIG_FILE_NAME} must not hold secrets"
        ));
    }
    Ok(())
}
