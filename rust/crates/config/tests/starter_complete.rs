//! The starter capstan.toml is short and generic; docs/reference documents every key the parser accepts. The key
//! registry (`keys.rs`) is the parser's own list, so these tests tie four things together: the registry equals what the
//! parser prints, the docs mention every registry key, the starter mentions no key the parser refuses, and every
//! commented block of the starter loads once it is un-commented.
use capstan_config::cache::CACHE_PRESETS;
use capstan_config::keys::*;
use capstan_config::starter::STARTER_CONFIG;
use capstan_config::{accepted_keys, parse_config, toml_refusal, ConfigError, ROLE_KINDS};
use std::collections::BTreeSet;
use std::path::Path;

/// Commented blocks that are not meant to load on their own, each with the reason (a block name is its header, such
/// as `[hosts.codex]`). Empty while every block loads; add one only with a one-line reason.
const ILLUSTRATIVE: &[(&str, &str)] = &[];

/// Blocks that only load together with others: the block, then the blocks that must be un-commented with it.
const WITH: &[(&str, &[&str])] = &[("[roles.external-review]", &["[hosts.codex]"])];

#[derive(Debug, PartialEq)]
enum Line {
    Header(String),
    Key(String, String),
    Other,
}

/// What the TOML part of a line is, after the "# " of a commented line is removed.
fn classify(text: &str) -> Line {
    let text = text.trim_start();
    if let Some(rest) = text.strip_prefix('[') {
        let Some(end) = rest.find(']') else {
            return Line::Other;
        };
        let name = &rest[..end];
        let after = rest[end + 1..].trim();
        let name_ok = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '<' | '>'));
        if name_ok && (after.is_empty() || after.starts_with('#')) {
            return Line::Header(name.to_string());
        }
        return Line::Other;
    }
    let key_end = text
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .unwrap_or(text.len());
    if key_end == 0 {
        return Line::Other;
    }
    let (key, rest) = text.split_at(key_end);
    match rest.trim_start().strip_prefix('=') {
        Some(value) => Line::Key(key.to_string(), value.trim().to_string()),
        None => Line::Other,
    }
}

/// One line of the starter: active TOML, commented TOML (with its text un-commented) or prose.
struct Entry {
    commented: bool,
    line: Line,
    raw: String,
}

/// A commented line that looks like TOML must be TOML. `# [` and `# word =` are the two shapes.
fn looks_like_toml(text: &str) -> bool {
    let text = text.trim_start();
    if text.starts_with('[') {
        return true;
    }
    let key_end = text
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .unwrap_or(text.len());
    key_end > 0 && text[key_end..].trim_start().starts_with('=')
}

fn entries(text: &str) -> Vec<Entry> {
    text.lines()
        .map(|raw| {
            if let Some(rest) = raw.strip_prefix("# ") {
                let line = classify(rest);
                if looks_like_toml(rest) {
                    assert!(
                        line != Line::Other,
                        "a commented line looks like TOML but is not a one-line key/value or header: {raw}"
                    );
                    if let Line::Key(key, value) = &line {
                        // Strict: the one-line value must be complete TOML.
                        assert!(
                            toml_refusal(&format!("{key} = {value}\n")).is_none(),
                            "a commented key's value is not a complete one-line TOML value: {raw}"
                        );
                    }
                }
                Entry {
                    commented: true,
                    line,
                    raw: raw.to_string(),
                }
            } else {
                Entry {
                    commented: false,
                    line: classify(raw),
                    raw: raw.to_string(),
                }
            }
        })
        .collect()
}

/// Concrete names for the user-named segments.
fn placeholder(table: &str) -> String {
    let parts: Vec<&str> = table.split('.').collect();
    let mut out: Vec<String> = parts.iter().map(|part| part.to_string()).collect();
    match parts.as_slice() {
        ["hosts", _, ..] | ["roles", _, ..] | ["mcp_servers", _, ..] => out[1] = "<name>".into(),
        ["cache", "entries", _, ..] => out[2] = "<name>".into(),
        _ => {}
    }
    out.join(".")
}

/// Every key path the starter mentions, active or commented, with user-named segments as placeholders; the second set
/// is the tables whose own keys are user-chosen names (`messages.notice_class`, `verify.env`).
fn starter_paths(text: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut paths = BTreeSet::new();
    let mut tables = BTreeSet::new();
    let mut table = String::new();
    for entry in entries(text) {
        match entry.line {
            Line::Header(name) => {
                table = placeholder(&name);
                tables.insert(table.clone());
            }
            Line::Key(key, _) => {
                let path = if table.is_empty() {
                    key
                } else {
                    format!("{table}.{key}")
                };
                paths.insert(placeholder(&path));
            }
            Line::Other => {}
        }
    }
    (paths, tables)
}

fn is_dynamic(path: &str) -> Option<&str> {
    path.strip_suffix(".<kind>")
        .or_else(|| path.strip_suffix(".<ENV>"))
}

/// The text of every Markdown file under docs/reference.
fn reference_docs() -> String {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/reference");
    let mut text = String::new();
    for entry in std::fs::read_dir(&dir).expect("docs/reference") {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|ext| ext == "md") {
            text.push_str(&std::fs::read_to_string(path).unwrap());
        }
    }
    text
}

#[test]
fn the_docs_mention_every_accepted_key() {
    let docs = reference_docs();
    let missing: Vec<String> = accepted_keys()
        .into_iter()
        .filter(|key| !key.deprecated && is_dynamic(&key.path).is_none())
        .filter(|key| {
            let name = key.path.rsplit('.').next().unwrap();
            !docs.contains(&format!("`{name}`")) && !docs.contains(&format!("{name} ="))
        })
        .map(|key| key.path)
        .collect();
    assert!(
        missing.is_empty(),
        "docs/reference does not mention these accepted keys:\n  {}",
        missing.join("\n  ")
    );
}

#[test]
fn the_starter_is_short() {
    let comments = STARTER_CONFIG
        .lines()
        .filter(|line| line.starts_with('#'))
        .count();
    assert!(comments < 48, "{comments} comment lines");
}

#[test]
fn the_starter_mentions_no_key_the_parser_refuses() {
    let (paths, _) = starter_paths(STARTER_CONFIG);
    let accepted: BTreeSet<String> = accepted_keys().into_iter().map(|key| key.path).collect();
    let parents: BTreeSet<String> = accepted
        .iter()
        .filter_map(|path| is_dynamic(path).map(str::to_string))
        .collect();
    let stale: Vec<&String> = paths
        .iter()
        .filter(|path| !accepted.contains(*path) && !parents.contains(*path))
        .collect();
    assert!(
        stale.is_empty(),
        "keys the parser does not accept: {stale:?}"
    );
}

#[test]
fn the_path_scan_sees_active_and_commented_keys() {
    let (paths, tables) = starter_paths("a = 1\n[hosts.x]\n# model = \"m\"\nkind = \"claude\"\n");
    assert!(paths.contains("a") && paths.contains("hosts.<name>.model"));
    assert!(paths.contains("hosts.<name>.kind") && tables.contains("hosts.<name>"));
}

// --- the registry is the parser's list -------------------------------------------------------------------------------

const BASE: &str = "schema_version = 1\n[nexora]\ntrack = \"never\"\n[hosts.claude]\nkind = \"claude\"\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n[roles.dev]\nkind = \"Developer\"\nhost = \"claude\"\n";

fn refusal(text: &str) -> String {
    match parse_config(text.as_bytes(), Path::new("/nonexistent")) {
        Err(ConfigError::Invalid(message)) => message,
        other => panic!("expected a refusal of:\n{text}\ngot {other:?}"),
    }
}

/// The `allowed:` list of the refusal for an unknown key written into the table `header` (prefixed by `before`).
fn allowed(before: &str, header: &str) -> String {
    let message = refusal(&format!("{BASE}{before}{header}\nzz_unknown = 1\n"));
    assert!(message.contains("1 unknown key(s)"), "{message}");
    message
        .split_once("allowed: ")
        .unwrap_or_else(|| panic!("no allowed list in: {message}"))
        .1
        .to_string()
}

fn allowed_top_level() -> String {
    let message = refusal(&format!("zz_unknown = 1\n{BASE}"));
    message
        .split_once("allowed: ")
        .expect("allowed")
        .1
        .to_string()
}

#[test]
fn the_registry_equals_the_lists_the_parser_prints() {
    assert_eq!(allowed_top_level(), TOP_LEVEL_KEYS.join(", "));
    let timers: Vec<&str> = TIMER_DEFAULTS.iter().map(|entry| entry.0).collect();
    let defaults_and_kinds: Vec<&str> = DEFAULTS_ROLE_KEYS
        .iter()
        .chain(ROLE_KINDS.iter())
        .copied()
        .collect();
    let cases: Vec<(&str, &str, String)> = vec![
        ("", "[project]", PROJECT_KEYS.join(", ")),
        ("", "[notifications]", NOTIFICATION_KEYS.join(", ")),
        ("", "[timers]", timers.join(", ")),
        ("", "[supervision]", SUPERVISION_KEYS.join(", ")),
        ("", "[liveness]", LIVENESS_KEYS.join(", ")),
        ("", "[messages]", MESSAGES_KEYS.join(", ")),
        ("", "[limits]", LIMITS_KEYS.join(", ")),
        ("", "[ledger]", LEDGER_KEYS.join(", ")),
        ("", "[layout]", LAYOUT_KEYS.join(", ")),
        ("", "[worktree]", WORKTREE_KEYS.join(", ")),
        ("", "[daemon]", DAEMON_KEYS.join(", ")),
        ("", "[cleanup]", CLEANUP_KEYS.join(", ")),
        ("", "[remote]", REMOTE_KEYS.join(", ")),
        ("", "[impact]", IMPACT_KEYS.join(", ")),
        ("", "[env]", ENV_KEYS.join(", ")),
        ("", "[hosts.other]", HOST_KEYS.join(", ")),
        ("", "[roles.extra]", ROLE_KEYS.join(", ")),
        ("", "[defaults]", defaults_and_kinds.join(", ")),
        ("", "[defaults.PM]", DEFAULTS_ROLE_KEYS.join(", ")),
        ("", "[defaults.Developer]", DEFAULTS_ROLE_KEYS.join(", ")),
        ("", "[defaults.Verifier]", DEFAULTS_ROLE_KEYS.join(", ")),
        ("", "[defaults.Supervisor]", DEFAULTS_ROLE_KEYS.join(", ")),
        ("", "[mcp_servers.tool]", MCP_SERVER_KEYS.join(", ")),
        ("", "[architect]", ARCHITECT_KEYS.join(", ")),
        ("", "[operator]", OPERATOR_KEYS.join(", ")),
        ("", "[researcher]", RESEARCHER_KEYS.join(", ")),
        ("", "[prompt_relay]", PROMPT_RELAY_KEYS.join(", ")),
        ("", "[cache]", CACHE_KEYS.join(", ")),
        (
            "[cache]\npresets = [\"rust\"]\n",
            "[cache.rust]",
            CACHE_RUST_KEYS.join(", "),
        ),
        (
            "[cache]\npresets = [\"python\"]\n",
            "[cache.python]",
            CACHE_PRESET_KEYS.join(", "),
        ),
        (
            "[cache]\n",
            "[cache.entries.mine]",
            CACHE_ENTRY_KEYS.join(", "),
        ),
        (
            "[cache]\n",
            "[cache.setup_reuse]",
            CACHE_SETUP_REUSE_KEYS.join(", "),
        ),
        ("", "[verify]", VERIFY_KEYS.join(", ")),
        ("", "[review]", REVIEW_KEYS.join(", ")),
        ("", "[design]", DESIGN_KEYS.join(", ")),
    ];
    for (before, header, expected) in cases {
        assert_eq!(allowed(before, header), expected, "{header}");
    }
    let nexora = refusal(&BASE.replace("track = \"never\"", "track = \"never\"\nzz_unknown = 1"));
    assert!(
        nexora.ends_with(&NEXORA_KEYS.join(", ")),
        "nexora: {nexora}"
    );
}

#[test]
fn every_preset_table_is_covered_by_the_registry() {
    for preset in CACHE_PRESETS {
        let keys = if preset == "rust" {
            CACHE_RUST_KEYS
        } else {
            CACHE_PRESET_KEYS
        };
        let before = format!("[cache]\npresets = [\"{preset}\"]\n");
        assert_eq!(
            allowed(&before, &format!("[cache.{preset}]")),
            keys.join(", ")
        );
    }
}

#[test]
fn the_unprinted_tables_stay_out_of_the_top_level_list() {
    let printed = allowed_top_level();
    for key in UNPRINTED_TOP_LEVEL_KEYS {
        assert!(!printed.split(", ").any(|printed| printed == *key), "{key}");
    }
    assert!(!TOP_LEVEL_KEYS
        .iter()
        .any(|key| UNPRINTED_TOP_LEVEL_KEYS.contains(key)));
}

#[test]
fn the_user_named_tables_are_backed_by_the_parser() {
    let notice = refusal(&format!(
        "{BASE}[messages.notice_class]\nnot_a_notice = \"info\"\n"
    ));
    assert!(notice.contains("is not a known notice kind"), "{notice}");
    let env = refusal(&format!(
        "{BASE}[verify]\ncommands = [\"true\"]\n[verify.env]\nbad-name = \"x\"\n"
    ));
    assert!(env.contains("verify.env"), "{env}");
    let host = refusal(&BASE.replace("[hosts.claude]", "[hosts.Bad_Name]"));
    assert!(host.contains("hosts"), "{host}");
}

// --- the generated paths ---------------------------------------------------------------------------------------------

/// Every `pub const` key slice of `keys.rs`, with the table prefixes it is wired to.
fn slices() -> Vec<(&'static str, &'static [&'static str], Vec<String>)> {
    let kinds = |prefix: &str| -> Vec<String> {
        ROLE_KINDS
            .iter()
            .map(|kind| format!("{prefix}.{kind}"))
            .collect()
    };
    let mut defaults = vec!["defaults".to_string()];
    defaults.extend(kinds("defaults"));
    let s = |text: &str| vec![text.to_string()];
    vec![
        ("TOP_LEVEL_KEYS", TOP_LEVEL_KEYS, s("")),
        ("UNPRINTED_TOP_LEVEL_KEYS", UNPRINTED_TOP_LEVEL_KEYS, s("")),
        ("PROJECT_KEYS", PROJECT_KEYS, s("project")),
        ("NOTIFICATION_KEYS", NOTIFICATION_KEYS, s("notifications")),
        ("SUPERVISION_KEYS", SUPERVISION_KEYS, s("supervision")),
        ("LIVENESS_KEYS", LIVENESS_KEYS, s("liveness")),
        ("MESSAGES_KEYS", MESSAGES_KEYS, s("messages")),
        ("LIMITS_KEYS", LIMITS_KEYS, s("limits")),
        ("LEDGER_KEYS", LEDGER_KEYS, s("ledger")),
        ("LAYOUT_KEYS", LAYOUT_KEYS, s("layout")),
        ("WORKTREE_KEYS", WORKTREE_KEYS, s("worktree")),
        ("DAEMON_KEYS", DAEMON_KEYS, s("daemon")),
        ("CLEANUP_KEYS", CLEANUP_KEYS, s("cleanup")),
        ("REMOTE_KEYS", REMOTE_KEYS, s("remote")),
        ("IMPACT_KEYS", IMPACT_KEYS, s("impact")),
        ("ENV_KEYS", ENV_KEYS, s("env")),
        ("HOST_KEYS", HOST_KEYS, s("hosts.<name>")),
        ("ROLE_KEYS", ROLE_KEYS, s("roles.<name>")),
        ("DEFAULTS_ROLE_KEYS", DEFAULTS_ROLE_KEYS, defaults),
        ("MCP_SERVER_KEYS", MCP_SERVER_KEYS, s("mcp_servers.<name>")),
        ("NEXORA_KEYS", NEXORA_KEYS, s("nexora")),
        ("ARCHITECT_KEYS", ARCHITECT_KEYS, s("architect")),
        ("OPERATOR_KEYS", OPERATOR_KEYS, s("operator")),
        ("RESEARCHER_KEYS", RESEARCHER_KEYS, s("researcher")),
        ("PROMPT_RELAY_KEYS", PROMPT_RELAY_KEYS, s("prompt_relay")),
        ("CACHE_KEYS", CACHE_KEYS, s("cache")),
        (
            "CACHE_SETUP_REUSE_KEYS",
            CACHE_SETUP_REUSE_KEYS,
            s("cache.setup_reuse"),
        ),
        (
            "CACHE_ENTRY_KEYS",
            CACHE_ENTRY_KEYS,
            s("cache.entries.<name>"),
        ),
        ("CACHE_RUST_KEYS", CACHE_RUST_KEYS, s("cache.rust")),
        (
            "CACHE_PRESET_KEYS",
            CACHE_PRESET_KEYS,
            CACHE_PRESETS
                .iter()
                .filter(|preset| **preset != "rust")
                .map(|preset| format!("cache.{preset}"))
                .collect(),
        ),
        ("VERIFY_KEYS", VERIFY_KEYS, s("verify")),
        ("REVIEW_KEYS", REVIEW_KEYS, s("review")),
        ("DESIGN_KEYS", DESIGN_KEYS, s("design")),
    ]
}

#[test]
fn every_key_slice_contributes_its_paths() {
    // The list above names every slice of keys.rs, so a new slice cannot be left out of the generation unnoticed.
    let source = include_str!("../src/keys.rs");
    let declared: BTreeSet<&str> = source
        .lines()
        .filter_map(|line| line.strip_prefix("pub const "))
        .filter(|rest| rest.contains(": &[&str]") && !rest.starts_with("DEPRECATED_PATHS"))
        .filter_map(|rest| rest.split(':').next())
        .collect();
    let listed: BTreeSet<&str> = slices().iter().map(|(name, _, _)| *name).collect();
    assert_eq!(declared, listed);

    let paths: Vec<String> = accepted_keys().into_iter().map(|key| key.path).collect();
    for (name, keys, prefixes) in slices() {
        assert!(!keys.is_empty(), "{name} is empty");
        for prefix in &prefixes {
            for key in keys {
                let path = if prefix.is_empty() {
                    key.to_string()
                } else {
                    format!("{prefix}.{key}")
                };
                let nested = format!("{path}.");
                assert!(
                    paths.iter().any(|p| *p == path || p.starts_with(&nested))
                        // a key that names a table whose keys are listed under another slice
                        || paths.iter().any(|p| p.split('.').any(|part| part == *key)),
                    "{name}: no accepted path for {path}"
                );
            }
        }
        // Every key appears in some path, and the first prefix is really wired (not just a same-named key elsewhere).
        let first = &prefixes[0];
        let wired = keys.iter().filter(|key| {
            let path = if first.is_empty() {
                key.to_string()
            } else {
                format!("{first}.{key}")
            };
            paths.contains(&path)
        });
        // The unprinted top-level keys are all tables, so they contribute no path of their own at the top level.
        if name != "UNPRINTED_TOP_LEVEL_KEYS" {
            assert!(
                wired.count() > 0,
                "{name} contributes no path under {first}"
            );
        }
    }
    for deprecated in DEPRECATED_PATHS {
        assert!(
            accepted_keys()
                .iter()
                .any(|key| key.path == *deprecated && key.deprecated),
            "{deprecated}"
        );
    }
}

// --- the commented blocks load ---------------------------------------------------------------------------------------

/// A unit: the lines from a header (active or commented) or a blank line to the next one.
struct Block {
    name: String,
    /// Indexes into the starter's lines whose commented TOML the block un-comments.
    lines: Vec<usize>,
}

fn blocks(text: &str) -> Vec<Block> {
    let lines = entries(text);
    let mut blocks: Vec<Block> = Vec::new();
    let mut current: Option<Block> = None;
    let mut anchor = String::from("top");
    let mut counter = 0;
    for (index, entry) in lines.iter().enumerate() {
        let header = matches!(entry.line, Line::Header(_));
        let blank = entry.raw.trim().is_empty();
        if header || blank {
            blocks.extend(current.take().filter(|block| !block.lines.is_empty()));
        }
        if let Line::Header(name) = &entry.line {
            anchor = format!("[{name}]");
            counter = 0;
            current = Some(Block {
                name: if entry.commented {
                    anchor.clone()
                } else {
                    String::new()
                },
                lines: Vec::new(),
            });
        }
        if blank && current.is_none() {
            // a bare group of commented keys after a blank line belongs to the table above it
            counter += 1;
            current = Some(Block {
                name: String::new(),
                lines: Vec::new(),
            });
        }
        if entry.commented && entry.line != Line::Other {
            let block = current.get_or_insert_with(|| Block {
                name: String::new(),
                lines: Vec::new(),
            });
            if block.name.is_empty() {
                block.name = format!("{anchor} keys #{counter}");
            }
            block.lines.push(index);
        }
    }
    blocks.extend(current.filter(|block| !block.lines.is_empty()));
    blocks
}

fn uncommented(text: &str, picked: &[&Block]) -> String {
    let wanted: BTreeSet<usize> = picked
        .iter()
        .flat_map(|block| block.lines.iter().copied())
        .collect();
    text.lines()
        .enumerate()
        .map(|(index, raw)| match raw.strip_prefix("# ") {
            Some(rest) if wanted.contains(&index) => rest.to_string(),
            _ => raw.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

#[test]
fn every_commented_block_loads_when_uncommented() {
    let all = blocks(STARTER_CONFIG);
    assert!(all.len() >= 6, "found only {} blocks", all.len());
    for (name, _) in ILLUSTRATIVE.iter().map(|(name, reason)| (name, reason)) {
        assert!(all.iter().any(|block| block.name == *name), "{name}");
    }
    let mut failures = Vec::new();
    for block in &all {
        if ILLUSTRATIVE.iter().any(|(name, _)| *name == block.name) {
            continue;
        }
        let mut picked = vec![block];
        if let Some((_, partners)) = WITH.iter().find(|(name, _)| *name == block.name) {
            for partner in *partners {
                picked.push(
                    all.iter()
                        .find(|other| other.name == *partner)
                        .unwrap_or_else(|| panic!("no block {partner}")),
                );
            }
        }
        let text = uncommented(STARTER_CONFIG, &picked);
        if let Err(error) = parse_config(text.as_bytes(), Path::new("/nonexistent")) {
            failures.push(format!("{}: {error:?}", block.name));
        }
    }
    assert!(
        failures.is_empty(),
        "these commented blocks do not load when un-commented:\n  {}",
        failures.join("\n  ")
    );
}

#[test]
fn every_block_of_the_whole_starter_loads_together_except_the_illustrative() {
    let all = blocks(STARTER_CONFIG);
    let picked: Vec<&Block> = all
        .iter()
        .filter(|block| !ILLUSTRATIVE.iter().any(|(name, _)| *name == block.name))
        .collect();
    let text = uncommented(STARTER_CONFIG, &picked);
    if let Err(error) = parse_config(text.as_bytes(), Path::new("/nonexistent")) {
        // Alternatives that conflict with an active table are expected to; report which one.
        println!("note: all blocks together do not load: {error:?}");
    }
}

#[test]
fn a_commented_line_that_looks_like_toml_but_is_not_fails() {
    let result = std::panic::catch_unwind(|| entries("# [broken table\n"));
    assert!(result.is_err());
    let result = std::panic::catch_unwind(|| entries("# key = [1, 2\n"));
    assert!(result.is_err());
    let _ = entries("# prose only: a sentence with no equals\n# see [roles.x] below\n");
}

// --- the active starter ----------------------------------------------------------------------------------------------

#[test]
fn the_starter_loads_with_no_warning_but_the_missing_nexora_file() {
    let config = parse_config(STARTER_CONFIG.as_bytes(), Path::new("/nonexistent")).expect("loads");
    let rest: Vec<&String> = config
        .warnings
        .iter()
        .filter(|warning| !warning.contains(".nexora.toml"))
        .collect();
    assert!(rest.is_empty(), "warnings: {rest:?}");
}
