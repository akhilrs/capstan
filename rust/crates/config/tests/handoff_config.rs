//! `[messages] max_body_bytes` and `roles.<name>.write_dirs`: defaults, bounds, refusals with their exact text, JSON and hash.
use capstan_config::{
    parse_config, starter::STARTER_CONFIG, ConfigError, RoleConfig, DEFAULT_MAX_BODY_BYTES,
    MAX_BODY_BYTES_LIMIT,
};
use std::path::Path;

const BASE: &str = r#"schema_version = 1
[nexora]
track = "never"
[hosts.claude]
kind = "claude"
[hosts.codex]
kind = "codex"
[roles.pm]
kind = "PM"
host = "claude"
[roles.verifier]
kind = "Verifier"
host = "claude"
"#;

fn load_text(text: &str) -> Result<RoleConfig, String> {
    parse_config(text.as_bytes(), Path::new("/nonexistent")).map_err(|error| match error {
        ConfigError::Invalid(message) => message,
        other => format!("{other:?}"),
    })
}

fn load(extra: &str) -> Result<RoleConfig, String> {
    load_text(&format!("{BASE}{extra}"))
}

fn dev(line: &str) -> String {
    format!("[roles.dev]\nkind = \"Developer\"\nhost = \"claude\"\n{line}\n")
}

fn dirs_refusal(line: &str) -> String {
    load(&dev(line)).expect_err("refused")
}

#[test]
fn max_body_bytes_default_and_bounds() {
    assert_eq!(DEFAULT_MAX_BODY_BYTES, 4096);
    assert_eq!(MAX_BODY_BYTES_LIMIT, 15360);
    assert_eq!(load("").unwrap().messages.max_body_bytes, 4096);
    for ok in [512, 15360] {
        let config = load(&format!("[messages]\nmax_body_bytes = {ok}\n")).unwrap();
        assert_eq!(config.messages.max_body_bytes, ok);
    }
    let want = "messages.max_body_bytes must be an integer from 512 to 15360";
    for bad in ["511", "15361", "\"4096\"", "4096.5", "true"] {
        assert_eq!(
            load(&format!("[messages]\nmax_body_bytes = {bad}\n")).unwrap_err(),
            want
        );
    }
}

#[test]
fn max_body_bytes_json_only_when_not_default() {
    let json = load("").unwrap().to_json();
    assert!(!json.contains("maxBodyBytes"), "{json}");
    let json = load("[messages]\nmax_body_bytes = 4096\n")
        .unwrap()
        .to_json();
    assert!(!json.contains("maxBodyBytes"), "{json}");
    let json = load("[messages]\nmax_body_bytes = 8192\n")
        .unwrap()
        .to_json();
    assert!(json.contains("\"maxBodyBytes\": 8192"), "{json}");
}

#[test]
fn write_dirs_resolve_without_trailing_slash() {
    let config = load(&dev("write_dirs = [\"docs/architecture/\", \"notes\"]")).unwrap();
    let role = config.roles.iter().find(|r| r.name == "dev").unwrap();
    assert_eq!(role.write_dirs, ["docs/architecture", "notes"]);
    assert!(config.to_json().contains("\"writeDirs\""));
}

#[test]
fn write_dirs_empty_or_absent_is_none() {
    for line in ["", "write_dirs = []"] {
        let config = load(&dev(line)).unwrap();
        let role = config.roles.iter().find(|r| r.name == "dev").unwrap();
        assert!(role.write_dirs.is_empty());
        assert!(!config.to_json().contains("writeDirs"));
    }
}

#[test]
fn write_dirs_entry_refusals() {
    for bad in [
        "a b", "../x", "a/../b", ".git", "a/.git/b", ".capstan", "/abs", ".", "a//b", "",
    ] {
        let want = format!(
            "roles.dev.write_dirs: \"{bad}\" must be a relative folder of letters, digits, ., _ and - without ., .., .git or .capstan segments"
        );
        assert_eq!(
            dirs_refusal(&format!("write_dirs = [\"{bad}\"]")),
            want,
            "{bad}"
        );
    }
    assert_eq!(
        dirs_refusal("write_dirs = [\"a\", \"a/\"]"),
        "roles.dev.write_dirs lists \"a\" twice"
    );
    let nine: Vec<String> = (0..9).map(|i| format!("\"d{i}\"")).collect();
    assert_eq!(
        dirs_refusal(&format!("write_dirs = [{}]", nine.join(", "))),
        "roles.dev.write_dirs may list at most 8 folders"
    );
    let eight: Vec<String> = (0..8).map(|i| format!("\"d{i}\"")).collect();
    load(&dev(&format!("write_dirs = [{}]", eight.join(", ")))).unwrap();
    let long = "a".repeat(201);
    assert!(
        dirs_refusal(&format!("write_dirs = [\"{long}\"]")).contains("must be a relative folder")
    );
}

#[test]
fn write_dirs_kind_and_host_refusals() {
    let verifier = BASE.replace(
        "[roles.verifier]\nkind = \"Verifier\"\nhost = \"claude\"\n",
        "[roles.verifier]\nkind = \"Verifier\"\nhost = \"claude\"\nwrite_dirs = [\"x\"]\n",
    );
    assert_eq!(
        load_text(&verifier).unwrap_err(),
        "roles.verifier.write_dirs: only a Developer role may have write folders; kind is Verifier"
    );
    let pm = BASE.replace(
        "[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n",
        "[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\nwrite_dirs = [\"x\"]\n",
    );
    assert_eq!(
        load_text(&pm).unwrap_err(),
        "roles.pm.write_dirs: only a Developer role may have write folders; kind is PM"
    );
    let codex = "[roles.cx]\nkind = \"Developer\"\nhost = \"codex\"\npermission_mode = \"auto\"\nwrite_dirs = [\"x\"]\n";
    assert_eq!(
        load(codex).unwrap_err(),
        "roles.cx.write_dirs: needs a claude host, which enforces it; host codex is codex"
    );
}

#[test]
fn write_dirs_refused_on_the_operator_role() {
    let text = format!(
        "{BASE}[operator]\nenabled = true\nrole = \"op\"\n[roles.op]\nkind = \"Developer\"\nhost = \"claude\"\nwrite_dirs = [\"x\"]\n"
    );
    assert_eq!(
        load_text(&text).unwrap_err(),
        "roles.op.write_dirs: the operator role cannot have write folders"
    );
}

#[test]
fn config_hash_ignores_an_absent_write_dirs() {
    let plain = load(&dev("")).unwrap();
    let hash = &plain
        .roles
        .iter()
        .find(|r| r.name == "dev")
        .unwrap()
        .config_hash;
    assert_eq!(
        hash,
        "d93732e9d4d49b964657169ebe400e025d5af8871503b98c407dcc91cd3ef724"
    );
    let with = load(&dev("write_dirs = [\"x\"]")).unwrap();
    let other = &with
        .roles
        .iter()
        .find(|r| r.name == "dev")
        .unwrap()
        .config_hash;
    assert_ne!(hash, other);
    let empty = load(&dev("write_dirs = []")).unwrap();
    assert_eq!(
        hash,
        &empty
            .roles
            .iter()
            .find(|r| r.name == "dev")
            .unwrap()
            .config_hash
    );
}

#[test]
fn starter_architect_has_the_two_folders() {
    let config = parse_config(STARTER_CONFIG.as_bytes(), Path::new("/nonexistent")).unwrap();
    let role = config.roles.iter().find(|r| r.name == "architect").unwrap();
    assert_eq!(role.write_dirs, ["docs/architecture", "docs/research"]);
}
