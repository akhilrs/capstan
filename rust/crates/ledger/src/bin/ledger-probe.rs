//! Dev-only probe used by test/ledger-rust-interop.test.ts:
//!   ledger-probe open <path>        open (migrate) the ledger
//!   ledger-probe open-ro <path>     open it read-only (a missing file is an error)
//!   ledger-probe write-values <path> open, then store a safe integer and a BLOB in `probe_values`
//!   ledger-probe lock-hold <path>   take the project lock, print {"ok":true,"held":true}, release on a stdin line or EOF
//!   ledger-probe lock-try <path>    take the project lock and release it at once
//! Prints one JSON object on stdout; exit 0 when "ok" is true, 1 when not, 2 on a usage error.
use capstan_ledger::{
    open_database, open_database_read_only, safe_integer, LedgerError, OpenOptions, ProjectLock,
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

fn run(args: &[String]) -> Result<i32, LedgerError> {
    let (command, path) = match args {
        [command, path] => (command.as_str(), Path::new(path)),
        _ => {
            eprintln!("usage: ledger-probe open|open-ro|write-values|lock-hold|lock-try <path>");
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
