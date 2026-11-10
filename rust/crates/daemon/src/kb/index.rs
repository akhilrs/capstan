//! The SQLite index: tables, rebuild from parsed entries, and the lookups.
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use capstan_config::kb::{alias_words, split_identifier, Parsed};
use capstan_config::KbEntry;
use rusqlite::{params, Connection, OptionalExtension};

use super::stale::Check;
use super::KbError;

const QUERY_DAYS: i64 = 30;
const QUERY_ROWS: i64 = 10_000;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS entries(
  id TEXT PRIMARY KEY, kind TEXT NOT NULL, title TEXT NOT NULL, body TEXT NOT NULL, path TEXT, area TEXT,
  tags TEXT NOT NULL, aliases TEXT NOT NULL, related TEXT NOT NULL, verified_commit TEXT NOT NULL,
  state TEXT NOT NULL, reason TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS entries_path ON entries(path);
CREATE TABLE IF NOT EXISTS links(from_id TEXT NOT NULL, target_kind TEXT NOT NULL, target TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS links_target ON links(target_kind, target);
CREATE VIRTUAL TABLE IF NOT EXISTS entries_fts USING fts5(title, body, tags, aliases,
  tokenize = 'unicode61 remove_diacritics 2');
CREATE TABLE IF NOT EXISTS queries(at INTEGER NOT NULL, words TEXT NOT NULL, hits INTEGER NOT NULL);
";

/// One entry as a lookup returns it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub path: Option<String>,
    pub area: Option<String>,
    pub state: String,
    pub reason: String,
    /// The entry names the path or entry it was found for in `related`.
    pub uses: bool,
}

/// The index's state for `cstan kb status` and the warnings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    /// "ok", or "missing" when the entries file is not at HEAD.
    pub state: String,
    pub head: String,
    pub entries: usize,
    pub invalid: Vec<(usize, String)>,
    pub stale: usize,
    pub missing: usize,
    pub searches_30d: usize,
    pub misses_30d: usize,
    pub vectors_hint: String,
}

pub struct KbIndex {
    conn: Connection,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn hit(row: &rusqlite::Row<'_>) -> rusqlite::Result<Hit> {
    Ok(Hit {
        id: row.get(0)?,
        kind: row.get(1)?,
        title: row.get(2)?,
        path: row.get(3)?,
        area: row.get(4)?,
        state: row.get(5)?,
        reason: row.get(6)?,
        uses: false,
    })
}

const COLS: &str = "e.id, e.kind, e.title, e.path, e.area, e.state, e.reason";

fn quote(term: &str) -> String {
    format!("\"{}\"*", term.replace('"', "\"\""))
}

fn terms(words: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in words {
        for term in split_identifier(word) {
            if !out.contains(&term) {
                out.push(term);
            }
        }
    }
    out
}

impl KbIndex {
    pub(super) fn open(state_dir: &Path, _root: &Path) -> Result<KbIndex, KbError> {
        let dir = state_dir.join("kb");
        std::fs::create_dir_all(&dir).map_err(|e| KbError::Store(e.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| KbError::Store(e.to_string()))?;
        }
        let conn = Connection::open(dir.join("index.sqlite"))?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        conn.execute_batch(SCHEMA)?;
        Ok(KbIndex { conn })
    }

    /// The commit the index was built at.
    pub fn head(&self) -> Option<String> {
        self.meta("head")
    }

    pub(super) fn meta(&self, key: &str) -> Option<String> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    /// Replaces the entry tables in one transaction; `queries` is kept.
    pub(super) fn rebuild(
        &mut self,
        head: &str,
        blob: &str,
        parsed: Option<&Parsed>,
        checks: &[Check],
    ) -> Result<(), KbError> {
        let tx = self.conn.transaction()?;
        tx.execute_batch("DELETE FROM entries; DELETE FROM links; DELETE FROM entries_fts;")?;
        let mut invalid: Vec<(usize, &str)> = Vec::new();
        if let Some(parsed) = parsed {
            invalid = parsed
                .invalid
                .iter()
                .map(|(l, r)| (*l, r.as_str()))
                .collect();
            for (entry, check) in parsed.entries.iter().zip(checks) {
                insert(&tx, entry, check)?;
            }
        }
        let invalid_json =
            serde_json::to_string(&invalid).map_err(|e| KbError::Store(e.to_string()))?;
        let file = if parsed.is_some() {
            "present"
        } else {
            "absent"
        };
        for (key, value) in [
            ("head", head),
            ("blob", blob),
            ("file", file),
            ("invalid", &invalid_json),
        ] {
            tx.execute(
                "INSERT INTO meta(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    fn collect(&self, sql: &str, args: impl rusqlite::Params) -> Result<Vec<Hit>, KbError> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(args, hit)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    fn log(&self, words: &[String], hits: usize) -> Result<(), KbError> {
        let at = now();
        self.conn.execute(
            "INSERT INTO queries(at, words, hits) VALUES(?1, ?2, ?3)",
            params![at, words.join(" "), hits as i64],
        )?;
        self.conn.execute(
            "DELETE FROM queries WHERE at < ?1 OR rowid <= (SELECT COALESCE(MAX(rowid), 0) FROM queries) - ?2",
            params![at - QUERY_DAYS * 86_400, QUERY_ROWS],
        )?;
        Ok(())
    }

    fn match_query(&self, expr: &str, limit: usize) -> Result<Vec<Hit>, KbError> {
        if expr.is_empty() {
            return Ok(Vec::new());
        }
        self.collect(
            &format!(
                "SELECT {COLS} FROM entries_fts JOIN entries e ON e.rowid = entries_fts.rowid \
                 WHERE entries_fts MATCH ?1 ORDER BY bm25(entries_fts, 8.0, 1.0, 4.0, 6.0), e.id LIMIT ?2"
            ),
            params![expr, limit as i64],
        )
    }

    /// Entries that match any word, best first. Every call is logged with its hit count.
    pub fn search(&self, words: &[String], limit: usize) -> Result<Vec<Hit>, KbError> {
        let expr = terms(words)
            .iter()
            .map(|t| quote(t))
            .collect::<Vec<_>>()
            .join(" OR ");
        let hits = self.match_query(&expr, limit)?;
        self.log(words, hits.len())?;
        Ok(hits)
    }

    /// Entries whose path is, is a directory of, or is under one of `paths`; then entries that name one of them in
    /// `related`, marked as uses.
    pub fn for_paths(&self, paths: &[String]) -> Result<Vec<Hit>, KbError> {
        let mut out: Vec<Hit> = Vec::new();
        for path in paths {
            let p = path.trim_end_matches('/');
            let found = if p.is_empty() {
                self.collect(
                    &format!("SELECT {COLS} FROM entries e WHERE e.path IS NOT NULL ORDER BY e.path, e.id"),
                    [],
                )?
            } else {
                self.collect(
                    &format!(
                        "SELECT {COLS} FROM entries e WHERE e.path IS NOT NULL AND (e.path = ?1 \
                         OR substr(?1, 1, length(rtrim(e.path, '/')) + 1) = rtrim(e.path, '/') || '/' \
                         OR substr(e.path, 1, length(?1) + 1) = ?1 || '/') ORDER BY e.path, e.id"
                    ),
                    [p],
                )?
            };
            for h in found {
                if !out.iter().any(|o| o.id == h.id) {
                    out.push(h);
                }
            }
        }
        for path in paths {
            let p = path.trim_end_matches('/');
            let found = self.collect(
                &format!(
                    "SELECT {COLS} FROM entries e WHERE e.id IN (SELECT from_id FROM links \
                     WHERE target_kind = 'path' AND rtrim(target, '/') = ?1) ORDER BY e.id"
                ),
                [p],
            )?;
            for mut h in found {
                if !out.iter().any(|o| o.id == h.id) {
                    h.uses = true;
                    out.push(h);
                }
            }
        }
        Ok(out)
    }

    /// Entries that depend on an entry id or a path, through `related`, followed up to three links.
    pub fn uses(&self, id_or_path: &str) -> Result<Vec<Hit>, KbError> {
        let is_entry: bool = self
            .conn
            .query_row("SELECT 1 FROM entries WHERE id = ?1", [id_or_path], |_| {
                Ok(true)
            })
            .optional()?
            .unwrap_or(false);
        let (kind, target) = if is_entry {
            ("entry", id_or_path.to_string())
        } else {
            let p = id_or_path.strip_prefix("path:").unwrap_or(id_or_path);
            ("path", p.trim_end_matches('/').to_string())
        };
        let mut hits = self.collect(
            &format!(
                "WITH RECURSIVE r(id, depth) AS (\
                   SELECT from_id, 1 FROM links WHERE target_kind = ?1 AND rtrim(target, '/') = ?2 \
                   UNION \
                   SELECT l.from_id, r.depth + 1 FROM links l JOIN r ON l.target_kind = 'entry' AND l.target = r.id \
                   WHERE r.depth < 3) \
                 SELECT {COLS} FROM entries e JOIN (SELECT id, MIN(depth) AS d FROM r GROUP BY id) m ON m.id = e.id \
                 ORDER BY m.d, e.id"
            ),
            params![kind, target],
        )?;
        hits.retain(|h| !(is_entry && h.id == id_or_path));
        for h in &mut hits {
            h.uses = true;
        }
        Ok(hits)
    }

    /// Ids of entries that may say the same thing as `entry`: the same kind and path, or a title whose every word
    /// matches. The entry itself is left out.
    pub fn similar(&self, entry: &KbEntry) -> Result<Vec<String>, KbError> {
        let mut ids: Vec<String> = Vec::new();
        if let Some(path) = &entry.path {
            let mut stmt = self
                .conn
                .prepare("SELECT id FROM entries WHERE kind = ?1 AND path = ?2 ORDER BY id")?;
            for id in stmt.query_map(params![entry.kind.as_str(), path], |r| {
                r.get::<_, String>(0)
            })? {
                ids.push(id?);
            }
        }
        let groups: Vec<String> = entry
            .title
            .split_whitespace()
            .map(|word| {
                let parts: Vec<String> = split_identifier(word).iter().map(|t| quote(t)).collect();
                format!("({})", parts.join(" OR "))
            })
            .filter(|g| g != "()")
            .collect();
        if !groups.is_empty() {
            for h in self.match_query(&groups.join(" AND "), 50)? {
                ids.push(h.id);
            }
        }
        let mut seen: Vec<String> = Vec::new();
        for id in ids {
            if id != entry.id && !seen.contains(&id) {
                seen.push(id);
            }
        }
        Ok(seen)
    }

    /// Up to `limit` stale or missing entries as (id, path, reason).
    pub fn attention(&self, limit: usize) -> Result<Vec<(String, String, String)>, KbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, COALESCE(path, ''), reason FROM entries WHERE state != 'ok' ORDER BY state DESC, id LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn status(&self) -> Result<Status, KbError> {
        let count = |sql: &str, args: &[&dyn rusqlite::ToSql]| -> Result<usize, KbError> {
            Ok(self.conn.query_row(sql, args, |r| r.get::<_, i64>(0))? as usize)
        };
        let entries = count("SELECT COUNT(*) FROM entries", &[])?;
        let stale = count("SELECT COUNT(*) FROM entries WHERE state = 'stale'", &[])?;
        let missing = count("SELECT COUNT(*) FROM entries WHERE state = 'missing'", &[])?;
        let since = now() - QUERY_DAYS * 86_400;
        let searches_30d = count("SELECT COUNT(*) FROM queries WHERE at >= ?1", &[&since])?;
        let misses_30d = count(
            "SELECT COUNT(*) FROM queries WHERE at >= ?1 AND hits = 0",
            &[&since],
        )?;
        let invalid: Vec<(usize, String)> = self
            .meta("invalid")
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        let hint = if (searches_30d >= 50 && misses_30d * 5 >= searches_30d) || entries >= 5000 {
            "consider"
        } else {
            "not needed"
        };
        Ok(Status {
            state: if self.meta("file").as_deref() == Some("absent") {
                "missing"
            } else {
                "ok"
            }
            .to_string(),
            head: self.meta("head").unwrap_or_default(),
            entries,
            invalid,
            stale,
            missing,
            searches_30d,
            misses_30d,
            vectors_hint: hint.to_string(),
        })
    }
}

fn insert(tx: &rusqlite::Transaction<'_>, entry: &KbEntry, check: &Check) -> Result<(), KbError> {
    let join = |items: &[String]| items.join(" ");
    tx.execute(
        "INSERT INTO entries(id, kind, title, body, path, area, tags, aliases, related, verified_commit, state, reason) \
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            entry.id,
            entry.kind.as_str(),
            entry.title,
            entry.body,
            entry.path,
            entry.area,
            join(&entry.tags),
            join(&entry.aliases),
            join(&entry.related),
            entry.verified_commit,
            check.0,
            check.1
        ],
    )?;
    let rowid = tx.last_insert_rowid();
    let aliases = format!("{} {}", join(&entry.aliases), alias_words(entry).join(" "));
    tx.execute(
        "INSERT INTO entries_fts(rowid, title, body, tags, aliases) VALUES(?1, ?2, ?3, ?4, ?5)",
        params![rowid, entry.title, entry.body, join(&entry.tags), aliases],
    )?;
    for related in &entry.related {
        let (kind, target) = match related.strip_prefix("path:") {
            Some(path) => ("path", path),
            None => ("entry", related.as_str()),
        };
        tx.execute(
            "INSERT INTO links(from_id, target_kind, target) VALUES(?1, ?2, ?3)",
            params![entry.id, kind, target],
        )?;
    }
    Ok(())
}
