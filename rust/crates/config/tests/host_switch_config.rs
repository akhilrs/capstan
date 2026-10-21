//! Host-switchable roles: changing only a role's `host` loads, with the model and permission mode of that host.
use capstan_config::{parse_config, ConfigError, RoleConfig};
use std::path::Path;

const HOSTS: &str = r#"schema_version = 1
[nexora]
track = "never"
[hosts.claude]
kind = "claude"
[hosts.codex]
kind = "codex"
model = "gpt-x"
[hosts.omp]
kind = "omp"
model = "o/m"
[hosts.pi]
kind = "pi"
model = "p/m"
[roles.pm]
kind = "PM"
host = "claude"
"#;

fn load(text: &str) -> Result<RoleConfig, String> {
    parse_config(text.as_bytes(), Path::new("/nonexistent")).map_err(|error| match error {
        ConfigError::Invalid(message) => message,
        other => format!("{other:?}"),
    })
}

fn role<'a>(config: &'a RoleConfig, name: &str) -> &'a capstan_config::Role {
    config.roles.iter().find(|r| r.name == name).expect("role")
}

#[test]
fn changing_only_host_loads_on_every_host() {
    for (host, model, source) in [
        ("claude", "claude-def", "defaults"),
        ("codex", "gpt-x", "host"),
        ("omp", "o/m", "host"),
        ("pi", "p/m", "host"),
    ] {
        let text = format!(
            "{HOSTS}[defaults.Verifier]\nmodel = \"claude-def\"\n[roles.tester]\nkind = \"Verifier\"\nhost = \"{host}\"\npermission_mode = \"acceptEdits\"\nallow = [\"Bash(git *)\"]\ndeny = [\"Write\"]\n"
        );
        let config = load(&text).unwrap_or_else(|e| panic!("{host}: {e}"));
        let tester = role(&config, "tester");
        assert_eq!(tester.model.as_deref(), Some(model), "{host}");
        assert_eq!(tester.model_source, source, "{host}");
        let rules = config
            .warnings
            .iter()
            .filter(|w| w.contains("allow and deny"))
            .collect::<Vec<_>>();
        if host == "claude" {
            assert_eq!(tester.allow, ["Bash(git *)"]);
            assert_eq!(tester.deny, ["Write"]);
            assert!(rules.is_empty());
        } else {
            assert!(tester.allow.is_empty() && tester.deny.is_empty(), "{host}");
            assert_eq!(
                rules,
                [&format!(
                    "roles.tester: allow and deny are Claude Code tool rules that host {host} ({host}) cannot enforce; they are ignored"
                )]
            );
        }
    }
}

#[test]
fn model_precedence_is_role_then_host_then_kind_defaults_then_defaults() {
    let build = |defaults: &str, host_model: &str, role_model: &str, host: &str| {
        let text = format!(
            "schema_version = 1\n[nexora]\ntrack = \"never\"\n{defaults}[hosts.claude]\nkind = \"claude\"\n[hosts.codex]\nkind = \"codex\"\n{host_model}[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n[roles.dev]\nkind = \"Developer\"\nhost = \"{host}\"\npermission_mode = \"acceptEdits\"\n{role_model}"
        );
        let config = load(&text).expect("loads");
        let dev = role(&config, "dev");
        (dev.model.clone(), dev.model_source)
    };
    let defaults = "[defaults]\nmodel = \"all\"\n[defaults.Developer]\nmodel = \"kind\"\n";
    for host in ["claude", "codex"] {
        let hm = if host == "claude" {
            ""
        } else {
            "model = \"host\"\n"
        };
        assert_eq!(
            build(defaults, hm, "model = \"role\"\n", host),
            (Some("role".into()), "role")
        );
        let expected = if host == "claude" {
            ("kind", "defaults")
        } else {
            ("host", "host")
        };
        assert_eq!(
            build(defaults, hm, "", host),
            (Some(expected.0.into()), expected.1)
        );
        assert_eq!(
            build("[defaults]\nmodel = \"all\"\n", hm, "", host)
                .0
                .as_deref(),
            Some(if host == "claude" { "all" } else { "host" })
        );
    }
    // [hosts.claude] model beats [defaults.Developer].
    let text = String::from(
        "schema_version = 1\n[nexora]\ntrack = \"never\"\n[defaults.Developer]\nmodel = \"kind\"\n[hosts.claude]\nkind = \"claude\"\nmodel = \"claude-host\"\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n[roles.dev]\nkind = \"Developer\"\nhost = \"claude\"\n"
    );
    assert_eq!(
        role(&load(&text).unwrap(), "dev").model.as_deref(),
        Some("claude-host")
    );
}

#[test]
fn no_model_anywhere_on_a_codex_role_loads_with_source_none() {
    let text = "schema_version = 1\n[nexora]\ntrack = \"never\"\n[hosts.claude]\nkind = \"claude\"\n[hosts.codex]\nkind = \"codex\"\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n[roles.dev]\nkind = \"Developer\"\nhost = \"codex\"\n";
    let config = load(text).expect("loads");
    let dev = role(&config, "dev");
    assert_eq!((dev.model.clone(), dev.model_source), (None, "none"));
    assert_eq!(dev.permission_mode, "auto");
    let with_defaults = format!("{text}[defaults.Developer]\nmodel = \"gpt-d\"\n");
    let config = load(&with_defaults).expect("loads");
    let dev = role(&config, "dev");
    assert_eq!(
        (dev.model.as_deref(), dev.model_source),
        (Some("gpt-d"), "defaults")
    );
}

#[test]
fn permission_mode_rules() {
    let base =
        "schema_version = 1\n[nexora]\ntrack = \"never\"\n[hosts.claude]\nkind = \"claude\"\n";
    let pm = "[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n";
    let err = load(&format!(
        "{base}[hosts.codex]\nkind = \"codex\"\npermission_mode = \"plan\"\n{pm}"
    ))
    .unwrap_err();
    assert_eq!(
        err,
        "hosts.codex.permission_mode must be acceptEdits or auto on a codex host, which runs unattended with full access"
    );
    let ok = load(&format!(
        "{base}[hosts.codex]\nkind = \"codex\"\npermission_mode = \"acceptEdits\"\n{pm}[roles.a]\nkind = \"Developer\"\nhost = \"codex\"\n[roles.b]\nkind = \"Developer\"\nhost = \"codex\"\npermission_mode = \"auto\"\n"
    ))
    .unwrap();
    assert_eq!(role(&ok, "a").permission_mode, "acceptEdits");
    assert_eq!(role(&ok, "b").permission_mode, "auto");
    // A role or [defaults] writing default or plan on codex keeps today's error.
    let today = "roles.a.permission_mode must be acceptEdits or auto on host codex (codex), which runs unattended with full access (set it on the role or in [defaults])";
    for mode in ["default", "plan"] {
        let role_written = format!("{base}[hosts.codex]\nkind = \"codex\"\n{pm}[roles.a]\nkind = \"Developer\"\nhost = \"codex\"\npermission_mode = \"{mode}\"\n");
        assert_eq!(load(&role_written).unwrap_err(), today);
        let defaults_written = format!("{base}[defaults]\npermission_mode = \"{mode}\"\n[hosts.codex]\nkind = \"codex\"\n{pm}[roles.a]\nkind = \"Developer\"\nhost = \"codex\"\n");
        assert_eq!(load(&defaults_written).unwrap_err(), today);
    }
}

#[test]
fn mcp_and_hooks_on_a_pi_role_warn_and_are_ignored() {
    let text = format!(
        "{HOSTS}[mcp_servers.playwright]\ncommand = \"npx\"\nargs = [\"-y\", \"@playwright/mcp@0.0.83\"]\n[roles.w]\nkind = \"Developer\"\nhost = \"pi\"\nmcp = [\"playwright\"]\nhooks = \"inherit\"\n"
    );
    let config = load(&text).expect("loads");
    let w = role(&config, "w");
    assert!(w.mcp.is_empty());
    assert_eq!(w.hooks, "inherit");
    assert!(config.warnings.contains(
        &"roles.w.mcp is a Claude Code option that host pi (pi) cannot use; it is ignored"
            .to_string()
    ));
    assert!(config.warnings.contains(&"roles.w.hooks = \"inherit\" is a Claude Code option that host pi (pi) cannot use; it is ignored".to_string()));
}

/// Recorded on the base commit (before this change) for the same TOML: a codex role with hooks = "inherit" only.
#[test]
fn a_hooks_inherit_role_keeps_its_hash() {
    let text = "schema_version = 1\n[nexora]\ntrack = \"never\"\n[hosts.claude]\nkind = \"claude\"\n[hosts.codex]\nkind = \"codex\"\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n[roles.dev]\nkind = \"Developer\"\nhost = \"codex\"\npermission_mode = \"acceptEdits\"\nhooks = \"inherit\"\n";
    let config = load(text).expect("loads");
    assert_eq!(
        role(&config, "dev").config_hash,
        "7dcde1232ae3b6d44e1049e4a52a74c6b14255a7841072fdaaea05f713c35ffd"
    );
    assert_eq!(
        role(&config, "pm").config_hash,
        "c7bf880a8c6b8da1bc46cddf22866ce60c0d7d37e2ed6e76f9adcb4db619dc64"
    );
}

#[test]
fn pm_and_supervisor_on_codex_keep_their_errors() {
    for kind in ["PM", "Supervisor"] {
        let text = format!(
            "schema_version = 1\n[nexora]\ntrack = \"never\"\n[hosts.claude]\nkind = \"claude\"\n[hosts.codex]\nkind = \"codex\"\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n[roles.x]\nkind = \"{kind}\"\nhost = \"codex\"\npermission_mode = \"acceptEdits\"\n"
        );
        let err = load(&text).unwrap_err();
        assert!(
            err.contains("is read-only by design and only a claude host can enforce that"),
            "{kind}: {err}"
        );
    }
}

#[test]
fn host_model_and_mode_reach_the_json_only_when_written() {
    let config = load(&format!(
        "{HOSTS}[roles.d]\nkind = \"Developer\"\nhost = \"pi\"\n"
    ))
    .unwrap();
    let json = config.to_json();
    assert!(json.contains("\"model\": \"p/m\""));
    assert!(!json.contains("model_source") && !json.contains("modelSource"));
}
