//! The `[messages]` table: defaults, every key, refusals that name the key, and the notice kind table.
use capstan_config::{
    parse_config, starter::STARTER_CONFIG, ConfigError, MessageClassName, RoleConfig, WakePolicy,
    MESSAGE_NOTICE_KINDS,
};
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

fn load_text(text: &str) -> Result<RoleConfig, String> {
    parse_config(text.as_bytes(), Path::new("/nonexistent")).map_err(|error| match error {
        ConfigError::Invalid(message) => message,
        other => format!("{other:?}"),
    })
}

fn load(extra: &str) -> Result<RoleConfig, String> {
    load_text(&format!("{BASE}{extra}"))
}

fn refusal(extra: &str) -> String {
    load(extra).expect_err("refused")
}

#[test]
fn absent_section_gives_the_defaults() {
    let config = load("").expect("loads");
    let messages = &config.messages;
    assert_eq!(messages.wake, WakePolicy::Action);
    assert_eq!(messages.send_default, MessageClassName::Action);
    assert_eq!(messages.info_wake_after_seconds, 600);
    assert!(messages.auto_ack_info);
    assert!(messages.notice_class.is_empty());
    let json = config.to_json();
    assert!(json.contains("\"messages\""), "{json}");
    assert!(json.contains("\"infoWakeAfterSeconds\": 600"), "{json}");
    assert_eq!(load("[messages]\n").unwrap(), config);
}

#[test]
fn every_key_is_read() {
    let config = load(
        "[messages]\nwake = \"all\"\nsend_default = \"info\"\ninfo_wake_after_seconds = 0\nauto_ack_info = false\n\
         [messages.notice_class]\nfinding_resolved = \"action\"\npause_notice = \"info\"\n",
    )
    .expect("loads");
    let messages = &config.messages;
    assert_eq!(messages.wake, WakePolicy::All);
    assert_eq!(messages.send_default, MessageClassName::Info);
    assert_eq!(messages.info_wake_after_seconds, 0);
    assert!(!messages.auto_ack_info);
    assert_eq!(
        messages.notice_class.get("finding_resolved"),
        Some(&MessageClassName::Action)
    );
    assert_eq!(
        messages.notice_class.get("pause_notice"),
        Some(&MessageClassName::Info)
    );
    assert_ne!(config, load("").unwrap());
}

#[test]
fn bad_values_are_refused_with_the_key() {
    for (extra, key) in [
        ("[messages]\nwake = \"some\"\n", "messages.wake"),
        (
            "[messages]\nsend_default = \"x\"\n",
            "messages.send_default",
        ),
        (
            "[messages]\ninfo_wake_after_seconds = -1\n",
            "messages.info_wake_after_seconds",
        ),
        (
            "[messages]\ninfo_wake_after_seconds = \"5\"\n",
            "messages.info_wake_after_seconds",
        ),
        (
            "[messages]\nauto_ack_info = \"yes\"\n",
            "messages.auto_ack_info",
        ),
        (
            "[messages.notice_class]\nunknown_kind = \"info\"\n",
            "messages.notice_class.unknown_kind",
        ),
        (
            "[messages.notice_class]\ndelivery_problem = \"info\"\n",
            "messages.notice_class.delivery_problem",
        ),
        (
            "[messages.notice_class]\nfinding_resolved = \"loud\"\n",
            "messages.notice_class.finding_resolved",
        ),
    ] {
        let message = refusal(extra);
        assert!(message.contains(key), "{extra:?}: {message}");
    }
    assert!(refusal("[messages]\nwak = \"all\"\n").contains("messages"));
}

#[test]
fn the_notice_table_has_unique_names() {
    let mut names: Vec<&str> = MESSAGE_NOTICE_KINDS.iter().map(|(name, _)| *name).collect();
    let total = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), total);
    assert!(MESSAGE_NOTICE_KINDS.contains(&("delivery_problem", false)));
    assert!(MESSAGE_NOTICE_KINDS.contains(&("finding_resolved", true)));
}

#[test]
fn the_starter_sets_no_messages_table_and_gets_the_defaults() {
    assert!(!STARTER_CONFIG.lines().any(|l| l.starts_with("[messages]")));
    let config = parse_config(
        STARTER_CONFIG.as_bytes(),
        std::path::Path::new("/nonexistent"),
    )
    .expect("the starter loads");
    assert_eq!(config.messages, load("").unwrap().messages);
}
