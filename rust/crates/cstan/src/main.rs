//! `cstan`: every command runs here; the commands only a process can do (the daemon, the restart helper, the watch loop,
//! the hand-over to `cstan-dash`) are done by this entry point.
use std::ffi::OsString;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cstan_front::watch::{fetch_status, watch_status};
use cstan_front::{run, Context, ExecPlan, Outcome, Serve, WatchPlan};

fn main() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let env: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => {
            eprintln!("cstan: {error}");
            std::process::exit(5);
        }
    };
    let cli_path = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("cstan"));
    let context = Context {
        args,
        env: env.clone(),
        cwd: cwd.clone(),
        now_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |d| d.as_millis() as f64),
        cli_path,
        terminal: None,
    };
    match run(&context) {
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
        Outcome::Exec { plan, stderr } => {
            let _ = std::io::stderr().write_all(&stderr);
            exec(&plan)
        }
        Outcome::Watch { plan, stderr } => {
            let _ = std::io::stderr().write_all(&stderr);
            watch(&plan);
            std::process::exit(0);
        }
        Outcome::Serve(Serve::Daemon) => {
            let vars = std::env::vars().collect();
            std::process::exit(capstan_daemon::cli::serve_cli(&cwd, &vars));
        }
        Outcome::Serve(Serve::RestartHelper(plan)) => {
            std::process::exit(capstan_daemon::cli::restart_helper(&plan));
        }
    }
}

/// Replaces this process with `cstan-dash`; when that cannot start, says so with the usage exit code.
fn exec(plan: &ExecPlan) -> ! {
    let error = Command::new(&plan.program)
        .args(&plan.args)
        .env_clear()
        .envs(plan.env.iter().map(|(k, v)| (k, v)))
        .exec();
    let _ = writeln!(
        std::io::stderr(),
        "cstan: {} ({})",
        cstan_front::dash::MISSING,
        error
    );
    std::process::exit(cstan_front::EXIT_USAGE);
}

fn watch(plan: &WatchPlan) {
    let mut stdout = std::io::stdout();
    watch_status(
        || fetch_status(plan),
        |text| {
            // A reader that has gone away ends the watch, as the write error ended it in Node.
            if stdout.write_all(text.as_bytes()).is_err() || stdout.flush().is_err() {
                std::process::exit(1);
            }
        },
        std::thread::sleep,
        Duration::from_secs(plan.interval_seconds),
        None,
    );
}
