//! Dev-only probe used by test/ledger-rust-interop.test.ts:
//!   ledger-probe open <path>        open (migrate) the ledger
//!   ledger-probe open-ro <path>     open it read-only (a missing file is an error)
//!   ledger-probe write-values <path> open, then store a safe integer and a BLOB in `probe_values`
//!   ledger-probe lock-hold <path>   take the project lock, print {"ok":true,"held":true}, release on a stdin line or EOF
//!   ledger-probe lock-try <path>    take the project lock and release it at once
//!   ledger-probe bench <db-copy> [--cache-mb N] [--mmap-mb N] [--runs N]
//!                                  time fixed hot SELECTs on a COPY of a ledger (it is opened read-write, so a newer
//!                                  schema is applied to the copy); per query ms and, on Linux, the read syscalls
//!   ledger-probe bench-migrate <db-copy>
//!                                  time the pre-migration backup plus the pending migrations on a copy of the copy
//! Prints one JSON object on stdout; exit 0 when "ok" is true, 1 when not, 2 on a usage error.
use capstan_ledger::{
    iso_from_millis, now_millis, open_database, open_database_read_only, safe_integer, LedgerError,
    OpenOptions, ProjectLock,
};
use std::io::{BufRead, Write};
use std::path::Path;

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn kind(error: &LedgerError) -> &'static str {
    match error {
        LedgerError::Migration(_) => "migration",
        LedgerError::Ownership(_) => "ownership",
        LedgerError::ProjectLockHeld => "held",
        LedgerError::InvalidArgument(_) => "argument",
        LedgerError::UnsafeValue(_) => "unsafe-value",
        LedgerError::Io(_) => "io",
        LedgerError::Sqlite(_) => "sqlite",
    }
}

fn failure(error: &LedgerError) -> i32 {
    println!(
        "{{\"ok\":false,\"kind\":{},\"error\":{}}}",
        json_string(kind(error)),
        json_string(&error.to_string())
    );
    1
}

fn write_values(path: &Path) -> Result<String, LedgerError> {
    let db = open_database(path, &OpenOptions::default())?;
    db.exec("CREATE TABLE IF NOT EXISTS probe_values (id INTEGER PRIMARY KEY, n INTEGER NOT NULL, b BLOB NOT NULL)")?;
    let rejected = safe_integer(9_007_199_254_740_992).is_err();
    db.prepare("INSERT OR REPLACE INTO probe_values (id, n, b) VALUES (1, ?, ?)")?
        .execute(rusqlite::params![
            safe_integer(9_007_199_254_740_991)?,
            vec![0u8, 1, 255]
        ])?;
    db.close()?;
    Ok(format!("{{\"ok\":true,\"unsafe_rejected\":{rejected}}}"))
}

/// Read syscalls of this process so far (`syscr` of /proc/self/io); None off Linux.
fn read_syscalls() -> Option<u64> {
    let io = std::fs::read_to_string("/proc/self/io").ok()?;
    io.lines()
        .find_map(|line| line.strip_prefix("syscr:"))
        .and_then(|n| n.trim().parse().ok())
}

fn count(db: &capstan_ledger::Database, sql: &str) -> Result<i64, LedgerError> {
    Ok(db.connection().query_row(sql, [], |r| r.get(0))?)
}

fn first_text(db: &capstan_ledger::Database, sql: &str) -> Option<String> {
    db.connection().query_row(sql, [], |r| r.get(0)).ok()
}

/// Steps `sql` to the end `runs` times; returns the milliseconds of each run and the rows of the last.
fn time_query(
    db: &capstan_ledger::Database,
    sql: &str,
    params: &[&dyn rusqlite::ToSql],
    runs: usize,
) -> Result<(Vec<f64>, usize), LedgerError> {
    let mut times = Vec::new();
    let mut rows_seen = 0;
    for _ in 0..runs {
        let started = std::time::Instant::now();
        let mut stmt = db.prepare(sql)?;
        let mut rows = stmt.query(params)?;
        rows_seen = 0;
        while rows.next()?.is_some() {
            rows_seen += 1;
        }
        times.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    Ok((times, rows_seen))
}

fn bench(path: &Path, rest: &[String]) -> Result<String, LedgerError> {
    let mut options = OpenOptions::default();
    let mut runs = 5usize;
    let mut args = rest.iter();
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .and_then(|v| v.parse::<u64>().ok())
            .ok_or_else(|| LedgerError::InvalidArgument(format!("{flag} needs a number")))?;
        match flag.as_str() {
            "--cache-mb" => options.cache_mb = value,
            "--mmap-mb" => options.mmap_mb = value,
            "--runs" => runs = value.max(1) as usize,
            other => {
                return Err(LedgerError::InvalidArgument(format!(
                    "unknown option {other}"
                )))
            }
        }
    }
    let db = open_database(path, &options)?;
    let project = first_text(
        &db,
        "SELECT project_id FROM projects ORDER BY created_at LIMIT 1",
    )
    .unwrap_or_default();
    let agent = first_text(
        &db,
        "SELECT entity_id FROM controller_events WHERE entity_type = 'agent' AND to_state = 'lost' LIMIT 1",
    )
    .or_else(|| first_text(&db, "SELECT entity_id FROM controller_events WHERE entity_type = 'agent' LIMIT 1"))
    .unwrap_or_default();
    let reviewer =
        first_text(&db, "SELECT reviewer_agent_id FROM reviews LIMIT 1").unwrap_or_default();
    let cutoff = iso_from_millis(now_millis() - 7 * 86_400_000);
    let generation = 1i64;
    let limit = 1000i64;
    let queries: [(&str, &str, Vec<&dyn rusqlite::ToSql>); 3] = [
        (
            "lost_lookup",
            "SELECT 1 AS present FROM controller_events WHERE project_id = ? AND entity_type = 'agent' AND entity_id = ?
               AND to_state = 'lost' AND json_extract(payload_json, '$.details.generation') = ?",
            vec![&project, &agent, &generation],
        ),
        (
            "warm_chain",
            "SELECT * FROM reviews WHERE project_id = ? AND reviewer_agent_id = ? AND state IN ('passed', 'findings')
               AND subject_plan_id IS NULL ORDER BY sequence",
            vec![&project, &reviewer],
        ),
        (
            "retention_select",
            "SELECT idempotency_key FROM mutation_requests WHERE project_id = ?1 AND created_at < ?2 LIMIT ?3",
            vec![&project, &cutoff, &limit],
        ),
    ];
    let mut out = Vec::new();
    for (name, sql, params) in &queries {
        let before = read_syscalls();
        let (times, rows) = time_query(&db, sql, params, runs)?;
        let delta = match (before, read_syscalls()) {
            (Some(a), Some(b)) => b.saturating_sub(a).to_string(),
            _ => "null".to_string(),
        };
        let first = times[0];
        let rest_avg = if times.len() > 1 {
            times[1..].iter().sum::<f64>() / (times.len() - 1) as f64
        } else {
            first
        };
        out.push(format!(
            "{{\"query\":{},\"rows\":{rows},\"first_ms\":{first:.3},\"warm_avg_ms\":{rest_avg:.3},\"syscr_delta\":{delta}}}",
            json_string(name)
        ));
    }
    let pragma = |name: &str| {
        db.pragma(name)
            .map(|r| format!("{:?}", r[0][0]))
            .unwrap_or_default()
    };
    let result = format!(
        "{{\"ok\":true,\"cache_size\":{},\"mmap_size\":{},\"runs\":{runs},\"queries\":[{}]}}",
        json_string(&pragma("cache_size")),
        json_string(&pragma("mmap_size")),
        out.join(",")
    );
    db.close()?;
    Ok(result)
}

fn bench_migrate(path: &Path) -> Result<String, LedgerError> {
    let size = std::fs::metadata(path)?.len();
    let directory = std::env::temp_dir().join(format!("ledger-probe-{}", std::process::id()));
    std::fs::create_dir_all(&directory)?;
    let work = directory.join("controller.sqlite");
    let started = std::time::Instant::now();
    std::fs::copy(path, &work)?;
    let copy_ms = started.elapsed().as_secs_f64() * 1000.0;
    // Opening applies every pending migration; each one first takes an online backup of the whole file.
    let started = std::time::Instant::now();
    let db = open_database(&work, &OpenOptions::default())?;
    let migrate_ms = started.elapsed().as_secs_f64() * 1000.0;
    let version = count(&db, "SELECT MAX(version) FROM schema_migrations")?;
    let events = count(&db, "SELECT COUNT(*) FROM controller_events")?;
    let messages = count(&db, "SELECT COUNT(*) FROM messages")?;
    let backups = std::fs::read_dir(&directory)?
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().contains(".pre-v"))
        .count();
    db.close()?;
    let _ = std::fs::remove_dir_all(&directory);
    Ok(format!(
        "{{\"ok\":true,\"size_bytes\":{size},\"controller_events\":{events},\"messages\":{messages},\"schema_version\":{version},\"file_copy_ms\":{copy_ms:.1},\"open_and_migrate_ms\":{migrate_ms:.1},\"backups_made\":{backups}}}"
    ))
}

fn run(args: &[String]) -> Result<i32, LedgerError> {
    if let [command, path, rest @ ..] = args {
        match command.as_str() {
            "bench" => {
                println!("{}", bench(Path::new(path), rest)?);
                return Ok(0);
            }
            "bench-migrate" if rest.is_empty() => {
                println!("{}", bench_migrate(Path::new(path))?);
                return Ok(0);
            }
            _ => {}
        }
    }
    let (command, path) = match args {
        [command, path] => (command.as_str(), Path::new(path)),
        _ => {
            eprintln!("usage: ledger-probe open|open-ro|write-values|lock-hold|lock-try <path> | bench <path> [options] | bench-migrate <path>");
            return Ok(2);
        }
    };
    match command {
        "open" => {
            open_database(path, &OpenOptions::default())?.close()?;
            println!("{{\"ok\":true}}");
        }
        "open-ro" => {
            open_database_read_only(path)?.close()?;
            println!("{{\"ok\":true}}");
        }
        "write-values" => println!("{}", write_values(path)?),
        "lock-hold" | "lock-try" => {
            let mut lock = ProjectLock::acquire(path)?;
            lock.assert_held()?;
            if command == "lock-hold" {
                println!("{{\"ok\":true,\"held\":true}}");
                std::io::stdout().flush()?;
                let mut line = String::new();
                let _ = std::io::stdin().lock().read_line(&mut line);
                lock.assert_held()?;
            } else {
                println!("{{\"ok\":true}}");
            }
            lock.close();
            if command == "lock-hold" {
                println!("{{\"ok\":true,\"released\":true}}");
            }
        }
        _ => {
            eprintln!("ledger-probe: unknown command {command}");
            return Ok(2);
        }
    }
    Ok(0)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match run(&args) {
        Ok(code) => code,
        Err(error) => failure(&error),
    };
    let _ = std::io::stdout().flush();
    std::process::exit(code);
}
