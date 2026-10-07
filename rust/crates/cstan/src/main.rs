//! `cstan`: the native commands run here; anything else replaces this process with the Node CLI.
use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use cstan_front::{run, Context, Outcome};

const NOT_FOUND: &str =
    "cstan: the Node implementation was not found (install cstan-node beside cstan or set CSTAN_NODE_CLI)\n";

fn main() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let env: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    let outcome = match std::env::current_dir() {
        Ok(cwd) => run(&Context {
            args: args.clone(),
            env: env.clone(),
            cwd,
            now_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0.0, |d| d.as_millis() as f64),
        }),
        Err(_) => Outcome::Fallback(cstan_front::Fallback::NotNative),
    };
    match outcome {
        Outcome::Done {
            stdout,
            stderr,
            exit,
        } => {
            let _ = std::io::stdout().write_all(&stdout);
            let _ = std::io::stdout().flush();
            let _ = std::io::stderr().write_all(&stderr);
            std::process::exit(exit);
        }
        Outcome::Fallback(_) => hand_to_node(&args, &env),
    }
}

fn var<'a>(env: &'a [(OsString, OsString)], name: &str) -> Option<&'a OsStr> {
    cstan_front::agent::var(env, name)
}

/// Replaces this process with the Node CLI, same arguments and environment: `CSTAN_NODE_CLI` (a `.js` or `.mjs` file runs
/// under `CSTAN_NODE` or `node`; anything else runs as it is), then `cstan-node` beside this executable. A candidate that
/// is this executable is skipped, so the hand-over cannot loop.
fn hand_to_node(args: &[OsString], env: &[(OsString, OsString)]) -> ! {
    let me = std::env::current_exe()
        .ok()
        .and_then(|path| std::fs::canonicalize(path).ok());
    let is_me = |candidate: &Path| match (&me, std::fs::canonicalize(candidate)) {
        (Some(me), Ok(real)) => *me == real,
        _ => false,
    };
    if let Some(cli) = var(env, "CSTAN_NODE_CLI").map(PathBuf::from) {
        if cli.is_absolute() && !is_me(&cli) {
            let script = matches!(cli.extension().and_then(OsStr::to_str), Some("js" | "mjs"));
            let error = if script {
                let node = var(env, "CSTAN_NODE")
                    .map(PathBuf::from)
                    .filter(|p| p.is_absolute())
                    .unwrap_or_else(|| PathBuf::from("node"));
                Command::new(node).arg(&cli).args(args).exec()
            } else {
                Command::new(&cli).args(args).exec()
            };
            fail_to_start(&cli, &error);
        }
    }
    if let Some(sibling) = me
        .as_ref()
        .and_then(|me| me.parent())
        .map(|dir| dir.join("cstan-node"))
        .filter(|path| path.exists() && !is_me(path))
    {
        let error = Command::new(&sibling).args(args).exec();
        fail_to_start(&sibling, &error);
    }
    let _ = std::io::stderr().write_all(NOT_FOUND.as_bytes());
    std::process::exit(5);
}

fn fail_to_start(path: &Path, error: &std::io::Error) -> ! {
    let _ = writeln!(
        std::io::stderr(),
        "cstan: cannot run {}: {error}",
        path.display()
    );
    std::process::exit(5);
}
