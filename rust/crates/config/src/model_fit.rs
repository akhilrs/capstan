//! Whether a model value is acceptable and fits a host kind. Config only: the launcher checks a model when it starts an
//! agent, and the daemon checks one when it accepts an override, both through these functions.

use crate::primitives::guard_credential_shape;
use crate::RoleConfig;

/// The most bytes a model value may have (the `capstan.toml` rule).
pub const MODEL_MAX_BYTES: usize = 100;

/// The model families of the host kinds that run one provider: (host kind, id prefixes, aliases). A new kind is one row.
pub const MODEL_FAMILIES: &[(&str, &[&str], &[&str])] = &[
    (
        "claude",
        &["claude-"],
        &["opus", "sonnet", "haiku", "fable", "opusplan"],
    ),
    ("codex", &["gpt-", "codex-", "o1", "o3", "o4"], &[]),
];

/// Host kinds that reach several providers: they take every valid value.
pub const MULTI_PROVIDER_HOSTS: &[&str] = &["omp", "pi"];

/// The `capstan.toml` model rules: 1 to 100 bytes, no leading dash, no control characters, no credential shape.
pub fn check_model_value(model: &str) -> Result<(), String> {
    if model.is_empty() || model.len() > MODEL_MAX_BYTES {
        return Err(format!(
            "a model must be 1 to {MODEL_MAX_BYTES} bytes, not {}",
            model.len()
        ));
    }
    if model.starts_with('-') {
        return Err("a model must not start with a dash".to_string());
    }
    if model.chars().any(char::is_control) {
        return Err("a model must not contain control characters".to_string());
    }
    guard_credential_shape(model, "model").map_err(|error| error.to_string())
}

/// The kind whose family claims `model`, if any.
fn family_of(model: &str) -> Option<&'static str> {
    let lower = model.to_ascii_lowercase();
    let alias = match lower.find('[') {
        Some(at) => &lower[..at],
        None => lower.as_str(),
    };
    MODEL_FAMILIES
        .iter()
        .find(|(_, prefixes, aliases)| {
            prefixes.iter().any(|prefix| lower.starts_with(prefix)) || aliases.contains(&alias)
        })
        .map(|(kind, _, _)| *kind)
}

/// Refuses a model that another host kind's family claims; takes an unclaimed one (the host reports unknown ids).
pub fn model_fits_host(host_kind: &str, model: &str) -> Result<(), String> {
    fit(host_kind, host_kind, model)
}

fn fit(host_name: &str, host_kind: &str, model: &str) -> Result<(), String> {
    if MULTI_PROVIDER_HOSTS.contains(&host_kind) {
        return Ok(());
    }
    match family_of(model) {
        Some(family) if family != host_kind => Err(format!(
            "model {model} is a {family} model and host {host_name} is {host_kind}; use a {host_kind} model"
        )),
        _ => Ok(()),
    }
}

/// `model_fits_host` for the host of the named role in the resolved configuration.
pub fn fit_for_role(config: &RoleConfig, role_name: &str, model: &str) -> Result<(), String> {
    let role = config
        .roles
        .iter()
        .find(|role| role.name == role_name)
        .ok_or_else(|| format!("the configuration has no role {role_name}"))?;
    let host = config
        .hosts
        .iter()
        .find(|host| host.name == role.host)
        .ok_or_else(|| format!("the configuration has no host {}", role.host))?;
    fit(&host.name, &host.kind, model)
}
