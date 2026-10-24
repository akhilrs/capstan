//! The built-in designer prompt: when a role gets it, and that a configured prompt or prompt_file still wins.
mod common;

use capstan_config::prompts::{
    build_role_prompt, builtin_role_prompt, Kind, PromptInput, DESIGNER_BUILTIN_PROMPT,
    DESIGNER_PROMPT,
};
use capstan_config::starter::{DESIGNER_PROMPT_PATH, STARTER_CONFIG};
use capstan_config::{parse_config, ConfigError, Role};
use std::path::Path;

const BASE: &str = "schema_version = 1\n[nexora]\ntrack = \"never\"\n[hosts.claude]\nkind = \"claude\"\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n";

fn role(extra: &str, name: &str, root: &Path) -> Result<Role, String> {
    let config =
        parse_config(format!("{BASE}{extra}").as_bytes(), root).map_err(|error| match error {
            ConfigError::Invalid(message) => message,
            other => format!("{other:?}"),
        })?;
    Ok(config.roles.into_iter().find(|r| r.name == name).unwrap())
}

#[test]
fn a_designer_developer_without_a_prompt_gets_the_built_in() {
    let r = role(
        "[roles.designer]\nkind = \"Developer\"\nhost = \"claude\"\n",
        "designer",
        Path::new("/nonexistent"),
    )
    .unwrap();
    assert_eq!(r.prompt.source, "builtin");
    assert!(r.prompt.path.is_none());
    assert_eq!(r.prompt.hash.as_deref().map(str::len), Some(64));
    assert_eq!(r.prompt_text.as_deref(), Some(DESIGNER_BUILTIN_PROMPT));
    assert_eq!(
        r.prompt.hash.as_deref(),
        Some(common::sha256_hex(DESIGNER_BUILTIN_PROMPT.as_bytes()).as_str())
    );
    assert!(DESIGNER_BUILTIN_PROMPT.starts_with(DESIGNER_PROMPT));
    assert_eq!(
        DESIGNER_BUILTIN_PROMPT
            .matches("## Design guidance")
            .count(),
        1
    );
}

#[test]
fn a_legacy_prompt_file_still_wins() {
    let dir = std::env::temp_dir().join(format!("capstan-builtin-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("roles")).unwrap();
    std::fs::write(dir.join(DESIGNER_PROMPT_PATH), "my own designer text\n").unwrap();
    let toml = "[roles.designer]\nkind = \"Developer\"\nhost = \"claude\"\nprompt_file = \"roles/designer.md\"\n";
    let r = role(toml, "designer", &dir).unwrap();
    assert_eq!(r.prompt.source, "file");
    assert!(r.prompt.path.is_some() && r.prompt.hash.is_some());
    assert_eq!(r.prompt_text.as_deref(), Some("my own designer text\n"));
    std::fs::remove_file(dir.join(DESIGNER_PROMPT_PATH)).unwrap();
    let r = role(toml, "designer", &dir).unwrap();
    assert_eq!(r.prompt.source, "builtin");
    assert_eq!(r.prompt_text.as_deref(), Some(DESIGNER_BUILTIN_PROMPT));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn only_a_developer_named_designer_has_a_default() {
    let verifier = role(
        "[roles.designer]\nkind = \"Verifier\"\nhost = \"claude\"\n",
        "designer",
        Path::new("/x"),
    )
    .unwrap();
    assert_eq!(verifier.prompt.source, "none");
    assert!(verifier.prompt_text.is_none());
    let other = role(
        "[roles.ui]\nkind = \"Developer\"\nhost = \"claude\"\n",
        "ui",
        Path::new("/x"),
    )
    .unwrap();
    assert_eq!(other.prompt.source, "none");
    let inline = role(
        "[roles.designer]\nkind = \"Developer\"\nhost = \"claude\"\nprompt = \"mine\"\n",
        "designer",
        Path::new("/x"),
    )
    .unwrap();
    assert_eq!(inline.prompt.source, "inline");
    assert_eq!(inline.prompt_text.as_deref(), Some("mine"));
    assert_eq!(
        builtin_role_prompt("designer", "Developer"),
        Some(DESIGNER_BUILTIN_PROMPT)
    );
    assert_eq!(builtin_role_prompt("designer", "PM"), None);
}

#[test]
fn the_built_in_text_reaches_the_assembled_prompt() {
    let mut input = PromptInput::new("designer", Kind::Developer, "designer-1", 600.0);
    input.role_prompt = builtin_role_prompt("designer", "Developer").map(str::to_string);
    let prompt = build_role_prompt(&input).unwrap();
    let line = DESIGNER_PROMPT.lines().next().unwrap();
    assert!(prompt.contains(line));
    assert!(prompt.contains("## Design guidance"));
}

/// A copy of `toml_string` in `rust/crates/herdr/src/hosts.rs` (lines 17-32), counting bytes: backslash and quote
/// take 2, code points below 0x20 and 0x7f-0x9f take 6, the rest their UTF-8 length.
fn toml_escaped_len(value: &str) -> usize {
    2 + value
        .chars()
        .map(|character| {
            let code = character as u32;
            if character == '\\' || character == '"' {
                2
            } else if code < 0x20 || (0x7f..=0x9f).contains(&code) {
                6
            } else {
                character.len_utf8()
            }
        })
        .sum::<usize>()
}

#[test]
fn the_built_designer_prompt_fits_the_argument_limits() {
    let mut input = PromptInput::new("designer", Kind::Developer, "designer-1", 600.0);
    input.role_prompt = builtin_role_prompt("designer", "Developer").map(str::to_string);
    let prompt = build_role_prompt(&input).unwrap();
    assert!(prompt.len() < 48 * 1024, "{} bytes", prompt.len());
    // The Codex argument limit is 120 KiB (hosts.rs:8); 96 KiB leaves 24 KiB for drift.
    let escaped = toml_escaped_len(&prompt);
    assert!(escaped < 96 * 1024, "{escaped} escaped bytes");
}

const FILE_DESIGNER: &str =
    "[roles.designer]\nkind = \"Developer\"\nprompt_file = \"roles/designer.md\"\n";

/// The starter with the designer reading `prompt_file = "roles/designer.md"` (what an older starter wrote) or not.
fn starter_designer(file_backed: bool, root: &Path) -> Role {
    let marker = "[roles.designer]\nkind = \"Developer\"\n";
    let text = if file_backed {
        STARTER_CONFIG.replace(marker, FILE_DESIGNER)
    } else {
        STARTER_CONFIG.to_string()
    };
    let config = parse_config(text.as_bytes(), root).unwrap();
    config
        .roles
        .into_iter()
        .find(|r| r.name == "designer")
        .unwrap()
}

// Measured at base 40735917, before the guidance was appended.
const BASE_PROMPT_HASH: &str = "783fe437fdb226f88196f1b7f0cd91f857d8f5ef3a9d3140554920ac36948f86";
const BASE_FILE_CONFIG_HASH: &str =
    "82e059d7c1293e13edb9a2a3f766d9046416f2513d843fab4f6397afc2f2d108";
const BASE_BUILTIN_CONFIG_HASH: &str =
    "e9e940ae1347cafd0dfd9fcbc4e55fa63fbc0a66a17cd0db225867884cfba366";
// Measured after the change.
const BUILTIN_CONFIG_HASH: &str =
    "7eeba52f4ea5abcb1dbc88a25b385748d62eac0c197029cbe90831925bafd9e5";

#[test]
fn a_file_backed_designer_keeps_its_hashes() {
    let dir = std::env::temp_dir().join(format!("capstan-filehash-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("roles")).unwrap();
    std::fs::write(dir.join(DESIGNER_PROMPT_PATH), DESIGNER_PROMPT).unwrap();
    let r = starter_designer(true, &dir);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(r.prompt.source, "file");
    assert_eq!(r.prompt.hash.as_deref(), Some(BASE_PROMPT_HASH));
    assert_eq!(r.config_hash, BASE_FILE_CONFIG_HASH);
}

#[test]
fn the_built_in_designer_hash_moves_with_the_guidance() {
    let r = starter_designer(false, Path::new("/nonexistent"));
    assert_eq!(r.prompt.source, "builtin");
    assert_ne!(r.prompt.hash.as_deref(), Some(BASE_PROMPT_HASH));
    assert_ne!(r.config_hash, BASE_BUILTIN_CONFIG_HASH);
    assert_eq!(r.config_hash, BUILTIN_CONFIG_HASH);
}

#[test]
fn the_starter_designer_is_still_a_valid_config() {
    // package init removes the starter's prompt_file line; until then the starter's file case keeps working.
    assert!(STARTER_CONFIG.contains("[roles.designer]"));
}

fn warnings(extra: &str, root: &Path) -> Vec<String> {
    parse_config(format!("{BASE}{extra}").as_bytes(), root)
        .unwrap()
        .warnings
}

#[test]
fn a_missing_prompt_file_warns_when_the_role_has_a_built_in() {
    let toml = "[roles.designer]\nkind = \"Developer\"\nhost = \"claude\"\nprompt_file = \"roles/designer.md\"\n";
    let w = warnings(toml, Path::new("/nonexistent"));
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(
        w[0].contains("roles.designer")
            && w[0].contains("roles/designer.md")
            && w[0].contains("built-in"),
        "{w:?}"
    );
}

#[test]
fn a_missing_prompt_file_still_errors_without_a_built_in() {
    let toml =
        "[roles.ui]\nkind = \"Developer\"\nhost = \"claude\"\nprompt_file = \"roles/ui.md\"\n";
    let error = role(toml, "ui", Path::new("/nonexistent")).unwrap_err();
    assert!(
        error.contains("roles.ui.prompt_file does not exist"),
        "{error}"
    );
}
