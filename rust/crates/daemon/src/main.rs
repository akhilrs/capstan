//! `cstan-daemon`: the controller daemon of one project (the working directory), as `cstan daemon` is in Node. It takes no
//! arguments; the hidden `__restart-helper <plan>` runs the restart helper that the Operator's restart starts detached.

use capstan_daemon::run::{announce_line, daemon_options, run_daemon, EXIT_USAGE};
use std::io::Write;
use std::sync::Arc;

const USAGE: &str = "usage: cstan-daemon";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("__restart-helper") {
        // Hidden: the detached restart helper, never typed by a person.
        std::process::exit(capstan_operator::restart_helper::run(&args[1..]));
    }
    if !args.is_empty() {
        eprintln!("{USAGE}");
        std::process::exit(EXIT_USAGE);
    }
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => fail(5, &error.to_string()),
    };
    let (mut options, warnings) = match daemon_options(&cwd) {
        Ok(loaded) => loaded,
        Err(error) => fail(error.exit_code, &error.message),
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
    if let Err(error) = run_daemon(options) {
        fail(error.exit_code, &error.message);
    }
}

fn fail(code: i32, message: &str) -> ! {
    eprintln!("cstan: {message}");
    std::process::exit(code);
}
