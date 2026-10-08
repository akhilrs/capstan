//! `cstan config check`: loads `capstan.toml` natively and prints what the Node CLI prints (`src/cli.ts`, the `config`
//! branch). Only a load that succeeds, or fails with one of the Node loader's own refusal texts, is answered here. Anything
//! else goes to Node, which words it: a TOML syntax error (Node names a line and column the Rust parser cannot reproduce),
//! a text the two TOML parsers might read differently (see `capstan_config::node_refuses`), a file that cannot be read, and a
//! project with no `capstan.toml` at all. Nothing in this module runs unless the command is exactly `config check`.
use capstan_config::{load_role_config, ConfigError};

use crate::jsops::EXIT_INVALID;
use crate::{Context, Fallback, Outcome};

/// The one refusal the front end leaves to Node although it is the Node loader's own text: a missing file is the case
/// most often hit outside a project, and Node's reading of it stays authoritative.
const MISSING: &str = "capstan.toml does not exist";

pub fn run(context: &Context) -> Outcome {
    if context.args.len() != 2 || context.args[1] != "check" {
        return Outcome::Fallback(Fallback::NotNative);
    }
    match load_role_config(&context.cwd) {
        Ok(config) => {
            let stdout = format!("{}\n", config.to_json()).into_bytes();
            let mut stderr = String::new();
            for warning in &config.warnings {
                stderr.push_str(&format!("warning: {warning}\n"));
            }
            for role in &config.roles {
                let host = config.hosts.iter().find(|host| host.name == role.host);
                if let Some(host) = host.filter(|host| host.kind != "claude") {
                    stderr.push_str(&format!(
                        "warning: role {} runs on {} with full access and no approval prompts; nothing blocks it from editing outside its worktree or from pushing\n",
                        role.name, host.kind
                    ));
                }
            }
            Outcome::Done {
                stdout,
                stderr: stderr.into_bytes(),
                exit: 0,
            }
        }
        Err(ConfigError::Invalid(message)) if message != MISSING => Outcome::Done {
            stdout: Vec::new(),
            stderr: format!("cstan: {message}\n").into_bytes(),
            exit: EXIT_INVALID,
        },
        Err(_) => Outcome::Fallback(Fallback::ConfigDeferred),
    }
}
