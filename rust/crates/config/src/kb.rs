//! The `[kb]` section and the knowledge base entry format. The committed entries file (JSON Lines) is the source of
//! truth; this module resolves the section, parses and validates entries, writes them in id order and splits
//! identifiers into the words a search matches. Nothing here reads or writes the project's files.
use capstan_wire::js::Value;
use serde_json::{Map, Value as Json};

use crate::error::{invalid, Result};
use crate::json::{number, object, strings, text};
use crate::keys::KB_KEYS;
use crate::primitives::*;
use crate::types::Role;
use crate::value::Item;

pub const DEFAULT_PATH: &str = "docs/capstan/kb.jsonl";
pub const MAX_PATH_BYTES: usize = 200;
pub const MAX_UI_PATHS: usize = 32;
const MAX_LIST: usize = 8;

/// The resolved `[kb]` section; `RoleConfig.kb` is `None` when the table is absent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KbConfig {
    pub enabled: bool,
    pub path: String,
    pub max_entry_bytes: i64,
    pub max_entries: i64,
    pub search_limit: i64,
    pub reuse_max_bytes: i64,
    pub stale_warn: i64,
    pub ui_paths: Vec<String>,
    pub design_stale_commits: i64,
    pub role: String,
    pub design_role: String,
}

impl Default for KbConfig {
    fn default() -> Self {
        KbConfig {
            enabled: true,
            path: DEFAULT_PATH.to_string(),
            max_entry_bytes: 1200,
            max_entries: 2000,
            search_limit: 5,
            reuse_max_bytes: 1500,
            stale_warn: 1,
            ui_paths: Vec::new(),
            design_stale_commits: 20,
            role: "docs-keeper".to_string(),
            design_role: "designer".to_string(),
        }
    }
}

impl KbConfig {
    pub(crate) fn to_value(&self) -> Value {
        object(vec![
            ("enabled", Value::Bool(self.enabled)),
            ("path", text(&self.path)),
            ("maxEntryBytes", number(self.max_entry_bytes)),
            ("maxEntries", number(self.max_entries)),
            ("searchLimit", number(self.search_limit)),
            ("reuseMaxBytes", number(self.reuse_max_bytes)),
            ("staleWarn", number(self.stale_warn)),
            ("uiPaths", strings(&self.ui_paths)),
            ("designStaleCommits", number(self.design_stale_commits)),
            ("role", text(&self.role)),
            ("designRole", text(&self.design_role)),
        ])
    }
}

/// Whether `path` is a repo-relative path: not empty, at most 200 bytes, no leading `/`, no `..` or empty segment, no control
/// character or backslash.
fn repo_path_ok(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_PATH_BYTES
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.chars().any(char::is_control)
        && !path
            .strip_suffix('/')
            .unwrap_or(path)
            .split('/')
            .any(|part| part == ".." || part.is_empty())
}

fn ui_path_globs(value: Option<&Item>) -> Result<Vec<String>> {
    let at = "kb.ui_paths";
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let Item::Array(entries) = value else {
        return invalid(format!(
            "{at} must be an array of 0 to {MAX_UI_PATHS} path globs"
        ));
    };
    if entries.len() > MAX_UI_PATHS {
        return invalid(format!("{at} must have at most {MAX_UI_PATHS} entries"));
    }
    let mut globs: Vec<String> = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let at = format!("{at}[{index}]");
        let glob = required_string(Some(entry), &at, MAX_PATH_BYTES, false)?;
        if glob.starts_with('/') || glob.starts_with('~') || glob.contains('\\') {
            return invalid(format!(
                "{at} must be a path relative to the project root, with / separators"
            ));
        }
        if glob.split('/').any(|part| part == ".." || part.is_empty()) {
            return invalid(format!("{at} must not have an empty or .. segment"));
        }
        if globs.iter().any(|seen| seen == glob) {
            return invalid(format!("{at} repeats {glob}"));
        }
        globs.push(glob.to_string());
    }
    Ok(globs)
}

fn role_name(
    table: &crate::value::Table,
    key: &str,
    fallback: &str,
    roles: &[Role],
    active: bool,
) -> Result<String> {
    let at = format!("kb.{key}");
    let written = table.get(key).is_some();
    let name = match optional_string(table.get(key), &at, 64)? {
        Some(name) => name.to_string(),
        None => fallback.to_string(),
    };
    if written
        && active
        && !roles
            .iter()
            .any(|role| role.kind == "Developer" && role.name == name)
    {
        return invalid(format!(
            "{at} \"{name}\" does not name a configured Developer role"
        ));
    }
    Ok(name)
}

/// Resolves `[kb]` against the resolved roles; an absent table is `None`. A role key is checked only when written;
/// `design_role` only when `[design]` is configured.
pub fn resolve_kb(
    value: Option<&Item>,
    roles: &[Role],
    design_configured: bool,
) -> Result<Option<KbConfig>> {
    if value.is_none() {
        return Ok(None);
    }
    let table = required_table(value, "kb")?;
    reject_unknown_keys(table, KB_KEYS, "kb")?;
    let d = KbConfig::default();
    let enabled = optional_boolean(table.get("enabled"), "kb.enabled", d.enabled)?;
    let path = match optional_string(table.get("path"), "kb.path", MAX_PATH_BYTES)? {
        None => d.path.clone(),
        Some(path) => {
            if !repo_path_ok(path) || path.ends_with('/') {
                return invalid(
                    "kb.path must be a repo-relative file of at most 200 bytes, without a leading / or a .. segment"
                        .to_string(),
                );
            }
            path.to_string()
        }
    };
    let int = |key: &str, min: i64, max: i64, fallback: i64| {
        optional_integer(table.get(key), &format!("kb.{key}"), min, max, fallback)
    };
    let max_entry_bytes = int("max_entry_bytes", 256, 4000, d.max_entry_bytes)?;
    let max_entries = int("max_entries", 1, 10_000, d.max_entries)?;
    let search_limit = int("search_limit", 1, 20, d.search_limit)?;
    let reuse_max_bytes = int("reuse_max_bytes", 0, 4000, d.reuse_max_bytes)?;
    let stale_warn = int("stale_warn", 1, 10_000, d.stale_warn)?;
    let ui_paths = ui_path_globs(table.get("ui_paths"))?;
    let design_stale_commits = int("design_stale_commits", 1, 10_000, d.design_stale_commits)?;
    let role = role_name(table, "role", &d.role, roles, true)?;
    // design_role is only held to a configured role while [design] is configured.
    let design_role = role_name(
        table,
        "design_role",
        &d.design_role,
        roles,
        design_configured,
    )?;
    Ok(Some(KbConfig {
        enabled,
        path,
        max_entry_bytes,
        max_entries,
        search_limit,
        reuse_max_bytes,
        stale_warn,
        ui_paths,
        design_stale_commits,
        role,
        design_role,
    }))
}

// --- the entry format -----------------------------------------------------------------------------------------------

/// What an entry records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Helper,
    Component,
    Convention,
    DesignRule,
    Decision,
}

impl Kind {
    pub const ALL: [Kind; 5] = [
        Kind::Helper,
        Kind::Component,
        Kind::Convention,
        Kind::DesignRule,
        Kind::Decision,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Helper => "helper",
            Kind::Component => "component",
            Kind::Convention => "convention",
            Kind::DesignRule => "design_rule",
            Kind::Decision => "decision",
        }
    }

    pub fn parse(text: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.as_str() == text)
    }
}

/// One knowledge base entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub kind: Kind,
    pub title: String,
    pub body: String,
    pub path: Option<String>,
    pub area: Option<String>,
    pub tags: Vec<String>,
    pub aliases: Vec<String>,
    pub related: Vec<String>,
    pub verified_commit: String,
}

/// The result of reading an entries file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parsed {
    pub entries: Vec<Entry>,
    /// (1-based line number, reason) for every line that is not a valid entry.
    pub invalid: Vec<(usize, String)>,
}

/// The keys of an entry line, in their order.
const FIELD_ORDER: [&str; 10] = [
    "id",
    "kind",
    "title",
    "body",
    "path",
    "area",
    "tags",
    "aliases",
    "related",
    "verified_commit",
];

fn id_ok(id: &str) -> bool {
    let bytes = id.as_bytes();
    let Some((head, tail)) = bytes.split_first() else {
        return false;
    };
    (head.is_ascii_lowercase() || head.is_ascii_digit())
        && tail.len() <= 63
        && tail.iter().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        })
}

fn slug_ok(text: &str, max: usize) -> bool {
    !text.is_empty()
        && text.len() <= max
        && text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn one_line(text: &str) -> bool {
    !text.chars().any(char::is_control)
}

/// Checks every rule of an entry; the text names the field and the rule.
pub fn validate(entry: &Entry, max_entry_bytes: usize) -> std::result::Result<(), String> {
    if !id_ok(&entry.id) {
        return Err("kb entry: id must match ^[a-z0-9][a-z0-9._-]{0,63}$".to_string());
    }
    if entry.title.is_empty() || entry.title.len() > 120 {
        return Err("kb entry: title must be 1 to 120 bytes".to_string());
    }
    if !one_line(&entry.title) {
        return Err("kb entry: title must be one line".to_string());
    }
    if entry.body.is_empty() {
        return Err("kb entry: body must not be empty".to_string());
    }
    if !one_line(&entry.body) {
        return Err("kb entry: body must be one line".to_string());
    }
    if let Some(path) = &entry.path {
        if !repo_path_ok(path) {
            return Err("kb entry: path must be a repo-relative file or directory of at most 200 bytes, without a leading / or a .. segment".to_string());
        }
    }
    if let Some(area) = &entry.area {
        if !slug_ok(area, 40) {
            return Err("kb entry: area must match ^[a-z0-9-]{1,40}$".to_string());
        }
    }
    if entry.tags.len() > MAX_LIST {
        return Err("kb entry: tags must have at most 8 entries".to_string());
    }
    if entry.tags.iter().any(|tag| !slug_ok(tag, 32)) {
        return Err("kb entry: tags entry must match ^[a-z0-9-]{1,32}$".to_string());
    }
    if entry.aliases.len() > MAX_LIST {
        return Err("kb entry: aliases must have at most 8 entries".to_string());
    }
    if entry
        .aliases
        .iter()
        .any(|alias| alias.is_empty() || alias.len() > 64 || !one_line(alias))
    {
        return Err("kb entry: aliases entry must be 1 to 64 bytes on one line".to_string());
    }
    if entry.related.len() > MAX_LIST {
        return Err("kb entry: related must have at most 8 entries".to_string());
    }
    let related_ok = |item: &String| match item.strip_prefix("path:") {
        Some(path) => repo_path_ok(path),
        None => id_ok(item),
    };
    if !entry.related.iter().all(related_ok) {
        return Err("kb entry: related entry must be an entry id or path:<repo path>".to_string());
    }
    let commit = entry.verified_commit.as_bytes();
    if commit.len() != 40
        || !commit
            .iter()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
    {
        return Err("kb entry: verified_commit must be 40 lowercase hex characters".to_string());
    }
    let size = to_line(entry).len();
    if size > max_entry_bytes {
        return Err(format!(
            "kb entry: line is {size} bytes, over the limit of {max_entry_bytes}"
        ));
    }
    Ok(())
}

/// The canonical line of an entry: keys in order, absent optional keys and empty lists left out, no newline.
pub fn to_line(entry: &Entry) -> String {
    let mut map = Map::new();
    let string = |s: &str| Json::String(s.to_string());
    let list = |items: &[String]| Json::Array(items.iter().map(|s| string(s)).collect());
    map.insert("id".into(), string(&entry.id));
    map.insert("kind".into(), string(entry.kind.as_str()));
    map.insert("title".into(), string(&entry.title));
    map.insert("body".into(), string(&entry.body));
    if let Some(path) = &entry.path {
        map.insert("path".into(), string(path));
    }
    if let Some(area) = &entry.area {
        map.insert("area".into(), string(area));
    }
    for (key, items) in [
        ("tags", &entry.tags),
        ("aliases", &entry.aliases),
        ("related", &entry.related),
    ] {
        if !items.is_empty() {
            map.insert(key.into(), list(items));
        }
    }
    map.insert("verified_commit".into(), string(&entry.verified_commit));
    serde_json::to_string(&Json::Object(map)).unwrap_or_default()
}

fn text_field(map: &Map<String, Json>, key: &str) -> std::result::Result<Option<String>, String> {
    match map.get(key) {
        None => Ok(None),
        Some(Json::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("kb entry: {key} must be a string")),
    }
}

fn list_field(map: &Map<String, Json>, key: &str) -> std::result::Result<Vec<String>, String> {
    match map.get(key) {
        None => Ok(Vec::new()),
        Some(Json::Array(items)) => items
            .iter()
            .map(|item| match item {
                Json::String(s) => Ok(s.clone()),
                _ => Err(format!("kb entry: {key} must be an array of strings")),
            })
            .collect(),
        Some(_) => Err(format!("kb entry: {key} must be an array of strings")),
    }
}

fn parse_line(line: &str, max_entry_bytes: usize) -> std::result::Result<Entry, String> {
    if line.len() > max_entry_bytes {
        return Err(format!(
            "kb entry: line is {} bytes, over the limit of {max_entry_bytes}",
            line.len()
        ));
    }
    let Ok(Json::Object(map)) = serde_json::from_str::<Json>(line) else {
        return Err("kb entry: line must be one JSON object".to_string());
    };
    let mut last = None;
    for key in map.keys() {
        let Some(position) = FIELD_ORDER.iter().position(|name| name == key) else {
            return Err(format!("kb entry: unknown key {key}"));
        };
        if last.is_some_and(|before| before >= position) {
            return Err(format!("kb entry: key {key} is out of order"));
        }
        last = Some(position);
    }
    let required =
        |key: &str| text_field(&map, key)?.ok_or_else(|| format!("kb entry: {key} is required"));
    let kind_text = required("kind")?;
    let Some(kind) = Kind::parse(&kind_text) else {
        return Err(
            "kb entry: kind must be one of helper, component, convention, design_rule, decision"
                .to_string(),
        );
    };
    let entry = Entry {
        id: required("id")?,
        kind,
        title: required("title")?,
        body: required("body")?,
        path: text_field(&map, "path")?,
        area: text_field(&map, "area")?,
        tags: list_field(&map, "tags")?,
        aliases: list_field(&map, "aliases")?,
        related: list_field(&map, "related")?,
        verified_commit: required("verified_commit")?,
    };
    validate(&entry, max_entry_bytes)?;
    Ok(entry)
}

/// Reads an entries file. Blank lines are skipped; every other line is an entry or is listed as invalid with its line
/// number. A duplicate id keeps the first entry and marks the later line invalid.
pub fn parse_file(text: &str, max_entry_bytes: usize) -> Parsed {
    let mut entries: Vec<Entry> = Vec::new();
    let mut first_line: Vec<usize> = Vec::new();
    let mut invalid = Vec::new();
    for (index, line) in text.split('\n').enumerate() {
        let number = index + 1;
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.trim().is_empty() {
            continue;
        }
        match parse_line(line, max_entry_bytes) {
            Ok(entry) => match entries.iter().position(|seen| seen.id == entry.id) {
                Some(at) => invalid.push((
                    number,
                    format!(
                        "kb entry: id {} repeats the entry on line {}",
                        entry.id, first_line[at]
                    ),
                )),
                None => {
                    first_line.push(number);
                    entries.push(entry);
                }
            },
            Err(reason) => invalid.push((number, reason)),
        }
    }
    Parsed { entries, invalid }
}

/// The id of a line, when it is a JSON object with a string id.
fn line_id(line: &str) -> Option<String> {
    match serde_json::from_str::<Json>(line).ok()? {
        Json::Object(mut map) => match map.remove("id")? {
            Json::String(id) => Some(id),
            _ => None,
        },
        _ => None,
    }
}

fn body_lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    text.strip_suffix('\n')
        .unwrap_or(text)
        .split('\n')
        .collect()
}

fn joined(lines: &[&str]) -> String {
    let mut out = lines.join("\n");
    if !lines.is_empty() {
        out.push('\n');
    }
    out
}

/// The file with `entry` written in id order: a line with the same id is replaced in place wherever it is (even in an
/// unsorted file); only when no line has that id does the entry go before the first greater id (or at the end). Every
/// other line is kept byte for byte; the result ends with a newline.
pub fn upsert_sorted(text: &str, entry: &Entry) -> String {
    let new_line = to_line(entry);
    let mut lines = body_lines(text);
    let existing = lines
        .iter()
        .position(|line| line_id(line).as_deref() == Some(entry.id.as_str()));
    match existing {
        Some(at) => lines[at] = &new_line,
        None => {
            let at = lines
                .iter()
                .position(|line| {
                    line_id(line).is_some_and(|id| id.as_bytes() > entry.id.as_bytes())
                })
                .unwrap_or(lines.len());
            lines.insert(at, &new_line);
        }
    }
    joined(&lines)
}

/// The file without the entry `id`, or `None` when no line has that id. Other lines and the final newline are kept.
pub fn remove(text: &str, id: &str) -> Option<String> {
    let lines = body_lines(text);
    let at = lines
        .iter()
        .position(|line| line_id(line).as_deref() == Some(id))?;
    let mut rest = lines;
    rest.remove(at);
    if text.ends_with('\n') {
        Some(joined(&rest))
    } else {
        Some(rest.join("\n"))
    }
}

/// The words of an identifier: split on characters that are not letters or digits, between a lowercase letter or digit
/// and an uppercase letter, and between an acronym and the word after it (`HTTPServer` is `http`, `server`);
/// lowercased, with the joined form added and duplicates removed.
pub fn split_identifier(text: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    for segment in text.split(|c: char| !c.is_alphanumeric()) {
        let chars: Vec<char> = segment.chars().collect();
        let mut word = String::new();
        for (i, &c) in chars.iter().enumerate() {
            if i > 0 && c.is_uppercase() {
                let before = chars[i - 1];
                let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
                if before.is_lowercase()
                    || before.is_ascii_digit()
                    || (before.is_uppercase() && next_lower)
                {
                    words.push(std::mem::take(&mut word));
                }
            }
            word.extend(c.to_lowercase());
        }
        words.push(word);
    }
    words.retain(|word| !word.is_empty());
    let joined: String = words.concat();
    if !joined.is_empty() {
        words.push(joined);
    }
    let mut seen: Vec<String> = Vec::new();
    for word in words {
        if !seen.contains(&word) {
            seen.push(word);
        }
    }
    seen
}

/// The words a search matches an entry on: its id, title words, the path's file stem and its aliases, each split by
/// `split_identifier`; deduplicated, in that order.
pub fn alias_words(entry: &Entry) -> Vec<String> {
    let mut sources: Vec<String> = vec![entry.id.clone()];
    sources.extend(entry.title.split_whitespace().map(str::to_string));
    if let Some(path) = &entry.path {
        let name = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
        let stem = match name.rfind('.') {
            Some(dot) if dot > 0 => &name[..dot],
            _ => name,
        };
        sources.push(stem.to_string());
    }
    sources.extend(entry.aliases.iter().cloned());
    let mut words: Vec<String> = Vec::new();
    for source in sources {
        for word in split_identifier(&source) {
            if !words.contains(&word) {
                words.push(word);
            }
        }
    }
    words
}
