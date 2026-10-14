//! The built-in designer prompt: when a role gets it, and that a configured prompt or prompt_file still wins.
use capstan_config::prompts::{
    build_role_prompt, builtin_role_prompt, Kind, PromptInput, DESIGNER_PROMPT,
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
    assert_eq!(r.prompt_text.as_deref(), Some(DESIGNER_PROMPT));
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
    assert_eq!(r.prompt_text.as_deref(), Some(DESIGNER_PROMPT));
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
        Some(DESIGNER_PROMPT)
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
