//! `cstan trust`: shows what changed in the checked keys of `capstan.toml` since it was last trusted on this machine and
//! records it. It never prompts: the printed list is the review.
use capstan_config::{
    checked_subset, read_config_pair, record_trust, trust_status_with, KeyChange, TrustChange,
    TrustError, TrustStatus,
};
use serde_json::{json, Value as Json};

use crate::agent::var;
use crate::{invalid, output, parse_options, take_flag, to_js, Cli, Fail, Flow};

const SET_DIR: &str = "Set CAPSTAN_TRUST_DIR to an absolute directory.";
const NOT_FOR_AGENTS: &str =
    "cstan trust is for the user: run it in your own terminal, not from an agent.";

/// The vocabulary of `cstan trust --json` and the `trust` member of `cstan status`.
pub fn state_name(status: &TrustStatus) -> &'static str {
    match status {
        TrustStatus::Trusted => "trusted",
        TrustStatus::NotNeeded => "not_needed",
        TrustStatus::Needed { .. } => "needed",
        TrustStatus::Unavailable { .. } => "unavailable",
    }
}

fn change_line(change: &KeyChange) -> String {
    let old = change.old.as_deref().unwrap_or_default();
    let new = change.new.as_deref().unwrap_or_default();
    match change.change {
        TrustChange::Added => format!("+ {} = {new}", change.key),
        TrustChange::Changed => format!("~ {}: {old} -> {new}", change.key),
        TrustChange::Removed => format!("- {} (was {old})", change.key),
    }
}

fn change_json(change: &KeyChange) -> Json {
    let name = match change.change {
        TrustChange::Added => "added",
        TrustChange::Changed => "changed",
        TrustChange::Removed => "removed",
    };
    json!({"key": change.key, "change": name, "old": change.old, "new": change.new})
}

impl Cli<'_> {
    /// The trust status of the working directory against the store of this run.
    pub fn trust_status(&self) -> TrustStatus {
        trust_status_with(&self.ctx.cwd, &self.ctx.trust.store(&self.ctx.env))
    }

    /// The refusal of a configuration that is not trusted (Needed or Unavailable), in the gate's own words; `None` when
    /// the configuration may be loaded.
    pub fn trust_refusal(&self) -> Option<Fail> {
        match self.trust_status() {
            TrustStatus::Trusted | TrustStatus::NotNeeded => None,
            _ => capstan_config::load_role_config_with(
                &self.ctx.cwd,
                &self.ctx.trust.store(&self.ctx.env),
            )
            .err()
            .map(crate::config_cmd::config_fail),
        }
    }

    /// `cstan trust [--check] [--json]`.
    pub fn trust(&mut self, rest: &[String]) -> Result<Flow, Fail> {
        self.trust_with_hook(rest, &mut || {})
    }

    /// `trust`, with `after_show` called between showing the changes and recording them (a test changes the file there).
    fn trust_with_hook(
        &mut self,
        rest: &[String],
        after_show: &mut dyn FnMut(),
    ) -> Result<Flow, Fail> {
        let mut flags = rest.to_vec();
        let check = take_flag(&mut flags, "--check");
        let (positional, json) = parse_options(&flags);
        if !positional.is_empty() {
            return Err(Fail::Usage);
        }
        if var(&self.ctx.env, "CAPSTAN_TOKEN").is_some()
            || var(&self.ctx.env, "CAPSTAN_SOCKET").is_some()
        {
            return Err(invalid(NOT_FOR_AGENTS));
        }
        let store = self.ctx.trust.store(&self.ctx.env);
        let cwd = self.ctx.cwd.clone();
        let status = trust_status_with(&cwd, &store);
        let state = state_name(&status);
        let (changes, store_dir): (&[KeyChange], Option<String>) = match &status {
            TrustStatus::Needed {
                changes, store_dir, ..
            } => (changes, Some(store_dir.display().to_string())),
            TrustStatus::Unavailable { store_dir, .. } => {
                (&[], store_dir.as_ref().map(|dir| dir.display().to_string()))
            }
            _ => (&[], None),
        };
        let report = |recorded: bool| {
            json!({
                "state": state,
                "storeDir": store_dir,
                "changes": changes.iter().map(change_json).collect::<Vec<_>>(),
                "recorded": recorded,
            })
        };
        match &status {
            TrustStatus::Trusted | TrustStatus::NotNeeded => {
                if json {
                    output(&mut self.io.out, Some(&to_js(&report(false))), true);
                } else {
                    self.io.out_line("capstan.toml is trusted on this machine.");
                }
                Ok(Flow::Code(0))
            }
            TrustStatus::Unavailable { reason, .. } => {
                if json {
                    output(&mut self.io.out, Some(&to_js(&report(false))), true);
                    Ok(Flow::Code(crate::jsops::EXIT_INVALID))
                } else {
                    Err(invalid(format!("{reason}. {SET_DIR}")))
                }
            }
            TrustStatus::Needed {
                digest, store_dir, ..
            } => {
                if !json {
                    for change in changes {
                        self.io.out_line(&change_line(change));
                    }
                }
                after_show();
                if check {
                    if json {
                        output(&mut self.io.out, Some(&to_js(&report(false))), true);
                        return Ok(Flow::Code(crate::jsops::EXIT_INVALID));
                    }
                    return Err(invalid("Not trusted. Run cstan trust to trust it."));
                }
                match record_trust(&cwd, digest, &store) {
                    Ok(_) => {}
                    Err(TrustError::DigestChanged) => {
                        return Err(invalid(
                            "capstan.toml changed while it was shown; run cstan trust again.",
                        ))
                    }
                    Err(TrustError::Unavailable(reason)) => {
                        return Err(invalid(format!("{reason}. {SET_DIR}")))
                    }
                    Err(error) => return Err(invalid(error.to_string())),
                }
                if json {
                    output(&mut self.io.out, Some(&to_js(&report(true))), true);
                } else {
                    let keys = read_config_pair(&cwd)
                        .ok()
                        .flatten()
                        .and_then(|pair| checked_subset(&pair).ok())
                        .map_or(changes.len(), |subset| subset.len());
                    self.io.out_line(&format!(
                        "Trusted {keys} key(s) of capstan.toml on this machine ({}).",
                        store_dir.display()
                    ));
                }
                Ok(Flow::Code(0))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use capstan_config::TrustStore;

    use super::*;
    use crate::Context;

    #[test]
    fn a_file_changed_after_it_was_shown_is_not_recorded() {
        let temp = capstan_testkit::short_temp_root()
            .join(format!("capstan-trust-hook-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        let project = temp.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let file = project.join("capstan.toml");
        let write = |text: &str| {
            std::fs::write(&file, text).unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        };
        write("[cache]\nmax_size_gb = 5\n");
        let store = TrustStore::at(temp.join("store"));
        let ctx = Context {
            args: Vec::new(),
            env: Vec::new(),
            cwd: project.clone(),
            now_ms: 0.0,
            cli_path: std::path::PathBuf::new(),
            terminal: Some(false),
            trust: crate::trust_seam::TrustSeam {
                store: Some(store.clone()),
            },
        };
        let mut cli = Cli {
            ctx: &ctx,
            io: crate::Io::default(),
            foreign_warned: false,
            status_all: false,
        };
        let result = cli.trust_with_hook(&[], &mut || write("[cache]\nmax_size_gb = 9\n"));
        match result {
            Err(Fail::Error(error)) => assert_eq!(
                error.message.to_utf8_lossy(),
                "capstan.toml changed while it was shown; run cstan trust again."
            ),
            _ => panic!("expected a refusal"),
        }
        assert!(matches!(
            trust_status_with(&project, &store),
            TrustStatus::Needed { .. }
        ));
        assert!(!temp.join("store").exists());
        let _ = std::fs::remove_dir_all(&temp);
    }
}
