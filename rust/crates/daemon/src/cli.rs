//! The entries behind `cstan daemon` and `cstan __restart-helper <plan>`: the `cstan` binary calls them, and the thin
//! `cstan-daemon` binary calls the same ones, so there is one code path.

use crate::ports::{cli_site, launch_disabled_in, wiring, WiringInput};
use crate::run::{announce_line, daemon_options, run_daemon_with, EXIT_RUNTIME};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

/// Runs the controller daemon of the project in `cwd` until it stops and returns the process's exit code. `env` is the
/// environment the daemon was started with (`CAPSTAN_LAUNCH`, `CSTAN_NODE_CLI`, `CSTAN_NODE`).
pub fn serve_cli(cwd: &Path, env: &HashMap<String, String>) -> i32 {
    let (mut options, warnings) = match daemon_options(cwd) {
        Ok(loaded) => loaded,
        Err(error) => return fail(error.exit_code, &error.message),
    };
    for warning in warnings {
        let line = serde_json::json!({
            "ts": capstan_ledger::now_iso(),
            "command": "daemon:config_warning",
            "detail": warning,
        });
        let _ = writeln!(std::io::stdout(), "{line}");
    }
    options.announce = Some(Arc::new(|event: &str, pid: u32| {
        let _ = writeln!(std::io::stdout(), "{}", announce_line(event, pid));
    }));
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => return fail(EXIT_RUNTIME, &error.to_string()),
    };
    // The real Herdr adapter, launcher, Operator and loops; CAPSTAN_LAUNCH=off keeps the daemon away from Herdr as in Node.
    let input = WiringInput {
        cli: cli_site(env, &executable),
        executable,
        launch: !launch_disabled_in(env.get("CAPSTAN_LAUNCH").map(String::as_str)),
    };
    let wired = match wiring(&options, input) {
        Ok(wired) => wired,
        Err(error) => return fail(error.exit_code, &error.message),
    };
    match run_daemon_with(options, wired) {
        Ok(()) => 0,
        Err(error) => fail(error.exit_code, &error.message),
    }
}

/// Runs the restart helper for the plan file `plan` (written by the Operator's restart) and returns its exit code: 0 the
/// new build answered, 1 rolled back, 2 down.
pub fn restart_helper(plan: &Path) -> i32 {
    let log = |message: &str| {
        let line = format!("{} {message}\n", capstan_ledger::now_iso());
        let mut out = std::io::stdout();
        let _ = out.write_all(line.as_bytes());
        let _ = out.flush();
    };
    capstan_operator::restart_helper::run_helper(plan, &log)
}

fn fail(code: i32, message: &str) -> i32 {
    eprintln!("cstan: {message}");
    code
}
