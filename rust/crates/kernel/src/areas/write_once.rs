//! The write-once columns of the ledger and the guard that keeps code from changing them.
//!
//! The columns below hold the text an event feed shows at event time (a message body, a prompt, a command, a report
//! summary). A row's state may move on, but these columns are written by the INSERT that creates the row and never again.
//! `write_once_violations` scans Rust source for SQL that writes one of them after the fact, and `write_once_snapshot` reads
//! a row's guarded columns so a test can compare them before and after an operation.
//!
//! The scan is a text scan: SQL assembled with `format!` cannot be seen. The one such writer, `messages::update_message`,
//! builds its SET list from the column names its callers pass, so the scan checks those call sites instead.

use crate::core::Core;
use crate::errors::{KernelError, KernelResult};
use crate::helpers::query_opt;
use regex::Regex;
use serde_json::Value;
use std::path::Path;
use std::sync::LazyLock;

/// Tables with columns that are written once, and those columns.
pub const WRITE_ONCE: &[(&str, &[&str])] = &[
    (
        "messages",
        &["body", "recipient_agent_id", "action_needed", "class"],
    ),
    (
        "prompt_relays",
        &["agent_id", "prompt_text", "options_json"],
    ),
    ("operator_proposals", &["kind", "command", "reason"]),
    ("agent_reports", &["agent_id", "summary", "commit_sha"]),
    (
        "design_approvals",
        &[
            "project_id",
            "decision_id",
            "sequence",
            "plan_id",
            "package_id",
            "decision",
            "report_id",
            "link",
            "asked_user",
            "decided_by",
            "decided_at",
        ],
    ),
    ("agents", &["kind", "role_name", "seat_id"]),
];

/// Reviewed exceptions: the whitespace-collapsed text of a reported match and the reason it is allowed. Empty entries
/// need a reason; an entry silences exactly the match whose text it equals.
pub const WRITE_ONCE_ALLOW: &[(&str, &str)] = &[(
    "update_message(kernel, &row, \"deferred\", &extra, &now)",
    "extra is a Vec<(&str, Sql)> built in the lines above with only the literal names deferred_at, deferred_reason and deferral_count",
)];

/// The row id column of each guarded table.
const ID_COLUMNS: &[(&str, &str)] = &[
    ("messages", "message_id"),
    ("prompt_relays", "relay_id"),
    ("operator_proposals", "proposal_id"),
    ("agent_reports", "report_id"),
    ("design_approvals", "decision_id"),
    ("agents", "agent_id"),
];

fn guarded(table: &str) -> Option<&'static [&'static str]> {
    WRITE_ONCE
        .iter()
        .find(|(name, _)| *name == table)
        .map(|(_, columns)| *columns)
}

// ----------------------------------------------------------------------------------------------------------- lexer

#[derive(Clone, Debug, PartialEq)]
enum Kind {
    /// A string literal, with its value (escapes resolved, line continuations joined).
    Str(String),
    Ident(String),
    Punct(char),
}

#[derive(Clone, Debug)]
struct Tok {
    kind: Kind,
    start: usize,
    end: usize,
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Splits Rust source into identifiers, punctuation and string literals; comments and whitespace are dropped.
fn lex(source: &str) -> Vec<Tok> {
    let chars: Vec<(usize, char)> = source.char_indices().collect();
    let at = |i: usize| chars.get(i).map(|(_, c)| *c);
    let pos = |i: usize| chars.get(i).map_or(source.len(), |(p, _)| *p);
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i].1;
        if c.is_whitespace() {
            i += 1;
        } else if c == '/' && at(i + 1) == Some('/') {
            while i < chars.len() && chars[i].1 != '\n' {
                i += 1;
            }
        } else if c == '/' && at(i + 1) == Some('*') {
            let mut depth = 0;
            while i < chars.len() {
                if at(i) == Some('/') && at(i + 1) == Some('*') {
                    depth += 1;
                    i += 2;
                } else if at(i) == Some('*') && at(i + 1) == Some('/') {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
        } else if c == '"' || ((c == 'r' || c == 'b') && is_string_start(&chars, i)) {
            let start = i;
            let mut j = i;
            while at(j) == Some('b') || at(j) == Some('r') {
                j += 1;
            }
            let raw = (i..j).any(|k| at(k) == Some('r'));
            let mut hashes = 0;
            while at(j) == Some('#') {
                hashes += 1;
                j += 1;
            }
            j += 1; // the opening quote
            let mut value = String::new();
            while j < chars.len() {
                let d = chars[j].1;
                if raw {
                    if d == '"' && (1..=hashes).all(|k| at(j + k) == Some('#')) {
                        j += 1 + hashes;
                        break;
                    }
                    value.push(d);
                    j += 1;
                } else if d == '\\' {
                    match at(j + 1) {
                        Some('\n') | Some('\r') => {
                            j += 1;
                            while at(j).is_some_and(char::is_whitespace) {
                                j += 1;
                            }
                        }
                        Some('n') => {
                            value.push('\n');
                            j += 2;
                        }
                        Some('t') => {
                            value.push('\t');
                            j += 2;
                        }
                        Some(other) => {
                            value.push(other);
                            j += 2;
                        }
                        None => j += 1,
                    }
                } else if d == '"' {
                    j += 1;
                    break;
                } else {
                    value.push(d);
                    j += 1;
                }
            }
            tokens.push(Tok {
                kind: Kind::Str(value),
                start: pos(start),
                end: pos(j),
            });
            i = j;
        } else if c == '\'' {
            // A char literal ('x', '\n', '"') or a lifetime ('a); only a char literal has a closing quote.
            let close = if at(i + 1) == Some('\\') {
                (i + 2..(i + 12).min(chars.len())).find(|&k| at(k) == Some('\''))
            } else if at(i + 2) == Some('\'') {
                Some(i + 2)
            } else {
                None
            };
            match close {
                Some(k) => i = k + 1,
                None => i += 1,
            }
        } else if is_ident_char(c) {
            let start = i;
            while at(i).is_some_and(is_ident_char) {
                i += 1;
            }
            tokens.push(Tok {
                kind: Kind::Ident(source[pos(start)..pos(i)].to_string()),
                start: pos(start),
                end: pos(i),
            });
        } else {
            tokens.push(Tok {
                kind: Kind::Punct(c),
                start: pos(i),
                end: pos(i + 1),
            });
            i += 1;
        }
    }
    tokens
}

/// Whether the `r`/`b` at `i` opens a string literal (r"..", r#".."#, b"..", br"..") rather than an identifier.
fn is_string_start(chars: &[(usize, char)], i: usize) -> bool {
    if i > 0 && is_ident_char(chars[i - 1].1) {
        return false;
    }
    let mut j = i;
    while matches!(chars.get(j).map(|(_, c)| *c), Some('b') | Some('r')) && j < i + 2 {
        j += 1;
    }
    while chars.get(j).map(|(_, c)| *c) == Some('#') {
        j += 1;
    }
    chars.get(j).map(|(_, c)| *c) == Some('"')
}

fn is_punct(token: &Tok, c: char) -> bool {
    token.kind == Kind::Punct(c)
}

fn is_ident(token: &Tok, name: &str) -> bool {
    matches!(&token.kind, Kind::Ident(n) if n == name)
}

/// Index of the token that closes the bracket opened at `open`.
fn matching(tokens: &[Tok], open: usize) -> Option<usize> {
    let mut depth = 0;
    for (k, token) in tokens.iter().enumerate().skip(open) {
        if let Kind::Punct(c) = token.kind {
            match c {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(k);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// The tokens without any item marked `#[cfg(test)]`.
fn without_test_items(tokens: Vec<Tok>) -> Vec<Tok> {
    let mut kept = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let is_cfg_test = i + 6 < tokens.len()
            && is_punct(&tokens[i], '#')
            && is_punct(&tokens[i + 1], '[')
            && is_ident(&tokens[i + 2], "cfg")
            && is_punct(&tokens[i + 3], '(')
            && is_ident(&tokens[i + 4], "test")
            && is_punct(&tokens[i + 5], ')')
            && is_punct(&tokens[i + 6], ']');
        if !is_cfg_test {
            kept.push(tokens[i].clone());
            i += 1;
            continue;
        }
        i += 7;
        // Other attributes of the same item.
        while i + 1 < tokens.len() && is_punct(&tokens[i], '#') && is_punct(&tokens[i + 1], '[') {
            i = matching(&tokens, i + 1).map_or(tokens.len(), |k| k + 1);
        }
        // The item itself: up to its `;` or through its `{ }` block.
        while i < tokens.len() {
            if is_punct(&tokens[i], ';') {
                i += 1;
                break;
            }
            if is_punct(&tokens[i], '{') {
                i = matching(&tokens, i).map_or(tokens.len(), |k| k + 1);
                break;
            }
            if is_punct(&tokens[i], '(') || is_punct(&tokens[i], '[') {
                i = matching(&tokens, i).map_or(tokens.len(), |k| k + 1);
            } else {
                i += 1;
            }
        }
    }
    kept
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ------------------------------------------------------------------------------------------------------------- SQL

static UPDATE_SET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bUPDATE\s+(?:OR\s+\w+\s+)?(\w+)\s+SET\s+").expect("a valid pattern")
});
static REPLACE_INTO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:INSERT\s+OR\s+)?REPLACE\s+INTO\s+(\w+)").expect("a valid pattern")
});
static INSERT_INTO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bINSERT\s+INTO\s+(\w+)").expect("a valid pattern"));
static DO_UPDATE_SET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bON\s+CONFLICT\b.*?\bDO\s+UPDATE\s+SET\s+").expect("a valid pattern")
});
static ASSIGNMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(\w+)\s*=").expect("a valid pattern"));

/// The column names assigned by the SET list at the start of `text` (which runs to the end of the statement).
fn assigned_columns(text: &str) -> Vec<String> {
    let mut columns = Vec::new();
    let mut depth = 0i32;
    let mut segment = String::new();
    let flush = |segment: &mut String, columns: &mut Vec<String>| {
        if let Some(found) = ASSIGNMENT.captures(segment) {
            columns.push(found[1].to_ascii_lowercase());
        }
        segment.clear();
    };
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if depth == 0 && (c == 'W' || c == 'w' || c == 'F' || c == 'f') {
            let upper = rest.to_ascii_uppercase();
            let boundary = segment.chars().last().is_none_or(|p| !is_ident_char(p));
            if boundary
                && (starts_keyword(&upper, "WHERE")
                    || starts_keyword(&upper, "FROM")
                    || starts_keyword(&upper, "RETURNING"))
            {
                break;
            }
        }
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ';' if depth == 0 => break,
            ',' if depth == 0 => {
                flush(&mut segment, &mut columns);
                rest = &rest[1..];
                continue;
            }
            _ => {}
        }
        segment.push(c);
        rest = &rest[c.len_utf8()..];
    }
    flush(&mut segment, &mut columns);
    columns
}

fn starts_keyword(upper: &str, keyword: &str) -> bool {
    upper.starts_with(keyword)
        && !upper[keyword.len()..]
            .chars()
            .next()
            .is_some_and(is_ident_char)
}

fn sql_violations(literal: &str) -> Vec<String> {
    let text = collapse(literal);
    let mut found = Vec::new();
    let mut check = |table: &str, columns: Option<Vec<String>>, what: &str| {
        let table = table.to_ascii_lowercase();
        let Some(guarded_columns) = guarded(&table) else {
            return;
        };
        match columns {
            None => found.push(format!("{what} {table}")),
            Some(columns) => {
                for column in columns {
                    if guarded_columns.contains(&column.as_str()) {
                        found.push(format!("{what} {table} writes {column}"));
                    }
                }
            }
        }
    };
    for caps in UPDATE_SET.captures_iter(&text) {
        let after = &text[caps.get(0).expect("whole match").end()..];
        check(&caps[1], Some(assigned_columns(after)), "UPDATE");
    }
    for caps in REPLACE_INTO.captures_iter(&text) {
        check(&caps[1], None, "REPLACE INTO");
    }
    for caps in INSERT_INTO.captures_iter(&text) {
        let after = &text[caps.get(0).expect("whole match").end()..];
        let statement = after.split(';').next().unwrap_or("");
        if let Some(upsert) = DO_UPDATE_SET.find(statement) {
            check(
                &caps[1],
                Some(assigned_columns(&statement[upsert.end()..])),
                "ON CONFLICT DO UPDATE",
            );
        }
    }
    found
}

// --------------------------------------------------------------------------------------------- update_message calls

/// One `update_message(...)` call: its text, and the column names it passes or why they cannot be read.
#[derive(Debug)]
pub struct UpdateMessageCall {
    pub text: String,
    pub columns: Result<Vec<String>, String>,
}

/// The names of a `&[("name", value), ...]` argument; `None` for any other shape.
fn literal_columns(arg: &[Tok]) -> Option<Vec<String>> {
    if arg.len() < 3 || !is_punct(&arg[0], '&') || !is_punct(&arg[1], '[') {
        return None;
    }
    if matching(arg, 1) != Some(arg.len() - 1) {
        return None;
    }
    let mut columns = Vec::new();
    for element in split_top_level(&arg[2..arg.len() - 1]) {
        if element.is_empty() {
            continue;
        }
        let tuple = element.len() >= 4
            && is_punct(&element[0], '(')
            && matching(element, 0) == Some(element.len() - 1)
            && is_punct(&element[2], ',');
        match (&element.get(1).map(|t| &t.kind), tuple) {
            (Some(Kind::Str(name)), true) => columns.push(name.to_ascii_lowercase()),
            _ => return None,
        }
    }
    Some(columns)
}

/// Splits tokens at the commas outside any bracket.
fn split_top_level(tokens: &[Tok]) -> Vec<&[Tok]> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut from = 0;
    for (k, token) in tokens.iter().enumerate() {
        match token.kind {
            Kind::Punct('(') | Kind::Punct('[') | Kind::Punct('{') => depth += 1,
            Kind::Punct(')') | Kind::Punct(']') | Kind::Punct('}') => depth -= 1,
            Kind::Punct(',') if depth == 0 => {
                parts.push(&tokens[from..k]);
                from = k + 1;
            }
            _ => {}
        }
    }
    parts.push(&tokens[from..]);
    parts
}

fn calls_in(source: &str, tokens: &[Tok]) -> Vec<UpdateMessageCall> {
    let mut calls = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        let is_call = is_ident(token, "update_message")
            && tokens.get(i + 1).is_some_and(|t| is_punct(t, '('))
            && !(i > 0 && is_ident(&tokens[i - 1], "fn"));
        if !is_call {
            continue;
        }
        let Some(close) = matching(tokens, i + 1) else {
            continue;
        };
        let text = collapse(&source[token.start..tokens[close].end]);
        let args = split_top_level(&tokens[i + 2..close]);
        let columns = match args.get(3).and_then(|arg| literal_columns(arg)) {
            Some(columns) if args.len() >= 5 => Ok(columns),
            _ => Err(format!(
                "update_message column argument is not a slice literal of (\"name\", value) tuples: {text}"
            )),
        };
        calls.push(UpdateMessageCall { text, columns });
    }
    calls
}

fn parse(source: &str) -> (Vec<Tok>, Vec<UpdateMessageCall>) {
    let tokens = without_test_items(lex(source));
    let calls = calls_in(source, &tokens);
    (tokens, calls)
}

/// Every `update_message(...)` call of `source` (comments and `#[cfg(test)]` items skipped).
#[doc(hidden)]
pub fn update_message_calls(source: &str) -> Vec<UpdateMessageCall> {
    parse(source).1
}

// -------------------------------------------------------------------------------------------------------- the scan

/// Writes to a guarded column found in `source`, as short descriptions; `allow` lists the texts of excepted matches.
#[doc(hidden)]
pub fn write_once_violations_with(source: &str, allow: &[(&str, &str)]) -> Vec<String> {
    let (tokens, calls) = parse(source);
    let allowed = |text: &str| allow.iter().any(|(entry, _)| *entry == text);
    let mut found = Vec::new();
    for token in &tokens {
        if let Kind::Str(literal) = &token.kind {
            for violation in sql_violations(literal) {
                if !allowed(&format!("{violation} in {}", collapse(literal)))
                    && !allowed(&violation)
                {
                    found.push(format!("{violation}: {}", collapse(literal)));
                }
            }
        }
    }
    let messages_guarded = guarded("messages").unwrap_or(&[]);
    for call in calls {
        if allowed(&call.text) {
            continue;
        }
        match call.columns {
            Err(reason) => found.push(format!("unscannable: {reason}")),
            Ok(columns) => {
                for column in columns {
                    if messages_guarded.contains(&column.as_str()) {
                        found.push(format!(
                            "update_message writes messages {column}: {}",
                            call.text
                        ));
                    }
                }
            }
        }
    }
    found
}

/// Writes to a guarded column found in `source`, with `WRITE_ONCE_ALLOW` applied.
#[doc(hidden)]
pub fn write_once_violations(source: &str) -> Vec<String> {
    write_once_violations_with(source, WRITE_ONCE_ALLOW)
}

/// The violations of every `.rs` file under `root` (the kernel's `src`), each prefixed with its path; this file is skipped.
#[doc(hidden)]
pub fn write_once_violations_in_tree(root: &Path) -> std::io::Result<Vec<String>> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(&dir)?.collect::<Result<_, _>>()?;
        entries.sort_by_key(|e| e.path());
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|e| e == "rs")
                && path.file_name().is_some_and(|n| n != "write_once.rs")
            {
                let source = std::fs::read_to_string(&path)?;
                let shown = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .display()
                    .to_string();
                found.extend(
                    write_once_violations(&source)
                        .into_iter()
                        .map(|v| format!("{shown}: {v}")),
                );
            }
        }
    }
    Ok(found)
}

// ------------------------------------------------------------------------------------------------------- snapshot

fn json_value(value: rusqlite::types::ValueRef<'_>) -> Value {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(n) => Value::from(n),
        ValueRef::Real(x) => Value::from(x),
        ValueRef::Text(bytes) => Value::String(String::from_utf8_lossy(bytes).into_owned()),
        ValueRef::Blob(bytes) => Value::Array(bytes.iter().map(|b| Value::from(*b)).collect()),
    }
}

/// The guarded columns of the row of `table` whose id column is `id` (in this project), in `WRITE_ONCE` order; empty for
/// a missing id. `table` must be one of `WRITE_ONCE`.
#[doc(hidden)]
pub fn write_once_snapshot(
    core: &Core,
    table: &str,
    id: &str,
) -> KernelResult<Vec<(String, Value)>> {
    let (Some(columns), Some((_, id_column))) = (
        guarded(table),
        ID_COLUMNS.iter().find(|(name, _)| *name == table),
    ) else {
        return Err(KernelError::type_error(format!(
            "{table} has no write-once columns"
        )));
    };
    let kernel = core.kernel();
    let sql = format!(
        "SELECT {} FROM {table} WHERE project_id = ? AND {id_column} = ?",
        columns.join(", ")
    );
    let row = query_opt(
        &kernel.database,
        &sql,
        rusqlite::params![kernel.project_id, id],
        |row| {
            (0..columns.len())
                .map(|k| row.get_ref(k).map(json_value))
                .collect::<rusqlite::Result<Vec<_>>>()
        },
    )?;
    Ok(row
        .map(|values| columns.iter().map(|c| c.to_string()).zip(values).collect())
        .unwrap_or_default())
}
