//! The feed cursor: the ledger's epoch and history anchors, and the codec of the cursor a client carries between polls.
//!
//! `controller_events` rows cannot change or vanish (the immutable triggers of migration 0001) and event ids are random,
//! so a ledger restored below some sequence and regrown has other event ids from there on. The epoch (from sequence 1)
//! and the anchor (from the sequence a cursor stands at) let a client notice that. All three reads are plain functions on
//! `&Core`; a caller that needs them to agree runs them inside one job, so no write falls between them.

use crate::canonical::sha256;
use crate::core::Core;
use crate::errors::KernelResult;
use crate::helpers::query_opt;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fmt;

/// The prefix of an encoded cursor; the digit is the format version.
pub const CURSOR_PREFIX: &str = "c1.";

fn short(digest: String) -> String {
    digest[..16].to_string()
}

fn event_id_at(core: &Core, sequence: i64) -> KernelResult<Option<String>> {
    let kernel = core.kernel();
    query_opt(
        &kernel.database,
        "SELECT event_id FROM controller_events WHERE project_id = ? AND sequence = ?",
        rusqlite::params![kernel.project_id, sequence],
        |row| row.get::<_, String>(0),
    )
}

/// 16 hex of the SHA-256 of the event id of sequence 1; a ledger without sequence 1 gets one derived from the project id.
/// Only an insert can change it, so a client that sees it change holds a replaced ledger.
pub fn epoch(core: &Core) -> KernelResult<String> {
    Ok(short(match event_id_at(core, 1)? {
        Some(event_id) => sha256(&event_id),
        None => sha256(&format!("no-genesis:{}", core.project_id())),
    }))
}

/// The highest sequence of the ledger, or 0 when it has none.
pub fn head(core: &Core) -> KernelResult<i64> {
    let kernel = core.kernel();
    Ok(query_opt(
        &kernel.database,
        "SELECT COALESCE(MAX(sequence), 0) FROM controller_events WHERE project_id = ?",
        [&kernel.project_id],
        |row| row.get::<_, i64>(0),
    )?
    .unwrap_or(0))
}

/// 16 hex of the SHA-256 of the event id at `sequence`; `None` for sequence 0 or a sequence the ledger lacks.
pub fn anchor(core: &Core, sequence: i64) -> KernelResult<Option<String>> {
    if sequence <= 0 {
        return Ok(None);
    }
    Ok(event_id_at(core, sequence)?.map(|event_id| short(sha256(&event_id))))
}

// ---------------------------------------------------------------------------------------------------------- codec

/// Where a client stands in one project's feed. `r` and `d` are carried for the CLI, which sets them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorEntry {
    /// The ledger epoch, 16 hex.
    pub e: String,
    /// The last sequence read.
    pub s: u64,
    /// The next sub-event of sequence `s` (omitted from the encoding when 0).
    pub k: u32,
    /// The anchor of `s`, 16 hex.
    pub h: String,
    /// The project root.
    pub r: String,
    /// Unix seconds since the project was found unavailable (omitted when `None`).
    pub d: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CursorError {
    Prefix,
    Base64,
    Json,
    Field(String),
}

impl fmt::Display for CursorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CursorError::Prefix => write!(f, "cursor does not start with {CURSOR_PREFIX}"),
            CursorError::Base64 => write!(f, "cursor is not base64url"),
            CursorError::Json => write!(f, "cursor is not valid JSON"),
            CursorError::Field(field) => write!(f, "cursor field {field} is not valid"),
        }
    }
}

impl std::error::Error for CursorError {}

/// `c1.` and the unpadded base64url of `{"p":{<projectId>:entry}}`, keys sorted and no whitespace.
pub fn encode_cursor(entries: &BTreeMap<String, CursorEntry>) -> String {
    let mut projects = Map::new();
    for (project_id, entry) in entries {
        // Inserted in key order (d, e, h, k, r, s) so the bytes are the same whatever the map keeps.
        let mut object = Map::new();
        if let Some(d) = entry.d {
            object.insert("d".into(), Value::from(d));
        }
        object.insert("e".into(), Value::from(entry.e.clone()));
        object.insert("h".into(), Value::from(entry.h.clone()));
        if entry.k != 0 {
            object.insert("k".into(), Value::from(entry.k));
        }
        object.insert("r".into(), Value::from(entry.r.clone()));
        object.insert("s".into(), Value::from(entry.s));
        projects.insert(project_id.clone(), Value::Object(object));
    }
    let mut root = Map::new();
    root.insert("p".into(), Value::Object(projects));
    format!(
        "{CURSOR_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(Value::Object(root).to_string())
    )
}

fn is_short_hex(value: &str) -> bool {
    value.len() == 16
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn text_field(object: &Map<String, Value>, name: &str) -> Result<String, CursorError> {
    match object.get(name) {
        Some(Value::String(text)) => Ok(text.clone()),
        _ => Err(CursorError::Field(name.into())),
    }
}

fn hex_field(object: &Map<String, Value>, name: &str) -> Result<String, CursorError> {
    let text = text_field(object, name)?;
    if is_short_hex(&text) {
        Ok(text)
    } else {
        Err(CursorError::Field(name.into()))
    }
}

fn entry_of(value: &Value) -> Result<CursorEntry, CursorError> {
    let object = value
        .as_object()
        .ok_or_else(|| CursorError::Field("p".into()))?;
    if let Some(unknown) = object
        .keys()
        .find(|key| !matches!(key.as_str(), "d" | "e" | "h" | "k" | "r" | "s"))
    {
        return Err(CursorError::Field(unknown.clone()));
    }
    let s = object
        .get("s")
        .and_then(Value::as_u64)
        .ok_or_else(|| CursorError::Field("s".into()))?;
    let k = match object.get("k") {
        None => 0,
        Some(value) => value
            .as_u64()
            .and_then(|k| u32::try_from(k).ok())
            .ok_or_else(|| CursorError::Field("k".into()))?,
    };
    let d = match object.get("d") {
        None => None,
        Some(value) => Some(
            value
                .as_i64()
                .ok_or_else(|| CursorError::Field("d".into()))?,
        ),
    };
    Ok(CursorEntry {
        e: hex_field(object, "e")?,
        s,
        k,
        h: hex_field(object, "h")?,
        r: text_field(object, "r")?,
        d,
    })
}

/// The entries of a cursor text; refuses another prefix, bad base64, bad JSON or a bad field.
pub fn decode_cursor(text: &str) -> Result<BTreeMap<String, CursorEntry>, CursorError> {
    let body = text
        .strip_prefix(CURSOR_PREFIX)
        .ok_or(CursorError::Prefix)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(body)
        .map_err(|_| CursorError::Base64)?;
    let root: Value = serde_json::from_slice(&bytes).map_err(|_| CursorError::Json)?;
    let root = root
        .as_object()
        .ok_or_else(|| CursorError::Field("p".into()))?;
    if root.len() != 1 {
        return Err(CursorError::Field("p".into()));
    }
    let projects = root
        .get("p")
        .and_then(Value::as_object)
        .ok_or_else(|| CursorError::Field("p".into()))?;
    projects
        .iter()
        .map(|(project_id, value)| Ok((project_id.clone(), entry_of(value)?)))
        .collect()
}
