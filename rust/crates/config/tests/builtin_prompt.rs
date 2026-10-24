//! The built-in designer prompt: when a role gets it, and that a configured prompt or prompt_file still wins.
mod common;

use capstan_config::prompts::{
    build_role_prompt, builtin_role_prompt, Kind, PromptInput, DESIGNER_BUILTIN_PROMPT,
    DESIGNER_PROMPT, DESIGN_GUIDANCE,
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

// ---------------------------------------------------------------------------------------------------------------
// The `[design]` keys and the designer's prompt.

// Measured on ecff675, before the keys existed.
const ECFF675_BUILTIN_PROMPT_HASH: &str =
    "bac7bf19b78e08c0a8f5eff13236aeb77f5ca96a778025ce7fd41b067a8b305a";
const ECFF675_BUILTIN_CONFIG_HASH: &str =
    "7eeba52f4ea5abcb1dbc88a25b385748d62eac0c197029cbe90831925bafd9e5";

const OWN: &str = "my own designer text\n";
const OWN_DESIGNER: &str = "[roles.designer]\nkind = \"Developer\"\nhost = \"claude\"\nprompt = \"my own designer text\\n\"\n";

fn starter_with(design: &str) -> Role {
    let text = format!("{STARTER_CONFIG}\n{design}");
    let config = parse_config(text.as_bytes(), Path::new("/nonexistent")).unwrap();
    config
        .roles
        .into_iter()
        .find(|r| r.name == "designer")
        .unwrap()
}

fn own(design: &str) -> Role {
    role(
        &format!("{OWN_DESIGNER}{design}"),
        "designer",
        Path::new("/nonexistent"),
    )
    .unwrap()
}

fn block_of(design: &str) -> capstan_config::design::Design {
    parse_config(
        format!("{BASE}{OWN_DESIGNER}{design}").as_bytes(),
        Path::new("/nonexistent"),
    )
    .unwrap()
    .design
}

fn sha(text: &str) -> String {
    common::sha256_hex(text.as_bytes())
}

#[test]
fn with_no_new_key_the_starter_designer_is_what_it_was() {
    for design in ["", "[design]\n", "[design]\nguidance = \"on\"\n"] {
        let r = starter_with(design);
        assert_eq!(
            r.prompt_text.as_deref(),
            Some(DESIGNER_BUILTIN_PROMPT),
            "{design}"
        );
        assert_eq!(
            r.prompt.hash.as_deref(),
            Some(ECFF675_BUILTIN_PROMPT_HASH),
            "{design}"
        );
        assert_eq!(r.config_hash, ECFF675_BUILTIN_CONFIG_HASH, "{design}");
        assert!(r.guidance.is_none(), "{design}");
    }
}

#[test]
fn a_block_key_ends_the_built_in_text_with_the_block() {
    let r = starter_with("[design]\ndirections_max = 4\n");
    let block =
        capstan_config::design_guidance::project_block(&block_of("[design]\ndirections_max = 4\n"))
            .unwrap();
    let text = r.prompt_text.unwrap();
    assert_eq!(text, format!("{DESIGNER_BUILTIN_PROMPT}\n{block}"));
    assert!(text.contains("offer 2 to 4"));
    assert_eq!(r.prompt.hash.as_deref(), Some(ECFF675_BUILTIN_PROMPT_HASH));
    assert_ne!(r.config_hash, ECFF675_BUILTIN_CONFIG_HASH);
    let guidance = r.guidance.unwrap();
    assert_eq!(guidance.parts, ["project"]);
    assert_eq!(guidance.hash, sha(&format!("\n{block}")));
}

#[test]
fn guidance_off_gives_the_designer_text_alone() {
    let r = starter_with("[design]\nguidance = \"off\"\n");
    assert_eq!(r.prompt_text.as_deref(), Some(DESIGNER_PROMPT));
    assert_eq!(
        r.prompt.hash.as_deref(),
        Some(sha(DESIGNER_PROMPT).as_str())
    );
    assert!(r.guidance.is_none());
    // A block key alone adds nothing to it either.
    let r = starter_with("[design]\nguidance = \"off\"\ndirections_max = 4\n");
    assert_eq!(r.prompt_text.as_deref(), Some(DESIGNER_PROMPT));
    // A record key adds the record lines only.
    let r = starter_with("[design]\nguidance = \"off\"\ndesign_record_mode = \"read\"\n");
    let record = capstan_config::design_guidance::record_block(&block_of(
        "[design]\nguidance = \"off\"\ndesign_record_mode = \"read\"\n",
    ))
    .unwrap();
    assert_eq!(
        r.prompt_text.unwrap(),
        format!("{DESIGNER_PROMPT}\n{record}")
    );
    assert_eq!(r.guidance.unwrap().parts, ["record"]);
}

#[test]
fn a_prompt_file_designer_with_guidance_on_gets_the_file_the_guidance_and_the_block() {
    let dir = std::env::temp_dir().join(format!("capstan-guidance-on-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("roles")).unwrap();
    std::fs::write(dir.join(DESIGNER_PROMPT_PATH), OWN).unwrap();
    let file = "[roles.designer]\nkind = \"Developer\"\nhost = \"claude\"\nprompt_file = \"roles/designer.md\"\n";
    let on = role(
        &format!("{file}[design]\nguidance = \"on\"\ndirections_max = 4\n"),
        "designer",
        &dir,
    )
    .unwrap();
    let builtin = role(
        &format!("{file}[design]\ndirections_max = 4\n"),
        "designer",
        &dir,
    )
    .unwrap();
    let block =
        capstan_config::design_guidance::project_block(&block_of("[design]\ndirections_max = 4\n"))
            .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(on.prompt.source, "file");
    assert_eq!(
        on.prompt_text.unwrap(),
        format!("{OWN}\n{DESIGN_GUIDANCE}\n{block}")
    );
    assert_eq!(on.prompt.hash.as_deref(), Some(sha(OWN).as_str()));
    assert_ne!(on.config_hash, builtin.config_hash);
    let guidance = on.guidance.unwrap();
    assert_eq!(guidance.parts, ["guidance", "project"]);
    assert_eq!(guidance.hash, sha(&format!("\n{DESIGN_GUIDANCE}\n{block}")));
    // guidance on with no block key appends the guidance alone.
    let dir = std::env::temp_dir().join(format!("capstan-guidance-only-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("roles")).unwrap();
    std::fs::write(dir.join(DESIGNER_PROMPT_PATH), OWN).unwrap();
    let only = role(
        &format!("{file}[design]\nguidance = \"on\"\n"),
        "designer",
        &dir,
    )
    .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(
        only.prompt_text.unwrap(),
        format!("{OWN}\n{DESIGN_GUIDANCE}")
    );
    assert_eq!(only.guidance.unwrap().parts, ["guidance"]);
}

#[test]
fn an_own_prompt_with_guidance_builtin_gets_only_the_record_lines() {
    let design = "[design]\ndesign_record = \"docs/DESIGN.md\"\n";
    let record = capstan_config::design_guidance::record_block(&block_of(design)).unwrap();
    let r = own(design);
    assert_eq!(r.prompt_text.unwrap(), format!("{OWN}\n{record}"));
    assert_eq!(r.prompt.hash.as_deref(), Some(sha(OWN).as_str()));
    assert_eq!(r.guidance.unwrap().parts, ["record"]);
}

#[test]
fn an_own_prompt_with_guidance_builtin_ignores_the_block_keys_and_says_so() {
    let plain = own("");
    let keyed = own("[design]\ndirections_max = 4\nreference_collection = \"/opt/refs\"\n");
    assert_eq!(keyed.prompt_text, plain.prompt_text);
    assert_eq!(keyed.config_hash, plain.config_hash);
    assert!(keyed.guidance.is_none());
    let w = warnings(
        &format!(
            "{OWN_DESIGNER}[design]\ndirections_max = 4\nreference_collection = \"/opt/refs\"\n"
        ),
        Path::new("/nonexistent"),
    );
    assert!(
        w.iter().any(|line| line
            == "design: guidance = \"builtin\" and roles.designer has its own prompt, so directions_max, reference_collection have no effect; set guidance = \"on\""),
        "{w:?}"
    );
    // The built-in designer takes the keys, so no warning; guidance on and off do not warn either.
    for design in [
        "[design]\nguidance = \"on\"\ndirections_max = 4\n",
        "[design]\nguidance = \"off\"\ndirections_max = 4\n",
    ] {
        let w = warnings(
            &format!("{OWN_DESIGNER}{design}"),
            Path::new("/nonexistent"),
        );
        assert!(
            w.iter().all(|line| !line.contains("has its own prompt")),
            "{w:?}"
        );
    }
}

#[test]
fn an_own_prompt_with_guidance_off_gets_the_record_lines_and_ignores_the_block_keys() {
    let design = "[design]\nguidance = \"off\"\ndesign_record = \"docs/DESIGN.md\"\n";
    let record = capstan_config::design_guidance::record_block(&block_of(design)).unwrap();
    let r = own(design);
    assert_eq!(r.prompt_text.unwrap(), format!("{OWN}\n{record}"));
    let plain = own("[design]\nguidance = \"off\"\n");
    let keyed = own("[design]\nguidance = \"off\"\ndirections_max = 4\n");
    assert_eq!(plain.prompt_text.as_deref(), Some(OWN));
    assert_eq!(keyed.prompt_text, plain.prompt_text);
}

#[test]
fn mode_off_puts_the_ignore_instruction_after_an_inline_or_file_prompt() {
    let dir = std::env::temp_dir().join(format!("capstan-mode-off-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("roles")).unwrap();
    std::fs::write(dir.join(DESIGNER_PROMPT_PATH), OWN).unwrap();
    let file = "[roles.designer]\nkind = \"Developer\"\nhost = \"claude\"\nprompt_file = \"roles/designer.md\"\n";
    for guidance in ["builtin", "off"] {
        let on = format!("[design]\nguidance = \"{guidance}\"\ndesign_record_mode = \"off\"\n");
        let without = format!("[design]\nguidance = \"{guidance}\"\n");
        let record = capstan_config::design_guidance::record_block(&block_of(&on)).unwrap();
        assert!(record.contains("mode off. Ignore it"));
        for (roles, own_text) in [(OWN_DESIGNER, OWN), (file, OWN)] {
            let changed = role(&format!("{roles}{on}"), "designer", &dir).unwrap();
            let same = role(&format!("{roles}{without}"), "designer", &dir).unwrap();
            assert_eq!(
                changed.prompt_text.unwrap(),
                format!("{own_text}\n{record}"),
                "{guidance}"
            );
            assert_eq!(changed.prompt.hash, same.prompt.hash);
            assert_eq!(changed.prompt.hash.as_deref(), Some(sha(own_text).as_str()));
            assert_ne!(changed.config_hash, same.config_hash, "{guidance}");
            assert_eq!(changed.guidance.unwrap().parts, ["record"]);
            // A block key alone changes nothing.
            let keyed = role(
                &format!("{roles}[design]\nguidance = \"{guidance}\"\ndirections_max = 4\n"),
                "designer",
                &dir,
            )
            .unwrap();
            assert_eq!(keyed.prompt_text, same.prompt_text);
            assert_eq!(keyed.config_hash, same.config_hash);
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_stale_prompt_file_falls_back_to_the_built_in_rules() {
    let toml = "[roles.designer]\nkind = \"Developer\"\nhost = \"claude\"\nprompt_file = \"roles/designer.md\"\n[design]\ndirections_max = 4\n";
    let r = role(toml, "designer", Path::new("/nonexistent")).unwrap();
    assert_eq!(r.prompt.source, "builtin");
    assert!(r.prompt_text.unwrap().contains("offer 2 to 4"));
    assert_eq!(r.prompt.hash.as_deref(), Some(ECFF675_BUILTIN_PROMPT_HASH));
}

#[test]
fn other_roles_are_not_changed_by_the_keys() {
    let design = "[design]\nguidance = \"on\"\ndirections_max = 4\ndesign_record_mode = \"off\"\n";
    let developer = "[roles.ui]\nkind = \"Developer\"\nhost = \"claude\"\nprompt = \"mine\"\n";
    let ui = role(&format!("{developer}{design}"), "ui", Path::new("/x")).unwrap();
    assert_eq!(ui.prompt_text.as_deref(), Some("mine"));
    assert!(ui.guidance.is_none());
    let verifier = role(
        &format!("[roles.designer]\nkind = \"Verifier\"\nhost = \"claude\"\n{design}"),
        "designer",
        Path::new("/x"),
    )
    .unwrap();
    assert!(verifier.prompt_text.is_none() && verifier.guidance.is_none());
}

fn prompt_json(toml: &str) -> serde_json::Value {
    let config = parse_config(toml.as_bytes(), Path::new("/nonexistent")).unwrap();
    serde_json::from_str::<serde_json::Value>(&config.to_json()).unwrap()["roles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|role| role["name"] == "designer")
        .unwrap()["prompt"]
        .clone()
}

#[test]
fn the_json_describes_the_appended_text_only_when_there_is_some() {
    let starter = |design: &str| prompt_json(&format!("{STARTER_CONFIG}\n{design}"));
    assert!(starter("").get("guidance").is_none());
    let block = starter_with("[design]\ndirections_max = 4\n");
    let prompt = starter("[design]\ndirections_max = 4\n");
    assert_eq!(prompt["guidance"]["parts"], serde_json::json!(["project"]));
    assert_eq!(
        prompt["guidance"]["hash"].as_str(),
        block.guidance.as_ref().map(|g| g.hash.as_str())
    );
    let record = prompt_json(&format!(
        "{BASE}{OWN_DESIGNER}[design]\ndesign_record_mode = \"off\"\n"
    ));
    assert_eq!(record["guidance"]["parts"], serde_json::json!(["record"]));
    let plain = prompt_json(&format!("{BASE}{OWN_DESIGNER}"));
    assert!(plain.get("guidance").is_none());
}
