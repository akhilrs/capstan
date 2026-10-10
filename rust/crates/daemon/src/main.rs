//! `cstan-daemon`: a thin binary over the entries `cstan daemon` and `cstan __restart-helper <plan>` call. It takes no
//! arguments besides the hidden `__restart-helper <plan>` that the Operator's restart starts detached.

use capstan_daemon::cli::{restart_helper, serve_cli};
use capstan_daemon::run::{EXIT_RUNTIME, EXIT_USAGE};
use std::path::Path;

const USAGE: &str = "usage: cstan-daemon";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("__restart-helper") {
        let Some(plan) = args.get(1) else {
            eprintln!("usage: cstan-daemon __restart-helper <plan.json>");
            std::process::exit(EXIT_USAGE);
        };
        std::process::exit(restart_helper(Path::new(plan)));
    }
    if !args.is_empty() {
        eprintln!("{USAGE}");
        std::process::exit(EXIT_USAGE);
    }
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => {
            eprintln!("cstan: {error}");
            std::process::exit(EXIT_RUNTIME);
        }
    };
    std::process::exit(serve_cli(&cwd, &std::env::vars().collect()));
}
