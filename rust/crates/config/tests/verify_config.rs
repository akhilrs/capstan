//! The `[verify]` table: defaults per key, every key, refusals that name the key, and `commands_sha`.
use capstan_config::{parse_config, ConfigError, RoleConfig};
use std::path::Path;

const BASE: &str = r#"schema_version = 1
[nexora]
track = "never"
[hosts.claude]
kind = "claude"
[roles.pm]
kind = "PM"
host = "claude"
[roles.dev]
kind = "Developer"
host = "claude"
"#;

fn load(extra: &str) -> Result<RoleConfig, String> {
    parse_config(
        format!("{BASE}{extra}").as_bytes(),
        Path::new("/nonexistent"),
    )
    .map_err(|error| match error {
        ConfigError::Invalid(message) => message,
        other => format!("{other:?}"),
    })
}

fn verify(extra: &str) -> capstan_config::Verify {
    load(&format!("[verify]\ncommands = [\"true\"]\n{extra}"))
        .expect("loads")
        .verify
        .expect("present")
}

fn refusal(extra: &str) -> String {
    load(extra).expect_err("refused")
}

#[test]
fn absent_section_is_none_and_not_printed() {
    let config = load("").expect("loads");
    assert!(config.verify.is_none());
    assert!(!config.to_json().contains("\"verify\""));
}

#[test]
fn every_default_applies() {
    let v = verify("");
    assert_eq!(v.commands, vec!["true".to_string()]);
    assert!(v.env.is_empty());
    assert_eq!(v.timeout_seconds, 3600);
    assert_eq!(v.max_parallel, 1);
    assert!(!v.auto_report && !v.auto_integration);
    assert!(!v.require_review && !v.require_integrate);
    assert_eq!(v.output_tail_bytes, 8192);
    assert_eq!(v.max_log_bytes, 4_194_304);
    assert_eq!(v.keep_logs, 20);
    assert!(v.use_worktree_hooks);
    assert!(v.reap);
    assert_eq!(v.scratch_dir, None);
}

#[test]
fn a_full_section_resolves() {
    let config = load(
        r#"[verify]
commands = ["scripts/check.sh rust", "pytest -q"]
env = { CI = "1", RUSTFLAGS = "-D warnings" }
timeout_seconds = 600
max_parallel = 4
auto = ["report", "integration"]
require = ["review", "integrate"]
output_tail_bytes = 2048
max_log_bytes = 65536
keep_logs = 5
worktree_hooks = false
reap = false
scratch_dir = "~/scratch/verify"
"#,
    )
    .expect("loads");
    let v = config.verify.clone().unwrap();
    assert_eq!(v.commands.len(), 2);
    assert_eq!(
        v.env,
        vec![
            ("CI".into(), "1".into()),
            ("RUSTFLAGS".into(), "-D warnings".into())
        ]
    );
    assert_eq!((v.timeout_seconds, v.max_parallel), (600, 4));
    assert!(v.auto_report && v.auto_integration && v.require_review && v.require_integrate);
    assert_eq!(
        (v.output_tail_bytes, v.max_log_bytes, v.keep_logs),
        (2048, 65536, 5)
    );
    assert!(!v.use_worktree_hooks && !v.reap);
    assert_eq!(v.scratch_dir.as_deref(), Some("~/scratch/verify"));
    assert!(config.to_json().contains("\"verify\""));
}

#[test]
fn auto_and_require_pick_one_each() {
    let v = verify("auto = [\"integration\"]\nrequire = [\"review\"]\n");
    assert!(!v.auto_report && v.auto_integration);
    assert!(v.require_review && !v.require_integrate);
}

#[test]
fn refusals_name_the_key() {
    let cases: &[(&str, &str)] = &[
        ("[verify]\n", "verify.commands"),
        ("[verify]\ncommands = []\n", "verify.commands"),
        ("[verify]\ncommands = [\"\"]\n", "verify.commands[0]"),
        ("[verify]\ncommands = [1]\n", "verify.commands[0]"),
        (
            "[verify]\ncommands = [\"true\"]\nbogus = 1\n",
            "unknown key",
        ),
        (
            "[verify]\ncommands = [\"true\"]\nmax_parallel = 0\n",
            "verify.max_parallel",
        ),
        (
            "[verify]\ncommands = [\"true\"]\nmax_parallel = 9\n",
            "verify.max_parallel",
        ),
        (
            "[verify]\ncommands = [\"true\"]\ntimeout_seconds = 59\n",
            "verify.timeout_seconds",
        ),
        (
            "[verify]\ncommands = [\"true\"]\ntimeout_seconds = 14401\n",
            "verify.timeout_seconds",
        ),
        (
            "[verify]\ncommands = [\"true\"]\nauto = [\"deploy\"]\n",
            "verify.auto[0]",
        ),
        (
            "[verify]\ncommands = [\"true\"]\nrequire = [\"merge\"]\n",
            "verify.require[0]",
        ),
        (
            "[verify]\ncommands = [\"true\"]\nauto = [\"report\", \"report\"]\n",
            "repeats",
        ),
        (
            "[verify]\ncommands = [\"true\"]\nenv = { PATH = \"/x\" }\n",
            "verify.env.PATH",
        ),
        (
            "[verify]\ncommands = [\"true\"]\nenv = { LD_PRELOAD = \"/x\" }\n",
            "verify.env.LD_PRELOAD",
        ),
        (
            "[verify]\ncommands = [\"true\"]\nenv = { lower = \"x\" }\n",
            "verify.env.lower",
        ),
        (
            "[verify]\ncommands = [\"true\"]\noutput_tail_bytes = 12289\n",
            "verify.output_tail_bytes",
        ),
        (
            "[verify]\ncommands = [\"true\"]\nmax_log_bytes = 1\n",
            "verify.max_log_bytes",
        ),
        (
            "[verify]\ncommands = [\"true\"]\nkeep_logs = 501\n",
            "verify.keep_logs",
        ),
        (
            "[verify]\ncommands = [\"true\"]\nscratch_dir = \"rel/dir\"\n",
            "verify.scratch_dir",
        ),
        (
            "[verify]\ncommands = [\"true\"]\nscratch_dir = \"/a/../b\"\n",
            "verify.scratch_dir",
        ),
        ("[verify]\ncommands = [\"true\"]\nreap = 1\n", "verify.reap"),
    ];
    for (text, key) in cases {
        let message = refusal(text);
        assert!(message.contains(key), "{text:?} -> {message}");
    }
}

#[test]
fn command_count_and_length_limits() {
    let many = (0..17)
        .map(|i| format!("\"c{i}\""))
        .collect::<Vec<_>>()
        .join(", ");
    assert!(refusal(&format!("[verify]\ncommands = [{many}]\n")).contains("verify.commands"));
    let sixteen = (0..16)
        .map(|i| format!("\"c{i}\""))
        .collect::<Vec<_>>()
        .join(", ");
    assert!(load(&format!("[verify]\ncommands = [{sixteen}]\n")).is_ok());
    let long = "x".repeat(1001);
    assert!(refusal(&format!("[verify]\ncommands = [\"{long}\"]\n")).contains("verify.commands[0]"));
    let edge = "x".repeat(1000);
    assert!(load(&format!("[verify]\ncommands = [\"{edge}\"]\n")).is_ok());
}

#[test]
fn commands_sha_tracks_commands_and_order() {
    let sha = |list: &str| {
        load(&format!("[verify]\ncommands = {list}\n"))
            .unwrap()
            .verify
            .unwrap()
            .commands_sha()
    };
    assert_eq!(sha("[\"a\", \"b\"]"), sha("[\"a\", \"b\"]"));
    assert_ne!(sha("[\"a\", \"b\"]"), sha("[\"b\", \"a\"]"));
    assert_ne!(sha("[\"a\", \"b\"]"), sha("[\"a\", \"c\"]"));
    assert_ne!(sha("[\"a\"]"), sha("[\"a\", \"b\"]"));
    assert!(sha("[\"a\"]")
        .chars()
        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
}
