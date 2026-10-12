//! Id prefixes, git-style: the one place that turns a unique prefix of a record id into the full id.
//!
//! An id that names a record exactly always stands for itself. Otherwise a prefix of at least `MIN_PREFIX` characters
//! that starts exactly one id of the given tables is that id; one that starts several is ambiguous; a shorter one that
//! would have matched is too short. Anything that matches nothing is left alone, so the command refuses it as it did
//! before.

use crate::errors::KernelResult;
use crate::helpers::{exists, query_all};
use crate::kernel::Kernel;

/// The shortest prefix that is resolved.
pub const MIN_PREFIX: usize = 4;
/// How many candidates an ambiguity lists.
pub const MAX_CANDIDATES: usize = 8;

/// A table and the column that holds its ids.
pub type IdColumn = (&'static str, &'static str);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolved {
    /// The id is exact, or nothing starts with it: pass it on as it is.
    Unchanged,
    /// The one id that starts with the prefix.
    Full(String),
    /// Several ids start with the prefix (at most `MAX_CANDIDATES`, sorted).
    Ambiguous(Vec<String>),
    /// Fewer than `MIN_PREFIX` characters, and some id starts with them.
    TooShort,
}

/// Resolves `id` against the ids of `targets` (table names and columns come from code, never from a client).
pub fn resolve_id(kernel: &Kernel, targets: &[IdColumn], id: &str) -> KernelResult<Resolved> {
    kernel.assert_open()?;
    for (table, column) in targets {
        let sql = format!("SELECT 1 FROM {table} WHERE project_id = ? AND {column} = ?");
        if exists(&kernel.database, &sql, [&kernel.project_id, id])? {
            return Ok(Resolved::Unchanged);
        }
    }
    let length = id.chars().count() as i64;
    let mut found: Vec<String> = Vec::new();
    for (table, column) in targets {
        let sql = format!(
            "SELECT {column} FROM {table} WHERE project_id = ?1 AND substr({column}, 1, ?2) = ?3 ORDER BY {column} LIMIT {}",
            MAX_CANDIDATES + 1
        );
        let rows = query_all(
            &kernel.database,
            &sql,
            rusqlite::params![kernel.project_id, length, id],
            |row| row.get::<_, String>(0),
        )?;
        found.extend(rows);
    }
    found.sort();
    found.dedup();
    Ok(if found.is_empty() {
        Resolved::Unchanged
    } else if length < MIN_PREFIX as i64 {
        Resolved::TooShort
    } else if found.len() == 1 {
        Resolved::Full(found.remove(0))
    } else {
        found.truncate(MAX_CANDIDATES);
        Resolved::Ambiguous(found)
    })
}

/// What a client is told when `resolved` is not usable (`Unchanged` and `Full` are never refused).
pub fn refusal(id: &str, resolved: &Resolved) -> String {
    match resolved {
        Resolved::TooShort => {
            format!("id prefix \"{id}\" is too short: give at least {MIN_PREFIX} characters")
        }
        Resolved::Ambiguous(candidates) => format!(
            "id prefix \"{id}\" is ambiguous: candidates: {}",
            candidates.join(", ")
        ),
        Resolved::Unchanged | Resolved::Full(_) => format!("id \"{id}\" is not a known record"),
    }
}
